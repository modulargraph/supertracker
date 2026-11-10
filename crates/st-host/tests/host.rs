//! The commands `st-host` implements itself, called as the interface calls them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use st_host::testing::{TempDir, TestHost};
use st_host::{install, Host, HostOptions, NoPayload, Opener, ProjectAudio, PublishOptions, QuitProgress, QuitRequest, Result, ScriptedDialogs};

#[derive(Default)]
struct RecordingOpener(Mutex<Vec<String>>);

impl Opener for RecordingOpener {
    fn open_url(&self, url: &str) -> Result<()> {
        self.0.lock().unwrap().push(format!("url {url}"));
        Ok(())
    }
    fn reveal(&self, path: &Path) -> Result<()> {
        self.0.lock().unwrap().push(format!("reveal {}", path.display()));
        Ok(())
    }
}

struct Fixture {
    folder: TempDir,
    host: TestHost,
    services: Host,
    dialogs: Arc<ScriptedDialogs>,
    opened: Arc<RecordingOpener>,
}

impl Fixture {
    fn new() -> Self {
        Self::in_folder(TempDir::new("host"), |_| {})
    }

    /// A host whose support folder is `folder/support`; `extra` registers other areas' commands.
    fn in_folder(folder: TempDir, extra: impl FnOnce(&mut st_host::BridgeBuilder)) -> Self {
        let dialogs = Arc::new(ScriptedDialogs::from_answers(Vec::new()));
        let opened = Arc::new(RecordingOpener::default());
        let mut services = None;
        let host = TestHost::start(|bridge| {
            let mut options = HostOptions::new(folder.path().join("support"), dialogs.clone());
            options.opener = opened.clone();
            options.autosave_every = None;
            // These tests are about the host's own commands; the audio area has its own (tests/audio.rs),
            // and so have the library, MIDI, the Maschine MK3 and AI control (tests/library.rs, midi.rs,
            // mk3.rs, agent.rs). Some tests below register stand-ins for those areas' commands.
            options.audio = None;
            options.services = st_host::areas::services::ServiceOptions::none();
            services = Some(install(bridge, options).expect("install the host"));
            extra(bridge);
        });
        Fixture { folder, host, services: services.unwrap(), dialogs, opened }
    }

    fn relaunch(self, extra: impl FnOnce(&mut st_host::BridgeBuilder)) -> Self {
        let Fixture { folder, host, services, .. } = self;
        drop(host);
        drop(services);
        Self::in_folder(folder, extra)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.folder.path().join(name)
    }
}

fn project(name: &str) -> Value {
    json!({
        "version": 1, "name": name, "bpm": 120,
        "tracks": [{"name": "Track 01", "instrument": 0}],
        "instruments": [{"name": "Instrument 01", "type": "sample", "samplePath": ""}],
        "patterns": [{"id": 0, "length": 64, "steps": [[{"note": 60, "instrument": 0}]]}],
        "sequence": [0], "audioPlugins": []
    })
}

