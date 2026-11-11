//! The control protocol. Requests go host -> container, each answered by exactly one
//! [`Response`] with the same frame id; [`ContainerNotice`]s are unsolicited container ->
//! host frames. The handshake is one [`Hello`] (container) answered by one [`Welcome`].

use crate::wire::Bytes;
use crate::{wire_enum, wire_struct};

wire_struct! {
    /// First frame on a new connection, sent by the container.
    pub struct Hello {
        pub magic: u32,
        pub protocol_version: u32,
        pub pid: u32,
        /// The token the supervisor passed on the command line.
        pub token: String,
        /// Mask of [`crate::wake::WakeMode::bit`]s the container can use.
        pub wake_support: u32,
        /// Free-form build description, for logs.
        pub runtime: String,
        /// Plug-in formats with a registered backend.
        pub formats: Vec<String>,
    }
}

wire_struct! {
    /// The supervisor's answer to [`Hello`]. `accepted == false` ends the connection.
    pub struct Welcome {
        pub accepted: bool,
        pub protocol_version: u32,
        pub reason: String,
    }
}

// ---- instance description ---------------------------------------------------------------

/// The plug-in is an instrument (no audio needed to produce sound).
pub const INFO_INSTRUMENT: u32 = 1;
pub const INFO_HAS_EDITOR: u32 = 2;
pub const INFO_WANTS_MIDI: u32 = 4;
pub const INFO_PRODUCES_MIDI: u32 = 8;
/// The plug-in implements bypass itself (keeps its latency while bypassed).
pub const INFO_HAS_BYPASS: u32 = 16;
/// The sustain pedal (MIDI controller 64) can reach the plug-in: as MIDI, or as a parameter
/// the plug-in maps the controller to. A host holds the notes of an instrument without this
/// itself while the pedal is down; one with it gets the pedal and sustains by itself.
pub const INFO_SUSTAIN_PEDAL: u32 = 32;

wire_struct! {
    pub struct BusInfo {
        pub name: String,
        /// Channels the plug-in uses of the channels the host allotted to this bus.
        pub channels: u32,
        /// Not the main bus (sidechain / auxiliary output).
        pub auxiliary: bool,
    }
}

wire_struct! {
    pub struct InstanceInfo {
        pub name: String,
        pub vendor: String,
        pub version: String,
        pub flags: u32,
        /// Processing latency in frames. The transport adds none.
        pub latency: u32,
        /// Tail in frames; `u64::MAX` means infinite.
        pub tail: u64,
        pub inputs: Vec<BusInfo>,
        pub outputs: Vec<BusInfo>,
        pub parameter_count: u32,
    }
}

pub const PARAM_AUTOMATABLE: u32 = 1;
pub const PARAM_READ_ONLY: u32 = 2;
pub const PARAM_HIDDEN: u32 = 4;
/// Values are integers (`step_count` intervals between min and max).
pub const PARAM_STEPPED: u32 = 8;
pub const PARAM_BOOLEAN: u32 = 16;
/// This parameter is the plug-in's own bypass switch.
pub const PARAM_BYPASS: u32 = 32;

wire_struct! {
    pub struct ParameterInfo {
        pub id: u64,
        pub name: String,
        pub short_name: String,
        /// Stable textual identifier where the format has one (AU key path), else empty.
        pub identifier: String,
        /// Group / unit path, e.g. "Filter/Envelope".
        pub group: String,
        pub unit_label: String,
        pub flags: u32,
        pub min: f64,
        pub max: f64,
        pub default: f64,
        /// Number of intervals for stepped parameters (a switch has 1); 0 if continuous.
        pub step_count: u32,
        /// Labels of a stepped parameter's values, when the plug-in enumerates them.
        pub value_strings: Vec<String>,
    }
}

wire_struct! {
    pub struct ParameterValue {
        pub id: u64,
        pub value: f64,
    }
}

wire_struct! {
    pub struct PresetInfo {
        pub id: u32,
        pub name: String,
    }
}

wire_struct! {
    pub struct EditorInfo {
        pub width: u32,
        pub height: u32,
        pub resizable: bool,
        /// Platform window number of the editor window (macOS: `NSWindow.windowNumber`),
        /// 0 if unknown. Lets the host order its own windows relative to it.
        pub window_id: u64,
    }
}

// ---- plug-in scanning -------------------------------------------------------------------

