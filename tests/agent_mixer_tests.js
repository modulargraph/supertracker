'use strict';
// window.STAgent mixer reach (agent-api.js): buses, master, outputs, sends, send effects, inserts on any channel and
// bus/master automation, on the real app modules (app.js, routing.js, studio.js, send-effects.js). Only the native
// message transport is a stand-in: a small audio host that keeps plug-in instances and accepts routing.
// Also: every MCP tool in rust/st-agent/src/tools.json names an existing STAgent method.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const { metadata, parameterList } = require('./helpers/builtin-effects');
const clone = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));

function makeDocument() {
  const stubs = new Map();
  const stub = id => {
    if (!stubs.has(id)) stubs.set(id, { id, innerHTML: '', textContent: '', value: '', dataset: {}, style: {}, classList: { toggle() {}, add() {}, remove() {}, contains: () => false },
      focus() {}, blur() {}, select() {}, scrollIntoView() {}, closest() { return null; }, querySelector() { return null; }, querySelectorAll() { return []; }, addEventListener() {},
      removeEventListener() {}, setAttribute() {}, removeAttribute() {}, getAttribute() { return null; }, getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 20 }), isConnected: false });
    return stubs.get(id);
  };
  return { activeElement: null, body: stub('body'), readyState: 'complete', addEventListener() {}, removeEventListener() {}, createElement: tag => stub('new-' + tag + Math.random()),
    getElementById: stub, querySelector: selector => /^#[\w-]+$/.test(selector) ? stub(selector.slice(1)) : null, querySelectorAll: () => [] };
}

// A routed song like the user's: a group bus "Avenue Kit" with inserts, tracks routed into it, master inserts.
function fixture() {
  return {
    name: 'Mixer', activePattern: 0, masterVolume: .7, bpm: 120, linesPerBeat: 4,
    tracks: [{ routingID: 'kick', name: 'Kick', instrument: 0, volume: .8, pan: 0 }, { routingID: 'hat', name: 'ClosedHH Avenue 1', instrument: 0, volume: .8, pan: 0 },
      { routingID: 'keys', name: 'Keys', instrument: 0, volume: .8, pan: 0 }],
    instruments: [{ type: 'sample', name: 'Kit', samplePath: '/fixture/kick.wav' }],
    routing: { version: 1, mixModel: 'standard', buses: [{ id: 'kit', name: 'Avenue Kit', type: 'group', volume: 1, pan: 0, mute: false, solo: false }],
      outputs: { 'track:kick': 'bus:kit', 'track:hat': 'bus:kit', 'track:keys': 'master', 'bus:kit': 'master' }, sends: [], sidechains: [] },
    audioPlugins: [
      { owner: 'bus:kit', kind: 'effect', slot: 0, component: 'stfx:filter', name: 'Multimode Filter', format: 'Builtin', instanceID: 'kit-filter', state: 'f', bypass: false },
      { owner: 'bus:kit', kind: 'effect', slot: 1, component: 'stfx:saturator', name: 'Saturator', format: 'Builtin', instanceID: 'kit-sat', state: 's', bypass: false },
      { owner: 'master', kind: 'effect', slot: 0, component: 'stfx:limiter', name: 'Peak Limiter', format: 'Builtin', instanceID: 'master-limiter', state: 'l', bypass: false }],
    patterns: [{ id: 0, name: 'A', length: 16, automation: [], steps: [Array(16).fill(null), Array(16).fill(null), Array(16).fill(null)] }],
    sequence: [0]
  };
}
const capabilities = () => ({ supported: true, mixModels: ['legacy', 'standard'], outputs: true, buses: { group: true, return: true, max: 64 },
  sends: { enabled: true, taps: ['preFader', 'postFader'], max: 128 }, sidechains: { enabled: false, taps: [], inputs: [] } });

