//! The catalogue of installed plug-ins: the client's [`Scanner`] with the host's folders, the
//! cache file and one scan at a time.
//!
//! Plug-in files are read by short-lived container processes, never by the host: a plug-in
//! that crashes or hangs while being listed costs that process and lands on the blocklist.
//! [`Catalog::plugins`] answers from the cache (it starts nothing); [`Catalog::scan`] walks
//! the folders, scans what is new or changed and reports progress.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use stpc_client::{FileOutcome, PluginDescription, ScanEvent, ScanOptions, ScanStatus, Scanner, ScannerConfig};

/// How the catalogue is set up.
#[derive(Debug, Clone, Default)]
pub struct CatalogOptions {
    /// The container executable (default: as the supervisor finds it).
    pub container_path: Option<PathBuf>,
    pub socket_dir: Option<PathBuf>,
    /// Where scan results are kept between runs.
    pub cache_path: Option<PathBuf>,
    /// The user's own plug-in folders (or single plug-in files), searched before the
    /// standard ones, for every format.
    pub folders: Vec<PathBuf>,
    /// Whether the standard folders of each format are searched.
    pub standard_folders: bool,
    /// Seconds one plug-in file gets before it is given up and blocklisted (default 30).
    pub timeout: Option<Duration>,
}

/// A plug-in file that is not listed: why.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Blocked {
    pub format: String,
    pub path: String,
    /// `failed` (not a plug-in, or it cannot be loaded), `crashed` or `timedOut` (blocklisted).
    pub status: &'static str,
    pub reason: String,
    pub blocklisted: bool,
}

/// Progress of a scan, as the `plugins.catalogue` event carries it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanning: bool,
    pub completed: usize,
    pub total: usize,
    /// The file being read or just finished.
    pub path: String,
    pub scanned: usize,
    pub cached: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub plugins: usize,
    pub warnings: Vec<String>,
}

pub struct Catalog {
    scanner: Mutex<Scanner>,
    options: Mutex<CatalogOptions>,
    scanning: AtomicBool,
    last: Mutex<ScanProgress>,
}

fn scanner(options: &CatalogOptions) -> Result<Scanner, String> {
    let defaults = ScannerConfig::default();
    let extra_paths = defaults.formats.iter().filter(|format| *format != "au").flat_map(|format| options.folders.iter().map(move |folder| (format.clone(), folder.clone()))).collect();
    Scanner::new(ScannerConfig {
        container_path: options.container_path.clone(),
        socket_dir: options.socket_dir.clone(),
        cache_path: options.cache_path.clone(),
        standard_paths: options.standard_folders,
        extra_paths,
        timeout: options.timeout.unwrap_or(defaults.timeout),
        ..defaults
    })
    .map_err(|error| format!("The plug-in scanner could not start: {error}"))
}

impl Catalog {
    /// Loads the cache. Starts no process.
    pub fn new(options: CatalogOptions) -> Result<Catalog, String> {
        Ok(Catalog { scanner: Mutex::new(scanner(&options)?), options: Mutex::new(options), scanning: AtomicBool::new(false), last: Mutex::new(ScanProgress::default()) })
    }

    fn scanner(&self) -> Scanner {
        self.scanner.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The user's own folders.
    pub fn folders(&self) -> Vec<PathBuf> {
        self.options.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).folders.clone()
    }

    /// Replaces the user's own folders (the next scan searches them).
    pub fn set_folders(&self, folders: Vec<PathBuf>) -> Result<(), String> {
        let mut options = self.options.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if options.folders == folders {
            return Ok(());
        }
        let next = CatalogOptions { folders, ..options.clone() };
        *self.scanner.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = scanner(&next)?;
        *options = next;
        Ok(())
    }

    /// Every plug-in known from the last scan (or the cache), by name.
    pub fn plugins(&self) -> Vec<PluginDescription> {
        self.scanner().plugins()
    }

    /// The installed plug-in with this format and id.
    pub fn find(&self, format: &str, id: &str) -> Option<PluginDescription> {
        self.plugins().into_iter().find(|plugin| plugin.format == format && plugin.id == id)
    }

    /// Files that are not listed: failed ones and the blocklist.
    pub fn blocked(&self) -> Vec<Blocked> {
        self.scanner()
            .files()
            .into_iter()
            .filter(|file| file.status != ScanStatus::Ok)
            .map(|file| Blocked {
                format: file.format,
                path: file.path.to_string_lossy().into_owned(),
                status: match file.status {
                    ScanStatus::Crashed => "crashed",
                    ScanStatus::TimedOut => "timedOut",
                    _ => "failed",
                },
                reason: file.reason,
                blocklisted: file.status.is_blocked(),
            })
            .collect()
    }

    pub fn scanning(&self) -> bool {
        self.scanning.load(Ordering::Acquire)
    }

    /// The progress of the running scan, or the outcome of the last one.
    pub fn progress(&self) -> ScanProgress {
        self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Stops the running scan.
    pub fn cancel(&self) {
        self.scanner().cancel();
    }

    /// Scans the folders (blocking; one scan at a time: `None` when one is running).
    /// `rescan` reads every file again whatever the cache says; `retry_failed` gives failed
    /// and blocklisted files another try. `progress` is called on this thread.
    pub fn scan(&self, rescan: bool, retry_failed: bool, progress: &mut dyn FnMut(&ScanProgress)) -> Option<ScanProgress> {
        if self.scanning.swap(true, Ordering::AcqRel) {
            return None;
        }
        let scanner = self.scanner();
        let mut state = ScanProgress { scanning: true, ..ScanProgress::default() };
        let store = |state: &ScanProgress| *self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = state.clone();
        store(&state);
        progress(&state);
        let report = scanner.scan(ScanOptions { rescan_all: rescan, retry_failed }, &mut |event| {
            match event {
                ScanEvent::Discovered { total } => state.total = total,
                ScanEvent::Started { path, .. } => state.path = path.to_string_lossy().into_owned(),
                ScanEvent::Completed { completed, total, path, outcome, .. } => {
                    state.completed = completed;
                    state.total = total;
                    state.path = path.to_string_lossy().into_owned();
                    match outcome {
                        FileOutcome::Cached { .. } => state.cached += 1,
                        FileOutcome::Scanned { .. } => state.scanned += 1,
                        _ => state.failed += 1,
                    }
                }
            }
            store(&state);
            progress(&state);
        });
        let done = ScanProgress {
            scanning: false,
            completed: state.completed.max(report.files),
            total: report.files,
            path: String::new(),
            scanned: report.scanned,
            cached: report.cached,
            failed: report.failures.len(),
            cancelled: report.cancelled,
            plugins: report.plugins.len(),
            warnings: report.warnings,
        };
        store(&done);
        self.scanning.store(false, Ordering::Release);
        progress(&done);
        Some(done)
    }
}
