//! One VST3 instance on the container's main thread: component, edit controller and
//! everything the [`Plugin`] trait asks of them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use stpc_runtime::{
    BusInfo, CreateRequest, EditorInfo, HostHandle, InstanceInfo, Notification, ParameterInfo, Plugin, PluginError,
    PresetInfo, Processor, INFO_HAS_BYPASS, INFO_HAS_EDITOR, INFO_INSTRUMENT, INFO_PRODUCES_MIDI, INFO_WANTS_MIDI,
    PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_BYPASS, PARAM_HIDDEN, PARAM_READ_ONLY, PARAM_STEPPED,
};
use vst3::Steinberg::Vst::{
    self as vst, IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentHandler, IComponentTrait,
    IConnectionPoint, IConnectionPointTrait, IEditController, IEditControllerTrait, IMidiMapping, IMidiMappingTrait,
    IUnitInfo, IUnitInfoTrait, SpeakerArrangement,
};
use vst3::Steinberg::{kNotImplemented, kResultOk, FUnknown, IPluginBaseTrait, TUID};
use vst3::{ComPtr, ComWrapper};

use super::editor::Editor;
use super::host::{stream_ptr, ComponentHandler, HostApplication, MemoryStream};
use super::module::{cid_words, wide_to_string, ClassInfo, Module};
use super::process::{
    BusMap, Flusher, ProcessorConfig, Shared, Vst3Processor, K_OFFLINE, MIDI_CONTROLLERS, NO_PARAMETER, SYNC_NOTIFY,
};
use super::state;

const K_AUDIO: i32 = vst::MediaTypes_::kAudio as i32;
const K_EVENT: i32 = vst::MediaTypes_::kEvent as i32;
const K_INPUT: i32 = vst::BusDirections_::kInput as i32;
const K_OUTPUT: i32 = vst::BusDirections_::kOutput as i32;
const K_AUX: i32 = vst::BusTypes_::kAux as i32;
const K_REALTIME: i32 = vst::ProcessModes_::kRealtime as i32;
const K_SAMPLE_32: i32 = vst::SymbolicSampleSizes_::kSample32 as i32;

const RESTART_RELOAD: u32 = vst::RestartFlags_::kReloadComponent as u32;
const RESTART_IO: u32 = vst::RestartFlags_::kIoChanged as u32;
const RESTART_VALUES: u32 = vst::RestartFlags_::kParamValuesChanged as u32;
const RESTART_LATENCY: u32 = vst::RestartFlags_::kLatencyChanged as u32;
const RESTART_TITLES: u32 = vst::RestartFlags_::kParamTitlesChanged as u32;
const RESTART_MIDI: u32 = vst::RestartFlags_::kMidiCCAssignmentChanged as u32;

const FLAG_AUTOMATE: i32 = vst::ParameterInfo_::ParameterFlags_::kCanAutomate as i32;
const FLAG_READ_ONLY: i32 = vst::ParameterInfo_::ParameterFlags_::kIsReadOnly as i32;
const FLAG_LIST: i32 = vst::ParameterInfo_::ParameterFlags_::kIsList as i32;
const FLAG_HIDDEN: i32 = vst::ParameterInfo_::ParameterFlags_::kIsHidden as i32;
const FLAG_PROGRAM: i32 = vst::ParameterInfo_::ParameterFlags_::kIsProgramChange as i32;
const FLAG_BYPASS: i32 = vst::ParameterInfo_::ParameterFlags_::kIsBypass as i32;

/// One audio bus of the plug-in.
#[derive(Clone, Debug)]
struct PlugBus {
    name: String,
    channels: u32,
    auxiliary: bool,
    /// The host bus this one is connected to.
    host: Option<usize>,
}

/// The program change parameter and the program list it selects from.
#[derive(Clone, Copy, Debug)]
struct Programs {
    parameter: u32,
    steps: i32,
    list: i32,
    count: i32,
}

pub struct Vst3Plugin {
    class: ClassInfo,
    shared: Arc<Shared>,
    /// The context the component and controller were initialized with.
    _host_context: ComWrapper<HostApplication>,
    handler: ComWrapper<ComponentHandler>,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// The controller is an object of its own (initialized, terminated and connected separately).
    separate_controller: bool,
    component_initialized: bool,
    controller_initialized: bool,
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    unit_info: Option<ComPtr<IUnitInfo>>,
    midi_mapping: Option<ComPtr<IMidiMapping>>,
    host_in: Vec<u32>,
    host_out: Vec<u32>,
    in_buses: Vec<PlugBus>,
    out_buses: Vec<PlugBus>,
    event_inputs: i32,
    event_outputs: i32,
    sample_rate: f64,
    max_frames: u32,
    event_capacity: usize,
    out_event_capacity: usize,
    /// Between `Plugin::activate` and `Plugin::deactivate`.
    active: bool,
    restart_requested: bool,
    io_changed: bool,
    parameters: Option<Vec<ParameterInfo>>,
    parameter_ids: HashSet<u32>,
    bypass_parameter: Option<u32>,
    programs: Option<Programs>,
    flusher: Flusher,
    has_editor: bool,
    editor: Option<Editor>,
    /// Last: the module is unloaded (when this was its last instance) after every object
    /// of it is gone.
    module: Rc<Module>,
}

