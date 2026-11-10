'use strict';
// Pattern view navigation: the left pane (instruments over the pattern matrix), song scroll in the pattern grid,
// the automation panel under the grid (row grid, linked rows, drawing across song slots) and the Automation view's
// lane stack. Run: node tests/pattern_navigation_tests.js
const assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) {
      const classes = new Set(), attrs = {}, listeners = {};
      elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: {}, classes, attrs, listeners, scrolls: 0,
        classList: { toggle(c, on) { (on ?? !classes.has(c)) ? classes.add(c) : classes.delete(c); }, remove(c) { classes.delete(c); }, add(c) { classes.add(c); }, contains: c => classes.has(c) },
        focus() { document.activeElement = this; }, select() {}, addEventListener(type, fn) { listeners[type] = fn; }, removeEventListener() {},
        setAttribute(name, value) { attrs[name] = String(value); }, removeAttribute(name) { delete attrs[name]; }, scrollIntoView() { this.scrolls++; },
        closest: () => null, querySelector: () => null, querySelectorAll: () => [] });
    }
    return elements.get(id);
  };
  const lists = new Map();
  const document = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: q => lists.get(q) || [],
    getElementById: id => element('#' + id), addEventListener() {}, createElement: tag => element(tag) };
  const window = { addEventListener() {}, STHostTransport: hostStub({ post() {} }) };
  const toasts = [];
  const { ST, app, context } = web.app({ document, window, requestAnimationFrame: fn => fn(), setTimeout: () => 1, clearTimeout() {}, innerWidth: 1280, innerHeight: 800 });
  web.load(context, 'song-model.js');
  web.load(context, 'automation-song.js');
  web.load(context, 'song-sequence.js');
  const toast = app.toast;
  app.toast = (message, ...rest) => { toasts.push(message); return toast(message, ...rest); };
  const state = ST.state;
  state.project = ST.demoProject();
  state.project.patterns[0].name = 'Intro'; state.project.patterns[1].name = 'Verse';
  state.project.sequence = [0, 0, 1];
  state.view = 'pattern';
  state.track = 0; state.row = 0; state.sequence = 0; state.selection = null;
  return { ST, app, context, document, state, element, lists, toasts, html: () => app.renderPattern(), p: () => state.project };
}
const plain = value => JSON.parse(JSON.stringify(value));
const key = (code, extra = {}) => ({ key: code, preventDefault() { this.prevented = true; }, stopPropagation() {}, ...extra });
const count = (html, pattern) => (html.match(pattern) || []).length;
const tests = [];
const test = (name, fn) => tests.push([name, fn]);

