//! Missing media: find what a project refers to, search for moved files, and apply chosen
//! replacements.
//!
//! * [`inspect`] lists every media reference with its availability. It only reads file metadata
//!   (and hashes frozen prints, within [`Limits`]).
//! * [`find_candidates`] searches chosen folders and root mappings for the missing files. The
//!   search is read-only, bounded by [`Limits`] and stops when its [`Cancel`] token is set. A
//!   file name is never proof: a candidate is `verified` only when its SHA-256 matches a saved
//!   fingerprint (`mediaManifest`, a frozen print's hash, or the hash prefix in a collected
//!   file's name).
//! * [`apply`] validates every selection before it changes anything and returns a new project;
//!   the caller publishes it. Audio files are never modified, renamed, copied or deleted.
//! * [`fingerprint`] records full hashes in `mediaManifest` while the files are available.
//!
//! All four are synchronous file work: call them from a worker thread. The result types
//! serialize to the JSON the interface's relink dialog reads (`docs/MEDIA_RELINKING.md`).

use crate::error::{Error, Result};
use crate::media::{self, hash_file};
use crate::model::instrument::is_builtin;
use crate::model::{MediaManifestEntry, Project, RelinkChange, RelinkRecord};
use crate::session::iso8601;
use crate::Cancel;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Bounds of a search or inspection.
#[derive(Debug, Clone, PartialEq)]
pub struct Limits {
    /// Files a search looks at (250,000).
    pub max_files: usize,
    /// Distinct candidate files kept (20,000).
    pub max_candidates: usize,
    /// Candidates reported per missing reference (16).
    pub candidates_per_reference: usize,
    /// Files hashed (128).
    pub max_hashes: usize,
    /// Total bytes hashed (1 GiB).
    pub max_hash_bytes: u64,
    /// Largest single file hashed (256 MiB).
    pub max_file_hash_bytes: u64,
    /// Time budget in seconds (30; clamped to 0.1..=300).
    pub seconds: f64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_files: 250_000, max_candidates: 20_000, candidates_per_reference: 16, max_hashes: 128, max_hash_bytes: 1_073_741_824, max_file_hash_bytes: 268_435_456, seconds: 30.0 }
    }
}

impl Limits {
    fn deadline(&self) -> f64 {
        if self.seconds.is_finite() {
            self.seconds.clamp(0.1, 300.0)
        } else {
            30.0
        }
    }
}

/// The most bytes [`apply`] hashes to verify replacements by default (4 GiB).
pub const DEFAULT_APPLY_HASH_BYTES: u64 = 4_294_967_296;
/// Selections one [`apply`] accepts.
pub const MAX_SELECTIONS: usize = 16_384;
/// Folders and root mappings one search accepts.
pub const MAX_SEARCH_ROOTS: usize = 64;

/// "Files under `from` now live under `to`": the path below the root is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    /// The old root.
    pub from: PathBuf,
    /// The new root.
    pub to: PathBuf,
}

/// What kind of field a reference is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReferenceKind {
    /// An instrument's sample.
    Sample,
    /// A multisample zone.
    Zone,
    /// An audio clip.
    Clip,
    /// A take of an audio clip.
    Take,
    /// A frozen track's print. Only the exact printed content can replace it.
    Freeze,
}

/// The state of a referenced file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    /// A readable file with content.
    Available,
    /// Nothing at the path.
    Missing,
    /// Not a regular file.
    NotFile,
    /// Cannot be opened.
    Unreadable,
    /// Zero bytes.
    Empty,
    /// A relative path without a project file to resolve it against.
    UnresolvedRelativePath,
    /// A frozen print whose content is not what was printed.
    ContentMismatch,
    /// A frozen print that could not be verified within the limits.
    Unverified,
    /// A frozen print without a usable recorded hash.
    InvalidFreeze,
}

/// How a candidate was matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MatchKind {
    /// Its SHA-256 equals the saved fingerprint.
    #[serde(rename = "sha256")]
    Sha256,
    /// Its SHA-256 starts with the hash prefix of the collected file's name.
    #[serde(rename = "hash-prefix")]
    HashPrefix,
    /// It is where a root mapping puts the file.
    #[serde(rename = "relocation")]
    Relocation,
    /// It has the same name (or saved size). Not proof of identity.
    #[serde(rename = "name")]
    Name,
}

/// What a search found for a missing reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    /// No candidate.
    Unresolved,
    /// Exactly one verified candidate.
    Verified,
    /// Several candidates and no single verified one.
    Ambiguous,
    /// One unverified candidate.
    Suggested,
}

/// A file that may be the missing one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    /// The file.
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
    /// How it was matched.
    #[serde(rename = "match")]
    pub matched: MatchKind,
    /// Its content matches the saved fingerprint.
    pub verified: bool,
    /// Its content is known to differ from the saved fingerprint (or the fingerprints conflict).
    pub content_mismatch: bool,
    /// Rank; higher is better. Equal scores are ordered by path.
    pub score: i64,
    /// Why it is listed, for display.
    pub reasons: Vec<String>,
    /// Its SHA-256, when it was computed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// One media reference of the project.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    /// An opaque token naming the field, e.g. `/instruments/0/zones/2/samplePath`. Pass it back
    /// in a [`Selection`].
    pub id: String,
    /// The kind of field.
    pub kind: ReferenceKind,
    /// A label for display, e.g. `Drums · Zone 3`.
    pub label: String,
    /// The path as the project holds it.
    pub path: String,
    /// The file the path names; absent for a relative path without a project file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
    /// The file's state.
    pub status: FileStatus,
    /// Whether the file can be used.
    pub available: bool,
    /// Its size when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// The saved full fingerprint, if any.
    #[serde(rename = "expectedSHA256", skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    /// The hash prefix from a collected file's name, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_hash_prefix: Option<String>,
    /// The saved size, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_size: Option<u64>,
    /// The project holds two different fingerprints for this file.
    pub fingerprint_conflict: bool,
    /// Instrument index, for samples and zones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instrument: Option<usize>,
    /// Zone index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<usize>,
    /// Clip index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<usize>,
    /// Take index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<usize>,
    /// Track index, for frozen prints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<usize>,
    /// Only the exact saved content can replace this file (frozen prints).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub requires_exact_content: bool,
    /// Candidates found by a search, best first (at most `Limits::candidates_per_reference`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<Candidate>>,
    /// How many candidates the search found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_count: Option<usize>,
    /// The search's verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    /// Several candidates qualify equally; the user must choose.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<bool>,
    /// Nothing is applied without a [`Selection`] (always true after a search).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_selection: Option<bool>,
}

