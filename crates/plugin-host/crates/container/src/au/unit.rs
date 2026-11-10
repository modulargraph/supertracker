//! Instantiation and bus configuration of an `AUAudioUnit`, and the few version 2 calls
//! the backend makes on bridged units (macOS only; main thread).

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, AnyThread};
use objc2_audio_toolbox::{
    kAUParameterListener_AnyParameter, kAudioUnitProperty_BypassEffect, kAudioUnitProperty_ParameterInfo,
    kAudioUnitProperty_ParameterList, kAudioUnitScope_Global, AUAudioUnit, AUAudioUnitBus, AUAudioUnitBusArray,
    AUAudioUnitV2Bridge, AUParameterListenerNotify, AudioComponentDescription, AudioComponentFindNext,
    AudioComponentInstantiationOptions, AudioUnit, AudioUnitGetParameter, AudioUnitGetProperty,
    AudioUnitGetPropertyInfo, AudioUnitParameter, AudioUnitParameterInfo, AudioUnitParameterOptions,
};
use objc2_avf_audio::{AVAudioChannelLayout, AVAudioFormat};
use objc2_core_audio_types::kAudioChannelLayoutTag_DiscreteInOrder;
use objc2_core_foundation::CFRetained;
use objc2_foundation::NSError;
use stpc_runtime::PluginError;

use super::ident::{ComponentId, ComponentKind};
use super::layout::{self, Capability};
use super::objc::{describe_error, guard, guard_quiet, spin_until, wake_main_run_loop};

/// How long an Audio Unit may take to instantiate (the host's own limit is 120 s).
const INSTANTIATE_TIMEOUT: Duration = Duration::from_secs(100);

/// `CreateRequest::options` for the format "au": words separated by commas or spaces.
///
/// * `out-of-process`: ask the system to run a version 3 unit in its own extension
///   process (the system default) instead of inside the container.
/// * `in-process` (the default here): load the unit into the container. The system falls
///   back to out-of-process by itself when a version 3 unit is not packaged for it.
///   Version 2 units always run in the container.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Options {
    pub out_of_process: bool,
}

impl Options {
    pub fn parse(text: &str) -> Options {
        let mut options = Options::default();
        for word in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|w| !w.is_empty()) {
            match word {
                "out-of-process" => options.out_of_process = true,
                "in-process" => options.out_of_process = false,
                other => eprintln!("[au] ignoring unknown option \"{other}\""),
            }
        }
        options
    }
}

pub(super) fn description(id: ComponentId) -> AudioComponentDescription {
    AudioComponentDescription {
        componentType: id.kind.0,
        componentSubType: id.subtype.0,
        componentManufacturer: id.manufacturer.0,
        componentFlags: 0,
        componentFlagsMask: 0,
    }
}

pub(super) fn is_installed(id: ComponentId) -> bool {
    let mut description = description(id);
    // SAFETY: a registry query; the description outlives the call.
    !unsafe { AudioComponentFindNext(std::ptr::null_mut(), NonNull::from(&mut description)) }.is_null()
}

enum Pending {
    Waiting,
    /// The caller gave up; a unit that still arrives is left alone.
    Abandoned,
    /// A retained `AUAudioUnit` pointer, or what went wrong.
    Done(Result<usize, String>),
}

struct Instantiation {
    done: AtomicBool,
    state: Mutex<Pending>,
}

