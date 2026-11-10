//! Decoding for browser previews: a fast "head" that is cached, a streamed "tail" for long
//! files, and the cache that makes switching between files immediate.
//!
//! * [`decode_head`] decodes a short file completely, or the first few seconds of a long one, to
//!   stereo at the device rate, stopping as soon as it has enough.
//! * [`stream_tail`] continues a long file after its head into a [`PreviewStream`], staying a few
//!   seconds ahead of the reader. The tail continues the head sample-exactly.
//! * [`PreviewCache`] keeps decoded heads in a byte-bounded least-recently-used cache, shares a
//!   decode between concurrent requests for the same file, lets a superseded decode be cancelled,
//!   and remembers recent failures so a broken file is not retried in a loop.
//!
//! The playback voice mixer lives in st-audio. What it needs from here is designed for the audio
//! thread:
//!
//! * [`PreviewPcm`] is immutable; the mixer reads its slices. Evicting an entry from the cache
//!   only drops the cache's reference.
//! * [`PreviewStreamReader`] reads without locking, allocating or freeing: blocks are allocated
//!   and released by the writer thread only.
//! * Neither type frees memory on the audio thread as long as a clone of the `Arc<PreviewPcm>`
//!   and the [`PreviewStream`] handle are kept on a control thread until the voice has retired
//!   (dropping a reference that is not the last one is a single atomic decrement).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::decode::AudioFileReader;
use crate::error::{file_name, Result, SampleError};
use crate::op::CancelToken;
use crate::resample::{resampled_frames, ResampleQuality, Resampler};

/// Stereo PCM at the device rate: the cached part of a preview. Immutable.
#[derive(Debug)]
pub struct PreviewPcm {
    left: Box<[f32]>,
    right: Box<[f32]>,
    complete: bool,
    estimated_total_frames: Option<usize>,
    duration: f64,
    sample_rate: u32,
}

impl PreviewPcm {
    /// Wraps decoded audio. `complete` says whether it is the whole file;
    /// `estimated_total_frames` is the whole file's length at `sample_rate` when known, and
    /// `duration` the whole file's length in seconds.
    pub fn new(left: Vec<f32>, right: Vec<f32>, complete: bool, estimated_total_frames: Option<usize>, duration: f64, sample_rate: u32) -> Self {
        let frames = left.len().min(right.len());
        let trim = |mut channel: Vec<f32>| {
            channel.truncate(frames);
            channel.into_boxed_slice()
        };
        PreviewPcm { left: trim(left), right: trim(right), complete, estimated_total_frames, duration, sample_rate }
    }

    /// Left channel.
    pub fn left(&self) -> &[f32] {
        &self.left
    }

    /// Right channel (the same length as the left).
    pub fn right(&self) -> &[f32] {
        &self.right
    }

    /// Frames held.
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// True when the frames held are the whole file; otherwise a tail stream continues them.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Length of the whole file in frames at this sample rate, when the file states its length.
    pub fn estimated_total_frames(&self) -> Option<usize> {
        self.estimated_total_frames
    }

    /// Length of the whole file in seconds.
    pub fn duration(&self) -> f64 {
        self.duration
    }

    /// Frames per second.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Bytes of audio held (what the cache counts).
    pub fn bytes(&self) -> usize {
        self.left.len() * 2 * std::mem::size_of::<f32>()
    }
}

struct StreamShared {
    chunk_frames: usize,
    /// Each block holds `chunk_frames` left samples followed by `chunk_frames` right samples.
    chunks: Box<[AtomicPtr<f32>]>,
    published: AtomicUsize,
    read_position: AtomicUsize,
    underruns: AtomicUsize,
    finished: AtomicBool,
    cancelled: AtomicBool,
    writer_taken: AtomicBool,
    reader_taken: AtomicBool,
}

impl StreamShared {
    fn release(&self, index: usize) {
        let pointer = self.chunks[index].swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pointer.is_null() {
            // SAFETY: the pointer came from `Box::<[f32]>::into_raw` of exactly this length in
            // `PreviewStreamWriter::append` and is swapped out before being freed, so it is
            // freed once.
            drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(pointer, self.chunk_frames * 2)) });
        }
    }
}

