'use strict';
// Actual app boot, keyboard/MIDI dispatch, Save and Undo with controlled native
// replies. These fixtures exercise the JSON contract; no native audio runs.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'held_notes_tests.js'), 'utf8');
const exportLine = 'return { ST, state, window, node, calls,';
assert(fixture.includes(exportLine));
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')).replace(exportLine, exportLine.replace('return {', 'return { context,')) + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const clock = (distanceBeats, extra = {}) => ({ distanceBeats, runID: '1', ...extra });

async function harness(options = {}) {
  const h = await makeHarness(), contexts = new Map(), requestIDs = new Map(), timers = new Map(), heldCommands = new Set(), midiEdges = new Map();
  let timerID = 100000, requestID = 0;
  h.context.setTimeout = (fn, delay = 0) => { timers.set(++timerID, { fn, delay }); return timerID; };
  h.context.clearTimeout = id => timers.delete(id);
  const project = h.state.project;
  project.bpm = 120; project.linesPerBeat = 4; project.swing = 0; project.loopSong = true;
  // Held-note durations are recorded only while the optional Gate column is shown.
  project.viewState = { gateColumn: true };
  project.patterns.forEach((pat, index) => { pat.id = index ? 19 : 7; pat.length = 8; pat.steps = project.tracks.map(() => Array(8).fill(null)); pat.automation = []; });
  project.activePattern = 7; project.sequence = [7, 19];

  Object.assign(h.state, { recordPlacement:'live', playing: true, row: 0, track: 0, instrument: 0, status: { playing: true, pattern: 7, row: 0, seconds: 0 }, audio: { inputAcknowledgements: { version: 1, supported: true } } });
  await h.ST.setNoteRecording(true); h.state.undo = []; h.state.redo = []; h.calls.length = 0;
  const result = (message, value) => h.window.STHostTransport.deliver({ id: message.id, ok: true, result: value });
  h.window.STHostTransport.post = message => {
    h.calls.push(copy(message));
    if (message.payload.captureInput) { contexts.set(message.id, copy(h.window.STInputCapture.recordingContext())); requestIDs.set(message.id, String(++requestID)); return; }
    if (heldCommands.has(message.command)) return;
    if (message.command === 'engine.snapshot') result(message, { audioPlugins: copy(h.state.project.audioPlugins) });
    else if (message.command.startsWith('project.save')) result(message, { saved: true, project: copy(message.payload.project), path: '/fixture/recorded.supertracker' });
    else result(message, {});
  };
  function ack(payload, extra = {}, context = h.window.STInputCapture.recordingContext(), edgeID) {
    return { version: 1, inputOwner: payload.inputOwner, requestID: edgeID, status: 'applied', applied: true, playing: true, recording: true, on: payload.on !== false, pattern: 7, row: 0, sequence: 0, track: payload.track ?? 0, instrument: payload.instrument ?? 0, note: payload.note ?? 60, velocity: payload.velocity ?? 100, applyNote: payload.applyNote !== false, recordingClock: clock(0), recordingContext: copy(context), ...extra };
  }
  return { ...h, project, timers, heldCommands, tick, result,
    captures: (on = true) => h.calls.filter(m => m.command === (on ? 'note.on' : 'note.off') && m.payload.captureInput),
    audible: on => h.calls.filter(m => m.command === (on ? 'note.on' : 'note.off') && m.payload.applyNote !== false),
    step: (row = 0, pattern = 7, track = 0) => h.state.project.patterns.find(p => p.id === pattern).steps[track][row],
    reply(message, extra = {}) { assert(message, 'Expected captured input request'); const response = ack({ ...message.payload, on: message.command === 'note.on' }, extra, contexts.get(message.id), requestIDs.get(message.id)); result(message, { inputAcknowledgement: response }); return response; },
    fireTimers(delay) { for (const [id, timer] of [...timers]) if (timer.delay === delay) { timers.delete(id); timer.fn(); } },
    midiPending(on, token, extra = {}) { const payload = { on, inputOwner: token, recordingPending: true, source: 1, channel: 1, track: 0, instrument: 0, note: 60, velocity: 93, ...extra }; midiEdges.set(token + ':' + on, { payload, context: copy(h.window.STInputCapture.recordingContext()), requestID: String(++requestID) }); h.window.nativeEvent({ type: 'midi.note', payload }); },
    midiAck(on, token, extra = {}) { const edge = midiEdges.get(token + ':' + on); assert(edge); h.window.nativeEvent({ type: 'midi.note', payload: { ...edge.payload, recordingPending: false, recordingOnly: true, inputAcknowledgement: ack(edge.payload, extra, edge.context, edge.requestID) } }); },
    midiCancel(token) { h.window.nativeEvent({ type: 'midi.note', payload: { on: false, inputOwner: token, recordingOnly: true, recordingCancelled: true, reason: 'retriggered', note: 60 } }); },
  };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('native onset chooses the saved pattern and row without provisional history, and observed capability stays sticky', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); assert.equal(h.state.undo.length, 0); assert.equal(h.step(), null); assert(h.window.STInputCapture.hasPendingEdits);
  const onset = h.captures()[0]; h.reply(onset, { pattern: 19, row: 5, sequence: 1, recordingClock: clock(3) }); await tick();
  assert.equal(h.step(), null); assert.equal(h.step(5, 19).note, 60); assert.equal(h.state.undo.length, 1); assert(!h.window.STInputCapture.hasPendingEdits);
  h.state.audio = {}; h.state.status = {}; h.up('z', 'KeyZ'); h.reply(h.captures(false)[0], { pattern: 19, row: 7, sequence: 1, recordingClock: clock(5) }); await tick();
  assert.equal(h.step(5, 19).gate, 8); assert.equal(h.state.undo.length, 1, 'The note and its duration are one recording-pass Undo step'); assert.equal(h.state.undo[0].label, 'record take'); assert.equal(h.audible(true).length, 1); assert.equal(h.audible(false).length, 1);
});

test('release before onset acknowledgement settles in issued order and never retriggers a released key', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); h.up('z', 'KeyZ'); const on = h.captures()[0], off = h.captures(false)[0];
  h.reply(off, { row: 6, recordingClock: clock(2) }); await tick(); assert.equal(h.state.undo.length, 0); assert(!h.highlight(60));
  h.reply(on, { row: 3, recordingClock: clock(1) }); await h.window.STInputCapture.flushEdits(); assert.equal(h.step(3).gate, 4); assert.equal(h.state.undo.length, 1);
  h.reply(on); h.reply(off); h.up('z', 'KeyZ'); await tick(); assert.equal(h.state.undo.length, 1); assert.equal(h.audible(true).length, 1); assert.equal(h.audible(false).length, 1); assert(!h.window.STInputCapture.hasPendingEdits);
});

