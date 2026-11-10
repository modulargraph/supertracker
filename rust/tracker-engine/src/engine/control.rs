//! Control-thread side of `STEngine` in `TrackerEngine.cpp`: the editable draft, the published
//! control copy, commit (`commit`, `normalize`, `buildTables`), reclamation (`collect`) and engine
//! construction/destruction.
//!
//! One serial control thread owns everything here. It publishes a complete immutable project
//! through `Shared::pending`; the render thread adopts it and hands the previous one back on the
//! `Shared::retired` list, which `collect()` frees together with unreferenced samples and maps.

use super::timing::EngineTiming;
use super::project::{Project, Sample};
use super::state::{Render, Rt, STEngine, Shared};
use super::tables::{BusRing, ChannelTables, PeakStore};
use super::abi::default_track;
use super::util::{cmax, try_box_with};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Source of process-unique pattern content identities (`st_engine_pattern_epoch`).
static PATTERN_EPOCHS: AtomicU64 = AtomicU64::new(0);

pub const BAD_ALLOC: &str = "std::bad_alloc";

pub struct Control {
    /// The project being edited inside (or implicitly outside) an update; `None` when there are
    /// no unpublished edits. Declared before `samples`: projects release their sample
    /// references when dropped, so they must go first.
    pub draft: Option<Box<Project>>,
    /// The control thread's copy of the last published project.
    pub control: Box<Project>,
    pub updating: bool,
    pub samples: Vec<Box<Sample>>,
    pub timing_maps: Vec<Arc<EngineTiming>>,
    /// Control-owned bus history, keyed by logical bus so renumbering keeps it.
    pub track_rings: Vec<Option<Arc<BusRing>>>,
    pub return_ring: Option<Arc<BusRing>>,
    pub peak_stores: Vec<Box<PeakStore>>,
    pub last_input_request_id: u64,
}

impl Control {
    /// The draft, created from the published project on first use.
    pub fn edit(&mut self) -> &mut Project {
        let control = &self.control;
        self.draft.get_or_insert_with(|| Box::new((**control).clone()))
    }

    /// `edit()` for the setters that report an allocation failure instead of dying (the C++
    /// functions with a `catch`): the draft's storage is reserved first, so a failed allocation
    /// leaves the engine unchanged.
    pub fn try_edit(&mut self) -> Result<&mut Project, String> {
        if self.draft.is_none() {
            let control = &self.control;
            self.draft = Some(try_box_with(|| (**control).clone()).ok_or(BAD_ALLOC)?);
        }
        Ok(self.edit())
    }

    /// The project control-thread edits apply to, without creating a draft.
    pub fn view(&self) -> &Project {
        match &self.draft {
            Some(draft) => draft,
            None => &self.control,
        }
    }

    /// The draft for a pattern edit: its pattern content no longer matches any published epoch.
    pub fn edit_patterns(&mut self) -> &mut Project {
        let project = self.edit();
        project.pattern_epoch = 0;
        project
    }

