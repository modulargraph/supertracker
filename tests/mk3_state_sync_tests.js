'use strict';
// Controller pages, banks and output stay in sync with the project: shrinking collections, selection made in
// the app, strip scrubbing, per-screen updates and LED colours that survive the hardware's 4-level palette.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const shifted = (h, name) => { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); };
const tests = [];

tests.push(['STEP page stays inside a pattern that got shorter', () => {
  const h = session(); h.MK3.subscribe(() => {});
  h.press('step'); for (let i = 0; i < 3; i++) h.press('arrowRight');
  assert.equal(h.MK3.state.stepPage, 3);
  h.ST.mutate('shorten', p => { const pat = p.patterns.find(x => x.id === p.activePattern); pat.length = 16; pat.steps.forEach(steps => { steps.length = 16; }); });
  h.ST.pattern().steps[h.state.track][2] = { note: 60, instrument: 0, velocity: 100, gate: .8 };
  const pads = h.MK3.frame().leds.pads;
  assert.equal(h.MK3.state.stepPage, 0, 'page clamped to the only page');
  assert.ok(pads[2], 'the step on row 2 is lit');
  h.down(5); h.up(5);
  assert.equal(h.ST.pattern().steps[h.state.track][4]?.note >= 0, true, 'pad 5 enters row 4');
}]);

tests.push(['group bank follows a track selected in the app and clamps when tracks go away', () => {
  const h = session(); h.MK3.subscribe(() => {});
  h.ST.mutate('tracks', p => { while (p.tracks.length < 12) p.tracks.push({ ...JSON.parse(JSON.stringify(p.tracks[0])), name: 'T' + (p.tracks.length + 1), color: '#c58b69' }); });
  h.ST.normalize(h.state.project);
  h.state.track = 10; h.MK3.frame();
  assert.equal(h.MK3.state.bank, 1, 'bank shows tracks 9–16');
  const groups = h.MK3.frame().leds.groups;
  assert.equal(Math.max(...[1, 3, 5].map(i => parseInt(groups[2].slice(i, i + 2), 16))), 255, 'the selected track’s group (C) is lit brightest');
  h.press('groupA'); assert.equal(h.state.track, 8, 'group A selects track 9 in bank 2');
  shifted(h, 'arrowLeft'); assert.equal(h.MK3.state.bank, 0, 'Shift+◄ still browses banks');
  h.MK3.frame(); assert.equal(h.MK3.state.bank, 0, 'browsing without a selection change keeps the bank');
  shifted(h, 'arrowRight'); h.state.project = h.ST.demoProject(); h.ST.normalize(h.state.project); h.state.track = 0;
  h.MK3.event('project.loaded', {}); h.MK3.frame();
  assert.equal(h.MK3.state.bank, 0, 'an 8-track project shows bank 1');
  assert.ok(h.MK3.frame().leds.groups[0], 'and its groups light');
}]);

tests.push(['Shift+ARRANGER opens the pattern list on the active pattern', () => {
  const h = session();
  h.state.project.activePattern = h.state.project.patterns[1].id;
  shifted(h, 'arranger');
  assert.equal(h.MK3.page().id, 'patterns');
  assert.equal(h.MK3.page().data.selected, 1);
}]);

tests.push(['touch strip scrub seeks once per row during playback', () => {
  const h = session(); h.MK3.subscribe(() => {});
  h.state.playing = true; h.state.playbackMode = 'pattern'; h.calls.length = 0;
  h.input({ type: 'touch', target: 'strip', touched: true });
  for (let i = 0; i < 200; i++) h.input({ type: 'strip', value: .5 + i / 100000 });
  assert.equal(h.calls.filter(c => c.command === 'transport.seek').length, 1, 'one seek for one row');
  h.input({ type: 'strip', value: .9 });
  assert.equal(h.calls.filter(c => c.command === 'transport.seek').length, 2, 'a new row seeks again');
  h.input({ type: 'touch', target: 'strip', touched: false }); h.input({ type: 'touch', target: 'strip', touched: true }); h.input({ type: 'strip', value: .9 });
  assert.equal(h.calls.filter(c => c.command === 'transport.seek').length, 3, 'a new touch on the same row seeks');
  h.state.playing = false;
}]);

