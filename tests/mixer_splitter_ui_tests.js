'use strict';
// Mixer splitter (mixer-splitter.js): drag resizes the channel detail within limits, ↑ / ↓ resize when focused,
// double-click and Home reset, the height persists as a per-user preference (not a project edit), and the
// clamp follows window resizes (1100×760 included). The mixer layout is a small fake with measurable heights.
const assert = require('node:assert/strict'), vm = require('node:vm');
const web = require('./helpers/web');

function element(id, className = '') {
  const node = {
    id, get className() { return [...this.classes].join(' '); }, set className(value) { this.classes = new Set(String(value).split(/\s+/).filter(Boolean)); }, tabIndex: -1, title: '', innerHTML: '', children: [], parentNode: null, handlers: {}, attributes: {}, style: {
      values: {}, setProperty(key, value) { this.values[key] = value; }, removeProperty(key) { delete this.values[key]; }
    },
    classes: new Set(className.split(/\s+/).filter(Boolean)),
    get classList() { const c = this.classes; return { add: (...x) => x.forEach(v => c.add(v)), remove: (...x) => x.forEach(v => c.delete(v)), contains: v => c.has(v), toggle: v => c.has(v) ? c.delete(v) : c.add(v) }; },
    setAttribute(key, value) { this.attributes[key] = String(value); }, getAttribute(key) { return this.attributes[key] ?? null; },
    addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }, removeEventListener(type, fn) { this.handlers[type] = (this.handlers[type] || []).filter(x => x !== fn); },
    setPointerCapture() {}, focus() {},
    fire(type, extra = {}) { const event = { type, button: 0, pointerId: 1, preventDefault() { this.prevented = true; }, stopPropagation() {}, ...extra }; for (const fn of [...(this.handlers[type] || [])]) fn(event); return event; },
    insertBefore(child, before) { child.parentNode = this; this.children.splice(this.children.indexOf(before), 0, child); },
    querySelector(selector) {
      const match = /^:scope > \.([\w-]+)$/.exec(selector);
      return match ? this.children.find(child => child.classes.has(match[1])) || null : null;
    }
  };
  return node;
}

function harness(height = 900) {
  const store = new Map();
  const root = element('mixer-workspace', 'studio-mixer'), head = element('', 'panel-head'), strips = element('mixer-channels', 'studio-channels'), bottom = element('', 'studio-bottom');
  for (const child of [head, strips, bottom]) { child.parentNode = root; root.children.push(child); }
  const layout = { height };
  Object.defineProperty(root, 'clientHeight', { get: () => layout.height });
  head.offsetHeight = 56;
  // The detail's rendered height: its sized height, else its natural (default) height of 330 px.
  const natural = 330;
  bottom.getBoundingClientRect = () => ({ height: bottom.classes.has('is-sized') ? parseFloat(bottom.style.values['--mixer-detail-height']) : Math.min(natural, layout.height - 56 - 170) });
  const resize = [];
  const document = {
    body: element('body'),
    getElementById: id => id === 'mixer-workspace' ? root : null,
    createElement: () => { const node = element(''); Object.defineProperty(node, 'offsetHeight', { get: () => 12 }); return node; }
  };
  const window = { addEventListener: (type, fn) => { if (type === 'resize') resize.push(fn); } };
  const context = vm.createContext({ window, document, console, Number, String, Math, Map, JSON, Object, Array,
    localStorage: { getItem: key => store.has(key) ? store.get(key) : null, setItem: (key, value) => store.set(key, String(value)), removeItem: key => store.delete(key) } });
  web.load(context, 'mixer-splitter.js');
  const S = window.STMixerSplitter;
  const splitter = () => root.children.find(child => child.classes.has('studio-splitter'));
  const detail = () => bottom.getBoundingClientRect().height;
  return { S, root, bottom, store, layout, splitter, detail, resize: h => { layout.height = h; for (const fn of resize) fn(); } };
}

