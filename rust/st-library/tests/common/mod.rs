//! Shared helpers of the integration tests: temporary folders and fixture files.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A folder under the system's temporary directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("st-library-{label}-{}-{}", std::process::id(), st_library::util::unique_id()));
        std::fs::create_dir_all(&path).unwrap();
        // Tests compare path text: use the canonical spelling (macOS /tmp is a link).
        TempDir(std::fs::canonicalize(&path).unwrap())
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn join(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
    /// The path of `relative` as text.
    pub fn text(&self, relative: &str) -> String {
        if relative.is_empty() {
            self.0.to_string_lossy().into_owned()
        } else {
            self.0.join(relative).to_string_lossy().into_owned()
        }
    }
    /// Writes a file, creating its folders.
    pub fn write(&self, relative: &str, bytes: &[u8]) -> String {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
    pub fn mkdir(&self, relative: &str) -> String {
        let path = self.0.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
