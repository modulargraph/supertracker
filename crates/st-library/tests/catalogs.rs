//! The NKS preset catalogue, product artwork, product registrations, and the kit index,
//! kit browser and kit commands of the library service.
//! Ported from `NKSMetadata.selfTest` (catalogue part), `NKSArtwork.selfTest` and
//! `tests/KitLibraryIndexTests.swift`.

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_library::api::*;
use st_library::kit::catalog::KitsBrowseResponse;
use st_library::nks::catalog::{discover_registrations, plist_string, posix_path, Extra};
use st_library::nks::fixtures::preset;
use st_library::nks::{ArtMatch, NksArtwork, NksBrowseResponse, NksCatalog};
use st_library::{Library, LibraryConfig};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn nks(name: &str, product: &str, bank: &str, device: &str, types: Value, characters: &[&str]) -> Vec<u8> {
    let summary = json!({ "name": name, "vendor": "Fixture Audio", "deviceType": device, "bankchain": [product, bank, ""], "types": types, "characters": characters });
    preset(&summary, Some(&json!({ "VST.magic": 0x4b61_7431u32 })), None, Some(&[1]))
}

fn browse(catalog: &NksCatalog, request: Value, favorites: &[&str]) -> NksBrowseResponse {
    let favorites: HashSet<String> = favorites.iter().map(|path| path.to_string()).collect();
    catalog.browse(&serde_json::from_value(request).unwrap(), &favorites)
}

fn names(items: &[Value]) -> Vec<String> {
    items.iter().map(|item| item["name"].as_str().unwrap().to_string()).collect()
}

fn facet_names(facets: &[st_library::nks::BrowseFacet]) -> Vec<String> {
    facets.iter().map(|facet| facet.label.clone().unwrap_or_else(|| facet.name.clone())).collect()
}

fn no_artwork(temp: &TempDir) -> Arc<NksArtwork> {
    Arc::new(NksArtwork::new(&temp.join("no-artwork")))
}

