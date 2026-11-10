//! Plug-in scanning inside the container: one function per format that lists the
//! plug-ins of a plug-in file (or of the system registry) without instantiating them.
//! The host's `stpc_client::Scanner` starts a container per file for this and is
//! prepared to lose it: loading a plug-in library runs its code.
//!
//! * `"vst3"`: [`crate::vst3::scan`], the class factory of a `.vst3` bundle.
//! * `"clap"`: [`scan_clap`], the plug-in factory of a `.clap` file.
//! * `"au"` (macOS): [`scan_au`], the system's Audio Unit registry; loads nothing.

use std::ffi::{c_char, CStr, CString};
use std::path::Path;

use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::version::clap_version_is_compatible;
use stpc_runtime::{
    PluginDescription, PluginError, Runtime, SCAN_AUDIO_INPUT, SCAN_AUDIO_OUTPUT, SCAN_EDITOR_KNOWN, SCAN_EFFECT,
    SCAN_HAS_EDITOR, SCAN_INSTRUMENT, SCAN_NOTE_EFFECT, SCAN_PRODUCES_MIDI, SCAN_WANTS_MIDI,
};

use crate::au::ident::{ComponentInfo, ComponentKind};

/// Registers the scanners of every format this build hosts.
pub fn register(runtime: &mut Runtime) {
    runtime.register_scanner("clap", Box::new(scan_clap));
    runtime.register_scanner("vst3", Box::new(crate::vst3::scan));
    #[cfg(target_os = "macos")]
    runtime.register_scanner("au", Box::new(scan_au));
}

unsafe fn string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(ptr).to_string_lossy().into_owned()
    }
}

