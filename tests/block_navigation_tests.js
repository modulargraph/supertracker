'use strict';
// Block navigation (block-nav.js + pane-navigation.js): geometry, wrap, remembered blocks, Esc / Enter,
// ⌥ + arrow conflict rules, dialogs, and the block layout of the Browser, Song, Recording, Automation and
// Settings pages. Real WebKit geometry is checked with the dev harness (docs/KEYBOARD_NAVIGATION.md).
const assert = require('node:assert/strict');
const vm = require('node:vm');
const web = require('./helpers/web');
const { createDOM } = require('./helpers/fake-dom');

const deep = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual)), expected, message);
const tests = [];
function test(name, run) { tests.push({ name, run }); }

function harness(view = 'browser') {
  const dom = createDOM(), { document, window } = dom, calls = [];
  let stops = 0, closes = 0;
  window.webkit = { messageHandlers: { supertracker: { postMessage(message) { calls.push(message); window.nativeReceive?.({ id: message.id, ok: true, result: {} }); } } } };
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, RegExp, Error,
    setTimeout() {}, clearTimeout() {} });
  web.prepare(context);
  web.loadAppWith(context, { stop: () => { stops++; }, refresh: () => {}, closeModal: () => { closes++; } });
  web.load(context, 'block-nav.js'); web.load(context, 'pane-navigation.js');
  const ST = window.ST, state = ST.state, nav = window.STBlockNav;
  state.project = ST.demoProject(); state.view = view;
  const capture = dom.listeners.window.keydown || [];
  function press(key, mods = {}) {
    const event = { target: document.activeElement, key, code: key, altKey: !!mods.alt, metaKey: !!mods.meta, shiftKey: !!mods.shift, ctrlKey: false,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; } };
    for (const listener of capture) listener(event);
    if (!event.stopped) ST.keyboard(event);
    return event;
  }
  const active = () => document.activeElement?.id || document.activeElement?.className;
  return { ...dom, ST, state, nav, press, active, calls, get stops() { return stops; }, get closes() { return closes; } };
}

