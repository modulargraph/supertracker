'use strict';
// MK3 redesign phase 1 (docs/MK3_DESIGN_STUDY.md §9): Home across both screens, no accidental overview, one page
// grammar, encoder modes instead of full-screen overlays, mixer selection, PATTERN page, FOLLOW drives STEP.
// Synthetic 'mk3.input' events drive the real app and controller modules; no device is opened.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session, groupSession } = require('./helpers/mk3-session');
const tests = []; const test = (name, run) => tests.push({ name, run });
const COL = 120;
// Arrays from the app's VM context have another prototype: compare by value.
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
function setup(group = false) {
  const h = group ? groupSession() : session();
  h.MK3.subscribe(() => {});
  h.hold = (name, down) => h.input({ type: 'button', name, pressed: down });
  h.shifted = name => { h.hold('shift', true); h.press(name); h.hold('shift', false); };
  h.touch = (target, touched) => h.input({ type: 'touch', target, touched });
  h.texts = side => h.MK3.frame().screens[side].filter(op => op.op === 'text');
  return h;
}

// ---- 1. Home: tracker across both screens -----------------------------------------------------------------
test('Home shows eight 120 px track columns under the display buttons, four per screen, eleven rows', () => {
  const h = setup(); h.state.row = 4; const page = h.MK3.page(), names = h.state.project.tracks.map(t => t.name);
  assert.equal(page.id, 'notes'); assert.equal(page.title, 'Home');
  same(Array.from(page.data.grid.tracks, t => t.index), [0, 1, 2, 3, 4, 5, 6, 7]);
  assert.equal(page.data.grid.rows.length, 11, 'eleven rows');
  const model = h.MK3.screenModel();
  same(model.buttons.map(b => b.label), names, 'display buttons name the columns below them');
  assert.ok(model.buttons[0].on && model.buttons.slice(1).every(b => !b.on), 'selected column lit');
  const { left, right } = h.MK3.frame().screens;
  // Each column's notes are drawn inside its 120 px column, tracks 1–4 left and 5–8 right.
  for (const [side, ops, offset] of [['left', left, 0], ['right', right, 4]]) {
    for (let c = 0; c < 4; c++) {
      const track = offset + c, cell = page.data.grid.rows.find(r => r.cells[track].text[0] !== '···')?.cells[track];
      if (!cell) continue;
      const op = ops.find(o => o.op === 'text' && o.mono && o.text === cell.text[0] && o.y >= 48 && o.y < 204 && o.x >= c * COL && o.x < (c + 1) * COL);
      assert.ok(op, `${side} column ${c + 1} shows track ${track + 1}`);
      assert.ok(op.x + (op.width || 0) <= (c + 1) * COL, 'cell stays inside its column');
    }
  }
  assert.ok(left.some(o => o.op === 'text' && o.text === '004' && o.mono), 'row numbers on the left screen');
});

test('Home display buttons select columns; Shift or MUTE held mutes, SOLO held solos', () => {
  const h = setup();
  h.press('display6'); assert.equal(h.state.track, 5, 'display 6 selects track 6');
  h.shifted('display3'); assert.equal(h.state.project.tracks[2].mute, true, 'Shift + display mutes'); assert.equal(h.state.track, 5, 'and keeps the cursor');
  h.hold('mute', true); h.press('display3'); h.hold('mute', false); assert.equal(h.state.project.tracks[2].mute, false, 'MUTE + display unmutes');
  h.hold('solo', true); h.press('display4'); h.hold('solo', false); assert.equal(h.state.project.tracks[3].solo, true, 'SOLO + display solos');
  const model = h.MK3.screenModel(); assert.equal(model.buttons[3].solo, true); assert.equal(model.buttons[5].on, true);
});

test('Home columns follow the group bank (column n = group button n)', () => {
  const h = setup();
  while (h.state.project.tracks.length < 12) h.ST.mutate('add', p => { const t = JSON.parse(JSON.stringify(p.tracks[0])); t.id = p.tracks.length; t.routingID = 'r' + p.tracks.length; t.name = 'Extra ' + p.tracks.length; p.tracks.push(t); p.patterns.forEach(pat => pat.steps.push(Array(pat.length).fill(null))); });
  h.shifted('arrowRight'); assert.equal(h.MK3.state.bank, 1);
  same(Array.from(h.MK3.page().data.grid.tracks, t => t.index), [8, 9, 10, 11]);
  h.press('display2'); assert.equal(h.state.track, 9); h.press('groupB'); assert.equal(h.state.track, 9, 'group B = the same track as display 2');
});

