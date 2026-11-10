//! Kits: SuperTracker's `.stkit` packages and the browsing metadata of Maschine `.mxgrp` groups.
//!
//! A `.stkit` kit is a folder:
//!
//! ```text
//! My kit.stkit/
//!   kit.json        small header the library indexes: format, version, name, type, tags, soundCount …
//!   project.json    the kit itself: a group project (see [`group`])
//!   Samples/        collected samples, named <24 hex digits of their SHA-256>-<original name>
//!   Preview.wav     optional audio preview (the header's previewFile)
//! ```
//!
//! Sample paths inside `project.json` are relative to the package when the samples were
//! collected, absolute otherwise; [`read`] returns them absolute. Saving stages the new package
//! next to the destination and publishes it by renaming, so a failed or cancelled save leaves
//! the destination as it was. A sample already collected under its content hash keeps its
//! name, so saving a kit over itself leaves the paths an open session uses valid.
//!
//! This module only reads and writes files: it never loads a plug-in or plays audio.

pub mod catalog;
pub mod group;

use crate::error::{Error, Result};
use crate::util::{components, extension, file_name, is_readable_file, iso8601_now, parent, stem, unique_id};
use group::SoundSummary;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use st_project::media::{map_media_paths, normalize};
use st_project::{Cancel, Project};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// File extensions the library treats as kits.
pub const EXTENSIONS: [&str; 2] = ["stkit", "mxgrp"];
/// The header version this crate reads and writes.
pub const FORMAT_VERSION: u64 = 2;
/// Largest `project.json`.
pub const MAXIMUM_PROJECT_BYTES: u64 = 64 * 1024 * 1024;
/// Largest `kit.json`.
pub const MAXIMUM_METADATA_BYTES: u64 = 256 * 1024;
/// The most sample data one kit collects.
pub const MAXIMUM_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const PREVIEW_EXTENSIONS: [&str; 7] = ["ogg", "wav", "aif", "aiff", "mp3", "m4a", "caf"];

/// The folder kits are saved to when the caller configures none: `Documents/SuperTracker/Kits`
/// in the user's home folder.
pub fn default_user_directory() -> Option<PathBuf> {
    crate::util::home_dir().map(|home| home.join("Documents").join("SuperTracker").join("Kits"))
}

// ---------------------------------------------------------------------------------------------
// Maschine group headers

/// What Maschine's own browser shows for a group, read from the start of the `.mxgrp` file
/// (NI SoundInfo): name, author, vendor, bank chain (product, bank, sub-bank) and type paths.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GroupHeader {
    /// Group name.
    pub name: String,
    /// Author.
    pub author: String,
    /// Vendor.
    pub vendor: String,
    /// Comment.
    pub comment: String,
    /// Product, bank, sub-bank.
    pub chain: Vec<String>,
    /// Type paths such as `["Kits", "Hip Hop"]`; only the most specific paths are kept.
    pub types: Vec<Vec<String>>,
}

/// Maschine 3 shows some older sub-types under their current names.
const GROUP_SUBTYPE_ALIASES: [(&str, &str); 1] = [("Urban Kit", "Hip Hop")];
const SOUND_INFO_MARKER: [u8; 16] = [b'D', b'S', b'I', b'N', 0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0];

/// Reads the header of the group file at `path`. Reads at most 64 KiB; never the archive.
pub fn group_header_at(path: &str) -> Option<GroupHeader> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(65_536);
    file.take(65_536).read_to_end(&mut bytes).ok()?;
    group_header(&bytes)
}