test('acknowledged playing state controls live versus stopped entry rather than stale UI status', async () => {
  const live = await harness(); live.state.playing = false; live.state.row = 2; live.down('z', 'KeyZ'); assert.equal(live.state.undo.length, 0);
  live.reply(live.captures()[0], { playing: true, row: 6, recordingClock: clock(1) }); await tick(); assert.equal(live.step(6).note, 60); assert.equal(live.step(2), null); assert.equal(live.state.row, 2);
  const stopped = await harness(); stopped.state.row = 2; stopped.down('z', 'KeyZ'); stopped.reply(stopped.captures()[0], { playing: false, row: 6, recordingClock: undefined }); await tick();
  assert.equal(stopped.step(2).note, 60); assert.equal(stopped.step(6), null); assert.equal(stopped.state.row, 3); stopped.up('z', 'KeyZ'); stopped.reply(stopped.captures(false)[0], { playing: false, recordingClock: undefined }); await tick(); assert.equal(stopped.state.undo.length, 1); assert.equal(stopped.step(2).gate, 0);
});

test('rapid stopped entry can revisit an owned row while an intervening manual edit remains protected', async () => {
  for (const shortPattern of [false, true]) {
    const h = await harness(); h.state.playing = false;
    if (shortPattern) { h.project.patterns[0].length = 1; h.project.patterns[0].steps.forEach(steps => { steps.length = 1; }); }
    else h.state.editStep = 0;
    h.down('z', 'KeyZ'); h.down('x', 'KeyX'); h.reply(h.captures()[0], { playing: false, recordingClock: undefined }); h.reply(h.captures()[1], { playing: false, recordingClock: undefined }); await tick();
    assert.equal(h.step().note, 62, shortPattern ? 'A one-row pattern wraps back to its owned row' : 'Edit Step 0 accepts the second queued entry'); assert.equal(h.state.undo.length, 2); assert.doesNotMatch(h.node('toast-region').innerHTML, /No step was inserted/);
  }
  const h = await harness(); h.state.playing = false; h.state.editStep = 0; h.down('z', 'KeyZ'); h.down('x', 'KeyX'); h.reply(h.captures()[0], { playing: false, recordingClock: undefined }); await tick();
  h.ST.mutate('manual note', () => { h.step().note = 65; }, { render: false }); h.reply(h.captures()[1], { playing: false, recordingClock: undefined }); await tick(); assert.equal(h.step().note, 65); assert.equal(h.state.undo.length, 2);
});

