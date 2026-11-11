/* Track groups. A group is a contiguous run of ordinary tracks (its members) routed to one
   group bus: the bus has its own fader, mute/solo, inserts and sends and outputs to Master.
   Kits load as groups (one member per loaded pad). This module only edits project data; the
   caller wraps an edit in ST.mutate so it is one Undo step and the engine follows.

   Project data:
     project.groups[] = { id, name, color, collapsed, mixerCollapsed, busID, libraryPath?, importNotes? }
     track.groupID     = the group a track belongs to (members are contiguous in track order)
     track.pad         = kit pad position 0..15 (Replace kit and Save kit match pads by it)
     track.padNote     = note that Pad keys / MK3 pads enter on this member (default C-4 = 60)
     track.chokeGroup  = 0 (off) or 1..16; a note on one member stops sounding notes of other
                         members of the same group with the same choke group
     track.voiceMode   = optional 'poly' | 'mono' | 'legato', overrides the instrument's mode
   The group bus is routing.buses[] { id: busID, type: 'group', groupID } and every member
   outputs to it: routing.outputs['track:<routingID>'] = 'bus:<busID>'. */
(() => {
  'use strict';
  const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const COLORS = ['#e2a44f', '#7fb7d9', '#9fc97a', '#d98a9e', '#b39ddb', '#80cbc4', '#e6d36f', '#ef8a5b'];
  const fallbackID = () => globalThis.crypto?.randomUUID?.() || `g${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;

  const list = p => Array.isArray(p?.groups) ? p.groups : [];
  const byID = (p, id) => list(p).find(group => group.id === id) || null;
  const idOf = group => typeof group === 'string' ? group : group?.id;
  const busOwner = group => group?.busID ? `bus:${group.busID}` : null;
  const trackOwner = track => typeof track?.routingID === 'string' && track.routingID ? `track:${track.routingID}` : null;
  const ofTrack = (p, index) => {
    const id = p?.tracks?.[index]?.groupID;
    return id ? byID(p, id) : null;
  };
  function members(p, group) {
    const id = idOf(group), result = [];
    (p?.tracks || []).forEach((track, index) => { if (id && track?.groupID === id) result.push(index); });
    return result;
  }
  function span(p, group) {
    const found = members(p, group);
    return found.length ? { first: found[0], last: found[found.length - 1], count: found.length } : null;
  }
  const groupOfBus = (p, owner) => list(p).find(group => busOwner(group) === owner) || null;
  // Owners whose audio a group contains: its bus and every member channel.
  const owners = (p, group) => [busOwner(group), ...members(p, group).map(index => trackOwner(p.tracks[index]))].filter(Boolean);

  // The project's routing table with its four collections (a saved table may leave empty ones out).
  function ensureRouting(p) {
    const r = p.routing;
    r.buses ||= []; r.outputs ||= {}; r.sends ||= []; r.sidechains ||= [];
    return r;
  }

  function ensureBus(p, group) {
    const r = ensureRouting(p);
    let bus = r.buses.find(item => item.id === group.busID);
    if (!bus) {
      bus = { id: group.busID, name: group.name, type: 'group', volume: 1, pan: 0, mute: false, solo: false };
      r.buses.push(bus);
    }
    bus.type = 'group';
    bus.groupID = group.id;
    if (!Object.hasOwn(r.outputs, busOwner(group))) r.outputs[busOwner(group)] = 'master';
    return bus;
  }

  /* Reorders tracks: order[newIndex] = oldIndex. Every index-based reference follows
     (pattern columns, automation lanes, clips, plug-in descriptors, frozen clips).
     Returns map[oldIndex] = newIndex. */
  function reorder(p, order) {
    const n = p.tracks.length;
    if (order.length !== n || new Set(order).size !== n || order.some(i => !Number.isInteger(i) || i < 0 || i >= n)) throw new Error('Invalid track order.');
    const map = new Array(n);
    order.forEach((old, next) => { map[old] = next; });
    if (order.every((old, next) => old === next)) return map;
    p.tracks = order.map(old => p.tracks[old]);
    p.tracks.forEach((track, index) => { track.id = index; });
    const fix = item => { if (object(item) && Number.isInteger(item.track) && item.track >= 0 && item.track < n) item.track = map[item.track]; };
    for (const pattern of p.patterns || []) {
      const steps = Array.isArray(pattern.steps) ? pattern.steps : [];
      pattern.steps = order.map(old => steps[old] || Array(pattern.length).fill(null));
      (pattern.automation || []).forEach(fix);
    }
    for (const key of ['audioClips', 'patternClips']) (p[key] || []).forEach(fix);
    return map;
  }

  // Contiguous order: every group's members follow its first member, in their current order.
  function contiguousOrder(p) {
    const order = [], placed = new Set();
    p.tracks.forEach((track, index) => {
      if (placed.has(index)) return;
      const id = track?.groupID;
      if (id && byID(p, id)) {
        for (const member of members(p, id)) { order.push(member); placed.add(member); }
      } else { order.push(index); placed.add(index); }
    });
    return order;
  }

  function routeMember(p, index, group) {
    const owner = trackOwner(p.tracks[index]);
    if (!owner || !group) return;
    ensureRouting(p).outputs[owner] = busOwner(group);
  }
  function unrouteMember(p, index, group) {
    const owner = trackOwner(p.tracks[index]), outputs = p.routing.outputs;
    if (owner && outputs && (!group || outputs[owner] === busOwner(group))) outputs[owner] = 'master';
  }

  function nextColor(p) {
    const used = new Set(list(p).map(group => group.color));
    return COLORS.find(color => !used.has(color)) || COLORS[list(p).length % COLORS.length];
  }

  /* New group from tracks (indices). Members leave any group they were in, move next to the
     first of them and route to a new group bus. Returns { group, map }. */
  function create(p, indices, options = {}) {
    const makeID = options.makeID || fallbackID;
    const chosen = [...new Set(indices)].filter(i => Number.isInteger(i) && p.tracks[i]).sort((a, b) => a - b);
    const group = {
      id: options.id || makeID(),
      name: String(options.name || `Group ${list(p).length + 1}`),
      color: options.color || nextColor(p),
      collapsed: !!options.collapsed,
      mixerCollapsed: !!options.mixerCollapsed,
      busID: options.busID || makeID()
    };
    for (const key of ['libraryPath', 'importNotes']) if (options[key] !== undefined) group[key] = options[key];
    p.groups ||= [];
    p.groups.push(group);
    const bus = ensureBus(p, group);
    if (options.bus) Object.assign(bus, options.bus, { id: group.busID, type: 'group', groupID: group.id });
    bus.name = group.name;
    for (const index of chosen) {
      const previous = ofTrack(p, index);
      if (previous) unrouteMember(p, index, previous);
      p.tracks[index].groupID = group.id;
      routeMember(p, index, group);
    }
    const map = reorder(p, contiguousOrder(p));
    dropEmpty(p, { removeBuses: true });
    return { group, map };
  }

  /* Adds a track to a group: it moves to the end of the group (or to position `at` inside it)
     and routes to the group bus. Returns map[old] = new. */
  function addTrack(p, groupID, index, at) {
    const group = byID(p, groupID);
    if (!group || !p.tracks[index]) throw new Error('Choose an existing group and track.');
    const previous = ofTrack(p, index);
    if (previous === group) return reorder(p, p.tracks.map((_, i) => i));
    if (previous) unrouteMember(p, index, previous);
    const before = members(p, group);
    p.tracks[index].groupID = group.id;
    routeMember(p, index, group);
    const order = p.tracks.map((_, i) => i).filter(i => i !== index);
    if (before.length) {
      const slot = Math.max(0, Math.min(before.length, Number.isInteger(at) ? at : before.length));
      order.splice(slot === before.length ? order.indexOf(before[before.length - 1]) + 1 : order.indexOf(before[slot]), 0, index);
    } else order.splice(index, 0, index);
    const map = reorder(p, order);
    dropEmpty(p, { removeBuses: true });
    return map;
  }

  /* Takes a track out of its group: it moves just after the group and routes to Master
     (when it was routed to the group bus). Removes the group when it becomes empty. */
  function removeTrack(p, index, options = {}) {
    const group = ofTrack(p, index);
    if (!group) return reorder(p, p.tracks.map((_, i) => i));
    const rest = members(p, group).filter(i => i !== index);
    delete p.tracks[index].groupID;
    unrouteMember(p, index, group);
    const order = p.tracks.map((_, i) => i).filter(i => i !== index);
    if (rest.length) order.splice(order.indexOf(rest[rest.length - 1]) + 1, 0, index); else order.splice(index, 0, index);
    const map = reorder(p, order);
    if (options.keepEmpty !== true) dropEmpty(p, { removeBuses: true });
    return map;
  }

  /* Moves a track one step left (delta -1) or right (+1). Group edges are a step of their own:
     a track next to an expanded group joins it at that edge, and a member at its group's edge
     leaves it, both without changing the order. A collapsed group is hopped over as a whole. */
  function moveTrack(p, from, delta) {
    const n = p.tracks.length, track = p.tracks[from], identity = p.tracks.map((_, i) => i);
    if (!track || !delta) return reorder(p, identity);
    delta = delta < 0 ? -1 : 1;
    const group = ofTrack(p, from);
    if (group) {
      const edge = span(p, group);
      if (delta < 0 ? from === edge.first : from === edge.last) {
        unrouteMember(p, from, group);
        delete track.groupID;
        dropEmpty(p, { removeBuses: true });
        return identity;
      }
    } else {
      const neighbour = from + delta, other = ofTrack(p, neighbour);
      if (neighbour < 0 || neighbour >= n) return reorder(p, identity);
      if (other && !other.collapsed) {
        track.groupID = other.id;
        routeMember(p, from, other);
        return identity;
      }
      if (other?.collapsed) {
        const edge = span(p, other), order = identity.filter(i => i !== from);
        order.splice(delta < 0 ? order.indexOf(edge.first) : order.indexOf(edge.last) + 1, 0, from);
        return reorder(p, order);
      }
    }
    const to = from + delta, order = [...identity];
    [order[from], order[to]] = [order[to], order[from]];
    return reorder(p, order);
  }

  /* Moves tracks (indices, any order) as one block so it lands just before track `before` (an index in the
     current order; tracks.length is the end). `groupID` is the group the moved tracks belong to afterwards
     (null: none); joining or leaving re-routes them. Groups stay contiguous: a block dropped inside a group it
     does not join lands after that group. Empty groups are removed. Returns map[old] = new. */
  function moveTracks(p, indices, before, groupID = null) {
    const n = p.tracks.length, identity = p.tracks.map((_, i) => i);
    const moving = [...new Set(indices)].filter(i => Number.isInteger(i) && p.tracks[i]).sort((a, b) => a - b);
    if (!moving.length) return identity;
    const target = groupID ? byID(p, groupID) : null;
    if (groupID && !target) throw new Error('Choose an existing group.');
    for (const index of moving) {
      const previous = ofTrack(p, index);
      if (previous === target) continue;
      if (previous) unrouteMember(p, index, previous);
      if (target) { p.tracks[index].groupID = target.id; routeMember(p, index, target); } else delete p.tracks[index].groupID;
    }
    const chosen = new Set(moving), rest = identity.filter(i => !chosen.has(i));
    const at = rest.filter(i => i < Math.max(0, Math.min(n, Number(before) || 0))).length;
    let map = reorder(p, [...rest.slice(0, at), ...moving, ...rest.slice(at)]);
    const order = contiguousOrder(p);
    if (order.some((old, next) => old !== next)) {
      const second = reorder(p, order);
      map = map.map(index => second[index]);
    }
    dropEmpty(p, { removeBuses: true });
    return map;
  }

  /* Deletes tracks (indices) and everything that belongs to them: pattern columns, automation lanes, clips,
     their plug-ins (inserts and hosted instruments), routing outputs, sends and sidechains. A channel that
     output to a deleted track becomes disconnected (null), as routing-project.js does. A group left without
     members is removed with its bus. Instruments stay. Returns map[old] = new (-1 for a deleted track). */
  function removeTracks(p, indices) {
    const n = p.tracks.length;
    const removing = [...new Set(indices)].filter(i => Number.isInteger(i) && p.tracks[i]).sort((a, b) => a - b);
    if (!removing.length) return p.tracks.map((_, i) => i);
    if (removing.length >= n) throw new Error('Keep at least one track in a session.');
    const chosen = new Set(removing), owners = new Set(removing.map(i => trackOwner(p.tracks[i])).filter(Boolean));
    const ownsPlugin = plugin => object(plugin) && owners.has(plugin.owner);
    // An automation lane names its channel, or the track index for the engine's instrument parameters.
    const ownsLane = lane => object(lane) && (lane.owner ? owners.has(lane.owner) : Number.isInteger(lane.track) && chosen.has(lane.track));
    const plugins = (p.audioPlugins || []).filter(ownsPlugin), instances = new Set(plugins.map(plugin => plugin.instanceID).filter(Boolean));
    p.audioPlugins = (p.audioPlugins || []).filter(plugin => !ownsPlugin(plugin));
    for (const id of instances) if (p.pluginPages) delete p.pluginPages[id];
    const r = ensureRouting(p);
    for (const owner of owners) delete r.outputs[owner];
    for (const source of Object.keys(r.outputs)) if (owners.has(r.outputs[source])) r.outputs[source] = null;
    const removedSends = new Set(r.sends.filter(send => owners.has(send.source) || owners.has(send.destination)).map(send => `send:${send.id}`));
    r.sends = r.sends.filter(send => !owners.has(send.source) && !owners.has(send.destination));
    r.sidechains = r.sidechains.filter(route => !owners.has(route.source) && !instances.has(route.destination));
    for (const pattern of p.patterns || []) if (Array.isArray(pattern.automation)) pattern.automation = pattern.automation.filter(lane => !ownsLane(lane) && !removedSends.has(lane.target));
    for (const key of ['audioClips', 'patternClips']) if (Array.isArray(p[key])) p[key] = p[key].filter(item => !(Number.isInteger(item?.track) && chosen.has(item.track)));
    const groups = new Set(removing.map(i => ofTrack(p, i)).filter(Boolean));
    // Deleted tracks move to the end (every reference follows), then the tail is cut off.
    const keep = p.tracks.map((_, i) => i).filter(i => !chosen.has(i)), map = reorder(p, [...keep, ...removing]);
    const length = keep.length;
    p.tracks.length = length;
    for (const pattern of p.patterns || []) if (Array.isArray(pattern.steps)) pattern.steps.length = Math.min(pattern.steps.length, length);
    for (const group of groups) if (!members(p, group).length) dissolve(p, group.id);
    return map.map(next => next < length ? next : -1);
  }

  // Removes the bus channel and everything attached to it (inserts, sends, automation).
  function removeBus(p, group, replacement = 'master') {
    const owner = busOwner(group);
    if (!owner) return;
    const r = ensureRouting(p);
    const removed = new Set((p.audioPlugins || []).filter(plugin => plugin.owner === owner).map(plugin => plugin.instanceID));
    r.buses = r.buses.filter(bus => `bus:${bus.id}` !== owner);
    delete r.outputs[owner];
    for (const source of Object.keys(r.outputs)) if (r.outputs[source] === owner) r.outputs[source] = replacement;
    const removedSends = new Set(r.sends.filter(send => send.source === owner || send.destination === owner).map(send => `send:${send.id}`));
    r.sends = r.sends.filter(send => send.source !== owner && send.destination !== owner);
    r.sidechains = r.sidechains.filter(route => route.source !== owner && !removed.has(route.destination));
    p.audioPlugins = (p.audioPlugins || []).filter(plugin => plugin.owner !== owner);
    for (const id of removed) if (p.pluginPages) delete p.pluginPages[id];
    for (const pattern of p.patterns || []) if (pattern.automation) pattern.automation = pattern.automation.filter(lane => lane.owner !== owner && !removedSends.has(lane.target));
  }

  // Ungroups every member (they route to Master) and removes the group and its bus.
  function dissolve(p, groupID) {
    const group = byID(p, groupID);
    if (!group) return false;
    for (const index of members(p, group)) { unrouteMember(p, index, group); delete p.tracks[index].groupID; }
    removeBus(p, group);
    p.groups = list(p).filter(item => item !== group);
    return true;
  }

  function dropEmpty(p, { removeBuses = false } = {}) {
    for (const group of [...list(p)]) if (!members(p, group).length) {
      if (removeBuses) removeBus(p, group);
      p.groups = list(p).filter(item => item !== group);
    }
  }

  function rename(p, groupID, name) {
    const group = byID(p, groupID);
    if (!group || !String(name || '').trim()) return false;
    group.name = String(name).trim();
    const bus = p.routing.buses?.find(item => item.id === group.busID);
    if (bus) bus.name = group.name;
    return true;
  }

  /* Repairs loaded data: unknown memberships are dropped, empty groups removed, members made
     contiguous and every group gets its bus. Returns true when something changed. */
  function normalize(p) {
    if (!object(p) || !Array.isArray(p.tracks)) return false;
    let changed = false;
    if (p.groups !== undefined && !Array.isArray(p.groups)) { delete p.groups; changed = true; }
    const seen = new Set();
    if (Array.isArray(p.groups)) {
      p.groups = p.groups.filter(group => {
        const valid = object(group) && typeof group.id === 'string' && group.id && !seen.has(group.id);
        if (valid) seen.add(group.id); else changed = true;
        return valid;
      });
    }
    for (const track of p.tracks) if (object(track) && track.groupID !== undefined && !seen.has(track.groupID)) { delete track.groupID; changed = true; }
    for (const group of list(p)) {
      if (typeof group.name !== 'string' || !group.name) { group.name = 'Group'; changed = true; }
      if (typeof group.color !== 'string') { group.color = nextColor(p); changed = true; }
      if (typeof group.busID !== 'string' || !group.busID) { group.busID = `${group.id}-bus`; changed = true; }
      group.collapsed = group.collapsed === true;
      group.mixerCollapsed = group.mixerCollapsed === true;
    }
    const before = list(p).length;
    dropEmpty(p);
    if (list(p).length !== before) changed = true;
    if (!list(p).length) { if (Array.isArray(p.groups) && !p.groups.length) delete p.groups; return changed; }
    const order = contiguousOrder(p);
    if (order.some((old, next) => old !== next)) { reorder(p, order); changed = true; }
    for (const group of list(p)) {
      const had = !!p.routing.buses?.some(bus => bus.id === group.busID && bus.type === 'group' && bus.groupID === group.id);
      ensureBus(p, group);
      if (!had) changed = true;
    }
    return changed;
  }

  /* Visible columns of the pattern grid: ordinary tracks, expanded members, and one overview
     column per collapsed group (index = its first member). */
  function columns(p) {
    const result = [];
    p.tracks.forEach((track, index) => {
      const group = ofTrack(p, index);
      if (group?.collapsed) {
        if (members(p, group)[0] === index) result.push({ track, index, group, collapsed: true, members: members(p, group) });
      } else result.push({ track, index, group: group || null });
    });
    return result;
  }

  const api = {
    COLORS, list, byID, ofTrack, members, span, owners, busOwner, trackOwner, groupOfBus, ensureRouting, ensureBus,
    reorder, contiguousOrder, create, addTrack, removeTrack, moveTrack, moveTracks, removeTracks, removeBus, dissolve, rename, normalize, columns
  };
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (typeof window !== 'undefined') window.STGroups = api;
})();
