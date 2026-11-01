/* UI state: the single state object, the view list, step-effect ranges and small accessors
   (current pattern, instrument, song position). */
(function () {
  'use strict';

  const app = window.STApp;

  const views = [['pattern', 'Pattern'], ['instrument', 'Instrument'], ['browser', 'Browser'], ['mixer', 'Mixer'],
      ['automation', 'Automation'], ['song', 'Song'], ['recording', 'Recording']];

  const FX = {
    volume: [0, 1, 0.01],
    pan: [-1, 1, 0.01],
    pitch: [-24, 24, 1],
    cutoff: [20, 20000, 1],
    resonance: [0, .95, .01],
    delay: [0, 1, .01],
    reverb: [0, 1, .01],
    probability: [0, 1, .01],
    ratchet: [1, 8, 1],
    reverse: [0, 1, 1],
    offset: [0, 1, .01],
    glide: [-24, 24, 1],
    microtiming: [0, .95, .01],
    arpeggio: [0, 255, 1],
    slice: [1, 128, 1],
    position: [0, 1, .01]
  };

  const state = {
    project: null,
    countIn: null,
    view: 'pattern',
    track: 0,
    instrument: 0,
    row: 0,
    column: 0,
    noteColumn: 0,
    sequence: 0,
    selectedLane: 0,
    automationQuery: '',
    playbackMode: 'pattern',
    playing: false,
    recording: false,
    follow: true,
    keyboardPlay: false,
    octave: 4,
    editStep: 1,
    chord: 'single',
    chordSet: '',
    midiApply: true,
    scale: 'chromatic',
    scaleMode: 'snap',
    keyVelocity: 100,
    root: 0,
    selection: null,
    clipboard: null,
    dirty: false,
    native: !!window.webkit?.messageHandlers?.supertracker,
    status: {},
    samples: {},
    plugins: [],
    audio: {},
    midi: {},
    library: { items: [], roots: [], query: '', category: '', kind: 'all', selected: 0, total: 0, scanning: false },
    modal: null,
    undo: [],
    redo: [],
    pending: new Map(),
    requestId: 0,
    engineReady: false,
    lastInputNote: 60
  };

  state.recordPlacement = (() => {
    try {
      return localStorage.getItem('st.recordPlacement') === 'live' ? 'live' : 'selected';
    } catch {
      return 'selected';
    }
  })();

  function pattern() {
    return state.project.patterns.find(x => x.id === state.project.activePattern) || state.project.patterns[0];
  }

  function beatsPerBar(p = state.project) {
    return (p.timeSignature?.numerator || 4) * 4 / (p.timeSignature?.denominator || 4);
  }

  function currentInstrument() {
    return state.project.instruments[state.instrument]
        || state.project.instruments[0];
  }

  function songBeat() {
    const p = state.project,
      sequence = state.playing ? state.status.sequence ?? state.sequence : state.sequence;
    return p.sequence.slice(0, sequence).reduce((sum, id) => sum
        + (p.patterns.find(x => x.id === id)?.length || 0) / p.linesPerBeat,
        0)
        + (state.playing ? (state.status.position ?? state.status.row ?? 0) / p.linesPerBeat : 0);
  }

  Object.assign(app, { views, FX, state, pattern, beatsPerBar, currentInstrument, songBeat });
})();
