'use strict';
// Block navigation, phase 2: the Instrument page (Sample, Slices, Zones, Modulation, Voice & Record, plug-in
// consoles, instrument effects) and the Mixer (strips, divider, insert chain, knob page, routing, returns).
// Layouts mirror the real WebKit geometry at 1500 × 940 (test-output/keyboard-nav/phase2/results.json).
const assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const vm = require('node:vm');
const web = require('./helpers/web');
const { createDOM } = require('./helpers/fake-dom');

const deep = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual)), expected, message);
const tests = [];
function test(name, run) { tests.push({ name, run }); }

function harness(view) {
  const dom = createDOM(), { document, window } = dom, calls = [];
  window.STHostTransport = hostStub({ post(message) { calls.push(message); window.STHostTransport.deliver({ id: message.id, ok: true, result: {} }); } });
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, RegExp, Error,
    setTimeout() {}, clearTimeout() {} });
  web.prepare(context);
  web.loadAppWith(context, { stop: () => {}, refresh: () => {}, closeModal: () => {} });
  web.load(context, 'block-nav.js'); web.load(context, 'pane-navigation.js');
  const ST = window.ST, nav = window.STBlockNav;
  ST.state.project = ST.demoProject(); ST.state.view = view;
  const capture = dom.listeners.window.keydown || [];
  function press(key, mods = {}) {
    const event = { target: document.activeElement, key, code: key, altKey: !!mods.alt, metaKey: !!mods.meta, shiftKey: !!mods.shift, ctrlKey: false,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; } };
    for (const listener of capture) listener(event);
    if (!event.stopped) ST.keyboard(event);
    return event;
  }
  const click = node => { for (const listener of dom.listeners.document.pointerdown || []) listener({ target: node }); node.focus(); };
  const active = () => document.activeElement?.id || document.activeElement?.className;
  const alt = (keys, mods = { alt: true }) => keys.map(key => { const event = press('Arrow' + key, mods); return [key, event.defaultPrevented ? nav.current()?.id : null]; });
  // Shared chrome: transport, view switcher, sidebar.
  const { el, box } = dom;
  const top = el('header', { className: 'topbar', rect: box(0, 0, 1500, 52) }); el('button', { id: 'play', rect: box(400, 8, 40, 36) }, top);
  const views = el('nav', { className: 'nav', rect: box(0, 52, 1500, 36) });
  el('button', { id: 'view-' + view, className: 'nav-button active', attrs: { 'data-view': view }, rect: box(300, 56, 90, 28) }, views);
  return { ...dom, ST, nav, press, click, active, alt, calls };
}

// One sampler parameter: slider and value field (app-instrument-params.js parameter()).
function param(h, parent, key, left, top) {
  const label = h.el('label', { className: 'parameter', rect: h.box(left, top, 214, 57) }, parent);
  h.el('input', { id: 'param-' + key, type: 'range', attrs: { 'data-inst-param': key }, rect: h.box(left + 7, top + 30, 120, 16) }, label);
  h.el('input', { id: 'param-value-' + key, type: 'number', className: 'parameter-value', attrs: { 'data-inst-param': key }, rect: h.box(left + 140, top + 26, 70, 24) }, label);
  return label;
}
function group(h, parent, id, label, left, top, width, height, keys, select) {
  const panel = h.el('section', { id: 'inst-group-' + id, className: 'panel inst-group', attrs: { 'aria-label': label }, rect: h.box(left, top, width, height) }, parent);
  const head = h.el('div', { className: 'panel-head', rect: h.box(left, top, width, 36) }, panel);
  if (select) h.el('select', { id: select, rect: h.box(left + width - 100, top + 8, 88, 18) }, head);
  const body = h.el('div', { className: 'panel-body', rect: h.box(left, top + 36, width, height - 36) }, panel);
  keys.forEach((key, i) => param(h, body, key, left + 7 + (i % 2) * 218, top + 42 + Math.floor(i / 2) * 58));
  return panel;
}
function chain(h, parent, prefix, top, tiles = []) {
  const root = h.el('section', { className: 'fx-chain', attrs: { 'data-fx-root': 'track:0' }, rect: h.box(166, top, 1311, 147) }, parent);
  const head = h.el('div', { className: 'fx-chain-head', rect: h.box(166, top, 1311, 24) }, root);
  if (prefix === 'mixfx') h.el('button', { id: 'mix-search-plugins', attrs: { 'data-studio-action': 'add-plugin' }, rect: h.box(700, top, 110, 24) }, head);
  const strip = h.el('div', { className: 'fx-tiles', rect: h.box(166, top + 36, 1311, 80) }, root);
  if (prefix === 'instfx') h.el('button', { id: 'instfx-source', className: 'fx-tile fx-source', attrs: { tabindex: '-1' }, rect: h.box(168, top + 38, 132, 72) }, strip);
  tiles.forEach((name, i) => h.el('div', { id: `${prefix}-tile-${i}`, className: 'fx-tile' + (i === 0 ? ' is-selected' : ''),
    attrs: { 'data-fx-tile': String(i), tabindex: i === 0 ? '0' : '-1', role: 'option' }, rect: h.box(168 + i * 146, top + 38, 132, 72) }, strip));
  h.el('button', { id: prefix + '-add', className: 'fx-tile fx-add', rect: h.box(168 + tiles.length * 146, top + 38, 96, 72) }, strip);
  if (!tiles.length) return root;
  const module = h.el('div', { className: 'fx-module', rect: h.box(18, top + 130, 826, 190) }, parent);
  h.el('button', { id: prefix + '-page-0', className: 'is-on', attrs: { 'data-fx-page': '0', role: 'tab' }, rect: h.box(200, top + 140, 60, 24) }, module);
  for (let i = 0; i < 4; i++) h.el('div', { id: `${prefix}-knob-${i}`, attrs: { 'data-fx-dial': String(i), role: 'slider', tabindex: '0' }, rect: h.box(40 + i * 100, top + 200, 48, 48) }, module);
  return root;
}

