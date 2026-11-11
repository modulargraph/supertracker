'use strict';
// window.STAgent (agent-api.js) against the real app.js renderer, history and engine
// scheduling: validation, one undo step per call, readable round-trips and UI sync.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const { owned } = require('./helpers/project');
const fixture = fs.readFileSync(path.join(__dirname, 'scalar_field_focus_tests.js'), 'utf8');
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')) + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));

function harness(view = 'pattern', responses = {}) {
  const h = makeHarness(view);
  for (const filename of ['song-model.js', 'agent-api.js']) web.load(h.context, filename);
  const toast = h.document.getElementById('toast-region');
  h.document.getElementById = (original => id => id === 'toast-region' ? toast : original(id))(h.document.getElementById);
  // Native requests answer from `responses` (a value or a function of the payload).
  h.window.STHostTransport.post = message => {
    h.calls.push(message);
    const answer = responses[message.command];
    try { const result = typeof answer === 'function' ? answer(message.payload) : answer ?? {}; h.window.STHostTransport.deliver({ id: message.id, ok: true, result }); }
    catch (error) { h.window.STHostTransport.deliver({ id: message.id, ok: false, error: error.message }); }
  };
  const agent = h.window.STAgent;
  async function call(method, params = {}) {
    const text = await agent.dispatch(method, JSON.stringify(params));
    assert.equal(typeof text, 'string', 'dispatch answers with a JSON string');
    return JSON.parse(text);
  }
  async function ok(method, params) { const reply = await call(method, params); assert(reply.ok, `${method} failed: ${reply.error}`); return reply.result; }
  async function rejects(method, params, pattern) {
    const before = copy(h.state.project), undo = h.state.undo.length, reply = await call(method, params);
    assert.equal(reply.ok, false, `${method} should fail`); assert.match(reply.error, pattern);
    assert.deepEqual(copy(h.state.project), before, 'A rejected call leaves the song untouched'); assert.equal(h.state.undo.length, undo, 'and adds no undo step');
    return reply.error;
  }
  return { ...h, agent, call, ok, rejects, t: agent._test };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('note names, step FX and unit values round-trip through the readable forms', () => {
  const { t } = harness();
  for (const [text, midi] of [['C-4', 60], ['C4', 60], ['c#4', 61], ['Db3', 49], ['G-9', 127], ['A-0', 21], ['F♯2', 42], [72, 72], ['OFF', -1], ['cut', -2]]) assert.equal(t.parseNote(text), midi, String(text));
  for (let n = 12; n < 128; n++) assert.equal(t.parseNote(t.noteName(n)), n);
  for (const bad of ['H-4', 'C-', 128, -5, 'C10', '']) assert.throws(() => t.parseNote(bad), /note/i);
  const samples = ['probability 50%', 'ratchet 3', 'pitch -12', 'pitch +7', 'cutoff 800Hz', 'pan -30%', 'reverse', 'reverse off', 'arpeggio 37', 'microtiming 25%', 'volume 80%', 'slice 4', 'glide -12', 'delay 40%', 'reverb 30%', 'offset 25%', 'resonance 50%'];
  for (const text of samples) assert.equal(t.formatFX(t.parseFX(text, 'fx1')), text, text);
  assert.deepEqual(copy(t.parseFX('prob 75', 'fx1')), { type: 'probability', value: .75 });
  assert.deepEqual(copy(t.parseFX('arp +4 +7', 'fx1')), { type: 'arpeggio', value: 0x47 });
  assert.deepEqual(copy(t.parseFX({ type: 'ratchet', value: 2 }, 'fx2')), { type: 'ratchet', value: 2 });
  assert.equal(t.parseFX(null, 'fx1'), null);
  assert.throws(() => t.parseFX('probability 150%', 'fx1'), /between 0% and 100%/);
  assert.throws(() => t.parseFX('wobble 3', 'fx1'), /unknown effect/);
  assert.throws(() => t.parseFX('ratchet 2.5', 'fx1'), /whole numbers/);
  const hz = { min: 20, max: 20000, kind: 'hz' }, db = { min: 0, max: 1.5, kind: 'db' }, time = { min: .001, max: 5, kind: 'time' }, pan = { min: -1, max: 1, kind: 'pan' };
  assert.equal(t.parseValue(hz, '800 Hz', 'cutoff'), 800); assert.equal(t.parseValue(hz, '1.2 kHz', 'cutoff'), 1200); assert.equal(t.parseValue(hz, 440, 'cutoff'), 440);
  assert.equal(Math.round(t.parseValue(db, '-6 dB', 'v') * 1000), 501); assert.equal(t.parseValue(db, '-inf dB', 'v'), 0);
  assert.equal(t.parseValue(time, '20 ms', 'a'), .02); assert.equal(t.parseValue(time, '1.5 s', 'a'), 1.5);
  assert.equal(t.parseValue(pan, '30% L', 'p'), -.3); assert.equal(t.parseValue(pan, 'R25', 'p'), .25); assert.equal(t.parseValue(pan, 'center', 'p'), 0);
  assert.equal(t.parseValue({ min: 0, max: 127, kind: 'note' }, 'C-3', 'root'), 48);
  assert.throws(() => t.parseValue(hz, '50 kHz', 'cutoff'), /cutoff must be between 20 Hz … 20 kHz/);
  assert.throws(() => t.parseValue(time, '3 Hz', 'attack'), /unit "hz" does not fit/);
  assert.equal(t.parseValue(hz, t.formatValue('hz', 1234), 'x'), 1234);
});

test('writeSteps is one labelled undo step, refreshes, syncs the engine, and reads back readably', async () => {
  const h = harness(), pattern = h.ST.pattern();
  const undo = h.state.undo.length, html = h.app.innerHTML;
  const result = await h.ok('writeSteps', { label: 'add bassline', steps: [
    { track: 0, row: 0, note: 'C-3', velocity: 110 }, { track: 0, row: 2, note: 'Eb3', gate: 1.5 },
    { track: 'Glass keys', row: 5, notes: ['C-4', 'E-4', 'G-4'], fx1: 'probability 50%' }, { track: 0, row: 6, note: 'OFF' }] });
  assert.equal(result.changed, true); assert.equal(result.undoLabel, 'AI: add bassline'); assert.equal(result.notes, 3);
  assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.undo.at(-1).label, 'AI: add bassline');
  assert.notEqual(h.app.innerHTML, html, 'The interface re-rendered');
  assert.deepEqual(copy(pattern.steps[0][0]), { note: 48, instrument: 0, velocity: 110, gate: .8 }, 'An existing step is merged and keeps its gate');
  assert.deepEqual(copy(pattern.steps[1][5]), { note: 60, instrument: 1, velocity: 100, gate: 0, chord: [0, 4, 7], fx1: { type: 'probability', value: .5 } });
  assert.equal(pattern.steps[0][6].note, -1);
  h.calls.length = 0; h.flush(100); await new Promise(resolve => setImmediate(resolve)); for (let i = 0; i < 20; i++) await Promise.resolve();
  assert(h.calls.some(call => call.command === 'engine.project'), 'The engine received the edited project');
  const read = await h.ok('readPattern', { tracks: [0, 1], from: 0, to: 7 });
  const keys = read.tracks[1].steps.find(step => step.row === 5);
  assert.deepEqual(keys, { row: 5, note: 'C-4', midi: 60, instrument: 1, velocity: 100, chord: [0, 4, 7], notes: ['C-4', 'E-4', 'G-4'], fx1: 'probability 50%' });
  assert.deepEqual(read.tracks[0].steps.find(step => step.row === 2), { row: 2, note: 'D#3', midi: 51, instrument: 0, velocity: 100, gate: 1.5 });
  // Reading back and writing the readable values reproduces the same data (no change, no undo step).
  const again = await h.ok('writeSteps', { steps: [{ track: 1, row: 5, notes: keys.notes, fx1: keys.fx1, velocity: keys.velocity, gate: keys.gate }] });
  assert.equal(again.changed, false); assert.equal(h.state.undo.length, undo + 1);
  const grid = await h.ok('readPattern', { tracks: [0], from: 0, to: 2, format: 'grid' });
  assert.equal(grid.lines.length, 3); assert.match(grid.lines[0], /^000 \| C-3 00 110/);
  const undone = await h.ok('undo');
  assert.deepEqual(copy(undone.undid), ['AI: add bassline']); assert.equal(h.state.undo.length, undo); assert.equal(h.ST.pattern().steps[1][5], null);
  const redone = await h.ok('redo'); assert.deepEqual(copy(redone.redid), ['AI: add bassline']); assert.deepEqual(copy(h.ST.pattern().steps[0][0]), { note: 48, instrument: 0, velocity: 110, gate: .8 });
});

