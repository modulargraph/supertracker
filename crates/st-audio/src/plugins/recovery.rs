//! The recovery store: which plug-ins failed while loading or were lost while running.
//!
//! A plug-in runs in a container process, so a crash costs that container and never the
//! application. Two things still need remembering:
//!
//! * **Crashes at load.** A plug-in whose container died (or stopped answering) while the
//!   plug-in was being created, activated or given its state is counted in
//!   `plugin-recovery.json`. After [`LOAD_CRASH_LIMIT`] such crashes in a row, and after a load
//!   the application itself did not survive, the plug-in is not loaded any more: its inserts
//!   pass through, its descriptors and states stay in the project, and it is listed for
//!   review. "Retry" in the review authorizes the next attempt; a successful load clears the
//!   count.
//! * **Losses while running** (this session only): a plug-in whose container crashed or hung
//!   during playback is listed for review until it is restored.
//!
//! The report has the shape the interface's recovery dialog reads (`plugin-recovery.js`).

use super::ident::Identity;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Crashes at load in a row after which a plug-in is held back.
pub const LOAD_CRASH_LIMIT: u32 = 2;

#[derive(Debug, Clone, Default)]
struct Entry {
    identity: Identity,
    /// Crashes at load in a row.
    load_crashes: u32,
    /// A load was in flight when the file was last written.
    loading: bool,
    /// A load was in flight when an earlier session ended.
    interrupted: bool,
    /// The user chose Retry: the next attempt is allowed.
    authorized: bool,
    /// Instances lost while running, this session.
    lost: u32,
    reasons: Vec<String>,
}

impl Entry {
    fn held(&self) -> bool {
        !self.authorized && (self.load_crashes >= LOAD_CRASH_LIMIT || self.interrupted)
    }

    fn requires_review(&self) -> bool {
        self.held() || self.lost > 0
    }

    fn persistent(&self) -> bool {
        self.load_crashes > 0 || self.loading || self.interrupted
    }
}

/// See the module documentation. All methods may be called from any control thread.
pub struct RecoveryStore {
    path: Option<PathBuf>,
    entries: Mutex<BTreeMap<String, Entry>>,
    warnings: Mutex<Vec<String>>,
}

