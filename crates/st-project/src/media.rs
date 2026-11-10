//! Media references of a project: which fields are file paths, how they resolve, and content
//! hashes.
//!
//! File ownership is defined by the schema, not by a key name or an extension: plug-in state and
//! unknown fields can hold path-like strings that mean something else and are never touched.
//! The media fields are:
//!
//! * `instruments[].samplePath`, `instruments[].zones[].samplePath` and `.path`
//! * `audioClips[].path` and `.samplePath`, `audioClips[].takeSet.takes[].path`
//! * `tracks[].frozen.path`, `.patternPrints[].path`, `.patternPrints[].loop.path`
//! * `mediaManifest[].path`
//!
//! `builtin:` references name application content and are not media. A path is absolute while
//! the project is open and relative to the project file's folder inside the file.

use crate::error::{Error, Result};
use crate::model::instrument::is_builtin;
use crate::model::Project;
use crate::Cancel;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// The most bytes [`hash_file`] reads by default (4 GiB), as the Swift host's save budget.
pub const DEFAULT_HASH_LIMIT: u64 = 4_294_967_296;

/// Rewrites every media path of `project`. `transform` returns the new path, or `None` to keep
/// the path. Empty paths and `builtin:` references are skipped.
pub fn map_media_paths(project: &mut Project, transform: &mut dyn FnMut(&str) -> Option<String>) {
    let mut apply = |field: &mut Option<String>| {
        if let Some(path) = field {
            if !path.is_empty() && !is_builtin(path) {
                if let Some(replacement) = transform(path) {
                    *path = replacement;
                }
            }
        }
    };
    for instrument in project.instruments.iter_mut().flatten() {
        apply(&mut instrument.sample_path);
        for zone in instrument.zones.iter_mut().flatten() {
            apply(&mut zone.path);
            apply(&mut zone.sample_path);
        }
    }
    for clip in project.audio_clips.iter_mut().flatten() {
        apply(&mut clip.path);
        apply(&mut clip.sample_path);
        if let Some(takes) = clip.take_set.as_mut().and_then(|set| set.takes.as_mut()) {
            for take in takes {
                apply(&mut take.path);
            }
        }
    }
    for track in project.tracks.iter_mut().flatten() {
        if let Some(frozen) = &mut track.frozen {
            apply(&mut frozen.path);
            for print in frozen.pattern_prints.iter_mut().flatten() {
                apply(&mut print.path);
                if let Some(loop_print) = &mut print.loop_print {
                    apply(&mut loop_print.path);
                }
            }
        }
    }
    for entry in project.media_manifest.iter_mut().flatten() {
        apply(&mut entry.path);
    }
}

/// Every distinct media path of `project`, in document order.
pub fn media_paths(project: &Project) -> Vec<String> {
    let mut copy = project.clone();
    let mut paths: Vec<String> = Vec::new();
    map_media_paths(&mut copy, &mut |path| {
        if !paths.iter().any(|known| known == path) {
            paths.push(path.to_string());
        }
        None
    });
    paths
}

/// The printed files of frozen tracks with the SHA-256 recorded when they were printed:
/// `(path, sha256)`. A print without a recorded hash is not listed.
///
/// Fails when a recorded hash is malformed or two records disagree about one path.
pub fn frozen_print_hashes(project: &Project) -> Result<Vec<(String, String)>> {
    let mut result: Vec<(String, String)> = Vec::new();
    let mut include = |path: &Option<String>, hash: &Option<String>| -> Result<()> {
        let (Some(path), Some(hash)) = (path, hash) else { return Ok(()) };
        if path.is_empty() || hash.is_empty() {
            return Ok(());
        }
        if !is_sha256(hash) {
            return Err(Error::invalid("Frozen audio has an invalid captured SHA-256. Restore its metadata or Unfreeze before collecting it."));
        }
        let hash = hash.to_ascii_lowercase();
        match result.iter().find(|(known, _)| known == path) {
            Some((_, known)) if *known != hash => Err(Error::invalid(format!("Frozen prints disagree about the content of {path}."))),
            Some(_) => Ok(()),
            None => {
                result.push((path.clone(), hash));
                Ok(())
            }
        }
    };
    for track in project.track_list() {
        let Some(frozen) = &track.frozen else { continue };
        include(&frozen.path, &frozen.sha256)?;
        for print in frozen.pattern_prints.iter().flatten() {
            include(&print.path, &print.sha256)?;
            if let Some(loop_print) = &print.loop_print {
                include(&loop_print.path, &loop_print.sha256)?;
            }
        }
    }
    Ok(result)
}

