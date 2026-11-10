//! Project files on disk.
//!
//! A project is one JSON file (`Song.supertracker`, indented, keys sorted) and, next to it, a
//! media folder (`Song.media`) that holds a copy of every sample, clip and print the project
//! uses, named `<first 16 hex digits of its SHA-256>-<original name>`. Inside the file, media
//! paths are relative to the file's folder, so the pair can be moved together; in memory they
//! are absolute.
//!
//! Writes are atomic: the bytes go to a temporary file in the destination's folder, are flushed
//! to disk, and replace the destination with one rename. A failed or interrupted write leaves
//! the previous file as it was. Media is collected the same way (copy under a temporary name,
//! then rename), so a truncated copy never sits under a final name.

use crate::error::{Error, Result};
use crate::media::{self, hash_file, is_readable_nonempty_file, DEFAULT_HASH_LIMIT};
use crate::model::Project;
use crate::validate::validate;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// The extension of project files.
pub const PROJECT_EXTENSION: &str = "supertracker";
/// Project files at or above this size are not opened (`ProjectStore.read`).
pub const MAX_PROJECT_FILE_BYTES: u64 = 100_000_000;
/// The most bytes of frozen prints a save verifies (4 GiB); a single print up to 1 GiB.
pub const FREEZE_VERIFY_BUDGET: u64 = 4_294_967_296;

/// A name no other call in this process, and in practice no other process, produces.
pub(crate) fn unique_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_nanos()).unwrap_or(0);
    format!("{:x}-{:x}-{:x}", std::process::id(), nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Writes `bytes` to `path` atomically: temporary file in the same folder, flush to disk, rename.
/// On any failure the temporary file is removed and whatever was at `path` is untouched.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let folder = path.parent().filter(|folder| !folder.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().ok_or_else(|| Error::invalid("Choose a file name to save to."))?;
    let temporary = folder.join(format!(".{}.{}.saving", name.to_string_lossy(), unique_token()));
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    match written {
        Ok(()) => {
            // Make the rename durable where the platform allows opening a folder.
            if let Ok(directory) = fs::File::open(folder) {
                let _ = directory.sync_all();
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(Error::io("save", path, &error))
        }
    }
}

/// Reads a project's JSON from disk without resolving media paths: parses and validates.
pub(crate) fn read_stored(file: &Path) -> Result<Project> {
    let metadata = fs::metadata(file).map_err(|error| Error::io("open", file, &error))?;
    if metadata.len() >= MAX_PROJECT_FILE_BYTES {
        return Err(Error::invalid("Project is too large to safely open (100 MB limit)."));
    }
    let bytes = fs::read(file).map_err(|error| Error::io("open", file, &error))?;
    let text = String::from_utf8(bytes).map_err(|_| Error::invalid("The file is not a SuperTracker project."))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| Error::invalid("The file is not a SuperTracker project."))?;
    let project = Project::from_value(value)?;
    validate(&project)?;
    Ok(project)
}

/// Opens a project file: reads it, validates it and makes its media paths absolute.
///
/// This changes nothing on disk and no session; [`crate::session::Session::open`] is the call
/// that also makes the file the open document.
pub fn read_project(file: &Path) -> Result<Project> {
    let mut project = read_stored(file)?;
    media::make_absolute(&mut project, file);
    Ok(project)
}

/// What [`write_project`] did.
#[derive(Debug, Clone)]
pub struct Written {
    /// The project as it is now open: media paths absolute, pointing into the media folder for
    /// every file that was collected.
    pub project: Project,
    /// SHA-256 of the project file as written.
    pub file_sha256: String,
    /// Files copied into the media folder by this call (0 when everything was already there).
    pub collected: usize,
}

/// Fails when a frozen print that can be read no longer has the content that was printed.
/// Missing or empty prints stay repairable and are not an error here.
fn verify_frozen_prints(hashes: &[(String, String)], base: &Path) -> Result<()> {
    let mut remaining = FREEZE_VERIFY_BUDGET;
    let mut sorted: Vec<&(String, String)> = hashes.iter().collect();
    sorted.sort();
    for (path, expected) in sorted {
        let Some(file) = media::resolve(path, Some(base)) else { continue };
        if !is_readable_nonempty_file(&file) {
            continue;
        }
        let size = fs::metadata(&file).map(|metadata| metadata.len()).unwrap_or(0);
        if size > remaining {
            return Err(Error::invalid("Frozen media exceeds the 4 GiB save-verification budget."));
        }
        remaining -= size;
        if hash_file(&file, size.min(1_073_741_824), None)? != *expected {
            let name = file.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            return Err(Error::invalid(format!(
                "Frozen audio content changed: {name}. Relink the verified original print or Unfreeze; its recorded hash cannot be replaced."
            )));
        }
    }
    Ok(())
}

