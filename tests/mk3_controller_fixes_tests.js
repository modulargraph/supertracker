'use strict';
// Regression checks from the MK3 controller bug scan: real app + MK3 modules, synthetic 'mk3.input' events.
// Each group states the fault it guards against.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const { metadata, parameterList } = require('./helpers/builtin-effects');
const { session, groupSession } = require('./helpers/mk3-session');
const clone = value => JSON.parse(JSON.stringify(value));

function start(h = session()) {
  h.MK3.subscribe(() => {});
  h.settle = async () => { for (let i = 0; i < 12; i++) { for (let j = 0; j < 60; j++) await Promise.resolve(); h.advance(200); } };
  h.hold = name => h.input({ type: 'button', name, pressed: true });
  h.release = name => h.input({ type: 'button', name, pressed: false });
  h.pad = n => { h.down(n); h.up(n); };
  h.turn = (index, delta) => h.input({ type: 'encoder', index, delta });
  // One hardware encoder count is 0.001 (MK3Protocol.knobCountsPerUnit = 1000); a slow turn sends them one by one.
  h.counts = (index, count) => { for (let i = 0; i < Math.abs(count); i++) { h.turn(index, Math.sign(count) * .001); h.advance(5); } };
  return h;
}
const shorten = (h, index, length) => h.ST.mutate('shorten', p => { const pat = p.patterns[index]; pat.length = length; pat.steps.forEach(steps => { steps.length = length; }); });

const cases = [], test = (name, fn) => cases.push([name, fn]);

test('SELECT + pad in PATTERN mode selects the pattern like a plain press: cursor row in range, engine told', async () => {
  const h = start(); shorten(h, 1, 16); await h.settle();
  h.state.row = 40; h.state.selection = { track: 0, row: 60 }; h.calls.length = 0;
  h.press('pattern'); h.hold('select'); h.pad(2); h.release('select'); await h.settle();
  assert.equal(h.state.project.activePattern, h.state.project.patterns[1].id);
  assert.equal(h.ST.pattern().length, 16);
  assert(h.state.row < 16, `the cursor row ${h.state.row} stays inside the 16-row pattern`);
  assert.equal(h.state.selection, null, 'the selection of the other pattern is dropped');
  assert(h.calls.some(call => call.command === 'engine.project'), 'the engine learns the active pattern');
});

test('DUPLICATE + step pads never write past the end of the pattern', async () => {
  const h = start(); shorten(h, 0, 8); await h.settle();
  h.ST.mutate('note', p => { p.patterns[0].steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0 }; });
  const undo = h.state.undo.length;
  h.press('step'); h.hold('duplicate'); h.pad(1); h.pad(13); h.release('duplicate'); await h.settle();
  assert.equal(h.ST.pattern().steps[0].length, 8, 'pad 13 is past row 7: nothing is written there');
  assert.equal(h.state.undo.length, undo);
  h.hold('duplicate'); h.pad(1); h.pad(4); h.release('duplicate'); await h.settle();
  assert.deepEqual(clone(h.ST.pattern().steps[0][3]), { note: 60, instrument: 0, velocity: 100, gate: 0 }, 'a pad inside the pattern still takes the copy');
});

test('clearing step FX keeps the extra note columns of a row', async () => {
  const holder = fx => ({ note: -3, fx1: fx, columns: [{ note: 64, instrument: 0, velocity: 90, gate: 0 }] });
  const kept = { note: -3, columns: [{ note: 64, instrument: 0, velocity: 90, gate: 0 }] };
  // MACRO · Clear FX (display 8).
  let h = start();
  h.ST.mutate('holder', p => { p.tracks[0].noteColumns = 2; p.patterns[0].steps[0][5] = holder({ type: 'volume', value: .5 }); });
  h.state.track = 0; h.state.row = 5; h.press('macro'); await h.settle();
  assert.equal(h.MK3.page().buttons[7].label, 'Clear FX');
  h.press('display8'); await h.settle();
  assert.deepEqual(clone(h.ST.pattern().steps[0][5]), kept, 'the note in column 2 survives Clear FX');
  // MACRO · Prob encoder turned down to Off.
  h = start();
  h.ST.mutate('holder', p => { p.tracks[0].noteColumns = 2; p.patterns[0].steps[0][5] = holder({ type: 'probability', value: .05 }); });
  h.state.track = 0; h.state.row = 5; h.press('macro'); await h.settle();
  assert.equal(h.MK3.page().knobs[4].label, 'Prob');
  h.turn(4, -.2); await h.settle();
  assert.deepEqual(clone(h.ST.pattern().steps[0][5]), kept, 'the note in column 2 survives removing the effect');
  // A row that only held the lock still goes.
  h = start();
  h.ST.mutate('lock only', p => { p.patterns[0].steps[0][5] = { note: -3, fx1: { type: 'volume', value: .5 } }; });
  h.state.track = 0; h.state.row = 5; h.press('macro'); await h.settle(); h.press('display8'); await h.settle();
  assert.equal(h.ST.pattern().steps[0][5], null);
});

