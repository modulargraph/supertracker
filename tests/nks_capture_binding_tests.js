'use strict';
// Actual current app.js + NKS module, using the existing parsed DOM harness.
// No native executable, plug-in, device, or real filesystem bridge is involved.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const prefix = fs.readFileSync(path.join(__dirname, 'nks_saved_view_tests.js'), 'utf8').split('\nconst tests = [], test =')[0];
const { harness, tick, deferred, bounded, copy } = new Function('require', '__dirname', 'process',
  prefix + '\nreturn {harness,tick,deferred,bounded,copy};')(require, __dirname, { env: {} });
const parameters = value => [{ address: '17', name: 'Cutoff', normalized: value, writable: true, automatable: true }];

async function make() {
  const h = await harness();
  let nativeValue = .074;
  h.handlers.set('plugin.parameters', () => parameters(nativeValue));
  h.handlers.set('plugin.parameter', payload => {
    assert.equal(payload.instanceID, h.ST.state.project.audioPlugins[0].instanceID);
    assert.equal(payload.address, '17'); nativeValue = payload.value;
    return { display: String(nativeValue) };
  });
  h.handlers.set('plugin.state', () => ({ ...copy(h.ST.state.project.audioPlugins[0]), state: `state:${nativeValue}` }));
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: [{ ...copy(h.ST.state.project.audioPlugins[0]), state: `state:${nativeValue}` }] }));
  h.handlers.set('engine.project', payload => {
    const value = Number(payload.project.audioPlugins[0]?.state?.split(':')[1]);
    if (Number.isFinite(value)) nativeValue = value;
    return {};
  });
  h.fireTimer(750); await tick();
  assert.equal(h.knob(0, 'number').value, '0.074');
  return Object.assign(h, { value: () => nativeValue });
}
function edit(h, value = '.47') {
  const input = h.knob(0, 'number'), before = h.window.STNKSControls.editGeneration;
  assert(input.isConnected && !input.disabled, 'Expected an enabled mounted mapped control');
  input.focus(); input.value = value; input.fire('input');
  assert.equal(h.window.STNKSControls.editGeneration, before + 1, 'Enabled mapped input must accept the gesture');
  assert.equal(h.window.STNKSControls.hasPendingEdits, true);
  return input;
}
async function assertSavedEdit(h, value = .47) {
  assert.equal(await h.save(), true); await tick();
  assert.equal(h.value(), value);
  const writes = h.calls.filter(item => item.command === 'plugin.parameter');
  assert(writes.some(item => item.payload.address === '17' && item.payload.value === value));
  assert.equal(h.calls.findLast(item => item.command === 'project.save').payload.project.audioPlugins[0].state, `state:${value}`);
}
// Fresh descriptors replace the project's as after a state capture (app-session.js): the mapped controls adopt them first.
const adoptDescriptors=(h,plugins)=>{h.window.STNKSControls.adoptPlugins(plugins);h.ST.state.project.audioPlugins=plugins;h.ST.refresh();};

const tests = [], test = (name, run) => tests.push({ name, run });

test('standalone native snapshot refresh keeps the mounted mapped control usable', async () => {
  const h = await make(), oldDescriptor = h.ST.state.project.audioPlugins[0];
  assert.equal(await bounded(h.ST.captureNativeEdits()), true); await tick();
  assert.notEqual(h.ST.state.project.audioPlugins[0], oldDescriptor);
  assert.equal(h.knob(0, 'number').dataset.nksAddress, '17');
  edit(h); await assertSavedEdit(h);
});

for (const outcome of ['cancelled', 'failed']) test(`${outcome} Save after native state refresh keeps a subsequent gesture usable on retry`, async () => {
  const h = await make(), originalSave = h.handlers.get('project.save');
  h.handlers.set('project.save', () => { if (outcome === 'failed') throw Error('Controlled disk error'); return { cancelled: true }; });
  assert.equal(await h.save(), false); await tick();
  edit(h);
  h.handlers.set('project.save', originalSave);
  await assertSavedEdit(h);
});

test('same opaque snapshot preserves an unfinished numeric draft without manufacturing a write', async () => {
  const h = await make();
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: copy(h.ST.state.project.audioPlugins) }));
  const input = h.knob(0, 'number'); input.focus(); input.value = ''; input.fire('input');
  assert.equal(await bounded(h.ST.captureNativeEdits()), true); await tick();
  assert.equal(h.knob(0, 'number'), input); assert.equal(input.value, '');
  assert(!h.calls.some(item => item.command === 'plugin.parameter'));
  edit(h); await assertSavedEdit(h);
});