impl RecoveryStore {
    /// Reads `path` (`None`: nothing is remembered between runs). A load that was in flight
    /// when the file was last written did not finish: its plug-in is held for review.
    pub fn open(path: Option<PathBuf>) -> RecoveryStore {
        let mut entries = BTreeMap::new();
        let mut warnings = Vec::new();
        if let Some(text) = path.as_ref().and_then(|path| std::fs::read_to_string(path).ok()) {
            match serde_json::from_str::<Value>(&text) {
                Ok(value) if value["version"] == 1 => {
                    for item in value["entries"].as_array().into_iter().flatten() {
                        let text = |key: &str| item[key].as_str().unwrap_or_default().to_string();
                        let identity = Identity { format: text("format"), id: text("id"), path: text("path"), name: text("name"), manufacturer: text("manufacturer") };
                        if identity.format.is_empty() || identity.id.is_empty() {
                            continue;
                        }
                        let interrupted = item["loading"] == true || item["interrupted"] == true;
                        let mut reasons: Vec<String> = item["reasons"].as_array().into_iter().flatten().filter_map(|reason| reason.as_str().map(String::from)).collect();
                        if item["loading"] == true {
                            reasons.push("SuperTracker ended while this plug-in was loading.".to_string());
                        }
                        entries.insert(identity.key(), Entry { identity, load_crashes: item["loadCrashes"].as_u64().unwrap_or(0) as u32, loading: false, interrupted, authorized: false, lost: 0, reasons });
                    }
                }
                _ => warnings.push("The plug-in recovery file could not be read and was started afresh.".to_string()),
            }
        }
        let store = RecoveryStore { path, entries: Mutex::new(entries), warnings: Mutex::new(warnings) };
        store.save(&store.lock());
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn save(&self, entries: &BTreeMap<String, Entry>) {
        let Some(path) = &self.path else { return };
        let items: Vec<Value> = entries
            .values()
            .filter(|entry| entry.persistent())
            .map(|entry| {
                let identity = &entry.identity;
                json!({
                    "format": identity.format, "id": identity.id, "path": identity.path, "name": identity.name, "manufacturer": identity.manufacturer,
                    "loadCrashes": entry.load_crashes, "loading": entry.loading, "interrupted": entry.interrupted, "reasons": entry.reasons,
                })
            })
            .collect();
        let text = json!({ "version": 1, "entries": items }).to_string();
        let temporary = path.with_extension("json.tmp");
        let written = std::fs::write(&temporary, text).and_then(|_| std::fs::rename(&temporary, path));
        if let Err(error) = written {
            let mut warnings = self.warnings.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let message = format!("The plug-in recovery file could not be written: {error}");
            if !warnings.contains(&message) {
                warnings.push(message);
            }
        }
    }

    /// Why `identity` is held back, if it is.
    pub fn held(&self, identity: &Identity) -> Option<String> {
        let entries = self.lock();
        entries.get(&identity.key()).filter(|entry| entry.held()).map(|entry| {
            let name = entry.identity.display_name().to_string();
            if entry.interrupted && entry.load_crashes < LOAD_CRASH_LIMIT {
                format!("{name} was held back because SuperTracker ended unexpectedly while it was loading. Choose Review interrupted plug-ins to load it again.")
            } else {
                format!("{name} was held back because it stopped working {} times in a row while loading. Choose Review interrupted plug-ins to load it again.", entry.load_crashes)
            }
        })
    }

    /// A load of `identity` starts: written to disk before any of the plug-in's code runs.
    pub fn begin_load(&self, identity: &Identity) {
        let mut entries = self.lock();
        let entry = entries.entry(identity.key()).or_default();
        entry.identity = identity.clone();
        entry.loading = true;
        self.save(&entries);
    }

    /// The load finished: `crashed` tells a container that died or hung from any other outcome
    /// (loaded, or refused with an ordinary error).
    pub fn end_load(&self, identity: &Identity, crashed: Option<&str>) {
        let mut entries = self.lock();
        let key = identity.key();
        let Some(entry) = entries.get_mut(&key) else { return };
        entry.loading = false;
        match crashed {
            Some(reason) => {
                entry.load_crashes += 1;
                entry.authorized = false;
                entry.reasons.push(reason.to_string());
                let excess = entry.reasons.len().saturating_sub(8);
                entry.reasons.drain(..excess);
            }
            None => {
                entry.load_crashes = 0;
                entry.interrupted = false;
                entry.authorized = false;
                if entry.lost == 0 {
                    entry.reasons.clear();
                }
            }
        }
        if !entry.persistent() && entry.lost == 0 {
            entries.remove(&key);
        }
        self.save(&entries);
    }

    /// An instance of `identity` was lost while running.
    pub fn lost(&self, identity: &Identity, reason: &str) {
        let mut entries = self.lock();
        let entry = entries.entry(identity.key()).or_default();
        entry.identity = identity.clone();
        entry.lost += 1;
        if !entry.reasons.iter().any(|known| known == reason) {
            entry.reasons.push(reason.to_string());
        }
    }

    /// A lost instance of `identity` works again (or is gone from the project).
    pub fn restored(&self, identity: &Identity) {
        let mut entries = self.lock();
        let key = identity.key();
        if let Some(entry) = entries.get_mut(&key) {
            entry.lost = entry.lost.saturating_sub(1);
            if entry.lost == 0 && !entry.persistent() {
                entries.remove(&key);
            }
        }
    }

    /// The user chose Retry for these keys: their next load is allowed. Returns the
    /// identities that were waiting for review.
    pub fn authorize(&self, keys: &[String]) -> Vec<Identity> {
        let mut entries = self.lock();
        let mut found = Vec::new();
        for key in keys {
            if let Some(entry) = entries.get_mut(key).filter(|entry| entry.requires_review()) {
                entry.authorized = true;
                entry.interrupted = false;
                found.push(entry.identity.clone());
            }
        }
        self.save(&entries);
        found
    }

    /// The keys waiting for review.
    pub fn pending_keys(&self) -> Vec<String> {
        self.lock().iter().filter(|(_, entry)| entry.requires_review()).map(|(key, _)| key.clone()).collect()
    }

    /// `plugins.recovery`: `{suspects: [{identity: {key, format, component, name,
    /// bundlePath?}, reasons, requiresReview, retryAuthorized, acknowledged, attribution,
    /// loadCrashes, lostInstances}], warnings, enabled, pendingInstances, ...}`.
    pub fn report(&self) -> Value {
        let entries = self.lock();
        let suspects: Vec<Value> = entries
            .iter()
            .filter(|(_, entry)| entry.requires_review() || entry.load_crashes > 0 || entry.authorized)
            .map(|(key, entry)| {
                let mut identity = identity_value(&entry.identity);
                identity["key"] = json!(key);
                json!({
                    "identity": identity, "reasons": entry.reasons, "sessionIDs": [], "acknowledged": entry.authorized, "retryAuthorized": entry.authorized,
                    "requiresReview": entry.requires_review(), "attribution": if entry.lost > 0 || entry.load_crashes > 0 { "its plug-in process stopped" } else { "suspected, not proven" },
                    "loadCrashes": entry.load_crashes, "lostInstances": entry.lost, "heldBack": entry.held(),
                })
            })
            .collect();
        let pending: u32 = entries.values().map(|entry| entry.lost).sum();
        json!({
            "suspects": suspects, "warnings": *self.warnings.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
            "liveSessionsIgnored": 0, "enabled": true, "pendingInstances": pending, "cleanShutdown": true,
        })
    }
}

fn identity_value(identity: &Identity) -> Value {
    let mut value = json!({ "format": identity.format_label(), "component": identity.component(), "name": identity.display_name(), "manufacturer": identity.manufacturer });
    if !identity.path.is_empty() {
        value["bundlePath"] = json!(identity.path);
    }
    value
}

impl RecoveryStore {
    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(id: &str) -> Identity {
        Identity { format: "clap".into(), id: id.into(), path: "/plugins/x.clap".into(), name: "X".into(), manufacturer: "M".into() }
    }

