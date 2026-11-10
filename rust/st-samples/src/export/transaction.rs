//! Filesystem ownership for one export.
//!
//! Everything an export writes goes into a private staging folder next to the destination:
//!
//! ```text
//! <parent>/.supertracker-export-<id>.partial/
//!     sources/    intermediate renders (one .caf per output)
//!     delivery/   the finished files, under their final names
//! ```
//!
//! [`ExportTransaction::publish`] makes the result visible with a single rename on the same
//! volume: a single file replaces (or creates) the destination, a stem export's `delivery`
//! folder becomes the destination folder. Until then the destination is untouched; a cancelled,
//! failed or abandoned export removes only its staging folder.
//!
//! The transaction holds the parent and staging directories open and works relative to them, so
//! it keeps acting on the same directories even if someone renames or replaces their paths, and
//! it refuses to publish when the destination or the parent is no longer what it was when the
//! export began. This gives atomic visibility; it is not durability against power loss, and it
//! does not lock other processes out between the final check and the rename.

use std::collections::HashSet;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use super::fsx::{self, Identity, Kind};
use crate::error::{Result, SampleError};
use crate::paths::unique_token;

/// What an export publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// One file at the destination path. An existing regular file there is replaced.
    File,
    /// A folder of files at the destination path, which must be absent or an empty folder.
    Stems,
}

/// The paths of one registered output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportEntry {
    /// Where to write the intermediate render (a `.caf` file in the private `sources` folder).
    pub source_path: PathBuf,
    /// Where to write the finished file (in the private `delivery` folder). This is the
    /// destination to give a [`super::DeliveryJob`]; never give it `final_path`.
    pub delivery_path: PathBuf,
    /// Where the file will be after [`ExportTransaction::publish`].
    pub final_path: PathBuf,
}

#[derive(PartialEq)]
enum State {
    Open,
    Published,
    Discarded,
}

/// One export's staging folder and its atomic publication. See the module documentation.
pub struct ExportTransaction {
    destination: PathBuf,
    sources_directory: PathBuf,
    delivery_directory: PathBuf,
    cleanup_error: Option<String>,
    kind: ExportKind,
    parent: PathBuf,
    destination_name: String,
    private_name: String,
    parent_identity: Identity,
    initial_target: Option<Identity>,
    root_identity: Identity,
    source_identity: Identity,
    delivery_identity: Identity,
    parent_fd: OwnedFd,
    root_fd: OwnedFd,
    source_fd: OwnedFd,
    delivery_fd: OwnedFd,
    entries: Vec<(String, ExportEntry)>,
    normalized_names: HashSet<String>,
    state: State,
    removed_private_root: bool,
}

fn failure(message: &str) -> SampleError {
    SampleError::Invalid(message.to_string())
}

fn system(operation: &str, error: std::io::Error) -> SampleError {
    SampleError::Io(format!("{operation}: {error}."))
}

/// Resolves "." and ".." textually, like a shell would before touching the disk.
fn standardize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// A single file name: no path separators, no control or invisible formatting characters.
fn validate_leaf(name: &str) -> Result<()> {
    let invisible = |c: char| c.is_control() || matches!(c, '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}');
    let valid = !name.is_empty() && name != "." && name != ".." && name.len() <= 255 && !name.contains(['/', '\\', ':']) && !name.chars().any(invisible);
    if valid {
        Ok(())
    } else {
        Err(failure("Export filenames must be single local names without path separators or control characters."))
    }
}

/// Every component of `path` must be a real directory: a symbolic link anywhere could redirect
/// the export.
fn require_real_directory_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let item = fsx::lstat(&current).map_err(|e| system("Inspect export path", e))?;
        if item.map(|item| item.kind()) != Some(Kind::Directory) {
            return Err(failure("The export parent must exist and cannot contain symbolic links."));
        }
    }
    Ok(())
}

fn leaf_names(fd: &OwnedFd) -> Result<Vec<String>> {
    let names = fsx::names(fd.as_fd()).map_err(|e| system("Read export folder", e))?;
    Ok(names.into_iter().map(|name| name.to_string_lossy().into_owned()).collect())
}

