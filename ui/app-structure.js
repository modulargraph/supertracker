/* Session structure: add and remove tracks, patterns and instruments. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    clone, uid, TRACK_SANITY_LIMIT, trackLimitMessage, colors, state, instrumentDefaults, emptyPattern,
    pattern, toast, mutate
  } = app;

  function addPattern(copy = false) {
    if (state.project.patterns.length >= 256) return toast('This session supports up to 256 patterns.', 'error');
    mutate(copy ? 'duplicate pattern' : 'add pattern', p => {
      const id = Array.from({ length: 256 }, (_, i) => i).find(i => !p.patterns.some(x => x.id === i)),
        pat = copy ? clone(pattern()) : emptyPattern(id, pattern().length, p.tracks.length);
      pat.id = id;
      pat.name = copy ? pat.name + ' copy' : 'Pattern ' + String(id + 1).padStart(2, '0');
      pat.automation.forEach(l => l.id = uid());
      p.patterns.push(pat);
      p.activePattern = id;
      state.row = 0;
      // The selection belonged to the pattern that was shown before.
      state.selection = null;
    });
  }

  // New instruments are empty samplers; `preset` overrides fields (e.g. a Wavetable sampler).
  function addInstrument(type = 'sample', preset = null, label = 'add instrument and track') {
    if (state.project.tracks.length >= TRACK_SANITY_LIMIT) {
      toast(trackLimitMessage(), 'error');
      return false;
    }
    const id = state.project.instruments.length;
    const accepted = mutate(label, p => {
      const instrument = { ...instrumentDefaults(id, 'Instrument ' + String(id).padStart(2, '0'), type), ...(preset || {}) };
      p.instruments.push(instrument);
      state.track = appendInstrumentTrack(p, id, instrument.name);
      state.instrument = id;
      state.selection = null;
    });
    return accepted === false ? false : id;
  }

  // "+ Wavetable": a sampler in Wavetable mode on the built-in Saw table, ready to play.
  function addWavetableInstrument(table = 'Saw', group = 'Basic') {
    const id = state.project.instruments.length,
      preset = app.wavetableInstrument(id, `Wavetable ${String(id).padStart(2, '0')}`, table, group);
    return addInstrument('sample', preset, 'add wavetable instrument');
  }

  function appendInstrumentTrack(project, instrumentIndex, name, stableRoutingID, makeID = uid) {
    if (project.tracks.length >= TRACK_SANITY_LIMIT) throw new Error(trackLimitMessage());
    if (!Number.isInteger(instrumentIndex)
        || !project.instruments[instrumentIndex]) throw new Error('Choose a valid instrument for the new track.');
    const index = project.tracks.length,
      track = {
        id: index,
        routingID: stableRoutingID || makeID(),
        name: name || 'Track ' + String(index + 1).padStart(2, '0'),
        color: colors[index % 8],
        volume: .8,
        pan: 0,
        mute: false,
        solo: false,
        instrument: instrumentIndex,
        delaySend: 0,
        reverbSend: 0
      };
    window.STRoutingProject.appendTrack(project, track, makeID);
    project.patterns.forEach(pat => pat.steps.push(Array(pat.length).fill(null)));
    return index;
  }

  function addTrack() {
    if (state.project.tracks.length >= TRACK_SANITY_LIMIT) return toast(trackLimitMessage(), 'error');
    mutate('add track', p => {
      state.track = appendInstrumentTrack(p, state.instrument);
      // The cursor jumps to the new track: a kept anchor would stretch the selection across every track between.
      state.selection = null;
    });
  }

  function removeTrack() {
    if (state.project.tracks.length === 1) return toast('Keep at least one track in a session.');
    const index = state.track;
    mutate('remove track', p => {
      const group = window.STGroups?.ofTrack(p, index),
        removedOwner = window.STRoutingProject?.trackOwner(p.tracks[index]),
        ownsPlugin = x => x.owner === removedOwner,
        // A lane names its channel by owner, or (the engine's own track parameters) its track by index.
        ownsLane = x => x.owner ? x.owner === removedOwner : x.track === index;
      window.STRoutingProject.keepInstrumentPlugins(p, [p.tracks[index]]);
      window.STRoutingProject.removeTrackReferences(p, p.tracks[index], p.audioPlugins.filter(ownsPlugin));
      p.tracks.splice(index, 1);
      p.audioClips = p.audioClips.filter(x => x.track !== index).map(x => ({
        ...x,
        track: x.track > index ? x.track - 1 : x.track
      }));
      p.tracks.forEach((t, i) => t.id = i);
      for (const pat of p.patterns) {
        pat.steps.splice(index, 1);
        pat.automation = pat.automation.filter(l => !ownsLane(l));
        pat.automation.forEach(l => {
          if (l.track > index) l.track--;
        });
      }
      p.audioPlugins = p.audioPlugins.filter(x => !ownsPlugin(x));
      // Removing a group's last track removes the group and its bus.
      if (group && !window.STGroups.members(p, group).length) window.STGroups.dissolve(p, group.id);
      state.track = Math.min(index, p.tracks.length - 1);
      state.instrument = p.tracks[state.track].instrument;
    });
  }

  function removeInstrument() {
    if (state.project.instruments.length === 1) return toast('Keep at least one instrument in a session.');
    const index = state.instrument;
    mutate('remove instrument', p => {
      const affected = new Set(p.tracks.filter(t => t.instrument === index).map(t => window.STRoutingProject?.trackOwner(t)));
      p.audioPlugins = p.audioPlugins.filter(x => !(x.kind === 'instrument' && affected.has(x.owner)));
      // The removed instrument's insert chain goes with it; later instruments' chains follow their new index.
      window.STInstrumentEffects?.reindexAfterRemoval(p, index);
      p.instruments.splice(index, 1);
      p.instruments.forEach((x, i) => x.id = i);
      for (const t of p.tracks) t.instrument = t.instrument === index
          ? 0
          : t.instrument > index
          ? t.instrument - 1
          : t.instrument;
      // Every note cell, extra note columns included (app-note-columns.js).
      for (const pat of p.patterns) for (const steps of pat.steps) for (const step of steps) if (step) for (const cell of [step, ...(step.columns || [])]) if (cell?.instrument !== undefined) cell.instrument = cell.instrument === index
          ? 0
          : cell.instrument > index
          ? cell.instrument - 1
          : cell.instrument;
      state.instrument = Math.min(index, p.instruments.length - 1);
    });
  }

  function removePattern() {
    if (state.project.patterns.length === 1) return toast('Keep at least one pattern in a session.');
    const id = pattern().id;
    if (mutate('remove pattern', p => {
      p.patterns = p.patterns.filter(x => x.id !== id);
      if (window.STSongModel) {
        window.STSongModel.removePattern(p, id);
      } else {
        p.sequence = p.sequence.filter(x => x !== id);
        if (!p.sequence.length) p.sequence = [p.patterns[0].id];
        delete p.songLoop;
      }
      p.activePattern = p.patterns[0].id;
      state.row = 0;
      state.selection = null;
      state.sequence = Math.min(state.sequence, p.sequence.length - 1);
    }) && state.patternLoop?.pattern === id) app.clearPatternLoop();
  }

  Object.assign(app, {
    addPattern, addInstrument, addWavetableInstrument, appendInstrumentTrack, addTrack, removeTrack, removeInstrument, removePattern
  });
})();
