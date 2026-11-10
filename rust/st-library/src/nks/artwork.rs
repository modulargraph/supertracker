//! Product and expansion artwork for NKS presets and Maschine groups, read from the shared
//! "NI Resources" folder that Native Access maintains:
//!
//! ```text
//! image/<product>/                 NI products and expansions (MST_artwork, VB_artwork, logos, NKS2 banners)
//! image/<vendor>/<product>/        partner products (Arturia, Waves, iZotope ...)
//! image/<product>/<sub-library>/   sub-libraries (Maschine 2 Factory Selection)
//! _DatabaseResources/color/color.db            background colours keyed "product" or "product|bank"
//! dist_database/<vendor>/<product>/color.json  partner background colours
//! ```
//!
//! Each folder's `.meta` names the product exactly as presets spell it in their bank chain.
//! Pictures leave this module only as data URLs keyed by a folder the index itself found, so
//! the page can never ask for an arbitrary file.
//!
//! The pictures are passed on as the files are (PNG or WebP), bounded in size, not rescaled:
//! this crate carries no image codec. Native Instruments ships them at thumbnail and banner
//! sizes already.

use super::sqlite;
use crate::util::{base64_encode, fold, lock, read_bounded};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The artwork found for a product and bank.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArtMatch {
    /// Key of the product's artwork folder ("image/massive x").
    pub product: Option<String>,
    /// Key of the bank's artwork folder, for a real sub-library or NI expansion.
    pub bank: Option<String>,
    /// Background colour, `#rrggbb`.
    pub color: Option<String>,
}

const IMAGE_NAMES: [&str; 9] =
    ["mst_artwork.png", "vb_artwork.png", "mst_logo.png", "vb_logo.png", "oso_logo.png", "nks2_hardware_banner.webp", "nks2_hardware_banner.png", "nks2_software_tile.webp", "mst_plugin.png"];
const SEPARATOR: &str = "\u{1f}";
/// Largest picture passed on, and the most picture data in one reply.
const MAXIMUM_IMAGE_BYTES: u64 = 1024 * 1024;
const MAXIMUM_REPLY_BYTES: usize = 24 * 1024 * 1024;

fn variant_files(variant: &str) -> Option<&'static [&'static str]> {
    match variant {
        "tile" => Some(&["mst_artwork.png", "vb_artwork.png", "nks2_software_tile.webp", "nks2_hardware_banner.webp", "nks2_hardware_banner.png"]),
        "banner" => Some(&["nks2_hardware_banner.webp", "nks2_hardware_banner.png", "nks2_software_tile.webp", "mst_artwork.png", "vb_artwork.png"]),
        "logo" => Some(&["vb_logo.png", "oso_logo.png", "mst_logo.png"]),
        _ => None,
    }
}

#[derive(Default)]
struct Index {
    /// folded name -> relative folder
    single: HashMap<String, String>,
    /// folded "parent<US>child" -> relative folder
    pairs: HashMap<String, String>,
    /// relative folder -> folded vendor from its .meta
    vendors: HashMap<String, String>,
    /// relative folder -> lower-case file name -> real name
    folders: HashMap<String, HashMap<String, String>>,
    /// folded location -> "#rrggbb"
    colors: HashMap<String, String>,
}

/// The artwork index of one "NI Resources" folder. Built on first use.
pub struct NksArtwork {
    root: PathBuf,
    index: Mutex<Option<Arc<Index>>>,
    matches: Mutex<HashMap<String, ArtMatch>>,
    images: Mutex<(HashMap<String, Arc<str>>, Vec<String>)>,
}

/// The standard location of "NI Resources", where the platform has one.
pub fn default_root() -> Option<PathBuf> {
    cfg!(target_os = "macos").then(|| PathBuf::from("/Users/Shared/NI Resources"))
}

/// Folded for matching folder names against preset spellings: also normalizes apostrophes and
/// white space.
fn art_fold(value: &str) -> String {
    fold(value).replace('\u{2019}', "'").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `#rrggbb` from six hexadecimal digits with optional `#`.
pub fn hex_color(value: &str) -> Option<String> {
    let hex = value.trim_matches(|c| c == '#' || c == ' ' || c == '\n').to_lowercase();
    (hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| format!("#{hex}"))
}

fn xml_tag(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    let value = xml[start..end].trim().replace("&amp;", "&").replace("&apos;", "'");
    (!value.is_empty()).then_some(value)
}

fn children(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| !name.starts_with('.')).collect())
        .unwrap_or_default();
    names.sort();
    names
}

