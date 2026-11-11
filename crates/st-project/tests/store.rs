//! Project files and sessions. Ported from tests/ProjectStoreTests.swift and
//! tests/ProjectStoreBoundaryTests.swift, plus atomicity and collection idempotence.

mod common;

use common::{difference, minimal, TempDir};
use serde_json::{json, Value};
use st_project::files::{read_project, write_atomic, write_project};
use st_project::media::sha256_hex;
use st_project::session::{Session, StartupOrigin, RECOVERY_BACKUP_LIMIT};
use st_project::Project;
use std::fs;
use std::path::Path;

fn stored(file: &Path) -> Value {
    serde_json::from_slice(&fs::read(file).unwrap()).unwrap()
}

fn sample_path(project: &Project) -> String {
    project.instrument_list()[0].sample_path.clone().unwrap()
}

fn named(name: &str) -> Project {
    let mut project = minimal(None);
    project.name = Some(name.to_string());
    project
}

fn names_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder).map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    names.sort();
    names
}

#[test]
fn save_collects_media_once_and_stores_relative_paths() {
    let root = TempDir::new("store");
    let sample = root.join("original.wav");
    let contents = [0u8, 1, 2, 3, 4, 5];
    fs::write(&sample, contents).unwrap();
    let mut session = Session::new(root.join("support")).unwrap();
    assert!(session.update(minimal(Some(&sample)), true).unwrap());
    assert!(session.is_dirty());
    session.autosave().unwrap();
    let destination = root.join("Test.supertracker");
    let outcome = session.save_as(&destination).unwrap();
    assert!(!session.is_dirty());
    assert_eq!((outcome.collected, outcome.recovery_warning.clone(), session.path()), (1, None, Some(destination.as_path())));
    let path1 = sample_path(session.project().unwrap());
    assert_ne!(path1, sample.to_string_lossy());
    assert_eq!(path1, root.join(&format!("Test.media/{}-original.wav", &sha256_hex(&contents)[..16])).to_string_lossy());
    assert_eq!(fs::read(&sample).unwrap(), contents, "the source is never modified");
    assert_eq!(fs::read(&path1).unwrap(), contents);

    // Saving again must not copy the collected copy again or add a second hash prefix.
    let again = session.save().unwrap();
    assert_eq!((again.collected, sample_path(session.project().unwrap())), (0, path1.clone()));

    // A path that reaches the same media folder through a link keeps the existing file's identity.
    #[cfg(unix)]
    {
        let alias = root.join("media-alias");
        std::os::unix::fs::symlink(root.join("Test.media"), &alias).unwrap();
        let mut aliased = session.project().unwrap().clone();
        aliased.instruments.as_mut().unwrap()[0].sample_path = Some(alias.join(Path::new(&path1).file_name().unwrap()).to_string_lossy().into_owned());
        session.save_with(&destination, Some(aliased), true).unwrap();
        assert_eq!(sample_path(session.project().unwrap()), path1);
    }
    assert_eq!(names_in(&root.join("Test.media")).len(), 1, "repeated saves add no second prefix and no duplicate");
    assert!(names_in(root.path()).iter().all(|name| !name.contains(".saving")), "no temporary files stay behind");

    let on_disk = stored(&destination);
    let stored_path = on_disk["instruments"][0]["samplePath"].as_str().unwrap();
    assert!(stored_path.starts_with("Test.media/") && !stored_path.starts_with('/'), "stored paths are relative: {stored_path}");

    // Open in a fresh session: absolute again, clean, attached; and the recovery copy is there.
    let mut fresh = Session::new(root.join("support")).unwrap();
    let loaded = fresh.open(&destination).unwrap().clone();
    assert_eq!(sample_path(&loaded), path1);
    assert!(!fresh.is_dirty() && fresh.path() == Some(destination.as_path()));
    assert!(fresh.restore().unwrap().is_some());
    fresh.update(loaded.clone(), false).unwrap();
    assert!(!fresh.update(loaded.clone(), true).unwrap());
    assert!(!fresh.is_dirty(), "publishing unchanged state after a save must not make the document dirty");
    let mut edited = loaded;
    edited.bpm = Some(121.0);
    assert!(fresh.update(edited, true).unwrap() && fresh.is_dirty());
}

