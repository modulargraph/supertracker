'use strict';
// Maschine MK3 with track groups (kits): group buttons A–H are entries (an ungrouped track or a whole group),
// PAD MODE pads are the cursor group's member tracks, Home shows collapsed groups as one column, and CHANNEL /
// PLUG-IN gain a Group level (the group bus). Real app + MK3 modules; no native host or hardware.
const assert = require('node:assert/strict');
const { session } = require('./helpers/mk3-session');
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 60; i++) await Promise.resolve(); };
const tests = [], test = (name, run) => tests.push([name, run]);
const held = (h, name, fn) => { h.input({ type: 'button', name, pressed: true }); fn(); h.input({ type: 'button', name, pressed: false }); };
const labels = h => h.MK3.page().buttons.map(b => b?.label ?? null);

// Demo project (8 tracks); tracks 3..6 (Kick, Snare, Closed hat, Open hat) grouped as "Drums".
function grouped(extra = 0) {
  const h = session();
  while (h.state.project.tracks.length < 8 + extra) h.ST.appendInstrumentTrack(h.state.project, 0);
  h.state.track = 3; h.state.selection = { track: 6, row: 0 };
  assert.equal(h.ST.perform('groupTracks'), true);
  const group = h.win.STGroups.list(h.state.project)[0];
  h.ST.mutate('rename', p => h.win.STGroups.rename(p, group.id, 'Drums'));
  h.state.track = 0; h.state.selection = null;
  h.MK3.subscribe(() => {});
  h.MK3.frame();
  const bus = () => h.state.project.routing.buses.find(item => item.id === group.busID);
  return { h, group: () => h.win.STGroups.byID(h.state.project, group.id), bus, owner: `bus:${group.busID}` };
}
const noteOns = h => h.calls.filter(call => call.command === 'note.on');

test('group buttons A–H are entries: ungrouped tracks and one button per group', () => {
  const { h, group } = grouped();
  same(h.MK3.frame().leds.groups.map(Boolean), [true, true, true, true, true, false, false, false], 'five entries: 0, 1, 2, Drums, 7');
  h.press('groupD');
  assert.equal(h.state.track, 3, 'a group selects its first member');
  h.press('groupE');
  assert.equal(h.state.track, 7, 'the entry after the group is track 8');
  // The group remembers the member selected last.
  h.state.track = 5; h.MK3.frame();
  h.press('groupA'); assert.equal(h.state.track, 0);
  h.press('groupD'); assert.equal(h.state.track, 5, 'back to the last selected member');
  // LEDs: group colour, bright while it holds the cursor track; dim otherwise.
  let leds = h.MK3.frame().leds.groups;
  const bright = leds[3], dim = h.MK3.frame().leds.groups[0];
  assert.ok(bright && dim);
  h.state.track = 0; leds = h.MK3.frame().leds.groups;
  assert.notEqual(leds[3], bright, 'the group dims when the cursor leaves it');
  assert.ok(leds[3], 'still lit');
  assert.ok(group().color);
});

test('Shift + ◄ ► pages banks of eight entries; the bank follows the cursor entry', () => {
  const { h } = grouped(12); // 20 tracks, 4 grouped → 17 entries
  const shifted = name => held(h, 'shift', () => h.press(name));
  assert.equal(h.MK3.state.bank, 0);
  shifted('arrowRight'); assert.equal(h.MK3.state.bank, 1);
  h.press('groupA'); assert.equal(h.state.track, 11, 'entry 9 is track 12 (the group counts once)');
  shifted('arrowRight'); assert.equal(h.MK3.state.bank, 2);
  h.press('groupA'); assert.equal(h.state.track, 19);
  same(h.MK3.frame().leds.groups.map(Boolean), [true, false, false, false, false, false, false, false]);
  shifted('arrowRight'); assert.equal(h.MK3.state.bank, 2, 'paging stops at the last bank');
  h.state.track = 4; h.MK3.frame(); assert.equal(h.MK3.state.bank, 0, 'the bank follows a member of the group');
});