impl NksArtwork {
    /// An index over the "NI Resources" folder at `root`. A missing folder gives no artwork.
    pub fn new(root: &Path) -> NksArtwork {
        NksArtwork { root: root.to_path_buf(), index: Mutex::new(None), matches: Mutex::new(HashMap::new()), images: Mutex::new((HashMap::new(), Vec::new())) }
    }

    /// Artwork folders for a preset's bank chain. Bank art is used only for a real
    /// sub-library: a folder nested under the product, or an NI expansion folder for an NI product.
    pub fn find(&self, product: &str, vendor: &str, bank: &str) -> ArtMatch {
        let cache_key = [product, vendor, bank].join(SEPARATOR);
        if let Some(cached) = lock(&self.matches).get(&cache_key) {
            return cached.clone();
        }
        let index = self.load_index();
        let (p, v, b) = (art_fold(product), art_fold(vendor), art_fold(bank));
        let mut result = ArtMatch::default();
        if !p.is_empty() {
            result.product = index.pairs.get(&format!("{v}{SEPARATOR}{p}")).or_else(|| index.single.get(&p)).cloned();
            if !b.is_empty() {
                if let Some(nested) = index.pairs.get(&format!("{p}{SEPARATOR}{b}")) {
                    result.bank = Some(nested.clone());
                } else if let Some(folder) = index.single.get(&b) {
                    let native = index.vendors.get(folder).map(String::as_str).unwrap_or("native instruments") == "native instruments";
                    if Some(folder) != result.product.as_ref() && v == "native instruments" && native {
                        result.bank = Some(folder.clone());
                    }
                }
            }
            result.color = (if b.is_empty() { None } else { index.colors.get(&format!("{p}|{b}")) })
                .or_else(|| index.colors.get(&p))
                .or_else(|| index.colors.get(&format!("{v}{SEPARATOR}{p}")))
                .cloned();
        }
        let mut matches = lock(&self.matches);
        if matches.len() > 20_000 {
            matches.clear();
        }
        matches.insert(cache_key, result.clone());
        result
    }

    /// Pictures for known artwork folders as data URLs: `{"image/massive x": "data:image/png;base64,…"}`.
    /// `variant` is `tile`, `banner` or `logo`. Unknown keys, unknown variants and folders
    /// without the requested picture are left out.
    pub fn thumbnails(&self, keys: &[String], variant: &str) -> HashMap<String, String> {
        let Some(files) = variant_files(variant) else { return HashMap::new() };
        let index = self.load_index();
        let mut result = HashMap::new();
        let mut total = 0usize;
        let mut seen = std::collections::HashSet::new();
        for key in keys.iter().take(400).filter(|key| seen.insert(key.as_str())) {
            let Some(folder) = index.folders.get(key) else { continue };
            let cache_key = format!("{variant}#{key}");
            let cached = lock(&self.images).0.get(&cache_key).cloned();
            let url: Arc<str> = match cached {
                Some(url) => url,
                None => {
                    let url: Arc<str> = files
                        .iter()
                        .filter_map(|name| folder.get(*name))
                        .find_map(|real| {
                            let path = self.root.join(key).join(real);
                            let data = read_bounded(&path, MAXIMUM_IMAGE_BYTES).filter(|data| !data.is_empty())?;
                            let mime = if real.to_lowercase().ends_with(".webp") { "image/webp" } else { "image/png" };
                            Some(format!("data:{mime};base64,{}", base64_encode(&data)))
                        })
                        .map(Arc::from)
                        .unwrap_or_else(|| Arc::from(""));
                    let mut images = lock(&self.images);
                    images.0.insert(cache_key.clone(), url.clone());
                    images.1.push(cache_key);
                    if images.1.len() > 1200 {
                        let oldest = images.1.remove(0);
                        images.0.remove(&oldest);
                    }
                    url
                }
            };
            if !url.is_empty() && total + url.len() <= MAXIMUM_REPLY_BYTES {
                total += url.len();
                result.insert(key.clone(), url.to_string());
            }
        }
        result
    }

    fn load_index(&self) -> Arc<Index> {
        if let Some(index) = lock(&self.index).as_ref() {
            return index.clone();
        }
        let built = Arc::new(self.build_index());
        lock(&self.index).get_or_insert(built).clone()
    }

