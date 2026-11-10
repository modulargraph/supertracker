'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = web.source('clip-arranger.js');
const copy = value => JSON.parse(JSON.stringify(value));
function harness() {
  const nodes = new Map(), changes = [];
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null,
    querySelector: selector => [...nodes.values()].find(node => node.matches(selector)) || null,
    querySelectorAll: selector => [...nodes.values()].filter(node => node.matches(selector)) };
  function node(id, parent = null, tag = 'BUTTON', dataset = {}, classes = []) {
    const item = { id, parentElement: parent, tagName: tag, dataset, classes, handlers: {}, disabled: false, scrolls: 0,
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
  const ST = { state, refresh() {}, escapeHTML: String, toast() {}, mutate(label, fn) { changes.push(label); fn(state.project); }, request() { throw Error('No native request is needed for clip navigation'); } };
  const context = vm.createContext({ window: { ST }, document, queueMicrotask() {}, setTimeout() {}, clearTimeout() {} });
  for (const name of ['clip-takes-project.js','clip-takes.js']) web.load(context, name);
  web.load(context, 'clip-arranger.js', { text: source });
  const clips = context.window.STClips; clips.bind();
  function key(target, key, options = {}) { target = typeof target === 'string' ? nodes.get(target) : target; document.activeElement = target; const event = { target, key, preventDefault() { this.defaultPrevented = true; }, ...options }; event.handled = clips.key(event); return event; }
  return { clips, state, document, nodes, node, changes, key, context, panel, timeline };
}
const tests = [];
const test = (name, fn) => tests.push({ name, fn });

test('toolbar, inspector and track-label shortcuts never mutate the selected clip', () => {
  for (const id of ['clips-import', 'clips-cursor-position', 'clips-import-cursor', 'clips-bounce', 'clips-zoom-in', 'clips-zoom-out', 'clips-preview', 'clips-source', 'clips-duplicate', 'clips-split', 'clips-delete', 'track-label']) {
    const h = harness(), before = copy(h.state.project);
    for (const key of ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'Delete', 'Backspace', 's', '[', ']']) {
      const event = h.key(id, key); assert(!event.handled && !event.defaultPrevented, id + ': ' + key);
    }
    assert(!h.key(id, 'd', { metaKey: true }).handled);
    assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
  }
});

test('text, numeric, select and editable descendants keep native editing keys', () => {
  const h = harness(), before = copy(h.state.project);
  for (const tag of ['INPUT', 'TEXTAREA', 'SELECT']) {
    const node = h.node('control-' + tag, h.timeline, tag);
    for (const key of ['ArrowRight', 'ArrowDown', 'Delete', 's']) assert(!h.key(node, key).handled);
  }
  const editable = h.node('editable', h.nodes.get('clip-a'), 'SPAN'); editable.isContentEditable = true;
  assert(!h.key(editable, 'Delete').handled); assert.deepEqual(h.state.project, before);
});

test('focused clip wins over stale selection when focus events are unavailable', () => {
  const h = harness(); const event = h.key('clip-b', 'ArrowRight');
  assert(event.handled && event.defaultPrevented); assert.equal(h.state.clipSelected, 'b');
  assert.equal(h.state.project.audioClips[0].startBeat, 4); assert.equal(h.state.project.audioClips[1].startBeat, 8.25);
  assert.equal(h.document.activeElement.id, 'clip-b'); assert.equal(h.changes[0], 'move audio clip');
});

test('native focus selects the focused clip without dirtying the project', () => {
  const h = harness(), before = copy(h.state.project); h.nodes.get('clip-c').focus();
  assert.equal(h.state.clipSelected, 'c'); assert.equal(h.state.track, 2); assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
});

