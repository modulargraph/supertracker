'use strict';
// Drive the real mixer module. Native fixtures model the plugin.move response contract.
const assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = web.source('studio.js');
const clone = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function fixture(slots = [0, 3, 7]) {
  return { activePattern: 0, tracks: [{ name: 'Keys', instrument: 0 }, { name: 'Bass', instrument: 1 }], instruments: [{ type: 'synth' }, { type: 'synth' }],
    audioPlugins: [...slots.map((slot, i) => ({ track: 0, kind: 'effect', slot, component: `fx:${i}`, name: ['Equalizer', 'Compressor', 'Echo'][i], state: `saved-${i}`, bypass: i === 1, parameters: [{ address: 42, name: 'Amount', normalized: .2 + i / 10 }] })), { track: 0, kind: 'instrument', slot: 0, component: 'instrument', state: 'preset' }, { track: 1, kind: 'effect', slot: 0, component: 'bassfx', state: 'bass' }],
    patterns: [{ id: 0, length: 64, automation: [{ track: 0, target: `plugin:effect:${slots[0]}:42`, points: [{ row: 0, value: .25 }] }, { track: 0, target: `plugin:${slots[1]}:42`, points: [{ row: 4, value: .75 }] }, { track: 0, target: 'plugin:instrument:0:42', points: [{ row: 0, value: .5 }] }, { track: 1, target: 'plugin:effect:0:42', points: [] }] }, { id: 1, length: 32, automation: [{ track: 0, target: `plugin:effect:${slots[2]}:99:part`, points: [{ row: 20, value: .6 }] }, { track: 0, target: 'volume', points: [] }] }] };
}
function response(owner, track, fromSlot, toSlot) {
  const before = clone(owner.audioPlugins.filter(p => p.track === track && p.kind === 'effect').sort((a, b) => a.slot - b.slot)).map((p, i) => ({ ...p, instanceID: p.instanceID || `instance-${i}`, state: `live-${i}` }));
  const slots = before.map(p => p.slot), from = slots.indexOf(fromSlot), to = slots.indexOf(toSlot), ordered = before.slice(); ordered.splice(to, 0, ordered.splice(from, 1)[0]);
  const after = ordered.map((p, i) => ({ ...p, slot: slots[i] }));
  return { moved: true, track, fromSlot, toSlot, previousPlugins: before, plugins: after, slotMap: before.map(p => ({ fromSlot: p.slot, toSlot: after.find(q => q.instanceID === p.instanceID).slot, instanceID: p.instanceID })) };
}
function harness(slots) {
  const nodes = new Map(), groups = new Map(), calls = [], mutations = [], errors = [], timers = new Map(); let timerID = 0, refreshes = 0;
  const document = { body: {}, activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => groups.get(selector) || [], querySelector: selector => groups.get(selector)?.[0] || null };
  function node(id, dataset = {}, tagName = 'BUTTON') {
    const item = { id, dataset, tagName, type: tagName === 'INPUT' ? 'range' : '', handlers: {}, value: '.5', disabled: false, style: {}, classList: { toggle() {} }, setAttribute() {}, addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }, focus() { document.activeElement = this; }, scrollIntoView() { this.scrolled = true; }, closest(selector) { return selector === '.studio-plugin > summary' && this.tagName === 'SUMMARY' ? this : null; }, async fire(type, extra = {}) { for (const fn of this.handlers[type] || []) await fn({ target: this, ...extra }); } }; nodes.set(id, item); return item;
  }
  const state = { project: fixture(slots), track: 0, instrument: 0, view: 'mixer', selectedLane: 0, plugins: [] };
  let native = (command, payload) => command === 'plugin.move' ? response(state.project, payload.track, payload.fromSlot, payload.toSlot) : command === 'plugin.parameters' ? { parameters: state.project.audioPlugins.find(p => p.track === payload.track && p.kind === payload.kind && p.slot === payload.slot)?.parameters || [] } : command === 'plugin.state' ? state.project.audioPlugins.find(p => p.track === payload.track && p.kind === payload.kind && p.slot === payload.slot) : {};
  const ST = { state, escapeHTML: value => String(value), toast(message, kind) { errors.push({ message, kind }); }, captureNativeEdits: async () => { calls.push({ command: 'capture' }); return true; }, syncEngine: async () => { calls.push({ command: 'sync' }); }, request(command, payload = {}) { calls.push({ command, payload: clone(payload) }); return Promise.resolve().then(() => native(command, payload)); }, mutate(label, fn) { mutations.push({ label, before: clone(state.project) }); fn(state.project); this.refresh(); }, refresh() { refreshes++; mount(); } };
  ST.syncEngineProject=async(makeProject,options={})=>{const project=makeProject();return project?ST.request('engine.project',{project,awaitPlugins:true,...(options.replace?{replace:true}:{})}):false;};
  const context = vm.createContext({ window: { ST }, document, setTimeout(fn) { const id = ++timerID; timers.set(id, fn); return id; }, clearTimeout(id) { timers.delete(id); }, requestAnimationFrame: fn => fn() }); web.load(context, 'studio.js', { text: source }); const studio = context.window.STStudio;
  function mount() {
    const focus = document.activeElement?.id; nodes.clear(); groups.clear();
    const details = [], summaries = [], moves = [], actions = [], addresses = [], presets = [];
    for (const p of state.project.audioPlugins.filter(p => p.track === state.track && p.kind === 'effect')) {
      const d = node(`detail-${p.slot}`, { pluginTrack: String(p.track), pluginSlot: String(p.slot) }, 'DETAILS'); d.open = p.slot === 0; details.push(d);
      const header = node(`plugin-header-${p.slot}`, {}, 'SUMMARY'); header.parentElement = d; summaries.push(header);
      for (const direction of [-1, 1]) moves.push(node(`plugin-move-${direction === -1 ? 'up' : 'down'}-${p.slot}`, { slot: String(p.slot), pluginMove: String(direction) }));
      for (const action of ['editor', 'refresh', 'remove', 'bypass']) actions.push(node(`plugin-${action}-${p.slot}`, { slot: String(p.slot), pluginAction: action }));
      const input = node(`plugin-${p.slot}-0`, { slot: String(p.slot), pluginAddress: '42' }, 'INPUT'); addresses.push(input); node(input.id + '-out');
      const preset = node(`plugin-preset-${p.slot}`, { pluginPreset: String(p.slot) }, 'SELECT'); preset.value = '1'; presets.push(preset);
    }
    node('mix-pan-0', { studioTrack: '0' }, 'INPUT'); node('mix-pan-1', { studioTrack: '1' }, 'INPUT'); node('elsewhere');
    const preview = node('mix-vol-0', { studioTrack: '0', studioProperty: 'volume' }, 'INPUT'); node('mix-vol-0-out'); groups.set('[data-studio-property]', [preview]);
    groups.set('.studio-plugin', details); groups.set('.studio-plugin > summary', summaries); groups.set('[data-plugin-move]', moves); groups.set('[data-plugin-action]', actions); groups.set('[data-plugin-address]', addresses); groups.set('[data-plugin-preset]', presets);
    studio.bindMixer(); if (focus && nodes.has(focus)) nodes.get(focus).focus();
  }
  mount();
  function key(slot, key, modifiers = { altKey: true }) { const target = nodes.get(`plugin-header-${slot}`); target.focus(); const e = { target, key, ...modifiers, preventDefault() { this.defaultPrevented = true; } }; e.handled = studio.key(e); return e; }
  async function finish() { for (let i = 0; i < 50; i++) { await tick(); if (!studio.isMovingEffect) return; } throw new Error('Move did not complete'); }
  return { state, studio, ST, nodes, document, calls, mutations, errors, timers, key, finish, mount, get refreshes() { return refreshes; }, set native(fn) { native = fn; }, async undo() { const before = mutations.pop().before; state.project = before; studio.event('project.loaded', {}); await ST.request('engine.project', { project: before, replace: true }); mount(); } };
}
// Real app state/undo/synchronization, with only the DOM and native message transport stubbed.
function integratedHarness() {
  const nodes=new Map(),calls=[],timers=new Map();let clock=0,nextTimer=1,nativeProject,moveGate=null;
  const document={activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  function element(id){id=String(id).replace(/^#/, '');if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',value:'',dataset:{},style:{},classList:{toggle(){},add(){},remove(){}},focus(){document.activeElement=this;},select(){},scrollIntoView(){},closest(selector){return selector==='.studio-plugin > summary'&&this.tagName==='SUMMARY'?this:null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(){}});return nodes.get(id);}
  document.body=element('body');
  const window={addEventListener(){},STHostTransport:hostStub({post(message){
    const request=clone(message);calls.push(request);
    Promise.resolve().then(async()=>{
      if(request.command==='plugin.move'){const result=response(nativeProject,request.payload.track,request.payload.fromSlot,request.payload.toSlot);await moveGate.promise;nativeProject.audioPlugins=[...nativeProject.audioPlugins.filter(p=>p.kind!=='effect'||p.track!==0),...clone(result.plugins)];return result;}
      if(request.command==='engine.project'){nativeProject=clone(request.payload.project);return {};}
      if(request.command==='engine.snapshot')return {audioPlugins:clone(nativeProject.audioPlugins)};
      if(request.command==='project.save')return {saved:true,project:request.payload.project,path:'/tmp/session.supertracker'};
      if(request.command==='plugin.parameters')return {parameters:[]};return {};
    }).then(result=>window.STHostTransport.deliver({id:request.id,ok:true,result}),error=>window.STHostTransport.deliver({id:request.id,ok:false,error:error.message}));
  }})};
  const context=vm.createContext({window,document,console,setTimeout:(fn,delay=0)=>{const id=nextTimer++;timers.set(id,{fn,at:clock+delay});return id;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800});
  web.load(context, 'app.js');web.load(context, 'studio.js', { text: source });
  const ST=window.ST;ST.state.project=ST.normalize(fixture());ST.state.view='mixer';ST.state.engineReady=true;nativeProject=clone(ST.state.project);
  function advance(ms){clock+=ms;for(const[id,timer]of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}}
  function move(){moveGate=deferred();const header=element('plugin-header-3');header.tagName='SUMMARY';header.parentElement={dataset:{pluginSlot:'3'}};header.focus();window.STStudio.key({target:header,key:'ArrowUp',altKey:true,preventDefault(){}});return moveGate;}
  return {ST,window,calls,advance,move};
}
const tests = [], test = (name, fn) => tests.push({ name, fn });

test('buttons expose direction and boundaries; ordinary arrows navigate without moving', async () => {
  const h = harness(); await tick(); const html = h.studio.renderMixer();
  assert.match(html, /id="plugin-move-up-0"[^>]*disabled/); assert.match(html, /id="plugin-move-down-7"[^>]*disabled/);
  assert.match(html, /aria-keyshortcuts="Alt\+ArrowUp Alt\+ArrowDown"/);
  h.key(0, 'ArrowDown', {}); assert.equal(h.document.activeElement.id, 'plugin-header-3'); assert.equal(h.calls.filter(x => x.command === 'plugin.move').length, 0);
  h.key(0, 'ArrowUp'); await h.finish(); assert.equal(h.mutations.length, 0);
});

test('sparse reorder preserves slots, native states and bypass; all pattern effect lanes move in one undo', async () => {
  const h = harness(); await tick(); const original = clone(h.state.project), event = h.key(7, 'ArrowUp');
  assert(event.handled && event.defaultPrevented); await h.finish();
  const effects = h.state.project.audioPlugins.filter(p => p.track === 0 && p.kind === 'effect');
  assert.deepEqual(clone(effects.map(p => [p.slot, p.component, p.bypass, p.instanceID, p.state])), [[0,'fx:0',false,'instance-0','live-0'],[3,'fx:2',false,'instance-2','live-2'],[7,'fx:1',true,'instance-1','live-1']]);
  assert.deepEqual(h.state.project.patterns.map(p => p.automation.map(l => l.target)), [['plugin:effect:0:42','plugin:7:42','plugin:instrument:0:42','plugin:effect:0:42'],['plugin:effect:3:99:part','volume']]);
  assert.deepEqual(h.state.project.patterns.map(p => p.automation.map(l => l.points)), original.patterns.map(p => p.automation.map(l => l.points)));
  assert.deepEqual(clone(h.state.project.audioPlugins.filter(p => p.kind === 'instrument' || p.track === 1)), original.audioPlugins.filter(p => p.kind === 'instrument' || p.track === 1));
  assert.equal(h.mutations.length, 1); assert.equal(h.mutations[0].label, 'Reorder insert effect'); assert.equal(h.document.activeElement.id, 'plugin-header-3'); assert(h.document.activeElement.scrolled); assert.equal(h.state.track, 0);
  const order = h.calls.filter(x => ['capture','engine.project','plugin.move'].includes(x.command)).map(x => x.command); assert.deepEqual(order, ['capture','engine.project','plugin.move']);
  await h.undo(); assert.deepEqual(h.state.project.patterns, original.patterns);
  assert.deepEqual(h.state.project.audioPlugins.filter(p => p.kind === 'effect' && p.track === 0).map(p => [p.slot,p.component,p.state,p.instanceID]), [[0,'fx:0','live-0','instance-0'],[3,'fx:1','live-1','instance-1'],[7,'fx:2','live-2','instance-2']]);
  assert.equal(h.calls.findLast(x => x.command === 'engine.project').payload.replace, true);
});

test('host refreshing opaque plug-in state during the pre-move sync does not cancel the move', async () => {
  // plugins.changed replaces the descriptors (same instances, new state blobs, other key order), as Ozone 12 on Master did.
  const h = harness(); await tick(); const base = (command, payload) => command === 'plugin.move' ? response(h.state.project, payload.track, payload.fromSlot, payload.toSlot) : { parameters: [] };
  h.native = (command, payload) => {
    if (command === 'engine.project') h.state.project.audioPlugins = h.state.project.audioPlugins.map((p, i) => ({ state: `refreshed-${i}`, ...p, state: `refreshed-${i}` }));
    return base(command, payload);
  };
  h.key(7, 'ArrowUp'); await h.finish();
  assert.deepEqual(h.errors.map(e => e.message), []); assert.equal(h.mutations.length, 1);
  assert.deepEqual(clone(h.state.project.audioPlugins.filter(p => p.track === 0 && p.kind === 'effect').map(p => [p.slot, p.component])), [[0,'fx:0'],[3,'fx:2'],[7,'fx:1']]);
});

test('Move down button uses adjacent occupied slot and follows the moved header', async () => {
  const h = harness([0,1,2]); await tick(); const button = h.nodes.get('plugin-move-down-0'); button.focus(); await button.fire('click'); await h.finish();
  assert.deepEqual(h.calls.find(x => x.command === 'plugin.move').payload, { track:0,fromSlot:0,toSlot:1 }); assert.equal(h.document.activeElement.id, 'plugin-header-1');
});

test('pending move suppresses duplicate moves, slot edits and preview timers; exposes completion gate', async () => {
  const h = harness(); await tick(); await h.nodes.get('mix-vol-0').fire('input'); assert.equal(h.timers.size, 1);
  const wait = deferred(); h.native = (command,payload) => command === 'plugin.move' ? wait.promise : command === 'plugin.parameters' ? { parameters: [] } : {};
  h.key(3,'ArrowUp'); await tick(); assert.equal(h.timers.size, 0); assert(h.studio.isMovingEffect);
  let finished = false; const completion = h.studio.waitForEffectMove().then(() => { finished = true; });
  h.key(3,'ArrowDown'); await h.nodes.get('plugin-3-0').fire('input'); await h.nodes.get('plugin-3-0').fire('change'); await h.nodes.get('plugin-bypass-3').fire('click'); await h.nodes.get('plugin-preset-3').fire('change');
  assert(!finished); assert.equal(h.calls.filter(x => x.command === 'plugin.move').length,1); assert.equal(h.calls.filter(x => x.command === 'plugin.parameter' || x.command === 'plugin.preset').length,0);
  const html=h.studio.renderMixer(); assert.match(html,/id="plugin-3-0"[^>]*disabled/);
  wait.resolve(response(h.state.project,0,3,0)); await h.finish(); await completion; assert(finished); assert.equal(h.mutations.length,1);
});

test('new mixer previews wait for the remapped chain and superseded previews never send', async () => {
  for (const supersede of [false,true]) {
    const h=harness();await tick();const wait=deferred(),result=response(h.state.project,0,3,0);h.native=command=>command==='plugin.move'?wait.promise:{parameters:[]};
    h.key(3,'ArrowUp');await tick();const input=h.nodes.get('mix-vol-0');input.value='.61';await input.fire('input');
    const timer=h.timers.values().next().value, pending=timer();await tick();assert.equal(h.calls.filter(x=>x.command==='engine.project').length,1);
    if(supersede){input.value='.72';await input.fire('change');}
    wait.resolve(result);await h.finish();await pending;
    const previews=h.calls.filter(x=>x.command==='engine.project'&&x.payload.project.tracks[0].volume===.61);assert.equal(previews.length,supersede?0:1);
    if(!supersede){assert.equal(previews[0].payload.project.tracks[0].volume,.61);assert.equal(previews[0].payload.project.audioPlugins.find(p=>p.track===0&&p.kind==='effect'&&p.slot===0).component,'fx:1');}
  }
});

test('in-flight remove or parameter commit prevents an overlapping move', async () => {
  for (const action of ['remove','parameter']) {
    const h=harness(); await tick(); const wait=deferred();
    h.native=(command,payload)=> command === (action === 'remove' ? 'plugin.remove':'plugin.parameter') ? wait.promise : command === 'plugin.parameters' ? {parameters:[]} : command === 'plugin.state' ? {...h.state.project.audioPlugins.find(p=>p.kind==='effect'&&p.slot===3),...payload} : {};
    const promise=h.nodes.get(action==='remove'?'plugin-remove-3':'plugin-3-0').fire(action==='remove'?'click':'change'); await tick(); h.key(3,'ArrowUp'); await tick();
    assert.equal(h.calls.filter(x=>x.command==='plugin.move').length,0); assert(h.errors.some(x=>x.message.includes('Finish')));
    wait.resolve({track:0,kind:'effect',slot:3}); await promise;
  }
});

test('failed native capture makes no move or undo entry', async () => {
  const h=harness(); await tick(); h.ST.captureNativeEdits=async()=>false; h.key(3,'ArrowUp'); await h.finish(); assert.equal(h.calls.filter(x=>x.command==='plugin.move').length,0); assert.equal(h.mutations.length,0);
});

test('late responses never attach to a replaced project, replaced track or edited chain', async () => {
  for (const kind of ['project','track','chain']) {
    const h=harness(); await tick(); const wait=deferred(), result=response(h.state.project,0,3,0); h.native=(command)=>command==='plugin.move'?wait.promise:{parameters:[]};
    h.key(3,'ArrowUp'); await tick();
    if(kind==='project')h.state.project=fixture([0,1,2]);
    if(kind==='track')h.state.project.tracks[0]={name:'Replacement',instrument:0};
    if(kind==='chain')h.state.project.audioPlugins.find(p=>p.kind==='effect'&&p.track===0&&p.slot===3).component='different';
    const expected=clone(h.state.project); wait.resolve(result); await h.finish(); assert.deepEqual(h.state.project,expected); assert.equal(h.mutations.length,0); assert(h.calls.some(x=>x.command==='engine.project'&&x.payload.replace));
  }
});

test('malformed host order is rejected and authoritative current project is restored', async () => {
  for (const corrupt of [r=>r.track=1,r=>r.plugins[1].slot=r.plugins[0].slot,r=>r.slotMap.pop(),r=>r.slotMap[0].instanceID='wrong',r=>{r.plugins[0].component=r.previousPlugins[0].component='substitute';},r=>r.plugins[0].instanceID='wrong']) {
    const h=harness(); await tick(); const original=clone(h.state.project); h.native=(command,payload)=>{if(command!=='plugin.move')return {parameters:[]};const r=response(h.state.project,payload.track,payload.fromSlot,payload.toSlot);corrupt(r);return r;};
    h.key(3,'ArrowUp'); await h.finish(); assert.deepEqual(h.state.project,original); assert.equal(h.mutations.length,0); assert(h.errors.some(x=>x.message.includes('mismatched'))); assert(h.calls.some(x=>x.command==='engine.project'));
  }
});

test('moving focus or channel during a pending move keeps the newer navigation', async () => {
  for (const anotherTrack of [false,true]) {
    const h=harness(); await tick(); const wait=deferred(), result=response(h.state.project,0,3,0); h.native=command=>command==='plugin.move'?wait.promise:{parameters:[]};
    h.key(3,'ArrowUp'); await tick(); if(anotherTrack){h.state.track=1;h.mount();}h.nodes.get(anotherTrack?'mix-pan-1':'elsewhere').focus();
    wait.resolve(result); await h.finish(); assert.equal(h.document.activeElement.id,anotherTrack?'mix-pan-1':'elsewhere'); assert.equal(h.state.track,anotherTrack?1:0); assert.equal(h.mutations.length,1);
  }
});

test('actual app waits for the module move before sync/save and Undo/Redo restores one atomic operation', async () => {
  const h=integratedHarness(),original=clone(h.ST.state.project),gate=h.move();await tick();assert(h.window.STStudio.isMovingEffect);
  const initialSyncs=h.calls.filter(x=>x.command==='engine.project').length;
  h.ST.mutate('Rename while moving',p=>p.name='Parallel edit');h.advance(2000);const saving=h.ST.projectAction('save');await tick();
  assert.equal(h.calls.filter(x=>x.command==='engine.project').length,initialSyncs);assert.equal(h.calls.filter(x=>x.command==='project.save').length,0);
  gate.resolve();await tick();assert.equal(await saving,true);await tick();
  assert.equal(h.ST.state.undo.length,2);assert.equal(h.ST.state.project.name,'Parallel edit');assert.equal(h.ST.state.project.patterns[0].automation[0].target,'plugin:effect:3:42');
  const saved=h.calls.find(x=>x.command==='project.save').payload.project;assert.equal(saved.name,'Parallel edit');assert.equal(saved.audioPlugins.find(p=>p.track===0&&p.kind==='effect'&&p.slot===0).component,'fx:1');
  const after=clone(h.ST.state.project);await h.ST.perform('undo');await h.ST.syncEngine(true,true);
  assert.equal(h.ST.state.project.name,'Parallel edit');assert.deepEqual(clone(h.ST.state.project.patterns),original.patterns);assert.equal(h.ST.state.project.audioPlugins.find(p=>p.track===0&&p.kind==='effect'&&p.slot===0).instanceID,'instance-0');
  await h.ST.perform('redo');assert.deepEqual(clone(h.ST.state.project),after);assert.equal(h.ST.state.undo.length,2);
});

test('beat-period automation has native bounds, a quarter-note default and readable units', async () => {
  const h=harness(); for(const name of ['lfoBeats','lfo2Beats']){const target=h.studio.target(0,name); assert.equal(target.min,1/64);assert.equal(target.max,64);assert.equal(target.step,1/64);assert.equal(target.value,1);assert.equal(target.format(.5),'0.5 beats');}
});

(async()=>{for(const {name,fn} of tests){await fn();console.log(`PASS ${name}`);}console.log(`Plug-in reorder: ${tests.length} regression groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