/// The result of [`inspect`] or [`find_candidates`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Every media reference.
    pub references: Vec<Reference>,
    /// The references whose file is not available (with candidates after a search).
    pub missing: Vec<Reference>,
    /// `references.len()`.
    pub reference_count: usize,
    /// `missing.len()`.
    pub missing_count: usize,
    /// References whose file is available.
    pub present_count: usize,
    /// Things the user should know, sorted, without duplicates.
    pub warnings: Vec<String>,
    /// Files the search looked at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_files: Option<usize>,
    /// Files the search hashed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hashed_files: Option<usize>,
    /// Bytes the search hashed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_bytes: Option<u64>,
    /// The search stopped at a limit; files it did not check are not assumed absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

/// Progress of a search, passed to the callback about four times a second.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// `searching`, `verifying` or `complete`.
    pub phase: &'static str,
    /// The folder being searched.
    pub root: String,
    /// Files looked at so far.
    pub scanned_files: usize,
    /// Candidate files so far.
    pub candidates: usize,
    /// Files hashed so far.
    pub hashed_files: usize,
    /// Bytes hashed so far.
    pub hash_bytes: u64,
}

/// One replacement the user chose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    /// The reference's [`Reference::id`].
    pub id: String,
    /// The reference's path when the selection was made; a reference that changed since is rejected.
    pub from: String,
    /// The replacement file (absolute).
    pub to: String,
    /// Accept a file whose content differs from the saved fingerprint: a deliberate sound
    /// replacement. Never honoured for frozen prints.
    #[serde(default)]
    pub allow_different_content: bool,
}

/// The result of [`apply`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    /// The project with the replacements made. The input project is not changed.
    pub project: Project,
    /// What changed.
    pub relinked: Vec<RelinkChange>,
    /// `relinked.len()`.
    pub changed: usize,
    /// References still missing afterwards.
    pub remaining: Vec<Reference>,
    /// Things the user should know.
    pub warnings: Vec<String>,
}

/// The result of [`fingerprint`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprinted {
    /// The project with `mediaManifest` updated.
    pub project: Project,
    /// Files hashed.
    pub fingerprinted: usize,
    /// Bytes hashed.
    pub hash_bytes: u64,
    /// Files skipped for the budget.
    pub warnings: Vec<String>,
}

// ---- Internals ---------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Path,
    SamplePath,
}

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Path => "path",
            Field::SamplePath => "samplePath",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Location {
    InstrumentSample { instrument: usize },
    Zone { instrument: usize, zone: usize, field: Field },
    Clip { clip: usize, field: Field },
    Take { clip: usize, take: usize },
    FrozenSong { track: usize },
    FrozenHead { track: usize, print: usize },
    FrozenLoop { track: usize, print: usize },
}

#[derive(Debug, Clone)]
struct Ref {
    id: String,
    kind: ReferenceKind,
    label: String,
    path: String,
    resolved: Option<String>,
    location: Location,
    required_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct FileInfo {
    path: String,
    size: u64,
    modified: f64,
    available: bool,
    status: FileStatus,
}

#[derive(Debug, Clone, Default)]
struct Expected {
    sha256: Option<String>,
    prefix: Option<String>,
    size: Option<u64>,
    conflict: bool,
}

#[derive(Debug, Clone)]
struct Fingerprint {
    path: String,
    size: u64,
    modified: f64,
    sha256: Option<String>,
}

const AUDIO_EXTENSIONS: [&str; 9] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "m4a", "caf"];

fn valid_hash(value: Option<&str>, length: usize) -> Option<String> {
    let value = value?;
    (value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| value.to_ascii_lowercase())
}

fn resolve(path: &str, project_file: Option<&Path>) -> Option<String> {
    media::resolve(path, project_file).map(|resolved| resolved.to_string_lossy().into_owned())
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

fn file_info(path: &str) -> FileInfo {
    let normalized = media::normalize(Path::new(path));
    let canonical = fs::canonicalize(&normalized).unwrap_or(normalized);
    let text = canonical.to_string_lossy().into_owned();
    let Ok(metadata) = fs::metadata(&canonical) else {
        return FileInfo { path: text, size: 0, modified: 0.0, available: false, status: FileStatus::Missing };
    };
    let modified = metadata.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map(|elapsed| elapsed.as_secs_f64()).unwrap_or(0.0);
    let regular = metadata.is_file();
    let readable = regular && fs::File::open(&canonical).is_ok();
    let status = if !regular {
        FileStatus::NotFile
    } else if !readable {
        FileStatus::Unreadable
    } else if metadata.len() == 0 {
        FileStatus::Empty
    } else {
        FileStatus::Available
    };
    FileInfo { path: text, size: metadata.len(), modified, available: status == FileStatus::Available, status }
}

fn hash_info(path: &str, max_bytes: u64, cancel: Option<&Cancel>) -> Result<Fingerprint> {
    let before = file_info(path);
    if !before.available || before.size > max_bytes {
        return Err(Error::invalid(format!("Cannot fingerprint this missing, unreadable or oversized file: {path}")));
    }
    let sha256 = hash_file(Path::new(&before.path), max_bytes, cancel).map_err(|error| match error {
        Error::Cancelled => Error::Cancelled,
        _ => Error::invalid("Media changed during content verification; search again."),
    })?;
    Ok(Fingerprint { path: before.path, size: before.size, modified: before.modified, sha256: Some(sha256) })
}

/// The original name of a collected file: `<16 or 24 hex digits>-name` without the prefix.
fn unhashed_name(name: &str) -> &str {
    for length in [24usize, 16] {
        if name.len() > length + 1 && name.is_char_boundary(length) && name.as_bytes()[length] == b'-' && valid_hash(Some(&name[..length]), length).is_some() {
            return &name[length + 1..];
        }
    }
    name
}

fn inside(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&if root == "/" { "/".to_string() } else { format!("{root}/") })
}

fn common_suffix(left: &str, right: &str) -> i64 {
    left.split('/').rev().filter(|part| !part.is_empty()).zip(right.split('/').rev().filter(|part| !part.is_empty())).take_while(|(a, b)| a.to_lowercase() == b.to_lowercase()).count() as i64
}

