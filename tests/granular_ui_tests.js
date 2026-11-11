'use strict';
// Sampler Granular panel, live instrument parameters and the sample editor's live playheads: the actual app
// modules with a mocked bridge. Covers the panel (only in Granular mode, leading the Sample page, automatable
// controls in musical units), automation/LFO targets, the live engine.instrumentParameter path for drags, key
// bursts and MK3 knobs, the playhead overlay (poll on/off, drawing per play mode, no waveform redraw per event)
// and the MK3 Granular CHANNEL page.
const vm = require('node:vm'), assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');

function recorder() {
  const ops = [];
  const ctx = new Proxy({ ops }, { get(target, name) {
    if (name in target) return target[name];
    if (name === 'measureText') return text => ({ width: String(text).length * 6 });
    return (...args) => { ops.push([name, ...args]); };
  }, set(target, name, value) { target[name] = value; ops.push(['set', name, value]); return true; } });
  return ctx;
}

function harness() {
  const nodes = new Map(), calls = [], handlers = new Map(), timers = new Map(), frames = []; let clock = 0, nextTimer = 1;
  const inputs = [];
  function element(id) {
    if (!nodes.has(id)) {
      const node = { id, innerHTML: '', value: '', dataset: {}, attributes: {}, style: {}, isConnected: true, clientWidth: 800, clientHeight: 190, width: 0, height: 0,
        classList: { toggle() {}, add() {}, remove() {}, contains() { return false; } }, focus() {}, select() {}, scrollIntoView() {},
        closest() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, setAttribute(name, value) { this.attributes[name] = String(value); },
        getAttribute(name) { return this.attributes[name]; }, click() { return this.onclick?.(); }, contexts: 0 };
      node.getContext = () => { node.contexts++; return (node.context ||= recorder()); };
      nodes.set(id, node);
    }
    return nodes.get(id);
  }
  const document = { body: element('body'), activeElement: null, hidden: false, querySelector: element,
    querySelectorAll: query => query === '[data-inst-param]' ? inputs : [], getElementById: element, createElement: element,
    listeners: {}, addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); } };
  const window = { addEventListener() {}, devicePixelRatio: 1, STHostTransport: hostStub({ post(message) {
    calls.push(message); const handler = handlers.get(message.command);
    Promise.resolve().then(() => handler ? handler(message.payload) : {}).then(result => window.STHostTransport.deliver({ id: message.id, ok: true, result }),
      error => window.STHostTransport.deliver({ id: message.id, ok: false, error: error.message }));
  } }) };
  const context = vm.createContext({ window, document, console, performance: { now: () => clock },
    requestAnimationFrame: fn => { frames.push(fn); return frames.length; },
    setTimeout: (fn, delay = 0) => { const id = nextTimer++; timers.set(id, { fn, at: clock + delay }); return id; },
    clearTimeout: id => timers.delete(id), Date, Map, Set, WeakMap, Math, JSON, Number, String, Object, Array, Promise, Event: class { constructor(type) { this.type = type; } },
    innerWidth: 1280, innerHeight: 800 });
  for (const name of ['fine-control.js', 'routing-project.js']) web.load(context, name);
  web.load(context, 'app.js');
  for (const name of ['modulation.js', 'step-fx.js', 'sampler.js', 'wavetables.js', 'granular.js', 'sampler-playheads.js', 'studio.js']) web.load(context, name);
  const ST = window.ST;
  ST.state.project = ST.normalize({ name: 'Grains', tracks: [{ id: 0, name: 'Track 01', instrument: 0 }],
    instruments: [{ id: 0, name: 'Cloud', type: 'sample', samplePath: '/Samples/Pad.wav', mode: 'granular' }] });
  ST.state.samples[0] = { path: '/Samples/Pad.wav', frames: 96000, sampleRate: 48000, duration: 2, peaks: [] };
  const advance = ms => { clock += ms; for (const [id, timer] of [...timers]) if (timer.at <= clock) { timers.delete(id); timer.fn(); } };
  const animate = () => { for (const fn of frames.splice(0)) fn(); };
  return { ST, window, document, calls, handlers, element, advance, animate, context, inputs, app: () => document.getElementById('#app').innerHTML,
    sent: command => calls.filter(c => c.command === command) };
}
const settle = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const tests = [], test = (name, run) => tests.push({ name, run });

