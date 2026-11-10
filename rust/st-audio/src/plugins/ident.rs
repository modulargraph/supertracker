//! Which plug-in a descriptor names.
//!
//! A descriptor's `component` is `<format>:<id>`: `vst3:<class id, 32 hex digits>`,
//! `clap:<plug-in id>` or `au:<type>:<subtype>:<manufacturer>` (the container's Audio Unit id).
//! `bundlePath` is the plug-in's file (VST3 bundle, CLAP file); Audio Units have none. A
//! component without a known prefix is read with the descriptor's `format` field.

/// The identity of an installed plug-in, as the container needs it to create an instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Identity {
    /// The container's backend name: `vst3`, `clap` or `au`.
    pub format: String,
    /// The plug-in's id in that format.
    pub id: String,
    /// The plug-in file or bundle; empty for Audio Units.
    pub path: String,
    pub name: String,
    pub manufacturer: String,
}

/// The formats the container hosts.
pub const FORMATS: [&str; 3] = ["vst3", "clap", "au"];

/// `(format, id)` of a component string; `format_hint` is the descriptor's `format` field.
pub fn parse_component(component: &str, format_hint: Option<&str>) -> Option<(String, String)> {
    let component = component.trim();
    if let Some((prefix, id)) = component.split_once(':') {
        let prefix = prefix.to_ascii_lowercase();
        if FORMATS.contains(&prefix.as_str()) && !id.is_empty() {
            return Some((prefix.clone(), if prefix == "vst3" { id.to_ascii_uppercase() } else { id.to_string() }));
        }
    }
    let format = format_hint?.to_ascii_lowercase();
    (FORMATS.contains(&format.as_str()) && !component.is_empty()).then(|| (format, component.to_string()))
}

impl Identity {
    /// The descriptor's `component`.
    pub fn component(&self) -> String {
        format!("{}:{}", self.format, self.id)
    }

    /// The descriptor's `format`: `VST3`, `CLAP` or `AU`.
    pub fn format_label(&self) -> String {
        self.format.to_ascii_uppercase()
    }

    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }

    /// A stable key of the installation (format, id and file), for the recovery store.
    pub fn key(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in self.format.bytes().chain([0]).chain(self.id.bytes()).chain([0]).chain(self.path.bytes()) {
            hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_name_a_format_and_an_id() {
        assert_eq!(parse_component("vst3:535456334761696e50726f6300000001", None), Some(("vst3".into(), "535456334761696E50726F6300000001".into())));
        assert_eq!(parse_component("CLAP:org.example.gain", None), Some(("clap".into(), "org.example.gain".into())));
        assert_eq!(parse_component("au:aufx:dely:appl", None), Some(("au".into(), "aufx:dely:appl".into())));
        assert_eq!(parse_component("org.example.gain", Some("CLAP")), Some(("clap".into(), "org.example.gain".into())));
        assert_eq!(parse_component("org.example.gain", None), None);
        assert_eq!(parse_component("stfx:eq", Some("Builtin")), None);
        let one = Identity { format: "clap".into(), id: "a".into(), path: "/x".into(), ..Identity::default() };
        let other = Identity { path: "/y".into(), ..one.clone() };
        assert_ne!(one.key(), other.key());
        assert_eq!((one.component().as_str(), one.format_label().as_str(), one.display_name()), ("clap:a", "CLAP", "a"));
    }
}
