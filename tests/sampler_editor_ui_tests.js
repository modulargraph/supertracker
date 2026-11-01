'use strict';
// Sample editor in the actual sampler module: keyboard markers, zero-crossing snap,
// source-preserving region edits, slicing and slices-to-pattern. No audio devices.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('sampler.js');
const clone = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
const settle = () => new Promise(resolve => setImmediate(resolve));

function harness({ native = true, mode = 'forward' } = {}) {
  const nodes = new Map(), requests = [], notices = [], notes = [];
  let html = '', responder = () => ({});
  const context2d = new Proxy({}, { get: (target, key) => key === 'measureText' ? () => ({ width: 8 }) : (key in target ? target[key] : () => {}), set: (target, key, value) => { target[key] = value; return true; } });
  class Element {
    constructor(id, tag, attrs) {
      this.id = id; this.tag = tag; this.attrs = {}; this.dataset = {}; this.listeners = new Map(); this.disabled = /\sdisabled(?:\s|$)/.test(attrs); this.value = ''; this.textContent = '';
      this.classList = { toggle() {} };
      for (const match of attrs.matchAll(/([\w-]+)="([^"]*)"/g)) { this.attrs[match[1]] = match[2]; if (match[1].startsWith('data-')) this.dataset[match[1].slice(5).replace(/-([a-z])/g, (_, x) => x.toUpperCase())] = match[2]; }
      if (tag === 'canvas') { this.clientWidth = 1000; this.clientHeight = 190; this.width = 0; this.height = 0; this.getContext = () => context2d; this.getBoundingClientRect = () => ({ left: 0, width: 1000 }); }
    }
    addEventListener(type, fn) { const list = this.listeners.get(type) || []; list.push(fn); this.listeners.set(type, list); }
    setAttribute(key, value) { this.attrs[key] = String(value); }
    getAttribute(key) { return this.attrs[key]; }
    focus() { document.activeElement = this; }
    querySelector() { return null; }
    contains(other) { return other === this; }
    closest(selector) { return selector === '#sampler-editor' && this.inEditor ? this : null; }
  }
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => {
    const key = selector.match(/^\[data-([\w-]+)\]$/)?.[1]; if (!key) return [];
    const name = key.replace(/-([a-z])/g, (_, x) => x.toUpperCase()); return [...nodes.values()].filter(node => node.dataset[name] !== undefined);
  } };
  const instrument = { id: 0, type: 'sample', name: 'Break', samplePath: '/fixture/break.wav', rootNote: 60, start: .1, end: .9, loopStart: .2, loopEnd: .8, mode, slicePoints: [], slices: [], sliceCount: 16, grainPosition: .5 };
  const state = { view: 'instrument', modal: null, native, instrument: 0, track: 0, row: 5, undo: [], redo: [], keyboardPlay: false,
    samples: { 0: { path: '/fixture/break.wav', frames: 48000, sampleRate: 48000, duration: 1, peaks: new Array(512).fill(.5) } },
    project: { bpm: 120, linesPerBeat: 4, activePattern: 0, instruments: [instrument], tracks: [{ name: 'Track 01', instrument: 0 }, { name: 'Track 02', instrument: 1 }], patterns: [{ id: 0, name: 'Pattern 01', length: 64, steps: [[], []], automation: [] }] } };
  const ST = { state, escapeHTML,
    mutate(label, fn, options = {}) { state.undo.push({ label, project: clone(state.project) }); fn(state.project); if (options.render !== false) mount(); },
    refresh() { mount(); }, toast(message, type) { notices.push({ message, type }); },
    request(command, payload) { requests.push({ command, payload: clone(payload) }); return Promise.resolve(responder(command, payload)); },
    noteOn(note, key, record) { notes.push({ on: note, key, record }); }, noteOff(key) { notes.push({ off: key }); },
    textPrompt(title, value, callback) { ST.prompt = { title, value, callback }; }, showRecording() {},
    syncEngineProject: async () => ({}) };
  const timers = [];
  const context = { window: { ST, devicePixelRatio: 1 }, document, setTimeout: (fn, ms) => { timers.push(fn); return timers.length; }, clearTimeout() {}, queueMicrotask, Date };
  // Local model of the shared transaction boundary. The separate sampler
  // transaction suite drives the actual app helper and its native ACK fence.
  context.window.STInstrumentEdits = {async run(id, tracks, work) {
    const owner=state.project, inst=owner.instruments[id], before=JSON.stringify(inst);
    const current=()=>state.project===owner&&owner.instruments[id]===inst&&JSON.stringify(inst)===before;
    try {return await work({
      async native(command,payload){const result=await ST.request(command,payload);if(!current())throw Error('The destination changed.');return result;},
      async commit(label,apply){if(!current())throw Error('The destination changed.');const candidate=clone(owner);apply(candidate);let committed=false;ST.mutate(label,p=>{apply(p);committed=true;},{render:false});if(!committed)throw Error('The edit was refused.');}
    });} catch(error) {if(await ST.syncEngineProject(()=>clone(state.project),{replace:true})===false)throw Error('Audio restoration was not acknowledged.');ST.toast(error.message,'error');throw error;}
  }};
  web.runText('sampler.js', source, context, { filename: 'sampler.js' });
  const module = context.window.STSampler;
  function mount() {
    const focus = document.activeElement?.id; nodes.clear();
    html = module.render(state.project.instruments[state.instrument]);
    const editorEnd = html.indexOf('</section>');
    for (const match of html.matchAll(/<(input|button|select|details|output|p|div|canvas|section|span)\b([^>]*)>/g)) {
      const id = match[2].match(/\bid="([^"]+)"/)?.[1] || `anonymous-${nodes.size}`, element = new Element(id, match[1], match[2]);
      element.inEditor = match.index < editorEnd; nodes.set(id, element);
    }
    module.bind(); if (focus) nodes.get(focus)?.focus();
  }
  mount();
  const canvas = () => nodes.get('sampler-wave-canvas');
  function key(name, options = {}) {
    const event = { key: name, target: canvas(), shiftKey: false, altKey: false, metaKey: false, ctrlKey: false, repeat: false, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...options };
    return module.key(event);
  }
  return { ST, module, state, document, window:context.window, instrument, nodes, requests, notices, notes, mount, canvas, key, timers, get html() { return html; }, respond(fn) { responder = fn; },
    flushTimers() { while (timers.length) timers.shift()(); } };
}
const tests = []; const test = (name, fn) => tests.push({ name, fn });

