//! Offline rendering on a private engine: what export, bounce, render selection and freeze are
//! built on.
//!
//! An [`OfflineRender`] owns an [`AudioEngine`] without a device, made from the live engine
//! ([`AudioEngine::offline_copy`]: the same insert factory, an offline copy of its plug-in
//! manager, samples from the same cache) and driven by the thread that asked for the render.
//! The live engine keeps playing; nothing is paused, and nothing of the render reaches the
//! device.
//!
//! [`OfflineRender::run`] renders block by block through the same [`Renderer`] a device would
//! call and hands the audio to a [`RenderTarget`]: the master output, and any number of
//! *captures* of strips taken after their fader and before their output bus
//! ([`Plan::strip_output`]). All of them come from one pass through the complete graph, so
//! sidechains, shared returns and the master's processors see exactly the signal of normal
//! playback, whatever is captured. Every stream is delay-compensated: frame `n` of each is the
//! engine's frame `n`, whatever latency the strip's (or the master's) path has, which is why
//! stems line up with the mix.
//!
//! [`Renderer`]: crate::graph::render::Renderer
//! [`Plan::strip_output`]: crate::graph::plan::Plan::strip_output

use crate::engine::{AudioEngine, PlayOptions};
use crate::graph::spec::GraphSpec;
use crate::project_sync::ApplyReport;
use crate::samples::SampleSource;
use st_project::model::Project;
use std::sync::Arc;
use tracker_engine::engine::tables::MAX_BLOCK;

/// Frames per render block.
pub const BLOCK: usize = 512;

/// Where the audio of a run goes, and when the run ends.
pub trait RenderTarget {
    /// The next frames of capture `capture` (the index into the captures given to
    /// [`OfflineRender::run`]): `left.len()` frames starting at frame `start`. Consecutive calls
    /// for one capture are contiguous.
    fn write(&mut self, capture: usize, start: u64, left: &[f32], right: &[f32]) -> Result<(), String>;

    /// Called after every block. `available` frames have been written to every capture;
    /// `stopped` is true once the transport no longer plays. Return the total number of frames
    /// wanted once it is known: the run ends when every capture has them (frames past the total
    /// may have been written; the target ignores them). An error (a cancellation) ends the run.
    fn wanted(&mut self, available: u64, stopped: bool) -> Result<Option<u64>, String>;
}

/// What a run did.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// Frames every capture received.
    pub frames: u64,
    /// Frames rendered (the captures' frames plus the graph's latency).
    pub rendered: u64,
    /// Output latency of the graph, in frames.
    pub latency_frames: u64,
    /// Arrival of each capture: how many frames it lagged the engine before compensation.
    pub arrivals: Vec<u64>,
}

/// A private engine for one offline render. See the module documentation.
pub struct OfflineRender {
    engine: Arc<AudioEngine>,
    spec: GraphSpec,
}

impl OfflineRender {
    /// A private copy of `live` at `sample_rate`, with samples from `samples` (which should
    /// answer every lookup at once: [`crate::samples_async::BlockingSamples`]).
    pub fn new(live: &AudioEngine, samples: Arc<dyn SampleSource>, sample_rate: f64) -> Result<OfflineRender, String> {
        Ok(OfflineRender { engine: live.offline_copy(samples, sample_rate)?, spec: GraphSpec::empty() })
    }

    pub fn engine(&self) -> &Arc<AudioEngine> {
        &self.engine
    }

    pub fn sample_rate(&self) -> f64 {
        self.engine.sample_rate()
    }

    /// The project to render. Its plug-in instances are made (from the state its descriptors
    /// carry) before this returns.
    pub fn publish(&mut self, project: &Project) -> Result<ApplyReport, String> {
        let report = self.engine.publish_with(project, true)?;
        self.spec = GraphSpec::from_project(project);
        Ok(report)
    }

    /// The mixer of the published project: its strips are what can be captured.
    pub fn spec(&self) -> &GraphSpec {
        &self.spec
    }

