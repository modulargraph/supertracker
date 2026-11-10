//! Finding the installed plug-ins without loading any of them into the host.
//!
//! A [`Scanner`] walks the search folders of each format for plug-in files (plain file
//! system work, done in this process), and has every file that is new or changed listed
//! by a **sacrificial container process**: one process per file, asked with
//! `Request::Scan`, given a time limit, and killed when it exceeds it. A plug-in that
//! crashes or hangs while its factory is read therefore costs one helper process and is
//! remembered as such; the scan goes on with the next file.
//!
//! Results are plain [`PluginDescription`]s. They are kept, per file, in a cache keyed by
//! path, modification time and size, so an unchanged file is never scanned twice. Files
//! that failed stay in the cache with the reason: a file that crashed the scanner or
//! timed out is on the **blocklist** ([`Scanner::blocklist`]) and is not touched again
//! until it changes, [`Scanner::unblock`] is called for it, or a scan is run with
//! [`ScanOptions::retry_failed`].
//!
//! Audio Units are not files: the format `"au"` is one request for the system's component
//! registry (macOS), listed inside a container like everything else.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use stpc_transport::channel::{FrameKind, Stream};
use stpc_transport::messages::{PluginDescription, Request, Response};
use stpc_transport::wire::Wire;
use stpc_transport::wire_struct;

use crate::spawn;
use crate::{Error, Result};

/// First bytes of a cache file.
pub const CACHE_MAGIC: &[u8; 8] = b"STPCSCAN";
/// Version of the cache file format; a file of another version is ignored (rescanned).
pub const CACHE_VERSION: u32 = 1;
const CACHE_LIMIT: u64 = 256 << 20;

#[derive(Clone, Debug)]
pub struct ScannerConfig {
    /// The container executable, as [`crate::SupervisorConfig::container_path`].
    pub container_path: Option<PathBuf>,
    pub container_args: Vec<OsString>,
    pub container_env: Vec<(OsString, OsString)>,
    /// As [`crate::SupervisorConfig::socket_dir`].
    pub socket_dir: Option<PathBuf>,
    /// Where results are kept between runs. `None`: in memory only.
    pub cache_path: Option<PathBuf>,
    /// Formats to scan. Default: "vst3", "clap", and "au" on macOS.
    pub formats: Vec<String>,
    /// Search the standard folders of each format ([`standard_search_paths`]).
    pub standard_paths: bool,
    /// Folders (or single plug-in files) searched in addition, per format: the user's own.
    pub extra_paths: Vec<(String, PathBuf)>,
    /// Time one plug-in file gets. After it the scanning process is killed and the file
    /// goes on the blocklist.
    pub timeout: Duration,
    /// For a scanning process to start and complete the handshake.
    pub spawn_timeout: Duration,
    /// Scanning processes running at a time.
    pub jobs: usize,
}

impl Default for ScannerConfig {
    fn default() -> ScannerConfig {
        let mut formats = vec!["vst3".to_string(), "clap".to_string()];
        if cfg!(target_os = "macos") {
            formats.push("au".to_string());
        }
        ScannerConfig {
            container_path: None,
            container_args: Vec::new(),
            container_env: Vec::new(),
            socket_dir: None,
            cache_path: None,
            formats,
            standard_paths: true,
            extra_paths: Vec::new(),
            timeout: Duration::from_secs(30),
            spawn_timeout: Duration::from_secs(20),
            jobs: std::thread::available_parallelism().map_or(1, |n| n.get() / 2).clamp(1, 4),
        }
    }
}

/// The folders a format's plug-ins are installed in on this system, most specific (the
/// user's) first. Empty for formats without files (`"au"`).
///
/// * VST3: `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` (Linux);
///   `~/Library/Audio/Plug-Ins/VST3`, `/Library/Audio/Plug-Ins/VST3`,
///   `/Network/Library/Audio/Plug-Ins/VST3` (macOS).
/// * CLAP: `~/.clap`, `/usr/lib/clap` (Linux); `~/Library/Audio/Plug-Ins/CLAP`,
///   `/Library/Audio/Plug-Ins/CLAP` (macOS); then every entry of `$CLAP_PATH`.
pub fn standard_search_paths(format: &str) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute());
    let in_home = |relative: &str| home.as_ref().map(|home| home.join(relative));
    let mut paths: Vec<PathBuf> = Vec::new();
    match (format, cfg!(target_os = "macos")) {
        ("vst3", true) => {
            paths.extend(in_home("Library/Audio/Plug-Ins/VST3"));
            paths.push("/Library/Audio/Plug-Ins/VST3".into());
            paths.push("/Network/Library/Audio/Plug-Ins/VST3".into());
        }
        ("vst3", false) => {
            paths.extend(in_home(".vst3"));
            paths.push("/usr/lib/vst3".into());
            paths.push("/usr/local/lib/vst3".into());
        }
        ("clap", mac) => {
            if mac {
                paths.extend(in_home("Library/Audio/Plug-Ins/CLAP"));
                paths.push("/Library/Audio/Plug-Ins/CLAP".into());
            } else {
                paths.extend(in_home(".clap"));
                paths.push("/usr/lib/clap".into());
            }
            if let Some(extra) = std::env::var_os("CLAP_PATH") {
                paths.extend(std::env::split_paths(&extra).filter(|path| path.is_absolute()));
            }
        }
        _ => {}
    }
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths
}

