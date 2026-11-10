//! Where the app keeps files it creates on its own: sample edits and automatic recordings.
//!
//! The functions are pure (the caller passes the environment value and the platform's
//! application-support folder), so development and tests never spill into the user's folder
//! when a private session root is configured.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{Result, SampleError};

/// Environment variable that redirects all app-owned files to a private folder. Only absolute
/// paths are honoured.
pub const SUPPORT_DIR_ENV: &str = "SUPERTRACKER_SUPPORT_DIR";

/// The app's support folder: `support_override` (the value of [`SUPPORT_DIR_ENV`]) when it is an
/// absolute path, otherwise `<application_support>/SuperTracker`.
pub fn support_directory(support_override: Option<&str>, application_support: &Path) -> PathBuf {
    match support_override {
        Some(value) if value.starts_with('/') => PathBuf::from(value),
        _ => application_support.join("SuperTracker"),
    }
}

/// The folder a sample edit is written to: an explicit `output_directory` first, otherwise
/// `Edits` in the support folder.
pub fn edit_directory(output_directory: Option<&str>, support_override: Option<&str>, application_support: &Path) -> PathBuf {
    match output_directory {
        Some(path) => PathBuf::from(path),
        None => support_directory(support_override, application_support).join("Edits"),
    }
}

/// Where a recording is written. An explicit non-empty `path` is used as it is. Otherwise the
/// file goes to `Recordings` in the support folder (created if needed) under a sanitised form
/// of `name` (letters, digits, spaces, `-` and `_`; at most 80 characters; "Recording" when
/// nothing is left) followed by a short unique suffix and `.wav`.
pub fn recording_destination(path: Option<&str>, name: Option<&str>, support_override: Option<&str>, application_support: &Path) -> Result<PathBuf> {
    if let Some(path) = path.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let base = support_directory(support_override, application_support).join("Recordings");
    std::fs::create_dir_all(&base).map_err(|e| SampleError::io("Create the recordings folder", &e))?;
    // Spaces of any kind and tabs are kept; line breaks are not.
    let space = |c: char| c.is_whitespace() && !matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\u{85}' | '\u{2028}' | '\u{2029}');
    let kept: String = name.unwrap_or("Recording").chars().filter(|&c| c.is_alphanumeric() || space(c) || c == '-' || c == '_').take(80).collect();
    let safe = kept.trim();
    Ok(base.join(format!("{}-{}.wav", if safe.is_empty() { "Recording" } else { safe }, unique_suffix())))
}

/// Eight hexadecimal digits that differ between calls and between runs. Not a security token:
/// it only keeps file names from colliding.
pub fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    // RandomState is seeded from the operating system once per process.
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_u32(std::process::id());
    if let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        hasher.write_u128(elapsed.as_nanos());
    }
    format!("{:08X}", hasher.finish() as u32)
}

/// Thirty-two hexadecimal digits, for private folder names that must not be guessable by
/// accident (two exports into one folder never share a staging area).
pub(crate) fn unique_token() -> String {
    format!("{}{}{}{}", unique_suffix(), unique_suffix(), unique_suffix(), unique_suffix())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_folder_policy() {
        let support = Path::new("/synthetic/application-support");
        assert_eq!(edit_directory(None, Some("/synthetic/private"), support), Path::new("/synthetic/private/Edits"));
        assert_eq!(edit_directory(Some("/synthetic/explicit"), Some("/synthetic/private"), support), Path::new("/synthetic/explicit"));
        for environment in [None, Some("relative")] {
            assert_eq!(edit_directory(None, environment, support), Path::new("/synthetic/application-support/SuperTracker/Edits"));
        }
    }

    #[test]
    fn recording_destinations() {
        let root = std::env::temp_dir().join(format!("st-samples-recording-{}-{}", std::process::id(), unique_suffix()));
        let private = root.join("private-session");
        let fallback = root.join("fallback-support");
        let captured = recording_destination(None, Some("  ../../Take:* A  "), private.to_str(), &fallback).unwrap();
        assert_eq!(captured.parent().unwrap(), private.join("Recordings"));
        let leaf = captured.file_name().unwrap().to_str().unwrap();
        assert!(leaf.starts_with("Take A-") && leaf.ends_with(".wav"), "sanitised name: {leaf}");
        assert!(private.join("Recordings").is_dir());
        assert!(!fallback.exists(), "resolving a private recording must not touch the default support root");

        let explicit = root.join("chosen.wav");
        assert_eq!(recording_destination(explicit.to_str(), Some("x"), private.to_str(), &fallback).unwrap(), explicit);

        let defaulted = recording_destination(None, Some("///"), None, &fallback).unwrap();
        assert_eq!(defaulted.parent().unwrap(), fallback.join("SuperTracker/Recordings"));
        assert!(defaulted.file_name().unwrap().to_str().unwrap().starts_with("Recording-"));
        let relative = recording_destination(Some(""), None, Some("relative"), &fallback).unwrap();
        assert_eq!(relative.parent(), defaulted.parent(), "a relative override is ignored");

        let long = recording_destination(None, Some(&"a".repeat(100)), private.to_str(), &fallback).unwrap();
        assert_eq!(long.file_stem().unwrap().to_str().unwrap().chars().count(), 89, "80 characters, a dash and 8 digits");
        let unicode = recording_destination(None, Some("Größe_été 日本-1"), private.to_str(), &fallback).unwrap();
        assert!(unicode.file_name().unwrap().to_str().unwrap().starts_with("Größe_été 日本-1-"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn suffixes_differ() {
        let all: std::collections::HashSet<String> = (0..200).map(|_| unique_suffix()).collect();
        assert!(all.len() >= 199);
        assert!(all.iter().all(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_hexdigit())));
    }
}
