//! A wrapper around the unit range cache for `tests/unit_range.rs`.
//!
//! TODO(api): `tracker_engine::unit_range` has no safe Rust API: `STUnitRangeCache` can only be
//! created, rendered, reset, inspected and destroyed through the `st_unit_range_*` C functions
//! with C callbacks. Every `unsafe` block in this file is one of those calls; when the cache gets
//! Rust methods (a constructor returning `Result`, `reset`, `stats`, and a `render` that takes
//! the output channels as slices and the prepare/span/process callbacks as a trait object or
//! closures), only this file changes.

use std::any::Any;
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use tracker_engine::clock::{STClockContext, STClockSpan};
use tracker_engine::unit_range::*;

/// The three callbacks of `st_unit_range_render`, as one object.
pub trait Source {
    /// `STUnitRangePrepareFn`: extend the upstream caches for the uncached suffix.
    fn prepare(&mut self, graph_frame: i64, frames: u32) -> i32;
    /// `STUnitRangeSpanFn`: the clock span at `graph_frame` (at most `frames` long).
    fn span(&mut self, graph_frame: i64, frames: u32, span: &mut STClockSpan) -> i32;
    /// `STUnitRangeProcessFn`: render `frames` frames of every channel.
    fn process(&mut self, context: &STClockContext, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> i32;
}

/// Which callbacks a render call passes (the C API takes each as an optional function pointer).
#[derive(Clone, Copy)]
pub struct Callbacks {
    pub prepare: bool,
    pub span: bool,
    pub process: bool,
}

impl Callbacks {
    pub const ALL: Callbacks = Callbacks { prepare: true, span: true, process: true };
    pub const WITHOUT_PREPARE: Callbacks = Callbacks { prepare: false, span: true, process: true };
    pub const PROCESS_ONLY: Callbacks = Callbacks { prepare: false, span: false, process: true };
    pub const SPAN_ONLY: Callbacks = Callbacks { prepare: false, span: true, process: false };
}

/// An owned cache (`st_unit_range_create` / `st_unit_range_destroy`).
pub struct Cache {
    raw: *mut STUnitRangeCache,
    channels: u32,
    maximum_request: u32,
}

// SAFETY: the C API's contract, which the crate states for `STUnitRangeCache` itself (`unsafe
// impl Sync`): any thread may call render/reset/get_stats; one atomic flag admits a single caller
// and everyone else gets `ST_UNIT_RANGE_BUSY`. The wrapper only adds immutable configuration.
unsafe impl Send for Cache {}
unsafe impl Sync for Cache {}

/// What the trampolines get as `user`.
struct Call<'a> {
    source: &'a mut dyn Source,
    channels: usize,
    /// A failed assertion inside a callback: carried out of the C call and resumed afterwards
    /// (callbacks must not unwind into the cache).
    panic: Option<Box<dyn Any + Send>>,
}

/// The callback result that reports a caught panic to the cache (any nonzero prepare/process
/// result and any non-OK span result abort the request).
const PANICKED: i32 = -987654;

unsafe extern "C" fn prepare_trampoline(user: *mut c_void, graph_frame: i64, frames: u32) -> i32 {
    // SAFETY: `user` is the `Call` that `Cache::render` passed for the duration of this call.
    let call = unsafe { &mut *(user as *mut Call) };
    match catch_unwind(AssertUnwindSafe(|| call.source.prepare(graph_frame, frames))) {
        Ok(status) => status,
        Err(payload) => {
            call.panic = Some(payload);
            PANICKED
        }
    }
}

unsafe extern "C" fn span_trampoline(user: *mut c_void, graph_frame: i64, frames: u32, span: *mut STClockSpan) -> i32 {
    // SAFETY: as in `prepare_trampoline`; `span` is the cache's plan entry, valid for this call.
    let (call, span) = unsafe { (&mut *(user as *mut Call), &mut *span) };
    match catch_unwind(AssertUnwindSafe(|| call.source.span(graph_frame, frames, span))) {
        Ok(status) => status,
        Err(payload) => {
            call.panic = Some(payload);
            PANICKED
        }
    }
}

unsafe extern "C" fn process_trampoline(
    user: *mut c_void,
    context: *const STClockContext,
    graph_frame: i64,
    frames: u32,
    output: *const *mut f32,
) -> i32 {
    // SAFETY: as in `prepare_trampoline`; `context` is valid for this call, and `output` holds
    // one pointer per channel, each writable for `frames` samples and disjoint from the others.
    let (call, context) = unsafe { (&mut *(user as *mut Call), &*context) };
    let mut empty: [&mut [f32]; 2] = [&mut [], &mut []];
    for (channel, slot) in empty.iter_mut().enumerate().take(call.channels) {
        // SAFETY: see above.
        *slot = unsafe { std::slice::from_raw_parts_mut(*output.add(channel), frames as usize) };
    }
    let channels = call.channels;
    match catch_unwind(AssertUnwindSafe(|| call.source.process(context, graph_frame, frames, &mut empty[..channels]))) {
        Ok(status) => status,
        Err(payload) => {
            call.panic = Some(payload);
            PANICKED
        }
    }
}