// ---- Left pane and pattern matrix ---------------------------------------------------------
test('the Pattern view has a left pane: instruments on top, the pattern matrix below', () => {
  const h = harness(), html = h.html();
  assert.match(html, /class="workspace has-left-pane"/);
  assert.match(html, /<section class="left-pane-instruments"[\s\S]*data-instrument="0"[\s\S]*<section class="left-pane-patterns"/);
  assert.equal(count(html, /data-instrument="/g), h.p().instruments.length, 'every instrument is listed');
  assert.match(html, /role="tablist" aria-label="Pattern list"/);
  assert.match(html, /id="matrix-tab-song"[^>]*aria-selected="true"/, 'Current song is the default tab');
  assert.match(html, /id="pattern-matrix" role="listbox" tabindex="0"/);
  // Current song: one block per run, with its repeat count, in the song tab's block look.
  assert.equal(count(html, /data-matrix-block="/g), 2);
  assert.match(html, /class="song-block matrix-block current selected[^"]*" id="matrix-block-0"[\s\S]*?×2/);
  h.ST.setPatternMatrixTab('all');
  const all = h.html();
  assert.match(all, /id="matrix-tab-all"[^>]*aria-selected="true"/);
  assert.equal(count(all, /data-matrix-pattern="/g), h.p().patterns.length);
  assert.match(all, /id="matrix-pattern-1"[^>]*title="Verse · 64 rows · 1× in the song"/);
  assert.equal(h.state.undo.length, 0, 'switching tabs is not an edit');
});

test('Current song edits the song: add, duplicate, clone, remove and reorder blocks, one undo step each', () => {
  const h = harness(), p = h.p, undo = () => h.state.undo.length;
  h.ST.patternMatrixAction('duplicate');
  assert.deepEqual(plain(p().sequence), [0, 0, 0, 0, 1], '⌘D duplicates the block (more repeats of a run)');
  h.state.sequence = 4;
  h.ST.patternMatrixAction('up');
  assert.deepEqual(plain(p().sequence), [1, 0, 0, 0, 0]);
  assert.equal(p().activePattern, 1, 'the grid stays on the moved block');
  const before = p().patterns.length;
  h.ST.patternMatrixAction('add');
  assert.equal(p().patterns.length, before + 1);
  assert.equal(p().sequence[1], p().patterns[before].id, 'a new empty pattern lands after the block');
  assert.equal(p().activePattern, p().patterns[before].id);
  h.ST.patternMatrixAction('remove');
  assert.deepEqual(plain(p().sequence), [1, 0, 0, 0, 0]);
  h.state.sequence = 1;
  h.ST.patternMatrixAction('clone');
  assert.equal(p().sequence.length, 6);
  assert.match(p().patterns.find(x => x.id === p().sequence[5]).name, /copy/, 'clone gives the block its own pattern');
  assert.equal(undo(), 5);
  h.ST.perform('undo');
  assert.deepEqual(plain(p().sequence), [1, 0, 0, 0, 0]);
});

test('All patterns: choose, reorder, add to the song and delete patterns', () => {
  const h = harness(), p = h.p;
  h.ST.setPatternMatrixTab('all');
  h.app.bindPatternMatrix();
  const list = h.element('#pattern-matrix');
  list.onkeydown(key('ArrowDown'));
  assert.equal(p().activePattern, 1, '↓ chooses the next pattern');
  list.onkeydown(key('ArrowUp', { altKey: true }));
  assert.deepEqual(plain(p().patterns.map(x => x.id)), [1, 0], '⌥↑ reorders the pattern list');
  assert.deepEqual(plain(p().sequence), [0, 0, 1], 'the song is untouched');
  list.onkeydown(key('Enter', { shiftKey: true }));
  assert.deepEqual(plain(p().sequence), [0, 0, 1, 1], '⇧Return adds the pattern after the current block (here it joins the next run)');
  const n = p().patterns.length;
  list.onkeydown(key('n'));
  assert.equal(p().patterns.length, n + 1, 'N adds a pattern');
  list.onkeydown(key('Delete'));
  assert.equal(p().patterns.length, n, '⌫ deletes it');
  const escape = key('Escape');
  list.onkeydown(escape);
  assert.equal(escape.prevented, true);
  assert.equal(h.document.activeElement, h.element('#matrix-tab-all'), 'Esc returns to the tabs');
});

test('Current song keys follow the arranger: I, + and −, ⌥↓, and letters yield to the keyboard piano', () => {
  const h = harness(), p = h.p;
  h.app.bindPatternMatrix();
  const list = h.element('#pattern-matrix');
  list.onkeydown(key('+'));
  assert.deepEqual(plain(p().sequence), [0, 0, 0, 1], '+ adds a repeat');
  list.onkeydown(key('-'));
  assert.deepEqual(plain(p().sequence), [0, 0, 1], '− removes one');
  list.onkeydown(key('ArrowDown', { altKey: true }));
  assert.deepEqual(plain(p().sequence), [1, 0, 0]);
  list.onkeydown(key('ArrowDown'));
  assert.equal(h.state.sequence, 1, '↓ chooses the next block');
  h.state.keyboardPlay = true;
  const typed = key('i');
  // An on-handler that returned false would cancel the key before the piano (the global handler) sees it.
  assert.notEqual(list.onkeydown(typed), false); assert.notEqual(typed.prevented, true);
  assert.deepEqual(plain(p().sequence), [1, 0, 0], 'I plays a note while the keyboard piano is on');
});

// ---- Song scroll ----------------------------------------------------------------------------
test('Song scroll renders the song as one list: slot headers, a live slot and read-only context rows', () => {
  const h = harness(), p = h.p, length = p().patterns[0].length;
  assert.equal(h.ST.songScrollOn(), false);
  h.ST.perform('songScroll');
  assert.equal(h.ST.songScrollOn(), true);
  assert.deepEqual(plain(p().viewState).songScroll, true, 'a saved view preference');
  assert.equal(h.state.undo.length, 0, 'not an undo step');
  const html = h.html();
  assert.equal(count(html, /class="song-scroll-head/g), 3, 'one header per slot');
  assert.match(html, /data-slot-head="1"[\s\S]*?repeat 2\/2/);
  assert.equal(count(html, /<tr data-row="\d+" data-slot="0" class/g), length, 'the live slot has ordinary rows');
  assert.equal(count(html, /data-context /g), 2 * length, 'the other slots are context rows');
  const tracks = count(html.slice(html.indexOf('data-slot="0"'), html.indexOf('data-slot-head="1"')), /data-cell="/g);
  assert.equal(count(html, /data-cell="/g), tracks, 'only the live slot has editable cells');
  assert.ok(count(html, /data-context-cell="/g) > 0);
  assert.match(html, /id="song-scroll" class="active" aria-pressed="true"/);
});

test('Song scroll: the cursor runs across slot edges, ⌘] steps slots, and Follow tracks the playing slot', () => {
  const h = harness(), p = h.p, last = p().patterns[0].length - 1;
  h.ST.setSongScroll(true);
  h.state.row = last;
  h.app.moveGrid(0, 1);
  assert.deepEqual(plain([h.state.sequence, p().activePattern, h.state.row]), [1, 0, 0], '↓ past the end enters the next slot (a repeat)');
  h.app.moveGrid(0, 16 * 4);
  assert.deepEqual(plain([h.state.sequence, p().activePattern, h.state.row]), [2, 1, 0], 'PageDowns continue into the next pattern');
  h.app.moveGrid(0, -1);
  assert.deepEqual(plain([h.state.sequence, p().activePattern, h.state.row]), [1, 0, last], '↑ at the top returns to the previous slot');
  h.app.moveGrid(0, 1, 0, true);
  assert.equal(h.state.sequence, 1, 'a selection stays inside its pattern');
  h.state.selection = null;
  h.ST.perform('previousPattern');
  assert.deepEqual(plain([h.state.sequence, h.state.row]), [0, last], '⌘[ steps a slot and keeps the row');
  h.state.follow = true;
  h.app.updatePlayhead({ playing: true, songMode: true, sequence: 2, pattern: 1, row: 3 });
  assert.deepEqual(plain([h.state.sequence, p().activePattern]), [2, 1], 'Follow makes the playing slot current');
  h.ST.setSongScroll(false);
  h.state.row = last; h.state.sequence = 0; p().activePattern = 0;
  h.app.moveGrid(0, 1);
  assert.deepEqual(plain([h.state.sequence, h.state.row]), [0, last], 'off: the cursor stops at the pattern end');
});

test('Song scroll caps long songs and notes the slots it leaves out', () => {
  const h = harness(), p = h.p;
  p().sequence = Array.from({ length: 40 }, (_, i) => i % 2);
  h.state.sequence = 20; p().activePattern = 0;
  h.ST.setSongScroll(true);
  const html = h.html(), rows = count(html, /<tr data-row="/g);
  assert.ok(rows <= 1500 && rows >= 1400, `${rows} rows rendered`);
  assert.match(html, /earlier slots · ⌘\[ steps back/);
  assert.match(html, /later slots · ⌘\] steps on/);
  p().sequence = [1];
  p().activePattern = 0;
  assert.match(h.html(), /This pattern is not in the song/);
});

// ---- Automation panel under the grid -------------------------------------------------------
test('⌥⇧A opens the automation panel: a saved view preference with one row line per pattern row', () => {
  const h = harness(), p = h.p;
  assert.equal(h.html().includes('id="auto-panel"'), false, 'hidden by default');
  h.ST.perform('automationPanel');
  assert.equal(h.ST.automationPanelScope(), 'pattern');
  assert.equal(h.state.undo.length, 0);
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .2 }, { row: 32, value: .8 }] }];
  h.state.row = 5;
  const html = h.html(), length = p().patterns[0].length;
  assert.match(html, /<section class="auto-panel" id="auto-panel"/);
  assert.equal(count(html, /class="auto-row(-beat|-bar)?"/g), length, 'a grid line per row');
  assert.equal(count(html, /class="auto-row-bar"/g), length / 16);
  assert.match(html, /class="auto-row-cursor" x="(\d+)"/, 'the grid cursor row is drawn');
  const px = Number(/data-px="(\d+)"/.exec(html)[1]);
  assert.equal(Number(/class="auto-row-cursor" x="(\d+)"/.exec(html)[1]), 5 * px);
  assert.doesNotMatch(html, /class="pattern-foot"/, 'the panel takes the hint line\'s place');
  assert.match(html, /id="auto-panel-toggle" class="active" aria-pressed="true"/);
  h.ST.setAutomationPanel(null);
  assert.equal(h.ST.automationPanelScope(), null);
  assert.match(h.html(), /class="pattern-foot"/);
});

test('Song scope lays the song out horizontally with pattern headers and holds where a slot has no lane', () => {
  const h = harness(), p = h.p;
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .2 }, { row: 32, value: .8 }] }];
  p().patterns[1].automation = [];
  h.ST.setAutomationPanel('song');
  const html = h.html(), total = p().sequence.reduce((n, id) => n + p().patterns.find(x => x.id === id).length, 0);
  assert.equal(count(html, /class="auto-panel-slot[ "]/g), 3, 'one header per slot');
  assert.match(html, /<b>P01<\/b> Intro[\s\S]*<b>P01<\/b> Intro[\s\S]*<b>P02<\/b> Verse/, 'pattern names in song order');
  assert.equal(count(html, /class="auto-row(-beat|-bar|-slot)?"/g), total, 'the whole song, row by row');
  assert.equal(count(html, /class="auto-row-slot"/g), 2, 'slot boundaries');
  assert.equal(count(html, /class="studio-song-hold"/g), 1, 'the lane-less slot holds the carried value');
  assert.match(html, /style="width:\d+px"/, 'scrolls horizontally from start to end');
});

test('Drawing across a slot boundary writes each slot\'s pattern lane in one undo step', () => {
  const h = harness(), p = h.p;
  p().sequence = [0, 1];
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .2 }, { row: 63, value: .2 }] }];
  p().patterns[1].automation = [];
  h.ST.setAutomationPanel('song');
  const ref = h.app.automationPanelRefs()[0], info = { min: 0, max: 1, step: .01, value: 0, format: String };
  const list = h.app.automationLaneSlots(ref, info, 'song');
  const stroke = new Map([[62, .5], [63, .6], [64, .7], [65, .8]]);
  const undo = h.state.undo.length;
  const touched = h.ST.writeAutomation(ref, info, list, { set: stroke, clear: [62, 65] });
  assert.deepEqual(plain(touched), [0, 1]);
  assert.equal(h.state.undo.length, undo + 1, 'one undo step');
  assert.deepEqual(plain(p().patterns[0].automation[0].points), [{ row: 0, value: .2 }, { row: 62, value: .5 }, { row: 63, value: .6 }], 'the stroke replaces the drawn rows');
  assert.deepEqual(plain(p().patterns[1].automation[0].points), [{ row: 0, value: .7 }, { row: 1, value: .8 }], 'rows continue into the next slot\'s pattern');
  assert.equal(p().patterns[1].automation[0].track, 0, 'a new lane matches the existing lane');
  h.ST.writeAutomation(ref, info, h.app.automationLaneSlots(ref, info, 'song'), { remove: new Set([62]) });
  assert.deepEqual(plain(p().patterns[0].automation[0].points.map(x => x.row)), [0, 63], '⌥-click removes a point');
});

test('A stroke on a repeated pattern says it edits every repeat', () => {
  const h = harness(), p = h.p;
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [] }];
  h.ST.setAutomationPanel('song');
  const ref = h.app.automationPanelRefs()[0], info = { min: 0, max: 1, step: .01, value: 0, format: String };
  h.ST.writeAutomation(ref, info, h.app.automationLaneSlots(ref, info, 'song'), { set: new Map([[70, .5]]) });
  assert.deepEqual(plain(p().patterns[0].automation[0].points), [{ row: 6, value: .5 }], 'slot 2 is pattern 0 again, row 6');
  assert.match(h.toasts.at(-1), /Edited all 2 repeats of P01/);
});

