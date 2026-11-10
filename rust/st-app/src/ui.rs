//! Finding the interface's files.

use std::path::{Path, PathBuf};

fn has_interface(folder: &Path) -> bool {
    folder.join("index.html").is_file()
}

/// The folder that holds `index.html`.
///
/// 1. `--ui <folder>` / `SUPERTRACKER_UI_DIR`, and nothing else when given;
/// 2. next to the executable: `Web/`, or `../Resources/Web/` (inside a macOS application bundle);
/// 3. `Resources/Web/` in a folder above the executable, or in the current folder (a development
///    build run from the repository).
pub fn locate(chosen: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(folder) = chosen {
        return if has_interface(folder) {
            Ok(folder.to_path_buf())
        } else {
            Err(format!("{} does not contain the interface (no index.html).", folder.display()))
        };
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(folder) = std::env::current_exe().ok().and_then(|executable| executable.parent().map(Path::to_path_buf)) {
        candidates.push(folder.join("Web"));
        candidates.push(folder.join("../Resources/Web"));
        candidates.extend(folder.ancestors().map(|above| above.join("Resources/Web")));
    }
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current.join("Resources/Web"));
    }
    candidates.into_iter().find(|folder| has_interface(folder)).ok_or_else(|| {
        "The interface files were not found next to the executable. Pass their folder with --ui <folder> (in the repository: Resources/Web).".to_string()
    })
}
