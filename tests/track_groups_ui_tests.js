'use strict';
// Track groups in the real pattern editor (app.js): grouping, the group header row, collapsed
// overview columns, cursor movement and selection across groups, ordinary editing on members
// and Pad keys.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { session } = require('./helpers/mk3-session');
const tests = [], test = (name, run) => tests.push([name, run]);
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };

// Demo project (8 tracks), tracks 3..6 (Kick, Snare, Closed hat, Open hat) grouped as "Drums".
function grouped() {
  const h = session(), app = h.win.STApp;
  h.state.view = 'pattern';
  h.state.track = 3; h.state.selection = { track: 6, row: 0 };
  assert.equal(h.ST.perform('groupTracks'), true);
  const group = h.win.STGroups.list(h.state.project)[0];
  return { h, app, group, p: () => h.state.project };
}
const html = h => { h.ST.refresh(); return h.element('#app').innerHTML; };

test('grouping the selected tracks routes them to a new group bus as one Undo step', () => {
  const { h, group, p } = grouped();
  assert.equal(group.name, 'Group 1');
  same(h.win.STGroups.members(p(), group), [3, 4, 5, 6]);
  for (const i of [3, 4, 5, 6]) assert.equal(p().routing.outputs[`track:${p().tracks[i].routingID}`], `bus:${group.busID}`);
  assert.equal(h.state.undo.at(-1).label, 'group tracks');
  const page = html(h);
  assert.match(page, /class="group-head[^"]*" colspan="4"/, 'one header spans the four members');
  assert.match(page, /data-group-meter="bus:/);
  assert.match(page, /id="pad-keys"/, 'Pad keys appear once the session has a group');
  h.ST.perform('undo');
  assert.equal(h.win.STGroups.list(h.state.project).length, 0);
});

test('a collapsed group is one overview column listing the members that hit on each row', () => {
  const { h, app, group, p } = grouped();
  app.setCollapsed(group.id, true);
  const columns = app.patternColumns();
  assert.equal(columns.length, 5);
  same(columns[3].members, [3, 4, 5, 6]);
  const page = html(h);
  assert.match(page, /class="track-head group-overview/);
  // Row 0 of the demo pattern: Kick and Closed hat.
  const row0 = page.slice(page.indexOf('data-cell="3:0"'), page.indexOf('</td>', page.indexOf('data-cell="3:0"')));
  assert.match(row0, /data-member-track="3"[^>]*>Kick C–2</, 'a pitch other than the pad note is shown');
  assert.match(row0, /data-member-track="5"[^>]*>Closed hat</);
  assert.doesNotMatch(row0, /Snare/);
  assert.doesNotMatch(page, /data-cell="4:0"/, 'members are hidden while collapsed');
  assert.equal(h.state.undo.at(-1).label, 'group tracks', 'collapsing is not an Undo step');
  assert.equal(p().groups[0].collapsed, true);
  h.ST.perform('undo'); h.ST.perform('redo');
  assert.equal(h.state.project.groups[0].collapsed, true, 'Undo/Redo keep the view');
});

test('a collapsed overview keeps a fixed column width and never stretches to fill a session with few tracks', () => {
  const { h, app, group, p } = grouped();
  app.setCollapsed(group.id, true);
  const width = app.GROUP_OVERVIEW_WIDTH;
  assert(width >= 222 && width <= 320, `overview about a track column wide (${width}px)`);
  let page = html(h);
  const colgroup = page.slice(page.indexOf('<colgroup>'), page.indexOf('</colgroup>'));
  same([...colgroup.matchAll(/<col(?:\s[^>]*)?>/g)].map(m => m[0]), ['<col style="width:40px">', '<col>', '<col>', '<col>',
    `<col class="group-overview-col" style="width:${width}px">`, '<col>'], 'track columns share the spare width; the overview has its own');
  assert.doesNotMatch(page, /tracker-fill/, 'no filler while track columns take the spare width');
  // The grid uses automatic table layout, where a cell's min-content width overrides its <col>: the hit chips must
  // not contribute (a row with many hits would otherwise widen the overview again).
  const css = fs.readFileSync(path.join(__dirname, '../ui/groups.css'), 'utf8');
  assert.match(/\.group-hits \{([^}]*)\}/.exec(css)[1], /contain:inline-size/);
  assert.match(/\.group-hits \{([^}]*)\}/.exec(css)[1], /flex-wrap:nowrap/, 'one line per row');
  // Only the group (all eight tracks): a filler column after it takes the rest of the window.
  h.state.track = 0; h.state.selection = { track: 7, row: 0 };
  app.setCollapsed(group.id, false);
  h.ST.mutate('regroup', q => { h.win.STGroups.dissolve(q, group.id); });
  assert.equal(h.ST.perform('groupTracks'), true);
  const all = h.win.STGroups.list(p())[0];
  app.setCollapsed(all.id, true);
  page = html(h);
  const cols = [...page.slice(page.indexOf('<colgroup>'), page.indexOf('</colgroup>')).matchAll(/<col(?:\s[^>]*)?>/g)].map(m => m[0]);
  same(cols, ['<col style="width:40px">', `<col class="group-overview-col" style="width:${width}px">`, '<col>']);
  const rows = page.split('<tr').length - 1;
  assert.equal(page.split('class="tracker-fill"').length - 1, rows, 'every row (group header, track header, steps) ends with the filler');
  // Many tracks: the virtual grid still sizes the overview from the same width.
  h.ST.mutate('more tracks', q => { while (q.tracks.length < 40) h.ST.appendInstrumentTrack(q, 0); });
  page = html(h);
  assert.match(page, /class="tracker is-virtual"/);
  assert.match(page, new RegExp(`<colgroup><col style="width:40px"><col style="width:${width}px">`), 'virtual grid: overview column width');
  assert.doesNotMatch(page, /tracker-fill/);
});

