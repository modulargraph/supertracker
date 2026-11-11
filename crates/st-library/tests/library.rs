//! The library service: scanning, the cache, search with every filter and facet, paging,
//! favorites, labels, offline roots and nested roots.

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_library::api::*;
use st_library::probe::fixtures::wav;
use st_library::{Library, LibraryConfig};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn open(temp: &TempDir, cache: &str, roots: &[&str]) -> Library {
    Library::open(LibraryConfig::isolated(temp.join(cache), roots.iter().map(PathBuf::from).collect())).unwrap()
}

fn scan_all(library: &Library) -> ScanStatus {
    let status = library.scan(ScanRequest::default()).unwrap();
    library.wait_idle();
    status
}

fn request(value: Value) -> SearchRequest {
    serde_json::from_value(value).unwrap()
}

fn search(library: &Library, value: Value) -> SearchResponse {
    library.search(request(value)).unwrap()
}

fn total(library: &Library, filters: Value) -> usize {
    search(library, json!({ "filters": filters })).total
}

fn facet_count(response: &SearchResponse, facet: &str, value: &str) -> usize {
    response.facets[facet].iter().find(|entry| entry.value == value).map(|entry| entry.count).unwrap_or(0)
}

fn row<'a>(response: &'a SearchResponse, path: &str) -> &'a LibraryItem {
    response.items.iter().find(|item| item.path == path).unwrap_or_else(|| panic!("{path} is not in the results"))
}

type Events = Arc<Mutex<Vec<(String, Value)>>>;

fn record_events(library: &Library) -> Events {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    library.set_event_sink(move |name, payload| sink.lock().unwrap().push((name.to_string(), payload)));
    events
}

#[test]
fn scan_search_cache_and_folder_errors() {
    let temp = TempDir::new("library");
    let root = temp.mkdir("Samples");
    let sample = temp.write("Samples/Orbit Pack/Loops/Drums/Kick_120bpm_Cmin.wav", b"RIFF");
    temp.write("Samples/Orbit Pack/Loops/Drums/._Ghost.wav", &[1]);
    temp.write("Samples/Orbit Pack/Loops/Drums/.Hidden.wav", &[1]);
    temp.write("Samples/Orbit Pack/Loops/Drums/Readme.txt", &[1]);
    temp.write("Samples/Orbit Pack/Loops/Drums/Empty.wav", &[]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&root, temp.join("Samples/Orbit Pack/Loops/Drums/cycle")).unwrap();
    let library = open(&temp, "Cache", &[&root]);
    let events = record_events(&library);
    let started = scan_all(&library);
    assert!(started.scanning && started.scan_id.is_some() && started.roots.as_ref().unwrap()[0].path == root);

    let results = search(&library, json!({ "query": "kick bpm:120 key:Cminor pack:\"Orbit Pack\"", "kind": "sample" }));
    assert_eq!(results.items.len(), 1, "search and inferred metadata");
    let found = &results.items[0];
    assert_eq!((found.category.as_str(), found.pack.as_str(), found.bpm, found.key.as_deref()), ("Kick", "Orbit Pack", Some(120.0), Some("C minor")));
    assert_eq!((found.id.as_str(), found.kind.as_str(), found.extension.as_str(), found.root.as_str()), (sample.as_str(), "sample", "wav", root.as_str()));
    assert_eq!((found.folder.as_str(), found.collection.as_str(), found.sample_type.as_str(), found.size), (temp.text("Samples/Orbit Pack/Loops/Drums").as_str(), "User libraries", "loop", 4));
    assert_eq!(found.metadata_source, "filename and folders");
    assert_eq!(library.status(Empty).unwrap().count, 1, "hidden files, empty files and the link loop are ignored");
    for query in ["bpm:119-121", "tag:loop", "category:kick", "ext:wav", "ext:.wav", "kind:sample", "key:Cm", "KICK", "orbit", "cminor"] {
        assert_eq!(search(&library, json!({ "query": query })).total, 1, "query {query}");
    }
    for query in ["bpm:121", "tag:vocal", "ext:aif", "kind:preset", "key:Dm", "snare", "bpm:"] {
        assert_eq!(search(&library, json!({ "query": query })).total, 0, "query {query}");
    }
    let roots = library.roots(Empty).unwrap();
    assert_eq!((roots[0].scan_status.as_str(), roots[0].indexed_count, roots[0].available, roots[0].source.as_str()), ("complete", 1, true, "configured"));

    // Events: progress while scanning, results at the end.
    {
        let events = events.lock().unwrap();
        let finished: Vec<&Value> = events.iter().filter(|(name, payload)| name == "library.results" && payload["scanning"] == false).map(|(_, payload)| payload).collect();
        assert_eq!(finished.len(), 1);
        assert_eq!((finished[0]["count"].as_u64(), finished[0]["scanned"].as_u64(), finished[0]["cancelled"].as_bool()), (Some(1), Some(1), Some(false)));
        assert_eq!(finished[0]["scanID"], started.scan_id.clone().unwrap());
        assert!(events.iter().any(|(name, payload)| name == "library.progress" && payload["scanning"] == true && payload["root"] == root.as_str()));
        assert_eq!(finished[0]["rootReports"][0]["status"], "complete");
    }

    // Favorites and the index survive a restart.
    library.favorite(serde_json::from_value(json!({ "path": sample })).unwrap()).unwrap();
    assert_eq!(events.lock().unwrap().last().unwrap(), &("library.favorite".to_string(), json!({ "path": sample, "favorite": true })));
    library.wait_idle();
    {
        let cached = open(&temp, "Cache", &[&root]);
        let favorites = search(&cached, json!({ "favorites": true }));
        assert_eq!(favorites.total, 1, "index and favorites persisted");
        assert!(favorites.items[0].favorite);
        assert_eq!(cached.roots(Empty).unwrap()[0].scan_status, "complete", "scan reports persist");
    }
    library.favorite(serde_json::from_value(json!({ "path": sample, "favorite": false })).unwrap()).unwrap();
    assert_eq!(search(&library, json!({ "favorites": true })).total, 0);
    assert!(library.favorite(serde_json::from_value(json!({})).unwrap()).is_err());

    // A deleted file leaves the index on the next complete scan.
    fs::remove_file(&sample).unwrap();
    scan_all(&library);
    assert_eq!(search(&library, json!({})).total, 0, "deleted sample removed on a complete scan");
    assert_eq!(library.roots(Empty).unwrap()[0].scan_status, "empty");

    // Diagnostics look at folders from this process.
    let missing = temp.text("DisconnectedLibrary");
    let probes = library.diagnostics(serde_json::from_value(json!({ "roots": [root, missing] })).unwrap()).unwrap();
    assert!(probes[0].available && probes[0].can_read_directory && probes[0].entry_count > 0 && probes[0].sample_entries[0].attributes.is_directory);
    assert!(!probes[1].available && !probes[1].can_read_directory && probes[1].error.is_some());

    // A folder that cannot be scanned is reported, and the report survives a restart.
    library.scan(serde_json::from_value(json!({ "roots": [missing] })).unwrap()).unwrap();
    library.wait_idle();
    drop(library);
    let restored = open(&temp, "Cache", &[&root]);
    let roots = restored.roots(Empty).unwrap();
    let report = roots.iter().find(|entry| entry.path == missing).expect("a scanned folder stays a root");
    assert_eq!((report.scan_status.as_str(), report.available, report.source.as_str()), ("error", false, "user"));
    assert_eq!(report.scan_errors, [format!("Library is offline or unavailable: {missing}")]);
    assert!(!search(&restored, json!({})).errors.unwrap().is_empty(), "search exposes persisted failures");
    let diagnostics: Value = serde_json::from_slice(&fs::read(temp.join("Cache/diagnostics.json")).unwrap()).unwrap();
    assert!(diagnostics["rootProbes"].as_array().unwrap().iter().any(|probe| probe["path"] == missing.as_str() && probe["canReadDirectory"] == false));
    assert_eq!(restored.status(Empty).unwrap().diagnostics_path.unwrap(), temp.text("Cache/diagnostics.json"));
    // Removing the root forgets it, also across restarts.
    let remaining = restored.remove_root(serde_json::from_value(json!({ "path": missing })).unwrap()).unwrap();
    assert!(remaining.iter().all(|entry| entry.path != missing));
    drop(restored);
    assert!(open(&temp, "Cache", &[&root]).roots(Empty).unwrap().iter().all(|entry| entry.path != missing));
}