test('Home encoders edit the cursor step with no blank slots; empty rows show greyed defaults', async () => {
  const h = setup(); const pat = h.ST.pattern();
  pat.steps[0][4] = { note: 60, instrument: 0, velocity: 90, gate: .8, fx1: { type: 'volume', value: .5 } }; pat.steps[0][5] = null;
  h.state.track = 0; h.state.row = 4;
  let knobs = h.MK3.page().knobs;
  same(knobs.map(k => k.label), ['Note', 'Inst', 'Velocity', 'Gate', 'FX1 VOL', 'FX2', 'Prob', 'Pattern']);
  assert.ok(knobs.every(Boolean) && !knobs.some(k => k.dim), 'a note row: every encoder live');
  assert.ok(!knobs.some(k => k.label === 'Length' || k.label === 'Octave'), 'Length moved to the PATTERN page');
  h.input({ type: 'encoder', index: 4, delta: .2 }); await h.MK3.flushEdits(); assert.ok(pat.steps[0][4].fx1.value > .5, 'encoder 5 edits FX 1');
  h.input({ type: 'encoder', index: 2, delta: .016 }); await h.MK3.flushEdits(); assert.equal(pat.steps[0][4].velocity, 92, 'encoder 3 edits velocity');
  h.state.row = 5; knobs = h.MK3.page().knobs;
  assert.ok(knobs.every(Boolean), 'no blank encoder on an empty row');
  same(knobs.filter(k => k.dim).map(k => k.label), ['Inst', 'Velocity', 'Gate', 'FX1', 'FX2', 'Prob'], 'defaults greyed out');
  assert.equal(h.MK3.screenModel().knobs[2].dim, true); assert.equal(knobs[2].value, '100');
  const undo = h.state.undo.length; h.input({ type: 'encoder', index: 2, delta: .3 }); await h.MK3.flushEdits(); assert.equal(pat.steps[0][5], null, 'a greyed encoder does not create a step'); assert.equal(h.state.undo.length, undo);
  h.input({ type: 'encoder', index: 0, delta: .1 }); await h.MK3.flushEdits(); assert.ok(pat.steps[0][5]?.note >= 0, 'turning Note creates the step');
  const labels = h.MK3.frame().screens.left.filter(op => op.op === 'text' && op.y >= 204).map(op => op.text); assert.ok(labels.includes('Note') && labels.includes('Gate'));
});

test('SELECT held turns the Home display buttons into the pattern tools', () => {
  const h = setup(); h.hold('select', true);
  same(h.MK3.page().buttons.map(b => b.label), ['Copy', 'Paste', 'Interp', 'Clear', 'Double', 'Stretch', 'Note off', 'Step FX']);
  h.state.row = 9; h.press('display7'); h.hold('select', false); assert.equal(h.ST.pattern().steps[0][9].note, -1, 'SELECT + display 7 writes a note-off');
  assert.equal(h.MK3.state.latched, null, 'a used SELECT does not latch'); assert.equal(h.MK3.page().buttons[0].label, h.state.project.tracks[0].name);
});

test('Home shows a track group as its member columns, or one column named after the group when collapsed', () => {
  const h = setup(true), group = h.group();
  same(Array.from(h.MK3.page().data.grid.tracks, t => t.index), [0, 1, 2, 3, 4, 5, 6, 7]);
  h.win.STApp.setCollapsed(group.id, true);
  const grid = h.MK3.page().data.grid;
  same(Array.from(grid.tracks, t => t.index), [0, 4, 5, 6, 7]);
  assert.equal(grid.tracks[0].collapsed, true); assert.equal(h.MK3.screenModel().buttons[0].label, 'Drums');
  h.press('display2'); assert.equal(h.state.track, 4);
  h.press('display1'); assert.equal(h.state.track, 0, 'the collapsed column selects the group\'s member');
});

// ---- 2. No accidental overview -----------------------------------------------------------------------------
test('the 4-D touch shows the overview only after 400 ms without turning (or Shift + touch)', () => {
  const h = setup(); h.state.row = 0;
  h.touch('main', true); for (let i = 0; i < 8; i++) { h.input({ type: 'main', delta: 1 }); h.advance(100); }
  assert.equal(h.MK3.page().data.overview, null, 'scrolling never swaps the right screen'); assert.equal(h.state.row, 8);
  assert.ok(h.texts('right').some(op => op.mono && /C-4|···/.test(op.text)), 'right screen still shows tracks 5–8 while scrolling');
  h.touch('main', false); h.touch('main', true); h.advance(399); assert.equal(h.MK3.page().data.overview, null);
  h.advance(2); assert.ok(h.MK3.page().data.overview, 'still touch shows it'); h.touch('main', false); assert.equal(h.MK3.page().data.overview, null);
  h.hold('shift', true); h.touch('main', true); assert.ok(h.MK3.page().data.overview); h.touch('main', false); h.hold('shift', false);
  // The pattern-position bar under the title marks the cursor while scrolling.
  h.state.row = 32; const bar = h.MK3.frame().screens.right.filter(op => op.op === 'rect' && op.y === 43 && op.h >= 3 && op.w === 4);
  assert.equal(bar.length, 1, 'cursor marker at the pattern middle sits on the right screen'); assert.ok(bar[0].x < 12);
});

