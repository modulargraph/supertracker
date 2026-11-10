//! The device layer under the MIDI host: a small interface that the `midir` backend implements for
//! real devices and [`crate::memory::MemoryPorts`] implements for tests.

use std::fmt;

/// One MIDI port as a backend lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// The device id: stable across launches and across unplugging and plugging the device back
    /// in, never zero. Settings and projects store it.
    pub id: i32,
    /// The name to show.
    pub name: String,
    /// The manufacturer, when the system knows it.
    pub manufacturer: String,
    /// The system lists the port but its device is not reachable right now.
    pub offline: bool,
}

/// Why a port could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The port went away between being listed and being opened. Not a failure: the next
    /// enumeration will not list it.
    Gone,
    /// The system refused; the text says why.
    Failed(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Gone => formatter.write_str("the device is no longer connected"),
            OpenError::Failed(message) => formatter.write_str(message),
        }
    }
}

/// Receives the bytes of an open input, on a thread of the backend. The bytes of one call hold
/// whole messages or any part of the stream; the host parses them (running status included).
pub type InputCallback = Box<dyn FnMut(&[u8]) + Send>;

/// An open input. Dropping it closes the port; no callback runs after the drop returns.
pub trait InputPort: Send {}

/// An open output.
pub trait OutputPort: Send {
    /// Sends one complete message now.
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// A MIDI system: lists ports and opens them.
pub trait Backend: Send {
    /// The inputs present now.
    fn inputs(&mut self) -> Result<Vec<PortInfo>, String>;
    /// The outputs present now.
    fn outputs(&mut self) -> Result<Vec<PortInfo>, String>;
    /// Opens the input with this id.
    fn open_input(&mut self, id: i32, receive: InputCallback) -> Result<Box<dyn InputPort>, OpenError>;
    /// Opens the output with this id.
    fn open_output(&mut self, id: i32) -> Result<Box<dyn OutputPort>, OpenError>;
}

/// A device id from a port's name: the same name gives the same id on every launch, and the
/// `occurrence`-th port of that name (0 for the first) gets its own. Positive and never zero.
pub fn id_from_name(name: &str, occurrence: usize) -> i32 {
    let mut hash: u32 = 0x811C_9DC5;
    for byte in name.bytes().chain([0u8]).chain((occurrence as u32).to_le_bytes()) {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    ((hash & 0x7FFF_FFFF) as i32).max(1)
}

/// Gives every port of a listing its id by name, counting repeated names.
pub fn ids_by_name<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<i32> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    names
        .into_iter()
        .map(|name| {
            let occurrence = seen.entry(name).or_insert(0);
            let id = id_from_name(name, *occurrence);
            *occurrence += 1;
            id
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_names_are_stable_distinct_and_never_zero() {
        assert_eq!(id_from_name("Minilogue xd", 0), id_from_name("Minilogue xd", 0));
        assert_ne!(id_from_name("Minilogue xd", 0), id_from_name("Minilogue xd", 1));
        assert_ne!(id_from_name("Minilogue xd", 0), id_from_name("Minilogue XD", 0));
        assert!(id_from_name("", 0) > 0);
        let ids = ids_by_name(["A", "B", "A", "A"]);
        assert_eq!(ids[0], id_from_name("A", 0));
        assert_eq!(ids[2], id_from_name("A", 1));
        assert_eq!(ids[3], id_from_name("A", 2));
        let unique: std::collections::HashSet<i32> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 4);
        // Pinned: stored settings refer to these values.
        assert_eq!(id_from_name("Midi Through:Midi Through Port-0", 0), 1_042_917_039);
    }
}
