//! Live line inputs and strip taps of [`AudioEngine`].
//!
//! The project is the only description of line inputs: `tracks[i].audioInput` says which
//! channels of which device feed the track. With every publication the engine tells
//! [`crate::inputs::Inputs`] the enabled routes, which opens the input when one needs it and
//! answers with a feed or with the reason a route is paused, and adds the live ones to the
//! mixer it compiles ([`crate::graph::spec::LineInputSpec`]). Nothing else configures a line
//! input, and nothing about one is remembered outside the project. The routes are resolved
//! again when the devices change ([`AudioEngine::refresh_inputs`]; the engine's tick does it
//! when an output or input is lost or returns).
//!
//! A frozen track's input is closed like its plug-ins are unloaded: the print holds what it
//! played ([`crate::freeze::playback_project`]).
//!
//! # The latency of a line input
//!
//! A hardware instrument that follows the song (played by MIDI from the render thread) answers
//! late: its note leaves when the block is heard, and its audio comes back through the input
//! device and the feed's ring. A live input is therefore a source with latency in the mixer's
//! delay compensation:
//!
//! ```text
//!   latency = output latency  +  input latency                    +  the feed's delay
//!             (one block; 0      (what the device reported when      (crate::inputs::LineFeed::latency)
//!              on the null        it was opened; 0 for the
//!              device)            null input)
//! ```
//!
//! The feed's delay is one block when the input is the output's own device: the frames pass
//! through as they are. An input with a clock of its own (any other device) is resampled onto
//! the output's clock, and its feed keeps an output block, an input block, half the
//! converter's filter and a margin waiting: 1 310 frames between two 48 kHz devices with
//! 512-frame blocks, where one device has 512 ([`crate::inputs`], "Two clocks"). Either way
//! it is a constant of the feed, in the engine's frames.
//!
//! Everything else is delayed by the sum, so the return lines up with the tracks the engine
//! sounds itself, and a tap on the strip knows which song frame each of its frames belongs
//! to. The figure is an estimate from what the devices say, fixed while the input stays open;
//! it has not been measured against hardware.

use super::{AudioEngine, DeviceChoice, State};
use crate::device::NULL_DEVICE;
use crate::graph::spec::{FeedHandle, GraphSpec, LineInputSpec, StripKind};
use crate::graph::tap::StripTap;
use crate::inputs::{LineRoute, LineState};
use serde_json::{json, Value};
use st_project::model::Project;
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// One enabled line input of the published project and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResolvedLine {
    pub track: usize,
    /// Gain of the input in the strip (0 while the route is not monitored).
    pub gain: f32,
    pub state: LineState,
    /// The input's latency in the mixer, in frames (see the module documentation).
    pub latency_frames: u32,
}

/// Adds the live inputs to a spec read from their project.
pub(super) fn apply(spec: &mut GraphSpec, lines: &[ResolvedLine]) {
    for line in lines {
        if let LineState::Live { feed, .. } = &line.state {
            spec.set_line_input(line.track, Some(LineInputSpec { feed: FeedHandle(feed.clone()), gain: line.gain, latency_frames: line.latency_frames }));
        }
    }
}

/// The enabled line inputs of a project as it is played, with each one's gain in its strip.
fn routes(project: &Project) -> Vec<(LineRoute, f32)> {
    let mut routes = Vec::new();
    for (track, item) in project.track_list().iter().enumerate() {
        let Some(input) = item.audio_input.as_ref().filter(|input| input.enabled()) else { continue };
        let channels: Vec<u32> = input.channels.as_deref().unwrap_or(&[]).iter().filter(|channel| (0..256).contains(*channel)).map(|channel| *channel as u32).collect();
        let (Some(left), true) = (channels.first().copied(), channels.len() <= 2) else { continue };
        let gain = if input.monitor() && input.gain().is_finite() { input.gain().clamp(0.0, 2.0) as f32 } else { 0.0 };
        routes.push((LineRoute { track, device_uid: input.device_uid.clone().unwrap_or_default(), channels: [left, channels.get(1).copied().unwrap_or(left)] }, gain));
    }
    routes
}

