//! Helpers shared by the integration tests. Every test spawns the real
//! `supertracker-plugin-container` binary through a real `Supervisor`.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use stpc_client::*;

pub const CONTAINER: &str = env!("CARGO_BIN_EXE_supertracker-plugin-container");

/// `STPC_TEST_WAKE=pipe` runs the whole suite over the pipe wake-up (the portable path).
pub fn wake_preference() -> WakePreference {
    match std::env::var("STPC_TEST_WAKE").as_deref() {
        Ok("pipe") => WakePreference::Pipe,
        Ok("address") => WakePreference::Address,
        _ => WakePreference::Auto,
    }
}

pub fn base_config() -> SupervisorConfig {
    SupervisorConfig {
        container_path: Some(PathBuf::from(CONTAINER)),
        wake: wake_preference(),
        ..SupervisorConfig::default()
    }
}

/// A supervisor with its own socket base directory, plus the bookkeeping needed to prove
/// at the end that nothing was left behind.
pub struct TestEnv {
    pub sup: Option<Supervisor>,
    pub base: PathBuf,
    pub pids: Arc<Mutex<Vec<u32>>>,
    pub events: Arc<Mutex<Vec<HostEvent>>>,
}

static ENV_COUNTER: AtomicU32 = AtomicU32::new(0);

impl TestEnv {
    pub fn new(policy: GroupingPolicy) -> TestEnv {
        TestEnv::with(|config| config.policy = policy)
    }

