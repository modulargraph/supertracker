/* Native events: everything Swift pushes to the page arrives in window.nativeEvent(). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, state, receiveSampleMetadata, toast, acceptProject, refresh, receiveNoteRecordingStatus,
    updateTransport, receiveCountInStatus, updatePlayhead, pianoHighlight, loadSample, projectAction,
    perform, latencyNote, receiveAcknowledgedMIDI, endRecordTake, liveRecordGroups, recordIncomingMIDI
  } = app;
  // Native menu items arrive here only when the web keyboard handler left the key alone (or from a mouse pick).
  const MODAL_MENU_COMMANDS = new Set(['undo', 'redo', 'selectAll', 'stop', 'save', 'saveAs']);

  window.nativeEvent = function (event) {
    if (typeof event === 'string') {
      try {
        event = JSON.parse(event);
      } catch {
        return;
      }
    }
    const { type, payload = {} } = event;
    // Kit imports, instrument edits and track duplications claim the events of their own operation.
    if (window.STNativeEvents.consume(type, payload)) return;
    window.STMK3?.event?.(type, payload);
    window.STRecording?.event(type, payload);
    window.STExport?.event?.(type, payload);
    window.STFreeze?.event?.(type, payload);
    window.STNKSControls?.event(type, payload);
    if (type === 'sampler.playheads') { window.STSamplerPlayheads?.receive(payload); return; }
    if (type === 'status' || type === 'transport.status') {
      if (state.playing
          && payload.playing
          && Number.isFinite(payload.seconds)
          && Number.isFinite(state.status.seconds)
          && payload.seconds + 0.01 < state.status.seconds) {
        for (const group of liveRecordGroups.values()) if (group.active.size) group.clockWrapped = true;
      }
      state.status = payload;
      if (!payload.playing) endRecordTake();
      receiveNoteRecordingStatus(payload);
      state.lastStatusAt = Date.now();
      state.playing = !!payload.playing;
      receiveCountInStatus(payload);
      updateTransport();
      if (state.view === 'pattern') updatePlayhead(payload);
      window.STStudio?.event(type, payload);
      window.STClips?.event?.(type, payload);
    } else if (type === 'project.openRequested') {
      if (payload.path) {
        if (app.acceptingOpenRequests) {
          projectAction('open', payload.path);
        } else {
          app.queuedOpenPath = payload.path;
        }
      }
    } else if (type === 'project.loaded' || type === 'project.changed') {
      acceptProject(payload.project || payload);
    } else if (type === 'error') {
      toast(payload.message || payload.error || 'Native operation failed', 'error');
    } else if (type === 'midi.note') {
      if (payload.recordingPending === true || payload.recordingOnly === true) {
        receiveAcknowledgedMIDI(payload);
      } else if (state.recording || payload.on === false) recordIncomingMIDI(payload);
      if (payload.recordingOnly !== true) pianoHighlight(payload.note, payload.on !== false);
    } else if (type === 'command') {
      const name = payload.name || payload.command || payload.action;
      if (state.modal && !MODAL_MENU_COMMANDS.has(name)) {
        toast('Close the open dialog first.');
        $('.modal')?.contains(document.activeElement) || $('.modal input,.modal select,.modal button')?.focus();
      } else {
        perform(name);
      }
    } else if (type === 'audio.recorded') {
      if (payload.path) loadSample(payload.path);
    } else if (type === 'sample.loaded') {
      receiveSampleMetadata(payload.instrument, payload.sample);
    } else if (type === 'sample.error') {
      toast(payload.error || payload.message || 'A referenced sample could not be loaded', 'error');
    } else if (type === 'plugin.error') {
      toast(payload.error || payload.message || 'Plug-in could not be restored', 'error');
    } else if (type === 'audio.latency') {
      state.audio.latencyCompensation = payload;
      const info = $('#audio-latency');
      if (info) info.innerHTML = latencyNote(payload);
    } else if (type === 'audio.devices') {
      // Device restores, hot-plugs and live input changes: the mixer and Recording page show the current state.
      state.audio = { ...state.audio, ...payload };
      if (state.view === 'mixer' || state.view === 'recording') refresh();
    } else if (type === 'midi.status') {
      if (payload.devices) state.midi = payload.devices;
      if (payload.type === 'midiError') toast(payload.message, 'error');
    } else if (type === 'plugins.changed' || type === 'audioPlugins.changed') {
      // While a kit is staged in the audio host (published, not yet committed), the host's
      // plug-ins belong to that candidate; the commit brings its own descriptors.
      if (window.STKitGroups?.isBusy) return;
      const p = state.project, tracks = new Set((p.tracks || []).map(t => 'track:' + t.routingID)),
        buses = new Set((p.routing?.buses || []).map(b => 'bus:' + b.id));
      // A descriptor for an owner this project does not have (a superseded candidate) is not adopted.
      const instruments = new Set((p.instruments || []).map((_, i) => 'instrument:' + i));
      const plugins = (payload.plugins || payload).filter(x => !x.owner || x.owner === 'master' || tracks.has(x.owner) || buses.has(x.owner)
          || instruments.has(x.owner));
      window.STNKSControls?.adoptPlugins?.(plugins);
      state.project.audioPlugins = plugins;
      if (state.view === 'mixer' || state.view === 'instrument') refresh();
    } else {
      window.STBrowser?.event(type, payload);
      window.STStudio?.event(type, payload);
      window.STClips?.event?.(type, payload);
      window.STMedia?.event?.(type, payload);
    }
  };
})();
