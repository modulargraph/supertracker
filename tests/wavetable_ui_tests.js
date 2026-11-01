'use strict';
// Sampler Wavetable UI and the retired built-in synth: the actual app modules with a mocked bridge.
// Covers new instruments (empty sampler, "+ Wavetable"), the Wavetable panel (Position, frame size,
// table picker from sample.wavetables, frame preview), the POS step effect, the wavetablePosition
// automation target, the LFO "Position" label, migrated projects and the MK3 Position knob.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict'), path = require('node:path');
const web = require('./helpers/web');

const TABLES = [
  { group: 'Basic', name: 'Sine', path: 'builtin:wavetables/Basic/Sine.wav', frames: 1, frameSize: 2048, wavetable: true },
  { group: 'Basic', name: 'Saw', path: 'builtin:wavetables/Basic/Saw.wav', frames: 1, frameSize: 2048, wavetable: true },
  { group: 'Basic', name: 'Noise', path: 'builtin:wavetables/Basic/Noise.wav', frames: 0, frameSize: 0, wavetable: false },
  { group: 'Morph', name: 'Vowels', path: 'builtin:wavetables/Morph/Vowels.wav', frames: 32, frameSize: 2048, wavetable: true }
];

function harness() {
  const nodes = new Map(), calls = [], handlers = new Map(), timers = new Map(); let clock = 0, nextTimer = 1;
  function element(id) {
    if (!nodes.has(id)) nodes.set(id, { id, innerHTML: '', value: '', dataset: {}, attributes: {}, style: {}, isConnected: true,
      classList: { toggle() {}, add() {}, remove() {}, contains() { return false; } }, focus() {}, select() {}, scrollIntoView() {},
      closest() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, setAttribute(name, value) { this.attributes[name] = String(value); },
      getAttribute(name) { return this.attributes[name]; }, click() { return this.onclick?.(); } });
    return nodes.get(id);
  }
  const document = { body: element('body'), activeElement: null, querySelector: element, querySelectorAll: () => [], getElementById: element,
    addEventListener() {}, createElement: element };
  const window = { addEventListener() {}, webkit: { messageHandlers: { supertracker: { postMessage(message) {
    calls.push(message); const handler = handlers.get(message.command);
    Promise.resolve().then(() => handler ? handler(message.payload) : {}).then(result => window.nativeReceive({ id: message.id, ok: true, result }),
      error => window.nativeReceive({ id: message.id, ok: false, error: error.message }));
  } } } } };
  const context = vm.createContext({ window, document, console, setTimeout: (fn, delay = 0) => { const id = nextTimer++; timers.set(id, { fn, at: clock + delay }); return id; },
    clearTimeout: id => timers.delete(id), Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, innerWidth: 1280, innerHeight: 800 });
  for (const name of ['fine-control.js', 'routing-project.js', 'project-timing.js']) web.load(context, name);
  web.load(context, 'app.js');
  for (const name of ['modulation.js', 'step-fx.js', 'sampler.js', 'wavetables.js', 'studio.js']) web.load(context, name);
  const ST = window.ST;
  ST.state.project = ST.normalize({ name: 'Wavetables', tracks: [{ id: 0, name: 'Track 01', instrument: 0 }], instruments: [{ id: 0, name: 'Empty', type: 'sample' }] });
  handlers.set('sample.wavetables', () => TABLES);
  handlers.set('sample.peaks', payload => ({ min: Array.from({ length: payload.bins }, (_, i) => Math.sin(i / payload.bins * Math.PI * 2) - .01),
    max: Array.from({ length: payload.bins }, (_, i) => Math.sin(i / payload.bins * Math.PI * 2) + .01) }));
  const advance = ms => { clock += ms; for (const [id, timer] of [...timers]) if (timer.at <= clock) { timers.delete(id); timer.fn(); } };
  return { ST, window, document, calls, handlers, element, advance, context, app: () => document.getElementById('#app').innerHTML,
    toast: () => element('#toast-region').innerHTML };
}
const settle = async () => { for (let i = 0; i < 40; i++) await Promise.resolve(); };
const tests = [], test = (name, run) => tests.push({ name, run });