fn failed(message: impl Into<String>) -> PluginError {
    PluginError::Failed(message.into())
}

fn arrangement_for(channels: u32, current: SpeakerArrangement) -> SpeakerArrangement {
    match channels {
        1 => vst::SpeakerArr::kMono,
        2 => vst::SpeakerArr::kStereo,
        n if current.count_ones() == n => current,
        n => 1u64.checked_shl(n).map_or(u64::MAX, |bit| bit - 1),
    }
}

/// Connects host buses to plug-in buses: host bus 0 is the main bus, so it goes to the
/// plug-in's first main bus even if an auxiliary bus precedes it; the others follow in order.
fn assign_buses(buses: &mut [PlugBus], host_buses: usize) {
    let first_main = buses.iter().position(|bus| !bus.auxiliary);
    let order = first_main.into_iter().chain((0..buses.len()).filter(|index| Some(*index) != first_main));
    for (host, bus) in order.enumerate().take(host_buses) {
        buses[bus].host = Some(host);
    }
}

impl Vst3Plugin {
    /// # Safety
    /// Main thread; runs the plug-in's constructors and initialisers.
    pub unsafe fn create(
        module: Rc<Module>,
        class: ClassInfo,
        request: &CreateRequest<'_>,
        handle: HostHandle,
    ) -> Result<Vst3Plugin, PluginError> {
        let host_context = ComWrapper::new(HostApplication);
        let context = host_context.as_com_ref::<FUnknown>().unwrap().as_ptr();
        let name = &class.name;
        let Some(component) = module.create::<IComponent>(&class.cid) else {
            return Err(failed(format!("the VST3 module could not create \"{name}\"")));
        };
        if component.initialize(context) != kResultOk {
            return Err(failed(format!("the VST3 component of \"{name}\" failed to initialise")));
        }
        let Some(processor) = component.cast::<IAudioProcessor>() else {
            component.terminate();
            return Err(failed(format!("\"{name}\" is not an audio processor")));
        };
        let shared = Arc::new(Shared::new(handle));
        let handler = ComWrapper::new(ComponentHandler { shared: shared.clone() });
        let mut this = Vst3Plugin {
            class,
            shared,
            _host_context: host_context,
            handler,
            component,
            processor,
            controller: None,
            separate_controller: false,
            component_initialized: true,
            controller_initialized: false,
            connection: None,
            unit_info: None,
            midi_mapping: None,
            host_in: request.input_buses.to_vec(),
            host_out: request.output_buses.to_vec(),
            in_buses: Vec::new(),
            out_buses: Vec::new(),
            event_inputs: 0,
            event_outputs: 0,
            sample_rate: request.sample_rate,
            max_frames: request.max_frames,
            event_capacity: request.event_capacity as usize,
            out_event_capacity: request.out_event_capacity as usize,
            active: false,
            restart_requested: false,
            io_changed: false,
            parameters: None,
            parameter_ids: HashSet::new(),
            bypass_parameter: None,
            programs: None,
            flusher: Flusher::new(0),
            has_editor: false,
            editor: None,
            module,
        };
        // From here on dropping `this` tears down whatever exists.
        if this.processor.canProcessSampleSize(K_SAMPLE_32) != kResultOk {
            return Err(failed(format!("\"{}\" cannot process 32-bit float audio", this.class.name)));
        }
        this.create_controller(context)?;
        this.negotiate_buses();
        this.shared.latency.store(this.processor.getLatencySamples(), Ordering::Release);
        this.scan_parameters();
        this.flusher = Flusher::new(this.parameter_ids.len());
        this.refresh_midi_mapping();
        if let Some(controller) = &this.controller {
            this.has_editor = Editor::probe(controller);
        }
        Ok(this)
    }

    unsafe fn create_controller(&mut self, context: *mut FUnknown) -> Result<(), PluginError> {
        // Single-component plug-ins: the component is the controller.
        self.controller = self.component.cast::<IEditController>();
        if self.controller.is_none() {
            let mut cid: TUID = [0; 16];
            if self.component.getControllerClassId(&mut cid) == kResultOk && cid != [0; 16] {
                if let Some(controller) = self.module.create::<IEditController>(&cid) {
                    if controller.initialize(context) != kResultOk {
                        return Err(failed(format!(
                            "the VST3 edit controller of \"{}\" failed to initialise",
                            self.class.name
                        )));
                    }
                    self.controller = Some(controller);
                    self.separate_controller = true;
                    self.controller_initialized = true;
                }
            }
        }
        let Some(controller) = self.controller.clone() else { return Ok(()) };
        controller.setComponentHandler(self.handler.as_com_ref::<IComponentHandler>().unwrap().as_ptr());
        if self.separate_controller {
            let ends = (self.component.cast::<IConnectionPoint>(), controller.cast::<IConnectionPoint>());
            if let (Some(component_end), Some(controller_end)) = ends {
                component_end.connect(controller_end.as_ptr());
                controller_end.connect(component_end.as_ptr());
                self.connection = Some((component_end, controller_end));
            }
        }
        // The controller starts from the component's state.
        let stream = MemoryStream::writer();
        if self.component.getState(stream_ptr(&stream)) == kResultOk {
            stream.rewind();
            self.sync_controller_state(&controller, &stream);
        }
        self.unit_info = controller.cast::<IUnitInfo>();
        self.midi_mapping = controller.cast::<IMidiMapping>();
        Ok(())
    }