function harness(project = fixture(), { duringLoad, dropWrite, queueWrites, applyAfterReads = 1 } = {}) {
  const document = makeDocument(), calls = [], timers = new Map(), pending = [];
  let clock = 0, nextTimer = 1, nextInstance = 1;
  let native = clone(project.audioPlugins || []).map(x => ({ ...x, parameters: parameterList(x.component) }));
  let host = clone(project);
  const ownerOf = payload => payload.owner || (payload.track !== undefined ? `track:${host.tracks[payload.track]?.routingID ?? payload.track}` : null);
  const strip = ({ parameters, ...rest }) => clone(rest);
  const descriptorFor = payload => native.find(p => p.owner === ownerOf(payload) && p.kind === (payload.kind || 'effect') && (p.slot ?? 0) === (payload.slot ?? 0));
  let omitFromSnapshot = false;
  const handlers = {
    'plugins.list': () => metadata().map(e => ({ id: e.id, name: e.name, manufacturer: 'SuperTracker', type: 'effect', format: 'Builtin' })),
    'builtin.effects': () => metadata(),
    'plugin.load': p => {
      const meta = metadata().find(effect => effect.id === p.component), owner = ownerOf(p);
      const descriptor = { owner, kind: p.kind, slot: p.slot, component: p.component, name: meta?.name || p.component, format: 'Builtin', instanceID: `fx-${nextInstance++}`,
        bypass: !!p.bypass, state: p.state || 'fresh', parameters: parameterList(p.component) };
      native = native.filter(x => !(x.owner === owner && x.kind === p.kind && x.slot === p.slot)).concat(descriptor);
      duringLoad?.();
      return strip(descriptor);
    },
    'plugin.state': p => {
      if (!Object.keys(p).length) return native.filter(x => !omitFromSnapshot || x.component !== 'stfx:chorus').map(strip);
      const d = descriptorFor(p); if (!d) throw new Error('There is no Audio Unit in this slot.');
      return strip(d);
    },
    'plugin.parameters': p => {
      for (const write of pending.splice(0)) {
        if (++write.reads < applyAfterReads) { pending.push(write); continue; }
        write.parameter.normalized = write.parameter.value = write.value; write.d.state = `edited-${write.parameter.address}-${write.value}`;
      }
      return clone(descriptorFor(p)?.parameters || []);
    },
    'plugin.parameter': p => {
      const d = descriptorFor(p); if (!d) throw new Error('There is no Audio Unit in this slot.');
      if (p.bypass !== undefined) { d.bypass = p.bypass; return strip(d); }
      const parameter = d.parameters.find(x => x.address === String(p.address));
      // Like the host: `written` is the position sent; the rest is what the plug-in reports right after the write.
      // A plug-in that drops a write still answers with the value it was sent (as some AUs report it).
      if (dropWrite?.(p)) return { ...clone(parameter), normalized: p.value, value: p.value, display: String(p.value), written: p.value };
      // NI Raum queues a write until its next render and meanwhile reports the old value; pending.applyAfterReads
      // reads of its parameters stand in for those renders (Infinity: a plug-in that is never rendered).
      if (queueWrites?.(p)) { pending.push({ d, parameter, value: p.value, reads: 0 }); return { ...clone(parameter), written: p.value }; }
      parameter.normalized = parameter.value = p.value; d.state = `edited-${p.address}-${p.value}`;
      return { ...clone(parameter), display: String(p.value), written: p.value };
    },
    'plugin.remove': p => { const owner = ownerOf(p); native = native.filter(x => !(x.owner === owner && x.slot === p.slot)); return {}; },
    'plugin.move': p => {
      const before = native.filter(x => x.owner === p.owner && x.kind === 'effect').sort((a, b) => a.slot - b.slot).map(strip);
      const slots = before.map(x => x.slot), order = before.slice(); order.splice(slots.indexOf(p.toSlot), 0, order.splice(slots.indexOf(p.fromSlot), 1)[0]);
      const after = order.map((x, i) => ({ ...x, slot: slots[i] }));
      for (const x of after) native.find(n => n.instanceID === x.instanceID).slot = x.slot;
      return { moved: true, owner: p.owner, fromSlot: p.fromSlot, toSlot: p.toSlot, previousPlugins: before, plugins: after,
        slotMap: before.map(x => ({ fromSlot: x.slot, toSlot: after.find(n => n.instanceID === x.instanceID).slot, instanceID: x.instanceID })) };
    },
    // Like the host: the published project's plug-in list is authoritative (a stale list removes newer instances).
    'engine.project': p => {
      host = clone(p.project);
      const wanted = new Set((p.project.audioPlugins || []).map(x => x.instanceID));
      native = native.filter(x => wanted.has(x.instanceID));
      for (const x of p.project.audioPlugins || []) {
        const n = native.find(y => y.instanceID === x.instanceID);
        if (n) { n.slot = x.slot; n.bypass = !!x.bypass; } else native.push({ ...clone(x), parameters: parameterList(x.component) });
      }
      return {};
    },
    'engine.snapshot': () => ({ audioPlugins: native.map(strip), routing: clone(host.routing) }),
    'routing.capabilities': () => capabilities(),
    'routing.validate': p => {
      const r = p.project.routing, owners = new Set(['master', ...p.project.tracks.map(t => `track:${t.routingID}`), ...r.buses.map(b => `bus:${b.id}`)]);
      for (const [from, to] of Object.entries(r.outputs)) if (!owners.has(from) || (to !== null && !owners.has(to))) return { valid: false, error: `Unknown routing owner ${from} → ${to}.` };
      return { valid: true, status: { unavailable: [] } };
    },
    'routing.apply': p => { host = clone(p.project); return { project: clone(p.project), capabilities: capabilities(), status: { unavailable: [] } }; },
    'routing.status': () => ({ unavailable: [] })
  };
  const window = { addEventListener() {}, removeEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, STHostTransport: hostStub({ post(message) {
    const request = clone(message); calls.push(request);
    Promise.resolve().then(() => (handlers[request.command] || (() => ({})))(request.payload || {}))
      .then(result => window.STHostTransport.deliver({ id: request.id, ok: true, result }), error => window.STHostTransport.deliver({ id: request.id, ok: false, error: error.message }));
  } }) };
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, Error, RegExp, innerWidth: 1400, innerHeight: 900,
    setTimeout: (fn, delay = 0) => { const id = nextTimer++; timers.set(id, { fn, at: clock + Number(delay || 0) }); return id; }, clearTimeout: id => timers.delete(id), setInterval: () => 0, clearInterval() {},
    queueMicrotask, requestAnimationFrame: fn => fn(), KeyboardEvent: class { constructor(type, init) { Object.assign(this, init, { type }); } }, Event: class { constructor(type) { this.type = type; } } });
  context.globalThis = window;
  web.load(context, 'app.js');
  for (const file of ['fine-control.js', 'effect-pages.js', 'routing-project.js', 'routing.js', 'studio.js', 'plugin-pages.js', 'insert-chain.js', 'send-effects.js', 'song-model.js', 'agent-api.js']) web.load(context, file);
  const ST = window.ST;
  ST.state.project = ST.normalize(project); ST.state.view = 'pattern'; ST.state.engineReady = true; ST.state.native = true;
  window.STEffectPages.setMetadata(metadata());
  const advance = ms => { clock += ms; for (const [id, timer] of [...timers].sort((a, b) => a[1].at - b[1].at)) if (timer.at <= clock) { timers.delete(id); timer.fn(); } };
  // Runs a call while the fake clock advances, so debounced engine updates and refreshes settle.
  async function call(method, params = {}) {
    let done = false, reply;
    window.STAgent.dispatch(method, JSON.stringify(params)).then(text => { reply = JSON.parse(text); done = true; });
    for (let i = 0; i < 400 && !done; i++) { await tick(); advance(20); }
    assert(done, `${method} did not settle`);
    for (let i = 0; i < 10; i++) { await tick(); advance(50); }
    return reply;
  }
  async function ok(method, params) { const reply = await call(method, params); assert(reply.ok, `${method} failed: ${reply.error}`); return reply.result; }
  async function rejects(method, params, pattern) {
    const before = JSON.stringify(ST.state.project), undo = ST.state.undo.length, reply = await call(method, params);
    assert.equal(reply.ok, false, `${method} should fail`); assert.match(reply.error, pattern);
    assert.equal(JSON.stringify(ST.state.project), before, 'A rejected call leaves the song untouched'); assert.equal(ST.state.undo.length, undo, 'and adds no undo step');
    return reply.error;
  }
  const undo = async () => { await ST.perform('undo'); for (let i = 0; i < 20; i++) { await tick(); advance(50); } };
  return { window, ST, calls, call, ok, rejects, undo, native: () => native, omitChorus: on => { omitFromSnapshot = on; }, advance,
    effects: owner => (ST.state.project.audioPlugins || []).filter(p => p.owner === owner && p.kind === 'effect').sort((a, b) => a.slot - b.slot).map(p => p.name) };
}

