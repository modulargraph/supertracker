'use strict';
// Count-in recording (ST.recordWithCountIn, palette "Record with count-in") against the real app with a mocked bridge:
// the engine does the clicks and the sample-accurate start; the app arms recording, sends Play with countIn,
// keeps notes played during the count-in out of the pattern and follows the native status.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
function harness() {
  const nodes = new Map(), calls = [];
  function element(id) {
    if (!nodes.has(id)) nodes.set(id, {
      id, dataset: {}, innerHTML: '', textContent: '', value: '', style: {}, classList: { toggle() {}, add() {}, remove() {} },
      addEventListener() {}, setAttribute() {}, scrollIntoView() {}, select() {}, querySelectorAll() { return []; },
      focus() { document.activeElement = this; }, closest(selector) { return selector === '.tracker' && this.id === 'tracker' ? this : null; }
    });
    return nodes.get(id);
  }
  const document = { activeElement: null, querySelector: selector => element(selector.startsWith('#') ? selector.slice(1) : selector), querySelectorAll: () => [], getElementById: element, createElement: element, addEventListener() {} };
  document.body = element('body');
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) {
    calls.push(copy(message)); const result = recording(message) ?? {}; Promise.resolve().then(() => window.STHostTransport.deliver({ id: message.id, ok: true, result }));
  } }) };
  const recording = noteRecordingHost(window);
  const context = vm.createContext({ window, document, console, setTimeout: () => 1, clearTimeout() {}, innerWidth: 1280, innerHeight: 800 });
  web.load(context, 'app.js');
  const ST = window.ST, state = ST.state;
  state.project = ST.demoProject();
  Object.assign(state, { view: 'pattern', track: 0, instrument: 0, row: 10, column: 0, playing: false, recording: false, keyboardPlay: true, octave: 4, editStep: 2, chord: 'single', scale: 'chromatic' });
  element('tracker').focus();
  const key = (key, code) => { const event = { key, code, target: element('tracker'), metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, repeat: false, preventDefault() { this.defaultPrevented = true; } }; ST.keyboard(event); return event; };
  const status = payload => window.nativeEvent({ type: 'status', payload });
  const sent = command => calls.filter(call => call.command === command);
  return { ST, state, window, calls, key, status, sent };
}
(async () => {
  {
    const h = harness();
    assert.ok(h.ST.commands.some(c => c[0] === 'recordCountIn' && /count-in/i.test(c[1])), 'palette offers Record with count-in');
    const started = h.ST.perform('recordCountIn'); await tick(); assert.equal(await started, true);
    assert.equal(h.state.recording, true, 'recording armed before the count-in');
    const play = h.sent('transport.play').at(-1); assert.ok(play, 'Play requested'); assert.equal(play.payload.countIn, 1); assert.equal(play.payload.mode, 'pattern');
    assert.ok(h.state.countIn && !h.state.playing, 'counting in: not playing yet');
    h.status({ playing: false, countIn: 4 }); assert.ok(h.state.countIn.seen); assert.equal(h.state.countIn.remaining, 4);
    // A note during the count-in sounds but is not step-entered at the cursor.
    const undo = h.state.undo.length, before = copy(h.ST.pattern().steps[0][10]);
    h.key('z', 'KeyZ'); assert.ok(h.sent('note.on').length, 'note sounds'); assert.equal(h.state.undo.length, undo); assert.deepEqual(copy(h.ST.pattern().steps[0][10]), before); assert.equal(h.state.row, 10);
    h.ST.noteOff('KeyZ');
    h.status({ playing: true, countIn: 0, row: 0, pattern: h.ST.pattern().id }); assert.equal(h.state.countIn, null, 'downbeat ends the count-in'); assert.equal(h.state.playing, true);
    console.log('PASS count-in: palette command arms recording, Play carries countIn, count-in notes are not step-entered, status ends it');
  }
  {
    const h = harness(); h.ST.recordWithCountIn(2); await tick();
    assert.equal(h.sent('transport.play').at(-1).payload.countIn, 2);
    h.status({ playing: false, countIn: 7 }); h.calls.length = 0;
    await h.ST.perform('play'); await tick();
    assert.ok(h.sent('transport.stop').length, 'Play during a count-in cancels it'); assert.equal(h.state.countIn, null); assert.equal(h.sent('transport.play').length, 0);
    // A count-in cancelled natively (stop elsewhere) clears once seen.
    h.ST.recordWithCountIn(); await tick(); h.status({ playing: false, countIn: 2 }); h.status({ playing: false, countIn: 0 }); assert.equal(h.state.countIn, null);
    // While playing it only arms recording.
    h.state.recording = false; h.state.playing = true; h.calls.length = 0; await h.ST.recordWithCountIn(); assert.equal(h.state.recording, true); assert.equal(h.sent('transport.play').length, 0);
    // Ordinary Play never sends countIn.
    h.state.playing = false; h.status({ playing: false }); h.calls.length = 0; await h.ST.perform('play'); await tick();
    assert.equal('countIn' in (h.sent('transport.play').at(-1)?.payload || {}), false, 'plain Play unchanged');
    console.log('PASS count-in: Play cancels, native cancellation clears, playing arms only, plain Play unchanged');
  }
})().catch(error => { console.error(error); process.exit(1); });
