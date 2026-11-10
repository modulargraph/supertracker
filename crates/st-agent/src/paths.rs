//! Where the control socket lives. The application and its clients compute the same path from the
//! same environment.

use std::path::PathBuf;

/// The control socket's path for this process's environment. See [`socket_path_for`].
pub fn socket_path() -> PathBuf {
    socket_path_for(&|name| std::env::var(name).ok())
}

/// The control socket's path for an environment (`lookup` returns a variable's value).
///
/// - `SUPERTRACKER_CONTROL_SOCKET`, when it is an absolute path;
/// - otherwise `control.sock` in the support folder: `SUPERTRACKER_SUPPORT_DIR` when absolute, else
///   the platform's folder (the same rule as `st_host::support_dir`);
/// - a support folder too deep for a socket address falls back to
///   `SuperTracker-<hash of the support folder>.sock` in the user's temporary folder, so that both
///   sides still agree and different support folders stay apart.
///
/// On Windows (named pipes, not implemented yet) the result is the pipe name
/// `\\.\pipe\SuperTracker-<hash>`.
pub fn socket_path_for(lookup: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let absolute = |name: &str| lookup(name).map(PathBuf::from).filter(|path| path.is_absolute());
    if let Some(explicit) = absolute("SUPERTRACKER_CONTROL_SOCKET") {
        return explicit;
    }
    let support = absolute("SUPERTRACKER_SUPPORT_DIR").or_else(|| platform_support_dir(&absolute).map(|folder| folder.join("SuperTracker")));
    endpoint_in(support, &absolute)
}

#[cfg(not(windows))]
fn endpoint_in(support: Option<PathBuf>, absolute: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    let key = support.as_ref().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
    if let Some(support) = support {
        let preferred = support.join("control.sock");
        if preferred.as_os_str().len() < max_socket_path_bytes() {
            return preferred;
        }
    }
    user_temporary_dir(absolute).join(format!("SuperTracker-{}.sock", fnv1a(&key)))
}

#[cfg(windows)]
fn endpoint_in(support: Option<PathBuf>, _absolute: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    let key = support.map(|path| path.to_string_lossy().into_owned()).unwrap_or_default();
    PathBuf::from(format!(r"\\.\pipe\SuperTracker-{}", fnv1a(&key)))
}

/// The longest socket path the system accepts, counting the terminating zero byte: a usable path
/// is shorter than this.
pub fn max_socket_path_bytes() -> usize {
    #[cfg(unix)]
    {
        // SAFETY: an all-zero `sockaddr_un` is a valid value; only the size of one field is used.
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        std::mem::size_of_val(&address.sun_path)
    }
    #[cfg(not(unix))]
    {
        108
    }
}

#[cfg(target_os = "macos")]
fn platform_support_dir(absolute: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    absolute("HOME").map(|home| home.join("Library/Application Support"))
}

#[cfg(target_os = "windows")]
fn platform_support_dir(absolute: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    absolute("APPDATA")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_support_dir(absolute: &dyn Fn(&str) -> Option<PathBuf>) -> Option<PathBuf> {
    absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|home| home.join(".local/share")))
}

/// A folder that is the same for every process of this user, also when `TMPDIR` differs between
/// them (a sandboxed client and the application).
#[cfg(target_os = "macos")]
fn user_temporary_dir(_absolute: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: `confstr` writes at most `buffer.len()` bytes, zero-terminated, into the buffer.
    let length = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buffer.as_mut_ptr().cast(), buffer.len()) };
    if length > 1 && length <= buffer.len() {
        buffer.truncate(length - 1);
        if let Ok(text) = String::from_utf8(buffer) {
            return PathBuf::from(text);
        }
    }
    std::env::temp_dir()
}

#[cfg(all(not(target_os = "macos"), not(windows)))]
fn user_temporary_dir(absolute: &dyn Fn(&str) -> Option<PathBuf>) -> PathBuf {
    absolute("XDG_RUNTIME_DIR").unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// 64-bit FNV-1a of `text`, in lowercase hexadecimal.
pub fn fnv1a(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:x}")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn path(variables: &[(&str, &str)]) -> String {
        let map: HashMap<String, String> = variables.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
        socket_path_for(&|name| map.get(name).cloned()).to_string_lossy().into_owned()
    }

    #[test]
    fn explicit_socket_path_wins() {
        assert_eq!(path(&[("SUPERTRACKER_CONTROL_SOCKET", "/tmp/x.sock"), ("SUPERTRACKER_SUPPORT_DIR", "/Users/me/ST")]), "/tmp/x.sock");
        assert_eq!(path(&[("SUPERTRACKER_CONTROL_SOCKET", "relative.sock"), ("SUPERTRACKER_SUPPORT_DIR", "/Users/me/ST")]), "/Users/me/ST/control.sock");
    }

    #[test]
    fn support_folder_override() {
        assert_eq!(path(&[("SUPERTRACKER_SUPPORT_DIR", "/Users/me/ST")]), "/Users/me/ST/control.sock");
    }

    #[test]
    fn the_default_lives_in_the_support_folder() {
        let default = path(&[("HOME", "/home/me")]);
        #[cfg(target_os = "macos")]
        assert_eq!(default, "/home/me/Library/Application Support/SuperTracker/control.sock");
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(default, "/home/me/.local/share/SuperTracker/control.sock");
            assert_eq!(path(&[("HOME", "/home/me"), ("XDG_DATA_HOME", "/data")]), "/data/SuperTracker/control.sock");
        }
    }

    #[test]
    fn long_support_paths_fall_back_to_a_short_stable_path() {
        let long = format!("/private/tmp/{}", "deep/".repeat(30));
        let fallback = path(&[("SUPERTRACKER_SUPPORT_DIR", &long)]);
        assert!(fallback.len() < max_socket_path_bytes() && fallback.ends_with(".sock"), "{fallback}");
        assert_eq!(fallback, path(&[("SUPERTRACKER_SUPPORT_DIR", &long)]), "stable for the application and its clients");
        assert_ne!(fallback, path(&[("SUPERTRACKER_SUPPORT_DIR", &format!("{long}other/"))]), "distinct per support folder");
    }

    #[test]
    fn fnv1a_matches_the_reference_values() {
        assert_eq!(fnv1a(""), "cbf29ce484222325");
        assert_eq!(fnv1a("a"), "af63dc4c8601ec8c");
    }
}
