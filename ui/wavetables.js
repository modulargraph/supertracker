/* Sampler Wavetable mode: the built-in table catalogue (sample.wavetables), the Wavetable panel on
   the instrument's Sample page (Position, frame size, table picker) and a preview of the frame
   that Position selects. Built-in tables are project references: builtin:wavetables/<group>/<name>.wav. */
(() => {
  'use strict';
  const ST = () => window.ST;
  const PREFIX = 'builtin:wavetables/';
  const GROUPS = ['Basic', 'Morph'];
  const FRAME_SIZES = [256, 512, 1024, 2048, 4096, 8192];
  const DEFAULT_FRAME = 2048;
  let catalogue = null, loading = null;
  const preview = { key: '', pending: '', timer: null, line: null };

  const pathFor = (name, group = 'Basic') => `${PREFIX}${group}/${name}.wav`;
  function parse(path) {
    if (typeof path !== 'string' || !path.startsWith(PREFIX)) return null;
    const [group, file, extra] = path.slice(PREFIX.length).split('/');
    return group && file && extra === undefined ? { group, name: file.replace(/\.wav$/i, ''), path } : null;
  }
  /** Readable table name for a built-in path ('' for any other sample). */
  const displayName = path => parse(path)?.name || '';
  const fileName = path => String(path || '').split('/').pop();

  function accept(list) {
    catalogue = (Array.isArray(list) ? list : [])
      .filter(item => item && typeof item.path === 'string' && parse(item.path) && item.wavetable !== false)
      .map(item => ({ group: String(item.group || parse(item.path).group), name: String(item.name || parse(item.path).name),
        path: item.path, frames: Number(item.frames) || 0, frameSize: Number(item.frameSize) || 0 }));
    return catalogue;
  }

  /** The built-in tables, from the native catalogue (cached after the first answer). */
  function load(force = false) {
    if (catalogue && !force) return Promise.resolve(catalogue);
    if (loading) return loading;
    loading = Promise.resolve(ST()?.request?.('sample.wavetables'))
      .then(list => accept(list))
      .catch(() => catalogue || [])
      .finally(() => { loading = null; });
    return loading;
  }

  // ---- Frames ----
  function frameInfo(inst, meta) {
    const frames = Number(meta?.frames) || 0,
      size = Number(inst.wavetableFrameSize) > 0 ? Number(inst.wavetableFrameSize)
        : Number(meta?.wavetableFrameSize) > 0 ? Number(meta.wavetableFrameSize) : DEFAULT_FRAME;
    if (!frames) return { frames: 0, size, count: 0, index: 0 };
    const first = Math.floor(inst.start * frames), last = Math.ceil(inst.end * frames),
      region = Math.max(1, last - first), width = Math.min(size, region), count = Math.max(1, Math.floor(region / width));
    const position = Math.min(1, Math.max(0, Number(inst.wavetablePosition) || 0)),
      index = Math.min(count - 1, Math.round(position * (count - 1)));
    return { frames, size, width, count, index, start: (first + index * width) / frames, end: (first + (index + 1) * width) / frames };
  }

  function frameText(info) {
    return info.count ? `Frame ${info.index + 1} / ${info.count}` : 'Loading table…';
  }

  // ---- Panel ----
  function tableOptions(inst) {
    const current = parse(inst.samplePath), list = catalogue || (current ? [current] : []);
    const groups = [...new Set([...GROUPS, ...list.map(item => item.group)])];
    const option = item => `<option value="${esc(item.path)}" ${item.path === inst.samplePath ? 'selected' : ''}>${esc(item.name)}</option>`;
    let html = groups.map(group => {
      const items = list.filter(item => item.group === group);
      return items.length ? `<optgroup label="${esc(group)}">${items.map(option).join('')}</optgroup>` : '';
    }).join('');
    if (current && catalogue && !catalogue.some(item => item.path === inst.samplePath)) html += `<option value="${esc(inst.samplePath)}" selected>${esc(current.name)}</option>`;
    const own = inst.samplePath && !current;
    html += `<optgroup label="Sample">${own ? `<option value="" selected>Current sample · ${esc(fileName(inst.samplePath))}</option>` : ''}`
      + `<option value="load">Load a wavetable file…</option></optgroup>`;
    if (!catalogue) html += '<option value="" disabled>Loading built-in tables…</option>';
    if (!inst.samplePath) html = '<option value="" selected>No table · choose one</option>' + html;
    return html;
  }

  function frameSizeOptions(inst, meta) {
    const detected = Number(meta?.wavetableFrameSize) || 0, value = Number(inst.wavetableFrameSize) || 0;
    return [`<option value="0" ${FRAME_SIZES.includes(value) ? '' : 'selected'}>Auto${detected ? ` · ${detected}` : ''}</option>`]
      .concat(FRAME_SIZES.map(size => `<option value="${size}" ${value === size ? 'selected' : ''}>${size}</option>`)).join('');
  }

  /** The Wavetable panel for a sampler in Wavetable mode ('' otherwise). */
  function render(inst, meta) {
    if (!inst || inst.type !== 'sample' || inst.mode !== 'wavetable') return '';
    if (!catalogue) void load().then(() => repaintTables());
    const app = ST(), info = frameInfo(inst, meta), position = Number(inst.wavetablePosition) || 0;
    const name = displayName(inst.samplePath) || (inst.samplePath ? fileName(inst.samplePath) : 'No table');
    const control = app.parameter('wavetablePosition', 'Position', 0, 1, .001, position, 'wavetablePosition', { name: 'Wavetable position' });
    return `<section class="panel wide wavetable-panel" id="inst-group-wavetable" aria-label="Wavetable"><div class="panel-head">`
      + `<div class="row"><h2>Wavetable</h2><span class="chip" id="wavetable-table-chip">${esc(name.toUpperCase())}</span></div>`
      + `<span class="inst-hint" aria-hidden="true">↑↓ fine · ⇧ coarse · ⌥A automate</span></div>`
      + `<div class="panel-body wavetable-body"><div class="wavetable-position">${control}</div>`
      + `<figure class="wavetable-preview" aria-label="Current frame"><svg id="wavetable-preview" viewBox="0 0 256 80" preserveAspectRatio="none" role="img" `
      + `aria-label="Waveform of the current frame"><line x1="0" y1="40" x2="256" y2="40" class="wavetable-axis"/>`
      + `<path id="wavetable-preview-line" class="wavetable-line" d="${esc(preview.line && preview.key === previewKey(inst, info) ? preview.line : '')}"/></svg>`
      + `<figcaption id="wavetable-frame">${esc(frameText(info))}</figcaption></figure>`
      + `<div class="wavetable-controls"><label class="field"><span>Table</span><select id="wavetable-table" aria-label="Wavetable table">${tableOptions(inst)}</select></label>`
      + `<label class="field"><span>Frame size</span><select id="wavetable-frame-size" aria-label="Wavetable frame size">${frameSizeOptions(inst, meta)}</select></label></div>`
      + `<p class="inst-caption wavetable-help">Position scans the table’s frames (automation, LFO → Position and the step effect `
      + `<b>POS</b> move it too). Frame size Auto uses the size stored in the file, else 2048 samples.</p></div></section>`;
  }

  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

  function repaintTables() {
    const select = document.getElementById('wavetable-table'), inst = selected();
    if (!select || !inst || inst.mode !== 'wavetable' || document.activeElement === select) return;
    select.innerHTML = tableOptions(inst);
  }

  // ---- Binding ----
  function selected() {
    const s = ST()?.state;
    if (!s?.project) return null;
    return s.project.instruments[s.instrument] || null;
  }

  function chooseTable(inst, path) {
    const table = parse(path);
    if (!table) return false;
    return ST().mutate('wavetable table', () => {
      delete inst.plugin;
      Object.assign(inst, { type: 'sample', samplePath: path, mode: 'wavetable', start: 0, end: 1, loopStart: 0, loopEnd: 1,
        slicePoints: [], slices: [], zones: [] });
    });
  }

  function bind() {
    const inst = selected();
    if (!inst || inst.mode !== 'wavetable') return;
    const table = document.getElementById('wavetable-table'), size = document.getElementById('wavetable-frame-size');
    if (table) table.onchange = () => {
      if (selected() !== inst) return;
      if (table.value === 'load') {
        table.innerHTML = tableOptions(inst);
        document.getElementById('load-sample')?.click();
        return;
      }
      if (table.value) chooseTable(inst, table.value);
    };
    if (size) size.onchange = () => {
      if (selected() !== inst) return;
      const value = Number(size.value);
      ST().mutate('wavetable frame size', () => { inst.wavetableFrameSize = FRAME_SIZES.includes(value) ? value : 0; });
    };
    for (const input of document.querySelectorAll('[data-inst-param="wavetablePosition"]')) {
      input.addEventListener('input', () => {
        const value = Number(input.dataset.fineMin !== undefined && window.STFine?.has(input) ? window.STFine.laneFromElement(input) : input.value);
        if (Number.isFinite(value)) schedulePreview({ ...inst, wavetablePosition: Math.min(1, Math.max(0, value)) });
      });
    }
    schedulePreview(inst);
  }

  // ---- Frame preview (sample.peaks of the current frame; the midpoint of each bin traces the cycle) ----
  const previewKey = (inst, info) => `${inst.samplePath}|${info.start?.toFixed(8)}|${info.end?.toFixed(8)}`;

  function draw(min, max) {
    const count = Math.min(min.length, max.length);
    if (!count) return '';
    let d = '';
    for (let i = 0; i < count; i++) {
      const value = Math.max(-1, Math.min(1, ((Number(min[i]) || 0) + (Number(max[i]) || 0)) / 2));
      d += `${i ? 'L' : 'M'}${(i / Math.max(1, count - 1) * 256).toFixed(1)} ${(40 - value * 36).toFixed(1)}`;
    }
    return d;
  }

  function schedulePreview(inst) {
    const s = ST()?.state, index = s?.project?.instruments.indexOf(selected());
    const meta = s?.samples?.[index], info = frameInfo(inst, meta);
    const caption = document.getElementById('wavetable-frame');
    if (caption) caption.textContent = frameText(info);
    if (!info.count || !s.native || !inst.samplePath) return;
    const key = previewKey(inst, info), line = document.getElementById('wavetable-preview-line');
    if (preview.key === key && preview.line) { if (line) line.setAttribute('d', preview.line); return; }
    if (preview.pending === key) return;
    clearTimeout(preview.timer);
    preview.timer = setTimeout(async () => {
      preview.pending = key;
      try {
        const result = await ST().request('sample.peaks', { instrument: index, path: inst.samplePath, start: info.start, end: info.end, bins: 128 });
        if (preview.pending !== key || !Array.isArray(result?.min) || !Array.isArray(result?.max)) return;
        preview.key = key; preview.line = draw(result.min, result.max);
        document.getElementById('wavetable-preview-line')?.setAttribute('d', preview.line);
      } catch { /* The table is still loading; the next render asks again. */ }
      finally { if (preview.pending === key) preview.pending = ''; }
    }, 40);
  }

  window.STWavetables = { PREFIX, FRAME_SIZES, pathFor, parse, displayName, load, accept, frameInfo, render, bind, chooseTable,
    get catalogue() { return catalogue; } };
})();
