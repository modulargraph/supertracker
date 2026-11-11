//! Event translation: from the block's [`Event`] list to calls of an Audio Unit's
//! schedule blocks, and from the unit's MIDI output back to [`Event`]s. Platform
//! independent (tested on Linux); the macOS side only invokes the blocks.
//!
//! Timing model. The processor numbers the frames it renders with its own running sample
//! counter and passes that as the render timestamp. An event at `offset` in a block that
//! starts at sample `t` is scheduled at the absolute time `t + offset`, immediately before
//! the render call that covers it, on the render thread. Absolute times (rather than
//! `AUEventSampleTimeImmediate + offset`, which is only defined for offsets below 4096)
//! are exact for any block size, and stay exact when a block is rendered in several
//! slices because the unit's `maximumFramesToRender` is smaller than the block.

use stpc_runtime::{Event, EVENT_MIDI, EVENT_PARAM};

/// `AUEventSampleTimeImmediate`.
pub const IMMEDIATE: i64 = -4_294_967_296;
/// Offsets that may be added to [`IMMEDIATE`].
pub const IMMEDIATE_OFFSET_LIMIT: i64 = 4096;

/// The other documented way to time an event: "immediately" plus an offset into the render
/// call that follows. `absolute` is the event's absolute time, `slice_time` the time stamp
/// of that render call. Only used when `STPC_AU_EVENT_TIME=immediate` is set (a switch for
/// comparing the two on a Mac); offsets are limited to what the constant allows.
pub fn immediate_time(absolute: i64, slice_time: i64) -> i64 {
    IMMEDIATE + absolute.saturating_sub(slice_time).clamp(0, IMMEDIATE_OFFSET_LIMIT - 1)
}

/// One call of a schedule block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scheduled {
    /// `scheduleParameterBlock(time, ramp, address, value)`.
    Parameter { time: i64, ramp: u32, address: u64, value: f32 },
    /// `scheduleMIDIEventBlock(time, cable, len, bytes)`.
    Midi { time: i64, cable: u8, len: u8, bytes: [u8; 3] },
}

impl Scheduled {
    pub fn time(&self) -> i64 {
        match *self {
            Scheduled::Parameter { time, .. } | Scheduled::Midi { time, .. } => time,
        }
    }
}

/// Length of a complete MIDI 1.0 message with this status byte: `None` for a data byte
/// and for system exclusive (not carried by the protocol).
pub fn midi_message_length(status: u8) -> Option<usize> {
    match status {
        0x00..=0x7F => None,
        0x80..=0xBF | 0xE0..=0xEF => Some(3),
        0xC0..=0xDF => Some(2),
        0xF0 | 0xF7 => None,
        0xF1 | 0xF3 => Some(2),
        0xF2 => Some(3),
        // Undefined system common (F4, F5) and the single-byte messages.
        0xF4..=0xF6 | 0xF8..=0xFF => Some(1),
    }
}

/// What the translation needs to know about the unit.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    /// The unit has a MIDI input (`scheduleMIDIEventBlock` is not nil).
    pub accepts_midi: bool,
    /// `virtualMIDICableCount`, at least 1 when `accepts_midi`.
    pub cables: u8,
    /// Sorted addresses of the parameters that may be ramped.
    pub rampable: &'a [u64],
}

/// Translates one event of a block. `block_start` is the sample time of the block's first
/// frame, `frames` the block length. Returns `None` for events the unit cannot take:
/// non-finite parameter values, malformed or system-exclusive MIDI, MIDI for a unit
/// without MIDI input, unknown kinds.
pub fn translate(event: &Event, block_start: i64, frames: u32, target: &Target<'_>) -> Option<Scheduled> {
    // The client clamps offsets to the block; a region is never trusted, though.
    let offset = event.offset.min(frames.saturating_sub(1));
    let time = block_start + offset as i64;
    match event.kind {
        EVENT_PARAM => {
            if !event.value.is_finite() {
                return None;
            }
            let value = event.value as f32;
            if !value.is_finite() {
                return None;
            }
            let ramp = if event.aux > 0 && target.rampable.binary_search(&event.id).is_ok() { event.aux } else { 0 };
            Some(Scheduled::Parameter { time, ramp, address: event.id, value })
        }
        EVENT_MIDI => {
            if !target.accepts_midi {
                return None;
            }
            let bytes = event.midi_bytes();
            let status = *bytes.first()?;
            let len = midi_message_length(status)?;
            if bytes.len() < len {
                return None;
            }
            let mut out = [0u8; 3];
            out[0] = status;
            for (slot, &byte) in out[1..len].iter_mut().zip(&bytes[1..len]) {
                *slot = byte & 0x7F;
            }
            let cable = if event.port < target.cables { event.port } else { 0 };
            Some(Scheduled::Midi { time, cable, len: len as u8, bytes: out })
        }
        _ => None,
    }
}