test('the cursor crosses a collapsed group as one column and selections cover all its members', () => {
  const { h, app, group } = grouped();
  app.setCollapsed(group.id, true);
  h.state.track = 2; h.state.row = 0;
  app.moveGrid(1, 0);
  assert.equal(h.state.track, 3, 'onto the overview column');
  app.moveGrid(1, 0);
  assert.equal(h.state.track, 7, 'past the hidden members');
  app.moveGrid(-1, 0);
  assert.equal(h.state.track, 3);
  // Copy on the collapsed column copies every member's row; selections widen to the group.
  h.ST.copySelection();
  assert.equal(h.state.clipboard.length, 4);
  h.state.selection = { track: 2, row: 0 }; h.state.track = 3; h.state.row = 3;
  same(h.ST.selectedBounds(), { r0: 0, r1: 3, t0: 2, t1: 6 });
  // Delete clears every member on the row.
  h.state.selection = null; h.state.row = 0;
  h.ST.clearSelection();
  for (const t of [3, 4, 5, 6]) assert.equal(h.ST.pattern().steps[t][0], null);
});

test('expanded members are ordinary tracks: multi-cell selection, copy/paste and transpose', () => {
  const { h, app } = grouped();
  const steps = () => h.ST.pattern().steps;
  // Select Kick..Closed hat rows 0..3 and paste them onto the next bar.
  h.state.track = 3; h.state.row = 0; h.state.selection = { track: 5, row: 3 };
  h.ST.copySelection();
  assert.equal(h.state.clipboard.length, 3);
  h.state.selection = null; h.state.row = 16; h.state.track = 4;
  for (const t of [4, 5, 6]) for (let r = 16; r < 20; r++) steps()[t][r] = null;
  h.ST.pasteSelection();
  assert.equal(steps()[4][16].note, 36, 'the kick step landed on the snare track');
  assert.equal(steps()[4][16].instrument, 4, 'and plays the snare track’s own instrument');
  h.state.track = 4; h.state.row = 16; h.state.selection = null;
  h.ST.transpose(1);
  assert.equal(steps()[4][16].note, 37);
  app.shiftPatternRows(true);
  assert.equal(steps()[4][17].note, 37, 'insert row works on a member like any track');
});