/// File name extension of a format's plug-in files and bundles.
fn extension_of(format: &str) -> Option<&'static str> {
    match format {
        "vst3" => Some("vst3"),
        "clap" => Some("clap"),
        _ => None,
    }
}

/// What became of a plug-in file the last time it was scanned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanStatus {
    /// Scanned; its plug-ins are known (possibly none).
    Ok,
    /// The scanner answered with an error: not a plug-in, or it cannot be loaded.
    Failed,
    /// The scanning process died while reading it. Blocklisted.
    Crashed,
    /// The scanning process did not answer in time and was killed. Blocklisted.
    TimedOut,
}

impl ScanStatus {
    fn code(self) -> u32 {
        match self {
            ScanStatus::Ok => 0,
            ScanStatus::Failed => 1,
            ScanStatus::Crashed => 2,
            ScanStatus::TimedOut => 3,
        }
    }

    fn from_code(code: u32) -> Option<ScanStatus> {
        [ScanStatus::Ok, ScanStatus::Failed, ScanStatus::Crashed, ScanStatus::TimedOut]
            .into_iter()
            .find(|status| status.code() == code)
    }

    /// Crashed or timed out: on the blocklist.
    pub fn is_blocked(self) -> bool {
        matches!(self, ScanStatus::Crashed | ScanStatus::TimedOut)
    }
}

mod cache_format {
    use super::*;

    wire_struct! {
        /// One plug-in file in the cache file.
        pub struct CacheEntry {
            pub format: String,
            /// Empty for a registry format ("au").
            pub path: String,
            /// The fingerprint the file had when it was scanned.
            pub size: u64,
            pub mtime_ns: u64,
            /// `ScanStatus` code.
            pub status: u32,
            /// Why it failed (empty for status 0).
            pub reason: String,
            pub plugins: Vec<PluginDescription>,
        }
    }
}
use cache_format::CacheEntry;

/// One plug-in file (or registry) as the scanner knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedFile {
    pub format: String,
    pub path: PathBuf,
    pub status: ScanStatus,
    /// Why it failed; empty for [`ScanStatus::Ok`].
    pub reason: String,
    pub plugins: Vec<PluginDescription>,
}

