//! Maschine group import: the archive reader across versions, module parameters, sampler zones,
//! hosted plug-in state, the control mappings and the conversion to a kit.
//! Ported from `tests/MaschineGroup*Tests.swift`. Every fixture is an original, synthetic
//! archive built to the decoded member layout; no vendor file, sample or code is involved.

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_library::maschine::archive::{encoded as e, sound_layout, Archive, View, Zone};
use st_library::maschine::mapping::{level_amplitude, map_additional, map_core, map_distortion, map_drum, map_effect, map_sampler, map_spatial, EffectMapping};
use st_library::maschine::plugin_state::PluginState;
use st_library::maschine::{self, catalogue, expansion_root, import_group, resolve, INSERT_SLOT_LIMIT};
use st_library::probe::fixtures::wav_with;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

fn string(text: &str) -> Vec<u8> {
    [e(text.len() as i64), text.as_bytes().to_vec()].concat()
}

fn float(value: f32) -> Vec<u8> {
    value.to_bits().to_le_bytes().to_vec()
}

/// A float parameter whose final tracked ID is `id`: three IDs (older archives) or four IDs and
/// the base-tracking byte (Maschine 2+), then value, default and modulation floats.
fn numeric(id: i64, value: f32, modern: bool) -> Vec<u8> {
    numeric_with(id, value, modern, [0.0, 0.0], 0)
}

fn numeric_with(id: i64, value: f32, modern: bool, trailer: [f32; 2], flag: u8) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..if modern { 4 } else { 3 }).rev().flat_map(|n| e(id - n)).collect();
    if modern {
        bytes.push(0);
    }
    [bytes, float(value), float(trailer[0]), float(trailer[1]), vec![flag], float(0.0)].concat()
}

/// An integer, enum or boolean parameter whose final tracked ID is `id`.
fn integral(id: i64, value: i64, modern: bool) -> Vec<u8> {
    let mut bytes: Vec<u8> = (0..if modern { 4 } else { 3 }).rev().flat_map(|n| e(id - n)).collect();
    if modern {
        bytes.push(0);
    }
    let signed = |value: i64| if value < 0 { let mut b = e(-value); b[0] = (-(b[0] as i8)) as u8; b } else { e(value) };
    [bytes, signed(value), e(0), e(0), e(0), e(0)].concat()
}

/// A name parameter (of a Sound or of the Group).
fn name(id: i64, text: &str, modern: bool, trailer: [f32; 2], flag: u8) -> Vec<u8> {
    let width = if modern { 4 } else { 3 };
    [e(id), if modern { vec![0] } else { vec![] }, string(text), numeric_with(id + width, 0.0, modern, trailer, flag)].concat()
}

fn container(archive: Vec<u8>) -> Vec<u8> {
    let mut container = vec![0u8; 32];
    container[12..16].copy_from_slice(b"hsin");
    container[20..28].copy_from_slice(b"2SAM PRG");
    container.extend((archive.len() as u32).to_le_bytes());
    container.extend(archive);
    let length = (container.len() as u64).to_le_bytes();
    container[..8].copy_from_slice(&length);
    container
}

type Body = Box<dyn Fn(i64, usize) -> Vec<u8>>;
/// A fixture zone: path, root key, key range, first and last frame, native gain.
type ZoneSpec<'a> = (&'a str, i64, (i64, i64), (i64, i64), f32);

struct Options {
    archive_version: i64,
    group_version: i64,
    sound_version: u64,
    /// Every Sound owns its tracked members instead of referencing the first Sound's.
    per_sound_copies: bool,
    names: Vec<String>,
    /// Sounds without any insert Slot (audio-input or MIDI-only pads).
    slotless: Vec<usize>,
    module_type: String,
    /// Register a Module class although no Slot exists (must be refused).
    orphan_module_class: bool,
    group_trailer: [f32; 2],
    group_flag: u8,
    /// Mixer members written before each Sound's name: (level, pan, mute, choke group).
    channel: Option<(f32, f32, i64, i64)>,
    /// The Module body after the Slot; receives the Module's object ID and the pad.
    body: Option<Body>,
    /// Group-level inserts (bypassed where listed), each an Iceverb-shaped module of `module_type`.
    group_inserts: usize,
    bypassed_group_inserts: Vec<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            archive_version: 12,
            group_version: 9,
            sound_version: 6,
            per_sound_copies: false,
            names: (1..=16).map(|n| format!("Sound {n}")).collect(),
            slotless: Vec::new(),
            module_type: "Iceverb".to_string(),
            orphan_module_class: false,
            group_trailer: [0.0, 0.0],
            group_flag: 0,
            channel: None,
            body: None,
            group_inserts: 0,
            bypassed_group_inserts: Vec::new(),
        }
    }
}

fn variant(archive_version: i64, group_version: i64, sound_version: u64) -> Options {
    Options { archive_version, group_version, sound_version, ..Default::default() }
}

fn fixture(o: &Options) -> Vec<u8> {
    let modern = o.archive_version >= 13;
    let layout = sound_layout(o.sound_version, modern).unwrap();
    let inherited: i64 = if modern { 4 } else { 3 };
    let step = inherited;
    let mut archive: Vec<u8> = vec![1, 22];
    archive.extend_from_slice(b"serialization::archive");
    archive.extend([e(o.archive_version), e(0), e(1), e(o.group_version)].concat());
    archive.extend([1, 16, 1, 1, 0, 1, 1]);
    let (mut slot_declared, mut module_declared) = (false, false);
    let default_body = |module: i64| (1..=4).flat_map(|n| numeric(module + n * step, n as f32 / 4.0, modern)).collect::<Vec<u8>>();
    for index in 0..16 {
        let id = 100 + index as i64 * 1000;
        let delta = if index == 0 || o.per_sound_copies { layout.first_name_delta } else { layout.name_delta };
        let name_id = id + delta;
        archive.extend(e(21));
        if index == 0 {
            archive.extend([e(1), e(o.sound_version as i64)].concat());
        }
        archive.extend([e(id), e(2), e(id + 1)].concat());
        if let Some((level, pan, mute, choke)) = o.channel {
            let d: [i64; 8] = if modern { [26, 30, 34, 38, 43, 53, 77, 82] } else { [21, 24, 27, 30, 34, 42, 61, 65] };
            let at = |n: usize| id + d[n] + layout.channel_shift;
            archive.extend([numeric(at(0), level, modern), numeric(at(1), pan, modern), integral(at(2), mute, modern), integral(at(3), 0, modern), numeric(at(4), 0.0, modern), numeric(at(5), 0.0, modern), integral(at(6), choke, modern), integral(at(7), 0, modern)].concat());
        }
        archive.extend(name(name_id, &o.names[index], modern, [0.0, 0.0], 0));
        if o.slotless.contains(&index) {
            continue;
        }
        let slot = name_id + if modern { 18 } else { 14 };
        let module = slot + inherited + 1;
        archive.extend(e(32));
        if !slot_declared {
            archive.extend([e(1), e(0)].concat());
            slot_declared = true;
        }
        archive.extend((0..=inherited).flat_map(|n| e(slot + n)));
        if modern {
            archive.push(0);
        }
        archive.extend([e(1), e(1), e(0), e(0), e(0)].concat());
        archive.extend(e(40));
        if !module_declared {
            archive.extend([string(&format!("NI::MASCHINE::DATA::{}", o.module_type)), e(1), e(0)].concat());
            module_declared = true;
        }
        archive.extend(e(module));
        archive.extend(match &o.body {
            Some(body) => body(module, index),
            None => default_body(module),
        });
    }
    archive.extend([0u8; 24]);
    archive.extend(name(17000, "Synthetic Group", modern, o.group_trailer, o.group_flag));
    for insert in 0..o.group_inserts {
        let slot = 20000 + insert as i64 * 100;
        let module = slot + inherited + 1;
        archive.extend(e(32));
        archive.extend((0..=inherited).flat_map(|n| e(slot + n)));
        if modern {
            archive.push(0);
        }
        archive.extend([e(if o.bypassed_group_inserts.contains(&insert) { 0 } else { 1 }), e(1), e(0), e(0), e(0), e(40), e(module)].concat());
        archive.extend(default_body(module));
    }
    if o.orphan_module_class {
        archive.extend([e(40), string(&format!("NI::MASCHINE::DATA::{}", o.module_type)), e(1), e(0)].concat());
    }
    container(archive)
}

fn parse(o: &Options) -> Archive {
    Archive::parse(&fixture(o)).unwrap()
}

fn position(bytes: &[u8], pattern: &[u8], from: usize) -> usize {
    from + bytes[from..].windows(pattern.len()).position(|window| window == pattern).unwrap()
}

#[test]
fn archive_structure_and_damage() {
    let original = fixture(&Options::default());
    let archive = Archive::parse(&original).unwrap();
    assert_eq!(archive.sounds.len(), 16);
    assert_eq!(archive.sounds.iter().map(|sound| sound.name.clone()).collect::<Vec<_>>(), (1..=16).map(|n| format!("Sound {n}")).collect::<Vec<_>>(), "Sound names keep their declared pad identity");
    assert_eq!(archive.modules.len(), 16);
    for module in &archive.modules {
        assert_eq!(archive.parameters(module).unwrap(), [0.25, 0.5, 0.75, 1.0], "typed parameter identity");
        assert_eq!((module.kind.as_str(), module.version, module.enabled), ("Iceverb", 0, true));
    }
    assert_eq!((archive.archive_version, archive.group_version, archive.sound_version, archive.group_name.as_str(), archive.modern()), (12, 9, 6, "Synthetic Group", false));
    for length in [0, 12, 255, original.len() - 1] {
        assert!(Archive::parse(&original[..length]).is_err(), "truncated archive accepted at {length}");
    }
    let mut bad = original.clone();
    bad[12] = 0;
    assert_eq!(Archive::parse(&bad).unwrap_err().to_string(), "Maschine group: this is not a supported native .mxgrp container.");
    bad = original.clone();
    bad[0] ^= 1;
    assert!(Archive::parse(&bad).is_err(), "wrong length accepted");
    bad = original.clone();
    let signature = position(&bad, b"serialization::archive", 0);
    bad[signature + 22 + 1] = 99;
    assert!(Archive::parse(&bad).unwrap_err().to_string().contains("has not been decoded safely"), "unknown archive version accepted");
    bad = original.clone();
    bad[archive.sounds[1].offset + 1] = 22;
    assert!(Archive::parse(&bad).is_err(), "missing Sound class accepted");
    // A non-finite parameter value is refused where it is read.
    bad = original.clone();
    let module = &archive.modules[0];
    let prefix = [e(module.object_id + 2), e(module.object_id + 3)].concat();
    let at = position(&bad, &prefix, module.body_offset) + prefix.len();
    bad[at..at + 4].copy_from_slice(&float(f32::NAN));
    let damaged = Archive::parse(&bad).unwrap();
    assert!(damaged.parameters(&damaged.modules[0]).is_err(), "non-finite parameter accepted");

    let view = View { bytes: &[255, 12] };
    let mut cursor = 0;
    assert_eq!(view.signed_integer(&mut cursor, 2).unwrap(), -12, "signed portable scalar");
    cursor = 0;
    assert!(View { bytes: &[5, 1, 2, 3, 4, 5] }.integer(&mut cursor, 6).is_err(), "oversized integer accepted");
    cursor = 0;
    assert!(View { bytes: &[2, 1, 0] }.integer(&mut cursor, 3).is_err(), "non-canonical integer accepted");
    assert_eq!((e(0), e(1), e(300), e(70000)), (vec![0], vec![1, 1], vec![2, 44, 1], vec![3, 112, 17, 1]));
}