test('gate is optional: new notes have none, 0/null removes one, a step saved without a gate key reads as the legacy 0.8', async () => {
  const h = harness(), pattern = h.ST.pattern();
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4', gate: 2 }, { track: 0, row: 1, note: 'D-4' }] });
  assert.equal(pattern.steps[0][0].gate, 2); assert.equal(pattern.steps[0][1].gate, 0);
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, gate: null }] }); assert.equal(pattern.steps[0][0].gate, 0);
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, gate: 1.25 }] }); assert.equal(pattern.steps[0][0].gate, 1.25);
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, gate: 0 }] }); assert.equal(pattern.steps[0][0].gate, 0);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, gate: .001 }] }, /gate must be 0\.01–256 rows, or 0\/null for no gate/);
  pattern.steps[0][2] = { note: 64, instrument: 0, velocity: 100 };
  const read = await h.ok('readPattern', { tracks: [0], from: 0, to: 2 });
  assert.deepEqual(read.tracks[0].steps.map(step => step.gate), [undefined, undefined, .8]);
  await h.ok('transform', { operation: 'gate', set: 3, tracks: [0], from: 0, to: 2 }); assert.deepEqual([0, 1, 2].map(r => pattern.steps[0][r].gate), [3, 3, 3]);
  await h.ok('transform', { operation: 'gate', set: 0, tracks: [0], from: 0, to: 2 }); assert.deepEqual([0, 1, 2].map(r => pattern.steps[0][r].gate), [0, 0, 0]);
});

