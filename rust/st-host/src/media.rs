//! Missing media: the `project.media.*` commands, on top of `st_project::relink`.
//!
//! Nothing here changes the session. Inspection and search report; `project.media.apply` returns the
//! relinked project, which the interface adopts and publishes like any other edit.
//!
//! A search can walk a whole drive, so it runs on a worker thread (all of these commands are
//! blocking commands), reports through `project.media.progress` events, and stops when
//! `project.media.cancel` arrives or a newer search starts.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Deserialize;
use serde_json::{json, Value};
use st_project::relink::{self, Limits, Relocation, Selection};
use st_project::Cancel;

use crate::bridge::EventSink;
use crate::session::{absolute, Session};
use crate::{Commands, Error, NoPayload, Result};

const NO_PROJECT: &str = "Open a project before relinking media.";

pub(crate) struct MediaRelink {
    session: Arc<Session>,
    events: EventSink,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_operation: u64,
    /// The running search.
    search: Option<(u64, Cancel)>,
    /// Running inspections and applications, by operation number.
    workers: Vec<(u64, Cancel)>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct InspectPayload {
    project: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SearchPayload {
    project: Option<Value>,
    roots: Vec<PathBuf>,
    relocations: Vec<RelocationPayload>,
    /// Echoed in the result and in every progress event of this search.
    request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelocationPayload {
    from: PathBuf,
    to: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ApplyPayload {
    project: Option<Value>,
    selections: Option<Vec<Selection>>,
}

/// Removes a worker's cancellation token when its operation ends, however it ends.
struct Worker<'a> {
    owner: &'a MediaRelink,
    operation: u64,
    cancel: Cancel,
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        self.owner.state().workers.retain(|(operation, _)| *operation != self.operation);
    }
}

fn home_relative(path: PathBuf) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path,
    }
}

impl MediaRelink {
    pub(crate) fn register(session: Arc<Session>, events: EventSink, commands: &mut Commands) {
        let service = Arc::new(MediaRelink { session, events, state: Mutex::default() });
        commands
            .with(&service)
            .register_blocking("project.media.inspect", MediaRelink::inspect)
            .register_blocking("project.media.search", MediaRelink::search)
            .register_blocking("project.media.apply", MediaRelink::apply)
            // Quick, so that it overtakes the search it cancels.
            .register("project.media.cancel", MediaRelink::cancel);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn worker(&self) -> Worker<'_> {
        let mut state = self.state();
        state.next_operation += 1;
        let (operation, cancel) = (state.next_operation, Cancel::new());
        state.workers.push((operation, cancel.clone()));
        Worker { owner: self, operation, cancel }
    }

    /// `project.media.inspect`: every media reference of the project and whether its file is there.
    fn inspect(&self, payload: InspectPayload) -> Result<Value> {
        let project = self.session.candidate(payload.project, NO_PROJECT)?;
        let worker = self.worker();
        let report = relink::inspect(&project, self.session.path().as_deref(), &Limits::default());
        if worker.cancel.is_cancelled() {
            return Err(Error::new(st_project::Error::Cancelled.to_string()));
        }
        Ok(serde_json::to_value(report)?)
    }

    /// `project.media.search`: looks for the missing files in folders and under moved roots.
    fn search(&self, payload: SearchPayload) -> Result<Value> {
        let project = self.session.candidate(payload.project, NO_PROJECT)?;
        let invalid = || Error::new("Choose up to 64 absolute media folders or root mappings.");
        let roots: Vec<PathBuf> = payload.roots.into_iter().map(home_relative).collect();
        let relocations: Vec<Relocation> = payload.relocations.into_iter().map(|moved| Relocation { from: moved.from, to: moved.to }).collect();
        let all_absolute = roots.iter().all(|root| root.is_absolute()) && relocations.iter().all(|moved| moved.from.is_absolute() && moved.to.is_absolute());
        if roots.len() > relink::MAX_SEARCH_ROOTS || relocations.len() > relink::MAX_SEARCH_ROOTS || !all_absolute {
            return Err(invalid());
        }
        // One search at a time: a newer one replaces the one still walking.
        let (operation, cancel) = {
            let mut state = self.state();
            if let Some((_, previous)) = state.search.take() {
                previous.cancel();
            }
            state.next_operation += 1;
            let started = (state.next_operation, Cancel::new());
            state.search = Some(started.clone());
            started
        };
        let request_id = payload.request_id.unwrap_or_else(|| format!("search-{operation}"));
        let operation_id = format!("media-search-{operation}");
        let announce = |mut event: Value| {
            event["requestId"] = json!(request_id);
            event["operationId"] = json!(operation_id);
            self.events.emit("project.media.progress", event);
        };
        announce(json!({ "phase": "starting", "scannedFiles": 0 }));
        let found = relink::find_candidates(&project, self.session.path().as_deref(), &roots, &relocations, &Limits::default(), &cancel, &mut |progress| {
            if !cancel.is_cancelled() {
                announce(serde_json::to_value(progress).unwrap_or_else(|_| json!({})));
            }
        });
        let still_current = {
            let mut state = self.state();
            let current = state.search.as_ref().is_some_and(|(running, _)| *running == operation);
            if current {
                state.search = None;
            }
            current
        };
        let report = found?;
        if cancel.is_cancelled() {
            return Err(Error::new(st_project::Error::Cancelled.to_string()));
        }
        if still_current {
            announce(json!({ "phase": "complete", "scannedFiles": report.scanned_files.unwrap_or(0), "missingCount": report.missing_count }));
        }
        let mut result = serde_json::to_value(report)?;
        result["requestId"] = json!(request_id);
        result["operationId"] = json!(operation_id);
        Ok(result)
    }

    /// `project.media.apply`: replaces the selected references and returns the relinked project.
    fn apply(&self, payload: ApplyPayload) -> Result<Value> {
        let project = self.session.candidate(payload.project, NO_PROJECT)?;
        let Some(selections) = payload.selections else {
            return Err(Error::new("Select replacement files before applying media relinks."));
        };
        for selection in &selections {
            absolute(PathBuf::from(&selection.to))?;
        }
        let worker = self.worker();
        let applied = relink::apply(&project, &selections, self.session.path().as_deref(), relink::DEFAULT_APPLY_HASH_BYTES, &worker.cancel)?;
        st_project::validate::validate(&applied.project)?;
        Ok(serde_json::to_value(applied)?)
    }

    /// `project.media.cancel`: stops the running search and every inspection or application.
    fn cancel(&self, _: NoPayload) -> Result<Value> {
        let mut state = self.state();
        let mut operations = state.workers.len();
        for (_, cancel) in &state.workers {
            cancel.cancel();
        }
        if let Some((operation, cancel)) = state.search.take() {
            operations += 1;
            cancel.cancel();
            drop(state);
            self.events.emit("project.media.progress", json!({ "phase": "cancelled", "operationId": format!("media-search-{operation}") }));
        }
        Ok(json!({ "cancelled": true, "operations": operations }))
    }
}
