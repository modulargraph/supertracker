/* Live recording: notes played during playback land on the row the host acknowledged, without a gate (they
   play until the next note, OFF or CUT). While the Gate column is shown, key release records the held length as
   the gate, measured from the engine clock of the two acknowledgements. One take is one undo step. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clamp, uid, state, toast, mutate, paintRecordedCell, stepInstrument, stableJSON } = app;
  // A take runs from the first live-recorded note until transport stop, disarm or a new session.
  let recordTake = null;

  const currentRecordTake = () => recordTake?.project === state.project
      && recordTake.session === app.recordArmSession ? recordTake : recordTake = {
    project: state.project,
    session: app.recordArmSession,
    entry: null,
    notes: 0
  };

  const endRecordTake = () => {
    recordTake = null;
  };

  const liveRecordGroups = new Map();

  const inputHitValue = step => JSON.stringify(step);

  const inexactRecordingWarnings = new WeakMap();

  // What a new column-0 note keeps of the row: its FX and its extra note columns (app-note-columns.js).
  function preservedStepFX(step) {
    return Object.fromEntries(['fx1', 'fx2', 'columns'].filter(key => step?.[key] != null).map(key => [key, step[key]]));
  }

  function liveRecordingBusy() {
    return window.STBusy.any(['lineInputs', 'kits', 'export', 'routing']);
  }

  function exactRecordingClock(value) {
    return value
        && typeof value.distanceBeats === 'number'
        && Number.isFinite(value.distanceBeats)
        && value.distanceBeats >= 0
        && typeof value.runID === 'string'
        && /^[1-9][0-9]*$/.test(value.runID)
        && BigInt(value.runID) <= 18446744073709551615n ? {
      distanceBeats: value.distanceBeats,
      runID: value.runID
    } : null;
  }

  function liveGateSignature(project, pat, step) {
    return stableJSON([project.linesPerBeat, project.swing, pat.length,
        ...['fx1', 'fx2'].map(key => ['microtiming', 'ratchet'].includes(step[key]?.type) ? step[key] : null)]);
  }

  // The spacing of a row's notes in beats and the engine clock at the note's start, for a gate measured in rows.
  function gateTiming(project, pat, row, step, clock) {
    const number = (value, fallback) => ['number', 'boolean'].includes(typeof value) && Number.isFinite(Number(value))
        ? Number(value)
        : fallback;
    const float = (value, fallback) => {
      const result = Math.fround(number(value, fallback));
      return Number.isFinite(result) ? result : fallback;
    };
    const lpb = clamp(Math.trunc(number(project.linesPerBeat, 4)), 1, 32),
      swing = clamp(float(project.swing, 0), 0, Math.fround(.95));
    let micro = 0,
      ratchet = 1;
    // Match the engine's per-voice spacing: the second FX column wins duplicates.
    for (const fx of [step.fx1, step.fx2]) {
      if (fx?.type === 'microtiming') micro = clamp(float(fx.value, 0), 0, Math.fround(.95));
      if (fx?.type === 'ratchet') ratchet = clamp(Math.trunc(float(fx.value, 1)), 1, 8);
    }
    const start = exactRecordingClock(clock);
    return {
      start,
      last: start,
      valid: !!start,
      signature: liveGateSignature(project, pat, step),
      spacing: (1 + (row % 2 ? -swing : swing)) / lpb * (1 - micro) / ratchet
    };
  }

  function advanceRecordingClock(timing, value) {
    const clock = exactRecordingClock(value);
    if (!timing.valid
        || !clock
        || clock.runID !== timing.start.runID
        || clock.distanceBeats < timing.last.distanceBeats) {
      timing.valid = false;
      return null;
    }
    timing.last = clock;
    return clock;
  }

  function warnInexactRecording(group) {
    if (inexactRecordingWarnings.get(group.project) === group.armSession) return;
    inexactRecordingWarnings.set(group.project, group.armSession);
    toast('Exact recorded-note duration was unavailable. Recorded notes were kept without a gate.', 'info');
  }

  /* Writes an acknowledged note at the pattern and row of its acknowledgement; recordingClock is the engine clock
     of that acknowledgement. Null when the place no longer exists. */
  function recordLiveStep(note, velocity, track, instrument, patternID, row, intervals = [0], recordingClock, options = {}) {
    if (liveRecordingBusy()) return null;
    const project = state.project;
    if (project.patterns.filter(p => p.id === patternID).length !== 1 || !Number.isInteger(row) || !Number.isInteger(track) || !project.tracks[track]
        || !Number.isInteger(instrument) || !project.instruments[instrument]) return null;
    const pat = project.patterns.find(p => p.id === patternID);
    if (row < 0 || row >= pat.length || !pat.steps[track]) return null;
    const r = row,
      t = track,
      cell = pat.steps[t][r],
      // A single note on a track with several note columns records into its own column (options.noteColumn, claimed by
      // the caller, app-note-columns.js): one group per column, so each held note gets its own gate.
      column = Number.isInteger(options.noteColumn) && intervals.length === 1 ? options.noteColumn : null;
    if (column != null) return recordLiveColumn(note, velocity, track, instrument, pat, r, column, recordingClock);
    const key = pat.id + ':' + t + ':' + r;
    let group = liveRecordGroups.get(key);
    if (group?.project !== project
        || group?.step !== cell
        || !group?.active.size
        || group?.inputWriteValue !== inputHitValue(group.step)) group = null;
    const previous = cell;
    const take = group?.take || currentRecordTake();
    mutate('record live note', () => {
      const notes = new Set(intervals.map(x => note + x));
      if (group) {
        notes.add(group.step.note);
        for (const offset of group.step.chord || []) notes.add(group.step.note + offset);
      }
      const root = Math.min(...notes),
        step = {
          ...preservedStepFX(previous),
          note: root,
          ...stepInstrument(track, instrument),
          velocity,
          gate: 0,
          ...(notes.size > 1 ? { chord: Array.from(notes).map(x => x - root).sort((a, b) => a - b) } : {})
        };
      pat.steps[t][r] = step;
      if (group) {
        group.step = step;
        advanceRecordingClock(group.timing, recordingClock);
      } else {
        group = {
          take,
          project,
          pat,
          track: t,
          row: r,
          step,
          armSession: app.recordArmSession,
          timing: gateTiming(project, pat, r, step, recordingClock),
          active: new Set()
        };
      }
    }, { render: false, recordTake: take });
    if (!group) return null;
    paintRecordedCell(pat, t, r);
    group.inputWriteValue = inputHitValue(group.step);
    const token = uid();
    group.active.add(token);
    liveRecordGroups.set(key, group);
    return { group, token };
  }

  // The value a live group owns: its note column's cell (column groups) or the whole step.
  const liveGroupValue = group => group.column != null ? group.column ? JSON.stringify(group.step) : app.cellValue(group.step, 0) : inputHitValue(group.step);
  const liveGroupCurrent = group => {
    const step = group.pat.steps[group.track]?.[group.row];
    return group.column != null ? app.noteCell(step, group.column) : step;
  };

  function recordLiveColumn(note, velocity, track, instrument, pat, r, column, recordingClock) {
    const project = state.project, key = pat.id + ':' + track + ':' + r + ':' + column,
      take = currentRecordTake();
    let group = null;
    mutate('record live note', () => {
      const steps = pat.steps[track], values = { note, ...stepInstrument(track, instrument), velocity, gate: 0 };
      if (column) app.setColumnCell(steps, r, column, values);
      else steps[r] = { ...preservedStepFX(steps[r]), ...values };
      const main = steps[r], cell = app.noteCell(main, column);
      group = {
        take, project, pat, track, row: r, column, step: cell, armSession: app.recordArmSession,
        timing: gateTiming(project, pat, r, main, recordingClock),
        active: new Set()
      };
    }, { render: false, recordTake: take });
    if (!group) return null;
    paintRecordedCell(pat, track, r);
    group.inputWriteValue = liveGroupValue(group);
    const token = uid();
    group.active.add(token);
    liveRecordGroups.set(key, group);
    return { group, token };
  }

  function finishLiveRecord(record, recordingClock) {
    const group = record?.group;
    if (!group || !group.active.delete(record.token)) return;
    const timing = group.timing,
      endpoint = advanceRecordingClock(timing, recordingClock);
    if (group.active.size) return;
    if (state.project !== group.project
        || !group.project.patterns.includes(group.pat)
        || liveGroupCurrent(group) !== group.step) return;
    if (liveRecordingBusy()) return;
    // With the Gate column shown the held length becomes the note's gate. Without it the note keeps no gate
    // (tracker semantics) and the release records an OFF on its row (recordReleaseOff).
    const gated = app.gateColumnShown(group.project);
    if (group.inputWriteValue !== liveGroupValue(group)) return;
    const rowStep = group.pat.steps[group.track]?.[group.row];
    if (!endpoint || liveGateSignature(group.project, group.pat, rowStep) !== timing.signature) {
      if (gated) warnInexactRecording(group);
      return;
    }
    const rows = clamp((endpoint.distanceBeats - timing.start.distanceBeats) / timing.spacing, .01, 256);
    if (!gated) {
      recordReleaseOff(group, rows);
      return;
    }
    mutate('record note duration', () => group.step.gate = Math.round(rows * 1000) / 1000, {
      render: !state.playing,
      recordTake: group.take
    });
    group.inputWriteValue = liveGroupValue(group);
  }

  /* A released key records an OFF on the row nearest the release (at least the next row), in the note's own
     column. Nothing is written when a later note in that column already starts by then (it ends the note), when
     the note was held for a whole pattern, or when the release falls past the pattern's end in Song mode. */
  function recordReleaseOff(group, rows) {
    const pat = group.pat, length = pat.length, steps = pat.steps[group.track], column = group.column || 0;
    const span = Math.max(1, Math.round(rows));
    if (!steps || span >= length) return;
    if (group.row + span >= length && state.status?.songMode) return;
    const target = (group.row + span) % length;
    for (let n = 1; n <= span; n++) {
      const cell = app.noteCell(steps[(group.row + n) % length], column);
      if (Number.isInteger(cell?.note) && cell.note !== -3) return;
    }
    mutate('record note off', () => {
      if (column) app.setColumnCell(steps, target, column, { note: -1 });
      else steps[target] = { ...preservedStepFX(steps[target]), note: -1 };
    }, { render: !state.playing, recordTake: group.take });
    paintRecordedCell(pat, group.track, target);
  }

  Object.assign(app, {
    currentRecordTake, endRecordTake, liveRecordGroups, liveGroupValue, inputHitValue, preservedStepFX, liveRecordingBusy, exactRecordingClock, recordLiveStep,
    finishLiveRecord
  });
})();
