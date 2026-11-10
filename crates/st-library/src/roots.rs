//! The library folders a fresh installation starts with.
//!
//! Every platform gets the user's kit folder. The standard content locations are known per
//! platform:
//!
//! * **macOS**: Apple Loops, `~/Music/Loopcloud`, the Native Instruments user folders in
//!   `~/Documents/Native Instruments`, and the content folders of the products Native Access
//!   registered (see [`crate::nks::catalog::discover_registrations`]).
//! * **Windows**: the Native Instruments user folders in `Documents\Native Instruments`.
//! * **Linux**: none.
//!
//! Only folders that exist are listed. Other folders, such as sample libraries on an external
//! drive, are added by the user and stay listed while offline.

use crate::index::Root;
use crate::nks::catalog::Registration;
use crate::util::{components, is_directory, is_inside, normalized, parent};
use std::collections::BTreeMap;
use std::path::Path;

/// Sub-folders of `Documents/Native Instruments` that hold user samples, sounds and groups.
const NATIVE_INSTRUMENTS_USER_FOLDERS: [&str; 6] = ["Maschine 3/Samples", "Maschine 3/Recordings", "Maschine 3/Sounds", "Maschine 3/Groups", "Maschine 2/Samples", "User Content"];

/// Content folders of installed Native Instruments products and expansions that hold samples
/// or kits. Products installed side by side are indexed through their shared parent folder
/// (for example ".../Native Access Content"), unless that parent is a volume, the home folder
/// or another broad folder.
pub fn native_instruments_content_roots(registrations: &[Registration], home: &str) -> Vec<String> {
    let mut broad: Vec<String> = ["/Users/Shared", "/Applications", "/Library", "/Library/Application Support"].iter().map(|path| path.to_string()).collect();
    broad.push(home.to_string());
    broad.extend(["Documents", "Music", "Desktop", "Downloads"].iter().map(|folder| format!("{home}/{folder}")));
    let content: Vec<&String> = registrations
        .iter()
        .map(|registration| &registration.content_dir)
        .filter(|directory| ["Samples", "Groups", "Kits", "Sounds"].iter().any(|folder| Path::new(directory.as_str()).join(folder).exists()))
        .collect();
    let mut grouped: BTreeMap<String, Vec<&String>> = BTreeMap::new();
    for directory in content {
        grouped.entry(normalized(parent(directory))).or_default().push(directory);
    }
    let mut roots = Vec::new();
    for (folder, children) in grouped {
        let depth = components(&folder).len();
        let volume_root = folder.starts_with("/Volumes/") && depth <= 2;
        if children.len() >= 2 && !broad.contains(&folder) && !volume_root && depth >= 2 {
            roots.push(folder);
        } else {
            roots.extend(children.into_iter().cloned());
        }
    }
    roots.sort();
    roots
}

/// The standard content locations of this platform as `(path, source)`, existing or not.
fn standard_candidates(registrations: &[Registration]) -> Vec<(String, &'static str)> {
    let mut candidates: Vec<(String, &'static str)> = Vec::new();
    let Some(home) = crate::util::home_dir().map(|home| home.to_string_lossy().into_owned()) else { return candidates };
    if cfg!(target_os = "macos") {
        candidates.push(("/Library/Audio/Apple Loops".to_string(), "Apple Loops"));
        candidates.push((format!("{home}/Music/Loopcloud"), "Loopcloud"));
    }
    if cfg!(any(target_os = "macos", windows)) {
        for folder in NATIVE_INSTRUMENTS_USER_FOLDERS {
            candidates.push((Path::new(&home).join("Documents").join("Native Instruments").join(folder).to_string_lossy().into_owned(), "Native Instruments"));
        }
    }
    candidates.extend(native_instruments_content_roots(registrations, &home).into_iter().map(|path| (path, "Native Instruments")));
    candidates
}

/// The default roots: the user's kit folder (created so an empty library is not reported as
/// offline) and the standard locations that exist and do not nest with one another.
pub(crate) fn discover_roots(user_kits: Option<&Path>, registrations: &[Registration]) -> Vec<Root> {
    let mut result = Vec::new();
    if let Some(folder) = user_kits {
        let _ = std::fs::create_dir_all(folder);
        result.push(Root::new(&folder.to_string_lossy(), "User kits"));
    }
    for (path, source) in standard_candidates(registrations) {
        let path = normalized(&path);
        if is_directory(&path) && !result.iter().any(|root: &Root| is_inside(&path, &root.path) || is_inside(&root.path, &path)) {
            result.push(Root::new(&path, source));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_by_side_products_share_their_parent_folder() {
        let temp = std::env::temp_dir().join(format!("st-library-roots-{}-{}", std::process::id(), crate::util::unique_id()));
        let make = |relative: &str| {
            let path = temp.join(relative);
            std::fs::create_dir_all(&path).unwrap();
            path.to_string_lossy().into_owned()
        };
        let registration = |content_dir: String| Registration { name: "P".to_string(), company: "NI".to_string(), content_dir, effect: None };
        let (a, b) = (make("Content/Alpha Library"), make("Content/Beta Library"));
        make("Content/Alpha Library/Samples");
        make("Content/Beta Library/Groups");
        let lonely = make("Elsewhere/Gamma Library");
        make("Elsewhere/Gamma Library/Sounds");
        let presets_only = make("Synths/Massive");
        let home = make("Home");
        let (in_home_a, in_home_b) = (make("Home/One"), make("Home/Two"));
        make("Home/One/Samples");
        make("Home/Two/Samples");
        let registrations: Vec<Registration> = [a, b, lonely.clone(), presets_only, in_home_a.clone(), in_home_b.clone()].into_iter().map(registration).collect();
        let roots = native_instruments_content_roots(&registrations, &home);
        let mut expected = vec![temp.join("Content").to_string_lossy().into_owned(), lonely, in_home_a, in_home_b];
        expected.sort();
        assert_eq!(roots, expected, "two products share their parent; a single one, and products in a broad folder, stay separate; presets-only products are not sample roots");

        let kits = temp.join("Kits");
        let found = discover_roots(Some(&kits), &[]);
        assert!(kits.is_dir() && found[0].source == "User kits" && found[0].path == kits.to_string_lossy());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
