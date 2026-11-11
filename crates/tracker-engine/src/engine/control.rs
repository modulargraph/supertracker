//! Control-thread side of the engine: the editable draft, the published
//! control copy, commit (`commit`, `normalize`, `buildTables`), reclamation (`collect`) and engine
//! construction/destruction.
//!
//! One serial control thread owns everything here. It publishes a complete immutable project
//! through `Shared::pending`; the render thread adopts it and hands the previous one back on the
//! `Shared::retired` list, which `collect()` frees together with unreferenced samples.

use super::project::{Project, Sample};
use super::state::{Command, Render, Rt, EngineCore, Shared};
use super::tables::ChannelTables;
use super::abi::default_track;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Source of process-unique pattern content identities (`pattern_epoch`).
static PATTERN_EPOCHS: AtomicU64 = AtomicU64::new(0);

pub const BAD_ALLOC: &str = "std::bad_alloc";

/// The error of a control operation that was handed another engine's `Shared`.
pub const FOREIGN_ENGINE: &str = "The shared state belongs to another engine.";

pub struct Control {
    /// The engine this control state belongs to (`Shared::engine_id`).
    pub(crate) engine_id: u64,
    /// The project being edited inside (or implicitly outside) an update; `None` when there are
    /// no unpublished edits. Declared before `samples`: projects release their sample
    /// references when dropped, so they must go first.
    pub(crate) draft: Option<Box<Project>>,
    /// The control thread's copy of the last published project.
    pub(crate) control: Box<Project>,
    pub(crate) updating: bool,
    pub(crate) samples: Vec<Box<Sample>>,
    pub(crate) last_input_request_id: u64,
}

impl Control {
    /// Whether `sh` is the shared state of this control state's own engine. Projects, samples
    /// and commands only ever travel between the two halves of one engine: every method that
    /// takes a `&Shared` does nothing (or fails) when handed another engine's.
    #[inline]
    pub(crate) fn owns(&self, sh: &Shared) -> bool {
        sh.engine_id == self.engine_id
    }

    /// Queue a command for this engine's render half.
    pub(crate) fn queue(&self, sh: &Shared, command: &Command) {
        if self.owns(sh) {
            sh.queue(command);
        }
    }

    /// Queue Play/Stop/count-in for this engine's render half.
    pub(crate) fn queue_transport(&self, sh: &Shared, command: Command, playing_after: bool) {
        if self.owns(sh) {
            sh.queue_transport(command, playing_after);
        }
    }

    /// The draft, created from the published project on first use.
    pub(crate) fn edit(&mut self) -> &mut Project {
        let control = &self.control;
        self.draft.get_or_insert_with(|| Box::new((**control).clone()))
    }

    /// The project control-thread edits apply to, without creating a draft.
    pub(crate) fn view(&self) -> &Project {
        match &self.draft {
            Some(draft) => draft,
            None => &self.control,
        }
    }

    /// The draft for a pattern edit: its pattern content no longer matches any published epoch.
    pub(crate) fn edit_patterns(&mut self) -> &mut Project {
        let project = self.edit();
        project.pattern_epoch = 0;
        project
    }

    /// Destroy retired projects, then the samples nothing refers to any more.
    pub fn collect(&mut self, sh: &Shared) {
        if !self.owns(sh) {
            return;
        }
        let mut retired = sh.retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
        while !retired.is_null() {
            // SAFETY: every pointer on the retired list came from `Box::into_raw` at commit and
            // was pushed exactly once by the render thread, which never touches a project again
            // after retiring it; the swap above made this thread the list's sole owner.
            let project = unsafe { Box::from_raw(retired) };
            retired = project.retired_next;
        }
        self.samples.retain(|sample| sample.refs.load(Ordering::Acquire) != 0);
    }

    /// `normalize`: pad the track list to the return bus and cache the solo state.
    fn normalize(p: &mut Project) {
        let base = p.return_bus() as usize;
        if p.tracks.len() < base {
            p.tracks.resize(base, default_track());
        }
        p.any_solo = false;
        for channel in 0..p.track_count as usize {
            p.any_solo |= p.tracks.get(channel).is_some_and(|track| track.solo != 0);
        }
    }

    /// Control thread: give the project per-channel render tables. The latest published tables
    /// are reused while the track count is unchanged; otherwise new tables are allocated here,
    /// never on render.
    fn build_tables(&mut self, p: &mut Project) -> Result<(), String> {
        let reusable = p.tables.as_ref().is_some_and(|tables| tables.same_shape(p.track_count));
        if !reusable {
            match &self.control.tables {
                Some(tables) if tables.same_shape(p.track_count) => p.tables = Some(tables.clone()),
                _ => {
                    p.tables = Some(Arc::new(ChannelTables::new(p.track_count).ok_or(BAD_ALLOC)?));
                }
            }
        }
        Ok(())
    }

