/* Pattern editing: step helpers, row insert/delete, transpose, cursor movement, step
   audition, pattern loops and the pattern-tools commands. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, clone, clamp, state, pattern, beatsPerBar, toast, request, mutate, setView, refresh, updateTransport,
    patternColumns
  } = app;
  let stepPreviewActive = false;
  let stepPreviewGeneration = 0;

  // Pattern loop range: transient (not saved), owned by one pattern, honoured by the engine during Pattern playback.
  function patternLoopFor(pat = pattern()) {
    const loop = state.patternLoop;
    if (!loop || !pat || loop.pattern !== pat.id || loop.start >= pat.length) return null;
    return { start: loop.start, end: Math.min(loop.end, pat.length - 1) };
  }

  function patternLoopButton() {
    const loop = patternLoopFor();
    return loop ? `<button id="pattern-loop" class="active" aria-pressed="true" title="Looping rows ${loop.start}–${loop.end}`
        + ` in Pattern playback · ⌥⇧L stops looping">⟲ Loop ${String(loop.start).padStart(2, '0')}–${String(loop.end).padStart(2, '0')}</button>` : `<button id="pattern-loop" class="quiet" aria-pressed="false" title="Loop the selected rows (or the current `
        + `bar) during Pattern playback · ⌥⇧L">⟲ Loop selection</button>`;
  }

  function sendPatternLoop() {
    if (!state.native || !state.playing || state.playbackMode !== 'pattern') return Promise.resolve();
    return request('transport.loop', { loop: patternLoopFor() }).catch(e => toast(e.message, 'error'));
  }

  function loopSelection(replace = false) {
    const pat = pattern();
    if (!pat) return false;
    if (patternLoopFor(pat) && !replace) {
      state.patternLoop = null;
      void sendPatternLoop();
      toast('Pattern loop off · playback continues through the whole pattern.');
      refresh();
      return false;
    }
    const bounds = app.selectedBounds();
    let start, end;
    if (bounds) {
      start = bounds.r0;
      end = bounds.r1;
    } else {
      const bar = Math.max(1, Math.round(state.project.linesPerBeat * beatsPerBar(state.project)));
      start = Math.floor(state.row / bar) * bar;
      end = Math.min(pat.length - 1, start + bar - 1);
    }
    state.patternLoop = { pattern: pat.id, start, end };
    let note = '';
    if (state.playbackMode !== 'pattern') {
      if (!state.playing && !app.pendingPlayback && !app.stoppingPlayback) {
        state.playbackMode = 'pattern';
        note = ' Playback mode set to Pattern.';
      } else {
        note = ' The loop applies to Pattern playback; this Song playback is unchanged.';
      }
    }
    void sendPatternLoop();
    toast(`Looping rows ${start}–${end}${bounds ? '' : ' (current bar)'}.${note} ⌥⇧L stops looping.`);
    refresh();
    return true;
  }

  function clearPatternLoop() {
    if (!state.patternLoop) return;
    state.patternLoop = null;
    void sendPatternLoop();
  }

  function ensureStep(note = 60) {
    const pat = pattern();
    if (!pat.steps[state.track][state.row]) pat.steps[state.track][state.row] = {
      note,
      ...stepInstrument(state.track),
      velocity: 100,
      gate: 0
    };
    return pat.steps[state.track][state.row];
  }

  // The cell under the cursor's note column, created when missing (app-note-columns.js): the step for column 0.
  function ensureCell(note = 60) {
    const n = app.cursorNoteColumn?.() || 0;
    if (!n) return ensureStep(note);
    const steps = pattern().steps[state.track], existing = app.noteCell(steps[state.row], n);
    if (existing) return existing;
    return app.noteCell(app.setColumnCell(steps, state.row, n, { note, ...stepInstrument(state.track), velocity: 100, gate: 0 }), n);
  }

  /* Gate column: a per-project view preference (see app.gateColumnShown). Like group collapse it is saved with
     the session, written into the Undo/Redo snapshots so Undo never flips the view, and adds no Undo step. */
  function setGateColumn(shown) {
    shown = !!shown;
    if (app.gateColumnShown() === shown) return false;
    for (const p of [state.project, ...state.undo.map(x => x.project), ...state.redo.map(x => x.project)]) {
      if (!p) continue;
      if (shown) p.viewState = { ...(p.viewState || {}), gateColumn: true };
      else if (p.viewState) {
        delete p.viewState.gateColumn;
        if (!Object.keys(p.viewState).length) delete p.viewState;
      }
    }
    state.dirty = true;
    updateTransport();
    app.scheduleAutosave?.();
    if (state.view === 'pattern') refresh();
    return true;
  }

  function toggleGateColumn() {
    setGateColumn(!app.gateColumnShown());
    toast(app.gateColumnShown()
        ? 'Gate column shown: notes with a gate stop after that many rows; live recording measures held notes.'
        : 'Gate column hidden: notes play until the next note, OFF or CUT on their track.');
    return true;
  }

  // Remove gates from the selected notes (or the cursor step): they then play until the next note, OFF or CUT.
  function removeGates() {
    const b = app.selectedBounds() || app.cursorBounds(), pat = pattern();
    let count = 0;
    for (let t = b.t0; t <= b.t1; t++) for (let r = b.r0; r <= b.r1; r++) if (app.stepGate(pat.steps[t]?.[r]) !== null) count++;
    if (!count) return toast('No gated notes in the selection.');
    return mutate('remove gates', () => {
      for (let t = b.t0; t <= b.t1; t++) for (let r = b.r0; r <= b.r1; r++) {
        const step = pat.steps[t]?.[r];
        if (app.stepGate(step) !== null) step.gate = 0;
      }
    });
  }

  // OFF (note-off: the track's notes release with their envelope) or CUT (immediate, with a short declick) at the
  // cursor; step effects stay. Same as 1 / ⇧1 in the note column.
  function insertNoteOff(cut = false) {
    if (state.view !== 'pattern') setView('pattern');
    return mutate(cut ? 'note cut' : 'note off', () => {
      const pat = pattern(), selected = pat.steps[state.track][state.row], n = app.cursorNoteColumn?.() || 0;
      // In an extra note column the OFF / CUT ends only that column's notes.
      if (n) app.setColumnCell(pat.steps[state.track], state.row, n, { note: cut ? -2 : -1 });
      else pat.steps[state.track][state.row] = {
        ...app.preservedStepFX(selected), note: cut ? -2 : -1, ...stepInstrument(), velocity: 100
      };
      state.row = (state.row + state.editStep) % pat.length;
    });
  }

  function stepInstrument(track = state.track, instrument = state.instrument) {
    return { instrument };
  }

  function shiftPatternRows(insert) {
    // Every track of the selection (a collapsed group: all of its tracks) shifts from the cursor row.
    const b = app.selectedBounds() || app.cursorBounds();
    mutate(insert ? 'insert row' : 'remove row', () => {
      const pat = pattern();
      for (let t = b.t0; t <= b.t1; t++) {
        const steps = pat.steps[t];
        if (insert) {
          steps.splice(state.row, 0, null);
          steps.length = pat.length;
        } else {
          steps.splice(state.row, 1);
          steps.push(null);
        }
      }
    });
  }

  function auditionStep(step = pattern().steps[state.track]?.[state.row], track = state.track) {
    window.STSampler?.stopAudition?.();
    if (!step || !Number.isFinite(step.note) || step.note < 0) return;
    const instrument = step.instrument ?? state.project.tracks[track]?.instrument ?? state.instrument;
    if (state.project.instruments[instrument]?.type === 'au'
        && state.project.tracks[track]?.instrument !== instrument) {
      toast('Assign this plug-in to the selected track before auditioning it.');
      return;
    }
    state.lastInputNote = step.note;
    updateTransport();
    const generation = ++stepPreviewGeneration;
    stepPreviewActive = true;
    request('step.preview', {
      track,
      instrument,
      step: clone(step),
      duration: .35
    }).catch(e => {
      if (generation === stepPreviewGeneration) {
        stepPreviewActive = false;
        toast(e.message, 'error');
      }
    });
  }

  function stopStepPreview() {
    if (!stepPreviewActive) return;
    stepPreviewActive = false;
    ++stepPreviewGeneration;
    request('step.preview', { track: state.track, step: { note: -1 }, duration: .03 }).catch(() => {});
  }

  // A selection transposes every note column of its steps; the cursor alone transposes the note column it is on.
  function transpose(amount) {
    const targets = [], cursorOnly = !state.selection;
    app.eachSelected((a, r, t) => {
      const step = a[r];
      for (const { cell, column } of app.playedCells(step)) {
        if (cursorOnly && column !== app.cursorNoteColumn(t)) continue;
        if (clamp(cell.note + amount, 0, 127) !== cell.note) targets.push({ hit: cell, step, column, track: t });
      }
    });
    if (!targets.length) return;
    if (!mutate('transpose', () => targets.forEach(({ hit }) => hit.note = clamp(hit.note + amount, 0, 127)))) return;
    const focused = app.cellStep(pattern().steps[state.track]?.[state.row], app.cursorNoteColumn());
    if (focused?.note >= 0) {
      auditionStep(focused);
    } else {
      auditionStep(app.cellStep(targets[0].step, targets[0].column), targets[0].track);
    }
  }

  function moveGrid(dt, dr, dc = 0, select = false) {
    const columns = patternColumns(),
      here = columns[app.columnFor(columns)];
    // A collapsed group or a collapsed (slim) track has no sub-columns: Left/Right move a whole column at a time.
    if (here?.collapsed || here?.slim) {
      dt += dc;
      dc = 0;
      state.column = 0;
      state.noteColumn = 0;
    }
    if (dt || dc) app.detachStoppedInputCursor();
    if (select && !state.selection) state.selection = { track: state.track, row: state.row };
    if (!select) state.selection = null;
    // Left / Right walk NOTE INS VEL of each note column, then FX 1 and FX 2 (app-note-columns.js).
    let entering = 0;
    if (dc) {
      entering = app.stepCursor ? app.stepCursor(dc) : 0;
      dt += entering;
    }
    if (dt) {
      const current = app.columnFor(columns),
        next = columns[clamp((current < 0 ? 0 : current) + dt, 0, columns.length - 1)];
      state.track = next.collapsed && next.members.includes(state.track) ? state.track : next.index;
      if (next.collapsed || next.slim) {
        state.column = 0;
        state.noteColumn = 0;
      } else if (entering < 0) {
        // Entering a track from the right lands on its FX 2, from the left on column 0's NOTE (Tab keeps the sub-column).
        state.noteColumn = 0;
        state.column = 4;
      } else if (entering > 0) {
        state.noteColumn = 0;
        state.column = 0;
      } else {
        state.noteColumn = Math.min(state.noteColumn || 0, (app.noteColumnCount?.(state.track) || 1) - 1);
      }
    }
    state.row = clamp(state.row + dr, 0, pattern().length - 1);
    state.instrument = state.project.tracks[state.track].instrument || 0;
    refresh();
    const after = patternColumns(),
      shown = after[app.columnFor(after)];
    $(`[data-cell="${shown?.index ?? state.track}:${state.row}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    $('#tracker')?.focus({ preventScroll: true });
  }

  // Pattern tools live in pattern-tools.js; they edit the visible pattern, so bring it forward first.
  function patternTool(command) {
    const tools = window.STPatternTools;
    if (!tools) return;
    if (command === 'muteTrack') return tools.toggleTrack('mute');
    if (command === 'soloTrack') return tools.toggleTrack('solo');
    if (command === 'clearMuteSolo') return tools.clearMuteSolo();
    if (command === 'automateParameter') return tools.automatePicker();
    if (state.view !== 'pattern') setView('pattern');
    if (command.startsWith('quickFX:')) {
      const [, type, value] = command.split(':');
      return tools.applyEffect(type, Number(value));
    }
    switch (command) {
      case 'interpolate':
        return tools.interpolate();
      case 'humanize':
        return tools.humanize();
      case 'randomizeVelocity':
        return tools.randomizeVelocity();
      case 'louder':
        return tools.scaleVelocity(1.1);
      case 'softer':
        return tools.scaleVelocity(1 / 1.1);
      case 'reverseSteps':
        return tools.reverse();
      case 'nudgeUp':
        return tools.nudge(-1);
      case 'nudgeDown':
        return tools.nudge(1);
      case 'stretch':
        return tools.stretch(2);
      case 'compress':
        return tools.stretch(.5);
      case 'doublePattern':
        return tools.resizePattern(2);
      case 'halvePattern':
        return tools.resizePattern(.5);
      case 'clearEffects':
        return tools.clearEffects();
      case 'stepEffect':
        return window.STStepFX?.show({ fx: state.column === 4 ? 2 : 1 });
    }
  }

  Object.assign(app, {
    patternLoopFor, patternLoopButton, loopSelection, clearPatternLoop, ensureStep, ensureCell, stepInstrument,
    setGateColumn, toggleGateColumn, removeGates, insertNoteOff, shiftPatternRows, auditionStep, stopStepPreview, transpose, moveGrid, patternTool
  });
})();
