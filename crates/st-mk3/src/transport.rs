//! The hardware as the driver sees it: three small interfaces. The real ones live in
//! [`crate::device`] (HID and USB) and [`crate::agents`] (Native Instruments' background agents);
//! [`crate::fake`] has in-memory ones for tests.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The controller's HID interface (USB interface 4): input reports in, LED reports out.
pub trait HidTransport: Send + Sync {
    /// Opens the controller if one is connected: `Ok(None)` when none is. Opening is shared: other
    /// software keeps its access.
    fn open(&self) -> Result<Option<HidConnection>, String>;
}

/// An open controller.
pub struct HidConnection {
    /// Input reports.
    pub reader: Box<dyn HidReader>,
    /// Output reports.
    pub writer: Box<dyn HidWriter>,
}

/// Why reading stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HidLoss {
    /// The controller was unplugged.
    Disconnected,
    /// Reading failed.
    Failed(String),
}

/// The reading end of an open controller.
pub trait HidReader: Send {
    /// Waits up to `timeout` for the next input report: `Ok(None)` when none came. The report
    /// starts with its report id where the system delivers it
    /// ([`crate::protocol::normalize_unidentified`] restores a missing one).
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidLoss>;
}

/// The writing end of an open controller.
pub trait HidWriter: Send {
    /// Sends an output report; byte 0 is the report id.
    fn write(&mut self, report: &[u8]) -> Result<(), String>;
}

/// The controller's display interface (USB interface 5, bulk endpoint 0x04).
pub trait ScreenTransport: Send + Sync {
    /// Whether a controller is connected. Looking does not open or claim anything.
    fn is_present(&self) -> bool;
    /// Claims the display interface. Fails while other software holds it.
    fn open(&self) -> Result<Box<dyn ScreenPipe>, String>;
}

/// The claimed display interface. Dropping it releases the interface.
pub trait ScreenPipe: Send {
    /// Sends one packet and returns how many bytes the device took. Fewer than the packet's length
    /// is a failed transfer (see [`require_complete`]).
    fn write(&mut self, packet: &[u8], timeout: Duration) -> Result<usize, String>;
}

/// A USB request can succeed and still report fewer bytes than the packet. Such a transfer must not
/// count as a frame on the screen: the next frame is sent in full.
pub fn require_complete(requested: usize, transferred: usize) -> Result<(), String> {
    if requested > 0 && transferred == requested {
        Ok(())
    } else {
        Err(format!(
            "Maschine MK3 screen transfer was incomplete ({transferred} of {requested} bytes). Check the USB connection and retry Take over screens."
        ))
    }
}

/// A background agent that was stopped and must be started again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    /// The agent's process name.
    pub name: String,
    /// Its launchd label, if it runs as a job.
    pub label: Option<String>,
    /// The application bundle to open when it is not a job.
    pub app_path: String,
    /// The job's property list, to load it again.
    pub plist_path: Option<String>,
}

/// Other software that owns the controller's screens. On macOS Native Instruments' agents
/// (NIHardwareAgent and NIHostIntegrationAgent) hold the display interface; to draw the screens
/// they are stopped, and started again when the controller is released, the application quits or,
/// through a record on disk, at the next launch after a crash.
pub trait VendorAgents: Send + Sync {
    /// Starts agents an earlier run left stopped (it crashed while it owned the screens).
    fn recover(&self, _log: &dyn Fn(&str)) {}
    /// Stops the running agents and returns what must be restored. The record is on disk before
    /// anything is stopped.
    fn stop(&self, log: &dyn Fn(&str)) -> Result<Vec<AgentRecord>, String>;
    /// Starts stopped agents again. Returns whether all of them run; the record on disk is removed
    /// then.
    fn restore(&self, records: &[AgentRecord], log: &dyn Fn(&str)) -> bool;
    /// The record on disk: agents that are stopped right now.
    fn read_state(&self) -> Vec<AgentRecord>;
}

/// No vendor software to step around (every system but macOS).
pub struct NoAgents;

impl VendorAgents for NoAgents {
    fn stop(&self, _log: &dyn Fn(&str)) -> Result<Vec<AgentRecord>, String> {
        Ok(Vec::new())
    }

    fn restore(&self, _records: &[AgentRecord], _log: &dyn Fn(&str)) -> bool {
        true
    }

    fn read_state(&self) -> Vec<AgentRecord> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_complete_transfer_counts() {
        // scripts/test-mk3-lifecycle.sh, scenario transfer-validation.
        for (requested, transferred) in [(100, 0), (100, 99), (100, 101), (0, 0)] {
            assert!(require_complete(requested, transferred).is_err(), "{transferred} of {requested}");
        }
        assert_eq!(require_complete(100, 100), Ok(()));
        assert_eq!(
            require_complete(100, 99).unwrap_err(),
            "Maschine MK3 screen transfer was incomplete (99 of 100 bytes). Check the USB connection and retry Take over screens."
        );
    }

    #[test]
    fn agent_records_keep_their_stored_form() {
        let record = AgentRecord { name: "NIHardwareAgent".into(), label: Some("com.native-instruments.x".into()), app_path: "/a.app".into(), plist_path: None };
        let stored = serde_json::to_value(&record).unwrap();
        assert_eq!(stored, serde_json::json!({"name": "NIHardwareAgent", "label": "com.native-instruments.x", "appPath": "/a.app", "plistPath": null}));
        assert_eq!(serde_json::from_value::<AgentRecord>(stored).unwrap(), record);
        assert!(NoAgents.stop(&|_| {}).unwrap().is_empty() && NoAgents.restore(&[], &|_| {}) && NoAgents.read_state().is_empty());
    }
}
