/* Maschine MK3 controller interface (docs/MASCHINE_MK3.md), split into mk3-*.js modules that share
   window.STMK3Parts. The native driver (Bridge) forwards every button, pad, encoder, touch sensor and the
   touch strip as 'mk3.input' events; these modules turn them into SuperTracker actions through the app's
   public APIs and answer with LED state ('mk3.leds') and two 480 × 272 display lists ('mk3.screen'), sent
   only on change. The tracker is the model: everything reads the live project and app state; the only
   controller-local state is which page/mode is shown, held buttons, and per-viewer preferences.

   This module: constants, per-viewer controller preferences and the controller state (M). */
(() => {
  'use strict';
  const mk = window.STMK3Parts = {};

  const S = () => window.ST;
  const st = () => window.ST.state;
  const P = () => window.ST.state.project;
  const clamp = (value, lo, hi) => Math.max(lo, Math.min(hi, value));
  const clamp01 = value => clamp(Number(value) || 0, 0, 1);
  const { clone: copy } = window.STUtil;
  const now = () => (mk.api._clock ? mk.api._clock() : Date.now());
  const NOTE_NAMES = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];
  const noteText = n => window.__supertracker?.noteText
      ? window.__supertracker.noteText(n)
      : n === -1
      ? 'OFF'
      : n === -2
      ? 'CUT'
      : n == null || n === -3
      ? '···'
      : NOTE_NAMES[n % 12] + (Math.floor(n / 12) - 1);
  const noteName = n => NOTE_NAMES[((n % 12) + 12) % 12] + (Math.floor(n / 12) - 1);
  const FX_SHORT = {
    plugin: 'PLG',
    volume: 'VOL',
    pan: 'PAN',
    pitch: 'PIT',
    cutoff: 'CUT',
    resonance: 'RES',
    delay: 'DLY',
    reverb: 'REV',
    probability: 'PRB',
    ratchet: 'RAT',
    reverse: 'RVS',
    offset: 'OFF',
    glide: 'GLD',
    microtiming: 'µTM',
    arpeggio: 'ARP',
    slice: 'SLC',
    position: 'POS'
  };
  const FX_NAMES = {
    volume: 'Volume',
    pan: 'Pan',
    pitch: 'Pitch',
    cutoff: 'Cutoff',
    resonance: 'Resonance',
    delay: 'Delay send',
    reverb: 'Reverb send',
    probability: 'Probability',
    ratchet: 'Ratchet',
    reverse: 'Reverse',
    offset: 'Sample offset',
    glide: 'Glide',
    microtiming: 'Microtiming',
    arpeggio: 'Arpeggio',
    slice: 'Slice',
    position: 'Position',
    plugin: 'Plug-in'
  };
  const fxText = fx => {
    if (!fx) return '·· ··';
    if (fx.type === 'arpeggio') return 'ARP ' + Math.round(fx.value).toString(16).toUpperCase().padStart(2, '0');
    if (fx.type === 'position') return 'POS ' + Math.round(Math.max(0, Math.min(1, Number(fx.value) || 0)) * 100);
    return (FX_SHORT[fx.type] || String(fx.type).slice(0, 3).toUpperCase())
        + ' '
        + (fx.type === 'plugin' || Math.abs(fx.value) < 1 ? Math.round(fx.value * 100) : Math.round(fx.value));
  };
  const TRACK_COLORS = ['#bba374', '#a0adb1', '#b0af89', '#c58b69', '#b79583', '#9da688', '#94a39a', '#aaa0ad'];
  const HOLD = new Set(['select', 'duplicate', 'erase', 'mute', 'solo', 'events', 'lock', 'noteRepeat', 'volume', 'swing', 'tempo']);
  const TAP_MS = 350;
  const BURST_MS = 650;
  const FRAME_MS = 33;
  const GROUPS = ['groupA', 'groupB', 'groupC', 'groupD', 'groupE', 'groupF', 'groupG', 'groupH'];
  const DISPLAY = Array.from({ length: 8 }, (_, i) => 'display' + (i + 1));
  const BUTTONS = ['channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'arrowLeft', 'arrowRight', 'file', 'settings', 'auto', 'macro', ...DISPLAY,
    'notes', 'volume', 'swing', 'tempo', 'noteRepeat', 'lock', 'pitch', 'mod', 'perform', 'padMode', 'keyboard', 'chords', 'step', 'fixedVel', 'scene', 'pattern',
    'events', 'variation', 'duplicate', 'select', 'solo', 'mute', ...GROUPS, 'restart', 'erase', 'tap', 'follow', 'play', 'rec', 'stop', 'shift',
    'encoderPush', 'encoderUp', 'encoderDown', 'encoderLeft', 'encoderRight'];
  const LED_BUTTONS = BUTTONS.filter(name => !GROUPS.includes(name) && !name.startsWith('encoder'));
  const SCREEN_VIEW = {
    notes: 'pattern',
    macro: 'pattern',
    autoLanes: 'pattern',
    channel: 'instrument',
    channelMidi: 'instrument',
    plugin: 'instrument',
    sampling: 'instrument',
    arranger: 'song',
    patterns: 'song',
    mixer: 'mixer',
    mixerSends: 'mixer',
    browser: 'browser'
  };
  const SCREEN_BUTTON = {
    notes: 'notes',
    macro: 'macro',
    autoLanes: 'macro',
    channel: 'channel',
    channelMidi: 'channel',
    plugin: 'plugin',
    sampling: 'sampling',
    arranger: 'arranger',
    patterns: 'arranger',
    mixer: 'mixer',
    mixerSends: 'mixer',
    browser: 'browser',
    settings: 'settings'
  };
  const PERF_FX = [['filter', 'Filter sweep'], ['stutter', 'Stutter'], ['fade', 'Volume fade'], ['echo', 'Echo throw']];
  const VEL_CURVES = [['soft', 'Soft'], ['linear', 'Linear'], ['hard', 'Hard']];
  const AFTERTOUCH = [['off', 'Off'], ['channel', 'Channel'], ['poly', 'Poly']];
  const EDIT_STEPS = [0, 1, 2, 3, 4, 8, 16];
  const FILE_ITEMS = [['save', 'Save', 'Shift + FILE'], ['saveAs', 'Save as…', 'Choose a new project file'], ['open', 'Open…', 'Open a project'],
      ['new', 'New', 'Start an empty project'], ['export', 'Export audio…', 'Render the song or pattern'],
      ['exportMIDI', 'Export MIDI…', 'Standard MIDI file'], ['importMIDI', 'Import MIDI…', 'Read a MIDI file into patterns'],
      ['renameProject', 'Rename…', 'Project name'], ['demo', 'Demo song', 'Load the demo project']];
  const QUICK_SCALES = ['chromatic', 'major', 'minor', 'dorian', 'mixolydian', 'pentatonic', 'majorPentatonic', 'blues'];

  // ---- Per-viewer controller preferences (not project data) -------------------------------------
  const prefs = { sensitivity: .5, velocityCurve: 'linear', ledBrightness: 1, screenBrightness: 1, aftertouch: 'channel' };
  try { Object.assign(prefs, JSON.parse(window.localStorage?.getItem('supertracker.mk3') || '{}')); } catch { /* storage unavailable */ }
  function savePrefs() { try { window.localStorage?.setItem('supertracker.mk3', JSON.stringify(prefs)); } catch { /* storage unavailable */ } }

  // ---- Controller state ---------------------------------------------------------------------------
  const M = {
    connected: false, screens: false, exclusive: false, error: '',
    shift: false, held: new Map(), latched: null,
    screen: 'notes', overlay: null, fileMenu: null, page: {},
    padMode: 'pad', padAlt: false, padBank: { pad: 0, pattern: 0, scene: 0 }, stepPage: 0,
    // PATTERN page shown while PATTERN pad mode is pinned (patternPage); the pad mode to return to.
    patternPage: false, prevPadMode: 'pad',
    // Parameter level of CHANNEL / PLUG-IN (display buttons): master, group (cursor track in a track group), track, instrument.
    level: 'instrument', levelSlots: {}, mainTouch: null,
    pads: new Map(), shiftPads: [], dupSource: null, events: new Set(),
    bank: 0, touched: new Set(), encoderAcc: new Map(), mainAcc: 0,
    strip: { mode: null, value: .5, touched: false, start: null }, perfFx: 'filter',
    fixedVel: false, fixedVelValue: 70, variationAmount: 12, lastNote: new Map(),
    lists: { arranger: 0, patterns: 0, autoLanes: 0, pluginSlots: 0, file: 0, settings: 0 }, sendPage: 0,
    // Scroll position of every on-screen list (key → first visible row) and main-encoder acceleration.
    listTops: new Map(), mainRun: { at: -Infinity, direction: 0, count: 0 },
    // Browser columns: the focused column ('results' or a filter field), the filter cursors and a pending filter.
    browser: { focus: 'results', last: {}, cursor: {}, pending: null },
    knobs: { master: null, headphone: null, mic: null }, pedal: false,
    taps: [], flash: null, lastView: null, wrapped: false, queued: null, bendTrack: null, modTrack: null, pedalTracks: null,
    // Count-in is app/engine state (ST.recordWithCountIn): remaining clicks from the native status.
    get countIn() {
      const count = st()?.countIn;
      if (!count) return null;
      const remaining = Number(st().status?.countIn) || 0;
      return { bars: count.bars, remaining: remaining || null };
    }
  };
  const isHeld = name => !!M.held.get(name) && !M.held.get(name).shifted;
  const active = name => isHeld(name) || M.latched === name;

  Object.assign(mk, {
    S, st, P, clamp, clamp01, copy, now, NOTE_NAMES, noteText, noteName, FX_SHORT, FX_NAMES, fxText, TRACK_COLORS,
    HOLD, TAP_MS, BURST_MS, FRAME_MS, GROUPS, DISPLAY, BUTTONS, LED_BUTTONS, SCREEN_VIEW, SCREEN_BUTTON, PERF_FX,
    VEL_CURVES, AFTERTOUCH, EDIT_STEPS, FILE_ITEMS, QUICK_SCALES, prefs, savePrefs, M, isHeld, active
  });
})();
