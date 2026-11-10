'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('plugin-recovery.js');
const copy = value => JSON.parse(JSON.stringify(value));
const esc = value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const decode = value => value.replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const suspect = (key, options = {}) => ({ identity: { key, componentID: key, name: `Synth ${key}`, format: 'VST3', bundlePath: `/Library/${key}.vst3` }, reasons: ['Active near the interrupted session'], requiresReview: true, acknowledged: false, retryAuthorized: false, attribution: 'suspected, not proven', ...options });
const report = () => ({ suspects: [suspect('A'), suspect('B')], warnings: [] });
function harness(respond = () => report()) {
  const calls = [], toasts = [], timers = [], nodes = new Map();
  const state = { project: { name: 'Recovered session', audioPlugins: [{ component: 'A', state: 'saved-state' }] }, modal: null };
  const document = { activeElement: null, modal: null, html: '', getElementById: id => nodes.get(id), querySelector: selector => selector === '.plugin-recovery-modal' ? document.modal : null, querySelectorAll: selector => [...nodes.values()].filter(node => (selector.includes('[data-plugin-recovery-key]') && node.dataset.pluginRecoveryKey) || (selector.includes('[data-plugin-recovery-enable]') && node.dataset.pluginRecoveryEnable)) };
  function node(attrs = {}) {
    return { id: attrs.id, dataset: { pluginRecoveryKey: attrs['data-plugin-recovery-key'], pluginRecoveryEnable: attrs['data-plugin-recovery-enable'] }, disabled: 'disabled' in attrs, checked: 'checked' in attrs, handlers: {}, textContent: '', classList: { toggle() {} }, focus() { document.activeElement = this; }, setAttribute() {}, addEventListener(type, fn) { this.handlers[type] = fn; }, fire(type, extra = {}) { if ((type === 'click' || type === 'change') && this.disabled) return; return this.handlers[type]?.({ target: this, preventDefault() {}, stopPropagation() {}, ...extra }); } };
  }
  const invoker = node({ id: 'invoker' }); document.activeElement = invoker;
  const ST = {
    state, escapeHTML: esc, toast(message, kind) { toasts.push({ message, kind }); },
    request(command, payload) { calls.push({ command, payload: copy(payload) }); return Promise.resolve(respond(command, payload)); },
    openModal(title, body, footer, options) {
      state.modal = options.kind; document.html = body + footer; nodes.clear(); document.modal = node();
      for (const match of document.html.matchAll(/<([a-z][a-z0-9-]*)\b([^>]*)>/gi)) {
        const attrs = {}; for (const item of match[2].matchAll(/([a-zA-Z_:][\w:.-]*)(?:="([^"]*)")?/g)) attrs[item[1]] = decode(item[2] || '');
        if (attrs.id) nodes.set(attrs.id, node(attrs));
      }
    },
    closeModal() { state.modal = null; document.modal = null; nodes.clear(); document.activeElement = invoker; }
  };
  const context = vm.createContext({ window: { ST }, document, Set, Promise, setTimeout(fn) { timers.push(fn); } }); web.load(context, 'plugin-recovery.js', { text: source });
  const ui = context.window.STPluginRecovery;
  const get = id => { const value = nodes.get(id); assert(value, `Missing #${id}`); return value; };
  const select = index => { const input = get(`plugin-recovery-choice-${index}`); input.checked = true; input.fire('change'); };
  return { ui, state, document, calls, toasts, get, select, invoker, flush() { timers.splice(0).forEach(fn => fn()); }, close: ST.closeModal };
}
const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('module is inert until shown and opening never retries or changes saved plug-ins', async () => {
  const h = harness(), before = copy(h.state.project); assert.equal(h.calls.length, 0);
  await h.ui.show(); h.flush();
  assert.deepEqual(h.calls.map(call => call.command), ['plugins.recovery']);
  assert.equal(h.get('plugin-recovery-retry').disabled, true);
  assert.equal(h.document.querySelectorAll('[data-plugin-recovery-key]').filter(input => input.checked).length, 0);
  assert.equal(h.document.activeElement.id, 'plugin-recovery-keep');
  assert(h.document.html.includes('suspected, not proven') && h.document.html.includes('remain in your project with their settings'));
  assert.deepEqual(h.state.project, before);
});

test('ready report uses no extra request and excludes already authorized prior incidents', async () => {
  const h = harness(); await h.ui.show({ suspects: [suspect('A'), suspect('B', { requiresReview: false, retryAuthorized: true })], warnings: [] });
  assert.equal(h.calls.length, 0); assert.equal(h.document.querySelectorAll('[data-plugin-recovery-key]').length, 1);
  assert(h.document.html.includes('Synth A')); assert(!h.document.html.includes('Synth B'));
});

test('names, reasons, paths and warnings are escaped before rendering', async () => {
  const h = harness(); const value = suspect('unsafe'); value.identity.name = '<script>bad</script>'; value.reasons = ['<img onerror="bad">']; value.identity.bundlePath = '/path/<unsafe>.vst3';
  await h.ui.show({ suspects: [value], warnings: ['<warning>'] });
  assert(h.document.html.includes('&lt;script&gt;bad&lt;/script&gt;')); assert(h.document.html.includes('&lt;img onerror=&quot;bad&quot;&gt;'));
  assert(h.document.html.includes('/path/&lt;unsafe&gt;.vst3')); assert(h.document.html.includes('&lt;warning&gt;'));
});

test('Keep disabled closes with no retry and preserves settings and focus return', async () => {
  const h = harness(); const project = copy(h.state.project); await h.ui.show(report()); h.select(0);
  h.get('plugin-recovery-keep').fire('click');
  assert.equal(h.state.modal, null); assert.equal(h.calls.length, 0); assert.deepEqual(h.state.project, project); assert.equal(h.document.activeElement, h.invoker);
});

test('retry sends only chosen stable identities and coalesces repeated activation', async () => {
  const wait = deferred(); const h = harness(command => command === 'plugins.recovery.retry' ? wait.promise : report());
  await h.ui.show(report()); h.select(1);
  assert.equal(h.get('plugin-recovery-retry').textContent, 'Retry selected (1)');
  const retry = h.get('plugin-recovery-retry'); const promise = retry.fire('click'); retry.fire('click');
  assert.equal(h.calls.length, 1); assert.deepEqual(h.calls[0], { command: 'plugins.recovery.retry', payload: { keys: ['B'] } });
  assert.equal(h.get('plugin-recovery-choice-0').disabled, true);
  wait.resolve({ suspects: [suspect('A'), suspect('B', { requiresReview: false, retryAuthorized: true })], warnings: [] }); await promise;
  assert(h.document.html.includes('1 remain disabled')); assert(!h.document.html.includes('Synth B'));
  assert.equal(h.get('plugin-recovery-retry').disabled, true);
});

test('each disabled plug-in enables directly without checking a selection', async () => {
  const wait = deferred(), h = harness(() => wait.promise); await h.ui.show(report());
  const button = h.get('plugin-recovery-enable-1');
  assert.equal(h.get('plugin-recovery-choice-1').checked, false);
  const result = button.fire('click'); button.fire('click');
  assert.deepEqual(h.calls, [{ command: 'plugins.recovery.retry', payload: { keys: ['B'] } }]);
  assert.equal(h.get('plugin-recovery-enable-0').disabled, true);
  wait.resolve({ suspects: [suspect('A'), suspect('B', { requiresReview: false })] }); await result;
  assert(h.document.html.includes('Synth A')); assert(!h.document.html.includes('Synth B'));
});

test('successful final retry closes with exact count and keeps project states intact', async () => {
  const h = harness(() => ({ suspects: [suspect('A', { requiresReview: false, retryAuthorized: true }), suspect('B', { requiresReview: false, retryAuthorized: true })], warnings: [] }));
  const before = copy(h.state.project); await h.ui.show(report()); h.select(0); h.select(1); await h.get('plugin-recovery-retry').fire('click');
  assert.equal(h.state.modal, null); assert.equal(h.toasts[0].message, 'Retried 2 plug-ins.'); assert.deepEqual(h.state.project, before);
});

test('partial native failures remain visible without claiming restoration succeeded', async () => {
  const h = harness(() => ({ suspects: [suspect('A', { requiresReview: false, retryAuthorized: true })], retryErrors: [{ name: 'Synth A', message: 'Missing required content' }], warnings: [] }));
  await h.ui.show(report()); h.select(0); await h.get('plugin-recovery-retry').fire('click');
  assert.equal(h.state.modal, 'plugin-recovery'); assert.equal(h.toasts.length, 0);
  assert(h.document.html.includes('Missing required content') && h.document.html.includes('saved settings are preserved'));
  assert(!h.document.getElementById('plugin-recovery-retry')); assert(h.document.html.includes('>Close</button>'));
});

test('native errors and malformed retry replies preserve the dialog and selection', async () => {
  for (const fail of [() => Promise.reject(new Error('Host unavailable')), () => ({})]) {
    const h = harness(fail); await h.ui.show(report()); h.select(0); await h.get('plugin-recovery-retry').fire('click');
    assert.equal(h.state.modal, 'plugin-recovery'); assert.equal(h.get('plugin-recovery-choice-0').checked, true); assert.equal(h.get('plugin-recovery-retry').disabled, false); assert.equal(h.toasts.length, 0);
  }
});

test('closing or changing project while a report/retry waits cannot reopen or steal another dialog', async () => {
  const wait = deferred(); const h = harness(() => wait.promise);
  const showing = h.ui.show(); h.close(); wait.resolve(report()); await showing; h.flush();
  assert.equal(h.state.modal, null); assert.equal(h.document.activeElement, h.invoker);
  const loading = deferred(); const b = harness(() => loading.promise); await b.ui.show(report()); b.select(0);
  const retrying = b.get('plugin-recovery-retry').fire('click'); b.state.project = { name: 'New session' }; b.state.modal = 'settings';
  loading.resolve({ suspects: [], warnings: [] }); await retrying;
  assert.equal(b.state.modal, 'settings'); assert.equal(b.toasts.length, 0);
});

test('replacing the project during a retry still leaves a closable dialog (no stuck disabled buttons)', async () => {
  const wait = deferred(); const h = harness(() => wait.promise); await h.ui.show(report()); h.select(0);
  const retrying = h.get('plugin-recovery-retry').fire('click'); h.state.project = { name: 'Undo replaced the project object' };
  h.get('plugin-recovery-keep').fire('click');
  assert.equal(h.state.modal, null, 'Keep disabled closes our dialog while the retry is in flight');
  wait.resolve({ suspects: [suspect('B')], warnings: [] }); await retrying; h.flush();
  assert.equal(h.state.modal, null, 'the late reply does not reopen it');
  const late = deferred(); const b = harness(() => late.promise); await b.ui.show(report()); b.select(0);
  const pendingRetry = b.get('plugin-recovery-retry').fire('click'); b.state.project = { name: 'Reloaded' };
  late.resolve({ suspects: [suspect('B')], warnings: [] }); await pendingRetry; b.flush();
  assert(b.state.modal === null || (!b.get('plugin-recovery-keep').disabled && !b.get('plugin-recovery-retry')?.disabled), 'the dialog is closed or usable after the retry ends');
  if (b.state.modal) { b.get('plugin-recovery-keep').fire('click'); assert.equal(b.state.modal, null, 'Keep disabled still closes it'); }
});

test('arrow navigation never selects or retries, and Command Return retries explicit selections', async () => {
  const h = harness(() => ({ suspects: [], warnings: [] })); await h.ui.show(report());
  const first = h.get('plugin-recovery-choice-0'), second = h.get('plugin-recovery-choice-1');
  let prevented = false; first.fire('keydown', { key: 'ArrowDown', preventDefault() { prevented = true; } });
  assert(prevented); assert.equal(h.document.activeElement, second); assert.equal(second.checked, false); assert.equal(h.calls.length, 0);
  prevented = false; second.fire('keydown', { key: ' ', preventDefault() { prevented = true; } }); assert.equal(prevented, false, 'Space remains native checkbox behavior');
  h.select(1); h.document.modal.fire('keydown', { key: 'Enter', metaKey: true });
  for (let i = 0; i < 8; i++) await Promise.resolve();
  assert.deepEqual(h.calls[0].payload, { keys: ['B'] }); assert.equal(h.state.modal, null);
});

test('failed inspection reports unavailable, not an empty all-clear', async () => {
  const h = harness(() => Promise.reject(new Error('Cannot read recovery journal'))); await h.ui.show();
  assert(h.document.html.includes('report is unavailable')); assert(!h.document.html.includes('No plug-ins are waiting'));
  assert(h.document.html.includes('Cannot read recovery journal')); assert(!h.document.getElementById('plugin-recovery-retry'));
});

test('a plug-in lost while the project is open brings the review up, unless another dialog is open', () => {
  // app-events.js with the few parts of the app it takes, and a recorder in place of the dialog.
  const boot = (modal = null) => {
    const shown = [], toasts = [], state = { modal, project: {}, audio: {} };
    const noop = () => {};
    const STApp = { $: () => null, state, toast: (message, kind) => toasts.push({ message, kind }), receiveSampleMetadata: noop, acceptProject: noop, refresh: noop, receiveNoteRecordingStatus: noop,
      updateTransport: noop, receiveCountInStatus: noop, updatePlayhead: noop, pianoHighlight: noop, projectAction: noop, perform: noop, latencyNote: () => '', receiveAcknowledgedMIDI: noop,
      endRecordTake: noop, liveRecordGroups: noop, recordIncomingMIDI: noop };
    const window = { STApp, STNativeEvents: { consume: () => false }, STPluginRecovery: { show: value => shown.push(value) } };
    const context = vm.createContext({ window, document: { querySelector: () => null }, setTimeout, clearTimeout });
    vm.runInContext(web.source('app-events.js'), context, { filename: 'app-events.js' });
    return { event: (type, payload) => window.nativeEvent({ type, payload }), shown, toasts };
  };
  const lost = { suspects: [suspect('A')], warnings: [], pendingInstances: 1 };
  const page = boot();
  page.event('plugin.error', { error: 'Synth A stopped working: its plug-in process crashed (signal 11).', lost: true });
  page.event('plugins.recovery', lost);
  assert.deepEqual(page.toasts, [{ message: 'Synth A stopped working: its plug-in process crashed (signal 11).', kind: 'error' }]);
  assert.deepEqual(copy(page.shown), [lost], 'the report the host sent is the one reviewed');
  // Nothing to review, or a dialog already open: the event shows nothing (the command still does).
  page.event('plugins.recovery', { suspects: [suspect('B', { requiresReview: false })], warnings: [] });
  assert.equal(page.shown.length, 1);
  const busy = boot('export'); busy.event('plugins.recovery', lost);
  assert.equal(busy.shown.length, 0);
});

(async () => { for (const item of tests) { await item.fn(); console.log(`PASS ${item.name}`); } console.log(`Plug-in recovery UI: ${tests.length} regression groups passed.`); })().catch(error => { console.error(error); process.exitCode = 1; });
