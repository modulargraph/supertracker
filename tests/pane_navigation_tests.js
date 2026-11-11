'use strict';
const assert = require('node:assert/strict');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// These fixtures contain only mounted elements: querying an absent #id returns
// null. Both the app dispatcher and the pane implementation run unmodified.
function harness(view = 'song') {
  const calls = [], nodes = new Map();
  const document = { activeElement: null, addEventListener() {} };
  function simpleMatches(node, selector) {
    let rejected = false;
    selector = selector.replace(/:not\(([^()]*)\)/g, (_, part) => {
      if (simpleMatches(node, part)) rejected = true;
      return '';
    });
    if (rejected) return false;
    if (selector.includes(':disabled')) {
      if (!node.disabled) return false;
      selector = selector.replace(':disabled', '');
    }
    let attributesMatch = true;
    selector = selector.replace(/\[([^\]=^]+)(\^?=)?["']?([^\]"']*)["']?\]/g, (_, name, op, value) => {
      const actual = node.getAttribute(name);
      if (actual === null || (op === '=' && actual !== value) || (op === '^=' && !actual.startsWith(value))) attributesMatch = false;
      return '';
    });
    if (!attributesMatch) return false;
    const id = selector.match(/#([\w-]+)/)?.[1];
    if (id && node.id !== id) return false;
    if ([...selector.matchAll(/\.([\w-]+)/g)].some(match => !node.classes.has(match[1]))) return false;
    const tag = selector.match(/^[\w-]+/)?.[0];
    return !tag || node.tagName === tag.toUpperCase();
  }
  function matches(node, selector) {
    return selector.split(',').some(part => {
      const tokens = part.trim().replace(/\s*>\s*/g, ' > ').split(/\s+/);
      let current = node, index = tokens.length - 1;
      if (!simpleMatches(current, tokens[index--])) return false;
      while (index >= 0) {
        const direct = tokens[index] === '>';
        if (direct) index--;
        current = current.parentElement;
        if (!direct) while (current && !simpleMatches(current, tokens[index])) current = current.parentElement;
        if (!current || !simpleMatches(current, tokens[index--])) return false;
      }
      return true;
    });
  }
  function descendants(root) { return root.children.flatMap(child => [child, ...descendants(child)]); }
  function node(tag, id, parent = document.body, options = {}) {
    const attrs = { ...(options.attrs || {}) };
    const item = { id, tagName: tag.toUpperCase(), parentElement: parent, children: [], attrs,
      classes: new Set((options.className || '').split(' ').filter(Boolean)), dataset: {},
      type: options.type || '', disabled: !!options.disabled, hidden: !!options.hidden,
      rendered: options.rendered !== false, isConnected: true, scrolls: 0,
      tabIndex: options.tabIndex ?? (['button', 'input', 'select', 'textarea', 'summary'].includes(tag) ? 0 : -1),
      getAttribute(name) {
        if (name === 'id') return this.id || null;
        if (name === 'type') return this.type || null;
        if (name === 'tabindex') return String(this.tabIndex);
        if (name === 'hidden' && this.hidden) return '';
        return Object.hasOwn(this.attrs, name) ? String(this.attrs[name]) : null;
      },
      matches(selector) { return matches(this, selector); },
      closest(selector) { for (let current = this; current; current = current.parentElement) if (current.matches(selector)) return current; return null; },
      contains(other) { for (let current = other; current; current = current.parentElement) if (current === this) return true; return false; },
      querySelectorAll(selector) { return descendants(this).filter(child => child.isConnected && child.matches(selector)); },
      querySelector(selector) { return this.querySelectorAll(selector)[0] || null; },
      getClientRects() {
        if (!this.isConnected) return [];
        for (let current = this; current; current = current.parentElement) if (!current.rendered || current.hidden) return [];
        return [{}];
      },
      focus() {
        if (!this.isConnected || this.disabled) return;
        const previous = document.activeElement;
        if (previous !== this) previous?.onblur?.();
        // A blur handler can synchronously replace this element. A browser then
        // leaves focus on the document until the new target is resolved.
        if (this.isConnected) { document.activeElement = this; this.onfocus?.(); }
      },
      scrollIntoView() { this.scrolls++; },
      addEventListener() {},
    };
    for (const [key, value] of Object.entries(attrs)) if (key.startsWith('data-')) item.dataset[key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase())] = String(value);
    parent?.children.push(item); nodes.set(id, item); return item;
  }
  document.body = node('body', 'body', null);
  document.activeElement = document.body;
  document.querySelectorAll = selector => document.body.querySelectorAll(selector);
  document.querySelector = selector => document.querySelectorAll(selector)[0] || null;
  document.getElementById = id => nodes.get(id)?.isConnected ? nodes.get(id) : null;
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) {
    calls.push(message); window.STHostTransport.deliver({ id: message.id, ok: true, result: recording(message) ?? {} });
  } }) };
  const recording = noteRecordingHost(window);
  const context = vm.createContext({ window, document, console, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, setTimeout() {}, clearTimeout() {} });
  for (const filename of ['app.js', 'block-nav.js', 'pane-navigation.js']) web.load(context, filename);
  const ST = window.ST, state = ST.state;
  state.project = ST.demoProject(); state.view = view; state.keyboardPlay = true; state.recording = true;
  function clear() {
    for (const item of descendants(document.body)) item.isConnected = false;
    document.body.children = []; document.activeElement = document.body;
  }
  function key(key = 'F6', extra = {}) {
    const event = { target: document.activeElement, key, code: key, preventDefault() { this.defaultPrevented = true; }, ...extra };
    ST.keyboard(event); return event;
  }
  function sidebar(kind = 'pattern') {
    const root = node('aside', 'sidebar', document.body, { className: 'sidebar' });
    node('button', 'sidebar-inactive', root, { tabIndex: -1, attrs: { ['data-' + kind]: '1' } });
    return node('button', 'sidebar-current', root, { attrs: { ['data-' + kind]: '0' } });
  }
  function song() {
    clear(); sidebar();
    const sequence = node('div', 'sequence-list', document.body, { tabIndex: 0 });
    node('span', 'sequence-descendant', sequence);
    const settings = node('aside', 'song-options', document.body, { className: 'song-options' });
    node('input', 'loop-song', settings, { type: 'checkbox' });
    const toolbar = node('div', 'clips-toolbar', document.body, { className: 'clips-toolbar' });
    node('input', 'clips-cursor', toolbar, { type: 'number' });
    const timeline = node('div', 'clips-timeline', document.body, { tabIndex: 0 });
    node('button', 'clip-a', timeline);
    const inspector = node('aside', 'clips-inspector', document.body, { className: 'clips-inspector' });
    node('input', 'clips-name', inspector, { type: 'text' });
    node('input', 'clips-startBeat', inspector, { type: 'number' });
  }
  return { ST, state, window, document, calls, nodes, node, clear, sidebar, song, key,
    sections: () => Array.from(window.STPaneNavigation.sections(), section => [section.key, section.target.id]) };
}

