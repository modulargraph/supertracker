//! What a file's path says about it: pack, category, tags, tempo, key, one-shot or loop, and
//! which collection (Loopcloud, Native Instruments, Maschine expansion) it belongs to.
//!
//! Pure functions over path text; no file is opened. The rules follow the naming of Maschine
//! expansions and Loopcloud packs and are deliberately conservative: a number or a letter is a
//! tempo or a key only where that reading is unambiguous.

use crate::util::{components, file_name, fold, is_inside, parent};

/// Categories and the words that select them, most specific first.
pub const CATEGORY_RULES: &[(&str, &[&str])] = &[
    ("Kick", &["kick", "kicks", "kickdrum", "bassdrum", "bd"]),
    ("Snare", &["snare", "snares", "snr", "sd", "rim", "rimshot"]),
    ("Clap", &["clap", "claps", "snap", "snaps", "fingersnap"]),
    ("Hi-hat", &["hihat", "hihats", "closedhat", "openhat", "hat", "hats", "hh", "chh", "ohh", "closedhh", "openhh", "hhclosed", "hhopen"]),
    ("Cymbal", &["cymbal", "cymbals", "crash", "ride", "splash", "china"]),
    ("Tom", &["tom", "toms"]),
    (
        "Percussion",
        &[
            "percussion", "perc", "percs", "conga", "congas", "bongo", "bongos", "shaker", "shakers", "tambourine", "tamb", "cowbell", "triangle",
            "guiro", "cabasa", "woodblock", "clave", "claves", "timbale", "timbales", "djembe", "tabla", "cajon", "metallic", "wooden",
        ],
    ),
    ("Drums", &["drum", "drums", "break", "breaks", "breakbeat", "kit", "top", "tops", "drumloop", "groove", "grooves"]),
    ("Bass", &["bass", "sub", "reese", "basses"]),
    (
        "Vocals",
        &["vocal", "vocals", "voice", "voices", "vox", "voc", "choir", "spoken", "chant", "chants", "adlib", "adlibs", "acapella", "phrase", "phrases"],
    ),
    (
        "Mallets & Bells",
        &[
            "mallet", "mallets", "marimba", "vibes", "vibraphone", "glockenspiel", "glock", "xylophone", "bell", "bells", "chime", "chimes", "kalimba",
            "celesta", "musicbox", "gamelan", "steelpan", "steeldrum",
        ],
    ),
    ("Keys", &["keys", "piano", "pianos", "organ", "rhodes", "clav", "clavinet", "wurly", "wurlitzer", "epiano", "harpsichord", "accordion", "melodica"]),
    (
        "Guitar & Plucked",
        &["guitar", "guitars", "gtr", "pluck", "plucks", "plucked", "harp", "lute", "koto", "sitar", "oud", "banjo", "mandolin", "ukulele", "bouzouki"],
    ),
    ("Strings", &["strings", "string", "violin", "violins", "viola", "cello", "cellos", "orchestra", "orchestral", "ensemble"]),
    (
        "Brass & Winds",
        &[
            "brass", "trumpet", "trumpets", "sax", "saxophone", "flute", "flutes", "winds", "woodwinds", "horn", "horns", "trombone", "tuba", "clarinet",
            "oboe", "bassoon", "bagpipe", "whistle", "ocarina", "duduk", "shakuhachi",
        ],
    ),
    (
        "Pad & Atmosphere",
        &["pad", "pads", "atmosphere", "atmospheres", "atmos", "texture", "textures", "drone", "drones", "ambience", "ambient", "soundscape", "soundscapes"],
    ),
    ("Synth", &["synth", "synths", "lead", "leads", "arp", "arps", "chord", "chords", "acid", "lick", "licks", "riff", "riffs", "seq", "sequence"]),
    ("Stabs & Hits", &["stab", "stabs", "hit", "hits", "strike", "strikes", "metal"]),
    (
        "FX",
        &[
            "fx", "sfx", "effect", "effects", "riser", "risers", "impact", "impacts", "sweep", "sweeps", "noise", "noises", "glitch", "glitches", "buzz",
            "zap", "zaps", "crackle", "click", "clicks", "distortion", "drill", "scratch", "scratches", "blip", "blips", "blop", "laser", "foley",
            "transition", "transitions", "uplifter", "downlifter", "whoosh", "swoosh", "boom", "braam",
        ],
    ),
    ("Full Mix", &["fullmix", "trackstarter", "trackstarters"]),
];