test('validation errors are clear and leave the song and history untouched', async () => {
  const h = harness();
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, note: 'H-4' }] }, /Could not read steps\[0\]\.note "H-4"/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 99, note: 'C-4' }] }, /steps\[0\]\.row must be 0–7/);
  await h.rejects('writeSteps', { steps: [{ track: 'Nope', row: 0, note: 'C-4' }] }, /No single track matches "Nope"/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 1, note: 'C-4' }, { track: 0, row: 2, note: 'C-4', fx1: 'probability 300' }] }, /probability must be between/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4', colour: 'red' }] }, /unknown field "colour"/);
  await h.rejects('writeSteps', { steps: [{ track: 7, row: 1, velocity: 90 }] }, /empty: give a note/);
  await h.rejects('writeSteps', { steps: [] }, /non-empty array/);
  await h.rejects('readPattern', { pattern: 42 }, /no pattern with id 42/);
  await h.rejects('setSong', { bpm: 900 }, /bpm must be between 20 BPM … 400 BPM/);
  await h.rejects('setSong', { tempo: 120 }, /Unknown parameter "tempo" for setSong/);
  await h.rejects('editInstrument', { instrument: 0, params: { cutof: 100 } }, /Unknown instrument parameter "cutof"/);
  await h.rejects('editInstrument', { instrument: 1, params: { mode: 'wobble' } }, /mode must be one of oneshot/);
  await h.rejects('editInstrument', { instrument: 1, params: { waveform: 'saw' } }, /Unknown instrument parameter "waveform"/);
  await h.rejects('createInstrument', { type: 'synth' }, /type must be "sample" or "midi"/);
  await h.rejects('editInstrument', { instrument: 1, type: 'synth' }, /type must be "sample" or "midi"/);
  await h.rejects('nonsense', {}, /Unknown method "nonsense"/);
  await h.rejects('runCommand', { id: 'explode' }, /Unknown command "explode"/);
  const reply = JSON.parse(await h.agent.dispatch('summary', '{not json'));
  assert.equal(reply.ok, false); assert.match(reply.error, /not valid JSON/);
  h.state.project.tracks[2].frozen = true;
  await h.rejects('writeSteps', { steps: [{ track: 2, row: 0, note: 'C-4' }] }, /Track 2 "Warm pad" is frozen/);
});

test('patterns, song blocks, tracks, tempo and instruments each edit in one undo step', async () => {
  const h = harness();
  let undo = h.state.undo.length;
  const step = async (method, params) => { const result = await h.ok(method, params); assert.equal(h.state.undo.length, ++undo, `${method} adds exactly one undo step`); assert.match(h.state.undo.at(-1).label, /^AI: /); return result; };
  const created = await step('createPattern', { name: 'Drop', length: 32, appendToSong: true });
  assert.equal(h.state.project.activePattern, created.pattern.id); assert.equal(h.ST.pattern().length, 32); assert.equal(h.state.project.sequence.at(-1), created.pattern.id);
  const copyResult = await step('duplicatePattern', { pattern: 'Drop', name: 'Drop B', select: false });
  await step('setPatternLength', { pattern: copyResult.pattern.id, length: 16 });
  assert.equal(h.state.project.patterns.find(p => p.id === copyResult.pattern.id).steps[0].length, 16);
  const song = await step('editSong', { action: 'set', sequence: [{ pattern: 0, repeats: 2 }, 'Drop', { pattern: 'Drop B', repeats: 3 }] });
  assert.deepEqual(song.sequence, [0, 0, created.pattern.id, copyResult.pattern.id, copyResult.pattern.id, copyResult.pattern.id]);
  await step('editSong', { action: 'repeats', block: 0, repeats: 4 });
  assert.deepEqual(copy(h.state.project.sequence.slice(0, 4)), [0, 0, 0, 0]);
  await step('deletePattern', { pattern: 'Drop B' });
  assert(!h.state.project.sequence.includes(copyResult.pattern.id));
  const track = await step('addTrack', { name: 'Perc', instrument: 3 });
  assert.equal(h.state.project.tracks[track.track].name, 'Perc'); assert(h.state.project.patterns.every(p => p.steps.length === h.state.project.tracks.length));
  const edited = await step('editTrack', { track: 'Perc', volume: '-6 dB', pan: '20% R', reverbSend: '25%', mute: true });
  assert.equal(edited.volume, '-6 dB'); assert.equal(h.state.project.tracks[track.track].pan, .2); assert.equal(h.state.project.tracks[track.track].reverbSend, .25);
  const settings = await step('setSong', { bpm: '128 BPM', swing: '10%', timeSignature: '3/4' });
  assert.equal(settings.bpm, 128); assert.equal(h.state.project.swing, .1); assert.deepEqual(copy(h.state.project.timeSignature), { numerator: 3, denominator: 4 });
  const inst = await step('createInstrument', { name: 'Acid', track: 'Perc', params: { mode: 'wavetable', wavetablePosition: '25%', wavetableFrameSize: 2048, cutoff: '600 Hz', resonance: '70%', attack: '2 ms', volume: '-3 dB' } });
  const acid = h.state.project.instruments[inst.instrument];
  assert.equal(acid.type, 'sample'); assert.equal(acid.mode, 'wavetable'); assert.equal(acid.samplePath, 'builtin:wavetables/Basic/Saw.wav', 'a wavetable sampler without a sample plays the Saw table');
  assert.equal(acid.wavetablePosition, .25); assert.equal(acid.wavetableFrameSize, 2048); assert.equal(acid.waveform, undefined); assert.equal(acid.cutoff, 600); assert.equal(acid.resonance, .7); assert.equal(acid.attack, .002); assert.equal(h.state.project.tracks[track.track].instrument, inst.instrument);
  const tuned = await step('editInstrument', { instrument: 'Acid', params: { tuneSemi: 7, tuneFine: -10, 'Filter cutoff': '1.5 kHz' } });
  assert.equal(acid.tune === undefined ? h.state.project.instruments[inst.instrument].tune : h.state.project.instruments[inst.instrument].tune, 6.9); assert.equal(tuned.instrument.params.cutoff.display, '1.5 kHz');
  const read = await h.ok('getInstrument', { instrument: inst.instrument });
  assert.equal(read.params.cutoff.value, 1500); assert.equal(read.params.waveform, undefined); assert.equal(read.params.wavetablePosition.display, '25%'); assert.equal(h.state.undo.length, undo, 'Reading adds no undo step');
  const summary = await h.ok('summary');
  assert.equal(summary.bpm, 128); assert.equal(summary.timeSignature, '3/4'); assert.equal(summary.tracks.at(-1).name, 'Perc'); assert.equal(summary.patterns.find(p => p.name === 'Drop').length, 32);
  assert.deepEqual(summary.song.blocks.map(b => [b.pattern, b.repeats]), [[0, 4], [created.pattern.id, 1]]);
  const back = await h.ok('undo', { steps: 3 }); assert.equal(back.undid.length, 3); assert.equal(h.state.project.bpm, 116);
});