#[test]
fn packages_software_bundles_and_offline_folders() {
    let temp = TempDir::new("library-packages");
    let package_root = temp.mkdir("PackageSamples");
    temp.write("PackageSamples/Expansion.band/Audio/Pad.wav", &[1]);
    temp.write("PackageSamples/Plugin.component/Resources/Ignore.wav", &[1]);
    temp.write("PackageSamples/node_modules/thing/Ignore.wav", &[1]);
    let library = open(&temp, "PackageCache", &[&package_root]);
    scan_all(&library);
    let all = search(&library, json!({}));
    assert_eq!(all.total, 1, "sample packages are descended, software bundles skipped");
    assert_eq!(all.items[0].name, "Pad");
    fs::rename(&package_root, temp.join("MovedPackageSamples")).unwrap();
    scan_all(&library);
    assert_eq!(search(&library, json!({})).total, 1, "an offline folder keeps its cached records");
    assert_eq!(library.roots(Empty).unwrap()[0].scan_status, "error");
}

struct FilterFixture {
    temp: TempDir,
    ni: String,
    loopcloud: String,
    user: String,
    kick: String,
}

fn filter_fixture() -> FilterFixture {
    let temp = TempDir::new("library-filters");
    let (ni, loopcloud, user) = (temp.mkdir("Native Access Content"), temp.mkdir("Loopcloud"), temp.mkdir("User"));
    let kick = temp.write("Native Access Content/Anima Ascent Library/Samples/One Shots/Kick_120bpm_Cmin.wav", &wav(0.25));
    temp.write("Native Access Content/Anima Ascent Library/Samples/One Shots/Snare_100bpm_Dmaj.wav", &wav(0.5));
    temp.write("Native Access Content/Anima Ascent Library/Samples/Loops/Drum_Loop_130bpm_Cmin.wav", &wav(2.0));
    temp.mkdir("Native Access Content/Anima Ascent Library/Groups");
    temp.mkdir("Native Access Content/Anima Ascent Library/Sounds");
    temp.write("Native Access Content/Piano Library/Samples/Keys/Piano.wav", &wav(3.0));
    temp.write("Loopcloud/Pack Two/One Shots/Kick_128bpm_Cmin.wav", &wav(1.0));
    temp.write("User/Unclassified.wav", &[1, 2, 3]);
    FilterFixture { temp, ni, loopcloud, user, kick }
}

