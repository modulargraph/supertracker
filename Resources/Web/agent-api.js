/* SuperTracker agent API (window.STAgent).
   A validated, UI-independent interface to the open song for AI assistants. The native
   control socket (AgentControlServer.swift, used by `SuperTracker --mcp`) calls
   STAgent.dispatch(method, paramsJSON) on the main thread. Every editing call is one
   ST.mutate, so it refreshes the interface, syncs the audio engine and is one ⌘Z step
   labelled "AI: …". Nothing here simulates key presses.

   Conventions (also returned by help()):
   - Tracks and instruments are 0-based indices (the UI shows track 01 for index 0) or names.
   - Patterns are referenced by id (see summary().patterns) or by name; default: the active one.
   - Rows are 0-based. Notes: "C-4", "C4", "C#4", "Db3", MIDI numbers (60 = C-4), "OFF", "CUT".
   - Step FX are readable strings: "probability 50%", "ratchet 3", "pitch -12", "cutoff 800Hz",
     "pan -30%", "reverse", "arpeggio 37" (hex nibbles = +3 +7), "microtiming 25%", "slice 4",
     "position 50%" (wavetable position).
     Percent types take percent numbers ("probability 50" = 50%).
   - Instrument/track values accept native numbers or strings with units: "800 Hz", "1.2 kHz",
     "-6 dB", "20 ms", "50%", "30% L", "+7 st", "C-3".
   - Mixer channels (channel): "master", a bus name or "bus:<id>", or a track like above; mixer() lists them. */
