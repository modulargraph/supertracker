'use strict';
// Exercise the actual app's keyboard, note entry and Undo handlers with a mocked bridge.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
// A recorded key becomes an edit when the host has acknowledged it.
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
function harness() {
  const nodes = new Map(), calls = [];
  let context;
  function element(id) {
    if (!nodes.has(id)) nodes.set(id, {
      id, dataset: {}, innerHTML: '', textContent: '', value: '', style: {}, classList: { toggle() {}, add() {}, remove() {} },
      addEventListener() {}, setAttribute() {}, scrollIntoView() {}, select() {}, querySelectorAll() { return []; },
      focus() { document.activeElement = this; },
      closest(selector) { return selector === '.tracker' && this.id === 'tracker' ? this : null; }
    });
    return nodes.get(id);
  }
  const document = {
    activeElement: null, querySelector: selector => element(selector.startsWith('#') ? selector.slice(1) : selector),
    querySelectorAll: () => [], getElementById: element, createElement: element, addEventListener() {}
  };
  document.body = element('body');
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) {
    calls.push(copy(message)); window.STHostTransport.deliver({ id: message.id, ok: true, result: recording(message) ?? {} });
  } }) };
  const recording = noteRecordingHost(window);
  context = vm.createContext({ window, document, console, setTimeout: () => 1, clearTimeout() {}, innerWidth: 1280, innerHeight: 800 });
  web.load(context, 'app.js');
  const ST = window.ST, state = ST.state;
  state.project = ST.demoProject();
  Object.assign(state, { view: 'pattern', track: 0, instrument: 0, row: 10, column: 0, playing: false, recording: true, keyboardPlay: true, octave: 4, editStep: 2, chord: 'single', scale: 'chromatic' });
  const prior = { note: 48, instrument: 3, velocity: 47, gate: 3, chord: [0, 3, 7], fx1: { type: 'plugin', instanceID: 'saved-target', address: '18446744073709551615', value: .37 }, fx2: { type: 'reverse', value: 1 } };
  ST.pattern().steps[0][10] = copy(prior); element('tracker').focus();
  function key(key, code, modifiers = {}) {
    const event = { key, code, target: element('tracker'), metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, preventDefault() { this.defaultPrevented = true; }, ...modifiers };
    ST.keyboard(event); return event;
  }
  return { ST, state, prior, window, document, calls, key };
}
const tests = [], test = (name, run) => tests.push({ name, run });
test('stopped single-note keyboard entry clears the prior chord and preserves step FX in one edit', async () => {
  const h = harness(), before = copy(h.state.project), event = h.key('z', 'KeyZ'); await tick(); const step = h.ST.pattern().steps[0][10];
  assert(event.defaultPrevented); assert.equal(step.note, 60); assert.equal(step.instrument, 0); assert.equal(step.velocity, 100); assert.equal(step.gate, 0);
  assert.equal(step.chord, undefined); assert.equal(Object.hasOwn(copy(step), 'chord'), false);
  assert.deepEqual(copy(step.fx1), h.prior.fx1); assert.deepEqual(copy(step.fx2), h.prior.fx2);
  assert.equal(h.state.undo.length, 1); assert.equal(h.state.undo[0].label, 'record note'); assert.deepEqual(copy(h.state.undo[0].project), before);
  assert.equal(h.state.row, 12); assert.equal(h.document.activeElement.id, 'tracker');
  assert.deepEqual(h.calls.filter(call => call.command === 'note.on').map(call => call.payload.note), [60]);
  h.ST.noteOff('KeyZ'); await tick(); assert.equal(h.state.undo.length, 1); assert.equal(h.state.row, 12);
});
test('stopped chord entry owns its interval array independently of the mode definition and other recorded rows', async () => {
  const h = harness(); h.state.chord = 'major';
  h.key('z', 'KeyZ'); h.ST.noteOff('KeyZ'); await tick();
  const first = h.ST.pattern().steps[0][10], definition = h.window.__supertracker.chords.major.notes;
  assert.deepEqual(copy(first.chord), [0, 4, 7]); assert.notEqual(first.chord, definition);
  assert.deepEqual(copy(first.fx1), h.prior.fx1); assert.deepEqual(copy(first.fx2), h.prior.fx2);
  assert.equal(h.state.undo.length, 1); assert.equal(h.state.row, 12);
  h.key('x', 'KeyX'); h.ST.noteOff('KeyX'); await tick();
  const second = h.ST.pattern().steps[0][12]; assert.equal(second.note, 62); assert.deepEqual(copy(second.chord), [0, 4, 7]);
  assert.notEqual(second.chord, definition); assert.notEqual(second.chord, first.chord);
  first.chord[1] = 9; assert.deepEqual(copy(definition), [0, 4, 7]); assert.deepEqual(copy(second.chord), [0, 4, 7]);
  assert.equal(h.state.undo.length, 2); assert.equal(h.state.row, 14);
});
test('actual keyboard Undo restores the overwritten chord and Redo restores the new single note', async () => {
  const h = harness(), before = copy(h.state.project);
  h.key('z', 'KeyZ'); h.ST.noteOff('KeyZ'); await tick(); const recorded = copy(h.ST.pattern().steps[0][10]);
  const undo = h.key('z', 'KeyZ', { metaKey: true }); assert(undo.defaultPrevented);
  assert.deepEqual(copy(h.ST.pattern().steps[0][10]), h.prior); assert.deepEqual(copy(h.state.project), before);
  assert.equal(h.state.undo.length, 0); assert.equal(h.state.redo.length, 1);
  const redo = h.key('z', 'KeyZ', { metaKey: true, shiftKey: true }); assert(redo.defaultPrevented);
  assert.deepEqual(copy(h.ST.pattern().steps[0][10]), recorded); assert.equal(h.ST.pattern().steps[0][10].chord, undefined);
  assert.equal(h.state.undo.length, 1); assert.equal(h.state.redo.length, 0);
  assert.equal(h.calls.filter(call => call.command === 'note.on').length, 1);
});
(async () => {
  for (const { name, run } of tests) { await run(); console.log(`PASS ${name}`); }
  console.log(`Stopped note recording: ${tests.length} regression groups passed.`);
})().catch(error => { console.error(error); process.exit(1); });
