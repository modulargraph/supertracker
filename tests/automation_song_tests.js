'use strict';
// Automation across patterns: the pure model in automation-song.js and the Automation view's
// pattern switching / song timeline in studio.js. Run: node tests/automation_song_tests.js
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const M = require('../Resources/Web/automation-song.js');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const close = (actual, expected, eps = 1e-6) => assert(Math.abs(actual - expected) < eps, `${actual} != ${expected}`);

const tests = [];
const test = (name, run) => tests.push({ name, run });

function project() {
  const steps = (length, plays) => [Array.from({ length }, (_, row) => plays && row % 16 === 0 ? { note: 40 } : null), Array(length).fill(null)];
  return {
    linesPerBeat: 4, activePattern: 0,
    tracks: [{ name: 'Bass', instrument: 0 }, { name: 'Lead', instrument: 1, routingID: 7 }],
    instruments: [{ type: 'synth' }, { type: 'synth' }],
    patterns: [
      { id: 0, name: 'Intro', length: 64, steps: steps(64, true), automation: [{ id: 'a', track: 0, target: 'cutoff', name: 'Bass cutoff', interpolation: 'linear', points: [{ row: 0, value: 200 }, { row: 63, value: 2000 }] }] },
      { id: 1, name: 'Verse', length: 32, steps: steps(32, true), automation: [] },
      { id: 2, name: 'Break', length: 64, steps: steps(64, false), automation: [{ id: 'b', track: 0, target: 'volume', name: 'Level', interpolation: 'step', points: [{ row: 8, value: .5 }] }] }
    ],
    sequence: [0, 1, 1, 2]
  };
}
const cutoff = { min: 20, max: 20000, step: 1, value: 18000, log: true };
const linear = { min: 0, max: 1, step: .01, value: .8 };
let counter = 0;
const uid = () => `uid-${++counter}`;

// ---- Model ----------------------------------------------------------------------------
test('timeline lays the sequence out with repeats and global rows', () => {
  const list = M.timeline(project());
  assert.deepEqual(list.map(item => [item.flat, item.pattern, item.start, item.length, item.occurrence, item.occurrences]), [[0, 0, 0, 64, 1, 1], [1, 1, 64, 32, 1, 2], [2, 1, 96, 32, 2, 2], [3, 2, 128, 64, 1, 1]]);
  assert.equal(M.totalRows(list), 192);
  assert.equal(M.instanceAt(list, 95), 1); assert.equal(M.instanceAt(list, 96), 2); assert.equal(M.instanceAt(list, 999), 3);
});

test('summary and patternsUsing report lanes, track use and song count', () => {
  const p = project(), before = copy(p);
  const rows = M.summary(p, 'track:0', 'cutoff');
  assert.deepEqual(rows.map(row => [row.id, row.lane, row.points, row.plays, row.inSong]), [[0, true, 2, true, 1], [1, false, 0, true, 2], [2, false, 0, false, 1]]);
  assert.deepEqual(M.patternsUsing(p, 'track:0'), [0, 1]);
  assert.deepEqual(M.patternsUsing(p, 'master'), [0, 1, 2], 'buses and master use every song pattern');
  assert.deepEqual(M.summary(p, 0, 'cutoff')[0].lane, true, 'numeric owners normalize');
  assert.deepEqual(p, before, 'reading never changes the project');
});

test('copyLane keeps rows as time, ends shorter patterns on the source value and replaces lanes', () => {
  const p = project();
  p.patterns[2].automation.push({ id: 'old', track: 0, target: 'cutoff', name: 'Old', interpolation: 'linear', points: [{ row: 5, value: 99 }] });
  const count = M.copyLane(p, 0, 'track:0', 'cutoff', [1, 2, 0, 9], { uid });
  assert.equal(count, 2, 'source and missing patterns are skipped');
  const verse = M.findLane(p, p.patterns[1], 'track:0', 'cutoff');
  assert.equal(verse.track, 0); assert.equal(verse.owner, undefined); assert.equal(verse.name, 'Bass cutoff'); assert.match(verse.id, /^uid-/);
  assert.equal(verse.points[0].row, 0); assert.equal(verse.points[0].value, 200);
  close(verse.points[1].row, 32 - 1 / 64);
  close(verse.points[1].value, 200 + (2000 - 200) * (32 - 1 / 64) / 63, 1e-5);
  const brk = M.findLane(p, p.patterns[2], 'track:0', 'cutoff');
  assert.equal(brk.id, 'old', 'existing lane object is reused'); assert.deepEqual(brk.points, [{ row: 0, value: 200 }, { row: 63, value: 2000 }]);
  assert.throws(() => M.copyLane(p, 1, 'track:0', 'pan', [0]), /no lane/);
});

