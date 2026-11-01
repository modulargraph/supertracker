'use strict';
// Live automation recording (Read/Touch/Latch/Write). Run: node tests/automation_record_tests.js
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = process.env.AUTOMATION_RECORD_SOURCE ? fs.readFileSync(process.env.AUTOMATION_RECORD_SOURCE, 'utf8') : web.source('automation-record.js');
const fineSource = web.source('fine-control.js');
const copy = value => JSON.parse(JSON.stringify(value));

function harness({ lanes = [], length = 64, fine = false } = {}) {
  let clock = 1_000_000, timerID = 0;
  const timers = new Map(), listeners = new Map(), mutations = [], syncs = [], toasts = [], storage = new Map();
  const document = {
    readyState: 'complete', activeElement: null,
    addEventListener(type, fn) { const list = listeners.get(type) || []; list.push(fn); listeners.set(type, list); },
    getElementById: () => null, querySelector: () => null, querySelectorAll: () => []
  };
  const state = { view: 'mixer', track: 0, instrument: 0, playing: false, status: {}, lastStatusAt: clock, project: {
    bpm: 120, linesPerBeat: 4, activePattern: 3,
    tracks: [{ name: 'Bass', instrument: 0, volume: .8, routingID: 'a' }, { name: 'Keys', instrument: 1, volume: .5, routingID: 'b' }],
    instruments: [{ type: 'synth', cutoff: 1000 }, { type: 'synth' }], audioPlugins: [],
    patterns: [{ id: 3, name: 'A', length, automation: copy(lanes) }, { id: 4, name: 'B', length: 32, automation: [] }]
  }, undo: [] };
  const descriptors = {
    volume: { target: 'volume', name: 'Track volume', min: 0, max: 1.5, step: .01, value: .8, format: v => `${v}` },
    cutoff: { target: 'cutoff', name: 'Filter cutoff', min: 20, max: 20000, step: 1, value: 1000, log: true, format: v => `${v} Hz` },
    'plugin:instrument:0:7': { target: 'plugin:instrument:0:7', name: 'Macro', min: 0, max: 1, step: .001, value: .25, format: v => `${v}` },
    lfoRate: { target: 'lfoRate', name: 'LFO 1 rate', min: 0, max: 100, step: .01, value: 1.25, fineCurve: { knee: .05, floor: .1 }, format: v => `${v} Hz` }
  };
  let uid = 0;
  const ST = { state, escapeHTML: v => String(v), uid: () => `u${++uid}`, toast: (m, kind) => toasts.push({ m, kind }),
    mutate(label, fn) { mutations.push({ label, before: copy(state.project) }); state.undo.push(label); fn(state.project); },
    syncEngineProject(make) { const draft = make(); syncs.push(draft); return Promise.resolve(true); } };
  const window = { ST, STStudio: { target: (owner, target) => descriptors[target] ? { ...descriptors[target] } : undefined }, localStorage: { getItem: k => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, v) } };
  const context = { window, document, Event: class { constructor(type) { this.type = type; } }, Date: { now: () => clock },
    setTimeout: (fn, delay = 0) => { timers.set(++timerID, { fn, at: clock + delay }); return timerID; }, clearTimeout: id => timers.delete(id), Promise, JSON, Math, Number, String, Object, Array, Map, Set };
  vm.createContext(context);
  if (fine) web.load(context, 'fine-control.js', { text: fineSource });
  web.load(context, 'automation-record.js', { text: source });
  const R = window.STAutomationRecord;
  const h = {
    R, state, mutations, syncs, toasts, listeners, window,
    pattern: (id = 3) => state.project.patterns.find(p => p.id === id),
    advance(ms) { const end = clock + ms; for (;;) { const due = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0]; if (!due) break; clock = Math.max(clock, due[1].at); timers.delete(due[0]); due[1].fn(); } clock = end; },
    status(position, { playing = true, pattern = 3, transportSeekRevision } = {}) {
      state.playing = playing; state.status = { playing, pattern, row: Math.floor(position), position, bpm: 120, transportSeekRevision }; state.lastStatusAt = clock;
      R.event('status', state.status);
    },
    stop() { h.status(0, { playing: false }); },
    ref: (target = 'volume', track = 0) => R.makeRef(track, target, descriptors[target]?.name),
    lane: (target = 'volume', id = 3) => { const lane = h.pattern(id).automation.find(l => l.target === target); return lane && copy(lane); },
    element(attrs) {
      const el = { type: 'range', value: String(attrs.value), min: String(attrs.min ?? 0), max: String(attrs.max ?? 1.5), step: String(attrs.step ?? .01), disabled: false, id: '',
        dataset: { automate: attrs.target || 'volume', track: '0', name: 'Track volume', ...(attrs.dataset || {}) },
        matches: sel => sel.includes('data-automate') && (!sel.includes('type=range') || el.type === 'range'), closest: sel => el.matches(sel) ? el : null, setAttribute() {}, getAttribute: () => null };
      return el;
    },
    fire(type, event) { const e = { defaultPrevented: false, prevented: false, stopped: false, preventDefault() { this.prevented = true; this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, stopImmediatePropagation() { this.stopped = true; }, ...event }; for (const fn of listeners.get(type) || []) fn(e); return e; }
  };
  h.advance(1);
  return h;
}
const tests = [];
const test = (name, run) => tests.push({ name, run });
const close = (a, b, eps = 1e-6) => assert(Math.abs(a - b) < eps, `${a} != ${b}`);