test('Rows are linked both ways: grid hover lights the panel column, panel hover lights the grid row', () => {
  const h = harness(), p = h.p;
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .2 }] }];
  h.ST.setAutomationPanel('pattern');
  const area = h.element('#auto-panel-envelope'), rect = h.element('rect.link');
  area.dataset.px = '10';
  area.querySelector = selector => selector === '.auto-row-link' ? rect : null;
  h.app.automationPanelHover(-1, 12);
  assert.deepEqual(plain([rect.attrs.visibility, rect.attrs.x]), ['visible', '120']);
  h.app.automationPanelHover(-1, null);
  assert.equal(rect.attrs.visibility, 'hidden');
  // Panel → grid: the panel's pointer row tints the live pattern's grid row (tr.row-link).
  const svg = h.element('svg'), tr = h.element('#tracker tr[data-row="7"]:not([data-context])');
  svg.getBoundingClientRect = () => ({ left: 0, top: 0 });
  area.querySelector = selector => selector === 'svg' ? svg : selector === '.auto-row-link' ? rect : null;
  h.app.bindAutomationPanel();
  svg.onmousemove({ clientX: 75, clientY: 20 });
  assert.equal(tr.classes.has('row-link'), true);
  assert.equal(rect.attrs.x, '70');
});

test('The panel lane edits from the keyboard at the grid cursor row', () => {
  const h = harness(), p = h.p;
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .5 }] }];
  h.ST.setAutomationPanel('pattern');
  const area = h.element('#auto-panel-envelope'), svg = h.element('svg');
  area.dataset.px = '10';
  area.querySelector = selector => selector === 'svg' ? svg : null;
  h.app.bindAutomationPanel();
  area.onkeydown(key('ArrowRight', { shiftKey: true }));
  assert.equal(h.state.row, 4, '⇧→ moves the shared cursor by a beat');
  area.onkeydown(key('Enter'));
  const lane = () => p().patterns[0].automation[0];
  assert.deepEqual(plain(lane().points.map(x => x.row)), [0, 4], 'Enter adds a point on the held value');
  const before = lane().points[1].value;
  area.onkeydown(key('ArrowUp'));
  assert.ok(lane().points[1].value > before, '↑ raises it');
  area.onkeydown(key('Delete'));
  assert.deepEqual(plain(lane().points.map(x => x.row)), [0]);
  area.onkeydown(key('Escape'));
  assert.equal(h.document.activeElement, h.element('#tracker'), 'Esc returns to the grid');
});

