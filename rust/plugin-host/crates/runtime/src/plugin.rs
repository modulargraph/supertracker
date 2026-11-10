//! The extension point: what a plug-in format backend implements.
//!
//! Threading contract (the runtime guarantees it; backends rely on it):
//!
//! * **Main thread** (the thread that owns [`crate::Runtime`], which must be the process's
//!   main thread): every method of [`PluginBackend`] and [`Plugin`], and
//!   [`Processor::reset`]. `Plugin` objects never leave this thread, so they need not be
//!   `Send` and may hold AppKit / COM objects directly. Editor windows are created here.
//! * **Real-time thread** (one per instance, owned by the runtime): [`Processor::process`]
//!   only. No allocation, locks or system calls that can block.
//! * **Exclusion**: `process` never runs concurrently with `Plugin::activate`,
//!   `Plugin::deactivate`, `Plugin::set_offline` or `Processor::reset`. While one of those
//!   runs, blocks requested by the host are answered "busy" (the host plays its fallback),
//!   so the audio thread of the host never waits for the main thread of the container.
//!   Every other `Plugin` method can run while `process` is running on the real-time
//!   thread, exactly as in an in-process host: share parameter values through atomics or a
//!   lock-free queue.

use std::fmt;

pub use stpc_transport::messages::{
    BusInfo, EditorInfo, InstanceInfo, ParameterInfo, PresetInfo, INFO_HAS_BYPASS, INFO_HAS_EDITOR, INFO_INSTRUMENT,
    INFO_PRODUCES_MIDI, INFO_WANTS_MIDI, PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_BYPASS, PARAM_HIDDEN, PARAM_READ_ONLY,
    PARAM_STEPPED,
};
pub use stpc_transport::region::{
    Event, Notification, Transport, EVENT_FLAG_SEQUENCED, EVENT_MIDI, EVENT_PARAM, TRANSPORT_LOOPING,
    TRANSPORT_PLAYING, TRANSPORT_PPQ_VALID, TRANSPORT_RECORDING, TRANSPORT_TEMPO_VALID, TRANSPORT_TIME_SIG_VALID,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginError {
    /// The plug-in does not have this capability. The host may substitute its own
    /// behaviour (e.g. bypass by dry pass-through).
    Unsupported,
    Failed(String),
}

impl PluginError {
    pub fn failed(message: impl Into<String>) -> PluginError {
        PluginError::Failed(message.into())
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Unsupported => write!(f, "not supported by this plug-in"),
            PluginError::Failed(message) => write!(f, "{message}"),
        }
    }
}
impl std::error::Error for PluginError {}

/// What the host asked for. The host fixes the bus layout (channel counts per bus); a
/// backend configures the plug-in to it or fails, and reports in [`InstanceInfo`] how many
/// channels of each bus the plug-in really uses.
#[derive(Clone, Debug)]
pub struct CreateRequest<'a> {
    pub plugin_id: &'a str,
    pub name: &'a str,
    /// Backend-specific options (e.g. a bundle path).
    pub options: &'a str,
    pub sample_rate: f64,
    pub max_frames: u32,
    /// Channels per input bus; bus 0 is the main input, further buses are sidechains.
    pub input_buses: &'a [u32],
    pub output_buses: &'a [u32],
}

/// A plug-in format ("clap", "vst3", "au", "test").
pub trait PluginBackend: 'static {
    /// The format name the host uses in its instance configuration.
    fn format(&self) -> &str;
    /// Loads and instantiates a plug-in. May take long (the host applies a generous
    /// timeout and other instances keep processing meanwhile).
    fn create(&self, request: &CreateRequest<'_>, host: HostHandle) -> Result<Box<dyn Plugin>, PluginError>;
}

/// One plug-in instance: everything except audio processing. Main thread only.
#[allow(unused_variables)]
pub trait Plugin {
    fn info(&mut self) -> InstanceInfo;

    /// Prepares processing and hands out the audio processor.
    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<Box<dyn Processor>, PluginError>;
    /// Ends processing; the processor comes back to be dismantled on the main thread.
    fn deactivate(&mut self, processor: Box<dyn Processor>);
    /// Offline rendering starts or ends. Called with processing excluded. Blocks also carry
    /// [`ProcessData::offline`].
    fn set_offline(&mut self, offline: bool) {}
    /// Plug-in's own bypass. `Unsupported` makes the host bypass by (latency-compensated)
    /// dry pass-through instead.
    fn set_bypass(&mut self, bypass: bool) -> Result<(), PluginError> {
        Err(PluginError::Unsupported)
    }

    fn parameters(&mut self) -> Vec<ParameterInfo> {
        Vec::new()
    }
    /// Current value, in the range of the parameter's metadata.
    fn parameter_value(&mut self, id: u64) -> Option<f64> {
        None
    }
    /// A non-timed change (host UI). Timed changes arrive as events in `process`.
    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), PluginError> {
        Err(PluginError::Unsupported)
    }
    /// Display text of a value ("-6.0 dB").
    fn format_parameter(&mut self, id: u64, value: f64) -> Option<String> {
        None
    }
    fn parse_parameter(&mut self, id: u64, text: &str) -> Option<f64> {
        None
    }

    /// The complete state as opaque bytes.
    fn state(&mut self) -> Result<Vec<u8>, PluginError> {
        Err(PluginError::Unsupported)
    }
    fn set_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        Err(PluginError::Unsupported)
    }
    fn presets(&mut self) -> Vec<PresetInfo> {
        Vec::new()
    }
    fn load_preset(&mut self, id: u32) -> Result<(), PluginError> {
        Err(PluginError::Unsupported)
    }

    /// Creates and shows the editor in a window of this process, titled `title`.
    fn open_editor(&mut self, title: &str) -> Result<EditorInfo, PluginError> {
        Err(PluginError::Unsupported)
    }
    fn close_editor(&mut self) {}
    /// Brings the editor window (and this process) to the front.
    fn focus_editor(&mut self) {}
    fn set_editor_title(&mut self, title: &str) {}

    /// Runs after [`HostHandle::request_main_callback`], and periodically if the runtime
    /// was configured with an idle interval.
    fn idle(&mut self) {}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    Ok,
    /// The plug-in failed to process; the runtime silences the outputs of this block.
    Failed,
}

