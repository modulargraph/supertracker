'use strict';
// Regressions in the app core (session replacement, track moves, transport readout, performance controls,
// browser preview) through the real app.js (held_notes_tests harness). Run: node tests/app_core_regressions_tests.js
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const fixture = fs.readFileSync(path.join(__dirname, 'held_notes_tests.js'), 'utf8');
const exportLine = 'return { ST, state, window, node, calls,';
assert(fixture.includes(exportLine));
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')).replace(exportLine, exportLine.replace('return {', 'return { context,')) + '\nreturn harness;')(require, __dirname);
const tests = [], test = (name, run) => tests.push({ name, run });

test('a replaced session drops the selection anchor and the pattern loop of the previous one', async () => {
  const h = await makeHarness(), s = h.state;
  Object.assign(s, { view: 'pattern', track: 7, row: 12, selection: { track: 7, row: 10 }, patternLoop: { pattern: 0, start: 10, end: 12 } });
  h.ST.acceptProject(h.window.STApp.blankProject());
  assert.equal(s.project.tracks.length, 1);
  assert.equal(s.selection, null, 'the anchor pointed at track 8 of the previous session');
  assert.equal(h.ST.selectedBounds(), null);
  assert.equal(s.patternLoop, null); assert.equal(h.ST.patternLoopFor(), null, 'pattern 0 of the new session is not looped');
  assert.doesNotThrow(() => h.down('Backspace', 'Backspace'));
});

test('moving a track past the first or last position is not an edit', async () => {
  const h = await makeHarness(), s = h.state;
  s.project.tracks.forEach((track, index) => { track.routingID = 'route-' + index; });
  const project = s.project, names = () => Array.from(s.project.tracks, track => track.name), before = names();
  Object.assign(s, { view: 'pattern', track: 0 });
  assert.equal(h.ST.perform('moveTrackLeft'), false);
  s.track = project.tracks.length - 1;
  assert.equal(h.ST.perform('moveTrackRight'), false);
  assert.equal(s.project, project, 'no rollback replaced the project (a queued engine update would be dropped)');
  assert.equal(s.undo.length, 0); assert.deepEqual(names(), before);
  // A real move is still one Undo step and the cursor follows the track.
  s.track = 0;
  assert.equal(h.ST.perform('moveTrackRight'), true);
  assert.deepEqual(names().slice(0, 2), [before[1], before[0]]); assert.equal(s.track, 1); assert.equal(s.undo.length, 1);
});

test('a new pattern or track does not inherit the selection anchor of where the cursor was', async () => {
  for (const command of ['addPattern', 'duplicatePattern', 'addTrack']) {
    const h = await makeHarness(), s = h.state;
    Object.assign(s, { view: 'pattern', track: 2, row: 12, selection: { track: 2, row: 10 } });
    h.ST.perform(command);
    assert.equal(s.undo.length, 1, command);
    // Before: rows 0–10 of the new pattern, or every track between the anchor and the new track, were selected.
    assert.equal(h.ST.selectedBounds(), null, command);
  }
});

test('Enter in a text prompt accepts it and is cancelled, so it cannot press the button that regains focus', async () => {
  // Accepting returns focus to the opening control (a Rename button); the uncancelled key then pressed it and reopened the prompt.
  const h = await makeHarness(), s = h.state, enter = extra => ({ key: 'Enter', preventDefault() { this.defaultPrevented = true; }, ...extra });
  const names = [];
  h.ST.textPrompt('Track name', 'Old', name => names.push(name));
  assert.equal(s.modal, 'dialog');
  h.node('text-prompt').value = 'Kana';
  const composing = enter({ isComposing: true });
  h.node('text-prompt').onkeydown(composing);
  assert.equal(composing.defaultPrevented, undefined); assert.equal(s.modal, 'dialog', 'Enter that confirms an input-method composition stays in the field');
  const accept = enter();
  h.node('text-prompt').onkeydown(accept);
  assert.equal(accept.defaultPrevented, true); assert.equal(s.modal, null); assert.deepEqual(names, ['Kana']);
  const asked = h.ST.askText('Kit name', 'Kit');
  h.node('text-prompt').value = 'Drums';
  const second = enter();
  h.node('text-prompt').onkeydown(second);
  assert.equal(second.defaultPrevented, true); assert.equal(await asked, 'Drums');
});

test('Stop returns the cursor to row 0 without stretching the selection up to it', async () => {
  const h = await makeHarness(), s = h.state;
  Object.assign(s, { view: 'pattern', track: 2, row: 11, selection: { track: 2, row: 10 } });
  assert.equal(await h.ST.perform('stop'), true);
  assert.equal(s.row, 0);
  assert.equal(h.ST.selectedBounds(), null, 'rows 0–10 are not selected (Delete would clear them)');
});

test('the Tempo field sets the project tempo as one Undo step and keeps it inside 20–400 BPM', async () => {
  const h = await makeHarness(), s = h.state, before = s.project.bpm;
  const field = h.node('tempo'); field.value = '140'; field.onchange({ target: field });
  assert.equal(s.project.bpm, 140); assert.equal(s.undo.length, 1);
  h.ST.perform('undo');
  assert.equal(s.project.bpm, before);
  field.value = '900'; field.onchange({ target: field });
  assert.equal(s.project.bpm, 400);
});

test('the Seq readout shows the selected song position while stopped and the playing one during playback', async () => {
  const h = await makeHarness(), s = h.state;
  s.project.sequence = [0, 0, 1, 1];
  // The engine keeps reporting its own (last played) sequence position while stopped.
  Object.assign(s, { playing: false, sequence: 2, row: 5, status: { playing: false, sequence: 0, row: 0 } });
  h.ST.updateTransport();
  assert.equal(h.node('position').textContent, '03 : 05');
  Object.assign(s, { playing: true, status: { playing: true, sequence: 1, row: 9 } });
  h.ST.updateTransport();
  assert.equal(h.node('position').textContent, '02 : 09');
});

test('the Instrument view chord control is saved with the song and leaves a chord set, like the Pattern view control', async () => {
  const h = await makeHarness(), s = h.state;
  s.view = 'instrument'; h.ST.refresh();
  h.ST.setPerformance({ chordSet: 'major-triads' }, false);
  h.ST.refresh();
  s.dirty = false;
  h.node('instrument-chord').onchange({ target: { value: 'minor' } });
  assert.equal(s.chord, 'minor'); assert.equal(s.chordSet, '', 'the chosen chord plays instead of the chord set');
  assert.equal(s.project.performance.chord, 'minor'); assert.equal(s.project.performance.chordSet, undefined);
  assert.equal(s.dirty, true);
});

test('the browser preview answers the seek before Play and step auditions', async () => {
  const h = await makeHarness(), s = h.state;
  s.native = false;
  assert.equal(JSON.stringify(await h.ST.request('transport.seek', { pattern: 0, row: 0, sequence: 0 })), '{}');
  assert.equal(JSON.stringify(await h.ST.request('step.preview', { track: 0, step: { note: 60 } })), '{}');
  // Play reaches its own explanation instead of failing on the seek.
  s.view = 'pattern';
  assert.equal(await h.ST.perform('play'), true);
  assert.match(h.node('toast-region').innerHTML, /Interface preview: open the macOS app for audio playback/);
  assert.equal(s.playing, false);
});

test('steps pasted from the system clipboard need typed values, and step values never add markup to the grid', async () => {
  const h = await makeHarness(), s = h.state, app = h.window.STApp, bad = '"><img src=x onerror="alert(1)">';
  Object.assign(s, { view: 'pattern', track: 0, row: 1 }); h.node('tracker').focus();
  const paste = steps => h.ST.clipboardEvent({ type: 'paste', preventDefault() {},
    clipboardData: { getData: type => type === 'text/plain' ? JSON.stringify({ supertracker: 1, steps }) : '' } });
  const before = JSON.stringify(h.ST.pattern().steps[0][1]);
  for (const step of [{ note: 60, instrument: bad }, { note: 60, velocity: bad }, { note: 60, gate: bad }, { note: 60, chord: [0, bad] },
    { note: 60, fx1: { type: 'pan', value: bad } }, { note: 60, fx2: bad }, { note: 60, columns: [{ note: 62, instrument: bad }] }, { note: 60, columns: bad }, [60]]) {
    paste([[step]]);
    assert.equal(JSON.stringify(h.ST.pattern().steps[0][1]), before, JSON.stringify(step));
    assert.match(h.node('toast-region').innerHTML, /Clipboard does not contain SuperTracker steps/);
  }
  assert.equal(s.undo.length, 0);
  // Everything the app copies itself still pastes: chords, effects, plug-in locks, gates and extra note columns.
  const good = { note: 60, instrument: 1, velocity: 90, gate: null, chord: [0, 4, 7], fx1: { type: 'ratchet', value: 2 },
    fx2: { type: 'plugin', value: .5, name: 'Cutoff', address: 12 }, columns: [null, { note: -1 }] };
  paste([[good, null]]);
  assert.equal(JSON.stringify(h.ST.pattern().steps[0][1]), JSON.stringify(good)); assert.equal(h.ST.pattern().steps[0][2], null);
  // A project file can still hold a string where a number belongs: the cell shows it as text.
  const html = app.renderPatternCellContent({ note: 60, instrument: bad, velocity: bad }, s.project.tracks[0], 0, false);
  assert.equal(/<img/.test(html), false); assert.match(html, /&lt;img/);
});

(async () => {
  for (const { name, run } of tests) { await run(); console.log('PASS ' + name); }
})().catch(error => { console.error(error); process.exit(1); });