// The Instrument page of a sampler (Sample page shown, Modulation page mounted but hidden), as at 1500 × 940.
function instrument(h, { granular = false } = {}) {
  const { el, box } = h;
  const side = el('aside', { className: 'sidebar', rect: box(0, 88, 152, 828) });
  el('button', { id: 'sidebar-instrument-0', attrs: { 'data-instrument': '0', tabindex: '0' }, rect: box(8, 140, 136, 50) }, side);
  const main = el('section', { className: 'workspace-main', rect: box(152, 88, 1348, 828) });
  const title = el('div', { className: 'view-title', rect: box(152, 88, 1348, 48) }, main);
  el('button', { id: 'instrument-find', rect: box(970, 94, 90, 26) }, title); el('button', { id: 'load-sample', rect: box(1235, 94, 90, 26) }, title);
  const toolbar = el('div', { className: 'toolbar', rect: box(152, 136, 1348, 31) }, main);
  const types = el('div', { className: 'instrument-type', rect: box(167, 136, 200, 31) }, toolbar);
  el('button', { id: 'instrument-type-sample', attrs: { 'data-inst-type': 'sample', tabindex: '0' }, rect: box(167, 138, 60, 26) }, types);
  const pages = el('div', { className: 'inst-pages', attrs: { role: 'tablist' }, rect: box(417, 136, 400, 31) }, toolbar);
  el('button', { id: 'inst-page-tab-sample', attrs: { role: 'tab', tabindex: '0', 'data-inst-page-tab': 'sample' }, rect: box(417, 138, 80, 26) }, pages);
  el('button', { id: 'inst-page-tab-modulation', attrs: { role: 'tab', tabindex: '-1', 'data-inst-page-tab': 'modulation' }, rect: box(600, 138, 80, 26) }, pages);
  const scroll = el('div', { className: 'view-padding', rect: box(152, 167, 1348, 749) }, main);
  const layout = el('div', { className: 'instrument-layout', rect: box(152, 167, 1340, 1120) }, scroll);
  const sample = el('div', { id: 'inst-page-sample', className: 'inst-page', attrs: { 'data-inst-page': 'sample' }, rect: box(152, 167, 1340, 722) }, layout);
  const shift = granular ? 307 : 0, y = value => value + shift;
  if (granular) {
    const panel = el('section', { id: 'inst-group-granular', className: 'panel wide granular-panel', attrs: { 'aria-label': 'Granular' }, rect: box(152, 167, 1340, 307) }, sample);
    const body = el('div', { className: 'panel-body granular-body', rect: box(152, 203, 1339, 270) }, panel);
    param(h, body, 'grainPosition', 166, 215);
    ['grainSize', 'grainDensity', 'grainSpray', 'grainScan'].forEach((key, i) => param(h, body, key, 166 + i * 236, 301));
  }
  const editor = el('section', { id: 'sampler-editor', className: 'panel wide sampler-editor', rect: box(152, y(167), 1340, 383) }, sample);
  const head = el('div', { className: 'panel-head', rect: box(152, y(167), 1339, 39) }, editor);
  el('button', { id: 'sampler-editor-audition', rect: box(1364, y(174), 113, 24) }, head);
  const body = el('div', { className: 'panel-body stack', rect: box(152, y(206), 1339, 343) }, editor);
  const modes = el('div', { className: 'sampler-modes', attrs: { role: 'radiogroup' }, rect: box(166, y(218), 1311, 26) }, body);
  el('button', { id: 'sampler-mode-oneshot', attrs: { role: 'radio', tabindex: granular ? '-1' : '0', 'data-sampler-mode': 'oneshot' }, rect: box(166, y(218), 53, 26) }, modes);
  el('button', { id: 'sampler-mode-granular', attrs: { role: 'radio', tabindex: granular ? '0' : '-1', 'data-sampler-mode': 'granular' }, rect: box(607, y(218), 63, 26) }, modes);
  const strip = el('div', { className: 'sampler-editor-strip', rect: box(166, y(252), 1311, 26) }, body);
  el('button', { id: 'sampler-select-start', attrs: { tabindex: '-1' }, rect: box(166, y(252), 80, 26) }, strip);
  const wrap = el('div', { className: 'sampler-wave-wrap', rect: box(166, y(286), 1311, 190) }, body);
  el('canvas', { id: 'sampler-wave-canvas', className: 'sampler-wave', attrs: { tabindex: '0', role: 'slider' }, rect: box(166, y(286), 1311, 190) }, wrap);
  const row = el('div', { className: 'sampler-editor-row', rect: box(166, y(484), 1311, 26) }, body);
  el('button', { id: 'sampler-edit-trim', rect: box(166, y(484), 60, 26) }, row); el('button', { id: 'sampler-editor-zoom-in', rect: box(1350, y(484), 30, 26) }, row);
  group(h, sample, 'level', 'Volume · Pan · Tune', 152, y(550), 447, 233, ['volume', 'pan', 'tuneSemi', 'tuneFine']);
  group(h, sample, 'filter', 'Filter · Drive', 599, y(550), 447, 233, ['cutoff', 'resonance', 'drive', 'bitDepth'], 'inst-filterType');
  group(h, sample, 'amp', 'Amp envelope', 1045, y(550), 447, 233, ['attack', 'decay', 'sustain', 'release']);
  group(h, sample, 'playback', 'Region · Root note', 152, y(783), 1340, 106, ['rootNote', 'start', 'end']);
  // Modulation page: mounted, hidden.
  const modulation = el('div', { id: 'inst-page-modulation', className: 'inst-page', hidden: true, attrs: { 'data-inst-page': 'modulation' }, rect: box(152, 167, 1340, 614) }, layout);
  const modPanel = el('section', { id: 'inst-group-modulation', className: 'panel inst-group wide', rect: box(152, 167, 1340, 614) }, modulation);
  const consoleRoot = el('section', { className: 'modulation-console', rect: box(166, 215, 1311, 551) }, modPanel);
  const envelope = el('section', { className: 'mod-filter-section', rect: box(166, 245, 1311, 266) }, consoleRoot);
  ['filterAttack', 'filterDecay'].forEach((key, i) => {
    const p = el('div', { className: 'mod-parameter', rect: box(879 + i * 300, 250, 290, 60) }, envelope);
    el('input', { id: 'mod-number-0-' + key, type: 'number', attrs: { 'data-mod-param': key }, rect: box(1050 + i * 300, 252, 70, 24) }, p);
    el('input', { id: 'mod-0-' + key, type: 'range', attrs: { 'data-mod-param': key }, rect: box(885 + i * 300, 280, 260, 16) }, p);
  });
  const lfos = el('section', { className: 'mod-lfos', rect: box(166, 511, 1311, 228) }, consoleRoot);
  ['lfo', 'lfo2'].forEach((prefix, i) => {
    const lfo = el('div', { className: 'mod-lfo-row', attrs: { 'data-mod-lfo': prefix }, rect: box(166, 511 + i * 114, 1311, 114) }, lfos);
    el('select', { id: `mod-0-${prefix}Shape`, rect: box(250, 530 + i * 114, 100, 22) }, lfo);
  });
  const effects = el('section', { id: 'instrument-effects', className: 'panel wide instrument-effects', rect: box(152, y(889), 1340, 231) }, layout);
  chain(h, effects, 'instfx', y(937));
  const perform = el('div', { className: 'panel wide inst-perform', attrs: { 'aria-label': 'Play & perform' }, rect: box(152, y(1120), 1340, 167) }, layout);
  el('select', { id: 'instrument-octave', rect: box(1295, y(1128), 39, 18) }, perform);
  el('button', { id: 'piano-60', attrs: { 'data-piano': '60', tabindex: '0' }, rect: box(167, y(1185), 94, 63) }, perform);
  return { sample, modulation };
}

