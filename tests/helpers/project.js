'use strict';
// Hand-written project fixtures.

// The empty routing table every project has.
const routingTable = (extra = {}) => ({ version: 1, mixModel: 'standard', buses: [], outputs: {}, sends: [], sidechains: [], ...extra });

/* Gives a hand-written project the shape every project in the page has (normalize in
   ui/app-project-model.js, descriptors as the host answers them): a routing table, a routing
   identity for every track (its index as text unless the fixture names one, so that track 0 is the
   channel "track:0") and the owner on every plug-in descriptor that only names its track. */
function routed(project) {
  project.tracks.forEach((track, index) => { track.routingID ??= String(index); });
  project.routing = routingTable(project.routing);
  return owned(project);
}

// The owner (as the host writes it) on every plug-in descriptor of a fixture that only names its track.
function owned(project) {
  for (const plugin of project.audioPlugins || []) {
    if (!plugin.owner && Number.isInteger(plugin.track)) plugin.owner = `track:${project.tracks[plugin.track].routingID}`;
  }
  return project;
}

module.exports = { routed, owned, routingTable };