test('songLane holds the previous value through patterns without a lane, like the engine', () => {
  const p = project(), song = M.songLane(p, 'track:0', 'cutoff', { base: 18000 });
  assert.equal(song[0].points.length, 2); assert.equal(song[0].incoming, 18000);
  assert.equal(song[1].points.length, 0); assert.equal(song[1].incoming, 2000);
  assert.equal(M.valueAtSong(song, 10), 200 + 1800 * 10 / 63);
  assert.equal(M.valueAtSong(song, 100), 2000);
  const live = M.songLane(p, 'track:0', 'cutoff', { base: 1, live: id => id === 1 ? [{ row: 4, value: 500 }] : null });
  assert.equal(M.valueAtSong(live, 100), 500, 'live recording points override the stored lane');
});

test('making several song occurrences unique rejects capacity before changing any pattern or slot', () => {
  const p=project();p.patterns=Array.from({length:255},(_,id)=>({id,name:'P'+id,length:16,steps:[[]],automation:[]}));p.sequence=[0,0,1,1];
  const before=JSON.stringify(p);assert.throws(()=>M.makeUnique(p,[1,3],{uid}),/256|capacity|patterns/i);assert.equal(JSON.stringify(p),before);
  const changes=M.makeUnique(p,[1],{uid});assert.equal(changes.length,1);assert.equal(p.patterns.length,256);assert.equal(p.sequence[0],0);assert.equal(p.sequence[1],255);
});

test('applySweep writes one ramp into every pattern it crosses and keeps the rest', () => {
  const p = project();
  p.patterns[1].automation = []; p.sequence = [0, 1, 2];
  // Sweep volume (linear) from Intro row 32 (0.2) to Break row 31 (1.0): global 32 .. 127.
  const touched = M.applySweep(p, 'track:0', 'volume', { row: 32, value: .2 }, { row: 127, value: 1 }, linear, { uid, name: 'Level' });
  assert.deepEqual(touched, [0, 1, 2]);
  const intro = M.findLane(p, p.patterns[0], 0, 'volume').points;
  // Intro had no volume lane: the value carried in (base .8) holds until the sweep starts.
  assert.deepEqual(intro.slice(0, 2), [{ row: 0, value: .8 }, { row: 32 - 1 / 64, value: .8 }]);
  assert.deepEqual(intro[2], { row: 32, value: .2 });
  close(intro[intro.length - 1].row, 64 - 1 / 64); close(intro[intro.length - 1].value, .2 + .8 * (32 - 1 / 64) / 95);
  const verse = M.findLane(p, p.patterns[1], 0, 'volume').points;
  close(verse[0].value, .2 + .8 * 32 / 95); close(verse[1].value, .2 + .8 * (64 - 1 / 64) / 95);
  const brk = M.findLane(p, p.patterns[2], 0, 'volume');
  assert.equal(brk.interpolation, 'step', 'an existing lane keeps its interpolation');
  close(brk.points[0].value, .2 + .8 * (96 - 32) / 95); assert.equal(brk.points[0].row, 0);
  const end = brk.points.find(point => point.row === 31); assert.equal(end.value, 1);
  // The old Break point at row 8 was inside the sweep and is replaced; nothing after row 31 existed.
  assert(!brk.points.some(point => point.row === 8));
});

test('applySweep keeps existing points after the sweep with an anchor back to the old curve', () => {
  const p = project(); p.sequence = [0];
  M.applySweep(p, 0, 'cutoff', { row: 10, value: 1000 }, { row: 20, value: 1000 }, { ...cutoff, log: false });
  const points = M.findLane(p, p.patterns[0], 0, 'cutoff').points;
  assert.deepEqual(points.map(point => point.row), [0, 10 - 1 / 64, 10, 20, 20 + 1 / 64, 63].map(row => Number(row.toFixed(6))));
  close(points[1].value, 200 + 1800 * (10 - 1 / 64) / 63, 1e-5); close(points[4].value, 200 + 1800 * (20 + 1 / 64) / 63, 1e-5);
});

