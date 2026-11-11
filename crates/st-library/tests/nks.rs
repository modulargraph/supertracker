//! NKS preset files: metadata, plug-in identity, controller pages and state bytes.

mod common;

use common::TempDir;
use serde_json::{json, Value};
use st_library::nks::fixtures::{chunk, metadata_chunk, riff, word};
use st_library::nks::msgpack::pack;
use st_library::nks::{catalog_summary, NksHostPreset, NksMetadata};
use std::collections::HashSet;

fn summary() -> Value {
    json!({ "name": "Night Keys", "author": "Test Author", "vendor": "Test Vendor", "types": [["Keys", "Electric Piano"]], "modes": ["Warm", "Soft"], "bankchain": ["Factory"] })
}

fn metadata_fixture() -> Vec<u8> {
    let control = json!({ "ni8": [[{ "id": 42, "name": "Tone", "section": "Main" }]] });
    let plugin = json!({ "VST.magic": 0x4b61_7431u32 });
    let mut chunks = metadata_chunk("NISI", &summary());
    chunks.extend(metadata_chunk("NICA", &control));
    chunks.extend(metadata_chunk("PLID", &plugin));
    chunks.extend(chunk("PCHK", &[1, 2, 3]));
    riff(&chunks)
}

#[test]
fn metadata_of_a_preset() {
    let temp = TempDir::new("nks-metadata");
    let path = temp.write("Fixture.nksf", &metadata_fixture());
    let metadata = NksMetadata::read(&path).unwrap();
    assert_eq!(metadata.name, "Night Keys");
    assert_eq!(metadata.categories, ["Keys / Electric Piano"], "hierarchical categories");
    assert_eq!(metadata.type_paths, [["Keys", "Electric Piano"]]);
    assert_eq!(metadata.tags, ["Warm", "Soft"]);
    assert_eq!(metadata.banks, ["Factory"]);
    assert_eq!((metadata.product.as_deref(), metadata.bank.as_deref()), (Some("Factory"), None));
    assert_eq!((metadata.author.as_deref(), metadata.vendor.as_deref()), (Some("Test Author"), Some("Test Vendor")));
    assert!(metadata.has_plugin_state, "state presence without decoding");
    assert_eq!(metadata.chunks, ["NISI", "NICA", "PLID", "PCHK"]);
    assert_eq!(metadata.chunk_versions.get("NISI"), Some(&1));
    let pages = metadata.controller_pages.as_ref().unwrap();
    assert_eq!((pages.len(), pages[0].name.as_str()), (1, "Main"));
    assert_eq!(pages[0].parameters[0], json!({ "id": 42, "name": "Tone", "section": "Main", "slot": 0, "requiresPluginParameterMapping": true }));
    assert_eq!(metadata.plugin_id, Some(json!({ "VST.magic": 0x4b61_7431u32 })));
    assert!(metadata.warnings.is_empty(), "{:?}", metadata.warnings);

    // The page receives it as JSON with these keys.
    let value = serde_json::to_value(&metadata).unwrap();
    assert_eq!(value["hasPluginState"], true);
    assert_eq!(value["directAUPresetLoadingSupported"], false);
    assert_eq!(value["rawMetadata"]["name"], "Night Keys");
    assert_eq!(value["controllerPages"][0]["parameters"][0]["slot"], 0);
    assert_eq!(value["pluginID"]["VST.magic"], 0x4b61_7431u32);
    assert!(value.get("previewPath").is_none() && value.get("UUID").is_none());

    // A summary without the version word is read too.
    let raw = temp.write("Raw.nksfx", &riff(&chunk("NISI", &pack(&summary()))));
    assert_eq!(NksMetadata::read(&raw).unwrap().name, "Night Keys", "unprefixed map");

    // A preview next to the preset is found; the catalogue summary reads the same file.
    temp.write(".previews/Fixture.nksf.ogg", &[0]);
    assert_eq!(NksMetadata::read(&path).unwrap().preview_path, Some(temp.text(".previews/Fixture.nksf.ogg")));
    let brief = catalog_summary(&path).unwrap();
    assert_eq!((brief.name.as_str(), brief.product.as_str(), brief.format.as_str(), brief.has_controls), ("Night Keys", "Factory", "VST2", true));
    assert_eq!(brief.characters, ["Warm", "Soft"]);
}

