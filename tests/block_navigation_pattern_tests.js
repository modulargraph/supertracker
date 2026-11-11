'use strict';
// Block navigation on the Pattern page (block-nav.js + pane-navigation.js, phase 3): the block layout (geometry from the
// real page at 1500×940), ⌥ / ⌥⌘ moves, the grid keeping all of its keys, the track header buttons (every track and
// group reachable with arrows, Return presses), focus returning to the grid's cursor cell, and ⌘0 back to the grid.
const assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const vm = require('node:vm');
const web = require('./helpers/web');
const { createDOM } = require('./helpers/fake-dom');

const deep = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual)), expected, message);
const tests = [];
function test(name, run) { tests.push({ name, run }); }

function harness() {
  const dom = createDOM(), { document, window } = dom, calls = [];
  let stops = 0;
  window.STHostTransport = hostStub({ post(message) { calls.push(message); window.STHostTransport.deliver({ id: message.id, ok: true, result: {} }); } });
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, RegExp, Error,
    setTimeout() {}, clearTimeout() {} });
  web.prepare(context);
  const app = web.loadAppWith(context, { stop: () => { stops++; }, refresh: () => {}, auditionStep: () => {}, closeModal: () => {} });
  web.load(context, 'block-nav.js'); web.load(context, 'pane-navigation.js');
  const ST = window.ST, state = ST.state, nav = window.STBlockNav;
  state.project = ST.demoProject(); state.view = 'pattern'; state.track = 1; state.row = 4; state.column = 0;
  const capture = dom.listeners.window.keydown || [];
  function press(key, mods = {}) {
    const event = { target: document.activeElement, key, code: mods.code || key, altKey: !!mods.alt, metaKey: !!mods.meta, shiftKey: !!mods.shift, ctrlKey: !!mods.ctrl,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; } };
    for (const listener of capture) listener(event);
    if (!event.stopped) ST.keyboard(event);
    return event;
  }
  const active = () => document.activeElement?.id || document.activeElement?.tagName;
  return { ...dom, ST, app, state, nav, press, active, calls, get stops() { return stops; } };
}

