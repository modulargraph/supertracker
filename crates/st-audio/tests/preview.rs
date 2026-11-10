//! Browser previews through the engine: mixed into the output after the master fader, from
//! files, with a streamed tail, and without allocating or freeing on the render thread.
//!
//! Like `realtime.rs`, this binary counts allocations and frees made while a thread-local switch
//! is on, which it is only around `Renderer::render`.

mod common;

use common::*;
use serde_json::json;
use st_audio::samples::wav_bytes;
use st_audio::PlayOptions;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Counting;

static COUNTED: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

fn count() {
    if ARMED.try_with(|armed| armed.get()).unwrap_or(false) {
        COUNTED.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: defers to the system allocator; only counts on the way.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Renders `frames` frames in blocks of `block` with the counter armed.
fn render_counted(engine: &st_audio::AudioEngine, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let renderer = engine.renderer();
    let mut renderer = renderer.lock().unwrap();
    let mut at = 0;
    while at < frames {
        let end = (at + block).min(frames);
        ARMED.with(|armed| armed.set(true));
        renderer.render(&mut left[at..end], &mut right[at..end]);
        ARMED.with(|armed| armed.set(false));
        at = end;
    }
    (left, right)
}

fn folder(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("st-audio-preview-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// A file whose sample `n` is `level` on the left and `-level` on the right.
fn constant_file(path: &std::path::Path, level: f32, frames: usize) {
    std::fs::write(path, wav_bytes(&vec![level; frames], Some(&vec![-level; frames]), 48_000)).unwrap();
}

#[test]
fn previews_are_mixed_after_the_master_fader_without_allocating_on_the_render_thread() {
    ARMED.with(|armed| armed.set(true));
    drop(std::hint::black_box(vec![0u8; 64]));
    ARMED.with(|armed| armed.set(false));
    assert!(COUNTED.swap(0, Ordering::Relaxed) >= 2, "the counting allocator counts");

    let dir = folder("mix");
    let (short, other, long) = (dir.join("short.wav"), dir.join("other.wav"), dir.join("long.wav"));
    constant_file(&short, 0.5, 24_000);
    constant_file(&other, 0.25, 24_000);
    // Twelve seconds: longer than the cache keeps whole, so its tail is streamed.
    let ramp: Vec<f32> = (0..12 * 48_000).map(|frame| 0.1 + (frame % 48_000) as f32 / 96_000.0).collect();
    std::fs::write(&long, wav_bytes(&ramp, None, 48_000)).unwrap();

    let engine = engine();
    let preview = engine.live().preview().clone();
    let ended = Arc::new(Mutex::new(Vec::new()));
    let log = ended.clone();
    preview.on_ended(move |path| log.lock().unwrap().push(path.to_string()));

    // The project's master is silent (master volume 0): what is heard is the preview alone,
    // at its own level, so it is mixed after the master fader.
    let mut project = project(1);
    project["masterVolume"] = json!(0.0);
    engine.publish(&parse(&project)).unwrap();
    engine.play(&PlayOptions { song: false, from_start: true, ..Default::default() });
    render_counted(&engine, 2_048, 512);

    let started = preview.play(&short.to_string_lossy(), 0.5).unwrap().expect("not superseded");
    assert!((started.duration - 0.5).abs() < 1e-6 && !started.cached);
    let (left, right) = render_counted(&engine, 4_800, 512);
    assert!(left[96..].iter().all(|sample| (sample - 0.25).abs() < 1e-4), "level 0.5 at volume 0.5, untouched by the master fader");
    assert!(right[96..].iter().all(|sample| (sample + 0.25).abs() < 1e-4));
    assert!(left[0] < 0.01 && left[48] > 0.1 && left[48] < 0.15, "a 2 ms fade-in");

    // The next preview fades in while the first fades out over 12 ms.
    let second = preview.play(&other.to_string_lossy(), 1.0).unwrap().unwrap();
    assert!(!second.cached);
    let (left, _) = render_counted(&engine, 4_800, 100);
    for (index, sample) in left.iter().enumerate() {
        let expected = 0.25 * (1.0 - (index + 1) as f32 / 576.0).max(0.0) + 0.25 * ((index + 1) as f32 / 96.0).min(1.0);
        assert!((sample - expected).abs() < 2e-4, "crossfade frame {index}: {sample} vs {expected}");
    }
    // Played again, the first file comes from the cache.
    assert!(preview.play(&short.to_string_lossy(), 1.0).unwrap().unwrap().cached);
    render_counted(&engine, 48_000, 512);
    engine.tick();
    assert_eq!(*ended.lock().unwrap(), [short.to_string_lossy()], "only the preview that played to its end is announced");

    // A long file: the cached head (4 s), then the stream, continuous across the seam.
    let started = preview.play(&long.to_string_lossy(), 1.0).unwrap().unwrap();
    assert!((started.duration - 12.0).abs() < 1e-6);
    let mut heard = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while heard.len() < 6 * 48_000 && Instant::now() < deadline {
        // Offline rendering outruns the tail decoder; give it time as a device would.
        heard.extend(render_counted(&engine, 4_800, 480).0);
        std::thread::sleep(Duration::from_millis(2));
    }
    let seam = 4 * 48_000;
    let mut position = 0usize;
    let mut silent = 0usize;
    for (index, sample) in heard.iter().enumerate().skip(96) {
        // Silence where the decoder was behind is allowed after the head; the ramp never jumps.
        if index >= seam && *sample == 0.0 {
            silent += 1;
            continue;
        }
        if index < seam {
            position = index;
        }
        assert!((sample - (0.1 + (position % 48_000) as f32 / 96_000.0)).abs() < 2e-4, "frame {index} (file frame {position}) is {sample}");
        position += 1;
    }
    assert!(position > seam + 48_000, "the stream was played past the head ({position} file frames, {silent} frames of waiting)");
    preview.stop(true);
    let (left, _) = render_counted(&engine, 2_048, 512);
    assert!(left[1_000..].iter().all(|sample| *sample == 0.0), "stop fades the voice out");
    assert_eq!(preview.sounding(), 0);

    assert_eq!(COUNTED.load(Ordering::Relaxed), 0, "allocations or frees on the render path");
    engine.tick();
    assert_eq!(preview.status()["leakedVoices"], 0);
    // A newer request supersedes one that is still decoding; a missing file is an error.
    assert!(preview.play("/no/such/preview.wav", 1.0).is_err());
    preview.set_enabled(false);
    assert!(preview.play(&short.to_string_lossy(), 1.0).unwrap_err().contains("export"));
    std::fs::remove_dir_all(dir).ok();
}