const tests = [], test = (name, fn) => tests.push({ name, fn });

test('mixer describes master, buses and tracks; channels resolve by name, "bus:<id>" and "master"', async () => {
  const h = harness();
  const mixer = await h.ok('mixer');
  assert.equal(mixer.mixModel, 'standard');
  assert.deepEqual(mixer.master.inserts.map(x => x.name), ['Peak Limiter']);
  const kit = mixer.buses[0];
  assert.equal(kit.channel, 'bus:kit'); assert.equal(kit.type, 'group'); assert.equal(kit.output, 'master');
  assert.deepEqual(kit.inputs, ['Kick', 'ClosedHH Avenue 1']); assert.deepEqual(kit.inserts.map(x => [x.slot, x.name, x.format, x.bypass]), [[0, 'Multimode Filter', 'Builtin', false], [1, 'Saturator', 'Builtin', false]]);
  assert.equal(mixer.tracks[1].output, 'bus:kit'); assert.equal(mixer.tracks[1].outputName, 'Avenue Kit'); assert.equal(mixer.tracks[2].output, 'master');
  assert.equal(mixer.capabilities.groupBuses, true);
  assert.match(mixer.sendEffects.delay.time, /ms/);
  assert.equal((await h.ok('mixer', { channel: 'avenue kit' })).channel, 'bus:kit');
  assert.equal((await h.ok('mixer', { channel: 'bus:kit' })).name, 'Avenue Kit');
  assert.equal((await h.ok('mixer', { channel: 'Master' })).type, 'master');
  assert.equal((await h.ok('mixer', { channel: 2 })).name, 'Keys');
  await h.rejects('mixer', { channel: 'Drums' }, /No channel matches "Drums". Channels: "master", "Avenue Kit" \(bus:kit\), 0 "Kick"/);
  const summary = await h.ok('summary');
  assert.deepEqual(summary.mixer.buses, [{ channel: 'bus:kit', name: 'Avenue Kit', type: 'group', effects: ['Multimode Filter', 'Saturator'] }]);
  assert.deepEqual(summary.mixer.masterEffects, ['Peak Limiter']);
});

