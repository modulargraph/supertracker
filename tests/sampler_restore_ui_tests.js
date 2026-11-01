'use strict';
// Drive the actual sampler stretch flow; model the shared sync helper at its public boundary.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const source = web.source('sampler.js');
const copy = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const escaped = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const rendered = { path: '/rendered/stretch-01.wav', duration: 3.2, sampleRate: 48000 };
function project(name = 'Original', samplePath = '/samples/original.wav') { return { name, bpm: 120, tracks: [{ instrument: 0 }], instruments: [{ id: 0, type: 'sample', samplePath }], audioPlugins: [{ instanceID: 'nks-instance', state: 'old-opaque-state', parameters: [{ address: '17', normalized: .2 }] }] }; }
function harness(options = {}) {
  const nodes = new Map(), calls = [], mutations = [], notices = [], synchronizations = [], nativeDrafts = [], events = [];
  const state = { instrument: 0, view: 'instrument', modal: null, project: project(), samples: { 0: { path: '/samples/original.wav', duration: 2 } } };
  let closed = 0;
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: () => [] };
  const clear = () => { for (const node of nodes.values()) node.isConnected = false; nodes.clear(); };
  function mount(markup) {
    clear();
    for (const tag of markup.matchAll(/<(?:input|button|div)\b[^>]*\bid="([^"]+)"[^>]*>/g)) {
      const node = { id: tag[1], isConnected: true, disabled: /\sdisabled(?:\s|>)/.test(tag[0]), value: /\bvalue="([^"]*)"/.exec(tag[0])?.[1] || '', textContent: '', focus() { if (this.isConnected && !this.disabled) document.activeElement = this; }, select() {}, async click() { if (this.isConnected && !this.disabled) return this.onclick?.(); } };
      nodes.set(node.id, node);
    }
  }
  const ST = { state, escapeHTML: escaped,
    request(command, payload) { assert.equal(command, 'sample.edit', 'Restoration must go through tracked syncEngineProject, not a direct native project write'); calls.push({ command, payload: copy(payload) }); events.push(command); return options.edit?.promise || Promise.resolve(copy(rendered)); },
    async waitForGraphEdit() { events.push('graph-wait'); await options.graph?.promise; events.push('graph-ready'); },
    async syncEngineProject(factory, settings = {}) {
      assert.equal(typeof factory, 'function', 'Clone the latest project lazily after the shared helper flushes graph/NKS work'); synchronizations.push({ factory, settings }); events.push('helper-start');
      await options.helperGraph?.promise; events.push('helper-graph-ready'); await options.nks?.promise; options.flushNKS?.(state); events.push('nks-flushed');
      const draft = factory(); events.push('factory'); assert(!draft || typeof draft.then !== 'function', 'The project factory must be synchronous');
      if (draft == null) return false;
      nativeDrafts.push({ project: draft, awaitPlugins: true, ...settings }); events.push('restore-start');
      const result = options.native ? await options.native.promise : options.hasOwnProperty('restoreResult') ? options.restoreResult : { accepted: true };
      events.push('restore-finished'); return result;
    },
    mutate(label, fn) { mutations.push({ label, before: copy(state.project) }); fn(state.project); }, refresh() {}, toast(message, type) { notices.push({ message, type }); events.push('notice'); },
    openModal(title, body, footer = '') { state.modal = 'dialog'; mount(body + footer); }, closeModal() { state.modal = null; closed++; clear(); }
  };
  const context = vm.createContext({ window: { ST }, document, clearTimeout() {}, setTimeout: () => 1 }); web.load(context, 'sampler.js', { text: source });
  const ui = context.window.STSampler, get = id => { const node = nodes.get(id); assert(node, `Missing ${id}`); return node; };
  return { ui, ST, state, calls, mutations, notices, synchronizations, nativeDrafts, events, get, get closed() { return closed; }, open() { ui.stretchDialog(); }, start(ratio = '1.6') { get('sampler-stretch-ratio').value = ratio; return get('sampler-stretch-apply').click(); }, async settle() { for (let i = 0; i < 4; i++) await tick(); } };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('stale stretch recovery clones only after pending NKS flush and awaits native restoration before its retained-file notice', async () => {
  const edit = deferred(), nks = deferred(), native = deferred(), h = harness({ edit, nks, native, flushNKS: state => { state.project.audioPlugins[0].state = 'latest-captured-NKS-state'; state.project.audioPlugins[0].parameters[0].normalized = .82; } });
  const original = h.state.project, samples = copy(h.state.samples); h.open(); const pending = h.start(); assert.deepEqual(h.calls[0].payload, { path: '/samples/original.wav', instrument: 0, operation: 'stretch', ratio: 1.6 });
  h.state.project = project('Replacement', '/samples/replacement.wav'); edit.resolve(copy(rendered)); await h.settle();
  assert.equal(h.synchronizations.length, 1); assert.equal(h.nativeDrafts.length, 0); assert.equal(h.notices.length, 0); assert.equal(h.mutations.length, 0);
  // The user can replace/edit again while a pending mapped control is being captured.
  h.state.project = project('Latest session', '/samples/latest.wav'); h.state.project.bpm = 137; nks.resolve(); await h.settle();
  const sent = h.nativeDrafts[0]; assert.equal(sent.awaitPlugins, true); assert.equal(sent.project.name, 'Latest session'); assert.equal(sent.project.bpm, 137); assert.equal(sent.project.instruments[0].samplePath, '/samples/latest.wav'); assert.equal(sent.project.audioPlugins[0].state, 'latest-captured-NKS-state'); assert.equal(sent.project.audioPlugins[0].parameters[0].normalized, .82);
  assert.notEqual(sent.project, h.state.project); assert.notEqual(sent.project.audioPlugins, h.state.project.audioPlugins); assert.equal(h.notices.length, 0, 'Do not report recovery before native acceptance');
  h.state.project.bpm = 138; assert.equal(sent.project.bpm, 137, 'Native input is an isolated snapshot'); native.resolve({ accepted: true }); await pending;
  assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.samples, samples); assert.equal(original.instruments[0].samplePath, '/samples/original.wav'); assert.equal(h.state.project.instruments[0].samplePath, '/samples/latest.wav'); assert.equal(h.notices.length, 1); assert.equal(h.notices[0].type, 'error'); assert.match(h.notices[0].message, /destination changed/); assert(h.notices[0].message.includes(rendered.path)); assert(h.events.indexOf('restore-finished') < h.events.indexOf('notice')); assert.equal(h.get('sampler-stretch-apply').disabled, false);
});

