//! Pedals played live: which instruments each sustain, sostenuto or soft pedal has reached, and
//! which notes each sustain pedal keeps sounding after their keys were released. Pure control
//! state: no MIDI or audio I/O. The owner (`st-host`) feeds it every live note and pedal message,
//! from MIDI devices and from the application's own keys, and carries out what it answers.
//!
//! # Who holds a note
//!
//! - An instrument with a pedal of its own (a plug-in instrument, an external MIDI instrument)
//!   **gets the pedal**, with its value, so that half-pedalling, re-pedalling and pedal resonance
//!   are the instrument's. Its note-offs are passed on as played; it sustains by itself.
//! - The engine's own instruments have no pedal, and neither has a plug-in that controller 64
//!   cannot reach. For them the sustain pedal is the host's: while it is down (64 or more), the
//!   note-off of a released key is held back until the pedal comes up. Sostenuto and soft mean
//!   nothing to them.
//!
//! # What a pedal acts on
//!
//! A pedal belongs to its source and channel, like a piano's belongs to its keyboard. It reaches
//! the instrument it is played to, the instruments that keys of the same source and channel are
//! holding at that moment, and every instrument those keys go on to play while it is down. So
//! when the keyboard is pointed at another track while the pedal is down, the first instrument
//! keeps sustaining, the second gets the pedal with the first note played on it, and both get
//! pedal-up when the pedal is lifted.
//!
//! Several pedals can reach one instrument (two keyboards, a keyboard and the application's
//! pedal). The instrument is sent the highest of their values, so one pedal coming up does not
//! let go of what another still holds.
//!
//! An instrument can be replaced while a pedal is down (another plug-in, a plug-in restored
//! after a crash, another device or channel). The owner passes a *generation* with every note:
//! a number that changes when what stands at that place may have lost its pedal. A note played
//! to a new generation brings the pedal along again. An instrument can also leave its place (its
//! track is moved or removed): the owner says so while it is still there
//! ([`SustainState::detach`]), and it is told pedal-up.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The sustain (damper) pedal's controller.
pub const SUSTAIN: u8 = 64;
/// The sostenuto pedal's controller.
pub const SOSTENUTO: u8 = 66;
/// The soft pedal's controller.
pub const SOFT: u8 = 67;
/// The controllers handled as pedals: whoever was told one is down must be told when it is up.
pub const PEDALS: [u8; 3] = [SUSTAIN, SOSTENUTO, SOFT];

/// The source of the notes and pedals the application itself plays (computer keyboard, on-screen
/// keys, a controller the interface reads). Device ids of MIDI inputs are the other sources.
pub const APP_SOURCE: i32 = -2;

/// What a note is played on, as far as pedals are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PedalTarget {
    /// An instrument without a pedal on a track (engine channel): one of the engine's own, or
    /// a plug-in the pedal cannot reach. The host holds back the releases of its notes.
    Engine {
        /// Engine channel.
        track: usize,
    },
    /// The plug-in instrument of a track. It gets the pedals.
    Plugin {
        /// Engine channel.
        track: usize,
    },
    /// An external MIDI instrument. Its device gets the pedals on its channel.
    External {
        /// Instrument, as the engine numbers them.
        instrument: usize,
    },
}

impl PedalTarget {
    /// Whether the instrument has a pedal of its own (it is sent the pedal controllers).
    pub fn has_pedal(&self) -> bool {
        !matches!(self, PedalTarget::Engine { .. })
    }
}

/// One thing to do after a note or pedal message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PedalEffect {
    /// Send `controller` with `value` to an instrument with a pedal of its own.
    Send {
        /// The instrument.
        target: PedalTarget,
        /// 64, 66 or 67.
        controller: u8,
        /// 0 to 127.
        value: u8,
    },
    /// A note whose key was released under the sustain pedal ends now. For
    /// [`PedalTarget::Engine`] the note-off that was held back must be sent. An instrument with
    /// its own pedal lets go by itself (it got the pedal-up); this only tells that the note is
    /// over (note recording gives the note its length here).
    NoteEnds {
        /// The source whose key played the note.
        source: i32,
        /// The channel of that key, 1 to 16.
        channel: u8,
        /// MIDI note number.
        note: u8,
        /// Where it sounded.
        target: PedalTarget,
    },
}