test('bus inserts: load, read and set parameters, each one "AI:" undo step; Undo restores', async () => {
  const h = harness();
  let undo = h.ST.state.undo.length;
  const loaded = await h.ok('loadPlugin', { channel: 'Avenue Kit', plugin: 'Phaser' });
  assert.equal(loaded.channel, 'bus:kit'); assert.equal(loaded.slot, 2); assert.equal(loaded.undoLabel, 'AI: load Phaser on Avenue Kit');
  assert.equal(h.ST.state.undo.length, ++undo); assert.equal(h.ST.state.undo.at(-1).label, 'AI: load Phaser on Avenue Kit');
  assert.deepEqual(h.effects('bus:kit'), ['Multimode Filter', 'Saturator', 'Phaser']);
  const load = h.calls.find(c => c.command === 'plugin.load'); assert.equal(load.payload.owner, 'bus:kit'); assert.equal(load.payload.track, undefined);
  const params = await h.ok('pluginParameters', { channel: 'Avenue Kit', slot: 2, query: 'freq' });
  assert.equal(params.channel, 'bus:kit'); assert.deepEqual(params.parameters.map(p => p.name), ['Frequency']);
  const set = await h.ok('setPluginParameters', { channel: 'bus:kit', slot: 2, values: { Frequency: '25%', Feedback: { normalized: .5 } } });
  assert.equal(set.changed, true); assert.equal(h.ST.state.undo.length, ++undo); assert.match(h.ST.state.undo.at(-1).label, /^AI: Avenue Kit Phaser: Frequency, Feedback$/);
  const writes = h.calls.filter(c => c.command === 'plugin.parameter').map(c => [c.payload.owner, c.payload.address, c.payload.value]);
  assert.deepEqual(writes, [['bus:kit', '0', .25], ['bus:kit', '1', .5]]);
  assert.equal(h.ST.state.project.audioPlugins.find(p => p.owner === 'bus:kit' && p.slot === 2).state, 'edited-1-0.5', 'the captured state is in the project');
  await h.rejects('loadPlugin', { channel: 'Avenue Kit', plugin: 'Phaser', slot: 0 }, /Slot 0 on group bus "Avenue Kit" holds Multimode Filter/);
  await h.rejects('loadPlugin', { channel: 'master', plugin: 'Phaser', kind: 'instrument' }, /takes insert effects only/);
  await h.rejects('pluginParameters', { channel: 'Avenue Kit', slot: 9 }, /group bus "Avenue Kit" has no effect in slot 9. Effects: 0 Multimode Filter, 1 Saturator, 2 Phaser/);
  await h.rejects('setPluginParameters', { channel: 'Avenue Kit', track: 0, values: { a: 1 } }, /track or channel, not both/);
  await h.undo(); await h.undo();
  assert.deepEqual(h.effects('bus:kit'), ['Multimode Filter', 'Saturator']);
});