// Shared chrome: transport bar and view switcher across the top.
function chrome(h, view) {
  const { el, box } = h;
  const top = el('header', { className: 'topbar', rect: box(0, 0, 1500, 52) });
  el('button', { id: 'play', rect: box(400, 8, 40, 36) }, top);
  const nav = el('nav', { className: 'nav', rect: box(0, 52, 1500, 36) });
  el('button', { id: 'view-' + view, className: 'nav-button active', attrs: { 'data-view': view }, rect: box(300, 56, 90, 28) }, nav);
}
function browser(h, tab = 'samples') {
  const { el, box } = h; chrome(h, 'browser');
  const app = el('div', { id: 'app', rect: box(0, 0, 1500, 940) });
  const work = el('div', { className: 'browser-workspace', rect: box(0, 88, 1500, 800) }, app);
  const tabs = el('div', { className: 'browser-tabs', attrs: { role: 'tablist' }, rect: box(1085, 88, 400, 33) }, work);
  ['samples', 'kits', 'nks', 'plugins'].forEach((name, i) => el('button', { id: 'browser-tab-' + name,
    attrs: { 'data-browser-tab': name, 'aria-selected': String(name === tab), tabindex: name === tab ? '0' : '-1' }, rect: box(1090 + i * 98, 90, 90, 30) }, tabs));
  const search = el('div', { className: 'browser-search-wrap', rect: box(218, 122, 1026, 42) }, work);
  el('input', { id: tab === 'plugins' ? 'browser-plugin-search' : 'browser-search', type: 'search', rect: box(240, 130, 900, 26) }, search);
  if (tab === 'kits' || tab === 'nks') {
    const side = el('aside', { className: 'browser-sidebar panel nksb-sidebar', rect: box(0, 122, 231, 766) }, work);
    el('button', { id: 'kitb-new', rect: box(10, 128, 80, 26) }, side);
    el('div', { id: 'kitb-products', className: 'nksb-product-list', attrs: { tabindex: '0', role: 'listbox' }, rect: box(0, 161, 231, 694) }, side);
  } else {
    const side = el('aside', { className: 'browser-sidebar panel', rect: box(0, 122, 218, 766) }, work);
    if (tab === 'plugins') el('button', { id: 'browser-plugin-kind-instrument', attrs: { 'data-browser-plugin-kind': 'instrument', tabindex: '0' }, rect: box(10, 150, 190, 28) }, side);
    else { const details = el('details', {}, side); el('summary', { id: 'browser-group-collections', rect: box(10, 330, 190, 28) }, details); }
  }
  const summary = el('div', { className: 'browser-result-summary', rect: box(218, 164, 1026, 29) }, work);
  el('input', { id: 'browser-audition', type: 'checkbox', rect: box(1200, 170, 16, 16) }, summary);
  el('div', { id: tab === 'plugins' ? 'browser-plugin-list' : 'browser-results-list', className: tab === 'plugins' ? 'browser-plugin-list' : 'browser-result-list',
    attrs: { tabindex: '0', role: 'listbox' }, rect: box(218, 221, 1026, 599) }, work);
  const footer = el('div', { className: 'browser-list-footer', rect: box(218, 820, 1026, 29) }, work);
  el('button', { id: 'browser-page-next', rect: box(1200, 822, 26, 24) }, footer);
  if (tab !== 'kits') {
    const detail = el('aside', { className: 'browser-detail', rect: box(1244, 122, 256, 766) }, work);
    el('button', { id: tab === 'plugins' ? 'browser-plugin-load' : 'browser-load', rect: box(1260, 260, 220, 36) }, detail);
    el('button', { id: 'browser-load-new', rect: box(1260, 310, 220, 36) }, detail);
  }
}
function song(h) {
  const { el, box } = h; chrome(h, 'song');
  const side = el('aside', { className: 'sidebar', rect: box(0, 88, 152, 828) });
  el('button', { id: 'sidebar-pattern-0', attrs: { 'data-pattern': '0', tabindex: '0' }, rect: box(8, 120, 136, 50) }, side);
  const main = el('section', { className: 'workspace-main', rect: box(152, 88, 1348, 828) });
  const title = el('div', { className: 'view-title', rect: box(152, 88, 1348, 43) }, main);
  el('button', { id: 'song-export', rect: box(1150, 94, 110, 30) }, title);
  el('button', { id: 'sequence-add', rect: box(1270, 94, 110, 30) }, title);
  el('div', { id: 'sequence-list', attrs: { tabindex: '0', role: 'listbox' }, rect: box(152, 212, 1089, 280) }, main);
  const options = el('div', { className: 'song-options', rect: box(1243, 167, 249, 317) }, main);
  el('input', { id: 'loop-song', type: 'checkbox', rect: box(1260, 220, 16, 16) }, options);
  el('input', { id: 'swing', type: 'range', rect: box(1260, 300, 200, 16) }, options);
  el('button', { id: 'song-play', rect: box(1260, 420, 220, 30) }, options);
  const toolbar = el('div', { className: 'clips-toolbar', rect: box(152, 553, 1340, 41) }, main);
  el('input', { id: 'clips-cursor', type: 'number', rect: box(300, 560, 90, 26) }, toolbar);
  el('div', { id: 'clips-timeline', attrs: { tabindex: '0' }, rect: box(152, 595, 1340, 212) }, main);
}
function arrows(h, keys, mods = { alt: true }) {
  return keys.map(key => { const event = h.press('Arrow' + key, mods); return [key, event.defaultPrevented ? h.nav.current()?.id : null]; });
}