test('A song edit in the matrix that shows another pattern leaves the grid selection behind', () => {
  const h = harness();
  Object.assign(h.state, { track: 2, row: 12, selection: { track: 2, row: 10 } });
  h.ST.patternMatrixAction('add');   // inserts a new empty pattern after the block and shows it
  assert.notEqual(h.p().activePattern, 0);
  assert.equal(h.ST.selectedBounds(), null, 'rows 10–12 of the previous pattern are not selected in the new one');
  // An edit that keeps the pattern keeps the selection.
  Object.assign(h.state, { selection: { track: 2, row: 1 }, row: 3 });
  h.ST.patternMatrixAction('more');
  assert.deepEqual(plain(h.state.selection), { track: 2, row: 1 });
});

test('Keys the matrix list and the panel lane do not use are left to the global key handler', () => {
  // An on-handler that returns false cancels the event, and the global handler ignores cancelled keys (F6, ⌘1…).
  const h = harness(), p = h.p;
  p().patterns[0].automation = [{ id: 'a', track: 0, target: 'cutoff', name: 'Cutoff', interpolation: 'linear', points: [{ row: 0, value: .5 }] }];
  h.ST.setAutomationPanel('pattern');
  const list = h.element('#pattern-matrix'), area = h.element('#auto-panel-envelope'), svg = h.element('svg');
  area.dataset.px = '10';
  area.querySelector = selector => selector === 'svg' ? svg : null;
  h.app.bindPatternMatrix();
  h.app.bindAutomationPanel();
  for (const node of [list, area]) for (const unused of [key('F6'), key('2', { metaKey: true }), key('Tab'), key('k', { metaKey: true })]) {
    assert.notEqual(node.onkeydown(unused), false);
    assert.notEqual(unused.prevented, true);
  }
  const used = key('ArrowRight');
  area.onkeydown(used);
  assert.equal(used.prevented, true, 'keys the lane uses are still taken');
});

