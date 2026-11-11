//! Which plug-in a descriptor names.
//!
//! A descriptor's `component` is `<format>:<id>`: `vst3:<class id, 32 upper-case hex digits>`,
//! `clap:<plug-in id>` or `au:<type>:<subtype>:<manufacturer>` (the container's Audio Unit id),
//! in the one spelling `st_project::model::plugin::parse_component` reads. `bundlePath` is the
//! plug-in's file (VST3 bundle, CLAP file); Audio Units have none.

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

/// `(format, id)` of a plug-in's component: the container's backend name and the plug-in's id
/// in that format. `None` for a built-in effect's component and for text that is no component.
pub fn parse_component(component: &str) -> Option<(String, String)> {
    let parsed = st_project::model::plugin::parse_component(component)?;
    FORMATS.contains(&parsed.prefix).then(|| (parsed.prefix.to_string(), parsed.id.to_string()))
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
        assert_eq!(parse_component("vst3:535456334761696E50726F6300000001"), Some(("vst3".into(), "535456334761696E50726F6300000001".into())));
        assert_eq!(parse_component("clap:org.example.gain"), Some(("clap".into(), "org.example.gain".into())));
        assert_eq!(parse_component("au:aufx:dely:appl"), Some(("au".into(), "aufx:dely:appl".into())));
        // One spelling: the prefix as written, a VST3 class id in upper case, and never an id alone.
        for other in ["org.example.gain", "CLAP:org.example.gain", "vst3:535456334761696e50726f6300000001", "vst3:ABCD", "clap:", "stfx:eq"] {
            assert_eq!(parse_component(other), None, "{other}");
        }
        // A component the project format knows and no container hosts.
        assert_eq!(parse_component("vst2:4E694D61"), None);
        let one = Identity { format: "clap".into(), id: "a".into(), path: "/x".into(), ..Identity::default() };
        let other = Identity { path: "/y".into(), ..one.clone() };
        assert_ne!(one.key(), other.key());
        assert_eq!((one.component().as_str(), one.format_label().as_str(), one.display_name()), ("clap:a", "CLAP", "a"));
    }
}
