//! Plug-in scanning, end to end: a `Scanner` in this process, real container processes
//! doing the listing, real plug-in files (the CLAP and VST3 test plug-ins) next to files
//! that crash the scanner, hang it, or are no plug-ins at all.
#![allow(clippy::bool_assert_comparison)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use stpc_clap_test_plugin as clap_plug;
use stpc_vst3_test_plugin as vst3_plug;

/// A folder tree of plug-in files and a scanner configuration that searches only it.
struct Fixture {
    scratch: Scratch,
    vst3: PathBuf,
    clap: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let scratch = Scratch::new(tag);
        let (vst3, clap) = (scratch.path("vst3"), scratch.path("clap"));
        std::fs::create_dir_all(&vst3).unwrap();
        std::fs::create_dir_all(&clap).unwrap();
        std::fs::create_dir_all(scratch.path("sockets")).unwrap();
        Fixture { scratch, vst3, clap }
    }

    fn config(&self) -> ScannerConfig {
        ScannerConfig {
            container_path: Some(PathBuf::from(CONTAINER)),
            socket_dir: Some(self.scratch.path("sockets")),
            cache_path: Some(self.scratch.path("cache").join("plugins.stpcscan")),
            formats: vec!["vst3".into(), "clap".into()],
            standard_paths: false,
            extra_paths: vec![("vst3".into(), self.vst3.clone()), ("clap".into(), self.clap.clone())],
            timeout: Duration::from_millis(1500),
            jobs: 3,
            ..ScannerConfig::default()
        }
    }

    fn scanner(&self) -> Scanner {
        Scanner::new(self.config()).unwrap()
    }

    fn clap_plugin(&self, name: &str) -> PathBuf {
        let path = self.clap.join(name);
        std::fs::copy(test_library("stpc_clap_test_plugin"), &path).unwrap();
        path
    }

    /// Nothing is left of the scanning processes: no process, no socket.
    fn assert_clean(&self, pids: &[u32]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        for &pid in pids {
            while process_exists(pid) {
                assert!(Instant::now() < deadline, "scanning process {pid} still exists (running or zombie)");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        for entry in std::fs::read_dir(self.scratch.path("sockets")).unwrap().flatten() {
            let left: Vec<_> = std::fs::read_dir(entry.path()).unwrap().flatten().map(|e| e.path()).collect();
            assert!(left.is_empty(), "sockets left behind: {left:?}");
        }
    }
}

/// What a scan reported, in a form the tests can assert on.
#[derive(Default)]
struct Seen {
    discovered: Option<usize>,
    started: Vec<(String, PathBuf, u32)>,
    completed: Vec<(usize, usize, PathBuf, FileOutcome)>,
}

impl Seen {
    fn outcome(&self, path: &Path) -> &FileOutcome {
        &self.completed.iter().find(|entry| entry.2 == path).unwrap_or_else(|| panic!("no result for {path:?}")).3
    }
    fn pids(&self) -> Vec<u32> {
        self.started.iter().map(|entry| entry.2).collect()
    }
}

fn scan(scanner: &Scanner, options: ScanOptions) -> (ScanReport, Seen) {
    let mut seen = Seen::default();
    let report = scanner.scan(options, &mut |event| match event {
        ScanEvent::Discovered { total } => seen.discovered = Some(total),
        ScanEvent::Started { format, path, pid } => seen.started.push((format.to_string(), path.to_path_buf(), pid)),
        ScanEvent::Completed { completed, total, path, outcome, .. } => {
            seen.completed.push((completed, total, path.to_path_buf(), outcome.clone()))
        }
    });
    // Progress counts every file exactly once, in order.
    let total = seen.discovered.expect("a Discovered event");
    assert_eq!(report.files, total);
    if !report.cancelled {
        assert_eq!(seen.completed.iter().map(|entry| entry.0).collect::<Vec<_>>(), (1..=total).collect::<Vec<_>>());
    }
    assert!(seen.completed.iter().all(|entry| entry.1 == total));
    assert_eq!(report.scanned, seen.started.len());
    (report, seen)
}

fn names(plugins: &[PluginDescription]) -> Vec<&str> {
    plugins.iter().map(|plugin| plugin.name.as_str()).collect()
}

#[test]
fn scan_lists_plugins_caches_them_and_survives_broken_files() {
    let fixture = Fixture::new("scan");
    let good = vst3_bundle(&fixture.vst3, "Good");
    let second = vst3_bundle(&fixture.vst3.join("sub"), "Second");
    let crash_on_load = vst3_bundle(&fixture.vst3, "CrashOnLoad");
    let crash_in_factory = vst3_bundle(&fixture.vst3, "CrashInFactory");
    let hang = vst3_bundle(&fixture.vst3, "HangOnScan");
    // Bundles that are no plug-ins: text where the library should be, nothing at all, and
    // a plain file with the extension.
    let fake = fixture.vst3.join("Fake.vst3");
    let fake_library = fake.join("Contents").join(format!("{}-linux", std::env::consts::ARCH)).join("Fake.so");
    std::fs::create_dir_all(fake_library.parent().unwrap()).unwrap();
    std::fs::write(&fake_library, b"this is not a shared library").unwrap();
    let empty = fixture.vst3.join("Empty.vst3");
    std::fs::create_dir_all(&empty).unwrap();
    let text = fixture.vst3.join("Text.vst3");
    std::fs::write(&text, b"just text").unwrap();
    let clap = fixture.clap_plugin("Test.clap");
    let garbage = fixture.clap.join("garbage.clap");
    std::fs::write(&garbage, b"not a plug-in either").unwrap();
    // A real shared library that is no plug-in.
    let libm = ["/lib/x86_64-linux-gnu/libm.so.6", "/usr/lib/libm.so.6", "/lib64/libm.so.6"]
        .into_iter()
        .find(|path| Path::new(path).exists())
        .expect("a libm to copy");
    let foreign = fixture.clap.join("libm.clap");
    std::fs::copy(libm, &foreign).unwrap();
    let total = 11;

    let scanner = fixture.scanner();
    assert_eq!(scanner.search_paths("vst3"), std::slice::from_ref(&fixture.vst3));
    assert!(scanner.plugins().is_empty() && scanner.files().is_empty());
    let started = Instant::now();
    let (report, seen) = scan(&scanner, ScanOptions::default());
    let elapsed = started.elapsed();
    assert_eq!((report.files, report.scanned, report.cached, report.cancelled), (total, total, 0, false));
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    // One file hangs for the whole time limit; the others do not wait for it.
    assert!(elapsed >= Duration::from_millis(1500) && elapsed < Duration::from_secs(8), "{elapsed:?}");

    // The plug-ins: both formats, each once although two bundles hold the VST3 ones.
    assert_eq!(
        names(&report.plugins),
        [
            clap_plug_name("Gain"),
            vst3_plug::GAIN_NAME,
            clap_plug_name("Sine"),
            "ST Test Sine (CLAP notes)",
            vst3_plug::SINE_NAME
        ]
    );
    assert_eq!(report.plugins, scanner.plugins());
    let gain = &report.plugins[1];
    assert_eq!(
        *gain,
        PluginDescription {
            format: "vst3".into(),
            id: vst3_plug::GAIN_CLASS_ID.into(),
            name: vst3_plug::GAIN_NAME.into(),
            vendor: vst3_plug::VENDOR.into(),
            version: vst3_plug::VERSION.into(),
            category: "Fx|Dynamics".into(),
            subcategories: vec!["Fx".into(), "Dynamics".into()],
            flags: SCAN_EFFECT | SCAN_AUDIO_INPUT | SCAN_AUDIO_OUTPUT,
            // Of the two bundles, the one found first.
            path: good.to_string_lossy().into_owned(),
            sdk_version: "VST 3.8.0".into(),
            url: "https://example.invalid/supertracker".into(),
        }
    );
    let sine = &report.plugins[4];
    assert_eq!(
        (sine.id.as_str(), sine.category.as_str(), sine.flags),
        (vst3_plug::SINE_CLASS_ID, "Instrument|Synth", SCAN_INSTRUMENT | SCAN_WANTS_MIDI | SCAN_AUDIO_OUTPUT)
    );
    let clap_gain = &report.plugins[0];
    assert_eq!(
        (clap_gain.format.as_str(), clap_gain.id.as_str(), clap_gain.vendor.as_str(), clap_gain.version.as_str()),
        ("clap", clap_plug::GAIN_ID, "SuperTracker Tests", "1.2.3")
    );
    assert_eq!(clap_gain.subcategories, ["audio-effect", "stereo"]);
    assert_eq!(clap_gain.flags, SCAN_EFFECT | SCAN_AUDIO_INPUT | SCAN_AUDIO_OUTPUT);
    assert_eq!(clap_gain.path, clap.to_string_lossy());
    for clap_sine in &report.plugins[2..4] {
        assert_eq!(clap_sine.flags, SCAN_INSTRUMENT | SCAN_WANTS_MIDI | SCAN_AUDIO_OUTPUT);
    }
    // The controller class of the VST3 gain is in its factory but is no plug-in.
    assert!(!report.plugins.iter().any(|plugin| plugin.id == vst3_plug::GAIN_CONTROLLER_CLASS_ID));

    // The broken files: each one cost a scanning process, none of them the scan.
    assert_eq!(*seen.outcome(&good), FileOutcome::Scanned { plugins: 2 });
    assert_eq!(*seen.outcome(&second), FileOutcome::Scanned { plugins: 2 });
    assert_eq!(*seen.outcome(&clap), FileOutcome::Scanned { plugins: 3 });
    let crashed = |path: &Path, needle: &str| match seen.outcome(path) {
        FileOutcome::Crashed { reason } => assert!(reason.contains(needle), "\"{reason}\" lacks \"{needle}\""),
        other => panic!("{path:?}: expected a crash, got {other:?}"),
    };
    crashed(&crash_on_load, "SIGABRT");
    crashed(&crash_in_factory, "SIGSEGV");
    assert_eq!(*seen.outcome(&hang), FileOutcome::TimedOut);
    let failed = |path: &Path, needle: &str| match seen.outcome(path) {
        FileOutcome::Failed { reason } => assert!(reason.contains(needle), "\"{reason}\" lacks \"{needle}\""),
        other => panic!("{path:?}: expected a failure, got {other:?}"),
    };
    failed(&fake, "cannot load");
    failed(&empty, "has no library for this machine");
    failed(&text, "cannot load");
    failed(&garbage, "cannot load");
    failed(&foreign, "not a CLAP plug-in");
    assert_eq!(report.failures.len(), 8);
    // The blocklist: what crashed or hung, with the reason.
    let blocked = scanner.blocklist();
    assert_eq!(
        blocked.iter().map(|file| (file.path.clone(), file.status)).collect::<Vec<_>>(),
        [
            (crash_in_factory.clone(), ScanStatus::Crashed),
            (crash_on_load.clone(), ScanStatus::Crashed),
            (hang.clone(), ScanStatus::TimedOut)
        ]
    );
    assert!(blocked[2].reason.contains("no answer within 1.5s"), "{}", blocked[2].reason);
    fixture.assert_clean(&seen.pids());

    // Again: nothing has changed, so nothing is loaded: no process is started at all.
    let (report, seen) = scan(&scanner, ScanOptions::default());
    assert_eq!((report.files, report.scanned, report.cached), (total, 0, 3));
    assert_eq!(*seen.outcome(&good), FileOutcome::Cached { plugins: 2 });
    assert!(matches!(seen.outcome(&hang), FileOutcome::Skipped { status: ScanStatus::TimedOut, .. }));
    assert!(matches!(seen.outcome(&fake), FileOutcome::Skipped { status: ScanStatus::Failed, .. }));
    assert_eq!(names(&report.plugins).len(), 5);
    assert_eq!(report.failures.len(), 8);

    // The cache is a file: another scanner (the next run of the application) starts with it.
    let cache = std::fs::read(fixture.config().cache_path.unwrap()).unwrap();
    assert_eq!(&cache[..12], b"STPCSCAN\x01\x00\x00\x00");
    let reopened = fixture.scanner();
    assert_eq!(reopened.plugins(), scanner.plugins());
    assert_eq!(reopened.blocklist(), scanner.blocklist());
    let (report, _) = scan(&reopened, ScanOptions::default());
    assert_eq!((report.scanned, report.cached), (0, 3));

    // A file that changed is scanned again, and only that one: the fake bundle gets a
    // real library, the good one's library is replaced by a broken one.
    std::fs::copy(test_library("stpc_vst3_test_plugin"), &fake_library).unwrap();
    let (report, seen) = scan(&reopened, ScanOptions::default());
    assert_eq!((report.scanned, report.cached), (1, 3));
    assert_eq!(*seen.outcome(&fake), FileOutcome::Scanned { plugins: 2 });
    assert_eq!(report.failures.len(), 7);

    // Retrying: one blocklisted file by name (it crashes again and is blocked again) ...
    assert!(reopened.unblock("vst3", &crash_on_load));
    assert!(!reopened.unblock("vst3", &crash_on_load));
    assert_eq!(reopened.blocklist().len(), 2);
    let (report, seen) = scan(&reopened, ScanOptions::default());
    assert_eq!(report.scanned, 1);
    assert!(matches!(seen.outcome(&crash_on_load), FileOutcome::Crashed { .. }));
    assert_eq!(reopened.blocklist().len(), 3);
    // ... one file at once, whatever the cache says ...
    assert_eq!(names(&reopened.scan_file("vst3", &good).unwrap()), [vst3_plug::GAIN_NAME, vst3_plug::SINE_NAME]);
    assert!(matches!(reopened.scan_file("vst3", &hang), Err(Error::Timeout(_))));
    assert!(matches!(reopened.scan_file("vst3", &crash_in_factory), Err(Error::Plugin(_))));
    assert!(matches!(reopened.scan_file("vst3", Path::new("/nonexistent.vst3")), Err(Error::Invalid(_))));
    // ... or everything that failed.
    let (report, seen) = scan(&reopened, ScanOptions { retry_failed: true, ..ScanOptions::default() });
    assert_eq!((report.scanned, report.cached), (7, 4));
    assert_eq!(report.failures.len(), 7, "they are as broken as before");
    fixture.assert_clean(&seen.pids());
    // And a full rescan ignores the cache altogether.
    let (report, _) = scan(&reopened, ScanOptions { rescan_all: true, ..ScanOptions::default() });
    assert_eq!((report.scanned, report.cached), (total, 0));

    // A plug-in that was removed disappears from the results; the other bundle now
    // provides the VST3 plug-ins.
    std::fs::remove_dir_all(&good).unwrap();
    std::fs::remove_dir_all(&fake).unwrap();
    let (report, _) = scan(&reopened, ScanOptions::default());
    assert_eq!((report.files, report.scanned), (total - 2, 0));
    assert_eq!(report.plugins[1].path, second.to_string_lossy());
    assert!(!reopened.files().iter().any(|file| file.path == good));
    // Forgetting all failures makes the next scan try them again.
    reopened.clear_failures();
    assert!(reopened.blocklist().is_empty());
    let (report, _) = scan(&reopened, ScanOptions::default());
    assert_eq!(report.scanned, 7);
    assert_eq!(reopened.blocklist().len(), 3);
}

fn clap_plug_name(kind: &str) -> &'static str {
    match kind {
        "Gain" => "ST Test Gain",
        _ => "ST Test Sine",
    }
}