#[test]
fn ready_describes_an_empty_first_launch_honestly() {
    let fixture = Fixture::new();
    let ready = fixture.host.request("ready", json!({})).unwrap();
    assert_eq!(ready["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(ready["project"], Value::Null);
    assert_eq!(ready["dirty"], false);
    assert_eq!(ready["path"], Value::Null);
    assert_eq!(ready["roots"], json!([]));
    assert_eq!(ready["plugins"], json!([]));
    assert_eq!(ready["audio"], json!({"inputs": [], "outputs": []}));
    assert_eq!(ready["midi"], json!({"inputs": [], "outputs": []}));
    for absent in ["warning", "audioError", "openPath", "pluginRecovery"] {
        assert!(ready.get(absent).is_none(), "{absent} should be absent");
    }
    let commands: Vec<&str> = ready["commands"].as_array().unwrap().iter().map(|name| name.as_str().unwrap()).collect();
    assert!(commands.contains(&"project.save") && commands.contains(&"ready") && !commands.contains(&"plugin.load"));
}

#[test]
fn ready_collects_what_other_areas_have_registered() {
    let fixture = Fixture::in_folder(TempDir::new("host-areas"), |bridge| {
        let commands = bridge.commands();
        commands.register("library.roots", |_: NoPayload| -> Result<Value> { Ok(json!([{"path": "/samples"}])) });
        commands.register("plugins.list", |_: NoPayload| -> Result<Value> { Ok(json!([{"id": "clap:synth"}])) });
        commands.register("audio.devices", |_: NoPayload| -> Result<Value> { Ok(json!({"inputs": [], "outputs": [{"name": "Null"}]})) });
        commands.register("midi.devices", |_: NoPayload| -> Result<Value> { st_host::fail!("The MIDI system is not running.") });
        commands.register("plugins.recovery", |_: NoPayload| -> Result<Value> { Ok(json!({"suspects": []})) });
    });
    let ready = fixture.host.request("ready", json!({})).unwrap();
    assert_eq!(ready["roots"], json!([{"path": "/samples"}]));
    assert_eq!(ready["plugins"], json!([{"id": "clap:synth"}]));
    assert_eq!(ready["audio"]["outputs"][0]["name"], "Null");
    assert_eq!(ready["pluginRecovery"], json!({"suspects": []}));
    // An area that fails does not take the handshake down: its part is empty and the reason is shown.
    assert_eq!(ready["midi"], json!({"inputs": [], "outputs": []}));
    assert_eq!(ready["warning"], "The MIDI system is not running.");
}

#[test]
fn publish_save_new_open_round_trip() {
    let fixture = Fixture::new();
    let file = fixture.file("Song.supertracker");
    assert_eq!(fixture.host.request("engine.project", json!({"project": project("Song"), "awaitPlugins": true})), Ok(json!({"audio": false})));

    // Save without a known file asks where, with the project's name proposed.
    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("project.save", json!({"project": project("Song")})), Ok(json!({"cancelled": true})));
    assert!(!file.exists());
    fixture.dialogs.push(Some(fixture.file("Song")));
    let saved = fixture.host.request("project.save", json!({"project": project("Song")})).unwrap();
    assert_eq!(saved, json!({"project": project("Song"), "path": file, "saved": true}));
    let asked = fixture.dialogs.asked();
    assert_eq!(asked[1].suggested_name.as_deref(), Some("Song.supertracker"));
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&file).unwrap()).unwrap(), project("Song"));

    // An edit, then Save: no dialog, same file.
    let edited = project("Song (edited)");
    fixture.host.request("engine.project", json!({"project": edited})).unwrap();
    assert!(fixture.services.session.dirty());
    let again = fixture.host.request("project.save", json!({"project": edited})).unwrap();
    assert_eq!(again["path"], json!(file));
    assert_eq!(fixture.dialogs.asked().len(), 2);
    assert!(!fixture.services.session.dirty());

    // Save As asks again even though the file is known; an explicit path asks nobody.
    let copy = fixture.file("Copy.supertracker");
    fixture.dialogs.push(Some(copy.clone()));
    assert_eq!(fixture.host.request("project.saveAs", json!({})).unwrap()["path"], json!(copy));
    let third = fixture.file("Third.supertracker");
    assert_eq!(fixture.host.request("project.saveAs", json!({"path": third})).unwrap()["path"], json!(third));
    assert_eq!(fixture.dialogs.asked().len(), 3);

    // New: the interface gets null (it supplies the blank project) and the session is untitled.
    assert_eq!(fixture.host.request("project.new", json!({})), Ok(Value::Null));
    assert_eq!(fixture.services.session.path(), None);
    assert!(fixture.services.session.project().is_none());

    // Open by path, then through the Open window.
    let opened = fixture.host.request("project.open", json!({"path": file})).unwrap();
    assert_eq!(opened, json!({"project": edited, "path": file, "dirty": false}));
    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("project.open", json!({})), Ok(json!({"cancelled": true})));
    assert_eq!(fixture.services.session.path(), Some(file.clone()), "a cancelled Open leaves the session alone");
    fixture.dialogs.push(Some(copy.clone()));
    assert_eq!(fixture.host.request("project.open", json!({})).unwrap()["path"], json!(copy));
    assert_eq!(fixture.host.bridge.pending(), 0);
}