test('new instruments are empty samplers and there is no synth type or 64-instrument cap', async () => {
  const h = harness(), { ST } = h;
  const blank = ST.instrumentDefaults(3, 'New');
  assert.equal(blank.type, 'sample'); assert.equal(blank.samplePath, ''); assert.equal(blank.waveform, undefined);
  assert.equal(blank.wavetablePosition, 0); assert.equal(blank.wavetableFrameSize, 0);
  while (ST.state.project.instruments.length < 64) ST.state.project.instruments.push(ST.instrumentDefaults(ST.state.project.instruments.length, 'Filler'));
  ST.perform('addInstrument');
  assert.equal(ST.state.project.instruments.length, 65); assert.equal(ST.state.project.instruments[64].type, 'sample');
  assert.equal(ST.state.project.instruments[64].samplePath, '');
  assert(!ST.commands.some(([id]) => id === 'instrumentType:synth')); assert(!ST.commands.some(([id]) => id === 'timing'));
  const before = JSON.stringify(ST.state.project);
  assert.equal(await ST.perform('instrumentType:synth'), false); assert.equal(JSON.stringify(ST.state.project), before);
  ST.setView('instrument');
  assert(!/instrument-type-synth|inst-oscillator|inst-waveform/.test(h.app()));
  assert.match(h.app(), /id="instrument-type-sample"[^>]*>Sampler</);
});

test('"+ Wavetable" adds one undoable sampler in Wavetable mode on the built-in Saw table', () => {
  const h = harness(), { ST } = h, undo = ST.state.undo.length;
  assert(ST.commands.some(([id, label]) => id === 'addWavetable' && /wavetable synth/i.test(label)));
  ST.setView('pattern'); ST.setView('instrument');
  assert.match(h.app(), /id="add-wavetable-instrument"[^>]*>\+ Wavetable</);
  ST.perform('addWavetable');
  const p = ST.state.project, inst = p.instruments.at(-1);
  assert.equal(inst.type, 'sample'); assert.equal(inst.mode, 'wavetable'); assert.equal(inst.samplePath, 'builtin:wavetables/Basic/Saw.wav');
  assert.equal(inst.wavetablePosition, 0); assert.equal(p.tracks.at(-1).instrument, p.instruments.length - 1);
  assert.equal(ST.state.instrument, p.instruments.length - 1); assert.equal(ST.state.view, 'instrument');
  assert.equal(ST.state.undo.length, undo + 1); assert.match(ST.state.undo.at(-1).label, /wavetable/);
  assert.match(h.app(), /wavetable · Saw/, 'the sidebar names the table');
});

