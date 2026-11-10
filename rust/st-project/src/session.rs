//! The open document: which file it belongs to, whether it has unsaved changes, autosave and
//! crash recovery, the startup session, and project templates.
//!
//! A [`Session`] works in a support directory the caller chooses (the application's support
//! folder; a temporary folder in tests). It writes there, and only there, unless asked to save:
//!
//! | File | Purpose |
//! |---|---|
//! | `Recovery.supertracker` | The open project as of the last [`Session::autosave`]. |
//! | `Recovery-session.json` | Which file that session belonged to and whether it had unsaved changes. |
//! | `Recovery Backups/Session <time>.supertracker` | The recovery file as each of the last five launches found it. |
//! | `Recovery (unreadable <time>).supertracker` | A recovery file that could not be read, kept for manual repair. |
//! | `Templates/<id>/Template.supertracker` (+ `Template.media`, `Metadata.json`) | Project templates. |
//! | `ProjectTemplates.json` | The default template. |
//!
//! Autosave never writes the user's project file; only [`Session::save`] and
//! [`Session::save_as`] do.

use crate::error::{Error, Result};
use crate::files::{self, read_project, write_atomic, write_project, PROJECT_EXTENSION};
use crate::media::{hash_file, sha256_hex, DEFAULT_HASH_LIMIT};
use crate::model::Project;
use crate::validate::validate;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Recovery files kept in `Recovery Backups`.
pub const RECOVERY_BACKUP_LIMIT: usize = 5;

/// What a save did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveOutcome {
    /// The file that was written.
    pub path: PathBuf,
    /// Media files copied into the media folder by this save.
    pub collected: usize,
    /// The project file was saved, but the recovery copy that follows a save could not be
    /// written; this is why.
    pub recovery_warning: Option<String>,
}

/// Where the startup session came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOrigin {
    /// The session an earlier run autosaved. [`Session::path`] and [`Session::is_dirty`] say
    /// whether it reattached to its file.
    Recovered,
    /// A new session from the default template with this id.
    Template(String),
    /// Nothing to open: the interface creates its blank project.
    Blank,
}

/// The session [`Session::startup`] opened.
#[derive(Debug, Clone)]
pub struct Startup {
    /// The project to show; `None` asks the interface for its blank project.
    pub project: Option<Project>,
    /// Where it came from.
    pub origin: StartupOrigin,
    /// Something the user should know: the recovery file or the default template could not be
    /// read (and where the unreadable session was kept).
    pub warning: Option<String>,
}

/// A saved project template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// Identity: letters, digits, `_` and `-`, at most 80 characters.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The template's project file.
    pub path: PathBuf,
}

/// The open document and its files. See the module documentation.
#[derive(Debug)]
pub struct Session {
    support: PathBuf,
    project: Option<Project>,
    path: Option<PathBuf>,
    dirty: bool,
    /// SHA-256 of `path` as this session last read or wrote it; recovery reattaches the file only
    /// while it still has this content.
    file_sha256: Option<String>,
}

fn civil_time(time: SystemTime) -> (i64, u32, u32, u32, u32, u32, u32) {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let (seconds, millis) = (elapsed.as_secs() as i64, elapsed.subsec_millis());
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day, (rest / 3600) as u32, (rest % 3600 / 60) as u32, (rest % 60) as u32, millis)
}

/// `2026-11-10T09:30:00Z` for `time` (UTC).
pub(crate) fn iso8601(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second, _) = civil_time(time);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn file_stamp(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second, millis) = civil_time(time);
    format!("{year:04}-{month:02}-{day:02} {hour:02}.{minute:02}.{second:02}.{millis:03}")
}

fn valid_template_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

impl Session {
    /// A session with no project, working in `support_dir` (created if needed).
    pub fn new(support_dir: impl Into<PathBuf>) -> Result<Session> {
        let support = support_dir.into();
        fs::create_dir_all(&support).map_err(|error| Error::io("create", &support, &error))?;
        Ok(Session { support, project: None, path: None, dirty: false, file_sha256: None })
    }

    /// The support directory.
    pub fn support_dir(&self) -> &Path {
        &self.support
    }
    /// The open project, as last published or loaded (media paths absolute).
    pub fn project(&self) -> Option<&Project> {
        self.project.as_ref()
    }
    /// The file the session belongs to; `None` for an untitled session.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    /// Whether the project has changes that are not in its file.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    /// The recovery file.
    pub fn recovery_file(&self) -> PathBuf {
        self.support.join(format!("Recovery.{PROJECT_EXTENSION}"))
    }
    /// The record that says which file the recovery file belongs to.
    pub fn recovery_record_file(&self) -> PathBuf {
        self.support.join("Recovery-session.json")
    }
    /// The folder of recovery files found by earlier launches.
    pub fn recovery_backups_dir(&self) -> PathBuf {
        self.support.join("Recovery Backups")
    }

