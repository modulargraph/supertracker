/* Commit a native NKS load to the current session only after the host validates it. */
(() => {
  'use strict';
  // Insert slots per mixer channel (0...15), matching the audio host.
  const INSERT_SLOTS = 16;
  let loading = false;
  const { clone: copy } = window.STUtil;
  async function loadInstrument(command, payload, options = {}) {
    const app = window.ST, state = app.state, project = state.project;
    const sourceIndex = state.instrument, sourceTrack = state.track, source = project.instruments[sourceIndex];
    const create = !!options.newInstrument
        || !options.replaceInstrument && source?.type !== 'plugin'
        || project.tracks[sourceTrack]?.instrument !== sourceIndex;
    const destination = create ? project.instruments.length : sourceIndex;
    const targetTrack = create ? project.tracks.length : sourceTrack;
    const trackLimit = window.TRACK_SANITY_LIMIT ?? Infinity;
    if (create
        && targetTrack >= trackLimit) throw new Error(`A new plug-in needs room for one track (maximum ${trackLimit} tracks).`);
    const routingID = create ? app.uid() : null;
    // The channel of the destination track: the new track's identity is chosen here.
    const owner = `track:${create ? routingID : project.tracks[sourceTrack].routingID}`;
    const append = candidate => {
      if (!create) return;
      if (candidate.instruments.length !== destination
          || candidate.tracks.length !== targetTrack) throw new Error('The destination list changed while loading. Try again.');
      candidate.instruments.push(app.instrumentDefaults(destination, options.name || 'Plug-in', 'plugin'));
      app.appendInstrumentTrack(candidate, destination, options.name || 'Plug-in', routingID);
    };
    await window.STInstrumentEdits.run(sourceIndex, create ? [] : [sourceTrack], async edit => {
      app.stopHeld();
      if (create) await edit.prepareProject(append);
      const loaded = await edit.native(command, { ...payload, owner, kind: 'instrument', slot: 0 });
      if (!loaded?.component || (command === 'plugin.loadNKS' && !loaded.nks)) throw new Error('The host did not return a validated instrument.');
      const captured = await edit.native('plugin.state', { owner, kind: 'instrument', slot: 0, instanceID: loaded.instanceID });
      if (!captured?.component
          || captured.stateError
          || captured.component !== loaded.component) throw new Error('The loaded plug-in state could not be captured.');
      const result = { ...loaded, ...captured, owner, kind: 'instrument', slot: 0 };
      const name = result.nks?.name || result.name || options.name || 'Plug-in';
      await edit.commit(`Load ${name}`, candidate => {
        append(candidate);
        Object.assign(candidate.instruments[destination], { type: 'plugin', name });
        delete candidate.instruments[destination].plugin;
        candidate.tracks[targetTrack].instrument = destination;
        if (create) candidate.tracks[targetTrack].name = name;
        candidate.audioPlugins = candidate.audioPlugins.filter(value => !(value.owner === owner && value.kind === 'instrument'));
        candidate.audioPlugins.push(copy(result));
      });
      if (state.project === project && state.track === sourceTrack && state.instrument === sourceIndex) {
        state.track = targetTrack; state.instrument = destination;
        // The commit repainted before the cursor moved; repaint headers and sidebar for the new track.
        if (!options.stay) app.setView('instrument'); else app.refresh();
      }
      app.toast(`Loaded ${name}${create ? ' on a new track' : ''}`);
    });
    return true;
  }
  async function load(path, options = {}) {
    const app = window.ST, state = app.state;
    if (loading) { app.toast('An NKS preset is already loading.'); return false; }
    if (!state.native) { app.toast('Open the SuperTracker app to load NKS presets.'); return false; }
    if (options.kind === 'effect') return loadEffect(path, options);
    const owner = state.project, trackIndex = state.track, track = owner.tracks[trackIndex];
    const channel = `track:${track?.routingID}`, ownInstrument = plugin => plugin.owner === channel && plugin.kind === 'instrument';
    const instrumentIndex = state.instrument, instrument = owner.instruments[instrumentIndex];
    if (!track || !instrument || typeof path !== 'string' || !path) return false;
    // Browsing a plug-in must not overwrite the selected sample.
    options = { ...options, newInstrument: !!options.newInstrument || instrument.type !== 'plugin' };
    if (options.newInstrument && window.STInstrumentEdits?.run && app.appendInstrumentTrack) {
      loading = true;
      try { return await loadInstrument('plugin.loadNKS', { path }, options); }
      catch (error) { app.toast(error.message || String(error), 'error'); return false; }
      finally { loading = false; }
    }
    const pluginOf = () => window.STRoutingProject.instrumentPlugin(owner, instrumentIndex);
    const previousAssignment = track.instrument;
    const pluginIdentity = plugin => [plugin?.component, plugin?.format, plugin?.bundlePath, plugin?.instanceID,
        plugin?.nks?.path, plugin?.nks?.identity];
    const destination = value => JSON.stringify({ ...value, plugin: pluginIdentity(pluginOf()) });
    const previousDestination = destination(instrument); let previousInstrument = null;
    const current = () => state.project === owner && owner.tracks[trackIndex] === track &&
      owner.instruments[instrumentIndex] === instrument && track.instrument === previousAssignment &&
      destination(instrument) === previousDestination && (previousInstrument === null || JSON.stringify(instrument) === previousInstrument);
    const restoreCurrent = async () => {
      if ((await app.syncEngineProject(() => copy(state.project), {
        replace: true
      })) === false) throw new Error('Audio restoration was canceled because the session changed again.');
    };
    let attempted = false;
    loading = true;
    try {
      if (await app.captureNativeEdits?.() === false) return false;
      if (!current()) return false;
      previousInstrument = JSON.stringify(instrument);
      if (await app.syncEngineProject(() => current() ? copy(owner) : null) === false || !current()) return false;
      const previousSlot = JSON.stringify(owner.audioPlugins.find(ownInstrument));
      const targetUnchanged = () => current()
          && JSON.stringify(owner.audioPlugins.find(ownInstrument)) === previousSlot;
      app.stopHeld();
      app.toast('Loading NKS preset…');
      attempted = true;
      const result = await app.request('plugin.loadNKS', { path, owner: channel, kind: 'instrument', slot: 0 });
      if (!targetUnchanged()) {
        await restoreCurrent(); app.toast('Preset loading was canceled because its destination changed.'); return false;
      }
      if (!result || !result.component || !result.nks) throw new Error('The host did not return a validated NKS instrument.');
      const stillSelected = state.track === trackIndex && state.instrument === instrumentIndex;
      // A busy or frozen session refuses the edit; the host must then return to the saved project.
      const applied = app.mutate('load NKS preset', project => {
        let destination = instrumentIndex;
        if (options.newInstrument) {
          // The track's previous plug-in instrument stays available.
          window.STRoutingProject.keepInstrumentPlugins(project, [track]);
          destination = project.instruments.length;
          const defaults = app.instrumentDefaults(destination, result.nks.name || result.name, 'plugin');
          project.instruments.push(defaults);
        }
        const selected = project.instruments[destination];
        selected.type = 'plugin'; selected.name = result.nks.name || result.name; delete selected.plugin;
        track.instrument = destination;
        if (stillSelected) state.instrument = destination;
        project.audioPlugins = project.audioPlugins.filter(plugin => !ownInstrument(plugin));
        project.audioPlugins.push(copy(result));
      });
      if (applied === false) throw new Error('The session could not take the preset right now. Nothing was changed.');
      if (stillSelected && !options.stay) app.setView('instrument');
      app.toast(`Loaded ${result.nks.name || result.name}`);
      return true;
    } catch (error) {
      if (attempted) {
        try { await restoreCurrent(); }
        catch (restoreError) { app.toast(`Preset loading failed; restoring the session also failed: ${restoreError.message}`,
            'error'); return false; }
      }
      app.toast(error.message || String(error), 'error'); return false;
    } finally { loading = false; }
  }
  // .nksfx presets load into one insert slot of the track. The instrument is untouched.
  async function loadEffect(path, options) {
    const app = window.ST, state = app.state;
    const owner = state.project, trackIndex = state.track, track = owner.tracks[trackIndex];
    const slot = Math.trunc(Number(options.slot ?? 0));
    if (!track || typeof path !== 'string' || !path || !Number.isInteger(slot) || slot < 0 || slot >= INSERT_SLOTS) return false;
    const channel = `track:${track?.routingID}`;
    const sameSlot = plugin => plugin.owner === channel && plugin.kind === 'effect' && (plugin.slot ?? 0) === slot;
    const slotState = () => JSON.stringify(owner.audioPlugins.find(sameSlot) ?? null);
    let previousSlot = null;
    const current = () => state.project === owner && owner.tracks[trackIndex] === track && (previousSlot === null || slotState() === previousSlot);
    const restoreCurrent = async () => {
      if ((await app.syncEngineProject(() => copy(state.project), {
        replace: true
      })) === false) throw new Error('Audio restoration was canceled because the session changed again.');
    };
    let attempted = false;
    loading = true;
    try {
      if (await app.captureNativeEdits?.() === false) return false;
      if (!current()) return false;
      previousSlot = slotState();
      if (await app.syncEngineProject(() => current() ? copy(owner) : null) === false || !current()) return false;
      app.toast('Loading NKS effect…');
      attempted = true;
      const result = await app.request('plugin.loadNKS', { path, owner: channel, kind: 'effect', slot });
      if (!current()) { await restoreCurrent(); app.toast('Effect loading was canceled because its track or slot changed.'); return false; }
      if (!result || !result.component || !result.nks) throw new Error('The host did not return a validated NKS effect.');
      const name = result.nks.name || result.name;
      const applied = app.mutate('load NKS effect', project => {
        const descriptor = { ...copy(result), owner: channel, kind: 'effect', slot };
        project.audioPlugins = project.audioPlugins.filter(plugin => !sameSlot(plugin));
        project.audioPlugins.push(descriptor);
      });
      if (applied === false) throw new Error('The session could not take the effect right now. Nothing was changed.');
      app.toast(`Loaded ${name} on ${track.name || `Track ${trackIndex + 1}`} · FX ${slot + 1}`);
      return true;
    } catch (error) {
      if (attempted) {
        try { await restoreCurrent(); }
        catch (restoreError) { app.toast(`Effect loading failed; restoring the session also failed: ${restoreError.message}`,
            'error'); return false; }
      }
      app.toast(error.message || String(error), 'error'); return false;
    } finally { loading = false; }
  }
  window.STNKS = { load, loadInstrument, get loading() { return loading; } };
})();
