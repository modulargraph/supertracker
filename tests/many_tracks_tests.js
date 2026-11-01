'use strict';
// Sessions have no fixed track limit (only TRACK_SANITY_LIMIT = 1024, shared with the engine and Swift).
// Real app.js / studio.js / mk3.js: adding tracks past sixteen, the sanity bound, horizontal virtualization
// of the pattern grid and mixer at 200+ tracks, and MK3 track banks of eight over any number of tracks.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm'), assert = require('node:assert/strict');
const { session } = require('./helpers/mk3-session');
const { harness: studioHarness } = require('./helpers/studio-session');
const tests = [], test = (name, run) => tests.push([name, run]);
const shifted = (h, name) => { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); };
const count = (html, pattern) => (html.match(pattern) || []).length;

function withTracks(h, total) {
  while (h.state.project.tracks.length < total) h.ST.appendInstrumentTrack(h.state.project, 0);
  return h;
}
// The grid's scroll container as refresh() sees it before replacing the DOM.
function viewport(h, left = 0, width = 1200) { const wrap = h.element('.tracker-wrap'); wrap.scrollLeft = left; wrap.clientWidth = width; return wrap; }
function rowCells(h, row = 0) { return [...h.element('#app').innerHTML.matchAll(new RegExp(`data-cell="(\\d+):${row}"`, 'g'))].map(m => Number(m[1])); }

test('the UI adds tracks past sixteen and only the 1024-track sanity bound refuses', async () => {
  const h = session();
  assert.equal(h.win.TRACK_SANITY_LIMIT, 1024);
  while (h.state.project.tracks.length < 40) h.ST.perform('addTrack');
  assert.equal(h.state.project.tracks.length, 40);
  assert(h.state.project.patterns.every(pattern => pattern.steps.length === 40));
  assert.equal(h.state.track, 39, 'the new track is selected');
  assert.equal(h.state.project.tracks[39].name, 'Track 40');
  withTracks(h, 1024);
  const undo = h.state.undo.length;
  h.ST.perform('addTrack');
  assert.equal(h.state.project.tracks.length, 1024); assert.equal(h.state.undo.length, undo);
  assert.match(h.element('#toast-region').innerHTML, /This session already has 1024 tracks/);
  assert.throws(() => h.ST.appendInstrumentTrack(h.state.project, 0), /already has 1024 tracks/);
  assert.equal(h.ST.addInstrument(), false);
});

test('a normal-width session renders every pattern column (no virtualization)', () => {
  const h = withTracks(session(), 16); h.state.view = 'pattern'; viewport(h); h.ST.refresh();
  assert.deepEqual(rowCells(h), Array.from({ length: 16 }, (_, i) => i));
  assert.doesNotMatch(h.element('#app').innerHTML, /tracker-spacer|is-virtual/);
});