/// Instantiates the component. Always through the asynchronous entry point, which is the
/// only one that works for every unit (version 3 units that require asynchronous
/// instantiation refuse the synchronous initialiser); the main run loop is served while
/// waiting.
pub(super) fn instantiate(id: ComponentId, options: Options) -> Result<Retained<AUAudioUnit>, PluginError> {
    let shared = Arc::new(Instantiation { done: AtomicBool::new(false), state: Mutex::new(Pending::Waiting) });
    let for_block = shared.clone();
    // The handler runs on an arbitrary thread (or synchronously, for most version 2 units).
    let handler = RcBlock::new(move |unit: *mut AUAudioUnit, error: *mut NSError| {
        // SAFETY: the pointers are valid for the duration of the handler; the unit is
        // retained here and handed to the main thread as a raw pointer.
        let outcome = unsafe {
            match Retained::retain(unit) {
                Some(unit) => Ok(Retained::into_raw(unit) as usize),
                None => Err(match error.as_ref() {
                    Some(error) => describe_error(error),
                    None => "the system gave no reason".to_string(),
                }),
            }
        };
        let mut state = for_block.state.lock().unwrap();
        if matches!(*state, Pending::Waiting) {
            *state = Pending::Done(outcome);
        }
        // Abandoned: a late unit stays retained forever. Releasing it here could run a
        // plug-in's teardown on this arbitrary thread, which is worse than the leak.
        drop(state);
        for_block.done.store(true, Ordering::Release);
        wake_main_run_loop();
    });
    let flags = if options.out_of_process {
        AudioComponentInstantiationOptions::LoadOutOfProcess
    } else {
        AudioComponentInstantiationOptions::LoadInProcess
    };
    guard("being opened", || {
        // SAFETY: the description is a plain value; the handler is a heap block.
        unsafe {
            AUAudioUnit::instantiateWithComponentDescription_options_completionHandler(description(id), flags, &handler)
        }
    })?;
    let arrived = spin_until(&shared.done, INSTANTIATE_TIMEOUT);
    let mut state = shared.state.lock().unwrap();
    let outcome = std::mem::replace(&mut *state, Pending::Abandoned);
    drop(state);
    match outcome {
        Pending::Done(Ok(pointer)) => {
            // SAFETY: the handler stored a retained pointer; ownership moves here.
            unsafe { Retained::from_raw(pointer as *mut AUAudioUnit) }
                .ok_or_else(|| PluginError::failed("The Audio Unit could not be opened."))
        }
        Pending::Done(Err(reason)) => {
            Err(PluginError::failed(format!("The Audio Unit {id} could not be opened: {reason}")))
        }
        Pending::Waiting | Pending::Abandoned => {
            debug_assert!(!arrived);
            Err(PluginError::failed(format!(
                "The Audio Unit {id} did not finish opening within {} seconds.",
                INSTANTIATE_TIMEOUT.as_secs()
            )))
        }
    }
}

// ---- buses --------------------------------------------------------------------------------