function unchanged(h, before) {
  assert.equal(JSON.stringify(h.state.project), before);
  assert.equal(h.state.undo.length, 0);
  assert.deepEqual(h.calls, [], 'Pane movement must not send audio or other bridge requests');
}
const tests = [];
function test(name, run) { tests.push({ name, run }); }

test('Song F6 and Shift-F6 navigate descendants, wrap, and share command actions without editing', () => {
  const h = harness(); h.song();
  const expected = ['sidebar-current', 'sequence-list', 'loop-song', 'clips-cursor', 'clips-timeline', 'clips-name'];
  assert.deepEqual(h.sections().map(entry => entry[1]), expected);
  const before = JSON.stringify(h.state.project);
  for (const id of [...expected, expected[0]]) { assert(h.key().defaultPrevented); assert.equal(h.document.activeElement.id, id); }
  assert(h.key('F6', { shiftKey: true }).defaultPrevented); assert.equal(h.document.activeElement.id, 'clips-name');
  h.nodes.get('sequence-descendant').focus(); h.key(); assert.equal(h.document.activeElement.id, 'loop-song');
  h.nodes.get('clip-a').focus(); h.key('F6', { shiftKey: true }); assert.equal(h.document.activeElement.id, 'clips-cursor');
  h.ST.perform('nextPane'); assert.equal(h.document.activeElement.id, 'clips-timeline');
  h.ST.perform('previousPane'); assert.equal(h.document.activeElement.id, 'clips-cursor');
  h.document.body.focus(); h.key('F6', { shiftKey: true }); assert.equal(h.document.activeElement.id, 'clips-name');
  unchanged(h, before);
});

