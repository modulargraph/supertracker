/* Build an independent track proposal. Publishing and native state capture belong to the caller. */
(() => {
  'use strict';
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const has = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
  const identifier = value => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value);
  const fail = message => { throw new Error(message); };
  // Insert slots per mixer channel (0...15), matching the audio host.
  const INSERT_SLOTS = 16;
  function clone(value) {
    try {
      return JSON.parse(JSON.stringify(value, (_key, item) => {
        if (typeof item === 'function' || typeof item === 'symbol' || typeof item === 'bigint' || typeof item === 'number' && !Number.isFinite(item)) fail('Track duplication requires JSON project data.');
        return item;
      }));
    } catch (error) { fail(`Cannot duplicate malformed project data: ${error.message}`); }
  }
  function array(value, label, optional = false) {
    if (value === undefined && optional) return [];
    if (!Array.isArray(value)) fail(`${label} must be an array.`);
    return value;
  }
  function build(project, index, makeID) {
    if (!object(project)) fail('Choose a valid project before duplicating a track.');
    const proposal = clone(project), tracks = array(proposal.tracks, 'Tracks'), instruments = array(proposal.instruments, 'Instruments');
    if (!Number.isInteger(index) || index < 0 || index >= tracks.length || !object(tracks[index])) fail('The source track does not exist.');
    const limit = (typeof window !== 'undefined' ? window : globalThis).TRACK_SANITY_LIMIT ?? Infinity; // defined by app.js
    if (tracks.length >= limit) fail(`Track duplication needs a new track; this session already has ${limit} tracks.`);
    const source = tracks[index], trackIndex = tracks.length;
    if (!Number.isInteger(source.instrument) || source.instrument < 0 || source.instrument >= instruments.length || !object(instruments[source.instrument])) fail('The source track has an invalid assigned instrument.');
    const patterns = array(proposal.patterns, 'Patterns', true), clips = array(proposal.audioClips, 'Audio clips', true);
    const plugins = array(proposal.audioPlugins, 'Audio plug-ins', true), routing = proposal.routing;
    if (!object(routing)) fail('Track duplication needs the project\'s routing table.');
    const buses = array(routing.buses, 'Routing buses', true);
    const sends = array(routing.sends, 'Routing sends', true);
    const sidechains = array(routing.sidechains, 'Routing sidechains', true);
    const owners = new Set(['master']), trackOwners = [], explicitTrackIDs = new Set();
    for (let i = 0; i < tracks.length; i++) {
      const track = tracks[i];
      if (!object(track)) fail('Tracks must contain track objects.');
      if (!identifier(track.routingID) || explicitTrackIDs.has(track.routingID)) fail('Track routing identities must be valid and unique.');
      explicitTrackIDs.add(track.routingID);
      const owner = `track:${track.routingID}`;
      trackOwners.push(owner); owners.add(owner);
    }
    for (const bus of buses) {
      if (!object(bus) || !identifier(bus.id) || owners.has(`bus:${bus.id}`)) fail('Bus identities must be valid and unique.');
      owners.add(`bus:${bus.id}`);
    }
    // Instrument insert chains (instrument-effects.js) are owned by their instrument.
    for (let i = 0; i < instruments.length; i++) owners.add(`instrument:${i}`);
    const sourceOwner = trackOwners[index];
    // A plug-in descriptor names its channel by owner.
    function pluginOwner(plugin) {
      if (!object(plugin)) fail('Audio plug-in must contain objects.');
      if (typeof plugin.owner !== 'string' || !owners.has(plugin.owner)) fail('Audio plug-in has an invalid owner.');
      return plugin.owner;
    }
    // An automation lane names its channel by owner, or (the engine's own track parameters) its track by index.
    function laneOwner(lane) {
      if (!object(lane)) fail('Automation lane must contain objects.');
      if (has(lane, 'owner')) {
        if (typeof lane.owner !== 'string' || !lane.owner) fail('Automation lane has an invalid owner.');
        return lane.owner;
      }
      return Number.isInteger(lane.track) && lane.track >= 0 && lane.track < tracks.length ? trackOwners[lane.track] : null;
    }
    const needed = new Set([source.instrument]);
    for (const pattern of patterns) {
      if (!object(pattern) || !Number.isInteger(pattern.length) || pattern.length < 1 || pattern.length > 256) fail('Patterns need a valid row count before duplication.');
      const steps = array(pattern.steps, 'Pattern steps');
      if (steps.length > tracks.length || steps[index] !== undefined && !Array.isArray(steps[index])) fail('The source pattern column is malformed.');
      const column = steps[index] || [];
      if (column.length > pattern.length) fail('The source pattern column exceeds its row count.');
      for (const step of column) {
        if (step == null) continue;
        if (!object(step)) fail('The source pattern column contains a malformed step.');
        for (const cell of [step, ...(Array.isArray(step.columns) ? step.columns : [])]) {
          if (cell && Number.isInteger(cell.instrument) && cell.instrument >= 0 && cell.instrument < instruments.length) {
            if (!object(instruments[cell.instrument])) fail('A source step references a malformed instrument.');
            needed.add(cell.instrument);
          }
        }
      }
      for (const lane of array(pattern.automation, 'Automation lanes', true)) laneOwner(lane);
    }
    const sourceClips = clips.filter(clip => { if (!object(clip)) fail('Audio clips must contain objects.'); return clip.track === index; });
    if (clips.length + sourceClips.length > 256) fail('Track duplication would exceed 256 audio clips.');
    const pluginIDs = new Set(), pluginSlots = new Set(), sourcePlugins = [];
    for (const plugin of plugins) {
      const owner = pluginOwner(plugin), kind = plugin.kind || 'effect', slot = plugin.slot ?? 0;
      if (!['instrument', 'effect'].includes(kind) || !Number.isInteger(slot) || slot < 0 || slot >= INSERT_SLOTS || kind === 'instrument' && (slot !== 0 || !owner.startsWith('track:'))) fail('Audio plug-in has an invalid slot or owner.');
      const location = `${owner}/${kind}/${slot}`;
      if (pluginSlots.has(location)) fail('Two audio plug-ins occupy the same owner and slot.');
      pluginSlots.add(location);
      if (has(plugin, 'instanceID')) {
        if (!identifier(plugin.instanceID) || pluginIDs.has(plugin.instanceID)) fail('Audio plug-in instance identities must be valid and unique.');
        pluginIDs.add(plugin.instanceID);
      }
      if (owner === sourceOwner) sourcePlugins.push(plugin);
    }
    const routeIDs = new Set();
    for (const route of [...sends, ...sidechains]) {
      if (!object(route) || !identifier(route.id) || routeIDs.has(route.id)) fail('Routing identities must be valid and unique.');
      routeIDs.add(route.id);
    }
    const sourceSends = sends.filter(send => send.source === sourceOwner);
    const copiedPluginIDs = new Set(sourcePlugins.map(plugin => plugin.instanceID).filter(Boolean));
    const sourceSidechains = sidechains.filter(route => copiedPluginIDs.has(route.destination));
    if (sends.length + sidechains.length + sourceSends.length + sourceSidechains.length > 128) fail('Track duplication would exceed 128 sends and sidechains combined.');
    if (routing.outputs !== undefined && !object(routing.outputs)) fail('Routing outputs must be an object.');

    // Reserve identity fields throughout the JSON, including dormant metadata,
    // without rewriting anything inside vendor state or NKS payloads.
    const occupied = new Set();
    function reserve(value) {
      if (typeof value === 'string') { const reference = /^(?:track|bus|send):(.+)$/.exec(value); if (reference) occupied.add(reference[1]); return; }
      if (Array.isArray(value)) { value.forEach(reserve); return; }
      if (!object(value)) return;
      for (const [key, item] of Object.entries(value)) {
        reserve(key);
        if (['id', 'routingID', 'instanceID'].includes(key) && ['string', 'number'].includes(typeof item)) occupied.add(String(item));
        reserve(item);
      }
    }
    reserve(proposal);
    let serial = 0;
    function fresh() {
      let candidate;
      try { candidate = makeID?.(); } catch (_) { /* A local fallback remains deterministic and bounded. */ }
      if (!identifier(candidate) || occupied.has(candidate)) {
        do { candidate = `track-copy-${++serial}`; } while (occupied.has(candidate));
      }
      occupied.add(candidate); return candidate;
    }
    const newTrack = clone(source), newOwner = `track:${newTrack.routingID = fresh()}`;
    newTrack.id = trackIndex; newTrack.name = `${source.name || `Track ${index + 1}`} copy`;
    const instrumentMap = new Map(), instanceMap = new Map(), sendMap = new Map();
    for (const old of needed) {
      const next = instruments.length, instrument = clone(instruments[old]); instrument.id = next;
      instrumentMap.set(old, next); instruments.push(instrument);
    }
    newTrack.instrument = instrumentMap.get(source.instrument);
    const pluginCopies = sourcePlugins.map(plugin => {
      const copied = clone(plugin); copied.owner = newOwner;
      copied.instanceID = fresh();
      if (plugin.instanceID) instanceMap.set(plugin.instanceID, copied.instanceID);
      return copied;
    });
    function remapDescriptor(descriptor) {
      if (!object(descriptor) || !instanceMap.has(descriptor.instanceID)) return;
      descriptor.instanceID = instanceMap.get(descriptor.instanceID);
      descriptor.owner = newOwner;
    }
    for (const next of instrumentMap.values()) remapDescriptor(instruments[next].plugin);
    for (const send of sourceSends) { const copied = clone(send); copied.id = fresh(); copied.source = newOwner; sendMap.set(send.id, copied.id); sends.push(copied); }
    for (const route of sourceSidechains) {
      const copied = clone(route); copied.id = fresh(); copied.destination = instanceMap.get(route.destination);
      if (copied.source === sourceOwner) copied.source = newOwner;
      sidechains.push(copied);
    }
    for (const pattern of patterns) {
      const original = pattern.steps[index] || [], column = Array.from({ length: pattern.length }, (_, row) => original[row] == null ? null : clone(original[row]));
      for (const step of column) if (step) {
        if (instrumentMap.has(step.instrument)) step.instrument = instrumentMap.get(step.instrument);
        // Extra note columns (app-note-columns.js) play the copied instruments too.
        for (const cell of Array.isArray(step.columns) ? step.columns : []) if (cell && instrumentMap.has(cell.instrument)) cell.instrument = instrumentMap.get(cell.instrument);
        for (const key of ['fx1', 'fx2']) if (step[key]?.type === 'plugin' && instanceMap.has(step[key].instanceID)) step[key].instanceID = instanceMap.get(step[key].instanceID);
      }
      while (pattern.steps.length < trackIndex) pattern.steps.push(Array(pattern.length).fill(null));
      pattern.steps.push(column);
      const lanes = array(pattern.automation, 'Automation lanes', true);
      const copies = lanes.filter(lane => laneOwner(lane) === sourceOwner).map(lane => {
        const copied = clone(lane); copied.id = fresh();
        if (has(copied, 'owner')) copied.owner = newOwner; else copied.track = trackIndex;
        if (instanceMap.has(copied.instanceID)) copied.instanceID = instanceMap.get(copied.instanceID);
        if (typeof copied.target === 'string' && copied.target.startsWith('send:') && sendMap.has(copied.target.slice(5))) copied.target = `send:${sendMap.get(copied.target.slice(5))}`;
        return copied;
      });
      if (copies.length) pattern.automation = [...lanes, ...copies];
    }
    for (const clip of sourceClips) { const copied = clone(clip); copied.id = fresh(); copied.track = trackIndex; clips.push(copied); }
    if (sourceClips.length) proposal.audioClips = clips;
    // Each copied instrument takes its insert chain, with fresh instance identities.
    const chainCopies = [...instrumentMap].flatMap(([old, next]) => plugins.filter(plugin => plugin.owner === `instrument:${old}`).map(plugin => {
      const copied = clone(plugin); copied.owner = `instrument:${next}`;
      copied.instanceID = fresh();
      if (plugin.instanceID) instanceMap.set(plugin.instanceID, copied.instanceID);
      if (plugin.instanceID && proposal.pluginPages?.[plugin.instanceID]) proposal.pluginPages[copied.instanceID] = clone(proposal.pluginPages[plugin.instanceID]);
      return copied;
    }));
    if (chainCopies.length) for (const pattern of patterns) for (const step of pattern.steps[trackIndex] || []) if (step) {
      for (const key of ['fx1', 'fx2']) if (step[key]?.type === 'plugin' && instanceMap.has(step[key].instanceID)) step[key].instanceID = instanceMap.get(step[key].instanceID);
    }
    if (pluginCopies.length || chainCopies.length) proposal.audioPlugins = [...plugins, ...pluginCopies, ...chainCopies];
    routing.outputs ||= {};
    routing.outputs[newOwner] = has(routing.outputs, sourceOwner) ? routing.outputs[sourceOwner] : 'master';
    if (sourceSends.length) routing.sends = sends;
    if (sourceSidechains.length) routing.sidechains = sidechains;
    tracks.push(newTrack);
    return { project: proposal, trackIndex, sourceTrackIndex: index, instrumentMap: Object.fromEntries(instrumentMap), instanceMap: Object.fromEntries(instanceMap), newPluginInstanceIDs: pluginCopies.map(plugin => plugin.instanceID) };
  }
  /* A whole track group (docs/KITS.md): every member is duplicated with build() (instruments, plug-ins,
     pattern columns, automation, sends, clips), then the copies get a new group with a copy of the group bus: its
     mix, output, inserts (fresh instances), sends, sidechains and automation. The copies are appended at the end
     (the caller moves them after the source group); members output to the new bus. */
  function buildGroup(project, groupID, makeID) {
    if (!object(project)) fail('Choose a valid project before duplicating a group.');
    const group = array(project.groups, 'Groups', true).find(item => object(item) && item.id === groupID);
    if (!group) fail('The source group does not exist.');
    const members = array(project.tracks, 'Tracks').map((track, index) => object(track) && track.groupID === groupID ? index : -1).filter(index => index >= 0);
    if (!members.length) fail('The source group has no tracks.');
    const limit = (typeof window !== 'undefined' ? window : globalThis).TRACK_SANITY_LIMIT ?? Infinity;
    if (project.tracks.length + members.length > limit) fail(`Group duplication needs ${members.length} new tracks; this session has room for ${Math.max(0, limit - project.tracks.length)}.`);
    let proposal = project;
    const trackIndices = [], instanceMap = new Map(), ownerMap = new Map(), newPluginInstanceIDs = [];
    for (const index of members) {
      const result = build(proposal, index, makeID);
      proposal = result.project;
      trackIndices.push(result.trackIndex);
      Object.entries(result.instanceMap).forEach(([from, to]) => instanceMap.set(from, to));
      newPluginInstanceIDs.push(...result.newPluginInstanceIDs);
      ownerMap.set(`track:${proposal.tracks[index].routingID}`, `track:${proposal.tracks[result.trackIndex].routingID}`);
    }
    const routing = proposal.routing;
    if (!object(routing)) fail('Group duplication needs routed tracks.');
    const buses = array(routing.buses, 'Routing buses', true), sourceBus = buses.find(bus => object(bus) && bus.id === group.busID);
    if (!sourceBus) fail('The source group has no bus.');
    const text = JSON.stringify(proposal);
    let serial = 0;
    const used = new Set();
    const fresh = () => {
      let candidate;
      try { candidate = makeID?.(); } catch (_) { /* fall back below */ }
      while (!identifier(candidate) || used.has(candidate) || text.includes(`"${candidate}"`) || text.includes(`:${candidate}"`)) candidate = `group-copy-${++serial}`;
      used.add(candidate); return candidate;
    };
    const sourceOwner = `bus:${sourceBus.id}`, busID = fresh(), busOwner = `bus:${busID}`, newGroupID = fresh();
    ownerMap.set(sourceOwner, busOwner);
    buses.push({ ...clone(sourceBus), id: busID, groupID: newGroupID, name: `${sourceBus.name || group.name || 'Group'} copy` });
    routing.buses = buses;
    routing.outputs ||= {};
    routing.outputs[busOwner] = has(routing.outputs, sourceOwner) ? routing.outputs[sourceOwner] : 'master';
    // Member copies output to the new bus where their source fed the source bus.
    for (const [from, to] of ownerMap) if (from.startsWith('track:') && routing.outputs[to] === sourceOwner) routing.outputs[to] = busOwner;
    const plugins = array(proposal.audioPlugins, 'Audio plug-ins', true);
    const busPlugins = plugins.filter(plugin => object(plugin) && plugin.owner === sourceOwner).map(plugin => {
      const copied = clone(plugin); copied.owner = busOwner; copied.instanceID = fresh();
      if (plugin.instanceID) instanceMap.set(plugin.instanceID, copied.instanceID);
      newPluginInstanceIDs.push(copied.instanceID);
      return copied;
    });
    if (busPlugins.length) proposal.audioPlugins = [...plugins, ...busPlugins];
    const sendMap = new Map();
    const sends = array(routing.sends, 'Routing sends', true);
    for (const send of sends.filter(send => send.source === sourceOwner)) { const copied = { ...clone(send), id: fresh(), source: busOwner }; sendMap.set(send.id, copied.id); sends.push(copied); }
    routing.sends = sends;
    const sidechains = array(routing.sidechains, 'Routing sidechains', true);
    const busInstances = new Set(busPlugins.length ? plugins.filter(plugin => plugin.owner === sourceOwner).map(plugin => plugin.instanceID) : []);
    for (const route of sidechains.filter(route => busInstances.has(route.destination))) {
      sidechains.push({ ...clone(route), id: fresh(), source: ownerMap.get(route.source) || route.source, destination: instanceMap.get(route.destination) });
    }
    routing.sidechains = sidechains;
    if (sends.length + sidechains.length > 128) fail('Group duplication would exceed 128 sends and sidechains combined.');
    for (const pattern of array(proposal.patterns, 'Patterns', true)) {
      const lanes = array(pattern.automation, 'Automation lanes', true), copies = lanes.filter(lane => object(lane) && lane.owner === sourceOwner).map(lane => {
        const copied = clone(lane); copied.id = fresh(); copied.owner = busOwner;
        if (instanceMap.has(copied.instanceID)) copied.instanceID = instanceMap.get(copied.instanceID);
        if (typeof copied.target === 'string' && copied.target.startsWith('send:') && sendMap.has(copied.target.slice(5))) copied.target = `send:${sendMap.get(copied.target.slice(5))}`;
        return copied;
      });
      if (copies.length) pattern.automation = [...lanes, ...copies];
    }
    for (const [from, to] of instanceMap) if (proposal.pluginPages?.[from]) proposal.pluginPages[to] = clone(proposal.pluginPages[from]);
    const record = { ...clone(group), id: newGroupID, busID, name: `${group.name || 'Group'} copy` };
    delete record.libraryPath; // Save kit must not overwrite the source group's kit file.
    proposal.groups = [...array(proposal.groups, 'Groups', true), record];
    for (const index of trackIndices) proposal.tracks[index].groupID = newGroupID;
    return { project: proposal, groupID: newGroupID, busID, sourceGroupID: groupID, sourceTrackIndices: members, trackIndices,
      instanceMap: Object.fromEntries(instanceMap), newPluginInstanceIDs };
  }
  const api = { build, buildGroup };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STTrackDuplicateProject = api;
})();
