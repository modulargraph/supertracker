'use strict';
// Exercise page navigation against the real controller modules. No hardware is opened.
const assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tests = [], test = (name, fn) => tests.push([name, fn]);
// A track group whose cursor member plays an external MIDI instrument.
function midiMember() {
  const h = groupSession();
  Object.assign(h.state.project.instruments[h.state.instrument], { type: 'midi', midiChannel: 9, midiInputChannel: 3 });
  return h;
}
for (const screen of ['channel', 'plugin']) {
  test(`MIDI group member ${screen} page arrows are safe and keep its routing`, () => {
    const h = midiMember(), original = JSON.stringify(h.state.project); h.press(screen);
    for (const name of ['arrowRight', 'arrowLeft']) {
      assert.doesNotThrow(() => h.press(name));
      assert.equal(h.state.track, 0);
      assert.equal(JSON.stringify(h.state.project), original);
    }
    for (const level of ['display1', 'display2', 'display3', 'display4']) { assert.doesNotThrow(() => { h.press(level); h.press('arrowRight'); h.press('arrowLeft'); }); }
    assert.equal(JSON.stringify(h.state.project), original);
  });
}
test('all eight mixer encoders and labels follow entries 9–16 in the second bank', async () => {
  const h = groupSession();
  while (h.state.project.tracks.length < 20) h.ST.appendInstrumentTrack(h.state.project, 0);
  h.MK3.frame(); h.input({ type: 'button', name: 'shift', pressed: true }); h.press('arrowRight'); h.input({ type: 'button', name: 'shift', pressed: false });
  h.press('mixer');
  const volumes = h.state.project.tracks.map(track => track.volume);
  const page = h.MK3.page(), model = h.MK3.screenModel(), tracks = Array.from({ length: 8 }, (_, i) => 11 + i);
  assert.deepEqual(Array.from(page.data.strips, s => s.track), tracks, 'entry 9 is track 12: the group (tracks 1–4) counts once');
  for (let i = 0; i < 8; i++) {
    const track = h.state.project.tracks[tracks[i]];
    assert.equal(model.knobs[i].label, track.name.slice(0, 14));
    assert.equal(model.buttons[i].label, track.name.slice(0, 16));
    h.input({ type: 'encoder', index: i, delta: -.1 });
  }
  await h.MK3.flushEdits();
  assert(h.state.project.tracks.slice(0, 11).every((track, i) => track.volume === volumes[i]));
  assert(tracks.every(i => h.state.project.tracks[i].volume < volumes[i]));
});
test('coalesced encoder detents move the automation picker as far as separate detents', () => {
  function navigate(deltas) {
    const h = session();
    h.win.STStudio = { automationChoices: () => Array.from({ length: 10 }, (_, index) => ({ available: true, name: 'Parameter ' + index, target: 'parameter:' + index, group: 'Test' })) };
    h.MK3.state.overlay = 'autoLanes'; h.MK3.state.lists.autoLanes = 5;
    // Establish the app view before opening the overlay so input's view synchronization preserves it.
    h.MK3.state.lastView = h.state.view;
    assert.equal(h.MK3.page().id, 'autoLanes');
    for (const delta of deltas) h.input({ type: 'encoder', index: 0, delta });
    return h.MK3.page().data.selected;
  }
  for (const direction of [-1, 1]) {
    const separate = navigate([.041, .04, .04].map(delta => delta * direction));
    assert.equal(separate, 5 + direction * 3);
    assert.equal(navigate([.121 * direction]), separate);
  }
});
(async () => { for (const [name, fn] of tests) { await fn(); console.log('PASS ' + name); } console.log(`${tests.length} MK3 page navigation groups passed`); })().catch(error => { console.error(error); process.exitCode = 1; });