test('editor renders play modes, markers and edit tools with an accessible waveform slider', () => {
  const h = harness();
  assert(h.html.indexOf('id="sampler-editor"') < h.html.indexOf('Record a sample'), 'editor comes first in the sampler panels');
  for (const label of ['1-Shot', 'Forward loop', 'Backward loop', 'Ping-pong', 'Slice', 'Wavetable', 'Granular']) assert(h.html.includes(`>${label}</button>`), label);
  assert.equal(h.nodes.get('sampler-mode-forward').attrs['aria-checked'], 'true');
  assert.equal(h.canvas().attrs.role, 'slider');
  assert.match(h.canvas().attrs['aria-valuetext'], /Start 0\.100 s/);
  for (const id of ['sampler-select-start', 'sampler-select-end', 'sampler-select-loopStart', 'sampler-select-loopEnd', 'sampler-edit-trim', 'sampler-edit-normalize', 'sampler-edit-reverse', 'sampler-edit-gain', 'sampler-edit-fadeIn', 'sampler-edit-fadeOut', 'sampler-editor-pattern']) assert(h.nodes.get(id), id);
  h.instrument.mode = 'oneshot'; h.mount();
  assert(!h.nodes.get('sampler-select-loopStart'), 'loop markers only in loop modes');
  h.instrument.type = 'synth'; h.mount(); assert(!h.html.includes('sampler-editor'), 'synths have no sample editor');
});

