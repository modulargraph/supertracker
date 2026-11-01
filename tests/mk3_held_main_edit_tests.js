'use strict';
// A main-encoder edit while holding a STEP pad must not become tap-to-delete on release.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tests = []; const test = (name, run) => tests.push({ name, run });
// group: the cursor track is a member of a track group.
function setup(group) {
  const h = group ? groupSession() : session();
  h.ST.pattern().steps[0][0] = { note: 60, velocity: 90 };
  h.state.undo = []; h.state.redo = []; h.press('step');
  h.hit = () => h.ST.pattern().steps[0][0];
  return h;
}
for (const group of [false, true]) test(`${group ? 'group member' : 'tracker'} held step + main encoder survives quick release and Undo`, async () => {
  const h = setup(group), before = h.hit().note; h.down(1); h.input({ type: 'main', delta: 1 });
  assert.equal(h.MK3.hasPendingEdits, true); h.up(1); await h.MK3.flushEdits();
  assert.equal(h.hit()?.note, before + 1, 'edited hit was deleted as if its pad had only been tapped');
  assert.equal(h.state.undo.length, 1);
  await h.ST.perform('undo'); assert.equal(h.hit().note, before);
});
test('plain step-pad tap still removes an existing hit', () => {
  const h = setup(false); h.down(1); h.up(1); assert.equal(h.hit(), null);
});
test('Undo while the edited step is still held cannot turn its eventual pad-up into a deletion', async () => {
  const h = setup(false); h.down(1); h.input({ type: 'main', delta: 1 }); await h.ST.perform('undo'); h.up(1);
  assert.equal(h.hit().note, 60); assert.equal(h.MK3.state.pads.size, 0);
});
(async () => { let failed = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length - failed}/${tests.length} held main-edit groups passed`); if (failed) process.exitCode = 1; })();
