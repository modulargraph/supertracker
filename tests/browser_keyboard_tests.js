'use strict';
// Focus-aware browser navigation, including DOM replacement during each application repaint.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = web.source('browser.js');

function harness() {
  const nodes = new Map(), specs = [], calls = [], timers = new Map();
  let serial = 0, now = 0, browser, markup = '', deferredPreview, deferredSearch;
  const state = { view: 'browser', track: 0, instrument: 0, plugins: [
    { id: 'au:synth', name: 'Alpha', type: 'instrument', format: 'AU' },
    { id: 'vst:synth', name: 'Beta', type: 'instrument', format: 'VST3' },
    { id: 'au:effect', name: 'Echo', type: 'effect', format: 'AU' }
  ], project: { tracks: [{}], instruments: [{}] }, library: { items: [0, 1, 2].map(index => ({ path: `/fixture/${index}.wav`, name: `Sound ${index}`, kind: 'sample' })), selected: 1 } };
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null,
    querySelector(selector) { return selector === '.browser-workspace' ? ['browser','plugins'].includes(state.view) ? {} : null : this.querySelectorAll(selector)[0] || null; },
    querySelectorAll(selector) { return [...nodes.values()].filter(node => node.matches(selector)); } };
  function add(id, selectors = [], dataset = {}, tagName = 'BUTTON', context = '') { specs.push({ id, selectors, dataset, tagName, context }); }
  for (const tab of ['samples', 'plugins']) add(`browser-tab-${tab}`, ['[data-browser-tab]'], { browserTab: tab });
  for (const kind of ['all', 'sample', 'nks']) add(`browser-kind-${kind}`, ['[data-browser-kind]'], { browserKind: kind }, 'BUTTON', '.browser-sidebar');
  for (const kind of ['instrument', 'effect']) add(`browser-plugin-kind-${kind}`, ['[data-browser-plugin-kind]'], { browserPluginKind: kind }, 'BUTTON', '.browser-sidebar');
  for (const format of ['all', 'AU', 'VST3']) add(`browser-plugin-format-${format}`, ['[data-browser-plugin-format]'], { browserPluginFormat: format }, 'BUTTON', '.browser-sidebar');
  for (let index = 0; index < 5; index++) add(`browser-duration-preset-${index}`, ['[data-browser-duration-preset]'], { browserDurationPreset: String(index) }, 'BUTTON', '.browser-sidebar');
  for (const id of ['browser-search', 'browser-plugin-search', 'browser-facet-search-packs', 'browser-range-duration-min']) add(id, [], {}, 'INPUT');
  add('browser-plugin-slot', [], {}, 'SELECT');
  add('editable', [], {}, 'DIV');
  add('browser-results-list', [], {}, 'DIV'); add('browser-plugin-list', [], {}, 'DIV');
  add('browser-load', [], {}, 'BUTTON', '.browser-detail'); add('browser-plugin-load', [], {}, 'BUTTON', '.browser-detail');
  add('browser-preview'); add('browser-plugin-rescan', [], {}, 'BUTTON', '.browser-sidebar'); add('outside');
  function rebuild() {
    const inBrowser = ['browser','plugins'].includes(state.view);
    markup = inBrowser ? browser.render() : '';
    nodes.clear();
    for (const spec of specs) {
      if (!inBrowser && spec.id !== 'outside') continue;
      const handlers = {};
      const node = { ...spec, handlers, value: spec.id === 'browser-search' ? state.library.query : '', scrollTop: 0,
        matches(selector) { return selector.split(',').some(part => { const name = part.trim(); return name === `#${this.id}` || name.toUpperCase() === this.tagName || this.selectors.includes(name); }); },
        closest(selector) { return this.matches(selector) ? this : selector === this.context ? { context: this.context } : null; },
        addEventListener(type, handler) { handlers[type] = handler; },
        click() { return handlers.click?.({ target: this, preventDefault() {}, stopPropagation() {} }); },
        focus(options) { this.focusOptions = options; document.activeElement = this; }, select() {}, scrollIntoView() {},
        setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; } };
      if (spec.id === 'editable') node.isContentEditable = true;
      nodes.set(spec.id, node);
    }
    if (inBrowser) browser.bind();
  }
  const ST = { state, escapeHTML: String, refresh: rebuild, toast() {}, mutate(_label, fn) { fn(); },
    async request(command, payload) {
      calls.push({ command, payload });
      if (command === 'sample.preview' && deferredPreview) return deferredPreview;
      if (command === 'library.search' && deferredSearch) return deferredSearch;
      if (command === 'library.search') return { items: state.library.items, total: state.library.items.length };
      return {};
    } };
  const context = vm.createContext({ window: { ST }, document, Map, Set, Promise, queueMicrotask() {},
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, at: now + delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'browser.js', { text: source }); browser = context.window.STBrowser; rebuild();
  function key(id, key, options = {}) {
    const target = nodes.get(id); target.focus();
    const event = { target, key, code: key === ' ' ? 'Space' : key, preventDefault() { this.defaultPrevented = true; }, ...options };
    const handled = browser.key(event);
    return { handled, prevented: !!event.defaultPrevented };
  }
  async function tick(ms = 0) {
    now += ms;
    for (const [id, timer] of [...timers]) if (timer.at <= now && timers.delete(id)) timer.fn();
    for (let index = 0; index < 12; index++) await Promise.resolve();
  }
  return { browser, state, nodes, calls, key, tick, rebuild, document, get markup() { return markup; },
    get focused() { return document.activeElement?.id; }, set previewPromise(value) { deferredPreview = value; }, set searchPromise(value) { deferredSearch = value; } };
}