test('Shift + NOTES turns a chord step into a plain note-off (FX stay, the chord goes)', async () => {
  const h = start();
  h.ST.mutate('chord', p => { p.patterns[0].steps[0][2] = { note: 60, instrument: 0, velocity: 100, gate: 0, chord: [0, 4, 7], fx1: { type: 'probability', value: .5 } }; });
  h.state.track = 0; h.state.row = 2;
  h.hold('shift'); h.press('notes'); h.release('shift'); await h.settle();
  assert.deepEqual(clone(h.state.project.patterns[0].steps[0][2]), { note: -1, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'probability', value: .5 } });
});

test('stepped encoders follow slow hardware turns (single 0.001 counts)', async () => {
  const h = start();
  // SWING held: encoder 1 = swing in 1 % steps; 300 counts = 0.3 turn = 22.5 % of the 0–75 % range.
  const swing = h.state.project.swing || 0;
  h.hold('swing'); await h.settle(); assert.equal(h.MK3.page().knobs[0].label, 'Swing');
  h.counts(0, 300); await h.settle(); h.release('swing'); await h.settle();
  assert(Math.abs(h.state.project.swing - (swing + .225)) < .011, `swing moved by about 22.5 % (${swing} → ${h.state.project.swing})`);
  assert.equal(Math.round(h.state.project.swing * 100), Number((h.state.project.swing * 100).toFixed(6)), 'and stays on whole percents');
  // TEMPO held: encoder 2 = fine tempo, 0.1 BPM steps; 300 counts = 11.4 BPM.
  const bpm = h.state.project.bpm;
  h.hold('tempo'); await h.settle(); assert.equal(h.MK3.page().knobs[1].label, 'Fine');
  h.counts(1, 300); await h.settle(); h.release('tempo'); await h.settle();
  assert(Math.abs(h.state.project.bpm - (bpm + 11.4)) < .11, `fine tempo moved by about 11.4 BPM (${bpm} → ${h.state.project.bpm})`);
  // MACRO: the value of a percent step effect, 1 % steps, there and back.
  h.ST.mutate('fx', p => { p.patterns[0].steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'volume', value: .5 } }; });
  h.state.track = 0; h.state.row = 0; h.press('macro'); await h.settle();
  h.counts(1, 300); await h.settle();
  assert.equal(h.ST.pattern().steps[0][0].fx1.value, .8);
  h.counts(1, -300); await h.settle();
  assert.equal(h.ST.pattern().steps[0][0].fx1.value, .5);
  // One Undo step per gesture, as before.
  const undo = h.state.undo.length; h.counts(1, 40); await h.settle(); assert.equal(h.state.undo.length, undo + 1);
  // A value set by another control during the gesture is the new starting point (no jump back).
  h.hold('tempo'); await h.settle(); h.counts(1, 3); h.press('display5'); h.counts(1, 30); await h.settle(); h.release('tempo'); await h.settle();
  assert(Math.abs(h.state.project.bpm - 121.1) < .11, `display button 120, then +1.1 on the fine knob (${h.state.project.bpm})`);
});

test('a group bus above +3.5 dB keeps its level when its MK3 volume encoder moves', async () => {
  const h = start(groupSession());
  h.ST.mutate('loud bus', () => { h.bus().volume = 2; });
  h.press('channel'); await h.settle(); h.press('display2'); await h.settle();
  assert.equal(h.MK3.page().knobs[0].value, '+6.0 dB'); assert.equal(h.MK3.page().knobs[0].norm, 1);
  h.turn(0, .001); await h.settle();
  assert.equal(h.bus().volume, 2, 'turning up at the top of the fader changes nothing');
  h.turn(0, -.25); await h.settle();
  assert.equal(h.bus().volume, 1.5, 'a quarter turn down is a quarter of the 0–2 range');
  // Track faders keep their 0–1.5 range.
  h.press('display3'); await h.settle(); h.turn(0, 1); await h.settle();
  assert.equal(h.state.project.tracks[0].volume, 1.5);
});

test('EVENTS rows chosen in a longer pattern are dropped in a shorter one', async () => {
  const h = start(); shorten(h, 1, 16); await h.settle();
  h.press('step'); h.press('arrowRight'); h.press('arrowRight'); h.press('arrowRight');
  assert.equal(h.MK3.state.stepPage, 3);
  h.press('events'); assert.equal(h.MK3.state.latched, 'events');
  h.pad(1); h.pad(2); assert.deepEqual([...h.MK3.state.events], [48, 49]);
  h.state.project.activePattern = h.state.project.patterns[1].id; h.state.row = 0; h.ST.refresh(); await h.settle();
  assert.deepEqual([...h.MK3.state.events], [], 'rows 48 and 49 do not exist in the 16-row pattern');
  h.press('display2'); await h.settle(); // Nudge ›
  assert(h.state.project.patterns[1].steps.every(steps => steps.length === 16), 'nudging writes no rows past the end');
});

