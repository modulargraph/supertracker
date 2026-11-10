// State and bridge regression tests, independent of a browser or audio device.
const fs = require('node:fs');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const path = require('node:path');
const elements = new Map();
function element(id) {
  if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: {}, classList: { toggle() {}, remove() {}, add() {} }, focus() {}, select() {}, addEventListener() {}, setAttribute() {}, scrollIntoView() {}, closest() { return null; }, querySelectorAll() { return []; } });
  return elements.get(id);
}
const doc = { body: element('body'), activeElement: null, querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element(tag) };
const calls = [];
let context;
const win = { addEventListener() {}, STHostTransport: hostStub({ post(message) { calls.push(message); const result = message.command === 'midi.configure' ? {} : message.command === 'plugin.parameters' ? [] : {}; context.window.STHostTransport.deliver({ id: message.id, ok: true, result }); } }) };
// A frozen clock: live-recording gates compare Date.now() against status timestamps.
const now = Date.now();
class FrozenDate extends Date { static now() { return now; } }
context = vm.createContext({ console, window: win, document: doc, requestAnimationFrame: fn => fn(), setTimeout: () => 1, clearTimeout() {}, Date: FrozenDate, Map, Set, Math, JSON, Number, String, Object, Array, Promise, innerWidth: 1280, innerHeight: 800 });
web.load(context, 'app.js');
const ST = win.ST;
const state = ST.state;
state.project = ST.demoProject();
assert.equal(state.project.tracks.length, 8);
assert.equal(state.project.patterns.length, 2);
assert.equal(state.project.patterns[0].steps[0][0].note, 38);
assert.equal(win.__supertracker.noteText(-1), 'OFF');
assert.equal(win.__supertracker.noteText(60), 'C–4');

// Chord note-off retains its originating route across a track change.
state.chord = 'major';
state.track = 2;
state.instrument = 2;
ST.noteOn(60, 'KeyZ');
state.track = 5;
state.instrument = 5;
ST.noteOff('KeyZ');
const off = calls.filter(x => x.command === 'note.off');
assert.equal(off.length, 3);
assert.ok(off.every(x => x.payload.track === 2 && x.payload.instrument === 2));
assert.deepEqual(off.map(x => x.payload.note), [60, 64, 67]);

// Region transpose preserves note-offs and clamps note bounds.
state.track = 0;
state.row = 0;
state.selection = { track: 0, row: 1 };
state.project.patterns[0].steps[0][0] = { note: 127, instrument: 0 };
state.project.patterns[0].steps[0][1] = { note: -1, instrument: 0 };
ST.transpose(12);
assert.equal(state.project.patterns[0].steps[0][0].note, 127);
assert.equal(state.project.patterns[0].steps[0][1].note, -1);

// Undo a newly selected track must restore valid selection indices.
ST.perform('addTrack');
assert.equal(state.track, 8);
ST.perform('undo');
assert.equal(state.project.tracks.length, 8);
assert.ok(state.track < state.project.tracks.length);

// Copy/paste is an independent deep copy and preserves empty cells.
state.track = 0; state.row = 0; state.selection = { track: 0, row: 1 };
ST.copySelection();
state.row = 4; state.selection = null;
ST.pasteSelection();
assert.equal(state.project.patterns[0].steps[0][4].note, 127);
assert.equal(state.project.patterns[0].steps[0][5].note, -1);
state.project.patterns[0].steps[0][4].note = 12;
assert.equal(state.project.patterns[0].steps[0][0].note, 127);

// Unknown automation is rejected instead of silently mapping to volume.
const lanes = state.project.patterns[0].automation.length;
ST.automate(0, 'unimplementedParameter', 'Unknown');
assert.equal(state.project.patterns[0].automation.length, lanes);
ST.automate(0, 'attack', 'Envelope attack');
assert.equal(state.project.patterns[0].automation.at(-1).target, 'attack');

// Native metadata survives asynchronous sample restoration.
state.project.instruments[1].type = 'sample';
state.project.instruments[1].samplePath = '/tmp/sample.wav';
win.nativeEvent({ type: 'sample.loaded', payload: { instrument: 1, sample: { path: '/tmp/sample.wav', peaks: [0, 0.5] } } });
assert.equal(state.samples[1].path, '/tmp/sample.wav');
win.nativeEvent({ type: 'sample.loaded', payload: { instrument: 1, sample: { path: '/tmp/previous.wav', peaks: [1] } } });
assert.equal(state.samples[1].path, '/tmp/sample.wav');
console.log('UI state regression tests passed (routing, editing, undo, automation, native metadata).');