#[derive(Debug, Clone, Default)]
struct Pedal {
    /// 1 to 127: a pedal at 0 is forgotten.
    value: u8,
    /// The instruments with a pedal of their own this pedal has reached.
    targets: BTreeSet<PedalTarget>,
    /// Sustain only: notes whose keys were released while the pedal was down.
    held: BTreeSet<(PedalTarget, u8)>,
}

/// A key that is down.
#[derive(Debug, Clone, Copy)]
struct Key {
    note: u8,
    /// Engine channel the note was started on.
    track: usize,
    target: PedalTarget,
    /// The target's generation when the key went down.
    generation: u64,
}

/// What an instrument with its own pedal was last sent.
#[derive(Debug, Clone, Copy)]
struct Sent {
    value: u8,
    generation: u64,
}

/// The pedals that are down and what they hold. See the module documentation.
#[derive(Debug, Clone, Default)]
pub struct SustainState {
    /// `(source, channel 1 to 16, controller)`.
    pedals: BTreeMap<(i32, u8, u8), Pedal>,
    /// Keys that are down, per source and channel.
    keys: HashMap<(i32, u8), Vec<Key>>,
    /// The last value sent to each instrument, per controller. Absent: 0.
    sent: BTreeMap<(PedalTarget, u8), Sent>,
}

fn valid(channel: u8, note: u8) -> bool {
    (1..=16).contains(&channel) && note < 128
}

impl SustainState {
    /// No pedal is down.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of a pedal (0: up).
    pub fn value(&self, source: i32, channel: u8, controller: u8) -> u8 {
        self.pedals.get(&(source, channel, controller)).map_or(0, |pedal| pedal.value)
    }

    /// Whether no pedal is down: there is nothing to keep track of.
    pub fn is_idle(&self) -> bool {
        self.pedals.is_empty()
    }

    /// Whether the sustain pedal of `source` on `channel` is down (64 or more): it holds the
    /// notes whose keys are released now.
    pub fn is_down(&self, source: i32, channel: u8) -> bool {
        self.value(source, channel, SUSTAIN) >= 64
    }

    /// What `target` was last sent for `controller` (0 when it never got it, or got it up).
    pub fn sent(&self, target: PedalTarget, controller: u8) -> u8 {
        self.sent.get(&(target, controller)).map_or(0, |sent| sent.value)
    }

    /// Brings what `target` was sent in line with the pedals that reach it. `generation` is
    /// given when the caller knows it (a note or pedal is being played to the target now).
    fn refresh(&mut self, target: PedalTarget, controller: u8, generation: Option<u64>, effects: &mut Vec<PedalEffect>) {
        if !target.has_pedal() {
            return;
        }
        let wanted = self.pedals.iter().filter(|((_, _, known), pedal)| *known == controller && pedal.targets.contains(&target)).map(|(_, pedal)| pedal.value).max().unwrap_or(0);
        let known = self.sent.get(&(target, controller)).copied();
        // What stands there now may be another instrument than the one that was sent the pedal.
        let has = known.filter(|sent| generation.is_none_or(|generation| generation == sent.generation)).map_or(0, |sent| sent.value);
        let generation = generation.or(known.map(|sent| sent.generation)).unwrap_or(0);
        if wanted == 0 {
            self.sent.remove(&(target, controller));
        } else {
            self.sent.insert((target, controller), Sent { value: wanted, generation });
        }
        if wanted != has {
            effects.push(PedalEffect::Send { target, controller, value: wanted });
        }
    }