test('pick: same row / column first, nearest edge, focused-control anchor, way back, wrap and edges', () => {
  const h = harness();
  const r = (left, top, width, height) => ({ left, top, right: left + width, bottom: top + height });
  const A = { id: 'A', rect: r(0, 0, 100, 300) }, B = { id: 'B', rect: r(110, 0, 300, 100) }, C = { id: 'C', rect: r(110, 110, 300, 190) },
    D = { id: 'D', rect: r(420, 0, 100, 300) }, wide = { id: 'wide', rect: r(0, 310, 520, 40) };
  const all = [A, B, C, D, wide], pick = (from, direction, prefer, anchor) => h.nav.pick({ ...from.rect, anchor }, all.filter(c => c !== from), direction, prefer)?.id ?? null;
  assert.equal(pick(A, 'right', null, { x: 50, y: 40 }), 'B', 'anchor near the top picks the upper neighbour');
  assert.equal(pick(A, 'right', null, { x: 50, y: 250 }), 'C', 'anchor near the bottom picks the lower neighbour');
  assert.equal(pick(A, 'right', 'C', { x: 50, y: 40 }), 'C', 'the block you came from wins a near tie');
  assert.equal(pick(B, 'down'), 'C'); assert.equal(pick(C, 'up'), 'B'); assert.equal(pick(C, 'down'), 'wide');
  assert.equal(pick(B, 'right'), 'D'); assert.equal(pick(D, 'left', null, { x: 470, y: 200 }), 'C');
  assert.equal(pick(wide, 'up', null, { x: 50, y: 330 }), 'A');
  const wrapped = h.nav.pick(D.rect, all.filter(c => c !== D), 'right');
  assert.equal(wrapped.id, 'A'); assert.equal(wrapped.wrapped, true, 'right of the last block wraps to the start of the row');
  assert.equal(pick(wide, 'right'), null, 'a full-width block has nothing beside it: stay');
  assert.equal(h.nav.pick(wide.rect, [{ id: 'top', rect: r(0, 0, 520, 40) }], 'right'), null, 'a wider block above is not "to the right"');
  assert.equal(h.nav.pick(B.rect, [], 'up'), null);
});

test('Browser: ⌥ arrows move between blocks by position, show the frame and remember the block', () => {
  const h = harness('browser'); browser(h);
  deep(h.nav.blocks().map(b => b.id), ['transport', 'views', 'browser-tabs', 'browser-search', 'browser-filters', 'browser-summary', 'browser-results', 'browser-pages', 'browser-detail']);
  h.nodes.get('browser-search').focus();
  assert.equal(h.nav.showing, false);
  deep(arrows(h, ['Left', 'Right', 'Down', 'Down', 'Right', 'Right', 'Up', 'Left', 'Up', 'Up']),
    [['Left', 'browser-filters'], ['Right', 'browser-search'], ['Down', 'browser-summary'], ['Down', 'browser-results'], ['Right', 'browser-detail'],
      ['Right', 'browser-filters'], ['Up', 'views'], ['Left', 'views'], ['Up', 'transport'], ['Up', 'browser-pages']], 'full-width bars have nothing beside them; up from the top wraps to the bottom of the column');
  assert.equal(h.nav.showing, true, 'keyboard navigation shows the frame and hint');
  assert.equal(h.nav.remembered('browser'), 'browser-pages');
  h.nodes.get('browser-results-list').focus(); assert.equal(h.nav.remembered('browser'), 'browser-results', 'any focus (also a click) sets the block');
  deep(arrows(h, ['Right', 'Up']), [['Right', 'browser-detail'], ['Up', 'browser-tabs']]);
  assert.equal(h.active(), 'browser-tab-samples', 'tabs land on the selected tab');
  deep(arrows(h, ['Down']), [['Down', 'browser-detail']], 'straight back down returns to the block you came from');
  assert(h.nodes.get('browser-load').scrolls >= 1, 'the block scrolls into view');
  deep(h.calls, [], 'moving between blocks sends nothing to the engine');
});

test('Browser tabs: Kits / NKS sidebars and Plug-ins resolve their blocks', () => {
  for (const tab of ['kits', 'nks', 'plugins']) {
    const h = harness('browser'); browser(h, tab);
    const ids = h.nav.blocks().map(b => [b.id, b.target.id]);
    if (tab === 'plugins') deep(ids.slice(2), [['browser-tabs', 'browser-tab-plugins'], ['browser-search', 'browser-plugin-search'],
      ['browser-filters', 'browser-plugin-kind-instrument'], ['browser-summary', 'browser-audition'], ['browser-results', 'browser-plugin-list'],
      ['browser-pages', 'browser-page-next'], ['browser-detail', 'browser-plugin-load']]);
    else deep(ids.find(([id]) => id === 'browser-products'), ['browser-products', 'kitb-products'], tab + ': product list is the entry, header buttons are in the block');
    assert(h.nav.blockOf(h.nodes.get(tab === 'plugins' ? 'browser-plugin-kind-instrument' : 'kitb-new')), tab);
  }
});