    pub fn with(adjust: impl FnOnce(&mut SupervisorConfig)) -> TestEnv {
        let base = std::env::temp_dir().join(format!(
            "stpc-it-{}-{}",
            std::process::id(),
            ENV_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        let mut config = base_config();
        config.socket_dir = Some(base.clone());
        adjust(&mut config);
        let sup = Supervisor::new(config).expect("supervisor");
        let pids = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let (pids_cb, events_cb) = (pids.clone(), events.clone());
        sup.set_event_callback(Some(Box::new(move |event| {
            if let HostEvent::ContainerStarted { pid, .. } = event {
                pids_cb.lock().unwrap().push(*pid);
            }
            events_cb.lock().unwrap().push(event.clone());
        })));
        TestEnv { sup: Some(sup), base, pids, events }
    }

    pub fn sup(&self) -> &Supervisor {
        self.sup.as_ref().unwrap()
    }

    pub fn events(&self) -> Vec<HostEvent> {
        self.events.lock().unwrap().clone()
    }

    /// Waits until an event satisfies `pred` (also looking at events already received).
    pub fn wait_event(&self, what: &str, timeout: Duration, pred: impl Fn(&HostEvent) -> bool) -> HostEvent {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.events.lock().unwrap().iter().find(|e| pred(e)) {
                return event.clone();
            }
            assert!(Instant::now() < deadline, "no event \"{what}\" within {timeout:?}; got {:#?}", self.events());
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn clear_events(&self) {
        self.events.lock().unwrap().clear();
    }

    /// Process ids of running containers.
    pub fn live_pids(&self) -> Vec<u32> {
        self.sup().containers().iter().filter(|c| c.state == ContainerState::Running).map(|c| c.pid).collect()
    }

    /// Shuts the supervisor down and asserts that no process, zombie, socket, socket
    /// directory or named shared-memory object is left.
    pub fn finish(mut self) {
        drop(self.sup.take());
        assert_clean(&self.base, &self.pids.lock().unwrap());
        std::fs::remove_dir(&self.base).unwrap();
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // A failing test must not leave container processes behind either.
        drop(self.sup.take());
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

pub fn process_exists(pid: u32) -> bool {
    // A zombie still has its /proc entry, so "does not exist" also means "was reaped".
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

pub fn assert_clean(base: &std::path::Path, pids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    for &pid in pids {
        while process_exists(pid) {
            assert!(Instant::now() < deadline, "container process {pid} still exists (running or zombie)");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let left: Vec<_> = std::fs::read_dir(base).unwrap().flatten().map(|e| e.path()).collect();
    assert!(left.is_empty(), "socket directory not cleaned up: {left:?}");
    let prefix = format!("stpc-{:08x}-", std::process::id());
    if let Ok(dir) = std::fs::read_dir("/dev/shm") {
        let leaked: Vec<_> = dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).collect();
        assert!(leaked.is_empty(), "leaked shared-memory objects: {leaked:?}");
    }
}

/// Children of this process that run the container binary (running or zombie).
pub fn container_children() -> Vec<u32> {
    let me = std::process::id();
    let mut out = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { continue };
        // pid (comm) state ppid ...
        let Some(after) = stat.rfind(')').map(|i| &stat[i + 1..]) else { continue };
        let mut fields = after.split_whitespace();
        let _state = fields.next();
        if fields.next().and_then(|p| p.parse::<u32>().ok()) == Some(me) {
            out.push(pid);
        }
    }
    out
}

/// A generous deadline: functional tests must not depend on the scheduler.
pub const LONG: Duration = Duration::from_secs(10);

pub fn effect(plugin: &str) -> InstanceConfig {
    InstanceConfig { deadline: Some(LONG), manufacturer: "Tests".into(), ..InstanceConfig::effect("test", plugin) }
}

pub fn instrument(plugin: &str) -> InstanceConfig {
    InstanceConfig { deadline: Some(LONG), manufacturer: "Tests".into(), ..InstanceConfig::instrument("test", plugin) }
}

pub fn active(sup: &Supervisor, config: InstanceConfig) -> Instance {
    let instance = sup.create_instance(config).expect("create instance");
    instance.activate().expect("activate");
    instance
}

/// Result of one block.
pub struct Block {
    pub outputs: Vec<Vec<f32>>,
    pub outcome: ProcessOutcome,
    pub out_events: Vec<Event>,
    pub elapsed: Duration,
}

/// Processes one block. Output buffers are 8 frames longer than the block and prefilled
/// with a sentinel, to catch writes past the block.
pub fn run(instance: &Instance, inputs: &[Vec<f32>], outputs: usize, frames: usize, events: &[Event]) -> Block {
    run_with(instance, inputs, outputs, frames, events, None)
}

pub const SENTINEL: f32 = 12345.678;

pub fn run_with(
    instance: &Instance,
    inputs: &[Vec<f32>],
    outputs: usize,
    frames: usize,
    events: &[Event],
    transport: Option<&Transport>,
) -> Block {
    let mut out: Vec<Vec<f32>> = (0..outputs).map(|_| vec![SENTINEL; frames + 8]).collect();
    let mut out_events = vec![Event::default(); 512];
    let input_refs: Vec<&[f32]> = inputs.iter().map(|c| c.as_slice()).collect();
    let started = Instant::now();
    let outcome = {
        let mut output_refs: Vec<&mut [f32]> = out.iter_mut().map(|c| c.as_mut_slice()).collect();
        instance.process(&mut ProcessBuffers {
            frames,
            inputs: &input_refs,
            outputs: &mut output_refs,
            events,
            out_events: &mut out_events,
            transport,
            deadline: None,
        })
    };
    let elapsed = started.elapsed();
    for channel in &mut out {
        assert!(channel[frames..].iter().all(|&s| s == SENTINEL), "process wrote past the block");
        channel.truncate(frames);
    }
    out_events.truncate(outcome.out_events);
    Block { outputs: out, outcome, out_events, elapsed }
}

/// Small deterministic generator (xorshift), so failures reproduce.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// Samples in [-1, 1).
    pub fn signal(&mut self, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0).collect()
    }
    /// Arbitrary bit patterns: NaNs, infinities and denormals included.
    pub fn raw_bits(&mut self, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| f32::from_bits(self.next() as u32)).collect()
    }
}

pub fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|s| s.to_bits()).collect()
}

pub fn assert_bits_eq(actual: &[f32], expected: &[f32], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length");
    if let Some(index) = (0..actual.len()).find(|&i| actual[i].to_bits() != expected[i].to_bits()) {
        panic!(
            "{what}: first difference at frame {index}: got {} ({:#x}), expected {} ({:#x})",
            actual[index],
            actual[index].to_bits(),
            expected[index],
            expected[index].to_bits()
        );
    }
}

// ---- plug-in files for the format backends and the scanner ---------------------------------

/// A directory under the temporary directory, removed when dropped.
pub struct Scratch {
    pub dir: PathBuf,
}

static SCRATCH_COUNTER: AtomicU32 = AtomicU32::new(0);

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "stpc-{tag}-{}-{}",
            std::process::id(),
            SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A test plug-in library cargo built next to this test executable (`stpc_clap_test_plugin`,
/// `stpc_vst3_test_plugin`).
pub fn test_library(crate_name: &str) -> PathBuf {
    let dir = std::env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let extension = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let path = dir.join(format!("lib{crate_name}.{extension}"));
    assert!(path.is_file(), "{} was not built", path.display());
    path
}

/// Packages the VST3 test plug-in as `<dir>/<name>.vst3`, the way an installed plug-in
/// looks on this platform. The library is copied, so each bundle is a module of its own
/// (and the test plug-in can tell by its file name how it is meant to misbehave).
pub fn vst3_bundle(dir: &std::path::Path, name: &str) -> PathBuf {
    let bundle = dir.join(format!("{name}.vst3"));
    let library = test_library("stpc_vst3_test_plugin");
    if cfg!(target_os = "macos") {
        let binaries = bundle.join("Contents").join("MacOS");
        std::fs::create_dir_all(&binaries).unwrap();
        std::fs::copy(&library, binaries.join(name)).unwrap();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n\
             <key>CFBundleExecutable</key><string>{name}</string>\n\
             <key>CFBundleIdentifier</key><string>org.supertracker.tests.{name}</string>\n\
             <key>CFBundlePackageType</key><string>BNDL</string>\n\
             <key>CFBundleShortVersionString</key><string>1.2.3</string>\n</dict></plist>\n"
        );
        std::fs::write(bundle.join("Contents").join("Info.plist"), plist).unwrap();
    } else {
        let binaries = bundle.join("Contents").join(format!("{}-linux", std::env::consts::ARCH));
        std::fs::create_dir_all(&binaries).unwrap();
        std::fs::copy(&library, binaries.join(format!("{name}.so"))).unwrap();
    }
    bundle
}