test('pure helpers: lane evaluation, unit mapping and thinning', () => {
  const { valueAt, thin, toUnit, fromUnit, finalize } = harness().R._test;
  const pts = [{ row: 0, value: 0 }, { row: 8, value: 1 }];
  close(valueAt(pts, 4), .5); close(valueAt(pts, 4, 'step'), 0); close(valueAt(pts, -1), 0); close(valueAt(pts, 99), 1); close(valueAt([], 3, 'linear', .7), .7);
  const info = { min: 20, max: 20000, log: true, step: 1 };
  close(toUnit(20, info), 0); close(toUnit(20000, info), 1); assert.equal(fromUnit(1, info), 20000);
  const ramp = Array.from({ length: 33 }, (_, i) => ({ row: i / 4, value: i / 32, rec: true }));
  const thinned = thin(ramp, { min: 0, max: 1 });
  assert.equal(thinned.length, 2, 'a straight recorded ramp keeps its endpoints only');
  const kept = thin([{ row: 0, value: .2 }, ...ramp.slice(1, 5), { row: 5, value: .9 }], { min: 0, max: 1 });
  assert.equal(kept[0].value, .2); assert.equal(kept[kept.length - 1].value, .9);
  const many = Array.from({ length: 9000 }, (_, i) => ({ row: i / 64, value: (i % 2) * .5, rec: true }));
  assert(finalize(many, { min: 0, max: 1 }, 'linear', 200).length <= 4000, 'engine point limit');
});

test('dense finalizer preserves the full timeline, late peaks and endpoints within a bounded time', () => {
  const h=harness(),start=process.hrtime.bigint();
  for(const interpolation of ['linear','step']){
    const input=Array.from({length:16001},(_,i)=>({row:i/64,value:(i%2)*.5,rec:true}));
    input[0].value=.13;input[15111].value=1;input.at(-1).value=.27;
    const before=JSON.stringify(input),summary={},result=h.R._test.finalize(input,{min:0,max:1},interpolation,255,summary);
    assert(result.length<=4000);assert.equal(summary.limited,true);
    assert.deepEqual(copy(result[0]),{row:0,value:.13});assert.deepEqual(copy(result.at(-1)),{row:250,value:.27});
    assert(result.some(point=>point.row===15111/64&&point.value===1),'a peak near the end cannot vanish with the tail');
    for(let i=1;i<result.length;i++)assert(result[i].row>result[i-1].row,'strictly ordered unique positions');
    assert(result.some(point=>point.row>245&&point.value===0));assert(result.some(point=>point.row>245&&point.value===.5),'late oscillations remain represented');
    assert.equal(JSON.stringify(input),before,'reduction never changes the recording buffer');
    assert.deepEqual(copy(h.R._test.finalize(input,{min:0,max:1},interpolation,255)),copy(result),'reduction is deterministic');
  }
  const elapsed=Number(process.hrtime.bigint()-start)/1e6;
  console.log(`dense reduction: ${elapsed.toFixed(1)} ms for four 16,001-point reductions`);
  assert(elapsed<3000,`dense reduction must not stall the UI for seconds (${elapsed.toFixed(1)} ms)`);
});

