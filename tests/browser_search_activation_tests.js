'use strict';
// Exercise actual browser event handlers across deferred native search/preview replies.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = web.source('browser.js');
const sample = name => ({ name, path: `/fixture/${name}.wav`, kind: 'sample' });
const response = items => ({ items, total: items.length, facets: { packs: [{ value: 'Vintage drums', count: 1 }] } });

function harness() {
  const calls = [], searches = [], previews = [], nodes = new Map(), timers = new Map();
  let serial = 0, now = 0, browser, markup = '', deferPreview = false;
  const state = { view: 'browser', track: 0, instrument: 0, plugins: [],
    project: { tracks: [{}], instruments: [{}] }, library: { items: [], selected: 0 } };
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null,
    querySelector(selector) { return selector === '.browser-workspace' ? {} : this.querySelectorAll(selector)[0] || null; },
    querySelectorAll(selector) { return [...nodes.values()].filter(node => node.matches(selector)); } };
  const specs = [
    ['browser-search', 'INPUT'], ['browser-results-list', 'DIV'],
    ['browser-result-0', 'DIV', { browserItem: '0' }, '[data-browser-item]'],
    ['browser-star-0', 'BUTTON', { browserStar: '0' }, '[data-browser-star]'],
    ['browser-load', 'BUTTON'], ['browser-load-new', 'BUTTON'], ['browser-preview', 'BUTTON'],
    ['browser-favorites', 'BUTTON'], ['browser-clear-all', 'BUTTON'], ['browser-search-retry', 'BUTTON'], ['outside', 'BUTTON'],
    ['browser-unknown-bpm', 'INPUT', { browserRangeUnknown: 'bpm' }, '[data-browser-range-unknown]'],
    ['browser-kind-sample', 'BUTTON', { browserKind: 'sample' }, '[data-browser-kind]'],
    ['browser-category-kick', 'BUTTON', { browserFacet: 'categories', browserValue: 'Kick' }, '[data-browser-facet]'],
    ['browser-duration-preset-1', 'BUTTON', { browserDurationPreset: '1' }, '[data-browser-duration-preset]'],
    ['browser-group-packs', 'SUMMARY'],
    ['browser-facet-search-packs', 'INPUT', { browserFacetSearch: 'packs' }, '[data-browser-facet-search]']
  ];
  function rebuild() {
    markup = browser.render(); nodes.clear();
    for (const [id, tagName, dataset = {}, selector] of specs) {
      const attributes = markup.match(new RegExp(`<[^>]*\\bid="${id}"[^>]*>`))?.[0] || '';
      const handlers = {};
      const node = { id, tagName, dataset, handlers, value: attributes.match(/\bvalue="([^"]*)"/)?.[1] || '', scrollTop: 0,
        matches(query) { return query.split(',').some(part => part.trim() === selector || part.trim() === `#${id}` || part.trim().toUpperCase() === tagName); },
        closest(query) { return this.matches(query) ? this : query === '.browser-workspace' ? {} : null; },
        addEventListener(type, fn) { handlers[type] = fn; },
        focus() { document.activeElement = this; handlers.focus?.({ target: this }); }, select() {}, scrollIntoView() {},
        click() { return handlers.click?.({ target: this, preventDefault() {}, stopPropagation() {} }); } };
      nodes.set(id, node);
    }
    browser.bind();
  }
  const ST = { state, escapeHTML: String, refresh: rebuild, toast() {},
    async loadSample(file, instrument) { calls.push({ command: 'loadSample', payload: { file, instrument } }); },
    async loadSampleAsNew(file) { calls.push({ command: 'loadSampleAsNew', payload: { file } }); },
    async addInstrument() { calls.push({ command: 'addInstrument' }); return 1; },
    async request(command, payload) {
      calls.push({ command, payload });
      if (command === 'library.search') return new Promise(resolve => searches.push({ payload, resolve }));
      if (command === 'sample.preview') {
        if (deferPreview) return new Promise(resolve => previews.push({ payload, resolve }));
        return { duration: 30 };
      }
      return {};
    } };
  const nksLoads = [];
  const context = vm.createContext({ window: { ST, STNKS: { async load(path, options) { nksLoads.push({ path, options }); return true; } } }, document, Map, Set, Promise, queueMicrotask() {},
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, at: now + delay }); return id; },
    clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'browser.js', { text: source }); browser = context.window.STBrowser; rebuild();
  async function flush() { for (let i = 0; i < 16; i++) await Promise.resolve(); }
  async function tick(ms = 0) {
    now += ms;
    for (const [id, timer] of [...timers]) if (timer.at <= now && timers.delete(id)) timer.fn();
    await flush();
  }
  function input(id, value) {
    const node = nodes.get(id); node.focus(); node.value = value;
    node.handlers.input?.({ target: node });
  }
  function key(id, key, options = {}) {
    const target = nodes.get(id); target.focus();
    const event = { target, key, code: key === ' ' ? 'Space' : key, defaultPrevented: false, stopped: false, bubbled: false,
      preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...options };
    target.handlers.keydown?.(event);
    // app.js ignores prevented events before delegating to the browser.
    if (!event.stopped && !event.defaultPrevented) { event.bubbled = true; event.handled = browser.key(event); }
    return event;
  }
  const count = command => calls.filter(call => call.command === command).length;
  return { browser, ST, nksLoads, state, nodes, calls, searches, previews, tick, flush, input, key, count,
    click(id) { return nodes.get(id).click(); },
    get focused() { return document.activeElement?.id; },
    get markup() { return markup; },
    set deferPreview(value) { deferPreview = value; },
    async prime(items = [sample('old')]) { const pending = browser.search(); searches.at(-1).resolve(response(items)); await pending; await flush(); } };
}