/// The plug-in is an instrument (makes sound from notes).
pub const SCAN_INSTRUMENT: u32 = 1;
/// The plug-in is an audio effect.
pub const SCAN_EFFECT: u32 = 2;
/// The plug-in works on notes / MIDI rather than audio (CLAP note effects, AU MIDI processors).
pub const SCAN_NOTE_EFFECT: u32 = 4;
/// The plug-in takes note / MIDI input, as far as the scan could tell without instantiating.
pub const SCAN_WANTS_MIDI: u32 = 8;
/// The plug-in emits notes / MIDI, as far as the scan could tell.
pub const SCAN_PRODUCES_MIDI: u32 = 16;
/// The plug-in has audio inputs, as far as the scan could tell.
pub const SCAN_AUDIO_INPUT: u32 = 32;
pub const SCAN_AUDIO_OUTPUT: u32 = 64;
/// `SCAN_HAS_EDITOR` is meaningful (most formats only know after instantiating).
pub const SCAN_EDITOR_KNOWN: u32 = 128;
pub const SCAN_HAS_EDITOR: u32 = 256;
/// The plug-in declares a sidechain input in its categories.
pub const SCAN_SIDECHAIN: u32 = 512;

wire_struct! {
    /// One installed plug-in, as found by scanning a plug-in file (or the system registry)
    /// without instantiating it. `format` + `id` + `path` is what an instance
    /// configuration needs (`InstanceConfig::format`, `plugin_id`, `options`).
    pub struct PluginDescription {
        /// "vst3", "clap", "au".
        pub format: String,
        /// VST3: the class id as 32 hex digits. CLAP: the plug-in id. AU:
        /// "type:subtype:manufacturer" four-character codes.
        pub id: String,
        pub name: String,
        pub vendor: String,
        pub version: String,
        /// The category as the format states it (VST3 subcategories "Fx|Dynamics", CLAP
        /// features joined with "|", AU type name).
        pub category: String,
        /// `category` split into its parts.
        pub subcategories: Vec<String>,
        /// `SCAN_*` bits.
        pub flags: u32,
        /// The plug-in file or bundle; empty for registry formats (AU).
        pub path: String,
        /// SDK / format version the plug-in was built against ("VST 3.7.8", "1.2.2"), if stated.
        pub sdk_version: String,
        pub url: String,
    }
}

wire_struct! {
    /// Everything the container needs to build an instance and map its region.
    pub struct CreateParams {
        pub format: String,
        pub plugin_id: String,
        pub name: String,
        /// Backend-specific options (bundle path, class id, ...).
        pub options: String,
        pub sample_rate: f64,
        pub max_frames: u32,
        pub in_buses: Vec<u32>,
        pub out_buses: Vec<u32>,
        pub event_capacity: u32,
        pub out_event_capacity: u32,
        pub queue_capacity: u32,
        /// [`crate::wake::WakeMode`] as u32. Pipe mode attaches two more descriptors.
        pub wake_mode: u32,
    }
}

wire_enum! {
    /// Host -> container. Attached descriptors: `CreateInstance` and `Reconfigure` carry
    /// the region, then (pipe wake mode) the read end of the request pipe and the write end
    /// of the response pipe.
    pub enum Request {
        /// Answered by the I/O thread: proves the process is alive.
        Ping {} = 1,
        /// Answered by the main thread: proves the main loop is responsive.
        PingMain {} = 2,
        /// Destroy all instances and exit.
        Shutdown {} = 3,
        CreateInstance { instance: u64, params: CreateParams } = 10,
        DestroyInstance { instance: u64 } = 11,
        Activate { instance: u64 } = 12,
        Deactivate { instance: u64 } = 13,
        Reset { instance: u64 } = 14,
        SetOffline { instance: u64, offline: bool } = 15,
        SetBypass { instance: u64, bypass: bool } = 16,
        /// New sample rate and/or block size, with a new region. Leaves the instance
        /// deactivated.
        Reconfigure { instance: u64, sample_rate: f64, max_frames: u32 } = 17,
        GetInfo { instance: u64 } = 18,
        GetParameters { instance: u64 } = 20,
        GetParameterValues { instance: u64 } = 21,
        GetParameter { instance: u64, id: u64 } = 22,
        SetParameter { instance: u64, id: u64, value: f64 } = 23,
        FormatParameter { instance: u64, id: u64, values: Vec<f64> } = 24,
        ParseParameter { instance: u64, id: u64, text: String } = 25,
        GetState { instance: u64 } = 30,
        SetState { instance: u64, data: Bytes } = 31,
        GetPresets { instance: u64 } = 32,
        LoadPreset { instance: u64, id: u32 } = 33,
        OpenEditor { instance: u64, title: String } = 40,
        CloseEditor { instance: u64 } = 41,
        FocusEditor { instance: u64 } = 42,
        SetEditorTitle { instance: u64, title: String } = 43,
        /// Lists the plug-ins in one plug-in file or bundle without instantiating them
        /// (`path` empty: the format's system registry, for Audio Units). Loads foreign
        /// code: the host sends it to a sacrificial container only.
        Scan { format: String, path: String } = 50,
    }
}

