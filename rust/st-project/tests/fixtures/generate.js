'use strict';
// Regenerates the project fixtures of the st-project tests from the web interface's own model
// (Resources/Web). Run from the repository root:  node rust/st-project/tests/fixtures/generate.js
// The JSON files are committed, so `cargo test` needs no node.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const root = path.join(__dirname, '../../../..'), web = path.join(root, 'Resources/Web');

let serial = 0;
const uid = () => `fx${(++serial).toString(36).padStart(4, '0')}`;
const window = { STApp: { clamp: (v, lo, hi) => Math.max(lo, Math.min(hi, v)), uid,
  colors: ['#bba374', '#a0adb1', '#b0af89', '#c58b69', '#b79583', '#9da688', '#94a39a', '#aaa0ad'], state: {} } };
window.window = window;
const document = { readyState: 'complete', addEventListener() {} };
const context = vm.createContext({ window, globalThis: window, document, console, structuredClone, setTimeout() {}, crypto: { randomUUID: uid } });
for (const name of ['util.js', 'routing-project.js', 'track-groups.js', 'app-project-model.js', 'clip-takes-project.js', 'song-model.js', 'send-effects.js']) {
  vm.runInContext(fs.readFileSync(path.join(web, name), 'utf8'), context, { filename: name });
}
const app = window.STApp, G = window.STGroups, Takes = window.STClipTakesProject, Sends = window.STSends;
const clone = value => JSON.parse(JSON.stringify(value));
const write = (name, project) => fs.writeFileSync(path.join(__dirname, name), JSON.stringify(project, null, 1) + '\n');

// 1. The interface's blank project and its demo project, as the page publishes them.
write('blank.json', app.blankProject());
const demo = app.normalize(app.demoProject());
write('demo.json', demo);

// 2. What native kit.read returns for a kit file (tests/helpers/kit-group-fixture.js), normalized.
const { groupProject } = require(path.join(root, 'tests/helpers/kit-group-fixture.js'));
write('kit-group.json', app.normalize(groupProject({ pads: [0, 1, 5], plugins: [{ pad: 0 }, { pad: 5, slot: 2, component: 'builtin:delay' }],
  busInserts: 2, choke: { 0: 1, 1: 1 }, notes: ['Pad 3 had no sample.'] })));

// 3. The project of tests/track_groups_model_tests.js, grouped through the interface's own group model.
{
  const tracks = Array.from({ length: 6 }, (_, i) => ({ id: i, name: `T${i + 1}`, routingID: `r${i}`, instrument: i, volume: .8 }));
  const p = {
    name: 'Groups', bpm: 120, tracks,
    instruments: tracks.map((_, i) => ({ id: i, name: `I${i}`, type: 'sample' })),
    patterns: [{ id: 0, length: 4, steps: tracks.map((_, i) => [{ note: 60 + i, instrument: i }, null, null, null]),
      automation: tracks.map((_, i) => ({ id: `a${i}`, track: i, target: 'volume', points: [] })) }],
    audioClips: [{ id: 'c', track: 4, path: '/Audio/c.wav', startBeat: 0, durationBeats: 4 }],
    audioPlugins: [{ instanceID: 'p', owner: 'track:r5', track: 5, kind: 'effect', slot: 0, component: 'builtin:eq' }],
    routing: { version: 1, mixModel: 'standard', buses: [], outputs: Object.fromEntries(tracks.map(t => [`track:${t.routingID}`, 'master'])), sends: [], sidechains: [] }
  };
  G.create(p, [1, 3, 4], { makeID: uid, name: 'Drums' });
  write('groups.json', app.normalize(p));
}

