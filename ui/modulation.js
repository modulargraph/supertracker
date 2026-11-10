/* Native instrument modulation and automation controls. */
(() => {
  'use strict';
  const defaults = Object.freeze({
    filterAttack: .005,
    filterDecay: .2,
    filterSustain: 0,
    filterRelease: .2,
    filterEnvAmount: 0,
    lfoRate: 0,
    lfoSync: false,
    lfoBeats: 1,
    lfoDepth: 0,
    lfoTarget: 'cutoff',
    lfoShape: 'sine',
    lfo2Rate: 0,
    lfo2Sync: false,
    lfo2Beats: 1,
    lfo2Depth: 0,
    lfo2Target: 'cutoff',
    lfo2Shape: 'sine'
  });
  const routes = [['cutoff', 'Filter cutoff'], ['pitch', 'Pitch'], ['volume', 'Level'], ['pan', 'Pan'], ['position', 'Position'], ['grainSize', 'Grain size']];
  const shapes = [['sine', 'Sine'], ['triangle', 'Triangle'], ['saw', 'Saw'], ['square', 'Square'], ['random', 'Random / hold']];
  const numeric = Object.freeze({
    filterAttack: { name: 'Filter attack', group: 'Filter envelope', min: 0, max: 60, step: .001, unit: 's', knee: .01, floor: .01 },
    filterDecay: { name: 'Filter decay', group: 'Filter envelope', min: 0, max: 60, step: .001, unit: 's', knee: .01, floor: .01 },
    filterSustain: { name: 'Filter sustain', group: 'Filter envelope', min: 0, max: 1, step: .001, unit: '%', scale: 100, floor: .01 },
    filterRelease: { name: 'Filter release', group: 'Filter envelope', min: .0001, max: 60, step: .001, unit: 's', knee: .01, floor: .01 },
    filterEnvAmount: { name: 'Filter envelope amount', group: 'Filter envelope', min: -48, max: 48, step: .1, unit: 'st', floor: 1 },
    lfoRate: { name: 'LFO 1 rate', group: 'LFO 1', min: 0, max: 100, step: .01, unit: 'Hz', knee: .05, floor: .1 },
    lfoBeats: { name: 'LFO 1 cycle', group: 'LFO 1', min: 1 / 64, max: 64, step: 1 / 64, unit: 'beats' },
    lfoDepth: { name: 'LFO 1 depth', group: 'LFO 1', min: 0, max: 1, step: .001, unit: '%', scale: 100, knee: .02, floor: .01 },
    lfo2Rate: { name: 'LFO 2 rate', group: 'LFO 2', min: 0, max: 100, step: .01, unit: 'Hz', knee: .05, floor: .1 },
    lfo2Beats: { name: 'LFO 2 cycle', group: 'LFO 2', min: 1 / 64, max: 64, step: 1 / 64, unit: 'beats' },
    lfo2Depth: { name: 'LFO 2 depth', group: 'LFO 2', min: 0, max: 1, step: .001, unit: '%', scale: 100, knee: .02, floor: .01 }
  });
  // Rates, times and depths are edited on perceptual sliders (knee: see fine-control.js) and step
  // relative to their magnitude; stored values stay in engine units (Hz, seconds, 0–1, semitones).
  const nativeTypes = new Set(['sample']);
  const S = () => window.ST;
  const F = () => window.STFine;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const finite = (value, fallback = 0) => Number.isFinite(Number(value)) ? Number(value) : fallback;
  const bound = (value, min, max) => Math.max(min, Math.min(max, finite(value)));
  const valueOf = (inst, key) => finite(inst[key], defaults[key]);
  const withDefaults = inst => ({ ...defaults, ...inst });
  // Readable numbers: up to four significant digits (0.05 Hz, 1.25%, 0.5 ms, 12.5 Hz).
  const sig = (value, digits = 4) => { const n = Number(Number(value).toPrecision(digits)); return String(Math.abs(n) < 1e-9 ? 0 : n); };
  const seconds = value => value < 1 ? `${sig(value * 1000, 3)} ms` : `${Number(value.toFixed(2))} s`;
  const signed = value => `${value > 0 ? '+' : ''}${sig(value, 3)}`;
  const hertz = value => `${sig(value)} Hz`;
  const percent = value => `${sig(value * 100)}%`;
  const supports = (target, type) => Object.hasOwn(numeric, target) && (!type || nativeTypes.has(typeof type === 'string' ? type : type.type));
  // Periods use quarter-note beats, independent of the project's time signature.
  const noteLengths = [[1, 'Whole note'], [2, 'Half note'], [4, 'Quarter note'], [8, 'Eighth note'], [16, 'Sixteenth note'], [32, '32nd note'],
      [64, '64th note'], [128, '128th note'], [256, '256th note']];
  const divisions = Object.freeze([
    ...[16,8,4,2].map(count => ({ id: `whole-${count}`, group: 'Straight', name: `${count} whole notes`, beats: count * 4 })),
    ...[['straight','Straight',1],['dotted','Dotted',1.5],
        ['triplet','Triplet',2 / 3]].flatMap(([kind,group,factor]) => noteLengths.map(([denominator,name]) => ({ id: `${kind}-${denominator}`, group, name: `${name}${kind === 'straight' ? '' : ` · ${kind}`}`, beats: 4 / denominator * factor })).filter(item => item.beats >= 1 / 64
            && item.beats <= 64))
  ]);
  const synced = (inst, prefix) => inst[`${prefix}Sync`] === true;
  const divisionFor = beats => divisions.find(item => Math.abs(item.beats - beats) < 1e-10);
  const displayNumber = (value, spec) => spec.unit === 'beats' ? String(value) : sig(value, 8);
  const scaleOf = spec => spec.scale || 1;
  const beatsLabel = value => `${Number(value.toFixed(6))} ${value === 1 ? 'beat' : 'beats'}`;
  let previewTimer = null;
  let previewOwner = null;
  let previewSerial = 0;
  function cancelPreview() { clearTimeout(previewTimer); previewSerial++; }

  function descriptor(inst, target) {
    const spec = numeric[target];
    const prefix = target.startsWith('lfo2') ? 'lfo2' : 'lfo', isRate = target.endsWith('Rate'), isBeats = target.endsWith('Beats');
    const inactive = (isRate && synced(inst,prefix)) || (isBeats && !synced(inst,prefix));
    return { ...spec, target, value: bound(valueOf(inst, target), spec.min, spec.max), available: true, inactive, fineCurve: spec.knee
        ? { knee: spec.knee, floor: spec.floor }
        : undefined,
      inactiveReason: inactive
          ? isRate
              ? 'Inactive while tempo sync is on; the stored Hz value and its automation are preserved.'
              : 'Inactive in Hz mode; switch this LFO to tempo sync to use beat automation.'
          : '',
      format: value => spec.unit === 's'
          ? seconds(value)
          : spec.unit === '%'
          ? percent(value)
          : spec.unit === 'st'
          ? `${signed(value)} st`
          : spec.unit === 'beats'
          ? beatsLabel(value)
          : hertz(value) };
  }

  function targets(track, includeUnavailable = false) {
    const p = S().state.project, selected = p?.tracks[track];
    if (!selected) return [];
    const inst = p.instruments[selected.instrument || 0];
    return inst && (includeUnavailable || nativeTypes.has(inst.type))
        ? Object.keys(numeric).map(target => ({...descriptor(inst, target),available:nativeTypes.has(inst.type)}))
        : [];
  }

  // Element contract for fine control (fine-control.js) and the automation recorder: lane units plus
  // the number field's display scale, and the knee on curved sliders whose value is a 0..1 position.
  const fineAttributes = (spec, slider) => `data-fine-min="${spec.min}" data-fine-max="${spec.max}" data-fine-floor="${spec.floor}"${slider ? spec.knee ? ` data-fine-knee="${spec.knee}"` : '' : ` data-fine-scale="${scaleOf(spec)}"`}`;
  const sliderValue = (spec, value) => spec.knee ? String(F().toPosition(value, spec)) : String(value);
  const KEY_HINT = 'Type a value · ↑↓ fine steps · ⌥ finer · ⇧ coarser · ⌥/⇧-drag the slider for fine control · ⌥A automates';
  function parameter(inst, index, track, key, label) {
    const spec = numeric[key], scale = scaleOf(spec), value = bound(valueOf(inst, key), spec.min, spec.max), unit = spec.unit;
    const beats = spec.unit === 'beats', text = descriptor(inst, key).format(value);
    const attributes = `data-mod-param="${key}" data-mod-instrument="${index}" data-automate="${key}" data-track="${track}" data-name="${esc(spec.name)}"`;
    if (beats) return `<div class="mod-parameter mod-beat-parameter"><label for="mod-number-${index}-${key}">${esc(label || spec.name)}</label>`
        + `<div class="mod-value"><input type="number" id="mod-number-${index}-${key}" ${attributes} min="${spec.min}" max="${spec.max}"`
        + ` step="any" value="${displayNumber(value,spec)}" aria-label="${esc(spec.name)} quarter-note beats per cycle" title="Quarter-note beats `
        + `per cycle. Up/Down adjusts by 1/64 beat; Shift adjusts by 1 beat. Option-A automates."><span>${esc(unit)}</span></div></div>`;
    return `<div class="mod-parameter"><label for="mod-${index}-${key}">${esc(label || spec.name)}</label><div class="mod-value">`
        + `<input type="number" id="mod-number-${index}-${key}" ${attributes} ${fineAttributes(spec, false)} min="${spec.min * scale}"`
        + ` max="${spec.max * scale}" step="any" value="${displayNumber(value * scale,spec)}" aria-label="${esc(spec.name)} ${spec.unit === '%' ? 'percent' : esc(spec.unit)}"`
        + ` title="${esc(text)} · ${KEY_HINT}"><span>${esc(unit)}</span></div><input type="range" id="mod-${index}-${key}" ${attributes} ${fineAttributes(spec, true)}`
        + ` min="${spec.knee ? 0 : spec.min}" max="${spec.knee ? 1 : spec.max}" step="any" value="${sliderValue(spec, value)}" aria-label="${esc(spec.name)}"`
        + ` aria-valuetext="${esc(text)}" title="${esc(text)} · ${KEY_HINT}"></div>`;
  }

  function timing(inst, index, track, prefix) {
    const sync = synced(inst, prefix),
      beatSpec = numeric[`${prefix}Beats`],
      beats = bound(valueOf(inst, `${prefix}Beats`), beatSpec.min, beatSpec.max),
      selected = divisionFor(beats);
    return `<div class="mod-lfo-timing"><label class="mod-select" for="mod-${index}-${prefix}Timing">Timing<select id="mod-${index}-${prefix}Timing"`
        + ` data-mod-mode="${prefix}" data-mod-instrument="${index}" aria-label="${prefix === 'lfo' ? 'LFO 1' : 'LFO 2'} timing">`
        + `<option value="hz" ${sync ? '' : 'selected'}>Free · Hz</option><option value="sync" ${sync ? 'selected' : ''}>Tempo sync</option>`
        + `</select></label><div class="mod-timing-value">${sync ? `<label class="mod-select" for="mod-${index}-${prefix}Division">Division<select id="mod-${index}-${prefix}Division" data-mod-division="${prefix}"`
        + ` data-mod-instrument="${index}"><option value="custom" ${selected ? '' : 'selected'} disabled>Custom cycle</option>${['Straight','Dotted','Triplet'].map(group => `<optgroup label="${group}">${divisions.filter(item => item.group === group).map(item => `<option value="${item.id}" ${selected?.id === item.id ? 'selected' : ''}>${item.name}</option>`).join('')}</optgroup>`).join('')}</select>`
        + `</label>${parameter(inst,index,track,`${prefix}Beats`,'Cycle')}` : parameter(inst,index,track,`${prefix}Rate`,'Rate')}</div>`
        + `<output class="mod-timing-note" data-mod-timing-note="${prefix}">${timingDescription(inst,prefix)}</output></div>`;
  }

  function timingDescription(inst, prefix) {
    return synced(inst,prefix)
        ? `Stored Hz rate: ${hertz(valueOf(inst,`${prefix}Rate`))} · inactive`
        : `Beat cycle stored separately · ${beatsLabel(valueOf(inst,`${prefix}Beats`))}`;
  }

  function envelope(inst) {
    const p = withDefaults(inst), left = 0, right = 1000, mid = 92, amplitude = bound(p.filterEnvAmount, -48, 48) / 48 * 64;
    const a = Math.sqrt(Math.max(.01, finite(p.filterAttack))),
      d = Math.sqrt(Math.max(.01, finite(p.filterDecay))),
      r = Math.sqrt(Math.max(.01, finite(p.filterRelease)));
    const stretch = (right - left) * .7 / (a + d + r), xA = left + a * stretch, xD = xA + d * stretch, xS = xD + (right - left) * .3;
    const yA = mid - amplitude, yS = mid - amplitude * bound(p.filterSustain, 0, 1);
    const path = `M ${left} ${mid} L ${xA} ${yA} L ${xD} ${yS} L ${xS} ${yS} L ${right} ${mid}`;
    return `<div class="mod-envelope-plot"><div class="mod-env-yaxis" aria-hidden="true"><span style="top:28px">+48 st</span>`
        + `<span style="top:92px">0 st</span><span style="top:156px">−48 st</span></div><div class="mod-env-drawing">`
        + `<svg viewBox="0 0 1000 190" preserveAspectRatio="none" role="img" aria-label="Filter envelope: attack ${seconds(p.filterAttack)}, decay ${seconds(p.filterDecay)}, `
        + `sustain ${Math.round(p.filterSustain * 100)} percent, release ${seconds(p.filterRelease)}, amount ${signed(p.filterEnvAmount)} semitones. `
        + `Time segments are expanded for visibility."><path class="mod-axis" d="M${left} 28H${right}M${left} ${mid}H${right}M${left} 156H${right}"/>`
        + `<path class="mod-fill" d="${path} Z"/><path class="mod-curve" d="${path}"/>${[[xA,yA], [xD,yS], [xS,yS]].map(([x,y]) => `<rect class="mod-breakpoint" x="${x - 4}" y="${y - 2.5}" width="8" height="5"/>`).join('')}<path `
        + `class="mod-gate" d="M${xS} 18V162"/></svg><span class="mod-env-gate-label" style="left:${xS / 10}%" aria-hidden="true">Note off</span>`
        + `<div class="mod-env-stages" aria-hidden="true"><span style="left:${(left + xA) / 20}%">A</span><span style="left:${(xA + xD) / 20}%">D</span>`
        + `<span style="left:${(xD + xS) / 20}%">S</span><span style="left:${(xS + right) / 20}%">R</span></div></div></div>`;
  }

  function waveform(shape, depth) {
    const values = [], random = [-.35,.62,.1,-.78,.38,-.2,.91,-.49];
    for (let i = 0; i <= 72; i++) {
      const unit = i / 72 * 2, phase = unit % 1;
      const value = shape === 'triangle'
          ? 1 - 4 * Math.abs(phase - .5)
          : shape === 'saw'
          ? phase * 2 - 1
          : shape === 'square'
          ? phase < .5 ? 1 : -1
          : shape === 'random'
          ? random[Math.min(7, Math.floor(unit * 4))]
          : Math.sin(unit * Math.PI * 2);
      values.push(`${i ? 'L' : 'M'} ${4 + i / 72 * 100} ${22 - value * bound(depth, 0, 1) * 16}`);
    }
    return `<svg viewBox="0 0 108 44" role="img" aria-label="${esc(shape)} waveform at ${sig(bound(depth, 0, 1) * 100, 3)} percent depth">`
        + `<path class="mod-axis" d="M4 22H104"/><path class="mod-wave-line" d="${values.join(' ')}"/></svg>`;
  }

  function effectDescription(target, depth) {
    const d = bound(depth, 0, 1);
    if (target === 'cutoff') return `±${sig(d * 4, 3)} octaves`;
    if (target === 'pitch') return `±${sig(d * 12, 3)} semitones`;
    if (target === 'pan') return `±${sig(d * 100, 3)}% pan`;
    if (target === 'position') return `±${sig(d * 50, 3)}% position`;
    if (target === 'grainSize') return `grain size ×${sig(Math.pow(2, -2 * d), 3)}–×${sig(Math.pow(2, 2 * d), 3)}`;
    return `${sig((1 - d) * 100, 3)}–100% level`;
  }

  function lfo(inst, index, track, second) {
    const p = withDefaults(inst), prefix = second ? 'lfo2' : 'lfo', n = second ? 2 : 1;
    const target = routes.some(([key]) => key === p[`${prefix}Target`]) ? p[`${prefix}Target`] : 'cutoff';
    const shape = shapes.some(([key]) => key === p[`${prefix}Shape`]) ? p[`${prefix}Shape`] : 'sine';
    return `<div class="mod-lfo-row" data-mod-lfo="${prefix}"><span class="mod-lfo-name">LFO ${n}</span><label class="mod-select" for="mod-${index}-${prefix}Shape">Wave<select `
        + `id="mod-${index}-${prefix}Shape" data-mod-select="${prefix}Shape" data-mod-instrument="${index}">${shapes.map(([key, name]) => `<option value="${key}" ${key === shape ? 'selected' : ''}>${esc(name)}</option>`).join('')}</select>`
        + `</label><label class="mod-select" for="mod-${index}-${prefix}Target">Destination<select id="mod-${index}-${prefix}Target"`
        + ` data-mod-select="${prefix}Target" data-mod-instrument="${index}">${routes.filter(([key]) => key !== 'position' || inst.type === 'sample' || target === 'position').map(([key, name]) => `<option value="${key}" ${key === target ? 'selected' : ''} ${key === 'position' && inst.type !== 'sample' ? 'disabled' : ''}>${esc(name)}${key === 'position' && inst.type !== 'sample' ? ' (sampler only)' : ''}</option>`).join('')}</select>`
        + `</label>${timing(inst, index, track, prefix)}${parameter(inst, index, track, `${prefix}Depth`, 'Depth')}<div class="mod-lfo-preview">`
        + `<div data-mod-wave="${prefix}">${waveform(shape, valueOf(inst, `${prefix}Depth`))}</div><output data-mod-extent="${prefix}">${effectDescription(target, valueOf(inst, `${prefix}Depth`))}</output></div></div>`;
  }

  function render(inst) {
    if (!inst || !nativeTypes.has(inst.type)) return '';
    const state = S().state, found = state.project.instruments.indexOf(inst), index = found >= 0 ? found : state.instrument, track = state.track;
    return `<section class="modulation-console" data-mod-root="${index}" aria-label="Instrument modulation"><header class="mod-console-head">`
        + `<h3>Modulation</h3><span>Per voice · Hz or tempo sync · ↑↓ fine steps, ⌥ finer, ⇧ coarser · ⌥/⇧-drag fine · ⌥A automates</span>`
        + `</header><section class="mod-filter-section" aria-label="Filter envelope"><div class="mod-envelope-view">`
        + `<div class="mod-section-label">Filter envelope<span data-mod-envelope-amount>${signed(valueOf(inst, 'filterEnvAmount'))} st</span>`
        + `</div><div class="mod-envelope" data-mod-envelope>${envelope(inst)}</div><p class="mod-caption">Offset from filter cutoff · Time segments `
        + `expanded for visibility</p></div><div class="mod-envelope-controls">${parameter(inst,index,track,'filterAttack','Attack')}${parameter(inst,index,track,'filterDecay','Decay')}${parameter(inst,index,track,'filterSustain','Sustain')}${parameter(inst,index,track,'filterRelease','Release')}${parameter(inst,index,track,'filterEnvAmount','Amount')}<div `
        + `class="mod-envelope-note">Amount 0 leaves the filter unchanged. Positive values open it; negative values close it.</div>`
        + `</div></section><section class="mod-lfos" aria-label="Low frequency oscillators">${lfo(inst,index,track,false)}${lfo(inst,index,track,true)}</section>`
        + `<p class="mod-caption mod-footer">Beat values use quarter notes, regardless of meter. Hz and beat automation remain separate. Retriggered `
        + `notes reset phase; legato preserves it. Depth 0 disables modulation.</p></section>`;
  }

  function preview(root, inst) {
    const plot = root.querySelector('[data-mod-envelope]'); if (plot) plot.innerHTML = envelope(inst);
    const amount = root.querySelector('[data-mod-envelope-amount]'); if (amount) amount.textContent = `${signed(valueOf(inst,'filterEnvAmount'))} st`;
    for (const prefix of ['lfo','lfo2']) {
      const wave = root.querySelector(`[data-mod-wave="${prefix}"]`), extent = root.querySelector(`[data-mod-extent="${prefix}"]`);
      if (wave) wave.innerHTML = waveform(inst[`${prefix}Shape`] || 'sine', valueOf(inst,`${prefix}Depth`));
      if (extent) extent.textContent = effectDescription(inst[`${prefix}Target`] || 'cutoff', valueOf(inst,`${prefix}Depth`));
      const note = root.querySelector(`[data-mod-timing-note="${prefix}"]`), division = root.querySelector(`[data-mod-division="${prefix}"]`);
      if (note) note.textContent = timingDescription(inst,prefix);
      if (division) division.value = divisionFor(valueOf(inst,`${prefix}Beats`))?.id || 'custom';
    }
  }

  function queueAudioPreview(root, index, key, value) {
    cancelPreview(); previewOwner = root;
    // Rates, depths and filter-envelope values are heard live on sounding voices (no project republish).
    if (S().liveInstParam?.(index, S().state.project.instruments[index], key, value)) return;
    const serial = previewSerial, sourceProject = S().state.project, sourceInstrument = sourceProject.instruments[index];
    previewTimer = setTimeout(async () => {
      try {
        await S().syncEngineProject(() => {
          if (serial !== previewSerial
              || !root.isConnected
              || S().state.project !== sourceProject
              || sourceProject.instruments[index] !== sourceInstrument) return null;
          const draft = window.STUtil.clone(S().state.project);
          if (!draft.instruments[index]) return null;
          draft.instruments[index][key] = value;
          return draft;
        });
      } catch (error) {
        if (serial === previewSerial && root.isConnected) S().toast(error.message || String(error),'error');
      }
    },80);
  }

  // Element value ⇄ stored value. Number fields show display units (percent for 0–1 values); curved
  // sliders hold a 0..1 position. Dragged positions round to three significant digits.
  function valueFromInput(input, spec) {
    if (input.value === '' || !Number.isFinite(input.valueAsNumber)) return NaN;
    if (input.type === 'range') return spec.knee
        ? F().quantize(F().fromPosition(input.valueAsNumber, spec), spec)
        : F().quantize(bound(input.valueAsNumber, spec.min, spec.max), spec);
    return bound(input.valueAsNumber / scaleOf(spec), spec.min, spec.max);
  }
  function inputValue(input, spec, value) {
    if (input.type === 'range') return sliderValue(spec, value);
    return displayNumber(value * (spec.unit === 'beats' ? 1 : scaleOf(spec)), spec);
  }
  const STEP_KEYS = ['ArrowUp','ArrowDown','PageUp','PageDown'], BURST_MS = 700;
  let burst = null;
  function endBurst() {
    const current = burst; if (!current) return;
    burst = null; clearTimeout(current.timer);
    if (current.input.isConnected !== false) current.input.dispatchEvent(new Event('change',{bubbles:true}));
  }

  function bind(scope = document) {
    for (const root of scope.querySelectorAll('[data-mod-root]')) {
      if (root.dataset.modBound === 'true') continue;
      root.dataset.modBound = 'true';
      const sourceProject = S().state.project, sourceIndex = Number(root.dataset.modRoot), sourceInstrument = sourceProject.instruments[sourceIndex];
      const owns = () => root.isConnected !== false
          && S().state.project === sourceProject
          && sourceProject.instruments[sourceIndex] === sourceInstrument;
      const restore = () => {
        if (!owns()) return;
        for (const field of root.querySelectorAll('[data-mod-param]')) {
          const fieldSpec = numeric[field.dataset.modParam];
          if (fieldSpec) {
            const value = valueOf(sourceInstrument, field.dataset.modParam);
            field.value = inputValue(field, fieldSpec, value);
            field.setAttribute('aria-valuetext', descriptor(sourceInstrument, field.dataset.modParam).format(value));
          }
        }
        for (const field of root.querySelectorAll('[data-mod-mode]')) field.value = synced(sourceInstrument,field.dataset.modMode) ? 'sync' : 'hz';
        for (const field of root.querySelectorAll('[data-mod-select]')) {
          const key = field.dataset.modSelect;
          field.value = sourceInstrument[key] || defaults[key];
        }
        preview(root, sourceInstrument);
      };
      const commit = (label, edit) => { const accepted = S().mutate(label, edit); if (accepted === false) restore(); return accepted; };
      for (const input of root.querySelectorAll('[data-mod-param]')) {
        const key = input.dataset.modParam, index = Number(input.dataset.modInstrument), spec = numeric[key];
        if (!spec) continue;
        input.addEventListener('input', () => {
          if (!owns()) return;
          const value = valueFromInput(input, spec), inst = S().state.project.instruments[index]; if (!Number.isFinite(value) || !inst) return;
          for (const twin of root.querySelectorAll(`[data-mod-param="${key}"]`)) if (twin !== input) twin.value = inputValue(twin, spec, value);
          const text = descriptor(inst,key).format(value);
          for (const twin of root.querySelectorAll(`[data-mod-param="${key}"]`)) if (twin.type === 'range') twin.setAttribute('aria-valuetext',text);
          if (input.type !== 'range') input.setAttribute('aria-valuetext',text);
          preview(root,{...inst,[key]:value}); queueAudioPreview(root,index,key,value);
        });
        input.addEventListener('change', () => {
          if (!owns()) return;
          cancelPreview(); if (burst?.input === input) { clearTimeout(burst.timer); burst = null; }
          const current = S().state.project.instruments[index]; if (!current || !nativeTypes.has(current.type)) return;
          const typed = valueFromInput(input, spec), value = Number.isFinite(typed) ? typed : valueOf(current,key);
          commit(`Adjust ${spec.name.toLowerCase()}`, p => { if (p.instruments[index]) p.instruments[index][key] = value; });
        });
        if (spec.unit !== 'beats') {
          // Magnitude-relative steps (fine-control.js); a burst of presses previews each step and
          // commits one Undo step when it ends (key idle, blur or another key).
          input.addEventListener('keydown', event => {
            if (!owns()) return;
            const horizontal = input.type === 'range' && ['ArrowLeft','ArrowRight'].includes(event.key);
            if (burst?.input === input && !STEP_KEYS.includes(event.key) && !horizontal) endBurst();
            if ((!STEP_KEYS.includes(event.key) && !horizontal)
                || event.metaKey
                || event.ctrlKey
                || event.isComposing
                || event.defaultPrevented) return;
            const current = S().state.project.instruments[index]; if (!current || !nativeTypes.has(current.type)) return;
            event.preventDefault(); event.stopPropagation?.();
            const direction = ['ArrowUp','ArrowRight','PageUp'].includes(event.key) ? 1 : -1, coarse = event.shiftKey || event.key.startsWith('Page');
            const from = valueFromInput(input, spec),
              next = F().stepValue(Number.isFinite(from) ? from : valueOf(current, key), direction, { fine: event.altKey && !coarse, coarse }, spec);
            if (burst && burst.input !== input) endBurst();
            if (!burst) burst = { input, timer: null };
            clearTimeout(burst.timer); burst.timer = setTimeout(endBurst, BURST_MS);
            if (next === from) return;
            input.value = inputValue(input, spec, next); input.dispatchEvent(new Event('input',{bubbles:true}));
          });
          input.addEventListener('blur', () => { if (burst?.input === input) endBurst(); });
          if (input.type === 'range') F().bindDrag(input);
        }
        // step="any" keeps exact triplet periods valid; explicit arrows still give
        // predictable keyboard increments without the browser snapping to a grid.
        if (spec.unit === 'beats') input.addEventListener('keydown', event => {
          if (!owns()) return;
          if (!['ArrowUp','ArrowDown'].includes(event.key) || event.altKey || event.metaKey || event.ctrlKey) return;
          event.preventDefault();
          const current = S().state.project.instruments[index]; if (!current || !nativeTypes.has(current.type)) return;
          const value = bound((Number.isFinite(input.valueAsNumber) ? input.valueAsNumber : valueOf(current, key))
              + (event.key === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? 1 : spec.step),
              spec.min, spec.max);
          input.value = String(value);
          input.dispatchEvent(new Event('input', { bubbles: true }));
          input.dispatchEvent(new Event('change', { bubbles: true }));
        });
      }
      for (const select of root.querySelectorAll('[data-mod-mode]')) select.addEventListener('change', () => {
        if (!owns()) return;
        if (!['hz','sync'].includes(select.value)) return;
        cancelPreview();
        const prefix = select.dataset.modMode, index = Number(select.dataset.modInstrument);
        if (!['lfo','lfo2'].includes(prefix)) return;
        const current = S().state.project.instruments[index]; if (!current || !nativeTypes.has(current.type)) return;
        commit(`Set ${prefix === 'lfo' ? 'LFO 1' : 'LFO 2'} timing`, p => { p.instruments[index][`${prefix}Sync`] = select.value === 'sync'; });
      });
      for (const select of root.querySelectorAll('[data-mod-division]')) select.addEventListener('change', () => {
        if (!owns()) return;
        const division = divisions.find(item => item.id === select.value),
          prefix = select.dataset.modDivision,
          index = Number(select.dataset.modInstrument);
        if (!division || !['lfo','lfo2'].includes(prefix)) return;
        const current = S().state.project.instruments[index]; if (!current || !nativeTypes.has(current.type)) return;
        cancelPreview();
        commit(`Set ${prefix === 'lfo' ? 'LFO 1' : 'LFO 2'} division`, p => { p.instruments[index][`${prefix}Beats`] = division.beats; });
      });
      for (const select of root.querySelectorAll('[data-mod-select]')) select.addEventListener('change', () => {
        if (!owns()) return;
        cancelPreview();
        const key = select.dataset.modSelect, index = Number(select.dataset.modInstrument), allowed = key.endsWith('Shape') ? shapes : routes;
        if (!allowed.some(([value]) => value === select.value)) return;
        commit(`Set ${key.startsWith('lfo2') ? 'LFO 2' : 'LFO 1'} ${key.endsWith('Shape') ? 'waveform' : 'destination'}`, p => {
          if (p.instruments[index] && nativeTypes.has(p.instruments[index].type)) p.instruments[index][key] = select.value;
        });
      });
      root.addEventListener('focusout', () => { if (!root.isConnected && previewOwner === root) cancelPreview(); });
    }
  }

  window.STModulation = { render, bind, targets, supports, defaults };
})();