#[test]
fn malformed_containers_are_rejected() {
    let temp = TempDir::new("nks-malformed");
    let fixture = metadata_fixture();
    let mut bad_length = fixture.clone();
    bad_length[16..20].copy_from_slice(&word(u32::MAX));
    let malformed: [Vec<u8>; 4] = [Vec::new(), b"not a preset".to_vec(), fixture[..fixture.len() - 1].to_vec(), bad_length];
    for (index, bytes) in malformed.iter().enumerate() {
        let path = temp.write(&format!("Malformed-{index}.nksf"), bytes);
        assert!(NksMetadata::read(&path).is_err(), "malformed RIFF {index}");
    }
    assert!(NksMetadata::read(&temp.text("Missing.nksf")).is_err());

    // A broken metadata chunk is a warning, not a failure; the file name becomes the name.
    let path = temp.write("Broken Summary.nksf", &riff(&chunk("NISI", &[1, 0, 0, 0, 0xc1])));
    let metadata = NksMetadata::read(&path).unwrap();
    assert_eq!(metadata.name, "Broken Summary");
    assert_eq!(metadata.warnings.len(), 2, "{:?}", metadata.warnings);
    // Bytes after the container and duplicate chunks are reported.
    let mut trailing = riff(&[metadata_chunk("NISI", &summary()), metadata_chunk("NISI", &summary())].concat());
    trailing.extend_from_slice(b"junk");
    let metadata = NksMetadata::read(&temp.write("Trailing.nksf", &trailing)).unwrap();
    assert!(metadata.warnings.iter().any(|w| w.contains("after the declared RIFF")) && metadata.warnings.iter().any(|w| w.contains("Duplicate NISI")));
}

fn identity() -> Value {
    json!({ "VST3.uid": [0x5653544Eu32, 0x6924486Du32, 0x61737369u32, 0x76652078u32], "VST.magic": 0, "pluginName": "Fixture Synth", "pluginVendor": "Fixture Vendor" })
}

fn mapping() -> Value {
    json!({ "ni8": [[
        { "id": 0, "name": "Cutoff", "section": "Tone" },
        { "id": 9001, "name": "Resonance" },
        { "name": "Empty" },
        { "id": -1, "name": "Invalid" },
        { "id": 99, "name": "Unavailable" }
    ]] })
}

const NATIVE_STATE: [u8; 13] = [2, 0, 0, 0, 3, 0, 0, 0, 1, 2, 3, 4, 5];

struct Fixture {
    plid: Value,
    plid_version: u32,
    state: Vec<u8>,
    version: u32,
    nica_version: u32,
    extra: Vec<u8>,
    with_state: bool,
}

impl Default for Fixture {
    fn default() -> Self {
        Fixture { plid: identity(), plid_version: 1, state: NATIVE_STATE.to_vec(), version: 1, nica_version: 1, extra: Vec::new(), with_state: true }
    }
}

impl Fixture {
    fn bytes(&self) -> Vec<u8> {
        let versioned = |version: u32, value: &Value| [word(version), pack(value)].concat();
        let mut chunks = chunk("NISI", &versioned(1, &json!({ "name": "Preset Fixture" })));
        chunks.extend(chunk("PLID", &versioned(self.plid_version, &self.plid)));
        chunks.extend(chunk("NICA", &versioned(self.nica_version, &mapping())));
        if self.with_state {
            chunks.extend(chunk("PCHK", &[word(self.version), self.state.clone()].concat()));
        }
        chunks.extend_from_slice(&self.extra);
        riff(&chunks)
    }
}