    /// `setComponentState`. A controller may not implement it; one that reports a failure
    /// is left as it is (its editor may show stale values), as other hosts do.
    unsafe fn sync_controller_state(&self, controller: &ComPtr<IEditController>, stream: &ComWrapper<MemoryStream>) {
        let result = controller.setComponentState(stream_ptr(stream));
        if result != kResultOk && result != kNotImplemented {
            eprintln!("[vst3] the edit controller of \"{}\" rejected the component state ({result})", self.class.name);
        }
    }

    unsafe fn read_buses(&self, direction: i32) -> Vec<PlugBus> {
        (0..self.component.getBusCount(K_AUDIO, direction).clamp(0, 64))
            .map(|index| {
                let mut info: vst::BusInfo = std::mem::zeroed();
                if self.component.getBusInfo(K_AUDIO, direction, index, &mut info) != kResultOk {
                    return PlugBus { name: String::new(), channels: 0, auxiliary: index > 0, host: None };
                }
                PlugBus {
                    name: wide_to_string(&info.name),
                    channels: info.channelCount.clamp(0, 64) as u32,
                    auxiliary: info.busType == K_AUX,
                    host: None,
                }
            })
            .collect()
    }

    /// Asks the plug-in for the bus layout the host fixed at creation, then takes the
    /// layout the plug-in settles on: whatever differs is bridged with scratch channels.
    unsafe fn negotiate_buses(&mut self) {
        let mut inputs = self.read_buses(K_INPUT);
        let mut outputs = self.read_buses(K_OUTPUT);
        assign_buses(&mut inputs, self.host_in.len());
        assign_buses(&mut outputs, self.host_out.len());
        let wanted = |buses: &[PlugBus], host: &[u32], direction: i32| -> Vec<SpeakerArrangement> {
            buses
                .iter()
                .enumerate()
                .map(|(index, bus)| {
                    let mut current: SpeakerArrangement = 0;
                    if self.processor.getBusArrangement(direction, index as i32, &mut current) != kResultOk {
                        current = arrangement_for(bus.channels, 0);
                    }
                    match bus.host.map(|host_bus| host[host_bus]) {
                        Some(channels) if channels > 0 => arrangement_for(channels, current),
                        _ => current,
                    }
                })
                .collect()
        };
        let mut in_arrangements = wanted(&inputs, &self.host_in, K_INPUT);
        let mut out_arrangements = wanted(&outputs, &self.host_out, K_OUTPUT);
        // A refusal leaves the plug-in's own layout in place.
        self.processor.setBusArrangements(
            in_arrangements.as_mut_ptr(),
            in_arrangements.len() as i32,
            out_arrangements.as_mut_ptr(),
            out_arrangements.len() as i32,
        );
        for (buses, direction) in [(&mut inputs, K_INPUT), (&mut outputs, K_OUTPUT)] {
            for (bus, settled) in buses.iter_mut().zip(self.read_buses(direction)) {
                bus.channels = settled.channels;
                bus.name = settled.name;
            }
        }
        self.in_buses = inputs;
        self.out_buses = outputs;
        self.event_inputs = self.component.getBusCount(K_EVENT, K_INPUT).clamp(0, 64);
        self.event_outputs = self.component.getBusCount(K_EVENT, K_OUTPUT).clamp(0, 64);
        self.io_changed = false;
    }

    fn controller(&self) -> Option<&ComPtr<IEditController>> {
        self.controller.as_ref()
    }

