'use strict';
// A frozen track refuses edits of what is printed into it. The refusal comes from the app's own mutate
// (app-history.js asks freeze.js checkMutation), for app code and for modules (window.ST.mutate) alike, and it
// must leave everything as it was: the project, the Undo and Redo history and the saved state. freeze.js once
// wrapped window.ST.mutate with a second guard that let the edit through as a no-op "success": it cleared Redo,
// marked the session edited and reported true to the caller.
const assert = require('node:assert/strict'), vm = require('node:vm');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const copy = value => JSON.parse(JSON.stringify(value));

function harness() {
  const listeners = {}, toasts = [], calls = [];
  const listen = (type, fn) => { (listeners[type] ||= []).push(fn); };
  const project = complete({
    name: 'Song', bpm: 120, linesPerBeat: 4, sequence: [0],
    tracks: [{ name: 'Bass', routingID: 'bass', instrument: 0, volume: .8, frozen: { id: 'print-1' } }, { name: 'Keys', routingID: 'keys', instrument: 1, volume: .8 }],
    instruments: [{ name: 'Bass' }, { name: 'Keys' }],
    patterns: [{ id: 0, length: 4, steps: [[{ note: 38, instrument: 0 }, null, null, null], [null, null, null, null]], automation: [] }]
  });
  const state = { project, undo: [], redo: [{ label: 'earlier edit', project: copy(project) }], dirty: false, track: 0, row: 0, instrument: 0, sequence: 0, selection: null, modal: null };
  const window = { addEventListener: listen };
  const document = { addEventListener: listen, getElementById: () => null, querySelectorAll: () => [] };
  const context = vm.createContext({ window, document, console, setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {} });
  web.loadAppWith(context, { state, clone: copy, clamp: (x, a, b) => Math.max(a, Math.min(b, x)), pattern: () => state.project.patterns[0], toast: () => {},
    refresh: () => calls.push('refresh'), updateTransport: () => calls.push('transport'), syncEngine: () => calls.push('sync'), scheduleAutosave: () => calls.push('autosave') });
  web.load(context, 'freeze.js');
  // The refusal is explained in a toast here (no dialog markup in this harness).
  window.ST.openModal = null; window.ST.toast = message => toasts.push(message);
  for (const type of ['DOMContentLoaded', 'load']) for (const fn of listeners[type] || []) fn({});
  return { window, state, toasts, calls, ST: window.ST };
}

const tests = [], test = (name, fn) => tests.push({ name, fn });

test('a module edit of a frozen track is refused without touching Redo, the saved state or the project', () => {
  const h = harness(), before = copy(h.state.project);
  const result = h.ST.mutate('set step effect', p => { p.patterns[0].steps[0][1] = { note: 40, instrument: 0 }; });
  assert.equal(result, false, 'the caller is told the edit was refused');
  assert.deepEqual(copy(h.state.project), before, 'the frozen notes are unchanged');
  assert.equal(h.state.undo.length, 0, 'no Undo step');
  assert.equal(h.state.redo.length, 1, 'Redo survives a refused edit');
  assert.equal(h.state.dirty, false, 'a refused edit does not mark the session edited');
  assert(!h.calls.includes('sync') && !h.calls.includes('autosave'), 'nothing is sent to the engine or autosaved');
  assert.equal(h.toasts.length, 1); assert.match(h.toasts[0], /Bass is frozen/);
});

test('the song arrangement printed into a frozen track is refused the same way', () => {
  const h = harness();
  assert.equal(h.ST.mutate('insert sequence block', p => { p.sequence.push(0); }), false);
  assert.deepEqual(copy(h.state.project.sequence), [0]); assert.equal(h.state.redo.length, 1); assert.equal(h.state.dirty, false);
  assert.match(h.toasts[0], /Tempo and the song arrangement are printed/);
});

test('mix edits of a frozen track and edits of other tracks stay ordinary Undo steps', () => {
  const h = harness();
  assert.equal(h.ST.mutate('Adjust volume', p => { p.tracks[0].volume = .5; }), true);
  assert.equal(h.ST.mutate('enter note', p => { p.patterns[0].steps[1][0] = { note: 60, instrument: 1 }; }), true);
  assert.equal(h.state.undo.length, 2); assert.equal(h.state.redo.length, 0); assert.equal(h.state.dirty, true);
  assert.equal(h.state.project.tracks[0].volume, .5); assert.equal(h.toasts.length, 0);
});

let failed = 0;
for (const { name, fn } of tests) {
  try { fn(); console.log('PASS ' + name); }
  catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); }
}
console.log(`${tests.length - failed}/${tests.length} frozen-track refused edit groups passed`);
process.exitCode = failed ? 1 : 0;
