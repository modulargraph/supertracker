'use strict';
// Render selection to sample: the pure model (selection, names, the applied edit), command
// availability (palette, shortcut, help, context menu), and the dialog workflow against a
// stubbed native host: one Undo step for a plain render and for bounce in place, cancel,
// progress events and refusals.
const vm = require('node:vm'), assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };

function harness() {
  const nodes = new Map(), calls = [], handlers = new Map(), keydown = [];
  const node = selector => {
    const key = String(selector).replace(/^#/, '');
    if (!nodes.has(key)) nodes.set(key, { id: key, value: '', checked: false, hidden: false, disabled: false, textContent: '', dataset: {}, innerHTML: '', style: {}, attributes: {},
      classList: { toggle() {}, add() {}, remove() {}, contains() { return false; } }, focus() { document.activeElement = this; }, select() {}, scrollIntoView() {},
      closest() { return null; }, querySelector() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, setAttribute(name, value) { this.attributes[name] = String(value); },
      appendChild() {}, remove() {}, click() { return this.onclick?.(); } });
    return nodes.get(key);
  };
  const document = { activeElement: null, querySelector: node, querySelectorAll: () => [], getElementById: node, createElement: node, body: null,
    addEventListener(type, fn) { if (type === 'keydown') keydown.push(fn); } };
  document.body = node('body');
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) {
    calls.push(copy(message)); const handler = handlers.get(message.command);
    Promise.resolve().then(() => handler ? handler(message.payload) : {}).then(result => window.STHostTransport.deliver({ id: message.id, ok: true, result }),
      error => window.STHostTransport.deliver({ id: message.id, ok: false, error: error.message }));
  } }) };
  const context = vm.createContext({ window, document, console, innerWidth: 1080, innerHeight: 700, setTimeout() { return 0; }, clearTimeout() {}, setInterval() { return 0; }, clearInterval() {},
    Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, Error });
  for (const name of ['routing-project.js', 'app.js', 'render-selection.js']) web.load(context, name);
  const ST = window.ST;
  const steps = [
    [null, null, null, null, { note: 36, instrument: 0, velocity: 100 }, null, null, null],
    [null, null, null, null, null, null, { note: 38, instrument: 1, velocity: 90 }, null],
    [{ note: 60, instrument: 2 }, null, null, null, null, null, null, null]];
  ST.state.project = ST.normalize({ name: 'Beat', groups: [{ id: 'kit', name: 'Kit', busID: 'kitbus' }],
    tracks: [{ id: 0, routingID: 'kick', name: 'Kick', instrument: 0, groupID: 'kit' }, { id: 1, routingID: 'snare', name: 'Snare', instrument: 1, groupID: 'kit' },
      { id: 2, routingID: 'bass', name: 'Bass', instrument: 2 }],
    instruments: [{ id: 0, name: 'Kick', type: 'sample' }, { id: 1, name: 'Snare', type: 'sample' }, { id: 2, name: 'Bass', type: 'sample' }],
    patterns: [{ id: 3, name: 'Verse', length: 8, steps }], sequence: [3], activePattern: 3 });
  ST.state.engineReady = true; ST.state.native = true; ST.state.view = 'pattern';
  ST.state.selection = { track: 0, row: 4 }; ST.state.track = 1; ST.state.row = 7;
  handlers.set('engine.project', () => ({}));
  handlers.set('render.selection', payload => ({ path: '/support/Recordings/Kick-Snare 04-07-abcd.wav', name: payload.name, frames: 36000, sampleRate: 48000,
    channels: 2, musicalFrames: 24000, tailFrames: 12000, tailEnd: 'silence', peak: .5 }));
  handlers.set('sample.load', payload => ({ path: payload.path, frames: 36000, sampleRate: 48000 }));
  const fill = values => { for (const [id, value] of Object.entries(values)) { if (typeof value === 'boolean') node(id).checked = value; else node(id).value = String(value); } };
  const toasts = () => node('toast-region').innerHTML;
  return { ST, window, document, node, calls, handlers, fill, toasts, keydown, model: window.STRenderSelection.model };
}

const tests = [], test = (name, fn) => tests.push([name, fn]);