test('dense budget retains original lane points and punch anchors whenever they fit', () => {
  const {finalize}=harness().R._test;
  const input=Array.from({length:12001},(_,i)=>({row:i/64,value:(i%2)*.5,rec:true}));
  const original={row:100/64,value:.23},anchor={row:11000/64,value:.73,anchor:true};input[100]=original;input[11000]=anchor;
  const result=finalize(input,{min:0,max:1},'linear',255);
  for(const point of [input[0],original,anchor,input.at(-1)])assert(result.some(item=>item.row===point.row&&item.value===point.value));
  const protectedOnly=input.map(({row,value})=>({row,value}));
  const overloaded=finalize(protectedOnly,{min:0,max:1},'linear',255);
  assert(overloaded.length<=4000);assert.equal(overloaded[0].row,0);assert.equal(overloaded.at(-1).row,187.5,'even an over-budget imported lane retains its end');
});

test('bounded simplification retains smooth ramps and error tolerance before a capacity reduction', () => {
  const {thin,valueAt}=harness().R._test;
  const ramp=Array.from({length:16001},(_,i)=>({row:i/64,value:i/16000,rec:true}));
  const reduced=thin(ramp,{min:0,max:1});assert(reduced.length<150,'a long ramp remains compact despite bounded spans');
  for(let i=0;i<ramp.length;i+=37)assert(Math.abs(valueAt(reduced,ramp[i].row)-ramp[i].value)<=.004);
  const curve=Array.from({length:2001},(_,i)=>({row:i/64,value:.5+.4*Math.sin(i/50),rec:true}));
  const curved=thin(curve,{min:0,max:1});assert(curved.length<400);
  for(const point of curve)assert(Math.abs(valueAt(curved,point.row)-point.value)<=.004+1e-12);
});

test('dense live preview and committed take retain the end and report lossy budget reduction', () => {
  const h=harness({length:256});h.R.setMode('latch',true);h.status(0);h.R.begin(h.ref(),.4);
  const lane=[...h.R._test.rec.pass.lanes.values()][0];lane.seg=null;
  lane.work=Array.from({length:12001},(_,i)=>({row:i/64,value:(i%2)*.5,rec:true}));lane.work.at(-1).value=.81;
  const preview=h.R._test.draftProject().patterns[0].automation[0].points;
  assert(preview.length<=4000);assert.equal(preview.at(-1).row,187.5);assert.equal(preview.at(-1).value,.81);
  h.R._test.startSegment(lane,100,.99);
  const held=h.R._test.draftProject().patterns[0].automation[0].points;
  assert(held.some(point=>point.row===100&&point.value===.99));assert(held.some(point=>point.row>102&&point.row<103&&point.value===.99),'the live look-ahead guard survives the dense preview budget');
  lane.seg=null;
  assert.equal(h.R.finishPass(),true);assert.equal(h.state.undo.length,1);assert.equal(h.lane().points.at(-1).value,.81);
  assert(h.toasts.some(item=>/Dense lanes were reduced/.test(item.m)),'capacity-driven loss is explicit');
});

test('Read mode never intercepts or records', () => {
  const h = harness();
  h.status(4);
  assert.equal(h.R.mode, 'read');
  assert.equal(h.R.begin(h.ref(), .3), false);
  const el = h.element({ value: .8 });
  const e = h.fire('keydown', { key: 'ArrowUp', target: el });
  assert.equal(e.prevented, false); assert.equal(el.value, '0.8');
  h.stop(); assert.equal(h.mutations.length, 0);
});