#[test]
fn scan_can_be_cancelled_and_keeps_what_it_knew() {
    let fixture = Fixture::new("scan-cancel");
    let good = vst3_bundle(&fixture.vst3, "AGood");
    for index in 0..4 {
        vst3_bundle(&fixture.vst3, &format!("HangOnScan{index}"));
    }
    let scanner = Scanner::new(ScannerConfig {
        timeout: Duration::from_secs(60),
        jobs: 2,
        formats: vec!["vst3".into()],
        ..fixture.config()
    })
    .unwrap();
    // Cancel from the progress callback as soon as the good file is in and a hanging one
    // has a process; another thread could call `cancel` just as well.
    let mut pids = Vec::new();
    let mut outcomes = Vec::new();
    let started = Instant::now();
    let canceller = scanner.clone();
    let report = scanner.scan(ScanOptions::default(), &mut |event| match event {
        ScanEvent::Started { path, pid, .. } => {
            pids.push(pid);
            if path != good {
                std::thread::sleep(Duration::from_millis(50));
                canceller.cancel();
            }
        }
        ScanEvent::Completed { path, outcome, .. } => outcomes.push((path.to_path_buf(), outcome.clone())),
        ScanEvent::Discovered { .. } => {}
    });
    assert!(report.cancelled);
    assert!(started.elapsed() < Duration::from_secs(10), "cancelling must not wait for the time limit");
    // Nothing was recorded for the files that were being scanned or never reached.
    assert!(outcomes.iter().any(|(path, outcome)| path != &good && matches!(outcome, FileOutcome::NotScanned { .. })));
    assert!(scanner.blocklist().is_empty());
    assert!(scanner.files().iter().all(|file| file.path == good));
    fixture.assert_clean(&pids);
    // What was finished before the cancellation is kept, here and in the cache file.
    if outcomes.iter().any(|(path, outcome)| path == &good && matches!(outcome, FileOutcome::Scanned { .. })) {
        assert_eq!(names(&report.plugins), [vst3_plug::GAIN_NAME, vst3_plug::SINE_NAME]);
        assert_eq!(names(&fixture.scanner().plugins()), [vst3_plug::GAIN_NAME, vst3_plug::SINE_NAME]);
    }
}

