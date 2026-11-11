'use strict';
// Track groups in the real mixer (studio.js): the group bus strip comes before its member strips,
// members hide while the group is collapsed in the mixer, and the bus has its own insert chain.
const fs = require('node:fs'), path = require('node:path'), assert = require('node:assert/strict');
const { harness } = require('./helpers/studio-session');
const tests = [], test = (name, run) => tests.push([name, run]);
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);

function grouped() {
  const h = harness(), S = h.S, app = h.window.STApp;
  while (S.state.project.tracks.length < 4) S.appendInstrumentTrack(S.state.project, 0);
  S.state.track = 1; S.state.selection = { track: 2, row: 0 };
  app.groupTracks();
  const group = h.window.STGroups.list(S.state.project)[0];
  return { h, S, app, group };
}

test('the group bus strip precedes its member strips; collapsing hides the members', () => {
  const { h, S, app, group } = grouped();
  const list = h.window.STStudio.channels();
  same(list.map(c => c.groupHead ? 'G' : c.type === 'track' ? `T${c.track}` : c.type).slice(0, 5), ['T0', 'G', 'T1', 'T2', 'T3']);
  assert.equal(list[1].owner, `bus:${group.busID}`);
  S.state.view = 'mixer';
  let html = h.window.STStudio.renderMixer();
  assert.match(html, /group-strip/);
  assert.match(html, /data-group-mixer-expand="[^"]+"[^>]*aria-expanded="true">Hide 2 tracks/);
  assert.equal((html.match(/group-member-strip/g) || []).length, 2);
  app.setViewPreference(group.id, 'mixerCollapsed', true);
  S.state.track = 0; S.state.mixerOwner = null;
  html = h.window.STStudio.renderMixer();
  assert.equal((html.match(/group-member-strip/g) || []).length, 0, 'collapsed members are hidden');
  assert.match(html, />Show 2 tracks</);
  assert.equal(S.state.project.groups[0].collapsed, false, 'the pattern view keeps its own collapse state');
});

test('the group bus is an insert-chain owner and a mixer selection target', () => {
  const { h, S, group } = grouped();
  S.state.view = 'mixer';
  assert.equal(h.window.STStudio.selectOwner(`bus:${group.busID}`), true);
  S.state.project.audioPlugins.push({ owner: `bus:${group.busID}`, kind: 'effect', slot: 0, instanceID: 'busfx', component: 'clap:com.example.plugin', format: 'CLAP', name: 'Glue' });
  const html = h.window.STStudio.renderMixer();
  assert.match(html, /1 insert effect/);
});

(async () => {
  let failed = 0;
  for (const [name, run] of tests) {
    try { await run(); console.log('ok  ', name); } catch (error) { failed++; console.error('FAIL', name, '\n', error); }
  }
  if (failed) process.exit(1);
  console.log(`track groups mixer: ${tests.length} passed`);
})();
