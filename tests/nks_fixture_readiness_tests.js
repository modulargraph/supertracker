'use strict';
// Run the native fixture's exact JavaScript guard/action against the parsed
// current app DOM. No WebKit, native executable, device, or plug-in is loaded.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { performance } = require('node:perf_hooks');
const fixture = fs.readFileSync(path.join(__dirname, 'NativeNKSUIAcceptance.swift'), 'utf8');
const preludeSource = /let bindingPrelude = """\n([\s\S]*?)\n\s*"""/.exec(fixture)?.[1];
const actionSource = /let immediateSave = try await mappedAction\("input-only edit and immediate Save", source: """\n([\s\S]*?)\n\s*""", arguments:/.exec(fixture)?.[1];
assert(preludeSource && actionSource, 'The actual native fixture guard/action must be extractable');
const prelude = preludeSource.replaceAll('\\(baseID)', 'nks-0-0');
assert(!prelude.includes('\\('), 'Only the baseID Swift interpolation is supported');
const prefix = fs.readFileSync(path.join(__dirname, 'nks_saved_view_tests.js'), 'utf8').split('\nconst tests = [], test =')[0];
const { harness, tick, deferred, bounded, copy, Element } = new Function('require', '__dirname', 'process',
  prefix + '\nreturn {harness,tick,deferred,bounded,copy,Element};')(require, __dirname, { env: {} });

// The reused parser models children in root.nodes. Give those children their
// real ancestor relation; leave all existing selector behavior untouched.
const originalClosest = Element.prototype.closest;
Element.prototype.closest = function (selector) {
  if (selector === '[data-nks-root]') {
    for (let node = this; node; node = node.parentElement)
      if (Object.hasOwn(node.dataset, 'nksRoot')) return node;
    return null;
  }
  return originalClosest.call(this, selector);
};
Element.prototype.dispatchEvent = function (event) { return this.fire(event.type, event); };
class Event { constructor(type, options = {}) { this.type = type; Object.assign(this, options); } }
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
const parameter = value => ({ address: '0', name: 'Cutoff', normalized: value, writable: true, automatable: true });
async function make() {
  const h = await harness(); let nativeValue = .074;
  h.handlers.set('plugin.parameters', () => [parameter(nativeValue)]);
  h.handlers.set('plugin.parameter', payload => {
    assert.equal(payload.instanceID, 'nks-save-instance'); assert.equal(payload.address, '0');
    nativeValue = payload.value; return { display: String(nativeValue) };
  });
  h.handlers.set('plugin.state', () => ({ ...copy(h.ST.state.project.audioPlugins[0]), state: `state:${nativeValue}` }));
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: [{ ...copy(h.ST.state.project.audioPlugins[0]), state: `state:${nativeValue}` }] }));
  const plugin = copy(h.ST.state.project.audioPlugins[0]); plugin.nks.pages[0].parameters[0].parameterID = 0;
  h.window.nativeEvent({ type: 'plugins.changed', payload: { plugins: [plugin] } }); await tick();
  h.window.__nksSelected = { page: 0, slot: 0, address: '0', name: 'Cutoff', instanceID: plugin.instanceID };
  assert(!h.knob(0, 'number').disabled && !h.knob().disabled);
  const run = source => {
    if (h.root) for (const node of h.root.nodes) node.parentElement = h.root;
    return bounded(new AsyncFunction('window', 'document', 'ST', 'STNKSControls', 'expectedComponent', 'expectedTarget', 'performance', 'Event', 'id',
      prelude + '\n' + source)(h.window, h.document, h.ST, h.window.STNKSControls, 'vst3:SAVED-VIEW', 'plugin:instrument:0:0', performance, Event, 'nks-0-0-number'));
  };
  return Object.assign(h, { run, value: () => nativeValue });
}
const readyOnly = 'return {ready:true,binding};';
const tests = [], test = (name, run) => tests.push({ name, run });
const mutations = h => h.calls.filter(call => ['plugin.parameter', 'plugin.state', 'engine.snapshot', 'project.save'].includes(call.command));

test('disabled exact controls return not ready before dispatching input or Save', async () => {
  const h = await make(); h.knob(0, 'number').disabled = true; h.knob().disabled = true;
  const generation = h.window.STNKSControls.editGeneration;
  const result = await h.run(actionSource);
  assert.equal(result.ready, false); assert.equal(result.binding.number.address, '0');
  assert.equal(result.binding.range.automate, 'plugin:instrument:0:0');
  assert.equal(h.window.STNKSControls.editGeneration, generation); assert.equal(mutations(h).length, 0);
});