test('transforms, row edits and automation edit ranges as one step', async () => {
  const h = harness();
  await h.ok('writeSteps', { steps: [0, 1, 2, 3].map(row => ({ track: 0, row, note: 61 + row, velocity: 100 })) });
  let undo = h.state.undo.length;
  await h.ok('transform', { tracks: [0], from: 0, to: 3, operation: 'transpose', amount: 12 });
  assert.deepEqual(h.ST.pattern().steps[0].slice(0, 4).map(s => s.note), [73, 74, 75, 76]); assert.equal(h.state.undo.length, ++undo);
  await h.ok('transform', { tracks: [0], from: 0, to: 3, operation: 'snapToScale', scale: 'major', root: 'C' });
  assert.deepEqual(h.ST.pattern().steps[0].slice(0, 4).map(s => s.note), [74, 74, 76, 76]); assert.equal(h.state.undo.length, ++undo);
  await h.ok('transform', { tracks: [0], operation: 'velocity', set: 64 });
  assert(h.ST.pattern().steps[0].filter(Boolean).filter(s => s.note >= 0).every(s => s.velocity === 64));
  await h.rejects('transform', { operation: 'transpose' }, /whole, non-zero amount/);
  undo = h.state.undo.length;
  await h.ok('insertRows', { row: 1, count: 2, tracks: [0] });
  assert.equal(h.ST.pattern().steps[0][1], null); assert.equal(h.ST.pattern().steps[0][3].note, 74); assert.equal(h.ST.pattern().steps[0].length, 8); assert.equal(h.state.undo.length, ++undo);
  await h.ok('deleteRows', { row: 1, count: 2, tracks: [0] });
  assert.equal(h.ST.pattern().steps[0][1].note, 74); assert.equal(h.state.undo.length, ++undo);
  const lane = await h.ok('setAutomation', { track: 0, target: 'cutoff', points: [{ row: 0, value: '400 Hz' }, { row: 7, value: 6000 }] });
  assert.equal(lane.lane.points, 2); assert.equal(h.state.undo.length, ++undo);
  const stored = h.ST.pattern().automation.find(l => l.target === 'cutoff' && l.track === 0);
  assert.deepEqual(copy(stored.points), [{ row: 0, value: 400 }, { row: 7, value: 6000 }]);
  await h.rejects('setAutomation', { track: 0, target: 'cutoff', points: [{ row: 0, value: 50000 }] }, /between 20 and 20000/);
  await h.ok('deleteAutomation', { track: 0, target: 'cutoff' }); assert(!h.ST.pattern().automation.some(l => l.target === 'cutoff' && l.track === 0));
  await h.ok('clearSteps', { tracks: [0] }); assert(h.ST.pattern().steps[0].every(s => s === null));
});

