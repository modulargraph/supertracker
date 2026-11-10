//! The open document as the session commands need it, behind a trait.
//!
//! `st-project` owns project files (validation, the media folder, recovery, templates). Its
//! `session::Session` is the intended implementation of [`Document`]; until that module is usable,
//! [`JsonDocument`] stands in: it stores the project JSON as the interface published it, without
//! collecting media and with only the structural checks below.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::write_atomically;
use crate::{Error, Result};

/// The project the host holds for the interface, and its files.
pub trait Document: Send + 'static {
    /// Called once at launch: the session to start with (the recovery copy of the previous
    /// session), or `None` for a new one. An unreadable recovery copy is an error whose message
    /// says where the copy was kept.
    fn startup(&mut self) -> Result<Option<Value>>;
    /// The project as last published, opened or saved.
    fn project(&self) -> Option<&Value>;
    /// The file the session was opened from or last saved to.
    fn path(&self) -> Option<&Path>;
    /// Whether the session differs from its file.
    fn dirty(&self) -> bool;
    /// Checks a project the interface sent before anything acts on it.
    fn validate(&self, project: &Value) -> Result<()>;
    /// Replaces the project with a newer publication. `mark_dirty` is false for publications that
    /// are not edits.
    fn update(&mut self, project: Value, mark_dirty: bool) -> Result<()>;
    /// Starts an untitled session. `None` leaves the project to the interface's blank one.
    fn start_new(&mut self, project: Option<Value>) -> Result<()>;
    /// Reads a project file without changing the session.
    fn read(&self, path: &Path) -> Result<Value>;
    /// Makes a project read from `path` the session.
    fn accept_opened(&mut self, project: Value, path: &Path);
    /// Writes the session to `path` and continues with it there. Returns the project as the session
    /// now holds it (media paths may have moved into the project's media folder).
    fn save(&mut self, path: &Path, collect_media: bool) -> Result<Value>;
    /// Writes the recovery copy.
    fn autosave(&mut self) -> Result<()>;
}

/// The stand-in [`Document`]: plain JSON files.
///
/// - Project files are the project JSON, pretty-printed, written atomically.
/// - The recovery copy is `Recovery.supertracker` in the support folder, with
///   `Recovery-session.json` recording the file it belongs to and whether it had unsaved changes.
///   The session reattaches to that file at the next launch only while the file is unchanged
///   (same size and modification time).
/// - Media is not collected; `collect_media` is ignored.
pub struct JsonDocument {
    support: PathBuf,
    project: Option<Value>,
    path: Option<PathBuf>,
    dirty: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct RecoveryRecord {
    dirty: bool,
    path: Option<PathBuf>,
    /// Size and modification time of `path` when the session last read or wrote it.
    fingerprint: Option<Fingerprint>,
    /// Size and modification time of the recovery copy this record describes.
    recovery: Option<Fingerprint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Fingerprint {
    bytes: u64,
    modified_nanos: u128,
}

fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Fingerprint { bytes: metadata.len(), modified_nanos: modified.as_nanos() })
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Projects larger than this are refused: nothing the interface produces comes close.
const MAX_PROJECT_BYTES: u64 = 100_000_000;

impl JsonDocument {
    /// A document that keeps its recovery copy in `support`.
    pub fn new(support: &Path) -> Self {
        JsonDocument { support: support.to_path_buf(), project: None, path: None, dirty: false }
    }

    fn recovery_path(&self) -> PathBuf {
        self.support.join("Recovery.supertracker")
    }

    fn record_path(&self) -> PathBuf {
        self.support.join("Recovery-session.json")
    }