fn require_empty_directory(name: &str, parent: &OwnedFd) -> Result<()> {
    let fd = fsx::open_dir_at(parent.as_fd(), name).map_err(|e| system("Open stem destination folder", e))?;
    if fsx::names(fd.as_fd()).map_err(|e| system("Read export folder", e))?.is_empty() {
        Ok(())
    } else {
        Err(failure("Stem export cannot replace a nonempty folder. Choose a new or empty folder."))
    }
}

/// Removes everything inside the directory `fd`, never following symbolic links.
fn remove_contents(fd: &OwnedFd) -> Result<()> {
    let names = fsx::names(fd.as_fd()).map_err(|e| system("Read export folder", e))?;
    for name in names {
        let Some(name) = name.to_str() else {
            return Err(failure("Private export cleanup found a file it did not create."));
        };
        let Some(item) = fsx::stat_at(fd.as_fd(), name).map_err(|e| system("Inspect export entry", e))? else { continue };
        if item.kind() == Kind::Directory {
            let child = fsx::open_dir_at(fd.as_fd(), name).map_err(|e| system("Open private export cleanup folder", e))?;
            let opened = fsx::stat_fd(child.as_fd()).map_err(|e| system("Inspect export directory", e))?;
            if !item.same_object(&opened) {
                return Err(failure("Private export cleanup found a replaced subfolder."));
            }
            remove_contents(&child)?;
            fsx::unlink_at(fd.as_fd(), name, true).map_err(|e| system("Remove private export subfolder", e))?;
        } else {
            // Includes symbolic links: the private entry itself is removed, never its target.
            fsx::unlink_at(fd.as_fd(), name, false).map_err(|e| system("Remove private export file", e))?;
        }
    }
    Ok(())
}

impl ExportTransaction {
    /// Starts an export to `destination` (an absolute local path).
    ///
    /// The parent folder must exist and its path must not pass through a symbolic link. For
    /// [`ExportKind::File`] the destination may be absent or a regular file; for
    /// [`ExportKind::Stems`] it may be absent or an empty folder. Nothing at the destination is
    /// touched; a private staging folder is created next to it.
    pub fn new(destination: &Path, kind: ExportKind) -> Result<Self> {
        if !destination.is_absolute() {
            return Err(failure("Choose a local absolute export destination."));
        }
        let destination = standardize(destination);
        let name = destination.file_name().and_then(|name| name.to_str()).ok_or_else(|| failure("Choose a local absolute export destination."))?.to_string();
        validate_leaf(&name)?;
        let parent = destination.parent().ok_or_else(|| failure("Choose a local absolute export destination."))?.to_path_buf();
        require_real_directory_path(&parent)?;
        let parent_fd = fsx::open_dir(&parent).map_err(|e| system("Open export folder", e))?;
        let parent_identity = fsx::stat_fd(parent_fd.as_fd()).map_err(|e| system("Inspect export directory", e))?;
        let target = fsx::stat_at(parent_fd.as_fd(), &name).map_err(|e| system("Inspect export entry", e))?;
        match kind {
            ExportKind::File => {
                if target.is_some_and(|target| target.kind() != Kind::File) {
                    return Err(failure("The export destination must be a regular file or a new filename."));
                }
            }
            ExportKind::Stems => {
                if target.is_some_and(|target| target.kind() != Kind::Directory) {
                    return Err(failure("Stem export needs a new or empty folder."));
                }
                if target.is_some() {
                    require_empty_directory(&name, &parent_fd)?;
                }
            }
        }
        let private_name = format!(".supertracker-export-{}.partial", unique_token());
        fsx::mkdir_at(parent_fd.as_fd(), &private_name, 0o700).map_err(|e| system("Create private export staging folder", e))?;
        // From here on a failure must take the staging folder with it.
        let build = || -> Result<(OwnedFd, OwnedFd, OwnedFd, Identity, Identity, Identity)> {
            let root_fd = fsx::open_dir_at(parent_fd.as_fd(), &private_name).map_err(|e| system("Open private export staging folder", e))?;
            for sub in ["sources", "delivery"] {
                fsx::mkdir_at(root_fd.as_fd(), sub, 0o700).map_err(|e| system("Create private export subfolders", e))?;
            }
            let source_fd = fsx::open_dir_at(root_fd.as_fd(), "sources").map_err(|e| system("Open private export subfolders", e))?;
            let delivery_fd = fsx::open_dir_at(root_fd.as_fd(), "delivery").map_err(|e| system("Open private export subfolders", e))?;
            let inspect = |fd: &OwnedFd| fsx::stat_fd(fd.as_fd()).map_err(|e| system("Inspect export directory", e));
            let identities = (inspect(&root_fd)?, inspect(&source_fd)?, inspect(&delivery_fd)?);
            Ok((root_fd, source_fd, delivery_fd, identities.0, identities.1, identities.2))
        };
        let (root_fd, source_fd, delivery_fd, root_identity, source_identity, delivery_identity) = match build() {
            Ok(parts) => parts,
            Err(error) => {
                if let Ok(root) = fsx::open_dir_at(parent_fd.as_fd(), &private_name) {
                    remove_contents(&root).ok();
                }
                fsx::unlink_at(parent_fd.as_fd(), &private_name, true).ok();
                return Err(error);
            }
        };
        let root = parent.join(&private_name);
        Ok(ExportTransaction {
            sources_directory: root.join("sources"),
            delivery_directory: root.join("delivery"),
            destination,
            cleanup_error: None,
            kind,
            parent,
            destination_name: name,
            private_name,
            parent_identity,
            initial_target: target,
            root_identity,
            source_identity,
            delivery_identity,
            parent_fd,
            root_fd,
            source_fd,
            delivery_fd,
            entries: Vec::new(),
            normalized_names: HashSet::new(),
            state: State::Open,
            removed_private_root: false,
        })
    }

