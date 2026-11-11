'use strict';
// MK3 workflow improvements through the real app + controller handlers (synthetic mk3.input only; no HID, USB,
// audio or native app): Arp page and note-repeat gate/octaves, 16 Velocities, AUTO hold-to-write, lit KEYBOARD
// opens its pad mode page.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const source = fs.readFileSync(path.join(__dirname, 'mk3_tests.js'), 'utf8');
// The shared harness, with the real automation recorder loaded before the controller.
const prefix = source.slice(0, source.indexOf('const contract =')).replace("'sampler.js', 'mk3-screens.js'", "'sampler.js', 'automation-record.js', 'mk3-screens.js'");
assert.ok(prefix.includes("'automation-record.js'"), 'harness loads the automation recorder');
const { harness } = new Function('require', '__dirname', prefix + '\nreturn {harness};')(require, __dirname);
const tick = async () => { for (let i = 0; i < 60; i++) await Promise.resolve(); };

function session() {
  const h = harness();
  h.state.project = h.ST.normalize({ name: 'Workflow', tracks: [{ id: 0, routingID: 'one', name: 'Bass', instrument: 0 }, { id: 1, routingID: 'two', name: 'Keys', instrument: 0 }] });
  Object.assign(h.state, { view: 'pattern', track: 0, instrument: 0, row: 0, selection: null });
  h.MK3.subscribe(() => {});
  h.input = payload => h.MK3.event('mk3.input', payload);
  h.press = (name, hold = false) => { h.input({ type: 'button', name, pressed: true }); if (!hold) h.input({ type: 'button', name, pressed: false }); };
  h.release = name => h.input({ type: 'button', name, pressed: false });
  h.shift = name => { h.press('shift', true); h.press(name); h.release('shift'); };
  h.pad = (n, velocity = 100, hold = false) => { h.input({ type: 'pad', pad: n, phase: 'down', velocity }); if (!hold) h.input({ type: 'pad', pad: n, phase: 'up' }); };
  h.sent = command => h.calls.filter(call => call.command === command);
  h.R = h.win.STNoteRepeat; h.AR = h.win.STAutomationRecord;
  return h;
}
const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('note repeat exposes gate and arp octaves; octaves spread arp modes only', async () => {
  const h = session(), { R } = h;
  R.set({ enabled: true, mode: 'up', rate: '1/16', octaves: 2, gate: .3 }, false);
  assert.equal(R.octaves, 2); assert.equal(R.gate, .3);
  assert.match(R.label(), /×2 oct/);
  R.set({ octaves: 9, gate: 4 }, false); assert.equal(R.octaves, 4); assert.equal(R.gate, 1, 'ranges are clamped');
  R.set({ octaves: 2, gate: .5 }, false);
  h.calls.length = 0;
  h.ST.noteOn(60, 'k1', false, 0, 0, true, 100);
  h.ST.noteOn(64, 'k2', false, 0, 0, true, 100);
  for (let i = 0; i < 12; i++) { h.advance(130); await tick(); }
  const notes = h.sent('note.on').map(call => call.payload.note);
  for (const note of [60, 64, 72, 76]) assert.ok(notes.includes(note), `arp up over two octaves plays ${note}: ${notes}`);
  h.ST.noteOff('k1'); h.ST.noteOff('k2'); await tick();
  R.set({ mode: 'repeat' }, false); assert.doesNotMatch(R.label(), /oct/, 'plain repeat has no octave range');
  R.set({ enabled: false }, false);
});

test('Shift + NOTE REPEAT opens the Arp page: modes on display buttons, Mode/Rate/Octaves/Gate encoders; again closes', async () => {
  const h = session(), { R, MK3 } = h;
  R.set({ enabled: false, mode: 'repeat', octaves: 1, gate: .5 }, false);
  h.shift('noteRepeat');
  let page = MK3.page();
  assert.equal(page.id, 'arp');
  assert.equal(page.knobs.slice(0, 4).map(k => k?.label).join(), 'Mode,Rate,Octaves,Gate');
  assert.equal(page.knobs[2].dim, true, 'octaves are greyed out for plain repeat');
  assert.equal(R.enabled, false, 'opening the page does not start repeating');
  h.press('display2'); // Up
  assert.equal(R.mode, 'up');
  page = MK3.page(); assert.equal(page.buttons[1].on, true); assert.notEqual(page.knobs[2].dim, true);
  h.input({ type: 'encoder', index: 2, delta: .3 }); assert.ok(R.octaves >= 2, 'encoder 3 sets octaves');
  h.input({ type: 'encoder', index: 3, delta: -.1 }); assert.ok(R.gate < .5, 'encoder 4 shortens the gate');
  h.input({ type: 'main', delta: 1 }); assert.equal(R.mode, 'down', 'the 4-D encoder walks the modes');
  const { screens } = MK3.frame(); assert.ok(screens.left.length && screens.right.length);
  assert.ok(MK3.frame().leds.buttons.noteRepeat >= .6, 'NOTE REPEAT is lit while the page is open');
  // Holding NOTE REPEAT over the page plays with these settings and shows the rate page with Gate too.
  h.press('noteRepeat', true); assert.equal(MK3.page().id, 'noteRepeat'); assert.equal(R.enabled, true);
  assert.ok(MK3.page().knobs.some(k => k?.label === 'Gate'));
  h.advance(500); h.release('noteRepeat'); assert.equal(R.enabled, false, 'a hold (not a tap) stops on release');
  assert.equal(MK3.page().id, 'arp', 'the Arp page is back after the hold');
  h.shift('noteRepeat'); assert.notEqual(MK3.page().id, 'arp');
});

