/* MK3: small app helpers (cursor, columns, note lanes, track groups, channels and levels). */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const { S, st, P, clamp, now, TRACK_COLORS, SCREEN_VIEW, M } = mk;

  // ---- Small app helpers -------------------------------------------------------------------------
  const pattern = () => S().pattern();
  const trackColor = index => P()?.tracks?.[index]?.color || TRACK_COLORS[index % 8];
  const playing = () => !!st().playing;
  const patternRow = () => playing() && st().status?.pattern === pattern()?.id ? Number(st().status.row) || 0 : st().row;
  function flash(text, ms = 1300) { M.flash = { text: String(text), until: now() + ms }; mk.schedule(); }
  let refreshTimer = null;
  function scheduleRefresh(delay = 90) {
    if (refreshTimer) return;
    refreshTimer = setTimeout(() => {
      refreshTimer = null;
      S().refresh();
      revealCursor();
    }, delay);
  }
  function revealCursor() {
    if (st().view !== 'pattern') return;
    document.querySelector?.(`[data-cell="${st().track}:${st().row}"]`)?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  }
  // Synchronous temporary state for one call into the app (velocity, voicing overrides for pads).
  function withState(overrides, fn) {
    const state = st(), saved = {};
    for (const key of Object.keys(overrides)) { saved[key] = state[key]; state[key] = overrides[key]; }
    try { return fn(); } finally { Object.assign(state, saved); }
  }
  function selectTrack(index, { silent = false } = {}) {
    const p = P(); if (!p?.tracks[index]) return false;
    st().track = index; st().instrument = p.tracks[index].instrument || 0; st().selection = null;
    if (!silent) flash(`${String(index + 1).padStart(2, '0')} ${p.tracks[index].name}`, 700);
    scheduleRefresh(); return true;
  }
  function setCursor({ row, track, column }) {
    const state = st(), p = P(), pat = pattern();
    if (track != null) { state.track = clamp(track, 0, p.tracks.length - 1); state.instrument = p.tracks[state.track].instrument || 0; }
    if (row != null) state.row = clamp(row, 0, pat.length - 1);
    if (column != null) state.column = clamp(column, 0, 4);
    state.selection = null; scheduleRefresh(60);
  }
  // ---- Track groups ----------------------------------------------------------------------------------
  /* A group (track-groups.js, window.STGroups) is a contiguous run of member tracks routed to a group bus.
     Group buttons A–H, Home columns, pads and the CHANNEL / PLUG-IN levels read groups from the project. */
  const G = () => window.STGroups;
  const app = () => window.STApp;
  const groupOf = (track = st().track, p = P()) => G()?.ofTrack(p, track) || null;
  const membersOf = (group, p = P()) => group ? G().members(p, group) : [];
  const busOf = (group, p = P()) => group ? p?.routing.buses?.find(bus => bus.id === group.busID) || null : null;
  /* Entries: what the group buttons show, in track order. Each ungrouped track is one entry and each group one
     entry (all its members). { group, index (first track), members } */
  function entries(p = P()) {
    const list = [], byGroup = new Map();
    (p?.tracks || []).forEach((track, index) => {
      const id = track?.groupID, group = id ? G()?.byID(p, id) : null;
      if (!group) { list.push({ group: null, index, members: [index] }); return; }
      let entry = byGroup.get(id);
      if (!entry) { entry = { group, index, members: [] }; byGroup.set(id, entry); list.push(entry); }
      entry.members.push(index);
    });
    return list;
  }
  const entryIndex = (track = st().track, list = entries()) => list.findIndex(entry => entry.members.includes(track));
  const entryColor = entry => entry?.group ? entry.group.color || trackColor(entry.index) : trackColor(entry?.index ?? 0);
  const entryName = (entry, p = P()) => entry?.group ? entry.group.name : p.tracks[entry?.index]?.name || '';
  // Mute / solo of an entry: the group bus for a group, the track otherwise.
  function entryFlags(entry, p = P()) {
    if (!entry) return { mute: false, solo: false };
    const source = entry.group ? busOf(entry.group, p) || {} : p.tracks[entry.index] || {};
    return { mute: !!source.mute, solo: !!source.solo };
  }
  const anySolo = (p = P()) => p.tracks.some(t => t.solo) || !!p.routing.buses?.some(bus => bus.type === 'group' && bus.solo);
  function toggleEntry(entry, property) {
    if (!entry) return;
    if (entry.group) app()?.toggleBus?.(entry.group.id, property);
    else window.STPatternTools?.toggleTrack(property, entry.index);
    mk.schedule();
  }
  // The member a group button selects: the last member selected in that group (if still a member), else its first.
  M.groupMember = new Map();
  function rememberMember(track = st().track, p = P()) {
    const group = groupOf(track, p), t = p?.tracks?.[track];
    if (group && t) M.groupMember.set(group.id, t.routingID || t);
  }
  function memberFor(group, p = P()) {
    const members = membersOf(group, p), saved = M.groupMember.get(group?.id);
    const found = members.find(i => saved != null && (p.tracks[i] === saved || (p.tracks[i].routingID && p.tracks[i].routingID === saved)));
    return found ?? members[0];
  }
  /* Home columns: app.patternColumns() (every track; a collapsed group is one column, index = first member). */
  function noteColumns() {
    const p = P(), list = app()?.patternColumns?.() || p.tracks.map((track, index) => ({ track, index, group: null }));
    return list.map(c => {
      if (c.collapsed) {
        const flags = entryFlags({ group: c.group }, p);
        return { index: c.index, group: c.group, collapsed: true, members: c.members, name: c.group.name, color: c.group.color || trackColor(c.index), ...flags };
      }
      const t = p.tracks[c.index];
      return { index: c.index, group: c.group || null, name: t.name, color: trackColor(c.index), mute: !!t.mute, solo: !!t.solo };
    });
  }
  const columnOf = (columns, track = st().track) => columns.findIndex(c => c.index === track || c.members?.includes(track));
  /* Eight 120 px columns, one under each display button and above each encoder. The window starts at the first
     column of the group-button bank and scrolls just enough to keep the cursor column in view. `width` 4 gives
     the four-column window of single-screen pages (MACRO). */
  function homeColumns(width = 8) {
    const columns = noteColumns(), selected = Math.max(0, columnOf(columns));
    const list = entries(), bankEntry = list[M.bank * 8];
    let first = width === 8 && bankEntry ? Math.max(0, columnOf(columns, bankEntry.index)) : Math.floor(selected / width) * width;
    const cursorEntry = entryIndex(st().track, list);
    if (Math.floor(cursorEntry / 8) === M.bank || width !== 8) {
      if (selected < first) first = selected;
      else if (selected >= first + width) first = selected - width + 1;
    }
    first = clamp(first, 0, Math.max(0, columns.length - 1));
    return { columns: columns.slice(first, first + width), selected, first };
  }
  function selectColumn(c) {
    if (!c) return;
    setCursor({ track: c.collapsed ? memberFor(c.group) : c.index });
    mk.schedule();
  }
  /** Mute or solo one home column: a track, or the group bus of a collapsed group. */
  function toggleColumn(c, property) {
    if (!c) return;
    if (c.collapsed) { app()?.toggleBus?.(c.group.id, property); mk.schedule(); return; }
    window.STPatternTools?.toggleTrack(property, c.index);
  }
  function moveNoteLane(direction, column = st().column) {
    const columns = noteColumns(), index = columnOf(columns), next = columns[clamp(index + direction, 0, columns.length - 1)];
    if (!next || index === columns.indexOf(next)) return;
    setCursor({ track: next.index, column });
  }
  function moveColumn(direction) {
    const column=st().column+direction;
    if(column<0)moveNoteLane(-1,4);else if(column>4)moveNoteLane(1,0);else setCursor({column});
  }
  function ensureView(screen) {
    const view = SCREEN_VIEW[screen];
    if (view && st().view !== view && !st().modal) { S().setView(view); }
    M.lastView = st().view;
  }
  function perform(command) { return S().perform(command); }
  // Steps are plain cells: one step per track per row.
  function ensureStep(p, pat, track, row) {
    let step = pat.steps[track][row];
    if (!step) step = pat.steps[track][row] = { note: -3, instrument: p.tracks[track]?.instrument ?? 0, velocity: 100, gate: 0 };
    return step;
  }
  function stepAt(track = st().track, row = st().row, pat = pattern()) { return pat?.steps?.[track]?.[row] || null; }
  function patternIndex(id = P().activePattern) { return P().patterns.findIndex(pat => pat.id === id); }
  const AR = () => window.STAutomationRecord;
  function arRef(track, target, name) { try { return AR()?.makeRef?.(track, target, name) || null; } catch { return null; } }
  function arRecording(ref) { const ar = AR(); return !!(ref && ar && (ar.wouldRecord?.(ref) || ar.claimed?.(ref))); }

  /* Mixer channels by owner: 'master' (the project: masterVolume), 'bus:<id>' (a routing bus, e.g. a group bus)
     or 'track:<routingID>'. */
  function channelOf(owner, project = P()) {
    if (owner === 'master') return project;
    if (owner?.startsWith('bus:')) return project?.routing.buses?.find(bus => `bus:${bus.id}` === owner) || null;
    if (owner?.startsWith('track:')) return project?.tracks?.find(track => `track:${track.routingID}` === owner) || null;
    return null;
  }
  function ownerMixKnob(owner, kind, label) {
    const channel = channelOf(owner); if (!channel) return null;
    const [name, min, trackMax, format, target] = mk.MIX[kind], field = owner === 'master' && kind === 'volume' ? 'masterVolume' : kind;
    // A bus fader reaches gain 2 (+6 dB), as in the mixer; with the track range a louder bus dropped to 1.5 on the first turn.
    const max = kind === 'volume' && owner.startsWith('bus:') ? 2 : trackMax;
    const fallback = kind === 'volume' ? (owner === 'master' ? .7 : owner.startsWith('bus:') ? 1 : .8) : 0;
    return mk.numKnob({ key: `ownerMix:${encodeURIComponent(owner)}:${field}`, label: label || name, name: `${label || owner} · ${name}`,
      read: project => Number(channelOf(owner, project)?.[field] ?? fallback),
      write: (project, value) => {
        const c = channelOf(owner, project);
        if (c) c[field] = value;
      },
      min, max, fmt: format, ref: arRef(owner, target, `${label || owner} · ${name}`) });
  }

  Object.assign(mk, {
    pattern, trackColor, playing, patternRow, flash, scheduleRefresh, withState, selectTrack, setCursor, homeColumns,
    selectColumn, toggleColumn, moveNoteLane, moveColumn, ensureView, perform, ensureStep, stepAt, patternIndex, AR,
    arRef, arRecording, G, app, groupOf, membersOf, busOf, entries, entryIndex, entryColor, entryName, entryFlags,
    anySolo, toggleEntry, rememberMember, memberFor, noteColumns, columnOf, channelOf, ownerMixKnob
  });
})();