impl ScannedFile {
    fn from_entry(entry: &CacheEntry) -> ScannedFile {
        ScannedFile {
            format: entry.format.clone(),
            path: PathBuf::from(&entry.path),
            status: ScanStatus::from_code(entry.status).unwrap_or(ScanStatus::Failed),
            reason: entry.reason.clone(),
            plugins: entry.plugins.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// Scan every file again, whatever the cache says.
    pub rescan_all: bool,
    /// Scan files again that failed, crashed or timed out before and have not changed.
    pub retry_failed: bool,
}

/// What happened to one file during a scan.
#[derive(Clone, Debug, PartialEq)]
pub enum FileOutcome {
    /// Unchanged since it was last scanned: taken from the cache.
    Cached {
        plugins: usize,
    },
    Scanned {
        plugins: usize,
    },
    /// The scanner answered with an error.
    Failed {
        reason: String,
    },
    /// The scanning process died; the file is now blocklisted.
    Crashed {
        reason: String,
    },
    /// The scanning process was killed after the time limit; the file is now blocklisted.
    TimedOut,
    /// Not scanned: it failed before and has not changed since.
    Skipped {
        status: ScanStatus,
        reason: String,
    },
    /// The scanning process could not be started (or the scan was cancelled under it).
    /// Nothing is recorded: the next scan tries again.
    NotScanned {
        reason: String,
    },
}

/// Progress of [`Scanner::scan`], delivered on the calling thread.
#[derive(Clone, Debug, PartialEq)]
pub enum ScanEvent<'a> {
    /// The search folders are read: `total` files will be looked at.
    Discovered { total: usize },
    /// A scanning process (`pid`) is running for this file.
    Started { format: &'a str, path: &'a Path, pid: u32 },
    /// One file is dealt with. `completed` counts up to `total`.
    Completed { completed: usize, total: usize, format: &'a str, path: &'a Path, outcome: &'a FileOutcome },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    /// Every plug-in known after the scan ([`Scanner::plugins`]).
    pub plugins: Vec<PluginDescription>,
    /// Files found in the search folders (plus one per registry format).
    pub files: usize,
    /// Files a scanning process was started for.
    pub scanned: usize,
    /// Files answered from the cache.
    pub cached: usize,
    /// Files that failed, crashed or timed out, in this scan or an earlier one.
    pub failures: Vec<ScannedFile>,
    /// The scan was cancelled; files not reached keep what the cache knew of them.
    pub cancelled: bool,
    /// Problems outside of plug-in files (a folder that cannot be read, a cache that
    /// cannot be written).
    pub warnings: Vec<String>,
}

struct Inner {
    config: ScannerConfig,
    container_path: PathBuf,
    dir: PathBuf,
    cache: Mutex<HashMap<(String, String), CacheEntry>>,
    cancel: AtomicBool,
    /// One scan at a time.
    scanning: Mutex<()>,
    next_socket: AtomicU32,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Lists the installed plug-ins by having short-lived container processes read the plug-in
/// files (see the crate documentation and `docs/PLUGIN_CONTAINER.md`). Cheap to clone; all
/// clones share the cache.
#[derive(Clone)]
pub struct Scanner {
    inner: Arc<Inner>,
}

struct Job {
    format: String,
    path: String,
    fingerprint: (u64, u64),
}

enum Raw {
    Plugins(Vec<PluginDescription>),
    Failed(String),
    Crashed(String),
    TimedOut,
    NotScanned(String),
}

enum Message {
    Started(usize, u32),
    Done(usize, Raw),
}

impl Scanner {
    /// Loads the cache (a missing, damaged or outdated cache file is an empty cache).
    pub fn new(config: ScannerConfig) -> Result<Scanner> {
        let container_path = spawn::resolve_container_path(config.container_path.as_ref())?;
        let dir = spawn::create_private_dir(config.socket_dir.as_ref())?;
        let cache = config.cache_path.as_deref().map(load_cache).unwrap_or_default();
        Ok(Scanner {
            inner: Arc::new(Inner {
                config,
                container_path,
                dir,
                cache: Mutex::new(cache),
                cancel: AtomicBool::new(false),
                scanning: Mutex::new(()),
                next_socket: AtomicU32::new(1),
            }),
        })
    }

    /// The folders searched for a format, in order of preference: the user's own
    /// ([`ScannerConfig::extra_paths`]) first, then the standard ones.
    pub fn search_paths(&self, format: &str) -> Vec<PathBuf> {
        let config = &self.inner.config;
        let mut paths: Vec<PathBuf> =
            config.extra_paths.iter().filter(|(f, _)| f == format).map(|(_, path)| path.clone()).collect();
        if config.standard_paths {
            paths.extend(standard_search_paths(format));
        }
        let mut seen = HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        paths
    }

    /// Stops a running [`Scanner::scan`] (from any thread): scanning processes are killed,
    /// the call returns with what it has. Files not reached keep their cached results.
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Release);
    }

    /// Brings the cache up to date with what is installed and returns the result. Blocks
    /// until done or cancelled; `progress` is called on this thread.
    pub fn scan(&self, options: ScanOptions, progress: &mut dyn FnMut(ScanEvent<'_>)) -> ScanReport {
        let inner = &*self.inner;
        let _one_at_a_time = inner.scanning.lock().unwrap();
        inner.cancel.store(false, Ordering::Release);
        let mut report = ScanReport::default();

        // 1. What is installed.
        let mut jobs: Vec<Job> = Vec::new();
        for format in &inner.config.formats {
            if extension_of(format).is_none() {
                // A registry format: one request, always asked again (it loads nothing).
                jobs.push(Job { format: format.clone(), path: String::new(), fingerprint: (0, 0) });
                continue;
            }
            for path in discover(format, &self.search_paths(format), &inner.cancel, &mut report.warnings) {
                let Some(fingerprint) = fingerprint(&path) else { continue };
                jobs.push(Job { format: format.clone(), path: path.to_string_lossy().into_owned(), fingerprint });
            }
        }
        let total = jobs.len();
        report.files = total;
        progress(ScanEvent::Discovered { total });

        // 2. What the cache already knows.
        let mut completed = 0;
        let mut pending: VecDeque<usize> = VecDeque::new();
        {
            let cache = inner.cache.lock().unwrap();
            for (index, job) in jobs.iter().enumerate() {
                let known = cache
                    .get(&(job.format.clone(), job.path.clone()))
                    .filter(|entry| !job.path.is_empty() && (entry.size, entry.mtime_ns) == job.fingerprint)
                    .filter(|_| !options.rescan_all);
                let outcome = match known.map(|entry| (ScanStatus::from_code(entry.status), entry)) {
                    Some((Some(ScanStatus::Ok), entry)) => FileOutcome::Cached { plugins: entry.plugins.len() },
                    Some((Some(status), entry)) if !options.retry_failed => {
                        FileOutcome::Skipped { status, reason: entry.reason.clone() }
                    }
                    _ => {
                        pending.push_back(index);
                        continue;
                    }
                };
                completed += 1;
                report.cached += matches!(outcome, FileOutcome::Cached { .. }) as usize;
                progress(ScanEvent::Completed {
                    completed,
                    total,
                    format: &job.format,
                    path: Path::new(&job.path),
                    outcome: &outcome,
                });
            }
        }

        // 3. Scan the rest, each file in a process of its own.
        let workers = inner.config.jobs.clamp(1, 16).min(pending.len());
        let queue = Mutex::new(pending);
        let mut unsaved = 0;
        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel::<Message>();
            for _ in 0..workers {
                let (sender, queue, jobs) = (sender.clone(), &queue, &jobs);
                scope.spawn(move || loop {
                    if inner.cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(index) = queue.lock().unwrap().pop_front() else { break };
                    let raw = inner.scan_one(&jobs[index].format, &jobs[index].path, &mut |pid| {
                        let _ = sender.send(Message::Started(index, pid));
                    });
                    let _ = sender.send(Message::Done(index, raw));
                });
            }
            drop(sender);
            for message in receiver {
                let (index, raw) = match message {
                    Message::Started(index, pid) => {
                        report.scanned += 1;
                        progress(ScanEvent::Started {
                            format: &jobs[index].format,
                            path: Path::new(&jobs[index].path),
                            pid,
                        });
                        continue;
                    }
                    Message::Done(index, raw) => (index, raw),
                };
                let job = &jobs[index];
                let outcome = inner.record(job, raw);
                completed += 1;
                progress(ScanEvent::Completed {
                    completed,
                    total,
                    format: &job.format,
                    path: Path::new(&job.path),
                    outcome: &outcome,
                });
                // A long first scan survives a host that is killed half way.
                unsaved += 1;
                if unsaved >= 16 {
                    unsaved = 0;
                    let _ = inner.save();
                }
            }
        });
        report.cancelled = inner.cancel.load(Ordering::Acquire);

        // 4. Forget what is no longer installed (a cancelled scan has not seen everything).
        if !report.cancelled {
            let present: HashSet<(&str, &str)> =
                jobs.iter().map(|job| (job.format.as_str(), job.path.as_str())).collect();
            let scanned_formats: HashSet<&str> = inner.config.formats.iter().map(String::as_str).collect();
            inner.cache.lock().unwrap().retain(|(format, path), _| {
                !scanned_formats.contains(format.as_str()) || present.contains(&(format.as_str(), path.as_str()))
            });
        }
        if let Err(error) = inner.save() {
            report.warnings.push(format!("the plug-in cache could not be saved: {error}"));
        }
        report.plugins = self.plugins();
        report.failures = self.files().into_iter().filter(|file| file.status != ScanStatus::Ok).collect();
        report
    }