/// Whether `text` is 64 hexadecimal digits.
pub fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether a media path is absolute (`/...` on every platform, or absolute for this platform).
pub fn is_absolute(path: &str) -> bool {
    path.starts_with('/') || Path::new(path).is_absolute()
}

/// Removes `.` and resolves `..` without touching the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// The file a media path names: absolute paths as they are, relative ones against the folder
/// of `project_file`. `None` for an empty path, or a relative path without a project file
/// (a relative path never resolves against the working directory).
pub fn resolve(path: &str, project_file: Option<&Path>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    if is_absolute(path) {
        return Some(normalize(Path::new(path)));
    }
    let folder = project_file?.parent()?;
    Some(normalize(&folder.join(path)))
}

/// Makes every relative media path absolute against the folder of `project_file`.
pub fn make_absolute(project: &mut Project, project_file: &Path) {
    map_media_paths(project, &mut |path| {
        if is_absolute(path) {
            return None;
        }
        resolve(path, Some(project_file)).map(|resolved| resolved.to_string_lossy().into_owned())
    });
}

/// The media folder of a project file: `Song.supertracker` -> `Song.media`.
pub fn media_folder(project_file: &Path) -> PathBuf {
    project_file.with_extension("media")
}

/// SHA-256 of `bytes` as 64 lowercase hexadecimal digits.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Whether `path` is a regular file with at least one byte that can be opened for reading.
pub fn is_readable_nonempty_file(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(metadata) => metadata.is_file() && metadata.len() > 0 && fs::File::open(path).is_ok(),
        Err(_) => false,
    }
}