#[test]
fn scan_results_are_what_an_instance_is_created_from() {
    let fixture = Fixture::new("scan-create");
    vst3_bundle(&fixture.vst3, "STTest");
    fixture.clap_plugin("Test.clap");
    let scanner = fixture.scanner();
    let (report, seen) = scan(&scanner, ScanOptions::default());
    assert_eq!(report.plugins.len(), 5);
    fixture.assert_clean(&seen.pids());

    let env = TestEnv::new(GroupingPolicy::PerPlugin);
    let mut rng = Rng(0x5CA7);
    let input = vec![rng.signal(64), rng.signal(64)];
    for plugin in &report.plugins {
        let instrument = plugin.flags & SCAN_INSTRUMENT != 0;
        assert_eq!(instrument, plugin.flags & SCAN_EFFECT == 0);
        let base = if instrument {
            InstanceConfig::instrument(&plugin.format, &plugin.id)
        } else {
            InstanceConfig::effect(&plugin.format, &plugin.id)
        };
        let instance = active(
            env.sup(),
            InstanceConfig {
                options: plugin.path.clone(),
                manufacturer: plugin.vendor.clone(),
                name: plugin.name.clone(),
                deadline: Some(LONG),
                ..base
            },
        );
        let info = instance.info();
        assert_eq!((info.name.as_str(), info.vendor.as_str()), (plugin.name.as_str(), plugin.vendor.as_str()));
        assert_eq!(info.flags & INFO_INSTRUMENT != 0, instrument);
        if instrument {
            let block = run(&instance, &[], 2, 64, &[Event::note_on(0, 0, 69, 100)]);
            assert!(block.outcome.processed() && block.outputs[0].iter().any(|&s| s != 0.0), "{}", plugin.name);
        } else {
            let block = run(&instance, &input, 2, 64, &[]);
            assert_bits_eq(&block.outputs[0], &input[0], &plugin.name);
        }
    }
    env.finish();
}