    /// Scans one plug-in file now, whatever the cache and the blocklist say, and records
    /// the result. For a file the user points at, or to retry a blocklisted one.
    pub fn scan_file(&self, format: &str, path: &Path) -> Result<Vec<PluginDescription>> {
        let inner = &*self.inner;
        let _one_at_a_time = inner.scanning.lock().unwrap();
        inner.cancel.store(false, Ordering::Release);
        let Some(fingerprint) = fingerprint(path) else {
            return Err(Error::Invalid(format!("{} does not exist", path.display())));
        };
        let job = Job { format: format.to_string(), path: path.to_string_lossy().into_owned(), fingerprint };
        let outcome = inner.record(&job, inner.scan_one(format, &job.path, &mut |_| {}));
        let _ = inner.save();
        match outcome {
            FileOutcome::Scanned { .. } => {
                let cache = inner.cache.lock().unwrap();
                Ok(cache.get(&(job.format, job.path)).map(|entry| entry.plugins.clone()).unwrap_or_default())
            }
            FileOutcome::Failed { reason } | FileOutcome::Crashed { reason } | FileOutcome::NotScanned { reason } => {
                Err(Error::Plugin(reason))
            }
            FileOutcome::TimedOut => Err(Error::Timeout(inner.config.timeout)),
            FileOutcome::Cached { .. } | FileOutcome::Skipped { .. } => unreachable!("scan_file always scans"),
        }
    }

