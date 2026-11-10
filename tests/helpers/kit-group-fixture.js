'use strict';
// What native kit.read returns for a kit file: the kit as a group project:
// one member track per loaded pad with its instrument, inserts and mix, plus the group bus.
function groupProject({ name = 'Fixture Kit', pads = [0, 1, 2], plugins = [], busInserts = 0, choke = {}, notes = [] } = {}) {
  const kitID = 'srckit', busID = 'srcbus';
  const tracks = pads.map((pad, i) => ({
    id: i, name: `Pad ${pad + 1} ${name}`, routingID: `${kitID}-pad${pad}`, groupID: kitID, pad, padNote: 60,
    instrument: i, volume: .5 + i / 10, pan: 0, mute: false, solo: false, delaySend: 0, reverbSend: 0, plugins: [],
    ...(choke[pad] ? { chokeGroup: choke[pad] } : {})
  }));
  const instruments = pads.map((pad, i) => ({ id: i, name: `Pad ${pad + 1} ${name}`, type: 'sample', samplePath: `/Kits/${name}/pad${pad + 1}.wav`, rootNote: 60, volume: 1 }));
  const audioPlugins = [
    ...plugins.map(({ pad, kind = 'effect', slot = 0, component = 'builtin:compressor' }, n) => ({
      owner: `track:${kitID}-pad${pad}`, track: pads.indexOf(pad), kind, slot, component, instanceID: `src-plugin-${n}`, name: component, state: 'AAAA' })),
    ...Array.from({ length: busInserts }, (_, slot) => ({ owner: `bus:${busID}`, kind: 'effect', slot, component: 'builtin:reverb', instanceID: `src-bus-${slot}`, name: 'Reverb', state: 'AAAA' }))
  ];
  const outputs = { [`bus:${busID}`]: 'master', ...Object.fromEntries(tracks.map(t => [`track:${t.routingID}`, `bus:${busID}`])) };
  return {
    version: 1, name, tracks, instruments, audioPlugins,
    patterns: [{ id: 0, length: 1, steps: tracks.map(() => [null]), automation: [] }],
    groups: [{ id: kitID, name, color: '#e2a44f', collapsed: false, mixerCollapsed: false, busID, ...(notes.length ? { importNotes: notes } : {}) }],
    routing: { version: 1, mixModel: 'standard', buses: [{ id: busID, name, type: 'group', groupID: kitID, volume: .9, pan: 0, mute: false, solo: false }], outputs, sends: [], sidechains: [] }
  };
}

module.exports = { groupProject };
