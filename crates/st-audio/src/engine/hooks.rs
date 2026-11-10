//! What the areas built on the playback core need from [`AudioEngine`]: the engine's voice
//! positions (sampler playheads), and later the live output stage and the offline engine.
//! A child module of [`crate::engine`], so that these additions stay out of that file.

use super::AudioEngine;
use serde_json::{json, Value};
use st_project::model::Project;
use tracker_engine::engine::abi::{STVoicePosition, ST_VOICE_POSITION_CAPACITY, ST_VOICE_POSITION_PREVIEW, ST_VOICE_POSITION_RELEASING, ST_VOICE_POSITION_ZONE};

impl AudioEngine {
    /// Which instrument's voices the engine reports positions for: an instrument index, -1 for
    /// every instrument, anything lower to stop. Survives nothing: call it again after the
    /// engine restarted at another sample rate (the playhead poll simply repeats it).
    pub fn watch_voice_positions(&self, instrument: i32) {
        self.lock().core.status.watch_voice_positions(instrument);
    }

    /// The latest snapshot of sounding voices and its serial (which changes when the snapshot
    /// does); `None` when no stable snapshot could be read.
    pub fn voice_positions(&self, out: &mut [STVoicePosition]) -> Option<(usize, u64)> {
        self.lock().core.status.voice_positions(out)
    }

    /// The snapshot as the interface draws it (`sampler.playheads`): one object per voice with
    /// `instrument`, `track`, `note`, `mode`, `direction`, `releasing`, `zone`, `preview`,
    /// `position`, `windowStart`, `windowEnd`, `frames`, `level` and `grains` (`[position,
    /// level]` pairs), and the snapshot's serial.
    pub fn playheads(&self) -> Option<(Vec<Value>, u64)> {
        let mut voices = [STVoicePosition::default(); ST_VOICE_POSITION_CAPACITY];
        let (count, serial) = self.voice_positions(&mut voices)?;
        let voices = voices[..count]
            .iter()
            .map(|voice| {
                let grains = (voice.grain_count.max(0) as usize).min(voice.grain_positions.len());
                let grains: Vec<[f64; 2]> = (0..grains).map(|index| [voice.grain_positions[index], voice.grain_levels[index] as f64]).collect();
                json!({
                    "instrument": voice.instrument, "track": voice.track, "note": voice.note, "mode": voice.mode, "direction": voice.direction,
                    "releasing": voice.flags & ST_VOICE_POSITION_RELEASING != 0, "zone": voice.flags & ST_VOICE_POSITION_ZONE != 0,
                    "preview": voice.flags & ST_VOICE_POSITION_PREVIEW != 0,
                    "position": voice.position, "windowStart": voice.window_start, "windowEnd": voice.window_end, "frames": voice.sample_frames,
                    "level": voice.level, "grains": grains,
                })
            })
            .collect();
        Some((voices, serial))
    }

    /// Reads the published project without copying it.
    pub fn with_project<R>(&self, read: impl FnOnce(Option<&Project>) -> R) -> R {
        read(self.lock().project.as_ref())
    }

    /// Gives the renderer in use the stage that runs on the live output (preview voices). The
    /// stage it replaces is dropped here, on a control thread.
    pub(super) fn attach_live(&self) {
        let mut renderer = self.renderer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let stage = self.live.stage(renderer.sample_rate());
        let previous = renderer.set_output_stage(Some(stage));
        drop(renderer);
        drop(previous);
    }

    /// The live output stage's control side: browser previews.
    pub fn live(&self) -> &std::sync::Arc<crate::live::Live> {
        &self.live
    }

    /// A private engine for an offline render (export, bounce, render selection, freeze): no
    /// device, this engine's insert factory, an offline copy of its plug-in manager (its own
    /// containers; instances start from the state their descriptors carry) and `samples`.
    pub fn offline_copy(&self, samples: std::sync::Arc<dyn crate::samples::SampleSource>, sample_rate: f64) -> Result<std::sync::Arc<AudioEngine>, String> {
        let engine = AudioEngine::new(super::EngineOptions { sample_rate, buffer_size: 512, samples, inserts: self.inserts.clone(), device: super::DeviceChoice::None })?;
        if let Some(manager) = self.plugins() {
            engine.set_plugins(manager.offline()?);
        }
        Ok(engine)
    }

    /// Whether the transport plays, as the render thread last reported it.
    pub fn is_playing(&self) -> bool {
        self.lock().core.status.status().playing != 0
    }
}