(async () => {
  const h = harness();
  assert.match(h.markup, /id="browser-tab-samples"[^>]*tabindex="0"/);
  assert.match(h.markup, /id="browser-tab-plugins"[^>]*tabindex="-1"/);
  assert.match(h.markup, /↑ ↓<\/kbd> Choices/);
  assert.match(h.markup, /← →<\/kbd> Panels/);
  assert.match(h.markup, /Tab<\/kbd> Groups/);
  assert.match(h.markup, /Space<\/kbd> Preview \/ select/);
  assert(!h.markup.includes('Filter choices'), 'footer must not imply horizontal arrows move vertical facet options');
  const oldTab = h.nodes.get('browser-tab-plugins');
  assert(h.key('browser-tab-samples', 'ArrowRight').handled);
  assert(h.markup.includes('id="browser-plugin-list"'));
  assert.equal(h.focused, 'browser-tab-plugins');
  assert.notEqual(h.document.activeElement, oldTab, 'focus lands on the newly rendered control');
  h.key('browser-tab-plugins', 'ArrowDown'); assert.equal(h.focused, 'browser-plugin-search');
  h.key('browser-plugin-search', 'ArrowDown'); assert.equal(h.focused, 'browser-plugin-list');
  h.key('browser-plugin-list', 'ArrowUp'); assert.equal(h.focused, 'browser-plugin-search');
  h.key('browser-tab-plugins', 'ArrowRight'); assert.equal(h.focused, 'browser-tab-samples');
  assert(h.markup.includes('id="browser-results-list"'), 'tabs wrap without changing sound selection');
  assert.equal(h.state.library.selected, 1);

  h.key('browser-kind-all', 'ArrowRight'); assert.equal(h.state.library.kind, 'sample'); assert.equal(h.focused, 'browser-kind-sample');
  h.key('browser-kind-sample', 'End'); assert.equal(h.state.library.kind, 'nks');
  h.key('browser-kind-nks', 'ArrowDown'); assert.equal(h.focused, 'browser-results-list');
  const selection = h.state.library.selected;
  for (const name of ['ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown']) {
    assert.equal(h.key('outside', name).handled, false, `${name} on unrelated controls is not result navigation`);
    assert.equal(h.key('browser-load', name).handled, false, `${name} on detail actions does not browse samples`);
    assert.equal(h.state.library.selected, selection);
  }
  h.key('browser-duration-preset-0', 'ArrowRight'); assert.equal(h.state.library.filters.duration.max, 1); assert.equal(h.focused, 'browser-duration-preset-1');
  h.key('browser-duration-preset-1', 'ArrowDown'); assert.equal(h.focused, 'browser-results-list');
  await h.tick(); // Results become navigable once the changed filters have resolved.
  h.key('browser-results-list', 'ArrowDown'); assert.equal(h.state.library.selected, 1);
  h.key('browser-results-list', 'End'); assert.equal(h.state.library.selected, 2);
  h.key('browser-results-list', 'PageUp'); assert.equal(h.state.library.selected, 0);
  h.key('browser-results-list', 'ArrowUp'); assert.equal(h.focused, 'browser-search');
  h.key('browser-results-list', 'ArrowRight'); assert.equal(h.focused, 'browser-load');
  h.key('browser-load', 'ArrowLeft'); assert.equal(h.focused, 'browser-results-list');

  h.browser.showPlugins('instrument');
  h.key('browser-plugin-kind-instrument', 'ArrowDown'); assert(h.markup.includes('1 effects')); assert.equal(h.focused, 'browser-plugin-kind-effect');
  h.key('browser-plugin-kind-effect', 'ArrowUp'); assert(h.markup.includes('2 instruments'));
  h.key('browser-plugin-kind-instrument', 'ArrowRight'); assert.equal(h.focused, 'browser-plugin-list');
  h.key('browser-plugin-format-all', 'ArrowRight'); assert(h.markup.includes('1 instruments')); assert.equal(h.focused, 'browser-plugin-format-AU');
  h.key('browser-plugin-format-AU', 'ArrowRight'); assert.equal(h.focused, 'browser-plugin-format-VST3'); assert(h.markup.includes('Beta'));
  h.key('browser-plugin-format-VST3', 'ArrowDown'); assert.equal(h.focused, 'browser-plugin-list');
  h.key('browser-plugin-kind-instrument', 'ArrowLeft'); assert.equal(h.focused, 'browser-tab-plugins');

  // Editable controls, native selection, modifiers and composition must retain their normal keyboard semantics.
  for (const id of ['browser-plugin-search', 'browser-facet-search-packs', 'browser-range-duration-min', 'browser-plugin-slot', 'editable']) {
    for (const key of ['ArrowLeft', 'ArrowRight', 'Home', 'End']) assert.equal(h.key(id, key).handled, false, `${id} retains ${key}`);
  }
  for (const modifier of ['metaKey', 'ctrlKey', 'altKey', 'shiftKey']) assert.equal(h.key('browser-plugin-search', 'ArrowDown', { [modifier]: true }).handled, false);
  assert.equal(h.key('browser-plugin-search', 'ArrowDown', { isComposing: true }).handled, false);
  assert.equal(h.key('browser-plugin-list', 'ArrowDown', { defaultPrevented: true }).handled, true);
  h.key('outside', 'f', { metaKey: true, defaultPrevented: true });
  assert.equal(h.focused, 'browser-plugin-search', 'global Command-F delegation may already prevent the browser default');
  assert.equal(h.key('browser-plugin-load', 'Enter').prevented, false, 'native button activation remains intact');

  // Escape cancels queued and in-flight audition without swallowing later transport Escape forever.
  const p = harness(); p.key('browser-results-list', 'ArrowDown');
  assert(p.key('browser-results-list', 'Escape').handled); await p.tick(70);
  assert(!p.calls.some(call => call.command === 'sample.preview'));
  assert.equal(p.key('browser-results-list', 'Escape').handled, false);
  let finishPreview; p.previewPromise = new Promise(resolve => { finishPreview = resolve; });
  p.key('browser-results-list', ' '); await p.tick();
  assert(p.key('browser-search', 'Escape').handled, 'pending preview can be cancelled from search');
  finishPreview({ duration: 10 }); await p.tick();
  assert.equal(p.key('browser-results-list', 'Escape').handled, false, 'late preview response cannot resurrect a cancelled preview');
  const q = harness(); q.key('browser-results-list', ' '); await q.tick();
  assert(q.key('browser-load', 'Escape').handled, 'playing preview stops from a detail control'); await q.tick();
  assert(q.calls.some(call => call.command === 'sample.stopPreview'));
  assert.equal(q.key('browser-results-list', 'Escape').handled, false);
  const e = harness(); e.state.library.items = []; e.state.library.selected = 0;
  assert(e.key('browser-results-list', 'ArrowDown').handled); await e.tick(70);
  assert(!e.calls.some(call => call.command === 'sample.preview'));

  // View entry focuses the active tab; asynchronous refresh restores the current input and caret.
  const entry = harness(); entry.state.library.query = 'kick'; entry.rebuild();
  entry.nodes.get('outside').focus(); entry.browser.focusSearch();
  const originalSearch = entry.document.activeElement;
  assert.equal(originalSearch, entry.nodes.get('browser-search'));
  assert.equal(originalSearch.focusOptions.preventScroll, true);
  originalSearch.setSelectionRange(1, 3);
  let finishSearch; entry.searchPromise = new Promise(resolve => { finishSearch = resolve; });
  const entrySearch = entry.browser.search();
  finishSearch({ items: entry.state.library.items, total: 3 }); await entrySearch;
  assert.notEqual(entry.document.activeElement, originalSearch, 'refresh replaces the old input');
  assert.equal(entry.document.activeElement, entry.nodes.get('browser-search'), 'focus belongs to the current rendered search node');
  assert.deepEqual([entry.document.activeElement.selectionStart, entry.document.activeElement.selectionEnd], [1, 3]);

  let finishMetadata; entry.searchPromise = new Promise(resolve => { finishMetadata = resolve; });
  entry.browser.event('library.metadata', {}); await entry.tick(300);
  finishMetadata({ items: entry.state.library.items, total: 3 }); await entry.tick();
  assert.equal(entry.document.activeElement, entry.nodes.get('browser-search'));
  assert.deepEqual([entry.document.activeElement.selectionStart, entry.document.activeElement.selectionEnd], [1, 3], 'metadata refresh preserves the caret');
  entry.browser.showPlugins('instrument'); entry.nodes.get('outside').focus(); entry.browser.focusSearch();
  assert.equal(entry.document.activeElement, entry.nodes.get('browser-plugin-search'));
  assert.equal(entry.document.activeElement.focusOptions.preventScroll, true);
  await entry.tick();

  // Responses follow current focus rather than pulling the user back to the browser's entry field.
  for (const leaveView of [false, true]) {
    const moved = harness(); moved.browser.focusSearch();
    let finish; moved.searchPromise = new Promise(resolve => { finish = resolve; });
    const request = moved.browser.search();
    if (leaveView) { moved.state.view = 'song'; moved.rebuild(); }
    moved.nodes.get('outside').focus();
    finish({ items: moved.state.library.items, total: 3 }); await request;
    assert.equal(moved.document.activeElement, moved.nodes.get('outside'), 'late results preserve a newly focused control');
    if (leaveView) { assert.equal(moved.state.view, 'song'); assert.equal(moved.nodes.has('browser-search'), false); }
  }
  console.log('Browser keyboard: passed active-tab entry focus, delayed result/metadata caret preservation, focus after leaving browser, tab/segment navigation, native editing, and queued/in-flight/playing preview Escape.');
})().catch(error => { console.error(error); process.exitCode = 1; });
