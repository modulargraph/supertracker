// Keyboard performance (scale/chord/note repeat) and pattern tool regressions, without a browser.
const fs = require('node:fs');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const elements = new Map();
function element(id) {
  if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: {}, classList: { toggle() {}, remove() {}, add() {} }, focus() {}, select() {}, addEventListener() {}, setAttribute() {}, removeAttribute() {}, scrollIntoView() {}, closest() { return null; }, querySelectorAll() { return []; } });
  return elements.get(id);
}
const doc = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element(tag) };
const calls = [], timers = [];
let context;
const win = { addEventListener() {}, STHostTransport: hostStub({ post(message) { calls.push(message); context.window.STHostTransport.deliver({ id: message.id, ok: true, result: message.command === 'plugin.parameters' ? [] : recording(message) ?? {} }); } }) };
const recording = noteRecordingHost(win);
context = vm.createContext({ console, window: win, document: doc, performance: { now: () => 0 }, requestAnimationFrame: fn => fn(), setTimeout: (fn, ms) => { timers.push({ fn, ms }); return timers.length; }, clearTimeout() {}, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, BigInt, innerWidth: 1280, innerHeight: 800 });
for (const file of ['app.js', 'note-repeat.js', 'pattern-tools.js']) web.load(context, file);
const ST = win.ST, state = ST.state, tools = win.STPatternTools, repeat = win.STNoteRepeat;
const same = (actual, expected) => assert.equal(JSON.stringify(actual), JSON.stringify(expected));
const flush = () => new Promise(resolve => setImmediate(resolve));
state.project = ST.demoProject();
const steps = track => ST.pattern().steps[track];