    /// Seeks to `row` of `pattern` at sequence position `sequence` and starts playback (`song`:
    /// follow the sequence; else loop the pattern). Takes effect with the first block.
    pub fn play_from(&self, pattern: i32, row: i32, sequence: i32, song: bool) {
        self.engine.seek(Some(pattern), row, sequence);
        self.engine.play(&PlayOptions { song, from_start: false, pattern_loop: None, count_in: 0 });
    }

    /// Renders until `target` has what it wants. `captures` are strip owners (`track:<id>`,
    /// `bus:<id>`), or `None` for the master output. With `musical_frames` the transport is
    /// stopped once that many frames are rendered (held notes are released, tails ring out);
    /// blocks never straddle that frame.
    pub fn run(&self, captures: &[Option<String>], musical_frames: Option<u64>, target: &mut dyn RenderTarget) -> Result<RunSummary, String> {
        let mut strips = Vec::with_capacity(captures.len());
        for capture in captures {
            strips.push(match capture {
                None => None,
                Some(owner) => Some(self.spec.strips.iter().position(|strip| strip.id == *owner).ok_or_else(|| format!("The channel {owner} is not part of the project's mixer."))?),
            });
        }
        let block = BLOCK.min(MAX_BLOCK);
        let (mut left, mut right, silence) = (vec![0.0f32; block], vec![0.0f32; block], vec![0.0f32; block]);
        let shared = self.engine.renderer();
        let mut renderer = shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        // An offline render plays no external MIDI instrument and sends no clock.
        let midi_was_suspended = renderer.midi().suspend(true);
        let mut rendered = 0u64;
        let mut delivered = vec![0u64; captures.len()];
        let mut arrivals = vec![0u64; captures.len()];
        let mut stopped_for_tail = false;
        // A day of audio: nothing legitimate is longer, and a target that never decides must end.
        let limit = (self.engine.sample_rate() * 86_400.0) as u64;
        let result = loop {
            let mut count = block;
            if let Some(musical) = musical_frames {
                if rendered >= musical && !stopped_for_tail {
                    self.engine.stop();
                    stopped_for_tail = true;
                } else if rendered < musical {
                    count = count.min((musical - rendered) as usize);
                }
            }
            renderer.render(&mut left[..count], &mut right[..count]);
            let plan = renderer.plan();
            let mut failure = None;
            for (capture, strip) in strips.iter().enumerate() {
                let (signal, arrival) = match strip {
                    None => (Some((&left[..count], &right[..count])), plan.latency_frames),
                    Some(strip) => match plan.strip_output(*strip) {
                        Some((signal, arrival)) => (signal.map(|(l, r)| (&l[..count], &r[..count])), arrival),
                        None => {
                            failure = Some("The mixer changed during the render.".to_string());
                            break;
                        }
                    },
                };
                arrivals[capture] = arrival;
                // Graph frames [rendered, rendered + count) hold this capture's frames from
                // `rendered - arrival` on; what precedes frame 0 is the path's latency.
                let skip = arrival.saturating_sub(rendered).min(count as u64) as usize;
                if skip < count {
                    let start = rendered + skip as u64 - arrival;
                    let (l, r) = signal.unwrap_or((&silence[..count], &silence[..count]));
                    if let Err(error) = target.write(capture, start, &l[skip..], &r[skip..]) {
                        failure = Some(error);
                        break;
                    }
                    delivered[capture] = start + (count - skip) as u64;
                }
            }
            if let Some(error) = failure {
                break Err(error);
            }
            rendered += count as u64;
            let available = delivered.iter().copied().min().unwrap_or(0);
            let stopped = stopped_for_tail || !self.engine.is_playing();
            match target.wanted(available, stopped) {
                Ok(Some(total)) if available >= total => break Ok(available),
                Ok(_) if rendered >= limit => break Err("The render did not reach its end.".to_string()),
                Ok(_) => {}
                Err(error) => break Err(error),
            }
        };
        let latency_frames = renderer.plan().latency_frames;
        renderer.midi().suspend(midi_was_suspended);
        drop(renderer);
        result.map(|frames| RunSummary { frames, rendered, latency_frames, arrivals })
    }
}

impl Drop for OfflineRender {
    fn drop(&mut self) {
        self.engine.shutdown();
    }
}
