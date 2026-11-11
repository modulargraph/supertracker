'use strict';
// The production sampler runs against the same parsed markup/event approach as
// sampler_audition_ui_tests, with destructive app-like refresh and native Tab
// departure modeled explicitly. No native audio, UI, or sample files are used.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const source = web.source('sampler.js');
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 32; i++) await Promise.resolve(); };

function harness() {
  const nodes = new Map(), elements = [], timers = new Map(), documentEvents = new Map(), windowEvents = new Map(), requests = [], mutations = [], notices = [];
  let nextTimer = 0, renders = 0, module;
  const document = {
    activeElement: null, body: null,
    getElementById: id => nodes.get(id) || null,
    querySelector(selector) { return selector.startsWith('#') ? this.getElementById(selector.slice(1)) : this.querySelectorAll(selector)[0] || null; },
    querySelectorAll(selector) { return elements.filter(element => element.matches(selector)); },
    addEventListener(type, fn) { const list = documentEvents.get(type) || []; list.push(fn); documentEvents.set(type, list); },
    fire(type, event = {}) { for (const fn of documentEvents.get(type) || []) fn({ type, ...event }); },
  };
  class Element {
    constructor(tag, attrs = '', parent = null) {
      Object.assign(this, { tagName: tag.toUpperCase(), parent, parentElement: parent, attrs: {}, dataset: {}, listeners: new Map(), isConnected: true, value: '', type: '', min: '', max: '', disabled: /\sdisabled(?:\s|$)/.test(attrs), hidden: false, tabIndex: 0 });
      for (const match of attrs.matchAll(/([\w-]+)="([^"]*)"/g)) {
        const [_, key, value] = match; this.attrs[key] = value;
        if (['value', 'type', 'min', 'max', 'step'].includes(key)) this[key] = value;
        if (key === 'tabindex') this.tabIndex = Number(value);
        if (key.startsWith('data-')) this.dataset[key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = value;
      }
      this.id = this.attrs.id || `anonymous-${elements.length}`;
      const classes = new Set((this.attrs.class || '').split(' '));
      this.classList = { contains: name => classes.has(name), toggle(name, on) { if (on) classes.add(name); else classes.delete(name); } };
    }
    matches(selector) {
      return selector.split(',').some(part => {
        part = part.trim();
        if (part.startsWith('#')) return this.id === part.slice(1);
        if (part.startsWith('.')) return this.classList.contains(part.slice(1));
        const attr = /^\[([^=\]]+)(?:="?([^\]"]+)"?)?\]$/.exec(part);
        if (attr) return Object.hasOwn(this.attrs, attr[1]) && (attr[2] === undefined || this.attrs[attr[1]] === attr[2]);
        return this.tagName === part.toUpperCase();
      });
    }
    closest(selector) { for (let element = this; element; element = element.parent) if (element.matches(selector)) return element; return null; }
    contains(element) { for (; element; element = element.parent) if (element === this) return true; return false; }
    querySelectorAll(selector) { return elements.filter(element => element !== this && this.contains(element) && element.matches(selector)); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    addEventListener(type, fn) { const list = this.listeners.get(type) || []; list.push(fn); this.listeners.set(type, list); }
    fire(type, extra = {}) {
      const event = { type, target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra };
      for (let element = this; element; element = ['blur', 'focus'].includes(type) ? null : element.parent) {
        for (const fn of element.listeners.get(type) || []) fn(event);
        if (event.stopped) break;
      }
      if (!event.stopped) document.fire(type, event);
      return event;
    }
    focus() {
      if (this.disabled || !this.isConnected || document.activeElement === this) return;
      const previous = document.activeElement; document.activeElement = this;
      previous?.fire('blur', { relatedTarget: this }); previous?.fire('focusout', { relatedTarget: this });
      this.fire('focus'); this.fire('focusin');
    }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key] ?? null; }
    scrollIntoView(options) { (this.scrollCalls ||= []).push(options); } select() {}
  }
  document.body = new Element('body', ' id="body"'); document.activeElement = document.body;
  const outside = new Element('button', ' id="outside"');
  const instrument = { id: 1, type: 'sample', samplePath: '/fixture/break.wav', rootNote: 60, start: 0, end: 1, mode: 'slice', sliceCount: 4, slicePoints: [0, .25, .5, .75], zones: [{ samplePath: '/fixture/zone.wav', rootNote: 60, keyLow: 0, keyHigh: 127, velocityLow: 0, velocityHigh: 127, tune: 0, volume: 1 }] };
  const state = { project: { instruments: [{ id: 0, type: 'synth' }, instrument], tracks: [{ instrument: 1 }], audioPlugins: [] }, view: 'instrument', instrument: 1, track: 0, modal: null, selection: null };
  const ST = {
    state, escapeHTML: value => String(value),
    mutate(label, fn, options = {}) { mutations.push({ label, before: copy(state.project), options: { ...options } }); fn(state.project); if (options.render !== false) ST.refresh(); },
    refresh() { mount(); },
    request(command, payload) { requests.push({ command, payload: copy(payload) }); return Promise.resolve({ ok: true, auditioning: payload?.step?.note >= 0 }); },
    async syncEngineProject(factory) { const project = factory(); return project ? ST.request('engine.project', { project }) : false; },
    toast(message, type) { notices.push({ message, type }); }, showRecording() {},
  };
  const window = { ST, addEventListener(type, fn) { const list = windowEvents.get(type) || []; list.push(fn); windowEvents.set(type, list); } };
  web.runText('sampler.js', source, { window, document, queueMicrotask, setTimeout(fn, delay = 0) { timers.set(++nextTimer, { fn, delay }); return nextTimer; }, clearTimeout(id) { timers.delete(id); } }, { filename: 'sampler.js' });
  module = window.STSampler;
  function mount() {
    renders++; const previous = document.activeElement, focusID = previous?.id;
    for (const element of elements) element.isConnected = false;
    elements.length = 0; nodes.clear(); if (previous !== outside) document.activeElement = document.body;
    const html = state.view === 'instrument' ? module.render(state.project.instruments[state.instrument]) : '', stack = [];
    for (const match of html.matchAll(/<(\/)?([a-z][\w-]*)\b([^>]*)>/gi)) {
      const tag = match[2].toLowerCase();
      if (match[1]) { while (stack.length && stack.pop().tagName !== tag.toUpperCase()) {} continue; }
      const element = new Element(tag, match[3], stack.at(-1)); elements.push(element); nodes.set(element.id, element);
      if (!['input', 'br', 'hr', 'img'].includes(tag)) stack.push(element);
    }
    module.bind(); if (focusID) nodes.get(focusID)?.focus();
  }
  function key(target, key, extra = {}) {
    const event = target.fire('keydown', { key, ...extra }); event.handled = module.key(event); return event;
  }
  function depart(sourceID, destinationID, value, shiftKey = false) {
    const input = nodes.get(sourceID), destination = nodes.get(destinationID); assert(input && destination);
    input.focus(); input.value = String(value); input.fire('input');
    const event = key(input, 'Tab', { shiftKey }); assert(!event.defaultPrevented, 'Tab must retain native browser navigation'); assert(!event.handled);
    document.activeElement = document.body; input.fire('change'); input.fire('blur'); input.fire('focusout', { relatedTarget: destination });
    if (destination.isConnected) destination.focus();
    return { input, destination };
  }
  function flush() { for (const [id, timer] of [...timers]) if (timer.delay === 0) { timers.delete(id); timer.fn(); } }
  mount();
  return { ST, state, instrument, module, nodes, document, outside, requests, mutations, notices, timers, mount, key, depart, flush, get renders() { return renders; } };
}