#[test]
fn unknown_formats_and_a_missing_container_are_reported() {
    let fixture = Fixture::new("scan-odd");
    // A format the container has no scanner for (Audio Units, off macOS): the registry
    // request fails; that is a failure of the scan of that format, not an error of the scan.
    let scanner = Scanner::new(ScannerConfig { formats: vec!["bogus".into()], ..fixture.config() }).unwrap();
    let (report, seen) = scan(&scanner, ScanOptions::default());
    assert_eq!((report.files, report.scanned, report.plugins.len()), (1, 1, 0));
    match seen.outcome(Path::new("")) {
        FileOutcome::Failed { reason } => assert!(reason.contains("no scanner for the format \"bogus\""), "{reason}"),
        other => panic!("{other:?}"),
    }
    // A registry is asked again every time (there is no file whose change could be seen).
    let (report, _) = scan(&scanner, ScanOptions::default());
    assert_eq!(report.scanned, 1);
    fixture.assert_clean(&seen.pids());

    // Without a container executable there is no scanner.
    let missing = ScannerConfig { container_path: Some("/nonexistent/container".into()), ..fixture.config() };
    assert!(matches!(Scanner::new(missing), Err(Error::Spawn(_))));
    // An executable that is no container: nothing is recorded against the plug-in files.
    vst3_bundle(&fixture.vst3, "Good");
    let impostor = ScannerConfig {
        container_path: Some("/bin/true".into()),
        spawn_timeout: Duration::from_secs(5),
        cache_path: None,
        ..fixture.config()
    };
    let scanner = Scanner::new(impostor).unwrap();
    let (report, seen) = scan(&scanner, ScanOptions::default());
    assert_eq!((report.files, report.scanned), (1, 0));
    assert!(matches!(seen.completed[0].3, FileOutcome::NotScanned { .. }));
    assert!(scanner.files().is_empty() && report.failures.is_empty());
}
