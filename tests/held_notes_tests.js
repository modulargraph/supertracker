'use strict';
// Boot the real app and exercise its registered keyboard/blur handlers. No native audio runs.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
async function harness() {
  const nodes = new Map(), calls = [], timers = new Map(), documentListeners = new Map(), windowListeners = new Map();
  let timerID = 0, now = 1000000;
  function node(id, dataset = {}, tagName = 'BUTTON') {
    if (nodes.has(id)) return nodes.get(id);
    const classes = new Set();
    const element = { id, dataset, tagName, type: '', value: '', style: {}, innerHTML: '',
      classList: { add: value => classes.add(value), remove: value => classes.delete(value), contains: value => classes.has(value), toggle(value, enabled) { if (enabled ?? !classes.has(value)) classes.add(value); else classes.delete(value); } },
      setAttribute() {}, addEventListener() {}, querySelectorAll() { return []; }, select() {}, scrollIntoView() {},
      focus() { document.activeElement = this; }, click() { return this.onclick?.({ target: this }); },
      closest(selector) {
        if (selector.includes('input:not')) return ['INPUT', 'SELECT', 'TEXTAREA'].includes(this.tagName) && this.type !== 'range' ? this : null;
        if (['button,select', 'button,input,select', 'button'].includes(selector)) return this.tagName === 'BUTTON' ? this : null;
        if (selector === '.tracker') return this.id === 'tracker' ? this : null;
        return null;
      }
    };
    nodes.set(id, element); return element;
  }
  const listen = (map, type, handler) => { if (!map.has(type)) map.set(type, []); map.get(type).push(handler); };
  const document = {
    activeElement: null, body: null,
    querySelector(selector) { if (selector === '.context-menu') return null; return selector.startsWith('#') ? node(selector.slice(1)) : node(selector); },
    querySelectorAll(selector) {
      const pitch = /^\[data-piano="(\d+)"\]$/.exec(selector);
      if (pitch) return [node('piano-' + pitch[1], { piano: pitch[1] })];
      if (selector === '[data-piano]') return Array.from({ length: 128 }, (_, n) => node('piano-' + n, { piano: String(n) }));
      return [];
    },
    getElementById: id => nodes.get(id) || null, createElement: node,
    addEventListener(type, handler) { listen(documentListeners, type, handler); }
  };
  document.body = node('body', {}, 'BODY'); node('tracker', {}, 'DIV');
  // Input ownership cases need independent tracks/instruments and two patterns;
  // provide that fixture explicitly instead of depending on the fresh-session default.
  const window = { addEventListener(type, handler) { listen(windowListeners, type, handler); }, STHostTransport: hostStub({ post(message) { calls.push(copy(message)); const result = message.command === 'ready' ? { project: copy(window.ST.demoProject()) } : recording(message) ?? {}; window.STHostTransport.deliver({ id: message.id, ok: true, result }); } }) };
  // The host confirms arming and acknowledges recorded keys; its engine clock runs at 120 BPM on the test's clock.
  const recording = noteRecordingHost(window, { clock: () => (now - 1000000) / 500 });
  class Clock extends Date { constructor(...args) { super(...(args.length ? args : [now])); } static now() { return now; } }
  const context = vm.createContext({ window, document, console, Date: Clock, Map, Set, Math, JSON, Number, String, Object, Array, Promise, innerWidth: 1280, innerHeight: 800,
    setTimeout(fn) { timers.set(++timerID, fn); return timerID; }, clearTimeout(id) { timers.delete(id); }
  });
  web.load(context, 'app.js', { path: process.env.SUPERTRACKER_HELD_NOTES_SOURCE }); for (const file of ['song-model.js', 'song-sequence.js']) web.load(context, file); await window.ST.boot();
  const ST = window.ST, state = ST.state; state.keyboardPlay = true; calls.length = 0;
  function dispatch(type, key, code, extra = {}) {
    const event = { target: node('tracker'), key, code, preventDefault() { this.defaultPrevented = true; }, ...extra };
    const handlers = documentListeners.get(type); assert(handlers?.length, `The real app must register ${type}`);
    for (const handler of handlers) handler(event); return event;
  }
  return { ST, state, window, node, calls,
    down: (key, code, extra) => dispatch('keydown', key, code, extra), up: (key, code, extra) => dispatch('keyup', key, code, extra),
    blur() { const handlers = windowListeners.get('blur'); assert(handlers?.length); for (const handler of handlers) handler(); },
    advance(ms) { now += ms; }, highlight: pitch => node('piano-' + pitch).classList.contains('held'),
    notes: command => calls.filter(call => call.command === 'note.' + command).map(call => call.payload),
    messages: () => calls.filter(call => call.command.startsWith('note.')).map(call => ({ command: call.command, ...call.payload })),
    audible: command => calls.filter(call => call.command === 'note.' + command && call.payload.applyNote !== false).map(call => call.payload),
    async settle() { for (let i = 0; i < 4; i++) await Promise.resolve(); },
    // Long enough for an acknowledgement to become an edit.
    async acknowledged() { for (let i = 0; i < 80; i++) await Promise.resolve(); }
  };
}
const tests = [], test = (name, run) => tests.push({ name, run });
const pitches = notes => notes.map(note => note.note).sort((a, b) => a - b);