impl Drop for StreamShared {
    fn drop(&mut self) {
        for index in 0..self.chunks.len() {
            self.release(index);
        }
    }
}

/// An append-only stream of stereo PCM for the part of a long preview after its cached head:
/// one writer (a decoding thread), one reader (the audio thread).
///
/// This handle is the control side: it reports state, can cancel, and hands out the writer and
/// the reader once each. Keep it (or a clone) on a control thread until the reader is no longer
/// used, so that the stream's memory is never released on the audio thread.
#[derive(Clone)]
pub struct PreviewStream {
    shared: Arc<StreamShared>,
}

impl PreviewStream {
    /// Default block size in frames.
    pub const CHUNK_FRAMES: usize = 16_384;

    /// A stream that can hold `estimated_frames` frames (plus some slack) in blocks of
    /// `chunk_frames` frames.
    pub fn new(estimated_frames: usize, chunk_frames: usize) -> Self {
        let chunk_frames = chunk_frames.max(1);
        let slots = (estimated_frames.div_ceil(chunk_frames) + 8).max(4);
        let chunks = (0..slots).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect();
        PreviewStream {
            shared: Arc::new(StreamShared {
                chunk_frames,
                chunks,
                published: AtomicUsize::new(0),
                read_position: AtomicUsize::new(0),
                underruns: AtomicUsize::new(0),
                finished: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                writer_taken: AtomicBool::new(false),
                reader_taken: AtomicBool::new(false),
            }),
        }
    }

    /// A stream sized for the rest of the file `head` begins (one second of slack; three hours
    /// when the file does not state its length).
    pub fn for_tail_of(head: &PreviewPcm) -> Self {
        let rate = head.sample_rate() as usize;
        let estimated = match head.estimated_total_frames() {
            Some(total) => total.saturating_sub(head.frames()) + rate,
            None => rate * 3 * 3600,
        };
        Self::new(estimated, Self::CHUNK_FRAMES)
    }

    /// The writing end. `None` after the first call.
    pub fn writer(&self) -> Option<PreviewStreamWriter> {
        (!self.shared.writer_taken.swap(true, Ordering::AcqRel)).then(|| PreviewStreamWriter { shared: self.shared.clone(), freed: 0 })
    }

    /// The reading end. `None` after the first call.
    pub fn reader(&self) -> Option<PreviewStreamReader> {
        (!self.shared.reader_taken.swap(true, Ordering::AcqRel)).then(|| PreviewStreamReader { shared: self.shared.clone(), position: 0 })
    }

    /// Frames written so far.
    pub fn published(&self) -> usize {
        self.shared.published.load(Ordering::Acquire)
    }

    /// Frames the reader has consumed.
    pub fn read_position(&self) -> usize {
        self.shared.read_position.load(Ordering::Acquire)
    }

    /// True once the writer has delivered everything it will deliver.
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    /// True once either side gave the stream up.
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }

    /// Times the reader wanted audio that had not been decoded yet.
    pub fn underruns(&self) -> usize {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Tells the writer to stop. Safe from any thread, including the audio thread.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
    }
}

/// The writing end of a [`PreviewStream`]. Lives on a background thread; allocates and frees.
pub struct PreviewStreamWriter {
    shared: Arc<StreamShared>,
    freed: usize,
}

