'use strict';

// Run with: node tests/media_relink_ui_tests.js
// A small DOM adapter drives public UI events; native operations are deferred explicitly.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const installEngineSyncBoundary = require('./helpers/engine_sync_boundary');
const source = web.source('media-relink.js');
const copy = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const decode = value => String(value).replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&#39;/g, "'").replace(/&amp;/g, '&');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const fixtureProject = () => ({ name: 'Unsaved session', bpm: 123, tracks: [{ name: 'Track 1' }], instruments: [{ name: 'Kick', samplePath: '/old/kick.wav' }], patterns: [], audioClips: [] });
const reference = () => ({ id: '/instruments/0/samplePath', kind: 'sample', label: 'Instrument 1: Kick', path: '/old/kick.wav', status: 'missing', expectedSHA256: 'a'.repeat(64) });
const candidates = () => [
  { path: '/new/verified.wav', size: 1024, match: 'sha256', verified: true, contentMismatch: false, reasons: ['Same SHA-256'] },
  { path: '/new/name-only.wav', size: 2048, match: 'name', verified: false, contentMismatch: false, reasons: ['No stored fingerprint'] },
  { path: '/new/different.wav', size: 4096, match: 'name', verified: false, contentMismatch: true, reasons: ['Content differs'] }
];

class Node {
  constructor(document, tag = 'div', attrs = {}, group = '') {
    this.document = document; this.tagName = tag.toUpperCase(); this.attrs = attrs; this.group = group;
    this.id = attrs.id || ''; this.value = attrs.value || ''; this.checked = Object.hasOwn(attrs, 'checked'); this.disabled = Object.hasOwn(attrs, 'disabled');
    this.dataset = {}; this.handlers = {}; this.scrollTop = 0; this.selectionStart = null; this.textContent = ''; this.children = [];
    for (const [key, value] of Object.entries(attrs)) if (key.startsWith('data-')) this.dataset[key.slice(5).replace(/-([a-z])/g, (_, char) => char.toUpperCase())] = value;
    this.classList = { toggle() {} };
  }
  set innerHTML(value) { this.html = value; this.children = this.document.parse(value, this.group); }
  get innerHTML() { return this.html || ''; }
  addEventListener(type, callback) { this.handlers[type] = callback; }
  dispatch(type, extra = {}) { if (type === 'click' && this.disabled) return; return this.handlers[type]?.({ target: this, preventDefault() {}, ...extra }); }
  focus() { this.document.activeElement = this; }
  setSelectionRange(start) { this.selectionStart = start; }
  scrollIntoView() {}
  querySelector(selector) { if (selector === '.modal-body') return this.body; if (selector === '.modal-foot') return this.foot; return null; }
  appendChild(node) { if (node.className === 'modal-foot') { this.foot = node; node.group = 'footer'; } }
}

class Document {
  constructor() { this.groups = new Map(); this.activeElement = null; this.modal = null; }
  parse(html, group) {
    const nodes = [];
    for (const match of html.matchAll(/<([a-z][a-z0-9-]*)\b([^>]*)>/gi)) {
      const attrs = {};
      for (const item of match[2].matchAll(/([a-zA-Z_:][\w:.-]*)(?:="([^"]*)")?/g)) attrs[item[1]] = decode(item[2] || '');
      nodes.push(new Node(this, match[1], attrs, group));
    }
    this.groups.set(group, nodes); return nodes;
  }
  getElementById(id) { return [...this.groups.values()].flat().find(node => node.id === id) || null; }
  createElement(tag) { return new Node(this, tag); }
  querySelector(selector) { if (selector === '.media-relink-modal') return this.modal; return this.querySelectorAll(selector)[0] || null; }
  querySelectorAll(selector) {
    const match = selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/); if (!match) return [];
    return [...this.groups.values()].flat().filter(node => Object.hasOwn(node.attrs, match[1]) && (match[2] == null || node.attrs[match[1]] === match[2]));
  }
  open(body, footer) {
    this.groups.clear(); this.modal = new Node(this, 'section'); this.modal.body = new Node(this, 'div', {}, 'body'); this.modal.body.innerHTML = body;
    if (footer) { this.modal.foot = new Node(this, 'footer', {}, 'footer'); this.modal.foot.innerHTML = footer; }
  }
  get html() { return (this.modal?.body?.innerHTML || '') + (this.modal?.foot?.innerHTML || ''); }
}

