//! The built-in effect catalogue as JSON, in the shape of the `builtin.effects` bridge command:
//! the fixture the interface tests read (`tests/helpers/builtin-effects.js`).
//!
//! `tests/fixtures/builtin-effects.json` is checked in. `the_fixture_is_up_to_date` fails when
//! the catalogue no longer produces exactly its bytes; after a deliberate change to an effect's
//! metadata, write it again with
//!
//! ```sh
//! UPDATE_EFFECT_METADATA=1 cargo test -p tracker-engine --test effects_metadata
//! ```
//!
//! The format is the one of the C++ program that used to print this file (numbers as C's
//! `%.17g`), so the bytes did not change when the engine moved to Rust.
#![cfg(feature = "effects")]

use std::ffi::CStr;
use std::path::PathBuf;
use tracker_engine::effects::catalogue;
use tracker_engine::effects::common::{EFFECT_SIDECHAIN, PARAMETER_INTEGER, PARAMETER_LOG, PARAMETER_TOGGLE};

/// A JSON string: quotes and backslashes escaped (the catalogue's texts hold nothing else that
/// needs it, which `the_texts_need_no_further_escaping` checks).
fn quote(text: &CStr) -> String {
    let mut out = String::from("\"");
    for character in text.to_str().expect("catalogue texts are UTF-8").chars() {
        if character == '"' || character == '\\' {
            out.push('\\');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// C's `%.17g`: 17 significant digits, fixed notation for decimal exponents -4..=16 and
/// scientific otherwise, trailing zeros removed.
fn general(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    assert!(value.is_finite(), "the catalogue holds finite numbers");
    // Correctly rounded to 17 significant digits: d.dddddddddddddddde<exponent>.
    let scientific = format!("{value:.16e}");
    let (mantissa, exponent) = scientific.split_once('e').expect("an exponent");
    let exponent: i32 = exponent.parse().expect("a decimal exponent");
    let trimmed = |digits: &str| -> String {
        if digits.contains('.') {
            digits.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            digits.to_string()
        }
    };
    if !(-4..17).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trimmed(mantissa), exponent.abs())
    } else {
        trimmed(&format!("{value:.*}", (16 - exponent) as usize))
    }
}

/// The catalogue as the JSON document of the fixture, newline included.
fn catalogue_json() -> String {
    let truth = |flag: bool| if flag { "true" } else { "false" };
    let mut out = String::from("[");
    for (index, effect) in catalogue().enumerate() {
        let info = &effect.info;
        if index != 0 {
            out.push(',');
        }
        let id = format!("\"stfx:{}", &quote(info.id())[1..]);
        out += &format!(
            "{{\"id\":{id},\"name\":{},\"category\":{},\"sidechain\":{},\"parameters\":[",
            quote(info.name()),
            quote(info.category()),
            truth(info.flags & EFFECT_SIDECHAIN != 0)
        );
        for (address, parameter) in effect.params.iter().enumerate() {
            if address != 0 {
                out.push(',');
            }
            out += &format!(
                "{{\"address\":{address},\"id\":{},\"name\":{},\"unit\":{},\"min\":{},\"max\":{},\"defaultValue\":{},\"logarithmic\":{},\"integer\":{},\"toggle\":{},\"labels\":[",
                quote(parameter.id()),
                quote(parameter.name()),
                quote(parameter.unit()),
                general(parameter.minimum),
                general(parameter.maximum),
                general(parameter.default_value),
                truth(parameter.flags & PARAMETER_LOG != 0),
                truth(parameter.flags & PARAMETER_INTEGER != 0),
                truth(parameter.flags & PARAMETER_TOGGLE != 0)
            );
            let labels: Vec<String> = (0..parameter.label_count()).filter_map(|label| parameter.label(label)).map(quote).collect();
            out += &labels.join(",");
            out += "]}";
        }
        out += "]}";
    }
    out += "]\n";
    out
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/builtin-effects.json")
}

#[test]
fn the_fixture_is_up_to_date() {
    let path = fixture_path();
    let json = catalogue_json();
    if std::env::var_os("UPDATE_EFFECT_METADATA").is_some_and(|value| value == "1") {
        std::fs::create_dir_all(path.parent().expect("a fixtures folder")).expect("the fixtures folder can be created");
        std::fs::write(&path, &json).expect("the fixture can be written");
        println!("wrote {} ({} bytes)", path.display(), json.len());
        return;
    }
    let fixture = std::fs::read_to_string(&path).unwrap_or_else(|problem| panic!("{} cannot be read: {problem}", path.display()));
    assert!(
        fixture == json,
        "{} is out of date ({} bytes, the catalogue gives {}): run UPDATE_EFFECT_METADATA=1 cargo test -p tracker-engine --test effects_metadata",
        path.display(),
        fixture.len(),
        json.len()
    );
}

#[test]
fn numbers_are_printed_as_percent_17g() {
    for (value, expected) in [
        (0.0, "0"),
        (1.0, "1"),
        (-60.0, "-60"),
        (20000.0, "20000"),
        (0.1, "0.10000000000000001"),
        (-0.3, "-0.29999999999999999"),
        (0.03125, "0.03125"),
        (20.2, "20.199999999999999"),
        (0.0001, "0.0001"),
        (0.00001, "1.0000000000000001e-05"),
        (1e16, "10000000000000000"),
        (1e17, "1e+17"),
        (123456789012345680000.0, "1.2345678901234568e+20"),
        (2.5e-7, "2.4999999999999999e-07"),
    ] {
        assert_eq!(general(value), expected, "{value:e}");
    }
}

#[test]
fn the_texts_need_no_further_escaping() {
    for effect in catalogue() {
        let mut texts = vec![effect.info.id(), effect.info.name(), effect.info.category()];
        for parameter in effect.params {
            texts.extend([parameter.id(), parameter.name(), parameter.unit()]);
            texts.extend((0..parameter.label_count()).filter_map(|label| parameter.label(label)));
        }
        for text in texts {
            let text = text.to_str().expect("catalogue texts are UTF-8");
            assert!(text.chars().all(|character| !character.is_control()), "{text:?} holds a control character");
        }
    }
}
