//! Preview decoding and caching: head plus streamed tail equals one full decode, the cache's
//! memory bound and eviction order, sharing of concurrent decodes, cancellation of superseded
//! ones, and fast failure for missing files and offline drives.

use std::f64::consts::PI;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use st_samples::preview::{decode_head, decode_preview, stream_tail, PreviewCache, PreviewStream, StreamAudio};
use st_samples::wav::{write_wav, Dither, WavEncoding, WavSpec};
use st_samples::{CancelToken, SampleError};

const RATE: u32 = 48_000;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("st-samples-preview-{}-{name}-{}", std::process::id(), st_samples::paths::unique_suffix()));
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn write_sine(path: &Path, file_rate: u32, seconds: f64, frequency: f64, channels: u16) {
    let frames = (file_rate as f64 * seconds) as usize;
    let tone: Vec<f32> = (0..frames).map(|i| (0.8 * (2.0 * PI * frequency * i as f64 / file_rate as f64).sin()) as f32).collect();
    let spec = WavSpec { sample_rate: file_rate, channels, encoding: WavEncoding::Float32, dither: Dither::None };
    write_wav(path, spec, &vec![tone; channels as usize], &[]).unwrap();
}

fn max_jump(samples: &[f32]) -> f32 {
    samples.windows(2).fold(0f32, |worst, pair| worst.max((pair[1] - pair[0]).abs()))
}

#[test]
fn head_plus_streamed_tail_equals_one_full_decode_across_a_rate_conversion() {
    let scratch = Scratch::new("tail");
    let path = scratch.0.join("long.wav");
    write_sine(&path, 44_100, 10.0, 523.0, 2);
    let never = CancelToken::new();
    let began = Instant::now();
    let head = decode_head(&path, RATE, 4.0, 8.0, &never).unwrap();
    let head_ms = began.elapsed().as_secs_f64() * 1000.0;
    assert!(!head.is_complete() && head.frames() == 4 * 48_000, "a 10 s file caches a 4 s head ({})", head.frames());
    assert_eq!(head.estimated_total_frames(), Some(480_000));
    assert!((head.duration() - 10.0).abs() < 1e-9);
    assert_eq!(head.bytes(), 4 * 48_000 * 8);

    let stream = PreviewStream::for_tail_of(&head);
    let mut reader = stream.reader().unwrap();
    stream_tail(&path, &head, stream.writer().unwrap(), 60.0);
    assert!(stream.is_finished(), "the tail decode finishes");

    let mut full = Vec::new();
    let ended = decode_preview(&path, RATE, 0, usize::MAX, &never, &mut |left, _| {
        full.extend_from_slice(left);
        true
    })
    .unwrap();
    assert!(ended && full.len() == 480_000);

    // Play the head, then the stream, the way a voice does.
    let mut played = head.left().to_vec();
    loop {
        let taken = match reader.available() {
            StreamAudio::Frames { left, right } => {
                assert_eq!(left, right);
                played.extend_from_slice(left);
                left.len()
            }
            StreamAudio::Pending => panic!("a finished stream is never pending"),
            StreamAudio::Ended => break,
        };
        reader.advance(taken);
    }
    assert_eq!(played.len(), full.len());
    assert!(played == full, "the streamed tail continues the converted head sample-exactly");
    let seam = &played[head.frames() - 32..head.frames() + 32];
    assert!(max_jump(seam) < 0.8 * 2.0 * std::f32::consts::PI * 523.0 / 48_000.0 + 0.01, "no discontinuity at the head/tail seam");
    assert_eq!(stream.read_position(), 480_000 - head.frames(), "the reader position is published");
    println!("MEASURE preview head: 4 s of a 10 s 44.1 kHz file to 48 kHz in {head_ms:.1} ms (this build profile)");

    // A short file is cached whole, and a mono file fills both channels.
    let short = scratch.0.join("short.wav");
    write_sine(&short, 44_100, 1.0, 300.0, 1);
    let whole = decode_head(&short, RATE, 4.0, 8.0, &never).unwrap();
    assert!(whole.is_complete() && whole.frames() == 48_000);
    assert_eq!(whole.left(), whole.right());
    // Without conversion the head is the file's own samples.
    let native = decode_head(&short, 44_100, 4.0, 8.0, &never).unwrap();
    assert!(native.is_complete() && native.frames() == 44_100);
    assert!((native.left()[100] - (0.8 * (2.0 * PI * 300.0 * 100.0 / 44_100.0).sin()) as f32).abs() < 1e-7);
}

