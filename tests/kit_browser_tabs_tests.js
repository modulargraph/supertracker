'use strict';
// Shared Library integration: tab keys/focus and hardware dispatch use the same Kits browser.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('browser.js');
const nodes = new Map(), calls = [], timers = new Map(); let serial = 0, markup = '', browser;
const state = { view: 'browser', track: 0, instrument: 0, project: { tracks: [{}], instruments: [{}] }, plugins: [], library: { items: [], total: 0 } };
const document = {
  activeElement: null, getElementById: id => nodes.get(id) || null,
  querySelector: selector => selector === '.browser-workspace' ? {} : null,
  querySelectorAll: selector => selector === '[data-browser-tab]' ? [...nodes.values()].filter(node => node.dataset.browserTab) : []
};
function node(id, tagName = 'BUTTON', dataset = {}) {
  const handlers = {};
  return { id, dataset, tagName, handlers, scrollTop: 0,
    addEventListener(type, fn) { handlers[type] = fn; }, focus() { document.activeElement = this; }, select() {},
    closest(selector) { if (selector === '[data-browser-tab]' && this.dataset.browserTab) return this; if (selector === '.browser-workspace') return {}; if (selector === 'button, a, summary' && this.tagName === 'BUTTON') return this; return null; },
    click() { return handlers.click?.({ target: this }); } };
}
function rebuild() {
  markup = browser.render(); nodes.clear();
  for (const tab of ['samples', 'kits', 'nks', 'plugins']) nodes.set(`browser-tab-${tab}`, node(`browser-tab-${tab}`, 'BUTTON', { browserTab: tab }));
  for (const id of ['browser-search', 'kitb-search', 'nksb-search', 'browser-plugin-search']) if (markup.includes(`id="${id}"`)) nodes.set(id, node(id, 'INPUT'));
  browser.bind();
}
const kit = {
  render: () => '<div class="kitb-layout"><input id="kitb-search"></div>', bind: () => calls.push(['kit.bind']),
  key: event => { calls.push(['kit.key', event.key]); return true; }, stopPreview: () => calls.push(['kit.stop']),
  focusSearch: () => document.getElementById('kitb-search')?.focus(), search: () => calls.push(['kit.search']),
  event: (type, payload) => calls.push(['kit.event', type, payload]), shortcuts: () => 'Group preview',
  settings: () => ({ query: 'Kit Query' }), restoreSettings: value => calls.push(['kit.restore', value]),
  controller: { fields: ['sources', 'expansions', 'tags'], filterLabel: field => field, filterValue: () => 'Deep',
    list: () => ({ tab: 'kits', items: [{ name: 'Cloud' }], selected: 0, total: 1 }),
    cycle: (...values) => calls.push(['kit.cycle', ...values]), move: delta => calls.push(['kit.move', delta]), page: delta => calls.push(['kit.page', delta]),
    load: mode => calls.push(['kit.load', mode]), preview: () => calls.push(['kit.preview']), favorite: () => calls.push(['kit.favorite']),
    toggleFavorites: () => calls.push(['kit.favorites']), toggleAutoPreview: () => calls.push(['kit.auto']), clearFilters: () => calls.push(['kit.clear']) }
};
const nks = { render: () => '<div class="nksb-layout"><input id="nksb-search"></div>', bind() {}, key: () => true, stopPreview: () => calls.push(['nks.stop']), shortcuts: () => '', show() {}, focusSearch: () => document.getElementById('nksb-search')?.focus(), event() {} };
const ST = { state, escapeHTML: String, refresh: rebuild, toast() {}, setView(view) { state.view = view; rebuild(); }, async request(command, payload) { calls.push([command, payload]); if (command === 'library.search') return { items: [], total: 0 }; return {}; } };
const context = vm.createContext({ window: { ST, STKitBrowser: kit, STNKSBrowser: nks }, document, Promise, Set, Map,
  queueMicrotask() {}, setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
web.load(context, 'browser.js', { text: source }); browser = context.window.STBrowser; rebuild();
function key(id, name) { const target = nodes.get(id); target.focus(); const event = { target, key: name, preventDefault() { this.defaultPrevented = true; } }; browser.key(event); }
(async () => {
  key('browser-tab-samples', 'ArrowRight'); assert.equal(browser.controller.tab, 'kits');
  assert.equal(document.activeElement.id, 'browser-tab-kits'); assert.match(markup, /Group preview/);
  key('browser-tab-kits', 'ArrowDown'); assert.equal(document.activeElement.id, 'kitb-search');
  key('kitb-search', 'ArrowDown'); assert(calls.some(call => call[0] === 'kit.key' && call[1] === 'ArrowDown'));
  browser.focusSearch(); assert.equal(document.activeElement.id, 'kitb-search');
  await browser.search(); assert(calls.some(call => call[0] === 'kit.search'));
  assert.equal(browser.controller.list().items[0].name, 'Cloud');
  assert.equal(JSON.stringify(browser.controller.fields), '["sources","expansions","tags"]');
  browser.controller.move(1); browser.controller.page(-1); browser.controller.cycleFilter('expansions', 1);
  browser.controller.preview(); browser.controller.load(); browser.controller.load(true); browser.controller.favorite(); browser.controller.toggleFavorites(); browser.controller.toggleAutoPreview(); browser.controller.clearFilters();
  for (const value of [['kit.move', 1], ['kit.page', -1], ['kit.cycle', 'expansions', 1], ['kit.load', 'stay'], ['kit.load', 'open']]) assert(calls.some(call => JSON.stringify(call) === JSON.stringify(value)));
  console.log('PASS Kits tab roving focus and shared keyboard/hardware dispatch reach the kit browser');

  const stops = calls.filter(call => call[0] === 'kit.stop').length;
  key('browser-tab-kits', 'ArrowRight'); assert.equal(browser.controller.tab, 'nks'); assert.equal(document.activeElement.id, 'browser-tab-nks');
  assert(calls.filter(call => call[0] === 'kit.stop').length > stops);
  browser.showKits(); assert.equal(document.activeElement.id, 'kitb-search'); browser.showPlugins(); assert.equal(browser.controller.tab, 'plugins');
  browser.showKits(); browser.controller.setTab('samples'); assert.equal(browser.controller.tab, 'samples');
  browser.showKits(); browser.stopPreview(); assert(calls.filter(call => call[0] === 'kit.stop').length >= stops + 4);
  console.log('PASS tab changes, plug-in browsing and global view exit all cancel kit previews');

  const count = calls.filter(call => call[0] === 'library.search').length;
  browser.event('library.results', { scanning: false });
  for (const [id, timer] of [...timers]) if (timer.delay <= 500 && timers.delete(id)) timer.fn();
  await Promise.resolve(); await Promise.resolve();
  assert.equal(calls.filter(call => call[0] === 'library.search').length, count, 'inactive samples browser cannot cancel the kit query on the shared native queue');
  assert(calls.some(call => call[0] === 'kit.event' && call[1] === 'library.results'));
  console.log('PASS kit scan events do not start competing searches for the inactive Samples tab');
})().catch(error => { console.error(error); process.exitCode = 1; });