tests.push(['only the screen that changed is sent', async () => {
  const h = session(); h.state.native = true;
  h.MK3.event('mk3.status', { connected: true, screens: true, exclusive: true }); h.advance(60); await tick();
  const screens = () => h.calls.filter(c => c.command === 'mk3.screen').map(c => Object.keys(c.payload).sort().join(','));
  assert.deepEqual(screens(), ['left,right'], 'the first frame draws both screens');
  h.calls.length = 0; h.input({ type: 'touch', target: 'encoder1', touched: true }); h.advance(60); await tick();
  assert.deepEqual(screens(), ['left'], 'touching encoder 1 redraws only the left screen');
  h.calls.length = 0; h.MK3.event('mk3.status', { connected: true, screens: false, exclusive: false }); h.MK3.event('mk3.status', { connected: true, screens: true, exclusive: true }); h.advance(60); await tick();
  assert.deepEqual(screens(), ['left,right'], 'reopened screens get both sides again');
  h.MK3.event('mk3.status', { connected: false });
}]);

tests.push(['LED colours reach full brightness and keep scaled levels', () => {
  const h = session(); h.MK3.subscribe(() => {});
  const leds = h.MK3.frame().leds, value = c => Math.max(...[1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16)));
  assert.equal(value(leds.groups[0]), 255, 'selected group at full value although the track colour is muted');
  assert.equal(value(leds.groups[1]), 64, 'unselected group at a quarter');
  const hue = c => { const [r, g, b] = [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16)); return Math.round(Math.atan2(Math.sqrt(3) * (g - b), 2 * r - g - b) * 180 / Math.PI); };
  assert.ok(Math.abs(hue(leds.groups[0]) - hue(h.state.project.tracks[0].color)) <= 2, 'hue preserved');
  h.press('step'); assert.ok(h.MK3.frame().leds.pads.some(c => c === '#0f0f0f'), 'beat markers on empty steps');
}]);

tests.push(['LOCK never edits a parameter that cannot be locked', () => {
  const h = session(); h.MK3.subscribe(() => {});
  const index = h.state.instrument, inst = h.state.project.instruments[index]; inst.type = 'synth';
  h.ST.pattern().steps[h.state.track][h.state.row] = { note: 60, instrument: index, velocity: 100, gate: .8 };
  h.press('channel'); h.press('arrowRight');
  assert.equal(h.MK3.page().knobs[0].label, 'Attack');
  const attack = inst.attack, undo = h.state.undo.length;
  h.input({ type: 'button', name: 'lock', pressed: true });
  h.input({ type: 'encoder', index: 0, delta: .2 });
  assert.match(h.MK3.screenModel().flash || '', /no step lock/, 'the screen says why');
  h.advance(1000);
  assert.equal(h.state.project.instruments[index].attack, attack, 'attack unchanged');
  assert.equal(h.state.undo.length, undo, 'no undo step');
  h.press('arrowLeft'); assert.equal(h.MK3.page().knobs[4].label, 'Cutoff');
  h.input({ type: 'encoder', index: 4, delta: -.2 }); h.advance(1000);
  h.input({ type: 'button', name: 'lock', pressed: false });
  const step = h.ST.pattern().steps[h.state.track][h.state.row];
  assert.equal(step.fx1?.type, 'cutoff', 'a lockable parameter still writes its lock');
}]);

tests.push(['page content never runs into the encoder labels', () => {
  // KNOBS_Y = 204 in mk3-screens.js: content ends above it, encoder labels start at 211.
  const overflow = (ops, where) => { for (const op of ops) {
    if (op.op === 'text' && op.y < 210 && op.y + op.size > 204) assert.fail(`${where}: text "${op.text}" at y ${op.y}`);
    if (['rect', 'bar', 'frame'].includes(op.op) && op.y < 203 && op.y + op.h > 205) assert.fail(`${where}: ${op.op} at y ${op.y}…${op.y + op.h}`);
  } };
  const check = (h, where) => { h.advance(50); const { left, right } = h.MK3.frame().screens; overflow(left, where + ' left'); overflow(right, where + ' right'); };
  const shifted_ = (h, n) => { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(n); h.input({ type: 'button', name: 'shift', pressed: false }); };
  for (const kit of [false, true]) {
    // kit: the cursor track is a member of a track group (Group level on CHANNEL / PLUG-IN).
    const h = kit ? groupSession() : session();
    h.MK3.subscribe(() => {}); h.state.project.instruments[h.state.instrument].type = 'synth';
    for (const screen of ['notes', 'macro', 'channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'settings']) {
      h.press(screen); check(h, `${kit ? 'group ' : ''}${screen}`);
      h.press('encoderPush'); check(h, `${kit ? 'group ' : ''}${screen} pushed`);
      for (const page of ['display2', 'display3', 'display4']) { h.press(page); check(h, `${kit ? 'group ' : ''}${screen} ${page}`); }
      shifted_(h, screen); check(h, `${kit ? 'group ' : ''}Shift ${screen}`);
    }
  }
}]);

(async () => { let failures = 0; for (const [name, run] of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length - failures}/${tests.length} MK3 state sync checks passed`); if (failures) process.exitCode = 1; })();
