//! File choosers, as the command handlers see them. The window shows the platform's panels
//! (`st-app`); tests and headless runs answer from a script.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

use crate::{Error, Result};

/// What kind of chooser to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// Choose one existing file.
    OpenFile,
    /// Choose where to write a file (the platform asks before replacing one).
    SaveFile,
    /// Choose one existing folder.
    ChooseFolder,
}

impl DialogKind {
    fn label(self) -> &'static str {
        match self {
            DialogKind::OpenFile => "Open",
            DialogKind::SaveFile => "Save",
            DialogKind::ChooseFolder => "Choose Folder",
        }
    }
}

/// A group of file name extensions offered together ("Audio files: wav, aiff, flac").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    /// Without the dot, lower case.
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: &str, extensions: &[&str]) -> Self {
        FileFilter { name: name.to_string(), extensions: extensions.iter().map(|extension| extension.to_string()).collect() }
    }
}

/// One request for a file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDialog {
    pub kind: DialogKind,
    /// The panel's title or message.
    pub title: String,
    /// For [`DialogKind::SaveFile`]: the file name to propose.
    pub suggested_name: Option<String>,
    /// Empty accepts every file.
    pub filters: Vec<FileFilter>,
    /// The label of the confirming button, where the platform lets it be changed.
    pub confirm_label: Option<String>,
}

impl FileDialog {
    pub fn open_file(title: &str, filters: Vec<FileFilter>) -> Self {
        FileDialog { kind: DialogKind::OpenFile, title: title.to_string(), suggested_name: None, filters, confirm_label: None }
    }

    pub fn save_file(title: &str, suggested_name: &str, filters: Vec<FileFilter>) -> Self {
        FileDialog {
            kind: DialogKind::SaveFile,
            title: title.to_string(),
            suggested_name: Some(suggested_name.to_string()),
            filters,
            confirm_label: None,
        }
    }

    pub fn choose_folder(title: &str) -> Self {
        FileDialog { kind: DialogKind::ChooseFolder, title: title.to_string(), suggested_name: None, filters: Vec::new(), confirm_label: None }
    }

    pub fn confirm_label(mut self, label: &str) -> Self {
        self.confirm_label = Some(label.to_string());
        self
    }
}

/// Shows file choosers. Called on worker threads (the commands that use it are registered with
/// `register_blocking`), and returns when the person has chosen: `Ok(None)` means cancelled.
pub trait Dialogs: Send + Sync + 'static {
    fn choose(&self, dialog: &FileDialog) -> Result<Option<PathBuf>>;
}

/// For a host without a display and without scripted answers: every chooser fails with an
/// explanation. Commands that accept a `path` in their payload still work.
pub struct NoDialogs;

impl Dialogs for NoDialogs {
    fn choose(&self, dialog: &FileDialog) -> Result<Option<PathBuf>> {
        Err(Error::new(format!(
            "The {} window (“{}”) cannot be shown: this host runs without a window. Send the path with the request, or start the host with --dialog-answers.",
            dialog.kind.label(),
            dialog.title
        )))
    }
}

/// Answers choosers from a list instead of showing them: `--dialog-answers <file>` and tests.
///
/// The file is a JSON array. Each chooser takes the next element: a string is the chosen path,
/// `null` is "the person cancelled". The file is read again for every chooser, so a test can append
/// answers while the host runs; answers already used are skipped by position.
///
/// ```json
/// ["/tmp/session/song.supertracker", null, "/tmp/session/song.supertracker"]
/// ```
pub struct ScriptedDialogs {
    source: Source,
    state: Mutex<Script>,
}

enum Source {
    File(PathBuf),
    Memory,
}

#[derive(Default)]
struct Script {
    answers: Vec<Option<PathBuf>>,
    used: usize,
    asked: Vec<FileDialog>,
}

impl ScriptedDialogs {
    /// Answers from the JSON file at `path` (see the type's documentation).
    pub fn from_file(path: &Path) -> Self {
        ScriptedDialogs { source: Source::File(path.to_path_buf()), state: Mutex::default() }
    }