impl PreviewStreamWriter {
    /// Appends frames. Returns false when the stream was cancelled or is full.
    pub fn append(&mut self, left: &[f32], right: &[f32]) -> bool {
        let shared = &*self.shared;
        let count = left.len().min(right.len());
        let chunk_frames = shared.chunk_frames;
        let mut written = 0;
        let mut total = shared.published.load(Ordering::Acquire);
        while written < count {
            if shared.cancelled.load(Ordering::Acquire) {
                return false;
            }
            let (chunk, offset) = (total / chunk_frames, total % chunk_frames);
            if chunk >= shared.chunks.len() {
                return false;
            }
            let mut storage = shared.chunks[chunk].load(Ordering::Acquire);
            if storage.is_null() {
                storage = Box::into_raw(vec![0f32; chunk_frames * 2].into_boxed_slice()) as *mut f32;
                shared.chunks[chunk].store(storage, Ordering::Release);
            }
            let n = (count - written).min(chunk_frames - offset);
            // SAFETY: `storage` points to `2 * chunk_frames` floats owned by the stream. The
            // ranges written lie beyond `published`, where the reader does not look yet, and
            // only this single writer writes.
            unsafe {
                std::ptr::copy_nonoverlapping(left[written..].as_ptr(), storage.add(offset), n);
                std::ptr::copy_nonoverlapping(right[written..].as_ptr(), storage.add(chunk_frames + offset), n);
            }
            written += n;
            total += n;
            shared.published.store(total, Ordering::Release);
        }
        self.release_consumed();
        true
    }

    /// Frees blocks the reader passed at least one whole block ago.
    pub fn release_consumed(&mut self) {
        let passed = self.shared.read_position.load(Ordering::Acquire) / self.shared.chunk_frames;
        while self.freed + 1 < passed && self.freed < self.shared.chunks.len() {
            self.shared.release(self.freed);
            self.freed += 1;
        }
    }

    /// Marks the stream complete: the reader ends when it has played everything written.
    pub fn finish(&mut self) {
        self.shared.finished.store(true, Ordering::Release);
    }

    /// True once the stream was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.shared.cancelled.load(Ordering::Acquire)
    }

    /// Frames written and not yet consumed by the reader.
    pub fn lead(&self) -> usize {
        self.shared.published.load(Ordering::Acquire).saturating_sub(self.shared.read_position.load(Ordering::Acquire))
    }
}

impl Drop for PreviewStreamWriter {
    fn drop(&mut self) {
        // A writer that goes away (also by panicking) must not leave the reader waiting.
        self.finish();
    }
}

/// What a [`PreviewStreamReader`] can deliver right now.
#[derive(Debug, PartialEq)]
pub enum StreamAudio<'a> {
    /// Contiguous frames at the read position. Take some, then call
    /// [`PreviewStreamReader::advance`].
    Frames {
        /// Left channel.
        left: &'a [f32],
        /// Right channel, the same length.
        right: &'a [f32],
    },
    /// Nothing decoded yet at the read position; output silence and try again next block.
    Pending,
    /// Everything was played, or the stream was given up.
    Ended,
}

/// The reading end of a [`PreviewStream`], for the audio thread: wait-free, no locks, no
/// allocation, and it never frees stream memory while another handle to the stream exists.
pub struct PreviewStreamReader {
    shared: Arc<StreamShared>,
    position: usize,
}