#[test]
fn every_archive_and_sound_version() {
    // Every Sound version and archive generation written by Maschine 1.8 up to Maschine 3.
    let variants: [(i64, i64, u64); 14] = [(10, 1, 1), (10, 2, 3), (10, 4, 4), (10, 9, 5), (10, 9, 6), (12, 9, 6), (13, 13, 6), (14, 14, 6), (14, 15, 8), (15, 20, 8), (16, 22, 8), (17, 22, 9), (18, 23, 9), (19, 23, 9)];
    for (av, gv, sv) in variants {
        let label = format!("archive {av}, Group {gv}, Sound {sv}");
        let options = variant(av, gv, sv);
        let mut data = fixture(&options);
        let archive = Archive::parse(&data).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(archive.sounds.iter().map(|sound| sound.name.as_str()).collect::<Vec<_>>(), options.names, "{label}: pad order");
        assert!(archive.modules.len() == 16 && archive.modules.iter().all(|module| module.kind == "Iceverb"), "{label}: one insert per Sound");
        assert!(archive.sounds.iter().all(|sound| archive.modules_of(sound).len() == 1), "{label}: inserts belong to their own Sound");
        assert!(archive.group_name == "Synthetic Group" && archive.group_modules().is_empty(), "{label}: Group name and boundary");
        for module in &archive.modules {
            assert_eq!(archive.parameters(module).unwrap(), [0.25, 0.5, 0.75, 1.0], "{label}: typed parameters");
        }
        // Each version's object-ID layout is distinct: decoding with a neighbouring Sound
        // version is refused rather than misread.
        let vector = position(&data, &[1, 16, 1, 1, 0, 1, 1], position(&data, b"serialization::archive", 0)) + 7;
        let version_offset = vector + e(21).len() + e(1).len() + 1;
        assert_eq!(data[version_offset], sv as u8, "{label}: fixture Sound version position");
        for other in [1u64, 3, 4, 6, 8, 9] {
            let modern = av >= 13;
            if sound_layout(other, modern).is_some() && sound_layout(other, modern) != sound_layout(sv, modern) {
                data[version_offset] = other as u8;
                assert!(Archive::parse(&data).is_err(), "{label} misread as Sound version {other}");
            }
        }
    }
    // Unknown archive, Group and Sound versions stay refused.
    for (av, gv, sv) in [(11, 9, 6u64), (20, 23, 9), (19, 24, 9), (19, 23, 10), (12, 9, 7), (17, 22, 7), (10, 1, 8)] {
        let modern = av >= 13;
        let usable = if sound_layout(sv.min(9), modern).is_some() { sv.min(9) } else if modern { 9 } else { 6 };
        let mut data = fixture(&variant(av, gv, usable));
        if sv != usable {
            let vector = position(&data, &[1, 16, 1, 1, 0, 1, 1], position(&data, b"serialization::archive", 0)) + 7;
            data[vector + e(21).len() + e(1).len() + 1] = sv as u8;
        }
        assert!(Archive::parse(&data).is_err(), "archive {av}, Group {gv}, Sound {sv} must be refused");
    }
    // External-MIDI templates: every Sound owns its members (all at the first-Sound distance).
    let copies = Options { per_sound_copies: true, ..variant(14, 15, 8) };
    assert_eq!(parse(&copies).sounds.iter().map(|sound| sound.name.clone()).collect::<Vec<_>>(), copies.names);
    // Empty pads have an empty name; Maschine shows them by pad number.
    let empty_names: Vec<String> = (1..=16).map(|n| if n > 8 { String::new() } else { format!("Pad {n}") }).collect();
    let empty = parse(&Options { names: empty_names.clone(), ..variant(18, 22, 9) });
    assert_eq!(empty.sounds.iter().map(|sound| sound.name.clone()).collect::<Vec<_>>(), empty_names, "empty Sound names are decoded");
    assert_eq!(empty.sounds.iter().map(maschine::display_name).collect::<Vec<_>>(), (1..=16).map(|n| if n > 8 { format!("Sound {n}") } else { format!("Pad {n}") }).collect::<Vec<_>>());
    assert_eq!(parse(&Options { names: empty_names.clone(), ..variant(10, 4, 4) }).sounds.iter().map(|sound| sound.name.clone()).collect::<Vec<_>>(), empty_names, "empty names in an old archive");
    // An input Sound without Slots first: the Slot class is declared by the next Sound.
    let input = parse(&Options { slotless: vec![0, 5], ..variant(14, 14, 6) });
    assert!(input.modules.len() == 14 && input.modules_of(&input.sounds[0]).is_empty() && input.modules_of(&input.sounds[5]).is_empty() && input.modules_of(&input.sounds[1]).len() == 1);
    // MIDI-only groups have no Slot and no Module class at all.
    let midi = Options { slotless: (0..16).collect(), ..variant(14, 15, 8) };
    let midi_archive = parse(&midi);
    assert!(midi_archive.modules.is_empty() && !midi_archive.has_slots && midi_archive.sounds.len() == 16);
    assert!(Archive::parse(&fixture(&Options { orphan_module_class: true, slotless: (0..16).collect(), ..variant(14, 15, 8) })).is_err(), "a Module class without any Slot is refused");
    // A Group name whose parameter trailer carries data is still unique.
    assert_eq!(parse(&Options { group_trailer: [10.0, 9.0], group_flag: 1, ..variant(19, 23, 9) }).group_name, "Synthetic Group");
    assert!(Archive::parse(&fixture(&Options { group_trailer: [f32::NAN, 9.0], group_flag: 1, ..variant(19, 23, 9) })).is_err(), "a non-finite Group name trailer is refused");
    // Truncation and corruption never crash and are refused.
    let old = fixture(&variant(10, 1, 1));
    for length in (0..old.len()).step_by((old.len() / 97).max(1)) {
        assert!(Archive::parse(&old[..length]).is_err(), "truncated archive accepted at {length}");
    }
    let mut refused = 0;
    for offset in (40..old.len()).step_by(7) {
        let mut bad = old.clone();
        bad[offset] ^= 0x5a;
        if Archive::parse(&bad).is_err() {
            refused += 1;
        }
    }
    assert!(refused > 0, "corrupted archives are refused");
    // Reading members of a corrupted but accepted archive never panics either.
    for offset in (40..old.len()).step_by(3) {
        let mut bad = old.clone();
        bad[offset] = bad[offset].wrapping_add(1);
        if let Ok(archive) = Archive::parse(&bad) {
            for module in &archive.modules {
                let _ = archive.parameters(module);
            }
            for sound in &archive.sounds {
                let _ = archive.channel(sound);
            }
        }
    }
}

fn write_group(temp: &TempDir, relative: &str, options: &Options) -> String {
    temp.write(relative, &fixture(options))
}

fn status_of(report: &Value, status: &str) -> Vec<Value> {
    report["modules"].as_array().unwrap().iter().filter(|module| module["status"] == status).cloned().collect()
}