    /// Reads the parameter list for what the backend itself needs from it.
    unsafe fn scan_parameters(&mut self) {
        self.parameters = None;
        self.parameter_ids.clear();
        self.bypass_parameter = None;
        self.programs = None;
        let Some(controller) = self.controller.clone() else { return };
        let mut program: Option<(u32, i32, i32)> = None;
        for index in 0..controller.getParameterCount().clamp(0, 1 << 20) {
            let mut info: vst::ParameterInfo = std::mem::zeroed();
            if controller.getParameterInfo(index, &mut info) != kResultOk {
                continue;
            }
            self.parameter_ids.insert(info.id);
            if info.flags & FLAG_BYPASS != 0 && self.bypass_parameter.is_none() {
                self.bypass_parameter = Some(info.id);
            }
            // The program change parameter of the root unit wins over those of sub-units.
            let better = program.is_none_or(|(_, _, unit)| unit != vst::kRootUnitId && info.unitId == vst::kRootUnitId);
            if info.flags & FLAG_PROGRAM != 0 && better {
                program = Some((info.id, info.stepCount, info.unitId));
            }
        }
        if let Some((parameter, steps, unit)) = program {
            let (list, count) = self.program_list_of(unit).unwrap_or((vst::kNoProgramListId, steps + 1));
            let steps = if steps > 0 { steps } else { (count - 1).max(0) };
            self.programs = Some(Programs { parameter, steps, list, count });
        }
        let (parameter, steps) = self.programs.map_or((NO_PARAMETER, 0), |programs| (programs.parameter, programs.steps));
        self.shared.program_steps.store(steps, Ordering::Relaxed);
        self.shared.program_parameter.store(parameter, Ordering::Relaxed);
    }

    /// The program list of a unit: its id and number of programs.
    unsafe fn program_list_of(&self, unit: i32) -> Option<(i32, i32)> {
        let units = self.unit_info.as_ref()?;
        let mut list = vst::kNoProgramListId;
        for index in 0..units.getUnitCount().clamp(0, 4096) {
            let mut info: vst::UnitInfo = std::mem::zeroed();
            if units.getUnitInfo(index, &mut info) == kResultOk && info.id == unit {
                list = info.programListId;
            }
        }
        for index in 0..units.getProgramListCount().clamp(0, 4096) {
            let mut info: vst::ProgramListInfo = std::mem::zeroed();
            if units.getProgramListInfo(index, &mut info) == kResultOk && info.id == list {
                return Some((info.id, info.programCount.max(0)));
            }
        }
        None
    }

    /// Unit id -> "Parent/Child" path (the root unit has the empty path).
    unsafe fn unit_paths(&self) -> HashMap<i32, String> {
        let mut units: HashMap<i32, (i32, String)> = HashMap::new();
        if let Some(info_source) = &self.unit_info {
            for index in 0..info_source.getUnitCount().clamp(0, 4096) {
                let mut info: vst::UnitInfo = std::mem::zeroed();
                if info_source.getUnitInfo(index, &mut info) == kResultOk {
                    units.insert(info.id, (info.parentUnitId, wide_to_string(&info.name)));
                }
            }
        }
        units
            .keys()
            .map(|&id| {
                let mut names = Vec::new();
                let mut current = id;
                // Bounded: a malformed unit tree must not loop.
                for _ in 0..32 {
                    if current == vst::kRootUnitId {
                        break;
                    }
                    let Some((parent, name)) = units.get(&current) else { break };
                    names.push(name.clone());
                    current = *parent;
                }
                names.reverse();
                (id, names.join("/"))
            })
            .collect()
    }

    unsafe fn refresh_midi_mapping(&self) {
        for channel in 0..16i16 {
            for controller in 0..MIDI_CONTROLLERS as i16 {
                let mut id = NO_PARAMETER;
                if let Some(mapping) = &self.midi_mapping {
                    // The out-parameter means nothing unless the call succeeds.
                    if mapping.getMidiControllerAssignment(0, channel, controller, &mut id) != kResultOk {
                        id = NO_PARAMETER;
                    }
                }
                self.shared.midi_map[channel as usize * MIDI_CONTROLLERS + controller as usize]
                    .store(id, Ordering::Relaxed);
            }
        }
    }

    fn process_mode(&self) -> i32 {
        self.shared.process_mode.load(Ordering::Relaxed)
    }

    /// Setup, bus activation, `setActive(true)`, `setProcessing(true)`: the VST3 order.
    unsafe fn start(&mut self, sample_rate: f64, max_frames: u32) -> Result<(), PluginError> {
        let mut setup = vst::ProcessSetup {
            processMode: self.process_mode(),
            symbolicSampleSize: K_SAMPLE_32,
            maxSamplesPerBlock: max_frames as i32,
            sampleRate: sample_rate,
        };
        let result = self.processor.setupProcessing(&mut setup);
        if result != kResultOk && result != kNotImplemented {
            return Err(failed(format!(
                "\"{}\" refused the processing setup ({sample_rate} Hz, {max_frames} frames)",
                self.class.name
            )));
        }
        // Buses are switched after the setup and before activation.
        for (buses, host, direction) in
            [(&self.in_buses, &self.host_in, K_INPUT), (&self.out_buses, &self.host_out, K_OUTPUT)]
        {
            for (index, bus) in buses.iter().enumerate() {
                let used = bus.host.is_some_and(|host_bus| host[host_bus] > 0) && bus.channels > 0;
                let result = self.component.activateBus(K_AUDIO, direction, index as i32, used as u8);
                if used && result != kResultOk && result != kNotImplemented {
                    eprintln!("[vst3] \"{}\" refused to activate its audio bus \"{}\"", self.class.name, bus.name);
                }
            }
        }
        for index in 0..self.event_inputs {
            self.component.activateBus(K_EVENT, K_INPUT, index, 1);
        }
        for index in 0..self.event_outputs {
            self.component.activateBus(K_EVENT, K_OUTPUT, index, (self.out_event_capacity > 0) as u8);
        }
        if self.component.setActive(1) != kResultOk {
            return Err(failed(format!("\"{}\" refused to activate", self.class.name)));
        }
        // setProcessing is optional: "not implemented" is not a refusal.
        let result = self.processor.setProcessing(1);
        if result != kResultOk && result != kNotImplemented {
            self.component.setActive(0);
            return Err(failed(format!("\"{}\" refused to start processing", self.class.name)));
        }
        Ok(())
    }

