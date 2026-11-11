'use strict';
// .nksfx presets load into one insert slot of the selected track; the instrument is untouched.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('nks-loader.js');
const copy = value => JSON.parse(JSON.stringify(value));
const descriptor = { component: 'vst3:46580000000000000000000000000000', owner: 'track:bass', kind: 'effect', slot: 1, instanceID: 'fx-a', state: 'fx-state', name: 'Delay', format: 'VST3', nks: { name: 'Tape Echo', pages: [] } };
function environment() {
  const project = { tracks: [{ instrument: 0, name: 'Bass', routingID: 'bass' }], instruments: [{ id: 0, name: 'Synth', type: 'plugin', plugin: { component: 'vst3:53594E54480000000000000000000000' } }],
    audioPlugins: [{ owner: 'track:bass', kind: 'instrument', slot: 0, component: 'vst3:53594E54480000000000000000000000', format: 'VST3' }, { owner: 'track:bass', kind: 'effect', slot: 0, component: 'au:comp', format: 'AU' }] };
  const calls = [], undo = [], notices = [];
  const state = { native: true, project, track: 0, instrument: 0, view: 'browser' };
  const app = { state, captureNativeEdits: async () => true, toast: text => notices.push(text), stopHeld() {}, setView: view => { state.view = view; },
    normalize: value => value, mutate: (label, fn) => { undo.push(label); fn(state.project); },
    request: async (command, payload) => { calls.push({ command, payload: copy(payload) }); return command === 'plugin.loadNKS' ? copy(descriptor) : {}; } };
  app.syncEngineProject = async make => { const value = make(); return value ? app.request('engine.project', { project: value }) : false; };
  const sandbox = { window: { ST: app } }; web.runText('nks-loader.js', source, sandbox);
  return { app, state, project, calls, undo, notices, loader: sandbox.window.STNKS };
}
(async () => {
  {
    const e = environment();
    assert.equal(await e.loader.load('/FX/Tape Echo.nksfx', { kind: 'effect', slot: 1 }), true);
    const request = e.calls.find(call => call.command === 'plugin.loadNKS');
    assert.deepEqual(request.payload, { path: '/FX/Tape Echo.nksfx', owner: 'track:bass', kind: 'effect', slot: 1 });
    assert.deepEqual(e.undo, ['load NKS effect']);
    assert.equal(e.project.audioPlugins.length, 3);
    assert.equal(e.project.audioPlugins.find(plugin => plugin.kind === 'effect' && plugin.slot === 1).instanceID, 'fx-a');
    assert.equal(e.project.audioPlugins.find(plugin => plugin.kind === 'effect' && plugin.slot === 1).nks.name, 'Tape Echo');
    assert.equal(e.project.tracks[0].plugins, undefined, 'the project keeps plug-ins in audioPlugins only');
    assert.equal(e.project.instruments[0].plugin.component, 'vst3:53594E54480000000000000000000000', 'the instrument is untouched');
    assert.equal(e.state.view, 'browser', 'effects load without leaving the browser');
    console.log('PASS NKS effect loads into the chosen insert slot with one Undo step');
  }
  {
    const e = environment();
    e.app.request = async (command, payload) => { e.calls.push({ command, payload }); if (command === 'plugin.loadNKS') { e.project.audioPlugins.push({ owner: 'track:bass', kind: 'effect', slot: 1, component: 'clap:com.example.other', format: 'CLAP' }); return copy(descriptor); } return {}; };
    assert.equal(await e.loader.load('/FX/Tape Echo.nksfx', { kind: 'effect', slot: 1 }), false);
    assert.equal(e.undo.length, 0); assert.equal(e.calls.at(-1).command, 'engine.project');
    console.log('PASS a slot replaced during loading is preserved and the engine is restored');
  }
  {
    const e = environment();
    e.app.request = async (command, payload) => { e.calls.push({ command, payload }); if (command === 'plugin.loadNKS') throw new Error('Unsupported'); return {}; };
    assert.equal(await e.loader.load('/FX/Tape Echo.nksfx', { kind: 'effect', slot: 0 }), false);
    assert.equal(e.undo.length, 0); assert(e.notices.includes('Unsupported'));
    assert.equal(await e.loader.load('/FX/x.nksfx', { kind: 'effect', slot: 9 }), false, 'slots are bounded');
    console.log('PASS failed and invalid effect loads leave the project unchanged');
  }
  {
    const e = environment(); e.state.view = 'browser';
    e.app.request = async (command, payload) => { e.calls.push({ command, payload }); return command === 'plugin.loadNKS' ? { ...copy(descriptor), kind: 'instrument', slot: 0 } : {}; };
    assert.equal(await e.loader.load('/Synth.nksf', { stay: true }), true);
    assert.equal(e.state.view, 'browser', 'stay keeps browsing after an instrument load');
    console.log('PASS instrument loads can keep the browser open');
  }
  {
    // Regression: a busy session (export, recording, routing, freeze) refuses the mutation.
    const e = environment(); e.app.mutate = () => false;
    assert.equal(await e.loader.load('/FX/Tape Echo.nksfx', { kind: 'effect', slot: 1 }), false);
    assert.equal(e.calls.at(-1).command, 'engine.project', 'the host returns to the unchanged session');
    assert(!e.notices.some(text => /^Loaded/.test(text)), 'no success notice for a refused effect');
    const instrument = environment(); instrument.app.mutate = () => false;
    assert.equal(await instrument.loader.load('/Synth.nksf'), false);
    assert.equal(instrument.calls.at(-1).command, 'engine.project');
    assert(!instrument.notices.some(text => /^Loaded/.test(text)), 'no success notice for a refused instrument');
    console.log('PASS a refused project edit restores the host instead of reporting a load');
  }
})().catch(error => { console.error(error); process.exit(1); });