#[test]
fn nks_catalogue_products_banks_types_and_cache() {
    let temp = TempDir::new("nks-catalogue");
    let folder = "Library/Synth One/Presets";
    let alpha = temp.write(&format!("{folder}/Alpha.nksf"), &nks("Alpha Pad", "Synth One", "Bank A", "INST", json!([["Synth Pad"], ["Synth Pad", "Warm"]]), &["Warm", "Analog"]));
    let beta = temp.write(&format!("{folder}/Beta.nksf"), &nks("Beta Bass", "Synth One", "Bank B", "INST", json!([["Bass"]]), &["Analog"]));
    let echo = temp.write(&format!("{folder}/Echo.nksfx"), &nks("Tape Echo", "Echo FX", "", "FX", json!([["Delay", "Tape"]]), &[]));
    let preview = temp.write(&format!("{folder}/.previews/Alpha.nksf.ogg"), &[0]);
    temp.write(&format!("{folder}/Samples/Ignored.nksf"), &nks("Ignored", "X", "", "INST", json!([]), &[]));
    temp.write(&format!("{folder}/Broken.nksf"), b"not a preset");
    let events: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
    let catalog = NksCatalog::new(&temp.join("Cache"), false, &[temp.text("Library")], no_artwork(&temp));
    let sink = events.clone();
    catalog.set_event_sink(Arc::new(move |name: &str, payload: Value| sink.lock().unwrap().push((name.to_string(), payload))));
    catalog.refresh_now();
    {
        let events = events.lock().unwrap();
        assert!(events.iter().all(|(name, _)| name == "library.nks.progress"));
        assert_eq!((events.first().unwrap().1["indexing"].as_bool(), events.last().unwrap().1["indexing"].as_bool(), events.last().unwrap().1["count"].as_u64()), (Some(true), Some(false), Some(3)));
    }

    let instruments = browse(&catalog, json!({ "kind": "instrument" }), &[]);
    assert_eq!(names(&instruments.items), ["Alpha Pad", "Beta Bass"]);
    assert_eq!((facet_names(&instruments.products), instruments.counts.effect, instruments.counts.instrument, instruments.total), (vec!["Synth One".to_string()], 1, 2, 2));
    assert_eq!(instruments.items[0]["previewPath"], preview.as_str());
    assert_eq!(instruments.items[0], json!({
        "path": alpha, "id": alpha, "kind": "nks", "name": "Alpha Pad", "product": "Synth One", "vendor": "Fixture Audio", "bank": "Bank A", "subBank": "", "effect": false,
        "types": ["Synth Pad"], "subtypes": ["Synth Pad\u{1f}Warm"], "characters": ["Warm", "Analog"], "author": "", "comment": "", "format": "VST2", "hasControls": false,
        "favorite": false, "extension": "nksf", "previewPath": preview
    }));
    assert_eq!((instruments.status.count, instruments.status.indexing, instruments.status.roots, instruments.status.error.clone()), (3, false, 1, None));
    let effects = browse(&catalog, json!({ "kind": "effect" }), &[]);
    assert_eq!((names(&effects.items), facet_names(&effects.subtypes)), (vec!["Tape Echo".to_string()], vec!["Tape".to_string()]));
    assert_eq!(effects.subtypes[0].parent.as_deref(), Some("Delay"));
    let product = browse(&catalog, json!({ "kind": "instrument", "product": "Synth One" }), &[]);
    assert_eq!(facet_names(&product.banks), ["Bank A", "Bank B"]);
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "product": "Synth One", "bank": "Bank B" }), &[]).items), ["Beta Bass"]);
    let typed = browse(&catalog, json!({ "kind": "instrument", "types": ["Bass"] }), &[]);
    assert_eq!((names(&typed.items), facet_names(&typed.types)), (vec!["Beta Bass".to_string()], vec!["Bass".to_string(), "Synth Pad".to_string()]), "a type filter keeps sibling types");
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "characters": ["Warm", "Analog"] }), &[]).items), ["Alpha Pad"], "characters match all");
    assert_eq!(facet_names(&instruments.characters), ["Analog", "Warm"], "characters by count");
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "query": "beta" }), &[]).items), ["Beta Bass"]);
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "favorites": true }), &[&beta]).items), ["Beta Bass"]);
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "offset": 1, "limit": 1 }), &[]).items), ["Beta Bass"]);

    fs::remove_file(&beta).unwrap();
    catalog.refresh_now();
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument" }), &[]).items), ["Alpha Pad"], "deleted presets leave the catalogue");
    // The cache restores the catalogue for a session whose folder is not reachable.
    let reopened = NksCatalog::new(&temp.join("Cache"), false, &[temp.text("Offline")], no_artwork(&temp));
    assert_eq!(names(&browse(&reopened, json!({ "kind": "instrument" }), &[]).items), ["Alpha Pad"], "cache reload");
    reopened.wait_idle();
    fs::remove_file(&preview).unwrap();
    catalog.refresh_now();
    assert!(browse(&catalog, json!({ "kind": "instrument" }), &[]).items[0].get("previewPath").is_none(), "a removed preview is cleared");

    // Presets the library contributes: kept while their folder is offline, removed when deleted.
    let contributed = NksCatalog::new(&temp.join("ContributedCache"), false, &[], no_artwork(&temp));
    let (root, disconnected) = (temp.text("Library"), temp.text("Disconnected"));
    let extra = Extra { presets: vec![alpha.clone(), echo.clone()], roots: vec![root.clone()] };
    contributed.set_extra_provider(Arc::new(move || extra.clone()));
    contributed.refresh_now();
    assert_eq!(names(&browse(&contributed, json!({ "kind": "instrument" }), &[]).items), ["Alpha Pad"]);
    fs::rename(&root, &disconnected).unwrap();
    contributed.refresh_now();
    let offline = browse(&contributed, json!({ "kind": "instrument" }), &[]);
    assert_eq!((names(&offline.items), offline.items[0]["available"].as_bool()), (vec!["Alpha Pad".to_string()], Some(false)), "offline presets stay and are marked");
    assert!(contributed.status().error.is_some(), "the unavailable folder is reported");
    fs::rename(&disconnected, &root).unwrap();
    contributed.refresh_now();
    assert!(browse(&contributed, json!({ "kind": "instrument" }), &[]).items[0].get("available").is_none() && contributed.status().error.is_none(), "reconnecting restores availability");
    fs::remove_file(&alpha).unwrap();
    contributed.refresh_now();
    assert!(browse(&contributed, json!({ "kind": "instrument" }), &[]).items.is_empty(), "a deleted contributed preset leaves the catalogue");
}

fn meta(temp: &TempDir, relative: &str, name: &str, vendor: Option<&str>) {
    let vendor = vendor.map(|vendor| format!("<vendor>{vendor}</vendor>\n")).unwrap_or_default();
    temp.write(relative, format!("<?xml version=\"1.0\"?>\n<resource>\n{vendor}<name>{name}</name>\n<type>image</type>\n</resource>\n").as_bytes());
}

