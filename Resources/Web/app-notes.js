/* Notes: play, hold and release notes from the computer keyboard, pointer and MIDI, and enter
   them into the pattern while recording. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, state, pattern, toast, request, mutate, updateTransport, paintRecordedCell, stepInstrument,
    stopStepPreview, performanceVoicing, chordChipHighlight
  } = app;
  const held = new Map();

  // The row of the last note recorded into a note column: a note pressed while it is held joins that row.
  let columnRow = null;

  function insertNote(note, velocity = 100, intervals = [0], column = null) {
    const live = state.playing && state.recordPlacement === 'live',
      joins = column != null && app.heldColumnCount(state.track) > 1 && columnRow?.track === state.track && columnRow.pat === pattern(),
      recordRow = joins ? columnRow.row : live ? state.status.row ?? state.row : state.row;
    if (column != null) mutate('record note', () => {
      // Several note columns (app-note-columns.js): the note takes its own column on the row.
      const pat = live ? state.project.patterns.find(p => p.id === state.status.pattern) || pattern() : pattern(),
        r = clamp(recordRow, 0, pat.length - 1), steps = pat.steps[state.track],
        values = { note, ...stepInstrument(), velocity, gate: 0 };
      if (column) app.setColumnCell(steps, r, column, values);
      else steps[r] = { ...app.preservedStepFX(steps[r]), ...values };
      columnRow = { track: state.track, row: r, pat };
      if (!state.playing) state.row = (r + state.editStep) % pat.length;
    }, { render: !state.playing, recordTake: live ? app.currentRecordTake() : undefined });
    else mutate('record note', () => {
      const pat = live ? state.project.patterns.find(p => p.id === state.status.pattern) || pattern() : pattern(),
        r = clamp(recordRow, 0, pat.length - 1),
        prior = pat.steps[state.track][r],
        next = {
          ...prior,
          note,
          ...stepInstrument(),
          velocity,
          gate: 0,
          chord: intervals.length > 1 ? [...intervals] : undefined
        };
      pat.steps[state.track][r] = next;
      if (!state.playing) state.row = (r + state.editStep) % pat.length;
    }, { render: !state.playing, recordTake: live ? app.currentRecordTake() : undefined });
    if (state.playing && !live) paintRecordedCell(pattern(), state.track, state.row);
    if (!state.playing) {
      $(`[data-cell="${state.track}:${state.row}"]`)?.scrollIntoView({ block: 'nearest' });
      $('#tracker')?.focus({ preventScroll: true });
    }
  }

  function noteOn(note, key = 'pointer', record = true, instrument = state.instrument, track = state.track, performance = record, velocityOverride, repeatable = performance) {
    window.STSampler?.stopAudition?.();
    if (state.project.instruments[instrument]?.type === 'au'
        && state.project.tracks[track]?.instrument !== instrument) {
      toast('Assign this plug-in to the selected track before playing or recording it.');
      return;
    }
    // Native note-off addresses a track/pitch, not an instrument or physical key.
    // Finish the old instrument's owners before changing that track's live sound.
    for (const [heldKey, active] of held) if (active.track === track && active.instrument !== instrument) noteOff(heldKey);
    if (held.has(key)) noteOff(key);
    const voiced = performance ? performanceVoicing(note) : { note, intervals: [0] },
      slot = voiced.slot;
    note = voiced.note;
    state.lastInputNote = note;
    const intervals = voiced.intervals,
      velocity = velocityOverride ?? (performance ? state.keyVelocity : 100);
    const ns = [...new Set(intervals.map(n => clamp(note + n, 0, 127)))];
    // Note repeat / arpeggio is a live-performance layer; armed recording keeps exact note entry.
    if (repeatable && window.STNoteRepeat?.enabled && !(record && state.recording)) {
      held.set(key, { notes: ns, track, instrument, repeat: true, slot });
      window.STNoteRepeat.press(key, { notes: ns, track, instrument, velocity });
      for (const n of ns) pianoHighlight(n, true);
      chordChipHighlight(slot);
      updateTransport();
      return;
    }
    const sounding = new Set([...held.values()].filter(active => !active.repeat
        && active.track === track
        && active.instrument === instrument).flatMap(active => active.notes));
    const capture = record
        && state.recording
        && state.view === 'pattern'
        && app.supportsInputAcknowledgements()
        ? app.makeInputCapture('keyboard-' + ++app.inputOwnerSerial, ns[0], velocity, track, instrument, ns)
        : null;
    const entry = { notes: ns, track, instrument, capture, slot };
    held.set(key, entry);
    chordChipHighlight(slot);
    for (const n of ns) {
      if (capture && n === capture.note) {
        app.requestInputEdge(capture, true, !sounding.has(n));
      } else if (!sounding.has(n)) request('note.on', {
        track,
        instrument,
        note: n,
        velocity
      }).catch(e => toast(e.message, 'error'));
      pianoHighlight(n, true);
    }
    if (!capture && record && state.recording && state.view === 'pattern' && !app.supportsInputAcknowledgements()) {
      // A single note on a track with several note columns takes a free column (app-note-columns.js).
      const column = (app.noteColumnCount?.(track) || 1) > 1 && intervals.length === 1 ? app.claimColumn(track, key, app.cursorNoteColumn(track)) : null;
      if (column != null) entry.columnOwner = key;
      if (state.playing && state.recordPlacement === 'live') {
        entry.record = app.recordLiveStep(note, velocity, track, instrument, state.status.pattern ?? pattern().id,
            state.status.row ?? state.row, intervals, undefined, { noteColumn: column });
      } else if (!state.countIn) insertNote(note, velocity, intervals, track === state.track ? column : null);
    }
    updateTransport();
  }

  function noteOff(key) {
    const entry = held.get(key);
    if (!entry) return;
    held.delete(key);
    if (entry.columnOwner) app.releaseColumn(entry.track, entry.columnOwner);
    chordChipHighlight(entry.slot);
    if (entry.repeat) {
      window.STNoteRepeat?.release(key);
      for (const n of entry.notes) pianoHighlight(n, [...held.values()].some(active => active.notes.includes(n)));
      return;
    }
    for (const n of entry.notes) {
      const owners = [...held.values()].filter(active => active.notes.includes(n));
      const release = !owners.some(active => !active.repeat && active.track === entry.track);
      if (entry.capture && n === entry.capture.note) {
        entry.capture.released = true;
        app.requestInputEdge(entry.capture, false, release);
      } else if (release) request('note.off', {
        track: entry.track,
        instrument: entry.instrument,
        note: n
      }).catch(() => {});
      pianoHighlight(n, owners.length > 0);
    }
    if (entry.record) app.finishLiveRecord(entry.record);
  }

  function stopHeld(owner) {
    if (typeof owner === 'string') {
      for (const [key, entry] of held) if (entry.owner === owner) noteOff(key);
      return;
    }
    window.STNoteRepeat?.stopAll?.();
    window.STClipTakes?.stopPreview?.();
    window.STSampler?.stopAudition?.();
    stopStepPreview();
    for (const key of held.keys()) noteOff(key);
  }

  function pianoHighlight(n, on) {
    for (const el of $$(`[data-piano="${n}"]`)) el.classList.toggle('held', on);
  }

  Object.assign(app, { held, noteOn, noteOff, stopHeld, pianoHighlight });
})();
