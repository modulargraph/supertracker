//! The project document as the host sees it. See `docs/RUST_ARCHITECTURE.md`.
//!
//! The interface (the web page) owns the open project and publishes it as JSON. This crate gives
//! the host a typed view of that document and everything that touches project files:
//!
//! | Module | What it does |
//! |---|---|
//! | [`model`] | Typed, lossless model of the project JSON: [`Project`] and its parts. |
//! | [`validate`] | The limits the host enforces before it accepts a project. |
//! | [`media`] | Which fields are media paths; relative/absolute conversion; content hashes. |
//! | [`files`] | Reading and writing project files atomically, with the `<Project>.media` folder. |
//! | [`session`] | The open document: path, dirty flag, autosave, crash recovery, startup, templates. |
//! | [`relink`] | Missing media: inspection, bounded search for candidates, applying replacements. |
//! | [`midi`] | Standard MIDI File import and export. |
//!
//! # The usual calls
//!
//! ```no_run
//! use st_project::{session::Session, Project};
//! # fn main() -> st_project::Result<()> {
//! let mut session = Session::new("/path/to/support/dir")?;
//! let startup = session.startup()?;              // recovered session, default template, or none
//! // The interface publishes an edit:
//! let project = Project::from_json(r#"{"tracks":[{}],"instruments":[],"patterns":[{"length":64,"steps":[[]]}],"bpm":120}"#)?;
//! session.update(project, true)?;                // validates, marks dirty
//! session.autosave()?;                           // recovery file in the support directory
//! session.save_as("/music/Song.supertracker".as_ref())?; // atomic; collects media into Song.media
//! # let _ = startup; Ok(()) }
//! ```
//!
//! Nothing here is real-time code and nothing here is platform specific: every function runs
//! and is tested on Linux. File operations are synchronous; the host calls them off the
//! interface thread and passes a [`Cancel`] token to the long ones.
//!
//! # No older layouts
//!
//! Only the current project layout is read. A project that still uses a retired layout (a tempo
//! map in `timing`, kit lanes in `kits`, the built-in synth instrument) is rejected by
//! [`validate::validate`] with a message that says so; nothing is converted.

mod error;
pub mod files;
pub mod json;
pub mod media;
pub mod midi;
pub mod model;
pub mod relink;
pub mod session;
pub mod validate;

pub use error::{Error, Result};
pub use model::Project;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cancellation token for long file operations. Clones share one flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A token that is not cancelled.
    pub fn new() -> Cancel {
        Cancel::default()
    }
    /// Asks the operation holding a clone of this token to stop; it returns [`Error::Cancelled`].
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    /// Whether [`Cancel::cancel`] was called.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