test('MUTE / SOLO + group button toggle the group bus (one Undo step); LED off when muted', () => {
  const { h, bus } = grouped();
  const undo = h.state.undo.length;
  held(h, 'mute', () => h.press('groupD'));
  assert.equal(bus().mute, true);
  assert.ok([3, 4, 5, 6].every(i => !h.state.project.tracks[i].mute), 'member tracks keep their own mute');
  assert.equal(h.state.undo.length, undo + 1);
  assert.equal(h.MK3.frame().leds.groups[3], null, 'muted group is dark');
  held(h, 'solo', () => h.press('groupD'));
  assert.equal(bus().solo, true);
  held(h, 'mute', () => h.press('groupA'));
  assert.equal(h.state.project.tracks[0].mute, true, 'a track entry mutes its track');
  h.ST.perform('undo'); h.ST.perform('undo'); h.ST.perform('undo');
  assert.equal(bus().mute, false);
  // Shift + MUTE clears track and group-bus mutes.
  held(h, 'mute', () => h.press('groupD'));
  held(h, 'shift', () => h.press('mute'));
  assert.equal(bus().mute, false);
});

test('ERASE + group button clears every member of the current pattern in one Undo step', () => {
  const { h } = grouped();
  const pat = () => h.ST.pattern();
  assert.ok([3, 4, 5, 6].some(i => pat().steps[i].some(Boolean)), 'demo drums have notes');
  const other = JSON.stringify(pat().steps[7]), undo = h.state.undo.length;
  held(h, 'erase', () => h.press('groupD'));
  assert.ok([3, 4, 5, 6].every(i => !pat().steps[i].some(Boolean)));
  assert.equal(JSON.stringify(pat().steps[7]), other);
  assert.equal(h.state.undo.length, undo + 1);
  // SELECT + group selects the member without other effects.
  held(h, 'select', () => h.press('groupD'));
  assert.equal(h.state.track, 3);
});

test('PAD MODE: pads are the cursor group’s members; LEDs bright = cursor track, dim = other members', () => {
  const { h } = grouped();
  h.state.track = 4; h.MK3.frame();
  const pads = h.MK3.frame().leds.pads;
  same(pads.map(Boolean), [true, true, true, true, ...Array(12).fill(false)]);
  assert.notEqual(pads[1], pads[0], 'the cursor member is brighter');
  // Pad 3 auditions member 3 (Closed hat) at its pad note and moves the cursor there.
  h.down(3);
  assert.equal(h.state.track, 5);
  const on = noteOns(h).at(-1);
  assert.equal(on.payload.track, 5);
  assert.equal(on.payload.note, 60);
  h.up(3);
  assert.ok(h.calls.some(call => call.command === 'note.off'), 'pad up releases the member');
  // Outside a group the pads are instruments again.
  h.state.track = 0; h.MK3.frame();
  h.down(2); h.up(2);
  assert.equal(h.state.instrument, 1);
  assert.equal(h.state.track, 0);
});

test('recording: a pad enters its pad note on that member at the cursor row with the member’s instrument', async () => {
  const { h } = grouped();
  const p = () => h.state.project, pat = () => h.ST.pattern();
  h.ST.mutate('pad note', q => { q.tracks[5].padNote = 62; });
  h.state.track = 3; h.state.row = 9; h.state.recording = true; h.MK3.frame();
  for (const i of [3, 4, 5, 6]) pat().steps[i][9] = null;
  h.down(3); h.up(3); await tick();
  const step = pat().steps[5][9];
  assert.ok(step, 'note entered on the Closed hat track');
  assert.equal(step.note, 62, 'its pad note');
  assert.equal(step.instrument, p().tracks[5].instrument);
  assert.equal(pat().steps[3][9], null, 'not on the previous cursor track');
  h.state.row = 10; pat().steps[3][10] = null;
  h.down(1); h.up(1); await tick();
  assert.equal(pat().steps[3][10]?.note, 60, 'C-4 by default');
});

