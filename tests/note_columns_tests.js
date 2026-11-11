'use strict';
// Note columns (app-note-columns.js): several notes per row on one track, Renoise style. Column 0 is the step itself;
// step.columns[i] is note column i + 1. Editing (cursor, typed entry, OFF/CUT, Delete, transpose, clipboard, Undo),
// normalization, the header commands, recording into free columns (computer keyboard, acknowledged native input while
// stopped and live) and the agent API.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const derive = (file, marker) => {
  const source = fs.readFileSync(path.join(__dirname, file), 'utf8');
  return new Function('require', '__dirname', source.slice(0, source.indexOf('\n' + marker)) + '\nreturn harness;')(require, __dirname);
};
const editorHarness = derive('typed_entry_tests.js', 'const tests = [];');
const ackHarness = derive('input_ack_ui_tests.js', 'const tests = [], test =');
const agentHarness = derive('agent_api_tests.js', 'const tests = [], test =');
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };

// The demo project with track 0 cleared and given `columns` note columns.
function editor(columns = 2) {
  const h = editorHarness();
  const p = h.state.project, pat = h.ST.pattern();
  pat.steps[0] = Array(pat.length).fill(null);
  pat.steps[1] = Array(pat.length).fill(null);
  if (columns > 1) p.tracks[0].noteColumns = columns;
  Object.assign(h.state, { recording: true, noteColumn: 0, octave: 4, chord: 'single', scale: 'chromatic', editStep: 1 });
  h.state.undo = []; h.state.redo = [];
  return h;
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('normalize clamps note columns, drops cells beyond the count and leaves projects without columns unchanged', () => {
  const h = editor(1), app = h.app, plain = copy(h.state.project);
  assert.deepEqual(copy(app.normalizeNoteColumns(copy(plain))), plain, 'a project without note columns is not touched');
  const p = copy(plain);
  p.tracks[0].noteColumns = 12; p.tracks[1].noteColumns = 0; p.tracks[2].noteColumns = 2;
  const steps = p.patterns[0].steps;
  steps[0][0] = { note: 60, columns: [{ note: 62 }, null, { note: 64 }] };
  steps[1][0] = { note: -3, columns: [{ note: 62 }] };
  steps[2][0] = { note: 60, columns: [{ note: 62 }, { note: 64 }] };
  steps[2][1] = { note: -3, columns: [null, { note: 64 }] };
  steps[2][2] = { note: 60, columns: [{ note: 'x' }, null] };
  app.normalizeNoteColumns(p);
  assert.equal(p.tracks[0].noteColumns, 8); assert.equal(p.tracks[1].noteColumns, undefined); assert.equal(p.tracks[2].noteColumns, 2);
  assert.deepEqual(copy(steps[0][0].columns), [{ note: 62 }, null, { note: 64 }]);
  assert.equal(steps[1][0], null, 'a one-column track keeps no extra cells; an emptied holder goes');
  assert.deepEqual(copy(steps[2][0].columns), [{ note: 62 }]);
  assert.equal(steps[2][1], null);
  assert.deepEqual(copy(steps[2][2]), { note: 60 }, 'invalid and trailing empty cells go');
  const again = copy(p); app.normalizeNoteColumns(again); assert.deepEqual(again, copy(p), 'normalization is idempotent');
  // The full normalize() runs it too.
  const loaded = copy(plain); loaded.tracks[0].noteColumns = 3; loaded.patterns[0].steps[0][4] = { note: 60, columns: [null, null, { note: 67 }] };
  h.ST.normalize(loaded); assert.equal(loaded.patterns[0].steps[0][4].columns, undefined);
});

test('the grid renders NOTE INS VEL per note column and the header + / − buttons; one column renders exactly as before', () => {
  const h = editor(1), app = h.app, t = h.state.project.tracks[0];
  const step = { note: 60, instrument: 0, velocity: 100, gate: 0 };
  const single = app.renderPatternCellContent(step, t, 0, true);
  assert(!single.includes('data-note-column'));
  t.noteColumns = 3;
  step.columns = [{ note: 64, instrument: 2, velocity: 90, gate: 0 }, { note: -1 }];
  h.state.noteColumn = 1; h.state.column = 2;
  const html = app.renderPatternCellContent(step, t, 0, true);
  assert.equal((html.match(/data-column="0"/g) || []).length, 3, 'a NOTE per note column');
  assert.match(html, /data-column="2" data-note-column="1" class="has-velocity note-column-cell active-column">090/);
  assert.match(html, /data-column="0" data-note-column="2" class="has-note note-column-cell ">OFF/);
  const holder = app.renderPatternCellContent({ note: -3, columns: [{ note: 64 }] }, t, 0, false);
  assert.match(holder, /^<span data-column="0" class=" ">···<\/span><span data-column="1" class=" ">··<\/span>/, 'column 0 of a holder row is empty');
  h.ST.refresh();
  const header = app.patternColumnHeader(app.patternColumns()[0]);
  assert.match(header, /N2<\/span>.*N3<\/span>/); assert.match(header, /data-note-column-add="0"/); assert.match(header, /data-note-column-remove="0"/);
  assert.match(app.patternColumnCell(app.patternColumns()[0], 0, null, []), /grid-template-columns:repeat\(3,/);
  assert(app.patternColumnWidth(app.patternColumns()[0]) > app.patternColumnWidth(app.patternColumns()[1]), 'the virtual grid widens the track');
});

test('Left / Right walk NOTE INS VEL of every note column, then FX 1 and FX 2; Tab keeps the note column in range', () => {
  const h = editor(2), at = () => [h.state.track, h.state.noteColumn, h.state.column];
  for (const expected of [[0, 0, 1], [0, 0, 2], [0, 1, 0], [0, 1, 1], [0, 1, 2], [0, 0, 3], [0, 0, 4], [1, 0, 0]]) {
    h.key('ArrowRight'); assert.deepEqual(at(), expected);
  }
  h.key('ArrowLeft'); assert.deepEqual(at(), [0, 0, 4], 'entering a track from the right lands on FX 2');
  h.key('ArrowLeft'); h.key('ArrowLeft'); assert.deepEqual(at(), [0, 1, 2]);
  h.key('Tab'); assert.deepEqual(at(), [1, 0, 2], 'a one-column track has no note column 1');
});

test('typing a note into an extra column records it there, previews it and leaves column 0 alone; OFF, CUT, INS and VEL too', () => {
  const h = editor(2), steps = h.ST.pattern().steps[0];
  steps[0] = { note: 48, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'ratchet', value: 2 } };
  h.state.noteColumn = 1;
  h.key('z');
  assert.equal(steps[0].note, 48); assert.deepEqual(copy(steps[0].fx1), { type: 'ratchet', value: 2 });
  assert.deepEqual(copy(steps[0].columns), [{ note: 60, instrument: 0, velocity: 100, gate: 0 }]);
  assert.equal(h.state.undo.length, 1); assert.equal(h.state.row, 1);
  assert.equal(h.notesPlayed(), 1, 'the typed note sounds');
  h.app.noteOff('KeyZ');
  // Return auditions the extra column's note with the row's FX.
  h.state.row = 0; h.calls.length = 0; h.key('Enter');
  const preview = h.calls.find(call => call.command === 'step.preview');
  assert.equal(preview.payload.step.note, 60); assert.deepEqual(copy(preview.payload.step.fx1), { type: 'ratchet', value: 2 });
  // VEL digits and INS digits in the extra column.
  h.state.column = 2; h.type('64'); assert.equal(steps[0].columns[0].velocity, 64); assert.equal(steps[0].velocity, 100);
  h.state.row = 0; h.state.column = 1; h.type('03'); assert.equal(steps[0].columns[0].instrument, 3); assert.equal(steps[0].instrument, 0);
  // OFF / CUT end only this column.
  h.state.row = 1; h.state.column = 0; h.key('1');
  assert.deepEqual(copy(h.ST.pattern().steps[0][1]), { note: -3, columns: [{ note: -1 }] });
  h.key('!', { shiftKey: true }); assert.deepEqual(copy(h.ST.pattern().steps[0][2].columns), [{ note: -2 }]);
  // The step editor shows and edits the cursor's note column.
  h.state.row = 0; h.state.noteColumn = 1; h.ST.refresh();
  assert.match(h.app.renderPattern(), /STEP 00 · N2/);
  assert.match(h.app.renderPattern(), /id="step-velocity" type="number" min="1" max="127" value="64"/);
});

test('Delete without a selection clears the cursor column only; with a selection it clears whole steps; both one Undo step', () => {
  const h = editor(3), steps = h.ST.pattern().steps[0];
  steps[0] = { note: 60, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'pan', value: .5 }, columns: [{ note: 64, instrument: 0, velocity: 100, gate: 0 }, { note: 67, instrument: 0, velocity: 100, gate: 0 }] };
  h.state.noteColumn = 2; h.key('Delete');
  assert.deepEqual(copy(steps[0].columns), [{ note: 64, instrument: 0, velocity: 100, gate: 0 }]); assert.equal(h.state.undo.length, 1);
  h.state.noteColumn = 0; h.key('Delete');
  assert.deepEqual(copy(steps[0]), { note: -3, columns: [{ note: 64, instrument: 0, velocity: 100, gate: 0 }] }, 'column 0 loses its note and FX, the other column stays');
  h.state.noteColumn = 1; h.key('Delete'); assert.equal(steps[0], null, 'the empty holder goes');
  steps[3] = { note: 60, columns: [{ note: 64 }] }; h.state.selection = { track: 0, row: 3 }; h.state.row = 4; h.key('Delete');
  assert.equal(steps[3], null); assert.equal(h.state.undo.length, 4);
  h.ST.undo?.() ?? h.app.undo(); assert.deepEqual(copy(h.ST.pattern().steps[0][3]), { note: 60, columns: [{ note: 64 }] });
});

