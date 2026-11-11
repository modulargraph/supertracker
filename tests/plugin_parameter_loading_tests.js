'use strict';
// Parameter loading for every insert owner (track, bus, group bus, Master) in the
// instrument plug-in pages, the MK3 controller view and the mixer insert panel. A fake bridge
// answers plugin.parameters; failures and silence must end in an error state, never an endless
// "Reading parameters…".
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const tick = async () => { for (let n = 0; n < 40; n++) await Promise.resolve(); };
const grain = Array.from({ length: 9 }, (_, index) => ({ address: String(index), name: ['Pitch', 'Size', 'Jitter', 'Reverse', 'Space', 'Density', 'Mod', 'Mix', 'Stereo'][index], normalized: index / 10, display: `g${index}`, writable: true, automatable: true }));

function pagesHarness() {
  const group = 'bus:kit-a-group';
  const plugins = [
    { owner: 'track:lane', kind: 'effect', slot: 0, instanceID: 'track-fx', component: 'aufx:dely:appl', name: 'AUDelay', format: 'AU' },
    { owner: 'bus:drums', kind: 'effect', slot: 1, instanceID: 'bus-vst3', component: 'vst3:abc', name: 'VST3 Comp', format: 'VST3' },
    { owner: group, kind: 'effect', slot: 0, instanceID: 'group-eq', component: 'stfx:eq', name: 'Four-band EQ', format: 'Builtin' },
    { owner: group, kind: 'effect', slot: 2, instanceID: 'group-grain', component: 'stfx:graindelay', name: 'Grain Delay', format: 'Builtin' },
    { owner: 'master', kind: 'effect', slot: 0, instanceID: 'master-limit', component: 'stfx:limiter', name: 'Peak Limiter', format: 'Builtin' }];
  const project = complete({ tracks: [{ instrument: 0, routingID: 'lane' }], instruments: [{ type: 'synth' }], audioPlugins: plugins,
    groups: [{ id: 'kit-a', name: 'Kit', busID: 'kit-a-group' }], routing: { buses: [{ id: 'drums' }, { id: 'kit-a-group' }] } });
  const state = { project, track: 0, instrument: 0, view: 'instrument' }, calls = [], timers = new Map(); let serial = 0, refreshes = 0, now = 1_000_000;
  // Per-instance native behaviour: an array answer, a rejection, or a promise that never settles.
  const behaviour = new Map();
  const ST = { state, escapeHTML: String, markDirty() {}, toast() {}, refresh() { refreshes++; },
    mutate(label, fn) { fn(state.project); },
    request(command, payload) {
      calls.push({ command, payload: JSON.parse(JSON.stringify(payload)) });
      if (command !== 'plugin.parameters') return Promise.resolve({});
      const plugin = plugins.find(item => item.instanceID === payload.instanceID);
      const mode = behaviour.get(payload.instanceID) ?? 'ok';
      if (mode === 'hang') return new Promise(() => {});
      if (mode instanceof Error) return Promise.reject(mode);
      if (mode === 'empty') return Promise.resolve([]);
      return Promise.resolve(plugin?.component === 'stfx:graindelay' ? grain : grain.slice(0, 3));
    } };
  const DateShim = class extends Date { static now() { return now; } };
  const context = vm.createContext({ window: { ST }, document: { activeElement: null, querySelector: () => null }, Map, Set, Promise, Date: DateShim, Error,
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, at: now + (delay || 0) }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'plugin-pages.js');
  const advance = ms => { now += ms; for (const [id, timer] of [...timers]) if (timer.at <= now) { timers.delete(id); timer.fn(); } };
  return { api: context.window.STPluginPages, ST, state, project, plugins, calls, behaviour, advance, group, get refreshes() { return refreshes; } };
}