    fn build_index(&self) -> Index {
        let mut index = Index::default();
        let images = self.root.join("image");
        let meta = |folder: &Path, names: &[String]| -> (Option<String>, Option<String>) {
            let Some(file) = names.iter().find(|name| name.to_lowercase().ends_with(".meta")) else { return (None, None) };
            let Some(xml) = read_bounded(&folder.join(file), 65_535).and_then(|data| String::from_utf8(data).ok()) else { return (None, None) };
            (xml_tag(&xml, "name"), xml_tag(&xml, "vendor"))
        };
        // Registers a folder that holds pictures. False for grouping folders (vendors).
        fn register(index: &mut Index, relative: &str, names: &[String]) -> bool {
            let files: HashMap<String, String> = names.iter().filter(|name| IMAGE_NAMES.contains(&name.to_lowercase().as_str())).map(|name| (name.to_lowercase(), name.clone())).collect();
            if files.is_empty() {
                return false;
            }
            index.folders.insert(relative.to_string(), files);
            true
        }
        for top in children(&images) {
            let folder = images.join(&top);
            if !folder.is_dir() {
                continue;
            }
            let names = children(&folder);
            let info = meta(&folder, &names);
            let relative = format!("image/{top}");
            let has_images = register(&mut index, &relative, &names);
            if has_images {
                for key in [Some(top.clone()), info.0.clone()].into_iter().flatten().map(|name| art_fold(&name)) {
                    index.single.entry(key).or_insert_with(|| relative.clone());
                }
                if let Some(vendor) = &info.1 {
                    index.vendors.insert(relative.clone(), art_fold(vendor));
                }
            }
            // Nested folders: partner products under a vendor folder, or sub-libraries under a product.
            for child in &names {
                let child_folder = folder.join(child);
                if !child_folder.is_dir() {
                    continue;
                }
                let child_names = children(&child_folder);
                let child_info = meta(&child_folder, &child_names);
                let child_relative = format!("{relative}/{child}");
                if !register(&mut index, &child_relative, &child_names) {
                    continue;
                }
                let parents: std::collections::BTreeSet<String> = [Some(top.clone()), info.0.clone(), child_info.1.clone()].into_iter().flatten().map(|name| art_fold(&name)).collect();
                let selves: std::collections::BTreeSet<String> = [Some(child.clone()), child_info.0.clone()].into_iter().flatten().map(|name| art_fold(&name)).collect();
                for parent in &parents {
                    for name in &selves {
                        index.pairs.insert(format!("{parent}{SEPARATOR}{name}"), child_relative.clone());
                    }
                }
                if let Some(vendor) = &child_info.1 {
                    index.vendors.insert(child_relative.clone(), art_fold(vendor));
                }
                if !has_images {
                    for name in &selves {
                        index.single.entry(name.clone()).or_insert_with(|| child_relative.clone());
                    }
                }
            }
        }
        // Background colours: NI products and expansions from color.db, partners from dist_database.
        if let Some(table) = read_bounded(&self.root.join("_DatabaseResources/color/color.db"), 64 * 1024 * 1024).and_then(|bytes| sqlite::read_table(&bytes, "color", 20_000)) {
            let column = |name: &str, fallback: usize| table.columns.iter().position(|column| column == name).unwrap_or(fallback);
            let (location, name, value) = (column("location", 0), column("name", 1), column("value", 2));
            for row in &table.rows {
                let (Some(location), Some(name), Some(value)) = (row.get(location).and_then(sqlite::Value::text), row.get(name).and_then(sqlite::Value::text), row.get(value).and_then(sqlite::Value::text)) else {
                    continue;
                };
                let mst = name.eq_ignore_ascii_case("MST_bgcolor");
                if !mst && !name.eq_ignore_ascii_case("VB_bgcolor") {
                    continue;
                }
                let key = location.split('|').map(art_fold).collect::<Vec<_>>().join("|");
                if let Some(color) = hex_color(value) {
                    if mst || !index.colors.contains_key(&key) {
                        index.colors.insert(key, color);
                    }
                }
            }
        }
        let partners = self.root.join("dist_database");
        for vendor in children(&partners) {
            for product in children(&partners.join(&vendor)) {
                let Some(json) = read_bounded(&partners.join(&vendor).join(&product).join("color.json"), 4095).and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok()) else { continue };
                let color = json.get("MST_bgcolor").or_else(|| json.get("VB_bgcolor")).and_then(serde_json::Value::as_str).and_then(hex_color);
                if let Some(color) = color {
                    index.colors.insert(format!("{}{SEPARATOR}{}", art_fold(&vendor), art_fold(&product)), color);
                }
            }
        }
        index
    }
}
