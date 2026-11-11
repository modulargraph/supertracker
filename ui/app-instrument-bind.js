/* Instrument view events. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, state, currentInstrument, toast, request, mutate, bindScalarField, setView, refresh,
    bindSidebar, noteOn, noteOff, stopHeld, instParamValue, instParamElementValue, applyInstParam,
    updateParamReadout, bindParameterKeys, loadSample, editSample
  } = app;

  function bindInstrument() {
    const instrumentOwner = currentInstrument(),
      instrumentIndex = state.instrument;
    bindSidebar();
    window.STNKSControls?.bind();
    window.STSampler?.bind();
    window.STWavetables?.bind();
    window.STGranular?.bind();
    window.STSamplerPlayheads?.sync();
    window.STModulation?.bind();
    window.STInstrumentPages?.bind();
    window.STInstrumentEffects?.bind(instrumentIndex);
    $('#instrument-find').onclick = () => window.STInstrumentNavigation?.show();
    $('#instrument-rename').onclick = () => {
      // Rename the instrument the prompt was opened for, even if the selection changes meanwhile.
      const instrument = currentInstrument();
      return app.textPrompt('Instrument name', instrument.name,
          name => mutate('rename instrument', p => { if (p.instruments.includes(instrument)) instrument.name = name; }));
    };
    $('#assign-instrument').onclick = app.assignInstrument;
    $('#load-sample').onclick = async () => {
      try {
        const r = await request('dialog.openSample');
        const path = typeof r === 'string' ? r : r?.path;
        if (path) await loadSample(path);
      } catch (e) {
        toast(e.message, 'error');
      }
    };
    for (const b of $$('[data-inst-type]')) b.onclick = () => app.changeInstrumentType(b.dataset.instType);
    for (const el of $$('[data-inst-param]')) {
      el.oninput = () => {
        if (el.isConnected === false
            || currentInstrument() !== instrumentOwner
            || state.instrument !== instrumentIndex) return;
        const key = el.dataset.instParam,
          value = instParamValue(el);
        if (!Number.isFinite(value)) return;
        for (const twin of $$(`[data-inst-param="${key}"]`)) if (twin !== el) twin.value = instParamElementValue(twin, value);
        updateParamReadout(key, value);
        // Heard at once on sounding voices; the change event commits it (one undo step, project publish).
        if (String(el.value).trim() !== '') app.liveInstParam(instrumentIndex, instrumentOwner, key, value);
      };
      bindScalarField(el, 'instrument ' + el.dataset.instParam,
          () => applyInstParam(instrumentOwner, el.dataset.instParam, instParamValue(el)),
          () => currentInstrument() === instrumentOwner && state.instrument === instrumentIndex);
    }
    bindParameterKeys();
    for (const el of $$('[data-inst-select]')) el.onchange = () => {
      if (el.isConnected === false
          || currentInstrument() !== instrumentOwner
          || state.instrument !== instrumentIndex) return;
      mutate('instrument ' + el.dataset.instSelect, () => {
        const key = el.dataset.instSelect,
          value = key.startsWith('midi') ? Number(el.value) : el.value;
        if (key.startsWith('midi') && instrumentOwner[key] !== value) stopHeld();
        instrumentOwner[key] = value;
      });
    };
    for (const b of $$('[data-sample-edit]')) b.onclick = () => editSample(b.dataset.sampleEdit);
    if ($('#sample-reveal')) $('#sample-reveal').onclick = () => request('app.external', {
      path: currentInstrument().samplePath,
      reveal: true
    }).catch(e => toast(e.message, 'error'));
    if ($('#browse-plugins')) $('#browse-plugins').onclick = () => {
      if (window.STBrowser?.showNKS) {
        app.browseNKS('instrument');
        return;
      }
      setView('browser');
      window.STBrowser?.showPlugins?.('instrument');
    };
    for (const id of ['plugin-editor', 'plugin-parameters']) if ($('#' + id)) $('#' + id).onclick = () => {
      const target = { owner: `track:${state.project.tracks[state.track].routingID}`, kind: 'instrument', slot: 0 };
      return id === 'plugin-editor' ? request('plugin.editor', target).catch(e => toast(e.message, 'error')) : app.showPluginParameters(target);
    };
    if ($('#midi-refresh')) $('#midi-refresh').onclick = async () => {
      try {
        state.midi = await request('midi.devices');
        refresh();
      } catch (e) {
        toast(e.message, 'error');
      }
    };
    if ($('#instrument-monitor')) $('#instrument-monitor').onclick = async () => {
      try {
        state.audio = await request('audio.configure', { monitor: !state.audio.monitoring });
        refresh();
      } catch (e) {
        toast(e.message, 'error');
      }
    };
    if ($('#instrument-record-input')) $('#instrument-record-input').onclick = () => app.showRecording({
      hardwareSample: true
    });
    if ($('#instrument-line-inputs')) $('#instrument-line-inputs').onclick = () => app.perform('lineInputs');
    if ($('#instrument-audio-settings')) $('#instrument-audio-settings').onclick = app.showSettings;
    $('#instrument-octave').onchange = e => {
      state.octave = Number(e.target.value);
      refresh();
    };
    // As the Pattern view's Chord control: saved with the song, leaves a chord set, updates the MIDI keyboard map.
    $('#instrument-chord').onchange = e => app.setPerformance({ chord: e.target.value, chordSet: '' }, false);
    for (const el of $$('[data-piano]')) {
      el.onpointerdown = e => {
        e.preventDefault();
        noteOn(Number(el.dataset.piano), 'piano:' + el.dataset.piano);
        el.setPointerCapture(e.pointerId);
      };
      el.onpointerup = () => noteOff('piano:' + el.dataset.piano);
      el.onpointercancel = () => noteOff('piano:' + el.dataset.piano);
      el.onkeydown = e => {
        if ((e.key === ' ' || e.key === 'Enter') && !e.repeat) {
          e.preventDefault();
          noteOn(Number(el.dataset.piano), 'piano:' + el.dataset.piano);
        }
      };
      el.onkeyup = e => {
        if (e.key === ' ' || e.key === 'Enter') {
          e.preventDefault();
          noteOff('piano:' + el.dataset.piano);
        }
      };
      el.onblur = () => noteOff('piano:' + el.dataset.piano);
    }
  }

  Object.assign(app, { bindInstrument });
})();
