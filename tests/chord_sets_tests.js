'use strict';
// Chord sets and the MIDI-keyboard performance map, without a browser.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };

// ---- Part 1: plain VM harness (app.js + note-repeat.js), as in pattern_performance_tests.
function plainHarness() {
  const elements = new Map();
  const element = id => { if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: {}, classList: { toggle() {}, remove() {}, add() {} }, focus() {}, select() {}, addEventListener() {}, setAttribute() {}, removeAttribute() {}, scrollIntoView() {}, closest() { return null; }, querySelectorAll() { return []; } }); return elements.get(id); };
  const doc = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element(tag) };
  const calls = [], timers = [];
  let context;
  let recording;
  const win = { addEventListener() {}, STHostTransport: hostStub({ post(message) { calls.push(message); context.window.STHostTransport.deliver({ id: message.id, ok: true, result: recording(message) ?? {} }); } }) };
  recording = noteRecordingHost(win);
  context = vm.createContext({ console, window: win, document: doc, performance: { now: () => 0 }, requestAnimationFrame: fn => fn(), setTimeout: (fn, ms) => { timers.push({ fn, ms }); return timers.length; }, clearTimeout() {}, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, BigInt, innerWidth: 1280, innerHeight: 800 });
  for (const file of ['app.js', 'note-repeat.js']) web.load(context, file);
  return { ST: win.ST, state: win.ST.state, repeat: win.STNoteRepeat, calls, timers, element };
}