/// One bus of the unit as configured.
#[derive(Clone, Debug, Default)]
pub(super) struct UnitBus {
    pub name: String,
    /// Channels of the bus's format; 0 if the bus is not used (disabled, or its format is
    /// not one this backend can render).
    pub channels: u32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct BusLayout {
    pub inputs: Vec<UnitBus>,
    pub outputs: Vec<UnitBus>,
}

fn bus_format(bus: &AUAudioUnitBus) -> Option<Retained<AVAudioFormat>> {
    // SAFETY: `format` is a readonly, nonnull object property of AUAudioUnitBus (the
    // framework crate does not bind it because of the AVFAudio type).
    unsafe { msg_send![bus, format] }
}

fn make_format(sample_rate: f64, channels: u32) -> Option<Retained<AVAudioFormat>> {
    // SAFETY: plain initialisers. More than two channels need an explicit layout; a
    // discrete one makes no claim about speaker positions.
    unsafe {
        if channels <= 2 {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(AVAudioFormat::alloc(), sample_rate, channels)
        } else {
            let layout = AVAudioChannelLayout::initWithLayoutTag(
                AVAudioChannelLayout::alloc(),
                kAudioChannelLayoutTag_DiscreteInOrder | channels,
            )?;
            Some(AVAudioFormat::initStandardFormatWithSampleRate_channelLayout(
                AVAudioFormat::alloc(),
                sample_rate,
                &layout,
            ))
        }
    }
}

fn set_bus_format(bus: &AUAudioUnitBus, sample_rate: f64, channels: u32) -> Result<(), String> {
    let format = make_format(sample_rate, channels)
        .ok_or_else(|| format!("no {channels}-channel format at {sample_rate} Hz"))?;
    // SAFETY: `setFormat:error:` of AUAudioUnitBus (unbound for the same reason as `format`).
    let result: Result<(), Retained<NSError>> = unsafe { msg_send![bus, setFormat: &*format, error: _] };
    result.map_err(|error| describe_error(&error))
}

/// Channels of a bus if this backend can render its current format at `sample_rate`
/// (deinterleaved 32-bit float), else 0.
fn usable_channels(bus: &AUAudioUnitBus, sample_rate: f64) -> u32 {
    let Some(format) = bus_format(bus) else { return 0 };
    // SAFETY: property reads.
    unsafe {
        let channels = format.channelCount();
        let ok = format.isStandard()
            && (format.sampleRate() - sample_rate).abs() < 0.5
            && (1..=layout::MAX_BUS_CHANNELS).contains(&channels);
        if ok {
            channels
        } else {
            0
        }
    }
}

fn bus_name(bus: &AUAudioUnitBus) -> String {
    // SAFETY: property read.
    unsafe { bus.name() }.map(|name| name.to_string()).unwrap_or_default()
}

fn set_enabled(bus: &AUAudioUnitBus, enabled: bool) {
    // Not every unit lets a host change this; then the bus stays as it is.
    // SAFETY: property accessors.
    guard_quiet("enabling or disabling a bus", || unsafe {
        if bus.isEnabled() != enabled {
            bus.setEnabled(enabled);
        }
    });
}

/// Configures an auxiliary bus for `host_channels` (0: the host has no such bus).
fn configure_auxiliary(bus: &AUAudioUnitBus, sample_rate: f64, host_channels: u32, input: bool) {
    if host_channels == 0 {
        if input {
            // An unconnected sidechain must look unconnected to the unit.
            set_enabled(bus, false);
        }
        return;
    }
    let _ = layout::sidechain_candidates(host_channels)
        .into_iter()
        .find(|&channels| set_bus_format(bus, sample_rate, channels).is_ok());
    // If none was accepted the bus keeps its format; `usable_channels` decides later.
    set_enabled(bus, true);
}

fn channel_capabilities(unit: &AUAudioUnit) -> Option<Vec<Capability>> {
    // SAFETY: property read.
    let numbers = unsafe { unit.channelCapabilities() }?;
    let flat: Vec<i32> = numbers.iter().map(|number| number.intValue()).collect();
    Some(layout::capabilities(&flat))
}

fn bus_at(buses: &AUAudioUnitBusArray, index: usize) -> Retained<AUAudioUnitBus> {
    // SAFETY: callers stay below `count`.
    unsafe { buses.objectAtIndexedSubscript(index) }
}

/// Configures every bus of `unit` for the host's layout at `sample_rate` and proves the
/// configuration by allocating render resources once (a unit judges the combination of
/// its bus formats only then; the old host learned of a refusal as an opaque engine error
/// later). Leaves the unit unallocated.
pub(super) fn configure(
    unit: &AUAudioUnit,
    kind: ComponentKind,
    sample_rate: f64,
    max_frames: u32,
    host_in: &[u32],
    host_out: &[u32],
) -> Result<BusLayout, PluginError> {
    guard("configuring its audio buses", || {
        configure_unguarded(unit, kind, sample_rate, max_frames, host_in, host_out)
    })?
}

fn configure_unguarded(
    unit: &AUAudioUnit,
    kind: ComponentKind,
    sample_rate: f64,
    max_frames: u32,
    host_in: &[u32],
    host_out: &[u32],
) -> Result<BusLayout, PluginError> {
    // SAFETY: main thread; the unit is not rendering (the caller holds it deactivated).
    unsafe {
        if unit.renderResourcesAllocated() {
            unit.deallocateRenderResources();
        }
        let inputs = unit.inputBusses();
        let outputs = unit.outputBusses();
        let (input_count, output_count) = (inputs.count(), outputs.count());
        if output_count == 0 {
            return Err(PluginError::failed("The plug-in does not provide an audio output."));
        }
        if matches!(kind, ComponentKind::Effect | ComponentKind::MusicEffect) && input_count == 0 {
            return Err(PluginError::failed("The plug-in does not provide an audio input."));
        }
        // An instrument's input bus (a vocoder's, say) is an auxiliary input: the main
        // pair of a source is "nothing in, bus 0 out".
        let main_input = input_count > 0 && !kind.is_source();
        let host = |buses: &[u32], index: usize| buses.get(index).copied().unwrap_or(0);

        for index in (if main_input { 1 } else { 0 })..input_count {
            configure_auxiliary(&bus_at(&inputs, index), sample_rate, host(host_in, index), true);
        }
        for index in 1..output_count {
            configure_auxiliary(&bus_at(&outputs, index), sample_rate, host(host_out, index), false);
        }

        unit.setMaximumFramesToRender(max_frames.max(1));
        let capabilities = channel_capabilities(unit);
        let candidates =
            layout::candidates(capabilities.as_deref(), main_input, host(host_in, 0), host(host_out, 0).max(1));
        let main_out = bus_at(&outputs, 0);
        let main_in = main_input.then(|| bus_at(&inputs, 0));
        let mut refusals = Vec::new();
        let mut accepted = false;
        for (ins, outs) in candidates {
            let attempt = (|| -> Result<(), String> {
                if let Some(bus) = &main_in {
                    set_bus_format(bus, sample_rate, ins)?;
                    set_enabled(bus, true);
                }
                set_bus_format(&main_out, sample_rate, outs)?;
                unit.allocateRenderResourcesAndReturnError().map_err(|error| describe_error(&error))?;
                unit.deallocateRenderResources();
                Ok(())
            })();
            match attempt {
                Ok(()) => {
                    accepted = true;
                    break;
                }
                Err(reason) => refusals.push(format!("{ins} in, {outs} out: {reason}")),
            }
        }
        if !accepted {
            // Last resort: whatever formats the unit has now, if they can be rendered.
            let usable = usable_channels(&main_out, sample_rate) > 0
                && main_in.as_ref().is_none_or(|bus| usable_channels(bus, sample_rate) > 0);
            let allocated = usable
                && match unit.allocateRenderResourcesAndReturnError() {
                    Ok(()) => {
                        unit.deallocateRenderResources();
                        true
                    }
                    Err(error) => {
                        refusals.push(format!("its own format: {}", describe_error(&error)));
                        false
                    }
                };
            if !allocated {
                return Err(PluginError::failed(format!(
                    "This Audio Unit cannot process audio in this channel layout at {} Hz ({}).",
                    sample_rate.round(),
                    refusals.join("; ")
                )));
            }
        }

        let describe = |buses: &AUAudioUnitBusArray, count: usize| -> Vec<UnitBus> {
            (0..count)
                .map(|index| {
                    let bus = bus_at(buses, index);
                    let enabled = bus.isEnabled();
                    UnitBus {
                        name: bus_name(&bus),
                        channels: if enabled { usable_channels(&bus, sample_rate) } else { 0 },
                    }
                })
                .collect()
        };
        let layout = BusLayout { inputs: describe(&inputs, input_count), outputs: describe(&outputs, output_count) };
        if layout.outputs[0].channels == 0 || (main_input && layout.inputs[0].channels == 0) {
            return Err(PluginError::failed(
                "The Audio Unit's main input or output uses a sample format this host cannot render.",
            ));
        }
        Ok(layout)
    }
}

// ---- version 2 units behind the bridge ----------------------------------------------------

/// The `AudioUnit` of a version 2 unit that the system wraps in `AUAudioUnitV2Bridge`.
#[derive(Clone, Copy)]
pub(super) struct V2(AudioUnit);

impl V2 {
    pub fn of(unit: &AUAudioUnit) -> Option<V2> {
        let object: &AnyObject = unit;
        let bridge = object.downcast_ref::<AUAudioUnitV2Bridge>()?;
        // SAFETY: `audioUnit` is a readonly property of the bridge (macOS 10.13 and later;
        // checked, because a missing selector would raise).
        let handle = guard_quiet("looking up the version 2 unit", || unsafe {
            let responds: bool = msg_send![bridge, respondsToSelector: sel!(audioUnit)];
            if responds {
                bridge.audioUnit()
            } else {
                std::ptr::null_mut()
            }
        })?;
        (!handle.is_null()).then_some(V2(handle))
    }