/// Finds and reads the SoundInfo item in the first 64 KiB of a group file. Accepts only
/// well-formed counts and UTF-16 strings; anything else gives `None`.
pub fn group_header(data: &[u8]) -> Option<GroupHeader> {
    let bytes = &data[..data.len().min(65_536)];
    if bytes.len() <= SOUND_INFO_MARKER.len() {
        return None;
    }
    let start = bytes.windows(SOUND_INFO_MARKER.len()).position(|window| window == SOUND_INFO_MARKER)?;
    // The item header (20 bytes) and its terminator (16 bytes) precede the SoundInfo fields.
    struct Cursor<'a> {
        bytes: &'a [u8],
        position: usize,
    }
    impl Cursor<'_> {
        fn word(&mut self) -> Option<usize> {
            let word = self.bytes.get(self.position..self.position + 4)?;
            self.position += 4;
            Some(u32::from_le_bytes([word[0], word[1], word[2], word[3]]) as usize)
        }
        fn text(&mut self) -> Option<String> {
            let count = self.word()?;
            if count > 1024 {
                return None;
            }
            let raw = self.bytes.get(self.position..self.position + count * 2)?;
            self.position += count * 2;
            let units: Vec<u16> = raw.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
            Some(String::from_utf16_lossy(&units))
        }
        fn list(&mut self) -> Option<Vec<String>> {
            let count = self.word()?;
            if count > 64 {
                return None;
            }
            (0..count).map(|_| self.text()).collect()
        }
    }
    let mut cursor = Cursor { bytes, position: start + 36 };
    if cursor.word()? != 1 {
        return None;
    }
    for _ in 0..3 {
        cursor.word()?;
    }
    let (name, author, vendor, comment) = (cursor.text()?, cursor.text()?, cursor.text()?, cursor.text()?);
    for _ in 0..9 {
        cursor.word()?;
    }
    let chain = cursor.list()?;
    let types = cursor.list()?;
    let clean = |value: &str| value.trim().to_string();
    let mut paths: Vec<Vec<String>> = types
        .iter()
        .filter_map(|path| {
            let parts: Vec<String> = path.split("\\:").map(clean).filter(|part| !part.is_empty()).collect();
            if parts.is_empty() {
                return None;
            }
            Some(
                parts
                    .into_iter()
                    .enumerate()
                    .map(|(index, part)| match GROUP_SUBTYPE_ALIASES.iter().find(|(old, _)| index == 1 && *old == part) {
                        Some((_, current)) => current.to_string(),
                        None => part,
                    })
                    .collect(),
            )
        })
        .collect();
    // "\:Kits" and "\:Kits\:Hip Hop" both arrive; keep only the most specific paths.
    let all = paths.clone();
    paths.retain(|path| !all.iter().any(|other| other.len() > path.len() && other[..path.len()] == path[..]));
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    Some(GroupHeader { name, author, vendor, comment, chain: chain.iter().map(|value| clean(value)).filter(|value| !value.is_empty()).collect(), types: paths })
}

// ---------------------------------------------------------------------------------------------
// Metadata

/// Whether a group can be loaded; kits are inspected when they are opened.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Compatibility {
    /// `uninspected` until the group is opened.
    pub status: String,
    /// What that means.
    pub summary: String,
}

/// What the library knows about a kit or group without opening its project.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KitMetadata {
    /// The kit package or group file.
    pub path: String,
    /// Display name.
    pub name: String,
    /// Always `kit`.
    pub kind: String,
    /// `supertracker-kit` or `maschine-group`.
    pub kit_format: String,
    /// Whether it is one of the user's own kits (a `.stkit` package).
    pub user: bool,
    /// The kit type shown in the browser (`Kit` unless the header says otherwise).
    #[serde(rename = "type")]
    pub kit_type: String,
    /// Tags: the kit's own, or for a group the folders below its library root.
    pub tags: Vec<String>,
    /// Always true for a kit that was just read.
    pub available: bool,
    /// The expansion (pack) the kit belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expansion: Option<String>,
    /// Where the fields came from, in words.
    pub metadata_source: String,
    /// The kit's stable identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Vendor (groups).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Loaded pads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_count: Option<u64>,
    /// When the kit was first saved (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// `User` for kits saved from a session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Bank chain of a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    /// Second bank chain position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank: Option<String>,
    /// Third bank chain position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_bank: Option<String>,
    /// Types of a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<Vec<String>>,
    /// Sub-types of a group as `Type\u{1f}Sub-type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtypes: Option<Vec<String>>,
    /// Load compatibility of a group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<Compatibility>,
    /// An audio preview of the kit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_path: Option<String>,
}