#[test]
fn host_preset_identity_state_and_pages() {
    let temp = TempDir::new("nks-host");
    let file = temp.write("Fixture.nksf", &Fixture::default().bytes());
    let preset = NksHostPreset::read(&file).unwrap();
    assert_eq!(preset.identity.vst3_class_id.as_deref(), Some("5653544E6924486D6173736976652078"), "class ID word order");
    assert_eq!((preset.identity.vst2_magic, preset.identity.name.as_deref(), preset.identity.vendor.as_deref()), (None, Some("Fixture Synth"), Some("Fixture Vendor")));
    assert_eq!((preset.state.as_slice(), preset.state_version), (&NATIVE_STATE[..], 1), "only the PCHK version word is removed; vendor bytes are preserved");
    assert_eq!((preset.controller_pages.len(), preset.controller_pages[0].name.as_str()), (1, "Tone"));
    assert_eq!(preset.controller_pages[0].slots[0].parameter_id, Some(0), "zero is a valid parameter ID");
    assert_eq!(preset.controller_pages[0].slots[3].parameter_id, None, "an invalid negative ID stays unbound");
    assert_eq!(preset.name, "Preset Fixture");

    let available: HashSet<u32> = [0, 9001].into_iter().collect();
    let bindings = preset.validated_mappings("5653544e6924486d6173736976652078", &available);
    let controls = &bindings[0].parameters;
    assert!(controls[0].mapped && controls[1].mapped, "ID-based binding");
    assert_eq!((controls[2].mapping_status.as_str(), controls[4].mapping_status.as_str()), ("unassigned", "parameterUnavailable"));
    let wrong = preset.validated_mappings(&"A".repeat(32), &available);
    assert!(!wrong[0].parameters[0].mapped && wrong[0].parameters[0].mapping_status == "identityMismatch", "another plug-in cannot claim mappings");

    let summary = serde_json::to_value(preset.summary()).unwrap();
    assert!(summary.get("state").is_none(), "the summary excludes binary state");
    assert_eq!(summary["stateBytes"], 13);
    assert_eq!(summary["stateEncoding"], "nativePluginState");
    assert_eq!(summary["identity"], json!({ "format": "vst3", "vst3ClassID": "5653544E6924486D6173736976652078", "name": "Fixture Synth", "vendor": "Fixture Vendor" }));
    assert_eq!(summary["controllerPages"][0]["parameters"][1], json!({ "slot": 1, "name": "Resonance", "parameterID": 9001 }));
    assert_eq!(serde_json::to_value(&bindings).unwrap()[0]["parameters"][0], json!({ "slot": 0, "name": "Cutoff", "section": "Tone", "parameterID": 0, "mapped": true, "mappingStatus": "mapped" }));

    // A VST2 identity binds to the vendor's VST2-compatible VST3 class only.
    let legacy = NksHostPreset::read(&temp.write("Legacy.nksf", &Fixture { plid: json!({ "VST.magic": 0x4E69_2448u32 }), ..Default::default() }.bytes())).unwrap();
    assert_eq!((legacy.identity.vst3_class_id.as_deref(), legacy.identity.vst2_magic, legacy.identity.format.as_str()), (None, Some(0x4E69_2448), "vst2"));
    assert!(!legacy.warnings.is_empty());
    assert!(legacy.validated_mappings("5653544E6924486D6173736976652078", &available)[0].parameters[0].mapped);
    assert!(!legacy.validated_mappings("41727475415649534478373150726F63", &available)[0].parameters[0].mapped, "an unrelated VST3 class never binds");
    assert!(st_library::nks::is_vst2_compatible("417274750000000044783731FFFFFFFF", 0x4478_3731), "Arturia stores the VST2 ID in the third word");

    let signed = NksHostPreset::read(&temp.write("Signed.nksf", &Fixture { plid: json!({ "VST3.uid": [-1, i32::MIN, 0, 1] }), ..Default::default() }.bytes())).unwrap();
    assert_eq!(signed.identity.vst3_class_id.as_deref(), Some("FFFFFFFF800000000000000000000001"), "signed words keep their exact bits");
    let unknown_mapping = NksHostPreset::read(&temp.write("Nica2.nksf", &Fixture { nica_version: 2, ..Default::default() }.bytes())).unwrap();
    assert!(unknown_mapping.controller_pages.is_empty() && !unknown_mapping.warnings.is_empty(), "future NICA versions are not mapped");

    assert_eq!(std::fs::read(&file).unwrap(), Fixture::default().bytes(), "the preset file is never written");
}

#[test]
fn host_preset_rejects_what_it_cannot_load_exactly() {
    let temp = TempDir::new("nks-host-reject");
    let mut serial = 0;
    let mut rejects = |bytes: Vec<u8>, message: &str, limit: usize| {
        serial += 1;
        let path = temp.write(&format!("Reject{serial}.nksf"), &bytes);
        assert!(NksHostPreset::read_with_limit(&path, limit).is_err(), "{message}");
    };
    rejects(Fixture { version: 2, ..Default::default() }.bytes(), "future PCHK version", 1024);
    rejects(Fixture { state: Vec::new(), ..Default::default() }.bytes(), "empty state", 1024);
    rejects(Fixture::default().bytes(), "state memory limit", 4);
    rejects(Fixture::default().bytes(), "invalid memory limit", 0);
    let second_state = chunk("PCHK", &[word(1), NATIVE_STATE.to_vec()].concat());
    rejects(Fixture { extra: second_state, ..Default::default() }.bytes(), "two state chunks", 1024);
    let second_identity = chunk("PLID", &[word(1), pack(&identity())].concat());
    rejects(Fixture { extra: second_identity, ..Default::default() }.bytes(), "two identities", 1024);
    rejects(Fixture { plid: json!({ "pluginName": "Unknown" }), ..Default::default() }.bytes(), "a name cannot replace a stable identity", 1024);
    for bad in [json!([1, 2, 3]), json!([0, 0, 0, 0]), json!([true, 1, 2, 3]), json!([0.5, 1, 2, 3]), json!([4_294_967_296u64, 1, 2, 3])] {
        rejects(Fixture { plid: json!({ "VST3.uid": bad }), ..Default::default() }.bytes(), "malformed VST3 class ID", 1024);
    }
    let mut truncated = Fixture::default().bytes();
    truncated.pop();
    rejects(truncated, "truncated RIFF or padding", 1024);
    rejects(Fixture { with_state: false, ..Default::default() }.bytes(), "absent state", 1024);
    rejects(Fixture { plid_version: 2, ..Default::default() }.bytes(), "future PLID version", 1024);
    assert_eq!(NksHostPreset::read(&temp.text("nowhere.nksf")).unwrap_err().to_string(), "The NKS preset must be a readable regular file.");
}