test('replaced instruments and changed sample paths restore the current project without installing the old rendered sample', async () => {
  for (const kind of ['instrument', 'sample']) {
    const edit = deferred(), h = harness({ edit }); h.open(); const pending = h.start();
    if (kind === 'instrument') h.state.project.instruments[0] = { id: 0, type: 'sample', samplePath: '/samples/new-instrument.wav' }; else h.state.project.instruments[0].samplePath = '/samples/new-file.wav';
    const before = copy(h.state.project); edit.resolve(copy(rendered)); await pending;
    assert.equal(h.synchronizations.length, 1, kind); assert.deepEqual(copy(h.nativeDrafts[0].project), before); assert.deepEqual(h.state.project, before); assert.equal(h.mutations.length, 0); assert.equal(h.notices.at(-1).type, 'error'); assert(h.notices.at(-1).message.includes(rendered.path));
  }
});

test('a destination change during the graph wait is detected before sampler mutation and restored through the shared helper', async () => {
  const graph = deferred(), h = harness({ graph }); h.open(); const pending = h.start(); await h.settle(); assert.equal(h.synchronizations.length, 0); assert.equal(h.mutations.length, 0);
  h.state.project = project('Changed during graph wait', '/samples/after-graph.wav'); graph.resolve(); await pending;
  assert.equal(h.synchronizations.length, 1); assert.equal(h.nativeDrafts[0].project.name, 'Changed during graph wait'); assert.equal(h.mutations.length, 0); assert.equal(h.notices.at(-1).type, 'error');
});