(async () => {
  const { ST, state, repeat, calls, timers, element } = plainHarness();
  state.project = ST.demoProject();
  const sets = ST.chordSets;

  // Library shape: 12–16 original sets of 8–16 chords, each playable through the 8-note MIDI map.
  assert.equal(sets.length, 31, '16 harmony + 15 style sets');
  assert.equal(sets.filter(set => set.group === 'harmony' && set.chords.length === 12).length, 16, 'harmony sets of 12 chords');
  assert.equal(sets.filter(set => set.group === 'style').length, 15, 'style set IDs are retained');
  assert.equal(new Set(sets.map(s => s.id)).size, sets.length);
  for (const set of sets) {
    assert.ok(set.name && set.mood, set.id);
    assert.ok(set.chords.length >= 8 && set.chords.length <= 16, set.id + ' chord count');
    for (const chord of set.chords) assert.ok(chord.notes.length >= 3 && chord.notes.length <= 8 && chord.notes.every(Number.isInteger), set.id);
  }
  // A saved ID that names no chord set selects none.
  same(['major-triads', 'minor-sevenths', 'neosoul', 'maschine-major-1', 'gone', undefined].map(ST.chordSetIdFor),
      ['major-triads', 'minor-sevenths', 'neosoul', '', '', '']);
  ST.acceptProject({ ...ST.demoProject(), performance: { root: 0, scale: 'chromatic', chord: 'single', chordSet: 'minor-sevenths' } });
  assert.equal(state.chordSet, 'minor-sevenths', 'a saved chord set opens selected');
  ST.setPerformance({ chordSet: 'major-triads' }, false);
  same([0, 6, 7, 9].map(i => ST.chordSetChordName(ST.activeChordSet(), i)), ['C', 'B°', 'C/E', 'G/B']);
  same(ST.chordSetPitches(9), [47, 62, 67, 71], 'G/B: B bass under a close G triad');
  state.project = ST.demoProject();
  console.log('PASS chord-set library: ' + sets.length + ' sets, ' + sets.reduce((n, s) => n + s.chords.length, 0) + ' chords');

  // Consecutive computer keys play chord 1..N, record as the existing note + chord step, and the root transposes.
  ST.setPerformance({ root: 0, scale: 'chromatic', chord: 'single', chordSet: 'neosoul' }, false);
  same(state.project.performance, { root: 0, scale: 'chromatic', scaleMode: 'snap', chord: 'single', chordSet: 'neosoul', applyToMIDI: true });
  await ST.setNoteRecording(true);
  const steps = () => ST.pattern().steps[1];
  state.track = 1; state.instrument = 1; state.row = 0; state.editStep = 1; state.keyVelocity = 90; state.octave = 4; state.keyboardPlay = true;
  ST.mutate('reset', () => { for (let r = 0; r < 8; r++) steps()[r] = null; });
  const keyboard = (key, code) => { ST.keyboard({ key, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, target: element('tracker'), preventDefault() {}, defaultPrevented: false }); ST.noteOff(code); };
  keyboard('z', 'KeyZ'); keyboard('s', 'KeyS'); keyboard('x', 'KeyX'); await tick();
  same(steps().slice(0, 3).map(s => [s.note, s.chord, s.velocity]), [[53, [0, 11, 14, 16, 19], 90], [52, [0, 10, 15, 19], 90], [50, [0, 15, 19, 22, 26], 90]], 'Fmaj9, Em7, Dm9 over C4');
  assert.equal(ST.chordSetChordName(ST.activeChordSet(), 0), 'Fmaj9');
  ST.setPerformance({ root: 2 }, false); keyboard('z', 'KeyZ'); await tick();
  same([steps()[3].note, steps()[3].chord], [55, [0, 11, 14, 16, 19]], 'root D transposes to Gmaj9');
  assert.equal(ST.chordSetChordName(ST.activeChordSet(), 0), 'Gmaj9');
  ST.setPerformance({ root: 9 }, false); keyboard('z', 'KeyZ'); await tick();
  assert.equal(steps()[4].note, 50, 'roots above F♯ transpose down to stay centred (Dmaj9)');
  // Keys past the last chord repeat the set an octave higher (12 chords: key index 12 = ",").
  ST.setPerformance({ root: 0 }, false); keyboard(',', 'Comma'); await tick();
  same([steps()[5].note, steps()[5].chord], [65, [0, 11, 14, 16, 19]]);
  await ST.setNoteRecording(false);
  console.log('PASS chord-set keys record chord steps, transpose with the root and wrap by octave');

  // Strip with set name and per-key chord names in the pattern view; flat spelling in flat keys.
  ST.setPerformance({ chordSet: 'jazzminor', root: 0 }, false);
  const html = element('#app').innerHTML;
  assert.match(html, /id="chord-set-strip"/); assert.match(html, /Jazz minor ii–V–i/); assert.match(html, /<kbd>Z<\/kbd>Dm7♭5/); assert.match(html, /<kbd>S<\/kbd>G7♭9/); assert.match(html, /A♭maj7/);
  assert.match(html, /id="chord-set"/); assert.match(html, /id="midi-apply" checked/);
  assert.match(ST.performanceSummary(), /Cm · Set Jazz minor/);
  ST.setPerformance({ chordSet: '' }, false);
  assert.doesNotMatch(element('#app').innerHTML, /chord-set-strip/);
  console.log('PASS chord-set strip, toolbar select and summary');

  // Palette / shortcut cycling: Off → first set → … → last → Off; named entries; choosing a chord leaves set mode.
  ST.perform('chordSetNext'); assert.equal(state.chordSet, sets[0].id);
  ST.perform('chordSetPrevious'); assert.equal(state.chordSet, '');
  ST.perform('chordSetPrevious'); assert.equal(state.chordSet, sets.at(-1).id);
  ST.perform('chordSet:lofi'); assert.equal(state.chordSet, 'lofi');
  ST.perform('chordNext'); assert.equal(state.chordSet, ''); assert.equal(state.chord, 'major');
  ST.perform('chordSet:house-nonexistent'); assert.equal(state.chordSet, '');
  ST.keyboard({ key: '©', code: 'KeyG', metaKey: false, ctrlKey: false, altKey: true, shiftKey: false, repeat: false, target: element('tracker'), preventDefault() {}, defaultPrevented: false });
  assert.equal(state.chordSet, sets[0].id, '⌥G selects the next set');
  ST.perform('chordSetOff'); assert.equal(state.chordSet, '');
  assert.ok(ST.commands.some(c => c[0] === 'chordSet:cinematic' && /Cinematic epic/.test(c[1])), 'palette entry per set');
  for (const id of ['chordSetNext', 'chordSetPrevious', 'chordSetOff', 'midiPerformance']) assert.ok(ST.commands.some(c => c[0] === id), id);
  console.log('PASS chord-set palette commands, ⌥G cycling and chord/set exclusivity');

  // Persistence.
  const saved = ST.demoProject(); saved.performance = { root: 5, scale: 'major', scaleMode: 'snap', chord: 'single', chordSet: 'gospel', applyToMIDI: false };
  ST.acceptProject(saved);
  assert.equal(state.chordSet, 'gospel'); assert.equal(state.root, 5); assert.equal(state.midiApply, false);
  ST.acceptProject(ST.demoProject()); assert.equal(state.chordSet, ''); assert.equal(state.midiApply, true);
  const unknown = ST.demoProject(); unknown.performance = { root: 0, scale: 'minor', chord: 'single', chordSet: 'nope' };
  ST.acceptProject(unknown); assert.equal(state.chordSet, ''); assert.equal(state.scale, 'minor');
  ST.setPerformance({ scale: 'chromatic' }, false);
  same(state.project.performance, { root: 0, scale: 'chromatic', scaleMode: 'snap', chord: 'single', chordSet: '', applyToMIDI: true }, 'the saved settings are complete');
  console.log('PASS chord set and MIDI choice persist with the song; a name the page does not know reads as the default');

  // Note repeat / arpeggiator over a chord-set chord.
  state.project = ST.demoProject(); state.track = 0; state.instrument = 0; state.octave = 4;
  ST.setPerformance({ root: 0, chordSet: 'pop' }, false);
  repeat.set({ enabled: true, mode: 'up', rate: '1/16' }, false);
  await tick(); // the new session's disarm has been sent and answered
  calls.length = 0; timers.length = 0;
  ST.noteOn(60, 'KeyZ'); await tick();
  for (let i = 0; i < 3; i++) { timers.filter(t => t.ms > 50).at(-1).fn(); await tick(); }
  same(calls.filter(c => c.command === 'note.on').map(c => c.payload.note), [48, 64, 67, 72], 'C chord of the pop set arpeggiated upward');
  ST.noteOff('KeyZ'); await tick(); assert.equal(repeat.active, false);
  repeat.set({ enabled: false }, false);
  console.log('PASS note repeat arpeggiates chord-set chords');

  // MIDI map: same voicing functions as the keyboard.
  ST.setPerformance({ chordSet: '', scale: 'chromatic', chord: 'single', scaleMode: 'snap', root: 0 }, false);
  assert.equal(ST.midiPerformanceMap(), null, 'default mode leaves MIDI untouched');
  ST.setPerformance({ chordSet: 'pop' }, false);
  let map = ST.midiPerformanceMap();
  assert.equal(map.length, 128); assert.ok(map.every(row => row.length >= 1 && row.length <= 8 && row.every(n => Number.isInteger(n) && n >= 0 && n <= 127)));
  same(map[48], [48, 64, 67, 72], 'MIDI C3 plays chord 1 voiced around C4');
  same(map[49], [55, 62, 67, 71], 'next MIDI key plays chord 2 (G)');
  same(map[48 + 12], [60, 76, 79, 84], 'chord 1 again an octave up after the 12-chord set');
  state.octave = 2; same(ST.midiPerformanceMap()[48], [48, 64, 67, 72], 'MIDI register is independent of the computer-keyboard octave'); state.octave = 4;
  ST.setPerformance({ chordSet: '', scale: 'major', root: 0, chord: 'single' }, false);
  map = ST.midiPerformanceMap(); same([map[60], map[61], map[66]], [[60], [62], [67]], 'snap to key');
  ST.setPerformance({ scaleMode: 'degrees' }, false);
  map = ST.midiPerformanceMap(); same([map[59], map[60], map[61], map[67]], [[59], [60], [62], [72]], 'scale degrees count from C4');
  ST.setPerformance({ scaleMode: 'snap', chord: 'scaleTriad', scale: 'minor' }, false);
  map = ST.midiPerformanceMap(); same(map[62], [62, 65, 68], 'D in C minor plays the diminished scale triad');
  ST.setPerformance({ midiApply: false }, false);
  assert.equal(ST.midiPerformanceMap(), null); assert.equal(state.project.performance.applyToMIDI, false);
  ST.setPerformance({ midiApply: true }, false); assert.equal(state.project.performance.applyToMIDI, true);
  console.log('PASS MIDI performance map for chord sets, snap, degrees and scale chords');

  // Debounced, de-duplicated sends to the native host once it is ready.
  state.engineReady = true; calls.length = 0; timers.length = 0;
  const flushMIDI = async () => { for (const t of timers.splice(0).filter(t => t.ms === 40)) t.fn(); await tick(); };
  const midiCalls = () => calls.filter(c => c.command === 'midi.performance');
  ST.setPerformance({ chord: 'major' }, false); ST.setPerformance({ chord: 'minor' }, false); ST.setPerformance({ chord: 'scaleTriad' }, false);
  await flushMIDI(); assert.equal(midiCalls().length, 1, 'one send per burst');
  same(midiCalls()[0].payload.map[62], [62, 65, 68]);
  ST.setPerformance({ root: 0 }, false); await flushMIDI(); assert.equal(midiCalls().length, 1, 'unchanged map is not resent');
  ST.setPerformance({ midiApply: false }, false); await flushMIDI(); assert.equal(midiCalls().length, 2); assert.equal(midiCalls()[1].payload.map, null, 'turning it off clears the native map');
  ST.setPerformance({ midiApply: true, chordSet: 'trap' }, false); await flushMIDI(); assert.equal(midiCalls().length, 3);
  ST.acceptProject(ST.demoProject()); await flushMIDI(); assert.equal(midiCalls().length, 4); assert.equal(midiCalls()[3].payload.map, null, 'loading a project without a chord set resets the MIDI map');
  ST.perform('midiPerformance'); assert.equal(state.midiApply, false); ST.perform('midiPerformance'); assert.equal(state.midiApply, true);
  console.log('PASS MIDI map sends are debounced, de-duplicated and follow project loads');

  await stoppedMIDIChordRecording();
})().catch(error => { console.error(error); process.exit(1); });

