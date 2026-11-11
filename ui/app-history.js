/* Edits and history: mutate() applies one undoable project edit (after the busy gate and the
   freeze check); undo()/applyUndo() restore snapshots, waiting for native edits to settle first. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clone, clamp, state, pattern, toast } = app;
  const busy = window.STBusy;

  function mutate(label, fn, options = {}) {
    // Commits of the operation that holds the gate (and quiet native captures) pass their own gate.
    const quietCapture = options.nativeCapture === true && options.render === false;
    if (busy.refuse('edit', [
      !options.recordingCommit && !options.nativeCapture && !options.kitEdit && 'inlineRecording',
      !options.lineInputCommit && !quietCapture && 'lineInputs'
    ])) return false;
    if (!state.project) return false;
    if (busy.refuse('edit', [
      !quietCapture && 'export',
      !options.routingCommit && 'routing'
    ])) return false;
    const freezeBaseline = clone(state.project),
      cursor = {
        track: state.track,
        row: state.row,
        instrument: state.instrument,
        sequence: state.sequence,
        selection: clone(state.selection ?? null)
      };
    const rollback = () => {
      state.project = freezeBaseline;
      Object.assign(state, cursor);
      app.refresh();
    };
    let frozenBlock;
    try {
      fn(state.project);
      window.STRoutingProject.dropPagesOfRemovedPlugins(state.project);
      frozenBlock = window.STFreeze?.checkMutation(freezeBaseline, state.project, label, options);
    } catch (error) {
      rollback();
      throw error;
    }
    if (frozenBlock) {
      rollback();
      window.STFreeze.explain(frozenBlock);
      return false;
    }
    // One recording pass is one Undo step: later notes and gates of the same take extend its entry while it is still on top.
    const take = options.recordTake;
    if (take?.entry && state.undo.at(-1) === take.entry) {
      take.notes++;
    } else {
      const entry = { label: take ? 'record take' : label, project: freezeBaseline };
      state.undo.push(entry);
      if (take) {
        take.entry = entry;
        take.notes = 1;
      }
      if (state.undo.length > 90) state.undo.shift();
    }
    state.redo = [];
    state.dirty = true;
    app.inputEditGeneration++;
    state.track = clamp(state.track, 0, state.project.tracks.length - 1);
    state.row = clamp(state.row, 0, pattern().length - 1);
    // A removed track or shortened pattern must not leave the selection anchor outside the grid.
    if (state.selection) state.selection = {
      track: clamp(state.selection.track, 0, state.project.tracks.length - 1),
      row: clamp(state.selection.row, 0, pattern().length - 1)
    };
    if (options.render !== false) {
      app.refresh();
    } else {
      app.updateTransport();
    }
    app.syncEngine();
    app.scheduleAutosave();
    return true;
  }

  let pendingNativeUndo = null;

  function undo(redo = false) {
    if (busy.refuse('undo', ['lineInputs'])) return false;
    if (window.STMK3?.hasPendingEdits) return Promise.resolve(window.STMK3.flushEdits()).then(ok => ok === false ? false : undo(redo), error => {
      toast(error.message, 'error');
      return false;
    });
    if (busy.isBusy('kits')) return window.STKitGroups.waitForIdle().then(() => undo(redo), error => {
      toast(error.message, 'error');
      return false;
    });
    if (busy.isBusy('instrumentEdits')) return window.STInstrumentEdits.waitForIdle().then(() => undo(redo), error => {
      toast(error.message, 'error');
      return false;
    });
    if (pendingNativeUndo) {
      toast('Finishing the current control edit before ' + (redo ? 'Redo' : 'Undo') + '.');
      return pendingNativeUndo;
    }
    if (!window.STPendingEdits.hasPending()) return applyUndo(redo);
    const owner = state.project,
      inputGeneration = app.inputCaptureGeneration;
    pendingNativeUndo = (async () => {
      try {
        if ((await window.STPendingEdits.flush()) === false || state.project !== owner) return false;
        if (app.inputCaptureGeneration !== inputGeneration) {
          toast('New recording input arrived. Retry '
              + (redo ? 'Redo' : 'Undo')
              + ' after the current input finishes.');
          return false;
        }
        return applyUndo(redo);
      } finally {
        pendingNativeUndo = null;
      }
    })();
    return pendingNativeUndo;
  }

  function applyUndo(redo = false) {
    if (busy.refuse('undo', ['export', 'routing'])) return;
    if (!app.settleRecordedAutomation()) return false;
    if (window.STMK3?.commitBursts?.() === false) {
      busy.report('undo', 'Finish or retry the pending Maschine control edit first.');
      return false;
    }
    const from = redo ? state.redo : state.undo,
      to = redo ? state.undo : state.redo,
      item = from.pop();
    if (!item) {
      toast(redo ? 'Nothing to redo' : 'Nothing to undo');
      return;
    }
    if (state.modal === 'clip-takes') app.closeModal();
    to.push({ label: item.label, project: clone(state.project) });
    app.discardInputCaptures();
    app.resetMIDIRoutePublication();
    state.project = item.project;
    // The keyboard performance settings are the session's, not an Undo step: the restored project takes them.
    app.writePerformance(state.project);
    state.selection = null;
    state.track = clamp(state.track, 0, state.project.tracks.length - 1);
    state.instrument = clamp(state.instrument, 0, state.project.instruments.length - 1);
    state.row = clamp(state.row, 0, pattern().length - 1);
    state.sequence = clamp(state.sequence, 0, state.project.sequence.length - 1);
    window.STStudio?.event('project.loaded', {});
    window.STNKSControls?.event('project.loaded', {});
    window.STMK3?.event?.('project.loaded', {});
    state.dirty = true;
    app.syncEngine(false, true);
    app.refresh();
    toast(`${redo ? 'Redid' : 'Undid'} ${item.label}`);
  }

  Object.assign(app, { mutate, undo, applyUndo });
})();