#[test]
fn nks_artwork_and_catalogue_drill_down() {
    let temp = TempDir::new("nks-artwork");
    let png = b"\x89PNG\r\n\x1a\nfixture picture bytes";
    temp.write("NI Resources/image/synth one/MST_artwork.png", png);
    temp.write("NI Resources/image/synth one/VB_logo.png", png);
    meta(&temp, "NI Resources/image/synth one/synth one.meta", "Synth One", None);
    temp.write("NI Resources/image/expansion x/MST_artwork.png", png);
    temp.write("NI Resources/image/expansion x/NKS2_hardware_banner.webp", b"RIFF\0\0\0\0WEBPfixture");
    meta(&temp, "NI Resources/image/expansion x/expansion x.meta", "Expansion X", None);
    temp.write("NI Resources/image/acme/acme gizmo/VB_artwork.png", png);
    meta(&temp, "NI Resources/image/acme/acme gizmo/acme gizmo.meta", "Acme Gizmo", Some("Acme"));
    temp.write("NI Resources/dist_database/acme/acme gizmo/color.json", br#"{ "VB_bgcolor" : "112233" }"#);
    temp.write("NI Resources/_DatabaseResources/color/color.db", include_bytes!("fixtures/color.db"));
    temp.write("NI Resources/image/too big/MST_artwork.png", &vec![0u8; 1024 * 1024 + 1]);
    let resources = temp.join("NI Resources");

    let art = NksArtwork::new(&resources);
    let found = |product: &str, bank: Option<&str>, color: Option<&str>| ArtMatch { product: Some(product.to_string()), bank: bank.map(String::from), color: color.map(String::from) };
    assert_eq!(art.find("Synth One", "Native Instruments", ""), found("image/synth one", None, Some("#aa5500")), "NI product folder and colour");
    assert_eq!(art.find("SYNTH ONE", "Native Instruments", "Expansion X"), found("image/synth one", Some("image/expansion x"), Some("#0055aa")), "NI expansion bank art and bank colour, case-insensitive");
    assert_eq!(art.find("Acme Gizmo", "Acme", "Expansion X"), found("image/acme/acme gizmo", None, Some("#112233")), "partner folder; NI expansion art is not borrowed by another vendor");
    assert_eq!(art.find("Unknown", "Nobody", ""), ArtMatch::default());
    let keys = |keys: &[&str]| keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
    let tiles = art.thumbnails(&keys(&["image/synth one", "image/expansion x", "../../etc", "/etc/passwd", "image", "image/too big"]), "tile");
    assert_eq!(tiles.keys().cloned().collect::<HashSet<_>>(), keys(&["image/synth one", "image/expansion x"]).into_iter().collect::<HashSet<_>>(), "only indexed folders with a bounded picture are served");
    assert_eq!(tiles["image/synth one"], format!("data:image/png;base64,{}", st_library::util::base64_encode(png)));
    assert!(art.thumbnails(&keys(&["image/synth one"]), "logo")["image/synth one"].starts_with("data:image/png;base64,"));
    assert!(art.thumbnails(&keys(&["image/acme/acme gizmo"]), "logo").is_empty(), "folders without a logo return nothing");
    assert!(art.thumbnails(&keys(&["image/synth one"]), "../x").is_empty(), "unknown variants are rejected");
    assert!(art.thumbnails(&keys(&["image/expansion x"]), "banner")["image/expansion x"].starts_with("data:image/webp;base64,"));
    assert_eq!(NksArtwork::new(&temp.join("missing")).find("Synth One", "Native Instruments", ""), ArtMatch::default(), "missing NI Resources gives no artwork");

    // The colour database reader: interior pages, overflow pages, column order from the schema.
    let table = st_library::nks::sqlite::read_table(include_bytes!("fixtures/colors-large.db"), "COLOR", 100_000).unwrap();
    assert_eq!(table.columns, ["location", "name", "value", "seqidx"]);
    assert_eq!(table.rows.len(), 601);
    assert_eq!(table.rows[599], [st_library::nks::sqlite::Value::Text("Product 599".into()), st_library::nks::sqlite::Value::Text("VB_bgcolor".into()), st_library::nks::sqlite::Value::Text("000257".into()), st_library::nks::sqlite::Value::Integer(599)]);
    assert_eq!((table.rows[600][2].text().map(str::len), &table.rows[600][3]), (Some(3000), &st_library::nks::sqlite::Value::Integer(-5)), "a value on overflow pages");
    assert_eq!(st_library::nks::sqlite::read_table(include_bytes!("fixtures/colors-large.db"), "color", 10).unwrap().rows.len(), 10);
    assert!(st_library::nks::sqlite::read_table(include_bytes!("fixtures/color.db"), "missing", 10).is_none());
    assert!(st_library::nks::sqlite::read_table(b"not a database", "color", 10).is_none());
    let mut damaged = include_bytes!("fixtures/colors-large.db").to_vec();
    damaged.truncate(4000);
    assert!(st_library::nks::sqlite::read_table(&damaged, "color", 100_000).is_some_and(|table| table.rows.len() < 601), "a truncated database gives what could be read");

    // Catalogue: vendor and sub-bank drill-down, artwork fields, hidden internal modes.
    let chained = |name: &str, vendor: &str, chain: &[&str], characters: &[&str]| preset(&json!({ "name": name, "vendor": vendor, "deviceType": "INST", "bankchain": chain, "characters": characters }), Some(&json!({ "VST.magic": 0x4b61_7431u32 })), None, None);
    temp.write("Presets/One.nksf", &chained("One", "Native Instruments", &["Synth One", "Expansion X", "Leads"], &["Bright", "_ SEL LEAD (ROUND 4)"]));
    temp.write("Presets/Two.nksf", &chained("Two", "Native Instruments", &["Synth One", "Expansion X", "Pads"], &[]));
    temp.write("Presets/Three.nksf", &chained("Three", "Native Instruments", &["Synth One", "Factory", ""], &[]));
    temp.write("Presets/Four.nksf", &chained("Four", "Acme", &["Acme Gizmo"], &[]));
    let catalog = NksCatalog::new(&temp.join("catalog"), false, &[temp.text("Presets")], Arc::new(NksArtwork::new(&resources)));
    catalog.refresh_now();
    let all = browse(&catalog, json!({ "kind": "instrument" }), &[]);
    assert_eq!(names(&all.items), ["Four", "One", "Three", "Two"]);
    assert_eq!(facet_names(&all.vendors), ["Native Instruments", "Acme"], "vendors by count");
    let product = |name: &str| all.products.iter().find(|facet| facet.name == name).unwrap();
    assert_eq!((product("Synth One").art.as_deref(), product("Synth One").vendor.as_deref(), product("Acme Gizmo").color.as_deref()), (Some("image/synth one"), Some("Native Instruments"), Some("#112233")));
    let one = all.items.iter().find(|item| item["name"] == "One").unwrap();
    assert_eq!((one["art"].as_str(), one["productArt"].as_str(), one["color"].as_str()), (Some("image/expansion x"), Some("image/synth one"), Some("#0055aa")), "preset rows carry expansion and product art");
    assert_eq!(one["characters"], json!(["Bright"]), "internal selection modes are hidden from presets");
    assert!(all.characters.iter().all(|facet| !facet.name.starts_with('_')), "and from filters");
    let acme = browse(&catalog, json!({ "kind": "instrument", "vendor": "Acme" }), &[]);
    assert_eq!((names(&acme.items), acme.products.len()), (vec!["Four".to_string()], 1), "a vendor narrows products and presets");
    let bank = browse(&catalog, json!({ "kind": "instrument", "product": "Synth One", "bank": "Expansion X" }), &[]);
    assert_eq!(names(&bank.items), ["One", "Two"]);
    assert_eq!(facet_names(&bank.sub_banks), ["Leads", "Pads"]);
    assert_eq!(bank.banks.iter().find(|facet| facet.name == "Expansion X").unwrap().art.as_deref(), Some("image/expansion x"), "bank facet artwork");
    assert_eq!(bank.banks.iter().find(|facet| facet.name == "Factory").unwrap().art, None, "banks without artwork stay plain");
    assert_eq!(names(&browse(&catalog, json!({ "kind": "instrument", "product": "Synth One", "bank": "Expansion X", "subBank": "Pads" }), &[]).items), ["Two"]);
    assert_eq!(browse(&catalog, json!({ "kind": "instrument", "product": "Synth One", "subBank": "Pads" }), &[]).total, 3, "a sub-bank without its bank is ignored");
}

/// A binary property list holding one dictionary of strings.
fn binary_plist(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut data = b"bplist00".to_vec();
    let mut offsets = Vec::new();
    offsets.push(data.len());
    data.push(0xd0 | pairs.len() as u8);
    data.extend((0..pairs.len() * 2).map(|index| index as u8 + 1));
    let string = |data: &mut Vec<u8>, text: &str| {
        if text.is_ascii() {
            if text.len() < 15 {
                data.push(0x50 | text.len() as u8);
            } else {
                data.extend([0x5f, 0x10, text.len() as u8]);
            }
            data.extend_from_slice(text.as_bytes());
        } else {
            let units: Vec<u16> = text.encode_utf16().collect();
            data.extend([0x6f, 0x10, units.len() as u8]);
            data.extend(units.iter().flat_map(|unit| unit.to_be_bytes()));
        }
    };
    for text in pairs.iter().map(|pair| pair.0).chain(pairs.iter().map(|pair| pair.1)) {
        offsets.push(data.len());
        string(&mut data, text);
    }
    let table = data.len();
    data.extend(offsets.iter().map(|offset| *offset as u8));
    data.extend([0u8; 6]);
    data.extend([1, 1]);
    data.extend((offsets.len() as u64).to_be_bytes());
    data.extend(0u64.to_be_bytes());
    data.extend((table as u64).to_be_bytes());
    data
}

#[test]
fn product_registrations() {
    assert_eq!(posix_path("Sound Drive:Arturia:DX7 V:", "Macintosh HD").as_deref(), Some("/Volumes/Sound Drive/Arturia/DX7 V"));
    assert_eq!(posix_path("Macintosh HD:Library:Massive X:", "Macintosh HD").as_deref(), Some("/Library/Massive X"));
    assert_eq!(posix_path("/:Applications:Waves:", "Macintosh HD").as_deref(), Some("/Applications/Waves"));
    assert_eq!(posix_path("/Users/Shared/Content", "Macintosh HD").as_deref(), Some("/Users/Shared/Content"));
    assert_eq!((posix_path("/a:b", "x"), posix_path("Volume", "x"), posix_path("", "x")), (None, None, None));

    let binary = binary_plist(&[("Other", "x"), ("ContentDir", "Macintosh HD:Users:Shared:Deep Matter Library:"), ("Name", "Ünïcode")]);
    assert_eq!(plist_string(&binary, "ContentDir").as_deref(), Some("Macintosh HD:Users:Shared:Deep Matter Library:"));
    assert_eq!((plist_string(&binary, "Name").as_deref(), plist_string(&binary, "Missing")), (Some("Ünïcode"), None));
    let xml = b"<?xml version=\"1.0\"?><plist><dict><key>ContentDir</key>\n\t<string>Drive:A &amp; B:</string></dict></plist>";
    assert_eq!(plist_string(xml, "ContentDir").as_deref(), Some("Drive:A & B:"));
    assert_eq!(plist_string(b"bplist00 truncated", "ContentDir"), None);

    let temp = TempDir::new("registrations");
    temp.write("Preferences/com.native-instruments.Deep Matter.plist", &binary);
    temp.write("Preferences/com.native-instruments.Arturia-DX7.plist", xml);
    temp.write("Preferences/com.native-instruments.Broken.plist", b"junk");
    temp.write("Preferences/com.other.vendor.plist", &binary);
    temp.write("Service Center/Deep Matter.xml", b"<ProductHints><Product><Name>Deep Matter</Name><Company>Native Instruments</Company><PluginID fx=\"false\">x</PluginID></Product></ProductHints>");
    let registrations = discover_registrations(&[temp.join("Preferences"), temp.join("Absent")], &temp.join("Service Center"), "Macintosh HD");
    assert_eq!(registrations.len(), 2);
    let deep = registrations.iter().find(|registration| registration.name == "Deep Matter").unwrap();
    assert_eq!((deep.company.as_str(), deep.content_dir.as_str(), deep.effect), ("Native Instruments", "/Users/Shared/Deep Matter Library", Some(false)));
    let arturia = registrations.iter().find(|registration| registration.name == "Arturia-DX7").unwrap();
    assert_eq!((arturia.company.as_str(), arturia.content_dir.as_str(), arturia.effect), ("Arturia", "/Volumes/Drive/A & B", None), "without Service Center data the vendor is the key's prefix");
}

fn group_header(name: &str, chain: &[&str], types: &[&str]) -> Vec<u8> {
    let u32le = |value: u32| value.to_le_bytes().to_vec();
    let text = |value: &str| -> Vec<u8> {
        let units: Vec<u16> = value.encode_utf16().collect();
        [u32le(units.len() as u32), units.iter().flat_map(|unit| unit.to_le_bytes()).collect()].concat()
    };
    let mut bytes = vec![0u8; 64];
    bytes.extend_from_slice(b"DSIN");
    bytes.extend_from_slice(&[0x6c, 0, 0, 0, 1, 0, 0, 0, 0x18, 0, 0, 0]);
    bytes.extend_from_slice(&[0u8; 20]);
    for word in [1, 2, 1, 0] {
        bytes.extend(u32le(word));
    }
    for value in [name, "Native Instruments", "Native Instruments", ""] {
        bytes.extend(text(value));
    }
    bytes.extend([u32le(0), u32le(u32::MAX), u32le(u32::MAX), vec![0u8; 20], u32le(1)].concat());
    bytes.extend(u32le(chain.len() as u32));
    bytes.extend(chain.iter().flat_map(|value| text(value)));
    bytes.extend(u32le(types.len() as u32));
    bytes.extend(types.iter().flat_map(|value| text(value)));
    bytes.extend([0u8; 64]);
    bytes
}

/// A session with one group of `names.len()` sampler tracks playing `sample`.
fn session(group: &str, names: &[&str], sample: &str) -> Value {
    json!({
        "bpm": 120,
        "tracks": names.iter().enumerate().map(|(index, name)| json!({ "id": index, "name": name, "routingID": format!("t{index}"), "groupID": "g", "pad": index, "instrument": index })).collect::<Vec<_>>(),
        "instruments": names.iter().enumerate().map(|(index, name)| json!({ "id": index, "type": "sample", "name": name, "samplePath": sample })).collect::<Vec<_>>(),
        "groups": [{ "id": "g", "name": group, "busID": "gb", "tags": ["Warm"] }],
        "routing": { "buses": [{ "id": "gb", "name": group, "type": "group", "groupID": "g" }], "outputs": {}, "sends": [], "sidechains": [] },
        "audioPlugins": [{ "instanceID": "p1", "owner": "bus:gb", "kind": "effect", "slot": 0, "component": "vst3:MISSING", "name": "Rare Reverb" },
                         { "instanceID": "p2", "owner": "bus:gb", "kind": "effect", "slot": 1, "component": "stfx:eq", "name": "EQ" }]
    })
}

fn search(library: &Library, value: Value) -> SearchResponse {
    library.search(serde_json::from_value(value).unwrap()).unwrap()
}

fn kits(library: &Library, value: Value) -> KitsBrowseResponse {
    library.kits_browse(serde_json::from_value(value).unwrap()).unwrap()
}

fn library_with_kits(temp: &TempDir, cache: &str, roots: &[String]) -> Library {
    let mut config = LibraryConfig::isolated(temp.join(cache), roots.iter().map(PathBuf::from).collect());
    config.user_kits_dir = Some(temp.join("My Kits"));
    Library::open(config).unwrap()
}

#[test]
fn kits_are_saved_indexed_read_and_browsed() {
    let temp = TempDir::new("kits-service");
    let native = temp.mkdir("Native Access Content");
    let group = temp.write("Native Access Content/Orbit Library/Groups/Drums/Orbit Kit.mxgrp", b"generated kit metadata fixture");
    let preview = temp.write("Native Access Content/Orbit Library/Groups/Drums/.previews/Orbit Kit.mxgrp.ogg", b"generated preview header");
    temp.mkdir("Native Access Content/Orbit Library/Sounds");
    temp.mkdir("Native Access Content/Orbit Library/Samples");
    temp.write("Native Access Content/Orbit Library/Groups/Drums/._ghost.mxgrp", &[1]);
    let sample = temp.write("Source.wav", &st_library::probe::fixtures::wav(0.1));
    let library = library_with_kits(&temp, "Cache", std::slice::from_ref(&native));
    let user_root = temp.text("My Kits");

    // kit.save: Save As with a name writes into the user's kit folder and indexes it.
    let save = |value: Value| library.kit_save(serde_json::from_value(value).unwrap());
    let saved = save(json!({ "project": session("Warm homemade kit", &["Kick", "Snare"], &sample), "groupID": "g", "name": "Warm" })).unwrap();
    let KitSaveResponse::Saved(saved) = saved else { panic!("expected a saved kit") };
    let kit = temp.text("My Kits/Warm.stkit");
    assert_eq!((saved.path.as_str(), saved.copied_samples, saved.metadata["soundCount"].as_u64(), saved.metadata["name"].as_str()), (kit.as_str(), 1, Some(2), Some("Warm")));
    assert_eq!(serde_json::to_value(KitSaveResponse::Saved(saved.clone())).unwrap()["saved"], true);
    library.wait_idle();
    // The same name again asks first; a path outside the kit folder asks for a name.
    let again = serde_json::to_value(save(json!({ "project": session("x", &["Kick"], &sample), "groupID": "g", "name": "Warm" })).unwrap()).unwrap();
    assert_eq!(again, json!({ "exists": true, "path": kit, "name": "Warm" }));
    let elsewhere = serde_json::to_value(save(json!({ "project": session("x", &["Kick"], &sample), "groupID": "g", "path": temp.text("Elsewhere.stkit"), "overwrite": true })).unwrap()).unwrap();
    assert_eq!(elsewhere, json!({ "needsName": true, "path": temp.text("Elsewhere.stkit") }));
    assert!(save(json!({ "project": {}, "groupID": "g", "name": "  " })).is_err() && save(json!({ "project": {}, "name": "x" })).is_err() && save(json!({ "project": {}, "groupID": "missing", "name": "y" })).is_err());
    // Without a name or path the kit takes its group's name, numbered when taken.
    let unnamed = |library: &Library| match library.kit_save(serde_json::from_value(json!({ "project": session("Loose", &["Kick"], &sample), "groupID": "g", "copySamples": false })).unwrap()).unwrap() {
        KitSaveResponse::Saved(saved) => saved.path,
        other => panic!("{other:?}"),
    };
    assert_eq!((unnamed(&library), unnamed(&library)), (temp.text("My Kits/Loose.stkit"), temp.text("My Kits/Loose 2.stkit")));
    fs::remove_dir_all(temp.join("My Kits/Loose 2.stkit")).unwrap();

    // kit.read returns the group project with absolute sample paths; saving it over itself is stable.
    let read = library.kit_read(serde_json::from_value(json!({ "path": kit })).unwrap()).unwrap();
    let stored_sample = read.group_project["instruments"][0]["samplePath"].as_str().unwrap().to_string();
    assert!(stored_sample.starts_with(&format!("{kit}/Samples/")) && Path::new(&stored_sample).exists());
    assert_eq!((read.group_project["groups"][0]["name"].as_str(), read.group_project["tracks"].as_array().unwrap().len(), read.sounds.len(), read.warnings.len()), (Some("Warm homemade kit"), 2, 2, 0));
    let first_id = read.metadata["id"].clone();
    let mut session_again = read.group_project.clone();
    session_again["tracks"][0]["name"] = json!("Kick renamed");
    let overwritten = save(json!({ "project": session_again, "groupID": "g", "path": kit, "overwrite": true })).unwrap();
    assert!(matches!(overwritten, KitSaveResponse::Saved(_)));
    let reread = library.kit_read(serde_json::from_value(json!({ "path": kit })).unwrap()).unwrap();
    assert_eq!(reread.group_project["instruments"][0]["samplePath"].as_str(), Some(stored_sample.as_str()), "Save over the kit keeps the sample paths an open session uses");
    assert_eq!((reread.group_project["tracks"][0]["name"].as_str(), &reread.metadata["id"], &reread.metadata["createdAt"]), (Some("Kick renamed"), &first_id, &read.metadata["createdAt"]), "the kit keeps its identity");
    assert!(library.kit_read(serde_json::from_value(json!({ "path": "relative.stkit" })).unwrap()).is_err());

    // kit.detail: pads and what to expect.
    let detail = serde_json::to_value(library.kit_detail(serde_json::from_value(json!({ "path": kit, "availableComponents": ["vst3:OTHER"] })).unwrap()).unwrap()).unwrap();
    assert_eq!((detail["name"].as_str(), detail["path"].as_str(), detail["kitFormat"].as_str(), detail["soundCount"].as_u64()), (Some("Warm homemade kit"), Some(kit.as_str()), Some("supertracker-kit"), Some(2)), "Save names the kit after its group");
    assert_eq!(detail["sounds"][0], json!({ "slot": 0, "id": "t0", "name": "Kick renamed", "type": "sample", "plugin": "", "empty": false, "missing": false }));
    assert_eq!(detail["compatibility"], json!({ "summary": "Review import notes", "loadable": true, "warnings": ["Plug-in unavailable on this computer: Rare Reverb"], "missingPlugins": ["Rare Reverb"] }));
    let ready = library.kit_detail(serde_json::from_value(json!({ "path": kit })).unwrap()).unwrap();
    assert_eq!(ready.compatibility.summary, "Ready to load", "without a plug-in list availability is not judged");

    // Indexing reads only the small header: corrupt project state does not hide the kit.
    fs::write(temp.join("My Kits/Warm.stkit/project.json"), b"not project JSON").unwrap();
    temp.write("My Kits/Broken.stkit/kit.json", b"bad header");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&user_root, temp.join("My Kits/loop")).unwrap();
    library.scan(ScanRequest::default()).unwrap();
    library.wait_idle();
    let result = search(&library, json!({ "kind": "kit" }));
    assert_eq!(result.total, 4, "both kit formats and the malformed kit, without package media or hidden files");
    let row = |path: &str| result.items.iter().find(|item| item.path == path).unwrap().clone();
    let homemade = row(&kit);
    assert_eq!((homemade.name.as_str(), homemade.user, homemade.sound_count, homemade.kit_type.as_deref(), homemade.kit_format.as_deref()), ("Warm homemade kit", Some(true), Some(2), Some("Kit"), Some("supertracker-kit")));
    assert_eq!(homemade.tags, ["Warm"], "a kit's tags are its own, not words guessed from folders");
    let ni = row(&group);
    assert_eq!((ni.kit_format.as_deref(), ni.preview_path.as_deref(), ni.expansion.as_deref(), ni.user), (Some("maschine-group"), Some(preview.as_str()), Some("Orbit"), Some(false)));
    assert!(row(&temp.text("My Kits/Broken.stkit")).metadata_error.is_some(), "a malformed kit stays visible with its error");
    assert_eq!(search(&library, json!({ "kind": "all" })).total, 0, "the All view never offers kits as samples");
    assert_eq!(search(&library, json!({ "kind": "kit", "query": "warm homemade" })).total, 1, "text finds kit names and tags");
    assert_eq!(search(&library, json!({ "kind": "kit", "filters": { "extensions": ["stkit"] } })).total, 3);
    assert_eq!(search(&library, json!({ "kind": "kit", "filters": { "categories": ["Kit"], "tags": ["Warm", "Absent"] } })).total, 2, "AND across kit fields, OR within tags");
    assert_eq!(search(&library, json!({ "kind": "kit", "filters": { "collections": ["Maschine expansions"], "packs": ["Orbit"] } })).total, 1, "the verified expansion layout is a source");
    assert_eq!(search(&library, json!({ "kind": "kit", "filters": { "collections": ["User libraries"], "tags": ["Warm"] } })).total, 2);
    assert!(result.facets["packs"].iter().any(|facet| facet.value == "Orbit"));
    library.favorite(serde_json::from_value(json!({ "path": group })).unwrap()).unwrap();
    assert_eq!(search(&library, json!({ "kind": "kit", "favorites": true })).total, 1);
    let page = search(&library, json!({ "kind": "kit", "limit": 1, "offset": 1 }));
    assert_eq!((page.items.len(), page.total), (1, 4));
    assert_eq!(library.kit_metadata(serde_json::from_value(json!({ "path": group })).unwrap()).unwrap().name, "Orbit Kit");

    // Offline: the kit stays visible; a cancelled scan of its folder keeps it; the cache restores it.
    library.wait_idle();
    fs::rename(&native, temp.join("Disconnected")).unwrap();
    assert_eq!(search(&library, json!({ "kind": "kit", "query": "Orbit" })).items[0].available, Some(false));
    assert_eq!(kits(&library, json!({})).items[0]["available"], false);
    library.scan(serde_json::from_value(json!({ "roots": [native] })).unwrap()).unwrap();
    library.cancel(Empty).unwrap();
    assert_eq!(search(&library, json!({ "kind": "kit", "query": "Orbit" })).total, 1);
    library.wait_idle();
    drop(library);
    assert_eq!(search(&library_with_kits(&temp, "Cache", &[]), json!({ "kind": "kit" })).total, 4, "both kit formats survive a cache reload");
}