function studioHarness(fail) {
  const groups = new Map(), nodes = new Map(), calls = []; let refreshes = 0;
  const document = { body: {}, activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => groups.get(selector) || [], querySelector: selector => groups.get(selector)?.[0] || null };
  const project = complete({ activePattern: 0, tracks: [{ name: 'Keys', instrument: 0 }], instruments: [{ type: 'synth' }], patterns: [{ id: 0, length: 16, automation: [] }],
    audioPlugins: [{ track: 0, kind: 'effect', slot: 0, component: 'stfx:graindelay', name: 'Grain Delay', format: 'Builtin', instanceID: 'grain' }, { track: 0, kind: 'effect', slot: 1, component: 'stfx:eq', name: 'Four-band EQ', format: 'Builtin', instanceID: 'eq' }] });
  const state = { project, track: 0, instrument: 0, view: 'mixer', selectedLane: 0, plugins: [] };
  const ST = { state, escapeHTML: value => String(value), toast() {}, mutate(label, fn) { fn(state.project); }, refresh() { refreshes++; },
    request(command, payload = {}) { calls.push({ command, payload }); if (command === 'plugin.parameters') return payload.slot === 1 ? Promise.reject(fail) : Promise.resolve({ parameters: grain }); return Promise.resolve({}); } };
  const context = vm.createContext({ window: { ST }, document, setTimeout() { return 0; }, clearTimeout() {}, setInterval() { return 0; }, clearInterval() {}, requestAnimationFrame: fn => fn() });
  web.load(context, 'studio.js'); web.load(context, 'insert-chain.js');
  return { studio: context.window.STStudio, calls, get refreshes() { return refreshes; } };
}

