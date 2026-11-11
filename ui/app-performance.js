/* Keyboard performance: chords, scales, octave and velocity for computer-keyboard and
   MIDI playing, and the performance settings sent to the engine for MIDI input. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    clamp, noteNames, state, toast, request, refresh, updateTransport, CHORD_SET_MIDI_BASE,
    CHORD_SET_MIDI_REGISTER, activeChordSet, chordSetPitchName, chordSetSlot, chordSetPitches,
    cycleChordSet
  } = app;

  const chords = {
    single: { name: 'Single note', notes: [0] },
    major: { name: 'Major', notes: [0, 4, 7] },
    minor: { name: 'Minor', notes: [0, 3, 7] },
    sus2: { name: 'Sus 2', notes: [0, 2, 7] },
    sus4: { name: 'Sus 4', notes: [0, 5, 7] },
    dim: { name: 'Diminished', notes: [0, 3, 6] },
    aug: { name: 'Augmented', notes: [0, 4, 8] },
    maj7: { name: 'Major 7', notes: [0, 4, 7, 11] },
    min7: { name: 'Minor 7', notes: [0, 3, 7, 10] },
    seventh: { name: 'Dominant 7', notes: [0, 4, 7, 10] },
    add9: { name: 'Add 9', notes: [0, 4, 7, 14] },
    min9: { name: 'Minor 9', notes: [0, 3, 7, 10, 14] },
    fifth: { name: 'Power fifth', notes: [0, 7, 12] },
    octave: { name: 'Octaves', notes: [0, 12] },
    // Maschine-style scale chords: stacked thirds taken from the selected key, so every key plays a chord that fits it.
    scaleTriad: { name: 'Scale triad', degrees: [0, 2, 4], notes: [0, 4, 7] },
    scaleSeventh: { name: 'Scale 7th', degrees: [0, 2, 4, 6], notes: [0, 4, 7, 11] },
    scaleNinth: { name: 'Scale 9th', degrees: [0, 2, 4, 6, 8], notes: [0, 4, 7, 11, 14] },
    scaleSus4: { name: 'Scale sus 4', degrees: [0, 3, 4], notes: [0, 5, 7] },
    scaleOpen: { name: 'Scale open (1-5-10)', degrees: [0, 4, 9], notes: [0, 7, 16] }
  };

  const scales = {
    chromatic: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    major: [0, 2, 4, 5, 7, 9, 11],
    minor: [0, 2, 3, 5, 7, 8, 10],
    harmonicMinor: [0, 2, 3, 5, 7, 8, 11],
    melodicMinor: [0, 2, 3, 5, 7, 9, 11],
    dorian: [0, 2, 3, 5, 7, 9, 10],
    phrygian: [0, 1, 3, 5, 7, 8, 10],
    lydian: [0, 2, 4, 6, 7, 9, 11],
    mixolydian: [0, 2, 4, 5, 7, 9, 10],
    locrian: [0, 1, 3, 5, 6, 8, 10],
    majorPentatonic: [0, 2, 4, 7, 9],
    pentatonic: [0, 3, 5, 7, 10],
    blues: [0, 3, 5, 6, 7, 10],
    wholeTone: [0, 2, 4, 6, 8, 10],
    japanese: [0, 1, 5, 7, 8]
  };

  const scaleNames = {
    chromatic: 'Chromatic',
    major: 'Major',
    minor: 'Minor',
    harmonicMinor: 'Harmonic minor',
    melodicMinor: 'Melodic minor',
    dorian: 'Dorian',
    phrygian: 'Phrygian',
    lydian: 'Lydian',
    mixolydian: 'Mixolydian',
    locrian: 'Locrian',
    majorPentatonic: 'Major pentatonic',
    pentatonic: 'Minor pentatonic',
    blues: 'Blues',
    wholeTone: 'Whole tone',
    japanese: 'Japanese (In)'
  };

  const keyVelocities = [127, 110, 100, 80, 60, 40];

  const keyNotes = {
    z: 0,
    s: 1,
    x: 2,
    d: 3,
    c: 4,
    v: 5,
    g: 6,
    b: 7,
    h: 8,
    n: 9,
    j: 10,
    m: 11,
    ',': 12,
    l: 13,
    '.': 14,
    ';': 15,
    '/': 16,
    q: 12,
    '2': 13,
    w: 14,
    '3': 15,
    e: 16,
    r: 17,
    '5': 18,
    t: 19,
    '6': 20,
    y: 21,
    '7': 22,
    u: 23,
    i: 24
  };

  function scaleNote(note) {
    const intervals = scales[state.scale] || scales.chromatic;
    if (state.scale === 'chromatic') return note;
    for (let delta = 0; delta < 12; delta++) if (intervals.includes(((note - state.root + delta) % 12 + 12) % 12)) return clamp(note + delta, 0, 127);
    return note;
  }

  // Computer-keyboard key index (0 = Z) to a pitch. In "degrees" mode every key is a
  // consecutive scale degree from the root, like Maschine pads in scale mode. MIDI input
  // uses the same mapping with its own base (see midiPerformanceMap).
  function keyboardNote(index, base = (state.octave + 1) * 12) {
    if (app.padKeysActive?.()
        || activeChordSet()
        || state.scaleMode !== 'degrees'
        || state.scale === 'chromatic') return clamp(base + index, 0, 127);
    const intervals = scales[state.scale] || scales.chromatic,
      length = intervals.length;
    return clamp(base + state.root + intervals[(index % length + length) % length] + 12 * Math.floor(index / length),
        0, 127);
  }

  // Semitone offsets of the active chord above an already scale-corrected root note.
  function chordIntervals(note) {
    const chord = chords[state.chord] || chords.single;
    if (!chord.degrees) return chord.notes.slice();
    const intervals = scales[state.scale === 'chromatic' ? 'major' : state.scale] || scales.major,
      length = intervals.length;
    const pc = ((note - state.root) % 12 + 12) % 12;
    let index = 0;
    intervals.forEach((value, i) => {
      if (value <= pc) index = i;
    });
    const pitch = k => intervals[k % length] + 12 * Math.floor(k / length);
    return chord.degrees.map(d => pitch(index + d) - pitch(index));
  }

  // One played pitch (from keyboardNote, the on-screen piano or a MIDI key) to the step it
  // performs: the lowest sounding note plus chord offsets, exactly as patterns store chords.
  // Chord sets read the key position relative to `base`; otherwise scale snap + chord apply.
  function performanceVoicing(note, base = (state.octave + 1) * 12, register = base) {
    const set = activeChordSet();
    if (set) {
      const pitches = chordSetPitches(note - base, set, register),
        root = pitches[0];
      return { note: root, intervals: pitches.map(p => p - root), slot: chordSetSlot(set, note - base) };
    }
    const root = scaleNote(note);
    return { note: root, intervals: chordIntervals(root) };
  }

  function performanceIsDefault() {
    return state.scale === 'chromatic' && state.chord === 'single' && !activeChordSet();
  }

  function performanceSummary() {
    const chord = chords[state.chord] || chords.single,
      set = activeChordSet(),
      repeat = window.STNoteRepeat;
    const mode = set
        ? `${chordSetPitchName(0, set)}${set.minor ? 'm' : ''} · Set ${set.name}`
        : `${noteNames[state.root]} ${scaleNames[state.scale] || state.scale}${state.scale !== 'chromatic' && state.scaleMode === 'degrees' ? ' · degrees' : ''}${state.chord !== 'single' ? ' · ' + chord.name : ''}`;
    return `${mode}${repeat?.enabled ? ' · ' + repeat.label() : ''}${!performanceIsDefault() && state.midiApply === false ? ' · MIDI off' : ''}`
        + ` · vel ${state.keyVelocity} · oct ${state.octave}`;
  }

  // Key, scale, keyboard mode, chord, chord set and the MIDI-keyboard choice are saved with the song
  // (project.performance, complete after normalize); repeat and velocity are live settings.
  function readPerformance(p) {
    const saved = p.performance;
    state.root = saved.root;
    state.scale = saved.scale;
    state.scaleMode = saved.scaleMode;
    state.chord = saved.chord;
    state.chordSet = saved.chordSet;
    state.midiApply = saved.applyToMIDI;
    scheduleMIDIPerformance();
  }

  function writePerformance(p) {
    p.performance = {
      root: state.root,
      scale: state.scale,
      scaleMode: state.scaleMode,
      chord: state.chord,
      chordSet: activeChordSet() ? state.chordSet : '',
      applyToMIDI: state.midiApply !== false
    };
  }

  function setPerformance(changes, announce = true) {
    Object.assign(state, changes);
    if (state.project) {
      writePerformance(state.project);
      state.dirty = true;
      app.scheduleAutosave();
    }
    app.stopHeld();
    scheduleMIDIPerformance();
    if (state.view === 'pattern' || state.view === 'instrument') {
      refresh();
    } else {
      updateTransport();
    }
    if (announce) toast(performanceSummary());
  }

  // ---- MIDI keyboard: the native host expands each incoming key through a 128-entry note
  // map, computed here with the same functions as the computer keyboard. MIDI C3 plays chord
  // 1 of a set; scale degrees count from C4. Sends are debounced and skipped when unchanged.
  function midiPerformanceMap() {
    if (state.midiApply === false || performanceIsDefault()) return null;
    const map = [];
    for (let key = 0; key < 128; key++) {
      const voiced = activeChordSet()
          ? performanceVoicing(key, CHORD_SET_MIDI_BASE, CHORD_SET_MIDI_REGISTER)
          : performanceVoicing(keyboardNote(key - CHORD_SET_MIDI_REGISTER, CHORD_SET_MIDI_REGISTER));
      map.push([...new Set(voiced.intervals.map(n => clamp(voiced.note + n, 0, 127)))].slice(0, 8));
    }
    return map.every((row, key) => row.length === 1 && row[0] === key) ? null : map;
  }

  let midiPerformanceTimer = null;
  let midiPerformanceSent;

  function scheduleMIDIPerformance(force = false) {
    if (force) midiPerformanceSent = undefined;
    clearTimeout(midiPerformanceTimer);
    midiPerformanceTimer = setTimeout(sendMIDIPerformance, 40);
  }

  function sendMIDIPerformance() {
    midiPerformanceTimer = null;
    if (!state.native || !state.engineReady || !state.project) return;
    const map = midiPerformanceMap(),
      json = JSON.stringify(map);
    if (json === midiPerformanceSent) return;
    midiPerformanceSent = json;
    request('midi.performance', { map }).catch(error => {
      midiPerformanceSent = undefined;
      toast('MIDI keyboard chords could not be applied: ' + error.message, 'error');
    });
  }

  function cyclePerformance(field, direction = 1) {
    const cycle = (list, value) => list[(list.indexOf(value) + direction + list.length) % list.length];
    if (field === 'chord') return setPerformance({ chord: cycle(Object.keys(chords), state.chord), chordSet: '' });
    if (field === 'chordSet') return cycleChordSet(direction);
    if (field === 'scale') return setPerformance({ scale: cycle(Object.keys(scales), state.scale) });
    if (field === 'root') return setPerformance({ root: (state.root + direction + 12) % 12 });
    if (field === 'scaleMode') return setPerformance({ scaleMode: state.scaleMode === 'degrees' ? 'snap' : 'degrees' });
    if (field === 'velocity') {
      state.keyVelocity = cycle(keyVelocities, state.keyVelocity);
      if (state.view === 'pattern') {
        refresh();
      } else {
        updateTransport();
      }
      return toast('Keyboard velocity ' + state.keyVelocity);
    }
    if (field === 'octave') {
      state.octave = clamp(state.octave + direction, 0, 8);
      if (state.view === 'pattern' || state.view === 'instrument') {
        refresh();
      } else {
        updateTransport();
      }
      return toast('Octave ' + state.octave);
    }
    if (field === 'editStep') {
      const steps = [0, 1, 2, 3, 4, 8, 16];
      state.editStep = steps[clamp(steps.indexOf(state.editStep) + direction, 0, steps.length - 1)] ?? 1;
      if (state.view === 'pattern') refresh();
      return toast('Edit step ' + state.editStep);
    }
  }

  Object.assign(app, {
    chords, scales, scaleNames, keyVelocities, keyNotes, keyboardNote, performanceVoicing,
    performanceIsDefault, performanceSummary, readPerformance, writePerformance, setPerformance,
    midiPerformanceMap, cyclePerformance
  });
})();
