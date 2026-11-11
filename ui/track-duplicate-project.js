/* Build an independent track proposal. Publishing and native state capture belong to the caller.
   The project is one the page holds, so it has the shape normalize gives every project (app-project-model.js);
   what is checked here is what a duplication can run into: a source that is not there, and the limits. */
(() => {
  'use strict';
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const has = (value, key) => Object.prototype.hasOwnProperty.call(value, key);
  const identifier = value => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value);
  const fail = message => { throw new Error(message); };
  const clone = value => JSON.parse(JSON.stringify(value));
  const trackLimit = () => (typeof window !== 'undefined' ? window : globalThis).TRACK_SANITY_LIMIT ?? Infinity; // defined by app-util.js
  function build(project, index, makeID) {
    const proposal = clone(project), { tracks, instruments, patterns, routing } = proposal;
    const clips = proposal.audioClips, plugins = proposal.audioPlugins, { sends, sidechains } = routing;
    if (!tracks[index]) fail('The source track does not exist.');
    const limit = trackLimit();
    if (tracks.length >= limit) fail(`Track duplication needs a new track; this session already has ${limit} tracks.`);
    const source = tracks[index], trackIndex = tracks.length;
    if (!instruments[source.instrument]) fail('The source track has an invalid assigned instrument.');
    const trackOwners = tracks.map(track => `track:${track.routingID}`), sourceOwner = trackOwners[index];
    // An automation lane names its channel by owner, or (the engine's own track parameters) its track by index.
    const laneOwner = lane => has(lane, 'owner') ? lane.owner : trackOwners[lane.track] ?? null;
    // The instruments the source column plays: the track's own and any a step or an extra note column names.
    const needed = new Set([source.instrument]);
    for (const pattern of patterns) for (const step of pattern.steps[index]) if (step) {
      for (const cell of [step, ...(step.columns || [])]) if (cell && instruments[cell.instrument]) needed.add(cell.instrument);
    }
    const sourceClips = clips.filter(clip => clip.track === index);
    if (clips.length + sourceClips.length > 256) fail('Track duplication would exceed 256 audio clips.');
    const sourcePlugins = plugins.filter(plugin => plugin.owner === sourceOwner);
    const sourceSends = sends.filter(send => send.source === sourceOwner);
    const copiedPluginIDs = new Set(sourcePlugins.map(plugin => plugin.instanceID).filter(Boolean));
    const sourceSidechains = sidechains.filter(route => copiedPluginIDs.has(route.destination));
    if (sends.length + sidechains.length + sourceSends.length + sourceSidechains.length > 128) fail('Track duplication would exceed 128 sends and sidechains combined.');

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
    for (const send of sourceSends) { const copied = clone(send); copied.id = fresh(); copied.source = newOwner; sendMap.set(send.id, copied.id); sends.push(copied); }
    for (const route of sourceSidechains) {
      const copied = clone(route); copied.id = fresh(); copied.destination = instanceMap.get(route.destination);
      if (copied.source === sourceOwner) copied.source = newOwner;
      sidechains.push(copied);
    }
    for (const pattern of patterns) {
      const column = pattern.steps[index].map(step => step && clone(step));
      for (const step of column) if (step) {
        if (instrumentMap.has(step.instrument)) step.instrument = instrumentMap.get(step.instrument);
        // Extra note columns (app-note-columns.js) play the copied instruments too.
        for (const cell of step.columns || []) if (cell && instrumentMap.has(cell.instrument)) cell.instrument = instrumentMap.get(cell.instrument);
        for (const key of ['fx1', 'fx2']) if (step[key]?.type === 'plugin' && instanceMap.has(step[key].instanceID)) step[key].instanceID = instanceMap.get(step[key].instanceID);
      }
      pattern.steps.push(column);
      const copies = pattern.automation.filter(lane => laneOwner(lane) === sourceOwner).map(lane => {
        const copied = clone(lane); copied.id = fresh();
        if (has(copied, 'owner')) copied.owner = newOwner; else copied.track = trackIndex;
        if (instanceMap.has(copied.instanceID)) copied.instanceID = instanceMap.get(copied.instanceID);
        if (typeof copied.target === 'string' && copied.target.startsWith('send:') && sendMap.has(copied.target.slice(5))) copied.target = `send:${sendMap.get(copied.target.slice(5))}`;
        return copied;
      });
      pattern.automation.push(...copies);
    }
    for (const clip of sourceClips) { const copied = clone(clip); copied.id = fresh(); copied.track = trackIndex; clips.push(copied); }
    // Each copied instrument takes its insert chain, with fresh instance identities.
    const chainCopies = [...instrumentMap].flatMap(([old, next]) => plugins.filter(plugin => plugin.owner === `instrument:${old}`).map(plugin => {
      const copied = clone(plugin); copied.owner = `instrument:${next}`;
      copied.instanceID = fresh();
      if (plugin.instanceID) instanceMap.set(plugin.instanceID, copied.instanceID);
      if (plugin.instanceID && proposal.pluginPages[plugin.instanceID]) proposal.pluginPages[copied.instanceID] = clone(proposal.pluginPages[plugin.instanceID]);
      return copied;
    }));
    if (chainCopies.length) for (const pattern of patterns) for (const step of pattern.steps[trackIndex]) if (step) {
      for (const key of ['fx1', 'fx2']) if (step[key]?.type === 'plugin' && instanceMap.has(step[key].instanceID)) step[key].instanceID = instanceMap.get(step[key].instanceID);
    }
    plugins.push(...pluginCopies, ...chainCopies);
    routing.outputs[newOwner] = has(routing.outputs, sourceOwner) ? routing.outputs[sourceOwner] : 'master';
    tracks.push(newTrack);
    return { project: proposal, trackIndex, sourceTrackIndex: index, instrumentMap: Object.fromEntries(instrumentMap), instanceMap: Object.fromEntries(instanceMap), newPluginInstanceIDs: pluginCopies.map(plugin => plugin.instanceID) };
  }
  /* A whole track group (docs/KITS.md): every member is duplicated with build() (instruments, plug-ins,
     pattern columns, automation, sends, clips), then the copies get a new group with a copy of the group bus: its
     mix, output, inserts (fresh instances), sends, sidechains and automation. The copies are appended at the end
     (the caller moves them after the source group); members output to the new bus. */
  function buildGroup(project, groupID, makeID) {
    const group = project.groups.find(item => item.id === groupID);
    if (!group) fail('The source group does not exist.');
    const members = project.tracks.flatMap((track, index) => track.groupID === groupID ? [index] : []);
    const limit = trackLimit();
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
    const routing = proposal.routing, { buses } = routing, sourceBus = buses.find(bus => bus.id === group.busID);
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
    routing.outputs[busOwner] = has(routing.outputs, sourceOwner) ? routing.outputs[sourceOwner] : 'master';
    // Member copies output to the new bus where their source fed the source bus.
    for (const [from, to] of ownerMap) if (from.startsWith('track:') && routing.outputs[to] === sourceOwner) routing.outputs[to] = busOwner;
    const plugins = proposal.audioPlugins, sourceBusPlugins = plugins.filter(plugin => plugin.owner === sourceOwner);
    const busPlugins = sourceBusPlugins.map(plugin => {
      const copied = clone(plugin); copied.owner = busOwner; copied.instanceID = fresh();
      if (plugin.instanceID) instanceMap.set(plugin.instanceID, copied.instanceID);
      newPluginInstanceIDs.push(copied.instanceID);
      return copied;
    });
    plugins.push(...busPlugins);
    const sendMap = new Map();
    const sends = routing.sends;
    for (const send of sends.filter(send => send.source === sourceOwner)) { const copied = { ...clone(send), id: fresh(), source: busOwner }; sendMap.set(send.id, copied.id); sends.push(copied); }
    const sidechains = routing.sidechains;
    const busInstances = new Set(sourceBusPlugins.map(plugin => plugin.instanceID));
    for (const route of sidechains.filter(route => busInstances.has(route.destination))) {
      sidechains.push({ ...clone(route), id: fresh(), source: ownerMap.get(route.source) || route.source, destination: instanceMap.get(route.destination) });
    }
    if (sends.length + sidechains.length > 128) fail('Group duplication would exceed 128 sends and sidechains combined.');
    for (const pattern of proposal.patterns) {
      const copies = pattern.automation.filter(lane => lane.owner === sourceOwner).map(lane => {
        const copied = clone(lane); copied.id = fresh(); copied.owner = busOwner;
        if (instanceMap.has(copied.instanceID)) copied.instanceID = instanceMap.get(copied.instanceID);
        if (typeof copied.target === 'string' && copied.target.startsWith('send:') && sendMap.has(copied.target.slice(5))) copied.target = `send:${sendMap.get(copied.target.slice(5))}`;
        return copied;
      });
      pattern.automation.push(...copies);
    }
    for (const [from, to] of instanceMap) if (proposal.pluginPages[from]) proposal.pluginPages[to] = clone(proposal.pluginPages[from]);
    const record = { ...clone(group), id: newGroupID, busID, name: `${group.name || 'Group'} copy` };
    delete record.libraryPath; // Save kit must not overwrite the source group's kit file.
    proposal.groups.push(record);
    for (const index of trackIndices) proposal.tracks[index].groupID = newGroupID;
    return { project: proposal, groupID: newGroupID, busID, sourceGroupID: groupID, sourceTrackIndices: members, trackIndices,
      instanceMap: Object.fromEntries(instanceMap), newPluginInstanceIDs };
  }
  const api = { build, buildGroup };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STTrackDuplicateProject = api;
})();
