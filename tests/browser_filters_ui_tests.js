'use strict';
// Browser contract and keyboard regressions; no installed audio libraries or plug-ins required.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const installEngineSyncBoundary = require('./helpers/engine_sync_boundary');
const source = web.source('browser.js');
const plain = value => JSON.parse(JSON.stringify(value));
const response = (overrides = {}) => ({ items: [], total: 0, facets: {}, scanning: false, ...overrides });
function harness() {
  const calls = [], toasts = [], nodes = new Map(), selectors = new Map(), timers = new Map();
  let nextTimer = 0, now = 0, refreshes = 0, reply = response();
  const state = { view: 'browser', track: 0, instrument: 0, plugins: [], project: { instruments: [{ name: 'Sample' }], tracks: [{ name: 'Track 1' }] }, library: { roots: [], items: [] } };
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelector: selector => selector === '.browser-workspace' ? {} : (selectors.get(selector) || [])[0] || null, querySelectorAll: selector => selectors.get(selector) || [] };
  const ST = { state, escapeHTML: value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'), refresh() { refreshes++; }, toast(message, kind) { toasts.push({message,kind}); }, mutate(label, fn) { fn(); }, async request(command, payload) { calls.push({ command, payload }); return typeof reply === 'function' ? reply(command, payload) : reply; } };
  const syncs = installEngineSyncBoundary(ST);
  const context = vm.createContext({ window: { ST }, document, Map, Set, Promise, setTimeout(fn, delay) { const id = ++nextTimer; timers.set(id, { fn, at: now + delay }); return id; }, clearTimeout(id) { timers.delete(id); }, queueMicrotask() {} });
  web.load(context, 'browser.js', { text: source });
  const browser = context.window.STBrowser;
  function node(id, dataset = {}, selector, extra = {}) {
    const handlers = {};
    const value = { id, dataset, tagName: 'BUTTON', value: '', handlers, addEventListener(type, fn) { handlers[type] = fn; }, focus() { document.activeElement = this; handlers.focus?.({ target: this }); }, select() {}, scrollIntoView() {}, closest() { return null; }, ...extra };
    value.fire = (type, options = {}) => handlers[type]?.({ target: value, preventDefault() {}, stopPropagation() {}, ...options });
    nodes.set(id, value);
    if (selector) { if (!selectors.has(selector)) selectors.set(selector, []); selectors.get(selector).push(value); }
    return value;
  }
  async function tick(ms = 0) {
    now += ms;
    for (const [id, timer] of [...timers]) if (timer.at <= now && timers.delete(id)) timer.fn();
    for (let i = 0; i < 8; i++) await Promise.resolve();
  }
  const searches = () => calls.filter(call => call.command === 'library.search');
  return { browser, state, document, calls, syncs, toasts, node, selectors, tick, searches, window: context.window, get refreshes() { return refreshes; }, set reply(value) { reply = value; } };
}
const facet = (h, field, value) => h.node(`${field}-${value}`, { browserFacet: field, browserValue: value }, '[data-browser-facet]');

