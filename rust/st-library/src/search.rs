//! Searching the index: free text with `field:value` filters, structured multi-select filters,
//! disjunctive facet counts, ranking and paging.
//!
//! A search is a pure function of a [`Snapshot`] of the index. The filtered population and its
//! facet counts are cached per query so that further pages of the same query only select and
//! sort their rows.

use crate::api::{DurationCoverage, Facet, LibraryItem, NumericFilter, OfflineRoot, RootReport, SearchRequest, SearchResponse};
use crate::index::{Field, Item, Kind, Root, Tables};
use crate::infer::{key_slot, COLLECTION_NAMES};
use crate::util::{extension, file_name, fold, parent, volume_name};
use memchr::memmem::Finder;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// The state of the index a search works on.
pub(crate) struct Snapshot {
    pub items: Arc<Vec<Arc<Item>>>,
    pub favorites: Arc<HashSet<String>>,
    pub scanning: bool,
    pub errors: Vec<String>,
    pub revision: u64,
    pub reports: Vec<RootReport>,
    pub verified_expansions: HashSet<String>,
    pub queued_durations: HashSet<String>,
    pub roots: Vec<Root>,
    pub offline: BTreeSet<String>,
}

/// A parsed query: words that must all match, and `field:value` filters.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Query {
    pub words: Vec<String>,
    pub filters: Vec<(String, String)>,
}

impl Query {
    pub fn parse(query: &str) -> Query {
        let mut result = Query::default();
        let characters: Vec<char> = query.chars().collect();
        let mut at = 0;
        while at < characters.len() {
            if characters[at].is_whitespace() {
                at += 1;
                continue;
            }
            // A token: runs of non-space characters and quoted sections, joined.
            let mut token = String::new();
            let start = at;
            while at < characters.len() && !characters[at].is_whitespace() {
                if characters[at] == '"' {
                    match characters[at + 1..].iter().position(|c| *c == '"') {
                        Some(length) => {
                            token.extend(&characters[at + 1..at + 1 + length]);
                            at += length + 2;
                        }
                        None => break, // An unclosed quote ends the token; the quote itself is skipped.
                    }
                } else {
                    token.push(characters[at]);
                    at += 1;
                }
            }
            if at == start {
                at += 1;
            }
            let value = fold(&token);
            match value.split_once(':').filter(|(field, _)| ["tag", "pack", "bpm", "key", "ext", "kind", "category"].contains(field)) {
                Some((field, argument)) => {
                    let argument = if field == "key" { key_slot(argument) } else { argument.to_string() };
                    result.filters.push((field.to_string(), argument));
                }
                None if !value.is_empty() => result.words.push(value),
                None => {}
            }
        }
        result
    }
}

fn numeric_active(filter: &NumericFilter) -> bool {
    bound(filter.min).is_some() || bound(filter.max).is_some() || filter.include_unknown
}

fn bound(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite()).map(|value| value.max(0.0))
}

fn numeric_matches(filter: &NumericFilter, value: Option<f64>) -> bool {
    if !numeric_active(filter) {
        return true;
    }
    let Some(value) = value else { return filter.include_unknown };
    let (min, max) = (bound(filter.min), bound(filter.max));
    if min.is_none() && max.is_none() {
        return false; // An unknown-only selection.
    }
    min.is_none_or(|min| value >= min) && max.is_none_or(|max| value <= max)
}

/// Facets in result order. The first eight take value selections; `bpm` and `duration` are
/// known/unknown counts of the numeric filters.
pub(crate) const FACET_NAMES: [&str; 10] = ["categories", "collections", "roots", "packs", "keys", "sampleTypes", "extensions", "tags", "bpm", "duration"];
const SLOT_FIELDS: [(usize, Field); 7] =
    [(0, Field::Category), (2, Field::Root), (3, Field::Pack), (4, Field::Key), (5, Field::SampleType), (6, Field::Extension), (7, Field::Tag)];

/// The request's structured filters, folded for comparison.
struct Selections {
    values: [BTreeSet<String>; 8],
    active: bool,
    cache_key: String,
}