test('plug-in parameters resolve by name, change natively and commit captured state in one step', async () => {
  const parameters = [{ address: '7', id: 'cutoff', name: 'Cutoff', min: 0, max: 100, value: 20, normalized: .2, display: '20', writable: true }, { address: '8', id: 'res', name: 'Resonance', min: 0, max: 1, value: 0, normalized: 0, display: '0', writable: true }];
  const set = [];
  const h = harness('pattern', {
    'engine.snapshot': () => ({ audioPlugins: copy(h.state.project.audioPlugins) }),
    'plugin.parameters': () => parameters,
    'plugin.parameter': payload => { set.push(payload); const item = parameters.find(p => p.address === payload.address); Object.assign(item, { normalized: payload.value, value: item.min + payload.value * (item.max - item.min), display: String(payload.value) }); return { ...item }; },
    'plugin.state': payload => ({ ...payload, component: 'aufx:test', name: 'Test FX', state: 'NEWSTATE' })
  });
  h.state.project.audioPlugins = [{ track: 1, kind: 'effect', slot: 0, instanceID: 'fx-1', component: 'aufx:test', name: 'Test FX', state: 'OLD' }]; owned(h.state.project);
  const list = await h.ok('pluginParameters', { track: 1, kind: 'effect', slot: 0, query: 'cut' });
  assert.deepEqual(list.parameters.map(p => p.name), ['Cutoff']);
  const undo = h.state.undo.length;
  const result = await h.ok('setPluginParameters', { track: 1, kind: 'effect', slot: 0, values: { Cutoff: 50, '8': '25%' } });
  assert.deepEqual(set.map(p => [p.address, p.value, p.normalized, p.instanceID]).sort(), [['7', .5, true, 'fx-1'], ['8', .25, true, 'fx-1']]);
  assert.equal(result.changed, true); assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.project.audioPlugins[0].state, 'NEWSTATE');
  assert.deepEqual(result.parameters.map(p => [p.name, p.normalized, p.applied]).sort(), [['Cutoff', .5, true], ['Resonance', .25, true]], 'the reply reads the values back');
  await h.rejects('setPluginParameters', { track: 1, kind: 'effect', values: { Cutoff: 500 } }, /Cutoff must be between 0 and 100/);
  await h.rejects('setPluginParameters', { track: 1, kind: 'effect', values: { Drive: 1 } }, /no parameter "Drive"/);
  await h.rejects('pluginParameters', { track: 2, kind: 'instrument' }, /has no plug-in instrument/);
});

test('UI state, navigation, commands and the status indicator', async () => {
  const h = harness();
  const ui = await h.ok('show', { view: 'pattern', track: 2, row: 5, column: 'velocity' });
  assert.equal(ui.cursor.track, 2); assert.equal(ui.cursor.row, 5); assert.equal(ui.cursor.column, 'velocity'); assert.equal(h.state.track, 2);
  const selected = await h.ok('show', { select: { fromTrack: 0, toTrack: 1, fromRow: 0, toRow: 3 } });
  assert.deepEqual(selected.selection, { tracks: [0, 1], rows: [0, 3] });
  const commands = await h.ok('listCommands', { query: 'duplicate track' });
  assert(commands.commands.some(c => c.id === 'duplicateTrack'));
  const help = await h.ok('help'); assert(help.methods.some(m => m.name === 'writeSteps' && m.params.includes('steps')));
  assert(!h.app.innerHTML.includes('id="agent-status" class="agent-status" title') || h.app.innerHTML.includes('hidden'), 'No indicator without a client');
  h.agent.setConnection({ enabled: true, clients: 1 }); h.ST.refresh();
  assert.match(h.app.innerHTML, /<span id="agent-status" class="agent-status"[^>]*title="[^"]*"\s*><i class="agent-led"><\/i>AI connected<\/span>/);
  h.agent.setConnection({ enabled: true, clients: 0 }); h.ST.refresh();
  assert.match(h.app.innerHTML, /<span id="agent-status"[^>]* hidden>/);
  assert.match(h.agent.settingsField(), /id="agent-enabled" checked/);
  h.agent.setConnection({ enabled: false, clients: 0 });
  assert.match(h.agent.settingsField(), /Allow AI control \(MCP\)/); assert.doesNotMatch(h.agent.settingsField(), /checked/);
});

test('appending to the song keeps the 1024-slot limit the host validates', async () => {
  const h = harness();
  h.state.project.sequence = Array(1024).fill(0);
  await h.rejects('editSong', { action: 'insert', pattern: 0, repeats: 2 }, /1024 sequence slots/);
  h.state.project.sequence = [0];
  await h.ok('editSong', { action: 'insert', pattern: 0, repeats: 2 });
  assert.deepEqual(copy(h.state.project.sequence), [0, 0, 0]);
  // createPattern/duplicatePattern with appendToSong append one slot too.
  h.state.project.sequence = Array(1024).fill(0);
  await h.rejects('createPattern', { name: 'Overflow', appendToSong: true }, /1024 sequence slots/);
  await h.rejects('duplicatePattern', { pattern: 0, appendToSong: true }, /1024 sequence slots/);
  const created = await h.ok('createPattern', { name: 'Unlisted' });
  assert.equal(h.state.project.sequence.length, 1024); assert(h.state.project.patterns.some(x => x.id === created.pattern.id));
});