impl PreviewStreamReader {
    /// The audio available at the read position.
    pub fn available(&self) -> StreamAudio<'_> {
        let shared = &*self.shared;
        let published = shared.published.load(Ordering::Acquire);
        if self.position >= published {
            return if shared.finished.load(Ordering::Acquire) && self.position >= shared.published.load(Ordering::Acquire) {
                StreamAudio::Ended
            } else {
                StreamAudio::Pending
            };
        }
        let (chunk, offset) = (self.position / shared.chunk_frames, self.position % shared.chunk_frames);
        let storage = shared.chunks[chunk].load(Ordering::Acquire);
        if storage.is_null() {
            return StreamAudio::Ended;
        }
        let count = (published - self.position).min(shared.chunk_frames - offset);
        // SAFETY: frames below `published` in this block were fully written before `published`
        // was stored. The writer frees a block only after the read position has moved at least
        // one whole block past it, and the position only moves in `advance`, which needs
        // `&mut self` and so cannot run while these slices are borrowed.
        unsafe {
            StreamAudio::Frames {
                left: std::slice::from_raw_parts(storage.add(offset), count),
                right: std::slice::from_raw_parts(storage.add(shared.chunk_frames + offset), count),
            }
        }
    }

    /// Moves the read position forward by `frames` (at most what [`Self::available`] offered).
    pub fn advance(&mut self, frames: usize) {
        let shared = &*self.shared;
        let published = shared.published.load(Ordering::Acquire);
        let room = shared.chunk_frames - self.position % shared.chunk_frames;
        self.position += frames.min(room).min(published.saturating_sub(self.position));
        shared.read_position.store(self.position, Ordering::Release);
    }

    /// Copies up to `left.len()` frames into the buffers and advances. Returns the number of
    /// frames copied and whether the stream has ended. A shortfall before the end is counted as
    /// an underrun; the caller outputs silence for the rest of its block.
    pub fn read(&mut self, left: &mut [f32], right: &mut [f32]) -> (usize, bool) {
        let wanted = left.len().min(right.len());
        let mut done = 0;
        while done < wanted {
            let taken = match self.available() {
                StreamAudio::Frames { left: l, right: r } => {
                    let n = l.len().min(wanted - done);
                    left[done..done + n].copy_from_slice(&l[..n]);
                    right[done..done + n].copy_from_slice(&r[..n]);
                    n
                }
                StreamAudio::Pending => {
                    self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                    return (done, false);
                }
                StreamAudio::Ended => return (done, true),
            };
            self.advance(taken);
            done += taken;
        }
        (done, false)
    }

    /// Frames consumed so far.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Tells the writer to stop decoding (the voice ended early).
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for PreviewStreamReader {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// True when `path` is on a `/Volumes/<name>` drive that is not mounted (macOS). Checking this
/// first turns a long timeout on a disconnected drive into an immediate, clear error.
pub fn volume_offline(path: &str) -> bool {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some("Volumes"), Some(volume)) => !Path::new("/Volumes").join(volume).is_dir(),
        _ => false,
    }
}

fn offline(path: &str) -> SampleError {
    SampleError::Offline(format!("The drive holding “{}” is offline.", file_name(path)))
}

/// Decodes `path` to stereo at `target_rate` and hands the frames from output frame `skip` on
/// to `emit`, at most `limit` frames. `emit` returns false to stop.
///
/// Returns true when the end of the file was reached. Frames are identical however the decode
/// is split: a call with `skip = n` continues a call with `limit = n` sample-exactly.
pub fn decode_preview(path: &Path, target_rate: u32, skip: usize, limit: usize, cancel: &CancelToken, emit: &mut dyn FnMut(&[f32], &[f32]) -> bool) -> Result<bool> {
    let mut reader = AudioFileReader::open(path)?;
    let source_rate = reader.spec().sample_rate;
    let mut resampler = Resampler::new(source_rate, target_rate, 2, ResampleQuality::Preview)?;
    resampler.skip_to_output(skip as u64);
    let mut converted = [Vec::new(), Vec::new()];
    let mut emitted = 0usize;
    let mut deliver = |converted: &mut [Vec<f32>; 2], emitted: &mut usize| -> bool {
        let count = converted[0].len().min(limit - *emitted);
        let more = if count > 0 {
            *emitted += count;
            emit(&converted[0][..count], &converted[1][..count]) && *emitted < limit
        } else {
            *emitted < limit
        };
        converted.iter_mut().for_each(Vec::clear);
        more
    };
    loop {
        cancel.check()?;
        let Some(block) = reader.next_block()? else { break };
        let stereo = [&block[0], block.get(1).unwrap_or(&block[0])];
        resampler.process(&stereo, &mut converted);
        if !deliver(&mut converted, &mut emitted) {
            return Ok(false);
        }
    }
    resampler.finish(&mut converted);
    deliver(&mut converted, &mut emitted);
    Ok(true)
}

