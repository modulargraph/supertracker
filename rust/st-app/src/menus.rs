//! The menu bar. Items carry the name of an interface command: choosing one sends the event
//! `command` with `{name}` to the page, which runs it exactly as its own keyboard handler and
//! command palette do. Standard items (Cut, Hide, Quit, …) are the platform's own.
//!
//! Shortcuts are listed only for commands whose key the interface's keyboard handler owns globally,
//! so that a key the page handles and a key the menu handles mean the same thing wherever focus is.
//! Context-dependent keys (Space, Option-letters, …) are shown in the help sheet and the command
//! palette instead.

/// One line of a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// Sends `command` to the interface. The shortcut uses muda's notation (`CmdOrCtrl+Shift+S`).
    Command { title: &'static str, command: &'static str, shortcut: Option<&'static str> },
    Standard(Standard),
    Separator,
}

/// Items the platform implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    About,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    CloseWindow,
    Cut,
    Copy,
    Paste,
    Minimize,
    Zoom,
    BringAllToFront,
    Fullscreen,
}

/// What the platform should know about a menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The application menu (macOS only; elsewhere its commands appear in File and Help).
    Application,
    Window,
    Help,
    Plain,
}

pub struct MenuSpec {
    pub title: &'static str,
    pub role: Role,
    pub items: &'static [Item],
}

const fn command(title: &'static str, command: &'static str) -> Item {
    Item::Command { title, command, shortcut: None }
}

const fn keyed(title: &'static str, command: &'static str, shortcut: &'static str) -> Item {
    Item::Command { title, command, shortcut: Some(shortcut) }
}

use Item::Separator;

const REVEAL_SAMPLE: &str = if cfg!(target_os = "macos") { "Show Sample in Finder" } else { "Show Sample in File Manager" };