test('⌥ + arrow conflicts: owners keep them once used; ⌥⌘ + arrows always move; the pattern grid keeps ⌥ + arrows', () => {
  const h = harness('browser'); browser(h);
  const search = h.nodes.get('browser-search');
  const owns = (node, key) => h.nav.owns(node, key);
  assert.equal(owns(search, 'ArrowLeft'), false, 'an empty text field has no words to jump through');
  search.value = 'kick drum';
  assert.equal(owns(search, 'ArrowLeft'), true); assert.equal(owns(search, 'ArrowUp'), true, 'library search: ⌥↑↓ walks recent searches');
  // Arriving by keyboard: ⌥ + arrows pass through until the field is used.
  h.nodes.get('browser-results-list').focus(); h.press('ArrowUp', { alt: true }); h.press('ArrowUp', { alt: true });
  assert.equal(h.active(), 'browser-search');
  assert.equal(h.nav.arrival, search);
  assert(h.press('ArrowLeft', { alt: true }).defaultPrevented, 'just arrived: ⌥← still moves'); assert.equal(h.nav.current().id, 'browser-filters');
  search.focus(); h.press('k');
  const typed = h.press('ArrowLeft', { alt: true });
  assert.equal(typed.defaultPrevented, undefined, 'after typing, ⌥← jumps a word in the field'); assert.equal(h.active(), 'browser-search');
  assert(h.press('ArrowLeft', { alt: true, meta: true }).defaultPrevented, '⌥⌘← moves from inside the field'); assert.equal(h.nav.current().id, 'browser-filters');
  assert.equal(h.press('ArrowLeft', { alt: true, shift: true }).defaultPrevented, undefined, '⌥⇧ + arrows are never block moves (selection, transpose by octave)');

  const s = harness('song'); song(s);
  const list = s.nodes.get('sequence-list'), button = s.el('button', { id: 'seq-button' }, list);
  assert.equal(s.nav.owns(list, 'ArrowUp'), true, '⌥↑↓ moves song blocks'); assert.equal(s.nav.owns(list, 'ArrowRight'), false);
  assert.equal(s.nav.owns(button, 'ArrowUp'), false, 'buttons inside the list do not move blocks');
  assert.equal(s.nav.owns(s.nodes.get('clips-timeline'), 'ArrowLeft'), true, 'clips: ⌥←→ slip audio');
  const marked = s.el('div', { attrs: { 'data-alt-arrows': 'horizontal' } }); const inner = s.el('button', {}, marked);
  assert.equal(s.nav.owns(inner, 'ArrowLeft'), true); assert.equal(s.nav.owns(inner, 'ArrowDown'), false);
  list.focus(); s.press('ArrowDown'); // used
  const moved = s.press('ArrowDown', { alt: true });
  assert.equal(moved.stopped, undefined, 'the song list keeps ⌥↓ (move block) once used');
  assert(s.press('ArrowRight', { alt: true }).defaultPrevented, '⌥→ is free in the list: next block'); assert.equal(s.nav.current().id, 'song-settings');

  const p = harness('pattern'); chrome(p, 'pattern'); p.el('div', { id: 'tracker', attrs: { tabindex: '0' }, rect: p.box(0, 120, 1500, 600) });
  p.nodes.get('play').focus();
  assert(p.press('ArrowDown', { alt: true }).defaultPrevented, 'pattern: plain ⌥ + arrows move outside the grid'); assert.equal(p.nav.current().id, 'views');
  assert(p.press('ArrowDown', { alt: true }).defaultPrevented); assert.equal(p.active(), 'tracker');
  assert.equal(p.press('ArrowDown', { alt: true }).stopped, undefined, 'pattern grid: ⌥↓ stays transpose even right after arriving');
  assert(p.press('ArrowUp', { alt: true, meta: true }).defaultPrevented, 'pattern: ⌥⌘ + arrows move from the grid'); assert.equal(p.nav.current().id, 'views');
});