// The Pattern page as it lays itself out at 1500×940 (rectangles written out here), with three tracks and one group.
function page(h) {
  const { el, box } = h, clicks = [];
  const button = (id, rect, parent, attrs = {}) => {
    const node = el('button', { id, rect, attrs }, parent);
    node.click = () => clicks.push(id);
    return node;
  };
  const top = el('header', { className: 'topbar', rect: box(0, 0, 1500, 52) }); el('button', { id: 'play', rect: box(400, 8, 40, 36) }, top);
  const views = el('nav', { className: 'nav', rect: box(0, 52, 1500, 36) });
  el('button', { id: 'view-pattern', className: 'nav-button active', attrs: { 'data-view': 'pattern' }, rect: box(10, 56, 90, 28) }, views);
  const app = el('div', { id: 'app', rect: box(0, 0, 1500, 940) });
  const side = el('aside', { className: 'sidebar', rect: box(0, 88, 152, 828) }, app);
  el('button', { id: 'sidebar-pattern-0', attrs: { 'data-pattern': '0', tabindex: '0' }, rect: box(8, 120, 136, 50) }, side);
  const main = el('section', { className: 'workspace-main', rect: box(152, 88, 1348, 828) }, app);
  const title = el('div', { className: 'view-title', rect: box(152, 88, 1348, 43) }, main);
  el('button', { id: 'pattern-rename', rect: box(1070, 94, 60, 30) }, title); el('button', { id: 'track-add', rect: box(1320, 94, 60, 30) }, title);
  const destination = el('section', { className: 'pattern-destination', rect: box(152, 131, 1348, 57) }, main);
  el('select', { id: 'pattern-destination-lane', rect: box(222, 137, 192, 24) }, destination);
  el('button', { id: 'pattern-enter-notes', rect: box(923, 137, 120, 24) }, destination);
  const toolbar = el('div', { className: 'toolbar pattern-toolbar', rect: box(152, 189, 1348, 35) }, main);
  const tools = el('div', { className: 'toolbar-group pattern-group', rect: box(167, 194, 460, 22) }, toolbar);
  el('input', { id: 'pattern-length', type: 'number', rect: box(238, 194, 46, 22) }, tools);
  el('select', { id: 'edit-step', rect: box(316, 194, 40, 22) }, tools);
  el('button', { id: 'follow', rect: box(360, 194, 44, 22) }, tools); el('button', { id: 'gate-column', rect: box(451, 194, 34, 22) }, tools);
  el('button', { id: 'hide-empty', rect: box(491, 194, 66, 22) }, tools);
  const perf = el('div', { className: 'toolbar-group perf-group', rect: box(639, 194, 846, 24) }, toolbar);
  el('button', { id: 'perf-toggle', rect: box(580, 194, 230, 24) }, perf);
  const wrap = el('div', { className: 'tracker-wrap', rect: box(152, 224, 1348, 626) }, main);
  // The table is taller than its scroll box; its header row sticks to the top.
  const table = el('table', { id: 'tracker', className: 'tracker', attrs: { tabindex: '0' }, rect: box(152, 224, 1340, 1668) }, wrap);
  const thead = el('thead', { rect: box(152, 224, 1340, 104) }, table);
  const groupRow = el('tr', { className: 'pattern-groups', rect: box(152, 224, 1340, 36) }, thead);
  const group = el('th', { className: 'group-head is-selected', rect: box(192, 224, 900, 36), attrs: { 'data-group-id': 'g1' } }, groupRow);
  button('group-g1-collapse', box(198, 230, 16, 16), group, { tabindex: '0' });
  button('group-g1-mute', box(1040, 230, 16, 16), group, { tabindex: '0' }); button('group-g1-solo', box(1060, 230, 16, 16), group, { tabindex: '0' });
  const heads = el('tr', { rect: box(152, 260, 1340, 68) }, thead);
  for (let i = 0; i < 3; i++) {
    const left = 192 + i * 450, selected = i === 1, tab = selected ? '0' : '-1';
    const th = el('th', { className: 'track-head' + (selected ? ' selected-track' : ''), attrs: { 'data-track': String(i) }, rect: box(left, 260, 450, 68) }, heads);
    button(`track-${i}-mute`, box(left + 380, 264, 16, 16), th, { tabindex: tab }); button(`track-${i}-solo`, box(left + 398, 264, 16, 16), th, { tabindex: tab });
    button(`track-${i}-collapse`, box(left + 418, 264, 14, 14), th, { tabindex: tab }); button(`track-${i}-delete`, box(left + 434, 264, 14, 14), th, { tabindex: tab });
    button(`track-${i}-source`, box(left + 8, 284, 430, 22), th, { tabindex: tab });
  }
  const tbody = el('tbody', { rect: box(152, 328, 1340, 1564) }, table);
  for (let r = 0; r < 8; r++) {
    const tr = el('tr', { attrs: { 'data-row': String(r) }, rect: box(152, 328 + r * 33, 1340, 33) }, tbody);
    for (let t = 0; t < 3; t++) el('td', { className: t === 1 && r === 4 ? 'selected' : '', attrs: { 'data-cell': `${t}:${r}` }, rect: box(192 + t * 450, 328 + r * 33, 450, 33) }, tr);
  }
  const step = el('div', { className: 'step-editor', rect: box(152, 873, 1348, 43) }, main);
  el('input', { id: 'step-note', type: 'number', rect: box(340, 880, 60, 26) }, step);
  el('button', { id: 'step-clear', rect: box(1150, 880, 60, 26) }, step);
  return { clicks, cell: () => h.document.querySelector('#tracker tbody td.selected') };
}
const alt = (h, keys, mods = { alt: true }) => keys.map(key => { const event = h.press('Arrow' + key, mods); return [key, event.stopped ? h.nav.current()?.id : null]; }); // stopped: a block move (capture phase)

test('Pattern blocks: reading order, entry controls and visible areas of the scrolled grid', () => {
  const h = harness(); page(h);
  deep(h.nav.blocks().map(b => [b.id, b.target.id]), [['transport', 'play'], ['views', 'view-pattern'], ['sidebar', 'sidebar-pattern-0'],
    ['view-actions', 'pattern-rename'], ['pattern-destination', 'pattern-destination-lane'], ['pattern-tools', 'pattern-length'],
    ['pattern-perform', 'perf-toggle'], ['track-headers', 'track-1-mute'], ['pattern-grid', 'tracker'], ['step-inspector', 'step-note']]);
  const area = id => { const r = h.nav.blocks().find(b => b.id === id).area(); return [r.left, r.top, r.right, r.bottom]; };
  deep(area('pattern-grid'), [152, 328, 1492, 850], 'the grid is its visible rows: below the sticky header, clipped to the scroll box');
  deep(area('track-headers'), [152, 224, 1492, 328]);
  assert.equal(h.nav.options('pattern').altArrows, true); assert.equal(h.nav.options('pattern').escape, true);
});

