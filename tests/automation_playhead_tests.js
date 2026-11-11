'use strict';
// Actual studio module, minimal DOM boundary. Run: node tests/automation_playhead_tests.js
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('studio.js');
const copy = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function harness(length = 64) {
  const nodes = new Map(), groups = new Map(), mutations = [], requests = [], timers = new Map();
  let timerID = 0;
  let refreshes = 0, focusCalls = 0, rafCalls = 0;
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null,
    querySelectorAll: selector => groups.get(selector) || [], querySelector: selector => (groups.get(selector) || [])[0] || null };
  class Element {
    constructor(id, dataset = {}) {
      this.id = id; this.dataset = dataset; this.style = {}; this.hidden = false; this.textContent = ''; this.value = '';
      this.listeners = new Map(); this.classes = new Set(); this.html = ''; this.htmlWrites = 0; this.scrollTop = 127;
      this.classList = { toggle: (key, enabled) => enabled ? this.classes.add(key) : this.classes.delete(key), contains: key => this.classes.has(key) };
    }
    set innerHTML(value) { this.html = value; this.htmlWrites++; }
    get innerHTML() { return this.html; }
    addEventListener(type, fn) { const handlers = this.listeners.get(type) || []; handlers.push(fn); this.listeners.set(type, handlers); }
    removeEventListener(type, fn) { this.listeners.set(type, (this.listeners.get(type) || []).filter(item => item !== fn)); }
    dispatch(type, values = {}) {
      const event = { type, target: this, key: '', button: 0, pointerId: 1, preventDefault() { this.prevented = true; }, stopPropagation() {}, ...values };
      for (const handler of [...(this.listeners.get(type) || [])]) handler(event);
      return event;
    }
    focus() { document.activeElement = this; focusCalls++; }
    getBoundingClientRect() { return { left: 0, top: 0, width: 900, height: 274 }; }
    setPointerCapture() {}
  }
  const node = (id, dataset) => { const el = new Element(id, dataset); nodes.set(id, el); return el; };
  for (const id of ['automation-envelope', 'automation-playhead', 'automation-position', 'automation-playback-context', 'automation-transport']) node(id);
  const points = [{ row: 0, value: .25 }, { row: Math.min(16, length - 1), value: .75 }];
  if (length === 1) points.splice(1, 1);
  const lane = { id: 'lane', track: 0, target: 'volume', name: 'Level', interpolation: 'linear', points };
  const state = { view: 'automation', track: 0, instrument: 0, selectedLane: 0, row: 7, playing: false, status: {}, project: complete({
    activePattern: 23, linesPerBeat: 4, tracks: [{ name: 'Bass', instrument: 0, volume: .8 }], instruments: [{ type: 'synth' }], audioPlugins: [],
    patterns: [{ id: 23, name: 'Verse', length, automation: [lane] }, { id: 41, name: 'Chorus <other>', length: 32, automation: [] }]
  }) };
  const rowNodes = points.map((_, i) => node(`table-row-${i}`)); groups.set('.studio-point-row', rowNodes);
  const inputs = points.flatMap((p, i) => [node(`point-row-${i}`, { pointRow: String(i) }), node(`point-value-${i}`, { pointValue: String(i) })]);
  for (const [i, input] of inputs.entries()) input.value = String(i % 2 ? points[Math.floor(i / 2)].value : points[Math.floor(i / 2)].row);
  groups.set('[data-point-row],[data-point-value]', inputs);
  const ST = { state, escapeHTML, refresh() { refreshes++; }, toast() {},
    mutate(label, fn, options = {}) { mutations.push({ label, before: copy(state.project) }); fn(state.project); if (options.render !== false) refreshes++; },
    request(command, payload) { requests.push({ command, payload }); return Promise.resolve([]); }
  };
  const context = { window: { ST }, document, requestAnimationFrame: fn => { rafCalls++; fn(); }, setTimeout: (fn, delay = 0) => { timers.set(++timerID,{fn,delay}); return timerID; }, clearTimeout: id => timers.delete(id) };
  web.runText('studio.js', source, context, { filename: 'studio.js' });
  const module = context.window.STStudio;
  module.renderAutomation(); module.bindAutomation();
  return { module, state, lane, nodes, document, mutations, requests, rowNodes, inputs,
    flushFields() { for (const [id,timer] of [...timers]) if (timer.delay === 0) { timers.delete(id); timer.fn(); } },
    get refreshes() { return refreshes; }, get focusCalls() { return focusCalls; }, get rafCalls() { return rafCalls; },
    tick(payload, type = 'status') { state.status = payload; state.playing = !!payload.playing; module.event(type, payload); },
    key(key, options = {}) { return nodes.get('automation-envelope').dispatch('keydown', { key, ...options }); }
  };
}
const tests = [];
const test = (name, run) => tests.push({ name, run });
const close = (actual, expected) => assert(Math.abs(actual - expected) < 1e-8, `${actual} != ${expected}`);
const headLeft = h => parseFloat(h.nodes.get('automation-playhead').style.left);
const positionLabel = h => h.nodes.get('automation-position').textContent;