/// Splits a block into render slices of at most `max_slice` frames and hands each slice
/// the events that fall into it, in order.
///
/// ```text
/// let mut slices = Slices::new(frames, max_render);
/// while let Some(slice) = slices.next() {
///     for event in slices.take_events(events, slice) { schedule(event) }
///     render(slice.start, slice.frames)
/// }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Slices {
    frames: u32,
    max_slice: u32,
    position: u32,
    cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slice {
    /// First frame of the slice within the block.
    pub start: u32,
    pub frames: u32,
}

impl Slices {
    pub fn new(frames: u32, max_slice: u32) -> Slices {
        Slices { frames, max_slice: max_slice.max(1), position: 0, cursor: 0 }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Slice> {
        if self.position >= self.frames {
            return None;
        }
        let slice = Slice { start: self.position, frames: (self.frames - self.position).min(self.max_slice) };
        self.position += slice.frames;
        Some(slice)
    }

    /// The events to schedule before rendering `slice`: everything not yet handed out
    /// whose offset lies before the end of the slice (a list that is not sorted, against
    /// the contract, is delivered late rather than dropped), and for the last slice
    /// everything that is left.
    pub fn take_events<'e>(&mut self, events: &'e [Event], slice: Slice) -> &'e [Event] {
        let end = slice.start + slice.frames;
        let last = end >= self.frames;
        let begin = self.cursor.min(events.len());
        let mut cursor = begin;
        while cursor < events.len() && (last || events[cursor].offset < end) {
            cursor += 1;
        }
        self.cursor = cursor;
        &events[begin..cursor]
    }
}

/// Offset within the current block of a MIDI event the unit emitted with `time` (an
/// absolute sample time, or [`IMMEDIATE`] plus a small offset).
pub fn output_offset(time: i64, block_start: i64, frames: u32) -> u32 {
    let last = frames.saturating_sub(1) as i64;
    let relative = if (IMMEDIATE..IMMEDIATE + IMMEDIATE_OFFSET_LIMIT).contains(&time) {
        time - IMMEDIATE
    } else {
        time.saturating_sub(block_start)
    };
    relative.clamp(0, last) as u32
}

/// A MIDI message the unit emitted, as a protocol event; `None` for system exclusive and
/// anything that is not one complete message.
pub fn output_event(time: i64, cable: u8, bytes: &[u8], block_start: i64, frames: u32) -> Option<Event> {
    let status = *bytes.first()?;
    if midi_message_length(status)? != bytes.len() {
        return None;
    }
    let mut event = Event::midi(output_offset(time, block_start, frames), bytes);
    event.port = cable;
    Some(event)
}

/// Stable in-place sort by offset without allocating (the lists are short and almost
/// always sorted already).
pub fn sort_by_offset(events: &mut [Event]) {
    for i in 1..events.len() {
        let mut j = i;
        while j > 0 && events[j - 1].offset > events[j].offset {
            events.swap(j - 1, j);
            j -= 1;
        }
    }
}

// ---- host transport as an Audio Unit asks for it ------------------------------------------

// `AUHostTransportStateFlags`.
pub const AU_TRANSPORT_CHANGED: usize = 1;
pub const AU_TRANSPORT_MOVING: usize = 2;
pub const AU_TRANSPORT_RECORDING: usize = 4;
pub const AU_TRANSPORT_CYCLING: usize = 8;