test('⌥ arrows move between Pattern blocks by position; from the grid ⌥⌘ arrows and F6 do', () => {
  const h = harness(); page(h);
  h.nodes.get('pattern-length').focus();
  deep(alt(h, ['Right', 'Left', 'Up', 'Up', 'Down', 'Down', 'Down']), [['Right', 'pattern-perform'], ['Left', 'pattern-tools'], ['Up', 'pattern-destination'],
    ['Up', 'view-actions'], ['Down', 'pattern-destination'], ['Down', 'pattern-tools'], ['Down', 'track-headers']]);
  assert.equal(h.active(), 'track-1-mute', 'headers: the cursor track\'s first button');
  deep(alt(h, ['Down']), [['Down', 'pattern-grid']]); assert.equal(h.active(), 'tracker');
  deep(alt(h, ['Down', 'Left', 'Right']), [['Down', null], ['Left', null], ['Right', null]], 'the grid keeps all four ⌥ arrows, also right after arriving');
  assert.equal(h.active(), 'tracker');
  deep(alt(h, ['Down', 'Up', 'Left'], { alt: true, meta: true }), [['Down', 'step-inspector'], ['Up', 'pattern-grid'], ['Left', 'sidebar']]);
  h.nodes.get('tracker').focus(); h.press('F6'); assert.equal(h.active(), 'step-note', 'F6 leaves the grid in reading order');
  deep(alt(h, ['Up']), [['Up', 'pattern-grid']], '⌥↑ from the step editor returns to the grid');
  h.document.body.focus(); h.document.activeElement = h.document.body;
  assert.equal(h.press('ArrowUp', { alt: true }).stopped, undefined, 'with nothing focused keys go to the grid: ⌥↑ is transpose, not a block move');
});

test('the grid keeps its keys: arrows, Tab, ⌥↑↓ transpose, Esc cancel / stop (no back-out)', () => {
  const h = harness(); page(h);
  const steps = h.ST.state.project.patterns.find(p => p.id === h.state.project.activePattern).steps;
  steps[1][4] = { note: 60, instrument: 0, velocity: 100 };
  h.nodes.get('tracker').focus();
  h.press('ArrowDown'); assert.equal(h.state.row, 5, '↓ moves the cursor'); h.press('ArrowUp'); assert.equal(h.state.row, 4);
  const up = h.press('ArrowUp', { alt: true });
  assert(up.defaultPrevented); assert.equal(up.stopped, undefined, 'not a block move');
  assert.equal(h.ST.state.project.patterns.find(p => p.id === h.state.project.activePattern).steps[1][4].note, 61, '⌥↑ transposes');
  h.press('Tab'); assert.equal(h.state.track, 2, 'Tab moves to the next track'); assert.equal(h.active(), 'tracker');
  h.press('Escape'); assert.equal(h.stops, 1, 'Esc in the grid stops as before'); assert.equal(h.active(), 'tracker', 'no back-out from the grid');
  assert.equal(h.nav.level, null);
});