test('Touch writes while held, returns to the old lane, one Undo step, base untouched', () => {
  const h = harness({ lanes: [{ id: 'l', track: 0, target: 'volume', name: 'Vol', interpolation: 'linear', points: [{ row: 0, value: .5 }, { row: 32, value: .5 }] }] });
  h.R.setMode('touch', true);
  h.status(8);
  const ref = h.ref();
  assert.equal(h.R.begin(ref, .5), true);
  for (const [pos, v] of [[9, .7], [10, 1], [11, 1.2]]) { h.status(pos); h.R.value(ref, v); }
  h.status(12);
  assert.equal(h.mutations.length, 0, 'nothing committed during the pass');
  assert.deepEqual(h.lane().points, [{ row: 0, value: .5 }, { row: 32, value: .5 }], 'project untouched while recording');
  h.R.end(ref);
  h.status(16); h.advance(300);
  h.stop();
  assert.equal(h.mutations.length, 1); assert.match(h.mutations[0].label, /^Record automation · Track volume/);
  const pts = h.lane().points;
  assert.deepEqual(pts[0], { row: 0, value: .5 }); assert.deepEqual(pts[pts.length - 1], { row: 32, value: .5 });
  assert(pts.some(p => p.value === 1.2), 'peak recorded');
  const anchor = pts.find(p => p.row > 7.5 && p.row < 8); assert(anchor && anchor.value === .5, 'punch-in anchor keeps the old lane');
  const after = pts.filter(p => p.row > 12 && p.row < 32); assert(after.length >= 1 && after.every(p => p.value === .5), 'Touch returns to the old lane value');
  assert.equal(h.state.project.tracks[0].volume, .8, 'base value never overwritten');
  assert(h.syncs.length > 0, 'engine heard a draft during the pass');
  assert(h.syncs.every(d => d !== h.state.project));
  // Undo snapshot is the pre-pass project.
  assert.deepEqual(h.mutations[0].before.patterns[0].automation[0].points, [{ row: 0, value: .5 }, { row: 32, value: .5 }]);
});

test('Latch keeps writing the last value until stop and replaces old points', () => {
  const h = harness({ lanes: [{ id: 'l', track: 0, target: 'volume', name: 'Vol', interpolation: 'linear', points: [{ row: 0, value: .1 }, { row: 20, value: .9 }, { row: 40, value: .1 }] }] });
  h.R.setMode('latch', true);
  h.status(4);
  const ref = h.ref();
  h.R.begin(ref, .6); h.R.end(ref);
  for (let pos = 5; pos <= 30; pos += .5) { h.status(pos); }
  h.stop();
  const pts = h.lane().points;
  assert(!pts.some(p => p.row === 20), 'old point inside the latched range is replaced');
  assert(pts.filter(p => p.row >= 4 && p.row <= 30).every(p => p.value === .6), 'latched value held');
  assert.deepEqual(pts[pts.length - 1], { row: 40, value: .1 }, 'points after stop survive');
  assert(pts.length <= 7, `thinned: ${JSON.stringify(pts)}`);
});

test('new lane: created as a track lane with a base-value anchor', () => {
  const h = harness();
  h.R.setMode('touch', true);
  h.status(16);
  const ref = h.ref('cutoff');
  h.R.begin(ref, 1000); h.status(17); h.R.value(ref, 4000); h.status(18); h.R.end(ref); h.status(19); h.stop();
  const lane = h.lane('cutoff');
  assert(lane, 'lane created'); assert.equal(lane.track, 0); assert.equal(lane.owner, undefined); assert.equal(lane.interpolation, 'linear');
  assert.equal(lane.points[0].value, 1000, 'value before the pass stays at the base value');
  assert(lane.points.some(p => p.value === 4000));
  assert.equal(lane.points[lane.points.length - 1].value, 1000, 'Touch returns to the base value');
  assert.equal(h.state.project.instruments[0].cutoff, 1000);
});

