#!/usr/bin/env node
'use strict';

// Run with: node tests/fine_control_tests.js
// Perceptual slider curves, magnitude-relative steps and ⌥/⇧ fine drags (Resources/Web/fine-control.js).
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('fine-control.js');

function load() {
  const context = vm.createContext({ window: {}, Event: class { constructor(type, fields) { this.type = type; Object.assign(this, fields); } } });
  web.load(context, 'fine-control.js', { text: source });
  return context.window.STFine;
}
const close = (a, b, eps = 1e-9) => assert(Math.abs(a - b) < eps, `${a} != ${b}`);

// A range input with the parts bindDrag touches: 200 px wide, value attributes as strings.
function slider({ min = 0, max = 1, step = 'any', value = 0, dataset = {} } = {}) {
  const listeners = new Map(), events = [];
  const el = {
    type: 'range', min: String(min), max: String(max), step: String(step), value: String(value), dataset: { ...dataset }, disabled: false, isConnected: true, focused: false, captured: null, events,
    addEventListener(type, fn) { listeners.set(type, [...(listeners.get(type) || []), fn]); },
    dispatchEvent(event) { events.push({ type: event.type, value: el.value }); return true; },
    getBoundingClientRect: () => ({ left: 100, width: 212 }),  // 200 px of thumb travel with a 12 px thumb
    setPointerCapture(id) { el.captured = id; }, releasePointerCapture() { el.captured = null; }, focus() { el.focused = true; },
    fire(type, fields = {}) { const event = { type, button: 0, pointerId: 1, pointerType: 'mouse', clientX: 0, altKey: false, shiftKey: false, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...fields }; for (const fn of listeners.get(type) || []) fn(event); return event; }
  };
  return el;
}

const tests = [];
const test = (name, run) => tests.push({ name, run });

test('curves are monotonic, exact at the ends and invertible', () => {
  const F = load(), spec = { min: 0, max: 100, knee: .05 };
  let previous = -1;
  for (let i = 0; i <= 100; i++) { const value = F.fromPosition(i / 100, spec); assert(value > previous); previous = value; close(F.toPosition(value, spec), i / 100); }
  assert.equal(F.fromPosition(0, spec), 0); assert.equal(F.fromPosition(1, spec), 100);
  assert.equal(F.fromPosition(-1, spec), 0); assert.equal(F.fromPosition(2, spec), 100);
  assert.equal(F.toPosition(-5, spec), 0); assert.equal(F.toPosition(500, spec), 1);
  close(F.fromPosition(.5, { min: 10, max: 20 }), 15, 1e-12);  // no knee: linear
  const offset = { min: .001, max: 60, knee: .01 };
  assert.equal(F.fromPosition(0, offset), .001); close(F.fromPosition(F.toPosition(1.5, offset), offset), 1.5, 1e-12);
});

test('steps follow the magnitude: second digit, ⌥ third, ⇧ first; toward zero uses the decade below', () => {
  const F = load(), rate = { min: 0, max: 100, floor: .1 }, depth = { min: 0, max: 1, floor: .01 }, amount = { min: -48, max: 48, floor: 1 };
  const step = (value, direction, modifiers, spec = rate) => F.stepValue(value, direction, modifiers, spec);
  assert.equal(step(1.25, 1), 1.3); assert.equal(step(1.25, 1, { fine: true }), 1.26); assert.equal(step(1.25, 1, { coarse: true }), 2);
  assert.equal(step(12.5, 1), 13); assert.equal(step(1, -1), .99); assert.equal(step(1, -1, { fine: true }), .999); assert.equal(step(10, -1), 9.9);
  assert.equal(step(.05, 1), .06); assert.equal(step(.05, 1, { fine: true }), .051); assert.equal(step(0, 1), .01); assert.equal(step(0, -1), 0);
  assert.equal(step(99.5, 1, { coarse: true }), 100); assert.equal(step(1.234, 1), 1.3); assert.equal(step(1.234, -1), 1.2);
  assert.equal(step(.0125, 1, {}, depth), .013); assert.equal(step(.0125, 1, { fine: true }, depth), .0126); assert.equal(step(.25, -1, {}, depth), .24);
  assert.equal(step(.5, -1, {}, amount), .4); assert.equal(step(-.5, 1, {}, amount), -.4); assert.equal(step(0, -1, {}, amount), -.1); assert.equal(step(-12, -1, {}, amount), -13);
  // 100 presses from 0 cover the small values first, and ⇧ crosses decades quickly.
  let value = 0; for (let i = 0; i < 10; i++) value = step(value, 1); assert.equal(value, .1);
  value = .1; for (let i = 0; i < 9; i++) value = step(value, 1, { coarse: true }); assert.equal(value, 1);
});

test('drag rounding keeps three significant digits and readable formatting', () => {
  const F = load(), rate = { min: 0, max: 100, floor: .1 };
  assert.equal(F.quantize(1.23456, rate), 1.23); assert.equal(F.quantize(12.3456, rate), 12.3); assert.equal(F.quantize(.051234, rate), .051);
  assert.equal(F.quantize(.0004, rate), 0); assert.equal(F.quantize(0, rate), 0); assert.equal(F.quantize(-3.456, { min: -48, max: 48, floor: 1 }), -3.46);
  assert.equal(F.sig(.05), '0.05'); assert.equal(F.sig(1.25), '1.25'); assert.equal(F.sig(0.1 + 0.2), '0.3'); assert.equal(F.sig(-0.0000000001), '0');
});