    unsafe fn stop(&mut self) {
        self.processor.setProcessing(0);
        self.component.setActive(0);
    }

    /// Tells the controller values it has not seen and the host application those the
    /// plug-in produced.
    unsafe fn absorb(&mut self, produced: Vec<(u32, f64)>) {
        for (id, value) in produced {
            if let Some(controller) = self.controller() {
                controller.setParamNormalized(id, value);
            }
            self.shared.handle.notify(Notification::ParamValue { id: id as u64, value });
        }
    }

    /// Moves edits parked in the overflow list into the ring, as far as they fit.
    fn drain_overflow(&mut self) {
        let mut overflow = self.shared.overflow.lock().unwrap();
        let mut moved = 0;
        for &(id, value) in overflow.iter() {
            if !self.shared.send_to_processor(id, value) {
                break;
            }
            moved += 1;
        }
        overflow.drain(..moved);
    }

    /// Hands every pending untimed change to the processor now, with a zero-sample block
    /// on this thread. False if the processor could not be reached.
    unsafe fn flush(&mut self) -> bool {
        for _ in 0..64 {
            self.drain_overflow();
            if self.shared.to_processor.is_empty() {
                return true;
            }
            let produced = if self.active {
                let (flusher, processor, shared) = (&self.flusher, &self.processor, &self.shared);
                match self.shared.handle.with_processing_excluded(|| flusher.run(processor, shared)) {
                    Some(produced) => produced,
                    None => return false,
                }
            } else {
                // Not activated: bring the component up just for the flush.
                if self.start(self.sample_rate, self.max_frames).is_err() {
                    return false;
                }
                let produced = self.flusher.run(&self.processor, &self.shared);
                self.stop();
                produced
            };
            self.absorb(produced);
        }
        !self.shared.has_pending()
    }