test('timeline arrows and Home/End navigate clips without editing their audio', () => {
  const h = harness(), before = copy(h.state.project);
  h.key('clips-timeline', 'End'); assert.equal(h.state.clipSelected, 'c');
  h.key('clip-c', 'Home'); assert.equal(h.state.clipSelected, 'a');
  h.key('clip-a', 'ArrowDown'); assert.equal(h.state.clipSelected, 'b');
  h.key('clip-b', 'ArrowUp'); assert.equal(h.state.clipSelected, 'a');
  h.state.clipSelected = null; h.key('clips-timeline', 'ArrowUp'); assert.equal(h.state.clipSelected, 'c');
  h.state.clipSelected = null; h.key('clips-timeline', 'ArrowDown'); assert.equal(h.state.clipSelected, 'a');
  assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0); assert(h.document.activeElement.scrolls > 0);
});

test('clip nudge, trim, slip and track modifiers retain their intended operations', () => {
  const h = harness(), a = h.state.project.audioClips[0];
  h.key('clip-a', 'ArrowRight', { shiftKey: true }); assert.equal(a.durationBeats, 4.25); assert.equal(a.startBeat, 4);
  h.key('clip-a', 'ArrowRight', { altKey: true }); assert.equal(a.sourceOffset, .125);
  h.key('clip-a', 'ArrowDown', { altKey: true }); assert.equal(a.track, 1); assert.equal(h.state.track, 1);
  const span = h.node('clip-title', h.nodes.get('clip-a'), 'SPAN'); h.key(span, 'ArrowLeft'); assert.equal(a.startBeat, 3.75);
  assert.equal(h.changes.length, 4);
});

test('delete and duplicate act on the focused clip rather than the old selection', () => {
  const h = harness(); h.key('clip-b', 'Delete'); assert.deepEqual(copy(h.state.project.audioClips.map(x => x.id)), ['a', 'c']);
  const other = harness(); other.key('clip-b', 'd', { metaKey: true });
  assert.equal(other.state.project.audioClips.length, 4); assert.equal(other.state.project.audioClips[3].startBeat, 12); assert.equal(other.state.project.audioClips[3].path, '/fixture/b.wav');
});

function installActualProjectUndo(h) {
  // Use the production project-only Undo contract, with rendering/native sync stubbed.
  // History does not store the old UI track/clip selection, so Undo preserves the
  // current valid active track while restoring each clip's original track data.
  h.state.undo = []; h.state.redo = []; h.state.instrument = 0; h.state.row = 0;
  h.state.project.instruments = [{}];
  const ST = h.context.window.ST, mutate = ST.mutate;
  ST.mutate = (label, fn) => {
    h.state.undo.push({ label, project: copy(h.state.project) }); h.state.redo = [];
    mutate(label, fn);
  };
  // The app modules run in their own context whose window inherits this harness's window.
  const app = web.loadAppWith(vm.createContext({ window: Object.create(h.context.window) }), {
    state: h.state, clone: copy, clamp: (value, lo, hi) => Math.min(hi, Math.max(lo, value)),
    pattern: () => h.state.project.patterns[0], discardInputCaptures() {}, resetMIDIRoutePublication() {}, syncEngine() {},
    refresh() {}, toast() {}, closeModal() {}, settleRecordedAutomation() { return true; }
  });
  return () => app.applyUndo();
}

test('cross-track delete aligns focus, selection and next import; Undo restores original clip placement', async () => {
  const h = harness(), undo = installActualProjectUndo(h), original = copy(h.state.project);
  h.nodes.get('clip-c').focus(); assert.equal(h.state.track, 2);
  h.key('clip-c', 'Delete');
  assert.equal(h.state.clipSelected, 'a'); assert.equal(h.document.activeElement.id, 'clip-a');
  assert.equal(h.state.track, 0, 'Active track must follow the selected fallback clip');
  assert.deepEqual(h.changes, ['remove audio clip']); assert.equal(h.state.undo.length, 1);
  h.clips.event('audio.clipLoaded', { path: '/new.wav', duration: 1, peaks: [0, 1] });
  const imported = await h.clips.addFile('/new.wav');
  assert.equal(imported.track, 0, 'Next default import must follow visible clip selection');
  undo(); undo();
  assert.deepEqual(copy(h.state.project), original, 'Undo import and deletion must restore all clip data');
  assert.equal(h.state.project.audioClips.find(clip => clip.id === 'c').track, 2);
  assert.equal(h.state.track, 0, 'Project-only Undo keeps the current valid UI track');
});