#[test]
fn kit_browser_expansions_banks_and_types() {
    let temp = TempDir::new("kits-browse");
    let native = temp.mkdir("Native Access Content");
    let orbit = "Native Access Content/Orbit Library/Groups/Kits";
    let factory = "Native Access Content/Maschine 2 Factory Library Library/Groups/Kits";
    temp.write(&format!("{orbit}/Acid Kit.mxgrp"), &group_header("Acid Kit", &["Orbit"], &["\\:Kits", "\\:Kits\\:Urban Kit"]));
    let box_kit = temp.write(&format!("{orbit}/Box Kit.mxgrp"), &group_header("Box Kit", &["Orbit"], &["\\:Kits\\:Analog Kit"]));
    temp.write(&format!("{orbit}/Opaque Kit.mxgrp"), b"opaque NI group");
    temp.write(&format!("{factory}/Abbey Kit.mxgrp"), &group_header("Abbey Kit", &["Maschine", "Maschine 1.8 Library"], &["\\:Kits\\:Acoustic Kit"]));
    temp.write("NI Resources/image/orbit/MST_artwork.png", b"png");
    let sample = temp.write("Kick.wav", &[1, 2, 3, 4]);
    let user = temp.mkdir("User Kits");
    temp.mkdir("User Kits/Breaks");
    let project = st_library::kit::group::extract_group(&session("Kit", &["Kick"], &sample), "g").unwrap().project;
    let header = |fields: Value| fields.as_object().cloned().unwrap();
    st_library::kit::save(&project, &header(json!({ "name": "Warm", "tags": ["Warm"] })), &temp.text("User Kits/Breaks/Warm.stkit"), &Default::default()).unwrap();
    st_library::kit::save(&project, &header(json!({ "name": "Loose" })), &temp.text("User Kits/Loose.stkit"), &Default::default()).unwrap();
    let library = library_with_kits(&temp, "Cache", &[native.clone(), user.clone()]);
    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    library.set_event_sink(move |name, _| sink.lock().unwrap().push(name.to_string()));
    library.scan(ScanRequest::default()).unwrap();
    library.wait_idle();
    // The first browse queues the group headers; they are read in the background.
    let first = kits(&library, json!({}));
    assert_eq!((first.counts.library, first.counts.user, first.scanning), (4, 2, false));
    library.wait_idle();
    assert!(events.lock().unwrap().iter().any(|name| name == "library.kits.progress"));
    let all = kits(&library, json!({}));
    let item_names = |result: &KitsBrowseResponse| names(&result.items);
    assert_eq!((all.counts.library, all.counts.user, all.status.reading), (4, 2, false), "library groups and saved kits are counted separately");
    assert_eq!(facet_names(&all.products), ["Maschine", "Orbit"], "bank chains and the Library folder name the expansions");
    assert_eq!(item_names(&all), ["Abbey Kit", "Acid Kit", "Box Kit", "Opaque Kit"], "groups by expansion, then name");
    let orbit_result = kits(&library, json!({ "product": "Orbit" }));
    assert_eq!(orbit_result.total, 3);
    let hip_hop = orbit_result.subtypes.iter().find(|facet| facet.name == "Kits\u{1f}Hip Hop").unwrap();
    assert_eq!((hip_hop.label.as_deref(), hip_hop.parent.as_deref(), hip_hop.count), (Some("Hip Hop"), Some("Kits"), 1), "Urban Kit appears as Maschine 3's Hip Hop");
    assert_eq!(item_names(&kits(&library, json!({ "product": "Orbit", "subtypes": ["Kits\u{1f}Hip Hop"] }))), ["Acid Kit"]);
    let maschine = kits(&library, json!({ "product": "Maschine" }));
    assert_eq!((facet_names(&maschine.banks), kits(&library, json!({ "product": "Maschine", "bank": "Maschine 1.8 Library" })).total), (vec!["Maschine 1.8 Library".to_string()], 1));
    assert_eq!(item_names(&kits(&library, json!({ "query": "acoustic" }))), ["Abbey Kit"], "search covers types");
    let acid = &all.items[1];
    assert_eq!((acid["product"].as_str(), acid["vendor"].as_str(), acid["kitFormat"].as_str(), acid["user"].as_bool(), &acid["subtypes"]), (Some("Orbit"), Some("Native Instruments"), Some("maschine-group"), Some(false), &json!(["Kits\u{1f}Hip Hop"])));
    let mine = kits(&library, json!({ "user": true }));
    assert_eq!((facet_names(&mine.products), facet_names(&mine.characters), item_names(&mine)), (vec!["Breaks".to_string(), "Saved kits".to_string()], vec!["Warm".to_string()], vec!["Warm".to_string(), "Loose".to_string()]));
    assert!(mine.items.iter().all(|item| item.get("art").is_none()), "user kits never borrow NI artwork");
    assert_eq!(mine.items[0]["soundCount"], 1);
    library.favorite(serde_json::from_value(json!({ "path": box_kit })).unwrap()).unwrap();
    assert_eq!(item_names(&kits(&library, json!({ "favorites": true }))), ["Box Kit"]);
    let paged = kits(&library, json!({ "offset": 1, "limit": 2 }));
    assert_eq!((item_names(&paged), paged.total), (vec!["Acid Kit".to_string(), "Box Kit".to_string()], 4));
    library.wait_idle();
    drop(library);
    // Headers are remembered by size and date: a new session browses without reading groups again.
    let restored = library_with_kits(&temp, "Cache", &[native, user]);
    let again = kits(&restored, json!({}));
    assert_eq!((facet_names(&again.products), again.status.reading, again.status.pending), (vec!["Maschine".to_string(), "Orbit".to_string()], false, 0));
}