/// The protocol's transport flags as `AUHostTransportStateFlags`. `changed`: the state
/// (playing or not, or the position, discontinuously) differs from the previous block.
pub fn au_transport_flags(flags: u32, changed: bool) -> usize {
    use stpc_runtime::{TRANSPORT_LOOPING, TRANSPORT_PLAYING, TRANSPORT_RECORDING};
    let mut out = 0;
    if changed {
        out |= AU_TRANSPORT_CHANGED;
    }
    if flags & TRANSPORT_PLAYING != 0 {
        out |= AU_TRANSPORT_MOVING;
    }
    if flags & TRANSPORT_RECORDING != 0 {
        out |= AU_TRANSPORT_RECORDING;
    }
    if flags & TRANSPORT_LOOPING != 0 {
        out |= AU_TRANSPORT_CYCLING;
    }
    out
}

/// `sampleOffsetToNextBeat` of the musical context: frames from a position (in quarter
/// notes) to the next whole beat; 0 on a beat and for nonsense input.
pub fn samples_to_next_beat(position: f64, tempo_bpm: f64, sample_rate: f64) -> i64 {
    if !position.is_finite()
        || !tempo_bpm.is_finite()
        || !sample_rate.is_finite()
        || tempo_bpm <= 0.0
        || sample_rate <= 0.0
    {
        return 0;
    }
    let beats = position.ceil() - position;
    let samples = (beats * 60.0 / tempo_bpm * sample_rate).round();
    if samples.is_finite() && samples >= 0.0 && samples < i64::MAX as f64 {
        samples as i64
    } else {
        0
    }
}

/// Tracks the transport from block to block to tell a unit when it "changed".
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportTracker {
    started: bool,
    playing: bool,
    expected_position: i64,
}

impl TransportTracker {
    /// Call once per block. True for the first block, after [`TransportTracker::reset`],
    /// on `discontinuity`, when playback starts or stops, and when the position is not
    /// where the previous block left it.
    pub fn advance(&mut self, playing: bool, sample_position: i64, frames: u32, discontinuity: bool) -> bool {
        let changed = !self.started
            || discontinuity
            || playing != self.playing
            || (playing && sample_position != self.expected_position);
        self.started = true;
        self.playing = playing;
        self.expected_position = if playing { sample_position.saturating_add(frames as i64) } else { sample_position };
        changed
    }

