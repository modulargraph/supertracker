//! Errors of the control channel.

use std::fmt;

/// Why a call over the control socket failed. The `Display` text is written for the person (or
/// model) on the other end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// Nothing listens on the control socket: the application is not open, or AI control is off.
    /// The value is the technical detail (the system's error and the socket path).
    NotRunning(String),
    /// The connection failed or was closed.
    Transport(String),
    /// A message could not be understood.
    Protocol(String),
    /// The application answered with an error, or did not answer in time.
    Remote(String),
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ControlError::NotRunning(detail) => write!(
                formatter,
                "SuperTracker is not running or AI control is off. Open SuperTracker first (Settings › Allow AI control (MCP) must be on). [{detail}]"
            ),
            ControlError::Transport(message) | ControlError::Protocol(message) | ControlError::Remote(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ControlError {}