    /// Every file the scanner knows, good or bad, ordered by format and path.
    pub fn files(&self) -> Vec<ScannedFile> {
        let mut files: Vec<ScannedFile> =
            self.inner.cache.lock().unwrap().values().map(ScannedFile::from_entry).collect();
        files.sort_by(|a, b| (&a.format, &a.path).cmp(&(&b.format, &b.path)));
        files
    }

    /// The installed plug-ins, as of the last scan (or the loaded cache), ordered by name.
    /// A plug-in present in several files is listed once: from the folder that comes
    /// first in [`Scanner::search_paths`].
    pub fn plugins(&self) -> Vec<PluginDescription> {
        let roots: HashMap<&str, Vec<PathBuf>> =
            self.inner.config.formats.iter().map(|format| (format.as_str(), self.search_paths(format))).collect();
        let rank = |file: &ScannedFile| -> usize {
            let roots = roots.get(file.format.as_str()).map(Vec::as_slice).unwrap_or(&[]);
            roots.iter().position(|root| file.path.starts_with(root)).unwrap_or(roots.len())
        };
        let mut files = self.files();
        files.sort_by_cached_key(|file| (rank(file), file.path.clone()));
        let mut seen = HashSet::new();
        let mut plugins: Vec<PluginDescription> = files
            .into_iter()
            .filter(|file| file.status == ScanStatus::Ok)
            .flat_map(|file| file.plugins)
            .filter(|plugin| seen.insert((plugin.format.clone(), plugin.id.clone())))
            .collect();
        plugins.sort_by_cached_key(|plugin| (plugin.name.to_lowercase(), plugin.format.clone(), plugin.id.clone()));
        plugins
    }

    /// Files that crashed the scanner or timed out, with the reason. They are skipped by
    /// later scans until they change.
    pub fn blocklist(&self) -> Vec<ScannedFile> {
        self.files().into_iter().filter(|file| file.status.is_blocked()).collect()
    }

    /// Forgets what is known about a file, so that the next scan tries it again. True if
    /// there was something to forget.
    pub fn unblock(&self, format: &str, path: &Path) -> bool {
        let key = (format.to_string(), path.to_string_lossy().into_owned());
        let removed = self.inner.cache.lock().unwrap().remove(&key).is_some();
        if removed {
            let _ = self.inner.save();
        }
        removed
    }

    /// Forgets every failed file (blocklisted or not); the next scan tries them all again.
    pub fn clear_failures(&self) {
        self.inner.cache.lock().unwrap().retain(|_, entry| entry.status == ScanStatus::Ok.code());
        let _ = self.inner.save();
    }
}

impl Inner {
    /// Stores the result of scanning one file and says what it was.
    fn record(&self, job: &Job, raw: Raw) -> FileOutcome {
        let (status, reason, plugins, outcome) = match raw {
            Raw::Plugins(plugins) => {
                let count = plugins.len();
                (ScanStatus::Ok, String::new(), plugins, FileOutcome::Scanned { plugins: count })
            }
            Raw::Failed(reason) => (ScanStatus::Failed, reason.clone(), Vec::new(), FileOutcome::Failed { reason }),
            Raw::Crashed(reason) => (ScanStatus::Crashed, reason.clone(), Vec::new(), FileOutcome::Crashed { reason }),
            Raw::TimedOut => {
                let reason = format!("no answer within {:?}", self.config.timeout);
                (ScanStatus::TimedOut, reason, Vec::new(), FileOutcome::TimedOut)
            }
            Raw::NotScanned(reason) => return FileOutcome::NotScanned { reason },
        };
        let entry = CacheEntry {
            format: job.format.clone(),
            path: job.path.clone(),
            size: job.fingerprint.0,
            mtime_ns: job.fingerprint.1,
            status: status.code(),
            reason,
            plugins,
        };
        self.cache.lock().unwrap().insert((job.format.clone(), job.path.clone()), entry);
        outcome
    }