test('a rapid live retrigger can replace its own finalized row without merging a stale chord', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); h.up('z', 'KeyZ'); h.down('x', 'KeyX');
  h.reply(h.captures()[0], { row: 2, recordingClock: clock(1) }); h.reply(h.captures(false)[0], { row: 2, recordingClock: clock(1.1) }); h.reply(h.captures()[1], { row: 2, recordingClock: clock(1.2) }); await tick();
  assert.equal(h.step(2).note, 62); assert.equal(h.step(2).chord, undefined); assert.equal(h.step(2).gate, 0); assert.equal(h.state.undo.length, 1); assert.equal(h.state.undo[0].project.patterns[0].steps[0][2], null, 'The whole pass, retrigger included, undoes to the pre-take row');
  const manual = await harness(); manual.down('z', 'KeyZ'); manual.down('x', 'KeyX'); manual.reply(manual.captures()[0], { row: 2, recordingClock: clock(1) }); await tick();
  manual.ST.mutate('manual velocity', () => { manual.step(2).velocity = 33; }, { render: false }); manual.reply(manual.captures()[1], { row: 2, recordingClock: clock(1.2) }); await tick(); assert.equal(manual.step(2).note, 60); assert.equal(manual.step(2).velocity, 33); assert.equal(manual.state.undo.length, 2, 'A later acknowledgement cannot replace an in-place manual edit');
  const gate = await harness(); gate.down('z', 'KeyZ'); gate.reply(gate.captures()[0], { row: 2, recordingClock: clock(1) }); await tick(); gate.up('z', 'KeyZ');
  gate.ST.mutate('manual gate', () => { gate.step(2).gate = 2.25; }, { render: false }); gate.reply(gate.captures(false)[0], { recordingClock: clock(3) }); await tick(); assert.equal(gate.step(2).gate, 2.25); assert.equal(gate.state.undo.length, 2, 'A late release cannot overwrite a manually edited gate');
});

test('shared physical pitches produce capture-only markers with one sounding voice and final-owner duration', async () => {
  for (const reverse of [false, true]) {
    const h = await harness(); h.down('q', 'KeyQ'); h.down(',', 'Comma'); const ons = h.captures(); assert.equal(ons.length, 2); assert.equal(ons[1].payload.applyNote, false); assert.notEqual(ons[0].payload.inputOwner, ons[1].payload.inputOwner); assert.equal(h.audible(true).length, 1);
    h.reply(ons[0], { recordingClock: clock(1) }); h.reply(ons[1], { recordingClock: clock(1.1) }); await tick();
    const keys = reverse ? [[',', 'Comma'], ['q', 'KeyQ']] : [['q', 'KeyQ'], [',', 'Comma']]; h.up(...keys[0]); assert.equal(h.captures(false)[0].payload.applyNote, false); h.reply(h.captures(false)[0], { recordingClock: clock(1.5) }); await tick(); assert.equal(h.step().gate, 0); assert(h.highlight(72));
    h.up(...keys[1]); h.reply(h.captures(false)[1], { recordingClock: clock(2) }); await tick(); assert.equal(h.step().gate, 4); assert.equal(h.audible(false).length, 1); assert(!h.highlight(72)); assert.equal(h.state.undo.length, 1);
  }
});