test('failed, canceled and unconfirmed restoration retain the output path and never claim stretch success', async () => {
  for (const result of ['reject', false, null]) {
    const edit = deferred(), native = deferred(), h = harness({ edit, native }); h.open(); const pending = h.start(); h.state.project = project('New session', '/samples/new.wav'); const before = copy(h.state.project); edit.resolve(copy(rendered)); await h.settle(); assert.equal(h.notices.length, 0);
    if (result === 'reject') native.reject(new Error('Native restoration failed')); else native.resolve(result); await pending;
    assert.deepEqual(h.state.project, before); assert.equal(h.mutations.length, 0); assert.equal(h.notices.length, 1); assert.equal(h.notices[0].type, 'error'); assert(h.notices[0].message.includes(rendered.path)); assert.doesNotMatch(h.notices[0].message, /Time stretch complete/);
    assert.match(h.notices[0].message, result === 'reject' ? /Native restoration failed/ : result === false ? /canceled/ : /did not confirm/);
  }
});

test('closing the original dialog during stale recovery still restores audio without reopening or mutating the sampler', async () => {
  const edit = deferred(), nks = deferred(), h = harness({ edit, nks }); h.open(); const pending = h.start(); h.state.project = project('Other project', '/samples/other.wav'); edit.resolve(copy(rendered)); await h.settle(); h.ST.closeModal(); nks.resolve(); await pending;
  assert.equal(h.synchronizations.length, 1); assert.equal(h.nativeDrafts.length, 1); assert.equal(h.state.modal, null); assert.equal(h.closed, 1); assert.equal(h.mutations.length, 0); assert.equal(h.notices.at(-1).type, 'error'); assert(h.notices.at(-1).message.includes(rendered.path));
});

test('an unchanged destination commits exactly once and does not run the recovery helper', async () => {
  const h = harness(); h.open(); await h.start(); assert.equal(h.synchronizations.length, 0); assert.equal(h.nativeDrafts.length, 0); assert.equal(h.mutations.length, 1); assert.equal(h.mutations[0].before.instruments[0].samplePath, '/samples/original.wav'); assert.equal(h.state.project.instruments[0].samplePath, rendered.path); assert.deepEqual(copy(h.state.samples[0]), rendered); assert.equal(h.state.modal, null); assert.match(h.notices.at(-1).message, /Time stretch complete/);
});

test('success from dialog A may update its captured destination but cannot close newer stretch dialog B', async () => {
  const edit = deferred(), h = harness({ edit }); h.open(); const oldButton = h.get('sampler-stretch-apply'), pending = h.start(); h.ST.closeModal(); h.open(); const newerButton = h.get('sampler-stretch-apply'); h.get('sampler-stretch-progress').textContent = 'New dialog draft';
  edit.resolve(copy(rendered)); await pending; assert.notEqual(newerButton, oldButton); assert.equal(h.state.project.instruments[0].samplePath, rendered.path); assert.equal(h.mutations.length, 1); assert.equal(h.state.modal, 'dialog'); assert.equal(h.get('sampler-stretch-apply'), newerButton); assert.equal(h.get('sampler-stretch-progress').textContent, 'New dialog draft'); assert.equal(h.closed, 1);
});

test('stale recovery from dialog A cannot overwrite progress or re-enable Apply in newer dialog B', async () => {
  const edit = deferred(), h = harness({ edit }); h.open(); const pending = h.start(); h.ST.closeModal(); h.state.project = project('Dialog B project', '/samples/B.wav'); h.open(); const button = h.get('sampler-stretch-apply'), progress = h.get('sampler-stretch-progress'); button.disabled = true; progress.textContent = 'Newer operation is working';
  edit.resolve(copy(rendered)); await pending; assert.equal(h.synchronizations.length, 1); assert.equal(h.mutations.length, 0); assert.equal(h.state.modal, 'dialog'); assert.equal(h.get('sampler-stretch-apply'), button); assert.equal(button.disabled, true); assert.equal(progress.textContent, 'Newer operation is working'); assert.equal(h.closed, 1);
});

(async () => { for (const { name, run } of tests) { await run(); console.log(`PASS ${name}`); } console.log(`${tests.length} sampler restore UI suites passed.`); })().catch(error => { console.error(error); process.exitCode = 1; });
