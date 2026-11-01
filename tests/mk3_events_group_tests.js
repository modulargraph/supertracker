'use strict';
// EVENTS gestures on a member of a track group edit only the cursor track's rows: the other members (which hit
// on the same rows) are untouched, pending gestures stay with their track, and each gesture is one Undo step.
const assert = require('node:assert/strict');
const { groupSession } = require('./helpers/mk3-session');
const copy = value => JSON.parse(JSON.stringify(value));
const tests = [], test = (name, fn) => tests.push([name, fn]);
const controls = [
  { index: 1, delta: .031, field: 'note', value: before => before + 1 },
  { index: 2, delta: .021, field: 'velocity', value: before => before + 2 },
  { index: 3, delta: .031, field: 'gate', value: before => Number((before * 1.08).toFixed(3)) },
  { index: 4, delta: .031, field: 'fx1', value: before => ({ type: 'microtiming', value: Number((before.value + .05).toFixed(2)) }) }
];
// Members 0–2 of the group all hit on row 3; the cursor is on `track`.
function session(track = 0) {
  const h = groupSession(), pat = h.ST.pattern();
  const event = index => ({ note: 60 + index, instrument: h.state.project.tracks[index].instrument, velocity: 80 + index, gate: .5, fx1: { type: 'microtiming', value: .2 }, fx2: { type: 'volume', value: .7 } });
  for (const index of [0, 1, 2]) { pat.steps[index] = Array(pat.length).fill(null); pat.steps[index][3] = event(index); }
  pat.steps[track][4] = event(track);
  h.state.track = track; h.state.row = 3; h.MK3.state.events = new Set([3]);
  h.input({ type: 'button', name: 'events', pressed: true });
  assert.equal(h.MK3.page().id, 'events');
  h.hit = index => h.ST.pattern().steps[index][3];
  return h;
}
for (const track of [0, 1]) test(`EVENTS encoders edit only member ${track + 1} of the group and Undo exactly`, async () => {
  for (const control of controls) {
    const h = session(track), before = copy(h.ST.pattern().steps), history = h.state.undo.length;
    const selected = copy(h.hit(track)), other = copy(h.hit(1 - track));
    h.input({ type: 'encoder', index: control.index, delta: control.delta }); await h.MK3.flushEdits();
    assert.deepEqual(copy(h.hit(track)[control.field]), control.value(selected[control.field]));
    assert.deepEqual(copy(h.hit(1 - track)), other); assert.deepEqual(copy(h.hit(2)), before[2][3]);
    assert.equal(h.state.undo.length, history + 1);
    await h.ST.perform('undo'); assert.deepEqual(copy(h.ST.pattern().steps), before);
  }
});
test('EVENTS on a member with no notes on the selected rows makes no edit or Undo entry', async () => {
  const h = session(0); h.state.track = 3; h.ST.pattern().steps[3][3] = null;
  const before = JSON.stringify(h.state.project), history = h.state.undo.length;
  for (const control of controls) h.input({ type: 'encoder', index: control.index, delta: control.delta });
  await h.MK3.flushEdits();
  assert.equal(JSON.stringify(h.state.project), before); assert.equal(h.state.undo.length, history);
});
test('pending EVENTS gestures stay with each member across a cursor change', async () => {
  const h = session(0), before = copy(h.ST.pattern().steps), history = h.state.undo.length;
  h.input({ type: 'encoder', index: 1, delta: .031 });
  h.state.track = 1;
  h.input({ type: 'encoder', index: 1, delta: .031 }); await h.MK3.flushEdits();
  assert.equal(h.hit(0).note, 61); assert.equal(h.hit(1).note, 62); assert.equal(h.hit(2).note, 62);
  assert.equal(h.state.undo.length, history + 2);
  await h.ST.perform('undo'); await h.ST.perform('undo'); assert.deepEqual(copy(h.ST.pattern().steps), before);
});
test('simultaneous EVENTS pitch, velocity, gate and timing gestures preserve one another', async () => {
  const h = session(1), before = copy(h.ST.pattern().steps), selected = copy(h.hit(1));
  for (const control of controls) h.input({ type: 'encoder', index: control.index, delta: control.delta });
  await h.MK3.flushEdits();
  for (const control of controls) assert.deepEqual(copy(h.hit(1)[control.field]), control.value(selected[control.field]));
  assert.deepEqual(copy(h.hit(1).fx2), selected.fx2); assert.deepEqual(copy(h.hit(2)), before[2][3]);
  for (const control of controls) await h.ST.perform('undo');
  assert.deepEqual(copy(h.ST.pattern().steps), before);
});
(async () => { for (const [name, fn] of tests) { await fn(); console.log('PASS ' + name); } console.log(`${tests.length} MK3 EVENTS group groups passed`); })().catch(error => { console.error(error); process.exitCode = 1; });