/// SHA-256 of a file's content, reading at most `max_bytes`.
///
/// Fails when the file is not a regular file, is larger than `max_bytes`, or changes while it
/// is read (size or modification time), so a hash never describes bytes that were never on disk
/// together. `cancel` is checked every mebibyte.
pub fn hash_file(path: &Path, max_bytes: u64, cancel: Option<&Cancel>) -> Result<String> {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let before = fs::metadata(path).map_err(|error| Error::io("read", path, &error))?;
    if !before.is_file() || before.len() > max_bytes {
        return Err(Error::invalid(format!("Media cannot be verified within the size budget: {name}.")));
    }
    let mut file = fs::File::open(path).map_err(|error| Error::io("read", path, &error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total: u64 = 0;
    loop {
        if let Some(cancel) = cancel {
            cancel.check()?;
        }
        let count = file.read(&mut buffer).map_err(|error| Error::io("read", path, &error))?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > max_bytes {
            return Err(Error::invalid(format!("Media grew while it was read: {name}.")));
        }
        hasher.update(&buffer[..count]);
    }
    let after = fs::metadata(path).map_err(|error| Error::io("read", path, &error))?;
    if total != before.len() || after.len() != before.len() || after.modified().ok() != before.modified().ok() {
        return Err(Error::invalid(format!("Media changed while it was read: {name}.")));
    }
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert!(is_sha256(&sha256_hex(b"x")) && !is_sha256("abc") && !is_sha256(&"g".repeat(64)));
    }

    #[test]
    fn paths_resolve_without_the_working_directory() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d.wav")), PathBuf::from("/a/c/d.wav"));
        assert_eq!(resolve("/x/y.wav", None), Some(PathBuf::from("/x/y.wav")));
        assert_eq!(resolve("Song.media/y.wav", Some(Path::new("/music/Song.supertracker"))), Some(PathBuf::from("/music/Song.media/y.wav")));
        assert_eq!(resolve("../shared/y.wav", Some(Path::new("/music/songs/Song.supertracker"))), Some(PathBuf::from("/music/shared/y.wav")));
        assert_eq!(resolve("relative.wav", None), None);
        assert_eq!(resolve("", Some(Path::new("/music/Song.supertracker"))), None);
        assert_eq!(media_folder(Path::new("/music/My Song.supertracker")), PathBuf::from("/music/My Song.media"));
    }

    #[test]
    fn media_fields_are_the_schema_fields_only() {
        let mut project = Project::from_value(json!({
            "instruments": [{ "samplePath": "/a.wav", "zones": [{ "samplePath": "/z.wav", "path": "/z.wav" }, { "path": "builtin:wavetables/Basic/Saw.wav" }], "plugin": { "state": "/not/media.wav" } },
                { "samplePath": "" }, { "samplePath": "builtin:drums/kick/00" }],
            "audioClips": [{ "path": "/c.wav", "samplePath": "/a.wav", "takeSet": { "takes": [{ "path": "/t.wav" }] }, "notes": "/not/media.txt" }],
            "tracks": [{ "frozen": { "path": "/f.wav", "patternPrints": [{ "path": "/h.wav", "loop": { "path": "/l.wav" } }] } }],
            "mediaManifest": [{ "path": "/a.wav" }],
            "audioPlugins": [{ "bundlePath": "/Plug-Ins/x.vst3", "state": "/also/not/media" }]
        }))
        .unwrap();
        assert_eq!(media_paths(&project), ["/a.wav", "/z.wav", "/c.wav", "/t.wav", "/f.wav", "/h.wav", "/l.wav"]);
        map_media_paths(&mut project, &mut |path| Some(format!("moved{path}")));
        let value = project.to_value();
        assert_eq!(value["instruments"][0]["zones"][0], json!({ "samplePath": "moved/z.wav", "path": "moved/z.wav" }));
        assert_eq!(value["instruments"][0]["zones"][1]["path"], "builtin:wavetables/Basic/Saw.wav");
        assert_eq!((&value["instruments"][1]["samplePath"], &value["instruments"][2]["samplePath"]), (&json!(""), &json!("builtin:drums/kick/00")));
        assert_eq!(value["instruments"][0]["plugin"]["state"], "/not/media.wav");
        assert_eq!((&value["audioClips"][0]["notes"], &value["audioPlugins"][0]["bundlePath"]), (&json!("/not/media.txt"), &json!("/Plug-Ins/x.vst3")));
        assert_eq!(value["tracks"][0]["frozen"]["patternPrints"][0]["loop"]["path"], "moved/l.wav");
        assert_eq!(value["mediaManifest"][0]["path"], "moved/a.wav");
    }

    #[test]
    fn frozen_prints_must_agree_about_a_file() {
        let record = |hash: &str| json!({ "path": "/f.wav", "sha256": hash });
        let project = |first: &str, second: &str| Project::from_value(json!({ "tracks": [{ "frozen": record(first) }, { "frozen": record(second) }] })).unwrap();
        let hash = "AB".repeat(32);
        assert_eq!(frozen_print_hashes(&project(&hash, &hash.to_lowercase())).unwrap(), [("/f.wav".to_string(), hash.to_lowercase())]);
        assert_eq!(frozen_print_hashes(&project(&hash, &"cd".repeat(32))).unwrap_err().to_string(), "Frozen prints disagree about the content of /f.wav.");
        assert!(frozen_print_hashes(&project(&hash, "")).is_ok(), "a print without a recorded hash is not listed");
    }

    #[test]
    fn hashing_is_bounded_and_cancellable() {
        let folder = std::env::temp_dir().join(format!("st-project-media-unit-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("data.bin");
        std::fs::write(&file, vec![7u8; 3_000_000]).unwrap();
        assert_eq!(hash_file(&file, DEFAULT_HASH_LIMIT, None).unwrap(), sha256_hex(&vec![7u8; 3_000_000]));
        assert_eq!(hash_file(&file, 100, None).unwrap_err().to_string(), "Media cannot be verified within the size budget: data.bin.");
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(hash_file(&file, DEFAULT_HASH_LIMIT, Some(&cancel)), Err(Error::Cancelled));
        assert!(hash_file(&folder, DEFAULT_HASH_LIMIT, None).is_err(), "a folder is not media");
        assert!(is_readable_nonempty_file(&file) && !is_readable_nonempty_file(&folder) && !is_readable_nonempty_file(&folder.join("absent")));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