/// Decodes the part of `path` a preview starts from: the whole file when it is at most
/// `full_seconds` long, otherwise its first `head_seconds`.
pub fn decode_head(path: &Path, target_rate: u32, head_seconds: f64, full_seconds: f64, cancel: &CancelToken) -> Result<PreviewPcm> {
    let shown = path.to_string_lossy();
    if volume_offline(&shown) {
        return Err(offline(&shown));
    }
    let spec = crate::decode::probe_file(path)?;
    let estimated = spec.frames.and_then(|frames| resampled_frames(frames, spec.sample_rate, target_rate)).map(|frames| frames as usize);
    let whole = estimated.is_some_and(|frames| frames as f64 <= full_seconds * target_rate as f64);
    let limit = match estimated {
        Some(frames) if whole => frames + 4096,
        _ => (head_seconds * target_rate as f64) as usize,
    };
    let capacity = limit.min(estimated.unwrap_or(limit));
    let (mut left, mut right) = (Vec::with_capacity(capacity), Vec::with_capacity(capacity));
    let ended = decode_preview(path, target_rate, 0, limit, cancel, &mut |l, r| {
        left.extend_from_slice(l);
        right.extend_from_slice(r);
        true
    })?;
    let duration = spec.duration().unwrap_or(left.len() as f64 / target_rate as f64);
    Ok(PreviewPcm::new(left, right, ended || whole, estimated, duration, target_rate))
}

/// Feeds `writer` with the frames of `path` after `head`. Blocks the calling (background)
/// thread, staying at most `lead_seconds` ahead of the reader, until the file ends or the
/// stream is cancelled; then marks the stream finished.
pub fn stream_tail(path: &Path, head: &PreviewPcm, mut writer: PreviewStreamWriter, lead_seconds: f64) {
    let lead_frames = (lead_seconds * head.sample_rate() as f64) as usize;
    let cancel = CancelToken::new();
    let result = decode_preview(path, head.sample_rate(), head.frames(), usize::MAX, &cancel, &mut |left, right| {
        while !writer.is_cancelled() && writer.lead() > lead_frames {
            writer.release_consumed();
            std::thread::sleep(Duration::from_millis(10));
        }
        writer.append(left, right)
    });
    // A read error mid-file ends the preview early; there is nobody to report it to here.
    drop(result);
    writer.finish();
}

/// Identifies one decoded head: the file (by path, size and modification time, so an edited
/// file is decoded again) and the rate it was converted to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewKey {
    path: String,
    size: u64,
    modified: Option<Duration>,
    rate: u32,
}

/// A head from the cache.
#[derive(Debug, Clone)]
pub struct PreviewHead {
    /// The decoded audio, shared with the cache and any voice playing it.
    pub pcm: Arc<PreviewPcm>,
    /// True when it came from the cache without decoding.
    pub cached: bool,
}

/// Counters for the preview status display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCacheStats {
    /// Heads held.
    pub cached_previews: usize,
    /// Bytes of audio held.
    pub cache_bytes: usize,
    /// Requests answered from the cache.
    pub cache_hits: u64,
    /// Requests that started a decode.
    pub cache_misses: u64,
}

struct Entry {
    pcm: Arc<PreviewPcm>,
    used: u64,
}

/// A decode in progress that other requests for the same key wait for.
struct Flight {
    outcome: Mutex<Option<Result<Arc<PreviewPcm>>>>,
    landed: Condvar,
}

/// Publishes a flight's outcome even if the decode panics, so that joiners never wait forever.
struct Landing<'a> {
    flight: &'a Flight,
    outcome: Option<Result<Arc<PreviewPcm>>>,
}

impl Drop for Landing<'_> {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or(Err(SampleError::Cancelled));
        *self.flight.outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
        self.flight.landed.notify_all();
    }
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<PreviewKey, Entry>,
    flights: HashMap<PreviewKey, Arc<Flight>>,
    failures: HashMap<String, Instant>,
    clock: u64,
    bytes: usize,
    hits: u64,
    misses: u64,
}

/// A memory-capped least-recently-used cache of decoded preview heads, with sharing of
/// in-flight decodes and a short negative cache.
///
/// All methods may be called from any thread; [`PreviewCache::head`] blocks while it decodes
/// or waits, so call it from a worker thread.
pub struct PreviewCache {
    capacity_bytes: usize,
    head_seconds: f64,
    full_seconds: f64,
    state: Mutex<CacheState>,
}

