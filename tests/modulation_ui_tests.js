#!/usr/bin/env node
'use strict';

// Run with: node tests/modulation_ui_tests.js
// Exercises actual modulation source without audio devices or installed plug-ins.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = process.env.SUPERTRACKER_MODULATION_SOURCE ? fs.readFileSync(process.env.SUPERTRACKER_MODULATION_SOURCE, 'utf8') : web.source('modulation.js');
const fineSource = web.source('fine-control.js');
const copy = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

class Element {
  constructor({ id = '', type = '', value = '', dataset = {} } = {}) {
    Object.assign(this, { id, type, value: String(value), dataset, listeners: new Map(), attributes: {}, innerHTML: '', textContent: '', isConnected: true });
  }
  get valueAsNumber() { return this.value.trim() === '' ? NaN : Number(this.value); }
  addEventListener(type, listener) {
    const listeners = this.listeners.get(type) || [];
    listeners.push(listener); this.listeners.set(type, listeners);
  }
  setAttribute(key, value) { this.attributes[key] = String(value); }
  dispatch(type, fields = {}) { const event = { target: this, type, preventDefault() { this.defaultPrevented = true; }, ...fields }; this.dispatchEvent(event); return event; }
  dispatchEvent(event) { for (const listener of this.listeners.get(event.type) || []) listener(event); return !event.defaultPrevented; }
  focus() { if (this.document) this.document.activeElement = this; }
}

