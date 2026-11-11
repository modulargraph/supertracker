/* Sampler Granular mode: the Granular panel at the top of the instrument's Sample page (Position, grain
   Size, Density, Spray, Scan, Pitch spread, Reverse and Stereo spread) and a small grain map that shows
   where the grains read. Every control is an ordinary automatable instrument parameter (⌥A), heard live
   while it moves; LFO → Position / Grain size and the POS step effect modulate it too. */
(() => {
  'use strict';
  const ST = () => window.ST;
  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  const num = (value, fallback) => Number.isFinite(Number(value)) ? Number(value) : fallback;
  const DEFAULTS = Object.freeze({ grainPosition: .5, grainSize: .08, grainDensity: 2, grainSpray: 0, grainScan: 0, grainPitchSpread: 0, grainReverse: 0, grainStereo: 0 });
  const value = (inst, key) => num(inst?.[key], DEFAULTS[key]);

  /** Grains per second at this size and density (each grain overlaps density-1 others). */
  const rate = inst => value(inst, 'grainDensity') / Math.max(.002, value(inst, 'grainSize'));

  function summary(inst) {
    const density = Math.round(value(inst, 'grainDensity')), size = value(inst, 'grainSize');
    return `${density} × ${size < .01 ? (size * 1000).toFixed(1) : Math.round(size * 1000)} ms · ${Math.round(rate(inst))} grains/s`;
  }

  // ---- Grain map: the region, the spray band, one grain's width and the position (an SVG, 256 × 56) ----
  function map(inst, meta) {
    const duration = num(meta?.duration, 0) || (meta?.frames && meta?.sampleRate ? meta.frames / meta.sampleRate : 0);
    const start = num(inst.start, 0), end = num(inst.end, 1), span = Math.max(1e-6, end - start);
    const x = position => (start + position * span) * 256;
    const seconds = duration * span || 1, size = value(inst, 'grainSize') / seconds, spray = value(inst, 'grainSpray') / seconds;
    const at = x(value(inst, 'grainPosition')), grain = Math.max(1.5, size * span * 256), band = spray * span * 256;
    const scan = value(inst, 'grainScan');
    const arrow = Math.abs(scan) < 1e-6 ? '' : `<path class="granular-scan" d="M${at} 8 l${scan > 0 ? 10 : -10} 0 l${scan > 0 ? -4 : 4} -4 M${at + (scan > 0 ? 10 : -10)} 8 l${scan > 0 ? -4 : 4} 4"/>`;
    return `<svg id="granular-map" viewBox="0 0 256 56" preserveAspectRatio="none" role="img" aria-label="Grain map: ${esc(describe(inst))}">`
      + `<rect class="granular-region" x="${(start * 256).toFixed(1)}" y="14" width="${(span * 256).toFixed(1)}" height="34"/>`
      + (band > .2 ? `<rect class="granular-spray" x="${(at - band).toFixed(1)}" y="16" width="${(2 * band).toFixed(1)}" height="30"/>` : '')
      + `<rect class="granular-grain" x="${(at - grain / 2).toFixed(1)}" y="20" width="${grain.toFixed(1)}" height="22"/>`
      + `<line class="granular-position" x1="${at.toFixed(1)}" y1="12" x2="${at.toFixed(1)}" y2="50"/>${arrow}</svg>`;
  }

  function describe(inst) {
    const parts = [`position ${Math.round(value(inst, 'grainPosition') * 100)}%`, summary(inst)];
    if (value(inst, 'grainSpray') > 0) parts.push(`spray ±${Math.round(value(inst, 'grainSpray') * 1000)} ms`);
    const scan = value(inst, 'grainScan');
    parts.push(Math.abs(scan) < 1e-6 ? 'frozen' : `scanning ${scan > 0 ? 'forward' : 'backward'} at ${Number(Math.abs(scan).toFixed(2))}×`);
    return parts.join(', ');
  }

  /** The Granular panel for a sampler in Granular mode ('' otherwise). */
  function render(inst, meta) {
    if (!inst || inst.type !== 'sample' || inst.mode !== 'granular') return '';
    const app = ST(), p = (key, label, name, step = .001) => app.parameter(key, label, 0, 1, step, value(inst, key), key, { name });
    return `<section class="panel wide granular-panel" id="inst-group-granular" aria-label="Granular"><div class="panel-head">`
      + `<div class="row"><h2>Granular</h2><span class="chip" id="granular-chip">${esc(summary(inst).toUpperCase())}</span></div>`
      + `<span class="inst-hint" aria-hidden="true">↑↓ fine · ⇧ coarse · ⌥A automate</span></div>`
      + `<div class="panel-body granular-body"><div class="granular-position">${p('grainPosition', 'Position', 'Grain position')}</div>`
      + `<figure class="granular-preview" aria-label="Grain map">${map(inst, meta)}<figcaption id="granular-caption">${esc(describe(inst))}</figcaption></figure>`
      + `<div class="granular-controls">${p('grainSize', 'Size', 'Grain size')}${p('grainDensity', 'Density', 'Grain density', 1)}`
      + `${p('grainSpray', 'Spray', 'Grain spray')}${p('grainScan', 'Scan', 'Grain scan speed')}`
      + `${p('grainPitchSpread', 'Pitch spread', 'Grain pitch spread')}${p('grainReverse', 'Reverse', 'Grain reverse chance')}`
      + `${p('grainStereo', 'Stereo', 'Grain stereo spread')}</div>`
      + `<p class="inst-caption granular-help">Position picks where grains read (automation, LFO → Position and the step effect <b>POS</b> move it). `
      + `Scan moves it through the sample: 1× follows the original speed at any pitch, 0 freezes, negative runs backwards. `
      + `Density is how many grains overlap; Spray scatters them around Position; Pitch spread, Reverse and Stereo randomize each grain. `
      + `The waveform below shows every grain as it plays.</p></div></section>`;
  }

  function selected() {
    const s = ST()?.state;
    return s?.project?.instruments[s.instrument] || null;
  }

  /** Redraw the map and summary while a control moves (before the edit commits). */
  function bind() {
    const inst = selected();
    if (!inst || inst.mode !== 'granular') return;
    for (const key of Object.keys(DEFAULTS)) for (const input of document.querySelectorAll(`[data-inst-param="${key}"]`)) {
      input.addEventListener('input', () => {
        const next = Number(input.dataset.fineMin !== undefined && window.STFine?.has(input) ? window.STFine.laneFromElement(input) : input.value);
        if (Number.isFinite(next) && selected() === inst) repaint({ ...inst, [key]: next });
      });
    }
  }

  function repaint(inst) {
    const s = ST()?.state, figure = document.getElementById('granular-map');
    const meta = s?.samples?.[s.project.instruments.indexOf(selected())];
    if (figure) figure.outerHTML = map(inst, meta);
    const caption = document.getElementById('granular-caption'), chip = document.getElementById('granular-chip');
    if (caption) caption.textContent = describe(inst);
    if (chip) chip.textContent = summary(inst).toUpperCase();
  }

  window.STGranular = { DEFAULTS, render, bind, summary, describe, rate };
})();