test('MUTE / SOLO + pad toggle that member; SELECT + pad selects it without playing', () => {
  const { h, bus } = grouped();
  h.state.track = 3; h.MK3.frame();
  held(h, 'mute', () => { h.down(2); h.up(2); });
  assert.equal(h.state.project.tracks[4].mute, true);
  assert.equal(bus().mute, false);
  held(h, 'solo', () => { h.down(4); h.up(4); });
  assert.equal(h.state.project.tracks[6].solo, true);
  const before = noteOns(h).length;
  held(h, 'select', () => { h.down(3); h.up(3); });
  assert.equal(h.state.track, 5);
  assert.equal(noteOns(h).length, before, 'SELECT does not play');
  // MUTE held shows member mute state on the pads.
  h.input({ type: 'button', name: 'mute', pressed: true });
  const leds = h.MK3.frame().leds.pads;
  h.input({ type: 'button', name: 'mute', pressed: false });
  assert.notEqual(leds[1], leds[0], 'muted member is dark');
});

test('more than 16 members: ◄ ► page the pads in 16s', () => {
  const h = session();
  while (h.state.project.tracks.length < 20) h.ST.appendInstrumentTrack(h.state.project, 0);
  h.state.track = 0; h.state.selection = { track: 17, row: 0 };
  assert.equal(h.ST.perform('groupTracks'), true);
  h.state.selection = null; h.state.track = 0; h.MK3.subscribe(() => {}); h.MK3.frame();
  same(h.MK3.frame().leds.pads.map(Boolean), Array(16).fill(true));
  h.press('arrowRight');
  same(h.MK3.frame().leds.pads.map(Boolean), [true, true, ...Array(14).fill(false)]);
  h.down(2); h.up(2);
  assert.equal(h.state.track, 17);
  h.MK3.frame();
  assert.equal(h.MK3.state.padBank.pad, 1, 'the page follows the cursor member');
  h.state.track = 2; h.MK3.frame();
  assert.equal(h.MK3.state.padBank.pad, 0);
});

test('Home: an expanded group is ordinary columns; a collapsed group is one column for the group bus', () => {
  const { h, group, bus } = grouped();
  h.press('notes');
  let grid = h.MK3.page().data.grid;
  same(grid.tracks.map(t => t.index), [0, 1, 2, 3, 4, 5, 6, 7]);
  h.win.STApp.setCollapsed(group().id, true);
  grid = h.MK3.page().data.grid;
  same(grid.tracks.map(t => t.index), [0, 1, 2, 3, 7]);
  const column = grid.tracks[3];
  assert.equal(column.collapsed, true);
  assert.equal(column.name, 'Drums');
  same(column.members, [3, 4, 5, 6]);
  assert.equal(labels(h)[3], 'Drums');
  // Row 0 of the demo: Kick and Closed hat hit; the summary lists member numbers (and pitches off the pad note).
  const summary = grid.rows.find(r => r.row === 0).cells[3].summary;
  assert.match(summary, /^1(:\S+)? 3(:\S+)?$/);
  // Display button: select (remembered member); MUTE held: the group bus.
  h.state.track = 5; h.MK3.frame(); h.state.track = 0;
  h.press('display4'); assert.equal(h.state.track, 5);
  held(h, 'mute', () => h.press('display4'));
  assert.equal(bus().mute, true);
  assert.equal(h.MK3.page().data.grid.tracks[3].mute, true);
  held(h, 'shift', () => h.press('display4'));
  assert.equal(bus().mute, false, 'Shift + display button mutes / unmutes too');
  // Tilt walks columns: from track 2 onto the group, then past it.
  h.state.track = 2; h.state.column = 4;
  h.press('encoderRight'); assert.equal(h.state.track, 3);
  h.state.column = 4; h.press('encoderRight'); assert.equal(h.state.track, 7);
});

