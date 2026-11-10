//! Browser preview playback: a small voice mixer on the render thread, fed from
//! `st-samples`' preview cache.
//!
//! A preview is a cached *head* (a short file completely, or the first seconds of a long one;
//! [`st_samples::preview::PreviewCache`]) and, for a long file, a *tail* that a decoding thread
//! streams a few seconds ahead of the playhead ([`st_samples::preview::PreviewStream`]). The
//! mixer plays the head and then the stream, sample-exactly continuous. Starting a preview
//! fades the previous one out (12 ms) while the new one fades in (2 ms), so stepping through a
//! list never clicks. The voices are mixed into the output after the master fader
//! ([`crate::live`]): the project's master volume, inserts and meters do not see them, and
//! offline renders do not contain them.
//!
//! # Threads and hand-overs
//!
//! | What | Direction | Mechanism |
//! |---|---|---|
//! | a voice to start (its head, the stream's reader) | control -> render | a `Box<Voice>` as an address on a single-producer ring (the producer side is serialized by the player's mutex) |
//! | stop (fade, or at once) | control -> render | an atomic request |
//! | a voice that ended, was faded out or was replaced | render -> control | its address on a second ring; [`PreviewPlayer::collect`] frees it |
//! | voices sounding | render -> anyone | an atomic |
//!
//! The render side ([`PreviewVoices::mix`]) reads PCM through `st-samples`' real-time readers and
//! never allocates, locks or frees: a `Voice` holds the last references that matter only until
//! the control side has taken it back, where the `Arc<PreviewPcm>`, the stream handle and the
//! reader are dropped. Should the return ring ever be full, the voice is leaked rather than
//! freed on the audio thread (and counted).

use st_samples::preview::{PreviewCache, PreviewPcm, PreviewStream, PreviewStreamReader, StreamAudio};
use st_samples::CancelToken;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use tracker_engine::engine::util::Ring;

/// Voices that can sound at once (one playing, the others fading out).
pub const VOICES: usize = 8;
const FADE_IN_SECONDS: f64 = 0.002;
const FADE_OUT_SECONDS: f64 = 0.012;
/// How far the tail decoder stays ahead of the playhead.
const STREAM_LEAD_SECONDS: f64 = 6.0;
/// Most files one prefetch request warms.
const MAXIMUM_PREFETCH: usize = 8;

const STOP_NONE: u8 = 0;
const STOP_FADE: u8 = 1;
const STOP_NOW: u8 = 2;

/// One preview voice. Built on a control thread, played by the render thread, freed on a
/// control thread.
struct Voice {
    pcm: Arc<PreviewPcm>,
    reader: Option<PreviewStreamReader>,
    /// Keeps the stream's memory alive until the control side drops the voice.
    stream: Option<PreviewStream>,
    path: String,
    serial: u64,
    volume: f32,
    position: usize,
    gain: f32,
    target: f32,
    delta: f32,
    /// Set by the render thread: the audio ran to its end (rather than being faded out).
    ended: bool,
}

/// What the two sides of one engine share.
struct Shared {
    starts: Ring<usize>,
    retired: Ring<usize>,
    stop: AtomicU8,
    sounding: AtomicU32,
    leaked: AtomicU64,
    underruns: AtomicU64,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Both sides are gone: whatever is still queued is freed here, wherever that is (the
        // render side is dropped on a control thread, see `crate::engine`).
        for ring in [&self.starts, &self.retired] {
            while let Some(address) = ring.pop() {
                // SAFETY: every address on either ring is the `Box::into_raw` of a `Voice`
                // pushed exactly once and owned by the ring until popped.
                drop(unsafe { Box::from_raw(address as *mut Voice) });
            }
        }
    }
}

/// The render side: the voices of one engine. Owned by the output stage.
pub struct PreviewVoices {
    shared: Arc<Shared>,
    slots: [*mut Voice; VOICES],
    fade_in: f32,
    fade_out: f32,
}

// SAFETY: the raw pointers are uniquely owned `Box<Voice>`es (handed over through the rings);
// a `Voice` is `Send` (its parts are), and `PreviewVoices` is used by one thread at a time.
unsafe impl Send for PreviewVoices {}

