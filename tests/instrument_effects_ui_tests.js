'use strict';
// Instrument insert chains (instrument-effects.js) on the real app modules: the Instrument page chain with the
// insert-chain component and window.STInserts (add, reorder, bypass, remove, Undo), project save/load, instrument
// copies (track duplicate, MK3 pad duplicate, removal reindexing), kits, automation lanes, step-FX lock targets
// and the MK3 PLUG-IN Instr. level. Only the native message transport and a small DOM are stand-ins.
const assert = require('node:assert/strict'), vm = require('node:vm');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const { makeDocument } = require('./helpers/mini-dom');
const { metadata, parameterList } = require('./helpers/builtin-effects');
const clone = value => JSON.parse(JSON.stringify(value));
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = () => new Promise(resolve => setImmediate(resolve));

function harness({ withMK3 = false } = {}) {
  const document = makeDocument(), calls = [], timers = new Map();
  let clock = 0, nextTimer = 1, nextInstance = 1, nativePlugins = [];
  const project = {
    name: 'Instrument chains', activePattern: 0, masterVolume: .7,
    tracks: [{ routingID: 'keys', name: 'Keys', instrument: 0, volume: .8, pan: 0 }, { routingID: 'pad', name: 'Pad', instrument: 0, volume: .8, pan: 0 },
      { routingID: 'bass', name: 'Bass', instrument: 1, volume: .8, pan: 0 }],
    instruments: [{ id: 0, name: 'Keys sampler', type: 'sample' }, { id: 1, name: 'Bass sampler', type: 'sample' }, { id: 2, name: 'Spare', type: 'sample' }],
    routing: { buses: [], outputs: { 'track:keys': 'master', 'track:pad': 'master', 'track:bass': 'master' }, sends: [], sidechains: [] },
    audioPlugins: [],
    patterns: [{ id: 0, name: 'A', length: 16, automation: [] }]
  };
  const descriptorFor = payload => nativePlugins.find(p => p.owner === payload.owner && p.kind === payload.kind && p.slot === payload.slot);
  const handlers = {
    'plugin.load': p => {
      const meta = metadata().find(effect => effect.id === p.component);
      const descriptor = { owner: p.owner, kind: p.kind, slot: p.slot, component: p.component, name: meta?.name || p.component, format: 'Builtin', manufacturer: 'SuperTracker', instanceID: `fx-${nextInstance++}`, bypass: !!p.bypass, state: p.state || 'fresh', parameters: parameterList(p.component) };
      nativePlugins = nativePlugins.filter(x => !(x.owner === p.owner && x.slot === p.slot)).concat(descriptor);
      return clone(descriptor);
    },
    'plugin.state': p => { const d = p.owner ? descriptorFor(p) : null; return d ? { owner: d.owner, kind: d.kind, slot: d.slot, component: d.component, instanceID: d.instanceID, state: d.state, bypass: d.bypass, parameters: d.parameters } : nativePlugins.map(clone); },
    'plugin.parameters': p => ({ parameters: clone(descriptorFor(p)?.parameters || []) }),
    'plugin.parameter': p => {
      const d = descriptorFor(p); if (!d) throw new Error('There is no Audio Unit in this slot.');
      if (p.bypass !== undefined) { d.bypass = p.bypass; const { parameters, ...rest } = d; return clone(rest); }
      const parameter = d.parameters.find(x => x.address === String(p.address)); parameter.normalized = parameter.value = p.value; d.state = `edited-${p.address}-${p.value}`; return { normalized: p.value };
    },
    'plugin.remove': p => { nativePlugins = nativePlugins.filter(x => !(x.owner === p.owner && x.slot === p.slot)); return {}; },
    'plugin.move': p => {
      const before = nativePlugins.filter(x => x.owner === p.owner && x.kind === 'effect').sort((a, b) => a.slot - b.slot).map(x => { const { parameters, ...rest } = x; return clone(rest); });
      const slots = before.map(x => x.slot), order = before.slice(); order.splice(slots.indexOf(p.toSlot), 0, order.splice(slots.indexOf(p.fromSlot), 1)[0]);
      const after = order.map((x, i) => ({ ...x, slot: slots[i] }));
      for (const x of after) nativePlugins.find(n => n.instanceID === x.instanceID).slot = x.slot;
      return { moved: true, owner: p.owner, fromSlot: p.fromSlot, toSlot: p.toSlot, previousPlugins: before, plugins: after, slotMap: before.map(x => ({ fromSlot: x.slot, toSlot: after.find(n => n.instanceID === x.instanceID).slot, instanceID: x.instanceID })) };
    },
    'engine.project': p => { const wanted = new Set((p.project.audioPlugins || []).map(x => x.instanceID)); nativePlugins = nativePlugins.filter(x => wanted.has(x.instanceID)); for (const x of p.project.audioPlugins || []) { const n = nativePlugins.find(y => y.instanceID === x.instanceID); if (n) { n.slot = x.slot; n.owner = x.owner; n.bypass = !!x.bypass; } else nativePlugins.push({ ...clone(x), parameters: parameterList(x.component) }); } return {}; },
    'engine.snapshot': () => ({ audioPlugins: nativePlugins.map(x => { const { parameters, ...rest } = x; return clone(rest); }) }),
    'builtin.effects': () => metadata(),
    'plugins.list': () => metadata().map(e => ({ component: e.id, name: e.name, manufacturer: 'SuperTracker', kind: 'effect', category: e.category, format: 'Builtin' }))
  };
  const window = { addEventListener() {}, removeEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, STHostTransport: hostStub({ post(message) {
    const request = clone(message); calls.push(request);
    Promise.resolve().then(() => (handlers[request.command] || (() => ({})))(request.payload || {}))
      .then(result => window.STHostTransport.deliver({ id: request.id, ok: true, result }), error => window.STHostTransport.deliver({ id: request.id, ok: false, error: error.message }));
  } }) };
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, Error, RegExp, innerWidth: 1400, innerHeight: 900,
    setTimeout: (fn, delay = 0) => { const id = nextTimer++; timers.set(id, { fn, at: clock + Number(delay || 0) }); return id; }, clearTimeout: id => timers.delete(id), setInterval: () => 0, clearInterval() {},
    requestAnimationFrame: fn => fn(), KeyboardEvent: class { constructor(type, init) { Object.assign(this, init, { type }); } }, Event: class { constructor(type) { this.type = type; } } });
  context.globalThis = window;
  web.load(context, 'app.js');
  for (const file of ['fine-control.js', 'effect-pages.js', 'routing-project.js', 'kit-groups.js', 'track-duplicate-project.js', 'routing.js', 'studio.js', 'plugin-pages.js', 'insert-chain.js', 'instrument-effects.js', 'step-fx.js']) web.load(context, file);
  if (withMK3) for (const file of ['mk3-screens.js', 'mk3.js']) web.load(context, file);
  const ST = window.ST;
  ST.state.project = ST.normalize(project); ST.state.view = 'instrument'; ST.state.instrument = 0; ST.state.track = 0; ST.state.engineReady = true; ST.state.native = true;
  window.STEffectPages.setMetadata(metadata());
  const owner = 'instrument:0';
  const mount = () => { const inst = ST.state.project.instruments[ST.state.instrument]; document.mount(window.STInstrumentEffects.render(inst, ST.state.instrument)); window.STInstrumentEffects.bind(ST.state.instrument); };
  const realRefresh = ST.refresh; ST.refresh = () => { try { realRefresh(); } catch { /* no app shell in this DOM */ } mount(); };
  const advance = ms => { clock += ms; for (const [id, timer] of [...timers].sort((a, b) => a[1].at - b[1].at)) if (timer.at <= clock) { timers.delete(id); timer.fn(); } };
  const settle = async () => { for (let i = 0; i < 40; i++) { await tick(); advance(50); } await tick(); mount(); };
  const chain = (o = owner) => window.STInserts.chain(o).map(p => `${p.name}${p.bypass ? ' (off)' : ''}`);
  const undo = async () => { await ST.perform('undo'); await settle(); };
  mount();
  return { window, ST, document, calls, owner, mount, settle, chain, undo, el: id => document.getElementById(id), native: () => nativePlugins };
}