fn references(project: &Project, project_file: Option<&Path>) -> Vec<Ref> {
    let mut result: Vec<Ref> = Vec::new();
    let mut push = |id: String, kind: ReferenceKind, label: String, path: &Option<String>, location: Location, hash: Option<&str>| {
        let Some(path) = path.as_deref().filter(|path| !path.is_empty()) else { return };
        // Built-in sample references are application content, never missing media.
        if is_builtin(path) {
            return;
        }
        let required_sha256 = if kind == ReferenceKind::Freeze { valid_hash(hash, 64) } else { None };
        result.push(Ref { id, kind, label, path: path.to_string(), resolved: resolve(path, project_file), location, required_sha256 });
    };
    for (track, item) in project.track_list().iter().enumerate() {
        let Some(frozen) = &item.frozen else { continue };
        let name = item.display_name(track);
        push(format!("/tracks/{track}/frozen/path"), ReferenceKind::Freeze, format!("{name} · Frozen song"), &frozen.path, Location::FrozenSong { track }, frozen.sha256.as_deref());
        for (print, entry) in frozen.pattern_prints.iter().flatten().take(256).enumerate() {
            let prefix = format!("/tracks/{track}/frozen/patternPrints/{print}");
            push(format!("{prefix}/path"), ReferenceKind::Freeze, format!("{name} · Frozen pattern {} head", print + 1), &entry.path, Location::FrozenHead { track, print }, entry.sha256.as_deref());
            if let Some(loop_print) = &entry.loop_print {
                push(format!("{prefix}/loop/path"), ReferenceKind::Freeze, format!("{name} · Frozen pattern {} loop", print + 1), &loop_print.path, Location::FrozenLoop { track, print }, loop_print.sha256.as_deref());
            }
        }
    }
    for (instrument, item) in project.instrument_list().iter().enumerate() {
        let label = item.display_name(instrument);
        push(format!("/instruments/{instrument}/samplePath"), ReferenceKind::Sample, label.clone(), &item.sample_path, Location::InstrumentSample { instrument }, None);
        for (zone, entry) in item.zones.iter().flatten().take(128).enumerate() {
            for (field, path) in [(Field::SamplePath, &entry.sample_path), (Field::Path, &entry.path)] {
                push(format!("/instruments/{instrument}/zones/{zone}/{}", field.key()), ReferenceKind::Zone, format!("{label} · Zone {}", zone + 1), path, Location::Zone { instrument, zone, field }, None);
            }
        }
    }
    for (clip, item) in project.clip_list().iter().take(256).enumerate() {
        let label = item.name.clone().unwrap_or_else(|| format!("Audio clip {}", clip + 1));
        for (field, path) in [(Field::Path, &item.path), (Field::SamplePath, &item.sample_path)] {
            push(format!("/audioClips/{clip}/{}", field.key()), ReferenceKind::Clip, label.clone(), path, Location::Clip { clip, field }, None);
        }
        if let Some(set) = &item.take_set {
            for (take, entry) in set.takes.iter().flatten().take(32).enumerate() {
                let name = entry.name.clone().unwrap_or_else(|| format!("Take {}", take + 1));
                push(format!("/audioClips/{clip}/takeSet/takes/{take}/path"), ReferenceKind::Take, format!("{label} · {name}"), &entry.path, Location::Take { clip, take }, None);
            }
        }
    }
    result
}

fn expectations(project: &Project, references: &[Ref], project_file: Option<&Path>) -> BTreeMap<String, Expected> {
    let mut manifest: BTreeMap<String, Expected> = BTreeMap::new();
    for item in project.media_manifest.iter().flatten() {
        let Some(resolved) = item.path.as_deref().and_then(|path| resolve(path, project_file)) else { continue };
        let hash = valid_hash(item.sha256.as_deref(), 64);
        let value = manifest.entry(resolved).or_default();
        if let (Some(old), Some(new)) = (&value.sha256, &hash) {
            if old != new {
                value.conflict = true;
            }
        }
        if hash.is_some() {
            value.sha256 = hash;
        }
        if let Some(size) = item.size.filter(|size| *size >= 0) {
            value.size = Some(size as u64);
        }
    }
    let mut result = BTreeMap::new();
    for reference in references {
        let mut value = reference.resolved.as_ref().and_then(|path| manifest.get(path)).cloned().unwrap_or_default();
        if reference.kind == ReferenceKind::Freeze {
            match &reference.required_sha256 {
                Some(required) => {
                    if value.sha256.as_ref().is_some_and(|saved| saved != required) {
                        value.conflict = true;
                    }
                    value.sha256 = Some(required.clone());
                    value.prefix = None;
                }
                None => value.conflict = true,
            }
        }
        if value.sha256.is_none() {
            // Only the known collection layouts establish a fingerprint: `<Project>.media/<16 hex>-name`
            // and `<Kit>.stkit/Samples/<24 hex>-name`. A hash-looking name elsewhere proves nothing.
            let path = Path::new(&reference.path);
            let name = file_name(&reference.path);
            let folder = path.parent();
            let extension = |path: Option<&Path>| path.and_then(Path::extension).map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
            let kit_samples = folder.and_then(Path::file_name).is_some_and(|name| name.to_string_lossy().to_lowercase() == "samples") && extension(folder.and_then(Path::parent)) == "stkit";
            let length = if kit_samples {
                24
            } else if extension(folder) == "media" {
                16
            } else {
                0
            };
            if length > 0 && name.len() > length + 1 && name.is_char_boundary(length) && name.as_bytes()[length] == b'-' {
                value.prefix = valid_hash(Some(&name[..length]), length);
            }
        }
        result.insert(reference.id.clone(), value);
    }
    result
}

fn row(reference: &Ref, info: Option<&FileInfo>, expected: &Expected) -> Reference {
    let (instrument, zone, clip, take, track) = match reference.location {
        Location::InstrumentSample { instrument } => (Some(instrument), None, None, None, None),
        Location::Zone { instrument, zone, .. } => (Some(instrument), Some(zone), None, None, None),
        Location::Clip { clip, .. } => (None, None, Some(clip), None, None),
        Location::Take { clip, take } => (None, None, Some(clip), Some(take), None),
        Location::FrozenSong { track } | Location::FrozenHead { track, .. } | Location::FrozenLoop { track, .. } => (None, None, None, None, Some(track)),
    };
    Reference {
        id: reference.id.clone(),
        kind: reference.kind,
        label: reference.label.clone(),
        path: reference.path.clone(),
        resolved_path: reference.resolved.clone(),
        status: info.map(|info| info.status).unwrap_or(FileStatus::UnresolvedRelativePath),
        available: info.is_some_and(|info| info.available),
        size: info.filter(|info| info.available).map(|info| info.size),
        expected_sha256: expected.sha256.clone(),
        expected_hash_prefix: expected.prefix.clone(),
        expected_size: expected.size,
        fingerprint_conflict: expected.conflict,
        instrument,
        zone,
        clip,
        take,
        track,
        requires_exact_content: track.is_some(),
        candidates: None,
        candidate_count: None,
        resolution: None,
        ambiguous: None,
        requires_selection: None,
    }
}