impl PreviewVoices {
    fn retire(&mut self, slot: usize) {
        let voice = std::mem::replace(&mut self.slots[slot], std::ptr::null_mut());
        if voice.is_null() {
            return;
        }
        // SAFETY: a non-null slot is a live `Box<Voice>` this side owns.
        if let Some(reader) = unsafe { &*voice }.reader.as_ref() {
            reader.cancel();
        }
        if !self.shared.retired.push(&(voice as usize)) {
            // Never freed here; see the module documentation.
            self.shared.leaked.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn fade_out_all(&mut self) {
        let frames = self.fade_out;
        for slot in self.slots {
            if slot.is_null() {
                continue;
            }
            // SAFETY: as in `retire`.
            let voice = unsafe { &mut *slot };
            if voice.target > 0.0 {
                voice.target = 0.0;
                voice.delta = -voice.gain.max(1e-4) / frames;
            }
        }
    }

    /// Adds the preview voices to `left`/`right`. Real-time safe.
    pub fn mix(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self.shared.stop.swap(STOP_NONE, Ordering::AcqRel) {
            STOP_FADE => self.fade_out_all(),
            STOP_NOW => (0..VOICES).for_each(|slot| self.retire(slot)),
            _ => {}
        }
        while let Some(address) = self.shared.starts.pop() {
            self.fade_out_all();
            let slot = match self.slots.iter().position(|slot| slot.is_null()) {
                Some(free) => free,
                None => {
                    // Every slot is still fading: the quietest one makes room.
                    // SAFETY: no slot is null here; each is a live `Box<Voice>`.
                    let quietest = (0..VOICES).min_by(|a, b| unsafe { (*self.slots[*a]).gain.total_cmp(&(*self.slots[*b]).gain) }).unwrap_or(0);
                    self.retire(quietest);
                    quietest
                }
            };
            let voice = address as *mut Voice;
            // SAFETY: the address is a `Box::into_raw` pushed by `PreviewPlayer::begin`; popping
            // it made this side its only owner.
            unsafe {
                (*voice).gain = 0.0;
                (*voice).target = 1.0;
                (*voice).delta = 1.0 / self.fade_in;
            }
            self.slots[slot] = voice;
        }
        let frames = left.len().min(right.len());
        let mut sounding = 0;
        for slot in 0..VOICES {
            if self.slots[slot].is_null() {
                continue;
            }
            // SAFETY: as in `retire`.
            let voice = unsafe { &mut *self.slots[slot] };
            let mut active = true;
            let mut produced = 0;
            while produced < frames && active {
                let head = voice.pcm.frames();
                let (source_left, source_right): (&[f32], &[f32]) = if voice.position < head {
                    (&voice.pcm.left()[voice.position..], &voice.pcm.right()[voice.position..])
                } else if let Some(reader) = voice.reader.as_ref() {
                    match reader.available() {
                        StreamAudio::Frames { left, right } => (left, right),
                        // Not decoded yet: silence for the rest of this block.
                        StreamAudio::Pending => {
                            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                        StreamAudio::Ended => {
                            voice.ended = voice.target > 0.0;
                            active = false;
                            continue;
                        }
                    }
                } else {
                    voice.ended = voice.target > 0.0;
                    active = false;
                    continue;
                };
                let count = source_left.len().min(source_right.len()).min(frames - produced);
                let (target, delta, volume) = (voice.target, voice.delta, voice.volume);
                let mut gain = voice.gain;
                let mut used = count;
                for index in 0..count {
                    if gain != target {
                        gain += delta;
                        if (delta > 0.0 && gain >= target) || (delta < 0.0 && gain <= target) {
                            gain = target;
                        }
                    }
                    let scale = gain * volume;
                    left[produced + index] += source_left[index] * scale;
                    right[produced + index] += source_right[index] * scale;
                    if gain <= 0.0 && target <= 0.0 {
                        active = false;
                        used = index + 1;
                        break;
                    }
                }
                voice.gain = gain;
                if voice.position >= head {
                    if let Some(reader) = voice.reader.as_mut() {
                        reader.advance(used);
                    }
                }
                voice.position += used;
                produced += used;
            }
            if active {
                sounding += 1;
            } else {
                self.retire(slot);
            }
        }
        self.shared.sounding.store(sounding, Ordering::Relaxed);
    }
}

impl Drop for PreviewVoices {
    fn drop(&mut self) {
        // The stage is dropped on a control thread (engine restart or shutdown).
        for slot in self.slots {
            if !slot.is_null() {
                // SAFETY: as in `retire`.
                drop(unsafe { Box::from_raw(slot) });
            }
        }
    }
}

/// What `sample.preview` answers.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewStarted {
    /// Length of the whole file in seconds.
    pub duration: f64,
    /// True when the head came from the cache without decoding.
    pub cached: bool,
    /// Milliseconds from the request to the voice being queued.
    pub start_ms: f64,
}

struct Link {
    shared: Arc<Shared>,
    sample_rate: u32,
    serial: u64,
    /// The newest voice: its serial and path, until it has been collected.
    newest: Option<(u64, String)>,
}

type EndedListener = Box<dyn Fn(&str) + Send + Sync>;

/// The control side of preview playback. Any control thread; `play` blocks while it decodes,
/// so call it from a worker.
pub struct PreviewPlayer {
    cache: Arc<PreviewCache>,
    link: Mutex<Link>,
    /// Bumped by every `play` and `stop`: a load that finishes under an older number is dropped.
    generation: AtomicU64,
    loading: Mutex<Option<CancelToken>>,
    prefetching: Mutex<Option<CancelToken>>,
    last_start_ms: Mutex<f64>,
    ended: Mutex<Option<EndedListener>>,
    /// False while something (an export) forbids previews.
    enabled: AtomicBool,
}

impl PreviewPlayer {
    pub fn new(cache: Arc<PreviewCache>) -> PreviewPlayer {
        PreviewPlayer {
            cache,
            link: Mutex::new(Link { shared: Self::shared(), sample_rate: 48_000, serial: 0, newest: None }),
            generation: AtomicU64::new(0),
            loading: Mutex::default(),
            prefetching: Mutex::default(),
            last_start_ms: Mutex::new(0.0),
            ended: Mutex::default(),
            enabled: AtomicBool::new(true),
        }
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared { starts: Ring::new(32), retired: Ring::new(128), stop: AtomicU8::new(STOP_NONE), sounding: AtomicU32::new(0), leaked: AtomicU64::new(0), underruns: AtomicU64::new(0) })
    }

    fn link(&self) -> MutexGuard<'_, Link> {
        self.link.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The render side for an engine at `sample_rate`. Voices of the previous engine are gone
    /// with it.
    pub fn voices(&self, sample_rate: f64) -> PreviewVoices {
        let shared = Self::shared();
        let mut link = self.link();
        link.shared = shared.clone();
        link.sample_rate = sample_rate.round().clamp(1_000.0, 768_000.0) as u32;
        link.newest = None;
        PreviewVoices { shared, slots: [std::ptr::null_mut(); VOICES], fade_in: (sample_rate * FADE_IN_SECONDS).max(1.0) as f32, fade_out: (sample_rate * FADE_OUT_SECONDS).max(1.0) as f32 }
    }

    /// Who hears that the newest preview played to its end (`sample.previewEnded`). Called from
    /// [`PreviewPlayer::collect`].
    pub fn on_ended(&self, listener: impl Fn(&str) + Send + Sync + 'static) {
        *self.ended.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(listener));
    }