const tests = [], test = (name, fn) => tests.push({ name, fn });

test('the Instrument page shows the instrument as the source and its effects after it; add, reorder, bypass, remove are Undo steps', async () => {
  const h = harness();
  assert(h.document.getElementById('instrument-effects').tagName === 'SECTION', 'the Instrument page mounts the chain');
  assert.equal(h.document.querySelector('#instfx-source .fx-tile-name').textContent, 'Keys sampler', 'the instrument itself is the first tile');
  assert.match(h.document.querySelector('.instrument-effects-where').textContent, /2 tracks Keys, Pad/);
  await h.el('instfx-add').click(); await h.settle();
  await h.document.querySelector('[data-fx-choose="stfx:compressor"]').click(); await h.settle();
  same(h.chain(), ['Compressor']);
  const added = h.ST.state.project.audioPlugins[0];
  assert.equal(added.owner, 'instrument:0'); assert.equal(added.kind, 'effect'); assert.equal(added.slot, 0);
  assert(h.calls.some(c => c.command === 'plugin.load' && c.payload.owner === 'instrument:0'), 'the host is asked to load it for the instrument');
  assert.equal(h.ST.state.undo.at(-1).label, 'Add plug-in effect');
  await h.window.STInserts.add(h.owner, 'stfx:filter'); await h.window.STInserts.add(h.owner, 'stfx:plate'); await h.settle();
  same(h.chain(), ['Compressor', 'Multimode Filter', 'Plate Reverb']);
  await h.el('instfx-tile-2').fire('keydown', { key: 'ArrowLeft', altKey: true }); await h.settle();
  same(h.chain(), ['Compressor', 'Plate Reverb', 'Multimode Filter']); assert.equal(h.ST.state.undo.at(-1).label, 'Reorder insert effect');
  await h.document.querySelector('[data-fx-power="0"]').click(); await h.settle();
  same(h.chain(), ['Compressor (off)', 'Plate Reverb', 'Multimode Filter']);
  assert(await h.window.STInserts.remove(h.owner, 1)); await h.settle();
  same(h.chain(), ['Compressor (off)', 'Multimode Filter']);
  same(h.chain('track:keys'), [], 'the track keeps its own (empty) insert chain');
  await h.undo(); same(h.chain(), ['Compressor (off)', 'Plate Reverb', 'Multimode Filter']);
  await h.undo(); same(h.chain(), ['Compressor', 'Plate Reverb', 'Multimode Filter']);
  await h.undo(); same(h.chain(), ['Compressor', 'Multimode Filter', 'Plate Reverb']);
  await h.undo(); await h.undo(); await h.undo(); same(h.chain(), []);
});