#[test]
fn tail_streaming_waits_for_the_reader_and_stops_when_cancelled() {
    let scratch = Scratch::new("lead");
    let path = scratch.0.join("long.wav");
    write_sine(&path, 48_000, 20.0, 440.0, 2);
    let head = decode_head(&path, RATE, 1.0, 2.0, &CancelToken::new()).unwrap();
    assert_eq!(head.frames(), 48_000);
    let stream = PreviewStream::for_tail_of(&head);
    let writer = stream.writer().unwrap();
    let mut reader = stream.reader().unwrap();
    let (tail_path, tail_head) = (path.clone(), Arc::new(head));
    let worker = {
        let head = tail_head.clone();
        std::thread::spawn(move || stream_tail(&tail_path, &head, writer, 2.0))
    };
    // The writer stops about two seconds ahead of a reader that does not move.
    let deadline = Instant::now() + Duration::from_secs(20);
    while stream.published() < 96_000 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(60));
    let ahead = stream.published();
    assert!((96_000..=96_000 + 3 * 16_384).contains(&ahead), "the writer ran {ahead} frames ahead of a stalled reader");
    assert!(!stream.is_finished());
    // Reading lets it continue.
    let (mut l, mut r) = (vec![0f32; 48_000], vec![0f32; 48_000]);
    assert_eq!(reader.read(&mut l, &mut r), (48_000, false));
    let deadline = Instant::now() + Duration::from_secs(20);
    while stream.published() < ahead + 40_000 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(stream.published() >= ahead + 40_000, "the writer did not follow the reader");
    // The voice ends: dropping the reader cancels the decode promptly.
    drop(reader);
    worker.join().unwrap();
    assert!(stream.is_cancelled() && stream.is_finished());
    assert!(stream.published() < 19 * 48_000, "the tail kept decoding after cancellation");
}

