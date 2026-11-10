// Real clip module with the real timing map; no native/audio calls.
'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const timing = require('../ui/project-timing.js');
const source = web.source('clip-arranger.js');
const copy = value => JSON.parse(JSON.stringify(value));
function harness() {
  const nodes = new Map(), changes = [];
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null,
    querySelector: selector => [...nodes.values()].find(node => node.matches(selector)) || null,
    querySelectorAll: selector => [...nodes.values()].filter(node => node.matches(selector)) };
  function node(id, parent = null, tag = 'BUTTON', dataset = {}, classes = []) {
    const item = { id, parentElement: parent, tagName: tag, dataset, classes, handlers: {}, disabled: false, scrolls: 0, style: {},
      matches(selector) { return selector.split(',').some(part => { const s = part.trim(); if (s[0] === '#') return this.id === s.slice(1); if (s[0] === '.') return this.classes.includes(s.slice(1)); if (s.startsWith('[data-')) { const key = s.slice(6, -1).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()); return this.dataset[key] !== undefined; } return this.tagName === s.toUpperCase(); }); },
      closest(selector) { for (let current = this; current; current = current.parentElement) if (current.matches(selector)) return current; return null; },
      addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); },
      fire(type, extra = {}) { const event = { target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() {}, ...extra }; for (const fn of this.handlers[type] || []) fn(event); return event; },
      focus() { const changed = document.activeElement !== this; document.activeElement = this; if (changed) this.fire('focus'); },
      scrollIntoView() { this.scrolls++; } };
    nodes.set(id, item); return item;
  }
  document.body = node('body', null, 'BODY');
  const panel = node('panel', document.body, 'SECTION', {}, ['clips-panel']);
  const timeline = node('clips-timeline', panel, 'DIV');
  for (const id of ['clips-import', 'clips-cursor-position', 'clips-import-cursor', 'clips-bounce', 'clips-zoom-in', 'clips-zoom-out', 'clips-preview', 'clips-stop-preview', 'clips-source', 'clips-duplicate', 'clips-split', 'clips-delete']) node(id, panel);
  node('clips-cursor', panel, 'INPUT'); node('clips-name', panel, 'INPUT'); node('clips-track', panel, 'SELECT'); node('clips-snap', panel, 'SELECT');
  node('track-label', timeline, 'BUTTON', { clipsTrack: '0' });
  const state = { view: 'song', track: 0, clipSelected: 'a', sequence: 0, status: {}, project: { bpm: 120, linesPerBeat: 4,
    tracks: [{ name: 'One' }, { name: 'Two' }, { name: 'Three' }], patterns: [{ id: 0, length: 16, steps: [] }], sequence: [0],
    audioClips: [{ id: 'a', track: 0, startBeat: 4, durationBeats: 4, sourceOffset: 0, path: '/fixture/a.wav' }, { id: 'b', track: 0, startBeat: 8, durationBeats: 4, sourceOffset: 0, path: '/fixture/b.wav' }, { id: 'c', track: 2, startBeat: 1, durationBeats: 4, sourceOffset: 0, path: '/fixture/c.wav' }] } };
  for (const clip of state.project.audioClips) node('clip-' + clip.id, timeline, 'BUTTON', { clipId: clip.id });
  const ST = { state, refresh() {}, escapeHTML: value => String(value).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])), toast() {}, mutate(label, fn) { changes.push(label); fn(state.project); }, request() { throw Error('No native request is needed for clip navigation'); } };
  const context = vm.createContext({ window: { ST, STProjectTiming: timing }, document, queueMicrotask() {}, setTimeout() {}, clearTimeout() {} });
  for (const name of ['clip-takes-project.js','clip-takes.js']) web.load(context, name);
  web.load(context, 'clip-arranger.js', { text: source });
  const clips = context.window.STClips; clips.bind();
  function key(target, key, options = {}) { target = typeof target === 'string' ? nodes.get(target) : target; document.activeElement = target; const event = { target, key, preventDefault() { this.defaultPrevented = true; }, ...options }; event.handled = clips.key(event); return event; }
  node('clips-playhead', timeline, 'DIV');
  return { clips, state, document, nodes, node, changes, key, context, panel, timeline, ST };
}
const tests=[],test=(name,run)=>tests.push({name,run});
function mapped(h){h.state.project.timeSignature={numerator:4,denominator:4};h.state.project.timing={version:1,tempos:[{id:'t',beat:0,bpm:120}],meters:[{id:'m0',beat:0,numerator:4,denominator:4},{id:'m1',beat:.75,numerator:3,denominator:4},{id:'m2',beat:4.25,numerator:6,denominator:8}]};return h;}
function barSnap(h){const select=h.nodes.get('clips-snap');select.value='bar';select.fire('change');}
function status(h,values){const payload={playing:true,sequence:0,row:0,position:0,...values};h.state.status=payload;h.state.playing=payload.playing;h.clips.event('status',payload);return payload;}
function left(h){return parseFloat(h.nodes.get('clips-playhead').style.left);}
test('ruler and clip labels represent off-bar meter changes, shortened bars and denominator beats',()=>{
 const h=mapped(harness());h.state.project.audioClips[0].startBeat=.75;h.state.project.audioClips[1].startBeat=4.75;
 const html=h.clips.render();for(const pair of [[1,0],[2,.75],[3,3.75],[4,4.25],[5,7.25]])assert(html.includes(`data-bar="${pair[0]}" data-beat="${pair[1]}"`),pair.join(':'));
 assert(html.includes('Bar 2 · beat 1 · 3/4'));assert(html.includes('Bar 3 · beat 1 · 3/4 · shortened bar'));assert(html.includes('Bar 4 · beat 2 · 6/8'));
 assert(html.includes('left:16.5px;width:77px;--clips-beat:22px;--clips-bar:66px'));assert(html.includes('--clips-beat:11px;--clips-bar:66px'));
 assert(!html.includes('starts bar Bar'));assert.equal(h.changes.length,0);
});
test('labels immediately before a boundary stay in the preceding short bar',()=>{
 const h=mapped(harness());h.state.project.audioClips[0].startBeat=.75-Number.EPSILON;let html=h.clips.render();assert(html.includes('Bar 1 · beat 1.749 · 4/4 · shortened bar'));
 h.state.project.audioClips[0].startBeat=.75;html=h.clips.render();assert(html.includes('Bar 2 · beat 1 · 3/4'));
});
test('bar nudges visit actual next and previous boundaries across short bars in one edit each',()=>{
 const h=mapped(harness()),clip=h.state.project.audioClips[0];clip.startBeat=.75;clip.durationBeats=.25;barSnap(h);
 for(const [key,expected] of [['ArrowRight',3.75],['ArrowRight',4.25],['ArrowLeft',3.75],['ArrowLeft',.75],['ArrowLeft',0]]){h.key('clip-a',key);assert.equal(clip.startBeat,expected);assert.equal(clip.durationBeats,.25);assert.equal(h.document.activeElement.id,'clip-a');}
 assert.equal(h.changes.length,5);
});
test('bar trim operates on the clip end while fractional quarter-note snapping stays unchanged',()=>{
 const h=mapped(harness()),clip=h.state.project.audioClips[0];clip.startBeat=.75;clip.durationBeats=3;barSnap(h);h.key('clip-a','ArrowRight',{shiftKey:true});assert.equal(clip.durationBeats,3.5);h.key('clip-a','ArrowLeft',{shiftKey:true});assert.equal(clip.durationBeats,3);
 const select=h.nodes.get('clips-snap');select.value='.25';select.fire('change');h.key('clip-a','ArrowRight');assert.equal(clip.startBeat,1);assert.equal(clip.durationBeats,3);
});
test('From playhead snaps to the closest map boundary and cursor brackets follow bars',()=>{
 const h=mapped(harness());barSnap(h);status(h,{beat:.4});h.nodes.get('clips-cursor-position').fire('click');assert(h.clips.render().includes('value="0.75" aria-label="Edit cursor'));
 h.key('clips-timeline',']');assert(h.clips.render().includes('value="3.75" aria-label="Edit cursor'));h.key('clips-timeline',']');assert(h.clips.render().includes('value="4.25" aria-label="Edit cursor'));h.key('clips-timeline','[');assert(h.clips.render().includes('value="3.75" aria-label="Edit cursor'));
 assert.equal(h.changes.length,0);
});
test('owned native absolute beat is used without adding the pattern occurrence twice or repainting',()=>{
 const h=mapped(harness());let repaints=0;h.ST.refresh=()=>repaints++;h.state.project.sequence=[0,0];h.nodes.get('clips-name').focus();status(h,{sequence:1,position:2.5,beat:4.625});assert.equal(left(h),132+4.625*22);assert.equal(repaints,0);assert.equal(h.document.activeElement.id,'clips-name');assert.equal(h.nodes.get('clips-playhead').style.display,'block');
});
test('a packet the arranger did not receive uses fractional sequence position rather than its native beat',()=>{
 const h=mapped(harness());h.state.project.patterns[0].length=4;h.state.project.sequence=[0,0];h.state.playing=true;h.state.status={playing:true,sequence:1,position:1.5,beat:900};assert(h.clips.render().includes(`left:${132+1.375*22}px;display:block`));
});
test('project replacement, in-place timing edits and loaded events invalidate native beat ownership',()=>{
 const h=mapped(harness());h.state.playing=true;status(h,{position:2,beat:99});assert.equal(left(h),132+99*22);
 h.state.project=copy(h.state.project);assert(h.clips.render().includes('left:143px;display:block'));
 status(h,{position:2,beat:99});h.state.project.timing.meters[1].numerator=5;assert(h.clips.render().includes('left:143px;display:block'));
 status(h,{position:2,beat:99});h.clips.event('project.loaded');assert(h.clips.render().includes('left:143px;display:block'));
});
test('mismatched pattern occurrence falls back to current selection and stopped packets hide the line',()=>{
 const h=mapped(harness());h.state.sequence=0;h.state.row=3;status(h,{pattern:99});assert.equal(left(h),132+.75*22);status(h,{playing:false});assert.equal(h.nodes.get('clips-playhead').style.display,'none');
});
test('long timelines bound ruler markup without losing meter-change markers',()=>{
 const h=mapped(harness());h.state.project.audioClips.push({id:'long',track:1,path:'/fixture/long.wav',startBeat:999995,durationBeats:4});const html=h.clips.render(),ticks=[...html.matchAll(/data-bar="/g)];assert(ticks.length<4096);assert(ticks.length>1000);assert(html.includes('data-beat="0.75"'));assert(html.includes('data-beat="4.25"'));assert.equal((html.match(/class="clips-meter-segment"/g)||[]).length,3);
});
test('malformed maps preserve stored clips and render a clear error instead of crashing or guessing a meter',()=>{
 const h=mapped(harness());h.state.project.timing.meters[1].denominator=3;const before=copy(h.state.project);const html=h.clips.render();assert(html.includes('role="alert"'));assert(html.includes('valid tempo map'));assert.deepEqual(h.state.project,before);assert.equal(h.changes.length,0);
 h.state.project.timing.meters[1].denominator=4;assert(h.clips.render().includes('data-bar="2" data-beat="0.75"'));
});
test('cache follows project, in-place timing edits and replacement helper dependencies',()=>{
 const h=mapped(harness());let first=0,second=0;h.context.window.STProjectTiming={...timing,create:p=>{first++;return timing.create(p);}};h.clips.render();h.clips.render();assert.equal(first,1);
 h.state.project.timing.meters[1].numerator=5;h.clips.render();assert.equal(first,2);
 h.context.window.STProjectTiming={...timing,create:p=>{second++;return timing.create(p);}};h.clips.render();assert.equal(second,1);
 h.state.project=copy(h.state.project);h.clips.render();assert.equal(second,2);
});
test('legacy meters keep their original ruler, extent and quarter-beat storage without a timing object',()=>{
 const h=harness();let html=h.clips.render();assert(html.includes('data-bar="2" data-beat="4"'));assert(html.includes('class="clips-ruler" style="width:440px"'));assert.equal(h.state.project.timing,undefined);
 h.state.project.timeSignature={numerator:6,denominator:8};html=h.clips.render();assert(html.includes('data-bar="2" data-beat="3"'));assert(html.includes('class="clips-ruler" style="width:462px"'));assert.equal(h.state.project.timing,undefined);
 delete h.context.window.STProjectTiming;html=h.clips.render();assert(html.includes('data-bar="2" data-beat="3"'));
});
for(const {name,run} of tests){run();console.log('PASS '+name);}console.log(`${tests.length} clip meter groups passed.`);