test('PLUG-IN · Group: bus inserts then Levels (volume, pan); one Undo step per gesture', async () => {
  const { h, bus, owner } = grouped();
  h.state.project.audioPlugins = [...(h.state.project.audioPlugins || []), { owner, instanceID: 'drum-comp', name: 'Drum Comp', kind: 'effect', slot: 0, parameters: [] }];
  h.state.track = 0; h.press('plugin');
  same(labels(h).slice(0, 3), ['Master', 'Track', 'Instr.'], 'no Group level outside a group');
  h.state.track = 4; h.MK3.frame();
  same(labels(h).slice(0, 4), ['Master', 'Group', 'Track', 'Instr.']);
  h.press('display2');
  let page = h.MK3.page();
  assert.equal(page.data.level, 'group');
  same(page.data.chain.items.map(item => item.right), ['Insert 1', 'Levels']);
  assert.equal(page.data.chain.items[0].name, 'Drum Comp');
  page.main.turn(1); page = h.MK3.page();
  assert.equal(page.data.name, 'Volume · pan');
  same(page.knobs.slice(0, 2).map(k => k?.label), ['Volume', 'Pan']);
  const undo = h.state.undo.length;
  h.input({ type: 'encoder', index: 0, delta: -.1 });
  await h.MK3.flushEdits(); await tick();
  assert.ok(bus().volume < 1, 'bus volume lowered');
  assert.equal(h.state.undo.length, undo + 1);
  // Leaving the group falls back to Track.
  h.state.track = 0; h.MK3.frame();
  assert.equal(h.MK3.page().data.level, 'track');
});

test('CHANNEL · Group: the group bus levels (volume, pan, mute, solo)', async () => {
  const { h, bus } = grouped();
  h.state.track = 6; h.MK3.frame();
  h.press('channel');
  same(labels(h).slice(0, 4), ['Master', 'Group', 'Track', 'Instr.']);
  h.press('display2');
  const page = h.MK3.page();
  assert.equal(page.data.kind, 'levels');
  assert.equal(page.data.level, 'group');
  assert.equal(page.data.title, 'Drums');
  same(page.knobs.map(k => k?.label ?? null), ['Volume', 'Pan', null, null, 'Mute', 'Solo', null, null]);
  h.input({ type: 'encoder', index: 1, delta: .2 });
  await h.MK3.flushEdits(); await tick();
  assert.ok(bus().pan > 0);
  // Track level still edits the member's own channel.
  h.press('display3');
  assert.equal(h.MK3.page().data.level, 'track');
  assert.equal(h.MK3.page().knobs[0].label, 'Volume');
});

test('MIXER: strips are the bank’s entries; a group strip is its bus', async () => {
  const { h, bus } = grouped();
  h.press('mixer');
  const page = h.MK3.page();
  same(page.data.strips.map(s => s.name), ['Velvet bass', 'Glass keys', 'Warm pad', 'Drums', 'Pulse']);
  h.input({ type: 'encoder', index: 3, delta: -.1 });
  await h.MK3.flushEdits(); await tick();
  assert.ok(bus().volume < 1);
  h.press('display4'); assert.equal(h.state.track, 3);
});

test('PLUG-IN · Group: + FX adds to the group bus chain, not the selected member', () => {
  const { h, owner } = grouped();
  h.state.track = 4; h.MK3.frame();
  h.press('plugin'); h.press('display2');
  assert.equal(h.MK3.page().data.level, 'group');
  const fx = labels(h).indexOf('+ FX');
  assert.ok(fx >= 0, 'Group level offers + FX');
  h.press('display' + (fx + 1));
  assert.equal(h.state.mixerOwner, owner, 'the mixer channel is the group bus');
  assert.equal(h.state.view, 'mixer', 'the add-effect list opens for that channel');
  assert.equal(h.state.track, 4, 'the member stays selected');
  // The Levels entry offers the same.
  h.press('plugin'); h.press('display2'); h.MK3.page().main.turn(9);
  h.state.mixerOwner = null;
  h.press('display' + (labels(h).indexOf('+ FX') + 1));
  assert.equal(h.state.mixerOwner, owner);
});

