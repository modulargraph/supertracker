'use strict';
// Maschine-style NKS browser: product/type/character filters, keyboard traversal, preview and loading.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('nks-browser.js');
const SEP = '\u001f';
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const tick = async () => { for (let n = 0; n < 20; n++) await Promise.resolve(); };
function harness() {
  const timers = new Map(), calls = [], loads = [], notices = []; let serial = 0, markup = '', mounted = true, requestHandler = null;
  const nodes = new Map();
  const node = id => { if (!nodes.has(id)) nodes.set(id, { id, dataset: {}, classList: { toggle() {} }, setAttribute() {}, focus() { document.activeElement = this; }, select() {}, scrollIntoView() {}, addEventListener() {}, querySelectorAll: () => [], closest: () => null, innerHTML: '' }); return nodes.get(id); };
  const document = { activeElement: null, querySelector: selector => selector === '.nksb-layout' && mounted ? {} : null, querySelectorAll: () => [],
    getElementById: id => markup.includes(`id="${id}"`) ? node(id) : null };
  const items = [
    { path: '/a.nksf', name: 'Alpha Pad', product: 'Synth One', bank: 'A', types: ['Synth Pad'], subtypes: ['Synth Pad' + SEP + 'Warm'], characters: ['Warm'], previewPath: '/a.ogg', hasControls: true, effect: false },
    { path: '/b.nksf', name: 'Beta Bass', product: 'Synth One', bank: 'B', types: ['Bass'], subtypes: [], characters: [], effect: false }];
  const effect = { path: '/e.nksfx', name: 'Tape Echo', product: 'Echo FX', bank: '', types: ['Delay'], subtypes: [], characters: [], previewPath: '/e.ogg', effect: true };
  const state = { project: { tracks: [{ name: 'Bass', routingID: 'bass' }], instruments: [{ name: 'Synth' }], audioPlugins: [{ owner: 'track:bass', kind: 'effect', slot: 0, name: 'Comp' }] }, track: 0, instrument: 0 };
  const ST = { state, escapeHTML, toast: text => notices.push(text), refresh() { markup = browser.render(); },
    async request(command, payload) {
      calls.push({ command, payload });
      if (requestHandler) { const answer = requestHandler(command, payload); if (answer !== undefined) return answer; }
      if (command === 'library.nks.browse') {
        const list = payload.kind === 'effect' ? [effect] : items.filter(item => (payload.product === undefined || item.product === payload.product) && (!payload.types.length || item.types.some(type => payload.types.includes(type))));
        return { items: list, total: list.length, products: [{ name: payload.kind === 'effect' ? 'Echo FX' : 'Synth One', vendor: 'Fixture', count: 2 }, { name: 'Zeta', vendor: 'Fixture', count: 1 }],
          banks: payload.product ? [{ name: 'A', count: 1 }, { name: 'B', count: 1 }] : [], types: [{ name: 'Bass', count: 1 }, { name: 'Synth Pad', count: 1 }],
          subtypes: [{ name: 'Synth Pad' + SEP + 'Warm', label: 'Warm', type: 'Synth Pad', count: 1 }], characters: [{ name: 'Warm', count: 1 }], counts: { instrument: 2, effect: 1 }, status: { indexing: false, count: 3, revision: 4 } };
      }
      if (command === 'sample.preview') return { duration: 2 };
      if (command === 'library.nks') return { controllerPages: [{ name: 'Main', parameters: [{ slot: 0, name: 'Cutoff' }] }] };
      return {};
    } };
  const window = { ST, STNKS: { async load(path, options) { loads.push({ path, options }); return true; } } };
  const context = vm.createContext({ window, document, Map, Set, Promise, queueMicrotask: fn => Promise.resolve().then(fn),
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'nks-browser.js', { text: source });
  const browser = context.window.STNKSBrowser;
  async function run() { for (let round = 0; round < 4; round++) { for (const [id, timer] of [...timers]) if (timer.delay < 300 && timers.delete(id)) timer.fn(); await tick(); } }
  function key(id, keyName, options = {}) {
    const target = node(id); target.tagName = id === 'nksb-search' ? 'INPUT' : 'DIV'; target.closest = selector => selector === '#nksb-results' && id === 'nksb-results' ? target : null;
    document.activeElement = target;
    const event = { target, key: keyName, code: keyName === ' ' ? 'Space' : keyName, preventDefault() { this.prevented = true; }, ...options };
    return { handled: browser.key(event), prevented: !!event.prevented };
  }
  return { browser, calls, loads, notices, key, run, state, set requestHandler(fn) { requestHandler = fn; }, get markup() { return markup; }, set mounted(value) { mounted = value; }, last: () => calls.filter(call => call.command === 'library.nks.browse').at(-1).payload };
}
(async () => {
  const h = harness();
  h.markup; h.browser.bind(); await h.run(); h.state; // initial search
  assert.equal(h.last().kind, 'instrument');
  assert.match(h.markup, /id="nksb-product-1"[^>]*>.*Synth One/);
  assert.match(h.markup, /id="nksb-row-0"[^>]*aria-selected="true"/);
  assert.match(h.markup, /data-nksb-chip="types"/);
  assert.match(h.markup, /Load and keep browsing/);
  console.log('PASS renders products, presets, type chips and load actions');

  let result = h.key('nksb-products', 'ArrowDown'); await h.run();
  assert(result.handled && result.prevented); assert.equal(h.last().product, 'Synth One', 'arrowing a product applies it immediately');
  assert.match(h.markup, /data-nksb-group="banks"/, 'banks appear for a chosen product');
  result = h.key('nksb-products', 'z'); await h.run(); assert.equal(h.last().product, 'Zeta', 'type-ahead jumps to a product');
  result = h.key('nksb-products', 'Escape'); await h.run(); assert.equal(h.last().product, undefined, 'Escape returns to all products');
  console.log('PASS product list arrows, type-ahead and reset');

  h.key('nksb-products', 'ArrowRight');
  result = h.key('nksb-results', 'ArrowDown'); assert(result.handled); await h.run();
  assert.match(h.markup, /nksb-row-1/);
  result = h.key('nksb-results', 'ArrowUp'); await h.run();
  assert(h.calls.some(call => call.command === 'sample.preview' && call.payload.path === '/a.ogg'), 'arrowing auto-previews');
  result = h.key('nksb-results', ' '); await h.run(); assert(result.prevented);
  assert(h.calls.some(call => call.command === 'sample.stopPreview'), 'Space toggles the playing preview off');
  console.log('PASS result arrows, automatic preview and Space toggle');

  h.key('nksb-results', 'Enter'); await h.run();
  h.key('nksb-results', 'Enter', { shiftKey: true }); await h.run();
  h.key('nksb-results', 'Enter', { altKey: true }); await h.run();
  assert.deepEqual(h.loads.map(load => [load.options.stay, load.options.newInstrument]), [[true, false], [false, false], [true, true]]);
  console.log('PASS Return keeps browsing, Shift Return opens controls, Option Return adds an instrument');

  h.browser.show('effect'); await h.run();
  assert.equal(h.last().kind, 'effect'); assert.match(h.markup, /Load into FX 2/, 'the first empty insert slot is proposed');
  h.key('nksb-results', 'Enter'); await h.run();
  assert.equal(JSON.stringify(h.loads.at(-1).options), JSON.stringify({ kind: 'effect', slot: 1 }));
  console.log('PASS effects load into an insert slot');

  h.browser.show('instrument'); await h.run();
  const items = h.browser.contextItems({ target: { closest: selector => selector === '[data-nksb-row]' ? { dataset: { nksbRow: '0' } } : null } });
  assert(items.some(item => item.title === 'Load and edit controls')); assert(items.some(item => item.title === 'Show all Synth One presets'));
  console.log('PASS context menu offers load, preview and product actions');

  h.browser.event('library.nks.progress', { indexing: true, done: 10, total: 20, revision: 9 }); await h.run();
  assert.equal(h.browser._local.status.done, 10);
  console.log('PASS indexing progress updates status and refreshes results');

  const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
  const fresh = async () => { const value = harness(); await value.browser.search(); return value; };
  const race = await fresh(), old = deferred();
  race.requestHandler = command => command === 'library.nks.browse' ? old.promise : undefined;
  const staleSearch = race.browser.search();
  race.browser.controller.cycle('types', 1); // debounce has not fired yet
  await race.browser.controller.load(); await race.browser.controller.preview();
  assert.equal(race.loads.length, 0, 'a pending filter cannot load the old selection');
  assert.equal(race.calls.filter(call => call.command === 'sample.preview').length, 0, 'a pending filter cannot preview the old selection');
  old.resolve({ items: [{ path: '/stale.nksf', name: 'Stale' }], total: 1 }); await staleSearch;
  assert.equal(race.browser.controller.list().items.length, 0, 'old in-flight response stays invalid before the debounce fires');
  race.requestHandler = null; await race.run();
  assert.equal(race.browser.controller.list().items[0].name, 'Beta Bass');
  console.log('PASS filter debounce immediately rejects old results, loads and previews');

  const previewRace = await fresh(), audio = deferred();
  previewRace.requestHandler = command => command === 'sample.preview' ? audio.promise : undefined;
  const pendingPreview = previewRace.browser.controller.preview();
  previewRace.browser.controller.move(1); await tick();
  assert(previewRace.calls.some(call => call.command === 'sample.stopPreview'), 'selection without a preview cancels the pending previous one');
  audio.resolve({ duration: 2 }); await pendingPreview;
  assert.equal(previewRace.browser._local.previewPath, '');
  previewRace.browser.controller.move(-1); await previewRace.run();
  previewRace.browser.controller.toggleAutoPreview(); await tick();
  assert.equal(previewRace.browser.previewActive, false, 'hardware auto-preview off cancels audition');
  console.log('PASS pending preview cancellation and controller auto-preview off');

  const menu = await fresh();
  const choices = menu.browser.contextItems({ target: { closest: selector => selector === '[data-nksb-row]' ? { dataset: { nksbRow: '0' } } : null } });
  menu.browser.controller.move(1);
  await choices.find(choice => choice.title === 'Load and keep browsing').action();
  assert.equal(menu.loads.length, 0, 'a native context menu cannot silently retarget to a different selection');
  const destination = await fresh(), stop = deferred();
  await destination.browser.controller.preview();
  destination.requestHandler = command => command === 'sample.stopPreview' ? stop.promise : undefined;
  const loading = destination.browser.controller.load(); destination.state.track = 1;
  stop.resolve({}); await loading;
  assert.equal(destination.loads.length, 0, 'destination changes while preview stops abort loading');
  console.log('PASS delayed context actions and load destination are guarded');

  const paging = await fresh(), page = deferred();
  paging.browser._local.total = 4;
  paging.requestHandler = (command, payload) => command === 'library.nks.browse' && payload.offset > 0 ? page.promise : undefined;
  paging.browser.controller.move(1); paging.browser.controller.move(1); paging.browser.controller.move(1);
  assert.equal(paging.calls.filter(call => call.command === 'library.nks.browse' && call.payload.offset > 0).length, 1, 'repeated arrows issue only one append');
  page.resolve({ items: [{ path: '/c.nksf', name: 'New revision' }], total: 4, status: { revision: 5 } }); await tick();
  assert.equal(paging.calls.filter(call => call.command === 'library.nks.browse').at(-1).payload.offset, 0, 'changed catalogue revision restarts paging');
  assert.equal(paging.browser.controller.list().items.some(item => item.name === 'New revision'), false);
  console.log('PASS append coalescing and catalogue revision paging guard');

  // A background refresh after browsing past the first page reloads every page shown: the selection must not jump back.
  const long = await fresh(), rows = Array.from({ length: 450 }, (_, index) => ({ path: `/long/${index}.nksf`, name: `Long ${index}`, product: 'Long' }));
  long.requestHandler = (command, payload) => command === 'library.nks.browse' ? Promise.resolve({ items: rows.slice(payload.offset, payload.offset + payload.limit), total: rows.length, status: { revision: 1 } }) : undefined;
  await long.browser.search(); await tick();
  long.browser.controller.move(199); await tick(); long.browser.controller.move(50); await tick();
  assert.equal(long.browser.controller.list().items.length, 400); assert.equal(long.browser.controller.list().selected, 249);
  await long.browser.search(); await tick();
  const refresh = long.calls.filter(call => call.command === 'library.nks.browse').at(-1).payload;
  assert.deepEqual([refresh.offset, refresh.limit], [0, 400], 'the refresh asks for the rows already shown');
  assert.equal(long.browser.controller.list().items.length, 400); assert.equal(long.browser.controller.list().selected, 249, 'the selected preset stays selected');
  console.log('PASS a background refresh keeps appended pages and the selection');

  const offline = await fresh();
  offline.browser._local.items[0].available = false;
  await offline.browser.controller.preview(); await offline.browser.controller.load();
  assert.equal(offline.loads.length, 0); assert.equal(offline.calls.some(call => call.command === 'sample.preview'), false);
  assert.match(offline.browser.render(), /unavailable/);
  const audition = await fresh();
  await audition.browser.controller.preview();
  audition.browser._local.items[1].available = false; audition.browser._local.items[1].previewPath = '/offline.ogg';
  audition.browser.controller.move(1); await audition.run();
  assert.equal(audition.browser.previewActive, false, 'moving to an offline preset stops the previous audition');
  assert.equal(audition.calls.some(call => call.command === 'sample.preview' && call.payload.path === '/offline.ogg'), false);
  console.log('PASS offline presets remain browsable without preview or load');
  const reconnect = await fresh(); reconnect.mounted = false;
  reconnect.browser.event('library.results', { volumes: true });
  assert.equal(reconnect.calls.filter(call => call.command === 'library.nks.browse').length, 1);
  reconnect.mounted = true; reconnect.browser.bind(); await reconnect.run();
  assert.equal(reconnect.calls.filter(call => call.command === 'library.nks.browse').length, 2, 'returning to NKS refreshes mount changes observed while another tab was open');
  console.log('PASS inactive NKS tab refreshes after a drive change');

  // Artwork, vendor filter and Product › Bank › Sub-bank drill-down.
  const drill = await fresh();
  drill.requestHandler = (command, payload) => {
    if (command !== 'library.nks.browse') return undefined;
    const item = { path: '/x.nksf', name: 'Exp Lead', product: 'Synth One', vendor: 'NI', bank: 'Beam', subBank: 'Leads', types: [], subtypes: [], characters: [], previewPath: '/x.ogg', art: 'image/beam', productArt: 'image/synth one', color: '#e17600' };
    return { items: [item], total: 1, products: [{ name: 'Synth One', vendor: 'NI', count: 1, art: 'image/synth one', color: '#e17600' }], vendors: [{ name: 'NI', count: 1 }, { name: 'Acme', count: 2 }],
      banks: payload.product ? [{ name: 'Beam', count: 1, art: 'image/beam' }, { name: 'Factory', count: 3 }] : [], subBanks: payload.bank ? [{ name: 'Leads', count: 1 }] : [],
      types: [], subtypes: [], characters: [], counts: { instrument: 1, effect: 0 }, status: { revision: 9 } };
  };
  drill.browser.search(); await drill.run();
  assert.match(drill.markup, /id="nksb-vendor"/, 'vendor filter shown when several vendors exist');
  assert.match(drill.markup, /data-nksb-art="image\/synth one"/, 'product rows request their artwork');
  drill.browser._local.art.tile.set('image/synth one', 'data:image/jpeg;base64,AAAA');
  drill.browser._local.art.tile.set('image/evil', 'javascript:alert(1)');
  drill.browser.controller.choose('product', 'Synth One'); await drill.run();
  assert.match(drill.markup, /background-image:url\('data:image\/jpeg;base64,AAAA'\)/, 'cached artwork is painted inline');
  assert.match(drill.markup, /class="nksb-hero"/, 'a chosen product shows its banner header');
  assert.match(drill.markup, /data-nksb-group="banks"[^]*nksb-tile/, 'banks with artwork render as expansion tiles');
  assert.match(drill.markup, /data-nksb-variant="banner"/, 'detail and hero use the banner variant');
  drill.browser.controller.choose('bank', 'Beam'); await drill.run();
  assert.equal(drill.last().bank, 'Beam');
  assert.match(drill.markup, /data-nksb-group="subBanks"/, 'sub-banks appear under a chosen bank');
  assert.match(drill.markup, /data-nksb-crumb="product"/, 'breadcrumb steps back to the product');
  drill.browser._local.filters.instrument.subBank = 'Leads';
  drill.browser.search(); await drill.run();
  assert.equal(drill.last().subBank, 'Leads');
  let up = drill.key('nksb-results', 'Backspace'); await drill.run();
  assert(up.handled && up.prevented); assert.equal(drill.last().subBank, undefined); assert.equal(drill.last().bank, 'Beam', 'Backspace leaves the sub-bank first');
  drill.key('nksb-results', 'Backspace'); await drill.run();
  assert.equal(drill.last().bank, undefined); assert.equal(drill.last().product, 'Synth One', 'then the bank');
  drill.key('nksb-results', 'Backspace'); await drill.run();
  assert.equal(drill.last().product, undefined, 'then the product');
  drill.browser._local.filters.instrument.vendor = 'Acme';
  drill.browser.search(); await drill.run();
  assert.equal(drill.last().vendor, 'Acme', 'vendor filter is sent to the catalogue');
  assert.match(drill.markup, /data-nksb-play="0"/, 'rows have a clickable preview button');
  assert.doesNotMatch(drill.markup, /javascript:/, 'only data:image artwork is painted');
  console.log('PASS artwork, vendor filter, expansion tiles and Product › Bank › Sub-bank drill-down');

})().catch(error => { console.error(error); process.exit(1); });
