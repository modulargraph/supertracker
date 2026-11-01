'use strict';
// Pure simulator DOM/input contract: no application, native bridge, audio, or hardware.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const dataKey = name => name.replace(/^data-/, '').replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
class Node {
  constructor(tag) {
    this.tagName = tag; this.children = []; this.dataset = {}; this.attributes = {}; this.listeners = new Map(); this.className = ''; this._text = '';
    this.style = { setProperty(name, value) { this[name] = value; } };
    this.classList = {
      contains: name => this.className.split(/\s+/).includes(name),
      toggle: (name, force) => { const on = force == null ? !this.classList.contains(name) : force; const names = this.className.split(/\s+/).filter(n => n && n !== name); if (on) names.push(name); this.className = names.join(' '); return on; },
      remove: name => this.classList.toggle(name, false)
    };
  }
  set textContent(value) { this._text = String(value); this.children = []; }
  get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter(node => node !== this); this.parent = null; }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  addEventListener(name, fn) { if (!this.listeners.has(name)) this.listeners.set(name, []); this.listeners.get(name).push(fn); }
  fire(name, props = {}) { const event = { target: this, preventDefault() {}, pointerId: 1, clientX: 20, clientY: 20, button: 0, ...props }; for (const fn of this.listeners.get(name) || []) fn(event); }
  setPointerCapture() {}
  getBoundingClientRect() { return { left: 0, top: 0, width: 100, height: 100 }; }
  matches(selector) {
    if (selector.startsWith('#')) return this.id === selector.slice(1);
    if (selector.startsWith('.')) return this.classList.contains(selector.slice(1));
    const attr = selector.match(/^\[(data-[\w-]+)(?:(\^?=)"([^"]*)")?\]$/);
    if (attr) { const value = this.dataset[dataKey(attr[1])]; return attr[2] === '=' ? value === attr[3] : attr[2] === '^=' ? value?.startsWith(attr[3]) : value !== undefined; }
    return this.tagName === selector;
  }
  querySelectorAll(selector) {
    const descendants = node => node.children.flatMap(child => [child, ...descendants(child)]);
    const parts = selector.split(' '), candidates = descendants(this);
    return candidates.filter(node => selector.split(',').some(part => {
      if (parts.length === 1) return node.matches(part);
      if (!node.matches(parts[1])) return false;
      for (let parent = node.parent; parent && parent !== this; parent = parent.parent) if (parent.matches(parts[0])) return true;
      return false;
    }));
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
}
const document = { body: new Node('body'), createElement: tag => new Node(tag), readyState: 'complete' };
const events = [];
// Entries: track 1 alone, then the group "Night kit" (tracks 2–4); the cursor is on its second member.
let subscriber;
const night = { name: 'Night kit' };
const project = { tracks: [{ name: 'Bass' }, { name: 'Kick' }, { name: 'Rim accent' }, { name: 'Hat' }] };
const entries = [{ group: null, index: 0, members: [0] }, { group: night, index: 1, members: [1, 2, 3] }];
const appState = { project, track: 2 };
const controller = {
  state: { screen: 'channel', padMode: 'pad', padAlt: false, padBank: { pad: 0 }, bank: 0, connected: true },
  entries: () => entries, groupOf: () => entries.find(entry => entry.members.includes(appState.track))?.group || null, page: () => ({ data: {} }),
  event(name, payload) { assert.equal(name, 'mk3.input'); events.push(payload); },
  subscribe(fn) { subscriber = fn; return () => { subscriber = null; }; },
  publish() { subscriber?.({ leds: { buttons: {}, groups: Array(8).fill('#ed7144'), pads: Array(16).fill('#ed7144'), strip: [], nav: {} }, screens: { left: [], right: [] }, screenChanged: true }); },
  cancelGestures() { throw new Error('Simulator must not cancel physical hardware gestures'); }
};
const window = { STMK3: controller, ST: { state: appState }, location: { search: '' } };
web.run('mk3-sim.js', { window, document, setTimeout() {} });
const sim = window.STMK3Sim, root = sim.open();
const button = name => root.querySelector(`[data-mk3-button="${name}"]`), pad = n => root.querySelector(`[data-mk3-pad="${n}"]`);
// Group buttons name their entries; the entry holding the cursor track is selected.
assert.equal(button('groupA').querySelector('.mk3-group-name').textContent, 'Bass');
assert.equal(button('groupB').querySelector('.mk3-group-name').textContent, 'Night kit');
assert.equal(button('groupB').title, 'Group B · Night kit');
assert(button('groupB').classList.contains('selected'));
assert(!button('groupA').classList.contains('selected'));
assert.equal(button('groupC').querySelector('.mk3-group-name').textContent, '');
// PAD MODE in a group: pads name its member tracks; the cursor member is selected.
assert.equal(pad(2).querySelector('.mk3-pad-number').textContent, '2');
assert.equal(pad(2).querySelector('.mk3-pad-name').textContent, 'Rim accent');
assert.equal(pad(2).title, 'Pad 2 · Rim accent');
assert(pad(2).classList.contains('selected'));
assert.equal(pad(1).querySelector('.mk3-pad-name').textContent, 'Kick');
assert(!pad(1).classList.contains('selected'));
assert.equal(pad(4).querySelector('.mk3-pad-name').textContent, '');
// Outside a group the pads carry no names.
appState.track = 0; controller.publish();
assert.equal(pad(2).querySelector('.mk3-pad-name').textContent, '');
assert.equal(pad(2).title, 'Pad 2');
assert(!pad(2).classList.contains('selected'));
assert(!button('groupB').classList.contains('selected'));
assert(button('groupA').classList.contains('selected'));
// Other pad modes do not name pads either.
appState.track = 2; controller.state.padMode = 'keyboard'; controller.publish();
assert.equal(pad(2).querySelector('.mk3-pad-name').textContent, '');
controller.state.padMode = 'pad';

button('volume').fire('pointerdown', { shiftKey: true });
button('lock').fire('pointerdown', { altKey: true });
button('tempo').fire('pointerdown'); button('tempo').fire('pointerup');
pad(9).fire('pointerdown'); pad(9).fire('pointermove', { pressure: .7 });
root.querySelector('[data-mk3-encoder="0"]').fire('pointerenter');
root.querySelector('[data-mk3-main]').fire('pointerenter'); root.querySelector('[data-mk3-main]').fire('pointerdown');
root.querySelector('[data-mk3-strip]').fire('pointerdown');
root.querySelector('[data-mk3-pedal]').fire('change', { target: { checked: true } });
const beforeClose = events.length;
sim.close();
const releases = events.slice(beforeClose);
assert.deepEqual(releases.map(event => JSON.parse(JSON.stringify(event))), [
  { type: 'pad', pad: 9, phase: 'up', pressure: 0, cancelled: true, targetOnly: true },
  { type: 'touch', target: 'encoder1', touched: false, cancelled: true, targetOnly: true }, { type: 'touch', target: 'main', touched: false, cancelled: true, targetOnly: true }, { type: 'touch', target: 'strip', touched: false, cancelled: true, targetOnly: true },
  { type: 'pedal', down: false },
  { type: 'button', name: 'volume', pressed: false, cancelled: true, targetOnly: true }, { type: 'button', name: 'lock', pressed: false, cancelled: true, targetOnly: true }, { type: 'button', name: 'encoderPush', pressed: false, cancelled: true, targetOnly: true }, { type: 'button', name: 'shift', pressed: false, cancelled: true, targetOnly: true }
]);
assert.equal(sim.isOpen, false); assert.equal(subscriber, null); assert.equal(document.body.children.length, 0);
const closedCount = events.length;
sim.close(); root.querySelector('[data-mk3-main]').fire('pointerleave');
assert.equal(events.length, closedCount, 'Closed panel cannot emit stale releases');
sim.open(); sim.close();
assert.equal(events.length, closedCount, 'Reopening starts without held simulator input');
const reopened = sim.open();
reopened.querySelector('[data-mk3-button="shift"]').fire('pointerdown');
sim.close();
assert.deepEqual(events.slice(-2).map(event => JSON.parse(JSON.stringify(event))), [{ type: 'button', name: 'shift', pressed: true }, { type: 'button', name: 'shift', pressed: false, cancelled: true, targetOnly: true }], 'Sticky Shift is released on close');
console.log('PASS MK3 simulator group and member names, selection, and owned input release on close');
