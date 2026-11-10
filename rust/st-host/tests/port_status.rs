//! The command catalogue is the port's work list. These tests keep it true: what it calls
//! implemented is what the host registers, and docs/RUST_PORT_STATUS.md shows exactly its tables.
//!
//! After changing `src/catalog_table.rs`, rewrite the document's tables with
//! `UPDATE_PORT_STATUS=1 cargo test -p st-host --test port_status`.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use st_host::catalog::{self, Area, State, COMMANDS};
use st_host::testing::TempDir;
use st_host::{install, Bridge, HostOptions, NoDialogs};

const BEGIN: &str = "<!-- commands:begin (generated from rust/st-host/src/catalog_table.rs; see rust/st-host/tests/port_status.rs) -->";
const END: &str = "<!-- commands:end -->";

fn registered_by_st_host() -> BTreeSet<String> {
    let folder = TempDir::new("port-status");
    let mut bridge = Bridge::builder();
    let mut options = HostOptions::new(folder.path().to_path_buf(), Arc::new(NoDialogs));
    options.autosave_every = None;
    install(&mut bridge, options).expect("install the host");
    bridge.commands().names().into_iter().map(str::to_string).collect()
}

#[test]
fn the_catalogue_is_sorted_and_names_each_command_once() {
    let names: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(names, sorted);
    for command in COMMANDS {
        assert!(!command.payload.is_empty() && !command.result.is_empty(), "{} needs its shapes", command.name);
        assert!(command.state != State::Partial || !command.note.is_empty(), "{} is partial: say what is missing", command.name);
    }
}

#[test]
fn the_catalogue_says_implemented_exactly_for_what_is_registered() {
    let registered = registered_by_st_host();
    // The window's commands are registered by st-app (rust/st-app/src/window.rs).
    let catalogued: BTreeSet<String> =
        COMMANDS.iter().filter(|command| command.state != State::NotYet && command.area != Area::App).map(|command| command.name.to_string()).collect();
    assert_eq!(registered, catalogued, "left: registered by st_host::install; right: implemented or partial in the catalogue");
    for name in &registered {
        assert!(catalog::find(name).is_some(), "`{name}` is registered but missing from the catalogue");
    }
}

#[test]
fn unregistered_commands_are_refused_with_their_owner() {
    assert_eq!(catalog::missing_message("transport.play"), "`transport.play` is not implemented in the Rust host yet (it belongs to st-audio).");
    assert_eq!(catalog::missing_message("library.search"), "`library.search` is not implemented in the Rust host yet (it belongs to st-library).");
    assert_eq!(catalog::missing_message("transport.moonwalk"), "`transport.moonwalk` is not a command this host knows.");
}

#[test]
fn the_status_document_shows_the_catalogue() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/RUST_PORT_STATUS.md");
    let document = std::fs::read_to_string(&path).expect("docs/RUST_PORT_STATUS.md exists");
    let (before, rest) = document.split_once(BEGIN).expect("the document has the commands:begin marker");
    let (shown, after) = rest.split_once(END).expect("the document has the commands:end marker");
    let tables = format!("\n{}\n", catalog::status_tables());
    if std::env::var_os("UPDATE_PORT_STATUS").is_some() {
        std::fs::write(&path, format!("{before}{BEGIN}{tables}{END}{after}")).expect("rewrite the document");
        return;
    }
    assert!(shown == tables, "docs/RUST_PORT_STATUS.md is out of date: run UPDATE_PORT_STATUS=1 cargo test -p st-host --test port_status");
}