const DRUM_HIT_CATEGORIES: &[&str] = &["Kick", "Snare", "Clap", "Hi-hat", "Cymbal", "Tom", "Percussion", "Drums"];
const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// What [`infer_metadata`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Inferred {
    /// One of the categories of [`CATEGORY_RULES`], or `Other`.
    pub category: String,
    /// The category, the folders below the pack and markers such as `Loop`; sorted.
    pub tags: Vec<String>,
    /// Tempo in BPM.
    pub bpm: Option<f64>,
    /// Canonical key name, for example `A# minor`.
    pub key: Option<String>,
}

/// Display pack names: without NI's " Library" suffix and Loopcloud's numeric "[12345]" pack IDs.
pub fn clean_pack(value: &str) -> String {
    let mut result = value.replace(" Library", "");
    // A trailing "[digits]" (2..=7 digits) with the white space around it.
    let trimmed = result.trim_end();
    if let Some(open) = trimmed.rfind('[') {
        let inner = &trimmed[open + 1..];
        if let Some(digits) = inner.strip_suffix(']') {
            if (2..=7).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                result = trimmed[..open].trim_end().to_string();
            }
        }
    }
    let mut collapsed = String::with_capacity(result.len());
    let mut spaces = 0;
    for character in result.chars() {
        if character.is_whitespace() {
            spaces += 1;
            if spaces == 1 {
                collapsed.push(character);
            } else if spaces == 2 {
                collapsed.pop();
                collapsed.push(' ');
            }
        } else {
            spaces = 0;
            collapsed.push(character);
        }
    }
    let collapsed = collapsed.trim_matches(' ').to_string();
    if collapsed.is_empty() {
        value.to_string()
    } else {
        collapsed
    }
}

/// Maps NKS "Type / Subtype" paths onto the sample categories so one category list covers both.
pub fn nks_category(value: &str) -> String {
    if value.starts_with("Piano / Keys") {
        return "Keys".to_string();
    }
    let top = value.split(" / ").next().unwrap_or(value).trim_matches(' ');
    let mapped = match top {
        "Synth Lead" | "Synth Misc" => "Synth",
        "Synth Pad" | "Soundscapes" => "Pad & Atmosphere",
        "Piano / Keys" | "Piano" | "Organ" => "Keys",
        "Bowed Strings" => "Strings",
        "Brass" | "Flute" | "Reed Instruments" => "Brass & Winds",
        "Sound Effects" => "FX",
        "Mallet Instruments" => "Mallets & Bells",
        "Plucked Strings" | "Guitar" => "Guitar & Plucked",
        "Vocal" | "Vocals" => "Vocals",
        "Drums" => "Drums",
        "Percussion" => "Percussion",
        "Bass" => "Bass",
        "Ethnic World" => "World",
        "Multitrack" => "Combinations",
        "" => "Other",
        other => other,
    };
    mapped.to_string()
}

/// Canonical sharp-based key names ("A# minor", "C major"); a bare note name is major, as in NI
/// and Loopcloud file names. `None` when `text` is not a key.
pub fn canonical_key(text: &str) -> Option<String> {
    let value: Vec<char> = text.trim_matches(' ').replace('♯', "#").replace('♭', "b").chars().collect();
    let letter = value.first()?.to_ascii_lowercase();
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let quality_of = |rest: &[char]| -> Option<bool> {
        match rest.iter().collect::<String>().trim_start().to_lowercase().as_str() {
            "" | "major" | "maj" => Some(false),
            "minor" | "min" | "m" => Some(true),
            _ => None,
        }
    };
    // The accidental is greedy; without it the rest must still be a quality.
    let with_accidental = value.get(1).and_then(|c| match c {
        '#' => Some(1),
        'b' | 'B' => Some(11),
        _ => None,
    });
    let (shift, minor) = match with_accidental.and_then(|shift| quality_of(&value[2..]).map(|minor| (shift, minor))) {
        Some(found) => found,
        None => (0, quality_of(&value[1..])?),
    };
    Some(format!("{} {}", NOTE_NAMES[(base + shift) % 12], if minor { "minor" } else { "major" }))
}

