'use strict';
// Actual app keyboard and recording handlers with parsed replaceable DOM and a simulated bridge. A note begins
// as an acknowledged input edge does (app-input-capture.js), with the engine clock of the edge. The clocks are
// fixtures, not an audio claim.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'scalar_field_focus_tests.js'), 'utf8');
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')) + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
const clock = (distanceBeats, extra = {}) => ({ distanceBeats, runID: '1', ...extra });
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
function harness() {
  const h = makeHarness(), warnings = []; let now = 100000, message = '';
  h.context.Date = class extends Date { static now() { return now; } };
  Object.defineProperty(h.toast, 'innerHTML', { get: () => message, set(value) { message = value; if (value.includes('Exact recorded-note duration')) warnings.push(value); } });
  const project = h.state.project; project.bpm = 120; project.linesPerBeat = 4; project.swing = 0; project.sequence = [0]; project.loopSong = true;
  // Held-note durations are recorded only while the optional Gate column is shown.
  project.viewState = { gateColumn: true };
  project.patterns[0].length = 32; project.patterns[0].steps.forEach(steps => { steps.length = 32; });
  Object.assign(h.state, { recordPlacement:'live', playing: true, recording: true, keyboardPlay: true, row: 0, track: 0, instrument: 0, status: { playing: true, pattern: 0, row: 0, seconds: 0, beat: 0 } });
  h.ST.refresh(); h.calls.length = 0;
  function status(seconds, beat, row = Math.floor(beat * 4), extra = {}) { h.window.nativeEvent({ type: 'status', payload: { playing: true, songMode: true, pattern: 0, sequence: 0, row, position: row, seconds, beat, ...extra } }); }
  return { ...h, project, warnings, status, advance(seconds) { now += seconds * 1000; }, step: (row = 0) => h.ST.pattern().steps[0][row], begin(row = 0, stamp, note = 60) { return h.ST.recordLiveStep(note, 100, 0, 0, 0, row, [0], stamp); } };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('acknowledged live notes repaint the mounted cell without replacing focus or the grid', async () => {
  const h = harness(), tracker = h.get('tracker'); tracker.focus();
  const cell = h.document.querySelector('[data-cell="0:0"]'), content = cell.querySelector('.tracker-cell'), renders = h.renders;
  h.key(tracker, 'z', { code: 'KeyZ' }); await tick();
  assert.match(content.innerHTML, /C–4/);
  assert.match(content.innerHTML, /has-note/);
  assert.equal(h.document.activeElement, tracker); assert.equal(tracker.isConnected, true);
  assert.equal(h.document.querySelector('[data-cell="0:0"]'), cell);
  assert.equal(h.renders, renders);
  h.begin(0, clock(0), 64); assert.match(content.innerHTML, /C–4/);
  assert(cell.classList.contains('has-chord')); assert.match(cell.attrs.title, /E–4/);
  h.begin(2, clock(.5), 67);
  assert.match(h.document.querySelector('[data-cell="0:2"]').querySelector('.tracker-cell').innerHTML, /G–4/);
});

test('without the Gate column a released key records an OFF on its row, unless a later note ends it first', () => {
  const h = harness(); h.project.viewState = { gateColumn: false }; h.ST.pattern().steps[0].fill(null);
  const held = h.begin(0, clock(0)); h.ST.finishLiveRecord(held, clock(2));
  assert.equal(h.step(0).note, 60); assert.equal(h.step(8).note, -1, 'held two beats at four rows per beat: OFF on row 8');
  assert.equal(h.state.undo.length, 1, 'the note and its OFF are one recording take');
  const ended = h.begin(10, clock(2.5), 62); h.begin(11, clock(2.75), 64); h.ST.finishLiveRecord(ended, clock(3.5));
  assert.equal(h.step(14)?.note, undefined, 'a later note before the release already ends the held note: no OFF');
  const short = h.begin(20, clock(5)); h.ST.finishLiveRecord(short, clock(5.01));
  assert.equal(h.step(21).note, -1, 'a short tap still gets its OFF on the next row');
  const unclocked = h.begin(26, clock(7)); h.ST.finishLiveRecord(unclocked);
  assert.equal(h.step(27), null, 'a release without its engine clock writes no OFF'); assert.equal(h.warnings.length, 0, 'and warns only where a gate was asked for');
});

test('an exact note without the engine clock of its edges keeps no gate and warns once per recording session', () => {
  const h = harness(); h.status(1.5, 3, 0, { recordingClock: clock(3) });
  const first = h.begin(0); h.advance(1.5); h.status(3, 5, 0, { recordingClock: clock(5) }); h.ST.finishLiveRecord(first);
  assert.equal(h.step().gate, 0, 'the status clock and the elapsed time are not the note\'s edges'); assert.equal(h.state.undo.length, 1); assert.equal(h.warnings.length, 1);
  const second = h.begin(1); h.advance(1.5); h.status(.5, 1); h.ST.finishLiveRecord(second);
  assert.equal(h.step(1).gate, 0); assert.equal(h.warnings.length, 1, 'Warn once per project/session rather than on every note');
  h.ST.finishLiveRecord(second); assert.equal(h.state.undo.length, 1, 'notes and gates of one recording pass are one Undo step'); assert.equal(h.warnings.length, 1);
});

test('exact input clocks measure the held beats across loop wraps and skipped whole cycles', () => {
  for (const [name, start, end, startSeconds, endSeconds, duration, expected] of [
    ['inside the pattern', 3, 5, 1.5, 2.5, 1, 8],
    ['song wrap', 7, 9, 3.5, .5, 1, 8],
    ['skipped whole cycle', 1, 10, .5, 1, 4.5, 36],
  ]) {
    const h = harness(); h.status(startSeconds, start % 8); const record = h.begin(0, clock(start));
    h.advance(duration); h.status(endSeconds, end % 8); h.ST.finishLiveRecord(record, clock(end));
    assert.equal(h.step().gate, expected, name); assert.equal(h.state.undo.length, 1); assert.equal(h.warnings.length, 0); assert.equal(h.calls.length, 0);
    h.ST.finishLiveRecord(record, clock(end)); assert.equal(h.state.undo.length, 1, 'Repeated note-off cannot finalize again');
  }
});

test('exact gate uses the onset row\'s swing and per-voice microtiming/ratchet spacing while preserving both FX columns', () => {
  const plugin = { type: 'plugin', instanceID: 'original-plugin', address: '18446744073709551615', value: .37 };
  for (const [name, row, swing, fx1, fx2, beats, expected] of [
    ['even swung row', 0, .5, null, null, .75, 2],
    ['odd swung row', 1, .5, null, null, .75, 6],
    ['combined micro/ratchet', 0, .5, { type: 'microtiming', value: .5 }, { type: 'ratchet', value: 3 }, .75, 12],
    ['FX2 micro wins', 0, 0, { type: 'microtiming', value: .25 }, { type: 'microtiming', value: .5 }, .5, 4],
    ['FX2 truncated ratchet wins', 0, 0, { type: 'ratchet', value: 8 }, { type: 'ratchet', value: 2.9 }, .5, 4],
    ['native ratchet clamp', 0, 0, { type: 'ratchet', value: 99 }, null, .5, 16],
    ['native micro clamp', 0, 0, { type: 'microtiming', value: .99 }, null, .5, 40],
    ['unrelated effects remain exact', 0, 0, plugin, { type: 'slice', value: 4 }, .5, 2],
  ]) {
    const h = harness(); h.project.swing = swing; h.ST.pattern().steps[0][row] = { note: -3, fx1, fx2 };
    const record = h.begin(row, clock(100)); h.ST.finishLiveRecord(record, clock(100 + beats));
    assert.equal(h.step(row).gate, expected, name); assert.deepEqual(copy(h.step(row).fx1 ?? null), fx1, name); assert.deepEqual(copy(h.step(row).fx2 ?? null), fx2, name); assert.equal(h.warnings.length, 0);
  }
});

test('chords use the earliest compatible onset and last release, with no duration history before the final owner', () => {
  const h = harness(), first = h.begin(0, clock(10)), second = h.begin(0, clock(10.1), 64);
  h.ST.finishLiveRecord(first, clock(10.3)); assert.equal(h.step().gate, 0); assert.equal(h.state.undo.length, 1); assert.equal(h.warnings.length, 0);
  h.ST.finishLiveRecord(second, clock(11)); assert.equal(h.step().gate, 4); assert.deepEqual(copy(h.step().chord), [0, 4]); assert.equal(h.state.undo.length, 1);
  h.ST.finishLiveRecord(first, clock(12)); h.ST.finishLiveRecord(second, clock(12)); assert.equal(h.state.undo.length, 1);
  const invalid = harness(), a = invalid.begin(0, clock(10)), b = invalid.begin(0, clock(10.1), 64);
  invalid.ST.finishLiveRecord(a); invalid.ST.finishLiveRecord(b, clock(11)); assert.equal(invalid.step().gate, 0); assert.equal(invalid.warnings.length, 1); assert.equal(invalid.state.undo.length, 1);
});

test('missing, invalid, reversed or reset clocks keep no gate and add no duration Undo', () => {
  for (const [start, end] of [
    [undefined, clock(2)], [clock(1), undefined], [clock(NaN), clock(2)], [clock(1), clock(Infinity)], [clock('1'), clock(2)],
    [clock(1), clock(.5)], [clock(1), clock(2, { runID: '2' })],
    [clock(1, { runID: '' }), clock(2)],
    ...['0','run-1','01','18446744073709551616'].map(runID => [clock(1, {runID}), clock(2, {runID})]),
  ]) {
    const h = harness(), record = h.begin(0, start); h.advance(60); h.ST.finishLiveRecord(record, end);
    assert.equal(h.step().gate, 0); assert.equal(h.state.undo.length, 1); assert.equal(h.warnings.length, 1);
  }
  const h = harness(), stamp = clock(1), record = h.begin(0, stamp); stamp.distanceBeats = 99;
  h.ST.finishLiveRecord(record, clock(2)); assert.equal(h.step().gate, 4, 'Caller mutation cannot alter the captured onset');
});

test('timing edits, project replacement and busy transactions cannot attach stale durations or create phantom MIDI records', () => {
  // A change of the row spacing leaves no gate and says so; a step edited meanwhile is no longer the recording's to write.
  for (const [change, warnings] of [[h => { h.project.linesPerBeat = 8; }, 1], [h => { h.project.swing = .2; }, 1], [h => { h.step().fx1 = { type: 'ratchet', value: 2 }; }, 0]]) {
    const h = harness(), record = h.begin(0, clock(1)); change(h); h.ST.finishLiveRecord(record, clock(2)); assert.equal(h.step().gate, 0); assert.equal(h.state.undo.length, 1); assert.equal(h.warnings.length, warnings);
  }
  const old = harness(), record = old.begin(0, clock(1)); old.state.project = copy(old.project); old.ST.finishLiveRecord(record, clock(2)); assert.equal(old.step().gate, 0); assert.equal(old.state.undo.length, 1); assert.equal(old.warnings.length, 0);
  for (const [module, flag] of [['STExport', 'isBusy'], ['STRouting', 'isApplying']]) {
    const h = harness(); h.window[module] = { [flag]: true }; const before = copy(h.project); assert.equal(h.begin(0, clock(1)), null);
    h.window[module][flag] = false; assert.deepEqual(copy(h.project), before); assert.equal(h.state.undo.length, 0); assert.equal(h.warnings.length, 0);
  }
});

test('a new recording session can warn once again', async () => {
  const exact = harness(); exact.ST.finishLiveRecord(exact.begin()); exact.ST.finishLiveRecord(exact.begin(1)); assert.equal(exact.warnings.length, 1);
  await exact.ST.setNoteRecording(true, { newSession: true }); exact.ST.finishLiveRecord(exact.begin(2)); assert.equal(exact.warnings.length, 2);
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`Live recording timing: ${tests.length - failures}/${tests.length} regression groups passed.`); if (failures) process.exitCode = 1; })();
