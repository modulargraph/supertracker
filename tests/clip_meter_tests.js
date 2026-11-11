// The real clip module with the real project timing; no host or audio calls.
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
function meter(h,numerator,denominator){h.state.project.timeSignature={numerator,denominator};return h;}
function barSnap(h){const select=h.nodes.get('clips-snap');select.value='bar';select.fire('change');}
function status(h,values){const payload={playing:true,sequence:0,row:0,position:0,...values};h.state.status=payload;h.state.playing=payload.playing;h.clips.event('status',payload);return payload;}
function left(h){return parseFloat(h.nodes.get('clips-playhead').style.left);}
test('ruler, grid and clip labels follow the project meter and count beats in its denominator',()=>{
 const h=meter(harness(),6,8);h.state.project.audioClips[0].startBeat=3;h.state.project.audioClips[1].startBeat=4.5;
 const html=h.clips.render();for(const pair of [[1,0],[2,3],[3,6],[4,9]])assert(html.includes(`data-bar="${pair[0]}" data-beat="${pair[1]}"`),pair.join(':'));
 assert(html.includes('Bar 2 · beat 1 · 6/8'));assert(html.includes('Bar 2 · beat 4 · 6/8'));
 assert(html.includes('class="clips-meter-grid" aria-hidden="true" style="width:462px;--clips-beat:11px;--clips-bar:66px"'));
 assert.equal(h.changes.length,0);
});
test('the first bar number carries the meter when there is room for it',()=>{
 const h=harness();let html=h.clips.render();assert(html.includes('class="clips-ruler-first"'));assert.equal((html.match(/<small>4\/4<\/small>/g)||[]).length,1);assert(html.includes('>1<small>4/4</small></span>'));
 meter(h,6,8);html=h.clips.render();assert(!html.includes('<small>'),'a 66 pixel bar has no room for the meter');
});
test('a position immediately before a bar line stays in the preceding bar',()=>{
 const h=meter(harness(),6,8);h.state.project.audioClips[0].startBeat=3-Number.EPSILON*2;let html=h.clips.render();assert(html.includes('Bar 1 · beat 6.999 · 6/8'));
 h.state.project.audioClips[0].startBeat=3;html=h.clips.render();assert(html.includes('Bar 2 · beat 1 · 6/8'));
});
test('bar nudges visit the next and previous bar lines in one edit each',()=>{
 const h=meter(harness(),3,4),clip=h.state.project.audioClips[0];clip.startBeat=3.5;clip.durationBeats=.25;barSnap(h);
 for(const [key,expected] of [['ArrowRight',6],['ArrowRight',9],['ArrowLeft',6],['ArrowLeft',3],['ArrowLeft',0]]){h.key('clip-a',key);assert.equal(clip.startBeat,expected);assert.equal(clip.durationBeats,.25);assert.equal(h.document.activeElement.id,'clip-a');}
 assert.equal(h.changes.length,5);h.key('clip-a','ArrowLeft');assert.equal(clip.startBeat,0);assert.equal(h.changes.length,5,'a nudge at the song start is not an edit');
});
test('bar trim operates on the clip end while fractional quarter-note snapping stays unchanged',()=>{
 const h=meter(harness(),3,4),clip=h.state.project.audioClips[0];clip.startBeat=.75;clip.durationBeats=3;barSnap(h);h.key('clip-a','ArrowRight',{shiftKey:true});assert.equal(clip.durationBeats,5.25);h.key('clip-a','ArrowLeft',{shiftKey:true});assert.equal(clip.durationBeats,2.25);
 const select=h.nodes.get('clips-snap');select.value='.25';select.fire('change');h.key('clip-a','ArrowRight');assert.equal(clip.startBeat,1);assert.equal(clip.durationBeats,2.25);
});
test('From playhead snaps to the closest bar line and cursor brackets follow bars',()=>{
 const h=meter(harness(),3,4);barSnap(h);status(h,{beat:2});h.nodes.get('clips-cursor-position').fire('click');assert(h.clips.render().includes('value="3" aria-label="Edit cursor'));
 h.key('clips-timeline',']');assert(h.clips.render().includes('value="6" aria-label="Edit cursor'));h.key('clips-timeline',']');assert(h.clips.render().includes('value="9" aria-label="Edit cursor'));h.key('clips-timeline','[');assert(h.clips.render().includes('value="6" aria-label="Edit cursor'));
 assert.equal(h.changes.length,0);
});
test('the absolute beat of a received status is used without adding the pattern occurrence twice or repainting',()=>{
 const h=harness();let repaints=0;h.ST.refresh=()=>repaints++;h.state.project.sequence=[0,0];h.nodes.get('clips-name').focus();status(h,{sequence:1,position:2.5,beat:4.625});assert.equal(left(h),132+4.625*22);assert.equal(repaints,0);assert.equal(h.document.activeElement.id,'clips-name');assert.equal(h.nodes.get('clips-playhead').style.display,'block');
});
test('a status the arranger did not receive uses the fractional sequence position rather than its beat',()=>{
 const h=harness();h.state.project.patterns[0].length=4;h.state.project.sequence=[0,0];h.state.playing=true;h.state.status={playing:true,sequence:1,position:1.5,beat:900};assert(h.clips.render().includes(`left:${132+1.375*22}px;display:block`));
});
test('project replacement and a tempo or meter edit end the authority of a received beat',()=>{
 const h=harness();h.state.playing=true;status(h,{position:2,beat:99});assert.equal(left(h),132+99*22);
 h.state.project=copy(h.state.project);assert(h.clips.render().includes('left:143px;display:block'));
 status(h,{position:2,beat:99});h.state.project.bpm=121;assert(h.clips.render().includes('left:143px;display:block'));
 status(h,{position:2,beat:99});h.state.project.timeSignature={numerator:5,denominator:4};assert(h.clips.render().includes('left:143px;display:block'));
});
test('mismatched pattern occurrence falls back to current selection and stopped packets hide the line',()=>{
 const h=harness();h.state.sequence=0;h.state.row=3;status(h,{pattern:99});assert.equal(left(h),132+.75*22);status(h,{playing:false});assert.equal(h.nodes.get('clips-playhead').style.display,'none');
});
test('long timelines bound the ruler markup',()=>{
 const h=meter(harness(),3,4);h.state.project.audioClips.push({id:'long',track:1,path:'/fixture/long.wav',startBeat:999995,durationBeats:4});const html=h.clips.render(),ticks=[...html.matchAll(/data-bar="/g)];assert(ticks.length<=2048);assert(ticks.length>1000);assert(html.includes('data-bar="1" data-beat="0"'));assert.equal((html.match(/class="clips-meter-grid"/g)||[]).length,1);
});
test('an arrangement past the supported song length preserves stored clips and renders a clear error',()=>{
 const h=harness();h.state.project.audioClips[0].startBeat=999999;const before=copy(h.state.project);const html=h.clips.render();assert(html.includes('role="alert"'));assert(html.includes('longer than the supported song length'));assert.deepEqual(h.state.project,before);assert.equal(h.changes.length,0);
 h.state.project.audioClips[0].startBeat=4;assert(h.clips.render().includes('data-bar="2" data-beat="4"'));
});
test('the tempo and meter are computed once per project and again after an edit of either',()=>{
 const h=harness();let count=0;h.context.window.STProjectTiming={...timing,create:p=>{count++;return timing.create(p);}};h.clips.render();h.clips.render();assert.equal(count,1);
 h.state.project.timeSignature={numerator:5,denominator:4};h.clips.render();assert.equal(count,2);
 h.state.project.bpm=90;h.clips.render();assert.equal(count,3);
 h.state.project=copy(h.state.project);h.clips.render();h.clips.render();assert.equal(count,4);
});
test('the ruler extent ends on a bar line of the project meter',()=>{
 const h=harness();let html=h.clips.render();assert(html.includes('data-bar="2" data-beat="4"'));assert(html.includes('class="clips-ruler" style="width:440px"'));
 meter(h,6,8);html=h.clips.render();assert(html.includes('data-bar="2" data-beat="3"'));assert(html.includes('class="clips-ruler" style="width:462px"'));
});
for(const {name,run} of tests){run();console.log('PASS '+name);}console.log(`${tests.length} clip meter groups passed.`);