test('arrow keys pick markers and nudge in frames; a nudge burst is one Undo step', () => {
  const h = harness(); h.canvas().focus();
  assert.equal(h.key('ArrowDown'), true); assert.match(h.canvas().attrs['aria-valuetext'], /^End/);
  assert.equal(h.key('ArrowDown'), true); assert.match(h.canvas().attrs['aria-valuetext'], /^Loop start/);
  const before = h.instrument.loopStart;
  h.key('ArrowRight'); h.key('ArrowRight'); h.key('ArrowRight', { altKey: true });
  assert.equal(h.state.undo.length, 1, 'held nudges coalesce');
  assert.equal(Math.round(h.instrument.loopStart * 48000), Math.round(before * 48000) + 48 + 48 + 1, 'fine = one column (48 frames at 1000 px), ⌥ = one frame');
  h.key('ArrowLeft', { shiftKey: true });
  assert(Math.abs(h.instrument.loopStart - (before + 97 / 48000 - .1)) < 1e-9, 'coarse = a tenth of the view');
  assert.equal(h.state.undo[0].project.instruments[0].loopStart, before, 'the Undo entry restores the pre-burst value');
  h.key('Home'); assert.equal(h.instrument.loopStart, h.instrument.start, 'Home = earliest allowed (region start)');
  h.key('ArrowUp'); h.key('ArrowUp'); h.key('End'); assert.equal(Math.round(h.instrument.start * 48000), Math.round(h.instrument.end * 48000) - 1, 'start stops one frame before end');
  h.flushTimers(); assert(h.nodes.get('sampler-wave-canvas'), 'the debounced refresh re-renders the editor');
});

test('moving the region keeps loop points inside it; zoom and scroll stay in bounds', () => {
  const h = harness(); h.canvas().focus();
  h.key('End'); // start → end - 1 frame pushes the loop into the region
  assert(h.instrument.loopStart >= h.instrument.start && h.instrument.loopEnd <= h.instrument.end && h.instrument.loopEnd > h.instrument.loopStart);
  h.key('0'); h.key('+'); h.key('+'); h.key('+');
  assert.match(h.nodes.get('sampler-editor-view').textContent, /8\.0×/);
  for (let n = 0; n < 20; n++) h.key('.');
  assert.match(h.nodes.get('sampler-editor-view').textContent, /– 1\.000 s/);
  for (let n = 0; n < 30; n++) h.key('+');
  assert.match(h.nodes.get('sampler-editor-view').textContent, /2000\.0×/, 'deepest zoom shows 24 frames');
  h.key('0'); assert.match(h.nodes.get('sampler-editor-view').textContent, /^View 0\.000 s – 1\.000 s · 1\.0×/);
});

test('zero-crossing snap asks the engine and always progresses in the nudge direction', async () => {
  const h = harness(); h.canvas().focus();
  h.key('z'); assert.equal(h.nodes.get('sampler-editor-snap').attrs['aria-pressed'], 'true');
  let call = 0;
  h.respond((command, payload) => { call++; return { snapped: [call === 1 ? .1 : .1 + 30 / 48000] }; }); // first answer would not move
  h.key('ArrowRight'); await settle(); await settle();
  assert.deepEqual(h.requests.map(r => [r.command, r.payload.direction]), [['sample.peaks', 0], ['sample.peaks', 1]]);
  assert.equal(Math.round(h.instrument.start * 48000), 4800 + 30);
  h.respond(() => ({ snapped: [.25] })); h.key('Z', { shiftKey: true }); await settle();
  assert.equal(h.instrument.start, .25);
  assert.equal(h.state.undo.at(-1).label, 'snap start to zero crossing');
});