/// The audio half of an instance. Lives on the instance's real-time thread between
/// `activate` and `deactivate`.
pub trait Processor: Send {
    /// Real-time thread.
    fn process(&mut self, data: &mut ProcessData<'_>) -> ProcessStatus;
    /// Clear delay lines, voices and envelopes. Main thread, with processing excluded.
    fn reset(&mut self) {}
}

/// Position of a bus inside the flat channel tables.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BusSpan {
    pub first: usize,
    pub channels: usize,
}

/// One block. Channel buffers point straight into the shared region (no copy on this
/// side); outputs are zero-filled before `process` is called.
pub struct ProcessData<'a> {
    pub frames: usize,
    /// Offline render: there is no deadline, take the time needed for full quality.
    pub offline: bool,
    /// Blocks were dropped before this one; held notes and envelopes may be stale.
    pub discontinuity: bool,
    pub transport: Transport,
    /// Incoming events, sorted by offset; every offset is below `frames`.
    pub events: &'a [Event],
    /// Bit n set: flat input channel n is all zeros.
    pub input_silence: u64,
    /// Set bit n to declare flat output channel n silent (optional).
    pub output_silence: u64,
    pub(crate) in_spans: &'a [BusSpan],
    pub(crate) out_spans: &'a [BusSpan],
    pub(crate) in_ptrs: &'a [*const f32],
    pub(crate) out_ptrs: &'a [*mut f32],
    pub(crate) out_events: *mut Event,
    pub(crate) out_event_capacity: usize,
    pub(crate) out_event_count: usize,
    pub(crate) out_events_dropped: bool,
}

impl<'a> ProcessData<'a> {
    pub fn input_bus_count(&self) -> usize {
        self.in_spans.len()
    }
    pub fn output_bus_count(&self) -> usize {
        self.out_spans.len()
    }
    /// Channels of input bus `bus` (0 for a bus that does not exist).
    pub fn input_channels(&self, bus: usize) -> usize {
        self.in_spans.get(bus).map_or(0, |span| span.channels)
    }
    pub fn output_channels(&self, bus: usize) -> usize {
        self.out_spans.get(bus).map_or(0, |span| span.channels)
    }

    /// Samples of one input channel. Panics if the bus or channel does not exist.
    pub fn input(&self, bus: usize, channel: usize) -> &[f32] {
        let span = self.in_spans[bus];
        assert!(channel < span.channels);
        // SAFETY: the pointer addresses `max_frames >= frames` samples of the region,
        // which this side owns for the duration of the block.
        unsafe { std::slice::from_raw_parts(self.in_ptrs[span.first + channel], self.frames) }
    }

    /// Samples of one output channel. Panics if the bus or channel does not exist.
    pub fn output(&mut self, bus: usize, channel: usize) -> &mut [f32] {
        let span = self.out_spans[bus];
        assert!(channel < span.channels);
        // SAFETY: as for `input`; `&mut self` prevents overlapping output borrows.
        unsafe { std::slice::from_raw_parts_mut(self.out_ptrs[span.first + channel], self.frames) }
    }

    /// One input and one output channel at once (they never overlap).
    pub fn io(&mut self, in_bus: usize, in_channel: usize, out_bus: usize, out_channel: usize) -> (&[f32], &mut [f32]) {
        let i = self.in_spans[in_bus];
        let o = self.out_spans[out_bus];
        assert!(in_channel < i.channels && out_channel < o.channels);
        // SAFETY: input and output channels are disjoint areas of the region.
        unsafe {
            (
                std::slice::from_raw_parts(self.in_ptrs[i.first + in_channel], self.frames),
                std::slice::from_raw_parts_mut(self.out_ptrs[o.first + out_channel], self.frames),
            )
        }
    }

    /// Raw channel pointers of an input bus, for backends that hand buffers to C plug-in
    /// APIs. Each addresses `frames` samples valid until `process` returns.
    pub fn input_ptrs(&self, bus: usize) -> &[*const f32] {
        let span = self.in_spans[bus];
        &self.in_ptrs[span.first..span.first + span.channels]
    }
    pub fn output_ptrs(&self, bus: usize) -> &[*mut f32] {
        let span = self.out_spans[bus];
        &self.out_ptrs[span.first..span.first + span.channels]
    }
    /// Index of a bus's first channel in the flat numbering used by the silence masks.
    pub fn input_flat_index(&self, bus: usize) -> usize {
        self.in_spans[bus].first
    }
    pub fn output_flat_index(&self, bus: usize) -> usize {
        self.out_spans[bus].first
    }

    /// Emits an event to the host (MIDI out), to be pushed in offset order. Returns false
    /// when the block's out-event list is full (the host learns that events were dropped).
    pub fn push_event(&mut self, event: Event) -> bool {
        if self.out_event_count >= self.out_event_capacity {
            self.out_events_dropped = true;
            return false;
        }
        // SAFETY: the index is below the capacity of the region's out-event list.
        unsafe { self.out_events.add(self.out_event_count).write(event) };
        self.out_event_count += 1;
        true
    }
}

pub use crate::runtime::HostHandle;
