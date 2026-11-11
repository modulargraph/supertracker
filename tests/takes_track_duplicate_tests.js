'use strict';
const assert = require('node:assert/strict');
const duplicate = require('../ui/track-duplicate-project.js');
const { complete } = require('./helpers/project');
const project = complete({
  tracks: [{id: 0, routingID: 'performance', name: 'Performance', instrument: 0}], instruments: [{id: 0, type: 'sample'}],
  routing: {buses: [], outputs: {}, sends: [], sidechains: []},
  patterns: [{id: 0, length: 1, steps: [[null]]}], sequence: [0],
  audioClips: [{id: 'clip', track: 0, path: '/takes/current.wav', startBeat: 4, durationBeats: 8, sourceOffset: .2,
    takeSet: {activeID: 'a', takes: [
      {id: 'a', name: 'Original', path: '/takes/current.wav', sourceOffset: .2, sourceDuration: 8},
      {id: 'b', name: 'Alternate', path: '/takes/alternate.wav', sourceOffset: .4, sourceDuration: 9}
    ]}}]
});
const before = JSON.parse(JSON.stringify(project));
const result = duplicate.build(project, 0);
const source = result.project.audioClips[0], copied = result.project.audioClips[1];
assert.notEqual(source.id, copied.id);
assert.equal(copied.track, 1);
assert.deepEqual(copied.takeSet, before.audioClips[0].takeSet);
assert.equal(copied.path, copied.takeSet.takes.find(take => take.id === copied.takeSet.activeID).path);
copied.takeSet.activeID = 'b'; copied.takeSet.takes[1].name = 'Independent alternate';
copied.takeSet.takes[1].sourceOffset = 1.2; copied.takeSet.takes.push({id: 'c', name: 'Third', path: '/third.wav', sourceOffset: 0});
assert.deepEqual(source, before.audioClips[0]);
assert.deepEqual(project, before);
console.log('PASS take groups survive track duplication with independent selection, names, offsets and take membership');