const tests = [], test = (name, run) => tests.push({ name, run });
test('marker forward and backward Tab commit once before paint and preserve native destinations', () => {
  for (const [destination, shift] of [['sampler-remove-marker-1', false], ['sampler-audition-1', true]]) {
    const h = harness(), before = h.renders;
    h.depart('sampler-marker-1', destination, 31.25, shift);
    assert.equal(h.instrument.slicePoints[1], .3125); assert.equal(h.mutations.length, 1); assert.equal(h.renders, before, 'change must not replace native Tab destination');
    assert.equal(h.document.activeElement.id, destination); h.flush(); assert.equal(h.document.activeElement.id, destination);
    assert.equal(h.nodes.get('sampler-marker-1').value, '31.250'); assert.equal(h.requests.length, 0);
  }
});
test('zone forward and backward Tab use parsed limits and commit exactly one history entry', () => {
  for (const [destination, shift] of [['sampler-zone-0-keyHigh', false], ['sampler-zone-0-rootNote', true]]) {
    const h = harness(), input = h.nodes.get('sampler-zone-0-keyLow'); assert.equal(input.min, '0'); assert.equal(input.max, '127');
    const before = h.renders; h.depart(input.id, destination, 64, shift);
    assert.equal(h.instrument.zones[0].keyLow, 64); assert.equal(h.mutations.length, 1); assert.equal(h.renders, before);
    assert.equal(h.document.activeElement.id, destination); h.flush(); assert.equal(h.document.activeElement.id, destination); assert.equal(h.requests.length, 0);
  }
});
test('native change with focus retained commits without moving focus; marker Return still commits and auditions', async () => {
  const h = harness(), zone = h.nodes.get('sampler-zone-0-tune'); zone.focus(); zone.value = '-7.25'; zone.fire('input'); zone.fire('change');
  assert.equal(h.instrument.zones[0].tune, -7.25); h.flush(); assert.equal(h.document.activeElement.id, zone.id);
  const marker = h.nodes.get('sampler-marker-1'); marker.focus(); marker.value = '32'; marker.fire('input'); const event = h.key(marker, 'Enter');
  assert(event.handled && event.defaultPrevented); assert.equal(h.instrument.slicePoints[1], .32); assert.equal(h.mutations.length, 2);
  await tick(); assert.equal(h.requests.filter(request => request.command === 'step.preview' && request.payload.step.note >= 0).length, 1);
  assert.equal(h.requests.find(request => request.command === 'engine.project').payload.project.instruments[1].slicePoints[1], .32);
  assert.equal(h.document.activeElement.id, 'sampler-marker-1'); h.flush(); assert.equal(h.document.activeElement.id, 'sampler-marker-1');
});
test('pending repaint preserves a new numeric draft and follows changed slice selection', () => {
  const h = harness(); h.depart('sampler-zone-0-keyLow', 'sampler-zone-0-keyHigh', 64);
  const draft = h.document.activeElement; assert.equal(draft.id, 'sampler-zone-0-keyHigh'); assert([...h.timers.values()].some(timer => timer.delay === 0));
  draft.value = ''; draft.fire('input'); const before = h.renders; h.flush();
  assert.equal(h.renders, before); assert.equal(h.document.activeElement, draft); assert.equal(draft.value, ''); assert.equal(h.instrument.zones[0].keyHigh, 127);
  h.outside.focus(); h.flush(); assert.equal(h.renders, before + 1); assert.equal(h.document.activeElement, h.outside);
  assert.equal(h.nodes.get('sampler-zone-0-keyHigh').value, '127'); assert.equal(h.mutations.length, 1, 'Leaving an unfinished draft must not create another history entry');
  const selection = harness(); selection.depart('sampler-marker-1', 'sampler-remove-marker-1', 30);
  selection.nodes.get('sampler-audition-2').focus(); selection.flush();
  assert.equal(selection.document.activeElement.id, 'sampler-audition-2'); assert(selection.nodes.get('sampler-row-2').classList.contains('is-selected'));
  assert.equal(selection.nodes.get('sampler-marker-2').tabIndex, 0); assert.equal(selection.nodes.get('sampler-marker-1').tabIndex, -1);
});
test('deferred repaint cannot steal focus after project, instrument, view or modal ownership changes', () => {
  for (const change of [h => h.state.project = copy(h.state.project), h => h.state.project.instruments[1] = { ...h.instrument }, h => h.state.instrument = 0, h => h.state.view = 'pattern', h => h.state.modal = 'dialog']) {
    const h = harness(); h.depart('sampler-marker-1', 'sampler-remove-marker-1', 30); assert([...h.timers.values()].some(timer => timer.delay === 0)); h.outside.focus(); const before = h.renders;
    change(h); h.flush(); assert.equal(h.renders, before); assert.equal(h.document.activeElement, h.outside); assert.equal(h.requests.length, 0);
  }
});
test('pointer departure keeps the intended button alive through pointerup and click', () => {
  const h = harness(), input = h.nodes.get('sampler-marker-1'), destination = h.nodes.get('sampler-equal');
  input.focus(); input.value = '30'; input.fire('input'); documentPointer('pointerdown');
  h.document.activeElement = h.document.body; input.fire('change'); input.fire('blur'); if (destination.isConnected) destination.focus();
  const before = h.renders; h.flush(); assert.equal(h.renders, before); assert(destination.isConnected);
  let clicks = 0; destination.addEventListener('click', () => clicks++); documentPointer('pointerup'); destination.fire('click'); assert.equal(clicks, 1); h.flush();
  assert.equal(h.document.activeElement.id, 'sampler-equal');
  function documentPointer(type) { h.document.fire(type, { target: destination }); }
});