/// Comparable key slot for filters: enharmonic spellings share one value ("a#minor").
pub fn key_slot(text: &str) -> String {
    fold(&canonical_key(text).unwrap_or_else(|| text.to_string())).replace(' ', "")
}

fn is_tempo_separator(unit: char) -> bool {
    unit == ' ' || unit == '_' || unit == '-'
}

/// A tempo number (40..=259) starting at `at`: its value and length.
fn tempo_number(text: &[char], at: usize) -> Option<(f64, usize)> {
    let digit = |index: usize| text.get(index).and_then(|c| c.to_digit(10));
    let first = digit(at)?;
    let (value, length) = match first {
        4..=9 => (first * 10 + digit(at + 1)?, 2),
        1 => (100 + digit(at + 1)? * 10 + digit(at + 2)?, 3),
        2 => {
            let second = digit(at + 1)?;
            if second > 5 {
                return None;
            }
            (200 + second * 10 + digit(at + 2)?, 3)
        }
        _ => return None,
    };
    Some((f64::from(value), length))
}

/// An explicit tempo: "120bpm", "120 BPM" or Maschine's "[120]". The leftmost one.
fn explicit_tempo(text: &[char]) -> Option<f64> {
    for at in 0..text.len() {
        // number, optional [._ ], "bpm"; not preceded by a digit, not followed by a letter or digit.
        if at == 0 || !text[at - 1].is_ascii_digit() {
            if let Some((value, length)) = tempo_number(text, at) {
                let next = at + length;
                let unit_at = |start: usize| -> bool {
                    text.len() >= start + 3
                        && text[start..start + 3].iter().collect::<String>().eq_ignore_ascii_case("bpm")
                        && text.get(start + 3).is_none_or(|c| !c.is_ascii_alphanumeric())
                };
                if (matches!(text.get(next), Some('.' | '_' | ' ')) && unit_at(next + 1)) || unit_at(next) {
                    return Some(value);
                }
            }
        }
        if text[at] == '[' {
            if let Some((value, length)) = tempo_number(text, at + 1) {
                if text.get(at + 1 + length) == Some(&']') {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// The leftmost delimited bare number such as Loopcloud's "PFX_120_Cm_...": its value and the
/// index just after it.
fn bare_tempo(text: &[char]) -> Option<(f64, usize)> {
    for at in 0..text.len() {
        let starts = [(at == 0).then_some(at), is_tempo_separator(text[at]).then_some(at + 1)];
        for start in starts.into_iter().flatten() {
            if let Some((value, length)) = tempo_number(text, start) {
                let end = start + length;
                if text.get(end).is_none_or(|c| is_tempo_separator(*c)) {
                    return Some((value, end));
                }
            }
        }
    }
    None
}

/// One match of the key pattern in a file name.
struct KeyMatch {
    /// Index of the note letter.
    letter: usize,
    note: char,
    accidental: Option<char>,
    /// `Some` for the explicit form ("Cmin", "F# major", "Am"); `None` for a bare note.
    quality: Option<String>,
    end: usize,
}

fn is_key_prefix(c: char) -> bool {
    c == ' ' || c == '_' || c == ']' || c == '-'
}

/// Tries the key pattern at `start`: explicit quality first, then a bare upper-case note.
fn key_match_at(name: &[char], start: usize) -> Option<KeyMatch> {
    let terminated = |at: usize| -> Option<usize> {
        match name.get(at) {
            None => Some(at),
            Some(c) if is_tempo_separator(*c) => Some(at + 1),
            _ => None,
        }
    };
    let mut letters = Vec::new();
    if start == 0 {
        letters.push(0);
    }
    if name.get(start).is_some_and(|c| is_key_prefix(*c)) {
        letters.push(start + 1);
    }
    // Explicit: note, optional accidental, optional separator, maj(or)/min(or)/m, then a separator or the end.
    for &letter in &letters {
        let Some(&note) = name.get(letter) else { continue };
        if !matches!(note, 'A'..='G' | 'a'..='g') {
            continue;
        }
        let accidentals: &[Option<char>] = match name.get(letter + 1) {
            Some(c @ ('#' | 'b')) => &[Some(if *c == '#' { '#' } else { 'b' }), None],
            _ => &[None],
        };
        for accidental in accidentals {
            let after_note = letter + 1 + usize::from(accidental.is_some());
            let separators: &[usize] = if name.get(after_note).is_some_and(|c| is_tempo_separator(*c)) { &[1, 0] } else { &[0] };
            for separator in separators {
                let at = after_note + separator;
                for quality in ["major", "maj", "minor", "min", "m"] {
                    let length = quality.len();
                    if name.len() >= at + length && name[at..at + length].iter().copied().eq(quality.chars()) {
                        if let Some(end) = terminated(at + length) {
                            return Some(KeyMatch { letter, note, accidental: *accidental, quality: Some(quality.to_string()), end });
                        }
                    }
                }
            }
        }
    }
    // Bare: an upper-case note with an optional accidental, delimited on both sides.
    for &letter in &letters {
        let Some(&note) = name.get(letter) else { continue };
        if !note.is_ascii_uppercase() || !matches!(note, 'A'..='G') {
            continue;
        }
        let accidentals: &[Option<char>] = match name.get(letter + 1) {
            Some('#') => &[Some('#'), None],
            Some('b') => &[Some('b'), None],
            _ => &[None],
        };
        for accidental in accidentals {
            if let Some(end) = terminated(letter + 1 + usize::from(accidental.is_some())) {
                return Some(KeyMatch { letter, note, accidental: *accidental, quality: None, end });
            }
        }
    }
    None
}

fn infer_key(name: &str, category: &str) -> Option<String> {
    let characters: Vec<char> = name.chars().collect();
    let mut start = 0;
    while start <= characters.len() {
        let Some(found) = (start..=characters.len()).find_map(|at| key_match_at(&characters, at)) else { break };
        start = found.end.max(start + 1);
        let accidental = found.accidental.map(String::from).unwrap_or_default();
        if let Some(quality) = &found.quality {
            return canonical_key(&format!("{}{accidental} {quality}", found.note));
        }
        // Bare letters in drum names are usually variations ("Kick A 2"), not keys, and a
        // leading "A " is usually an article ("A Fading Dream").
        if DRUM_HIT_CATEGORIES.contains(&category) || category == "FX" {
            continue;
        }
        if found.letter == 0 && found.accidental.is_none() && characters.len() > 2 && characters[2].is_alphabetic() {
            continue;
        }
        return canonical_key(&format!("{}{accidental}", found.note));
    }
    None
}

/// Whether a file name starts like a Maschine construction-kit full mix: "Full[", "Combo [".
fn is_full_mix(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["fullmix", "full mix", "full_mix", "full", "combo"]
        .iter()
        .any(|prefix| lower.strip_prefix(prefix).is_some_and(|rest| rest.trim_start().starts_with('[')))
}

/// "BoomSnare" -> "Boom Snare", "ClosedHH" -> "Closed HH".
fn split_camel_case(text: &str) -> String {
    let mut result = String::with_capacity(text.len() + 4);
    let mut previous = '\0';
    for character in text.chars() {
        if previous.is_ascii_lowercase() && character.is_ascii_uppercase() {
            result.push(' ');
        }
        result.push(character);
        previous = character;
    }
    result
}

fn fold_hihat(text: &str) -> String {
    fold(text).replace("hi-hat", "hihat").replace("hi hat", "hihat")
}

/// Category, tags, tempo and key of a file from its name (without extension) and its path
/// components below the library root (`hierarchy`, ending with the file name).
pub fn infer_metadata(name: &str, hierarchy: &[String]) -> Inferred {
    let folders: &[String] = if hierarchy.is_empty() { &[] } else { &hierarchy[..hierarchy.len() - 1] };
    // Folders below the pack folder (the first component is the pack).
    let inner: &[String] = if folders.is_empty() { &[] } else { &folders[1..] };
    let text = fold_hihat(&std::iter::once(name).chain(folders.iter().map(String::as_str)).collect::<Vec<_>>().join(" "));
    let words: std::collections::HashSet<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();

    // File name and nearest folder are stronger signals than the pack's name: a snare in
    // "808 Kick Collection" is still a snare.
    let mut category = "Other";
    if is_full_mix(name) {
        category = "Full Mix";
    } else {
        for component in std::iter::once(name).chain(inner.iter().rev().map(String::as_str)) {
            let component_text = fold_hihat(&split_camel_case(component));
            let component_words: std::collections::HashSet<&str> = component_text.split(|c: char| !c.is_alphabetic()).collect();
            if let Some((rule, _)) = CATEGORY_RULES.iter().find(|(_, words)| words.iter().any(|word| component_words.contains(word))) {
                category = rule;
                break;
            }
        }
    }

    let mut tags: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    tags.insert(category.to_string());
    const PLAIN_FOLDERS: &[&str] = &["samples", "sample", "sounds", "presets", "instruments", "wav", "aiff", "loops"];
    for folder in inner.iter().filter(|folder| !PLAIN_FOLDERS.contains(&fold(folder).as_str())).take(8) {
        tags.insert(folder.clone());
    }
    if words.contains("loop") || words.contains("loops") {
        tags.insert("Loop".to_string());
    }
    if words.contains("oneshot") || (words.contains("one") && words.contains("shots")) || text.contains("one shot") {
        tags.insert("One-shot".to_string());
    }
    if words.contains("stereo") {
        tags.insert("Stereo".to_string());
    }
    if words.contains("mono") {
        tags.insert("Mono".to_string());
    }

    // A tempo in the file name is stronger evidence than a number in a pack name.
    let tempo_text: Vec<char> = format!("{name} {}", inner.join(" ")).chars().collect();
    let mut bpm = explicit_tempo(&tempo_text);
    if bpm.is_none() {
        // Bare numbers are often take, variation or velocity numbers ("Snare 64", "Kick_46"):
        // accepted only inside the file name, never as its final token.
        if let Some((value, end)) = bare_tempo(&tempo_text) {
            if end < name.chars().count() {
                bpm = Some(value);
            }
        }
    }
    let key = infer_key(name, category);
    Inferred { category: category.to_string(), tags: tags.into_iter().collect(), bpm, key }
}

/// `oneshot`, `loop` or `unknown` for a sample, from explicit words in its name and folders,
/// the Maschine expansion layout, and whether its file name carries a tempo.
pub fn infer_sample_type(name: &str, path: &str, root: &str, is_sample: bool, filename_tempo: bool) -> &'static str {
    if !is_sample {
        return "unknown";
    }
    let folder = parent(path);
    let relative = if folder.len() >= root.len() { &folder[root.len().min(folder.len())..] } else { "" };
    let mut parts: Vec<&str> = vec![name];
    parts.extend(components(relative).into_iter().rev());
    parts.push(file_name(root));
    for part in parts {
        let folded = fold(part);
        let tokens: std::collections::HashSet<&str> = folded.split(|c: char| !c.is_alphanumeric()).filter(|word| !word.is_empty()).collect();
        if tokens.contains("oneshot") || tokens.contains("oneshots") || (tokens.contains("one") && (tokens.contains("shot") || tokens.contains("shots"))) {
            return "oneshot";
        }
        if tokens.contains("loop") || tokens.contains("loops") {
            return "loop";
        }
    }
    // Maschine expansion layout: Samples/Drums/<type> holds drum hits, Samples/Instruments holds playable notes.
    let all = components(path);
    if let Some(samples) = all.iter().rposition(|part| *part == "Samples") {
        if samples + 2 < all.len() && ["Drums", "Instruments", "Insruments"].contains(&all[samples + 1]) {
            return "oneshot";
        }
    }
    // Library file names carry a tempo for loops (Loopcloud "_120_"), not for one-shots.
    if filename_tempo {
        return "loop";
    }
    "unknown" // A kick or a short file is not sufficient evidence of a one-shot.
}

/// The collections a library file can belong to, in the order the page lists them.
pub const COLLECTION_NAMES: [&str; 4] = ["Loopcloud", "Maschine expansions", "Native Instruments", "User libraries"];

/// The collection a path belongs to before expansions are verified: index into
/// [`COLLECTION_NAMES`] (never 1; that is decided by [`collection_root`] and a folder check).
pub fn base_collection(path: &str) -> usize {
    let folded = path.to_lowercase();
    if components(&folded).contains(&"loopcloud") {
        return 0;
    }
    let normalized = folded.replace('\\', "/");
    if normalized.contains("/native access content/") || normalized.contains("/native instruments/") || normalized.contains("/native instruments user content/") {
        return 2;
    }
    3
}

/// The folder that would be the Maschine expansion of a file: the folder holding its `Samples`,
/// `Sounds` or `Groups` folder, else the product folder below a Native Instruments root, else
/// the root. `None` for Loopcloud content.
pub fn collection_root(path: &str, root: &str, collection: usize) -> Option<String> {
    if collection == 0 {
        return None;
    }
    let parts = components(path);
    if let Some(marker) = parts.iter().rposition(|part| ["Samples", "Sounds", "Groups"].contains(part)) {
        if marker > 1 {
            // The text of `path` before the marker component, without the separator.
            let offset = parts[marker].as_ptr() as usize - path.as_ptr() as usize;
            let candidate = path[..offset].trim_end_matches(['/', '\\']);
            if is_inside(candidate, root) {
                return Some(candidate.to_string());
            }
        }
    }
    let relative = if path.len() >= root.len() { &path[root.len()..] } else { "" };
    let below = components(relative);
    if collection == 2 && below.len() > 1 {
        return Some(crate::util::join(root, below[0]));
    }
    Some(root.to_string())
}

/// The pack, name and inferred metadata of a file below a library root.
pub struct PathInference {
    /// File name without extension.
    pub name: String,
    /// Display pack name: the first folder below the root, or the root's name.
    pub pack: String,
    /// Category, tags, tempo and key.
    pub metadata: Inferred,
}

/// Everything the path alone says about `path` below `root` (named `root_name`).
pub fn infer_path(path: &str, root: &str, root_name: &str) -> PathInference {
    let relative = if path.len() >= root.len() && path.starts_with(root) { &path[root.len()..] } else { path };
    let hierarchy: Vec<String> = components(relative).into_iter().map(String::from).collect();
    let name = crate::util::stem(path).to_string();
    let pack = clean_pack(if hierarchy.len() > 1 { &hierarchy[0] } else { root_name });
    let metadata = infer_metadata(&name, &hierarchy);
    PathInference { name, pack, metadata }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn infer(path: &str, root: &str) -> (PathInference, &'static str) {
        let full = format!("{root}/{path}");
        let inferred = infer_path(&full, root, file_name(root));
        let sample_type = infer_sample_type(&inferred.name, &full, root, true, inferred.metadata.bpm.is_some());
        (inferred, sample_type)
    }

    const NI: &str = "/Volumes/Fixture/Native Access Content";
    const LC: &str = "/Volumes/Fixture/Samples/Loopcloud";

    #[test]
    fn maschine_expansion_names() {
        let (full, kind) = infer("Bumpin Flava Library/Samples/Loops/Construction/Target Kit/Full[135] G# Target 1.wav", NI);
        assert_eq!((full.metadata.bpm, full.metadata.key.as_deref(), full.metadata.category.as_str(), kind, full.pack.as_str()), (Some(135.0), Some("G# major"), "Full Mix", "loop", "Bumpin Flava"));
        let (hat, _) = infer("District Xeo Library/Samples/Loops/Construction/Radium/ClosedHH[127] Radium.wav", NI);
        assert_eq!((hat.metadata.category.as_str(), hat.metadata.bpm), ("Hi-hat", Some(127.0)), "camelCase hi-hat stem");
        let (combo, _) = infer("Infamous Flow Library/Samples/Loops/Construction/The Vault/Combo[90] C#m TheVault 1.wav", NI);
        assert_eq!((combo.metadata.key.as_deref(), combo.metadata.bpm), (Some("C# minor"), Some(90.0)), "minor key suffix");
        let (kick, kind) = infer("Maschine 2 Factory Library Library/Samples/Drums/Kick/Kick RnB A 2.wav", NI);
        assert_eq!((kick.metadata.key, kick.metadata.bpm, kind, kick.pack.as_str()), (None, None, "oneshot", "Maschine 2 Factory"), "drum variation letters and numbers are not key or tempo");
        let (velocity, kind) = infer("Maschine 2 Factory Library Library/Samples/Instruments/Strings/Strings Pizzicato Samples/Violn Pizzi 69.wav", NI);
        assert_eq!((velocity.metadata.bpm, kind, velocity.metadata.category.as_str()), (None, "oneshot", "Strings"), "trailing note numbers are not tempo");
        let (bells, _) = infer("Pure Drip Library/Samples/Loops/Bells/Bells[139] Am Animosity.wav", NI);
        assert_eq!((bells.metadata.category.as_str(), bells.metadata.key.as_deref()), ("Mallets & Bells", Some("A minor")));
        let (stab, kind) = infer("Street Swarm Library/Samples/One Shots/Stab & Hit/Stab A BlackMamba 2.wav", NI);
        assert_eq!((stab.metadata.category.as_str(), kind), ("Stabs & Hits", "oneshot"));
    }

    #[test]
    fn loopcloud_names() {
        let (looped, kind) = infer("Chilled Guitars [14011]/31L_120_Dmin_Elec_Guitar.wav", LC);
        assert_eq!(
            (looped.metadata.bpm, looped.metadata.key.as_deref(), looped.pack.as_str(), kind, looped.metadata.category.as_str()),
            (Some(120.0), Some("D minor"), "Chilled Guitars", "loop", "Guitar & Plucked")
        );
        let (numbered, _) = infer("Loopcloud Drum - Intro II Drums [12983]/LCD2_Snare_92.wav", LC);
        assert_eq!((numbered.metadata.bpm, numbered.metadata.category.as_str()), (None, "Snare"), "a take number is not a tempo");
    }

    #[test]
    fn keys_are_canonical_and_enharmonic() {
        assert_eq!(canonical_key("Bb min").as_deref(), Some("A# minor"));
        assert_eq!(canonical_key("f#").as_deref(), Some("F# major"));
        assert_eq!(canonical_key("Cminor").as_deref(), Some("C minor"));
        assert_eq!(canonical_key("C♭ major").as_deref(), Some("B major"));
        assert_eq!(canonical_key("H"), None);
        assert_eq!(canonical_key("C dorian"), None);
        assert_eq!(key_slot("Bb minor"), key_slot("A# minor"));
        assert_eq!(key_slot("Bbm"), "a#minor");
    }

    #[test]
    fn nks_types_join_the_sample_categories() {
        assert_eq!(nks_category("Synth Pad / Warm"), "Pad & Atmosphere");
        assert_eq!(nks_category("Bass / Synth"), "Bass");
        assert_eq!(nks_category("Piano / Keys / Grand"), "Keys");
        assert_eq!(nks_category("Something New"), "Something New");
        assert_eq!(nks_category(""), "Other");
    }

    #[test]
    fn categories_prefer_the_file_name() {
        let hierarchy = ["Kick Pack".to_string(), "Drums".to_string(), "snare02.wav".to_string()];
        assert_eq!(infer_metadata("snare02", &hierarchy).category, "Snare");
        let simple = infer_metadata("Kick_120bpm_Cmin", &["Orbit Pack".to_string(), "Loops".to_string(), "Drums".to_string(), "Kick_120bpm_Cmin.wav".to_string()]);
        assert_eq!((simple.category.as_str(), simple.bpm, simple.key.as_deref()), ("Kick", Some(120.0), Some("C minor")));
        assert!(simple.tags.contains(&"Loop".to_string()) && simple.tags.contains(&"Drums".to_string()) && simple.tags.contains(&"Kick".to_string()));
        assert_eq!(infer_metadata("A Fading Dream", &["Pack".to_string(), "A Fading Dream.wav".to_string()]).key, None, "a leading article is not a key");
        assert_eq!(infer_metadata("Pad C", &["Pack".to_string(), "Pad C.wav".to_string()]).key.as_deref(), Some("C major"));
        assert_eq!(infer_metadata("Bass[120] Am Up", &["Pack".to_string(), "Loops".to_string(), "x.wav".to_string()]).key.as_deref(), Some("A minor"));
    }

    #[test]
    fn tempo_rules() {
        let chars = |text: &str| text.chars().collect::<Vec<char>>();
        assert_eq!(explicit_tempo(&chars("Loop 120 BPM")), Some(120.0));
        assert_eq!(explicit_tempo(&chars("Loop_95.bpm x")), Some(95.0));
        assert_eq!(explicit_tempo(&chars("Loop 1200bpm")), None);
        assert_eq!(explicit_tempo(&chars("Loop 120bpms")), None);
        assert_eq!(explicit_tempo(&chars("Full[135] G#")), Some(135.0));
        assert_eq!(explicit_tempo(&chars("Take [12345]")), None);
        assert_eq!(explicit_tempo(&chars("Slow 39bpm")), None);
        assert_eq!(bare_tempo(&chars("PFX_120_Cm")), Some((120.0, 7)));
        assert_eq!(bare_tempo(&chars("Kick_46")), Some((46.0, 7)));
        assert_eq!(bare_tempo(&chars("Kick_460_x")), None);
    }

    #[test]
    fn packs_and_collections() {
        assert_eq!(clean_pack("Chilled Guitars [14011]"), "Chilled Guitars");
        assert_eq!(clean_pack("Maschine 2 Factory Library Library"), "Maschine 2 Factory");
        assert_eq!(clean_pack("Pack  Two"), "Pack Two");
        assert_eq!(clean_pack(" Library"), " Library");
        assert_eq!(base_collection("/x/Loopcloud/Pack/a.wav"), 0);
        assert_eq!(base_collection("/x/Native Access Content/Pack/a.wav"), 2);
        assert_eq!(base_collection("/x/Mine/a.wav"), 3);
        assert_eq!(collection_root("/r/NI/Anima Library/Samples/Loops/a.wav", "/r/NI", 2).as_deref(), Some("/r/NI/Anima Library"));
        assert_eq!(collection_root("/r/NI/Piano Library/Keys/a.wav", "/r/NI", 2).as_deref(), Some("/r/NI/Piano Library"));
        assert_eq!(collection_root("/r/User/a.wav", "/r/User", 3).as_deref(), Some("/r/User"));
        assert_eq!(collection_root("/r/Loopcloud/a.wav", "/r/Loopcloud", 0), None);
    }

    #[test]
    fn sample_types_need_evidence() {
        assert_eq!(infer_sample_type("Kick", "/r/Pack/One Shots/Kick.wav", "/r", true, false), "oneshot");
        assert_eq!(infer_sample_type("Drum_Loop", "/r/Pack/Drum_Loop.wav", "/r", true, false), "loop");
        assert_eq!(infer_sample_type("Piano", "/r/Pack/Samples/Keys/Piano.wav", "/r", true, false), "unknown");
        assert_eq!(infer_sample_type("Thing_120_x", "/r/Pack/Thing_120_x.wav", "/r", true, true), "loop");
        assert_eq!(infer_sample_type("Preset", "/r/Pack/Loops/Preset.nksf", "/r", false, false), "unknown");
    }
}
