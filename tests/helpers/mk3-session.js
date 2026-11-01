'use strict';
// Real app.js and MK3 modules (mk3-screens.js + mk3.js) without a browser or hardware: synthetic 'mk3.input'
// events drive the app; a fake native bridge answers. session() starts from the demo project.
const vm = require('node:vm');
const web = require('./web');

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: { setProperty() {} }, classList: { toggle() {}, remove() {}, add() {}, contains() { return false; } },
      focus() {}, select() {}, blur() {}, click() {}, remove() {}, append() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() {}, setAttribute() {}, removeAttribute() {}, getAttribute() { return null; }, scrollIntoView() {},
      closest() { return null; }, contains() { return false; }, querySelector() { return null; }, querySelectorAll() { return []; }, getBoundingClientRect() { return { left: 0, top: 0, width: 100, height: 100 }; }, isConnected: true });
    return elements.get(id);
  };
  const doc = { body: element('body'), activeElement: null, readyState: 'complete', querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element('new-' + tag + Math.random()) };
  const calls = [], clock = { now: 1000000 }, timers = [];
  let context, timerId = 0;
  const reply = message => {
    const p = message.payload || {};
    if (message.command === 'project.save') return {saved:true,project:p.project,path:'/tmp/mk3-kit.supertracker'};
    if (message.command === 'transport.recordArm') return { armed: !!p.armed };
    if (message.command === 'mk3.connect' || message.command === 'mk3.status') return { connected: true, screens: true, exclusive: !!p.exclusive };
    return {};
  };
  const win = { addEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, webkit: { messageHandlers: { supertracker: { postMessage(message) { calls.push(message); Promise.resolve().then(() => context.window.nativeReceive({ id: message.id, ok: true, result: reply(message) })); } } } } };
  const setTimeout_ = (fn, ms = 0) => { const id = ++timerId; timers.push({ id, fn, at: clock.now + Math.max(0, Number(ms) || 0) }); return id; };
  const clearTimeout_ = id => { const index = timers.findIndex(timer => timer.id === id); if (index >= 0) timers.splice(index, 1); };
  context = vm.createContext({ console, window: win, document: doc, performance: { now: () => clock.now }, requestAnimationFrame: fn => fn(), setTimeout: setTimeout_, clearTimeout: clearTimeout_, setInterval: () => 0, clearInterval() {},
    Date: class extends Date { static now() { return clock.now; } }, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, BigInt, Error, Event: class { constructor(type) { this.type = type; } }, innerWidth: 1280, innerHeight: 800, globalThis: null });
  context.globalThis = context.window;
  win.document = doc;
  for (const file of ['app.js', 'note-repeat.js', 'pattern-tools.js', 'song-model.js', 'song-sequence.js', 'plugin-pages.js', 'nks-controls.js', 'sampler.js', 'mk3-screens.js', 'mk3.js']) web.load(context, file);
  const MK3 = win.STMK3;
  MK3._clock = () => clock.now;
  // Run due timers; the controller's bursts and frame scheduling use setTimeout.
  const advance = (ms = 0) => { const end = clock.now + ms; for (;;) { timers.sort((a, b) => a.at - b.at); const next = timers[0]; if (!next || next.at > end) break; timers.shift(); clock.now = Math.max(clock.now, next.at); next.fn(); } clock.now = end; };
  return { load: file => web.load(context, file), win, ST: win.ST, state: win.ST.state, MK3, calls, clock, advance, element };
}


function session() {
  const h = harness(); h.state.project = h.ST.demoProject(); h.ST.normalize(h.state.project);
  h.state.track = 0; h.state.instrument = 0; h.state.row = 0;
  h.input = p => h.MK3.event('mk3.input', p);
  h.press = name => { h.input({type:'button', name, pressed:true}); h.input({type:'button', name, pressed:false}); };
  h.down = n => h.input({type:'pad', pad:n, phase:'down', velocity:100});
  h.up = n => h.input({type:'pad', pad:n, phase:'up'});
  return h;
}

/* A session whose cursor track is a member of a track group: demo tracks `tracks` (default 0–3) grouped as
   "Drums" (one group bus); the cursor stays on track 0. */
function groupSession(tracks = [0, 1, 2, 3]) {
  const h = session();
  h.state.track = tracks[0]; h.state.selection = { track: tracks[tracks.length - 1], row: 0 };
  if (h.ST.perform('groupTracks') !== true) throw new Error('grouping failed');
  const group = h.win.STGroups.list(h.state.project)[0];
  h.ST.mutate('rename group', p => h.win.STGroups.rename(p, group.id, 'Drums'));
  h.state.track = tracks[0]; h.state.instrument = h.state.project.tracks[tracks[0]].instrument || 0; h.state.selection = null;
  h.state.undo = []; h.state.redo = [];
  h.group = () => h.win.STGroups.byID(h.state.project, group.id);
  h.bus = () => h.state.project.routing.buses.find(bus => bus.id === group.busID);
  return h;
}

module.exports = { harness, session, groupSession };
