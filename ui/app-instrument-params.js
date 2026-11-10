/* Instrument parameters: specs and formatting for sampler parameters, the
   parameter control markup and its keyboard handling. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, clone, clamp, esc, noteText, state, currentInstrument, syncEngineProject, stopHeld } = app;

  // ---- Instrument page (Polyend-style groups, readable units, keyboard bursts) ----
  const PLAY_MODE_LABELS = {
    oneshot: '1-Shot',
    forward: 'Forward loop',
    reverse: 'Backward loop',
    pingpong: 'Ping-pong',
    slice: 'Slice',
    wavetable: 'Wavetable',
    granular: 'Granular'
  };


  const LOOPING_PLAY_MODES = new Set(['forward', 'reverse', 'pingpong']);

  const PARAM_KINDS = {
    attack: 'time',
    decay: 'time',
    release: 'time',
    filterAttack: 'time',
    filterDecay: 'time',
    filterRelease: 'time',
    grainSize: 'time',
    sustain: 'pct',
    filterSustain: 'pct',
    resonance: 'pct',
    drive: 'pct',
    lfoDepth: 'pct',
    lfo2Depth: 'pct',
    grainPosition: 'pct',
    wavetablePosition: 'pct',
    grainDensity: 'grains',
    grainSpray: 'time',
    grainPitchSpread: 'spread',
    grainScan: 'speed',
    grainReverse: 'pct',
    grainStereo: 'pct',
    cutoff: 'hz',
    volume: 'db',
    pan: 'pan',
    tuneSemi: 'semi',
    tuneFine: 'cents',
    filterEnvAmount: 'semi',
    rootNote: 'note',
    bitDepth: 'bits',
    start: 'pos',
    end: 'pos',
    loopStart: 'pos',
    loopEnd: 'pos',
    sliceCount: 'int',
    lfoRate: 'rate',
    lfo2Rate: 'rate',
    lfoBeats: 'beats',
    lfo2Beats: 'beats',
    midiChannel: 'int',
    midiInputChannel: 'int'
  };

  // Native editor limits and perceptual travel. Stored values always remain in engine units.
  const INST_PARAM_SPECS = {
    midiChannel: { min: 1, max: 16, integer: true },
    midiInputChannel: { min: 0, max: 16, integer: true },
    attack: { min: 0, max: 60, knee: .01, floor: .01 },
    decay: { min: 0, max: 60, knee: .01, floor: .01 },
    release: { min: .0001, max: 60, knee: .01, floor: .01 },
    grainSize: { min: .002, max: .5, knee: .01, floor: .01 },
    cutoff: { min: 20, max: 20000, knee: 20, floor: 1 },
    volume: { min: 0, max: 1, floor: .1 },
    pan: { min: -1, max: 1, floor: .1 },
    sustain: { min: 0, max: 1, floor: .1 },
    resonance: { min: 0, max: .95, floor: .1 },
    drive: { min: 0, max: 1, floor: .1 },
    grainPosition: { min: 0, max: 1, floor: .01 },
    wavetablePosition: { min: 0, max: 1, floor: .01 },
    grainDensity: { min: 1, max: 8, integer: true },
    grainSpray: { min: 0, max: 1, knee: .02, floor: .01 },
    grainPitchSpread: { min: 0, max: 24, floor: .1 },
    grainScan: { min: -4, max: 4, floor: .01 },
    grainReverse: { min: 0, max: 1, floor: .01 },
    grainStereo: { min: 0, max: 1, floor: .01 },
    rootNote: { min: 0, max: 127, integer: true },
    bitDepth: { min: 2, max: 16, integer: true },
    sliceCount: { min: 1, max: 128, integer: true },
    tuneSemi: { min: -48, max: 48, integer: true },
    tuneFine: { min: -99, max: 99, integer: true },
    start: { min: 0, max: .999 },
    end: { min: .001, max: 1 },
    loopStart: { min: 0, max: .999 },
    loopEnd: { min: .001, max: 1 }
  };

  function instParamValue(input) {
    return window.STFine?.has(input) ? window.STFine.laneFromElement(input) : Number(input.value);
  }

  function instParamElementValue(input, value) {
    return window.STFine?.has(input) ? window.STFine.elementFromLane(input, value) : String(value);
  }

  function formatParam(kind, value) {
    const v = Number(value);
    if (!Number.isFinite(v)) return '';
    const sign = x => x > 0 ? '+' : x < 0 ? '−' : '';
    switch (kind) {
      case 'time':
        return v < .01
            ? `${Number((v * 1000).toFixed(1))} ms`
            : v < 1
            ? `${Math.round(v * 1000)} ms`
            : `${v.toFixed(v < 10 ? 2 : 1)} s`;
      case 'hz':
        return v < 1000 ? `${Number(v.toPrecision(4))} Hz` : `${(v / 1000).toFixed(v < 10000 ? 2 : 1)} kHz`;
      case 'rate':
        return v <= 0 ? 'Off' : `${v.toFixed(v < 10 ? 2 : 1)} Hz`;
      case 'db':
        {
          if (v <= .0001) return '−∞ dB';
          const db = 20 * Math.log10(v),
            r = Math.round(db * 10) / 10;
          return `${sign(r)}${Math.abs(r).toFixed(1)} dB`;
        }
      case 'pan':
        return Math.abs(v) < 1e-9 ? 'Center' : `${Number((Math.abs(v) * 100).toPrecision(4))}% ${v < 0 ? 'L' : 'R'}`;
      case 'pct':
        return `${Number((v * 100).toPrecision(4))}%`;
      case 'pos':
        {
          const duration = Number(state.samples?.[state.instrument]?.duration);
          return `${(v * 100).toFixed(1)}%` + (duration > 0 ? ` · ${formatParam('time', v * duration)}` : '');
        }
      case 'semi':
        {
          const r = Math.round(v * 10) / 10;
          return `${sign(r)}${Math.abs(r)} st`;
        }
      case 'cents':
        {
          const r = Math.round(v);
          return `${sign(r)}${Math.abs(r)} ct`;
        }
      case 'note':
        return `${noteText(Math.round(v))} · ${Math.round(v)}`;
      case 'bits':
        return v >= 16 ? '16-bit · clean' : `${Math.round(v)}-bit`;
      case 'beats':
        return `${Number(v.toFixed(4))} ${v === 1 ? 'beat' : 'beats'}`;
      case 'int':
        return String(Math.round(v));
      case 'grains':
        return `${Math.round(v)} ${Math.round(v) === 1 ? 'grain' : 'grains'}`;
      case 'spread':
        return v <= 0 ? 'Off' : `±${Number(v.toFixed(v < 10 ? 2 : 1))} st`;
      case 'speed':
        return Math.abs(v) < 1e-6 ? 'Frozen' : `${v < 0 ? '−' : ''}${Number(Math.abs(v).toFixed(2))}×${v < 0 ? ' back' : ''}`;
      default:
        return String(Number(v.toFixed(3)));
    }
  }

  // ↑/↓ (and ←/→ on sliders) fine; ⇧ or PageUp/PageDown coarse. Times and frequencies move in musical proportions.
  function paramStep(kind, value, direction, coarse, input) {
    const v = Number(value) || 0,
      d = direction;
    switch (kind) {
      case 'time':
        {
          const fine = v < .1 || v === .1 && d < 0 ? .001 : v < 1 || v === 1 && d < 0 ? .01 : .1;
          return v + d * fine * (coarse ? 10 : 1);
        }
      case 'hz':
        return Math.max(1, v) * Math.pow(2, d * (coarse ? 1 : 1 / 12));
      case 'rate':
        {
          const fine = v < 1 || v === 1 && d < 0 ? .01 : v < 10 || v === 10 && d < 0 ? .1 : 1;
          return v + d * fine * (coarse ? 10 : 1);
        }
      case 'db':
      case 'pan':
      case 'pct':
        return v + d * (coarse ? .1 : .01);
      case 'pos':
        return v + d * (coarse ? .01 : .001);
      case 'semi':
      case 'note':
        return v + d * (coarse ? 12 : 1);
      case 'cents':
        return v + d * (coarse ? 10 : 1);
      case 'bits':
        return v + d * (coarse ? 4 : 1);
      case 'int':
        return v + d * (coarse ? 8 : 1);
      case 'grains':
        return v + d * (coarse ? 4 : 1);
      case 'spread':
        return v + d * (coarse ? 1 : .1);
      case 'speed':
        return v + d * (coarse ? .25 : .01);
      case 'beats':
        return v + d * (coarse ? 1 : 1 / 64);
      default:
        {
          const step = Number(input?.step);
          return v + d * (Number.isFinite(step) && step > 0 ? step : .01) * (coarse ? 10 : 1);
        }
    }
  }

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

  // One place for every keyboard/pointer edit of a native instrument parameter.
  function applyInstParam(inst, key, value) {
    const spec = INST_PARAM_SPECS[key];
    if (!Number.isFinite(value)) return;
    if (spec) value = clamp(spec.integer ? Math.round(value) : value, spec.min, spec.max);
    if (key === 'tuneSemi') {
      inst.tune = clamp(clamp(Math.round(value), -48, 48) + tuneParts(inst).cents / 100, -48, 48);
      return;
    }
    if (key === 'tuneFine') {
      inst.tune = clamp(tuneParts(inst).semis + clamp(Math.round(value), -99, 99) / 100, -48, 48);
      return;
    }
    if (key.startsWith('midi') && inst[key] !== value) stopHeld();
    inst[key] = value;
    if (key === 'start') inst.start = Math.min(inst.start, inst.end - .001);
    if (key === 'end') inst.end = Math.max(inst.end, inst.start + .001);
    if (key === 'loopStart') inst.loopStart = Math.min(inst.loopStart, inst.loopEnd - .001);
    if (key === 'loopEnd') inst.loopEnd = Math.max(inst.loopEnd, inst.loopStart + .001);
  }

  function parameter(key, label, min, max, step, value, target = key, opts = {}) {
    const spec = INST_PARAM_SPECS[key];
    if (spec) {
      min = spec.min;
      max = spec.max;
    }
    const fine = spec?.floor && window.STFine,
      kind = opts.kind ?? PARAM_KINDS[key] ?? '',
      readout = kind ? formatParam(kind, value) : '',
      name = esc(opts.name || label);
    const automateValue = opts.automateValue !== undefined ? ` data-automate-value="${opts.automateValue}"` : '';
    const common = `data-inst-param="${key}" data-automate="${target}" data-track="${state.track}" data-name="${name}"${automateValue}`;
    const fineAttributes = fine ? ` data-fine-min="${min}" data-fine-max="${max}" data-fine-floor="${spec.floor}"` : '';
    const curved = fine && spec.knee,
      sliderValue = curved ? window.STFine.toPosition(value, spec) : value;
    const hint = 'Type a value · ↑↓ fine · ⌥ finer · ⇧ coarser · ⌥/⇧-drag fine · ⌥A automate';
    return `<label class="parameter"${kind ? ` data-param-kind="${kind}"` : ''}><span class="parameter-top">`
        + `<span class="parameter-label">${esc(label)}</span>${kind
            ? `<output class="parameter-readout" data-param-readout="${key}" aria-hidden="true">${esc(readout)}</output>`
            : ''}</span>`
        + `<span class="parameter-controls"><input id="param-${key}" ${common}${fineAttributes}${curved ? ` data-fine-knee="${spec.knee}"` : ''}`
        + ` min="${curved ? 0 : min}" max="${curved ? 1 : max}" step="${fine ? 'any' : step}" value="${sliderValue}"`
        + ` aria-label="${esc(label)}" type="range" title="${hint}"${readout ? ` aria-valuetext="${esc(readout)}"` : ''}>`
        + `<input class="parameter-value" id="param-value-${key}" ${common}${fineAttributes} min="${min}"`
        + ` max="${max}" step="${fine ? 'any' : step}" value="${value}" type="number" aria-label="${esc(label)} value"`
        + ` title="${hint}"></span></label>`;
  }

  function updateParamReadout(key, value) {
    const kind = PARAM_KINDS[key];
    if (!kind) return;
    const text = formatParam(kind, value);
    for (const out of $$(`[data-param-readout="${key}"]`)) out.textContent = text;
    for (const el of $$(`input[type=range][data-inst-param="${key}"]`)) el.setAttribute('aria-valuetext', text);
    if (key === 'tuneSemi' || key === 'tuneFine') {
      const semis = Number($('#param-value-tuneSemi')?.value) || 0,
        cents = Number($('#param-value-tuneFine')?.value) || 0,
        tune = clamp(semis + cents / 100, -48, 48);
      for (const el of $$('[data-inst-param="tuneSemi"],[data-inst-param="tuneFine"]')) el.dataset.automateValue = String(Number(tune.toFixed(4)));
    }
  }

  // Arrow-key editing burst: each press previews (and is heard), one undo step commits when the burst ends.
  app.paramBurst = null;
  let paramPreviewTimer = null;
  const PARAM_ARROWS = ['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown'];

  function fireParamEvent(el, type) {
    if (typeof el.dispatchEvent === 'function') el.dispatchEvent(new Event(type, { bubbles: true }));
  }

  function endParamBurst() {
    const burst = app.paramBurst;
    if (!burst) return;
    app.paramBurst = null;
    clearTimeout(burst.timer);
    clearTimeout(paramPreviewTimer);
    if (burst.input.isConnected !== false) fireParamEvent(burst.input, 'change');
  }

  /* Live parameters: a drag or key burst is heard on the instrument's sounding voices at once through
     engine.instrumentParameter (no project republish); the committed edit publishes the project as usual.
     Requests are coalesced per target: one in flight, the latest value sent when it returns. */
  const LIVE_TARGETS = new Set(['attack', 'decay', 'sustain', 'release', 'drive', 'bitDepth', 'lfoRate', 'lfoDepth', 'start', 'end',
    'loopStart', 'loopEnd', 'grainSize', 'grainPosition', 'sliceCount', 'filterAttack', 'filterDecay', 'filterSustain', 'filterRelease',
    'filterEnvAmount', 'lfo2Rate', 'lfo2Depth', 'lfoBeats', 'lfo2Beats', 'wavetablePosition', 'grainDensity', 'grainSpray',
    'grainPitchSpread', 'grainScan', 'grainReverse', 'grainStereo', 'cutoff', 'resonance']);
  const LIVE_ALIASES = { volume: 'instrumentVolume', pan: 'instrumentPan', tuneSemi: 'tune', tuneFine: 'tune' };
  const liveSends = new Map();
  /** Engine target and value for a sampler parameter edit, or null when it only applies on commit. */
  function liveTarget(inst, key, value) {
    if (!inst || inst.type !== 'sample' || !Number.isFinite(Number(value))) return null;
    const target = LIVE_ALIASES[key] || (LIVE_TARGETS.has(key) ? key : null);
    if (!target) return null;
    const draft = { ...inst };
    applyInstParam(draft, key, Number(value));
    const engineValue = target === 'tune' ? draft.tune : target === 'instrumentVolume' ? draft.volume : target === 'instrumentPan' ? draft.pan : draft[key];
    return Number.isFinite(Number(engineValue)) ? { target, value: Number(engineValue) } : null;
  }
  function liveInstParam(index, inst, key, value) {
    const live = liveTarget(inst, key, value);
    if (!live || !state.native || !(index >= 0)) return false;
    const id = `${index}:${live.target}`, entry = liveSends.get(id) || { busy: false, next: null };
    liveSends.set(id, entry);
    entry.next = { instrument: index, target: live.target, value: live.value };
    const send = () => {
      if (entry.busy || !entry.next) return;
      const payload = entry.next; entry.next = null; entry.busy = true;
      Promise.resolve(app.request('engine.instrumentParameter', payload)).catch(() => {}).finally(() => { entry.busy = false; send(); });
    };
    send();
    return true;
  }

  function previewInstParam(key, value) {
    // Heard live through the input event (liveInstParam); only other parameters republish a draft.
    if (state.native && liveTarget(currentInstrument(), key, value)) { clearTimeout(paramPreviewTimer); return; }
    clearTimeout(paramPreviewTimer);
    const project = state.project,
      index = state.instrument,
      inst = currentInstrument();
    paramPreviewTimer = setTimeout(() => {
      Promise.resolve(syncEngineProject(() => {
        if (state.project !== project || currentInstrument() !== inst || !project.instruments[index]) return null;
        const draft = clone(project);
        applyInstParam(draft.instruments[index], key, value);
        return draft;
      })).catch(() => {});
    }, 60);
  }

  function parameterKeydown(e) {
    const input = e.currentTarget || e.target;
    if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.isComposing || !PARAM_ARROWS.includes(e.key)) return;
    const fine = window.STFine?.has(input) ? window.STFine : null;
    if (e.altKey && !fine) return;
    if (input.type === 'number'
        && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) return; // Caret movement while typing.
    const key = input.dataset.instParam ?? input.dataset.modParam,
      kind = PARAM_KINDS[key];
    const current = String(input.value).trim() !== '' && Number.isFinite(Number(input.value))
        ? instParamValue(input)
        : Number(input.defaultValue ?? input.getAttribute?.('value')) || 0;
    const direction = ['ArrowUp', 'ArrowRight', 'PageUp'].includes(e.key) ? 1 : -1,
      coarse = e.shiftKey || e.key === 'PageUp' || e.key === 'PageDown';
    const min = input.min !== '' && input.min != null ? Number(input.min) : -Infinity,
      max = input.max !== '' && input.max != null ? Number(input.max) : Infinity;
    let next = fine ? fine.stepValue(current, direction, {
      fine: e.altKey && !coarse,
      coarse
    }, fine.specOf(input)) : clamp(paramStep(kind, current, direction, coarse, input), min, max);
    const step = Number(input.step);
    if (Number.isFinite(step) && step >= 1) next = clamp(Math.round(next), min, max);
    next = Number(next.toFixed(6));
    e.preventDefault();
    e.stopPropagation?.();
    if (app.paramBurst && app.paramBurst.input !== input) endParamBurst();
    if (!app.paramBurst) app.paramBurst = { input, timer: null };
    clearTimeout(app.paramBurst.timer);
    app.paramBurst.timer = setTimeout(endParamBurst, 700);
    if (next === current) return;
    input.value = instParamElementValue(input, next);
    fireParamEvent(input, 'input');
    if (input.dataset.instParam !== undefined) previewInstParam(key, next);
  }

  function bindParameterKeys() {
    for (const el of $$('input[data-inst-param],input[data-mod-param]')) {
      if (el.dataset.modParam?.endsWith('Beats')) continue;
      el.addEventListener('keydown', parameterKeydown);
      if (el.type === 'range' && el.dataset.instParam !== undefined) window.STFine?.bindDrag(el);
      el.addEventListener('blur', () => {
        if (app.paramBurst?.input === el) endParamBurst();
      });
    }
  }

  Object.assign(app, {
    PLAY_MODE_LABELS, LOOPING_PLAY_MODES, instParamValue, instParamElementValue, tuneParts, applyInstParam, liveTarget, liveInstParam,
    parameter, updateParamReadout, PARAM_ARROWS, fireParamEvent, endParamBurst, bindParameterKeys
  });
})();