    pub fn reset(&mut self) {
        *self = TransportTracker::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_flags_and_beats() {
        use stpc_runtime::{TRANSPORT_LOOPING, TRANSPORT_PLAYING, TRANSPORT_RECORDING, TRANSPORT_TEMPO_VALID};
        assert_eq!(au_transport_flags(0, false), 0);
        assert_eq!(au_transport_flags(TRANSPORT_PLAYING | TRANSPORT_TEMPO_VALID, false), AU_TRANSPORT_MOVING);
        assert_eq!(
            au_transport_flags(TRANSPORT_PLAYING | TRANSPORT_RECORDING | TRANSPORT_LOOPING, true),
            AU_TRANSPORT_CHANGED | AU_TRANSPORT_MOVING | AU_TRANSPORT_RECORDING | AU_TRANSPORT_CYCLING
        );
        assert_eq!(au_transport_flags(0, true), AU_TRANSPORT_CHANGED);

        // 120 BPM at 48 kHz: a beat is 24000 frames.
        assert_eq!(samples_to_next_beat(0.0, 120.0, 48000.0), 0);
        assert_eq!(samples_to_next_beat(0.5, 120.0, 48000.0), 12000);
        assert_eq!(samples_to_next_beat(3.75, 120.0, 48000.0), 6000);
        assert_eq!(samples_to_next_beat(-0.25, 60.0, 44100.0), 11025);
        for (position, tempo, rate) in [
            (f64::NAN, 120.0, 48000.0),
            (1.5, 0.0, 48000.0),
            (1.5, -3.0, 48000.0),
            (1.5, 120.0, 0.0),
            (1.5, f64::INFINITY, 48000.0),
        ] {
            assert_eq!(samples_to_next_beat(position, tempo, rate), 0);
        }
        assert_eq!(samples_to_next_beat(0.5, 1.0e-300, 48000.0), 0);
    }

    #[test]
    fn immediate_times() {
        assert_eq!(immediate_time(1000, 1000), IMMEDIATE);
        assert_eq!(immediate_time(1017, 1000), IMMEDIATE + 17);
        // Late events land at the start, far ones at the last representable offset.
        assert_eq!(immediate_time(900, 1000), IMMEDIATE);
        assert_eq!(immediate_time(1000 + 10_000, 1000), IMMEDIATE + 4095);
        assert_eq!(immediate_time(i64::MAX, i64::MIN), IMMEDIATE + 4095);
        // Round trip with the output side.
        assert_eq!(output_offset(immediate_time(1017, 1000), 1000, 64), 17);
    }

    #[test]
    fn transport_changes() {
        let mut tracker = TransportTracker::default();
        // First block: always a change.
        assert!(tracker.advance(false, 0, 64, false));
        assert!(!tracker.advance(false, 0, 64, false));
        // Start.
        assert!(tracker.advance(true, 0, 64, false));
        assert!(!tracker.advance(true, 64, 64, false));
        assert!(!tracker.advance(true, 128, 100, false));
        // A jump (loop, seek).
        assert!(tracker.advance(true, 0, 64, false));
        assert!(!tracker.advance(true, 64, 64, false));
        // Dropped blocks.
        assert!(tracker.advance(true, 128, 64, true));
        // Stop; the position while stopped may move without a "change" per block.
        assert!(tracker.advance(false, 192, 64, false));
        assert!(!tracker.advance(false, 500, 64, false));
        tracker.reset();
        assert!(tracker.advance(false, 500, 64, false));
    }

    const MIDI: Target<'static> = Target { accepts_midi: true, cables: 1, rampable: &[] };

    #[test]
    fn message_lengths() {
        assert_eq!(midi_message_length(0x90), Some(3));
        assert_eq!(midi_message_length(0x8F), Some(3));
        assert_eq!(midi_message_length(0xB3), Some(3));
        assert_eq!(midi_message_length(0xE0), Some(3));
        assert_eq!(midi_message_length(0xC5), Some(2));
        assert_eq!(midi_message_length(0xD0), Some(2));
        assert_eq!(midi_message_length(0xF1), Some(2));
        assert_eq!(midi_message_length(0xF2), Some(3));
        assert_eq!(midi_message_length(0xF3), Some(2));
        assert_eq!(midi_message_length(0xF6), Some(1));
        assert_eq!(midi_message_length(0xF8), Some(1));
        assert_eq!(midi_message_length(0xFF), Some(1));
        assert_eq!(midi_message_length(0xF0), None);
        assert_eq!(midi_message_length(0xF7), None);
        assert_eq!(midi_message_length(0x45), None);
        for status in 0..=255u8 {
            assert!(midi_message_length(status).is_none_or(|len| (1..=3).contains(&len)));
        }
    }

    #[test]
    fn notes_and_offsets_become_absolute_times() {
        let event = Event::note_on(17, 2, 60, 100);
        assert_eq!(
            translate(&event, 48_000, 512, &MIDI),
            Some(Scheduled::Midi { time: 48_017, cable: 0, len: 3, bytes: [0x92, 60, 100] })
        );
        // Far beyond the range of "immediate plus offset".
        let event = Event::note_off(8191, 0, 60);
        assert_eq!(translate(&event, 1 << 40, 8192, &MIDI).unwrap().time(), (1 << 40) + 8191);
        // An offset outside the block is pulled to its last frame.
        let event = Event::note_on(9999, 0, 60, 1);
        assert_eq!(translate(&event, 100, 64, &MIDI).unwrap().time(), 163);
        assert_eq!(translate(&event, 100, 0, &MIDI).unwrap().time(), 100);
    }

    #[test]
    fn midi_is_validated() {
        // Program change: two bytes, even if the event carries a third.
        let mut event = Event::midi(0, &[0xC1, 5, 99]);
        assert_eq!(
            translate(&event, 0, 64, &MIDI),
            Some(Scheduled::Midi { time: 0, cable: 0, len: 2, bytes: [0xC1, 5, 0] })
        );
        // Real-time byte.
        event = Event::midi(3, &[0xF8]);
        assert_eq!(
            translate(&event, 0, 64, &MIDI),
            Some(Scheduled::Midi { time: 3, cable: 0, len: 1, bytes: [0xF8, 0, 0] })
        );
        // Controllers and pressure go to the unit as the MIDI they are, at their frame.
        for (bytes, len) in [(&[0xB2, 74, 99][..], 3), (&[0xA0, 60, 45][..], 3), (&[0xE5, 0, 96][..], 3), (&[0xD3, 127][..], 2), (&[0xD3, 127, 0][..], 2)] {
            let mut expected = [0u8; 3];
            expected[..len].copy_from_slice(&bytes[..len]);
            assert_eq!(translate(&Event::midi(9, bytes), 100, 64, &MIDI), Some(Scheduled::Midi { time: 109, cable: 0, len: len as u8, bytes: expected }));
        }
        // Truncated, data byte as status, sysex, empty.
        assert_eq!(translate(&Event::midi(0, &[0x90, 60]), 0, 64, &MIDI), None);
        assert_eq!(translate(&Event::midi(0, &[0x40, 1, 2]), 0, 64, &MIDI), None);
        assert_eq!(translate(&Event::midi(0, &[0xF0, 1, 2]), 0, 64, &MIDI), None);
        assert_eq!(translate(&Event::midi(0, &[]), 0, 64, &MIDI), None);
        // Data bytes with the top bit set are masked, not passed on.
        let event = Event::midi(0, &[0x90, 0xC0, 0xFF]);
        assert_eq!(
            translate(&event, 0, 64, &MIDI),
            Some(Scheduled::Midi { time: 0, cable: 0, len: 3, bytes: [0x90, 0x40, 0x7F] })
        );
        // A unit without MIDI input gets none.
        let effect = Target { accepts_midi: false, cables: 0, rampable: &[] };
        assert_eq!(translate(&Event::note_on(0, 0, 60, 100), 0, 64, &effect), None);
        // Unknown kinds are skipped.
        let unknown = Event { kind: 99, ..Event::default() };
        assert_eq!(translate(&unknown, 0, 64, &MIDI), None);
    }

    #[test]
    fn cables() {
        let two = Target { accepts_midi: true, cables: 2, rampable: &[] };
        let mut event = Event::note_on(0, 0, 60, 100);
        event.port = 1;
        assert!(matches!(translate(&event, 0, 64, &two), Some(Scheduled::Midi { cable: 1, .. })));
        assert!(matches!(translate(&event, 0, 64, &MIDI), Some(Scheduled::Midi { cable: 0, .. })));
        event.port = 200;
        assert!(matches!(translate(&event, 0, 64, &two), Some(Scheduled::Midi { cable: 0, .. })));
    }

    #[test]
    fn parameters_and_ramps() {
        let target = Target { accepts_midi: false, cables: 0, rampable: &[3, 10, 1 << 40] };
        let event = Event::param(5, 10, 0.25);
        assert_eq!(
            translate(&event, 1000, 64, &target),
            Some(Scheduled::Parameter { time: 1005, ramp: 0, address: 10, value: 0.25 })
        );
        // A ramp is passed on for a rampable parameter ...
        let mut ramped = Event::param(0, 1 << 40, -6.0);
        ramped.aux = 480;
        assert_eq!(
            translate(&ramped, 0, 64, &target),
            Some(Scheduled::Parameter { time: 0, ramp: 480, address: 1 << 40, value: -6.0 })
        );
        // ... and becomes a jump at the event's offset for any other.
        ramped.id = 11;
        assert_eq!(
            translate(&ramped, 0, 64, &target),
            Some(Scheduled::Parameter { time: 0, ramp: 0, address: 11, value: -6.0 })
        );
        // Values no unit should ever see.
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1.0e300] {
            assert_eq!(translate(&Event::param(0, 10, bad), 0, 64, &target), None);
        }
    }