#[test]
fn project_commands_fail_with_messages_for_the_user() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.request("project.save", json!({"path": fixture.file("x.supertracker")})), Err("There is no project to save.".to_string()));
    assert_eq!(fixture.host.request("engine.project", json!({})), Err("`engine.project` needs the project to publish.".to_string()));
    let mut slow = project("Slow");
    slow["bpm"] = json!(3);
    assert_eq!(fixture.host.request("engine.project", json!({"project": slow})), Err("Project tempo must be between 20 and 400 BPM.".to_string()));
    assert_eq!(fixture.host.request("project.open", json!({"path": "relative.supertracker"})), Err("File paths must be absolute (got “relative.supertracker”).".to_string()));
    let missing = fixture.host.request("project.open", json!({"path": fixture.file("Missing.supertracker")})).unwrap_err();
    assert!(missing.starts_with("Could not open Missing.supertracker:"), "{missing}");
    std::fs::write(fixture.file("notes.txt"), "hello").unwrap();
    let not_a_project = fixture.host.request("project.open", json!({"path": fixture.file("notes.txt")})).unwrap_err();
    assert!(not_a_project.contains("not"), "{not_a_project}");
    // A dialog with no scripted answer is an error, not a hang and not a silent cancel.
    fixture.host.request("engine.project", json!({"project": project("Song")})).unwrap();
    let unanswered = fixture.host.request("project.saveAs", json!({})).unwrap_err();
    assert!(unanswered.starts_with("No scripted answer is left for the Save window"), "{unanswered}");
    let template = fixture.host.request("project.new", json!({"templateID": "abc"})).unwrap_err();
    assert!(template.starts_with("Project template abc could not be opened:"), "{template}");
    assert_eq!(fixture.services.session.project().unwrap().display_name(), "Song", "a template that cannot be opened does not cost the session");
    assert_eq!(fixture.host.request("project.new", json!({"templateID": "abc", "blank": true})), Ok(Value::Null));
}

#[test]
fn autosave_writes_a_recovery_copy_that_the_next_launch_restores() {
    let fixture = Fixture::new();
    let file = fixture.file("Song.supertracker");
    fixture.host.request("engine.project", json!({"project": project("Song")})).unwrap();
    fixture.host.request("project.save", json!({"path": file})).unwrap();
    assert_eq!(fixture.host.request("project.autosave", json!({"project": project("Song, unsaved edit")})), Ok(json!({"saved": true})));

    let relaunched = fixture.relaunch(|_| {});
    let ready = relaunched.host.request("ready", json!({})).unwrap();
    assert_eq!(ready["project"], project("Song, unsaved edit"));
    assert_eq!(ready["dirty"], true);
    assert_eq!(ready["path"], json!(file));

    // A recovery copy that cannot be read: blank session, and the handshake says why.
    std::fs::write(relaunched.folder.path().join("support/Recovery.supertracker"), "{").unwrap();
    let broken = relaunched.relaunch(|_| {});
    let ready = broken.host.request("ready", json!({})).unwrap();
    assert_eq!(ready["project"], Value::Null);
    let warning = ready["warning"].as_str().unwrap();
    assert!(warning.starts_with("The previous session could not be recovered:") && warning.contains("was kept as"), "{warning}");
}

#[test]
fn a_reloading_interface_gets_the_session_back() {
    let fixture = Fixture::new();
    fixture.host.request("engine.project", json!({"project": project("In progress")})).unwrap();
    fixture.host.request("engine.project", json!({"project": project("In progress, edited")})).unwrap();
    let ready = fixture.host.request("ready", json!({})).unwrap();
    assert_eq!(ready["project"], project("In progress, edited"));
    assert_eq!(ready["dirty"], true);
}

struct Graph {
    prepared: Mutex<Vec<(String, PublishOptions)>>,
}

impl ProjectAudio for Graph {
    fn prepare(&self, project: &st_project::Project, options: &PublishOptions) -> Result<Value> {
        let name = project.display_name().to_string();
        if name == "Broken graph" {
            st_host::fail!("The audio device is gone.");
        }
        self.prepared.lock().unwrap().push((name, *options));
        Ok(json!({"latency": 128}))
    }
}