test('deleting the final clip retains its valid track and focuses the empty timeline', () => {
  const h = harness(); h.state.project.audioClips = [h.state.project.audioClips[2]];
  const undo = installActualProjectUndo(h), original = copy(h.state.project);
  h.nodes.get('clip-c').focus(); h.key('clip-c', 'Delete');
  assert.equal(h.state.project.audioClips.length, 0); assert.equal(h.state.clipSelected, null);
  assert.equal(h.state.track, 2); assert.equal(h.document.activeElement.id, 'clips-timeline');
  assert.equal(h.state.undo.length, 1); undo();
  assert.deepEqual(copy(h.state.project), original); assert.equal(h.state.track, 2);
});

test('split remains scoped to the timeline and preserves source offsets', () => {
  const h = harness(); const cursor = h.nodes.get('clips-cursor'); cursor.value = 6; cursor.fire('change');
  assert(!h.key('clips-split', 's').handled); assert.equal(h.state.project.audioClips.length, 3);
  h.key('clip-a', 's'); assert.equal(h.state.project.audioClips.length, 4);
  assert.equal(h.state.project.audioClips[0].durationBeats, 2); assert.equal(h.state.project.audioClips[3].sourceOffset, 1);
});

test('button activation, Tab and unrelated modified keys remain native', () => {
  const h = harness();
  for (const id of ['clip-a', 'clips-duplicate']) for (const key of ['Enter', ' ', 'Tab']) assert(!h.key(id, key).handled);
  assert(!h.key('clip-a', 'ArrowRight', { metaKey: true }).handled); assert(!h.key('clip-a', 'Delete', { defaultPrevented: true }).handled);
  h.nodes.get('clips-duplicate').fire('click'); assert.equal(h.state.project.audioClips.length, 4);
});

test('actual app dispatcher leaves clip toolbar arrows and duplicate shortcuts outside sequence editing', () => {
  const h = harness(); web.load(h.context, 'app.js');
  const app = h.context.window.ST; app.state.project = copy(h.state.project); app.state.view = 'song'; app.state.modal = null; app.state.keyboardPlay = false; app.state.sequence = 0;
  const before = copy(app.state.project);
  for (const [key, extra] of [['ArrowUp', {}], ['ArrowDown', { altKey: true }], ['Delete', {}], ['d', { metaKey: true }]]) {
    const event = { key, code: key, target: h.nodes.get('clips-preview'), preventDefault() { this.defaultPrevented = true; }, ...extra };
    app.keyboard(event); assert(!event.defaultPrevented, key);
  }
  assert.deepEqual(app.state.project, before);
});


