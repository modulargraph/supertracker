/* Automation panel under the pattern grid (like Renoise's automation editor below its pattern editor): one
   parameter's lane on a horizontal row axis, with a translucent grid line per pattern row (stronger on beats and
   bars). Rows are linked both ways: hovering a row in the panel tints the matching grid row (tr.row-link), hovering
   a grid row tints the panel's column, and the grid cursor row is drawn in the panel. With the panel focused, ← →
   move that same cursor, so the link works from the keyboard too.

   Two scopes:
   - Pattern: the current pattern's rows.
   - Song: every song slot in song order along one axis, the slots' pattern names as headers in their song-tab
     colours, scrolling from the start of the song to the end. Strokes may cross pattern boundaries.

   Automation stays stored per pattern (docs/AUTOMATION.md): an edit is split per slot and written into each slot's
   pattern lane in one undo step, so a repeated pattern changes in every repeat (the toast says so). A slot without
   a lane holds the value the previous one left (dashed), as the engine plays it.
   Mouse, as in the Automation view: drag adds or moves a point, ⇧-drag draws a straight line, ⌘-drag draws freehand
   (one point per row), ⌥-click removes a point. Keys on the lane: ← → row (⇧ beat), ↑ ↓ value (⇧ coarse) at the
   cursor row, Enter adds a point there, ⌫ removes it, ⌥⇧A or Esc back to the grid.

   Shown/scope is a saved view preference, like Hide empty: project.viewState.automationPanel = 'pattern' | 'song'
   (absent: hidden). The lane markup is shared with the Automation view's lane stack (studio.js). */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, refresh, pattern, mutate, clamp } = app;
  const AS = () => window.STAutomationSong;
  const SM = () => window.STSongModel;
  const HEIGHT = 60, TOP = 4, BOTTOM = 56, MIN_ROW_WIDTH = 6, SONG_ROW_WIDTH = 6;
  const panel = { ref: null, zoom: 1, width: 0, lastScope: 'pattern' };
  const pad2 = n => String(n).padStart(2, '0');

  const scope = (p = state.project) => p?.viewState?.automationPanel === 'song' ? 'song'
      : p?.viewState?.automationPanel ? 'pattern' : null;
  const shown = () => !!scope();

  // ---- Lanes and values ------------------------------------------------------------------
  const ownerOf = lane => AS()?.normalizeOwner(lane.owner ?? lane.track, state.project) ?? String(lane.owner ?? lane.track);
  const refKey = ref => ref ? `${ref.owner}|${ref.target}` : '';

  /** Every automated parameter of the song: the current pattern's lanes first, then the rest in song order. */
  function refs(p = state.project) {
    const seen = new Map(), order = [pattern(), ...(p.sequence || []).map(id => p.patterns.find(x => x.id === id)), ...p.patterns];
    for (const item of order) for (const lane of item?.automation || []) {
      const ref = { owner: ownerOf(lane), target: lane.target, name: lane.name || lane.target };
      if (!seen.has(refKey(ref))) seen.set(refKey(ref), ref);
    }
    return [...seen.values()];
  }

  function currentRef() {
    const list = refs();
    const kept = panel.ref && list.find(ref => refKey(ref) === refKey(panel.ref));
    if (kept) return kept;
    const lane = pattern().automation?.[state.selectedLane] || pattern().automation?.[0];
    panel.ref = lane ? list.find(ref => ref.target === lane.target && ref.owner === ownerOf(lane)) : list[0] || null;
    return panel.ref;
  }

  /** Range and display of a parameter: the Automation view's target info, or a plain 0…1 lane. */
  function laneInfo(ref) {
    const info = ref && window.STStudio?.laneInfo?.({ owner: ref.owner, target: ref.target, name: ref.name });
    return info || { min: 0, max: 1, step: .01, value: 0, format: value => Number(value).toFixed(2) };
  }
  const toUnit = (value, info) => AS() ? AS().toUnit(value, info) : clamp((value - info.min) / ((info.max - info.min) || 1), 0, 1);
  const fromUnit = (unit, info) => AS() ? AS().fromUnit(unit, info) : info.min + clamp(unit, 0, 1) * (info.max - info.min);

  /** The slots shown: [{flat, pattern, start, length, points, interpolation, incoming, occurrences}] along one row axis. */
  function slots(ref, info, which = scope() || 'pattern', p = state.project) {
    const base = Number.isFinite(Number(info.value)) ? Number(info.value) : info.min;
    if (which === 'song' && AS()) return AS().songLane(p, ref.owner, ref.target, { base });
    const current = p.patterns.find(x => x.id === p.activePattern) || p.patterns[0];
    const lane = ref && AS()?.findLane(p, current, ref.owner, ref.target);
    const points = (lane?.points || []).map(point => ({ row: Number(point.row), value: Number(point.value) })).sort((a, b) => a.row - b.row);
    return [{ flat: -1, pattern: current.id, start: 0, length: current.length, points, lane, interpolation: lane?.interpolation === 'step' ? 'step' : 'linear',
      incoming: base, occurrences: 1 }];
  }
  const totalRows = list => list.length ? list[list.length - 1].start + list[list.length - 1].length : 0;
  const valueAt = (slot, row) => slot.points.length && AS() ? AS().valueAt(slot.points, row, slot.interpolation) : slot.incoming;

  /** The slot that holds the grid cursor: the song position when it plays the shown pattern, else its first slot. */
  function cursorSlot(list) {
    if (list.length === 1 && list[0].flat < 0) return list[0];
    const active = state.project.activePattern;
    return list[state.sequence]?.pattern === active ? list[state.sequence] : list.find(slot => slot.pattern === active) || null;
  }

  // ---- Geometry and markup (shared with the Automation view's lane boxes) -----------------
  /** Pixels per row: a pattern fills the lane width; the song uses a fixed, zoomable width per row. */
  function rowWidth(rows, which = scope(), available = panel.width) {
    if (which === 'song') return SONG_ROW_WIDTH * panel.zoom;
    const width = available || Math.max(320, (Number(window.innerWidth) || 1280) - 280);
    return Math.max(MIN_ROW_WIDTH, Math.floor(width / Math.max(1, rows)));
  }

  /** SVG for one lane over `list` slots: row grid, holds, curve and points; `cursor` is a global row or null.
      `stretch` fills the container's width (compact, dot-less boxes in the Automation view). */
  function laneMarkup(list, info, px, { height = HEIGHT, top = TOP, bottom = BOTTOM, cursor = null, dots = true, stretch = false } = {}) {
    const p = state.project, rows = totalRows(list), width = Math.max(1, rows * px);
    const beat = Math.max(1, (p.linesPerBeat || 4) * 4 / (p.timeSignature?.denominator || 4)), bar = Math.max(1, (p.linesPerBeat || 4) * app.beatsPerBar(p));
    const y = value => bottom - toUnit(value, info) * (bottom - top);
    let grid = '', curve = '', marks = '';
    for (const slot of list) {
      for (let row = 0; row < slot.length; row++) {
        const cls = row === 0 && slot.flat > 0 ? 'auto-row-slot' : row % bar === 0 ? 'auto-row-bar' : row % beat === 0 ? 'auto-row-beat' : 'auto-row';
        grid += `<line x1="${(slot.start + row) * px}" x2="${(slot.start + row) * px}" y1="0" y2="${height}" class="${cls}"/>`;
      }
      const x0 = slot.start * px, x1 = (slot.start + slot.length) * px;
      if (!slot.points.length) { curve += `<path d="M ${x0} ${y(slot.incoming)} H ${x1}" class="studio-song-hold"/>`; continue; }
      let d = '';
      slot.points.forEach((point, i) => {
        const x = (slot.start + point.row) * px, py = y(point.value);
        d += i === 0 ? `M ${x0} ${py} L ${x} ${py}` : slot.interpolation === 'step' ? ` H ${x} V ${py}` : ` L ${x} ${py}`;
        if (dots) marks += `<circle cx="${x}" cy="${py}" r="3" class="studio-envelope-point" data-panel-point="${slot.start + point.row}">`
            + `<title>${slot.flat >= 0 ? `Slot ${pad2(slot.flat + 1)} · ` : ''}row ${Number(point.row.toFixed(3))}: ${esc(info.format(point.value))}</title></circle>`;
      });
      curve += `<path d="${d} H ${x1}" class="studio-envelope-line"/>`;
    }
    const cursorLine = cursor === null ? '' : `<rect class="auto-row-cursor" x="${cursor * px}" y="0" width="${Math.max(1, px)}" height="${height}"/>`;
    return `<svg width="${stretch ? '100%' : width}" height="${height}" viewBox="0 0 ${width} ${height}"${stretch ? ' preserveAspectRatio="none"' : ''} class="auto-lane-svg" aria-hidden="true">`
        + `<rect width="${width}" height="${height}" class="studio-plot-bg"/>${grid}${cursorLine}<rect class="auto-row-link" x="0" y="0" width="${px}"`
        + ` height="${height}" visibility="hidden"/>${curve}<path class="auto-stroke-preview" d=""/>${marks}`
        + `<line class="auto-playhead" x1="0" x2="0" y1="0" y2="${height}" visibility="hidden"/></svg>`;
  }

  function slotHeaders(list, px) {
    const p = state.project;
    return `<div class="auto-panel-slots" aria-hidden="true">${list.map(slot => {
      const item = p.patterns.find(x => x.id === slot.pattern);
      return `<span class="auto-panel-slot ${slot.flat === cursorSlot(list)?.flat ? 'is-current' : ''}" data-panel-slot="${slot.flat}"`
          + ` style="left:${slot.start * px}px;width:${slot.length * px}px;--pattern-color:${SM()?.patternColor?.(p, slot.pattern) || 'var(--amber)'}"`
          + ` title="Slot ${pad2(slot.flat + 1)} · ${esc(item?.name || 'Missing pattern')}${slot.occurrences > 1 ? ` · plays ${slot.occurrences}× in the song` : ''}">`
          + `<b>P${pad2(SM()?.patternNumber?.(p, slot.pattern) || 0)}</b> ${esc(item?.name || 'Missing pattern')}</span>`;
    }).join('')}</div>`;
  }

  function automationPanel() {
    if (!shown()) return '';
    const which = scope(), ref = currentRef(), list = refs();
    const header = `<div class="auto-panel-head"><span class="eyebrow">Automation</span>`
        + `<div class="studio-scope-toggle" role="group" aria-label="Automation panel scope"><button id="auto-panel-scope-pattern"`
        + ` class="${which === 'pattern' ? 'is-selected' : ''}" aria-pressed="${which === 'pattern'}" title="This pattern's rows">Pattern</button>`
        + `<button id="auto-panel-scope-song" class="${which === 'song' ? 'is-selected' : ''}" aria-pressed="${which === 'song'}"`
        + ` title="The whole song, slot after slot">Song</button></div>${list.length ? `<select id="auto-panel-lane" aria-label="Parameter shown in the`
        + ` automation panel">${list.map(item => `<option value="${esc(refKey(item))}" ${refKey(item) === refKey(ref) ? 'selected' : ''}>`
        + `${esc(window.STStudio?.channelTitle?.(item.owner) || item.owner)} · ${esc(item.name)}</option>`).join('')}</select>` : ''}`
        + `${which === 'song' ? '<button class="quiet" id="auto-panel-zoom-out" title="Zoom out · −">−</button><button class="quiet" id="auto-panel-zoom-in" title="Zoom in · +">+</button>' : ''}`
        + `<span class="grow"></span><output class="auto-panel-readout" id="auto-panel-readout" aria-live="off"></output><button class="quiet" id="auto-panel-open"`
        + ` title="Edit points in the Automation view">Edit…</button><button class="quiet" id="auto-panel-close" title="Hide the automation panel · ⌘K">×</button></div>`;
    if (!ref) return `<section class="auto-panel" id="auto-panel" aria-label="Automation panel">${header}<p class="auto-panel-empty muted">No automation yet.`
        + ` Focus a parameter and press ⌥A to automate it.</p></section>`;
    const info = laneInfo(ref), lanes = slots(ref, info), px = rowWidth(totalRows(lanes)), at = cursorSlot(lanes);
    const label = value => `<span>${esc(info.format(value))}</span>`;
    return `<section class="auto-panel" id="auto-panel" aria-label="Automation panel" data-scope="${which}">${header}<div class="auto-panel-body">`
        + `<div class="auto-panel-gutter" aria-hidden="true">${label(fromUnit(1, info))}${label(fromUnit(0, info))}</div>`
        + `<div class="auto-panel-scroll" id="auto-panel-scroll" data-scroll="auto-panel"><div class="auto-panel-track" style="width:${totalRows(lanes) * px}px">`
        + `${which === 'song' ? slotHeaders(lanes, px) : ''}<div class="auto-panel-lane" id="auto-panel-envelope" tabindex="0" role="application" data-px="${px}"`
        + ` aria-label="${esc(ref.name)} automation${which === 'song' ? ' across the song' : ''}. Left and right move the row cursor, up and down`
        + ` change the value at the cursor, Enter adds a point, Delete removes it, Escape returns to the grid.">`
        + `${laneMarkup(lanes, info, px, { cursor: at ? at.start + state.row : null })}</div></div></div></div></section>`;
  }

  // ---- Row link between the panel and the grid -------------------------------------------
  function hit(list, x, px) {
    const row = clamp(x / px, 0, Math.max(0, totalRows(list) - 1e-6));
    const slot = list.find(item => row < item.start + item.length) || list[list.length - 1];
    return { slot, row: row - slot.start, global: row };
  }

  /** Tint the grid row of `slot` (song scroll mode: only that slot's copy of the row). */
  function linkGridRow(slot, row) {
    for (const tr of $$('#tracker tr.row-link')) tr.classList.remove('row-link');
    if (!slot || row === null) return;
    const songScroll = app.songScrollOn?.();
    if (!songScroll && slot.pattern !== state.project.activePattern) return;
    const selector = songScroll && slot.flat >= 0 ? `#tracker tr[data-slot="${slot.flat}"][data-row="${row}"]` : `#tracker tr[data-row="${row}"]:not([data-context])`;
    $(selector)?.classList.add('row-link');
  }

  function linkPanelRow(global) {
    const area = $('#auto-panel-envelope'), rect = area?.querySelector?.('.auto-row-link');
    if (!rect) return;
    const px = Number(area.dataset.px) || 1;
    if (global === null) return rect.setAttribute('visibility', 'hidden');
    rect.setAttribute('visibility', 'visible');
    rect.setAttribute('x', String(global * px));
  }

  /** A grid row was hovered: `flat` is its song slot (song scroll) or -1 for the live pattern. */
  function gridHover(flat, row) {
    const ref = currentRef(); if (!$('#auto-panel-envelope') || !ref) return;
    if (row === null) return linkPanelRow(null);
    const list = slots(ref, laneInfo(ref)), slot = scope() === 'song' ? (flat >= 0 ? list[flat] : cursorSlot(list)) : list[0];
    linkPanelRow(slot && row < slot.length ? slot.start + row : null);
  }

  /** Transport tick: the playhead line, without a refresh. */
  function paintAutomationPanelPlayhead(status = state.status || {}) {
    const area = $('#auto-panel-envelope'), line = area?.querySelector?.('.auto-playhead'), ref = currentRef();
    if (!line || !ref) return;
    const list = slots(ref, laneInfo(ref)), px = Number(area.dataset.px) || 1, position = Number(status.row || 0) + clamp(status.fraction ?? 0, 0, 1);
    const slot = !status.playing ? null : scope() === 'song'
        ? status.songMode && Number.isInteger(status.sequence) ? list[status.sequence] : null
        : status.pattern === list[0].pattern ? list[0] : null;
    if (!slot) return line.setAttribute('visibility', 'hidden');
    const x = String((slot.start + clamp(position, 0, slot.length)) * px);
    line.setAttribute('x1', x); line.setAttribute('x2', x); line.setAttribute('visibility', 'visible');
  }

  // ---- Editing -----------------------------------------------------------------------------
  /** Write one edit into the slots it covers (one undo step): `set` global row → value, `remove` global rows,
      `clear` a [from, to] global range replaced by `set`. Returns the touched pattern ids. */
  function writeAutomation(ref, info, list, { set = new Map(), remove = new Set(), clear = null } = {}, label = 'Draw automation') {
    const rows = [...set.keys(), ...remove, ...(clear || [])];
    if (!rows.length || !AS()) return [];
    const from = Math.min(...rows), to = Math.max(...rows), touched = [];
    const edits = list.filter(slot => !slot.missing && slot.length > 0 && slot.start + slot.length > from && slot.start <= to);
    if (!edits.length) return [];
    mutate(label, p => {
      for (const slot of edits) {
        const target = p.patterns.find(x => x.id === slot.pattern); if (!target) continue;
        const inside = row => row >= slot.start && row < slot.start + slot.length;
        const lane = AS().ensureLane(p, target, ref.owner, ref.target, { uid: app.uid, name: ref.name });
        const kept = (lane.points || []).filter(point => {
          const global = slot.start + Number(point.row);
          return !remove.has(global) && !(clear && global >= clear[0] - 1e-9 && global <= clear[1] + 1e-9);
        });
        const drawn = [...set].filter(([row]) => inside(row)).map(([row, value]) => ({ row: row - slot.start, value }));
        lane.points = AS().cleanPoints([...kept, ...drawn], info, target.length);
        if (!touched.includes(slot.pattern)) touched.push(slot.pattern);
      }
    });
    const repeated = touched.map(id => [id, (state.project.sequence || []).filter(x => x === id).length]).filter(([, n]) => n > 1);
    if (repeated.length) app.toast(repeated.map(([id, n]) => `Edited all ${n} repeats of P${pad2(SM()?.patternNumber?.(state.project, id) || 0)}`).join(' · ')
        + ' — U in the Automation view’s song timeline makes a slot unique');
    return touched;
  }

  /** Move the grid cursor to a global panel row; crossing into another song slot makes that slot current. */
  function moveCursor(list, global) {
    const target = hit(list, clamp(global, 0, totalRows(list) - 1), 1), row = Math.floor(target.row); // px 1: x is the row
    if (target.slot.flat >= 0 && target.slot.flat !== state.sequence) state.sequence = target.slot.flat;
    if (target.slot.pattern !== state.project.activePattern) {
      state.project.activePattern = target.slot.pattern;
      state.selection = null;
    }
    state.row = row;
    refresh();
    focusPanel();
  }

  const focusPanel = () => {
    const area = $('#auto-panel-envelope');
    area?.focus({ preventScroll: true });
    area?.querySelector?.('.auto-row-cursor')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  };

  function panelKey(event, ref, info) {
    const list = slots(ref, info), at = cursorSlot(list);
    if (!at) return false;
    const k = event.key, global = at.start + state.row, beat = Math.max(1, state.project.linesPerBeat || 4);
    const done = () => { event.preventDefault(); event.stopPropagation?.(); return true; };
    if (event.metaKey || event.ctrlKey) return false;
    if (k === 'Escape' || event.altKey && event.shiftKey && (event.code === 'KeyA' || String(k).toLowerCase() === 'a')) {
      done(); $('#tracker')?.focus({ preventScroll: true }); return true;
    }
    if (k === 'ArrowLeft' || k === 'ArrowRight') { done(); moveCursor(list, global + (k === 'ArrowLeft' ? -1 : 1) * (event.shiftKey ? beat : 1)); return true; }
    if (k === 'Home' || k === 'End') { done(); moveCursor(list, k === 'Home' ? at.start : at.start + at.length - 1); return true; }
    const existing = at.points.find(point => Math.abs(point.row - state.row) < 1e-9);
    if (k === 'ArrowUp' || k === 'ArrowDown' || k === 'Enter') {
      done();
      const unit = toUnit(existing ? existing.value : valueAt(at, state.row), info);
      const step = k === 'Enter' ? 0 : (k === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? .1 : .01);
      writeAutomation(ref, info, list, { set: new Map([[global, fromUnit(clamp(unit + step, 0, 1), info)]]) },
          existing ? 'Edit automation point' : 'Add automation point');
      focusPanel();
      return true;
    }
    if ((k === 'Delete' || k === 'Backspace') && existing) {
      done(); writeAutomation(ref, info, list, { remove: new Set([global]) }, 'Remove automation point'); focusPanel(); return true;
    }
    if (scope() === 'song' && (k === '+' || k === '=' || k === '-')) { done(); zoom(k === '-' ? -1 : 1); return true; }
    return false;
  }

  function zoom(direction) {
    panel.zoom = clamp(direction > 0 ? panel.zoom * 1.5 : panel.zoom / 1.5, .25, 4);
    refresh();
    focusPanel();
  }

  function bindAutomationPanel() {
    const on = (id, fn) => { const node = $('#' + id); if (node) node.onclick = fn; };
    on('auto-panel-scope-pattern', () => setPanel('pattern'));
    on('auto-panel-scope-song', () => setPanel('song'));
    on('auto-panel-close', () => setPanel(null));
    on('auto-panel-zoom-in', () => zoom(1));
    on('auto-panel-zoom-out', () => zoom(-1));
    on('auto-panel-open', () => {
      const ref = currentRef(), index = ref ? pattern().automation.findIndex(lane => lane.target === ref.target && ownerOf(lane) === ref.owner) : -1;
      if (index >= 0) state.selectedLane = index;
      app.setView('automation');
    });
    const select = $('#auto-panel-lane');
    if (select) select.onchange = () => {
      panel.ref = refs().find(ref => refKey(ref) === select.value) || null;
      refresh();
      $('#auto-panel-lane')?.focus({ preventScroll: true });
    };
    const scroller = $('#auto-panel-scroll');
    if (scroller?.clientWidth) panel.width = scroller.clientWidth;
    const tracker = $('#tracker');
    if (tracker) {
      tracker.onmouseover = event => {
        const tr = event.target?.closest?.('tr[data-row]');
        if (!tr) return;
        for (const old of $$('#tracker tr.row-link')) if (old !== tr) old.classList.remove('row-link');
        tr.classList.add('row-link');
        gridHover(tr.dataset.slot !== undefined ? Number(tr.dataset.slot) : -1, Number(tr.dataset.row));
      };
      tracker.onmouseleave = () => { for (const old of $$('#tracker tr.row-link')) old.classList.remove('row-link'); gridHover(-1, null); };
    }
    const area = $('#auto-panel-envelope'), svg = area?.querySelector?.('svg'), ref = currentRef();
    if (!area || !svg || !ref) return;
    const info = laneInfo(ref), px = Number(area.dataset.px) || 1;
    // Block body: returning panelKey's false from an on-handler would cancel keys the panel does not use (F6, ⌘1…).
    area.onkeydown = event => { panelKey(event, ref, info); };
    const locate = event => {
      const box = svg.getBoundingClientRect(), list = slots(ref, info), at = hit(list, event.clientX - box.left, px);
      const unit = 1 - clamp((event.clientY - box.top - TOP) / (BOTTOM - TOP), 0, 1);
      return { list, ...at, value: fromUnit(unit, info) };
    };
    const readout = $('#auto-panel-readout');
    svg.onmousemove = event => {
      const at = locate(event);
      linkPanelRow(Math.floor(at.global));
      linkGridRow(at.slot, Math.floor(at.row));
      if (readout) readout.textContent = `${at.slot.flat >= 0 ? `Slot ${pad2(at.slot.flat + 1)} · ` : ''}Row ${pad2(Math.floor(at.row))} · ${info.format(at.value)}`;
    };
    svg.onmouseleave = () => { linkPanelRow(null); linkGridRow(null, null); if (readout) readout.textContent = ''; };
    svg.onpointerdown = event => startGesture(event, svg, ref, info, px, locate);
  }

  /* Drag adds or moves one point (inside its slot), ⇧-drag a straight line, ⌘-drag freehand; ⌥-click removes. */
  function startGesture(event, svg, ref, info, px, locate) {
    if (event.button !== 0) return;
    event.preventDefault?.();
    $('#auto-panel-envelope')?.focus({ preventScroll: true });
    const start = locate(event), startRow = Math.round(start.global);
    const nearest = start.slot.points.reduce((best, point) => !best || Math.abs(point.row - start.row) < Math.abs(best.row - start.row) ? point : best, null);
    const grabbed = nearest && Math.abs(nearest.row - start.row) * px <= 6 ? start.slot.start + nearest.row : null;
    if (event.altKey) {
      if (grabbed !== null) writeAutomation(ref, info, start.list, { remove: new Set([grabbed]) }, 'Remove automation point');
      return;
    }
    const mode = event.metaKey || event.ctrlKey ? 'freehand' : event.shiftKey ? 'line' : 'point';
    const preview = svg.querySelector('.auto-stroke-preview'), y = value => BOTTOM - toUnit(value, info) * (BOTTOM - TOP);
    const stroke = new Map([[startRow, start.value]]);
    let last = { row: startRow, value: start.value }, end = last;
    const draw = () => preview?.setAttribute('d', [...stroke].sort((a, b) => a[0] - b[0]).map(([r, v], i) => `${i ? 'L' : 'M'} ${r * px} ${y(v)}`).join(' '));
    const move = e => {
      const at = locate(e), row = Math.round(at.global);
      end = { row, value: at.value };
      if (mode === 'point') {
        // A point stays inside the slot it started in.
        const lo = start.slot.start, hi = start.slot.start + start.slot.length - 1;
        stroke.clear(); stroke.set(clamp(row, lo, hi), at.value);
      } else if (mode === 'line') {
        stroke.clear();
        const a = Math.min(startRow, row), b = Math.max(startRow, row);
        for (let r = a; r <= b; r++) stroke.set(r, b === a ? at.value : r === startRow ? start.value : start.value + (at.value - start.value) * (r - startRow) / (row - startRow));
      } else {
        // Fill skipped rows so a fast drag still writes one point per row.
        const step = row >= last.row ? 1 : -1;
        for (let r = last.row + step; step > 0 ? r <= row : r >= row; r += step) stroke.set(r, last.value + (at.value - last.value) * (r - last.row) / ((row - last.row) || 1));
        last = end;
      }
      draw();
    };
    const up = () => {
      svg.removeEventListener('pointermove', move);
      svg.removeEventListener('pointerup', up);
      svg.removeEventListener('pointercancel', up);
      const list = slots(ref, info);
      if (mode === 'point') writeAutomation(ref, info, list, { set: stroke, remove: grabbed !== null && !stroke.has(grabbed) ? new Set([grabbed]) : new Set() },
          grabbed !== null ? 'Move automation point' : 'Add automation point');
      else {
        const rows = [...stroke.keys()];
        writeAutomation(ref, info, list, { set: stroke, clear: [Math.min(...rows), Math.max(...rows)] });
      }
    };
    svg.setPointerCapture?.(event.pointerId);
    svg.addEventListener('pointermove', move);
    svg.addEventListener('pointerup', up);
    svg.addEventListener('pointercancel', up);
  }

  function setPanel(value, { focus = false } = {}) {
    const next = value === 'song' || value === 'pattern' ? value : null;
    if (next) panel.lastScope = next;
    app.setViewState(view => { view.automationPanel = next || undefined; });
    refresh();
    if (next && focus) focusPanel();
    if (!next) $('#tracker')?.focus({ preventScroll: true });
    return true;
  }

  /** ⌥⇧A: open the panel and focus its lane; from the lane, back to the grid. */
  function cycleAutomationPanel() {
    if (!shown()) return setPanel(panel.lastScope, { focus: true });
    if (document.activeElement?.closest?.('#auto-panel')) { $('#tracker')?.focus({ preventScroll: true }); return true; }
    focusPanel();
    return true;
  }

  app.defineCommands({
    automationPanel: () => { if (state.view !== 'pattern') app.setView('pattern'); return cycleAutomationPanel(); },
    automationPanelHide: () => setPanel(null),
    automationPanelSong: () => setPanel(scope() === 'song' ? 'pattern' : 'song', { focus: true })
  });

  Object.assign(app, {
    automationPanel, bindAutomationPanel, automationPanelShown: shown, automationPanelScope: scope, setAutomationPanel: setPanel,
    automationLaneMarkup: laneMarkup, automationLaneSlots: slots, automationLaneInfo: laneInfo, automationRowWidth: rowWidth,
    automationPanelRefs: refs, writeAutomation, automationPanelHover: gridHover, paintAutomationPanelPlayhead,
    selectAutomationPanelLane: ref => { panel.ref = ref; refresh(); }
  });
})();
