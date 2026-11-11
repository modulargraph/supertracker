/* Automation across patterns. See docs/AUTOMATION.md.
   Automation stays stored per pattern (`project.patterns[].automation`), which is what the
   engine, export, freeze and live recording already play. This module is an editing layer on
   top of that model:
   - the pure model (exported for node tests) lays the song sequence out as a timeline of
     pattern instances, evaluates one parameter's lanes across it, copies a lane to other
     patterns, writes a sweep spanning several instances into their pattern lanes, and makes a
     sequence slot's pattern unique;
   - the Song timeline UI (rendered inside the Automation view by studio.js) shows and edits a
     parameter along the song. Editing an instance edits its pattern, so every repeat of that
     pattern changes too: the view says so and offers to make the slot unique first. */
(() => {
  'use strict';
  const { clone } = typeof window !== 'undefined' && window.STUtil ? window.STUtil : require('./util.js');
  const Q = 1 / 64;               // stored row quantum (live recording resolution)
  const MAX_POINTS = 4000;        // engine lane limit is 4096
  const finite = value => value !== '' && value !== null && value !== undefined && Number.isFinite(Number(value));
  const clamp = (value, min, max) => Math.max(min, Math.min(max, Number(value) || 0));
  const quantize = row => Math.round(row / Q) * Q;
  const sorted = points => (points || []).filter(p => finite(p?.row) && finite(p?.value)).map(p => ({
    row: Number(p.row),
    value: Number(p.value)
  })).sort((a, b) => a.row - b.row);

  // ---- Pure model ---------------------------------------------------------------------
  const trackOwner = (index, p) => `track:${p.tracks[index]?.routingID ?? index}`;
  const normalizeOwner = (owner, p) => typeof owner === 'number' || /^\d+$/.test(String(owner)) ? trackOwner(Number(owner), p) : String(owner);
  const laneOwner = (lane, p) => normalizeOwner(lane.owner ?? lane.track, p);
  const patternById = (p, id) => p.patterns.find(item => Number(item.id) === Number(id)) || null;
  function findLane(p, pattern, owner, target) {
    const wanted = normalizeOwner(owner, p);
    return (pattern?.automation || []).find(lane => lane.target === target && laneOwner(lane, p) === wanted) || null;
  }
  function trackIndexOf(p, owner) {
    const wanted = normalizeOwner(owner, p);
    return wanted.startsWith('track:') ? p.tracks.findIndex((_, index) => trackOwner(index, p) === wanted) : -1;
  }
  function toUnit(value, info) {
    if (info.fineCurve && typeof window !== 'undefined' && window.STFine) return window.STFine.toPosition(value,{...info,...info.fineCurve});
    const min = Number(info.min), max = Number(info.max);
    if (!(max > min)) return 0;
    if (info.log && min > 0) return clamp(Math.log(Math.max(min, value) / min) / Math.log(max / min), 0, 1);
    return clamp((value - min) / (max - min), 0, 1);
  }
  function fromUnit(unit, info) {
    if (info.fineCurve
        && typeof window !== 'undefined'
        && window.STFine) { const spec={...info,...info.fineCurve}; return window.STFine.quantize(window.STFine.fromPosition(unit,spec),
        spec); }
    const min = Number(info.min), max = Number(info.max), u = clamp(unit, 0, 1);
    return info.log && min > 0 ? min * Math.pow(max / min, u) : min + u * (max - min);
  }
  /** Lane value at a row: holds before the first / after the last point (engine rule). */
  function valueAt(points, row, interpolation = 'linear', fallback = 0) {
    if (!points?.length) return fallback;
    if (row <= points[0].row) return points[0].value;
    const last = points[points.length - 1];
    if (row >= last.row) return last.value;
    for (let i = 1; i < points.length; i++) {
      const b = points[i];
      if (row < b.row) {
        const a = points[i - 1];
        if (interpolation === 'step' || b.row === a.row) return a.value;
        return a.value + (b.value - a.value) * (row - a.row) / (b.row - a.row);
      }
      if (row === b.row) return b.value;
    }
    return last.value;
  }
  /** Lane fields for a new lane of this parameter, shaped like an existing one when possible. */
  function laneTemplate(p, owner, target, name) {
    for (const pattern of p.patterns) {
      const lane = findLane(p, pattern, owner, target);
      if (lane) { const copy = clone(lane); delete copy.id; delete copy.points; return copy; }
    }
    // Only the engine's own track parameters name their track by index; every other lane names its channel.
    const track = trackIndexOf(p, owner), route = track < 0 || /^(send|plugin):/.test(String(target));
    return { ...(route ? { owner: normalizeOwner(owner, p) } : { track }), target, name: name || target, interpolation: 'linear' };
  }
  function ensureLane(p, pattern, owner, target, { uid, name } = {}) {
    let lane = findLane(p, pattern, owner, target);
    if (lane) return lane;
    lane = { id: uid ? uid() : `lane-${Math.random().toString(36).slice(2)}`, ...laneTemplate(p, owner, target, name), points: [] };
    pattern.automation.push(lane);
    return lane;
  }
  /** Unique, sorted, clamped points inside a pattern of `length` rows. */
  function cleanPoints(points, info, length) {
    const maxRow = Math.max(0, length - Q), unique = new Map();
    for (const point of points) if (finite(point?.row) && finite(point?.value)) unique.set(clamp(quantize(point.row), 0, maxRow),
        clamp(point.value, info?.min ?? -Infinity, info?.max ?? Infinity));
    return [...unique].sort((a, b) => a[0] - b[0]).slice(0,
        MAX_POINTS).map(([row, value]) => ({ row: Number(row.toFixed(6)), value: Number(Number(value).toFixed(6)) }));
  }
  /** Flat sequence slots per pattern id. */
  function occurrences(p) {
    const map = new Map();
    p.sequence.forEach((id, flat) => { const key = Number(id); map.set(key, [...(map.get(key) || []), flat]); });
    return map;
  }
  /** Does this track have any step in the pattern? null for bus/master owners. */
  function trackPlays(p, pattern, owner) {
    const track = trackIndexOf(p, owner);
    if (track < 0) return null;
    return pattern.steps[track].some(Boolean);
  }
  /** Per pattern: whether it has a lane for this parameter, its point count, use and song count. */
  function summary(p, owner, target) {
    const occ = occurrences(p);
    return p.patterns.map((pattern, index) => {
      const lane = findLane(p, pattern, owner, target);
      return { id: pattern.id, number: index + 1, name: pattern.name || `Pattern ${index + 1}`, length: pattern.length, lane: !!lane, points: lane?.points.length || 0, plays: trackPlays(p, pattern, owner), inSong: occ.get(Number(pattern.id))?.length
          || 0 };
    });
  }
  /** Patterns this parameter's channel uses: patterns where the track has steps, or (for buses and
      master, which have no steps) every pattern in the song. */
  function patternsUsing(p, owner) {
    const occ = occurrences(p);
    return p.patterns.filter(pattern => { const plays = trackPlays(p, pattern, owner); return plays === null
        ? occ.has(Number(pattern.id))
        : plays; }).map(pattern => pattern.id);
  }
  /** Copy one pattern's lane to other patterns (rows are time, so they are kept as they are; a
      shorter destination ends with the source's value at its last row). Replaces existing lanes. */
  function copyLane(p, fromId, owner, target, toIds, { uid } = {}) {
    const source = findLane(p, patternById(p, fromId), owner, target);
    if (!source) throw new Error('There is no lane to copy in this pattern.');
    const points = sorted(source.points), interpolation = source.interpolation === 'step' ? 'step' : 'linear';
    let count = 0;
    for (const id of new Set(toIds.map(Number))) {
      if (id === Number(fromId)) continue;
      const pattern = patternById(p, id); if (!pattern) continue;
      const maxRow = Math.max(0, pattern.length - Q), inside = points.filter(point => point.row <= maxRow + 1e-9);
      if (points.length && points[points.length - 1].row > maxRow + 1e-9) inside.push({ row: maxRow, value: valueAt(points, maxRow, interpolation) });
      const lane = ensureLane(p, pattern, owner, target, { uid, name: source.name });
      lane.points = cleanPoints(inside, null, pattern.length);
      lane.interpolation = interpolation;
      if (source.name) lane.name = source.name;
      count++;
    }
    return count;
  }
  /** The song as pattern instances along one global row axis. */
  function timeline(p) {
    const occ = occurrences(p), seen = new Map();
    let start = 0;
    return p.sequence.map((id, flat) => {
      const pattern = patternById(p, id), length = Math.max(0, Number(pattern?.length) || 0), key = Number(id);
      const occurrence = (seen.get(key) || 0) + 1; seen.set(key, occurrence);
      const item = { flat, pattern: key, start, length, occurrence, occurrences: occ.get(key)?.length || 1, missing: !pattern };
      start += length;
      return item;
    });
  }
  const totalRows = list => list.length ? list[list.length - 1].start + list[list.length - 1].length : 0;
  function instanceAt(list, row) {
    for (let i = 0; i < list.length; i++) if (row < list[i].start + list[i].length) return i;
    return Math.max(0, list.length - 1);
  }
  /** One parameter along the song. A pattern without a lane (or with an empty one) leaves the
      parameter where the previous pattern left it, exactly as the engine does; before the first
      lane the base value plays. `live(patternId)` may supply in-progress recorded points. */
  function songLane(p, owner, target, { base = 0, live } = {}) {
    let carried = base;
    return timeline(p).map(inst => {
      const pattern = patternById(p, inst.pattern), lane = findLane(p, pattern, owner, target);
      const points = sorted(live?.(inst.pattern) || lane?.points);
      const incoming = carried;
      if (points.length) carried = points[points.length - 1].value;
      return { ...inst, lane, interpolation: lane?.interpolation === 'step' ? 'step' : 'linear', points, incoming };
    });
  }
  function valueAtSong(song, row) {
    if (!song.length) return 0;
    const inst = song[instanceAt(song, row)];
    return inst.points.length ? valueAt(inst.points, row - inst.start, inst.interpolation) : inst.incoming;
  }
  /** Instances a global row range touches whose pattern also plays elsewhere in the song. */
  function sharedInRange(p, fromRow, toRow) {
    const a = Math.min(fromRow, toRow), b = Math.max(fromRow, toRow), list = timeline(p);
    const groups = new Map();
    for (const inst of list) {
      if (inst.missing || inst.length <= 0 || inst.start + inst.length <= a || inst.start > b || inst.occurrences < 2) continue;
      const group = groups.get(inst.pattern) || {
        pattern: inst.pattern,
        slots: list.filter(item => item.pattern === inst.pattern).map(item => item.flat),
        inRange: []
      };
      group.inRange.push(inst.flat); groups.set(inst.pattern, group);
    }
    return [...groups.values()];
  }
  /** Give sequence slots their own copy of their pattern. A pattern whose every slot is listed
      keeps the original for its first slot. Returns [{flat, from, to}]. */
  function makeUnique(p, flats, { uid } = {}) {
    const wanted = [...new Set(flats.map(Number))].filter(flat => Number.isInteger(flat)
        && flat >= 0
        && flat < p.sequence.length).sort((a, b) => a - b);
    const occ = occurrences(p), changes = [];
    const needed = wanted.filter(flat => {
      const all = occ.get(Number(p.sequence[flat])) || [];
      return all.length > 1 && !(all.every(slot => wanted.includes(slot)) && flat === all[0]);
    }).length;
    const used = new Set(p.patterns.map(pattern => pattern.id));
    const available = Array.from({length:256}, (_, id) => id).filter(id => !used.has(id)).length;
    if (needed > available) throw new Error('This session supports up to 256 patterns. Remove unused patterns before making these slots unique.');
    const SM = typeof window !== 'undefined' && window.STSongModel
        ? window.STSongModel
        : typeof require === 'function'
        ? require('./song-model.js')
        : null;
    for (const flat of wanted) {
      const id = Number(p.sequence[flat]), all = occ.get(id) || [];
      if (all.length < 2) continue;
      if (all.every(slot => wanted.includes(slot)) && flat === all[0]) continue;
      const copy = SM.clonePattern(p, id, uid);
      const source = patternById(p, id), occurrence = all.indexOf(flat) + 1;
      copy.name = `${source?.name || 'Pattern'} · ${occurrence}`;
      p.patterns.push(copy);
      p.sequence[flat] = copy.id;
      changes.push({ flat, from: id, to: copy.id });
    }
    return changes;
  }
  /** Write a straight sweep from a = {row, value} to b (global song rows) into the pattern lanes
      of every instance it crosses. Inside each instance the swept rows are replaced; what plays
      before and after the sweep is kept (a hold before it where the pattern had no lane). Log
      scaled parameters sweep perceptually (one extra point per beat). Shared patterns receive
      every write in song order, so a later repeat overwrites an earlier one where they overlap.
      Returns the touched pattern ids. */
  function applySweep(p, owner, target, a, b, info, { uid, name, linesPerBeat } = {}) {
    if (b.row < a.row) [a, b] = [b, a];
    const song = songLane(p, owner, target, { base: finite(info.value) ? Number(info.value) : a.value });
    const ua = toUnit(a.value, info), ub = toUnit(b.value, info), span = b.row - a.row;
    const at = row => span <= 0 ? b.value : fromUnit(ua + (ub - ua) * clamp((row - a.row) / span, 0, 1), info);
    const beat = Math.max(1, Number(linesPerBeat || p.linesPerBeat) || 4), touched = [];
    for (const inst of song) {
      if (inst.missing || inst.length <= 0 || inst.start + inst.length <= a.row || inst.start > b.row) continue;
      const pattern = patternById(p, inst.pattern), maxRow = Math.max(0, inst.length - Q);
      const la = clamp(quantize(a.row - inst.start), 0, maxRow), lb = clamp(quantize(b.row - inst.start), 0, maxRow);
      const lane = findLane(p, pattern, owner, target),
        existing = sorted(lane?.points),
        interpolation = lane?.interpolation === 'step' ? 'step' : 'linear';
      const next = existing.filter(point => point.row < la - 1e-9);
      if (la > 0) {
        // Keep what plays before the sweep: the old curve, or the value carried into this pattern.
        const prior = existing.length ? valueAt(existing, la - Q, interpolation) : inst.incoming;
        if (!next.length) next.push({ row: 0, value: prior });
        next.push({ row: la - Q, value: prior });
      }
      next.push({ row: la, value: at(inst.start + la) });
      if (info.log) for (let row = Math.ceil((la + Q) / beat) * beat; row < lb; row += beat) next.push({ row, value: at(inst.start + row) });
      if (lb > la) next.push({ row: lb, value: at(inst.start + lb) });
      const after = existing.filter(point => point.row > lb + 1e-9);
      if (after.length && lb + Q <= maxRow) next.push({ row: lb + Q, value: valueAt(existing, lb + Q, interpolation) });
      next.push(...after);
      const target_ = ensureLane(p, pattern, owner, target, { uid, name });
      target_.points = cleanPoints(next, info, inst.length);
      if (!touched.includes(inst.pattern)) touched.push(inst.pattern);
    }
    return touched;
  }

  const model = {
    Q,
    findLane,
    ensureLane,
    laneTemplate,
    cleanPoints,
    occurrences,
    summary,
    patternsUsing,
    copyLane,
    timeline,
    totalRows,
    instanceAt,
    songLane,
    valueAt,
    valueAtSong,
    sharedInRange,
    makeUnique,
    applySweep,
    toUnit,
    fromUnit,
    normalizeOwner
  };

  // ---- Song timeline UI (browser only) ------------------------------------------------
  const S = () => window.ST, state = () => S().state, project = () => state().project;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const view = { flat: -1, point: -1, span: 0, contextRegistered: false, sweep: null };
  const L = 96, R = 876, TOP = 28, BOTTOM = 236;
  const number2 = n => String(n).padStart(2, '0');
  const patternNumber = id => project().patterns.findIndex(item => Number(item.id) === Number(id)) + 1;
  const patternLabel = id => `P${number2(patternNumber(id) || 0)}`;
  const color = id => window.STSongModel?.patternColor?.(project(), id) || '#d6a35c';
  const lpb = () => Number(project().linesPerBeat) || 4;
  /** Run one undoable edit; false when the app refused it (e.g. a frozen track or a busy export). */
  function edit(label, fn) {
    // Validate the complete operation before mutate can create an Undo entry or
    // partially clone several repeated patterns at the session capacity limit.
    try { fn(clone(project())); }
    catch (error) { S().toast(error.message || String(error), 'error'); return false; }
    const undo = state().undo, top = undo?.[undo.length - 1];
    S().mutate(label, fn);
    const after = state().undo;
    return !after || after[after.length - 1] !== top;
  }

  function song(ctx) {
    return songLane(project(), ctx.ref.owner, ctx.ref.target, { base: finite(ctx.info.value) ? Number(ctx.info.value) : 0, live: ctx.livePoints });
  }
  function settleAutomationEdit(ctx) {
    const recorder = window.STAutomationRecord;
    if (!recorder?.hasPendingEdits) return true;
    const row = song(ctx)[view.flat]?.points[view.point]?.row;
    if (recorder.finishPass({render:false}) === false) return false;
    const points = song(ctx)[view.flat]?.points || [];
    if (Number.isFinite(row) && points.length) view.point = points.reduce((best, point, index) => Math.abs(point.row - row) < Math.abs(points[best].row - row)
        ? index
        : best,
        0);
    return true;
  }
  function selectedIndex(list) {
    if (!list.length) return -1;
    if (view.flat < 0 || view.flat >= list.length || list[view.flat].pattern !== Number(project().activePattern)) {
      // Follow the pattern chosen elsewhere: the arranger's slot when it plays it, else its first slot.
      const slot = Number(state().sequence);
      const pick = list[slot]?.pattern === Number(project().activePattern)
          ? slot
          : list.findIndex(item => item.pattern === Number(project().activePattern));
      view.flat = pick >= 0 ? pick : clamp(view.flat, 0, list.length - 1);
    }
    return view.flat;
  }
  function windowRange(list) {
    if (!view.span || view.span >= list.length) return [0, Math.max(0, list.length - 1)];
    const half = Math.floor(view.span / 2), first = clamp(view.flat - half, 0, Math.max(0, list.length - view.span));
    return [first, Math.min(list.length - 1, first + view.span - 1)];
  }
  function geometry(list) {
    const [first, last] = windowRange(list), from = list[first]?.start || 0, to = list[last] ? list[last].start + list[last].length : 1;
    const width = Math.max(1, to - from);
    return { first, last, from, to, x: row => L + (row - from) / width * (R - L), row: x => from + clamp((x - L) / (R - L), 0, 1) * width };
  }

  function envelope(ctx, list, preview) {
    const info = ctx.info, g = geometry(list), sel = list[view.flat], y = value => BOTTOM - toUnit(value, info) * (BOTTOM - TOP);
    let out = `<rect x="${L}" y="${TOP}" width="${R - L}" height="${BOTTOM - TOP}" class="studio-plot-bg"/>`;
    for (let i = g.first; i <= g.last; i++) {
      const inst = list[i], x0 = g.x(inst.start), x1 = g.x(inst.start + inst.length);
      const cls = inst.flat === sel?.flat ? ' is-selected' : sel && inst.pattern === sel.pattern ? ' is-echo' : '';
      out += `<rect x="${x0}" y="${TOP}" width="${Math.max(0, x1 - x0)}" height="${BOTTOM - TOP}" class="studio-song-block${cls}"`
          + ` style="fill:${color(inst.pattern)}" data-song-slot="${inst.flat}"/>`;
      if (i > g.first) out += `<line x1="${x0}" x2="${x0}" y1="${TOP}" y2="${BOTTOM}" class="studio-song-boundary"/>`;
    }
    [0, .25, .5, .75, 1].forEach(unit => {
      const h = BOTTOM - unit * (BOTTOM - TOP);
      out += `<line x1="${L}" x2="${R}" y1="${h}" y2="${h}" class="studio-grid-line"/>`;
    });
    for (let i = g.first; i <= g.last; i++) {
      const inst = list[i], x0 = g.x(inst.start), x1 = g.x(inst.start + inst.length);
      if (!inst.points.length) { out += `<path d="M ${x0} ${y(inst.incoming)} H ${x1}" class="studio-song-hold"/>`; continue; }
      let d = '';
      inst.points.forEach((point, k) => {
        const px = g.x(inst.start + point.row),
          py = y(point.value);
        d += k === 0 ? `M ${x0} ${py} L ${px} ${py}` : inst.interpolation === 'step' ? ` H ${px} V ${py}` : ` L ${px} ${py}`;
      });
      d += ` H ${x1}`;
      out += `<path d="${d}" class="studio-envelope-line${sel && inst.pattern === sel.pattern && inst.flat !== sel.flat ? ' is-echo' : ''}"/>`;
    }
    for (let i = g.first; i <= g.last; i++) {
      const inst = list[i], own = inst.flat === sel?.flat, echo = !own && sel && inst.pattern === sel.pattern;
      inst.points.forEach((point, k) => {
        const selected = own && k === view.point;
        out += `<circle class="studio-envelope-point studio-song-point${selected ? ' is-selected' : ''}${echo ? ' is-echo' : ''}"`
            + ` cx="${g.x(inst.start + point.row)}" cy="${y(point.value)}" r="${selected ? 6 : own ? 4.5 : 3.2}" data-song-point="${inst.flat}:${k}">`
            + `<title>Slot ${number2(inst.flat + 1)} · ${esc(patternLabel(inst.pattern))} row ${Number(point.row.toFixed(3))}: ${esc(info.format(point.value))}</title></circle>`;
      });
    }
    if (preview) out += `<line x1="${g.x(preview.a.row)}" y1="${y(preview.a.value)}" x2="${g.x(preview.b.row)}" y2="${y(preview.b.value)}" class="studio-song-sweep-preview"/>`;
    return out;
  }
  function labels(ctx, list) {
    const g = geometry(list), sel = list[view.flat], pct = x => x / 9;
    let out = '';
    const visible = g.last - g.first + 1, every = Math.max(1, Math.ceil(visible / 16));
    for (let i = g.first; i <= g.last; i++) {
      const inst = list[i];
      if ((i - g.first) % every && inst.flat !== sel?.flat) continue;
      const mid = g.x(inst.start + inst.length / 2);
      out += `<span class="studio-song-label${inst.flat === sel?.flat ? ' is-selected' : ''}" style="left:${pct(mid)}%;top:${14 / 274 * 100}%">${esc(patternLabel(inst.pattern))}${inst.occurrences > 1 ? `<small>${inst.occurrence}/${inst.occurrences}</small>` : ''}</span>`;
      out += `<span class="studio-axis-row" style="left:${pct(g.x(inst.start))}%;top:${256 / 274 * 100}%">${number2(inst.flat + 1)}</span>`;
    }
    for (const unit of [0, .25, .5, .75, 1]) out += `<span class="studio-axis-value" style="right:${(900 - 84) / 9}%;top:${(BOTTOM - unit * 208) / 274 * 100}%">${esc(ctx.info.format(fromUnit(unit, ctx.info)))}</span>`;
    return `<div class="studio-envelope-labels" aria-hidden="true">${out}</div>`;
  }
  function slotName(inst) {
    const pattern = patternById(project(), inst.pattern);
    return `${number2(inst.flat + 1)} · ${patternLabel(inst.pattern)} ${pattern?.name || 'Missing pattern'}${inst.occurrences > 1 ? ` (${inst.occurrence}/${inst.occurrences})` : ''}`;
  }
  function sharedNotice(list) {
    const inst = list[view.flat]; if (!inst || inst.occurrences < 2) return '';
    const slots = list.filter(item => item.pattern === inst.pattern).map(item => number2(item.flat + 1));
    const pattern = patternById(project(), inst.pattern);
    return `<div class="notice studio-song-shared" id="automation-song-shared" role="note"><span><strong>${esc(patternLabel(inst.pattern))} “${esc(pattern?.name || '')}” `
        + `plays ${inst.occurrences}× in this song</strong> (slots ${slots.join(', ')}). Editing it here changes every repeat.</span>`
        + `<button class="btn studio-small" id="automation-song-unique" title="Give this slot its own copy of the pattern · U">Make slot ${number2(inst.flat + 1)}`
        + ` unique</button></div>`;
  }
  function playback(payload, list) {
    const p = payload || state().status || {}, g = geometry(list);
    if (!p.playing) return { visible: false, label: 'Stopped', left: L / 9 };
    let row = null;
    const position = Number.isFinite(p.position) ? p.position : Number(p.row || 0) + clamp(p.fraction ?? p.rowFraction, 0, 1);
    if (p.songMode && Number.isInteger(p.sequence) && list[p.sequence]) row = list[p.sequence].start + clamp(position, 0, list[p.sequence].length);
    else if (Number.isFinite(p.pattern)
        && list[view.flat]?.pattern === p.pattern) row = list[view.flat].start + clamp(position, 0, list[view.flat].length);
    if (row === null) return { visible: false, label: p.songMode ? 'Playing' : 'Playing pattern · not this slot', left: L / 9 };
    const at = list[instanceAt(list, row)];
    return { visible: row >= g.from && row <= g.to, label: `Playing · slot ${number2(at.flat + 1)} · row ${(row - at.start).toFixed(2)}`, left: g.x(row) / 9 };
  }

  function render(ctx) {
    const p = project(), list = song(ctx); selectedIndex(list);
    if (!list.length) return `<section class="panel studio-automation-empty"><h3>The song is empty</h3><p>Add blocks in the Song view to see automation across the song.</p></section>`;
    const inst = list[view.flat], points = inst.points;
    view.point = points.length ? clamp(view.point, 0, points.length - 1) : -1;
    const head = playback(null, list), [first, last] = windowRange(list);
    const sweep = sweepDefaults(ctx, list);
    const slotOptions = selected => list.map(item => `<option value="${item.flat}" ${item.flat === selected ? 'selected' : ''}>${esc(slotName(item))}</option>`).join('');
    const value = v => Number(Number(v).toFixed(4));
    const shared = sweepShared(sweep, list);
    return `<section class="panel studio-lane-panel studio-song-lane" id="automation-song-panel"><div class="studio-lane-header">`
        + `<div><h3>${esc(ctx.ref.name || ctx.info.name)} across the song</h3><div class="studio-lane-source muted">${esc(ctx.channel)} / ${esc(ctx.info.name)}`
        + ` · ${list.length} slot${list.length === 1 ? '' : 's'} · ${totalRows(list)} rows${view.span && view.span < list.length
            ? ` · showing slots ${number2(first + 1)}–${number2(last + 1)}`
            : ''}</div></div>
      <div class="studio-song-zoom" role="group" aria-label="Song timeline zoom"><button class="btn studio-small" id="automation-song-zoom-out"`
          + ` title="Show more slots · −">−</button><button class="btn studio-small" id="automation-song-zoom-in" title="Show fewer slots · +">+</button>`
          + `<button class="btn studio-small" id="automation-song-zoom-all" title="Whole song · 0">All</button></div></div>
      ${sharedNotice(list)}
      <div class="studio-automation-transport" id="automation-song-transport"><output id="automation-song-position" aria-live="off">${esc(head.label)}</output>`
          + `<span id="automation-song-slot">Slot ${esc(slotName(inst))} · ${points.length
              ? `${points.length} point${points.length === 1 ? '' : 's'}`
              : `no lane · holds ${esc(ctx.info.format(inst.incoming))}`}</span></div>
      <div class="studio-envelope-wrap${ctx.isWriting ? ' is-recording' : ''}" id="automation-song-wrap"><svg id="automation-song-envelope"`
          + ` viewBox="0 0 900 274" tabindex="0" role="application" preserveAspectRatio="none" aria-label="${esc(ctx.ref.name || ctx.info.name)} across `
          + `the song. Page Up and Page Down choose a song slot. Option left and right select the previous or next point anywhere in the song. Left `
          + `and right move the selected point by a row; up and down change its value; Shift for larger steps. Enter adds a point, Delete removes it. `
          + `U makes the slot's pattern unique. Plus and minus zoom.">${envelope(ctx, list)}</svg>${labels(ctx, list)}<div id="automation-song-playhead"`
          + ` class="studio-automation-playhead" aria-hidden="true" ${head.visible ? '' : 'hidden'} style="left:${head.left}%;top:${TOP / 274 * 100}%;height:${208 / 274 * 100}%"></div></div>
      <div class="studio-envelope-hint"><span><span class="studio-dot"></span>Each slot plays its pattern's lane · dashed: no lane, the value `
          + `holds</span><span>PageUp/PageDown slot · ⌥← / ⌥→ point · Enter adds · ⇧-drag draws a sweep</span></div>
      <form class="studio-sweep" id="automation-sweep" aria-label="Sweep across slots"><h4>Sweep across slots <span class="muted">writes a straight `
          + `ramp into each slot's pattern lane</span></h4>
        <div class="studio-sweep-grid"><span class="studio-control-label">From</span><select class="field" id="automation-sweep-from-slot"`
            + ` aria-label="Sweep start slot">${slotOptions(sweep.from.flat)}</select><input class="field" type="number" id="automation-sweep-from-row"`
            + ` min="0" step="any" value="${value(sweep.from.row)}" aria-label="Sweep start row in that slot"><input class="field" type="number"`
            + ` id="automation-sweep-from-value" min="${ctx.info.min}" max="${ctx.info.max}" step="${ctx.info.step}" value="${value(sweep.from.value)}"`
            + ` aria-label="Sweep start value">
        <span class="studio-control-label">To</span><select class="field" id="automation-sweep-to-slot" aria-label="Sweep end slot">${slotOptions(sweep.to.flat)}</select>`
            + `<input class="field" type="number" id="automation-sweep-to-row" min="0" step="any" value="${value(sweep.to.row)}" aria-label="Sweep end `
            + `row in that slot"><input class="field" type="number" id="automation-sweep-to-value" min="${ctx.info.min}" max="${ctx.info.max}"`
            + ` step="${ctx.info.step}" value="${value(sweep.to.value)}" aria-label="Sweep end value"></div>
        <div class="studio-sweep-actions"><button class="btn studio-small" type="submit" id="automation-sweep-apply">Apply sweep</button>`
            + `<span class="muted" id="automation-sweep-note">${shared.length ? `Crosses repeated pattern${shared.length === 1 ? '' : 's'} ${shared.map(group => patternLabel(group.pattern)).join(', ')}: you will be `
            + `asked to make them unique.` : 'Rows are inside each slot; values use the parameter’s units.'}</span>`
            + `</div></form></section>`;
  }

  function sweepDefaults(ctx, list) {
    const s = view.sweep;
    if (s && s.project === project() && list[s.from.flat] && list[s.to.flat]) return s;
    const inst = list[view.flat], end = list[Math.min(list.length - 1, view.flat + 1)];
    const songNow = song(ctx), fromValue = valueAtSong(songNow, inst.start);
    view.sweep = {
      project: project(),
      from: { flat: inst.flat, row: 0, value: fromValue },
      to: { flat: end.flat, row: Math.max(0, end.length - 1), value: fromValue === ctx.info.max ? ctx.info.min : ctx.info.max }
    };
    return view.sweep;
  }
  const globalRow = (list, end) => list[end.flat].start + clamp(end.row, 0, Math.max(0, list[end.flat].length - Q));
  function sweepShared(sweep, list) { return sharedInRange(project(), globalRow(list, sweep.from), globalRow(list, sweep.to)); }

  // ---- Editing ---------------------------------------------------------------------
  const focusEnvelope = () => { if (!state().modal) document.getElementById('automation-song-envelope')?.focus({ preventScroll: true }); };
  function selectSlot(ctx, flat, { point = 0, focus = true } = {}) {
    const list = timeline(project()); if (!list.length) return;
    view.flat = clamp(flat, 0, list.length - 1); view.point = point;
    state().sequence = view.flat;
    const id = list[view.flat].pattern;
    if (Number(project().activePattern) !== id) ctx.selectPattern(id);
    else S().refresh();
    if (focus) focusEnvelope();
  }
  function writePoints(ctx, flat, points, label, selectRow) {
    const list = timeline(project()), inst = list[flat]; if (!inst || inst.missing) return;
    const clean = cleanPoints(points, ctx.info, inst.length);
    view.point = selectRow === undefined
        ? clamp(view.point, 0, clean.length - 1)
        : clean.findIndex(point => Math.abs(point.row - quantize(selectRow)) < 1e-9);
    // A key that changes nothing (a point already at the slot's first or last row, a value at its limit) is not an Undo step.
    const stored = findLane(project(), patternById(project(), inst.pattern), ctx.ref.owner, ctx.ref.target);
    if (stored && JSON.stringify(stored.points) === JSON.stringify(clean)) { refresh(ctx); focusEnvelope(); return; }
    S().mutate(label, p => {
      const pattern = patternById(p, inst.pattern); if (!pattern) return;
      ensureLane(p, pattern, ctx.ref.owner, ctx.ref.target, { uid: S().uid, name: ctx.ref.name }).points = clean;
    });
    focusEnvelope();
  }
  function addPoint(ctx, list = song(ctx), atRow) {
    if (!settleAutomationEdit(ctx)) return;
    list = song(ctx);
    const inst = list[view.flat]; if (!inst || inst.missing || !inst.length) return;
    const points = inst.points.map(point => ({ ...point })), occupied = new Set(points.map(point => point.row));
    let row = atRow ?? (view.point >= 0 && points[view.point] ? Math.min(inst.length - 1, points[view.point].row + lpb()) : 0);
    if (atRow === undefined && occupied.has(row)) { row = 0; while (occupied.has(row) && row < inst.length) row++; }
    if (row >= inst.length) { S().toast('Every row of this slot already has a point.'); return; }
    points.push({ row, value: valueAtSong(list, inst.start + row) });
    writePoints(ctx, inst.flat, points, 'Add automation point', row);
  }
  function makeSlotUnique(ctx, flat = view.flat) {
    if (!settleAutomationEdit(ctx)) return false;
    const list = timeline(project()), inst = list[flat];
    if (!inst || inst.occurrences < 2) { S().toast('This slot’s pattern plays only once in the song.'); return false; }
    let changes = [];
    const done = edit('Make song slot unique', p => {
      changes = makeUnique(p, [flat], { uid: S().uid });
      if (changes.length) p.activePattern = changes[0].to;
    });
    if (done && changes.length) { view.flat = flat; S().toast(`Slot ${number2(flat + 1)} now plays its own copy (${patternLabel(changes[0].to)}).`); }
    focusEnvelope();
    return done && changes.length > 0;
  }
  function readSweep(list) {
    const read = (slot, row, val) => ({
      flat: clamp(Number(document.getElementById(slot)?.value), 0, list.length - 1),
      row: Math.max(0, Number(document.getElementById(row)?.value) || 0),
      value: Number(document.getElementById(val)?.value)
    });
    return { project: project(), from: read('automation-sweep-from-slot', 'automation-sweep-from-row',
        'automation-sweep-from-value'), to: read('automation-sweep-to-slot', 'automation-sweep-to-row', 'automation-sweep-to-value') };
  }
  /** Apply a sweep between two global positions; asks first when it crosses repeated patterns. */
  function sweep(ctx, a, b) {
    const info = ctx.info;
    a = { row: Math.max(0, a.row), value: clamp(a.value, info.min, info.max) };
    b = { row: Math.max(0, b.row), value: clamp(b.value, info.min, info.max) };
    const shared = sharedInRange(project(), a.row, b.row);
    const run = unique => {
      if (!settleAutomationEdit(ctx)) return;
      let touched = [];
      const done = edit(unique ? 'Make slots unique and sweep automation' : 'Sweep automation across slots', p => {
        if (unique) {
          const changes = makeUnique(p, shared.flatMap(group => group.inRange), { uid: S().uid });
          const now = changes.find(change => change.flat === view.flat); if (now) p.activePattern = now.to;
        }
        touched = applySweep(p, ctx.ref.owner, ctx.ref.target, a, b, info, { uid: S().uid, name: ctx.ref.name });
      });
      if (done && touched.length) S().toast(`Sweep written into ${touched.length} pattern${touched.length === 1 ? '' : 's'}${unique ? ' (repeated slots made unique)' : ''}. ⌘Z undoes it.`);
      focusEnvelope();
    };
    if (!shared.length) { run(false); return; }
    const lines = shared.map(group => {
      const pattern = patternById(project(), group.pattern);
      return `<li><strong>${esc(patternLabel(group.pattern))} “${esc(pattern?.name || '')}”</strong> plays in slots ${group.slots.map(flat => number2(flat + 1)).join(', ')}; `
          + `the sweep crosses ${group.inRange.map(flat => number2(flat + 1)).join(', ')}.</li>`;
    }).join('');
    S().openModal('Sweep crosses repeated patterns', `<p>Automation is stored in each pattern, so writing into a repeated pattern changes <em>every</em> slot that plays it, and repeats `
        + `inside the sweep overwrite each other.</p><ul class="studio-sweep-shared">${lines}</ul><div class="studio-sweep-choices">`
        + `<button class="btn primary" id="automation-sweep-unique">Make those slots unique, then sweep</button>`
        + `<button class="btn" id="automation-sweep-shared">Sweep into the shared patterns</button><button class="btn" id="automation-sweep-cancel">Cancel</button></div>`, '', {
      kind: 'automation-sweep'
    });
    const choose = unique => { S().closeModal(); if (unique !== null) run(unique); else focusEnvelope(); };
    const on = (id, fn) => { const node = document.getElementById(id); if (node) node.onclick = fn; };
    on('automation-sweep-unique', () => choose(true));
    on('automation-sweep-shared', () => choose(false));
    on('automation-sweep-cancel', () => choose(null));
  }

  function pointer(ctx, svg, event) {
    const box = svg.getBoundingClientRect(), list = song(ctx), g = geometry(list);
    const x = (event.clientX - box.left) / box.width * 900, y = (event.clientY - box.top) / box.height * 274;
    const row = g.row(x), value = fromUnit(clamp((BOTTOM - y) / (BOTTOM - TOP), 0, 1), ctx.info);
    const step = !ctx.info.fineCurve && Number(ctx.info.step) > 0 ? Number(ctx.info.step) : 0;
    return { row, value: clamp(step ? Math.round(value / step) * step : value, ctx.info.min, ctx.info.max), list, g };
  }
  function bind(ctx) {
    registerContextMenu(ctx);
    const svg = document.getElementById('automation-song-envelope');
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    on('automation-song-unique', 'click', () => makeSlotUnique(ctx));
    on('automation-song-zoom-in', 'click', () => zoom(ctx, 1));
    on('automation-song-zoom-out', 'click', () => zoom(ctx, -1));
    on('automation-song-zoom-all', 'click', () => zoom(ctx, 0));
    const form = document.getElementById('automation-sweep');
    form?.addEventListener('submit', event => {
      event.preventDefault();
      const list = timeline(project()), s = readSweep(list);
      if (!finite(s.from.value) || !finite(s.to.value)) { S().toast('Enter a start and end value for the sweep.', 'error'); return; }
      view.sweep = s;
      sweep(ctx, { row: globalRow(list, s.from), value: s.from.value }, { row: globalRow(list, s.to), value: s.to.value });
    });
    form?.addEventListener('change', () => {
      const list = timeline(project());
      view.sweep = readSweep(list);
      const note = document.getElementById('automation-sweep-note');
      const shared = sweepShared(view.sweep, list);
      if (note) note.textContent = shared.length ? `Crosses repeated pattern${shared.length === 1 ? '' : 's'} ${shared.map(group => patternLabel(group.pattern)).join(', ')}: you will `
          + `be asked to make them unique.` : 'Rows are inside each slot; values use the parameter’s units.';
    });
    if (!svg) return;
    svg.addEventListener('keydown', event => keydown(ctx, event));
    svg.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      event.preventDefault(); svg.focus?.();
      const beforeHit = event.target?.dataset?.songPoint?.split(':').map(Number);
      const hitRow = beforeHit ? song(ctx)[beforeHit[0]]?.points[beforeHit[1]]?.row : undefined;
      if (!settleAutomationEdit(ctx)) return;
      const start = pointer(ctx, svg, event), hit = event.target?.dataset?.songPoint;
      const recordingGeneration = window.STAutomationRecord?.editGeneration;
      svg.setPointerCapture?.(event.pointerId);
      const done = () => {
        svg.removeEventListener('pointermove', move);
        svg.removeEventListener('pointerup', up);
        svg.removeEventListener('pointercancel', cancel);
      };
      let move, up;
      const cancel = () => { done(); svg.innerHTML = envelope(ctx, song(ctx)); };
      if (event.shiftKey && !hit) {
        // Shift-drag draws a sweep from the press to the release point.
        let end = start;
        move = e => { end = pointer(ctx, svg, e); svg.innerHTML = envelope(ctx, start.list, { a: start, b: end }); };
        up = e => {
          done();
          end = pointer(ctx, svg, e);
          if (Math.abs(end.row - start.row) < Q) {
            svg.innerHTML = envelope(ctx, song(ctx));
            return;
          }
          sweep(ctx, { row: start.row, value: start.value }, { row: end.row, value: end.value });
        };
      } else {
        const list = start.list;
        let flat, index, isNew = false;
        if (hit) {
          [flat, index] = hit.split(':').map(Number);
          if (Number.isFinite(hitRow)
              && list[flat]?.points.length) index = list[flat].points.reduce((best, point, at) => Math.abs(point.row - hitRow) < Math.abs(list[flat].points[best].row - hitRow)
              ? at
              : best,
              0);
        }
        else { flat = list[instanceAt(list, start.row)].flat; isNew = true; }
        const inst = list[flat]; if (!inst || inst.missing || !inst.length) return;
        if (view.flat !== flat) { view.flat = flat; state().sequence = flat; }
        const points = inst.points.map(point => ({ ...point }));
        if (isNew) {
          points.push({ row: clamp(Math.round(start.row - inst.start), 0, inst.length - 1), value: start.value });
          index = points.length - 1;
        }
        view.point = index;
        let moved = false;
        const place = e => {
          const at = pointer(ctx, svg, e);
          points[index] = { row: clamp(Math.round(at.row - inst.start), 0, inst.length - 1), value: at.value };
        };
        const preview = () => {
          const draft = list.map(item => item.pattern === inst.pattern ? { ...item, points: [...points].sort((a, b) => a.row - b.row) } : item);
          svg.innerHTML = envelope(ctx, draft);
        };
        move = e => { place(e); moved = true; preview(); };
        up = () => {
          done();
          if (!isNew && !moved) { if (Number(project().activePattern) !== inst.pattern) ctx.selectPattern(inst.pattern); else S().refresh(); focusEnvelope(); return; }
          if (window.STAutomationRecord?.hasPendingEdits
              && recordingGeneration !== window.STAutomationRecord.editGeneration) { S().toast('Automation changed while dragging. Try the point edit again.'); svg.innerHTML = envelope(ctx, song(ctx)); return; }
          const point = points[index];
          if (Number(project().activePattern) !== inst.pattern) project().activePattern = inst.pattern;
          writePoints(ctx, flat, points.filter((_, i) => i !== index).concat([point]), isNew ? 'Add automation point' : 'Move automation point',
              point.row);
        };
        preview();
      }
      svg.addEventListener('pointermove', move); svg.addEventListener('pointerup', up); svg.addEventListener('pointercancel', cancel);
    });
  }
  function zoom(ctx, direction) {
    const count = timeline(project()).length;
    if (!direction) view.span = 0;
    else { const current = view.span || count; view.span = clamp(direction > 0 ? Math.max(1, Math.floor(current / 2)) : current * 2, 1,
        count); if (view.span >= count) view.span = 0; }
    S().refresh(); focusEnvelope();
  }
  /** Flat list of every point in the song, for ⌥← / ⌥→ and Home / End. */
  const allPoints = list => list.flatMap(inst => inst.points.map((_, index) => ({ flat: inst.flat, index })));
  function keydown(ctx, event) {
    const keys = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Delete', 'Backspace', 'Insert', 'Enter', 'Home', 'End', 'PageUp', 'PageDown',
        '+', '=', '-', '0', 'u', 'U'];
    if (!keys.includes(event.key) || event.metaKey || event.ctrlKey) return;
    const editing = ['Delete', 'Backspace', 'Insert', 'Enter', 'u', 'U'].includes(event.key)
        || event.key.startsWith('Arrow') && !(event.altKey && ['ArrowLeft', 'ArrowRight'].includes(event.key));
    if (editing && !settleAutomationEdit(ctx)) { event.preventDefault(); event.stopPropagation(); return; }
    const list = song(ctx), inst = list[view.flat]; if (!inst) return;
    event.preventDefault(); event.stopPropagation();
    if (event.key === 'u' || event.key === 'U') { makeSlotUnique(ctx); return; }
    if (event.key === '+' || event.key === '=') { zoom(ctx, 1); return; }
    if (event.key === '-') { zoom(ctx, -1); return; }
    if (event.key === '0') { zoom(ctx, 0); return; }
    if (event.key === 'PageUp' || event.key === 'PageDown') { selectSlot(ctx, view.flat + (event.key === 'PageDown' ? 1 : -1)); return; }
    if (event.key === 'Insert' || event.key === 'Enter') { addPoint(ctx, list); return; }
    const every = allPoints(list), here = every.findIndex(item => item.flat === view.flat && item.index === view.point);
    if (event.key === 'Home' || event.key === 'End' || (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight'))) {
      if (!every.length) { S().toast('No points in this song lane yet. Enter adds one.'); return; }
      let pick;
      if (event.key === 'Home') pick = every[0];
      else if (event.key === 'End') pick = every[every.length - 1];
      else if (here >= 0) pick = every[clamp(here + (event.key === 'ArrowRight' ? 1 : -1), 0, every.length - 1)];
      else pick = event.key === 'ArrowRight'
          ? every.find(item => item.flat >= view.flat) || every[every.length - 1]
          : [...every].reverse().find(item => item.flat <= view.flat) || every[0];
      if (pick.flat !== view.flat) selectSlot(ctx, pick.flat, { point: pick.index });
      else { view.point = pick.index; const svg = document.getElementById('automation-song-envelope'); if (svg) svg.innerHTML = envelope(ctx, list); }
      return;
    }
    const points = inst.points.map(point => ({ ...point }));
    if (view.point < 0 || !points[view.point]) { if (points.length) { view.point = 0; const svg = document.getElementById('automation-song-envelope'); if (svg) svg.innerHTML = envelope(ctx, list); } else S().toast('This slot has no points. Enter adds one.'); return; }
    const point = points[view.point];
    if (event.key === 'Delete'
        || event.key === 'Backspace') { points.splice(view.point, 1); view.point = Math.min(view.point, points.length - 1); writePoints(ctx,
        inst.flat, points, 'Delete automation point'); return; }
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') point.row = clamp(point.row
        + (event.key === 'ArrowRight' ? 1 : -1) * (event.shiftKey ? lpb() : 1),
        0, inst.length - 1);
    else {
      const step = !ctx.info.fineCurve && Number(ctx.info.step) > 0 ? Number(ctx.info.step) : 0,
        raw = fromUnit(toUnit(point.value, ctx.info) + (event.key === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? .1 : .01), ctx.info);
      point.value = clamp(step ? Math.round(raw / step) * step : raw, ctx.info.min, ctx.info.max);
    }
    writePoints(ctx, inst.flat, points.filter((_, i) => i !== view.point).concat([point]), 'Edit automation point', point.row);
  }

  /** Live repaint (recording / transport) without re-rendering the view. */
  function refresh(ctx) {
    const svg = document.getElementById('automation-song-envelope'); if (!svg || !ctx) return;
    svg.innerHTML = envelope(ctx, song(ctx));
    document.getElementById('automation-song-wrap')?.classList?.toggle('is-recording', !!ctx.isWriting);
  }
  function paintPlayhead(payload) {
    const head = document.getElementById('automation-song-playhead'), output = document.getElementById('automation-song-position'); if (!head && !output) return;
    const list = timeline(project()), at = playback(payload, list);
    if (output && output.textContent !== at.label) output.textContent = at.label;
    if (head) { head.hidden = !at.visible; head.style.left = `${at.left}%`; }
  }
  function command(name, ctx) {
    if (name === 'unique') return makeSlotUnique(ctx);
    if (name === 'sweep') { const field = document.getElementById('automation-sweep-from-slot'); field?.focus(); field?.scrollIntoView?.({ block: 'nearest' }); return !!field; }
    return false;
  }
  /** Context menu for the song timeline (right-click, ⇧F10). */
  function registerContextMenu(ctx) {
    view.ctx = ctx;
    if (view.contextRegistered || !window.STContextMenu?.register) return;
    view.contextRegistered = true;
    window.STContextMenu.register(context => {
      const c = view.ctx, svg = context?.target?.closest?.('#automation-song-envelope');
      if (!svg || !c || state().view !== 'automation') return null;
      const list = song(c);
      let flat = view.flat, at = null;
      const hit = context.target?.dataset?.songPoint;
      if (hit) [flat] = hit.split(':').map(Number);
      else if (!context.keyboard && Number.isFinite(context.x)) { at = pointer(c, svg,
          { clientX: context.x, clientY: context.y }); flat = list[instanceAt(list, at.row)].flat; }
      const inst = list[flat]; if (!inst) return null;
      return [
        { header: true, title: `Slot ${slotName(inst)}` },
        { title: 'Add point here', key: '↩', enabled: !inst.missing, action: () => { view.flat = flat; addPoint(c, song(c),
            at ? clamp(Math.round(at.row - inst.start), 0, inst.length - 1) : undefined); } },
        hit
            ? { title: 'Delete point', key: '⌫', action: () => { const row = inst.points[Number(hit.split(':')[1])]?.row; if (!settleAutomationEdit(c)) return; const current = song(c)[flat]; if (!current) return; view.flat = flat; writePoints(c,
                flat, current.points.filter(point => point.row !== row), 'Delete automation point'); } }
            : null,
        { title: `Make slot ${number2(flat + 1)} unique`, key: 'U', enabled: inst.occurrences > 1, action: () => makeSlotUnique(c, flat) },
        { title: 'Sweep from here to the next slot…', action: () => { view.sweep = { project: project(), from: { flat, row: at
            ? Math.round(at.row - inst.start)
            : 0, value: valueAtSong(list, at?.row ?? inst.start) }, to: { flat: Math.min(list.length - 1, flat + 1), row: Math.max(0,
            list[Math.min(list.length - 1, flat + 1)].length - 1), value: c.info.max } }; S().refresh(); command('sweep', c); } },
        { separator: true },
        { title: 'Edit this pattern’s lane', key: '\\', action: () => c.showPattern(inst.pattern) }
      ];
    }, 70);
  }
  function enter() { view.flat = -1; view.point = 0; view.sweep = null; }

  const api = { ...model, render, bind, refresh, paintPlayhead, command, enter, makeSlotUnique, get selectedSlot() { return view.flat; } };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STAutomationSong = api;
})();
