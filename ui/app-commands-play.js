/* Commands: transport, recording and keyboard performance (chords, scales, octave, velocity). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, state, toast, mutate, toggleKeyboardPlay, toggleNoteRecordingFromUI, setPlaybackMode, playFromStart, togglePlay,
    stop, recordWithCountIn, chordSetById, cycleChordSet, setPerformance, cyclePerformance, defineCommands, definePrefixCommand
  } = app;

  function playbackMode(command) {
    const changed = setPlaybackMode(command === 'modeSong' ? 'song' : 'pattern');
    if (changed) $('#transport-mode')?.focus({ preventScroll: true });
    return changed;
  }

  function chordSet(command) {
    const set = chordSetById[command.slice(9)];
    if (!set) return;
    setPerformance({ chordSet: set.id }, false);
    toast(`Chord set · ${set.name} (${set.mood})`);
  }

  function chordSetOff() {
    setPerformance({ chordSet: '' }, false);
    return toast('Chord set off');
  }

  function midiPerformance() {
    setPerformance({ midiApply: state.midiApply === false }, false);
    return toast(state.midiApply ? 'MIDI keyboard follows key, chord and chord set' : 'MIDI keyboard plays plain notes');
  }

  // Next/previous pairs: the first name steps forward, the second back.
  const cycle = (setting, forward) => command => cyclePerformance(setting, command === forward ? 1 : -1);

  defineCommands({
    play: () => togglePlay(),
    playFromStart: () => playFromStart(),
    stop: () => stop(),
    modeSong: playbackMode,
    modePattern: playbackMode,
    metronome: () => mutate('toggle metronome', p => p.metronome = !p.metronome),
    record: () => toggleNoteRecordingFromUI(),
    recordCountIn: () => recordWithCountIn(1),
    keyboard: () => toggleKeyboardPlay(),

    chordNext: cycle('chord', 'chordNext'),
    chordPrevious: cycle('chord', 'chordNext'),
    chordSetNext: command => cycleChordSet(command === 'chordSetNext' ? 1 : -1),
    chordSetPrevious: command => cycleChordSet(command === 'chordSetNext' ? 1 : -1),
    chordSetOff,
    midiPerformance,
    scaleNext: cycle('scale', 'scaleNext'),
    scalePrevious: cycle('scale', 'scaleNext'),
    rootUp: cycle('root', 'rootUp'),
    rootDown: cycle('root', 'rootUp'),
    scaleMode: () => cyclePerformance('scaleMode'),
    velocityCycle: () => cyclePerformance('velocity'),
    octaveUp: cycle('octave', 'octaveUp'),
    octaveDown: cycle('octave', 'octaveUp'),
    editStepUp: cycle('editStep', 'editStepUp'),
    editStepDown: cycle('editStep', 'editStepUp'),

    noteRepeat: () => window.STNoteRepeat?.toggle(),
    noteRepeatMode: () => window.STNoteRepeat?.cycleMode(1),
    noteRepeatFaster: command => window.STNoteRepeat?.cycleRate(command === 'noteRepeatFaster' ? 1 : -1),
    noteRepeatSlower: command => window.STNoteRepeat?.cycleRate(command === 'noteRepeatFaster' ? 1 : -1)
  });
  definePrefixCommand('chordSet:', chordSet);
})();