    /// Takes a project the interface published. Validates it; on success it becomes the open
    /// project. With `dirty` the session becomes dirty if the project differs from the one held
    /// (publishing an unchanged project after a save does not make it dirty); without, the
    /// session is clean. Returns whether the project changed.
    pub fn update(&mut self, project: Project, dirty: bool) -> Result<bool> {
        validate(&project)?;
        let changed = self.project.as_ref() != Some(&project);
        self.project = Some(project);
        self.dirty = dirty && (self.dirty || changed);
        Ok(changed)
    }

    /// Starts an untitled, clean session with `project` (`None`: the interface's blank project
    /// follows through [`Session::update`]).
    pub fn accept_new_project(&mut self, project: Option<Project>) -> Result<()> {
        if let Some(project) = &project {
            validate(project)?;
        }
        self.project = project;
        self.path = None;
        self.file_sha256 = None;
        self.dirty = false;
        Ok(())
    }

    /// Opens a project file as the session's document: clean, attached to `file`.
    /// On failure the session is unchanged.
    pub fn open(&mut self, file: &Path) -> Result<&Project> {
        let project = read_project(file)?;
        self.file_sha256 = hash_file(file, DEFAULT_HASH_LIMIT, None).ok();
        self.path = Some(file.to_path_buf());
        self.dirty = false;
        Ok(self.project.insert(project))
    }

    /// Saves to the session's file. Fails for an untitled session (use [`Session::save_as`]).
    pub fn save(&mut self) -> Result<SaveOutcome> {
        let Some(path) = self.path.clone() else {
            return Err(Error::invalid("This project has not been saved yet. Choose where to save it."));
        };
        self.save_with(&path, None, true)
    }

    /// Saves to `destination`, collecting media into its `.media` folder, and makes it the
    /// session's file.
    pub fn save_as(&mut self, destination: &Path) -> Result<SaveOutcome> {
        self.save_with(destination, None, true)
    }

    /// Saves `snapshot` (or the open project) to `destination`.
    ///
    /// On success the session holds the saved project with its media paths pointing into the
    /// media folder, is attached to `destination` and is clean; a recovery copy follows. On
    /// failure nothing changes: the session, the previous file at `destination` and all media.
    pub fn save_with(&mut self, destination: &Path, snapshot: Option<Project>, collect_media: bool) -> Result<SaveOutcome> {
        let Some(current) = snapshot.as_ref().or(self.project.as_ref()) else {
            return Err(Error::invalid("There is no project to save."));
        };
        let written = write_project(current, destination, self.path.as_deref(), collect_media)?;
        self.project = Some(written.project);
        self.path = Some(destination.to_path_buf());
        self.file_sha256 = Some(written.file_sha256);
        self.dirty = false;
        // The project file is committed. A failing recovery copy must not turn that into a failure.
        let recovery_warning = self.autosave().err().map(|error| error.to_string());
        Ok(SaveOutcome { path: destination.to_path_buf(), collected: written.collected, recovery_warning })
    }

    /// Writes the open project to the recovery file (atomically), with the record of which file
    /// it belongs to. Does nothing without a project. Never touches the user's file.
    pub fn autosave(&self) -> Result<()> {
        let Some(project) = &self.project else { return Ok(()) };
        validate(project)?;
        let mut text = project.to_json_pretty();
        text.push('\n');
        write_atomic(&self.recovery_file(), text.as_bytes())?;
        let mut record = json!({ "recoverySHA256": sha256_hex(text.as_bytes()), "dirty": self.dirty });
        if let (Some(path), Some(hash)) = (&self.path, &self.file_sha256) {
            record["path"] = Value::String(path.to_string_lossy().into_owned());
            record["sha256"] = Value::String(hash.clone());
        }
        write_atomic(&self.recovery_record_file(), record.to_string().as_bytes())
    }

    /// Reopens the autosaved session, if there is one.
    ///
    /// The session keeps its file (and is dirty only if it had unsaved changes) when the record
    /// describes this recovery file and the project file on disk still has the content the
    /// session last read or wrote. Otherwise (file changed or gone, record missing or stale) it
    /// reopens untitled and dirty. On failure the session is unchanged.
    pub fn restore(&mut self) -> Result<Option<&Project>> {
        let recovery = self.recovery_file();
        if !recovery.exists() {
            return Ok(None);
        }
        let project = read_project(&recovery)?;
        self.path = None;
        self.file_sha256 = None;
        self.dirty = true;
        let record: Option<Value> = fs::read(self.recovery_record_file()).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let recovery_hash = fs::read(&recovery).ok().map(|bytes| sha256_hex(&bytes));
        if let (Some(record), Some(recovery_hash)) = (record, recovery_hash) {
            let text = |key: &str| record.get(key).and_then(Value::as_str);
            if let (Some(recorded), Some(path), Some(expected)) = (text("recoverySHA256"), text("path"), text("sha256")) {
                let file = Path::new(path);
                if recorded == recovery_hash && file.is_absolute() && hash_file(file, DEFAULT_HASH_LIMIT, None).ok().as_deref() == Some(expected) {
                    self.path = Some(file.to_path_buf());
                    self.file_sha256 = Some(expected.to_string());
                    self.dirty = record.get("dirty").and_then(Value::as_bool).unwrap_or(true);
                }
            }
        }
        Ok(Some(self.project.insert(project)))
    }