test('The playing song block is marked in the pattern matrix on transport ticks', () => {
  const h = harness();
  const blocks = [0, 1].map(index => Object.assign(h.element('#matrix-block-' + index), { dataset: { matrixBlock: String(index) } }));
  h.lists.set('[data-matrix-block]', blocks);
  h.state.follow = false;
  h.state.playing = true;
  h.app.updatePlayhead({ playing: true, songMode: true, sequence: 2, pattern: 1, row: 3 });   // sequence [0, 0, 1]: slot 2 is block 1
  assert.deepEqual(blocks.map(node => node.classes.has('playing')), [false, true]);
  h.app.updatePlayhead({ playing: true, songMode: false, sequence: 2, pattern: 1, row: 3 });
  assert.deepEqual(blocks.map(node => node.classes.has('playing')), [false, false], 'Pattern playback marks no song block');
});

test('Names, modes and colours from a project file cannot add markup to the Pattern view', () => {
  const h = harness(), p = h.p(), bad = name => `"><img src=x onerror="${name}()">`;
  Object.assign(p.tracks[0], { routingID: 'r0', color: bad('trackColour') });
  Object.assign(p.tracks[1], { routingID: 'r1', color: bad('slimColour') });
  p.viewState = { collapsedTracks: ['r1'] };
  Object.assign(p.instruments[0], { samplePath: '/a.wav', mode: bad('mode') });
  Object.assign(p.instruments[1], { type: 'au', plugin: { format: bad('format') } });
  p.patterns[0].color = bad('patternColour');
  for (const scope of [null, 'song']) {
    h.ST.setAutomationPanel(scope);
    h.ST.setSongScroll(scope === 'song');
    const html = h.html();
    assert.equal(/<img/.test(html), false, 'no injected element');
    assert.match(html, /data-track-slim/);
  }
  const M = h.context.window.STSongModel;
  assert.equal(M.patternColor(p, 0), M.COLORS[0], 'a colour that is not a hex colour falls back to the pattern’s default');
  p.patterns[0].color = '#AbC123';
  assert.equal(M.patternColor(p, 0), '#AbC123');
});