function harness(overrides = {}) {
  const document = new Document();
  const state = { project: fixtureProject(), modal: null, track: 0, instrument: 0, row: 19, column: 3, view: 'pattern', samples: { 0: { path: '/old/kick.wav' } } };
  const calls = [], edits = [], toasts = [];
  const defaults = {
    'library.roots': () => [{ path: '/new', name: 'Samples', available: true }],
    'project.media.inspect': payload => payload.project.instruments[0].samplePath === '/old/kick.wav' ? { references: [reference()], missing: [reference()], presentCount: 0, warnings: [] } : { references: [], missing: [], presentCount: 1, warnings: [] },
    'project.media.search': () => ({ missing: [{ ...reference(), candidates: candidates() }], warnings: [] }),
    'project.media.apply': payload => { const project = copy(payload.project); project.instruments[0].samplePath = payload.selections[0].to; return { project, relinked: payload.selections, remaining: [], warnings: [] }; },
    'dialog.openSample': () => ({ path: '/new/manual.wav' }),
    'engine.project': () => ({ applied: true })
  };
  const ST = {
    state, escapeHTML, normalize: value => value,
    request(command, payload = {}) { calls.push({ command, payload: copy(payload) }); return Promise.resolve((overrides[command] || defaults[command] || (() => ({})))(payload)); },
    openModal(title, body, footer, options) { state.modal = options?.kind || 'dialog'; document.open(body, footer); },
    closeModal() { state.modal = null; document.modal = null; },
    mutate(label, callback) { edits.push({ label, before: copy(state.project) }); callback(state.project); },
    toast(message, kind) { toasts.push({ message, kind }); }, refresh() {}
  };
  const syncs = installEngineSyncBoundary(ST);
  const context = vm.createContext({ window: { ST }, document, console, Map, Set, Promise }); web.load(context, 'media-relink.js', { text: source });
  const ui = context.window.STMedia;
  const node = id => { const value = document.getElementById(id); assert(value, `Expected #${id} in the rendered dialog`); return value; };
  const choose = index => { const radio = node(`media-candidate-${index}`); radio.checked = true; radio.dispatch('change'); };
  return { ST, syncs, state, document, calls, edits, toasts, ui, node, choose, async search() { await node('media-search').dispatch('click'); } };
}

const tests = [];
const test = (name, run) => tests.push({ name, run });

test('module stays unmounted and inspection uses current unsaved project', async () => {
  const h = harness(); assert.equal(h.calls.length, 0); await h.ui.open();
  assert.equal(h.calls.find(call => call.command === 'project.media.inspect').payload.project.name, 'Unsaved session');
  assert(h.document.html.includes('/old/kick.wav')); assert.equal(h.edits.length, 0);
});

test('stale inspection cannot populate a different project dialog', async () => {
  const wait = deferred(); const h = harness({ 'project.media.inspect': () => wait.promise });
  const opening = h.ui.open(); h.state.project = fixtureProject(); h.state.project.name = 'Another project';
  wait.resolve({ missing: [{ ...reference(), label: 'STALE INSPECTION' }], presentCount: 0 }); await opening;
  assert(!h.document.html.includes('STALE INSPECTION')); assert.equal(h.edits.length, 0);
});