#[test]
fn groups_import_as_kits() {
    let temp = TempDir::new("maschine-import");
    let path = write_group(&temp, "Synthetic.mxgrp", &Options { channel: Some((0.75, -0.5, 1, 3)), ..Default::default() });
    let imported = import_group(&path, Some(&temp.text(""))).unwrap();
    assert_eq!((imported.name.as_str(), imported.sounds.len()), ("Synthetic Group", 16));
    assert!(imported.sounds.iter().all(|sound| sound.instrument.get("unavailable").is_none()), "effects-only Sounds keep empty sources");
    assert!(imported.sounds.iter().all(|sound| sound.plugins.len() == 1 && sound.plugins[0]["kind"] == "effect" && sound.plugins[0]["slot"] == 0), "first-slot effects stay inserts");
    let first = &imported.sounds[0];
    assert_eq!((first.volume, first.pan, first.mute, first.choke_group, first.name.as_str()), (1.0, -0.5, true, 3, "Sound 1"), "mixer members: unity level, pan, mute, choke group");
    assert_eq!(first.maschine_channel["chokeGroup"], 3.0);
    let plugin = &first.plugins[0];
    assert_eq!((plugin["component"].as_str(), plugin["format"].as_str(), plugin["name"].as_str(), plugin["bypass"].as_bool()), (Some("stfx:ice"), Some("Builtin"), Some("Ice"), Some(false)));
    let state: Value = serde_json::from_slice(&st_library::util::base64_decode(plugin["state"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(state, json!({ "format": "SuperTracker.BuiltinFX", "version": 1, "id": "ice", "parameters": { "color": 0.25, "ice": 0.5, "size": 0.75, "mix": 1.0 } }), "a built-in effect state holds normalized values by parameter id");
    assert!(imported.warnings.iter().any(|warning| warning.contains("effects-only Sound")) && imported.warnings.iter().any(|warning| warning.contains("Pattern notes")));
    assert_eq!((status_of(&imported.report, "approximation-mapped-controls").len(), imported.report["archiveVersion"].as_u64(), imported.report["sourcePath"].as_str()), (16, Some(12), Some(path.as_str())));
    assert_eq!(imported.report["modules"][0]["parameters"], json!([0.25, 0.5, 0.75, 1.0]));

    // As a kit: every pad with inserts is a member track of one group routed to the group bus.
    let project = imported.group_project();
    st_library::kit::group::validate(&project).unwrap();
    let (group, tracks) = (&project["groups"][0], project["tracks"].as_array().unwrap());
    assert_eq!((tracks.len(), project["instruments"].as_array().unwrap().len(), project["audioPlugins"].as_array().unwrap().len()), (16, 16, 16));
    assert_eq!((group["name"].as_str(), group["importNotes"].as_array().map(Vec::len)), (Some("Synthetic Group"), Some(imported.warnings.len())));
    for (index, track) in tracks.iter().enumerate() {
        assert_eq!((track["pad"].as_u64(), track["groupID"].as_str(), track["instrument"].as_u64(), track["chokeGroup"].as_u64(), track["mute"].as_bool()), (Some(index as u64), group["id"].as_str(), Some(index as u64), Some(3), Some(true)));
        let owner = format!("track:{}", track["routingID"].as_str().unwrap());
        assert_eq!(project["routing"]["outputs"][&owner], format!("bus:{}", group["busID"].as_str().unwrap()));
        assert_eq!(project["audioPlugins"][index]["owner"], owner);
    }
    assert_eq!(project["routing"]["buses"][0]["groupID"], group["id"]);
    assert_ne!(imported.group_project()["groups"][0]["id"], group["id"], "every load gets fresh identities");

    // Through the service: kit.read and kit.detail for a group.
    let library = st_library::Library::open(st_library::LibraryConfig::isolated(temp.join("Cache"), Vec::new())).unwrap();
    let read = library.kit_read(serde_json::from_value(json!({ "path": path })).unwrap()).unwrap();
    assert_eq!((read.metadata["name"].as_str(), read.metadata["kitFormat"].as_str(), read.metadata["soundCount"].as_u64(), read.sounds.len()), (Some("Synthetic Group"), Some("maschine-group"), Some(0), 16));
    assert!(read.report.is_some() && read.group_project["groups"][0]["name"] == "Synthetic Group");
    let detail = library.kit_detail(serde_json::from_value(json!({ "path": path, "availableComponents": [] })).unwrap()).unwrap();
    assert!(detail.compatibility.loadable && detail.compatibility.missing_plugins.is_empty() && detail.compatibility.summary == "Review import notes");

    // A group with eleven inserts keeps them all (a channel holds sixteen), renumbered, bypass kept.
    assert_eq!(INSERT_SLOT_LIMIT, 16);
    let long_path = write_group(&temp, "Long.mxgrp", &Options { group_inserts: 11, bypassed_group_inserts: vec![2], ..Default::default() });
    assert_eq!(Archive::parse(&fs::read(&long_path).unwrap()).unwrap().group_modules().len(), 11);
    let long = import_group(&long_path, None).unwrap();
    assert_eq!(long.group_plugins.iter().map(|plugin| plugin["slot"].as_u64().unwrap()).collect::<Vec<_>>(), (0..11).collect::<Vec<_>>());
    assert_eq!(long.group_plugins.iter().map(|plugin| plugin["bypass"].as_bool().unwrap()).collect::<Vec<_>>(), (0..11).map(|n| n == 2).collect::<Vec<_>>(), "bypassed inserts are kept, bypassed, in order");
    assert!(status_of(&long.report, "omitted-insert-limit").is_empty() && !long.warnings.iter().any(|warning| warning.contains("inserts; its original state")));
    let long_project = long.group_project();
    st_library::kit::group::validate(&long_project).unwrap();
    let bus_owner = format!("bus:{}", long_project["groups"][0]["busID"].as_str().unwrap());
    assert_eq!(long_project["audioPlugins"].as_array().unwrap().iter().filter(|plugin| plugin["owner"] == bus_owner.as_str()).count(), 11);
    // Beyond sixteen: bypassed inserts are left out first, then the trailing ones.
    let huge = import_group(&write_group(&temp, "Huge.mxgrp", &Options { group_inserts: 19, bypassed_group_inserts: vec![2], ..Default::default() }), None).unwrap();
    assert_eq!(huge.group_plugins.iter().map(|plugin| plugin["slot"].as_u64().unwrap()).collect::<Vec<_>>(), (0..16).collect::<Vec<_>>());
    assert!(huge.group_plugins.iter().all(|plugin| plugin["bypass"] == false), "bypassed inserts are omitted before active ones");
    let omitted = status_of(&huge.report, "omitted-insert-limit");
    let mut slots: Vec<u64> = omitted.iter().map(|module| module["slot"].as_u64().unwrap()).collect();
    slots.sort();
    assert_eq!(slots, [2, 17, 18]);
    assert!(omitted.iter().all(|module| module.get("parameters").is_some()), "omitted inserts keep their original parameters in the report");
    assert_eq!(huge.warnings.iter().filter(|warning| warning.contains("holds 16 inserts")).count(), 3);

    // Unknown source modules stay explicit, unavailable pads; with nothing loadable the kit keeps one pad.
    let unknown = import_group(&write_group(&temp, "Unknown.mxgrp", &Options { module_type: "UnknownInstrument".to_string(), ..Default::default() }), None).unwrap();
    assert!(unknown.sounds.iter().all(|sound| sound.instrument["unavailable"] == true && !sound.has_source()));
    assert!(unknown.warnings.iter().any(|warning| warning.contains("UnknownInstrument instrument state is retained")));
    assert_eq!(unknown.group_project()["tracks"].as_array().unwrap().len(), 1);
    // MIDI-only groups import as empty pads with a group-level note.
    let mut midi = Options { slotless: (0..16).collect(), ..variant(14, 15, 8) };
    midi.names[3] = String::new();
    let midi_import = import_group(&write_group(&temp, "Midi.mxgrp", &midi), None).unwrap();
    assert_eq!(midi_import.sounds[3].name, "Sound 4", "an unnamed pad imports by its number");
    assert!(midi_import.warnings.iter().any(|warning| warning.contains("no audio modules")));
    assert!(import_group(&temp.write("Empty.mxgrp", &[]), None).is_err() && import_group(&temp.text("Missing.mxgrp"), None).is_err());
}

#[test]
fn drum_synths_and_effect_modules_convert() {
    let temp = TempDir::new("maschine-drums");
    // A Kick drum synth (version 2, 31 members): engine 1, Tune 0.75, Impact 0.81.
    let kinds = "effffffefffffffffffeffeffeffbff";
    let body: Body = Box::new(move |module, _| {
        let mut id = module;
        let mut bytes = Vec::new();
        for (position, kind) in kinds.chars().enumerate() {
            id += if kind == 'e' { 4 } else { 3 };
            let value = match position {
                0 => 1.0,
                1 => 0.75,
                4 => 0.81,
                _ => 0.5,
            };
            bytes.extend(if kind == 'f' { numeric(id, value, false) } else { integral(id, if position == 0 { 1 } else { 0 }, false) });
        }
        bytes
    });
    let mut data = fixture(&Options { module_type: "Kick".to_string(), body: Some(body), ..Default::default() });
    // The fixture registers the class as version 0; the Kick's schema is version 2.
    let class = position(&data, b"NI::MASCHINE::DATA::Kick", 0) + 24;
    assert_eq!(&data[class..class + 3], &[1, 1, 0]);
    data.splice(class + 2..class + 3, [1, 2]);
    let length = (data.len() as u64).to_le_bytes();
    data[..8].copy_from_slice(&length);
    let archive_length = ((data.len() - 36) as u32).to_le_bytes();
    data[32..36].copy_from_slice(&archive_length);
    let path = temp.write("Drums.mxgrp", &data);
    let archive = Archive::parse(&data).unwrap();
    assert_eq!((archive.modules[0].kind.as_str(), archive.modules[0].version), ("Kick", 2));
    let raw = archive.parameters(&archive.modules[0]).unwrap();
    assert_eq!((raw.len(), raw[0], raw[1], raw[4]), (31, 1.0, 0.75, 0.8100000023841858));
    let imported = import_group(&path, None).unwrap();
    let instrument = &imported.sounds[0].instrument;
    let sample = instrument["samplePath"].as_str().unwrap();
    assert!(sample.starts_with("builtin:drums/kick/") && sample.rsplit('/').next().unwrap().len() == 64, "a drum synth renders as a built-in sample carrying all eight controls: {sample}");
    assert_eq!((instrument["type"].as_str(), instrument["mode"].as_str(), instrument["end"].as_f64(), instrument.get("drumModel")), (Some("sample"), Some("oneshot"), Some(1.0), None));
    let controls: Vec<f32> = (0..8).map(|index| f32::from_bits(u32::from_str_radix(&sample.rsplit('/').next().unwrap()[index * 8..index * 8 + 8], 16).unwrap())).collect();
    let mapped = map_drum("Kick", 2, &raw).unwrap();
    assert_eq!(controls, mapped.drum_parameters.iter().map(|value| *value as f32).collect::<Vec<_>>());
    assert!((mapped.physical(0) - 6.0).abs() < 1e-5 && (mapped.physical(3) - 81.0).abs() < 1e-4);
    assert_eq!(status_of(&imported.report, "approximation-mapped-controls").len(), 16);
    assert!(imported.sounds[0].has_source() && imported.group_project()["tracks"].as_array().unwrap().len() == 16);
}

/// The body of a Sampler module in an older archive (three IDs per parameter) with `zones`
/// (path, root key, key range, start and end frame, gain) and its controls.
fn sampler_body(module: i64, first: bool, zones: &[ZoneSpec], controls: &BTreeMap<&str, f32>) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend([e(zones.len() as i64), vec![1, 1]].concat());
    if first {
        bytes.extend([0, 1, 1]);
    }
    let deltas: [i64; 16] = [3, 5, 8, 10, 13, 16, 19, 22, 25, 28, 30, 33, 35, 38, 41, 44];
    let mut last = module;
    for (index, (path, root, keys, frames, gain)) in zones.iter().enumerate() {
        let id = module + 1 + index as i64 * 50;
        last = id;
        bytes.extend(e(50));
        if first && index == 0 {
            bytes.extend([1, 1, 1, 2]);
        }
        bytes.extend([e(id), if first && index == 0 { vec![0, 0] } else { vec![] }, e(2), string(path), if first && index == 0 { vec![0, 0] } else { vec![] }, string("C:/Original/"), string("Fixture Library")].concat());
        let integers: [i64; 13] = [frames.0, frames.1, frames.0, frames.1, 0, 0, 0, 0, *root, keys.0, keys.1, 0, 127];
        for (position, value) in integers.iter().enumerate() {
            bytes.extend(integral(id + deltas[position], *value, false));
        }
        bytes.extend([numeric(id + deltas[13], *gain, false), numeric(id + deltas[14], 0.25, false), numeric(id + deltas[15], -12.0, false)].concat());
    }
    let fields: [(&str, char); 27] = [
        ("0xc0", 'e'), ("0xc8", 'e'), ("0xd0", 'f'), ("0xd8", 'f'), ("0xe0", 'e'), ("0xe8", 'e'), ("0xf0", 'b'), ("0xf8", 'f'), ("0x100", 'f'), ("0x128", 'f'), ("0x130", 'f'), ("0x138", 'f'), ("0x140", 'f'),
        ("0x148", 'e'), ("0x150", 'f'), ("0x158", 'f'), ("0x160", 'f'), ("0x168", 'f'), ("0x170", 'f'), ("0x178", 'f'), ("0x180", 'f'), ("0x188", 'f'), ("0x190", 'f'), ("0x198", 'f'), ("0x1a0", 'f'), ("0x1a8", 'f'), ("0x1b0", 'f'),
    ];
    let mut id = last + 58;
    // The fixture registers the Sampler class as version 0, which has no member at 0xc0.
    for (key, kind) in fields.into_iter().filter(|(key, _)| *key != "0xc0") {
        id += if kind == 'e' { 4 } else { 3 };
        let value = controls.get(key).copied().unwrap_or(if key == "0x190" { 1.0 } else { 0.0 });
        bytes.extend(if kind == 'f' { numeric(id, value, false) } else { integral(id, value as i64, false) });
    }
    for envelope in ["ampEnvelope", "filterEnvelope"] {
        for index in 0..9 {
            id += 3;
            bytes.extend(numeric(id, controls.get(format!("{envelope}.{index}").as_str()).copied().unwrap_or(0.0), false));
        }
    }
    bytes
}

#[test]
fn sampler_sounds_keep_their_zones() {
    let temp = TempDir::new("maschine-sampler");
    temp.write("Fixture Library/Samples/Drums/Kick.wav", &wav_with(0.1, 44_100, 1)); // 4410 frames
    temp.write("Fixture Library/Samples/Drums/Snare.wav", &wav_with(0.05, 44_100, 2)); // 2205 frames
    let controls: BTreeMap<&str, f32> = [("0xc8", 1.0), ("0xe8", 2.0), ("0xf8", -0.25), ("0x148", 1.0), ("0x150", 0.5), ("0x158", 0.2), ("ampEnvelope.0", 0.5), ("ampEnvelope.2", 1.0), ("ampEnvelope.3", 0.5), ("ampEnvelope.4", 0.25)].into_iter().collect();
    let body: Body = Box::new(move |module, index| {
        let zones: Vec<ZoneSpec> = match index {
            0 => vec![("Samples/Drums/Kick.wav", 36, (0, 59), (0, 4409), 0.75), ("//Samples/Drums/Snare.wav", 62, (60, 127), (441, 2204), 1.0)],
            1 => vec![("Samples/Drums/Missing.wav", 60, (0, 127), (0, 99), 0.75), ("../Outside.wav", 60, (0, 127), (0, 99), 0.75)],
            _ => vec![("Samples/Drums/Kick.wav", 60, (0, 127), (0, 2204), 0.5)],
        };
        sampler_body(module, index == 0, &zones, &controls)
    });
    let options = Options { module_type: "Sampler".to_string(), body: Some(body), channel: Some((0.5, 0.25, 0, 0)), ..Default::default() };
    let path = write_group(&temp, "Fixture Library/Groups/Kits/Sampled.mxgrp", &options);
    let archive = Archive::parse(&fs::read(&path).unwrap()).unwrap();
    let first = archive.zones(&archive.modules[0]).unwrap();
    assert_eq!(first.iter().map(|zone| zone.path.as_str()).collect::<Vec<_>>(), ["Samples/Drums/Kick.wav", "//Samples/Drums/Snare.wav"]);
    assert_eq!((first[0].origin.as_str(), first[0].library.as_str(), first[1].values["root"], first[1].values["keyLow"], first[1].values["endFrame"], first[1].values["gain"], first[1].values["tune"]), ("C:/Original/", "Fixture Library", 62.0, 60.0, 2204.0, 1.0, -12.0));
    assert_eq!(archive.zones(&archive.modules[5]).unwrap().len(), 1, "later Samplers reuse the Zone class");
    let parameters = archive.sampler_parameters(&archive.modules[0], &first).unwrap();
    assert_eq!((parameters.len(), parameters["0xe8"], parameters["0xf8"], parameters["ampEnvelope.3"], parameters["0x190"], parameters.contains_key("0xc0")), (44, 2.0, -0.25, 0.5, 1.0, false));

    assert_eq!(expansion_root(Path::new(&path), None), temp.join("Fixture Library"));
    let imported = import_group(&path, None).unwrap();
    let kick = &imported.sounds[0];
    let zones = kick.instrument["zones"].as_array().unwrap();
    assert_eq!(zones.len(), 2);
    assert_eq!(zones[0]["path"], temp.text("Fixture Library/Samples/Drums/Kick.wav"));
    assert_eq!((zones[0]["rootNote"].as_i64(), zones[0]["keyLow"].as_i64(), zones[0]["keyHigh"].as_i64(), zones[0]["start"].as_f64(), zones[0]["end"].as_f64(), zones[0]["volume"].as_f64()), (Some(36), Some(0), Some(59), Some(0.0), Some(1.0), Some(1.0)), "native ranges include their final frame");
    assert_eq!((zones[1]["start"].as_f64(), zones[1]["end"].as_f64(), zones[1]["tune"].as_f64(), zones[1]["pan"].as_f64(), zones[1]["loopEnabled"].as_bool()), (Some(0.2), Some(1.0), Some(-12.0), Some(0.25), Some(false)));
    assert!((zones[1]["volume"].as_f64().unwrap() - 2.0).abs() < 1e-9 && imported.warnings.iter().any(|warning| warning.contains("sample-zone level exceeded")), "a +10 dB zone is limited to the supported gain");
    assert_eq!(kick.instrument["samplePath"], zones[0]["path"]);
    assert_eq!((kick.instrument["type"].as_str(), kick.instrument["voiceMode"].as_str(), kick.instrument["filterType"].as_str(), kick.instrument["tune"].as_f64()), (Some("sample"), Some("mono"), Some("lowpass"), Some(-9.0)));
    assert!((kick.instrument["sustain"].as_f64().unwrap() - 0.125).abs() < 1e-9 && (kick.instrument["attack"].as_f64().unwrap() - 0.171032).abs() < 1e-4);
    assert!((kick.volume - level_amplitude(0.5).unwrap()).abs() < 1e-12 && kick.pan == 0.25 && !kick.mute);
    // A missing sample keeps its zone, marked; a zone that leaves the library is skipped.
    let second = imported.sounds[1].instrument["zones"].as_array().unwrap();
    assert_eq!((second.len(), second[0]["unavailable"].as_bool()), (1, Some(true)));
    assert!(imported.warnings.iter().any(|warning| warning.contains("../Outside.wav") && warning.contains("leaves its expansion library")));
    assert!(imported.warnings.iter().any(|warning| warning == "Missing or unreadable sample: Samples/Drums/Missing.wav"));
    assert_eq!(status_of(&imported.report, "sample-zones-decoded").len(), 16);
    let project = imported.group_project();
    assert_eq!((project["tracks"][0]["voiceMode"].as_str(), project["instruments"][0].get("voiceMode"), project["tracks"].as_array().unwrap().len()), (Some("mono"), None, 16), "the voice mode is the member track's");
    let saved = st_library::kit::save(&project, &Default::default(), &temp.text("Imported.stkit"), &st_library::kit::SaveOptions { copy_samples: true, ..Default::default() });
    assert!(saved.is_err(), "collecting fails on the missing sample rather than writing a kit without it");

    // Audio modules: their single sample reference becomes a whole-file zone.
    temp.write("Renamed Expansion/Renamed Expansion.nicnt", &[]);
    temp.write("Renamed Expansion/Samples/Loops/Beat.wav", &wav_with(0.1, 44_100, 1));
    let audio_body: Body = Box::new(|module, index| [e(module + 1), if index == 0 { vec![0, 0] } else { vec![] }, e(2), string("//Samples/Loops/Beat.wav"), string("C:/Expansions/Renamed Expansion/"), string("Renamed")].concat());
    let audio = Options { module_type: "AudioModule".to_string(), body: Some(audio_body), ..variant(17, 22, 9) };
    let audio_path = write_group(&temp, "Renamed Expansion/Groups/Kits/Synthetic.mxgrp", &audio);
    assert_eq!(expansion_root(Path::new(&audio_path), Some(temp.path())), temp.join("Renamed Expansion"), "an expansion without a \"Library\" suffix is found by its .nicnt");
    let audio_archive = Archive::parse(&fs::read(&audio_path).unwrap()).unwrap();
    assert_eq!(audio_archive.audio_module_sample(&audio_archive.modules[3]).unwrap().path, "//Samples/Loops/Beat.wav");
    let audio_import = import_group(&audio_path, Some(&temp.text(""))).unwrap();
    let zone = &audio_import.sounds[0].instrument["zones"][0];
    assert!(zone["path"].as_str().unwrap().ends_with("Renamed Expansion/Samples/Loops/Beat.wav") && zone.get("unavailable").is_none());
    assert_eq!((zone["start"].as_f64(), zone["end"].as_f64(), zone["volume"].as_f64()), (Some(0.0), Some(1.0), Some(1.0)), "an Audio module zone covers its whole file at unity");
    assert!(audio_import.warnings.iter().any(|warning| warning.contains("Audio module") && warning.contains("tempo sync")));
    let bad_body: Body = Box::new(|module, _| [e(module + 1), e(2), string("Samples/Loops/Beat.exe"), string(""), string("")].concat());
    let bad_audio = parse(&Options { module_type: "AudioModule".to_string(), body: Some(bad_body), ..variant(17, 22, 9) });
    assert!(bad_audio.audio_module_sample(&bad_audio.modules[0]).is_err(), "a non-audio Audio module reference is refused");

    // REX2 slices play from the bundled WAV rendering only when the slices tile it exactly.
    temp.write("Renamed Expansion/Samples/Loops/Beat.rx2", b"CAT REX2");
    let slice = |start: f64, end: f64| Zone {
        object_id: 1,
        path: "Samples/Loops/Beat.rx2".to_string(),
        origin: String::new(),
        library: String::new(),
        values: [("keyLow", 60.0), ("keyHigh", 60.0), ("velocityLow", 0.0), ("velocityHigh", 127.0), ("root", 60.0), ("pan", 0.0), ("tune", 0.0), ("gain", 0.75), ("loopEnabled", 0.0), ("startFrame", start), ("endFrame", end), ("loopStartFrame", start), ("loopEndFrame", end)].into_iter().collect(),
    };
    let library = temp.join("Renamed Expansion");
    let (rex, warnings) = maschine::map_zones(&[slice(0.0, 2204.0), slice(2205.0, 4409.0)], &library);
    assert!(rex.len() == 2 && rex.iter().all(|zone| zone["path"].as_str().unwrap().ends_with("Beat.wav") && zone.get("unavailable").is_none()) && rex[1]["start"].as_f64() == Some(2205.0 / 4410.0));
    assert!(warnings.iter().any(|warning| warning.contains("REX2")));
    let (too_long, warnings) = maschine::map_zones(&[slice(0.0, 9000.0)], &library);
    assert!(too_long.len() == 1 && too_long[0]["unavailable"] == true && warnings.iter().any(|warning| warning.contains("do not match")));
    let (short, _) = maschine::map_zones(&[slice(0.0, 1000.0), slice(1001.0, 3000.0)], &library);
    assert!(short.iter().all(|zone| zone["unavailable"] == true), "slices on another timeline than the WAV stay unavailable");
}

#[test]
fn sample_references_stay_inside_their_library() {
    let temp = TempDir::new("maschine-resolve");
    let library = temp.path();
    assert_eq!(resolve("Samples/Drum.wav", Path::new("/tmp/expansion")).unwrap().file_name().unwrap(), "Drum.wav");
    for path in ["../other.wav", "/tmp/sample.wav", "C:/sample.wav", "Samples/../../escape.wav"] {
        assert_eq!(resolve(path, Path::new("/tmp/expansion")).unwrap_err().to_string(), "Maschine group: sample path leaves its expansion library.", "{path}");
    }
    // Library-relative references with leading slashes stay inside the expansion.
    for path in ["//Samples/Loops/Beat.wav", "/Samples/Loops/Beat.wav", "Samples/Loops/Beat.wav", "Samples\\Loops\\Beat.wav"] {
        assert_eq!(resolve(path, library).unwrap(), temp.join("Samples/Loops/Beat.wav"), "{path}");
    }
    for path in ["/Users/Shared/Beat.wav", "//Samples/../../Beat.wav", "///Samples/Beat.wav", "/tmp/Samples/Beat.wav"] {
        assert!(resolve(path, library).is_err(), "unsafe path {path} accepted");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(library.parent().unwrap(), temp.join("escaped")).unwrap();
        assert!(resolve("escaped/file.wav", library).is_err(), "a link escaped the expansion");
        symlink("escaped/missing.wav", temp.join("nested")).unwrap();
        assert!(resolve("nested", library).is_err(), "a nested dangling link escaped the expansion");
        symlink("cycle", temp.join("cycle")).unwrap();
        assert!(resolve("cycle/sample.wav", library).unwrap_err().to_string().contains("cycle"));
        symlink("Samples", temp.join("inside")).unwrap();
        assert_eq!(resolve("inside/missing.wav", library).unwrap(), temp.join("Samples/missing.wav"), "a link inside the library keeps its target, also for a missing sample");
    }
}

const IDENTITY: &str = "Fixture Vendor:VST:INSTR:1315523937";
const FLAGS: [bool; 4] = [true, false, false, true];
const PAYLOAD: [u8; 9] = [0, 1, 2, 3, 254, 255, 0, 65, 66];
const OBJECT_ID: i64 = 4000;

struct HostFixture {
    bytes: Vec<u8>,
    chunk_length: std::ops::Range<usize>,
    bools: Vec<std::ops::Range<usize>>,
    ids: Vec<std::ops::Range<usize>>,
}

#[allow(clippy::too_many_arguments)]
fn host(modern: bool, intro: bool, identity: &str, name: &str, chunk_version: u32, chunk_type: u32, payload: &[u8], preset: i64) -> HostFixture {
    let mut bytes = Vec::new();
    let (mut ids, mut bools) = (Vec::new(), Vec::new());
    let mut next = OBJECT_ID + 1;
    for flag in FLAGS {
        for _ in 0..if modern { 4 } else { 3 } {
            ids.push(bytes.len()..bytes.len() + e(next).len());
            bytes.extend(e(next));
            next += 1;
        }
        if modern {
            bytes.push(0);
        }
        for value in [i64::from(flag), i64::from(!flag), i64::from(flag), 0, 1] {
            bools.push(bytes.len()..bytes.len() + e(value).len());
            bytes.extend(e(value));
        }
    }
    bytes.extend([string(identity), string(name)].concat());
    if intro {
        bytes.extend([0, 0]);
    }
    let chunk = [chunk_version.to_le_bytes().to_vec(), chunk_type.to_le_bytes().to_vec(), (payload.len() as u32).to_le_bytes().to_vec(), payload.to_vec()].concat();
    let chunk_length = bytes.len()..bytes.len() + e(chunk.len() as i64).len();
    bytes.extend([e(chunk.len() as i64), chunk, e(preset)].concat());
    HostFixture { bytes, chunk_length, bools, ids }
}

fn standard(modern: bool, intro: bool) -> HostFixture {
    host(modern, intro, IDENTITY, "Prism – Éclat", 1, 3, &PAYLOAD, 257)
}

fn decode(bytes: &[u8], modern: bool) -> st_library::Result<PluginState> {
    PluginState::decode(bytes, OBJECT_ID, modern, 1, 32 * 1024 * 1024)
}

#[test]
fn hosted_plugin_state() {
    for modern in [false, true] {
        for intro in [false, true] {
            for kind in 0..=7u32 {
                let fixture = host(modern, intro, IDENTITY, "Prism – Éclat", 1, kind, &PAYLOAD, 257);
                let decoded = decode(&fixture.bytes, modern).unwrap();
                assert_eq!((decoded.unique_plugin_id.as_str(), decoded.plugin_name.as_str()), (IDENTITY, "Prism – Éclat"), "identity and Unicode name survive exactly");
                assert_eq!((decoded.vst2_unique_id, decoded.is_instrument), (Some(1_315_523_937), Some(true)));
                assert_eq!(decoded.flags, FLAGS, "each flag uses its current value");
                assert_eq!((decoded.chunk_version, decoded.chunk_type, decoded.payload.as_slice(), decoded.preset_index), (1, kind, &PAYLOAD[..], 257));
                let summary = decoded.summary();
                assert!(summary.get("payload").is_none() && summary.get("state").is_none() && summary["requiresHostStateValidation"] == true && summary["stateBytes"] == 9 && summary["kind"] == "instrument");
                // Members after the preset index are not read.
                let with_tail = decode(&[fixture.bytes.clone(), vec![9, 9, 9, 9]].concat(), modern).unwrap();
                assert_eq!((with_tail.payload.as_slice(), with_tail.preset_index), (&PAYLOAD[..], 257));
            }
            // Every truncated required field fails; the two layouts are not interchangeable.
            let fixture = standard(modern, intro);
            for count in 0..fixture.bytes.len() {
                assert!(decode(&fixture.bytes[..count], modern).is_err(), "truncated at {count} (modern {modern}, intro {intro})");
            }
            assert!(decode(&fixture.bytes, !modern).is_err());
        }
    }
    for preset in [0i64, 1, 255, 256, 65_535, 65_536, i64::from(u32::MAX)] {
        assert_eq!(decode(&host(true, false, IDENTITY, "N", 1, 1, &PAYLOAD, preset).bytes, true).unwrap().preset_index as i64, preset);
    }
    // Identities: only the exact tokens classify.
    let effect = decode(&host(true, false, "Fixture Vendor:VST:FX:4294967295", "Instrument-looking name", 1, 3, &PAYLOAD, 0).bytes, true).unwrap();
    assert_eq!((effect.vst2_unique_id, effect.is_instrument, effect.component().as_deref()), (Some(u32::MAX), Some(false), Some("vst2:FFFFFFFF")));
    assert_eq!(decode(&host(true, false, IDENTITY, "FX Effect Processor", 1, 3, &PAYLOAD, 0).bytes, true).unwrap().is_instrument, Some(true), "the display name never overrides the identity token");
    for unknown in ["Fixture Vendor:AU:INSTR:1315523937", "Fixture Vendor:VST3:INSTR:1315523937", "Fixture Vendor:Custom:FX:1315523937"] {
        let decoded = decode(&host(true, false, unknown, "N", 1, 3, &PAYLOAD, 0).bytes, true).unwrap();
        assert_eq!((decoded.vst2_unique_id, decoded.is_instrument, decoded.payload.as_slice()), (None, Some(!unknown.contains(":FX:")), &PAYLOAD[..]), "{unknown}");
    }
    assert_eq!(decode(&host(true, false, "Fixture FX Vendor:VST:UNKNOWN:1315523937", "FX", 1, 3, &PAYLOAD, 0).bytes, true).unwrap().is_instrument, None);
    for malformed in ["NoColons", "A:VST:INSTR", ":VST:INSTR:12", "A:VST:INSTR:12x", "A:VST:INSTR:0", "A:VST:INSTR:99999999999"] {
        let decoded = decode(&host(true, false, malformed, "N", 1, 3, &PAYLOAD, 0).bytes, true).unwrap();
        assert_eq!(decoded.vst2_unique_id, None, "{malformed}");
    }
    // Invalid frames.
    let value = standard(true, false);
    let replacing = |range: &std::ops::Range<usize>, with: &[u8]| [&value.bytes[..range.start], with, &value.bytes[range.end..]].concat();
    for range in &value.ids {
        assert!(decode(&replacing(range, &e(9)), true).is_err(), "every parameter identity matches its expected object ID");
    }
    for range in &value.bools {
        assert!(decode(&replacing(range, &e(2)), true).is_err(), "Boolean members are 0 or 1");
    }
    assert!(decode(&replacing(&value.chunk_length, &e(i64::from(u32::MAX))), true).is_err());
    assert!(decode(&replacing(&value.chunk_length, &e(11)), true).is_err(), "the chunk length includes its fixed header");
    for (version, kind) in [(0, 3), (2, 3), (u32::MAX, 3), (1, 8), (1, u32::MAX)] {
        assert!(decode(&host(true, false, IDENTITY, "N", version, kind, &PAYLOAD, 0).bytes, true).is_err());
    }
    for (identity, name) in [("", "N"), (IDENTITY, ""), (&"x".repeat(10_000) as &str, "N")] {
        assert!(decode(&host(true, false, identity, name, 1, 3, &PAYLOAD, 0).bytes, true).is_err());
    }
    assert!(decode(&host(true, false, IDENTITY, "N", 1, 3, &[], 0).bytes, true).is_err(), "a chunk without payload fails");
    for version in [-1, 2, i64::MAX] {
        assert!(PluginState::decode(&value.bytes, OBJECT_ID, true, version, 1024).is_err());
    }
    assert!(PluginState::decode(&value.bytes, OBJECT_ID, true, 0, 1024).is_err(), "a version 0 PluginHost does not take four-ID parameters");
    let legacy = PluginState::decode(&standard(false, false).bytes, OBJECT_ID, true, 0, 1024).unwrap();
    assert_eq!((legacy.flags.as_slice(), legacy.payload.as_slice(), legacy.preset_index), (&FLAGS[..], &PAYLOAD[..], 257));
    for base in [-1, 0, i64::MAX, i64::from(u32::MAX) - 15] {
        assert!(PluginState::decode(&value.bytes, base, true, 1, 1024).is_err());
    }
    for maximum in [0, 32 * 1024 * 1024 + 1, PAYLOAD.len() - 1] {
        assert!(PluginState::decode(&value.bytes, OBJECT_ID, true, 1, maximum).is_err());
    }
    assert_eq!(PluginState::decode(&value.bytes, OBJECT_ID, true, 1, PAYLOAD.len()).unwrap().payload, PAYLOAD);

    // In a group: the plug-in is kept as a descriptor the host cannot load, and reported.
    let temp = TempDir::new("maschine-plugins");
    // Each Sound's body numbers its parameters from its own Module.
    let body_for_module: Body = Box::new(move |module, index| {
        let mut bytes = Vec::new();
        let mut next = module + 1;
        for flag in FLAGS {
            for _ in 0..3 {
                bytes.extend(e(next));
                next += 1;
            }
            bytes.extend([e(i64::from(flag)), e(0), e(0), e(0), e(1)].concat());
        }
        let chunk = [1u32.to_le_bytes().to_vec(), 1u32.to_le_bytes().to_vec(), (PAYLOAD.len() as u32).to_le_bytes().to_vec(), PAYLOAD.to_vec()].concat();
        [bytes, string(IDENTITY), string("Fixture Synth"), if index == 0 { vec![0, 0] } else { vec![] }, e(chunk.len() as i64), chunk, e(5)].concat()
    });
    let path = write_group(&temp, "Hosted.mxgrp", &Options { module_type: "PluginHost".to_string(), body: Some(body_for_module), ..Default::default() });
    let mut data = fs::read(&path).unwrap();
    // Register the PluginHost class as version 1 in an older archive (three-ID parameters).
    let class = position(&data, b"NI::MASCHINE::DATA::PluginHost", 0) + 30;
    data.splice(class + 2..class + 3, [1, 1]);
    let length = (data.len() as u64).to_le_bytes();
    data[..8].copy_from_slice(&length);
    let archive_length = ((data.len() - 36) as u32).to_le_bytes();
    data[32..36].copy_from_slice(&archive_length);
    fs::write(&path, &data).unwrap();
    let imported = import_group(&path, None).unwrap();
    let sound = &imported.sounds[0];
    assert_eq!((sound.instrument["type"].as_str(), sound.instrument.get("plugin"), sound.plugins.len()), (Some("plugin"), None, 1), "the descriptor is the pad's, not copied onto its instrument");
    let descriptor = &sound.plugins[0];
    assert_eq!((descriptor["component"].as_str(), descriptor["kind"].as_str(), descriptor["missing"].as_bool(), descriptor["name"].as_str()), (Some("vst2:4E694D61"), Some("instrument"), Some(true), Some("Fixture Synth")));
    assert_eq!(st_library::util::base64_decode(descriptor["state"].as_str().unwrap()).unwrap(), PAYLOAD, "the state bytes are kept exactly");
    assert_eq!((descriptor["maschinePluginState"]["presetIndex"].as_u64(), descriptor["maschinePluginState"]["vst2UniqueID"].as_u64()), (Some(5), Some(1_315_523_937)));
    assert!(imported.warnings.iter().any(|warning| warning.contains("VST2 plug-in, which SuperTracker does not load")));
    assert_eq!(status_of(&imported.report, "external-state-retained").len(), 16);
    let project = imported.group_project();
    st_library::kit::group::validate(&project).unwrap();
    assert_eq!(project["audioPlugins"][0]["owner"], format!("track:{}", project["tracks"][0]["routingID"].as_str().unwrap()), "a Sound's plug-in is owned by its track's channel");
}

fn close(actual: Option<f64>, expected: f64, message: &str) {
    let actual = actual.unwrap_or_else(|| panic!("{message}: no mapped value"));
    assert!((actual - expected).abs() < (expected.abs() * 1e-6).max(1e-8), "{message}: {actual} != {expected}");
}

fn warned(mapping: &EffectMapping, text: &str) -> bool {
    mapping.warnings.iter().any(|warning| warning.contains(text))
}

#[test]
fn core_mapping_eq_and_compressor() {
    // Distinct serialized values detect confusion with the order of the controls on screen.
    let raw = [0.0, -0.8, 0.25, -0.4, 0.75, 0.2, 1.0, 0.6, 0.0, 1.0, -0.25];
    let eq = map_core("NI::MASCHINE::DATA::EQ", 0, &raw).unwrap();
    assert_eq!((eq.id.as_str(), eq.parameters.len()), ("stfx:eq", 11));
    for (key, expected) in [("lowfreq", 20.0), ("lowmidfreq", 178.8854382), ("highmidfreq", 3577.708764), ("highfreq", 20000.0), ("lowgain", -16.0), ("lowmidgain", -8.0), ("highmidgain", 4.0), ("highgain", 12.0), ("lowmidwidth", 0.1), ("highmidwidth", 4.0), ("gain", -5.0)] {
        close(eq.physical(key), expected, key);
    }
    let neutral = map_core("EQ", 0, &[0.5, 0.0, 0.5, 0.0, 0.5, 0.0, 0.5, 0.0, 0.5, 0.5, 0.0]).unwrap();
    close(neutral.physical("lowgain"), 0.0, "signed zero is neutral gain");
    close(neutral.physical("lowmidwidth"), 2.05, "width midpoint");
    close(neutral.physical("lowfreq"), 400.0, "log frequency midpoint");
    let compressor = map_core("Compressor", 1, &[1.0, 0.5, 0.8, 0.3, 0.5, 0.5, 0.75]).unwrap();
    close(compressor.physical("mode"), 1.0, "Feedback mode");
    close(compressor.physical("threshold"), -30.0, "inverted threshold");
    close(compressor.physical("attack"), 1.0, "cubic attack");
    close(compressor.physical("gain"), -9.0, "gain");
    assert!((99.0..101.0).contains(&compressor.physical("release").unwrap()));
    assert!(compressor.physical("ratio").is_none() && compressor.physical("knee").is_none() && warned(&compressor, "Amount and Knee"), "unverified ratio and knee are never guessed");
    let old = map_core("Compressor", 0, &[1.0, 0.8, 0.3, 0.0, 0.0, 0.0]).unwrap();
    close(old.physical("mode"), 0.0, "an old archive has no mode");
    close(old.physical("threshold"), -60.0, "version 0 indexing");
    close(old.physical("attack"), 0.1, "zero attack limited to the minimum");
    close(old.physical("gain"), -24.0, "-36 dB limited to the minimum");
    assert_eq!(old.warnings.iter().filter(|warning| warning.contains("limited")).count(), 2, "each limited parameter is disclosed");
    for (kind, version, values) in [("EQ", 1, &raw[..]), ("Compressor", 2, &raw[..]), ("Unknown", 0, &raw[..]), ("EQ", 0, &[][..]), ("Compressor", 1, &[0.0; 6][..])] {
        assert!(map_core(kind, version, values).is_none(), "{kind} {version}");
    }
    for bad in [f64::NAN, f64::INFINITY] {
        let mut invalid = raw;
        invalid[0] = bad;
        assert!(map_core("EQ", 0, &invalid).is_none());
    }
    let mut invalid = raw;
    invalid[0] = -0.1;
    let partial = map_core("EQ", 0, &invalid).unwrap();
    assert!(partial.physical("lowfreq").is_none() && warned(&partial, "verified range"));
    close(partial.physical("highfreq"), 20000.0, "a bad control does not discard valid ones");
    assert_eq!(map_effect("EQ", 0, &raw), Some(eq));
}

#[test]
fn spatial_mapping_reverbs_delays_and_grains() {
    let ice = map_spatial("NI::MASCHINE::DATA::Iceverb", 0, &[0.2, 0.7, 0.9, 0.4]).unwrap();
    assert_eq!(ice.id, "stfx:ice");
    for (key, expected) in [("color", 20.0), ("ice", 70.0), ("mix", 40.0)] {
        close(ice.physical(key), expected, key);
    }
    assert!(!ice.warnings.is_empty());
    let legacy = map_spatial("Reverb", 0, &[2.0, 0.4, 0.5, 0.5, -0.75, 0.8, 0.25, 1.0]).unwrap();
    close(legacy.physical("pan"), -75.0, "signed pan");
    close(legacy.physical("freeze"), 1.0, "freeze");
    close(legacy.physical("mix"), 25.0, "mix");
    assert!(legacy.physical("low").unwrap().abs() < 1e-9 && legacy.physical("high").unwrap().abs() < 1e-9, "the EQ midpoint stays neutral");
    close(map_spatial("Reverb", 0, &[0.0, 0.5, 0.5, 0.5, 0.0, 0.0, 0.5, 0.0]).unwrap().physical("stereo"), 100.0, "zero Stereo keeps normal width");
    close(map_spatial("Reverb", 0, &[0.0, 0.5, 0.5, 0.5, 0.0, -0.5, 0.5, 0.0]).unwrap().physical("stereo"), 50.0, "negative Stereo narrows");
    for unit in 0..3 {
        let delay = map_spatial("BeatDelay", 0, &[3.0, 0.0, 0.4, 0.2, 0.8, 0.3, -1.0, 0.5, unit as f64]).unwrap();
        close(delay.physical("time"), 5.0, "delay time enum");
        close(delay.physical("unit"), [2.0, 5.0, 1.0][unit], "straight, triplet, 32nd order");
        close(delay.physical("stereo"), -100.0, "negative stereo");
    }
    let half = map_spatial("BeatDelay", 0, &[0.0, 2.0, 0.2, 0.2, 0.8, 0.0, 0.0, 0.5, 0.0]).unwrap();
    close(half.physical("time"), 0.0, "half-unit enum");
    assert!(warned(&half, "Offset"));
    let grain = map_spatial("GrainDelay", 0, &[-0.5, 0.25, 0.0, 1.0, 0.2, 0.5, 0.0, 0.3, 1.0]).unwrap();
    close(grain.physical("pitch"), -12.0, "signed grain pitch");
    close(grain.physical("reverse"), 1.0, "reverse");
    for (choice, steps) in [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0].iter().enumerate() {
        close(map_spatial("GrainStretch", 0, &[1.0, 0.5, choice as f64, 0.5, 0.0, 0.5, 1.0]).unwrap().physical("loop"), *steps, "loop length");
    }
    let reso = map_spatial("Resochord", 0, &[1.0, 1.0, 2.0, 0.5, -12.0, 0.7, 0.5, 0.5, 0.3]).unwrap();
    close(reso.physical("mode"), 0.0, "mode order");
    close(reso.physical("style"), 3.0, "minor seventh");
    close(reso.physical("tune"), -12.0, "tune in semitones");
    assert!(reso.physical("chord").is_none(), "the voicing enum never becomes a pitch root");
    close(map_spatial("Resochord", 0, &[0.0, 0.0, 1.0, 0.5, 0.0, 0.7, 0.5, 0.5, 0.3]).unwrap().physical("mode"), 1.0, "String mode");
    let plate = map_spatial("PlateReverb", 0, &[0.4, 0.5, 0.3, 0.5, 0.7]).unwrap();
    assert_eq!(plate.id, "stfx:plate");
    close(plate.physical("predelay"), 100.0, "old Plate order");
    close(plate.physical("mix"), 30.0, "old Plate mix");
    let mut modern = [0.5; 23];
    (modern[2], modern[5], modern[15], modern[16]) = (0.42, 1.0, 0.3, 0.81);
    let hall = map_spatial("PlateReverb", 1, &modern).unwrap();
    assert_eq!(hall.id, "stfx:reverb");
    close(hall.physical("mode"), 1.0, "Hall");
    close(hall.physical("size"), 81.0, "the active parameter bank");
    close(hall.physical("mix"), 42.0, "mix");
    (modern[21], modern[22]) = (0.0, 0.0);
    let flat = map_spatial("PlateReverb", 1, &modern).unwrap();
    assert!((flat.physical("highcut").unwrap() - 20000.0).abs() < 1e-8 && flat.physical("lowshelf").unwrap().abs() < 1e-9, "zero EQ controls stay neutral");
    modern[5] = 2.0;
    assert_eq!(map_spatial("PlateReverb", 1, &modern).unwrap().id, "stfx:plate");
    assert!(map_spatial("Iceverb", 1, &[0.0; 4]).is_none() && map_spatial("PlateReverb", 1, &[0.0; 5]).is_none() && map_spatial("Iceverb", 0, &[f64::NAN, 0.0, 0.0, 0.0]).is_none() && map_spatial("Unknown", 0, &[]).is_none());
    let invalid = map_spatial("Iceverb", 0, &[-1.0, 0.5, 0.5, 0.5]).unwrap();
    assert!(invalid.physical("color").is_none() && warned(&invalid, "outside"));
}

#[test]
fn additional_mapping_dynamics_and_modulation() {
    let map = |kind: &str, raw: &[f64], version: u64| map_additional(kind, version, raw).unwrap_or_else(|| panic!("missing {kind}"));
    let gate = map("NI::MASCHINE::DATA::Gate", &[0.316227766, 0.1, 0.2, 0.3, 0.4], 0);
    close(gate.physical("threshold"), -20.0, "logarithmic threshold");
    close(gate.physical("hold"), 200.0, "hold");
    close(gate.physical("mix"), 40.0, "mix");
    assert!(warned(&gate, "millisecond curves are approximated"));
    assert!(map("Gate", &[0.0, 0.0, 0.0, 0.0, 1.0], 0).physical("threshold").is_none(), "a zero threshold is refused before the logarithm");
    let limiter = map("Limiter", &[-8.0, 1.0, 0.5, 0.75], 1);
    close(limiter.physical("threshold"), -8.0, "raw dB");
    close(limiter.physical("mode"), 1.0, "mode");
    close(limiter.physical("release"), 500f64.sqrt(), "exponential release");
    close(limiter.physical("ceiling"), -10.225, "linear ceiling");
    for endpoint in [0.0, 1.0] {
        let result = map("Limiter", &[-40.0 * endpoint, endpoint, endpoint, endpoint], 1);
        close(result.physical("release"), if endpoint == 0.0 { 1.0 } else { 500.0 }, "release endpoints");
        close(result.physical("ceiling"), if endpoint == 0.0 { -40.0 } else { -0.3 }, "ceiling endpoints");
    }
    let transient = map("TransientDesigner", &[-0.5, -0.25, 0.75, 1.0], 0);
    close(transient.physical("gain"), -12.0, "negative gain curve");
    close(transient.physical("attack"), -25.0, "signed attack");
    close(transient.physical("sustain"), 75.0, "sustain");
    close(map("TransientDesigner", &[0.5, 0.0, 0.0, 0.0], 0).physical("gain"), 3.0, "asymmetric positive gain");
    let cabinet = map("CabinetEmulation", &[3.0, 5.0, 0.2, 0.3], 0);
    for (key, expected) in [("cabinet", 3.0), ("microphone", 5.0), ("distance", 20.0), ("mix", 30.0)] {
        close(cabinet.physical(key), expected, key);
    }
    let filter = map("Filter", &[2.0, 0.5, 0.3, -0.4, 2.0, 0.6, 0.7, 0.8, 0.2, 0.1, 0.9], 0);
    close(filter.physical("mode"), 2.0, "mode");
    close(filter.physical("cutoff"), (46.0f64 * 12540.0).sqrt(), "exponential cutoff");
    close(filter.physical("amount"), -2.0, "signed modulation");
    close(filter.physical("source"), 2.0, "envelope source");
    close(filter.physical("phase"), 288.0, "phase");
    assert!(filter.physical("shape").is_none(), "a continuous LFO shape is not an enum");
    let phaser = map("Phaser", &[0.1, -0.6, 1.0, 0.3, 0.4, 0.5, 0.7, 1.0, 0.9], 0);
    for (key, expected) in [("amount", -60.0), ("source", 1.0), ("feedback", 70.0), ("eightpole", 1.0), ("mix", 90.0)] {
        close(phaser.physical(key), expected, key);
    }
    assert!(warned(&phaser, "four-beat"));
    let flanger = map("Flanger", &[0.2, -0.3, 0.4, 0.5, 0.6, 0.7, 1.0, 0.8], 0);
    for (key, expected) in [("amount", -30.0), ("source", 0.0), ("stereo", 108.0), ("feedback", 70.0), ("invert", 1.0), ("mix", 80.0)] {
        close(flanger.physical(key), expected, key);
    }
    let shift = map("FreqShifter", &[0.6, 0.1, 0.7, 0.3, 1.0, 0.5], 0);
    assert!(shift.physical("coarse").is_none() && shift.physical("fine").is_none() && warned(&shift, "remain at zero"));
    close(shift.physical("feedback"), 70.0, "feedback");
    close(shift.physical("stereo"), 30.0, "stereo");
    let maximizer = map("Maximizer", &[0.2, 0.4, 1.0], 0);
    for (key, expected) in [("amount", 6.0), ("curve", 40.0), ("turbo", 1.0)] {
        close(maximizer.physical(key), expected, key);
    }
    let chorus = map("Chorus", &[0.5, 0.6, 0.7], 0);
    close(chorus.physical("amount"), 60.0, "amount");
    close(chorus.physical("mix"), 70.0, "mix");
    let fm = map("FM", &[0.2, 0.3, 0.4, 0.5], 0);
    close(fm.physical("contour"), 40.0, "contour");
    close(fm.physical("amount"), 50.0, "amount");
    let invalid = map("CabinetEmulation", &[2.5, 1000.0, -1.0, 0.5], 0);
    assert!(invalid.physical("cabinet").is_none() && invalid.physical("microphone").is_none() && invalid.physical("distance").is_none());
    close(invalid.physical("mix"), 50.0, "valid fields survive");
    let clipped = map("FreqShifter", &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0], 0);
    close(clipped.physical("feedback"), 95.0, "feedback limited for stability");
    assert!(warned(&clipped, "limited"));
    let fixtures: [(&str, u64, Vec<f64>); 11] = [
        ("Gate", 0, vec![0.5, 0.0, 0.0, 0.0, 0.0]), ("Limiter", 1, vec![-8.0, 0.0, 0.0, 1.0]), ("Maximizer", 0, vec![0.0; 3]), ("TransientDesigner", 0, vec![0.0; 4]), ("CabinetEmulation", 0, vec![0.0; 4]),
        ("Chorus", 0, vec![0.0; 3]), ("FM", 0, vec![0.0; 4]), ("Filter", 0, vec![0.0; 11]), ("Flanger", 0, vec![0.0; 8]), ("Phaser", 0, vec![0.0; 9]), ("FreqShifter", 0, vec![0.0; 6]),
    ];
    for (kind, version, raw) in fixtures {
        let result = map(kind, &raw, version);
        let addresses: Vec<usize> = result.parameters.iter().map(|(address, _)| *address).collect();
        assert!(addresses.windows(2).all(|pair| pair[0] < pair[1]), "stable unique addresses {kind}");
        assert!(result.parameters.iter().all(|(_, value)| value.is_finite() && (0.0..=1.0).contains(value)) && warned(&result, "independent"), "{kind}");
        assert!(map_additional(kind, version + 10, &raw).is_none() && map_additional(kind, version, &raw[..raw.len() - 1]).is_none() && map_additional(kind, version, &[raw.clone(), vec![0.0]].concat()).is_none(), "{kind}");
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bad_raw = raw.clone();
            bad_raw[0] = bad;
            assert!(map_additional(kind, version, &bad_raw).is_none(), "{kind}");
        }
    }
    assert!(map_additional("Unknown", 0, &[]).is_none());
}