test('Esc backs out to the block, Enter goes back in, Esc at block level stops; while playing Esc stops at once', () => {
  const h = harness('song'); song(h);
  h.nodes.get('swing').focus();
  const out = h.press('Escape');
  assert(out.defaultPrevented); assert.equal(h.stops, 0, 'first Esc does not stop');
  const root = h.document.activeElement;
  assert(root.classes.has('song-options')); assert(root.hasAttribute('data-nav-root')); assert.equal(root.getAttribute('tabindex'), '-1');
  assert.equal(h.nav.level.id, 'song-settings');
  h.press('Enter'); assert.equal(h.active(), 'loop-song', 'Enter goes back in to the block entry');
  h.press('Escape'); h.press('ArrowDown'); assert.equal(h.active(), 'loop-song', 'arrows at block level go in as well');
  h.press('Escape'); h.press('Escape'); assert.equal(h.stops, 1, 'Esc at block level does what it did before');
  h.nodes.get('sequence-list').focus(); h.press('Escape'); assert.equal(h.stops, 2, 'a block that is itself a widget is already block level');
  h.state.playing = true; h.nodes.get('song-play').focus(); h.press('Escape');
  assert.equal(h.stops, 3, 'while playing, Esc stops on the first press'); assert(h.document.activeElement.classes.has('song-options'));
  h.state.playing = false;
  // Dialogs: Esc closes; no back-out.
  const d = harness('song'); song(d); d.state.modal = 'dialog';
  d.nodes.get('swing').focus(); d.press('Escape'); assert.equal(d.nav.level, null); assert.equal(d.closes, 1, 'Esc closes a dialog at once');
});

test('arrows on buttons move to the nearest control inside the block; F6 keeps reading order', () => {
  const h = harness('song'); song(h);
  h.nodes.get('song-export').focus();
  assert(h.press('ArrowRight').defaultPrevented); assert.equal(h.active(), 'sequence-add');
  assert.equal(h.press('ArrowRight').defaultPrevented, undefined, 'no wrap at the end of a block'); assert.equal(h.active(), 'sequence-add');
  h.nodes.get('song-play').focus(); h.press('ArrowUp'); assert.equal(h.active(), 'swing');
  h.nodes.get('loop-song').focus(); assert.equal(h.press('ArrowLeft').defaultPrevented, undefined, 'nothing to the left inside the block');
  const tools = h.nodes.get('clips-cursor').parentElement; h.el('button', { id: 'clips-preview', rect: h.box(400, 560, 80, 26) }, tools); h.el('button', { id: 'clips-bounce', rect: h.box(500, 560, 80, 26) }, tools);
  h.nodes.get('clips-preview').focus(); assert.equal(h.press('ArrowRight').defaultPrevented, undefined); assert.equal(h.active(), 'clips-preview', 'clip buttons keep native arrows');
  deep(h.window.STPaneNavigation.sections().map(s => s.key), ['transport', 'views', 'sidebar', 'view-actions', 'sequence', 'song-settings', 'clip-tools', 'timeline']);
  h.nodes.get('play').focus(); h.press('F6'); assert.equal(h.active(), 'view-song'); h.press('F6', { shift: true }); assert.equal(h.active(), 'play');
});

test('each page remembers its block; restore() only on the page on screen and only on audited pages', () => {
  const h = harness('browser'); browser(h);
  h.nodes.get('browser-load').focus(); assert.equal(h.nav.remembered('browser'), 'browser-detail');
  h.nodes.get('browser-search').focus(); assert.equal(h.nav.remembered(), 'browser-search');
  assert(h.nav.restore('browser', 'browser-detail')); assert.equal(h.active(), 'browser-load');
  assert.equal(h.nav.restore('browser', 'browser-detail'), false, 'already there');
  assert.equal(h.nav.restore('song', 'browser-detail'), false, 'only the page on screen');
  const p = harness('pattern'); chrome(p, 'pattern'); p.el('div', { id: 'tracker', attrs: { tabindex: '0' }, rect: p.box(0, 120, 1500, 600) });
  assert(p.nav.restore('pattern', 'views'), 'pattern is audited: restore() works there too');
  // ⌘1–⌘7 keep their documented entry control (NativeUIAcceptanceTests); restore() is the hook for a later opt-in.
  assert(!/STBlockNav/.test(web.read('app-shell.js')), 'view switches keep their entry focus');
});

