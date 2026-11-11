'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const S = require('../ui/send-effects.js');
const project = (extra = {}) => ({ bpm: 120, tracks: [{ name: 'A', delaySend: .2, reverbSend: 0 }, { name: 'B' }], delay: { time: .375, feedback: .35, mix: .3 }, reverb: { size: .7, damping: .4, mix: .25 }, ...extra });
const tests = [];
const test = (name, fn) => tests.push({ name, fn });
const spec = (group, key) => S.RETURNS[group].find(item => item.key === key);

test('synced delay stores a division, its quarter-note length and a compatible time', () => {
  const p = project();
  S.setDivision(p, '1/8.');
  assert.deepEqual([p.delay.sync, p.delay.beats], ['1/8.', .75]); assert.equal(p.delay.time, .375);
  assert.equal(S.delaySeconds({ ...p, bpm: 150 }), .3); // follows tempo
  S.setFreeTime(p, .2); assert.deepEqual([p.delay.sync, p.delay.beats, p.delay.time], [null, 0, .2]);
  assert.equal(S.nearestDivision(p, .25), '1/8');
  assert.throws(() => S.setDivision(p, '1/5'), /Unknown/);
  // Unrelated delay fields survive edits.
  assert.equal(p.delay.feedback, .35); assert.equal(p.delay.mix, .3);
});

test('arrow adjustment steps divisions in sync mode and milliseconds in free mode', () => {
  const p = project(), time = spec('delay', 'time'), timing = spec('delay', 'sync');
  S.adjust(p, timing, 1); assert.equal(p.delay.sync, '1/8.'); // free .375 s at 120 BPM -> dotted eighth
  S.adjust(p, time, 1); assert.equal(p.delay.sync, '1/4T');
  S.adjust(p, time, -1, 10); assert.equal(p.delay.sync, '1/8T');
  S.adjust(p, timing, 1); assert.equal(p.delay.sync, null);
  const before = p.delay.time; S.adjust(p, time, 1); assert.equal(Math.round((p.delay.time - before) * 1000), 10);
  S.adjust(p, time, -1, 10); assert.equal(Math.round((p.delay.time - before) * 1000), -40);
  S.setExact(p, time, '1/16'); assert.equal(p.delay.sync, '1/16');
  S.setExact(p, time, '500 ms'); assert.equal(p.delay.time, .5); assert.equal(p.delay.sync, null);
  assert.throws(() => S.setExact(p, time, 'soon'), /milliseconds/);
});

test('return parameters clamp, accept typed percent/ms values and reset to defaults', () => {
  const p = project(), feedback = spec('delay', 'feedback'), tone = spec('delay', 'tone'), predelay = spec('reverb', 'predelay'), damping = spec('reverb', 'damping');
  for (let n = 0; n < 200; n++) S.adjust(p, feedback, 1); assert.equal(p.delay.feedback, .95);
  S.setExact(p, tone, '40%'); assert.equal(p.delay.tone, .4); assert.equal(S.toneHz(.4), '1.3 kHz'); assert.equal(S.toneHz(1), 'Open');
  S.setExact(p, predelay, '30'); assert.equal(p.reverb.predelay, .03);
  S.adjust(p, predelay, 1, 10); assert.equal(p.reverb.predelay, .04);
  S.setExact(p, predelay, '900'); assert.equal(p.reverb.predelay, .25);
  S.adjust(p, damping, -1, .1); assert.equal(p.reverb.damping, .399);
  S.reset(p, feedback, 'delay'); assert.equal(p.delay.feedback, .35);
  S.reset(p, predelay, 'reverb'); assert.equal(p.reverb.predelay, 0);
});

test('track send specs edit delaySend/reverbSend only', () => {
  const p = project(), delay = S.sendSpec(0, 'delay'), reverb = S.sendSpec(1, 'reverb');
  S.adjust(p, delay, 1, 10); assert.equal(p.tracks[0].delaySend, .3);
  S.setExact(p, reverb, '55'); assert.equal(p.tracks[1].reverbSend, .55);
  S.reset(p, delay, 'delay-send'); assert.equal(p.tracks[0].delaySend, 0);
  assert.equal(p.tracks[0].reverbSend, 0); assert.equal(p.tracks[1].delaySend, undefined);
});

