'use strict';
// Hand-written project fixtures.

// The empty routing table every project has.
const routingTable = (extra = {}) => ({ buses: [], outputs: {}, sends: [], sidechains: [], ...extra });

/* Gives a hand-written project the shape every project in the page has (normalize in
   ui/app-project-model.js, descriptors as the host answers them): a routing table, a routing
   identity for every track (its index as text unless the fixture names one, so that track 0 is the
   channel "track:0") and an owner on every plug-in descriptor (see owned). */
function routed(project) {
  project.tracks.forEach((track, index) => { track.routingID ??= String(index); });
  project.routing = routingTable(project.routing);
  return owned(project);
}

// A fixture may write `track: <index>` on a plug-in descriptor as shorthand for the owner of that track's
// channel: the descriptor gets the owner and loses the shorthand, which is not part of a project.
function owned(project) {
  for (const plugin of project.audioPlugins || []) {
    if (!plugin.owner && Number.isInteger(plugin.track)) plugin.owner = `track:${project.tracks[plugin.track].routingID}`;
    delete plugin.track;
  }
  return project;
}

module.exports = { routed, owned, routingTable };
