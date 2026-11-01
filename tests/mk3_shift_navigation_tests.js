'use strict';
// Printed Save / Exit / Navigate legends must never trigger unrelated destructive edits.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
// group: the cursor track is a member of a track group.
function create(group) { return group ? groupSession() : session(); }
function shifted(h, name) { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); }
const tests = [];
for (const kit of [false, true]) {
  const label = kit ? 'group member' : 'track';
  tests.push([`${label}: FILE opens menu, Shift FILE saves, Save As is explicit`, async () => {
    const h = create(kit); h.calls.length = 0; h.press('file'); await tick();
    assert.equal(h.MK3.page().id, 'file');
    assert(!h.calls.some(x => /^project.save/.test(x.command)));
    h.press('encoderPush'); assert.notEqual(h.MK3.page().id, 'file');
    shifted(h, 'file'); await tick();
    assert(h.calls.some(x => x.command === 'project.save'));
    assert(!h.calls.some(x => x.command === 'project.saveAs'));
    h.press('file'); h.press('display6'); await tick();
    assert(h.calls.some(x => x.command === 'project.saveAs'));
  }]);
  tests.push([`${label}: Shift LOCK exits and preserves step locks and history`, async () => {
    const h = create(kit), row = h.state.row;
    h.ST.pattern().steps[h.state.track][row] = { note: 60, velocity: 97, gate: .8, fx1: { type: 'volume', value: .45 }, instrument: 0 };
    h.press('lock'); assert.equal(h.MK3.page().id, 'lock');
    const before = JSON.stringify(h.state.project), undo = h.state.undo.length;
    shifted(h, 'lock'); await tick();
    assert.notEqual(h.MK3.page().id, 'lock');
    assert.equal(JSON.stringify(h.state.project), before);
    assert.equal(h.state.undo.length, undo);
  }]);
  tests.push([`${label}: Shift VARIATION navigates without randomizing or humanizing`, async () => {
    const h = create(kit); h.press('mixer');
    h.ST.pattern().steps[h.state.track][h.state.row] = { note: 60, velocity: 97, gate: .8, instrument: 0 };
    const before = JSON.stringify(h.state.project), undo = h.state.undo.length;
    shifted(h, 'variation'); await tick();
    assert.equal(h.MK3.page().id, 'notes');
    const row = h.state.row; h.input({ type: 'main', delta: 1 });
    assert.equal(h.state.row, row + 1);
    assert.equal(JSON.stringify(h.state.project), before);
    assert.equal(h.state.undo.length, undo);
  }]);
}
(async () => { let failures = 0; for (const [name, run] of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length-failures}/${tests.length} MK3 Shift navigation checks passed`); if (failures) process.exitCode = 1; })();