test('overlapping chords capture only their representative pitch and preserve their complete note set', async () => {
  const h = await harness(); h.state.chord = 'major'; h.down('z', 'KeyZ'); h.down('b', 'KeyB');
  assert.deepEqual(h.audible(true).map(m => m.payload.note).sort((a, b) => a - b), [60, 64, 67, 71, 74]); assert.equal(h.captures().length, 2); assert.equal(h.captures()[1].payload.applyNote, false);
  h.reply(h.captures()[0], { recordingClock: clock(1) }); h.reply(h.captures()[1], { recordingClock: clock(1.1) }); await tick();
  h.up('z', 'KeyZ'); h.reply(h.captures(false)[0], { recordingClock: clock(1.5) }); await tick(); assert.equal(h.step().gate, 0);
  h.up('b', 'KeyB'); h.reply(h.captures(false)[1], { recordingClock: clock(2) }); await tick();
  assert.deepEqual(copy(h.step().chord), [0, 4, 7, 11, 14]); assert.equal(h.step().gate, 4); assert.deepEqual(h.audible(false).map(m => m.payload.note).sort((a, b) => a - b), [60, 64, 67, 71, 74]);
});

test('MIDI pending events own highlights while recording-only responses pair tokens without sounding or highlighting twice', async () => {
  const h = await harness(); h.midiPending(true, 'midi-1'); assert(h.highlight(60)); assert.equal(h.state.undo.length, 0);
  h.midiPending(false, 'midi-1'); assert(!h.highlight(60)); h.midiAck(false, 'midi-1', { recordingClock: clock(3) }); await tick(); assert.equal(h.state.undo.length, 0);
  h.midiAck(true, 'midi-1', { row: 2, recordingClock: clock(1) }); await tick(); assert(!h.highlight(60), 'Delayed recording-only onset cannot relight the released pitch'); assert.equal(h.step(2).gate, 8); assert.equal(h.state.undo.length, 1); assert.equal(h.messages().length, 0);
  h.midiAck(true, 'midi-1'); h.midiAck(false, 'midi-1'); await tick(); assert.equal(h.state.undo.length, 1);
});

test('MIDI retrigger cancellation ends the old owner safely and the physical release finalizes only the new owner', async () => {
  const h = await harness(); h.midiPending(true, 'old'); h.midiAck(true, 'old', { row: 1, recordingClock: clock(1) }); await tick();
  h.midiCancel('old'); h.midiPending(true, 'new'); h.midiAck(true, 'new', { row: 3, recordingClock: clock(2) }); await tick(); assert.equal(h.step(1).gate, 0); assert(h.highlight(60));
  h.midiPending(false, 'new'); h.midiAck(false, 'new', { recordingClock: clock(3) }); await tick(); assert.equal(h.step(1).gate, 0); assert.equal(h.step(3).gate, 4); assert.equal(h.state.undo.length, 1); assert.equal(h.messages().length, 0); assert(!h.window.STInputCapture.hasPendingEdits);
  h.midiCancel('old'); await tick(); assert.equal(h.state.undo.length, 1);
});

test('stale project, arrangement, geometry, edited cells, identity and arm cycles reject delayed onset writes', async () => {
  for (const change of [h => { h.state.project = copy(h.project); }, h => { h.project.sequence.reverse(); }, h => { h.project.patterns[0].length = 7; }, h => { h.project.linesPerBeat = 8; }, h => { h.project.swing = .25; }, h => { h.project.bpm = 80; }, h => { h.project.audioClips = [{ id: 'new', track: 0, path: '/clip.wav', startBeat: 0, durationBeats: 20 }]; }, h => { h.project.tracks[0] = { ...h.project.tracks[0] }; }, h => { h.project.instruments[0] = { ...h.project.instruments[0] }; }, h => { h.project.patterns[0].steps[0][2] = { note: 67, gate: .2 }; }, async h => { await h.ST.setNoteRecording(false); await h.ST.setNoteRecording(true); }]) {
    const h = await harness(); h.down('z', 'KeyZ'); const on = h.captures()[0]; await change(h); const before = copy(h.state.project); h.reply(on, { row: 2, recordingClock: clock(1) }); await tick(); assert.deepEqual(copy(h.state.project), before); assert.equal(h.state.undo.length, 0);
  }
});