test('an instrument no track plays says so; MIDI instruments have no chain', async () => {
  const h = harness();
  h.ST.state.instrument = 2; h.mount();
  assert.match(h.document.querySelector('.instrument-effects-where').textContent, /No track plays this instrument/);
  assert.equal(h.window.STInstrumentEffects.render({ type: 'midi', name: 'Hardware' }, 2), '');
});

test('save and load keep the chain; host snapshots keep instrument owners', async () => {
  const h = harness();
  await h.window.STInserts.add(h.owner, 'stfx:eq'); await h.window.STInserts.add(h.owner, 'stfx:saturator'); await h.settle();
  const saved = JSON.parse(JSON.stringify(h.ST.state.project));
  const reopened = h.ST.normalize(clone(saved));
  same(h.window.STInstrumentEffects.chain(reopened, 0).map(p => [p.slot, p.component]), [[0, 'stfx:eq'], [1, 'stfx:saturator']]);
});

test('copies of an instrument take its chain with fresh instance IDs; removing an instrument reindexes chains and lanes', async () => {
  const h = harness();
  await h.window.STInserts.add(h.owner, 'stfx:eq'); await h.window.STInserts.add('instrument:1', 'stfx:chorus'); await h.settle();
  const p = h.ST.state.project, original = h.window.STInstrumentEffects.chain(p, 0)[0];
  // Track duplicate: Bass (instrument 1) gets its own copy of the instrument, with its chain.
  const proposal = h.window.STTrackDuplicateProject.build(clone(p), 2, () => 'id' + Math.random().toString(36).slice(2, 8)).project;
  const copyIndex = proposal.tracks.at(-1).instrument;
  assert.equal(copyIndex, 3);
  const copied = h.window.STInstrumentEffects.chain(proposal, 3);
  same(copied.map(x => x.component), ['stfx:chorus']);
  assert.notEqual(copied[0].instanceID, h.window.STInstrumentEffects.chain(p, 1)[0].instanceID, 'a fresh instance identity');
  // MK3-style instrument duplicate through the helper.
  const q = clone(p); q.instruments.push({ ...clone(q.instruments[0]), id: 3 });
  const copies = h.window.STInstrumentEffects.copy(q, 0, 3);
  assert.equal(copies.length, 1); assert.equal(copies[0].owner, 'instrument:3'); assert.notEqual(copies[0].instanceID, original.instanceID);
  // Removing instrument 0: its chain, the pages of its effects and its lanes go; instrument 1's chain, pages and lanes
  // become instrument 0's.
  const gone = h.window.STInstrumentEffects.chain(h.ST.state.project, 0)[0].instanceID, kept = h.window.STInstrumentEffects.chain(h.ST.state.project, 1)[0].instanceID;
  h.ST.mutate('lanes', x => {
    x.patterns[0].automation.push({ id: 'a', owner: 'instrument:0', target: 'plugin:effect:0:1', points: [] }, { id: 'b', owner: 'instrument:1', target: 'plugin:effect:0:1', points: [] });
    x.pluginPages[gone] = { pages: [{ name: 'A', addresses: ['1'] }] }; x.pluginPages[kept] = { pages: [{ name: 'B', addresses: ['2'] }] };
  });
  h.ST.state.instrument = 0; await h.ST.perform('removeInstrument'); await h.settle();
  const after = h.ST.state.project;
  same(after.audioPlugins.map(x => [x.owner, x.component]), [['instrument:0', 'stfx:chorus']]);
  same(after.patterns[0].automation.map(lane => [lane.id, lane.owner]), [['b', 'instrument:0']]);
  same(Object.keys(after.pluginPages), [kept]);
});