test('Recording and Automation pages resolve their blocks and move spatially', () => {
  const h = harness('recording'); chrome(h, 'recording');
  const { el, box } = h;
  const side = el('aside', { className: 'sidebar recording-sidebar', rect: box(0, 88, 184, 828) }); el('button', { id: 'recording-capture-section', rect: box(8, 120, 168, 40) }, side);
  const setup = el('section', { id: 'recording-setup', rect: box(210, 180, 760, 700) });
  const content = el('div', { className: 'recording-content', rect: box(229, 238, 728, 380) }, setup);
  const grid = el('div', { className: 'recording-grid', rect: box(229, 238, 728, 95) }, content); el('input', { id: 'record-name', rect: box(229, 260, 340, 26) }, grid);
  const input = el('div', { id: 'record-input-settings', rect: box(229, 376, 728, 91) }, content); el('select', { id: 'record-input-device', rect: box(229, 390, 340, 26) }, input);
  const foot = el('div', { className: 'recording-inline-foot', rect: box(229, 768, 735, 90) }, setup); el('button', { id: 'record-start', rect: box(760, 800, 190, 34) }, foot);
  const iface = el('div', { className: 'recording-interface', rect: box(1006, 218, 469, 102) }); el('button', { id: 'recording-studio-settings', rect: box(1020, 280, 150, 28) }, iface);
  const routing = el('section', { id: 'recording-routing', rect: box(1005, 339, 471, 211) }); el('button', { id: 'recording-routing-configure', rect: box(1380, 345, 80, 26) }, routing);
  deep(h.nav.blocks().map(b => [b.id, b.target.id]).slice(2), [['sidebar', 'recording-capture-section'], ['record-details', 'record-name'],
    ['record-input', 'record-input-device'], ['record-capture', 'record-start'], ['recording-interface', 'recording-studio-settings'], ['recording-inputs', 'recording-routing-configure']]);
  assert.equal(h.nav.blocks().find(b => b.id === 'sidebar').label, 'Recording');
  h.nodes.get('record-input-device').focus();
  deep(arrows(h, ['Right', 'Down', 'Left', 'Right', 'Up']), [['Right', 'recording-inputs'], ['Down', 'record-capture'], ['Left', 'sidebar'], ['Right', 'record-capture'], ['Up', 'record-input']],
    'diagonal neighbours count when nothing shares the column; going back returns');

  const a = harness('automation'); chrome(a, 'automation');
  const scope = a.el('div', { className: 'studio-automation-scope', rect: a.box(18, 100, 1460, 40) }); a.el('select', { id: 'automation-pattern', rect: a.box(100, 105, 120, 28) }, scope);
  const targets = a.el('div', { className: 'studio-target-panel', rect: a.box(18, 183, 230, 700) }); a.el('input', { id: 'automation-search', type: 'search', rect: a.box(25, 190, 210, 26) }, targets);
  const lanes = a.el('div', { className: 'studio-lane-tabs', rect: a.box(260, 183, 1200, 36) }); a.el('button', { id: 'lane-1', attrs: { 'data-select-lane': '0', 'aria-selected': 'true' }, rect: a.box(265, 186, 120, 30) }, lanes);
  a.el('svg', { id: 'automation-envelope', attrs: { tabindex: '0' }, rect: a.box(260, 230, 1200, 300) });
  const table = a.el('div', { className: 'studio-points-table', rect: a.box(260, 540, 1200, 300) }); a.el('input', { id: 'point-0', type: 'number', attrs: { 'data-point-row': '0' }, rect: a.box(270, 550, 80, 24) }, table);
  deep(a.nav.blocks().map(b => b.id).slice(2), ['automation-scope', 'automation-targets', 'automation-lanes', 'automation-envelope', 'automation-points']);
  a.nodes.get('automation-search').focus();
  deep(arrows(a, ['Right', 'Down', 'Down', 'Up']), [['Right', 'automation-lanes'], ['Down', 'automation-envelope'], ['Down', 'automation-points'], ['Up', 'automation-envelope']],
    'consecutive ⌥ + arrows pass through the envelope');
  a.press('ArrowLeft');
  deep(arrows(a, ['Down']), [['Down', null]], 'once used, the envelope keeps ⌥↓ (move point)');
  assert(a.press('ArrowDown', { alt: true, meta: true }).defaultPrevented); assert.equal(a.nav.current().id, 'automation-points');
});

