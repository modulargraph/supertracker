'use strict';
// Typed values, tracker style: in the pattern grid only the NOTE column plays notes; INS / VEL take digits and the FX
// columns take an effect code + value (R04, Y50, P-30 …), each entry one Undo step. Focused step-editor boxes keep
// their keys (never the piano), and their FX / Ins boxes take typed codes, names and numbers.
const assert = require('node:assert/strict');
const web = require('./helpers/web');

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { id, tagName: 'DIV', innerHTML: '', value: '', textContent: '', dataset: {}, style: {},
      classList: { toggle() {}, remove() {}, add() {} }, focus() {}, blur() {}, select() {}, addEventListener() {}, setAttribute() {},
      removeAttribute() {}, scrollIntoView() {}, querySelector: () => null, querySelectorAll: () => [], closest: () => null });
    return elements.get(id);
  };
  const document = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: () => [], getElementById: element,
    addEventListener() {}, createElement: tag => element(tag) };
  const calls = [], toasts = [];
  const window = { addEventListener() {}, webkit: { messageHandlers: { supertracker: { postMessage(message) { calls.push(message); } } } } };
  const { ST, app } = web.app({ document, window, requestAnimationFrame: fn => fn(), setTimeout: () => 1, clearTimeout() {}, innerWidth: 1280, innerHeight: 800 });
  const state = ST.state;
  state.project = ST.demoProject();
  Object.assign(state, { view: 'pattern', track: 0, row: 0, column: 0, selection: null, editStep: 1, keyboardPlay: true, recording: false, engineReady: true });
  const toast = app.toast;
  app.toast = (message, kind) => { toasts.push(String(message)); return toast?.(message, kind); };
  const grid = { id: 'tracker', tagName: 'TABLE', dataset: {}, closest: selector => selector === '.tracker' ? grid : null };
  function key(k, extra = {}) {
    const event = { key: k, code: extra.code || (k.length === 1 && /[a-z]/i.test(k) ? 'Key' + k.toUpperCase() : /^[0-9]$/.test(k) ? 'Digit' + k : k),
      target: extra.target || grid, shiftKey: !!extra.shiftKey, altKey: false, metaKey: false, ctrlKey: false, repeat: false,
      defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {} };
    ST.keyboard(event);
    return event;
  }
  const notesPlayed = () => calls.filter(c => c.command === 'note.on').length;
  const type = text => { for (const ch of text) key(ch); };
  return { ST, app, state, calls, toasts, key, type, notesPlayed, element, step: (t = 0, r = 0) => ST.pattern().steps[t][r] };
}
const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test('grid VEL column: digits set the velocity, one Undo step, no note played (Keys on)', () => {
  const h = harness();
  h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  h.state.column = 2; const undo = h.state.undo.length;
  h.type('5');
  assert.equal(h.notesPlayed(), 0, '5 must not play a note');
  assert.equal(h.step().velocity, 100, 'pending until complete');
  h.type('0'); h.key('Enter');
  assert.equal(h.step().velocity, 50); assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.undo.at(-1).label, 'type velocity');
  assert.equal(h.state.row, 1, 'the cursor advances by the edit step');
  h.ST.pattern().steps[0][1] = { note: 62, instrument: 0, velocity: 100, gate: 0 };
  h.type('127');
  assert.equal(h.step(0, 1).velocity, 127, 'three digits commit by themselves'); assert.equal(h.state.row, 2);
  h.ST.pattern().steps[0][2] = { note: 62, instrument: 0, velocity: 100, gate: 0 };
  h.type('9'); h.type('0');
  assert.equal(h.step(0, 2).velocity, 90, '90: a third digit could not stay in 1–127');
  assert.equal(h.notesPlayed(), 0);
  // Letters (piano keys) in VEL never play either.
  h.key('z'); h.key('q'); assert.equal(h.notesPlayed(), 0);
});