    /// Lists one file in a process of its own.
    fn scan_one(&self, format: &str, path: &str, started: &mut dyn FnMut(u32)) -> Raw {
        let socket = format!("scan-{}.sock", self.next_socket.fetch_add(1, Ordering::Relaxed));
        let launched = spawn::launch(&spawn::Launch {
            container_path: &self.container_path,
            args: &self.config.container_args,
            env: &self.config.container_env,
            dir: &self.dir,
            socket: &socket,
            timeout: self.config.spawn_timeout,
        });
        let (mut child, stream, _hello) = match launched {
            Ok(launched) => launched,
            Err(error) => return Raw::NotScanned(format!("the scanning process could not be started: {error}")),
        };
        started(child.id());
        let request = Request::Scan { format: format.to_string(), path: path.to_string() };
        let answer = if stream.send(FrameKind::Request, 1, &request.encode(), &[]).is_err() {
            None
        } else {
            self.await_answer(&stream)
        };
        match answer {
            Some(Ok(response)) => {
                // Let it leave in an orderly way (plug-ins may want their exit function);
                // what happens from here on no longer matters for the result.
                let _ = stream.send(FrameKind::Request, 2, &Request::Shutdown {}.encode(), &[]);
                reap(&mut child, Duration::from_secs(2));
                match response {
                    Response::Scan { plugins } => Raw::Plugins(plugins),
                    Response::Error { message, .. } => Raw::Failed(message),
                    other => Raw::Failed(format!("unexpected answer {}", other.name())),
                }
            }
            Some(Err(Waited::TimedOut)) => {
                reap(&mut child, Duration::ZERO);
                Raw::TimedOut
            }
            Some(Err(Waited::Cancelled)) => {
                reap(&mut child, Duration::ZERO);
                Raw::NotScanned("the scan was cancelled".into())
            }
            // The connection broke: the process died.
            None => Raw::Crashed(describe_exit(&mut child)),
        }
    }

    /// Waits for the response to request 1. `None`: the connection broke.
    fn await_answer(&self, stream: &Stream) -> Option<std::result::Result<Response, Waited>> {
        let deadline = Instant::now() + self.config.timeout;
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Some(Err(Waited::Cancelled));
            }
            let now = Instant::now();
            if now >= deadline {
                return Some(Err(Waited::TimedOut));
            }
            match stream.wait_readable((deadline - now).min(Duration::from_millis(20))) {
                Ok(false) => continue,
                Ok(true) => {}
                Err(_) => return None,
            }
            let frame = stream.recv().ok()?;
            if frame.kind == FrameKind::Response && frame.id == 1 {
                return Response::decode(&frame.payload).ok().map(Ok);
            }
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.config.cache_path else { return Ok(()) };
        let mut entries: Vec<CacheEntry> = self.cache.lock().unwrap().values().cloned().collect();
        entries.sort_by(|a, b| (&a.format, &a.path).cmp(&(&b.format, &b.path)));
        let mut bytes = CACHE_MAGIC.to_vec();
        bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&entries.encode());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Written beside the cache and renamed over it: a reader never sees half a file.
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(format!(".tmp-{}", std::process::id()));
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, &bytes)?;
        std::fs::rename(&temporary, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
    }
}

enum Waited {
    TimedOut,
    Cancelled,
}

/// Waits `grace` for the process to exit by itself, then kills it; reaps it either way.
fn reap(child: &mut Child, grace: Duration) {
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// How a scanning process that broke the connection ended.
fn describe_exit(child: &mut Child) -> String {
    // It is dead or dying; a process that merely closed its socket is made dead.
    let deadline = Instant::now() + Duration::from_millis(500);
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
        let _ = child.wait();
        return "the scanning process closed the connection".into();
    }
    match child.wait() {
        Ok(status) => match (status.signal(), status.code()) {
            (Some(signal), _) => format!("the scanning process crashed (signal {signal}{})", signal_name(signal)),
            (None, Some(code)) => format!("the scanning process exited with code {code}"),
            _ => "the scanning process ended".into(),
        },
        Err(error) => format!("the scanning process ended ({error})"),
    }
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        libc::SIGSEGV => ", SIGSEGV",
        libc::SIGABRT => ", SIGABRT",
        libc::SIGBUS => ", SIGBUS",
        libc::SIGILL => ", SIGILL",
        libc::SIGFPE => ", SIGFPE",
        libc::SIGKILL => ", SIGKILL",
        libc::SIGTRAP => ", SIGTRAP",
        _ => "",
    }
}

