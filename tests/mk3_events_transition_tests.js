'use strict';
// Adversarial EVENTS gesture order: moving rows must not discard pending field edits.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tests = []; const test = (name, run) => tests.push({ name, run });
// group: the cursor track is a member of a track group (it edits like any track).
function setup(group = false) {
  const h = group ? groupSession() : session();
  h.state.row = 4; h.ST.pattern().steps[0] = Array(h.ST.pattern().length).fill(null);
  h.ST.pattern().steps[0][4] = { note: 60, velocity: 90, gate: .8 };
  h.press('events'); h.MK3.state.events = new Set([4]);
  h.hit = row => h.ST.pattern().steps[0][row];
  return h;
}
for (const group of [false, true]) for (const order of ['field-first', 'nudge-first']) {
  test(`${group ? 'group member' : 'tracker'} ${order} EVENTS preserves pitch and moves the row`, async () => {
    const h = setup(group), original = h.hit(4).note;
    if (order === 'field-first') { h.press('display4'); h.press('display2'); } else { h.press('display2'); h.press('display4'); }
    await h.MK3.flushEdits(); assert.equal(h.hit(5).note, original + 1); assert.equal(h.ST.pattern().steps[0][4], null);
  });
}
test('rejected field commit blocks nudge without losing the retryable edit or selected rows', async () => {
  const h = setup(); h.press('display4'); h.win.STFreeze = { checkMutation: () => 'blocked', explain() {} };
  h.press('display2'); assert.equal(h.ST.pattern().steps[0][4].note, 60); assert.equal(h.ST.pattern().steps[0][5], null);
  assert.deepEqual([...h.MK3.state.events], [4]); assert.equal(h.MK3.hasPendingEdits, true);
  h.win.STFreeze.checkMutation = () => null; await h.MK3.flushEdits(); assert.equal(h.hit(4).note, 61);
});
test('pending transpose on another lane stays owned when a new lane is nudged', async () => {
  const h = setup(); h.press('display4'); h.state.track = 1;
  h.ST.pattern().steps[1][4] = { note: 72, velocity: 100 }; h.ST.pattern().steps[1][5] = null;
  h.press('display2'); await h.MK3.flushEdits(); assert.equal(h.ST.pattern().steps[0][4].note, 61); assert.equal(h.ST.pattern().steps[1][5].note, 72);
});
for (const overlay of ['volume', 'swing']) test(`pending ${overlay === 'volume' ? 'Velocity' : 'Position'} overlay survives EVENTS nudge`, async () => {
  const h = setup(true); h.press('events');
  h.input({ type: 'button', name: 'shift', pressed: true }); h.press(overlay); h.input({ type: 'button', name: 'shift', pressed: false });
  h.input({ type: 'main', delta: 1 }); h.input({ type: 'button', name: 'events', pressed: true }); h.MK3.state.events = new Set([4]); h.press('display2'); h.input({ type: 'button', name: 'events', pressed: false }); await h.MK3.flushEdits();
  if (overlay === 'volume') assert.equal(h.hit(5).velocity, 91); else assert.equal(h.hit(5).fx1.value, .05);
  assert.equal(h.hit(5).note, 60);
});
for (const group of [false, true]) for (const overlay of ['volume', 'swing']) test(`pending nudge settles before ${group ? 'group member' : 'tracker'} ${overlay} overlay reads the cursor`, async () => {
  const h = setup(group); h.press('display2'); assert.equal(h.MK3.hasPendingEdits, true);
  h.input({ type: 'button', name: 'shift', pressed: true }); h.press(overlay); h.input({ type: 'button', name: 'shift', pressed: false });
  assert.equal(h.ST.pattern().steps[0][4], null); assert.equal(h.MK3.page().data.header.value, 'Unavailable');
  h.state.row = 5;
  h.input({ type: 'main', delta: 1 }); await h.MK3.flushEdits();
  if (overlay === 'volume') assert.equal(h.hit(5).velocity, 91); else assert.equal(h.hit(5).fx1.value, .05);
});
for (const overlay of ['volume', 'swing', 'tempo']) test(`rejected nudge keeps the existing mode when opening Shift ${overlay}`, async () => {
  const h = setup(); h.press('display2'); h.win.STFreeze = { checkMutation: () => 'blocked', explain() {} };
  h.input({ type: 'button', name: 'shift', pressed: true }); h.press(overlay); h.input({ type: 'button', name: 'shift', pressed: false });
  assert.equal(h.MK3.state.overlay, null); assert.equal(h.MK3.page().id, 'events'); assert.equal(h.MK3.hasPendingEdits, true);
  assert.match(h.MK3.state.flash.text, /pending.*edit.*retry/i); assert.equal(h.hit(4).note, 60);
  h.win.STFreeze.checkMutation = () => null; await h.MK3.flushEdits(); assert.equal(h.hit(5).note, 60);
});
(async () => { let failed = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length - failed}/${tests.length} EVENTS transition groups passed`); if (failed) process.exitCode = 1; })();