test('invalid native onset context/status/identity inserts nothing; a valid onset with unusable release keeps no gate', async () => {
  for (const patch of [{ status: 'unavailable', applied: false }, { status: 'discontinuity' }, { status: 'controlFlush' }, { recording: false }, { inputOwner: 'wrong' }, { requestID: '01' }, { pattern: 999 }, { row: 99 }, { track: 1 }, 'context-lpb']) {
    const h = await harness(); h.down('z', 'KeyZ'); h.reply(h.captures()[0], patch === 'context-lpb' ? { recordingContext: { ...h.window.STInputCapture.recordingContext(), linesPerBeat: 8 } } : patch); await tick(); assert.equal(h.state.undo.length, 0); assert.equal(h.step(), null);
  }
  const h = await harness(); h.down('z', 'KeyZ'); h.reply(h.captures()[0], { recordingClock: clock(1) }); await tick(); h.up('z', 'KeyZ'); h.reply(h.captures(false)[0], { status: 'unavailable', applied: false, recordingClock: undefined }); await tick(); assert.equal(h.step().gate, 0); assert.equal(h.state.undo.length, 1); assert.match(h.node('toast-region').innerHTML, /duration was unavailable/);
});

test('missing acknowledgements settle at the bounded capture timeout without replaying notes or accepting late writes', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); let complete = false; const flushing = h.window.STInputCapture.flushEdits().then(() => { complete = true; }); await tick(); assert(!complete);
  h.fireTimers(1500); await flushing; assert(complete); assert.equal(h.state.undo.length, 0); assert.equal(h.audible(true).length, 1); h.reply(h.captures()[0], { recordingClock: clock(1) }); await tick(); assert.equal(h.state.undo.length, 0);
  h.up('z', 'KeyZ'); h.fireTimers(1500); await h.window.STInputCapture.flushEdits(); assert.equal(h.audible(false).length, 1); assert(!h.window.STInputCapture.hasPendingEdits);
  const saving = await harness(); saving.down('z', 'KeyZ'); let saved;
  saving.ST.perform('save').then(value => { saved = value; }); await tick(); assert.equal(saved, undefined); saving.fireTimers(1500); await tick(); assert.equal(saved, true, 'Save is bounded by the issued-edge timeout, not key release'); assert.equal(saving.calls.find(m => m.command === 'project.save').payload.project.patterns[0].steps[0][0], null);
  const undoing = await harness(), before = undoing.project.masterVolume; undoing.ST.mutate('earlier edit', p => { p.masterVolume = .2; }, { render: false }); undoing.down('z', 'KeyZ'); let undone = false;
  undoing.ST.perform('undo').then(() => { undone = true; }); await tick(); assert(!undone); undoing.fireTimers(1500); await tick(); assert(undone, 'Undo also completes after an unavailable input edge'); assert.equal(undoing.state.project.masterVolume, before); assert.equal(undoing.state.redo.length, 1);
});

test('Save waits for an issued onset but not a held key, and same-session saved adoption preserves its later release', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); const saving = h.ST.perform('save'); await tick(); assert.equal(h.calls.filter(m => m.command === 'project.save').length, 0);
  h.reply(h.captures()[0], { row: 2, recordingClock: clock(1) }); assert.equal(await saving, true); const save = h.calls.find(m => m.command === 'project.save'); assert.equal(save.payload.project.patterns[0].steps[0][2].gate, 0); assert.notEqual(h.state.project, h.project); assert.equal(h.audible(false).length, 0);
  h.up('z', 'KeyZ'); h.reply(h.captures(false)[0], { recordingClock: clock(2) }); await tick(); assert.equal(h.step(2).gate, 4); assert.equal(h.state.dirty, true); assert.equal(h.state.undo.length, 1);
});