// 4. A project that uses every part of the schema, built with the interface's creators where they
//    exist and with the shapes its modules write elsewhere (the file named in each comment).
{
  const p = clone(demo);
  p.name = 'Everything';
  p.routing = { version: 1, mixModel: 'standard', buses: [], outputs: {}, sends: [], sidechains: [] };
  for (const track of p.tracks) p.routing.outputs[`track:${track.routingID}`] = 'master';
  G.create(p, [3, 4, 5, 6], { makeID: uid, name: 'Kit', libraryPath: '/Library/Kits/Night.stkit', importNotes: ['Pad 9 was empty.'] });
  p.tracks[3].pad = 0; p.tracks[3].padNote = 36; p.tracks[5].chokeGroup = 1; p.tracks[6].chokeGroup = 1; p.tracks[4].voiceMode = 'mono';
  // routing.js: a return bus, a send, a sidechain.
  p.routing.buses.push({ id: 'ret1', name: 'Plate', type: 'return', volume: .9, pan: 0, mute: false, solo: false });
  p.routing.outputs['bus:ret1'] = 'master';
  p.routing.outputs[`track:${p.tracks[7].routingID}`] = null;
  p.routing.sends.push({ id: 'send1', source: `track:${p.tracks[0].routingID}`, tap: 'postFader', destination: 'bus:ret1', gain: .5, enabled: true });
  p.routing.sidechains.push({ id: 'sc1', source: `track:${p.tracks[3].routingID}`, tap: 'preFader',
    destination: { instanceID: 'comp-1', inputBus: 1, layoutSignature: { name: 'Sidechain', channels: 2 } }, gain: 1, enabled: true });
  // Plug-in descriptors as plugin.load returns them (AudioPluginRack.summary) plus built-in effects (insert-chain.js).
  p.audioPlugins = [
    { owner: `track:${p.tracks[0].routingID}`, track: 0, kind: 'effect', slot: 0, instanceID: 'comp-1', component: 'vst3:ABCDEF0123456789ABCDEF0123456789',
      name: 'Squash', manufacturer: 'Acme', format: 'VST3', bundlePath: '/Library/Audio/Plug-Ins/VST3/Squash.vst3', bypass: false, latency: 0.0013, tailTime: 0,
      state: 'AAECAwQ=', presets: [], parameters: [{ address: '18446744073709551615', name: 'Threshold', value: .5 }], hostStatus: { state: 'running' } },
    { owner: `track:${p.tracks[1].routingID}`, track: 1, kind: 'instrument', slot: 0, instanceID: 'keys-1', component: 'clap:com.acme.keys', name: 'Keys',
      manufacturer: 'Acme', format: 'CLAP', bypass: false, latency: 0, tailTime: 2, state: 'c3RhdGU=', nks: { name: 'Glass', path: '/Library/NKS/Glass.nksf' }, missing: true },
    { owner: 'bus:ret1', kind: 'effect', slot: 0, instanceID: 'plate-1', component: 'builtin:reverb', name: 'Reverb', format: 'Builtin', state: 'AAAA' },
    { owner: 'master', kind: 'effect', slot: 15, instanceID: 'limit-1', component: 'builtin:limiter', name: 'Limiter', format: 'Builtin', state: 'AAAA', stateError: 'busy' },
    { owner: 'instrument:2', kind: 'effect', slot: 1, instanceID: 'pad-chorus', component: 'builtin:chorus', name: 'Chorus', format: 'Builtin', state: 'AAAA' }
  ];
  p.instruments[1].type = 'au'; p.instruments[1].plugin = clone(p.audioPlugins[1]);
  // plugin-pages.js
  p.pluginPages = { 'comp-1': { name: 'Squash', pages: [{ name: 'Main', addresses: ['1', '18446744073709551615', null, null, null, null, null, null] }] } };
  // sampler.js: zones and slice markers; modulation.js; granular.js.
  Object.assign(p.instruments[3], { samplePath: '/Samples/Kick.wav', mode: 'slice', slicePoints: [0, .25, .5, .75], slices: [0, .25, .5, .75], sliceCount: 4,
    zones: [{ samplePath: '/Samples/Kick soft.wav', keyLow: 0, keyHigh: 127, velocityLow: 0, velocityHigh: 63, rootNote: 36, tune: 0, volume: 1 },
      { samplePath: '/Samples/Kick hard.wav', path: '/Samples/Kick hard.wav', keyLow: 0, keyHigh: 127, velocityLow: 64, velocityHigh: 127, rootNote: 36, tune: -.5, volume: .9 }] });
  Object.assign(p.instruments[2], { mode: 'granular', samplePath: '/Samples/Pad.flac', grainSize: .12, grainDensity: 4, grainSpray: .2, grainScan: -1, lfoSync: true, lfoBeats: .5,
    lfoTarget: 'position', lfoShape: 'random', lfo2Target: 'grainSize', voiceMode: 'legato', chokeGroup: 2 });
  // docs/MIDI_FILES.md: external MIDI instrument.
  Object.assign(p.instruments[7], { type: 'midi', midiOutput: 2, midiChannel: 5, midiProgram: 10, midiBankMSB: 1, midiBankLSB: 2, midiCCMap: { cutoff: 74 }, midiCCRanges: { cutoff: [20, 20000] } });
  // app-note-columns.js, step-fx.js: extra note columns and a plug-in parameter lock.
  p.tracks[1].noteColumns = 3;
  p.patterns[0].steps[1][0] = { note: 60, instrument: 1, velocity: 100, gate: 0, columns: [{ note: 64, velocity: 90 }, null], fx1: { type: 'microtiming', value: .25 }, fx2: null };
  p.patterns[0].steps[0][1] = { note: -3, fx1: { type: 'plugin', instanceID: 'comp-1', address: '18446744073709551615', name: 'Threshold', value: .25 } };
  p.patterns[0].steps[0][2] = { note: -1 }; p.patterns[0].steps[0][3] = { note: -2 };
  p.patterns[0].color = '#d6a35c';
  // automation-song.js / routing.js: owner lanes.
  p.patterns[0].automation.push(
    { id: uid(), owner: `track:${p.tracks[0].routingID}`, target: 'plugin:effect:0:1', name: 'Squash · Threshold', interpolation: 'step', points: [{ row: 0, value: .2 }, { row: 32.5, value: .9 }] },
    { id: uid(), owner: 'bus:ret1', target: 'volume', name: 'Plate · level', interpolation: 'linear', points: [{ row: 0, value: 1 }] },
    { id: uid(), owner: `track:${p.tracks[0].routingID}`, target: 'send:send1', name: 'Send', interpolation: 'linear', points: [{ row: 4, value: 2 }] },
    { id: uid(), owner: 'instrument:2', target: 'plugin:effect:1:7', name: 'Chorus · rate', interpolation: 'linear', points: [{ row: 0, value: .5 }] });
  // song-model.js: a loop section. send-effects.js: synced delay.
  p.songLoop = { start: 2, end: 3, restoreLoopSong: true };
  Sends.setDivision(p, '1/8.'); p.reverb.predelay = .02;
  // clip-arranger.js / recording.js clips, clip-takes-project.js takes.
  const clip = { id: 'clip-1', name: 'Vocal', track: 7, path: '/Audio/Vocal 1.wav', startBeat: 4, durationBeats: 8, sourceOffset: .5, sourceDuration: 30, gain: .8, fadeIn: .01, fadeOut: .2, muted: false };
  const withTakes = Takes.add(clip, { name: 'Take 2', path: '/Audio/Vocal 2.wav', sourceOffset: 0, sourceDuration: 31.5 }, 60 / p.bpm, () => 'take-2');
  p.audioClips = [withTakes, { id: 'clip-2', track: 0, samplePath: '/Audio/Loop.wav', path: '/Audio/Loop.wav', startBeat: 0, durationBeats: 16 }];
  // line-inputs.js
  p.tracks[7].audioInput = { enabled: true, deviceUID: 'BuiltInMicrophoneDevice', channels: [0, 1], gain: 1, monitor: false };
  // freeze.js stores what freeze.track returns (AudioFreezeTrack.Record.dictionary).
  p.tracks[2].frozen = { version: 1, id: 'frz-1', path: '/Prints/pad.wav', sha256: 'ab'.repeat(32), sampleRate: 48000, frames: 480000, musicalFrames: 400000, tailFrames: 80000,
    musicalEndBeat: 64, tailSeconds: 10, dormantPluginIDs: ['pad-chorus'], fingerprint: 'cd'.repeat(32), mixMode: 'routed', createdAt: '2026-10-01T10:00:00Z',
    patternPrints: [{ pattern: 0, path: '/Prints/pad-p0.wav', sha256: 'ef'.repeat(32), frames: 120000, passFrames: 100000, passes: 2, loop: { path: '/Prints/pad-p0-loop.wav', sha256: '01'.repeat(32), frames: 120000 } }],
    unprintedPatterns: [1], printGain: 1, tailMode: 'auto', method: 'offline' };
  // app-track-lanes.js, app-pattern-edit.js, app-automation-panel.js, app-song-scroll.js
  p.viewState = { gateColumn: true, automationPanel: 'song', songScroll: true, hideEmpty: true, hideEmptyScope: 'song', collapsedTracks: [p.tracks[0].routingID] };
  // MediaRelinker.fingerprint / apply
  p.mediaManifest = [{ path: '/Audio/Vocal 1.wav', size: 123456, modified: 1790000000.25, sha256: '9f'.repeat(32) }];
  p.mediaRelinkHistory = [{ date: '2026-10-02T08:00:00Z', changes: [{ id: '/audioClips/0/path', from: '/Old/Vocal 1.wav', to: '/Audio/Vocal 1.wav', differentContent: false }] }];
  p.metronome = true; p.timeSignature = { numerator: 7, denominator: 8 };
  // Fields no version of this crate knows: they must survive at every level.
  p.futureTopLevel = { nested: [1, 2, { three: null }], flag: true };
  p.tracks[0].futureTrackField = 'kept'; p.instruments[0].futureInstrumentField = [1.5, null];
  p.patterns[0].futurePatternField = { a: 1 }; p.patterns[0].steps[0][0].futureStepField = 9007199254740993;
  p.patterns[0].steps[0][0].fx1 = { type: 'futureEffect', value: 1, shape: 'x' };
  p.patterns[0].automation[0].futureLaneField = true; p.patterns[0].automation[0].points[0].curve = .3;
  p.routing.futureRoutingField = 'kept'; p.routing.buses[0].futureBusField = 1; p.routing.sends[0].futureSendField = 2;
  p.audioPlugins[0].futurePluginField = { deep: 'kept' }; p.audioClips[0].futureClipField = 1; p.audioClips[0].takeSet.takes[0].futureTakeField = 1;
  p.groups[0].futureGroupField = 'kept'; p.delay.futureDelayField = 1; p.viewState.futureViewField = 1; p.tracks[2].frozen.futureFreezeField = 1;
  p.instruments[3].zones[0].futureZoneField = 'kept';
  // Wrong types are kept too (and read as their defaults).
  p.tracks[4].color = 12; p.instruments[4].rootNote = '60'; p.instruments[5].mode = 'futureMode';
  write('everything.json', app.normalize(p));
}
console.log('fixtures written to', __dirname);
