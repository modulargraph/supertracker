//! Plug-in descriptors (`project.audioPlugins`): what is loaded where, and its saved state.

use crate::json::{model, string_enum};
use serde_json::Value;

/// Insert slots per mixer channel (the interface's `INSERT_SLOTS`).
pub const INSERT_SLOTS: i64 = 16;

string_enum! {
    /// Which slot kind a plug-in occupies on its channel.
    pub enum PluginKind {
        /// The track's instrument plug-in (slot 0).
        Instrument = "instrument",
        /// An insert effect (slots 0..16): a built-in effect or a plug-in.
        Effect = "effect",
    }
}

/// Who owns a plug-in descriptor or an automation lane: a mixer channel, or an instrument
/// (whose insert chain runs on every track that plays it).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Owner {
    /// `track:<routingID>`
    Track(String),
    /// `bus:<id>`: a group or return bus.
    Bus(String),
    /// `master`
    Master,
    /// `instrument:<index>`: the instrument's insert chain.
    Instrument(usize),
}

impl Owner {
    /// Parses an owner string. `None` when it is none of the four forms.
    pub fn parse(text: &str) -> Option<Owner> {
        if text == "master" {
            return Some(Owner::Master);
        }
        if let Some(id) = text.strip_prefix("track:") {
            return (!id.is_empty()).then(|| Owner::Track(id.to_string()));
        }
        if let Some(id) = text.strip_prefix("bus:") {
            return (!id.is_empty()).then(|| Owner::Bus(id.to_string()));
        }
        let digits = text.strip_prefix("instrument:")?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok().filter(|index| *index < 4096).map(Owner::Instrument)
    }

    /// The owner string.
    pub fn text(&self) -> String {
        match self {
            Owner::Track(id) => format!("track:{id}"),
            Owner::Bus(id) => format!("bus:{id}"),
            Owner::Master => "master".to_string(),
            Owner::Instrument(index) => format!("instrument:{index}"),
        }
    }
}

model! {
    /// One plug-in (or built-in effect) of the project.
    ///
    /// The host writes these when it loads a plug-in and captures state; the interface keeps
    /// them in `project.audioPlugins` and moves them with their owner. Fields the host adds for
    /// display only (`parameters`, `presets`, `currentPreset`, `hostStatus`,
    /// `migration`, `editorOpen`, `windowFrame`) stay in `extra`.
    pub struct PluginDescriptor {
        /// The owner: `track:<routingID>`, `bus:<id>`, `master` or `instrument:<index>`; see [`Owner`].
        /// Required: it is the only way a descriptor names its channel.
        "owner" => owner: String,
        /// Instrument or insert effect. Absent reads as an effect.
        "kind" => kind: PluginKind = PluginKind::Effect,
        /// Insert slot 0..16 (0 for an instrument).
        "slot" => slot: i64 = 0,
        /// Stable identity of this instance: automation, step locks, sidechains and saved pages
        /// refer to it.
        "instanceID" => instance_id: String,
        /// Component identity: `builtin:<effect>` or the format's identifier of the plug-in.
        "component" => component: String,
        /// Plug-in format as the host names it (for example `VST3`, `CLAP`, `AU`).
        "format" => format: String,
        /// The plug-in's bundle or file, where the format has one.
        "bundlePath" => bundle_path: String,
        /// Display name.
        "name" => name: String,
        /// Manufacturer.
        "manufacturer" => manufacturer: String,
        /// Bypassed.
        "bypass" => bypass: bool = false,
        /// The plug-in's saved state, base64. Opaque to this crate; never treated as media.
        "state" => state: String,
        /// Why the last state capture failed, if it did.
        "stateError" => state_error: String,
        /// The plug-in could not be found when the project was loaded.
        "missing" => missing: bool = false,
        /// Reported latency in seconds.
        "latency" => latency: f64 = 0.0,
        /// Reported tail in seconds.
        "tailTime" => tail_time: f64 = 0.0,
        /// The NKS preset the instance was loaded from (`{name, path, ...}`).
        "nks" => nks: Value,
    }
}

impl PluginDescriptor {
    /// The parsed owner, when `owner` is present and well formed.
    pub fn parsed_owner(&self) -> Option<Owner> {
        self.owner.as_deref().and_then(Owner::parse)
    }

    /// Whether the component is a built-in effect.
    pub fn is_builtin(&self) -> bool {
        self.component.as_deref().is_some_and(|component| component.starts_with("builtin:"))
    }
}

model! {
    /// One page of eight parameters of a plug-in's saved pages.
    pub struct ParameterPage {
        /// Page name.
        "name" => name: String,
        /// Eight parameter addresses (decimal strings), `null` for an empty slot.
        "addresses" => addresses: Vec<Option<String>>,
    }
}

model! {
    /// The parameter pages the user arranged for one plug-in instance
    /// (`project.pluginPages[instanceID]`, `plugin-pages.js`).
    pub struct PluginPages {
        /// The plug-in's name when the pages were edited.
        "name" => name: String,
        /// The pages.
        "pages" => pages: Vec<ParameterPage>,
    }
}