test('Dialogs: blocks stay inside the dialog, Settings registers its sections, Tab is trapped', () => {
  const h = harness('song'); song(h);
  const { el, box } = h;
  const overlay = el('div', { id: 'overlay' });
  const modal = el('section', { className: 'modal studio-settings-modal', rect: box(424, 100, 644, 700) }, overlay);
  const head = el('header', { className: 'modal-head', rect: box(424, 100, 644, 50) }, modal); el('button', { id: 'modal-close', rect: box(1030, 110, 26, 26) }, head);
  const body = el('div', { className: 'modal-body', rect: box(424, 150, 644, 560) }, modal);
  const g1 = el('div', { className: 'grid2', rect: box(440, 164, 610, 120) }, body); el('select', { id: 'audio-output', rect: box(440, 180, 300, 26) }, g1);
  const g2 = el('div', { className: 'grid2', rect: box(440, 290, 610, 60) }, body); el('select', { id: 'input-channel-left', rect: box(440, 300, 300, 26) }, g2);
  const g3 = el('div', { className: 'grid2', rect: box(440, 400, 610, 120) }, body); el('select', { id: 'midi-input', rect: box(440, 410, 300, 26) }, g3);
  const agent = el('div', { className: 'agent-settings', rect: box(440, 600, 610, 60) }, body); el('input', { id: 'agent-enabled', type: 'checkbox', rect: box(440, 620, 16, 16) }, agent);
  const foot = el('footer', { className: 'modal-foot', rect: box(424, 720, 644, 60) }, modal);
  el('button', { id: 'settings-cancel', rect: box(850, 730, 90, 30) }, foot); el('button', { id: 'settings-apply', className: 'primary', rect: box(950, 730, 100, 30) }, foot);
  h.state.modal = 'settings';
  deep(h.nav.blocks().map(b => [b.id, b.target.id]), [['settings-audio', 'audio-output'], ['settings-midi', 'midi-input'], ['settings-agent', 'agent-enabled'], ['settings-actions', 'settings-apply']]);
  h.nodes.get('audio-output').focus();
  deep(arrows(h, ['Down', 'Down', 'Down', 'Down', 'Up']), [['Down', 'settings-midi'], ['Down', 'settings-agent'], ['Down', 'settings-actions'], ['Down', 'settings-audio'], ['Up', 'settings-actions']], 'wraps top to bottom inside the dialog');
  assert.equal(h.window.STPaneNavigation.next(), false, 'F6 stays a workspace key');
  h.state.modal = 'dialog';
  deep(h.nav.blocks().map(b => b.id), ['modal-head', 'modal-body', 'modal-foot'], 'any dialog: title, body and buttons');
  h.nodes.get('audio-output').focus();
  assert.equal(h.press('ArrowDown', { alt: true }).stopped, undefined, 'plain ⌥ + arrows are left to dialog controls (fine adjustment in send effects)');
  assert(h.press('ArrowDown', { alt: true, meta: true }).defaultPrevented); assert.equal(h.active(), 'settings-apply');
  // Focus trap: Tab from outside the dialog (e.g. body after a click on its background) comes back in.
  h.document.body.focus(); h.document.activeElement = h.document.body;
  for (const node of [h.nodes.get('modal-close'), h.nodes.get('settings-apply')]) node.offsetParent = modal;
  for (const node of ['audio-output', 'input-channel-left', 'midi-input', 'agent-enabled', 'settings-cancel']) h.nodes.get(node).offsetParent = modal;
  const tab = h.press('Tab'); assert(tab.defaultPrevented); assert.equal(h.active(), 'modal-close');
  h.document.activeElement = h.document.body; h.press('Tab', { shift: true }); assert.equal(h.active(), 'settings-apply');
});

test('commands, help sheet and catalogue document the scheme', () => {
  const { app, ST } = web.app();
  for (const [id, keys] of [['blockLeft', '⌥⌘←'], ['blockRight', '⌥⌘→'], ['blockUp', '⌥⌘↑'], ['blockDown', '⌥⌘↓']]) {
    assert.equal(ST.commands.find(c => c[0] === id)?.[2], keys, id); assert(app.hasCommand(id), id);
  }
  const help = JSON.parse(JSON.stringify(app.helpSections())).find(([name]) => /Blocks/.test(name));
  assert(help && help[1].some(([, keys]) => keys.includes('⌥⌘←')) && help[1].some(([, keys]) => keys === 'Escape'));
});

for (const { name, run } of tests) { run(); console.log('PASS ' + name); }
console.log(`Block navigation: ${tests.length} regression groups passed.`);