test('a second apply that throws leaves no undo step; history validates limit', async () => {
  const h = harness(), before = copy(h.state.project), undo = h.state.undo.length;
  let runs = 0;
  assert.throws(() => h.t.commit('flaky', p => { if (runs++) throw new Error('second apply failed'); p.bpm = 99; }), /second apply failed/);
  assert.equal(runs, 2); assert.deepEqual(copy(h.state.project), before); assert.equal(h.state.undo.length, undo, 'no no-op "AI: flaky" step');
  await h.ok('writeSteps', { steps: [{ track: 0, row: 1, note: 60 }] }); await h.ok('writeSteps', { steps: [{ track: 0, row: 2, note: 62 }] });
  assert.equal((await h.ok('history', { limit: 1 })).undo.length, 1);
  for (const limit of [0, -1, 'all', 1.5, 1000]) await h.rejects('history', { limit }, /limit must be/);
});

test('calls run one at a time in arrival order', async () => {
  const h = harness();
  const replies = await Promise.all([1, 2, 3].map(n => h.call('writeSteps', { label: 'n' + n, steps: [{ track: 0, row: n, note: 60 + n }] })));
  assert(replies.every(reply => reply.ok));
  assert.deepEqual(copy(h.state.undo.slice(-3).map(x => x.label)), ['AI: n1', 'AI: n2', 'AI: n3']);
});

test('addTrack has no fixed track limit; only the 1024-track sanity bound rejects', async () => {
  const h = harness();
  assert.doesNotMatch(h.agent.describe ? JSON.stringify(h.agent.describe()) : '', /up to 16/);
  while (h.state.project.tracks.length < 40) await h.ok('addTrack', { name: 'Lane ' + h.state.project.tracks.length });
  assert.equal(h.state.project.tracks.length, 40); assert.equal(h.state.project.tracks[39].name, 'Lane 39');
  assert(h.state.project.patterns.every(pattern => pattern.steps.length === 40), 'every pattern gains a step column');
  h.ST.mutate('fill', p => { while (p.tracks.length < 1024) p.tracks.push({ ...copy(p.tracks[0]), id: p.tracks.length, routingID: 'fill-' + p.tracks.length }); for (const pattern of p.patterns) while (pattern.steps.length < p.tracks.length) pattern.steps.push(Array(pattern.length).fill(null)); });
  await h.rejects('addTrack', { name: 'One too many' }, /already has 1024 tracks/);
});

test('transform reports changed like every edit, takes each track once and keeps plug-in instruments on their own track', async () => {
  const h = harness(), steps = () => copy(h.ST.pattern().steps[0].slice(0, 3).map(step => step?.note ?? null));
  await h.ok('clearSteps', { tracks: [0] });
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, note: 60 }, { track: 0, row: 1, note: 62 }] });
  const first = await h.ok('transform', { tracks: [0], operation: 'velocity', set: 64 });
  assert.equal(first.changed, true); assert.equal(first.affected, 2);
  const undo = h.state.undo.length, again = await h.ok('transform', { tracks: [0], operation: 'velocity', set: 64 });
  assert.equal(again.changed, false, 'a transform that changes nothing says so'); assert.equal(again.affected, 2); assert.equal(h.state.undo.length, undo);
  // A track named twice is transposed and shifted once.
  await h.ok('transform', { tracks: [0, 'Velvet bass', '0'], operation: 'transpose', amount: 1 }); assert.deepEqual(steps(), [61, 63, null]);
  await h.ok('insertRows', { row: 0, count: 1, tracks: [0, 0] }); assert.deepEqual(steps(), [null, 61, 63]);
  assert.equal((await h.ok('readPattern', { tracks: [0, 0] })).tracks.length, 1);
  // writeSteps refuses a plug-in instrument of another track; transform {instrument} must as well.
  h.state.project.instruments[2].type = 'plugin';
  await h.rejects('transform', { tracks: [0], operation: 'instrument', set: 2 }, /plug-in that track 0 "Velvet bass" does not play/);
  await h.rejects('transform', { operation: 'instrument', set: 2 }, /only play on their own track/);
  await h.ok('writeSteps', { steps: [{ track: 2, row: 0, note: 60 }] });
  assert.equal((await h.ok('transform', { tracks: [2], operation: 'instrument', set: 2 })).operation, 'instrument', 'its own track may use it');
});

test('writeSteps counts only notes and a cleared column does not grow the track', async () => {
  const h = harness();
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4', fx1: 'probability 50%' }] });
  const fxOnly = await h.ok('writeSteps', { steps: [{ track: 0, row: 0, note: null }, { track: 0, row: 1, note: 'OFF' }, { track: 0, row: 2, note: 'D-4' }] });
  assert.equal(fxOnly.notes, 1, 'note:null and OFF are not notes');
  const undo = h.state.undo.length;
  const cleared = await h.ok('writeSteps', { steps: [{ track: 1, row: 3, column: 5, clear: true }, { track: 1, row: 4, column: 6, note: null }] });
  assert.equal(cleared.changed, false); assert.equal(h.state.project.tracks[1].noteColumns, undefined, 'the track keeps its one note column'); assert.equal(h.state.undo.length, undo);
  await h.ok('writeSteps', { steps: [{ track: 1, row: 3, column: 2, note: 'E-4' }] });
  assert.equal(h.state.project.tracks[1].noteColumns, 3, 'a written column is added');
});