test('Instrument: the Sample page splits into named blocks in reading order; hidden pages drop out', () => {
  const h = harness('instrument'); const { sample, modulation } = instrument(h);
  deep(h.nav.blocks().map(b => [b.id, b.target.id]), [['transport', 'play'], ['views', 'view-instrument'], ['sidebar', 'sidebar-instrument-0'],
    ['view-actions', 'instrument-find'], ['instrument-type', 'instrument-type-sample'], ['instrument-pages', 'inst-page-tab-sample'],
    ['sample-file', 'sampler-editor-audition'], ['play-mode', 'sampler-mode-oneshot'], ['waveform', 'sampler-wave-canvas'], ['sample-edits', 'sampler-edit-trim'],
    ['level', 'param-volume'], ['filter', 'param-cutoff'], ['amp', 'param-attack'], ['region', 'param-rootNote'],
    ['instrument-effects', 'instfx-add'], ['perform', 'instrument-octave']]);
  assert.equal(h.nav.blocks().find(b => b.id === 'sidebar').label, 'Instruments');
  assert.match(h.nav.blocks().find(b => b.id === 'waveform').keys, /nudge marker/);
  // Switching to the Modulation page (pages only toggle hidden).
  sample.hidden = true; modulation.hidden = false;
  deep(h.nav.blocks().map(b => b.id).slice(6), ['mod-envelope', 'lfo-1', 'lfo-2', 'instrument-effects', 'perform']);
  assert.equal(h.nav.blocks().find(b => b.id === 'lfo-2').target.id, 'mod-0-lfo2Shape');
  // Granular mode: the Granular panel leads the Sample page.
  const g = harness('instrument'); instrument(g, { granular: true });
  deep(g.nav.blocks().map(b => [b.id, b.target.id]).slice(6, 8), [['granular', 'param-grainPosition'], ['sample-file', 'sampler-editor-audition']]);
  deep(h.calls, [], 'resolving blocks sends nothing');
});