test('native fractional status drives the current pattern without a render', () => {
  const h = harness();
  h.tick({ playing: true, pattern: 23, row: 16, position: 16.5 });
  assert.equal(h.nodes.get('automation-playhead').hidden, false);
  close(headLeft(h), (96 + 16.5 / 64 * 780) / 9);
  assert.equal(positionLabel(h), 'Playing · Row 16.50 / 64');
  assert.equal(h.nodes.get('automation-transport').dataset.mode, 'playing');
  assert.equal(h.refreshes, 0); assert.equal(h.mutations.length, 0); assert.equal(h.requests.length, 0);
  const html = h.module.renderAutomation();
  assert(html.includes('id="automation-playhead"')); assert(html.includes('aria-live="off"'));
  assert(html.includes('data-envelope-point="0"')); assert(html.includes('Option plus left or right'));
});

test('full last-row progress and a loop wrap use exact endpoints', () => {
  const h = harness();
  h.tick({ playing: true, pattern: 23, row: 63, position: 63.75 });
  const late = headLeft(h); assert(late > (96 + 63 / 64 * 780) / 9); assert(late < 876 / 9);
  h.tick({ playing: true, pattern: 23, row: 0, position: .125 });
  close(headLeft(h), (96 + .125 / 64 * 780) / 9);
  assert.equal(positionLabel(h), 'Playing · Row 00.13 / 64');
  h.tick({ playing: true, pattern: 23, row: 63, position: 64 }); close(headLeft(h), 876 / 9);
  h.tick({ playing: true, pattern: 23, row: 99, position: 999 }); close(headLeft(h), 876 / 9);
  h.tick({ playing: true, pattern: 23, row: 0, position: -2 }); close(headLeft(h), 96 / 9);
});

test('the row and fraction of a status stand in for a missing position', () => {
  const h = harness();
  h.tick({ playing: true, pattern: 23, row: 4, position: 4.5 });
  close(headLeft(h), (96 + 4.5 / 64 * 780) / 9); assert.equal(positionLabel(h), 'Playing · Row 04.50 / 64');
  h.tick({ playing: true, pattern: 23, row: 5, position: 5.25 });
  close(headLeft(h), (96 + 5.25 / 64 * 780) / 9);
  h.tick({ playing: true, pattern: 23, row: 6 }); assert.equal(positionLabel(h), 'Playing · Row 06.00 / 64');
});

test('other patterns and stop hide the line and explain what is happening', () => {
  const h = harness(); h.tick({ playing: true, pattern: 41, row: 8, position: 8.5 });
  assert.equal(h.nodes.get('automation-playhead').hidden, true);
  assert.equal(positionLabel(h), 'Row 08.50 · Playing Chorus <other>');
  assert.equal(h.nodes.get('automation-playback-context').textContent, 'Viewing Verse');
  assert.equal(h.state.project.activePattern, 23);
  assert(h.module.renderAutomation().includes('Playing Chorus &lt;other&gt;'));
  h.tick({ playing: false, pattern: 23, row: 8, position: 8.5 });
  assert.equal(h.nodes.get('automation-playhead').hidden, true); assert.equal(positionLabel(h), 'Stopped · Edit row 07');
  h.tick({ playing: true, row: 8 }); assert.equal(h.nodes.get('automation-playhead').hidden, true);
  assert(positionLabel(h).includes('Waiting for pattern position'));
});

test('one-row and maximum-length patterns preserve fractional time geometry', () => {
  for (const length of [1, 256]) {
    const h = harness(length); h.tick({ playing: true, pattern: 23, row: 0, position: length / 2 });
    close(headLeft(h), (96 + 390) / 9); assert(Number.isFinite(headLeft(h)));
  }
});

test('ticks preserve the edit draft, focus, point selection and scroll position', () => {
  const h = harness(), input = h.nodes.get('point-value-1'), svg = h.nodes.get('automation-envelope');
  input.focus(); input.dispatch('focus'); input.value = '0.432';
  const writes = svg.htmlWrites, focus = h.focusCalls, raf = h.rafCalls, before = copy(h.lane.points);
  for (let i = 0; i < 200; i++) h.tick({ playing: true, pattern: i % 2 ? 41 : 23, row: i % 32, position: i % 32 + .5 });
  assert.equal(h.document.activeElement, input); assert.equal(input.value, '0.432'); assert.equal(input.scrollTop, 127);
  assert.equal(svg.htmlWrites, writes); assert.equal(h.focusCalls, focus); assert.equal(h.rafCalls, raf);
  assert.equal(h.rowNodes[1].classes.has('is-selected'), true); assert.deepEqual(copy(h.lane.points), before);
  assert.equal(h.refreshes, 0); assert.equal(h.mutations.length, 0);
});