test('dialog keys coalesce bursts into one undo step and move within a column', () => {
  const labels = [], syncs = [];
  const nodes = new Map();
  function row(id, group, index, parent) {
    const node = { id, dataset: { sendGroup: group, sendIndex: String(index) }, tabIndex: -1, tagName: 'DIV', parentElement: parent,
      value: { textContent: '' }, bar: { style: {} }, attrs: {}, closest(selector) { return selector === '[data-send-group]' ? this : null; },
      querySelector(selector) { return selector === '.send-value' ? this.value : this.bar; }, setAttribute(name, value) { this.attrs[name] = value; }, focus() { document.activeElement = this; }, scrollIntoView() {}, addEventListener() {} };
    nodes.set(id, node); parent.children.push(node); return node;
  }
  const group = name => ({ name, children: [], querySelectorAll() { return this.children; } });
  const delayGroup = group('delay'), sendGroup = group('delay-send');
  S.RETURNS.delay.forEach((_, index) => row(`send-delay-${index}`, 'delay', index, delayGroup));
  [0, 1].forEach(index => row(`send-delay-send-${index}`, 'delay-send', index, sendGroup));
  const root = { listeners: {}, addEventListener(type, fn) { this.listeners[type] = fn; }, querySelectorAll: () => [...nodes.values()], querySelector: () => null };
  const document = { readyState: 'complete', activeElement: null, getElementById: id => id === 'send-fx' ? root : id === 'send-fx-close' ? {} : nodes.get(id) || null, addEventListener() {} };
  const state = { modal: null, project: project() };
  const ST = { state, escapeHTML: String, toast() {}, markDirty() {}, syncEngine: () => syncs.push(1), closeModal() {}, openModal(title, body, footer, options) { state.modal = options.kind; },
    mutate(label, fn) { labels.push(label); fn(state.project); } };
  let now = 1000;
  const context = vm.createContext({ window: { ST }, document, setTimeout: fn => fn(), Date: { now: () => now }, JSON, Math, Number, String, Error });
  web.load(context, 'send-effects.js');
  const sends = context.window.STSends;
  assert(sends.show({ track: 1, bus: 'delay' })); assert.equal(document.activeElement.id, 'send-delay-send-1');
  const key = (target, k, extra = {}) => root.listeners.keydown({ key: k, target, preventDefault() { this.defaultPrevented = true; }, ...extra });
  const feedback = nodes.get('send-delay-2');
  key(feedback, 'ArrowRight'); now += 100; key(feedback, 'ArrowRight'); now += 100; key(feedback, 'ArrowRight', { shiftKey: true });
  assert.equal(state.project.delay.feedback, .47); assert.equal(labels.length, 1); assert.equal(syncs.length, 2);
  now += 5000; key(feedback, 'ArrowLeft'); assert.equal(labels.length, 2);
  key(feedback, 'ArrowDown'); assert.equal(document.activeElement.id, 'send-delay-3'); assert.equal(nodes.get('send-delay-3').tabIndex, 0); assert.equal(feedback.tabIndex, -1);
  key(nodes.get('send-delay-send-0'), '0'); assert.equal(state.project.tracks[0].delaySend, 0);
  assert.equal(feedback.value.textContent, '46%');
});

test('mixer context menu offers send levels for the channel', () => {
  const state = { view: 'mixer', modal: null, project: project() }, labels = [];
  const ST = { state, escapeHTML: String, mutate(label, fn) { labels.push(label); fn(state.project); } };
  const context = vm.createContext({ window: { ST }, document: { readyState: 'complete', addEventListener() {}, getElementById: () => null }, setTimeout() {}, Date, JSON, Math, Number, String, Error });
  web.load(context, 'send-effects.js');
  const target = { closest(selector) { if (selector === '#mixer-workspace') return {}; if (selector === '[id^="mix-"]') return { id: 'mix-reverb-1' }; return null; } };
  const items = context.window.STSends.contextItems({ target });
  const reverb = items.find(item => item.title?.startsWith('Reverb send'));
  reverb.items.find(item => item.title === '50%').action();
  assert.equal(state.project.tracks[1].reverbSend, .5); assert.deepEqual(labels, ['reverb send']);
  assert.equal(context.window.STSends.contextItems({ target: { closest: () => null } }), null);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) { try { await fn(); console.log('PASS', name); } catch (error) { failed++; console.error('FAIL', name); console.error(error); } }
  if (failed) process.exit(1);
})();
