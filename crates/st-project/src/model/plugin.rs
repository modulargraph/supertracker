//! Plug-in descriptors (`project.audioPlugins`): what is loaded where, and its saved state.

use crate::json::{model, string_enum};
use serde_json::Value;

/// Insert slots per mixer channel (the interface's `INSERT_SLOTS`).
pub const INSERT_SLOTS: i64 = 16;

/// The prefix of a built-in effect's component: `stfx:<effect id>`, as the plug-in catalogue
/// lists it. It is the only spelling.
pub const BUILTIN_EFFECT_PREFIX: &str = "stfx:";

/// The id of the built-in effect `component` names, if it names one.
pub fn builtin_effect_id(component: &str) -> Option<&str> {
    component.strip_prefix(BUILTIN_EFFECT_PREFIX)
}

/// What a component can name, as `(component prefix, descriptor format)`: the built-in effects,
/// the plug-in formats the container hosts, and VST2, which nothing here loads: a Maschine group
/// import keeps such a plug-in's identity and state in a descriptor that is `missing`.
pub const COMPONENT_FORMATS: [(&str, &str); 5] = [("stfx", "Builtin"), ("clap", "CLAP"), ("vst3", "VST3"), ("au", "AU"), ("vst2", "VST2")];

/// What a descriptor's `component` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component<'a> {
    /// The component's prefix: `stfx`, `clap`, `vst3`, `au` or `vst2` (for a plug-in the
    /// container hosts, its backend name).
    pub prefix: &'static str,
    /// The descriptor's `format` that goes with the prefix: `Builtin`, `CLAP`, `VST3`, `AU` or
    /// `VST2`.
    pub format: &'static str,
    /// The id in that format: the effect id, the CLAP plug-in id, the VST3 class id (32
    /// upper-case hex digits), the Audio Unit's `type:subtype:manufacturer` or the VST2 unique
    /// id (8 upper-case hex digits).
    pub id: &'a str,
}

/// Reads a component, which is always `<prefix>:<id>` with one of the prefixes of
/// [`COMPONENT_FORMATS`] as written there. `None` for anything else: an id without a prefix,
/// another prefix or another case of one, an empty id, a VST3 class id or VST2 unique id that is
/// not 32 or 8 upper-case hex digits.
pub fn parse_component(component: &str) -> Option<Component<'_>> {
    let (prefix, id) = component.split_once(':')?;
    let (prefix, format) = COMPONENT_FORMATS.iter().copied().find(|(known, _)| *known == prefix)?;
    let hex = |digits: usize| id.len() == digits && id.bytes().all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte));
    let well_formed = match prefix {
        "vst3" => hex(32),
        "vst2" => hex(8),
        _ => !id.is_empty() && id.trim() == id && !id.chars().any(char::is_control),
    };
    well_formed.then_some(Component { prefix, format, id })
}

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
        /// What is loaded: `<prefix>:<id>`, as the plug-in catalogue lists it (`stfx:<effect>`,
        /// `clap:<plug-in id>`, `vst3:<class id>`, `au:<type>:<subtype>:<manufacturer>`), or the
        /// `vst2:<unique id>` of a plug-in an import kept; see [`parse_component`].
        "component" => component: String,
        /// The format the component's prefix names (`Builtin`, `CLAP`, `VST3`, `AU`, `VST2`;
        /// [`COMPONENT_FORMATS`]), for display.
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
        self.component.as_deref().is_some_and(|component| builtin_effect_id(component).is_some())
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
