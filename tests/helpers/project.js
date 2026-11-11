'use strict';
// Hand-written project fixtures.

// The empty routing table every project has.
const routingTable = (extra = {}) => ({ buses: [], outputs: {}, sends: [], sidechains: [], ...extra });

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
   once and not booted) fills in everything else, as it does for a project the host hands over. Before that,
   a track without a routing identity gets its index as text (so that track 0 is the channel "track:0") and a
   descriptor written with the `track` shorthand its owner (see owned). The object passed in is completed in
   place and returned; what normalize adds are objects of this realm, so they compare equal to literals in a
   test. */
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

module.exports = { owned, routingTable, complete };