test('track header buttons: every track and group button reachable with arrows, Return presses, Esc backs out', () => {
  const h = harness(); const { clicks } = page(h);
  h.nodes.get('pattern-length').focus(); h.press('ArrowDown', { alt: true });
  assert.equal(h.active(), 'track-1-mute');
  const walk = keys => keys.map(key => { const event = h.press(key); assert(event.defaultPrevented, key); return h.active(); });
  deep(walk(['ArrowRight', 'ArrowRight', 'ArrowRight', 'ArrowRight']), ['track-1-solo', 'track-1-collapse', 'track-1-delete', 'track-2-mute'],
    '→ walks the title row into the next track (tabindex -1 buttons included)');
  deep(walk(['ArrowDown', 'ArrowLeft', 'ArrowUp', 'ArrowUp']), ['track-2-source', 'track-1-source', 'track-1-mute', 'group-g1-mute'], '↓ ↑ change rows, ↑ reaches the group row');
  deep(walk(['ArrowRight', 'ArrowLeft', 'ArrowLeft', 'Home', 'End', 'Tab']), ['group-g1-solo', 'group-g1-mute', 'group-g1-collapse', 'group-g1-collapse', 'track-2-source', 'track-2-source'],
    'Home / End: first / last; Tab stops at the end');
  h.nodes.get('track-0-source').focus();
  const step = h.press('ArrowLeft'); assert(step.defaultPrevented); assert.equal(h.active(), 'track-0-source', 'nothing further: stay (no wrap)');
  h.nodes.get('track-0-mute').focus();
  assert.equal(h.state.track, 1, 'arrows on header buttons leave the grid cursor alone');
  h.press('Enter'); deep(clicks, ['track-0-mute'], 'Return presses the button (not audition)');
  h.press('Escape'); assert.equal(h.nav.level?.id, 'track-headers', 'Esc backs out to the header block'); assert.equal(h.stops, 0);
  h.press('Enter'); assert.equal(h.active(), 'track-1-mute', 'Return goes back in at the cursor track');
  assert(h.press('ArrowDown', { alt: true }).stopped, 'header buttons do not own ⌥ arrows'); assert.equal(h.active(), 'tracker');
});

test('focus returning to the grid lands on the cursor cell; ⌘0 comes back from anywhere', () => {
  const h = harness(); const { cell } = page(h);
  h.nodes.get('step-note').focus();
  const before = cell().scrolls;
  h.press('ArrowUp', { alt: true });
  assert.equal(h.active(), 'tracker'); assert.equal(cell().scrolls, before + 1, 'the cursor cell is scrolled into view, not the table');
  assert.equal(h.nodes.get('tracker').scrolls, 0);
  h.nodes.get('follow').focus();
  const zero = h.press('0', { meta: true, code: 'Digit0' });
  assert(zero.defaultPrevented); assert.equal(h.active(), 'tracker'); assert.equal(cell().scrolls, before + 2); assert.equal(h.nav.showing, true);
  h.ST.setView = view => { h.state.view = view; }; // the real one re-renders the page (no DOM here)
  h.state.view = 'song'; h.nodes.get('play').focus();
  h.press('0', { meta: true, code: 'Digit0' });
  assert.equal(h.state.view, 'pattern', '⌘0 switches to the Pattern page'); assert.equal(h.active(), 'tracker');
  h.state.modal = 'dialog'; h.nodes.get('play').focus(); h.press('0', { meta: true });
  assert.equal(h.active(), 'play', 'dialogs keep focus'); h.state.modal = null;
  assert.equal(h.ST.commands.find(c => c[0] === 'focusGrid')?.[2], '⌘0'); assert(h.app.hasCommand('focusGrid'));
  deep(h.calls.filter(c => /midi/i.test(JSON.stringify(c))), [], 'navigation sends no MIDI');
});

test('markup: header buttons have stable ids, the cursor track and group are in the Tab order', () => {
  const columns = web.read('app-pattern-columns.js'), lanes = web.read('app-track-lanes.js');
  for (const id of ['track-${i}-mute', 'track-${i}-solo', 'track-${i}-source', 'group-${id}-collapse', 'group-${id}-mute', 'group-${id}-solo']) assert(columns.includes(`id="${id}"`), id);
  for (const id of ['track-${i}-collapse', 'track-${i}-delete', 'group-${esc(group.id)}-delete', 'group-${esc(group.id)}-reveal']) assert(lanes.includes(`id="${id}"`), id);
  assert(/tab = i === state\.track \? 0 : -1/.test(columns) && /tab = selected \? 0 : -1/.test(columns));
  const doc = require('node:fs').readFileSync(require('node:path').join(__dirname, '../docs/KEYBOARD_NAVIGATION.md'), 'utf8');
  for (const words of ['⌘0', 'Track headers', 'F6', 'transpose']) assert(doc.includes(words), words);
  const { app } = web.app();
  const help = JSON.parse(JSON.stringify(app.helpSections())).find(([name]) => /Blocks/.test(name));
  assert(help[1].some(([, keys]) => keys === '⌘0'));
});

for (const { name, run } of tests) { run(); console.log('PASS ' + name); }
console.log(`Block navigation (Pattern): ${tests.length} regression groups passed.`);
