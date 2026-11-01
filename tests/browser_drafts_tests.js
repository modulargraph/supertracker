'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('browser.js');
const settle = async () => { for (let n = 0; n < 12; n++) await Promise.resolve(); };
const deferred = () => { let resolve; const promise = new Promise(value => { resolve = value; }); return { promise, resolve }; };

function harness() {
  let browser, nodes = [], markup = '', timerID = 0, paints = 0, reply = null;
  const timers = new Map(), documentHandlers = {}, calls = [];
  const state = { view: 'browser', track: 0, instrument: 0, plugins: [], project: { tracks: [{}], instruments: [{}] },
    library: { items: [{ name: 'Kick', path: '/fixture/kick.wav', kind: 'sample' }], roots: [], filters: { duration: { min: 1 } }, facets: { categories: [{ value: 'Kick', count: 1 }, { value: 'Snare', count: 1 }] } } };
  const document = { activeElement: null,
    getElementById: id => nodes.find(node => node.id === id) || null,
    querySelector: selector => selector === '.browser-workspace' ? {} : nodes.find(node => node.matches(selector)) || null,
    querySelectorAll: selector => nodes.filter(node => node.matches(selector)),
    addEventListener(type, handler) { (documentHandlers[type] ||= []).push(handler); } };
  class Node {
    constructor(tagName, attributes) {
      this.tagName = tagName.toUpperCase(); this.attributes = {}; this.dataset = {}; this.handlers = {}; this.disabled = /\sdisabled(?:\s|$)/.test(attributes); this.value = ''; this.scrollTop = 0;
      for (const match of attributes.matchAll(/([\w-]+)="([^"]*)"/g)) {
        const value = match[2].replace(/&quot;/g, '"').replace(/&amp;/g, '&'); this.attributes[match[1]] = value;
        if (match[1].startsWith('data-')) this.dataset[match[1].slice(5).replace(/-([a-z])/g, (_, char) => char.toUpperCase())] = value;
        else this[match[1]] = value;
      }
    }
    matches(selector) {
      const attribute = selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/);
      if (attribute) return Object.hasOwn(this.attributes, attribute[1]) && (attribute[2] === undefined || this.attributes[attribute[1]] === attribute[2]);
      return selector === `#${this.id}`;
    }
    closest(selector) {
      if (selector === '.browser-workspace') return {};
      if (selector === '[data-browser-filter-group]') {
        const field = this.dataset.browserFacet || this.dataset.browserFacetSearch || (this.id?.startsWith('browser-root-') ? 'roots' : null);
        return field ? { dataset: { browserFilterGroup: ({ roots: 'locations', sampleTypes: 'length', extensions: 'tempo' }[field] || field) } } : null;
      }
      return null;
    }
    addEventListener(type, handler) { (this.handlers[type] ||= []).push(handler); }
    fire(type, values = {}) { const event = { target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...values }; for (const handler of this.handlers[type] || []) handler(event); return event; }
    focus() { if (document.activeElement !== this) { document.activeElement?.fire('blur'); document.activeElement = this; this.fire('focus'); } }
    click() { this.fire('click'); }
    select() {} setSelectionRange() {} scrollIntoView() {}
  }
  function rebuild() {
    paints++; markup = browser.render(); document.activeElement = null; nodes = [];
    for (const match of markup.matchAll(/<(input|button|summary|div|details|select)\b([^>]*)>/g)) nodes.push(new Node(match[1], match[2]));
    browser.bind();
  }
  const ST = { state, escapeHTML: value => String(value).replace(/&/g, '&amp;').replace(/"/g, '&quot;'), refresh: rebuild, toast() {}, mutate(_label, work) { work(); },
    async request(command, payload) {
      calls.push({ command, payload });
      if (command === 'library.search') return reply ? reply(payload) : { items: state.library.items, total: state.library.items.length, facets: state.library.facets };
      return {};
    } };
  const context = vm.createContext({ window: { ST }, document, Map, Set, Promise, queueMicrotask() {},
    setTimeout(fn, delay) { const id = ++timerID; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'browser.js', { text: source }); browser = context.window.STBrowser; rebuild();
  return { browser, state, document, calls, rebuild, node: id => document.getElementById(id), get paints() { return paints; }, get markup() { return markup; },
    set searchReply(value) { reply = value; }, async timers(delay = 0) { for (const [id, timer] of [...timers]) if (timer.delay === delay && timers.delete(id)) timer.fn(); await settle(); },
    documentEvent(type, target) { for (const handler of documentHandlers[type] || []) handler({ target }); } };
}

(async () => {
  {
    const h = harness(), input = h.node('browser-range-duration-min'); input.focus(); input.value = '12'; input.fire('input');
    const before = h.paints; await h.browser.search();
    assert.equal(h.paints, before, 'background search does not replace a live number editor'); assert.equal(h.document.activeElement, input); assert.equal(input.value, '12');
    input.value = ''; input.fire('input'); await h.browser.search(); assert.equal(input.value, '', 'blank/partial drafts are independent of numeric parsing');
    assert.equal(h.state.library.filters.duration.min, 1, 'uncommitted draft does not change search semantics');
    input.value = '12'; input.fire('input'); input.fire('keydown', { key: 'Tab' }); input.fire('change');
    assert.equal(h.paints, before, 'change during Tab departure must not restore the old input');
    h.node('browser-search').focus(); await h.timers();
    assert.equal(h.document.activeElement.id, 'browser-search'); assert.equal(h.state.library.filters.duration.min, 12); assert.equal(h.node('browser-range-duration-min').value, '12');
    const pair = harness(); pair.state.library.filters.duration = { min: 1, max: 5 }; pair.rebuild();
    const min = pair.node('browser-range-duration-min'); min.focus(); min.value = '12'; min.fire('input'); min.fire('keydown', { key: 'Tab' }); min.fire('change');
    pair.node('browser-range-duration-max').focus(); await pair.timers();
    assert.equal(pair.document.activeElement.id, 'browser-range-duration-max'); assert.equal(pair.node('browser-range-duration-max').value, '12', 'an untouched next range field receives corrected bounds after native Tab focus');
  }
  {
    const h = harness(), first = deferred(), latest = deferred();
    h.searchReply = payload => payload.query === 'new' ? latest.promise : first.promise;
    h.state.library.query = 'old'; const old = h.browser.search();
    const input = h.node('browser-range-duration-min'); input.focus(); input.value = '7'; input.fire('input');
    h.state.library.query = 'new'; const current = h.browser.search();
    latest.resolve({ items: [{ name: 'New result', path: '/new.wav' }], total: 1, facets: {} }); await current;
    first.resolve({ items: [{ name: 'Stale result', path: '/old.wav' }], total: 1, facets: {} }); await old;
    assert.equal(h.state.library.items[0].name, 'New result'); assert.equal(input.value, '7'); assert.equal(h.document.activeElement, input);
    input.fire('keydown', { key: 'Tab' }); input.fire('change'); h.node('browser-results-list').focus(); await h.timers();
    assert.equal(h.document.activeElement.id, 'browser-results-list'); assert(h.markup.includes('New result')); assert(!h.markup.includes('Stale result'));
  }
  {
    const h = harness(), input = h.node('browser-range-duration-min'); input.focus(); input.value = '15'; input.fire('input'); await h.browser.search();
    const clear = h.node('browser-clear-all'); h.documentEvent('pointerdown', clear); clear.focus(); await h.timers();
    assert.equal(h.node('browser-clear-all'), clear, 'deferred blur repaint must not destroy a pressed pointer target before click');
    h.documentEvent('pointerup', clear); clear.click(); await h.timers();
    assert.equal(h.document.activeElement.id, 'browser-results-list', 'clear-all removal falls back to the result list'); assert.deepEqual(JSON.parse(JSON.stringify(h.state.library.filters.duration)), {});
    assert.equal(h.node('browser-range-duration-min').value, '', 'explicit clear discards old drafts');
    const preset = h.node('browser-duration-preset-2'); preset.focus(); preset.click(); await h.timers();
    assert.equal(h.node('browser-range-duration-min').value, '1'); assert.equal(h.node('browser-range-duration-max').value, '5');
  }
  {
    const h = harness(); const chip = h.node('browser-filter-chip-0'); chip.focus(); chip.click(); await h.timers();
    assert.equal(h.node('browser-clear-all'), null); assert.equal(h.document.activeElement.id, 'browser-results-list', 'last chip does not strand focus on the document');
    const category = h.node('browser-facet-categories-Kick'); category.focus();
    h.searchReply = () => ({ items: [], total: 0, facets: { categories: [{ value: 'Snare', count: 1 }] } }); await h.browser.search();
    assert.equal(h.document.activeElement.id, 'browser-group-categories', 'removed facet option returns to its own group');
  }
  {
    const h = harness(), root = '/Fixture Samples'; h.state.library.roots = [{ path: root, name: 'Fixture', available: true, scanStatus: 'error', scanErrors: ['Fixture read error'] }]; h.rebuild();
    for (const prefix of ['browser-root-remove-', 'browser-root-rechoose-', 'browser-root-details-']) assert(h.node(prefix + encodeURIComponent(root)), 'root controls have stable identity');
    const remove = h.node('browser-root-remove-' + encodeURIComponent(root)); remove.focus(); h.state.library.roots = []; await h.browser.search();
    assert.equal(h.document.activeElement.id, 'browser-group-locations', 'removed root action falls back to locations');
    const delayed = deferred(); h.searchReply = () => delayed.promise; h.node('browser-facet-categories-Kick').focus(); const pending = h.browser.search();
    h.node('browser-search').focus(); delayed.resolve({ items: [], total: 0, facets: { categories: [] } }); await pending;
    assert.equal(h.document.activeElement.id, 'browser-search', 'late response follows current focus rather than the request initiator');
  }
  console.log('Browser drafts: 5 suites passed (raw numeric drafts, Tab/pointer departure, stale search generations, clear/preset reset, disappearing facet/root/chip focus, and user-moved focus).');
})().catch(error => { console.error(error); process.exitCode = 1; });