    /// Ids of the global-scope parameters. Only these read back unambiguously: the bridge
    /// gives parameters of other scopes encoded addresses.
    pub fn global_parameters(&self) -> Vec<u32> {
        let mut size = 0u32;
        // SAFETY: standard property queries; the buffer is sized by the first call.
        unsafe {
            let status = AudioUnitGetPropertyInfo(
                self.0,
                kAudioUnitProperty_ParameterList,
                kAudioUnitScope_Global,
                0,
                &mut size,
                std::ptr::null_mut(),
            );
            if status != 0 || size == 0 {
                return Vec::new();
            }
            let mut ids = vec![0u32; size as usize / 4];
            let status = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ParameterList,
                kAudioUnitScope_Global,
                0,
                NonNull::new_unchecked(ids.as_mut_ptr().cast::<c_void>()),
                NonNull::from(&mut size),
            );
            if status != 0 {
                return Vec::new();
            }
            ids.truncate(size as usize / 4);
            ids
        }
    }

    /// The unit's own current value of a global parameter. Apple's bridge caches each
    /// `AUParameter` value and refreshes it only when the plug-in posts a change; plug-ins
    /// that apply a write on their next render and post nothing leave the cache stale
    /// indefinitely. This reads past the cache.
    pub fn parameter_value(&self, id: u32) -> Option<f32> {
        let mut value = 0f32;
        // SAFETY: AudioUnitGetParameter is callable from any thread.
        let status = unsafe { AudioUnitGetParameter(self.0, id, kAudioUnitScope_Global, 0, NonNull::from(&mut value)) };
        (status == 0 && value.is_finite()).then_some(value)
    }