test('Wavetable panel: big Position control, frame size, grouped table picker by name and frame preview', async () => {
  const h = harness(), { ST, window } = h;
  ST.perform('addWavetable'); await settle();
  assert.equal(h.element('wavetable-table').innerHTML.includes('Vowels'), true, 'the open picker is repainted when the catalogue arrives');
  ST.refresh(); let html = h.app();
  assert.match(html, /id="inst-group-wavetable"/);
  assert(html.indexOf('id="inst-group-wavetable"') < html.indexOf('id="sampler-editor"'), 'the Wavetable panel leads the Sample page');
  assert.match(html, /<input id="param-wavetablePosition" data-inst-param="wavetablePosition" data-automate="wavetablePosition"[^>]*type="range"[^>]*aria-valuetext="0%"/);
  assert.match(html, /data-param-readout="wavetablePosition"[^>]*>0%</);
  const frameSizes = [.../<select id="wavetable-frame-size"[^>]*>([\s\S]*?)<\/select>/.exec(html)[1].matchAll(/value="(\d+)"/g)].map(m => Number(m[1]));
  assert.deepEqual(frameSizes, [0, 256, 512, 1024, 2048, 4096, 8192]);
  const picker = /<select id="wavetable-table"[^>]*>([\s\S]*?)<\/select>/.exec(html)[1];
  assert.match(picker, /<optgroup label="Basic"><option value="builtin:wavetables\/Basic\/Sine.wav" >Sine<\/option><option value="builtin:wavetables\/Basic\/Saw.wav" selected>Saw<\/option><\/optgroup>/);
  assert.match(picker, /<optgroup label="Morph"><option value="builtin:wavetables\/Morph\/Vowels.wav" >Vowels<\/option><\/optgroup>/);
  assert(!picker.includes('Noise'), 'loops that are not wavetables stay out of the picker'); assert.match(picker, /Load a wavetable file…/);
  assert.equal(h.calls.filter(c => c.command === 'sample.wavetables').length, 1, 'the catalogue is requested once and cached');
  assert.match(html, /chip" id="wavetable-table-chip">SAW</);

  // Choosing a table, a frame size and moving Position are ordinary undoable instrument edits.
  const inst = ST.state.project.instruments.at(-1), select = h.element('wavetable-table'), size = h.element('wavetable-frame-size');
  window.STWavetables.bind();
  select.value = 'builtin:wavetables/Morph/Vowels.wav'; select.onchange();
  assert.equal(ST.state.project.instruments.at(-1).samplePath, 'builtin:wavetables/Morph/Vowels.wav'); assert.equal(ST.state.undo.at(-1).label, 'wavetable table');
  window.STWavetables.bind(); size.value = '1024'; size.onchange();
  assert.equal(ST.state.project.instruments.at(-1).wavetableFrameSize, 1024);
  window.STWavetables.bind(); size.value = '0'; size.onchange(); assert.equal(ST.state.project.instruments.at(-1).wavetableFrameSize, 0);
  ST.mutate('position', p => { p.instruments.at(-1).wavetablePosition = .5; });
  assert.match(h.app(), /data-param-readout="wavetablePosition"[^>]*>50%</);
  assert.ok(inst);

  // Preview: the current frame of a 32-frame table is fetched with sample.peaks and drawn as a line.
  ST.state.native = true; const index = ST.state.project.instruments.length - 1;
  ST.state.samples[index] = { path: 'builtin:wavetables/Morph/Vowels.wav', frames: 32 * 2048, wavetableFrameSize: 2048, peaks: [] };
  const info = window.STWavetables.frameInfo(ST.state.project.instruments[index], ST.state.samples[index]);
  assert.equal(info.count, 32); assert.equal(info.index, 16);
  window.STWavetables.bind(); h.advance(50); await settle();
  const peaks = h.calls.filter(c => c.command === 'sample.peaks').at(-1);
  assert(peaks, 'frame preview requested'); assert.equal(peaks.payload.instrument, index); assert.equal(peaks.payload.bins, 128);
  assert.equal(peaks.payload.start, 16 / 32); assert.equal(peaks.payload.end, 17 / 32);
  assert.match(h.element('wavetable-preview-line').attributes.d, /^M0\.0 40\.0L/);
  assert.equal(h.element('wavetable-frame').textContent, 'Frame 17 / 32');
});

test('the panel only shows in Wavetable mode; a user sample appears as "Current sample"; empty samplers offer a built-in table', () => {
  const h = harness(), { ST, window } = h;
  ST.setView('instrument');
  let html = h.app();
  assert(!html.includes('id="inst-group-wavetable"')); assert.match(html, /id="sampler-use-wavetable"/);
  ST.mutate('own sample', p => { Object.assign(p.instruments[0], { samplePath: '/Samples/Cycle.wav', mode: 'wavetable' }); });
  html = h.app();
  assert.match(html, /<option value="" selected>Current sample · Cycle\.wav<\/option>/); assert(!html.includes('id="sampler-use-wavetable"'));
  ST.mutate('loop', p => { p.instruments[0].mode = 'forward'; });
  assert(!h.app().includes('id="inst-group-wavetable"'));
  ST.mutate('empty', p => { Object.assign(p.instruments[0], { samplePath: '', mode: 'oneshot' }); });
  window.STSampler.bind();
  assert.equal(window.STWavetables.chooseTable(ST.state.project.instruments[0], window.STWavetables.pathFor('Saw')) !== false, true);
  assert.equal(ST.state.project.instruments[0].mode, 'wavetable'); assert.equal(ST.state.project.instruments[0].samplePath, 'builtin:wavetables/Basic/Saw.wav');
  assert.equal(window.STWavetables.displayName('builtin:wavetables/Morph/Sync Sweep.wav'), 'Sync Sweep'); assert.equal(window.STWavetables.displayName('/x/Saw.wav'), '');
});

test('POS step effect, wavetablePosition automation target and the LFO Position destination', () => {
  const h = harness(), { ST, window } = h;
  assert.deepEqual(Array.from(ST.FX.position), [0, 1, .01]);
  ST.mutate('fx', p => { p.patterns[0].steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 1, fx1: { type: 'position', value: .5 }, fx2: { type: 'position', value: 1 } }; });
  ST.setView('pattern');
  const html = h.app(); assert.match(html, />W50</); assert.match(html, />W100</);
  ST.perform('addWavetable');
  const index = ST.state.track;
  ST.automate(index, 'wavetablePosition', 'Wavetable position', .25);
  const lane = ST.pattern().automation.find(l => l.target === 'wavetablePosition');
  assert(lane, 'wavetablePosition is an automation target'); assert.equal(lane.track, index);
  const mod = window.STModulation.render(ST.state.project.instruments.at(-1));
  assert.match(mod, /<option value="position"[^>]*>Position<\/option>/);
});