#[test]
fn distortion_mapping_lofi_saturator_and_distortion() {
    let map = |kind: &str, raw: &[f64], version: u64| map_distortion(kind, version, raw).unwrap_or_else(|| panic!("missing {kind}"));
    let single = |bits: u32| f64::from(f32::from_bits(bits));
    let lofi = map("NI::MASCHINE::DATA::LoFi", &[0.0, 0.3, 0.4, 0.6, 0.8], 0);
    for (key, expected) in [("sample_rate", 44100.0), ("smooth", 30.0), ("bits", 7.6), ("stereo", 60.0), ("mix", 80.0)] {
        close(lofi.physical(key), expected, key);
    }
    let degraded = map("LoFi", &[1.0, 1.0, 1.0, 0.0, 1.0], 0);
    close(degraded.physical("sample_rate"), 44100.0 * single(0x3f87_9c63).powf(-105.5), "minimum rate");
    close(degraded.physical("bits"), 1.0, "bits endpoint");
    let saturator = [1.0, 0.1, 0.2, 0.3, 0.5, -0.6, 0.5, 0.7, 3.0, 0.8, 0.4, -7.0, 1.0, 1.0];
    let tape = map("Saturator", &saturator, 0);
    close(tape.physical("input"), 15.0, "Tape input");
    close(tape.physical("drive"), 35.0, "negative Tape drive as tilt");
    close(tape.physical("contour"), 100.0 * 16f64.ln() / 20f64.ln(), "Tape cutoff");
    assert!(tape.physical("output").is_none(), "inactive Tube gain does not overwrite Tape output");
    let mut tube_raw = saturator;
    tube_raw[0] = 2.0;
    let tube = map("Saturator", &tube_raw, 0);
    for (key, expected) in [("mode", 2.0), ("drive", 70.0), ("charge", 75.0), ("output", -7.0), ("overload", 12.0), ("eq_bypass", 1.0)] {
        close(tube.physical(key), expected, key);
    }
    assert!(tube.physical("bass").is_none() && tube.physical("treble").is_none() && warned(&tube, "remain neutral"));
    let mut classic_raw = saturator;
    classic_raw[0] = 0.0;
    let classic = map("Saturator", &classic_raw, 0);
    close(classic.physical("drive"), 20.0, "Classic drive");
    close(classic.physical("contour"), 30.0, "Classic contour");
    assert!(warned(&classic, "approximated"));
    let distortion = [0.0, 0.2, 0.3, 0.4, 1.0, 0.6, 0.7, 0.1, 0.8, -0.5, 0.9, -0.4, 1.0, 0.5, 0.25];
    let fuzz = map("MullhollandDrive", &distortion, 1);
    close(fuzz.physical("drive"), 4.2, "Fuzz drive");
    close(fuzz.physical("mix"), 25.0, "Fuzz mix");
    close(fuzz.physical("feedback"), 0.0, "negative feedback is disabled, not flipped");
    close(fuzz.physical("tone_mod"), 0.0, "negative tone modulation is disabled");
    close(fuzz.physical("gate_release"), 1000.0 * (single(0x3c23_d70a) + (0.25 + (0.0625 - 0.25) * single(0x3f4f_41f2)) * single(0x409f_ae14)), "release quartic");
    assert!(warned(&fuzz, "signed source"));
    let mut analog_raw = distortion;
    analog_raw[0] = 1.0;
    let analog = map("MullhollandDrive", &analog_raw, 1);
    close(analog.physical("drive"), 0.0, "inactive Fuzz drive");
    close(analog.physical("saturation"), 20.0, "Analog saturation");
    close(analog.physical("mix"), 70.0, "Analog mix");
    close(analog.physical("tone"), 150.0 * (16000.0f64 / 150.0).powf(0.4), "Analog tone");
    let mut limited_raw = distortion;
    (limited_raw[13], limited_raw[9]) = (1.0, 1.0);
    let limited = map("MullhollandDrive", &limited_raw, 1);
    close(limited.physical("gate_release"), 2000.0, "release maximum");
    close(limited.physical("feedback"), 95.0, "feedback bound");
    assert!(warned(&limited, "limited"));
    for bad_mode in [-1.0, 0.5, 3.0] {
        let mut invalid = saturator;
        invalid[0] = bad_mode;
        let result = map("Saturator", &invalid, 0);
        assert!(result.parameters.is_empty() && warned(&result, "Unknown mode"), "an invalid mode selects no controls");
    }
    for (kind, version, raw) in [("LoFi", 0, vec![0.0; 5]), ("Saturator", 0, saturator.to_vec()), ("MullhollandDrive", 1, distortion.to_vec())] {
        let result = map(kind, &raw, version);
        assert!(result.parameters.windows(2).all(|pair| pair[0].0 < pair[1].0) && result.parameters.iter().all(|(_, value)| (0.0..=1.0).contains(value)));
        assert!(map_distortion(kind, version + 1, &raw).is_none() && map_distortion(kind, version, &raw[..raw.len() - 1]).is_none() && map_distortion(kind, version, &[raw.clone(), vec![0.0]].concat()).is_none());
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut invalid = raw.clone();
            invalid[1] = bad;
            assert!(map_distortion(kind, version, &invalid).is_none());
        }
    }
    assert!(map_distortion("Unknown", 0, &[]).is_none());
}

