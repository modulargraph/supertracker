//! Helpers for tests: of this crate, and of every crate that registers commands.
//!
//! ```
//! use st_host::testing::TestHost;
//! use st_host::{NoPayload, Result};
//! use serde_json::json;
//!
//! let host = TestHost::start(|bridge| {
//!     bridge.commands().register("math.double", |value: f64| -> Result<f64> { Ok(value * 2.0) });
//!     let events = bridge.events();
//!     bridge.commands().register("math.announce", move |_: NoPayload| -> Result<()> {
//!         events.emit("math.announced", json!({"answer": 42}));
//!         Ok(())
//!     });
//! });
//! assert_eq!(host.request("math.double", json!(21)), Ok(json!(42.0)));
//! host.request("math.announce", json!({})).unwrap();
//! assert_eq!(host.events(), [("math.announced".to_string(), json!({"answer": 42}))]);
//! assert!(host.request("math.halve", json!(1)).unwrap_err().contains("math.halve"));
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::transport::{Next, Outbox};
use crate::{Bridge, BridgeBuilder, Connection};

/// A folder under the system's temporary directory that is removed when the value is dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates a new empty folder whose name contains `label`.
    pub fn new(label: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.subsec_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("st-host-{label}-{}-{unique}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create a temporary folder");
        TempDir(path)
    }

    /// The folder.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A bridge with one attached interface that the test plays: send requests, read replies and events.
pub struct TestHost {
    pub bridge: Bridge,
    pub connection: Connection,
    outbox: Arc<Outbox>,
    next_id: AtomicU64,
    /// Messages taken from the outbox while looking for something else.
    seen: std::sync::Mutex<Vec<Value>>,
}

impl TestHost {
    /// Starts a bridge with the commands `register` adds and attaches an interface.
    ///
    /// Events are only delivered to interfaces that have sent `ready`. When `register` adds no
    /// `ready` command, the interface is made ready here (the request fails, which is enough); when
    /// it does, the test sends `ready` itself, like the page.
    pub fn start(register: impl FnOnce(&mut BridgeBuilder)) -> Self {
        let mut builder = Bridge::builder();
        register(&mut builder);
        let has_ready = builder.commands().contains("ready");
        let bridge = builder.start();
        let outbox = Outbox::new();
        let connection = bridge.connect(outbox.clone());
        let host = TestHost { bridge, connection, outbox, next_id: AtomicU64::new(1), seen: Default::default() };
        if !has_ready {
            let _ = host.request("ready", json!({}));
        }
        host
    }

    /// Sends a request without waiting; returns its id.
    pub fn send(&self, command: &str, payload: Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.connection.receive(&json!({ "id": id, "command": command, "payload": payload }).to_string());
        id
    }

    /// Waits for the reply to request `id`: its result, or its error message.
    ///
    /// # Panics
    /// When no reply arrives within ten seconds.
    pub fn reply(&self, id: u64) -> Result<Value, String> {
        let message = self.wait_for(|message| message.get("id").and_then(Value::as_u64) == Some(id), Duration::from_secs(10));
        let message = message.unwrap_or_else(|| panic!("no reply to request {id}"));
        if message["ok"] == json!(true) {
            Ok(message["result"].clone())
        } else {
            Err(message["error"].as_str().unwrap_or_default().to_string())
        }
    }

    /// Sends a request and waits for its reply.
    pub fn request(&self, command: &str, payload: Value) -> Result<Value, String> {
        let id = self.send(command, payload);
        self.reply(id)
    }

    /// Every message received so far and not yet taken, oldest first, as JSON values.
    pub fn messages(&self) -> Vec<Value> {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(self.outbox.drain().iter().map(|text| serde_json::from_str(text).expect("the host sends JSON")));
        std::mem::take(&mut *seen)
    }

    /// The events received so far and not yet taken, as `(name, payload)`. Replies stay available to
    /// [`reply`](TestHost::reply).
    pub fn events(&self) -> Vec<(String, Value)> {
        let mut seen = self.seen.lock().unwrap();
        seen.extend(self.outbox.drain().iter().map(|text| serde_json::from_str(text).expect("the host sends JSON")));
        let (events, replies): (Vec<Value>, Vec<Value>) = std::mem::take(&mut *seen).into_iter().partition(|message| message.get("type").is_some());
        *seen = replies;
        events.into_iter().map(|event| (event["type"].as_str().unwrap_or_default().to_string(), event["payload"].clone())).collect()
    }

    /// Waits up to `timeout` for the event `name` and returns its payload.
    pub fn wait_for_event(&self, name: &str, timeout: Duration) -> Option<Value> {
        self.wait_for(|message| message.get("type").and_then(Value::as_str) == Some(name), timeout).map(|event| event["payload"].clone())
    }

    fn wait_for(&self, wanted: impl Fn(&Value) -> bool, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            {
                let mut seen = self.seen.lock().unwrap();
                if let Some(index) = seen.iter().position(&wanted) {
                    return Some(seen.remove(index));
                }
            }
            let remaining = deadline.checked_duration_since(Instant::now())?;
            match self.outbox.wait(remaining) {
                Next::Message(text) => self.seen.lock().unwrap().push(serde_json::from_str(&text).expect("the host sends JSON")),
                Next::Idle | Next::Closed => return None,
            }
        }
    }
}
