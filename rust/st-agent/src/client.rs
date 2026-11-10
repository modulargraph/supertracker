//! The client end of the control socket, used by the MCP server and the scripting command line.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::framing::{self, LineBuffer};
use crate::transport::{self, Channel};
use crate::ControlError;

/// How long a call waits for the application's answer unless told otherwise. Calls such as an
/// audio export, or a command that opens a dialog and waits for the user, take long.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// What a control method returned.
#[derive(Debug, Clone, PartialEq)]
pub struct CallOutcome {
    /// The method's result.
    pub result: Value,
    /// Messages the application showed the user while the call ran (toasts).
    pub notices: Vec<String>,
}

/// A blocking client. It keeps one connection open (the application shows "AI connected" while it
/// is) and reconnects once when the application was restarted between two calls.
pub struct ControlClient {
    path: PathBuf,
    channel: Option<Arc<dyn Channel>>,
    buffer: LineBuffer,
    queued: Vec<Vec<u8>>,
    next_id: u64,
}

impl ControlClient {
    /// A client for the control socket at `path` (see [`crate::socket_path`]). Nothing is opened
    /// until the first call.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        ControlClient { path: path.into(), channel: None, buffer: LineBuffer::default(), queued: Vec::new(), next_id: 1 }
    }

    /// The socket this client talks to.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True while the connection is open.
    pub fn is_connected(&self) -> bool {
        self.channel.is_some()
    }

    /// Opens the connection if it is not open.
    pub fn connect(&mut self) -> Result<(), ControlError> {
        if self.channel.is_none() {
            self.channel = Some(transport::connect(&self.path)?);
            self.buffer = LineBuffer::default();
            self.queued.clear();
        }
        Ok(())
    }

    /// Closes the connection.
    pub fn disconnect(&mut self) {
        if let Some(channel) = self.channel.take() {
            channel.close();
        }
    }

    /// Calls a control method with [`DEFAULT_TIMEOUT`].
    pub fn call(&mut self, method: &str, params: Map<String, Value>) -> Result<CallOutcome, ControlError> {
        self.call_within(method, params, DEFAULT_TIMEOUT)
    }

    /// Calls a control method and waits up to `timeout` for the answer.
    ///
    /// A connection that turns out to be dead (the application quit and was opened again since the
    /// last call) is replaced once and the call sent again.
    pub fn call_within(&mut self, method: &str, params: Map<String, Value>, timeout: Duration) -> Result<CallOutcome, ControlError> {
        match self.attempt(method, &params, timeout) {
            Err(ControlError::Transport(_)) if !self.is_connected() => self.attempt(method, &params, timeout),
            outcome => outcome,
        }
    }

    fn attempt(&mut self, method: &str, params: &Map<String, Value>, timeout: Duration) -> Result<CallOutcome, ControlError> {
        self.connect()?;
        let id = self.next_id;
        self.next_id += 1;
        let request = framing::encode(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        if self.channel().write_all(&request).is_err() {
            self.disconnect();
            return Err(ControlError::Transport("SuperTracker closed the control connection.".into()));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let line = self.receive_line(deadline)?;
            let message = framing::decode(&line)?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            let notices: Vec<String> = message
                .get("notices")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            if let Some(error) = message.get("error").and_then(Value::as_object) {
                let mut text = error.get("message").and_then(Value::as_str).unwrap_or("The request failed.").to_string();
                for notice in &notices {
                    text.push('\n');
                    text.push_str(notice);
                }
                return Err(ControlError::Remote(text));
            }
            return Ok(CallOutcome { result: message.get("result").cloned().unwrap_or(Value::Null), notices });
        }
    }

    fn channel(&self) -> &Arc<dyn Channel> {
        self.channel.as_ref().expect("connected before use")
    }

    fn receive_line(&mut self, deadline: Instant) -> Result<Vec<u8>, ControlError> {
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if !self.queued.is_empty() {
                return Ok(self.queued.remove(0));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.disconnect();
                return Err(ControlError::Remote("SuperTracker did not answer in time.".into()));
            }
            let channel = self.channel().clone();
            let _ = channel.set_read_timeout(Some(remaining.min(Duration::from_secs(3600))));
            match channel.read(&mut chunk) {
                Ok(0) => {
                    self.disconnect();
                    return Err(ControlError::Transport(
                        "SuperTracker closed the control connection (it may have quit or AI control was turned off).".into(),
                    ));
                }
                Ok(count) => {
                    let lines = self.buffer.append(&chunk[..count]);
                    if self.buffer.overflowed() {
                        self.disconnect();
                        return Err(ControlError::Protocol("The response was too large.".into()));
                    }
                    self.queued.extend(lines);
                }
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => {
                    self.disconnect();
                    return Err(ControlError::Transport("The control connection failed.".into()));
                }
            }
        }
    }
}

impl Drop for ControlClient {
    fn drop(&mut self) {
        self.disconnect();
    }
}