#[test]
fn filters_facets_and_header_lengths() {
    let fixture = filter_fixture();
    let original = fs::read(&fixture.kick).unwrap();
    let library = open(&fixture.temp, "Cache", &[&fixture.ni, &fixture.loopcloud, &fixture.user]);
    scan_all(&library);
    let first = search(&library, json!({ "limit": 1 }));
    assert_eq!(first.total, 6, "all fixture files indexed");
    assert_eq!(first.duration_coverage.as_ref().unwrap().pending, 1, "only the visible page queues header reads");
    assert_eq!((first.limit, first.offset, first.items.len()), (Some(1), Some(0), 1));
    assert_eq!(total(&library, json!({ "categories": ["Kick", "Snare"] })), 3, "categories OR together");
    assert_eq!(total(&library, json!({ "collections": ["Maschine expansions"] })), 3, "verified expansion provenance");
    assert_eq!(total(&library, json!({ "collections": ["Native Instruments"] })), 1, "ordinary NI libraries are not expansions");
    assert_eq!(total(&library, json!({ "categories": ["Kick", "Snare"], "collections": ["Maschine expansions"] })), 2, "different filters intersect");
    assert_eq!(total(&library, json!({ "sources": ["Loopcloud"] })), 1, "source alias");
    assert_eq!(total(&library, json!({ "collections": ["Loopcloud", "Native Instruments"] })), 2, "source selections OR together");
    assert_eq!(total(&library, json!({ "roots": [fixture.loopcloud, fixture.user] })), 2, "root selections OR together");
    assert_eq!(total(&library, json!({ "packs": ["Anima Ascent", "Pack Two"] })), 4, "pack selections OR together");
    assert_eq!((total(&library, json!({ "keys": ["C minor"] })), total(&library, json!({ "keys": ["unknown"] }))), (3, 2), "key and unknown filters");
    assert_eq!(total(&library, json!({ "keys": ["Cm"] })), 3, "key spellings are equivalent");
    assert_eq!(total(&library, json!({ "keys": ["C minor", "D major"], "sampleTypes": ["loop", "oneshot"] })), 4, "OR within a facet, AND across facets");
    assert_eq!(
        (total(&library, json!({ "sampleTypes": ["loop"] })), total(&library, json!({ "sampleTypes": ["oneshot"] })), total(&library, json!({ "sampleTypes": ["unknown"] }))),
        (1, 3, 2),
        "sample type requires explicit evidence"
    );
    assert_eq!(total(&library, json!({ "extensions": ["wav"], "tags": ["Kick", "Snare"] })), 3, "format and tag filters");
    assert_eq!(search(&library, json!({ "category": "Kick" })).total, 2, "single category parameter");
    assert_eq!(search(&library, json!({ "pack": "Pack Two", "category": "all" })).total, 1, "single pack parameter");

    let faceted = search(&library, json!({ "filters": { "categories": ["Kick"], "collections": ["Maschine expansions"] } }));
    assert_eq!(facet_count(&faceted, "categories", "Snare"), 1, "a disjunctive facet keeps its alternatives");
    assert_eq!(faceted.facets["categories"][0].value, "Kick", "selected values are listed first");
    assert_eq!((facet_count(&faceted, "collections", "Loopcloud"), facet_count(&faceted, "sources", "Maschine expansions")), (1, 1));
    let all = search(&library, json!({}));
    assert_eq!(facet_count(&all, "roots", &fixture.ni), 4);
    assert_eq!(all.facets["roots"].iter().find(|facet| facet.value == fixture.ni).unwrap().label, "Native Access Content", "roots are labelled by name");
    assert_eq!((facet_count(&all, "keys", "unknown"), all.facets["keys"].iter().find(|facet| facet.value == "unknown").unwrap().label.as_str()), (2, "Unknown"));
    assert_eq!((facet_count(&all, "bpm", "known"), facet_count(&all, "bpm", "unknown")), (4, 2));
    assert_eq!((facet_count(&all, "sampleTypes", "oneshot"), facet_count(&all, "extensions", "wav"), facet_count(&all, "packs", "Anima Ascent")), (3, 6, 3));
    assert_eq!(all.categories, all.facets["categories"]);
    assert_eq!(all.facets_truncated, Some(vec![]));

    assert_eq!(total(&library, json!({ "bpm": { "min": 110, "max": 129 } })), 2, "BPM range");
    assert_eq!(total(&library, json!({ "bpm": { "min": 110, "max": 129, "includeUnknown": true } })), 4, "BPM range plus unknown");
    assert_eq!(total(&library, json!({ "bpm": { "includeUnknown": true } })), 2, "unknown-only BPM");

    search(&library, json!({ "filters": { "duration": { "min": 0.45, "max": 0.6 } } }));
    library.wait_idle();
    assert_eq!(total(&library, json!({ "duration": { "min": 0.45, "max": 0.6 } })), 1, "a range filter reads candidate headers before excluding by length");
    library.wait_idle();
    assert_eq!(total(&library, json!({ "duration": { "includeUnknown": true } })), 1, "unreadable audio stays explicitly unknown");
    assert_eq!(total(&library, json!({ "duration": { "min": 0.45, "max": 0.6, "includeUnknown": true } })), 2, "duration range plus unknown");
    library.wait_idle();
    let timed = search(&library, json!({}));
    let kick = row(&timed, &fixture.kick);
    assert!((kick.duration.unwrap() - 0.25).abs() < 0.001, "WAV header length read without decoding");
    assert_eq!((kick.duration_status.as_str(), kick.duration_source.as_deref()), ("known", Some("audio header")));
    assert_eq!(timed.duration_coverage, Some(DurationCoverage { known: 5, unknown: 1, pending: 0 }));
    assert_eq!((facet_count(&timed, "duration", "known"), facet_count(&timed, "duration", "unknown")), (5, 1));

    library.flush_cache();
    {
        let restored = open(&fixture.temp, "Cache", &[&fixture.ni, &fixture.loopcloud, &fixture.user]);
        assert!((row(&search(&restored, json!({})), &fixture.kick).duration.unwrap() - 0.25).abs() < 0.001, "header length persists");
        restored.wait_idle();
        assert_eq!(total(&restored, json!({ "collections": ["Maschine expansions"] })), 3, "verified provenance persists");
    }
    assert_eq!(fs::read(&fixture.kick).unwrap(), original, "source sample unchanged");

    // Presets are their own kind with their own facets.
    let summary = json!({ "name": "Warm Pad", "types": [["Synth Pad", "Warm"]], "modes": ["Analog"] });
    let preset = fixture.temp.write("User/Presets/Preset.nksf", &st_library::nks::fixtures::preset(&summary, None, None, Some(&[1])));
    library.scan(serde_json::from_value(json!({ "roots": [fixture.user] })).unwrap()).unwrap();
    library.wait_idle();
    let only_nks = search(&library, json!({ "kind": "nks", "limit": 1 }));
    assert_eq!((only_nks.total, only_nks.facets["extensions"][0].value.as_str()), (1, "nksf"), "the kind filter separates preset facets");
    let found = row(&only_nks, &preset);
    assert_eq!((found.name.as_str(), found.category.as_str(), found.kind.as_str(), found.key.clone()), ("Warm Pad", "Pad & Atmosphere", "nks", None));
    assert!(found.tags.contains(&"Synth Pad / Warm".to_string()) && found.tags.contains(&"Analog".to_string()) && !found.tags.contains(&"Other".to_string()));
    assert_eq!(found.metadata_source, "NKS metadata and folders");
    let next_page = search(&library, json!({ "kind": "nks", "offset": 1, "limit": 1 }));
    assert_eq!((next_page.total, next_page.items.len()), (1, 0), "the population cache keeps pagination counts");
    let only_samples = search(&library, json!({ "kind": "sample" }));
    assert_eq!((only_samples.total, only_samples.facets["extensions"][0].value.as_str()), (6, "wav"), "kind changes do not reuse stale facets");
    assert_eq!(search(&library, json!({ "kind": "all" })).total, 7);
    assert_eq!(search(&library, json!({ "query": "kind:preset" })).total, 1);
}

