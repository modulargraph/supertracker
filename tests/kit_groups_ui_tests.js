'use strict';
// Kits are track groups (kit-groups.js) in the real app: Load kit adds a group with one member track per
// loaded pad and the kit's bus as one Undo step, published to the audio host before it commits; Replace kit
// swaps sources pad by pad and keeps the patterns; Save kit sends the group; Remove kit removes the group.
const assert = require('node:assert/strict');
const { harness } = require('./helpers/studio-session');
const { groupProject } = require('./helpers/kit-group-fixture');
const copy = value => JSON.parse(JSON.stringify(value));
const same = (a, b, m) => assert.equal(JSON.stringify(a), JSON.stringify(b), m);
const tests = [], test = (name, run) => tests.push([name, run]);

function session(source = groupProject({ name: '1988 Kit', pads: [0, 1, 3], plugins: [{ pad: 1 }, { pad: 3, kind: 'instrument', component: 'vst3:drum' }], busInserts: 2, choke: { 3: 1 } })) {
  const h = harness();
  h.handlers.set('kit.read', () => ({ groupProject: copy(source), warnings: ['Flanger is unavailable.'] }));
  return h;
}

test('Load kit appends one member per loaded pad, routed to a new group bus, as one Undo step', async () => {
  const h = session(), p0 = copy(h.S.state.project), undo = h.S.state.undo.length;
  const published = [];
  const engine = h.handlers.get('engine.project');
  h.handlers.set('engine.project', payload => { published.push(copy(payload.project)); return engine ? engine(payload) : {}; });
  assert.equal(await h.KG.loadKit('/Kits/1988 Kit.mxgrp'), true);
  const p = h.S.state.project, group = h.G.list(p)[0];
  assert.equal(h.S.state.undo.length, undo + 1);
  assert.equal(h.S.state.undo.at(-1).label, 'load kit');
  assert.equal(p.tracks.length, p0.tracks.length + 3, 'members are appended after the existing tracks');
  const members = h.G.members(p, group);
  same(members, [p0.tracks.length, p0.tracks.length + 1, p0.tracks.length + 2]);
  same(members.map(i => p.tracks[i].pad), [0, 1, 3]);
  assert.equal(p.tracks[members[2]].chokeGroup, 1);
  assert.equal(group.name, '1988 Kit'); same(group.importNotes, ['Flanger is unavailable.']);
  assert.equal(group.libraryPath, undefined, 'a Maschine group is not a library package');
  const bus = p.routing.buses.find(b => b.id === group.busID);
  assert.equal(bus.type, 'group'); assert.equal(bus.volume, .9);
  for (const i of members) assert.equal(p.routing.outputs[`track:${p.tracks[i].routingID}`], `bus:${group.busID}`);
  assert.equal(p.routing.outputs[`bus:${group.busID}`], 'master');
  // Instruments and plug-ins follow their members with fresh identities.
  for (const i of members) assert.equal(p.instruments[p.tracks[i].instrument].name, p.tracks[i].name);
  const insert = p.audioPlugins.find(x => x.owner === `track:${p.tracks[members[1]].routingID}`);
  assert.equal(insert.track, members[1]); assert.notEqual(insert.instanceID, 'src-plugin-0');
  const instrument = p.audioPlugins.find(x => x.kind === 'instrument' && x.owner === `track:${p.tracks[members[2]].routingID}`);
  assert.equal(instrument.track, members[2]);
  assert.equal(p.audioPlugins.filter(x => x.owner === `bus:${group.busID}`).length, 2);
  assert(published.some(project => project.tracks.length === p.tracks.length), 'the audio host accepted the candidate before the commit');
  assert.equal(h.S.state.track, members[0], 'the cursor moves to the first pad');
  h.S.perform('undo');
  assert.equal(h.S.state.project.tracks.length, p0.tracks.length);
  assert.equal(h.G.list(h.S.state.project).length, 0);
});

test('loading the same kit twice gives independent identities; .stkit packages remember their path', async () => {
  const h = session();
  await h.KG.loadKit('/Kits/A.stkit'); await h.KG.loadKit('/Kits/A.stkit');
  const p = h.S.state.project, [a, b] = h.G.list(p);
  assert.notEqual(a.id, b.id); assert.notEqual(a.busID, b.busID);
  const ids = [...h.G.members(p, a), ...h.G.members(p, b)].map(i => p.tracks[i].routingID), instances = p.audioPlugins.map(x => x.instanceID);
  assert.equal(new Set(ids).size, ids.length); assert.equal(new Set(instances).size, instances.length);
  assert.equal(a.libraryPath, '/Kits/A.stkit');
});