test('region edits write a new file, keep markers (trim remaps them) and are one Undo step', async () => {
  const h = harness(); h.canvas().focus();
  h.respond((command, payload) => ({ path: `/edits/break-${payload.operation}.wav`, frames: 38400, sampleRate: 48000, duration: .8, peaks: [] }));
  h.key('n'); await settle(); await settle();
  assert.deepEqual(h.requests.at(-1), { command: 'sample.edit', payload: { path: '/fixture/break.wav', instrument: 0, operation: 'normalize', start: .1, end: .9 } });
  assert.equal(h.instrument.samplePath, '/edits/break-normalize.wav'); assert.equal(h.instrument.start, .1);
  assert.equal(h.state.undo.at(-1).project.instruments[0].samplePath, '/fixture/break.wav', 'Undo restores the source path');
  h.nodes.get('sampler-fade-curve').value = 'smooth'; for (const fn of h.nodes.get('sampler-fade-curve').listeners.get('change')) fn({ target: h.nodes.get('sampler-fade-curve') });
  h.canvas().focus(); h.key('o'); await settle(); await settle();
  assert.equal(h.requests.at(-1).payload.curve, 'smooth');
  h.canvas().focus(); h.key('t'); await settle(); await settle();
  assert.equal(h.requests.at(-1).payload.operation, 'trim');
  assert.deepEqual([h.instrument.start, h.instrument.end], [0, 1]);
  assert(Math.abs(h.instrument.loopStart - .125) < 1e-9 && Math.abs(h.instrument.loopEnd - .875) < 1e-9, 'loop points stay on the same audio after trim');
  assert.equal(h.state.undo.length, 3);
});

test('gain prompt validates and edits; failures leave the project untouched', async () => {
  const h = harness(); h.canvas().focus();
  h.respond(() => ({ path: '/edits/gain.wav' }));
  h.key('g'); const ST = h.ST; assert.match(ST.prompt.title, /Gain/); ST.prompt.callback('40'); assert.equal(h.notices.at(-1).type, 'error');
  ST.prompt.callback('-6'); await settle(); await settle();
  assert.equal(h.requests.at(-1).payload.decibels, -6); assert.equal(h.instrument.samplePath, '/edits/gain.wav');
  h.respond(() => { throw new Error('Disk full'); });
  const undo = h.state.undo.length; h.canvas().focus(); h.key('r'); await settle(); await settle();
  assert.equal(h.state.undo.length, undo); assert.equal(h.instrument.samplePath, '/edits/gain.wav');
  assert.match(h.nodes.get('sampler-editor-message').textContent, /Disk full/);
});

test('slices: add at selected marker, nudge between neighbours, remove, then build a pattern', () => {
  const h = harness({ mode: 'slice' }); h.canvas().focus();
  h.instrument.slicePoints = [0, .5]; h.instrument.slices = [0, .5]; h.instrument.sliceCount = 2; h.mount(); h.canvas().focus();
  h.key('a'); // split slice 1 (Start selected)
  assert.deepEqual(clone(h.instrument.slicePoints), [0, .25, .5]);
  assert.match(h.canvas().attrs['aria-valuetext'], /^Slice 2/);
  h.key('End'); assert(h.instrument.slicePoints[1] < .5 && h.instrument.slicePoints[1] > .499, 'slice markers stop before the next one');
  h.key('Delete'); assert.deepEqual(clone(h.instrument.slicePoints), [0, .5]);
  h.key('a'); h.key('a');
  const count = h.instrument.slicePoints.length, patterns = h.state.project.patterns.length;
  h.key('p');
  assert.equal(h.state.project.patterns.length, patterns + 1);
  const pattern = h.state.project.patterns.at(-1);
  assert.equal(h.state.project.activePattern, pattern.id); assert.equal(pattern.steps.length, 2);
  const notesPlaced = pattern.steps[0].map((step, row) => step && [row, step.note, step.gate]).filter(Boolean);
  // 0.8 s region at 120 BPM, 4 lines per beat = 6.4 rows; slice starts land on their timing.
  assert.equal(notesPlaced.length, count);
  assert.deepEqual(clone(notesPlaced).map(([, note]) => note), Array.from({ length: count }, (_, i) => 60 + i));
  assert.equal(notesPlaced[0][0], 0); assert(notesPlaced.every(([row], i) => !i || row > notesPlaced[i - 1][0]));
  assert.equal(pattern.length, 16);
  assert.equal(h.state.undo.at(-1).label, 'slices to pattern');
  h.state.track = 1; h.key('p'); assert.match(h.notices.at(-1).message, /Assign this instrument/);
});