/// Lists the plug-ins of a `.clap` file from its factory's descriptors.
pub fn scan_clap(path: &str) -> Result<Vec<PluginDescription>, PluginError> {
    if path.is_empty() {
        return Err(PluginError::failed("CLAP plug-ins are scanned per file: no path given"));
    }
    let bundle = Path::new(path);
    // macOS: a .clap is a bundle; the code is in Contents/MacOS/<name>. (By reading.)
    let binary = if bundle.is_dir() {
        let stem = bundle.file_stem().ok_or_else(|| PluginError::failed("bad CLAP bundle path"))?;
        bundle.join("Contents").join("MacOS").join(stem)
    } else {
        bundle.to_path_buf()
    };
    let c_binary = CString::new(binary.to_string_lossy().as_bytes()).map_err(|_| PluginError::failed("bad path"))?;
    let c_path = CString::new(path).map_err(|_| PluginError::failed("bad path"))?;
    // SAFETY: loading a plug-in runs its initialisers; this process exists to take that
    // risk. The entry and factory are used as the CLAP headers prescribe; descriptor
    // strings are copied before the library is deinitialised.
    unsafe {
        let handle = libc::dlopen(c_binary.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if handle.is_null() {
            let reason = libc::dlerror();
            let reason =
                if reason.is_null() { "unknown error".into() } else { CStr::from_ptr(reason).to_string_lossy() };
            return Err(PluginError::failed(format!("cannot load {}: {reason}", binary.display())));
        }
        let entry = libc::dlsym(handle, c"clap_entry".as_ptr()) as *const clap_plugin_entry;
        if entry.is_null() {
            return Err(PluginError::failed(format!("{} is not a CLAP plug-in (no clap_entry)", binary.display())));
        }
        let version = (*entry).clap_version;
        if !clap_version_is_compatible(version) {
            return Err(PluginError::failed("incompatible CLAP version"));
        }
        let (Some(init), Some(get_factory)) = ((*entry).init, (*entry).get_factory) else {
            return Err(PluginError::failed("incomplete clap_entry"));
        };
        if !init(c_path.as_ptr()) {
            return Err(PluginError::failed("the CLAP library failed to initialise"));
        }
        let factory = get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
        let mut plugins = Vec::new();
        if let Some(factory) = factory.as_ref() {
            let count = factory.get_plugin_count.map_or(0, |count| count(factory)).min(16384);
            for index in 0..count {
                let Some(descriptor) = factory.get_plugin_descriptor.and_then(|get| get(factory, index).as_ref())
                else {
                    continue;
                };
                let mut features = Vec::new();
                let mut feature = descriptor.features;
                while !feature.is_null() && !(*feature).is_null() && features.len() < 256 {
                    features.push(string(*feature));
                    feature = feature.add(1);
                }
                let has = |name: &str| features.iter().any(|feature| feature == name);
                let mut flags = 0;
                if has("instrument") {
                    flags |= SCAN_INSTRUMENT | SCAN_WANTS_MIDI | SCAN_AUDIO_OUTPUT;
                }
                if has("audio-effect") || has("analyzer") {
                    flags |= SCAN_EFFECT | SCAN_AUDIO_INPUT | SCAN_AUDIO_OUTPUT;
                }
                if has("note-effect") {
                    flags |= SCAN_NOTE_EFFECT | SCAN_WANTS_MIDI | SCAN_PRODUCES_MIDI;
                }
                if has("note-detector") {
                    flags |= SCAN_AUDIO_INPUT | SCAN_PRODUCES_MIDI;
                }
                plugins.push(PluginDescription {
                    format: "clap".into(),
                    id: string(descriptor.id),
                    name: string(descriptor.name),
                    vendor: string(descriptor.vendor),
                    version: string(descriptor.version),
                    category: features.join("|"),
                    subcategories: features,
                    flags,
                    path: path.to_string(),
                    sdk_version: format!("{}.{}.{}", version.major, version.minor, version.revision),
                    url: string(descriptor.url),
                });
            }
        }
        if let Some(deinit) = (*entry).deinit {
            deinit();
        }
        if factory.is_null() {
            return Err(PluginError::failed("the CLAP library has no plug-in factory"));
        }
        Ok(plugins)
    }
}

/// One Audio Unit of the system's registry as a scan result.
pub fn describe_au(component: &ComponentInfo) -> PluginDescription {
    // Every Audio Unit can be shown: its own view, or the generic parameter editor.
    let mut flags = SCAN_EDITOR_KNOWN | SCAN_HAS_EDITOR;
    flags |= match component.kind {
        ComponentKind::Instrument => SCAN_INSTRUMENT | SCAN_AUDIO_OUTPUT,
        ComponentKind::Effect | ComponentKind::MusicEffect => SCAN_EFFECT | SCAN_AUDIO_INPUT | SCAN_AUDIO_OUTPUT,
        ComponentKind::MidiEffect => SCAN_NOTE_EFFECT,
        ComponentKind::Generator => SCAN_AUDIO_OUTPUT,
    };
    if component.has_midi_input {
        flags |= SCAN_WANTS_MIDI;
    }
    if component.has_midi_output {
        flags |= SCAN_PRODUCES_MIDI;
    }
    PluginDescription {
        format: "au".into(),
        id: component.plugin_id.clone(),
        name: component.name.clone(),
        vendor: component.manufacturer.clone(),
        version: component.version_string.clone(),
        category: component.type_name.clone(),
        subcategories: component.tags.clone(),
        flags,
        path: String::new(),
        sdk_version: if component.is_v3 { "AUv3".into() } else { "AUv2".into() },
        url: String::new(),
    }
}

/// Lists the installed Audio Units from the system's registry (`path` must be empty).
/// Nothing is loaded; it still runs in a container, where the AudioToolbox code lives.
pub fn scan_au(path: &str) -> Result<Vec<PluginDescription>, PluginError> {
    if !path.is_empty() {
        return Err(PluginError::failed("Audio Units are listed from the system registry, not from files"));
    }
    Ok(crate::au::list_components().iter().map(describe_au).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanners_refuse_requests_that_make_no_sense() {
        assert!(scan_au("/somewhere/Unit.component").is_err(), "Audio Units are not scanned from files");
        assert!(scan_clap("").is_err() && crate::vst3::scan("").is_err(), "file formats need a file");
        match scan_clap("/nonexistent/plugin.clap") {
            Err(PluginError::Failed(message)) => assert!(message.contains("cannot load"), "{message}"),
            other => panic!("{other:?}"),
        }
        // On systems without Audio Units the registry is simply empty.
        if !cfg!(target_os = "macos") {
            assert_eq!(scan_au("").unwrap(), []);
        }
    }
}
