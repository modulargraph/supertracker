//! MIDI beat clock: 24 pulses per quarter note, generated from the transport position of each
//! audio render block, and the Start / Stop / Continue messages that follow the transport.
//!
//! Clock alone sets a device's tempo (arpeggiators, synced LFOs and delays) without running its
//! sequencer; Start, Stop and Continue do run it and go only to outputs that asked for them. While
//! the transport is stopped the clock keeps running at the project tempo, so devices stay in tempo.
//!
//! [`ClockGenerator`] is pure and runs on the audio thread (no allocation); [`ClockSettings`] is
//! the stored choice.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

/// Pulses per quarter-note beat.
pub const PULSES_PER_BEAT: f64 = 24.0;
/// The longest clock lead, in seconds.
pub const MAXIMUM_LEAD_SECONDS: f64 = 0.5;

/// One audio render block as the clock sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockBlock {
    /// Song position, in quarter-note beats, at the block's first frame.
    pub beat: f64,
    /// Whether the transport is playing.
    pub playing: bool,
    /// Tempo in beats per minute.
    pub tempo: f64,
    /// Frames in the block.
    pub frames: u32,
    /// Sample rate in hertz.
    pub sample_rate: f64,
    /// Device time of the block's first frame, in ticks. Zero (unknown) generates nothing.
    pub host_time: u64,
    /// Device ticks per frame.
    pub ticks_per_frame: f64,
}

/// What the generator asks to be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockEvent {
    /// Status byte: 0xF8 clock, 0xFA Start, 0xFB Continue, 0xFC Stop, 0xF2 Song Position Pointer.
    pub status: u8,
    /// First data byte (Song Position only).
    pub data1: u8,
    /// Second data byte (Song Position only).
    pub data2: u8,
    /// Device time to send it at.
    pub time: u64,
    /// True for the messages that run a device's sequencer: they go only to outputs with transport
    /// enabled. Clock pulses go to every clock output.
    pub transport: bool,
}

/// Generates the clock of one output set, block by block. Owned by the audio thread.
#[derive(Debug, Clone, Default)]
pub struct ClockGenerator {
    was_playing: bool,
    free_beat: f64,
}

impl ClockGenerator {
    /// A generator at rest.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the last block rendered was playing.
    pub fn was_playing(&self) -> bool {
        self.was_playing
    }

    /// Renders one block.
    ///
    /// `lead_seconds` sends the clock that much ahead of the song, so that a device whose audio
    /// returns through the audio interface is heard in time. With `active` false (no output wants
    /// clock) nothing is emitted and only the transport state is followed.
    pub fn render(&mut self, block: &ClockBlock, lead_seconds: f64, active: bool, mut emit: impl FnMut(ClockEvent)) {
        let usable = block.host_time > 0 && block.tempo.is_finite() && block.tempo > 0.0 && block.sample_rate > 0.0 && block.beat.is_finite();
        if !usable {
            return;
        }
        if !active {
            self.was_playing = block.playing;
            return;
        }
        let lead = if lead_seconds.is_finite() { lead_seconds.clamp(0.0, MAXIMUM_LEAD_SECONDS) } else { 0.0 };
        let beats_per_frame = block.tempo / 60.0 / block.sample_rate;
        let span = f64::from(block.frames) * beats_per_frame;
        let start = if block.playing { block.beat.max(0.0) } else { self.free_beat };
        if block.playing != self.was_playing {
            let transport = |status, data1, data2| ClockEvent { status, data1, data2, time: block.host_time, transport: true };
            if !block.playing {
                emit(transport(0xFC, 0, 0));
            } else if start < 1e-6 {
                emit(transport(0xFA, 0, 0));
            } else {
                // Song Position Pointer counts sixteenth notes; Continue resumes from it.
                let sixteenths = ((start * 4.0).floor() as u64).min(16_383) as u16;
                emit(transport(0xF2, (sixteenths & 0x7F) as u8, (sixteenths >> 7) as u8));
                emit(transport(0xFB, 0, 0));
            }
            self.was_playing = block.playing;
        }
        // The pulses of the beats `lead` ahead of this block are sent during it.
        let ahead = start + lead * block.tempo / 60.0;
        let mut pulse = (ahead * PULSES_PER_BEAT - 1e-9).ceil();
        while pulse / PULSES_PER_BEAT < ahead + span {
            let offset = ((pulse / PULSES_PER_BEAT - ahead) / beats_per_frame).max(0.0);
            emit(ClockEvent { status: 0xF8, data1: 0, data2: 0, time: block.host_time + (offset * block.ticks_per_frame) as u64, transport: false });
            pulse += 1.0;
        }
        self.free_beat = start + span;
        if self.free_beat > 1e9 {
            self.free_beat = 0.0;
        }
    }
}

/// What one MIDI output receives. Outputs default to clock on, Start/Stop off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClockDevice {
    /// Clock pulses.
    pub clock: bool,
    /// Start, Stop and Continue.
    pub transport: bool,
}

impl Default for ClockDevice {
    fn default() -> Self {
        ClockDevice { clock: true, transport: false }
    }
}

