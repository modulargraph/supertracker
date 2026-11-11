/* Persistent routing identity and explicit project edits; no audio graph mutation. */
(() => {
  'use strict';
  const trackOwner = track => typeof track?.routingID === 'string' && track.routingID ? `track:${track.routingID}` : null;

  function ensureTrackIDs(project, makeID) {
    const { tracks } = project;
    const occupied = new Set(tracks.map(track => track.routingID).filter(id => typeof id === 'string' && id));
    for (const track of tracks) {
      if (typeof track.routingID === 'string' && track.routingID) continue;
      const base = String(makeID?.() || globalThis.crypto?.randomUUID?.() || `track-${Date.now().toString(36)}`);
      let id = base, suffix = 1;
      while (occupied.has(id)) id = `${base}-${suffix++}`;
      track.routingID = id; occupied.add(id);
    }
    // Explicit duplicate identities remain intact: the host's validation
    // explains them rather than the page silently rerouting audio.
    return project;
  }

  function appendTrack(project, track, makeID) {
    project.tracks.push(track);
    ensureTrackIDs(project, makeID);
    const routing = project.routing, owner = trackOwner(track);
    if (!Object.hasOwn(routing.outputs, owner)) routing.outputs[owner] = 'master';
    return track;
  }

  function removeTrackReferences(project, track, removedPlugins = []) {
    const owner = trackOwner(track), routing = project.routing;
    if (!owner) return;
    delete routing.outputs[owner];
    // An explicitly removed destination becomes disconnected, never another track.
    for (const source of Object.keys(routing.outputs)) if (routing.outputs[source] === owner) routing.outputs[source] = null;
    const removedSends = new Set(routing.sends.filter(send => send.source === owner || send.destination === owner).map(send => `send:${send.id}`));
    routing.sends = routing.sends.filter(send => send.source !== owner && send.destination !== owner);
    for (const pattern of project.patterns) pattern.automation = pattern.automation.filter(lane => lane.owner !== owner && !removedSends.has(lane.target));
    const instances = new Set(removedPlugins.map(plugin => plugin.instanceID));
    routing.sidechains = routing.sidechains.filter(route => route.source !== owner && !instances.has(route.destination));
  }

  /* Drops the plug-in descriptors that belong to nothing: the owner is a deleted track or bus, an instrument
     index past the end, or no owner at all. The host refuses a project with such a descriptor, so one of them
     would block every later change. Returns how many were dropped. */
  function pruneOrphanPlugins(project) {
    const tracks = new Set(project.tracks.map(trackOwner)),
      buses = new Set(project.routing.buses.map(bus => 'bus:' + bus.id)),
      instruments = project.instruments.length;
    const live = item => {
      const owner = String(item.owner);
      if (owner === 'master') return true;
      if (owner.startsWith('track:')) return tracks.has(owner);
      if (owner.startsWith('bus:')) return buses.has(owner);
      const instrument = owner.match(/^instrument:(\d+)$/);
      return !!instrument && Number(instrument[1]) < instruments;
    };
    const before = project.audioPlugins.length;
    if (project.audioPlugins.every(live)) return 0;
    project.audioPlugins = project.audioPlugins.filter(live);
    return before - project.audioPlugins.length;
  }

  /* The plug-in of a plug-in instrument (type 'plugin'). It is in one place: the `audioPlugins` entry of kind
     `instrument` on a track that plays the instrument. Only while no track plays it does the instrument keep
     the descriptor itself (`instruments[i].plugin`, without owner or instance), so that it can be assigned to
     a track again with its settings. */
  const trackInstrumentPlugin = (project, track) => project.audioPlugins.find(plugin => plugin.kind === 'instrument' && plugin.owner === trackOwner(track));
  function instrumentPlugin(project, index) {
    for (const track of project.tracks) {
      const plugin = track.instrument === index && trackInstrumentPlugin(project, track);
      if (plugin) return plugin;
    }
    return project.instruments[index]?.plugin || null;
  }
  /* Called before `tracks` stop playing their instruments (reassigned or removed): a plug-in instrument
     that none of the remaining tracks plays keeps the descriptor of the first of them. */
  function keepInstrumentPlugins(project, tracks) {
    const leaving = new Set(tracks);
    for (const track of tracks) {
      const index = track.instrument, instrument = project.instruments[index], plugin = trackInstrumentPlugin(project, track);
      if (!plugin || instrument?.type !== 'plugin' || instrument.plugin) continue;
      if (project.tracks.some(other => !leaving.has(other) && other.instrument === index && trackInstrumentPlugin(project, other))) continue;
      const { owner, instanceID, ...kept } = JSON.parse(JSON.stringify(plugin));
      instrument.plugin = kept;
    }
  }

  const api = { ensureTrackIDs, trackOwner, appendTrack, removeTrackReferences, pruneOrphanPlugins, instrumentPlugin, keepInstrumentPlugins };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STRoutingProject = api;
})();