pub const ERR_UNKNOWN_FORMAT: u32 = 1;
pub const ERR_UNKNOWN_INSTANCE: u32 = 2;
/// The plug-in (backend) reported the failure.
pub const ERR_PLUGIN: u32 = 3;
pub const ERR_UNSUPPORTED: u32 = 4;
pub const ERR_PROTOCOL: u32 = 5;
pub const ERR_INVALID_STATE: u32 = 6;

wire_enum! {
    /// Container -> host, one per request.
    pub enum Response {
        Ok {} = 1,
        Error { code: u32, message: String } = 2,
        Info { info: InstanceInfo } = 3,
        Parameters { list: Vec<ParameterInfo> } = 4,
        Values { values: Vec<ParameterValue> } = 5,
        Value { value: Option<f64> } = 6,
        Strings { strings: Vec<String> } = 7,
        State { data: Bytes } = 8,
        Presets { list: Vec<PresetInfo> } = 9,
        Editor { info: EditorInfo } = 10,
        Scan { plugins: Vec<PluginDescription> } = 11,
    }
}

wire_enum! {
    /// Container -> host, unsolicited.
    pub enum ContainerNotice {
        Log { level: u32, message: String } = 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{Wire, WireError};

    #[test]
    fn requests_round_trip() {
        let requests = vec![
            Request::Ping {},
            Request::CreateInstance {
                instance: 42,
                params: CreateParams {
                    format: "test".into(),
                    plugin_id: "gain".into(),
                    name: "Gain ü".into(),
                    options: "x=1".into(),
                    sample_rate: 48000.0,
                    max_frames: 512,
                    in_buses: vec![2, 2],
                    out_buses: vec![2],
                    event_capacity: 256,
                    out_event_capacity: 64,
                    queue_capacity: 256,
                    wake_mode: 1,
                },
            },
            Request::SetState { instance: 1, data: Bytes(vec![1, 2, 3, 255]) },
            Request::FormatParameter { instance: 3, id: u64::MAX, values: vec![0.0, -1.5, f64::MAX] },
            Request::OpenEditor { instance: 9, title: "Track 1 – Synth".into() },
            Request::Scan { format: "vst3".into(), path: "/usr/lib/vst3/X.vst3".into() },
        ];
        for request in requests {
            let bytes = request.encode();
            assert_eq!(Request::decode(&bytes).unwrap(), request);
            // Every strict prefix fails cleanly.
            for cut in 0..bytes.len() {
                assert!(Request::decode(&bytes[..cut]).is_err(), "{} cut at {cut}", request.name());
            }
        }
    }

    #[test]
    fn responses_round_trip_and_reject_garbage() {
        let response = Response::Parameters {
            list: vec![ParameterInfo {
                id: 7,
                name: "Cutoff".into(),
                unit_label: "Hz".into(),
                flags: PARAM_AUTOMATABLE,
                min: 20.0,
                max: 20000.0,
                default: 1000.0,
                value_strings: vec!["a".into(), "b".into()],
                ..ParameterInfo::default()
            }],
        };
        assert_eq!(Response::decode(&response.encode()).unwrap(), response);
        let scan = Response::Scan {
            plugins: vec![PluginDescription {
                format: "clap".into(),
                id: "com.vendor.synth".into(),
                name: "Synth".into(),
                subcategories: vec!["instrument".into(), "stereo".into()],
                flags: SCAN_INSTRUMENT | SCAN_WANTS_MIDI,
                ..PluginDescription::default()
            }],
        };
        assert_eq!(Response::decode(&scan.encode()).unwrap(), scan);
        assert_eq!(Response::decode(&[0xFF, 0xFF]).err(), Some(WireError::UnknownOpcode(0xFFFF)));
        // A forged vector length must not allocate or panic.
        let mut forged = (4u16).to_le_bytes().to_vec();
        forged.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Response::decode(&forged).err(), Some(WireError::Truncated));
        let mut trailing = Response::Ok {}.encode();
        trailing.push(0);
        assert_eq!(Response::decode(&trailing).err(), Some(WireError::Trailing(1)));
    }
}