test('Granular panel: only in Granular mode, leads the Sample page, automatable controls in musical units', () => {
  const h = harness(), { ST } = h;
  ST.setView('instrument');
  let html = h.app();
  assert.match(html, /id="inst-group-granular"/);
  assert(html.indexOf('id="inst-group-granular"') < html.indexOf('id="sampler-editor"'), 'the Granular panel leads the Sample page');
  for (const [key, readout] of [['grainPosition', '50%'], ['grainSize', '80 ms'], ['grainDensity', '2 grains'], ['grainSpray', '0 ms'],
    ['grainScan', 'Frozen'], ['grainPitchSpread', 'Off'], ['grainReverse', '0%'], ['grainStereo', '0%']]) {
    assert.match(html, new RegExp(`<input id="param-${key}" data-inst-param="${key}" data-automate="${key}"`), `${key} is an automatable control`);
    assert.match(html, new RegExp(`data-param-readout="${key}"[^>]*>${readout}<`), `${key} reads ${readout}`);
  }
  assert.equal((html.match(/id="param-grainPosition"/g) || []).length, 1, 'Position appears once (not again in the region group)');
  assert.match(html, /id="granular-chip">2 × 80 MS · 25 GRAINS\/S</);
  assert.match(html, /<svg id="granular-map"[^>]*aria-label="Grain map: position 50%/);
  ST.mutate('grains', p => Object.assign(p.instruments[0], { grainScan: -.5, grainPitchSpread: 7, grainDensity: 6, grainSpray: .12 }));
  html = h.app();
  assert.match(html, /data-param-readout="grainScan"[^>]*>−0\.5× back</); assert.match(html, /data-param-readout="grainPitchSpread"[^>]*>±7 st</);
  assert.match(html, /data-param-readout="grainDensity"[^>]*>6 grains</); assert.match(html, /data-param-readout="grainSpray"[^>]*>120 ms</);
  assert.match(html, /class="granular-spray"/); assert.match(html, /class="granular-scan"/);
  ST.mutate('loop', p => { p.instruments[0].mode = 'forward'; });
  assert(!h.app().includes('id="inst-group-granular"'));
});

test('granular automation targets, the LFO "Grain size" destination and defaults for older projects', () => {
  const h = harness(), { ST, window } = h;
  const inst = ST.state.project.instruments[0];
  assert.equal(inst.grainDensity, 2); assert.equal(inst.grainScan, 0); assert.equal(inst.grainSpray, 0);
  for (const target of ['grainDensity', 'grainSpray', 'grainPitchSpread', 'grainScan', 'grainReverse', 'grainStereo']) {
    ST.automate(0, target, target, 1);
    assert(ST.pattern().automation.some(l => l.target === target), `${target} is an automation target`);
    const descriptor = window.STStudio.target(0, target); assert(descriptor?.available, `${target} is available on sampler tracks`);
  }
  assert.match(window.STModulation.render(inst), /<option value="grainSize"[^>]*>Grain size<\/option>/);
});

test('live parameters: drags, key bursts and LFO edits reach sounding voices without republishing the project', async () => {
  const h = harness(), { ST } = h;
  ST.state.native = true;
  // A slider's input event (the bound oninput) sends the value straight to the engine.
  const slider = { dataset: { instParam: 'cutoff' }, value: '800', type: 'range', isConnected: true, addEventListener() {}, setAttribute() {} };
  h.inputs.push(slider);
  ST.setView('instrument'); await settle();
  const before = h.sent('engine.project').length;
  slider.oninput();
  assert.deepEqual({ ...h.sent('engine.instrumentParameter').at(-1).payload }, { instrument: 0, target: 'cutoff', value: 800 });
  // Rapid moves coalesce: one request in flight, then the latest value.
  for (const value of ['900', '1000', '1100']) { slider.value = value; slider.oninput(); }
  assert.equal(h.sent('engine.instrumentParameter').length, 1, 'later values wait for the request in flight');
  await settle();
  assert.equal(h.sent('engine.instrumentParameter').at(-1).payload.value, 1100);
  assert.equal(h.sent('engine.instrumentParameter').length, 2);
  // Units: tune combines semitones and cents, volume and pan are the instrument's own.
  const inst = ST.state.project.instruments[0]; inst.tune = 2.25;
  assert.equal(ST.liveInstParam(0, inst, 'tuneSemi', 5), true); await settle();
  assert.deepEqual({ ...h.sent('engine.instrumentParameter').at(-1).payload }, { instrument: 0, target: 'tune', value: 5.25 });
  ST.liveInstParam(0, inst, 'volume', .5); await settle();
  assert.equal(h.sent('engine.instrumentParameter').at(-1).payload.target, 'instrumentVolume');
  ST.liveInstParam(0, inst, 'grainScan', 9); await settle();
  assert.deepEqual({ ...h.sent('engine.instrumentParameter').at(-1).payload }, { instrument: 0, target: 'grainScan', value: 4 }, 'clamped to the engine range');
  assert.equal(ST.liveInstParam(0, inst, 'rootNote', 61), false, 'structural parameters apply on commit');
  assert.equal(h.sent('engine.project').length, before, 'no project publication while dragging');
  // The commit is still one ordinary undoable edit.
  const undo = ST.state.undo.length; slider.value = '1100'; slider.onchange({});
  assert.equal(ST.state.project.instruments[0].cutoff, 1100); assert.equal(ST.state.undo.length, undo + 1);
  // Not native (web preview): nothing is sent.
  ST.state.native = false; assert.equal(ST.liveInstParam(0, inst, 'cutoff', 500), false);
});

test('playheads: the editor polls only while visible and draws each voice on the overlay, never redrawing the waveform', async () => {
  const h = harness(), { ST, window } = h;
  ST.state.native = true; ST.setView('instrument'); await settle();
  assert.deepEqual({ ...h.sent('sampler.playheads').at(-1).payload }, { enabled: true, instrument: 0 });
  const requests = h.sent('sampler.playheads').length;
  window.STSamplerPlayheads.sync(); assert.equal(h.sent('sampler.playheads').length, requests, 'unchanged visibility sends nothing');
  const wave = h.element('sampler-wave-canvas'), overlay = h.element('sampler-playhead-canvas');
  const waveContexts = wave.contexts;
  // A forward-loop voice at the middle of the sample, a released backward voice and other instruments' voices.
  window.nativeEvent({ type: 'sampler.playheads', payload: { instrument: 0, voices: [
    { instrument: 0, track: 0, note: 60, mode: 1, direction: 1, position: 48000, windowStart: 48000, windowEnd: 48000, frames: 96000, level: 1, grains: [] },
    { instrument: 0, track: 0, note: 64, mode: 2, direction: -1, position: 24000, windowStart: 24000, windowEnd: 24000, frames: 96000, level: .5, releasing: true, grains: [] },
    { instrument: 3, track: 1, note: 60, mode: 1, direction: 1, position: 0, frames: 96000, level: 1, grains: [] },
    { instrument: 0, track: 0, note: 72, mode: 1, direction: 1, position: 10, frames: 500, level: 1, zone: true, grains: [] }] } });
  h.animate();
  assert.equal(window.STSamplerPlayheads.voices.length, 2, 'only this instrument and its own sample');
  let ops = overlay.context.ops; const lines = ops.filter(op => op[0] === 'moveTo').map(op => op[1]);
  assert(lines.includes(400.5), 'the forward voice is drawn at the middle (800 px wide view)'); assert(lines.includes(200.5));
  const arrows = ops.filter(op => op[0] === 'lineTo' && op[2] === 2).map(op => op[1]);
  assert(arrows.includes(400) && arrows.includes(200), 'direction arrows');
  assert.equal(ops.filter(op => op[0] === 'moveTo' && op[2] === 7).length, 2, 'two arrow tips');
  assert(ops.some(op => op[0] === 'moveTo' && op[1] === 407) && ops.some(op => op[0] === 'moveTo' && op[1] === 193), 'forward points right, backward left');
  assert.equal(wave.contexts, waveContexts, 'the waveform canvas is not redrawn for playheads');
  // Wavetable: the current frame region; Granular: the cloud, every grain and the scan position.
  overlay.context.ops.length = 0;
  window.nativeEvent({ type: 'sampler.playheads', payload: { instrument: 0, voices: [
    { instrument: 0, track: 0, note: 60, mode: 5, direction: 1, position: 12000, windowStart: 12000, windowEnd: 24000, frames: 96000, level: 1, grains: [] }] } });
  h.animate(); ops = overlay.context.ops;
  assert(ops.some(op => op[0] === 'strokeRect' && op[1] === 100.5 && op[3] === 99), 'wavetable frame from 12000 to 24000 frames');
  overlay.context.ops.length = 0;
  window.nativeEvent({ type: 'sampler.playheads', payload: { instrument: 0, voices: [
    { instrument: 0, track: 0, note: 60, mode: 6, direction: 1, position: 48000, windowStart: 46000, windowEnd: 50000, frames: 96000, level: 1,
      grains: [[46500, .2], [48000, 1], [49500, .6]] }] } });
  h.animate(); ops = overlay.context.ops;
  const grainTicks = ops.filter(op => op[0] === 'fillRect' && op[3] === 3).map(op => op[1]);
  assert.deepEqual([...grainTicks], [387, 399, 412], 'one tick per grain at its read position');
  assert(ops.some(op => op[0] === 'fillRect' && Math.abs(op[1] - 383.33) < .01), 'the grain cloud');
  assert(ops.some(op => op[0] === 'moveTo' && op[1] === 400.5), 'the scan position');
  // Silence clears the overlay.
  overlay.context.ops.length = 0;
  window.nativeEvent({ type: 'sampler.playheads', payload: { instrument: 0, voices: [] } }); h.animate();
  assert(overlay.context.ops.some(op => op[0] === 'clearRect') && !overlay.context.ops.some(op => op[0] === 'fillRect'));
  // Leaving the instrument view stops the poll.
  ST.setView('pattern'); h.advance(450);
  assert.deepEqual({ ...h.sent('sampler.playheads').at(-1).payload }, { enabled: false });
  window.nativeEvent({ type: 'sampler.playheads', payload: { instrument: 0, voices: [{ instrument: 0, mode: 1, position: 1, frames: 10, level: 1 }] } });
  assert.equal(window.STSamplerPlayheads.voices.length, 0, 'late events after hiding are ignored');
});

test('playheads: the poll stops while the window is hidden and restarts when it is shown again', async () => {
  const h = harness(), { ST, document } = h, fire = () => { for (const fn of document.listeners.visibilitychange || []) fn(); };
  ST.state.native = true; ST.setView('instrument'); await settle();
  assert.deepEqual({ ...h.sent('sampler.playheads').at(-1).payload }, { enabled: true, instrument: 0 });
  document.hidden = true; fire();
  assert.deepEqual({ ...h.sent('sampler.playheads').at(-1).payload }, { enabled: false }, 'a hidden window stops the poll');
  // Nothing renders when the window returns, so the visibility event itself must restart the poll.
  document.hidden = false; fire();
  assert.deepEqual({ ...h.sent('sampler.playheads').at(-1).payload }, { enabled: true, instrument: 0 }, 'showing the window restarts the poll');
  assert.equal(h.window.STSamplerPlayheads.watching, 0);
  const requests = h.sent('sampler.playheads').length; fire();
  assert.equal(h.sent('sampler.playheads').length, requests, 'an unchanged visibility sends nothing');
});

test('MK3: Granular CHANNEL page with Position, Size, Density, Spray, Scan, Pitch, Reverse and Stereo knobs', async () => {
  const { session } = require('./helpers/mk3-session');
  const h = session();
  Object.assign(h.state.project.instruments[h.state.instrument], { type: 'sample', mode: 'granular', samplePath: '/x.wav', grainPosition: .25, grainDensity: 4, grainScan: 1 });
  h.press('channel');
  const pages = h.MK3.page().pages; assert.equal(pages.at(-1), 'Granular'); assert.equal(pages.at(-2), 'Play mode');
  h.MK3.state.page.channel = pages.length - 1;
  const page = h.MK3.page();
  assert.deepEqual([...page.knobs.map(k => k?.label)], ['Position', 'Grain size', 'Density', 'Spray', 'Scan', 'Pitch rnd', 'Reverse', 'Stereo']);
  assert.equal(page.knobs[0].value, '25%'); assert.equal(page.knobs[2].value, '4 grains'); assert.equal(page.knobs[4].value, '1×');
  // A knob turn is heard live (engine.instrumentParameter) and commits as one edit.
  h.state.native = true;
  page.knobs[4].turn(.1); h.advance(100); await Promise.resolve();
  const live = h.calls.filter(c => c.command === 'engine.instrumentParameter');
  assert(live.length && live.at(-1).payload.target === 'grainScan', 'the scan knob is heard live');
  await h.MK3.flushEdits();
  assert(h.state.project.instruments[h.state.instrument].grainScan > 1);
  Object.assign(h.state.project.instruments[h.state.instrument], { mode: 'oneshot' });
  assert(!h.MK3.page().pages.includes('Granular'));
});

test('agent API: granular parameters by name with units', async () => {
  const h = harness(), { window } = h; web.load(h.context, 'agent-api.js');
  const created = await window.STAgent.call('createInstrument', { name: 'Cloud', params: { mode: 'granular', grainDensity: 6, grainScan: '0.5', grainPitchSpread: '7 st', grainReverse: '25%' } });
  assert.equal(created.ok, true, JSON.stringify(created));
  const inst = h.ST.state.project.instruments[created.result.instrument];
  assert.equal(inst.grainDensity, 6); assert.equal(inst.grainScan, .5); assert.equal(inst.grainPitchSpread, 7); assert.equal(inst.grainReverse, .25);
});

(async () => {
  let failed = 0;
  for (const { name, run } of tests) {
    try { await run(); console.log('PASS ' + name); } catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); }
  }
  console.log(`Granular UI: ${tests.length - failed}/${tests.length} groups passed.`);
  process.exitCode = failed ? 1 : 0;
})();