    /// Keeps a copy of the recovery file as this launch found it (distinct sessions only, the
    /// newest [`RECOVERY_BACKUP_LIMIT`] are kept), so a session replaced right after launch can
    /// still be opened from `Recovery Backups`.
    pub fn keep_launch_recovery_copy(&self) -> Result<()> {
        let Ok(data) = fs::read(self.recovery_file()) else { return Ok(()) };
        if data.is_empty() {
            return Ok(());
        }
        let folder = self.recovery_backups_dir();
        fs::create_dir_all(&folder).map_err(|error| Error::io("create", &folder, &error))?;
        let mut existing = self.recovery_backups();
        if let Some(newest) = existing.first() {
            if fs::read(newest).ok().as_deref() == Some(&data[..]) {
                return Ok(());
            }
        }
        // Names sort by time, so the newest copy is always the last name.
        let mut time = SystemTime::now();
        let mut target = folder.join(format!("Session {}.{PROJECT_EXTENSION}", file_stamp(time)));
        while target.exists() || existing.first().is_some_and(|newest| newest.file_name() >= target.file_name()) {
            time += std::time::Duration::from_millis(1);
            target = folder.join(format!("Session {}.{PROJECT_EXTENSION}", file_stamp(time)));
        }
        write_atomic(&target, &data)?;
        existing.insert(0, target);
        for old in existing.iter().skip(RECOVERY_BACKUP_LIMIT) {
            let _ = fs::remove_file(old);
        }
        Ok(())
    }

