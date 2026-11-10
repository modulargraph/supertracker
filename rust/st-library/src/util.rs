//! Small shared helpers: text folding, path strings, base64, identifiers, atomic files and
//! the worker threads the services own.

use crate::error::{Error, Result};
use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------------------------
// Text

/// Case- and diacritic-insensitive form of `text`, used for every comparison a search makes.
///
/// Latin letters lose their accents (`é` -> `e`, `Ø` -> `o`, `ß` -> `ss`), combining marks are
/// dropped and everything is lower case. Other scripts are lower-cased only.
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii() {
            result.push(character.to_ascii_lowercase());
        } else if ('\u{0300}'..='\u{036f}').contains(&character) {
            // Combining diacritical marks of decomposed text.
        } else if let Some(base) = strip_diacritic(character) {
            result.push_str(base);
        } else {
            result.extend(character.to_lowercase());
        }
    }
    result
}

/// The unaccented lower-case base of an accented Latin letter (Latin-1 Supplement and Latin
/// Extended-A), or `None` for any other character.
fn strip_diacritic(character: char) -> Option<&'static str> {
    Some(match character {
        'À'..='Å' | 'à'..='å' | 'Ā'..='ą' => "a",
        'Æ' | 'æ' => "ae",
        'Ç' | 'ç' | 'Ć'..='č' => "c",
        'Ð' | 'ð' | 'Ď'..='đ' => "d",
        'È'..='Ë' | 'è'..='ë' | 'Ē'..='ě' => "e",
        'Ĝ'..='ģ' => "g",
        'Ĥ'..='ħ' => "h",
        'Ì'..='Ï' | 'ì'..='ï' | 'Ĩ'..='ı' => "i",
        'Ĵ' | 'ĵ' => "j",
        'Ķ' | 'ķ' => "k",
        'Ĺ'..='ł' => "l",
        'Ñ' | 'ñ' | 'Ń'..='ň' => "n",
        'Ò'..='Ö' | 'Ø' | 'ò'..='ö' | 'ø' | 'Ō'..='ő' => "o",
        'Œ' | 'œ' => "oe",
        'Ŕ'..='ř' => "r",
        'Ś'..='š' => "s",
        'ß' => "ss",
        'Ţ'..='ŧ' => "t",
        'Ù'..='Ü' | 'ù'..='ü' | 'Ũ'..='ų' => "u",
        'Ŵ' | 'ŵ' => "w",
        'Ý' | 'ý' | 'ÿ' | 'Ŷ'..='Ÿ' => "y",
        'Ź'..='ž' => "z",
        'Þ' | 'þ' => "th",
        _ => return None,
    })
}

/// Whether `haystack` contains `needle` (bytes). An empty needle is contained in everything.
pub fn bytes_contain(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else { return true };
    if haystack.len() < needle.len() {
        return false;
    }
    let last = haystack.len() - needle.len();
    let mut start = 0;
    while start <= last {
        match haystack[start..=last].iter().position(|&byte| byte == first) {
            None => return false,
            Some(offset) => {
                let at = start + offset;
                if haystack[at + 1..at + needle.len()] == *rest {
                    return true;
                }
                start = at + 1;
            }
        }
    }
    false
}

/// Orders text the way a file browser does: case-insensitively, with runs of digits compared
/// as numbers ("Kit 2" before "Kit 10"). Ties fall back to plain comparison.
pub fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering::Equal;
    let (a, b): (Vec<char>, Vec<char>) = (fold(left).chars().collect(), fold(right).chars().collect());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (start_i, start_j) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let trim = |digits: &[char]| -> Vec<char> { digits.iter().copied().skip_while(|c| *c == '0').collect() };
            let (x, y) = (trim(&a[start_i..i]), trim(&b[start_j..j]));
            let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
            if order != Equal {
                return order;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| left.cmp(right))
}

/// Case-insensitive ordering with a stable tie-break.
pub fn caseless_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    fold(left).cmp(&fold(right)).then_with(|| left.cmp(right))
}

// ---------------------------------------------------------------------------------------------
// Path strings. The page identifies files by their path text, so paths are `String`s here.

fn is_separator(character: char) -> bool {
    character == '/' || (cfg!(windows) && character == '\\')
}

/// The components of a path, without empty ones.
pub fn components(path: &str) -> Vec<&str> {
    path.split(is_separator).filter(|part| !part.is_empty()).collect()
}

/// The last component of a path.
pub fn file_name(path: &str) -> &str {
    path.trim_end_matches(is_separator).rsplit(is_separator).next().unwrap_or("")
}

/// The path without its last component (`/a/b.wav` -> `/a`).
pub fn parent(path: &str) -> &str {
    let trimmed = path.trim_end_matches(is_separator);
    match trimmed.rfind(is_separator) {
        Some(0) => &trimmed[..1],
        Some(index) => &trimmed[..index],
        None => "",
    }
}

/// The extension of a file name, lower case, without the dot. Empty when there is none.
pub fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(index) if index > 0 => name[index + 1..].to_lowercase(),
        _ => String::new(),
    }
}