test('loop wrap continues a held gesture from the pattern start', () => {
  const h = harness({ length: 16 });
  h.R.setMode('latch', true);
  h.status(12);
  const ref = h.ref();
  h.R.begin(ref, .3);
  h.status(14); h.status(15.5); h.status(1); h.status(3);
  h.stop();
  const pts = h.lane().points;
  assert(pts.some(p => p.row <= 1.01 && p.value === .3), 'wrapped segment written from the start');
  assert(pts.some(p => p.row >= 15 && p.value === .3));
});

test('explicit forward seek splits a latched pass and preserves automation in skipped rows',()=>{
 const h=harness({lanes:[{id:'l',track:0,target:'volume',name:'Vol',interpolation:'linear',points:[{row:0,value:.2},{row:8,value:.6},{row:32,value:.2}]}]});h.R.setMode('latch',true);h.status(2,{transportSeekRevision:'0'});h.R.begin(h.ref(),1);h.status(3,{transportSeekRevision:'0'});h.status(20,{transportSeekRevision:'1'});h.status(21,{transportSeekRevision:'1'});h.stop();const points=h.lane().points;assert(points.some(p=>p.row===8&&p.value===.6));assert(points.some(p=>p.row===20&&p.value===1));assert.equal(h.state.project.tracks[0].volume,.8);assert.equal(h.mutations.length,1);
});

test('keyboard arrows on a focused parameter record instead of editing the base', () => {
  const h = harness();
  h.R.setMode('touch', true);
  h.status(4);
  const el = h.element({ value: .8 });
  const e = h.fire('keydown', { key: 'ArrowUp', target: el });
  assert.equal(e.prevented, true); assert.equal(e.stopped, true);
  assert(Number(el.value) > .8);
  const input = h.fire('input', { type: 'input', target: el });
  assert.equal(input.stopped, true, 'the base handler never sees the edit');
  h.status(6); h.advance(700); h.status(8); h.stop();
  assert.equal(h.mutations.length, 1);
  assert(h.lane().points.some(p => Math.abs(p.value - Number(el.value)) < 1e-9));
  assert.equal(h.state.project.tracks[0].volume, .8);
});

test('fine-control sliders record engine values: curved positions, ⌥ fine steps and curved MIDI', () => {
  const h = harness({ fine: true }), F = h.window.STFine, spec = { min: 0, max: 100, knee: .05, floor: .1 };
  h.R.setMode('touch', true); h.status(4);
  const el = h.element({ target: 'lfoRate', value: F.toPosition(1.25, spec), min: 0, max: 1, step: 'any', dataset: { name: 'LFO 1 rate', fineMin: '0', fineMax: '100', fineKnee: '.05', fineFloor: '.1' } });
  const e = h.fire('keydown', { key: 'ArrowUp', altKey: true, target: el });
  assert.equal(e.prevented, true, '⌥ arrows record on fine-control sliders');
  close(F.fromPosition(Number(el.value), spec), 1.26);
  h.status(5); el.value = String(F.toPosition(.05, spec)); h.fire('input', { type: 'input', target: el });
  h.status(6); h.advance(700); h.status(8); h.stop();
  const values = h.lane('lfoRate').points.map(p => p.value);
  assert(values.includes(1.26), `keyboard step recorded in Hz: ${values}`); assert(values.includes(.05), `slider position recorded in Hz: ${values}`);
  assert(values.every(v => v >= 0 && v <= 100));
  // MIDI CC follows the perceptual curve: mid-travel is a musical rate, not 50 Hz.
  const g = harness({ fine: true }), ref = g.ref('lfoRate');
  g.R.learn(ref); g.R.event('midi.message', { status: 0xB0, data1: 30, data2: 0 });
  g.R.setMode('latch', true); g.status(2); g.R.event('midi.message', { status: 0xB0, data1: 30, data2: 64 }); g.status(4); g.stop();
  const midi = g.lane('lfoRate').points.map(p => p.value).filter(v => v !== 1.25);
  assert(midi.length && midi.every(v => v > .5 && v < 5), `curved CC value: ${midi}`);
  // Without fine-control.js, plain controls keep their linear behaviour.
  assert.equal(harness().R._test.fromUnit(.5, { min: 0, max: 100 }, { knee: .05 }), 50);
});