test('grid INS column: two digits pick the instrument; a missing instrument is rejected with a hint', () => {
  const h = harness(), count = h.state.project.instruments.length;
  h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  h.state.column = 1;
  h.type('03'); assert.equal(h.step().instrument, 3); assert.equal(h.notesPlayed(), 0);
  h.state.row = 0; const undo = h.state.undo.length;
  assert(count < 10, 'demo fixture: fewer than ten instruments, so one digit beyond the last is complete and invalid');
  h.type('9');
  assert.equal(h.step().instrument, 3); assert.equal(h.state.undo.length, undo);
  assert.match(h.toasts.at(-1), /does not exist/);
});

test('grid FX columns: code + value enters effects in their units; invalid codes get a hint; Delete clears; no notes', () => {
  const h = harness();
  h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  for (let r = 1; r < 8; r++) h.ST.pattern().steps[0][r] = null;
  h.state.column = 3; const undo = h.state.undo.length;
  h.type('r4');
  assert.deepEqual(JSON.parse(JSON.stringify(h.step().fx1)), { type: 'ratchet', value: 4 }); assert.equal(h.state.undo.length, undo + 1);
  assert.equal(h.app.fxCodeText(h.step().fx1), 'R04');
  h.state.row = 0; h.state.column = 4;
  h.type('y50');
  assert.deepEqual(JSON.parse(JSON.stringify(h.step().fx2)), { type: 'probability', value: .5 });
  h.state.row = 1; h.state.column = 3;
  h.type('p-30');
  assert.deepEqual(JSON.parse(JSON.stringify(h.step(0, 1).fx1)), { type: 'pan', value: -.3 }, 'Pan in percent, signed'); assert.equal(h.step(0, 1).note, -3);
  assert.equal(h.app.fxCodeText(h.step(0, 1).fx1), 'P-30');
  h.state.row = 2; h.type('c2400');
  assert.equal(h.step(0, 2).fx1.value, 2400);
  h.state.row = 3; h.type('a37'); assert.deepEqual(JSON.parse(JSON.stringify(h.step(0, 3).fx1)), { type: 'arpeggio', value: 0x37 });
  h.state.row = 4; h.type('b1'); assert.equal(h.step(0, 4).fx1.type, 'reverse');
  h.state.row = 5; h.type('t-12'); assert.deepEqual(JSON.parse(JSON.stringify(h.step(0, 5).fx1)), { type: 'pitch', value: -12 });
  h.state.row = 6; const before = h.state.undo.length;
  h.type('j'); assert.match(h.toasts.at(-1), /No effect code “J”/); assert.equal(h.state.undo.length, before);
  h.type('r9'); assert.equal(h.step(0, 6), null, 'Ratchet 9 is out of range'); assert.match(h.toasts.at(-1), /Ratchet.*1–8/);
  h.state.row = 7; h.state.column = 3; h.type('r'); assert.equal(h.app.gridEntry?.code, 'R', JSON.stringify([h.state.row, h.state.column, h.toasts.at(-1)])); h.key('Escape');
  assert.equal(h.app.gridEntry, null, 'Escape abandons a half-typed effect'); assert.equal(h.step(0, 7), null);
  h.state.row = 0; h.state.column = 3; h.key('Delete');
  assert.equal(h.step().fx1, undefined); assert.equal(h.step().fx2.type, 'probability'); assert.equal(h.step().note, 60, 'Delete clears just the effect');
  assert.equal(h.notesPlayed(), 0, 'no letter or digit in an FX column plays a note');
});

test('the NOTE column still plays notes with the computer keyboard', () => {
  const h = harness();
  h.state.column = 0;
  h.key('z');
  assert.equal(h.notesPlayed(), 1);
});

test('a focused step-editor box keeps digits and letters: no note, no grid entry', () => {
  const h = harness();
  h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  const velocity = { id: 'step-velocity', tagName: 'INPUT', type: 'number', dataset: {}, closest: selector => selector.startsWith('input') ? velocity : null };
  h.state.column = 2;
  const event = h.key('5', { target: velocity });
  assert.equal(event.defaultPrevented, false, 'the browser types 5 into the box');
  assert.equal(h.notesPlayed(), 0); assert.equal(h.app.gridEntry, null); assert.equal(h.step().velocity, 100);
  h.key('z', { target: velocity }); assert.equal(h.notesPlayed(), 0);
});