fn failure(message: impl Into<String>) -> Error {
    Error::invalid(message)
}

fn readable_audio(path: &Path) -> bool {
    let known = path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| PREVIEW_EXTENSIONS.contains(&extension.to_lowercase().as_str()));
    known && fs::metadata(path).map(|metadata| metadata.is_file() && metadata.len() > 0).unwrap_or(false) && is_readable_file(path)
}

/// The audio preview of a kit: `Preview.<ext>` inside a package, or a file named after the kit
/// next to it or in a `.previews` or `Previews` folder. Exact names only; folders are not listed.
pub fn preview_path(path: &str) -> Option<String> {
    let kit = Path::new(path);
    let folder = kit.parent()?;
    let (file, stem) = (file_name(path), stem(path));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if extension(path) == "stkit" {
        candidates.extend(PREVIEW_EXTENSIONS.iter().map(|extension| kit.join(format!("Preview.{extension}"))));
    }
    for directory in [folder.join(".previews"), folder.join("Previews"), folder.to_path_buf()] {
        for extension in PREVIEW_EXTENSIONS {
            candidates.push(directory.join(format!("{file}.{extension}")));
            candidates.push(directory.join(format!("{stem}.{extension}")));
        }
    }
    candidates.iter().find(|candidate| readable_audio(candidate)).map(|candidate| candidate.to_string_lossy().into_owned())
}

/// A JSON object from a package member of at most `limit` bytes.
fn read_object(path: &Path, limit: u64) -> Result<Map<String, Value>> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let size = fs::metadata(path).ok().filter(|metadata| metadata.is_file()).map(|metadata| metadata.len());
    match size {
        Some(size) if size > 0 && size <= limit => {}
        _ => return Err(failure(format!("Kit data is missing or exceeds its supported size: {name}"))),
    }
    let mut data = Vec::new();
    fs::File::open(path).and_then(|file| file.take(limit + 1).read_to_end(&mut data)).map_err(|error| Error::io("read", path, &error))?;
    if data.len() as u64 > limit {
        return Err(failure(format!("Kit data is missing or exceeds its supported size: {name}")));
    }
    match serde_json::from_slice::<Value>(&data) {
        Ok(Value::Object(object)) => Ok(object),
        _ => Err(failure(format!("Malformed kit JSON: {name}"))),
    }
}

/// The file `relative` names inside `folder`, with symbolic links resolved. Fails when the
/// path is absolute or leaves the folder, also through a link.
fn inside(folder: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || st_project::media::is_absolute(relative) || relative.contains('\0') {
        return Err(failure("Invalid package-relative kit path."));
    }
    let folder = fs::canonicalize(folder).map_err(|error| Error::io("read", folder, &error))?;
    let lexical = normalize(&folder.join(relative));
    // Resolve links of the deepest part that exists; the rest is appended as written.
    let mut existing = lexical.clone();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    let resolved = loop {
        match fs::canonicalize(&existing) {
            Ok(resolved) => break resolved,
            Err(_) => match (existing.parent().map(Path::to_path_buf), existing.file_name().map(|name| name.to_os_string())) {
                (Some(up), Some(name)) => {
                    remainder.push(name);
                    existing = up;
                }
                _ => break existing.clone(),
            },
        }
    };
    let target = remainder.iter().rev().fold(resolved, |path, name| path.join(name));
    if target == folder || !target.starts_with(&folder) {
        return Err(failure(format!("A kit path escapes its package: {relative}")));
    }
    Ok(target)
}