#[test]
fn an_attached_audio_engine_prepares_publications_and_can_refuse_them() {
    let snapshots = Arc::new(Mutex::new(json!({"audioPlugins": [{"id": "p1", "state": "AAAA"}]})));
    let served = snapshots.clone();
    let quieted: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let (panic_log, arm_log) = (quieted.clone(), quieted.clone());
    let fixture = Fixture::in_folder(TempDir::new("host-audio"), move |bridge| {
        let commands = bridge.commands();
        commands.register("engine.snapshot", move |_: NoPayload| -> Result<Value> { Ok(served.lock().unwrap().clone()) });
        commands.register("engine.panic", move |_: NoPayload| -> Result<()> {
            panic_log.lock().unwrap().push("panic");
            Ok(())
        });
        commands.register("transport.recordArm", move |_: Value| -> Result<()> {
            arm_log.lock().unwrap().push("disarm");
            Ok(())
        });
    });
    let graph = Arc::new(Graph { prepared: Mutex::default() });
    fixture.services.session.attach_audio(graph.clone());

    assert_eq!(fixture.host.request("engine.project", json!({"project": project("Song"), "replace": true})), Ok(json!({"latency": 128})));
    assert_eq!(graph.prepared.lock().unwrap()[0], ("Song".to_string(), PublishOptions { replace: true, force_plugin_state: true, await_plugins: false }));
    // A refused publication leaves the session as it was.
    assert_eq!(fixture.host.request("engine.project", json!({"project": project("Broken graph")})), Err("The audio device is gone.".to_string()));
    assert_eq!(fixture.services.session.project().unwrap().display_name(), "Song");

    // Saving stores the plug-in states the engine reports.
    let file = fixture.file("Song.supertracker");
    let saved = fixture.host.request("project.save", json!({"path": file})).unwrap();
    assert_eq!(saved["project"]["audioPlugins"], json!([{"id": "p1", "state": "AAAA"}]));
    // A plug-in whose state cannot be captured stops the save, and the file keeps its content.
    *snapshots.lock().unwrap() = json!({"audioPlugins": [{"name": "Reverb", "stateError": "it does not answer"}]});
    assert_eq!(fixture.host.request("project.save", json!({})), Err("Reverb: it does not answer".to_string()));
    *snapshots.lock().unwrap() = json!({"audioPlugins": []});

    // Replacing the session silences the engine first.
    fixture.host.request("project.new", json!({})).unwrap();
    fixture.host.request("project.open", json!({"path": file})).unwrap();
    assert_eq!(*quieted.lock().unwrap(), ["panic", "disarm", "disarm"]);
    assert_eq!(graph.prepared.lock().unwrap().last().unwrap().1, PublishOptions { replace: true, force_plugin_state: true, await_plugins: false });

    // The handshake prepares the session's audio and reports a failure without withholding the session.
    let ready = fixture.host.request("ready", json!({})).unwrap();
    assert!(ready.get("audioError").is_none());
}

#[test]
fn file_choosers_answer_with_a_path_or_cancelled() {
    let fixture = Fixture::in_folder(TempDir::new("host-dialogs"), |bridge| {
        bridge.commands().register("library.scan", |payload: Value| -> Result<Value> { Ok(json!({"scanning": true, "roots": payload["roots"]})) });
    });
    fixture.dialogs.push(Some(PathBuf::from("/samples/kick.wav")));
    assert_eq!(fixture.host.request("dialog.openSample", json!({})), Ok(json!({"path": "/samples/kick.wav"})));
    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("dialog.openSample", json!({})), Ok(json!({"cancelled": true})));
    assert!(fixture.dialogs.asked()[0].filters[0].extensions.contains(&"flac".to_string()));

    fixture.dialogs.push(Some(fixture.file("Mix.wav")));
    assert_eq!(fixture.host.request("dialog.export", json!({"name": "Mix"})), Ok(json!({"path": fixture.file("Mix.wav")})));
    assert_eq!(fixture.dialogs.asked()[2].suggested_name.as_deref(), Some("Mix.wav"));

    // Stems: a new folder each time, never an existing one.
    for expected in ["My- Song stems", "My- Song stems 2"] {
        fixture.dialogs.push(Some(fixture.folder.path().to_path_buf()));
        let reserved = fixture.host.request("dialog.export", json!({"mode": "stems", "name": "My/ Song"})).unwrap();
        let expected = fixture.file(expected);
        assert_eq!(reserved, json!({"path": expected, "mode": "stems"}));
        assert!(expected.is_dir());
    }
    fixture.dialogs.push(Some(fixture.file("not-a-folder")));
    assert_eq!(fixture.host.request("dialog.export", json!({"mode": "stems"})), Err("Choose an existing folder for the stem export.".to_string()));

    fixture.dialogs.push(Some(PathBuf::from("/media")));
    assert_eq!(fixture.host.request("project.media.chooseRoot", json!({})), Ok(json!({"path": "/media"})));
    fixture.dialogs.push(Some(PathBuf::from("/samples")));
    assert_eq!(fixture.host.request("library.chooseRoot", json!({})), Ok(json!({"scanning": true, "roots": ["/samples"]})));
    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("library.chooseRoot", json!({})), Ok(json!({"cancelled": true})));
}

