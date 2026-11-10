/* Boot and the public surface: window.ST (used by every other module, MK3 and the agent API)
   and window.__supertracker (used by native code). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, clone, uid, esc, noteText, views, FX, state, setRecordPlacement, instrumentDefaults, blankProject,
    demoProject, normalize, pattern, beatsPerBar, toast, request, acceptProject, scheduleAutosave, mutate,
    bindScalarField, waitForEffectMove, waitForEngineUpdate, cancelPendingEngineSync, syncEngineProject,
    syncEngine, setView, refresh, setNoteRecording, updateTransport, recordWithCountIn, selectPattern,
    selectInstrument, addInstrument, patternLoopFor, loopSelection, appendInstrumentTrack, selectedBounds,
    copySelection, pasteSelection, clearSelection, fillSelection, auditionStep, transpose, chords, scales,
    scaleNames, chordSets, chordSetIdFor, activeChordSet, chordSetChordName, chordSetPitches, performanceSummary,
    readPerformance, setPerformance, cycleChordSet, midiPerformanceMap, cyclePerformance, noteOn, noteOff,
    stopHeld, parameter, loadSample, loadSampleAsNew, editSample, openModal, closeModal, askText,
    confirmChoice, textPrompt, captureNativeEdits, projectAction, showHelp, commands, openPalette, browseNKS,
    perform, showSettings, songBeat, showRecording, showPluginParameters, automate, contextMenu,
    midiFileAction, recordLiveStep, finishLiveRecord, clipboardEvent, automateValue, keyboard
  } = app;

  async function boot() {
    document.body.classList.toggle('native', state.native);
    state.project = blankProject();
    // A placeholder until the host hands over the startup session (often recovered unsaved work): it must never
    // reach the recovery copy, which still holds that session (app-session.js autosave, the quit checkpoint).
    app.startupPending = state.native;
    refresh();
    document.addEventListener('keydown', keyboard);
    for (const type of ['copy', 'cut', 'paste']) document.addEventListener(type, clipboardEvent);
    document.addEventListener('keyup', e => noteOff(e.code));
    document.addEventListener('contextmenu', contextMenu);
    window.addEventListener('blur', stopHeld);
    try {
      const data = await request('ready');
      // The host lists what it implements, so the page can keep its own fallback for the rest.
      state.hostCommands = Array.isArray(data?.commands) ? new Set(data.commands) : null;
      if (data?.project) state.project = normalize(data.project);
      app.startupPending = false;
      readPerformance(state.project);
      state.dirty = !!data?.dirty;
      state.plugins = data?.plugins || [];
      state.audio = data?.audio || {};
      state.midi = data?.midi || {};
      state.library.roots = data?.roots || [];
      state.engineReady = true;
      refresh();
      const project = state.project;
      try {
        // The host reports a startup session whose audio could not be prepared (audioError): retry it as a full replacement.
        if ((await syncEngineProject(() => state.project === project
            ? clone(project)
            : null, { replace: !!data?.audioError })) === false) throw new Error('Startup audio preparation was canceled. Press Play to retry.');
      } catch (error) {
        toast('Audio could not be prepared: ' + error.message, 'error');
      }
      // Keep project opening and recovery available when the desired graph needs repair.
      if (data?.warning) toast(data.warning, 'error');
      app.acceptingOpenRequests = true;
      const startupPath = app.queuedOpenPath || data?.openPath;
      app.queuedOpenPath = null;
      if (startupPath) await projectAction('open', startupPath);
      if (data?.pluginRecovery?.suspects?.some(item => item.requiresReview)) await window.STPluginRecovery?.show(data.pluginRecovery);
    } catch (e) {
      toast(e.message, 'error');
    }
    if (!state.modal) $('#tracker')?.focus({ preventScroll: true });
  }

  window.ST = {
    state,
    setRecordPlacement,
    selectInstrument,
    instrumentDefaults,
    bindScalarField,
    selectPattern,
    movePattern: (...args) => app.movePattern(...args),
    // Pattern view navigation: the pattern matrix, song scroll and the automation panel under the grid.
    setPatternMatrixTab: (tab, focus = false) => app.setPatternMatrixTab(tab, focus),
    patternMatrixAction: name => app.patternMatrixAction(name),
    songScrollOn: () => app.songScrollOn(),
    setSongScroll: on => app.setSongScroll(on),
    automationPanelScope: () => app.automationPanelScope(),
    setAutomationPanel: scope => app.setAutomationPanel(scope),
    writeAutomation: (...args) => app.writeAutomation(...args),
    FX,
    boot,
    recordWithCountIn,
    setPerformance,
    cyclePerformance,
    performanceSummary,
    scales,
    scaleNames,
    chords,
    chordSets,
    chordSetIdFor,
    activeChordSet,
    chordSetChordName,
    chordSetPitches,
    cycleChordSet,
    midiPerformanceMap,
    commands,
    request,
    longRequest: (...args) => app.longRequest(...args),
    hostImplements: (...args) => app.hostImplements(...args),
    get isChangingSession() {
      return app.savingProject || app.sessionTransition;
    },
    /** True while the window shows the boot placeholder instead of the host's startup session. */
    get startupPending() {
      return !!app.startupPending;
    },
    waitForGraphEdit: waitForEffectMove,
    waitForEngineUpdate,
    updateTransport,
    markDirty: () => {
      state.dirty = true;
      updateTransport();
    },
    scheduleAutosave,
    askText,
    confirmChoice,
    captureNativeEdits,
    syncEngine,
    syncEngineProject,
    cancelPendingEngineSync,
    liveInstParam: (...args) => app.liveInstParam(...args),
    toast,
    mutate,
    refresh,
    uid,
    escapeHTML: esc,
    loadSample,
    loadSampleAsNew,
    addInstrument,
    addWavetableInstrument: (...args) => app.addWavetableInstrument(...args),
    wavetablePath: (...args) => app.wavetablePath(...args),
    appendInstrumentTrack,
    automate,
    showPluginParameters,
    setView,
    stopHeld,
    setNoteRecording,
    parameter,
    pattern,
    perform,
    editSample,
    openModal,
    closeModal,
    showSettings,
    showRecording,
    songBeat,
    beatsPerBar,
    demoProject,
    normalize,
    acceptProject,
    noteOn,
    noteOff,
    auditionStep,
    copySelection,
    pasteSelection,
    fillSelection,
    transpose,
    keyboard,
    clipboardEvent,
    recordLiveStep,
    finishLiveRecord,
    showHelp,
    projectAction,
    midiFileAction,
    clearSelection,
    selectedBounds,
    textPrompt,
    openPalette,
    views,
    loopSelection,
    patternLoopFor,
    browseNKS,
    automateValue,
    LEGACY_GATE: app.LEGACY_GATE,
    stepGate: step => app.stepGate(step),
    gateText: step => app.gateText(step),
    gateColumnShown: (...args) => app.gateColumnShown(...args),
    setGateColumn: shown => app.setGateColumn(shown),
    insertNoteOff: cut => app.insertNoteOff(cut),
    removeGates: () => app.removeGates()
  };

  window.__supertracker = {
    state,
    getProject: () => clone(state.project),
    setProject: acceptProject,
    command: perform,
    noteText,
    FX,
    scales,
    chords,
    normalize,
    demoProject
  };
})();