#[test]
fn moving_the_project_folder_keeps_its_media() {
    let root = TempDir::new("portable");
    let sample = root.join("library/kick.wav");
    fs::create_dir_all(sample.parent().unwrap()).unwrap();
    fs::write(&sample, b"kick").unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    write_project(&minimal(Some(&sample)), &root.join("a/Song.supertracker"), None, true).unwrap();
    fs::remove_file(&sample).unwrap();
    fs::rename(root.join("a"), root.join("b")).unwrap();
    let opened = read_project(&root.join("b/Song.supertracker")).unwrap();
    assert!(sample_path(&opened).starts_with(&*root.join("b/Song.media").to_string_lossy()));
    assert_eq!(fs::read(sample_path(&opened)).unwrap(), b"kick");
}

#[test]
fn save_as_elsewhere_reuses_the_hash_prefix() {
    let root = TempDir::new("saveas");
    let sample = root.join("snare.wav");
    fs::write(&sample, b"snare").unwrap();
    let mut session = Session::new(root.join("support")).unwrap();
    session.update(minimal(Some(&sample)), true).unwrap();
    session.save_as(&root.join("One.supertracker")).unwrap();
    session.save_as(&root.join("Two.supertracker")).unwrap();
    let name = format!("{}-snare.wav", &sha256_hex(b"snare")[..16]);
    assert_eq!(names_in(&root.join("Two.media")), vec![name.clone()], "a collected file keeps its one prefix in another project's folder");
    assert_eq!(sample_path(session.project().unwrap()), root.join("Two.media").join(&name).to_string_lossy());
    assert_eq!(names_in(&root.join("One.media")), vec![name], "the first project's media is left alone");
}

#[test]
fn zones_clips_takes_and_extensionless_files_are_collected() {
    let root = TempDir::new("complex");
    let (source, alternate) = (root.join("raw-recording"), root.join("raw-alternative"));
    fs::write(&source, [1u8, 2, 3, 4]).unwrap();
    fs::write(&alternate, [4u8, 3, 2, 1]).unwrap();
    let text = |path: &Path| path.to_string_lossy().into_owned();
    let project = Project::from_value(json!({
        "version": 1, "name": "Complex", "bpm": 120, "tracks": [{ "id": 0, "routingID": "t0" }], "routing": {},
        "instruments": [{ "id": 0, "samplePath": text(&source), "zones": [{ "samplePath": text(&source), "rootNote": 60 }] }],
        "patterns": [{ "id": 0, "length": 4, "steps": [[]] }], "sequence": [0],
        "audioClips": [{ "id": "phrase", "track": 0, "startBeat": 0.0, "durationBeats": 4.0, "path": text(&source),
            "takeSet": { "version": 1, "activeID": "a", "takes": [{ "id": "a", "path": text(&source), "sourceOffset": 0.0 }, { "id": "b", "path": text(&alternate), "sourceOffset": 0.0 }] } }],
        "mediaManifest": [{ "path": text(&source) }, { "path": text(&alternate) }]
    }))
    .unwrap();
    let destination = root.join("Song.supertracker");
    let written = write_project(&project, &destination, None, true).unwrap();
    assert_eq!(written.collected, 2, "one copy per distinct file");
    let raw = stored(&destination);
    let collected = |value: &Value| value.as_str().is_some_and(|path| path.starts_with("Song.media/"));
    assert!(collected(&raw["audioClips"][0]["path"]), "clip paths without an extension are collected");
    assert!(raw["audioClips"][0]["takeSet"]["takes"].as_array().unwrap().iter().all(|take| collected(&take["path"])), "every take is portable");
    assert!(collected(&raw["instruments"][0]["samplePath"]) && collected(&raw["instruments"][0]["zones"][0]["samplePath"]), "zones are collected");
    assert!(raw["mediaManifest"].as_array().unwrap().iter().all(|entry| collected(&entry["path"])), "manifest paths follow their files");
    fs::remove_file(&source).unwrap();
    fs::remove_file(&alternate).unwrap();
    let opened = read_project(&destination).unwrap();
    assert_eq!(fs::read(opened.clip_list()[0].path.as_ref().unwrap()).unwrap(), [1u8, 2, 3, 4], "collected media survives the loss of its source");
}