// Native AppKit clipboard commands arrive as DOM clipboard events in WKWebView.
state.view = 'pattern'; state.track = 0; state.row = 0; state.selection = null;
doc.activeElement = { closest(selector) { return selector === '.tracker' ? this : null; } };
const clipboardData = new Map();
const clipboard = { getData: key => clipboardData.get(key) || '', setData: (key, value) => clipboardData.set(key, value) };
let prevented = false;
ST.clipboardEvent({ type: 'copy', clipboardData: clipboard, preventDefault() { prevented = true; } });
assert.equal(prevented, true);
assert.equal(JSON.parse(clipboardData.get('text/plain')).supertracker, 1);
state.row = 8;
ST.clipboardEvent({ type: 'paste', clipboardData: clipboard, preventDefault() {} });
assert.equal(state.project.patterns[0].steps[0][8].note, state.project.patterns[0].steps[0][0].note);
// Sessions have no 16-track limit: steps copied from more than 16 tracks paste through the system clipboard too.
const wide = JSON.stringify({ supertracker: 1, steps: Array.from({ length: 20 }, () => [{ note: 71, instrument: 0, velocity: 90 }]) });
clipboardData.set('application/x-supertracker-steps', wide); clipboardData.set('text/plain', wide);
state.track = 0; state.row = 12; state.selection = null;
ST.clipboardEvent({ type: 'paste', clipboardData: clipboard, preventDefault() {} });
assert.equal(state.project.patterns[0].steps[0][12]?.note, 71, 'a 20-track clipboard pastes');
assert.equal(state.project.patterns[0].steps[7][12]?.note, 71, 'tracks past the session end are dropped, the rest land');
console.log('Native clipboard event regression tests passed.');

// Without the Gate column a live-recorded note keeps no gate (it plays until the next note, OFF or CUT).
state.view = 'pattern'; state.track = 0; state.instrument = 0; state.recording = true; state.recordPlacement='live';
state.playing = true;
state.project.bpm = 120; state.project.linesPerBeat = 4;
assert.equal(ST.gateColumnShown(), false);
state.status = { seconds: 10, pattern: 0, row: 9 }; state.lastStatusAt = now;
ST.noteOn(62, 'LiveKey');
state.status.seconds = 12; state.lastStatusAt = now;
ST.noteOff('LiveKey');
assert.equal(state.project.patterns[0].steps[0][9].note, 62);
assert.equal(state.project.patterns[0].steps[0][9].gate, 0);
assert.equal(ST.stepGate(state.project.patterns[0].steps[0][9]), null);
console.log('Live notes without the Gate column record no gate.');

// Live recording (Gate column shown) holds long notes and uses the last chord key's release for the shared gate.
ST.setGateColumn(true);
state.status = { seconds: 10, pattern: 0, row: 10 }; state.lastStatusAt = now;
ST.noteOn(60, 'LiveKey');
state.status.seconds = 12; state.lastStatusAt = now;
ST.noteOff('LiveKey');
assert.ok(Math.abs(state.project.patterns[0].steps[0][10].gate - 16) < .02);
state.status = { seconds: 20, pattern: 0, row: 12 }; state.lastStatusAt = now;
win.nativeEvent({ type: 'midi.note', payload: { on: true, source: 1, channel: 1, track: 1, instrument: 1, pattern: 0, row: 12, note: 60, velocity: 100 } });
win.nativeEvent({ type: 'midi.note', payload: { on: true, source: 1, channel: 1, track: 1, instrument: 1, pattern: 0, row: 12, note: 64, velocity: 100 } });
state.status.seconds = 21; state.lastStatusAt = now;
win.nativeEvent({ type: 'midi.note', payload: { on: false, source: 1, channel: 1, note: 60 } });
assert.equal(state.project.patterns[0].steps[1][12].gate, 0);
state.status.seconds = 22; state.lastStatusAt = now;
win.nativeEvent({ type: 'midi.note', payload: { on: false, source: 1, channel: 1, note: 64 } });
assert.ok(Math.abs(state.project.patterns[0].steps[1][12].gate - 16) < .02);
assert.deepEqual(Array.from(state.project.patterns[0].steps[1][12].chord), [0, 4]);
console.log('Live note duration and chord release regression tests passed.');

// Song-loop clock resets must not collapse held-note recordings to minimum gate.
state.project = ST.demoProject(); state.project.bpm = 120; state.project.linesPerBeat = 4;
state.project.viewState = { gateColumn: true };
state.track = 0; state.instrument = 0; state.playing = true; state.recording = true;
state.status = { seconds: 32, pattern: 0, row: 60, playing: true }; state.lastStatusAt = now;
const wrappedRecord = ST.recordLiveStep(60,100,0,0,0,60);
wrappedRecord.group.startedAt = now - 2000;
win.nativeEvent({ type: 'status', payload: { seconds: .2, pattern: 0, row: 0, playing: true } });
ST.finishLiveRecord(wrappedRecord);
assert.ok(Math.abs(state.project.patterns[0].steps[0][60].gate - 16) < .05);
console.log('Song-loop live-note duration regression passed.');