#[test]
fn drum_mapping() {
    let raw = |count: usize, engine: f64| {
        let mut values = vec![0.5; count];
        values[0] = engine;
        values
    };
    let near = |actual: f64, expected: f64, message: &str| assert!((actual - expected).abs() < 0.00001, "{message}: {actual} != {expected}");
    for (kind, version, count, model) in [("Kick", 2, 31, "kick"), ("Snare", 1, 22, "snare"), ("Hihat", 2, 18, "hihat"), ("Tom", 1, 23, "tom"), ("Perc", 1, 29, "perc"), ("Cymbal", 0, 13, "cymbal")] {
        let result = map_drum(&format!("NI::MASCHINE::DATA::{kind}"), version, &raw(count, 0.0)).unwrap();
        assert_eq!(result.drum_model, model);
        let (_, info) = catalogue::drum_model(model).unwrap();
        for parameter in [1, 2, 4, 5, 6, 7] {
            near(result.drum_parameters[parameter], catalogue::normalized(&info.parameters[parameter], info.parameters[parameter].default), "unverified controls keep their defaults");
        }
        assert!(result.warnings.iter().any(|warning| warning.contains("millisecond")), "no implicit percent-to-ms mapping");
        assert!(map_drum(kind, version + 1, &raw(count, 0.0)).is_none() && map_drum(kind, version, &raw(count - 1, 0.0)).is_none());
    }
    let mut kick = raw(31, 1.0);
    (kick[1], kick[2], kick[3], kick[4]) = (0.75, 1.0, 0.13, 0.81);
    let mapped = map_drum("Kick", 2, &kick).unwrap();
    near(mapped.physical(0), 6.0, "Tune in semitones around the midpoint");
    near(mapped.physical(3), 81.0, "Impact is serializer slot 4");
    near(mapped.physical(1), 350.0, "a percentage Decay does not overwrite the default in ms");
    assert_eq!((mapped.mapped_parameters.clone(), mapped.original_engine), (vec![0, 3], 1));
    (kick[0], kick[1]) = (7.0, 1.0);
    near(map_drum("Kick", 2, &kick).unwrap().physical(0), 6.0, "the acoustic Kick's narrower span");
    for engine in [2.0, 6.0, 8.0, 9.0] {
        kick[0] = engine;
        near(map_drum("Kick", 2, &kick).unwrap().physical(0), 0.0, "an unknown Tune law is not read as semitones");
    }
    kick[0] = 0.0;
    near(map_drum("Kick", 2, &kick).unwrap().physical(3), 50.0, "the Sub engine's inactive Impact is not imported");
    let mut snare = raw(22, 5.0);
    (snare[1], snare[4]) = (0.0, 0.27);
    let mapped = map_drum("Snare", 1, &snare).unwrap();
    near(mapped.physical(0), -9.0, "the Snare's 18 semitone span");
    near(mapped.physical(3), 27.0, "active Impact");
    for engine in [0.0, 1.0, 8.0, 9.0] {
        snare[0] = engine;
        near(map_drum("Snare", 1, &snare).unwrap().physical(3), 50.0, "inactive Snare Impact");
    }
    let mut tom = raw(23, 3.0);
    tom[1] = 1.0;
    near(map_drum("Tom", 1, &tom).unwrap().physical(0), 7.0, "the Tom's 14 semitone span");
    let mut perc = raw(29, 0.0);
    (perc[1], perc[4], perc[25], perc[26]) = (0.0, 0.14, 0.29, 0.83);
    let mapped = map_drum("Perc", 1, &perc).unwrap();
    near(mapped.physical(0), -24.0, "the Perc's 48 semitone span");
    near(mapped.physical(3), 83.0, "Perc Impact is index 26");
    let mut cymbal = raw(13, 0.0);
    cymbal[3] = 0.42;
    near(map_drum("Cymbal", 0, &cymbal).unwrap().physical(3), 42.0, "Cymbal Impact");
    let unknown = map_drum("Kick", 2, &raw(31, 99.0)).unwrap();
    assert!(unknown.mapped_parameters.is_empty() && unknown.warnings.iter().any(|warning| warning.contains("Unknown original engine")));
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut values = raw(31, 0.0);
        values[6] = bad;
        assert!(map_drum("Kick", 2, &values).is_none());
    }
    for bad in [-1.0, 0.5, f64::from(i32::MAX) + 1.0] {
        assert!(map_drum("Kick", 2, &raw(31, bad)).is_none(), "an invalid engine value is refused");
    }
    let mut invalid = raw(31, 1.0);
    (invalid[1], invalid[4]) = (2.0, -0.1);
    let mapped = map_drum("Kick", 2, &invalid).unwrap();
    near(mapped.physical(0), 0.0, "out-of-range Tune keeps the default");
    near(mapped.physical(3), 50.0, "out-of-range Impact keeps the default");
    assert_eq!(mapped.warnings.iter().filter(|warning| warning.contains("verified range")).count(), 2);
    assert!(map_drum("Sampler", 0, &[]).is_none());
}

