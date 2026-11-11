/* Optional mouse/trackpad editing for the pattern grid: drag to select a block of steps,
   drag a selection to move it (hold Option to copy). Keyboard editing stays primary.
   Steps moved onto a member of a track group play that member's own instrument when they
   played their source track's instrument (the same rule as paste). */
(() => {
  'use strict';
  const S = () => window.ST;
  let gesture = null, suppressClick = false;

  const cellAt = (x, y) => document.elementFromPoint(x, y)?.closest?.('#tracker td[data-cell]');
  const coords = cell => cell.dataset.cell.split(':').map(Number);
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));

  function covers(b, track, row) {
    return !!b && track >= b.t0 && track <= b.t1 && row >= b.r0 && row <= b.r1;
  }
  function inSelection(track, row) { return covers(S().selectedBounds?.(), track, row); }
  function render() {
    const state = S().state, b = S().selectedBounds();
    document.querySelectorAll('#tracker td[data-cell]').forEach(cell => {
      const [t, r] = coords(cell),
        active = t === state.track && r === state.row;
      cell.classList.toggle('selected', active);
      cell.classList.toggle('selection', covers(b, t, r));
      cell.querySelectorAll('[data-column]').forEach(column => column.classList.toggle('active-column',
          active && Number(column.dataset.column) === state.column && Number(column.dataset.noteColumn || 0) === (window.STApp?.cursorNoteColumn() || 0)));
    });
  }
  function current(done) {
    return S().state.project === done.project && S().pattern() === done.pattern && S().state.view === 'pattern' && !S().state.modal;
  }
  function cleanup() {
    window.removeEventListener('pointermove', move);
    window.removeEventListener('pointerup', end);
    window.removeEventListener('pointercancel', cancel);
    window.removeEventListener('blur', cancel);
  }
  function cancel() { const done = gesture; gesture = null; cleanup(); paintGhost(); if (done && current(done)) S().refresh(); }

  // Move target preview: outline the cells the block would land on.
  function paintGhost() {
    document.querySelectorAll('#tracker td.grid-drop').forEach(cell => cell.classList.remove('grid-drop', 'grid-drop-copy'));
    if (!gesture || gesture.kind !== 'move' || (!gesture.dt && !gesture.dr)) return;
    const { bounds: b, dt, dr } = gesture;
    for (let t = b.t0 + dt; t <= b.t1 + dt; t++) for (let r = b.r0 + dr; r <= b.r1 + dr; r++)
      document.querySelectorAll(`#tracker td[data-cell="${t}:${r}"]`).forEach(cell => cell.classList.add('grid-drop',
          ...(gesture.copy ? ['grid-drop-copy'] : [])));
  }

  // A step that played its source track's own instrument plays the destination member's.
  function moved(step, from, to) {
    const project = S().state.project, copy = window.STUtil.clone(step), source = project.tracks[from], target = project.tracks[to];
    if (copy && source && target && from !== to && window.STGroups.ofTrack(project, to) && copy.instrument === source.instrument) copy.instrument = target.instrument ?? 0;
    return copy;
  }

  function limits() {
    const app = S(), pat = app.pattern();
    return { tracks: app.state.project.tracks.length, rows: pat.length };
  }

  function start(event) {
    if (gesture || event.button !== 0 || event.shiftKey || event.metaKey || event.ctrlKey) return;
    if (event.target.closest?.('[data-gate-inspector]')) return;
    const cell = event.target.closest?.('#tracker td[data-cell]');
    if (!cell || !S()?.state?.project || S().state.modal || S().state.view !== 'pattern') return;
    const [track, row] = coords(cell), state = S().state, step = S().pattern().steps[track]?.[row];
    event.preventDefault();
    document.getElementById('tracker')?.focus({ preventScroll: true });
    const onCursor = !state.selection && state.track === track && state.row === row && !!step;
    if (inSelection(track, row) || onCursor) {
      const b = S().selectedBounds() || { t0: track, t1: track, r0: row, r1: row };
      gesture = { kind: 'move', origin: [track, row], bounds: b, dt: 0, dr: 0, copy: event.altKey, moved: false };
    } else {
      gesture = { kind: 'select', origin: [track, row], moved: false };
      state.selection = { track, row }; state.track = track; state.row = row;
      const span = event.target.closest?.('[data-column]'), column = span?.dataset.column;
      if (column !== undefined) {
        state.column = Number(column);
        state.noteColumn = Number(span.dataset.noteColumn || 0);
      }
      state.instrument = state.project.tracks[track]?.instrument || 0;
      render();
    }
    gesture.project = state.project; gesture.pattern = S().pattern();
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', end);
    window.addEventListener('pointercancel', cancel);
    window.addEventListener('blur', cancel);
  }

  function move(event) {
    if (!gesture) return;
    if (!current(gesture)) return cancel();
    const cell = cellAt(event.clientX, event.clientY);
    if (!cell) return;
    const [track, row] = coords(cell), state = S().state;
    if (gesture.kind === 'select') {
      if (track === state.track && row === state.row) return;
      gesture.moved = true; state.track = track; state.row = row; state.instrument = state.project.tracks[track]?.instrument || 0; render();
    } else {
      const { tracks, rows } = limits(), b = gesture.bounds;
      let dt = clamp(track - gesture.origin[0], -b.t0, tracks - 1 - b.t1);
      const dr = clamp(row - gesture.origin[1], -b.r0, rows - 1 - b.r1);
      gesture.copy = event.altKey;
      if (dt === gesture.dt && dr === gesture.dr) { paintGhost(); return; }
      gesture.dt = dt; gesture.dr = dr; gesture.moved = true; paintGhost();
    }
  }

  function end(event) {
    cleanup();
    const done = gesture; gesture = null; paintGhost();
    if (!done || !current(done)) return;
    if (done.moved) { suppressClick = true; setTimeout(() => { suppressClick = false; }, 50); }
    if (done.kind === 'select') {
      // A plain click without dragging behaves like before: cursor only, no selection.
      if (!done.moved) S().state.selection = null;
      S().refresh();
      return;
    }
    if (!done.dt && !done.dr) return;
    // Apply after the trailing click so the grid re-renders under a finished gesture.
    const copy = done.copy || event.altKey;
    setTimeout(() => apply(done, copy), 0);
  }

  function apply(done, copy) {
    const { bounds: b, dt, dr } = done, app = S();
    if (!current(done)) return;
    const applied = app.mutate(copy ? 'copy steps' : 'move steps', () => {
      const steps = app.pattern().steps, block = [];
      for (let t = b.t0; t <= b.t1; t++) block.push(steps[t].slice(b.r0, b.r1 + 1).map(step => step ? moved(step, t, t + dt) : null));
      if (!copy) for (let t = b.t0; t <= b.t1; t++) for (let r = b.r0; r <= b.r1; r++) steps[t][r] = null;
      block.forEach((column, i) => column.forEach((step, j) => {
        window.STApp?.growNoteColumns(app.state.project, b.t0 + dt + i, step);
        steps[b.t0 + dt + i][b.r0 + dr + j] = step;
      }));
    });
    if (!applied) return;
    const state = app.state;
    state.selection = { track: b.t0 + dt, row: b.r0 + dr };
    state.track = b.t1 + dt;
    state.row = b.r1 + dr;
    state.instrument = state.project.tracks[state.track]?.instrument || 0;
    app.refresh();
    app.toast(`${copy ? 'Copied' : 'Moved'} ${b.r1 - b.r0 + 1} steps × ${b.t1 - b.t0 + 1} tracks`);
  }

  document.addEventListener('pointerdown', start, true);
  // The grid's own click handler would reset the selection right after a drag.
  document.addEventListener('click', event => {
    if (suppressClick && event.target.closest?.('#tracker')) {
      event.stopPropagation();
      event.preventDefault();
    }
  }, true);
  window.STGridMouse = { get active() { return !!gesture; } };
})();