test('model: selection, sounding tracks and default names', () => {
  const h = harness(), p = h.ST.state.project, pat = p.patterns[0];
  const sel = h.model.selection(p, pat, { t0: 0, t1: 1, r0: 4, r1: 7 });
  assert.deepEqual(copy(sel), { pattern: 3, tracks: [0, 1], firstRow: 4, lastRow: 7 });
  assert.equal(h.model.defaultName(p, pat, sel), 'Kit 04–07', 'a whole kit is named after its group');
  assert.equal(h.model.defaultName(p, pat, h.model.selection(p, pat, { t0: 0, t1: 2, r0: 0, r1: 4 })), 'Kick+Bass 00–04', 'tracks that play, then rows');
  assert.equal(h.model.defaultName(p, pat, h.model.selection(p, pat, { t0: 1, t1: 2, r0: 1, r1: 3 })), 'Snare+Bass 01–03', 'no notes: every selected track');
  p.tracks.push({ name: 'Hat' }, { name: 'Ride' });
  pat.steps.push([null, { note: 42 }], [null, { note: 51 }]);
  assert.equal(h.model.defaultName(p, pat, { pattern: 3, tracks: [0, 1, 2, 3, 4], firstRow: 0, lastRow: 7 }), 'Kick+Snare+Bass+2 00–07');
  assert.equal(h.model.selection(p, pat, { t0: 0, t1: 0, r0: 9, r1: 12 }), null, 'rows past the pattern end');
});

test('model: the applied edit (render only, bounce to a new track, to the first track, keep and mute)', () => {
  const h = harness(), helpers = { instrument: (id, name) => ({ id, name, type: 'sample', volume: .7, attack: .005, sustain: .7 }),
    appendTrack: (p, instrument, name) => h.ST.appendInstrumentTrack(p, instrument, name) };
  const sel = { pattern: 3, tracks: [0, 1], firstRow: 4, lastRow: 7 }, result = { name: 'Kit 04–07', path: '/r.wav' };
  let p = copy(h.ST.state.project);
  let applied = h.model.apply(p, sel, result, { replace: false }, helpers);
  assert.deepEqual(copy(applied), { instrument: 3, track: null });
  const inst = p.instruments[3];
  assert.equal(inst.samplePath, '/r.wav'); assert.equal(inst.mode, 'oneshot'); assert.equal(inst.rootNote, 60);
  assert.equal(inst.volume, 1); assert.equal(inst.attack, 0); assert.equal(inst.sustain, 1); assert.equal(inst.plugin, null);
  assert.deepEqual(copy(p.patterns[0].steps), copy(h.ST.state.project.patterns[0].steps), 'a plain render leaves the notes alone');

  p = copy(h.ST.state.project);
  applied = h.model.apply(p, sel, result, { replace: true, destination: 'new', originals: 'clear' }, helpers);
  assert.equal(applied.track, 3); assert.equal(p.tracks[3].name, 'Kit 04–07'); assert.equal(p.tracks[3].volume, 1);
  assert.deepEqual(copy(p.patterns[0].steps[3][4]), { note: 60, instrument: 3, velocity: 127 });
  assert(p.patterns[0].steps[0].slice(4, 8).every(step => step === null) && p.patterns[0].steps[1].slice(4, 8).every(step => step === null), 'selected cells cleared');
  assert.deepEqual(copy(p.patterns[0].steps[2]), copy(h.ST.state.project.patterns[0].steps[2]), 'other tracks untouched');

  p = copy(h.ST.state.project);
  applied = h.model.apply(p, sel, result, { replace: true, destination: 'track', originals: 'clear' }, helpers);
  assert.equal(applied.track, 0); assert.equal(p.tracks.length, 3);
  assert.deepEqual(copy(p.patterns[0].steps[0][4]), { note: 60, instrument: 3, velocity: 127 });
  assert.equal(p.patterns[0].steps[1][6], null);

  p = copy(h.ST.state.project);
  h.model.apply(p, sel, result, { replace: true, destination: 'new', originals: 'mute' }, helpers);
  assert.equal(p.tracks[0].mute, true); assert.equal(p.tracks[1].mute, true); assert(!p.tracks[2].mute);
  assert.deepEqual(copy(p.patterns[0].steps[1][6]), copy(h.ST.state.project.patterns[0].steps[1][6]), 'kept notes');

  // Kept (muted) originals with "first selected track": the trigger still goes on a new, audible track
  // (on the muted first track it would be silent and replace the note it was meant to keep).
  p = copy(h.ST.state.project);
  applied = h.model.apply(p, sel, result, { replace: true, destination: 'track', originals: 'mute' }, helpers);
  assert.equal(applied.track, 3); assert(!p.tracks[3].mute, 'the trigger track is audible');
  assert.deepEqual(copy(p.patterns[0].steps[3][4]), { note: 60, instrument: 3, velocity: 127 });
  assert.deepEqual(copy(p.patterns[0].steps[0]), copy(h.ST.state.project.patterns[0].steps[0]), 'every original note of the first track is kept');
});