test('Q and comma share one voice until the final physical key releases, in either release order', async () => {
  for (const reverse of [false, true]) {
    const h = await harness(); assert(h.down('q', 'KeyQ').defaultPrevented); assert(h.down(',', 'Comma').defaultPrevented);
    h.down('q', 'KeyQ', { repeat: true }); h.down(',', 'Comma', { repeat: true });
    assert.deepEqual(h.notes('on'), [{ track: 0, instrument: 0, note: 72, velocity: 100 }]); assert(h.highlight(72));
    const keys = reverse ? [[',', 'Comma'], ['q', 'KeyQ']] : [['q', 'KeyQ'], [',', 'Comma']];
    h.up(...keys[0]); assert.equal(h.notes('off').length, 0, 'A shared pitch must not stop on its first keyup'); assert(h.highlight(72));
    h.up(...keys[1]); assert.deepEqual(h.notes('off'), [{ track: 0, instrument: 0, note: 72 }]); assert(!h.highlight(72));
    h.up(...keys[0]); h.up(...keys[1]); assert.equal(h.notes('off').length, 1, 'Repeated keyup must be harmless');
  }
});

test('overlapping major chords release only unowned tones and keep the shared piano highlight', async () => {
  for (const reverse of [false, true]) {
    const h = await harness(); h.state.chord = 'major'; h.down('z', 'KeyZ'); h.down('b', 'KeyB');
    assert.deepEqual(pitches(h.notes('on')), [60, 64, 67, 71, 74]);
    h.up(reverse ? 'b' : 'z', reverse ? 'KeyB' : 'KeyZ'); assert.deepEqual(pitches(h.notes('off')), reverse ? [71, 74] : [60, 64]); assert(h.highlight(67));
    h.up(reverse ? 'z' : 'b', reverse ? 'KeyZ' : 'KeyB'); assert.deepEqual(pitches(h.notes('off')), [60, 64, 67, 71, 74]);
    for (const pitch of [60, 64, 67, 71, 74]) assert(!h.highlight(pitch));
  }
});

test('scale-quantized neighboring keys coalesce after quantization', async () => {
  const h = await harness(); h.state.scale = 'major'; h.down('s', 'KeyS'); h.down('x', 'KeyX');
  assert.deepEqual(pitches(h.notes('on')), [62]); h.up('s', 'KeyS'); assert.equal(h.notes('off').length, 0); h.up('x', 'KeyX'); assert.deepEqual(pitches(h.notes('off')), [62]);
});

test('same pitches on separate tracks stay independent and keyup uses the original track and instrument', async () => {
  const h = await harness(); h.down('q', 'KeyQ'); h.state.track = 1; h.state.instrument = 1; h.down(',', 'Comma');
  assert.deepEqual(h.notes('on').map(n => [n.track, n.instrument, n.note]), [[0, 0, 72], [1, 1, 72]]);
  h.state.track = 2; h.state.instrument = 2; h.up('q', 'KeyQ'); assert.deepEqual(h.notes('off'), [{ track: 0, instrument: 0, note: 72 }]); assert(h.highlight(72), 'Another track still owns the lit pitch');
  h.up(',', 'Comma'); assert.deepEqual(h.notes('off').at(-1), { track: 1, instrument: 1, note: 72 }); assert(!h.highlight(72));
});