test('Replace kit swaps member sources pad by pad and keeps their pattern steps', async () => {
  const h = session();
  await h.KG.loadKit('/Kits/Old.stkit');
  let p = h.S.state.project;
  const group = h.G.list(p)[0], members = h.G.members(p, group), kick = members[0], kickTrack = p.tracks[kick];
  p.patterns[0].steps[kick][0] = { note: 60, instrument: kickTrack.instrument, velocity: 100, gate: .8 };
  h.handlers.set('kit.read', () => ({ groupProject: groupProject({ name: 'New Kit', pads: [0, 2], busInserts: 1 }), warnings: [] }));
  h.S.state.track = kick;
  const undo = h.S.state.undo.length;
  assert.equal(await h.KG.loadKit('/Kits/New.stkit', { replace: true }), true);
  p = h.S.state.project;
  assert.equal(h.S.state.undo.length, undo + 1);
  const after = h.G.members(p, group.id);
  assert.equal(after.length, 4, 'pads 1 and 2 kept, pad 4 emptied, pad 3 added');
  assert.equal(p.tracks[kick].routingID, kickTrack.routingID, 'pad 1 keeps its track and routing');
  assert.equal(p.tracks[kick].name, 'Pad 1 New Kit');
  assert.equal(p.instruments[p.tracks[kick].instrument].samplePath, '/Kits/New Kit/pad1.wav');
  assert.equal(p.patterns[0].steps[kick][0].note, 60, 'the pattern is kept');
  const empty = after.find(i => p.tracks[i].pad === 3);
  assert.equal(p.instruments[p.tracks[empty].instrument].samplePath, '', 'a pad the new kit lacks becomes an empty sampler');
  assert.equal(p.audioPlugins.filter(x => x.owner === `track:${p.tracks[empty].routingID}`).length, 0);
  assert(after.some(i => p.tracks[i].pad === 2));
  assert.equal(group.name === 'New Kit' || h.G.byID(p, group.id).name === 'New Kit', true);
  assert.equal(p.audioPlugins.filter(x => x.owner === `bus:${group.busID}`).length, 1, 'the bus takes the new kit\'s inserts');
});

test('Save kit sends the cursor group; Remove kit removes the group and its tracks in one Undo', async () => {
  const h = session(), saves = [];
  h.handlers.set('kit.save', payload => { saves.push(payload); return { path: '/Kits/Mine.stkit' }; });
  await h.KG.loadKit('/Kits/1988 Kit.mxgrp');
  const p = h.S.state.project, group = h.G.list(p)[0];
  assert.equal(await h.KG.saveKit({ name: 'Mine' }), true);
  assert.equal(saves.at(-1).groupID, group.id); assert.equal(saves.at(-1).name, 'Mine');
  assert.equal(h.G.byID(h.S.state.project, group.id).libraryPath, '/Kits/Mine.stkit');
  const tracks = h.S.state.project.tracks.length, undo = h.S.state.undo.length;
  assert.equal(h.KG.removeKit(), true);
  assert.equal(h.S.state.project.tracks.length, tracks - 3);
  assert.equal(h.G.list(h.S.state.project).length, 0);
  assert(!h.S.state.project.routing.buses.some(b => b.id === group.busID));
  assert(!h.S.state.project.audioPlugins.some(x => x.owner === `bus:${group.busID}`));
  assert.equal(h.S.state.undo.length, undo + 1);
});

test('a kit with bus inserts loads although the host reports the staged candidate\'s plug-ins first', async () => {
  // The host publishes the candidate before the commit and reports its plug-ins (plugins.changed).
  // That report must not rewrite the session (which would make the load look superseded) nor
  // leave descriptors for a bus the session does not have.
  const h = session(groupProject({ name: '1988 Kit', pads: [0, 1], busInserts: 2 }));
  const engine = h.handlers.get('engine.project');
  h.handlers.set('engine.project', payload => {
    h.window.nativeEvent('plugins.changed', { plugins: copy(payload.project.audioPlugins || []) });
    return engine ? engine(payload) : {};
  });
  assert.equal(await h.KG.loadKit('/Kits/1988 Kit.mxgrp'), true);
  const p = h.S.state.project, group = h.G.list(p)[0];
  assert.equal(p.audioPlugins.filter(x => x.owner === `bus:${group.busID}`).length, 2);
  h.window.nativeEvent('plugins.changed', { plugins: [...copy(p.audioPlugins), { owner: 'bus:gone', kind: 'effect', slot: 0, component: 'x', instanceID: 'stale' }] });
  assert.ok(!h.S.state.project.audioPlugins.some(x => x.owner === 'bus:gone'), 'a descriptor for a missing bus is not adopted');
});

test('New empty kit is a group with one empty pad track', () => {
  const h = harness();
  assert.equal(h.KG.createKit('Mine'), true);
  const p = h.S.state.project, group = h.G.list(p)[0];
  assert.equal(group.name, 'Mine'); assert.equal(h.G.members(p, group).length, 1);
  assert.equal(p.tracks[h.S.state.track].pad, 0);
});

(async () => {
  let failed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log('ok  ', name); } catch (error) { failed++; console.error('FAIL', name, '\n', error); }
  }
  if (failed) process.exit(1);
  console.log(`kit groups UI: ${tests.length} passed`);
})();