test('command: palette entry with ⌥B, handled, in the help sheet and the selection context menu; ⌥B opens it', () => {
  const h = harness();
  const entry = h.ST.commands.find(command => command[0] === 'renderSelection');
  assert(entry && entry[1] === 'Render selection to sample…' && entry[2] === '⌥B');
  assert.equal(h.window.STApp.hasCommand('renderSelection'), true);
  assert(h.ST.commands.filter(command => command[2] === '⌥B').length === 1, '⌥B is used by one command');
  assert(JSON.stringify(h.window.STApp.helpSections()).includes('⌥B'), 'help lists ⌥B');
  assert(/Render selection to sample…'[^\n]*'⌥B'[^\n]*perform\('renderSelection'\)/.test(web.read('context-menu.js')), 'context menu on pattern cells');
  let opened = 0; h.window.STRenderSelection.open = () => { opened++; };
  const event = { altKey: true, metaKey: false, ctrlKey: false, shiftKey: false, code: 'KeyB', target: h.document.body, preventDefault() { this.prevented = true; } };
  h.keydown.forEach(fn => fn(event));
  assert.equal(opened, 1); assert(event.prevented);
  h.ST.state.view = 'mixer'; h.keydown.forEach(fn => fn({ ...event }));
  assert.equal(opened, 1, 'only in the pattern editor');
});

test('render to a new instrument: one Undo step, file path, progress, instrument selected', async () => {
  const h = harness(), before = copy(h.ST.state.project);
  assert.equal(h.ST.perform('renderSelection'), true);
  assert.equal(h.ST.state.modal, 'render-selection');
  assert(h.node('overlay').innerHTML.includes('Kit 04–07'), 'default name in the dialog');
  h.fill({ 'rs-name': 'Kit 04–07', 'rs-mode': 'track', 'rs-tail': '4', 'rs-rate': '44100', 'rs-channels': '1', 'rs-normalize': true, 'rs-replace': false });
  let release; h.handlers.set('render.selection', payload => new Promise(resolve => { release = () => resolve({ path: '/support/Recordings/Kit 04-07-abcd.wav', name: payload.name,
    frames: 33075, sampleRate: 44100, channels: 1, musicalFrames: 24000, tailEnd: 'silence', peak: .97 }); }));
  const running = h.node('rs-go').onclick(); await tick();
  const request = h.calls.find(call => call.command === 'render.selection').payload;
  assert.deepEqual(request, { pattern: 3, tracks: [0, 1], firstRow: 4, lastRow: 7, mode: 'track', tail: 4, sampleRate: 44100, channels: 1, normalize: true, name: 'Kit 04–07' });
  assert.equal(h.window.STRenderSelection.isBusy, true); assert.equal(h.node('rs-go').disabled, true);
  h.window.nativeEvent({ type: 'render.selectionProgress', payload: { phase: 'rendering', progress: .5 } });
  assert.equal(h.node('rs-meter').value, .5); assert.equal(h.node('rs-status').textContent, 'Rendering…');
  release(); await running; await tick();
  const p = h.ST.state.project;
  assert.equal(p.instruments.length, 4); assert.equal(p.instruments[3].samplePath, '/support/Recordings/Kit 04-07-abcd.wav');
  assert.equal(p.instruments[3].name, 'Kit 04–07');
  assert.equal(h.calls.find(call => call.command === 'sample.load').payload.instrument, 3);
  assert.equal(h.ST.state.undo.length, 1); assert.equal(h.ST.state.undo[0].label, 'render selection to sample');
  assert.equal(h.ST.state.instrument, 3); assert.equal(h.ST.state.modal, null);
  assert.deepEqual(copy(p.patterns), before.patterns);
  h.ST.perform('undo');
  assert.deepEqual(copy(h.ST.state.project), before, 'Undo removes the instrument');
});

test('bounce in place is one Undo step and Redo restores it', async () => {
  const h = harness(), before = copy(h.ST.state.project);
  h.ST.perform('renderSelection');
  h.fill({ 'rs-name': 'Kit 04–07', 'rs-mode': 'track', 'rs-tail': '10', 'rs-rate': '48000', 'rs-channels': '2', 'rs-normalize': false,
    'rs-replace': true, 'rs-destination': 'new', 'rs-originals': 'clear' });
  await h.node('rs-go').onclick(); await tick();
  const p = h.ST.state.project, after = copy(p);
  assert.equal(p.tracks.length, 4); assert.equal(p.tracks[3].instrument, 3);
  assert.deepEqual(copy(p.patterns[0].steps[3][4]), { note: 60, instrument: 3, velocity: 127 });
  assert.equal(p.patterns[0].steps[0][4], null); assert.equal(p.patterns[0].steps[1][6], null);
  assert.equal(h.ST.state.undo.length, 1); assert.equal(h.ST.state.undo[0].label, 'bounce selection in place');
  assert.equal(h.ST.state.track, 3); assert.equal(h.ST.state.row, 4);
  h.ST.perform('undo'); assert.deepEqual(copy(h.ST.state.project), before);
  h.ST.perform('redo'); assert.deepEqual(copy(h.ST.state.project), after);
});

test('cancel and native refusals leave the project and history untouched', async () => {
  const h = harness(), before = copy(h.ST.state.project);
  h.ST.perform('renderSelection');
  h.fill({ 'rs-name': 'Kit', 'rs-tail': '10' });
  let reject; h.handlers.set('render.selection', () => new Promise((_, no) => { reject = no; }));
  h.handlers.set('render.cancelSelection', () => { reject(new Error('Render cancelled.')); return { cancelled: true }; });
  const running = h.node('rs-go').onclick(); await tick();
  assert.equal(h.node('rs-cancel').textContent, 'Cancel render');
  h.node('rs-cancel').onclick(); await running; await tick();
  assert(h.calls.some(call => call.command === 'render.cancelSelection'));
  assert.deepEqual(copy(h.ST.state.project), before); assert.equal(h.ST.state.undo.length, 0);
  assert.equal(h.window.STRenderSelection.isBusy, false); assert.equal(h.node('rs-status').textContent, 'Render cancelled.');

  h.handlers.set('render.selection', () => { throw new Error('Kick is frozen. Unfreeze it to render its notes.'); });
  await h.node('rs-go').onclick(); await tick();
  assert.equal(h.node('rs-status').textContent, 'Kick is frozen. Unfreeze it to render its notes.');
  h.fill({ 'rs-tail': '61' }); await h.node('rs-go').onclick();
  assert.match(h.node('rs-status').textContent, /between 0 and 60/);
  assert.deepEqual(copy(h.ST.state.project), before); assert.equal(h.ST.state.undo.length, 0);
});

test('a bounce in place whose tracks moved while it rendered is not applied to the tracks that took their place', async () => {
  const h = harness();
  h.ST.perform('renderSelection');
  h.fill({ 'rs-name': 'Kit 04–07', 'rs-mode': 'track', 'rs-tail': '10', 'rs-rate': '48000', 'rs-channels': '2', 'rs-normalize': false,
    'rs-replace': true, 'rs-destination': 'new', 'rs-originals': 'clear' });
  let release; h.handlers.set('render.selection', payload => new Promise(resolve => { release = () => resolve({ path: '/support/Recordings/Kit.wav', name: payload.name,
    frames: 48000, sampleRate: 48000, channels: 2, tailEnd: 'silence', peak: .5 }); }));
  const running = h.node('rs-go').onclick(); await tick();
  // Playback and control surfaces keep running during a render: the Kick track is deleted (an edit made in place).
  const p = h.ST.state.project;
  p.tracks.splice(0, 1); for (const pattern of p.patterns) pattern.steps.splice(0, 1);
  const during = copy(p);
  release(); await running; await tick();
  assert.deepEqual(copy(h.ST.state.project), during, 'the Snare and Bass cells that moved into the rendered columns are not cleared');
  assert.equal(h.ST.state.undo.length, 0); assert.equal(h.window.STRenderSelection.isBusy, false);
  assert.match(h.node('rs-status').textContent, /changed while rendering.*kept at \/support\/Recordings\/Kit\.wav/);
});

test('a selection without notes explains instead of opening', () => {
  const h = harness();
  h.ST.state.selection = { track: 1, row: 0 }; h.ST.state.track = 2; h.ST.state.row = 3;
  h.ST.state.project.patterns[0].steps[2][0] = null;
  assert.equal(h.ST.perform('renderSelection'), false);
  assert.equal(h.ST.state.modal, null);
  assert.match(h.toasts(), /no notes to render/);
});

(async () => {
  for (const [name, fn] of tests) {
    try { await fn(); console.log('PASS', name); }
    catch (error) { console.error('FAIL', name); console.error(error); process.exitCode = 1; }
  }
})();
