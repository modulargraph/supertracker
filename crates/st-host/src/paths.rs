//! Where the host keeps its files.

use std::env;
use std::path::PathBuf;

use crate::{Error, Result};

/// The application's support folder: settings, the recovery copy of the session, and (as their areas
/// arrive) templates, recordings, plug-in caches and the library index.
///
/// - `SUPERTRACKER_SUPPORT_DIR`, when set to an absolute path (tests and development sessions use
///   this to stay away from the user's files);
/// - macOS: `~/Library/Application Support/SuperTracker`;
/// - Windows: `%APPDATA%\SuperTracker`;
/// - elsewhere: `$XDG_DATA_HOME/SuperTracker`, or `~/.local/share/SuperTracker`.
pub fn support_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SUPERTRACKER_SUPPORT_DIR").map(PathBuf::from) {
        if path.is_absolute() {
            return Ok(path);
        }
        return Err(Error::new("SUPERTRACKER_SUPPORT_DIR must be an absolute path."));
    }
    platform_support_dir().map(|folder| folder.join("SuperTracker")).ok_or_else(|| {
        Error::new("The folder for SuperTracker's settings could not be determined (no home folder). Set SUPERTRACKER_SUPPORT_DIR.")
    })
}

fn absolute(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from).filter(|path| path.is_absolute())
}

#[cfg(target_os = "macos")]
fn platform_support_dir() -> Option<PathBuf> {
    absolute("HOME").map(|home| home.join("Library/Application Support"))
}

#[cfg(target_os = "windows")]
fn platform_support_dir() -> Option<PathBuf> {
    absolute("APPDATA")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_support_dir() -> Option<PathBuf> {
    absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|home| home.join(".local/share")))
}
