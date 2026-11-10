//! MIDI ports that exist only in memory, for tests of this crate and of the host: plug devices in
//! and out, play bytes into inputs, and read what outputs were sent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::ports::{Backend, InputCallback, InputPort, OpenError, OutputPort, PortInfo};

type SharedCallback = Arc<Mutex<InputCallback>>;
type OpenHook = Box<dyn FnMut(&MemoryPorts, i32) -> Option<OpenError> + Send>;

#[derive(Default)]
struct State {
    inputs: Vec<PortInfo>,
    outputs: Vec<PortInfo>,
    listeners: HashMap<i32, Vec<(u64, SharedCallback)>>,
    next_listener: u64,
    sent: Vec<Sent>,
    open_outputs: HashMap<i32, usize>,
    failing_outputs: Vec<i32>,
    input_opens: usize,
    output_opens: usize,
    enumeration_error: Option<String>,
}

/// One message an output received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The output's device id.
    pub output: i32,
    /// The message bytes.
    pub bytes: Vec<u8>,
    /// When the device layer was handed the message.
    pub at: Instant,
}

/// The set of in-memory ports. Clones share the same ports; [`MemoryPorts::backend`] is what a
/// [`crate::MidiHost`] is built on.
#[derive(Clone, Default)]
pub struct MemoryPorts {
    state: Arc<Mutex<State>>,
    hook: Arc<Mutex<Option<OpenHook>>>,
}

impl MemoryPorts {
    /// No ports.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A backend over these ports.
    pub fn backend(&self) -> Box<dyn Backend> {
        Box::new(MemoryBackend { ports: self.clone() })
    }

    /// Plugs in an input device.
    pub fn add_input(&self, id: i32, name: &str) {
        self.lock().inputs.push(PortInfo { id, name: name.into(), manufacturer: "Memory".into(), offline: false });
    }

    /// Plugs in an output device.
    pub fn add_output(&self, id: i32, name: &str) {
        self.lock().outputs.push(PortInfo { id, name: name.into(), manufacturer: "Memory".into(), offline: false });
    }

    /// Unplugs an input: it is no longer listed and its open connections fall silent.
    pub fn remove_input(&self, id: i32) {
        let mut state = self.lock();
        state.inputs.retain(|port| port.id != id);
        state.listeners.remove(&id);
    }

    /// Unplugs an output: it is no longer listed and sends to it fail.
    pub fn remove_output(&self, id: i32) {
        self.lock().outputs.retain(|port| port.id != id);
    }

    /// The device sends bytes: every open connection of the input receives them, on this thread.
    pub fn feed(&self, id: i32, bytes: &[u8]) {
        let listeners: Vec<SharedCallback> = self.lock().listeners.get(&id).map(|list| list.iter().map(|(_, callback)| callback.clone()).collect()).unwrap_or_default();
        for listener in listeners {
            let mut callback = listener.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            (*callback)(bytes);
        }
    }

    /// Everything outputs received so far, in order.
    pub fn sent(&self) -> Vec<Sent> {
        self.lock().sent.clone()
    }

    /// The bytes one output received so far, concatenated.
    pub fn sent_bytes(&self, output: i32) -> Vec<u8> {
        self.lock().sent.iter().filter(|sent| sent.output == output).flat_map(|sent| sent.bytes.clone()).collect()
    }

    /// The messages one output received so far.
    pub fn sent_messages(&self, output: i32) -> Vec<Vec<u8>> {
        self.lock().sent.iter().filter(|sent| sent.output == output).map(|sent| sent.bytes.clone()).collect()
    }

    /// Forgets what outputs received.
    pub fn clear_sent(&self) {
        self.lock().sent.clear();
    }

    /// Makes sends to an output fail (or work again) while it stays listed.
    pub fn fail_output(&self, id: i32, failing: bool) {
        let mut state = self.lock();
        state.failing_outputs.retain(|other| *other != id);
        if failing {
            state.failing_outputs.push(id);
        }
    }

    /// Makes enumeration fail with this message (`None`: work again).
    pub fn fail_enumeration(&self, error: Option<&str>) {
        self.lock().enumeration_error = error.map(str::to_string);
    }