// ---- Automation view: lane stack -----------------------------------------------------------
test('The Automation view shows every lane as a box; the parameter list scrolls to the chosen one', () => {
  const h = harness(), p = h.p;
  web.load(h.context, 'studio.js');
  const studio = h.context.window.STStudio;
  p().patterns[0].automation = ['volume', 'pan', 'cutoff'].map((target, i) => ({ id: 'l' + i, track: 0, target, name: target, interpolation: 'linear',
    points: [{ row: 0, value: .2 }, { row: 8 * (i + 1), value: .7 }] }));
  h.state.view = 'automation'; h.state.track = 0; h.state.selectedLane = 0;
  let html = studio.renderAutomation();
  assert.match(html, /class="studio-lane-stack" id="automation-lane-stack" data-scroll="automation-lanes" role="listbox"/);
  assert.equal(count(html, /class="studio-lane-box[ "]/g), 3, 'one box per automated parameter');
  assert.equal(count(html, /class="auto-lane-svg"/g), 3, 'each box draws its lane');
  assert.match(html, /id="automation-lane-0" role="option" aria-selected="true" tabindex="0"/);
  assert.equal(count(html, /id="automation-envelope"/g), 1, 'the selected lane is edited in one docked editor');
  // Choosing an automated parameter selects its box and scrolls to it; the list keeps focus. Enter enters the envelope.
  const button = h.element('#automation-target-cutoff');
  Object.assign(button.dataset, { chooseTarget: 'cutoff', targetName: 'cutoff' });
  button.id = 'automation-target-cutoff';
  h.lists.set('[data-choose-target]', [button]);
  studio.bindAutomation();
  h.document.activeElement = button;
  button.listeners.click();
  assert.equal(h.state.selectedLane, 2);
  assert.equal(h.document.activeElement, button, 'a click keeps focus in the parameter list');
  assert.ok(h.element('#automation-lane-2').scrolls > 0, 'scrolled into view');
  assert.equal(h.state.project.patterns[0].automation.length, 3, 'no lane was created or replaced');
  html = studio.renderAutomation();
  assert.match(html, /id="automation-lane-2" role="option" aria-selected="true"/);
  assert.equal(count(html, /class="studio-lane-box[ "]/g), 3, 'the other lanes stay in view');
  const enter = key('Enter');
  button.listeners.keydown(enter);
  assert.equal(enter.prevented, true);
  assert.equal(h.document.activeElement, h.element('#automation-envelope'), 'Enter enters the lane envelope');
});

(async () => {
  for (const [name, fn] of tests) {
    try { await fn(); console.log('PASS', name); }
    catch (error) { console.error('FAIL', name); console.error(error); process.exitCode = 1; }
  }
})();
