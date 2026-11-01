/* Track groups in the pattern editor: group header actions (collapse, rename, bus mute/solo),
   group commands (group the selected tracks, ungroup, move tracks, add a member) and Pad keys.
   The project model lives in track-groups.js (window.STGroups). */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, clamp, state, toast, mutate, refresh, updateTransport } = app;
  const G = () => window.STGroups;

  state.padKeys = (() => {
    try { return localStorage.getItem('st.padKeys') === '1'; } catch { return false; }
  })();

  const cursorGroup = () => G()?.ofTrack(state.project, state.track) || null;

  /* Collapse state is a display preference: it is saved with the session but adds no Undo
     step, and it is written into the Undo/Redo snapshots so Undo never flips the view. */
  function setViewPreference(groupID, key, value) {
    let changed = false;
    for (const p of [state.project, ...state.undo.map(x => x.project), ...state.redo.map(x => x.project)]) {
      const group = G()?.byID(p, groupID);
      if (group && group[key] !== value) {
        group[key] = value;
        if (p === state.project) changed = true;
      }
    }
    if (changed) {
      state.dirty = true;
      updateTransport();
      app.scheduleAutosave?.();
    }
    return changed;
  }

  function setCollapsed(groupID, collapsed, options = {}) {
    const group = G()?.byID(state.project, groupID);
    if (!group) return false;
    setViewPreference(groupID, 'collapsed', !!collapsed);
    const members = G().members(state.project, group);
    if (!collapsed && Number.isInteger(options.track) && members.includes(options.track)) state.track = options.track;
    if (collapsed && members.includes(state.track)) state.column = 0;
    state.instrument = state.project.tracks[state.track]?.instrument || 0;
    if (options.refresh !== false) refresh();
    return true;
  }

  function toggleCollapse(groupID = cursorGroup()?.id) {
    const group = G()?.byID(state.project, groupID);
    if (!group) return toast('Move the cursor to a group to collapse or expand it.');
    return setCollapsed(group.id, !group.collapsed);
  }

  function toggleBus(groupID, property) {
    const group = G()?.byID(state.project, groupID);
    if (!group || !['mute', 'solo'].includes(property)) return false;
    return mutate(`${property} group`, p => {
      const bus = G().ensureBus(p, G().byID(p, groupID));
      bus[property] = !bus[property];
    });
  }

  function renameGroup(groupID = cursorGroup()?.id) {
    const group = G()?.byID(state.project, groupID);
    if (!group) return toast('Move the cursor to a group to rename it.');
    const project = state.project;
    app.textPrompt('Group name', group.name, name => state.project === project
        && mutate('rename group', p => G().rename(p, groupID, name)));
  }

  // Indices of the selected tracks (selection range or the cursor track).
  function selectedTracks() {
    const b = app.selectedBounds() || app.cursorBounds();
    return Array.from({ length: b.t1 - b.t0 + 1 }, (_, i) => b.t0 + i);
  }

  function groupTracks(indices = selectedTracks(), options = {}) {
    const p = state.project;
    if (!indices.length) return false;
    const legacy = p.routing && p.routing.mixModel !== 'standard';
    const commit = () => mutate('group tracks', q => {
      const { group, map } = G().create(q, indices, { name: options.name || `Group ${G().list(q).length + 1}`, makeID: app.uid });
      state.track = map[state.track] ?? state.track;
      state.selection = null;
      state.lastGroup = group.id;
    });
    if (!legacy) return commit();
    return app.confirmChoice('Enable standard routing?',
        'Group buses use standard routing: track faders follow their insert effects and the master gain follows the routing graph. Undo restores the current mix.',
        'Enable and group').then(ok => ok && state.project === p ? commit() : false);
  }

  function ungroupTrack(index = state.track) {
    if (!G()?.ofTrack(state.project, index)) return toast('The selected track is not in a group.');
    return mutate('ungroup track', p => {
      const map = G().removeTrack(p, index);
      state.track = map[index];
    });
  }

  function dissolveGroup(groupID = cursorGroup()?.id) {
    if (!G()?.byID(state.project, groupID)) return toast('Move the cursor to a group to dissolve it.');
    return mutate('dissolve group', p => G().dissolve(p, groupID));
  }

  function moveTrack(delta) {
    const from = state.track, before = state.project.tracks[from]?.groupID;
    if (!state.project.tracks[from]) return false;
    const project = state.project, snapshot = JSON.stringify([project.tracks.map(t => t.routingID + ':' + (t.groupID || ''))]);
    let moved = false;
    mutate('move track', p => {
      const map = G().moveTrack(p, from, delta);
      state.track = map[from];
      state.selection = null;
      moved = JSON.stringify([p.tracks.map(t => t.routingID + ':' + (t.groupID || ''))]) !== snapshot || p.tracks[state.track]?.groupID !== before;
      if (!moved) throw new Error('unchanged');
    });
    return moved;
  }

  // A new empty sampler track at the end of the cursor's group (a new pad).
  function addGroupTrack(groupID = cursorGroup()?.id) {
    const group = G()?.byID(state.project, groupID);
    if (!group) return toast('Move the cursor to a group to add a track to it.');
    const id = state.project.instruments.length;
    return mutate('add track to group', p => {
      const instrument = { ...app.instrumentDefaults(id, `${group.name} ${G().members(p, groupID).length + 1}`), type: 'sample' };
      p.instruments.push(instrument);
      const index = app.appendInstrumentTrack(p, id, instrument.name);
      const map = G().addTrack(p, groupID, index);
      const track = p.tracks[map[index]];
      const used = new Set(G().members(p, groupID).map(i => p.tracks[i].pad).filter(Number.isInteger));
      const pad = Array.from({ length: 16 }, (_, i) => i).find(i => !used.has(i));
      if (pad !== undefined) track.pad = pad;
      state.track = map[index];
      state.instrument = id;
      state.selection = null;
    });
  }

  /* Pad keys: Z S X D C V … (and the Q row continuing) pick the members of the cursor's
     group. A key auditions that member at its pad note and, while recording, enters the
     note on that member's row. Off: keys play pitch on the cursor track. */
  const padKeysActive = () => !!state.padKeys && !!cursorGroup();

  function setPadKeys(value = !state.padKeys) {
    app.stopHeld?.();
    state.padKeys = !!value;
    try { localStorage.setItem('st.padKeys', state.padKeys ? '1' : '0'); } catch {}
    toast(state.padKeys ? 'Pad keys on · Z S X D C V … play and enter the group’s tracks.' : 'Pad keys off · keys play pitch on the cursor track.');
    refresh();
    return state.padKeys;
  }

  // The member a pad index addresses in a group (pads follow track order).
  const padMember = (group, pad) => G().members(state.project, group)[pad];

  function playPad(pad, key, options = {}) {
    const group = options.group || cursorGroup();
    const member = group ? padMember(group, pad) : undefined;
    if (member === undefined) return false;
    const track = state.project.tracks[member];
    state.track = member;
    state.instrument = track.instrument || 0;
    state.column = 0;
    state.noteColumn = 0;
    app.noteOn(Number.isInteger(track.padNote) ? track.padNote : 60, key, options.record !== false, track.instrument || 0,
        member, false, options.velocity ?? state.keyVelocity, options.repeatable !== false);
    if (!state.playing) refresh();
    return true;
  }

  // Called by noteOn for computer-keyboard notes: true when Pad keys handled the note.
  function padKeyNote(note, key, record, velocity) {
    if (!padKeysActive()) return false;
    const pad = note - (state.octave + 1) * 12;
    if (!Number.isInteger(pad) || pad < 0) return true;
    playPad(pad, key, { record, velocity });
    return true;
  }

  // Entering notes on a collapsed group needs Pad keys (or expanding it): the overview is read-only.
  function blocksRecording(track = state.track) {
    const group = G()?.ofTrack(state.project, track);
    return !!group?.collapsed && !state.padKeys;
  }

  function bindPatternGroups() {
    for (const button of $$('[data-group-collapse]')) button.onclick = e => {
      e.stopPropagation();
      const group = G().byID(state.project, button.dataset.groupCollapse);
      if (group) setCollapsed(group.id, !group.collapsed);
    };
    for (const button of $$('[data-group-toggle]')) button.onclick = e => {
      e.stopPropagation();
      toggleBus(button.dataset.groupId, button.dataset.groupToggle);
    };
    for (const label of $$('[data-group-rename]')) label.ondblclick = e => {
      e.stopPropagation();
      renameGroup(label.dataset.groupRename);
    };
    for (const head of $$('th[data-group-id]')) head.onclick = () => {
      const members = G().members(state.project, head.dataset.groupId);
      if (!members.length || members.includes(state.track)) return;
      state.track = members[0];
      state.instrument = state.project.tracks[state.track].instrument || 0;
      state.selection = null;
      refresh();
    };
    // A hit in a collapsed group's overview opens the group at that member and row.
    for (const chip of $$('[data-member-track]')) chip.onclick = e => {
      e.stopPropagation();
      const track = Number(chip.dataset.memberTrack), row = Number(chip.closest('[data-cell]')?.dataset.cell.split(':')[1]);
      const group = G().ofTrack(state.project, track);
      state.selection = null;
      if (Number.isInteger(row)) state.row = row;
      if (group) setCollapsed(group.id, false, { track, refresh: false });
      state.track = track;
      state.instrument = state.project.tracks[track].instrument || 0;
      refresh();
      $('#tracker')?.focus({ preventScroll: true });
    };
    const pads = $('#pad-keys');
    if (pads) pads.onclick = () => { setPadKeys(); $('#pad-keys')?.focus({ preventScroll: true }); };
  }

  function padKeysButton() {
    return `<button id="pad-keys" class="${state.padKeys ? 'active' : ''}" aria-pressed="${!!state.padKeys}"`
        + ` title="Pad keys: Z S X D C V … play and enter the tracks of the cursor's group · ⌥Z">Pad keys</button>`;
  }

  Object.assign(app, {
    setViewPreference, setCollapsed, toggleCollapse, toggleBus, renameGroup, groupTracks, ungroupTrack, dissolveGroup, moveTrack,
    addGroupTrack, padKeysActive, setPadKeys, playPad, padKeyNote, blocksRecording, bindPatternGroups, padKeysButton,
    selectedTracks
  });
})();

(function () {
  'use strict';
  const app = window.STApp;
  app.defineCommands({
    groupTracks: () => app.groupTracks(),
    ungroupTrack: () => app.ungroupTrack(),
    dissolveGroup: () => app.dissolveGroup(),
    renameGroup: () => app.renameGroup(),
    toggleGroup: () => app.toggleCollapse(),
    addGroupTrack: () => app.addGroupTrack(),
    // Hidden empty tracks (app-track-lanes.js) are stepped over.
    moveTrackLeft: () => (app.moveSkippingHidden || app.moveTrack)(-1),
    moveTrackRight: () => (app.moveSkippingHidden || app.moveTrack)(1),
    padKeys: () => app.setPadKeys()
  });
})();