test('play mode keys, keyboard-play passthrough, audition hold and Escape', () => {
  const h = harness(); h.canvas().focus();
  h.key('m'); assert.equal(h.instrument.mode, 'reverse');
  h.key('M', { shiftKey: true }); assert.equal(h.instrument.mode, 'forward');
  h.state.keyboardPlay = true; assert.equal(h.key('t'), false, 'letters play notes while ⌘P keyboard is on'); h.state.keyboardPlay = false;
  assert.equal(h.key(' '), true); assert.deepEqual(h.notes, [{ on: 60, key: 'sampler-editor', record: false }]);
  assert.equal(h.key(' ', { repeat: true }), true); assert.equal(h.notes.length, 1, 'key repeat does not retrigger');
  assert.equal(h.key('Escape'), true); assert.deepEqual(h.notes.at(-1), { off: 'sampler-editor' });
  for (const fn of h.canvas().listeners.get('keyup') || []) fn({ key: ' ', preventDefault() {} });
  assert.equal(h.notes.length, 2);
  // Keys outside the waveform are left to the rest of the app.
  assert.equal(h.module.key({ key: 'ArrowRight', target: h.nodes.get('sampler-edit-trim'), preventDefault() {} }), false);
});

test('context menu offers editor actions only inside the editor', () => {
  const h = harness();
  assert.equal(h.module.contextItems({ target: h.nodes.get('sampler-record-input') }), null);
  const items = h.module.contextItems({ target: h.canvas() });
  const titles = items.filter(item => item.title).map(item => item.title);
  for (const title of ['Trim to region', 'Normalize region', 'Snap marker to zero crossing', 'Slices to new pattern', 'Play mode']) assert(titles.includes(title), title);
});

test('marker nudges do not absorb an intervening musical edit into the old Undo burst', () => {
  const h=harness();h.canvas().focus();h.key('ArrowRight');const first=h.instrument.start;
  h.ST.mutate('change tempo',p=>p.bpm=135,{render:false});h.key('ArrowRight');
  assert.equal(h.state.undo.length,3,'a new nudge after tempo editing needs its own Undo');
  assert.equal(h.state.undo[2].project.instruments[0].start,first);assert.equal(h.state.undo[2].project.bpm,135);
});

test('a marker burst preserves the bounded Undo history rather than dropping extra entries', () => {
  const h=harness();for(let i=0;i<90;i++)h.state.undo.push({label:'Earlier '+i,project:clone(h.state.project)});
  const mutate=h.ST.mutate;h.ST.mutate=(...args)=>{mutate(...args);if(h.state.undo.length>90)h.state.undo.shift();};
  h.canvas().focus();h.key('ArrowRight');const history=h.state.undo.slice();h.key('ArrowRight');assert.deepEqual(h.state.undo,history);assert.equal(h.state.undo.length,90);
});

test('late zero crossing does not move a marker after selection, region or focus changed', async () => {
  for(const change of ['selection','region','view']){
    const h=harness();h.canvas().focus();let resolve;h.respond(()=>new Promise(done=>resolve=done));h.key('Z',{shiftKey:true});
    if(change==='selection')h.key('ArrowDown');else if(change==='region')h.instrument.end=.7;else h.state.view='mixer';
    resolve({snapped:[.25]});await settle();assert.equal(h.instrument.start,.1);assert.equal(h.state.undo.length,0);
  }
});