test('Instrument: plain ⌥ + arrows move between blocks by position (the same moves as in WebKit)', () => {
  const h = harness('instrument'); instrument(h);
  h.nodes.get('inst-page-tab-sample').focus();
  deep(h.alt(['Down', 'Down', 'Down', 'Down', 'Down', 'Right', 'Right', 'Down', 'Up', 'Left']),
    [['Down', 'sample-file'], ['Down', 'play-mode'], ['Down', 'waveform'], ['Down', 'sample-edits'], ['Down', 'level'], ['Right', 'filter'], ['Right', 'amp'],
      ['Down', 'region'], ['Up', 'amp'], ['Left', 'filter']]);
  assert.equal(h.active(), 'param-cutoff'); assert(h.nav.showing);
  assert(h.document.activeElement.classes.has('block-nav-focus'), 'the focused control is marked while the frame shows');
  assert.equal(h.nodes.get('param-attack').classes.has('block-nav-focus'), false, 'only one control is marked');
  h.press('Escape'); assert.equal(h.nav.level?.id, 'filter', 'Esc backs out to the block');
  assert.equal(h.document.querySelectorAll('.block-nav-focus').length, 0, 'at block level the frame alone shows the selection');
  h.press('Enter'); assert.equal(h.active(), 'param-cutoff', 'Return goes back in');
  h.click(h.nodes.get('param-drive'));
  assert.equal(h.nav.showing, false); assert.equal(h.document.querySelectorAll('.block-nav-focus').length, 0, 'a click hides frame and mark');
});

