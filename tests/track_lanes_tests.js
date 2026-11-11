'use strict';
// Track lanes (app-track-lanes.js, app-track-drag.js, STGroups.moveTracks / removeTracks): hide empty tracks,
// collapsed (slim) tracks, drag-and-drop reordering in the pattern editor and the mixer, and deleting tracks and
// groups, with Undo / Redo for each.
const assert = require('node:assert/strict');
const G = require('../ui/track-groups.js');
const { session } = require('./helpers/mk3-session');
const tests = [], test = (name, run) => tests.push([name, run]);
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };

// Demo project (8 tracks, 2 patterns); every track gets a stable routing identity, as normalize gives it in the app.
function lanes() {
  const h = session(), app = h.win.STApp;
  h.state.view = 'pattern';
  h.state.project.tracks.forEach((track, i) => { track.routingID = `r${i}`; });
  const p = () => h.state.project;
  const visible = () => app.patternColumns().map(c => c.collapsed ? `G${c.index}` : c.slim ? `s${c.index}` : c.index);
  const html = () => { h.ST.refresh(); return h.element('#app').innerHTML; };
  const names = () => p().tracks.map(t => t.name).join(' | ');
  return { h, app, p, visible, html, names };
}
const keyEvent = (h, key, code, mods = {}) => {
  const event = { key, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, ...mods,
    target: h.element('tracker'), preventDefault() { this.prevented = true; }, defaultPrevented: false };
  event.target.id = 'tracker';
  h.ST.keyboard(event);
  return event;
};
const clearTrack = (p, index, patterns = p.patterns) => { for (const pat of patterns) pat.steps[index] = pat.steps[index].map(() => null); };

/* ---------- model ---------- */

function model(count = 6) {
  const tracks = Array.from({ length: count }, (_, i) => ({ id: i, name: `T${i + 1}`, routingID: `r${i}`, instrument: i }));
  return {
    tracks,
    instruments: tracks.map((_, i) => ({ id: i, name: `I${i}`, type: 'sample' })),
    patterns: [{ id: 0, length: 2, steps: tracks.map((_, i) => [{ note: 60 + i }, null]),
      automation: tracks.map((_, i) => ({ id: `a${i}`, track: i, owner: `track:r${i}`, target: 'volume', points: [] })) }],
    audioClips: [{ id: 'c4', track: 4 }, { id: 'c1', track: 1 }],
    audioPlugins: [{ instanceID: 'p5', owner: 'track:r5', track: 5, kind: 'effect', slot: 0 },
      { instanceID: 'p2', owner: 'track:r2', track: 2, kind: 'effect', slot: 0 }],
    pluginPages: { p2: {}, p5: {} },
    routing: { buses: [], outputs: Object.fromEntries(tracks.map(t => [`track:${t.routingID}`, 'master'])),
      sends: [], sidechains: [] }
  };
}
const order = p => p.tracks.map(t => t.name).join(' ');

test('moveTracks moves a block before a track, joins or leaves groups and keeps groups contiguous', () => {
  let serial = 0;
  const p = model(), { group } = G.create(p, [1, 2], { name: 'G', makeID: () => `id${++serial}` }), bus = `bus:${group.busID}`;
  // Into the group, between its members.
  let map = G.moveTracks(p, [4], 2, group.id);
  assert.equal(order(p), 'T1 T2 T5 T3 T4 T6');
  same(G.members(p, group), [1, 2, 3]);
  assert.equal(p.routing.outputs['track:r4'], bus);
  assert.equal(map[4], 2);
  assert.equal(p.patterns[0].steps[2][0].note, 64, 'notes follow');
  assert.equal(p.audioClips.find(c => c.id === 'c4').track, 2, 'clips follow');
  // Out of the group, to the end.
  G.moveTracks(p, [2], 6, null);
  assert.equal(order(p), 'T1 T2 T3 T4 T6 T5');
  assert.equal(p.tracks[5].groupID, undefined);
  assert.equal(p.routing.outputs['track:r4'], 'master');
  // A drop inside a group it does not join lands after the group.
  G.moveTracks(p, [0], 2, null);
  assert.equal(order(p), 'T2 T3 T1 T4 T6 T5');
  same(G.members(p, group), [0, 1]);
  // The whole group moves as one block.
  G.moveTracks(p, G.members(p, group), 6, group.id);
  assert.equal(order(p), 'T1 T4 T6 T5 T2 T3');
  same(G.members(p, group), [4, 5]);
  assert.equal(p.audioPlugins.find(x => x.instanceID === 'p5').track, 2, 'plug-in descriptors follow');
  // The last member leaving removes the group and its bus.
  G.moveTracks(p, [4, 5], 0, null);
  assert.equal(G.list(p).length, 0);
  assert(!p.routing.buses.some(b => b.id === group.busID));
});

