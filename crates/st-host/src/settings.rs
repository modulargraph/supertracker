//! Settings: one JSON file in the application's support folder.
//!
//! The file is an object with one entry per section. A section belongs to one area of the host,
//! which reads and writes it as a typed value:
//!
//! | Section | Owner | Contents |
//! |---|---|---|
//! | `window` | `st-app` | size, position and maximised state of the main window |
//! | `interface` | the page, through `settings.get` / `settings.set` | preferences the interface wants to keep across launches and transports |
//! | `audio.devices`, `midi.clock`, `mk3`, `agent`, `templates`, ... | the area's crate | that area's preferences |
//!
//! Every change is written at once, atomically (a temporary file renamed over the old one), so a
//! crash leaves either the previous or the new settings, never half a file.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{Error, Result};

/// The settings file. Share it as `Arc<Settings>`.
pub struct Settings {
    path: PathBuf,
    sections: Mutex<Map<String, Value>>,
    load_warning: Option<String>,
}

impl Settings {
    /// Opens `settings.json` in `support`, creating the folder if needed. A file that cannot be
    /// read as settings is kept under another name and replaced by defaults;
    /// [`load_warning`](Settings::load_warning) then says so.
    pub fn open(support: &Path) -> Result<Self> {
        fs::create_dir_all(support)
            .map_err(|error| Error::new(format!("Could not create the settings folder {}: {error}.", support.display())))?;
        let path = support.join("settings.json");
        let mut load_warning = None;
        let sections = match fs::read(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
            Err(error) => {
                return Err(Error::new(format!("Could not read the settings file {}: {error}.", path.display())));
            }
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Object(sections)) => sections,
                _ => {
                    let kept = support.join("settings (unreadable).json");
                    let kept_note = match fs::rename(&path, &kept) {
                        Ok(()) => format!(" The old file was kept as {}.", kept.display()),
                        Err(_) => String::new(),
                    };
                    load_warning = Some(format!("The settings file was unreadable, so settings start from their defaults.{kept_note}"));
                    Map::new()
                }
            },
        };
        Ok(Settings { path, sections: Mutex::new(sections), load_warning })
    }

    /// Set when the file on disk could not be used at startup.
    pub fn load_warning(&self) -> Option<&str> {
        self.load_warning.as_deref()
    }

    /// The file's location.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Map<String, Value>> {
        self.sections.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The section `name` as `T`. A missing section, and one that no longer fits `T` (written by a
    /// different version), read as `T::default()`.
    pub fn get<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        self.lock().get(name).cloned().and_then(|value| serde_json::from_value(value).ok()).unwrap_or_default()
    }

    /// The section `name` as stored, if present.
    pub fn get_raw(&self, name: &str) -> Option<Value> {
        self.lock().get(name).cloned()
    }

    /// Replaces the section `name` and writes the file.
    pub fn set<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let value = serde_json::to_value(value).map_err(|error| Error::new(format!("The setting {name} cannot be stored: {error}.")))?;
        let mut sections = self.lock();
        if sections.get(name) == Some(&value) {
            return Ok(());
        }
        let mut next = sections.clone();
        next.insert(name.to_string(), value);
        self.write(&next)?;
        *sections = next;
        Ok(())
    }

    /// Reads the section `name`, lets `change` modify it, and stores the result.
    pub fn update<T: DeserializeOwned + Default + Serialize>(&self, name: &str, change: impl FnOnce(&mut T)) -> Result<T> {
        let mut value: T = self.get(name);
        change(&mut value);
        self.set(name, &value)?;
        Ok(value)
    }

    /// Removes the section `name` and writes the file.
    pub fn remove(&self, name: &str) -> Result<()> {
        let mut sections = self.lock();
        if !sections.contains_key(name) {
            return Ok(());
        }
        let mut next = sections.clone();
        next.remove(name);
        self.write(&next)?;
        *sections = next;
        Ok(())
    }

    fn write(&self, sections: &Map<String, Value>) -> Result<()> {
        let text = serde_json::to_string_pretty(sections).map_err(|error| Error::new(format!("The settings cannot be stored: {error}.")))?;
        write_atomically(&self.path, text.as_bytes())
            .map_err(|error| Error::new(format!("Could not save the settings to {}: {error}.", self.path.display())))
    }
}

/// Writes `bytes` to `path` so that the file is either the old or the new content at any moment.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// The `interface` section: free-form values the page stores by key.
const INTERFACE: &str = "interface";

/// Payload of `settings.get`.
#[derive(Debug, Deserialize)]
pub struct GetSetting {
    /// The key; without it the result holds every interface setting.
    #[serde(default)]
    pub key: Option<String>,
}