fn sorted_unique(warnings: Vec<String>) -> Vec<String> {
    warnings.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

/// Lists every media reference of `project` and whether its file is available.
///
/// `project_file` resolves relative paths (a project read with [`crate::files::read_project`]
/// has none). A frozen print counts as available only when its content matches the hash
/// recorded when it was printed; those hashes are computed within `limits`.
pub fn inspect(project: &Project, project_file: Option<&Path>, limits: &Limits) -> Report {
    let references = references(project, project_file);
    let expected = expectations(project, &references, project_file);
    let mut memo: BTreeMap<String, FileInfo> = BTreeMap::new();
    let mut hashes: BTreeMap<String, Option<String>> = BTreeMap::new();
    let (mut hash_bytes, mut hash_count) = (0u64, 0usize);
    let mut warnings = Vec::new();
    let began = Instant::now();
    let deadline = limits.deadline();
    let stop = Cancel::new();
    let mut rows = Vec::with_capacity(references.len());
    for reference in &references {
        let mut info = reference.resolved.as_ref().map(|path| memo.entry(path.clone()).or_insert_with(|| file_info(path)).clone());
        let fingerprint = expected.get(&reference.id).cloned().unwrap_or_default();
        if fingerprint.conflict {
            warnings.push(if reference.kind == ReferenceKind::Freeze {
                format!("Conflicting saved fingerprints for {}; repair its original capture metadata or Unfreeze. Different content cannot replace a print.", reference.label)
            } else {
                format!("Conflicting saved fingerprints for {}; explicit replacement is required.", reference.label)
            });
        }
        if reference.kind == ReferenceKind::Freeze {
            if let Some(current) = &mut info {
                if fingerprint.conflict || reference.required_sha256.is_none() {
                    current.available = false;
                    current.status = FileStatus::InvalidFreeze;
                } else if current.available {
                    if !hashes.contains_key(&current.path)
                        && hash_count < limits.max_hashes
                        && current.size <= limits.max_file_hash_bytes
                        && current.size <= limits.max_hash_bytes.saturating_sub(hash_bytes)
                        && began.elapsed().as_secs_f64() < deadline
                    {
                        hash_count += 1;
                        hash_bytes += current.size;
                        hashes.insert(current.path.clone(), hash_info(&current.path, current.size, Some(&stop)).ok().and_then(|value| value.sha256));
                    }
                    match hashes.get(&current.path).cloned().flatten() {
                        Some(hash) => {
                            if Some(&hash) != reference.required_sha256.as_ref() {
                                current.available = false;
                                current.status = FileStatus::ContentMismatch;
                            }
                        }
                        None => {
                            current.available = false;
                            current.status = FileStatus::Unverified;
                            warnings.push(format!("Frozen print content could not be verified within the inspection budget: {}.", reference.label));
                        }
                    }
                }
            }
        }
        rows.push(row(reference, info.as_ref(), &fingerprint));
    }
    let missing: Vec<Reference> = rows.iter().filter(|row| !row.available).cloned().collect();
    Report {
        reference_count: rows.len(),
        missing_count: missing.len(),
        present_count: rows.len() - missing.len(),
        references: rows,
        missing,
        warnings: sorted_unique(warnings),
        scanned_files: None,
        hashed_files: None,
        hash_bytes: None,
        truncated: None,
    }
}

/// Searches `search_roots` (recursively) and `relocations` for the missing files of `project`.
///
/// Also looks in the project's `.media` folder. Hidden files and symbolic links are skipped.
/// The search stops at any of `limits` (then `truncated` is set and a warning says so) and
/// returns [`Error::Cancelled`] when `cancel` is set. `progress` is called at most about four
/// times a second and once at the end. Nothing is changed on disk or in `project`.
#[allow(unused_assignments)]
pub fn find_candidates(
    project: &Project,
    project_file: Option<&Path>,
    search_roots: &[PathBuf],
    relocations: &[Relocation],
    limits: &Limits,
    cancel: &Cancel,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Report> {
    cancel.check()?;
    if search_roots.len() > MAX_SEARCH_ROOTS || relocations.len() > MAX_SEARCH_ROOTS {
        return Err(Error::invalid("Search up to 64 media folders or root mappings at a time."));
    }
    let mut report = inspect(project, project_file, limits);
    let all = references(project, project_file);
    let expected = expectations(project, &all, project_file);
    let missing_ids: BTreeSet<&str> = report.missing.iter().map(|row| row.id.as_str()).collect();
    let missing: Vec<&Ref> = all.iter().filter(|reference| missing_ids.contains(reference.id.as_str())).collect();
    if missing.is_empty() {
        report.scanned_files = Some(0);
        report.truncated = Some(false);
        return Ok(report);
    }
    let start = Instant::now();
    let deadline = limits.deadline();
    let mut last_progress: Option<Instant> = None;
    let mut warnings = std::mem::take(&mut report.warnings);
    let (mut truncated, mut scanned, mut hashed, mut hash_bytes) = (false, 0usize, 0usize, 0u64);
    let mut candidate_info: BTreeMap<String, FileInfo> = BTreeMap::new();
    // reference id -> candidate path -> reasons
    let mut hints: BTreeMap<String, BTreeMap<String, BTreeSet<&'static str>>> = BTreeMap::new();
    let candidate_limit = limits.max_candidates.clamp(1, 100_000);

    fn add(
        candidate_info: &mut BTreeMap<String, FileInfo>,
        hints: &mut BTreeMap<String, BTreeMap<String, BTreeSet<&'static str>>>,
        truncated: &mut bool,
        limit: usize,
        info: FileInfo,
        reference: &Ref,
        reason: &'static str,
    ) {
        if !info.available {
            return;
        }
        if !candidate_info.contains_key(&info.path) && candidate_info.len() >= limit {
            *truncated = true;
            return;
        }
        hints.entry(reference.id.clone()).or_default().entry(info.path.clone()).or_default().insert(reason);
        candidate_info.insert(info.path.clone(), info);
    }
    macro_rules! notify {
        ($phase:expr, $root:expr) => {
            if $phase == "complete" || last_progress.map_or(true, |last| last.elapsed().as_secs_f64() >= 0.25) {
                progress(&Progress { phase: $phase, root: $root.to_string(), scanned_files: scanned, candidates: candidate_info.len(), hashed_files: hashed, hash_bytes });
                last_progress = Some(Instant::now());
            }
        };
    }
    // Returns false when the time budget is used up.
    macro_rules! in_budget {
        () => {{
            cancel.check()?;
            if start.elapsed().as_secs_f64() >= deadline {
                truncated = true;
                false
            } else {
                true
            }
        }};
    }

    let mut names: BTreeMap<String, Vec<&Ref>> = BTreeMap::new();
    let mut sizes: BTreeMap<u64, Vec<&Ref>> = BTreeMap::new();
    let mut prefixes: BTreeMap<String, Vec<&Ref>> = BTreeMap::new();
    for reference in &missing {
        let name = file_name(&reference.path);
        names.entry(name.to_lowercase()).or_default().push(reference);
        let original = unhashed_name(&name);
        if original != name {
            names.entry(original.to_lowercase()).or_default().push(reference);
        }
        if let Some(value) = expected.get(&reference.id) {
            if let (Some(_), Some(size)) = (&value.sha256, value.size.filter(|size| *size > 0)) {
                sizes.entry(size).or_default().push(reference);
            }
            if let Some(hash) = value.sha256.as_ref().or(value.prefix.as_ref()) {
                prefixes.entry(hash[..16].to_string()).or_default().push(reference);
            }
        }
    }
    for reference in &missing {
        if !in_budget!() {
            break;
        }
        if let Some(path) = &reference.resolved {
            for relocation in relocations {
                let from = media::normalize(&relocation.from).to_string_lossy().into_owned();
                if inside(path, &from) {
                    let suffix = path[from.len()..].trim_matches('/');
                    let moved = media::normalize(&relocation.to).join(suffix);
                    add(&mut candidate_info, &mut hints, &mut truncated, candidate_limit, file_info(&moved.to_string_lossy()), reference, "root relocation");
                }
            }
        }
        let name = file_name(&reference.path);
        if let Some(project_file) = project_file {
            let collected = media::media_folder(project_file).join(&name);
            add(&mut candidate_info, &mut hints, &mut truncated, candidate_limit, file_info(&collected.to_string_lossy()), reference, "project media folder");
        }
        for root in search_roots {
            add(&mut candidate_info, &mut hints, &mut truncated, candidate_limit, file_info(&root.join(&name).to_string_lossy()), reference, "selected folder");
        }
    }
    let mut roots: Vec<String> = search_roots.iter().map(|root| media::normalize(root).to_string_lossy().into_owned()).collect::<BTreeSet<_>>().into_iter().collect();
    roots.sort_by(|left, right| left.len().cmp(&right.len()).then(left.cmp(right)));
    let file_limit = limits.max_files.clamp(1, 2_000_000);
    let mut visited: Vec<String> = Vec::new();
    'roots: for root in &roots {
        if !in_budget!() {
            break;
        }
        if visited.iter().any(|done| inside(root, done)) {
            continue;
        }
        visited.push(root.clone());
        if !Path::new(root).is_dir() {
            warnings.push(format!("Search folder is unavailable: {root}"));
            continue;
        }
        let mut pending: Vec<PathBuf> = vec![PathBuf::from(root)];
        while let Some(folder) = pending.pop() {
            let entries = match fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(error) => {
                    if warnings.len() < 30 {
                        warnings.push(format!("{}: {}", file_name(&folder.to_string_lossy()), error.kind()));
                    }
                    continue;
                }
            };
            // Sorted, so a truncated search is the same search every time.
            let mut entries: Vec<fs::DirEntry> = entries.flatten().collect();
            entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
            let mut files = Vec::new();
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if kind.is_file() {
                    files.push((name, entry));
                }
            }
            for (name, entry) in files.into_iter().rev() {
                if !in_budget!() {
                    break 'roots;
                }
                scanned += 1;
                if scanned > file_limit {
                    truncated = true;
                    break 'roots;
                }
                let original = unhashed_name(&name);
                let hashed_name = original != name;
                let path = entry.path();
                let extension = path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
                let size = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                let mut found: Vec<(&Ref, &'static str)> = Vec::new();
                found.extend(names.get(&name.to_lowercase()).into_iter().flatten().map(|reference| (*reference, "matching filename")));
                if hashed_name {
                    found.extend(names.get(&original.to_lowercase()).into_iter().flatten().map(|reference| (*reference, "matching filename")));
                    found.extend(prefixes.get(&name[..16].to_lowercase()).into_iter().flatten().map(|reference| (*reference, "collected filename hash prefix")));
                }
                if AUDIO_EXTENSIONS.contains(&extension.as_str()) {
                    found.extend(sizes.get(&size).into_iter().flatten().map(|reference| (*reference, "matching saved size")));
                }
                if !found.is_empty() {
                    let info = file_info(&path.to_string_lossy());
                    for (reference, reason) in found {
                        add(&mut candidate_info, &mut hints, &mut truncated, candidate_limit, info.clone(), reference, reason);
                    }
                }
                notify!("searching", root);
            }
        }
    }
    // Hash the most promising candidates first, within the budget.
    let mut priority: BTreeMap<&str, i32> = BTreeMap::new();
    for reference in &missing {
        if !expected.get(&reference.id).is_some_and(|value| value.sha256.is_some() || value.prefix.is_some()) {
            continue;
        }
        for (path, reasons) in hints.get(&reference.id).into_iter().flatten() {
            let rank = if reasons.contains("root relocation") {
                400
            } else if reasons.contains("collected filename hash prefix") {
                300
            } else if reasons.contains("matching filename") {
                200
            } else {
                0
            };
            let entry = priority.entry(path.as_str()).or_insert(0);
            *entry = (*entry).max(rank);
        }
    }
    let mut verification: Vec<(&str, i32)> = priority.into_iter().collect();
    verification.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
    let mut fingerprints: BTreeMap<String, Fingerprint> = BTreeMap::new();
    let mut hash_errors: BTreeMap<String, String> = BTreeMap::new();
    for (path, _) in verification {
        if !in_budget!() {
            break;
        }
        let Some(info) = candidate_info.get(path) else { continue };
        if hashed >= limits.max_hashes || info.size > limits.max_file_hash_bytes || info.size > limits.max_hash_bytes.saturating_sub(hash_bytes) {
            hash_errors.insert(path.to_string(), "Content verification budget reached".to_string());
            truncated = true;
            continue;
        }
        hashed += 1;
        hash_bytes += info.size;
        match hash_info(path, limits.max_file_hash_bytes, Some(cancel)) {
            Ok(value) => {
                fingerprints.insert(path.to_string(), value);
            }
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) => {
                hash_errors.insert(path.to_string(), error.to_string());
            }
        }
        notify!("verifying", "");
    }
    let mut updated = Vec::with_capacity(missing.len());
    for reference in &missing {
        let expectation = expected.get(&reference.id).cloned().unwrap_or_default();
        let mut entry = report.missing.iter().find(|row| row.id == reference.id).cloned().unwrap_or_else(|| row(reference, None, &expectation));
        let mut candidates: Vec<Candidate> = Vec::new();
        for (path, reasons) in hints.get(&reference.id).into_iter().flatten() {
            let Some(info) = candidate_info.get(path) else { continue };
            let mut reasons: Vec<String> = reasons.iter().map(|reason| reason.to_string()).collect();
            let fingerprint = fingerprints.get(path).and_then(|value| value.sha256.clone());
            let required = expectation.sha256.as_ref().or(expectation.prefix.as_ref());
            let verified = !expectation.conflict && required.is_some_and(|required| fingerprint.as_ref().is_some_and(|hash| hash.starts_with(required.as_str())));
            let mismatch = required.is_some() && fingerprint.is_some() && !verified;
            let relocated = reasons.iter().any(|reason| reason == "root relocation");
            let matched = if verified {
                if expectation.sha256.is_some() {
                    MatchKind::Sha256
                } else {
                    MatchKind::HashPrefix
                }
            } else if relocated {
                MatchKind::Relocation
            } else {
                MatchKind::Name
            };
            if verified {
                reasons.push(if expectation.sha256.is_some() { "Exact SHA-256 content match" } else { "Matches collected-media SHA-256 prefix" }.to_string());
            }
            if mismatch {
                reasons.push("Content differs from the saved fingerprint".to_string());
            }
            if let Some(failure) = hash_errors.get(path) {
                reasons.push(failure.clone());
            }
            if expectation.conflict {
                reasons.push(
                    if reference.kind == ReferenceKind::Freeze { "Frozen print fingerprints conflict; restore original capture metadata or Unfreeze" } else { "Conflicting project fingerprints require deliberate replacement" }
                        .to_string(),
                );
            }
            let suffix = common_suffix(reference.resolved.as_deref().unwrap_or(&reference.path), path);
            let score = if verified { 1000 } else { 0 } + if matched == MatchKind::Relocation { 400 } else { 0 } + suffix * 20 + if mismatch { -1000 } else { 0 };
            candidates.push(Candidate { path: path.clone(), size: info.size, matched, verified, content_mismatch: mismatch || expectation.conflict, score, reasons, sha256: fingerprint });
        }
        candidates.sort_by(|left, right| right.score.cmp(&left.score).then(left.path.cmp(&right.path)));
        let verified_count = candidates.iter().filter(|candidate| candidate.verified).count();
        entry.candidate_count = Some(candidates.len());
        entry.resolution = Some(if candidates.is_empty() {
            Resolution::Unresolved
        } else if verified_count == 1 {
            Resolution::Verified
        } else if candidates.len() > 1 {
            Resolution::Ambiguous
        } else {
            Resolution::Suggested
        });
        entry.ambiguous = Some(verified_count > 1 || (verified_count == 0 && candidates.len() > 1));
        entry.requires_selection = Some(true);
        candidates.truncate(limits.candidates_per_reference.clamp(1, 100));
        entry.candidates = Some(candidates);
        updated.push(entry);
    }
    if truncated {
        warnings.push("Search or verification reached its limit. Narrow the chosen folders or increase the limits; omitted candidates are not assumed absent.".to_string());
    }
    report.missing = updated;
    report.scanned_files = Some(scanned);
    report.hashed_files = Some(hashed);
    report.hash_bytes = Some(hash_bytes);
    report.truncated = Some(truncated);
    report.warnings = sorted_unique(warnings);
    notify!("complete", "");
    Ok(report)
}

