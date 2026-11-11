'use strict';
// A freeze (or unfreeze) that finishes after the track list changed must not land on the track that moved into its index.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');

function harness() {
  const nodes = new Map(), mutations = [], toasts = [];
  const node = id => {
    if (!nodes.has(id)) nodes.set(id, { id, value: id === 'st-freeze-tail' ? '10' : '', checked: true, hidden: false, disabled: false, textContent: '', focus() {} });
    return nodes.get(id);
  };
  let release;
  const project = complete({ tracks: [{ routingID: 'a', name: 'Drums', instrument: 0 }, { routingID: 'b', name: 'Bass', instrument: 0 }],
    patterns: [{ id: 0, length: 16, steps: [[], []] }], instruments: [{}], sequence: [0] });
  const state = { project, track: 0, playing: false, modal: null, native: false };
  const window = { ST: {
    state, escapeHTML: String, toast: message => toasts.push(message), perform() {}, waitForEngineUpdate: async () => {},
    openModal: (title, body, footer, options) => { state.modal = options.kind; }, closeModal: () => { state.modal = null; },
    request: command => new Promise(resolve => { release = () => resolve(command === 'freeze.track' ? { record: { id: 'print' } } : {}); }),
    mutate: (label, fn) => { mutations.push(label); fn(state.project); return true; }
  }, addEventListener() {} };
  web.run('freeze.js', { window, document: { addEventListener() {}, getElementById: node }, console, setInterval: () => 0, clearInterval() {} });
  return { window, state, project, mutations, toasts, node, release: () => release() };
}

const settle = () => new Promise(resolve => setImmediate(resolve));

(async () => {
  {
    const h = harness();
    h.window.STFreeze.freeze(0);
    const pending = h.node('st-freeze-go').onclick();
    await settle();
    h.project.tracks.splice(0, 1); // e.g. an AI control edit deletes the track while it prints
    h.release(); await pending;
    assert.equal(h.mutations.length, 0, 'the print is not applied');
    assert.equal(h.project.tracks[0].name, 'Bass');
    assert.equal(h.project.tracks[0].frozen, undefined, 'the track that shifted into index 0 is not marked frozen');
    assert.match(h.node('st-freeze-status').textContent, /session changed/);
  }
  {
    const h = harness();
    h.window.STFreeze.freeze(1);
    const pending = h.node('st-freeze-go').onclick();
    await settle();
    h.release(); await pending;
    assert.deepEqual(h.mutations, ['freeze Bass'], 'an undisturbed freeze still commits');
    assert.deepEqual(h.project.tracks[1].frozen, { id: 'print' });
  }
  {
    const h = harness();
    h.project.tracks[1].frozen = { id: 'print' };
    h.window.STFreeze.unfreeze(1);
    const pending = h.node('st-freeze-go').onclick();
    await settle();
    h.project.tracks.splice(0, 0, { routingID: 'c', name: 'New', instrument: 0 });
    h.release(); await pending;
    assert.equal(h.mutations.length, 0, 'an unfreeze after the track moved is not applied by position');
    assert.deepEqual(h.project.tracks[2].frozen, { id: 'print' });
  }
  console.log('PASS freeze and unfreeze results apply only to the track they were started for');
})().catch(error => { console.error(error); process.exitCode = 1; });