test('transpose: a selection moves every note column, the cursor alone moves its own column', () => {
  const h = editor(2), steps = h.ST.pattern().steps[0];
  steps[0] = { note: 60, instrument: 0, velocity: 100, gate: 0, columns: [{ note: 64, instrument: 0, velocity: 100, gate: 0 }] };
  steps[1] = { note: -3, columns: [{ note: 67, instrument: 0, velocity: 100, gate: 0 }] };
  h.state.selection = { track: 0, row: 0 }; h.state.row = 1;
  h.app.transpose(1);
  assert.deepEqual([steps[0].note, steps[0].columns[0].note, steps[1].columns[0].note], [61, 65, 68]); assert.equal(h.state.undo.length, 1);
  h.state.selection = null; h.state.row = 0; h.state.noteColumn = 1;
  h.app.transpose(12); assert.deepEqual([steps[0].note, steps[0].columns[0].note], [61, 77]);
  assert.equal(h.calls.filter(call => call.command === 'step.preview').at(-1).payload.step.note, 77, 'the transposed column is auditioned');
});

test('copy and paste carry note columns; the destination track grows in the same Undo step', () => {
  const h = editor(3), p = h.state.project, steps = h.ST.pattern().steps;
  steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 0, columns: [null, { note: 67, instrument: 0, velocity: 100, gate: 0 }] };
  h.app.copySelection();
  h.state.track = 1; h.state.row = 4; h.app.pasteSelection();
  assert.deepEqual(copy(steps[1][4].columns), [null, { note: 67, instrument: 0, velocity: 100, gate: 0 }]);
  assert.equal(p.tracks[1].noteColumns, 3); assert.equal(h.state.undo.length, 1);
  h.app.undo(); assert.equal(h.state.project.tracks[1].noteColumns, undefined); assert.equal(h.ST.pattern().steps[1][4], null);
});