    /// A pedal of `source` on `channel` (1 to 16) moved to `value`. `played` is the instrument
    /// the pedal is played to and its generation: where a note of this source and channel would
    /// go now, or `None` when its input is refused there (a pedal that comes up still lets go of
    /// everything it holds). Controllers other than [`PEDALS`] are ignored.
    pub fn pedal(&mut self, source: i32, channel: u8, controller: u8, value: u8, played: Option<(PedalTarget, u64)>) -> Vec<PedalEffect> {
        let mut effects = Vec::new();
        if !valid(channel, 0) || !PEDALS.contains(&controller) {
            return effects;
        }
        let key = (source, channel, controller);
        let value = value.min(127);
        if value == 0 {
            if let Some(pedal) = self.pedals.remove(&key) {
                effects.extend(pedal.held.iter().map(|(target, note)| PedalEffect::NoteEnds { source, channel, note: *note, target: *target }));
                for target in pedal.targets {
                    self.refresh(target, controller, None, &mut effects);
                }
            }
            return effects;
        }
        // The instruments the pedal reaches now: the one it is played to, and those the keys
        // of this source and channel hold.
        let mut reached: Vec<(PedalTarget, u64)> = self.keys.get(&(source, channel)).map(|keys| keys.iter().map(|key| (key.target, key.generation)).collect()).unwrap_or_default();
        reached.extend(played);
        if reached.is_empty() && !self.pedals.contains_key(&key) {
            return effects;
        }
        let pedal = self.pedals.entry(key).or_default();
        pedal.value = value;
        pedal.targets.extend(reached.iter().map(|(target, _)| *target).filter(PedalTarget::has_pedal));
        if controller == SUSTAIN && value < 64 {
            // Half way up is up for the notes the host holds, and for where a note's length ends.
            effects.extend(std::mem::take(&mut pedal.held).into_iter().map(|(target, note)| PedalEffect::NoteEnds { source, channel, note, target }));
        }
        let targets: Vec<PedalTarget> = pedal.targets.iter().copied().collect();
        for target in targets {
            let generation = reached.iter().rev().find(|(known, _)| *known == target).map(|(_, generation)| *generation);
            self.refresh(target, controller, generation, &mut effects);
        }
        effects
    }

    /// A key of `source` on `channel` went down and plays `note` on engine channel `track`, on
    /// `target`, whose generation is `generation`. Carry out the answer before the note is
    /// started: an instrument with its own pedal that this source's pedals had not reached gets
    /// them first. The note-off the sustain pedal was holding for the same note there is
    /// dropped: the note sounds again.
    pub fn note_on(&mut self, source: i32, channel: u8, note: u8, track: usize, target: PedalTarget, generation: u64) -> Vec<PedalEffect> {
        let mut effects = Vec::new();
        if !valid(channel, note) {
            return effects;
        }
        let keys = self.keys.entry((source, channel)).or_default();
        keys.retain(|key| (key.note, key.track) != (note, track));
        keys.push(Key { note, track, target, generation });
        for controller in PEDALS {
            let Some(pedal) = self.pedals.get_mut(&(source, channel, controller)) else { continue };
            if controller == SUSTAIN {
                pedal.held.remove(&(target, note));
            }
            if target.has_pedal() {
                pedal.targets.insert(target);
                self.refresh(target, controller, Some(generation), &mut effects);
            }
        }
        effects
    }

    /// The key of `note` on engine channel `track` was released. Returns what the note sounds
    /// on (what its key-down said; `otherwise` for a key this state did not see go down), and
    /// whether the sustain pedal of this source and channel is down: the note then goes on
    /// sounding and ends with [`PedalEffect::NoteEnds`]. For [`PedalTarget::Engine`] the caller
    /// must then not send the note-off; an instrument with its own pedal gets the note-off all
    /// the same.
    pub fn note_off(&mut self, source: i32, channel: u8, note: u8, track: usize, otherwise: PedalTarget) -> (PedalTarget, bool) {
        if !valid(channel, note) {
            return (otherwise, false);
        }
        let mut target = otherwise;
        if let Some(keys) = self.keys.get_mut(&(source, channel)) {
            if let Some(index) = keys.iter().position(|key| (key.note, key.track) == (note, track)) {
                target = keys.swap_remove(index).target;
            }
            if keys.is_empty() {
                self.keys.remove(&(source, channel));
            }
        }
        match self.pedals.get_mut(&(source, channel, SUSTAIN)) {
            Some(pedal) if pedal.value >= 64 => {
                pedal.held.insert((target, note));
                (target, true)
            }
            _ => (target, false),
        }
    }

    /// The pedals of every source `lifted` names come up (their input route was taken down: no
    /// pedal-up will arrive through it).
    pub fn lift(&mut self, lifted: impl Fn(i32) -> bool) -> Vec<PedalEffect> {
        let keys: Vec<(i32, u8, u8)> = self.pedals.keys().filter(|(source, _, _)| lifted(*source)).copied().collect();
        keys.into_iter().flat_map(|(source, channel, controller)| self.pedal(source, channel, controller, 0, None)).collect()
    }