test('project.open of a migrated project marks it unsaved and says what was converted', async () => {
  const h = harness(), { ST } = h;
  const project = ST.normalize({ name: 'Old synths', tracks: [{ id: 0, name: 'Bass', instrument: 0 }, { id: 1, name: 'Lead', instrument: 1 }],
    instruments: [{ id: 0, name: 'Bass', type: 'sample', mode: 'wavetable', samplePath: 'builtin:wavetables/Basic/Saw.wav', wavetablePosition: 0 },
      { id: 1, name: 'Lead', type: 'sample', mode: 'wavetable', samplePath: 'builtin:wavetables/Basic/Square.wav', wavetablePosition: 0 }] });
  h.handlers.set('project.open', () => ({ project, path: '/Old.supertracker', migrated: true, dirty: true }));
  ST.state.dirty = false;
  await ST.projectAction('open', '/Old.supertracker'); await settle();
  assert.equal(ST.state.project.name, 'Old synths'); assert.equal(ST.state.dirty, true);
  assert.match(h.toast(), /Converted 2 synth instruments to wavetable samplers/);
  h.handlers.set('project.open', () => ({ project: { ...project, name: 'Current' }, path: '/New.supertracker', migrated: false, dirty: false }));
  ST.state.dirty = false; h.element('#toast-region').innerHTML = ''; await ST.projectAction('open', '/New.supertracker'); await settle();
  assert.equal(ST.state.dirty, false); assert.doesNotMatch(h.toast(), /Converted/);
});

test('projects that never passed the native migration still load synths as wavetable samplers', () => {
  const h = harness(), p = h.ST.normalize({ instruments: [{ id: 0, type: 'synth', name: 'Old', waveform: 'square' }, { id: 1, type: 'synth', waveform: 'noise' },
    { id: 2, type: 'sample', mode: 'wavetable', samplePath: '/t.wav', grainPosition: .3 }] });
  assert.equal(p.instruments[0].type, 'sample'); assert.equal(p.instruments[0].mode, 'wavetable'); assert.equal(p.instruments[0].samplePath, 'builtin:wavetables/Basic/Square.wav');
  assert.equal(p.instruments[0].waveform, undefined);
  assert.equal(p.instruments[1].mode, 'forward'); assert.equal(p.instruments[1].samplePath, 'builtin:wavetables/Basic/Noise.wav');
  assert.equal(p.instruments[2].wavetablePosition, .3);
  assert(h.ST.demoProject().instruments.every(inst => inst.type === 'sample' && inst.samplePath.startsWith('builtin:wavetables/Basic/')));
});

test('agent API: sample/midi types, wavetable parameters and the "position" step effect', async () => {
  const h = harness(), { window } = h; web.load(h.context, 'agent-api.js');
  const api = window.STAgent;
  const created = await api.call('createInstrument', { name: 'WT', params: { mode: 'wavetable', position: '40%', wavetableFrameSize: 512 } });
  assert.equal(created.ok, true, JSON.stringify(created)); const inst = h.ST.state.project.instruments[created.result.instrument];
  assert.equal(inst.type, 'sample'); assert.equal(inst.wavetablePosition, .4); assert.equal(inst.wavetableFrameSize, 512);
  assert.equal(inst.samplePath, 'builtin:wavetables/Basic/Saw.wav');
  const refused = await api.call('createInstrument', { type: 'synth' }); assert.equal(refused.ok, false); assert.match(refused.error, /"sample" or "midi"/);
  const written = await api.call('writeSteps', { steps: [{ track: 0, row: 2, note: 'C-4', fx1: 'position 75%' }] }); assert.equal(written.ok, true, written.error);
  assert.deepEqual({ ...h.ST.pattern().steps[0][2].fx1 }, { type: 'position', value: .75 });
});

test('MK3 sampler pages show Position as a knob in Wavetable mode', () => {
  const { session } = require('./helpers/mk3-session');
  const h = session();
  Object.assign(h.state.project.instruments[h.state.instrument], { type: 'sample', mode: 'wavetable', samplePath: 'builtin:wavetables/Basic/Saw.wav', wavetablePosition: .25 });
  h.press('channel');
  const pages = h.MK3.page().pages; assert.equal(pages.at(-1), 'Wavetable');
  h.MK3.state.page.channel = pages.length - 1;
  const page = h.MK3.page(), knob = page.knobs[0];
  assert.equal(knob.label, 'Position'); assert.equal(knob.value, '25%');
  assert(page.knobs.some(k => k?.label === 'Frame size'));
  Object.assign(h.state.project.instruments[h.state.instrument], { mode: 'oneshot' });
  assert(!h.MK3.page().pages.includes('Wavetable'));
});

(async () => {
  let failed = 0;
  for (const { name, run } of tests) {
    try { await run(); console.log('PASS ' + name); } catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); }
  }
  console.log(`Wavetable UI: ${tests.length - failed}/${tests.length} groups passed.`);
  process.exitCode = failed ? 1 : 0;
})();
