'use strict';
const assert = require('node:assert/strict'), fs = require('node:fs'), vm = require('node:vm');
const web = require('./helpers/web');
const source = web.source('grid-mouse.js');
const copy = value => JSON.parse(JSON.stringify(value));
function harness({ tracks = [{instrument: 0}, {instrument: 1}], groups } = {}) {
  const listeners = new Map(), timers = [], cells = new Map(), toasts = [], history = [];
  const pattern = { length: 8, steps: tracks.map(() => Array(8).fill(null)) };
  const state = { project: { tracks: copy(tracks), ...(groups ? { groups: copy(groups) } : {}) }, view: 'pattern', track: 0, row: 0, column: 0, selection: null };
  let refreshes = 0, hit = null, refuse = false, scoped = null;
  const add = (type, fn) => { const list = listeners.get(type) || new Set(); list.add(fn); listeners.set(type, list); };
  const remove = (type, fn) => listeners.get(type)?.delete(fn);
  function cell(t, r, sound) {
    const key = `${t}:${r}${sound ? ':' + sound : ''}`;
    if (!cells.has(key)) {
      const classes = new Set();
      cells.set(key, { dataset: {cell: `${t}:${r}`, ...(sound ? {soundId: sound} : {})}, classes, closest(selector) { return selector.includes('#tracker') || (selector === '[data-sound-id]' && sound) ? this : null; }, querySelectorAll() { return []; }, classList: { add(...names) { names.forEach(n => classes.add(n)); }, remove(...names) { names.forEach(n => classes.delete(n)); }, toggle(name, on) { if (on) classes.add(name); else classes.delete(name); } } });
    }
    return cells.get(key);
  }
  for (let t = 0; t < tracks.length; t++) for (let r = 0; r < 8; r++) cell(t, r);
  const app = { state, pattern: () => pattern, selectedBounds() { const s = state.selection; return s ? {t0: Math.min(s.track,state.track),t1: Math.max(s.track,state.track),r0: Math.min(s.row,state.row),r1: Math.max(s.row,state.row)} : null; }, refresh() { refreshes++; }, mutate(label, fn) { if (refuse) return false; const before = copy(pattern.steps); try { fn(); } catch (error) { pattern.steps.splice(0, Infinity, ...before); throw error; } history.push(before); return true; }, toast: message => toasts.push(message) };
  const document = { addEventListener: add, getElementById: () => ({focus(){}}), elementFromPoint: () => hit, querySelectorAll(selector) { const at = selector.match(/data-cell="(.*?)"/)?.[1], sound = selector.match(/data-sound-id="(.*?)"/)?.[1]; return [...cells.values()].filter(c => (!selector.includes('td.grid-drop') || c.classes.has('grid-drop')) && (!at || c.dataset.cell === at) && (!sound || c.dataset.soundId === sound)); }, querySelector(selector) { return this.querySelectorAll(selector)[0]; } };
  const window = { ST: app, STGroups: require('../Resources/Web/track-groups.js'), STUtil: { clone: copy }, addEventListener: add, removeEventListener: remove };
  web.runText('grid-mouse.js', source, {window, document, setTimeout: fn => timers.push(fn)});
  function fire(type, t = 0, r = 0, extra = {}) { hit = extra.target || cell(t, r, extra.sound); const event = {button:0,target:hit,clientX:0,clientY:0, preventDefault(){this.defaultPrevented=true;},stopPropagation(){this.stopped=true;},...extra}; for (const fn of [...listeners.get(type)||[]]) fn(event); return event; }
  // Press, drag and release in one gesture, then run the deferred drop.
  function drag(from, to, extra = {}) { fire('pointerdown', from[0], from[1], {...extra, sound: from[2]}); fire('pointermove', to[0], to[1], {...extra, sound: to[2]}); fire('pointerup', to[0], to[1], {...extra, sound: to[2]}); while (timers.length) timers.shift()(); }
  return { state, pattern, history, toasts, window, cell, fire, drag, get scoped() { return scoped; }, flush() { while(timers.length) timers.shift()(); }, get refreshes(){return refreshes;}, refuse(){refuse=true;} };
}
let count = 0;
function test(name, run) { run(); count++; console.log('PASS '+name); }
test('drag selection preserves the pressed DOM through movement and suppresses trailing click', () => {
  const h=harness(); h.fire('pointerdown',0,1); h.fire('pointermove',1,3);
  assert.equal(h.refreshes,0); assert(h.cell(0,2).classes.has('selection')); assert(h.cell(1,3).classes.has('selected'));
  h.fire('pointerup',1,3); assert.equal(h.refreshes,1); assert.equal(h.state.selection.row,1);
  assert(h.fire('click',1,3).stopped); assert.equal(h.window.STGridMouse.active,false);
});
test('selected single note moves with its chord and FX, and has one undo checkpoint', () => {
  const h=harness(), step={note:60,instrument:0,chord:[0,4,7],fx1:{type:'reverse',value:1}};
  h.pattern.steps[0][0]=copy(step); h.fire('pointerdown'); h.fire('pointermove',1,2); h.fire('pointerup',1,2); h.flush();
  assert.equal(h.pattern.steps[0][0],null); assert.deepEqual(copy(h.pattern.steps[1][2]),step); assert.equal(h.history.length,1); assert.deepEqual(h.history[0][0][0],step); assert.equal(h.state.instrument,1);
});
test('single-cell explicit selection can move and Option-copy keeps independent data', () => {
  const h=harness(); h.pattern.steps[0][0]={note:60,chord:[0,7]}; h.state.selection={track:0,row:0};
  h.fire('pointerdown',0,0,{altKey:true}); h.fire('pointermove',0,3,{altKey:true}); h.fire('pointerup',0,3,{altKey:true}); h.flush();
  assert.equal(h.pattern.steps[0][0].note,60); assert.equal(h.pattern.steps[0][3].note,60); h.pattern.steps[0][3].chord[1]=4; assert.equal(h.pattern.steps[0][0].chord[1],7);
});
test('overlapping block movement snapshots before clearing the source', () => {
  const h=harness(); h.pattern.steps[0][0]={note:60}; h.pattern.steps[0][1]={note:62}; h.state.selection={track:0,row:0}; h.state.row=1;
  h.fire('pointerdown'); h.fire('pointermove',0,1); h.fire('pointerup',0,1); h.flush();
  assert.deepEqual(copy(h.pattern.steps[0].slice(0,3)),[null,{note:60},{note:62}]);
});
test('cancel and lost window focus remove listeners without committing', () => {
  for (const event of ['pointercancel','blur']) { const h=harness(); h.pattern.steps[0][0]={note:60}; h.fire('pointerdown'); h.fire('pointermove',0,2); h.fire(event); h.fire('pointerup',0,2); h.flush(); assert.equal(h.history.length,0); assert.equal(h.window.STGridMouse.active,false); }
});
test('deferred drops cannot edit a switched project and refused edits cannot report success', () => {
  for (const mode of ['switch','refuse']) { const h=harness(); h.pattern.steps[0][0]={note:60}; h.fire('pointerdown'); h.fire('pointermove',0,2); h.fire('pointerup',0,2); if(mode==='switch')h.state.project={tracks:[]};else h.refuse(); h.flush(); assert.equal(h.history.length,0); assert.equal(h.toasts.length,0); assert.equal(h.pattern.steps[0][0].note,60); }
});
test('Gate inspector clicks do not start drag selection or suppress the field-focus click',()=>{
 const h=harness(),gate={closest(selector){return selector==='[data-gate-inspector]'?this:h.cell(0,0);}};
 const down=h.fire('pointerdown',0,0,{target:gate});h.fire('pointerup',0,0,{target:gate});const click=h.fire('click',0,0,{target:gate});h.flush();
 assert.equal(down.defaultPrevented,undefined);assert.equal(click.stopped,undefined);assert.equal(h.window.STGridMouse.active,false);assert.equal(h.refreshes,0);
});
test('steps dragged onto a group member play that member\'s instrument; other instruments stay', () => {
  const h = harness({ tracks: [{instrument: 0, groupID: 'g'}, {instrument: 1, groupID: 'g'}], groups: [{id: 'g', name: 'Kit', busID: 'b'}] });
  h.pattern.steps[0][0] = {note: 36, instrument: 0, velocity: 90}; h.pattern.steps[0][1] = {note: 40, instrument: 5};
  h.state.selection = {track: 0, row: 0}; h.state.track = 0; h.state.row = 1;
  h.drag([0, 0], [1, 0]);
  assert.deepEqual(copy(h.pattern.steps[1][0]), {note: 36, instrument: 1, velocity: 90});
  assert.deepEqual(copy(h.pattern.steps[1][1]), {note: 40, instrument: 5});
  assert.equal(h.pattern.steps[0][0], null);
});
console.log(`Grid mouse: ${count} regression groups passed.`);