test('plug-in automation gestures end per parameter', async () => {
  const h = start();
  for (const file of ['fine-control.js', 'effect-pages.js', 'routing-project.js', 'studio.js']) h.load(file);
  h.win.STEffectPages.setMetadata(metadata());
  const plugins = [{ owner: `track:${h.state.project.tracks[0].routingID}`, kind: 'effect', slot: 0, component: 'stfx:filter', name: 'Multimode Filter', format: 'Builtin', instanceID: 'fx-filter', state: 's' }];
  h.state.project.audioPlugins = clone(plugins);
  h.win.STHostTransport.post = message => {
    h.calls.push(message);
    const result = message.command === 'plugin.parameters' ? { parameters: parameterList('stfx:filter') } : message.command === 'engine.snapshot' ? { audioPlugins: clone(plugins) } : {};
    Promise.resolve().then(() => h.win.STHostTransport.deliver({ id: message.id, ok: true, result }));
  };
  const log = [];
  h.win.STAutomationRecord = { mode: 'touch', labels: {}, makeRef: (owner, target, name) => ({ owner, target, name }), wouldRecord: () => true, claimed: () => false,
    value: ref => { log.push('value ' + ref.target); return true; }, end: ref => { log.push('end ' + ref.target); return true; } };
  h.press('plugin'); h.press('display2'); await h.settle(); await h.settle();
  const knobs = h.MK3.page().knobs; assert.equal(knobs[0].label, 'Cutoff'); assert.equal(knobs[1].label, 'Reso');
  h.turn(0, .05); h.advance(100); h.turn(1, .05); h.advance(100);
  await h.settle();
  const ends = log.filter(entry => entry.startsWith('end ')).sort(), values = [...new Set(log.filter(entry => entry.startsWith('value ')))].sort();
  assert.equal(values.length, 2, 'two parameters were written');
  assert.deepEqual(ends, values.map(entry => entry.replace('value ', 'end ')), 'each written parameter ends its own gesture');
});

test('simulator input is not taken for a connected controller', async () => {
  const h = start();
  const elements = [], node = () => { const n = { dataset: {}, style: { setProperty() {} }, classList: { toggle() {}, add() {}, remove() {}, contains: () => false }, children: [], handlers: {},
    append(...items) { this.children.push(...items); }, remove() {}, setAttribute() {}, addEventListener(type, fn) { this.handlers[type] = fn; }, setPointerCapture() {}, getContext() { return null; },
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
    querySelector(selector) { return this.all().find(item => matches(item, selector)) || node(); }, querySelectorAll(selector) { return this.all().filter(item => matches(item, selector)); },
    all() { return this.children.flatMap(child => [child, ...child.all()]); }, set id(value) { this._id = value; }, get id() { return this._id; } }; elements.push(n); return n; };
  const matches = (item, selector) => selector.split(',').some(part => { const data = /^\[data-mk3-(\w+)(?:="([^"]*)")?\]/.exec(part.trim()), id = /^#([\w-]+)$/.exec(part.trim());
    if (id) return item.id === id[1];
    if (!data) return false; const key = 'mk3' + data[1][0].toUpperCase() + data[1].slice(1); return item.dataset[key] !== undefined && (data[2] === undefined || item.dataset[key] === data[2]); });
  const document = { readyState: 'complete', body: node(), createElement: () => node() };
  assert.equal(h.state.native, true); assert.equal(h.MK3.state.connected, false);
  web.run('mk3-sim.js', { window: h.win, document, setTimeout() {} });
  const root = h.win.STMK3Sim.open(), mixer = root.querySelectorAll('[data-mk3-button]').find(item => item.dataset.mk3Button === 'mixer');
  h.calls.length = 0;
  mixer.handlers.pointerdown({ preventDefault() {}, button: 0 }); mixer.handlers.pointerup({});
  await h.settle();
  assert.equal(h.MK3.state.screen, 'mixer', 'the simulator still drives the pages');
  assert.equal(h.MK3.state.connected, false, 'but no controller is connected');
  assert.deepEqual(h.calls.filter(call => /^mk3\./.test(call.command)), [], 'so nothing is sent to the MK3 driver');
  // Hardware input (from the native driver) still marks the controller connected.
  h.MK3.event('mk3.input', { type: 'button', name: 'notes', pressed: true }); h.MK3.event('mk3.input', { type: 'button', name: 'notes', pressed: false });
  assert.equal(h.MK3.state.connected, true);
});

(async () => {
  let failures = 0;
  for (const [name, fn] of cases) { try { await fn(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`MK3 controller fixes: ${cases.length - failures}/${cases.length} groups passed.`);
  if (failures) process.exitCode = 1;
})();
