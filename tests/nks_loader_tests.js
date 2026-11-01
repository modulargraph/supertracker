const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('nks-loader.js');
const copy = value => JSON.parse(JSON.stringify(value));
const descriptor = { component: 'vst3:TEST', track: 0, kind: 'instrument', slot: 0,
  state: 'restorable-state', name: 'Test synth', format: 'VST3', nks: { name: 'Mapped patch', pages: [] } };
function environment() {
  const project = { tracks: [{ instrument: 0 }], instruments: [{ id: 0, name: 'Previous', type: 'synth' }],
    patterns: [{ steps: [[{ note: 60, velocity: 97 }]] }], audioPlugins: [] };
  const calls = [], undo = [], notices = [];
  const state = { native: true, project, track: 0, instrument: 0 };
  const app = { state, captureNativeEdits: async () => true, syncEngine: async () => {},
    toast: text => notices.push(text), stopHeld: () => {}, setView: view => { state.view = view; },
    normalize: value => value,
    mutate: (label, fn) => { undo.push(copy(state.project)); fn(state.project); },
    request: async (command, payload) => { calls.push({ command, payload }); return copy(descriptor); } };
  app.syncEngineProject=async(makeProject,options={})=>{const project=makeProject();return project?app.request('engine.project',{project,awaitPlugins:true,...(options.replace?{replace:true}:{})}):false;};
  const sandbox = { window: { ST: app } }; web.runText('nks-loader.js', source, sandbox);
  return { app, state, project, calls, undo, notices, loader: sandbox.window.STNKS };
}
async function main() {
  {
    const e = environment();
    assert.equal(await e.loader.load('/Library/Fixture.nksf'), true);
    assert.equal(e.undo.length, 1); assert.equal(e.undo[0].instruments[0].type, 'synth');
    assert.equal(e.project.instruments[0].name, 'Previous');
    assert.equal(e.project.instruments[1].name, 'Mapped patch');
    assert.equal(e.project.audioPlugins[0].state, 'restorable-state');
    assert.equal(e.project.patterns[0].steps[0][0].velocity, 97);
    assert.equal(e.state.view, 'instrument');
    console.log('PASS NKS load captures one undo and preserves pattern edits with restorable plug-in state');
  }
  {
    const e = environment();
    e.app.captureNativeEdits = async () => false;
    assert.equal(await e.loader.load('/Library/Fixture.nksf'), false);
    assert.equal(e.calls.length, 0); assert.equal(e.undo.length, 0);
    console.log('PASS NKS load stops when existing native plug-in edits cannot be secured');
  }
  {
    const e = environment(); let resolve;
    e.app.request = async (command, payload) => {
      e.calls.push({ command, payload });
      if (command === 'plugin.loadNKS') return new Promise(done => { resolve = done; });
      return {};
    };
    const result = e.loader.load('/Library/Fixture.nksf');
    while (!resolve) await Promise.resolve();
    assert.equal(await e.loader.load('/Library/Other.nksf'), false);
    e.state.project = { tracks: [{ instrument: 0 }], instruments: [{ name: 'New project' }], audioPlugins: [] };
    resolve(copy(descriptor));
    assert.equal(await result, false); assert.equal(e.undo.length, 0);
    assert.equal(e.calls.at(-1).command, 'engine.project');
    assert.equal(e.calls.at(-1).payload.project.instruments[0].name, 'New project');
    console.log('PASS concurrent loads are bounded and a changed project is restored without stale mutation');
  }
  {
    const e = environment();
    e.app.request = async (command, payload) => {
      e.calls.push({ command, payload });
      if (command === 'plugin.loadNKS') {
        e.project.audioPlugins.push({ track: 0, kind: 'instrument', component: 'Changed while loading' });
        return copy(descriptor);
      }
      return {};
    };
    assert.equal(await e.loader.load('/Library/Fixture.nksf'), false);
    assert.equal(e.project.audioPlugins[0].component, 'Changed while loading'); assert.equal(e.undo.length, 0);
    console.log('PASS replacement of the same slot during loading is preserved');
  }
  for (const fails of [true, false]) {
    const e = environment(), before = copy(e.project);
    e.app.request = async (command, payload) => {
      e.calls.push({ command, payload });
      if (command === 'plugin.loadNKS') { if (fails) throw new Error('Unsupported preset'); return { name: 'Invalid response' }; }
      return {};
    };
    assert.equal(await e.loader.load('/Library/Fixture.nksf'), false);
    assert.deepEqual(e.project, before); assert.equal(e.undo.length, 0);
    assert.equal(e.calls.at(-1).command, 'engine.project');
  }
  console.log('PASS failed or invalid native loads restore the current session');
  {
    for (const change of [e => { e.project.instruments[0].type = 'midi'; },
      e => { e.project.tracks[0].instrument = 4; },
      e => { e.project.instruments[0].cutoff = 1200; }]) {
      const e = environment(); let edited;
      e.app.request = async (command, payload) => {
        e.calls.push({ command, payload });
        if (command === 'plugin.loadNKS') { change(e); edited = copy(e.project); return copy(descriptor); }
        return {};
      };
      assert.equal(await e.loader.load('/Library/Fixture.nksf'), false);
      assert.deepEqual(e.project, edited); assert.equal(e.undo.length, 0);
      assert.equal(e.calls.at(-1).command, 'engine.project');
    }
    console.log('PASS pending NKS loads preserve instrument edits and track reassignment');
  }
  {
    const e = environment();
    assert.equal(await e.loader.load('/Library/Fixture.nksf', { newInstrument: true }), true);
    assert.equal(e.project.instruments.length, 2); assert.equal(e.project.instruments[0].type, 'synth');
    assert.equal(e.project.tracks[0].instrument, 1); assert.equal(e.undo.length, 1);
    const full = environment(); full.project.instruments = Array.from({ length: 64 }, (_, id) => ({ id, type: 'sample' }));
    assert.equal(await full.loader.load('/Library/Fixture.nksf', { newInstrument: true }), true);
    assert.equal(full.project.instruments.length, 65, 'no 64-instrument cap');
    console.log('PASS new-instrument loading is atomic and has no instrument cap');
  }
  {
    const e = environment();
    e.project.tracks.push({ instrument: 1 });
    e.project.instruments.push({ id: 1, name: 'Other instrument', type: 'synth' });
    e.app.request = async (command, payload) => {
      e.calls.push({ command, payload });
      if (command === 'plugin.loadNKS') {
        e.state.track = 1; e.state.instrument = 1; e.state.view = 'pattern';
        return copy(descriptor);
      }
      return {};
    };
    assert.equal(await e.loader.load('/Library/Fixture.nksf', { newInstrument: true }), true);
    assert.equal(e.project.tracks[0].instrument, 2);
    assert.equal(e.project.tracks[1].instrument, 1);
    assert.equal(e.state.track, 1); assert.equal(e.state.instrument, 1); assert.equal(e.state.view, 'pattern');
    console.log('PASS asynchronous NKS completion preserves navigation to another track and workspace');
  }
  {
    const e = environment(); e.state.native = false;
    assert.equal(await e.loader.load('/Library/Fixture.nksf'), false); assert.equal(e.calls.length, 0);
    console.log('PASS browser preview never pretends to load a native NKS instrument');
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
