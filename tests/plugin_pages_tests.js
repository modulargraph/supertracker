'use strict';
// Eight-knob pages for non-NKS plug-ins and NKS effects; user pages persist in the project.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('plugin-pages.js');
const tick = async () => { for (let n = 0; n < 30; n++) await Promise.resolve(); };
function harness() {
  const parameters = Array.from({ length: 11 }, (_, index) => ({ address: String(index), name: `P${index}`, normalized: index / 10, display: `d${index}`, writable: index !== 9, automatable: true }));
  const instrument = { owner: 'track:t0', kind: 'instrument', slot: 0, instanceID: 'inst', component: 'clap:com.example.plugin', format: 'CLAP', name: 'Synth', parameters };
  const nksEffect = { owner: 'track:t0', kind: 'effect', slot: 1, instanceID: 'fx', component: 'clap:com.example.plugin', format: 'CLAP', name: 'Delay', nks: { name: 'Echo', pages: [{ name: 'Main', parameters: [{ slot: 0, name: 'Mix', parameterID: 4, mapped: true }, { slot: 1, name: 'Gone', parameterID: 7, mapped: false }] }] } };
  const project = complete({ tracks: [{ instrument: 0, routingID: 't0' }], instruments: [{ type: 'plugin' }], audioPlugins: [instrument, nksEffect] });
  const state = { project, track: 0, instrument: 0 }, calls = [], undo = [], timers = new Map(); let serial = 0;
  const ST = { state, escapeHTML: String, markDirty() {}, toast() {}, refresh() {},
    mutate(label, fn) { undo.push(label); fn(state.project); },
    async request(command, payload) { calls.push({ command, payload }); if (command === 'plugin.state') return { state: 'captured', instanceID: payload.instanceID }; if (command === 'plugin.parameter') return { display: `v${payload.value}` }; return parameters; } };
  const context = vm.createContext({ window: { ST }, document: { activeElement: null, querySelector: () => null }, Map, Set, Promise, Date,
    setTimeout(fn, delay) { const id = ++serial; timers.set(id, { fn, delay }); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'plugin-pages.js', { text: source });
  return { api: context.window.STPluginPages, window:context.window, ST, state, project, instrument, nksEffect, calls, undo };
}
(async () => {
  {
    const h = harness(), { pagesFor, entryFor, slotsFor } = h.api._test;
    const entry = entryFor(h.instrument), pages = pagesFor(h.instrument, entry);
    assert.equal(pages.length, 2, 'ten writable parameters make two generated pages');
    assert.deepEqual(Array.from(pages[0].slots.map(slot => slot.address)), ['0', '1', '2', '3', '4', '5', '6', '7']);
    assert.equal(pages[0].name, 'P0 – P7'); assert.equal(pages[1].slots[1].address, '10', 'read-only parameters are skipped'); assert.equal(pages[1].slots[2], null);
    h.project.pluginPages = { inst: { pages: [{ name: 'Pinned', addresses: ['10', null, '3'] }] } };
    const withUser = pagesFor(h.instrument, entry);
    assert.equal(withUser.length, 3); assert.equal(withUser[0].kind, 'user'); assert.equal(withUser[0].slots[0].name, 'P10'); assert.equal(withUser[0].slots[1], null);
    const nks = pagesFor(h.nksEffect, entryFor(h.nksEffect));
    assert.equal(nks.length, 1); assert.equal(nks[0].slots[0].address, '4'); assert.equal(nks[0].slots[1].address, null, 'unmapped NKS controls never guess an address');
    assert.deepEqual(Array.from(slotsFor(null, h.instrument).map(plugin => plugin.instanceID)), ['inst', 'fx']);
    assert.deepEqual(Array.from(slotsFor(null, { ...h.instrument, nks: { pages: [{}] } }).map(plugin => plugin.instanceID)), ['fx'], 'an NKS instrument keeps its own console');
    console.log('PASS generated, user and NKS effect pages');
  }
  {
    const h = harness(), entry = h.api._test.entryFor(h.instrument);
    assert.equal(h.api.render(null, h.instrument).includes('data-pp-knob="0"'), true);
    assert.match(h.api.render(null, h.instrument), /data-automate="plugin:instrument:0:0"/);
    assert.equal(h.api.hasPendingEdits, false);
    // Simulate a knob edit through the internal preview path used by input events.
    entry.values.get('3').value = .3;
    const before = h.api.editGeneration;
    entry.pending.set('3', .42); entry.revision++;
    assert.equal(h.api.hasPendingEdits, true); assert(before === h.api.editGeneration);
    assert.equal(await h.api.flushEdits(), true); await tick();
    assert.equal(h.calls.filter(call => call.command === 'plugin.parameter').at(-1).payload.value, .42);
    assert.equal(h.calls.filter(call => call.command === 'plugin.state').length, 1);
    assert.deepEqual(h.undo, ['Adjust plug-in control']); assert.equal(h.project.audioPlugins[0].state, 'captured');
    assert.equal(h.api.hasPendingEdits, false);
    console.log('PASS edits flush, capture state and record one Undo step before saving');
  }
  {
    const h=harness(), owner='sound:kit-a:pad-1';
    const synth={...h.instrument,owner,instanceID:'kit-synth'};
    const effect={...h.instrument,kind:'effect',slot:0,owner,instanceID:'kit-fx'};
    const group={...h.nksEffect,owner:'bus:kit-group',instanceID:'kit-group-fx'};
    h.project.audioPlugins.push(synth,effect,group);
    h.window.STKits={controller:{view:()=>({active:true,ownerID:owner,selectedPluginID:'kit-synth'}),selectPlugin(){}}};
    const bound=h.api.controller.forOwner(owner,'kit-synth');
    assert.equal(bound.view().instanceID,'kit-synth');
    assert.deepEqual(Array.from(bound.view().slots.map(x=>x.instanceID)),['kit-synth','kit-fx']);
    assert.equal(bound.view().owner,owner);assert.equal(bound.view().slotIndex,0);
    bound.selectSlot('effect:0');assert.equal(bound.view().instanceID,'kit-fx');
    assert.equal(h.api.controller.forOwner('sound:missing:pad').view(),null,'no fallback to an unrelated track');
    assert.equal(h.api.controller.forOwner('bus:kit-group').view().nks,true);
    assert.equal(h.api.controller.forOwner('bus:kit-group').view().knobs[0].address,'4');
    assert.match(h.api.render(null,synth),/data-owner="sound:kit-a:pad-1"/);
    await tick();bound.adjust(3,0.1);bound.finish();await h.api.flushEdits();await tick();
    const target=h.calls.filter(x=>x.command==='plugin.parameter').at(-1).payload;
    assert.equal(target.owner,owner);assert.equal(target.instanceID,'kit-fx');assert.equal(target.track,undefined);
    assert.equal(h.project.audioPlugins.find(x=>x.instanceID==='kit-fx').owner,owner);
    console.log('PASS owner-bound Sound/Group pages, NKS mapping, chain navigation and captured gestures');
  }
})().catch(error => { console.error(error); process.exit(1); });