test('log-scaled sweeps add a point per beat on a perceptual curve', () => {
  const p = project(); p.sequence = [0];
  M.applySweep(p, 0, 'cutoff', { row: 0, value: 20 }, { row: 32, value: 20000 }, cutoff);
  const points = M.findLane(p, p.patterns[0], 0, 'cutoff').points;
  const mid = points.find(point => point.row === 16);
  close(mid.value, Math.sqrt(20 * 20000), 1e-3);
  assert.equal(points.filter(point => point.row > 0 && point.row < 32).length, 7);
});

test('sharedInRange and makeUnique give repeated slots their own pattern', () => {
  const p = project();
  const shared = M.sharedInRange(p, 70, 140);
  assert.deepEqual(shared, [{ pattern: 1, slots: [1, 2], inRange: [1, 2] }]);
  assert.deepEqual(M.sharedInRange(p, 0, 63), []);
  const changes = M.makeUnique(p, [1, 2], { uid });
  assert.deepEqual(changes.map(change => [change.flat, change.from]), [[2, 1]], 'the first listed slot keeps the original');
  assert.equal(p.sequence[1], 1); assert.equal(p.sequence[2], changes[0].to);
  const clone = p.patterns.find(item => item.id === changes[0].to);
  assert.equal(clone.name, 'Verse · 2'); assert.equal(clone.length, 32);
  const q = project(); q.patterns[1].automation = [{ id: 'keep', track: 0, target: 'pan', points: [] }];
  const one = M.makeUnique(q, [2], { uid });
  assert.equal(q.sequence[1], 1, 'unlisted slots keep the original'); assert.equal(q.sequence[2], one[0].to);
  assert.notEqual(q.patterns.find(item => item.id === one[0].to).automation[0].id, 'keep', 'cloned lanes get fresh ids');
  assert.deepEqual(M.makeUnique(q, [0]), [], 'a pattern that plays once stays as it is');
});

// ---- Studio integration ---------------------------------------------------------------
function harness({ song = true } = {}) {
  const nodes = new Map(), mutations = [], toasts = [], automated = [];
  let refreshes = 0;
  class Element {
    constructor(id) { this.id = id; this.dataset = {}; this.style = {}; this.listeners = new Map(); this.value = ''; this.textContent = ''; this.classList = { toggle() {}, contains: () => false }; this.html = ''; }
    set innerHTML(value) { this.html = value; } get innerHTML() { return this.html; }
    addEventListener(type, fn) { this.listeners.set(type, [...(this.listeners.get(type) || []), fn]); }
    removeEventListener(type, fn) { this.listeners.set(type, (this.listeners.get(type) || []).filter(item => item !== fn)); }
    dispatch(type, values = {}) { const event = { type, target: this, currentTarget: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...values }; for (const fn of [...(this.listeners.get(type) || [])]) fn(event); return event; }
    focus() { document.activeElement = this; }
    getBoundingClientRect() { return { left: 0, top: 0, width: 900, height: 274 }; }
    closest() { return null; }
  }
  const body = new Element(''); body.tagName = 'BODY';
  const document = { activeElement: body, body, getElementById: id => { if (!nodes.has(id)) nodes.set(id, new Element(id)); return nodes.get(id); }, querySelectorAll: () => [], querySelector: () => null };
  const state = { view: 'automation', track: 0, instrument: 0, selectedLane: 0, row: 0, playing: false, status: {}, sequence: 0, project: project() };
  // Pattern 2 has a different lane first, so the cutoff lane sits at another index there.
  state.project.patterns[2].automation.push({ id: 'c', track: 0, target: 'cutoff', name: 'Bass cutoff', interpolation: 'linear', points: [{ row: 0, value: 5000 }] });
  const ST = { state, escapeHTML: value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c])), uid,
    refresh() { refreshes++; }, toast(message) { toasts.push(message); }, request: () => Promise.resolve([]),
    mutate(label, fn, options = {}) { mutations.push(label); fn(state.project); if (options.render !== false) refreshes++; },
    automate(owner, target, name) { automated.push([owner, target, name]); },
    openModal() {}, closeModal() {} };
  const context = { window: { ST }, document, setTimeout: () => 0, clearTimeout() {}, requestAnimationFrame: fn => fn(), console };
  context.window.window = context.window;
  vm.createContext(context);
  if (song) { web.load(context, 'song-model.js'); web.load(context, 'automation-song.js'); }
  web.load(context, 'studio.js');
  const studio = context.window.STStudio;
  return { studio, state, nodes, document, mutations, toasts, automated, body, get refreshes() { return refreshes; },
    render() { const html = studio.renderAutomation(); nodes.clear(); studio.bindAutomation(); return html; }, // a real render replaces every node
    key(key, target = body, extra = {}) { const event = { key, target, preventDefault() { event.prevented = true; }, stopPropagation() {}, ...extra }; return [studio.key(event), event]; } };
}