#[test]
fn cache_hits_lru_memory_cap_offline_volumes_and_missing_files() {
    let scratch = Scratch::new("cache");
    let mut paths = Vec::new();
    for index in 0..5 {
        let path = scratch.0.join(format!("one-shot-{index}.wav"));
        write_sine(&path, 48_000, 1.0, 200.0 + index as f64 * 50.0, if index == 0 { 1 } else { 2 });
        paths.push(path.to_string_lossy().into_owned());
    }
    let never = CancelToken::new();
    // One second of stereo float at 48 kHz is 384 000 bytes: the cap holds two.
    let cache = PreviewCache::new(800_000, 4.0, 8.0);
    let began = Instant::now();
    let first = cache.head(&paths[0], RATE, &never).unwrap();
    let miss_ms = began.elapsed().as_secs_f64() * 1000.0;
    assert!(!first.cached && first.pcm.is_complete(), "the first request decodes");
    assert_eq!(first.pcm.left()[100], first.pcm.right()[100], "mono is duplicated to both channels");
    let mut hit_times = Vec::new();
    for _ in 0..20 {
        let began = Instant::now();
        let again = cache.head(&paths[0], RATE, &never).unwrap();
        hit_times.push(began.elapsed().as_secs_f64() * 1000.0);
        assert!(again.cached && Arc::ptr_eq(&again.pcm, &first.pcm), "a second request hits");
    }
    hit_times.sort_by(f64::total_cmp);
    for path in &paths[1..] {
        cache.head(path, RATE, &never).unwrap();
    }
    assert!(cache.bytes() <= 800_000 && cache.count() == 2, "the memory cap evicts the least recently used ({} bytes, {} entries)", cache.bytes(), cache.count());
    assert!(cache.head(&paths[4], RATE, &never).unwrap().cached && cache.head(&paths[3], RATE, &never).unwrap().cached, "the two newest stay");
    // The evicted head is still valid for whoever holds it (a playing voice).
    assert_eq!(first.pcm.frames(), 48_000);
    let evicted = cache.head(&paths[0], RATE, &never).unwrap();
    assert!(!evicted.cached, "an evicted entry decodes again");
    // Using an entry protects it: re-adding 0 pushed out 4 (3 was used after it), and with 3
    // touched again, adding 1 pushes out 0.
    assert!(cache.head(&paths[3], RATE, &never).unwrap().cached);
    cache.head(&paths[1], RATE, &never).unwrap();
    let key = |index: usize| cache.key(&paths[index], RATE).unwrap();
    assert!(cache.cached(&key(1)).is_some() && cache.cached(&key(3)).is_some() && cache.cached(&key(0)).is_none() && cache.cached(&key(4)).is_none());
    let stats = cache.stats();
    assert_eq!((stats.cached_previews, stats.cache_bytes), (2, 768_000));
    assert_eq!(stats.cache_misses, 7);
    assert!(serde_json::to_value(stats).unwrap().get("cacheHits").is_some());

    // A different device rate is a different entry.
    let other_rate = cache.head(&paths[1], 44_100, &never).unwrap();
    assert!(!other_rate.cached && other_rate.pcm.frames() == 44_100);

    // An edited file is decoded again.
    std::thread::sleep(Duration::from_millis(20));
    write_sine(Path::new(&paths[3]), 48_000, 0.5, 999.0, 2);
    let edited = cache.head(&paths[3], RATE, &never).unwrap();
    assert!(!edited.cached && edited.pcm.frames() == 24_000);

    let began = Instant::now();
    let offline = cache.head(&format!("/Volumes/SuperTrackerMissingDrive-{}/a.wav", std::process::id()), RATE, &never).unwrap_err();
    assert!(matches!(offline, SampleError::Offline(_)) && offline.to_string().contains("a.wav"), "{offline}");
    assert!(began.elapsed() < Duration::from_millis(500), "offline volumes fail fast");
    let missing = scratch.0.join("missing.wav").to_string_lossy().into_owned();
    assert!(matches!(cache.head(&missing, RATE, &never).unwrap_err(), SampleError::NotFound(_)));
    assert!(cache.recently_failed(&missing, PreviewCache::FAILURE_MEMORY), "missing files are not retried in a loop");
    assert!(!cache.recently_failed(&missing, Duration::ZERO), "old failures are forgotten");
    let broken = scratch.0.join("broken.wav");
    std::fs::write(&broken, "not audio ".repeat(100)).unwrap();
    let broken = broken.to_string_lossy().into_owned();
    assert!(cache.head(&broken, RATE, &never).is_err());
    assert!(cache.recently_failed(&broken, PreviewCache::FAILURE_MEMORY));
    assert!(!cache.recently_failed(&paths[0], PreviewCache::FAILURE_MEMORY));
    println!("MEASURE preview cache: miss {miss_ms:.2} ms, hit median {:.4} ms (this build profile)", hit_times[hit_times.len() / 2]);
}

#[test]
fn concurrent_requests_for_one_file_share_a_decode() {
    let scratch = Scratch::new("shared");
    let path = scratch.0.join("shared.wav");
    write_sine(&path, 44_100, 6.0, 330.0, 2);
    let path = path.to_string_lossy().into_owned();
    let cache = Arc::new(PreviewCache::default());
    let threads = 6;
    let barrier = Arc::new(Barrier::new(threads));
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            let (cache, barrier, path) = (cache.clone(), barrier.clone(), path.clone());
            std::thread::spawn(move || {
                barrier.wait();
                cache.head(&path, RATE, &CancelToken::new()).unwrap()
            })
        })
        .collect();
    let heads: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert!(heads.iter().all(|head| Arc::ptr_eq(&head.pcm, &heads[0].pcm)), "every request got the same decoded audio");
    assert_eq!(cache.stats().cache_misses, 1, "in-flight decodes are shared");
    assert_eq!(cache.count(), 1);
    assert!(heads[0].pcm.is_complete() && heads[0].pcm.frames() == 6 * 48_000);
}

