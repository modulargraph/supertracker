'use strict';
// Song/Pattern mode switching keeps the music where it is: the real transport module, a stubbed host.
const assert = require('node:assert/strict');
const vm = require('node:vm');
const web = require('./helpers/web');

function harness({ playing = false, status = {}, sequence = 0, activePattern = 0, mode = 'song' } = {}) {
  const requests = [], toasts = [], selected = [];
  const context = vm.createContext({ window: {}, String, Set, Math, Number, Map, Array, Object, JSON, Promise });
  const app = web.loadAppWith(context, {
    request: (command, payload) => { requests.push(JSON.parse(JSON.stringify({ command, payload }))); return Promise.resolve({}); },
    toast: message => toasts.push(message),
    selectPattern: id => { selected.push(id); app.state.project.activePattern = id; return true; },
    $: () => null
  });
  Object.assign(app.state, {
    project: { activePattern, sequence: [3, 5, 3, 7], patterns: [3, 5, 7, 9].map(id => ({ id, length: 64 })) },
    playbackMode: mode, playing, status, sequence
  });
  app.pendingPlayback = null; app.stoppingPlayback = null;
  return { app, state: app.state, requests, toasts, selected };
}

const tests = [];
const test = (name, fn) => tests.push([name, fn]);

test('stopped Song to Pattern selects the pattern at the song position', () => {
  const h = harness({ sequence: 1, activePattern: 9 });
  assert.equal(h.app.setPlaybackMode('pattern'), true);
  assert.equal(h.state.playbackMode, 'pattern');
  assert.deepEqual(h.selected, [5]);
  assert.deepEqual(h.requests, []);
});

test('stopped Pattern to Song moves the song position to that pattern', () => {
  const h = harness({ mode: 'pattern', sequence: 1, activePattern: 3 });
  h.app.setPlaybackMode('song');
  assert.equal(h.state.playbackMode, 'song');
  assert.equal(h.state.sequence, 2, 'the next occurrence after the current position');
  const kept = harness({ mode: 'pattern', sequence: 2, activePattern: 3 });
  kept.app.setPlaybackMode('song');
  assert.equal(kept.state.sequence, 2, 'a matching current block is kept');
  const absent = harness({ mode: 'pattern', sequence: 1, activePattern: 9 });
  absent.app.setPlaybackMode('song');
  assert.equal(absent.state.sequence, 1, 'a pattern outside the song keeps the song position');
});

test('playing Song to Pattern queues looping the playing pattern', () => {
  const h = harness({ playing: true, status: { pattern: 7, sequence: 3 }, sequence: 0, activePattern: 3 });
  assert.equal(h.app.setPlaybackMode('pattern'), true);
  assert.deepEqual(h.selected, [7]);
  assert.deepEqual(h.requests, [{ command: 'transport.queue', payload: { mode: 'pattern', pattern: 7 } }]);
  assert.deepEqual(h.toasts, []);
});

test('playing Pattern to Song continues after the playing pattern block', () => {
  const h = harness({ mode: 'pattern', playing: true, status: { pattern: 5, sequence: 0 }, sequence: 0, activePattern: 5 });
  h.app.setPlaybackMode('song');
  assert.equal(h.state.sequence, 1);
  assert.deepEqual(h.requests, [{ command: 'transport.queue', payload: { mode: 'song', sequence: 2 } }]);
  const lastBlock = harness({ mode: 'pattern', playing: true, status: { pattern: 7, sequence: 0 }, activePattern: 7 });
  lastBlock.app.setPlaybackMode('song');
  assert.deepEqual(lastBlock.requests[0].payload, { mode: 'song', sequence: 3 }, 'the last block repeats, then the song ends');
});

test('the same mode and a pending start change nothing', () => {
  const h = harness({ playing: true, status: { pattern: 3, sequence: 0 } });
  assert.equal(h.app.setPlaybackMode('song'), true);
  assert.deepEqual(h.requests, []);
  const pending = harness();
  pending.app.pendingPlayback = {};
  assert.equal(pending.app.setPlaybackMode('pattern'), false);
  assert.equal(pending.state.playbackMode, 'song');
  assert.equal(pending.toasts.length, 1);
});

(async () => {
  for (const [name, fn] of tests) { await fn(); console.log('PASS', name); }
  console.log(`Playback mode: ${tests.length} groups passed.`);
})().catch(error => { console.error(error); process.exit(1); });