test('Pattern, instrument, both browsers, mixer and automation resolve useful scope entries', () => {
  const fixtures = [
    ['pattern', h => {
      h.sidebar(); const main = h.node('section', 'main', undefined, { className: 'workspace-main' });
      const destination = h.node('section', 'destination', main, { className: 'pattern-destination' }); h.node('select', 'pattern-destination-lane', destination);
      const toolbar = h.node('div', 'toolbar', main, { className: 'toolbar pattern-toolbar' }); const group = h.node('div', 'pattern-group', toolbar, { className: 'toolbar-group pattern-group' });
      h.node('input', 'pattern-length', group, { type: 'number' });
      h.node('div', 'tracker', main, { tabIndex: 0 }); const inspector = h.node('aside', 'step', main, { className: 'step-editor' }); h.node('input', 'step-note', inspector, { type: 'number' });
      return ['sidebar-current', 'pattern-destination-lane', 'pattern-length', 'tracker', 'step-note'];
    }],
    ['instrument', h => {
      h.sidebar('instrument'); const layout = h.node('div', 'layout', undefined, { className: 'instrument-layout' });
      const source = h.node('div', 'source', layout, { className: 'panel' }); h.node('button', 'audition', source); h.node('select', 'inst-waveform', source);
      const mapped = h.node('div', 'nks', layout, { className: 'panel' }); h.node('input', 'nks-range', mapped, { type: 'range', attrs: { 'data-nks-address': '17' } }); h.node('input', 'nks-number', mapped, { type: 'number', attrs: { 'data-nks-address': '17' } });
      const perform = h.node('div', 'perform', layout, { className: 'panel' }); h.node('button', 'piano-60', perform, { attrs: { 'data-piano': '60' } });
      return ['sidebar-current', 'inst-waveform', 'nks-number', 'piano-60'];
    }],
    ...[false, true].map(plugins => ['browser', h => {
      const search = h.node('div', 'search-wrap', undefined, { className: 'browser-search-wrap' }); h.node('input', 'search', search, { type: 'search' });
      const filters = h.node('aside', 'filters', undefined, { className: 'browser-sidebar' });
      if (plugins) h.node('button', 'plugin-kind', filters, { attrs: { 'data-browser-plugin-kind': 'instrument' } });
      else { const details = h.node('details', 'filter-group', filters); h.node('summary', 'browser-group-collections', details); }
      h.node('div', 'results', undefined, { className: plugins ? 'browser-plugin-list' : 'browser-result-list', tabIndex: 0 });
      const detail = h.node('aside', 'detail', undefined, { className: 'browser-detail' }); h.node('button', 'preview', detail); h.node('button', plugins ? 'browser-plugin-load' : 'browser-load', detail);
      return ['search', plugins ? 'plugin-kind' : 'browser-group-collections', 'results', plugins ? 'browser-plugin-load' : 'browser-load'];
    }]),
    ['mixer', h => {
      const channels = h.node('div', 'mixer-channels', undefined, { tabIndex: 0 });
      const other = h.node('div', 'other-channel', channels, { className: 'studio-channel' }); h.node('button', 'other-title', other, { className: 'studio-channel-title' });
      const selected = h.node('div', 'selected-channel', channels, { className: 'studio-channel is-selected' }); h.node('button', 'selected-title', selected, { className: 'studio-channel-title' });
      const rack = h.node('div', 'rack', undefined, { className: 'studio-rack' }); const details = h.node('details', 'insert', rack); h.node('summary', 'plugin-header-0', details); h.node('button', 'add-plugin', rack, { attrs: { 'data-studio-action': 'add-plugin' } });
      const routing = h.node('div', 'routing', undefined, { className: 'studio-routing-panel' }); h.node('select', 'routing-output', routing);
      const returns = h.node('div', 'returns', undefined, { className: 'studio-returns' }); h.node('input', 'reverb-size', returns, { type: 'range' }); h.node('input', 'delay-time', returns, { type: 'range' });
      return ['selected-title', 'plugin-header-0', 'routing-output', 'delay-time'];
    }],
    ['automation', h => {
      const targets = h.node('div', 'targets', undefined, { className: 'studio-target-panel' }); h.node('select', 'automation-track', targets); h.node('input', 'automation-search', targets, { type: 'search' });
      const lanes = h.node('div', 'lanes', undefined, { className: 'studio-lane-stack' }); h.node('button', 'lane-other', lanes, { attrs: { 'data-select-lane': '0', 'aria-selected': 'false' } }); h.node('button', 'lane-selected', lanes, { attrs: { 'data-select-lane': '1', 'aria-selected': 'true' } });
      h.node('svg', 'automation-envelope', undefined, { tabIndex: 0 });
      const tools = h.node('div', 'point-tools', undefined, { className: 'studio-point-toolbar' }); h.node('button', 'automation-add-point', tools);
      const table = h.node('div', 'points', undefined, { className: 'studio-points-table' }); const row = h.node('div', 'selected-point', table, { className: 'studio-point-row is-selected' }); h.node('input', 'point-row-1', row, { type: 'number', attrs: { 'data-point-row': '1' } });
      return ['automation-search', 'lane-selected', 'automation-envelope', 'point-row-1'];
    }],
  ];
  for (const [view, mount] of fixtures) {
    const h = harness(view), expected = mount(h), before = JSON.stringify(h.state.project);
    assert.equal(h.document.querySelector('#not-mounted'), null);
    assert.deepEqual(h.sections().map(entry => entry[1]), expected, view);
    for (const id of expected) { assert(h.key().defaultPrevented, view); assert.equal(h.document.activeElement.id, id, view); }
    h.key(); assert.equal(h.document.activeElement.id, expected[0], view);
    unchanged(h, before);
  }
});

