//! Missing-media inspection, search and relinking. Ported from tests/MediaRelinkerTests.swift,
//! tests/MediaRelinkerAliasTests.swift and tests/MediaPortableTests.swift, plus frozen prints.

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_project::files::{read_project, write_project};
use st_project::media::sha256_hex;
use st_project::relink::*;
use st_project::{Cancel, Error, Project};
use std::fs;
use std::path::{Path, PathBuf};

fn put(data: &[u8], path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fixture(sample: &str, zone: Option<&str>, clip: Option<&str>) -> Project {
    let mut instrument = json!({ "id": 0, "name": "Drums", "type": "sample", "samplePath": sample, "cutoff": 1234 });
    if let Some(zone) = zone {
        instrument["zones"] = json!([{ "samplePath": zone, "rootNote": 42, "keyLow": 20, "keyHigh": 80, "volume": 0.7 }, { "path": sample, "rootNote": 64 }]);
    }
    let mut project = json!({
        "version": 1, "name": "Relink fixture", "bpm": 120.0, "linesPerBeat": 4, "tracks": [{ "id": 0 }],
        "instruments": [instrument], "patterns": [{ "id": 0, "length": 64, "steps": [[]] }], "sequence": [0],
        "audioPlugins": [{ "opaqueVendorState": { "path": "/keep/opaque/plugin.wav" } }]
    });
    if let Some(clip) = clip {
        project["audioClips"] = json!([{ "id": "clip-1", "name": "Field recording", "track": 0, "path": clip, "startBeat": 4.0, "durationBeats": 8.0, "sourceOffset": 1.25, "fadeIn": 0.2, "gain": 0.6 }]);
    }
    Project::from_value(project).unwrap()
}

fn search(project: &Project, project_file: Option<&Path>, roots: &[PathBuf], relocations: &[Relocation], limits: &Limits) -> Report {
    find_candidates(project, project_file, roots, relocations, limits, &Cancel::new(), &mut |_| {}).unwrap()
}

/// One selection per missing reference: its first verified candidate.
fn verified_selections(report: &Report) -> Vec<Selection> {
    report
        .missing
        .iter()
        .map(|row| {
            let candidate = row.candidates.as_ref().unwrap().iter().find(|candidate| candidate.verified).expect("a verified candidate");
            Selection { id: row.id.clone(), from: row.path.clone(), to: candidate.path.clone(), allow_different_content: false }
        })
        .collect()
}

fn relink(project: &Project, selections: &[Selection]) -> Result<Applied, Error> {
    apply(project, selections, None, DEFAULT_APPLY_HASH_BYTES, &Cancel::new())
}

fn fingerprinted(project: &Project) -> Project {
    fingerprint(project, None, &Limits::default(), &Cancel::new()).unwrap().project
}

#[test]
fn moved_root_and_transactional_apply() {
    let root = TempDir::new("relink-moved");
    let (original, relocated) = (root.join("Original/Pack"), root.join("Relocated/Pack"));
    let (kick, zone, clip) = (original.join("Kick.wav"), original.join("Zone.aif"), original.join("Recording.caf"));
    let (kick_data, zone_data, clip_data): (Vec<u8>, _, _) = ((0..73).collect(), vec![3u8; 81], vec![5u8; 90]);
    put(&kick_data, &kick);
    put(&zone_data, &zone);
    put(&clip_data, &clip);
    let source = fixture(&text(&kick), Some(&text(&zone)), Some(&text(&clip)));
    let source_before = source.clone();
    let result = fingerprint(&source, None, &Limits::default(), &Cancel::new()).unwrap();
    assert_eq!(result.fingerprinted, 3, "duplicate references are hashed once");
    assert_eq!(source, source_before, "fingerprinting must not change the caller's project");
    let project = result.project;
    let inventory = inspect(&project, None, &Limits::default());
    assert_eq!((inventory.present_count, inventory.missing_count, inventory.reference_count), (4, 0, 4), "sample, two zone aliases and the clip are inventoried");
    assert_eq!(inventory.references.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(), vec!["/instruments/0/samplePath", "/instruments/0/zones/0/samplePath", "/instruments/0/zones/1/path", "/audioClips/0/path"]);
    assert_eq!(inventory.references[1].label, "Drums · Zone 1");

    fs::create_dir_all(relocated.parent().unwrap()).unwrap();
    fs::rename(&original, &relocated).unwrap();
    let before = project.clone();
    let report = search(&project, None, &[relocated.clone()], &[Relocation { from: original.clone(), to: relocated.clone() }], &Limits::default());
    assert_eq!(report.missing.len(), 4, "moved media includes every usage");
    assert!(report.missing.iter().all(|row| row.status == FileStatus::Missing && row.requires_selection == Some(true)));
    assert!(report.missing.iter().all(|row| row.candidates.as_ref().unwrap().iter().any(|candidate| candidate.verified && candidate.matched == MatchKind::Sha256)), "root mappings verify actual content");
    assert!(report.missing.iter().all(|row| row.resolution == Some(Resolution::Verified) && row.ambiguous == Some(false)));

    let applied = relink(&project, &verified_selections(&report)).unwrap();
    assert!(applied.changed == 4 && applied.remaining.is_empty(), "apply repairs every selected usage");
    assert_eq!(project, before, "apply returns a copy");
    let updated = &applied.project;
    let instrument = &updated.instrument_list()[0];
    let zones = instrument.zones.as_ref().unwrap();
    assert_eq!((instrument.cutoff(), zones[0].root_note(), updated.clip_list()[0].source_offset()), (1234.0, 42, 1.25), "relinking preserves sound and clip edits");
    assert_eq!(instrument.sample_path.as_deref(), Some(text(&relocated.join("Kick.wav")).as_str()));
    assert_eq!(zones[1].path.as_deref(), instrument.sample_path.as_deref());
    for (name, data) in [("Kick.wav", &kick_data), ("Zone.aif", &zone_data), ("Recording.caf", &clip_data)] {
        assert_eq!(&fs::read(relocated.join(name)).unwrap(), data, "relinking must not modify {name}");
    }
    assert_eq!(updated.to_value()["audioPlugins"], source.to_value()["audioPlugins"], "opaque plug-in paths are not rewritten");
    assert_eq!(updated.media_manifest.as_ref().unwrap().len(), 3, "the manifest follows the active paths without duplicates");
    let history = updated.media_relink_history.as_ref().unwrap();
    assert_eq!((history.len(), history[0].changes.as_ref().unwrap().len()), (1, 4));
    updated.validate().unwrap();
    // The report and the result serialize to the JSON the interface reads.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!((json["missingCount"].clone(), json["missing"][0]["candidates"][0]["match"].clone(), json["missing"][0]["resolvedPath"].clone()), (json!(4), json!("sha256"), json!(text(&kick))));
    assert!(json["missing"][0]["expectedSHA256"].is_string() && json["missing"][0]["kind"] == "sample" && json["scannedFiles"].is_number());
    assert_eq!(serde_json::to_value(&applied).unwrap()["relinked"][0]["differentContent"], false);
}

#[test]
fn ambiguity_and_different_content() {
    let root = TempDir::new("relink-ambiguous");
    let source = root.join("AmbiguousOld/Hit.wav");
    let data = vec![9u8; 64];
    put(&data, &source);
    let project = fingerprinted(&fixture(&text(&source), None, None));
    let (first, second, wrong) = (root.join("AmbiguousA/Hit.wav"), root.join("AmbiguousB/Hit.wav"), root.join("AmbiguousWrong/Hit.wav"));
    put(&data, &first);
    put(&data, &second);
    put(&[8u8; 64], &wrong);
    fs::remove_file(&source).unwrap();
    let roots: Vec<PathBuf> = [&first, &second, &wrong].iter().map(|path| path.parent().unwrap().to_path_buf()).collect();
    let report = search(&project, None, &roots, &[], &Limits::default());
    let reversed: Vec<PathBuf> = roots.iter().rev().cloned().collect();
    let reverse = search(&project, None, &reversed, &[], &Limits::default());
    let row = &report.missing[0];
    let options = row.candidates.as_ref().unwrap();
    assert!(row.ambiguous == Some(true) && row.resolution == Some(Resolution::Ambiguous), "duplicate content requires explicit disambiguation");
    assert_eq!(options.iter().filter(|candidate| candidate.verified).count(), 2, "both identical files stay visible");
    assert_eq!(options, reverse.missing[0].candidates.as_ref().unwrap(), "candidate order does not depend on the order of the chosen folders");
    assert!(options.iter().find(|candidate| candidate.path.ends_with("AmbiguousWrong/Hit.wav")).unwrap().content_mismatch, "same name, different content is flagged");

    let selection = Selection { id: row.id.clone(), from: row.path.clone(), to: text(&wrong), allow_different_content: false };
    let error = relink(&project, &[selection.clone()]).unwrap_err().to_string();
    assert!(error.contains("does not match its saved content fingerprint"), "{error}");
    let replaced = relink(&project, &[Selection { allow_different_content: true, ..selection.clone() }]).unwrap();
    assert_eq!(replaced.warnings, vec!["Different content explicitly selected for Drums.".to_string()]);
    assert_eq!(replaced.relinked[0].different_content, Some(true));
    let stale = relink(&project, &[Selection { from: "/old/stale.wav".into(), ..selection.clone() }]).unwrap_err().to_string();
    assert!(stale.starts_with("A relink selection is stale or invalid."), "{stale}");
    assert!(relink(&project, &[Selection { to: "relative.wav".into(), ..selection.clone() }]).is_err(), "a replacement must be an absolute path");
    assert!(relink(&project, &[Selection { to: text(&root.join("nowhere.wav")), ..selection.clone() }]).unwrap_err().to_string().starts_with("Replacement is not a readable, nonempty file"));
    // One bad selection rejects the batch; the caller's project is untouched either way.
    let valid = Selection { to: text(&first), ..selection.clone() };
    let invalid = Selection { id: "/audioPlugins/0/opaqueVendorState/path".into(), ..selection.clone() };
    let before = project.clone();
    assert!(relink(&project, &[valid.clone(), invalid]).is_err());
    assert!(relink(&project, &[valid.clone(), valid.clone()]).is_err(), "a reference cannot be selected twice");
    assert_eq!(project, before);
    assert_eq!(relink(&project, &[valid]).unwrap().changed, 1);
    assert_eq!(relink(&project, &[]).unwrap().remaining.len(), 1, "no selections: nothing changes, the missing list is returned");
}

#[test]
fn portable_prefix_and_relative_paths() {
    let root = TempDir::new("relink-portable");
    let data = vec![4u8; 87];
    let prefix = sha256_hex(&data)[..16].to_string();
    let project_file = root.join("Portable/Song.supertracker");
    let relative = format!("Song.media/{prefix}-Original.wav");
    let restored = root.join("Recovered/Original.wav");
    put(&data, &restored);
    let project = fixture(&relative, None, None);
    let inventory = inspect(&project, Some(&project_file), &Limits::default());
    assert_eq!(inventory.missing[0].expected_hash_prefix.as_deref(), Some(prefix.as_str()), "a collected name supplies its SHA-256 prefix");
    assert!(inventory.missing[0].resolved_path.as_ref().unwrap().ends_with(&relative), "relative paths resolve beside the project");
    let report = search(&project, Some(&project_file), &[restored.parent().unwrap().to_path_buf()], &[], &Limits::default());
    let best = &report.missing[0].candidates.as_ref().unwrap()[0];
    assert!(best.matched == MatchKind::HashPrefix && best.verified, "the original unprefixed file name is found and verified by content");
    let applied = apply(&project, &verified_selections(&report), Some(&project_file), DEFAULT_APPLY_HASH_BYTES, &Cancel::new()).unwrap();
    assert!(applied.remaining.is_empty());
    assert_eq!(applied.project.instrument_list()[0].sample_path.as_deref(), Some(text(&restored).as_str()));
    let unknown = inspect(&project, None, &Limits::default());
    assert_eq!(unknown.missing[0].status, FileStatus::UnresolvedRelativePath, "relative media without a project location never resolves against the working directory");
    // A hash-looking name outside a collection folder proves nothing.
    let plain = fixture(&text(&root.join(&format!("Library/{prefix}-Original.wav"))), None, None);
    assert_eq!(inspect(&plain, None, &Limits::default()).missing[0].expected_hash_prefix, None);
}

#[test]
fn renamed_by_content_limits_and_safety() {
    let root = TempDir::new("relink-renamed");
    let old = root.join("RenameOld/Original.wav");
    let data: Vec<u8> = (0..97).collect();
    put(&data, &old);
    let project = fingerprinted(&fixture(&text(&old), None, None));
    let folder = root.join("RenameNew");
    let renamed = folder.join("Changed name.wav");
    put(&data, &renamed);
    fs::remove_file(&old).unwrap();
    put(&[2u8; 97], &folder.join("Wrong renamed.wav"));
    put(&data, &folder.join("._Original.wav"));
    put(&data, &folder.join(".hidden/Original.wav"));
    #[cfg(unix)]
    std::os::unix::fs::symlink(&folder, folder.join("cycle")).unwrap();
    let report = search(&project, None, &[folder.clone()], &[], &Limits::default());
    let candidates = report.missing[0].candidates.as_ref().unwrap();
    assert!(candidates.iter().any(|candidate| candidate.path.ends_with("Changed name.wav") && candidate.verified), "renamed content of the saved size is found by its hash");
    assert!(!candidates.iter().any(|candidate| candidate.path.contains("._") || candidate.path.contains(".hidden") || candidate.path.contains("cycle")), "hidden files and links are not searched");
    assert_eq!(report.scanned_files, Some(2));

    let cancel = Cancel::new();
    cancel.cancel();
    assert_eq!(find_candidates(&project, None, &[root.path().to_path_buf()], &[], &Limits::default(), &cancel, &mut |_| {}).unwrap_err(), Error::Cancelled);
    assert_eq!(apply(&project, &verified_selections(&report), None, DEFAULT_APPLY_HASH_BYTES, &cancel).unwrap_err(), Error::Cancelled);
    assert_eq!(Error::Cancelled.to_string(), "The operation was cancelled.");

    let limited = search(&project, None, &[folder.clone()], &[], &Limits { max_hashes: 0, ..Limits::default() });
    assert!(limited.truncated == Some(true) && !limited.missing[0].candidates.as_ref().unwrap().iter().any(|candidate| candidate.verified), "an exhausted verification budget stays unverified and is disclosed");
    assert!(limited.warnings.iter().any(|warning| warning.starts_with("Search or verification reached its limit")));
    let bounded = search(&project, None, &[folder.clone()], &[], &Limits { max_files: 1, ..Limits::default() });
    assert_eq!((bounded.truncated, bounded.scanned_files), (Some(true), Some(2)), "the file bound stops the walk");
    let error = apply(&project, &verified_selections(&report), None, 1, &Cancel::new()).unwrap_err().to_string();
    assert!(error.starts_with("The replacement exceeds the verification budget."), "{error}");
    assert!(find_candidates(&project, None, &vec![folder.clone(); 65], &[], &Limits::default(), &Cancel::new(), &mut |_| {}).is_err(), "at most 64 folders");

    // A name alone never claims verification, but an explicit manual choice works without a fingerprint.
    let weak = fixture(&text(&root.join("Missing/Changed name.wav")), None, None);
    let weak_report = search(&weak, None, &[folder.clone()], &[], &Limits::default());
    let weak_row = &weak_report.missing[0];
    assert!(weak_row.candidates.as_ref().unwrap().iter().all(|candidate| !candidate.verified && candidate.matched == MatchKind::Name));
    assert_eq!(weak_row.resolution, Some(Resolution::Suggested));
    let manual = relink(&weak, &[Selection { id: weak_row.id.clone(), from: weak_row.path.clone(), to: text(&renamed), allow_different_content: false }]).unwrap();
    assert_eq!(manual.changed, 1);
    assert_eq!(fs::read(&renamed).unwrap(), data, "search and apply never touch the replacement");
    // A reconnected original is not replaced.
    put(&data, &old);
    let error = relink(&project, &verified_selections(&report)).unwrap_err().to_string();
    assert_eq!(error, "Drums is available again. Refresh the report before replacing a reconnected original.");
}

#[test]
fn progress_is_reported() {
    let root = TempDir::new("relink-progress");
    for index in 0..40 {
        put(b"x", &root.join(&format!("library/{index}/Other {index}.wav")));
    }
    put(b"found", &root.join("library/7/Hit.wav"));
    let project = fixture(&text(&root.join("gone/Hit.wav")), None, None);
    let mut seen: Vec<Progress> = Vec::new();
    let report = find_candidates(&project, None, &[root.join("library")], &[], &Limits::default(), &Cancel::new(), &mut |progress| seen.push(progress.clone())).unwrap();
    assert_eq!(report.scanned_files, Some(41));
    let last = seen.last().unwrap();
    assert_eq!((last.phase, last.scanned_files, last.candidates), ("complete", 41, 1));
    assert_eq!(seen[0].phase, "searching");
    assert_eq!(serde_json::to_value(last).unwrap()["scannedFiles"], 41);
}

#[test]
fn linked_aliases_move_together() {
    let root = TempDir::new("relink-alias");
    let missing = text(&root.join("missing.wav"));
    let (replacement, other) = (root.join("replacement.wav"), root.join("other.wav"));
    put(&[1, 2, 3], &replacement);
    put(&[3, 2, 1], &other);
    let project = Project::from_value(json!({
        "version": 1, "bpm": 120.0, "tracks": [{ "id": 0 }],
        "instruments": [{ "id": 0, "zones": [{ "path": missing, "samplePath": missing, "rootNote": 60 }] }],
        "patterns": [{ "id": 0, "length": 4, "steps": [[]] }], "sequence": [0],
        "audioClips": [{ "id": "clip", "track": 0, "startBeat": 0.0, "durationBeats": 4.0, "path": missing, "samplePath": missing }]
    }))
    .unwrap();
    let choose = |id: String, to: &Path| Selection { id, from: missing.clone(), to: text(to), allow_different_content: false };
    for (prefix, pointer) in [("/instruments/0/zones/0/", "/instruments/0/zones/0"), ("/audioClips/0/", "/audioClips/0")] {
        let before = project.clone();
        let result = relink(&project, &[choose(format!("{prefix}samplePath"), &replacement)]).unwrap();
        let item = result.project.to_value().pointer(pointer).unwrap().clone();
        assert!(item["path"] == text(&replacement).as_str() && item["samplePath"] == text(&replacement).as_str(), "relinking one alias repairs the path the host plays too");
        assert_eq!(project, before, "the input project (an Undo snapshot) is not changed");
        let error = relink(&project, &[choose(format!("{prefix}path"), &replacement), choose(format!("{prefix}samplePath"), &other)]).unwrap_err().to_string();
        assert_eq!(error, "Linked sample or audio clip references must use the same replacement file.");
        let mut divergent = project.to_value();
        *divergent.pointer_mut(&format!("{pointer}/samplePath")).unwrap() = json!(format!("{missing}-different"));
        let error = relink(&Project::from_value(divergent).unwrap(), &[choose(format!("{prefix}path"), &replacement)]).unwrap_err().to_string();
        assert_eq!(error, "The sample or audio clip aliases refer to different files. Repair the source selection before relinking.");
    }
}

#[test]
fn takes_follow_the_clip() {
    let root = TempDir::new("relink-takes");
    let (first, second, found) = (text(&root.join("gone/take1.wav")), text(&root.join("gone/take2.wav")), root.join("found.wav"));
    put(b"take", &found);
    let project = Project::from_value(json!({
        "version": 1, "bpm": 120, "tracks": [{}], "instruments": [], "patterns": [{ "length": 4, "steps": [[]] }],
        "audioClips": [{ "id": "c", "track": 0, "startBeat": 0, "durationBeats": 4, "path": second,
            "takeSet": { "version": 1, "activeID": "b", "takes": [{ "id": "a", "path": first, "sourceOffset": 0 }, { "id": "b", "path": second, "sourceOffset": 0 }] } }]
    }))
    .unwrap();
    let report = inspect(&project, None, &Limits::default());
    assert_eq!(report.missing.iter().map(|row| (row.kind, row.take)).collect::<Vec<_>>(), vec![(ReferenceKind::Clip, None), (ReferenceKind::Take, Some(0)), (ReferenceKind::Take, Some(1))]);
    let choose = |id: &str, from: &str| Selection { id: id.into(), from: from.into(), to: text(&found), allow_different_content: false };
    // The active take and the clip are one file.
    let active = relink(&project, &[choose("/audioClips/0/takeSet/takes/1/path", &second)]).unwrap().project.to_value();
    assert!(active["audioClips"][0]["path"] == text(&found).as_str() && active["audioClips"][0]["takeSet"]["takes"][1]["path"] == text(&found).as_str());
    let clip = relink(&project, &[choose("/audioClips/0/path", &second)]).unwrap().project.to_value();
    assert_eq!(clip["audioClips"][0]["takeSet"]["takes"][1]["path"], text(&found).as_str());
    // An inactive take changes alone.
    let inactive = relink(&project, &[choose("/audioClips/0/takeSet/takes/0/path", &first)]).unwrap().project.to_value();
    assert!(inactive["audioClips"][0]["path"] == second.as_str() && inactive["audioClips"][0]["takeSet"]["takes"][0]["path"] == text(&found).as_str());
}

/// Collected hashes, relative manifest paths, a moved project folder and recovery from the
/// original library by full hash (tests/MediaPortableTests.swift).
#[test]
fn portable_project_recovers_from_its_library() {
    let root = TempDir::new("relink-portable-store");
    let source = root.join("Library/Kick.wav");
    let original = vec![7u8; 123];
    put(&original, &source);
    let initial = common::minimal(Some(&source));
    let fingerprinted = fingerprinted(&initial);
    fs::create_dir_all(root.join("Before")).unwrap();
    let file = root.join("Before/Session.supertracker");
    let saved = write_project(&fingerprinted, &file, None, true).unwrap().project;
    let stored: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    let sample_path = stored["instruments"][0]["samplePath"].as_str().unwrap();
    assert_eq!(stored["mediaManifest"][0]["path"], sample_path, "manifest paths stay aligned with sample paths");
    assert!(sample_path.starts_with("Session.media/"));
    assert_eq!(inspect(&saved, None, &Limits::default()).missing_count, 0);
    fs::rename(root.join("Before"), root.join("After")).unwrap();
    let loaded = read_project(&root.join("After/Session.supertracker")).unwrap();
    assert_eq!(inspect(&loaded, None, &Limits::default()).missing_count, 0, "moving a portable project keeps its media available");
    let collected = loaded.instrument_list()[0].sample_path.clone().unwrap();
    fs::rename(&collected, root.join("Temporarily unavailable.wav")).unwrap();
    let report = search(&loaded, None, &[source.parent().unwrap().to_path_buf()], &[], &Limits::default());
    let reference = &report.missing[0];
    let candidate = reference.candidates.as_ref().unwrap().iter().find(|candidate| candidate.verified).unwrap();
    assert_eq!(candidate.matched, MatchKind::Sha256, "a collected project recovers its library file by full hash");
    let repaired = relink(&loaded, &[Selection { id: reference.id.clone(), from: reference.path.clone(), to: candidate.path.clone(), allow_different_content: false }]).unwrap();
    repaired.project.validate().unwrap();
    assert_eq!(fs::read(&source).unwrap(), original, "the library file is not modified");
}

/// A frozen print is identified by the hash recorded when it was printed: other content never
/// replaces it, on relink or on save.
#[test]
fn frozen_prints_require_their_exact_content() {
    let root = TempDir::new("relink-frozen");
    let (print, moved, changed) = (root.join("prints/song.wav"), root.join("moved/song.wav"), root.join("changed/song.wav"));
    put(b"printed audio", &print);
    put(b"printed audio", &moved);
    put(b"another render", &changed);
    let mut value = common::minimal(None).to_value();
    value["tracks"][0] = json!({ "id": 0, "name": "Pad", "frozen": { "version": 1, "id": "f1", "path": text(&print), "sha256": sha256_hex(b"printed audio"),
        "sampleRate": 48000, "frames": 100, "musicalFrames": 90, "tailFrames": 10, "musicalEndBeat": 16,
        "patternPrints": [{ "pattern": 0, "path": text(&print), "sha256": sha256_hex(b"printed audio"), "frames": 10, "passFrames": 8, "loop": { "path": text(&root.join("prints/loop.wav")), "sha256": "ab".repeat(32), "frames": 8 } }] } });
    let project = Project::from_value(value).unwrap();
    let report = inspect(&project, None, &Limits::default());
    assert_eq!(report.references.iter().map(|row| (row.id.as_str(), row.available, row.status)).collect::<Vec<_>>(), vec![
        ("/tracks/0/frozen/path", true, FileStatus::Available),
        ("/tracks/0/frozen/patternPrints/0/path", true, FileStatus::Available),
        ("/tracks/0/frozen/patternPrints/0/loop/path", false, FileStatus::Missing),
    ]);
    assert!(report.references[0].requires_exact_content && report.references[0].label == "Pad · Frozen song");
    // The print's bytes change: it is no longer available, saving refuses to certify it, fingerprinting too.
    fs::write(&print, b"tampered audio!").unwrap();
    let report = inspect(&project, None, &Limits::default());
    assert_eq!((report.references[0].available, report.references[0].status), (false, FileStatus::ContentMismatch));
    let error = write_project(&project, &root.join("Song.supertracker"), None, true).unwrap_err().to_string();
    assert!(error.starts_with("Frozen audio content changed: song.wav."), "{error}");
    assert!(!root.join("Song.supertracker").exists() && !root.join("Song.media").exists(), "nothing is written or collected");
    assert!(fingerprint(&project, None, &Limits::default(), &Cancel::new()).unwrap_err().to_string().starts_with("Frozen print Pad · Frozen song changed."));
    // Only the exact content relinks, with or without allowDifferentContent.
    let choose = |to: &Path, allow: bool| Selection { id: "/tracks/0/frozen/path".into(), from: text(&print), to: text(to), allow_different_content: allow };
    let error = relink(&project, &[choose(&changed, true)]).unwrap_err().to_string();
    assert!(error.starts_with("Frozen print Pad · Frozen song requires its exact original SHA-256."), "{error}");
    let applied = relink(&project, &[choose(&moved, false)]).unwrap();
    assert_eq!(applied.project.track_list()[0].frozen.as_ref().unwrap().path.as_deref(), Some(text(&moved).as_str()));
    assert_eq!(applied.project.track_list()[0].frozen.as_ref().unwrap().sha256.as_deref(), Some(sha256_hex(b"printed audio").as_str()), "the recorded hash is never rewritten");
    // With the intact print, a save collects it and the missing loop keeps its repairable path.
    fs::write(&print, b"printed audio").unwrap();
    let written = write_project(&project, &root.join("Song.supertracker"), None, true).unwrap();
    let frozen = written.project.track_list()[0].frozen.clone().unwrap();
    assert!(frozen.path.unwrap().starts_with(&text(&root.join("Song.media"))));
    assert_eq!(frozen.pattern_prints.unwrap()[0].loop_print.as_ref().unwrap().path.as_deref(), Some(text(&root.join("prints/loop.wav")).as_str()));
    // A record with a malformed hash is refused before anything is collected.
    let mut malformed = project.to_value();
    malformed["tracks"][0]["frozen"]["sha256"] = json!("not-a-hash");
    let malformed = Project::from_value(malformed).unwrap();
    assert!(write_project(&malformed, &root.join("Bad.supertracker"), None, true).unwrap_err().to_string().starts_with("Frozen audio has an invalid captured SHA-256."));
    assert_eq!(inspect(&malformed, None, &Limits::default()).references[0].status, FileStatus::InvalidFreeze);
}

#[test]
fn builtin_references_are_never_media() {
    let project = fixture("builtin:wavetables/Basic/Saw.wav", None, None);
    assert_eq!(inspect(&project, None, &Limits::default()).reference_count, 0);
    assert!(st_project::media::media_paths(&project).is_empty());
}