#[test]
fn choosing_a_library_folder_reports_the_missing_library_until_it_exists() {
    let fixture = Fixture::new();
    fixture.dialogs.push(Some(PathBuf::from("/samples")));
    let error = fixture.host.request("library.chooseRoot", json!({})).unwrap_err();
    assert!(error.contains("`library.scan` is not implemented"), "{error}");
}

#[test]
fn external_links_and_files_go_to_the_desktop_and_nothing_else_does() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.request("app.external", json!({"url": "https://example.com/manual"})), Ok(json!({"opened": true})));
    let here = fixture.file("kick.wav");
    std::fs::write(&here, "RIFF").unwrap();
    assert_eq!(fixture.host.request("app.external", json!({"path": here, "reveal": true})), Ok(json!({"opened": true})));
    assert_eq!(*fixture.opened.0.lock().unwrap(), ["url https://example.com/manual".to_string(), format!("reveal {}", here.display())]);
    for refused in [json!({"url": "file:///etc/passwd"}), json!({"url": "javascript:alert(1)"}), json!({"url": "https://a b"}), json!({}), json!({"path": here})] {
        assert_eq!(fixture.host.request("app.external", refused), Err("Only web links can be opened here.".to_string()));
    }
    let gone = fixture.host.request("app.external", json!({"path": fixture.file("gone.wav"), "reveal": true}));
    assert_eq!(gone, Err("This file is unavailable. Reconnect its drive or rescan the library.".to_string()));
    assert_eq!(fixture.opened.0.lock().unwrap().len(), 2);
}

#[test]
fn interface_settings_persist_across_launches() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.request("settings.set", json!({"key": "pattern.zoom", "value": 1.5})), Ok(json!({})));
    let relaunched = fixture.relaunch(|_| {});
    assert_eq!(relaunched.host.request("settings.get", json!({"key": "pattern.zoom"})), Ok(json!({"value": 1.5})));
    assert_eq!(relaunched.host.request("settings.get", json!({})), Ok(json!({"values": {"pattern.zoom": 1.5}})));
    assert!(relaunched.folder.path().join("support/settings.json").exists());
}