#[test]
fn opaque_state_and_unknown_fields_are_never_treated_as_media() {
    let root = TempDir::new("opaque");
    let (source, opaque) = (root.join("real.wav"), root.join("vendor-state.wav"));
    fs::write(&source, [1u8, 2, 3]).unwrap();
    fs::write(&opaque, [9u8, 8, 7]).unwrap();
    let state = json!({ "path": opaque.to_string_lossy(), "nested": { "samplePath": "opaque-relative.wav" }, "binary": "opaque untouched" });
    let mut value = minimal(Some(&source)).to_value();
    value["audioPlugins"] = json!([{ "owner": "master", "id": "vendor", "state": state }]);
    value["extensions"] = json!({ "future": { "path": opaque.to_string_lossy(), "samplePath": "unknown.wav" } });
    value["mediaRelinkHistory"] = json!([{ "path": opaque.to_string_lossy(), "samplePath": "historical.wav" }]);
    value["instruments"][0]["plugin"] = json!({ "state": state });
    let destination = root.join("Opaque.supertracker");
    let saved = write_project(&Project::from_value(value.clone()).unwrap(), &destination, None, true).unwrap().project.to_value();
    let opened = read_project(&destination).unwrap().to_value();
    for candidate in [&saved, &opened] {
        for key in ["audioPlugins", "extensions", "mediaRelinkHistory"] {
            assert_eq!(difference(&candidate[key], &value[key], key), None, "save and open must not reinterpret opaque or historical fields");
        }
        assert_eq!(difference(&candidate["instruments"][0]["plugin"], &value["instruments"][0]["plugin"], "plugin"), None);
    }
    assert_eq!(names_in(&root.join("Opaque.media")).len(), 1, "opaque state is not a collection candidate");
}

#[test]
fn a_failed_save_leaves_the_previous_file_and_the_session_untouched() {
    let root = TempDir::new("atomic");
    let mut session = Session::new(root.join("support")).unwrap();
    let destination = root.join("Good.supertracker");
    session.save_with(&destination, Some(named("Saved")), true).unwrap();
    session.update(named("Unsaved"), true).unwrap();
    let previous = fs::read(&destination).unwrap();
    // The final rename cannot replace a folder: the write fails at its last step.
    let bad = root.join("Destination.supertracker");
    fs::create_dir_all(&bad).unwrap();
    let error = session.save_as(&bad).unwrap_err();
    assert!(error.to_string().starts_with("Could not save Destination.supertracker"), "{error}");
    assert_eq!((session.path(), session.is_dirty(), session.project().unwrap().display_name()), (Some(destination.as_path()), true, "Unsaved"));
    assert_eq!(fs::read(&destination).unwrap(), previous, "a failed Save As must not touch the prior document");
    assert!(names_in(root.path()).iter().all(|name| !name.contains(".saving")), "the temporary file is removed: {:?}", names_in(root.path()));

    // A project the host would reject never reaches the disk either.
    let mut invalid = named("Invalid");
    invalid.bpm = Some(-10.0);
    assert!(session.save_with(&destination, Some(invalid), true).is_err());
    assert_eq!(fs::read(&destination).unwrap(), previous);

    // write_atomic itself: a failure leaves the old bytes, a success replaces them whole.
    let file = root.join("plain.txt");
    write_atomic(&file, b"old").unwrap();
    assert!(write_atomic(&root.join("missing-folder/plain.txt"), b"new").is_err());
    assert_eq!(fs::read(&file).unwrap(), b"old");
    write_atomic(&file, b"new contents").unwrap();
    assert_eq!(fs::read(&file).unwrap(), b"new contents");
}

#[test]
fn a_media_collision_does_not_commit_and_missing_media_keeps_its_reference() {
    let root = TempDir::new("collision");
    let source = root.join("source.wav");
    fs::write(&source, [1u8, 2, 3]).unwrap();
    let target = root.join(&format!("Collision.media/{}-source.wav", &sha256_hex(&[1, 2, 3])[..16]));
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, [9u8, 8, 7]).unwrap();
    let mut session = Session::new(root.join("support")).unwrap();
    session.update(minimal(Some(&source)), true).unwrap();
    let destination = root.join("Collision.supertracker");
    let error = session.save_as(&destination).unwrap_err();
    assert!(error.to_string().contains("different content"), "{error}");
    assert!(session.path().is_none() && session.is_dirty() && !destination.exists(), "a collected-media collision must not commit");
    assert_eq!((fs::read(&target).unwrap(), fs::read(&source).unwrap()), (vec![9u8, 8, 7], vec![1u8, 2, 3]), "both files are preserved");

    let missing = root.join("missing.wav");
    session.save_with(&root.join("Missing.supertracker"), Some(minimal(Some(&missing))), true).unwrap();
    assert_eq!(sample_path(session.project().unwrap()), missing.to_string_lossy(), "a missing source keeps its exact, repairable reference");
    assert_eq!(stored(&root.join("Missing.supertracker"))["instruments"][0]["samplePath"], missing.to_string_lossy().as_ref());
}