/// The settings section `midi.clock`: whether clock is sent at all, the choice per output (by
/// device id), and the lead.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClockSettings {
    /// Send MIDI clock.
    pub enabled: bool,
    /// Per output, keyed by the device id as text. Outputs not listed use the defaults.
    pub devices: BTreeMap<String, ClockDevice>,
    /// Clock lead in milliseconds; `None` follows the measured input-path latency.
    pub lead_milliseconds: Option<f64>,
}

/// Payload of `midi.clock`. An empty request only reads.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClockRequest {
    /// Turn the clock on or off.
    pub enabled: Option<bool>,
    /// Outputs to change; fields left out keep their value.
    pub devices: Option<Vec<ClockDeviceRequest>>,
    /// `Some(None)` (a JSON `null`) returns to the automatic lead; `Some(Some(ms))` sets it.
    #[serde(deserialize_with = "present")]
    pub lead_milliseconds: Option<Option<f64>>,
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<f64>>, D::Error> {
    Option::<f64>::deserialize(deserializer).map(Some)
}

/// One output of a [`ClockRequest`].
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
pub struct ClockDeviceRequest {
    /// The output's device id.
    pub id: Option<i64>,
    /// Send clock pulses to it.
    pub clock: Option<bool>,
    /// Send Start, Stop and Continue to it.
    pub transport: Option<bool>,
}

impl ClockRequest {
    /// True when the request changes nothing (it reads the settings).
    pub fn is_read(&self) -> bool {
        self.enabled.is_none() && self.devices.is_none() && self.lead_milliseconds.is_none()
    }
}

impl ClockSettings {
    /// The choice for the output with this id.
    pub fn device(&self, id: i32) -> ClockDevice {
        self.devices.get(&id.to_string()).copied().unwrap_or_default()
    }