    #[test]
    fn two_crashes_at_load_in_a_row_hold_a_plug_in_back_until_retry_is_chosen() {
        let folder = std::env::temp_dir().join(format!("st-recovery-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("plugin-recovery.json");
        let x = identity("x");
        {
            let store = RecoveryStore::open(Some(path.clone()));
            assert!(store.held(&x).is_none());
            store.begin_load(&x);
            store.end_load(&x, Some("The plug-in process crashed (signal 6)."));
            assert!(store.held(&x).is_none(), "one crash is not a habit");
            assert_eq!(store.report()["suspects"][0]["requiresReview"], false);
            store.begin_load(&x);
            store.end_load(&x, Some("The plug-in process crashed (signal 6)."));
            assert!(store.held(&x).unwrap().contains("2 times"));
        }
        // The next launch still holds it back, and says so in the shape the dialog reads.
        let store = RecoveryStore::open(Some(path.clone()));
        assert!(store.held(&x).is_some());
        let report = store.report();
        let suspect = &report["suspects"][0];
        assert_eq!((suspect["requiresReview"].clone(), suspect["identity"]["key"].clone(), suspect["identity"]["name"].clone()), (json!(true), json!(x.key()), json!("X")));
        assert_eq!(suspect["identity"]["component"], "clap:x");
        assert_eq!(store.pending_keys(), [x.key()]);
        // Retry authorizes one attempt; success clears everything.
        assert_eq!(store.authorize(&[x.key(), "unknown".into()]), std::slice::from_ref(&x));
        assert!(store.held(&x).is_none());
        store.begin_load(&x);
        store.end_load(&x, None);
        assert_eq!(store.entry_count(), 0);
        assert_eq!(RecoveryStore::open(Some(path.clone())).report()["suspects"], json!([]));
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_load_the_application_did_not_survive_is_held_for_review() {
        let folder = std::env::temp_dir().join(format!("st-recovery-b-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("plugin-recovery.json");
        let x = identity("y");
        RecoveryStore::open(Some(path.clone())).begin_load(&x);
        let store = RecoveryStore::open(Some(path.clone()));
        assert!(store.held(&x).unwrap().contains("ended unexpectedly"));
        assert_eq!(store.report()["suspects"][0]["requiresReview"], true);
        store.authorize(&[x.key()]);
        assert!(store.held(&x).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_loss_while_running_is_listed_until_the_instance_is_restored() {
        let store = RecoveryStore::open(None);
        let x = identity("z");
        store.lost(&x, "The plug-in process crashed (signal 11).");
        let report = store.report();
        assert_eq!((report["suspects"][0]["requiresReview"].clone(), report["pendingInstances"].clone()), (json!(true), json!(1)));
        assert!(store.held(&x).is_none(), "a loss while running does not stop the next load");
        store.restored(&x);
        assert_eq!(store.report()["suspects"], json!([]));
    }
}