test('add / remove note column: commands, the cursor moves into a new column, removing one with notes asks first, one Undo step', async () => {
  const h = editor(1), p = h.state.project, asked = [];
  assert(h.app.hasCommand('addNoteColumn') && h.app.hasCommand('removeNoteColumn'));
  h.app.perform('addNoteColumn');
  assert.equal(h.state.project.tracks[0].noteColumns, 2); assert.equal(h.state.noteColumn, 1); assert.equal(h.state.undo.at(-1).label, 'add note column');
  for (let n = 0; n < 7; n++) h.app.addNoteColumn(0);
  assert.equal(h.state.project.tracks[0].noteColumns, 8, 'at most eight');
  h.app.setNoteColumns(0, 2);
  h.ST.pattern().steps[0][5] = { note: 60, columns: [{ note: 64, instrument: 0, velocity: 100, gate: 0 }] };
  h.app.confirmChoice = async (title, body) => { asked.push(body); return false; };
  assert.equal(await h.app.removeNoteColumn(0), false); assert.equal(h.state.project.tracks[0].noteColumns, 2);
  assert.match(asked[0], /Note column 2 of .* has 1 note/);
  h.app.confirmChoice = async () => true;
  const undo = h.state.undo.length;
  assert(await h.app.removeNoteColumn(0));
  assert.equal(h.state.project.tracks[0].noteColumns, undefined); assert.deepEqual(copy(h.ST.pattern().steps[0][5]), { note: 60 });
  assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.noteColumn, 0);
  h.app.undo(); assert.equal(h.state.project.tracks[0].noteColumns, 2); assert.equal(h.ST.pattern().steps[0][5].columns[0].note, 64);
  // An empty column goes without asking.
  asked.length = 0; h.app.confirmChoice = async (title, body) => { asked.push(body); return false; };
  h.ST.pattern().steps[0][5] = { note: 60 }; assert(await h.app.removeNoteColumn(0)); assert.equal(asked.length, 0);
  assert(p);
});