/// The menu bar, in order.
pub const MENUS: &[MenuSpec] = &[
    MenuSpec {
        title: "SuperTracker",
        role: Role::Application,
        items: &[
            Item::Standard(Standard::About),
            Separator,
            keyed("Settings…", "settings", "CmdOrCtrl+,"),
            Separator,
            Item::Standard(Standard::Hide),
            Item::Standard(Standard::HideOthers),
            Item::Standard(Standard::ShowAll),
            Separator,
            Item::Standard(Standard::Quit),
        ],
    },
    MenuSpec {
        title: "File",
        role: Role::Plain,
        items: &[
            keyed("New Project", "new", "CmdOrCtrl+N"),
            keyed("Open Project…", "open", "CmdOrCtrl+O"),
            Separator,
            keyed("Save Project", "save", "CmdOrCtrl+S"),
            keyed("Save Project As…", "saveAs", "CmdOrCtrl+Shift+S"),
            command("Rename Session…", "renameProject"),
            Separator,
            command("Import MIDI File…", "importMIDI"),
            command("Import Audio Clip…", "importAudio"),
            command("Load Sample…", "loadSample"),
            Separator,
            keyed("Export Audio…", "export", "CmdOrCtrl+Shift+E"),
            command("Export MIDI File…", "exportMIDI"),
            Separator,
            keyed("Record Audio Input…", "recordInput", "CmdOrCtrl+Shift+R"),
            command("Relink Missing Media…", "relinkMedia"),
            command("Review Interrupted Plug-ins…", "pluginRecovery"),
            Separator,
            Item::Standard(Standard::CloseWindow),
        ],
    },
    MenuSpec {
        title: "Edit",
        role: Role::Plain,
        items: &[
            keyed("Undo", "undo", "CmdOrCtrl+Z"),
            keyed("Redo", "redo", "CmdOrCtrl+Shift+Z"),
            Separator,
            Item::Standard(Standard::Cut),
            Item::Standard(Standard::Copy),
            Item::Standard(Standard::Paste),
            keyed("Select All", "selectAll", "CmdOrCtrl+A"),
            Separator,
            command("Fill Selected Phrase Down", "fill"),
            command("Clear Step Effects", "clearEffects"),
        ],
    },
    MenuSpec {
        title: "View",
        role: Role::Plain,
        items: &[
            keyed("Pattern", "view:pattern", "CmdOrCtrl+1"),
            keyed("Instrument", "view:instrument", "CmdOrCtrl+2"),
            keyed("Browser", "view:browser", "CmdOrCtrl+3"),
            keyed("Mixer", "view:mixer", "CmdOrCtrl+4"),
            keyed("Automation", "view:automation", "CmdOrCtrl+5"),
            keyed("Song", "view:song", "CmdOrCtrl+6"),
            keyed("Recording", "view:recording", "CmdOrCtrl+7"),
            Separator,
            keyed("Next Section", "nextPane", "F6"),
            keyed("Previous Section", "previousPane", "Shift+F6"),
            Separator,
            keyed("Command Palette…", "palette", "CmdOrCtrl+K"),
            command("Find Instrument Control…", "findControl"),
            keyed("Browse NKS Instruments", "browseNKS", "CmdOrCtrl+Shift+B"),
            command("Browse NKS Effects", "browseNKSEffects"),
            command("Browse Plug-ins", "browsePlugins"),
            keyed("Send Effects…", "sendEffects", "CmdOrCtrl+Shift+X"),
            Separator,
            Item::Standard(Standard::Fullscreen),
        ],
    },
    MenuSpec {
        title: "Transport",
        role: Role::Plain,
        items: &[
            command("Play / Stop", "play"),
            command("Stop and Silence", "stop"),
            keyed("Arm Note Recording", "record", "CmdOrCtrl+R"),
            keyed("Computer Keyboard Piano", "keyboard", "CmdOrCtrl+P"),
            keyed("Metronome", "metronome", "CmdOrCtrl+Shift+M"),
            Separator,
            command("Song Playback", "modeSong"),
            command("Pattern Playback", "modePattern"),
            command("Loop Selected Rows", "loopSelection"),
            command("Follow Playback", "follow"),
            Separator,
            command("Set Tempo…", "setTempo"),
            Separator,
            command("Automation Write On / Off", "automationWriteToggle"),
            command("Cycle Automation Write Mode", "automationWriteCycle"),
            command("Disarm All Automation Parameters", "automationClearArms"),
        ],
    },
    MenuSpec {
        title: "Pattern",
        role: Role::Plain,
        items: &[
            keyed("New Pattern", "addPattern", "CmdOrCtrl+Shift+N"),
            command("Duplicate Pattern", "duplicatePattern"),
            command("Rename Pattern…", "renamePattern"),
            command("Set Pattern Length…", "patternLength"),
            command("Remove Pattern", "removePattern"),
            Separator,
            keyed("Previous Pattern", "previousPattern", "CmdOrCtrl+["),
            keyed("Next Pattern", "nextPattern", "CmdOrCtrl+]"),
            Separator,
            command("Insert Row", "insertRow"),
            command("Delete Row", "deleteRow"),
            Separator,
            command("Choose Step Effect…", "stepEffect"),
            command("Automate Parameter…", "automateParameter"),
            Separator,
            command("Interpolate Velocity / FX", "interpolate"),
            command("Humanize Velocity", "humanize"),
            command("Reverse Steps", "reverseSteps"),
            command("Stretch ×2", "stretch"),
            command("Compress ½", "compress"),
            command("Double Pattern Length", "doublePattern"),
            command("Halve Pattern Length", "halvePattern"),
            Separator,
            command("Octave Up", "octaveUp"),
            command("Octave Down", "octaveDown"),
        ],
    },
    MenuSpec {
        title: "Track",
        role: Role::Plain,
        items: &[
            keyed("Add Track", "addTrack", "CmdOrCtrl+T"),
            keyed("Duplicate Track", "duplicateTrack", "CmdOrCtrl+Shift+T"),
            command("Rename Track…", "renameTrack"),
            command("Delete Track…", "deleteTracks"),
            Separator,
            command("Move Track Left", "moveTrackLeft"),
            command("Move Track Right", "moveTrackRight"),
            command("Group Selected Tracks", "groupTracks"),
            command("Take Track Out of Group", "ungroupTrack"),
            command("Hide / Show Empty Tracks", "hideEmptyTracks"),
            Separator,
            command("Mute / Unmute", "muteTrack"),
            command("Solo / Unsolo", "soloTrack"),
            command("Unmute All and Clear Solo", "clearMuteSolo"),
            Separator,
            command("Freeze Track…", "freezeTrack"),
            command("Unfreeze Track…", "unfreezeTrack"),
            command("Bounce Track to Audio…", "bounceTrack"),
            Separator,
            command("Route Mixer Channel…", "routing"),
            command("Add Insert Effect…", "addEffect"),
            command("New Group Bus…", "addGroup"),
            command("New Return Bus…", "addReturn"),
        ],
    },
    MenuSpec {
        title: "Instrument",
        role: Role::Plain,
        items: &[
            command("Add Instrument", "addInstrument"),
            command("Add Wavetable Synth", "addWavetable"),
            command("Rename Instrument…", "renameInstrument"),
            command("Assign to Selected Track", "assignInstrument"),
            command("Remove Instrument", "removeInstrument"),
            Separator,
            command("Previous Instrument", "previousInstrument"),
            command("Next Instrument", "nextInstrument"),
            Separator,
            command("Load Sample…", "loadSample"),
            command(REVEAL_SAMPLE, "revealSample"),
            command("Sample Editor", "sample:editor"),
            command("Detect Transients", "sample:detectTransients"),
            command("Slices to New Pattern", "sample:slicesToPattern"),
        ],
    },
    MenuSpec {
        title: "Window",
        role: Role::Window,
        items: &[Item::Standard(Standard::Minimize), Item::Standard(Standard::Zoom), Separator, Item::Standard(Standard::BringAllToFront)],
    },
    MenuSpec {
        title: "Help",
        role: Role::Help,
        items: &[keyed("Keyboard Shortcuts", "help", "CmdOrCtrl+/"), command("Command Palette…", "palette")],
    },
];

