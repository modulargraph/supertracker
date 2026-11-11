'use strict';
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const file = path.join(__dirname, '../ui/track-duplicate-project.js');
const duplicate = require(file), copy = value => JSON.parse(JSON.stringify(value));
const tests = [], test = (name, run) => tests.push({ name, run });
globalThis.TRACK_SANITY_LIMIT = 1024; // app.js defines it in the app
const maxAddress = '18446744073709551615';
function fixture() {
  return {
    name: 'Independent takes', bpm: 113, activePattern: 0, sequence: [0, 1],
    tracks: [
      { id: 0, routingID: 'source', name: 'Juno', instrument: 0, volume: .73, pan: -.4, mute: true, solo: true, delaySend: .2, reverbSend: .3 },
      { id: 1, routingID: 'other', name: 'Shared Juno', instrument: 0, volume: .9, mute: false }
    ],
    instruments: [
      { id: 0, name: 'Juno patch', type: 'au', rootNote: 60 },
      { id: 1, name: 'Slice', type: 'sample', samplePath: '/samples/chord.wav', slices: [.2, .4], zones: [{ samplePath: '/samples/zone.wav', lo: 20, hi: 127 }] },
      { id: 2, name: 'Unused', type: 'midi', midiChannel: 7 }
    ],
    audioPlugins: [
      { instanceID: 'instrument', owner: 'track:source', kind: 'instrument', slot: 0, component: 'AU:synth', state: 'opaque:instrument:track:source', nks: { instanceID: 'instrument', source: 'track:source' } },
      { instanceID: 'fx-six', owner: 'track:source', kind: 'effect', slot: 6, component: 'AU:echo', state: { instanceID: 'fx-six', nested: { owner: 'track:source' } }, bypass: true },
      { instanceID: 'fx-two', owner: 'track:source', kind: 'effect', slot: 2, component: 'VST3:filter', state: 'opaque-filter' },
      { instanceID: 'master', owner: 'master', kind: 'effect', slot: 0, component: 'AU:master' },
      { instanceID: 'other-fx', owner: 'track:other', kind: 'effect', slot: 0, component: 'AU:other' },
      { instanceID: 'bus-fx', owner: 'bus:group', kind: 'effect', slot: 0, component: 'AU:group' },
      { instanceID: 'master-two', owner: 'master', kind: 'effect', slot: 3, component: 'AU:master-two' }
    ],
    patterns: [
      { id: 0, length: 2, steps: [
        [{ note: 60, instrument: 0, chord: [0, 4, 7], fx1: { type: 'plugin', instanceID: 'instrument', address: maxAddress, value: .73 }, fx2: { type: 'plugin', instanceID: 'other-fx', address: '42', value: .23 } }, { note: 64, instrument: 1, fx1: { type: 'slice', value: 3 }, fx2: { type: 'plugin', instanceID: 'fx-six', address: maxAddress, value: .42 } }],
        [{ note: 72, instrument: 0 }, null]
      ], automation: [
        { id: 'lane-effect', owner: 'track:source', target: `plugin:effect:6:${maxAddress}`, points: [{ row: 0, value: .5 }] },
        { id: 'lane-send', owner: 'track:source', target: 'send:send-source', interpolation: 'step', points: [{ row: 1, value: .3 }] },
        { id: 'lane-mix', track: 0, target: 'volume', points: [{ row: 0, value: .7 }] },
        { id: 'lane-other', track: 1, target: 'volume', points: [] },
        { id: 'lane-master', owner: 'master', target: 'volume', points: [] },
        { id: 'lane-bus', owner: 'bus:group', target: 'pan', points: [] },
        { id: 'lane-dormant', owner: 'track:unavailable', target: 'send:unavailable', points: [] }
      ] },
      { id: 1, length: 3, steps: [[{ note: 48, instrument: 99 }, { note: 50, instrument: -1, fx1: { type: 'plugin', instanceID: 'missing-instance', address: '7', value: .8 } }, null], [null, null, null]], automation: [] }
    ],
    audioClips: [{ id: 'clip-source', track: 0, path: '/takes/synth.wav', startBeat: 4, durationBeats: 8, gain: .4, fadeIn: .2, muted: true }, { id: 'clip-other', track: 1, path: '/takes/other.wav' }],
    routing: {
      buses: [{ id: 'group', type: 'group', name: 'Synths' }, { id: 'return', type: 'return', name: 'Room' }],
      outputs: { 'track:source': 'bus:group', 'track:other': 'track:source', 'bus:group': 'master', 'bus:return': 'master' },
      sends: [{ id: 'send-source', source: 'track:source', destination: 'bus:return', tap: 'preFader', gain: .6, enabled: true }, { id: 'send-other', source: 'track:other', destination: 'track:source', gain: .8 }],
      sidechains: [
        { id: 'self-key', source: 'track:source', tap: 'postFader', destination: 'fx-six', gain: .4 },
        { id: 'external-key', source: 'track:other', tap: 'preInserts', destination: 'fx-two' },
        { id: 'missing-key', source: 'bus:disconnected', destination: 'fx-six' },
        { id: 'outgoing-key', source: 'track:source', destination: 'other-fx' }
      ]
    }
  };
}
function build(project = fixture(), makeID) { let id = 0; return duplicate.build(project, 0, makeID || (() => `new-${++id}`)); }
function freeze(value) { if (value && typeof value === 'object') { Object.values(value).forEach(freeze); Object.freeze(value); } return value; }
test('the helper is available as a browser global and a CommonJS module', () => {
  const context = vm.createContext({ window: {} }); web.load(context, 'track-duplicate-project.js');
  assert.equal(typeof duplicate.build, 'function'); assert.equal(typeof context.window.STTrackDuplicateProject.build, 'function');
});
test('the new track preserves mixer attributes and independently clones every valid referenced instrument', () => {
  const original = fixture(), result = build(original), project = result.project, track = project.tracks[2];
  assert.equal(result.trackIndex, 2); assert.equal(result.sourceTrackIndex, 0); assert.deepEqual(result.instrumentMap, { 0: 3, 1: 4 });
  assert.equal(track.name, 'Juno copy'); assert.equal(track.id, 2); assert.equal(track.instrument, 3); assert.notEqual(track.routingID, original.tracks[0].routingID);
  for (const key of ['volume', 'pan', 'mute', 'solo', 'delaySend', 'reverbSend']) assert.equal(track[key], original.tracks[0][key]);
  assert.equal(project.instruments.length, 5); assert.deepEqual(project.instruments.slice(0, 3), original.instruments); assert.equal(project.tracks[1].instrument, 0);
  assert.equal(project.patterns[0].steps[2][0].instrument, 3); assert.equal(project.patterns[0].steps[2][1].instrument, 4);
  assert.equal(project.patterns[1].steps[2][0].instrument, 99); assert.equal(project.patterns[1].steps[2][1].instrument, -1); assert.equal(project.patterns[1].steps[2][2], null);
});
test('the plug-ins of the source channel are copied to the new channel with sparse effect slots and exact opaque state', () => {
  const original = fixture(), result = build(original), copied = result.project.audioPlugins.slice(original.audioPlugins.length);
  assert.deepEqual(copied.map(plugin => [plugin.kind, plugin.slot]), [['instrument', 0], ['effect', 6], ['effect', 2]]);
  assert.deepEqual(Object.keys(result.instanceMap), ['instrument', 'fx-six', 'fx-two']);
  assert.deepEqual(result.newPluginInstanceIDs, copied.map(plugin => plugin.instanceID));
  for (let i = 0; i < copied.length; i++) {
    assert.equal(copied[i].instanceID, result.instanceMap[original.audioPlugins[i].instanceID]);
    assert.deepEqual(copied[i].state, original.audioPlugins[i].state); assert.deepEqual(copied[i].nks, original.audioPlugins[i].nks);
    assert.equal(copied[i].owner, `track:${result.project.tracks[2].routingID}`); assert.equal(Object.hasOwn(copied[i], 'track'), false);
  }
  assert.equal(copied[1].bypass, true); assert.deepEqual(result.project.audioPlugins.slice(0, original.audioPlugins.length), original.audioPlugins);
});
test('the copied instrument plug-in is a descriptor of the new track: only its identity changes, and no instrument carries a copy', () => {
  const original = fixture(), result = build(original), source = original.audioPlugins[0];
  const copied = result.project.audioPlugins.find(plugin => plugin.instanceID === result.instanceMap.instrument);
  assert.equal(copied.owner, `track:${result.project.tracks[2].routingID}`); assert.equal(copied.kind, 'instrument');
  assert.deepEqual(copied.state, source.state); assert.deepEqual(copied.nks, source.nks);
  assert(result.project.instruments.every(instrument => !Object.hasOwn(instrument, 'plugin')));
});
test('both step FX remap copied plug-in identities without changing exact addresses or unrelated targets', () => {
  const original = fixture(), result = build(original), steps = result.project.patterns[0].steps[2];
  assert.deepEqual(steps[0].fx1, { ...original.patterns[0].steps[0][0].fx1, instanceID: result.instanceMap.instrument });
  assert.deepEqual(steps[1].fx2, { ...original.patterns[0].steps[0][1].fx2, instanceID: result.instanceMap['fx-six'] });
  assert.equal(steps[0].fx1.address, maxAddress); assert.equal(typeof steps[0].fx1.address, 'string');
  assert.deepEqual(steps[0].fx2, original.patterns[0].steps[0][0].fx2); assert.deepEqual(steps[1].fx1, original.patterns[0].steps[0][1].fx1);
  assert.deepEqual(result.project.patterns[1].steps[2][1].fx1, original.patterns[1].steps[0][1].fx1);
});
test('routing preserves shared destinations and incoming connections while remapping only copied sends and incoming plug-in sidechains', () => {
  const original = fixture(), result = build(original), routing = result.project.routing, newOwner = `track:${result.project.tracks[2].routingID}`;
  assert.deepEqual(routing.outputs, { ...original.routing.outputs, [newOwner]: 'bus:group' }); assert.deepEqual(routing.buses, original.routing.buses);
  assert.equal(routing.sends.length, 3); assert.deepEqual(routing.sends.slice(0, 2), original.routing.sends); assert.deepEqual(routing.sends[2], { ...original.routing.sends[0], id: routing.sends[2].id, source: newOwner }); assert.notEqual(routing.sends[2].id, 'send-source');
  assert.equal(routing.sidechains.length, 7); assert.deepEqual(routing.sidechains.slice(0, 4), original.routing.sidechains);
  assert.deepEqual(routing.sidechains.slice(4).map(route => route.source), [newOwner, 'track:other', 'bus:disconnected']);
  assert.deepEqual(routing.sidechains.slice(4).map(route => route.destination), [result.instanceMap['fx-six'], result.instanceMap['fx-two'], result.instanceMap['fx-six']]);
  for (let i = 0; i < 3; i++) { assert.notEqual(routing.sidechains[4 + i].id, original.routing.sidechains[i].id); assert.equal(routing.sidechains[4 + i].tap, original.routing.sidechains[i].tap); }
});
test('copied automation has fresh IDs and correct owner, track and send identity without altering unrelated lanes', () => {
  const original = fixture(), result = build(original), lanes = result.project.patterns[0].automation, copied = lanes.slice(original.patterns[0].automation.length), owner = `track:${result.project.tracks[2].routingID}`;
  assert.equal(copied.length, 3); assert.deepEqual(lanes.slice(0, 7), original.patterns[0].automation);
  // Plug-in and send lanes name the new channel; the lane of the track's own volume names the new track's index.
  assert.equal(copied[0].owner, owner); assert.equal(copied[1].owner, owner); assert.equal(Object.hasOwn(copied[0], 'track') || Object.hasOwn(copied[1], 'track'), false);
  assert.equal(copied[2].track, 2); assert.equal(Object.hasOwn(copied[2], 'owner'), false);
  assert.equal(copied[0].target, `plugin:effect:6:${maxAddress}`); assert.equal(copied[1].target, `send:${result.project.routing.sends[2].id}`); assert.equal(copied[2].target, 'volume');
  copied.forEach((lane, i) => { assert.notEqual(lane.id, original.patterns[0].automation[i].id); assert.deepEqual(lane.points, original.patterns[0].automation[i].points); });
});
test('audio clips are independent copies with new IDs and unchanged paths, timing, fades and mute', () => {
  const original = fixture(), result = build(original), clips = result.project.audioClips;
  assert.equal(clips.length, 3); assert.deepEqual(clips.slice(0, 2), original.audioClips); assert.notEqual(clips[2].id, original.audioClips[0].id);
  assert.deepEqual(clips[2], { ...original.audioClips[0], id: clips[2].id, track: 2 });
});
test('a frozen input remains unchanged and changing any duplicated nested data cannot alter its source or other tracks', () => {
  const original = fixture(), before = copy(original), result = build(freeze(original)), p = result.project;
  assert.deepEqual(original, before); assert.deepEqual(copy(result), result);
  p.instruments[3].rootNote = 61; p.instruments[4].zones[0].lo = 80; p.patterns[0].steps[2][0].chord[0] = 12;
  p.patterns[0].automation[7].points[0].value = .1; p.routing.sidechains[4].gain = .5; p.audioClips[2].path = '/edited.wav';
  assert.deepEqual(p.instruments.slice(0, 3), before.instruments); assert.deepEqual(p.patterns[0].steps.slice(0, 2), before.patterns[0].steps); assert.deepEqual(p.patterns[0].automation.slice(0, 7), before.patterns[0].automation);
  assert.deepEqual(p.routing.sidechains.slice(0, 4), before.routing.sidechains); assert.deepEqual(p.audioClips.slice(0, 2), before.audioClips); assert.deepEqual(original, before);
});
test('repeated, missing, invalid and throwing ID generators terminate with globally unique fresh identifiers', () => {
  for (const makeID of [() => 'fx-six', () => '', () => 'invalid:identifier', () => { throw new Error('Unavailable UUID generator'); }]) {
    const original = fixture(); original.instruments[2].metadata = { id: 'track-copy-1', instanceID: 'track-copy-2' };
    let calls = 0; const result = build(original, () => { calls++; return makeID(); }), p = result.project;
    const ids = [p.tracks[2].routingID, ...Object.values(result.instanceMap), ...p.patterns[0].automation.slice(7).map(lane => lane.id), p.audioClips[2].id, p.routing.sends[2].id, ...p.routing.sidechains.slice(4).map(route => route.id)];
    assert.equal(new Set(ids).size, ids.length); assert(calls <= ids.length); assert(ids.every(id => /^[A-Za-z0-9_.-]{1,128}$/.test(id)));
    assert(!ids.includes('track-copy-1')); assert(!ids.includes('track-copy-2')); assert(!ids.includes('fx-six'));
  }
});
test('inactive descriptors do not become active plug-ins when the authoritative array is empty or absent', () => {
  for (const absent of [false, true]) {
    const project = fixture(); project.routing = {}; project.audioPlugins = []; if (absent) delete project.audioPlugins;
    for (const pattern of project.patterns) pattern.automation = pattern.automation.filter(lane => !String(lane.target).startsWith('send:'));
    const result = build(project);
    assert.deepEqual(result.instanceMap, {});
    assert.equal(Object.hasOwn(result.project, 'audioPlugins'), !absent); if (!absent) assert.deepEqual(result.project.audioPlugins, []);
    assert.deepEqual(Object.keys(result.project.routing), ['outputs'], 'collections the table left out are not invented');
  }
});
test('new identity allocation cannot revive unavailable owner, send or plug-in references', () => {
  for (const candidate of ['unavailable', 'disconnected', 'missing-instance', 'missing-send', 'orphan-output', '0']) {
    const project = fixture(); project.patterns[0].automation.push({ owner: 'track:source', target: 'send:missing-send', points: [] }); project.routing.outputs['track:orphan-output'] = null;
    const result = build(project, () => candidate);
    assert.notEqual(result.project.tracks[2].routingID, candidate); assert(!result.newPluginInstanceIDs.includes(candidate));
    assert.equal(result.project.patterns[0].automation.at(-1).target, 'send:missing-send');
  }
});
test('every copied plug-in is reported even when its original descriptor has no explicit identity', () => {
  const project = fixture(); delete project.audioPlugins[1].instanceID; project.routing.sidechains = []; project.patterns[0].automation[0].instanceID = 'fx-two';
  const result = build(project), copied = result.project.audioPlugins.slice(project.audioPlugins.length);
  assert.equal(result.newPluginInstanceIDs.length, 3); assert.deepEqual(result.newPluginInstanceIDs, copied.map(plugin => plugin.instanceID));
  assert.equal(Object.hasOwn(result.instanceMap, 'fx-six'), false);
  assert.equal(result.project.patterns[0].steps[2][1].fx2.instanceID, 'fx-six');
  assert.equal(result.project.patterns[0].automation[7].instanceID, result.instanceMap['fx-two']);
});
test('disconnected output is copied exactly and omitted output uses the existing default Master destination', () => {
  for (const destination of [null, undefined]) {
    const project = fixture(); if (destination === undefined) delete project.routing.outputs['track:source']; else project.routing.outputs['track:source'] = destination;
    const result = build(project), owner = `track:${result.project.tracks[2].routingID}`;
    assert.equal(result.project.routing.outputs[owner], destination === undefined ? 'master' : null);
  }
});
test('selection of a later source track appends at the end and follows authoritative ownership', () => {
  const original = fixture(), result = duplicate.build(original, 1, () => 'new-later-track'), project = result.project;
  assert.equal(result.sourceTrackIndex, 1); assert.equal(result.trackIndex, 2); assert.equal(project.tracks[2].name, 'Shared Juno copy'); assert.deepEqual(result.instrumentMap, { 0: 3 });
  assert.deepEqual(Object.keys(result.instanceMap), ['other-fx']); assert.equal(project.audioPlugins.at(-1).owner, `track:${project.tracks[2].routingID}`);
  assert.deepEqual(project.patterns[0].steps[2], [{ note: 72, instrument: 3 }, null]);
  assert.equal(project.patterns[0].automation.length, original.patterns[0].automation.length + 1); assert.equal(project.patterns[0].automation.at(-1).track, 2);
  assert.equal(project.routing.outputs[`track:${project.tracks[2].routingID}`], 'track:source');
  assert.equal(project.routing.sidechains.at(-1).source, 'track:source'); assert.equal(project.routing.sidechains.at(-1).destination, result.instanceMap['other-fx']);
});
test('lanes addressed by track index stay numeric, inactive metadata is kept and short columns are padded safely', () => {
  const original = { tracks: [{ routingID: 'plain', instrument: 0, name: 'Plain' }], instruments: [{ id: 0, type: 'sample', metadata: { id: 'unused' } }], patterns: [{ id: 0, length: 4, steps: [[{ note: 60, instrument: 0 }]], automation: [{ track: 0, target: 'pan', points: [] }] }],
    routing: {} };
  const result = build(original), project = result.project;
  assert.equal(project.tracks.length, 2); assert.equal(project.tracks[0].routingID, 'plain'); assert.equal(typeof project.tracks[1].routingID, 'string'); assert.notEqual(project.tracks[1].routingID, 'plain');
  assert.deepEqual(project.patterns[0].steps[1], [{ note: 60, instrument: 1 }, null, null, null]); assert.equal(project.patterns[0].steps[0].length, 1);
  assert.equal(project.patterns[0].automation[1].track, 1); assert.equal(Object.hasOwn(project.patterns[0].automation[1], 'owner'), false);
  assert.equal(project.routing.outputs[`track:${project.tracks[1].routingID}`], 'master'); assert.equal(Object.hasOwn(project, 'audioPlugins'), false); assert.equal(Object.hasOwn(project, 'audioClips'), false);
  assert.deepEqual(project.instruments[1].metadata, original.instruments[0].metadata);
});
test('capacity checks reject a proposal before mutating the input', () => {
  for (const kind of ['tracks', 'clips', 'routes']) {
    const project = fixture();
    if (kind === 'tracks') while (project.tracks.length < globalThis.TRACK_SANITY_LIMIT) project.tracks.push({ id: project.tracks.length, routingID: `extra-${project.tracks.length}`, instrument: 0 });
    if (kind === 'clips') while (project.audioClips.length < 256) project.audioClips.push({ id: `extra-${project.audioClips.length}`, track: 1 });
    if (kind === 'routes') while (project.routing.sends.length + project.routing.sidechains.length < 125) project.routing.sends.push({ id: `extra-${project.routing.sends.length}`, source: 'track:other', destination: 'master' });
    const before = copy(project); assert.throws(() => build(project), /1024|64|256|128/); assert.deepEqual(project, before);
  }
});
test('tracks have no fixed limit: duplicating beyond sixteen tracks succeeds', () => {
  const project = fixture(); while (project.tracks.length < 40) project.tracks.push({ id: project.tracks.length, routingID: `extra-${project.tracks.length}`, instrument: 2 });
  for (const pattern of project.patterns) while (pattern.steps.length < project.tracks.length) pattern.steps.push(Array(pattern.length).fill(null));
  const result = build(project); assert.equal(result.trackIndex, 40); assert.equal(result.project.tracks.length, 41); assert.equal(project.tracks.length, 40);
});
test('exact capacities are accepted when all copied content fits', () => {
  const project = fixture(); while (project.instruments.length < 62) project.instruments.push({ id: project.instruments.length });
  while (project.audioClips.length < 255) project.audioClips.push({ id: `extra-clip-${project.audioClips.length}`, track: 1 });
  while (project.routing.sends.length + project.routing.sidechains.length < 124) project.routing.sends.push({ id: `extra-send-${project.routing.sends.length}`, source: 'track:other', destination: 'master' });
  const result = build(project); assert.equal(result.project.instruments.length, 64); assert.equal(result.project.audioClips.length, 256); assert.equal(result.project.routing.sends.length + result.project.routing.sidechains.length, 128);
});
test('malformed sources, unsupported routing and ambiguous active identities fail without modification', () => {
  const changes = [
    p => { p.tracks[0].instrument = 99; }, p => { p.tracks[0].instrument = -1; }, p => { p.tracks[0] = null; },
    p => { p.routing.outputs = []; }, p => { p.tracks[1].routingID = 'source'; },
    p => { p.audioPlugins[1].instanceID = 'instrument'; }, p => { p.audioPlugins[1].owner = 'bus:absent'; }, p => { p.audioPlugins[1].owner = ''; },
    p => { p.audioPlugins[2].slot = 6; }, p => { delete p.audioPlugins[1].owner; }, p => { p.routing.sends[1].id = 'self-key'; },
    p => { p.patterns[0].steps[0][0] = 42; }, p => { p.patterns[0].length = 0; }, p => { p.patterns[0].steps[0] = {}; }
  ];
  for (const change of changes) { const project = fixture(); change(project); const before = copy(project); assert.throws(() => build(project)); assert.deepEqual(copy(project), before); }
  assert.throws(() => duplicate.build(fixture(), -1)); assert.throws(() => duplicate.build(fixture(), 5));
  const functions = fixture(); functions.run = () => {}; assert.throws(() => build(functions), /JSON/);
});
test('a duplicated group member keeps its group and group-bus output', () => {
  const p = fixture();
  p.groups = [{ id: 'g', name: 'Kit', busID: 'group' }];
  p.tracks[0].groupID = 'g';
  p.routing.outputs[`track:${p.tracks[0].routingID}`] = 'bus:group';
  const result = build(p).project, copied = result.tracks.at(-1);
  assert.equal(copied.groupID, 'g');
  assert.equal(result.routing.outputs[`track:${copied.routingID}`], 'bus:group');
});
test('duplicate preserves the exact physical input mapping with independently editable channels', () => {
  const p = fixture(); p.tracks[0].audioInput = { enabled: true, deviceUID: 'interface:stable-uid', channels: [4, 7], gain: .65 };
  const before = copy(p), result = build(p), cloned = result.project.tracks.at(-1).audioInput;
  assert.deepEqual(cloned, p.tracks[0].audioInput);
  cloned.channels[0] = 2; cloned.gain = 1;
  assert.deepEqual(result.project.tracks[0].audioInput, before.tracks[0].audioInput);
  assert.deepEqual(p, before);
});
test('a whole group duplicates its members and a new bus with inserts, sends, sidechains and automation', () => {
  const p = fixture();
  p.groups = [{ id: 'g', name: 'Kit', busID: 'group', color: '#a0a', libraryPath: '/kits/kit.stkit' }];
  p.tracks.forEach(track => { track.groupID = 'g'; p.routing.outputs[`track:${track.routingID}`] = 'bus:group'; });
  if (!p.routing.buses.some(bus => bus.id === 'group')) p.routing.buses.push({ id: 'group', name: 'Kit', type: 'group', groupID: 'g' });
  p.routing.buses.find(bus => bus.id === 'group').volume = .6;
  p.routing.sends.push({ id: 'bus-send', source: 'bus:group', destination: 'master', level: .4 });
  p.routing.sidechains.push({ id: 'bus-key', source: 'track:source', destination: 'bus-fx' });
  p.patterns[0].automation.push({ id: 'lane-bus-send', owner: 'bus:group', target: 'send:bus-send', points: [] });
  const before = copy(p), ids = ['n1', 'n2', 'n3', 'n4', 'n5', 'n6', 'n7', 'n8', 'n9', 'n10', 'n11', 'n12', 'n13', 'n14', 'n15', 'n16', 'n17', 'n18', 'n19', 'n20', 'n21', 'n22'];
  const result = duplicate.buildGroup(p, 'g', () => ids.shift()), q = result.project;
  assert.deepEqual(p, before, 'the source project is not changed');
  assert.deepEqual(result.sourceTrackIndices, [0, 1]); assert.deepEqual(result.trackIndices, [2, 3]);
  const group = q.groups.find(item => item.id === result.groupID), bus = q.routing.buses.find(item => item.id === result.busID);
  assert.equal(group.name, 'Kit copy'); assert.equal(group.busID, result.busID); assert.equal(group.libraryPath, undefined, 'Save kit keeps writing the source kit only');
  assert.equal(bus.groupID, result.groupID); assert.equal(bus.volume, .6);
  for (const i of result.trackIndices) { assert.equal(q.tracks[i].groupID, result.groupID); assert.equal(q.routing.outputs[`track:${q.tracks[i].routingID}`], `bus:${result.busID}`); }
  assert.equal(q.routing.outputs[`bus:${result.busID}`], q.routing.outputs['bus:group'] ?? 'master');
  const insert = q.audioPlugins.find(plugin => plugin.owner === `bus:${result.busID}`);
  assert.equal(insert.component, 'AU:group'); assert.notEqual(insert.instanceID, 'bus-fx'); assert.ok(result.newPluginInstanceIDs.includes(insert.instanceID));
  const send = q.routing.sends.find(item => item.source === `bus:${result.busID}`);
  assert.equal(send.level, .4); assert.notEqual(send.id, 'bus-send');
  const key = q.routing.sidechains.find(route => route.destination === insert.instanceID);
  assert.equal(key.source, `track:${q.tracks[2].routingID}`, 'a sidechain from a member follows the member copy');
  const lanes = q.patterns[0].automation.filter(item => item.owner === `bus:${result.busID}`), lane = lanes.find(item => item.target.startsWith('send:'));
  assert.equal(lanes.length, 2, 'bus lanes are copied');
  assert.equal(lane.target, `send:${send.id}`);
  assert.throws(() => duplicate.buildGroup(p, 'missing', () => 'x'), /does not exist/);
});
for (const { name, run } of tests) { run(); console.log(`PASS ${name}`); }
console.log(`Track duplicate project: ${tests.length} regression groups passed.`);