test('PageUp and PageDown choose alternatives of the actually focused clip without moving or trimming it', () => {
 const h=harness(),M=h.context.window.STClipTakesProject,b=h.state.project.audioClips[1];Object.assign(b,M.add(b,{path:'/alternate.wav',sourceDuration:8,name:'Alternate'},.5,()=> 'alt'));
 const pos=b.startBeat,len=b.durationBeats;h.key('clip-b','PageUp');assert.equal(h.state.clipSelected,'b');assert.equal(b.path,'/fixture/b.wav');
 h.key('clip-b','PageDown');assert.equal(b.path,'/alternate.wav');assert.equal(b.startBeat,pos);assert.equal(b.durationBeats,len);assert.equal(h.document.activeElement.id,'clip-b');
 for(const id of ['clips-preview','clips-name','track-label'])assert(!h.key(id,'PageDown').handled);
 assert(!h.key('clip-b','PageDown',{altKey:true}).handled);assert(!h.key('clip-b','PageUp',{metaKey:true}).handled);
});
test('timeline split keeps alternatives and allows the second region to choose another source', () => {
 const h=harness(),M=h.context.window.STClipTakesProject,a=h.state.project.audioClips[0];Object.assign(a,M.add(a,{path:'/alternate.wav',sourceDuration:8,name:'Alternate'},.5,()=> 'alt'));
 h.nodes.get('clips-cursor').value=6;h.nodes.get('clips-cursor').fire('change');h.key('clip-a','s');const right=h.state.project.audioClips.at(-1);
 assert.equal(right.takeSet.takes.length,2);assert.equal(right.sourceOffset,1);assert.equal(right.takeSet.takes[0].sourceOffset,1);assert.equal(a.durationBeats,2);
 h.key('clips-timeline','PageUp');assert.equal(right.path,'/fixture/a.wav');assert.equal(a.path,'/alternate.wav');assert.equal(right.sourceOffset,1);
});
test('duplicate and source-slip keep take metadata independent and retain current offset for later selection', () => {
 const h=harness(),M=h.context.window.STClipTakesProject,a=h.state.project.audioClips[0];Object.assign(a,M.add(a,{path:'/alternate.wav',sourceDuration:8,name:'Alternate'},.5,()=> 'alt'));
 h.key('clip-a','ArrowRight',{altKey:true});assert.equal(a.takeSet.takes[1].sourceOffset,.125);
 h.key('clip-a','d',{metaKey:true});const duplicate=h.state.project.audioClips.at(-1);duplicate.takeSet.takes[0].name='Independent';assert.notEqual(a.takeSet.takes[0].name,'Independent');
 h.state.clipSelected='a';h.key('clip-a','PageUp');h.key('clip-a','PageDown');assert.equal(a.sourceOffset,.125);
});


