//! Format `"vst3"`: hosts VST3 plug-ins through the COM interfaces of the VST3 SDK (the
//! `vst3` crate's bindings; no C++).
//!
//! Instance configuration on the host side: `format = "vst3"`, `plugin_id` = the class id
//! of the plug-in's component as 32 hex digits (what [`scan`] returns), `options` = path
//! of the `.vst3` bundle.
//!
//! | File | Contents |
//! |---|---|
//! | [`module`] | Loading a module per platform, the class factory, class ids |
//! | [`host`] | The host's COM objects: `IHostApplication`, messages, streams, `IComponentHandler` |
//! | [`process`] | Parameter queues, event lists, process context, the real-time `Processor` |
//! | [`plugin`] | The instance on the main thread: life cycle, buses, parameters, state, presets |
//! | [`state`] | The container format for component + controller state |
//! | [`editor`] | `IPlugView` in a container window (macOS) |
//!
//! Verified by running on Linux against `stpc-vst3-test-plugin` (`tests/vst3.rs`). The
//! macOS module loading and the editor are type-checked only; the Windows module loading
//! has never been compiled. See `docs/PLUGIN_CONTAINER.md` for what is and is not
//! implemented.

pub mod editor;
pub mod host;
pub mod module;
pub mod plugin;
pub mod process;
pub mod state;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use stpc_runtime::{
    CreateRequest, HostHandle, Plugin, PluginBackend, PluginDescription, PluginError, SCAN_AUDIO_INPUT,
    SCAN_AUDIO_OUTPUT, SCAN_EFFECT, SCAN_INSTRUMENT, SCAN_SIDECHAIN, SCAN_WANTS_MIDI,
};
use vst3::Steinberg::FUnknown;
use vst3::ComWrapper;

use host::HostApplication;
use module::{cid_to_hex, hex_to_cid, Module};
use plugin::Vst3Plugin;

pub struct Vst3Backend {
    /// The context offered to factories (`IPluginFactory3::setHostContext`).
    host_context: ComWrapper<HostApplication>,
    /// Loaded modules by bundle path. A module is shared by its instances and unloaded
    /// with the last of them (exit function included): plug-ins keep process-wide state
    /// per module, and entering a module twice is not something they expect.
    modules: RefCell<HashMap<PathBuf, Weak<Module>>>,
}

impl Default for Vst3Backend {
    fn default() -> Vst3Backend {
        Vst3Backend::new()
    }
}

impl Vst3Backend {
    pub fn new() -> Vst3Backend {
        Vst3Backend { host_context: ComWrapper::new(HostApplication), modules: RefCell::new(HashMap::new()) }
    }

    fn module(&self, path: &Path) -> Result<Rc<Module>, PluginError> {
        let mut modules = self.modules.borrow_mut();
        modules.retain(|_, module| module.strong_count() > 0);
        if let Some(module) = modules.get(path).and_then(Weak::upgrade) {
            return Ok(module);
        }
        let context = self.host_context.as_com_ref::<FUnknown>().unwrap().as_ptr();
        // SAFETY: loading a plug-in runs its code; that is what this process is for. The
        // host context lives as long as the backend, which outlives every module.
        let module = Rc::new(unsafe { Module::load(path, context) }.map_err(PluginError::Failed)?);
        modules.insert(path.to_path_buf(), Rc::downgrade(&module));
        Ok(module)
    }
}

impl PluginBackend for Vst3Backend {
    fn format(&self) -> &str {
        "vst3"
    }

    fn create(&self, request: &CreateRequest<'_>, handle: HostHandle) -> Result<Box<dyn Plugin>, PluginError> {
        if request.options.is_empty() {
            return Err(PluginError::failed("the VST3 backend needs the path of the .vst3 bundle in `options`"));
        }
        let Some(cid) = hex_to_cid(request.plugin_id) else {
            return Err(PluginError::failed(format!(
                "\"{}\" is not a VST3 class id (32 hex digits)",
                request.plugin_id
            )));
        };
        let module = self.module(Path::new(request.options))?;
        let class = module.classes().into_iter().find(|class| class.cid == cid && class.is_audio_module());
        let Some(class) = class else {
            return Err(PluginError::failed(format!(
                "{} has no plug-in with the class id {}",
                request.options,
                cid_to_hex(&cid)
            )));
        };
        // SAFETY: main thread (the runtime's contract for `create`).
        let plugin = unsafe { Vst3Plugin::create(module, class, request, handle) }?;
        Ok(Box::new(plugin))
    }
}

/// Lists the plug-ins of one `.vst3` bundle from its class factory, without creating any
/// of them, and unloads the module again. For [`stpc_runtime::Runtime::register_scanner`].
pub fn scan(path: &str) -> Result<Vec<PluginDescription>, PluginError> {
    if path.is_empty() {
        return Err(PluginError::failed("VST3 plug-ins are scanned per bundle: no path given"));
    }
    let host_context = ComWrapper::new(HostApplication);
    let context = host_context.as_com_ref::<FUnknown>().unwrap().as_ptr();
    // SAFETY: as in `Vst3Backend::module`; the context outlives the module (dropped first).
    let module = unsafe { Module::load(Path::new(path), context) }.map_err(PluginError::Failed)?;
    let url = module.factory_info().url;
    let bundle_version = module.bundle_version();
    let plugins = module
        .classes()
        .into_iter()
        .filter(|class| class.is_audio_module())
        .map(|class| {
            let subcategories: Vec<String> =
                class.sub_categories.split('|').filter(|part| !part.is_empty()).map(str::to_string).collect();
            let has = |name: &str| subcategories.iter().any(|part| part == name);
            // Buses are only known to an instance; the categories say this much.
            let mut flags = SCAN_AUDIO_OUTPUT;
            if has("Instrument") {
                flags |= SCAN_INSTRUMENT | SCAN_WANTS_MIDI;
            }
            if has("Fx") || !has("Instrument") {
                flags |= SCAN_EFFECT | SCAN_AUDIO_INPUT;
            }
            if has("Sidechain") {
                flags |= SCAN_SIDECHAIN;
            }
            PluginDescription {
                format: "vst3".into(),
                id: cid_to_hex(&class.cid),
                name: class.name,
                vendor: class.vendor,
                // The class's own version; the bundle's where a class states none.
                version: if class.version.is_empty() { bundle_version.clone() } else { class.version },
                category: class.sub_categories,
                subcategories,
                flags,
                path: path.to_string(),
                sdk_version: class.sdk_version,
                url: url.clone(),
            }
        })
        .collect();
    drop(module);
    Ok(plugins)
}