fn validate_header(header: &Map<String, Value>) -> Result<()> {
    if header.get("format").and_then(Value::as_str) != Some("supertracker-kit") || header.get("projectFile").and_then(Value::as_str) != Some("project.json") {
        return Err(failure("Unsupported or malformed SuperTracker kit format."));
    }
    match header.get("version") {
        Some(Value::Number(version)) if version.as_f64() == Some(FORMAT_VERSION as f64) => {}
        Some(Value::Number(version)) if version.as_f64() == Some(1.0) => {
            return Err(failure("This kit was saved in the retired kit-lane layout (version 1) and cannot be read. Save it again from a current session."))
        }
        _ => return Err(failure("Unsupported or malformed SuperTracker kit format.")),
    }
    for key in ["name", "type", "id", "author", "expansion", "source"] {
        match header.get(key) {
            None => {}
            Some(Value::String(text)) if text.chars().count() <= 256 => {}
            Some(_) => return Err(failure(format!("Invalid kit metadata field: {key}"))),
        }
    }
    match header.get("tags") {
        None => {}
        Some(Value::Array(tags)) if tags.len() <= 64 && tags.iter().all(|tag| tag.as_str().is_some_and(|tag| tag.chars().count() <= 80)) => {}
        Some(_) => return Err(failure("Kit tags exceed the supported limit.")),
    }
    Ok(())
}

