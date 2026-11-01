const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const routing = require('../Resources/Web/routing-project.js');

{
  const project = { tracks: [{ id: 0 }, { id: 0 }, { routingID: 'saved' }] };
  routing.ensureTrackIDs(project, () => 'saved');
  assert.deepEqual(project.tracks.map(track => track.routingID), ['saved-1', 'saved-2', 'saved']);
  assert.equal(project.routing, undefined);
  const before = JSON.stringify(project);
  routing.ensureTrackIDs(project, () => { throw Error('must not regenerate identity'); });
  assert.equal(JSON.stringify(project), before);
  const duplicate = { tracks: [{ routingID: 'same' }, { routingID: 'same' }], routing: { version: 29, unknown: true } };
  routing.ensureTrackIDs(duplicate);
  assert.deepEqual(duplicate.tracks.map(track => track.routingID), ['same', 'same']);
  assert.deepEqual(duplicate.routing, { version: 29, unknown: true });
  console.log('PASS stable track identities survive normalization; ambiguous and future routing stays available for validation');
}
{
  const project = { tracks: [{ routingID: 'a' }], routing: { version: 1, outputs: { 'track:a': null } } };
  const track = routing.appendTrack(project, { id: 1 }, () => 'b');
  assert.equal(routing.trackOwner(track), 'track:b');
  assert.deepEqual(project.routing.outputs, { 'track:a': null, 'track:b': 'master' });
  console.log('PASS new tracks get their own master route without changing an explicitly disconnected channel');
}
{
  const project = {
    patterns: [{ automation: [{ owner: 'track:a', target: 'volume' }, { owner: 'track:b', target: 'send:incoming' }, { owner: 'bus:g', target: 'volume' }, { track: 1, target: 'cutoff' }] }],
    routing: {
      version: 1, outputs: { 'track:a': 'bus:g', 'track:b': 'master', 'bus:g': 'track:a' },
      sends: [{ id: 'gone', source: 'track:a', destination: 'bus:g' }, { id: 'incoming', source: 'track:b', destination: 'track:a' }, { id: 'keep', source: 'track:b', destination: 'bus:g' }],
      sidechains: [{ id: 'source', source: 'track:a', destination: { instanceID: 'other' } }, { id: 'insert', source: 'track:b', destination: { instanceID: 'removed' } }, { id: 'keep', source: 'track:b', destination: { instanceID: 'other' } }]
    }
  };
  routing.removeTrackReferences(project, { routingID: 'a' }, [{ instanceID: 'removed' }]);
  assert.deepEqual(project.routing.outputs, { 'track:b': 'master', 'bus:g': null });
  assert.deepEqual(project.routing.sends.map(item => item.id), ['keep']);
  assert.deepEqual(project.routing.sidechains.map(item => item.id), ['keep']);
  assert.deepEqual(project.patterns[0].automation, [{ owner: 'bus:g', target: 'volume' }, { track: 1, target: 'cutoff' }]);
  console.log('PASS explicit track deletion disconnects only its routes, sidechains and owner lanes');
}

function harness() {
  const nodes = new Map(), calls = [], timers = [];
  const element = id => {
    if (!nodes.has(id)) nodes.set(id, { id, innerHTML: '', value: '', dataset: {}, classList: { toggle() {} }, querySelectorAll: () => [], focus() {}, scrollIntoView() {}, setAttribute() {}, addEventListener() {}, closest: () => null });
    return nodes.get(id);
  };
  const document = { querySelector: element, querySelectorAll: () => [], getElementById: element, activeElement: null, body: element('body'), addEventListener() {} };
  const window = { STRoutingProject: routing, addEventListener() {}, webkit: { messageHandlers: { supertracker: { postMessage(message) { calls.push(message); queueMicrotask(() => window.nativeReceive({ id: message.id, ok: true, result: {} })); } } } } };
  const context = vm.createContext({ window, document, console, setTimeout: (fn, delay) => { timers.push({ fn, delay }); return timers.length; }, clearTimeout() {}, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise });
  web.load(context, 'app.js');
  const ST = window.ST;
  ST.state.project = ST.normalize(ST.demoProject());
  ST.state.project.routing = { version: 1, mixModel: 'standard', buses: [{ id: 'group', name: 'Drums' }], outputs: {}, sends: [{ id: 'echo', source: 'bus:group', gain: .35 }], sidechains: [] };
  window.STStudio = { target: (owner, target) => ({ value: target === 'send:echo' ? .35 : .8, available: true }), renderAutomation: () => '', bindAutomation() {}, event() {} };
  return { ST, window, calls, timers };
}