impl Selections {
    fn new(request: &SearchRequest) -> Selections {
        let filters = &request.filters;
        let folded = |values: &[String]| -> BTreeSet<String> {
            values.iter().take(128).map(|value| fold(&value.chars().take(2048).collect::<String>())).filter(|value| !value.is_empty()).collect()
        };
        let mut values = [
            folded(&filters.categories),
            folded(&filters.collections),
            folded(&filters.roots),
            folded(&filters.packs),
            folded(&filters.keys),
            folded(&filters.sample_types),
            folded(&filters.extensions),
            folded(&filters.tags),
        ];
        values[1].extend(folded(&filters.sources));
        let single = |value: &Option<String>| value.as_deref().filter(|value| !value.is_empty() && *value != "all").map(fold);
        if values[0].is_empty() {
            values[0].extend(single(&request.category));
        }
        if values[3].is_empty() {
            values[3].extend(single(&request.pack));
        }
        values[4] = values[4].iter().map(|key| if key == "unknown" { key.clone() } else { key_slot(key) }).collect();
        let active = values.iter().any(|set| !set.is_empty()) || numeric_active(&filters.bpm) || numeric_active(&filters.duration);
        let numeric = |filter: &NumericFilter| format!("{:?}/{:?}/{}", bound(filter.min), bound(filter.max), filter.include_unknown);
        let cache_key = format!("{:?}{};{}", values, numeric(&filters.bpm), numeric(&filters.duration));
        Selections { values, active, cache_key }
    }
}

/// The filtered population of one query at one index revision.
pub(crate) struct Population {
    key: String,
    revision: u64,
    /// The snapshot `matching` indexes into.
    items: Arc<Vec<Arc<Item>>>,
    matching: Vec<Ranked>,
    facet_counts: Vec<HashMap<Arc<str>, usize>>,
    duration_known: usize,
    duration_unknown: usize,
    eligible_pending: HashSet<String>,
    duration_candidates: Vec<String>,
    offline_matches: HashMap<u32, usize>,
}

/// What the previous searches left for the next one.
#[derive(Default)]
pub(crate) struct SearchCache {
    last_key: String,
    last_revision: u64,
    last_result: Option<SearchResponse>,
    pub population: Option<Population>,
}

/// A finished search.
pub(crate) struct Outcome {
    pub response: SearchResponse,
    /// Samples whose length the response reports as pending: their headers should be read.
    pub duration_candidates: Vec<String>,
}

/// A matching item: its position in the snapshot, its score, and the first sixteen bytes of
/// its folded name so that most comparisons need not follow the pointer to the item.
#[derive(Clone, Copy)]
struct Ranked {
    index: u32,
    score: u32,
    prefix: [u64; 2],
}

fn name_prefix(name: &[u8]) -> [u64; 2] {
    let mut bytes = [0u8; 16];
    let length = name.len().min(16);
    bytes[..length].copy_from_slice(&name[..length]);
    [u64::from_be_bytes(bytes[..8].try_into().unwrap_or([0; 8])), u64::from_be_bytes(bytes[8..].try_into().unwrap_or([0; 8]))]
}

/// Best score first, then by folded name, then by path.
fn ranks_before(items: &[Arc<Item>], left: &Ranked, right: &Ranked) -> std::cmp::Ordering {
    right.score.cmp(&left.score).then_with(|| left.prefix.cmp(&right.prefix)).then_with(|| {
        let (a, b) = (&items[left.index as usize], &items[right.index as usize]);
        a.folded_name().cmp(b.folded_name()).then_with(|| a.path.cmp(&b.path))
    })
}

