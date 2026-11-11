/* Persistent routing identity and explicit project edits; no audio graph mutation. */
(() => {
  'use strict';
  const object = value => value && typeof value === 'object' && !Array.isArray(value);
  const trackOwner = track => typeof track?.routingID === 'string' && track.routingID ? `track:${track.routingID}` : null;

  function ensureTrackIDs(project, makeID) {
    const tracks = Array.isArray(project?.tracks) ? project.tracks : [];
    const occupied = new Set(tracks.map(track => track?.routingID).filter(id => typeof id === 'string' && id));
    for (const track of tracks) {
      if (!object(track) || typeof track.routingID==='string'&&track.routingID) continue;
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
    routing.outputs ||= {};
    if (!Object.hasOwn(routing.outputs, owner)) routing.outputs[owner] = 'master';
    return track;
  }

  function removeTrackReferences(project, track, removedPlugins = []) {
    const owner = trackOwner(track), routing = project.routing;
    if (!owner) return;
    for (const pattern of project.patterns || []) if (Array.isArray(pattern.automation)) pattern.automation = pattern.automation.filter(lane => lane.owner !== owner);
    if (object(routing.outputs)) {
      delete routing.outputs[owner];
      // An explicitly removed destination becomes disconnected, never another track.
      for (const source of Object.keys(routing.outputs)) if (routing.outputs[source] === owner) routing.outputs[source] = null;
    }
    if (Array.isArray(routing.sends)) {
      const removedSends = new Set(routing.sends.filter(send => send.source === owner || send.destination === owner).map(send => `send:${send.id}`));
      routing.sends = routing.sends.filter(send => send.source !== owner && send.destination !== owner);
      for (const pattern of project.patterns || []) if (Array.isArray(pattern.automation)) pattern.automation = pattern.automation.filter(lane => !removedSends.has(lane.target));
    }
    const instances = new Set(removedPlugins.map(plugin => plugin.instanceID).filter(Boolean));
    if (Array.isArray(routing.sidechains)) routing.sidechains = routing.sidechains.filter(route => route.source !== owner && !instances.has(route.destination?.instanceID));
  }

  /* Drops plug-in descriptors whose owner is gone: a deleted track or bus, or an instrument index past the end.
     The audio host rejects a project with such an owner, so one stale descriptor (left by a deleted track) would
     block every later change. Returns how many were dropped. */
  function pruneOrphanPlugins(project) {
    if (!Array.isArray(project?.audioPlugins)) return 0;
    const tracks = new Set((project.tracks || []).map(trackOwner).filter(Boolean)),
      buses = new Set((project.routing.buses || []).map(bus => 'bus:' + bus.id)),
      instruments = (project.instruments || []).length;
    const live = item => {
      const owner = item?.owner;
      if (typeof owner !== 'string') return true;
      if (owner.startsWith('track:')) return tracks.has(owner);
      if (owner.startsWith('bus:')) return buses.has(owner);
      const instrument = owner.match(/^instrument:(\d+)$/);
      return instrument ? Number(instrument[1]) < instruments : true;
    };
    const before = project.audioPlugins.length;
    if (project.audioPlugins.every(live)) return 0;
    project.audioPlugins = project.audioPlugins.filter(live);
    return before - project.audioPlugins.length;
  }

  const api = { ensureTrackIDs, trackOwner, appendTrack, removeTrackReferences, pruneOrphanPlugins };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STRoutingProject = api;
})();
