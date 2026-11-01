/* Commands: editing patterns, tracks and instruments (history, selection, structure, renames,
   pattern tools, instrument pages and sources). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, state, pattern, currentInstrument, toast, request, mutate, undo, setView, refresh, addPattern, addInstrument,
    addTrack, removeTrack, removeInstrument, removePattern, shiftPatternRows, loopSelection, patternTool, fillSelection,
    textPrompt, changeInstrumentType, assignInstrument, isTextInput, usesNativeTextUndo, defineCommands,
    definePrefixCommand
  } = app;

  const showView = view => { if (state.view !== view) setView(view); };

  // ⌘Z / ⇧⌘Z: a focused text field keeps its own undo stack.
  function undoCommand(command) {
    if (usesNativeTextUndo(document.activeElement)) return document.execCommand(command);
    return command === 'redo' ? undo(true) : undo();
  }

  function selectAll() {
    if (isTextInput(document.activeElement)) return document.execCommand('selectAll');
    if (state.view === 'pattern') {
      state.selection = { track: 0, row: 0 };
      state.track = state.project.tracks.length - 1;
      state.row = pattern().length - 1;
      refresh();
    }
  }

  function addInstrumentCommand() {
    if (addInstrument() === false) return false;
    return setView('instrument');
  }

  function addWavetableCommand() {
    if (app.addWavetableInstrument() === false) return false;
    return setView('instrument');
  }

  function loadSample() {
    setView('instrument');
    return $('#load-sample')?.click();
  }

  function shiftRows(command) {
    showView('pattern');
    return shiftPatternRows(command === 'insertRow');
  }

  function follow() {
    state.follow = !state.follow;
    toast(state.follow ? 'Follow playback on' : 'Follow playback off');
    return state.view === 'pattern' ? refresh() : undefined;
  }

  function loopSelectionCommand() {
    if (state.view !== 'pattern' && !state.patternLoop) setView('pattern');
    return loopSelection();
  }

  function focusField(command) {
    if (command === 'patternLength') showView('pattern');
    const field = $(command === 'setTempo' ? '#tempo' : '#pattern-length');
    field?.focus();
    field?.select?.();
    return !!field;
  }

  function renamePattern() {
    if (state.view !== 'pattern' && state.view !== 'song') setView('pattern');
    const id = state.project.activePattern;
    return textPrompt('Rename pattern', pattern().name, name => mutate('rename pattern', p => {
      const target = p.patterns.find(x => x.id === id);
      if (target) target.name = name;
    }));
  }

  // The prompt stays open while AI control or MK3 can add, delete or reorder tracks and instruments:
  // rename the one it was opened for (by identity), never whatever sits at its old index.
  function renameTrack() {
    const track = state.project.tracks[state.track];
    return textPrompt('Track name', track?.name || '', name => mutate('rename track', p => {
      if (track && p.tracks.includes(track)) track.name = name;
    }));
  }

  function renameInstrument() {
    const instrument = currentInstrument();
    return textPrompt('Instrument name', instrument?.name || '', name => mutate('rename instrument', p => {
      if (instrument && p.instruments.includes(instrument)) instrument.name = name;
    }));
  }

  function instrumentType(command) {
    showView('instrument');
    const type = command.slice(15),
      sourceProject = state.project;
    return Promise.resolve(changeInstrumentType(type)).finally(() => {
      if (!state.modal && state.view === 'instrument' && state.project === sourceProject) $('#instrument-type-' + type)?.focus({ preventScroll: true });
    });
  }

  function sampleCommand(command) {
    showView('instrument');
    return window.STSampler?.command?.(command.slice(7));
  }

  function instrumentPage(command) {
    showView('instrument');
    const pages = window.STInstrumentPages;
    if (!pages?.list().length) {
      toast('Pages apply to Sampler instruments.');
      return false;
    }
    if (command === 'instrumentPageNext' || command === 'instrumentPagePrevious') {
      return pages.step(command === 'instrumentPageNext' ? 1 : -1);
    }
    const index = Number(command.slice(14));
    if (index > pages.list().length) {
      toast(`This instrument has ${pages.list().length} pages.`);
      return false;
    }
    return pages.show(index);
  }

  // ⌘[ ⌘]: the previous / next pattern in the list (wrapping, like [ ] in the automation view, which keeps its parameter).
  function stepPattern(command) {
    const delta = command === 'nextPattern' ? 1 : -1;
    if (state.view === 'automation') return app.perform(delta > 0 ? 'automationPatternNext' : 'automationPatternPrev');
    if (state.view === 'pattern' && app.songScrollOn?.()) return app.songScrollSlot(delta);
    const list = state.project.patterns;
    if (list.length < 2) {
      toast('This session has one pattern.');
      return false;
    }
    const index = Math.max(0, list.findIndex(x => x.id === state.project.activePattern)),
      next = list[(index + delta + list.length) % list.length];
    return app.selectPattern(next.id);
  }

  // ⇧⌘[ ⇧⌘]: the previous / next instrument (as a click in the instrument list).
  function stepInstrumentCommand(command) {
    const count = state.project.instruments.length;
    if (count < 2) {
      toast('This session has one instrument.');
      return false;
    }
    const delta = command === 'nextInstrument' ? 1 : -1;
    return app.selectInstrument((state.instrument + delta + count) % count);
  }

  function findControl() {
    showView('instrument');
    return window.STInstrumentNavigation?.show();
  }

  function revealSample() {
    const path = currentInstrument()?.samplePath;
    if (!path) return toast('This instrument has no sample file to show.');
    return request('app.external', { path, reveal: true }).catch(e => toast(e.message, 'error'));
  }

  // Pattern tools (pattern-tools.js) edit the visible pattern; patternTool() brings it forward.
  const PATTERN_TOOLS = ['muteTrack', 'soloTrack', 'clearMuteSolo', 'automateParameter', 'interpolate', 'humanize',
    'randomizeVelocity', 'louder', 'softer', 'reverseSteps', 'nudgeUp', 'nudgeDown', 'stretch', 'compress',
    'doublePattern', 'halvePattern', 'clearEffects', 'stepEffect'];
  for (const name of PATTERN_TOOLS) defineCommands({ [name]: patternTool });
  definePrefixCommand('quickFX:', patternTool);

  defineCommands({
    undo: undoCommand,
    redo: undoCommand,
    selectAll,
    fill: () => fillSelection(),
    insertRow: shiftRows,
    deleteRow: shiftRows,
    follow,
    loopSelection: loopSelectionCommand,
    setTempo: focusField,
    patternLength: focusField,

    addTrack: () => addTrack(),
    removeTrack: () => removeTrack(),
    addPattern: () => addPattern(),
    duplicatePattern: () => addPattern(true),
    removePattern: () => removePattern(),
    addInstrument: addInstrumentCommand,
    addWavetable: addWavetableCommand,
    removeInstrument: () => removeInstrument(),
    assignInstrument: () => assignInstrument(),
    loadSample,

    renamePattern,
    renameTrack,
    renameInstrument,

    findControl,
    instrumentPage1: instrumentPage,
    instrumentPage2: instrumentPage,
    instrumentPage3: instrumentPage,
    instrumentPage4: instrumentPage,
    instrumentPage5: instrumentPage,
    instrumentPageNext: instrumentPage,
    instrumentPagePrevious: instrumentPage,
    revealSample,
    previousPattern: stepPattern,
    nextPattern: stepPattern,
    previousInstrument: stepInstrumentCommand,
    nextInstrument: stepInstrumentCommand,

    noteOff: () => app.insertNoteOff(false),
    noteCut: () => app.insertNoteOff(true),
    toggleGateColumn: () => app.toggleGateColumn(),
    removeGates: () => {
      showView('pattern');
      return app.removeGates();
    },

    freezeTrack: () => window.STFreeze?.freeze(state.track),
    unfreezeTrack: () => window.STFreeze?.unfreeze(state.track),
    renderSelection: () => window.STRenderSelection?.open()
  });
  definePrefixCommand('instrumentType:', instrumentType);
  definePrefixCommand('sample:', sampleCommand);
})();