    /// The default value the unit states for a global parameter.
    pub fn parameter_default(&self, id: u32) -> Option<f32> {
        // SAFETY: the struct is plain data plus two CFString pointers, zero is valid.
        unsafe {
            let mut info: AudioUnitParameterInfo = std::mem::zeroed();
            let mut size = std::mem::size_of::<AudioUnitParameterInfo>() as u32;
            let status = AudioUnitGetProperty(
                self.0,
                kAudioUnitProperty_ParameterInfo,
                kAudioUnitScope_Global,
                id,
                NonNull::from(&mut info).cast::<c_void>(),
                NonNull::from(&mut size),
            );
            if status != 0 {
                return None;
            }
            // The name string is the caller's to release when the unit says so (the unit
            // name is not).
            let owned = AudioUnitParameterOptions::Flag_HasCFNameString | AudioUnitParameterOptions::Flag_CFNameRelease;
            if info.flags.contains(owned) {
                if let Some(name) = NonNull::new(info.cfNameString.cast_mut()) {
                    drop(CFRetained::from_raw(name));
                }
            }
            info.defaultValue.is_finite().then_some(info.defaultValue)
        }
    }

    /// Whether the unit implements `kAudioUnitProperty_BypassEffect` as a writable property.
    pub fn supports_bypass(&self) -> bool {
        let mut size = 0u32;
        let mut writable = 0u8;
        // SAFETY: a property query with valid out pointers.
        let status = unsafe {
            AudioUnitGetPropertyInfo(
                self.0,
                kAudioUnitProperty_BypassEffect,
                kAudioUnitScope_Global,
                0,
                &mut size,
                &mut writable,
            )
        };
        status == 0 && writable != 0
    }

    /// Tells every parameter listener (the unit's own Cocoa view among them) that all
    /// values may have changed: the host-side duty after a state or preset load.
    pub fn notify_all_parameters_changed(&self) {
        let mut parameter = AudioUnitParameter {
            mAudioUnit: self.0,
            mParameterID: kAUParameterListener_AnyParameter,
            mScope: kAudioUnitScope_Global,
            mElement: 0,
        };
        // SAFETY: a notification with a valid parameter description; no sending listener.
        unsafe {
            AUParameterListenerNotify(std::ptr::null_mut(), std::ptr::null_mut(), NonNull::from(&mut parameter));
        }
    }
}