test('arming limits recording to armed parameters', () => {
  const h = harness();
  h.R.setMode('touch', true);
  h.R.toggleArm(h.ref('cutoff'));
  h.status(4);
  assert.equal(h.R.begin(h.ref('volume'), .4), false, 'unarmed parameter passes through');
  assert.equal(h.R.begin(h.ref('cutoff'), 500), true);
});

test('Write mode overwrites armed parameters from playback start', () => {
  const h = harness();
  h.R.toggleArm(h.ref('volume'));
  h.R.setMode('write', true);
  h.status(0); h.status(4); h.status(8); h.stop();
  const lane = h.lane();
  assert(lane && lane.points.length >= 2, 'armed lane written without touching');
  assert(lane.points.every(p => p.value === .8));
});

test('MIDI learn maps a CC that then records a plug-in parameter', () => {
  const h = harness();
  const ref = h.ref('plugin:instrument:0:7');
  h.R.learn(ref);
  h.R.event('midi.message', { status: 0xB2, data1: 21, data2: 0 });
  h.R.setMode('latch', true);
  h.status(2);
  h.R.event('midi.message', { status: 0xB2, data1: 21, data2: 127 });
  h.status(4); h.stop();
  const lane = h.lane('plugin:instrument:0:7');
  assert(lane.points.some(p => p.value === 1));
  assert(h.toasts.some(t => /CC 21/.test(t.m)));
});

test('context items expose arm, mode and MIDI learn for automatable controls', () => {
  const h = harness();
  const el = h.element({ value: .8 });
  const items = h.R.contextItems({ target: el });
  assert(items.some(i => i?.title === 'Arm automation write'));
  assert(items.some(i => i?.title === 'Automation write mode' && i.items.length === 4));
  assert(items.some(i => i?.title === 'Learn MIDI controller…'));
  assert.equal(h.R.contextItems({ target: { closest: () => null } }), null);
});

test('⌥W toggles Read and the last write mode; ⌥⇧W cycles', () => {
  const h = harness();
  h.fire('keydown', { key: '∑', code: 'KeyW', altKey: true, target: { closest: () => null } });
  assert.equal(h.R.mode, 'touch');
  h.fire('keydown', { key: '∑', code: 'KeyW', altKey: true, shiftKey: true, target: { closest: () => null } });
  assert.equal(h.R.mode, 'latch');
  h.fire('keydown', { key: '∑', code: 'KeyW', altKey: true, target: { closest: () => null } });
  assert.equal(h.R.mode, 'read');
  h.fire('keydown', { key: '∑', code: 'KeyW', altKey: true, target: { closest: () => null } });
  assert.equal(h.R.mode, 'latch');
});

test('project replacement discards the pass instead of committing stale lanes', () => {
  const h = harness();
  h.R.setMode('latch', true);
  h.status(2); h.R.begin(h.ref(), .4); h.status(4);
  h.state.project = copy(h.state.project);
  h.status(6); h.stop();
  assert.equal(h.mutations.length, 0);
});

test('a refused stop commit retains the pass for retry without partial project or history writes', () => {
  const h = harness(); h.R.setMode('latch', true); h.status(2); h.R.begin(h.ref(), .4); h.status(4);
  const before = JSON.stringify(h.state.project), mutate = h.window.ST.mutate;
  h.window.STRouting = {isApplying:true}; h.window.ST.mutate = () => {};
  h.stop(); assert.equal(JSON.stringify(h.state.project),before); assert.equal(h.state.undo.length,0);
  assert.equal(h.R.hasPending,true,'the recorded take must survive a refused commit');
  assert.equal(h.R.finishPass(),false); assert.equal(h.R.hasPending,true);
  h.window.STRouting.isApplying=false; h.window.ST.mutate=mutate;
  assert.equal(h.R.finishPass(),true); assert.equal(h.R.hasPending,false); assert(h.lane().points.some(point=>point.value===.4)); assert.equal(h.state.undo.length,1);
  assert.equal(h.R.finishPass(),true,'an already settled pass is successful for capture callers');
});

