'use strict';
// Observable browse, audition and import behaviour with delayed native replies.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('kit-browser.js');
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = async () => { for (let n = 0; n < 24; n++) await Promise.resolve(); };
function harness() {
  const calls = [], loads = [], notices = [], timers = new Map(), nodes = new Map();
  let serial = 0, markup = '', mounted = true, handler = null, browser;
  const items = [
    { kind: 'kit', path: '/Factory/Deep/Kits/Cloud.mxgrp', name: 'Cloud', product: 'Deep', bank: '', vendor: 'Native Instruments', types: ['Kits'], subtypes: ['Kits\u001fHip Hop'], characters: [], extension: 'mxgrp', user: false, art: 'image/deep', productArt: 'image/deep', color: '#336699', previewPath: '/Factory/Deep/Kits/.previews/Cloud.mxgrp.ogg' },
    { kind: 'kit', path: '/Factory/Molten Veil/Kits/Rain.mxgrp', name: 'Rain', product: 'Molten Veil', bank: '', vendor: 'Native Instruments', types: ['Kits'], subtypes: ['Kits\u001fAnalog Kit'], characters: [], extension: 'mxgrp', user: false },
    { kind: 'kit', path: '/Mine/Break.stkit', name: 'Break', product: 'Saved kits', bank: '', types: ['Kits'], subtypes: [], characters: ['Breakbeat'], extension: 'stkit', user: true, favorite: true, soundCount: 4 }
  ];
  function browse(payload) {
    const facet = (list, values) => { const counts = new Map(); for (const item of list) for (const value of values(item)) counts.set(value, (counts.get(value) || 0) + 1); return [...counts].map(([name, count]) => ({ name, count, label: name.split('\u001f').pop() })); };
    let pool = items.filter(item => !payload.favorites || item.favorite);
    if (payload.query) pool = pool.filter(item => item.name.toLowerCase().includes(payload.query.toLowerCase()));
    const counts = { library: pool.filter(item => !item.user).length, user: pool.filter(item => item.user).length };
    pool = pool.filter(item => !!item.user === !!payload.user);
    const products = facet(pool, item => [item.product]).map(product => ({ ...product, vendor: product.name === 'Saved kits' ? '' : 'Native Instruments', ...(product.name === 'Deep' ? { art: 'image/deep' } : {}) }));
    if (payload.product) pool = pool.filter(item => item.product === payload.product);
    for (const field of ['types', 'subtypes', 'characters']) if (payload[field]?.length) pool = pool.filter(item => item[field].some(value => payload[field].includes(value)));
    const page = pool.slice(payload.offset, payload.offset + payload.limit);
    return { items: page, total: pool.length, products, banks: [], subBanks: [], types: facet(pool, item => item.types), subtypes: facet(pool, item => item.subtypes), characters: facet(pool, item => item.characters), counts, status: { reading: false, revision: 1 }, scanning: false };
  }
  const state = { view: 'browser', project: complete({ name: 'Test', tracks: [{}], instruments: [{}] }), track: 0, instrument: 0 };
  const document = {
    activeElement: null,
    getElementById: id => markup.includes(`id="${id}"`) ? node(id) : null,
    querySelector: selector => selector === '.kitb-layout' && mounted ? {} : null,
    querySelectorAll: () => []
  };
  function node(id, context = '') {
    if (!nodes.has(id)) nodes.set(id, {
      id, context, tagName: id.includes('search') ? 'INPUT' : id === 'kitb-results' ? 'DIV' : 'BUTTON', dataset: {}, handlers: {}, scrollTop: 0,
      focus() { document.activeElement = this; }, select() { this.selectedText = true; }, scrollIntoView() {},
      closest(selector) { if (selector === '.kitb-layout' && id.startsWith('kitb-')) return {}; if (selector === this.context) return {}; if (selector === 'button, a, summary' && this.tagName === 'BUTTON') return this; return null; },
      addEventListener(type, fn) { this.handlers[type] = fn; },
      setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
    });
    return nodes.get(id);
  }
  const ST = { state, escapeHTML, toast: text => notices.push(text), refresh() { if (mounted) { markup = browser.render(); browser.bind(); } },
    async request(command, payload) {
      calls.push({ command, payload });
      if (handler) { const value = handler(command, payload); if (value !== undefined) return value; }
      if (command === 'library.kits.browse') return browse(payload);
      if (command === 'kit.detail') return { sounds: [{ slot: 0, name: 'Kick', type: 'sample' }, { slot: 15, name: 'Air', plugin: 'Massive', missing: true }], compatibility: { summary: 'One instrument is missing.', warnings: ['Some effects are approximations.'], missingPlugins: ['Massive'] } };
      if (command === 'sample.preview') return { duration: 2 };
      return {};
    }
  };
  const groups = require('../ui/track-groups.js');
  const window = { ST, STGroups: groups, STKitGroups: { async loadKit(file, options) { loads.push({ file, options }); return true; }, createKit() { calls.push({ command: 'create' }); },
    cursorGroup: () => groups.ofTrack(state.project, state.track) } };
  const context = vm.createContext({ window, document, Map, Set, Promise, queueMicrotask: fn => Promise.resolve().then(fn),
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'kit-browser.js', { text: source }); browser = window.STKitBrowser;
  const render = () => { markup = browser.render(); browser.bind(); };
  const run = async () => { for (let round = 0; round < 5; round++) { await tick(); for (const [id, timer] of [...timers]) if (timer.delay < 1000 && timers.delete(id)) timer.fn(); } await tick(); };
  function key(id, name, options = {}) {
    const target = node(id); document.activeElement = target;
    const event = { target, key: name, code: name === ' ' ? 'Space' : name, preventDefault() { this.defaultPrevented = true; }, ...options };
    return { handled: browser.key(event), prevented: !!event.defaultPrevented };
  }
  const input = (id, text) => { const value = node(id); value.value = text; value.handlers.input?.({ target: value }); };
  return { browser, kits:window.STKitGroups, calls, loads, notices, items, state, render, run, key, input, node, document,
    last: command => calls.filter(call => call.command === command).at(-1),
    get markup() { return markup; }, get focused() { return document.activeElement?.id; },
    set mounted(value) { mounted = value; }, set requestHandler(value) { handler = value; } };
}
async function fresh() { const h = harness(); await h.browser.search(); h.render(); await h.run(); return h; }


(async () => {
  const h = await fresh();
  const first = h.last('library.kits.browse').payload;
  assert.equal(first.user, false); assert.equal(first.limit, 200);
  assert.match(h.markup, /aria-label="Kits"/); assert.match(h.markup, /Group preview/);
  assert.match(h.markup, /Load new kit/); assert.match(h.markup, /15 · empty|Pad 16/);
  assert.match(h.markup, /Missing plug-in: Massive/);
  assert.equal((h.markup.match(/class="filled/g) || []).length, 2, 'only read sound slots render as occupied');
  assert.match(h.markup, /id="kitb-product-1"[^>]*title="Deep · Native Instruments"/);
  assert.match(h.markup, /data-kitb-art="image\/deep" data-kitb-variant="tile"/, 'expansion artwork is requested from the shared NKS artwork index');
  assert.match(h.markup, /Hip Hop/, 'rows show the Maschine sub-type');
  assert.match(h.markup, /Library<small>2<\/small>/); assert.match(h.markup, /My kits<small>1<\/small>/);
  console.log('PASS kit browser renders expansions with artwork keys, Library / My kits counts, group preview and 16 pads');

  h.browser.controller.choose('expansions', 'Deep'); await h.run();
  assert.equal(h.last('library.kits.browse').payload.product, 'Deep');
  assert.match(h.markup, /class="nksb-hero kitb-hero"/); assert.match(h.markup, /id="kitb-crumb-all"/);
  assert.equal(h.browser.controller.filterValue('expansions'), 'Deep');
  h.browser.controller.choose('tags', 'Kits\u001fHip Hop'); await h.run();
  assert.equal(JSON.stringify(h.last('library.kits.browse').payload.subtypes), '["Kits\\u001fHip Hop"]');
  assert.equal(h.browser.controller.filterValue('tags'), 'Hip Hop');
  assert.deepEqual(JSON.parse(JSON.stringify(h.browser.controller.options('tags').map(option => option.label))), ['Hip Hop']);
  h.browser.controller.up('all'); await h.run();
  assert.equal(h.last('library.kits.browse').payload.product, undefined, 'the All expansions crumb steps back out');
  h.browser.controller.clear(); await h.run(); h.browser.controller.toggleUser(); await h.run();
  assert.equal(h.last('library.kits.browse').payload.user, true);
  assert.equal(h.browser.controller.list().items[0].name, 'Break'); assert.equal(h.browser.controller.list().user, true);
  assert.match(h.markup, /class="nksb-art nksb-row-art "[^>]*style="--nksb-tint:#[0-9a-f]{6}"><span>SK<\/span>/, 'user kits get a stable colour and initials instead of NI artwork');
  assert.match(h.markup, /Breakbeat/);
  h.browser.controller.toggleUser(); await h.run(); h.browser.controller.toggleFavorites(); await h.run();
  assert.equal(h.browser.controller.list().items.length, 0, 'favorites stay within Library');
  h.browser.controller.toggleUser(); await h.run(); assert.equal(h.browser.controller.list().items[0].name, 'Break');
  await h.browser.controller.favorite(); await h.run(); assert.equal(h.browser.controller.list().items.length, 0);
  assert.equal(h.last('library.favorite').payload.path, '/Mine/Break.stkit');
  console.log('PASS expansion › sub-type drill-down, breadcrumbs, My kits with colour fallback and favorites');

  const keyboard = await fresh();
  keyboard.key('kitb-search', 'ArrowDown'); assert.equal(keyboard.focused, 'kitb-results');
  keyboard.key('kitb-results', 'ArrowDown'); assert.equal(keyboard.browser.controller.list().selected, 1);
  keyboard.key('kitb-results', 'ArrowUp'); await keyboard.run();
  assert.equal(keyboard.last('sample.preview').payload.path, keyboard.items[0].previewPath, 'preview plays the group asset, not its archive');
  assert.equal(keyboard.loads.length, 0, 'audition never imports a kit');
  keyboard.key('kitb-results', ' '); await tick(); assert(keyboard.calls.some(call => call.command === 'sample.stopPreview'));
  keyboard.key('kitb-results', 'ArrowLeft'); assert.equal(keyboard.focused, 'kitb-products');
  keyboard.key('kitb-products', 'ArrowDown'); await keyboard.run();
  assert.equal(keyboard.last('library.kits.browse').payload.product, 'Deep', '↓ in the expansion list chooses the next expansion');
  keyboard.key('kitb-products', 'm'); await keyboard.run();
  assert.equal(keyboard.last('library.kits.browse').payload.product, 'Molten Veil', 'type-ahead jumps to an expansion');
  keyboard.key('kitb-products', 'ArrowRight'); assert.equal(keyboard.focused, 'kitb-results');
  keyboard.key('kitb-results', 'Backspace'); await keyboard.run();
  assert.equal(keyboard.last('library.kits.browse').payload.product, undefined, 'Backspace goes up a level');
  keyboard.key('kitb-results', 'ArrowRight'); assert.equal(keyboard.focused, 'kitb-load');
  keyboard.node('kitb-load').context = '.kitb-detail'; keyboard.key('kitb-load', 'ArrowLeft'); assert.equal(keyboard.focused, 'kitb-results');
  assert.equal(keyboard.key('kitb-load', 'ArrowDown').handled, false, 'detail controls do not silently browse kits');
  assert.equal(keyboard.key('kitb-load', 'Enter').prevented, false, 'buttons keep native keyboard activation');
  for (const name of ['ArrowLeft', 'ArrowRight', 'Home', 'End']) assert.equal(keyboard.key('kitb-search', name).handled, false, 'text fields retain native editing');
  assert.equal(keyboard.key('kitb-results', 'ArrowDown', { isComposing: true }).handled, false);
  keyboard.key('kitb-results', 'f', { metaKey: true }); assert.equal(keyboard.focused, 'kitb-search');
  keyboard.key('kitb-results', 'Enter'); await tick(); keyboard.key('kitb-results', 'Enter', { shiftKey: true }); await tick();
  assert.equal(JSON.stringify(keyboard.loads.map(value => value.options)), '[{"stay":true},{"stay":false}]');
  console.log('PASS keyboard: expansions list, type-ahead, Backspace up a level, panels, native editing, audition versus add');

  const stale = await fresh(), response = deferred();
  stale.requestHandler = command => command === 'library.kits.browse' ? response.promise : undefined;
  const searching = stale.browser.search(); stale.browser.controller.choose('expansions', 'Deep');
  await stale.browser.controller.load(); await stale.browser.controller.preview();
  assert.equal(stale.loads.length, 0); assert.equal(stale.calls.some(call => call.command === 'sample.preview'), false);
  response.resolve({ items: [{ path: '/old.mxgrp', name: 'Old kit' }], total: 1 }); await searching;
  assert.equal(stale.browser.controller.list().items.length, 0);
  stale.requestHandler = null; await stale.run(); assert.equal(stale.browser.controller.list().items[0].name, 'Cloud');
  console.log('PASS changing filters invalidates old results and actions before the debounce finishes');

  const pending = await fresh(), audio = deferred();
  pending.requestHandler = command => command === 'sample.preview' ? audio.promise : undefined;
  const audition = pending.browser.controller.preview(); pending.browser.controller.move(1);
  audio.resolve({ duration: 10 }); await audition; await pending.run();
  assert.equal(pending.browser.previewActive, false, 'a group without a preview clears the previous pending audition');
  assert(pending.calls.some(call => call.command === 'sample.stopPreview'));
  const detailRace = await fresh(), details = deferred();
  detailRace.browser._local.metadata.clear(); delete detailRace.items[0].previewPath;
  detailRace.requestHandler = command => command === 'kit.detail' ? details.promise : undefined;
  const previewWait = detailRace.browser.controller.preview(); detailRace.browser.controller.move(1);
  details.resolve({ previewPath: '/late.ogg' }); await previewWait;
  assert.equal(detailRace.calls.some(call => call.command === 'sample.preview'), false);
  const ended = await fresh(); await ended.browser.controller.preview(); assert.match(ended.markup, /Stop group preview/);
  ended.browser.event('sample.previewEnded', { path: ended.items[0].previewPath }); assert.doesNotMatch(ended.markup, /Stop group preview/);
  console.log('PASS late preview and metadata replies cannot resurrect a previous audition; finished previews reset the controls');

  const leaving = await fresh(), stop = deferred();
  await leaving.browser.controller.preview(); leaving.requestHandler = command => command === 'sample.stopPreview' ? stop.promise : undefined;
  const adding = leaving.browser.controller.load(); leaving.state.project = complete({ name: 'Different project' });
  stop.resolve({}); await adding; assert.equal(leaving.loads.length, 0);
  const menu = await fresh();
  const actions = menu.browser.contextItems({ target: { closest: selector => selector === '[data-kitb-row]' ? { dataset: { kitbRow: '0' } } : null } });
  menu.browser.controller.move(1); await actions.find(action => action.title === 'Load new kit').action();
  assert.equal(menu.loads.length, 0);
  assert(actions.some(action => action.title === 'Show all Deep kits'));
  const tab = await fresh(); await tab.browser.controller.preview(); await tab.browser.stopPreview(); tab.mounted = false;
  assert.equal(tab.browser.previewActive, false); await tab.browser.controller.load(); assert.equal(tab.loads.length, 0);
  console.log('PASS project, context-menu and tab changes cannot retarget an asynchronous kit import');

  const offline = await fresh(); offline.items[0].available = false; offline.items[0].volume = 'Sound Drive';
  await offline.browser.search(); await offline.browser.controller.preview(); await offline.browser.controller.load();
  assert.equal(offline.calls.some(call => call.command === 'sample.preview'), false); assert.equal(offline.loads.length, 0);
  assert.match(offline.markup, /Reconnect its drive/); assert.match(offline.markup, /“Sound Drive”/); assert.match(offline.markup, /browser-offline-badge/);
  const remount = await fresh(), before = remount.calls.filter(call => call.command === 'library.kits.browse').length;
  remount.mounted = false; remount.browser.event('library.results', { volumes: true }); await remount.run();
  assert.equal(remount.calls.filter(call => call.command === 'library.kits.browse').length, before);
  remount.mounted = true; remount.render(); await remount.run(); assert.equal(remount.calls.filter(call => call.command === 'library.kits.browse').length, before + 1);
  const reading = await fresh(), count = reading.calls.filter(call => call.command === 'library.kits.browse').length;
  reading.browser.event('library.kits.progress', { reading: true, read: 10, pending: 40, revision: 7 }); await reading.run();
  assert.equal(reading.calls.filter(call => call.command === 'library.kits.browse').length, count + 1, 'newly read group headers refresh the results');
  reading.browser.event('library.kits.progress', { reading: true, read: 20, pending: 40, revision: 1 }); await reading.run();
  assert.equal(reading.calls.filter(call => call.command === 'library.kits.browse').length, count + 1, 'unchanged revisions only update the status line');
  console.log('PASS offline kits remain browsable; library changes and header reads refresh results only when mounted');

  const page = await fresh(), reply = deferred();
  page.browser._local.total = 140; page.requestHandler = (command, payload) => command === 'library.kits.browse' && payload.offset ? reply.promise : undefined;
  page.browser.controller.move(1); page.browser.controller.move(1); page.browser.controller.move(1);
  assert.equal(page.calls.filter(call => call.command === 'library.kits.browse' && call.payload.offset).length, 1, 'reaching the end appends one page');
  assert.equal(page.last('library.kits.browse').payload.offset, 2);
  page.browser.controller.choose('tags', 'Kits\u001fHip Hop'); reply.resolve({ items: [{ path: '/late-page.mxgrp', name: 'Late page' }], total: 140 }); await tick();
  assert.equal(page.browser.controller.list().items.length, 0);
  page.requestHandler = null; await page.run(); assert.equal(page.browser.controller.list().items[0].name, 'Cloud');
  console.log('PASS appended pages coalesce repeats and reject a page belonging to old filters');

  // A background refresh after browsing past the first page reloads every page shown: the selection must not jump back.
  const long = await fresh(), rows = Array.from({ length: 450 }, (_, index) => ({ path: `/long/${index}.mxgrp`, name: `Long ${index}` }));
  long.requestHandler = (command, payload) => command === 'library.kits.browse' ? Promise.resolve({ items: rows.slice(payload.offset, payload.offset + payload.limit), total: rows.length }) : undefined;
  await long.browser.search(); await long.run();
  long.browser.controller.move(199); await long.run(); long.browser.controller.move(50); await long.run();
  assert.equal(long.browser.controller.list().items.length, 400); assert.equal(long.browser.controller.list().selected, 249);
  long.browser.event('library.results', { scanning: false }); await long.run();
  assert.deepEqual([long.last('library.kits.browse').payload.offset, long.last('library.kits.browse').payload.limit], [0, 400], 'the refresh asks for the rows already shown');
  assert.equal(long.browser.controller.list().items.length, 400); assert.equal(long.browser.controller.list().selected, 249, 'the selected kit stays selected');
  long.browser.controller.choose('tags', 'Kits\u001fHip Hop'); await long.run();
  assert.equal(long.last('library.kits.browse').payload.limit, 200, 'a new filter starts again from one page');
  console.log('PASS a background refresh keeps appended pages and the selection');

  const failure = await fresh(); failure.requestHandler = command => command === 'library.kits.browse' ? Promise.reject(new Error('Drive read failed')) : undefined;
  await failure.browser.search(); const failures = failure.calls.length; failure.render(); await failure.run();
  assert.equal(failure.calls.length, failures, 'failed search does not spin on repaint'); assert.match(failure.markup, /Drive read failed/);
  assert.match(failure.markup, /id="kitb-retry"/);
  console.log('PASS failed kit searches stay actionable without a repaint retry loop');

  const unreadable = await fresh(); unreadable.browser._local.metadata.clear();
  unreadable.requestHandler = command => command === 'kit.detail' ? Promise.reject(new Error('Unsupported Maschine group archive version')) : undefined;
  unreadable.browser.controller.move(0); await unreadable.run();
  assert.match(unreadable.markup, /id="kitb-load" disabled/);
  assert.match(unreadable.markup, /id="kitb-load-open" disabled/);
  assert.match(unreadable.markup, /Unsupported Maschine group archive version/);
  assert.match(unreadable.markup, /id="kitb-detail-retry"/);
  assert.doesNotMatch(unreadable.markup, /id="kitb-preview" disabled/);
  await unreadable.browser.controller.load();
  unreadable.key('kitb-results', 'Enter'); unreadable.key('kitb-results', 'Enter', { shiftKey: true }); await tick();
  const unreadableMenu = unreadable.browser.contextItems({ target: { closest: selector => selector === '[data-kitb-row]' ? { dataset: { kitbRow: '0' } } : null } });
  for (const action of unreadableMenu.filter(action => ['Load new kit', 'Add and edit sounds'].includes(action.title))) { assert.equal(action.enabled, false); await action.action(); }
  assert.equal(unreadable.loads.length, 0, 'known parse errors block buttons, keyboard, controller and stale context actions');
  await unreadable.browser.controller.preview(); assert(unreadable.last('sample.preview'), 'associated audio can still be auditioned');
  unreadable.requestHandler = command => command === 'kit.detail' ? { sounds: [], compatibility: { loadable: true } } : undefined;
  await unreadable.node('kitb-detail-retry').handlers.click(); await tick();
  assert.doesNotMatch(unreadable.markup, /id="kitb-load" disabled/);
  await unreadable.browser.controller.load(); assert.equal(unreadable.loads.length, 1, 'successful retry restores import');
  const lateError = await fresh(), stopForError = deferred();
  await lateError.browser.controller.preview();
  lateError.requestHandler = command => command === 'sample.stopPreview' ? stopForError.promise : undefined;
  const awaitingImport = lateError.browser.controller.load();
  lateError.browser._local.metadata.set(lateError.items[0].path, { error: 'Unsupported archive' });
  stopForError.resolve({}); await awaitingImport;
  assert.equal(lateError.loads.length, 0, 'a detail error resolved during preview cancellation is checked before import');
  console.log('PASS unreadable group details block import without hiding preview or retry, including delayed errors');

  const replacement = await fresh();
  assert.match(replacement.markup, /id="kitb-replace" disabled/);
  await replacement.browser.controller.load('replace'); assert.equal(replacement.loads.length, 0);
  // Two groups (kits): tracks 0 and 1 are 'existing', track 2 is 'second'; the cursor picks the target.
  Object.assign(replacement.state.project, { groups: [{ id: 'existing', name: 'Current drums', busID: 'b1' }, { id: 'second', name: 'Other drums', busID: 'b2' }],
    tracks: [{ groupID: 'existing', instrument: 0 }, { groupID: 'existing', instrument: 0 }, { groupID: 'second', instrument: 0 }] });
  replacement.state.track = 0; replacement.render();
  const target = () => replacement.kits.cursorGroup();
  assert.doesNotMatch(replacement.markup, /id="kitb-replace" disabled/);
  assert.match(replacement.markup, /Replace Current drums pad by pad, keeping its tracks, patterns and mixer routing/);
  replacement.node('kitb-replacement-target').handlers.change({ target: { value: 'second' } });
  assert.equal(target().id, 'second', 'replacement target can be chosen without leaving the browser');
  replacement.node('kitb-replacement-target').handlers.change({ target: { value: 'existing' } });
  assert.equal(target().id, 'existing');
  await replacement.node('kitb-replace').handlers.click();
  assert.deepEqual(JSON.parse(JSON.stringify(replacement.loads.at(-1).options)), { stay: true, replace: true });
  await replacement.node('kitb-load').handlers.click();
  assert.deepEqual(JSON.parse(JSON.stringify(replacement.loads.at(-1).options)), { stay: true });
  const stopReplace = deferred(); await replacement.browser.controller.preview();
  replacement.requestHandler = command => command === 'sample.stopPreview' ? stopReplace.promise : undefined;
  const replacing = replacement.browser.controller.load('replace'), loaded = replacement.loads.length;
  replacement.state.track = 2; stopReplace.resolve({}); await replacing;
  assert.equal(replacement.loads.length, loaded, 'changing target while preview stops cannot replace another kit');
  console.log('PASS explicit new/replacement actions preserve intent and reject changed replacement targets');

  const saved = await fresh(); saved.browser.restoreSettings({ query: 'Cloud', product: 'Deep', characters: ['Warm', 'Warm', 123], autoPreview: false }); await saved.run();
  assert.equal(saved.browser.settings().product, 'Deep', 'the saved expansion is the chosen one');
  assert.equal(JSON.stringify(saved.browser.settings().characters), '["Warm"]');
  assert.equal(saved.last('library.kits.browse').payload.query, 'Cloud'); assert.equal(saved.browser.controller.list().autoPreview, false);
  saved.browser.restoreSettings({ user: true, product: 'Saved kits', bank: 'x'.repeat(5000), subtypes: ['A', 'A', 7] }); await saved.run();
  assert.equal(saved.browser.settings().bank, null, 'overlong values are rejected'); assert.equal(JSON.stringify(saved.browser.settings().subtypes), '["A"]');
  assert.equal(saved.last('library.kits.browse').payload.user, true);
  saved.browser.restoreSettings({ user: true, query: 'Acceptance Drums' }); await saved.run();
  assert.equal(saved.last('library.kits.browse').payload.product, undefined, 'settings without an expansion choose every expansion');
  saved.node('kitb-new').handlers.click(); assert(saved.calls.some(call => call.command === 'create'));
  saved.node('kitb-add').handlers.click(); await tick(); assert(saved.calls.some(call => call.command === 'library.chooseRoot'));
  console.log('PASS restored kit settings are bounded and de-duplicated; new kit and add folder use actual host actions');
})().catch(error => { console.error(error); process.exitCode = 1; });