test('removeTracks deletes tracks with their steps, automation, clips, plug-ins, sends and sidechains', () => {
  const p = model();
  p.routing.sends.push({ id: 's1', source: 'track:r2', destination: 'track:r0', level: .5 }, { id: 's2', source: 'track:r1', destination: 'track:r3' });
  p.routing.sidechains.push({ source: 'track:r2', destination: { instanceID: 'p5' } }, { source: 'track:r1', destination: { instanceID: 'p2' } });
  p.routing.outputs['track:r3'] = 'track:r2';
  p.patterns[0].automation.push({ id: 'send', target: 'send:s1', points: [] });
  const map = G.removeTracks(p, [2, 4]);
  assert.equal(order(p), 'T1 T2 T4 T6');
  same(map, [0, 1, -1, 2, -1, 3]);
  assert.equal(p.patterns[0].steps.length, 4);
  same(p.patterns[0].steps.map(s => s[0].note), [60, 61, 63, 65]);
  same(p.patterns[0].automation.map(l => l.id), ['a0', 'a1', 'a3', 'a5'], 'their lanes and the lanes of removed sends go');
  same(p.patterns[0].automation.map(l => l.track), [0, 1, 2, 3]);
  same(p.audioClips.map(c => c.id), ['c1']);
  same(p.audioPlugins.map(x => [x.instanceID, x.track]), [['p5', 3]]);
  assert.equal(p.pluginPages.p2, undefined);
  same(p.routing.sends.map(s => s.id), ['s2']);
  same(p.routing.sidechains, [], 'sidechains from a removed track or into its plug-ins go');
  assert.equal(p.routing.outputs['track:r2'], undefined);
  assert.equal(p.routing.outputs['track:r3'], null, 'a channel that fed a removed track is disconnected');
  same(p.tracks.map(t => t.id), [0, 1, 2, 3]);
  assert.throws(() => G.removeTracks(p, [0, 1, 2, 3]), /at least one track/);
});

/* ---------- hide empty tracks ---------- */

