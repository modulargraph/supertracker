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

/* A complete project from the parts a test cares about: the page's own normalize (the app's modules, loaded
   once and not booted) fills in everything else, as it does for a project the host hands over. Tracks without a
   routing identity get their index, and descriptors their owner, as routed() does. The object passed in is
   completed in place and returned; what normalize adds are objects of this realm, so they compare equal to
   literals in a test. */
let page = null;
function complete(project = {}) {
  page ||= require('./web').app().ST;
  (project.tracks || []).forEach((track, index) => { track.routingID ??= String(index); });
  owned(project);
  return local(page.normalize(project));
}

// Objects and arrays that another realm made (normalize ran in the page's context), remade here.
function local(value) {
  if (!value || typeof value !== 'object') return value;
  const proto = Object.getPrototypeOf(value);
  let result = value;
  if (Array.isArray(value) && proto !== Array.prototype) result = Array.from(value);
  else if (!Array.isArray(value) && proto !== Object.prototype && proto !== null) result = { ...value };
  for (const key of Object.keys(result)) result[key] = local(result[key]);
  return result;
}

module.exports = { routed, owned, routingTable, complete };