test('Instrument: ← → step through a block\'s sliders and menus (value fields skipped); Return types a value', () => {
  const h = harness('instrument'); instrument(h);
  h.nav.focusBlock('filter');
  const steps = ['Right', 'Right', 'Right', 'Right', 'Left', 'Left', 'Left', 'Left', 'Left'].map(key => { const e = h.press('Arrow' + key); assert(e.defaultPrevented, key); return h.active(); });
  deep(steps, ['param-resonance', 'param-drive', 'param-bitDepth', 'param-bitDepth', 'param-drive', 'param-resonance', 'param-cutoff', 'inst-filterType', 'inst-filterType'],
    'reading order, stops at the ends (the slider never moves sideways), the filter menu is a stop');
  assert(h.press('ArrowRight').defaultPrevented); assert.equal(h.active(), 'param-cutoff', '→ on a menu steps on');
  assert.equal(h.press('ArrowUp').defaultPrevented, undefined, '↑ ↓ stay the slider\'s own (adjust)');
  assert(h.press('Enter').defaultPrevented); assert.equal(h.active(), 'param-value-cutoff', 'Return: type a value');
  assert.equal(h.press('ArrowLeft').defaultPrevented, undefined, 'the value field keeps its caret');
  assert(h.press('Enter').defaultPrevented); assert.equal(h.active(), 'param-cutoff', 'Return in the value field commits (blur) and goes back to the slider');
  // Modulation sliders work the same way; pane keys on other pages are untouched.
  h.nodes.get('inst-page-sample').hidden = true; h.nodes.get('inst-page-modulation').hidden = false;
  h.nav.focusBlock('mod-envelope'); assert.equal(h.active(), 'mod-0-filterAttack');
  h.press('ArrowRight'); assert.equal(h.active(), 'mod-0-filterDecay');
  h.press('Enter'); assert.equal(h.active(), 'mod-number-0-filterDecay');
  const song = harness('song'); const field = song.el('input', { id: 'swing', type: 'range', attrs: { 'data-inst-param': 'volume' }, rect: song.box(0, 200, 100, 16) });
  field.focus(); assert.equal(song.press('ArrowRight').defaultPrevented, undefined, 'only pages with { sliders: true }');
  deep(h.calls, [], 'moving between controls sends nothing to the engine');
});

test('Instrument: ⌥ ↑ ↓ fine-adjust only while you adjust; ⌥ ← → always move; the waveform keeps its own keys', () => {
  const h = harness('instrument'); instrument(h);
  const owns = (id, key) => h.nav.owns(h.nodes.get(id), key);
  h.nav.focusBlock('filter');
  assert(h.press('ArrowUp', { alt: true }).defaultPrevented, 'just arrived: ⌥↑ moves'); assert.equal(h.nav.current().id, 'sample-edits');
  h.press('ArrowDown', { alt: true }); assert.equal(h.nav.current().id, 'filter', 'straight back');
  h.press('ArrowUp'); // adjusting the cutoff
  assert.equal(owns('param-cutoff', 'ArrowUp'), true); assert.equal(owns('param-cutoff', 'ArrowLeft'), false, 'sliders never own ⌥ ← →');
  const fine = h.press('ArrowUp', { alt: true });
  assert.equal(fine.stopped, undefined, 'after ↑, ⌥↑ is fine adjustment'); assert.equal(h.active(), 'param-cutoff');
  assert(h.press('ArrowRight', { alt: true }).defaultPrevented, '⌥→ still moves on'); assert.equal(h.nav.current().id, 'amp');
  h.press('ArrowLeft', { alt: true }); h.press('ArrowUp'); h.press('Enter'); // typed a value: no longer adjusting
  assert.equal(h.active(), 'param-value-cutoff');
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented, 'after typing, ⌥↓ moves to the block below'); assert.equal(h.nav.current().id, 'region');
  h.nav.focusBlock('filter'); h.press('ArrowUp'); h.press('ArrowRight'); // ← → to another slider hands nothing on
  assert.equal(h.active(), 'param-resonance');
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented, 'a slider reached with → is not being adjusted');
  assert(h.press('ArrowDown', { alt: true, meta: true }).defaultPrevented, '⌥⌘ + arrows always move');
  // Waveform: ← → nudge, ⌥ ← → one frame once used; arriving there, ⌥ + arrows pass on.
  h.nav.focusBlock('waveform');
  for (const [key, mods] of [['ArrowLeft', {}], ['ArrowRight', { shift: true }], ['ArrowUp', {}], ['+', {}], ['0', {}]]) {
    assert.equal(h.press(key, mods).stopped, undefined, 'waveform keeps ' + key); assert.equal(h.active(), 'sampler-wave-canvas');
  }
  assert.equal(h.press('ArrowRight', { alt: true }).stopped, undefined, 'used: ⌥→ nudges the marker one frame');
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented, '⌥↓ is free on the waveform'); assert.equal(h.nav.current().id, 'sample-edits');
  h.press('ArrowUp', { alt: true }); assert.equal(h.nav.current().id, 'waveform');
  assert(h.press('ArrowLeft', { alt: true }).defaultPrevented, 'just arrived: ⌥← leaves the waveform'); assert.equal(h.nav.current().id, 'sidebar');
  // A click starts adjusting (⌥-drag is fine control as well).
  const c = harness('instrument'); instrument(c); c.nav.focusBlock('amp'); c.click(c.nodes.get('param-attack'));
  assert.equal(c.press('ArrowUp', { alt: true }).stopped, undefined, 'clicked: ⌥↑ fine-adjusts');
});