test('DUPLICATE + group duplicates the whole group: members, bus and its inserts, as one Undo step', async () => {
  const { h, group, owner } = grouped();
  // As in the app (index.html): every track has a routing identity before duplication.
  h.load('routing-project.js'); h.win.STRoutingProject.ensureTrackIDs(h.state.project, h.ST.uid);
  h.load('track-duplicate-project.js'); h.load('track-duplicate.js');
  const memberOwner = `track:${h.state.project.tracks[3].routingID}`;
  h.ST.mutate('inserts', p => {
    p.audioPlugins = [...(p.audioPlugins || []), { owner, kind: 'effect', slot: 0, name: 'Glue', component: 'stfx:compressor', instanceID: 'drum-glue', state: 'glue-state' },
      { owner: memberOwner, kind: 'effect', slot: 0, name: 'Kick EQ', component: 'stfx:eq', instanceID: 'kick-eq', state: 'eq-state' }];
    p.routing.buses.find(bus => bus.id === group().busID).volume = .7;
  });
  const before = JSON.stringify(h.state.project), undo = h.state.undo.length, notes = cols => cols.map(c => JSON.stringify(c.map(step => step && [step.note, step.velocity, step.gate]))), steps = notes(h.ST.pattern().steps.slice(3, 7));
  h.state.track = 5; h.MK3.frame();
  held(h, 'duplicate', () => h.press('groupD'));
  for (let i = 0; i < 30; i++) { await tick(); h.advance(50); }
  const p = h.state.project, groups = h.win.STGroups.list(p);
  assert.equal(groups.length, 2, 'a second group');
  assert.equal(p.tracks.length, 12, 'four member copies');
  const copy = groups[1], members = h.win.STGroups.members(p, copy);
  same(members, [7, 8, 9, 10], 'the copy follows the source group');
  same(members.map(i => p.tracks[i].name), ['Kick copy', 'Snare copy', 'Closed hat copy', 'Open hat copy']);
  same(h.win.STGroups.members(p, group()), [3, 4, 5, 6], 'the source group is unchanged');
  assert.equal(p.tracks[11].name, 'Pulse', 'later tracks move after the copy');
  same(notes(h.ST.pattern().steps.slice(7, 11)), steps, 'pattern columns are copied');
  for (const i of members) assert.notEqual(p.tracks[i].instrument, p.tracks[i - 4].instrument, 'with independent instruments');
  const bus = p.routing.buses.find(item => item.id === copy.busID);
  assert.ok(bus && copy.busID !== group().busID, 'a new group bus');
  assert.equal(bus.volume, .7, 'with the source bus mix');
  for (const i of members) assert.equal(p.routing.outputs[`track:${p.tracks[i].routingID}`], `bus:${copy.busID}`, 'copies feed the new bus');
  const busInserts = p.audioPlugins.filter(plugin => plugin.owner === `bus:${copy.busID}`);
  same(busInserts.map(plugin => [plugin.name, plugin.state]), [['Glue', 'glue-state']], 'the bus inserts are copied');
  assert.notEqual(busInserts[0].instanceID, 'drum-glue', 'as a new instance');
  assert.ok(p.audioPlugins.some(plugin => plugin.owner === `track:${p.tracks[7].routingID}` && plugin.name === 'Kick EQ' && plugin.instanceID !== 'kick-eq'), 'member inserts too');
  assert.equal(p.tracks[h.state.track].name, 'Closed hat copy', 'the cursor moves to the copy of its member');
  assert.equal(h.state.undo.length, undo + 1, 'one Undo step');
  assert.equal(h.state.undo.at(-1).label, 'Duplicate group');
  h.ST.perform('undo');
  assert.equal(JSON.stringify(h.state.project), before, 'Undo removes the whole copy');
});

test('no kit code paths remain in the MK3 modules', () => {
  const fs = require('node:fs'), path = require('node:path'), dir = path.join(__dirname, '../ui');
  for (const file of fs.readdirSync(dir).filter(name => /^mk3-.*\.js$/.test(name))) {
    const text = fs.readFileSync(path.join(dir, file), 'utf8');
    assert.doesNotMatch(text, /STKits|kitView|kitEvents|soundID|selectedSoundID|'sound:/, file);
  }
});

(async () => {
  let passed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log('PASS ' + name); passed++; } catch (error) { console.error('FAIL ' + name + '\n' + error.stack); process.exitCode = 1; }
  }
  console.log(`${passed}/${tests.length} MK3 track group groups passed`);
})();