test('playing a new instrument retires incompatible held owners only on that track before starting its new voice', async () => {
  const h = await harness(); h.down('q', 'KeyQ'); h.down(',', 'Comma');
  h.ST.noteOn(72, 'other-track', false, 2, 1); const before = h.messages().length;
  h.state.instrument = 1; h.down('z', 'KeyZ');
  assert.deepEqual(h.messages().slice(before), [{ command: 'note.off', track: 0, instrument: 0, note: 72 }, { command: 'note.on', track: 0, instrument: 1, note: 60, velocity: 100 }]);
  assert(h.highlight(72)); h.up('q', 'KeyQ'); h.up(',', 'Comma'); assert.equal(h.notes('off').length, 1, 'Retired physical keys cannot release the replacement voice');
  h.up('z', 'KeyZ'); assert.deepEqual(h.notes('off').at(-1), { track: 0, instrument: 1, note: 60 }); assert(h.highlight(72));
  h.ST.noteOff('other-track'); assert.deepEqual(h.notes('off').at(-1), { track: 1, instrument: 2, note: 72 }); assert(!h.highlight(72));
});

test('reusing a key releases its old pitch before replacement and leaves no stranded voice', async () => {
  const h = await harness(); h.ST.noteOn(60, 'pointer', false); h.ST.noteOn(64, 'pointer', false); h.ST.noteOff('pointer');
  assert.deepEqual(h.messages().map(n => [n.command, n.note]), [['note.on', 60], ['note.off', 60], ['note.on', 64], ['note.off', 64]]); assert(!h.highlight(60)); assert(!h.highlight(64));
  const count = h.messages().length; h.ST.stopHeld(); h.ST.noteOff('pointer'); assert.equal(h.messages().length, count);
});

test('replacing one key preserves a different key still owning its old pitch', async () => {
  const h = await harness(); h.ST.noteOn(60, 'first', false); h.ST.noteOn(60, 'second', false); h.ST.noteOn(64, 'first', false);
  assert.deepEqual(pitches(h.notes('on')), [60, 64]); assert.equal(h.notes('off').length, 0); assert(h.highlight(60)); assert(h.highlight(64));
  h.ST.noteOff('second'); assert.deepEqual(pitches(h.notes('off')), [60]); assert(!h.highlight(60)); assert(h.highlight(64)); h.ST.noteOff('first'); assert.deepEqual(pitches(h.notes('off')), [60, 64]);
});

test('high-note chords are deduplicated after MIDI clamping', async () => {
  const h = await harness(); h.state.chord = 'major'; h.state.octave = 8; h.down('i', 'KeyI');
  assert.deepEqual(pitches(h.notes('on')), [127]); h.up('i', 'KeyI'); assert.deepEqual(pitches(h.notes('off')), [127]);
  h.ST.noteOn(125, 'near-limit', false, 0, 0, true); assert.deepEqual(pitches(h.notes('on').slice(1)), [125, 127]);
  h.ST.noteOff('near-limit'); assert.deepEqual(pitches(h.notes('off').slice(1)), [125, 127]);
});

test('Stop, Escape, view/session changes, keyboard disable and focus loss release every shared voice exactly once', async () => {
  const actions = {
    Stop: h => h.node('stop').click(), Escape: h => h.down('Escape', 'Escape'),
    view: h => h.ST.setView('instrument'), session: h => h.ST.acceptProject(h.ST.demoProject()),
    keyboard: h => h.down('p', 'KeyP', { metaKey: true }), blur: h => h.blur()
  };
  for (const [name, action] of Object.entries(actions)) {
    const h = await harness(); h.state.chord = 'major'; h.down('q', 'KeyQ'); h.down(',', 'Comma'); h.down('z', 'KeyZ');
    const sounding = pitches(h.notes('on')); assert.deepEqual(sounding, [60, 64, 67, 72, 76, 79]);
    await action(h); await h.settle(); assert.deepEqual(pitches(h.notes('off')), sounding, name);
    for (const pitch of sounding) assert(!h.highlight(pitch), `${name}: pitch ${pitch} is no longer held`);
    h.up('q', 'KeyQ'); h.up(',', 'Comma'); h.up('z', 'KeyZ'); h.ST.stopHeld(); assert.equal(h.notes('off').length, sounding.length, `${name}: later releases must not touch another voice`);
  }
});