    /// Answers from a list: `Some(path)` chooses, `None` cancels.
    pub fn from_answers(answers: Vec<Option<PathBuf>>) -> Self {
        ScriptedDialogs { source: Source::Memory, state: Mutex::new(Script { answers, ..Script::default() }) }
    }

    /// Adds an answer to an in-memory script.
    pub fn push(&self, answer: Option<PathBuf>) {
        self.lock().answers.push(answer);
    }

    /// The choosers requested so far.
    pub fn asked(&self) -> Vec<FileDialog> {
        self.lock().asked.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Script> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn read_answers(path: &Path) -> Result<Vec<Option<PathBuf>>> {
    let text = std::fs::read_to_string(path).map_err(|error| Error::new(format!("Could not read the dialog answers file {}: {error}.", path.display())))?;
    let malformed = || Error::new(format!("The dialog answers file {} must be a JSON array of paths and nulls.", path.display()));
    let Value::Array(entries) = serde_json::from_str::<Value>(&text).map_err(|_| malformed())? else {
        return Err(malformed());
    };
    entries
        .into_iter()
        .map(|entry| match entry {
            Value::String(path) => Ok(Some(PathBuf::from(path))),
            Value::Null => Ok(None),
            _ => Err(malformed()),
        })
        .collect()
}

impl Dialogs for ScriptedDialogs {
    fn choose(&self, dialog: &FileDialog) -> Result<Option<PathBuf>> {
        let mut script = self.lock();
        script.asked.push(dialog.clone());
        if let Source::File(path) = &self.source {
            script.answers = read_answers(path)?;
        }
        let Some(answer) = script.answers.get(script.used).cloned() else {
            return Err(Error::new(format!(
                "No scripted answer is left for the {} window “{}” (answer {} was requested).",
                dialog.kind.label(),
                dialog.title,
                script.used + 1
            )));
        };
        script.used += 1;
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn scripted_answers_are_used_in_order_and_run_out_with_an_explanation() {
        let dialogs = ScriptedDialogs::from_answers(vec![Some(PathBuf::from("/tmp/a.supertracker")), None]);
        let save = FileDialog::save_file("Save Project", "Song.supertracker", vec![]);
        assert_eq!(dialogs.choose(&save).unwrap(), Some(PathBuf::from("/tmp/a.supertracker")));
        assert_eq!(dialogs.choose(&save).unwrap(), None);
        let error = dialogs.choose(&save).unwrap_err();
        assert_eq!(error.message(), "No scripted answer is left for the Save window “Save Project” (answer 3 was requested).");
        assert_eq!(dialogs.asked().len(), 3);
    }

    #[test]
    fn an_answers_file_is_reread_so_answers_can_be_appended() {
        let folder = TempDir::new("dialogs");
        let file = folder.path().join("answers.json");
        std::fs::write(&file, r#"["/tmp/one"]"#).unwrap();
        let dialogs = ScriptedDialogs::from_file(&file);
        let open = FileDialog::open_file("Open Project", vec![]);
        assert_eq!(dialogs.choose(&open).unwrap(), Some(PathBuf::from("/tmp/one")));
        assert!(dialogs.choose(&open).is_err());
        std::fs::write(&file, r#"["/tmp/one", null]"#).unwrap();
        assert_eq!(dialogs.choose(&open).unwrap(), None);
    }

    #[test]
    fn a_malformed_answers_file_is_an_error_not_a_cancel() {
        let folder = TempDir::new("dialogs-bad");
        let file = folder.path().join("answers.json");
        std::fs::write(&file, r#"{"path": "/tmp/one"}"#).unwrap();
        let dialogs = ScriptedDialogs::from_file(&file);
        let error = dialogs.choose(&FileDialog::choose_folder("Folder")).unwrap_err();
        assert!(error.message().contains("must be a JSON array of paths and nulls"), "{error}");
    }

    #[test]
    fn a_host_without_dialogs_says_what_to_do_instead() {
        let error = NoDialogs.choose(&FileDialog::open_file("Open Project", vec![])).unwrap_err();
        assert!(error.message().contains("--dialog-answers"), "{error}");
    }
}