#[test]
fn an_interrupted_collection_leaves_nothing_under_the_final_name() {
    let root = TempDir::new("partial");
    let source = root.join("pad.wav");
    fs::write(&source, b"pad audio").unwrap();
    // What a crash in the middle of a copy leaves: a partial file under the private name only.
    fs::create_dir_all(root.join("Song.media")).unwrap();
    fs::write(root.join("Song.media/.collecting-stale"), b"pad").unwrap();
    let destination = root.join("Song.supertracker");
    let written = write_project(&minimal(Some(&source)), &destination, None, true).unwrap();
    assert_eq!(fs::read(sample_path(&written.project)).unwrap(), b"pad audio", "the next save collects the whole file");
    let finals: Vec<String> = names_in(&root.join("Song.media")).into_iter().filter(|name| !name.starts_with(".collecting-")).collect();
    assert_eq!(finals.len(), 1);
    assert_eq!(written.file_sha256, sha256_hex(&fs::read(&destination).unwrap()));
}

#[test]
fn a_recovery_copy_failure_does_not_fail_the_save() {
    let root = TempDir::new("recovery-warning");
    let mut session = Session::new(root.join("support")).unwrap();
    session.update(named("First"), true).unwrap();
    fs::create_dir_all(session.recovery_file()).unwrap();
    let destination = root.join("Saved.supertracker");
    let outcome = session.save_as(&destination).unwrap();
    assert!(session.path() == Some(destination.as_path()) && !session.is_dirty(), "a committed project is acknowledged despite the recovery failure");
    assert_eq!(&read_project(&destination).unwrap(), session.project().unwrap());
    assert!(outcome.recovery_warning.is_some_and(|warning| !warning.is_empty()), "the failure is reported with the save");
    fs::remove_dir_all(session.recovery_file()).unwrap();
    session.update(named("Retry"), true).unwrap();
    assert_eq!(session.save().unwrap().recovery_warning, None, "a successful retry reports no stale warning");
    assert_eq!(read_project(&session.recovery_file()).unwrap().display_name(), "Retry");
}

#[test]
fn an_unreadable_recovery_file_is_kept_aside() {
    let root = TempDir::new("unreadable");
    let mut session = Session::new(root.join("support")).unwrap();
    let bytes = br#"{"version":1,"name":"Newer build","tracks":[]}"#;
    fs::write(session.recovery_file(), bytes).unwrap();
    let startup = session.startup().unwrap();
    assert!(startup.project.is_none() && startup.origin == StartupOrigin::Blank);
    let warning = startup.warning.expect("an unreadable recovery file must be reported");
    assert!(warning.contains("could not be recovered") && warning.contains("was kept as"), "{warning}");
    session.update(minimal(None), true).unwrap();
    session.autosave().unwrap();
    let kept: Vec<String> = names_in(session.support_dir()).into_iter().filter(|name| name.starts_with("Recovery (unreadable")).collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(fs::read(session.support_dir().join(&kept[0])).unwrap(), bytes, "the new session's autosave must not destroy the unreadable one");
}