    /// The instruments `gone` names are about to leave their places (their tracks are moved or
    /// removed). Carry out the answer while they are still there: they are told pedal-up, and
    /// the notes a sustain pedal held on them end. The pedals forget them; what is played at
    /// those places afterwards gets a pedal that is still down like any new instrument.
    pub fn detach(&mut self, gone: impl Fn(PedalTarget) -> bool) -> Vec<PedalEffect> {
        let mut effects = Vec::new();
        for ((source, channel, _), pedal) in &mut self.pedals {
            let (ended, kept): (BTreeSet<_>, BTreeSet<_>) = std::mem::take(&mut pedal.held).into_iter().partition(|(target, _)| gone(*target));
            pedal.held = kept;
            effects.extend(ended.into_iter().map(|(target, note)| PedalEffect::NoteEnds { source: *source, channel: *channel, note, target }));
            pedal.targets.retain(|target| !gone(*target));
        }
        for keys in self.keys.values_mut() {
            keys.retain(|key| !gone(key.target));
        }
        let told: Vec<(PedalTarget, u8)> = self.sent.keys().filter(|(target, _)| gone(*target)).copied().collect();
        for (target, controller) in told {
            self.sent.remove(&(target, controller));
            effects.push(PedalEffect::Send { target, controller, value: 0 });
        }
        effects
    }

    /// Everything is being silenced, but the pedals stay where the player has them (the
    /// transport stopped): every instrument that was sent a pedal is sent pedal-up, and every
    /// note a sustain pedal held ends. A pedal that is still down reaches an instrument again
    /// with the next note played to it.
    pub fn silence(&mut self) -> Vec<PedalEffect> {
        let mut effects = Vec::new();
        for ((source, channel, _), pedal) in &mut self.pedals {
            effects.extend(std::mem::take(&mut pedal.held).into_iter().map(|(target, note)| PedalEffect::NoteEnds { source: *source, channel: *channel, note, target }));
            pedal.targets.clear();
        }
        effects.extend(std::mem::take(&mut self.sent).into_keys().map(|(target, controller)| PedalEffect::Send { target, controller, value: 0 }));
        effects
    }

