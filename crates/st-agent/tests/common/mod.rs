//! Shared helpers of the integration tests: a scripted page and short socket paths.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use st_agent::Page;

/// A folder with a short path (socket addresses are limited to about a hundred bytes), removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!("st-agent-{label}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        let mut path = std::env::temp_dir().join(&name);
        if path.as_os_str().len() > 60 {
            path = PathBuf::from("/tmp").join(&name);
        }
        std::fs::create_dir_all(&path).expect("create a temporary folder");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn socket(&self) -> PathBuf {
        self.0.join("control.sock")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A PNG file of 3×2 pixels (a red, a green and a blue column).
pub const PNG_3X2: [u8; 73] = [
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02, 0x08,
    0x02, 0x00, 0x00, 0x00, 0x12, 0x16, 0xF1, 0x4D, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78, 0xDA, 0x63, 0xF8, 0xCF, 0xC0, 0xC0, 0x00, 0xC1,
    0x70, 0x16, 0x00, 0x3B, 0xD8, 0x05, 0xFB, 0x36, 0x11, 0x98, 0xEA, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];
/// [`PNG_3X2`] in base64.
pub const PNG_3X2_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAMAAAACCAIAAAASFvFNAAAAEElEQVR42mP4z8DAAMFwFgA72AX7NhGY6gAAAABJRU5ErkJggg==";

/// Waits until `condition` holds, for at most five seconds.
pub fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A page whose agent API is a few scripted methods.
///
/// - `echo` (and anything unknown): `{ok: true, result: {method, params}}`
/// - `noisy`: a result with the notice "Saved"
/// - `boom`: `{ok: false, error: "Unknown method \"boom\".", notices: ["Careful"]}`
/// - `garbage`: a reply that is not an object
/// - `offline`: the page cannot be asked
/// - `slow`: blocks until [`FakePage::release`] is called, then answers like `echo`
pub struct FakePage {
    pub calls: Mutex<Vec<(String, Value)>>,
    pub connected: Mutex<bool>,
    pub project: Mutex<Option<String>>,
    entered: Mutex<Sender<()>>,
    pub entered_slow: Mutex<Receiver<()>>,
    release: Mutex<Receiver<()>>,
    releaser: Mutex<Sender<()>>,
}

impl FakePage {
    pub fn new() -> Arc<Self> {
        let (entered, entered_slow) = mpsc::channel();
        let (releaser, release) = mpsc::channel();
        Arc::new(FakePage {
            calls: Mutex::new(Vec::new()),
            connected: Mutex::new(true),
            project: Mutex::new(None),
            entered: Mutex::new(entered),
            entered_slow: Mutex::new(entered_slow),
            release: Mutex::new(release),
            releaser: Mutex::new(releaser),
        })
    }

    /// Lets one `slow` call finish.
    pub fn release(&self) {
        self.releaser.lock().unwrap().send(()).unwrap();
    }

    /// Waits until a `slow` call is running in the page.
    pub fn wait_slow(&self) {
        self.entered_slow.lock().unwrap().recv_timeout(Duration::from_secs(5)).expect("a slow call starts");
    }

    pub fn methods(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|(method, _)| method.clone()).collect()
    }
}

impl Page for FakePage {
    fn connected(&self) -> bool {
        *self.connected.lock().unwrap()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push((method.to_string(), params.clone()));
        match method {
            "noisy" => Ok(json!({"ok": true, "result": {"method": method}, "notices": ["Saved"]})),
            "boom" => Ok(json!({"ok": false, "error": "Unknown method \"boom\".", "notices": ["Careful"]})),
            "garbage" => Ok(json!("not an object")),
            "offline" => Err("the interface went away.".into()),
            "slow" => {
                self.entered.lock().unwrap().send(()).unwrap();
                self.release.lock().unwrap().recv_timeout(Duration::from_secs(10)).expect("the test releases the slow call");
                Ok(json!({"ok": true, "result": {"method": method, "params": params}}))
            }
            _ => Ok(json!({"ok": true, "result": {"method": method, "params": params}})),
        }
    }

    fn project_path(&self) -> Option<String> {
        self.project.lock().unwrap().clone()
    }

    fn screenshot(&self, width: Option<u32>) -> Result<Value, String> {
        Ok(json!({"mimeType": "image/png", "data": "iVBORw0KGgo=", "width": width.unwrap_or(1280), "height": 800}))
    }
}
