/* Chord sets: each key plays the next chord of a set (harmony sets and style sets),
   the chord-set strip and its MIDI mapping. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $$, clamp, esc, noteNames, noteText, state, toast } = app;
  // ---- Chord sets: each key plays the next chord of a curated set ----
  // Voicings written in C (C minor for minor sets) as semitone offsets from the
  // tonic at the playing octave (0 = C4 at octave 4). The Key root transposes a set; keys
  // past the last chord repeat the set an octave up. Each chord: [root pitch class, name
  // suffix, voicing, optional bass pitch class for slash names].
  const CHORD_SET_MIDI_BASE = 48;

  const CHORD_SET_MIDI_REGISTER = 60; // MIDI C3 plays chord 1, voiced around C4
  const toChordSet = group => ([id, name, mood, minor, list]) => ({
    id,
    name,
    mood,
    minor,
    group,
    chords: list.map(([root, suffix, notes, bass]) => ({ root, suffix, notes, bass }))
  });
  // Harmony sets: twelve chords of one key or mode, the bass note below and the chord tones
  // close above the tonic (the root is left to the bass in chords of four or more notes).
  const harmonyChordSets = [
      ['major-triads', 'Major triads', 'Basics', false, [[0, '', [-12, 4, 7, 12]], [2, 'm', [-10, 2, 5, 9]], [4, 'm', [-8, 4, 7, 11]], [5, '', [-7, 5, 9, 12]], [7, '', [-5, 2, 7, 11]], [9, 'm', [-15, 4, 9, 12]], [11, '°', [-13, 2, 5, 11]], [0, '', [-8, 4, 7, 12], 4], [5, '', [-15, 5, 9, 12], 9], [7, '', [-13, 2, 7, 11], 11], [0, 'sus2', [-12, 2, 7, 12]], [7, 'sus4', [-5, 2, 7, 12]]]],
      ['major-sevenths', 'Major sevenths', 'Basics', false, [[0, 'maj7', [-12, 4, 7, 11]], [2, 'm7', [-10, 5, 9, 12]], [4, 'm7', [-8, 2, 7, 11]], [5, 'maj7', [-7, 4, 9, 12]], [7, '7', [-5, 2, 5, 11]], [9, 'm7', [-15, 4, 7, 12]], [11, 'm7♭5', [-13, 2, 5, 9]], [0, '7', [-12, 4, 7, 10]], [5, 'm6', [-7, 2, 8, 12]], [4, '7', [-8, 2, 8, 11]], [9, '7', [-15, 4, 7, 13]], [2, '7', [-10, 6, 9, 12]]]],
      ['major-colours', 'Add9 & sus colours', 'Colour', false, [[0, 'add9', [-12, 2, 4, 7]], [2, 'm(add9)', [-10, 4, 5, 9]], [4, 'sus4', [-8, 4, 9, 11]], [5, 'add9', [-7, 7, 9, 12]], [7, 'sus4', [-5, 2, 7, 12]], [9, 'm(add9)', [-15, 4, 11, 12]], [10, 'add9', [-14, 2, 5, 12]], [0, 'sus2', [-12, 2, 7, 12]], [5, 'sus2', [-7, 5, 7, 12]], [7, 'add9', [-5, 2, 9, 11]], [9, 'sus4', [-15, 2, 4, 9]], [0, '6', [-12, 4, 7, 9]]]],
      ['major-borrowed', 'Borrowed from minor', 'Colour', false, [[0, '', [-12, 4, 7, 12]], [0, 'm', [-12, 3, 7, 12]], [1, 'maj7', [-11, 5, 8, 12]], [3, '', [-9, 3, 7, 10]], [5, 'm', [-7, 5, 8, 12]], [5, 'm6', [-7, 2, 8, 12]], [7, 'm', [-5, 2, 7, 10]], [8, '', [-16, 3, 8, 12]], [8, 'maj7', [-16, 3, 7, 12]], [10, '', [-14, 2, 5, 10]], [10, '7', [-14, 2, 5, 8]], [7, '7', [-5, 2, 5, 11]]]],
      ['major-secondary', 'Secondary dominants', 'Basics', false, [[0, '', [-12, 4, 7, 12]], [9, '7', [-15, 4, 7, 13]], [2, 'm', [-10, 2, 5, 9]], [11, '7', [-13, 3, 6, 9]], [4, 'm', [-8, 4, 7, 11]], [0, '7', [-12, 4, 7, 10]], [5, '', [-7, 5, 9, 12]], [2, '7', [-10, 6, 9, 12]], [7, '', [-5, 2, 7, 11]], [4, '7', [-8, 2, 8, 11]], [9, 'm', [-15, 4, 9, 12]], [7, '7', [-5, 2, 5, 11]]]],
      ['major-inversions', 'Bass walk inversions', 'Basics', false, [[0, '', [-12, 4, 7, 12]], [7, '', [-13, 2, 7, 11], 11], [9, 'm', [-15, 4, 9, 12]], [0, '', [-5, 4, 7, 12], 7], [5, '', [-7, 5, 9, 12]], [0, '', [-8, 4, 7, 12], 4], [2, 'm7', [-10, 5, 9, 12]], [7, 'sus4', [-5, 2, 7, 12]], [7, '', [-5, 2, 7, 11]], [5, '', [-5, 5, 9, 12], 7], [9, 'm', [-8, 4, 9, 12], 4], [4, '', [-16, 4, 8, 11], 8]]],
      ['major-mixolydian', 'Mixolydian rock', 'Modal', false, [[0, '', [-12, 4, 7, 12]], [10, '', [-14, 2, 5, 10]], [5, '', [-7, 5, 9, 12]], [7, 'm', [-5, 2, 7, 10]], [2, 'm', [-10, 2, 5, 9]], [9, 'm', [-15, 4, 9, 12]], [3, '', [-9, 3, 7, 10]], [8, '', [-16, 3, 8, 12]], [0, '7', [-12, 4, 7, 10]], [5, '', [-12, 5, 9, 12], 0], [10, '', [-12, 2, 5, 10], 0], [0, 'sus4', [-12, 5, 7, 12]]]],
      ['major-lydian', 'Lydian float', 'Modal', false, [[0, 'maj7', [-12, 4, 7, 11]], [2, '', [-10, 2, 6, 9]], [4, 'm7', [-8, 2, 7, 11]], [6, 'm7♭5', [-6, 4, 9, 12]], [7, 'maj7', [-5, 2, 6, 11]], [9, 'm7', [-15, 4, 7, 12]], [11, 'm7', [-13, 2, 6, 9]], [0, 'maj7♯11', [-12, 4, 6, 11]], [2, '', [-12, 2, 6, 9], 0], [2, 'add9', [-10, 4, 6, 9]], [4, 'm', [-8, 4, 7, 11]], [7, '6', [-5, 2, 4, 11]]]],
      ['minor-triads', 'Minor triads', 'Basics', true, [[0, 'm', [-12, 3, 7, 12]], [2, '°', [-10, 2, 5, 8]], [3, '', [-9, 3, 7, 10]], [5, 'm', [-7, 5, 8, 12]], [7, 'm', [-5, 2, 7, 10]], [8, '', [-16, 3, 8, 12]], [10, '', [-14, 2, 5, 10]], [7, '', [-5, 2, 7, 11]], [0, 'm', [-5, 3, 7, 12], 7], [5, 'm', [-12, 5, 8, 12], 0], [8, '', [-12, 3, 8, 12], 0], [10, '', [-10, 2, 5, 10], 2]]],
      ['minor-sevenths', 'Minor sevenths', 'Basics', true, [[0, 'm7', [-12, 3, 7, 10]], [2, 'm7♭5', [-10, 5, 8, 12]], [3, 'maj7', [-9, 2, 7, 10]], [5, 'm7', [-7, 3, 8, 12]], [7, 'm7', [-5, 2, 5, 10]], [8, 'maj7', [-16, 3, 7, 12]], [10, '7', [-14, 2, 5, 8]], [7, '7', [-5, 2, 5, 11]], [0, 'm(maj7)', [-12, 3, 7, 11]], [5, 'm6', [-7, 2, 8, 12]], [2, 'm7', [-10, 5, 9, 12]], [8, '7', [-16, 3, 6, 12]]]],
      ['minor-harmonic', 'Harmonic minor', 'Modal', true, [[0, 'm', [-12, 3, 7, 12]], [2, '°', [-10, 2, 5, 8]], [3, '+', [-9, 3, 7, 11]], [5, 'm', [-7, 5, 8, 12]], [7, '', [-5, 2, 7, 11]], [8, '', [-16, 3, 8, 12]], [11, '°', [-13, 2, 5, 11]], [7, '7', [-5, 2, 5, 11]], [11, '°7', [-13, 2, 5, 8]], [0, 'm(maj7)', [-12, 3, 7, 11]], [1, '', [-11, 5, 8, 13]], [7, '7♭9', [-5, 5, 8, 11]]]],
      ['minor-dorian', 'Dorian groove', 'Modal', true, [[0, 'm7', [-12, 3, 7, 10]], [2, 'm7', [-10, 5, 9, 12]], [3, 'maj7', [-9, 2, 7, 10]], [5, '7', [-7, 3, 9, 12]], [7, 'm7', [-5, 2, 5, 10]], [9, 'm7♭5', [-15, 3, 7, 12]], [10, 'maj7', [-14, 2, 5, 9]], [0, 'm9', [-12, 2, 3, 7, 10]], [5, '9', [-7, 3, 7, 9]], [0, 'm6', [-12, 3, 7, 9]], [3, '6', [-9, 7, 10, 12]], [10, '6', [-14, 2, 5, 7]]]],
      ['minor-phrygian', 'Phrygian shadows', 'Modal', true, [[0, 'm', [-12, 3, 7, 12]], [1, '', [-11, 5, 8, 13]], [3, '', [-9, 3, 7, 10]], [5, 'm', [-7, 5, 8, 12]], [7, '°', [-5, 7, 10, 13]], [8, '', [-16, 3, 8, 12]], [10, 'm', [-14, 5, 10, 13]], [1, 'maj7', [-11, 5, 8, 12]], [1, '', [-12, 5, 8, 13], 0], [5, 'm7', [-7, 3, 8, 12]], [10, 'm7', [-14, 5, 8, 13]], [0, 'sus4', [-12, 5, 7, 12]]]],
      ['minor-colours', 'Minor add9 & sus', 'Colour', true, [[0, 'm(add9)', [-12, 2, 3, 7]], [0, 'sus2', [-12, 2, 7, 12]], [5, 'm(add9)', [-7, 7, 8, 12]], [5, 'sus2', [-7, 5, 7, 12]], [7, 'sus4', [-5, 2, 7, 12]], [7, '', [-5, 2, 7, 11]], [8, 'add9', [-16, 3, 10, 12]], [10, 'sus4', [-14, 3, 5, 10]], [10, 'add9', [-14, 2, 5, 12]], [3, 'add9', [-9, 5, 7, 10]], [7, 'm(add9)', [-5, 2, 9, 10]], [0, 'sus4', [-12, 5, 7, 12]]]],
      ['minor-melodic', 'Melodic minor', 'Modal', true, [[0, 'm(maj7)', [-12, 3, 7, 11]], [2, 'm7', [-10, 5, 9, 12]], [3, '+', [-9, 3, 7, 11]], [5, '7', [-7, 3, 9, 12]], [7, '7', [-5, 2, 5, 11]], [9, 'm7♭5', [-15, 3, 7, 12]], [11, 'm7♭5', [-13, 2, 5, 9]], [0, 'm6', [-12, 3, 7, 9]], [5, '9', [-7, 3, 7, 9]], [7, '7♭9', [-5, 5, 8, 11]], [3, 'maj7', [-9, 2, 7, 10]], [0, 'm9', [-12, 2, 3, 7, 10]]]],
      ['minor-to-major', 'Minor to major', 'Colour', true, [[0, 'm', [-12, 3, 7, 12]], [8, '', [-16, 3, 8, 12]], [3, '', [-9, 3, 7, 10]], [10, '', [-14, 2, 5, 10]], [5, 'm', [-7, 5, 8, 12]], [7, '', [-5, 2, 7, 11]], [0, '', [-12, 4, 7, 12]], [5, '', [-7, 5, 9, 12]], [3, '', [-5, 3, 7, 10], 7], [8, 'maj7', [-16, 3, 7, 12]], [7, '7sus4', [-5, 2, 5, 12]], [0, '', [-8, 4, 7, 12], 4]]]].map(toChordSet('harmony'));
  // Style sets: progressions and voicings for a genre.
  const styleChordSets = [['pop', 'Pop anthem', 'Pop', false,
      [[0, '', [-12, 4, 7, 12]], [7, '', [-5, 2, 7, 11]], [9, 'm', [-3, 4, 9, 12]], [5, '', [-7, 5, 9, 12]],
          [2, 'm', [-10, 5, 9, 14]], [4, 'm', [-8, 4, 7, 11]], [7, '', [-13, 2, 7, 11], 11],
          [10, '', [-14, 5, 10, 14]], [0, 'sus2', [-12, 2, 7, 12]], [5, 'maj7', [-7, 4, 9, 12]],
          [9, 'm7', [-3, 4, 7, 12]], [7, 'sus4', [-5, 2, 7, 12]]]],
      ['ballad', 'Piano ballad', 'Ballad', false,
          [[0, '', [-12, 4, 7, 12]], [7, '', [-13, 2, 7, 11], 11], [9, 'm', [-3, 4, 9, 12]],
              [9, 'm', [-5, 4, 9, 12], 7], [5, '', [-7, 5, 9, 12]], [0, '', [-8, 4, 7, 12], 4],
              [2, 'm7', [-10, 5, 9, 12]], [7, '7sus4', [-5, 2, 5, 12]], [7, '', [-5, 2, 7, 11]],
              [4, '', [-16, 4, 8, 11], 8], [5, 'm', [-16, 5, 8, 12], 8], [5, 'add9', [-7, 0, 7, 9]],
              [4, 'm7', [-8, 2, 7, 11]], [0, '', [-17, 4, 7, 12], 7]]],
      ['neosoul', 'Neo-soul 9ths', 'Soul', false,
          [[5, 'maj9', [-7, 4, 7, 9, 12]], [4, 'm7', [-8, 2, 7, 11]], [2, 'm9', [-10, 5, 9, 12, 16]],
              [0, 'maj9', [-12, 4, 7, 11, 14]], [9, 'm9', [-15, 0, 4, 7, 11]], [2, 'm11', [-10, 0, 5, 7, 9]],
              [7, '13', [-17, 5, 9, 11, 16]], [4, '7♯9', [-8, -4, 2, 7]], [5, 'm9', [-7, 3, 7, 8, 12]],
              [8, 'maj7', [-16, 0, 3, 7]], [10, '13', [-14, -4, 2, 7]], [0, '6/9', [-12, 4, 7, 9, 14]]]],
      ['gospel', 'Soul & gospel 7ths', 'Gospel', false,
          [[0, 'maj7', [-12, 4, 7, 11]], [0, '7', [-12, 4, 7, 10]], [5, 'maj7', [-7, 4, 9, 12]],
              [6, '°7', [-6, 3, 9, 12]], [0, '', [-17, 4, 7, 12], 7], [9, '7', [-15, 4, 7, 13]],
              [2, 'm7', [-10, 5, 9, 12]], [7, '7', [-5, 5, 11, 14]], [4, 'm7', [-8, 2, 7, 11]],
              [9, 'm7', [-15, 4, 7, 12]], [5, 'm6', [-7, 2, 8, 12]], [4, '7', [-8, 2, 8, 11]],
              [10, '7', [-14, 2, 5, 8]], [7, '13sus', [-5, 5, 9, 12, 16]]]],
      ['jazz', 'Jazz ii–V–I', 'Jazz', false,
          [[2, 'm9', [-10, 5, 9, 12, 16]], [7, '13', [-17, 5, 9, 11, 16]], [0, 'maj9', [-12, 4, 7, 11, 14]],
              [0, '6/9', [-12, 4, 9, 14]], [4, 'm7', [-8, 2, 7, 11]], [9, '7♭9', [-15, 1, 7, 10, 16]],
              [1, '9', [-11, -1, 3, 5]], [7, 'm9', [-17, 5, 9, 10, 14]], [0, '13', [-12, -2, 2, 4, 9]],
              [5, 'maj9', [-7, 4, 7, 9, 12]], [5, 'm9', [-7, 3, 7, 8, 12]], [10, '13', [-14, -4, 2, 7]],
              [3, 'maj9', [-9, 2, 5, 7, 10]], [8, 'maj7', [-16, 0, 3, 7]]]],
      ['jazzminor', 'Jazz minor ii–V–i', 'Jazz', true,
          [[2, 'm7♭5', [-10, 0, 5, 8]], [7, '7♭9', [-17, -1, 5, 8]], [0, 'm9', [-12, 2, 3, 7, 10]],
              [0, 'm6', [-12, 3, 7, 9]], [0, 'm(maj7)', [-12, 3, 7, 11]], [8, 'maj7', [-16, 0, 3, 7]],
              [5, 'm9', [-7, 3, 7, 8, 12]], [10, '13', [-14, -4, 2, 7]], [3, 'maj9', [-9, 2, 5, 7, 10]],
              [9, 'm7♭5', [-15, 0, 3, 7]], [2, '7♭9', [-10, 0, 3, 6]], [7, 'm9', [-17, -2, 2, 5, 9]],
              [7, '7alt', [-17, -1, 3, 5, 10]]]],
      ['lofi', 'Lo-fi dreams', 'Lo-fi', false,
          [[5, 'maj9', [-7, 4, 7, 9, 12]], [4, 'm7', [-8, 2, 7, 11]], [2, 'm9', [-10, 5, 9, 12, 16]],
              [0, 'maj7', [-12, 4, 7, 11]], [9, 'm9', [-15, 0, 4, 7, 11]], [7, '9sus', [-5, 5, 9, 12]],
              [4, '7♭9', [-8, 2, 5, 8, 11]], [11, 'm7♭5', [-13, 2, 5, 9]], [10, 'maj7', [-14, 2, 5, 9]],
              [5, 'm7', [-7, 3, 8, 12]], [7, 'm9', [-17, -2, 2, 5, 9]], [0, '9', [-12, -2, 2, 4, 7]],
              [3, 'maj7', [-9, 2, 7, 10]]]],
      ['deephouse', 'Deep house', 'House', true,
          [[0, 'm9', [-12, 3, 7, 10, 14]], [0, 'm11', [-12, -2, 3, 5, 7]], [8, 'maj9', [-16, 0, 3, 7, 10]],
              [10, 'add9', [-14, 0, 2, 5]], [5, 'm9', [-7, 3, 7, 8, 12]], [5, 'm7', [-7, 3, 8, 12]],
              [3, 'maj7', [-9, 2, 7, 10]], [3, '6', [-9, 0, 3, 7]], [7, 'm7', [-17, -2, 2, 5]],
              [7, 'm11', [-17, 0, 5, 10]], [10, '9sus', [-14, 0, 3, 8]], [1, 'maj7', [-11, 0, 5, 8]]]],
      ['stabs', 'Chord-memory stabs', 'House', true,
          [[0, 'm7', [0, 7, 10, 15]], [3, 'm7', [-9, -2, 1, 6]], [5, 'm7', [-7, 0, 3, 8]], [7, 'm7', [-5, 2, 5, 10]],
              [8, 'm7', [-4, 3, 6, 11]], [10, 'm7', [-2, 5, 8, 13]], [2, 'm7', [-10, -3, 0, 5]],
              [9, 'm7', [-3, 4, 7, 12]], [0, 'm9', [0, 7, 10, 14, 15]], [5, 'm9', [-7, 0, 3, 7, 8]]]],
      ['cinematic', 'Cinematic epic', 'Cinematic', true,
          [[0, 'm', [-12, -5, 0, 3, 7]], [8, '', [-16, -4, 0, 3, 8]], [3, '', [-9, -2, 3, 7]],
              [10, '', [-14, -2, 2, 5]], [5, 'm', [-7, 0, 5, 8]], [7, 'sus4', [-5, 0, 2, 7]], [7, '', [-5, -1, 2, 7]],
              [0, 'sus2', [-12, -5, 0, 2, 7]], [8, 'add9', [-16, -2, 0, 3]], [5, 'm', [-16, 0, 5, 8], 8],
              [1, '', [-11, -4, 1, 5]], [4, '', [-8, -1, 4, 8]]]],
      ['dark', 'Dark minor', 'Dark', true,
          [[0, 'm', [-12, 0, 3, 7]], [5, 'm', [-7, 0, 5, 8]], [7, '7', [-5, -1, 2, 5]], [8, '', [-16, 0, 3, 8]],
              [2, '°', [-10, 2, 5, 8]], [11, '°7', [-13, 2, 5, 8]], [0, 'm(maj7)', [-12, 3, 7, 11]],
              [1, '', [-7, 1, 5, 8], 5], [3, '+', [-9, 3, 7, 11]], [5, 'm(add9)', [-7, 0, 7, 8]],
              [7, '7♭9', [-5, -1, 5, 8]], [8, 'maj7', [-16, 0, 3, 7]], [0, 'm', [-17, 0, 3, 7], 7]]],
      ['trap', 'Trap & drill', 'Trap', true,
          [[0, 'm', [-12, 0, 3, 7]], [1, '', [-11, 1, 5, 8]], [8, 'maj7', [-16, 0, 3, 7]], [7, 'm', [-17, -2, 2, 7]],
              [5, 'm', [-7, 0, 5, 8]], [10, 'm', [-14, -2, 1, 5]], [0, 'm(add9)', [-12, 2, 3, 7]],
              [7, '°', [-17, -2, 1, 7]], [1, 'maj7', [-11, 0, 5, 8]], [5, 'm9', [-7, 3, 7, 8, 12]],
              [0, 'm7', [-12, -2, 3, 7]]]],
      ['synthwave', 'Synthwave nights', 'Synthwave', true,
          [[0, 'm(add9)', [-12, 2, 3, 7]], [8, 'maj7', [-16, 0, 3, 7]], [3, 'add9', [-9, -2, 5, 7]],
              [10, '6', [-14, 2, 5, 7]], [5, 'm9', [-7, 3, 7, 8, 12]], [7, 'sus4', [-5, 0, 2, 7]],
              [7, '', [-5, -1, 2, 7]], [1, 'maj7', [-11, 0, 5, 8]], [8, '', [-12, 0, 3, 8], 0],
              [10, '', [-10, -2, 2, 5], 2], [0, 'm7', [-12, -2, 3, 7]], [3, 'maj7', [-9, -2, 2, 7]]]],
      ['ambient', 'Ambient lydian', 'Ambient', false,
          [[0, 'maj7♯11', [-12, -5, 4, 6, 11]], [2, '', [-12, 2, 6, 9], 0], [0, 'add9', [-12, -5, 2, 4, 7]],
              [4, 'm7', [-8, -1, 2, 7]], [7, 'maj7', [-5, -1, 2, 6]], [11, 'm7', [-13, -3, 2, 6]],
              [9, 'm9', [-15, -1, 0, 4, 7]], [2, 'add9', [-10, -3, 4, 6]], [6, 'm7', [-6, 1, 4, 9]],
              [4, 'sus4', [-8, -1, 4, 9]], [0, 'sus2', [-12, -5, 0, 2, 7]], [7, '6', [-5, -1, 2, 4]]]],
      ['funk', 'Blues & funk', 'Funk', false,
          [[0, '9', [-12, 4, 10, 14]], [0, '13', [-12, 4, 9, 10]], [5, '9', [-7, 3, 7, 9]], [5, '13', [-7, 3, 9, 14]],
              [0, '7♯9', [-12, 4, 10, 15]], [7, '9', [-5, 5, 9, 11]], [8, '9', [-16, 0, 6, 10]],
              [7, '7♯9', [-5, -1, 5, 10]], [10, '9', [-14, 2, 8, 12]], [3, '9', [-9, 1, 5, 7]],
              [0, 'm7', [-12, 3, 7, 10]], [6, '°7', [-6, 3, 9, 12]]]]].map(toChordSet('style'));

  const chordSets = [...harmonyChordSets, ...styleChordSets];
  const chordSetById = Object.fromEntries(chordSets.map(set => [set.id, set]));
  const flatNoteNames = ['C', 'D♭', 'D', 'E♭', 'E', 'F', 'G♭', 'G', 'A♭', 'A', 'B♭', 'B'];

  // A saved chord-set ID ('' when it names no chord set).
  function chordSetIdFor(id) {
    return chordSetById[id] ? id : '';
  }

  function activeChordSet() {
    return state.chordSet && chordSetById[state.chordSet] || null;
  }

  // Keep transposed sets centred: roots above F♯ move down rather than up.
  function chordSetShift() {
    return state.root > 6 ? state.root - 12 : state.root;
  }

  function chordSetPitchName(pc, set) {
    const flats = set.minor ? [0, 2, 3, 5, 7, 8, 10] : [0, 1, 3, 5, 6, 8, 10];
    return (flats.includes(state.root) ? flatNoteNames : noteNames)[((pc + state.root) % 12 + 12) % 12];
  }

  function chordSetSlot(set, index) {
    return (index % set.chords.length + set.chords.length) % set.chords.length;
  }

  function chordSetChordName(set, index) {
    const chord = set.chords[chordSetSlot(set, index)];
    return chordSetPitchName(chord.root, set)
        + chord.suffix
        + (chord.bass != null ? '/' + chordSetPitchName(chord.bass, set) : '');
  }

  // Absolute pitches (ascending) for chord-set key `index` (0 = chord 1) voiced around `base`.
  function chordSetPitches(index, set = activeChordSet(), base = (state.octave + 1) * 12) {
    const slot = chordSetSlot(set, index),
      octave = Math.floor(index / set.chords.length);
    return [...new Set(set.chords[slot].notes.map(n => clamp(base
        + chordSetShift()
        + n
        + 12 * octave,
        0, 127)))].sort((a, b) => a - b);
  }

  function chordSetKeyLabel(index) {
    const key = Object.keys(app.keyNotes).find(k => app.keyNotes[k] === index);
    return key ? key.toUpperCase() : '';
  }

  // Chord sets cycle Off → each set → Off, so one shortcut both browses and leaves the mode.
  function cycleChordSet(direction = 1) {
    const ids = ['', ...chordSets.map(set => set.id)],
      next = ids[(ids.indexOf(activeChordSet() ? state.chordSet : '') + direction + ids.length) % ids.length];
    app.setPerformance({ chordSet: next }, false);
    const set = activeChordSet();
    toast(set ? `Chord set · ${set.name} (${set.mood}) · ${set.chords.slice(0, 4).map((_, i) => chordSetChordName(set, i)).join(' ')}`
        + ` …` : 'Chord set off');
  }

  function chordSetStrip() {
    const set = activeChordSet();
    if (!set) return '';
    return `<div class="chord-set-strip" id="chord-set-strip" role="group" aria-label="Chord set ${esc(set.name)}: keys `
        + `play chords"><span class="chord-set-name" title="Chord set · ⌥G next · ⌥⇧G previous · Key transposes">${esc(set.name)}`
        + ` <small>${esc(set.mood)} · ${esc(chordSetPitchName(0, set))}${set.minor ? ' minor' : ''}</small>`
        + `</span>${set.chords.map((_, i) => {
      const label = chordSetKeyLabel(i),
        notes = chordSetPitches(i, set).map(noteText).join(' ');
      return `<span class="chord-chip" data-chord-chip="${i}" title="${esc(`${label ? 'Key ' + label + ' · ' : ''}MIDI ${noteText(CHORD_SET_MIDI_BASE + i)} · ${notes}`)}">`
          + `<kbd>${esc(label || String(i + 1))}</kbd>${esc(chordSetChordName(set, i))}</span>`;
    }).join('')}</div>`;
  }

  function chordChipHighlight(slot) {
    if (slot == null) return;
    const on = [...app.held.values()].some(entry => entry.slot === slot);
    for (const el of $$(`[data-chord-chip="${slot}"]`)) el.classList.toggle('held', on);
  }

  Object.assign(app, {
    CHORD_SET_MIDI_BASE, CHORD_SET_MIDI_REGISTER, chordSets, chordSetById, chordSetIdFor, activeChordSet, chordSetPitchName,
    chordSetSlot, chordSetChordName, chordSetPitches, cycleChordSet, chordSetStrip, chordChipHighlight
  });
})();
