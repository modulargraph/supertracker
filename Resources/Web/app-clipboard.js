/* Selection and clipboard: copy, paste, clear and fill the selected block, and the
   system clipboard events. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clone, state, pattern, toast, mutate, stableJSON, TRACK_SANITY_LIMIT } = app;

  // A collapsed group is one column: a selection edge inside it covers all of its members.
  function widenToGroups(b) {
    const groups = window.STGroups, p = state.project;
    if (!b || !groups?.list(p).length) return b;
    const first = groups.ofTrack(p, b.t0), last = groups.ofTrack(p, b.t1);
    if (first?.collapsed) b.t0 = Math.min(b.t0, groups.span(p, first).first);
    if (last?.collapsed) b.t1 = Math.max(b.t1, groups.span(p, last).last);
    return b;
  }

  function selectedBounds() {
    return state.selection ? widenToGroups({
      r0: Math.min(state.selection.row, state.row),
      r1: Math.max(state.selection.row, state.row),
      t0: Math.min(state.selection.track, state.track),
      t1: Math.max(state.selection.track, state.track)
    }) : null;
  }

  const cursorBounds = () => widenToGroups({ r0: state.row, r1: state.row, t0: state.track, t1: state.track });

  function eachSelected(fn) {
    const b = selectedBounds() || cursorBounds(),
      pat = pattern();
    for (let t = b.t0; t <= b.t1; t++) for (let r = b.r0; r <= b.r1; r++) fn(pat.steps[t], r, t);
  }

  function copySelection(cut = false) {
    const b = selectedBounds() || cursorBounds();
    state.clipboardInstruments = state.project.tracks.slice(b.t0, b.t1 + 1).map(t => t.instrument ?? 0);
    state.clipboard = pattern().steps.slice(b.t0, b.t1 + 1).map(t => clone(t.slice(b.r0, b.r1 + 1)));
    toast(`Copied ${b.r1 - b.r0 + 1} steps × ${b.t1 - b.t0 + 1} tracks`);
    if (cut) clearSelection();
  }

  // A group member plays its own instrument: a step that played its source track's own
  // instrument plays the destination member's instrument (a kick pasted onto the snare plays the snare).
  const pastedStep = (cell, index, column) => {
    const step = clone(cell), track = state.project.tracks[index];
    if (step && track && window.STGroups?.ofTrack(state.project, index)
        && step.instrument !== undefined && step.instrument === state.clipboardInstruments?.[column]) step.instrument = track.instrument ?? 0;
    return step;
  };

  function pasteSelection() {
    if (!state.clipboard) return toast('Copy some steps first.');
    mutate('paste steps', () => state.clipboard.forEach((track, t) => {
      const index = state.track + t;
      if (index >= state.project.tracks.length) return;
      track.forEach((s, r) => {
        const row = state.row + r;
        if (row >= pattern().length) return;
        const step = app.tidyStep ? app.tidyStep(pastedStep(s, index, t)) : pastedStep(s, index, t);
        app.growNoteColumns?.(state.project, index, step);
        pattern().steps[index][row] = step;
      });
    }));
  }

  /* A selection clears whole steps (every note column and the FX). Without one, Delete clears what the cursor is on:
     an extra note column's cell, or column 0's note and FX (other note columns stay; app-note-columns.js). */
  function clearSelection() {
    if (!state.selection && app.clearCell && (state.project.tracks[state.track]?.noteColumns || 1) > 1
        && !window.STGroups?.ofTrack(state.project, state.track)?.collapsed) {
      return mutate('clear steps', () => app.clearCell(pattern().steps[state.track], state.row, app.cursorNoteColumn()));
    }
    mutate('clear steps', () => eachSelected((a, r) => { a[r] = null; }));
  }

  function fillSelection() {
    const b = selectedBounds();
    if (!b) return toast('Select a phrase with Shift + arrows, then fill down.');
    mutate('repeat phrase', () => {
      const pat = pattern(),
        size = b.r1 - b.r0 + 1;
      for (let t = b.t0; t <= b.t1; t++) {
        const src = clone(pat.steps[t].slice(b.r0, b.r1 + 1));
        for (let r = b.r1 + 1; r < pat.length; r++) pat.steps[t][r] = clone(src[(r - b.r0) % size]);
      }
    });
  }

  function clipboardEvent(event) {
    if (state.view !== 'pattern'
        || state.modal
        || app.isTextInput(document.activeElement)
        || !document.activeElement?.closest?.('.tracker')) return;
    event.preventDefault();
    if (event.type === 'paste') {
      const data = event.clipboardData?.getData('application/x-supertracker-steps')
          || event.clipboardData?.getData('text/plain');
      if (data) {
        try {
          const parsed = JSON.parse(data);
          if (parsed.supertracker === 1
              && Array.isArray(parsed.steps)
              && parsed.steps.length <= TRACK_SANITY_LIMIT
              && parsed.steps.every(t => Array.isArray(t)
                  && t.length <= 256
                  && t.every(s => s === null
                      || typeof s === 'object' && Number.isFinite(s.note) && s.note >= -3 && s.note <= 127))) {
            if (stableJSON(parsed.steps) !== stableJSON(state.clipboard)) state.clipboardInstruments = null;
            state.clipboard = parsed.steps;
          } else {
            return toast('Clipboard does not contain SuperTracker steps.');
          }
        } catch {
          return toast('Clipboard does not contain SuperTracker steps.');
        }
      }
      pasteSelection();
    } else {
      copySelection(false);
      const text = JSON.stringify({ supertracker: 1, steps: state.clipboard });
      event.clipboardData?.setData('application/x-supertracker-steps', text);
      event.clipboardData?.setData('text/plain', text);
      if (event.type === 'cut') clearSelection();
    }
  }

  Object.assign(app, {
    selectedBounds, cursorBounds, eachSelected, copySelection, pasteSelection, clearSelection, fillSelection,
    clipboardEvent
  });
})();