impl Cache {
    /// `st_unit_range_create`: the cache, or the error text.
    pub fn create(
        channels: u32,
        maximum_request_frames: u32,
        history_frames: u32,
        maximum_child_frames: u32,
        maximum_spans: u32,
    ) -> Result<Cache, String> {
        let mut error = [0 as c_char; 256];
        // SAFETY: the error buffer is valid for its stated capacity.
        let raw = unsafe {
            st_unit_range_create(
                channels,
                maximum_request_frames,
                history_frames,
                maximum_child_frames,
                maximum_spans,
                error.as_mut_ptr(),
                error.len() as u32,
            )
        };
        if raw.is_null() {
            // SAFETY: the function always NUL-terminates the buffer it was given.
            let text = unsafe { CStr::from_ptr(error.as_ptr()) };
            Err(text.to_string_lossy().into_owned())
        } else {
            Ok(Cache { raw, channels, maximum_request: maximum_request_frames })
        }
    }

    /// `st_unit_range_reset`
    pub fn reset(&self) -> i32 {
        // SAFETY: `raw` is a live cache until `drop`.
        unsafe { st_unit_range_reset(self.raw) }
    }

    /// `st_unit_range_get_stats(cache, &stats)`: the status; `stats` is written only on success.
    pub fn stats_into(&self, stats: &mut STUnitRangeStats) -> i32 {
        // SAFETY: `raw` is a live cache and `stats` a valid place to write.
        unsafe { st_unit_range_get_stats(self.raw, stats) }
    }

    /// One pointer per channel; every buffer must cover what a failing call may clear.
    fn pointers(&self, frames: u32, output: &mut [&mut [f32]]) -> [*mut f32; 2] {
        assert_eq!(output.len(), self.channels as usize, "one buffer per channel");
        let needed = frames.min(self.maximum_request) as usize;
        let mut pointers = [std::ptr::null_mut(); 2];
        for (pointer, buffer) in pointers.iter_mut().zip(output.iter_mut()) {
            assert!(buffer.len() >= needed, "output buffer shorter than the request");
            *pointer = buffer.as_mut_ptr();
        }
        pointers
    }

    /// `st_unit_range_render(cache, frame, frames, output, nullptr, nullptr, nullptr, nullptr)`:
    /// a request that can only be served from retained audio.
    pub fn render_cached(&self, graph_frame: i64, frames: u32, output: &mut [&mut [f32]]) -> i32 {
        let pointers = self.pointers(frames, output);
        // SAFETY: a live cache; `pointers` holds one writable buffer per channel (checked above).
        unsafe {
            st_unit_range_render(
                self.raw,
                graph_frame,
                frames,
                pointers.as_ptr(),
                None,
                None,
                None,
                std::ptr::null_mut(),
            )
        }
    }

    /// `st_unit_range_render` with `source` as the callbacks named in `callbacks`.
    pub fn render(
        &self,
        graph_frame: i64,
        frames: u32,
        output: &mut [&mut [f32]],
        source: &mut dyn Source,
        callbacks: Callbacks,
    ) -> i32 {
        let pointers = self.pointers(frames, output);
        let mut call = Call { source, channels: self.channels as usize, panic: None };
        let prepare: STUnitRangePrepareFn = if callbacks.prepare { Some(prepare_trampoline) } else { None };
        let span: STUnitRangeSpanFn = if callbacks.span { Some(span_trampoline) } else { None };
        let process: STUnitRangeProcessFn = if callbacks.process { Some(process_trampoline) } else { None };
        // SAFETY: a live cache; one writable buffer per channel; `call` outlives the call and the
        // trampolines neither unwind nor touch anything but what they are handed.
        let status = unsafe {
            st_unit_range_render(
                self.raw,
                graph_frame,
                frames,
                pointers.as_ptr(),
                prepare,
                span,
                process,
                &mut call as *mut Call as *mut c_void,
            )
        };
        if let Some(payload) = call.panic.take() {
            resume_unwind(payload);
        }
        status
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `st_unit_range_create`, no call is in progress (`&mut self`)
        // and it is not used again.
        unsafe { st_unit_range_destroy(self.raw) }
    }
}

/// A source for calls that must be rejected before any callback runs.
pub struct NeverCalled;

impl Source for NeverCalled {
    fn prepare(&mut self, _graph_frame: i64, _frames: u32) -> i32 {
        panic!("prepare called on a request that must be rejected first");
    }
    fn span(&mut self, _graph_frame: i64, _frames: u32, _span: &mut STClockSpan) -> i32 {
        panic!("span called on a request that must be rejected first");
    }
    fn process(
        &mut self,
        _context: &STClockContext,
        _graph_frame: i64,
        _frames: u32,
        _output: &mut [&mut [f32]],
    ) -> i32 {
        panic!("process called on a request that must be rejected first");
    }
}
