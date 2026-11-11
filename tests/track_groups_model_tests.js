'use strict';
// Track groups project model (ui/track-groups.js): creation, contiguous members,
// auto-routing to the group bus, reordering with every index-based reference, ungrouping,
// dissolving and load-time repair.
const assert = require('node:assert/strict');
const G = require('../ui/track-groups.js');
const { complete } = require('./helpers/project');

let serial = 0;
const makeID = () => `id${++serial}`;
function project(count = 6) {
  const tracks = Array.from({ length: count }, (_, i) => ({ id: i, name: `T${i + 1}`, routingID: `r${i}`, instrument: i, volume: .8 }));
  const outputs = Object.fromEntries(tracks.map(t => [`track:${t.routingID}`, 'master']));
  return complete({
    tracks,
    instruments: tracks.map((_, i) => ({ id: i, name: `I${i}`, type: 'sample' })),
    patterns: [{ id: 0, length: 4, steps: tracks.map((_, i) => [{ note: 60 + i, instrument: i }, null, null, null]),
      automation: tracks.map((_, i) => ({ id: `a${i}`, track: i, target: 'volume', points: [] })) }],
    audioClips: [{ id: 'c', track: 4 }],
    audioPlugins: [{ instanceID: 'p', owner: 'track:r5', kind: 'effect', slot: 0 }],
    routing: { buses: [], outputs, sends: [], sidechains: [] }
  });
}
const names = p => p.tracks.map(t => t.name).join(' ');
const tests = [];
const test = (name, run) => tests.push([name, run]);

test('creating a group makes members contiguous, adds a group bus and routes members to it', () => {
  const p = project();
  const { group, map } = G.create(p, [1, 4], { name: 'Drums', makeID });
  assert.equal(names(p), 'T1 T2 T5 T3 T4 T6');
  assert.deepEqual(map, [0, 1, 3, 4, 2, 5]);
  assert.deepEqual(G.members(p, group), [1, 2]);
  const bus = p.routing.buses.find(b => b.id === group.busID);
  assert.equal(bus.type, 'group'); assert.equal(bus.name, 'Drums'); assert.equal(bus.groupID, group.id);
  assert.equal(p.routing.outputs[`bus:${group.busID}`], 'master');
  assert.equal(p.routing.outputs['track:r1'], `bus:${group.busID}`);
  assert.equal(p.routing.outputs['track:r4'], `bus:${group.busID}`);
  assert.equal(p.routing.outputs['track:r2'], 'master');
  // Every index-based reference followed the move.
  assert.equal(p.patterns[0].steps[2][0].note, 64);
  assert.equal(p.patterns[0].automation.find(l => l.id === 'a4').track, 2);
  assert.equal(p.audioClips[0].track, 2);
  assert.deepEqual(p.tracks.map(t => t.id), [0, 1, 2, 3, 4, 5]);
});

test('adding, removing and moving members re-routes them automatically', () => {
  const p = project();
  const { group } = G.create(p, [0, 1], { name: 'G', makeID });
  const bus = `bus:${group.busID}`;
  G.addTrack(p, group.id, 4);
  assert.equal(names(p), 'T1 T2 T5 T3 T4 T6');
  assert.equal(p.routing.outputs['track:r4'], bus);
  G.addTrack(p, group.id, 5, 0);
  assert.equal(names(p), 'T6 T1 T2 T5 T3 T4', 'position 0 inside the group');
  assert.equal(p.audioPlugins[0].owner, 'track:r5', 'a plug-in stays with its channel wherever the track moves');
  G.removeTrack(p, 1);
  assert.equal(names(p), 'T6 T2 T5 T1 T3 T4', 'a removed member lands right after its group');
  assert.equal(p.tracks[3].groupID, undefined);
  assert.equal(p.routing.outputs['track:r0'], 'master');
  // Group edges are a move of their own: a neighbour joins at the edge, an edge member leaves.
  G.moveTrack(p, 3, -1);
  assert.equal(names(p), 'T6 T2 T5 T1 T3 T4');
  assert.equal(p.tracks[3].groupID, group.id); assert.equal(p.routing.outputs['track:r0'], bus);
  G.moveTrack(p, 3, -1);
  assert.equal(names(p), 'T6 T2 T1 T5 T3 T4', 'inside a group a move swaps neighbours');
  G.moveTrack(p, 0, -1);
  assert.equal(names(p), 'T6 T2 T1 T5 T3 T4');
  assert.equal(p.tracks[0].groupID, undefined); assert.equal(p.routing.outputs['track:r5'], 'master');
  group.collapsed = true;
  G.moveTrack(p, 0, 1);
  assert.equal(names(p), 'T2 T1 T5 T6 T3 T4', 'a collapsed group is hopped over');
  group.collapsed = false;
  // A member routed elsewhere on purpose keeps that output when it leaves.
  p.routing.outputs['track:r1'] = null;
  G.removeTrack(p, 0);
  assert.equal(p.routing.outputs['track:r1'], null);
});

test('the last member leaving removes the group and its bus, inserts and sends', () => {
  const p = project(3);
  const { group } = G.create(p, [2], { name: 'Solo', makeID });
  const bus = `bus:${group.busID}`;
  p.audioPlugins.push({ instanceID: 'busfx', owner: bus, kind: 'effect', slot: 0 });
  p.routing.sends.push({ id: 's1', source: bus, destination: 'master' });
  G.removeTrack(p, 2);
  assert.equal(G.list(p).length, 0);
  assert(!p.routing.buses.length && !(bus in p.routing.outputs));
  assert(!p.audioPlugins.some(x => x.owner === bus) && !p.routing.sends.length);
});

test('dissolving a group routes its members to Master and keeps them in place', () => {
  const p = project(4);
  const { group } = G.create(p, [1, 2], { name: 'G', makeID });
  G.dissolve(p, group.id);
  assert.equal(names(p), 'T1 T2 T3 T4');
  assert(p.tracks.every(t => t.groupID === undefined));
  assert.equal(p.routing.outputs['track:r1'], 'master');
  assert.equal(p.routing.buses.length, 0);
});

test('normalize repairs loaded groups: unknown members, gaps, missing buses, empty groups', () => {
  const p = project(5);
  p.groups = [{ id: 'g', name: 'Kit', busID: 'b' }, { id: 'empty', name: 'E', busID: 'eb' }];
  p.tracks[0].groupID = 'g'; p.tracks[3].groupID = 'g'; p.tracks[4].groupID = 'missing';
  assert.equal(G.normalize(p), true);
  assert.equal(names(p), 'T1 T4 T2 T3 T5');
  assert.equal(p.tracks[4].groupID, undefined);
  assert.deepEqual(G.list(p).map(g => g.id), ['g']);
  assert(p.routing.buses.some(b => b.id === 'b' && b.type === 'group'));
  assert.equal(G.normalize(p), false, 'normalize is idempotent');
});

test('collapsed groups show one overview column; expanded groups show their members', () => {
  const p = project(5);
  const { group } = G.create(p, [1, 2, 3], { name: 'G', makeID });
  assert.equal(G.columns(p).length, 5);
  group.collapsed = true;
  const columns = G.columns(p);
  assert.deepEqual(columns.map(c => c.index), [0, 1, 4]);
  assert.deepEqual(columns[1].members, [1, 2, 3]);
  assert.equal(columns[1].collapsed, true);
});

(async () => {
  let failed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log('ok  ', name); } catch (error) { failed++; console.error('FAIL', name, '\n', error); }
  }
  if (failed) process.exit(1);
  console.log(`track groups model: ${tests.length} passed`);
})();
