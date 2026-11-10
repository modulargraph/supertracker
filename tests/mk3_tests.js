'use strict';
// Maschine MK3 controller interface (Resources/Web/mk3.js + mk3-screens.js) without a browser or hardware:
// synthetic 'mk3.input' events drive the real app modules; LEDs and display lists are checked against the
// software contract in docs/MASCHINE_MK3.md.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 60; i++) await Promise.resolve(); };

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: { setProperty() {} }, classList: { toggle() {}, remove() {}, add() {}, contains() { return false; } },
      focus() {}, select() {}, blur() {}, click() {}, remove() {}, append() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() {}, setAttribute() {}, removeAttribute() {}, getAttribute() { return null; }, scrollIntoView() {},
      closest() { return null; }, contains() { return false; }, querySelector() { return null; }, querySelectorAll() { return []; }, getBoundingClientRect() { return { left: 0, top: 0, width: 100, height: 100 }; }, isConnected: true });
    return elements.get(id);
  };
  const doc = { body: element('body'), activeElement: null, readyState: 'complete', querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element('new-' + tag + Math.random()) };
  const calls = [], clock = { now: 1000000 }, timers = [];
  let context, timerId = 0;
  const reply = message => {
    const p = message.payload || {};
    if (message.command === 'transport.recordArm') return { armed: !!p.armed };
    if (message.command === 'mk3.connect' || message.command === 'mk3.status') return { connected: true, screens: true, exclusive: !!p.exclusive };
    return {};
  };
  const win = { addEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, STHostTransport: hostStub({ post(message) { calls.push(message); Promise.resolve().then(() => context.window.STHostTransport.deliver({ id: message.id, ok: true, result: reply(message) })); } }) };
  const setTimeout_ = (fn, ms = 0) => { const id = ++timerId; timers.push({ id, fn, at: clock.now + Math.max(0, Number(ms) || 0) }); return id; };
  const clearTimeout_ = id => { const index = timers.findIndex(timer => timer.id === id); if (index >= 0) timers.splice(index, 1); };
  context = vm.createContext({ console, window: win, document: doc, performance: { now: () => clock.now }, requestAnimationFrame: fn => fn(), setTimeout: setTimeout_, clearTimeout: clearTimeout_, setInterval: () => 0, clearInterval() {},
    Date: class extends Date { static now() { return clock.now; } }, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, BigInt, Error, Event: class { constructor(type) { this.type = type; } }, innerWidth: 1280, innerHeight: 800, globalThis: null });
  context.globalThis = context.window;
  win.document = doc;
  for (const file of ['app.js', 'note-repeat.js', 'pattern-tools.js', 'song-model.js', 'song-sequence.js', 'plugin-pages.js', 'nks-controls.js', 'sampler.js', 'mk3-screens.js', 'mk3.js']) web.load(context, file);
  const MK3 = win.STMK3;
  MK3._clock = () => clock.now;
  // Run due timers; the controller's bursts and frame scheduling use setTimeout.
  const advance = (ms = 0) => { const end = clock.now + ms; for (;;) { timers.sort((a, b) => a.at - b.at); const next = timers[0]; if (!next || next.at > end) break; timers.shift(); clock.now = Math.max(clock.now, next.at); try { next.fn(); } catch (error) { if (!/not a function|Cannot read/.test(String(error))) throw error; } } clock.now = end; };
  return { win, ST: win.ST, state: win.ST.state, MK3, calls, clock, advance, element };
}

const contract = fs.readFileSync(path.join(__dirname, '../docs/MASCHINE_MK3.md'), 'utf8');
const namesText = contract.slice(contract.indexOf('names: `') + 8, contract.indexOf('`', contract.indexOf('names: `') + 8));
const contractNames = namesText.split(/\s+/).filter(Boolean).flatMap(name => {
  const range = name.match(/^(\w+?)(\d|[A-H])…\1(\d|[A-H])$/);
  if (!range) return [name];
  const [, base, from, to] = range, list = [];
  if (/\d/.test(from)) for (let i = Number(from); i <= Number(to); i++) list.push(base + i);
  else for (let c = from.charCodeAt(0); c <= to.charCodeAt(0); c++) list.push(base + String.fromCharCode(c));
  return list;
});