    /// Forgets every pedal and every held note without releasing anything (a panic silenced the
    /// engine, the plug-ins and the devices, and told them pedal-up). A pedal still held down
    /// counts as up until it is pressed again.
    pub fn clear(&mut self) {
        self.pedals.clear();
        self.keys.clear();
        self.sent.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINE: PedalTarget = PedalTarget::Engine { track: 0 };
    const PLUGIN: PedalTarget = PedalTarget::Plugin { track: 1 };
    const SYNTH: PedalTarget = PedalTarget::External { instrument: 2 };

    fn send(target: PedalTarget, controller: u8, value: u8) -> PedalEffect {
        PedalEffect::Send { target, controller, value }
    }

    fn ends(source: i32, channel: u8, note: u8, target: PedalTarget) -> PedalEffect {
        PedalEffect::NoteEnds { source, channel, note, target }
    }

    /// The engine channel a test note for `target` is played on.
    fn track_of(target: PedalTarget) -> usize {
        match target {
            PedalTarget::Engine { track } | PedalTarget::Plugin { track } => track,
            PedalTarget::External { instrument } => 10 + instrument,
        }
    }

    fn on(state: &mut SustainState, source: i32, channel: u8, note: u8, target: PedalTarget, generation: u64) -> Vec<PedalEffect> {
        state.note_on(source, channel, note, track_of(target), target, generation)
    }

    /// Releases the key; true when the pedal holds the note.
    fn off(state: &mut SustainState, source: i32, channel: u8, note: u8, target: PedalTarget) -> bool {
        state.note_off(source, channel, note, track_of(target), target).1
    }

    #[test]
    fn an_instrument_with_its_own_pedal_gets_the_pedal_and_its_note_offs() {
        let mut state = SustainState::new();
        assert!(on(&mut state, 10, 1, 60, PLUGIN, 0).is_empty(), "no pedal is down");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0))), [send(PLUGIN, 64, 127)]);
        assert!(off(&mut state, 10, 1, 60, PLUGIN), "the instrument sustains: the note is not over");
        // Half pedal, with its value; then up.
        assert_eq!(state.pedal(10, 1, SUSTAIN, 80, Some((PLUGIN, 0))), [send(PLUGIN, 64, 80)]);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 80, Some((PLUGIN, 0))), [], "a repeated value is not sent again");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, Some((PLUGIN, 0))), [ends(10, 1, 60, PLUGIN), send(PLUGIN, 64, 0)]);
        assert_eq!((state.value(10, 1, SUSTAIN), state.sent(PLUGIN, SUSTAIN)), (0, 0));
        assert!(!off(&mut state, 10, 1, 60, PLUGIN), "pedal up: a key release ends the note");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, Some((PLUGIN, 0))), [], "a repeated pedal-up is inert");
    }

    #[test]
    fn half_pedal_values_below_the_middle_reach_the_instrument_but_hold_nothing_for_the_host() {
        let mut state = SustainState::new();
        assert_eq!(state.pedal(10, 1, SUSTAIN, 40, Some((SYNTH, 0))), [send(SYNTH, 64, 40)]);
        assert!(!state.is_down(10, 1) && !off(&mut state, 10, 1, 60, SYNTH));
        assert_eq!(state.pedal(10, 1, SUSTAIN, 100, Some((SYNTH, 0))), [send(SYNTH, 64, 100)]);
        on(&mut state, 10, 1, 62, SYNTH, 0);
        on(&mut state, 10, 1, 50, ENGINE, 0);
        assert!(off(&mut state, 10, 1, 62, SYNTH) && off(&mut state, 10, 1, 50, ENGINE));
        // Down to 30: the instrument hears it; for held notes that is up.
        assert_eq!(state.pedal(10, 1, SUSTAIN, 30, Some((SYNTH, 0))), [ends(10, 1, 50, ENGINE), ends(10, 1, 62, SYNTH), send(SYNTH, 64, 30)]);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [send(SYNTH, 64, 0)]);
    }

    #[test]
    fn the_host_holds_the_notes_of_the_engines_own_instruments() {
        let mut state = SustainState::new();
        assert_eq!(state.pedal(10, 1, SUSTAIN, 64, Some((ENGINE, 0))), [], "nothing to send: the pedal is the host's");
        assert!(state.is_down(10, 1));
        on(&mut state, 10, 1, 60, ENGINE, 0);
        on(&mut state, 10, 1, 64, ENGINE, 0);
        assert!(off(&mut state, 10, 1, 60, ENGINE), "the release is held back");
        // Struck again under the pedal: its own held release is dropped, the other stays.
        assert!(off(&mut state, 10, 1, 64, ENGINE));
        assert!(on(&mut state, 10, 1, 60, ENGINE, 0).is_empty());
        assert_eq!(state.pedal(10, 1, SUSTAIN, 63, Some((ENGINE, 0))), [ends(10, 1, 64, ENGINE)]);
        assert!(!off(&mut state, 10, 1, 60, ENGINE), "the key struck again releases its note itself");
        // Sostenuto and soft mean nothing to the engine's instruments.
        assert_eq!(state.pedal(10, 1, SOSTENUTO, 127, Some((ENGINE, 0))), []);
        assert_eq!(state.pedal(10, 1, SOFT, 127, Some((ENGINE, 0))), []);
        assert_eq!(state.pedal(10, 1, SOSTENUTO, 0, None), []);
    }

    #[test]
    fn a_pedal_follows_its_keys_to_another_instrument_and_lets_go_of_both() {
        let mut state = SustainState::new();
        on(&mut state, 10, 1, 60, PLUGIN, 0);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0))), [send(PLUGIN, 64, 127)]);
        assert!(off(&mut state, 10, 1, 60, PLUGIN));
        // The keyboard now plays the synth: it gets the pedal with its first note.
        assert_eq!(on(&mut state, 10, 1, 72, SYNTH, 0), [send(SYNTH, 64, 127)]);
        assert_eq!(on(&mut state, 10, 1, 74, SYNTH, 0), [], "once");
        assert!(off(&mut state, 10, 1, 72, SYNTH));
        // And an instrument of the engine: its releases are the host's to hold.
        assert_eq!(on(&mut state, 10, 1, 48, ENGINE, 0), []);
        assert!(off(&mut state, 10, 1, 48, ENGINE));
        // Pedal up, played to the engine's instrument: everything it reached is let go.
        assert_eq!(
            state.pedal(10, 1, SUSTAIN, 0, Some((ENGINE, 0))),
            [ends(10, 1, 48, ENGINE), ends(10, 1, 60, PLUGIN), ends(10, 1, 72, SYNTH), send(PLUGIN, 64, 0), send(SYNTH, 64, 0)]
        );
        assert!(!off(&mut state, 10, 1, 74, SYNTH));
    }

    #[test]
    fn a_key_is_released_where_it_went_down() {
        let mut state = SustainState::new();
        state.pedal(10, 1, SUSTAIN, 127, Some((ENGINE, 0)));
        // The same pitch on two tracks, one played by the engine and one by a plug-in.
        state.note_on(10, 1, 60, 0, ENGINE, 0);
        state.note_on(10, 1, 60, 1, PLUGIN, 0);
        // The caller no longer knows what track 0 plays (it guesses the plug-in): the key does.
        assert_eq!(state.note_off(10, 1, 60, 0, PedalTarget::Plugin { track: 0 }), (ENGINE, true));
        assert_eq!(state.note_off(10, 1, 60, 1, ENGINE), (PLUGIN, true));
        assert_eq!(state.note_off(10, 1, 61, 1, SYNTH), (SYNTH, true), "a key that was never seen going down is taken at the caller's word");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [ends(10, 1, 60, ENGINE), ends(10, 1, 60, PLUGIN), ends(10, 1, 61, SYNTH), send(PLUGIN, 64, 0)]);
    }

    #[test]
    fn a_pedal_reaches_the_instruments_its_keys_are_holding() {
        let mut state = SustainState::new();
        // Keys held on the plug-in; the keyboard is pointed at the synth; then the pedal.
        on(&mut state, 10, 1, 60, PLUGIN, 3);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 127, Some((SYNTH, 0))), [send(PLUGIN, 64, 127), send(SYNTH, 64, 127)]);
        assert!(off(&mut state, 10, 1, 60, PLUGIN), "the chord still held on the first instrument is sustained");
        // A pedal that is refused where the keyboard points now is no pedal, unless keys hold something.
        assert_eq!(state.pedal(20, 1, SUSTAIN, 127, None), []);
        assert!(!state.is_down(20, 1));
        on(&mut state, 20, 1, 40, SYNTH, 0);
        assert_eq!(state.pedal(20, 1, SUSTAIN, 127, None), [], "the synth has the pedal already");
        assert!(state.is_down(20, 1));
    }

    #[test]
    fn sources_and_channels_have_their_own_pedals_and_an_instrument_hears_the_highest() {
        let mut state = SustainState::new();
        assert_eq!(state.pedal(10, 1, SUSTAIN, 100, Some((PLUGIN, 0))), [send(PLUGIN, 64, 100)]);
        // Another keyboard, and another channel of the first: their own pedals, the same instrument.
        assert_eq!(state.pedal(20, 1, SUSTAIN, 127, Some((PLUGIN, 0))), [send(PLUGIN, 64, 127)]);
        assert_eq!(state.pedal(10, 2, SUSTAIN, 90, Some((PLUGIN, 0))), [], "lower than what the instrument has");
        on(&mut state, 10, 1, 60, PLUGIN, 0);
        on(&mut state, 20, 1, 64, PLUGIN, 0);
        assert!(off(&mut state, 10, 1, 60, PLUGIN) && off(&mut state, 20, 1, 64, PLUGIN));
        assert!(!off(&mut state, 30, 1, 60, PLUGIN) && !off(&mut state, 10, 3, 60, PLUGIN), "a pedal holds for its own source and channel only");
        // One pedal up: its notes are over for the record, the instrument keeps the others' pedal.
        assert_eq!(state.pedal(20, 1, SUSTAIN, 0, None), [ends(20, 1, 64, PLUGIN), send(PLUGIN, 64, 100)]);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [ends(10, 1, 60, PLUGIN), send(PLUGIN, 64, 90)]);
        assert_eq!(state.pedal(10, 2, SUSTAIN, 0, None), [send(PLUGIN, 64, 0)]);
        // The engine's instruments: one keyboard's pedal does not hold another's notes.
        state.pedal(10, 1, SUSTAIN, 127, Some((ENGINE, 0)));
        assert!(!off(&mut state, 20, 1, 60, ENGINE) && off(&mut state, 10, 1, 60, ENGINE));
        assert_eq!(state.pedal(20, 1, SUSTAIN, 0, None), [], "another pedal's up releases nothing");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [ends(10, 1, 60, ENGINE)]);
    }

    #[test]
    fn sostenuto_and_soft_are_followed_like_the_sustain_pedal() {
        let mut state = SustainState::new();
        assert_eq!(state.pedal(10, 1, SOSTENUTO, 127, Some((PLUGIN, 0))), [send(PLUGIN, 66, 127)]);
        assert_eq!(state.pedal(10, 1, SOFT, 127, Some((PLUGIN, 0))), [send(PLUGIN, 67, 127)]);
        assert!(!off(&mut state, 10, 1, 60, PLUGIN), "only the sustain pedal extends a note's length");
        assert_eq!(on(&mut state, 10, 1, 60, SYNTH, 0), [send(SYNTH, 66, 127), send(SYNTH, 67, 127)]);
        assert_eq!(state.pedal(10, 1, SOSTENUTO, 0, None), [send(PLUGIN, 66, 0), send(SYNTH, 66, 0)]);
        assert_eq!(state.pedal(10, 1, SOFT, 0, None), [send(PLUGIN, 67, 0), send(SYNTH, 67, 0)]);
        assert_eq!(state.pedal(10, 1, 1, 127, Some((PLUGIN, 0))), [], "the modulation wheel is no pedal");
    }

    #[test]
    fn a_replaced_instrument_gets_the_pedal_again_with_the_next_note() {
        let mut state = SustainState::new();
        assert_eq!(state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 1))), [send(PLUGIN, 64, 127)]);
        assert_eq!(on(&mut state, 10, 1, 60, PLUGIN, 1), []);
        // The plug-in crashed and was restored: generation 2 never heard of the pedal.
        assert_eq!(on(&mut state, 10, 1, 62, PLUGIN, 2), [send(PLUGIN, 64, 127)]);
        assert_eq!(on(&mut state, 10, 1, 64, PLUGIN, 2), []);
        // A pedal movement is sent whatever stands there.
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [send(PLUGIN, 64, 0)]);
        // The pedal itself played to a new generation, at the same value.
        state.pedal(10, 1, SOFT, 127, Some((SYNTH, 5)));
        assert_eq!(state.pedal(10, 1, SOFT, 127, Some((SYNTH, 6))), [send(SYNTH, 67, 127)]);
    }

    #[test]
    fn silence_tells_every_instrument_pedal_up_and_keeps_the_pedals_down() {
        let mut state = SustainState::new();
        state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0)));
        state.pedal(10, 1, SOSTENUTO, 127, Some((PLUGIN, 0)));
        on(&mut state, 10, 1, 72, SYNTH, 0);
        on(&mut state, 10, 1, 48, ENGINE, 0);
        for (note, target) in [(72, SYNTH), (48, ENGINE)] {
            assert!(off(&mut state, 10, 1, note, target));
        }
        // Stop.
        assert_eq!(
            state.silence(),
            [ends(10, 1, 48, ENGINE), ends(10, 1, 72, SYNTH), send(PLUGIN, 64, 0), send(PLUGIN, 66, 0), send(SYNTH, 64, 0), send(SYNTH, 66, 0)]
        );
        assert_eq!(state.silence(), [], "once");
        // The pedal is still down: the next notes are sustained again.
        assert!(state.is_down(10, 1));
        assert_eq!(on(&mut state, 10, 1, 60, PLUGIN, 0), [send(PLUGIN, 64, 127), send(PLUGIN, 66, 127)]);
        assert!(off(&mut state, 10, 1, 60, PLUGIN));
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [ends(10, 1, 60, PLUGIN), send(PLUGIN, 64, 0)]);
        assert_eq!(state.pedal(10, 1, SOSTENUTO, 0, None), [send(PLUGIN, 66, 0)]);
    }

    #[test]
    fn an_instrument_that_leaves_its_place_is_told_pedal_up_and_forgotten() {
        let mut state = SustainState::new();
        assert!(state.is_idle());
        state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0)));
        state.pedal(10, 1, SOFT, 127, Some((PLUGIN, 0)));
        assert!(!state.is_idle());
        on(&mut state, 10, 1, 72, SYNTH, 0);
        on(&mut state, 10, 1, 60, PLUGIN, 0);
        on(&mut state, 10, 1, 48, ENGINE, 0);
        for (note, target) in [(60, PLUGIN), (48, ENGINE)] {
            assert!(off(&mut state, 10, 1, note, target));
        }
        // Tracks 0 and 1 are moved: the sampler's held note ends and the plug-in is told
        // pedal-up while both are still where the pedal knows them. The synth is untouched.
        let moved = |target: PedalTarget| matches!(target, PedalTarget::Engine { .. } | PedalTarget::Plugin { .. });
        assert_eq!(state.detach(moved), [ends(10, 1, 48, ENGINE), ends(10, 1, 60, PLUGIN), send(PLUGIN, 64, 0), send(PLUGIN, 67, 0)]);
        assert_eq!(state.detach(moved), [], "once");
        // The pedal is still down: whatever stands at track 1 now gets it with its first note.
        assert_eq!(on(&mut state, 10, 1, 62, PLUGIN, 0), [send(PLUGIN, 64, 127), send(PLUGIN, 67, 127)]);
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [send(PLUGIN, 64, 0), send(SYNTH, 64, 0)]);
        state.pedal(10, 1, SOFT, 0, None);
        assert!(state.is_idle());
    }

    #[test]
    fn a_panic_forgets_and_a_lost_route_lifts_its_pedals() {
        let mut state = SustainState::new();
        state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0)));
        on(&mut state, 10, 1, 60, ENGINE, 0);
        assert!(off(&mut state, 10, 1, 60, ENGINE));
        state.clear();
        state.clear();
        assert!(!state.is_down(10, 1), "after a panic a pedal counts as up until it is pressed again");
        assert_eq!(state.pedal(10, 1, SUSTAIN, 0, None), [], "nothing of the old session is released into the new one");

        // The input route is taken down: the devices' pedals come up, the application's stays.
        state.pedal(10, 1, SUSTAIN, 127, Some((PLUGIN, 0)));
        state.pedal(20, 4, SUSTAIN, 127, Some((ENGINE, 0)));
        state.pedal(APP_SOURCE, 1, SUSTAIN, 127, Some((PLUGIN, 0)));
        on(&mut state, 20, 4, 50, ENGINE, 0);
        assert!(off(&mut state, 20, 4, 50, ENGINE));
        assert_eq!(state.lift(|source| source != APP_SOURCE), [ends(20, 4, 50, ENGINE)], "the plug-in keeps the application's pedal");
        assert!(!state.is_down(10, 1) && !state.is_down(20, 4) && state.is_down(APP_SOURCE, 1));
        assert_eq!(state.lift(|_| true), [send(PLUGIN, 64, 0)]);
    }

    #[test]
    fn invalid_channels_and_notes_change_nothing() {
        let mut state = SustainState::new();
        for channel in [0, 17, 255] {
            assert_eq!(state.pedal(10, channel, SUSTAIN, 127, Some((PLUGIN, 0))), []);
            assert!(!state.is_down(10, channel));
            assert!(on(&mut state, 10, channel, 60, PLUGIN, 0).is_empty() && !off(&mut state, 10, channel, 60, PLUGIN));
        }
        state.pedal(i32::MIN, 16, SUSTAIN, 200, Some((ENGINE, 0)));
        assert_eq!(state.value(i32::MIN, 16, SUSTAIN), 127, "a negative device id and the last channel are valid; values stop at 127");
        assert!(!off(&mut state, i32::MIN, 16, 128, ENGINE), "no such note");
        assert!(off(&mut state, i32::MIN, 16, 127, ENGINE));
        assert_eq!(state.pedal(i32::MIN, 16, SUSTAIN, 0, None), [ends(i32::MIN, 16, 127, ENGINE)]);
    }
}