test('the Automation view shows a pattern selector with lane presence per pattern', () => {
  const h = harness(), html = h.render();
  assert.match(html, /id="automation-pattern"/); assert.match(html, /id="automation-pattern-prev"/); assert.match(html, /id="automation-pattern-next"/);
  assert.match(html, /01 · Intro · ∿ 2 pts · ×1 in song/);
  assert.match(html, /02 · Verse · ×2 in song/);
  assert.match(html, /data-automation-goto="0"[^>]*>P01<small>2<\/small>/);
  assert.match(html, /data-automation-goto="2"[^>]*>P03<small>1<\/small>/);
  assert.match(html, /id="automation-scope-song"/); assert.match(html, /id="automation-copy-lane"/);
});

test('the pattern selector, chips and [ ] follow the song order; unused patterns come last', () => {
  const h = harness();
  h.state.project.sequence = [2, 0, 2];
  const html = h.render();
  assert.match(html, /<optgroup label="In the song"><option value="2"[^>]*>03 · [^<]*<\/option><option value="0"[^>]*>01 · Intro[^<]*<\/option><\/optgroup><optgroup label="Not in the song"><option value="1"[^>]*>02 · Verse[^<]*<\/option><\/optgroup>/);
  assert.ok(html.indexOf('data-automation-goto="2"') < html.indexOf('data-automation-goto="0"'), 'chips in song order, numbers kept');
  h.key(']'); assert.equal(h.state.project.activePattern, 1, 'after the last song pattern comes the unused one');
  h.render(); h.key(']'); assert.equal(h.state.project.activePattern, 2, 'then it wraps to the first song pattern');
  h.render(); h.key(']'); assert.equal(h.state.project.activePattern, 0);
});

test('[ and ] switch patterns and keep the parameter, even at another lane index', () => {
  const h = harness(); h.render();
  let [handled, event] = h.key(']');
  assert.equal(handled, true); assert.equal(event.prevented, true);
  assert.equal(h.state.project.activePattern, 1);
  let html = h.render();
  assert.match(html, /Bass cutoff has no automation in Verse/, 'a pattern without the lane shows the ghost panel');
  assert.match(html, /id="automation-ghost-copy" data-from="0"/);
  assert.equal(h.mutations.length, 0, 'switching is not an edit');
  h.key(']'); html = h.render();
  assert.equal(h.state.project.activePattern, 2);
  assert.equal(h.state.selectedLane, 1, 'the cutoff lane is selected, not the first lane');
  assert.match(html, /id="automation-envelope"/);
  h.key('['); h.key('['); h.render();
  assert.equal(h.state.project.activePattern, 0); assert.equal(h.state.selectedLane, 0);
  assert.equal(h.state.octave, undefined, '[ and ] do not change the octave in the Automation view');
  const [typed] = h.key('[', { tagName: 'INPUT', type: 'search', dataset: {} });
  assert.equal(typed, false, 'text fields keep typing [');
  const [menu] = h.key(']', { id: 'automation-pattern', tagName: 'SELECT', dataset: {} });
  assert.equal(menu, true, 'the pattern menu switches with ] as well');
});

test('ghost actions create or copy the lane in one undo step', () => {
  const h = harness(); h.render(); h.key(']'); h.render();
  h.nodes.get('automation-ghost-create').dispatch('click');
  assert.deepEqual(copy(h.automated), [['track:0', 'cutoff', 'Bass cutoff']]);
  h.nodes.get('automation-ghost-copy').dataset.from = '0';
  h.nodes.get('automation-ghost-copy').dispatch('click');
  assert.deepEqual(h.mutations, ['Copy automation lane']);
  const lane = h.state.project.patterns[1].automation.find(item => item.target === 'cutoff');
  assert.equal(lane.points[0].value, 200);
  const html = h.render();
  assert.match(html, /id="automation-envelope"/); assert.doesNotMatch(html, /automation-ghost/);
});

