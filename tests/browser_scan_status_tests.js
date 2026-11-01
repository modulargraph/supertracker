'use strict';
// Run with: node tests/browser_scan_status_tests.js
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const script = web.source('browser.js');
const rootPath = '/Volumes/Sound Drive/Expansion Samples';
const denied = `Cannot read library folder: ${rootPath} <permission denied>`;
const calls = [], handlers = {};
let searchResponse = { items: [], total: 0, scanning: false, errors: [] };
let rootResponse = [{ path: rootPath, name: 'Expansion Samples', available: true, scanStatus: 'error', scanErrors: [denied], indexedCount: 17 }];
const state = { view: 'browser', track: 0, instrument: 0, plugins: [], project: { instruments: [{ name: 'Instrument 1' }], tracks: [{}] }, library: { roots: rootResponse, items: [] } };
const nodes = new Map();
function node(id) { if (!nodes.has(id)) nodes.set(id, { id, addEventListener(type, callback) { handlers[`${id}:${type}`] = callback; }, focus() {}, select() {}, scrollTop: 0 }); return nodes.get(id); }
const rechoose = { dataset: { browserRechooseRoot: rootPath }, addEventListener(type, callback) { handlers[`root-rechoose:${type}`] = callback; } };
const document = { activeElement: null, getElementById: node, querySelector: () => ({}), querySelectorAll: selector => selector === '[data-browser-rechoose-root]' ? [rechoose] : [] };
const ST = {
  state,
  escapeHTML: value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
  refresh() {}, toast() {},
  async request(command, payload) {
    calls.push({ command, payload });
    if (command === 'library.search') return searchResponse;
    if (command === 'library.roots') return rootResponse;
    if (command === 'library.chooseRoot') return { scanning: true, count: 17, roots: [{ path: rootPath, available: true }] };
    return {};
  }
};
// Native scan notifications are tested synchronously; background timers do not need to execute.
let timer = 0;
const context = vm.createContext({ window: { ST }, document, Map, Set, Promise, setTimeout: () => ++timer, clearTimeout() {}, queueMicrotask() {} });
web.load(context, 'browser.js', { text: script });
const browser = context.window.STBrowser;

(async () => {
  let html = browser.render();
  assert(html.includes('Indexing error'), 'available=true must not hide a native read error');
  assert(html.includes('Choose folder again'), 'failed locations need a visible picker recovery action');
  assert(html.includes('&lt;permission denied&gt;'), 'native error text must be escaped');

  browser.event('library.results', { scanning: false, errors: [denied], rootReports: [{ path: rootPath, status: 'error', matched: 17, errors: [denied] }] });
  await browser.search();
  assert(browser.render().includes('Indexing error'), 'ordinary searches must preserve root errors');

  searchResponse = { cancelled: true, errors: [], rootReports: [{ path: rootPath, status: 'complete', matched: 17, errors: [] }] };
  await browser.search();
  assert(browser.render().includes('Indexing error'), 'canceled searches cannot clear persistent failures');

  browser.event('library.progress', { scanning: true, errors: [], rootReports: [{ path: rootPath, status: 'pending', matched: 0, errors: [] }] });
  assert(browser.render().includes('Indexing error'), 'retrying is not proof that the folder is readable');

  browser.bind();
  await handlers['root-rechoose:click']();
  assert(calls.some(call => call.command === 'library.chooseRoot' && call.payload.path === rootPath), 'recovery must open the native folder chooser');
  assert(browser.render().includes('Indexing error'), 'choosing a folder alone must not mark indexing successful');

  browser.event('library.results', { scanning: false, errors: [], rootReports: [{ path: rootPath, status: 'complete', matched: 42, errors: [] }] });
  html = browser.render();
  assert(!html.includes('Indexing error'), 'a clean completed scan clears the persisted failure');
  assert(!html.includes('library folder needs attention'));
  assert(html.includes('42 indexed'));

  browser.event('library.results', { scanning: false, errors: [], rootReports: [{ path: rootPath, status: 'empty', matched: 0, errors: [] }] });
  assert(browser.render().includes('No supported audio found'), 'a readable empty folder is distinct from read failure');

  state.library.roots = [{ path: '/unchecked', name: 'Unchecked folder', available: true }];
  assert(browser.render().includes('Not checked'), 'folder existence is not proof of indexing or readability');
  console.log('Browser scan status: 8 regression groups passed (persistent errors, canceled search, retry, picker action, successful/empty scans, unchecked roots, escaped messages).');
})().catch(error => { console.error(error); process.exitCode = 1; });