test('master inserts: load into a chosen slot, bypass, reorder and remove, each one "AI:" undo step', async () => {
  const h = harness();
  const loaded = await h.ok('loadPlugin', { channel: 'master', plugin: 'Compressor', slot: 3 });
  assert.equal(loaded.slot, 3); assert.deepEqual(h.effects('master'), ['Peak Limiter', 'Compressor']);
  const undo = h.ST.state.undo.length;
  const bypass = await h.ok('editInsert', { channel: 'master', slot: 0, bypass: true });
  assert.equal(bypass.undoLabel, 'AI: bypass Peak Limiter on Master'); assert.equal(h.native().find(p => p.instanceID === 'master-limiter').bypass, true);
  assert.equal((await h.ok('editInsert', { channel: 'master', slot: 0, bypass: true })).changed, false, 'no step when nothing changes');
  const moved = await h.ok('editInsert', { channel: 'master', slot: 3, moveTo: 'first' });
  assert.deepEqual(moved.inserts.map(x => [x.slot, x.name]), [[0, 'Compressor'], [3, 'Peak Limiter']]);
  assert.equal(h.ST.state.undo.at(-1).label, 'AI: move Compressor on Master');
  const removed = await h.ok('editInsert', { channel: 'master', slot: 0, remove: true });
  assert.deepEqual(removed.inserts.map(x => x.name), ['Peak Limiter']); assert.equal(h.ST.state.undo.at(-1).label, 'AI: remove Compressor from Master');
  assert.equal(h.ST.state.undo.length, undo + 3);
  await h.rejects('editInsert', { channel: 'master', slot: 0, bypass: false, remove: true }, /exactly one of bypass/);
  await h.rejects('editInsert', { channel: 'master', slot: 7, remove: true }, /Master has no insert in slot 7. Inserts: 3 Peak Limiter/);
  await h.undo(); assert.deepEqual(h.effects('master'), ['Compressor', 'Peak Limiter']);
});

test('channel levels, mute and names are plain edits; outputs go through the routing transaction', async () => {
  const h = harness();
  let undo = h.ST.state.undo.length;
  const edited = await h.ok('editChannel', { channel: 'Avenue Kit', volume: '-6 dB', pan: '20% L', mute: true });
  assert.equal(edited.channels[0].volume, '-6 dB'); assert.equal(h.ST.state.project.routing.buses[0].pan, -.2); assert.equal(h.ST.state.project.routing.buses[0].mute, true);
  assert.equal(h.ST.state.undo.length, ++undo); assert.equal(h.ST.state.undo.at(-1).label, 'AI: edit Avenue Kit (volume, pan, mute)');
  await h.ok('editChannel', { channel: 'master', volume: '-3 dB' }); assert.equal(Math.round(h.ST.state.project.masterVolume * 1000), 708); assert.equal(h.ST.state.undo.length, ++undo);
  await h.ok('editChannel', { channel: 'Avenue Kit', volume: '+6 dB' }); assert(h.ST.state.project.routing.buses[0].volume > 1.99, 'buses go up to +6 dB');
  undo++;
  h.calls.length = 0;
  const routed = await h.ok('editChannel', { channel: 'Keys', output: 'Avenue Kit' });
  assert.equal(h.ST.state.project.routing.outputs['track:keys'], 'bus:kit'); assert.equal(routed.channels[0].outputName, 'Avenue Kit');
  assert.equal(h.ST.state.undo.length, ++undo); assert.equal(h.ST.state.undo.at(-1).label, 'AI: edit Keys (output)');
  assert(['routing.validate', 'routing.apply'].every(command => h.calls.some(c => c.command === command)), 'validated and applied by the host');
  await h.ok('editChannel', { channel: ['Kick', 'ClosedHH Avenue 1'], output: 'none' });
  assert.equal(h.ST.state.project.routing.outputs['track:kick'], null); assert.equal(h.ST.state.undo.length, ++undo);
  await h.rejects('editChannel', { channel: 'master', mute: true }, /Master has no mute/);
  await h.rejects('editChannel', { channel: 'Keys', output: 'Kick' }, /cannot take an output: outputs go to a group or return bus/);
  await h.rejects('editChannel', { channel: 'Avenue Kit', volume: '+9 dB' }, /volume \(Avenue Kit\) must be between/);
  await h.undo(); assert.equal(h.ST.state.project.routing.outputs['track:kick'], 'bus:kit');
  await h.undo(); assert.equal(h.ST.state.project.routing.outputs['track:keys'], 'master');
});