/// Records the SHA-256, size and modification time of every available media file in
/// `mediaManifest`, so the files can be recognised by content later. Returns a new project;
/// nothing is saved. A frozen print whose content changed is an error: its recorded hash is
/// never replaced.
pub fn fingerprint(project: &Project, project_file: Option<&Path>, limits: &Limits, cancel: &Cancel) -> Result<Fingerprinted> {
    cancel.check()?;
    let references = references(project, project_file);
    let expected = expectations(project, &references, project_file);
    let mut manifest = project.media_manifest.clone().unwrap_or_default();
    let (mut count, mut bytes, mut warnings) = (0usize, 0u64, Vec::new());
    let paths: BTreeSet<&String> = references.iter().filter_map(|reference| reference.resolved.as_ref()).collect();
    for path in paths {
        cancel.check()?;
        let info = file_info(path);
        if !info.available {
            continue;
        }
        if count >= limits.max_hashes || info.size > limits.max_file_hash_bytes || info.size > limits.max_hash_bytes.saturating_sub(bytes) {
            warnings.push(format!("Fingerprint budget skipped: {path}"));
            continue;
        }
        let value = hash_info(path, limits.max_file_hash_bytes, Some(cancel))?;
        for reference in references.iter().filter(|reference| reference.resolved.as_ref() == Some(path) && reference.kind == ReferenceKind::Freeze) {
            let conflict = expected.get(&reference.id).is_some_and(|value| value.conflict);
            if conflict || reference.required_sha256.is_none() || value.sha256 != reference.required_sha256 {
                return Err(Error::invalid(format!(
                    "Frozen print {} changed. Fingerprinting cannot replace its captured SHA-256; relink the original print or Unfreeze.",
                    reference.label
                )));
            }
        }
        manifest.retain(|entry| entry.path.as_deref().and_then(|entry| resolve(entry, project_file)).as_ref() != Some(path));
        // Keep the reference's spelling, including an intentional alias.
        manifest.push(manifest_entry(path.clone(), &value));
        count += 1;
        bytes += info.size;
    }
    manifest.sort_by(|left, right| left.path.cmp(&right.path));
    let mut updated = project.clone();
    updated.media_manifest = Some(manifest);
    Ok(Fingerprinted { project: updated, fingerprinted: count, hash_bytes: bytes, warnings })
}