#[test]
fn sampler_mapping() {
    for (raw, expected) in [(0.0, 0.0), (0.25, 0.050305371172726154), (0.5, 0.26670458912849426), (0.75, 1.0), (1.0, 3.1622776985168457)] {
        assert!((level_amplitude(raw).unwrap() - expected).abs() <= 0.0000002, "level law at {raw}");
    }
    let levels: Vec<f64> = (0..=100).map(|n| level_amplitude(f64::from(n) / 100.0).unwrap()).collect();
    assert!(levels.windows(2).all(|pair| pair[0] < pair[1]), "the fader curve is strictly increasing");
    assert!([-0.01, 1.01, f64::INFINITY, f64::NAN].iter().all(|value| level_amplitude(*value).is_none()));
    let mut raw: BTreeMap<String, f64> = [("0xe8", 2.0), ("0xf8", -0.25), ("0xc8", 1.0), ("0x148", 1.0), ("0x150", 0.5), ("0x158", 0.2), ("0x190", 1.0)].into_iter().map(|(key, value)| (key.to_string(), value)).collect();
    for prefix in ["ampEnvelope", "filterEnvelope"] {
        for index in 0..9 {
            raw.insert(format!("{prefix}.{index}"), 0.0);
        }
    }
    for (key, value) in [("ampEnvelope.0", 0.5), ("ampEnvelope.2", 1.0), ("ampEnvelope.3", 0.5), ("ampEnvelope.4", 0.25), ("filterEnvelope.0", 0.25), ("filterEnvelope.2", 0.5), ("filterEnvelope.3", 0.75), ("filterEnvelope.4", 1.0)] {
        raw.insert(key.to_string(), value);
    }
    let result = map_sampler(&raw);
    let field = |mapping: &st_library::maschine::mapping::SamplerMapping, key: &str| mapping.instrument.get(key).and_then(Value::as_f64);
    let near = |actual: Option<f64>, expected: f64, tolerance: f64, message: &str| {
        let actual = actual.unwrap_or_else(|| panic!("{message}: no value"));
        assert!((actual - expected).abs() <= (expected.abs() * tolerance).max(tolerance), "{message}: {actual} != {expected}");
    };
    near(field(&result, "tune"), -9.0, 1e-5, "tune is 36 semitones per unit");
    assert_eq!(result.instrument["voiceMode"], "mono");
    near(field(&result, "sustain"), 0.125, 1e-5, "cubic amp sustain");
    near(field(&result, "filterSustain"), 0.421875, 1e-5, "the second envelope is independent");
    near(field(&result, "attack"), 0.171032, 1e-4, "attack midpoint is about 171 ms");
    near(field(&result, "decay"), 12.31959, 1e-4, "maximum decay is about 12.32 s");
    let release = field(&result, "release").unwrap();
    assert!(release > 0.05 && release < 0.1, "quarter release uses the recovered curve");
    assert!(field(&result, "filterAttack").unwrap() < field(&result, "attack").unwrap() && field(&result, "filterRelease") == field(&result, "decay"));
    assert!(result.instrument.get("volume").is_none(), "velocity-to-volume never overwrites the instrument gain");
    assert_eq!(result.instrument["filterType"], "lowpass");
    let cutoff = field(&result, "cutoff").unwrap();
    assert!(cutoff > 950.0 && cutoff < 1100.0, "the low-pass cutoff midpoint is physically converted: {cutoff}");
    near(field(&result, "resonance"), 0.2, 1e-5, "resonance");
    assert!(result.warnings.iter().any(|warning| warning.contains("resonance") && warning.contains("approximation")));
    assert_eq!(result.physical["ampEnvelopeHoldSeconds"], 0.0);
    for mode in [0.0, 1.0, 2.0] {
        raw.insert("0xe8".to_string(), mode);
        let mapped = map_sampler(&raw);
        if mode == 0.0 {
            assert_eq!((field(&mapped, "attack"), field(&mapped, "sustain")), (Some(0.0), Some(1.0)), "one-shot ignores the stored envelope");
        }
        if mode == 1.0 {
            assert_eq!(field(&mapped, "sustain"), Some(0.0), "AHD approximates AD without sustain");
        }
        assert!(!mapped.warnings.is_empty());
    }
    raw.insert("0xe8".to_string(), 2.0);
    raw.insert("0x148".to_string(), 3.0);
    let highpass = map_sampler(&raw);
    assert!(highpass.instrument["filterType"] == "highpass" && field(&highpass, "cutoff").unwrap() < cutoff);
    for mode in [2.0, 4.0] {
        raw.insert("0x148".to_string(), mode);
        assert!(map_sampler(&raw).instrument.get("cutoff").is_none(), "band-pass and EQ modes are not invented");
    }
    for (key, value) in [("0xc8", 0.0), ("0xf0", 1.0), ("filterEnvelope.8", 0.7), ("unrecognizedFutureField", 0.37)] {
        raw.insert(key.to_string(), value);
    }
    let preserved = map_sampler(&raw);
    assert_eq!((preserved.source_parameters["filterEnvelope.8"].as_f64(), preserved.source_parameters["unrecognizedFutureField"].as_f64()), (Some(0.7), Some(0.37)));
    assert!(preserved.instrument["voiceMode"] == "legato" && preserved.instrument.get("mode").is_none(), "a reversed one-shot does not become a reverse loop");
    for (key, value) in [("0xf8", f64::INFINITY), ("0x100", 1e300), ("ampEnvelope.0", -0.2)] {
        raw.insert(key.to_string(), value);
    }
    let invalid = map_sampler(&raw);
    assert!(invalid.instrument.get("tune").is_none() && invalid.instrument.get("attack").is_none() && !invalid.physical.contains_key("startOffsetFraction"));
    assert!(invalid.source_parameters["0xf8"].is_string(), "a non-finite source value stays as diagnostic text");
}
