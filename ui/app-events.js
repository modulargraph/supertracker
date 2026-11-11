/* Host events: everything the host pushes to the page arrives in window.nativeEvent()
   (app-bridge.js hands it the events host-transport.js receives). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, state, receiveSampleMetadata, toast, refresh, receiveNoteRecordingStatus,
    updateTransport, receiveCountInStatus, updatePlayhead, pianoHighlight, projectAction,
    perform, receiveAcknowledgedMIDI, endRecordTake, liveRecordGroups, recordIncomingMIDI
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
    // An operation in flight (an instrument edit, a render) claims the events that belong to it.
    if (window.STNativeEvents.consume(type, payload)) return;
    window.STMK3?.event?.(type, payload);
    window.STRecording?.event(type, payload);
    window.STExport?.event?.(type, payload);
    window.STFreeze?.event?.(type, payload);
    window.STNKSControls?.event(type, payload);
    if (type === 'sampler.playheads') { window.STSamplerPlayheads?.receive(payload); return; }
    if (type === 'status') {
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
    } else if (type === 'app.quitRequested') {
      app.hostQuitRequested?.();
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
    } else if (type === 'sample.loaded') {
      receiveSampleMetadata(payload.instrument, payload.sample);
    } else if (type === 'sample.error') {
      toast(payload.error || payload.message || 'A referenced sample could not be loaded', 'error');
    } else if (type === 'plugin.error') {
      toast(payload.error || payload.message || 'Plug-in could not be restored', 'error');
    } else if (type === 'plugins.recovery') {
      // A plug-in stopped working while the project was open: offer the review (and Retry) at once,
      // unless another dialog is open; "Review interrupted plug-ins" shows the same report later.
      if (!state.modal && payload?.suspects?.some(item => item.requiresReview)) window.STPluginRecovery?.show(payload);
    } else if (type === 'audio.devices') {
      // Device restores, hot-plugs and live input changes: the mixer and Recording page show the current state.
      state.audio = { ...state.audio, ...payload };
      if (state.view === 'mixer' || state.view === 'recording') refresh();
    } else if (type === 'midi.status') {
      if (payload.devices) state.midi = payload.devices;
      if (payload.type === 'midiError') toast(payload.message, 'error');
    } else {
      window.STBrowser?.event(type, payload);
      window.STStudio?.event(type, payload);
      window.STClips?.event?.(type, payload);
      window.STMedia?.event?.(type, payload);
    }
  };
})();