test('Pad keys enter C-4 (or the pad note) on the member a key addresses; off plays pitch on the cursor track', async () => {
  const { h, app, p } = grouped();
  h.state.recording = true; h.state.playing = false; h.state.keyboardPlay = true;
  h.state.track = 3; h.state.row = 2; h.state.editStep = 0;
  h.ST.pattern().steps[4][2] = null; h.ST.pattern().steps[5][2] = null;
  app.setPadKeys(true);
  p().tracks[5].padNote = 62;
  const base = (h.state.octave + 1) * 12;
  assert.equal(app.padKeyNote(base + 1, 'KeyS', true), true);
  h.ST.noteOff('KeyS');
  await tick();
  assert.equal(h.ST.pattern().steps[4][2]?.note, 60, 'S (pad 2) entered C-4 on the second member');
  assert.equal(h.ST.pattern().steps[4][2]?.instrument, p().tracks[4].instrument);
  assert.equal(app.padKeyNote(base + 2, 'KeyX', true), true);
  h.ST.noteOff('KeyX');
  await tick();
  assert.equal(h.ST.pattern().steps[5][2]?.note, 62, 'X (pad 3) used that member’s pad note');
  assert.equal(h.state.track, 5, 'the cursor follows the pad');
  app.setPadKeys(false);
  assert.equal(app.padKeyNote(base + 4, 'KeyC', true), false, 'off: keys play pitch');
});

test('group mute/solo toggle the bus and silence its members in the grid', () => {
  const { h, app, group, p } = grouped();
  app.toggleBus(group.id, 'mute');
  assert.equal(p().routing.buses.find(b => b.id === group.busID).mute, true);
  assert.equal(app.trackSilenced(p(), 4), true);
  assert.equal(app.trackSilenced(p(), 0), false);
  h.ST.perform('undo');
  assert.equal(h.state.project.routing.buses.find(b => b.id === group.busID).mute, false);
});

test('moving tracks joins and leaves groups; ungroup and dissolve keep the tracks', () => {
  const { h, app, group, p } = grouped();
  h.state.track = 2;
  h.ST.perform('moveTrackRight');
  assert.equal(h.state.track, 2, 'a track next to a group joins it at its edge without moving');
  assert.equal(p().tracks[2].groupID, group.id);
  assert.equal(p().routing.outputs[`track:${p().tracks[2].routingID}`], `bus:${group.busID}`);
  h.ST.perform('moveTrackRight');
  assert.equal(h.state.track, 3); assert.equal(p().tracks[3].name, 'Warm pad');
  h.ST.perform('moveTrackLeft'); h.ST.perform('moveTrackLeft');
  assert.equal(p().tracks[2].groupID, undefined, 'the first member moving left leaves the group');
  assert.equal(p().routing.outputs[`track:${p().tracks[2].routingID}`], 'master');
  // A collapsed group is hopped over.
  h.win.STApp.setCollapsed(group.id, true);
  h.ST.perform('moveTrackRight');
  assert.equal(p().tracks[6].name, 'Warm pad'); assert.equal(p().tracks[6].groupID, undefined);
  h.win.STApp.setCollapsed(group.id, false);
  h.state.track = 3;
  h.ST.perform('moveTrackRight'); h.ST.perform('moveTrackRight');
  h.state.track = 5;
  h.ST.perform('ungroupTrack');
  assert.equal(p().tracks.find(t => t.name === 'Warm pad').groupID, undefined);
  h.state.track = 3;
  h.ST.perform('dissolveGroup');
  assert.equal(h.win.STGroups.list(p()).length, 0);
  assert.equal(p().tracks.length, 8);
  assert(p().tracks.every(t => p().routing.outputs[`track:${t.routingID}`] !== `bus:${group.busID}`));
});

test('removing the last member of a group removes the group and its bus', () => {
  const { h, p } = grouped();
  h.state.track = 3; h.state.selection = null;
  h.ST.perform('ungroupTrack'); h.state.track = 3; h.ST.perform('ungroupTrack'); h.state.track = 3; h.ST.perform('ungroupTrack');
  const group = h.win.STGroups.list(p())[0];
  const member = h.win.STGroups.members(p(), group)[0];
  h.state.track = member;
  h.win.STApp.removeTrack();
  assert.equal(h.win.STGroups.list(p()).length, 0);
  assert(!p().routing.buses.some(b => b.id === group.busID));
});

(async () => {
  let failed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log('ok  ', name); } catch (error) { failed++; console.error('FAIL', name, '\n', error); }
  }
  if (failed) process.exit(1);
  console.log(`track groups UI: ${tests.length} passed`);
})();
