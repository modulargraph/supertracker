//! Every command the interface can send, with the crate that owns it. The table is the host's
//! command reference: `docs/HOST.md` is generated from it (`tests/host_doc.rs`), and a command
//! nobody has registered yet answers with an error built from its entry.

/// The crate a command belongs to (docs/ARCHITECTURE.md, "Workspace").
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

/// How far a command is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Registered and complete.
    Implemented,
    /// Registered; the note says what is still missing and who provides it.
    Partial,
    /// Not registered: requests are answered with [`missing_message`].
    NotYet,
}

/// One command of the bridge.
#[derive(Debug, Clone, Copy)]
pub struct CommandInfo {
    pub name: &'static str,
    /// The crate that implements it (its handler is registered in `st-host` and delegates there).
    pub area: Area,
    pub state: State,
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
        Some(command) => format!("`{name}` is not implemented yet (it belongs to {}).", command.area.crate_name()),
        None => format!("`{name}` is not a command this host knows."),
    }
}

/// The areas in the order the status document lists them.
pub const AREAS: &[Area] = &[Area::Host, Area::App, Area::Project, Area::Audio, Area::Plugins, Area::Library, Area::Midi, Area::Mk3, Area::Agent];

/// The command tables of `docs/HOST.md`: a summary by crate, then one table per crate.
pub fn status_tables() -> String {
    let cell = |text: &str| text.replace('|', "\\|");
    let count = |area: Area, state: State| COMMANDS.iter().filter(|command| command.area == area && command.state == state).count();
    let mut text = String::from("| Crate | Commands | Implemented | Partial | Not yet |\n|---|---:|---:|---:|---:|\n");
    for &area in AREAS {
        let (done, partial, open) = (count(area, State::Implemented), count(area, State::Partial), count(area, State::NotYet));
        text.push_str(&format!("| `{}` | {} | {done} | {partial} | {open} |\n", area.crate_name(), done + partial + open));
    }
    let total = |state: State| COMMANDS.iter().filter(|command| command.state == state).count();
    text.push_str(&format!(
        "| **all** | **{}** | **{}** | **{}** | **{}** |\n",
        COMMANDS.len(),
        total(State::Implemented),
        total(State::Partial),
        total(State::NotYet)
    ));
    for &area in AREAS {
        text.push_str(&format!("\n### `{}`\n\n| Command | Payload | Result | State | Notes |\n|---|---|---|---|---|\n", area.crate_name()));
        for command in COMMANDS.iter().filter(|command| command.area == area) {
            let state = match command.state {
                State::Implemented => "implemented",
                State::Partial => "partial",
                State::NotYet => "not yet",
            };
            text.push_str(&format!(
                "| `{}` | `{}` | {} | {state} | {} |\n",
                command.name,
                cell(command.payload),
                cell(command.result),
                cell(command.note)
            ));
        }
    }
    text
}