#[test]
fn offline_roots_and_browser_state() {
    let temp = TempDir::new("library-offline");
    let (online, offline) = (temp.mkdir("Online"), temp.mkdir("Offline"));
    let up = temp.write("Online/Pack/Loops/Bass[120] Am Up.wav", &[1, 2]);
    let away = temp.write("Offline/Pack/Loops/Synth[128] C Away.wav", &[1, 2]);
    let library = open(&temp, "Cache", &[&online, &offline]);
    let events = record_events(&library);
    scan_all(&library);
    library.flush_cache();
    drop(library);
    fs::rename(&offline, temp.join("Ejected")).unwrap();

    let restored = open(&temp, "Cache", &[]);
    assert_eq!(restored.roots(Empty).unwrap().len(), 2, "the roots are remembered without being configured again");
    let all = search(&restored, json!({ "limit": 10 }));
    assert_eq!(all.items.len(), 2);
    assert_eq!((row(&all, &away).available, row(&all, &up).available), (Some(false), None), "an offline root marks its sounds unavailable");
    assert_eq!((row(&all, &away).bpm, row(&all, &away).key.as_deref(), row(&all, &up).category.as_str()), (Some(128.0), Some("C major"), "Bass"));
    let summary = all.offline.unwrap();
    assert_eq!((summary.len(), summary[0].path.as_str(), summary[0].name.as_str(), summary[0].matching), (1, offline.as_str(), "Offline", 1));
    assert_eq!(search(&restored, json!({ "hideOffline": true })).total, 1, "offline sounds can be hidden");
    let filtered = search(&restored, json!({ "hideOffline": true, "filters": { "keys": ["A minor"] } }));
    assert_eq!(filtered.offline.unwrap()[0].matching, 0, "the offline summary respects filters while hidden");
    let only_offline = search(&restored, json!({ "filters": { "keys": ["C major"] } }));
    assert_eq!((only_offline.offline.unwrap()[0].matching, only_offline.total), (1, 1));
    assert!(!restored.roots(Empty).unwrap().iter().find(|root| root.path == offline).unwrap().available);
    fs::rename(temp.join("Ejected"), &offline).unwrap();
    assert!(search(&restored, json!({ "limit": 10 })).offline.unwrap().is_empty(), "a reconnected root becomes available");

    // What the browser asked to remember survives a restart.
    assert_eq!(restored.state_get(Empty).unwrap(), json!({}));
    restored.state_set(serde_json::from_value(json!({ "state": { "query": "kick", "recent": [{ "query": "snare" }] } })).unwrap()).unwrap();
    assert!(restored.state_set(serde_json::from_value(json!({ "state": [1] })).unwrap()).is_err());
    assert!(restored.state_set(serde_json::from_value(json!({ "state": { "big": "x".repeat(300_000) } })).unwrap()).is_err());
    restored.wait_idle();
    drop(restored);
    let state = open(&temp, "Cache", &[]).state_get(Empty).unwrap();
    assert_eq!(state, json!({ "query": "kick", "recent": [{ "query": "snare" }] }));
    drop(events);
}

#[test]
fn folder_watching_reports_mounts_and_ejects() {
    let temp = TempDir::new("library-watch");
    let root = temp.mkdir("Drive");
    temp.write("Drive/Pack/Kick.wav", &[1]);
    let mut config = LibraryConfig::isolated(temp.join("Cache"), vec![PathBuf::from(&root)]);
    config.watch_interval = Some(std::time::Duration::from_millis(20));
    let library = Library::open(config).unwrap();
    scan_all(&library);
    let events = record_events(&library);
    fs::rename(&root, temp.join("Gone")).unwrap();
    let seen = |events: &Events| events.lock().unwrap().iter().filter(|(name, payload)| name == "library.results" && payload["volumes"] == true).count();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while seen(&events) < 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(seen(&events), 1, "an ejected folder is announced once");
    fs::rename(temp.join("Gone"), &root).unwrap();
    while seen(&events) < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(seen(&events), 2, "and so is its return");
}

