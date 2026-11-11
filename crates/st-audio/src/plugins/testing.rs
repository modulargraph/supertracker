//! For tests: the container executable and the test plug-ins of `crates/plugin-host`.
//!
//! [`artifacts`] finds `supertracker-plugin-container`, `libstpc_clap_test_plugin` and
//! `libstpc_vst3_test_plugin` in the cargo target directory of the running test (the
//! directory above the test executable's `deps`), and builds them there with
//! `cargo build --manifest-path crates/plugin-host/Cargo.toml` when they are missing.
//! [`Artifacts::install`] lays the plug-ins out in a folder the way installed plug-ins look.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Ids of the test plug-ins (see `crates/plugin-host/crates/*-test-plugin`).
pub const CLAP_GAIN: &str = "org.supertracker.test.gain";
pub const CLAP_SINE: &str = "org.supertracker.test.sine";
/// The sine instrument whose note port takes CLAP note events only (no MIDI).
pub const CLAP_NOTE_SINE: &str = "org.supertracker.test.sine.notes";
pub const VST3_GAIN: &str = "535456334761696E50726F6300000001";
pub const VST3_SINE: &str = "5354563353696E6553796E7400000003";

/// Parameter ids of the CLAP gain effect (values in the parameter's own range).
pub mod clap_gain {
    /// 0..2, a factor.
    pub const GAIN: u64 = 1;
    pub const BYPASS: u64 = 2;
    /// 1: abort, 2: segfault, inside `process`.
    pub const CRASH: u64 = 3;
    /// The latency the plug-in reports, in frames (it does not delay).
    pub const LATENCY: u64 = 4;
}

/// Parameter ids of the CLAP sine instruments. The probes are read-only and show what arrived
/// last; channel pressure also scales the level by `1 + pressure / 127`. The instrument has a
/// sustain pedal (controller 64, down from 64): a note whose key is released under it sounds
/// until the pedal comes up.
pub mod clap_sine {
    /// 0..1.
    pub const LEVEL: u64 = 1;
    /// Control change: `controller * 128 + value`.
    pub const LAST_CONTROLLER: u64 = 20;
    /// Channel pressure, 0..127.
    pub const PRESSURE: u64 = 21;
    /// Poly pressure: `key * 128 + pressure`.
    pub const POLY_PRESSURE: u64 = 22;
    /// Pitch bend, 0..16383.
    pub const BEND: u64 = 23;
    /// The note: [`SILENT`], [`KEY_DOWN`] or [`SUSTAINED`].
    pub const NOTE_STATE: u64 = 24;
    /// Nothing sounds.
    pub const SILENT: f64 = 0.0;
    /// The note sounds and its key is down.
    pub const KEY_DOWN: f64 = 1.0;
    /// The note-off arrived; the pedal keeps the note sounding.
    pub const SUSTAINED: f64 = 2.0;
}

/// Parameter ids of the VST3 sine instrument (normalized 0..1). Controllers reach it as the
/// parameters it maps them to: 7 is the level, 74 the brightness, 64 the sustain pedal (down
/// from 0.5), channel pressure the pressure (which scales the level by `1 + pressure`), pitch
/// bend the bend.
pub mod vst3_sine {
    pub const LEVEL: u64 = 1;
    pub const BEND: u64 = 3;
    pub const PRESSURE: u64 = 4;
    pub const BRIGHTNESS: u64 = 5;
    /// Read-only: the last poly pressure event, `(pitch * 128 + pressure 0..127) / 16383`.
    pub const POLY_PRESSURE: u64 = 6;
    /// The sustain pedal: a note whose key is released under it sounds until it comes up.
    pub const SUSTAIN: u64 = 7;
    /// Read-only, the note: [`SILENT`], [`KEY_DOWN`] or [`SUSTAINED`].
    pub const NOTE_STATE: u64 = 8;
    /// Nothing sounds.
    pub const SILENT: f64 = 0.0;
    /// The note sounds and its key is down.
    pub const KEY_DOWN: f64 = 0.5;
    /// The note-off arrived; the pedal keeps the note sounding.
    pub const SUSTAINED: f64 = 1.0;
}

/// Parameter ids of the VST3 gain effect (normalized 0..1).
pub mod vst3_gain {
    /// 0..1 is a factor of 0..2.
    pub const GAIN: u64 = 100;
    pub const BYPASS: u64 = 101;
    /// 0..1 is 0..1024 frames.
    pub const LATENCY: u64 = 102;
    pub const CRASH: u64 = 103;
    /// How much of the sidechain input is added to the output.
    pub const SIDECHAIN: u64 = 105;
}

