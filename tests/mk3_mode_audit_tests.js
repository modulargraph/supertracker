'use strict';
// Mode changes must finish the previous physical gesture, including its native owner.
// Uses synthetic hardware events and the real app/controller modules; no device is opened.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const tests = [];
const test = (name, run) => tests.push({ name, run });
function shifted(h, name) { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); }
function touch(h, touched) { h.input({ type: 'touch', target: 'strip', touched }); }
function expressions(h, kind) { return h.calls.filter(x => x.command === 'midi.expression' && x.payload.kind === kind).map(x => x.payload); }

for (const group of [false, true]) for (const [mode, kind] of [['pitch', 'pitchBend'], ['mod', 'cc']]) {
  test(`${group ? 'group member' : 'track'} ${mode} toggle-off releases expression on its original owner`, async () => {
    const h = group ? groupSession() : session();
    h.press(mode); touch(h, true); h.input({ type: 'strip', value: 1 });
    const before = expressions(h, kind).at(-1); assert.equal(before.value, 1);
    h.state.track = 1;
    h.press(mode); touch(h, false); await tick();
    const after = expressions(h, kind).at(-1);
    assert.equal(after.value, 0, 'turning the mode off must spring the previous gesture back');
    assert.equal(after.owner ?? after.track, before.owner ?? before.track);
    assert.equal(h.MK3.state.strip.mode, null);
  });
}

test('changing pitch to MOD ends pitch immediately before another strip movement', () => {
  const h = session(); h.press('pitch'); touch(h, true); h.input({ type: 'strip', value: 1 });
  h.press('mod'); assert.equal(expressions(h, 'pitchBend').at(-1).value, 0);
});

test('turning Perform off restores the original pattern loop after stutter', async () => {
  const h = session(); h.state.playing = true; h.state.playbackMode = 'pattern';
  h.state.status = { pattern: h.ST.pattern().id, row: 6, songMode: false };
  const original = { pattern: h.ST.pattern().id, start: 0, end: 15 }; h.state.patternLoop = { ...original };
  h.MK3.state.perfFx = 'stutter'; h.press('perform'); touch(h, true); h.input({ type: 'strip', value: 1 });
  assert.notEqual(JSON.stringify(h.state.patternLoop), JSON.stringify(original));
  h.press('perform'); touch(h, false); await tick();
  assert.equal(JSON.stringify(h.state.patternLoop), JSON.stringify(original));
  assert.equal(h.MK3.state.stutter, null);
});

test('Shift + a different group button keeps recording armed; the same one toggles it off', async () => {
  const h = groupSession(); await h.ST.setNoteRecording(true); assert.equal(h.state.recording, true);
  shifted(h, 'groupB'); await tick(); assert.equal(h.state.track, 4, 'group B = the entry after the group (tracks 1–4)');
  assert.equal(h.state.recording, true, 'selecting another group should arm that destination');
  shifted(h, 'groupB'); await tick(); assert.equal(h.state.recording, false);
  shifted(h, 'groupA'); await tick(); assert.equal(h.state.track, 0, 'group A selects the group member'); assert.equal(h.state.recording, true);
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`${tests.length - failures}/${tests.length} MK3 mode audit checks passed`);
  if (failures) process.exitCode = 1;
})();