test('stale search cannot attach candidates after a project switch', async () => {
  const wait = deferred(); const h = harness({ 'project.media.search': () => wait.promise }); await h.ui.open();
  const search = h.search(); h.state.project = fixtureProject();
  wait.resolve({ missing: [{ ...reference(), candidates: [{ ...candidates()[0], path: '/STALE-CANDIDATE.wav' }] }] }); await search;
  assert(!h.document.html.includes('STALE-CANDIDATE')); assert.equal(h.edits.length, 0);
});

test('search never selects or applies candidates automatically', async () => {
  const h = harness(); await h.ui.open(); await h.search();
  assert.equal(h.node('media-relink-apply').disabled, true);
  assert.equal(h.document.querySelectorAll('[data-media-candidate]').filter(node => node.checked).length, 0);
  assert.equal(h.calls.filter(call => call.command === 'project.media.apply').length, 0);
});

test('verified selection preserves project identity, navigation, and undo', async () => {
  const h = harness(); const original = h.state.project; await h.ui.open(); await h.search(); h.choose(0);
  await h.node('media-relink-apply').dispatch('click');
  const applied = h.calls.find(call => call.command === 'project.media.apply').payload.selections[0];
  assert.equal(applied.from, '/old/kick.wav'); assert.equal(applied.to, '/new/verified.wav'); assert.equal('allowDifferentContent' in applied, false);
  assert.equal(h.state.project, original); assert.equal(original.instruments[0].samplePath, '/new/verified.wav');
  assert.equal(h.state.row, 19); assert.equal(h.state.column, 3); assert.equal(h.edits.length, 1);
  assert.equal(h.edits[0].before.instruments[0].samplePath, '/old/kick.wav'); assert.equal(Object.keys(h.state.samples).length, 0);
});

test('unverified filename choice is explicit and does not grant content override', async () => {
  const h = harness(); await h.ui.open(); await h.search(); h.choose(1);
  assert.equal(h.node('media-relink-apply').disabled, false); await h.node('media-relink-apply').dispatch('click');
  const selection = h.calls.find(call => call.command === 'project.media.apply').payload.selections[0];
  assert.equal(selection.to, '/new/name-only.wav'); assert.equal('allowDifferentContent' in selection, false);
});

test('known content mismatch requires the mapping-specific checkbox', async () => {
  const h = harness(); await h.ui.open(); await h.search(); h.choose(2);
  assert.equal(h.node('media-relink-apply').disabled, true); assert.equal(h.node('media-allow-content').checked, false);
  const checkbox = h.node('media-allow-content'); checkbox.checked = true; checkbox.dispatch('change');
  assert.equal(h.node('media-relink-apply').disabled, false); await h.node('media-relink-apply').dispatch('click');
  assert.equal(h.calls.find(call => call.command === 'project.media.apply').payload.selections[0].allowDifferentContent, true);
});

test('manual file is verified on apply and exposes override only after a mismatch', async () => {
  let attempts = 0;
  const h = harness({ 'project.media.apply': payload => {
    attempts++; if (!payload.selections[0].allowDifferentContent) throw new Error(`The selected file for ${reference().label} does not match its saved content fingerprint. Select the original file, or explicitly allow different content to replace the sound.`);
    const project = copy(payload.project); project.instruments[0].samplePath = payload.selections[0].to; return { project, relinked: payload.selections, remaining: [] };
  } });
  await h.ui.open(); await h.node('media-browse').dispatch('click');
  assert.equal(h.document.getElementById('media-allow-content'), null); await h.node('media-relink-apply').dispatch('click');
  assert.equal(h.edits.length, 0); assert.equal(h.node('media-allow-content').checked, false);
  const checkbox = h.node('media-allow-content'); checkbox.checked = true; checkbox.dispatch('change'); await h.node('media-relink-apply').dispatch('click');
  assert.equal(attempts, 2); assert.equal(h.state.project.instruments[0].samplePath, '/new/manual.wav');
});

