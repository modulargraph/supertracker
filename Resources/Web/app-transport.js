/* Transport: play/stop, song and pattern playback modes, count-in, note-record arming and
   the status bar readouts. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, clone, clamp, noteText, state, pattern, toast, request, syncEngineProject, refresh
  } = app;

  function setRecordPlacement(value) {
    const next = value === 'live' ? 'live' : 'selected';
    if (next === state.recordPlacement) return;
    app.stopHeld();
    app.discardInputCaptures();
    state.recordPlacement = next;
    try {
      localStorage.setItem('st.recordPlacement', next);
    } catch {}
    updateTransport();
  }

  app.recordArmSession = 0;
  app.recordArmSupported = false;
  app.recordArmDesired = false;
  let recordArmPending = null;
  let recordArmTask = null;

  // Intent is separate from accepted capture state. A session epoch survives Save/Undo
  // object replacement, but prevents an old-session acknowledgment from arming a new song.
  function setNoteRecording(armed, options = {}) {
    app.recordArmSupported = app.recordArmSupported || state.status.recordArmSupported === true;
    if (options.newSession) {
      app.recordArmSession++;
      app.inputArmGeneration++;
      app.discardInputCaptures();
      state.recording = false;
    }
    const desired = armed === undefined ? !(recordArmTask ? app.recordArmDesired : state.recording) : !!armed;
    if (desired !== app.recordArmDesired) {
      app.inputArmGeneration++;
      if (!desired) {
        app.discardInputCaptures();
        app.endRecordTake();
      }
    }
    app.recordArmDesired = desired;
    if (!app.recordArmSupported) {
      state.recording = desired;
      if (desired) state.keyboardPlay = true;
      updateTransport();
      return Promise.resolve(desired);
    }
    recordArmPending = { session: app.recordArmSession, armed: desired };
    if (!recordArmTask) {
      recordArmTask = Promise.resolve().then(async () => {
        try {
          while (recordArmPending) {
            const job = recordArmPending;
            recordArmPending = null;
            try {
              const result = await request('transport.recordArm', { armed: job.armed });
              if (typeof result?.armed !== 'boolean') throw new Error('The audio host did not confirm the recording state.');
              if (job.session !== app.recordArmSession) continue;
              // A newer opposite intent must not briefly re-enable note insertion.
              if (app.recordArmDesired === job.armed) {
                if (result.armed !== job.armed) throw new Error('The audio host did not '
                    + (job.armed ? 'arm' : 'disarm')
                    + ' note recording.');
                state.recording = result.armed;
                app.recordArmDesired = result.armed;
                if (result.armed) state.keyboardPlay = true;
                if (recordArmPending?.session === job.session
                    && recordArmPending.armed === result.armed) recordArmPending = null;
              }
            } catch (error) {
              if (job.session === app.recordArmSession) {
                if (!recordArmPending) app.recordArmDesired = state.recording;
                toast('Could not change note recording: ' + error.message, 'error');
              }
            }
            updateTransport();
          }
          return state.recording;
        } finally {
          recordArmTask = null;
          updateTransport();
        }
      });
    }
    updateTransport();
    return recordArmTask;
  }

  function receiveNoteRecordingStatus(payload) {
    const wasSupported = app.recordArmSupported;
    app.recordArmSupported = app.recordArmSupported || payload.recordArmSupported === true;
    if (!app.recordArmSupported) return;
    if (!wasSupported && state.recording && !recordArmTask) {
      state.recording = false;
      setNoteRecording(true);
      return;
    }
    if (recordArmTask || typeof payload.noteRecordArmed !== 'boolean') return;
    // Status packets have no session identity: a delayed true cannot arm a fresh song.
    if (!payload.noteRecordArmed) {
      if (state.recording || app.recordArmDesired) {
        app.inputArmGeneration++;
        app.discardInputCaptures();
      }
      state.recording = false;
      app.recordArmDesired = false;
    } else if (app.recordArmDesired) state.recording = true;
  }

  function updateRoutingStatus() {
    const label = $('#status-message');
    if (!label) return;
    const failure = typeof state.status.routingFailure === 'string'
        ? state.status.routingFailure
        : typeof state.status.routing?.error === 'string'
        ? state.status.routing.error
        : '';
    const message = failure
        ? 'Audio paused · Routing needs repair (⌘4)'
        : recordArmTask
        ? app.recordArmDesired ? 'Arming note recording…' : 'Disarming note recording…'
        : state.recording
        ? state.playing ? 'Recording live notes' : 'Step recording armed'
        : app.pendingPlayback
        ? 'Preparing playback… Space or Escape cancels'
        : state.keyboardPlay
        ? 'Keyboard preview · ⌘R to enter notes'
        : 'Ready to make something';
    if (label.textContent !== message) label.textContent = message;
    label.title = failure;
    label.classList.toggle('routing-error', !!failure);
  }

  // Native render utilization is already a percentage, including values below 1.
  function formatNativeDSPUsage(value) {
    return 'DSP ' + Math.round(Number.isFinite(value) ? Math.max(0, value) : 0) + '%';
  }

  function updateTransport() {
    const p = state.project;
    if (!p) return;
    window.STPatternDestination?.update();
    updateRoutingStatus();
    const modeControl = $('#transport-mode');
    if (modeControl) {
      const locked = state.playing || !!app.pendingPlayback || !!app.stoppingPlayback;
      modeControl.value = state.playbackMode;
      modeControl.disabled = locked;
      modeControl.title = app.stoppingPlayback
          ? 'Wait for playback to stop before changing mode.'
          : app.pendingPlayback
          ? 'Cancel playback preparation before changing mode.'
          : state.playing
          ? 'Stop playback before changing mode.'
          : 'Song plays the arrangement from the selected sequence position. Pattern plays the selected pattern from its cursor.';
      modeControl.setAttribute('aria-label',
          locked
              ? 'Playback mode · '
                  + (app.stoppingPlayback
                      ? 'wait for stop to finish'
                      : app.pendingPlayback
                      ? 'cancel preparation to change'
                      : 'stop playback to change')
              : 'Playback mode');
    }
    if ($('#record')) {
      const button = $('#record'),
        pending = !!recordArmTask;
      button.classList.toggle('active', state.recording);
      button.setAttribute('aria-pressed', String(state.recording));
      button.setAttribute('aria-busy', String(pending));
      button.setAttribute('aria-label',
          pending
              ? app.recordArmDesired ? 'Cancel arming note recording' : 'Arm note recording'
              : state.recording
              ? 'Disarm note recording'
              : 'Arm note recording');
      button.title = pending
          ? app.recordArmDesired
              ? 'Arming note recording… · Click or ⌘R to cancel'
              : 'Disarming note recording… · Click or ⌘R to arm'
          : 'Step / live record arm · ⌘R';
    }
    if ($('#keyboard-play')) {
      $('#keyboard-play').classList.toggle('active', state.keyboardPlay);
      $('#keyboard-play').setAttribute('aria-pressed', String(state.keyboardPlay));
      $('#keyboard-play').title = state.keyboardPlay
          ? state.recording
              ? 'Keyboard notes will be recorded · ⌘P to disable'
              : 'Keyboard preview · Arm Record (⌘R) to enter notes · ⌘P to disable'
          : 'Enable computer-keyboard piano · ⌘P';
      $('#keyboard-play').textContent = '⌨ ' + (state.keyboardPlay ? 'Keys on' : 'Keys off');
    }
    if ($('#record-keyboard')) $('#record-keyboard').checked = state.keyboardPlay;
    if ($('#status-shortcuts')) $('#status-shortcuts').textContent = state.keyboardPlay
        ? `Keys · ${app.performanceSummary()}`
        : 'Space play · ⌘K commands';
    if ($('#perf-summary')) $('#perf-summary').textContent = app.performanceSummary();
    if ($('#play')) {
      const button = $('#play'),
        preparing = !!app.pendingPlayback;
      button.textContent = preparing ? '…' : state.playing ? 'Ⅱ' : '▶';
      button.setAttribute('aria-busy', String(preparing));
      button.setAttribute('aria-label',
          preparing ? 'Cancel preparing playback' : state.playing ? 'Pause playback' : 'Play');
      button.title = preparing
          ? 'Cancel preparing playback · Space'
          : state.playing
          ? 'Pause playback · Space'
          : 'Play · Space';
      button.title += ' · Play from start: ⇧Space';
      button.classList.toggle('active', state.playing);
    }
    if ($('#status-selection')) $('#status-selection').textContent = (p.tracks[state.track]?.name || '')
        + ' · '
        + noteText(state.lastInputNote);
    if ($('#project-name')) $('#project-name').textContent = p.name + (state.dirty ? ' · edited' : '');
    if ($('#position')) $('#position').textContent = String((state.status.sequence ?? state.sequence) + 1).padStart(2,
        '0')
        + ' : '
        + String(state.playing ? state.status.row ?? 0 : state.row).padStart(2, '0');
    if ($('#record-playback')) $('#record-playback').textContent = state.playing ? 'Pause playback' : 'Play selection';
    if ($('#cpu') && state.native) $('#cpu').textContent = formatNativeDSPUsage(state.status.cpu);
  }

  app.pendingPlayback = null;
  app.stoppingPlayback = null;
  app.queuedPlayback = null;

  /* Song block that holds `id`: the current block when it does, else the next occurrence, else the first; -1 if none. */
  function songIndexOf(project, id, current) {
    if (project.sequence[current] === id) return current;
    const after = project.sequence.findIndex((entry, i) => i > current && entry === id);
    return after >= 0 ? after : project.sequence.indexOf(id);
  }

  // Switching keeps the music where it is: Song to Pattern selects the song's current pattern,
  // Pattern to Song moves the song position to that pattern's block. While playing, the switch
  // is queued natively and happens at the end of the playing pattern (Pattern mode then loops it;
  // Song mode continues with the block after it).
  function setPlaybackMode(mode) {
    if (!['song', 'pattern'].includes(mode)) {
      updateTransport();
      return false;
    }
    if (app.pendingPlayback || app.stoppingPlayback) {
      toast(app.stoppingPlayback
          ? 'Wait for playback to stop before changing mode.'
          : 'Cancel playback preparation before changing mode.');
      updateTransport();
      return false;
    }
    if (mode === state.playbackMode) {
      updateTransport();
      return true;
    }
    const project = state.project, last = project.sequence.length - 1;
    const playingPattern = state.playing && project.patterns.some(p => p.id === state.status.pattern) ? state.status.pattern : null;
    const songPosition = clamp(Math.floor(state.playing ? state.status.sequence ?? state.sequence : state.sequence) || 0, 0, Math.max(0, last));
    let queue = null;
    if (mode === 'pattern') {
      const id = playingPattern ?? project.sequence[songPosition];
      if (project.patterns.some(p => p.id === id) && id !== project.activePattern) app.selectPattern(id);
      if (state.playing && project.patterns.some(p => p.id === id)) queue = { mode, pattern: id };
    } else {
      const id = playingPattern ?? project.activePattern, index = songIndexOf(project, id, songPosition);
      if (index >= 0) state.sequence = index;
      // The song continues after the block that matches the playing pattern (or from the song position).
      if (state.playing && last >= 0) queue = { mode, sequence: index >= 0 ? Math.min(index + 1, last) : songPosition };
    }
    state.playbackMode = mode;
    updateTransport();
    if (queue) request('transport.queue', queue).catch(error => toast(error.message, 'error'));
    return true;
  }

  function requestPlaybackStop(panic = false) {
    if (app.stoppingPlayback) {
      app.stoppingPlayback.panic ||= panic;
      return app.stoppingPlayback.promise;
    }
    const stopping = { project: state.project, panic };
    app.stoppingPlayback = stopping;
    updateTransport();
    stopping.promise = (async () => {
      await request('transport.stop');
      if (stopping.project !== state.project) return false;
      if (stopping.panic) await request('engine.panic');
      if (stopping.project !== state.project) return false;
      state.playing = false;
      if (stopping.panic) {
        state.status.row = 0;
        state.row = 0;
        refresh();
        app.updatePlayhead({ playing: false, row: -1 });
      }
      return true;
    })().finally(() => {
      if (app.stoppingPlayback === stopping) app.stoppingPlayback = null;
      updateTransport();
    });
    return stopping.promise;
  }

  // Play pressed while a Stop is still being confirmed (Stop then Play in quick succession, e.g. on the MK3)
  // starts once the Stop is confirmed instead of being dropped. A later Stop, a failed Stop or a project
  // change cancels it; repeated presses share it. A Play that was preparing before the Stop never restarts.
  function playAfterStop(options, selection) {
    if (app.queuedPlayback) {
      if (selection) app.queuedPlayback.selection = selection;
      return app.queuedPlayback.promise;
    }
    const stopping = app.stoppingPlayback,
      project = state.project,
      mode = state.playbackMode,
      sequence = clamp(Math.floor(state.sequence) || 0, 0, project.sequence.length - 1);
    const selected = mode === 'song' ? project.patterns.find(p => p.id === project.sequence[sequence]) : pattern();
    const queued = {
      project,
      selection: selection || {
        mode,
        sequence,
        pattern: selected,
        row: mode === 'song' ? 0 : clamp(Math.floor(state.row) || 0, 0, (selected?.length ?? 1) - 1)
      }
    };
    app.queuedPlayback = queued;
    updateTransport();
    queued.promise = (async () => {
      let stopped = false;
      try {
        stopped = await stopping.promise;
      } catch {
        stopped = false;
      }
      if (app.queuedPlayback !== queued) return false;
      app.queuedPlayback = null;
      if (stopped !== true || state.project !== queued.project || state.playing || app.pendingPlayback) {
        updateTransport();
        return false;
      }
      return togglePlay(options, queued.selection);
    })();
    return queued.promise;
  }

  function playFromStart() {
    const project = state.project,
      mode = state.playbackMode,
      sequence = mode === 'song' ? 0 : clamp(Math.floor(state.sequence) || 0, 0, project.sequence.length - 1);
    const selection = {
      mode,
      sequence,
      pattern: mode === 'song' ? project.patterns.find(p => p.id === project.sequence[0]) : pattern(),
      row: 0
    };
    if (app.stoppingPlayback) return playAfterStop(undefined, selection);
    if (state.playing || app.pendingPlayback || state.countIn) {
      app.pendingPlayback = null;
      app.queuedPlayback = null;
      state.countIn = null;
      requestPlaybackStop();
      return playAfterStop(undefined, selection);
    }
    return togglePlay(undefined, selection);
  }

  async function togglePlay(options, queuedSelection) {
    if (app.stoppingPlayback) return playAfterStop(options, queuedSelection);
    // Optional count-in (bars) is only for recordWithCountIn; a click event or other argument is ignored.
    const countIn = Number.isInteger(options?.countIn) && options.countIn > 0 ? Math.min(4, options.countIn) : 0;
    let starting;
    try {
      if (state.playing || app.pendingPlayback || state.countIn && !countIn) {
        app.pendingPlayback = null;
        state.countIn = null;
        updateTransport();
        await requestPlaybackStop();
      } else {
        const project = state.project,
          mode = queuedSelection?.mode ?? state.playbackMode,
          sequence = queuedSelection?.sequence
              ?? clamp(Math.floor(state.sequence) || 0, 0, project.sequence.length - 1);
        const selectedPattern = queuedSelection
            ? queuedSelection.pattern
            : mode === 'song'
            ? project.patterns.find(p => p.id === project.sequence[sequence])
            : pattern();
        if (!selectedPattern) throw new Error('Choose an available pattern before starting playback.');
        const position = {
          pattern: selectedPattern.id,
          row: queuedSelection
              ? queuedSelection.row
              : mode === 'song'
              ? 0
              : clamp(Math.floor(state.row) || 0, 0, selectedPattern.length - 1),
          sequence
        };
        starting = { project, mode, position, pattern: selectedPattern };
        app.pendingPlayback = starting;
        updateTransport();
        const current = () => app.pendingPlayback === starting && state.project === starting.project;
        const requireSelection = () => {
          const retainedPattern = project.patterns.find(p => p.id === position.pattern);
          if (retainedPattern !== starting.pattern
              || position.row >= retainedPattern.length
              || mode === 'song'
                  && project.sequence[position.sequence] !== position.pattern) throw new Error('The playback selection changed while preparing. Press Play again.');
        };
        const synced = await syncEngineProject(() => current() ? clone(starting.project) : null);
        if (synced === false || !current()) return false;
        requireSelection();
        await request('transport.seek', position);
        if (!current()) return false;
        requireSelection();
        starting.playRequested = true;
        const loop = mode === 'pattern' ? app.patternLoopFor(selectedPattern) : null;
        await request('transport.play', {
          mode,
          ...position,
          ...(loop ? { loop } : {}),
          ...(countIn ? { countIn } : {})
        });
        if (!current()) return false;
        state.playing = state.native && !countIn;
      }
      updateTransport();
      return true;
    } catch (e) {
      if (starting && (app.pendingPlayback !== starting || state.project !== starting.project)) return false;
      let message = e.message;
      // A rejected/timed-out Play may still be preparing natively. Invalidate that
      // owned request before admitting another start; a superseded request cannot stop its successor.
      if (starting?.playRequested && app.pendingPlayback === starting && state.project === starting.project) {
        try {
          await requestPlaybackStop();
        } catch (cancelError) {
          message += ' Playback cancellation could not be confirmed: ' + cancelError.message + '. Press Stop to retry.';
        }
      }
      if (starting && (app.pendingPlayback !== starting || state.project !== starting.project)) return false;
      toast(message, 'error');
      return false;
    } finally {
      if (app.pendingPlayback === starting) app.pendingPlayback = null;
      updateTransport();
    }
  }

  async function stop() {
    app.endRecordTake();
    app.pendingPlayback = null;
    app.queuedPlayback = null;
    state.countIn = null;
    updateTransport();
    app.stopHeld();
    for (const record of app.midiRecordNotes.values()) app.finishLiveRecord(record);
    app.midiRecordNotes.clear();
    try {
      return await requestPlaybackStop(true);
    } catch (e) {
      toast('Playback stop could not be confirmed: ' + e.message + '. Press Stop to retry.', 'error');
      return false;
    }
  }

  // Count-in recording: arm note recording, then let the engine click one bar (or `bars`) before Play starts
  // sample-accurately. Notes played during the count-in sound but are not recorded; acknowledged input after
  // the downbeat records live. While playing it simply arms recording.
  async function recordWithCountIn(bars = 1) {
    if (state.playing) return setNoteRecording(true);
    if (app.pendingPlayback || app.stoppingPlayback || state.countIn) return false;
    const armed = await setNoteRecording(true);
    if (!armed || state.playing || app.pendingPlayback || app.stoppingPlayback) return false;
    const count = { bars: clamp(Math.floor(bars) || 1, 1, 4), at: Date.now(), seen: false };
    state.countIn = count;
    updateTransport();
    const started = await togglePlay({ countIn: count.bars });
    if (state.countIn === count && (!started || !state.native)) state.countIn = null;
    updateTransport();
    return started;
  }

  function receiveCountInStatus(payload) {
    const count = state.countIn;
    if (!count) return;
    if (Number(payload.countIn) > 0) {
      count.seen = true;
      count.remaining = Number(payload.countIn);
      return;
    }
    // Play has started (or the count-in was cancelled natively); a status from before the request is ignored.
    if (payload.playing || count.seen || Date.now() - count.at > 3000) state.countIn = null;
  }

  Object.assign(app, {
    setRecordPlacement, setNoteRecording, receiveNoteRecordingStatus, formatNativeDSPUsage, updateTransport,
    setPlaybackMode, playFromStart, togglePlay, stop, recordWithCountIn, receiveCountInStatus
  });
})();