test('setAutomation keeps one point per row, needs real values and refuses unknown targets', async () => {
  const h = harness();
  const lane = await h.ok('setAutomation', { track: 0, target: 'volume', points: [{ row: 4, value: .2 }, { row: 0, value: .5 }, { row: 0, value: '0 dB' }] });
  assert.equal(lane.lane.points, 2);
  assert.deepEqual(copy(h.ST.pattern().automation.find(l => l.target === 'volume' && l.track === 0).points), [{ row: 0, value: 1 }, { row: 4, value: .2 }], 'the last point of a row wins');
  for (const value of [null, true, '', [], {}]) await h.rejects('setAutomation', { track: 0, target: 'pan', points: [{ row: 0, value }] }, /points\[0\]\.value/);
  for (const target of ['constructor', 'toString', '__proto__', 'filterType', 'lfoSync', 'nope']) await h.rejects('setAutomation', { track: 0, target, points: [{ row: 0, value: 1 }] }, /Unknown automation target/);
  // A plug-in target names a plug-in the track holds.
  await h.rejects('setAutomation', { track: 0, target: 'plugin:garbage', points: [{ row: 0, value: .5 }] }, /has no plug-in for "plugin:garbage"/);
  await h.rejects('setAutomation', { track: 0, target: 'plugin:effect:3:7', points: [{ row: 0, value: .5 }] }, /has no plug-in/);
  h.state.project.audioPlugins = [{ track: 0, kind: 'effect', slot: 3, instanceID: 'fx-3', component: 'aufx:test', name: 'Test FX' }]; owned(h.state.project);
  assert.equal((await h.ok('setAutomation', { track: 0, target: 'plugin:effect:3:7', points: [{ row: 0, value: '50%' }] })).lane.points, 1);
});

test('show validates the whole request before it moves the pattern, cursor or selection', async () => {
  const h = harness();
  const two = await h.ok('createPattern', { name: 'Two', length: 4, select: false });
  const view = () => copy([h.state.project.activePattern, h.state.track, h.state.row, h.state.column, h.state.instrument, h.state.selection]), before = view();
  await h.rejects('show', { pattern: 'Two', track: 3, row: 7 }, /row must be 0–3/); assert.deepEqual(view(), before, 'the pattern and track did not move');
  await h.rejects('show', { track: 5, column: 'zzz' }, /column must be one of/); assert.deepEqual(view(), before);
  await h.rejects('show', { track: 2, instrument: 99 }, /instrument 99 does not exist/); assert.deepEqual(view(), before);
  await h.rejects('show', { row: 3, select: { fromRow: 5, toRow: 2 } }, /must not be before/); assert.deepEqual(view(), before);
  for (const select of [5, 'all', [0, 1]]) { await h.rejects('show', { select }, /select must be \{fromTrack, toTrack, fromRow, toRow\} or null/); assert.deepEqual(view(), before); }
  const ui = await h.ok('show', { pattern: 'Two', track: 3, row: 2, select: { toTrack: 4, fromRow: 1, toRow: 3 } });
  assert.equal(h.state.project.activePattern, two.pattern.id); assert.deepEqual(ui.selection, { tracks: [3, 4], rows: [1, 3] });
});

test('editInstrument validates its parameters before it retires a plug-in instrument', async () => {
  const setup = () => {
    const h = harness(), ran = [], perform = h.ST.perform;
    h.state.project.instruments[2].type = 'plugin'; h.state.project.instruments[2].plugin = { name: 'Synth', component: 'aumu:x' };
    // The app's own command removes the hosted plug-in (it needs the native host; here it simply succeeds).
    h.ST.perform = command => { if (!/^instrumentType:/.test(command)) return perform(command); ran.push(command);
      return h.ST.mutate('instrument type', p => { p.instruments[2].type = command.split(':')[1]; p.instruments[2].plugin = null; }); };
    return { h, ran };
  };
  for (const [params, pattern] of [[{ cutof: 100 }, /Unknown instrument parameter "cutof"/], [{ cutoff: '100 kHz' }, /cutoff must be between/]]) {
    const { h, ran } = setup();
    await h.rejects('editInstrument', { instrument: 2, type: 'sample', params }, pattern);
    assert.deepEqual(ran, [], 'a refused call does not run the type switch'); assert.equal(h.state.project.instruments[2].type, 'plugin');
  }
  const { h, ran } = setup(), done = await h.ok('editInstrument', { instrument: 2, type: 'sample', params: { cutoff: '1 kHz' } });
  assert.deepEqual(ran, ['instrumentType:sample']); assert.equal(done.instrument.type, 'sample'); assert.equal(h.state.project.instruments[2].cutoff, 1000);
});