test('a new input edge during asynchronous Save prevents stale clone adoption and remains recordable', async () => {
  const h = await harness(); h.heldCommands.add('project.save'); const owner = h.state.project, saving = h.ST.perform('save'); await tick(); const save = h.calls.find(m => m.command === 'project.save'); assert(save);
  h.down('z', 'KeyZ'); h.result(save, { saved: true, project: copy(save.payload.project) }); assert.equal(await saving, false); assert.equal(h.state.project, owner); assert.equal(h.state.dirty, true);
  h.reply(h.captures()[0], { row: 4, recordingClock: clock(1) }); await tick(); assert.equal(h.step(4).note, 60); assert.equal(h.state.undo.length, 1);
});

test('Undo waits for pending onset before restoring its previous step, and cannot discard a newly issued edge', async () => {
  const h = await harness(); h.down('z', 'KeyZ'); const undoing = h.ST.perform('undo'); await tick(); assert.equal(h.state.undo.length, 0); h.reply(h.captures()[0], { row: 2, recordingClock: clock(1) }); await undoing; assert.equal(h.step(2), null); assert.equal(h.state.redo.length, 1);
  const race = await harness(); const original = race.state.project; race.down('z', 'KeyZ'); const pendingUndo = race.ST.perform('undo'); await tick(); race.down('x', 'KeyX'); race.reply(race.captures()[0], { row: 2, recordingClock: clock(1) }); await tick();
  assert.equal(race.state.project, original, 'Undo cannot replace the session when a newer input edge was issued'); race.reply(race.captures()[1], { row: 3, recordingClock: clock(2) }); await pendingUndo; await tick(); assert.equal(race.step(2).note, 60); assert.equal(race.step(3).note, 62); assert.equal(race.state.undo.length, 1, 'Both notes of the pass share one Undo step'); assert.equal(race.state.redo.length, 0);
});

test('Selected step records keyboard and MIDI at editing cursor during playback without advancing',async()=>{
 const h=await harness();h.ST.setRecordPlacement('selected');h.state.row=3;h.down('z','KeyZ');h.reply(h.captures()[0],{pattern:19,row:6,sequence:1});await tick();assert.equal(h.step(3).note,60);assert.equal(h.step(6,19),null);assert.equal(h.state.row,3);h.up('z','KeyZ');h.reply(h.captures(false)[0],{pattern:19,row:7,sequence:1});await tick();assert.equal(h.step(3).gate,0);
 h.midiPending(true,'selected-midi',{note:65});h.midiAck(true,'selected-midi',{pattern:19,row:2,sequence:1,note:65});await tick();assert.equal(h.step(3).note,65);assert.equal(h.state.row,3);assert.equal(h.step(2,19),null);
});
test('switching placement invalidates delayed input rather than redirecting its note',async()=>{
 const h=await harness();h.down('z','KeyZ');const onset=h.captures()[0];h.ST.setRecordPlacement('selected');h.reply(onset,{row:5});await tick();assert.equal(h.step(5),null);assert.equal(h.step(0),null);
});

test('quick stopped note retains original target after immediate lane and row navigation',async()=>{
 const h=await harness();h.state.playing=false;h.state.row=2;h.down('z','KeyZ');h.up('z','KeyZ');h.state.track=1;h.state.instrument=1;h.state.row=5;h.reply(h.captures(false)[0],{playing:false});h.reply(h.captures()[0],{playing:false});await tick();assert.equal(h.step(2,7,0).note,60);assert.equal(h.step(5,7,1),null);assert.equal(h.state.track,1);assert.equal(h.state.row,5);assert.equal(h.state.instrument,1);
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`Input acknowledgement UI: ${tests.length - failures}/${tests.length} regression groups passed.`); if (failures) process.exitCode = 1; })();