    /// Destroy retired projects, then samples and timing maps nothing refers to any more.
    pub fn collect(&mut self, sh: &Shared) {
        let mut retired = sh.retired.swap(std::ptr::null_mut(), Ordering::AcqRel);
        while !retired.is_null() {
            // SAFETY: every pointer on the retired list came from `Box::into_raw` at commit and
            // was pushed exactly once by the render thread, which never touches a project again
            // after retiring it; the swap above made this thread the list's sole owner.
            let project = unsafe { Box::from_raw(retired) };
            retired = project.retired_next;
        }
        self.samples.retain(|sample| sample.refs.load(Ordering::Acquire) != 0);
        self.timing_maps.retain(|map| !(Arc::strong_count(map) == 1 && map.origin_pins.load(Ordering::Acquire) == 0));
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
    /// are reused while the track count is unchanged; otherwise new tables (and history rings for
    /// new buses) are allocated here, never on render.
    fn build_tables(&mut self, sh: &Shared, p: &mut Project) -> Result<(), String> {
        let reusable = p.tables.as_ref().is_some_and(|tables| tables.same_shape(p.track_count));
        if !reusable {
            match &self.control.tables {
                Some(tables) if tables.same_shape(p.track_count) => p.tables = Some(tables.clone()),
                _ => {
                    self.track_rings.resize(p.track_count as usize, None);
                    for ring in &mut self.track_rings {
                        if ring.is_none() {
                            *ring = Some(Arc::new(BusRing::new().ok_or(BAD_ALLOC)?));
                        }
                    }
                    if self.return_ring.is_none() {
                        self.return_ring = Some(Arc::new(BusRing::new().ok_or(BAD_ALLOC)?));
                    }
                    let base = p.return_bus() as usize;
                    let mut rings: Vec<Option<Arc<BusRing>>> = vec![None; base + 1];
                    for (slot, ring) in rings.iter_mut().zip(&self.track_rings) {
                        *slot = ring.clone();
                    }
                    rings[base] = self.return_ring.clone();
                    p.tables = Some(Arc::new(ChannelTables::new(p.track_count, rings).ok_or(BAD_ALLOC)?));
                }
            }
        }
        let needed = p.tables.as_ref().map_or(0, |tables| tables.channels as usize);
        let store = sh.peak_store.load(Ordering::Acquire);
        // SAFETY: a non-null peak store is one of the boxes in `peak_stores`, which are never
        // freed before the engine.
        let current_count = if store.is_null() { None } else { Some(unsafe { &*store }.values.len()) };
        if current_count.is_none_or(|count| count < needed) {
            let grown = Box::new(PeakStore::new(cmax(needed, current_count.map_or(needed, |count| count * 2))));
            sh.peak_store.store(&*grown as *const PeakStore as *mut PeakStore, Ordering::Release);
            self.peak_stores.push(grown);
        }
        Ok(())
    }

    /// Compile, validate and publish the draft. On failure the previous published project stays
    /// and the draft is retained for further edits.
    pub fn commit(&mut self, sh: &Shared, sample_rate: f64) -> Result<(), String> {
        let Some(mut draft) = self.draft.take() else { return Ok(()) };
        match self.prepare(sh, sample_rate, &mut draft) {
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

    fn prepare(&mut self, sh: &Shared, sample_rate: f64, draft: &mut Project) -> Result<(), String> {
        Self::normalize(draft);
        draft.compile_timing(sample_rate)?;
        draft.validate_frozen_extent(sample_rate)?;
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
        self.build_tables(sh, draft)?;
        if draft.pattern_epoch == 0 {
            draft.pattern_epoch = PATTERN_EPOCHS.fetch_add(1, Ordering::Relaxed) + 1;
        }
        Ok(())
    }
}

impl STEngine {
    /// `st_engine_create`: `None` for an unsupported sample rate or when memory is exhausted.
    pub fn create(rate: f64) -> Option<Box<STEngine>> {
        if !rate.is_finite() || rate < 8000.0 || rate > 384000.0 {
            return None;
        }
        let shared = Shared::new();
        let mut control = Control {
            draft: None,
            control: Box::new(Project::new()),
            updating: false,
            samples: Vec::new(),
            timing_maps: Vec::new(),
            track_rings: Vec::new(),
            return_ring: None,
            peak_stores: Vec::new(),
            last_input_request_id: 0,
        };
        let mut first = Box::new(Project::new());
        control.build_tables(&shared, &mut first).ok()?;
        control.control = first;
        let mut current = Box::new(Project::new());
        current.tables = control.control.tables.clone();
        let tb = current.tables.as_ref()?.state_ptr();
        let render = Render::new(rate, Box::into_raw(current), tb);
        let mut engine = Box::new(STEngine {
            sample_rate: rate,
            shared,
            control: UnsafeCell::new(control),
            render: UnsafeCell::new(render),
        });
        let STEngine { shared, render, .. } = &mut *engine;
        // SAFETY: the engine is not shared yet, so this thread stands in for the render thread;
        // no project adoption can happen while the view exists.
        unsafe { Rt::attach(render.get_mut(), shared) }.reset_track_state(false);
        Some(engine)
    }
}

impl Drop for STEngine {
    /// `~STEngine`. The engine must be destroyed only after the audio graph has stopped calling
    /// it, so this thread owns both halves.
    fn drop(&mut self) {
        let STEngine { shared, render, control, .. } = self;
        let render = render.get_mut();
        let history = shared.freeze_history.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !history.is_null() {
            // SAFETY: the history was leaked from a Box by `st_engine_set_frozen_song_policy`
            // and is freed only here.
            drop(unsafe { Box::from_raw(history) });
        }
        render.release_clock_origin();
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
        // The draft and control projects, then the samples, maps and rings, drop with `Control`.
    }
}