(async () => {
  {
    const { ST } = harness(), p = ST.state.project;
    p.tracks[0].audioInput = { enabled: true, deviceUID: 'interface-stable', channels: [2], gain: .8 };
    p.tracks[1].audioInput = { enabled: true, deviceUID: 'interface-stable', channels: [5, 3], gain: 1.2 };
    const saved = JSON.stringify(p), survivor = JSON.stringify(p.tracks[1].audioInput);
    assert.equal(JSON.stringify(ST.normalize(JSON.parse(saved))), saved, 'load normalization retains exact input identity and physical channel order');
    ST.perform('removeTrack');
    assert.equal(JSON.stringify(p.tracks[0].audioInput), survivor, 'removing another track never renumbers physical input channels');
    const removed = JSON.stringify(p);
    ST.perform('undo'); assert.equal(JSON.stringify(ST.state.project), saved);
    ST.perform('redo'); assert.equal(JSON.stringify(ST.state.project), removed);
    console.log('PASS saved line-input identities survive normalization, track removal and exact undo/redo');
  }
  {
    const { ST } = harness(), project = ST.state.project;
    const original = JSON.stringify(project), proposal = JSON.parse(original);
    const index = ST.appendInstrumentTrack(proposal, 1, 'New plug-in', 'new-plugin-track');
    assert.equal(index, project.tracks.length);
    assert.equal(proposal.tracks[index].instrument, 1);
    assert.equal(proposal.routing.outputs['track:new-plugin-track'], 'master');
    for (const pat of proposal.patterns) assert.deepEqual(pat.steps[index], Array(pat.length).fill(null));
    assert.equal(JSON.stringify(project), original);
    ST.perform('addTrack');
    assert.equal(project.tracks.length, index + 1);
    assert.equal(project.routing.outputs[routing.trackOwner(project.tracks[index])], 'master');
    ST.perform('undo'); assert.equal(JSON.stringify(ST.state.project), original);
    console.log('PASS added tracks use independent master routes and proposal preparation leaves the live session unchanged');
  }
  {
    const { ST } = harness(), project = ST.state.project;
    const owner = routing.trackOwner(project.tracks[0]), nextID = project.tracks[1].routingID;
    project.audioPlugins = [{ owner, kind: 'effect', slot: 0, instanceID: 'remove' }, { track: 1, kind: 'effect', slot: 0, instanceID: 'next' }, { owner: 'bus:group', kind: 'effect', slot: 0, instanceID: 'bus' }];
    project.routing.outputs[owner] = 'bus:group';
    project.routing.sidechains = [{ id: 'key', source: routing.trackOwner(project.tracks[1]), destination: { instanceID: 'remove' } }];
    const before = JSON.stringify(project);
    ST.perform('removeTrack');
    assert.deepEqual(project.audioPlugins.map(item => item.instanceID), ['next', 'bus']);
    assert.equal(project.audioPlugins[0].track, 0);
    assert.equal(project.tracks[0].routingID, nextID);
    assert.equal(project.routing.sidechains.length, 0);
    assert.equal(Object.hasOwn(project.routing.outputs, owner), false);
    ST.perform('undo');
    assert.equal(JSON.stringify(ST.state.project), before);
    console.log('PASS track removal and Undo preserve bus inserts and exact stable routing identities');
  }
  {
    const { ST } = harness(), p = ST.state.project;
    const removed = routing.trackOwner(p.tracks[0]), survivor = routing.trackOwner(p.tracks[1]);
    p.audioPlugins = [
      { owner: removed, track: 1, instanceID: 'gone' },
      { owner: survivor, track: 0, instanceID: 'survivor' },
      { owner: 'bus:group', track: 0, instanceID: 'bus' },
      { owner: 'sound:kit:kick', track: 0, instanceID: 'sound' }
    ];
    p.patterns[0].automation = [
      { owner: removed, track: 1, target: 'volume' },
      { owner: survivor, track: 0, target: 'pan' },
      { owner: 'bus:group', track: 0, target: 'volume' },
      { owner: 'sound:kit:kick', track: 0, target: 'volume' },
      { track: 0, target: 'cutoff' }, { track: 1, target: 'cutoff' }
    ];
    p.audioClips = [{ id: 'gone', track: 0 }, { id: 'keep', track: 1 }];
    p.routing.sidechains = [{ id: 'keep', source: survivor, destination: { instanceID: 'sound' } }];
    const assigned = p.tracks[1].instrument, before = JSON.stringify(p);
    ST.perform('removeTrack');
    assert.deepEqual(p.audioPlugins.map(x => x.instanceID), ['survivor', 'bus', 'sound']);
    assert.equal(p.patterns[0].automation.length, 4);
    assert.equal(p.patterns[0].automation.at(-1).track, 0);
    assert.equal(p.audioClips.length, 1); assert.equal(p.audioClips[0].track, 0);
    assert.equal(p.tracks[0].instrument, assigned);
    assert.equal(p.routing.sidechains.length, 1);
    const after = JSON.stringify(p);
    ST.perform('undo'); assert.equal(JSON.stringify(ST.state.project), before);
    ST.perform('redo'); assert.equal(JSON.stringify(ST.state.project), after);
    assert.equal(JSON.stringify(ST.normalize(JSON.parse(after))).includes('sound:kit:kick'), true);
    console.log('PASS removal honors explicit owners over stale indices and keeps clips, instruments, sidechains and undo/redo');

  }
  {
    const { ST } = harness();
    ST.state.track = 3;
    ST.automate('bus:group', 'volume', 'Drums volume');
    let lanes = ST.pattern().automation;
    assert.equal(lanes.at(-1).owner, 'bus:group');
    assert.equal(lanes.at(-1).track, undefined);
    assert.equal(ST.state.track, 3);
    assert.equal(ST.state.mixerOwner, 'bus:group');
    const count = lanes.length;
    ST.automate('bus:group', 'volume', 'Drums volume');
    assert.equal(lanes.length, count);
    ST.automate('bus:group', 'send:echo', 'Echo send');
    assert.equal(lanes.at(-1).points[0].value, .35);
    const previous = lanes.length;
    ST.automate('bus:missing', 'volume', 'Missing');
    assert.equal(lanes.length, previous);
    ST.automate('master', 'volume', 'Master volume');
    assert.equal(lanes.at(-1).owner, 'master');
    assert.equal(ST.state.track, 3);
    console.log('PASS routed automation uses persistent owners, reuses lanes and leaves note-track selection valid');
  }
  {
    const { ST, window, calls } = harness();
    let complete, applying = true;
    const pending = new Promise(resolve => { complete = () => { applying = false; resolve(); }; });
    window.STRouting = { get isApplying() { return applying; }, waitForApply: () => pending };
    const previous = ST.state.project.name;
    ST.mutate('conflicting edit', project => { project.name = 'incorrect'; });
    assert.equal(ST.state.project.name, previous);
    const sync = ST.syncEngine(true);
    for (let i = 0; i < 6; i++) await Promise.resolve();
    assert.equal(calls.some(call => call.command === 'engine.project'), false);
    ST.mutate('commit routing', project => { project.routing.outputs['bus:group'] = 'master'; }, { render: false, routingCommit: true });
    complete(); await sync;
    assert.equal(calls.find(call => call.command === 'engine.project').payload.project.routing.outputs['bus:group'], 'master');
    console.log('PASS routing transactions block conflicting edits and engine sync waits for committed routes');
  }
})().catch(error => { console.error(error); process.exitCode = 1; });
