'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const M = require('../Resources/Web/song-model.js');
const copy = value => JSON.parse(JSON.stringify(value));
const pattern = (id, length = 16) => ({ id, name: 'P' + id, length, steps: [Array(length).fill(null)], automation: [] });
const base = (sequence = [0, 0, 1, 2, 2, 2], extra = {}) => ({ linesPerBeat: 4, tracks: [{}], patterns: [pattern(0), pattern(1, 32), pattern(2)], sequence: sequence.slice(), loopSong: false, ...extra });
const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('blocks group consecutive equal patterns and map back to flat slots', () => {
  assert.deepEqual(M.blocks([0, 0, 1, 2, 2, 2, 0]).map(b => [b.pattern, b.start, b.count]), [[0, 0, 2], [1, 2, 1], [2, 3, 3], [0, 6, 1]]);
  assert.equal(M.blockAt([0, 0, 1, 2, 2, 2], 4), 2);
  const p = base();
  // Playback order is exactly the flat sequence; repeats expand inline.
  assert.deepEqual([...new Set(M.playbackOrder(p).map(step => step.sequence))], [0, 1, 2, 3, 4, 5]);
  assert.equal(M.startBeat(p, 3), 4 + 4 + 8);
});

test('repeats, pattern changes, inserts, moves, duplicates and removal stay plain sequence edits', () => {
  const p = base();
  assert.equal(M.setRepeats(p, 0, 4), 0); assert.deepEqual(p.sequence, [0, 0, 0, 0, 1, 2, 2, 2]);
  M.setRepeats(p, 2, 1); assert.deepEqual(p.sequence, [0, 0, 0, 0, 1, 2]);
  assert.equal(M.setPattern(p, 1, 2), 1); assert.deepEqual(p.sequence, [0, 0, 0, 0, 2, 2]); // merges with the following run
  assert.equal(M.insert(p, 0, 1), 1); assert.deepEqual(p.sequence, [0, 0, 0, 0, 1, 2, 2]);
  assert.equal(M.move(p, 1, 1, -1), 0); assert.deepEqual(p.sequence, [1, 0, 0, 0, 0, 2, 2]);
  assert.equal(M.move(p, 0, 0, -1), -1);
  assert.equal(M.duplicate(p, 0, 1), 2); assert.deepEqual(p.sequence, [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 2, 2]);
  M.remove(p, 2, 3); assert.deepEqual(p.sequence, [1, 0, 0, 0, 0, 2, 2]);
  assert.throws(() => M.remove(p, 0, 2), /at least one/);
  assert.throws(() => M.setPattern(p, 0, 99), /does not exist/);
  const capped = base([0]); M.setRepeats(capped, 0, 2000); assert.equal(capped.sequence.length, M.MAX_REPEATS);
  assert.equal(M.nextPatternId(p, 2, 1), 0); assert.equal(M.nextPatternId(p, 0, -1), 2);
});

test('section loop follows its music through edits and restores Loop song', () => {
  const p = base([0, 1, 2, 2, 0]);
  M.setLoop(p, 1, 2);
  assert.deepEqual([p.songLoop.start, p.songLoop.end, p.loopSong], [1, 3, true]);
  M.insert(p, 0, 2, { before: true }); // before everything
  assert.deepEqual(M.loopRange(p), { start: 2, end: 4 });
  M.setRepeats(p, 3, 4); // the 2×2 block inside the loop grows
  assert.deepEqual(M.loopRange(p), { start: 2, end: 6 });
  M.move(p, 4, 4, -1); // the trailing 0 moves before the loop's pattern 2 block: loop spans whole blocks
  const loop = M.loopRange(p), blocks = M.blocks(p.sequence);
  assert(blocks.some(b => b.start === loop.start) && blocks.some(b => b.start + b.count - 1 === loop.end));
  M.clearLoop(p); assert.equal(p.songLoop, undefined); assert.equal(p.loopSong, false);
  const q = base([0, 1]); M.setLoop(q, 1); M.remove(q, 1); assert.equal(q.songLoop, undefined); assert.equal(q.loopSong, false);
});

test('new and cloned patterns get free ids, names and fresh automation ids', () => {
  const p = base();
  const blank = M.blankPattern(p, 32); assert.equal(blank.id, 3); assert.equal(blank.steps[0].length, 32);
  p.patterns[0].automation = [{ id: 'old', points: [] }];
  const clone = M.clonePattern(p, 0, () => 'fresh'); assert.equal(clone.id, 3); assert.equal(clone.name, 'P0 copy'); assert.equal(clone.automation[0].id, 'fresh');
  assert.notEqual(clone.steps, p.patterns[0].steps);
  assert.equal(M.cycleColor(p, 1), M.COLORS[0]); assert.equal(M.cycleColor(p, 1), M.COLORS[1]); assert.equal(M.patternColor(p, 1), M.COLORS[1]);
});

// ---- Keyboard harness -------------------------------------------------------------------
function harness(sequence = [0, 0, 1], options = {}) {
  const nodes = new Map(), labels = [], toasts = [];
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: () => [], querySelector: () => null };
  const list = { id: 'sequence-list', dataset: { songArranger: '1' }, tagName: 'DIV', closest(selector) { return selector.includes('#sequence-list') ? this : null; }, focus() { document.activeElement = this; }, setAttribute() {} };
  nodes.set('sequence-list', list);
  const state = { view: 'song', sequence: 0, row: 0, modal: null, keyboardPlay: !!options.piano, playing: false, playbackMode: 'pattern', status: {}, undo: [], project: base(sequence) };
  const ST = { state, escapeHTML: String, toast: (message) => toasts.push(message), uid: () => 'u', updateTransport() {}, setView: view => { state.view = view; }, beatsPerBar: () => 4,
    mutate(label, fn) { labels.push(label); state.undo.push(copy(state.project)); fn(state.project); }, textPrompt() {}, request: async () => ({}), perform: () => true };
  const context = vm.createContext({ window: { ST }, document, Date, setTimeout() {}, console });
  web.load(context, 'song-model.js');
  web.load(context, 'song-sequence.js');
  const song = context.window.STSong;
  const key = (k, extra = {}) => { const event = { key: k, target: list, preventDefault() { this.defaultPrevented = true; }, ...extra }; event.handled = song.key(event); return event; };
  return { song, state, key, labels, toasts, ST };
}