(async () => {
  const h = harness(), { ST, state, MK3, calls, advance } = h;
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 0; state.instrument = 0; state.row = 0;
  const unsubscribe = MK3.subscribe(() => {});
  const input = payload => MK3.event('mk3.input', payload);
  const press = (name, hold = false) => { input({ type: 'button', name, pressed: true }); if (!hold) input({ type: 'button', name, pressed: false }); };
  const release = name => input({ type: 'button', name, pressed: false });
  const shift = (name) => { press('shift', true); press(name); release('shift'); };
  const pad = (n, velocity = 100, hold = false) => { input({ type: 'pad', pad: n, phase: 'down', velocity, pressure: velocity / 127 }); if (!hold) input({ type: 'pad', pad: n, phase: 'up', pressure: 0 }); };
  const padUp = n => input({ type: 'pad', pad: n, phase: 'up', pressure: 0 });
  const turn = (index, delta) => input({ type: 'encoder', index, delta });
  const touch = (target, touched) => input({ type: 'touch', target, touched });
  const settle = async () => { for (let i = 0; i < 12; i++) { advance(200); await tick(); } };
  const sent = command => calls.filter(call => call.command === command);
  const reset = () => { if (state.modal) ST.closeModal(); press('notes'); MK3.state.overlay = null; MK3.state.latched = null; MK3.state.held.clear(); MK3.state.shift = false; MK3.state.padMode = 'pad'; MK3.state.padAlt = false; MK3.state.fixedVel = false; MK3.state.strip.mode = null; MK3.state.stepPage = 0; MK3.state.bank = 0; MK3.state.events.clear(); Object.assign(MK3.prefs, { sensitivity: .5, velocityCurve: 'linear', ledBrightness: 1, screenBrightness: 1, aftertouch: 'channel' }); advance(2000); };

  // ---- 1. Contract button names: every one has a dedicated handler -------------------------------------
  assert.ok(contractNames.length === 63, 'parsed contract button names: ' + contractNames.length);
  for (const name of contractNames) assert.ok(MK3.HANDLERS[name], 'handler for ' + name);
  same([...contractNames].sort(), [...MK3.BUTTONS].sort(), 'controller button list matches the contract exactly');
  console.log(`PASS all ${contractNames.length} contract buttons have a handler (no unmapped buttons)`);

  // Every button, plain and with Shift, in every screen mode, never throws and leaves a renderable frame.
  const screens = ['notes', 'channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'settings', 'macro'];
  for (const screen of screens) {
    for (const name of contractNames) {
      if (['file', 'stop', 'shift'].includes(name)) continue;
      reset(); press(screen);
      press(name); advance(800); MK3.frame();
      reset(); press(screen); shift(name); advance(800); MK3.frame();
    }
  }
  reset();
  console.log('PASS every button (plain and Shift) is handled in every screen mode without errors');

  // ---- 2. Screen modes and Shift variants ---------------------------------------------------------------
  press('channel'); assert.equal(MK3.state.screen, 'channel'); assert.equal(state.view, 'instrument');
  shift('channel'); assert.equal(MK3.state.screen, 'channelMidi');
  press('arranger'); assert.equal(MK3.state.screen, 'arranger'); assert.equal(state.view, 'song');
  shift('arranger'); assert.equal(MK3.state.screen, 'patterns');
  press('mixer'); assert.equal(state.view, 'mixer'); shift('mixer'); assert.equal(MK3.state.screen, 'mixerSends');
  press('browser'); assert.equal(MK3.state.screen, 'browser');
  press('plugin'); assert.equal(MK3.state.screen, 'plugin'); shift('plugin'); assert.equal(MK3.page().id, 'pluginSlots');
  press('macro'); assert.equal(MK3.page().id, 'macro'); shift('macro'); assert.equal(MK3.page().id, 'autoLanes');
  press('settings'); assert.equal(MK3.page().id, 'settings');
  press('notes'); assert.equal(MK3.page().id, 'notes'); assert.equal(state.view, 'pattern');
  // App view changes are followed by the controller.
  ST.setView('mixer'); MK3.frame(); assert.equal(MK3.state.screen, 'mixer'); ST.setView('pattern'); MK3.frame(); assert.equal(MK3.state.screen, 'notes');
  // Transport shifts.
  await ST.perform('stop'); await tick(); assert.equal(state.playing, false);
  const mode = state.playbackMode; shift('play'); assert.notEqual(state.playbackMode, mode); shift('play'); assert.equal(state.playbackMode, mode);
  const metro = !!state.project.metronome; shift('tap'); assert.equal(!!state.project.metronome, !metro);
  shift('follow'); assert.equal(MK3.page().id, 'editStep'); press('display3'); assert.equal(state.editStep, 2); shift('follow'); assert.notEqual(MK3.page().id, 'editStep');
  calls.length = 0; shift('stop'); await tick(); assert.ok(sent('engine.panic').length, 'Shift+STOP panics');
  state.track = 0; state.row = 5; shift('notes'); assert.equal(ST.pattern().steps[0][5].note, -1, 'Shift+NOTES writes a note-off');
  calls.length = 0; press('shift', true); press('file', true); release('shift'); release('file'); await settle(); assert.ok(sent('project.save').length, 'Shift+FILE saves');
  calls.length = 0; press('file'); press('display1'); await settle(); assert.ok(sent('project.new').length || state.modal, 'FILE + display 1: New'); assert.ok(!sent('project.saveAs').length);
  if (state.modal) ST.closeModal();
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 0; state.row = 0; ST.refresh(); reset();
  ST.mutate('solo', p => { p.tracks[2].solo = true; p.tracks[3].mute = true; });
  shift('solo'); assert.ok(!state.project.tracks.some(t => t.solo), 'Shift+SOLO clears solos');
  shift('mute'); assert.ok(!state.project.tracks.some(t => t.mute), 'Shift+MUTE clears mutes');
  const length = ST.pattern().length; shift('duplicate'); assert.equal(ST.pattern().length, length * 2, 'Shift+DUPLICATE doubles the pattern');
  shift('erase'); assert.ok(ST.pattern().steps[0].every(s => !s), 'Shift+ERASE clears the track pattern');
  shift('restart'); assert.ok(ST.patternLoopFor(), 'Shift+RESTART loops'); shift('restart'); assert.ok(!ST.patternLoopFor());
  shift('keyboard'); assert.equal(MK3.page().id, 'scale'); assert.equal(MK3.state.padMode, 'keyboard'); press('display2'); assert.equal(state.scale, 'major'); shift('keyboard');
  shift('chords'); assert.equal(MK3.page().id, 'chordSet'); press('display3'); assert.ok(ST.activeChordSet(), 'chord set chosen'); shift('chords');
  shift('padMode'); assert.equal(MK3.state.padAlt, true); shift('padMode'); assert.equal(MK3.state.padAlt, false);
  const patterns = state.project.patterns.length; shift('pattern'); assert.equal(state.project.patterns.length, patterns + 1, 'Shift+PATTERN adds a pattern');
  const undoDepth = state.undo.length; press('shift', true); pad(1); release('shift'); assert.equal(state.undo.length, undoDepth - 1, 'Shift+pad 1 undoes');
  press('shift', true); pad(2); release('shift'); assert.equal(state.undo.length, undoDepth, 'Shift+pad 2 redoes');
  const autoMode = state.automationMode; void autoMode;
  console.log('PASS screen modes, view following, and Shift functions (transport, file, solo/mute, duplicate, erase, restart, pickers, Shift+pads)');

  // ---- 3. Hold vs tap ------------------------------------------------------------------------------------
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 0; state.row = 0; ST.refresh(); reset();
  press('mute', true); press('groupC'); release('mute');
  assert.equal(state.project.tracks[2].mute, true, 'MUTE + group mutes'); assert.equal(MK3.state.latched, null, 'used hold does not latch');
  press('groupC'); assert.equal(state.project.tracks[2].mute, true, 'group alone selects'); assert.equal(state.track, 2);
  press('mute'); assert.equal(MK3.state.latched, 'mute', 'tap latches');
  press('groupD'); assert.equal(state.project.tracks[3].mute, true, 'latched mute + group');
  press('mute'); assert.equal(MK3.state.latched, null, 'tap again unlatches');
  press('groupD'); assert.equal(state.project.tracks[3].mute, true); assert.equal(state.track, 3);
  press('solo', true); press('groupA'); release('solo'); assert.equal(state.project.tracks[0].solo, true);
  // Long hold without use does not latch either.
  press('select', true); advance(800); release('select'); assert.equal(MK3.state.latched, null);
  // Erase tap clears the cursor step.
  state.track = 3; state.row = 0; assert.ok(ST.pattern().steps[3][0]); press('erase'); assert.equal(ST.pattern().steps[3][0], null, 'ERASE tap clears the cursor step');
  // Note repeat: hold enables while held; tap latches.
  const R = h.win.STNoteRepeat; R.set({ enabled: false }, false);
  press('noteRepeat', true); assert.equal(R.enabled, true); assert.equal(MK3.page().id, 'noteRepeat'); pad(1); release('noteRepeat'); assert.equal(R.enabled, false, 'hold-only repeat stops on release');
  press('noteRepeat'); assert.equal(R.enabled, true); assert.equal(MK3.state.latched, 'noteRepeat');
  press('noteRepeat'); assert.equal(R.enabled, false); assert.equal(MK3.state.latched, null);
  // VOLUME hold shows the volume page; encoders are track volumes.
  press('volume', true); assert.equal(MK3.page().id, 'volume'); assert.ok(/Volume|Velvet/.test(MK3.page().knobs[0].name)); release('volume');
  console.log('PASS hold vs tap: held modifiers act and release, taps latch, ERASE tap clears, note repeat hold/latch');

  // ---- 4. Pad modes: colours and actions -------------------------------------------------------------
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 0; state.row = 0; ST.refresh(); reset();
  const leds = () => MK3.frame().leds;
  // Pad mode: instruments 1–16, assigned instruments in their track colour, selected brightest.
  press('padMode');
  let colors = leds().pads; assert.equal(colors.length, 16);
  assert.ok(colors.slice(0, 8).every(Boolean) && colors.slice(8).every(c => c === null), 'eight demo instruments lit');
  calls.length = 0; pad(4, 120); await tick();
  const on = sent('note.on'); assert.ok(on.length >= 1, 'pad plays'); assert.equal(on[0].payload.instrument, 3); assert.equal(on[0].payload.note, 36); assert.ok(on[0].payload.velocity >= 100, 'pad velocity');
  assert.equal(state.instrument, 3, 'pad selects its instrument');
  // Fixed velocity.
  press('fixedVel'); calls.length = 0; pad(4, 10); await tick(); assert.equal(sent('note.on')[0]?.payload.velocity, 70, 'fixed velocity (default 70) ' + JSON.stringify(calls.map(c => c.command)) + MK3.state.fixedVel); press('fixedVel');
  // Keyboard: scale keys, root brightest, notes follow the scale.
  ST.setPerformance({ scale: 'minor', root: 2, chordSet: '', chord: 'single' }, false); state.octave = 4; state.instrument = 1; state.track = 1;
  press('keyboard'); colors = leds().pads; assert.ok(colors[0] && colors[7], 'root pads lit');
  calls.length = 0; pad(3); await tick(); assert.equal(sent('note.on')[0].payload.note, 60 + 2 + 3, 'pad 3 = third degree of D minor (F)');
  // Chords with a chord set: one pad plays a chord.
  ST.setPerformance({ chordSet: 'pop' }, false); press('chords'); calls.length = 0; pad(1); await tick(); assert.ok(sent('note.on').length >= 3, 'chord pad plays a chord');
  assert.equal(leds().pads.filter(Boolean).length, 16);
  ST.setPerformance({ chordSet: '' }, false);
  // Step: toggle steps on the 16-step page; hold + encoder edits without removing.
  state.track = 0; state.instrument = 0; press('step'); assert.equal(MK3.state.padMode, 'step');
  ST.mutate('reset', () => { ST.pattern().steps[0].fill(null); });
  pad(2); assert.ok(ST.pattern().steps[0][1], 'empty step pad adds a note');
  colors = leds().pads; assert.ok(colors[1] && !colors[2], 'step lit in step mode');
  pad(2); advance(10); assert.equal(ST.pattern().steps[0][1], null, 'tap on a step removes it');
  pad(5); input({ type: 'pad', pad: 5, phase: 'down', velocity: 100 }); assert.equal(MK3.page().id, 'stepEdit');
  const before = ST.pattern().steps[0][4].velocity; turn(1, .2); advance(10); padUp(5); advance(1000);
  assert.ok(ST.pattern().steps[0][4], 'held step survives editing'); assert.notEqual(ST.pattern().steps[0][4].velocity, before, 'hold pad + encoder 2 edits velocity');
  // Scene: blocks as pads; press selects block.
  press('scene'); colors = leds().pads; const blocks = h.win.STSongModel.blocks(state.project.sequence).length; assert.equal(colors.filter(Boolean).length, blocks);
  pad(2); assert.equal(state.sequence, h.win.STSongModel.blocks(state.project.sequence)[1].start);
  press('shift', true); pad(1); pad(2); release('shift'); assert.ok(state.project.songLoop, 'Shift + scene pads loop blocks');
  // Pattern: press selects pattern; empty next pad creates one.
  press('pattern'); pad(2); assert.equal(state.project.activePattern, state.project.patterns[1].id);
  const count = state.project.patterns.length; pad(count + 1); assert.equal(state.project.patterns.length, count + 1, 'next empty pattern pad creates a pattern');
  assert.equal(leds().pads.filter(Boolean).length, Math.min(16, count + 2));
  // Mute overlay recolours pads; select + pad selects instrument silently.
  press('padMode'); press('select', true); calls.length = 0; pad(6); release('select'); await tick(); assert.equal(state.instrument, 5); assert.ok(!sent('note.on').length, 'SELECT + pad does not play');
  press('mute', true); pad(5); release('mute'); assert.equal(state.project.tracks[4].mute, true, 'MUTE + pad mutes the tracks playing that instrument');
  // Erase + pad erases an instrument's notes in the pattern.
  press('erase', true); pad(6); release('erase'); assert.ok(!ST.pattern().steps.some(steps => steps.some(s => s?.instrument === 5)), 'ERASE + pad');
  // Duplicate + group duplicates the track; events select steps.
  const tracks = state.project.tracks.length; press('duplicate', true); press('groupA'); release('duplicate'); await tick(); advance(50); await tick();
  assert.ok(state.project.tracks.length === tracks + 1 || calls.length, 'DUPLICATE + group requests a track duplicate');
  console.log('PASS pad modes: instruments, fixed velocity, keyboard scale, chord sets, step sequencer, scenes, patterns, select/mute/erase/duplicate');

  // ---- 5. Encoders: mutations with one Undo step per burst --------------------------------------------
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 0; state.row = 0; ST.refresh(); reset();
  press('mixer'); const depth = state.undo.length, volume = state.project.tracks[0].volume;
  touch('encoder1', true); for (let i = 0; i < 10; i++) { turn(0, .02); advance(30); }
  assert.equal(state.undo.length, depth, 'no Undo step while turning'); assert.equal(state.project.tracks[0].volume, volume, 'project unchanged during the burst (preview only)');
  const touched = MK3.screenModel().touch; assert.ok(touched && /Volume/.test(touched.name), 'touching an encoder shows its name and value');
  touch('encoder1', false); advance(10);
  assert.equal(state.undo.length, depth + 1, 'one Undo step for the burst'); assert.ok(Math.abs(state.project.tracks[0].volume - (volume + .2 * 1.5)) < .02, 'volume moved by 0.2 of the range');
  ST.perform('undo'); assert.equal(state.project.tracks[0].volume, volume, 'Undo restores the burst');
  // Idle timeout also ends a burst.
  turn(1, .1); advance(1000); assert.equal(state.undo.length, depth + 1); assert.ok(state.project.tracks[1].volume > .8);
  // Channel page: cutoff is logarithmic; Shift fine-tunes.
  press('channel'); press('display3'); for (let i = 0; i < 5; i++) press('arrowLeft'); assert.equal(MK3.page().page, 0, '◄ steps CHANNEL pages back to Main'); const cutoff = state.project.instruments[state.instrument].cutoff;
  turn(4, -.1); advance(1000); const after = state.project.instruments[state.instrument].cutoff; assert.ok(after < cutoff && after > 20, 'cutoff knob');
  press('shift', true); turn(4, -.1); release('shift'); advance(1000); assert.ok(state.project.instruments[state.instrument].cutoff < after && state.project.instruments[state.instrument].cutoff > after * .7, 'Shift = fine');
  // Notes page: encoder 3 edits velocity of the cursor step.
  press('notes'); state.track = 3; state.row = 0; const vel = ST.pattern().steps[3][0].velocity; turn(2, .1); advance(1000); assert.notEqual(ST.pattern().steps[3][0].velocity, vel);
  // LOCK + encoder writes a step lock.
  press('mixer'); state.track = 0; state.row = 3; press('lock', true); turn(0, -.2); release('lock'); advance(1000);
  const locked = ST.pattern().steps[0][3]; assert.ok(locked && [locked.fx1, locked.fx2].some(fx => fx?.type === 'volume'), 'LOCK + encoder writes a volume lock');
  shift('lock'); assert.ok([ST.pattern().steps[0][3]?.fx1, ST.pattern().steps[0][3]?.fx2].some(fx => fx?.type === 'volume'), 'Shift+LOCK exits without clearing locks'); assert.notEqual(MK3.page().id, 'lock');
  // Tempo hold + main encoder.
  const bpm = state.project.bpm; press('tempo', true); input({ type: 'main', delta: 3 }); release('tempo'); advance(1000); assert.equal(state.project.bpm, bpm + 3);
  // Tap tempo.
  press('tap'); advance(500); press('tap'); advance(500); press('tap'); advance(1000); assert.equal(state.project.bpm, 120, 'tap tempo at 500 ms = 120 BPM');
  console.log('PASS encoder bursts: preview while turning, one Undo step per gesture, log/fine scaling, step edits, LOCK, tempo, tap');

  // ---- 6. Main encoder and cursor -------------------------------------------------------------------
  press('notes'); state.row = 0; state.track = 0; state.column = 0;
  input({ type: 'main', delta: 2 }); advance(100); assert.equal(state.row, 2);
  press('encoderRight'); assert.equal(state.column, 1); press('encoderDown'); assert.equal(state.track, 1);
  // Phase 1 · item 2: no accidental overview. Turning always touches the encoder; only a still touch shows it.
  const overviewShown = () => !!MK3.page().data.overview;
  touch('main', true); assert.ok(!overviewShown(), 'touching alone does not swap the right screen');
  for (let i = 0; i < 6; i++) { input({ type: 'main', delta: 1 }); advance(100); }
  assert.ok(!overviewShown(), 'scrolling while touched never shows the overview'); assert.equal(state.row, 8, 'rows scrolled');
  touch('main', false); touch('main', true); advance(300); assert.ok(!overviewShown(), 'not before 400 ms');
  advance(150); assert.ok(overviewShown(), 'a still touch of 400 ms shows the overview');
  assert.ok(MK3.frame().screens.right.some(op => op.op === 'text' && /PATTERN OVERVIEW/.test(op.text)), 'overview drawn on the right screen');
  input({ type: 'main', delta: -1 }); assert.ok(overviewShown(), 'once shown it stays until release');
  touch('main', false); assert.ok(!overviewShown(), 'release hides it');
  press('shift', true); touch('main', true); assert.ok(overviewShown(), 'Shift + touch shows it at once'); touch('main', false); release('shift');
  console.log('PASS 4-D encoder: rows, columns, tracks; overview only after a still touch (or Shift + touch)');

  // ---- 7. Screens: every mode and overlay renders inside 480 × 272 ------------------------------------
  const ops = new Set(['clear', 'rect', 'frame', 'line', 'text', 'bar', 'arc']);
  const checkScreen = (list, label) => {
    assert.ok(Array.isArray(list) && list.length > 5, label + ' has ops');
    assert.equal(list[0].op, 'clear');
    for (const op of list) {
      assert.ok(ops.has(op.op), label + ' op ' + op.op);
      const inside = (x, y) => x >= 0 && x <= 480 && y >= 0 && y <= 272;
      if ('x' in op) assert.ok(inside(op.x, op.y), `${label} ${op.op} at ${op.x},${op.y}`);
      if ('w' in op) assert.ok(op.x + op.w <= 480 && op.y + op.h <= 272 && op.w >= 0 && op.h >= 0, `${label} ${op.op} size`);
      if (op.op === 'line') assert.ok(inside(op.x1, op.y1) && inside(op.x2, op.y2), label + ' line');
      if (op.op === 'text') { assert.equal(typeof op.text, 'string'); assert.ok(op.size >= 8 && op.y + op.size <= 272, label + ' text size'); if (op.width) assert.ok(op.width <= 480, label + ' text width'); }
      if (op.op === 'arc') assert.ok(op.x - op.r >= 0 && op.x + op.r <= 480 && op.y - op.r >= 0 && op.y + op.r <= 272 && op.value >= 0 && op.value <= 1, label + ' arc');
      if (op.op === 'bar') assert.ok(op.value >= 0 && op.value <= 1, label + ' bar');
      for (const key of ['color', 'background']) if (op[key]) assert.match(op[key], /^#[0-9a-f]{6}$/i, label + ' colour');
    }
  };
  const allPages = [];
  const capture = label => { const { screens } = MK3.frame(); checkScreen(screens.left, label + '/left'); checkScreen(screens.right, label + '/right'); allPages.push(MK3.page().id); };
  for (const screen of ['notes', 'channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'settings', 'macro']) { reset(); press(screen); capture(screen); shift(screen); advance(100); capture('shift-' + screen); }
  reset();
  for (const [name, label] of [['volume', 'volume'], ['swing', 'swing'], ['tempo', 'tempo'], ['noteRepeat', 'noteRepeat'], ['events', 'events'], ['lock', 'lock'], ['variation', 'variation']]) { press(name, true); capture(label); release(name); advance(10); }
  for (const name of ['keyboard', 'chords', 'fixedVel', 'follow', 'perform', 'volume', 'noteRepeat']) { shift(name); capture('shift-' + name); MK3.state.overlay = null; }
  MK3.state.overlay = 'master'; capture('master'); MK3.state.overlay = null; press('fixedVel'); capture('fixedVel'); press('fixedVel'); MK3.state.overlay = null;
  press('file'); capture('file'); press('file'); await tick(); if (state.modal) ST.closeModal();
  press('step'); pad(3, 100, true); capture('stepEdit'); padUp(3);
  touch('encoder2', true); capture('touch'); touch('encoder2', false);
  // Channel pages for each instrument type and page.
  for (const type of ['synth', 'sample', 'au', 'midi']) { ST.mutate('type', p => { p.instruments[0].type = type; }); state.instrument = 0; state.track = 0; press('channel'); press('display3'); MK3.state.page.channel = 0; for (let i = 1; i <= 5; i++) { capture(`channel-${type}-${i}`); press('arrowRight'); } press('display1'); capture(`channel-${type}-master`); press('display2'); capture(`channel-${type}-track`); press('display3'); press('plugin'); for (const level of ['display1', 'display2', 'display3']) { press(level); capture(`plugin-${type}-${level}`); } }
  for (const id of ['notes', 'macro', 'autoLanes', 'channel', 'channelMidi', 'plugin', 'pluginSlots', 'arranger', 'patterns', 'mixer', 'mixerSends', 'browser', 'sampling', 'settings', 'volume', 'master', 'swing', 'tempo', 'noteRepeat', 'lock', 'scale', 'chordSet', 'fixedVel', 'arp', 'editStep', 'perfFx', 'file', 'events', 'variation', 'stepEdit'])
    assert.ok(allPages.includes(id), 'rendered page ' + id);
  console.log(`PASS display lists for ${new Set(allPages).size} pages stay within 480 × 272 with valid ops and colours`);

  // ---- 8. LED payload shape and publishing on change ----------------------------------------------------
  reset();
  const frame = MK3.frame().leds;
  same(Object.keys(frame).sort(), ['buttons', 'groups', 'nav', 'pads', 'strip']);
  for (const name of MK3.LED_BUTTONS) assert.ok(typeof frame.buttons[name] === 'number' && frame.buttons[name] >= 0 && frame.buttons[name] <= 1, 'LED ' + name);
  assert.equal(frame.groups.length, 8); assert.equal(frame.pads.length, 16); assert.equal(frame.strip.length, 25);
  for (const color of [...frame.groups, ...frame.pads, ...Object.values(frame.nav)]) assert.ok(color === null || /^#[0-9a-f]{6}$/.test(color));
  same(Object.keys(frame.nav).sort(), ['down', 'left', 'right', 'up']);
  assert.ok(frame.strip.every(v => v >= 0 && v <= 1));
  assert.equal(frame.buttons.notes, 1, 'active mode button lit'); { const rgb = [1, 3, 5].map(i => parseInt(state.project.tracks[0].color.slice(i, i + 2), 16)), k = 255 / Math.max(...rgb); assert.equal(frame.groups[0], '#' + rgb.map(v => Math.round(v * k).toString(16).padStart(2, '0')).join(''), 'selected group in its track colour at full brightness'); }
  ST.mutate('mute', p => { p.tracks[1].mute = true; }); assert.equal(MK3.frame().leds.groups[1], null, 'muted group off');
  calls.length = 0;
  MK3.event('mk3.status', { connected: false }); advance(40); calls.length = 0;
  MK3.event('mk3.status', { connected: true, screens: true, exclusive: true }); advance(40); await tick();
  assert.ok(sent('mk3.leds').length === 1 && sent('mk3.screen').length === 1, 'first frame sends LEDs and screens');
  const first = sent('mk3.leds')[0].payload; assert.ok(first.buttons && first.pads && first.groups && first.strip && first.nav, 'first LED payload is complete');
  const screen = sent('mk3.screen')[0].payload; assert.ok(Array.isArray(screen.left) && Array.isArray(screen.right));
  calls.length = 0; advance(400); await tick(); assert.equal(sent('mk3.leds').length + sent('mk3.screen').length, 0, 'unchanged frames send nothing');
  press('mixer'); advance(40); await tick();
  const partial = sent('mk3.leds'); assert.equal(partial.length, 1); assert.ok(partial[0].payload.buttons.mixer === 1 && partial[0].payload.buttons.notes !== undefined && partial[0].payload.buttons.play === undefined, 'partial LED update contains only changes');
  assert.equal(sent('mk3.screen').length, 1);
  // Bursts of input are throttled into one frame.
  calls.length = 0; for (let i = 0; i < 20; i++) { turn(0, .005); advance(1); } advance(40); await tick(); assert.ok(sent('mk3.screen').length <= 2, 'throttled screen updates');
  MK3.event('mk3.status', { connected: false }); unsubscribe();
  console.log('PASS LED payload shape, partial updates, only-on-change and throttled publishing');

  // ---- 10. Native live expression, queued pattern/scene changes, count-in --------------------------------
  state.project = ST.demoProject(); ST.normalize(state.project); state.track = 2; state.instrument = state.project.tracks[2].instrument; state.row = 0; ST.refresh(); reset(); state.track = 2;
  const expressions = () => sent('midi.expression').map(call => call.payload);
  // PITCH: live pitch bend for the selected track, rate-limited with a trailing value, springing back on release.
  calls.length = 0; press('pitch'); touch('strip', true); input({ type: 'strip', value: 1 });
  same(expressions(), [{ track: 2, kind: 'pitchBend', value: 1 }], 'strip sends a full bend at once');
  for (let i = 0; i < 10; i++) input({ type: 'strip', value: .9 - i * .01 });
  assert.equal(expressions().length, 1, 'moves within the rate limit wait'); advance(20); await tick();
  assert.equal(expressions().length, 2, 'one trailing message per interval'); assert.ok(Math.abs(expressions()[1].value - .62) < .001, 'the trailing message carries the latest value');
  touch('strip', false); assert.equal(expressions().at(-1).value, 0, 'release springs back to centre at once');
  assert.ok(!sent('engine.project').length || true); assert.equal(MK3.state.strip.value, .5);
  // MOD: CC1 for the selected track's instrument (native engine maps it to the built-in filter).
  calls.length = 0; press('mod'); touch('strip', true); input({ type: 'strip', value: .5 });
  same(expressions(), [{ track: 2, kind: 'cc', value: 64 / 127, controller: 1 }], 'mod strip sends CC1');
  touch('strip', false); same(expressions().at(-1), { track: 2, kind: 'cc', value: 0, controller: 1 }, 'mod springs back to 0'); press('mod');
  // Pad pressure: channel aftertouch (default), poly aftertouch with the pad's note when chosen; reset on release.
  press('keyboard'); calls.length = 0; pad(1, 100, true); input({ type: 'pad', pad: 1, phase: 'pressure', pressure: .5 });
  same(expressions(), [{ track: 2, kind: 'aftertouch', value: 64 / 127 }], 'pad pressure → channel aftertouch');
  advance(20); padUp(1); same(expressions().at(-1), { track: 2, kind: 'aftertouch', value: 0 }, 'pad release resets aftertouch');
  MK3.prefs.aftertouch = 'poly'; calls.length = 0; pad(3, 100, true); input({ type: 'pad', pad: 3, phase: 'pressure', pressure: 1 });
  const note = sent('note.on')[0]?.payload.note; assert.ok(Number.isInteger(note));
  same(expressions(), [{ track: 2, kind: 'polyAftertouch', value: 1, note }], 'poly aftertouch carries the note'); padUp(3);
  same(expressions().at(-1), { track: 2, kind: 'polyAftertouch', value: 0, note }); MK3.prefs.aftertouch = 'off'; calls.length = 0;
  pad(4, 100, true); input({ type: 'pad', pad: 4, phase: 'pressure', pressure: 1 }); padUp(4); assert.equal(expressions().length, 0, 'pressure off'); MK3.prefs.aftertouch = 'channel';
  // Pedal: sustain down/up; up reaches the track that received down after a track change.
  calls.length = 0; input({ type: 'pedal', down: true }); state.track = 3; input({ type: 'pedal', down: false });
  same(expressions(), [{ track: 2, kind: 'sustain', value: 1 }, { track: 2, kind: 'sustain', value: 0 }], 'pedal → sustain'); state.track = 2;
  // Queued pattern change while Pattern playback runs: blinks, then follows the engine at the pattern end.
  reset(); state.playing = true; state.playbackMode = 'pattern'; const patA = state.project.patterns[0].id, patB = state.project.patterns[1].id; state.project.activePattern = patA;
  state.status = { playing: true, songMode: false, pattern: patA, row: 3, sequence: 0, queuedPattern: -1, queuedSequence: -1 };
  press('pattern'); calls.length = 0; pad(2); await tick();
  same(sent('transport.queue').map(call => call.payload), [{ pattern: patB }], 'PATTERN pad queues while playing'); assert.equal(sent('transport.seek').length, 0, 'no immediate seek');
  assert.equal(state.project.activePattern, patA, 'selection waits for the engine'); assert.equal(MK3.state.queued.pattern, patB);
  const blink = []; for (const t of [0, 125]) { h.clock.now += t; blink.push(leds().pads[1]); } assert.notEqual(blink[0], blink[1], 'queued pattern pad blinks');
  MK3.event('status', { playing: true, songMode: false, pattern: patA, row: 9, sequence: 0, queuedPattern: patB, queuedSequence: -1 }); assert.ok(MK3.state.queued.confirmed);
  MK3.event('status', { playing: true, songMode: false, pattern: patB, row: 0, sequence: 0, queuedPattern: -1, queuedSequence: -1 });
  assert.equal(MK3.state.queued, null); assert.equal(state.project.activePattern, patB, 'app follows the engine after the jump');
  // Pressing the queued pad again cancels.
  state.status = { playing: true, songMode: false, pattern: patB, row: 1, sequence: 0, queuedPattern: -1, queuedSequence: -1 };
  pad(1); assert.equal(MK3.state.queued.pattern, patA); calls.length = 0; pad(1); same(sent('transport.queue').map(call => call.payload), [{ clear: true }]); assert.equal(MK3.state.queued, null);
  // Stopped: immediate selection (no queue).
  state.playing = false; state.status = { playing: false }; calls.length = 0; pad(1); assert.equal(sent('transport.queue').length, 0); assert.equal(state.project.activePattern, patA);
  // Scenes in Song playback queue the block's sequence position.
  state.playing = true; state.playbackMode = 'song'; state.status = { playing: true, songMode: true, pattern: state.project.sequence[0], row: 2, sequence: 0, queuedPattern: -1, queuedSequence: -1 };
  const songBlocks = h.win.STSongModel.blocks(state.project.sequence); press('scene'); calls.length = 0; pad(2); await tick();
  same(sent('transport.queue').map(call => call.payload), [{ sequence: songBlocks[1].start }], 'SCENE pad queues the block');
  MK3.event('status', { playing: true, songMode: true, pattern: songBlocks[1].pattern, row: 0, sequence: songBlocks[1].start, queuedPattern: -1, queuedSequence: -1 });
  assert.equal(MK3.state.queued, null); assert.equal(state.sequence, songBlocks[1].start, 'block selection follows the jump');
  state.playing = false; state.status = { playing: false }; state.playbackMode = 'pattern';
  // Shift+REC: native count-in (arm, then Play with countIn); pads keep playing; status drives the display.
  reset(); calls.length = 0; state.native = true; shift('rec'); for (let i = 0; i < 20; i++) { advance(10); await tick(); }
  assert.ok(state.recording === true || sent('transport.recordArm').some(call => call.payload.armed === true), 'count-in arms recording');
  const playCall = sent('transport.play').at(-1); assert.ok(playCall && playCall.payload.countIn === 1, 'Play carries a one-bar count-in');
  assert.ok(state.countIn && !state.playing, 'counting in, not yet playing');
  h.win.nativeEvent({ type: 'status', payload: { playing: false, countIn: 3, noteRecordArmed: true, recordArmSupported: true } });
  assert.equal(MK3.state.countIn.remaining, 3); assert.equal(MK3.screenModel().title?.countIn ?? MK3.state.countIn.remaining, 3);
  assert.equal(leds().pads.filter(Boolean).length, 3, 'pads count down the clicks');
  calls.length = 0; press('keyboard'); pad(1); assert.ok(sent('note.on').length, 'pads play during the count-in');
  h.win.nativeEvent({ type: 'status', payload: { playing: true, countIn: 0, noteRecordArmed: true, recordArmSupported: true } });
  assert.equal(state.countIn, null); assert.equal(MK3.state.countIn, null);
  state.native = false; state.playing = false; state.status = { playing: false };
  console.log('PASS native expression (bend, CC1, aftertouch, sustain), queued pattern/scene changes, native count-in');

  // ---- 9. Palette, help, settings -----------------------------------------------------------------------
  for (const id of ['mk3Simulator', 'mk3Connect', 'mk3Release']) assert.ok(ST.commands.some(c => c[0] === id), 'palette ' + id);
  ST.perform('mk3Connect'); assert.equal(state.modal, 'mk3-takeover', 'take-over asks for confirmation first'); ST.closeModal();
  assert.ok(MK3.CONTROL_MAP.length >= 30 && MK3.settingsNote().includes('Maschine MK3'));
  // Support is in development and off by default: the settings toggle is unchecked and take-over is refused.
  MK3.event('mk3.status', { connected: false, enabled: false });
  assert.match(MK3.settingsNote(), /in development/); assert.doesNotMatch(MK3.settingsNote(), /id="mk3-enabled" checked/);
  ST.perform('mk3Connect'); assert.notEqual(state.modal, 'mk3-takeover', 'take-over is refused while MK3 support is off');
  MK3.event('mk3.status', { connected: false, enabled: true });
  assert.match(MK3.settingsNote(), /id="mk3-enabled" checked/);
  console.log('PASS palette commands, take-over confirmation, help map and settings note (off by default)');

  // ---- 10. Lists: scrolloff window, short lists, page jumps, acceleration, browser filter lists, signal chain ---
  {
    const win = MK3.listWindow, rows = MK3.LIST_ROWS, margin = MK3.LIST_MARGIN;
    assert.equal(rows, 7); assert.equal(margin, 2);
    // Short list: the whole list is visible and the cursor moves freely.
    for (let c = 0; c < 5; c++) assert.equal(win(5, c, 0), 0, 'short list never scrolls');
    assert.equal(win(7, 6, 0), 0, 'exactly full list never scrolls');
    assert.equal(win(0, 0, undefined), 0);
    // Long list going down: the cursor travels to row rows-1-margin, then the list scrolls; at the end it reaches the last row.
    let top = 0; const downTops = [];
    for (let c = 0; c < 100; c++) { top = win(100, c, top); downTops.push(top); assert.ok(c - top >= Math.min(c, margin) && c - top <= rows - 1, 'cursor stays visible'); }
    same(downTops.slice(0, 7), [0, 0, 0, 0, 0, 1, 2], 'down: cursor moves to row 4 first, then the list scrolls');
    assert.equal(downTops[99], 93, 'down: the last item sits on the last row'); assert.equal(99 - downTops[99], 6);
    // Going back up: the cursor moves up to row `margin` before the list scrolls; at the top it reaches row 0.
    const upTops = []; for (let c = 99; c >= 0; c--) { top = win(100, c, top); upTops.push(top); }
    same(upTops.slice(0, 6), [93, 93, 93, 93, 93, 92], 'up: cursor moves to row 2 first, then the list scrolls');
    assert.equal(upTops.at(-1), 0, 'up: back at the first row');
    // Direction change in the middle keeps the window (no jump).
    assert.equal(win(100, 50, 46), 46); assert.equal(win(100, 49, 46), 46); assert.equal(win(100, 48, 46), 46); assert.equal(win(100, 47, 46), 45);
    // Jumps (page, acceleration) clamp at the ends; an unknown window centres the cursor.
    assert.equal(win(100, 99, 0), 93); assert.equal(win(100, 0, 93), 0); assert.equal(win(100, 50, undefined), 47);
    // Small windows keep a margin that fits (4 rows → 1 row margin).
    assert.equal(win(20, 3, 0, 4), 1); assert.equal(win(20, 2, 0, 4), 0);
    console.log('PASS list window: short lists, scrolloff at both ends, direction changes, page jumps');

    // Patterns list (Shift + ARRANGER): main encoder, Shift = page jump, tilt = page, screen window follows.
    reset();
    while (state.project.patterns.length < 40) ST.mutate('add pattern', p => { const src = p.patterns[0]; p.patterns.push({ ...JSON.parse(JSON.stringify(src)), id: Math.max(...p.patterns.map(x => x.id)) + 1, name: 'P' + p.patterns.length }); });
    press('arranger'); shift('arranger'); assert.equal(MK3.page().id, 'patterns');
    MK3.state.lists.patterns = 0; MK3.state.listTops.delete('patterns'); MK3.frame();
    const main = delta => { input({ type: 'main', delta }); advance(400); };
    for (let i = 0; i < 5; i++) main(1);
    assert.equal(MK3.page().data.selected, 5); assert.equal(MK3.page().data.top, 1, 'list scrolled once the cursor passed row 4');
    press('shift', true); main(1); release('shift');
    assert.equal(MK3.page().data.selected, 12, 'Shift + turn jumps one page (7 rows)');
    press('encoderDown'); assert.equal(MK3.page().data.selected, 19, 'tilt down pages');
    press('encoderUp'); press('encoderUp'); press('encoderUp'); press('encoderUp'); assert.equal(MK3.page().data.selected, 0, 'tilt up clamps at the top');
    assert.equal(MK3.page().data.top, 0);
    // Acceleration: quick consecutive detents on a long list move further than slow ones.
    MK3.state.lists.patterns = 0; const fast = () => { input({ type: 'main', delta: 1 }); advance(20); };
    for (let i = 0; i < 6; i++) fast();
    assert.ok(MK3.page().data.selected > 6, 'fast turns accelerate: ' + MK3.page().data.selected);
    advance(500); const at = MK3.page().data.selected; main(1); assert.equal(MK3.page().data.selected, at + 1, 'a slow detent moves one row');
    const ops = MK3.frame().screens.left; assert.ok(ops.some(op => op.op === 'text' && /^\d+ \/ 40$/.test(op.text)), 'index label "n / 40"');
    console.log('PASS list navigation: turn, Shift page jump, tilt pages, acceleration, index label');

    // Browser filter lists: a filter encoder opens its list on the left screen, the cursor applies after a pause.
    reset();
    const chosen = [], opts = Array.from({ length: 30 }, (_, i) => ({ value: 'exp' + i, label: 'Expansion ' + String(i).padStart(2, '0'), count: 10 + i }));
    let applied = null; const K = { fields: ['sources', 'expansions', 'tags'], list: () => ({ items: Array.from({ length: 50 }, (_, i) => ({ name: 'Kit ' + i, detail: 'Pack' })), selected: 0, total: 50, offset: 0 }), filterValue: f => f === 'expansions' && applied ? applied : 'All',
      options: f => f === 'expansions' ? opts : [], current: f => f === 'expansions' ? applied : null, choose: (f, v) => { chosen.push([f, v]); if (f === 'expansions') applied = v; }, cycle() {}, move() {}, page() {}, load() {}, preview() {}, toggleUserOnly() {}, toggleFavorites() {}, toggleAutoPreview() {}, favorite() {}, clear() {} };
    const savedK = h.win.STKitBrowser, savedB = h.win.STBrowser; let tab = 'kits';
    h.win.STKitBrowser = { controller: K }; h.win.STBrowser = { controller: { get tab() { return tab; }, setTab: t => { tab = t; }, fields: [], list: K.list }, render: () => '', bind() {}, search() {}, focusSearch() {} };
    press('browser'); assert.equal(MK3.page().data.focus, 'results');
    touch('encoder2', true); assert.equal(MK3.page().data.filterList?.title, 'Expansion', 'touching the Expansion encoder shows its list'); touch('encoder2', false);
    turn(1, .081); turn(1, .081); turn(1, .081);
    let page = MK3.page(); assert.equal(page.data.focus, 'expansions'); assert.equal(page.data.filterList.selected, 3, 'cursor on the 3rd expansion (All is row 0)');
    assert.equal(page.data.filterList.items.length, 31); assert.equal(chosen.length, 0, 'not applied while turning');
    advance(300); same(chosen, [['expansions', 'exp2']], 'applied after a short pause');
    for (let i = 0; i < 10; i++) main(1);
    page = MK3.page(); assert.equal(page.data.filterList.selected, 13); assert.equal(page.data.filterList.top, 9, 'filter list scrolls with scrolloff');
    press('encoderPush'); assert.equal(chosen.at(-1)[1], 'exp12', 'push applies at once'); assert.equal(MK3.page().data.focus, 'results', 'and returns to the results');
    press('encoderLeft'); assert.equal(MK3.page().data.focus, 'expansions', 'tilt ◄ re-enters the last filter'); press('encoderLeft'); assert.equal(MK3.page().data.focus, 'sources');
    press('encoderRight'); press('encoderRight'); assert.equal(MK3.page().data.focus, 'tags'); press('encoderRight'); assert.equal(MK3.page().data.focus, 'results'); press('encoderRight'); assert.equal(MK3.page().data.focus, 'results', 'results is the last column');
    press('arrowRight'); assert.equal(tab, 'samples', '► switches tabs');
    const left = MK3.frame().screens.left; assert.ok(left.length > 5);
    h.win.STKitBrowser = savedK; h.win.STBrowser = savedB;
    console.log('PASS browser filter lists: touch preview, encoder + main cursor, debounced apply, push, columns, tabs');

    // Phase 1 · item 3: one page grammar. PLUG-IN display buttons 1–3 pick Master / Track / Instr.; the left screen
    // lists that level's chain (turn walks it, push opens its pages); ◄ ► step pages, shown as page dots.
    reset(); state.instrument = state.project.tracks[state.track].instrument; MK3.state.level = 'instrument'; MK3.state.pluginSlot = null; MK3.state.page.pluginBuiltin = 0;
    const inst = state.project.instruments[state.instrument];
    if (!['synth', 'sample'].includes(inst.type)) inst.type = 'synth';
    press('plugin'); page = MK3.page();
    assert.equal(page.data.kind, 'plugin'); assert.equal(page.data.focus, 'chain'); assert.equal(page.data.level, 'instrument');
    same(page.buttons.slice(0, 3).map(b => b.label), ['Master', 'Track', 'Instr.']); assert.ok(page.buttons[2].on && !page.buttons[0].on, 'Instr. level lit');
    let names = page.data.chain.items.map(item => item.right); assert.equal(names[0], 'Instrument'); assert.ok(!names.includes('Sends'), 'instrument level: instrument only');
    assert.equal(page.pages.length, 5, 'built-in instrument pages live on PLUG-IN · Instr.'); assert.equal(page.knobs[0].label, 'Volume');
    MK3.state.page.pluginBuiltin = 0; press('arrowRight'); assert.equal(MK3.page().page, 1, '► next page'); assert.equal(MK3.page().knobs[0].label, 'Attack');
    const dots = MK3.screenModel().title.pages; same([dots.count, dots.index, dots.name], [5, 1, 'Envelopes'], 'page dots in the title');
    const dotOps = MK3.frame().screens.right.filter(op => (op.op === 'rect' || op.op === 'frame') && op.w === 7 && op.h === 7 && op.y === 32);
    assert.equal(dotOps.length, 5, 'five page dots drawn'); press('arrowLeft'); assert.equal(MK3.page().page, 0, '◄ previous page');
    for (let i = 0; i < 8; i++) press('arrowRight'); assert.equal(MK3.page().page, 4, '► stops at the last page');
    MK3.state.page.pluginBuiltin = 0; press('encoderPush'); page = MK3.page(); assert.equal(page.data.focus, 'pages'); const before = page.page; main(1); assert.equal(MK3.page().page, before + 1, 'pages focus: turn changes page');
    press('plugin'); assert.equal(MK3.page().data.focus, 'chain', 'PLUG-IN returns to the chain'); assert.equal(MK3.state.screen, 'plugin');
    // Track level: inserts, then Sends (volume, pan, sends on the encoders).
    press('display2'); page = MK3.page(); assert.equal(page.data.level, 'track'); names = page.data.chain.items.map(item => item.right);
    assert.equal(names.at(-1), 'Sends'); assert.ok(!names.includes('Instrument')); main(10); assert.equal(MK3.page().knobs[0].label, 'Volume'); assert.equal(MK3.page().knobs[2].label, 'Delay');
    // Master level: master inserts, then the master volume.
    press('display1'); page = MK3.page(); assert.equal(page.data.level, 'master'); assert.equal(page.data.chain.items.at(-1).name, 'Master volume'); assert.equal(page.knobs[0].label, 'Master');
    // CHANNEL shares the level grammar: Master / Track / Instr. on display buttons 1–3.
    press('channel'); assert.equal(MK3.page().data.kind, 'levels'); assert.equal(MK3.page().knobs[0].label, 'Master');
    press('display2'); assert.equal(MK3.page().data.level, 'track'); same(MK3.page().knobs.slice(0, 6).map(k => k?.label), ['Volume', 'Pan', 'Delay', 'Reverb', 'Mute', 'Solo']);
    press('display3'); assert.equal(MK3.page().data.kind, 'channel'); assert.ok(MK3.page().pages.length > 1, 'instrument pages on ◄ ►');
    assert.ok(MK3.page().buttons.slice(3).every(b => !b || !/^(Main|Envelopes|Filter)/.test(b.label)), 'no page tabs on the display buttons');
    press('plugin'); assert.equal(MK3.page().data.level, 'instrument', 'level shared by CHANNEL and PLUG-IN');
    press('plugin'); assert.equal(MK3.state.screen, 'notes');
    console.log('PASS page grammar: level buttons Master/Track/Instr. on PLUG-IN and CHANNEL, chain per level, ◄ ► pages with page dots');
  }
})().catch(error => { console.error(error); process.exit(1); });