test('a frozen-source refusal retains the take and does not roll back unrelated project identity', () => {
  const h = harness(); h.R.setMode('touch',true); h.status(2); h.R.begin(h.ref('cutoff'),1200); h.status(4);
  const owner=h.state.project; h.window.STFreeze={checkMutation:()=>({message:'Unfreeze to commit cutoff'})};
  assert.equal(h.R.finishPass(),false); assert.equal(h.state.project,owner); assert.equal(h.R.hasPending,true); assert.equal(h.state.undo.length,0);
  h.window.STFreeze.checkMutation=()=>null; assert.equal(h.R.finishPass(),true); assert.equal(h.state.undo.length,1);
});

test('capture commits during an export without changing write mode and reports subsequent edits', () => {
  const h=harness();h.R.setMode('latch',true);h.status(1);h.R.begin(h.ref(),.4);const generation=h.R.editGeneration;h.status(2);assert(h.R.editGeneration>generation);assert(h.R.hasPendingEdits);
  h.window.STExport={isBusy:true};let options;const mutate=h.window.ST.mutate;h.window.ST.mutate=(label,fn,value)=>{options=value;mutate(label,fn);};
  assert.equal(h.R.finishPass({nativeCapture:true,render:false}),true);assert.equal(options.nativeCapture,true);assert.equal(options.render,false);assert.equal(h.R.mode,'latch');assert.equal(h.R.hasPendingEdits,false);
  const captured=h.R.editGeneration;h.R.value(h.ref(),.7);assert(h.R.editGeneration>captured);assert(h.R.hasPendingEdits);
});

test('buffered automation follows stable track identity and never lands in a reused pattern', () => {
  const h=harness();h.R.setMode('latch',true);h.status(1);h.R.begin(h.ref('volume',1),.4);h.state.project.tracks.shift();h.stop();assert.equal(h.lane().track,0);
  for(const removed of ['pattern','channel']){
    const h=harness();h.R.setMode('latch',true);h.status(1);h.R.begin(h.ref(),.4);
    if(removed==='pattern')h.state.project.patterns[0]={...copy(h.pattern()),name:'Replacement'};else h.state.project.tracks.shift();
    h.stop();assert.equal(h.mutations.length,0);assert.equal(h.R.hasPending,false);assert.equal(h.pattern().automation.length,0);assert(h.toasts.some(item=>/removed pattern or channel/.test(item.m)));
  }
});

test('replacement clears old release timers and recent claims without touching the next session', () => {
  const h=harness(); h.R.setMode('touch',true); h.status(1);
  h.fire('keydown',{key:'ArrowUp',target:h.element({value:.8})});
  h.state.project=copy(h.state.project); h.R.event('project.loaded');
  h.status(2); const ref=h.ref(); h.R.begin(ref,.9);
  h.advance(700);
  assert.equal(h.R._test.rec.gestures.get(`${ref.owner}|${ref.target}`)?.active,true,'a timer from the old session cannot release the new gesture');
  h.R.end(ref); assert.equal(h.R.claimed(ref),true);
  h.state.project=copy(h.state.project); h.R.event('project.new');
  assert.equal(h.R.claimed(h.ref()),false,'old change-event suppression cannot swallow a new-session base edit');
  assert.equal(h.R._test.rec.pointer,null); assert.equal(h.R._test.rec.wasPlaying,false);
});

let failed = 0;
const selected = tests.filter(item => !process.env.AUTOMATION_TEST_FILTER || item.name.includes(process.env.AUTOMATION_TEST_FILTER));
for (const { name, run } of selected) {
  try { run(); console.log(`ok - ${name}`); }
  catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
}
if (failed) { console.error(`${failed} failed`); process.exit(1); }
console.log(`${selected.length} automation record tests passed`);