#[derive(Debug, Clone)]
pub struct Artifacts {
    pub container: PathBuf,
    pub clap_library: PathBuf,
    pub vst3_library: PathBuf,
}

/// The plug-ins of [`Artifacts::install`].
#[derive(Debug, Clone)]
pub struct Installed {
    pub folder: PathBuf,
    /// `<folder>/ST Test.clap`
    pub clap: PathBuf,
    /// `<folder>/ST Test.vst3`
    pub vst3: PathBuf,
}

fn library(name: &str) -> String {
    format!("lib{name}.{}", if cfg!(target_os = "macos") { "dylib" } else { "so" })
}

/// The directory cargo builds into for this test run: `<target>/<profile>`.
pub fn build_directory() -> PathBuf {
    let executable = std::env::current_exe().expect("the test executable");
    let folder = executable.parent().expect("a folder");
    if folder.file_name().is_some_and(|name| name == "deps" || name == "examples") { folder.parent().expect("a profile folder").to_path_buf() } else { folder.to_path_buf() }
}

/// See the module documentation. Panics with instructions when the build fails.
pub fn artifacts() -> &'static Artifacts {
    static FOUND: OnceLock<Artifacts> = OnceLock::new();
    FOUND.get_or_init(|| {
        let directory = build_directory();
        let found = Artifacts {
            container: directory.join("supertracker-plugin-container"),
            clap_library: directory.join(library("stpc_clap_test_plugin")),
            vst3_library: directory.join(library("stpc_vst3_test_plugin")),
        };
        let complete = |found: &Artifacts| found.container.is_file() && found.clap_library.is_file() && found.vst3_library.is_file();
        if !complete(&found) {
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin-host/Cargo.toml");
            let target = directory.parent().expect("a target folder");
            let mut command = std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            command.arg("build").arg("--manifest-path").arg(&manifest).args(["-p", "stpc-container", "-p", "stpc-clap-test-plugin", "-p", "stpc-vst3-test-plugin"]);
            command.env("CARGO_TARGET_DIR", target);
            if directory.file_name().is_some_and(|name| name == "release") {
                command.arg("--release");
            }
            let status = command.status().expect("cargo runs");
            assert!(status.success() && complete(&found), "the plug-in container and its test plug-ins could not be built into {}: run `cargo build --manifest-path crates/plugin-host/Cargo.toml` with CARGO_TARGET_DIR set to {}", directory.display(), target.display());
        }
        found
    })
}

impl Artifacts {
    /// Copies the test plug-ins into `folder` as installed plug-ins: a CLAP file and a VST3
    /// bundle. Each copy is a plug-in file of its own.
    pub fn install(&self, folder: &Path) -> Installed {
        std::fs::create_dir_all(folder).expect("the plug-in folder");
        let clap = folder.join("ST Test.clap");
        let vst3 = folder.join("ST Test.vst3");
        if cfg!(target_os = "macos") {
            for (bundle, source, kind) in [(&clap, &self.clap_library, "clap"), (&vst3, &self.vst3_library, "vst3")] {
                let binaries = bundle.join("Contents").join("MacOS");
                std::fs::create_dir_all(&binaries).expect("a bundle");
                std::fs::copy(source, binaries.join("ST Test")).expect("copy the plug-in");
                let plist = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\n<key>CFBundleExecutable</key><string>ST Test</string>\n<key>CFBundleIdentifier</key><string>org.supertracker.tests.{kind}</string>\n<key>CFBundlePackageType</key><string>BNDL</string>\n</dict></plist>\n"
                );
                std::fs::write(bundle.join("Contents").join("Info.plist"), plist).expect("the bundle's property list");
            }
        } else {
            std::fs::copy(&self.clap_library, &clap).expect("copy the CLAP plug-in");
            let binaries = vst3.join("Contents").join(format!("{}-linux", std::env::consts::ARCH));
            std::fs::create_dir_all(&binaries).expect("a VST3 bundle");
            std::fs::copy(&self.vst3_library, binaries.join("ST Test.so")).expect("copy the VST3 plug-in");
        }
        Installed { folder: folder.to_path_buf(), clap, vst3 }
    }
}
