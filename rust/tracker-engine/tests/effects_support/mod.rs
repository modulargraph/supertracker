//! Shared helpers for the effects integration tests (`tests/effects_*.rs`), which are ports
//! of the C++ test programs that include the engine's internal headers.
#![allow(dead_code)]

use std::cell::Cell;
use tracker_engine::effects::common::{Descriptor, Processor};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Counts heap allocations made on the current thread while counting is switched on: the
/// equivalent of the C++ tests' replaced `operator new`.
#[cfg(not(feature = "cxx-alloc"))]
mod allocator {
    use super::{ALLOCATIONS, COUNTING};
    use std::alloc::{GlobalAlloc, Layout, System};

    pub struct CountingAllocator;

    fn note() {
        let _ = COUNTING.try_with(|counting| {
            if counting.get() {
                let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
    }

    // SAFETY: defers to the system allocator; the bookkeeping never allocates.
    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            note();
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            note();
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            note();
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;
}

/// Runs `f` with allocation counting on and returns its result.
pub fn counted<T>(f: impl FnOnce() -> T) -> T {
    COUNTING.with(|counting| counting.set(true));
    let result = f();
    COUNTING.with(|counting| counting.set(false));
    result
}

/// Allocations seen so far inside `counted` sections on this thread.
pub fn allocations() -> usize {
    ALLOCATIONS.with(|count| count.get())
}

pub fn reset_allocations() {
    ALLOCATIONS.with(|count| count.set(0));
}

/// Check counter in the style of the C++ tests' `require`: counts, and fails on the spot.
#[derive(Default)]
pub struct Checks {
    pub count: usize,
}

impl Checks {
    #[track_caller]
    pub fn require(&mut self, ok: bool, why: &str) {
        self.count += 1;
        assert!(ok, "FAIL: {why}");
    }
}

/// A processor created from its descriptor, with the catalogue defaults applied.
pub struct Effect {
    pub info: &'static Descriptor,
    pub dsp: Box<dyn Processor>,
    pub sr: f64,
}

impl Effect {
    /// `descriptor.create(rate, 256)` without touching the parameters.
    pub fn raw(info: &'static Descriptor, rate: f64) -> Self {
        Effect {
            info,
            dsp: (info.create)(rate, 256),
            sr: rate,
        }
    }

    /// `descriptor.create(rate, 256)` followed by the catalogue defaults.
    pub fn with_defaults(info: &'static Descriptor, rate: f64) -> Self {
        let mut effect = Self::raw(info, rate);
        for (slot, parameter) in effect.dsp.core_mut().p.iter_mut().zip(info.params.iter()) {
            *slot = parameter.default_value;
        }
        effect
    }

    pub fn parameter_index(&self, id: &str) -> Option<usize> {
        self.info
            .params
            .iter()
            .position(|parameter| parameter.id().to_bytes() == id.as_bytes())
    }

    /// Sets the physical value of the parameter with the given id.
    #[track_caller]
    pub fn set(&mut self, id: &str, value: f64) {
        let index = self
            .parameter_index(id)
            .unwrap_or_else(|| panic!("missing parameter {id}"));
        self.dsp.core_mut().p[index] = value;
    }

    pub fn set_tempo(&mut self, tempo: f64) {
        self.dsp.core_mut().tempo = tempo;
    }

    /// The control-side tail estimate for the processor's current parameters.
    pub fn tail_now(&self, tempo: f64) -> f64 {
        self.dsp.tail_for_parameters(&self.dsp.core().p, tempo)
    }
}

/// Finds a descriptor by id in a family.
#[track_caller]
pub fn descriptor(family: &'static [Descriptor], id: &str) -> &'static Descriptor {
    family
        .iter()
        .find(|d| d.info.id().to_bytes() == id.as_bytes())
        .unwrap_or_else(|| panic!("unknown effect {id}"))
}

/// A stereo test signal.
#[derive(Clone, PartialEq)]
pub struct Audio {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Audio {
    pub fn new(frames: usize) -> Self {
        Audio {
            l: vec![0.0; frames],
            r: vec![0.0; frames],
        }
    }

    pub fn len(&self) -> usize {
        self.l.len()
    }

    pub fn resize(&mut self, frames: usize) {
        self.l.resize(frames, 0.0);
        self.r.resize(frames, 0.0);
    }
}

/// Renders `audio` in place in blocks of `block` frames (0 = one block), counting allocations
/// made by `process`.
pub fn render(dsp: &mut dyn Processor, audio: &mut Audio, block: usize) {
    let block = if block == 0 {
        audio.len().max(1)
    } else {
        block
    };
    for (l, r) in audio.l.chunks_mut(block).zip(audio.r.chunks_mut(block)) {
        counted(|| dsp.process(l, r, None, None));
    }
}

/// Sum of squares of `a[begin..end]` (`end` 0 or past the end = to the end).
pub fn energy(a: &[f32], begin: usize, end: usize) -> f64 {
    let end = if end == 0 { a.len() } else { end.min(a.len()) };
    a[begin.min(end)..end]
        .iter()
        .map(|&v| v as f64 * v as f64)
        .sum()
}

/// Sum of squared differences from `begin`.
pub fn squared_difference(a: &[f32], b: &[f32], begin: usize) -> f64 {
    a[begin..]
        .iter()
        .zip(b[begin..].iter())
        .map(|(&x, &y)| {
            let d = (x - y) as f64;
            d * d
        })
        .sum()
}