(async () => {
  // Scale chords follow the key; degrees mode maps consecutive keys to scale notes.
  ST.setPerformance({ root: 0, scale: 'minor', scaleMode: 'degrees', chord: 'scaleTriad' }, false);
  same(state.project.performance, { root: 0, scale: 'minor', scaleMode: 'degrees', chord: 'scaleTriad', chordSet: '', applyToMIDI: true });
  await ST.setNoteRecording(true);
  state.track = 1; state.instrument = 1; state.row = 0; state.editStep = 1; state.keyVelocity = 80;
  state.row = 0;
  ST.mutate('reset', () => { for (let r = 0; r < 8; r++) steps(1)[r] = null; });
  const keyboard = (key, code) => { const event = { key, code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, target: element('tracker'), preventDefault() {}, defaultPrevented: false }; ST.keyboard(event); ST.noteOff(code); };
  state.keyboardPlay = true;
  keyboard('z', 'KeyZ'); keyboard('x', 'KeyX'); keyboard('b', 'KeyB'); await flush();
  same([0, 1, 2].map(r => [steps(1)[r].note, steps(1)[r].chord, steps(1)[r].velocity]), [[60, [0, 3, 7], 80], [63, [0, 4, 7], 80], [72, [0, 3, 7], 80]]);
  ST.setPerformance({ scaleMode: 'snap', chord: 'min7' }, false);
  keyboard('s', 'KeyS'); await flush();
  same([steps(1)[3].note, steps(1)[3].chord], [62, [0, 3, 7, 10]]);
  console.log('PASS scale degrees, snap-to-key, scale chords and keyboard velocity reach recorded steps');
  await ST.setNoteRecording(false);

  // Old projects without performance settings open in chromatic single-note mode.
  ST.acceptProject(ST.demoProject());
  assert.equal(state.scale, 'chromatic'); assert.equal(state.chord, 'single'); assert.equal(state.scaleMode, 'snap');
  const saved = ST.demoProject(); saved.performance = { root: 9, scale: 'dorian', scaleMode: 'degrees', chord: 'scaleSeventh' };
  ST.acceptProject(saved);
  assert.equal(state.root, 9); assert.equal(state.scale, 'dorian'); assert.equal(state.chord, 'scaleSeventh');
  ST.acceptProject(ST.demoProject());
  console.log('PASS key/scale/chord persist with the song and default for older projects');

  // Note repeat retriggers held notes on its own clock and releases them on key up.
  ST.setPerformance({ scale: 'chromatic', chord: 'major' }, false);
  repeat.set({ enabled: true, mode: 'up', rate: '1/16' }, false);
  await flush(); // the new session's disarm has been sent and answered
  calls.length = 0; timers.length = 0;
  state.track = 0; state.instrument = 0;
  ST.noteOn(60, 'KeyZ');
  await flush();
  same(calls.filter(c => c.command === 'note.on').map(c => c.payload.note), [60]);
  timers.filter(t => t.ms > 50).at(-1).fn(); await flush();
  timers.filter(t => t.ms > 50).at(-1).fn(); await flush();
  same(calls.filter(c => c.command === 'note.on').map(c => c.payload.note), [60, 64, 67]);
  ST.noteOff('KeyZ'); await flush();
  const offs = calls.filter(c => c.command === 'note.off').map(c => c.payload.note);
  assert.equal(offs.at(-1), 67);
  assert.equal(repeat.active, false);
  repeat.set({ enabled: false }, false);
  console.log('PASS note repeat arpeggiates held chord notes and stops on release');

  // Repeats lock to the tempo grid (song position while playing), and the held pad's velocity can change.
  ST.setPerformance({ scale: 'chromatic', chord: 'single' }, false);
  repeat.set({ enabled: true, mode: 'repeat', rate: '1/16' }, false);
  const bpm = Number(state.project.bpm) || 120, sixteenth = 15000 / bpm;
  Object.assign(state, { playing: true, lastStatusAt: Date.now() }); state.status = { ...(state.status || {}), beat: .1 };
  calls.length = 0; timers.length = 0;
  ST.noteOn(62, 'KeyX');
  await flush();
  const toLine = (.25 - .1) * 60000 / bpm, next = timers.filter(t => t.ms > 0 && Math.abs(t.ms - sixteenth / 2) > 1).at(-1);
  assert.ok(Math.abs(next.ms - toLine) < 3, `the first repeat waits for the next 1/16 line (${toLine} ms), not a 1/16 after the hit: ${next.ms}`);
  repeat.setVelocity('KeyX', 40); next.fn(); await flush();
  assert.equal(calls.filter(c => c.command === 'note.on').at(-1).payload.velocity, 40, 'held pressure sets the repeat velocity');
  ST.noteOff('KeyX'); await flush();
  Object.assign(state, { playing: false }); repeat.set({ enabled: false }, false); ST.setPerformance({ chord: 'major' }, false);
  console.log('PASS note repeat locks to the tempo grid and follows the held velocity');

  // Mute/solo are undoable track state.
  tools.toggleTrack('mute', 2); assert.equal(state.project.tracks[2].mute, true);
  tools.toggleTrack('solo', 3); assert.equal(state.project.tracks[3].solo, true);
  tools.clearMuteSolo(); assert.ok(!state.project.tracks.some(t => t.mute || t.solo));
  console.log('PASS mute, solo and clear');

  // Interpolate velocity and FX ramps.
  state.selection = { track: 5, row: 0 }; state.track = 5; state.row = 14; state.column = 2;
  tools.interpolate();
  same(steps(5).slice(0, 15).filter(Boolean).map(s => s.velocity), [76, 72, 69, 65, 61, 57, 54, 50]);
  ST.mutate('fx', () => { steps(0)[0].fx1 = { type: 'cutoff', value: 200 }; steps(0)[8] = { note: -3, instrument: 0, velocity: 100, gate: .8, fx1: { type: 'cutoff', value: 1000 } }; });
  state.selection = { track: 0, row: 0 }; state.track = 0; state.row = 8; state.column = 3;
  tools.interpolate();
  same(steps(0).slice(0, 9).map(s => s.fx1.value), [200, 300, 400, 500, 600, 700, 800, 900, 1000]);
  console.log('PASS interpolate velocity and FX ramps');

  // Reverse, nudge, stretch, double/halve and quick effects.
  const before = JSON.stringify(steps(0).slice(0, 9));
  tools.reverse(); assert.equal(JSON.stringify(steps(0).slice(0, 9).reverse()), before);
  await ST.perform('undo'); assert.equal(JSON.stringify(steps(0).slice(0, 9)), before);
  // Time tools on a range without steps change nothing and add no Undo step.
  {
    const undo = state.undo.length, column = JSON.stringify(steps(0));
    for (let row = 20; row <= 23; row++) steps(0)[row] = null;
    const emptied = JSON.stringify(steps(0));
    state.track = 0; state.selection = { track: 0, row: 20 }; state.row = 23;
    tools.reverse(); tools.nudge(1); tools.stretch(2);
    assert.equal(state.undo.length, undo, 'no Undo step for an empty range'); assert.equal(JSON.stringify(steps(0)), emptied);
    ST.pattern().steps[0] = JSON.parse(column);
  }
  state.selection = null; state.track = 3;
  const kicks = () => steps(3).map((s, i) => s ? i : -1).filter(i => i >= 0);
  same(kicks().slice(0, 2), [0, 8]);
  tools.nudge(1); same(kicks().slice(0, 2), [1, 9]);
  tools.nudge(-1); tools.stretch(2); same(kicks().slice(0, 3), [0, 16, 32]);
  tools.stretch(.5); same(kicks().slice(0, 3), [0, 8, 16]);
  tools.resizePattern(2); assert.equal(ST.pattern().length, 128); assert.ok(ST.pattern().steps.every(s => s.length === 128)); assert.equal(steps(3)[64].note, 36);
  tools.resizePattern(.5); assert.equal(ST.pattern().length, 64); assert.ok(ST.pattern().automation.every(l => l.points.every(p => p.row < 64)));
  state.track = 4; tools.applyEffect('ratchet', 3);
  assert.ok(steps(4).filter(Boolean).every(s => s.fx1?.type === 'ratchet' && s.fx1.value === 3));
  tools.clearEffects(); assert.ok(steps(4).filter(Boolean).every(s => !s.fx1));
  console.log('PASS reverse, nudge, stretch/compress, double/halve and quick step effects');

  // The pattern list reorders (one undo step); the song still refers to the same patterns by id.
  {
    const base = JSON.parse(JSON.stringify(state.project.patterns[0]));
    ST.mutate('test patterns', p => { p.patterns = [0, 1, 2].map(id => ({ ...JSON.parse(JSON.stringify(base)), id, name: 'P' + id })); p.sequence = [0, 1, 2]; p.activePattern = 0; });
    const order = () => state.project.patterns.map(x => x.name).join(',');
    assert.equal(ST.movePattern(2, 0), true); assert.equal(order(), 'P2,P0,P1');
    same(state.project.sequence, [0, 1, 2]);
    assert.equal(ST.movePattern(2, 0), false, 'already first');
    await ST.perform('undo'); assert.equal(order(), 'P0,P1,P2');
    ST.movePattern(0, 99); assert.equal(order(), 'P1,P2,P0', 'moving past the end stops at the end');
    state.project.activePattern = 2; ST.selectPattern(2);
    const renamed = state.project.patterns.find(x => x.id === 2);
    assert.equal(ST.pattern().name, 'P2', 'the active pattern is found by id, not list position');
    assert.ok(renamed);
  }
  console.log('PASS the pattern list reorders as one undo step and keeps the song references');

  // Every new command is in the palette and dispatches.
  for (const command of ['follow', 'muteTrack', 'interpolate', 'humanize', 'reverseSteps', 'nudgeDown', 'stretch', 'doublePattern', 'halvePattern', 'chordNext', 'scaleNext', 'rootUp', 'scaleMode', 'noteRepeat', 'noteRepeatMode', 'editStepUp', 'octaveUp', 'quickFX:probability:0.5']) ST.perform(command);
  assert.equal(state.chord, 'minor'); assert.equal(state.scale, 'major'); assert.equal(state.root, 1);
  console.log('PASS palette commands dispatch');
})().catch(error => { console.error(error); process.exit(1); });
