//! Every command the interface can send, with the crate that owns it. The table is the work list of
//! the port: `docs/RUST_PORT_STATUS.md` is generated from it (`tests/port_status.rs`), and a command
//! nobody has registered yet answers with an error built from its entry.

/// The crate a command belongs to (docs/RUST_ARCHITECTURE.md, "Workspace").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Area {
    Host,
    App,
    Project,
    Audio,
    Plugins,
    Library,
    Midi,
    Mk3,
    Agent,
}

impl Area {
    /// The crate's name.
    pub fn crate_name(self) -> &'static str {
        match self {
            Area::Host => "st-host",
            Area::App => "st-app",
            Area::Project => "st-project",
            Area::Audio => "st-audio",
            Area::Plugins => "st-plugin-*",
            Area::Library => "st-library",
            Area::Midi => "st-midi",
            Area::Mk3 => "st-mk3",
            Area::Agent => "st-agent",
        }
    }
}

/// One command of the bridge.
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    /// The crate that implements it (its handler is registered in `st-host` and delegates there).
    pub area: Area,
    /// The payload, in one line.
    pub payload: &'static str,
    /// The result, in one line.
    pub result: &'static str,
    /// Anything a porter must know that the shapes do not say.
    pub note: &'static str,
}

include!("catalog_table.rs");

/// The entry for `name`.
pub fn find(name: &str) -> Option<&'static CommandInfo> {
    COMMANDS.iter().find(|command| command.name == name)
}

/// The error a request for an unregistered command gets. It always names the command.
pub fn missing_message(name: &str) -> String {
    match find(name) {
        Some(command) => format!("`{name}` is not implemented in the Rust host yet (it belongs to {}).", command.area.crate_name()),
        None => format!("`{name}` is not a command this host knows."),
    }
}