const actionCommands = ['sample.preview', 'library.favorite', 'loadSample', 'loadSampleAsNew', 'addInstrument'];
async function assertInactive(h, label) {
  const before = actionCommands.map(command => h.count(command));
  h.key('browser-search', 'ArrowDown');
  h.key('browser-results-list', 'Enter');
  h.key('browser-results-list', 'Enter', { altKey: true });
  h.key('browser-results-list', ' ');
  h.key('browser-results-list', 'f');
  for (const id of ['browser-load', 'browser-load-new', 'browser-preview', 'browser-star-0']) h.click(id);
  h.nodes.get('browser-result-0').handlers.dblclick?.();
  await h.tick(70);
  assert.deepEqual(actionCommands.map(command => h.count(command)), before, label);
}
async function assertActive(h, expectedPath) {
  h.key('browser-search', 'ArrowDown'); await h.tick(65);
  assert.equal(h.calls.filter(call => call.command === 'sample.preview').at(-1)?.payload.path, expectedPath);
  h.key('browser-results-list', 'f'); await h.flush();
  assert.equal(h.calls.filter(call => call.command === 'library.favorite').at(-1)?.payload.path, expectedPath);
  h.key('browser-results-list', 'Enter'); await h.flush();
  assert.equal(h.calls.filter(call => call.command === 'loadSampleAsNew').at(-1)?.payload.file, expectedPath);
}