/// Copies `source` into `folder` as `name` unless a file of that name is already there.
/// The copy is made under a private name and renamed, so an interrupted copy leaves nothing
/// under the final name.
fn collect_file(source: &Path, folder: &Path, name: &str) -> Result<(PathBuf, bool)> {
    let target = folder.join(name);
    if target.exists() {
        return Ok((target, false));
    }
    let partial = folder.join(format!(".collecting-{}", unique_token()));
    let copied = fs::copy(source, &partial).and_then(|_| fs::File::open(&partial)?.sync_all()).and_then(|_| fs::rename(&partial, &target));
    if let Err(error) = copied {
        let _ = fs::remove_file(&partial);
        return Err(Error::io("collect", source, &error));
    }
    Ok((target, true))
}

/// Saves `project` to `destination`.
///
/// * `base` is the project file the document's relative media paths (if any) belong to: the
///   file it was opened from. `None` uses `destination`.
/// * With `collect_media`, every media file that exists is copied into `<destination>.media`
///   (once: a file already in that folder, or already carrying its hash prefix, keeps its name)
///   and the stored paths become relative. Files that are missing keep their path, so they can
///   be relinked later. A frozen print is never collected or saved with changed content.
///
/// The project file is written last and atomically; if anything fails before that, the previous
/// file is untouched. The source media files are never modified.
pub fn write_project(project: &Project, destination: &Path, base: Option<&Path>, collect_media: bool) -> Result<Written> {
    validate(project)?;
    let base = base.unwrap_or(destination);
    let frozen = media::frozen_print_hashes(project)?;
    verify_frozen_prints(&frozen, base)?;

    let mut stored = project.clone();
    let mut live = project.clone();
    let mut collected = 0;
    if collect_media {
        let folder = media::media_folder(destination);
        let folder_name = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        // path as written in the project -> (stored spelling, live spelling)
        let mut replacements: BTreeMap<String, (String, String)> = BTreeMap::new();
        for path in media::media_paths(project) {
            let Some(source) = media::resolve(&path, Some(base)) else { continue };
            if !source.exists() {
                continue;
            }
            // A missing, empty or unreadable print stays repairable: it is not collected as a valid print.
            if frozen.iter().any(|(known, _)| *known == path) && !is_readable_nonempty_file(&source) {
                continue;
            }
            fs::create_dir_all(&folder).map_err(|error| Error::io("create", &folder, &error))?;
            let digest = hash_file(&source, DEFAULT_HASH_LIMIT, None)?;
            let file_name = source.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let already_inside = match (source.parent().and_then(|parent| fs::canonicalize(parent).ok()), fs::canonicalize(&folder).ok()) {
                (Some(parent), Some(folder)) => parent == folder,
                _ => false,
            };
            let target = if already_inside {
                folder.join(&file_name)
            } else {
                let prefix = format!("{}-", &digest[..16]);
                let name = if file_name.starts_with(&prefix) { file_name.clone() } else { format!("{prefix}{file_name}") };
                let (target, copied) = collect_file(&source, &folder, &name)?;
                if copied {
                    collected += 1;
                }
                if hash_file(&target, DEFAULT_HASH_LIMIT, None)? != digest {
                    return Err(Error::invalid(format!("Collected media has different content: {name}. The existing file was not overwritten.")));
                }
                target
            };
            let target_name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            replacements.insert(path, (format!("{folder_name}/{target_name}"), target.to_string_lossy().into_owned()));
        }
        media::map_media_paths(&mut stored, &mut |path| replacements.get(path).map(|(stored, _)| stored.clone()));
        media::map_media_paths(&mut live, &mut |path| replacements.get(path).map(|(_, live)| live.clone()));
    }
    validate(&stored)?;
    validate(&live)?;
    verify_frozen_prints(&media::frozen_print_hashes(&live)?, destination)?;
    let mut text = stored.to_json_pretty();
    text.push('\n');
    write_atomic(destination, text.as_bytes())?;
    Ok(Written { project: live, file_sha256: media::sha256_hex(text.as_bytes()), collected })
}