    /// The destination, with "." and ".." resolved.
    pub fn destination(&self) -> &Path {
        &self.destination
    }

    /// The private folder for intermediate renders.
    pub fn sources_directory(&self) -> &Path {
        &self.sources_directory
    }

    /// The private folder for finished files.
    pub fn delivery_directory(&self) -> &Path {
        &self.delivery_directory
    }

    /// Set when the export was published but its staging folder could not be removed (or a
    /// later [`ExportTransaction::discard`] failed). The publication itself stands.
    pub fn cleanup_error(&self) -> Option<&str> {
        self.cleanup_error.as_deref()
    }

    /// Reserves an output named `filename`. A single-file export registers exactly one (its
    /// name is only used inside the staging folder); a stem export registers one per stem, and
    /// names that differ only in case or Unicode composition count as the same name.
    pub fn register(&mut self, filename: &str) -> Result<ExportEntry> {
        if self.state != State::Open {
            return Err(failure("This export transaction is already finished."));
        }
        validate_leaf(filename)?;
        let normalized = filename.nfc().collect::<String>().to_lowercase();
        if self.normalized_names.contains(&normalized) {
            return Err(failure("Each exported stem needs a unique filename."));
        }
        if self.kind == ExportKind::File && !self.entries.is_empty() {
            return Err(failure("A single-file export can register only one output."));
        }
        self.require_owned_directories()?;
        if fsx::stat_at(self.delivery_fd.as_fd(), filename).map_err(|e| system("Inspect export entry", e))?.is_some() {
            return Err(failure("The private export filename is already occupied."));
        }
        let entry = ExportEntry {
            source_path: self.sources_directory.join(format!("{}.caf", unique_token())),
            delivery_path: self.delivery_directory.join(filename),
            final_path: match self.kind {
                ExportKind::File => self.destination.clone(),
                ExportKind::Stems => self.destination.join(filename),
            },
        };
        self.entries.push((filename.to_string(), entry.clone()));
        self.normalized_names.insert(normalized);
        Ok(entry)
    }