    fn offsets(events: &[Event]) -> Vec<u32> {
        events.iter().map(|e| e.offset).collect()
    }

    #[test]
    fn a_block_is_one_slice_when_it_fits() {
        let events = [Event::param(0, 1, 0.0), Event::param(63, 1, 1.0)];
        let mut slices = Slices::new(64, 512);
        let slice = slices.next().unwrap();
        assert_eq!(slice, Slice { start: 0, frames: 64 });
        assert_eq!(offsets(slices.take_events(&events, slice)), [0, 63]);
        assert_eq!(slices.next(), None);
    }

    #[test]
    fn slices_cover_the_block_and_every_event_exactly_once() {
        let events: Vec<Event> =
            [0u32, 0, 1, 127, 128, 128, 300, 511, 512, 999, 1000].iter().map(|&o| Event::param(o, 1, 0.0)).collect();
        for max in [1u32, 7, 128, 256, 1000, 1001, 5000] {
            let mut slices = Slices::new(1001, max);
            let mut covered = 0;
            let mut seen = Vec::new();
            while let Some(slice) = slices.next() {
                assert_eq!(slice.start, covered);
                assert!(slice.frames >= 1 && slice.frames <= max);
                for event in slices.take_events(&events, slice) {
                    // Each event is scheduled before the slice that contains it.
                    assert!(event.offset >= slice.start && event.offset < slice.start + slice.frames);
                    seen.push(event.offset);
                }
                covered += slice.frames;
            }
            assert_eq!(covered, 1001);
            assert_eq!(seen, offsets(&events), "max {max}");
        }
        // Scheduled times across slices equal block start plus offset, in order.
        let mut slices = Slices::new(1001, 128);
        let mut times = Vec::new();
        while let Some(slice) = slices.next() {
            for event in slices.take_events(&events, slice) {
                times.push(translate(event, 10_000, 1001, &MIDI).unwrap().time());
            }
        }
        assert_eq!(times, [10_000, 10_000, 10_001, 10_127, 10_128, 10_128, 10_300, 10_511, 10_512, 10_999, 11_000]);
    }