    /// Runs `hook` at every attempt to open an input, before it is opened: the hook may unplug
    /// devices, and may make the attempt fail by returning an error.
    pub fn on_open_input(&self, hook: impl FnMut(&MemoryPorts, i32) -> Option<OpenError> + Send + 'static) {
        *self.hook.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
    }

    /// How many times an input was asked to open.
    pub fn input_open_attempts(&self) -> usize {
        self.lock().input_opens
    }

    /// How many times an output was asked to open.
    pub fn output_open_attempts(&self) -> usize {
        self.lock().output_opens
    }

    /// How many connections to this input are open.
    pub fn input_connections(&self, id: i32) -> usize {
        self.lock().listeners.get(&id).map_or(0, Vec::len)
    }

    /// How many connections to this output are open.
    pub fn output_connections(&self, id: i32) -> usize {
        self.lock().open_outputs.get(&id).copied().unwrap_or(0)
    }
}

struct MemoryBackend {
    ports: MemoryPorts,
}

struct MemoryInput {
    ports: MemoryPorts,
    id: i32,
    listener: u64,
}

impl InputPort for MemoryInput {}

impl Drop for MemoryInput {
    fn drop(&mut self) {
        let callback = {
            let mut state = self.ports.lock();
            let list = state.listeners.get_mut(&self.id);
            list.and_then(|list| list.iter().position(|(listener, _)| *listener == self.listener).map(|index| list.remove(index).1))
        };
        // Wait for a delivery in progress, as a real backend does when a port closes.
        if let Some(callback) = callback {
            drop(callback.lock());
        }
    }
}

struct MemoryOutput {
    ports: MemoryPorts,
    id: i32,
}

impl OutputPort for MemoryOutput {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut state = self.ports.lock();
        if !state.outputs.iter().any(|port| port.id == self.id) {
            return Err("the device is not connected".into());
        }
        if state.failing_outputs.contains(&self.id) {
            return Err("the device refused the message".into());
        }
        state.sent.push(Sent { output: self.id, bytes: bytes.to_vec(), at: Instant::now() });
        Ok(())
    }
}

impl Drop for MemoryOutput {
    fn drop(&mut self) {
        if let Some(count) = self.ports.lock().open_outputs.get_mut(&self.id) {
            *count = count.saturating_sub(1);
        }
    }
}

impl Backend for MemoryBackend {
    fn inputs(&mut self) -> Result<Vec<PortInfo>, String> {
        let state = self.ports.lock();
        state.enumeration_error.clone().map_or_else(|| Ok(state.inputs.clone()), Err)
    }

    fn outputs(&mut self) -> Result<Vec<PortInfo>, String> {
        let state = self.ports.lock();
        state.enumeration_error.clone().map_or_else(|| Ok(state.outputs.clone()), Err)
    }

    fn open_input(&mut self, id: i32, receive: InputCallback) -> Result<Box<dyn InputPort>, OpenError> {
        self.ports.lock().input_opens += 1;
        let hooked = {
            let mut hook = self.ports.hook.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            hook.as_mut().and_then(|hook| hook(&self.ports, id))
        };
        if let Some(error) = hooked {
            return Err(error);
        }
        let mut state = self.ports.lock();
        if !state.inputs.iter().any(|port| port.id == id) {
            return Err(OpenError::Gone);
        }
        state.next_listener += 1;
        let listener = state.next_listener;
        state.listeners.entry(id).or_default().push((listener, Arc::new(Mutex::new(receive))));
        Ok(Box::new(MemoryInput { ports: self.ports.clone(), id, listener }))
    }

    fn open_output(&mut self, id: i32) -> Result<Box<dyn OutputPort>, OpenError> {
        let mut state = self.ports.lock();
        state.output_opens += 1;
        if !state.outputs.iter().any(|port| port.id == id) {
            return Err(OpenError::Gone);
        }
        *state.open_outputs.entry(id).or_insert(0) += 1;
        Ok(Box::new(MemoryOutput { ports: self.ports.clone(), id }))
    }
}