#[test]
fn an_open_request_from_the_system_reaches_the_interface_before_or_after_it_is_ready() {
    // Before `ready`: the path waits in the handshake, once.
    let mut services = None;
    let folder = TempDir::new("host-open");
    let mut bridge = st_host::Bridge::builder();
    let mut options = HostOptions::new(folder.path().join("support"), Arc::new(st_host::NoDialogs));
    options.autosave_every = None;
    services = services.or(Some(install(&mut bridge, options).unwrap()));
    let services = services.unwrap();
    let bridge = bridge.start();
    services.session.request_open(PathBuf::from("/music/Early.supertracker"));
    let page = st_host::Outbox::new();
    let connection = bridge.connect(page.clone());
    // The reply to request `id`; events of the areas (status, library, controller) may come first.
    let reply = |id: u64| -> Value {
        loop {
            let st_host::Next::Message(text) = page.wait(Duration::from_secs(5)) else { panic!("no handshake") };
            let message: Value = serde_json::from_str(&text).unwrap();
            if message["id"] == id {
                return message;
            }
        }
    };
    connection.receive(r#"{"id": 1, "command": "ready"}"#);
    assert_eq!(reply(1)["result"]["openPath"], "/music/Early.supertracker");
    // After: an event.
    services.session.request_open(PathBuf::from("/music/Late.supertracker"));
    let opened: Vec<String> = page.drain().into_iter().filter(|text| text.contains("project.openRequested")).collect();
    assert_eq!(opened, [r#"{"payload":{"path":"/music/Late.supertracker"},"type":"project.openRequested"}"#]);
    connection.receive(r#"{"id": 2, "command": "ready"}"#);
    assert!(reply(2)["result"].get("openPath").is_none());
}

#[test]
fn quitting_asks_the_interface_and_secures_its_final_project() {
    let fixture = Fixture::new();
    let progress: Arc<Mutex<Vec<QuitProgress>>> = Arc::default();
    let heard = progress.clone();
    fixture.services.lifecycle.on_progress(move |step| heard.lock().unwrap().push(step));
    let answer = |payload: Value| fixture.host.request("app.quitAnswer", payload);

    fixture.host.request("ready", json!({})).unwrap();
    assert_eq!(answer(json!({"state": "proceed"})), Err("SuperTracker is not waiting for an answer about quitting.".to_string()));
    assert_eq!(fixture.services.lifecycle.request_quit(), QuitRequest::Asked);
    assert_eq!(fixture.services.lifecycle.request_quit(), QuitRequest::AlreadyAsked);
    assert_eq!(fixture.host.wait_for_event("app.quitRequested", Duration::from_secs(5)), Some(json!({})));
    assert!(fixture.services.lifecycle.is_unanswered());

    // The person cancels: the host keeps running and can be asked again.
    assert_eq!(answer(json!({"state": "deciding"})), Ok(json!({})));
    assert!(!fixture.services.lifecycle.is_unanswered());
    assert_eq!(answer(json!({"state": "stay", "reason": "Finish or cancel the audio export before quitting."})), Ok(json!({})));
    assert_eq!(fixture.services.lifecycle.request_quit(), QuitRequest::Asked);

    // A final project that cannot be secured keeps the question open; force quits anyway.
    let error = answer(json!({"state": "proceed", "project": {"bpm": 1}})).unwrap_err();
    assert!(error.starts_with("Your latest changes could not be secured:"), "{error}");
    assert_eq!(answer(json!({"state": "sideways"})), Err("`app.quitAnswer` does not know the state “sideways”.".to_string()));
    assert_eq!(answer(json!({"state": "proceed", "project": project("Last words")})), Ok(json!({"quitting": true})));
    assert_eq!(
        *progress.lock().unwrap(),
        [
            QuitProgress::Deciding,
            QuitProgress::Stay(Some("Finish or cancel the audio export before quitting.".to_string())),
            QuitProgress::Deciding,
            QuitProgress::Proceed
        ]
    );
    // The final project is in the recovery copy.
    let recovery = std::fs::read(fixture.folder.path().join("support/Recovery.supertracker")).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&recovery).unwrap(), project("Last words"));
    assert_eq!(fixture.services.lifecycle.request_quit(), QuitRequest::AlreadyAsked);
}

#[test]
fn quitting_without_a_ready_interface_needs_no_answer() {
    let folder = TempDir::new("host-quit");
    let mut bridge = st_host::Bridge::builder();
    let mut options = HostOptions::new(folder.path().join("support"), Arc::new(st_host::NoDialogs));
    options.autosave_every = None;
    let services = install(&mut bridge, options).unwrap();
    let _bridge = bridge.start();
    assert_eq!(services.lifecycle.request_quit(), QuitRequest::QuitNow);
}

#[test]
fn the_recovery_timer_writes_without_being_asked() {
    let folder = TempDir::new("host-timer");
    let mut services = None;
    let host = TestHost::start(|bridge| {
        let mut options = HostOptions::new(folder.path().join("support"), Arc::new(st_host::NoDialogs));
        options.autosave_every = Some(Duration::from_millis(20));
        services = Some(install(bridge, options).unwrap());
    });
    host.request("engine.project", json!({"project": project("Timed")})).unwrap();
    let recovery = folder.path().join("support/Recovery.supertracker");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !recovery.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&recovery).unwrap()).unwrap(), project("Timed"));
    services.unwrap().session.stop();
}

#[test]
fn a_project_in_a_retired_layout_is_refused_with_st_projects_own_words() {
    let fixture = Fixture::new();
    let mut old = project("Old");
    old["timing"] = json!({"tempoMap": [{"beat": 0, "bpm": 90}]});
    let expected = "This project was saved in an older layout (a tempo map) that this version no longer reads. Open and save it with a version of SuperTracker that converts it, then open it again.";
    assert_eq!(fixture.host.request("engine.project", json!({"project": old})), Err(expected.to_string()));
    let file = fixture.file("Old.supertracker");
    std::fs::write(&file, old.to_string()).unwrap();
    assert_eq!(fixture.host.request("project.open", json!({"path": file})), Err(expected.to_string()));
    assert!(fixture.services.session.project().is_none());
}

