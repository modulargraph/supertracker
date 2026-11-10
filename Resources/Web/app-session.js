/* Session: save, open, new and replace the project (with unsaved-change prompts), accept a
   project from native, autosave and MIDI file import/export. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clone, esc, state, reconcileSampleMetadata, blankProject, demoProject, normalize, toast, request,
    waitForEffectMove, syncEngine, drainEngineSync, resetMIDIRoutePublication,
    adoptSavedMIDIRoute, refresh, setNoteRecording, updateTransport, readPerformance, stopHeld, openModal,
    closeModal, stableJSON
  } = app;
  let autosaveTimer;

  function acceptProject(p) {
    resetMIDIRoutePublication();
    app.pendingPlayback = null;
    app.stoppingPlayback = null;
    app.queuedPlayback = null;
    state.countIn = null;
    state.playbackMode = 'pattern';
    state.sequence = 0;
    stopHeld();
    app.midiRecordNotes.clear();
    app.liveRecordGroups.clear();
    request('transport.stop').catch(() => {});
    state.playing = false;
    setNoteRecording(false, { newSession: true });
    state.status = app.recordArmSupported ? { recordArmSupported: true, noteRecordArmed: false } : {};
    state.project = normalize(p);
    app.startupPending = false;
    readPerformance(state.project);
    state.track = 0;
    state.instrument = state.project.tracks[0].instrument || 0;
    state.row = 0;
    state.column = 0;
    state.noteColumn = 0;
    // The selection anchor and the pattern loop belong to the session that was replaced.
    state.selection = null;
    state.patternLoop = null;
    state.undo = [];
    state.redo = [];
    state.dirty = false;
    reconcileSampleMetadata();
    window.STStudio?.event('project.loaded', {});
    window.STNKSControls?.event('project.loaded', {});
    window.STMK3?.event?.('project.loaded', {});
    refresh();
    syncEngine();
  }

  function scheduleAutosave(delay = 1800) {
    clearTimeout(autosaveTimer);
    autosaveTimer = setTimeout(async () => {
      // The boot placeholder is not the session: the recovery copy keeps the host's startup project.
      if (app.startupPending) return;
      const busy = () => window.STBusy.any(['lineInputs', 'saving', 'sessionDialog', 'export', 'instrumentEdits', 'kits']);
      if (busy()) {
        scheduleAutosave(1000);
        return;
      }
      const owner = state.project;
      await waitForEffectMove();
      if (state.project !== owner) return;
      if (busy()) {
        scheduleAutosave(1000);
        return;
      }
      try {
        const result = await request('project.autosave', { project: owner });
        if (result?.deferred && state.project === owner) scheduleAutosave(1000);
      } catch {}
    }, delay);
  }

  app.sessionTransition = false;
  app.savingProject = false;
  window.STBusy.register('saving', { active: () => app.savingProject, reason: 'Wait for the current save to finish.' });
  window.STBusy.register('sessionDialog', { active: () => app.sessionTransition, reason: 'Finish the current session dialog first.' });
  app.queuedOpenPath = null;
  app.acceptingOpenRequests = false;

  function settleRecordedAutomation(options = {}) {
    return window.STAutomationRecord?.finishPass?.(options) !== false;
  }

  async function captureNativeEdits(options = {}) {
    try {
      if (!options.kitEdit) await window.STKitGroups?.waitForIdle?.();
      if (!options.instrumentEdit) await window.STInstrumentEdits?.waitForIdle?.();
      if ((await window.STMK3?.flushEdits?.({
        nativeCapture: true,
        render: false,
        mk3Edit: options.mk3Edit
      })) === false) return false;
      if (!settleRecordedAutomation({ nativeCapture: true, render: false })) return false;
      if ((await window.STPendingEdits.flush()) === false) return false;
      const syncFailures = await drainEngineSync();
      if (!state.native || !state.engineReady) {
        if (syncFailures.length) throw syncFailures[0];
        return true;
      }
      if (!syncFailures.length && !state.project.audioPlugins?.length && !options.preserveUnavailable) return true;
      const owner = state.project,
        pluginsBefore = stableJSON(owner.audioPlugins),
        nativeEditGeneration = window.STPendingEdits.generation();
      let unavailable = false;
      if (syncFailures.length || options.preserveUnavailable) {
        const routing = await request('routing.status');
        if (state.project !== owner) return false;
        unavailable = routing?.projectUnavailable === true;
        if (syncFailures.length && !unavailable) throw syncFailures[0];
      }
      const snapshot = await request('engine.snapshot');
      if (state.project !== owner) return false;
      if (window.STPendingEdits.generation() !== nativeEditGeneration
          || window.STPendingEdits.hasPending(options)) throw new Error('Recording input or plug-in controls changed during capture. Save again after the current edit finishes.');
      if (stableJSON(owner.audioPlugins) !== pluginsBefore) throw new Error('The plug-ins changed during capture. Retry after the current edit finishes.');
      if (unavailable) {
        // An opened, unavailable graph retains its desired project in the host.
        // Permit saving or repairing it only when no opaque state would be replaced.
        if (!owner.routing
            || stableJSON(snapshot?.routing) !== stableJSON(owner.routing)
            || stableJSON(snapshot?.audioPlugins ?? []) !== pluginsBefore) throw syncFailures[0]
                || new Error('The audio host no longer retains the current routing and plug-in state.');
        return true;
      }
      if (!Array.isArray(snapshot?.audioPlugins)) throw new Error('The host returned no plug-in state.');
      const failed = snapshot.audioPlugins.find(plugin => plugin?.stateError);
      if (failed) throw new Error((failed.name || 'A plug-in') + ': ' + failed.stateError);
      if (stableJSON(owner.audioPlugins) !== stableJSON(snapshot.audioPlugins)) {
        window.STNKSControls?.adoptPlugins?.(snapshot.audioPlugins);
        owner.audioPlugins = snapshot.audioPlugins;
        state.dirty = true;
        updateTransport();
      }
      return true;
    } catch (error) {
      toast('Could not capture current project edits: ' + error.message, 'error');
      state.dirty = true;
      return false;
    }
  }

  async function saveProject(action = 'save', path) {
    if (window.STBusy.refuse('save', ['basePatch', 'lineInputs'])) return false;
    if (window.STBusy.refuse('save', ['export', 'saving'])) return false;
    const mk3Owner = state.project;
    if ((await window.STMK3?.flushEdits?.({ nativeCapture: true, render: false })) === false) return false;
    if (state.project !== mk3Owner || window.STBusy.refuse('save', ['saving'])) return false;
    app.savingProject = true;
    const owner = state.project;
    try {
      if ((await captureNativeEdits()) === false) return false;
      if (state.project !== owner) return false;
      const before = stableJSON(owner),
        nativeEditGeneration = window.STPendingEdits.generation(),
        result = await request('project.' + action, { project: clone(owner), ...(path ? { path } : {}) });
      if (result?.cancelled) return false;
      if (state.project !== owner) return false;
      if (stableJSON(owner) !== before
          || window.STPendingEdits.generation() !== nativeEditGeneration
          || window.STPendingEdits.hasPending()
          || window.STAutomationRecord?.hasPendingEdits
          || window.STBusy.isBusy('instrumentEdits')) {
        state.dirty = true;
        toast('Project saved. Newer edits remain unsaved.');
        updateTransport();
        return false;
      }
      app.startupPending = false;
      if (result?.project) {
        const saved = normalize(result.project);
        adoptSavedMIDIRoute(owner, saved);
        app.rebindSavedPluginParameterDialog(owner, saved);
        app.rebindSavedInputCaptures(owner, saved);
        if (result.saved === true) window.STNKSControls?.adoptSavedView?.(owner, saved);
        const samplePathsChanged = stableJSON(owner.instruments.map(inst => inst.samplePath)) !== stableJSON(saved.instruments.map(inst => inst.samplePath));
        state.project = saved;
        reconcileSampleMetadata();
        if (samplePathsChanged) syncEngine();
        if (state.view === 'instrument') refresh();
      }
      state.dirty = false;
      updateTransport();
      const savedMessage = 'Project saved' + (result?.path ? ' · ' + result.path.split('/').pop() : '');
      toast(result?.recoveryWarning
          ? savedMessage + '. Recovery copy could not be updated: ' + result.recoveryWarning
          : savedMessage,
          result?.recoveryWarning ? 'error' : undefined);
      return true;
    } catch (error) {
      toast(error.message, 'error');
      return false;
    } finally {
      app.savingProject = false;
    }
  }

  async function confirmSessionReplacement(actionLabel) {
    if ((await captureNativeEdits()) === false) return false;
    if (!state.dirty) return true;
    return new Promise(resolve => {
      let settled = false;
      const finish = proceed => {
        if (settled) return;
        settled = true;
        app.modalDismiss = null;
        closeModal();
        resolve(proceed);
      };
      openModal(`Save changes to “${state.project.name}”?`, `<p class="muted" style="line-height:1.7">Save your current session before ${esc(actionLabel)}. Discard continues `
          + `without saving these changes.</p>`, '<button id="unsaved-cancel">Cancel</button><button id="unsaved-discard">Discard</button>'
          + '<button id="unsaved-save" class="primary">Save</button>', {
        kind: 'unsaved'
      });
      app.modalDismiss = () => {
        if (!settled) {
          settled = true;
          resolve(false);
        }
      };
      $('#unsaved-cancel').onclick = () => finish(false);
      $('#unsaved-discard').onclick = () => finish(true);
      $('#unsaved-save').onclick = async () => {
        for (const button of $$('.modal button')) button.disabled = true;
        const saved = await saveProject();
        finish(saved);
      };
      setTimeout(() => $('#unsaved-save')?.focus(), 0);
    });
  }

  async function replaceSession(label, operation) {
    if (window.STBusy.refuse('replace', ['basePatch', 'templates', 'lineInputs', 'settings'])) return false;
    if (window.STBusy.refuse('replace', ['export', 'recording', 'sessionDialog'])) return false;
    app.sessionTransition = true;
    try {
      if (!(await confirmSessionReplacement(label))) return false;
      if (window.STBusy.refuse('replace', ['export', 'recording'])) return false;
      await operation();
      return true;
    } catch (error) {
      toast(error.message, 'error');
      return false;
    } finally {
      app.sessionTransition = false;
      if (app.queuedOpenPath) {
        const path = app.queuedOpenPath;
        app.queuedOpenPath = null;
        projectAction('open', path);
      }
    }
  }

  async function projectAction(action, path) {
    if (action === 'save' || action === 'saveAs') return saveProject(action, path);
    if (action === 'open' && window.STBusy.isBusy('sessionDialog')) {
      if (path) {
        app.queuedOpenPath = path;
      } else {
        window.STBusy.refuse('open', ['sessionDialog']);
      }
      return false;
    }
    if (action === 'new') return replaceSession('starting a new session', async () => {
      const result = await request('project.new', path ? { templateID: path } : {});
      acceptProject(result?.project || blankProject());
    });
    if (action === 'demo') return replaceSession('loading the demo', async () => {
      await request('project.new', { blank: true });
      acceptProject(demoProject());
    });
    if (action === 'open') return replaceSession('opening another project', async () => {
      const result = await request('project.open', { project: state.project, ...(path ? { path } : {}) });
      if (result?.cancelled) return;
      acceptProject(result.project || result);
      acceptMigration(result);
    });
    return false;
  }

  // project.open reports a project converted from an older format (built-in synths became
  // wavetable samplers or rendered drum samples, tempo maps were removed): keep it unsaved.
  function migrationMessage(project) {
    const instruments = project?.instruments || [];
    const tables = instruments.filter(inst => /^builtin:wavetables\//.test(inst?.samplePath || '')).length,
      drums = instruments.filter(inst => /^builtin:drums\//.test(inst?.samplePath || '')).length;
    const parts = [tables ? `${tables} synth instrument${tables === 1 ? '' : 's'} to wavetable samplers` : '',
      drums ? `${drums} drum synth${drums === 1 ? '' : 's'} to samples` : ''].filter(Boolean);
    return parts.length ? `Converted ${parts.join(' and ')}` : 'Updated this project to the current format';
  }

  function acceptMigration(result) {
    if (result?.migrated !== true) return false;
    if (result.dirty !== false) {
      state.dirty = true;
      updateTransport();
    }
    const kits = (result.migrationNotes || []).filter(note => typeof note === 'string' && note);
    toast(kits.length ? kits.join(' ') + ' Save to keep the converted project; the original file is unchanged until then.' : migrationMessage(state.project));
    return true;
  }

  async function midiFileAction(importing) {
    const operation = async () => {
      const result = await request(importing ? 'project.importMIDI' : 'project.exportMIDI', importing ? {} : {
        project: state.project
      });
      if (result?.cancelled) return;
      if (importing && result?.project) acceptProject(result.project);
      if (result?.warnings?.length) {
        openModal(importing
            ? 'MIDI imported'
            : 'MIDI exported', `<div class="stack"><p>${esc(result.path?.split('/').pop() || 'MIDI file')}</p>`
            + `<div class="notice">${result.warnings.map(esc).join('<br>')}</div></div>`);
      } else {
        toast(importing ? 'MIDI file imported' : 'MIDI file exported');
      }
    };
    if (importing) return replaceSession('importing MIDI as a new session', operation);
    try {
      await operation();
    } catch (error) {
      toast(error.message, 'error');
    }
  }

  Object.assign(app, {
    acceptProject, scheduleAutosave, settleRecordedAutomation, captureNativeEdits, projectAction,
    midiFileAction, migrationMessage, acceptMigration
  });
})();