/// The row of `item` as the page receives it.
pub(crate) fn item_row(item: &Arc<Item>, tables: &Tables, favorites: &HashSet<String>, verified: &HashSet<u32>, pending: &HashSet<String>) -> LibraryItem {
    let path = item.path.to_string();
    let extension = extension(&path);
    let collection = COLLECTION_NAMES[collection_index(item, verified)].to_string();
    let pack = tables.value(Field::Pack, item.pack).to_string();
    let category = tables.value(Field::Category, item.category).to_string();
    let kit = item.kind == Kind::Kit;
    LibraryItem {
        id: path.clone(),
        name: item.name.to_string(),
        kind: item.kind.text().to_string(),
        tags: item.tags.iter().map(|tag| tables.value(Field::Tag, *tag).to_string()).collect(),
        favorite: favorites.contains(&path),
        size: item.size,
        folder: parent(&path).to_string(),
        metadata_source: tables.value(Field::Source, item.source).to_string(),
        root: tables.value(Field::Root, item.root).to_string(),
        sample_type: tables.value(Field::SampleType, item.sample_type).to_string(),
        duration_status: if item.duration.is_some() { "known" } else if pending.contains(&path) { "pending" } else { "unknown" }.to_string(),
        user_metadata: item.user.as_ref().map(|labels| labels.to_value()).unwrap_or_else(|| serde_json::json!({})),
        inferred_metadata: item.inferred.as_deref().unwrap_or(item).editable(tables),
        bpm: item.bpm,
        key: item.has_key.then(|| tables.value(Field::Key, item.key).to_string()),
        preview_path: item.preview.as_deref().map(String::from),
        duration: item.duration,
        duration_source: item.duration.map(|_| "audio header".to_string()),
        kit_format: kit.then(|| if extension == "stkit" { "supertracker-kit" } else { "maschine-group" }.to_string()),
        source: kit.then(|| collection.clone()),
        expansion: kit.then(|| pack.clone()),
        kit_type: kit.then(|| category.clone()),
        user: kit.then_some(extension == "stkit"),
        sound_count: item.kit_sound_count.filter(|_| kit),
        metadata_error: item.kit_error.as_deref().filter(|_| kit).map(String::from),
        available: None,
        volume: None,
        path,
        extension,
        collection,
        pack,
        category,
    }
}

fn collection_index(item: &Item, verified: &HashSet<u32>) -> usize {
    if item.collection_root.is_some_and(|root| verified.contains(&root)) {
        1
    } else {
        item.base_collection as usize
    }
}

/// Which slots of a table contain `needle` in their folded value.
fn slots_containing(folded: &[Arc<str>], needle: &str) -> Vec<bool> {
    folded.iter().map(|value| value.contains(needle)).collect()
}

struct WordMatcher {
    bytes: Vec<u8>,
    finder: Finder<'static>,
    categories: Vec<bool>,
    packs: Vec<bool>,
    tags: Vec<bool>,
    keys: Vec<bool>,
}

enum TextFilter {
    Pack(Vec<bool>),
    Tag(Vec<bool>),
    Category(Vec<bool>),
    Kind(String),
    Key(String),
    Extension(u32),
    NoMatch,
    Tempo(Option<f64>, Option<f64>),
}

fn hit(slots: &[bool], id: u32) -> bool {
    slots.get(id as usize).copied().unwrap_or(false)
}