/// After a relaunch a session saved to a file reopens as that file: clean when nothing changed
/// since the save, dirty when it had unsaved edits. If the file changed on disk, or the record is
/// missing or stale, it reopens untitled and dirty; only an explicit save writes the user's file.
#[test]
fn recovery_reattaches_the_file_only_while_it_is_unchanged() {
    let root = TempDir::new("recovery-file");
    let (support, file) = (root.join("support"), root.join("Song.supertracker"));
    let mut session = Session::new(&support).unwrap();
    session.update(minimal(None), true).unwrap();
    session.save_with(&file, None, false).unwrap();
    let reopen = || {
        let mut reopened = Session::new(&support).unwrap();
        let name = reopened.restore().unwrap().map(|project| project.display_name().to_string());
        (reopened, name)
    };
    let (mut reopened, _) = reopen();
    assert!(reopened.path() == Some(file.as_path()) && !reopened.is_dirty(), "a saved, unchanged session reopens as its file, clean");
    reopened.update(named("Edited"), true).unwrap();
    reopened.autosave().unwrap();
    let saved_bytes = fs::read(&file).unwrap();
    let (reopened, name) = reopen();
    assert!(name.as_deref() == Some("Edited") && reopened.path() == Some(file.as_path()) && reopened.is_dirty(), "unsaved edits reopen dirty on their file");
    reopened.autosave().unwrap();
    assert_eq!(fs::read(&file).unwrap(), saved_bytes, "autosave never writes the user's file");

    // The file changed on disk since the session last saved it.
    fs::write(&file, named("Saved elsewhere").to_json()).unwrap();
    let (reopened, _) = reopen();
    assert!(reopened.path().is_none() && reopened.is_dirty(), "a changed file is not reattached");

    // A record that does not describe this recovery file is ignored.
    session.save_with(&file, None, false).unwrap();
    fs::write(session.recovery_file(), named("Other session").to_json()).unwrap();
    let (reopened, _) = reopen();
    assert!(reopened.path().is_none() && reopened.is_dirty(), "a stale session record does not reattach a file");

    // No record at all.
    session.save_with(&file, None, false).unwrap();
    fs::remove_file(session.recovery_record_file()).unwrap();
    let (reopened, _) = reopen();
    assert!(reopened.path().is_none() && reopened.is_dirty(), "recovery without a record is untitled and dirty");

    // Untitled sessions stay untitled and dirty.
    session.accept_new_project(Some(minimal(None))).unwrap();
    session.autosave().unwrap();
    let (reopened, _) = reopen();
    assert!(reopened.path().is_none() && reopened.is_dirty());
}

/// Each launch keeps the recovery file as it found it, before autosave or a New from a template
/// can replace it.
#[test]
fn each_launch_keeps_the_session_it_found() {
    let root = TempDir::new("launch-copies");
    let support = root.join("support");
    let launch = || {
        let mut session = Session::new(&support).unwrap();
        let startup = session.startup().unwrap();
        (session, startup.project.map(|project| project.display_name().to_string()))
    };
    let (mut first, _) = launch();
    first.update(named("Unsaved break"), true).unwrap();
    first.autosave().unwrap();
    // Relaunch: the session comes back, then is replaced and autosaved.
    let (mut second, restored) = launch();
    assert_eq!(restored.as_deref(), Some("Unsaved break"));
    second.accept_new_project(Some(named("Template"))).unwrap();
    second.autosave().unwrap();
    let (third, replaced) = launch();
    assert_eq!(replaced.as_deref(), Some("Template"));
    let copies = third.recovery_backups();
    assert_eq!(copies.len(), 2, "each distinct session found at launch is kept");
    assert_eq!(read_project(&copies[1]).unwrap().display_name(), "Unsaved break", "the replaced unsaved session can still be opened");
    assert_eq!(read_project(&copies[0]).unwrap().display_name(), "Template");
    // Relaunching the same session adds no copy; old copies are pruned to the limit, newest first.
    launch();
    assert_eq!(third.recovery_backups().len(), 2, "an unchanged session is not copied again");
    for index in 0..RECOVERY_BACKUP_LIMIT + 2 {
        let mut session = Session::new(&support).unwrap();
        session.update(named(&format!("Session {index}")), true).unwrap();
        session.autosave().unwrap();
        launch();
    }
    let copies = third.recovery_backups();
    assert_eq!(copies.len(), RECOVERY_BACKUP_LIMIT, "launch copies are pruned to the limit");
    assert_eq!(read_project(&copies[0]).unwrap().display_name(), format!("Session {}", RECOVERY_BACKUP_LIMIT + 1), "the newest copy is kept");
}