test('changed opaque snapshot clears stale drafts and caches; an older passive read cannot resurrect them', async () => {
  const h = await make(), oldRead = deferred(), freshRead = deferred(); let reads = 0;
  h.handlers.set('plugin.parameters', () => ++reads === 1 ? oldRead.promise : freshRead.promise);
  h.fireTimer(750); await tick(); assert.equal(reads, 1);
  const input = h.knob(0, 'number'); input.focus(); input.value = ''; input.fire('input');
  assert.equal(await bounded(h.ST.captureNativeEdits()), true); await tick();
  assert(reads >= 2, 'Changed opaque state must read a new authoritative parameter snapshot');
  assert(h.knob(0, 'number').disabled, 'Do not expose the preceding parameter cache as current');
  oldRead.resolve(parameters(.11)); await tick();
  assert(h.knob(0, 'number').disabled); assert.notEqual(h.knob(0, 'number').value, '0.11');
  freshRead.resolve(parameters(.82)); await tick();
  assert(!h.knob(0, 'number').disabled); assert.equal(h.knob(0, 'number').value, '0.82');
  assert.equal(h.window.STNKSControls.hasPendingEdits, false);
  assert(!h.calls.some(item => item.command === 'plugin.parameter'));
});

for (const mismatch of ['instance', 'component', 'owner', 'mapping', 'duplicate', 'removed']) test(`snapshot ${mismatch} change visibly revokes the old mapped controls and never redirects their input`, async () => {
  const h = await make(), input = h.knob(0, 'number'), oldRead = deferred();
  h.handlers.set('plugin.parameters', () => oldRead.promise); h.fireTimer(750); await tick();
  const replacement = copy(h.ST.state.project.audioPlugins[0]); replacement.state = 'different-opaque-state';
  if (mismatch === 'instance') replacement.instanceID = 'replacement-instance';
  if (mismatch === 'component') replacement.component = 'vst3:DIFFERENT';
  if (mismatch === 'owner') replacement.owner = 'track:track-B';
  if (mismatch === 'mapping') replacement.nks.pages[0].parameters[0].parameterID = 99;
  const next = mismatch === 'removed' ? [] : mismatch === 'duplicate' ? [replacement, copy(replacement)] : [replacement];
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: copy(next) }));
  assert.equal(await bounded(h.ST.captureNativeEdits()), true); await tick();
  assert(!input.isConnected || input.disabled, 'The previous visible binding must be disabled or removed');
  const generation = h.window.STNKSControls.editGeneration;
  input.value = '.93'; input.fire('input');
  assert.equal(h.window.STNKSControls.editGeneration, generation);
  assert.equal(h.window.STNKSControls.hasPendingEdits, false);
  oldRead.resolve(parameters(.91)); await tick();
  assert(!input.isConnected || input.disabled, 'An old passive response cannot revive a revoked control');
  assert(!h.calls.some(item => item.command === 'plugin.parameter'));
});

test('a mapped draft revoked by a mismatched native event makes the first Save fail explicitly', async () => {
  const h = await make(); edit(h, '.81');
  const replacement = { ...copy(h.ST.state.project.audioPlugins[0]), instanceID: 'replacement-instance' };
  adoptDescriptors(h,[replacement]);
  assert.equal(await h.save(), false);
  assert(!h.calls.some(item => item.command === 'project.save' || item.command === 'plugin.parameter'));
});

test('queued engine sync after acceptProject settles the mapped input before publication', async () => {
  const h = await make(); h.ST.acceptProject(copy(h.ST.state.project)); await tick();
  edit(h); await assertSavedEdit(h);
  const write = h.calls.findIndex(item => item.command === 'plugin.parameter');
  const publication = h.calls.findIndex(item => item.command === 'engine.project');
  assert(write >= 0 && publication > write);
});

test('input during an in-flight engine sync survives a matching native descriptor event', async () => {
  const h = await make(), gate = deferred(); let applying = false;
  h.handlers.set('engine.project', async () => { applying = true; await gate.promise; return {}; });
  h.ST.acceptProject(copy(h.ST.state.project)); await tick(); h.fireTimer(100); await tick(); assert(applying);
  edit(h); const saving = h.save(); await tick();
  assert(!h.calls.some(item => item.command === 'plugin.parameter'));
  adoptDescriptors(h,copy(h.ST.state.project.audioPlugins));
  gate.resolve(); assert.equal(await saving, true); assert.equal(h.value(), .47);
});

test('input arriving during native snapshot capture rejects that stale capture and remains retryable', async () => {
  const h = await make(), gate = deferred(), original = h.ST.state.project.audioPlugins[0];
  h.handlers.set('engine.snapshot', () => gate.promise);
  const capture = h.ST.captureNativeEdits(); await tick(); edit(h);
  gate.resolve({ audioPlugins: [{ ...copy(original), state: 'state:0.074' }] });
  assert.equal(await bounded(capture), false);
  assert.equal(h.ST.state.project.audioPlugins[0], original);
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: copy(h.ST.state.project.audioPlugins) }));
  await assertSavedEdit(h);
});

(async () => {
  let failed = 0;
  for (const { name, run } of tests) {
    try { await run(); console.log('PASS ' + name); }
    catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); }
  }
  console.log(`${tests.length - failed}/${tests.length} NKS capture-binding groups passed.`);
  if (failed) process.exitCode = 1;
})();