fn load_cache(path: &Path) -> HashMap<(String, String), CacheEntry> {
    let read = || -> Option<Vec<CacheEntry>> {
        if std::fs::metadata(path).ok()?.len() > CACHE_LIMIT {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() < 12 || &bytes[..8] != CACHE_MAGIC || bytes[8..12] != CACHE_VERSION.to_le_bytes() {
            return None;
        }
        Vec::<CacheEntry>::decode(&bytes[12..]).ok()
    };
    read()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| ScanStatus::from_code(entry.status).is_some())
        .map(|entry| ((entry.format.clone(), entry.path.clone()), entry))
        .collect()
}

/// Size and modification time (ns since the epoch) that change when a plug-in is replaced.
/// For a bundle directory: summed over the files in `Contents` and its direct
/// sub-directories (the binaries, `Info.plist`, `moduleinfo.json`), so that replacing the
/// binary inside an existing bundle is noticed. `None`: the path does not exist.
pub fn fingerprint(path: &Path) -> Option<(u64, u64)> {
    let modified = |metadata: &std::fs::Metadata| -> u64 {
        metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64)
    };
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_dir() {
        return Some((metadata.len(), modified(&metadata)));
    }
    let (mut size, mut newest, mut budget) = (0u64, modified(&metadata), 4096usize);
    let contents = path.join("Contents");
    let top = if contents.is_dir() { contents } else { path.to_path_buf() };
    let mut visit = |dir: &Path, descend: bool, pending: &mut Vec<PathBuf>| {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if budget == 0 {
                return;
            }
            budget -= 1;
            let Ok(metadata) = std::fs::metadata(entry.path()) else { continue };
            if metadata.is_dir() {
                if descend {
                    pending.push(entry.path());
                }
            } else {
                size = size.wrapping_add(metadata.len());
                newest = newest.max(modified(&metadata));
            }
        }
    };
    let mut sub_directories = Vec::new();
    visit(&top, true, &mut sub_directories);
    sub_directories.sort();
    for directory in sub_directories {
        visit(&directory, false, &mut Vec::new());
    }
    Some((size, newest))
}