test('Plug-in instrument: slot tabs own ⌥ ← →, knobs leave them to block moves; NKS console', () => {
  const h = harness('instrument'); const { el, box } = h;
  const main = el('section', { className: 'workspace-main', rect: box(152, 88, 1348, 828) });
  const layout = el('div', { className: 'instrument-layout', rect: box(152, 167, 1340, 900) }, main);
  const overview = el('div', { className: 'panel wide instrument-plugin-overview', rect: box(152, 167, 1340, 100) }, layout);
  el('button', { id: 'browse-plugins', rect: box(900, 180, 120, 26) }, overview); el('button', { id: 'plugin-editor', rect: box(1030, 180, 120, 26) }, overview);
  const pp = el('section', { className: 'panel wide nks-console pp-console', attrs: { 'data-pp-root': '0' }, rect: box(152, 267, 1340, 360) }, layout);
  const head = el('div', { className: 'panel-head nks-heading', rect: box(152, 267, 1340, 40) }, pp);
  el('select', { id: 'pp-0-page', attrs: { 'data-pp-page': '' }, rect: box(1200, 275, 120, 24) }, head);
  const tabs = el('div', { className: 'pp-tabs', attrs: { role: 'tablist' }, rect: box(152, 307, 1340, 40) }, pp);
  el('button', { id: 'pp-0-tab-instrument', attrs: { role: 'tab', tabindex: '0', 'data-pp-tab': 'instrument', 'aria-selected': 'true' }, rect: box(166, 310, 160, 34) }, tabs);
  el('button', { id: 'pp-0-tab-effect-0', attrs: { role: 'tab', tabindex: '-1', 'data-pp-tab': 'effect:0' }, rect: box(330, 310, 160, 34) }, tabs);
  const body = el('div', { className: 'panel-body', rect: box(152, 347, 1340, 280) }, pp);
  for (let i = 0; i < 3; i++) el('input', { id: 'pp-knob-' + i, type: 'range', attrs: { 'data-pp-knob': String(i) }, rect: box(170 + i * 160, 400, 140, 16) }, body);
  const fx = el('section', { id: 'instrument-effects', className: 'panel wide instrument-effects', rect: box(152, 627, 1340, 231) }, layout);
  chain(h, fx, 'instfx', 660);
  deep(h.nav.blocks().map(b => [b.id, b.target.id]).slice(2), [['plugin', 'plugin-editor'], ['plugin-slots', 'pp-0-tab-instrument'], ['plugin-controls', 'pp-knob-0'], ['instrument-effects', 'instfx-add']]);
  h.nav.focusBlock('plugin-controls'); h.press('ArrowUp'); // adjusting a knob
  assert.equal(h.press('ArrowUp', { alt: true }).stopped, undefined, '⌥↑ fine-adjusts the knob');
  assert(h.press('ArrowRight', { alt: true }).defaultPrevented, '⌥→ on a knob wraps along the row (no plug-in switch)');
  h.nav.focusBlock('plugin-slots'); h.press('End'); // plugin-pages.js: the tab strip's own key (switch plug-in)
  assert.equal(h.nav.owns(h.nodes.get('pp-0-tab-instrument'), 'ArrowRight'), true);
  assert.equal(h.press('ArrowRight', { alt: true }).stopped, undefined, 'used slot tabs keep ⌥ ← → (switch plug-in)');
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented); assert.equal(h.nav.current().id, 'plugin-controls');
  assert.match(h.nav.blocks().find(b => b.id === 'plugin-slots').keys, /← →: plug-in/);
  assert(/on the slot tabs: plug-in/.test(web.read('plugin-pages.js')) && !/<kbd>⌥ ← →<\/kbd> plug-in/.test(web.read('plugin-pages.js')), 'the console hint says so');
  // NKS console (no plug-in pages): knobs are a block too.
  const n = harness('instrument');
  const l2 = n.el('div', { className: 'instrument-layout', rect: n.box(152, 167, 1340, 600) });
  const nks = n.el('section', { className: 'panel wide nks-console', attrs: { 'data-nks-root': '0' }, rect: n.box(152, 167, 1340, 300) }, l2);
  n.el('input', { id: 'nks-17', type: 'range', attrs: { 'data-nks-address': '17' }, rect: n.box(170, 220, 140, 16) }, nks);
  n.el('input', { id: 'nks-17-value', type: 'number', attrs: { 'data-nks-address': '17' }, rect: n.box(320, 216, 60, 24) }, nks);
  deep(n.nav.blocks().map(b => [b.id, b.target.id]).slice(2), [['nks-controls', 'nks-17']]);
  assert.equal(n.nav.owns(n.nodes.get('nks-17'), 'ArrowLeft'), false, 'NKS knobs never own ⌥ ← →');
});

