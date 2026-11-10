//! A synthetic index of 150,000 entries: search, deep paging, cached filters, a burst of
//! superseded searches, and the cache file. Ported from `LibraryService.stressTest`
//! (`tests/LibraryStressTests.swift`); the numbers it prints are the crate's measured
//! performance (run with `--release -- --nocapture` for the figures quoted in the docs).

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_library::api::*;
use st_library::{Library, LibraryConfig};
use std::sync::{Arc, Barrier};
use std::time::Instant;

const COUNT: usize = 150_000;

fn name(index: usize) -> String {
    format!("Sample_{index:08}")
}

type Synthetic = (String, String, String, String, String, Vec<String>, Option<f64>, Option<String>, Option<f64>);

fn synthetic(count: usize, root: String) -> impl Iterator<Item = Synthetic> {
    (0..count).map(move |index| {
        let pack = format!("Pack-{}", index % 100);
        (
            format!("{root}/{pack}/{}.wav", name(index)),
            root.clone(),
            name(index),
            if index % 2 == 0 { "Kick" } else { "Snare" }.to_string(),
            pack,
            vec!["Loop".to_string()],
            Some(120.0),
            Some("C minor".to_string()),
            Some(1.0),
        )
    })
}

fn search(library: &Library, value: Value) -> SearchResponse {
    library.search(serde_json::from_value(value).unwrap()).unwrap()
}

/// Resident memory of this process in MiB (Linux), or 0.
fn resident_mib() -> f64 {
    std::fs::read_to_string("/proc/self/statm").ok().and_then(|text| text.split_whitespace().nth(1).and_then(|pages| pages.parse::<f64>().ok())).map(|pages| pages * 4096.0 / (1024.0 * 1024.0)).unwrap_or(0.0)
}

#[test]
fn a_large_index_stays_interactive() {
    let temp = TempDir::new("stress");
    let library = Arc::new(Library::open(LibraryConfig::isolated(temp.join("Cache"), vec![temp.join("synthetic")])).unwrap());
    let before = resident_mib();
    let begin = Instant::now();
    library.load_synthetic(synthetic(COUNT, temp.text("synthetic")));
    let build = begin.elapsed();
    let memory = resident_mib() - before;

    let begin = Instant::now();
    let broad = search(&library, json!({ "limit": 150 }));
    let broad_time = begin.elapsed();
    assert_eq!((broad.total, broad.items.len()), (COUNT, 150));
    assert_eq!(broad.items[0].name, name(0));
    assert_eq!(broad.facets["packs"].len(), 100);

    let begin = Instant::now();
    let paged = search(&library, json!({ "limit": 150, "offset": COUNT / 2 }));
    let page_time = begin.elapsed();
    assert_eq!(paged.items.len(), 150);
    assert_eq!((paged.items[0].name.as_str(), paged.items[149].name.as_str()), (name(COUNT / 2).as_str(), name(COUNT / 2 + 149).as_str()), "partial selection gives the same page as a full sort");

    let begin = Instant::now();
    let filtered = search(&library, json!({ "query": "tag:loop pack:Pack-17 bpm:120 key:Cminor" }));
    let filter_time = begin.elapsed();
    assert_eq!(filtered.total, (COUNT + 82) / 100, "normalized filters");

    let begin = Instant::now();
    let text = search(&library, json!({ "query": "sample 0001", "limit": 150 }));
    let text_time = begin.elapsed();
    assert_eq!(text.total, (0..COUNT).filter(|index| name(*index).contains("0001")).count(), "every name containing 0001");

    let begin = Instant::now();
    let structured = search(&library, json!({ "filters": { "categories": ["Kick"], "packs": ["Pack-2", "Pack-4"], "bpm": { "min": 118, "max": 122 }, "keys": ["C minor"], "sampleTypes": ["unknown", "loop"] }, "limit": 150 }));
    let structured_time = begin.elapsed();
    assert_eq!(structured.total, COUNT / 50);
    assert_eq!(structured.facets["categories"].iter().map(|facet| (facet.value.as_str(), facet.count)).collect::<Vec<_>>(), [("Kick", COUNT / 50), ("Snare", 0)].into_iter().filter(|(_, count)| *count > 0).collect::<Vec<_>>());

    // A burst of searches: one computes at a time, superseded ones return at once.
    let barrier = Arc::new(Barrier::new(33));
    let threads: Vec<_> = (0..32)
        .map(|index| {
            let (library, barrier) = (library.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                library.search(serde_json::from_value(json!({ "query": name(COUNT - 1 - index) })).unwrap()).unwrap()
            })
        })
        .collect();
    let begin = Instant::now();
    barrier.wait();
    let results: Vec<SearchResponse> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    let burst_time = begin.elapsed();
    let completed = results.iter().filter(|result| result.cancelled.is_none()).count();
    assert!(completed > 0 && results.iter().filter(|result| result.cancelled.is_none()).all(|result| result.total == 1), "every search that was not superseded found its sample");
    assert_eq!(library.peak_concurrent_searches(), 1, "searches never compute concurrently");
    // The last request issued is never the one that is cancelled.
    assert_eq!(search(&library, json!({ "query": name(7) })).total, 1);

    // The cache file: size, and the time to write and to load it.
    let begin = Instant::now();
    library.flush_cache();
    let write_time = begin.elapsed();
    let size = std::fs::metadata(temp.join("Cache/index.bin")).unwrap().len();
    drop(results);
    let library = Arc::try_unwrap(library).ok().expect("the burst threads have finished");
    drop(library);
    let begin = Instant::now();
    let restored = Library::open(LibraryConfig::isolated(temp.join("Cache"), vec![temp.join("synthetic")])).unwrap();
    let load_time = begin.elapsed();
    assert_eq!(restored.status(Empty).unwrap().count, COUNT);
    assert_eq!(search(&restored, json!({ "limit": 1, "offset": COUNT - 1 })).items[0].name, name(COUNT - 1));

    println!(
        "st-library stress: {COUNT} items | build {:.0} ms | memory +{memory:.0} MiB | broad {:.1} ms | deep page {:.1} ms | query filters {:.1} ms | text {:.1} ms | structured filters {:.1} ms | burst of 32: {} computed, {} superseded in {:.1} ms | cache {:.1} MiB, write {:.0} ms, load {:.0} ms",
        build.as_secs_f64() * 1000.0,
        broad_time.as_secs_f64() * 1000.0,
        page_time.as_secs_f64() * 1000.0,
        filter_time.as_secs_f64() * 1000.0,
        text_time.as_secs_f64() * 1000.0,
        structured_time.as_secs_f64() * 1000.0,
        completed,
        32 - completed,
        burst_time.as_secs_f64() * 1000.0,
        size as f64 / (1024.0 * 1024.0),
        write_time.as_secs_f64() * 1000.0,
        load_time.as_secs_f64() * 1000.0,
    );
}