#[test]
fn templates_and_the_startup_session() {
    let root = TempDir::new("templates");
    let support = root.join("template-support");
    let mut store = Session::new(&support).unwrap();
    store.update(minimal(None), true).unwrap();
    store.autosave().unwrap();
    let recovery_before = fs::read(store.recovery_file()).unwrap();
    let current_before = store.project().unwrap().clone();
    let sample = root.join("template.wav");
    fs::write(&sample, b"template audio").unwrap();
    let hardware = Project::from_value(json!({
        "version": 1, "name": "Eight tracks", "bpm": 120,
        "tracks": (0..8).map(|index| json!({ "id": index, "routingID": format!("t{index}"), "name": format!("Track {}", index + 1), "instrument": if index < 3 { index } else { -1 } })).collect::<Vec<_>>(),
        "instruments": (0..3).map(|index| json!({ "id": index, "name": format!("Synth {index} Editor"), "type": "midi", "samplePath": if index == 0 { sample.to_string_lossy().into_owned() } else { String::new() } })).collect::<Vec<_>>(),
        "patterns": [{ "id": 0, "length": 64, "steps": (0..8).map(|_| vec![Value::Null; 64]).collect::<Vec<_>>(), "automation": [] }],
        "sequence": [0], "routing": {}
    }))
    .unwrap();
    let template = store.save_template(&hardware, "  Hardware editors ", Some("hardware-editors")).unwrap();
    assert_eq!((template.id.as_str(), template.name.as_str()), ("hardware-editors", "Hardware editors"));
    store.set_default_template(Some("hardware-editors")).unwrap();
    assert!(store.project() == Some(&current_before) && store.is_dirty(), "saving a template does not touch the open session");
    assert_eq!(fs::read(store.recovery_file()).unwrap(), recovery_before, "template saves must not change the recovery file");
    assert_eq!(store.templates(), vec![template.clone()]);
    assert!(sample_path(&store.template_project("hardware-editors").unwrap()).starts_with(&*support.join("Templates/hardware-editors/Template.media").to_string_lossy()), "a template owns a copy of its media");

    let mut reopened = Session::new(&support).unwrap();
    let recovered = reopened.startup().unwrap();
    assert_eq!((recovered.origin, recovered.project.unwrap().display_name().to_string()), (StartupOrigin::Recovered, "Test".to_string()), "recovery takes priority over the default template");
    fs::remove_file(reopened.recovery_file()).unwrap();
    let mut startup_store = Session::new(&support).unwrap();
    let startup = startup_store.startup().unwrap();
    let fresh = startup.project.unwrap();
    assert_eq!(startup.origin, StartupOrigin::Template("hardware-editors".into()));
    assert!(fresh.track_list().len() == 8 && startup_store.path().is_none() && !startup_store.is_dirty());
    assert!(fresh.pattern_list()[0].automation.is_some());
    assert!(startup_store.proposed_new_project(None, true).unwrap().is_none(), "an explicit blank bypasses the default");
    assert!(startup_store.set_default_template(Some("missing")).is_err(), "a missing template cannot become the default");
    assert_eq!(startup_store.default_template_id().unwrap().as_deref(), Some("hardware-editors"));
    assert_eq!(startup_store.save_template(&hardware, "Escape", Some("../escape")).unwrap_err().to_string(), "Invalid project template identifier.");
    assert!(startup_store.save_template(&hardware, "   ", None).is_err());
    let generated = startup_store.save_template(&hardware, "Another", None).unwrap();
    assert!(generated.id.len() == 32 && startup_store.templates().len() == 2);

    fs::write(&template.path, b"broken").unwrap();
    let error = startup_store.proposed_new_project(None, false).unwrap_err().to_string();
    assert!(error.starts_with("Project template hardware-editors could not be opened"), "{error}");
    assert_eq!(startup_store.project(), Some(&fresh), "a corrupt default preserves the open session");
    // At launch a corrupt default is reported and the interface's blank project is used.
    let broken = Session::new(&support).unwrap().startup().unwrap();
    assert!(broken.project.is_none() && broken.origin == StartupOrigin::Blank && broken.warning.is_some());
    startup_store.set_default_template(None).unwrap();
    assert!(startup_store.proposed_new_project(None, false).unwrap().is_none());
}

#[test]
fn oversized_and_foreign_files_are_refused() {
    let root = TempDir::new("foreign");
    fs::write(root.join("notes.supertracker"), b"just text").unwrap();
    assert_eq!(read_project(&root.join("notes.supertracker")).unwrap_err().to_string(), "The file is not a SuperTracker project.");
    let error = read_project(&root.join("absent.supertracker")).unwrap_err().to_string();
    assert!(error.starts_with("Could not open absent.supertracker: "), "{error}");
    let mut session = Session::new(root.join("support")).unwrap();
    assert_eq!(session.save().unwrap_err().to_string(), "This project has not been saved yet. Choose where to save it.");
    assert_eq!(session.save_as(&root.join("x.supertracker")).unwrap_err().to_string(), "There is no project to save.");
    assert!(session.open(&root.join("notes.supertracker")).is_err() && session.project().is_none());
}