// Newly exposed native modulation keeps its actual value when creating a lane.
web.load(context, 'modulation.js');
web.load(context, 'studio.js');
state.project = ST.demoProject(); state.project.instruments[0].filterEnvAmount = 18;
state.view = 'instrument'; state.track = 0; state.instrument = 0; state.row = 0;
ST.automate(0,'filterEnvAmount','Filter envelope amount');
assert.equal(state.project.patterns[0].automation.at(-1).points[0].value,18);
// Tempo-sync automation uses native quarter-note periods without reinterpreting
// saved Hz lanes. Exercise the actual app whitelist and studio target resolver.
const modInstrument = state.project.instruments[0];
assert.equal(modInstrument.lfoSync,false); assert.equal(modInstrument.lfoBeats,1);
modInstrument.lfoRate = 7.25; modInstrument.lfoBeats = 2 / 3;
modInstrument.lfo2Beats = .75; modInstrument.lfo2Sync = true;
ST.automate(0,'lfoRate','LFO 1 rate');
ST.automate(0,'lfoBeats','LFO 1 cycle');
ST.automate(0,'lfo2Beats','LFO 2 cycle');
const periodLane = state.project.patterns[0].automation.find(lane => lane.target === 'lfoBeats');
assert.equal(periodLane.points[0].value,2 / 3);
assert.equal(state.project.patterns[0].automation.at(-1).points[0].value,.75);
assert.equal(win.STStudio.target(0,'lfoBeats').inactive,true);
modInstrument.lfoSync = true;
assert.equal(win.STStudio.target(0,'lfoRate').inactive,true);
assert.equal(win.STStudio.target(0,'lfoBeats').inactive,false);
assert.equal(win.STStudio.target(0,'lfoBeats').min,1 / 64);
assert.equal(state.project.patterns[0].automation.find(lane => lane.target === 'lfoRate').points[0].value,7.25);
assert.equal(periodLane.points[0].value,2 / 3);
state.project.instruments[0].type = 'au';
assert.equal(win.STStudio.target(0,'filterEnvAmount'), undefined);
assert.equal(ST.beatsPerBar({timeSignature:{numerator:7,denominator:8}}),3.5);
const clickBefore = !!state.project.metronome;
ST.perform('metronome');
assert.equal(state.project.metronome,!clickBefore);
ST.perform('undo');
assert.equal(!!state.project.metronome,clickBefore);

// Audio capture performance can use chords without inserting tracker notes.
state.project = ST.demoProject(); state.project.patterns[0].steps[0][0] = null;
state.view = 'pattern'; state.track = 0; state.instrument = 0; state.row = 0;
state.recording = true; state.playing = false; state.chord = 'major';
const onBefore = calls.filter(call => call.command === 'note.on').length;
ST.noteOn(60,'CaptureKey',false,0,0,true);
ST.noteOff('CaptureKey');
assert.equal(calls.filter(call => call.command === 'note.on').length-onBefore,3);
assert.equal(state.project.patterns[0].steps[0][0],null);
console.log('Integrated modulation, metronome, signature and capture-performance regressions passed.');

// The actual app bridge forwards fractional transport to the mounted automation module.
state.view = 'automation'; state.project.activePattern = 0;
const editingAutomation = element('point-value-0');
editingAutomation.value = '0.432'; doc.activeElement = editingAutomation;
win.nativeEvent({ type: 'status', payload: { playing: true, pattern: 0, row: 8, position: 8.5 } });
assert.equal(element('automation-playhead').hidden, false);
assert.ok(Math.abs(parseFloat(element('automation-playhead').style.left) - (96 + 8.5 / 64 * 780) / 9) < 1e-8);
assert.equal(element('automation-position').textContent, 'Playing · Row 08.50 / 64');
assert.equal(doc.activeElement, editingAutomation); assert.equal(editingAutomation.value, '0.432');
win.nativeEvent({ type: 'transport.status', payload: { playing: true, pattern: 1, row: 4, position: 4.25 } });
assert.equal(element('automation-playhead').hidden, true);
assert.equal(state.project.activePattern, 0);
win.nativeEvent({ type: 'status', payload: { playing: false, pattern: 0, row: 8, position: 8.5 } });
assert.equal(element('automation-playhead').hidden, true);
assert(element('automation-position').textContent.startsWith('Stopped'));
console.log('Native status → automation playhead bridge regression passed.');