/// The file name without its extension.
pub fn stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(index) if index > 0 => &name[..index],
        _ => name,
    }
}

/// `folder` joined with `name` by the platform's separator.
pub fn join(folder: &str, name: &str) -> String {
    if folder.ends_with(is_separator) {
        format!("{folder}{name}")
    } else {
        format!("{folder}{}{name}", std::path::MAIN_SEPARATOR)
    }
}

/// Whether `path` is `root` or lies below it.
pub fn is_inside(path: &str, root: &str) -> bool {
    if path == root {
        return true;
    }
    let root = root.trim_end_matches(is_separator);
    path.len() > root.len() && path.starts_with(root) && path[root.len()..].starts_with(is_separator)
}

/// The user's home folder, when the environment names one.
pub fn home_dir() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable).filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// An absolute, standardized path string: `~` expanded, relative paths made absolute against
/// the working directory, `.` and `..` resolved textually. Symbolic links are not resolved.
pub fn normalized(path: &str) -> String {
    let mut expanded = PathBuf::new();
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = home_dir() {
            expanded.push(home);
            expanded.push(path.trim_start_matches('~').trim_start_matches('/'));
        } else {
            expanded.push(path);
        }
    } else {
        expanded.push(path);
    }
    if !expanded.is_absolute() && !path.starts_with('/') {
        if let Ok(current) = std::env::current_dir() {
            expanded = current.join(expanded);
        }
    }
    let text = st_project::media::normalize(&expanded).to_string_lossy().into_owned();
    if text.len() > 1 && text.ends_with(is_separator) && !(cfg!(windows) && text.ends_with(":\\")) {
        text.trim_end_matches(is_separator).to_string()
    } else {
        text
    }
}

/// Whether `path` is an existing directory.
pub fn is_directory(path: &str) -> bool {
    fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false)
}

/// Whether `path` is a regular file that can be opened for reading.
pub fn is_readable_file(path: &Path) -> bool {
    fs::metadata(path).map(|metadata| metadata.is_file()).unwrap_or(false) && fs::File::open(path).is_ok()
}

/// The volume a path lives on when it is an external macOS volume (`/Volumes/<name>/...`).
pub fn volume_name(path: &str) -> Option<String> {
    let parts = components(path);
    (parts.len() >= 2 && parts[0] == "Volumes" && path.starts_with('/')).then(|| parts[1].to_string())
}

/// Modification time as seconds since 1970 (fractional).
pub fn modified_seconds(metadata: &fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

/// Whether two modification times are the same instant. Times that went through a JSON file
/// can differ in the last bit; a microsecond is far below what file systems distinguish reliably.
pub fn same_time(left: f64, right: f64) -> bool {
    (left - right).abs() < 1.0e-6
}

// ---------------------------------------------------------------------------------------------
// Base64 (standard alphabet, padded)

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 of `bytes`.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        text.push(BASE64[(value >> 18) as usize & 63] as char);
        text.push(BASE64[(value >> 12) as usize & 63] as char);
        text.push(if chunk.len() > 1 { BASE64[(value >> 6) as usize & 63] as char } else { '=' });
        text.push(if chunk.len() > 2 { BASE64[value as usize & 63] as char } else { '=' });
    }
    text
}

/// Decodes standard base64 (padding optional, white space ignored). `None` when malformed.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut result = Vec::with_capacity(text.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        buffer = buffer << 6 | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            result.push((buffer >> bits) as u8);
        }
    }
    Some(result)
}

// ---------------------------------------------------------------------------------------------
// Identifiers and time

/// A random identifier in UUID form (lower case). Not a version-4 UUID from a cryptographic
/// source: uniqueness within a library is all that is asked of it.
pub fn unique_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let word = |salt: u64| {
        // RandomState carries per-process random keys from the operating system.
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(nanos);
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u64(salt);
        hasher.write_u32(std::process::id());
        hasher.finish()
    };
    let (high, low) = (word(1), word(2));
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (high >> 32) as u32,
        (high >> 16) as u16,
        high as u16 & 0x0fff,
        (low >> 48) as u16 & 0x3fff | 0x8000,
        low & 0xffff_ffff_ffff
    )
}

/// The current time as an ISO 8601 UTC timestamp (`2026-01-31T12:00:00Z`).
pub fn iso8601_now() -> String {
    let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

// ---------------------------------------------------------------------------------------------
// Files

/// Writes `bytes` to `path` through a temporary file in the same folder and a rename, so a
/// reader never sees a partial file. Creates the folder when it is missing.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder).map_err(|error| Error::io("create", folder, &error))?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = folder.join(format!(".{name}.{}.tmp", unique_id()));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(Error::io("write", path, &error));
    }
    Ok(())
}

/// Reads a file of at most `limit` bytes. `None` when it is missing, larger or unreadable.
pub fn read_bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > limit {
        return None;
    }
    fs::read(path).ok()
}

// ---------------------------------------------------------------------------------------------
// Worker threads

type Job = Box<dyn FnOnce() + Send + 'static>;

enum Message {
    Run(Job),
    Stop,
}