function installTiming(h, tempos=[{id:'tempo-0',beat:0,bpm:120}]) {
 h.context.TextEncoder=TextEncoder;web.load(h.context, 'project-timing.js');
 h.state.project.timing={version:1,tempos,meters:[{id:'meter-0',beat:0,numerator:4,denominator:4}]};
}
test('tempo-map split with base60/map120 preserves four plus four beats and two-second source regions',()=>{
 const h=harness(),clip=h.state.project.audioClips[0];h.state.project.audioClips=[clip];h.state.project.bpm=60;installTiming(h);
 Object.assign(clip,{startBeat:0,durationBeats:8,sourceOffset:0,sourceDuration:4,fadeIn:3,fadeOut:3});h.clips.event('audio.clipLoaded',{path:clip.path,duration:4,peaks:[0,.5,1,.5]});
 const M=h.context.window.STClipTakesProject;Object.assign(clip,M.add(clip,{path:'/alternate.wav',sourceDuration:4},.5,()=> 'alt'));h.clips.event('audio.clipLoaded',{path:clip.path,duration:4,peaks:[0,.5,1,.5]});
 h.nodes.get('clips-cursor').value=4;h.nodes.get('clips-cursor').fire('change');h.key('clip-a','s');
 const [left,right]=h.state.project.audioClips;assert.equal(left.durationBeats,4);assert.equal(right.durationBeats,4);assert.equal(right.startBeat,4);assert.equal(right.sourceOffset,2);
 assert.equal(left.fadeIn,2);assert.equal(right.fadeOut,2);assert.deepEqual(copy(right.takeSet.takes.map(t=>t.sourceOffset)),[2,2]);assert.equal(h.changes.length,1);
});
test('map-aware source bounds, fades and split offsets integrate tempo changes inside a region',()=>{
 const h=harness(),clip=h.state.project.audioClips[0];h.state.project.audioClips=[clip];h.state.project.bpm=180;installTiming(h,[{id:'t0',beat:0,bpm:120},{id:'t1',beat:4,bpm:60}]);
 Object.assign(clip,{startBeat:2,durationBeats:6,sourceOffset:1,sourceDuration:6,fadeIn:2.5,fadeOut:3.5});h.clips.event('audio.clipLoaded',{path:clip.path,duration:6,peaks:[0,.5,1]});
 h.nodes.get('clips-cursor').value=5;h.nodes.get('clips-cursor').fire('change');h.key('clip-a','s');const [left,right]=h.state.project.audioClips;
 assert.equal(left.durationBeats,3);assert.equal(right.durationBeats,3);assert.equal(right.sourceOffset,3);assert.equal(left.fadeIn,2);assert.equal(right.fadeOut,3);
 h.state.clipSelected='a';left.durationBeats=8;h.key('clip-a','ArrowRight',{shiftKey:true});assert.equal(left.durationBeats,6);assert.equal(left.sourceOffset,1);
});
test('moving and duplicating original-speed regions preserve elapsed audio time across tempo boundaries',()=>{
 const h=harness(),clip=h.state.project.audioClips[0];h.state.project.audioClips=[clip];installTiming(h,[{id:'t0',beat:0,bpm:60},{id:'t1',beat:4,bpm:120}]);
 Object.assign(clip,{startBeat:3,durationBeats:3,sourceDuration:2,sourceOffset:0});h.clips.event('audio.clipLoaded',{path:clip.path,duration:2,peaks:[0,.5,1]});
 h.key('clip-a','ArrowRight');assert.equal(clip.startBeat,3.25);assert.equal(clip.durationBeats,3.25);
 h.key('clip-a','ArrowLeft');assert.equal(clip.startBeat,3);assert.equal(clip.durationBeats,3);
 h.key('clip-a','d',{metaKey:true});const duplicate=h.state.project.audioClips[1];assert.equal(duplicate.startBeat,6);assert.equal(duplicate.durationBeats,4);assert.equal(duplicate.sourceOffset,0);
});
test('source-slip follows the elapsed grid step and opposite keys are reversible across a tempo boundary',()=>{
 const h=harness(),clip=h.state.project.audioClips[0];installTiming(h,[{id:'t0',beat:0,bpm:60},{id:'t1',beat:4,bpm:120}]);Object.assign(clip,{startBeat:3.9,durationBeats:1,sourceOffset:.2,sourceDuration:5});
 h.key('clip-a','ArrowRight',{altKey:true});assert(Math.abs(clip.sourceOffset-.375)<1e-9);h.key('clip-a','ArrowLeft',{altKey:true});assert(Math.abs(clip.sourceOffset-.2)<1e-9);
});
test('waveform samples and fade geometry follow elapsed seconds at each musical position',()=>{
 const h=harness(),clip=h.state.project.audioClips[0];h.state.project.audioClips=[clip];installTiming(h,[{id:'t0',beat:0,bpm:60},{id:'t1',beat:4,bpm:120}]);Object.assign(clip,{startBeat:0,durationBeats:8,sourceOffset:0,sourceDuration:6,fadeIn:2,fadeOut:1});
 h.clips.event('audio.clipLoaded',{path:clip.path,duration:6,peaks:[0,.25,.5,.75,1,0]});const html=h.clips.render();assert.match(html,/M90 11V25/);assert.match(html,/M0 34L90 2H270L360 34/);assert.match(html,/id="clips-fadeIn"[^>]*max="6"/);
});
test('audio import inverts duration from its actual song position across tempo changes',async()=>{
 const h=harness();h.state.project.bpm=60;installTiming(h,[{id:'t0',beat:0,bpm:60},{id:'t1',beat:4,bpm:120}]);h.clips.event('audio.clipLoaded',{path:'/mapped.wav',duration:2,peaks:[0,.5,1]});
 const imported=await h.clips.addFile('/mapped.wav',{startBeat:3,track:0});assert.equal(imported.durationBeats,3);assert.equal(imported.startBeat,3);assert.equal(imported.sourceDuration,2);
});
test('invalid maps and out-of-domain inverse conversions reject before history or source changes',async()=>{
 const h=harness(),clip=h.state.project.audioClips[0];h.state.project.audioClips=[clip];installTiming(h);h.state.project.timing.tempos[0].bpm=0;const before=copy(h.state.project);
 h.nodes.get('clips-cursor').value=6;h.nodes.get('clips-cursor').fire('change');h.key('clip-a','s');h.key('clip-a','ArrowRight');h.key('clip-a','d',{metaKey:true});assert.deepEqual(copy(h.state.project),before);assert.equal(h.changes.length,0);
 installTiming(h);h.clips.event('audio.clipLoaded',{path:'/end.wav',duration:2,peaks:[0,1]});const count=h.state.project.audioClips.length,result=await h.clips.addFile('/end.wav',{startBeat:999999,track:0});assert.equal(result,null);assert.equal(h.state.project.audioClips.length,count);assert.equal(h.changes.length,0);
});