#[test]
fn nested_roots_take_their_parents_result() {
    let temp = TempDir::new("library-nested");
    let parent = temp.mkdir("Parent");
    let nested = temp.mkdir("Parent/Pack A/Loops");
    let other = temp.mkdir("Other");
    let inside = temp.write("Parent/Pack A/Loops/Loop_120_x.wav", &[1]);
    temp.write("Parent/Pack B/Kick.wav", &[1]);
    let library = open(&temp, "Cache", &[&parent, &nested, &other]);
    let events = record_events(&library);
    scan_all(&library);
    let roots = library.roots(Empty).unwrap();
    let status = |path: &str| roots.iter().find(|root| root.path == path).unwrap().scan_status.clone();
    assert_eq!((status(&parent), status(&nested), status(&other)), ("complete".to_string(), "complete".to_string(), "empty".to_string()), "a nested root takes its parent's result instead of staying pending");
    let all = search(&library, json!({}));
    assert_eq!(all.total, 2, "files below nested roots are indexed once");
    assert_eq!((row(&all, &inside).root.as_str(), row(&all, &inside).pack.as_str()), (parent.as_str(), "Pack A"), "the parent walks nested roots, so packs stay stable");
    let last = events.lock().unwrap().iter().rev().find(|(name, _)| name == "library.results").unwrap().1.clone();
    assert_eq!(last["scanned"], 2);
    // Scanning only the nested root later keeps the files under their parent's identity.
    library.scan(serde_json::from_value(json!({ "roots": [nested] })).unwrap()).unwrap();
    library.wait_idle();
    let again = search(&library, json!({}));
    assert_eq!(again.total, 2);
    assert_eq!(library.roots(Empty).unwrap().len(), 3);

    assert!(library.scan(serde_json::from_value(json!({ "roots": [] })).unwrap()).is_err());
    assert!(library.scan(serde_json::from_value(json!({ "roots": vec!["/x"; 129] })).unwrap()).is_err());
}

/// A synthetic library: `packs` packs of `per_pack` files with realistic names.
fn synthetic_library(temp: &TempDir, packs: usize, per_pack: usize) -> String {
    let kinds = ["Kick", "Snare", "ClosedHH", "Bass", "Pad", "Vocal", "Guitar", "FX Riser"];
    let keys = ["Cmin", "F#maj", "Am", "Dmin", "G", "Bbm"];
    for pack in 0..packs {
        for index in 0..per_pack {
            let kind = kinds[index % kinds.len()];
            let (folder, name) = if index % 2 == 0 {
                ("One Shots", format!("{kind} {:03}.wav", index))
            } else {
                ("Loops", format!("{kind}_Loop_{}_{}_{:03}.wav", 90 + (index % 9) * 10, keys[index % keys.len()], index))
            };
            temp.write(&format!("Library/Pack {pack:02} [1{pack:04}]/{folder}/{kind}/{name}"), &[1]);
        }
    }
    temp.text("Library")
}