test('modified/composing F6 and modal focus stay untouched; arrows remain with local navigation', () => {
  const h = harness(); h.song(); h.nodes.get('clips-cursor').focus();
  const before = JSON.stringify(h.state.project), focused = h.document.activeElement;
  for (const extra of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }, { isComposing: true }]) {
    assert(!h.key('F6', extra).defaultPrevented); assert.equal(h.document.activeElement, focused);
  }
  h.key('F6', { defaultPrevented: true }); assert.equal(h.document.activeElement, focused);
  for (const modal of ['dialog', 'palette', 'recording']) {
    h.state.modal = modal; assert(!h.key().defaultPrevented); assert.equal(h.document.activeElement, focused);
    assert.equal(h.window.STPaneNavigation.next(), false);
  }
  h.state.modal = null;
  for (const key of ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown']) {
    assert(!h.key(key).defaultPrevented); assert.equal(h.document.activeElement, focused);
  }
  const local = []; h.window.STClips = { key(event) { if (event.key === 'ArrowRight') { local.push(event.key); event.preventDefault(); return true; } return false; } };
  h.nodes.get('clips-timeline').focus(); assert(h.key('ArrowRight').defaultPrevented); assert.deepEqual(local, ['ArrowRight']); assert.equal(h.document.activeElement.id, 'clips-timeline');
  unchanged(h, before);
});

test('absent, disabled, hidden, inert and collapsed entries are skipped with usable fallbacks', () => {
  const h = harness(); h.song();
  h.nodes.get('sidebar').hidden = true;
  h.nodes.get('loop-song').disabled = true;
  h.nodes.get('clips-toolbar').attrs.inert = '';
  h.nodes.get('clips-inspector').attrs['aria-hidden'] = 'true';
  assert.deepEqual(h.sections().map(entry => entry[1]), ['sequence-list', 'clips-timeline']);
  h.key(); h.key(); h.key(); assert.equal(h.document.activeElement.id, 'sequence-list');
  h.clear(); assert.deepEqual(h.sections(), []); assert(!h.key().defaultPrevented);
  h.state.view = 'mixer';
  const rack = h.node('div', 'rack', undefined, { className: 'studio-rack' });
  const collapsed = h.node('div', 'collapsed-content', rack, { rendered: false });
  h.node('input', 'studio-plugin-search', collapsed, { type: 'search' });
  h.node('summary', 'plugin-header-disabled', rack, { attrs: { 'aria-disabled': 'true' } });
  h.node('button', 'add-plugin', rack, { attrs: { 'data-studio-action': 'add-plugin' } });
  assert.deepEqual(h.sections(), [['inserts', 'add-plugin']]); h.key(); assert.equal(h.document.activeElement.id, 'add-plugin');
  h.clear(); h.state.view = 'automation';
  const tools = h.node('div', 'point-tools', undefined, { className: 'studio-point-toolbar' }); h.node('button', 'automation-add-point', tools);
  assert.deepEqual(h.sections(), [['automation-points', 'automation-add-point']]);
});

test('numeric change may replace the Song workspace synchronously without losing the intended pane', () => {
  for (const shiftKey of [false, true]) {
    const h = harness(); h.song(); const original = h.nodes.get('clips-startBeat'), destination = shiftKey ? 'clips-timeline' : 'sidebar-current';
    const staleTarget = h.nodes.get(destination); let commits = 0;
    original.value = '12'; original.onchange = () => { commits++; h.state.project.audioClips = [{ id: 'a', startBeat: Number(original.value) }]; h.song(); };
    original.onblur = () => original.onchange(); original.focus();
    assert(h.key('F6', { shiftKey }).defaultPrevented);
    assert.equal(commits, 1); assert.equal(h.state.project.audioClips[0].startBeat, 12);
    assert.equal(staleTarget.isConnected, false); assert.notEqual(h.document.activeElement, staleTarget);
    assert.equal(h.document.activeElement, h.nodes.get(destination)); assert.equal(h.document.activeElement.scrolls, 1);
    assert.deepEqual(h.calls, []);
  }
});

for (const { name, run } of tests) { run(); console.log('PASS ' + name); }
console.log(`Pane navigation: ${tests.length} regression groups passed.`);