// The app re-renders on every selection change and edit, and then gives focus back to the control that had it
// (app-shell.js refresh). A clip that receives focus becomes the selection, so that restore must never undo a
// selection that just moved to another clip.
function rerenderRestoresFocus(h) {
  const ST = h.context.window.ST, mutate = ST.mutate;
  ST.refresh = () => { const focused = h.document.activeElement; if (focused) { h.document.activeElement = null; focused.focus(); } };
  ST.mutate = (label, fn) => { mutate(label, fn); ST.refresh(); };
}
test('arrows, Home and End move the selection off a focused clip although the re-render restores focus', () => {
  const h = harness(); rerenderRestoresFocus(h);
  h.key('clip-a', 'ArrowDown'); assert.equal(h.state.clipSelected, 'b'); assert.equal(h.document.activeElement.id, 'clip-b');
  h.key('clip-b', 'End'); assert.equal(h.state.clipSelected, 'c'); assert.equal(h.document.activeElement.id, 'clip-c'); assert.equal(h.state.track, 2);
  h.key('clip-c', 'ArrowUp'); assert.equal(h.state.clipSelected, 'b');
  h.key('clip-b', 'Home'); assert.equal(h.state.clipSelected, 'a'); assert.equal(h.document.activeElement.id, 'clip-a');
  assert.equal(h.changes.length, 0);
});
test('duplicate and split select the new region although the re-render restores focus to the source clip', () => {
  const h = harness(); rerenderRestoresFocus(h);
  h.key('clip-b', 'd', { metaKey: true });
  const duplicate = h.state.project.audioClips.at(-1);
  assert.equal(duplicate.startBeat, 12); assert.equal(h.state.clipSelected, duplicate.id, 'the copy is selected, not the source again');
  const other = harness(); rerenderRestoresFocus(other);
  other.nodes.get('clips-cursor').value = 6; other.nodes.get('clips-cursor').fire('change');
  other.key('clip-a', 's');
  const right = other.state.project.audioClips.at(-1);
  assert.equal(right.startBeat, 6); assert.equal(other.state.clipSelected, right.id, 'the region after the cut is selected');
});
test('keys that change nothing add no Undo step', () => {
  const h = harness(); Object.assign(h.state.project.audioClips[0], { fadeIn: 0, fadeOut: 0 }); // as imported clips are stored
  h.key('clip-a', 'ArrowUp', { altKey: true }); // already on the first track
  h.key('clip-a', 'ArrowLeft', { altKey: true }); // source offset already 0
  assert.equal(h.changes.length, 0); assert.equal(h.state.project.audioClips[0].track, 0);
  h.key('clip-a', 'ArrowDown', { altKey: true }); assert.deepEqual(h.changes, ['move audio clip track']);
});
test('an emptied inspector field keeps the clip value instead of collapsing it to the minimum', () => {
  const h = harness(), a = h.state.project.audioClips[0];
  const field = h.node('clips-durationBeats', h.panel, 'INPUT', { clipsField: 'durationBeats' });
  field.min = '.001'; field.max = '1000000'; h.clips.bind();
  field.value = ''; field.fire('change'); assert.equal(a.durationBeats, 4); assert.equal(h.changes.length, 0);
  field.value = '2'; field.fire('change'); assert.equal(a.durationBeats, 2); assert.deepEqual(h.changes, ['edit audio clip']);
});

(async()=>{for (const { name, fn } of tests) { await fn(); console.log('PASS ' + name); }
console.log(`Clip navigation: ${tests.length} regression groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