fn manifest_entry(path: String, value: &Fingerprint) -> MediaManifestEntry {
    MediaManifestEntry { path: Some(path), size: Some(value.size as i64), modified: Some(value.modified), sha256: value.sha256.clone(), ..MediaManifestEntry::default() }
}

/// The paths that must move together with `reference`, and a name for the group:
/// a zone's two aliases; a clip's `path`, `samplePath` and active take.
fn linked(project: &Project, reference: &Ref) -> Option<(String, Vec<String>)> {
    let present = |paths: Vec<&Option<String>>| paths.into_iter().flatten().filter(|path| !path.is_empty()).cloned().collect::<Vec<String>>();
    match reference.location {
        Location::Zone { instrument, zone, .. } => {
            let entry = project.instrument_list().get(instrument)?.zones.as_ref()?.get(zone)?;
            Some((format!("/instruments/{instrument}/zones/{zone}"), present(vec![&entry.path, &entry.sample_path])))
        }
        Location::Clip { clip, .. } | Location::Take { clip, .. } => {
            let item = project.clip_list().get(clip)?;
            let active = item.take_set.as_ref().and_then(|set| set.active_index());
            if let Location::Take { take, .. } = reference.location {
                if Some(take) != active {
                    return None;
                }
            }
            let mut paths = present(vec![&item.path, &item.sample_path]);
            if let Some(path) = active.and_then(|active| item.take_set.as_ref()?.takes.as_ref()?.get(active)?.path.clone()) {
                paths.push(path);
            }
            Some((format!("/audioClips/{clip}"), paths))
        }
        _ => None,
    }
}