/// Payload of `settings.set`.
#[derive(Debug, Deserialize)]
pub struct SetSetting {
    pub key: String,
    /// The value to store; `null` (or no value) removes the key.
    #[serde(default)]
    pub value: Value,
}

impl Settings {
    /// `settings.get`: `{key}` → `{value}` (null when unset), or `{}` → `{values: {...}}`.
    pub fn command_get(&self, payload: GetSetting) -> Result<Value> {
        let stored: Map<String, Value> = self.get(INTERFACE);
        Ok(match payload.key {
            Some(key) => serde_json::json!({ "value": stored.get(&key).cloned().unwrap_or(Value::Null) }),
            None => serde_json::json!({ "values": stored }),
        })
    }

    /// `settings.set`: `{key, value}` → `{}`.
    pub fn command_set(&self, payload: SetSetting) -> Result<Value> {
        if payload.key.is_empty() || payload.key.len() > 200 {
            return Err(Error::new("A setting needs a name of 1 to 200 characters."));
        }
        self.update(INTERFACE, |stored: &mut Map<String, Value>| {
            if payload.value.is_null() {
                stored.remove(&payload.key);
            } else {
                stored.insert(payload.key.clone(), payload.value.clone());
            }
        })?;
        Ok(serde_json::json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Window {
        width: u32,
        height: u32,
        maximized: bool,
    }

    #[test]
    fn sections_are_typed_default_when_missing_and_survive_reopening() {
        let folder = TempDir::new("settings");
        let settings = Settings::open(folder.path()).unwrap();
        assert_eq!(settings.get::<Window>("window"), Window::default());
        settings.set("window", &Window { width: 1500, height: 940, maximized: false }).unwrap();
        settings.update("window", |window: &mut Window| window.maximized = true).unwrap();
        drop(settings);

        let reopened = Settings::open(folder.path()).unwrap();
        assert_eq!(reopened.get::<Window>("window"), Window { width: 1500, height: 940, maximized: true });
        assert!(reopened.load_warning().is_none());
        reopened.remove("window").unwrap();
        assert_eq!(Settings::open(folder.path()).unwrap().get_raw("window"), None);
    }

    #[test]
    fn a_section_of_another_shape_reads_as_the_default_and_other_sections_are_kept() {
        let folder = TempDir::new("settings-shape");
        fs::write(folder.path().join("settings.json"), r#"{"window": "sideways", "mk3": {"enabled": true}}"#).unwrap();
        let settings = Settings::open(folder.path()).unwrap();
        assert_eq!(settings.get::<Window>("window"), Window::default());
        settings.set("window", &Window { width: 1, height: 2, maximized: false }).unwrap();
        assert_eq!(settings.get_raw("mk3"), Some(serde_json::json!({"enabled": true})));
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_and_reported() {
        let folder = TempDir::new("settings-broken");
        fs::write(folder.path().join("settings.json"), "{ not json").unwrap();
        let settings = Settings::open(folder.path()).unwrap();
        assert!(settings.load_warning().unwrap().contains("settings (unreadable).json"));
        assert_eq!(fs::read_to_string(folder.path().join("settings (unreadable).json")).unwrap(), "{ not json");
        settings.set("mk3", &serde_json::json!({"enabled": false})).unwrap();
        assert!(Settings::open(folder.path()).unwrap().load_warning().is_none());
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let folder = TempDir::new("settings-atomic");
        let settings = Settings::open(folder.path()).unwrap();
        settings.set("a", &1).unwrap();
        settings.set("b", &2).unwrap();
        let names: Vec<String> = fs::read_dir(folder.path()).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn the_interface_commands_store_values_by_key() {
        let folder = TempDir::new("settings-commands");
        let settings = Settings::open(folder.path()).unwrap();
        settings.command_set(SetSetting { key: "theme".into(), value: serde_json::json!("dark") }).unwrap();
        settings.command_set(SetSetting { key: "zoom".into(), value: serde_json::json!(1.25) }).unwrap();
        assert_eq!(settings.command_get(GetSetting { key: Some("theme".into()) }).unwrap(), serde_json::json!({"value": "dark"}));
        assert_eq!(settings.command_get(GetSetting { key: Some("missing".into()) }).unwrap(), serde_json::json!({"value": null}));
        settings.command_set(SetSetting { key: "zoom".into(), value: Value::Null }).unwrap();
        assert_eq!(settings.command_get(GetSetting { key: None }).unwrap(), serde_json::json!({"values": {"theme": "dark"}}));
        assert!(settings.command_set(SetSetting { key: String::new(), value: Value::Null }).is_err());
        // Interface settings live in their own section and cannot reach the host's.
        assert_eq!(settings.get_raw("theme"), None);
    }
}
