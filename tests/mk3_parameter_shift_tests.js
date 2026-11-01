'use strict';
// Printed Velocity / Position / Tune layers use real model ownership and boundaries.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tests = []; const test = (name, run) => tests.push({ name, run });
// group: the cursor track is a member of a track group (its own instrument and steps, like any track).
function prepare(group = false) {
  const h = group ? groupSession() : session();
  h.state.row = 4;
  h.ST.pattern().steps[0][4] = { note: 60, velocity: 90, gate: .8 };
  h.hit = () => h.ST.pattern().steps[0][4];
  h.inst = () => h.state.project.instruments[h.state.instrument];
  h.shifted = name => { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); };
  h.turn = (delta, index) => h.input(index == null ? { type: 'main', delta } : { type: 'encoder', index, delta });
  return h;
}
for (const group of [false, true]) {
  const kit = group, name = group ? 'group member' : 'tracker';
  test(`${name} Velocity edits its note and clamps 1–127 without changing master`, async () => {
    const h = prepare(kit), master = h.state.project.masterVolume, before = h.hit().velocity;
    h.shifted('volume'); assert.equal(h.MK3.page().id, 'velocity'); h.turn(1); await h.MK3.flushEdits(); assert.equal(h.hit().velocity, before + 1);
    h.turn(128); await h.MK3.flushEdits(); assert.equal(h.hit().velocity, 127);
    h.turn(-128); await h.MK3.flushEdits(); assert.equal(h.hit().velocity, 1);
    assert.equal(h.state.project.masterVolume, master);
  });
  test(`${name} Position edits only microtiming and never resets swing`, async () => {
    const h = prepare(kit); h.state.project.swing = .22;
    h.shifted('swing'); assert.equal(h.MK3.page().id, 'position'); h.turn(.026, 0); await h.MK3.flushEdits(); assert.equal(h.hit().fx1.type, 'microtiming'); assert.equal(h.hit().fx1.value, .05);
    h.turn(128); await h.MK3.flushEdits(); assert.equal(h.hit().fx1.value, .95);
    h.turn(-128); await h.MK3.flushEdits(); assert.equal(h.hit().fx1, undefined);
    assert.equal(h.state.project.swing, .22);
  });
  test(`${name} Tune edits semitones/cents within ±48 and keeps tempo`, async () => {
    const h = prepare(kit), bpm = h.state.project.bpm; h.inst().tune = 0;
    h.shifted('tempo'); assert.equal(h.MK3.page().id, 'tune'); h.turn(1); await h.MK3.flushEdits(); assert.equal(h.inst().tune, 1);
    h.turn(.021, 1); await h.MK3.flushEdits(); assert.equal(h.inst().tune, 1.01);
    h.turn(128); await h.MK3.flushEdits(); assert.equal(h.inst().tune, 48);
    h.turn(-128); await h.MK3.flushEdits(); assert.equal(h.inst().tune, -48);
    assert.equal(h.state.project.bpm, bpm); if (kit) assert.equal(h.state.project.instruments[1].tune || 0, 0, 'other members keep their tuning');
  });
}
test('Tune rejects plug-in and MIDI sources for tracks and group members without history changes', async () => {
  for (const kit of [false, true]) for (const type of ['au', 'midi']) {
    const h = prepare(kit); h.inst().type = type; const project = JSON.stringify(h.state.project), undo = h.state.undo.length;
    h.shifted('tempo'); assert.equal(h.MK3.page().data.header.value, 'Unavailable'); assert(h.MK3.page().knobs.every(x => !x));
    h.turn(5); h.turn(.8, 0); await h.MK3.flushEdits(); assert.equal(JSON.stringify(h.state.project), project); assert.equal(h.state.undo.length, undo);
  }
});
test('Velocity and Position on an empty row of a group member do not create notes or edit a neighbor', async () => {
  const h = prepare(true); h.state.row = 5; h.ST.pattern().steps[0][5] = null; const project = JSON.stringify(h.state.project), undo = h.state.undo.length;
  for (const key of ['volume', 'swing']) { h.shifted(key); assert.equal(h.MK3.page().data.header.value, 'Unavailable'); h.turn(1); h.turn(.5, 0); await h.MK3.flushEdits(); }
  assert.equal(JSON.stringify(h.state.project), project); assert.equal(h.state.undo.length, undo);
});
test('pending Tune remains on the original member when SELECT + pad selects another member', async () => {
  const h = prepare(true), first = h.state.instrument; h.inst().tune = 0;
  h.shifted('tempo'); h.turn(2);
  h.input({ type: 'button', name: 'select', pressed: true }); h.down(3); h.up(3); h.input({ type: 'button', name: 'select', pressed: false });
  assert.equal(h.state.track, 2); await h.MK3.flushEdits();
  assert.equal(h.state.project.instruments[first].tune, 2); assert.equal(h.state.project.instruments[h.state.instrument].tune || 0, 0);
});
(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length - failures}/${tests.length} printed parameter layer groups passed`); if (failures) process.exitCode = 1; })();