impl AudioEngine {
    /// Tells the input which line inputs `project` routes and returns what became of each.
    /// Opens or closes the input device: call it without the state's lock.
    pub(super) fn resolve_lines(&self, project: &Project) -> Vec<ResolvedLine> {
        let routes = routes(&crate::freeze::playback_project(project));
        let (rate, block, largest, output, follow_default) = {
            let state = self.lock();
            let block = state.stream.as_ref().map_or(state.buffer_size, |stream| stream.buffer_size());
            let largest = state.stream.as_ref().map_or(block, |stream| stream.largest_block());
            (state.core.compiler.sample_rate(), block, largest, state.stream.as_ref().map_or(String::new(), |stream| stream.uid().to_string()), state.wanted != DeviceChoice::None)
        };
        let inputs = self.live.inputs();
        if routes.is_empty() {
            inputs.set_line_routes(&[], follow_default);
            return Vec::new();
        }
        inputs.set_format(rate, block, &output);
        inputs.set_output_block(largest);
        // What the output adds to a hardware return: the block the render runs ahead of it.
        let output_latency = if output == NULL_DEVICE || !follow_default { 0 } else { largest };
        let wanted: Vec<LineRoute> = routes.iter().map(|(route, _)| route.clone()).collect();
        let states = inputs.set_line_routes(&wanted, follow_default);
        routes
            .into_iter()
            .zip(states)
            .map(|((route, gain), state)| {
                let latency_frames = match &state {
                    LineState::Live { feed, input_latency } => output_latency + input_latency + feed.latency(),
                    LineState::Paused(_) => 0,
                };
                ResolvedLine { track: route.track, gain, state, latency_frames }
            })
            .collect()
    }

    /// Resolves the published project's line inputs against the devices as they are now (an
    /// input was chosen, lost or connected; the output changed) and recompiles the mixer when
    /// a route changed. Returns whether one did.
    pub fn refresh_inputs(&self) -> Result<bool, String> {
        let _gate = self.publish_gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.refresh_gated()
    }

    fn refresh_gated(&self) -> Result<bool, String> {
        self.lines_stale.store(false, Ordering::Release);
        let Some(project) = self.project() else { return Ok(false) };
        let lines = self.resolve_lines(&project);
        let mut state = self.lock();
        if lines == state.lines {
            return Ok(false);
        }
        let previous = std::mem::replace(&mut state.lines, lines);
        match Self::publish_locked(&mut state, &project) {
            Ok(report) => {
                state.last_report = report;
                Ok(true)
            }
            Err(error) => {
                // The mixer is the one compiled before: so is the list it was compiled from.
                state.lines = previous;
                Err(error)
            }
        }
    }

    /// The tick's part: resolves the line inputs again when the devices changed or a paused
    /// input may be tried again, unless a publication is running (then at a later tick).
    pub(super) fn refresh_lines_if_due(&self) -> bool {
        // An output that asks for larger blocks than it was opened with needs other feeds.
        let largest = self.lock().stream.as_ref().map(|stream| stream.largest_block());
        if let Some(largest) = largest {
            self.live.inputs().set_output_block(largest);
        }
        if !self.lines_stale.load(Ordering::Acquire) && !self.live.inputs().line_retry_due() {
            return false;
        }
        let Ok(_gate) = self.publish_gate.try_lock() else { return false };
        self.refresh_gated().unwrap_or(false)
    }