    /// Compile, validate and publish the draft. On failure the previous published project stays
    /// and the draft is retained for further edits.
    pub fn commit(&mut self, sh: &Shared) -> Result<(), String> {
        if !self.owns(sh) {
            return Err(FOREIGN_ENGINE.into());
        }
        let Some(mut draft) = self.draft.take() else { return Ok(()) };
        match self.prepare(&mut draft) {
            Ok(()) => {}
            Err(problem) => {
                self.draft = Some(draft);
                return Err(problem);
            }
        }
        let next = Box::new((*draft).clone());
        self.control = draft;
        let old = sh.pending.swap(Box::into_raw(next), Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: a non-null `pending` is a `Box::into_raw` from an earlier commit that the
            // render thread did not take (its swap would have left null); the swap above
            // transferred it back to this thread.
            drop(unsafe { Box::from_raw(old) });
        }
        self.updating = false;
        self.collect(sh);
        Ok(())
    }

    fn prepare(&mut self, draft: &mut Project) -> Result<(), String> {
        Self::normalize(draft);
        // Only lane lists edited in this draft can be unsorted; shared ones were sorted when
        // first published (and cannot be mutated here).
        for pattern in &mut draft.patterns {
            if !pattern.lanes_dirty {
                continue;
            }
            for list in [&mut pattern.lanes, &mut pattern.host_lanes] {
                if let Some(lanes) = list.as_mut().and_then(Arc::get_mut) {
                    for lane in lanes {
                        lane.points.sort_by(|a, b| a.row.partial_cmp(&b.row).unwrap_or(std::cmp::Ordering::Equal));
                    }
                }
            }
            pattern.lanes_dirty = false;
        }
        self.build_tables(draft)?;
        if draft.pattern_epoch == 0 {
            draft.pattern_epoch = PATTERN_EPOCHS.fetch_add(1, Ordering::Relaxed) + 1;
        }
        Ok(())
    }
}

impl EngineCore {
    /// `None` for an unsupported sample rate or when memory is exhausted.
    pub fn create(rate: f64) -> Option<Box<EngineCore>> {
        if !rate.is_finite() || rate < 8000.0 || rate > 384000.0 {
            return None;
        }
        let shared = Shared::new();
        let mut control = Control {
            engine_id: shared.engine_id,
            draft: None,
            control: Box::new(Project::new()),
            updating: false,
            samples: Vec::new(),
            last_input_request_id: 0,
        };
        let mut first = Box::new(Project::new());
        control.build_tables(&mut first).ok()?;
        control.control = first;
        let mut current = Box::new(Project::new());
        current.tables = control.control.tables.clone();
        let tb = current.tables.as_ref()?.state_ptr();
        let render = Render::new(shared.engine_id, rate, Box::into_raw(current), tb);
        let mut engine = Box::new(EngineCore {
            sample_rate: rate,
            shared,
            control: UnsafeCell::new(control),
            render: UnsafeCell::new(render),
        });
        let EngineCore { shared, render, .. } = &mut *engine;
        // SAFETY: the engine is not shared yet, so this thread stands in for the render thread;
        // no project adoption can happen while the view exists.
        unsafe { Rt::attach(render.get_mut(), shared) }.reset_track_state(false);
        Some(engine)
    }
}

impl Drop for EngineCore {
    /// Runs when the last handle of the engine is dropped, so no borrow of either half exists
    /// and this thread owns both.
    fn drop(&mut self) {
        let EngineCore { shared, render, control, .. } = self;
        let render = render.get_mut();
        // SAFETY: no render call overlaps destruction (API contract), so this thread stands in
        // for the render thread; no adoption happens while the view exists.
        unsafe { Rt::attach(render, shared) }.all_off(true, false);
        let pending = shared.pending.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !pending.is_null() {
            // SAFETY: an unadopted `Box::into_raw` project from commit; ownership returns here.
            drop(unsafe { Box::from_raw(pending) });
        }
        let current = std::mem::replace(&mut render.current, std::ptr::null_mut());
        if !current.is_null() {
            // SAFETY: `current` is the `Box::into_raw` project the render thread adopted (or the
            // one made at creation) and was never pushed onto `retired`.
            drop(unsafe { Box::from_raw(current) });
        }
        control.get_mut().collect(shared);
        // The draft and control projects, then the samples, drop with `Control`.
    }
}
