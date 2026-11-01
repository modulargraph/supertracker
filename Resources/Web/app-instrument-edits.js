/* Instrument edits: native instrument changes (type, plug-in, sample) run as one
   transaction that is published to the engine and rolled back on failure. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    clone, state, rememberSampleMetadata, reconcileSampleMetadata, toast, request, mutate, syncEngineProject,
    stableJSON, captureNativeEdits
  } = app;
  // Native proposals remain private until their project publication and Undo commit
  // both succeed. Save/session fences wait on `promise`; internal capture must opt
  // out of that fence so it can drain earlier engine updates without waiting itself.
  let instrumentEditOperation = null;
  let instrumentEditGeneration = 0;
  let instrumentEditRestoreError = null;

  function removeInstrumentDescriptors(project, tracks) {
    const removed = project.audioPlugins.filter(x => x.kind === 'instrument' && tracks.includes(x.track));
    const ids = new Set(removed.map(x => x.instanceID).filter(Boolean));
    project.audioPlugins = project.audioPlugins.filter(x => !removed.includes(x));
    if (project.routing?.sidechains) project.routing.sidechains = project.routing.sidechains.filter(x => !ids.has(x.destination?.instanceID));
  }

  async function runInstrumentEdit(id, tracks, work) {
    const blockers = ['lineInputs', 'saving', 'sessionDialog', 'export', 'routing', 'trackDuplicate'];
    if (instrumentEditOperation || window.STBusy.any(blockers)) {
      const error = new Error('Finish the current audio or project operation before editing this instrument.');
      toast(error.message, 'error');
      throw error;
    }
    const project = state.project,
      inst = id === null ? null : project.instruments[id],
      targets = tracks.map(track => project.tracks[track]);
    if (inst === undefined || id === null && tracks.length || targets.some(x => !x)) {
      throw new Error('The destination instrument or track no longer exists.');
    }
    let release;
    const edit = {
      project,
      id,
      tracks,
      phase: 'preparing',
      changedNative: false,
      protectedTracks: new Set(tracks),
      sampleEvents: new Map(),
      promise: new Promise(resolve => release = resolve)
    };
    instrumentEditOperation = edit;
    instrumentEditGeneration++;
    const owned = () => state.project === project
        && (id === null ? null : project.instruments[id]) === inst
        && tracks.every((track, i) => project.tracks[track] === targets[i]);
    const signature = () => stableJSON([inst, tracks.map(track => project.tracks[track]),
        project.audioPlugins.filter(x => x.kind === 'instrument' && tracks.includes(x.track))]);
    const hadPluginInstance = !!inst?.plugin?.instanceID;
    const preparationSignature = () => stableJSON([{
      ...inst,
      plugin: inst?.plugin
          ? [inst.plugin.component, inst.plugin.format, inst.plugin.bundlePath,
              hadPluginInstance ? inst.plugin.instanceID : null, inst.plugin.nks?.path, inst.plugin.nks?.identity]
          : null
    }, tracks.map(track => project.tracks[track])]);
    const preparationBaseline = preparationSignature();
    let baseline = signature();
    const current = () => owned() && signature() === baseline;
    const requireCurrent = () => {
      if (!current()) throw new Error('The destination changed while the instrument edit was pending. The result was not attached.');
    };
    const restore = async () => {
      edit.phase = 'restoring';
      instrumentEditGeneration++;
      const result = await syncEngineProject(() => clone(state.project), {
        replace: true,
        instrumentEdit: true,
        capture: true
      });
      if (result === false) throw new Error('The current audio project restoration was not acknowledged.');
      edit.changedNative = false;
      instrumentEditRestoreError = null;
    };
    edit.native = async (command, payload) => {
      requireCurrent();
      edit.phase = 'applying';
      edit.changedNative = true;
      instrumentEditGeneration++;
      const result = await request(command, payload, { instrumentEdit: true });
      requireCurrent();
      if (result === false) throw new Error(command + ' was not acknowledged.');
      return result;
    };
    edit.prepareProject = async (apply, extraTracks = []) => {
      requireCurrent();
      edit.phase = 'applying';
      edit.changedNative = true;
      instrumentEditGeneration++;
      extraTracks.forEach(track => edit.protectedTracks.add(track));
      const result = await syncEngineProject(() => {
        if (!current()) return null;
        const candidate = clone(project);
        apply(candidate);
        return candidate;
      }, { instrumentEdit: true });
      requireCurrent();
      if (result === false) throw new Error('The new instrument destination was not accepted by the audio host.');
    };
    edit.commit = async (label, apply) => {
      requireCurrent();
      edit.phase = 'applying';
      edit.changedNative = true;
      instrumentEditGeneration++;
      let publishedBaseline;
      const result = await syncEngineProject(() => {
        if (!current()) return null;
        publishedBaseline = stableJSON(project);
        const candidate = clone(project);
        apply(candidate);
        return candidate;
      }, { instrumentEdit: true });
      requireCurrent();
      if (result === false) throw new Error('The instrument edit was not accepted by the audio host.');
      if (stableJSON(project) !== publishedBaseline) throw new Error('The project changed during the instrument edit. The result was not attached.');
      if (!mutate(label, apply, {
        instrumentEdit: true
      })) throw new Error('The instrument edit was not accepted. The previous sound was restored.');
      edit.changedNative = false;
      instrumentEditRestoreError = null;
    };
    try {
      if ((await captureNativeEdits({
        instrumentEdit: true
      })) === false) throw new Error('Current plug-in controls could not be captured. The instrument was not changed.');
      // Capture may refresh opaque descriptors, but must not retarget this edit.
      if (!owned()
          || preparationSignature() !== preparationBaseline) throw new Error('The destination changed during preparation.');
      baseline = signature();
      edit.protectEvents = true;
      if ((await syncEngineProject(() => current() ? clone(project) : null, {
        instrumentEdit: true
      })) === false) throw new Error('The audio project could not be prepared.');
      requireCurrent();
      // Always capture the retiring instance, even if its controls were edited in
      // a native editor. Keep that state in the canonical model for Undo/rollback.
      for (const track of tracks) {
        const plugin = project.audioPlugins.find(x => x.track === track && x.kind === 'instrument');
        if (!plugin) continue;
        const snapshot = await request('plugin.state', { track, kind: 'instrument', slot: 0 });
        requireCurrent();
        if (!snapshot?.component
            || snapshot.stateError
            || snapshot.component !== plugin.component
            || plugin.instanceID
                && snapshot.instanceID
                && snapshot.instanceID !== plugin.instanceID) throw new Error('Could not capture the current instrument plug-in before replacing it.');
        const merged = { ...plugin, ...snapshot };
        project.audioPlugins = project.audioPlugins.map(x => x === plugin ? merged : x);
        state.dirty = true;
        baseline = signature();
      }
      return await work(edit);
    } catch (error) {
      if (edit.changedNative) {
        try {
          await restore();
        } catch (restoreError) {
          instrumentEditRestoreError = restoreError;
          error = new Error(error.message + ' Could not restore the audio project: ' + restoreError.message);
        }
      }
      toast(error.message, 'error');
      throw error;
    } finally {
      reconcileSampleMetadata();
      for (const [index, sample] of edit.sampleEvents) if (sample?.path
          && state.project.instruments[index]?.samplePath === sample.path) rememberSampleMetadata(sample);
      reconcileSampleMetadata(false);
      instrumentEditOperation = null;
      instrumentEditGeneration++;
      release();
    }
  }

  window.STInstrumentEdits = {
    // The project transform passed to edit.commit runs on a proposal and then
    // canonical state: mutate ONLY that argument; update editor UI after await.
    run: runInstrumentEdit,
    get isBusy() {
      return !!instrumentEditOperation;
    },
    get isApplying() {
      return !!instrumentEditOperation && instrumentEditOperation.phase !== 'preparing';
    },
    get editGeneration() {
      return instrumentEditGeneration;
    },
    async waitForIdle() {
      while (instrumentEditOperation) await instrumentEditOperation.promise;
      if (instrumentEditRestoreError) throw instrumentEditRestoreError;
    },
    interceptPluginEvent(plugins) {
      const edit = instrumentEditOperation;
      if (!edit || !edit.protectEvents || !Array.isArray(plugins)) return false;
      const protectedPlugin = x => x.kind === 'instrument' && edit.protectedTracks.has(x.track);
      const key = x => stableJSON([x.owner, x.track, x.kind, x.slot ?? 0]);
      const remaining = plugins.filter(x => !protectedPlugin(x)),
        merged = [];
      for (const plugin of state.project.audioPlugins) {
        if (protectedPlugin(plugin)) {
          merged.push(plugin);
          continue;
        }
        const index = remaining.findIndex(x => key(x) === key(plugin));
        if (index >= 0) merged.push(...remaining.splice(index, 1));
      }
      merged.push(...remaining);
      if (stableJSON(merged) !== stableJSON(state.project.audioPlugins)) {
        window.STNKSControls?.adoptPlugins?.(merged);
        state.project.audioPlugins = merged;
      }
      return true;
    },
    interceptSampleEvent(payload) {
      const edit = instrumentEditOperation;
      if (!edit || !edit.protectEvents || payload.instrument !== edit.id) return false;
      edit.sampleEvents.set(payload.instrument, payload.sample);
      return true;
    }
  };

  window.STPendingEdits.register('instrumentEdits', { order: 70, generation: () => window.STInstrumentEdits?.editGeneration ?? 0 });

  // An instrument edit owns the plug-in and sample events of its native transaction.
  const events = window.STNativeEvents;
  events.intercept(events.PLUGIN_EVENTS, payload => window.STInstrumentEdits?.interceptPluginEvent?.(events.plugins(payload)), 30);
  events.intercept('sample.loaded', payload => window.STInstrumentEdits?.interceptSampleEvent?.(payload), 40);

  async function removeInstrumentPlugin(track, edit) {
    if (!edit || edit !== instrumentEditOperation) throw new Error('Instrument removal requires an owned edit.');
    if (edit.project.audioPlugins.some(x => x.track === track
        && x.kind === 'instrument')) await edit.native('plugin.remove', {
      track,
      kind: 'instrument',
      slot: 0
    });
  }

  async function changeInstrumentType(type) {
    const id = state.instrument;
    if (!['sample', 'au', 'midi'].includes(type)) {
      toast('Unknown instrument type.', 'error');
      return false;
    }
    const tracks = type === 'au' ? [] : state.project.tracks.flatMap((t, i) => t.instrument === id ? [i] : []);
    try {
      return await runInstrumentEdit(id, tracks, async edit => {
        for (const track of tracks) await removeInstrumentPlugin(track, edit);
        await edit.commit('instrument type', p => {
          p.instruments[id].type = type;
          if (type !== 'au') {
            p.instruments[id].plugin = null;
            removeInstrumentDescriptors(p, tracks);
          }
        });
        return true;
      });
    } catch {
      return false;
    }
  }

  async function assignInstrument() {
    const index = state.instrument,
      track = state.track,
      project = state.project,
      inst = project.instruments[index];
    if (project.tracks[track].instrument === index
        && inst.type === 'au'
        && project.audioPlugins.some(x => x.track === track && x.kind === 'instrument')) {
      toast('This plug-in is already assigned to the selected track.');
      return false;
    }
    const initialSourceTrack = inst.type === 'au'
        ? project.tracks.findIndex((t, i) => t.instrument === index
            && project.audioPlugins.some(x => x.track === i && x.kind === 'instrument'))
        : -1;
    const protectedTracks = [...new Set([track, ...(initialSourceTrack >= 0 ? [initialSourceTrack] : [])])];
    try {
      return await runInstrumentEdit(index, protectedTracks, async edit => {
        if (inst.type !== 'au') {
          await removeInstrumentPlugin(track, edit);
          await edit.commit('assign instrument', p => {
            removeInstrumentDescriptors(p, [track]);
            p.tracks[track].instrument = index;
          });
          return true;
        }
        const sourceTrack = project.tracks.findIndex((t, i) => t.instrument === index
            && project.audioPlugins.some(x => x.track === i && x.kind === 'instrument'));
        const source = sourceTrack >= 0
            ? project.audioPlugins.find(x => x.track === sourceTrack && x.kind === 'instrument')
            : inst.plugin;
        // runInstrumentEdit secured both lanes and protects their native events.
        // A second unowned source capture could refresh descriptors during publication.
        const snapshot = source;
        if (sourceTrack !== initialSourceTrack) throw new Error('The source plug-in changed while preparing its assignment.');
        if (!snapshot?.component
            || snapshot.stateError) throw new Error('Load a plug-in for this instrument from the browser first.');
        const loadDescriptor = {
          ...source,
          ...snapshot,
          track,
          kind: 'instrument',
          slot: 0,
          component: snapshot.component
        };
        // Native owner IDs outrank numeric tracks; a copied source must not retain
        // its old channel or instance identity when assigned to another lane.
        delete loadDescriptor.owner;
        delete loadDescriptor.instanceID;
        const loaded = await edit.native('plugin.load', loadDescriptor);
        const full = await request('plugin.state', { track, kind: 'instrument', slot: 0 });
        if (!full?.component
            || full.stateError
            || full.component !== snapshot.component) throw new Error('Could not capture the assigned plug-in.');
        const descriptor = { ...loadDescriptor, ...loaded, ...full, track, kind: 'instrument', slot: 0 };
        await edit.commit('assign plug-in', p => {
          removeInstrumentDescriptors(p, [track]);
          p.audioPlugins.push(descriptor);
          p.tracks[track].instrument = index;
          p.instruments[index].plugin = clone(descriptor);
        });
        return true;
      });
    } catch {
      return false;
    }
  }

  Object.assign(app, {
    removeInstrumentDescriptors, runInstrumentEdit, removeInstrumentPlugin, changeInstrumentType,
    assignInstrument
  });
})();