/// Runs `request` against `snapshot`. `None` when `cancelled` reported a newer request.
pub(crate) fn search(request: &SearchRequest, snapshot: &Snapshot, tables: &Tables, cache: &mut SearchCache, cancelled: &dyn Fn() -> bool) -> Option<Outcome> {
    if cancelled() {
        return None;
    }
    let query_text: String = request.query.chars().take(1024).collect();
    let kind = if request.kind.is_empty() { "all" } else { request.kind.as_str() };
    let selections = Selections::new(request);
    let (offset, limit) = crate::api::page(request.offset, request.limit);
    let population_key = format!("{}\u{1f}{kind}\u{1f}{}\u{1f}{}\u{1f}{}", request.query, selections.cache_key, request.favorites, request.hide_offline);
    let cache_key = format!("{population_key}\u{1f}{offset}\u{1f}{limit}");
    if cache_key == cache.last_key && snapshot.revision == cache.last_revision {
        if let Some(mut cached) = cache.last_result.clone() {
            cached.scanning = snapshot.scanning;
            cached.errors = Some(snapshot.errors.clone());
            cached.root_reports = Some(snapshot.reports.clone());
            return Some(Outcome { response: cached, duration_candidates: Vec::new() });
        }
    }
    let verified: HashSet<u32> = snapshot.verified_expansions.iter().map(|root| tables.id(Field::CollectionRoot, root)).collect();
    let offline_ids: HashSet<u32> = snapshot.offline.iter().map(|root| tables.id(Field::Root, root)).collect();
    let duration_filter = numeric_active(&request.filters.duration);

    let reusable = cache.population.as_ref().is_some_and(|population| population.key == population_key && population.revision == snapshot.revision);
    if !reusable {
        let query = Query::parse(&query_text);
        let (categories, packs, tags, keys) = (tables.snapshot(Field::Category).1, tables.snapshot(Field::Pack).1, tables.snapshot(Field::Tag).1, tables.snapshot(Field::Key).1);
        let words: Vec<WordMatcher> = query
            .words
            .iter()
            .map(|word| WordMatcher {
                bytes: word.as_bytes().to_vec(),
                finder: Finder::new(word.as_bytes()).into_owned(),
                categories: slots_containing(&categories, word),
                packs: slots_containing(&packs, word),
                tags: slots_containing(&tags, word),
                keys: keys.iter().map(|key| &**key != "unknown" && key.contains(word.as_str())).collect(),
            })
            .collect();
        let text_filters: Vec<TextFilter> = query
            .filters
            .iter()
            .map(|(field, value)| match field.as_str() {
                "pack" => TextFilter::Pack(slots_containing(&packs, value)),
                "tag" => TextFilter::Tag(slots_containing(&tags, value)),
                "category" => TextFilter::Category(slots_containing(&categories, value)),
                "kind" => TextFilter::Kind(value.clone()),
                "key" => TextFilter::Key(value.clone()),
                "ext" => TextFilter::Extension(tables.id(Field::Extension, value.trim_matches('.'))),
                "bpm" => {
                    let bounds: Vec<f64> = value.split('-').filter(|part| !part.is_empty()).filter_map(|part| part.parse().ok()).collect();
                    match bounds.as_slice() {
                        [low, high] => TextFilter::Tempo(Some(*low), Some(*high)),
                        [exact, ..] => TextFilter::Tempo(Some(*exact), None),
                        [] => TextFilter::NoMatch,
                    }
                }
                _ => TextFilter::NoMatch,
            })
            .collect();
        let key_folded = keys;
        // Selected slots per facet: resolved once instead of comparing strings per item.
        let mut slot_sets: [Option<Vec<bool>>; 8] = Default::default();
        let mut slot_counts: [Vec<usize>; 8] = Default::default();
        let mut names: [Vec<Arc<str>>; 8] = Default::default();
        for (index, field) in SLOT_FIELDS {
            let (values, folded) = tables.snapshot(field);
            if !selections.values[index].is_empty() {
                slot_sets[index] = Some(folded.iter().map(|value| selections.values[index].contains(&**value)).collect());
            }
            slot_counts[index] = vec![0; values.len()];
            names[index] = values;
        }
        let folded_collections: Vec<String> = COLLECTION_NAMES.iter().map(|name| fold(name)).collect();
        let (mut collection_counts, mut bpm_counts, mut duration_counts) = ([0usize; 4], [0usize; 2], [0usize; 2]);
        let (mut duration_known, mut duration_unknown) = (0, 0);
        let mut eligible_pending: HashSet<String> = HashSet::new();
        let mut duration_candidates: Vec<String> = Vec::new();
        let mut offline_matches: HashMap<u32, usize> = HashMap::new();
        let mut matching: Vec<Ranked> = Vec::with_capacity(snapshot.items.len().min(4096));
        let bump = |counts: &mut Vec<usize>, id: u32| {
            if counts.len() <= id as usize {
                counts.resize(id as usize + 1, 0);
            }
            counts[id as usize] += 1;
        };
        let ids = |item: &Item| -> [u32; 8] { [item.category, 0, item.root, item.pack, item.key, item.sample_type, item.extension, 0] };

        for (index, item) in snapshot.items.iter().enumerate() {
            if index % 4096 == 0 && cancelled() {
                return None;
            }
            // The All view lists samples and presets; kits have their own view and are never
            // offered as playable samples.
            let kind_matches = if kind == "all" { item.kind != Kind::Kit } else { item.kind.text() == kind };
            if !kind_matches || (request.favorites && !snapshot.favorites.contains(&*item.path)) {
                continue;
            }
            let word_matches = words.iter().all(|word| {
                word.finder.find(&item.folded).is_some()
                    || hit(&word.categories, item.category)
                    || hit(&word.packs, item.pack)
                    || (item.has_key && hit(&word.keys, item.key))
                    || item.tags.iter().any(|tag| hit(&word.tags, *tag))
            });
            if !word_matches {
                continue;
            }
            let filters_match = text_filters.iter().all(|filter| match filter {
                TextFilter::Pack(slots) => hit(slots, item.pack),
                TextFilter::Tag(slots) => item.tags.iter().any(|tag| hit(slots, *tag)),
                TextFilter::Category(slots) => hit(slots, item.category),
                TextFilter::Kind(value) => item.kind.text() == value || (value == "preset" && item.kind == Kind::Nks),
                TextFilter::Key(value) => item.has_key && key_folded.get(item.key as usize).is_some_and(|key| **key == **value),
                TextFilter::Extension(id) => item.extension == *id,
                TextFilter::Tempo(low, high) => match (item.bpm, low, high) {
                    (Some(bpm), Some(low), Some(high)) => bpm >= *low && bpm <= *high,
                    (Some(bpm), Some(exact), None) => (bpm - exact).abs() < 0.5,
                    _ => false,
                },
                TextFilter::NoMatch => false,
            });
            if !filters_match {
                continue;
            }
            let collection = collection_index(item, &verified);
            let item_ids = ids(item);
            let mut failures = 0u32;
            if selections.active {
                for (field, _) in SLOT_FIELDS.iter().take(6) {
                    if slot_sets[*field].as_ref().is_some_and(|set| !hit(set, item_ids[*field])) {
                        failures |= 1 << field;
                    }
                }
                if !selections.values[1].is_empty() && !selections.values[1].contains(&folded_collections[collection]) {
                    failures |= 1 << 1;
                }
                if slot_sets[7].as_ref().is_some_and(|set| !item.tags.iter().any(|tag| hit(set, *tag))) {
                    failures |= 1 << 7;
                }
                if !numeric_matches(&request.filters.bpm, item.bpm) {
                    failures |= 1 << 8;
                }
                if !numeric_matches(&request.filters.duration, item.duration) {
                    failures |= 1 << 9;
                }
            }
            let offline = !offline_ids.is_empty() && offline_ids.contains(&item.root);
            if offline {
                if failures == 0 {
                    *offline_matches.entry(item.root).or_insert(0) += 1;
                }
                if request.hide_offline {
                    continue;
                }
            }
            // Disjunctive facets: an option is counted when every filter but its own matches.
            if failures.count_ones() <= 1 {
                for (field, _) in SLOT_FIELDS.iter().take(6) {
                    if failures & !(1 << field) == 0 {
                        bump(&mut slot_counts[*field], item_ids[*field]);
                    }
                }
                if failures & !(1 << 1) == 0 {
                    collection_counts[collection] += 1;
                }
                if failures & !(1 << 7) == 0 {
                    for tag in item.tags.iter() {
                        bump(&mut slot_counts[7], *tag);
                    }
                }
                if failures & !(1 << 8) == 0 {
                    bpm_counts[usize::from(item.bpm.is_none())] += 1;
                }
                if failures & !(1 << 9) == 0 {
                    duration_counts[usize::from(item.duration.is_none())] += 1;
                }
            }
            if failures & !(1 << 9) == 0 {
                if item.duration.is_some() {
                    duration_known += 1;
                } else {
                    duration_unknown += 1;
                }
                let queued = !snapshot.queued_durations.is_empty() && snapshot.queued_durations.contains(&*item.path);
                if queued {
                    eligible_pending.insert(item.path.to_string());
                }
                if duration_filter && item.kind == Kind::Sample && !item.duration_checked && !queued && duration_candidates.len() < 32 && !offline {
                    duration_candidates.push(item.path.to_string());
                }
            }
            if failures != 0 {
                continue;
            }
            let name = item.folded_name();
            let score = words.iter().map(|word| if name.starts_with(&word.bytes) { 2 } else if word.finder.find(name).is_some() { 1 } else { 0 }).sum();
            matching.push(Ranked { index: index as u32, score, prefix: name_prefix(name) });
        }
        if cancelled() {
            return None;
        }
        let mut facet_counts: Vec<HashMap<Arc<str>, usize>> = vec![HashMap::new(); FACET_NAMES.len()];
        for (field, _) in SLOT_FIELDS {
            for (slot, count) in slot_counts[field].iter().enumerate() {
                if *count > 0 {
                    if let Some(name) = names[field].get(slot) {
                        *facet_counts[field].entry(name.clone()).or_insert(0) += count;
                    }
                }
            }
        }
        for (index, count) in collection_counts.iter().enumerate() {
            if *count > 0 {
                facet_counts[1].insert(Arc::from(COLLECTION_NAMES[index]), *count);
            }
        }
        for (index, name) in ["known", "unknown"].iter().enumerate() {
            if bpm_counts[index] > 0 {
                facet_counts[8].insert(Arc::from(*name), bpm_counts[index]);
            }
            if duration_counts[index] > 0 {
                facet_counts[9].insert(Arc::from(*name), duration_counts[index]);
            }
        }
        cache.population = Some(Population {
            key: population_key,
            revision: snapshot.revision,
            items: snapshot.items.clone(),
            matching,
            facet_counts,
            duration_known,
            duration_unknown,
            eligible_pending,
            duration_candidates,
            offline_matches,
        });
    }
    let population = cache.population.as_mut()?;
    let total = population.matching.len();
    let (start, end) = (offset.min(total), (offset.min(total) + limit).min(total));
    // Select the page in linear time; only the visible rows are sorted.
    let items = population.items.clone();
    let order = |left: &Ranked, right: &Ranked| ranks_before(&items, left, right);
    if end < total {
        population.matching.select_nth_unstable_by(end, order);
    }
    if start > 0 && start < end {
        population.matching[..end].select_nth_unstable_by(start, order);
    }
    let mut ranked: Vec<Ranked> = population.matching[start..end].to_vec();
    ranked.sort_by(order);
    let visible: Vec<&Arc<Item>> = ranked.iter().map(|ranked| &items[ranked.index as usize]).collect();

    let mut duration_candidates: Vec<String> = if duration_filter {
        population.duration_candidates.iter().filter(|path| !snapshot.queued_durations.contains(*path)).cloned().collect()
    } else {
        visible
            .iter()
            .filter(|item| item.kind == Kind::Sample && !item.duration_checked && !snapshot.queued_durations.contains(&*item.path) && !offline_ids.contains(&item.root))
            .take(32)
            .map(|item| item.path.to_string())
            .collect()
    };
    duration_candidates.truncate(32);
    let duration_pending = population.eligible_pending.iter().filter(|path| snapshot.queued_durations.contains(*path)).count();
    let pending: HashSet<String> = snapshot.queued_durations.iter().cloned().chain(duration_candidates.iter().cloned()).collect();
    let rows: Vec<LibraryItem> = visible
        .iter()
        .map(|item| {
            let mut row = item_row(item, tables, &snapshot.favorites, &verified, &pending);
            if offline_ids.contains(&item.root) {
                row.available = Some(false);
                row.volume = Some(volume_name(&row.root).unwrap_or_default());
            }
            row
        })
        .collect();

    let root_labels: HashMap<&str, &str> = snapshot.roots.iter().map(|root| (root.path.as_str(), root.name.as_str())).collect();
    let mut facets: BTreeMap<String, Vec<Facet>> = BTreeMap::new();
    let mut truncated = Vec::new();
    for (field, name) in FACET_NAMES.iter().enumerate() {
        let counts = &population.facet_counts[field];
        // Bound the payload even when user tags hold thousands of distinct values.
        let large = counts.len() > 1000;
        if large {
            truncated.push(name.to_string());
        }
        let mut keyed: Vec<(&Arc<str>, usize, bool)> = counts
            .iter()
            .map(|(value, count)| {
                let selected = field < 8 && !selections.values[field].is_empty() && {
                    let folded = if field == 4 && &**value != "unknown" { key_slot(value) } else { fold(value) };
                    selections.values[field].contains(&folded)
                };
                (value, *count, selected)
            })
            .collect();
        keyed.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| if large { right.1.cmp(&left.1) } else { std::cmp::Ordering::Equal }).then_with(|| left.0.cmp(right.0)));
        let values = keyed
            .into_iter()
            .take(1000)
            .map(|(value, count, _)| {
                let label = if field == 2 { root_labels.get(&**value).copied().unwrap_or(value).to_string() } else if &**value == "unknown" { "Unknown".to_string() } else { value.to_string() };
                Facet { value: value.to_string(), name: value.to_string(), label, count }
            })
            .collect();
        facets.insert(name.to_string(), values);
    }
    facets.insert("sources".to_string(), facets.get("collections").cloned().unwrap_or_default());
    let offline = snapshot
        .offline
        .iter()
        .map(|path| OfflineRoot {
            path: path.clone(),
            name: root_labels.get(path.as_str()).map(|name| name.to_string()).unwrap_or_else(|| file_name(path).to_string()),
            volume: volume_name(path).unwrap_or_default(),
            matching: population.offline_matches.get(&tables.id(Field::Root, path)).copied().unwrap_or(0),
        })
        .collect();
    let response = SearchResponse {
        items: rows,
        total,
        scanning: snapshot.scanning,
        cancelled: None,
        categories: facets.get("categories").cloned().unwrap_or_default(),
        packs: facets.get("packs").cloned().unwrap_or_default(),
        facets,
        facets_truncated: Some(truncated),
        duration_coverage: Some(DurationCoverage { known: population.duration_known, unknown: population.duration_unknown, pending: duration_pending + duration_candidates.len() }),
        offset: Some(offset),
        limit: Some(limit),
        errors: Some(snapshot.errors.clone()),
        root_reports: Some(snapshot.reports.clone()),
        offline: Some(offline),
        hide_offline: Some(request.hide_offline),
    };
    if cancelled() {
        return None;
    }
    population.eligible_pending.extend(duration_candidates.iter().cloned());
    cache.last_key = cache_key;
    cache.last_revision = snapshot.revision;
    cache.last_result = Some(response.clone());
    Some(Outcome { response, duration_candidates })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_split_into_words_and_filters() {
        let phrase = Query::parse("pack:\"Orbit Pack\" tag:loop");
        assert_eq!(phrase.filters, [("pack".to_string(), "orbit pack".to_string()), ("tag".to_string(), "loop".to_string())]);
        assert!(phrase.words.is_empty());
        let mixed = Query::parse("  Kick  bpm:120 key:Bbm \"dark pad\" other:thing ext:.WAV ");
        assert_eq!(mixed.words, ["kick", "dark pad", "other:thing"]);
        assert_eq!(mixed.filters, [("bpm".to_string(), "120".to_string()), ("key".to_string(), "a#minor".to_string()), ("ext".to_string(), ".wav".to_string())]);
        assert_eq!(Query::parse("\"unclosed kick").words, ["unclosed", "kick"]);
        assert_eq!(Query::parse("").words.len(), 0);
    }

    #[test]
    fn numeric_filters() {
        let filter = |min: Option<f64>, max: Option<f64>, include_unknown: bool| NumericFilter { min, max, include_unknown };
        assert!(numeric_matches(&filter(None, None, false), None), "an inactive filter matches everything");
        assert!(numeric_matches(&filter(Some(110.0), Some(129.0), false), Some(120.0)) && !numeric_matches(&filter(Some(110.0), Some(129.0), false), Some(130.0)));
        assert!(!numeric_matches(&filter(Some(110.0), None, false), None) && numeric_matches(&filter(Some(110.0), None, true), None));
        assert!(!numeric_matches(&filter(None, None, true), Some(1.0)), "unknown-only excludes known values");
        assert!(numeric_matches(&filter(Some(f64::NAN), Some(-5.0), false), Some(0.0)), "non-finite bounds are ignored; negative ones clamp to zero");
    }
}