    /// The kept recovery files, newest first.
    pub fn recovery_backups(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = fs::read_dir(self.recovery_backups_dir())
            .map(|entries| entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|extension| extension == PROJECT_EXTENSION)).collect())
            .unwrap_or_default();
        files.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
        files
    }

    fn preserve_unreadable_recovery(&self) -> Result<PathBuf> {
        let stamp = iso8601(SystemTime::now()).replace(':', ".");
        let mut target = self.support.join(format!("Recovery (unreadable {stamp}).{PROJECT_EXTENSION}"));
        if target.exists() {
            target = self.support.join(format!("Recovery (unreadable {stamp} {}).{PROJECT_EXTENSION}", files::unique_token()));
        }
        fs::rename(self.recovery_file(), &target).map_err(|error| Error::io("keep", &target, &error))?;
        Ok(target)
    }

    /// Opens the session a launch starts with: the recovered session if there is one, else a
    /// new session from the default template, else none (the interface creates its blank
    /// project).
    ///
    /// This does not fail on unreadable files: an unreadable recovery file is moved aside (the
    /// first autosave of the new session would otherwise replace it) and an unreadable default
    /// template is skipped; [`Startup::warning`] says what happened.
    pub fn startup(&mut self) -> Result<Startup> {
        let _ = self.keep_launch_recovery_copy();
        let mut warning = None;
        match self.restore() {
            Ok(Some(project)) => return Ok(Startup { project: Some(project.clone()), origin: StartupOrigin::Recovered, warning: None }),
            Ok(None) => {}
            Err(error) => {
                let kept = self.preserve_unreadable_recovery().ok();
                let kept = kept.map(|path| format!(" The previous session was kept as {}.", path.display())).unwrap_or_default();
                warning = Some(format!("The previous session could not be recovered: {error}{kept}"));
            }
        }
        let template = self.default_template_id().and_then(|id| match id {
            Some(id) => self.template_project(&id).map(|project| Some((id, project))),
            None => Ok(None),
        });
        match template {
            Ok(Some((id, project))) => {
                self.accept_new_project(Some(project.clone()))?;
                Ok(Startup { project: Some(project), origin: StartupOrigin::Template(id), warning })
            }
            Ok(None) => {
                self.accept_new_project(None)?;
                Ok(Startup { project: None, origin: StartupOrigin::Blank, warning })
            }
            Err(error) => {
                self.accept_new_project(None)?;
                let message = error.to_string();
                let warning = Some(match warning {
                    Some(earlier) => format!("{earlier} {message}"),
                    None => message,
                });
                Ok(Startup { project: None, origin: StartupOrigin::Blank, warning })
            }
        }
    }

    /// The project a New command should open, resolved before the open session is replaced so a
    /// broken template never discards it: the template `template_id`, else the default template;
    /// `None` (also with `blank`) asks the interface for its blank project.
    pub fn proposed_new_project(&self, template_id: Option<&str>, blank: bool) -> Result<Option<Project>> {
        if blank {
            return Ok(None);
        }
        let id = match template_id {
            Some(id) => Some(id.to_string()),
            None => self.default_template_id()?,
        };
        id.map(|id| self.template_project(&id)).transpose()
    }

    // ---- Templates ---------------------------------------------------------------------------

    /// The templates folder.
    pub fn templates_dir(&self) -> PathBuf {
        self.support.join("Templates")
    }
    fn template_settings_file(&self) -> PathBuf {
        self.support.join("ProjectTemplates.json")
    }
    fn template_dir(&self, id: &str) -> Result<PathBuf> {
        if !valid_template_id(id) {
            return Err(Error::invalid("Invalid project template identifier."));
        }
        Ok(self.templates_dir().join(id))
    }

    /// The default template's id, if one is set.
    pub fn default_template_id(&self) -> Result<Option<String>> {
        let file = self.template_settings_file();
        if !file.exists() {
            return Ok(None);
        }
        let unreadable = || Error::invalid("Project template preferences are unreadable. Choose a default template again.");
        let settings: Value = serde_json::from_slice(&fs::read(&file).map_err(|_| unreadable())?).map_err(|_| unreadable())?;
        let Value::Object(settings) = settings else { return Err(unreadable()) };
        match settings.get("defaultID") {
            None => Ok(None),
            Some(Value::String(id)) => Ok(Some(id.clone())),
            Some(_) => Err(Error::invalid("The default project template setting is invalid. Choose a default template again.")),
        }
    }

    /// Sets or clears the default template. A template that cannot be opened is not accepted.
    pub fn set_default_template(&self, id: Option<&str>) -> Result<()> {
        if let Some(id) = id {
            self.template_project(id)?;
        }
        let settings = match id {
            Some(id) => json!({ "defaultID": id }),
            None => json!({}),
        };
        write_atomic(&self.template_settings_file(), settings.to_string().as_bytes())
    }

    /// Opens a template's project (media paths absolute, inside the template's folder).
    pub fn template_project(&self, id: &str) -> Result<Project> {
        let file = self.template_dir(id)?.join(format!("Template.{PROJECT_EXTENSION}"));
        read_project(&file).map_err(|error| {
            Error::invalid(format!("Project template {id} could not be opened: {error} Choose another default or save this template again."))
        })
    }

    /// The saved templates, by name.
    pub fn templates(&self) -> Vec<Template> {
        let mut templates: Vec<Template> = fs::read_dir(self.templates_dir())
            .map(|entries| entries.flatten().filter_map(|entry| self.read_template(&entry.path())).collect())
            .unwrap_or_default();
        templates.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        templates
    }

    fn read_template(&self, folder: &Path) -> Option<Template> {
        let metadata: Value = serde_json::from_slice(&fs::read(folder.join("Metadata.json")).ok()?).ok()?;
        let id = metadata.get("id")?.as_str()?;
        // The folder is the identity: metadata naming another folder is not a template of this one.
        if self.template_dir(id).ok()? != folder {
            return None;
        }
        Some(Template {
            id: id.to_string(),
            name: metadata.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            path: folder.join(format!("Template.{PROJECT_EXTENSION}")),
        })
    }

    /// Saves `project` as a template named `name` (1..=160 characters), with its media collected
    /// into the template's folder. `id` replaces an existing template; `None` makes a new one.
    /// The open session, its dirty flag and its recovery file are not touched.
    pub fn save_template(&self, project: &Project, name: &str, id: Option<&str>) -> Result<Template> {
        validate(project)?;
        let title = name.trim();
        if title.is_empty() || title.chars().count() > 160 {
            return Err(Error::invalid("Give the project template a name of 1–160 characters."));
        }
        let id = match id {
            Some(id) => id.to_string(),
            None => sha256_hex(files::unique_token().as_bytes())[..32].to_string(),
        };
        let folder = self.template_dir(&id)?;
        fs::create_dir_all(&folder).map_err(|error| Error::io("create", &folder, &error))?;
        let file = folder.join(format!("Template.{PROJECT_EXTENSION}"));
        write_project(project, &file, self.path.as_deref(), true)?;
        let metadata = json!({ "id": id, "name": title, "path": file.to_string_lossy() });
        write_atomic(&folder.join("Metadata.json"), metadata.to_string().as_bytes())?;
        Ok(Template { id, name: title.to_string(), path: file })
    }
}