test('built-in sample references load, prototype names are not parameters, and song-model refusals are plain answers', async () => {
  const loads = [], h = harness('pattern', { 'sample.load': payload => { loads.push(payload); return { path: payload.path, name: 'Vowels' }; } });
  // createInstrument (and the create_instrument tool) send agents to loadSample with a builtin: table.
  const loaded = await h.ok('loadSample', { path: 'builtin:wavetables/Morph/Vowels.wav', track: 1 });
  assert.equal(loaded.sample, 'builtin:wavetables/Morph/Vowels.wav'); assert.equal(loads[0].path, 'builtin:wavetables/Morph/Vowels.wav');
  await h.rejects('loadSample', { path: 'relative/kick.wav' }, /absolute file path, or a built-in sample/);
  await h.rejects('loadSample', { path: 'builtin:' }, /absolute file path/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4', fx1: { type: 'toString', value: 1 } }] }, /unknown effect type "toString"/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4', fx1: 'constructor 3' }] }, /unknown effect "constructor 3"/);
  await h.rejects('editInstrument', { instrument: 1, params: { constructor: 1 } }, /Unknown instrument parameter "constructor"/);
  h.state.project.sequence = Array(1024).fill(0);
  assert.doesNotMatch(await h.rejects('editSong', { action: 'insert', pattern: 0 }, /1024 sequence slots/), /Internal error/);
  for (const [limit, offset] of [['10', 0], [0, 0], [10, -5], [10, '5']]) await h.rejects('pluginParameters', { track: 0, limit, offset }, /limit must be|offset must be/);
  // Only the caller's own text is "not valid JSON"; a JSON failure inside a method is reported as internal.
  assert.match(JSON.parse(await h.agent.dispatch('summary', '{"a":')).error, /^params are not valid JSON/);
  delete h.state.project.reverb;
  assert.match(JSON.parse(await h.agent.dispatch('summary', '{}')).error, /^Internal error in summary/);
});

test('the host reaches the agent API through agent.call events and gets agent.reply; agent.status sets the connection', async () => {
  const h = harness();
  const replies = () => h.calls.filter(message => message.command === 'agent.reply').map(message => message.payload);
  const settle = async () => { for (let i = 0; i < 200 && replies().length < expected; i++) await new Promise(resolve => setImmediate(resolve)); };
  let expected = 1;
  // A read: the reply is what dispatch resolves to (JSON text), under the call's id.
  h.window.nativeEvent({ type: 'agent.call', payload: { id: 7, method: 'summary', params: { _host: { projectPath: null } } } });
  await settle();
  assert.equal(replies().length, 1, 'one reply per call');
  assert.equal(replies()[0].id, 7);
  assert.equal(typeof replies()[0].reply, 'string');
  const summary = JSON.parse(replies()[0].reply);
  assert.equal(summary.ok, true, summary.error); assert.equal(summary.result.name, h.state.project.name);
  // An edit: it lands in the project as one undo step, like a direct dispatch.
  const undo = h.state.undo.length; expected = 2;
  h.window.nativeEvent({ type: 'agent.call', payload: { id: 8, method: 'writeSteps', params: { steps: [{ track: 0, row: 3, note: 'D-4' }] } } });
  await settle();
  const edit = JSON.parse(replies()[1].reply);
  assert.equal(replies()[1].id, 8); assert.equal(edit.ok, true, edit.error);
  assert.equal(h.state.project.patterns[0].steps[0][3].note, 62); assert.equal(h.state.undo.length, undo + 1);
  // A failure is an answer too, and calls run in the order they arrived.
  expected = 4;
  h.window.nativeEvent(JSON.stringify({ type: 'agent.call', payload: { id: 9, method: 'noSuchMethod', params: {} } }));
  h.window.nativeEvent({ type: 'agent.call', payload: { id: 10, method: 'readPattern' } });
  await settle();
  assert.deepEqual(replies().slice(2).map(reply => reply.id), [9, 10]);
  assert.match(JSON.parse(replies()[2].reply).error, /Unknown method "noSuchMethod"/);
  assert.equal(JSON.parse(replies()[3].reply).ok, true, 'missing params are an empty object');
  // The connection status arrives as an event (and is not handed on to the other modules).
  h.window.nativeEvent({ type: 'agent.status', payload: { available: true, enabled: true, listening: true, clients: 1, socket: '/tmp/control.sock', suspendedForTermination: false } });
  h.ST.refresh();
  assert.match(h.app.innerHTML, /<span id="agent-status" class="agent-status"[^>]*title="[^"]*"\s*><i class="agent-led"><\/i>AI connected<\/span>/);
  h.window.nativeEvent({ type: 'agent.status', payload: { available: true, enabled: false, listening: false, clients: 0 } });
  h.ST.refresh();
  assert.match(h.app.innerHTML, /<span id="agent-status"[^>]* hidden>/);
  assert.doesNotMatch(h.agent.settingsField(), /checked/);
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`Agent API: ${tests.length - failures}/${tests.length} groups passed.`); if (failures) process.exitCode = 1; })();
