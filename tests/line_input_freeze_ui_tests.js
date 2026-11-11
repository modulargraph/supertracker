'use strict';
// Track Freeze for hardware tracks: a live-input track freezes by recording, with the request the host expects.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const project = { tracks: [{ routingID: 'one', name: 'Live', instrument: 0, audioInput: { enabled: true, deviceUID: 'saved-interface', channels: [2, 3], gain: 1 } }, { routingID: 'two', name: 'Synth', instrument: 0 }], patterns: [], instruments: [{}], sequence: [] };
const messages = [], modals = [];
const window = { ST: { state: { project, track: 0 }, toast: message => messages.push(message), openModal: (...args) => modals.push(args) }, addEventListener() {} };
web.run('freeze.js', { window, document: { addEventListener() {}, getElementById() { return null; } }, console });
window.STFreeze.freeze(0);
assert.equal(modals.length, 1, 'a live-input (hardware) track opens the freeze-by-recording dialog');
assert.match(modals[0][1], /frozen by <em>recording<\/em>/); assert.match(modals[0][2], /Record and freeze/);
window.STFreeze.freeze(1);
assert.equal(modals.length, 2, 'an unrelated live-input lane does not block the target freeze dialog');
assert.doesNotMatch(modals[1][1], /External hardware/, 'a software track freezes offline');
project.tracks[0].audioInput.enabled = false;
window.STFreeze.freeze(0); assert.equal(modals.length, 3, 'a disabled saved mapping permits offline freeze');
assert.doesNotMatch(modals[2][1], /External hardware/);
project.tracks[0].frozen = { id: 'print' };
const before = copy(project), changed = copy(project);
changed.tracks[0].audioInput.enabled = true;
assert(window.STFreeze.checkMutation(before, changed, 'enable input'), 'frozen track protects its input source configuration');
console.log('PASS live-input freeze by recording, disabled/unrelated routes, and frozen source mutation protection');

// The dialog with its controls: what "Record and freeze" asks the host for, and what it does with the answer.
(async () => {
  const nodes = new Map(), calls = [], toasts = [], progress = [];
  const node = id => { if (!nodes.has(id)) nodes.set(id, { id, value: '', checked: false, hidden: false, disabled: false, textContent: '', focus() {} }); return nodes.get(id); };
  const live = { tracks: [{ routingID: 'hw', name: 'Hardware', instrument: 0, audioInput: { enabled: true, deviceUID: 'interface', channels: [0, 1], gain: 1 } }],
    patterns: [{ id: 0, length: 16, steps: [[{ note: 60, instrument: 0 }]] }], instruments: [{}], sequence: [0], bpm: 120, linesPerBeat: 4, audioPlugins: [] };
  const record = { version: 1, id: 'rec-1', path: '/Freezes/hardware-song.wav', method: 'recorded', patternPrints: [], unprintedPatterns: [0] };
  const state = { project: live, track: 0, modal: null, playing: false, native: true };
  const ST = { state, escapeHTML: String, toast: (text, kind) => toasts.push({ text, kind }), perform() {}, waitForEngineUpdate: async () => {},
    openModal(title, body, footer, options) { state.modal = options.kind; node('st-freeze-tail').value = '10'; node('st-freeze-auto').checked = true; node('st-freeze-patterns').checked = true; node('st-freeze-monitor').checked = true; },
    closeModal() { state.modal = null; },
    mutate(label, fn, options) { assert.equal(options.freezeCommit, true); fn(state.project); return true; },
    request: async () => ({ tracks: [] }),
    async longRequest(command, payload) {
      calls.push({ command, payload: copy(payload) });
      window2.STFreeze.event('freeze.progress', { track: 0, progress: 0.42, phase: 'song', recording: true }); progress.push(node('st-freeze-status').textContent);
      window2.STFreeze.event('freeze.progress', { track: 0, progress: 0.9, phase: 'patterns', recording: true }); progress.push(node('st-freeze-status').textContent);
      return { track: 0, record, audioPlugins: [], releasedPlugins: 0, unprintedPatterns: payload.patterns ? [] : [0], notes: [] };
    } };
  const window2 = { ST, addEventListener() {} };
  web.run('freeze.js', { window: window2, document: { addEventListener() {}, getElementById: node, querySelectorAll: () => [] }, console, setInterval: () => 1, clearInterval() {}, setTimeout: () => 1, clearTimeout() {} });
  window2.STFreeze.freeze(0);
  assert.equal(state.modal, 'freeze');
  // The defaults: an automatic tail of at most ten seconds, pattern prints, the mix heard.
  await node('st-freeze-go').onclick();
  assert.deepEqual(calls[0], { command: 'freeze.track', payload: { track: 0, maxTail: 10, method: 'record', patterns: true, monitor: true } });
  assert.deepEqual(progress, ['Recording song · 42% · press Stop to cancel', 'Recording patterns · 90% · press Stop to cancel']);
  assert.deepEqual(copy(live.tracks[0].frozen), record, 'the record the host answered with is what the track stores');
  assert.equal(state.modal, null); assert.match(toasts.at(-1).text, /Hardware frozen/);
  assert.equal(window2.STFreeze.isBusy, false);
  // A fixed tail, the song only, the mix silent: the page says that the patterns stay silent.
  delete live.tracks[0].frozen; window2.STFreeze.freeze(0);
  node('st-freeze-auto').checked = false; node('st-freeze-tail').value = '2.5'; node('st-freeze-patterns').checked = false; node('st-freeze-monitor').checked = false;
  await node('st-freeze-go').onclick();
  assert.deepEqual(calls[1].payload, { track: 0, tail: 2.5, method: 'record', patterns: false, monitor: false });
  assert.match(toasts.at(-1).text, /1 pattern was not printed and stay silent in Pattern mode/); assert.equal(toasts.at(-1).kind, 'error');
  // What the host refuses is shown in the dialog, which stays open for another try.
  delete live.tracks[0].frozen; window2.STFreeze.freeze(0);
  ST.longRequest = async () => { throw new Error('Playback stopped before the end of the song. Nothing was changed.'); };
  await node('st-freeze-go').onclick();
  assert.equal(node('st-freeze-status').textContent, 'Playback stopped before the end of the song. Nothing was changed.');
  assert.equal(state.modal, 'freeze'); assert.equal(live.tracks[0].frozen, undefined); assert.equal(node('st-freeze-go').disabled, false);
  // While the song plays the dialog asks for Stop first, and sends nothing.
  state.playing = true; const sent = calls.length;
  await node('st-freeze-go').onclick();
  assert.equal(calls.length, sent); assert.equal(node('st-freeze-status').textContent, 'Stop playback before freezing.');
  console.log('PASS Record and freeze sends method, patterns, monitor and the tail; shows recording progress; stores the record; shows a refusal');
})().catch(error => { console.error(error); process.exitCode = 1; });