#[test]
fn superseded_decodes_are_cancelled_without_poisoning_the_cache() {
    let scratch = Scratch::new("cancel");
    let path = scratch.0.join("long.wav");
    write_sine(&path, 44_100, 12.0, 220.0, 2);
    let path = path.to_string_lossy().into_owned();
    // The whole file counts as a head here, so the decode takes a while.
    let cache = Arc::new(PreviewCache::new(64 << 20, 30.0, 30.0));

    let stale = CancelToken::new();
    stale.cancel();
    assert_eq!(cache.head(&path, RATE, &stale).unwrap_err(), SampleError::Cancelled);
    assert_eq!(cache.count(), 0);
    assert!(!cache.recently_failed(&path, PreviewCache::FAILURE_MEMORY), "a cancellation is not a failure");

    // A prefetch starts decoding; a listener joins it; the prefetch is superseded. The listener
    // must still get its audio.
    let prefetch = CancelToken::new();
    let owner = {
        let (cache, path, token) = (cache.clone(), path.clone(), prefetch.clone());
        std::thread::spawn(move || cache.head(&path, RATE, &token))
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.stats().cache_misses < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let listener = {
        let (cache, path) = (cache.clone(), path.clone());
        std::thread::spawn(move || cache.head(&path, RATE, &CancelToken::new()))
    };
    std::thread::sleep(Duration::from_millis(30));
    let cancelled_at = Instant::now();
    prefetch.cancel();
    let owner_result = owner.join().unwrap();
    let stopped_ms = cancelled_at.elapsed().as_secs_f64() * 1000.0;
    let heard = listener.join().unwrap().expect("the listener decodes for itself");
    assert!(heard.pcm.is_complete() && heard.pcm.frames() == 12 * 48_000);
    match owner_result {
        // Usual: the prefetch stopped between two decode blocks.
        Err(error) => {
            assert_eq!(error, SampleError::Cancelled);
            assert!(!heard.cached);
            assert_eq!(cache.stats().cache_misses, 3, "the listener started its own decode");
            println!("MEASURE preview cancel: a superseded decode stopped {stopped_ms:.1} ms after cancellation");
        }
        // On a very fast machine the prefetch may finish before it is cancelled.
        Ok(head) => assert!(Arc::ptr_eq(&head.pcm, &heard.pcm)),
    }
    assert_eq!(cache.count(), 1);
    assert!(cache.head(&path, RATE, &CancelToken::new()).unwrap().cached);

    // A joiner whose own request is superseded stops waiting.
    let other = scratch.0.join("other.wav");
    write_sine(&other, 44_100, 12.0, 220.0, 2);
    let other = other.to_string_lossy().into_owned();
    let owner = {
        let (cache, other) = (cache.clone(), other.clone());
        std::thread::spawn(move || cache.head(&other, RATE, &CancelToken::new()))
    };
    let before = cache.stats().cache_misses;
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.stats().cache_misses == before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let impatient = CancelToken::new();
    let joiner = {
        let (cache, other, token) = (cache.clone(), other.clone(), impatient.clone());
        std::thread::spawn(move || cache.head(&other, RATE, &token))
    };
    std::thread::sleep(Duration::from_millis(30));
    impatient.cancel();
    match joiner.join().unwrap() {
        Err(error) => assert_eq!(error, SampleError::Cancelled),
        Ok(head) => assert!(head.pcm.is_complete()),
    }
    assert!(owner.join().unwrap().unwrap().pcm.is_complete(), "the owner is unaffected by a joiner leaving");
}