    fn encode(project: &Value) -> Result<Vec<u8>> {
        serde_json::to_vec_pretty(project).map_err(|error| Error::new(format!("The project cannot be written: {error}.")))
    }
}

impl Document for JsonDocument {
    fn startup(&mut self) -> Result<Option<Value>> {
        let recovery = self.recovery_path();
        if !recovery.exists() {
            return Ok(None);
        }
        let restored = match self.read(&recovery) {
            Ok(project) => project,
            Err(error) => {
                // A blank session's first autosave would replace this file: keep it for manual recovery.
                let seconds = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or(0);
                let kept = self.support.join(format!("Recovery (unreadable {seconds}).supertracker"));
                let note = match fs::rename(&recovery, &kept) {
                    Ok(()) => format!(" The previous session was kept as {}.", kept.display()),
                    Err(_) => String::new(),
                };
                return Err(Error::new(format!("{error}{note}")));
            }
        };
        self.project = Some(restored.clone());
        self.path = None;
        self.dirty = true;
        let record: Option<RecoveryRecord> = fs::read(self.record_path()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if let Some(record) = record {
            let describes_this_copy = record.recovery.is_some() && record.recovery == fingerprint(&recovery);
            if let (true, Some(path), Some(expected)) = (describes_this_copy, record.path, record.fingerprint) {
                if fingerprint(&path) == Some(expected) {
                    self.path = Some(path);
                    self.dirty = record.dirty;
                }
            }
        }
        Ok(Some(restored))
    }

    fn project(&self) -> Option<&Value> {
        self.project.as_ref()
    }

    fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn dirty(&self) -> bool {
        self.dirty
    }

    fn validate(&self, project: &Value) -> Result<()> {
        let Some(fields) = project.as_object() else {
            return Err(Error::new("The project is not a JSON object."));
        };
        if let Some(version) = fields.get("version") {
            if version.as_i64() != Some(1) {
                return Err(Error::new("This project uses an unsupported format version."));
            }
        }
        let count = |name: &str| fields.get(name).and_then(Value::as_array).map(Vec::len);
        if count("tracks").unwrap_or(0) == 0 || count("instruments").is_none() || count("patterns").unwrap_or(0) == 0 {
            return Err(Error::new("The project must contain at least one track and one pattern, and a list of instruments."));
        }
        match fields.get("bpm").and_then(Value::as_f64) {
            Some(bpm) if (20.0..=400.0).contains(&bpm) => Ok(()),
            _ => Err(Error::new("Project tempo must be between 20 and 400 BPM.")),
        }
    }

    fn update(&mut self, project: Value, mark_dirty: bool) -> Result<()> {
        self.validate(&project)?;
        // The first publication of a new untitled session is its starting point, not an edit.
        let changed = self.project.as_ref().is_some_and(|current| *current != project);
        self.project = Some(project);
        self.dirty = mark_dirty && (self.dirty || changed);
        Ok(())
    }

    fn start_new(&mut self, project: Option<Value>) -> Result<()> {
        if let Some(project) = &project {
            self.validate(project)?;
        }
        self.project = project;
        self.path = None;
        self.dirty = false;
        Ok(())
    }

    fn read(&self, path: &Path) -> Result<Value> {
        let name = file_name(path);
        let metadata = fs::metadata(path).map_err(|error| Error::new(format!("Could not open {name}: {error}.")))?;
        if metadata.len() > MAX_PROJECT_BYTES {
            return Err(Error::new(format!("{name} is too large to open safely (100 MB limit).")));
        }
        let bytes = fs::read(path).map_err(|error| Error::new(format!("Could not open {name}: {error}.")))?;
        let project: Value = serde_json::from_slice(&bytes).map_err(|_| Error::new(format!("{name} is not a SuperTracker project.")))?;
        self.validate(&project).map_err(|error| Error::new(format!("{name} cannot be opened: {error}")))?;
        Ok(project)
    }

    fn accept_opened(&mut self, project: Value, path: &Path) {
        self.project = Some(project);
        self.path = Some(path.to_path_buf());
        self.dirty = false;
    }

    fn save(&mut self, path: &Path, _collect_media: bool) -> Result<Value> {
        let Some(project) = self.project.clone() else {
            return Err(Error::new("There is no project to save."));
        };
        self.validate(&project)?;
        let bytes = Self::encode(&project)?;
        write_atomically(path, &bytes).map_err(|error| Error::new(format!("Could not save {}: {error}.", file_name(path))))?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(project)
    }

    fn autosave(&mut self) -> Result<()> {
        let Some(project) = &self.project else {
            return Ok(());
        };
        self.validate(project)?;
        let recovery = self.recovery_path();
        let failed = |error: std::io::Error| Error::new(format!("Could not write the recovery copy in {}: {error}.", self.support.display()));
        fs::create_dir_all(&self.support).map_err(failed)?;
        write_atomically(&recovery, &Self::encode(project)?).map_err(failed)?;
        let record = RecoveryRecord {
            dirty: self.dirty,
            path: self.path.clone(),
            fingerprint: self.path.as_deref().and_then(fingerprint),
            recovery: fingerprint(&recovery),
        };
        let encoded = serde_json::to_vec_pretty(&record).map_err(|error| Error::new(format!("The recovery record cannot be written: {error}.")))?;
        write_atomically(&self.record_path(), &encoded).map_err(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use serde_json::json;

    pub(crate) fn project(name: &str) -> Value {
        json!({
            "version": 1, "name": name, "bpm": 120.0,
            "tracks": [{"name": "Track 01", "instrument": 0}],
            "instruments": [{"name": "Instrument 01", "type": "sample", "samplePath": ""}],
            "patterns": [{"id": 0, "length": 64, "steps": [[]]}],
            "sequence": [0]
        })
    }

    #[test]
    fn validation_names_what_is_wrong() {
        let document = JsonDocument::new(Path::new("/nonexistent"));
        assert!(document.validate(&project("ok")).is_ok());
        let mut fast = project("fast");
        fast["bpm"] = json!(999);
        assert_eq!(document.validate(&fast).unwrap_err().message(), "Project tempo must be between 20 and 400 BPM.");
        let mut future = project("future");
        future["version"] = json!(2);
        assert_eq!(document.validate(&future).unwrap_err().message(), "This project uses an unsupported format version.");
        assert!(document.validate(&json!({"bpm": 120, "tracks": [], "instruments": [], "patterns": [{}]})).is_err());
        assert!(document.validate(&json!([1, 2, 3])).is_err());
    }

    #[test]
    fn publications_mark_the_session_dirty_only_when_they_change_it() {
        let mut document = JsonDocument::new(Path::new("/nonexistent"));
        document.update(project("a"), true).unwrap();
        assert!(!document.dirty(), "the first publication of a new session is its starting point");
        document.update(project("a"), true).unwrap();
        assert!(!document.dirty());
        document.update(project("b"), false).unwrap();
        assert!(!document.dirty());
        document.update(project("c"), true).unwrap();
        assert!(document.dirty());
        assert!(document.update(json!({"bpm": 1}), true).is_err());
        assert_eq!(document.project().unwrap()["name"], "c", "a rejected publication leaves the session alone");
    }

    #[test]
    fn save_then_read_round_trips_and_reports_file_problems_by_name() {
        let folder = TempDir::new("document");
        let file = folder.path().join("Song.supertracker");
        let mut document = JsonDocument::new(folder.path());
        assert_eq!(document.save(&file, true).unwrap_err().message(), "There is no project to save.");
        document.update(project("first"), true).unwrap();
        document.update(project("Song"), true).unwrap();
        assert_eq!(document.save(&file, true).unwrap(), project("Song"));
        assert!(!document.dirty());
        assert_eq!(document.path(), Some(file.as_path()));
        assert_eq!(document.read(&file).unwrap(), project("Song"));

        let missing = document.read(&folder.path().join("Gone.supertracker")).unwrap_err();
        assert!(missing.message().starts_with("Could not open Gone.supertracker:"), "{missing}");
        let other = folder.path().join("notes.json");
        fs::write(&other, "[1, 2").unwrap();
        assert_eq!(document.read(&other).unwrap_err().message(), "notes.json is not a SuperTracker project.");
        fs::write(&other, r#"{"bpm": 5}"#).unwrap();
        assert!(document.read(&other).unwrap_err().message().starts_with("notes.json cannot be opened:"));
    }

    #[test]
    fn the_recovery_copy_restores_the_session_and_its_file_while_the_file_is_unchanged() {
        let folder = TempDir::new("recovery");
        let file = folder.path().join("Song.supertracker");
        let mut document = JsonDocument::new(folder.path());
        assert_eq!(document.startup().unwrap(), None);
        document.update(project("Song"), true).unwrap();
        document.save(&file, true).unwrap();
        document.update(project("Song, edited"), true).unwrap();
        document.autosave().unwrap();

        let mut relaunched = JsonDocument::new(folder.path());
        assert_eq!(relaunched.startup().unwrap(), Some(project("Song, edited")));
        assert_eq!(relaunched.path(), Some(file.as_path()));
        assert!(relaunched.dirty());

        // Saved and unchanged: the session comes back clean.
        document.save(&file, true).unwrap();
        document.autosave().unwrap();
        let mut clean = JsonDocument::new(folder.path());
        clean.startup().unwrap();
        assert!(!clean.dirty());

        // The file changed behind the session's back: untitled and unsaved.
        fs::write(&file, "changed elsewhere").unwrap();
        let mut detached = JsonDocument::new(folder.path());
        assert_eq!(detached.startup().unwrap(), Some(project("Song, edited")));
        assert_eq!(detached.path(), None);
        assert!(detached.dirty());
    }

    #[test]
    fn an_unreadable_recovery_copy_is_kept_aside_and_reported() {
        let folder = TempDir::new("recovery-broken");
        fs::write(folder.path().join("Recovery.supertracker"), "{ truncated").unwrap();
        let mut document = JsonDocument::new(folder.path());
        let error = document.startup().unwrap_err();
        assert!(error.message().contains("The previous session was kept as"), "{error}");
        assert!(!folder.path().join("Recovery.supertracker").exists());
        let kept = fs::read_dir(folder.path()).unwrap().filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().starts_with("Recovery (unreadable")).count();
        assert_eq!(kept, 1);
        assert_eq!(document.startup().unwrap(), None, "the next launch starts clean");
    }
}