    #[test]
    fn hostile_event_lists_lose_nothing() {
        // Unsorted, and with offsets beyond the block.
        let events: Vec<Event> = [100u32, 5, 70_000, 20].iter().map(|&o| Event::param(o, 1, 0.0)).collect();
        let mut slices = Slices::new(256, 64);
        let mut seen = Vec::new();
        while let Some(slice) = slices.next() {
            seen.extend(offsets(slices.take_events(&events, slice)));
        }
        assert_eq!(seen, [100, 5, 70_000, 20]);
        // No frames: no slices, nothing scheduled.
        let mut slices = Slices::new(0, 64);
        assert_eq!(slices.next(), None);
        // A zero slice size cannot loop forever.
        let mut slices = Slices::new(3, 0);
        assert_eq!(slices.next(), Some(Slice { start: 0, frames: 1 }));
    }

    #[test]
    fn output_events() {
        assert_eq!(output_offset(1000, 1000, 64), 0);
        assert_eq!(output_offset(1063, 1000, 64), 63);
        assert_eq!(output_offset(2000, 1000, 64), 63);
        assert_eq!(output_offset(10, 1000, 64), 0);
        assert_eq!(output_offset(IMMEDIATE, 1000, 64), 0);
        assert_eq!(output_offset(IMMEDIATE + 5, 1000, 64), 5);
        assert_eq!(output_offset(IMMEDIATE + 500, 1000, 64), 63);
        assert_eq!(output_offset(i64::MIN, 1000, 64), 0);
        assert_eq!(output_offset(i64::MAX, -5, 64), 63);
        assert_eq!(output_offset(5, 0, 0), 0);

        let event = output_event(1010, 2, &[0x90, 60, 100], 1000, 64).unwrap();
        assert_eq!((event.offset, event.port, event.kind), (10, 2, EVENT_MIDI));
        assert_eq!(event.midi_bytes(), [0x90, 60, 100]);
        assert_eq!(output_event(1010, 0, &[0xF8], 1000, 64).unwrap().midi_bytes(), [0xF8]);
        // Sysex, running status, several messages in one packet, nothing.
        assert!(output_event(0, 0, &[0xF0, 1, 2, 3, 0xF7], 0, 64).is_none());
        assert!(output_event(0, 0, &[60, 100], 0, 64).is_none());
        assert!(output_event(0, 0, &[0x90, 60, 100, 0x80, 60, 0], 0, 64).is_none());
        assert!(output_event(0, 0, &[0x90, 60], 0, 64).is_none());
        assert!(output_event(0, 0, &[], 0, 64).is_none());
    }

    #[test]
    fn sorting_is_stable() {
        let mut events = vec![
            Event::midi(5, &[0x90, 1, 1]),
            Event::midi(0, &[0x90, 2, 1]),
            Event::midi(5, &[0x90, 3, 1]),
            Event::midi(0, &[0x90, 4, 1]),
            Event::midi(3, &[0x90, 5, 1]),
        ];
        sort_by_offset(&mut events);
        let order: Vec<(u32, u8)> = events.iter().map(|e| (e.offset, e.data[1])).collect();
        assert_eq!(order, [(0, 2), (0, 4), (3, 5), (5, 1), (5, 3)]);
        sort_by_offset(&mut []);
    }
}