/// The plug-in files of a format under `roots`: every file or directory whose name has
/// the format's extension (bundles are not looked into), in the order of the roots, then
/// by path. Hidden entries are skipped; links are followed.
fn discover(format: &str, roots: &[PathBuf], cancel: &AtomicBool, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let Some(extension) = extension_of(format) else { return Vec::new() };
    let is_plugin =
        |path: &Path| path.extension().is_some_and(|found| found.to_string_lossy().eq_ignore_ascii_case(extension));
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut budget = 200_000usize;
    for root in roots {
        if !root.exists() {
            continue;
        }
        if is_plugin(root) {
            if seen.insert(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone())) {
                found.push(root.clone());
            }
            continue;
        }
        let mut in_root: Vec<PathBuf> = Vec::new();
        let mut stack = vec![(root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    warnings.push(format!("cannot read {}: {error}", dir.display()));
                    continue;
                }
            };
            for entry in entries.flatten() {
                if budget == 0 {
                    warnings.push(format!("{} holds too many files; the search stopped early", root.display()));
                    stack.clear();
                    break;
                }
                budget -= 1;
                let path = entry.path();
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if is_plugin(&path) {
                    // The same bundle reached twice (a link, overlapping roots) counts once.
                    if seen.insert(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone())) {
                        in_root.push(path);
                    }
                } else if depth < 8 && path.is_dir() {
                    stack.push((path, depth + 1));
                }
            }
        }
        in_root.sort();
        found.extend(in_root);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stpc-scanner-unit-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discovery_finds_files_and_bundles_in_root_order() {
        let dir = scratch("discover");
        let (user, system) = (dir.join("user"), dir.join("system"));
        std::fs::create_dir_all(user.join("Vendor/Deep.vst3/Contents/x86_64-linux")).unwrap();
        std::fs::write(user.join("Vendor/Deep.vst3/Contents/x86_64-linux/Deep.so"), b"x").unwrap();
        // Not looked into: a bundle inside a bundle is not a second plug-in.
        std::fs::create_dir_all(user.join("Vendor/Deep.vst3/Contents/Nested.vst3")).unwrap();
        std::fs::write(user.join("b.VST3"), b"single file").unwrap();
        std::fs::write(user.join("notes.txt"), b"x").unwrap();
        std::fs::write(user.join("._hidden.vst3"), b"x").unwrap();
        std::fs::write(user.join("other.clap"), b"x").unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("a.vst3"), b"x").unwrap();
        std::os::unix::fs::symlink(user.join("b.VST3"), system.join("link.vst3")).unwrap();
        let cancel = AtomicBool::new(false);
        let mut warnings = Vec::new();
        let roots = [user.clone(), system.clone(), dir.join("missing"), user.clone()];
        let found = discover("vst3", &roots, &cancel, &mut warnings);
        assert_eq!(found, [user.join("Vendor/Deep.vst3"), user.join("b.VST3"), system.join("a.vst3")]);
        assert_eq!(discover("clap", &roots, &cancel, &mut warnings), [user.join("other.clap")]);
        // A root may be a plug-in itself (a file the user picked).
        assert_eq!(discover("vst3", &[system.join("a.vst3")], &cancel, &mut warnings), [system.join("a.vst3")]);
        assert!(discover("au", &roots, &cancel, &mut warnings).is_empty());
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fingerprint_follows_the_binary_inside_a_bundle() {
        let dir = scratch("fingerprint");
        let bundle = dir.join("P.vst3");
        let binary = bundle.join("Contents/x86_64-linux/P.so");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, b"one").unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), b"plist").unwrap();
        let first = fingerprint(&bundle).unwrap();
        assert_eq!(first.0, 8);
        assert_eq!(fingerprint(&bundle), Some(first), "stable");
        std::fs::write(&binary, b"three").unwrap();
        assert_ne!(fingerprint(&bundle), Some(first), "the binary changed inside the same bundle");
        assert_eq!(fingerprint(&binary).unwrap().0, 5);
        assert_eq!(fingerprint(&dir.join("missing")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_files_round_trip_and_bad_ones_are_ignored() {
        let dir = scratch("cache");
        let path = dir.join("sub/plugins.cache");
        let entry = |name: &str, status: ScanStatus| CacheEntry {
            format: "vst3".into(),
            path: format!("/plugins/{name}.vst3"),
            size: 123,
            mtime_ns: 456,
            status: status.code(),
            reason: if status == ScanStatus::Ok { String::new() } else { "it crashed".into() },
            plugins: vec![PluginDescription { format: "vst3".into(), id: name.into(), ..PluginDescription::default() }],
        };
        let inner = Inner {
            config: ScannerConfig { cache_path: Some(path.clone()), ..ScannerConfig::default() },
            container_path: PathBuf::new(),
            dir: dir.join("sockets"),
            cache: Mutex::new(HashMap::new()),
            cancel: AtomicBool::new(false),
            scanning: Mutex::new(()),
            next_socket: AtomicU32::new(1),
        };
        for item in [entry("a", ScanStatus::Ok), entry("b", ScanStatus::Crashed)] {
            inner.cache.lock().unwrap().insert((item.format.clone(), item.path.clone()), item);
        }
        inner.save().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!((&bytes[..8], &bytes[8..12]), (&CACHE_MAGIC[..], &1u32.to_le_bytes()[..]));
        let loaded = load_cache(&path);
        assert_eq!(loaded, *inner.cache.lock().unwrap());
        // Truncated, of another version, or not a cache at all: an empty cache, no error.
        std::fs::write(&path, &bytes[..bytes.len() - 3]).unwrap();
        assert!(load_cache(&path).is_empty());
        let mut newer = bytes.clone();
        newer[8] = 2;
        std::fs::write(&path, &newer).unwrap();
        assert!(load_cache(&path).is_empty());
        std::fs::write(&path, b"hello").unwrap();
        assert!(load_cache(&path).is_empty());
        assert!(load_cache(&dir.join("missing")).is_empty());
        drop(inner);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn standard_paths_follow_the_platform_conventions() {
        let vst3 = standard_search_paths("vst3");
        let clap = standard_search_paths("clap");
        if cfg!(target_os = "macos") {
            assert!(vst3.contains(&PathBuf::from("/Library/Audio/Plug-Ins/VST3")));
            assert!(clap.contains(&PathBuf::from("/Library/Audio/Plug-Ins/CLAP")));
        } else {
            assert!(
                vst3.contains(&PathBuf::from("/usr/lib/vst3")) && vst3.contains(&PathBuf::from("/usr/local/lib/vst3"))
            );
            assert!(clap.contains(&PathBuf::from("/usr/lib/clap")));
            if let Some(home) = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute()) {
                assert_eq!((&vst3[0], &clap[0]), (&home.join(".vst3"), &home.join(".clap")), "the user's folder first");
            }
        }
        assert!(standard_search_paths("au").is_empty());
    }
}