// The Mixer at 1500 × 940 with one track, Master and one insert (studio.js, mixer-splitter.js, insert-chain.js).
function mixer(h, { inserts = ['Compressor', 'Maximizer'] } = {}) {
  const { el, box } = h;
  const work = el('div', { id: 'mixer-workspace', className: 'studio-mixer', rect: box(0, 88, 1500, 828) });
  const head = el('div', { className: 'panel-head', rect: box(18, 88, 1464, 56) }, work);
  el('button', { id: 'mixer-add-group', rect: box(1298, 105, 42, 22) }, head);
  const channels = el('div', { id: 'mixer-channels', className: 'studio-channels', rect: box(18, 144, 1464, 264) }, work);
  const strip = el('section', { className: 'studio-channel is-selected', attrs: { 'data-mixer-owner': 'track:0' }, rect: box(18, 145, 210, 262) }, channels);
  el('button', { id: 'mix-track-0', className: 'studio-channel-title', rect: box(30, 159, 185, 27) }, strip);
  el('input', { id: 'mix-vol-0', type: 'range', rect: box(121, 249, 22, 65) }, strip);
  const master = el('section', { className: 'studio-channel studio-master', rect: box(228, 145, 116, 262) }, channels);
  el('button', { id: 'mix-track-master', className: 'studio-channel-title', attrs: { tabindex: '-1' }, rect: box(241, 159, 91, 27) }, master);
  el('div', { id: 'mixer-splitter', className: 'studio-splitter', attrs: { tabindex: '0', role: 'separator' }, rect: box(18, 409, 1464, 10) }, work);
  const bottom = el('div', { className: 'studio-bottom', rect: box(18, 420, 1464, 496) }, work);
  const rack = el('section', { className: 'panel studio-rack', rect: box(18, 420, 826, 496) }, bottom);
  chain(h, rack, 'mixfx', 437, inserts);
  if (inserts.length) {
    const details = el('details', { className: 'studio-rack-details', rect: box(18, 800, 826, 30) }, rack);
    el('summary', { id: 'rack-details-summary', rect: box(18, 800, 826, 30) }, details);
  }
  const side = el('div', { className: 'studio-side', rect: box(870, 420, 612, 496) }, bottom);
  const routing = el('section', { className: 'studio-routing-panel', rect: box(871, 420, 611, 252) }, side);
  el('select', { id: 'routing-output', rect: box(970, 484, 500, 28) }, routing);
  const returns = el('section', { className: 'panel studio-returns', rect: box(871, 672, 611, 244) }, side);
  el('input', { id: 'delay-time', type: 'range', rect: box(895, 790, 280, 10) }, returns);
}