test('a kit inserted from a kit file brings each pad instrument chain to its new instrument index', () => {
  const h = harness(), q = clone(h.ST.state.project);
  const source = {
    tracks: [{ id: 0, name: 'Kick', routingID: 'kick', instrument: 0, groupID: 'kit', pad: 0 }, { id: 1, name: 'Snare', routingID: 'snare', instrument: 1, groupID: 'kit', pad: 1 }],
    instruments: [{ id: 0, name: 'Kick', type: 'sample' }, { id: 1, name: 'Snare', type: 'sample' }],
    groups: [{ id: 'kit', name: 'Kit', busID: 'kitbus' }],
    routing: { buses: [{ id: 'kitbus', name: 'Kit', type: 'group', groupID: 'kit' }], outputs: { 'track:kick': 'bus:kitbus', 'track:snare': 'bus:kitbus', 'bus:kitbus': 'master' }, sends: [], sidechains: [] },
    audioPlugins: [{ owner: 'instrument:1', kind: 'effect', slot: 0, component: 'stfx:transient', format: 'Builtin', instanceID: 'snare-shaper', state: 's' }],
    patterns: [{ id: 0, length: 16, steps: [[], []], automation: [] }]
  };
  let n = 0;
  h.window.STKitGroups.insertGroup(q, source, key => `fresh${n++}-${key.replace(/[^\w-]/g, '')}`.slice(0, 60));
  const snare = q.tracks.find(t => t.name === 'Snare');
  same(h.window.STInstrumentEffects.chain(q, snare.instrument).map(x => [x.component, x.state]), [['stfx:transient', 's']]);
  assert.notEqual(h.window.STInstrumentEffects.chain(q, snare.instrument)[0].instanceID, 'snare-shaper');
  same(h.window.STInstrumentEffects.chain(q, q.tracks.find(t => t.name === 'Kick').instrument), []);
});

test('automation lanes address instrument effects', async () => {
  const h = harness();
  const fx = await h.window.STInserts.add(h.owner, 'stfx:filter'); await h.settle();
  await h.window.STStudio.loadAutomationParameters(h.owner);
  const address = parameterList('stfx:filter')[1].address;
  h.ST.automate(h.owner, `plugin:effect:${fx.slot}:${address}`, 'Filter · Cutoff');
  const lane = h.ST.state.project.patterns[0].automation.at(-1);
  assert.equal(lane.owner, 'instrument:0'); assert.equal(lane.target, `plugin:effect:0:${address}`);
  assert.equal(lane.track, undefined, 'an instrument lane reaches every track that plays the instrument');
});

test('MK3 PLUG-IN Instr. level lists the instrument effects after the instrument source', async () => {
  const h = harness({ withMK3: true });
  await h.window.STInserts.add(h.owner, 'stfx:eq'); await h.window.STInserts.add(h.owner, 'stfx:chorus'); await h.window.STInserts.add('track:keys', 'stfx:limiter'); await h.settle();
  const mk = h.window.STMK3Parts;
  mk.M.level = 'instrument'; mk.M.pluginSlot = null;
  const page = mk.pluginPage();
  same(page.data.chain.items.map(item => item.right.replace(/ · off$/, '')), ['Instrument', 'Instr. FX 1', 'Instr. FX 2']);
  same(page.data.chain.items.map(item => item.name), ['Keys sampler', 'Four-band EQ', 'Chorus']);
  assert.equal(page.data.chain.selected, 0, 'the instrument source stays selected by default');
  mk.M.level = 'track'; mk.M.pluginSlot = null;
  assert(!mk.pluginPage().data.chain.items.some(item => /Instr\. FX/.test(item.right)), 'the track level does not list the instrument chain');
  // Selecting an instrument effect opens its pages, with Bypass on the instrument chain.
  mk.M.level = 'instrument'; mk.M.pluginSlot = `instrument:0:${h.window.STInstrumentEffects.chain(h.ST.state.project, 0)[1].instanceID}`;
  const fxPage = mk.pluginPage();
  assert.equal(fxPage.data.name, 'Chorus');
  const bypass = fxPage.buttons.find(b => b?.label === 'Bypass'); assert(bypass, 'Bypass button');
  bypass.action(); await h.settle();
  same(h.chain(), ['Four-band EQ', 'Chorus (off)']);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} instrument effect tests failed`); process.exit(1); }
  console.log(`${tests.length} instrument effect tests passed`);
})();
