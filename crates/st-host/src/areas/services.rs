//! The service crates of the host, installed together: the sound library ([`super::library`]),
//! MIDI ([`super::midi`]), the Maschine MK3 ([`super::mk3`]) and AI control ([`super::agent`]).
//!
//! [`crate::install`] calls [`install`] with [`ServiceOptions`] (part of
//! [`crate::HostOptions`]) and keeps the result as [`crate::Host::services`]. What the
//! executable has to do with it:
//!
//! - call `host.services.start_agent()` once its transport can attach the interface;
//! - if it shows the interface in a window, give AI control what takes the window's picture
//!   (`host.services.agent`, [`Agent::set_screenshot_provider`]);
//! - call `host.services.shutdown()` when the host quits, and on termination signals: it
//!   closes the control socket, leaves the controller dark with the vendor's agents restored,
//!   silences external MIDI instruments and closes the devices, and writes the library's index.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use st_audio::AudioEngine;

use super::agent::{Agent, AgentOptions};
use super::library::{LibraryArea, LibraryOptions};
use super::midi::{Midi, MidiOptions};
use super::mk3::{Mk3Devices, Mk3Options};
use crate::app::Lifecycle;
use crate::bridge::{Calls, EventSink};
use crate::commands::Commands;
use crate::session::Session;
use crate::{Result, Settings};

/// Which services [`crate::install`] sets up, and how. `None` leaves a service out: its
/// commands then answer "not implemented" (tests of other areas).
#[derive(Clone)]
pub struct ServiceOptions {
    pub library: Option<LibraryOptions>,
    pub midi: Option<MidiOptions>,
    pub mk3: Option<Mk3Options>,
    pub agent: Option<AgentOptions>,
}

impl ServiceOptions {
    /// Every service, for the host whose support folder is `support`: the library in that
    /// folder, the system's MIDI devices (`SUPERTRACKER_MIDI=off` turns them off), the real
    /// controller (which stays untouched unless support for it is on in the settings), and the
    /// control socket of that folder.
    pub fn for_support(support: &Path) -> ServiceOptions {
        ServiceOptions {
            library: Some(LibraryOptions::for_support(support)),
            midi: Some(MidiOptions::from_environment()),
            mk3: Some(Mk3Options { devices: Mk3Devices::System }),
            agent: Some(AgentOptions::for_support(support)),
        }
    }

    /// No service at all.
    pub fn none() -> ServiceOptions {
        ServiceOptions { library: None, midi: None, mk3: None, agent: None }
    }
}

/// The running services.
pub struct Services {
    pub library: Option<Arc<LibraryArea>>,
    pub midi: Option<Arc<Midi>>,
    pub mk3: Option<Arc<st_mk3::Mk3Service>>,
    pub agent: Option<Arc<Agent>>,
    stopped: AtomicBool,
}

impl Services {
    /// Opens the AI control socket (if the setting allows it). Call once the transport is up.
    pub fn start_agent(&self) {
        if let Some(agent) = &self.agent {
            agent.start();
        }
    }

    /// Ends every service in an orderly way (see the module documentation). Returns when that
    /// is done; a second call does nothing.
    pub fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(agent) = &self.agent {
            agent.shutdown();
        }
        if let Some(mk3) = &self.mk3 {
            mk3.shutdown();
        }
        if let Some(midi) = &self.midi {
            midi.shutdown();
        }
        if let Some(library) = &self.library {
            library.shutdown();
        }
    }
}

/// Installs the services `options` names and connects AI control to the quit handshake.
#[allow(clippy::too_many_arguments)]
pub fn install(
    commands: &mut Commands,
    session: &Arc<Session>,
    settings: &Arc<Settings>,
    lifecycle: &Lifecycle,
    events: EventSink,
    calls: Calls,
    engine: Option<Arc<AudioEngine>>,
    support: &Path,
    options: &ServiceOptions,
) -> Result<Services> {
    let library = match &options.library {
        Some(library) => Some(super::library::install(commands, session, calls, events.clone(), library)?),
        None => None,
    };
    let midi = match &options.midi {
        Some(midi) => Some(super::midi::install(commands, session, settings, events.clone(), engine, midi)?),
        None => None,
    };
    let mk3 = options.mk3.as_ref().map(|mk3| super::mk3::install(commands, support, settings, events.clone(), mk3));
    let agent = options.agent.as_ref().map(|agent| super::agent::install(commands, session, settings, events, agent));
    if let Some(agent) = &agent {
        let agent = agent.clone();
        lifecycle.set_fence(move |quitting| if quitting { agent.suspend() } else { agent.resume() });
    }
    Ok(Services { library, midi, mk3, agent, stopped: AtomicBool::new(false) })
}