(async () => {
  // Old results may remain visible, but neither keyboard nor action buttons may use them.
  const q = harness(); await q.prime();
  const previousRefresh = q.browser.search(), previousReply = q.searches.at(-1);
  q.input('browser-search', 'new');
  await assertInactive(q, 'a query change blocks old results before its debounce fires');
  previousReply.resolve(response([sample('late-old')])); await previousRefresh; await q.flush();
  await assertInactive(q, 'an obsolete reply during debounce must not reactivate old results');
  await q.tick(20);
  assert.equal(q.searches.at(-1).payload.query, 'new');
  await assertInactive(q, 'old results remain inactive while the matching request is pending');
  q.searches.at(-1).resolve(response([sample('new')])); await q.flush();
  await assertActive(q, '/fixture/new.wav');

  // An older query that returns after its replacement has started still cannot unlock actions.
  const late = harness(); await late.prime();
  late.input('browser-search', 'first'); await late.tick(160); const firstReply = late.searches.at(-1);
  late.input('browser-search', 'latest'); await late.tick(160); const latestReply = late.searches.at(-1);
  firstReply.resolve(response([sample('obsolete')])); await late.flush();
  await assertInactive(late, 'a late reply cannot unlock results while its replacement is pending');
  latestReply.resolve(response([sample('latest')])); await late.flush();
  await assertActive(late, '/fixture/latest.wav');

  // A backend cancellation is not a fresh result; an automatic retry restores actions.
  const c = harness(); await c.prime(); c.input('browser-search', 'cancelled'); await c.tick(160);
  c.searches.at(-1).resolve({ cancelled: true }); await c.flush();
  assert.equal(c.searches.length, 3, 'latest cancellation automatically retries the same search');
  await assertInactive(c, 'cancelled requests cannot unlock results from earlier criteria');
  c.searches.at(-1).resolve(response([sample('retry')])); await c.flush();
  await assertActive(c, '/fixture/retry.wav');

  // An explicit search consumes the pending debounce rather than issuing two competing native requests.
  const explicit = harness(); await explicit.prime(); explicit.input('browser-search', 'direct');
  const direct = explicit.browser.search();
  assert.equal(explicit.searches.length, 2);
  await explicit.tick(160);
  assert.equal(explicit.searches.length, 2, 'explicit search consumes its scheduled user-query timer');
  explicit.searches.at(-1).resolve(response([sample('direct')])); await direct;

  // Model the native enqueue inversion observed after unknown BPM -> Clear all.
  const clear = harness(), four = [sample('Bass120'), sample('Bass128'), sample('Kick'), sample('Soft')];
  four[0].bpm = 120; four[1].bpm = 128;
  await clear.prime(four);
  const unknown = clear.nodes.get('browser-unknown-bpm'); unknown.checked = true;
  unknown.handlers.change({ target: unknown });
  const unknownRequest = clear.browser.search();
  assert.deepEqual(JSON.parse(JSON.stringify(clear.searches.at(-1).payload.filters)), { bpm: { includeUnknown: true } });
  clear.searches.at(-1).resolve(response(four.slice(2))); await unknownRequest;
  assert.equal(clear.state.library.total, 2);
  const oldRequest = clear.browser.search(), oldReply = clear.searches.at(-1);
  clear.click('browser-clear-all'); const clearing = clear.browser.search(), clearReply = clear.searches.at(-1);
  assert.deepEqual(JSON.parse(JSON.stringify(clearReply.payload.filters)), {});
  clearReply.resolve({ cancelled: true }); await clear.flush();
  const clearRetry = clear.searches.at(-1);
  assert.notEqual(clearRetry, clearReply, 'cancelled newest clear request is retried');
  assert.deepEqual(JSON.parse(JSON.stringify(clearRetry.payload.filters)), {});
  oldReply.resolve(response(four.slice(2))); await oldRequest;
  await assertInactive(clear, 'late unknown-tempo response cannot unlock the cleared search');
  clearRetry.resolve(response(four)); await clearing; await clear.flush();
  assert.equal(clear.state.library.total, 4, 'automatic retry restores all four sounds after Clear all');
  assert.deepEqual(clear.state.library.items.map(item => item.name), four.map(item => item.name));

  // Stop after three native attempts, retain the changed criteria, and offer an explicit retry.
  const exhausted = harness(); await exhausted.prime(); exhausted.input('browser-search', 'blocked');
  await exhausted.tick(160);
  for (let attempt = 1; attempt <= 3; attempt++) {
    assert.equal(exhausted.searches.length, attempt + 1);
    exhausted.searches.at(-1).resolve({ cancelled: true }); await exhausted.flush();
  }
  assert.equal(exhausted.searches.length, 4, 'automatic retry is bounded to three native attempts');
  assert.match(exhausted.markup, /id="browser-search-retry"/);
  assert.match(exhausted.markup, /Retry search/i, 'exhaustion exposes a visible, actionable recovery');
  await assertInactive(exhausted, 'exhaustion never unlocks old-query audio');
  await exhausted.tick(2000);
  assert.equal(exhausted.searches.length, 4, 'exhaustion does not spin in a retry loop');
  exhausted.click('browser-search-retry'); await exhausted.flush();
  assert.equal(exhausted.searches.length, 5, 'Retry search starts a new bounded attempt');
  assert.equal(exhausted.searches.at(-1).payload.query, 'blocked');
  exhausted.searches.at(-1).resolve(response([sample('recovered')])); await exhausted.flush();
  await assertActive(exhausted, '/fixture/recovered.wav');

  // An obsolete cancellation must neither retry nor reclaim focus from the user's new destination.
  const obsolete = harness(); await obsolete.prime(); obsolete.input('browser-search', 'old'); await obsolete.tick(160);
  const obsoleteReply = obsolete.searches.at(-1);
  obsolete.input('browser-search', 'new'); obsolete.nodes.get('outside').focus();
  const beforeObsoleteReply = obsolete.searches.length;
  obsoleteReply.resolve({ cancelled: true }); await obsolete.flush();
  assert.equal(obsolete.searches.length, beforeObsoleteReply, 'cancelled old criteria do not queue a retry');
  assert.equal(obsolete.focused, 'outside');
  await obsolete.tick(160);
  assert.equal(obsolete.searches.length, beforeObsoleteReply + 1);
  assert.equal(obsolete.searches.at(-1).payload.query, 'new');
  obsolete.searches.at(-1).resolve(response([sample('new')])); await obsolete.flush();
  assert.equal(obsolete.focused, 'outside', 'fresh results also respect the user-moved focus');

  // Every principal filter path invalidates synchronously, even before its zero-delay search starts.
  for (const control of ['browser-category-kick', 'browser-kind-sample', 'browser-favorites', 'browser-duration-preset-1']) {
    const f = harness(); await f.prime(); f.click(control);
    await assertInactive(f, `${control} cannot activate results from the previous filter`);
    assert.equal(f.searches.length, 2);
    f.searches.at(-1).resolve(response([sample(control)])); await f.flush();
    await assertActive(f, `/fixture/${control}.wav`);
  }

  // Invalidating criteria cancels a queued audition and prevents an in-flight reply restoring playback state.
  const queued = harness(); await queued.prime(); queued.key('browser-search', 'ArrowDown');
  queued.input('browser-search', 'other'); await queued.tick(70);
  assert.equal(queued.count('sample.preview'), 0, 'a queued old audition must not reach the audio engine');
  const pending = harness(); await pending.prime(); pending.deferPreview = true;
  pending.key('browser-results-list', ' '); await pending.flush();
  const stops = pending.count('sample.stopPreview'); pending.input('browser-search', 'other'); await pending.flush();
  assert(pending.count('sample.stopPreview') > stops, 'criteria changes stop in-flight audio');
  pending.previews[0].resolve({ duration: 30 }); await pending.flush();
  const afterCancel = pending.count('sample.stopPreview'); pending.key('browser-search', 'Escape'); await pending.flush();
  assert.equal(pending.count('sample.stopPreview'), afterCancel, 'late preview replies must not resurrect preview state');
  assert.equal(pending.focused, 'browser-results-list');

  // Metadata/scan refreshes for unchanged criteria keep already valid results usable.
  const background = harness(); await background.prime();
  background.browser.event('library.metadata', {}); await background.tick(300);
  assert.equal(background.searches.length, 2);
  await assertActive(background, '/fixture/old.wav');
  background.searches.at(-1).resolve(response([sample('refreshed')])); await background.flush();
  await assertActive(background, '/fixture/refreshed.wav');

  // Dispatch through the facet's local listener first, as a real key event does.
  for (const phase of ['queued', 'pending', 'playing']) {
    const e = harness(); await e.prime(); e.deferPreview = phase === 'pending';
    if (phase === 'queued') e.key('browser-search', 'ArrowDown');
    else { e.key('browser-results-list', ' '); await e.flush(); }
    e.input('browser-facet-search-packs', 'Vintage');
    const first = e.key('browser-facet-search-packs', 'Escape');
    assert(first.bubbled && first.handled, `${phase} audition Escape must reach the preview owner`);
    await e.flush();
    assert.equal(e.focused, 'browser-facet-search-packs', `${phase}: first Escape retains facet focus`);
    assert.equal(e.nodes.get('browser-facet-search-packs').value, 'Vintage', `${phase}: first Escape retains facet query`);
    if (phase === 'pending') { e.previews[0].resolve({ duration: 30 }); await e.flush(); }
    await e.tick(70);
    if (phase === 'queued') assert.equal(e.count('sample.preview'), 0);
    const stopCount = e.count('sample.stopPreview');
    const second = e.key('browser-facet-search-packs', 'Escape'); await e.flush();
    assert(second.stopped, `${phase}: second Escape is owned by facet search`);
    assert.equal(e.nodes.get('browser-facet-search-packs').value, '');
    assert.equal(e.focused, 'browser-group-packs');
    assert.equal(e.count('sample.stopPreview'), stopCount, `${phase}: cleared preview must not intercept another Escape`);
  }
  // Native menus execute after dismissal: never apply their label to a different selected sound.
  const menu = harness(); await menu.prime([sample('first'), sample('second')]);
  const actions = menu.browser.contextItems({ target: menu.nodes.get('browser-result-0') });
  menu.state.library.selected = 1;
  await actions.find(action => action.title === 'Add sample on new track').action();
  assert.equal(menu.count('loadSample')+menu.count('loadSampleAsNew'), 0, 'a delayed native menu cannot load a different sound');

  // Vendors may store FX with .nksf. NISI deviceType must determine the destination.
  for (const [extension, deviceType, effect] of [['nksf', 'FX', true], ['nksfx', 'INST', false], ['nksfx', '', true]]) {
    const nks = harness(), request = nks.ST.request;
    nks.ST.request = (command, payload) => command === 'library.nks' ? Promise.resolve({ deviceType }) : request(command, payload);
    await nks.prime([{ path: `/fixture/preset.${extension}`, name: 'Preset', kind: 'nks', extension }]);
    nks.key('browser-results-list', 'Enter'); await nks.flush();
    assert.equal(nks.nksLoads.length, 1);
    assert.equal(nks.nksLoads[0].options.kind === 'effect', effect, `${extension}/${deviceType} destination`);
  }
  const shared = harness(), sharedRequest = shared.ST.request;
  let resolveMetadata, metadataReads = 0;
  shared.ST.request = (command, payload) => command === 'library.nks' ? (++metadataReads, new Promise(resolve => { resolveMetadata = resolve; })) : sharedRequest(command, payload);
  await shared.prime([{ path: '/fixture/shared.nksf', name: 'Shared', kind: 'nks', extension: 'nksf' }]);
  shared.key('browser-results-list', ' '); await shared.flush(); // audition starts the metadata request
  shared.key('browser-results-list', 'Enter'); await shared.flush();
  resolveMetadata({ deviceType: 'FX' }); await shared.flush();
  assert.equal(metadataReads, 1, 'preview and load share one in-flight metadata read');
  assert.equal(shared.nksLoads.length, 1, 'Return waits for metadata already being read by preview');
  assert.equal(shared.nksLoads[0].options.kind, 'effect');
  const moved = harness(), original = moved.ST.request;
  let stop;
  moved.ST.request = (command, payload) => command === 'sample.stopPreview' ? new Promise(resolve => { stop = resolve; }) : original(command, payload);
  await moved.prime(); moved.key('browser-results-list', 'Enter'); await moved.flush();
  moved.state.project = { tracks: [{}], instruments: [{}] }; stop({}); await moved.flush();
  assert.equal(moved.count('loadSample'), 0, 'changing projects while a preview stops cannot load into the new project');
  // Hardware auto-preview and offline selection must cancel the same jobs as the checkbox.
  const hardwarePreview = harness(); await hardwarePreview.prime(); hardwarePreview.deferPreview = true;
  hardwarePreview.key('browser-results-list', ' '); await hardwarePreview.flush();
  hardwarePreview.browser.controller.toggleAutoPreview(); await hardwarePreview.flush();
  assert.equal(hardwarePreview.count('sample.stopPreview'), 1, 'hardware auto-preview off cancels pending audition');
  hardwarePreview.previews[0].resolve({ duration: 30 }); await hardwarePreview.flush();
  assert.equal(hardwarePreview.browser.controller.list().preview, false, 'late reply cannot reactivate disabled audition');
  const offlinePreview = harness(); await offlinePreview.prime([sample('online'), { ...sample('offline'), available: false }]);
  offlinePreview.key('browser-results-list', ' '); await offlinePreview.flush();
  offlinePreview.key('browser-results-list', 'ArrowDown'); await offlinePreview.tick(70);
  assert.equal(offlinePreview.count('sample.stopPreview'), 1, 'selecting an offline sound stops the previous preview');
  assert.equal(offlinePreview.browser.controller.list().preview, false);
  const unavailablePreview = harness(); await unavailablePreview.prime([sample('online'), { path: '/fixture/preset.nksf', name: 'No preview', kind: 'nks' }]);
  unavailablePreview.key('browser-results-list', ' '); await unavailablePreview.flush();
  unavailablePreview.key('browser-results-list', 'ArrowDown'); await unavailablePreview.tick(70);
  assert.equal(unavailablePreview.count('sample.stopPreview'), 1, 'NKS metadata without a preview stops the previous sound');
  assert.equal(unavailablePreview.browser.controller.list().preview, false);
  const navigation = harness(), items = ['one','two','three','four'].map(sample);
  await navigation.prime(items);
  const refreshing = navigation.browser.search();
  navigation.key('browser-results-list', 'End');
  navigation.key('browser-results-list', 'ArrowUp');
  assert.equal(navigation.state.library.selected, 2);
  // Reordering the refreshed result must retain the latest selected identity, not the old index.
  navigation.searches.at(-1).resolve(response([items[2],items[0],items[1],items[3]]));
  await refreshing;
  assert.equal(navigation.state.library.items[navigation.state.library.selected].path, items[2].path);
  assert.equal(navigation.focused, 'browser-results-list');
  console.log('PASS delayed background search preserves newer keyboard selection by identity');
  console.log('PASS browser native menu selection, NKS metadata instrument/effect routing, and project destination guards');
  console.log('Browser search activation: passed debounced/pending/stale/cancelled searches, filter transitions, fresh results, preview invalidation, background refresh usability, and actual facet Escape bubbling in queued/pending/playing states.');
})().catch(error => { console.error(error); process.exitCode = 1; });