(function () {
  'use strict';
  const S = () => window.ST, st = () => S().state;
  const { clone } = window.STUtil;
  const clampNum = (n, a, b) => Math.max(a, Math.min(b, n));
  const round = (value, digits = 4) => Number(Number(value).toFixed(digits));
  const isInt = value => Number.isInteger(value);
  class AgentError extends Error {}
  const fail = message => { throw new AgentError(message); };
  const LIMITS = { tracks: window.TRACK_SANITY_LIMIT ?? Infinity, patterns: 256, length: 256, effectSlots: 16, songSlots: 1024 };
  const TRACK_COLORS = ['#bba374', '#a0adb1', '#b0af89', '#c58b69', '#b79583', '#9da688', '#94a39a', '#aaa0ad'];

  // ---------------------------------------------------------------- notes
  const SHARP = ['C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B'];
  const PITCH = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };
  function noteName(n) {
    if (n === -1) return 'OFF';
    if (n === -2) return 'CUT';
    if (n == null || n === -3) return null;
    const name = SHARP[n % 12], octave = Math.floor(n / 12) - 1;
    return octave < 0 ? String(n) : name.length === 1 ? `${name}-${octave}` : `${name}${octave}`;
  }
  function parseNote(value, what = 'note') {
    if (typeof value === 'number') {
      if (isInt(value) && value >= 0 && value <= 127) return value;
      if (value === -1 || value === -2) return value;
      fail(`${what} must be a MIDI number 0–127 (60 = C-4), a name like "C-4" or "F#3", "OFF" or "CUT" (got ${value}).`);
    }
    if (typeof value !== 'string') fail(`${what} must be a note name like "C-4", a MIDI number, "OFF" or "CUT".`);
    const text = value.trim();
    if (/^(off|===|note ?off)$/i.test(text)) return -1;
    if (/^(cut|\^\^\^)$/i.test(text)) return -2;
    if (/^\d+$/.test(text)) return parseNote(Number(text), what);
    const match = /^([a-g])\s*([#♯b♭]?)\s*-?\s*(\d)$/i.exec(text);
    if (!match) fail(`Could not read ${what} "${value}". Use names like "C-4", "C4", "C#4", "Db3" (60 = C-4), a MIDI number, "OFF" or "CUT".`);
    const accidental = match[2] === '#' || match[2] === '♯' ? 1 : match[2] === 'b' || match[2] === '♭' ? -1 : 0;
    const midi = (Number(match[3]) + 1) * 12 + PITCH[match[1].toLowerCase()] + accidental;
    if (midi < 0 || midi > 127) fail(`${what} "${value}" is outside the MIDI range.`);
    return midi;
  }

  // ---------------------------------------------------------------- step effects
  const FX_TYPES = {
    volume: { unit: '%', scale: 100, aliases: ['vol', 'level'] },
    pan: { unit: '%', scale: 100, aliases: [] },
    pitch: { unit: 'st', scale: 1, signed: true, aliases: ['pit', 'transpose'] },
    cutoff: { unit: 'Hz', scale: 1, aliases: ['cut', 'filter'] },
    resonance: { unit: '%', scale: 100, aliases: ['res', 'q'] },
    delay: { unit: '%', scale: 100, aliases: ['dly', 'delaysend'] },
    reverb: { unit: '%', scale: 100, aliases: ['verb', 'reverbsend'] },
    probability: { unit: '%', scale: 100, aliases: ['prob', 'prb', 'chance'] },
    ratchet: { unit: '', scale: 1, aliases: ['rat', 'retrig', 'repeat'] },
    reverse: { unit: '', scale: 1, flag: true, aliases: ['rvs', 'rev'] },
    offset: { unit: '%', scale: 100, aliases: ['off', 'sampleoffset', 'start'] },
    glide: { unit: 'st', scale: 1, signed: true, aliases: ['gld', 'portamento', 'slide'] },
    microtiming: { unit: '%', scale: 100, aliases: ['micro', 'µtm', 'utm', 'late', 'nudge'] },
    arpeggio: { unit: 'hex', scale: 1, aliases: ['arp'] },
    slice: { unit: '', scale: 1, aliases: ['slc'] },
    position: { unit: '%', scale: 100, aliases: ['pos', 'wavetable', 'wtpos', 'wavetableposition'] }
  };
  // Tables looked up with caller-supplied keys have no prototype: "constructor" or "toString" is not an effect.
  const FX_ALIAS = Object.create(null);
  for (const [type, spec] of Object.entries(FX_TYPES)) { FX_ALIAS[type] = type; for (const alias of spec.aliases) FX_ALIAS[alias] = type; }
  const ownValue = (table, key) => typeof key === 'string' && Object.hasOwn(table, key) ? table[key] : undefined;
  function fxRange(type) { return S().FX?.[type] || [-1e9, 1e9, .01]; }
  function formatFX(fx) {
    if (!fx || typeof fx !== 'object') return null;
    if (fx.type === 'plugin') return `plugin ${fx.name || fx.address} ${round(Number(fx.value) * 100, 1)}%`;
    const spec = FX_TYPES[fx.type], value = Number(fx.value);
    if (!spec) return `${fx.type} ${value}`;
    if (spec.flag) return value >= .5 ? fx.type : `${fx.type} off`;
    if (spec.unit === 'hex') return `${fx.type} ${Math.round(value).toString(16).toUpperCase().padStart(2, '0')}`;
    const shown = round(value * spec.scale, spec.unit === '%' ? 1 : 3);
    return `${fx.type} ${spec.signed && shown > 0 ? '+' : ''}${shown}${spec.unit === '%' ? '%' : spec.unit === 'Hz' ? 'Hz' : ''}`;
  }
  function parseFX(value, slot) {
    if (value === null) return null;
    if (value && typeof value === 'object') {
      if (value.type === 'plugin') {
        if (typeof value.instanceID !== 'string'
            || typeof value.address !== 'string'
                && typeof value.address !== 'number') fail(`${slot}: plug-in step effects need {type:"plugin", instanceID, address, value 0–1}. Use pluginParameters to find addresses.`);
        const v = Number(value.value);
        if (!Number.isFinite(v) || v < 0 || v > 1) fail(`${slot}: plug-in effect value must be 0–1.`);
        return { type: 'plugin', instanceID: value.instanceID, address: String(value.address), name: String(value.name || ''), value: v };
      }
      if (!ownValue(FX_TYPES, value.type)) fail(`${slot}: unknown effect type "${value.type}". Types: ${Object.keys(FX_TYPES).join(', ')}.`);
      return checkFX(value.type, Number(value.value), slot, value.value);
    }
    if (typeof value !== 'string'
        || !value.trim()) fail(`${slot} must be a string like "probability 50%", an object {type, value}, or null to remove it.`);
    const text = value.trim().toLowerCase().replace(/\s+/g, ' ');
    const match = /^([a-zµ]+)\s*(.*)$/.exec(text);
    const type = match && FX_ALIAS[match[1]];
    if (!type) fail(`${slot}: unknown effect "${value}". Types: ${Object.keys(FX_TYPES).join(', ')}.`);
    const spec = FX_TYPES[type], rest = match[2].trim();
    if (spec.flag) {
      if (!rest || /^(on|1|yes|true)$/.test(rest)) return { type, value: 1 };
      if (/^(off|0|no|false)$/.test(rest)) return { type, value: 0 };
      fail(`${slot}: write "reverse" or "reverse off".`);
    }
    if (spec.unit === 'hex') {
      const plus = rest.match(/^\+?(\d{1,2})\s+\+?(\d{1,2})$/);
      if (plus) { const a = Number(plus[1]), b = Number(plus[2]); if (a > 15 || b > 15) fail(`${slot}: arpeggio offsets are 0–15 semitones.`); return { type, value: a * 16 + b }; }
      if (!/^[0-9a-f]{1,2}$/.test(rest)) fail(`${slot}: write arpeggio as two hex digits ("arpeggio 37" = +3 +7) or "arpeggio +3 +7".`);
      return checkFX(type, parseInt(rest, 16), slot, value);
    }
    const number = /^([+-]?\d*\.?\d+)\s*(%|hz|khz|st|semitones?|x|×)?$/.exec(rest.replace('−', '-'));
    if (!number) fail(`${slot}: could not read the value in "${value}". Example: "${type} ${spec.unit === '%' ? '50%' : spec.unit === 'Hz' ? '800Hz' : spec.unit === 'st' ? '-12' : '3'}".`);
    let raw = Number(number[1]);
    if (number[2] === 'khz') raw *= 1000;
    return checkFX(type, raw / (spec.unit === '%' ? 100 : 1), slot, value);
  }
  function checkFX(type, value, slot, original) {
    const [min, max] = fxRange(type), spec = FX_TYPES[type];
    if (!Number.isFinite(value)) fail(`${slot}: "${original}" is not a number.`);
    if (value < min - 1e-9 || value > max + 1e-9) {
      const show = v => spec.unit === '%' ? `${round(v * 100, 1)}%` : `${v}${spec.unit === 'Hz' ? ' Hz' : ''}`;
      fail(`${slot}: ${type} must be between ${show(min)} and ${show(max)} (got ${show(value)}).`);
    }
    if (['ratchet', 'slice', 'arpeggio', 'pitch', 'glide'].includes(type) && !isInt(round(value, 6))) fail(`${slot}: ${type} takes whole numbers.`);
    return { type, value: round(value, 4) };
  }

  // ---------------------------------------------------------------- parameter values with units
  const PARAMS = {
    volume: { label: 'Volume', min: 0, max: 1, kind: 'db', automation: 'instrumentVolume' },
    pan: { label: 'Pan', min: -1, max: 1, kind: 'pan', automation: 'instrumentPan' },
    tune: { label: 'Tune', min: -48, max: 48, kind: 'semi', note: 'semitones, fractional = cents' },
    attack: { label: 'Attack', min: .001, max: 5, kind: 'time' }, decay: { label: 'Decay', min: .001, max: 5, kind: 'time' },
    sustain: { label: 'Sustain', min: 0, max: 1, kind: 'pct' }, release: { label: 'Release', min: .001, max: 8, kind: 'time' },
    cutoff: { label: 'Filter cutoff', min: 20, max: 20000, kind: 'hz' }, resonance: { label: 'Filter resonance', min: 0, max: .95, kind: 'pct' },
    drive: { label: 'Drive', min: 0, max: 1, kind: 'pct' }, bitDepth: { label: 'Bit depth', min: 2, max: 16, kind: 'int' },
    rootNote: { label: 'Root note', min: 0, max: 127, kind: 'note', types: ['sample'] },
    start: { label: 'Sample start', min: 0, max: .999, kind: 'pos', types: ['sample'] },
    end: { label: 'Sample end', min: .001, max: 1, kind: 'pos', types: ['sample'] },
    loopStart: { label: 'Loop start', min: 0, max: .999, kind: 'pos', types: ['sample'] },
    loopEnd: { label: 'Loop end', min: .001, max: 1, kind: 'pos', types: ['sample'] },
    sliceCount: { label: 'Slices', min: 1, max: 128, kind: 'int', types: ['sample'] },
    grainSize: { label: 'Grain size', min: .005, max: .5, kind: 'time', types: ['sample'] },
    grainPosition: { label: 'Grain position', min: 0, max: 1, kind: 'pct', types: ['sample'], automation: 'grainPosition' },
    grainDensity: { label: 'Grain density', min: 1, max: 8, kind: 'int', types: ['sample'], automation: 'grainDensity', note: 'overlapping grains (2 = classic)' },
    grainSpray: { label: 'Grain spray', min: 0, max: 1, kind: 'time', types: ['sample'], automation: 'grainSpray', note: 'random position offset per grain' },
    grainPitchSpread: { label: 'Grain pitch spread', min: 0, max: 24, kind: 'semi', types: ['sample'], automation: 'grainPitchSpread', note: 'random ± semitones per grain' },
    grainScan: { label: 'Grain scan speed', min: -4, max: 4, types: ['sample'], automation: 'grainScan', note: '1 = original speed through the sample, 0 = frozen, negative = backwards' },
    grainReverse: { label: 'Grain reverse chance', min: 0, max: 1, kind: 'pct', types: ['sample'], automation: 'grainReverse' },
    grainStereo: { label: 'Grain stereo spread', min: 0, max: 1, kind: 'pct', types: ['sample'], automation: 'grainStereo' },
    wavetablePosition: { label: 'Wavetable position', min: 0, max: 1, kind: 'pct', types: ['sample'], automation: 'wavetablePosition' },
    wavetableFrameSize: { label: 'Wavetable frame size', min: 0, max: 8192, kind: 'int', types: ['sample'], note: '0 = auto (from the file, else 2048); 16–8192 samples' },
    filterAttack: { label: 'Filter attack', min: 0, max: 60, kind: 'time' }, filterDecay: { label: 'Filter decay', min: 0, max: 60, kind: 'time' },
    filterSustain: { label: 'Filter sustain', min: 0, max: 1, kind: 'pct' },
    filterRelease: { label: 'Filter release', min: .001, max: 60, kind: 'time' },
    filterEnvAmount: { label: 'Filter envelope amount', min: -48, max: 48, kind: 'semi' },
    lfoRate: { label: 'LFO 1 rate', min: 0, max: 100, kind: 'rate' },
    lfoBeats: { label: 'LFO 1 cycle', min: 1 / 64, max: 64, kind: 'beats' },
    lfoDepth: { label: 'LFO 1 depth', min: 0, max: 1, kind: 'pct' },
    lfo2Rate: { label: 'LFO 2 rate', min: 0, max: 100, kind: 'rate' },
    lfo2Beats: { label: 'LFO 2 cycle', min: 1 / 64, max: 64, kind: 'beats' },
    lfo2Depth: { label: 'LFO 2 depth', min: 0, max: 1, kind: 'pct' },
    midiChannel: { label: 'MIDI output channel', min: 1, max: 16, kind: 'int', types: ['midi'] },
    midiInputChannel: { label: 'MIDI input channel', min: 0, max: 16, kind: 'int', types: ['midi'] },
    filterType: { label: 'Filter type', options: ['lowpass', 'highpass', 'bandpass'] },
    mode: { label: 'Sample play mode', options: ['oneshot', 'forward', 'reverse', 'pingpong', 'slice', 'wavetable', 'granular'], types: ['sample'] },
    lfoTarget: { label: 'LFO 1 target', options: ['cutoff', 'pitch', 'volume', 'pan', 'position', 'grainSize'] },
    lfoShape: { label: 'LFO 1 shape', options: ['sine', 'triangle', 'saw', 'square', 'random'] },
    lfo2Target: { label: 'LFO 2 target', options: ['cutoff', 'pitch', 'volume', 'pan', 'position', 'grainSize'] },
    lfo2Shape: { label: 'LFO 2 shape', options: ['sine', 'triangle', 'saw', 'square', 'random'] },
    lfoSync: { label: 'LFO 1 tempo sync', bool: true },
    lfo2Sync: { label: 'LFO 2 tempo sync', bool: true },
    monitor: { label: 'Input monitor', bool: true, types: ['midi'] }
  };
  const PARAM_ALIAS = Object.create(null);
  for (const [key, spec] of Object.entries(PARAMS)) { PARAM_ALIAS[key.toLowerCase()] = key; PARAM_ALIAS[spec.label.toLowerCase()] = key; }
  Object.assign(PARAM_ALIAS, {
    level: 'volume',
    gain: 'volume',
    filter: 'cutoff',
    'filter cutoff': 'cutoff',
    q: 'resonance',
    position: 'wavetablePosition',
    'frame size': 'wavetableFrameSize',
    'play mode': 'mode',
    root: 'rootNote',
    slices: 'sliceCount',
    'lfo rate': 'lfoRate',
    'lfo depth': 'lfoDepth',
    'lfo target': 'lfoTarget',
    'lfo shape': 'lfoShape',
    'filter env amount': 'filterEnvAmount',
    'env amount': 'filterEnvAmount'
  });

  function formatValue(kind, value) {
    const v = Number(value); if (!Number.isFinite(v)) return String(value);
    switch (kind) {
      case 'time': return v < 1 ? `${round(v * 1000, 1)} ms` : `${round(v, 3)} s`;
      case 'hz': return v < 1000 ? `${round(v, 1)} Hz` : `${round(v / 1000, 3)} kHz`;
      case 'rate': return v <= 0 ? 'off' : `${round(v, 3)} Hz`;
      case 'db': return v <= .0001 ? '-inf dB' : `${round(20 * Math.log10(v), 2)} dB`;
      case 'pan': return Math.abs(v) < .005 ? 'center' : `${round(Math.abs(v) * 100, 1)}% ${v < 0 ? 'L' : 'R'}`;
      case 'pct': case 'pos': return `${round(v * 100, 2)}%`;
      case 'semi': return `${v > 0 ? '+' : ''}${round(v, 2)} st`;
      case 'note': return `${noteName(Math.round(v)) ?? Math.round(v)} (${Math.round(v)})`;
      case 'beats': return `${round(v, 5)} beats`;
      case 'bpm': return `${round(v, 3)} BPM`;
      default: return String(round(v, 4));
    }
  }
  function parseValue(spec, raw, name) {
    const range = () => `${formatValue(spec.kind, spec.min)} … ${formatValue(spec.kind, spec.max)}`;
    const check = value => {
      if (!Number.isFinite(value)) fail(`${name}: "${raw}" is not a valid value. Range: ${range()}.`);
      if (value < spec.min - 1e-9 || value > spec.max + 1e-9) fail(`${name} must be between ${range()} (got ${formatValue(spec.kind, value)}).`);
      return spec.kind === 'int' || spec.kind === 'note' ? Math.round(value) : round(value, 6);
    };
    if (typeof raw === 'number') return check(raw);
    if (typeof raw !== 'string' || !raw.trim()) fail(`${name}: give a number or a string with units (e.g. "800 Hz", "-6 dB", "50%"). Range: ${range()}.`);
    const text = raw.trim().toLowerCase().replace(/−/g, '-').replace(/\s+/g, ' ');
    if (spec.kind === 'note') return check(parseNote(raw, name));
    if (spec.kind === 'pan') {
      if (/^(c|center|centre|middle|0%?)$/.test(text)) return 0;
      let m = /^(\d*\.?\d+)\s*%?\s*([lr])$/.exec(text) || /^([lr])\s*(\d*\.?\d+)\s*%?$/.exec(text);
      if (m) { const [num, side] = /[lr]/.test(m[1]) ? [m[2], m[1]] : [m[1], m[2]]; return check(Number(num) / 100 * (side === 'l' ? -1 : 1)); }
      m = /^([+-]?\d*\.?\d+)\s*%$/.exec(text); if (m) return check(Number(m[1]) / 100);
    }
    if (spec.kind === 'db' && /^-?inf/.test(text)) return check(0);
    if (spec.kind === 'rate' && text === 'off') return check(0);
    const fraction = /^(\d+)\s*\/\s*(\d+)\s*(beats?)?$/.exec(text);
    if (fraction && spec.kind === 'beats') return check(Number(fraction[1]) / Number(fraction[2]));
    const m = /^([+-]?\d*\.?\d+)\s*([a-z%]*)$/.exec(text);
    if (!m) fail(`${name}: could not read "${raw}". Range: ${range()}.`);
    const n = Number(m[1]), unit = m[2];
    if (!unit) return check(n);
    if (unit === '%') return check(spec.kind === 'db'
        ? n / 100
        : spec.kind === 'semi' || spec.kind === 'hz' || spec.kind === 'int' || spec.kind === 'bpm'
        ? NaN
        : n / 100);
    if (spec.kind === 'db' && unit === 'db') return check(Math.pow(10, n / 20));
    if (spec.kind === 'hz' || spec.kind === 'rate') { if (unit === 'hz') return check(n); if (unit === 'khz') return check(n * 1000); }
    if (spec.kind === 'time') { if (unit === 'ms') return check(n / 1000); if (unit === 's' || unit === 'sec') return check(n); }
    if (spec.kind === 'semi' && /^(st|semi|semis|semitones?)$/.test(unit)) return check(n);
    if (spec.kind === 'beats' && /^beats?$/.test(unit)) return check(n);
    if (spec.kind === 'bpm' && unit === 'bpm') return check(n);
    fail(`${name}: the unit "${unit}" does not fit this parameter. Range: ${range()}.`);
  }
  const TRACK_VALUES = {
    volume: { min: 0, max: 1.5, kind: 'db' }, pan: { min: -1, max: 1, kind: 'pan' },
    delaySend: { min: 0, max: 1, kind: 'pct' }, reverbSend: { min: 0, max: 1, kind: 'pct' }
  };
  const SONG_VALUES = {
    bpm: { min: 20, max: 400, kind: 'bpm' }, swing: { min: 0, max: .75, kind: 'pct' },
    masterVolume: { min: 0, max: 1.5, kind: 'db' }, metronomeVolume: { min: 0, max: 1, kind: 'pct' }
  };

  // ---------------------------------------------------------------- lookups
  function project() { const p = window.ST?.state?.project; if (!p) fail('SuperTracker is still starting. Try again in a moment.'); return p; }
  function activePattern(p) { return p.patterns.find(x => x.id === p.activePattern) || p.patterns[0]; }
  function findPattern(p, ref, what = 'pattern') {
    if (ref === undefined || ref === null || ref === '') return activePattern(p);
    if (typeof ref === 'number') { const hit = p.patterns.find(x => x.id === ref); if (hit) return hit; fail(`There is no ${what} with id ${ref}. Pattern ids: ${p.patterns.map(x => `${x.id} "${x.name}"`).join(', ')}.`); }
    if (typeof ref === 'string') {
      const text = ref.trim().toLowerCase();
      const hit = p.patterns.find(x => x.name.toLowerCase() === text) || (/^\d+$/.test(text) ? p.patterns.find(x => x.id === Number(text)) : null);
      if (hit) return hit;
      const partial = p.patterns.filter(x => x.name.toLowerCase().includes(text));
      if (partial.length === 1) return partial[0];
      fail(`No single ${what} matches "${ref}". Patterns: ${p.patterns.map(x => `${x.id} "${x.name}"`).join(', ')}.`);
    }
    fail(`${what} must be a pattern id or name.`);
  }
  function findTrack(p, ref, what = 'track') {
    if (typeof ref === 'number') { if (isInt(ref)
        && ref >= 0
        && ref < p.tracks.length) return ref; fail(`${what} ${ref} does not exist. Tracks are 0–${p.tracks.length - 1} (0-based; the UI shows 01 for track 0).`); }
    if (typeof ref === 'string' && ref.trim()) {
      const text = ref.trim().toLowerCase();
      if (/^\d+$/.test(text)) return findTrack(p, Number(text), what);
      let index = p.tracks.findIndex(t => String(t.name).toLowerCase() === text);
      if (index < 0) {
        const hits = p.tracks.map((t, i) => String(t.name).toLowerCase().includes(text) ? i : -1).filter(i => i >= 0);
        if (hits.length === 1) index = hits[0];
      }
      if (index >= 0) return index;
      fail(`No single track matches "${ref}". Tracks: ${p.tracks.map((t, i) => `${i} "${t.name}"`).join(', ')}.`);
    }
    fail(`${what} must be a 0-based track index or a track name.`);
  }
  function findInstrument(p, ref, what = 'instrument') {
    if (typeof ref === 'number') { if (isInt(ref)
        && ref >= 0
        && ref < p.instruments.length) return ref; fail(`${what} ${ref} does not exist. Instruments are 0–${p.instruments.length - 1}.`); }
    if (typeof ref === 'string' && ref.trim()) {
      const text = ref.trim().toLowerCase();
      if (/^\d+$/.test(text)) return findInstrument(p, Number(text), what);
      let index = p.instruments.findIndex(x => String(x.name).toLowerCase() === text);
      if (index < 0) {
        const hits = p.instruments.map((x, i) => String(x.name).toLowerCase().includes(text) ? i : -1).filter(i => i >= 0);
        if (hits.length === 1) index = hits[0];
      }
      if (index >= 0) return index;
      fail(`No single instrument matches "${ref}". Instruments: ${p.instruments.map((x, i) => `${i} "${x.name}"`).join(', ')}.`);
    }
    fail(`${what} must be an instrument index or name.`);
  }
  function trackList(p, ref) {
    if (ref === undefined || ref === null) return p.tracks.map((_, i) => i);
    // Each track once: a track named twice ([0, "Bass"]) must not be shifted or transposed twice.
    return [...new Set((Array.isArray(ref) ? ref : [ref]).map(item => findTrack(p, item)))];
  }
  function rowRange(pat, from, to) {
    const a = from === undefined ? 0 : from, b = to === undefined ? pat.length - 1 : to;
    for (const [name, value] of [['from', a], ['to', b]]) if (!isInt(value)
        || value < 0
        || value >= pat.length) fail(`${name} must be a row from 0 to ${pat.length - 1} in "${pat.name}" (got ${value}).`);
    if (b < a) fail(`to (${b}) must not be before from (${a}).`);
    return [a, b];
  }
  function requireUnfrozen(p, tracks) {
    for (const t of tracks) if (p.tracks[t]?.frozen) fail(`Track ${t} "${p.tracks[t].name}" is frozen. Unfreeze it first (runCommand "unfreezeTrack" with that track selected) or edit another track.`);
  }
  const hasNote = step => !!step && Number.isFinite(step.note) && step.note >= 0;
  /* Note columns (app-note-columns.js): a track has 1–8; column 0 is the step, column c > 0 is step.columns[c - 1]
     ({note, instrument, velocity, gate}); the row's FX belong to column 0. */
  const noteColumns = (p, t) => clampNum(Math.round(Number(p.tracks[t]?.noteColumns) || 1), 1, 8);
  const noteCells = step => [...(hasNote(step) ? [step] : []), ...(step?.columns || []).filter(cell => cell?.note >= 0)];
  const dropEmpty = step => !step || step.note === -3 && !step.fx1 && !step.fx2 && !(step.columns || []).some(Boolean) ? null : step;
  const onTrack = (p, plugin, t) => plugin.owner ? plugin.owner === `track:${p.tracks[t]?.routingID}` : plugin.track === t;
  function pluginOn(p, t, kind, slot = 0) {
    return (p.audioPlugins || []).find(plugin => onTrack(p, plugin, t) && plugin.kind === kind && (plugin.slot ?? 0) === slot) || null;
  }
  function pluginTarget(plugin, t) {
    return { track: t, ...(plugin.owner ? { owner: plugin.owner } : {}), kind: plugin.kind, slot: plugin.slot ?? 0, instanceID: plugin.instanceID };
  }

  /* Mixer channels, as the mixer (studio.js) and routing (routing.js) address them: {type: 'track' | 'group' |
     'return' | 'master', owner: 'track:<routingID>' | 'bus:<id>' | 'master', name, track?, bus?}. Callers name
     them "master", by bus name or "bus:<id>", or like tracks (index or name). */
  const standardRouting = p => p.routing?.version === 1 && p.routing.mixModel === 'standard';
  const trackChannel = (p, t) => ({ type: 'track', owner: `track:${p.tracks[t].routingID || t}`, name: p.tracks[t].name, track: t });
  const busChannel = bus => ({ type: bus.type === 'return' ? 'return' : 'group', owner: `bus:${bus.id}`, name: bus.name || 'Bus', bus: bus.id });
  const MASTER = Object.freeze({ type: 'master', owner: 'master', name: 'Master' });
  function channelList(p) {
    return [MASTER, ...(p.routing?.buses || []).map(busChannel), ...p.tracks.map((_, t) => trackChannel(p, t))];
  }
  const channelLabel = ch => ch.type === 'track' ? `track ${ch.track} "${ch.name}"` : ch.type === 'master' ? 'Master' : `${ch.type} bus "${ch.name}"`;
  function channelChoices(p) {
    const list = channelList(p).map(ch => ch.type === 'track' ? `${ch.track} "${ch.name}"` : ch.type === 'master' ? '"master"' : `"${ch.name}" (${ch.owner})`);
    return list.length > 40 ? `${list.slice(0, 40).join(', ')}, … (see mixer)` : list.join(', ');
  }
  function findChannel(p, ref, what = 'channel') {
    if (typeof ref === 'number') return trackChannel(p, findTrack(p, ref, what));
    if (typeof ref !== 'string' || !ref.trim()) fail(`${what} must be "master", a bus name or "bus:<id>", or a track index or name.`);
    const text = ref.trim(), lower = text.toLowerCase();
    if (lower === 'master') return MASTER;
    if (/^\d+$/.test(text)) return trackChannel(p, findTrack(p, Number(text), what));
    const prefixed = /^(bus|track):(.+)$/i.exec(text);
    if (prefixed) {
      const hit = channelList(p).find(ch => ch.owner === `${prefixed[1].toLowerCase()}:${prefixed[2]}`);
      if (hit) return hit;
      fail(`There is no channel ${text}. Channels: ${channelChoices(p)}.`);
    }
    const named = channelList(p).filter(ch => ch.type !== 'master');
    let hits = named.filter(ch => String(ch.name).toLowerCase() === lower);
    if (!hits.length) hits = named.filter(ch => String(ch.name).toLowerCase().includes(lower));
    if (hits.length === 1) return hits[0];
    if (hits.length > 1) fail(`"${ref}" matches several channels: ${hits.slice(0, 12).map(ch => ch.type === 'track' ? `track ${ch.track}` : ch.owner).join(', ')}. Use a track index or "bus:<id>".`);
    fail(`No channel matches "${ref}". Channels: ${channelChoices(p)}.`);
  }
  function channelEntity(p, ch) {
    if (ch.type === 'master') return p;
    if (ch.type === 'track') return p.tracks[ch.track] || null;
    return (p.routing?.buses || []).find(bus => bus.id === ch.bus) || null;
  }
  const channelOf = (p, owner) => channelList(p).find(ch => ch.owner === owner) || null;
  function channelEffects(p, ch) {
    const own = ch.type === 'track' ? plugin => onTrack(p, plugin, ch.track) : plugin => plugin.owner === ch.owner;
    return (p.audioPlugins || []).filter(plugin => own(plugin) && plugin.kind === 'effect').sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
  }
  /* The plug-in in a {channel, kind, slot} reference (tracks also have their instrument plug-in). */
  function slotPlugin(p, ref) {
    if (ref.channel.type === 'track') return pluginOn(p, ref.channel.track, ref.kind, ref.slot);
    return ref.kind === 'effect' ? channelEffects(p, ref.channel).find(plugin => (plugin.slot ?? 0) === ref.slot) || null : null;
  }
  function slotTarget(plugin, ch) {
    return ch.type === 'track' ? pluginTarget(plugin, ch.track) : { owner: ch.owner, kind: plugin.kind, slot: plugin.slot ?? 0, instanceID: plugin.instanceID };
  }
  const insertOut = plugin => ({ slot: plugin.slot ?? 0, name: plugin.nks?.name || plugin.name, format: plugin.format, bypass: !!plugin.bypass,
    ...(plugin.instanceID ? { instanceID: plugin.instanceID } : {}) });
  /* track or channel (not both); without either, the track under the cursor. */
  function pluginChannel(p, params, { required = false } = {}) {
    if (params.channel !== undefined && params.track !== undefined) fail('Give track or channel, not both.');
    if (params.channel !== undefined) return findChannel(p, params.channel);
    if (params.track === undefined && required) fail('Give channel ("master", a bus name or "bus:<id>", or a track) or track.');
    return trackChannel(p, params.track === undefined ? st().track : findTrack(p, params.track));
  }
  function requireStandardRouting(p, what) {
    if (!standardRouting(p)) fail(`${what} needs standard routing, and this song uses the legacy mix. enableStandardRouting turns it on (it changes `
        + 'the gain order: track faders after inserts, master gain after the routing graph); ask the user first.');
  }

  // ---------------------------------------------------------------- editing core
  function stable(value) { return JSON.stringify(value); }
  function toastText() { return (typeof document !== 'undefined' && document.getElementById?.('toast-region')?.textContent || '').trim(); }
  /* Validate on a copy, then apply the same function inside one ST.mutate (one undo step). */
  function commit(label, apply, options = {}) {
    const p = project(), draft = clone(p);
    const preview = apply(draft);
    if (stable(draft) === stable(p)) return { changed: false, ...(preview && typeof preview === 'object' ? preview : {}) };
    const history = st().undo, top = history[history.length - 1], beforeToast = toastText();
    let result, failure = null;
    // Rethrow inside mutate: it rolls back and pushes no undo step (a swallowed error left a no-op "AI: …" entry).
    try {
      S().mutate(`AI: ${label}`, target => {
        const snapshot = clone(target);
        try { result = apply(target); }
        catch (error) { failure = error; for (const key of Object.keys(target)) delete target[key]; Object.assign(target, snapshot); throw error; }
      }, options);
    } catch (error) { if (!failure) throw error; }
    if (failure) throw failure;
    const after = st().undo;
    if (after[after.length - 1] === top) {
      const notice = toastText();
      fail(`The edit was not applied${notice && notice !== beforeToast
          ? `: ${notice}`
          : notice
          ? `: ${notice}`
          : ' because the session is busy (an export, recording, routing or tempo change may be in progress). Try again in a moment.'}`);
    }
    return { changed: true, undoLabel: `AI: ${label}`, ...(result && typeof result === 'object' ? result : {}) };
  }
  function refreshUI() { try { S().refresh(); } catch { /* the next refresh repaints */ } }

  // ---------------------------------------------------------------- reading
  function describeStep(step, row) {
    const out = { row };
    if (step.note >= 0 || step.note === -1 || step.note === -2) out.note = noteName(step.note);
    if (step.note >= 0) out.midi = step.note;
    if (step.note === -3 || step.note == null) out.note = null;
    if (step.instrument !== undefined) out.instrument = step.instrument;
    // gate (rows) only for a note with one; without it the note plays until the next note, OFF or CUT on its track.
    if (step.note >= 0) { out.velocity = step.velocity ?? 100; const gate = S().stepGate(step); if (gate !== null) out.gate = gate; }
    if (Array.isArray(step.chord) && step.chord.length) {
      out.chord = step.chord.slice();
      out.notes = step.chord.map(i => noteName(clampNum(step.note + i, 0, 127)));
    }
    for (const key of ['fx1', 'fx2']) if (step[key]) {
      out[key] = formatFX(step[key]);
      if (step[key].type === 'plugin') out[key + 'Raw'] = clone(step[key]);
    }
    // Extra note columns: [{column, note, midi?, instrument?, velocity?, gate?}].
    const columns = (step.columns || []).map((cell, i) => cell && describeCell(cell, i + 1)).filter(Boolean);
    if (columns.length) out.columns = columns;
    return out;
  }
  function describeCell(cell, column) {
    const out = { column, note: noteName(cell.note) };
    if (cell.note >= 0) {
      out.midi = cell.note;
      if (cell.instrument !== undefined) out.instrument = cell.instrument;
      out.velocity = cell.velocity ?? 100;
      if (cell.gate > 0) out.gate = cell.gate;
    }
    return out;
  }
  function gridCell(step, columns = 1) {
    // Each extra note column adds "note instrument velocity" after FX 2.
    const extra = Array.from({ length: columns - 1 }, (_, i) => {
      const cell = step?.columns?.[i];
      if (!cell) return ' --- -- ---';
      const note = cell.note >= 0 ? noteName(cell.note).padEnd(3) : cell.note === -1 ? 'OFF' : 'CUT';
      return ` ${note} ${cell.note >= 0 ? String(cell.instrument ?? '--').padStart(2, '0') : '--'} ${cell.note >= 0 ? String(cell.velocity ?? 100).padStart(3) : '---'}`;
    }).join('');
    if (!step) return '--- -- --- ------- -------' + extra;
    const note = step.note >= 0 ? noteName(step.note).padEnd(3) : step.note === -1 ? 'OFF' : step.note === -2 ? 'CUT' : '···';
    const fx = f => f ? formatFX(f).replace(/^(\w{3})\w*\s*/, '$1').slice(0, 7).padEnd(7) : '-------';
    const chord = Array.isArray(step.chord) && step.chord.length > 1 ? '+' : ' ';
    return `${note}${chord}${String(step.note === -3 ? '--' : step.instrument ?? '--').padStart(2, '0')} ${step.note >= 0 ? String(step.velocity ?? 100).padStart(3) : '---'}`
        + ` ${fx(step.fx1)} ${fx(step.fx2)}` + extra;
  }
  function automationSummary(p, pat, tracks) {
    return (pat.automation || []).filter(lane => lane.track === undefined || tracks.includes(lane.track)).map(lane => ({
      track: lane.track, owner: lane.owner, target: lane.target, name: lane.name, interpolation: lane.interpolation || 'linear',
      points: (lane.points || []).map(point => ({ row: point.row, value: round(point.value, 5) }))
    }));
  }

  // ---------------------------------------------------------------- API methods
  const api = {};
  const docs = {};
  function method(name, description, keys, fn) { api[name] = fn; docs[name] = { description, params: keys }; }

  method('help', 'List methods, parameters and value conventions.', [], () => ({
    conventions: ['Tracks/instruments: 0-based index or name. Patterns: id or name (default active).', 'Rows are 0-based.', 'Notes: "C-4" (=60), "C#4", "Db3", MIDI number, "OFF", "CUT".', 'FX: "probability 50%", "ratchet 3", "pitch -12", "cutoff 800Hz", "pan -30%", "reverse", "arpeggio 37", "microtiming 25%", "volume '
        + '80%", "slice 4", "glide -12", "delay 40%", "reverb 30%", "offset 25%", "resonance 50%", "position 50%" (wavetable position).', 'Values with units: "800 Hz", "-6 dB", "20 ms", "50%", "30% L", "+7 st", "C-3".', 'Mixer channels (channel): "master", a bus name or "bus:<id>", '
        + 'or a track index/name; mixer lists them with outputs, sends and inserts.', 'Each editing call is one undo step labelled "AI: …".'],
    methods: Object.entries(docs).map(([name, info]) => ({ name, ...info }))
  }));

  method('summary', 'Song overview: tempo, meter, tracks, instruments, patterns, song sequence, mixer, performance and history.', ['_host'], params => {
    const p = project(), model = window.STSongModel, s = st();
    const rowsPerBar = Math.max(1, Math.round(p.linesPerBeat * (S().beatsPerBar ? S().beatsPerBar(p) : 4)));
    const counts = pat => pat.steps.reduce((n, steps) => n + steps.reduce((k, step) => k + noteCells(step).length, 0), 0);
    return {
      name: p.name, path: params?._host?.projectPath || null, edited: !!s.dirty,
      bpm: p.bpm,
      swing: p.swing || 0,
      timeSignature: `${p.timeSignature?.numerator || 4}/${p.timeSignature?.denominator || 4}`,
      linesPerBeat: p.linesPerBeat,
      rowsPerBar,
      masterVolume: { value: p.masterVolume, display: formatValue('db', p.masterVolume) }, loopSong: !!p.loopSong, metronome: !!p.metronome,
      activePattern: p.activePattern,
      tracks: p.tracks.map((t, i) => ({
        index: i, name: t.name, instrument: t.instrument ?? 0, instrumentName: p.instruments[t.instrument ?? 0]?.name, mute: !!t.mute, solo: !!t.solo,
        volume: formatValue('db', t.volume ?? .8),
        pan: formatValue('pan', t.pan ?? 0),
        delaySend: formatValue('pct', t.delaySend ?? 0),
        reverbSend: formatValue('pct', t.reverbSend ?? 0),
        effects: (p.audioPlugins || []).filter(plugin => onTrack(p, plugin, i)
            && plugin.kind === 'effect').sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0)).map(plugin => ({
          slot: plugin.slot ?? 0,
          name: plugin.nks?.name || plugin.name,
          format: plugin.format,
          bypass: !!plugin.bypass
        })),
        ...(t.frozen ? { frozen: true } : {})
      })),
      instruments: p.instruments.map((x, i) => ({
        index: i, name: x.name, type: x.type === 'au' ? 'plugin' : x.type,
        ...(x.type === 'sample' ? { sample: x.samplePath || null, mode: x.mode } : {}),
        ...(x.type === 'sample' && x.mode === 'wavetable' ? { wavetablePosition: formatValue('pct', x.wavetablePosition ?? 0) } : {}),
        ...(x.type === 'au' && x.plugin ? {
          plugin: {
            name: x.plugin.name,
            format: x.plugin.format,
            manufacturer: x.plugin.manufacturer,
            ...(x.plugin.nks ? { nks: x.plugin.nks.name || x.plugin.nks.path } : {})
          }
        } : {}),
        usedBy: p.tracks.map((t, ti) => (t.instrument ?? 0) === i ? ti : -1).filter(ti => ti >= 0)
      })),
      patterns: p.patterns.map((pat, i) => ({
        id: pat.id,
        number: i + 1,
        name: pat.name,
        length: pat.length,
        bars: round(pat.length / rowsPerBar, 2),
        notes: counts(pat),
        automationLanes: (pat.automation || []).length
      })),
      song: {
        sequence: p.sequence.slice(),
        blocks: (model ? model.blocks(p.sequence) : p.sequence.map((id, k) => ({ index: k, start: k, count: 1, pattern: id }))).map(b => ({
          block: b.index,
          start: b.start,
          pattern: b.pattern,
          name: p.patterns.find(x => x.id === b.pattern)?.name,
          repeats: b.count
        })),
        loop: model?.loopRange(p) || null
      },
      sendEffects: { delay: clone(p.delay), reverb: clone(p.reverb) },
      // Buses and the master channel in brief; mixer() has outputs, sends, levels and insert details.
      mixer: {
        mixModel: standardRouting(p) ? 'standard' : 'legacy',
        masterEffects: channelEffects(p, MASTER).map(plugin => plugin.nks?.name || plugin.name),
        buses: (p.routing?.buses || []).map(bus => ({ channel: `bus:${bus.id}`, name: bus.name, type: busChannel(bus).type,
          effects: channelEffects(p, busChannel(bus)).map(plugin => plugin.nks?.name || plugin.name) }))
      },
      performance: { scale: s.scale, root: SHARP[s.root || 0], chord: s.chord, chordSet: s.chordSet || null, summary: S().performanceSummary?.() },
      history: { undo: s.undo.length, redo: s.redo.length, last: s.undo[s.undo.length - 1]?.label || null }
    };
  });

  method('readPattern', 'Read a pattern: steps per track with note names, instrument, velocity, gate, chord and FX. format "grid" returns tracker text lines.', ['pattern',
      'tracks', 'from', 'to', 'format', 'includeEmpty'], params => {
    const p = project(),
      pat = findPattern(p, params.pattern),
      tracks = trackList(p, params.tracks),
      [from, to] = rowRange(pat, params.from, params.to);
    if (params.format !== undefined && !['steps', 'grid'].includes(params.format)) fail('format must be "steps" or "grid".');
    const base = { pattern: { id: pat.id, name: pat.name, length: pat.length }, rows: [from, to], automation: automationSummary(p, pat, tracks) };
    if (params.format === 'grid') {
      const header = 'row | ' + tracks.map(t => `${String(t).padStart(2, '0')} ${String(p.tracks[t].name).slice(0, 22).padEnd(22)}`).join(' | ');
      const lines = [];
      for (let r = from; r <= to; r++) lines.push(`${String(r).padStart(3, '0')} | ` + tracks.map(t => gridCell(pat.steps[t]?.[r], noteColumns(p, t))).join(' | '));
      return { ...base, legend: 'note(+ = chord) instrument velocity fx1 fx2, then note instrument velocity of each extra note column', header, lines };
    }
    return {
      ...base,
      tracks: tracks.map(t => {
        const steps = [];
        for (let r = from; r <= to; r++) {
          const step = pat.steps[t]?.[r];
          if (step) {
            steps.push(describeStep(step, r));
          } else if (params.includeEmpty) steps.push({ row: r, empty: true });
        }
        return { track: t, name: p.tracks[t].name, instrument: p.tracks[t].instrument ?? 0, ...(noteColumns(p, t) > 1 ? { noteColumns: noteColumns(p, t) } : {}), steps };
      })
    };
  });

  function normalizeChord(value, what) {
    if (value === null) return null;
    if (typeof value === 'string') {
      const chords = S().chords || {},
        key = Object.keys(chords).find(k => k.toLowerCase() === value.trim().toLowerCase()
            || chords[k].name.toLowerCase() === value.trim().toLowerCase());
      if (!key) fail(`${what}: unknown chord "${value}". Known chords: ${Object.keys(chords).join(', ')}; or give semitone intervals like [0,3,7].`);
      return chords[key].notes.slice();
    }
    if (!Array.isArray(value)
        || !value.length
        || value.length > 8
        || value.some(v => !isInt(v) || v < -48 || v > 48)) fail(`${what}: chord must be up to 8 whole semitone intervals between -48 and 48, e.g. [0,4,7].`);
    return value[0] === 0 ? value.slice() : [0, ...value.filter(v => v !== 0)].slice(0, 8);
  }
  const STEP_KEYS = ['track', 'row', 'column', 'note', 'notes', 'instrument', 'velocity', 'gate', 'chord', 'fx1', 'fx2', 'clear'];
  function planSteps(p, pat, entries) {
    if (!Array.isArray(entries) || !entries.length) fail('steps must be a non-empty array of {track, row, note, …}.');
    if (entries.length > 4096) fail('Write at most 4096 steps per call.');
    return entries.map((entry, index) => {
      const at = `steps[${index}]`;
      if (!entry || typeof entry !== 'object' || Array.isArray(entry)) fail(`${at} must be an object.`);
      for (const key of Object.keys(entry)) if (!STEP_KEYS.includes(key)) fail(`${at}: unknown field "${key}". Fields: ${STEP_KEYS.join(', ')}.`);
      const track = findTrack(p, entry.track, `${at}.track`);
      if (!isInt(entry.row)
          || entry.row < 0
          || entry.row >= pat.length) fail(`${at}.row must be 0–${pat.length - 1} in "${pat.name}" (got ${entry.row}).`);
      const plan = { track, row: entry.row };
      if (entry.column !== undefined) {
        if (!isInt(entry.column) || entry.column < 0 || entry.column > 7) fail(`${at}.column must be a note column 0–7 (0 is the main column).`);
        if (entry.column > 0) {
          plan.column = entry.column;
          for (const key of ['notes', 'chord', 'fx1', 'fx2']) if (entry[key] !== undefined) fail(`${at}: ${key} belongs to note column 0; an extra note column holds one note (the row's FX apply to it).`);
        }
      }
      if (entry.clear === true) { plan.clear = true; return plan; }
      if (entry.clear !== undefined && entry.clear !== false) fail(`${at}.clear must be true or false.`);
      if (entry.notes !== undefined) {
        if (entry.note !== undefined || entry.chord !== undefined) fail(`${at}: use either notes:[…] or note + chord, not both.`);
        if (!Array.isArray(entry.notes)
            || !entry.notes.length
            || entry.notes.length > 8) fail(`${at}.notes must list 1–8 notes, e.g. ["C-4","E-4","G-4"].`);
        const midi = entry.notes.map((n, k) => parseNote(n, `${at}.notes[${k}]`));
        if (midi.some(n => n < 0)) fail(`${at}.notes may only contain pitched notes.`);
        plan.note = midi[0]; plan.chord = midi.length > 1 ? midi.map(n => n - midi[0]) : null;
      } else if (entry.note !== undefined) plan.note = entry.note === null ? null : parseNote(entry.note, `${at}.note`);
      if (entry.chord !== undefined) plan.chord = normalizeChord(entry.chord, `${at}.chord`);
      if (entry.instrument !== undefined) {
        plan.instrument = findInstrument(p, entry.instrument, `${at}.instrument`);
        const inst = p.instruments[plan.instrument];
        if (inst?.type === 'au'
            && (p.tracks[track].instrument ?? 0) !== plan.instrument) fail(`${at}: instrument ${plan.instrument} "${inst.name}" is a plug-in on another track. Plug-in instruments only play on their own track.`);
      }
      if (entry.velocity !== undefined) {
        if (!isInt(entry.velocity) || entry.velocity < 1 || entry.velocity > 127) fail(`${at}.velocity must be a whole number 1–127.`);
        plan.velocity = entry.velocity;
      }
      if (entry.gate !== undefined) {
        const g = entry.gate === null ? 0 : Number(entry.gate);
        if (!Number.isFinite(g) || g !== 0 && (g < .01 || g > 256)) fail(`${at}.gate must be 0.01–256 rows, or 0/null for no gate (the note plays until the next note, OFF or CUT).`);
        plan.gate = round(g, 3);
      }
      for (const key of ['fx1', 'fx2']) if (entry[key] !== undefined) plan[key] = parseFX(entry[key], `${at}.${key}`);
      if (!Object.keys(plan).some(key => !['track', 'row'].includes(key))) fail(`${at}: nothing to write. Give note/notes, velocity, gate, chord, fx1/fx2 or clear:true.`);
      return plan;
    });
  }
  // An extra note column: {note, instrument, velocity, gate}. The track grows to show it (same undo step).
  function applyColumn(p, pat, plan) {
    const steps = pat.steps[plan.track], prior = steps[plan.row], column = plan.column, existing = prior?.columns?.[column - 1] || null;
    const write = cell => {
      const next = { ...(prior || { note: -3 }), columns: [...(prior?.columns || [])] };
      while (next.columns.length < column) next.columns.push(null);
      next.columns[column - 1] = cell;
      while (next.columns.length && !next.columns.at(-1)) next.columns.pop();
      if (!next.columns.length) delete next.columns;
      steps[plan.row] = dropEmpty(next);
    };
    if (plan.clear || plan.note === null) return write(null);
    if (!existing && plan.note === undefined) fail(`Track ${plan.track} row ${plan.row} column ${column} is empty: give a note to create it.`);
    // Only a written cell adds its column to the track; clearing a column the track does not have changes nothing.
    if (p.tracks[plan.track] && noteColumns(p, plan.track) <= column) p.tracks[plan.track].noteColumns = column + 1;
    const cell = { ...(existing || { instrument: p.tracks[plan.track].instrument ?? 0, velocity: 100, gate: 0 }) };
    if (plan.note !== undefined) cell.note = plan.note;
    if (plan.instrument !== undefined) cell.instrument = plan.instrument;
    if (plan.velocity !== undefined) cell.velocity = plan.velocity;
    if (plan.gate !== undefined) cell.gate = plan.gate;
    if (!(cell.note >= 0)) { delete cell.instrument; delete cell.velocity; delete cell.gate; }
    write(cell);
  }
  function applyStep(p, pat, plan) {
    const steps = pat.steps[plan.track];
    if (plan.column) return applyColumn(p, pat, plan);
    // Clearing column 0 keeps the row's extra note columns.
    const keep = steps[plan.row]?.columns ? { note: -3, columns: steps[plan.row].columns } : null;
    if (plan.clear
        || (plan.note === null && plan.fx1 === undefined && plan.fx2 === undefined && !steps[plan.row]?.fx1 && !steps[plan.row]?.fx2)) { steps[plan.row] = dropEmpty(keep); return; }
    let step = steps[plan.row] ? { ...steps[plan.row] } : null;
    const creating = !step;
    // A new note has no gate (it plays until the next note, OFF or CUT on its track) unless the entry gives one.
    if (!step) step = { note: -3, instrument: p.tracks[plan.track].instrument ?? 0, velocity: 100, gate: 0 };
    if (plan.note !== undefined) step.note = plan.note === null ? -3 : plan.note;
    if (creating
        && plan.note === undefined
        && plan.fx1 === undefined
        && plan.fx2 === undefined) fail(`Track ${plan.track} row ${plan.row} is empty: give a note (or an FX) to create a step.`);
    if (plan.instrument !== undefined) step.instrument = plan.instrument;
    if (plan.velocity !== undefined) step.velocity = plan.velocity;
    if (plan.gate !== undefined) step.gate = plan.gate;
    if (plan.chord !== undefined) { if (plan.chord && plan.chord.length > 1) step.chord = plan.chord; else delete step.chord; }
    if (!(step.note >= 0)) delete step.chord;
    for (const key of ['fx1', 'fx2']) if (plan[key] !== undefined) { if (plan[key]) step[key] = plan[key]; else delete step[key]; }
    steps[plan.row] = dropEmpty(step);
  }
  method('writeSteps', 'Set or clear steps. Each entry: {track, row, column?, note|notes, instrument?, velocity?, gate?, chord?, fx1?, fx2?, clear?}. Existing steps are '
      + 'merged; note:null keeps FX only. note "OFF" releases the track\'s notes, "CUT" stops them. gate is optional: without one a note plays until the '
      + 'next note, OFF or CUT on its track (classic tracker); gate N (0.01–256 rows) ends it after N rows; gate 0 or null removes a gate. column (0-based, '
      + 'default 0) picks a note column: a track can play up to 8 notes per row, each column its own voice (a new note, OFF or CUT ends only its own '
      + 'column\'s note). Column 1–7 holds note/instrument/velocity/gate only; the row\'s fx1/fx2 (column 0) apply to it. Writing a column adds it to the '
      + 'track. clear:true clears that column (column 0: its note and FX; other columns stay).', ['pattern', 'steps', 'label'], params => {
    const p = project(), pat = findPattern(p, params.pattern), plans = planSteps(p, pat, params.steps);
    requireUnfrozen(p, [...new Set(plans.map(plan => plan.track))]);
    const id = pat.id;
    // note: null (FX only / clear the note) is not a note; null >= 0 is true.
    const notes = plans.filter(plan => isInt(plan.note) && plan.note >= 0).length, cleared = plans.filter(plan => plan.clear).length;
    return commit(params.label || `write ${plans.length} step${plans.length === 1 ? '' : 's'} in ${pat.name}`, target => {
      const t = target.patterns.find(x => x.id === id);
      for (const plan of plans) applyStep(target, t, plan);
      return { pattern: { id, name: t.name }, written: plans.length - cleared, notes, cleared };
    });
  });

  method('clearSteps', 'Clear a rectangular range of steps, every note column included (all tracks by default).', ['pattern', 'tracks', 'from', 'to', 'label'], params => {
    const p = project(),
      pat = findPattern(p, params.pattern),
      tracks = trackList(p, params.tracks),
      [from, to] = rowRange(pat, params.from, params.to),
      id = pat.id;
    requireUnfrozen(p, tracks);
    return commit(params.label || `clear rows ${from}–${to} in ${pat.name}`, target => {
      const t = target.patterns.find(x => x.id === id); let count = 0;
      for (const track of tracks) for (let r = from; r <= to; r++) if (t.steps[track][r]) { t.steps[track][r] = null; count++; }
      return { cleared: count };
    });
  });

  function shiftRows(params, insert) {
    const p = project(), pat = findPattern(p, params.pattern), tracks = trackList(p, params.tracks), id = pat.id;
    const count = params.count === undefined ? 1 : params.count;
    if (!isInt(params.row) || params.row < 0 || params.row >= pat.length) fail(`row must be 0–${pat.length - 1}.`);
    if (!isInt(count) || count < 1 || count > pat.length) fail(`count must be 1–${pat.length}.`);
    requireUnfrozen(p, tracks);
    return commit(params.label || `${insert ? 'insert' : 'delete'} ${count} row${count === 1 ? '' : 's'} at ${params.row} in ${pat.name}`, target => {
      const t = target.patterns.find(x => x.id === id), L = t.length;
      for (const track of tracks) {
        const steps = t.steps[track];
        if (insert) { steps.splice(params.row, 0, ...Array(count).fill(null)); steps.length = L; }
        else { steps.splice(params.row, Math.min(count, L - params.row)); while (steps.length < L) steps.push(null); }
      }
      for (const lane of t.automation || []) {
        if (lane.track === undefined || !tracks.includes(lane.track)) continue;
        lane.points = lane.points.map(point => point.row < params.row ? point : insert ? {
          ...point,
          row: point.row + count
        } : point.row < params.row + count ? null : { ...point, row: point.row - count }).filter(point => point && point.row < L);
      }
      return { pattern: id, rows: count };
    });
  }
  method('insertRows', 'Insert empty rows at a row (content below moves down; rows past the end drop off).',
      ['pattern', 'row', 'count', 'tracks', 'label'], params => shiftRows(params, true));
  method('deleteRows', 'Delete rows at a row (content below moves up; empty rows fill the end).', ['pattern', 'row', 'count', 'tracks', 'label'],
      params => shiftRows(params, false));

  const TRANSFORMS = ['transpose', 'humanize', 'velocity', 'quantize', 'snapToScale', 'reverse', 'rotate', 'gate', 'instrument', 'removeFX'];
  method('transform', `Transform notes in a range: ${TRANSFORMS.join(', ')}. amount: transpose semitones / humanize ±velocity (default 12) / rotate rows; velocity `
      + `{scale|set}; quantize removes microtiming; snapToScale {scale, root}; gate {set: rows, or 0 to remove gates}; instrument {set}.`, ['pattern', 'tracks', 'from', 'to',
          'operation', 'amount', 'scale', 'root', 'set', 'label'], params => {
    const p = project(),
      pat = findPattern(p, params.pattern),
      tracks = trackList(p, params.tracks),
      [from, to] = rowRange(pat, params.from, params.to),
      id = pat.id,
      op = params.operation;
    if (!TRANSFORMS.includes(op)) fail(`operation must be one of ${TRANSFORMS.join(', ')}.`);
    requireUnfrozen(p, tracks);
    const amount = params.amount;
    let scaleSet = null, rootPc = 0, setValue;
    if (op === 'transpose' || op === 'rotate') {
      if (!isInt(amount) || amount === 0 || Math.abs(amount) > 127) fail(`${op} needs a whole, non-zero amount.`);
    }
    if (op === 'humanize'
        && amount !== undefined
        && (!Number.isFinite(amount) || amount <= 0 || amount > 64)) fail('humanize amount is the maximum velocity change, 1–64.');
    if (op === 'velocity') {
      const scale = params.scale, set = params.set;
      if ((scale === undefined) === (set === undefined)) fail('velocity needs either scale (e.g. 1.1) or set (1–127).');
      if (scale !== undefined && (!Number.isFinite(scale) || scale <= 0 || scale > 8)) fail('velocity scale must be between 0 and 8.');
      if (set !== undefined && (!isInt(set) || set < 1 || set > 127)) fail('velocity set must be 1–127.');
    }
    if (op === 'snapToScale') {
      const scales = S().scales || {}; const key = Object.keys(scales).find(k => k.toLowerCase() === String(params.scale || '').toLowerCase());
      if (!key) fail(`snapToScale needs scale: one of ${Object.keys(scales).join(', ')}.`);
      scaleSet = new Set(scales[key]); rootPc = params.root === undefined ? 0 : pitchClass(params.root);
    }
    if (op === 'gate') {
      setValue = Number(params.set);
      if (!Number.isFinite(setValue) || setValue !== 0 && (setValue < .01 || setValue > 256)) fail('gate set must be 0.01–256 rows, or 0 to remove gates.');
    }
    if (op === 'instrument') {
      setValue = findInstrument(p, params.set, 'set');
      // As writeSteps and the grid's instrument column: a plug-in instrument only plays on its own track.
      const foreign = p.instruments[setValue].type === 'au' ? tracks.find(t => (p.tracks[t].instrument ?? 0) !== setValue) : undefined;
      if (foreign !== undefined) fail(`Instrument ${setValue} "${p.instruments[setValue].name}" is a plug-in that track ${foreign} "${p.tracks[foreign].name}" does not play. `
          + 'Plug-in instruments only play on their own track; limit tracks to it.');
    }
    const humanize = amount ?? 12;
    return commit(params.label || `${op} ${pat.name} rows ${from}–${to}`, target => {
      // `affected` counts the steps / notes the operation visited; commit() reports `changed` (false for a no-op).
      const t = target.patterns.find(x => x.id === id); let affected = 0;
      for (const track of tracks) {
        const steps = t.steps[track];
        if (op === 'reverse' || op === 'rotate') {
          const slice = steps.slice(from, to + 1), n = slice.length;
          const result = op === 'reverse' ? slice.reverse() : slice.map((_, i) => slice[((i - amount) % n + n) % n]);
          for (let i = 0; i < n; i++) steps[from + i] = result[i]; affected += slice.filter(Boolean).length; continue;
        }
        for (let r = from; r <= to; r++) {
          const step = steps[r]; if (!step) continue;
          if (op === 'quantize') { for (const key of ['fx1', 'fx2']) if (step[key]?.type === 'microtiming') { delete step[key]; affected++; } steps[r] = dropEmpty(step); continue; }
          if (op === 'removeFX') { if (step.fx1 || step.fx2) { delete step.fx1; delete step.fx2; affected++; steps[r] = dropEmpty(step); } continue; }
          // Every note column's note.
          for (const cell of noteCells(step)) {
            if (op === 'transpose') cell.note = clampNum(cell.note + amount, 0, 127);
            if (op === 'humanize') cell.velocity = clampNum(Math.round((cell.velocity ?? 100) + (Math.random() * 2 - 1) * humanize), 1, 127);
            if (op === 'velocity') cell.velocity = params.set !== undefined
                ? params.set
                : clampNum(Math.round((cell.velocity ?? 100) * params.scale), 1, 127);
            if (op === 'gate') cell.gate = round(setValue, 3);
            if (op === 'instrument') cell.instrument = setValue;
            if (op === 'snapToScale') {
              let n = cell.note;
              for (let k = 0; k < 12 && !scaleSet.has(((n - rootPc) % 12 + 12) % 12); k++) n = n + 1 <= 127 ? n + 1 : n - 1;
              cell.note = n;
            }
            affected++;
          }
        }
      }
      return { operation: op, affected };
    });
  });
  function pitchClass(value) {
    if (isInt(value) && value >= 0 && value < 12) return value;
    if (typeof value === 'string') { const m = /^([a-g])([#♯b♭]?)$/i.exec(value.trim()); if (m) return ((PITCH[m[1].toLowerCase()] + (m[2] === '#' || m[2] === '♯'
        ? 1
        : m[2]
        ? -1
        : 0)) % 12 + 12) % 12; }
    fail(`root must be a pitch class like "C", "F#", "Bb" or 0–11 (got ${value}).`);
  }

  // ---------------------------------------------------------------- patterns
  function freePatternId(p) { for (let id = 0; id < LIMITS.patterns; id++) if (!p.patterns.some(x => x.id === id)) return id; return -1; }
  function checkLength(length, what = 'length') {
    if (!isInt(length) || length < 1 || length > LIMITS.length) fail(`${what} must be 1–256 rows.`);
    return length;
  }
  // appendToSong pushes one slot; keep the 1024-slot song limit the host validates (ProjectStore refuses to save past it).
  function checkSongRoom(p, append) {
    if (append && p.sequence.length >= LIMITS.songSlots) fail(`The song supports up to ${LIMITS.songSlots} sequence slots. Remove a block before appending.`);
  }
  function checkName(name, what = 'name') {
    if (typeof name !== 'string' || !name.trim() || name.length > 120) fail(`${what} must be a non-empty string up to 120 characters.`);
    return name.trim();
  }
  method('createPattern', 'Create a new empty pattern (optionally appended to the song). Selects it unless select:false.', ['name', 'length',
      'appendToSong', 'select', 'label'], params => {
    const p = project(); if (p.patterns.length >= LIMITS.patterns) fail('This session already has 256 patterns.');
    checkSongRoom(p, params.appendToSong);
    const length = params.length === undefined ? activePattern(p).length : checkLength(params.length),
      name = params.name === undefined ? null : checkName(params.name);
    return commit(params.label || `create pattern${name ? ' ' + name : ''}`, target => {
      const id = freePatternId(target);
      target.patterns.push({
        id,
        name: name || 'Pattern ' + String(id + 1).padStart(2, '0'),
        length,
        steps: target.tracks.map(() => Array(length).fill(null)),
        automation: []
      });
      if (params.appendToSong) target.sequence.push(id);
      if (params.select !== false) target.activePattern = id;
      return { pattern: { id, name: name || 'Pattern ' + String(id + 1).padStart(2, '0'), length } };
    });
  });
  method('duplicatePattern', 'Copy a pattern (steps and automation) to a new pattern.', ['pattern', 'name', 'appendToSong', 'select',
      'label'], params => {
    const p = project(),
      pat = findPattern(p, params.pattern);
    if (p.patterns.length >= LIMITS.patterns) fail('This session already has 256 patterns.');
    checkSongRoom(p, params.appendToSong);
    const name = params.name === undefined ? `${pat.name} copy` : checkName(params.name), source = pat.id;
    return commit(params.label || `duplicate ${pat.name}`, target => {
      const id = freePatternId(target), copy = clone(target.patterns.find(x => x.id === source));
      copy.id = id; copy.name = name; for (const lane of copy.automation || []) lane.id = S().uid();
      target.patterns.push(copy);
      if (params.appendToSong) target.sequence.push(id);
      if (params.select !== false) target.activePattern = id;
      return { pattern: { id, name, length: copy.length } };
    });
  });
  method('renamePattern', 'Rename a pattern.', ['pattern', 'name'], params => {
    const p = project(), pat = findPattern(p, params.pattern), name = checkName(params.name), id = pat.id;
    return commit(`rename pattern to ${name}`, target => { target.patterns.find(x => x.id === id).name = name; return { pattern: { id, name } }; });
  });
  method('deletePattern', 'Delete a pattern and remove it from the song sequence.', ['pattern', 'label'], params => {
    const p = project(), pat = findPattern(p, params.pattern), id = pat.id;
    if (p.patterns.length === 1) fail('The session needs at least one pattern.');
    if (p.sequence.every(x => x === id)) fail(`"${pat.name}" is the only pattern in the song. Add another pattern to the song first.`);
    return commit(params.label || `delete pattern ${pat.name}`, target => {
      target.patterns = target.patterns.filter(x => x.id !== id);
      const model = window.STSongModel;
      if (model) { while (target.sequence.includes(id)) { const block = model.blocks(target.sequence).find(b => b.pattern === id); model.remove(target, block.index); } }
      else target.sequence = target.sequence.filter(x => x !== id);
      if (target.activePattern === id) target.activePattern = target.patterns[0].id;
      return { deleted: id };
    });
  });
  method('setPatternLength', 'Set a pattern length in rows (1–256). Shortening drops steps and automation past the end.', ['pattern', 'length', 'label'], params => {
    const p = project(), pat = findPattern(p, params.pattern), length = checkLength(params.length), id = pat.id;
    return commit(params.label || `set ${pat.name} length ${length}`, target => {
      const t = target.patterns.find(x => x.id === id);
      t.length = length; t.steps.forEach(steps => { steps.length = length; for (let r = 0; r < length; r++) steps[r] = steps[r] || null; });
      (t.automation || []).forEach(lane => { lane.points = lane.points.filter(point => point.row < length); });
      return { pattern: { id, length } };
    });
  });

  // ---------------------------------------------------------------- song sequence
  const SONG_ACTIONS = ['set', 'insert', 'remove', 'repeats', 'replace', 'move', 'duplicate', 'loop', 'clearLoop'];
  method('editSong', `Edit the song sequence (blocks = consecutive repeats of one pattern). action: ${SONG_ACTIONS.join(', ')}. set {sequence:[id|{pattern,repeats}]}; `
      + `insert {pattern, block?, before?, repeats?}; remove {block, toBlock?}; repeats {block, repeats}; replace {block, pattern}; move {block, `
      + `toBlock?, delta}; duplicate {block, toBlock?}; loop {block, toBlock?}; clearLoop.`, ['action', 'sequence', 'pattern', 'block', 'toBlock',
          'before', 'repeats', 'delta', 'label'], params => {
    const p = project(), model = window.STSongModel; if (!model) fail('The song model is unavailable.');
    const action = params.action; if (!SONG_ACTIONS.includes(action)) fail(`action must be one of ${SONG_ACTIONS.join(', ')}.`);
    const blocks = model.blocks(p.sequence);
    const blockIndex = (value, what = 'block') => {
      if (!isInt(value)
          || value < 0
          || value >= blocks.length) fail(`${what} must be a block index 0–${blocks.length - 1}. Current blocks: ${blocks.map(b => `${b.index}: ${p.patterns.find(x => x.id === b.pattern)?.name} ×${b.count}`).join(', ')}.`);
      return value;
    };
    const repeats = value => {
      if (value === undefined) return 1;
      if (!isInt(value) || value < 1 || value > model.MAX_REPEATS) fail(`repeats must be 1–${model.MAX_REPEATS}.`);
      return value;
    };
    let sequence = null, patternId = null, a = null, b = null, count = null;
    if (action === 'set') {
      if (!Array.isArray(params.sequence) || !params.sequence.length) fail('set needs sequence: a list of pattern ids/names or {pattern, repeats}.');
      sequence = [];
      for (const item of params.sequence) {
        const entry = item && typeof item === 'object' ? item : { pattern: item };
        const pid = findPattern(p, entry.pattern).id;
        for (let k = 0; k < repeats(entry.repeats); k++) sequence.push(pid);
      }
      if (sequence.length > 1024) fail('The song supports up to 1024 sequence slots.');
    }
    if (['insert', 'replace'].includes(action)) patternId = findPattern(p, params.pattern).id;
    if (['remove', 'repeats', 'replace', 'move', 'duplicate', 'loop'].includes(action)) a = blockIndex(params.block);
    if (action === 'insert' && params.block !== undefined) a = blockIndex(params.block);
    if (['remove', 'move', 'duplicate', 'loop'].includes(action)) b = params.toBlock === undefined ? a : blockIndex(params.toBlock, 'toBlock');
    if (action === 'repeats') count = repeats(params.repeats);
    if (action === 'insert') count = repeats(params.repeats);
    if (action === 'move' && (params.delta !== -1 && params.delta !== 1)) fail('move needs delta -1 (earlier) or 1 (later).');
    return commit(params.label || `song ${action}`, target => {
      // The song model refuses with plain errors (slot limit, last block): they are answers, not internal errors.
      try { switch (action) {
        case 'set': target.sequence = sequence.slice(); if (target.songLoop) model.clearLoop(target); break;
        // Appending goes through the model too: it enforces the 1024-slot song limit the host validates.
        case 'insert': model.insert(target, a === null ? blocks.length - 1 : a, patternId, { before: a !== null && !!params.before, count }); break;
        case 'remove': model.remove(target, a, b); break;
        case 'repeats': model.setRepeats(target, a, count); break;
        case 'replace': model.setPattern(target, a, patternId); break;
        case 'move': if (model.move(target, a, b, params.delta) < 0) fail('Those blocks cannot move further in that direction.'); break;
        case 'duplicate': model.duplicate(target, a, b); break;
        case 'loop': model.setLoop(target, a, b); break;
        case 'clearLoop': model.clearLoop(target); break;
      } } catch (error) { if (error instanceof AgentError || error?.name !== 'Error') throw error; fail(error.message); }
      return { sequence: target.sequence.slice(), blocks: model.blocks(target.sequence).map(x => ({ block: x.index, pattern: x.pattern, repeats: x.count })), loop: model.loopRange(target) };
    });
  });

  // ---------------------------------------------------------------- tracks and song settings
  method('addTrack', 'Add a track with an optional name and instrument. Sessions have no fixed track limit.', ['name', 'instrument', 'label'], params => {
    const p = project(); if (p.tracks.length >= LIMITS.tracks) fail(`This session already has ${LIMITS.tracks} tracks.`);
    const name = params.name === undefined ? null : checkName(params.name),
      instrument = params.instrument === undefined ? null : findInstrument(p, params.instrument);
    if (instrument !== null
        && p.instruments[instrument].type === 'au') fail('Plug-in instruments are bound to their track. Load the plug-in on the new track with loadPlugin instead.');
    return commit(params.label || `add track${name ? ' ' + name : ''}`, target => {
      const i = target.tracks.length;
      const track = {
        id: i,
        routingID: S().uid(),
        name: name || 'Track ' + String(i + 1).padStart(2, '0'),
        color: TRACK_COLORS[i % 8],
        volume: .8,
        pan: 0,
        mute: false,
        solo: false,
        instrument: instrument ?? 0,
        delaySend: 0,
        reverbSend: 0,
        plugins: []
      };
      if (window.STRoutingProject?.appendTrack) window.STRoutingProject.appendTrack(target, track, S().uid); else target.tracks.push(track);
      target.patterns.forEach(pat => pat.steps.push(Array(pat.length).fill(null)));
      return { track: i, name: track.name };
    });
  });
  method('editTrack', 'Change a track: name, mute, solo, volume ("-6 dB" or 0–1.5), pan ("30% L" or -1…1), delaySend/reverbSend ("40%" or 0–1), instrument.', ['track',
      'name', 'mute', 'solo', 'volume', 'pan', 'delaySend', 'reverbSend', 'instrument', 'label'], params => {
    const p = project(), t = findTrack(p, params.track), changes = {};
    if (params.name !== undefined) changes.name = checkName(params.name);
    for (const key of ['mute', 'solo']) if (params[key] !== undefined) {
      if (typeof params[key] !== 'boolean') fail(`${key} must be true or false.`);
      changes[key] = params[key];
    }
    for (const key of Object.keys(TRACK_VALUES)) if (params[key] !== undefined) changes[key] = parseValue(TRACK_VALUES[key], params[key], key);
    if (params.instrument !== undefined) {
      const i = findInstrument(p, params.instrument);
      if (p.instruments[i].type === 'au'
          && (p.tracks[t].instrument ?? 0) !== i) fail(`"${p.instruments[i].name}" is a plug-in instrument bound to another track. Use loadPlugin on track ${t} instead.`);
      if (p.instruments[p.tracks[t].instrument ?? 0]?.type === 'au'
          && pluginOn(p, t, 'instrument')
          && (p.tracks[t].instrument ?? 0) !== i) fail(`Track ${t} plays a plug-in instrument. Switch it with runCommand "assignInstrument" (select the track and instrument with show) `
          + `so the plug-in is removed safely.`);
      changes.instrument = i;
    }
    if (!Object.keys(changes).length) fail('Nothing to change. Give name, mute, solo, volume, pan, delaySend, reverbSend or instrument.');
    if (Object.keys(changes).some(key => !['mute', 'solo'].includes(key))) requireUnfrozen(p, [t]);
    return commit(params.label || `edit track ${p.tracks[t].name}`, target => {
      Object.assign(target.tracks[t], changes);
      const track = target.tracks[t];
      return { track: t, name: track.name, mute: !!track.mute, solo: !!track.solo, volume: formatValue('db', track.volume), pan: formatValue('pan', track.pan), delaySend: formatValue('pct',
          track.delaySend ?? 0), reverbSend: formatValue('pct',
          track.reverbSend ?? 0), instrument: track.instrument };
    });
  });
  function parseMeter(value) {
    let numerator, denominator;
    if (typeof value === 'string') { const m = /^\s*(\d+)\s*\/\s*(\d+)\s*$/.exec(value); if (!m) fail('timeSignature must look like "3/4".'); numerator = Number(m[1]); denominator = Number(m[2]); }
    else if (value && typeof value === 'object') ({ numerator, denominator } = value);
    if (!isInt(numerator)
        || numerator < 1
        || numerator > 16
        || ![2, 4, 8, 16].includes(denominator)) fail('timeSignature needs 1–16 beats over 2, 4, 8 or 16.');
    return { numerator, denominator };
  }
  method('setSong', 'Song settings: name, bpm (20–400), swing ("10%" or 0–0.75), timeSignature ("3/4"), linesPerBeat (1,2,3,4,6,8,12,16), masterVolume, loopSong, '
      + 'metronome, metronomeVolume.', ['name', 'bpm', 'swing', 'timeSignature', 'linesPerBeat', 'masterVolume', 'loopSong', 'metronome',
          'metronomeVolume', 'label'], async params => {
    const p = project(), changes = {};
    if (params.name !== undefined) changes.name = checkName(params.name);
    for (const key of ['bpm', 'swing', 'masterVolume',
        'metronomeVolume']) if (params[key] !== undefined) changes[key] = parseValue(SONG_VALUES[key], params[key], key);
    if (params.timeSignature !== undefined) changes.timeSignature = parseMeter(params.timeSignature);
    if (params.linesPerBeat !== undefined) {
      if (![1, 2, 3, 4, 6, 8, 12, 16].includes(params.linesPerBeat)) fail('linesPerBeat must be 1, 2, 3, 4, 6, 8, 12 or 16.');
      changes.linesPerBeat = params.linesPerBeat;
    }
    for (const key of ['loopSong', 'metronome']) if (params[key] !== undefined) {
      if (typeof params[key] !== 'boolean') fail(`${key} must be true or false.`);
      changes[key] = params[key];
    }
    if (!Object.keys(changes).length) fail('Nothing to change.');
    const result = commit(params.label || `song settings (${Object.keys(changes).join(', ')})`, target => {
      Object.assign(target, changes);
      return {};
    });
    const q = project();
    return { ...result, bpm: q.bpm, swing: q.swing || 0, timeSignature: `${q.timeSignature?.numerator || 4}/${q.timeSignature?.denominator || 4}`, linesPerBeat: q.linesPerBeat, name: q.name };
  });

  // ---------------------------------------------------------------- instruments
  function describeInstrument(p, i) {
    const inst = p.instruments[i], params = {};
    for (const [key, spec] of Object.entries(PARAMS)) {
      if (spec.types && !spec.types.includes(inst.type)) continue;
      if (inst.type === 'au' || (inst.type === 'midi' && !spec.types)) continue;
      const value = inst[key];
      if (value === undefined) continue;
      params[key] = spec.options
          ? { value, options: spec.options }
          : spec.bool
          ? { value: !!value }
          : { value: round(value, 6), display: formatValue(spec.kind, value), range: [spec.min, spec.max] };
    }
    return {
      index: i,
      name: inst.name,
      type: inst.type === 'au' ? 'plugin' : inst.type,
      usedBy: p.tracks.map((t, ti) => (t.instrument ?? 0) === i ? ti : -1).filter(ti => ti >= 0),
      ...(inst.type === 'sample' ? { sample: inst.samplePath || null } : {}),
      ...(inst.type === 'au' && inst.plugin ? {
        plugin: {
          name: inst.plugin.name,
          format: inst.plugin.format,
          manufacturer: inst.plugin.manufacturer,
          nks: inst.plugin.nks ? { name: inst.plugin.nks.name, path: inst.plugin.nks.path } : null
        },
        hint: 'Use pluginParameters/setPluginParameters on the track that plays it.'
      } : {}),
      params
    };
  }
  function planParams(inst, values, what = 'params') {
    if (values === undefined) return {};
    if (!values
        || typeof values !== 'object'
        || Array.isArray(values)) fail(`${what} must be an object like {"cutoff": "800 Hz", "attack": "5 ms"}.`);
    if (inst.type === 'au') fail(`"${inst.name}" is a plug-in instrument. Use setPluginParameters for its controls.`);
    const plan = {};
    for (const [name, raw] of Object.entries(values)) {
      const key = PARAM_ALIAS[name.toLowerCase()] || (name === 'tuneSemi' || name === 'tuneFine' ? name : null);
      if (!key) fail(`Unknown instrument parameter "${name}". Parameters: ${Object.keys(PARAMS).join(', ')}.`);
      if (key === 'tuneSemi' || key === 'tuneFine') { plan[key] = key === 'tuneSemi'
          ? parseValue({ min: -48, max: 48, kind: 'int' }, raw, key)
          : parseValue({ min: -99, max: 99, kind: 'int' }, raw, key); continue; }
      const spec = PARAMS[key];
      if (spec.options) { const v = String(raw).toLowerCase().replace(/[\s-]/g, ''); const option = spec.options.find(o => o === v
          || (key === 'mode' && v === '1shot' && o === 'oneshot')); if (!option) fail(`${key} must be one of ${spec.options.join(', ')}.`); plan[key] = option; continue; }
      if (spec.bool) { if (typeof raw !== 'boolean') fail(`${key} must be true or false.`); plan[key] = raw; continue; }
      plan[key] = parseValue(spec, raw, key);
    }
    return plan;
  }
  function applyParams(inst, plan) {
    for (const [key, value] of Object.entries(plan)) {
      if (key === 'tuneSemi' || key === 'tuneFine') {
        const tune = clampNum(Number(inst.tune) || 0, -48, 48); let semis = Math.trunc(tune), cents = Math.round((tune - semis) * 100);
        if (key === 'tuneSemi') semis = value; else cents = value;
        inst.tune = clampNum(semis + cents / 100, -48, 48); continue;
      }
      inst[key] = value;
    }
    if (plan.start !== undefined || plan.end !== undefined) { if (inst.start >= inst.end) fail('Sample start must be before sample end.'); }
    if (plan.loopStart !== undefined || plan.loopEnd !== undefined) {
      if (inst.loopStart >= inst.loopEnd) fail('Loop start must be before loop end.');
    }
  }
  method('getInstrument', 'Read an instrument with all parameters in readable units and ranges.', ['instrument'], params => {
    const p = project(); return describeInstrument(p, params.instrument === undefined ? st().instrument : findInstrument(p, params.instrument));
  });
  method('createInstrument', 'Create a sampler (default) or external MIDI instrument; optionally set params and assign it to a track. A sampler with '
      + 'params {"mode":"wavetable"} and no sample plays the built-in Saw wavetable (load another table with loadSample "builtin:wavetables/Morph/Vowels.wav").', ['type', 'name',
      'params', 'track', 'label'], params => {
    const p = project();
    const type = params.type === undefined ? 'sample' : params.type;
    if (!['sample', 'midi'].includes(type)) fail('type must be "sample" or "midi". For plug-ins use loadPlugin or loadNKS; for a synth use a sampler in Wavetable mode.');
    const name = params.name === undefined ? null : checkName(params.name), track = params.track === undefined ? null : findTrack(p, params.track);
    if (track !== null) {
      requireUnfrozen(p, [track]);
      if (pluginOn(p, track, 'instrument')) fail(`Track ${track} plays a plug-in instrument; assign the new instrument with runCommand "assignInstrument" after selecting it, or choose another track.`);
    }
    const probe = S().normalize({ instruments: [{ id: 0, type, name: 'probe' }] }).instruments[0];
    const plan = planParams(probe, params.params);
    return commit(params.label || `create ${type} instrument${name ? ' ' + name : ''}`, target => {
      const id = target.instruments.length;
      const inst = S().normalize({ instruments: [{ id, type, name: name || 'Instrument ' + String(id).padStart(2, '0') }] }).instruments[0];
      inst.id = id; applyParams(inst, plan);
      if (inst.type === 'sample' && inst.mode === 'wavetable' && !inst.samplePath) inst.samplePath = S().wavetablePath?.('Saw') || 'builtin:wavetables/Basic/Saw.wav';
      target.instruments.push(inst);
      if (track !== null) target.tracks[track].instrument = id;
      return { instrument: id, name: inst.name, ...(track !== null ? { assignedTo: track } : {}) };
    });
  });
  method('editInstrument', 'Rename an instrument or set parameters by name with units, e.g. {"cutoff":"800 Hz","attack":"5 ms","wavetablePosition":"25%","volume":"-6 dB"}. '
      + 'type may switch between sample/midi.', ['instrument', 'name', 'type', 'params', 'label'], async params => {
    const p = project(), i = params.instrument === undefined ? st().instrument : findInstrument(p, params.instrument), inst = p.instruments[i];
    const name = params.name === undefined ? null : checkName(params.name);
    if (params.type !== undefined && !['sample', 'midi'].includes(params.type)) fail('type must be "sample" or "midi".');
    // Validate the parameters before the type switch below: it retires the hosted plug-in and must not run for a call that is then refused.
    const plan = planParams({ ...inst, type: params.type || inst.type }, params.params);
    requireUnfrozen(p, p.tracks.map((t, ti) => (t.instrument ?? 0) === i ? ti : -1).filter(ti => ti >= 0));
    if (params.type !== undefined && params.type !== inst.type && inst.type === 'au') {
      // Leaving a plug-in instrument removes the hosted plug-in natively; use the app's own command.
      st().instrument = i; await S().perform('instrumentType:' + params.type);
      if (project().instruments[i]?.type !== params.type) fail(`Could not change the instrument type${toastText() ? ': ' + toastText() : '.'}`);
    }
    const current = project().instruments[i];
    if (name === null
        && !Object.keys(plan).length
        && (params.type === undefined || params.type === current.type)) return { changed: params.type !== undefined
            && params.type !== inst.type, instrument: describeInstrument(project(), i) };
    const result = commit(params.label
        || `edit instrument ${current.name}${Object.keys(plan).length ? ' (' + Object.keys(plan).join(', ') + ')' : ''}`, target => {
      const x = target.instruments[i]; if (name) x.name = name; if (params.type && x.type !== 'au') { x.type = params.type; x.plugin = null; }
      applyParams(x, plan); return {};
    });
    return { ...result, instrument: describeInstrument(project(), i) };
  });

  // ---------------------------------------------------------------- library, samples, plug-ins, NKS
  const slimItem = item => {
    const out = {};
    for (const [key, value] of Object.entries(item || {})) if (value !== null
        && value !== undefined
        && typeof value !== 'object'
        && String(value).length < 400) {
      out[key] = value;
    } else if (Array.isArray(value) && value.length <= 32 && value.every(v => typeof v === 'string')) out[key] = value;
    return out;
  };
  method('searchLibrary', 'Search the sample/NKS library. kind: "sample" (default), "nks" or "all".', ['query', 'kind', 'limit', 'offset',
      'favorites'], async params => {
    const kind = params.kind === undefined ? 'sample' : params.kind;
    if (!['sample', 'nks', 'all'].includes(kind)) fail('kind must be "sample", "nks" or "all".');
    const limit = params.limit === undefined ? 20 : params.limit; if (!isInt(limit) || limit < 1 || limit > 200) fail('limit must be 1–200.');
    const offset = params.offset === undefined ? 0 : params.offset; if (!isInt(offset) || offset < 0) fail('offset must be 0 or more.');
    let result;
    for (let attempt = 0; attempt < 3; attempt++) { result = await S().request('library.search',
        { query: String(params.query ?? ''), kind, filters: {}, favorites: !!params.favorites, offset, limit }); if (!result?.cancelled) break; }
    if (!result || result.cancelled) fail('The library search was interrupted. Try again.');
    return { total: result.total
        ?? (result.items || []).length, offset, items: (result.items || []).slice(0, limit).map(slimItem), ...(result.scanning
        ? { note: 'The library is still being scanned; results may be incomplete.' }
        : {}) };
  });
  method('loadSample', 'Load an audio file into an instrument (it becomes a sampler) and assign it to a track. Give path, or query to load the best library match.', ['path',
      'query', 'instrument', 'track'], async params => {
    const p = project();
    if ((params.path === undefined) === (params.query === undefined)) fail('Give either path (an audio file) or query (a library search).');
    let path = params.path;
    if (params.query !== undefined) {
      const found = await api.searchLibrary({ query: params.query, kind: 'sample', limit: 1 });
      if (!found.items.length) fail(`No library sample matches "${params.query}". Try searchLibrary with other words.`);
      path = found.items[0].path;
    }
    // Built-in samples are project references the host resolves itself (createInstrument names one).
    if (typeof path !== 'string' || !(path.startsWith('/') || /^builtin:./.test(path))) fail('path must be an absolute file path, or a built-in sample such as "builtin:wavetables/Basic/Saw.wav".');
    const track = params.track === undefined ? st().track : findTrack(p, params.track);
    const index = params.instrument === undefined ? (p.tracks[track].instrument ?? 0) : findInstrument(p, params.instrument);
    requireUnfrozen(p, [track]);
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot load a sample while ${busy}.`);
    st().track = track;
    const before = st().undo[st().undo.length - 1];
    try { await S().loadSample(path, index); } catch (error) { fail(`Could not load the sample: ${error.message || error}`); }
    const q = project();
    if (st().undo[st().undo.length - 1] === before) fail(`The sample was not attached${toastText() ? ': ' + toastText() : '.'}`);
    return { changed: true, instrument: index, track, name: q.instruments[index].name, sample: q.instruments[index].samplePath, duration: st().samples?.[index]?.duration ?? null };
  });
  function pluginCatalogue(all) {
    const list = Array.isArray(st().plugins) ? st().plugins : [];
    if (all) return list;
    const rank = { AU: 0, VST3: 1, VST2: 2, VST: 2 }, best = new Map();
    for (const plugin of list) {
      const key = `${String(plugin.manufacturer || '').toLowerCase()}|${String(plugin.name || '').toLowerCase()}|${plugin.type}`,
        current = best.get(key);
      if (!current || (rank[plugin.format] ?? 9) < (rank[current.format] ?? 9)) best.set(key, plugin);
    }
    return [...best.values()];
  }
  method('listPlugins', 'List installed plug-ins (one per plug-in: AU, else VST3, else VST2 unless allFormats). type: "instrument" or "effect".', ['query',
      'type', 'format', 'limit', 'allFormats', 'refresh'], async params => {
    if (params.refresh || !(st().plugins || []).length) { try { const result = await S().request('plugins.list'); st().plugins = Array.isArray(result)
        ? result
        : result?.plugins || []; } catch (error) { fail(`Could not list plug-ins: ${error.message}`); } }
    if (params.type !== undefined && !['instrument', 'effect'].includes(params.type)) fail('type must be "instrument" or "effect".');
    const words = String(params.query || '').toLowerCase().split(/\s+/).filter(Boolean), limit = params.limit === undefined ? 50 : params.limit;
    if (!isInt(limit) || limit < 1 || limit > 500) fail('limit must be 1–500.');
    const hits = pluginCatalogue(!!params.allFormats).filter(plugin => (!params.type || plugin.type === params.type)
        && (!params.format || String(plugin.format).toLowerCase() === String(params.format).toLowerCase())
        && words.every(word => `${plugin.name} ${plugin.manufacturer} ${plugin.format}`.toLowerCase().includes(word)));
    return { total: hits.length, plugins: hits.slice(0, limit).map(plugin => ({ id: plugin.id, name: plugin.name, manufacturer: plugin.manufacturer, format: plugin.format, type: plugin.type, version: plugin.version })) };
  });
  function resolvePlugin(ref, kind) {
    if (typeof ref !== 'string' || !ref.trim()) fail('plugin must be a plug-in name or id from listPlugins.');
    const list = Array.isArray(st().plugins) ? st().plugins : [], text = ref.trim().toLowerCase();
    const byId = list.find(plugin => plugin.id === ref); if (byId) return byId;
    const preferred = pluginCatalogue(false).filter(plugin => !kind || plugin.type === kind);
    const exact = preferred.filter(plugin => String(plugin.name).toLowerCase() === text
        || `${plugin.manufacturer} ${plugin.name}`.toLowerCase() === text);
    if (exact.length === 1) return exact[0];
    const partial = preferred.filter(plugin => `${plugin.manufacturer} ${plugin.name}`.toLowerCase().includes(text));
    if (partial.length === 1) return partial[0];
    if (exact.length > 1 || partial.length > 1) fail(`"${ref}" matches several plug-ins: ${(exact.length > 1 ? exact : partial).slice(0, 12).map(plugin => `${plugin.manufacturer} ${plugin.name} (${plugin.format}, id ${plugin.id})`).join('; ')}. Use the id.`);
    fail(`No ${kind || ''} plug-in matches "${ref}". Use listPlugins to see what is installed.`);
  }
  function blockedByBusyUI() {
    if (window.STBusy.isBusy('session')) return 'a save or project change is in progress';
    if (window.STBusy.isBusy('export')) return 'an audio export is running';
    if (window.STBusy.isBusy('recording')) return 'an audio recording is running';
    if (window.STBusy.isBusy('routing')) return 'a routing change is being applied';
    return null;
  }
  method('loadPlugin', 'Load a plug-in (AU/VST3/VST2/built-in) on a track as its instrument (kind "instrument") or as an insert effect (kind "effect", slot 0–15). '
      + 'channel loads an insert effect on a bus or the master channel ("master", bus name or "bus:<id>").', ['plugin',
      'track', 'channel', 'kind', 'slot', 'instrument'], async params => {
    const p = project(); if (!st().native) fail('Plug-ins load only in the macOS app.');
    if (!(st().plugins || []).length) await api.listPlugins({ refresh: true });
    const channel = params.channel === undefined ? null : pluginChannel(p, params);
    const kind = params.kind === undefined ? channel && channel.type !== 'track' ? 'effect' : 'instrument' : params.kind;
    if (!['instrument', 'effect'].includes(kind)) fail('kind must be "instrument" or "effect".');
    if (channel && channel.type !== 'track') {
      if (kind !== 'effect') fail(`${channelLabel(channel)} takes insert effects only (kind "effect").`);
      if (params.instrument !== undefined) fail('instrument applies to plug-in instruments on tracks only.');
      return loadChannelInsert(p, channel, resolvePlugin(params.plugin, 'effect'), params.slot);
    }
    const plugin = resolvePlugin(params.plugin, kind), track = channel ? channel.track : params.track === undefined ? st().track : findTrack(p, params.track);
    const usedSlots = new Set((p.audioPlugins || []).filter(x => onTrack(p, x, track) && x.kind === 'effect').map(x => x.slot ?? 0));
    const slot = kind === 'instrument' ? 0 : params.slot === undefined ? Array.from({
      length: LIMITS.effectSlots
    }, (_, i) => i).find(i => !usedSlots.has(i)) : params.slot;
    if (slot === undefined) fail(`All ${LIMITS.effectSlots} effect slots are occupied. Choose an explicit slot to replace an effect.`);
    if (!isInt(slot) || slot < 0 || slot >= LIMITS.effectSlots) fail(`slot must be 0–${LIMITS.effectSlots - 1}.`);
    requireUnfrozen(p, [track]);
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot load a plug-in while ${busy}.`);
    let instrument = null;
    if (kind === 'instrument') {
      const own = p.tracks[track].instrument ?? 0, shared = p.tracks.some((t, ti) => ti !== track && (t.instrument ?? 0) === own);
      instrument = params.instrument === undefined
          ? shared ? 'new' : own
          : params.instrument === 'new'
          ? 'new'
          : findInstrument(p, params.instrument);
    }
    const owner = p,
      trackObject = p.tracks[track],
      // Routed projects address track plug-ins by owner, as the mixer does (studio.js ownerPayload).
      routed = standardRouting(p) && !!trackObject.routingID,
      target = { track, ...(routed ? { owner: trackChannel(p, track).owner } : {}), kind, slot, component: plugin.id, ...(plugin.bundlePath ? { bundlePath: plugin.bundlePath } : {}) };
    const stillValid = () => st().project === owner && owner.tracks[track] === trackObject;
    // The slot holds this load's instance: in a routed project the snapshot names the owner, else the track.
    const attached = list => (Array.isArray(list) ? list : []).find(item => (item.owner ? item.owner === trackChannel(p, track).owner : item.track === track)
        && item.kind === kind && (item.slot ?? 0) === slot && (result?.instanceID ? item.instanceID === result.instanceID : item.component === plugin.id));
    let result, snapshot;
    try {
      // Publish every committed edit first, as the mixer's insert loader does: a debounced engine update that
      // arrives while the load is in flight describes the project without the new plug-in and removes it again.
      S().cancelPendingEngineSync?.();
      if (await S().syncEngineProject(() => stillValid() ? clone(owner) : null) === false || !stillValid()) fail('The session changed before the plug-in could load; nothing was attached.');
      result = await S().request('plugin.load', target);
      if (!stillValid()) { await S().syncEngineProject(() => clone(st().project)); fail('The session changed while the plug-in was loading; it was not attached.'); }
      snapshot = await S().request('plugin.state', {});
    } catch (error) { if (error instanceof AgentError) throw error; fail(`Could not load ${plugin.name}: ${error.message || error}`); }
    if (!stillValid()) { await S().syncEngineProject(() => clone(st().project)); fail('The session changed while the plug-in was loading; it was not attached.'); }
    if (Array.isArray(snapshot) && !attached(snapshot)) {
      await S().syncEngineProject(() => clone(st().project)).catch(() => undefined);
      fail(`${plugin.name} was not attached: the audio host no longer lists it on track ${track} (${kind}${kind === 'effect' ? ' slot ' + slot : ''}). Try again.`);
    }
    let instrumentIndex = null;
    const out = commit(`load ${plugin.name} on ${trackObject.name}`, project => {
      if (Array.isArray(snapshot)) project.audioPlugins = clone(snapshot);
      else { project.audioPlugins = (project.audioPlugins || []).filter(x => !(onTrack(project, x, track) && x.kind === kind && (x.slot ?? 0) === slot)); project.audioPlugins.push({ ...clone(result), ...target }); }
      // Never record a load the project does not hold.
      if (!attached([pluginOn(project, track, kind, slot)].filter(Boolean))) fail(`${plugin.name} was not attached to track ${track}.`);
      if (kind === 'instrument') {
        instrumentIndex = instrument === 'new' ? project.instruments.length : instrument;
        if (instrument === 'new') {
          const inst = S().normalize({ instruments: [{ id: instrumentIndex, type: 'au', name: plugin.name }] }).instruments[0];
          inst.id = instrumentIndex;
          project.instruments.push(inst);
        }
        const inst = project.instruments[instrumentIndex];
        inst.type = 'au';
        inst.plugin = clone(result);
        inst.name = plugin.name;
        project.tracks[track].instrument = instrumentIndex;
      } else { project.tracks[track].plugins ||= []; project.tracks[track].plugins[slot] = clone(result); }
      return {};
    });
    return { ...out, plugin: { name: plugin.name, format: plugin.format, id: plugin.id }, track, kind, slot, ...(instrumentIndex !== null
        ? { instrument: instrumentIndex }
        : {}) };
  });
  /* A bus or master insert loads through the mixer's insert API (STInserts in studio.js), like the mixer's own + button. */
  async function loadChannelInsert(p, channel, plugin, slot) {
    requireStandardRouting(p, `${channelLabel(channel)} inserts`);
    const inserts = window.STInserts; if (!inserts) fail('The mixer insert interface is unavailable.');
    const chain = channelEffects(p, channel);
    if (slot !== undefined) {
      if (!isInt(slot) || slot < 0 || slot >= LIMITS.effectSlots) fail(`slot must be 0–${LIMITS.effectSlots - 1}.`);
      const held = chain.find(item => (item.slot ?? 0) === slot);
      if (held) fail(`Slot ${slot} on ${channelLabel(channel)} holds ${held.name}. Remove it first (editInsert remove:true) or leave slot out for the next free slot.`);
    } else if (chain.length >= LIMITS.effectSlots) fail(`All ${LIMITS.effectSlots} insert slots on ${channelLabel(channel)} are occupied. Remove an effect first.`);
    const busy = blockedByBusyUI() || (inserts.busy ? 'another plug-in or routing change is in progress' : null);
    if (busy) fail(`Cannot load a plug-in while ${busy}.`);
    const label = `load ${plugin.name} on ${channel.name}`, before = st().undo[st().undo.length - 1];
    let added;
    try { added = await inserts.add(channel.owner, plugin.id, { bundlePath: plugin.bundlePath || '', label: `AI: ${label}`, ...(slot !== undefined ? { slot } : {}) }); }
    catch (error) { fail(`Could not load ${plugin.name}: ${error.message || error}`); }
    if (!added || st().undo[st().undo.length - 1] === before) fail(`${plugin.name} was not loaded${toastText() ? ': ' + toastText() : '.'}`);
    return { changed: true, undoLabel: `AI: ${label}`, plugin: { name: plugin.name, format: plugin.format, id: plugin.id }, channel: channel.owner, channelName: channel.name,
      kind: 'effect', slot: added.slot ?? slot, inserts: channelEffects(project(), channel).map(insertOut) };
  }
  async function fetchParameters(p, ref) {
    const plugin = slotPlugin(p, ref), { channel, kind, slot } = ref;
    if (!plugin) fail(`${channel.type === 'track' ? 'Track ' + channel.track + ' "' + channel.name + '"' : channelLabel(channel)} has no ${kind === 'instrument'
        ? 'plug-in instrument'
        : `effect in slot ${slot}`}.${kind === 'effect'
        ? ' Effects: ' + (channelEffects(p, channel).map(x => `${x.slot ?? 0} ${x.name}`).join(', ') || 'none')
        : ''}`);
    let list;
    try { list = await S().request('plugin.parameters',
        slotTarget(plugin, channel)); } catch (error) { fail(`Could not read ${plugin.name} parameters: ${error.message || error}`); }
    return { plugin, list: Array.isArray(list) ? list : [] };
  }
  /* {channel, kind, slot} for the plug-in tools: tracks default to their instrument, buses and master to effects. */
  function pluginRef(p, params) {
    const channel = pluginChannel(p, params), kind = params.kind || (channel.type === 'track' ? 'instrument' : 'effect'), slot = kind === 'instrument' ? 0 : params.slot ?? 0;
    if (!['instrument', 'effect'].includes(kind)
        || !isInt(slot)
        || slot < 0
        || slot >= LIMITS.effectSlots) fail(`Choose kind instrument/effect and slot 0–${LIMITS.effectSlots - 1}.`);
    if (kind === 'instrument' && channel.type !== 'track') fail(`${channelLabel(channel)} has insert effects only (kind "effect").`);
    return { channel, kind, slot };
  }
  const paramOut = item => ({
    address: item.address,
    name: item.name,
    value: round(item.value, 5),
    normalized: round(item.normalized, 5),
    display: item.display,
    min: item.min,
    max: item.max,
    unit: item.unitName || undefined,
    writable: item.writable !== false
  });
  // Native writes can succeed even when their response or subsequent state capture
  // fails. Keep these edits in the same persistence barrier as UI control edits.
  // A retry captures what actually exists; it never sends the parameter writes again.
  const parameterEdits = new Set(), parameterCancellations = new WeakSet();
  let parameterGeneration = 0;
  function parameterOwner(edit) {
    return st().project === edit.project && channelEntity(edit.project, edit.ref.channel) === edit.entity &&
      (edit.target.kind !== 'instrument' || ((edit.entity.instrument ?? 0) === edit.instrumentIndex && edit.project.instruments[edit.instrumentIndex] === edit.instrumentObject)) &&
      slotPlugin(edit.project, edit.ref) === edit.plugin &&
      edit.plugin.instanceID === edit.target.instanceID && edit.plugin.component === edit.component &&
      (edit.target.owner ? edit.plugin.owner === edit.target.owner : edit.plugin.track === edit.target.track);
  }
  function cancelParameterEdit(edit) {
    if (!parameterEdits.has(edit)) return;
    parameterEdits.delete(edit);
    if (st().project === edit.project) { parameterCancellations.add(edit.project); parameterGeneration++; st().dirty = true; }
  }
  function pendingParameterEdits() {
    for (const edit of parameterEdits) if (!parameterOwner(edit)) cancelParameterEdit(edit);
    return parameterCancellations.has(st().project) || [...parameterEdits].some(edit => edit.project === st().project);
  }
  function requireParameterOwner(edit) {
    if (!parameterOwner(edit)) {
      cancelParameterEdit(edit);
      fail('The project, channel or plug-in slot changed. The previous control edit was not attached to its replacement.');
    }
  }
  const otherControlGeneration = () => `${window.STPluginParameters?.editGeneration ?? 0}:${window.STNKSControls?.editGeneration ?? 0}`;
  function captureParameterEdit(edit) {
    if (edit.capture) return edit.capture;
    edit.capture = (async () => {
      requireParameterOwner(edit);
      const generation = otherControlGeneration(), snapshot = await S().request('plugin.state', edit.target);
      requireParameterOwner(edit);
      if (generation !== otherControlGeneration() || window.STPluginParameters?.hasPendingEdits || window.STNKSControls?.hasPendingEdits)
        fail('Other plug-in controls changed during capture. Finish those edits and save again.');
      if (snapshot?.stateError) fail(String(snapshot.stateError));
      if (!snapshot || typeof snapshot !== 'object' || Array.isArray(snapshot) || typeof snapshot.state !== 'string' || !snapshot.state)
        fail('The plug-in did not return its saved state. The edit remains pending; retry Save to capture it.');
      if (snapshot.instanceID !== edit.target.instanceID || snapshot.component !== edit.component || snapshot.kind !== edit.target.kind ||
          (snapshot.slot ?? 0) !== edit.target.slot || (edit.target.owner
              ? snapshot.owner !== edit.target.owner
              : snapshot.track !== edit.target.track))
        fail('The plug-in returned state for a different instance or slot. The edit remains pending.');
      const result = commit(edit.label, project => {
        const old = slotPlugin(project, edit.ref);
        if (!old
            || old.instanceID !== edit.target.instanceID
            || old.component !== edit.component) fail('The plug-in changed before capture completed.');
        const replacement = { ...old, ...clone(snapshot) }; project.audioPlugins[project.audioPlugins.indexOf(old)] = replacement;
        if (edit.target.kind === 'instrument') {
          const inst = project.instruments[project.tracks[edit.ref.channel.track].instrument ?? 0];
          if (inst?.type === 'au') inst.plugin = { ...inst.plugin, ...replacement };
        }
        return {};
      }, { nativeCapture: true, render: false });
      parameterEdits.delete(edit); edit.result = result; edit.error = null;
      return result;
    })().finally(() => { edit.capture = null; });
    return edit.capture;
  }
  async function flushParameterEdits() {
    const owner = st().project;
    pendingParameterEdits();
    if (parameterCancellations.has(owner)) { parameterCancellations.delete(owner); return false; }
    const watermark = [...parameterEdits].filter(edit => edit.project === owner);
    try {
      for (const edit of watermark) {
        await edit.work?.catch(() => {});
        if (st().project !== owner) return false;
        if (parameterCancellations.has(owner)) { parameterCancellations.delete(owner); return false; }
        if (parameterEdits.has(edit)) await captureParameterEdit(edit);
      }
      return st().project === owner && !pendingParameterEdits();
    } catch (error) { S().toast(`AI control edit is still pending: ${error.message || error}`, 'error'); return false; }
  }
  method('pluginParameters', 'List a plug-in\'s parameters (a track\'s instrument, or an effect slot on a track, bus or master), optionally filtered by query.', ['track',
      'channel', 'kind', 'slot', 'query', 'limit', 'offset'], async params => {
    const p = project(), ref = pluginRef(p, params), { channel, kind } = ref;
    const words = String(params.query || '').toLowerCase().split(/\s+/).filter(Boolean), limit = params.limit ?? 100, offset = params.offset ?? 0;
    if (!isInt(limit) || limit < 1) fail('limit must be a whole number, 1 or more.');
    if (!isInt(offset) || offset < 0) fail('offset must be 0 or more.');
    const { plugin, list } = await fetchParameters(p, ref);
    const hits = list.filter(item => words.every(word => `${item.name} ${item.id} ${item.address}`.toLowerCase().includes(word)));
    return { plugin: { name: plugin.name, format: plugin.format, instanceID: plugin.instanceID }, ...(channel.type === 'track'
        ? { track: channel.track }
        : { channel: channel.owner, channelName: channel.name }), kind, slot: plugin.slot ?? 0, total: hits.length, parameters: hits.slice(offset, offset + limit).map(paramOut) };
  });
  method('setPluginParameters', 'Set plug-in parameters by name or address on a track\'s instrument or an effect slot of a track, bus or master (channel). Values: "50%" '
      + 'or {normalized:0.5} = position 0–1; a plain number = the parameter\'s own value (min…max).', ['track', 'channel', 'kind', 'slot', 'values', 'label'], async params => {
    const p = project(), ref = pluginRef(p, params), { channel, kind, slot } = ref, t = channel.track;
    if (!params.values
        || typeof params.values !== 'object'
        || Array.isArray(params.values)
        || !Object.keys(params.values).length) fail('values must be an object like {"Cutoff": "40%", "Resonance": 0.3}.');
    if (channel.type === 'track') requireUnfrozen(p, [t]);
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot edit plug-in parameters while ${busy}.`);
    const entity = channelEntity(p, channel),
      instrumentIndex = channel.type === 'track' ? entity.instrument ?? 0 : null,
      instrumentObject = channel.type === 'track' ? p.instruments[instrumentIndex] : null,
      originalPlugin = slotPlugin(p, ref),
      originalIdentity = originalPlugin && {
        instanceID: originalPlugin.instanceID,
        component: originalPlugin.component,
        owner: originalPlugin.owner
      };
    if (!originalPlugin) await fetchParameters(p, ref); // explains the empty slot
    const instrumentCurrent = () => kind !== 'instrument'
        || (entity.instrument ?? 0) === instrumentIndex && p.instruments[instrumentIndex] === instrumentObject;
    const current = () => st().project === p && channelEntity(p, channel) === entity && instrumentCurrent() && slotPlugin(p, ref) === originalPlugin;
    // No pending record exists while these barriers run: they may flush earlier
    // agent edits, so registering first would wait recursively on this operation.
    await S().waitForGraphEdit?.();
    if (!current()) fail('The project or plug-in changed before the edit started.');
    if (await S().captureNativeEdits?.() === false) fail('Finish or retry the pending control edit before changing more parameters.');
    // A preflight snapshot can replace an equivalent descriptor without changing
    // its live instance. Take ownership of that settled descriptor below.
    if (st().project !== p
        || channelEntity(p, channel) !== entity
        || !instrumentCurrent()) fail('The project or assigned instrument changed before the edit started.');
    const settled = slotPlugin(p, ref);
    if (!originalIdentity
        || !settled
        || originalIdentity.instanceID !== settled.instanceID
        || originalIdentity.component !== settled.component
        || originalIdentity.owner !== settled.owner) fail('The plug-in changed before the edit started.');
    await S().waitForEngineUpdate?.();
    if (st().project !== p
        || channelEntity(p, channel) !== entity
        || slotPlugin(p, ref) !== settled) fail('The plug-in changed before the edit started.');
    const { plugin, list } = await fetchParameters(p, ref);
    if (st().project !== p
        || channelEntity(p, channel) !== entity
        || slotPlugin(p, ref) !== settled
        || plugin.instanceID !== originalIdentity.instanceID
        || plugin.component !== originalIdentity.component
        || plugin.owner !== originalIdentity.owner)
      fail('The project or plug-in slot changed while parameters were being read.');
    const plan = Object.entries(params.values).map(([name, raw]) => {
      const text = name.toLowerCase();
      const exact = list.filter(item => String(item.address) === name
          || String(item.name).toLowerCase() === text
          || String(item.id).toLowerCase() === text);
      const hits = exact.length ? exact : list.filter(item => String(item.name).toLowerCase().includes(text));
      if (hits.length !== 1) fail(hits.length ? `"${name}" matches several parameters: ${hits.slice(0, 10).map(item => `${item.name} (address ${item.address})`).join(', ')}. Use `
          + `the address.` : `${plugin.name} has no parameter "${name}". Use pluginParameters to list them.`);
      const item = hits[0]; if (item.writable === false) fail(`${item.name} is read-only.`);
      let normalized;
      if (raw && typeof raw === 'object' && Number.isFinite(raw.normalized)) normalized = raw.normalized;
      else if (typeof raw === 'string' && /%\s*$/.test(raw)) normalized = Number(raw.replace('%', '')) / 100;
      else { const value = Number(raw); if (!Number.isFinite(value)) fail(`${item.name}: give a number between ${item.min} and ${item.max}, a percentage like "50%", or {normalized: 0–1}.`); if (value < item.min - 1e-6
          || value > item.max + 1e-6) fail(`${item.name} must be between ${item.min} and ${item.max}${item.unitName ? ' ' + item.unitName : ''} (got ${value}).`); normalized = item.max > item.min
          ? (value - item.min) / (item.max - item.min)
          : 0; }
      if (!Number.isFinite(normalized) || normalized < 0 || normalized > 1) fail(`${item.name}: the position must be 0–100%.`);
      return { item, normalized };
    });
    const target = slotTarget(plugin, channel),
      applied = [],
      edit = {
        project: p,
        ref,
        entity,
        instrumentIndex,
        instrumentObject,
        plugin,
        component: plugin.component,
        target,
        label: params.label || `${channel.type === 'track' ? '' : channel.name + ' '}${plugin.name}: ${plan.map(x => x.item.name).join(', ')}`
      };
    requireParameterOwner(edit);
    const nowBusy = blockedByBusyUI(); if (nowBusy) fail(`Cannot edit plug-in parameters while ${nowBusy}.`);
    parameterEdits.add(edit); parameterGeneration++; st().dirty = true; S().updateTransport?.();
    // What must stick is the position the host sent (`written`, snapped to the parameter's steps). Never the value in
    // the same answer: a plug-in that applies writes on its next render (NI Raum) still reports the old value there.
    const write = async ({ item, normalized }) => {
      requireParameterOwner(edit);
      const result = await S().request('plugin.parameter', { ...target, address: item.address, value: normalized, normalized: true });
      requireParameterOwner(edit);
      return { item, requested: normalized, expected: Number.isFinite(result?.written) ? result.written : normalized };
    };
    const readBack = async (owned = true) => {
      let list; try { list = await S().request('plugin.parameters', target); } catch { return new Map(); }
      if (owned) requireParameterOwner(edit);
      return new Map((Array.isArray(list) ? list : list?.parameters || []).map(item => [String(item.address), item]));
    };
    const kept = (values, entry) => { const now = values.get(String(entry.item.address)); return !!now && Math.abs(Number(now.normalized) - entry.expected) <= 1e-3; };
    const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
    edit.work = (async () => {
      let writeError = null;
      try { for (const entry of plan) applied.push(await write(entry)); } catch (error) { writeError = error; }
      requireParameterOwner(edit);
      // Some plug-ins (NI Raum, for one) queue writes and apply them on their next render, and some move a value
      // themselves. Their state is captured only once they report every new value, rewriting missing ones one at a
      // time: a capture taken earlier holds the old values and would put them back when the project is next published.
      if (!writeError && applied.length) {
        for (let attempt = 0; attempt < 6; attempt++) {
          const values = await readBack(), missing = applied.filter(entry => !kept(values, entry));
          if (!missing.length) break;
          if (attempt >= 2) try { for (const entry of missing) Object.assign(entry, await write({ item: entry.item, normalized: entry.requested })); } catch (error) { writeError = error; break; }
          await pause(30 * (attempt + 1));
        }
      }
      let out;
      try { out = await captureParameterEdit(edit); }
      catch (error) { edit.error = error; fail(`${writeError ? `Parameter write failed: ${writeError.message}. ` : ''}${error.message || error} The live change remains pending until `
          + `its state is captured.`); }
      if (writeError) fail(`Could not set every ${plugin.name} parameter: ${writeError.message || writeError}. The actual resulting state was captured${out.changed ? ' as one undoable edit' : ''}.`);
      return out;
    })();
    const out = await edit.work;
    // Report what the plug-in holds after the commit reached the host, not what was asked for.
    await S().waitForEngineUpdate?.();
    const values = st().project === p && slotPlugin(p, ref)?.instanceID === plugin.instanceID ? await readBack(false) : new Map();
    const parameters = applied.map(entry => {
      const now = values.get(String(entry.item.address));
      return { name: entry.item.name, address: entry.item.address, requested: round(entry.requested, 5), ...(now
          ? { normalized: round(now.normalized, 5), display: now.display, applied: kept(values, entry) }
          : { applied: null }) };
    });
    const dropped = parameters.filter(item => item.applied === false);
    return { ...out, plugin: plugin.name, parameters, ...(dropped.length ? { warning: `${plugin.name} did not keep ${dropped.map(item => item.name).join(', ')} (read back `
        + `${dropped.map(item => `${item.display ?? round(item.normalized * 100, 1) + '%'}`).join(', ')}). Set them again, one per call.` } : {}) };
  });
  method('searchNKS', 'Search the NKS preset catalogue (Native Instruments and other NKS presets). kind "instrument" or "effect".', ['query', 'kind',
      'product', 'limit', 'offset'], async params => {
    const kind = params.kind || 'instrument'; if (!['instrument', 'effect'].includes(kind)) fail('kind must be "instrument" or "effect".');
    const limit = params.limit ?? 20; if (!isInt(limit) || limit < 1 || limit > 200) fail('limit must be 1–200.');
    let result;
    try { result = await S().request('library.nks.browse',
        { kind, query: String(params.query ?? ''), ...(params.product
            ? { product: params.product }
            : {}), types: [], subtypes: [], characters: [], favorites: false, offset: params.offset ?? 0, limit }); }
    catch (error) { fail(`NKS search failed: ${error.message || error}`); }
    return { total: Number(result?.total) || 0, items: (result?.items || []).slice(0, limit).map(slimItem), ...(result?.status?.scanning
        ? { note: 'The NKS catalogue is still being indexed.' }
        : {}) };
  });
  method('loadNKS', 'Load an NKS preset (by path, or the best match for query) on a track: kind "instrument" (default) or "effect" (slot 0–15; default: the next '
      + 'free slot, an explicit slot replaces what it holds).', ['path',
      'query', 'track', 'kind', 'slot', 'instrument'], async params => {
    const p = project(); if (!st().native) fail('NKS presets load only in the macOS app.');
    if (!window.STNKS) fail('The NKS loader is unavailable.');
    const kind = params.kind || 'instrument'; if (!['instrument', 'effect'].includes(kind)) fail('kind must be "instrument" or "effect".');
    if ((params.path === undefined) === (params.query === undefined)) fail('Give either path (an .nksf/.nksfx file) or query.');
    let path = params.path;
    if (params.query !== undefined) { const found = await api.searchNKS({ query: params.query, kind, limit: 1 }); if (!found.items.length) fail(`No NKS ${kind} preset matches "${params.query}".`); path = found.items[0].path; }
    if (typeof path !== 'string' || !path.startsWith('/')) fail('path must be an absolute preset path.');
    const track = params.track === undefined ? st().track : findTrack(p, params.track);
    requireUnfrozen(p, [track]);
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot load a preset while ${busy}.`);
    const options = { kind, stay: true };
    if (kind === 'effect') {
      // As loadPlugin: the first free slot. The loader replaces what a slot holds, so only an explicit slot may do that.
      const usedSlots = new Set((p.audioPlugins || []).filter(x => onTrack(p, x, track) && x.kind === 'effect').map(x => x.slot ?? 0));
      options.slot = params.slot ?? Array.from({ length: LIMITS.effectSlots }, (_, i) => i).find(i => !usedSlots.has(i));
      if (options.slot === undefined) fail(`All ${LIMITS.effectSlots} effect slots are occupied. Choose an explicit slot to replace an effect.`);
      if (!isInt(options.slot)
          || options.slot < 0
          || options.slot >= LIMITS.effectSlots) fail(`slot must be 0–${LIMITS.effectSlots - 1}.`);
    } else {
      const own = p.tracks[track].instrument ?? 0, shared = p.tracks.some((t, ti) => ti !== track && (t.instrument ?? 0) === own);
      const choice = params.instrument === undefined
          ? shared ? 'new' : own
          : params.instrument === 'new'
          ? 'new'
          : findInstrument(p, params.instrument);
      if (choice !== 'new' && choice !== own) fail('An NKS instrument replaces the instrument the track plays. Leave instrument out (or "new").');
      if (choice === 'new') options.newInstrument = true;
    }
    st().track = track; if (kind === 'instrument' && !options.newInstrument) st().instrument = p.tracks[track].instrument ?? 0;
    const before = st().undo[st().undo.length - 1];
    const ok = await window.STNKS.load(path, options);
    if (!ok || st().undo[st().undo.length - 1] === before) fail(`The preset was not loaded${toastText() ? ': ' + toastText() : '.'}`);
    const q = project();
    return { changed: true, track, kind, path, ...(kind === 'effect' ? { slot: options.slot } : {}), instrument: kind === 'instrument' ? q.tracks[track].instrument : undefined, name: kind === 'instrument'
        ? q.instruments[q.tracks[track].instrument]?.name
        : undefined };
  });

  // ---------------------------------------------------------------- automation
  const BUILTIN_AUTOMATION = {
    volume: TRACK_VALUES.volume,
    pan: TRACK_VALUES.pan,
    delay: TRACK_VALUES.delaySend,
    reverb: TRACK_VALUES.reverbSend,
    instrumentVolume: PARAMS.volume,
    instrumentPan: PARAMS.pan,
    tune: PARAMS.tune,
    pitch: { min: -24, max: 24, kind: 'semi' }
  };
  function automationSpec(p, t, target) {
    const studio = window.STStudio, descriptor = studio?.target?.(t, target);
    const known = ownValue(BUILTIN_AUTOMATION, target) || ownValue(PARAMS, target);
    if (target.startsWith('plugin:')) {
      // plugin:<instrument|effect>:<slot>:<address> of a plug-in this track holds (as bus and master lanes check).
      const match = /^plugin:(instrument|effect):(\d+):(.+)$/.exec(target);
      if (!match || !pluginOn(p, t, match[1], Number(match[2]))) fail(`Track ${t} "${p.tracks[t].name}" has no plug-in for "${target}". Plug-in targets are `
          + 'plugin:instrument:0:<address> and plugin:effect:<slot>:<address> (see automationTargets).');
      return { min: 0, max: 1, kind: 'pct', name: descriptor?.name || target, descriptor };
    }
    if (descriptor && descriptor.available === false) fail(descriptor.unavailableReason || `${target} cannot be automated on this track.`);
    // The mixer lists the targets this track can play (STStudio.target); one it does not list would be a lane nothing reads.
    // Without the mixer only the numeric built-in parameters are known.
    if (!descriptor && (studio?.target || !known || !Number.isFinite(known.min))) fail(studio?.target && known
        ? `"${target}" cannot be automated on track ${t} "${p.tracks[t].name}". Use automationTargets to list what can.`
        : `Unknown automation target "${target}". Use automationTargets to list them.`);
    return { min: descriptor?.min ?? known.min, max: descriptor?.max ?? known.max, kind: known?.kind || 'num', name: descriptor?.name
        || known?.label
        || target, descriptor };
  }
  /* Bus and master lanes (and track send lanes) are owner lanes {owner, target}, as app-automation.js creates them;
     they play through AudioRoutingAutomation, which needs standard routing. */
  function ownerAutomationSpec(p, channel, target) {
    const descriptor = window.STStudio?.target?.(channel.owner, target), master = channel.type === 'master';
    const effect = /^plugin:effect:(\d+):(.+)$/.exec(target);
    if (effect) {
      if (!channelEffects(p, channel).some(plugin => (plugin.slot ?? 0) === Number(effect[1]))) fail(`${channelLabel(channel)} has no effect in slot ${effect[1]}. Effects: ${channelEffects(p,
          channel).map(x => `${x.slot ?? 0} ${x.name}`).join(', ') || 'none'}.`);
      return { min: 0, max: 1, kind: 'pct', name: descriptor?.name || target };
    }
    if (target.startsWith('send:')) {
      const send = (p.routing?.sends || []).find(item => `send:${item.id}` === target);
      if (!send || send.source !== channel.owner) fail(`${channelLabel(channel)} has no send ${target.slice(5)}. Sends: ${(p.routing?.sends || []).filter(item => item.source === channel.owner)
          .map(item => `send:${item.id} (to ${channelOf(p, item.destination)?.name || item.destination})`).join(', ') || 'none'}.`);
      return { min: 0, max: 2, kind: 'db', name: `Send to ${channelOf(p, send.destination)?.name || send.destination}` };
    }
    if (channel.type !== 'track') {
      if (target === 'volume') return { min: 0, max: master ? 1.5 : 2, kind: 'db', name: master ? 'Master volume' : 'Channel volume' };
      if (target === 'pan' && !master) return { min: -1, max: 1, kind: 'pan', name: 'Channel pan' };
    }
    fail(`"${target}" cannot be automated on ${channelLabel(channel)}. Targets: ${master ? 'volume' : 'volume, pan'}, send:<id>, plugin:effect:<slot>:<address> (see automationTargets).`);
  }
  /* The lane a call addresses: {channel, owner lane?}. Track lanes stay track lanes except for sends. */
  function automationChannel(p, params, target) {
    const channel = pluginChannel(p, params, { required: true });
    const ownerLane = channel.type !== 'track' || String(target || '').startsWith('send:');
    if (channel.type !== 'track') requireStandardRouting(p, `Automation on ${channelLabel(channel)}`);
    return { channel, ownerLane };
  }
  const laneMatches = (lane, channel, ownerLane, target) => lane.target === target && (ownerLane ? lane.owner === channel.owner : !lane.owner && lane.track === channel.track);
  method('automationTargets', 'List parameters that can be automated on a track, bus or master (channel): built-in, instrument, sends, plug-in parameters.', ['track',
      'channel', 'query'], async params => {
    const p = project(), channel = pluginChannel(p, params), t = channel.type === 'track' ? channel.track : channel.owner;
    if (!window.STStudio?.automationChoices) fail('Automation is unavailable.');
    if (channel.type !== 'track') requireStandardRouting(p, `Automation on ${channelLabel(channel)}`);
    try { await window.STStudio.loadAutomationParameters?.(t); } catch { /* plug-in parameters stay unlisted */ }
    const words = String(params.query || '').toLowerCase().split(/\s+/).filter(Boolean);
    const choices = window.STStudio.automationChoices(t).filter(item => words.every(word => `${item.name} ${item.group} ${item.target} ${item.search || ''}`.toLowerCase().includes(word)));
    return { ...(channel.type === 'track' ? { track: t } : { channel: channel.owner, channelName: channel.name }), targets: choices.slice(0, 300).map(item => { const d = window.STStudio.target?.(t,
        item.target); return { target: item.target, name: item.name, group: item.group, available: item.available, automated: item.automated, min: d?.min, max: d?.max, value: round(item.value ?? d?.value ?? 0, 5) }; }) };
  });
  method('setAutomation', 'Create or replace an automation lane for a track, bus or master parameter (channel) in a pattern. points: [{row, value}] with values in the '
      + 'parameter\'s units (strings with units allowed for built-in targets, volumes and sends; plug-in targets 0–1 or "50%").', ['pattern', 'track', 'channel', 'target', 'points',
          'interpolation', 'merge', 'label'], params => {
    const p = project(), pat = findPattern(p, params.pattern);
    if (typeof params.target !== 'string'
        || !params.target) fail('target must be an automation target id such as "cutoff", "volume" or "plugin:instrument:0:12" (see automationTargets).');
    const { channel, ownerLane } = automationChannel(p, params, params.target), t = channel.track;
    const spec = ownerLane ? ownerAutomationSpec(p, channel, params.target) : automationSpec(p, t, params.target);
    if (!Array.isArray(params.points) || !params.points.length || params.points.length > 1024) fail('points must be a list of 1–1024 {row, value}.');
    const parsed = params.points.map((point, k) => {
      if (!point || !isInt(point.row) || point.row < 0 || point.row >= pat.length) fail(`points[${k}].row must be 0–${pat.length - 1}.`);
      // A number or a string: null, true or [] would otherwise become 0 or 1.
      const given = typeof point.value === 'number' || typeof point.value === 'string' && point.value.trim() !== '';
      const value = !given ? NaN : typeof point.value === 'string' && spec.kind !== 'num'
          ? parseValue(spec, point.value, `points[${k}].value`)
          : Number(point.value);
      if (!Number.isFinite(value)
          || value < spec.min - 1e-9
          || value > spec.max + 1e-9) fail(`points[${k}].value must be between ${spec.min} and ${spec.max} for ${spec.name} (got ${point.value}).`);
      return { row: point.row, value: round(value, 6) };
    });
    // A lane holds one point per row (the automation editor addresses points by row): the last one given wins.
    const lastAt = new Map(parsed.map((point, k) => [point.row, k]));
    const points = parsed.filter((point, k) => lastAt.get(point.row) === k).sort((a, b) => a.row - b.row);
    if (params.interpolation !== undefined && !['linear', 'step'].includes(params.interpolation)) fail('interpolation must be "linear" or "step".');
    if (channel.type === 'track') requireUnfrozen(p, [t]);
    const id = pat.id, target = params.target;
    return commit(params.label || `automate ${spec.name} on ${channel.name}`, project => {
      const x = project.patterns.find(q => q.id === id); x.automation ||= [];
      let lane = x.automation.find(l => laneMatches(l, channel, ownerLane, target));
      if (!lane) {
        lane = { id: S().uid(), ...(ownerLane ? { owner: channel.owner } : { track: t }), target, name: `${channel.name} · ${spec.name}`, interpolation: 'linear', points: [] };
        x.automation.push(lane);
      }
      const merged = params.merge
          ? [...lane.points.filter(pt => !points.some(q => q.row === pt.row)), ...points].sort((a, b) => a.row - b.row)
          : points;
      lane.points = merged.map(pt => ({ ...pt }));
      if (params.interpolation) lane.interpolation = params.interpolation;
      return { lane: { ...(channel.type === 'track' ? { track: t } : {}), ...(ownerLane ? { channel: channel.owner } : {}), target, name: lane.name, points: lane.points.length,
        interpolation: lane.interpolation } };
    });
  });
  method('deleteAutomation', 'Delete a track\'s, bus\'s or master\'s (channel) automation lane for a target in a pattern.', ['pattern', 'track', 'channel', 'target', 'label'], params => {
    const p = project(), pat = findPattern(p, params.pattern), id = pat.id, { channel, ownerLane } = automationChannel(p, params, params.target);
    if (!(pat.automation || []).some(l => laneMatches(l, channel, ownerLane, params.target))) fail(`"${pat.name}" has no ${params.target} lane on ${channelLabel(channel)}.`);
    return commit(params.label || `delete ${params.target} automation`,
        project => { const x = project.patterns.find(q => q.id === id); x.automation = x.automation.filter(l => !laneMatches(l, channel, ownerLane, params.target)); return {}; });
  });

  // ---------------------------------------------------------------- mixer: buses, master, routing, sends, inserts
  /* Routing edits go through STRouting.transaction (routing.js), exactly like the mixer's routing panel: the host
     validates the draft, applies it, and the committed project becomes one undo step. Levels, mute/solo and names are
     plain project edits (commit), like the mixer's faders and buttons; inserts use the mixer's STInserts API. */
  const BUS_VOLUME = { min: 0, max: 2, kind: 'db' }, SEND_GAIN = { min: 0, max: 2, kind: 'db' };
  const volumeSpec = ch => ch.type === 'master' ? SONG_VALUES.masterVolume : ch.type === 'track' ? TRACK_VALUES.volume : BUS_VOLUME;
  async function routingCapabilities() {
    const R = window.STRouting; if (!R) fail('Routing is unavailable in this interface.');
    if (!R.available()) await R.refreshCapabilities();
    if (!R.available()) fail('Advanced routing is not available from the current audio host.');
    return R.capabilities || {};
  }
  async function routingCommit(label, edit, owner) {
    const busy = blockedByBusyUI() || (window.STInserts?.busy ? 'a plug-in change is in progress' : null);
    if (busy) fail(`Cannot change routing while ${busy}.`);
    const before = st().undo[st().undo.length - 1];
    try { await window.STRouting.transaction(`AI: ${label}`, edit, owner ? { owner } : {}); }
    catch (error) { fail(error instanceof AgentError ? error.message : `The routing change was not applied: ${error.message || error}`); }
    if (st().undo[st().undo.length - 1] === before) fail(`The routing change was not applied${toastText() ? ': ' + toastText() : '.'}`);
    return { changed: true, undoLabel: `AI: ${label}` };
  }
  const outputOf = (p, owner) => Object.hasOwn(p.routing?.outputs || {}, owner) ? p.routing.outputs[owner] : 'master';
  function describeSend(p, send) {
    return { id: send.id, from: send.source, fromName: channelOf(p, send.source)?.name, to: send.destination, toName: channelOf(p, send.destination)?.name,
      tap: send.tap, gain: formatValue('db', send.gain ?? 0), enabled: send.enabled !== false, automationTarget: `send:${send.id}` };
  }
  function describeChannel(p, ch) {
    const entity = channelEntity(p, ch), standard = standardRouting(p), out = { channel: ch.owner, name: ch.name, type: ch.type };
    if (ch.type === 'track') out.index = ch.track;
    if (ch.type === 'master') out.volume = formatValue('db', p.masterVolume ?? .7);
    else {
      Object.assign(out, { volume: formatValue('db', entity.volume ?? (ch.type === 'track' ? .8 : 1)), pan: formatValue('pan', entity.pan ?? 0), mute: !!entity.mute, solo: !!entity.solo });
      const output = standard ? outputOf(p, ch.owner) : 'master';
      Object.assign(out, { output, outputName: output === null ? 'No output' : channelOf(p, output)?.name || output });
    }
    if (ch.type === 'track') Object.assign(out, { delaySend: formatValue('pct', entity.delaySend ?? 0), reverbSend: formatValue('pct', entity.reverbSend ?? 0), ...(entity.frozen
        ? { frozen: true }
        : {}) });
    if (ch.type === 'group' || ch.type === 'return') {
      const group = (p.groups || []).find(item => item.busID === ch.bus);
      if (group) out.trackGroup = group.name;
      out.inputs = Object.entries(p.routing?.outputs || {}).filter(([, to]) => to === ch.owner).map(([from]) => channelOf(p, from)?.name || from);
      out.sendsIn = (p.routing?.sends || []).filter(send => send.destination === ch.owner).map(send => channelOf(p, send.source)?.name || send.source);
    }
    if (ch.type !== 'master') out.sends = (p.routing?.sends || []).filter(send => send.source === ch.owner).map(send => describeSend(p, send));
    out.inserts = channelEffects(p, ch).map(insertOut);
    return out;
  }
  function sendEffectsOut(p) {
    const model = window.STSends, read = group => Object.fromEntries((model?.RETURNS?.[group] || []).filter(spec => spec.kind !== 'choice').map(spec => [spec.key, spec.text(p)]));
    return model ? { delay: read('delay'), reverb: read('reverb') } : { delay: clone(p.delay), reverb: clone(p.reverb) };
  }
  method('mixer', 'Describe the mixer: mix model, master (volume, inserts), buses (type, level, output, inputs, sends, inserts), tracks (level, output, sends, '
      + 'inserts), sidechains and the built-in delay/reverb send effects. channel limits it to one channel.', ['channel'], async params => {
    const p = project(), standard = standardRouting(p);
    if (params.channel !== undefined) return describeChannel(p, findChannel(p, params.channel));
    if (standard && window.STRouting && !window.STRouting.capabilities) { try { await window.STRouting.refreshCapabilities(); } catch { /* capabilities stay unlisted */ } }
    const caps = window.STRouting?.capabilities, q = project();
    return {
      mixModel: standard ? 'standard' : 'legacy',
      ...(standard ? {} : { note: 'Legacy mix: every track goes to Master and only the built-in delay/reverb sends exist. Buses, outputs, custom sends and master inserts '
          + 'need enableStandardRouting (ask the user first: it changes the gain order).' }),
      master: describeChannel(q, MASTER),
      buses: (q.routing?.buses || []).map(bus => describeChannel(q, busChannel(bus))),
      tracks: q.tracks.map((_, t) => describeChannel(q, trackChannel(q, t))),
      sidechains: (q.routing?.sidechains || []).map(route => {
        const plugin = (q.audioPlugins || []).find(item => item.instanceID === route.destination?.instanceID),
          owner = plugin && (plugin.owner || (q.tracks[plugin.track] ? trackChannel(q, plugin.track).owner : null));
        return { id: route.id, from: route.source, fromName: channelOf(q, route.source)?.name, to: { channel: owner || null, plugin: plugin?.name || null, slot: plugin?.slot ?? null,
          input: route.destination?.layoutSignature?.name || null }, tap: route.tap, gain: formatValue('db', route.gain ?? 1), enabled: route.enabled !== false };
      }),
      sendEffects: sendEffectsOut(q),
      ...(caps ? { capabilities: { outputs: caps.outputs === true, groupBuses: caps.buses?.group === true, returnBuses: caps.buses?.return === true, sends: caps.sends?.enabled === true,
        sendTaps: caps.sends?.taps || [] } } : {})
    };
  });
  method('enableStandardRouting', 'Switch the song from the legacy mix to standard routing (buses, track outputs, custom sends, master inserts). It changes the gain '
      + 'order (track faders after inserts, master gain after the routing graph), so ask the user first. One undo step.', ['label'], async params => {
    if (standardRouting(project())) return { changed: false, mixModel: 'standard' };
    const caps = await routingCapabilities();
    if (!(caps.mixModels || []).includes('standard')) fail('The current audio host does not offer standard routing.');
    return { ...await routingCommit(params.label || 'enable standard routing', draft => { draft.routing.mixModel = 'standard'; }), mixModel: standardRouting(project()) ? 'standard' : 'legacy' };
  });
  /* An output destination: a group/return bus or Master that adds no feedback loop (routing.js decides), or null = disconnected. */
  function outputDestination(p, sources, value) {
    if (value === null || typeof value === 'string' && /^(none|no output|off|disconnected)$/i.test(value.trim())) return null;
    const destination = findChannel(p, value, 'output');
    if (destination.type === 'track') fail(`${channelLabel(destination)} cannot take an output: outputs go to a group or return bus, Master, or "none".`);
    for (const source of sources) {
      if (source.type === 'master') fail('Master is the final output; it has no output to change.');
      const allowed = window.STRouting.destinationOptions(source.owner, p, { output: true });
      if (!allowed.some(item => item.owner === destination.owner)) fail(`${channelLabel(source)} cannot output to ${channelLabel(destination)}: that would create a feedback loop. `
          + `Valid outputs: ${['none', ...allowed.map(item => item.owner === 'master' ? 'master' : `"${item.name}" (${item.owner})`)].join(', ')}.`);
    }
    return destination.owner;
  }
  method('editChannel', 'Change mixer channels: name, volume ("-6 dB"; tracks/master 0–1.5, buses 0–2 gain), pan ("30% L"), mute, solo, output (a bus, "master" or '
      + '"none"; standard routing). channel: "master", a bus name or "bus:<id>", a track index/name, or a list of them. Master takes volume only.', ['channel', 'name',
          'volume', 'pan', 'mute', 'solo', 'output', 'label'], async params => {
    const p = project();
    if (params.channel === undefined) fail('channel is required: "master", a bus name or "bus:<id>", a track index or name, or a list of them (see mixer).');
    const refs = Array.isArray(params.channel) ? params.channel : [params.channel];
    if (!refs.length) fail('channel must name at least one channel.');
    const channels = [...new Map(refs.map(ref => findChannel(p, ref)).map(ch => [ch.owner, ch])).values()];
    const keys = ['name', 'volume', 'pan', 'mute', 'solo', 'output'].filter(key => params[key] !== undefined);
    if (!keys.length) fail('Nothing to change. Give name, volume, pan, mute, solo or output.');
    if (params.name !== undefined && channels.length > 1) fail('name changes one channel at a time.');
    const name = params.name === undefined ? undefined : checkName(params.name);
    for (const key of ['mute', 'solo']) if (params[key] !== undefined && typeof params[key] !== 'boolean') fail(`${key} must be true or false.`);
    const plan = channels.map(ch => {
      const unsupported = ch.type === 'master' ? keys.filter(key => key !== 'volume') : [];
      if (unsupported.length) fail(`Master has no ${unsupported.join(', ')}: it is the final output. Give volume only (editInsert and loadPlugin reach its inserts).`);
      if (ch.type === 'track' && keys.some(key => !['mute', 'solo'].includes(key))) requireUnfrozen(p, [ch.track]);
      return { ch, volume: params.volume === undefined ? undefined : parseValue(volumeSpec(ch), params.volume, `volume (${ch.name})`),
        pan: params.pan === undefined ? undefined : parseValue(TRACK_VALUES.pan, params.pan, 'pan') };
    });
    let output;
    if (params.output !== undefined) {
      requireStandardRouting(p, 'Changing a channel output');
      if ((await routingCapabilities()).outputs !== true) fail('Output routing is not available from the current audio host.');
      if (project() !== p) fail('The project changed while routing was being prepared. Try again.');
      output = outputDestination(p, channels, params.output);
    }
    const label = params.label || `edit ${channels.map(ch => ch.name).join(', ')} (${keys.join(', ')})`;
    const apply = (draft, remap = owner => owner) => {
      for (const { ch, volume, pan } of plan) {
        const entity = channelEntity(draft, ch); if (!entity) fail(`${channelLabel(ch)} no longer exists.`);
        if (volume !== undefined) { if (ch.type === 'master') draft.masterVolume = volume; else entity.volume = volume; }
        if (pan !== undefined) entity.pan = pan;
        for (const key of ['mute', 'solo']) if (params[key] !== undefined) entity[key] = params[key];
        // A track group's bus carries the group's name (track-groups.js renames both).
        if (name !== undefined) { if (entity.groupID && window.STGroups?.byID(draft, entity.groupID)) window.STGroups.rename(draft, entity.groupID, name); else entity.name = name; }
        if (output !== undefined) draft.routing.outputs[remap(ch.owner)] = output === null ? null : remap(output);
      }
      return {};
    };
    const result = output === undefined
        ? commit(label, draft => apply(draft))
        : await routingCommit(label, (draft, { remapOwner }) => { apply(draft, remapOwner); }, channels.length === 1 ? channels[0].owner : undefined);
    const q = project();
    return { ...result, channels: channels.map(ch => describeChannel(q, ch.type === 'track' ? trackChannel(q, ch.track) : channelOf(q, ch.owner) || ch)) };
  });
  method('createBus', 'Create a group or return bus (standard routing) as one undo step: name, type ("group" default, or "return"), inputs (tracks/buses whose output '
      + 'moves to the new bus), output (default "master"), volume, pan.', ['name', 'type', 'inputs', 'output', 'volume', 'pan', 'label'], async params => {
    requireStandardRouting(project(), 'A bus');
    const type = params.type === undefined ? 'group' : params.type;
    if (!['group', 'return'].includes(type)) fail('type must be "group" or "return".');
    const caps = await routingCapabilities(), q = project();
    if (caps.buses?.[type] !== true) fail(`The current audio host cannot create ${type} buses.`);
    const limit = Math.min(LIMITS.tracks, Number.isFinite(Number(caps.buses?.max)) ? Number(caps.buses.max) : Infinity);
    if ((q.routing?.buses || []).length >= limit) fail(`This song already has ${limit} buses, the most the audio host supports.`);
    const name = params.name === undefined ? `${type === 'group' ? 'Group' : 'Return'} ${(q.routing?.buses || []).filter(bus => bus.type === type).length + 1}` : checkName(params.name);
    const inputs = params.inputs === undefined ? [] : [...new Map((Array.isArray(params.inputs) ? params.inputs : [params.inputs]).map(ref => findChannel(q, ref, 'inputs'))
        .map(ch => [ch.owner, ch])).values()];
    for (const input of inputs) {
      if (input.type === 'master') fail('Master cannot feed a bus.');
      if (input.type === 'track') requireUnfrozen(q, [input.track]);
    }
    const output = params.output === undefined ? 'master' : outputDestination(q, [], params.output);
    if (output !== null && inputs.some(input => input.owner === output)) fail('A bus cannot output to one of its own inputs.');
    const volume = params.volume === undefined ? 1 : parseValue(BUS_VOLUME, params.volume, 'volume'), pan = params.pan === undefined ? 0 : parseValue(TRACK_VALUES.pan, params.pan, 'pan');
    const id = globalThis.crypto?.randomUUID?.() || S().uid(), owner = `bus:${id}`;
    const result = await routingCommit(params.label || `create ${type} bus ${name}`, (draft, { remapOwner }) => {
      draft.routing.buses.push({ id, name, type, volume, pan, mute: false, solo: false });
      draft.routing.outputs[owner] = output === null ? null : remapOwner(output);
      for (const input of inputs) draft.routing.outputs[remapOwner(input.owner)] = owner;
    });
    const after = project(), created = channelOf(after, owner);
    if (!created) fail('The audio host did not keep the new bus.');
    return { ...result, ...describeChannel(after, created) };
  });
  method('removeBus', 'Remove a group or return bus with its inserts, sends and automation (one undo step). Channels that output to it move to the bus\'s own output.',
      ['channel', 'label'], async params => {
    const p = project(), ch = findChannel(p, params.channel, 'channel');
    if (ch.type !== 'group' && ch.type !== 'return') fail(`${channelLabel(ch)} is not a bus. removeBus removes group and return buses (see mixer).`);
    const group = (p.groups || []).find(item => item.busID === ch.bus);
    if (group) fail(`${channelLabel(ch)} belongs to the track group "${group.name}". Dissolve the group instead (show its first track, then runCommand "dissolveGroup").`);
    await routingCapabilities();
    const result = await routingCommit(params.label || `remove bus ${ch.name}`, draft => {
      // As routing.js removeBus: outputs into the bus follow its own output; its sends, sidechains, inserts and lanes go.
      const r = draft.routing, owner = ch.owner, replacement = outputOf(draft, owner),
        removed = new Set((draft.audioPlugins || []).filter(plugin => plugin.owner === owner).map(plugin => plugin.instanceID));
      r.buses = r.buses.filter(item => `bus:${item.id}` !== owner);
      delete r.outputs[owner];
      for (const source of Object.keys(r.outputs)) if (r.outputs[source] === owner) r.outputs[source] = replacement;
      const removedSends = new Set(r.sends.filter(send => send.source === owner || send.destination === owner).map(send => `send:${send.id}`));
      r.sends = r.sends.filter(send => !removedSends.has(`send:${send.id}`));
      r.sidechains = r.sidechains.filter(route => route.source !== owner && !removed.has(route.destination?.instanceID));
      draft.audioPlugins = (draft.audioPlugins || []).filter(plugin => plugin.owner !== owner);
      for (const pattern of draft.patterns || []) if (pattern.automation) pattern.automation = pattern.automation.filter(lane => lane.owner !== owner && !removedSends.has(lane.target));
    }, ch.owner);
    return { ...result, removed: ch.owner, name: ch.name };
  });
  method('setSend', 'Add, change or remove a custom send (standard routing) from a track or bus (from) to a bus or Master (to): gain ("-6 dB" or 0–2; new sends '
      + '0 dB), tap ("postFader" default, "preFader"), enabled, remove:true. id picks an existing send (see mixer).', ['from', 'to', 'id', 'gain', 'tap', 'enabled',
          'remove', 'label'], async params => {
    requireStandardRouting(project(), 'A custom send (the built-in delay/reverb sends are editTrack delaySend/reverbSend)');
    const caps = await routingCapabilities(), q = project(), sends = q.routing?.sends || [];
    if (caps.sends?.enabled !== true) fail('Custom sends are not available from the current audio host.');
    const taps = (caps.sends?.taps || []).filter(tap => ['preFader', 'postFader'].includes(tap));
    let send = null;
    if (params.id !== undefined) {
      send = sends.find(item => item.id === params.id || `send:${item.id}` === params.id);
      if (!send) fail(`There is no send "${params.id}". Sends: ${sends.map(item => `${item.id} (${channelOf(q, item.source)?.name} → ${channelOf(q, item.destination)?.name})`).join(', ') || 'none'}.`);
    }
    const source = params.from !== undefined ? findChannel(q, params.from, 'from') : send ? channelOf(q, send.source) : fail('from is required: the track or bus that sends.');
    if (source.type === 'master') fail('Master cannot feed a send.');
    if (send && send.source !== source.owner) fail('A send keeps its source. Remove it and add a new send from the other channel.');
    const destination = params.to !== undefined ? findChannel(q, params.to, 'to') : send ? channelOf(q, send.destination) : fail('to is required: the bus or "master" that receives.');
    if (!send) send = sends.find(item => item.source === source.owner && item.destination === destination.owner) || null;
    const label = params.label || `${params.remove !== undefined ? 'remove' : send ? 'edit' : 'add'} send ${source.name} → ${destination.name}`;
    if (params.remove !== undefined) {
      if (params.remove !== true) fail('remove must be true.');
      if (!send) fail(`There is no send from ${channelLabel(source)} to ${channelLabel(destination)}.`);
      const id = send.id;
      const result = await routingCommit(label, draft => {
        draft.routing.sends = draft.routing.sends.filter(item => item.id !== id);
        for (const pattern of draft.patterns || []) if (pattern.automation) pattern.automation = pattern.automation.filter(lane => lane.target !== `send:${id}`);
      }, source.owner);
      return { ...result, removed: id };
    }
    if (!send || send.destination !== destination.owner) {
      const allowed = window.STRouting.destinationOptions(source.owner, q, send ? { omitID: send.id } : {});
      if (!allowed.some(item => item.owner === destination.owner)) fail(`${channelLabel(source)} cannot send to ${channelLabel(destination)}${destination.type === 'track'
          ? ': sends go to a group or return bus or to Master'
          : ': that would create a feedback loop'}. Valid destinations: ${allowed.map(item => item.owner === 'master' ? 'master' : `"${item.name}" (${item.owner})`).join(', ') || 'none'}.`);
    }
    if (!send && sends.length + (q.routing?.sidechains || []).length >= Math.min(128, Number(caps.sends?.max) || 128)) fail('The song has reached its send and sidechain limit.');
    if (!send && !taps.length) fail('The audio host offers no send taps.');
    const changes = {};
    if (params.gain !== undefined) changes.gain = parseValue(SEND_GAIN, params.gain, 'gain');
    if (params.tap !== undefined) { if (!taps.includes(params.tap)) fail(`tap must be one of ${taps.join(', ')}.`); changes.tap = params.tap; }
    if (params.enabled !== undefined) { if (typeof params.enabled !== 'boolean') fail('enabled must be true or false.'); changes.enabled = params.enabled; }
    if (send && send.destination !== destination.owner) changes.destination = destination.owner;
    if (send && !Object.keys(changes).length) fail('Nothing to change. Give gain, tap, enabled, to, or remove:true.');
    const id = send?.id || globalThis.crypto?.randomUUID?.() || S().uid();
    const result = await routingCommit(label, (draft, { remapOwner }) => {
      let item = draft.routing.sends.find(x => x.id === id);
      if (!item) draft.routing.sends.push(item = { id, source: remapOwner(source.owner), tap: taps.includes('postFader') ? 'postFader' : taps[0],
        destination: remapOwner(destination.owner), gain: 1, enabled: true });
      Object.assign(item, changes, changes.destination ? { destination: remapOwner(changes.destination) } : {});
    }, source.owner);
    const after = project(), stored = (after.routing?.sends || []).find(item => item.id === id);
    return { ...result, send: stored ? describeSend(after, stored) : null };
  });
  method('setSendEffects', 'Built-in send effects (fed by each track\'s delaySend/reverbSend): delay {time ("375 ms", seconds, or a tempo division like "1/8."), '
      + 'feedback (0–95%), tone (0–100%, 100% = open), mix (return level)} and reverb {size, damping (0–99%), predelay (0–250 ms), mix}.', ['delay', 'reverb',
          'label'], params => {
    const model = window.STSends; if (!model) fail('The send effects are unavailable.');
    const SPECS = {
      delay: { feedback: { min: 0, max: .95, kind: 'pct' }, tone: { min: 0, max: 1, kind: 'pct' }, mix: { min: 0, max: 1, kind: 'pct' } },
      reverb: { size: { min: 0, max: 1, kind: 'pct' }, damping: { min: 0, max: .99, kind: 'pct' }, predelay: { min: 0, max: .25, kind: 'time' }, mix: { min: 0, max: 1, kind: 'pct' } }
    };
    const steps = [];
    for (const group of ['delay', 'reverb']) {
      const values = params[group]; if (values === undefined) continue;
      if (!values || typeof values !== 'object' || Array.isArray(values)) fail(`${group} must be an object like ${group === 'delay' ? '{"time":"1/8.","feedback":"40%"}' : '{"size":"80%","mix":"30%"}'}.`);
      for (const [key, raw] of Object.entries(values)) {
        if (group === 'delay' && key === 'time') {
          const division = typeof raw === 'string' && model.DIVISIONS.find(([label]) => label === raw.trim());
          if (division) { steps.push(p => model.setDivision(p, division[0])); continue; }
          const seconds = parseValue({ min: .001, max: 3.99, kind: 'time' }, raw, 'delay.time');
          steps.push(p => model.setFreeTime(p, seconds));
          continue;
        }
        const spec = ownValue(SPECS[group], key);
        if (!spec) fail(`Unknown ${group} setting "${key}". ${group}: ${[...(group === 'delay' ? ['time'] : []), ...Object.keys(SPECS[group])].join(', ')}${group === 'delay'
            ? ` (time divisions: ${model.DIVISIONS.map(([label]) => label).join(', ')})`
            : ''}.`);
        const value = parseValue(spec, raw, `${group}.${key}`);
        steps.push(p => { p[group] = { ...(group === 'delay' ? { time: .375, feedback: .35, mix: .3 } : { size: .7, damping: .4, mix: .25 }), ...(p[group] || {}), [key]: value }; });
      }
    }
    if (!steps.length) fail('Nothing to change. Give delay and/or reverb settings.');
    const result = commit(params.label || `send effects (${['delay', 'reverb'].filter(group => params[group] !== undefined).join(', ')})`, draft => { for (const step of steps) step(draft); });
    return { ...result, sendEffects: sendEffectsOut(project()) };
  });
  method('editInsert', 'Bypass, remove or reorder an insert effect on a track, bus or master (channel, or track) by slot: exactly one of bypass (true/false), remove '
      + '(true; also removes its automation) or moveTo (the slot of another insert, "first" or "last"; the others shift). One undo step.', ['channel', 'track', 'slot', 'bypass',
          'remove', 'moveTo', 'label'], async params => {
    const p = project(), ch = pluginChannel(p, params, { required: true });
    if (!isInt(params.slot) || params.slot < 0 || params.slot >= LIMITS.effectSlots) fail(`slot must be the insert slot 0–${LIMITS.effectSlots - 1} (see mixer).`);
    const actions = ['bypass', 'remove', 'moveTo'].filter(key => params[key] !== undefined);
    if (actions.length !== 1) fail('Give exactly one of bypass (true/false), remove (true) or moveTo (a slot, "first" or "last").');
    const action = actions[0];
    if (ch.type !== 'track') requireStandardRouting(p, `${channelLabel(ch)} inserts`); else requireUnfrozen(p, [ch.track]);
    const chain = channelEffects(p, ch), plugin = chain.find(item => (item.slot ?? 0) === params.slot);
    if (!plugin) fail(`${channelLabel(ch)} has no insert in slot ${params.slot}. Inserts: ${chain.map(item => `${item.slot ?? 0} ${item.name}`).join(', ') || 'none'}.`);
    if (action === 'bypass' && typeof params.bypass !== 'boolean') fail('bypass must be true or false.');
    if (action === 'remove' && params.remove !== true) fail('remove must be true.');
    let toSlot = null;
    if (action === 'moveTo') {
      toSlot = params.moveTo === 'first' ? chain[0].slot ?? 0 : params.moveTo === 'last' ? chain[chain.length - 1].slot ?? 0 : params.moveTo;
      if (!chain.some(item => (item.slot ?? 0) === toSlot)) fail(`moveTo must be "first", "last" or the slot of another insert: ${chain.map(item => item.slot ?? 0).join(', ')}.`);
    }
    const summary = () => ({ channel: ch.owner, channelName: ch.name, inserts: channelEffects(project(), ch).map(insertOut) });
    if (action === 'bypass' ? !!plugin.bypass === params.bypass : action === 'moveTo' && toSlot === params.slot) return { changed: false, ...summary() };
    const inserts = window.STInserts; if (!inserts) fail('The mixer insert interface is unavailable.');
    const busy = blockedByBusyUI() || (inserts.busy ? 'another plug-in or routing change is in progress' : null);
    if (busy) fail(`Cannot edit inserts while ${busy}.`);
    const label = params.label || (action === 'bypass'
        ? `${params.bypass ? 'bypass' : 'enable'} ${plugin.name} on ${ch.name}`
        : action === 'remove'
        ? `remove ${plugin.name} from ${ch.name}`
        : `move ${plugin.name} on ${ch.name}`), options = { label: `AI: ${label}` };
    const before = st().undo[st().undo.length - 1];
    let done;
    try {
      done = action === 'bypass'
          ? await inserts.setBypass(ch.owner, params.slot, params.bypass, options) !== null
          : action === 'remove'
          ? await inserts.remove(ch.owner, params.slot, options) === true
          : await inserts.move(ch.owner, params.slot, toSlot, options) === true;
    } catch (error) { fail(`The insert edit was not applied: ${error.message || error}`); }
    if (!done || st().undo[st().undo.length - 1] === before) fail(`The insert edit was not applied${toastText() ? ': ' + toastText() : '.'}`);
    return { changed: true, undoLabel: options.label, ...summary() };
  });

  // ---------------------------------------------------------------- performance (chords / scales)
  method('setPerformance', 'Keyboard performance settings: scale, root, chord, chordSet ("" = off), scaleMode ("snap"/"degrees"), octave (0–8). Saved with the song; '
      + 'not an undo step.', ['scale', 'root', 'chord', 'chordSet', 'scaleMode', 'octave'], params => {
    const changes = {}, scales = S().scales || {}, chords = S().chords || {}, sets = S().chordSets || [];
    const pick = (list, value, what) => {
      const key = list.find(k => k.toLowerCase() === String(value).toLowerCase());
      if (!key) fail(`${what} must be one of ${list.join(', ')}.`);
      return key;
    };
    if (params.scale !== undefined) changes.scale = pick(Object.keys(scales), params.scale, 'scale');
    if (params.root !== undefined) changes.root = pitchClass(params.root);
    if (params.chord !== undefined) { changes.chord = pick(Object.keys(chords), params.chord, 'chord'); changes.chordSet = ''; }
    if (params.chordSet !== undefined) changes.chordSet = params.chordSet === '' || params.chordSet === null
        ? ''
        : pick(sets.map(set => set.id), params.chordSet, 'chordSet');
    if (params.scaleMode !== undefined) changes.scaleMode = pick(['snap', 'degrees'], params.scaleMode, 'scaleMode');
    if (params.octave !== undefined) {
      if (!isInt(params.octave) || params.octave < 0 || params.octave > 8) fail('octave must be 0–8.');
      changes.octave = params.octave;
    }
    if (!Object.keys(changes).length) return { options: { scales: Object.keys(scales), chords: Object.fromEntries(Object.entries(chords).map(([k, v]) => [k, v.notes])), chordSets: sets.map(set => ({ id: set.id, name: set.name, mood: set.mood, minor: !!set.minor })), scaleModes: ['snap', 'degrees'] }, current: S().performanceSummary?.() };
    S().setPerformance(changes, false);
    return { changed: true, performance: S().performanceSummary?.() };
  });

  // ---------------------------------------------------------------- transport
  function transportStatus() {
    const s = st(), p = project(), status = s.status || {};
    return { playing: !!s.playing, mode: s.playbackMode, pattern: s.playing
        ? status.pattern ?? p.activePattern
        : p.activePattern, row: s.playing ? status.row ?? s.row : s.row, sequence: s.playing
            ? status.sequence ?? s.sequence
            : s.sequence, seconds: Number.isFinite(status.seconds)
        ? round(status.seconds, 3)
        : null, patternLoop: s.patternLoop || null, recording: !!s.recording };
  }
  const waitFor = async (condition, ms = 3000) => {
    const start = Date.now();
    while (!condition() && Date.now() - start < ms) await new Promise(resolve => setTimeout(resolve, 25));
    return condition();
  };
  method('transport', 'Transport: action "play" (current mode), "playPattern", "playSong", "stop" or "status". Options: pattern, row (pattern start row), block '
      + '(song start block), loop {from,to} rows for pattern playback (null clears).', ['action', 'pattern', 'row', 'block',
          'loop'], async params => {
    const action = params.action || 'status', p = project();
    if (!['play', 'playPattern', 'playSong', 'stop', 'status'].includes(action)) fail('action must be play, playPattern, playSong, stop or status.');
    if (action === 'status') return transportStatus();
    if (action === 'stop') { await S().perform('stop'); await waitFor(() => !st().playing, 2000); return transportStatus(); }
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot start playback while ${busy}.`);
    const mode = action === 'playSong' ? 'song' : action === 'playPattern' ? 'pattern' : st().playbackMode;
    const pat = params.pattern === undefined ? activePattern(p) : findPattern(p, params.pattern), sequence = p.sequence.slice();
    const row = params.row === undefined ? (params.pattern !== undefined && p.activePattern !== pat.id ? 0 : st().row) : params.row;
    if (!isInt(row) || row < 0 || row >= pat.length) fail(`row must be 0–${pat.length - 1}.`);
    let blockPosition = st().sequence, loop = st().patternLoop;
    if (params.block !== undefined) { const blocks = window.STSongModel.blocks(sequence); if (!isInt(params.block)
        || params.block < 0
        || params.block >= blocks.length) fail(`block must be 0–${blocks.length - 1}.`); blockPosition = blocks[params.block].start; }
    else if (mode === 'song' && params.pattern !== undefined) { const at = sequence.indexOf(pat.id); if (at >= 0) blockPosition = at; }
    if (params.loop === null) loop = null;
    else if (params.loop !== undefined) { if (typeof params.loop !== 'object'
        || Array.isArray(params.loop)) fail('loop must be {from,to} or null.'); const [from, to] = rowRange(pat, params.loop.from,
            params.loop.to); loop = { pattern: pat.id, start: from, end: to }; }
    // Validate the complete request before stopping or changing any selection.
    if (st().playing) { if (await S().perform('stop') === false
        || !await waitFor(() => !st().playing, 2000)) fail('Playback stop could not be confirmed.'); }
    if (st().project !== p
        || p.patterns.find(x => x.id === pat.id) !== pat
        || row >= pat.length
        || stable(p.sequence) !== stable(sequence)) fail('The project or playback selection changed while stopping. Press Play again.');
    if (mode !== st().playbackMode
        && S().perform(mode === 'song' ? 'modeSong' : 'modePattern') === false) fail('Could not change the playback mode right now.');
    if (p.activePattern !== pat.id) { p.activePattern = pat.id; S().syncEngine?.(); }
    st().row = row; st().sequence = blockPosition; st().patternLoop = loop;
    refreshUI();
    if (await S().perform('play') === false) fail('Playback could not start.');
    if (st().native) await waitFor(() => st().playing, 4000);
    const status = transportStatus();
    if (st().native && !status.playing) fail(`Playback did not start${toastText() ? ': ' + toastText() : '.'}`);
    return status;
  });

  // ---------------------------------------------------------------- history, files, commands, UI
  function isTextField(el) {
    return !!el
        && (el.isContentEditable || el.tagName === 'TEXTAREA' || el.tagName === 'INPUT'
            && !['range', 'checkbox', 'radio', 'button', 'submit', 'color', 'file'].includes(String(el.type || '').toLowerCase()));
  }
  async function history(redo, steps) {
    const count = steps === undefined ? 1 : steps; if (!isInt(count) || count < 1 || count > 90) fail('steps must be 1–90.');
    const labels = [];
    for (let k = 0; k < count; k++) {
      const stack = redo ? st().redo : st().undo; if (!stack.length) break;
      const label = stack[stack.length - 1].label, length = stack.length;
      if (isTextField(document.activeElement)) document.activeElement.blur?.();
      await S().perform(redo ? 'redo' : 'undo');
      if ((redo ? st().redo : st().undo).length === length) fail(`${redo ? 'Redo' : 'Undo'} is not available right now${toastText() ? ': ' + toastText() : '.'}`);
      labels.push(label);
    }
    if (!labels.length) fail(redo ? 'Nothing to redo.' : 'Nothing to undo.');
    return { [redo ? 'redid' : 'undid']: labels, undo: st().undo.length, redo: st().redo.length, next: (redo ? st().redo : st().undo).slice(-1)[0]?.label
        || null };
  }
  method('undo', 'Undo the last edit(s) (yours or the user\'s), like ⌘Z.', ['steps'], params => history(false, params.steps));
  method('redo', 'Redo, like ⌘⇧Z.', ['steps'], params => history(true, params.steps));
  method('history', 'List recent undo/redo labels.', ['limit'], params => {
    const limit = params.limit ?? 20;
    if (!isInt(limit) || limit < 1 || limit > 90) fail(`limit must be a whole number from 1 to 90 (got ${JSON.stringify(params.limit)}).`);
    return { undo: st().undo.slice(-limit).map(x => x.label).reverse(), redo: st().redo.slice(-limit).map(x => x.label).reverse() };
  });
  method('save', 'Save the project (to its current file, or to path as .supertracker).', ['path', '_host'], async params => {
    if (params.path !== undefined
        && (typeof params.path !== 'string' || !params.path.startsWith('/') || !/\.supertracker$/i.test(params.path))) fail('path must be an absolute path ending in .supertracker.');
    const ok = await S().projectAction(params.path ? 'saveAs' : 'save', params.path);
    if (!ok) fail(`The project was not saved${toastText() ? ': ' + toastText() : '.'}`);
    return { saved: true, path: params.path || params._host?.projectPath || null, name: project().name };
  });
  method('exportAudio', 'Render the song or the current pattern to a WAV file at path (absolute, .wav).', ['path', 'mode', 'pattern', 'tail'], async params => {
    const mode = params.mode || 'song'; if (!['song', 'pattern'].includes(mode)) fail('mode must be "song" or "pattern".');
    if (typeof params.path !== 'string'
        || !params.path.startsWith('/')
        || !/\.wav$/i.test(params.path)) fail('path must be an absolute path ending in .wav.');
    const tail = params.tail ?? 3; if (!Number.isFinite(tail) || tail < 0 || tail > 30) fail('tail must be 0–30 seconds.');
    if (!st().native) fail('Audio export runs only in the macOS app.');
    if (window.STBusy.isBusy('export')) fail('An export is already running.');
    const busy = blockedByBusyUI(); if (busy) fail(`Cannot export while ${busy}.`);
    const p = project(), pat = params.pattern === undefined ? activePattern(p) : findPattern(p, params.pattern);
    if (typeof window.STExport?.exportTo !== 'function') fail('The audio export interface is unavailable. Nothing was exported.');
    // Export owns preparation, the edit lock, cancellation and completion checks.
    // Delegate immediately so it captures this exact project/pattern before waiting.
    let result;
    try { result = await window.STExport.exportTo({ mode, path: params.path, tail, pattern: pat.id }); }
    catch (error) { fail(`Export failed: ${error.message || error}`); }
    return { exported: true, mode, path: result.path || result.paths[0], paths: result.paths, ...(result.warning
        ? { warning: result.warning }
        : {}) };
  });
  function commandList() { return (S().commands || []).map(([id, label, shortcut]) => ({ id, label, ...(shortcut ? { shortcut } : {}) })); }
  method('listCommands', 'List command-palette commands (optionally filtered by query words).', ['query'], params => {
    const words = String(params.query || '').toLowerCase().split(/\s+/).filter(Boolean);
    const list = commandList().filter(item => words.every(word => `${item.id} ${item.label}`.toLowerCase().includes(word)));
    return { total: list.length, commands: list };
  });
  method('runCommand', 'Run a command-palette command by id (see listCommands). It acts on the current selection, like choosing it in ⌘K.', ['id'], async params => {
    const item = commandList().find(x => x.id === params.id);
    if (!item) fail(`Unknown command "${params.id}". Use listCommands to find ids.`);
    if (isTextField(document.activeElement) && ['undo', 'redo', 'selectAll'].includes(item.id)) document.activeElement.blur?.();
    const value = await S().perform(item.id);
    return { ran: item.id, label: item.label, ...(value === false
        ? { note: toastText() || 'The command reported that it could not run right now.' }
        : {}), ui: api.uiState({}) };
  });
  const COLUMNS = ['note', 'instrument', 'velocity', 'fx1', 'fx2'];
  method('uiState', 'Describe what the user sees: view, open dialog, pattern, cursor track/row/column, selection, instrument, transport.', [], () => {
    const s = st(), p = project(), pat = activePattern(p), sel = S().selectedBounds?.();
    return {
      view: s.view, dialog: s.modal || null, pattern: { id: pat.id, name: pat.name, length: pat.length },
      cursor: {
        track: s.track,
        trackName: p.tracks[s.track]?.name,
        row: s.row,
        column: COLUMNS[s.column] || s.column,
        ...(window.STApp?.cursorNoteColumn?.() ? { noteColumn: window.STApp.cursorNoteColumn() } : {}),
        step: pat.steps[s.track]?.[s.row] ? describeStep(pat.steps[s.track][s.row], s.row) : null
      },
      selection: sel ? { tracks: [sel.t0, sel.t1], rows: [sel.r0, sel.r1] } : null,
      instrument: { index: s.instrument, name: p.instruments[s.instrument]?.name }, songPosition: s.sequence,
      transport: transportStatus(),
      octave: s.octave,
      editStep: s.editStep,
      keyboardPiano: !!s.keyboardPlay,
      follow: !!s.follow,
      performance: S().performanceSummary?.()
    };
  });
  const VIEWS = ['pattern', 'instrument', 'browser', 'mixer', 'automation', 'song'];
  method('show', 'Move the user\'s view: view, pattern, track, row, column, instrument, select {fromTrack,toTrack,fromRow,toRow}, channel (selects a mixer channel and '
      + 'shows the mixer unless view is given) (no undo step).', ['view', 'pattern', 'track', 'row', 'column', 'instrument', 'select', 'channel'], params => {
    const s = st(), p = project();
    if (params.view !== undefined && !VIEWS.includes(params.view)) fail(`view must be one of ${VIEWS.join(', ')}.`);
    // Validate the whole request before moving anything: a refused call leaves the cursor and pattern where they were.
    const shown = params.pattern === undefined ? null : findPattern(p, params.pattern), pat = shown || activePattern(p);
    const track = params.track === undefined ? null : findTrack(p, params.track);
    if (params.row !== undefined && (!isInt(params.row) || params.row < 0 || params.row >= pat.length)) fail(`row must be 0–${pat.length - 1}.`);
    let column = null;
    if (params.column !== undefined) {
      column = typeof params.column === 'number' ? params.column : COLUMNS.indexOf(params.column);
      if (!isInt(column) || column < 0 || column > 4) fail(`column must be one of ${COLUMNS.join(', ')}.`);
    }
    const instrument = params.instrument === undefined ? null : findInstrument(p, params.instrument);
    let selection = null;
    if (params.select !== undefined && params.select !== null) {
      const sel = params.select;
      if (typeof sel !== 'object' || Array.isArray(sel)) fail('select must be {fromTrack, toTrack, fromRow, toRow} or null.');
      const t0 = findTrack(p, sel.fromTrack ?? track ?? s.track), t1 = findTrack(p, sel.toTrack ?? t0), [r0, r1] = rowRange(pat, sel.fromRow, sel.toRow);
      selection = { t0, t1, r0, r1 };
    }
    const channel = params.channel === undefined ? null : findChannel(p, params.channel);
    if (channel && channel.type !== 'track') requireStandardRouting(p, `Showing ${channelLabel(channel)}`);
    if (shown && p.activePattern !== shown.id) {
      p.activePattern = shown.id;
      s.row = 0;
      s.selection = null;
      if (s.patternLoop && s.patternLoop.pattern !== shown.id) s.patternLoop = null;
      S().syncEngine?.();
    }
    if (track !== null) { s.track = track; s.instrument = p.tracks[track].instrument ?? 0; }
    if (params.row !== undefined) s.row = params.row;
    if (column !== null) s.column = column;
    if (instrument !== null) s.instrument = instrument;
    if (params.select === null) s.selection = null;
    else if (selection) { s.selection = { track: selection.t0, row: selection.r0 }; s.track = selection.t1; s.row = selection.r1; }
    let view = params.view;
    if (channel) {
      if (window.STStudio?.selectOwner?.(channel.owner) === false) { refreshUI(); fail(`${channelLabel(channel)} cannot be selected in the mixer.`); }
      view ??= 'mixer';
    }
    if (view !== undefined && view !== s.view) S().setView(view); else refreshUI();
    return api.uiState({});
  });

  // ---------------------------------------------------------------- dispatch (serialised)
  const KEYS = Object.fromEntries(Object.entries(docs).map(([name, info]) => [name, new Set([...info.params, '_host'])]));
  let tail = Promise.resolve(), busyCount = 0, terminationSuspended = false, admissionGeneration = 0;
  async function run(name, params) {
    if (terminationSuspended) fail('SuperTracker is preparing to quit. Cancel quitting before sending more AI commands.');
    if (!Object.hasOwn(api, name)) fail(`Unknown method "${name}". Methods: ${Object.keys(api).join(', ')}.`);
    if (params === undefined || params === null) params = {};
    if (typeof params !== 'object' || Array.isArray(params)) fail('params must be a JSON object.');
    for (const key of Object.keys(params)) if (!KEYS[name].has(key)) fail(`Unknown parameter "${key}" for ${name}. Parameters: ${docs[name].params.filter(k => k !== '_host').join(', ') || 'none'}.`);
    return api[name](params);
  }
  /* Entry point for the native control socket. Returns a JSON string {ok, result|error, notices}. */
  function dispatch(name, paramsJSON) {
    const admitted = !terminationSuspended, admission = admissionGeneration;
    const job = tail.then(async () => {
      const notices = [], app = S(), originalToast = app?.toast;
      if (app && originalToast) app.toast = (message, type) => {
        notices.push(type === 'error' ? `Error: ${message}` : String(message));
        return originalToast(message, type);
      };
      setBusy(+1);
      try {
        if (!admitted
            || admission !== admissionGeneration) fail('The AI command was canceled because SuperTracker started quitting. Send it again after canceling Quit.');
        // Only this parse reads the caller's text: a SyntaxError from inside a method is an internal error, not bad params.
        let params = paramsJSON;
        if (typeof paramsJSON === 'string') { try { params = paramsJSON ? JSON.parse(paramsJSON) : {}; } catch (error) { fail(`params are not valid JSON: ${error.message}`); } }
        const result = await run(name, params);
        return JSON.stringify({ ok: true, result: result === undefined ? null : result, ...(notices.length ? { notices } : {}) });
      } catch (error) {
        const message = error instanceof AgentError
            ? error.message
            : `Internal error in ${name}: ${error?.message || error}`;
        return JSON.stringify({ ok: false, error: message, ...(notices.length ? { notices } : {}) });
      } finally {
        if (app && originalToast) app.toast = originalToast;
        setBusy(-1);
      }
    });
    tail = job.catch(() => {});
    return job;
  }
  async function suspendForTermination(timeoutMilliseconds = 3000) {
    terminationSuspended = true; admissionGeneration++;
    // Reject queued commands when their turn arrives; allow only the operation
    // already running to settle. The timeout never silently admits further work.
    const issued = tail;
    let timer;
    try { return await Promise.race([issued.then(() => true, () => true),
        new Promise(resolve => { timer = setTimeout(() => resolve(false), timeoutMilliseconds); })]); }
    finally { clearTimeout(timer); }
  }
  function resumeAfterTermination() { terminationSuspended = false; }

  // ---------------------------------------------------------------- status bar indicator and settings toggle
  let connection = { available: false, enabled: true, clients: 0 };
  function badgeText() { return busyCount > 0 ? 'AI editing…' : 'AI connected'; }
  function statusBadge() {
    const visible = connection.enabled && connection.clients > 0;
    return `<span id="agent-status" class="agent-status${busyCount > 0 ? ' busy' : ''}" title="An AI assistant is connected through SuperTracker's `
        + `local MCP control socket. Its edits appear here and undo with ⌘Z. Turn it off in Settings." ${visible ? '' : 'hidden'}>`
        + `<i class="agent-led"></i>${badgeText()}</span>`;
  }
  function paintBadge() {
    const el = typeof document !== 'undefined' && document.getElementById?.('agent-status'); if (!el) return;
    el.hidden = !(connection.enabled && connection.clients > 0);
    el.classList?.toggle?.('busy', busyCount > 0);
    const led = '<i class="agent-led"></i>'; el.innerHTML = led + badgeText();
  }
  function setBusy(delta) { busyCount = Math.max(0, busyCount + delta); paintBadge(); }
  function setConnection(status) {
    if (status && typeof status === 'object') connection = { ...connection, ...status, available: true };
    paintBadge();
    paintSettings();
  }
  function settingsField() {
    return `<div class="agent-settings"><div class="eyebrow">AI assistant</div><label class="row agent-toggle"><input type="checkbox"`
        + ` id="agent-enabled" ${connection.enabled ? 'checked' : ''} ${connection.available ? '' : 'disabled'}> Allow AI control (MCP)</label>`
        + `<p class="small-note" id="agent-settings-note">${settingsNote()}</p></div>`;
  }
  function settingsNote() {
    if (!connection.available) return 'Available in the macOS app.';
    if (!connection.enabled) return 'Off. Claude and other MCP clients cannot read or edit your songs.';
    if (connection.error) return S().escapeHTML(`On, but the control socket is unavailable: ${connection.error}`);
    return `On. MCP clients on this Mac (run <code>SuperTracker --mcp</code>) can read and edit the open song; every edit is undoable. ${connection.clients ? `${connection.clients} connected.` : 'None connected.'}`
        + ` Applies immediately.`;
  }
  function paintSettings() {
    const box = typeof document !== 'undefined' && document.getElementById?.('agent-enabled'),
      note = typeof document !== 'undefined' && document.getElementById?.('agent-settings-note');
    if (box) { box.checked = !!connection.enabled; box.disabled = !connection.available; }
    if (note) note.innerHTML = settingsNote();
  }
  function bindSettings() {
    const box = document.getElementById?.('agent-enabled'); if (!box) return;
    box.onchange = async () => {
      const enabled = box.checked; box.disabled = true;
      try { setConnection(await S().request('agent.configure', { enabled })); S().toast(enabled ? 'AI control allowed' : 'AI control turned off'); }
      catch (error) { box.checked = !enabled; S().toast(`Could not change AI control: ${error.message}`, 'error'); }
      finally { box.disabled = !connection.available; }
    };
    refreshConnection();
  }
  async function refreshConnection() {
    if (!st()?.native) return;
    try { setConnection(await S().request('agent.status')); } catch { /* older host */ }
  }
  if (typeof window !== 'undefined') {
    window.STPendingEdits.register('agent', {
      order: 40,
      hasPending: () => !!window.STAgent?.hasPendingEdits,
      generation: () => window.STAgent?.editGeneration ?? 0,
      flush: () => window.STAgent?.flushEdits?.()
    });
    window.STAgent = globalThis.STAgent = {
      dispatch,
      call: (name, params) => dispatch(name, params === undefined ? '{}' : JSON.stringify(params)).then(JSON.parse),
      methods: () => Object.keys(api),
      docs: () => clone(docs),
      setConnection,
      statusBadge,
      settingsField,
      bindSettings,
      refreshConnection,
      suspendForTermination,
      resumeAfterTermination,
      flushEdits: flushParameterEdits,
      get hasPendingEdits() {
        return pendingParameterEdits();
      },
      get editGeneration() {
        return parameterGeneration;
      },
      _test: { parseNote, noteName, parseFX, formatFX, parseValue, formatValue, commit }
    };
    setTimeout(() => { refreshConnection(); }, 0);
  }
})();