    /// The line inputs of the published project for `audio.devices`: one object per track that
    /// has an `audioInput` (`{track, state: "live" / "paused" / "off", active, status?,
    /// monitoring, latencyFrames?, clock?, driftPpm?, underruns?, overruns?}`), and the reason
    /// the first route that should run does not, if one does not. A live route says whether
    /// its input runs on the output's clock (`clock: "shared"`) or is resampled from a clock
    /// of its own (`"converted"`, with the measured difference of the two in `driftPpm`), how
    /// often the input did not deliver in time (`underruns`) and how often audio was dropped
    /// because too much waited (`overruns`).
    pub fn line_inputs(&self) -> (Vec<Value>, Option<String>) {
        let state = self.lock();
        let Some(project) = &state.project else { return (Vec::new(), None) };
        let mut problem = None;
        let mut rows = Vec::new();
        for (track, item) in project.track_list().iter().enumerate() {
            let Some(input) = &item.audio_input else { continue };
            let resolved = state.lines.iter().find(|line| line.track == track);
            let frozen = item.frozen.as_ref().is_some_and(|record| record.path.as_deref().is_some_and(|path| !path.is_empty()));
            let mut row = json!({ "track": track, "state": "off", "active": false, "monitoring": false });
            match (input.enabled(), resolved.map(|line| (&line.state, line))) {
                (false, _) => {}
                (true, Some((LineState::Live { feed, .. }, line))) => {
                    row["state"] = json!("live");
                    row["active"] = json!(true);
                    row["monitoring"] = json!(input.monitor());
                    row["latencyFrames"] = json!(line.latency_frames);
                    row["clock"] = json!(if feed.drift_ppm().is_some() { "converted" } else { "shared" });
                    if let Some(drift) = feed.drift_ppm() {
                        row["driftPpm"] = json!((drift * 10.0).round() / 10.0);
                    }
                    row["underruns"] = json!(feed.underruns());
                    row["overruns"] = json!(feed.overruns());
                }
                (true, Some((LineState::Paused(reason), _))) => {
                    row["state"] = json!("paused");
                    row["status"] = json!(reason);
                    problem.get_or_insert_with(|| reason.clone());
                }
                (true, None) => {
                    row["state"] = json!("paused");
                    row["status"] = json!(if frozen { "The track is frozen: its input is closed while it plays its print." } else { "The input's channels cannot be used." });
                }
            }
            rows.push(row);
        }
        (rows, problem)
    }

    /// How often the live input of track `track` has had a gap or a skip so far (its feed's
    /// underruns and overruns together); `None` when the track has no live input.
    pub fn line_input_glitches(&self, track: usize) -> Option<u64> {
        self.lock().lines.iter().find(|line| line.track == track).and_then(|line| match &line.state {
            LineState::Live { feed, .. } => Some(feed.underruns() + feed.overruns()),
            LineState::Paused(_) => None,
        })
    }

    /// Puts a tap on the strip of track `track`, after its inserts and before its fader, or
    /// takes the tap away (`None`). The mixer is recompiled with it; every plan compiled while
    /// the tap is set carries it. See [`crate::graph::tap`].
    pub fn set_strip_tap(&self, tap: Option<(usize, Arc<StripTap>)>) -> Result<(), String> {
        let _gate = self.publish_gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self.lock();
        let State { core, .. } = &mut *state;
        let taps = match tap {
            Some((track, tap)) => {
                let owner = core.spec.strips.iter().find(|strip| strip.kind == StripKind::Track(track)).map(|strip| strip.id.clone());
                vec![(owner.ok_or("That track has no mixer strip to record.")?, tap)]
            }
            None => Vec::new(),
        };
        core.compiler.set_taps(taps);
        let plan = core.compiler.compile(&core.spec)?;
        core.meters = plan.meters().clone();
        core.shared.publish(plan);
        Ok(())
    }

    /// Whether the render thread uses the mixer compiled last (a device that renders adopts a
    /// new one with its next block).
    pub fn plan_adopted(&self) -> bool {
        let state = self.lock();
        state.core.shared.adopted_serial() == state.core.compiler.serial()
    }

    /// Whether a device renders this engine in real time.
    pub fn is_live(&self) -> bool {
        self.lock().stream.as_ref().is_some_and(|stream| stream.is_alive())
    }

    /// Whether the transport plays, as the status reports it: a Play or Stop that still waits
    /// for the audio thread counts as done.
    pub fn transport_playing(&self) -> bool {
        self.lock().core.status.status().transport_playing()
    }
}