test('copy lane to patterns using this track is one undo step', () => {
  const h = harness(); h.render();
  assert.equal(h.studio.command('automationCopyLaneUsing'), 1);
  assert.deepEqual(h.mutations, ['Copy automation lane to 1 pattern'], 'Break has no Bass steps, so only Verse');
  assert(h.state.project.patterns[1].automation.some(item => item.target === 'cutoff'));
});

test('backslash opens the song timeline with repeats and a shared-pattern warning', () => {
  const h = harness(); h.render();
  h.key(']'); h.render(); // Verse: ghost, still the cutoff parameter
  const [handled] = h.key('\\');
  assert.equal(handled, true); assert.equal(h.studio.automationScope, 'song');
  const html = h.render();
  assert.match(html, /Bass cutoff across the song/);
  assert.match(html, /id="automation-song-envelope"/);
  assert.match(html, /4 slots · 192 rows/);
  assert.match(html, /P02 “Verse” plays 2× in this song/);
  assert.match(html, /class="studio-song-hold"/, 'a slot without a lane draws the held value');
  assert.match(html, /data-song-point="0:1"/); assert.match(html, /data-song-point="3:0"/);
  assert.match(html, /id="automation-sweep-apply"/);
  assert.match(html, /<small>1\/2<\/small>/);
});

test('song timeline keys edit the slot pattern lane, one undo step per edit', () => {
  const h = harness(); h.render(); h.key('\\'); h.render();
  const svg = { dispatch: (...args) => h.document.getElementById('automation-song-envelope').dispatch(...args) };
  // Slot 1 (Intro) is selected; Enter adds a point there.
  svg.dispatch('keydown', { key: 'Enter' });
  assert.deepEqual(h.mutations, ['Add automation point']);
  const intro = h.state.project.patterns[0].automation[0].points;
  assert.deepEqual(copy(intro.map(point => point.row)), [0, 4, 63]);
  h.render();
  svg.dispatch('keydown', { key: 'ArrowUp', shiftKey: true });
  assert.equal(h.mutations.length, 2); assert(h.state.project.patterns[0].automation[0].points[1].value > intro[1].value);
  // PageDown selects Verse (no lane); Enter creates its lane with the held value.
  svg.dispatch('keydown', { key: 'PageDown' }); h.render();
  assert.equal(h.state.project.activePattern, 1);
  svg.dispatch('keydown', { key: 'Enter' });
  const verse = h.state.project.patterns[1].automation.find(item => item.target === 'cutoff');
  assert.deepEqual(copy(verse.points), [{ row: 0, value: 2000 }]);
  assert.equal(verse.track, 0); assert.equal(verse.name, 'Bass cutoff');
  assert.equal(h.mutations.length, 3);
  // U makes the selected Verse slot unique: slot 2 gets a copy.
  h.render(); svg.dispatch('keydown', { key: 'u' });
  assert.equal(h.mutations[3], 'Make song slot unique');
  assert.equal(h.state.project.sequence[1], h.state.project.patterns[3].id);
  assert.equal(h.state.project.sequence[2], 1);
});

test('without the song module the view still switches patterns', () => {
  const h = harness({ song: false }), html = h.render();
  assert.doesNotMatch(html, /automation-scope-song/); assert.match(html, /id="automation-pattern"/);
  h.key(']'); assert.equal(h.state.project.activePattern, 1);
  const [handled] = h.key('\\'); assert.equal(handled, false);
});

test('palette, help sheet and index wiring', () => {
  const { ST, app } = web.app(), source = web.source('app.js');
  for (const id of ['automationPatternPrev', 'automationPatternNext', 'automationScope', 'automationCopyLane', 'automationCopyLaneUsing', 'automationSweep', 'automationMakeUnique']) {
    assert(ST.commands.some(command => command[0] === id && command[1].startsWith('Automation:')), `${id} in the palette`);
    assert(app.hasCommand(id), `${id} in perform`);
  }
  assert.match(source, /\['Previous \/ next pattern · keeps the parameter',\s*'\[ \/ \]'\]/);
  assert.equal(typeof ST.selectPattern, 'function', 'pattern selection remains public regardless of other exports');
  const html = web.read('index.html');
  assert(html.indexOf('song-model.js') < html.indexOf('automation-song.js') && html.indexOf('automation-song.js') < html.indexOf('automation-record.js'));
});

let passed = 0;
for (const { name, run } of tests) {
  try { run(); passed++; console.log(`ok - ${name}`); }
  catch (error) { console.error(`not ok - ${name}`); throw error; }
}
console.log(`${passed} automation song tests passed`);
