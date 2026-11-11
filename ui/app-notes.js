/* Notes: play, hold and release notes from the computer keyboard and pointer. While recording, a note is
   an input capture (app-input-capture.js): the host's acknowledgement enters it into the pattern. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $$, clamp, state, toast, request, updateTransport, stopStepPreview, performanceVoicing, chordChipHighlight
  } = app;
  const held = new Map();

  /* The interface's own sustain pedal (the MK3's pedal jack). The host does the sustaining: an instrument with a
     pedal of its own gets the pedal, and for the engine's instruments the host holds the note-off. Here it only
     matters for recording: a recorded note is as long as it sounded, so the capture of a key released under the
     pedal stays open, and gets its release when the pedal comes up or the same note is struck again. */
  let pedalDown = false;
  const sustainedCaptures = new Map();

  function endSustained(id) {
    const capture = sustainedCaptures.get(id);
    if (!capture) return;
    sustainedCaptures.delete(id);
    if (capture.finished) return;
    capture.released = true;
    // The sound is the pedal's business: this edge only marks where the note ends.
    app.requestInputEdge(capture, false, false);
  }

  function sustainCapture(capture) {
    const id = capture.track + ':' + capture.note;
    endSustained(id);
    sustainedCaptures.set(id, capture);
    const ended = () => { if (sustainedCaptures.get(id) === capture) endSustained(id); };
    // The host says whether the note sounds on. If it does not (a panic forgot the pedal), it ends here.
    request('note.off', { track: capture.track, instrument: capture.instrument, note: capture.note })
        .then(response => { if (response?.sustained !== true) ended(); }, ended);
  }

  function sustainPedal(down) {
    pedalDown = !!down;
    if (!pedalDown) for (const id of [...sustainedCaptures.keys()]) endSustained(id);
  }

  function noteOn(note, key = 'pointer', record = true, instrument = state.instrument, track = state.track, performance = record, velocityOverride, repeatable = performance) {
    window.STSampler?.stopAudition?.();
    if (state.project.instruments[instrument]?.type === 'plugin'
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
    // A note that is being recorded is a capture: the host's acknowledgement says where it lands.
    const capture = record
        && state.recording
        && state.view === 'pattern'
        ? app.makeInputCapture('keyboard-' + ++app.inputOwnerSerial, ns[0], velocity, track, instrument, ns)
        : null;
    const entry = { notes: ns, track, instrument, capture, slot };
    held.set(key, entry);
    chordChipHighlight(slot);
    for (const n of ns) {
      // A note the pedal kept sounding is struck again: its recording ends here.
      endSustained(track + ':' + n);
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
    updateTransport();
  }

  function noteOff(key) {
    const entry = held.get(key);
    if (!entry) return;
    held.delete(key);
    chordChipHighlight(entry.slot);
    if (entry.repeat) {
      window.STNoteRepeat?.release(key);
      for (const n of entry.notes) pianoHighlight(n, [...held.values()].some(active => active.notes.includes(n)));
      return;
    }
    for (const n of entry.notes) {
      const owners = [...held.values()].filter(active => active.notes.includes(n));
      const release = !owners.some(active => !active.repeat && active.track === entry.track);
      if (entry.capture && n === entry.capture.note && pedalDown && release) {
        sustainCapture(entry.capture);
      } else if (entry.capture && n === entry.capture.note) {
        entry.capture.released = true;
        app.requestInputEdge(entry.capture, false, release);
      } else if (release) request('note.off', {
        track: entry.track,
        instrument: entry.instrument,
        note: n
      }).catch(() => {});
      pianoHighlight(n, owners.length > 0);
    }
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
    for (const id of [...sustainedCaptures.keys()]) endSustained(id);
  }

  function pianoHighlight(n, on) {
    for (const el of $$(`[data-piano="${n}"]`)) el.classList.toggle('held', on);
  }

  Object.assign(app, { held, noteOn, noteOff, stopHeld, sustainPedal, pianoHighlight });
})();
