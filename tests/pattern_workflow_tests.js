'use strict';
// Pattern, song and step-recording workflows through the real app.js (held_notes_tests harness).
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const fixture = fs.readFileSync(path.join(__dirname, 'held_notes_tests.js'), 'utf8');
const exportLine = 'return { ST, state, window, node, calls,';
assert(fixture.includes(exportLine));
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')).replace(exportLine, exportLine.replace('return {', 'return { context,')) + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const tests = [], test = (name, run) => tests.push({ name, run });

test('removing a track inside a selection keeps the selection on the grid, so Delete still clears', async () => {
  const h = await makeHarness(), s = h.state;
  s.view = 'pattern'; s.selection = { track: 7, row: 0 }; s.track = 6; s.row = 1;
  h.ST.pattern().steps[6][0] = { note: 60, instrument: 6, velocity: 100, gate: .8 };
  h.ST.perform('removeTrack');
  assert.equal(s.project.tracks.length, 7); assert.equal(s.selection.track, 6);
  h.down('Backspace', 'Backspace');
  assert.equal(h.ST.pattern().steps[6][0], null);
});

test('step editor note and velocity fields clamp to MIDI ranges', async () => {
  const h = await makeHarness(), s = h.state;
  s.view = 'pattern'; s.track = 0; s.row = 3; h.ST.refresh();
  for (const [id, value] of [['step-velocity', '500'], ['step-note', '300.4']]) { const field = h.node(id); field.value = value; field.onchange({ target: field }); h.ST.refresh(); }
  let step = h.ST.pattern().steps[0][3]; assert.equal(step.velocity, 127); assert.equal(step.note, 127);
  for (const [id, value] of [['step-velocity', '0'], ['step-note', '-9']]) { const field = h.node(id); field.value = value; field.onchange({ target: field }); h.ST.refresh(); }
  step = h.ST.pattern().steps[0][3]; assert.equal(step.velocity, 1); assert.equal(step.note, -3);
});

test('song "+ Add pattern" keeps the block arranger handler instead of the legacy splice', async () => {
  const h = await makeHarness();
  h.window.STClips = { render: () => '', bind() { h.node('sequence-add').onclick = 'arranger'; }, key: () => false, event() {} };
  h.ST.setView('song');
  assert.equal(h.node('sequence-add').onclick, 'arranger');
});

test('removing a pattern keeps the song section loop on its remaining music and restores Loop song when emptied', async () => {
  const h = await makeHarness(), s = h.state, p = s.project;
  web.load(h.context, 'song-model.js');
  const add = () => { h.ST.perform('addPattern'); return p.patterns.at(-1).id; };
  const a = p.patterns[0].id, b = p.patterns[1].id, c = add();
  p.sequence = [a, b, b, c]; p.loopSong = false; h.window.STSongModel.setLoop(p, 1, 2); // loops b b c
  p.activePattern = b; h.ST.perform('removePattern');
  assert.deepEqual(copy(p.sequence), [a, c]); assert.deepEqual(copy(p.songLoop), { start: 1, end: 1, restoreLoopSong: false }); assert.equal(p.loopSong, true);
  p.activePattern = c; h.ST.perform('removePattern');
  assert.deepEqual(copy(p.sequence), [a]); assert.equal(p.songLoop, undefined); assert.equal(p.loopSong, false, 'Loop song returns to its value before the section loop');
  h.ST.perform('undo'); assert.deepEqual(copy(s.project.sequence), [a, c]); assert.equal(s.project.loopSong, true);
});

test('Shift+Return and the palette insert a row; the palette also deletes one', async () => {
  const h = await makeHarness(), s = h.state, steps = () => h.ST.pattern().steps[0];
  s.view = 'pattern'; s.track = 0; s.row = 0; steps().fill(null); steps()[0] = { note: 60, instrument: 0, velocity: 100, gate: .8 };
  h.down('Enter', 'Enter', { shiftKey: true });
  assert.equal(steps()[0], null); assert.equal(steps()[1].note, 60); assert.equal(steps().length, h.ST.pattern().length);
  h.ST.perform('deleteRow'); assert.equal(steps()[0].note, 60);
  h.ST.perform('insertRow'); assert.equal(steps()[1].note, 60);
});

test('a chord played on a MIDI keyboard during step entry lands on one row', async () => {
  for (const playing of [false, true]) {
    const h = await makeHarness(), s = h.state, project = s.project; let requestID = 0;
    h.context.setTimeout = () => 0; h.context.clearTimeout = () => {};
    project.patterns.forEach((pat, index) => { pat.id = index ? 19 : 7; pat.length = 8; pat.steps = project.tracks.map(() => Array(8).fill(null)); pat.automation = []; });
    project.activePattern = 7; project.sequence = [7, 19];
    // Stopped step entry, and the default "Selected step" placement while playing.
    Object.assign(s, { playing, recordPlacement: 'selected', row: 2, track: 0, instrument: 0, editStep: 1, status: { playing, pattern: 7, row: 0 }, audio: { inputAcknowledgements: { version: 1, supported: true } } });
    h.window.STHostTransport.post = message => h.window.STHostTransport.deliver({ id: message.id, ok: true, result: {} });
    await h.ST.setNoteRecording(true);
    const edges = new Map();
    const pending = (token, note) => { const payload = { on: true, inputOwner: token, recordingPending: true, source: 1, channel: 1, track: 0, instrument: 0, note, velocity: 93 }; edges.set(token, { payload, context: copy(h.window.STInputCapture.recordingContext()), requestID: String(++requestID) }); h.window.nativeEvent({ type: 'midi.note', payload }); };
    const ack = token => { const { payload: p, context, requestID: id } = edges.get(token); h.window.nativeEvent({ type: 'midi.note', payload: { ...p, recordingPending: false, recordingOnly: true, inputAcknowledgement: { version: 1, inputOwner: token, requestID: id, status: 'applied', applied: true, playing: false, recording: true, on: true, pattern: 7, row: 0, sequence: 0, track: 0, instrument: 0, note: p.note, velocity: p.velocity, applyNote: true, recordingContext: context } } }); };
    [60, 64, 67].forEach((note, i) => { pending('m' + i, note); h.advance(15); });
    ['m0', 'm1', 'm2'].forEach(ack); await tick();
    const rows = project.patterns[0].steps[0];
    assert.deepEqual([rows[2].note, copy(rows[2].chord)], [60, [0, 4, 7]], playing ? 'selected step while playing' : 'stopped');
    assert.equal(rows[3], null); assert.equal(s.row, playing ? 2 : 3);
    h.advance(500); pending('next', 62); ack('next'); await tick();
    assert.equal(rows[playing ? 2 : 3].note, 62, 'a later key is a separate entry');
  }
});

test('follow into a shorter pattern keeps the cursor and selection on existing rows', async () => {
  const h = await makeHarness(), s = h.state, p = s.project, short = p.patterns[1];
  short.length = 16; short.steps.forEach(steps => { steps.length = 16; });
  Object.assign(s, { view: 'pattern', follow: true, playing: true, track: 0, row: 40, selection: { track: 0, row: 50 } });
  h.window.nativeEvent({ type: 'status', payload: { playing: true, pattern: short.id, row: 0 } });
  assert.equal(h.ST.pattern(), short); assert.equal(s.row, 15); assert.equal(s.selection.row, 15);
  h.down('Backspace', 'Backspace'); assert.equal(short.steps[0].length, 16, 'clearing cannot grow the pattern data past its length');
});

(async () => {
  for (const { name, run } of tests) { await run(); console.log('PASS ' + name); }
})().catch(error => { console.error(error); process.exit(1); });