    /// Before the state is read: makes sure the processor has every change the host or
    /// the editor made. While audio runs the next block delivers them; otherwise this
    /// thread flushes.
    unsafe fn settle_pending(&mut self) {
        if !self.shared.has_pending() {
            return;
        }
        self.drain_overflow();
        if self.active {
            let deadline = Instant::now() + Duration::from_millis(50);
            while self.shared.has_pending() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
                self.drain_overflow();
            }
        }
        if self.shared.has_pending() {
            self.flush();
        }
    }

    /// A value for the processor from the main thread (host UI, preset selection).
    unsafe fn send(&mut self, id: u32, value: f64) {
        if !self.shared.send_to_processor(id, value) {
            self.flush();
            if !self.shared.send_to_processor(id, value) {
                self.shared.overflow.lock().unwrap().push((id, value));
            }
        }
    }

    unsafe fn handle_restart(&mut self, flags: u32) {
        if flags & (RESTART_TITLES | RESTART_RELOAD | RESTART_IO) != 0 {
            self.scan_parameters();
        }
        if flags & (RESTART_MIDI | RESTART_RELOAD) != 0 {
            self.refresh_midi_mapping();
        }
        if flags & (RESTART_VALUES | RESTART_TITLES | RESTART_RELOAD) != 0 {
            self.shared.handle.notify(Notification::ParamsRescan);
        }
        if flags & (RESTART_IO | RESTART_RELOAD) != 0 {
            self.io_changed = true;
        }
        if flags & (RESTART_LATENCY | RESTART_IO | RESTART_RELOAD) == 0 {
            return;
        }
        if self.active {
            // These take a setActive(false) / setActive(true) cycle, which the host
            // performs when it suits it: deactivate() + activate().
            if !self.restart_requested {
                self.restart_requested = true;
                self.shared.handle.notify(Notification::RestartRequested);
            }
        } else {
            if self.io_changed {
                self.negotiate_buses();
            }
            self.shared.report_latency(&self.processor);
        }
    }

    fn bus_infos(buses: &[PlugBus], host: &[u32], input: bool) -> Vec<BusInfo> {
        host.iter()
            .enumerate()
            .map(|(index, &channels)| match buses.iter().find(|bus| bus.host == Some(index)) {
                Some(bus) => {
                    BusInfo { name: bus.name.clone(), channels: channels.min(bus.channels), auxiliary: bus.auxiliary }
                }
                None => BusInfo {
                    name: if input { "Unused input".into() } else { "Unused output".into() },
                    channels: 0,
                    auxiliary: index > 0,
                },
            })
            .collect()
    }

    unsafe fn build_parameters(&mut self) -> Vec<ParameterInfo> {
        let Some(controller) = self.controller.clone() else { return Vec::new() };
        let paths = self.unit_paths();
        let mut list = Vec::new();
        for index in 0..controller.getParameterCount().clamp(0, 1 << 20) {
            let mut info: vst::ParameterInfo = std::mem::zeroed();
            if controller.getParameterInfo(index, &mut info) != kResultOk {
                continue;
            }
            let steps = info.stepCount.max(0) as u32;
            let mut flags = 0;
            for (theirs, ours) in [
                (FLAG_AUTOMATE, PARAM_AUTOMATABLE),
                (FLAG_READ_ONLY, PARAM_READ_ONLY),
                (FLAG_HIDDEN, PARAM_HIDDEN),
                (FLAG_BYPASS, PARAM_BYPASS),
            ] {
                if info.flags & theirs != 0 {
                    flags |= ours;
                }
            }
            if steps > 0 {
                flags |= PARAM_STEPPED;
            }
            if steps == 1 {
                flags |= PARAM_BOOLEAN;
            }
            // Lists: fetch the labels, as hosts show them in menus.
            let value_strings = if info.flags & FLAG_LIST != 0 && (1..=256).contains(&steps) {
                (0..=steps).map(|step| self.text_of(info.id, step as f64 / steps as f64).unwrap_or_default()).collect()
            } else {
                Vec::new()
            };
            list.push(ParameterInfo {
                id: info.id as u64,
                name: wide_to_string(&info.title),
                short_name: wide_to_string(&info.shortTitle),
                identifier: String::new(),
                group: paths.get(&info.unitId).cloned().unwrap_or_default(),
                unit_label: wide_to_string(&info.units),
                flags,
                // VST3 parameters are normalized: 0..1 everywhere outside the plug-in.
                min: 0.0,
                max: 1.0,
                default: info.defaultNormalizedValue,
                step_count: steps,
                value_strings,
            });
        }
        list
    }

    unsafe fn text_of(&self, id: u32, value: f64) -> Option<String> {
        let mut text: vst::String128 = [0; 128];
        let result = self.controller()?.getParamStringByValue(id, value, &mut text);
        (result == kResultOk).then(|| wide_to_string(&text))
    }

    fn known(&self, id: u64) -> Option<u32> {
        u32::try_from(id).ok().filter(|id| self.parameter_ids.contains(id))
    }
}

impl Plugin for Vst3Plugin {
    fn info(&mut self) -> InstanceInfo {
        let categories: Vec<&str> = self.class.sub_categories.split('|').collect();
        let mut flags = 0;
        if categories.contains(&"Instrument") || (self.in_buses.is_empty() && self.event_inputs > 0) {
            flags |= INFO_INSTRUMENT;
        }
        if self.event_inputs > 0 {
            flags |= INFO_WANTS_MIDI;
        }
        if self.event_outputs > 0 {
            flags |= INFO_PRODUCES_MIDI;
        }
        if self.bypass_parameter.is_some() {
            flags |= INFO_HAS_BYPASS;
        }
        if self.has_editor {
            flags |= INFO_HAS_EDITOR;
        }
        // SAFETY: main thread; plain getters.
        let (latency, tail) = unsafe { (self.processor.getLatencySamples(), self.processor.getTailSamples()) };
        InstanceInfo {
            name: self.class.name.clone(),
            vendor: self.class.vendor.clone(),
            version: self.class.version.clone(),
            flags,
            latency,
            tail: if tail == vst::kInfiniteTail { u64::MAX } else { tail as u64 },
            inputs: Self::bus_infos(&self.in_buses, &self.host_in, true),
            outputs: Self::bus_infos(&self.out_buses, &self.host_out, false),
            parameter_count: self.parameter_ids.len() as u32,
        }
    }

    fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<Box<dyn Processor>, PluginError> {
        // SAFETY: main thread, component inactive, the real-time thread excluded.
        unsafe {
            if self.io_changed {
                self.negotiate_buses();
            }
            self.start(sample_rate, max_frames)?;
            self.sample_rate = sample_rate;
            self.max_frames = max_frames;
            self.active = true;
            self.restart_requested = false;
            // A latency announced with kLatencyChanged is valid from here on.
            self.shared.report_latency(&self.processor);
        }
        let maps = |buses: &[PlugBus]| -> Vec<BusMap> {
            buses.iter().map(|bus| BusMap { host: bus.host, channels: bus.channels as usize }).collect()
        };
        Ok(Box::new(Vst3Processor::new(ProcessorConfig {
            component: self.component.clone(),
            processor: self.processor.clone(),
            shared: self.shared.clone(),
            sample_rate,
            max_frames: max_frames as usize,
            inputs: maps(&self.in_buses),
            outputs: maps(&self.out_buses),
            event_inputs: self.event_inputs as usize,
            parameter_count: self.parameter_ids.len(),
            event_capacity: self.event_capacity,
            out_event_capacity: self.out_event_capacity,
        })))
    }