    /// Makes the export visible: one rename, after checking that every registered output is a
    /// non-empty regular file, that nothing else is in the delivery folder, and that the parent
    /// folder and the destination are still what they were when the export began.
    ///
    /// Returns the final paths in registration order. On error nothing was published and the
    /// destination is untouched; call [`ExportTransaction::discard`] (or drop the transaction).
    pub fn publish(&mut self) -> Result<Vec<PathBuf>> {
        if self.state != State::Open {
            return Err(failure("This export transaction is already finished."));
        }
        if self.entries.is_empty() {
            return Err(failure("The export has no completed output files."));
        }
        self.require_owned_directories()?;
        let expected: HashSet<&str> = self.entries.iter().map(|(name, _)| name.as_str()).collect();
        let present = leaf_names(&self.delivery_fd)?;
        if present.len() != expected.len() || !present.iter().all(|name| expected.contains(name.as_str())) {
            return Err(failure("The export is incomplete or contains an unregistered delivery file."));
        }
        for (name, _) in &self.entries {
            let item = fsx::stat_at(self.delivery_fd.as_fd(), name).map_err(|e| system("Inspect export entry", e))?;
            if !item.is_some_and(|item| item.kind() == Kind::File && item.size > 0) {
                return Err(failure("Every export delivery must be a nonempty regular file."));
            }
        }
        require_real_directory_path(&self.parent)?;
        let current_parent = fsx::lstat(&self.parent).map_err(|e| system("Inspect export path", e))?;
        if !current_parent.is_some_and(|current| self.parent_identity.same_object(&current)) {
            return Err(failure("The export parent folder changed. Choose the destination again."));
        }
        if fsx::stat_at(self.parent_fd.as_fd(), &self.destination_name).map_err(|e| system("Inspect export entry", e))? != self.initial_target {
            return Err(failure("The export destination changed. Choose the destination again."));
        }
        if self.kind == ExportKind::Stems && self.initial_target.is_some() {
            require_empty_directory(&self.destination_name, &self.parent_fd)?;
        }
        let (from_fd, from_name, directory) = match self.kind {
            ExportKind::File => (&self.delivery_fd, self.entries[0].0.as_str(), false),
            ExportKind::Stems => {
                // The staging folders are private (0700); the published folder is an ordinary one.
                fsx::chmod_fd(self.delivery_fd.as_fd(), 0o755).map_err(|e| system("Publish completed export", e))?;
                (&self.root_fd, "delivery", true)
            }
        };
        let renamed = if self.initial_target.is_none() {
            fsx::rename_exclusive_at(from_fd.as_fd(), from_name, self.parent_fd.as_fd(), &self.destination_name, directory)
        } else {
            fsx::rename_at(from_fd.as_fd(), from_name, self.parent_fd.as_fd(), &self.destination_name)
        };
        renamed.map_err(|e| system("Publish completed export", e))?;
        self.state = State::Published;
        self.cleanup_error = self.remove_private_root().err().map(|error| error.to_string());
        Ok(self.entries.iter().map(|(_, entry)| entry.final_path.clone()).collect())
    }

    /// Removes the staging folder. Use it to cancel or after a failure; it is also safe after
    /// publication, because published files are no longer inside the staging folder.
    pub fn discard(&mut self) -> Result<()> {
        if self.state == State::Open {
            self.state = State::Discarded;
        }
        match self.remove_private_root() {
            Ok(()) => {
                self.cleanup_error = None;
                Ok(())
            }
            Err(error) => {
                self.cleanup_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn require_owned_directories(&self) -> Result<()> {
        let same = |dir: &OwnedFd, name: &str, identity: &Identity| -> Result<bool> {
            let current = fsx::stat_at(dir.as_fd(), name).map_err(|e| system("Inspect export entry", e))?;
            Ok(current.is_some_and(|current| identity.same_object(&current)))
        };
        if same(&self.parent_fd, &self.private_name, &self.root_identity)?
            && same(&self.root_fd, "sources", &self.source_identity)?
            && same(&self.root_fd, "delivery", &self.delivery_identity)?
        {
            Ok(())
        } else {
            Err(failure("The private export staging folder changed."))
        }
    }

    fn remove_private_root(&mut self) -> Result<()> {
        if self.removed_private_root {
            return Ok(());
        }
        let Some(root) = fsx::stat_at(self.parent_fd.as_fd(), &self.private_name).map_err(|e| system("Inspect export entry", e))? else {
            self.removed_private_root = true;
            return Ok(());
        };
        if !self.root_identity.same_object(&root) {
            return Err(failure("Private export cleanup refused a replaced staging folder."));
        }
        remove_contents(&self.root_fd)?;
        fsx::unlink_at(self.parent_fd.as_fd(), &self.private_name, true).map_err(|e| system("Remove private export staging folder", e))?;
        self.removed_private_root = true;
        Ok(())
    }
}

impl Drop for ExportTransaction {
    fn drop(&mut self) {
        // An abandoned export leaves nothing behind. `source_fd` is held only to keep the
        // folder's identity stable for as long as the transaction lives.
        let _ = &self.source_fd;
        self.remove_private_root().ok();
    }
}