test('pending region edit refuses stale markers and retains focus on a newer control', async () => {
  const h=harness();h.canvas().focus();let resolve;h.respond(()=>new Promise(done=>resolve=done));h.key('t');
  h.instrument.start=.3;const next=h.nodes.get('sampler-fade-curve');next.focus();const metadata=h.state.samples[0];
  resolve({path:'/edits/old-region.wav',frames:38400,duration:.8});await settle();await settle();
  assert.equal(h.instrument.samplePath,'/fixture/break.wav');assert.equal(h.instrument.start,.3);assert.equal(h.state.samples[0],metadata);assert.equal(h.state.undo.length,0);assert.equal(h.document.activeElement,next);assert.equal(h.nodes.get('sampler-fade-curve'),next);assert(!h.notices.some(item=>/done|restores/.test(item.message)));
});

test('a newer focused control survives delayed native candidate acceptance', async () => {
  const h=harness();h.canvas().focus();h.respond(()=>({path:'/edits/accepted.wav',frames:100}));
  const run=h.window.STInstrumentEdits.run;let release,ready=false;const gate=new Promise(resolve=>release=resolve);
  h.window.STInstrumentEdits.run=(id,tracks,work)=>run(id,tracks,edit=>work({...edit,async commit(...args){ready=true;await gate;return edit.commit(...args);}}));
  const task=h.module.command('trim');await settle();assert(ready);h.nodes.get('sampler-fade-curve').focus();release();await task;
  assert.equal(h.instrument.samplePath,'/edits/accepted.wav');assert.equal(h.document.activeElement.id,'sampler-fade-curve');assert.equal(h.state.undo.length,1);
});

test('a refused sample commit leaves waveform metadata unchanged and reports retained file', async () => {
  const h=harness();const metadata=h.state.samples[0];h.respond(()=>({path:'/edits/retained.wav',frames:100,duration:.01}));h.ST.mutate=()=>{};
  await h.module.command('trim');assert.equal(h.instrument.samplePath,'/fixture/break.wav');assert.equal(h.state.samples[0],metadata);assert.equal(h.state.undo.length,0);assert(h.notices.some(item=>item.message.includes('/edits/retained.wav')));
});

test('waveform slice hold uses explicit slice at root pitch across the MIDI note limit', async () => {
  for (const [root, count, selected] of [[127,2,1],[60,128,100]]) {
    const h=harness({mode:'slice'});h.instrument.rootNote=root;h.instrument.slicePoints=Array.from({length:count},(_,i)=>i/count);h.instrument.slices=[...h.instrument.slicePoints];h.instrument.sliceCount=count;h.mount();h.canvas().focus();
    for(let i=0;i<selected+1;i++)h.key('ArrowDown');
    const before=clone(h.state.project);h.key('Enter');h.key('Enter',{repeat:true});await settle();
    assert.match(h.canvas().attrs['aria-valuetext'],new RegExp(`^Slice ${selected+1} `));assert.equal(h.notes.length,0,'slice selection must not transpose an ordinary note');
    assert.deepEqual(h.requests,[{command:'step.preview',payload:{track:0,step:{note:root,instrument:0,velocity:100,fx1:{type:'slice',value:selected+1}},duration:2}}]);
    assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,0);
    for(const fn of h.canvas().listeners.get('keyup')||[])fn({key:'Enter',preventDefault(){}});
    assert.deepEqual(h.requests.at(-1).payload,{track:0,step:{note:-1},duration:.03});assert.equal(h.notes.length,0);
  }
});

test('waveform key release cancels pending synchronization before sound starts', async () => {
  const h=harness({mode:'slice'});let release;const gate=new Promise(done=>release=done);h.ST.syncEngineProject=async factory=>{await gate;return factory()===null?false:{};};h.canvas().focus();h.key(' ');
  for(const fn of h.canvas().listeners.get('keyup')||[])fn({key:' ',preventDefault(){}});release();await settle();assert.equal(h.requests.length,0);assert.equal(h.notes.length,0);
});