    fn deactivate(&mut self, processor: Box<dyn Processor>) {
        // The processor makes no plug-in calls when dropped; notes it held end with the
        // deactivation.
        drop(processor);
        if self.active {
            // SAFETY: main thread, the real-time thread excluded.
            unsafe { self.stop() };
            self.active = false;
        }
    }

    fn set_offline(&mut self, offline: bool) {
        let mode = if offline { K_OFFLINE } else { K_REALTIME };
        if self.shared.process_mode.swap(mode, Ordering::AcqRel) == mode || !self.active {
            return;
        }
        // The process mode is part of the setup, which needs an inactive component.
        // SAFETY: main thread, the real-time thread excluded.
        unsafe {
            self.stop();
            if let Err(error) = self.start(self.sample_rate, self.max_frames) {
                eprintln!("[vst3] {error}");
            }
            self.shared.report_latency(&self.processor);
        }
    }

    fn set_bypass(&mut self, bypass: bool) -> Result<(), PluginError> {
        let id = self.bypass_parameter.ok_or(PluginError::Unsupported)?;
        self.set_parameter(id as u64, if bypass { 1.0 } else { 0.0 })
    }

    fn parameters(&mut self) -> Vec<ParameterInfo> {
        if self.parameters.is_none() {
            // SAFETY: main thread.
            self.parameters = Some(unsafe { self.build_parameters() });
        }
        self.parameters.clone().unwrap_or_default()
    }

    fn parameter_value(&mut self, id: u64) -> Option<f64> {
        let id = self.known(id)?;
        // SAFETY: main thread.
        Some(unsafe { self.controller()?.getParamNormalized(id) })
    }

    fn set_parameter(&mut self, id: u64, value: f64) -> Result<(), PluginError> {
        let Some(id) = self.known(id) else {
            return Err(if self.controller.is_none() {
                PluginError::Unsupported
            } else {
                failed(format!("\"{}\" has no parameter {id}", self.class.name))
            });
        };
        if !value.is_finite() {
            return Err(failed("parameter values must be finite"));
        }
        let value = value.clamp(0.0, 1.0);
        // SAFETY: main thread. The controller shows the value; the processor gets it with
        // the next block (or the next flush).
        unsafe {
            if let Some(controller) = self.controller() {
                controller.setParamNormalized(id, value);
            }
            self.send(id, value);
        }
        Ok(())
    }

    fn format_parameter(&mut self, id: u64, value: f64) -> Option<String> {
        let id = self.known(id)?;
        // SAFETY: main thread.
        unsafe { self.text_of(id, value) }
    }

    fn parse_parameter(&mut self, id: u64, text: &str) -> Option<f64> {
        let id = self.known(id)?;
        let mut wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value = 0.0;
        // SAFETY: main thread; the string is NUL-terminated.
        let result = unsafe { self.controller()?.getParamValueByString(id, wide.as_mut_ptr(), &mut value) };
        (result == kResultOk && value.is_finite()).then_some(value)
    }

    fn state(&mut self) -> Result<Vec<u8>, PluginError> {
        // SAFETY: main thread; VST3 plug-ins serve getState concurrently with process.
        unsafe {
            self.settle_pending();
            let component = MemoryStream::writer();
            if self.component.getState(stream_ptr(&component)) != kResultOk {
                return Err(failed(format!("\"{}\" did not provide its state", self.class.name)));
            }
            let mut controller_state = None;
            if let Some(controller) = self.controller() {
                let stream = MemoryStream::writer();
                let result = controller.getState(stream_ptr(&stream));
                if result == kResultOk {
                    controller_state = Some(stream.take());
                } else if stream.len() != 0 {
                    // Bytes from a failed save cannot be restored: no snapshot at all is
                    // better than one that silently lacks half.
                    return Err(failed(format!(
                        "the edit controller of \"{}\" did not provide a complete state",
                        self.class.name
                    )));
                }
            }
            Ok(state::encode(cid_words(&self.class.cid), &component.take(), controller_state.as_deref()))
        }
    }