#[test]
fn thousands_of_files_incremental_rescan_paging_and_cancellation() {
    let temp = TempDir::new("library-large");
    let root = synthetic_library(&temp, 30, 100);
    let library = open(&temp, "Cache", &[&root]);
    let events = record_events(&library);
    scan_all(&library);
    assert_eq!(library.status(Empty).unwrap().count, 3000);
    let all = search(&library, json!({ "limit": 1000 }));
    assert_eq!((all.total, all.items.len()), (3000, 1000));
    assert_eq!(all.facets["packs"].len(), 30);
    assert_eq!(all.facets["packs"][0].value, "Pack 00", "Loopcloud pack numbers are not part of the pack name");
    assert_eq!((facet_count(&all, "categories", "Hi-hat"), facet_count(&all, "categories", "Kick"), facet_count(&all, "sampleTypes", "loop")), (390, 390, 1500));
    assert_eq!(search(&library, json!({ "query": "snare loop bpm:100-120 key:Am" })).total, search(&library, json!({ "query": "snare", "filters": { "sampleTypes": ["loop"], "bpm": { "min": 100, "max": 120 }, "keys": ["A minor"] } })).total);

    // Paging: pages are disjoint, ordered, and together the whole result.
    let query = json!({ "query": "pack", "filters": { "categories": ["Bass", "Pad & Atmosphere"] } });
    let whole = search(&library, json!({ "query": "pack", "filters": query["filters"], "limit": 1000 }));
    assert_eq!(whole.total, 750);
    let mut paged: Vec<String> = Vec::new();
    for offset in (0..whole.total).step_by(64) {
        let page = search(&library, json!({ "query": "pack", "filters": query["filters"], "offset": offset, "limit": 64 }));
        assert_eq!(page.total, whole.total);
        paged.extend(page.items.into_iter().map(|item| item.path));
    }
    assert_eq!(paged, whole.items.iter().map(|item| item.path.clone()).collect::<Vec<_>>(), "pages follow the full ordering");
    assert_eq!(search(&library, json!({ "offset": 5000 })).items.len(), 0);
    // A name match ranks before a path-only match; a prefix before an infix.
    let ranked = search(&library, json!({ "query": "kick", "limit": 5 }));
    assert!(ranked.items.iter().all(|item| item.name.to_lowercase().starts_with("kick")));

    // An incremental rescan reads only what changed.
    let changed = temp.write("Library/Pack 00 [10000]/One Shots/Kick/Kick 000.wav", &[1, 2, 3]);
    let added = temp.write("Library/Pack 00 [10000]/One Shots/Kick/Kick New.wav", &[1]);
    let removed = temp.text("Library/Pack 01 [10001]/One Shots/Kick/Kick 000.wav");
    fs::remove_file(&removed).unwrap();
    events.lock().unwrap().clear();
    scan_all(&library);
    let rescanned = search(&library, json!({ "query": "\"kick 000\" pack:\"pack 0\"", "limit": 50 }));
    assert_eq!(row(&rescanned, &changed).size, 3, "a changed file is read again");
    assert!(rescanned.items.iter().all(|item| item.path != removed), "a deleted file leaves the index");
    assert_eq!(search(&library, json!({ "query": "\"kick new\"" })).items[0].path, added);
    assert_eq!(library.status(Empty).unwrap().count, 3000);
    let finished = events.lock().unwrap().iter().rev().find(|(name, payload)| name == "library.results" && payload["scanning"] == false).unwrap().1.clone();
    assert_eq!((finished["scanned"].as_u64(), finished["count"].as_u64()), (Some(3000), Some(3000)));

    // Cancelling a scan: it stops, reports as cancelled, and removes nothing.
    let mut slow = LibraryConfig::isolated(temp.join("SlowCache"), vec![PathBuf::from(&root)]);
    slow.scan_pause = std::time::Duration::from_millis(20);
    let slow = Library::open(slow).unwrap();
    let slow_events = record_events(&slow);
    slow.scan(ScanRequest::default()).unwrap();
    while slow.status(Empty).unwrap().count == 0 {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let cancelled = slow.cancel(Empty).unwrap();
    assert_eq!((cancelled.scanning, cancelled.cancelled), (false, Some(true)));
    assert!(cancelled.root_reports.unwrap().iter().all(|report| report.status == "cancelled"));
    slow.wait_idle();
    let partial = slow.status(Empty).unwrap();
    assert!(!partial.scanning && partial.count > 0 && partial.count < 3000, "a cancelled scan keeps what it indexed: {}", partial.count);
    assert_eq!(slow.roots(Empty).unwrap()[0].scan_status, "cancelled");
    assert!(slow_events.lock().unwrap().iter().all(|(name, payload)| !(name == "library.results" && payload["scanning"] == false)), "a cancelled scan publishes no final results");
    // A new scan replaces a running one and completes.
    slow.scan(ScanRequest::default()).unwrap();
    let second = slow.scan(ScanRequest::default()).unwrap();
    drop(slow_events);
    let mut fast = LibraryConfig::isolated(temp.join("SlowCache"), vec![PathBuf::from(&root)]);
    fast.scan_pause = std::time::Duration::ZERO;
    drop(slow);
    let fast = Library::open(fast).unwrap();
    assert!(fast.status(Empty).unwrap().count < 3000, "a scan interrupted by quitting leaves a partial index");
    assert_eq!(fast.roots(Empty).unwrap()[0].scan_status, "cancelled", "and is not reported as still scanning");
    scan_all(&fast);
    assert_eq!(fast.status(Empty).unwrap().count, 3000);
    assert!(second.scan_id.is_some());
}

#[test]
fn a_damaged_or_foreign_cache_is_rebuilt() {
    let temp = TempDir::new("library-rebuild");
    let root = synthetic_library(&temp, 3, 20);
    {
        let library = open(&temp, "Cache", &[&root]);
        scan_all(&library);
        library.favorite(serde_json::from_value(json!({ "path": temp.text("Library/Pack 00 [10000]/One Shots/Kick/Kick 000.wav") })).unwrap()).unwrap();
        library.wait_idle();
    }
    let cache = temp.join("Cache/index.bin");
    let good = fs::read(&cache).unwrap();
    for (label, bytes) in [
        ("truncated", good[..good.len() / 2].to_vec()),
        ("damaged", {
            let mut bytes = good.clone();
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xff;
            bytes
        }),
        ("another version", {
            let mut bytes = good.clone();
            bytes[8] = bytes[8].wrapping_add(1);
            bytes
        }),
        ("another format", br#"{"version":1,"roots":[],"items":[]}"#.to_vec()),
    ] {
        fs::write(&cache, &bytes).unwrap();
        let events: Events = Arc::new(Mutex::new(Vec::new()));
        // Opened without roots: the remembered folders are scanned again by themselves.
        let mut config = LibraryConfig::isolated(temp.join("Cache"), Vec::new());
        config.load_cache_synchronously = false;
        let library = Library::open(config).unwrap();
        let sink = events.clone();
        library.set_event_sink(move |name, payload| sink.lock().unwrap().push((name.to_string(), payload)));
        library.wait_idle();
        assert_eq!(search(&library, json!({})).total, 60, "{label}: the index is rebuilt from the folders");
        assert_eq!(search(&library, json!({ "favorites": true })).total, 1, "{label}: favorites are not part of the cache");
        assert_eq!(library.roots(Empty).unwrap()[0].scan_status, "complete");
        drop(library);
        assert!(st_library::Library::open(LibraryConfig::isolated(temp.join("Cache"), Vec::new())).unwrap().status(Empty).unwrap().count == 60, "{label}: the rebuilt cache is written");
    }
    // No cache at all (a first start) does not scan by itself.
    fs::remove_file(&cache).unwrap();
    let fresh = open(&temp, "Cache", &[]);
    fresh.wait_idle();
    assert_eq!((fresh.status(Empty).unwrap().count, fresh.roots(Empty).unwrap().len()), (0, 1));
}

fn edit(library: &Library, paths: &[&str], changes: Value) -> st_library::Result<MetadataEditResponse> {
    library.edit_metadata(serde_json::from_value(json!({ "paths": paths, "changes": changes })).unwrap())
}

#[test]
fn labels_are_edited_persisted_and_never_touch_files() {
    let temp = TempDir::new("library-labels");
    let root = temp.mkdir("Sounds");
    let kick = temp.write("Sounds/Pack/Kick_120bpm_Cmin.wav", &wav(0.5));
    let pad = temp.write("Sounds/Pack/Warm Pad.wav", &wav(1.0));
    let snare = temp.write("Sounds/Pack/Snare 01.wav", &wav(0.25));
    let bytes: Vec<Vec<u8>> = [&kick, &pad, &snare].iter().map(|path| fs::read(path).unwrap()).collect();
    let dates: Vec<_> = [&kick, &pad, &snare].iter().map(|path| fs::metadata(path).unwrap().modified().unwrap()).collect();
    let library = open(&temp, "Cache", &[&root]);
    let events = record_events(&library);
    let scanned = scan_all(&library);
    assert!(scanned.scanning);
    let before = search(&library, json!({}));
    let original = row(&before, &kick).clone();
    assert_eq!((original.category.as_str(), original.bpm, original.key.as_deref(), original.sample_type.as_str()), ("Kick", Some(120.0), Some("C minor"), "loop"));

    // A batch edit: duplicates count once, values are trimmed and deduplicated.
    let response = edit(&library, &[&kick, &snare, &kick], json!({ "category": "  Texture ", "tags": [" Custom", "custom", "Wide "] })).unwrap();
    assert_eq!(response.updated, 2);
    for item in &response.items {
        assert_eq!((item.category.as_str(), item.tags.clone()), ("Texture", vec!["Custom".to_string(), "Wide".to_string()]));
        assert_eq!(item.user_metadata, json!({ "category": "Texture", "tags": ["Custom", "Wide"] }), "partial edits create no unrelated labels");
    }
    let edited = row_in(&response.items, &kick);
    assert_eq!((edited.bpm, edited.key.clone(), edited.sample_type.as_str()), (original.bpm, original.key.clone(), "loop"), "omitted fields are unchanged");
    assert_eq!(edited.inferred_metadata, original.inferred_metadata, "inferred metadata stays available");
    assert_eq!(edited.inferred_metadata.category, "Kick");
    assert_eq!(events.lock().unwrap().last().unwrap(), &("library.metadata".to_string(), json!({ "paths": [kick, snare], "edited": true })));
    let after = search(&library, json!({}));
    assert_eq!(row(&after, &pad).user_metadata, json!({}), "an unselected item stays unedited");
    assert_eq!((facet_count(&after, "categories", "Texture"), facet_count(&after, "categories", "Kick")), (2, 0), "category facets reflect labels");
    assert_eq!((facet_count(&after, "tags", "Custom"), facet_count(&after, "tags", "Kick")), (2, 0), "tag facets reflect replacement");
    assert_eq!(search(&library, json!({ "query": "tag:custom", "filters": { "categories": ["Texture"] } })).total, 2, "query and filters find labels at once");

    edit(&library, &[&kick], json!({ "bpm": 142, "key": "gb MIN", "sampleType": "oneshot" })).unwrap();
    let musical = search(&library, json!({ "filters": { "bpm": { "min": 140, "max": 144 }, "keys": ["F# minor"], "sampleTypes": ["oneshot"] } }));
    assert_eq!(musical.total, 1, "tempo, key and type filters use edited values");
    assert_eq!(row(&musical, &kick).key.as_deref(), Some("Gb minor"), "the user's spelling is kept in canonical form");
    for (typed, expected) in [("BB MAJOR", "Bb major"), ("eB m", "Eb minor"), ("c#", "C#"), ("A", "A")] {
        let changed = edit(&library, &[&kick], json!({ "key": typed })).unwrap();
        assert_eq!(changed.items[0].key.as_deref(), Some(expected));
    }

    // Explicit unknowns hide inferred values; null restores them.
    let unknown = edit(&library, &[&kick], json!({ "bpm": 0, "key": "", "sampleType": "unknown", "tags": [] })).unwrap().items.remove(0);
    assert_eq!((unknown.bpm, unknown.key.clone(), unknown.sample_type.as_str(), unknown.tags.len()), (None, None, "unknown", 0));
    assert_eq!((unknown.user_metadata["bpm"].as_f64(), unknown.user_metadata["key"].as_str()), (Some(0.0), Some("")), "an explicit unknown is a stored label");
    assert_eq!(search(&library, json!({ "filters": { "bpm": { "includeUnknown": true }, "keys": ["unknown"] } })).total, 3, "unknown filters include explicit and natural unknowns");
    let partial = edit(&library, &[&kick], json!({ "bpm": null, "key": null, "sampleType": "oneshot" })).unwrap().items.remove(0);
    assert_eq!((partial.bpm, partial.key.clone(), partial.sample_type.as_str(), partial.category.as_str(), partial.tags.len()), (Some(120.0), Some("C minor".to_string()), "oneshot", "Texture", 0));
    let reset = edit(&library, &[&kick, &snare], json!({ "category": null, "tags": null, "sampleType": null })).unwrap();
    assert!(reset.items.iter().all(|item| item.user_metadata == json!({})), "restoring the last fields clears the labels");
    assert_eq!((row_in(&reset.items, &kick).category.as_str(), &row_in(&reset.items, &kick).tags), ("Kick", &original.tags));
    let persisted: Value = serde_json::from_slice(&fs::read(temp.join("Cache/metadata.json")).unwrap()).unwrap();
    assert_eq!(persisted, json!({ "version": 1, "items": {} }), "empty label records leave the file");

    // Add tags keeps each item's own tags.
    let added = edit(&library, &[&kick, &pad], json!({ "addTags": ["Shared", "kick"] })).unwrap();
    assert_eq!(row_in(&added.items, &kick).tags, [original.tags.clone(), vec!["Shared".to_string()]].concat(), "added tags are appended and deduplicated");
    assert!(row_in(&added.items, &pad).tags.contains(&"Shared".to_string()) && row_in(&added.items, &pad).tags.contains(&"kick".to_string()));
    assert!(edit(&library, &[&kick], json!({ "addTags": ["x"], "tags": ["y"] })).is_err());
    edit(&library, &[&kick, &pad], json!({ "tags": null })).unwrap();

    // Labels and the inferred cache are separate files; both survive a restart and a rescan.
    edit(&library, &[&kick], json!({ "category": "Texture", "tags": ["Persistent"], "bpm": 142, "key": "" })).unwrap();
    library.flush_cache();
    let saved = fs::read(temp.join("Cache/metadata.json")).unwrap();
    drop(library);
    let reopened = open(&temp, "Cache", &[&root]);
    let cached = row(&search(&reopened, json!({})), &kick).clone();
    assert_eq!((cached.category.as_str(), cached.tags.clone(), cached.bpm, cached.key.clone()), ("Texture", vec!["Persistent".to_string()], Some(142.0), None), "a recreated service restores every label");
    assert_eq!((cached.inferred_metadata.category.as_str(), cached.inferred_metadata.bpm, cached.inferred_metadata.key.as_str()), ("Kick", 120.0, "C minor"), "the cache holds inferred values, not labels");
    // The file changes on disk: the rescan refreshes inference and keeps the labels by path.
    fs::write(&kick, wav(1.0)).unwrap();
    scan_all(&reopened);
    let rescanned = row(&search(&reopened, json!({})), &kick).clone();
    assert_eq!((rescanned.size, rescanned.category.as_str(), rescanned.bpm), (fs::metadata(&kick).unwrap().len(), "Texture", Some(142.0)));
    reopened.wait_idle();
    let enriched = row(&search(&reopened, json!({})), &kick).clone();
    assert!((enriched.duration.unwrap() - 1.0).abs() < 0.001 && enriched.user_metadata == cached.user_metadata, "a header length arrives without disturbing labels");
    assert_eq!(fs::read(temp.join("Cache/metadata.json")).unwrap(), saved, "scanning and header reads never rewrite the labels file");
    let restored = edit(&reopened, &[&kick], json!({ "category": null, "tags": null, "bpm": null, "key": null })).unwrap().items.remove(0);
    assert_eq!((restored.category.as_str(), restored.bpm, restored.key.as_deref()), ("Kick", Some(120.0), Some("C minor")));
    assert!((restored.duration.unwrap() - 1.0).abs() < 0.001, "restoring labels keeps the length");

    // Invalid edits change nothing, in memory or on disk; a batch is all or nothing.
    let baseline = fs::read(temp.join("Cache/metadata.json")).unwrap();
    let baseline_rows = search(&reopened, json!({})).items;
    for changes in [json!({ "bpm": 1000 }), json!({ "key": "H" }), json!({ "sampleType": "x" }), json!({ "unknown": 1 }), json!({ "tags": "one" }), json!({ "category": "" }), json!({ "category": "Good", "bpm": -3 })] {
        assert!(edit(&reopened, &[&kick, &pad], changes.clone()).is_err(), "{changes}");
    }
    assert!(edit(&reopened, &[], json!({ "category": "x" })).is_err());
    assert!(edit(&reopened, &[&kick, "/not/indexed.wav"], json!({ "category": "x" })).unwrap_err().to_string().contains("no longer indexed"));
    assert!(reopened.edit_metadata(serde_json::from_value(json!({ "paths": [kick] })).unwrap()).is_err());
    assert_eq!(fs::read(temp.join("Cache/metadata.json")).unwrap(), baseline);
    assert_eq!(search(&reopened, json!({})).items, baseline_rows, "invalid edits do not partly apply");
    // A tag limit that only the combined tags of one item exceed rolls the whole batch back.
    let many: Vec<String> = (0..63).map(|index| format!("Tag {index}")).collect();
    edit(&reopened, &[&pad], json!({ "tags": many })).unwrap();
    let full = fs::read(temp.join("Cache/metadata.json")).unwrap();
    assert!(edit(&reopened, &[&kick, &pad], json!({ "addTags": ["One more", "And another"] })).is_err());
    assert_eq!(fs::read(temp.join("Cache/metadata.json")).unwrap(), full);
    assert_eq!(row(&search(&reopened, json!({})), &kick).user_metadata, json!({}), "a later failure in the batch rolls back every item");

    // A failed write publishes nothing.
    fs::remove_file(temp.join("Cache/metadata.json")).unwrap();
    fs::create_dir(temp.join("Cache/metadata.json")).unwrap();
    assert!(edit(&reopened, &[&kick], json!({ "category": "Unsaved" })).is_err());
    assert_eq!(row(&search(&reopened, json!({})), &kick).category, "Kick", "a failed write does not change results");
    fs::remove_dir(temp.join("Cache/metadata.json")).unwrap();
    assert_eq!(edit(&reopened, &[&kick], json!({ "category": "Saved" })).unwrap().items[0].category, "Saved", "a retry starts from the last successful state");
    drop(reopened);

    // A labels file that cannot be read is preserved byte for byte and blocks edits.
    for malformed in [&b"{not json"[..], br#"{"version":2,"items":{}}"#, br#"{"version":1,"items":{"/a.wav":{"bpm":5000}}}"#] {
        fs::write(temp.join("Cache/metadata.json"), malformed).unwrap();
        let blocked = open(&temp, "Cache", &[&root]);
        assert!(search(&blocked, json!({})).items.iter().all(|item| item.user_metadata == json!({})), "an invalid labels file is not partly applied");
        assert!(edit(&blocked, &[&kick], json!({ "category": "x" })).unwrap_err().to_string().contains("has been preserved"));
        assert_eq!(fs::read(temp.join("Cache/metadata.json")).unwrap(), malformed);
    }
    for (index, path) in [&kick, &pad, &snare].iter().enumerate().skip(1) {
        assert_eq!(fs::read(path).unwrap(), bytes[index], "editing and header reads leave sample bytes unchanged");
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), dates[index], "and their dates");
    }
}

