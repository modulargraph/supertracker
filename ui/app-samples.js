/* Samples: metadata cache per source path, and loading or editing sample files for the
   selected instrument (or as a new sampler track). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    clamp, uid, TRACK_SANITY_LIMIT, trackLimitMessage, state, instrumentDefaults, currentInstrument, toast,
    request, stableJSON
  } = app;
  // Metadata belongs to a source path, not to the instrument slot that happened
  // to load it. Keep this small cache across canonical project replacements.
  const sampleMetadataCache = new Map();
  const sampleMetadataRequests = new WeakMap();

  function rememberSampleMetadata(sample) {
    if (!sample || typeof sample.path !== 'string' || !sample.path) return;
    sampleMetadataCache.delete(sample.path);
    sampleMetadataCache.set(sample.path, sample);
    while (sampleMetadataCache.size > 128) sampleMetadataCache.delete(sampleMetadataCache.keys().next().value);
  }

  function reconcileSampleMetadata(rememberCurrent = true) {
    if (rememberCurrent) for (const sample of Object.values(state.samples)) rememberSampleMetadata(sample);
    const next = {};
    for (const [index, inst] of (state.project?.instruments || []).entries()) {
      const sample = sampleMetadataCache.get(inst.samplePath);
      if (sample) {
        next[index] = sample;
        sampleMetadataCache.delete(inst.samplePath);
        sampleMetadataCache.set(inst.samplePath, sample);
      }
    }
    state.samples = next;
  }

  function receiveSampleMetadata(index, sample) {
    if (!Number.isInteger(index)
        || !sample?.path
        || state.project?.instruments[index]?.samplePath !== sample.path) return false;
    const changed = stableJSON(state.samples[index]) !== stableJSON(sample);
    rememberSampleMetadata(sample);
    state.samples[index] = sample;
    reconcileSampleMetadata(false);
    if (changed && state.view === 'instrument' && currentInstrument()?.samplePath === sample.path) {
      const active = document.activeElement;
      if (active?.closest?.('input,textarea,select,[contenteditable=true]')) {
        // Keep a focused field's uncommitted draft/caret intact. Its normal edit
        // or blur will repaint the facts; the waveform can redraw in place now.
        window.STSampler?.redraw?.();
        const owner = state.project;
        active.addEventListener('blur', () => {
          if (state.project !== owner
              || state.view !== 'instrument'
              || currentInstrument()?.samplePath !== sample.path) return;
          // A click elsewhere blurs the field inside its pointerdown; repainting now would replace
          // the button being clicked and lose the click. Repaint once the field has settled.
          if (app.scalarFields) app.paintScalarFields(app.scalarFields); else app.refresh();
        }, { once: true });
      } else {
        app.refresh();
      }
    }
    return true;
  }

  function hydrateSampleMetadata(owner, publishedPaths) {
    if (!state.native || owner !== state.project) return;
    reconcileSampleMetadata();
    let attempted = sampleMetadataRequests.get(owner);
    if (!attempted) {
      attempted = new Map();
      sampleMetadataRequests.set(owner, attempted);
    }
    owner.instruments.forEach((inst, index) => {
      const path = inst.samplePath;
      const previous = attempted.get(path);
      if (inst.type !== 'sample'
          || !path
          || publishedPaths[index] !== path
          || state.samples[index]?.path === path
          || previous
              && owner.instruments[previous.index] === previous.inst
              && previous.inst.samplePath === path) return;
      attempted.set(path, { index, inst });
      request('sample.info', { instrument: index, path }).then(sample => {
        if (state.project !== owner
            || owner.instruments[index] !== inst
            || inst.samplePath !== path
            || sample?.available === false
            || sample?.path !== path) return;
        receiveSampleMetadata(index, sample);
      }).catch(() => {}); // Loading failures have their own sample.error event.
    });
  }

  async function loadSample(path, id = state.instrument) {
    const track = state.track,
      selected = state.instrument;
    return app.runInstrumentEdit(id, [...new Set([track,
        ...state.project.tracks.flatMap((t, i) => t.instrument === id ? [i] : [])])], async edit => {
      toast('Loading sample…');
      const result = await edit.native('sample.load', { path, instrument: id });
      if (!result
          || typeof (result.path || path) !== 'string') throw new Error('The sample loader returned no sample.');
      for (const t of edit.tracks) await app.removeInstrumentPlugin(t, edit);
      await edit.commit('load sample', p => {
        const inst = p.instruments[id];
        inst.type = 'sample';
        inst.plugin = null;
        inst.samplePath = result.path || path;
        inst.name = result.name || path.split('/').pop().replace(/\.[^.]+$/, '');
        inst.slicePoints = [];
        inst.slices = [];
        inst.zones = [];
        inst.start = 0;
        inst.end = 1;
        inst.loopStart = 0;
        inst.loopEnd = 1;
        app.removeInstrumentDescriptors(p, edit.tracks);
        p.tracks[track].instrument = id;
      });
      rememberSampleMetadata(result);
      reconcileSampleMetadata(false);
      if (state.track === track && state.instrument === selected) state.instrument = id;
      app.refresh();
      toast(`Loaded ${state.project.instruments[id].name}`);
      return result;
    });
  }

  // A new browser destination is appended only after decoding and native publication.
  // Anchoring the transaction to the existing instrument also retains its live state
  // for Undo when the selected track retires a hosted instrument.
  async function loadSampleAsNew(path) {
    const project = state.project,
      track = state.track,
      selected = state.instrument,
      id = project.instruments.length;
    if (project.tracks.length >= TRACK_SANITY_LIMIT) {
      toast(trackLimitMessage(), 'error');
      return false;
    }
    const destinationTrack = project.tracks.length,
      routingID = uid(),
      // An empty track selects no instrument (-1); anchor to the first one, or to none in an empty project.
      anchor = project.instruments[selected] ? selected : project.instruments.length ? 0 : null;
    return app.runInstrumentEdit(anchor, [], async edit => {
      toast('Loading into a new instrument…');
      const result = await edit.native('sample.load', { path, instrument: id });
      if (!result
          || typeof (result.path || path) !== 'string') throw new Error('The sample loader returned no sample.');
      const name = result.name || path.split('/').pop().replace(/\.[^.]+$/, '');
      await edit.commit('add sample instrument', p => {
        if (p.instruments.length !== id
            || p.tracks.length !== destinationTrack) throw new Error('The destination changed. Add the sample again.');
        // A loaded sample (a hit, a break) starts monophonic: a new note cuts the previous one, as in a tracker.
        p.instruments.push({ ...instrumentDefaults(id, name, 'sample'), voiceMode: 'mono', samplePath: result.path || path });
        app.appendInstrumentTrack(p, id, name, routingID);
      });
      rememberSampleMetadata(result);
      reconcileSampleMetadata(false);
      if (state.project === project && state.track === track && state.instrument === selected) {
        state.instrument = id;
        state.track = destinationTrack;
        state.selection = null;
      }
      app.refresh();
      toast(`Added ${name} · instrument ${String(id).padStart(2, '0')}`);
      return result;
    });
  }

  async function editSample(operation, extra = {}) {
    const id = state.instrument,
      inst = state.project.instruments[id];
    if (!inst.samplePath) {
      toast('Load a sample first.');
      return false;
    }
    const source = { ...extra, path: inst.samplePath, instrument: id, operation, start: inst.start, end: inst.end };
    try {
      return await app.runInstrumentEdit(id, [], async edit => {
        const result = await edit.native('sample.edit', source);
        if (!result?.path) throw new Error('The sample editor returned no edited file.');
        const start = source.start ?? 0,
          end = source.end ?? 1;
        await edit.commit(operation + ' sample', p => {
          const target = p.instruments[id];
          target.samplePath = result.path;
          if (operation === 'trim') {
            const width = Math.max(1e-9, end - start),
              map = value => clamp(((value ?? 0) - start) / width, 0, 1);
            target.loopStart = map(target.loopStart ?? 0);
            target.loopEnd = Math.max(map(target.loopEnd ?? 1), target.loopStart + 1e-6);
            target.start = 0;
            target.end = 1;
          }
        });
        rememberSampleMetadata(result);
        reconcileSampleMetadata(false);
        app.refresh();
        toast(`${operation} complete. Original sample preserved.`);
        return result;
      });
    } catch {
      return false;
    }
  }

  Object.assign(app, {
    rememberSampleMetadata, reconcileSampleMetadata, receiveSampleMetadata, hydrateSampleMetadata,
    loadSample, loadSampleAsNew, editSample
  });
})();
