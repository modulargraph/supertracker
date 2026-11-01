/* MK3: instrument parameter knobs and step locks (LOCK + encoders). */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, clamp01, FX_NAMES, pattern, flash, stepAt, arRef, burstValue, burstEdit, toNorm, fromNorm,
    numKnob, intKnob, enumKnob, fmt
  } = mk;

  // ---- Instrument parameters (Polyend-style groups) ---------------------------------------------------
  function tuneParts(inst) {
    const tune = clamp(Number(inst?.tune) || 0, -48, 48);
    let semis = Math.trunc(tune),
      cents = Math.round((tune - semis) * 100);
    if (Math.abs(cents) >= 100) {
      semis += Math.sign(cents);
      cents = 0;
    }
    return { semis, cents };
  }
  function applyInst(inst, key, value) {
    if (key === 'tuneSemi') { inst.tune = clamp(clamp(Math.round(value), -48, 48) + tuneParts(inst).cents / 100, -48, 48); return; }
    if (key === 'tuneFine') { inst.tune = clamp(tuneParts(inst).semis + clamp(Math.round(value), -99, 99) / 100, -48, 48); return; }
    inst[key] = value;
    if (key === 'start') inst.start = Math.min(inst.start, inst.end - .001); if (key === 'end') inst.end = Math.max(inst.end, inst.start + .001);
    if (key === 'loopStart') inst.loopStart = Math.min(inst.loopStart, inst.loopEnd - .001);
    if (key === 'loopEnd') inst.loopEnd = Math.max(inst.loopEnd, inst.loopStart + .001);
  }
  const INST = {
    volume: ['Volume', 0, 1, 'lin', fmt.db, 'instrumentVolume', 'volume'], pan: ['Pan', -1, 1, 'lin', fmt.pan, 'instrumentPan', 'pan'],
    cutoff: ['Cutoff', 20, 20000, 'log', fmt.hz, 'cutoff', 'cutoff'], resonance: ['Reso', 0, .95, 'lin', fmt.pct, 'resonance', 'resonance'],
    drive: ['Drive', 0, 1, 'lin', fmt.pct, 'drive'],
    attack: ['Attack', 0, 60, 'log', fmt.time, 'attack'],
    decay: ['Decay', 0, 60, 'log', fmt.time, 'decay'],
    sustain: ['Sustain', 0, 1, 'lin', fmt.pct, 'sustain'], release: ['Release', .0001, 60, 'log', fmt.time, 'release'],
    filterAttack: ['F.Attack', 0, 60, 'log', fmt.time],
    filterDecay: ['F.Decay', 0, 60, 'log', fmt.time],
    filterSustain: ['F.Sustain', 0, 1, 'lin', fmt.pct],
    filterRelease: ['F.Release', .0001, 60, 'log', fmt.time],
    filterEnvAmount: ['Env amt', -48, 48, 'lin', fmt.semi],
    lfoRate: ['LFO1 rate', 0, 40, 'lin', fmt.rate, 'lfoRate'],
    lfoDepth: ['LFO1 depth', 0, 1, 'lin', fmt.pct],
    lfo2Rate: ['LFO2 rate', 0, 40, 'lin', fmt.rate],
    lfo2Depth: ['LFO2 depth', 0, 1, 'lin', fmt.pct],
    lfoBeats: ['LFO1 cycle', 1 / 64, 64, 'log', fmt.beats, 'lfoBeats'],
    lfo2Beats: ['LFO2 cycle', 1 / 64, 64, 'log', fmt.beats, 'lfo2Beats'],
    start: ['Start', 0, .999, 'lin', fmt.pct],
    end: ['End', .001, 1, 'lin', fmt.pct],
    loopStart: ['Loop start', 0, .999, 'lin', fmt.pct],
    loopEnd: ['Loop end', .001, 1, 'lin', fmt.pct],
    grainPosition: ['Position', 0, 1, 'lin', fmt.pct, 'grainPosition', 'position'], grainSize: ['Grain size', .002, .5, 'log', fmt.time, 'grainSize'],
    grainSpray: ['Spray', 0, 1, 'log', fmt.time, 'grainSpray'], grainPitchSpread: ['Pitch rnd', 0, 24, 'lin', fmt.semi, 'grainPitchSpread'],
    grainScan: ['Scan', -4, 4, 'lin', v => `${Number(v.toFixed(2))}×`, 'grainScan'], grainReverse: ['Reverse', 0, 1, 'lin', fmt.pct, 'grainReverse'],
    grainStereo: ['Stereo', 0, 1, 'lin', fmt.pct, 'grainStereo'],
    wavetablePosition: ['Position', 0, 1, 'lin', fmt.pct, 'wavetablePosition', 'position']
  };
  const INT_PARAMS = {
    tuneSemi: ['Tune', -48, 48, fmt.semi],
    tuneFine: ['Fine', -99, 99, fmt.cents],
    bitDepth: ['Bits', 2, 16, fmt.bits],
    rootNote: ['Root', 0, 127, fmt.note],
    sliceCount: ['Slices', 1, 128, fmt.int],
    grainDensity: ['Density', 1, 8, v => `${v} grains`],
    midiChannel: ['MIDI out ch', 1, 16, fmt.int],
    midiInputChannel: ['MIDI in ch', 0, 16, v => v ? String(v) : 'Omni']
  };
  const ENUM_PARAMS = {
    filterType: ['Filter', [['lowpass', 'Low-pass'], ['highpass', 'High-pass'], ['bandpass', 'Band-pass']]],
    wavetableFrameSize: ['Frame size', [[0, 'Auto'], ...[256, 512, 1024, 2048, 4096, 8192].map(size => [size, String(size)])]],
    mode: ['Play mode',
        [['oneshot', '1-Shot'], ['forward', 'Fwd loop'], ['reverse', 'Bwd loop'], ['pingpong', 'Ping-pong'], ['slice', 'Slice'],
            ['wavetable', 'Wavetable'], ['granular', 'Granular']]],
    lfoTarget: ['LFO1 dest', [['cutoff', 'Cutoff'], ['pitch', 'Pitch'], ['level', 'Level'], ['pan', 'Pan'], ['position', 'Position'], ['grainSize', 'Grain size']]],
    lfo2Target: ['LFO2 dest', [['cutoff', 'Cutoff'], ['pitch', 'Pitch'], ['level', 'Level'], ['pan', 'Pan'], ['position', 'Position'], ['grainSize', 'Grain size']]],
    lfoShape: ['LFO1 shape', [['sine', 'Sine'], ['triangle', 'Tri'], ['saw', 'Saw'], ['square', 'Square'], ['random', 'Random']]],
    lfo2Shape: ['LFO2 shape', [['sine', 'Sine'], ['triangle', 'Tri'], ['saw', 'Saw'], ['square', 'Square'], ['random', 'Random']]],
    lfoSync: ['LFO1 time', [[false, 'Free Hz'], [true, 'Sync']]], lfo2Sync: ['LFO2 time', [[false, 'Free Hz'], [true, 'Sync']]]
  };
  function instKnob(key) {
    const index = st().instrument,
      track = st().track,
      get = p => p.instruments[index],
      inst = () => get(P());
    if (!inst()) return null;
    const read = p => {
      const i = get(p);
      if (key === 'tuneSemi') return tuneParts(i).semis;
      if (key === 'tuneFine') return tuneParts(i).cents;
      return i?.[key];
    };
    const write = (p, v) => { if (get(p)) applyInst(get(p), key, v); };
    const label = `instrument ${key}`, burstKey = `inst:${index}:${key}`;
    const assigned = P().tracks[track]?.instrument === index;
    if (INT_PARAMS[key]) { const [name, min, max, f] = INT_PARAMS[key]; return intKnob({ key: burstKey, label: name, name: `${inst().name} · ${name}`, read: p => Number(read(p)) || 0, write, min, max, fmt: f, detent: key === 'rootNote'
        || key === 'tuneSemi'
        ? .025
        : .02 }); }
    if (ENUM_PARAMS[key]) { const [name, options] = ENUM_PARAMS[key]; return enumKnob({ key: burstKey, label: name, name: `${inst().name} · ${name}`, read: p => read(p) ?? options[0][0], write, options }); }
    const spec = INST[key]; if (!spec) return null;
    const [name, min, max, curve, f, target, lockType] = spec;
    const lock = lockType ? lockSpec(lockType) : null;
    return numKnob({ key: burstKey, label: name, name: `${inst().name} · ${name}`, read: p => { const v = Number(read(p)); return Number.isFinite(v) ? v : min; }, write, min, max, curve, fmt: f, lock, ref: target && assigned
        ? arRef(track, target, `${inst().name} · ${name}`)
        : null, label2: label });
  }

  // ---- Step locks (LOCK + encoders) --------------------------------------------------------------------
  const LOCK_RANGES = {
    volume: [0, 1, 'lin'],
    pan: [-1, 1, 'lin'],
    cutoff: [20, 20000, 'log'],
    resonance: [0, .95, 'lin'],
    delay: [0, 1, 'lin'],
    reverb: [0, 1, 'lin'],
    pitch: [-24, 24, 'lin'],
    position: [0, 1, 'lin']
  };
  function lockSpec(type, plugin = null) { return { type, plugin }; }
  function lockTurn(lock, delta, baseValue) {
    const track = st().track,
      row = st().row,
      pat = pattern(),
      patternId = pat.id;
    const sameFx = fx => fx
        && fx.type === lock.type
        && (lock.type !== 'plugin' || fx.instanceID === lock.plugin.instanceID && String(fx.address) === String(lock.plugin.address));
    const step = stepAt(track, row, pat);
    const slot = ['fx1', 'fx2'].find(key => sameFx(step?.[key])) || ['fx1', 'fx2'].find(key => !step?.[key]);
    if (!slot) { flash('Both FX columns of this step are in use'); return; }
    const range = lock.type === 'plugin' ? [0, 1, 'lin'] : LOCK_RANGES[lock.type];
    const o = { min: range[0], max: range[1], curve: range[2] };
    const burstKey = `lock:${patternId}:${track}:${row}:${lock.type}:${lock.plugin?.address ?? ''}`;
    const read = p => {
      const s = p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[row];
      const fx = ['fx1', 'fx2'].map(key => s?.[key]).find(sameFx);
      return fx ? fx.value : null;
    };
    const current = burstValue(burstKey, read) ?? clamp(Number(baseValue ?? o.min), o.min, o.max);
    let next = fromNorm(clamp01(toNorm(current, o) + delta), o);
    next = lock.type === 'pitch'
        ? Math.round(next)
        : lock.type === 'plugin' && lock.plugin.stepCount > 0
        ? Math.round(next * lock.plugin.stepCount) / lock.plugin.stepCount
        : Number(next.toFixed(4));
    const write = (p, value) => {
      const target = p.patterns.find(item => item.id === patternId); if (!target) return;
      const steps = target.steps[track];
      let s = steps[row];
      if (!s) s = steps[row] = { note: -3, instrument: p.tracks[track]?.instrument ?? 0, velocity: 100, gate: 0 };
      const key = ['fx1', 'fx2'].find(k => sameFx(s[k])) || ['fx1', 'fx2'].find(k => !s[k]); if (!key) return;
      s[key] = lock.type === 'plugin' ? {
        type: 'plugin',
        instanceID: lock.plugin.instanceID,
        address: String(lock.plugin.address),
        value,
        name: lock.plugin.name
      } : { type: lock.type, value };
    };
    burstEdit(burstKey, 'step lock', read, write, next);
    flash(`Lock ${lock.type === 'plugin' ? lock.plugin.name : FX_NAMES[lock.type]} · row ${String(row).padStart(2, '0')}`, 600);
  }
  function clearLocks() {
    const s = stepAt(); if (!s?.fx1 && !s?.fx2) { flash('No locks on this step'); return; }
    const track = st().track, row = st().row;
    S().mutate('clear step locks', () => {
      const step = pattern().steps[track][row];
      if (!step) return;
      delete step.fx1;
      delete step.fx2;
      if (step.note === -3) pattern().steps[track][row] = null;
    });
    flash('Step locks cleared');
  }

  Object.assign(mk, { instKnob, lockSpec, lockTurn, clearLocks });
})();