test('queued waveform preview rejects changed project, instrument, track, sample, marker, view or modal', async () => {
  const changes=[h=>h.state.project=clone(h.state.project),h=>h.state.project.instruments[0]={...h.instrument},h=>h.state.track=1,h=>h.state.project.tracks[0]={instrument:0},h=>h.instrument.samplePath='/new.wav',h=>h.instrument.rootNote=61,h=>h.instrument.slicePoints=[0,.25,.5],h=>h.key('ArrowDown'),h=>h.state.view='pattern',h=>h.state.modal='new-dialog'];
  for(const change of changes){const h=harness({mode:'slice'});let release;const gate=new Promise(done=>release=done);h.ST.syncEngineProject=async factory=>{await gate;return factory()===null?false:{};};h.canvas().focus();h.key('Enter');change(h);release();await settle();assert.equal(h.requests.length,0,String(change));assert.equal(h.notes.length,0);assert.equal(h.module.controller.info()?.auditioning||false,false);}
});

test('released pending native preview cannot cancel or label its successor, and stop uses original track', async () => {
  const h=harness({mode:'slice'});let release;h.respond((command,payload)=>payload.step.note>=0?new Promise(done=>release=done):{auditioning:false});h.canvas().focus();h.key('Enter');await settle();
  h.state.track=1;h.module.stopAudition();assert.deepEqual(h.requests.at(-1).payload,{track:0,step:{note:-1},duration:.03});h.state.track=0;
  const oldRelease=release;h.respond(()=>({auditioning:true}));h.key('ArrowDown');h.key('ArrowDown');h.key('Enter');await settle();const count=h.requests.length,status=h.nodes.get('sampler-editor-message').textContent;oldRelease({auditioning:true});await settle();assert.equal(h.requests.length,count);assert.equal(h.nodes.get('sampler-editor-message').textContent,status);assert.match(status,/slice 2/);
  h.key('Escape');assert.equal(h.requests.at(-1).payload.step.note,-1);assert(!h.requests.some(x=>x.command.startsWith('transport.')||x.command==='engine.panic'));
});

test('frozen waveform audition and failed strict preparation cannot start sound or remain held', async () => {
  const h=harness({mode:'slice'});let blocked=true;h.window.STFreeze={blockAudition:()=>blocked};h.canvas().focus();h.key('Enter');assert.equal(h.requests.length,0);assert(!h.module.controller.info().auditioning);
  blocked=false;let release;const gate=new Promise(done=>release=done);h.ST.syncEngineProject=async()=>{await gate;return {};};h.key('Enter');blocked=true;release();await settle();assert.equal(h.requests.length,0);assert(!h.module.controller.info().auditioning);
  blocked=false;h.ST.syncEngineProject=async()=>{throw Error('Native capture failed');};h.key('Enter');await settle();assert.equal(h.requests.length,0);assert(!h.module.controller.info().auditioning);assert.match(h.notices.at(-1).message,/Native capture failed/);
});

test('global sampler stop releases ordinary waveform holds without recursive note-on cancellation', () => {
  const h=harness();const base=h.ST.noteOn;h.ST.noteOn=(...args)=>{h.module.stopAudition();base(...args);};h.canvas().focus();h.key('Enter');assert(h.module.controller.info().auditioning);assert.deepEqual(h.notes,[{on:60,key:'sampler-editor',record:false}]);h.module.stopAudition();assert(!h.module.controller.info().auditioning);assert.deepEqual(h.notes.at(-1),{off:'sampler-editor'});h.module.stopAudition();assert.equal(h.notes.length,2);
});

(async () => {
  for (const { name, fn } of tests) { await fn(); console.log(`PASS ${name}`); }
  console.log(`Sampler editor UI: ${tests.length} groups passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
