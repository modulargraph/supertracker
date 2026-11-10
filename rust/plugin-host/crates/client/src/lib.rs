//! Host side of SuperTracker's out-of-process plug-in hosting.
//!
//! A [`Supervisor`] spawns `supertracker-plugin-container` processes and places plug-in
//! instances in them according to a [`GroupingPolicy`]. Each [`Instance`] is a proxy:
//!
//! * [`Instance::process`] runs on the host's audio thread. It is real-time safe (no
//!   allocation, no lock, a bounded wait) and synchronous: the block is processed by the
//!   container within the call, so the transport adds **no latency**. If the container
//!   misses the deadline or is gone, the call returns the fallback (silence, or the dry
//!   signal for effects) and flags the instance; it does not wait again until the container
//!   has caught up or the instance was restored.
//! * Everything else (parameters, state, presets, editor, activation) is a blocking request
//!   over the control channel, for the host's control threads.
//!
//! Container crashes and hangs surface as [`HostEvent`]s (poll or callback). The client
//! remembers each instance's configuration and last known state, so
//! [`Supervisor::restart_container`] / [`Instance::restore`] can rebuild lost instances.
//!
//! See `docs/PLUGIN_CONTAINER.md` in the repository for the design.

mod connection;
mod instance;
mod supervisor;

use std::fmt;
use std::time::Duration;

pub use instance::{
    Fallback, Instance, InstanceConfig, InstanceStats, InstanceStatus, PluginKind, ProcessBuffers, ProcessOutcome,
    RawBlock, DEADLINE_NONE, PROCESS_BUSY, PROCESS_BYPASSED, PROCESS_EVENTS_DROPPED, PROCESS_FALLBACK,
    PROCESS_INACTIVE, PROCESS_INVALID, PROCESS_LATE, PROCESS_LOST, PROCESS_OK, PROCESS_OUT_EVENTS_DROPPED,
    PROCESS_PLUGIN_ERROR, PROCESS_RESYNCED,
};
pub use stpc_transport as transport;
pub use stpc_transport::messages::{
    BusInfo, EditorInfo, InstanceInfo, ParameterInfo, ParameterValue, PresetInfo, INFO_HAS_BYPASS, INFO_HAS_EDITOR,
    INFO_INSTRUMENT, INFO_PRODUCES_MIDI, INFO_WANTS_MIDI, PARAM_AUTOMATABLE, PARAM_BOOLEAN, PARAM_BYPASS,
    PARAM_HIDDEN, PARAM_READ_ONLY, PARAM_STEPPED,
};
pub use stpc_transport::region::{
    Event, Transport, EVENT_FLAG_SEQUENCED, EVENT_MIDI, EVENT_PARAM, TRANSPORT_LOOPING, TRANSPORT_PLAYING,
    TRANSPORT_PPQ_VALID, TRANSPORT_RECORDING, TRANSPORT_TEMPO_VALID, TRANSPORT_TIME_SIG_VALID,
};
pub use stpc_transport::PROTOCOL_VERSION;
pub use supervisor::{
    ContainerInfo, ContainerState, ExitReason, GroupingPolicy, HostEvent, LossReason, RestartReport, Supervisor,
    SupervisorConfig, WakePreference, CONTAINER_BINARY_NAME, ENV_CONTAINER_PATH,
};

pub type InstanceId = u64;
pub type ContainerId = u32;

#[derive(Debug)]
pub enum Error {
    /// Bad configuration or argument.
    Invalid(String),
    /// The container process could not be started or did not connect.
    Spawn(String),
    /// The container was refused at the handshake (protocol version or token mismatch).
    Rejected(String),
    Io(std::io::Error),
    Protocol(String),
    /// No answer within the request's timeout. The container's main thread may be busy
    /// (a slow plug-in) or hung; the request may still complete later.
    Timeout(Duration),
    /// The container process died or was killed before answering.
    ContainerDied,
    /// The instance's container is gone; restore the instance first.
    InstanceLost,
    /// The container has no backend for the requested plug-in format.
    UnknownFormat(String),
    /// The plug-in does not offer this capability.
    Unsupported,
    /// The plug-in (or its backend) reported a failure.
    Plugin(String),
    /// The supervisor was shut down.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(msg) => write!(f, "invalid argument: {msg}"),
            Error::Spawn(msg) => write!(f, "cannot start the plug-in container: {msg}"),
            Error::Rejected(msg) => write!(f, "plug-in container rejected: {msg}"),
            Error::Io(err) => write!(f, "plug-in container I/O: {err}"),
            Error::Protocol(msg) => write!(f, "plug-in container protocol error: {msg}"),
            Error::Timeout(after) => write!(f, "the plug-in container did not answer within {after:?}"),
            Error::ContainerDied => write!(f, "the plug-in container process died"),
            Error::InstanceLost => write!(f, "the plug-in instance was lost with its container"),
            Error::UnknownFormat(msg) => write!(f, "{msg}"),
            Error::Unsupported => write!(f, "not supported by this plug-in"),
            Error::Plugin(msg) => write!(f, "{msg}"),
            Error::Closed => write!(f, "the plug-in supervisor is shut down"),
        }
    }
}
impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
