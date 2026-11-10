//! Real devices through the `midir` crate: ALSA sequencer on Linux, CoreMIDI on macOS, WinMM on
//! Windows.
//!
//! What `midir` does not offer, and what stands in for it:
//!
//! - **Timestamped output.** `MidiOutputConnection::send` sends now (its CoreMIDI backend passes
//!   "now" as the packet time). Scheduling is therefore done in this crate: the sender thread
//!   ([`crate::output`]) holds each message until its time.
//! - **Hot-plug notifications.** There are none; [`crate::MidiHost`] lists the ports twice a second
//!   and compares.
//! - **Persistent device ids.** CoreMIDI's unique id is used where `midir` exposes it (macOS).
//!   Elsewhere a port's id is derived from its name ([`crate::ports::id_from_name`]), so a device
//!   keeps its id across launches and re-plugging as long as it keeps its name.

use midir::{ConnectErrorKind, Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::ports::{ids_by_name, Backend, InputCallback, InputPort, OpenError, OutputPort, PortInfo};

/// The name this application's own client and ports carry in the system. Ports with this name are
/// not listed: on ALSA every open connection is itself a port other applications (and this one)
/// can see.
const CLIENT_NAME: &str = "SuperTracker";

/// The system's MIDI devices.
pub struct MidirBackend {
    input: MidiInput,
    output: MidiOutput,
}

impl MidirBackend {
    /// Connects to the system's MIDI service. Fails where there is none (a Linux machine without
    /// the ALSA sequencer, for instance).
    pub fn new() -> Result<Self, String> {
        let unavailable = |error: midir::InitError| format!("The system's MIDI service is not available ({error}).");
        let mut input = MidiInput::new(CLIENT_NAME).map_err(unavailable)?;
        input.ignore(Ignore::SysexAndActiveSense);
        let output = MidiOutput::new(CLIENT_NAME).map_err(unavailable)?;
        Ok(MidirBackend { input, output })
    }
}

/// The name to show and to derive the id from. ALSA port names end in the client and port numbers
/// ("Minilogue xd:Minilogue xd MIDI 1 28:0"), which change when the device is plugged in again.
fn display_name(raw: &str) -> String {
    if cfg!(target_os = "linux") {
        if let Some((name, address)) = raw.rsplit_once(' ') {
            let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
            if address.split_once(':').is_some_and(|(client, port)| numeric(client) && numeric(port)) {
                return name.to_string();
            }
        }
    }
    raw.to_string()
}

fn is_own(name: &str) -> bool {
    name.starts_with(CLIENT_NAME)
}

/// Ids for a listing of `(system id, name)`: the system's own persistent id where it is one
/// (CoreMIDI), the name's otherwise.
fn assign_ids(ports: &[(String, String)]) -> Vec<i32> {
    let by_name = ids_by_name(ports.iter().map(|(_, name)| name.as_str()));
    ports
        .iter()
        .zip(by_name)
        .map(|((system, _), named)| if cfg!(target_os = "macos") { system.parse::<i32>().ok().filter(|id| *id != 0).unwrap_or(named) } else { named })
        .collect()
}

fn listing(ports: Vec<(String, String)>) -> Vec<PortInfo> {
    let ids = assign_ids(&ports);
    ports.into_iter().zip(ids).map(|((_, name), id)| PortInfo { id, name, manufacturer: String::new(), offline: false }).collect()
}

struct OpenInput(#[allow(dead_code)] MidiInputConnection<()>);
impl InputPort for OpenInput {}

struct OpenOutput(MidiOutputConnection);
impl OutputPort for OpenOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0.send(bytes).map_err(|error| error.to_string())
    }
}

impl MidirBackend {
    fn input_ports(&self) -> Vec<(midir::MidiInputPort, (String, String))> {
        self.input
            .ports()
            .into_iter()
            .filter_map(|port| {
                let name = self.input.port_name(&port).ok()?;
                (!is_own(&name)).then(|| {
                    let entry = (port.id(), display_name(&name));
                    (port, entry)
                })
            })
            .collect()
    }

    fn output_ports(&self) -> Vec<(midir::MidiOutputPort, (String, String))> {
        self.output
            .ports()
            .into_iter()
            .filter_map(|port| {
                let name = self.output.port_name(&port).ok()?;
                (!is_own(&name)).then(|| {
                    let entry = (port.id(), display_name(&name));
                    (port, entry)
                })
            })
            .collect()
    }
}