test('orphaned plug-in descriptors (owner deleted) are dropped on load and before every audio publish', async () => {
  const { h, app, p } = lanes();
  const keep = { owner: `track:${p().tracks[1].routingID}`, kind: 'effect', slot: 0, instanceID: 'keep' }, bus = { owner: 'bus:nope', kind: 'effect', slot: 0, instanceID: 'bus' },
    gone = { owner: 'track:deleted', kind: 'effect', slot: 0, instanceID: 'gone' }, inst = { owner: 'instrument:999', kind: 'effect', slot: 0, instanceID: 'inst' };
  const loaded = app.normalize({ ...JSON.parse(JSON.stringify(p())), audioPlugins: [keep, gone, bus, inst] });
  same(loaded.audioPlugins.map(x => x.instanceID), ['keep']);
  p().audioPlugins = [keep, gone];
  await app.syncEngineProject(() => JSON.parse(JSON.stringify(p())), { replace: true });
  const sent = h.calls.filter(c => c.command === 'engine.project').at(-1);
  same(sent.payload.project.audioPlugins.map(x => x.instanceID), ['keep']);
});
test('Hide empty tracks hides tracks without steps (this pattern or the whole song); a view setting, not an Undo step', () => {
  const { h, app, p, visible } = lanes();
  h.ST.perform('addTrack'); h.ST.perform('addTrack');
  clearTrack(p(), 6, [p().patterns[0]]);
  h.state.track = 0; h.state.undo = []; h.state.redo = [];
  same(visible(), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  h.ST.perform('hideEmptyTracks');
  assert.equal(p().viewState.hideEmpty, true);
  same(visible(), [0, 1, 2, 3, 4, 5, 7], 'Open hat (empty in this pattern) and the two new tracks hide');
  assert.equal(h.state.undo.length, 0, 'not an Undo step');
  h.ST.perform('hideEmptyScope');
  same(visible(), [0, 1, 2, 3, 4, 5, 6, 7], 'whole song: Open hat plays in pattern 2');
  h.ST.perform('hideEmptyScope');
  // The cursor track and the selection anchor are never hidden.
  h.state.track = 8;
  same(visible(), [0, 1, 2, 3, 4, 5, 7, 8]);
  h.state.track = 0; h.state.selection = { track: 9, row: 0 };
  same(visible(), [0, 1, 2, 3, 4, 5, 7, 9]);
  h.state.selection = null;
  const page = (h.ST.refresh(), h.element('#app').innerHTML);
  assert.match(page, /id="hide-empty" class="active" aria-pressed="true"[^>]*>Hide empty <span class="lane-hidden-count">3<\/span>/);
  assert.match(page, /id="hide-empty-scope"/);
  assert.doesNotMatch(page, /data-cell="6:0"/);
  // Undo / Redo of an edit keep the view.
  h.ST.mutate('edit', q => { q.patterns[0].steps[0][1] = null; });
  h.ST.perform('undo'); h.ST.perform('redo');
  assert.equal(p().viewState.hideEmpty, true);
  keyEvent(h, 'u', 'KeyU', { altKey: true });
  same(visible().length, 10, '⌥U shows every track again');
});

test('a hidden track reappears as soon as it gets a note (edit, agent mutate, live paint)', () => {
  const { h, app, p, visible, html } = lanes();
  h.ST.perform('addTrack'); h.ST.perform('addTrack');
  h.state.track = 0;
  app.setHideEmpty(true);
  same(visible(), [0, 1, 2, 3, 4, 5, 6, 7]);
  h.ST.mutate('AI: write steps', q => { q.patterns[0].steps[8][4] = { note: 60, instrument: 0, velocity: 100 }; });
  same(visible(), [0, 1, 2, 3, 4, 5, 6, 7, 8]);
  // Live recording paints a single cell; a track hidden in the rendered grid makes the grid render again.
  let page = html();
  assert.doesNotMatch(page, /data-cell="9:3"/);
  p().patterns[0].steps[9][3] = { note: 62, instrument: 0, velocity: 90 };
  h.element('#app').innerHTML = '';
  app.paintRecordedCell(p().patterns[0], 9, 3);
  page = h.element('#app').innerHTML;
  assert.match(page, /data-cell="9:3"/, 'the grid rendered again with the track');
});

test('keyboard navigation skips hidden tracks; ⌘⇧← / → step over them as one Undo step', () => {
  const { h, app, p, visible, names } = lanes();
  clearTrack(p(), 4); clearTrack(p(), 6);
  h.state.track = 3;
  app.setHideEmpty(true);
  same(visible(), [0, 1, 2, 3, 5, 7]);
  app.moveGrid(1, 0);
  assert.equal(h.state.track, 5, 'Right skips the hidden Snare');
  app.moveGrid(1, 0);
  assert.equal(h.state.track, 7);
  app.moveGrid(-2, 0);
  assert.equal(h.state.track, 3);
  // Moving the Kick right steps over the hidden Snare in one go.
  h.state.undo = [];
  h.ST.perform('moveTrackRight');
  assert.equal(names(), 'Velvet bass | Glass keys | Warm pad | Snare | Closed hat | Kick | Open hat | Pulse');
  assert.equal(p().tracks[h.state.track].name, 'Kick');
  assert.equal(h.state.undo.length, 1);
  h.ST.perform('undo');
  assert.equal(p().tracks[3].name, 'Kick');
});

test('inside groups empty members hide; the header shows “+N hidden” and reveals them; a group keeps one column', () => {
  const { h, app, p, visible, html } = lanes();
  h.state.track = 3; h.state.selection = { track: 6, row: 0 };
  h.ST.perform('groupTracks');
  const group = G.list(p())[0];
  clearTrack(p(), 4); clearTrack(p(), 6);
  h.state.track = 0; h.state.selection = null;
  app.setHideEmpty(true);
  same(visible(), [0, 1, 2, 3, 5, 7]);
  let page = html();
  assert.match(page, /data-group-reveal="[^"]+"[^>]*>\+2 hidden<\/button>/);
  assert.match(page, /class="group-head[^"]*" colspan="2"/);
  app.revealGroup(group.id);
  same(visible(), [0, 1, 2, 3, 4, 5, 6, 7]);
  page = html();
  assert.match(page, />hide 2<\/button>/);
  app.revealGroup(group.id);
  // Every member empty: the first stays so the header can still be reached.
  clearTrack(p(), 3); clearTrack(p(), 5);
  same(visible(), [0, 1, 2, 3, 7]);
  assert.match(html(), />\+3 hidden</);
});

/* ---------- collapsed tracks ---------- */

test('a collapsed track is a slim column (name + note hits); a saved view state that Undo never flips', () => {
  const { h, app, p, visible, html } = lanes();
  h.state.track = 2; h.state.column = 3;
  h.state.undo = [];
  h.ST.perform('toggleTrackCollapsed');
  same(p().viewState.collapsedTracks, ['r2']);
  assert.equal(h.state.column, 0);
  same(visible(), [0, 1, 's2', 3, 4, 5, 6, 7]);
  assert.equal(h.state.undo.length, 0);
  const page = html();
  assert.match(page, /class="track-head track-slim[^"]*"[^>]*data-track="2" data-track-slim/);
  assert.match(page, /<col class="track-slim-col" style="width:58px">/);
  const cell = page.slice(page.indexOf('data-cell="2:0"'), page.indexOf('</td>', page.indexOf('data-cell="2:0"')));
  assert.match(cell, /slim-hit has-note">[A-G]/);
  assert.equal(app.patternColumnWidth({ slim: true }), 58);
  // Left / Right cross a slim column as a whole.
  app.moveGrid(0, 0, 1);
  assert.equal(h.state.track, 3);
  app.moveGrid(0, 0, -1); // back to FX 2 of … no: from column 0 of track 3 Left goes to the slim track
  assert.equal(h.state.track, 2); assert.equal(h.state.column, 0);
  h.ST.mutate('edit', q => { q.patterns[0].steps[0][1] = null; });
  h.ST.perform('undo'); h.ST.perform('redo');
  same(p().viewState.collapsedTracks, ['r2'], 'Undo / Redo keep the view');
  keyEvent(h, 'J', 'KeyJ', { altKey: true, shiftKey: true });
  assert.equal(p().viewState, undefined, '⌥⇧J expands it again');
  app.setTrackCollapsed(1, true); app.setTrackCollapsed(5, true);
  h.ST.perform('expandAllTracks');
  assert.equal(p().viewState, undefined);
});

/* ---------- delete ---------- */

test('deleting an empty track happens at once; one Undo step restores it', async () => {
  const { h, app, p } = lanes();
  h.ST.perform('addTrack');
  h.state.track = 8; h.state.undo = [];
  h.ST.perform('deleteTracks');
  await tick();
  assert.equal(p().tracks.length, 8);
  assert.equal(h.state.modal, null);
  assert.equal(h.state.undo.at(-1).label, 'delete track');
  assert.equal(h.state.track, 7);
  h.ST.perform('undo');
  assert.equal(p().tracks.length, 9);
  h.ST.perform('redo');
  assert.equal(p().tracks.length, 8);
});

test('deleting a track with content asks first with a summary; Cancel keeps it, Delete removes it (⌘⌫)', async () => {
  const { h, app, p } = lanes();
  h.state.track = 3; h.state.undo = [];
  const kick = p().tracks[3];
  const summary = app.contentSummary(p(), [3]);
  assert.equal(summary.notes, 16); assert.equal(summary.patterns, 2);
  keyEvent(h, 'Backspace', 'Backspace', { metaKey: true });
  await tick();
  assert.equal(h.state.modal, 'confirm');
  assert.match(h.element('#overlay').innerHTML, /Kick has 16 notes in 2 patterns/);
  h.element('#confirm-cancel').onclick();
  await tick();
  assert.equal(p().tracks[3], kick);
  assert.equal(h.state.undo.length, 0);
  const done = app.deleteTracks();
  await tick();
  h.element('#confirm-accept').onclick();
  assert.equal(await done, true);
  assert.equal(p().tracks.length, 7);
  assert(!p().tracks.includes(kick));
  assert.equal(p().patterns[0].steps.length, 7);
  assert.equal(h.state.undo.length, 1);
  h.ST.perform('undo');
  assert.equal(p().tracks[3].name, 'Kick');
  assert.equal(p().patterns[0].steps[3].filter(Boolean).length, 8);
  // Automation lanes count too.
  p().patterns[0].automation.push({ id: 'k', track: 3, target: 'volume', points: [{ row: 0, value: .5 }] });
  assert.match(app.describeContent(app.contentSummary(p(), [3])), /16 notes in 2 patterns and 1 automation lane/);
});

test('deleting several selected tracks asks once and is one Undo step; hidden tracks in the range stay', async () => {
  const { h, app, p, names } = lanes();
  clearTrack(p(), 1);
  app.setHideEmpty(true);
  h.state.track = 2; h.state.selection = { track: 0, row: 0 }; h.state.undo = [];
  same(app.laneSelection(), [0, 2]);
  const done = app.deleteTracks();
  await tick();
  assert.match(h.element('#overlay').innerHTML, /2 tracks \(Velvet bass, Warm pad\) have 40 notes in 2 patterns/);
  h.element('#confirm-accept').onclick();
  assert.equal(await done, true);
  assert.equal(names(), 'Glass keys | Kick | Snare | Closed hat | Open hat | Pulse');
  assert.equal(h.state.undo.length, 1);
  assert.equal(h.state.undo[0].label, 'delete tracks');
  h.ST.perform('undo');
  assert.equal(p().tracks.length, 8);
  // Never every track.
  h.state.track = 7; h.state.selection = { track: 0, row: 0 };
  app.setHideEmpty(false);
  assert.equal(await app.deleteTracks(), false);
  assert.equal(p().tracks.length, 8);
});

test('deleting a group asks: ungroup (keep tracks) or delete the group and its tracks; each one Undo step', async () => {
  const { h, app, p, names } = lanes();
  h.state.track = 3; h.state.selection = { track: 6, row: 0 };
  h.ST.perform('groupTracks');
  const group = G.list(p())[0];
  h.state.undo = [];
  let choice = app.deleteGroup(group.id);
  await tick();
  assert.match(h.element('#overlay').innerHTML, /Group 1 has 4 tracks with 104 notes in 2 patterns/);
  h.element('#group-delete-ungroup').onclick();
  assert.equal(await choice, 'ungroup');
  assert.equal(G.list(p()).length, 0);
  assert.equal(p().tracks.length, 8);
  assert(!p().routing.buses.some(b => b.id === group.busID));
  h.ST.perform('undo');
  assert.equal(G.list(p()).length, 1);
  // On a collapsed group ⌘⌫ asks about the group.
  app.setCollapsed(group.id, true);
  h.state.track = 4; h.state.selection = null;
  h.ST.perform('deleteTracks');
  await tick();
  assert.match(h.element('#overlay').innerHTML, /Delete group/);
  h.element('#group-delete-all').onclick();
  await tick();
  assert.equal(names(), 'Velvet bass | Glass keys | Warm pad | Pulse');
  assert.equal(G.list(p()).length, 0);
  assert(!p().routing.buses.some(b => b.id === group.busID), 'the bus goes with the group');
  assert.equal(h.state.undo.length, 1);
  h.ST.perform('undo');
  assert.equal(p().tracks.length, 8);
  same(G.members(p(), G.list(p())[0]), [3, 4, 5, 6]);
  // Cancel changes nothing.
  choice = app.deleteGroup(group.id);
  await tick();
  h.element('#group-delete-cancel').onclick();
  assert.equal(await choice, false);
  assert.equal(p().tracks.length, 8);
});

/* ---------- drag and drop ---------- */

// Fake header boxes: column i spans [100 * i, 100 * i + 100).
function fakeDOM(h, specs, view = 'pattern') {
  const nodes = specs.map((spec, i) => ({ dataset: spec.dataset, classList: { add() {}, remove() {}, toggle() {} },
    getBoundingClientRect: () => ({ left: 100 * i, right: 100 * i + 100, top: 0, bottom: 300, width: 100, height: 300 }),
    closest(selector) { return spec.matches.some(m => selector.includes(m)) ? this : null; } }));
  const doc = h.win.document;
  const created = [];
  doc.querySelectorAll = selector => selector.includes(view === 'mixer' ? '#mixer-channels [data-mixer-channel]' : '#tracker thead') ? nodes : selector.startsWith('[data-scroll]') || selector.includes('lane-drag-source') || selector.startsWith('#') ? [] : [];
  const box = { getBoundingClientRect: () => ({ left: 0, right: 2000, top: 0, height: 300 }), get scrollLeft() { return 0; }, set scrollLeft(value) {} };
  const original = doc.querySelector;
  doc.querySelector = selector => selector === '.tracker-wrap' || selector === '#mixer-channels' ? box : selector.startsWith('#tracker th') || selector.startsWith('#mixer-channels [') ? null : original(selector);
  doc.createElement = () => { const node = { style: { setProperty() {} }, classList: { toggle() {}, add() {}, remove() {} }, setAttribute() {}, appendChild() {}, remove() { node.removed = true; } }; created.push(node); return node; };
  doc.body.appendChild = () => {};
  return { nodes, created };
}
const pointer = (target, x, extra = {}) => ({ button: 0, clientX: x, clientY: 10, target, preventDefault() {}, stopPropagation() {}, ...extra });

test('dropTarget: halves of a column pick the side; group columns decide membership; groups never nest', () => {
  const { h, p } = lanes();
  h.state.track = 3; h.state.selection = { track: 4, row: 0 };
  h.ST.perform('groupTracks');
  const g = G.list(p())[0].id, D = h.win.STTrackLanes.dropTarget;
  const cols = [0, 1, 2, 3, 4, 5].map(i => ({ left: i * 100, right: i * 100 + 100, kind: 'track', track: i, group: [3, 4].includes(i) ? g : null }));
  same(D(p(), cols, 120, { tracks: [0] }), { before: 1, group: null, x: 100 });
  same(D(p(), cols, 180, { tracks: [0] }), { before: 2, group: null, x: 200 });
  same(D(p(), cols, 420, { tracks: [0] }), { before: 4, group: g, x: 400 }, 'between members joins');
  same(D(p(), cols, 480, { tracks: [0] }), { before: 5, group: g, x: 500 }, 'right half of the last member joins at the end');
  same(D(p(), cols, 520, { tracks: [0] }), { before: 5, group: null, x: 500 }, 'left half of the next column is outside');
  same(D(p(), cols, 950, { tracks: [0] }), { before: 6, group: null, x: 600 }, 'past the end');
  same(D(p(), cols, 430, { tracks: [0], group: 'other' }), { before: 3, group: 'other', x: 300 }, 'a group snaps to a group edge');
  same(D(p(), cols, 470, { tracks: [0], group: 'other' }), { before: 5, group: 'other', x: 500 });
  same(D(p(), cols, 120, { tracks: [3, 4], group: 'other' }), { before: 1, group: 'other', x: 100 });
  // Mixer: the group strip precedes its members; its right half joins at the start.
  const mixer = [{ left: 0, right: 100, kind: 'track', track: 2, group: null }, { left: 100, right: 200, kind: 'groupHead', group: g },
    { left: 200, right: 300, kind: 'track', track: 3, group: g }, { left: 300, right: 400, kind: 'end' }];
  same(D(p(), mixer, 120, { tracks: [0] }), { before: 3, group: null, x: 100 });
  same(D(p(), mixer, 180, { tracks: [0] }), { before: 3, group: g, x: 200 });
  same(D(p(), mixer, 350, { tracks: [0] }), { before: 8, group: null, x: 300 }, 'Master: after the last track');
});

test('dragging a track header reorders it as one Undo step; into and out of a group; Esc cancels', () => {
  const { h, app, p, names } = lanes();
  h.state.track = 3; h.state.selection = { track: 4, row: 0 };
  h.ST.perform('groupTracks');
  const g = G.list(p())[0];
  h.state.selection = null; h.state.track = 0; h.state.undo = [];
  const specs = p().tracks.map((_, i) => ({ dataset: { track: String(i) }, matches: ['th[data-track]'] }));
  const { nodes } = fakeDOM(h, specs);
  const drag = h.win.STTrackLanes.gesture;
  // Velvet bass onto the right half of the Kick: into the group, after the Kick.
  drag.start(pointer(nodes[0], 50));
  drag.move(pointer(nodes[0], 52));
  assert.equal(drag.active.active, false, 'below the threshold it is still a click');
  drag.move(pointer(nodes[0], 360));
  assert.equal(drag.active.active, true);
  same(drag.active.target, { before: 4, group: g.id, x: 400 });
  assert.notEqual(drag.end(pointer(nodes[0], 360)), false);
  assert.equal(names(), 'Glass keys | Warm pad | Kick | Velvet bass | Snare | Closed hat | Open hat | Pulse');
  same(G.members(p(), g), [2, 3, 4]);
  assert.equal(p().tracks[h.state.track].name, 'Velvet bass', 'the moved track is selected');
  assert.equal(h.state.undo.length, 1);
  assert.equal(h.state.undo[0].label, 'move track');
  // Notes follow the track.
  assert.equal(p().patterns[0].steps[3].filter(Boolean).length, 16);
  // Out again: to the end.
  drag.start(pointer(nodes[3], 350));
  drag.move(pointer(nodes[3], 790));
  drag.end(pointer(nodes[3], 790));
  assert.equal(names(), 'Glass keys | Warm pad | Kick | Snare | Closed hat | Open hat | Pulse | Velvet bass');
  assert.equal(p().tracks[7].groupID, undefined);
  // Esc cancels.
  drag.start(pointer(nodes[0], 50));
  drag.move(pointer(nodes[0], 650));
  const esc = { key: 'Escape', preventDefault() { this.prevented = true; }, stopPropagation() {}, stopImmediatePropagation() {} };
  drag.key(esc);
  assert.equal(esc.prevented, true);
  assert.equal(drag.active, null);
  assert.equal(drag.end(pointer(nodes[0], 650)), undefined);
  assert.equal(h.state.undo.length, 2);
  h.ST.perform('undo'); h.ST.perform('undo');
  assert.equal(names(), 'Velvet bass | Glass keys | Warm pad | Kick | Snare | Closed hat | Open hat | Pulse');
  h.ST.perform('redo');
  assert.equal(p().tracks[3].name, 'Velvet bass');
});

test('dragging a group header moves the whole group; dropping a track where it is changes nothing', () => {
  const { h, app, p, names } = lanes();
  h.state.track = 3; h.state.selection = { track: 4, row: 0 };
  h.ST.perform('groupTracks');
  const g = G.list(p())[0];
  h.state.selection = null; h.state.track = 0; h.state.undo = [];
  const specs = p().tracks.map((_, i) => ({ dataset: { track: String(i) }, matches: ['th[data-track]'] }));
  const { nodes } = fakeDOM(h, specs);
  const head = { dataset: { groupId: g.id }, closest(selector) { return selector.includes('th.group-head') ? this : null; } };
  const drag = h.win.STTrackLanes.gesture;
  drag.start(pointer(head, 350));
  drag.move(pointer(head, 30));
  drag.end(pointer(head, 30));
  assert.equal(names(), 'Kick | Snare | Velvet bass | Glass keys | Warm pad | Closed hat | Open hat | Pulse');
  same(G.members(p(), g), [0, 1]);
  assert.equal(h.state.undo.at(-1).label, 'move group');
  assert.equal(p().tracks[h.state.track].name, 'Velvet bass', 'the cursor stays on its track');
  const undo = h.state.undo.length;
  assert.equal(app.moveLanes([5], 5, null), false, 'no change, no Undo step');
  assert.equal(h.state.undo.length, undo);
});

test('mixer strips drag like headers (into a group via its strip) and have delete buttons and menu items', async () => {
  const { h, app, p, names } = lanes();
  h.state.track = 3; h.state.selection = { track: 4, row: 0 };
  h.ST.perform('groupTracks');
  const g = G.list(p())[0];
  h.state.selection = null; h.state.track = 0; h.state.undo = []; h.state.view = 'mixer';
  // Strips: 0 1 2 [group] 3 4 5 6 7 master.
  const specs = [0, 1, 2].map(i => ({ dataset: { mixerOwner: `track:r${i}` }, matches: ['[data-mixer-channel]', '.studio-channel-title'] }))
    .concat([{ dataset: { mixerOwner: `bus:${g.busID}` }, matches: ['[data-mixer-channel]', '.studio-channel-title'] }])
    .concat([3, 4, 5, 6, 7].map(i => ({ dataset: { mixerOwner: `track:${p().tracks[i].routingID}` }, matches: ['[data-mixer-channel]', '.studio-channel-title'] })))
    .concat([{ dataset: { mixerOwner: 'master' }, matches: ['[data-mixer-channel]'] }]);
  const { nodes } = fakeDOM(h, specs, 'mixer');
  const drag = h.win.STTrackLanes.gesture;
  drag.start(pointer(nodes[7], 750)); // Open hat strip … onto the right half of the group strip: first member
  drag.move(pointer(nodes[7], 380));
  drag.end(pointer(nodes[7], 380));
  assert.equal(names(), 'Velvet bass | Glass keys | Warm pad | Open hat | Kick | Snare | Closed hat | Pulse');
  same(G.members(p(), g), [3, 4, 5]);
  assert.equal(p().routing.outputs[`track:${p().tracks[3].routingID}`], `bus:${g.busID}`, 'routing follows membership');
  assert.equal(h.state.undo.length, 1);
  // The group strip moves the group.
  drag.start(pointer(nodes[3], 350));
  drag.move(pointer(nodes[3], 20));
  drag.end(pointer(nodes[3], 20));
  assert.equal(names(), 'Open hat | Kick | Snare | Velvet bass | Glass keys | Warm pad | Closed hat | Pulse');
  // Strip delete buttons and menus.
  assert.match(h.win.STTrackLanes.stripDeleteButton('track:r1', 'Glass keys', 'track', 0), /data-strip-delete="track:r1"/);
  const strip = { dataset: { mixerOwner: `bus:${g.busID}` }, closest(selector) { return selector.includes('[data-mixer-channel]') ? this : null; } };
  const items = h.win.STTrackLanes.contextItems({ target: strip });
  assert.equal(items[1].title, 'Delete group…');
  // ⌘⌫ in the mixer on a track strip: an empty track goes at once.
  h.ST.perform('addTrack');
  h.state.view = 'mixer'; h.state.mixerOwner = `track:${p().tracks[8].routingID}`; h.state.track = 8;
  h.ST.perform('deleteTracks');
  await tick();
  assert.equal(p().tracks.length, 8);
  h.state.mixerOwner = `bus:${g.busID}`;
  h.ST.perform('deleteTracks');
  await tick();
  assert.match(h.element('#overlay').innerHTML, /Delete group/);
  h.element('#group-delete-cancel').onclick();
});

(async () => {
  let failed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log(`ok   ${name}`); } catch (error) { failed++; console.error(`FAIL ${name}\n${error.stack}`); }
  }
  console.log(`track lanes: ${tests.length - failed} passed${failed ? `, ${failed} failed` : ''}`);
  if (failed) process.exitCode = 1;
})();
