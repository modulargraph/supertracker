'use strict';
// Background preview warming: neighbour order, skipped items, debounce/dedupe,
// cancellation, and the NKS browser asking for its neighbours' .ogg previews.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const web = require('./helpers/web');
const tick = async () => { for (let n = 0; n < 20; n++) await Promise.resolve(); };

function context(extra = {}) {
  const timers = new Map(), calls = []; let serial = 0;
  const ST = { async request(command, payload) { calls.push({ command, payload }); return {}; } };
  const window = { ST, ...extra.window };
  const ctx = vm.createContext({ window, document: extra.document || { querySelector: () => null }, Map, Set, Promise, JSON, queueMicrotask: fn => Promise.resolve().then(fn),
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(ctx, 'preview-prefetch.js');
  async function flush(max = 300) { for (let round = 0; round < 4; round++) { for (const [id, timer] of [...timers]) if (timer.delay < max && timers.delete(id)) timer.fn(); await tick(); } }
  return { ctx, window, P: window.STPreviewPrefetch, calls, timers, flush };
}

(async () => {
  const { P, calls, flush, timers } = context();
  const order = indices => JSON.parse(JSON.stringify(indices));
  assert.deepEqual(order(P.order(5, 20, 1, 3)), [5, 6, 4, 7, 3, 8, 2]);
  assert.deepEqual(order(P.order(5, 20, -1, 3)), [5, 4, 6, 3, 7, 2, 8], 'moving up warms the items above first');
  assert.deepEqual(order(P.order(0, 3, 1, 3)), [0, 1, 2]);
  assert.deepEqual(order(P.order(9, 3, 1, 3)), []);
  console.log('PASS neighbour order is nearest first, biased toward travel');

  const items = [
    { path: '/s/a.wav' }, { path: '/s/b.mid' }, { path: '/off/c.wav', available: false }, { path: '/s/d.aif' },
    { path: '/s/e.wav' }, { path: '/s/a.wav' }, { path: '/s/f.flac' }, { path: '/s/g.ogg' }];
  const entry = item => ({ path: item.path });
  const plan = order(P.plan({ items, selected: 3, direction: 1, entry }));
  assert.deepEqual(plan, [{ path: '/s/d.aif' }, { path: '/s/e.wav' }, { path: '/s/a.wav' }, { path: '/s/f.flac' }]);
  assert(!plan.some(request => request.path.endsWith('.mid')), 'non-audio files are not decoded');
  assert(!plan.some(request => request.path.startsWith('/off/')), 'offline items are never requested');
  const kits = order(P.plan({ items: [{ path: '/k/A.mxgrp' }, { path: '/k/B.stkit', previewPath: '/k/B.stkit/Preview.ogg' }], selected: 0, entry: item => item.previewPath ? { path: item.previewPath } : { kit: item.path } }));
  assert.deepEqual(kits, [{ kit: '/k/A.mxgrp' }, { path: '/k/B.stkit/Preview.ogg' }], 'kits without a known preview are resolved natively');
  console.log('PASS plan skips offline, non-audio and duplicate items; kits fall back to native resolution');

  // Holding an arrow key: only the settled selection is sent, once.
  const list = Array.from({ length: 40 }, (_, index) => ({ path: `/s/${index}.wav` }));
  for (let selected = 0; selected < 12; selected++) P.schedule('samples', { items: list, selected, entry });
  assert.equal(calls.length, 0, 'nothing is sent while the selection is still moving');
  assert.equal(timers.size, 1, 'one pending debounce');
  await flush();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'sample.prefetchPreviews');
  assert.deepEqual(order(calls[0].payload.items.map(item => item.path)), ['/s/11.wav', '/s/12.wav', '/s/10.wav', '/s/13.wav', '/s/9.wav', '/s/14.wav', '/s/8.wav']);
  P.schedule('samples', { items: list, selected: 11, entry }); await flush();
  assert.equal(calls.length, 1, 'an unchanged neighbourhood is not re-sent');
  P.schedule('samples', { items: list, selected: 10, entry }); await flush();
  assert.equal(calls.length, 2);
  assert.deepEqual(order(calls[1].payload.items.slice(0, 3).map(item => item.path)), ['/s/10.wav', '/s/9.wav', '/s/11.wav'], 'moving up prefers the items above');
  P.cancel(); await flush();
  assert.equal(calls.length, 3); assert.deepEqual(order(calls[2].payload.items), [], 'cancel abandons the native queue');
  P.cancel(); await flush();
  assert.equal(calls.length, 3, 'a second cancel sends nothing');
  console.log('PASS debounce, dedupe, direction and cancellation');

  // Request failures (e.g. during export) are swallowed.
  const failing = context();
  failing.window.ST.request = async () => { throw new Error('Wait for export to finish'); };
  failing.P.schedule('nks', { items: list, selected: 0, entry }); await failing.flush();
  console.log('PASS request failures stay silent');

  // NKS browser integration: arrowing warms the neighbours' .ogg previews.
  const SEP = '\u001f';
  const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  let markup = '';
  const nodes = new Map();
  const node = id => { if (!nodes.has(id)) nodes.set(id, { id, dataset: {}, classList: { toggle() {} }, setAttribute() {}, focus() {}, select() {}, scrollIntoView() {}, addEventListener() {}, querySelectorAll: () => [], closest: () => null, innerHTML: '' }); return nodes.get(id); };
  const document = { activeElement: null, querySelector: selector => selector === '.nksb-layout' ? {} : null, querySelectorAll: () => [], getElementById: id => markup.includes(`id="${id}"`) ? node(id) : null };
  const presets = Array.from({ length: 10 }, (_, index) => ({ path: `/p/${index}.nksf`, name: `Preset ${index}`, product: 'Synth', bank: 'A', types: ['Pad'], subtypes: [], characters: [], previewPath: index === 4 ? undefined : `/p/.previews/${index}.nksf.ogg`, available: index === 3 ? false : undefined, effect: false }));
  const h = context({ document });
  const state = { project: { tracks: [{ name: 'T' }], instruments: [{ name: 'I' }], audioPlugins: [] }, track: 0, instrument: 0 };
  Object.assign(h.window.ST, { state, escapeHTML, toast() {}, refresh() { markup = h.window.STNKSBrowser.render(); },
    async request(command, payload) {
      h.calls.push({ command, payload });
      if (command === 'library.nks.browse') return { items: presets, total: presets.length, products: [{ name: 'Synth', vendor: 'V', count: 10 }], banks: [], types: [], subtypes: [], characters: [], counts: { instrument: 10, effect: 0 }, status: { indexing: false, count: 10, revision: 1 } };
      if (command === 'sample.preview') return { duration: 2 };
      return {};
    } });
  h.window.STNKS = { async load() { return true; } };
  web.load(h.ctx, 'nks-browser.js');
  const browser = h.window.STNKSBrowser;
  markup = browser.render(); browser.bind(); await h.flush();
  const prefetches = () => h.calls.filter(call => call.command === 'sample.prefetchPreviews');
  assert(prefetches().length >= 1, 'results arriving warm the first previews');
  assert.equal(prefetches().at(-1).payload.items[0].path, '/p/.previews/0.nksf.ogg');
  const results = node('nksb-results'); results.closest = selector => selector === '#nksb-results' ? results : null; results.tagName = 'DIV';
  for (let n = 0; n < 5; n++) { document.activeElement = results; browser.key({ target: results, key: 'ArrowDown', code: 'ArrowDown', preventDefault() {} }); }
  await h.flush();
  const last = prefetches().at(-1).payload.items.map(item => item.path);
  assert.equal(last[0], '/p/.previews/5.nksf.ogg', 'the settled selection is warmed first');
  assert(last.includes('/p/.previews/6.nksf.ogg') && last.includes('/p/.previews/8.nksf.ogg'), 'items ahead are warmed');
  assert(!last.some(value => value.includes('/3.')), 'offline presets are skipped');
  assert(h.calls.some(call => call.command === 'sample.preview' && call.payload.path === '/p/.previews/5.nksf.ogg'), 'auto preview still plays the selection');
  console.log('PASS NKS browser warms neighbouring .ogg previews and skips offline presets');
})().catch(error => { console.error(error); process.exit(1); });