(async () => {
  {
    // Every owner type addresses its exact slot by its stable owner.
    const h = pagesHarness();
    const expected = [
      ['track-fx', { owner: 'track:lane', kind: 'effect', slot: 0, instanceID: 'track-fx' }],
      ['bus-vst3', { owner: 'bus:drums', kind: 'effect', slot: 1, instanceID: 'bus-vst3' }],
      ['group-grain', { owner: h.group, kind: 'effect', slot: 2, instanceID: 'group-grain' }],
      ['master-limit', { owner: 'master', kind: 'effect', slot: 0, instanceID: 'master-limit' }]];
    for (const [id, target] of expected) {
      const plugin = h.plugins.find(item => item.instanceID === id), entry = h.api._test.entryFor(plugin);
      await h.api._test.read(entry);
      assert.deepEqual(h.calls.at(-1).payload, target, `${id} reads its own slot`);
      assert.equal(entry.loaded, true); assert.equal(entry.error, '');
    }
    console.log('PASS track, bus, group bus and Master inserts address their own slot');
  }
  {
    // The reported case: group bus insert 3 (Grain Delay) on the MK3.
    const h = pagesHarness(), grainPlugin = h.plugins.find(item => item.instanceID === 'group-grain');
    const ctl = h.api.controller.forOwner(h.group, 'group-grain');
    assert.equal(ctl.view().loaded, false);
    await tick();
    const loaded = ctl.view();
    assert.equal(loaded.loaded, true); assert.equal(loaded.error, '');
    assert.equal(loaded.slotIndex, 2); assert.equal(loaded.owner, h.group); assert.equal(loaded.name, 'Grain Delay');
    assert.deepEqual(Array.from(loaded.knobs.map(knob => knob?.name)), ['Pitch', 'Size', 'Jitter', 'Reverse', 'Space', 'Density', 'Mod', 'Mix']);
    assert.equal(loaded.knobs[0].writable, true, 'MK3 knobs can change the built-in effect');
    ctl.adjust(0, 0.1); ctl.finish(); await tick();
    const write = h.calls.find(call => call.command === 'plugin.parameter').payload;
    assert.deepEqual({ owner: write.owner, kind: write.kind, slot: write.slot, instanceID: write.instanceID, track: write.track }, { owner: h.group, kind: 'effect', slot: 2, instanceID: 'group-grain', track: undefined });
    assert.ok(grainPlugin);
    console.log('PASS group bus Grain Delay shows eight writable MK3 knobs');
  }
  {
    // A native refusal ends in an error with Retry, then recovers on its own once the slot loads.
    const h = pagesHarness();
    Object.assign(h.plugins[0], { owner: 'track:lane', component: 'stfx:graindelay', name: 'Grain Delay' });
    h.behaviour.set('track-fx', new Error('Grain Delay is not loaded in the audio engine, so its parameters are unavailable.'));
    const ctl = h.api.controller.forOwner('track:lane', 'track-fx');
    ctl.view(); await tick();
    const failed = ctl.view();
    assert.equal(failed.loaded, false); assert.match(failed.error, /not loaded in the audio engine/);
    assert.equal(failed.knobs.every(knob => knob === null), true, 'no knobs are invented for an unread slot');
    const html = h.api.render(null, null);
    assert.doesNotMatch(html, /Reading parameters…/, 'the error replaces the loading text');
    assert.match(html, /data-pp-state="error"/); assert.match(html, /not loaded in the audio engine/); assert.match(html, /data-pp-action="retry"/);
    const attempts = () => h.calls.filter(call => call.command === 'plugin.parameters').length, first = attempts();
    ctl.view(); await tick();
    assert.equal(attempts(), first, 'errors are not re-requested on every MK3 frame');
    h.behaviour.delete('track-fx'); h.advance(h.api._test.RETRY_AFTER);
    ctl.view(); await tick();
    assert.equal(attempts(), first + 1, 'the read retries after the retry interval');
    const recovered = ctl.view();
    assert.equal(recovered.loaded, true); assert.equal(recovered.error, ''); assert.equal(recovered.knobs[7].name, 'Mix');
    console.log('PASS failed reads show the native reason with Retry and recover automatically');
  }
  {
    // A request that never answers times out instead of reading forever.
    const h = pagesHarness(), plugin = h.plugins.find(item => item.instanceID === 'master-limit');
    h.behaviour.set('master-limit', 'hang');
    const entry = h.api._test.entryFor(plugin), pending = h.api._test.read(entry);
    await tick(); assert.equal(entry.loading, true);
    h.advance(h.api._test.READ_TIMEOUT); await pending;
    assert.equal(entry.loading, false); assert.equal(entry.loaded, false);
    assert.match(entry.error, /Peak Limiter did not report its parameters within 8 seconds/);
    console.log('PASS a silent parameter request times out with a clear message');
  }
  {
    // An empty list is a finished read: "no host parameters", not loading.
    const h = pagesHarness(); h.plugins[0].owner = 'track:lane'; h.behaviour.set('track-fx', 'empty');
    h.api.render(null, null); const ctl = h.api.controller.forOwner('track:lane', 'track-fx'); ctl.view(); await tick();
    assert.equal(ctl.view().loaded, true); assert.equal(ctl.view().pages.length, 0);
    const html = h.api.render(null, null);
    assert.match(html, /data-pp-state="empty"/); assert.match(html, /exposes no host parameters/); assert.doesNotMatch(html, /Reading parameters…/);
    console.log('PASS a plug-in without host parameters says so');
  }
  {
    // A slot that disappeared from the project reports it instead of waiting.
    const h = pagesHarness(), plugin = h.plugins.find(item => item.instanceID === 'bus-vst3'), entry = h.api._test.entryFor(plugin);
    h.project.audioPlugins = h.project.audioPlugins.filter(item => item !== plugin);
    await h.api._test.read(entry);
    assert.match(entry.error, /no longer in the project/);
    console.log('PASS a removed slot ends its read with an explanation');
  }
  {
    // Mixer insert panel: the failing insert shows why, the working one shows controls.
    const h = studioHarness(new Error('Four-band EQ is not loaded in the audio engine, so its parameters are unavailable.'));
    await h.studio.loadAutomationParameters(0); await tick();
    const html = h.studio.renderMixer();
    assert.match(html, /studio-plugin-error[^>]*>Four-band EQ is not loaded in the audio engine/);
    assert.match(html, /data-plugin-address="8"/, 'the Grain Delay insert lists its parameters');
    assert.ok(h.refreshes > 0, 'the new error is rendered');
    const refreshes = h.refreshes; await h.studio.loadAutomationParameters(0); await tick();
    assert.equal(h.refreshes, refreshes, 'an unchanged error does not re-render in a loop');
    console.log('PASS mixer insert panel shows parameter errors next to working inserts');
  }
})().catch(error => { console.error(error); process.exit(1); });