const tests = [], test = (name, fn) => tests.push({ name, fn });

test('the splitter mounts between strips and detail as a focusable separator with a grab affordance', () => {
  const h = harness(); h.S.bind();
  const s = h.splitter();
  assert(s, 'mounted'); assert.equal(h.root.children.indexOf(s), h.root.children.indexOf(h.bottom) - 1, 'right above the detail panel');
  assert.equal(s.attributes.role, 'separator'); assert.equal(s.attributes['aria-orientation'], 'horizontal'); assert.equal(s.tabIndex, 0);
  assert.match(s.innerHTML, /studio-splitter-grip/); assert.match(s.title, /Drag/);
  assert(!h.bottom.classes.has('is-sized'), 'the default layout until the user resizes');
  h.S.bind(); assert.equal(h.root.children.filter(c => c.classes.has('studio-splitter')).length, 1, 'a re-render does not add a second splitter');
});

test('dragging resizes the detail within limits and persists only on release', () => {
  const h = harness(); h.S.bind();
  const s = h.splitter(), start = h.detail();
  s.fire('pointerdown', { clientY: 500 });
  s.fire('pointermove', { clientY: 400 });
  assert.equal(h.detail(), start + 100, 'dragging up grows the detail');
  assert.equal(h.store.get(h.S.KEY), undefined, 'nothing is saved mid-drag');
  s.fire('pointermove', { clientY: -2000 });
  const [min, max] = h.S.limits();
  assert.equal(h.detail(), max, 'the strips keep their minimum height');
  assert.equal(max, 900 - 56 - 12 - h.S.MIN_STRIPS);
  s.fire('pointermove', { clientY: 5000 });
  assert.equal(h.detail(), min, 'the detail keeps its chain tiles');
  s.fire('pointerup', { clientY: 450 });
  assert.equal(h.detail(), start + 50); assert.equal(h.store.get(h.S.KEY), String(start + 50), 'saved as a UI preference');
  assert(h.root.classes.has('has-sized-detail'));
});

test('arrow keys resize the focused splitter; double-click and Home reset to the default', () => {
  const h = harness(); h.S.bind();
  const s = h.splitter(), start = h.detail();
  s.fire('keydown', { key: 'ArrowUp' }); assert.equal(h.detail(), start + 24);
  s.fire('keydown', { key: 'ArrowDown', shiftKey: true }); assert.equal(h.detail(), start + 24 - 96);
  assert.equal(h.store.get(h.S.KEY), String(start - 72));
  assert.equal(s.attributes['aria-valuenow'], String(start - 72));
  s.fire('dblclick'); assert(!h.bottom.classes.has('is-sized')); assert.equal(h.store.has(h.S.KEY), false);
  s.fire('keydown', { key: 'ArrowUp' }); s.fire('keydown', { key: 'Home' });
  assert(!h.bottom.classes.has('is-sized')); assert.equal(h.store.has(h.S.KEY), false);
});

test('the saved height returns after a re-render and is clamped (not overwritten) in a small window', () => {
  const h = harness(); h.S.bind();
  h.S.set(800);
  assert.equal(h.detail(), 900 - 56 - 12 - h.S.MIN_STRIPS, 'clamped to the window');
  h.S.set(560); h.S.bind(); assert.equal(h.detail(), 560, 'restored after a re-render');
  h.resize(760); // 1100×760
  assert.equal(h.detail(), 760 - 56 - 12 - h.S.MIN_STRIPS, 'a smaller window clamps the detail');
  assert.equal(h.store.get(h.S.KEY), '560', 'the preference survives the clamp');
  h.resize(900); assert.equal(h.detail(), 560, 'growing the window restores it');
  h.resize(300); assert.equal(h.detail(), h.S.MIN_DETAIL, 'a tiny window keeps the chain tiles');
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} mixer splitter tests failed`); process.exit(1); }
  console.log(`${tests.length} mixer splitter tests passed`);
})();