// ---- Part 2: a MIDI key expanded natively into a chord records one chord step while stopped.
async function stoppedMIDIChordRecording() {
  const fixture = fs.readFileSync(path.join(__dirname, 'held_notes_tests.js'), 'utf8');
  const exportLine = 'return { ST, state, window, node, calls,';
  const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')).replace(exportLine, exportLine.replace('return {', 'return { context,')) + '\nreturn harness;')(require, __dirname);
  const h = await makeHarness(), copy = value => JSON.parse(JSON.stringify(value)), timers = new Map();
  let timerID = 1000, requestID = 0;
  h.context.setTimeout = (fn, delay = 0) => { timers.set(++timerID, { fn, delay }); return timerID; };
  h.context.clearTimeout = id => timers.delete(id);
  const project = h.state.project;
  project.patterns.forEach((pat, index) => { pat.id = index ? 19 : 7; pat.length = 8; pat.steps = project.tracks.map(() => Array(8).fill(null)); pat.automation = []; });
  project.activePattern = 7; project.sequence = [7, 19];
  Object.assign(h.state, { playing: false, row: 2, track: 0, instrument: 0, editStep: 1, status: { playing: false, pattern: 7, row: 0 } });
  h.window.STHostTransport.post = message => { h.calls.push(copy(message)); h.window.STHostTransport.deliver({ id: message.id, ok: true, result: message.command === 'transport.recordArm' ? { armed: message.payload.armed } : {} }); };
  await h.ST.setNoteRecording(true);
  h.ST.setPerformance({ chordSet: 'pop', root: 0 }, false);
  for (const [id, timer] of [...timers]) if (timer.delay === 40) { timers.delete(id); timer.fn(); }
  await tick();
  assert.ok(h.calls.some(c => c.command === 'midi.performance' && Array.isArray(c.payload.map)), 'map sent');
  const edges = new Map();
  const pending = (on, token, note) => { const payload = { on, inputOwner: token, recordingPending: true, source: 1, channel: 1, track: 0, instrument: 0, note, velocity: 93 }; edges.set(token + ':' + on, { payload, context: copy(h.window.STInputCapture.recordingContext()), requestID: String(++requestID) }); h.window.nativeEvent({ type: 'midi.note', payload }); };
  const ack = (on, token) => { const edge = edges.get(token + ':' + on), p = edge.payload; h.window.nativeEvent({ type: 'midi.note', payload: { ...p, recordingPending: false, recordingOnly: true, inputAcknowledgement: { inputOwner: token, requestID: edge.requestID, status: 'applied', applied: true, playing: false, recording: true, on, pattern: 7, row: 0, sequence: 0, track: 0, instrument: 0, note: p.note, velocity: p.velocity, applyNote: true, recordingContext: edge.context } } }); };
  const chord = [48, 64, 67, 72];
  chord.forEach((note, i) => pending(true, 'midi:' + i, note));
  chord.forEach((_, i) => ack(true, 'midi:' + i));
  await tick();
  const step = h.state.project.patterns[0].steps[0][2];
  same([step.note, step.chord, step.velocity], [48, [0, 16, 19, 24], 93], 'one chord step');
  assert.equal(h.state.project.patterns[0].steps[0][3], null, 'no arpeggiated spill into later rows');
  assert.equal(h.state.row, 3);
  // A later key (outside the chord window) takes the next row as usual.
  h.advance(500); pending(true, 'midi:next', 55); ack(true, 'midi:next'); await tick();
  assert.equal(h.state.project.patterns[0].steps[0][3].note, 55);
  console.log('PASS stopped MIDI recording keeps a mapped chord on one step');
}
