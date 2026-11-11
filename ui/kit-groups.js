/* Kits are track groups (docs/KITS.md): loading a kit (kit browser, Maschine group,
   .stkit) adds a group with one member track per loaded pad and the kit's group bus; Replace kit
   swaps the members' sources pad by pad and keeps the patterns; Save kit writes the group as a
   sixteen-pad .stkit package. The host reads and writes kit files: kit.read returns the kit as a
   group project it has validated (tracks, instruments, audioPlugins, one group, a complete routing
   table; st-library kit::group), so nothing here checks its shape again; kit.save takes the group
   out of the session. */
(() => {
  'use strict';
  const S = () => window.ST, state = () => S().state, project = () => state().project, G = () => window.STGroups;
  const clone = value => JSON.parse(JSON.stringify(value));
  const stableJSON = value => JSON.stringify(value, (_, item) => item && typeof item === 'object' && !Array.isArray(item)
      ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]])) : item);
  const ui = { loading: null, status: null };
  const idle = () => ui.loading || Promise.resolve();

  const cursorGroup = (p = project()) => G().ofTrack(p, state().track) || null;
  const busOwner = group => `bus:${group.busID}`;
  const trackOwner = track => `track:${track.routingID}`;

  function defaultInstrument(id, name) {
    return { ...S().instrumentDefaults(id, name, 'sample'), name, volume: 1, voiceMode: 'mono' };
  }
  // A kit document's instrument as an instrument of the session: every key an instrument has, the kit's values.
  const kitInstrument = (id, instrument) => ({ ...S().instrumentDefaults(id, instrument.name), ...clone(instrument), id });

  /* Owners a group project's group contains: its members, its bus and the buses it feeds
     (a kit file may carry its own returns). */
  function sourceOwners(source, group) {
    const members = source.tracks.filter(t => t.groupID === group.id);
    const owners = new Set([busOwner(group), ...members.map(trackOwner)]);
    const buses = new Set(source.routing.buses.map(bus => `bus:${bus.id}`));
    let grew = true;
    while (grew) {
      grew = false;
      for (const owner of [...owners]) {
        const targets = [source.routing.outputs[owner], ...source.routing.sends.filter(s => s.source === owner).map(s => s.destination)];
        for (const target of targets) if (buses.has(target) && !owners.has(target)) { owners.add(target); grew = true; }
      }
    }
    return { members, owners };
  }

  /* Adds the group of a group project (from kit.read) to the session: member tracks at the end
     (so existing tracks keep their indices and the live graph only grows), their instruments,
     plug-ins, the bus and routes, all with fresh identities. fresh(key) must return the same ID
     for the same key so the published candidate and the committed edit are identical. */
  function insertGroup(q, source, fresh, options = {}) {
    const group = source.groups[0], { members, owners } = sourceOwners(source, group);
    const limit = window.TRACK_SANITY_LIMIT ?? Infinity;
    if (q.tracks.length + members.length > limit) throw new Error(`This session has room for ${limit - q.tracks.length} more tracks; the kit needs ${members.length}.`);
    const groupID = fresh('group'), owner = new Map();
    for (const bus of source.routing.buses) if (owners.has(`bus:${bus.id}`)) owner.set(`bus:${bus.id}`, `bus:${bus.id === group.busID ? fresh('bus') : fresh('bus:' + bus.id)}`);
    members.forEach((member, i) => owner.set(trackOwner(member), `track:${fresh('track' + i)}`));
    const firstTrack = q.tracks.length;
    members.forEach((member, i) => {
      const instrumentIndex = q.instruments.length;
      q.instruments.push(kitInstrument(instrumentIndex, source.instruments[member.instrument]));
      // The pad's instrument takes its insert chain (instrument-effects.js).
      owners.add(`instrument:${member.instrument}`); owner.set(`instrument:${member.instrument}`, `instrument:${instrumentIndex}`);
      const track = { ...clone(member), id: q.tracks.length, routingID: owner.get(trackOwner(member)).slice(6), groupID, instrument: instrumentIndex };
      track.color ||= S().colors?.[(q.tracks.length) % 8];
      q.tracks.push(track);
      for (const pattern of q.patterns) pattern.steps.push(Array(pattern.length).fill(null));
    });
    const instances = new Map();
    for (const plugin of source.audioPlugins) {
      if (!owners.has(plugin.owner)) continue;
      const copy = { ...clone(plugin), owner: owner.get(plugin.owner), instanceID: fresh('plugin:' + plugin.instanceID) };
      instances.set(plugin.instanceID, copy.instanceID);
      q.audioPlugins.push(copy);
    }
    const r = q.routing;
    for (const bus of source.routing.buses) {
      const mapped = owner.get(`bus:${bus.id}`);
      if (!mapped) continue;
      const copy = { ...clone(bus), id: mapped.slice(4) };
      if (bus.id === group.busID) { copy.type = 'group'; copy.groupID = groupID; copy.name = group.name; } else delete copy.groupID;
      r.buses.push(copy);
    }
    for (const [from, to] of Object.entries(source.routing.outputs)) {
      if (!owner.has(from)) continue;
      r.outputs[owner.get(from)] = to === null ? null : to === 'master' ? 'master' : owner.get(to) || 'master';
    }
    source.routing.sends.forEach((send, i) => {
      if (owner.has(send.source) && (owner.has(send.destination) || send.destination === 'master')) {
        r.sends.push({ ...clone(send), id: fresh('send' + i), source: owner.get(send.source), destination: owner.get(send.destination) || 'master' });
      }
    });
    source.routing.sidechains.forEach((route, i) => {
      if (owner.has(route.source) && instances.has(route.destination)) {
        r.sidechains.push({ ...clone(route), id: fresh('sidechain' + i), source: owner.get(route.source), destination: instances.get(route.destination) });
      }
    });
    // A kit document carries parameter pages only when its plug-ins have some.
    for (const [old, pages] of Object.entries(source.pluginPages || {})) if (instances.has(old)) q.pluginPages[instances.get(old)] = clone(pages);
    const record = { id: groupID, name: group.name, color: options.color || nextColor(q), collapsed: false, mixerCollapsed: false, busID: owner.get(busOwner(group)).slice(4) };
    if (options.libraryPath) record.libraryPath = options.libraryPath;
    if (options.notes?.length) record.importNotes = options.notes;
    q.groups.push(record);
    G().ensureBus(q, record);
    return { groupID, firstTrack, count: members.length };
  }

  function nextColor(q) {
    const used = new Set(G().list(q).map(group => group.color));
    return G().COLORS.find(color => !used.has(color)) || G().COLORS[G().list(q).length % G().COLORS.length];
  }

  // Everything that belongs to one member's sound source: its instrument plug-in and inserts.
  function removeTrackPlugins(q, track) {
    const ownerID = trackOwner(track), chainID = `instrument:${track.instrument}`;
    const removed = new Set(q.audioPlugins.filter(p => p.owner === ownerID || p.owner === chainID).map(p => p.instanceID));
    q.audioPlugins = q.audioPlugins.filter(p => p.owner !== ownerID && p.owner !== chainID);
    q.routing.sidechains = q.routing.sidechains.filter(route => !removed.has(route.destination));
    for (const id of removed) delete q.pluginPages[id];
  }

  /* Replace kit: the target group's members take the new kit's sources pad by pad (instrument,
     plug-ins, inserts, mix, choke group, name); their pattern steps, automation and routing stay.
     New pads add members at the end of the group; pads the new kit leaves empty become empty
     samplers, keeping their notes. The bus takes the new kit's inserts and mix. */
  function replaceGroup(q, groupID, source, fresh, options = {}) {
    const group = G().byID(q, groupID), incoming = source.groups[0];
    if (!group) throw new Error('The kit to replace is unavailable.');
    const { members: sourceMembers } = sourceOwners(source, incoming);
    const byPad = new Map(sourceMembers.map(member => [member.pad, member]));
    const existing = G().members(q, group);
    const usedPads = new Set();
    const adopt = (index, member) => {
      const track = q.tracks[index];
      removeTrackPlugins(q, track);
      const instrumentIndex = track.instrument;
      q.instruments[instrumentIndex] = kitInstrument(instrumentIndex, source.instruments[member.instrument]);
      for (const key of ['name', 'volume', 'pan', 'delaySend', 'reverbSend', 'chokeGroup', 'voiceMode', 'padNote']) {
        if (member[key] === undefined) delete track[key]; else track[key] = clone(member[key]);
      }
      for (const plugin of source.audioPlugins) {
        if (plugin.owner === `instrument:${member.instrument}`) { q.audioPlugins.push({ ...clone(plugin), owner: `instrument:${instrumentIndex}`, instanceID: fresh('plugin:' + plugin.instanceID) }); continue; }
        if (plugin.owner !== trackOwner(member)) continue;
        const copy = { ...clone(plugin), owner: trackOwner(track), instanceID: fresh('plugin:' + plugin.instanceID) };
        q.audioPlugins.push(copy);
      }
    };
    for (const index of existing) {
      const track = q.tracks[index], member = byPad.get(track.pad);
      if (member && !usedPads.has(track.pad)) { adopt(index, member); usedPads.add(track.pad); continue; }
      removeTrackPlugins(q, track);
      q.instruments[track.instrument] = defaultInstrument(track.instrument, `Pad ${String((track.pad ?? 0) + 1).padStart(2, '0')}`);
      track.name = `Pad ${String((track.pad ?? 0) + 1).padStart(2, '0')}`;
      delete track.chokeGroup; delete track.voiceMode;
    }
    let added = 0;
    for (const member of sourceMembers) {
      if (usedPads.has(member.pad)) continue;
      const instrumentIndex = q.instruments.length;
      q.instruments.push(defaultInstrument(instrumentIndex, member.name));
      const index = q.tracks.length;
      q.tracks.push({ id: index, routingID: fresh('track:' + member.pad), name: member.name, instrument: instrumentIndex, volume: .8, pan: 0,
        mute: false, solo: false, delaySend: 0, reverbSend: 0, pad: member.pad, padNote: 60, color: S().colors?.[index % 8] });
      for (const pattern of q.patterns) pattern.steps.push(Array(pattern.length).fill(null));
      const map = G().addTrack(q, groupID, index);
      adopt(map[index], member);
      usedPads.add(member.pad);
      added++;
    }
    // The bus: the new kit's inserts and mix.
    const owner = busOwner(group), removed = new Set(q.audioPlugins.filter(p => p.owner === owner).map(p => p.instanceID));
    q.audioPlugins = q.audioPlugins.filter(p => p.owner !== owner);
    q.routing.sidechains = q.routing.sidechains.filter(route => !removed.has(route.destination));
    for (const plugin of source.audioPlugins) if (plugin.owner === busOwner(incoming)) q.audioPlugins.push({ ...clone(plugin), owner, instanceID: fresh('plugin:' + plugin.instanceID) });
    const bus = G().ensureBus(q, group), sourceBus = source.routing.buses.find(item => item.id === incoming.busID);
    for (const key of ['volume', 'pan']) if (sourceBus[key] !== undefined) bus[key] = sourceBus[key];
    G().rename(q, groupID, incoming.name);
    if (options.libraryPath) group.libraryPath = options.libraryPath; else delete group.libraryPath;
    if (options.notes?.length) group.importNotes = options.notes; else delete group.importNotes;
    return { groupID, added };
  }

  /* Load kit: read and convert the file natively, publish the complete candidate to the audio
     host (samples decode in the background), then commit it as one Undo step. */
  async function loadKit(path, { stay = true, replace = false } = {}) {
    if (ui.loading) { S().toast('A kit is still loading.'); return false; }
    const p = project(), target = replace ? cursorGroup(p) : null,
      name = String(path || '').split('/').pop().replace(/\.[^.]+$/, '') || 'kit';
    if (replace && !target) { S().toast('Move the cursor to a group (kit) to replace it.'); return false; }
    let published = false, finish;
    ui.loading = new Promise(resolve => { finish = resolve; });
    const progress = text => { ui.status = text; S().toast(text); };
    try {
      if (await S().captureNativeEdits({ kitEdit: true }) === false || project() !== p) return false;
      progress(`Loading ${name}… reading kit`);
      const result = await S().request('kit.read', { path });
      if (project() !== p || result?.cancelled) return false;
      const source = result.groupProject;
      const notes = [...new Set([...(result.warnings || []), ...(source.groups[0].importNotes || [])].map(String))];
      const libraryPath = /\.stkit$/i.test(String(path)) ? String(path) : undefined;
      const ids = new Map(), fresh = key => { if (!ids.has(key)) ids.set(key, S().uid()); return ids.get(key); };
      if (replace && !G().byID(p, target.id)) throw new Error('The kit to replace changed while reading. Select it and try again.');
      const apply = q => replace ? replaceGroup(q, target.id, source, fresh, { libraryPath, notes }) : insertGroup(q, source, fresh, { libraryPath, notes });
      const baseline = stableJSON(p), valid = () => project() === p && stableJSON(p) === baseline;
      const candidate = clone(p), info = apply(candidate);
      progress(`Loading ${name}… preparing ${info.count ?? info.added ?? 0} tracks`);
      published = true;
      if (await S().syncEngineProject(() => valid() ? clone(candidate) : null, { kitEdit: true }) === false || !valid()) {
        throw new Error('The session changed while the kit was loading. Load the kit again.');
      }
      let done = null;
      S().mutate(replace ? 'replace kit' : 'load kit', q => {
        done = apply(q);
        if (!replace) { state().track = done.firstTrack; state().instrument = q.tracks[done.firstTrack].instrument; }
        state().selection = null;
      }, { kitEdit: true });
      if (!done) throw new Error('The kit could not be added to this session.');
      published = false;
      if (!stay) S().setView('pattern');
      const group = G().byID(project(), done.groupID);
      S().toast(replace
          ? `${group.name} replaced the kit · pattern notes kept${notes.length ? ` · ${notes.length} import notes` : ''}`
          : `${group.name} loaded · ${done.count} track${done.count === 1 ? '' : 's'} in a group${notes.length ? ` · ${notes.length} import notes` : ''}`);
      return true;
    } catch (error) {
      let message = error?.message || String(error);
      if (published && project() === p) {
        try { if (await S().syncEngineProject(() => clone(project()), { capture: true, kitEdit: true }) === false) throw new Error('not acknowledged'); }
        catch (restore) { message += ' Audio restoration failed: ' + restore.message; }
      }
      S().toast(`Could not load ${name}: ${message}`, 'error');
      return false;
    } finally { ui.loading = null; ui.status = null; finish(); }
  }

  /* An empty kit: a group with one empty sampler track (pad 1). Add pads with "Add track to group". */
  function createKit(name = 'My kit') {
    let created = false;
    S().mutate('new kit', q => {
      const id = q.instruments.length;
      q.instruments.push(defaultInstrument(id, 'Pad 01'));
      const index = S().appendInstrumentTrack(q, id, 'Pad 01');
      q.tracks[index].pad = 0; q.tracks[index].padNote = 60;
      const { map } = G().create(q, [index], { name, makeID: S().uid });
      state().track = map[index]; state().instrument = id; state().selection = null;
      created = true;
    });
    return created;
  }

  // Save writes the group's own library package (group.libraryPath); Save As, or Save of a
  // group without one, asks for a name. The location is a session record, not an Undo step.
  function saveKit(options = {}) {
    const group = cursorGroup();
    if (!group) { S().toast('Move the cursor to a group (kit) to save it.'); return Promise.resolve(false); }
    if (options.name) return writeKit(group, { name: options.name });
    return !options.as && group.libraryPath ? writeKit(group, { path: group.libraryPath, overwrite: true }) : saveKitAs();
  }
  async function saveKitAs() {
    const group = cursorGroup(), p = project();
    if (!group) { S().toast('Move the cursor to a group (kit) to save it.'); return false; }
    const name = await S().askText('Save kit as', group.libraryPath?.split('/').pop().replace(/\.stkit$/i, '') || group.name, 'Save kit');
    if (!name || project() !== p || !G().byID(p, group.id)) return false;
    return writeKit(group, { name });
  }
  function recordLibraryPath(groupID, path) {
    for (const q of [project(), ...state().undo.map(x => x.project), ...state().redo.map(x => x.project)]) {
      const group = G().byID(q, groupID);
      if (group && group.libraryPath !== path) {
        group.libraryPath = path;
        if (q === project()) { S().markDirty(); S().scheduleAutosave?.(); }
      }
    }
  }
  async function writeKit(group, target) {
    const p = project();
    try {
      if (await S().captureNativeEdits() === false || project() !== p) return false;
      await S().waitForGraphEdit?.();
      if (project() !== p || !G().byID(p, group.id)) return false;
      if (await S().syncEngineProject(() => project() === p ? clone(p) : null, { capture: true }) === false) throw new Error('The kit could not be synchronized. Retry Save kit.');
      if (project() !== p) return false;
      const r = await S().request('kit.save', { project: clone(p), groupID: group.id, ...target });
      if (project() !== p || r?.cancelled) return false;
      if (r?.needsName) return saveKitAs();
      if (r?.exists) {
        if (!await S().confirmChoice('Replace kit?', `A kit named “${r.name || target.name}” is already in your kit library. Replacing it overwrites that kit file.`, 'Replace')) return false;
        if (project() !== p) return false;
        return writeKit(group, { name: target.name, overwrite: true });
      }
      if (typeof r?.path === 'string' && r.path) recordLibraryPath(group.id, r.path);
      const warnings = Array.isArray(r?.warnings) ? [...new Set(r.warnings.filter(x => typeof x === 'string' && x.trim()).map(x => x.trim()))] : [];
      S().toast('Kit saved' + (r?.path ? ' · ' + r.path : '') + (warnings.length ? ' · ' + warnings.join(' ') : ''));
      return true;
    } catch (error) { S().toast(error.message, 'error'); return false; }
  }

  // Removes a group and its member tracks (with everything they own) in one Undo step.
  function removeKit(groupID = cursorGroup()?.id) {
    const p = project(), group = G().byID(p, groupID);
    if (!group) { S().toast('Move the cursor to a group (kit) to remove it.'); return false; }
    const members = G().members(p, group);
    if (members.length >= p.tracks.length) { S().toast('Keep at least one track in a session: dissolve the group instead.'); return false; }
    return S().mutate('remove kit', q => {
      for (const index of [...G().members(q, groupID)].reverse()) {
        // A lane names its channel by owner, or (the engine's own track parameters) its track by index.
        const track = q.tracks[index], owner = trackOwner(track), belongs = x => x.owner ? x.owner === owner : x.track === index;
        window.STRoutingProject.keepInstrumentPlugins(q, [track]);
        window.STRoutingProject.removeTrackReferences(q, track, q.audioPlugins.filter(x => x.owner === owner));
        q.tracks.splice(index, 1);
        q.audioPlugins = q.audioPlugins.filter(x => x.owner !== owner);
        q.audioClips = q.audioClips.filter(x => x.track !== index).map(x => ({ ...x, track: x.track > index ? x.track - 1 : x.track }));
        for (const pattern of q.patterns) {
          pattern.steps.splice(index, 1);
          pattern.automation = pattern.automation.filter(l => !belongs(l)).map(l => Number.isInteger(l.track) && l.track > index ? { ...l, track: l.track - 1 } : l);
        }
      }
      q.tracks.forEach((track, i) => { track.id = i; });
      G().dissolve(q, groupID);
      state().track = Math.min(members[0], q.tracks.length - 1);
      state().instrument = q.tracks[state().track].instrument;
      state().selection = null;
    });
  }

  window.STKitGroups = {
    loadKit, replaceKit: path => loadKit(path, { replace: true }), createKit, saveKit, saveKitAs, removeKit, insertGroup, replaceGroup,
    cursorGroup: () => cursorGroup(), waitForIdle: idle,
    get isBusy() { return !!ui.loading; },
    get status() { return ui.status; }
  };
})();