/// A named thread that runs submitted jobs one at a time, in order.
///
/// The services own their workers: dropping a `Worker` lets the running job finish, discards
/// the jobs behind it and joins the thread.
pub struct Worker {
    sender: Mutex<Option<Sender<Message>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    idle: Arc<(Mutex<u64>, Condvar)>,
}

impl Worker {
    /// Starts the thread.
    pub fn new(name: &str) -> Worker {
        let (sender, receiver) = channel::<Message>();
        let idle = Arc::new((Mutex::new(0u64), Condvar::new()));
        let pending = idle.clone();
        let handle = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                while let Ok(Message::Run(job)) = receiver.recv() {
                    job();
                    let (count, signal) = &*pending;
                    *count.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) -= 1;
                    signal.notify_all();
                }
            })
            .ok();
        Worker { sender: Mutex::new(Some(sender)), handle: Mutex::new(handle), idle }
    }

    /// Queues `job`. Returns false when the worker has stopped (the job is dropped).
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> bool {
        let guard = self.sender.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(sender) = guard.as_ref() else { return false };
        *self.idle.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        if sender.send(Message::Run(Box::new(job))).is_err() {
            *self.idle.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) -= 1;
            return false;
        }
        true
    }

    /// Blocks until every job submitted so far has run.
    pub fn wait_idle(&self) {
        if self.is_current() {
            return;
        }
        let (count, signal) = &*self.idle;
        let mut pending = count.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while *pending > 0 {
            if self.handle.lock().map(|handle| handle.as_ref().is_none_or(|handle| handle.is_finished())).unwrap_or(true) {
                return;
            }
            pending = signal.wait_timeout(pending, std::time::Duration::from_millis(50)).unwrap_or_else(|poisoned| poisoned.into_inner()).0;
        }
    }

    /// Whether the calling thread is this worker's thread.
    pub fn is_current(&self) -> bool {
        self.handle
            .lock()
            .ok()
            .and_then(|handle| handle.as_ref().map(|handle| handle.thread().id() == std::thread::current().id()))
            .unwrap_or(false)
    }

    /// Stops the thread after its current job and joins it. Queued jobs are dropped.
    pub fn stop(&self) {
        let sender = self.sender.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(sender) = sender {
            let _ = sender.send(Message::Stop);
        }
        if self.is_current() {
            return;
        }
        let handle = self.handle.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Locks a mutex, continuing with the data when another thread panicked while holding it.
pub fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("Café DÉJÀ Vu"), "cafe deja vu");
        assert_eq!(fold("Bjørk Straße"), "bjork strasse");
        assert_eq!(fold("e\u{0301}"), "e");
        assert_eq!(fold("ПРИВЕТ"), "привет");
        assert!(bytes_contain(b"hello world", b"lo w") && !bytes_contain(b"hello", b"hellos") && bytes_contain(b"x", b""));
        assert!(bytes_contain(b"aaab", b"aab") && !bytes_contain(b"abab", b"abb"));
    }

    #[test]
    fn natural_order_compares_numbers() {
        let mut names = vec!["Kit 10", "kit 2", "Kit 1", "Alpha"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["Alpha", "Kit 1", "kit 2", "Kit 10"]);
    }

    #[test]
    fn path_helpers() {
        assert_eq!(components("/a/b//c.wav"), ["a", "b", "c.wav"]);
        assert_eq!((file_name("/a/b/c.WAV"), parent("/a/b/c.wav"), parent("/a"), stem("/a/My.Kit.stkit")), ("c.WAV", "/a/b", "/", "My.Kit"));
        assert_eq!((extension("/a/b/c.WAV"), extension("/a/.hidden"), extension("/a/none")), ("wav".to_string(), String::new(), String::new()));
        assert!(is_inside("/a/b/c", "/a/b") && is_inside("/a/b", "/a/b") && !is_inside("/a/bc", "/a/b") && is_inside("/x", "/"));
        assert_eq!(normalized("/a/./b/../c/"), "/a/c");
        assert_eq!(volume_name("/Volumes/Drive/Samples"), Some("Drive".to_string()));
        assert_eq!(volume_name("/Users/me"), None);
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0u8, 255, 128, 7]] {
            assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        }
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert!(base64_decode("a$b").is_none());
    }

    #[test]
    fn identifiers_and_time_are_well_formed() {
        let (a, b) = (unique_id(), unique_id());
        assert!(a != b && a.len() == 36 && a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        let now = iso8601_now();
        assert!(now.len() == 20 && now.ends_with('Z') && now.starts_with("20"));
    }

    #[test]
    fn workers_run_jobs_in_order() {
        let worker = Worker::new("test-worker");
        let seen = Arc::new(Mutex::new(Vec::new()));
        for index in 0..20 {
            let seen = seen.clone();
            worker.submit(move || seen.lock().unwrap().push(index));
        }
        worker.wait_idle();
        assert_eq!(*seen.lock().unwrap(), (0..20).collect::<Vec<_>>());
        worker.stop();
        assert!(!worker.submit(|| {}));
    }
}
