'use strict';
// Actual app queue, undo stack, native event dispatcher and duplication workflow.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm'), assert = require('node:assert/strict');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = async () => { for (let i = 0; i < 30; i++) await Promise.resolve(); };
function harness() {
  const nodes = new Map(), calls = [], handlers = new Map(), timers = new Map(); let nextTimer = 0, native;
  const node = selector => {
    const key = String(selector).replace(/^#/, '');
    if (!nodes.has(key)) nodes.set(key, { id: key, value: '', dataset: {}, innerHTML: '', style: {}, attributes: {}, classList: { toggle() {}, add() {}, remove() {} }, focus() { document.activeElement = this; }, select() {}, scrollIntoView() {}, closest() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, setAttribute(name, value) { this.attributes[name] = String(value); }, appendChild() {}, remove() {}, click() { return this.onclick?.(); } });
    return nodes.get(key);
  };
  const document = { activeElement: null, querySelector: node, querySelectorAll: () => [], getElementById: node, addEventListener() {}, createElement: node, body: null }; document.body = node('body');
  const window = { addEventListener() {}, webkit: { messageHandlers: { supertracker: { postMessage(message) {
    calls.push(copy(message)); const handler = handlers.get(message.command);
    Promise.resolve().then(() => handler ? handler(message.payload) : {}).then(result => window.nativeReceive({ id: message.id, ok: true, result }), error => window.nativeReceive({ id: message.id, ok: false, error: error.message }));
  } } } } };
  const context = vm.createContext({ window, document, console, innerWidth: 1080, innerHeight: 700, setTimeout(fn) { const id = ++nextTimer; timers.set(id, fn); return id; }, clearTimeout(id) { timers.delete(id); }, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise });
  for (const name of ['routing-project.js', 'app.js', 'track-duplicate-project.js', 'track-duplicate.js']) web.load(context, name);
  web.pendingEditsStub(context.window, 'nksControls', 'STNKSControls', 30);
  const ST = window.ST;
  ST.state.project = ST.normalize({ name: 'Original', tracks: [{ id: 0, routingID: 'lead', name: 'Lead', instrument: 0, volume: .7, pan: .1 }], instruments: [{ id: 0, name: 'Lead patch', type: 'au', plugin: { instanceID: 'synth', component: 'fixture' } }], patterns: [{ id: 0, length: 4, steps: [[{ note: 60, instrument: 0, fx1: { type: 'plugin', instanceID: 'synth', address: '19', value: .31 } }, null, null, null]], automation: [{ id: 'lane', track: 0, target: 'plugin:instrument:0:19', points: [{ row: 0, value: .31 }] }] }], audioPlugins: [{ track: 0, kind: 'instrument', slot: 0, instanceID: 'synth', component: 'fixture', name: 'Fixture synth', state: 'opaque-original' }, { track: 0, kind: 'effect', slot: 6, instanceID: 'effect', component: 'effect-fixture', state: 'opaque-effect' }] });
  ST.state.engineReady = true; native = copy(ST.state.project);
  const publish = project => { native = copy(project); window.nativeEvent({ type: 'plugins.changed', payload: { plugins: copy(project.audioPlugins || []) } }); };
  handlers.set('engine.snapshot', () => copy(native));
  handlers.set('engine.project', payload => { publish(payload.project); return {}; });
  handlers.set('plugin.parameters', payload => {
    if (!(native.audioPlugins || []).some(plugin => plugin.instanceID === payload.instanceID && plugin.track === payload.track && plugin.kind === payload.kind && plugin.slot === payload.slot)) throw Error('Exact plug-in instance was not restored');
    assert.deepEqual(copy(payload.addresses), []); return { parameters: [] };
  });
  return { ST, window, document, node, calls, handlers, publish, native: () => native, run: () => ST.perform('duplicateTrack') };
}
const tests = [], test = (name, fn) => tests.push([name, fn]);
test('Cmd-K command prepares exact state, publishes atomically once, verifies sparse slots and focuses the duplicate', async () => {
  const h = harness(), owner = h.ST.state.project, before = copy(owner); let observed;
  h.handlers.set('engine.project', payload => { h.publish(payload.project); observed = copy(owner); return {}; });
  assert.equal(await h.run(), true, h.node('toast-region').innerHTML);
  assert.deepEqual(observed, before, 'Proposal event did not replace original plug-ins before undo');
  assert.equal(h.ST.state.project, owner); assert.equal(owner.tracks.length, 2); assert.equal(owner.instruments.length, 2);
  assert.equal(h.ST.state.undo.length, 1); assert.deepEqual(copy(h.ST.state.undo[0].project), before);
  assert.equal(h.ST.state.track, 1); assert.equal(h.ST.state.instrument, 1); assert.equal(h.document.activeElement.id, 'tracker');
  const verified = h.calls.filter(call => call.command === 'plugin.parameters').map(call => call.payload);
  assert.deepEqual(verified.map(value => value.slot).sort(), [0, 6]); assert(verified.every(value => value.track === 1 && !['synth', 'effect'].includes(value.instanceID)));
  assert.equal(owner.audioPlugins[2].state, 'opaque-original'); assert.equal(owner.audioPlugins[3].state, 'opaque-effect');
  assert(h.calls.find(call => call.command === 'engine.project').payload.awaitPlugins);
});
test('undo and redo keep the same independent identities and opaque copied state', async () => {
  const h = harness(), original = copy(h.ST.state.project); assert.equal(await h.run(), true); const duplicated = copy(h.ST.state.project);
  h.ST.perform('undo'); assert.deepEqual(copy(h.ST.state.project), original);
  h.ST.perform('redo'); assert.deepEqual(copy(h.ST.state.project), duplicated); assert.equal(h.ST.state.undo.length, 1);
});
test('capture and native synchronization failures make no edit or history entry', async () => {
  for (const phase of ['capture', 'apply']) {
    const h = harness(), original = copy(h.ST.state.project); let applies = 0;
    if (phase === 'capture') h.handlers.set('engine.snapshot', () => ({ audioPlugins: [{ stateError: 'Cannot capture fixture' }] }));
    else h.handlers.set('engine.project', payload => { applies++; if (applies === 1) throw Error('Rejected proposal'); h.publish(payload.project); return {}; });
    assert.equal(await h.run(), false); assert.deepEqual(copy(h.ST.state.project), original); assert.equal(h.ST.state.undo.length, 0);
    assert.equal(h.window.STTrackDuplicate.isBusy, false); assert.equal(h.window.STTrackDuplicate.isApplying, false);
    if (phase === 'capture') assert.equal(applies, 0); else assert.equal(applies, 2);
  }
});
test('a retained desired descriptor cannot masquerade as a successfully loaded fresh instance', async () => {
  const h = harness(), original = copy(h.ST.state.project);
  h.handlers.set('plugin.parameters', () => { throw Error('Exact plug-in instance was not restored'); });
  assert.equal(await h.run(), false); assert.deepEqual(copy(h.ST.state.project), original); assert.equal(h.ST.state.undo.length, 0);
  const restored = h.calls.filter(call => call.command === 'engine.project').at(-1);
  assert.equal(restored.payload.replace, true); assert.deepEqual(restored.payload.project, original);
  assert.match(h.node('toast-region').innerHTML, /not restored/);
});
test('failed rollback reports both errors while leaving the original project and history intact', async () => {
  const h = harness(), original = copy(h.ST.state.project); let applies = 0;
  h.handlers.set('engine.project', payload => { if (++applies > 1) throw Error('Original graph unavailable'); h.publish(payload.project); return {}; });
  h.handlers.set('plugin.parameters', () => { throw Error('Copied instance unavailable'); });
  assert.equal(await h.run(), false); assert.deepEqual(copy(h.ST.state.project), original); assert.equal(h.ST.state.undo.length, 0);
  assert.match(h.node('toast-region').innerHTML, /Copied instance unavailable.*Could not restore.*Original graph unavailable/);
  assert.equal(h.window.STTrackDuplicate.isApplying, false);
});
test('every new plug-in is verified even when a legacy source lacks an explicit instance identity', async () => {
  const h = harness(); delete h.ST.state.project.audioPlugins[1].instanceID; h.publish(h.ST.state.project);
  assert.equal(await h.run(), true);
  const verified = h.calls.filter(call => call.command === 'plugin.parameters');
  assert.equal(verified.length, 2); assert(verified.every(call => call.payload.instanceID));
});
test('an ordinary synth track duplicates without inventing native plug-in descriptors', async () => {
  const h = harness(); h.ST.state.project.audioPlugins = []; h.ST.state.project.instruments[0].type = 'synth'; h.publish(h.ST.state.project);
  assert.equal(await h.run(), true); assert.deepEqual(copy(h.ST.state.project.audioPlugins), []);
  assert.equal(h.calls.filter(call => call.command === 'plugin.parameters').length, 0); assert.equal(h.ST.state.undo.length, 1);
});
test('edits or a different project during native apply win over the stale proposal', async () => {
  for (const kind of ['edit', 'replacement']) {
    const h = harness(), pending = deferred(); let applies = 0;
    h.handlers.set('engine.project', async payload => { applies++; if (applies === 1) { h.publish(payload.project); await pending.promise; } else h.publish(payload.project); return {}; });
    const running = h.run(); await tick(); assert(h.window.STTrackDuplicate.isApplying);
    if (kind === 'edit') h.ST.mutate('User changed note', project => { project.patterns[0].steps[0][0].note = 72; });
    else h.ST.state.project = h.ST.normalize({ name: 'New project' });
    const newer = copy(h.ST.state.project), history = h.ST.state.undo.length;
    pending.resolve(); assert.equal(await running, false);
    assert.deepEqual(copy(h.ST.state.project), newer); assert.equal(h.ST.state.undo.length, history);
    assert.deepEqual(h.calls.filter(call => call.command === 'engine.project').at(-1).payload.project, newer);
  }
});
test('pending mapped edits settle before proposal serialization and later mapped edits invalidate completion', async () => {
  for (const laterEdit of [false, true]) {
    const h = harness(), pending = deferred(); let waiting = true, generation = 0;
    h.window.STNKSControls = { get hasPendingEdits() { return waiting; }, get editGeneration() { return generation; }, async flushEdits() { await pending.promise; waiting = false; h.publish({ ...h.ST.state.project, audioPlugins: h.ST.state.project.audioPlugins.map(plugin => ({ ...plugin, state: 'latest mapped patch' })) }); return true; }, event() {}, adoptPlugins() {} };
    const native = deferred(); h.handlers.set('engine.project', async payload => { h.publish(payload.project); if (laterEdit) await native.promise; return {}; });
    const running = h.run(); await tick(); assert(!h.calls.some(call => call.command === 'engine.project'));
    pending.resolve(); await tick();
    if (laterEdit) { generation++; h.window.STNKSControls.flushEdits = async () => true; native.resolve(); }
    assert.equal(await running, !laterEdit);
    const payload = h.calls.find(call => call.command === 'engine.project').payload.project;
    assert.equal(payload.audioPlugins.at(-1).state, 'latest mapped patch');
    assert.equal(h.ST.state.undo.length, laterEdit ? 0 : 1);
  }
});
test('an unrelated plug-in publication is preserved and invalidates the staged duplicate', async () => {
  const h = harness(), pending = deferred(); let applies = 0;
  h.handlers.set('engine.project', async payload => { h.publish(payload.project); if (++applies === 1) await pending.promise; return {}; });
  const running = h.run(); await tick();
  const plugins = copy(h.ST.state.project.audioPlugins); plugins[0].instanceID = 'replacement-live-instance';
  h.window.nativeEvent({ type: 'plugins.changed', payload: { plugins } }); pending.resolve();
  assert.equal(await running, false); assert.equal(h.ST.state.project.audioPlugins[0].instanceID, 'replacement-live-instance'); assert.equal(h.ST.state.undo.length, 0);
});
test('save waits for the native proposal to commit instead of snapshotting staged plug-ins', async () => {
  const h = harness(), pending = deferred(); let applies = 0;
  h.handlers.set('engine.project', async payload => { h.publish(payload.project); if (++applies === 1) await pending.promise; return {}; });
  h.handlers.set('project.save', payload => ({ saved: true, project: payload.project }));
  const running = h.run(); await tick(); const saving = h.ST.projectAction('save'); await tick();
  assert(!h.calls.some(call => call.command === 'project.save')); pending.resolve(); assert.equal(await running, true); await saving;
  const saved = h.calls.find(call => call.command === 'project.save'); assert(saved); assert.equal(saved.payload.project.tracks.length, 2);
});
test('capacity rejects before any state capture and concurrent duplicate requests are coalesced', async () => {
  const full = harness(); full.ST.state.project.tracks = Array.from({ length: 1024 }, (_, id) => ({ id, routingID: `track-${id}`, instrument: 0 }));
  assert.equal(await full.run(), false); assert.equal(full.calls.length, 0); assert.equal(full.ST.state.undo.length, 0);
  const h = harness(), pending = deferred(); h.handlers.set('engine.project', async payload => { h.publish(payload.project); await pending.promise; return {}; });
  const first = h.run(); await tick(); assert.equal(await h.run(), false); pending.resolve(); assert.equal(await first, true);
  assert.equal(h.ST.state.undo.length, 1);
});
(async () => { for (const [name, run] of tests) { await run(); console.log('PASS ' + name); } console.log(`Track duplication UI: ${tests.length} groups passed.`); })().catch(error => { console.error(error); process.exitCode = 1; });