test('zone focus reveals the existing table viewport without changing draft, cursor, focus or sound', () => {
  const h = harness(), field = h.nodes.get('sampler-zone-0-volume'), table = h.nodes.get('sampler-zone-scroll'), before = copy(h.state.project);
  field.value = '.37'; field.selectionStart = 1; field.selectionEnd = 3; field.focus();
  assert.equal(h.document.activeElement, field); assert.equal(field.value, '.37'); assert.equal(field.selectionStart, 1); assert.equal(field.selectionEnd, 3);
  assert.deepEqual(copy(field.scrollCalls), [{block:'nearest',inline:'nearest'}]);
  assert.deepEqual(copy(table.scrollCalls), [{block:'nearest',inline:'nearest'}]);
  assert.deepEqual(h.state.project, before); assert.equal(h.mutations.length, 0); assert.equal(h.requests.length, 0);
  field.fire('input'); field.fire('keydown', {key:'ArrowUp'}); assert.equal(table.scrollCalls.length, 1, 'Only focus entry reveals context');
});

test('zone context references preserve complete sample identity and reject stale focus events', () => {
  const h = harness(); h.instrument.zones[0].samplePath = '/fixture/A very long original multisample recording with its full filename.wav'; h.mount();
  const field = h.nodes.get('sampler-zone-0-rootNote'), name = h.nodes.get('sampler-zone-name-0'), table = h.nodes.get('sampler-zone-scroll');
  assert.equal(field.getAttribute('aria-describedby'), name.id);
  assert.equal(name.getAttribute('aria-label'), 'Replace sample for zone 1: A very long original multisample recording with its full filename.wav');
  assert.equal(name.getAttribute('title'), h.instrument.zones[0].samplePath);
  h.state.view = 'pattern'; field.focus(); assert.equal(table.scrollCalls, undefined); assert.equal(field.scrollCalls, undefined);
  assert.equal(h.mutations.length, 0); assert.equal(h.requests.length, 0);
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`Sampler field focus: ${tests.length - failures}/${tests.length} regression groups passed.`); if (failures) process.exitCode = 1;
})();