#[test]
fn a_project_without_routing_is_accepted_like_one_with_standard_routing() {
    let fixture = Fixture::new();
    let mut plain = project("Plain");
    plain.as_object_mut().unwrap().remove("routing");
    assert_eq!(fixture.host.request("engine.project", json!({"project": plain})), Ok(json!({"audio": false})));
    let mut standard = project("Standard");
    standard["tracks"][0]["routingID"] = json!("track-1");
    standard["routing"] = json!({"version": 1, "mixModel": "standard", "buses": [], "outputs": {}, "sends": [], "sidechains": []});
    assert_eq!(fixture.host.request("engine.project", json!({"project": standard})), Ok(json!({"audio": false})));
}

#[test]
fn templates_are_saved_listed_made_default_and_used_by_new() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.request("project.templates.list", json!({})), Ok(json!({"templates": [], "defaultID": null})));
    assert_eq!(fixture.host.request("project.templates.save", json!({"name": "Live set"})), Err("Choose a project and name for the template.".to_string()));
    let saved = fixture.host.request("project.templates.save", json!({"project": project("Live set"), "name": "  Live set "})).unwrap();
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["name"], "Live set");
    assert!(std::path::Path::new(saved["path"].as_str().unwrap()).is_file());
    // Saving a template touches neither the session nor its recovery copy.
    assert!(fixture.services.session.project().is_none());

    let listed = fixture.host.request("project.templates.list", json!({})).unwrap();
    assert_eq!(listed, json!({"templates": [saved.clone()], "defaultID": null}));
    assert_eq!(fixture.host.request("project.templates.setDefault", json!({})), Err("Choose a template identifier, or null for a blank default.".to_string()));
    assert_eq!(fixture.host.request("project.templates.setDefault", json!({"id": id})), Ok(json!({"defaultID": id})));

    // New uses the default template; `blank` and an explicit template override it.
    let from_default = fixture.host.request("project.new", json!({})).unwrap();
    assert_eq!(from_default["project"]["name"], "Live set");
    assert_eq!(fixture.host.request("project.new", json!({"blank": true})), Ok(Value::Null));
    assert_eq!(fixture.host.request("project.new", json!({"templateID": id})).unwrap()["project"]["name"], "Live set");
    assert_eq!(fixture.host.request("project.templates.setDefault", json!({"id": null})), Ok(json!({"defaultID": null})));
    assert_eq!(fixture.host.request("project.new", json!({})), Ok(Value::Null));

    // The next launch starts from the default template.
    fixture.host.request("project.templates.setDefault", json!({"id": id})).unwrap();
    let relaunched = fixture.relaunch(|_| {});
    assert_eq!(relaunched.host.request("ready", json!({})).unwrap()["project"]["name"], "Live set");
}

#[test]
fn midi_files_export_and_import_through_the_dialogs_or_a_path() {
    let fixture = Fixture::new();
    assert_eq!(fixture.host.request("project.exportMIDI", json!({})), Err("There is no project to export.".to_string()));
    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("project.exportMIDI", json!({"project": project("Melody")})), Ok(json!({"cancelled": true})));
    fixture.dialogs.push(Some(fixture.file("Melody")));
    let exported = fixture.host.request("project.exportMIDI", json!({"project": project("Melody")})).unwrap();
    let file = fixture.file("Melody.mid");
    assert_eq!(exported["path"], json!(file));
    assert_eq!(exported["noteCount"], 1);
    assert_eq!(fixture.dialogs.asked()[1].suggested_name.as_deref(), Some("Melody.mid"));
    assert_eq!(&std::fs::read(&file).unwrap()[..4], b"MThd");

    fixture.dialogs.push(None);
    assert_eq!(fixture.host.request("project.importMIDI", json!({})), Ok(json!({"cancelled": true})));
    let imported = fixture.host.request("project.importMIDI", json!({"path": file})).unwrap();
    assert_eq!(imported["path"], json!(file));
    assert_eq!(imported["project"]["name"], "Melody");
    assert!(imported["warnings"].is_array());
    // The import is a new session that has never been saved.
    assert_eq!(fixture.services.session.path(), None);
    assert!(fixture.services.session.dirty());
    assert_eq!(fixture.services.session.project().unwrap().display_name(), "Melody");
    let garbage = fixture.file("garbage.mid");
    std::fs::write(&garbage, "not midi").unwrap();
    assert!(fixture.host.request("project.importMIDI", json!({"path": garbage})).is_err());
    assert_eq!(fixture.services.session.project().unwrap().display_name(), "Melody", "a failed import leaves the session alone");
}

