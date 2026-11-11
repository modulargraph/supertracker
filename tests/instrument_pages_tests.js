'use strict';
// Instrument pages (Sample · Slices · Zones · Modulation · Voice & Record): the real module in
// instrument-navigation.js over a small DOM. Switching pages only toggles visibility and focus.
const assert = require('node:assert/strict'), fs = require('node:fs'), vm = require('node:vm');
const web = require('./helpers/web');
const source = web.source('instrument-navigation.js');
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function harness(type = 'sample', mode = 'slice') {
  let document;
  class Element {
    constructor(tag, attrs = {}) {
      this.tagName = tag.toUpperCase(); this.attrs = { ...attrs }; this.children = []; this.parentElement = null; this.isConnected = true; this.dataset = {}; this.style = {};
      for (const [key, value] of Object.entries(attrs)) if (key.startsWith('data-')) this.dataset[key.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
      this.tabIndex = attrs.tabindex !== undefined ? Number(attrs.tabindex) : ['BUTTON', 'INPUT', 'SELECT', 'CANVAS'].includes(this.tagName) && !(this.tagName === 'CANVAS' && attrs.tabindex === undefined) ? 0 : -1;
      this.classList = { toggle: (name, on) => { const names = new Set((this.attrs.class || '').split(' ').filter(Boolean)); on ? names.add(name) : names.delete(name); this.attrs.class = [...names].join(' '); } };
    }
    get id() { return this.attrs.id || ''; } get type() { return this.attrs.type || ''; } get disabled() { return Object.hasOwn(this.attrs, 'disabled'); }
    get hidden() { return Object.hasOwn(this.attrs, 'hidden'); } set hidden(on) { if (on) this.attrs.hidden = ''; else delete this.attrs.hidden; }
    append(child) { child.parentElement = this; this.children.push(child); return child; }
    contains(node) { return node === this || this.children.some(child => child.contains(node)); }
    getAttribute(name) { return this.attrs[name] ?? null; } setAttribute(name, value) { this.attrs[name] = String(value); }
    matches(selector) {
      return selector.split(',').some(part => {
        part = part.trim().replace(/:not\(:disabled\)/g, () => { if (this.disabled) return '\u0000'; return ''; });
        if (part.includes('\u0000')) return false;
        const tag = /^[a-z][\w-]*/i.exec(part)?.[0]; if (tag && this.tagName !== tag.toUpperCase()) return false;
        const id = /#([\w-]+)/.exec(part)?.[1]; if (id && this.id !== id) return false;
        for (const [, name, value] of part.matchAll(/\[([^=\]]+)(?:="([^"]*)")?\]/g)) if (!Object.hasOwn(this.attrs, name) || value !== undefined && this.attrs[name] !== value) return false;
        if (/:not\(\[type="range"\]\)/.test(part) && this.type === 'range') return false;
        return true;
      });
    }
    closest(selector) { for (let node = this; node; node = node.parentElement) if (node.matches(selector)) return node; return null; }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    focus() { if (!this.disabled) document.activeElement = this; } scrollIntoView() {}
  }
  const n = (tag, attrs, children = []) => { const node = new Element(tag, attrs); children.forEach(child => node.append(child)); return node; };
  const body = n('body'), main = body.append(n('main', { id: 'main' })), scroller = main.append(n('div', { 'data-scroll': 'instrument' }));
  scroller.scrollTop = 120;
  document = { body, activeElement: body, getElementById: id => body.querySelectorAll('[id]').find(node => node.id === id) || null, querySelectorAll: selector => body.querySelectorAll(selector) };
  const inst = { type, mode, slicePoints: [0, .25, .5, .75], zones: [{}, {}] };
  const state = { view: 'instrument', modal: null, instrument: 0, track: 0, project: { instruments: [inst] } };
  let redraws = 0, playheadSyncs = 0;
  const window = { ST: { state, escapeHTML, toast() {} }, STSampler: { redraw: () => redraws++ }, STSamplerPlayheads: { sync: () => playheadSyncs++ }, getComputedStyle: () => ({}) };
  web.runText('instrument-navigation.js', source, { window, document });
  const pages = window.STInstrumentPages;
  // Mount the bar and page containers as app.js would.
  const bar = scroller.append(n('div', { class: 'inst-pages' }));
  for (const match of pages.renderBar(inst).matchAll(/<button ([^>]*)>/g)) {
    const attrs = {}; for (const [, key, value] of match[1].matchAll(/([\w-]+)="([^"]*)"/g)) attrs[key] = value;
    bar.append(n('button', attrs));
  }
  const contents = {
    sample: [...(mode === 'wavetable' ? [n('input', { id: 'param-value-wavetablePosition', type: 'number' })] : []), n('canvas', { id: 'sampler-wave-canvas', tabindex: '0' })],
    slices: [n('button', { id: 'sampler-audition-0', 'data-sampler-audition': '0', tabindex: '-1' }), n('button', { id: 'sampler-audition-1', 'data-sampler-audition': '1', tabindex: '0' })],
    zones: [n('input', { id: 'sampler-zone-0-keyLow', type: 'number', 'data-sampler-zone-field': 'keyLow' })],
    modulation: [n('input', { id: 'mod-0-lfoRate', type: 'range' }), n('input', { id: 'mod-number-0-lfoRate', type: 'number' })],
    voice: [n('select', { id: 'sampler-voice-mode' }), n('select', { id: 'sampler-choke' })]
  };
  for (const [id] of pages.list(inst)) {
    const attrs = {}; for (const [, key, value] of pages.attributes(inst, id).matchAll(/([\w-]+)="([^"]*)"/g)) attrs[key] = value;
    if (/\shidden(\s|$)/.test(pages.attributes(inst, id))) attrs.hidden = '';
    scroller.append(n('div', attrs, contents[id]));
  }
  const key = (target, init) => { const event = { target, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...init }; return pages.key(event) && event.defaultPrevented; };
  return { pages, document, state, inst, scroller, key, get: id => document.getElementById(id), shown: () => Array.from(document.querySelectorAll('[data-inst-page]').filter(page => !page.hidden), page => page.getAttribute('data-inst-page')), get redraws() { return redraws; }, get playheadSyncs() { return playheadSyncs; } };
}

const tests = [], test = (name, run) => tests.push([name, run]);
test('sampler has five pages, other types (and the retired synth) none; bar marks the current page and counts', () => {
  const h = harness();
  assert.deepEqual(Array.from(h.pages.list(), ([id]) => id), ['sample', 'slices', 'zones', 'modulation', 'voice']);
  assert.deepEqual(h.shown(), ['sample']);
  const bar = h.pages.renderBar(h.inst);
  assert.match(bar, /role="tablist"/); assert.match(bar, /id="inst-page-tab-sample"[^>]*aria-selected="true"[^>]*tabindex="0"/);
  assert.match(bar, /Slices<span class="inst-page-count">4<\/span>/); assert.match(bar, /Zones<span class="inst-page-count">2<\/span>/);
  assert.equal(harness('synth').pages.list().length, 0);
  assert.equal(harness('midi').pages.list().length, 0);
  assert.equal(harness('plugin').pages.renderBar({ type: 'plugin' }), '');
});
test('in Wavetable mode ⌥1 focuses the Position value before the waveform', () => {
  const h = harness('sample', 'wavetable');
  assert(h.key(h.document.body, { code: 'Digit3', altKey: true })); assert.deepEqual(h.shown(), ['zones']);
  assert(h.key(h.document.activeElement, { code: 'Digit1', altKey: true })); assert.deepEqual(h.shown(), ['sample']);
  assert.equal(h.document.activeElement.id, 'param-value-wavetablePosition');
});
test('⌥1–5 open pages and focus their primary control; ⌃Tab cycles; the page survives a re-render', () => {
  const h = harness(); h.get('sampler-wave-canvas').focus();
  assert(h.key(h.document.activeElement, { code: 'Digit2', key: '™', altKey: true }));
  assert.deepEqual(h.shown(), ['slices']); assert.equal(h.document.activeElement.id, 'sampler-audition-1'); assert.equal(h.scroller.scrollTop, 0);
  assert.equal(h.get('inst-page-tab-slices').getAttribute('aria-selected'), 'true'); assert.equal(h.get('inst-page-tab-sample').tabIndex, -1);
  assert(h.key(h.document.activeElement, { code: 'Digit4', altKey: true })); assert.equal(h.document.activeElement.id, 'mod-number-0-lfoRate', 'numeric entry, not the slider');
  assert(h.key(h.document.activeElement, { key: 'Tab', ctrlKey: true })); assert.deepEqual(h.shown(), ['voice']); assert.equal(h.document.activeElement.id, 'sampler-voice-mode');
  assert(h.key(h.document.activeElement, { key: 'Tab', ctrlKey: true })); assert.deepEqual(h.shown(), ['sample']);
  assert(h.key(h.document.activeElement, { key: 'Tab', ctrlKey: true, shiftKey: true })); assert.deepEqual(h.shown(), ['voice']);
  assert(h.key(h.document.activeElement, { code: 'Digit1', altKey: true })); assert.equal(h.document.activeElement.id, 'sampler-wave-canvas'); assert(h.redraws > 0, 'waveform redrawn once visible');
  assert.equal(h.key(h.document.activeElement, { code: 'Digit6', altKey: true }), false, 'no sixth page');
  h.pages.show('zones', { focus: false }); assert.equal(h.pages.current(), 'zones');
  assert.doesNotMatch(h.pages.attributes(h.inst, 'zones'), /hidden/); assert.match(h.pages.attributes(h.inst, 'sample'), /hidden/);
});
test('switching pages restarts or stops the waveform playhead poll (pages toggle visibility without a render)', () => {
  const h = harness(); let syncs = h.playheadSyncs;
  h.pages.show('slices'); assert.equal(h.playheadSyncs, syncs + 1, 'leaving the Sample page'); syncs = h.playheadSyncs;
  h.pages.show('sample'); assert.equal(h.playheadSyncs, syncs + 1, 'returning to the Sample page');
});

test('page tabs: arrows move and activate, ↓ enters the page; modals and other views are left alone', () => {
  const h = harness(), tab = h.get('inst-page-tab-sample'); tab.focus();
  assert(h.key(tab, { key: 'ArrowRight' })); assert.deepEqual(h.shown(), ['slices']); assert.equal(h.document.activeElement.id, 'inst-page-tab-slices');
  assert(h.key(h.document.activeElement, { key: 'End' })); assert.equal(h.document.activeElement.id, 'inst-page-tab-voice');
  assert(h.key(h.document.activeElement, { key: 'ArrowRight' })); assert.equal(h.document.activeElement.id, 'inst-page-tab-sample', 'wraps');
  assert(h.key(h.document.activeElement, { key: 'ArrowDown' })); assert.equal(h.document.activeElement.id, 'sampler-wave-canvas');
  h.state.modal = 'palette'; assert.equal(h.key(h.document.activeElement, { code: 'Digit3', altKey: true }), false); h.state.modal = null;
  h.state.view = 'pattern'; assert.equal(h.key(h.document.activeElement, { code: 'Digit3', altKey: true }), false); assert.equal(h.pages.show('zones'), false);
});
test('reveal opens the page holding a control (Find control, fixtures) without moving focus', () => {
  const h = harness(), choke = h.get('sampler-choke'), outside = h.get('inst-page-tab-zones'); outside.focus();
  assert.equal(h.pages.reveal(choke), true); assert.deepEqual(h.shown(), ['voice']); assert.equal(h.document.activeElement, outside);
  assert.equal(h.pages.reveal(choke), false, 'already visible');
  // A control on a hidden page that held focus hands focus to the new page tab.
  const zone = h.get('sampler-zone-0-keyLow'); h.pages.show('zones', { focus: false }); zone.focus(); h.pages.show('sample', { focus: false });
  assert.equal(h.document.activeElement.id, 'inst-page-tab-sample');
});

let failed = 0;
for (const [name, run] of tests) { try { run(); console.log('PASS', name); } catch (error) { failed++; console.error('FAIL', name); console.error(error); } }
if (failed) process.exit(1); console.log(`Instrument pages: ${tests.length} groups passed.`);