(async () => {
  // Multiple choices in one category remain selected; provenance, packs and keys combine as separate facets.
  const h = harness(), kick = facet(h, 'categories', 'Kick'), snare = facet(h, 'categories', 'Snare'), sourceChoice = facet(h, 'collections', 'Maschine expansions'), pack = facet(h, 'packs', 'Electric Touch'), key = facet(h, 'keys', 'F#minor');
  h.browser.render(); h.browser.bind();
  h.state.library.offset = 300; h.state.library.selected = 7;
  kick.fire('click'); snare.fire('click'); sourceChoice.fire('click'); pack.fire('click'); key.fire('click');
  await h.tick();
  assert.equal(h.searches().length, 1, 'rapid facet choices should coalesce into one native search');
  assert.deepEqual(plain(h.searches()[0].payload), { query: '', kind: 'all', filters: { categories: ['Kick', 'Snare'], collections: ['Maschine expansions'], packs: ['Electric Touch'], keys: ['F#minor'] }, favorites: false, offset: 0, limit: 150 });
  assert.equal(h.state.library.selected, 0);
  const firstPayload = h.searches()[0].payload;
  kick.fire('click');
  assert.deepEqual(plain(firstPayload.filters.categories), ['Kick', 'Snare'], 'later edits must not mutate an in-flight request');
  await h.tick();
  assert.deepEqual(plain(h.searches().at(-1).payload.filters.categories), ['Snare']);

  // Disjunctive zero-result selections remain removable and counts cannot become a hidden filter.
  h.reply = response({ facets: { categories: [{ value: 'Kick', count: 12 }], packs: [{ value: '<Bright & Soft>', count: 2 }], collections: [{ value: 'Loopcloud', count: 42 }] } });
  await h.browser.search();
  let html = h.browser.render();
  assert.match(html, /aria-checked="true" data-browser-facet="categories" data-browser-value="Snare"/);
  assert(html.includes('Category: Snare') && html.includes('Source: Maschine expansions'));
  assert(html.includes('&lt;Bright &amp; Soft&gt;'));
  assert(html.includes('F♯ minor'));
  assert(!html.includes('data-browser-category='), 'categories must not fall back to exclusive old buttons');

  // One Tab stop per facet; arrows focus options without changing sound selection or starting audition.
  const group = { open: true, querySelectorAll: () => [kick, snare], querySelector: () => summary };
  const summary = h.node('category-summary', {}, null, { tagName: 'SUMMARY' });
  kick.closest = snare.closest = selector => selector === '.browser-facet' || selector === '[data-facet-options]' ? group : null;
  let prevented = false;
  const previewBefore = h.calls.filter(call => call.command === 'sample.preview').length;
  assert(h.browser.key({ target: kick, key: 'ArrowDown', preventDefault() { prevented = true; } }));
  assert(prevented); assert.equal(h.document.activeElement, snare); assert.equal(snare.tabIndex, 0); assert.equal(kick.tabIndex, -1);
  assert.equal(h.calls.filter(call => call.command === 'sample.preview').length, previewBefore);
  h.browser.key({ target: snare, key: 'ArrowLeft', preventDefault() {} });
  assert.equal(group.open, false); assert.equal(h.document.activeElement, summary);
  html = h.browser.render();
  const categoryMarkup = html.slice(html.indexOf('data-facet-options="categories"'), html.indexOf('data-browser-filter-group="packs"'));
  assert.equal((categoryMarkup.match(/tabindex="0"/g) || []).length, 1);

  // Numeric ranges have exact inclusive bounds; unknown-only is explicit instead of silently excluding files.
  const r = harness();
  const min = r.node('min', { browserRange: 'duration', bound: 'min' }, '[data-browser-range]', { tagName: 'INPUT', value: '1' });
  const max = r.node('max', { browserRange: 'duration', bound: 'max' }, '[data-browser-range]', { tagName: 'INPUT', value: '5' });
  const unknown = r.node('unknown', { browserRangeUnknown: 'duration' }, '[data-browser-range-unknown]', { tagName: 'INPUT', type: 'checkbox', checked: true });
  r.browser.render(); r.browser.bind(); min.fire('change'); max.fire('change'); unknown.fire('change'); await r.tick();
  assert.deepEqual(plain(r.searches().at(-1).payload.filters.duration), { min: 1, max: 5, includeUnknown: true });
  assert(r.browser.render().includes('Also show unknown lengths'));
  min.value = ''; max.value = ''; min.fire('change'); max.fire('change'); await r.tick();
  assert.deepEqual(plain(r.searches().at(-1).payload.filters.duration), { includeUnknown: true });
  assert(r.browser.render().includes('Unknown lengths only'));
  prevented = false;
  assert(r.browser.key({ target: unknown, key: ' ', code: 'Space', preventDefault() { prevented = true; } }));
  assert(!prevented, 'checkbox Space must retain native toggle and suppress transport');
  unknown.checked = false; unknown.fire('change'); await r.tick();
  assert(!('duration' in r.searches().at(-1).payload.filters));

  // Reversed numeric bounds are repaired visibly; invalid text cannot poison native filter values.
  min.value = '9'; max.value = '3'; min.fire('change'); max.fire('change'); await r.tick();
  assert.deepEqual(plain(r.searches().at(-1).payload.filters.duration), { min: 3, max: 3, includeUnknown: false });
  const count = r.searches().length; min.value = '-8'; min.fire('change'); await r.tick();
  assert.equal(r.searches().length, count);

  // Group search narrows only pack choices, preserving the user's text query and complete backend search.
  const f = harness();
  f.state.library.query = 'warm';
  f.state.library.facets = { packs: [{ value: 'Electric Touch', count: 3 }, { value: 'Velvet Lounge', count: 5 }] };
  const findPack = f.node('browser-facet-search-packs', { browserFacetSearch: 'packs' }, '[data-browser-facet-search]', { tagName: 'INPUT', value: 'velvet' });
  f.browser.render(); f.browser.bind(); findPack.fire('input'); await f.tick(500);
  html = f.browser.render();
  assert(html.includes('data-browser-value="Velvet Lounge"')); assert(!html.includes('data-browser-value="Electric Touch"'));
  assert.equal(f.state.library.query, 'warm'); assert.equal(f.searches().length, 0);

  // Per-chip removal leaves all other filters intact; Clear all really clears query/kind/favorites too.
  const c = harness();
  c.state.library = { query: 'drum', kind: 'sample', favorites: true, filters: { categories: ['Kick', 'Snare'], roots: ['/Volumes/Sound Drive/Loopcloud'], sampleTypes: ['oneshot'], duration: { max: 1 } } };
  const remove = c.node('remove-kick', { browserRemoveFilter: 'categories', browserRemoveValue: 'Kick' }, '[data-browser-remove-filter]');
  const clear = c.node('browser-clear-all');
  c.browser.render(); c.browser.bind(); remove.fire('click'); await c.tick();
  assert.deepEqual(plain(c.searches().at(-1).payload.filters.categories), ['Snare']);
  assert.equal(c.searches().at(-1).payload.query, 'drum'); assert.equal(c.searches().at(-1).payload.favorites, true);
  clear.fire('click'); await c.tick();
  assert.deepEqual(plain(c.searches().at(-1).payload.filters), {});
  assert.equal(c.searches().at(-1).payload.kind, 'all'); assert.equal(c.searches().at(-1).payload.query, ''); assert.equal(c.searches().at(-1).payload.favorites, false);
  assert(!c.browser.render().includes('aria-label="Active search filters"'));

  // User changes invalidate old queries immediately, even before the next debounce fires.
  const stale = harness(), staleKick = facet(stale, 'categories', 'Kick');
  let resolveOld;
  stale.reply = command => command === 'library.search' ? new Promise(resolve => { resolveOld = resolve; }) : {};
  stale.browser.render(); stale.browser.bind();
  const pending = stale.browser.search(); staleKick.fire('click');
  resolveOld(response({ items: [{ name: 'Wrong old result', path: '/old.wav' }], total: 1 })); await pending;
  assert.equal(stale.state.library.items.length, 0);
  stale.reply = response({ items: [{ name: 'Correct result', path: '/kick.wav' }], total: 1 }); await stale.tick();
  assert.equal(stale.state.library.items[0].name, 'Correct result');
  stale.reply = response({ cancelled: true }); await stale.browser.search();
  assert.equal(stale.state.library.items[0].name, 'Correct result', 'superseded backend responses cannot clear the list');

  // Metadata bursts coalesce; a user query keeps its earlier deadline and structured filters.
  const m = harness();
  const search = m.node('browser-search', {}, null, { tagName: 'INPUT' });
  m.state.library.filters = { sampleTypes: ['loop'], duration: { min: 3 } };
  m.browser.render(); m.browser.bind();
  for (let i = 0; i < 50; i++) m.browser.event('library.metadata', { durationCoverage: { known: 80, unknown: 20, pending: 2 } });
  await m.tick(299); assert.equal(m.searches().length, 0); await m.tick(1); assert.equal(m.searches().length, 1);
  search.value = 'bass'; search.fire('input');
  for (let i = 0; i < 50; i++) m.browser.event('library.metadata', {});
  await m.tick(160); assert.equal(m.searches().length, 2);
  assert.equal(m.searches().at(-1).payload.query, 'bass');
  assert.deepEqual(plain(m.searches().at(-1).payload.filters), { sampleTypes: ['loop'], duration: { min: 3 } });

  // Generic plug-in catalog distinguishes duplicate names by format and loads the selected VST3 ID.
  const p = harness();
  p.state.plugins = [
    { id: 'au:echo', name: 'Echo', manufacturer: 'Maker', type: 'effect', format: 'AU' },
    { id: 'vst3:echo', name: 'Echo', manufacturer: 'Maker', type: 'effect', format: 'VST3', bundlePath: '/Users/test/Library/Audio/Plug-Ins/VST3/Echo.vst3' },
    { id: 'au:keys', name: 'Keys', manufacturer: 'Maker', type: 'instrument' }
  ];
  const vst = p.node('format-vst', { browserPluginFormat: 'VST3' }, '[data-browser-plugin-format]');
  const all = p.node('format-all', { browserPluginFormat: 'all' }, '[data-browser-plugin-format]');
  const load = p.node('browser-plugin-load');
  p.reply = command => command === 'plugin.load' ? { name: 'Echo', format: 'VST3' } : command === 'plugin.state' ? [] : {};
  p.browser.showPlugins('effect'); p.browser.bind();
  html = p.browser.render(); assert(html.includes('>Plug-ins <span>')); assert(html.includes('1 effects'), 'All shows one version per plug-in');
  assert(html.includes('browser-format-badge">AU') && !html.includes('browser-format-badge">VST3'), 'AU is preferred over VST3');
  vst.fire('click'); html = p.browser.render(); assert(html.includes('1 effects')); assert(!html.includes('browser-format-badge">AU'));
  await load.fire('click');
  assert.equal(p.calls.find(call => call.command === 'plugin.load').payload.component, 'vst3:echo');
  assert.equal(p.calls.find(call => call.command === 'plugin.load').payload.bundlePath, '/Users/test/Library/Audio/Plug-Ins/VST3/Echo.vst3');
  all.fire('click'); assert(p.browser.render().includes('1 effects'));
  p.browser.showPlugins('instrument'); html = p.browser.render();
  assert(html.includes('browser-format-badge">AU'), 'legacy AU descriptors retain their format');

  // A stale plug-in attachment restores the current project only after pending edits settle.
  for (const outcome of ['latest', 'replaced-again', 'failed']) {
    const b = harness(); let finishLoad, finishSync, enterSync;
    const syncing = new Promise(resolve => { enterSync = resolve; }), gate = new Promise(resolve => { finishSync = resolve; });
    b.state.plugins = [{ id: 'au:fixture', name: 'Fixture keys', type: 'instrument' }];
    const button = b.node('browser-plugin-load');
    b.window.ST.beforeEngineSync = () => { enterSync(); return gate; };
    b.reply = command => command === 'plugin.load' ? new Promise(resolve => { finishLoad = resolve; }) : command === 'engine.project' && outcome === 'failed' ? Promise.reject(new Error('Stale plug-in restoration failed')) : {};
    b.browser.showPlugins('instrument'); b.browser.bind(); const loading = button.fire('click');
    const original = b.state.project; b.state.project = plain(original); finishLoad({ name: 'Fixture keys' }); await syncing;
    assert(!b.calls.some(call => call.command === 'engine.project'));
    b.state.project.audioPlugins = [{ state: 'settled mapped edit' }];
    if (outcome === 'replaced-again') b.state.project = plain(original);
    finishSync(); await loading;
    const native = b.calls.filter(call => call.command === 'engine.project');
    if (outcome === 'replaced-again') assert.equal(native.length, 0);
    else { assert.equal(native.length, 1); assert.equal(native[0].payload.project.audioPlugins[0].state, 'settled mapped edit'); }
    assert.equal(original.instruments[0].name, 'Sample'); assert.equal(b.state.project.instruments[0].name, 'Sample');
    if (outcome === 'failed') { assert(b.toasts.some(toast => toast.kind === 'error' && /restoration failed/.test(toast.message))); assert(!b.toasts.some(toast => toast.kind === 'info')); }
    // A rejected restoration releases the browser's local lock for the next action.
    b.reply = command => command === 'plugin.load' ? { name: 'Fixture keys' } : command === 'plugin.state' ? [] : {};
    await button.fire('click'); assert.equal(b.calls.filter(call => call.command === 'plugin.load').length, 2);
  }

  // NKS loading reads its device metadata once, then loads and supports Option-Return instrument creation.
  const n = harness(), nksCalls = [];
  n.state.library.items = [{ name: 'Mapped preset', path: '/presets/mapped.nksf', kind: 'nks' }];
  n.window.STNKS = { async load(file, options) { nksCalls.push({ file, options }); return true; } };
  const nksLoad = n.node('browser-load'), nksNew = n.node('browser-load-new'), nksInspect = n.node('browser-inspect-nks');
  n.browser.render(); n.browser.bind();
  assert(n.browser.render().includes('Load preset'));
  await nksLoad.fire('click'); await nksNew.fire('click');
  assert.deepEqual(plain(nksCalls), [{ file: '/presets/mapped.nksf', options: { newInstrument: false } }, { file: '/presets/mapped.nksf', options: { newInstrument: true } }]);
  assert.equal(n.calls.filter(call => call.command === 'library.nks').length, 1, 'routing reads metadata once and reuses it for the next load');
  await nksInspect.fire('click'); assert(n.calls.some(call => call.command === 'library.nks'));

  console.log('Browser filters: 12 regression groups passed (multi-select contract, zero-result facets, keyboard focus, unknown metadata, numeric validation, list search, chips/clear, stale results, metadata coalescing, AU/VST3 filtering/loading, deferred stale plug-in restoration, direct NKS load).');
})().catch(error => { console.error(error); process.exitCode = 1; });