#[test]
fn missing_media_is_inspected_searched_and_relinked() {
    let fixture = Fixture::new();
    fixture.host.request("ready", json!({})).unwrap();
    assert_eq!(fixture.host.request("project.media.inspect", json!({})), Err("Open a project before relinking media.".to_string()));
    let found = fixture.file("found/deep/kick.wav");
    std::fs::create_dir_all(found.parent().unwrap()).unwrap();
    std::fs::write(&found, b"RIFF kick").unwrap();
    let mut song = project("Relink");
    song["instruments"][0]["samplePath"] = json!("/gone/drive/kick.wav");
    fixture.host.request("engine.project", json!({"project": song})).unwrap();

    let inspected = fixture.host.request("project.media.inspect", json!({})).unwrap();
    assert_eq!((inspected["referenceCount"].as_u64(), inspected["missingCount"].as_u64()), (Some(1), Some(1)));
    let missing = inspected["missing"][0].clone();
    assert_eq!(missing["path"], "/gone/drive/kick.wav");

    assert_eq!(fixture.host.request("project.media.search", json!({"roots": ["relative/folder"]})), Err("Choose up to 64 absolute media folders or root mappings.".to_string()));
    let searched = fixture.host.request("project.media.search", json!({"roots": [fixture.file("found")], "requestId": "r1"})).unwrap();
    assert_eq!(searched["requestId"], "r1");
    assert_eq!(searched["missing"][0]["candidates"][0]["path"], json!(found));
    // Progress is reported as events that carry the request's id, from "starting" to "complete".
    let events: Vec<(String, Value)> = fixture.host.events().into_iter().filter(|(name, _)| name == "project.media.progress").collect();
    assert_eq!(events.first().unwrap().1["phase"], "starting");
    assert_eq!(events.last().unwrap().1["phase"], "complete");
    assert!(events.iter().all(|(_, payload)| payload["requestId"] == "r1" && payload["operationId"] == searched["operationId"]));

    assert_eq!(fixture.host.request("project.media.apply", json!({})), Err("Select replacement files before applying media relinks.".to_string()));
    let selection = json!({"id": missing["id"], "from": "/gone/drive/kick.wav", "to": found, "allowDifferentContent": true});
    let applied = fixture.host.request("project.media.apply", json!({"selections": [selection]})).unwrap();
    assert_eq!(applied["changed"], 1);
    assert_eq!(applied["project"]["instruments"][0]["samplePath"], json!(found));
    // Applying returns the relinked project; the session changes when the interface publishes it.
    assert_eq!(fixture.services.session.project().unwrap().to_value()["instruments"][0]["samplePath"], "/gone/drive/kick.wav");
    assert_eq!(fixture.host.request("project.media.cancel", json!({})), Ok(json!({"cancelled": true, "operations": 0})));
}

#[test]
fn a_media_search_stops_when_it_is_cancelled() {
    let fixture = Fixture::new();
    fixture.host.request("ready", json!({})).unwrap();
    // Enough folders that the walk is still going when the cancellation arrives.
    let root = fixture.file("haystack");
    for folder in 0..200 {
        let path = root.join(format!("d{folder}"));
        std::fs::create_dir_all(&path).unwrap();
        for file in 0..50 {
            std::fs::write(path.join(format!("f{file}.wav")), b"x").unwrap();
        }
    }
    let mut song = project("Cancel");
    song["instruments"][0]["samplePath"] = json!("/gone/needle.wav");
    fixture.host.request("engine.project", json!({"project": song})).unwrap();
    let search = fixture.host.send("project.media.search", json!({"roots": [root], "requestId": "slow"}));
    assert!(fixture.host.wait_for_event("project.media.progress", Duration::from_secs(5)).is_some());
    let cancelled = fixture.host.request("project.media.cancel", json!({})).unwrap();
    match fixture.host.reply(search) {
        // The usual case: the walk was still running.
        Err(error) => {
            assert_eq!(error, "The operation was cancelled.");
            assert_eq!(cancelled, json!({"cancelled": true, "operations": 1}));
        }
        // A very fast disk finished first; then there was nothing left to cancel.
        Ok(report) => assert_eq!((report["requestId"].as_str(), cancelled["operations"].as_u64()), (Some("slow"), Some(0))),
    }
    assert_eq!(fixture.host.bridge.pending(), 0);
}