    fn set_state(&mut self, data: &[u8]) -> Result<(), PluginError> {
        let parts = state::decode(cid_words(&self.class.cid), data).map_err(failed)?;
        // SAFETY: main thread; VST3 plug-ins serve setState concurrently with process.
        unsafe {
            // Changes queued before the state are older than the state.
            self.shared.epoch.fetch_add(1, Ordering::AcqRel);
            self.shared.overflow.lock().unwrap().clear();
            let stream = MemoryStream::reader(parts.component);
            if self.component.setState(stream_ptr(&stream)) != kResultOk {
                return Err(failed(format!(
                    "\"{}\" rejected the state (it may come from another version of the plug-in)",
                    self.class.name
                )));
            }
            if let Some(controller) = self.controller.clone() {
                stream.rewind();
                self.sync_controller_state(&controller, &stream);
                if let Some(bytes) = parts.controller {
                    let stream = MemoryStream::reader(bytes);
                    let result = controller.setState(stream_ptr(&stream));
                    if result != kResultOk && result != kNotImplemented {
                        return Err(failed(format!(
                            "the edit controller of \"{}\" rejected its state",
                            self.class.name
                        )));
                    }
                }
            }
            self.refresh_midi_mapping();
            if !self.active {
                self.shared.report_latency(&self.processor);
            }
        }
        Ok(())
    }

    fn presets(&mut self) -> Vec<PresetInfo> {
        let (Some(programs), Some(units)) = (self.programs, self.unit_info.clone()) else { return Vec::new() };
        (0..programs.count.clamp(0, 16384))
            .map(|index| {
                let mut name: vst::String128 = [0; 128];
                // SAFETY: main thread.
                let named = unsafe { units.getProgramName(programs.list, index, &mut name) } == kResultOk;
                PresetInfo {
                    id: index as u32,
                    name: if named { wide_to_string(&name) } else { format!("Program {}", index + 1) },
                }
            })
            .collect()
    }

    fn load_preset(&mut self, id: u32) -> Result<(), PluginError> {
        let programs = self.programs.ok_or(PluginError::Unsupported)?;
        if id as i64 >= programs.count as i64 || programs.steps <= 0 && id != 0 {
            return Err(failed(format!("\"{}\" has no program {id}", self.class.name)));
        }
        let value = if programs.steps > 0 { id as f64 / programs.steps as f64 } else { 0.0 };
        self.set_parameter(programs.parameter as u64, value)
    }

    fn open_editor(&mut self, title: &str) -> Result<EditorInfo, PluginError> {
        self.close_editor();
        let controller = self.controller.clone().ok_or(PluginError::Unsupported)?;
        // SAFETY: main thread.
        let (editor, info) = unsafe { Editor::open(&controller, &self.shared, title)? };
        self.editor = Some(editor);
        Ok(info)
    }

    fn close_editor(&mut self) {
        if let Some(editor) = self.editor.take() {
            editor.close();
        }
    }

    fn focus_editor(&mut self) {
        if let Some(editor) = &mut self.editor {
            editor.focus();
        }
    }

    fn set_editor_title(&mut self, title: &str) {
        if let Some(editor) = &mut self.editor {
            editor.set_title(title);
        }
    }

    fn idle(&mut self) {
        // SAFETY: main thread, outside of any call into the plug-in.
        unsafe {
            let flags = self.shared.restart_flags.swap(0, Ordering::AcqRel);
            if flags != 0 {
                self.handle_restart(flags);
            }
            self.drain_overflow();
            // Values the processor saw or produced: the controller learns all of them,
            // the host application those the plug-in made.
            if self.shared.sync_requested.swap(false, Ordering::AcqRel) || !self.shared.to_main.is_empty() {
                let mut latest: Vec<(u32, f64, bool)> = Vec::new();
                while let Some((id, tag, value)) = self.shared.to_main.pop() {
                    match latest.iter_mut().find(|entry| entry.0 == id) {
                        Some(entry) => *entry = (id, value, entry.2 || tag == SYNC_NOTIFY),
                        None => latest.push((id, value, tag == SYNC_NOTIFY)),
                    }
                }
                for (id, value, notify) in latest {
                    if let Some(controller) = self.controller() {
                        controller.setParamNormalized(id, value);
                    }
                    if notify {
                        self.shared.handle.notify(Notification::ParamValue { id: id as u64, value });
                    }
                }
            }
        }
        if self.editor.as_ref().is_some_and(|editor| editor.close_requested()) {
            self.close_editor();
        }
        if let Some(editor) = &mut self.editor {
            editor.idle();
        }
    }
}

impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        self.close_editor();
        // SAFETY: main thread; the runtime took the processor back before dropping the
        // plug-in. The order is the SDK's own host's: stop, disconnect, drop the handler,
        // terminate the component before a separate controller, and only then release the
        // interfaces; the module goes last (field order).
        unsafe {
            if self.active {
                self.stop();
                self.active = false;
            }
            if let Some((component_end, controller_end)) = self.connection.take() {
                component_end.disconnect(controller_end.as_ptr());
                controller_end.disconnect(component_end.as_ptr());
            }
            if let Some(controller) = &self.controller {
                controller.setComponentHandler(std::ptr::null_mut());
            }
            if self.component_initialized {
                self.component.terminate();
            }
            if self.controller_initialized {
                if let Some(controller) = &self.controller {
                    controller.terminate();
                }
            }
            self.unit_info = None;
            self.midi_mapping = None;
            self.controller = None;
        }
    }
}