test('buses and custom sends: create with inputs, send, edit, remove, remove bus; legacy songs explain enableStandardRouting', async () => {
  const h = harness();
  let undo = h.ST.state.undo.length;
  const bus = await h.ok('createBus', { name: 'Space', type: 'return', volume: '-3 dB' });
  assert.equal(bus.type, 'return'); assert.equal(bus.output, 'master'); assert.equal(h.ST.state.undo.length, ++undo); assert.equal(h.ST.state.undo.at(-1).label, 'AI: create return bus Space');
  const group = await h.ok('createBus', { name: 'Music', inputs: ['Keys'] });
  assert.equal(h.ST.state.project.routing.outputs['track:keys'], group.channel); assert.deepEqual(group.inputs, ['Keys']); undo++;
  const send = await h.ok('setSend', { from: 'Avenue Kit', to: 'Space', gain: '-12 dB' });
  assert.equal(send.send.gain, '-12 dB'); assert.equal(send.send.tap, 'postFader'); assert.equal(h.ST.state.undo.length, ++undo);
  await h.ok('setSend', { from: 'Avenue Kit', to: 'Space', tap: 'preFader', enabled: false });
  assert.equal(h.ST.state.project.routing.sends.length, 1); assert.equal(h.ST.state.project.routing.sends[0].tap, 'preFader'); undo++;
  await h.rejects('setSend', { from: 'Space', to: 'Space' }, /feedback loop/);
  await h.rejects('setSend', { from: 'master', to: 'Space' }, /Master cannot feed a send/);
  await h.ok('setAutomation', { channel: 'Avenue Kit', target: `send:${send.send.id}`, points: [{ row: 0, value: '-inf dB' }, { row: 8, value: '0 dB' }] }); undo++;
  await h.ok('setSend', { from: 'Avenue Kit', to: 'Space', remove: true });
  assert.equal(h.ST.state.project.routing.sends.length, 0); assert(!h.ST.state.project.patterns[0].automation.length, 'the send lane goes with it'); assert.equal(h.ST.state.undo.length, ++undo);
  await h.ok('loadPlugin', { channel: 'Music', plugin: 'Reverb' }); undo++;
  const removed = await h.ok('removeBus', { channel: 'Music' });
  assert.equal(removed.removed, group.channel); assert.equal(h.ST.state.project.routing.outputs['track:keys'], 'master');
  assert(!h.ST.state.project.audioPlugins.some(p => p.owner === group.channel), 'its inserts are removed'); assert.equal(h.ST.state.undo.length, ++undo);
  await h.rejects('removeBus', { channel: 'Keys' }, /is not a bus/);
  const effects = await h.ok('setSendEffects', { delay: { time: '1/8.', feedback: '40%' }, reverb: { predelay: '20 ms', size: '80%' } });
  assert.equal(h.ST.state.project.delay.sync, '1/8.'); assert.equal(h.ST.state.project.delay.feedback, .4); assert.equal(h.ST.state.project.reverb.predelay, .02);
  assert.equal(effects.sendEffects.reverb.predelay, '20 ms'); assert.equal(h.ST.state.undo.length, ++undo);
  await h.rejects('setSendEffects', { delay: { wobble: 1 } }, /Unknown delay setting "wobble". delay: time, feedback, tone, mix/);

  const legacy = fixture(); legacy.routing = { version: 1, mixModel: 'legacy', buses: [], outputs: {}, sends: [], sidechains: [] }; legacy.audioPlugins = [];
  const l = harness(legacy);
  await l.rejects('createBus', { name: 'Drums' }, /A bus needs standard routing.*enableStandardRouting/);
  await l.rejects('loadPlugin', { channel: 'master', plugin: 'Compressor' }, /Master inserts needs standard routing/);
  const legacyMixer = await l.ok('mixer'); assert.equal(legacyMixer.mixModel, 'legacy'); assert.match(legacyMixer.note, /enableStandardRouting/);
  const enabled = await l.ok('enableStandardRouting'); assert.equal(enabled.mixModel, 'standard'); assert.equal(l.ST.state.undo.at(-1).label, 'AI: enable standard routing');
  await l.ok('createBus', { name: 'Drums', inputs: [0, 1] }); assert.equal(l.ST.state.project.routing.buses[0].name, 'Drums');
});

test('bus and master automation are owner lanes in units; track send lanes are owner lanes too', async () => {
  const h = harness();
  const lane = await h.ok('setAutomation', { channel: 'Avenue Kit', target: 'volume', points: [{ row: 0, value: '-inf dB' }, { row: 15, value: '+6 dB' }] });
  assert.equal(lane.lane.channel, 'bus:kit');
  const stored = h.ST.state.project.patterns[0].automation[0];
  assert.equal(stored.owner, 'bus:kit'); assert.equal(stored.track, undefined); assert.deepEqual(stored.points.map(p => Math.round(p.value * 100)), [0, 200]);
  assert.equal(h.ST.state.undo.at(-1).label, 'AI: automate Channel volume on Avenue Kit');
  await h.ok('setAutomation', { channel: 'master', target: 'plugin:effect:0:1', points: [{ row: 0, value: '25%' }] });
  assert.equal(h.ST.state.project.patterns[0].automation[1].owner, 'master');
  const targets = await h.ok('automationTargets', { channel: 'Avenue Kit', query: 'volume' });
  assert.equal(targets.channel, 'bus:kit'); assert(targets.targets.some(t => t.target === 'volume' && t.automated));
  await h.rejects('setAutomation', { channel: 'master', target: 'pan', points: [{ row: 0, value: 0 }] }, /"pan" cannot be automated on Master/);
  await h.rejects('setAutomation', { channel: 'Avenue Kit', target: 'plugin:effect:7:1', points: [{ row: 0, value: 0 }] }, /no effect in slot 7/);
  await h.rejects('setAutomation', { channel: 'master', target: 'volume', points: [{ row: 0, value: '+6 dB' }] }, /between -inf dB … 3.52 dB/);
  await h.ok('deleteAutomation', { channel: 'bus:kit', target: 'volume' });
  assert.deepEqual(h.ST.state.project.patterns[0].automation.map(l => l.owner), ['master']);
  await h.ok('setAutomation', { track: 2, target: 'cutoff', points: [{ row: 0, value: '800 Hz' }] });
  assert.equal(h.ST.state.project.patterns[0].automation.at(-1).track, 2, 'track lanes stay track lanes');
  const send = await h.ok('setSend', { from: 'Keys', to: 'Avenue Kit', gain: '-6 dB' });
  await h.ok('setAutomation', { track: 2, target: send.send.automationTarget, points: [{ row: 4, value: '-12 dB' }] });
  const sendLane = h.ST.state.project.patterns[0].automation.at(-1);
  assert.equal(sendLane.owner, 'track:keys'); assert.equal(sendLane.track, undefined); assert.equal(Math.round(sendLane.points[0].value * 1000), 251);
});

