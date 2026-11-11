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

  // A track's mixer channel, `track:<routingID>`: what a plug-in descriptor names as its owner.
  const trackOwner = (project, track) => `track:${project.tracks[track]?.routingID}`;
  const instrumentPluginOf = (project, track) => project.audioPlugins.find(x => x.kind === 'instrument'
      && x.owner === trackOwner(project, track));

  /* Takes the instrument plug-in off `tracks` (indices). A plug-in instrument that no other track plays keeps
     its descriptor (routing-project.js), unless `discard`: the instrument stops being a plug-in. */
  function removeInstrumentDescriptors(project, tracks, { discard = false } = {}) {
    if (!discard) window.STRoutingProject.keepInstrumentPlugins(project, tracks.map(track => project.tracks[track]));
    const owners = new Set(tracks.map(track => trackOwner(project, track)));
    const removed = project.audioPlugins.filter(x => x.kind === 'instrument' && owners.has(x.owner));
    const ids = new Set(removed.map(x => x.instanceID).filter(Boolean));
    project.audioPlugins = project.audioPlugins.filter(x => !removed.includes(x));
    project.routing.sidechains = project.routing.sidechains.filter(x => !ids.has(x.destination));
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
      sampleEvents: new Map(),
      promise: new Promise(resolve => release = resolve)
    };
    instrumentEditOperation = edit;
    instrumentEditGeneration++;
    const owned = () => state.project === project
        && (id === null ? null : project.instruments[id]) === inst
        && tracks.every((track, i) => project.tracks[track] === targets[i]);
    const signature = () => stableJSON([inst, tracks.map(track => project.tracks[track]),
        tracks.map(track => instrumentPluginOf(project, track)).filter(Boolean)]);
    // What the instrument is, without the settings a capture may refresh while the edit is prepared.
    const pluginOf = () => id === null ? null : window.STRoutingProject.instrumentPlugin(project, id);
    const hadPluginInstance = !!pluginOf()?.instanceID;
    const preparationSignature = () => {
      const plugin = pluginOf();
      return stableJSON([{
        ...inst,
        plugin: plugin
            ? [plugin.component, plugin.format, plugin.bundlePath, hadPluginInstance ? plugin.instanceID : null, plugin.nks?.path, plugin.nks?.identity]
            : null
      }, tracks.map(track => project.tracks[track])]);
    };
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
    edit.prepareProject = async apply => {
      requireCurrent();
      edit.phase = 'applying';
      edit.changedNative = true;
      instrumentEditGeneration++;
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
        const plugin = instrumentPluginOf(project, track);
        if (!plugin) continue;
        const snapshot = await request('plugin.state', { owner: plugin.owner, kind: 'instrument', slot: 0 });
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
    interceptSampleEvent(payload) {
      const edit = instrumentEditOperation;
      if (!edit || !edit.protectEvents || payload.instrument !== edit.id) return false;
      edit.sampleEvents.set(payload.instrument, payload.sample);
      return true;
    }
  };

  window.STPendingEdits.register('instrumentEdits', { order: 70, generation: () => window.STInstrumentEdits?.editGeneration ?? 0 });

  // An instrument edit owns the sample events of its own transaction.
  window.STNativeEvents.intercept('sample.loaded', payload => window.STInstrumentEdits?.interceptSampleEvent?.(payload), 40);

  async function removeInstrumentPlugin(track, edit) {
    if (!edit || edit !== instrumentEditOperation) throw new Error('Instrument removal requires an owned edit.');
    if (instrumentPluginOf(edit.project, track)) await edit.native('plugin.remove', {
      owner: trackOwner(edit.project, track),
      kind: 'instrument',
      slot: 0
    });
  }

  async function changeInstrumentType(type) {
    const id = state.instrument;
    if (!['sample', 'plugin', 'midi'].includes(type)) {
      toast('Unknown instrument type.', 'error');
      return false;
    }
    const tracks = type === 'plugin' ? [] : state.project.tracks.flatMap((t, i) => t.instrument === id ? [i] : []);
    try {
      return await runInstrumentEdit(id, tracks, async edit => {
        for (const track of tracks) await removeInstrumentPlugin(track, edit);
        await edit.commit('instrument type', p => {
          p.instruments[id].type = type;
          if (type !== 'plugin') {
            delete p.instruments[id].plugin;
            removeInstrumentDescriptors(p, tracks, { discard: true });
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
        && inst.type === 'plugin'
        && instrumentPluginOf(project, track)) {
      toast('This plug-in is already assigned to the selected track.');
      return false;
    }
    const initialSourceTrack = inst.type === 'plugin'
        ? project.tracks.findIndex((t, i) => t.instrument === index && instrumentPluginOf(project, i))
        : -1;
    const protectedTracks = [...new Set([track, ...(initialSourceTrack >= 0 ? [initialSourceTrack] : [])])];
    try {
      return await runInstrumentEdit(index, protectedTracks, async edit => {
        if (inst.type !== 'plugin') {
          await removeInstrumentPlugin(track, edit);
          await edit.commit('assign instrument', p => {
            removeInstrumentDescriptors(p, [track]);
            p.tracks[track].instrument = index;
          });
          return true;
        }
        const sourceTrack = project.tracks.findIndex((t, i) => t.instrument === index && instrumentPluginOf(project, i));
        // From a track that plays the instrument, or what the instrument kept while none did.
        const source = sourceTrack >= 0 ? instrumentPluginOf(project, sourceTrack) : inst.plugin;
        // runInstrumentEdit secured both lanes and protects their native events.
        // A second unowned source capture could refresh descriptors during publication.
        const snapshot = source;
        if (sourceTrack !== initialSourceTrack) throw new Error('The source plug-in changed while preparing its assignment.');
        if (!snapshot?.component
            || snapshot.stateError) throw new Error('Load a plug-in for this instrument from the browser first.');
        const owner = trackOwner(project, track);
        const loadDescriptor = {
          ...source,
          ...snapshot,
          owner,
          kind: 'instrument',
          slot: 0,
          component: snapshot.component
        };
        // The copy is a new instance on this track's channel, not the source's.
        delete loadDescriptor.instanceID;
        const loaded = await edit.native('plugin.load', loadDescriptor);
        const full = await request('plugin.state', { owner, kind: 'instrument', slot: 0 });
        if (!full?.component
            || full.stateError
            || full.component !== snapshot.component) throw new Error('Could not capture the assigned plug-in.');
        const descriptor = { ...loadDescriptor, ...loaded, ...full, owner, kind: 'instrument', slot: 0 };
        await edit.commit('assign plug-in', p => {
          removeInstrumentDescriptors(p, [track]);
          p.audioPlugins.push(descriptor);
          p.tracks[track].instrument = index;
          delete p.instruments[index].plugin;
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