class Root extends Element {
  constructor(index = 0) {
    super({ dataset: { modRoot: String(index) } });
    this.inputs = [];
    this.selects = [];
    this.modes = []; this.divisions = [];
    this.visuals = new Map(['[data-mod-envelope]', '[data-mod-envelope-amount]', '[data-mod-wave="lfo"]', '[data-mod-wave="lfo2"]', '[data-mod-extent="lfo"]', '[data-mod-extent="lfo2"]'].map(selector => [selector, new Element()]));
  }
  addParameter(key, value, index = 0) {
    const pair = ['number', 'range'].map(type => new Element({
      id: `mod-${type === 'number' ? 'number-' : ''}${index}-${key}`, type, value,
      dataset: { modParam: key, modInstrument: String(index), automate: key, track: '0', name: key }
    }));
    this.inputs.push(...pair); return pair;
  }
  addSelect(key, value, index = 0) {
    const select = new Element({ id: `mod-${index}-${key}`, value, dataset: { modSelect: key, modInstrument: String(index) } });
    this.selects.push(select); return select;
  }
  addTiming(kind, prefix, value, index = 0) {
    const select = new Element({ id: `mod-${index}-${prefix}${kind === 'mode' ? 'Timing' : 'Division'}`, value, dataset: { [kind === 'mode' ? 'modMode' : 'modDivision']: prefix, modInstrument: String(index) } });
    (kind === 'mode' ? this.modes : this.divisions).push(select); return select;
  }
  querySelectorAll(selector) {
    if (selector === '[data-mod-param]') return this.inputs;
    if (selector === '[data-mod-select]') return this.selects;
    if (selector === '[data-mod-mode]') return this.modes;
    if (selector === '[data-mod-division]') return this.divisions;
    const match = selector.match(/^\[data-mod-param="([^"]+)"\]$/);
    return match ? this.inputs.filter(input => input.dataset.modParam === match[1]) : [];
  }
  querySelector(selector) { const division = selector.match(/^\[data-mod-division="([^"]+)"\]$/); return division ? this.divisions.find(item => item.dataset.modDivision === division[1]) || null : this.visuals.get(selector) || null; }
}

function harness() {
  const synth = { id: 0, type: 'sample', name: 'Wavetable', lfoDepth: .25, lfo2Depth: .4, filterEnvAmount: -12 };
  const sampler = { id: 1, type: 'sample', name: 'Sampler', lfoDepth: .1 };
  const state = { instrument: 0, track: 0, project: { tracks: [{ instrument: 0 }, { instrument: 1 }, { instrument: 2 }, { instrument: 3 }], instruments: [synth, sampler, { id: 2, type: 'au' }, { id: 3, type: 'midi' }] } };
  const timers = new Map(), requests = [], mutations = [], notices = [];
  let timerID = 0, rerenderCount = 0, rejectRequests = false, pendingEdits = async () => {}, syncCalls = 0;
  const root = new Root(), document = { activeElement: null, querySelectorAll: selector => selector === '[data-mod-root]' ? [root] : [] };
  const ST = {
    state, escapeHTML,
    request(command, payload) { requests.push({ command, payload: copy(payload) }); return rejectRequests ? Promise.reject(new Error('Preview engine unavailable')) : Promise.resolve({}); },
    async syncEngineProject(makeProject) {
      syncCalls++; const owner = state.project;
      while (context.window.STStudio?.isMovingEffect) await context.window.STStudio.waitForEffectMove();
      await pendingEdits(); if (state.project !== owner) return false;
      const project = makeProject(); if (project == null) return false;
      return ST.request('engine.project', { project });
    },
    mutate(label, fn) {
      const focusedID = document.activeElement?.id;
      mutations.push({ label, before: copy(state.project) }); fn(state.project); rerenderCount++;
      // The main refresh restores focus by stable ID. Reproduce that boundary.
      if (focusedID) document.activeElement = { id: focusedID, restored: true };
    },
    refresh() { rerenderCount++; },
    toast(message, type) { notices.push({ message, type }); }
  };
  const context = { window: { ST }, document, Event: class { constructor(type, fields) { this.type = type; Object.assign(this,fields); } }, setTimeout: callback => { timers.set(++timerID, callback); return timerID; }, clearTimeout: id => timers.delete(id) };
  vm.createContext(context);
  web.load(context, 'fine-control.js', { text: fineSource });
  web.load(context, 'modulation.js', { text: source });
  return { module: context.window.STModulation, fine: context.window.STFine, window: context.window,
    // Slider position for a stored value on a curved (perceptual) modulation slider.
    position(key, value) { const spec = context.window.STModulation.targets(0).find(item => item.target === key); return String(context.window.STFine.toPosition(value, spec)); }, ST, state, synth, sampler, root, document, requests, mutations, notices, timers,
    flushTimers() { const callbacks = [...timers.values()]; timers.clear(); return Promise.all(callbacks.map(callback => callback())); },
    pendingEdits(fn) { pendingEdits = fn; }, get syncCalls() { return syncCalls; },
    setRejectRequests(value) { rejectRequests = value; },
    get rerenderCount() { return rerenderCount; }
  };
}

function assertControl(html, id, expected) {
  const tag = html.match(new RegExp(`<input\\b[^>]*\\bid="${id}"[^>]*>`))?.[0];
  assert(tag, `Missing control ${id}`);
  for (const [attribute, value] of Object.entries(expected)) assert(tag.includes(`${attribute}="${value}"`), `${id}: expected ${attribute}=${value}, got ${tag}`);
}

const tests = [];
function test(name, run) { tests.push({ name, run }); }

test('stale controls cannot change replacement projects or instruments', () => {
  for (const replacement of ['project','instrument','detached']) {
    const h = harness(), [input] = h.root.addParameter('filterEnvAmount',-12), mode = h.root.addTiming('mode','lfo','hz'), division = h.root.addTiming('division','lfo','straight-4'), select = h.root.addSelect('lfoShape','sine');
    h.module.bind();
    if (replacement === 'project') h.state.project = copy(h.state.project);
    if (replacement === 'instrument') h.state.project.instruments[0] = {...h.synth};
    if (replacement === 'detached') h.root.isConnected = false;
    const before = copy(h.state.project);
    input.value = '24'; input.dispatch('input'); input.dispatch('change');
    mode.value = 'sync'; mode.dispatch('change'); division.value = 'straight-8'; division.dispatch('change'); select.value = 'square'; select.dispatch('change');
    assert.deepEqual(h.state.project,before,replacement + ' must not accept a stale edit');
    assert.equal(h.mutations.length,0); assert.equal(h.timers.size,0);
  }
});

test('refused modulation edits restore the visible control and its twin', () => {
  const h = harness(), [input, slider] = h.root.addParameter('filterEnvAmount',-12);
  h.ST.mutate = () => false; h.module.bind();
  input.value = '24'; input.dispatch('input'); input.dispatch('change');
  assert.equal(h.synth.filterEnvAmount,-12);
  assert.equal(input.value,'-12'); assert.equal(slider.value,'-12');
  assert.equal(h.timers.size,0);
});

test('native defaults and automation units match the engine contract', () => {
  const h = harness(), M = h.module;
  assert.deepEqual(copy(M.defaults), { filterAttack: .005, filterDecay: .2, filterSustain: 0, filterRelease: .2, filterEnvAmount: 0, lfoRate: 0, lfoSync: false, lfoBeats: 1, lfoDepth: 0, lfoTarget: 'cutoff', lfoShape: 'sine', lfo2Rate: 0, lfo2Sync: false, lfo2Beats: 1, lfo2Depth: 0, lfo2Target: 'cutoff', lfo2Shape: 'sine' });
  assert(Object.isFrozen(M.defaults));
  const descriptors = M.targets(0), keys = descriptors.map(d => d.target);
  assert.equal(descriptors.length, 11); assert.equal(new Set(keys).size, 11);
  const target = key => descriptors.find(d => d.target === key);
  assert.equal(target('filterAttack').value, .005);
  assert.equal(target('filterEnvAmount').value, -12);
  assert.equal(target('filterEnvAmount').min, -48); assert.equal(target('filterEnvAmount').max, 48);
  assert.equal(target('lfo2Rate').max, 100); assert.equal(target('lfo2Depth').value, .4); assert.equal(target('lfo2Depth').max, 1);
  assert.equal(target('lfoDepth').format(.25), '25%'); assert.equal(target('filterAttack').format(.005), '5 ms');
  assert.equal(target('lfoBeats').value, 1); assert.equal(target('lfoBeats').min, 1 / 64); assert.equal(target('lfoBeats').max, 64);
  assert.equal(target('lfoBeats').format(1), '1 beat'); assert.equal(target('lfo2Beats').format(1 / 64), '0.015625 beats');
});

test('unsupported instruments and missing tracks expose no modulation targets', () => {
  const { module: M, state } = harness();
  assert.equal(M.targets(1).length, 11);
  for (const track of [2, 3, -1, 99]) assert.equal(M.targets(track).length, 0);
  assert(M.supports('filterEnvAmount')); assert(M.supports('lfo2Depth', 'sample'));
  assert(!M.supports('lfo2Depth', 'au')); assert(!M.supports('plugin:effect:0:1')); assert(!M.supports('lfo2Shape'));
  assert.equal(M.render(state.project.instruments[2]), ''); assert.equal(M.render(state.project.instruments[3]), '');
});

test('rendered controls preserve native values for the global automation controller', () => {
  const h = harness(), html = h.module.render(h.synth);
  // Depth, rate and times use perceptual sliders (0..1 positions); the number fields carry the value.
  assertControl(html, 'mod-0-lfoDepth', { min: 0, max: 1, value: h.position('lfoDepth', .25), 'data-automate': 'lfoDepth', 'data-fine-knee': .02, 'data-fine-min': 0, 'data-fine-max': 1 });
  assertControl(html, 'mod-number-0-lfoDepth', { min: 0, max: 100, value: 25, step: 'any', 'data-fine-scale': 100, 'data-automate': 'lfoDepth' });
  assertControl(html, 'mod-0-filterEnvAmount', { min: -48, max: 48, value: -12 });
  assert(!/id="mod-0-filterEnvAmount"[^>]*data-fine-knee/.test(html), 'Bipolar envelope amount stays linear');
  assertControl(html, 'mod-0-filterAttack', { min: 0, max: 1, value: h.position('filterAttack', .005), 'data-fine-max': 60 });
  assertControl(html, 'mod-number-0-filterAttack', { value: .005, max: 60 });
  const ids = [...html.matchAll(/\sid="([^"]+)"/g)].map(match => match[1]);
  assert.equal(new Set(ids).size, ids.length);
  assert(html.includes('Amount 0 leaves the filter unchanged'));
  assert(html.includes('Depth 0 disables modulation'));
  // Every native instrument is a sampler now: Position (wavetable / grain position) is a destination everywhere.
  assert(html.includes('<option value="position" >Position</option>') || /<option value="position"[^>]*>Position<\/option>/.test(html));
  assert(h.module.render(h.sampler).includes('<option value="position"'));
  assert(!html.includes('sampler only'));
});

test('preview updates twins and visual feedback without mutating the project or undo history', async () => {
  const h = harness(), [numberInput, slider] = h.root.addParameter('lfoDepth', .25);
  h.module.bind(); slider.value = h.position('lfoDepth', .75); slider.dispatch('input');
  assert.equal(numberInput.value, '75');
  assert.equal(slider.attributes['aria-valuetext'], '75%');
  assert(h.root.visuals.get('[data-mod-wave="lfo"]').innerHTML.includes('75 percent depth'));
  assert.equal(h.synth.lfoDepth, .25); assert.equal(h.mutations.length, 0); assert.equal(h.rerenderCount, 0);
  await h.flushTimers();
  assert.equal(h.requests.length, 1); assert.equal(h.requests[0].command, 'engine.project');
  assert.equal(h.syncCalls, 1);
  assert.equal(h.requests[0].payload.project.instruments[0].lfoDepth, .75);
  assert.equal(h.state.project.instruments[0].lfoDepth, .25);
});

test('rapid input coalesces and change commits once to the original instrument with stable focus', () => {
  const h = harness(), [, slider] = h.root.addParameter('lfo2Depth', .4);
  slider.document = h.document; h.module.bind(); h.module.bind();
  slider.focus(); slider.value = h.position('lfo2Depth', .5); slider.dispatch('input'); slider.value = h.position('lfo2Depth', .8); slider.dispatch('input');
  assert.equal(h.timers.size, 1);
  h.state.instrument = 1; slider.dispatch('change'); h.flushTimers();
  assert.equal(h.requests.length, 0, 'Committed changes must cancel stale preview requests');
  assert.equal(h.mutations.length, 1, 'Rebinding must not duplicate listeners');
  assert.equal(h.state.project.instruments[0].lfo2Depth, .8);
  assert.equal(h.state.project.instruments[1].lfo2Depth, undefined);
  assert.equal(h.document.activeElement.id, 'mod-0-lfo2Depth'); assert(h.document.activeElement.restored);
  assertControl(h.module.render(h.state.project.instruments[0]), 'mod-0-lfo2Depth', { value: h.position('lfo2Depth', .8) });
});

test('pending previews do not leak into a replacement project or detached surface', () => {
  for (const detach of [false, true]) {
    const h = harness(), [, slider] = h.root.addParameter('filterEnvAmount', -12);
    h.module.bind(); slider.value = '24'; slider.dispatch('input');
    if (detach) h.root.isConnected = false;
    else h.state.project = copy(h.state.project);
    h.flushTimers(); assert.equal(h.requests.length, 0);
  }
});

test('clamping, blank numeric input and route changes commit valid native payloads', () => {
  const h = harness(), [input] = h.root.addParameter('filterEnvAmount', -12), shape = h.root.addSelect('lfo2Shape', 'sine'), target = h.root.addSelect('lfo2Target', 'cutoff');
  h.module.bind(); input.value = '90'; input.dispatch('change'); assert.equal(h.synth.filterEnvAmount, 48);
  input.value = ''; input.dispatch('change'); assert.equal(h.synth.filterEnvAmount, 48);
  shape.value = 'random'; shape.dispatch('change'); assert.equal(h.synth.lfo2Shape, 'random');
  target.value = 'pitch'; target.dispatch('change'); assert.equal(h.synth.lfo2Target, 'pitch');
  const before = h.mutations.length; target.value = 'not-a-route'; target.dispatch('change'); assert.equal(h.mutations.length, before);
});

test('native preview errors produce a user-visible error without corrupting project state', async () => {
  const h = harness(), [, slider] = h.root.addParameter('lfoRate', 0);
  h.setRejectRequests(true); h.module.bind(); slider.value = '8'; slider.dispatch('input'); h.flushTimers();
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(h.notices.length, 1); assert.equal(h.notices[0].type, 'error');
  assert.match(h.notices[0].message, /Preview engine unavailable/);
  assert.equal(h.state.project.instruments[0].lfoRate, undefined);
});

test('legacy instruments stay in Hz mode and inactive automation remains available with explicit units', () => {
  const h = harness(); h.synth.lfoRate = 7.25;
  let html = h.module.render(h.synth), targets = h.module.targets(0);
  assert.match(html, /value="hz" selected/); assertControl(html,'mod-number-0-lfoRate',{value:7.25,'data-automate':'lfoRate'});
  assert(!html.includes('id="mod-number-0-lfoBeats"'));
  assert.equal(targets.find(item => item.target === 'lfoRate').inactive,false);
  assert.equal(targets.find(item => item.target === 'lfoBeats').inactive,true);
  h.synth.lfoSync = true; targets = h.module.targets(0); html = h.module.render(h.synth);
  const rate = targets.find(item => item.target === 'lfoRate'), beat = targets.find(item => item.target === 'lfoBeats');
  assert(rate.available && rate.inactive); assert.equal(rate.value,7.25); assert.match(rate.inactiveReason,/Hz value.*preserved/);
  assert(beat.available && !beat.inactive); assert.equal(beat.unit,'beats');
  assert.match(html,/Stored Hz rate: 7.25 Hz · inactive/);
  assertControl(html,'mod-number-0-lfoBeats',{value:1,step:'any','data-automate':'lfoBeats','aria-label':'LFO 1 cycle quarter-note beats per cycle'});
  assert(!h.module.supports('lfoSync')); assert(!h.module.supports('lfo2Sync'));
});

test('mode changes preserve rates and all existing automation, commit once, and retain keyboard focus', () => {
  const h = harness(), mode = h.root.addTiming('mode','lfo','hz');
  h.synth.lfoRate = 4.3; h.synth.lfoBeats = 2 / 3; h.synth.lfo2Sync = true;
  h.state.project.patterns = [{automation:[{target:'lfoRate',points:[{row:0,value:4.3}]},{target:'lfoBeats',points:[{row:0,value:2 / 3}]}]}];
  const lanes = copy(h.state.project.patterns); mode.document = h.document; mode.focus(); h.module.bind(); h.module.bind();
  mode.value = 'sync'; mode.dispatch('change');
  assert.equal(h.synth.lfoSync,true); assert.equal(h.synth.lfoRate,4.3); assert.equal(h.synth.lfoBeats,2 / 3); assert.equal(h.synth.lfo2Sync,true);
  assert.deepEqual(h.state.project.patterns,lanes); assert.equal(h.mutations.length,1); assert.equal(h.document.activeElement.id,mode.id);
  mode.value = 'hz'; mode.dispatch('change'); assert.equal(h.synth.lfoSync,false); assert.equal(h.synth.lfoRate,4.3); assert.equal(h.synth.lfoBeats,2 / 3);
  mode.value = 'invalid'; mode.dispatch('change'); assert.equal(h.mutations.length,2);
});

test('note divisions provide exact straight, dotted and triplet quarter-beat periods without meter assumptions', () => {
  const h = harness(), division = h.root.addTiming('division','lfo2','straight-4'); h.synth.lfo2Sync = true; h.synth.lfo2Rate = 9;
  h.state.project.timeSignature = {numerator:5,denominator:8}; h.module.bind();
  for (const [choice,expected] of [['straight-4',1],['dotted-8',.75],['triplet-4',2 / 3],['straight-256',1 / 64],['whole-16',64]]) {
    const before = h.mutations.length; division.value = choice; division.dispatch('change');
    assert.equal(h.synth.lfo2Beats,expected); assert.equal(h.synth.lfo2Rate,9); assert.equal(h.synth.lfoBeats,undefined); assert.equal(h.mutations.length,before+1);
    const html = h.module.render(h.synth);
    assertControl(html,'mod-number-0-lfo2Beats',{value:expected}); assert(html.includes(`value="${choice}" selected`));
    assert(!/\bbar(s)?\b/i.test(html));
  }
  const html = h.module.render(h.synth); for (const group of ['Straight','Dotted','Triplet']) assert(html.includes(`<optgroup label="${group}">`));
  const before = h.mutations.length;
  for (const invalid of ['triplet-256','whole-32','custom','nonsense']) { division.value = invalid; division.dispatch('change'); }
  assert.equal(h.mutations.length,before);
});

test('custom beat input retains full precision and updates division feedback without replacing focus', async () => {
  const h = harness(), [input,twin] = h.root.addParameter('lfoBeats',1), division = h.root.addTiming('division','lfo','straight-4'); h.synth.lfoSync = true;
  h.module.bind(); input.document = h.document; input.focus(); input.value = String(2 / 3); input.dispatch('input');
  assert.equal(twin.value,String(2 / 3)); assert.equal(division.value,'triplet-4'); assert.equal(h.rerenderCount,0); assert.equal(h.document.activeElement,input);
  await h.flushTimers(); assert.equal(h.requests[0].payload.project.instruments[0].lfoBeats,2 / 3); assert.equal(h.synth.lfoBeats,undefined);
  input.dispatch('change'); assert.equal(h.synth.lfoBeats,2 / 3); assert.equal(h.mutations.length,1);
  input.value = '.123456789'; input.dispatch('input'); assert.equal(division.value,'custom');
  input.dispatch('change'); assert.equal(h.synth.lfoBeats,.123456789);
  assert.match(h.module.render(h.synth),/value="custom" selected disabled/);
});

test('beat keyboard increments preserve triplet values, clamp safely, and leave automation shortcuts untouched', () => {
  const h = harness(), [input] = h.root.addParameter('lfoBeats',2 / 3); h.synth.lfoSync = true; h.module.bind(); input.document = h.document; input.focus();
  let event = input.dispatch('keydown',{key:'ArrowUp'}); assert(event.defaultPrevented); assert.equal(h.synth.lfoBeats,2 / 3 + 1 / 64); assert.equal(h.mutations.length,1); assert.equal(h.timers.size,0);
  input.value = '63.5'; input.dispatch('keydown',{key:'ArrowUp',shiftKey:true}); assert.equal(h.synth.lfoBeats,64);
  input.value = '.02'; input.dispatch('keydown',{key:'ArrowDown'}); assert.equal(h.synth.lfoBeats,1 / 64);
  const before = h.mutations.length;
  for (const fields of [{key:'a',altKey:true},{key:'ArrowUp',altKey:true},{key:'ArrowDown',metaKey:true}]) { event = input.dispatch('keydown',fields); assert(!event.defaultPrevented); }
  assert.equal(h.mutations.length,before);
  for (const invalid of ['', 'not-a-number']) { input.value = invalid; input.dispatch('change'); assert.equal(h.synth.lfoBeats,1 / 64); }
});

test('audio previews wait for native effect moves, clone the completed order and discard stale work', async () => {
  for (const cancel of ['none','commit','supersede','detach','project','instrument']) {
    const h = harness(), [input] = h.root.addParameter('lfoBeats',1);
    h.state.project.tracks[0].effects = [{id:'first'},{id:'second'}];
    let release; const gate = new Promise(resolve => release = resolve);
    h.window.STStudio = {isMovingEffect:true,waitForEffectMove:()=>gate};
    h.module.bind(); input.value = '.75'; input.dispatch('input'); h.flushTimers();
    assert.equal(h.requests.length,0,'No stale project may be sent while a move is pending');
    h.state.project.tracks[0].effects.reverse();
    if (cancel === 'commit') input.dispatch('change');
    if (cancel === 'supersede') {input.value = '2'; input.dispatch('input'); h.flushTimers();}
    if (cancel === 'detach') h.root.isConnected = false;
    if (cancel === 'project') h.state.project = copy(h.state.project);
    if (cancel === 'instrument') h.state.project.instruments[0] = {type:'sample'};
    h.window.STStudio.isMovingEffect = false; release(); await new Promise(resolve => setImmediate(resolve));
    if (cancel === 'none' || cancel === 'supersede') {
      assert.equal(h.requests.length,1);
      assert.deepEqual(h.requests[0].payload.project.tracks[0].effects.map(effect=>effect.id),['second','first']);
      assert.equal(h.requests[0].payload.project.instruments[0].lfoBeats,cancel === 'none' ? .75 : 2);
    } else assert.equal(h.requests.length,0,`Waiting ${cancel} preview must be discarded`);
  }
});

test('modulation builds its preview after pending plug-in edits and keeps newer state without committing the draft', async () => {
  const h = harness(), [input] = h.root.addParameter('lfoDepth', .25); let release;
  const pending = new Promise(resolve => release = resolve); h.state.project.audioPlugins = [{ instanceID: 'keys', state: 'old' }];
  h.pendingEdits(async () => { await pending; h.state.project.audioPlugins[0].state = 'new'; });
  h.module.bind(); input.value = '80'; input.dispatch('input'); const run = h.flushTimers();
  assert.equal(h.requests.length, 0); release(); await run;
  assert.equal(h.syncCalls, 1); assert.equal(h.requests[0].payload.project.audioPlugins[0].state, 'new');
  assert.equal(h.requests[0].payload.project.instruments[0].lfoDepth, .8); assert.equal(h.synth.lfoDepth, .25); assert.equal(h.mutations.length, 0);
});

test('pending control capture cannot revive a committed, detached, replaced, or superseded modulation preview', async () => {
  for (const change of ['commit', 'detach', 'project', 'instrument', 'supersede', 'error']) {
    const h = harness(), [input] = h.root.addParameter('lfoDepth', .25); let release, reject;
    const pending = new Promise((resolve, no) => { release = resolve; reject = no; }); h.pendingEdits(() => pending);
    h.module.bind(); input.value = '70'; input.dispatch('input'); const first = h.flushTimers();
    if (change === 'commit') input.dispatch('change'); if (change === 'detach') h.root.isConnected = false;
    if (change === 'project') h.state.project = copy(h.state.project); if (change === 'instrument') h.state.project.instruments[0] = { type: 'sample' };
    let second; if (change === 'supersede') { input.value = '90'; input.dispatch('input'); second = h.flushTimers(); }
    if (change === 'error') reject(new Error('Control capture failed')); else release(); await first; if (second) await second;
    assert.equal(h.requests.length, change === 'supersede' ? 1 : 0, change);
    if (change === 'supersede') assert.equal(h.requests[0].payload.project.instruments[0].lfoDepth, .9);
    assert.equal(h.notices.length, change === 'error' ? 1 : 0, change);
  }
});

test('perceptual sliders give small rates, times and depths most of the travel without changing stored values', () => {
  const h = harness(), F = h.fine, targets = h.module.targets(0), spec = key => targets.find(item => item.target === key);
  const rate = spec('lfoRate'), depth = spec('lfoDepth'), attack = spec('filterAttack');
  assert(F.fromPosition(.5, rate) < 3, 'Half the rate slider stays below 3 Hz');
  assert(F.fromPosition(.25, rate) < .5, 'A quarter of the rate slider stays below 0.5 Hz');
  assert(F.fromPosition(.5, depth) < .15, 'Half the depth slider stays below 15%');
  assert(F.fromPosition(.5, attack) < 1, 'Half the attack slider stays below one second');
  for (const item of [rate, depth, attack]) { assert.equal(F.fromPosition(0, item), item.min); assert.equal(F.fromPosition(1, item), item.max); }
  // Older projects: stored values survive render → slider position → value exactly, and opening edits nothing.
  Object.assign(h.synth, { lfoRate: 7.25, lfo2Rate: .05, lfoDepth: .0125, filterAttack: .005, filterRelease: 1.5 });
  const before = copy(h.synth), html = h.module.render(h.synth);
  for (const [key, value] of [['lfoRate', 7.25], ['lfo2Rate', .05], ['lfoDepth', .0125], ['filterAttack', .005], ['filterRelease', 1.5]]) {
    const position = Number(html.match(new RegExp(`id="mod-0-${key}"[^>]*value="([^"]+)"`))[1]);
    assert.equal(F.quantize(F.fromPosition(position, spec(key)), spec(key)), value, key);
  }
  const [, slider] = h.root.addParameter('lfoDepth', .0125); h.module.bind();
  assert.deepEqual(copy(h.synth), before); assert.equal(h.mutations.length, 0);
  assert.equal(slider.dataset.fineDrag, 'true', 'Sliders take over drags for ⌥/⇧ fine control');
});

test('values read naturally at small magnitudes', () => {
  const targets = harness().module.targets(0), format = (key, value) => targets.find(item => item.target === key).format(value);
  assert.equal(format('lfoRate', .05), '0.05 Hz'); assert.equal(format('lfoRate', 1.25), '1.25 Hz'); assert.equal(format('lfoRate', 12.5), '12.5 Hz'); assert.equal(format('lfoRate', 0), '0 Hz');
  assert.equal(format('lfoDepth', .0125), '1.25%'); assert.equal(format('lfoDepth', .001), '0.1%'); assert.equal(format('lfoDepth', 1), '100%');
  assert.equal(format('filterAttack', .0005), '0.5 ms'); assert.equal(format('filterAttack', .125), '125 ms'); assert.equal(format('filterDecay', 2.5), '2.5 s');
  assert.equal(format('filterEnvAmount', .25), '+0.25 st');
  const h = harness(); h.synth.lfoSync = true; h.synth.lfoRate = .05;
  assert.match(h.module.render(h.synth), /Stored Hz rate: 0.05 Hz · inactive/);
});

test('arrow keys step relative to magnitude, ⌥ finer, ⇧ coarser, one Undo step per burst', async () => {
  const h = harness(), [, slider] = h.root.addParameter('lfoRate', 0), [depthNumber] = h.root.addParameter('lfoDepth', 1.25);
  h.synth.lfoRate = 1.25; h.synth.lfoDepth = .0125; slider.value = h.position('lfoRate', 1.25); h.module.bind();
  const press = (input, fields) => input.dispatch('keydown', fields);
  let event = press(slider, { key: 'ArrowUp' }); assert(event.defaultPrevented);
  assert.equal(slider.attributes['aria-valuetext'], '1.3 Hz'); assert.equal(h.mutations.length, 0, 'Each press previews only');
  press(slider, { key: 'ArrowUp', altKey: true }); assert.equal(slider.attributes['aria-valuetext'], '1.31 Hz');
  press(slider, { key: 'ArrowRight', shiftKey: true }); assert.equal(slider.attributes['aria-valuetext'], '2 Hz');
  press(slider, { key: 'ArrowDown' }); assert.equal(slider.attributes['aria-valuetext'], '1.9 Hz');
  assert.equal(h.mutations.length, 0); await h.flushTimers();
  assert.equal(h.mutations.length, 1, 'A burst commits one Undo step'); assert.equal(h.synth.lfoRate, 1.9);
  assert.equal(h.mutations[0].label, 'Adjust lfo 1 rate');
  // Toward zero uses the decade below: 1 → 0.99, then ⌥ 0.999-style steps at small values.
  h.synth.lfoRate = 1; slider.value = h.position('lfoRate', 1); press(slider, { key: 'ArrowDown' }); slider.dispatch('blur');
  assert.equal(h.synth.lfoRate, .99, 'Blur ends the burst'); assert.equal(h.mutations.length, 2);
  h.synth.lfoRate = .05; slider.value = h.position('lfoRate', .05);
  press(slider, { key: 'ArrowUp', altKey: true }); press(slider, { key: 'ArrowUp', altKey: true }); press(slider, { key: 'a' });
  assert.equal(h.synth.lfoRate, .052, 'Another key ends the burst'); assert.equal(h.mutations.length, 3);
  // Number fields step in display units: 1.25% → 1.3%, ⌥ 1.26%.
  press(depthNumber, { key: 'ArrowUp' }); assert.equal(depthNumber.value, '1.3');
  press(depthNumber, { key: 'ArrowDown', altKey: true }); assert.equal(depthNumber.value, '1.29');
  depthNumber.dispatch('blur'); assert.equal(h.synth.lfoDepth, .0129); assert.equal(h.mutations.length, 4);
  // Clamped at the range ends; ⌘ / ⌃ and caret keys are left alone.
  h.synth.lfoDepth = 0; depthNumber.value = '0'; press(depthNumber, { key: 'ArrowDown' }); depthNumber.dispatch('blur'); assert.equal(h.synth.lfoDepth, 0);
  for (const fields of [{ key: 'ArrowUp', metaKey: true }, { key: 'ArrowUp', ctrlKey: true }, { key: 'ArrowLeft' }]) assert(!press(depthNumber, fields).defaultPrevented);
});

test('exact values can be typed in natural units and older automation keeps engine units', async () => {
  const h = harness(), [rate] = h.root.addParameter('lfoRate', 0), [depth] = h.root.addParameter('lfoDepth', 25), [release] = h.root.addParameter('filterRelease', .2);
  h.module.bind();
  rate.value = '0.05'; rate.dispatch('change'); assert.equal(h.synth.lfoRate, .05);
  depth.value = '1.25'; depth.dispatch('input'); await h.flushTimers();
  assert.equal(h.requests.at(-1).payload.project.instruments[0].lfoDepth, .0125, 'Percent fields preview 0–1 engine values');
  depth.dispatch('change'); assert.equal(h.synth.lfoDepth, .0125);
  release.value = '0.0005'; release.dispatch('change'); assert.equal(h.synth.filterRelease, .0005, 'Sub-millisecond release remains within the DSP range');
  release.value='0';release.dispatch('change');assert.equal(h.synth.filterRelease,.0001,'Engine minimum still clamps');
  const descriptor = h.module.targets(0).find(item => item.target === 'lfoDepth');
  assert.deepEqual([descriptor.min, descriptor.max, descriptor.value], [0, 1, .0125], 'Automation lanes keep 0–1 depth');
  assert.deepEqual(copy(descriptor.fineCurve), { knee: .02, floor: .01 }, 'MIDI control follows the on-screen curve');
  assert.equal(h.module.targets(0).find(item => item.target === 'filterEnvAmount').fineCurve, undefined);
});

(async () => {
  for (const { name, run } of tests) { await run(); process.stdout.write(`PASS ${name}\n`); }
  process.stdout.write(`${tests.length} modulation UI suites passed.\n`);
})().catch(error => { console.error(error); process.exitCode = 1; });