test('arranger keys edit blocks, repeats and patterns with one undo step each', () => {
  const h = harness([0, 0, 1]);
  assert(h.key('=').handled); assert.deepEqual(h.state.project.sequence, [0, 0, 0, 1]);
  assert(h.key('-').handled); assert.deepEqual(h.state.project.sequence, [0, 0, 1]);
  assert(h.key('ArrowDown').handled); assert.equal(h.state.sequence, 2);
  assert(h.key('ArrowRight').handled); assert.deepEqual(h.state.project.sequence, [0, 0, 2]);
  assert(h.key('ArrowUp', { altKey: true }).handled); assert.deepEqual(h.state.project.sequence, [2, 0, 0]); assert.equal(h.state.sequence, 0);
  assert(h.key('i').handled); assert.deepEqual(h.state.project.sequence, [2, 1, 0, 0]); // the next pattern that starts a new block (0 would merge)
  assert(h.key('d', { metaKey: true }).handled);
  assert(h.key('Delete').handled);
  assert.equal(h.labels.length, h.state.undo.length);
  assert(h.labels.every(Boolean));
});

test('typed digits choose a pattern number; invalid edits never add an empty undo step', () => {
  const h = harness([0, 1]);
  h.key('3'); assert.deepEqual(h.state.project.sequence, [2, 1]);
  h.key('9'); assert.deepEqual(h.state.project.sequence, [2, 1]); assert(h.toasts.some(t => /Pattern 9/.test(t)));
  const before = h.labels.length;
  h.key('-'); assert.equal(h.labels.length, before); // repeat below 1 is refused before mutate
  const single = harness([0]); single.key('Delete'); assert.equal(single.labels.length, 0); assert.deepEqual(single.state.project.sequence, [0]);
});

test('shift extends a range for section loop and duplicate; L toggles the loop', () => {
  const h = harness([0, 1, 2]);
  h.key('ArrowDown', { shiftKey: true });
  h.key('l'); assert.deepEqual(h.state.project.songLoop && [h.state.project.songLoop.start, h.state.project.songLoop.end], [0, 1]); assert.equal(h.state.project.loopSong, true);
  h.key('l'); assert.equal(h.state.project.songLoop, undefined); assert.equal(h.state.project.loopSong, false);
  h.key('d', { metaKey: true }); assert.deepEqual(h.state.project.sequence, [0, 1, 0, 1, 2]);
});

test('letter commands yield to the keyboard piano; arrows and modals are respected', () => {
  const piano = harness([0, 1], { piano: true });
  for (const k of ['i', 'n', 'l', 'p', 'e', 'c', 'f', '3']) assert(!piano.key(k).handled, k);
  assert(piano.key('ArrowDown').handled);
  const modal = harness([0, 1]); modal.state.modal = 'palette'; assert(!modal.key('ArrowDown').handled);
  const other = harness([0, 1]); other.state.view = 'pattern'; assert(!other.key('ArrowDown').handled);
});

test('queue from block: queued jump while the song plays, Play from here otherwise', async () => {
  const h = harness([0, 0, 1, 1, 0]), requests = [];
  h.ST.request = async (command, payload) => { requests.push([command, payload]); return command === 'transport.queue' ? { queued: true } : {}; };
  const plays = []; h.ST.perform = command => { plays.push(command); return true; };
  // Stopped: same as Play from here.
  await h.song.actions.queueFrom(1); assert.equal(requests.length, 0); assert.ok(plays.includes('play'));
  // Song playing: one transport.queue with the block's first sequence index, no seek.
  h.state.playing = true; h.state.status = { songMode: true, sequence: 0 }; plays.length = 0;
  assert.equal(await h.song.actions.queueFrom(1), true);
  assert.equal(JSON.stringify(requests), JSON.stringify([['transport.queue', { sequence: 2 }]])); assert.equal(plays.length, 0); assert.equal(h.state.sequence, 2);
  // Q key and context menu entry.
  requests.length = 0; h.key('q'); await Promise.resolve(); assert.equal(requests[0]?.[0], 'transport.queue');
  const items = h.song.contextItems({ target: { closest: selector => selector === '[data-song-block]' ? { dataset: { songBlock: '0' } } : null } }).filter(Boolean);
  assert.ok(items.some(item => item.title === 'Play from here at pattern end' && item.enabled));
});

test('context menu offers the arranger actions for a block', () => {
  const h = harness([0, 0, 1]);
  const items = h.song.contextItems({ target: { closest: selector => selector === '[data-song-block]' ? { dataset: { songBlock: '1' } } : null } });
  const titles = items.filter(Boolean).map(item => item.title);
  for (const title of ['Play song from here', 'Edit pattern', 'Rename pattern…', 'Insert new empty pattern', 'Loop this block', 'Remove block']) assert(titles.includes(title), title);
  assert.equal(h.state.sequence, 2);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) { try { await fn(); console.log('PASS', name); } catch (error) { failed++; console.error('FAIL', name); console.error(error); } }
  if (failed) process.exit(1);
})();