test('a track deleted while a confirmation or rename prompt is open: the column removal and the rename never hit the track that moved into its index', async () => {
  const h = editor(2), p = h.state.project;
  p.tracks[1].noteColumns = 2;
  h.ST.pattern().steps[0][3] = { note: 60, columns: [{ note: 64 }] };
  h.ST.pattern().steps[1][3] = { note: 62, columns: [{ note: 65 }] };
  const second = p.tracks[1];
  // e.g. an AI control edit deletes track 0 while "Remove note column?" is open.
  h.app.confirmChoice = async () => { p.tracks.splice(0, 1); for (const pat of p.patterns) pat.steps.splice(0, 1); return true; };
  assert.equal(await h.app.removeNoteColumn(0), false, 'the removal is dropped');
  assert.equal(p.tracks[0], second); assert.equal(second.noteColumns, 2, 'the shifted track keeps its note column');
  assert.equal(h.ST.pattern().steps[0][3].columns[0].note, 65, 'and its notes');
  // Rename: the prompt names the selected track; renaming after a deletion renames that track, not its successor.
  p.tracks.push({ ...copy(p.tracks[0]), name: 'Third' }); for (const pat of p.patterns) pat.steps.push(Array(pat.length).fill(null));
  h.state.track = 1; const renamed = p.tracks[1];
  h.app.perform('renameTrack');
  p.tracks.splice(0, 1); for (const pat of p.patterns) pat.steps.splice(0, 1);
  h.element('#text-prompt').value = 'Lead'; h.element('#prompt-accept').onclick();
  assert.equal(renamed.name, 'Lead'); assert.equal(p.tracks[0], renamed);
  assert.equal(p.tracks.filter(track => track.name === 'Lead').length, 1);
});

test('computer-keyboard recording: notes held together take free note columns of one row; then the cursor moves on', () => {
  const h = editor(3), steps = h.ST.pattern().steps[0];
  h.key('z'); h.key('c'); h.key('b');
  assert.equal(steps[0].note, 60); assert.deepEqual(copy(steps[0].columns.map(cell => cell.note)), [64, 67]);
  assert.equal(h.state.row, 1, 'the held notes share the row; the edit step applies once');
  h.key('m'); // a fourth held note: every column is held, so it is entered as on a one-column track (cursor row, column 0)
  assert.equal(h.ST.pattern().steps[0][1].note, 71); assert.equal(h.state.row, 2);
  for (const code of ['KeyZ', 'KeyC', 'KeyB', 'KeyM']) h.app.noteOff(code);
  h.state.noteColumn = 2; h.key('x');
  assert.deepEqual(copy(h.ST.pattern().steps[0][2]), { note: -3, columns: [null, { note: 62, instrument: 0, velocity: 100, gate: 0 }] }, 'released columns are free again; the cursor column comes first');
  // A one-column track keeps chord-free single-note entry.
  const one = editor(1);
  one.key('z'); one.app.noteOff('KeyZ'); one.key('c');
  assert.equal(one.ST.pattern().steps[0][0].note, 60); assert.equal(one.ST.pattern().steps[0][1].note, 64); assert.equal(one.ST.pattern().steps[0][0].columns, undefined);
});