// ---- 3. One page grammar ---------------------------------------------------------------------------------
test('PLUG-IN and CHANNEL in a track group: display 1–4 = Master / Group / Track / Instr., ◄ ► step pages', () => {
  const h = setup(true);
  for (const mode of ['channel', 'plugin']) {
    h.press(mode); const page = h.MK3.page();
    same(page.buttons.slice(0, 4).map(b => b.label), ['Master', 'Group', 'Track', 'Instr.']);
    assert.ok(!page.buttons.some(b => b && /Page/.test(b.label)), 'no ‹ Page / Page › buttons');
    if (page.pages?.length > 1) { const before = page.page; h.press('arrowRight'); assert.equal(h.MK3.page().page, before + 1); assert.equal(h.MK3.screenModel().title.pages.index, before + 1); h.press('arrowLeft'); }
  }
});

test('MIXER ◄ ► step Volume / Pan / Delay / Reverb pages with page dots', () => {
  const h = setup(); h.press('mixer');
  same(h.MK3.page().pages, ['Volume', 'Pan', 'Delay', 'Reverb']); assert.equal(h.MK3.page().knobs[0].label, 'Velvet bass');
  h.press('arrowRight'); assert.equal(h.MK3.state.screen, 'mixerSends'); assert.equal(h.MK3.page().data.param, 'pan'); assert.equal(h.MK3.screenModel().title.pages.index, 1);
  h.press('arrowRight'); h.press('arrowRight'); h.press('arrowRight'); assert.equal(h.MK3.page().data.param, 'reverbSend', 'stops at the last page');
  for (let i = 0; i < 4; i++) h.press('arrowLeft'); assert.equal(h.MK3.state.screen, 'mixer'); assert.equal(h.MK3.page().data.param, 'volume');
});

// ---- 4. Encoder modes instead of full-screen overlays ----------------------------------------------------
for (const [name, title] of [['volume', 'VOLUME'], ['swing', 'SWING'], ['tempo', 'TEMPO'], ['noteRepeat', 'NOTE REPEAT'], ['events', 'EVENTS']]) {
  test(`${title} keeps the current page and changes only the encoders plus a header strip`, () => {
    const h = setup(); h.state.row = 4; const home = h.MK3.page();
    h.hold(name, true); const page = h.MK3.page();
    assert.equal(page.id, name); assert.equal(page.encoderMode, true);
    assert.equal(page.data.kind, 'grid', 'the tracker stays on both screens'); assert.equal(page.data.header.title, title);
    same(Array.from(page.data.grid.tracks, t => t.index), Array.from(home.data.grid.tracks, t => t.index));
    const left = h.texts('left'), right = h.texts('right');
    assert.ok(left.some(op => op.text === title && op.y >= 180 && op.y < 204), 'mode chip in the header strip above the encoders');
    assert.ok(left.some(op => op.mono && op.text === '004'), 'grid rows still drawn');
    assert.ok(!left.some(op => op.size >= 30), 'no big full-screen value');
    assert.ok(right.some(op => op.y >= 180 && op.y < 204 && op.size <= 13), 'hint in the right header');
    if (name === 'volume') same(page.knobs.map(k => k?.label), h.state.project.tracks.map(t => t.name.slice(0, 10)), 'encoders = the bank\'s track volumes');
    if (name === 'noteRepeat') assert.ok(page.buttons.some(b => b?.label === '1/16'), 'rates on the display buttons');
    h.hold(name, false);
  });
}
test('encoder modes keep MIXER, PLUG-IN and Shift Velocity pages underneath', async () => {
  const h = setup(); h.press('mixer'); h.hold('volume', true); assert.equal(h.MK3.page().data.kind, 'mixer'); h.advance(400); h.hold('volume', false); assert.equal(h.MK3.state.latched, null);
  h.press('plugin'); h.hold('tempo', true); assert.equal(h.MK3.page().data.kind, 'plugin'); h.input({ type: 'main', delta: 2 }); h.hold('tempo', false); await h.MK3.flushEdits();
  assert.equal(h.state.project.bpm, 118, 'tempo mode edits tempo'); h.press('plugin');
  h.ST.pattern().steps[0][4] = { note: 60, instrument: 0, velocity: 90, gate: .8 }; h.state.row = 4;
  h.shifted('volume'); const page = h.MK3.page(); assert.equal(page.id, 'velocity'); assert.equal(page.data.kind, 'grid'); assert.equal(page.data.header.value, '90');
  h.input({ type: 'main', delta: 3 }); await h.MK3.flushEdits(); assert.equal(h.ST.pattern().steps[0][4].velocity, 93);
  h.press('encoderPush'); assert.equal(h.MK3.page().id, 'notes', 'push closes the mode');
});
test('EVENTS marks the rows it edits on the cursor column', () => {
  const h = setup(); h.state.row = 4; h.hold('events', true); h.MK3.state.events = new Set([2, 4]);
  const rows = h.MK3.page().data.grid.rows.filter(r => r.cells[0].event).map(r => r.row);
  same(rows, [2, 4]); assert.ok(h.MK3.frame().screens.left.some(op => op.op === 'rect' && op.color === '#5a4a22'));
  h.hold('events', false);
});