/// The prefix of the menu ids of interface commands; the rest of the id is `<menu>/<line>`.
pub const COMMAND_ID: &str = "command:";

/// The interface command behind a menu id built by the window.
pub fn command_for_id(id: &str) -> Option<&'static str> {
    let (menu, line) = id.strip_prefix(COMMAND_ID)?.split_once('/')?;
    match MENUS.get(menu.parse::<usize>().ok()?)?.items.get(line.parse::<usize>().ok()?)? {
        Item::Command { command, .. } => Some(command),
        _ => None,
    }
}

/// The menu id of line `line` of menu `menu`.
pub fn id_for(menu: usize, line: usize) -> String {
    format!("{COMMAND_ID}{menu}/{line}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn commands() -> Vec<(&'static str, &'static str, Option<&'static str>)> {
        MENUS
            .iter()
            .flat_map(|menu| menu.items.iter())
            .filter_map(|item| match item {
                Item::Command { title, command, shortcut } => Some((*title, *command, *shortcut)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_menu_bar_has_the_menus_of_the_application() {
        let titles: Vec<&str> = MENUS.iter().map(|menu| menu.title).collect();
        assert_eq!(titles, ["SuperTracker", "File", "Edit", "View", "Transport", "Pattern", "Track", "Instrument", "Window", "Help"]);
        assert_eq!(MENUS.iter().filter(|menu| menu.role == Role::Application).count(), 1);
        // As many command items as the menu bar of the Swift application had.
        assert_eq!(commands().len(), 100);
    }

    #[test]
    fn ids_lead_back_to_their_commands() {
        for (menu_index, menu) in MENUS.iter().enumerate() {
            for (line, item) in menu.items.iter().enumerate() {
                let expected = match item {
                    Item::Command { command, .. } => Some(*command),
                    _ => None,
                };
                assert_eq!(command_for_id(&id_for(menu_index, line)), expected);
            }
        }
        for unknown in ["command:99/0", "command:1/999", "command:x/y", "context:1/2", ""] {
            assert_eq!(command_for_id(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn shortcuts_are_unique_and_parse() {
        let mut seen = BTreeSet::new();
        for (title, _, shortcut) in commands() {
            let Some(shortcut) = shortcut else { continue };
            assert!(seen.insert(shortcut), "{shortcut} is used twice ({title})");
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            assert!(shortcut.parse::<muda::accelerator::Accelerator>().is_ok(), "{shortcut} ({title})");
        }
    }

    /// The interface must know every command a menu sends: its command catalogue names them.
    #[test]
    fn every_menu_command_exists_in_the_interface() {
        let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Resources/Web");
        let mut source = String::new();
        for entry in std::fs::read_dir(&web).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "js") {
                source.push_str(&std::fs::read_to_string(path).unwrap());
            }
        }
        let mut missing = BTreeMap::new();
        for (title, command, _) in commands() {
            let name = command.split(':').next().unwrap();
            let known = [format!("'{command}'"), format!("{name}:"), format!("'{name}'"), format!("{name}(")].iter().any(|needle| source.contains(needle));
            if !known {
                missing.insert(command, title);
            }
        }
        assert!(missing.is_empty(), "menu commands the interface does not mention: {missing:?}");
    }
}