    /// Applies a request: only the fields it names change.
    pub fn apply(&mut self, request: &ClockRequest) {
        if let Some(enabled) = request.enabled {
            self.enabled = enabled;
        }
        match request.lead_milliseconds {
            Some(None) => self.lead_milliseconds = None,
            Some(Some(lead)) if lead.is_finite() => self.lead_milliseconds = Some(lead.clamp(0.0, 500.0)),
            _ => {}
        }
        for item in request.devices.iter().flatten() {
            let Some(id) = item.id.and_then(|id| i32::try_from(id).ok()) else { continue };
            let mut entry = self.device(id);
            if let Some(clock) = item.clock {
                entry.clock = clock;
            }
            if let Some(transport) = item.transport {
                entry.transport = transport;
            }
            self.devices.insert(id.to_string(), entry);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Ported from tests/MIDIClockOutputTests.swift.
    use super::*;
    use serde_json::json;

    struct Fixture {
        clock: ClockGenerator,
        sent: Vec<(u8, u64)>,
        full: Vec<ClockEvent>,
        /// Whether the single output takes transport messages (it always takes clock when active).
        transport: bool,
        active: bool,
        lead: f64,
    }

    impl Fixture {
        // 120 BPM at 48 kHz: one beat is 24 000 frames, one pulse 1000 frames. One tick per frame.
        fn block(&mut self, beat: f64, playing: bool, frames: u32) {
            let block = ClockBlock { beat, playing, tempo: 120.0, frames, sample_rate: 48_000.0, host_time: 1_000_000, ticks_per_frame: 1.0 };
            let (sent, full, transport) = (&mut self.sent, &mut self.full, self.transport);
            self.clock.render(&block, self.lead, self.active, |event| {
                if !event.transport || transport {
                    sent.push((event.status, event.time));
                    full.push(event);
                }
            });
        }

        fn pulses(&self) -> Vec<u64> {
            self.sent.iter().filter(|(status, _)| *status == 0xF8).map(|(_, time)| *time).collect()
        }
    }

    #[test]
    fn pulses_transport_and_lead() {
        let mut f = Fixture { clock: ClockGenerator::new(), sent: Vec::new(), full: Vec::new(), transport: false, active: false, lead: 0.0 };
        f.block(0.0, true, 4800);
        assert!(f.sent.is_empty(), "nothing is sent without outputs");
        assert!(f.clock.was_playing(), "but the transport is followed");

        f.active = true;
        f.block(0.0, true, 4800);
        assert_eq!(f.pulses(), [1_000_000, 1_001_000, 1_002_000, 1_003_000, 1_004_000], "pulses every 1/24 beat");
        assert!(!f.sent.iter().any(|(status, _)| [0xFA, 0xFB, 0xFC].contains(status)), "clock-only outputs never get Start/Stop");

        f.sent.clear();
        f.transport = true;
        f.block(0.2, true, 4800);
        f.sent.clear();
        f.block(0.4, false, 4800);
        assert_eq!(f.sent.first().map(|event| event.0), Some(0xFC), "Stop goes to transport outputs");
        assert_eq!(f.pulses().len(), 5, "the clock keeps running while stopped");
        f.sent.clear();
        f.block(0.0, true, 4800);
        assert!(f.sent[0].0 == 0xFA && f.sent[1].0 == 0xF8, "Play from the start sends Start before the first pulse");
        f.sent.clear();
        f.block(0.2, false, 4800);
        f.sent.clear();
        f.full.clear();
        f.block(2.5, true, 4800);
        assert!(f.sent.len() >= 2 && f.sent[0].0 == 0xF2 && f.sent[1].0 == 0xFB, "Play mid-song sends Song Position then Continue");
        assert_eq!((f.full[0].data1, f.full[0].data2), (10, 0), "2.5 beats are ten sixteenths");
        assert!(f.full[0].transport && f.full[1].transport && !f.full[2].transport);
        f.sent.clear();
        f.block(1.0 / 48.0, true, 600);
        assert_eq!(f.sent, [(0xF8, 1_000_500)], "a pulse between block starts lands at its frame");

        f.sent.clear();
        f.transport = false;
        f.lead = 0.01;
        f.block(0.2, true, 4800);
        f.sent.clear();
        f.block(1.0 / 48.0, true, 600);
        assert_eq!(f.pulses().len(), 1);
        assert!((f.sent[0].1 as i64 - 1_000_020).abs() <= 1, "a 10 ms lead sends each pulse 10 ms (480 frames) early: {:?}", f.sent);
    }

    #[test]
    fn consecutive_blocks_tile_without_gaps_or_duplicates() {
        // 512-frame blocks at 44.1 kHz and 133 BPM: no pulse may be lost or doubled at block edges.
        let mut clock = ClockGenerator::new();
        let (mut count, mut last) = (0u64, 0u64);
        let (tempo, rate, frames) = (133.0, 44_100.0, 512u32);
        for index in 0..2000u64 {
            let beat = index as f64 * f64::from(frames) * tempo / 60.0 / rate;
            let block = ClockBlock { beat, playing: true, tempo, frames, sample_rate: rate, host_time: 1_000 + index * u64::from(frames), ticks_per_frame: 1.0 };
            clock.render(&block, 0.0, true, |event| {
                if event.status == 0xF8 {
                    assert!(event.time > last || count == 0, "pulses are in time order");
                    last = event.time;
                    count += 1;
                }
            });
        }
        let beats = 2000.0 * f64::from(frames) * tempo / 60.0 / rate;
        assert_eq!(count, (beats * PULSES_PER_BEAT).ceil() as u64);
    }

    #[test]
    fn unusable_blocks_generate_nothing() {
        let mut clock = ClockGenerator::new();
        let good = ClockBlock { beat: 0.0, playing: true, tempo: 120.0, frames: 4800, sample_rate: 48_000.0, host_time: 1, ticks_per_frame: 1.0 };
        for bad in [
            ClockBlock { host_time: 0, ..good },
            ClockBlock { tempo: 0.0, ..good },
            ClockBlock { tempo: f64::NAN, ..good },
            ClockBlock { sample_rate: 0.0, ..good },
            ClockBlock { beat: f64::INFINITY, ..good },
        ] {
            clock.render(&bad, 0.0, true, |event| panic!("unexpected {event:?}"));
        }
        assert!(!clock.was_playing());
    }

    #[test]
    fn preferences() {
        let mut settings = ClockSettings::default();
        assert_eq!(settings.device(9), ClockDevice { clock: true, transport: false }, "outputs default to clock on, Start/Stop off");
        let request: ClockRequest = serde_json::from_value(json!({"enabled": true, "devices": [{"id": 9, "transport": true}]})).unwrap();
        assert!(!request.is_read());
        settings.apply(&request);
        assert!(settings.enabled && settings.device(9) == ClockDevice { clock: true, transport: true }, "a request changes only the fields it names");
        settings.apply(&serde_json::from_value(json!({"leadMilliseconds": 18})).unwrap());
        assert_eq!(settings.lead_milliseconds, Some(18.0), "a manual clock lead is kept");
        settings.apply(&serde_json::from_value(json!({"devices": [{"id": 9, "clock": false}, {"clock": false}, {"id": 1e12}]})).unwrap());
        assert_eq!(settings.device(9), ClockDevice { clock: false, transport: true });
        assert_eq!(settings.devices.len(), 1, "entries without a usable id are skipped");
        assert_eq!(settings.lead_milliseconds, Some(18.0));
        settings.apply(&serde_json::from_value(json!({"leadMilliseconds": 9000})).unwrap());
        assert_eq!(settings.lead_milliseconds, Some(500.0), "the lead is limited to half a second");
        settings.apply(&serde_json::from_value(json!({"leadMilliseconds": null})).unwrap());
        assert_eq!(settings.lead_milliseconds, None, "an empty lead returns to automatic");
        assert!(serde_json::from_value::<ClockRequest>(json!({})).unwrap().is_read());

        let stored = serde_json::to_value(&settings).unwrap();
        assert_eq!(stored, json!({"enabled": true, "devices": {"9": {"clock": false, "transport": true}}, "leadMilliseconds": null}));
        assert_eq!(serde_json::from_value::<ClockSettings>(stored).unwrap(), settings);
        assert_eq!(serde_json::from_value::<ClockSettings>(json!({})).unwrap(), ClockSettings::default());
    }
}