test('element contract maps number fields, curved sliders and ⌥ key steps in engine units', () => {
  const F = load();
  const curved = slider({ dataset: { fineMin: '0', fineMax: '100', fineFloor: '.1', fineKnee: '.05' } });
  const number = { type: 'number', value: '25', dataset: { fineMin: '0', fineMax: '1', fineFloor: '.01', fineScale: '100' } };
  assert(F.has(curved) && F.curved(curved) && F.has(number) && !F.curved(number) && !F.has({ dataset: {} }));
  curved.value = F.elementFromLane(curved, .05); assert.equal(F.laneFromElement(curved), .05);
  assert.equal(F.laneFromElement(number), .25); assert.equal(F.elementFromLane(number, .0125), '1.25');
  assert.equal(F.stepElement(number, 1, { fine: true }), .251); assert.equal(number.value, '25.1');
  curved.value = F.elementFromLane(curved, 1.25); assert.equal(F.stepElement(curved, -1, {}), 1.2); assert.equal(F.laneFromElement(curved), 1.2);
  assert.equal(F.elementFromLane(number, 4), '100', 'clamped to the lane range');
});

test('⌥/⇧-drag moves at a fraction of the pointer speed and commits once', () => {
  const F = load(), el = slider({ min: 0, max: 1, step: '0.001', value: .5 });
  F.bindDrag(el); F.bindDrag(el);
  // Thumb centre for .5: 100 + 6 + 100 = 206. ⌥-press on the track does not jump.
  let down = el.fire('pointerdown', { clientX: 300, altKey: true });
  assert(down.defaultPrevented); assert(el.focused); assert.equal(el.captured, 1); assert.equal(el.value, '0.5');
  el.fire('pointermove', { clientX: 400, altKey: true });
  assert.equal(el.value, '0.55', '100 px with ⌥ = 1/10 of 0.5'); assert.deepEqual(el.events.map(e => e.type), ['input']);
  el.fire('pointermove', { clientX: 420, altKey: true, shiftKey: true }); assert.equal(el.value, '0.552', '⌥⇧ = 1/50');
  el.fire('pointermove', { clientX: 440, shiftKey: true }); assert.equal(el.value, '0.577', '⇧ = 1/4, switchable mid-drag');
  el.fire('pointermove', { clientX: 450 }); assert.equal(el.value, '0.627', 'plain = full speed');
  el.fire('pointerup', { clientX: 450 }); el.fire('lostpointercapture');
  assert.deepEqual(el.events.map(e => e.type), ['input', 'input', 'input', 'input', 'change'], 'one change ends the gesture');
  // Sub-step fine moves accumulate instead of being lost.
  el.events.length = 0; el.fire('pointerdown', { clientX: 0, altKey: true });
  for (let i = 0; i < 4; i++) el.fire('pointermove', { clientX: i + 1, altKey: true });
  assert.equal(el.value, '0.629', 'four 0.0005 moves = 0.002');
  el.fire('pointerup');
  assert.deepEqual(el.events.map(e => e.type), ['input', 'input', 'change']);
});

test('plain clicks jump like the native slider, thumb grabs do not, and nothing changes without movement', () => {
  const F = load(), el = slider({ min: 0, max: 1, value: .5 });
  F.bindDrag(el);
  el.fire('pointerdown', { clientX: 106 + 50 }); assert.equal(el.value, '0.25'); el.fire('pointerup');
  assert.deepEqual(el.events.map(e => e.type), ['input', 'change']);
  el.events.length = 0; el.fire('pointerdown', { clientX: 106 + 50 + 3 }); el.fire('pointerup');
  assert.deepEqual(el.events, [], 'grabbing the thumb and releasing is not an edit');
  el.fire('pointerdown', { clientX: 400, pointerType: 'touch' }); el.fire('pointerdown', { clientX: 400, button: 2 });
  el.disabled = true; el.fire('pointerdown', { clientX: 400 });
  assert.equal(el.value, '0.25', 'touch, secondary buttons and disabled sliders keep native behaviour');
  const position = slider({ dataset: { fineMin: '0', fineMax: '100', fineKnee: '.05', fineFloor: '.1' } }); F.bindDrag(position);
  position.fire('pointerdown', { clientX: 106 + 100 }); position.fire('pointerup');
  assert(F.laneFromElement(position) > 1 && F.laneFromElement(position) < 3, 'mid-slider on a rate curve is a couple of Hz');
  assert.equal(slider({ dataset: {} }).dataset.fineDrag, undefined);
});

(async () => {
  for (const { name, run } of tests) { await run(); process.stdout.write(`PASS ${name}\n`); }
  process.stdout.write(`${tests.length} fine control suites passed.\n`);
})().catch(error => { console.error(error); process.exitCode = 1; });