test('step editor parsing: FX codes and names, instrument numbers and name prefixes', () => {
  const h = harness(), { parseFX, parseInstrument } = h.app;
  assert.deepEqual(JSON.parse(JSON.stringify(parseFX('rat 4').fx)), { type: 'ratchet', value: 4 });
  assert.deepEqual(JSON.parse(JSON.stringify(parseFX('R04').fx)), { type: 'ratchet', value: 4 });
  assert.deepEqual(JSON.parse(JSON.stringify(parseFX('rev').fx)), { type: 'reverse', value: 1 });
  assert.deepEqual(JSON.parse(JSON.stringify(parseFX('prob 25').fx)), { type: 'probability', value: .25 });
  assert.deepEqual(JSON.parse(JSON.stringify(parseFX('Q50').fx)), { type: 'microtiming', value: .5 });
  assert.match(parseFX('p').error || '', /^$/, 'P alone is pan with its default');
  assert.match(parseFX('re 3').error, /matches .*Reverse.*Reverb|matches .*Reverb.*Reverse|Resonance/);
  assert.match(parseFX('zz 4').error, /Unknown effect/);
  assert.match(parseFX('X50').error, /plug-in parameter lock/);
  assert.equal(parseFX('').clear, true);
  const names = h.state.project.instruments.map(i => i.name);
  assert.equal(parseInstrument('02').index, 2);
  assert.equal(parseInstrument(names[3].slice(0, 4).toLowerCase()).index, names.findIndex(n => n.toLowerCase().startsWith(names[3].slice(0, 4).toLowerCase())));
  assert.match(parseInstrument('99').error, /does not exist/);
  assert.match(parseInstrument('zzzz').error, /No instrument name/);
  const codes = h.app.FX_CODES.map(c => c.code);
  assert.equal(new Set(codes).size, codes.length, 'codes are unique');
  for (const type of Object.keys(h.ST.FX)) assert(h.app.fxCode(type), `${type} has a code`);
  assert(h.ST.commands.some(c => c[0] === 'effectCodes'));
});

test('step editor boxes: typed FX code and typed instrument commit one Undo step each; invalid input is refused with a hint', () => {
  const h = harness();
  h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  const box = id => h.app.$(id);
  h.app.bindStepEditor();
  const fx = box('#step-fx1-code'), ins = box('#step-instrument'); ins.tagName = 'INPUT';
  h.app.bindStepEditor();
  const undo = h.state.undo.length;
  fx.value = 'rat 3'; fx.onchange();
  assert.deepEqual(JSON.parse(JSON.stringify(h.step().fx1)), { type: 'ratchet', value: 3 }); assert.equal(h.state.undo.length, undo + 1);
  fx.value = 'nonsense 4'; fx.onchange(); assert.match(h.toasts.at(-1), /Unknown effect/); assert.equal(h.state.undo.length, undo + 1);
  assert.equal(fx.value, 'R03', 'the box shows the code again');
  ins.value = '04'; ins.onchange(); assert.equal(h.step().instrument, 4); assert.equal(h.state.undo.length, undo + 2);
  ins.value = '42'; ins.onchange(); assert.match(h.toasts.at(-1), /does not exist/); assert.equal(h.step().instrument, 4);
  fx.value = ''; fx.onchange(); assert.equal(h.step().fx1, undefined, 'an empty FX box removes the effect');
  assert.equal(h.notesPlayed(), 0);
});

(async () => {
  let failed = 0;
  for (const [name, fn] of tests) {
    try { await fn(); console.log('PASS', name); } catch (error) { failed++; console.error('FAIL', name); console.error(error); }
  }
  if (failed) process.exit(1);
})();