test('loadPlugin on a routed track survives a pending engine update and never reports a load it did not attach', async () => {
  // The user's case: a sample track routed into a group bus. A debounced engine update from an earlier edit that
  // reached the host during the load used to remove the new insert, while the call still answered "changed".
  let h;
  h = harness(fixture(), { duringLoad: () => h.advance(500) });
  await h.ok('editTrack', { track: 1, volume: '-3 dB' });
  // Without advancing the clock, this edit leaves its debounced engine.project pending when loadPlugin starts.
  assert(JSON.parse(await h.window.STAgent.dispatch('editTrack', JSON.stringify({ track: 1, pan: '10% L' }))).ok);
  const loaded = await h.ok('loadPlugin', { plugin: 'Chorus', track: 1, kind: 'effect', slot: 0 });
  assert.equal(loaded.slot, 0);
  assert(h.native().some(p => p.owner === 'track:hat' && p.component === 'stfx:chorus'), 'the host keeps the instance');
  assert.deepEqual((await h.ok('summary')).tracks[1].effects.map(x => x.name), ['Chorus']);
  const load = h.calls.find(c => c.command === 'plugin.load'); assert.equal(load.payload.owner, 'track:hat', 'routed track plug-ins are addressed by owner');
  assert.equal((await h.ok('pluginParameters', { track: 1, kind: 'effect', slot: 0 })).plugin.name, 'Chorus');
  // A host that loads but no longer lists the instance: no success, no undo step.
  const missing = harness(); missing.omitChorus(true);
  await missing.rejects('loadPlugin', { plugin: 'Chorus', track: 2, kind: 'effect' }, /Chorus was not attached/);
});

test('setPluginParameters rewrites values a plug-in dropped from a burst and reports what it actually holds', async () => {
  // Writes after the first one in a burst are dropped (until the plug-in is read), like the AU in the user's report.
  let sinceRead = 0, reads = 0;
  const h = harness(fixture(), { dropWrite: p => p.owner === 'bus:kit' && (String(p.address) === '3' || sinceRead++ > 0) });
  const original = h.window.STHostTransport.post;
  h.window.STHostTransport.post = message => { if (message.command === 'plugin.parameters') { sinceRead = 0; reads++; } return original(message); };
  const undo = h.ST.state.undo.length;
  const result = await h.ok('setPluginParameters', { channel: 'Avenue Kit', slot: 0, values: { '0': '20%', '1': '40%', '2': { normalized: .6 } } });
  assert.deepEqual(result.parameters.map(p => [p.address, p.normalized, p.applied]), [['0', .2, true], ['1', .4, true], ['2', .6, true]]);
  assert.equal(h.ST.state.undo.length, undo + 1, 'still one undo step');
  assert.deepEqual(['0', '1', '2'].map(a => h.native().find(p => p.instanceID === 'kit-filter').parameters.find(x => x.address === a).normalized), [.2, .4, .6]);
  assert.match(h.ST.state.project.audioPlugins.find(p => p.instanceID === 'kit-filter').state, /^edited-/, 'the state was captured after the values stuck');
  // A value the plug-in never keeps is reported as such, not echoed back.
  const stuck = await h.ok('setPluginParameters', { channel: 'Avenue Kit', slot: 0, values: { '3': '90%' } });
  assert.equal(stuck.parameters[0].applied, false); assert.notEqual(stuck.parameters[0].normalized, .9); assert.match(stuck.warning, /did not keep/);
  assert(reads > 2);
});

