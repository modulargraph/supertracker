//! SuperTracker's sound library: what is behind the Browser workspace. See
//! `docs/RUST_ARCHITECTURE.md`.
//!
//! The crate indexes the user's sample folders, NKS presets and kits, searches the index,
//! remembers favorites and labels, reads NKS preset files, reads and writes the application's
//! own kit packages, and imports Maschine groups as kits. It plays no audio and loads no
//! plug-in: it only reads and writes files, and nothing in it is real-time code.
//!
//! | Module | What it does |
//! |---|---|
//! | [`Library`] | The service the host registers the `library.*` and `kit.*` commands on. |
//! | [`api`] | Payloads and results of those commands (serde, the page's field names). |
//! | [`infer`] | Pack, category, tags, tempo, key and one-shot/loop from path names. |
//! | [`probe`] | Length of an audio file from its header. |
//! | [`nks`] | NKS preset files (metadata, identity, state, pages), the preset catalogue, product artwork. |
//! | [`kit`] | `.stkit` kit packages, the kit catalogue, Maschine group headers. |
//! | [`maschine`] | Importing Maschine `.mxgrp` groups: archive reader and control mappings. |
//! | [`roots`] | The library folders a fresh installation starts with, per platform. |
//!
//! # The service
//!
//! ```no_run
//! use st_library::{Library, LibraryConfig};
//! # fn main() -> st_library::Result<()> {
//! let library = Library::open(LibraryConfig::new("/path/to/support/Library"))?;
//! library.set_event_sink(|name, payload| println!("{name}: {payload}"));   // -> bridge events
//! library.scan(Default::default())?;                                        // returns at once
//! let results = library.search(serde_json::from_str(r#"{"query": "kick bpm:120", "limit": 100}"#).unwrap())?;
//! # let _ = results; Ok(()) }
//! ```
//!
//! Every command is one method that takes the command's payload type and returns its result,
//! so a host registers it in one line (`commands.with(&library).register_blocking("library.search", Library::search)`):
//!
//! | Command | Method | Payload | Result |
//! |---|---|---|---|
//! | `library.roots` | [`Library::roots`] | - | `Vec<`[`api::RootInfo`]`>` |
//! | `library.status` | [`Library::status`] | - | [`api::ScanStatus`] |
//! | `library.scan` | [`Library::scan`] | [`api::ScanRequest`] | [`api::ScanStatus`] |
//! | `library.cancel` | [`Library::cancel`] | - | [`api::ScanStatus`] |
//! | `library.removeRoot` | [`Library::remove_root`] | [`api::PathRequest`] | `Vec<`[`api::RootInfo`]`>` |
//! | `library.diagnostics` | [`Library::diagnostics`] | [`api::DiagnosticsRequest`] | `Vec<`[`api::RootProbe`]`>` |
//! | `library.search` | [`Library::search`] | [`api::SearchRequest`] | [`api::SearchResponse`] |
//! | `library.metadata.edit` | [`Library::edit_metadata`] | [`api::MetadataEditRequest`] | [`api::MetadataEditResponse`] |
//! | `library.favorite` | [`Library::favorite`] | [`api::FavoriteRequest`] | [`api::FavoriteResponse`] |
//! | `library.state.get` / `.set` | [`Library::state_get`], [`Library::state_set`] | - / [`api::StateRequest`] | JSON / [`api::Saved`] |
//! | `library.nks` | [`Library::nks_metadata`] | [`api::PathRequest`] | [`nks::NksMetadata`] |
//! | `library.nks.browse` | [`Library::nks_browse`] | [`api::NksBrowseRequest`] | [`nks::NksBrowseResponse`] |
//! | `library.nks.rescan` / `.status` | [`Library::nks_rescan`], [`Library::nks_status`] | - | [`nks::NksStatus`] |
//! | `library.nks.artwork` | [`Library::nks_artwork`] | [`api::ArtworkRequest`] | `{images, variant}` |
//! | `library.kits.browse` | [`Library::kits_browse`] | [`api::KitsBrowseRequest`] | [`kit::catalog::KitsBrowseResponse`] |
//! | `library.kit` | [`Library::kit_metadata`] | [`api::PathRequest`] | [`kit::KitMetadata`] |
//! | `kit.read` | [`Library::kit_read`] | [`api::PathRequest`] | [`api::KitReadResponse`] |
//! | `kit.detail` | [`Library::kit_detail`] | [`api::KitDetailRequest`] | [`api::KitDetail`] |
//! | `kit.save` | [`Library::kit_save`] | [`api::KitSaveRequest`] | [`api::KitSaveResponse`] |
//! | `plugin.loadNKS` (the NKS part) | [`Library::nks_preset`] | a path | [`nks::NksHostPreset`] |
//!
//! `library.chooseRoot` is the host's folder dialog followed by `library.scan`; the audio
//! preview a kit is saved with is rendered by the host and passed to `kit.save` as `previewPath`;
//! `kit.detail` judges plug-in availability from the component list the host passes in.
//!
//! Events (through the [`EventSink`], from worker threads): `library.progress` and
//! `library.results` ([`api::ScanStatus`]), `library.metadata` (header lengths arrived, labels
//! edited), `library.favorite`, `library.error`, `library.kits.progress`, `library.nks.progress`.
//!
//! The service owns its threads: one each for scanning, header reads, file writes, the NKS
//! catalogue and kit headers, plus a small watcher that notices library folders appearing and
//! disappearing (external drives). Scans, catalogue refreshes and writes never block a caller.
//! `search` and the two `browse` calls compute in the calling thread and belong on the host's
//! blocking lane; searches run one at a time and a search that a newer one overtook returns
//! with `cancelled: true`.
//!
//! # Files
//!
//! Everything lives in the support directory the caller gives ([`LibraryConfig::support_dir`]):
//!
//! | File | Content |
//! |---|---|
//! | `roots.json` | Library folders and the folders the user removed. User settings. |
//! | `index.bin` | The index cache: a compact binary file with a checksum. A cache of another version, a truncated or a damaged one is ignored and the index is rebuilt by scanning the roots. |
//! | `metadata.json` | The user's labels per file. Authoritative: written before results change, never rebuilt; an unreadable file is preserved and edits are refused. |
//! | `favorites.json` | Favorite paths. |
//! | `browser-state.json` | What the browser asked to remember (at most 256 KiB). |
//! | `diagnostics.json` | The last scan's outcome and a probe of every root. |
//! | `nks-catalog.json`, `kit-headers.json` | Caches of the preset catalogue and of Maschine group headers. |
//!
//! Plain files are enough at this scale: with 150,000 indexed files the cache is about 14 MiB,
//! loads in under a second, and an unfiltered search over the whole index takes tens of
//! milliseconds (see `tests/stress.rs`, which prints the figures).
//!
//! # Platforms
//!
//! Everything is portable Rust and is tested on Linux. What differs by platform is which
//! standard locations exist; that knowledge is in three small functions:
//!
//! | Function | macOS | Windows | Linux |
//! |---|---|---|---|
//! | [`roots`] defaults | Apple Loops, `~/Music/Loopcloud`, `~/Documents/Native Instruments/…`, registered NI content | `Documents\Native Instruments\…` | the kit folder only |
//! | [`nks::catalog::system_registrations`] | `com.native-instruments.*.plist` preferences and Service Center XML | none | none |
//! | [`nks::artwork::default_root`] | `/Users/Shared/NI Resources` | none | none |
//!
//! The readers behind them (property lists, classic Mac paths, the artwork folder layout, the
//! colour database) are platform-independent and tested on Linux with fixtures; the three
//! functions themselves only name locations and have not run on macOS or Windows yet. The
//! crate is type-checked for `aarch64-apple-darwin`.
//!
//! # No older layouts
//!
//! Only current formats are read. A kit package in the retired kit-lane layout (header
//! version 1), an index cache of another version and the Swift host's JSON index are not
//! converted: kits are refused with a message that says so, caches are rebuilt.

pub mod api;
mod error;
mod index;
pub mod infer;
pub mod kit;
mod library;
pub mod maschine;
pub mod nks;
pub mod probe;
pub mod roots;
mod search;
pub mod util;

pub use error::{Error, Result};
pub use library::{EventSink, Library, LibraryConfig};