fn set_path(project: &mut Project, location: Location, path: &str) -> Option<()> {
    let value = Some(path.to_string());
    let follow = |field: &mut Option<String>| {
        if field.is_some() {
            *field = Some(path.to_string());
        }
    };
    match location {
        Location::InstrumentSample { instrument } => project.instruments.as_mut()?.get_mut(instrument)?.sample_path = value,
        Location::Zone { instrument, zone, field } => {
            let entry = project.instruments.as_mut()?.get_mut(instrument)?.zones.as_mut()?.get_mut(zone)?;
            match field {
                Field::Path => entry.path = value,
                Field::SamplePath => entry.sample_path = value,
            }
            follow(&mut entry.path);
            follow(&mut entry.sample_path);
        }
        Location::Clip { clip, field } => {
            let item = project.audio_clips.as_mut()?.get_mut(clip)?;
            match field {
                Field::Path => item.path = value,
                Field::SamplePath => item.sample_path = value,
            }
            follow(&mut item.path);
            follow(&mut item.sample_path);
            // The active take mirrors the clip's own path.
            if let Some(set) = &mut item.take_set {
                if let Some(active) = set.active_index() {
                    set.takes.as_mut()?.get_mut(active)?.path = Some(path.to_string());
                    item.path = Some(path.to_string());
                }
            }
        }
        Location::Take { clip, take } => {
            let item = project.audio_clips.as_mut()?.get_mut(clip)?;
            let set = item.take_set.as_mut()?;
            let active = set.active_index();
            set.takes.as_mut()?.get_mut(take)?.path = value;
            if active == Some(take) {
                item.path = Some(path.to_string());
                follow(&mut item.sample_path);
            }
        }
        Location::FrozenSong { track } => project.tracks.as_mut()?.get_mut(track)?.frozen.as_mut()?.path = value,
        Location::FrozenHead { track, print } => project.tracks.as_mut()?.get_mut(track)?.frozen.as_mut()?.pattern_prints.as_mut()?.get_mut(print)?.path = value,
        Location::FrozenLoop { track, print } => {
            project.tracks.as_mut()?.get_mut(track)?.frozen.as_mut()?.pattern_prints.as_mut()?.get_mut(print)?.loop_print.as_mut()?.path = value
        }
    }
    Some(())
}

