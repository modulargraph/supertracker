/* Live automation recording (Read / Touch / Latch / Write).
   While the transport plays, moving an automatable control ([data-automate]) — keyboard
   arrows, pointer drags, a learned MIDI CC, or an NKS / plug-in 8-knob page — writes into
   that parameter's pattern lane at the playhead instead of changing its base value.
   The project is not touched during a pass: the engine hears a draft project with the
   working lanes, and stopping (or leaving the write mode) commits every lane written in
   the pass as ONE Undo step. Recorded runs are thinned; Touch returns to the previous
   lane (or the base value) when the control is released. */
(() => {
  'use strict';
  const S = () => window.ST;
  const MODES = ['read', 'touch', 'latch', 'write'];
  const LABELS = { read: 'Read', touch: 'Touch', latch: 'Latch', write: 'Write' };
  const HINTS = {
    read: 'Lanes play back; moving a control edits its base value.',
    touch: 'While playing, moving a control writes automation until you let go, then the lane resumes.',
    latch: 'While playing, moving a control writes automation and keeps its last value until playback stops.',
    write: 'While playing, armed parameters are overwritten from the start; touched controls latch until stop.'
  };
  const Q = 1 / 64;              // stored row quantum
  const SAMPLE = 1 / 16;         // minimum spacing between recorded samples, rows
  const RETURN = 1 / 4;          // Touch return ramp, rows
  const TOLERANCE = .004;        // thinning error, unit range
  const MAX_POINTS = 4000;       // engine lane limit is 4096
  const KEY_IDLE_MS = 650, MIDI_IDLE_MS = 450, TICK_MS = 40, PREVIEW_MS = 110, LOOKAHEAD_S = .3;
  const STORE = 'supertracker.automationRecord.v1';
  const finite = value => value !== '' && value !== null && value !== undefined && Number.isFinite(Number(value));
  const clamp = (value, min, max) => Math.max(min, Math.min(max, Number(value) || 0));
  const quantize = row => Math.round(row / Q) * Q;
  const copyPoints = points => (points || []).filter(p => finite(p?.row) && finite(p?.value)).map(p => ({
    row: Number(p.row),
    value: Number(p.value)
  })).sort((a, b) => a.row - b.row);

  // ---- Pure helpers (also exercised by tests) --------------------------------------
  function toUnit(value, info) {
    const min = Number(info.min), max = Number(info.max);
    if (!(max > min)) return 0;
    if (info.log && min > 0) return Math.log(Math.max(min, value) / min) / Math.log(max / min);
    return (value - min) / (max - min);
  }
  function fromUnit(unit, info, curve) {
    const min = Number(info.min), max = Number(info.max), u = clamp(unit, 0, 1);
    // A perceptual curve (fine-control.js) spreads controller travel like the on-screen slider.
    if (curve && window.STFine) return window.STFine.quantize(window.STFine.fromPosition(u, { min, max, knee: curve.knee }),
        { min, max, floor: curve.floor });
    const raw = info.log && min > 0 ? min * Math.pow(max / min, u) : min + u * (max - min);
    const step = Number(info.step);
    return clamp(step > 0 ? Math.round(raw / step) * step : raw, min, max);
  }
  /** Lane value at a row: holds before the first / after the last point. */
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
  /** Douglas–Peucker on bounded spans of vertical unit error. Only flagged recorded
      runs are thinned; original points, anchors and span endpoints remain fixed.
      Bounding each span prevents dense alternating takes from doing quadratic work
      over the whole lane. Each retained segment still meets the requested tolerance. */
  function thin(points, info, interpolation = 'linear', tolerance = TOLERANCE) {
    const out = [];
    let run = [];
    const flush = () => {
      if (run.length <= 2) { out.push(...run); run = []; return; }
      if (interpolation === 'step') {
        let previous = null;
        run.forEach((point, i) => {
          const keep = i === 0
              || i === run.length - 1
              || point.anchor
              || previous === null
              || Math.abs(toUnit(point.value, info) - toUnit(previous.value, info)) > tolerance;
          if (keep) { out.push(point); previous = point; }
        });
        run = []; return;
      }
      const keep = new Array(run.length).fill(false); keep[0] = keep[run.length - 1] = true;
      run.forEach((point, i) => { if (point.anchor) keep[i] = true; });
      const unit = run.map(point => toUnit(point.value, info));
      const stack = [];
      for (let i = 128; i < run.length - 1; i += 128) keep[i] = true;
      for (let i = 0, start = 0; i < run.length; i++) if (keep[i] && i > start) { stack.push([start, i]); start = i; }
      while (stack.length) {
        const [a, b] = stack.pop(); if (b - a < 2) continue;
        let worst = -1, error = tolerance;
        const slope = (unit[b] - unit[a]) / Math.max(1e-9, run[b].row - run[a].row), midpoint = (a + b) / 2;
        for (let k = a + 1; k < b; k++) {
          const e = Math.abs(unit[k] - (unit[a] + slope * (run[k].row - run[a].row)));
          if (e > error || (worst >= 0 && e === error && Math.abs(k - midpoint) < Math.abs(worst - midpoint))) { error = e; worst = k; }
        }
        if (worst >= 0) { keep[worst] = true; stack.push([a, worst], [worst, b]); }
      }
      run.forEach((point, i) => { if (keep[i]) out.push(point); });
      run = [];
    };
    for (const point of points) {
      if (point.rec || point.anchor) run.push(point);
      else { flush(); out.push(point); }
    }
    flush();
    return out;
  }
  /** A hard point budget cannot retain every oscillation. Keep the temporal bounds
      and, when they fit, every original/anchor point; spend the remaining budget on
      chronological min/max pairs across the full recorded span. This is linear and
      retains late peaks and returns instead of silently chopping off the tail. */
  function limitPoints(points, limit = MAX_POINTS) {
    if (points.length <= limit) return points;
    const kept = new Set([0, points.length - 1]);
    for (let i = 1; i < points.length - 1; i++) if (points[i].anchor || !points[i].rec) kept.add(i);
    if (kept.size > limit) { kept.clear(); kept.add(0); kept.add(points.length - 1); }
    const candidates = [];
    for (let i = 1; i < points.length - 1; i++) if (!kept.has(i)) candidates.push(i);
    const room = limit - kept.size, buckets = Math.floor(room / 2);
    for (let bucket = 0; bucket < buckets; bucket++) {
      const from = Math.floor(bucket * candidates.length / buckets), to = Math.floor((bucket + 1) * candidates.length / buckets);
      let low = candidates[from], high = low;
      for (let i = from + 1; i < to; i++) {
        const index = candidates[i];
        if (points[index].value < points[low].value) low = index;
        if (points[index].value >= points[high].value) high = index;
      }
      kept.add(low); kept.add(high);
    }
    if (room % 2) {
      // One remaining place goes to the strongest departure from the endpoint line.
      const first = points[0], last = points[points.length - 1], span = Math.max(1e-9, last.row - first.row);
      let best = -1, error = -1;
      for (const i of candidates) if (!kept.has(i)) {
        const distance = Math.abs(points[i].value - (first.value + (last.value - first.value) * (points[i].row - first.row) / span));
        if (distance > error) { best = i; error = distance; }
      }
      if (best >= 0) kept.add(best);
    }
    return points.filter((_, index) => kept.has(index));
  }
  /** Unique, sorted, clamped plain points within the engine limit. */
  function finalize(points, info, interpolation, maxRow, summary) {
    const result = thin(points, info, interpolation);
    const unique = new Map();
    for (const point of result) unique.set(clamp(quantize(point.row), 0, maxRow), { ...point, value: clamp(point.value, info.min, info.max) });
    const ordered = [...unique].sort((a, b) => a[0] - b[0]).map(([row, point]) => ({ ...point, row }));
    if (summary) summary.limited = ordered.length > MAX_POINTS;
    return limitPoints(ordered).map(({row, value}) => ({ row: Number(row.toFixed(6)), value: Number(Number(value).toFixed(6)) }));
  }

  // Lane-level writing. `lane` = { work:[{row,value,rec?,anchor?,hold?}], interpolation, base, maxRow, seg }.
  const removeWhere = (work, test) => { for (let i = work.length - 1; i >= 0; i--) if (test(work[i].row)) work.splice(i, 1); };
  const insert = (work, point) => { let i = work.length; while (i > 0 && work[i - 1].row > point.row) i--; work.splice(i, 0, point); return point; };
  function startSegment(lane, row, value) {
    const r = clamp(quantize(row), 0, lane.maxRow);
    const reference = lane.work.map(point => ({ ...point }));
    const anchorRow = quantize(r - SAMPLE);
    removeWhere(lane.work, at => at >= anchorRow - 1e-9 && at <= r + 1e-9);
    if (anchorRow >= 0 && (reference.length || finite(lane.base))) insert(lane.work, {
      row: anchorRow,
      value: valueAt(reference, anchorRow, lane.interpolation, lane.base),
      anchor: true
    });
    const point = insert(lane.work, { row: r, value, rec: true });
    lane.seg = { reference, start: r, last: r, value, tail: point };
    return lane.seg;
  }
  function sample(lane, row, value) {
    const seg = lane.seg; if (!seg) return startSegment(lane, row, value);
    let r = clamp(quantize(row), 0, lane.maxRow);
    if (r < seg.last) r = seg.last;
    removeWhere(lane.work, at => at > seg.last + 1e-9 && at <= r + 1e-9);
    const tail = seg.tail;
    if (value === seg.value) {
      if (r > seg.last) {
        if (tail?.hold) tail.row = r;
        else seg.tail = insert(lane.work, { row: r, value, rec: true, hold: true });
      }
    } else if (r - seg.last < SAMPLE - 1e-9) {
      if (tail && !tail.hold && !tail.anchor) tail.value = value;
      else {
        r = Math.min(lane.maxRow, quantize(seg.last + SAMPLE));
        removeWhere(lane.work, at => at > seg.last + 1e-9 && at <= r + 1e-9);
        seg.tail = insert(lane.work, { row: r, value, rec: true });
      }
    } else {
      // A step after a hold: keep the old value until just before the new one.
      if (tail?.hold && r - SAMPLE > tail.row + 1e-9) tail.row = quantize(r - SAMPLE);
      seg.tail = insert(lane.work, { row: r, value, rec: true });
    }
    seg.last = Math.max(seg.last, r); seg.value = value;
    return seg;
  }
  function endSegment(lane, row, returnToLane) {
    const seg = lane.seg; if (!seg) return;
    if (Number.isFinite(row)) sample(lane, row, seg.value);
    if (returnToLane) {
      const back = quantize(seg.last + RETURN);
      if (back <= lane.maxRow) {
        removeWhere(lane.work, at => at > seg.last + 1e-9 && at <= back + 1e-9);
        insert(lane.work, { row: back, value: valueAt(seg.reference, back, lane.interpolation, lane.base), anchor: true });
      }
    }
    lane.seg = null;
  }
  /** Plain points the engine should hear now: a guard holds the live value a little
      ahead of the playhead so stale points ahead cannot pull the value away. */
  function engineView(lane, lookahead) {
    const points = lane.work.map(point => ({ row: point.row, value: point.value }));
    const seg = lane.seg;
    if (seg && lookahead > 0) {
      const guard = Math.min(lane.maxRow, quantize(seg.last + lookahead));
      if (guard > seg.last) {
        for (let i = points.length - 1; i >= 0; i--) if (points[i].row > seg.last + 1e-9 && points[i].row <= guard + 1e-9) points.splice(i, 1);
        let i = points.length; while (i > 0 && points[i - 1].row > guard) i--;
        points.splice(i, 0, { row: guard, value: seg.value });
      }
    }
    return points;
  }

  // ---- Session state ----------------------------------------------------------------
  const rec = {
    generation: 0,
    mode: 'read',
    lastWrite: 'touch',
    armed: new Set(),
    gestures: new Map(),
    pass: null,
    learn: null,
    midi: new Map(),
    midiChange: new Map(), // parameter → pending commit of a controller move outside recording
    tick: null,
    previewTimer: null,
    previewBusy: false,
    previewDirty: false,
    paintTimer: null,
    wasPlaying: false,
    pointer: null,
    warned: new Set(),
    recent: new Map()
  };
  function load() {
    try {
      const saved = JSON.parse(window.localStorage?.getItem(STORE) || 'null');
      if (saved && MODES.includes(saved.mode)) rec.mode = saved.mode;
      if (saved && MODES.includes(saved.lastWrite) && saved.lastWrite !== 'read') rec.lastWrite = saved.lastWrite;
      for (const [key, value] of Object.entries(saved?.midi || {})) if (value?.target && value?.owner) rec.midi.set(key, value);
    } catch {}
    rec.mode = 'read'; // Arming is a per-session decision: always start in Read.
  }
  function save() {
    try {
      window.localStorage?.setItem(STORE, JSON.stringify({ mode: rec.mode, lastWrite: rec.lastWrite, midi: Object.fromEntries(rec.midi) }));
    } catch {}
  }

  const state = () => S()?.state;
  const project = () => state()?.project;
  const patternById = (id, p = project()) => p?.patterns?.find(item => Number(item.id) === Number(id));
  const trackOwner = (index, p = project()) => `track:${p?.tracks?.[index]?.routingID ?? index}`;
  const normalizeOwner = (owner, p = project()) => typeof owner === 'number' || /^\d+$/.test(String(owner))
      ? trackOwner(Number(owner), p)
      : String(owner);
  const laneOwner = lane => normalizeOwner(lane.owner ?? lane.track);
  const refKey = ref => `${ref.owner}|${ref.target}`;
  function makeRef(owner, target, name) {
    const p = project(); if (!p || !target) return null;
    const normalized = normalizeOwner(owner ?? state().track, p);
    const track = normalized.startsWith('track:') ? p.tracks.findIndex((_, i) => trackOwner(i, p) === normalized) : -1;
    return { owner: normalized, track, target: String(target), name: name || String(target) };
  }
  function refFor(el) {
    if (!el?.dataset?.automate) return null;
    const ref = makeRef(el.dataset.owner ?? Number(el.dataset.track ?? state().track), el.dataset.automate,
        el.dataset.name || el.getAttribute?.('aria-label'));
    if (ref) ref.element = el;
    return ref;
  }
  function descriptor(ref) {
    try { return window.STStudio?.target?.(ref.owner, ref.target) || null; } catch { return null; }
  }
  /** Why this control cannot record now, or '' if it can. */
  function blocked(ref) {
    const p = project(), st = state();
    if (!p || !ref) return 'No project';
    if (!ref.owner.startsWith('track:') && ref.owner !== 'master' && !ref.owner.startsWith('bus:')) return 'Unknown channel';
    if (ref.owner.startsWith('track:') && ref.track < 0) return 'The channel for this parameter is no longer available.';
    if (ref.track >= 0
        && st.view === 'instrument'
        && !ref.target.startsWith('plugin:')
        && p.tracks[ref.track]?.instrument !== st.instrument) return 'Assign this instrument to the selected track before recording its automation.';
    const info = descriptor(ref);
    if (!info) return 'This parameter does not support a tracker automation lane.';
    if (info.available === false) return info.unavailableReason || 'This parameter is unavailable.';
    return '';
  }
  const playing = () => {
    const st = state();
    return !!(st?.playing && st.status?.playing && Number.isFinite(st.status.pattern) && patternById(st.status.pattern));
  };
  const inScope = ref => !rec.armed.size || rec.armed.has(refKey(ref)) || rec.armed.has(`${ref.owner}|*`);
  function wouldRecord(ref) { return rec.mode !== 'read' && playing() && !!ref && inScope(ref) && !blocked(ref); }

  function rowsPerSecond() {
    const st = state(), status = st?.status || {}, p = project();
    const bpm = Number(status.bpm) > 0 ? Number(status.bpm) : Number(p?.bpm) > 0 ? Number(p.bpm) : 120;
    const lines = Number(p?.linesPerBeat) > 0 ? Number(p.linesPerBeat) : 4;
    return bpm * lines / 60;
  }
  /** Current playhead: the last status position extrapolated by at most 150 ms. */
  function playhead() {
    const st = state(), status = st?.status || {};
    const pattern = patternById(status.pattern); if (!pattern) return null;
    const base = Number.isFinite(status.position) ? status.position : Number(status.row || 0) + clamp(status.fraction ?? status.rowFraction, 0, 1);
    const elapsed = Number.isFinite(st.lastStatusAt) ? clamp((Date.now() - st.lastStatusAt) / 1000, 0, .15) : 0;
    return { pattern: Number(pattern.id), row: clamp(base + elapsed * rowsPerSecond(), 0, Math.max(0, pattern.length - Q)), length: pattern.length };
  }

  // ---- Pass management --------------------------------------------------------------
  function ensurePass() {
    if (rec.pass && rec.pass.project === project()) return rec.pass;
    if (rec.pass) abortPass('The project changed; the automation pass was discarded.');
    rec.pass = { project: project(), lanes: new Map(), started: Date.now() };
    startTicking(); paintStatus();
    return rec.pass;
  }
  function laneFor(ref, patternId) {
    const pass = ensurePass(), key = `${patternId}|${refKey(ref)}`;
    let lane = pass.lanes.get(key);
    if (lane) return lane;
    const pattern = patternById(patternId), info = descriptor(ref) || { min: 0, max: 1, step: .001, value: 0 };
    const existing = pattern?.automation?.find(item => item.target === ref.target && laneOwner(item) === ref.owner);
    const original = copyPoints(existing?.points);
    lane = { key, patternObject: pattern, patternId: Number(patternId), ref: { owner: ref.owner, track: ref.track, target: ref.target, name: ref.name }, info, existed: !!existing,
      interpolation: existing?.interpolation === 'step' ? 'step' : 'linear', base: finite(info.value) ? Number(info.value) : original[0]?.value ?? 0,
      maxRow: Math.max(0, Number(pattern?.length || 1) - Q), work: original.map(point => ({ ...point })), seg: null, written: false };
    pass.lanes.set(key, lane);
    return lane;
  }
  const activeLanes = () => rec.pass ? [...rec.pass.lanes.values()].filter(lane => lane.seg) : [];
  function write(ref, value) {
    const head = playhead(); if (!head) return false;
    const lane = laneFor(ref, head.pattern);
    const info = lane.info, v = clamp(value, info.min ?? -Infinity, info.max ?? Infinity);
    if (lane.seg) sample(lane, head.row, v); else startSegment(lane, head.row, v);
    lane.written = true; rec.generation++;
    schedulePreview(); schedulePaint();
    return true;
  }
  function latched(lane) { return rec.mode === 'latch' || rec.mode === 'write' || !!rec.gestures.get(refKey(lane.ref))?.active; }
  /** Advance every writing lane to the playhead; handle loop wraps and pattern changes. */
  function advance(seeked = false) {
    const pass = rec.pass; if (!pass) return;
    if (pass.project !== project()) { abortPass('The project changed; the automation pass was discarded.'); return; }
    if (!playing()) { finishPass(); return; }
    const head = playhead(); if (!head) return;
    for (const lane of activeLanes()) {
      rec.generation++;
      const seg = lane.seg, keep = latched(lane);
      if (seeked || lane.patternId !== head.pattern || head.row < seg.last - .5) {
        // Pattern changed (Song mode) or the loop wrapped: close this segment where it
        // stopped and continue in the lane now under the playhead.
        const value = seg.value;
        endSegment(lane, null, seeked);
        if (keep) {
          const next = laneFor(lane.ref, head.pattern);
          next.written = true;
          if (next.seg) {
            sample(next, head.row, value);
          } else {
            startSegment(next, head.row, value);
          }
        }
        continue;
      }
      if (!keep && rec.mode === 'touch') { endSegment(lane, head.row, true); continue; }
      sample(lane, head.row, seg.value);
    }
    schedulePreview(); schedulePaint();
  }
  function startTicking() {
    if (rec.tick) return;
    const loop = () => { rec.tick = null; if (!rec.pass) return; advance(); if (rec.pass) rec.tick = setTimeout(loop, TICK_MS); };
    rec.tick = setTimeout(loop, TICK_MS);
  }
  function resetPass() {
    clearTimeout(rec.tick); rec.tick = null; clearTimeout(rec.previewTimer); rec.previewTimer = null; rec.previewDirty = false;
    rec.pass = null; paintStatus(); schedulePaint();
  }
  function abortPass(message) {
    resetPass();
    if (message) S()?.toast?.(message, 'error');
  }
  function laneDestination(lane, p = project()) {
    const source = project();
    if (patternById(lane.patternId, source) !== lane.patternObject) return null;
    const owner = lane.ref.owner;
    const track = owner.startsWith('track:') ? (p?.tracks || []).findIndex((_, index) => trackOwner(index, p) === owner) : -1;
    if (owner.startsWith('track:') && track < 0) return null;
    if (owner.startsWith('bus:') && !(p?.routing.buses || []).some(bus => `bus:${bus.id}` === owner)) return null;
    return {track, pattern:patternById(lane.patternId, p)};
  }
  /** Close all segments and commit the pass as one Undo step. */
  function finishPass(options = {}) {
    const pass = rec.pass; if (!pass) return true;
    if (pass.project !== project()) { abortPass('The project changed; the automation pass was not applied.'); return false; }
    const head = playing() ? playhead() : null;
    for (const lane of pass.lanes.values()) if (lane.seg) endSegment(lane, head && head.pattern === lane.patternId ? head.row : null, true);
    const allWritten = [...pass.lanes.values()].filter(lane => lane.written);
    const written = allWritten.filter(lane => laneDestination(lane));
    if (written.length < allWritten.length) S().toast?.('Automation for a removed pattern or channel was discarded; other recorded lanes are retained.');
    if (!written.length) { resetPass(); return true; }
    const final = written.map(lane => {
      const summary = {};
      return { lane, summary, points: finalize(lane.work, lane.info, lane.interpolation, lane.maxRow, summary) };
    });
    const names = [...new Set(written.map(lane => lane.ref.name))];
    const label = `Record automation · ${names.length === 1 ? names[0] : `${names.length} parameters`}`;
    const apply = p => {
      let committed = 0;
      for (const { lane, points } of final) {
        const destination = laneDestination(lane, p), pattern = destination?.pattern; if (!pattern) continue;
        pattern.automation = pattern.automation || [];
        let target = pattern.automation.find(item => item.target === lane.ref.target
            && normalizeOwner(item.owner ?? item.track, p) === lane.ref.owner);
        if (!target) {
          const routeLane = destination.track < 0 || lane.ref.target.startsWith('send:');
          target = {
            id: S().uid ? S().uid() : `lane-${Date.now().toString(36)}-${committed}`,
            ...(routeLane ? { owner: lane.ref.owner } : { track: destination.track }),
            target: lane.ref.target,
            name: lane.ref.name,
            interpolation: 'linear',
            points: []
          };
          pattern.automation.push(target);
        }
        target.points = points; committed++;
      }
      return committed;
    };
    const candidate = window.STUtil.clone(pass.project);
    apply(candidate);
    const gate = window.STBusy.isBusy('export') && !(options.nativeCapture && options.render === false)
        ? 'Finish or cancel the export first.'
        : window.STBusy.isBusy('routing')
        ? 'Wait for the routing change to finish.'
        : window.STFreeze?.checkMutation?.(pass.project, candidate, label)?.message;
    const refuse = message => {
      clearTimeout(rec.tick); rec.tick = null; clearTimeout(rec.previewTimer); rec.previewTimer = null; rec.previewDirty = false;
      if (pass.commitError !== message) S().toast?.(`Automation take is still pending. ${message}`, 'error');
      pass.commitError = message; paintStatus(); return false;
    };
    if (gate) return refuse(gate);
    let committed = 0;
    S().mutate(label, p => { if (p === pass.project) committed = apply(p); }, options);
    if (!committed || project() !== pass.project) return refuse('The edit was refused. Retry committing the take before leaving this session.');
    resetPass();
    // Automation lives in pattern lanes (docs/AUTOMATION.md): a pattern that plays several times in
    // the song shares one lane, so the last pass over it wins and every repeat plays the result.
    const repeated = [...new Set(final.map(item => item.lane.patternId))].filter(id => (project()?.sequence || []).filter(slot => Number(slot) === id).length > 1);
    const shared = repeated.length ? ` ${repeated.map(id => patternById(id)?.name || `Pattern ${id + 1}`).join(', ')} repeat${repeated.length === 1 ? 's' : ''} in the song, `
        + `so every repeat plays this take.` : '';
    const limited = final.some(item => item.summary.limited)
        ? ' Dense lanes were reduced to the point limit, keeping their start, end and peaks.'
        : '';
    if (committed) S().toast?.(`${label} · ${final.reduce((sum, item) => sum + item.points.length, 0)} points.${shared}${limited} ⌘Z undoes the pass.`);
    return committed > 0;
  }

  // ---- Engine preview and lane painting ---------------------------------------------
  function draftProject() {
    const pass = rec.pass, p = project();
    if (!pass || pass.project !== p) return null;
    const draft = window.STUtil.clone(p), look = rowsPerSecond() * LOOKAHEAD_S;
    for (const lane of pass.lanes.values()) {
      if (!lane.written) continue;
      const destination = laneDestination(lane, draft), pattern = destination?.pattern; if (!pattern) continue;
      pattern.automation = pattern.automation || [];
      let target = pattern.automation.find(item => item.target === lane.ref.target
          && normalizeOwner(item.owner ?? item.track, draft) === lane.ref.owner);
      if (!target) {
        const routeLane = destination.track < 0 || lane.ref.target.startsWith('send:');
        target = {
          id: `recording-${lane.key}`,
          ...(routeLane ? { owner: lane.ref.owner } : { track: destination.track }),
          target: lane.ref.target,
          name: lane.ref.name,
          interpolation: 'linear'
        };
        pattern.automation.push(target);
      }
      const unique = new Map();
      const source = new Map(lane.work.map(point => [point.row, point]));
      const held = lane.seg ? new Set([lane.seg.last, Math.min(lane.maxRow, quantize(lane.seg.last + look))]) : new Set();
      for (const point of engineView(lane, look)) unique.set(point.row, {
        ...source.get(point.row),
        ...point,
        anchor: !!source.get(point.row)?.anchor || held.has(point.row)
      });
      target.points = limitPoints([...unique.values()]).map(({row,value}) => ({row,value}));
    }
    return draft;
  }
  function schedulePreview() {
    if (rec.previewBusy) { rec.previewDirty = true; return; }
    if (rec.previewTimer) return;
    rec.previewTimer = setTimeout(() => {
      rec.previewTimer = null;
      const app = S(); if (!app?.syncEngineProject || !rec.pass) return;
      rec.previewBusy = true; rec.previewDirty = false;
      let job; try { job = app.syncEngineProject(draftProject); } catch (error) { job = Promise.reject(error); }
      Promise.resolve(job).catch(() => {}).finally(() => {
        rec.previewBusy = false;
        if (rec.previewDirty && rec.pass) schedulePreview();
      });
    }, PREVIEW_MS);
  }
  function schedulePaint() {
    if (rec.paintTimer) return;
    rec.paintTimer = setTimeout(() => { rec.paintTimer = null; try { window.STStudio?.refreshLiveAutomation?.(); } catch {} paintStatus(); }, 90);
  }
  /** Working points for a lane being recorded, for the Automation view. */
  function livePoints(patternId, owner, target) {
    const pass = rec.pass; if (!pass) return null;
    const lane = pass.lanes.get(`${Number(patternId)}|${normalizeOwner(owner)}|${target}`);
    return lane?.written ? lane.work.map(point => ({ row: point.row, value: point.value })) : null;
  }
  function isWriting(owner, target) { return activeLanes().some(lane => lane.ref.owner === normalizeOwner(owner) && lane.ref.target === target); }

  // ---- Gestures -----------------------------------------------------------------------
  /** Begin a control gesture. Returns true when this module records it (the caller must
      then leave the base value alone). */
  function begin(ref, value, source = 'api') {
    if (!ref) return false;
    const key = refKey(ref), gesture = rec.gestures.get(key) || { ref, value: undefined, active: false, claimed: false, timer: null, source };
    gesture.ref = ref; gesture.source = source; gesture.active = true; clearTimeout(gesture.timer);
    if (finite(value)) gesture.value = Number(value);
    rec.gestures.set(key, gesture);
    if (!wouldRecord(ref)) {
      if (rec.mode !== 'read' && playing() && inScope(ref)) {
        const why = blocked(ref);
        if (why && !rec.warned.has(key)) {
          rec.warned.add(key);
          S()?.toast?.(why);
        }
      }
      return gesture.claimed;
    }
    gesture.claimed = true;
    if (finite(gesture.value)) write(ref, gesture.value);
    paintStatus();
    return true;
  }
  function value(ref, next, source = 'api') {
    if (!ref || !finite(next)) return false;
    const gesture = rec.gestures.get(refKey(ref));
    if (!gesture?.active) return begin(ref, next, source);
    gesture.value = Number(next);
    if (!wouldRecord(ref)) return gesture.claimed;
    gesture.claimed = true;
    return write(ref, gesture.value);
  }
  function end(ref) {
    if (!ref) return false;
    const key = refKey(ref), gesture = rec.gestures.get(key); if (!gesture) return false;
    clearTimeout(gesture.timer);
    const claimed = gesture.claimed;
    gesture.active = false; rec.gestures.delete(key);
    if (claimed) rec.generation++;
    if (claimed) rec.recent.set(key, { at: Date.now(), value: gesture.value });
    if (rec.mode === 'touch' && rec.pass) {
      const head = playhead();
      for (const lane of activeLanes()) if (refKey(lane.ref) === key) endSegment(lane, head && head.pattern === lane.patternId ? head.row : null,
          true);
      schedulePreview(); schedulePaint();
    }
    paintStatus();
    return claimed;
  }
  function endLater(ref, delay) {
    const gesture = rec.gestures.get(refKey(ref)); if (!gesture) return;
    clearTimeout(gesture.timer); gesture.timer = setTimeout(() => end(ref), delay);
  }
  const claimed = ref => {
    const key = refKey(ref),
      recent = rec.recent.get(key);
    return !!rec.gestures.get(key)?.claimed || !!(recent && Date.now() - recent.at < 500);
  };

  // ---- Modes and arming -------------------------------------------------------------
  function setMode(mode, quiet = false) {
    if (!MODES.includes(mode) || mode === rec.mode) { paintStatus(); return rec.mode; }
    if (rec.pass && mode === 'read') finishPass();
    rec.mode = mode; if (mode !== 'read') rec.lastWrite = mode;
    rec.warned.clear(); save(); paintStatus();
    try { window.STStudio?.refreshLiveAutomation?.(); } catch {}
    if (mode === 'write' && playing()) startWriteMode();
    if (!quiet) S()?.toast?.(`Automation ${LABELS[mode]}${mode === 'read' ? '' : rec.armed.size ? ` · ${rec.armed.size} armed` : ' · all parameters'} — ${HINTS[mode]}`);
    return rec.mode;
  }
  const toggleWrite = () => setMode(rec.mode === 'read' ? rec.lastWrite : 'read');
  function cycleMode(direction = 1) {
    const order = ['touch', 'latch', 'write'];
    const index = order.indexOf(rec.mode);
    return setMode(order[index < 0 ? (direction > 0 ? 0 : order.length - 1) : (index + direction + order.length) % order.length]);
  }
  function toggleArm(ref, wholeChannel = false) {
    if (!ref) return false;
    const key = wholeChannel ? `${ref.owner}|*` : refKey(ref);
    const armed = !rec.armed.has(key);
    if (armed) rec.armed.add(key); else rec.armed.delete(key);
    paintStatus();
    try { window.STStudio?.refreshLiveAutomation?.(); } catch {}
    S()?.toast?.(`${armed ? 'Armed' : 'Disarmed'} ${wholeChannel ? 'all parameters on this channel' : ref.name} for automation write${rec.mode === 'read' && armed ? '. Press ⌥W to choose Touch.' : '.'}${!rec.armed.size ? ' No arms left: every parameter can write.' : ''}`);
    if (armed && rec.mode === 'write' && playing() && !wholeChannel) startWriteMode();
    return armed;
  }
  const isArmed = (ref, wholeChannel = false) => !!ref && rec.armed.has(wholeChannel ? `${ref.owner}|*` : refKey(ref));
  function clearArms() { rec.armed.clear(); paintStatus(); try { window.STStudio?.refreshLiveAutomation?.(); } catch {} }
  /** Write mode overwrites explicitly armed parameters from the moment playback runs. */
  function startWriteMode() {
    for (const key of rec.armed) {
      const [owner, target] = [key.slice(0, key.lastIndexOf('|')), key.slice(key.lastIndexOf('|') + 1)];
      if (target === '*') continue;
      const ref = makeRef(owner, target, descriptor({ owner, target })?.name || target);
      if (!ref || blocked(ref)) continue;
      const head = playhead(); if (!head) continue;
      const lane = laneFor(ref, head.pattern);
      if (lane.seg) continue;
      const v = rec.gestures.get(key)?.value ?? descriptor(ref)?.value ?? lane.base;
      if (finite(v)) write(ref, Number(v));
    }
  }

  // ---- DOM controls: keyboard, pointer, input interception ---------------------------
  const isPluginTarget = ref => ref.target.startsWith('plugin:');
  function elementScale(el) { const scale = Number(el?.dataset?.automateScale); return scale > 0 ? scale : 1; }
  // Fine-control elements (fine-control.js) carry their own lane mapping: a scaled number field or a
  // curved slider whose value is a 0..1 position.
  const fine = el => window.STFine?.has?.(el) ? window.STFine : null;
  function elementValue(el, laneV) { const f = fine(el); return f ? f.elementFromLane(el, laneV) : String(laneV * elementScale(el)); }
  /** Lane value for an element value (tune is split over semitone / cent fields). */
  function laneValue(el, raw) {
    const key = el.dataset.instParam;
    if (key === 'tuneSemi' || key === 'tuneFine') {
      const semis = key === 'tuneSemi' ? Number(raw) : Number(document.getElementById('param-value-tuneSemi')?.value) || 0;
      const cents = key === 'tuneFine' ? Number(raw) : Number(document.getElementById('param-value-tuneFine')?.value) || 0;
      return clamp(semis + cents / 100, -48, 48);
    }
    const f = fine(el); if (f) return f.laneFromElement(el, raw);
    const value=Number(raw)/elementScale(el);
    return el.dataset.automate?.startsWith('plugin:') && window.STFine ? window.STFine.snapParameter(value,el) : value;
  }
  function currentLaneValue(el, ref) {
    const gesture = rec.gestures.get(refKey(ref));
    if (finite(gesture?.value)) return gesture.value;
    if (finite(el.value)) return laneValue(el, el.value);
    return descriptor(ref)?.value;
  }
  function reflect(el, ref, laneV, elementV) {
    const info = descriptor(ref), text = info?.format ? info.format(laneV) : String(elementV);
    const twins = el.dataset.instParam !== undefined
        ? document.querySelectorAll(`[data-inst-param="${el.dataset.instParam}"]`)
        : el.dataset.nksAddress !== undefined
        ? document.querySelectorAll(`[data-nks-address="${el.dataset.nksAddress}"]`)
        : el.dataset.modParam !== undefined
        ? document.querySelectorAll(`[data-mod-param="${el.dataset.modParam}"][data-mod-instrument="${el.dataset.modInstrument}"]`)
        : [el];
    for (const twin of twins) {
      if (twin !== el) twin.value = fine(twin) ? fine(twin).elementFromLane(twin, laneV) : String(elementV);
      if (twin.type === 'range') twin.setAttribute('aria-valuetext', text);
    }
    if (el.dataset.instParam !== undefined) for (const out of document.querySelectorAll(`[data-param-readout="${el.dataset.instParam}"]`)) out.textContent = text;
    if (el.dataset.nksAddress !== undefined) for (const out of document.querySelectorAll(`[data-nks-display="${el.dataset.nksAddress}"]`)) out.textContent = text;
    const out = el.id && document.getElementById(`${el.id}-out`); if (out) out.textContent = text;
  }
  const ARROWS = ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'];
  function stepElement(el, ref, event) {
    const up = event.key === 'ArrowUp' || event.key === 'ArrowRight' || event.key === 'PageUp';
    const coarse = event.shiftKey || event.key.startsWith('Page');
    const current = finite(el.value) ? Number(el.value) : Number(el.defaultValue) || 0;
    const min = finite(el.min) ? Number(el.min) : 0, max = finite(el.max) ? Number(el.max) : 1, step = Number(el.step);
    const info = descriptor(ref) || {};
    const f = fine(el);
    if (f) {
      const v = f.stepElement(el, up ? 1 : -1, { fine: event.altKey && !coarse, coarse });
      reflect(el, ref, v, Number(el.value));
      return v;
    }
    let next;
    if (isPluginTarget(ref)) next = window.STFine?.stepParameter(current,up?1:-1,{coarse,fine:event.altKey},info.parameter||el)
        ?? clamp(current + (up ? 1 : -1) * (coarse ? .1 : event.altKey ? .001 : .01), min, max);
    else if (step >= 1) next = clamp(Math.round(current) + (up ? 1 : -1) * (coarse ? Math.max(1, Math.round((max - min) / 10)) : 1), min, max);
    else {
      const elementInfo = { min, max, log: !!info.log && min > 0, step: step > 0 ? step : 0 };
      next = fromUnit(toUnit(current, elementInfo) + (up ? 1 : -1) * (coarse ? .1 : .01), elementInfo);
      if (next === current && step > 0) next = clamp(current + (up ? step : -step), min, max);
    }
    if (!window.STFine?.parameterSteps(info.parameter||el)) next = Number(next.toFixed(6));
    el.value = String(next);
    const v = laneValue(el, next);
    reflect(el, ref, v, next);
    return v;
  }
  function onKeyDown(event) {
    if (event.defaultPrevented || event.isComposing || event.metaKey || event.ctrlKey) return;
    // ⌥W: Read ⇄ last write mode · ⌥⇧W: cycle Touch → Latch → Write.
    if (event.altKey && (event.code === 'KeyW' || event.key?.toLowerCase() === 'w')) {
      const field = event.target?.closest?.('input[type=text],input[type=search],input:not([type]),textarea,[contenteditable=""],[contenteditable=true]');
      if (field) return;
      event.preventDefault(); event.stopPropagation();
      if (!event.repeat) { if (event.shiftKey) cycleMode(1); else toggleWrite(); }
      return;
    }
    const el = event.target;
    if (!el?.matches?.('input[data-automate]') || el.disabled) return;
    const horizontal = (event.key === 'ArrowLeft' || event.key === 'ArrowRight') && el.type === 'range' && el.dataset.instParam !== undefined;
    if (!ARROWS.includes(event.key) && !horizontal) return;
    if (event.altKey && !isPluginTarget({ target: el.dataset.automate }) && !fine(el)) return;
    const ref = refFor(el);
    if (!ref || (!wouldRecord(ref) && !claimed(ref))) return;
    event.preventDefault(); event.stopImmediatePropagation();
    const v = stepElement(el, ref, event);
    value(ref, v, 'keyboard');
    endLater(ref, KEY_IDLE_MS);
  }
  function onPointerDown(event) {
    const el = event.target?.closest?.('input[type=range][data-automate]');
    if (!el || el.disabled || event.button > 0) return;
    const ref = refFor(el); if (!ref || !wouldRecord(ref)) return;
    rec.pointer = ref;
    begin(ref, currentLaneValue(el, ref), 'pointer');
  }
  function onPointerUp() { const ref = rec.pointer; rec.pointer = null; if (ref) end(ref); }
  function onInput(event) {
    const el = event.target;
    if (!el?.matches?.('[data-automate]') || el.disabled) return;
    const ref = refFor(el); if (!ref) return;
    const recording = wouldRecord(ref), owned = claimed(ref);
    if (!recording && !owned) return;
    event.stopImmediatePropagation();
    if (event.type === 'input' && el.type === 'number') return; // typed drafts commit on change
    if (!finite(el.value)) return;
    const v = laneValue(el, el.value), gesture = rec.gestures.get(refKey(ref)), recent = rec.recent.get(refKey(ref));
    reflect(el, ref, v, Number(el.value));
    // The change that ends a drag repeats the last recorded value: swallow it.
    if (!recording || (event.type === 'change' && (gesture?.value === v || (!gesture && recent?.value === v)))) return;
    value(ref, v, rec.pointer && refKey(rec.pointer) === refKey(ref) ? 'pointer' : 'input');
    if (!(rec.pointer && refKey(rec.pointer) === refKey(ref))) endLater(ref, event.type === 'change' ? 220 : KEY_IDLE_MS);
  }

  // ---- MIDI CC learn and control ------------------------------------------------------
  function learn(ref) {
    if (!ref) return;
    rec.learn = { owner: ref.owner, target: ref.target, name: ref.name };
    S()?.toast?.(`Move a MIDI controller knob or fader to control ${ref.name}. Escape cancels.`);
    paintStatus();
  }
  function forget(ref) {
    for (const [key, value] of rec.midi) if (value.owner === ref.owner && value.target === ref.target) rec.midi.delete(key);
    save(); S()?.toast?.(`Forgot the MIDI controller for ${ref.name}.`);
  }
  const mappingFor = ref => [...rec.midi].find(([, value]) => value.owner === ref.owner && value.target === ref.target);
  function midi(payload) {
    const status = Number(payload?.status), kind = status & 0xF0;
    if (kind !== 0xB0) return false;
    const cc = Number(payload.data1), raw = Number(payload.data2);
    if (cc === 64 || cc >= 120) return false;
    const key = `${status & 0x0F}:${cc}`;
    if (rec.learn) {
      const mapping = { ...rec.learn };
      for (const [other, value] of rec.midi) if (value.owner === mapping.owner && value.target === mapping.target) rec.midi.delete(other);
      rec.midi.set(key, mapping); rec.learn = null; save(); paintStatus();
      S()?.toast?.(`CC ${cc} (channel ${(status & 0x0F) + 1}) now controls ${mapping.name}.`);
      return true;
    }
    const mapping = rec.midi.get(key); if (!mapping) return false;
    const ref = makeRef(mapping.owner, mapping.target, mapping.name); if (!ref) return false;
    const info = descriptor(ref); if (!info) return false;
    const v = fromUnit(clamp(raw, 0, 127) / 127, info, info.fineCurve);
    if (wouldRecord(ref) || claimed(ref)) {
      value(ref, v, 'midi'); endLater(ref, MIDI_IDLE_MS);
      const el = controlElement(ref);
      if (el && el.dataset.instParam !== 'tuneSemi' && el.dataset.instParam !== 'tuneFine') {
        el.value = elementValue(el, v);
        reflect(el, ref, v, Number(el.value));
      }
      return true;
    }
    // Not recording: move the visible control so its own handlers edit the base value.
    const el = controlElement(ref);
    if (!el || el.dataset.instParam === 'tuneSemi' || el.dataset.instParam === 'tuneFine') return false;
    el.value = elementValue(el, v);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    // One commit per controlled parameter: two knobs turned together each keep their own edit.
    const mapped = refKey(ref);
    clearTimeout(rec.midiChange.get(mapped));
    rec.midiChange.set(mapped, setTimeout(() => {
      rec.midiChange.delete(mapped);
      // Another parameter's commit may have re-rendered the page: commit on the control that shows this one now.
      const node = el.isConnected ? el : controlElement(ref);
      if (!node) return;
      if (node !== el) node.value = elementValue(node, v);
      node.dispatchEvent(new Event('change', { bubbles: true }));
    }, MIDI_IDLE_MS));
    return true;
  }
  function controlElement(ref) {
    for (const el of document.querySelectorAll(`input[type=range][data-automate="${typeof CSS !== 'undefined' && CSS.escape
        ? CSS.escape(ref.target)
        : ref.target.replace(/["\\]/g, '\\$&')}"]`)) {
      const other = refFor(el); if (other && other.owner === ref.owner) return el;
    }
    return null;
  }

  // ---- Native events ------------------------------------------------------------------
  function event(type, payload = {}) {
    if (type === 'status' || type === 'transport.status') {
      const revision=payload.transportSeekRevision;
      const seeked=typeof revision==='string'&&rec.seekRevision!==undefined&&revision!==rec.seekRevision;
      if(typeof revision==='string')rec.seekRevision=revision;
      const now = playing();
      if (now && !rec.wasPlaying && rec.mode === 'write') startWriteMode();
      if (!now && rec.pass) finishPass();
      if (rec.wasPlaying !== now) { rec.wasPlaying = now; paintStatus(); }
      else if (rec.pass) advance(seeked);
      return;
    }
    if (type === 'midi.message') { midi(payload); return; }
    if (type === 'project.loaded' || type === 'project.new') {
      if (rec.pass) abortPass();
      for (const gesture of rec.gestures.values()) clearTimeout(gesture.timer);
      for (const timer of rec.midiChange.values()) clearTimeout(timer);
      rec.midiChange.clear();
      rec.gestures.clear(); rec.recent.clear(); rec.armed.clear(); rec.warned.clear();
      rec.pointer = null; rec.seekRevision=undefined; rec.wasPlaying = false; rec.learn = null; paintStatus();
    }
  }

  // ---- Status chip --------------------------------------------------------------------
  function statusText() {
    const writing = activeLanes().length;
    if (rec.pass?.commitError) return { text: 'Automation take pending', state: 'armed' };
    if (rec.learn) return { text: `MIDI learn · ${rec.learn.name}`, state: 'learn' };
    if (rec.mode === 'read') return { text: 'Auto Read', state: 'read' };
    if (writing) return { text: `● ${LABELS[rec.mode]} · writing ${writing}`, state: 'writing' };
    return { text: `Auto ${LABELS[rec.mode]}${rec.armed.size ? ` · ${rec.armed.size} armed` : ''}`, state: 'armed' };
  }
  function statusMarkup() {
    const status = statusText();
    return `<button type="button" id="automation-record-status" class="automation-record-status" data-state="${status.state}" tabindex="-1"`
        + ` title="Automation write mode: ${LABELS[rec.mode]}. ${HINTS[rec.mode]} ⌥W Read / ${LABELS[rec.lastWrite]} · ⌥⇧W cycle Touch, Latch, Write">${S()?.escapeHTML ? S().escapeHTML(status.text) : status.text}</button>`;
  }
  function paintStatus() {
    if (typeof document === 'undefined' || !document.querySelector) return;
    let chip = document.getElementById('automation-record-status');
    if (!chip) {
      const host = document.querySelector('.statusbar .end'); if (!host) return;
      host.insertAdjacentHTML('afterbegin', statusMarkup());
      chip = document.getElementById('automation-record-status');
      chip?.addEventListener('click', event => { if (event.altKey || event.shiftKey) cycleMode(1); else toggleWrite(); });
      return;
    }
    const status = statusText();
    if (chip.textContent !== status.text) chip.textContent = status.text;
    chip.dataset.state = status.state;
    chip.title = `Automation write mode: ${LABELS[rec.mode]}. ${HINTS[rec.mode]} ⌥W Read / ${LABELS[rec.lastWrite]} · ⌥⇧W cycle Touch, Latch, Write`;
  }

  // ---- Context menu ------------------------------------------------------------------
  function contextItems({ target } = {}) {
    const control = target?.closest?.('[data-automate]');
    if (!control || control.dataset.automateRecord === 'off') return null;
    const ref = refFor(control); if (!ref) return null;
    const mapping = mappingFor(ref), why = blocked(ref);
    return [
      { title: isArmed(ref)
          ? 'Disarm automation write'
          : 'Arm automation write', checked: isArmed(ref), enabled: !why, action: () => toggleArm(ref) },
      ref.owner
          ? { title: isArmed(ref, true)
              ? 'Disarm all parameters on this channel'
              : 'Arm all parameters on this channel', checked: isArmed(ref, true), action: () => toggleArm(ref, true) }
          : null,
      { title: 'Automation write mode', items: MODES.map(mode => ({ title: LABELS[mode], checked: rec.mode === mode, key: (mode === 'read' && rec.mode !== 'read')
          || (mode === rec.lastWrite && rec.mode === 'read')
          ? '⌥W'
          : undefined, action: () => setMode(mode) })) },
      mapping
          ? { title: `Forget MIDI controller (CC ${mapping[0].split(':')[1]})`, action: () => forget(ref) }
          : { title: 'Learn MIDI controller…', enabled: !why, action: () => learn(ref) }
    ];
  }

  // ---- Boot ---------------------------------------------------------------------------
  load();
  if (typeof document !== 'undefined' && document.addEventListener) {
    document.addEventListener('keydown', event => {
      if (rec.learn && event.key === 'Escape') { rec.learn = null; paintStatus(); S()?.toast?.('MIDI learn canceled.'); event.preventDefault(); event.stopImmediatePropagation(); return; }
      onKeyDown(event);
    }, true);
    document.addEventListener('pointerdown', onPointerDown, true);
    for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, onPointerUp, true);
    document.addEventListener('input', onInput, true);
    document.addEventListener('change', onInput, true);
    const attach = () => {
      if (window.STContextMenu?.register && !rec.menuRegistered) { rec.menuRegistered = true; window.STContextMenu.register(contextItems, 89); }
      const app = document.getElementById?.('app');
      if (app && typeof MutationObserver === 'function' && !rec.observer) {
        rec.observer = new MutationObserver(() => {
          if (!document.getElementById('automation-record-status')) paintStatus();
        });
        rec.observer.observe(app, { childList: true });
      }
      paintStatus();
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', attach); else setTimeout(attach, 0);
  }

  window.STAutomationRecord = {
    get mode() { return rec.mode; }, get lastWriteMode() { return rec.lastWrite; }, get modes() { return MODES.slice(); }, labels: LABELS,
    get editGeneration() {
      return rec.generation;
    },
    get hasPendingEdits() {
      return !!rec.pass && [...rec.pass.lanes.values()].some(lane => lane.written);
    },
    get isRecording() {
      return !!rec.pass;
    },
    get hasPending() {
      return !!rec.pass && [...rec.pass.lanes.values()].some(lane => lane.written);
    },
    get armed() {
      return [...rec.armed];
    },
    setMode, toggleWrite, cycleMode, toggleArm, isArmed, clearArms, refFor, makeRef,
    begin, value, end, wouldRecord, claimed, finishPass, livePoints, isWriting,
    learn, forget, contextItems, statusMarkup, event, key: onKeyDown,
    _test: {
      toUnit,
      fromUnit,
      valueAt,
      thin,
      finalize,
      limitPoints,
      startSegment,
      sample,
      endSegment,
      engineView,
      quantize,
      rec,
      draftProject,
      playhead,
      advance,
      constants: { Q, SAMPLE, RETURN, TOLERANCE }
    }
  };
  window.STPendingEdits.register('automationRecord', { order: 60, generation: () => window.STAutomationRecord?.editGeneration ?? 0 });
})();