test('Mixer: strips, divider, insert chain, knob page, details, routing and returns are blocks; ⌥ moves by position', () => {
  const h = harness('mixer'); mixer(h);
  deep(h.nav.blocks().map(b => [b.id, b.target.id]).slice(2), [['mixer-actions', 'mixer-add-group'], ['channels', 'mix-track-0'], ['splitter', 'mixer-splitter'],
    ['inserts', 'mixfx-tile-0'], ['inserts-module', 'mixfx-knob-0'], ['insert-details', 'rack-details-summary'], ['routing', 'routing-output'], ['returns', 'delay-time']]);
  h.nav.focusBlock('channels');
  deep(h.alt(['Down', 'Down', 'Right', 'Left', 'Down', 'Up', 'Up', 'Up']),
    [['Down', 'splitter'], ['Down', 'inserts'], ['Right', 'routing'], ['Left', 'inserts'], ['Down', 'inserts-module'], ['Up', 'inserts'], ['Up', 'splitter'], ['Up', 'channels']]);
  assert.equal(h.nav.blocks().find(b => b.id === 'inserts-module').esc, 'Esc: modules', 'knobs use Esc themselves: the hint says where it goes');
  // Divider: plain ↑ ↓ (and ⇧, Home) are its own; ⌥ ↑ ↓ move on.
  h.nav.focusBlock('splitter');
  for (const [key, mods] of [['ArrowUp', {}], ['ArrowDown', { shift: true }], ['Home', {}]]) assert.equal(h.press(key, mods).stopped, undefined, 'divider keeps ' + key);
  assert.match(h.nav.blocks().find(b => b.id === 'splitter').keys, /resize/);
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented, 'used or not, the divider has no ⌥ keys'); assert.equal(h.nav.current().id, 'inserts');
  // Mixer sliders keep their arrows (← → change channel in studio.js): no slider stepping here.
  h.nodes.get('mix-vol-0').focus(); assert.equal(h.press('ArrowRight').defaultPrevented, undefined);
  // Without the insert chain (older rack): one Insert effects block.
  const o = harness('mixer'); const rack = o.el('section', { className: 'studio-rack', rect: o.box(18, 420, 826, 496) });
  const plugin = o.el('details', { className: 'studio-plugin', rect: o.box(30, 440, 600, 30) }, rack);
  o.el('summary', { id: 'plugin-header-0', rect: o.box(30, 440, 600, 30) }, plugin);
  deep(o.nav.blocks().map(b => [b.id, b.target.id]).slice(2), [['inserts', 'plugin-header-0']]);
  assert.equal(o.nav.owns(o.nodes.get('plugin-header-0'), 'ArrowUp'), true, 'its headers keep ⌥ ↑ ↓ (move insert) once used');
  deep(h.calls, [], 'moving between blocks sends nothing');
});

test('Mixer: insert tiles reorder with ⌥ ← → once used, also after ← → re-render the strip; knobs fine-adjust while adjusting', () => {
  const h = harness('mixer'); mixer(h);
  h.nav.focusBlock('inserts');
  assert(h.press('ArrowRight', { alt: true }).defaultPrevented, 'just arrived: ⌥→ moves on'); assert.equal(h.nav.current().id, 'routing');
  h.press('ArrowLeft', { alt: true }); assert.equal(h.active(), 'mixfx-tile-0');
  // → selects the next module: insert-chain.js re-renders and focuses the new tile from its own key handler.
  const tile = h.nodes.get('mixfx-tile-1');
  const event = h.press('ArrowRight'); tile.focus(); void event;
  assert.equal(h.nav.arrival, null, 'focus moved by the control\'s own key is not an arrival');
  assert.equal(h.press('ArrowRight', { alt: true }).stopped, undefined, '⌥→ reorders the selected module');
  assert(h.press('ArrowDown', { alt: true }).defaultPrevented, 'tiles own ⌥ ← → only'); assert.equal(h.nav.current().id, 'inserts-module');
  h.press('ArrowUp'); // knob adjusted
  assert.equal(h.press('ArrowUp', { alt: true }).stopped, undefined, '⌥↑ fine-adjusts the knob');
  assert(h.press('ArrowRight', { alt: true }).defaultPrevented); assert.equal(h.nav.current().id, 'routing');
  // An instrument page block switch (a page tab ↓ focusing a slider) does not count as adjusting it.
  const i = harness('instrument'); instrument(i);
  i.nodes.get('inst-page-tab-sample').focus(); i.press('ArrowDown'); i.nodes.get('param-volume').focus();
  assert(i.press('ArrowDown', { alt: true }).defaultPrevented, '⌥↓ after the page tab handed focus on moves blocks');
});

test('docs, help sheet and hints describe the Instrument and Mixer blocks', () => {
  const doc = require('node:fs').readFileSync(require('node:path').join(__dirname, '..', 'docs', 'KEYBOARD_NAVIGATION.md'), 'utf8');
  for (const word of ['Waveform', 'Amp envelope', 'LFO 1', 'Plug-in slots', 'Channel / detail divider', 'Selected insert', 'while adjusting']) assert(doc.includes(word), word);
  assert(!/do not enable plain ⌥ \+ arrows yet: their blocks/.test(doc), 'the doc no longer lists Instrument and Mixer as unaudited');
  const { app } = web.app();
  const help = JSON.parse(JSON.stringify(app.helpSections())).find(([name]) => /Blocks/.test(name));
  assert(help[1].some(([text]) => /Instrument/.test(text) && /Mixer/.test(text)), 'help names the pages with plain ⌥ + arrows');
  assert(help[1].some(([text]) => /type a value/i.test(text)), 'help: Return on a slider types a value');
});

for (const { name, run } of tests) { run(); console.log('PASS ' + name); }
console.log(`Block navigation (Instrument, Mixer): ${tests.length} regression groups passed.`);