test('live recording keeps a shared gate open until its final owner releases without retriggering audio', async () => {
  for (const reverse of [false, true]) {
    const h = await harness(); Object.assign(h.state, { recordPlacement:'live', recording: true, playing: true, row: 0, status: { pattern: 0, row: 0, seconds: 10 } }); h.state.project.viewState = { gateColumn: true }; h.state.project.bpm = 120; h.state.project.linesPerBeat = 4; h.state.project.swing = 0; h.state.project.patterns[0].steps[0][0] = null;
    h.down('q', 'KeyQ'); h.down(',', 'Comma'); await h.acknowledged(); assert.equal(h.audible('on').length, 1); assert.equal(h.state.undo.length, 1);
    const keys = reverse ? [[',', 'Comma'], ['q', 'KeyQ']] : [['q', 'KeyQ'], [',', 'Comma']]; h.advance(250); h.up(...keys[0]); await h.acknowledged();
    assert.equal(h.audible('off').length, 0); assert.equal(h.state.project.patterns[0].steps[0][0].gate, 0); assert.equal(h.state.undo.length, 1, 'A partial release cannot finalize the recorded gate'); const gateBefore = h.state.project.patterns[0].steps[0][0].gate;
    h.advance(500); h.up(...keys[1]); await h.acknowledged(); const step = h.state.project.patterns[0].steps[0][0]; assert.equal(step.note, 72); assert.equal(step.chord, undefined); assert.equal(step.gate, 6); assert.notEqual(step.gate, gateBefore); assert.equal(h.state.undo.length, 1, 'Notes and their gates join the take'); assert.equal(h.audible('off').length, 1);
    h.up(...keys[0]); h.ST.stopHeld(); await h.acknowledged(); assert.equal(h.state.undo.length, 1, 'A completed gate is finalized once');
  }
});

test('overlapping live chords share one duration group and retain their full recorded note set', async () => {
  const h = await harness(); Object.assign(h.state, { recordPlacement:'live', recording: true, playing: true, chord: 'major', status: { pattern: 0, row: 0, seconds: 10 } }); h.state.project.viewState = { gateColumn: true }; h.state.project.bpm = 120; h.state.project.linesPerBeat = 4; h.state.project.swing = 0; h.state.project.patterns[0].steps[0][0] = null;
  h.down('z', 'KeyZ'); h.down('b', 'KeyB'); await h.acknowledged(); h.advance(200); h.up('z', 'KeyZ'); await h.acknowledged(); assert.deepEqual(pitches(h.audible('off')), [60, 64]); assert.equal(h.state.undo.length, 1);
  h.advance(800); h.up('b', 'KeyB'); await h.acknowledged(); const step = h.state.project.patterns[0].steps[0][0]; assert.equal(step.note, 60); assert.deepEqual(Array.from(step.chord), [0, 4, 7, 11, 14]); assert.equal(step.gate, 8); assert.equal(h.state.undo.length, 1);
});

test('one live recording pass is one Undo step until transport stop, and an intervening edit splits the take', async () => {
  const h = await harness(); Object.assign(h.state, { recordPlacement:'live', recording: true, playing: true, row: 0, status: { pattern: 0, row: 0, seconds: 10 } }); h.state.undo = [];
  const before = copy(h.state.project.patterns[0].steps[0]), pat = h.state.project.patterns[0];
  for (let i = 0; i < 46; i++) { h.state.status = { pattern: 0, row: i % pat.length, seconds: 10 + i / 8 }; h.down('z', 'KeyZ'); h.advance(60); h.up('z', 'KeyZ'); await h.acknowledged(); }
  assert.equal(h.state.undo.length, 1, 'A 46-note take with gates is a single history entry'); assert.equal(h.state.undo[0].label, 'record take');
  h.ST.mutate('manual edit', p => { p.masterVolume = .3; }, { render: false }); h.state.status = { pattern: 0, row: 1, seconds: 20 }; h.down('x', 'KeyX'); h.advance(60); h.up('x', 'KeyX'); await h.acknowledged();
  assert.deepEqual(h.state.undo.map(x => x.label), ['record take', 'manual edit', 'record take'], 'Notes after another edit start a new step rather than rewriting history under it');
  h.window.nativeEvent({ type: 'status', payload: { playing: false, pattern: 0, row: 0, seconds: 0 } }); Object.assign(h.state, { playing: true, status: { pattern: 0, row: 3, seconds: 30 } });
  h.down('c', 'KeyC'); h.advance(60); h.up('c', 'KeyC'); await h.acknowledged(); assert.equal(h.state.undo.length, 4, 'Stopping the transport ends the take');
  h.ST.perform('undo'); h.ST.perform('undo'); h.ST.perform('undo'); h.ST.perform('undo'); assert.deepEqual(copy(h.state.project.patterns[0].steps[0]), before, 'Undoing the takes restores the pre-recording lane');
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log(`PASS ${name}`); } catch (error) { failures++; console.error(`FAIL ${name}\n${error.stack}`); } } console.log(`${tests.length - failures}/${tests.length} held-note UI suites passed.`); if (failures) process.exitCode = 1; })();