impl Default for PreviewCache {
    /// 64 MB; files up to 8 s are cached whole, longer ones by their first 4 s.
    fn default() -> Self {
        Self::new(64 << 20, 4.0, 8.0)
    }
}

impl PreviewCache {
    /// How long a failed file is not retried by prefetching.
    pub const FAILURE_MEMORY: Duration = Duration::from_secs(30);

    /// A cache holding at most `capacity_bytes` of audio (always at least the newest head).
    pub fn new(capacity_bytes: usize, head_seconds: f64, full_seconds: f64) -> Self {
        PreviewCache { capacity_bytes, head_seconds, full_seconds, state: Mutex::default() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The key of `path` at `rate`; `None` when it is not a regular file.
    pub fn key(&self, path: &str, rate: u32) -> Option<PreviewKey> {
        let metadata = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
        Some(PreviewKey { path: path.to_string(), size: metadata.len(), modified, rate })
    }

    /// The cached head for `key`, marking it as just used.
    pub fn cached(&self, key: &PreviewKey) -> Option<Arc<PreviewPcm>> {
        let mut state = self.lock();
        state.clock += 1;
        let clock = state.clock;
        let entry = state.entries.get_mut(key)?;
        entry.used = clock;
        Some(entry.pcm.clone())
    }

    /// Whether decoding `path` failed within the last `within`.
    pub fn recently_failed(&self, path: &str, within: Duration) -> bool {
        let mut state = self.lock();
        match state.failures.get(path) {
            Some(when) if when.elapsed() <= within => true,
            Some(_) => {
                state.failures.remove(path);
                false
            }
            None => false,
        }
    }

    /// Adds a head, evicting the least recently used ones while the cache is over its limit.
    pub fn insert(&self, pcm: Arc<PreviewPcm>, key: &PreviewKey) {
        let mut evicted = Vec::new();
        {
            let mut state = self.lock();
            state.clock += 1;
            let clock = state.clock;
            state.bytes += pcm.bytes();
            if let Some(old) = state.entries.insert(key.clone(), Entry { pcm, used: clock }) {
                state.bytes -= old.pcm.bytes();
                evicted.push(old.pcm);
            }
            state.failures.remove(&key.path);
            while state.bytes > self.capacity_bytes && state.entries.len() > 1 {
                let Some(oldest) = state.entries.iter().filter(|(k, _)| *k != key).min_by_key(|(_, e)| e.used).map(|(k, _)| k.clone()) else { break };
                if let Some(entry) = state.entries.remove(&oldest) {
                    state.bytes -= entry.pcm.bytes();
                    evicted.push(entry.pcm);
                }
            }
        }
        // Evicted audio is released here, outside the lock.
        drop(evicted);
    }

    /// Heads held.
    pub fn count(&self) -> usize {
        self.lock().entries.len()
    }

    /// Bytes of audio held.
    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Counters for the status display.
    pub fn stats(&self) -> PreviewCacheStats {
        let state = self.lock();
        PreviewCacheStats { cached_previews: state.entries.len(), cache_bytes: state.bytes, cache_hits: state.hits, cache_misses: state.misses }
    }

    fn note_failure(&self, path: &str) {
        self.lock().failures.insert(path.to_string(), Instant::now());
    }

    fn end_flight(&self, key: &PreviewKey, flight: &Arc<Flight>) {
        let mut state = self.lock();
        if state.flights.get(key).is_some_and(|current| Arc::ptr_eq(current, flight)) {
            state.flights.remove(key);
        }
    }

    /// Returns the cached head of `path` at `rate`, joins a decode already running for it, or
    /// decodes it.
    ///
    /// `cancel` belongs to the caller: cancel it when the request is superseded (a newer
    /// selection, a replaced prefetch queue) and the call returns [`SampleError::Cancelled`]
    /// between decode blocks. A request that had joined a decode whose owner was cancelled
    /// decodes again on its own instead of failing. Failures other than cancellation are
    /// remembered for [`PreviewCache::recently_failed`].
    pub fn head(&self, path: &str, rate: u32, cancel: &CancelToken) -> Result<PreviewHead> {
        if volume_offline(path) {
            return Err(offline(path));
        }
        let Some(key) = self.key(path, rate) else {
            self.note_failure(path);
            return Err(SampleError::NotFound(format!("The preview file is missing: {}", file_name(path))));
        };
        enum Step {
            Hit(Arc<PreviewPcm>),
            Join(Arc<Flight>),
            Decode(Arc<Flight>),
        }
        for _ in 0..3 {
            let step = {
                let mut state = self.lock();
                state.clock += 1;
                let clock = state.clock;
                if let Some(entry) = state.entries.get_mut(&key) {
                    entry.used = clock;
                    let pcm = entry.pcm.clone();
                    state.hits += 1;
                    Step::Hit(pcm)
                } else if let Some(running) = state.flights.get(&key) {
                    Step::Join(running.clone())
                } else {
                    state.misses += 1;
                    let flight = Arc::new(Flight { outcome: Mutex::new(None), landed: Condvar::new() });
                    state.flights.insert(key.clone(), flight.clone());
                    Step::Decode(flight)
                }
            };
            match step {
                Step::Hit(pcm) => return Ok(PreviewHead { pcm, cached: true }),
                Step::Join(flight) => {
                    let mut outcome = flight.outcome.lock().unwrap_or_else(|e| e.into_inner());
                    while outcome.is_none() {
                        if cancel.is_cancelled() {
                            return Err(SampleError::Cancelled);
                        }
                        outcome = flight.landed.wait_timeout(outcome, Duration::from_millis(20)).unwrap_or_else(|e| e.into_inner()).0;
                    }
                    match outcome.clone().expect("checked above") {
                        Ok(pcm) => return Ok(PreviewHead { pcm, cached: false }),
                        Err(SampleError::Cancelled) if !cancel.is_cancelled() => {
                            // The joined decode was cancelled with its owner (a superseded
                            // prefetch). Forget it here: the owner may not have removed it yet,
                            // and joining it again would fail the same way.
                            drop(outcome);
                            self.end_flight(&key, &flight);
                        }
                        Err(error) => return Err(error),
                    }
                }
                Step::Decode(flight) => {
                    let mut landing = Landing { flight: &flight, outcome: None };
                    let decoded = decode_head(Path::new(path), rate, self.head_seconds, self.full_seconds, cancel).map(Arc::new);
                    self.end_flight(&key, &flight);
                    match &decoded {
                        Ok(pcm) => self.insert(pcm.clone(), &key),
                        Err(error) if !error.is_cancelled() => self.note_failure(path),
                        Err(_) => {}
                    }
                    landing.outcome = Some(decoded.clone());
                    drop(landing);
                    return decoded.map(|pcm| PreviewHead { pcm, cached: false });
                }
            }
        }
        Err(SampleError::Cancelled)
    }
}

/// Which neighbouring items to warm when item `selected` of `count` is selected: the item
/// itself, then its neighbours nearest first up to `radius` away, the one in the direction of
/// travel (`direction` < 0 is upwards) before the one behind.
pub fn prefetch_order(selected: usize, count: usize, direction: i32, radius: usize) -> Vec<usize> {
    if selected >= count {
        return Vec::new();
    }
    let mut result = vec![selected];
    let ahead: i64 = if direction < 0 { -1 } else { 1 };
    for distance in 1..=radius.max(1) as i64 {
        for index in [selected as i64 + ahead * distance, selected as i64 - ahead * distance] {
            if index >= 0 && (index as usize) < count {
                result.push(index as usize);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_blocks_are_published_read_and_released() {
        let stream = PreviewStream::new(30_000, 4096);
        let mut writer = stream.writer().unwrap();
        let mut reader = stream.reader().unwrap();
        assert!(stream.writer().is_none() && stream.reader().is_none(), "one writer, one reader");
        assert_eq!(reader.available(), StreamAudio::Pending);
        let left: Vec<f32> = (0..20_000).map(|i| i as f32).collect();
        let right: Vec<f32> = left.iter().map(|v| -v).collect();
        assert!(writer.append(&left[..7000], &right[..7000]));
        assert_eq!(stream.published(), 7000);
        let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
        let mut collected = Vec::new();
        loop {
            let (n, ended) = reader.read(&mut l, &mut r);
            assert!(l[..n].iter().zip(&r[..n]).all(|(a, b)| *a == -*b));
            collected.extend_from_slice(&l[..n]);
            if n < 256 {
                assert!(!ended, "not finished: a shortfall is an underrun");
                break;
            }
        }
        assert_eq!((collected.len(), stream.underruns(), stream.read_position()), (7000, 1, 7000));
        assert!(writer.append(&left[7000..], &right[7000..]));
        writer.finish();
        loop {
            let (n, ended) = reader.read(&mut l, &mut r);
            collected.extend_from_slice(&l[..n]);
            if ended {
                break;
            }
        }
        assert_eq!(collected, left);
        assert!(stream.is_finished() && reader.position() == 20_000);
        // Blocks behind the reader were released by the writer, not by the reader.
        writer.release_consumed();
        assert!(stream.shared.chunks[0].load(Ordering::Acquire).is_null() && stream.shared.chunks[2].load(Ordering::Acquire).is_null());
        assert!(!stream.shared.chunks[4].load(Ordering::Acquire).is_null(), "the block being read stays");
        // The stream is bounded.
        let small = PreviewStream::new(0, 16);
        let mut writer = small.writer().unwrap();
        assert!(!writer.append(&vec![0.0; 16 * 9], &vec![0.0; 16 * 9]), "out of block slots");
        // Dropping the reader tells the writer to stop.
        let dropped = PreviewStream::new(1000, 64);
        let mut writer = dropped.writer().unwrap();
        drop(dropped.reader());
        assert!(dropped.is_cancelled() && !writer.append(&[0.0; 8], &[0.0; 8]));
    }

    #[test]
    fn stream_survives_a_writer_and_reader_on_different_threads() {
        let stream = PreviewStream::new(400_000, 1024);
        let mut writer = stream.writer().unwrap();
        let mut reader = stream.reader().unwrap();
        let total = 400_000usize;
        let producer = std::thread::spawn(move || {
            let mut next = 0usize;
            while next < total {
                let n = (total - next).min(777);
                let block: Vec<f32> = (next..next + n).map(|i| (i % 65_536) as f32).collect();
                assert!(writer.append(&block, &block));
                next += n;
            }
            writer.finish();
        });
        let (mut l, mut r) = (vec![0f32; 300], vec![0f32; 300]);
        let mut expected = 0usize;
        loop {
            let (n, ended) = reader.read(&mut l, &mut r);
            for i in 0..n {
                assert_eq!(l[i], ((expected + i) % 65_536) as f32);
                assert_eq!(l[i], r[i]);
            }
            expected += n;
            if ended {
                break;
            }
            if n == 0 {
                std::thread::yield_now();
            }
        }
        producer.join().unwrap();
        assert_eq!(expected, total);
    }

    #[test]
    fn prefetch_order_is_nearest_first_and_biased_forward() {
        assert_eq!(prefetch_order(5, 20, 1, 3), vec![5, 6, 4, 7, 3, 8, 2]);
        assert_eq!(prefetch_order(5, 20, -1, 2), vec![5, 4, 6, 3, 7]);
        assert_eq!(prefetch_order(0, 3, -1, 3), vec![0, 1, 2]);
        assert_eq!(prefetch_order(3, 3, 1, 3), Vec::<usize>::new());
        assert_eq!(prefetch_order(0, 0, 1, 3), Vec::<usize>::new());
        assert_eq!(prefetch_order(1, 3, 1, 0), vec![1, 2, 0]);
    }

    #[test]
    fn offline_volumes_are_recognised_by_path() {
        assert!(volume_offline(&format!("/Volumes/SuperTrackerMissingDrive-{}/a.wav", std::process::id())));
        assert!(!volume_offline("/tmp/a.wav"));
        assert!(!volume_offline("/Volumes"));
        assert!(!volume_offline("relative/Volumes/x/a.wav"));
    }
}