impl Backend for MidirBackend {
    fn inputs(&mut self) -> Result<Vec<PortInfo>, String> {
        Ok(listing(self.input_ports().into_iter().map(|(_, entry)| entry).collect()))
    }

    fn outputs(&mut self) -> Result<Vec<PortInfo>, String> {
        Ok(listing(self.output_ports().into_iter().map(|(_, entry)| entry).collect()))
    }

    fn open_input(&mut self, id: i32, mut receive: InputCallback) -> Result<Box<dyn InputPort>, OpenError> {
        let ports = self.input_ports();
        let ids = assign_ids(&ports.iter().map(|(_, entry)| entry.clone()).collect::<Vec<_>>());
        let Some(index) = ids.iter().position(|candidate| *candidate == id) else {
            return Err(OpenError::Gone);
        };
        // A connection consumes its client, so each one gets a client of its own.
        let mut client = MidiInput::new(CLIENT_NAME).map_err(|error| OpenError::Failed(error.to_string()))?;
        client.ignore(Ignore::SysexAndActiveSense);
        match client.connect(&ports[index].0, "SuperTracker in", move |_, bytes, _| receive(bytes), ()) {
            Ok(connection) => Ok(Box::new(OpenInput(connection))),
            Err(error) if matches!(error.kind(), ConnectErrorKind::InvalidPort) => Err(OpenError::Gone),
            Err(error) => Err(OpenError::Failed(error.to_string())),
        }
    }

    fn open_output(&mut self, id: i32) -> Result<Box<dyn OutputPort>, OpenError> {
        let ports = self.output_ports();
        let ids = assign_ids(&ports.iter().map(|(_, entry)| entry.clone()).collect::<Vec<_>>());
        let Some(index) = ids.iter().position(|candidate| *candidate == id) else {
            return Err(OpenError::Gone);
        };
        let client = MidiOutput::new(CLIENT_NAME).map_err(|error| OpenError::Failed(error.to_string()))?;
        match client.connect(&ports[index].0, "SuperTracker out") {
            Ok(connection) => Ok(Box::new(OpenOutput(connection))),
            Err(error) if matches!(error.kind(), ConnectErrorKind::InvalidPort) => Err(OpenError::Gone),
            Err(error) => Err(OpenError::Failed(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alsa_addresses_are_not_part_of_a_name() {
        if cfg!(target_os = "linux") {
            assert_eq!(display_name("Minilogue xd:Minilogue xd MIDI 1 28:0"), "Minilogue xd:Minilogue xd MIDI 1");
            assert_eq!(display_name("Midi Through:Midi Through Port-0 14:0"), "Midi Through:Midi Through Port-0");
        }
        assert_eq!(display_name("Synth 2"), "Synth 2");
        assert_eq!(display_name("IAC Driver Bus 1"), "IAC Driver Bus 1");
        assert!(is_own("SuperTracker:SuperTracker in") && !is_own("Minilogue xd"));
    }

    #[test]
    fn ids_come_from_the_system_only_where_it_has_persistent_ones() {
        let ports = [("-1234".to_string(), "A".to_string()), ("28:0".to_string(), "B".to_string()), ("0".to_string(), "A".to_string())];
        let ids = assign_ids(&ports);
        if cfg!(target_os = "macos") {
            assert_eq!(ids[0], -1234);
        } else {
            assert_eq!(ids[0], crate::ports::id_from_name("A", 0));
        }
        assert_eq!(ids[1], crate::ports::id_from_name("B", 0));
        assert_eq!(ids[2], crate::ports::id_from_name("A", 1));
    }

    /// With a MIDI service present this lists its ports; without one (this crate's test machines
    /// have no ALSA sequencer) the failure is a message, not a panic.
    #[test]
    fn the_system_backend_starts_or_explains() {
        match MidirBackend::new() {
            Ok(mut backend) => {
                let inputs = backend.inputs().unwrap();
                let outputs = backend.outputs().unwrap();
                assert!(inputs.iter().chain(&outputs).all(|port| port.id != 0 && !is_own(&port.name)));
                assert!(matches!(backend.open_output(0), Err(OpenError::Gone)));
            }
            Err(message) => assert!(message.starts_with("The system's MIDI service is not available"), "{message}"),
        }
    }
}