test('stale apply recovers the newly selected project instead of mutating it', async () => {
  const wait = deferred(); const h = harness({ 'project.media.apply': () => wait.promise }); await h.ui.open(); await h.search(); h.choose(0);
  const applying = h.node('media-relink-apply').dispatch('click'); const replacement = fixtureProject(); replacement.name = 'New destination'; h.state.project = replacement;
  const stale = fixtureProject(); stale.instruments[0].samplePath = '/new/verified.wav'; wait.resolve({ project: stale, relinked: [] }); await applying;
  assert.equal(h.edits.length, 0); assert.equal(replacement.instruments[0].samplePath, '/old/kick.wav');
  assert.equal(h.calls.filter(call => call.command === 'engine.project').at(-1).payload.project.name, 'New destination');
});

test('stale relink restoration builds after shared sync waits and surfaces strict failures', async () => {
  for (const result of ['latest', 'replaced-again', 'failed']) {
    const apply = deferred(), sync = deferred(), entered = deferred();
    const h = harness({ 'project.media.apply': () => apply.promise, 'engine.project': () => { if (result === 'failed') throw new Error('Relink restoration failed'); return {}; } });
    h.ST.beforeEngineSync = () => { entered.resolve(); return sync.promise; };
    await h.ui.open(); await h.search(); h.choose(0); const running = h.node('media-relink-apply').dispatch('click');
    h.state.project = fixtureProject(); h.state.project.name = 'Current project'; apply.resolve({ project: fixtureProject() }); await entered.promise;
    assert(!h.calls.some(call => call.command === 'engine.project'));
    h.state.project.audioPlugins = [{ state: 'settled mapped edit' }];
    if (result === 'replaced-again') h.state.project = fixtureProject();
    sync.resolve(); await running;
    const native = h.calls.filter(call => call.command === 'engine.project');
    if (result === 'replaced-again') assert.equal(native.length, 0);
    else { assert.equal(native.length, 1); assert.equal(native[0].payload.project.audioPlugins[0].state, 'settled mapped edit'); assert.equal(native[0].payload.awaitPlugins, true); }
    assert.equal(h.edits.length, 0); assert(!h.toasts.some(toast => toast.kind === 'success'));
    if (result === 'failed') assert(h.toasts.some(toast => toast.kind === 'error' && /Relink restoration failed/.test(toast.message)));
  }
});

test('concurrent edits within the same project are not overwritten by apply', async () => {
  const wait = deferred(); const h = harness({ 'project.media.apply': () => wait.promise }); await h.ui.open(); await h.search(); h.choose(0);
  const applying = h.node('media-relink-apply').dispatch('click'); h.state.project.bpm = 147;
  const stale = fixtureProject(); stale.instruments[0].samplePath = '/new/verified.wav'; wait.resolve({ project: stale, relinked: [] }); await applying;
  assert.equal(h.edits.length, 0); assert.equal(h.state.project.bpm, 147); assert.equal(h.state.project.instruments[0].samplePath, '/old/kick.wav');
  assert(h.node('media-relink-status').textContent.includes('session changed') || h.document.html.includes('session changed'));
});

test('closing the dialog does not discard an already authorized valid apply', async () => {
  const wait = deferred(); const h = harness({ 'project.media.apply': () => wait.promise }); await h.ui.open(); await h.search(); h.choose(0);
  const applying = h.node('media-relink-apply').dispatch('click'); h.node('media-relink-close').dispatch('click');
  const updated = fixtureProject(); updated.instruments[0].samplePath = '/new/verified.wav'; wait.resolve({ project: updated, relinked: [{}] }); await applying;
  assert.equal(h.state.modal, null); assert.equal(h.edits.length, 1); assert.equal(h.state.project.instruments[0].samplePath, '/new/verified.wav');
});

(async () => {
  let passed = 0;
  for (const { name, run } of tests) { await run(); passed++; console.log(`PASS ${name}`); }
  console.log(`Media relink UI: ${passed} regression checks passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