    pub fn cache(&self) -> &Arc<PreviewCache> {
        &self.cache
    }

    /// The rate previews are decoded to: the engine's.
    pub fn sample_rate(&self) -> u32 {
        self.link().sample_rate
    }

    /// Allows or forbids previews (an export forbids them); forbidding also silences them.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
        if !enabled {
            self.stop(false);
        }
    }

    /// Frees the voices the render thread has finished with, and reports a preview that played
    /// to its end. Called on the engine's housekeeping tick and before every start.
    pub fn collect(&self) {
        let mut ended = None;
        {
            let mut link = self.link();
            while let Some(address) = link.shared.retired.pop() {
                // SAFETY: an address on the retired ring is a `Box::into_raw` the render side
                // pushed exactly once and no longer touches.
                let voice = unsafe { Box::from_raw(address as *mut Voice) };
                if link.newest.as_ref().is_some_and(|(serial, _)| *serial == voice.serial) {
                    link.newest = None;
                    if voice.ended {
                        ended = Some(voice.path.clone());
                    }
                }
                drop(voice);
            }
        }
        if let Some(path) = ended {
            if let Some(listener) = self.ended.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
                listener(&path);
            }
        }
    }

    /// Decodes (or takes from the cache) the head of `path` and starts it, fading out whatever
    /// plays. `Ok(None)` when a newer `play` or a `stop` arrived meanwhile.
    pub fn play(&self, path: &str, volume: f32) -> Result<Option<PreviewStarted>, String> {
        if path.is_empty() {
            return Err("Choose a sample to preview.".to_string());
        }
        if !self.enabled.load(Ordering::Acquire) {
            return Err("Wait for export to finish before previewing.".to_string());
        }
        let began = Instant::now();
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let cancel = CancelToken::new();
        if let Some(previous) = self.loading.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).replace(cancel.clone()) {
            previous.cancel();
        }
        let rate = self.sample_rate();
        let head = self.cache.head(path, rate, &cancel);
        if self.generation.load(Ordering::Acquire) != generation {
            return Ok(None);
        }
        let head = head.map_err(|error| error.to_string())?;
        if !self.enabled.load(Ordering::Acquire) {
            return Err("Wait for export to finish before previewing.".to_string());
        }
        self.begin(path, head.pcm.clone(), volume);
        let start_ms = began.elapsed().as_secs_f64() * 1000.0;
        *self.last_start_ms.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = start_ms;
        Ok(Some(PreviewStarted { duration: head.pcm.duration(), cached: head.cached, start_ms }))
    }

    /// Starts `pcm` now (and the decoder of its tail when it is only the head of its file).
    pub fn begin(&self, path: &str, pcm: Arc<PreviewPcm>, volume: f32) {
        self.collect();
        let (stream, reader) = if pcm.is_complete() {
            (None, None)
        } else {
            let stream = PreviewStream::for_tail_of(&pcm);
            let reader = stream.reader();
            if let Some(writer) = stream.writer() {
                let (file, head) = (path.to_string(), pcm.clone());
                let spawned = std::thread::Builder::new().name("preview-stream".to_string()).spawn(move || st_samples::preview::stream_tail(Path::new(&file), &head, writer, STREAM_LEAD_SECONDS));
                if spawned.is_err() {
                    stream.cancel();
                }
            }
            (Some(stream), reader)
        };
        let mut link = self.link();
        link.serial += 1;
        let serial = link.serial;
        let voice = Box::new(Voice { pcm, reader, stream, path: path.to_string(), serial, volume: if volume.is_finite() { volume.max(0.0) } else { 0.0 }, position: 0, gain: 0.0, target: 1.0, delta: 0.0, ended: false });
        let address = Box::into_raw(voice) as usize;
        if link.shared.starts.push(&address) {
            link.newest = Some((serial, path.to_string()));
        } else {
            // The render thread is not taking voices (no device is running): nothing plays.
            // SAFETY: the push failed, so the address is still only ours.
            let voice = unsafe { Box::from_raw(address as *mut Voice) };
            if let Some(stream) = &voice.stream {
                stream.cancel();
            }
        }
    }

    /// Fades every voice out (or silences them at once) and drops a load in flight.
    pub fn stop(&self, fade: bool) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(loading) = self.loading.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take() {
            loading.cancel();
        }
        let link = self.link();
        // A hard stop is never downgraded to a fade by a later request.
        if fade {
            let _ = link.shared.stop.compare_exchange(STOP_NONE, STOP_FADE, Ordering::AcqRel, Ordering::Acquire);
        } else {
            link.shared.stop.store(STOP_NOW, Ordering::Release);
        }
    }

    /// Voices the render thread mixed in its last block.
    pub fn sounding(&self) -> u32 {
        self.link().shared.sounding.load(Ordering::Relaxed)
    }

    /// Warms the cache for `paths` (at most eight) on a background thread; each call replaces
    /// the previous queue. Returns the number queued.
    pub fn prefetch(self: &Arc<Self>, paths: Vec<String>) -> usize {
        let cancel = CancelToken::new();
        if let Some(previous) = self.prefetching.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).replace(cancel.clone()) {
            previous.cancel();
        }
        let paths: Vec<String> = paths.into_iter().filter(|path| !path.is_empty()).take(MAXIMUM_PREFETCH).collect();
        if paths.is_empty() {
            return 0;
        }
        let queued = paths.len();
        let (cache, rate) = (self.cache.clone(), self.sample_rate());
        let _ = std::thread::Builder::new().name("preview-prefetch".to_string()).spawn(move || {
            for path in paths {
                if cancel.is_cancelled() {
                    break;
                }
                if st_samples::preview::volume_offline(&path) || cache.recently_failed(&path, PreviewCache::FAILURE_MEMORY) {
                    continue;
                }
                if cache.key(&path, rate).is_some_and(|key| cache.cached(&key).is_some()) {
                    continue;
                }
                let _ = cache.head(&path, rate, &cancel);
            }
        });
        queued
    }

    /// `sample.previewStatus`.
    pub fn status(&self) -> serde_json::Value {
        let stats = self.cache.stats();
        let (sounding, underruns, leaked) = {
            let link = self.link();
            (link.shared.sounding.load(Ordering::Relaxed), link.shared.underruns.load(Ordering::Relaxed), link.shared.leaked.load(Ordering::Relaxed))
        };
        serde_json::json!({
            "cachedPreviews": stats.cached_previews, "cacheBytes": stats.cache_bytes, "cacheHits": stats.cache_hits, "cacheMisses": stats.cache_misses,
            "voices": sounding, "lastStartMs": *self.last_start_ms.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            "underruns": underruns, "leakedVoices": leaked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    fn player() -> (Arc<PreviewPlayer>, PreviewVoices) {
        let player = Arc::new(PreviewPlayer::new(Arc::new(PreviewCache::default())));
        let voices = player.voices(RATE);
        (player, voices)
    }

    fn constant(value: f32, frames: usize) -> Arc<PreviewPcm> {
        Arc::new(PreviewPcm::new(vec![value; frames], vec![-value; frames], true, Some(frames), frames as f64 / RATE, RATE as u32))
    }

    fn mix(voices: &mut PreviewVoices, frames: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut left, mut right) = (vec![0.0; frames], vec![0.0; frames]);
        voices.mix(&mut left, &mut right);
        (left, right)
    }

    #[test]
    fn a_voice_fades_in_plays_at_its_volume_and_ends() {
        let (player, mut voices) = player();
        player.begin("/a.wav", constant(0.5, 1_000), 0.5);
        let (left, right) = mix(&mut voices, 512);
        // 2 ms at 48 kHz: 96 frames of linear fade-in, then gain 1 times the volume.
        assert!((left[0] - 0.5 * 0.5 / 96.0).abs() < 1e-6 && (left[47] - 0.25 * 48.0 / 96.0).abs() < 1e-5);
        assert!(left[96..].iter().all(|sample| (sample - 0.25).abs() < 1e-6), "full level after the fade-in");
        assert!(right.iter().zip(&left).all(|(r, l)| (r + l).abs() < 1e-7), "both channels are played");
        assert_eq!(player.sounding(), 1);
        let (left, _) = mix(&mut voices, 512);
        assert!(left[..488].iter().all(|sample| (sample - 0.25).abs() < 1e-6) && left[488..].iter().all(|sample| *sample == 0.0), "the voice ends with its audio");
        assert_eq!(player.sounding(), 0);
        let ended = Arc::new(Mutex::new(Vec::new()));
        let log = ended.clone();
        player.on_ended(move |path| log.lock().unwrap().push(path.to_string()));
        player.collect();
        assert_eq!(*ended.lock().unwrap(), ["/a.wav"]);
    }

    #[test]
    fn consecutive_previews_crossfade() {
        let (player, mut voices) = player();
        player.begin("/a.wav", constant(1.0, 48_000), 1.0);
        mix(&mut voices, 1_024);
        player.begin("/b.wav", constant(1.0, 48_000), 1.0);
        let (left, _) = mix(&mut voices, 1_024);
        // The old voice ramps 1 -> 0 over 12 ms (576 frames), the new one 0 -> 1 over 2 ms (96).
        for (index, sample) in left.iter().enumerate() {
            let out = (1.0 - (index + 1) as f32 / 576.0).max(0.0);
            let fade_in = ((index + 1) as f32 / 96.0).min(1.0);
            assert!((sample - (out + fade_in)).abs() < 1e-4, "frame {index}: {sample} vs {}", out + fade_in);
        }
        assert_eq!(player.sounding(), 1, "the faded voice is gone");
        // The replaced voice did not end by itself: no event for it.
        let ended = Arc::new(Mutex::new(Vec::new()));
        let log = ended.clone();
        player.on_ended(move |path| log.lock().unwrap().push(path.to_string()));
        player.collect();
        assert!(ended.lock().unwrap().is_empty());
        // Stop fades the playing voice out; a hard stop silences at once.
        player.stop(true);
        let (left, _) = mix(&mut voices, 1_024);
        assert!((left[0] - (1.0 - 1.0 / 576.0)).abs() < 1e-4 && left[575].abs() < 1e-4 && left[576..].iter().all(|sample| *sample == 0.0));
        player.begin("/c.wav", constant(1.0, 48_000), 1.0);
        mix(&mut voices, 256);
        player.stop(false);
        assert_eq!(mix(&mut voices, 256).0, vec![0.0; 256]);
        player.collect();
        assert!(ended.lock().unwrap().is_empty());
    }

    #[test]
    fn rapid_starts_never_exceed_the_voice_count() {
        let (player, mut voices) = player();
        for index in 0..20 {
            player.begin(&format!("/{index}.wav"), constant(0.1, 48_000), 1.0);
            let (left, _) = mix(&mut voices, 16);
            assert!(left.iter().all(|sample| sample.abs() <= 0.1 * VOICES as f32 + 1e-6));
            assert!(player.sounding() as usize <= VOICES);
        }
        mix(&mut voices, 2_048);
        assert_eq!(player.sounding(), 1);
        assert_eq!(player.status()["leakedVoices"], 0);
    }

    #[test]
    fn a_long_file_continues_from_its_head_into_the_stream() {
        let (player, mut voices) = player();
        // The head is frames 0..1000 of a ramp; the stream delivers the rest.
        let ramp = |from: usize, to: usize| -> Vec<f32> { (from..to).map(|frame| frame as f32 / 10_000.0).collect() };
        let head = Arc::new(PreviewPcm::new(ramp(0, 1_000), ramp(0, 1_000), false, Some(3_000), 3_000.0 / RATE, RATE as u32));
        let stream = PreviewStream::new(2_000, 512);
        let mut writer = stream.writer().unwrap();
        // Built by hand (begin would start a file decoder).
        let voice = Box::new(Voice { pcm: head, reader: stream.reader(), stream: Some(stream.clone()), path: "/long.wav".to_string(), serial: 1, volume: 1.0, position: 0, gain: 0.0, target: 1.0, delta: 0.0, ended: false });
        {
            let mut link = player.link();
            link.serial = 1;
            link.newest = Some((1, "/long.wav".to_string()));
            assert!(link.shared.starts.push(&(Box::into_raw(voice) as usize)));
        }
        writer.append(&ramp(1_000, 1_800), &ramp(1_000, 1_800));
        let (left, _) = mix(&mut voices, 1_500);
        for (frame, sample) in left.iter().enumerate().skip(96) {
            assert!((sample - frame as f32 / 10_000.0).abs() < 1e-6, "frame {frame} continues sample-exactly");
        }
        // The decoder is behind: silence, not an end.
        let (left, _) = mix(&mut voices, 600);
        assert!(left[..300].iter().enumerate().all(|(index, sample)| (sample - (1_500 + index) as f32 / 10_000.0).abs() < 1e-6));
        assert!(left[300..].iter().all(|sample| *sample == 0.0));
        assert_eq!(player.sounding(), 1);
        assert!(player.status()["underruns"].as_u64().unwrap() >= 1);
        writer.append(&ramp(1_800, 3_000), &ramp(1_800, 3_000));
        writer.finish();
        let (left, _) = mix(&mut voices, 2_000);
        assert!(left[..1_200].iter().enumerate().all(|(index, sample)| (sample - (1_800 + index) as f32 / 10_000.0).abs() < 1e-6));
        assert!(left[1_200..].iter().all(|sample| *sample == 0.0));
        assert_eq!(player.sounding(), 0);
        let ended = Arc::new(Mutex::new(Vec::new()));
        let log = ended.clone();
        player.on_ended(move |path| log.lock().unwrap().push(path.to_string()));
        player.collect();
        assert_eq!(*ended.lock().unwrap(), ["/long.wav"]);
    }
}