/// The browsing metadata of the kit package or Maschine group at `path`.
///
/// With `root` (the library folder being scanned) the folders between the root and the kit
/// become its tags and its expansion. Nothing but the small header is read: never collected
/// media, project state or the group archive. Symbolic links are not followed.
pub fn metadata(path: &str, root: Option<&str>) -> Result<KitMetadata> {
    let path = crate::util::normalized(path);
    let kind = extension(&path);
    if !EXTENSIONS.contains(&kind.as_str()) {
        return Err(failure("Choose a .stkit kit or Maschine .mxgrp group."));
    }
    let file = fs::symlink_metadata(&path).map_err(|error| Error::io("read", Path::new(&path), &error))?;
    if file.file_type().is_symlink() {
        return Err(failure("Kit aliases are not followed while indexing."));
    }
    let own = kind == "stkit";
    let mut result = KitMetadata {
        name: stem(&path).to_string(),
        kind: "kit".to_string(),
        kit_format: if own { "supertracker-kit" } else { "maschine-group" }.to_string(),
        user: own,
        kit_type: "Kit".to_string(),
        tags: Vec::new(),
        available: true,
        expansion: None,
        metadata_source: String::new(),
        id: None,
        author: None,
        vendor: None,
        description: None,
        sound_count: None,
        created_at: None,
        source: None,
        product: None,
        bank: None,
        sub_bank: None,
        types: None,
        subtypes: None,
        compatibility: None,
        preview_path: None,
        path: path.clone(),
    };
    if let Some(root) = root {
        let root = crate::util::normalized(root);
        if crate::util::is_inside(&path, &root) && path != root {
            let below = components(&path[root.len()..]);
            let folders = &below[..below.len().saturating_sub(1)];
            result.tags = folders.iter().filter(|folder| !["groups", "kits"].contains(&folder.to_lowercase().as_str())).map(|folder| folder.to_string()).collect();
            result.expansion = Some(folders.first().map(|folder| folder.to_string()).unwrap_or_else(|| file_name(&root).to_string()));
        }
    }
    if !own {
        // A root inside an expansion (".../Anima Ascent Library/Groups") must not name the expansion
        // "Kits" or "Groups": it is the nearest "… Library" folder, or the folder that holds Groups.
        let parents = components(parent(&path));
        if let Some(library) = parents.iter().rev().find(|folder| folder.ends_with(" Library")) {
            result.expansion = Some(library.to_string());
        } else if let Some(index) = parents.iter().rposition(|folder| folder.to_lowercase() == "groups").filter(|index| *index > 0) {
            result.expansion = Some(parents[index - 1].to_string());
        }
    }
    if own {
        if !file.is_dir() {
            return Err(failure("A SuperTracker kit must be a .stkit package directory."));
        }
        let package = Path::new(&path);
        let header = read_object(&inside(package, "kit.json")?, MAXIMUM_METADATA_BYTES)?;
        validate_header(&header)?;
        let text = |key: &str| header.get(key).and_then(Value::as_str).map(String::from);
        result.id = text("id");
        if let Some(name) = text("name") {
            result.name = name;
        }
        if let Some(kit_type) = text("type") {
            result.kit_type = kit_type;
        }
        if let Some(tags) = header.get("tags").and_then(Value::as_array) {
            result.tags = tags.iter().filter_map(Value::as_str).map(String::from).collect();
        }
        result.author = text("author");
        result.description = text("description");
        if let Some(expansion) = text("expansion") {
            result.expansion = Some(expansion);
        }
        result.sound_count = header.get("soundCount").and_then(Value::as_u64);
        result.created_at = text("createdAt");
        result.source = text("source");
        result.metadata_source = "SuperTracker kit metadata".to_string();
        if let Some(relative) = header.get("previewFile").and_then(Value::as_str) {
            let preview = inside(package, relative)?;
            if readable_audio(&preview) {
                result.preview_path = Some(preview.to_string_lossy().into_owned());
            }
        }
    } else {
        if !file.is_file() {
            return Err(failure("A Maschine group must be a regular .mxgrp file."));
        }
        result.metadata_source = "filename and folders".to_string();
        if let Some(header) = group_header_at(&path) {
            result.metadata_source = "Maschine group header".to_string();
            result.author = (!header.author.is_empty()).then(|| header.author.clone());
            result.vendor = (!header.vendor.is_empty()).then(|| header.vendor.clone());
            result.product = header.chain.first().cloned();
            result.bank = header.chain.get(1).cloned();
            result.sub_bank = header.chain.get(2).cloned();
            if let Some(product) = header.chain.first() {
                result.expansion = Some(product.clone());
            }
            let mut types: Vec<String> = header.types.iter().map(|path| path[0].clone()).collect::<HashSet<_>>().into_iter().collect();
            types.sort();
            result.types = Some(types);
            result.subtypes = Some(header.types.iter().filter(|path| path.len() > 1).map(|path| path[..2].join("\u{1f}")).collect());
        }
        result.compatibility = Some(Compatibility {
            status: "uninspected".to_string(),
            summary: "Inspect the group before loading; installed plug-ins and unsupported settings are checked separately.".to_string(),
        });
    }
    if result.preview_path.is_none() {
        result.preview_path = preview_path(&path);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------------------------
// Reading and writing packages

/// Visits the media paths of a group project (instrument samples and zones). `builtin:`
/// references and plug-in state are never touched. Fails with the first error `transform` returns.
fn map_media(project: &Value, transform: &mut dyn FnMut(&str) -> Result<String>) -> Result<Value> {
    let mut typed = Project::from_value(project.clone())?;
    let mut failure: Option<Error> = None;
    map_media_paths(&mut typed, &mut |path| {
        if failure.is_some() {
            return None;
        }
        match transform(path) {
            Ok(replacement) => Some(replacement),
            Err(error) => {
                failure = Some(error);
                None
            }
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(typed.to_value()),
    }
}

/// An opened kit package.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KitContents {
    /// The package.
    pub path: String,
    /// Its browsing metadata.
    pub metadata: KitMetadata,
    /// The kit as a group project, sample paths absolute.
    #[serde(rename = "groupProject")]
    pub project: Value,
    /// The pads.
    pub sounds: Vec<SoundSummary>,
    /// Samples that are missing or unreadable, and other notes.
    pub warnings: Vec<String>,
}

/// Opens the `.stkit` package at `path`.
pub fn read(path: &str) -> Result<KitContents> {
    let path = crate::util::normalized(path);
    if extension(&path) != "stkit" {
        return Err(failure("Maschine groups require the Maschine group importer."));
    }
    let details = metadata(&path, None)?;
    let package = Path::new(&path);
    let project = Value::Object(read_object(&inside(package, "project.json")?, MAXIMUM_PROJECT_BYTES)?);
    group::validate(&project)?;
    let mut warnings: Vec<String> = Vec::new();
    let mut checked: HashSet<PathBuf> = HashSet::new();
    let project = map_media(&project, &mut |media| {
        let source = if st_project::media::is_absolute(media) { normalize(Path::new(media)) } else { inside(package, media)? };
        if checked.insert(source.clone()) && !is_readable_file(&source) && warnings.len() < 128 {
            warnings.push(format!("Missing or unreadable sample: {}", source.display()));
        }
        Ok(source.to_string_lossy().into_owned())
    })?;
    Ok(KitContents { sounds: group::sound_summaries(&project), path, metadata: details, project, warnings })
}

/// How [`save`] writes a kit.
#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    /// Copy every sample into the package (`Samples/`). Without it the kit refers to the
    /// samples where they are.
    pub copy_samples: bool,
    /// The project file relative sample paths are relative to. Absent: relative paths are refused.
    pub relative_to: Option<PathBuf>,
    /// Replace an existing kit package at the destination.
    pub overwrite: bool,
    /// Cancels the save; the destination is left as it was.
    pub cancel: Option<Cancel>,
}

/// What [`save`] wrote.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedKit {
    /// Always true.
    pub saved: bool,
    /// The package.
    pub path: String,
    /// The header that was written, with `path`, `kind`, `kitFormat`, `user` and `previewPath`.
    pub metadata: Value,
    /// Distinct samples copied into the package.
    pub copied_samples: usize,
    /// Bytes of collected media (samples and preview).
    pub media_bytes: u64,
    /// Notes for the user.
    pub warnings: Vec<String>,
}

struct Stage(PathBuf);

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn check_cancel(cancel: &Option<Cancel>) -> Result<()> {
    match cancel {
        Some(cancel) if cancel.is_cancelled() => Err(Error::Cancelled),
        _ => Ok(()),
    }
}

/// Copies `source` into `folder` under its content-hash name and returns the copy.
fn copy_file(source: &Path, folder: &Path, remaining: &mut u64, cancel: &Option<Cancel>) -> Result<PathBuf> {
    let over_budget = || failure(format!("Sample is missing, empty or exceeds the kit's 4 GiB media budget: {}", source.display()));
    let before = fs::metadata(source).map_err(|_| over_budget())?;
    if !before.is_file() || before.len() == 0 || before.len() > *remaining {
        return Err(over_budget());
    }
    let temporary = folder.join(format!(".copy-{}", unique_id()));
    let copied = (|| -> Result<(String, u64)> {
        let mut input = fs::File::open(source).map_err(|error| Error::io("read", source, &error))?;
        let mut output = fs::File::create(&temporary).map_err(|_| failure("Could not create collected kit media."))?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut count = 0u64;
        loop {
            let read = input.read(&mut buffer).map_err(|error| Error::io("read", source, &error))?;
            if read == 0 {
                break;
            }
            check_cancel(cancel)?;
            count += read as u64;
            if count > *remaining {
                return Err(failure("Kit media exceeds the 4 GiB collection budget."));
            }
            hash.update(&buffer[..read]);
            output.write_all(&buffer[..read]).map_err(|error| Error::io("write", &temporary, &error))?;
        }
        output.sync_all().map_err(|error| Error::io("write", &temporary, &error))?;
        let after = fs::metadata(source).map_err(|error| Error::io("read", source, &error))?;
        if count != before.len() || after.len() != before.len() || after.modified().ok() != before.modified().ok() {
            return Err(failure(format!("A source sample changed while saving the kit: {}", source.display())));
        }
        let digest: String = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        Ok((digest, count))
    })();
    let (digest, count) = match copied {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    };
    // A sample already collected under this content hash keeps its name, so saving a kit over
    // itself leaves the paths an open session still uses valid.
    let prefix = format!("{}-", &digest[..24]);
    let source_text = source.to_string_lossy();
    let mut name = stem(&source_text);
    while let Some(rest) = name.strip_prefix(&prefix) {
        name = rest;
    }
    let short: String = name.chars().take(100).collect();
    let target = folder.join(format!("{prefix}{short}.{}", extension(&source_text)));
    if target.exists() {
        let _ = fs::remove_file(&temporary);
    } else {
        fs::rename(&temporary, &target).map_err(|error| Error::io("write", &target, &error))?;
        *remaining -= count;
    }
    Ok(target)
}

/// Writes `project` (a group project, see [`group`]) as the kit package `destination`.
///
/// `header` supplies the kit header fields (`name`, `type`, `tags`, `author`, `description`,
/// `expansion`, `source`, `id`, `createdAt`); `previewPath` names an audio file to copy in as
/// the kit's preview. Missing fields get defaults. The package is staged next to the
/// destination and published by renaming: on any failure or cancellation the destination is
/// untouched and the stage is removed. `project` is not modified.
pub fn save(project: &Value, header: &Map<String, Value>, destination: &str, options: &SaveOptions) -> Result<SavedKit> {
    group::validate(project)?;
    let destination_text = crate::util::normalized(destination);
    let destination = Path::new(&destination_text);
    if extension(&destination_text) != "stkit" {
        return Err(failure("Use the .stkit extension for a SuperTracker kit."));
    }
    let folder = destination.parent().filter(|folder| folder.is_dir()).ok_or_else(|| failure("Choose an existing kit destination folder."))?;
    let existing = fs::symlink_metadata(destination).ok();
    if let Some(existing) = &existing {
        if !options.overwrite {
            return Err(failure("A kit already exists at this destination. Choose another name or explicitly replace it."));
        }
        if !existing.is_dir() || existing.file_type().is_symlink() {
            return Err(failure("Only an existing regular .stkit package can be replaced."));
        }
        metadata(&destination_text, None)?; // Never replace an unrelated directory with a .stkit suffix.
    }
    check_cancel(&options.cancel)?;
    let stage = Stage(folder.join(format!(".supertracker-kit-{}", unique_id())));
    fs::create_dir(&stage.0).map_err(|error| Error::io("create", &stage.0, &error))?;

    let mut header = header.clone();
    header.insert("format".to_string(), json!("supertracker-kit"));
    header.insert("version".to_string(), json!(FORMAT_VERSION));
    header.insert("projectFile".to_string(), json!("project.json"));
    if header.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
        header.insert("id".to_string(), json!(unique_id()));
    }
    if header.get("name").and_then(Value::as_str).is_none_or(|name| name.trim().is_empty()) {
        header.insert("name".to_string(), json!(stem(&destination_text)));
    }
    header.entry("type").or_insert(json!("Kit"));
    header.entry("tags").or_insert(json!([]));
    header.insert("soundCount".to_string(), json!(group::loaded_sound_count(project)));
    header.entry("createdAt").or_insert(json!(iso8601_now()));
    header.insert("collectedSamples".to_string(), json!(options.copy_samples));
    let supplied_preview = header.remove("previewPath").and_then(|value| value.as_str().map(String::from));
    header.remove("previewFile");
    validate_header(&header)?;

    let mut remaining = MAXIMUM_MEDIA_BYTES;
    let mut copied: BTreeMap<PathBuf, String> = BTreeMap::new();
    let canonical_destination = fs::canonicalize(destination).ok();
    let stored = map_media(project, &mut |media| {
        check_cancel(&options.cancel)?;
        let source = if st_project::media::is_absolute(media) {
            normalize(Path::new(media))
        } else if let Some(folder) = options.relative_to.as_deref().and_then(Path::parent) {
            normalize(&folder.join(media))
        } else {
            return Err(failure(format!("A relative sample reference needs its original project location: {media}")));
        };
        if !options.copy_samples {
            // Replacing this package deletes the members its new snapshot would refer to.
            let resolved = fs::canonicalize(&source).unwrap_or_else(|_| source.clone());
            if canonical_destination.as_ref().is_some_and(|package| resolved.starts_with(package)) {
                return Err(failure(format!("This sample belongs to the kit being replaced. Collect samples or save to a different kit: {}", source.display())));
            }
            return Ok(source.to_string_lossy().into_owned());
        }
        if let Some(prior) = copied.get(&source) {
            return Ok(prior.clone());
        }
        let samples = stage.0.join("Samples");
        fs::create_dir_all(&samples).map_err(|error| Error::io("create", &samples, &error))?;
        let target = copy_file(&source, &samples, &mut remaining, &options.cancel)?;
        let relative = format!("Samples/{}", target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default());
        copied.insert(source, relative.clone());
        Ok(relative)
    })?;
    group::validate(&stored)?;
    if let Some(preview) = supplied_preview.filter(|preview| !preview.is_empty()) {
        let source = Path::new(&preview);
        if !readable_audio(source) {
            return Err(failure("The requested kit preview is unavailable or has an unsupported audio extension."));
        }
        // The preview keeps the conventional name, so the browser finds it without the header too.
        let target = stage.0.join(format!("Preview.{}", extension(&preview)));
        let size = fs::copy(source, &target).map_err(|error| Error::io("copy", source, &error))?;
        if size > remaining {
            return Err(failure("Kit media exceeds the 4 GiB collection budget."));
        }
        remaining -= size;
        header.insert("previewFile".to_string(), json!(target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()));
    }
    let payload = serde_json::to_vec(&stored).map_err(|_| failure("Kit project state must contain JSON values."))?;
    if payload.len() as u64 > MAXIMUM_PROJECT_BYTES {
        return Err(failure("Kit project state exceeds the 64 MiB limit."));
    }
    let header_data = serde_json::to_vec_pretty(&Value::Object(header.clone())).map_err(|_| failure("Kit metadata must contain JSON values."))?;
    if header_data.len() as u64 > MAXIMUM_METADATA_BYTES {
        return Err(failure("Kit metadata exceeds the 256 KiB limit."));
    }
    fs::write(stage.0.join("project.json"), payload).map_err(|error| Error::io("write", &stage.0.join("project.json"), &error))?;
    fs::write(stage.0.join("kit.json"), header_data).map_err(|error| Error::io("write", &stage.0.join("kit.json"), &error))?;
    check_cancel(&options.cancel)?;

    // Publish. A replaced package is moved aside first and restored if the new one cannot take its place.
    let preserved = |error: std::io::Error| failure(format!("Could not publish the kit; the prior destination was preserved: {}", crate::error::io_reason(&error)));
    if existing.is_some() {
        let aside = folder.join(format!(".supertracker-kit-old-{}", unique_id()));
        fs::rename(destination, &aside).map_err(preserved)?;
        if let Err(error) = fs::rename(&stage.0, destination) {
            let _ = fs::rename(&aside, destination);
            return Err(preserved(error));
        }
        let _ = fs::remove_dir_all(&aside);
    } else {
        if destination.exists() {
            return Err(failure("A kit already exists at this destination. Choose another name or explicitly replace it."));
        }
        fs::rename(&stage.0, destination).map_err(preserved)?;
    }

    let mut published = header;
    published.insert("path".to_string(), json!(destination_text));
    published.insert("kind".to_string(), json!("kit"));
    published.insert("kitFormat".to_string(), json!("supertracker-kit"));
    published.insert("user".to_string(), json!(true));
    if let Some(preview) = published.get("previewFile").and_then(Value::as_str).map(String::from) {
        published.insert("previewPath".to_string(), json!(destination.join(preview).to_string_lossy()));
    }
    Ok(SavedKit {
        saved: true,
        path: destination_text.clone(),
        metadata: Value::Object(published),
        copied_samples: copied.len(),
        media_bytes: MAXIMUM_MEDIA_BYTES - remaining,
        warnings: Vec::new(),
    })
}
