'use strict';
// Note lifetimes like a classic tracker: gate is optional (new notes have none), the Gate column is a hidden-by-default
// per-project view preference (saved, never an Undo step), the step editor shows "—" for no gate, OFF/CUT are one click
// or palette command away, and "Remove gates" clears gates in the selection.
const assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: {}, classList: { toggle() {}, remove() {}, add() {} },
      focus() {}, select() {}, addEventListener() {}, setAttribute() {}, removeAttribute() {}, scrollIntoView() {}, querySelector: () => null, querySelectorAll: () => [] });
    return elements.get(id);
  };
  const document = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: () => [], getElementById: element,
    addEventListener() {}, createElement: tag => element(tag) };
  const calls = [];
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) { calls.push(message); } }) };
  const { ST, app } = web.app({ document, window, requestAnimationFrame: fn => fn(), setTimeout: () => 1, clearTimeout() {}, innerWidth: 1280, innerHeight: 800 });
  const state = ST.state;
  state.project = ST.demoProject();
  state.view = 'pattern';
  state.track = 0; state.row = 0; state.selection = null; state.editStep = 1;
  return { ST, app, state, calls, html: () => app.renderPattern() };
}
const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test('the Gate column is hidden by default; a gateless note reads "—", a step without a gate key 0.8', () => {
  const h = harness(), pat = h.ST.pattern();
  assert.equal(h.ST.gateColumnShown(), false);
  pat.steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  pat.steps[0][1] = { note: 62, instrument: 0, velocity: 100 };
  pat.steps[0][2] = { note: 64, instrument: 0, velocity: 100, gate: 2.5 };
  assert.equal(h.ST.stepGate(pat.steps[0][0]), null); assert.equal(h.ST.gateText(pat.steps[0][0]), '—');
  assert.equal(h.ST.stepGate(pat.steps[0][1]), .8); assert.equal(h.ST.stepGate(pat.steps[0][2]), 2.5);
  assert.equal(h.ST.stepGate({ note: -1 }), null, 'OFF has no gate');
  const html = h.html();
  assert.match(html, /data-gate-column="hidden"/);
  assert.doesNotMatch(html, />GATE</); assert.doesNotMatch(html, /class="pattern-gate/);
  assert.match(html, /<output id="step-gate-readout">—<\/output>/, 'The step editor shows no gate as —');
  assert.doesNotMatch(html, /id="step-gate"/, 'Gate is read-only while the column is hidden');
  assert.match(html, /id="step-off"[^>]*>OFF</); assert.match(html, /id="step-cut"[^>]*>CUT</);
  assert.match(html, /1 OFF · ⇧1 CUT/, 'The pattern help line names the note-off keys');
  assert.match(html, /id="gate-column"[^>]*aria-pressed="false"/);
});

test('new notes get no gate', () => {
  const h = harness(), pat = h.ST.pattern();
  pat.steps[0][5] = null; h.state.row = 5;
  assert.equal(h.app.ensureStep().gate, 0);
  assert.equal(h.ST.stepGate(pat.steps[0][5]), null);
});

test('showing the Gate column is a saved view preference, not an Undo step, and Undo never flips it', () => {
  const h = harness(), pat = h.ST.pattern();
  pat.steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  h.ST.mutate('edit', () => { h.ST.pattern().steps[0][3] = { note: 67, instrument: 0, velocity: 100, gate: 0 }; });
  const undo = h.state.undo.length; h.state.dirty = false;
  assert.equal(h.ST.perform('toggleGateColumn'), true);
  assert.equal(h.ST.gateColumnShown(), true); assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.viewState)), { gateColumn: true });
  assert.equal(h.state.undo.length, undo, 'No Undo step'); assert.equal(h.state.dirty, true, 'Saved with the session');
  const html = h.html();
  assert.match(html, /data-gate-column="shown"/); assert.match(html, />GATE</); assert.match(html, /class="pattern-gate /);
  assert.match(html, /id="step-gate"[^>]*placeholder="—" value=""/, 'Editable, empty for no gate');
  assert.match(html, /id="gate-column"[^>]*aria-pressed="true"/);
  h.ST.perform('undo');
  assert.equal(h.ST.gateColumnShown(), true, 'Undo keeps the view');
  h.ST.perform('toggleGateColumn');
  assert.equal(h.ST.gateColumnShown(), false); assert.equal(JSON.stringify(h.state.project.viewState), '{}');
});

test('OFF and CUT: palette commands with their shortcuts, keeping step effects and advancing the cursor', () => {
  const h = harness(), pat = h.ST.pattern();
  const off = h.ST.commands.find(c => c[0] === 'noteOff'), cut = h.ST.commands.find(c => c[0] === 'noteCut');
  assert.equal(off[2], '1'); assert.equal(cut[2], '⇧1'); assert.match(off[1], /Insert note off/); assert.match(cut[1], /Insert note cut/);
  assert(h.ST.commands.some(c => c[0] === 'toggleGateColumn')); assert(h.ST.commands.some(c => c[0] === 'removeGates'));
  pat.steps[0][4] = { note: 60, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'pan', value: .5 } };
  h.state.row = 4; const undo = h.state.undo.length;
  h.ST.perform('noteOff');
  assert.deepEqual(JSON.parse(JSON.stringify(pat.steps[0][4])), { fx1: { type: 'pan', value: .5 }, note: -1, instrument: h.state.instrument, velocity: 100 });
  assert.equal(h.state.row, 5); assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.undo.at(-1).label, 'note off');
  h.ST.perform('noteCut');
  assert.equal(pat.steps[0][5].note, -2); assert.equal(h.state.row, 6); assert.equal(h.state.undo.at(-1).label, 'note cut');
});

test('Remove gates clears gates in the selection as one Undo step', () => {
  const h = harness(), pat = h.ST.pattern();
  for (let t = 0; t < 2; t++) for (let r = 0; r < 4; r++) pat.steps[t][r] = { note: 60 + r, instrument: t, velocity: 100, gate: r + 1 };
  pat.steps[0][1] = { note: 61, instrument: 0, velocity: 100 };   // no gate key is 0.8
  pat.steps[2][0] = { note: 50, instrument: 2, velocity: 100, gate: 3 };
  h.state.selection = { track: 0, row: 0 }; h.state.track = 1; h.state.row = 3;
  const undo = h.state.undo.length;
  h.ST.perform('removeGates');
  assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.undo.at(-1).label, 'remove gates');
  for (let t = 0; t < 2; t++) for (let r = 0; r < 4; r++) assert.equal(h.ST.stepGate(pat.steps[t][r]), null, `${t}:${r}`);
  assert.equal(pat.steps[2][0].gate, 3, 'Outside the selection');
  h.ST.perform('removeGates');
  assert.equal(h.state.undo.length, undo + 1, 'Nothing left to remove adds no Undo step');
});

(async () => {
  let failed = 0;
  for (const [name, fn] of tests) {
    try { await fn(); console.log('PASS', name); } catch (error) { failed++; console.error('FAIL', name); console.error(error); }
  }
  if (failed) process.exit(1);
})();