// ---- 5. Smaller moves ------------------------------------------------------------------------------------
test('MIXER display buttons select; Shift + display or MUTE held mutes, SOLO held solos', () => {
  const h = setup(); h.press('mixer');
  h.press('display4'); assert.equal(h.state.track, 3); assert.equal(h.state.project.tracks[3].mute, false, 'select, not mute');
  assert.equal(h.MK3.screenModel().buttons[3].on, true);
  h.shifted('display2'); assert.equal(h.state.project.tracks[1].mute, true); assert.equal(h.state.track, 3);
  h.hold('mute', true); h.press('display2'); h.hold('mute', false); assert.equal(h.state.project.tracks[1].mute, false);
  h.hold('solo', true); h.press('display5'); h.hold('solo', false); assert.equal(h.state.project.tracks[4].solo, true);
});
test('MIXER group strip: display button selects its member; Shift + display mutes the group bus', () => {
  const h = setup(true); h.press('mixer');
  same(h.MK3.page().data.strips.map(s => s.name).slice(0, 2), ['Drums', 'Snare']);
  h.state.track = 2; h.MK3.frame(); h.state.track = 5; h.MK3.frame();
  h.press('display1'); assert.equal(h.state.track, 2, 'the member selected last');
  h.shifted('display1'); assert.equal(h.bus().mute, true); assert.ok(h.state.project.tracks.slice(0, 4).every(t => !t.mute));
});
test('PATTERN shows the PATTERN page with Length and Lines/beat; pressing it again goes back', async () => {
  const h = setup(); h.press('mixer');
  h.press('pattern'); const page = h.MK3.page();
  assert.equal(page.id, 'pattern'); assert.equal(h.MK3.state.padMode, 'pattern');
  same(page.knobs.slice(0, 4).map(k => k.label), ['Pattern', 'Length', 'Lines/beat', 'Colour']);
  h.input({ type: 'encoder', index: 1, delta: .1 }); await h.MK3.flushEdits(); assert.ok(h.ST.pattern().length > 64, 'Length edits the pattern');
  h.input({ type: 'encoder', index: 2, delta: .08 }); await h.MK3.flushEdits(); assert.equal(h.state.project.linesPerBeat, 6, 'Lines/beat');
  h.input({ type: 'pad', pad: 2, phase: 'down', velocity: 100 }); h.input({ type: 'pad', pad: 2, phase: 'up' });
  assert.equal(h.state.project.activePattern, h.state.project.patterns[1].id); assert.equal(h.MK3.page().data.selected, 1, 'list follows the pad');
  h.press('pattern'); assert.equal(h.MK3.state.padMode, 'pad'); assert.equal(h.MK3.page().id, 'mixer', 'back to the previous screen and pad mode');
  h.press('pattern'); h.press('arranger'); assert.equal(h.MK3.page().id, 'arranger', 'a mode button leaves the PATTERN page'); assert.equal(h.MK3.state.padMode, 'pattern', 'pads stay patterns');
});
test('FOLLOW makes the STEP page follow the playhead; Shift + STEP no longer toggles it', () => {
  const h = setup(); h.press('step'); h.shifted('step'); assert.equal(h.MK3.state.padMode, 'step'); assert.equal(h.MK3.state.stepFollow, undefined);
  const pat = h.ST.pattern(); h.state.playing = true; h.state.status = { playing: true, pattern: pat.id, row: 37 };
  h.state.follow = false; h.MK3.frame(); assert.equal(h.MK3.state.stepPage, 0, 'without FOLLOW the page stays');
  h.press('follow'); assert.equal(h.state.follow, true); h.MK3.frame(); assert.equal(h.MK3.state.stepPage, 2, 'FOLLOW: page of row 37');
  h.state.playing = false; h.state.status = { playing: false };
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`${tests.length - failures}/${tests.length} MK3 phase 1 checks passed`);
  if (failures) process.exitCode = 1;
})();