test('setPluginParameters reports a write as applied only when the read-back holds it, not when it matches the write\'s own answer', async () => {
  // The user's report: NI Raum answered each write with its old value, the read-back showed the same old value,
  // and every parameter came back "applied": true. A plug-in that is never rendered never applies its queue.
  const h = harness(fixture(), { queueWrites: p => p.owner === 'bus:kit', applyAfterReads: Infinity });
  const before = ['0', '1'].map(a => Math.round(h.native().find(p => p.instanceID === 'kit-filter').parameters.find(x => x.address === a).normalized * 1e5) / 1e5);
  const result = await h.ok('setPluginParameters', { channel: 'Avenue Kit', slot: 0, values: { '0': '20%', '1': { normalized: .4 } } });
  assert.deepEqual(result.parameters.map(p => [p.address, p.requested, p.normalized, p.applied]), [['0', .2, before[0], false], ['1', .4, before[1], false]]);
  assert.match(result.warning, /did not keep .*Set them again/);
});

test('setPluginParameters waits for a plug-in that applies writes on its next render, then captures its state', async () => {
  const h = harness(fixture(), { queueWrites: p => p.owner === 'bus:kit', applyAfterReads: 2 });
  const result = await h.ok('setPluginParameters', { channel: 'Avenue Kit', slot: 0, values: { '0': '20%', '1': '40%', '2': { normalized: .6 } } });
  assert.deepEqual(result.parameters.map(p => [p.address, p.normalized, p.applied]), [['0', .2, true], ['1', .4, true], ['2', .6, true]]);
  assert.equal(result.warning, undefined);
  assert.match(h.ST.state.project.audioPlugins.find(p => p.instanceID === 'kit-filter').state, /^edited-/, 'the state was captured after the values applied');
  assert.equal(h.calls.filter(c => c.command === 'plugin.parameter').length, 3, 'nothing was written twice');
});

test('track automation needs a target the mixer lists for that track', async () => {
  const h = harness();
  await h.ok('setAutomation', { track: 2, target: 'cutoff', points: [{ row: 0, value: '800 Hz' }] });
  // Instrument settings that are not automation targets, and names from Object.prototype, make no lane.
  for (const target of ['filterType', 'mode', 'lfoSync', 'rootNote']) await h.rejects('setAutomation', { track: 2, target, points: [{ row: 0, value: 1 }] }, /cannot be automated on track 2 "Keys"/);
  for (const target of ['constructor', 'toString']) await h.rejects('setAutomation', { track: 2, target, points: [{ row: 0, value: 1 }] }, /Unknown automation target/);
  await h.rejects('setAutomation', { track: 2, target: 'plugin:effect:0:1', points: [{ row: 0, value: .5 }] }, /Track 2 "Keys" has no plug-in/);
  // A track that plays external MIDI has no sampler filter to automate (automationTargets does not list it either).
  h.ST.state.project.instruments[0].type = 'midi';
  await h.rejects('setAutomation', { track: 1, target: 'cutoff', points: [{ row: 0, value: '800 Hz' }] }, /"cutoff" cannot be automated on track 1/);
  await h.ok('setAutomation', { track: 1, target: 'volume', points: [{ row: 0, value: '-6 dB' }] });
});

test('every MCP tool names an STAgent method, and the mixer tools take channel', async () => {
  const h = harness(), methods = new Set(h.window.STAgent.methods());
  // The MCP server's tool catalogue (rust/st-agent/src/tools.json; `supertracker --mcp` serves it).
  const catalogue = JSON.parse(fs.readFileSync(path.join(__dirname, '../rust/st-agent/src/tools.json'), 'utf8'));
  const tools = catalogue.tools.map(tool => [tool.name, tool.method]);
  for (const [name, method] of tools) assert(/^[a-z_]+$/.test(name) && typeof method === 'string' && method, `${name} has a method`);
  assert(tools.length >= 50, `${tools.length} tools`);
  for (const [name, method] of tools) if (method !== '*' && !method.startsWith('app.')) assert(methods.has(method), `${name} → unknown method ${method}`);
  const docs = h.window.STAgent.docs();
  for (const method of ['loadPlugin', 'pluginParameters', 'setPluginParameters', 'automationTargets', 'setAutomation', 'deleteAutomation', 'editInsert', 'mixer', 'editChannel', 'show'])
    assert(docs[method].params.includes('channel'), `${method} takes channel`);
});

(async () => {
  let failures = 0;
  for (const { name, fn } of tests) { try { await fn(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`Agent mixer: ${tests.length - failures}/${tests.length} groups passed.`);
  if (failures) process.exitCode = 1;
})();