test('200 tracks build DOM only for visible columns, follow horizontal scroll and keep the cursor column', () => {
  const h = withTracks(session(), 200); h.state.view = 'pattern'; h.state.track = 0;
  viewport(h, 0, 1200); h.ST.refresh();
  let cells = rowCells(h), html = h.element('#app').innerHTML;
  assert(cells.length <= 12, `only visible columns plus overscan are built (got ${cells.length})`);
  assert.deepEqual(cells, Array.from({ length: cells.length }, (_, i) => i));
  assert.match(html, /class="tracker is-virtual"[^>]*data-virtual-columns="200"/);
  assert.equal(count(html, /<th class="track-head/g), cells.length, 'headers match the built columns');
  const width = Number(/style="width:(\d+)px"/.exec(html.slice(html.indexOf('class="tracker is-virtual"')))[1]);
  assert.equal(width, 40 + 200 * 182, 'spacers keep the full scroll width');
  // Scroll to the middle: the built range follows the viewport and the cursor column stays built.
  viewport(h, 100 * 182, 1200); h.ST.refresh();
  cells = rowCells(h);
  assert(cells.includes(0), 'the cursor column is always built so moves can scroll it into view');
  assert(cells.includes(100) && cells.includes(105) && !cells.includes(50) && !cells.includes(150));
  assert(cells.length <= 13);
  // Keyboard moves to an off-screen track build it, then scroll it into view.
  viewport(h, 0, 1200); h.ST.refresh(); h.state.track = 0;
  h.state.track = 199; h.ST.refresh();
  assert(rowCells(h).includes(199), 'moving the cursor to the last track builds its column');
});

test('pattern refresh time stays flat from 16 to 200 tracks (visible columns only)', () => {
  const timings = {};
  for (const total of [16, 64, 200]) {
    const h = withTracks(session(), total); h.state.view = 'pattern'; viewport(h, 0, 1200);
    for (let i = 0; i < 3; i++) h.ST.refresh();
    const runs = 10, start = process.hrtime.bigint();
    for (let i = 0; i < runs; i++) h.ST.refresh();
    timings[total] = Number(process.hrtime.bigint() - start) / 1e6 / runs;
    assert(rowCells(h).length <= (total > 32 ? 12 : total));
  }
  console.log(`  pattern refresh (64 rows): ${Object.entries(timings).map(([n, ms]) => `${n} tracks ${ms.toFixed(2)} ms`).join(' · ')}`);
  assert(timings[200] < Math.max(timings[16] * 4, 20), 'refresh at 200 tracks is not proportional to the track count');
});

test('a wide mixer builds only visible strips (plus the selected one), so meters touch only those', () => {
  const h = studioHarness(); while (h.S.state.project.tracks.length < 200) h.S.appendInstrumentTrack(h.S.state.project, 0);
  h.S.state.view = 'mixer'; h.S.state.track = 150;
  const box = h.element('mixer-channels'); box.scrollLeft = 0; box.clientWidth = 1280;
  const html = h.window.STStudio.renderMixer(), strips = count(html, /data-track-meter="/g);
  assert(strips <= 14, `visible strips only (got ${strips})`);
  assert.match(html, /data-track-meter="150"/, 'the selected strip is built');
  assert.match(html, /studio-channels is-virtual/);
  assert.match(html, /studio-channel-spacer/);
  const small = studioHarness(); small.S.state.view = 'mixer';
  assert.doesNotMatch(small.window.STStudio.renderMixer(), /studio-channel-spacer|is-virtual/, 'small sessions are unchanged');
});

for (const total of [20, 200]) test(`MK3 group buttons page banks of eight through ${total} tracks`, () => {
  const h = withTracks(session(), total); h.MK3.subscribe(() => {});
  const banks = Math.ceil(total / 8);
  h.state.track = 0; h.MK3.frame(); assert.equal(h.MK3.state.bank, 0);
  for (let i = 1; i < banks + 3; i++) shifted(h, 'arrowRight');
  assert.equal(h.MK3.state.bank, banks - 1, 'paging stops at the last bank');
  const last = (banks - 1) * 8, present = total - last;
  const groups = h.MK3.frame().leds.groups;
  assert.equal(groups.filter(Boolean).length, present, 'only existing tracks in the last bank light');
  h.press('groupA'); assert.equal(h.state.track, last, 'group A selects the bank’s first track');
  h.press(`group${'ABCDEFGH'[present - 1]}`); assert.equal(h.state.track, total - 1);
  if (present < 8) { h.press(`group${'ABCDEFGH'[present]}`); assert.equal(h.state.track, total - 1, 'a group beyond the last track is ignored'); }
  shifted(h, 'arrowLeft'); assert.equal(h.MK3.state.bank, banks - 2);
  h.press('groupC'); assert.equal(h.state.track, (banks - 2) * 8 + 2);
  // A track selected in the app moves the bank to floor(track / 8).
  h.state.track = total - 3; h.MK3.frame(); assert.equal(h.MK3.state.bank, Math.floor((total - 3) / 8));
  // Mute pads cover the sixteen tracks (two banks) containing the current bank.
  h.MK3.state.padMode = 'keyboard'; // instrument pads mute instruments; other pad modes mute tracks
  h.input({ type: 'button', name: 'mute', pressed: true });
  const base = Math.floor(h.MK3.state.bank / 2) * 16, target = Math.min(total - 1, base + 1);
  h.down(target - base + 1); h.up(target - base + 1);
  h.input({ type: 'button', name: 'mute', pressed: false });
  assert.equal(h.state.project.tracks[target].mute, true, `pad ${target - base + 1} mutes track ${target + 1}`);
  if (base) assert.equal(!!h.state.project.tracks[1].mute, false, 'not a track in the first sixteen');
});

(async () => {
  let failures = 0;
  for (const [name, run] of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
  console.log(`Many tracks: ${tests.length - failures}/${tests.length} groups passed.`);
  if (failures) process.exitCode = 1;
})();
