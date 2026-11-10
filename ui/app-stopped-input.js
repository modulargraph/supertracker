/* Stopped-transport input: keys entered while stopped advance the cursor by the edit step
   and are applied once their engine acknowledgement arrives. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    clamp, state, pattern, toast, mutate, paintRecordedCell, stepInstrument, transpose, moveGrid,
    pendingInputEdges, stoppedGridScopes, inputDomain, MIDI_CHORD_MS, captureCursor
  } = app;

  function stoppedGridCurrent(scope) {
    return !scope.invalid
        && !scope.lineage?.invalid
        && state.project === scope.project
        && scope.project.patterns.includes(scope.pat)
        && scope.project.tracks[scope.track] === scope.trackObject
        && scope.project.instruments[scope.instrument] === scope.instrumentObject
        && state.recording
        && !state.playing
        && app.inputArmGeneration === scope.armGeneration
        && app.inputEditGeneration === scope.editGeneration
        && inputDomain(state.project) === scope.domain
        && (scope.detached || !state.modal
            && pattern() === scope.pat
            && state.view === scope.view
            && state.track === scope.track
            && state.instrument === scope.instrument
            && state.column === scope.column
            && state.editStep === scope.editStep
            && state.row === (scope.lineage?.visibleRow ?? scope.row)
            && JSON.stringify(state.selection ?? null) === scope.selection);
  }

  function finishStoppedGridItem(scope) {
    if (--scope.remaining === 0) {
      stoppedGridScopes.delete(scope);
      if (app.stoppedGridScope === scope) app.stoppedGridScope = null;
    }
  }

  function publishInputMutation(project, priorGeneration) {
    for (const scope of stoppedGridScopes) if (scope.project === project) {
      if (scope.editGeneration === priorGeneration) {
        scope.editGeneration = app.inputEditGeneration;
      } else {
        scope.invalid = true;
      }
    }
  }

  function applyDetachedGridKey(scope, key, alt, shift) {
    const row = scope.lineage?.visibleRow ?? scope.row,
      selection = JSON.parse(scope.selection);
    if (alt) {
      const amount = (key === 'ArrowUp' ? 1 : -1) * (shift ? 12 : 1),
        r0 = Math.min(selection?.row ?? row, row),
        r1 = Math.max(selection?.row ?? row, row),
        t0 = Math.min(selection?.track ?? scope.track, scope.track),
        t1 = Math.max(selection?.track ?? scope.track, scope.track),
        steps = [];
      for (let t = t0; t <= t1; t++) for (let r = r0; r <= r1; r++) {
        for (const { cell } of app.playedCells(scope.pat.steps[t]?.[r])) if (clamp(cell.note + amount, 0, 127) !== cell.note) steps.push(cell);
      }
      if (steps.length) mutate('transpose', () => {
        for (const step of steps) step.note = clamp(step.note + amount, 0, 127);
      });
      scope.row = row;
    } else {
      scope.selection = JSON.stringify(shift ? selection || { track: scope.track, row } : null);
      scope.row = clamp(row
          + (key === 'ArrowUp'
              ? -1
              : key === 'ArrowDown'
              ? 1
              : key === 'PageUp'
              ? -16
              : key === 'PageDown'
              ? 16
              : key === 'Home'
              ? -row
              : scope.pat.length - row - 1),
          0, scope.pat.length - 1);
    }
    scope.lineage = null;
  }

  function queueStoppedGridKey(key, alt, shift) {
    if (state.playing || !state.recording) return false;
    let scope = app.stoppedGridScope;
    if (!scope) {
      const lineage = app.inputCursorLineage;
      if (!lineage
          || ![...pendingInputEdges].some(edge => edge.on && edge.capture.cursor?.lineage === lineage)) return false;
      scope = {
        project: state.project,
        pat: pattern(),
        track: state.track,
        instrument: state.instrument,
        trackObject: state.project.tracks[state.track],
        instrumentObject: state.project.instruments[state.instrument],
        view: state.view,
        column: state.column,
        editStep: state.editStep,
        row: state.row,
        lineage,
        selection: JSON.stringify(state.selection ?? null),
        armGeneration: app.inputArmGeneration,
        editGeneration: app.inputEditGeneration,
        domain: inputDomain(state.project),
        remaining: 0,
        invalid: false,
        detached: false
      };
      app.stoppedGridScope = scope;
      stoppedGridScopes.add(scope);
    }
    scope.remaining++;
    app.inputCaptureGeneration++;
    app.inputCaptureTail = app.inputCaptureTail.then(() => {
      if (!stoppedGridCurrent(scope)) {
        scope.invalid = true;
        return;
      }
      const priorGeneration = app.inputEditGeneration;
      if (scope.detached) {
        applyDetachedGridKey(scope, key, alt, shift);
      } else {
        if (alt) {
          transpose((key === 'ArrowUp' ? 1 : -1) * (shift ? 12 : 1));
        } else {
          moveGrid(0,
              key === 'ArrowUp'
                  ? -1
                  : key === 'ArrowDown'
                  ? 1
                  : key === 'PageUp'
                  ? -16
                  : key === 'PageDown'
                  ? 16
                  : key === 'Home'
                  ? -state.row
                  : pattern().length - state.row - 1,
              0, shift);
        }
        scope.row = state.row;
        scope.lineage = null;
        scope.selection = JSON.stringify(state.selection ?? null);
      }
      // A refused mutation may replace the project with its rollback checkpoint.
      if (state.project !== scope.project) {
        scope.invalid = true;
        return;
      }
      publishInputMutation(scope.project, priorGeneration);
    }).catch(error => {
      scope.invalid = true;
      toast(error.message, 'error');
    }).finally(() => finishStoppedGridItem(scope));
    return true;
  }

  function detachStoppedInputCursor() {
    if (state.playing) return;
    for (const edge of pendingInputEdges) if (edge.on
        && edge.capture.project === state.project
        && edge.capture.cursor?.lineage.pat === pattern()) edge.capture.cursor.lineage.detached = true;
    if (app.stoppedGridScope) {
      app.stoppedGridScope.detached = true;
      if (app.stoppedGridScope.lineage) app.stoppedGridScope.lineage.detached = true;
      app.stoppedGridScope = null;
    }
    app.inputCursorLineage = null;
  }

  function applyStoppedInput(capture) {
    if (capture.gridScope) {
      const scope = capture.gridScope;
      if (!scope.detached
          && (pattern() !== scope.pat || state.view !== scope.view || state.track !== scope.track || state.instrument !== scope.instrument || state.row !== (scope.lineage?.visibleRow ?? scope.row) || state.column !== scope.column || state.editStep !== scope.editStep)) {
        scope.detached = true;
        if (scope.lineage) scope.lineage.detached = true;
      }
      if (!stoppedGridCurrent(scope)) return false;
      if (scope.detached) {
        const lineage = scope.lineage || {
          project: scope.project,
          pat: scope.pat,
          track: scope.track,
          instrument: scope.instrument,
          view: scope.view,
          visibleRow: scope.row,
          nextRow: scope.row,
          column: scope.column,
          editStep: scope.editStep,
          invalid: false,
          detached: true,
          writes: new Map()
        };
        const chordInput = capture.source === 'midi',
          now = Date.now(), columns = capture.columnCursor || {};
        if (lineage.chord && (chordInput && now - lineage.chord.at <= MIDI_CHORD_MS || columns.held)) {
          capture.cursor = { lineage, row: lineage.chord.row, merged: true };
        } else {
          const row = lineage.nextRow;
          lineage.nextRow = (row + lineage.editStep) % lineage.pat.length;
          lineage.chord = chordInput || columns.columns ? { row, at: now } : null;
          capture.cursor = { lineage, row };
        }
      } else {
        capture.cursor = captureCursor(capture.project, capture.track, capture.instrument, capture.source === 'midi', capture.columnCursor);
      }
      const { pat } = capture.cursor.lineage,
        row = capture.cursor.row;
      capture.patterns.get(pat.id).rows[row] = {
        ref: pat.steps[capture.track][row],
        value: JSON.stringify(pat.steps[capture.track][row])
      };
      capture.gridScope.lineage = capture.cursor.lineage;
    }
    const { lineage, row } = capture.cursor,
      pat = lineage.pat;
    // A cursor move is not cancellation: the native edge belongs to its captured
    // target. Detach cursor advancement so its eventual edit cannot steal focus.
    if (!lineage.detached
        && (pattern() !== pat || state.view !== lineage.view || state.track !== lineage.track || state.instrument !== lineage.instrument || state.row !== lineage.visibleRow || state.column !== lineage.column || state.editStep !== lineage.editStep)) lineage.detached = true;
    if (lineage.invalid || !capture.project.patterns.includes(pat) || row >= pat.length) return false;
    const before = capture.patterns.get(pat.id)?.rows[row],
      step = pat.steps[capture.track]?.[row],
      written = lineage.writes.get(row);
    const ownWrite = written?.ref === step && written.value === JSON.stringify(step);
    if (!before || !ownWrite && (before.ref !== step || before.value !== JSON.stringify(step))) return false;
    const notes = capture.cursor.merged && ownWrite && step?.note >= 0
        ? [...new Set([...capture.notes, step.note, ...(step.chord || []).map(x => step.note + x)])]
        : capture.notes;
    const root = Math.min(...notes),
      intervals = notes.map(note => note - root).sort((a, b) => a - b);
    const priorGeneration = app.inputEditGeneration, column = capture.noteColumn;
    if (!mutate('record note', () => {
      // Several note columns (app-note-columns.js): the note takes its own column instead of joining a chord.
      if (column != null) {
        const values = { note: capture.note, ...stepInstrument(capture.track, capture.instrument), velocity: capture.velocity, gate: 0 };
        if (column) app.setColumnCell(pat.steps[capture.track], row, column, values);
        else pat.steps[capture.track][row] = { ...(app.isColumnHolder(step) ? { columns: step.columns } : step), ...values, chord: undefined };
      } else {
        pat.steps[capture.track][row] = {
          ...step,
          note: root,
          ...stepInstrument(capture.track, capture.instrument),
          velocity: capture.velocity,
          gate: 0,
          chord: intervals.length > 1 ? intervals : undefined
        };
      }
      lineage.visibleRow = (row + (capture.holdCursor ? 0 : lineage.editStep)) % pat.length;
      if (!lineage.detached) state.row = lineage.visibleRow;
    }, { render: !lineage.detached })) return false;
    if (lineage.detached) paintRecordedCell(pat, capture.track, row);
    publishInputMutation(capture.project, priorGeneration);
    lineage.writes.set(row, {
      ref: pat.steps[capture.track][row],
      value: JSON.stringify(pat.steps[capture.track][row])
    });
    return true;
  }

  Object.assign(app, {
    finishStoppedGridItem, queueStoppedGridKey, detachStoppedInputCursor, applyStoppedInput
  });
})();