/// Applies the chosen replacements to a copy of `project`.
///
/// Every selection is checked first; one bad selection rejects the batch and nothing changes:
///
/// * `id` must name a current reference and `from` must equal its path (a selection made before
///   the project changed is stale);
/// * `to` must be an absolute path to a readable file with content;
/// * a reference whose original file is available again is not replaced (refresh instead);
/// * linked fields (a zone's `path`/`samplePath`, a clip's path and active take) must agree
///   before and get the same replacement;
/// * a replacement whose content differs from the saved fingerprint needs
///   `allow_different_content`; a frozen print only accepts its exact printed content.
///
/// The result's project has the paths replaced, `mediaManifest` updated for the new files and
/// the operation appended to `mediaRelinkHistory` (the last 64 are kept). `max_hash_bytes` bounds
/// the verification reads ([`DEFAULT_APPLY_HASH_BYTES`]).
pub fn apply(project: &Project, selections: &[Selection], project_file: Option<&Path>, max_hash_bytes: u64, cancel: &Cancel) -> Result<Applied> {
    cancel.check()?;
    if selections.len() > MAX_SELECTIONS {
        return Err(Error::invalid("Too many media replacements in one operation."));
    }
    if selections.is_empty() {
        let remaining = inspect(project, project_file, &Limits::default()).missing;
        return Ok(Applied { project: project.clone(), relinked: Vec::new(), changed: 0, remaining, warnings: Vec::new() });
    }
    let references = references(project, project_file);
    let by_id: BTreeMap<&str, &Ref> = references.iter().map(|reference| (reference.id.as_str(), reference)).collect();
    let expected = expectations(project, &references, project_file);
    let mut used = BTreeSet::new();
    let mut memo: BTreeMap<String, Fingerprint> = BTreeMap::new();
    let mut bytes = 0u64;
    let mut linked_targets: BTreeMap<String, String> = BTreeMap::new();
    let mut prepared: Vec<(&Ref, Fingerprint, bool)> = Vec::new();
    let mut warnings = Vec::new();
    for selection in selections {
        cancel.check()?;
        let stale = || Error::invalid("A relink selection is stale or invalid. Refresh missing media and select a local replacement for its current reference.");
        let reference = *by_id.get(selection.id.as_str()).ok_or_else(stale)?;
        if !used.insert(selection.id.as_str()) || selection.from != reference.path || !media::is_absolute(&selection.to) {
            return Err(stale());
        }
        let target = file_info(&selection.to);
        if !target.available {
            return Err(Error::invalid(format!("Replacement is not a readable, nonempty file: {}", selection.to)));
        }
        if let Some((owner, paths)) = linked(project, reference) {
            if !paths.iter().all(|path| resolve(path, project_file) == reference.resolved) {
                return Err(Error::invalid("The sample or audio clip aliases refer to different files. Repair the source selection before relinking."));
            }
            if linked_targets.get(&owner).is_some_and(|previous| *previous != target.path) {
                return Err(Error::invalid("Linked sample or audio clip references must use the same replacement file."));
            }
            linked_targets.insert(owner, target.path.clone());
        }
        let expectation = expected.get(&reference.id).cloned().unwrap_or_default();
        let frozen = reference.kind == ReferenceKind::Freeze;
        let deliberate = !frozen && selection.allow_different_content;
        if let Some(old) = reference.resolved.as_deref().map(file_info).filter(|old| old.available && old.path != target.path) {
            match (&reference.required_sha256, frozen) {
                (Some(required), true) => {
                    if old.size > max_hash_bytes.saturating_sub(bytes) {
                        return Err(Error::invalid("The existing frozen print exceeds the verification budget."));
                    }
                    let original = hash_info(&old.path, max_hash_bytes - bytes, Some(cancel))?;
                    bytes += old.size;
                    if original.sha256.as_ref() == Some(required) {
                        return Err(Error::invalid(format!("{} is available again. Refresh before replacing a reconnected original.", reference.label)));
                    }
                }
                _ => return Err(Error::invalid(format!("{} is available again. Refresh the report before replacing a reconnected original.", reference.label))),
            }
        }
        let value = match memo.get(&target.path) {
            Some(value) => value.clone(),
            None => {
                let value = if target.size <= max_hash_bytes.saturating_sub(bytes) {
                    let value = hash_info(&target.path, max_hash_bytes - bytes, Some(cancel))?;
                    bytes += target.size;
                    value
                } else {
                    if (expectation.sha256.is_some() || expectation.prefix.is_some()) && !deliberate {
                        return Err(Error::invalid(if frozen {
                            "The frozen print exceeds the verification budget. Increase the budget to verify its exact SHA-256 or Unfreeze."
                        } else {
                            "The replacement exceeds the verification budget. Increase the budget or explicitly choose a different-content replacement."
                        }));
                    }
                    warnings.push(format!("Replacement was selected explicitly but could not be fingerprinted within the budget: {}", target.path));
                    Fingerprint { path: target.path.clone(), size: target.size, modified: target.modified, sha256: None }
                };
                memo.insert(target.path.clone(), value.clone());
                value
            }
        };
        let required = expectation.sha256.as_ref().or(expectation.prefix.as_ref());
        let matches = |required: &String| value.sha256.as_ref().is_some_and(|hash| hash.starts_with(required.as_str()));
        let mismatch = expectation.conflict || required.is_some_and(|required| !matches(required));
        if frozen && (expectation.conflict || reference.required_sha256.is_none() || value.sha256 != reference.required_sha256) {
            return Err(Error::invalid(format!(
                "Frozen print {} requires its exact original SHA-256. Different-content replacement is unavailable; relink the verified print or Unfreeze.",
                reference.label
            )));
        }
        if mismatch && !deliberate {
            return Err(Error::invalid(format!(
                "The selected file for {} does not match its saved content fingerprint. Select the original file, or explicitly allow different content to replace the sound.",
                reference.label
            )));
        }
        if mismatch {
            warnings.push(format!("Different content explicitly selected for {}.", reference.label));
        }
        prepared.push((reference, value, mismatch));
    }
    let mut updated = project.clone();
    let mut changes = Vec::with_capacity(prepared.len());
    for (reference, value, mismatch) in &prepared {
        set_path(&mut updated, reference.location, &value.path);
        changes.push(RelinkChange { id: Some(reference.id.clone()), from: Some(reference.path.clone()), to: Some(value.path.clone()), different_content: Some(*mismatch), ..RelinkChange::default() });
    }
    let active: BTreeSet<String> = self::references(&updated, project_file).into_iter().filter_map(|reference| reference.resolved).collect();
    let resolved_entry = |entry: &MediaManifestEntry| entry.path.as_deref().and_then(|path| resolve(path, project_file));
    let mut manifest: Vec<MediaManifestEntry> = project.media_manifest.iter().flatten().filter(|entry| resolved_entry(entry).is_some_and(|path| active.contains(&path))).cloned().collect();
    for value in memo.values().filter(|value| active.contains(&value.path)) {
        manifest.retain(|entry| resolved_entry(entry).as_deref() != Some(value.path.as_str()));
        manifest.push(manifest_entry(value.path.clone(), value));
    }
    manifest.sort_by(|left, right| left.path.cmp(&right.path));
    updated.media_manifest = Some(manifest);
    if !changes.is_empty() {
        let mut history = project.media_relink_history.clone().unwrap_or_default();
        history.push(RelinkRecord { date: Some(iso8601(SystemTime::now())), changes: Some(changes.clone()), ..RelinkRecord::default() });
        let keep = history.len().saturating_sub(64);
        updated.media_relink_history = Some(history.split_off(keep));
    }
    let remaining = inspect(&updated, project_file, &Limits::default()).missing;
    Ok(Applied { project: updated, changed: changes.len(), relinked: changes, remaining, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collected_names_give_up_their_hash_prefix() {
        assert_eq!(unhashed_name("0123456789abcdef-Kick.wav"), "Kick.wav");
        assert_eq!(unhashed_name("0123456789abcdef01234567-Kick.wav"), "Kick.wav");
        assert_eq!(unhashed_name("0123456789abcdeg-Kick.wav"), "0123456789abcdeg-Kick.wav", "not hexadecimal");
        assert_eq!(unhashed_name("0123456789abcdef-"), "0123456789abcdef-");
        assert_eq!(unhashed_name("Kick.wav"), "Kick.wav");
        assert_eq!(unhashed_name("ééééééééé-Kick.wav"), "ééééééééé-Kick.wav", "multi-byte names are not split inside a character");
    }

    #[test]
    fn path_helpers() {
        assert!(inside("/a/b/c.wav", "/a/b") && inside("/a/b", "/a/b") && inside("/x", "/"));
        assert!(!inside("/a/bc/d.wav", "/a/b"));
        assert_eq!(common_suffix("/old/Pack/Drums/Kick.wav", "/new/pack/drums/kick.WAV"), 3);
        assert_eq!(common_suffix("/old/Kick.wav", "/new/Snare.wav"), 0);
        assert_eq!(valid_hash(Some("ABCDEF0123456789"), 16).as_deref(), Some("abcdef0123456789"));
        assert_eq!(valid_hash(Some("abc"), 16), None);
    }

    #[test]
    fn limits_default_to_the_documented_bounds() {
        let limits = Limits::default();
        assert_eq!((limits.max_files, limits.max_candidates, limits.candidates_per_reference, limits.max_hashes), (250_000, 20_000, 16, 128));
        assert_eq!((limits.max_hash_bytes, limits.max_file_hash_bytes, limits.seconds), (1 << 30, 256 << 20, 30.0));
        assert_eq!((Limits { seconds: 0.0, ..Limits::default() }.deadline(), Limits { seconds: f64::NAN, ..Limits::default() }.deadline(), Limits { seconds: 1e9, ..Limits::default() }.deadline()), (0.1, 30.0, 300.0));
    }
}