test('Shift + FIXED VEL: 16 Velocities on the cursor track note, graded LEDs, again returns to the previous pad mode', async () => {
  const h = session(), { MK3, state } = h;
  h.press('keyboard'); assert.equal(MK3.state.padMode, 'keyboard');
  h.shift('fixedVel');
  assert.equal(MK3.state.padMode, 'velocity');
  assert.match(MK3.state.flash.text, /^16 velocities · .* · Bass$/);
  const leds = MK3.frame().leds;
  assert.equal(leds.buttons.fixedVel, 1);
  assert.ok(leds.pads.every(Boolean), 'all sixteen pads lit');
  assert.notEqual(leds.pads[0], leds.pads[15], 'soft pad 1 is dimmer than pad 16');
  h.calls.length = 0;
  h.pad(1, 120); h.pad(16, 3); h.pad(8, 64); await tick();
  const on = h.sent('note.on').map(call => call.payload);
  assert.equal(on.map(p => p.velocity).join(), '8,127,64', 'pad n plays velocity n × 127 / 16, whatever the hit');
  assert.ok(on.every(p => p.note === on[0].note && p.track === 0), 'one note on the cursor track');
  // ◄ ► move the note by a semitone.
  h.press('arrowRight'); h.calls.length = 0; h.pad(16); await tick();
  assert.equal(h.sent('note.on')[0].payload.note, on[0].note + 1, '► raises the note a semitone');
  h.press('arrowLeft');
  // Recording (stopped): the pad enters its velocity at the cursor.
  state.row = 2; h.ST.setNoteRecording(true); await tick();
  h.pad(4); await tick(); h.advance(200); await tick();
  assert.equal(h.ST.pattern().steps[0][2]?.velocity, 32, 'recorded with the pad velocity');
  h.ST.setNoteRecording(false); await tick();
  h.shift('fixedVel'); assert.equal(MK3.state.padMode, 'keyboard', 'back to KEYBOARD');
  // Any pad mode button also leaves it.
  h.shift('fixedVel'); h.press('step'); assert.equal(MK3.state.padMode, 'step');
});

test('AUTO: tap toggles write and stays; held from Read with a turn writes only while held; Shift cycles', async () => {
  const h = session(), { AR, MK3 } = h;
  AR.setMode('read', true);
  h.press('auto'); assert.notEqual(AR.mode, 'read', 'tap enters the last write mode'); const mode = AR.mode;
  h.press('auto'); assert.equal(AR.mode, 'read', 'tap again returns to Read');
  h.press('auto', true); assert.equal(AR.mode, mode, 'press enters write at once');
  h.input({ type: 'encoder', index: 2, delta: .05 });
  h.release('auto'); await tick();
  assert.equal(AR.mode, 'read', 'release after turning returns to Read');
  assert.match(MK3.state.flash.text, /written/);
  // Held without turning behaves like a tap (stays in write).
  h.press('auto', true); h.advance(800); h.release('auto'); assert.equal(AR.mode, mode);
  h.press('auto'); assert.equal(AR.mode, 'read');
  h.shift('auto'); assert.notEqual(AR.mode, 'read', 'Shift + AUTO cycles the write mode');
  AR.setMode('read', true);
});

test('KEYBOARD while lit opens the key & scale page; KEYBOARD again closes it', () => {
  const h = session(), { MK3 } = h;
  h.press('keyboard'); assert.equal(MK3.page().id, 'notes', 'first press only selects the pad mode');
  h.press('keyboard'); assert.equal(MK3.page().id, 'scale');
  h.press('keyboard'); assert.equal(MK3.page().id, 'notes');
  assert.equal(MK3.state.padMode, 'keyboard');
});

test('CHANNEL · Track sets voice mode and choke group of a sample track on encoders 7 and 8 (one Undo step each)', async () => {
  const h = session(), { MK3, ST, state } = h;
  ST.mutate('type', p => { p.instruments[0].type = 'sample'; p.tracks[0].chokeGroup = 3; });
  h.press('channel'); h.press('display2');
  let page = MK3.page();
  assert.equal(page.knobs[6]?.label, 'Voice'); assert.equal(page.knobs[7]?.label, 'Choke');
  assert.equal(page.knobs[7].value, 'Group 3');
  const undo = state.undo.length;
  h.input({ type: 'touch', target: 'encoder7', touched: true }); h.input({ type: 'encoder', index: 6, delta: .08 }); h.input({ type: 'touch', target: 'encoder7', touched: false });
  h.input({ type: 'touch', target: 'encoder8', touched: true }); h.input({ type: 'encoder', index: 7, delta: .08 }); h.input({ type: 'touch', target: 'encoder8', touched: false });
  h.advance(500); await tick();
  assert.equal(state.project.tracks[0].voiceMode, 'mono');
  assert.equal(state.project.tracks[0].chokeGroup, 4);
  assert.equal(state.undo.length, undo + 2);
  assert.equal(state.project.tracks[1].voiceMode, undefined, 'other tracks keep theirs');
  // A synth track has neither.
  ST.mutate('type', p => { p.instruments[0].type = 'synth'; });
  page = MK3.page(); assert.equal(page.knobs[6], null); assert.equal(page.knobs[7], null);
});

(async () => {
  for (const t of tests) { await t.fn(); console.log('PASS ' + t.name); }
  console.log(`${tests.length}/${tests.length} MK3 workflow groups passed`);
})().catch(error => { console.error(error); process.exitCode = 1; });