test('point navigation updates graph/table selection without moving focus or creating undo', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope'); svg.focus();
  h.key('End'); assert(h.rowNodes[1].classes.has('is-selected')); assert(!h.rowNodes[0].classes.has('is-selected'));
  h.key('ArrowLeft', { altKey: true }); assert(h.rowNodes[0].classes.has('is-selected'));
  h.key('ArrowRight', { altKey: true }); assert(h.rowNodes[1].classes.has('is-selected'));
  h.key('Home'); assert(h.rowNodes[0].classes.has('is-selected'));
  assert.equal(h.document.activeElement, svg); assert.equal(h.mutations.length, 0); assert.equal(h.refreshes, 0);
});

test('arrow edits capture the original point for undo and retain envelope focus', () => {
  const h = harness(), original = copy(h.lane.points); h.key('ArrowRight');
  assert.deepEqual(h.mutations[0].before.patterns[0].automation[0].points, original);
  assert.equal(h.lane.points[0].row, 1); assert.equal(h.document.activeElement.id, 'automation-envelope');
  const beforeValue = h.lane.points[0].value; h.key('ArrowUp');
  assert.equal(h.mutations[1].before.patterns[0].automation[0].points[0].value, beforeValue);
  assert(h.lane.points[0].value > beforeValue);
});

test('numeric row edits retain original undo data while reordering selection/focus', () => {
  const h = harness(), input = h.nodes.get('point-row-0'), original = copy(h.lane.points);
  input.focus(); input.value = '20'; input.dispatch('change');
  assert.deepEqual(h.mutations[0].before.patterns[0].automation[0].points, original);
  assert.deepEqual(copy(h.lane.points.map(p => p.row)), [16, 20]); assert.equal(h.refreshes,0); assert.equal(h.document.activeElement,input);
  h.flushFields(); assert.equal(h.document.activeElement.id, 'point-row-1');
});

test('drag preview is isolated from the project and cancel preserves source points', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope'), before = copy(h.lane.points);
  svg.dispatch('pointerdown', { target: { dataset: { envelopePoint: '0' } }, clientX: 96, clientY: 200 });
  svg.dispatch('pointermove', { clientX: 400, clientY: 90 });
  assert.deepEqual(copy(h.lane.points), before); assert.equal(h.mutations.length, 0);
  svg.dispatch('pointercancel'); assert.deepEqual(copy(h.lane.points), before); assert.equal(h.mutations.length, 0);
});

test('drag commit uses full-pattern coordinates and captures exactly one undo', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope'), before = copy(h.lane.points);
  svg.dispatch('pointerdown', { target: { dataset: { envelopePoint: '0' } }, clientX: 96, clientY: 200 });
  svg.dispatch('pointermove', { clientX: 876, clientY: 90 }); svg.dispatch('pointerup');
  assert.equal(h.mutations.length, 1); assert.deepEqual(h.mutations[0].before.patterns[0].automation[0].points, before);
  assert.equal(h.lane.points.at(-1).row, 63); assert.equal(h.document.activeElement.id, 'automation-envelope');
});

test('canceling a new point restores valid keyboard selection', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope'), before = copy(h.lane.points);
  h.key('End');
  svg.dispatch('pointerdown', { clientX: 450, clientY: 100 });
  svg.dispatch('pointermove', { clientX: 700, clientY: 100 }); svg.dispatch('pointercancel');
  assert.deepEqual(copy(h.lane.points), before); assert.equal(h.mutations.length, 0);
  assert(h.rowNodes[1].classes.has('is-selected'));
  h.key('ArrowUp'); assert(h.lane.points[1].value > before[1].value);
});

test('selecting an existing point creates no edit or undo entry', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope');
  svg.dispatch('pointerdown', { target: { dataset: { envelopePoint: '1' } }, clientX: 291, clientY: 132 });
  svg.dispatch('pointerup');
  assert(h.rowNodes[1].classes.has('is-selected')); assert.equal(h.mutations.length, 0); assert.equal(h.refreshes, 0);
});

test('drag highlight follows its point when crossing another breakpoint', () => {
  const h = harness(), svg = h.nodes.get('automation-envelope');
  svg.dispatch('pointerdown', { target: { dataset: { envelopePoint: '0' } }, clientX: 96, clientY: 200 });
  svg.dispatch('pointermove', { clientX: 700, clientY: 100 });
  const selected = svg.innerHTML.match(/<circle class="studio-envelope-point is-selected"[^>]*><title>Row (\d+)/);
  assert(selected); assert(Number(selected[1]) > 16); assert.equal(h.mutations.length, 0);
});

let failed = 0;
for (const { name, run } of tests) {
  try { run(); console.log(`PASS ${name}`); } catch (error) { failed++; console.error(`FAIL ${name}\n${error.stack}`); }
}
if (failed) process.exitCode = 1;
else console.log(`Automation playhead and editing: ${tests.length} checks passed.`);
