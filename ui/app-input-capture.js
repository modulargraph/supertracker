/* Input capture: notes typed or played while recording are captured against the cursor
   and acknowledged by the engine before they become one undoable edit. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clone, clamp, state, pattern, toast, request, stableJSON } = app;
  // Only input edges the host acknowledged place notes. The ledger is bounded by host
  // replies/timeouts; it never waits for a held key.
  app.inputOwnerSerial = 0;
  app.inputCaptureGeneration = 0;
  app.inputArmGeneration = 0;
  app.inputEditGeneration = 0;
  app.inputCaptureTail = Promise.resolve();
  app.inputCursorLineage = null;
  app.stoppedGridScope = null;
  const pendingInputEdges = new Set();
  const inputCaptures = new Set();
  const midiInputCaptures = new Map();
  const midiInputEdges = new Map();
  const inputCaptureWarnings = new WeakMap();
  const stoppedGridScopes = new Set();

  // Notes sound immediately. Stopped-grid commands share the acknowledgement order,
  // so an arrow cannot outrun a pending note's insertion and edit-step advance.
  function pendingInputWork() {
    return pendingInputEdges.size > 0 || stoppedGridScopes.size > 0;
  }

  function inputRecordingContext(project) {
    const number = (value, fallback) => ['number', 'boolean'].includes(typeof value) && Number.isFinite(Number(value))
        ? Number(value)
        : fallback;
    // Compared field by field with the context in the host's acknowledgement.
    return {
      linesPerBeat: clamp(Math.trunc(number(project.linesPerBeat, 4)), 1, 32),
      swing: Math.fround(clamp(Math.fround(number(project.swing, 0)), 0, Math.fround(.95))),
      loopSong: project.loopSong === true,
      bpm: clamp(number(project.bpm, 120), 20, 999),
      patterns: project.patterns.map((pat, index) => ({
        id: Math.trunc(number(pat.id, index)),
        length: clamp(Math.trunc(number(pat.length, 64)), 1, 256)
      })),
      sequence: project.sequence.map(Math.trunc),
      clips: (project.audioClips || []).map(clip => ({
        id: String(clip.id ?? ''),
        track: Math.trunc(number(clip.track, 0)),
        startBeat: number(clip.startBeat, 0),
        durationBeats: number(clip.durationBeats, 0),
        path: String(clip.path ?? clip.samplePath ?? ''),
        sourceOffset: clamp(number(clip.sourceOffset, number(clip.offsetSeconds, 0)), 0, 1000000),
        muted: (clip.muted ?? clip.mute) === true
      }))
    };
  }

  function inputDomain(project) {
    return stableJSON([inputRecordingContext(project), project.tracks.map(t => [t.id, t.routingID]),
        project.instruments.map(i => i.id)]);
  }

  function inputCaptureCurrent(capture) {
    return state.project === capture.project
        && state.recording
        && app.inputArmGeneration === capture.armGeneration
        && inputDomain(state.project) === capture.domain
        && capture.project.tracks[capture.track] === capture.trackObject
        && capture.project.instruments[capture.instrument] === capture.instrumentObject;
  }

  function warnInputCapture(capture, message = 'Input recording position was unavailable. No step was inserted.') {
    if (state.project !== capture.project
        || inputCaptureWarnings.get(capture.project) === capture.armGeneration) return;
    inputCaptureWarnings.set(capture.project, capture.armGeneration);
    toast(message, 'info');
  }

  // MIDI notes that start together (a chord played on the keyboard, or one key expanded
  // by the performance map) join the first note's step instead of each taking the next edit row.
  const MIDI_CHORD_MS = 60;

  /* columns: the track has several note columns (app-note-columns.js), so a note pressed while another recorded note
     is still held (held) joins that note's row in its own note column, without the MIDI chord time window. */
  function captureCursor(project, track, instrument, chordInput = false, { columns = false, held = false } = {}) {
    const pat = pattern(),
      old = app.inputCursorLineage;
    if (!old
        || old.invalid
        || old.detached
        || old.project !== project
        || old.pat !== pat
        || old.track !== track
        || old.instrument !== instrument
        || old.view !== state.view
        || old.visibleRow !== state.row
        || old.column !== state.column
        || (old.noteColumn || 0) !== (state.noteColumn || 0)
        || old.editStep !== state.editStep) app.inputCursorLineage = {
      project,
      pat,
      track,
      instrument,
      view: state.view,
      visibleRow: state.row,
      nextRow: state.row,
      column: state.column,
      noteColumn: state.noteColumn || 0,
      editStep: state.editStep,
      invalid: false,
      writes: new Map()
    };
    const lineage = app.inputCursorLineage,
      now = Date.now();
    if (lineage.chord && (chordInput && now - lineage.chord.at <= MIDI_CHORD_MS || held)) return {
      lineage,
      row: lineage.chord.row,
      merged: true
    };
    const row = lineage.nextRow;
    lineage.nextRow = (row + (state.playing && state.recordPlacement !== 'live' ? 0 : state.editStep)) % pat.length;
    lineage.chord = chordInput || columns ? { row, at: now } : null;
    return { lineage, row };
  }

  function makeInputCapture(inputOwner, note, velocity, track, instrument, notes, source = 'keyboard') {
    const project = state.project;
    if (!state.recording
        || app.liveRecordingBusy()
        || !Number.isInteger(track)
        || !project.tracks[track]
        || !Number.isInteger(instrument) || !project.instruments[instrument]
        || inputCaptures.size >= 512) return null;
    let recordingContext, domain;
    try {
      recordingContext = inputRecordingContext(project);
      domain = inputDomain(project);
    } catch {
      toast('Correct the project timing before recording notes.', 'error');
      return null;
    }
    const gridScope = !state.playing
        && app.stoppedGridScope
        && !app.stoppedGridScope.invalid
        && track === app.stoppedGridScope.track
        && instrument === app.stoppedGridScope.instrument
        ? app.stoppedGridScope
        : null;
    if (gridScope) gridScope.remaining++;
    // Several note columns: a single note takes a free note column (held notes keep theirs), see app-note-columns.js.
    const columns = (app.noteColumnCount?.(track, project) || 1) > 1 && notes.length === 1,
      noteColumn = columns ? app.claimColumn(track, inputOwner, app.cursorNoteColumn(track)) : null,
      columnCursor = { columns: columns && noteColumn !== null, held: columns && noteColumn !== null && app.heldColumnCount(track) > 1 };
    const capture = {
      placement: state.recordPlacement,
      holdCursor: state.playing && state.recordPlacement !== 'live',
      inputOwner,
      project,
      note,
      velocity,
      track,
      instrument,
      notes: notes.slice(),
      source,
      countIn: !!state.countIn,
      trackObject: project.tracks[track],
      instrumentObject: project.instruments[instrument],
      armGeneration: app.inputArmGeneration,
      domain,
      recordingContext,
      gridScope,
      noteColumn,
      columnCursor,
      cursor: gridScope ? null : captureCursor(project, track, instrument, source === 'midi', columnCursor),
      record: null,
      onset: null,
      released: false,
      finished: false,
      patterns: new Map(project.patterns.map(p => [p.id, {
        pat: p,
        rows: (p.steps[track] || []).map(step => ({ ref: step, value: JSON.stringify(step) }))
      }]))
    };
    inputCaptures.add(capture);
    return capture;
  }

  function inputAckValid(capture, on, ack) {
    return !!ack
        && ack.inputOwner === capture.inputOwner
        && ack.on === on
        && ack.track === capture.track
        && ack.instrument === capture.instrument
        && ack.note === capture.note
        && typeof ack.requestID === 'string'
        && /^[1-9][0-9]*$/.test(ack.requestID)
        && BigInt(ack.requestID) <= 18446744073709551615n
        && ack.status === 'applied'
        && ack.applied === true
        && ack.recording === true
        && typeof ack.playing === 'boolean';
  }

  function inputTarget(capture, ack) {
    const saved = capture.patterns.get(ack.pattern),
      pat = saved?.pat,
      row = ack.row;
    if (!pat
        || capture.project.patterns.filter(p => p.id === ack.pattern).length !== 1
        || !capture.project.patterns.includes(pat)
        || !Number.isInteger(row)
        || row < 0
        || row >= pat.length
        || !pat.steps[capture.track]
        || !Number.isInteger(ack.sequence)
        || ack.sequence < 0) return null;
    const before = saved.rows[row],
      current = pat.steps[capture.track][row];
    // A note column: only that column must be unchanged (other columns' recordings may have written the row).
    if (capture.noteColumn != null) {
      const column = capture.noteColumn, group = app.liveRecordGroups.get(pat.id + ':' + capture.track + ':' + row + ':' + column),
        owned = group?.project === capture.project && app.noteCell(current, column) === group.step
            && group.inputWriteValue === app.cellValue(current, column);
      if (!before || !owned && app.cellValue(JSON.parse(before.value ?? 'null'), column) !== app.cellValue(current, column)) return null;
      return { pat, row };
    }
    const group = app.liveRecordGroups.get(pat.id + ':' + capture.track + ':' + row);
    // Earlier acknowledged edges may have written this row since capture began.
    // Accept those exact writes, including a finalized retrigger, but not user edits.
    const ownedGroup = group?.project === capture.project
        && group.step === current
        && group.inputWriteValue === JSON.stringify(current);
    if (!before || !ownedGroup && (before.ref !== current || before.value !== JSON.stringify(current))) return null;
    return { pat, row };
  }

  function deliverInputEdge(capture, on, ack) {
    if (!inputCaptureCurrent(capture) || app.liveRecordingBusy()) return;
    if (on) {
      if (!inputAckValid(capture, true, ack)
          || stableJSON(ack.recordingContext) !== stableJSON(capture.recordingContext)) {
        if (capture.cursor) capture.cursor.lineage.invalid = true;
        if (capture.gridScope) capture.gridScope.invalid = true;
        warnInputCapture(capture);
        return;
      }
      capture.onset = ack;
      // Notes played during a count-in sound but are not step-entered; after the downbeat they record live.
      if (!ack.playing && capture.countIn) return;
      if (!ack.playing || capture.placement !== 'live') {
        if (!app.applyStoppedInput(capture)) {
          if (capture.cursor) capture.cursor.lineage.invalid = true;
          if (capture.gridScope) capture.gridScope.invalid = true;
          warnInputCapture(capture);
        }
        return;
      }
      if (capture.cursor) capture.cursor.lineage.invalid = true;
      if (capture.gridScope) capture.gridScope.invalid = true;
      const target = inputTarget(capture, ack);
      if (!target) {
        warnInputCapture(capture, 'The input recording target changed. No step was inserted.');
        return;
      }
      const root = Math.min(...capture.notes);
      capture.record = app.recordLiveStep(root, capture.velocity, capture.track, capture.instrument, target.pat.id, target.row, capture.notes.map(note => note - root), ack.recordingClock, {
        noteColumn: capture.noteColumn
      });
    } else if (capture.record) {
      const valid = inputAckValid(capture, false, ack)
          && ack.playing
          && capture.onset?.playing
          && BigInt(ack.requestID) > BigInt(capture.onset.requestID)
          && stableJSON(ack.recordingContext) === stableJSON(capture.recordingContext)
          && app.exactRecordingClock(ack.recordingClock);
      app.finishLiveRecord(capture.record, valid ? ack.recordingClock : undefined);
    }
  }

  function queueInputEdge(capture, on) {
    let settle;
    const reply = new Promise(resolve => {
        settle = resolve;
      }),
      previous = app.inputCaptureTail;
    const edge = {
      capture,
      on,
      settled: false,
      promise: null,
      resolve(ack) {
        if (edge.settled) return;
        edge.settled = true;
        clearTimeout(edge.timer);
        settle(ack);
      }
    };
    edge.timer = setTimeout(() => edge.resolve(null), 1500);
    pendingInputEdges.add(edge);
    app.inputCaptureGeneration++;
    edge.promise = previous.then(() => reply).then(ack => deliverInputEdge(capture, on, ack)).catch(error => {
      warnInputCapture(capture, error.message || 'The input recording edge could not be applied.');
    }).finally(() => {
      pendingInputEdges.delete(edge);
      if (on && capture.gridScope) app.finishStoppedGridItem(capture.gridScope);
      if (!on) {
        if (capture.record) capture.record.group.active.delete(capture.record.token);
        app.releaseColumn?.(capture.track, capture.inputOwner);
        capture.finished = true;
        inputCaptures.delete(capture);
        if (midiInputCaptures.get(capture.inputOwner) === capture) midiInputCaptures.delete(capture.inputOwner);
      }
      if (midiInputEdges.get(capture.inputOwner + ':' + on) === edge) midiInputEdges.delete(capture.inputOwner + ':' + on);
    });
    app.inputCaptureTail = edge.promise;
    return edge;
  }

  function requestInputEdge(capture, on, applyNote) {
    const edge = queueInputEdge(capture, on),
      payload = {
        track: capture.track,
        instrument: capture.instrument,
        note: capture.note,
        ...(on ? { velocity: capture.velocity } : {}),
        captureInput: true,
        inputOwner: capture.inputOwner,
        ...(!applyNote ? { applyNote: false } : {})
      };
    request(on ? 'note.on' : 'note.off',
        payload).then(response => edge.resolve(response?.inputAcknowledgement), error => {
      edge.resolve(null);
      toast(error.message, 'error');
    });
    return edge;
  }

  function flushInputCaptures() {
    const watermark = app.inputCaptureTail;
    return watermark.then(() => true);
  }

  function discardInputCaptures() {
    app.inputCursorLineage = null;
    app.clearHeldColumns?.();
    for (const scope of stoppedGridScopes) scope.invalid = true;
    stoppedGridScopes.clear();
    app.stoppedGridScope = null;
    for (const edge of pendingInputEdges) edge.resolve(null);
    for (const capture of inputCaptures) {
      if (capture.record) capture.record.group.active.delete(capture.record.token);
      capture.finished = true;
      if (capture.cursor) capture.cursor.lineage.invalid = true;
    }
    inputCaptures.clear();
    midiInputCaptures.clear();
    midiInputEdges.clear();
  }

  function rebindSavedInputCaptures(previous, saved) {
    if (inputDomain(previous) !== inputDomain(saved)) return;
    const rebound = new Set();
    for (const capture of inputCaptures) {
      if (capture.project !== previous || capture.domain !== inputDomain(previous)) continue;
      const track = saved.tracks[capture.track],
        instrument = saved.instruments[capture.instrument];
      if (JSON.stringify(track) !== JSON.stringify(capture.trackObject)
          || JSON.stringify(instrument) !== JSON.stringify(capture.instrumentObject)) continue;
      const oldGroup = capture.record?.group,
        newPat = oldGroup && saved.patterns.find(p => p.id === oldGroup.pat.id),
        newCell = newPat?.steps[capture.track]?.[oldGroup?.row],
        newStep = oldGroup?.column != null ? app.noteCell(newCell, oldGroup.column) : newCell;
      if (oldGroup && (oldGroup.column != null ? app.cellValue(newCell, oldGroup.column) !== app.liveGroupValue(oldGroup)
          : JSON.stringify(newStep) !== JSON.stringify(oldGroup.step))) continue;
      capture.project = saved;
      capture.trackObject = track;
      capture.instrumentObject = instrument;
      capture.patterns = new Map(saved.patterns.map(p => [p.id, {
        pat: p,
        rows: (p.steps[capture.track] || []).map(step => ({ ref: step, value: JSON.stringify(step) }))
      }]));
      const lineage = capture.cursor?.lineage;
      if (lineage) {
        lineage.project = saved;
        lineage.pat = saved.patterns.find(p => p.id === lineage.pat.id) || lineage.pat;
      }
      if (oldGroup && !rebound.has(oldGroup)) {
        oldGroup.project = saved;
        oldGroup.pat = newPat;
        oldGroup.step = newStep;
        rebound.add(oldGroup);
      }
    }
  }

  function receiveAcknowledgedMIDI(data) {
    const token = data.inputOwner;
    if (typeof token !== 'string' || !token || token.length > 256) return;
    const on = data.on !== false;
    if (data.recordingCancelled === true && data.recordingOnly === true && !on) {
      const capture = midiInputCaptures.get(token);
      if (capture && !capture.released) {
        capture.released = true;
        queueInputEdge(capture, false).resolve(null);
      }
      return;
    }
    if (data.recordingOnly === true) {
      const edge = midiInputEdges.get(token + ':' + on);
      edge?.resolve(data.inputAcknowledgement);
      return;
    }
    if (data.recordingPending !== true) return;
    if (on) {
      if (midiInputCaptures.has(token)) return;
      const capture = makeInputCapture(token, data.note, data.velocity || 100, data.track,
          data.owner || data.instrument, [data.note], 'midi');
      if (!capture) return;
      midiInputCaptures.set(token, capture);
      midiInputEdges.set(token + ':true', queueInputEdge(capture, true));
    } else {
      const capture = midiInputCaptures.get(token);
      if (!capture || capture.released) return;
      capture.released = true;
      midiInputEdges.set(token + ':false', queueInputEdge(capture, false));
    }
  }

  // Input edges must reach the engine before anything else settles; a flush records whether there were any.
  window.STPendingEdits.register('inputCapture', {
    order: 10,
    hasPending: () => pendingInputWork(),
    generation: () => app.inputCaptureGeneration,
    flush: context => {
      context.hadInput = pendingInputWork();
      if (context.hadInput) return app.inputCaptureTail.then(() => undefined);
    }
  });

  window.STInputCapture = {
    flushEdits: flushInputCaptures,
    recordingContext: () => clone(inputRecordingContext(state.project)),
    get hasPendingEdits() {
      return pendingInputWork();
    },
    get editGeneration() {
      return app.inputCaptureGeneration;
    }
  };

  Object.assign(app, {
    pendingInputEdges, stoppedGridScopes, pendingInputWork, inputDomain,
    warnInputCapture, MIDI_CHORD_MS, captureCursor, makeInputCapture, requestInputEdge, discardInputCaptures,
    rebindSavedInputCaptures, receiveAcknowledgedMIDI
  });
})();