test('acknowledged native input: held notes record into their own columns, stopped and live, each with its own gate', async () => {
  // Live: two notes on row 0, released at different times.
  const h = await ackHarness();
  h.project.tracks[0].noteColumns = 2;
  h.down('z', 'KeyZ'); h.down('c', 'KeyC');
  const [first, second] = h.captures();
  h.reply(first, { row: 0, recordingClock: { distanceBeats: 0, runID: '1' } });
  h.reply(second, { row: 0, recordingClock: { distanceBeats: 0, runID: '1' } });
  await tick();
  assert.equal(h.step(0).note, 60); assert.equal(h.step(0).columns[0].note, 64);
  h.up('z', 'KeyZ'); h.reply(h.captures(false)[0], { row: 2, recordingClock: { distanceBeats: .25, runID: '1' } }); await tick();
  h.up('c', 'KeyC'); h.reply(h.captures(false)[1], { row: 4, recordingClock: { distanceBeats: .5, runID: '1' } }); await tick();
  assert.equal(h.step(0).gate, 1); assert.equal(h.step(0).columns[0].gate, 2, 'each column keeps its own held length');
  assert.equal(h.state.undo.length, 1, 'one recording pass is one Undo step');
  // Live: a second note while the first is held lands on a later row in another column, so the first keeps sounding.
  const later = await ackHarness();
  later.project.tracks[0].noteColumns = 2;
  later.down('z', 'KeyZ'); later.reply(later.captures()[0], { row: 0, recordingClock: { distanceBeats: 0, runID: '1' } }); await tick();
  later.down('x', 'KeyX'); later.reply(later.captures()[1], { row: 3, recordingClock: { distanceBeats: .75, runID: '1' } }); await tick();
  assert.equal(later.step(0).note, 60); assert.equal(later.step(3).note, -3); assert.equal(later.step(3).columns[0].note, 62);
  // Stopped: held notes share the cursor row.
  const stopped = await ackHarness();
  stopped.project.tracks[0].noteColumns = 3; stopped.state.playing = false; stopped.state.row = 2;
  stopped.down('z', 'KeyZ'); stopped.down('c', 'KeyC');
  for (const capture of stopped.captures()) stopped.reply(capture, { playing: false, recordingClock: undefined });
  await tick();
  assert.equal(stopped.step(2).note, 60); assert.deepEqual(copy(stopped.step(2).columns.map(cell => cell.note)), [64]);
  assert.equal(stopped.state.row, 3); assert.equal(stopped.step(3), null);
});

test('agent API: writeSteps with column, readPattern shows columns, transform and clear reach every column', async () => {
  const h = agentHarness();
  const p = h.state.project;
  p.patterns[0].steps[0] = Array(p.patterns[0].length).fill(null);
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, note: 'C-4' }, { track: 0, row: 0, column: 2, note: 'G-4', velocity: 90 }, { track: 0, row: 1, column: 1, note: 'OFF' }] });
  const step = h.state.project.patterns[0].steps[0][0];
  assert.equal(h.state.project.tracks[0].noteColumns, 3, 'writing column 2 gives the track three note columns');
  assert.deepEqual(copy(step.columns), [null, { note: 67, instrument: h.state.project.tracks[0].instrument ?? 0, velocity: 90, gate: 0 }]);
  const read = await h.ok('readPattern', { tracks: [0], from: 0, to: 1 });
  assert.equal(read.tracks[0].noteColumns, 3);
  assert.deepEqual(read.tracks[0].steps[0].columns, [{ column: 2, note: 'G-4', midi: 67, instrument: step.columns[1].instrument, velocity: 90 }]);
  assert.deepEqual(read.tracks[0].steps[1].columns, [{ column: 1, note: 'OFF' }]);
  const grid = await h.ok('readPattern', { tracks: [0], from: 0, to: 0, format: 'grid' });
  assert.match(grid.lines[0], /C-4.* --- -- --- G-4/);
  await h.ok('transform', { tracks: [0], from: 0, to: 1, operation: 'transpose', amount: 2 });
  assert.equal(h.state.project.patterns[0].steps[0][0].note, 62); assert.equal(h.state.project.patterns[0].steps[0][0].columns[1].note, 69);
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, clear: true }] });
  assert.deepEqual(copy(h.state.project.patterns[0].steps[0][0]), { note: -3, columns: [null, { note: 69, instrument: step.columns[1].instrument, velocity: 90, gate: 0 }] }, 'clearing column 0 keeps the others');
  await h.ok('writeSteps', { steps: [{ track: 0, row: 0, column: 2, clear: true }] });
  assert.equal(h.state.project.patterns[0].steps[0][0], null);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, column: 1, notes: ['C-4', 'E-4'] }] }, /belongs to note column 0/);
  await h.rejects('writeSteps', { steps: [{ track: 0, row: 0, column: 8, note: 'C-4' }] }, /column must be a note column 0–7/);
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) {
    try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); }
  }
  console.log(`Note columns: ${tests.length - failures}/${tests.length} groups passed.`);
  if (failures) process.exitCode = 1;
})();