test('enabled wrong ParamID, automation target, or binding fails without an action', async () => {
  for (const change of ['number-address', 'range-address', 'automation-target', 'unbound', 'wrong-enabled-with-disabled-peer']) {
    const h = await make();
    if (change === 'number-address') h.knob(0, 'number').dataset.nksAddress = '17';
    if (change === 'range-address') h.knob().dataset.nksAddress = '17';
    if (change === 'automation-target') h.knob().dataset.automate = 'plugin:instrument:0:17';
    if (change === 'unbound') h.root.dataset.nksBound = 'false';
    if (change === 'wrong-enabled-with-disabled-peer') { h.knob(0, 'number').disabled = true; h.knob().dataset.automate = 'wrong'; }
    await assert.rejects(h.run(actionSource), /Enabled .*wrong exact/, change);
    assert.equal(mutations(h).length, 0, change);
  }
});

test('changed or ambiguous exact plug-in identity and route fail without an action', async () => {
  for (const change of ['instance', 'component', 'kind', 'track', 'slot', 'duplicate']) {
    const h = await make(), plugin = h.ST.state.project.audioPlugins[0];
    if (change === 'instance') plugin.instanceID = 'replacement';
    if (change === 'component') plugin.component = 'vst3:OTHER';
    if (change === 'kind') plugin.kind = 'effect';
    if (change === 'track') plugin.track = 1;
    if (change === 'slot') plugin.slot = 1;
    if (change === 'duplicate') h.ST.state.project.audioPlugins.push(copy(plugin));
    await assert.rejects(h.run(actionSource), /identity or route changed/, change);
    assert.equal(mutations(h).length, 0, change);
  }
});

test('changed mapped metadata fails without an action', async () => {
  for (const change of ['mapped', 'address', 'name', 'missing']) {
    const h = await make(), page = h.ST.state.project.audioPlugins[0].nks.pages[0], mapping = page.parameters[0];
    if (change === 'mapped') mapping.mapped = false;
    if (change === 'address') mapping.parameterID = 17;
    if (change === 'name') mapping.name = 'Other';
    if (change === 'missing') page.parameters = [];
    await assert.rejects(h.run(actionSource), /metadata changed/, change);
    assert.equal(mutations(h).length, 0, change);
  }
});

test('missing or detached controls fail without an action', async () => {
  for (const change of ['missing', 'detached']) {
    const h = await make();
    if (change === 'missing') h.root.nodes = h.root.nodes.filter(node => node !== h.knob());
    else h.knob(0, 'number').isConnected = false;
    await assert.rejects(h.run(actionSource), /missing from the instrument view/, change);
    assert.equal(mutations(h).length, 0, change);
  }
});

async function assertOneSavedGesture(h) {
  const generation = h.window.STNKSControls.editGeneration;
  const result = await h.run(actionSource); assert.equal(result.ready, true); assert.equal(result.saved, true);
  assert.equal(h.window.STNKSControls.editGeneration, generation + 1);
  assert.equal(h.value(), .47);
  assert.equal(h.calls.filter(call => call.command === 'plugin.parameter').length, 1);
  assert.equal(h.calls.filter(call => call.command === 'project.save').length, 1);
  assert.equal(h.calls.find(call => call.command === 'project.save').payload.project.audioPlugins[0].state, 'state:0.47');
  assert.equal(result.inputTrace.afterInput.pendingEdits, true);
}

test('an enabled exact mapping runs the actual input and immediate Save exactly once', async () => {
  await assertOneSavedGesture(await make());
});

test('readiness-gap remount performs no action while loading, then one action after a fresh read', async () => {
  const h = await make(), oldInput = h.knob(0, 'number'), read = deferred();
  assert.equal((await h.run(readyOnly)).ready, true);
  const replacement = copy(h.ST.state.project.audioPlugins[0]); replacement.state = 'changed-opaque-state'; delete replacement.parameters;
  h.handlers.set('plugin.parameters', () => read.promise);
  h.window.nativeEvent({ type: 'plugins.changed', payload: { plugins: [replacement] } }); await tick();
  assert(!oldInput.isConnected); assert.notEqual(h.knob(0, 'number'), oldInput);
  const result = await h.run(actionSource);
  assert.equal(result.ready, false); assert.equal(result.binding.number.address, '0');
  assert.equal(result.binding.range.automate, null); assert.equal(mutations(h).length, 0);
  read.resolve([parameter(.074)]); await tick();
  await assertOneSavedGesture(h);
});

(async () => {
  let failed = 0;
  for (const { name, run } of tests) {
    try { await run(); console.log('PASS ' + name); }
    catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); }
  }
  console.log(`${tests.length - failed}/${tests.length} native fixture readiness groups passed.`);
  if (failed) process.exitCode = 1;
})();