fn row_in<'a>(items: &'a [LibraryItem], path: &str) -> &'a LibraryItem {
    items.iter().find(|item| item.path == path).unwrap()
}

#[test]
fn requests_accept_what_the_page_sends() {
    // Missing, null and extra fields; numbers as floats.
    let request: SearchRequest = serde_json::from_value(json!({ "query": null, "kind": "sample", "filters": { "categories": null, "bpm": { "min": 100.5, "includeUnknown": null }, "extra": 1 }, "offset": 20.0, "limit": 1e9, "favorites": null, "unknownField": {} })).unwrap();
    assert_eq!((request.query.as_str(), request.kind.as_str(), request.filters.bpm.min, request.offset, request.favorites), ("", "sample", Some(100.5), Some(20.0), false));
    let favorite: FavoriteRequest = serde_json::from_value(json!({ "path": "/a.wav" })).unwrap();
    assert!(favorite.favorite, "favorite defaults to true");
    let save: KitSaveRequest = serde_json::from_value(json!({ "project": {}, "groupID": "g", "overwrite": null })).unwrap();
    assert!(save.copy_samples && !save.overwrite && save.group_id.as_deref() == Some("g"));
    let _: Empty = serde_json::from_value(json!({ "anything": [1, 2] })).unwrap();
    let _: Empty = serde_json::from_value(json!(null)).unwrap();
    assert_eq!(serde_json::to_value(ScanStatus { scanning: true, count: 3, ..Default::default() }).unwrap(), json!({ "scanning": true, "count": 3 }));
    let cancelled = serde_json::to_value(SearchResponse { cancelled: Some(true), ..Default::default() }).unwrap();
    assert_eq!(cancelled, json!({ "items": [], "total": 0, "scanning": false, "cancelled": true, "categories": [], "packs": [] }));
}
