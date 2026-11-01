'use strict';
// Maschine MK3 controller interface (Resources/Web/mk3.js + mk3-screens.js) without a browser or hardware:
// synthetic 'mk3.input' events drive the real app modules; LEDs and display lists are checked against the
// software contract in docs/MASCHINE_MK3.md.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = async () => { for (let i = 0; i < 60; i++) await Promise.resolve(); };

function harness() {
  const elements = new Map();
  const element = id => {
    if (!elements.has(id)) elements.set(id, { id, innerHTML: '', value: '', textContent: '', dataset: {}, style: { setProperty() {} }, classList: { toggle() {}, remove() {}, add() {}, contains() { return false; } },
      handlers: {}, focus() {}, select() {}, blur() {}, click() { return this.handlers.click?.(); }, remove() {}, append() {}, addEventListener(name, fn) { this.handlers[name] = fn; }, removeEventListener() {}, dispatchEvent() {}, setAttribute() {}, removeAttribute() {}, getAttribute() { return null; }, scrollIntoView() {},
      closest() { return null; }, contains() { return false; }, querySelector() { return null; }, querySelectorAll() { return []; }, getBoundingClientRect() { return { left: 0, top: 0, width: 100, height: 100 }; }, isConnected: true });
    return elements.get(id);
  };
  const doc = { body: element('body'), activeElement: null, readyState: 'complete', querySelector: q => element(q), querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: tag => element('new-' + tag + Math.random()) };
  const calls = [], clock = { now: 1000000 }, timers = [];
  let context, timerId = 0;
  const reply = message => {
    const p = message.payload || {};
    if (message.command === 'transport.recordArm') return { armed: !!p.armed };
    if (message.command === 'mk3.connect' || message.command === 'mk3.status') return { connected: true, screens: true, exclusive: !!p.exclusive };
    return {};
  };
  const win = { addEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, webkit: { messageHandlers: { supertracker: { postMessage(message) { calls.push(message); Promise.resolve().then(() => context.window.nativeReceive({ id: message.id, ok: true, result: reply(message) })); } } } } };
  const setTimeout_ = (fn, ms = 0) => { const id = ++timerId; timers.push({ id, fn, at: clock.now + Math.max(0, Number(ms) || 0) }); return id; };
  const clearTimeout_ = id => { const index = timers.findIndex(timer => timer.id === id); if (index >= 0) timers.splice(index, 1); };
  context = vm.createContext({ console, window: win, document: doc, performance: { now: () => clock.now }, requestAnimationFrame: fn => fn(), setTimeout: setTimeout_, clearTimeout: clearTimeout_, setInterval: () => 0, clearInterval() {},
    Date: class extends Date { static now() { return clock.now; } }, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, BigInt, Error, Event: class { constructor(type) { this.type = type; } }, innerWidth: 1280, innerHeight: 800, globalThis: null });
  context.globalThis = context.window;
  win.document = doc;
  for (const file of ['app.js', 'note-repeat.js', 'pattern-tools.js', 'song-model.js', 'song-sequence.js', 'plugin-pages.js', 'nks-controls.js', 'sampler.js', 'mk3-screens.js', 'mk3.js']) web.load(context, file);
  const MK3 = win.STMK3;
  MK3._clock = () => clock.now;
  // Run due timers; the controller's bursts and frame scheduling use setTimeout.
  const advance = (ms = 0) => { const end = clock.now + ms; for (;;) { timers.sort((a, b) => a.at - b.at); const next = timers[0]; if (!next || next.at > end) break; timers.shift(); clock.now = Math.max(clock.now, next.at); next.fn(); } clock.now = end; };
  return { win, ST: win.ST, state: win.ST.state, MK3, calls, clock, advance, element };
}


function session() {
  const h = harness(); h.state.project = h.ST.demoProject(); h.ST.normalize(h.state.project);
  h.state.track = 0; h.state.instrument = 0; h.state.row = 0;
  h.input = p => h.MK3.event('mk3.input', p);
  h.press = name => { h.input({type:'button', name, pressed:true}); h.input({type:'button', name, pressed:false}); };
  h.down = n => h.input({type:'pad', pad:n, phase:'down', velocity:100});
  h.up = n => h.input({type:'pad', pad:n, phase:'up'});
  return h;
}
const cases = [];
const test = (name, fn) => cases.push([name,fn]);
test('step release stays on pressed track and pattern', async () => {
  const h=session(), p=h.ST.pattern(); h.press('step');
  const original={note:60,instrument:0,velocity:100,gate:0.8}, other={...original,note:64};
  p.steps[0][0]=original; p.steps[1][0]=other; h.down(1); h.state.track=1; h.up(1);
  assert.equal(p.steps[0][0],null); assert.equal(p.steps[1][0],other);
});
test('step release cannot delete a replacement session note', async () => {
  const h=session(); h.press('step'); h.down(1);
  h.state.project=h.ST.demoProject(); h.ST.normalize(h.state.project);
  const before=JSON.stringify(h.ST.pattern().steps); h.up(1);
  assert.equal(JSON.stringify(h.ST.pattern().steps),before);
});
test('rejected encoder edit remains retryable with one undo', async () => {
  const h=session(); h.press('mixer'); const before=h.state.project.tracks[0].volume;
  h.input({type:'encoder',index:0,delta:-0.2}); const depth=h.state.undo.length;
  h.win.STExport={isBusy:true}; assert.equal(h.MK3.commitBursts(),false);
  assert.equal(h.MK3.hasPendingEdits,true); assert.equal(h.state.project.tracks[0].volume,before);
  h.win.STExport.isBusy=false; assert.equal(await h.MK3.flushEdits(),true);
  assert.ok(h.state.project.tracks[0].volume<before); assert.equal(h.state.undo.length,depth+1);
  assert.equal(h.MK3.hasPendingEdits,false);
});
test('disconnect releases notes and expression without triggering a step erase', async () => {
  const h=session(); h.MK3.event('mk3.status',{connected:true}); h.press('keyboard'); h.down(1);
  h.input({type:'pedal',down:true}); h.input({type:'button',name:'erase',pressed:true});
  const before=JSON.stringify(h.ST.pattern().steps); h.calls.length=0;
  h.MK3.event('mk3.status',{connected:false}); await tick();
  assert.equal(h.MK3.state.pads.size,0); assert.equal(h.MK3.state.held.size,0); assert.equal(h.MK3.state.pedal,false);
  assert.ok(h.calls.some(x=>x.command==='note.off'));
  assert.ok(h.calls.some(x=>x.command==='midi.expression'&&x.payload.kind==='sustain'&&x.payload.value===0));
  assert.equal(JSON.stringify(h.ST.pattern().steps),before);
});
test('native cancelled release cannot become a tap-to-erase', async () => {
  const h=session(); h.MK3.event('mk3.status',{connected:true});
  h.input({type:'button',name:'erase',pressed:true}); const before=JSON.stringify(h.ST.pattern().steps);
  h.input({type:'button',name:'erase',pressed:false,cancelled:true});
  assert.equal(JSON.stringify(h.ST.pattern().steps),before); assert.equal(h.MK3.state.held.size,0);
});
test('session replacement cancels old buffered edits and modifiers', async () => {
  const h=session(); h.press('mixer'); h.input({type:'encoder',index:0,delta:-0.2});
  h.input({type:'button',name:'shift',pressed:true}); h.MK3.event('project.loaded',{});
  assert.equal(h.MK3._bursts.size,0); assert.equal(h.MK3.state.shift,false);
});
test('Shift+MOD cutoff remains bound to the instrument at gesture start', async () => {
  const h=session(); h.state.instrument=0; h.input({type:'button',name:'shift',pressed:true}); h.press('mod');
  h.input({type:'touch',target:'strip',touched:true}); h.input({type:'strip',value:0.5});
  const other=h.state.project.instruments[1].cutoff; h.state.instrument=1;
  h.input({type:'touch',target:'strip',touched:false});
  assert.ok(h.state.project.instruments[0].cutoff<1000); assert.equal(h.state.project.instruments[1].cutoff,other);
});

test('disconnect clears pressure shared by multiple held pads', async () => {
  const h=session(); h.MK3.event('mk3.status',{connected:true}); h.press('keyboard'); h.down(1); h.down(2);
  for (const pad of [1,2]) h.input({type:'pad',pad,phase:'pressure',pressure:0.8});
  h.MK3.event('mk3.status',{connected:false}); await tick();
  const pressure=h.calls.filter(x=>x.command==='midi.expression'&&x.payload.kind==='aftertouch');
  assert.equal(pressure.at(-1).payload.value,0); h.advance(20); await tick();
  assert.equal(h.calls.filter(x=>x.command==='midi.expression'&&x.payload.kind==='aftertouch').at(-1).payload.value,0);
});
test('validation rollback preserves every pending encoder on unchanged clone', async () => {
  const h=session(); h.press('mixer'); h.input({type:'encoder',index:0,delta:-0.2}); h.input({type:'encoder',index:1,delta:-0.2});
  h.win.STFreeze={checkMutation:()=> 'blocked',explain(){}};
  assert.equal(h.MK3.commitBursts(),false); assert.equal(h.MK3._bursts.size,2);
  h.win.STFreeze.checkMutation=()=>null;
  assert.equal(await h.MK3.flushEdits(),true); assert.equal(h.MK3._bursts.size,0); assert.equal(h.state.undo.length,2);
});
test('tempo encoder edits the song bpm directly (no tempo map) as one flushed edit', async () => {
  const h=session(); const before=h.state.project.bpm;
  h.input({type:'button',name:'tempo',pressed:true}); h.input({type:'main',delta:5});
  assert.equal(await h.MK3.flushEdits(), true); assert.equal(h.MK3.hasPendingEdits, false); assert.equal(h.state.project.bpm, before+5);
  assert.equal(Object.hasOwn(h.state.project,'timing'), false);
});

test('pending mixer gesture cannot land on a track moved into its old index', async () => {
  const h=session();h.press('mixer');h.input({type:'encoder',index:0,delta:-.2});
  const survivor=h.state.project.tracks[1],before=survivor.volume;h.ST.perform('removeTrack');
  await h.MK3.flushEdits();assert.equal(h.state.project.tracks[0],survivor);assert.equal(survivor.volume,before);
});
test('pending sampler gesture cannot land on a replacement instrument', async () => {
  const h=session();h.press('channel');h.input({type:'encoder',index:0,delta:-.2});
  h.ST.perform('removeInstrument');const before=JSON.stringify(h.state.project.instruments);
  await h.MK3.flushEdits();assert.equal(JSON.stringify(h.state.project.instruments),before);
});

test('envelope knobs cover engine endpoints and remain finite at instant attack', async () => {
  const h=session(); h.press('channel'); h.MK3.state.page.channel=1;
  for (const [index,key,min] of [[0,'attack',0],[1,'decay',0],[3,'release',.0001],[4,'filterAttack',0],[5,'filterDecay',0],[7,'filterRelease',.0001]]) {
    h.state.project.instruments[0][key]=min;
    assert.equal(h.MK3.page().knobs[index].norm,0,key+' minimum normalization');
    h.MK3.page().knobs[index].turn(.01); await h.MK3.flushEdits();
    assert.ok(Number.isFinite(h.state.project.instruments[0][key]) && h.state.project.instruments[0][key]>min);
    h.MK3.page().knobs[index].turn(1); await h.MK3.flushEdits(); assert.equal(h.state.project.instruments[0][key],60);
    h.MK3.page().knobs[index].turn(-1); await h.MK3.flushEdits(); assert.equal(h.state.project.instruments[0][key],min);
  }
  h.state.project.instruments[0].type='sample';h.state.project.instruments[0].mode='granular';h.MK3.state.page.channel=5;
  h.MK3.page().knobs[1].turn(-1);await h.MK3.flushEdits();assert.equal(h.state.project.instruments[0].grainSize,.002);
});

test('discrete plug-in encoders and step locks move one legal interval per detent', async () => {
  const h=session(), param={address:'1',name:'Mode',display:'Mode 2',value:.5,writable:true,automatable:false,stepCount:4,normalizedStep:.25};
  const values=[]; h.win.STPluginPages={render:()=>'',bind(){},event(){},controller:{view:()=>({name:'Test',slot:'instrument',kind:'instrument',instanceID:'fixture-plugin',pages:['Main'],page:0,slots:[{key:'instrument',slot:0}],knobs:[param]}),adjust:(i,delta)=>{param.value=Math.max(0,Math.min(1,Math.round((param.value+delta)*4)/4));values.push(param.value);}}};
  h.press('plugin');h.input({type:'encoder',index:0,delta:.03});assert.equal(values.at(-1),.75);
  h.input({type:'button',name:'shift',pressed:true});h.input({type:'encoder',index:0,delta:-.03});assert.equal(values.at(-1),.5);h.input({type:'button',name:'shift',pressed:false});
  h.input({type:'button',name:'lock',pressed:true});h.input({type:'encoder',index:0,delta:.03});await h.MK3.flushEdits();
  const fx=h.ST.pattern().steps[0][0].fx1;assert.equal(fx.type,'plugin');assert.equal(fx.value,.75);
  assert(values.every(value=>Number.isInteger(value*4)));
});

for (const [label, response, expected, succeeds] of [
  ['absent device', {connected:false,screens:false,exclusive:false,error:'Connect your Maschine MK3.'}, 'Connect your Maschine MK3.', false],
  ['occupied screens', {connected:true,screens:false,exclusive:false,error:'USB interface is busy.'}, 'USB interface is busy.', false],
  ['incomplete screen acknowledgement', {connected:true,screens:false,exclusive:true}, 'screens could not be opened', false],
  ['missing reply', null, 'No Maschine MK3 connected', false],
  ['verified screen ownership', {connected:true,screens:true,exclusive:true}, 'screens taken over', true],
]) test('takeover reports '+label+' accurately', async () => {
  const h=session(), notices=[], requests=[];let closed=0;
  h.ST.openModal=()=>{};h.ST.closeModal=()=>closed++;h.ST.toast=(message,kind)=>notices.push({message,kind});
  h.ST.request=async(command,payload)=>{requests.push({command,payload});return response;};
  h.MK3.command('mk3Connect');
  assert.equal(requests.length,0,'confirmation must precede takeover');
  await h.element('mk3-takeover-confirm').click();
  assert.equal(closed,1);assert.equal(requests.filter(r=>r.command==='mk3.connect').length,1);
  assert.ok(notices.at(-1).message.includes(expected),JSON.stringify(notices));
  assert.equal(notices.at(-1).kind==='error',!succeeds);
  assert.equal(notices.some(n=>n.message==='Maschine MK3 screens taken over'),succeeds);
});
test('settings retains a safe, readable takeover error', async () => {
  const h=session();h.MK3.event('mk3.status',{connected:false,error:'USB <device> unavailable & retry'});
  const note=h.MK3.settingsNote();assert.match(note,/role="alert"/);assert.match(note,/USB &lt;device&gt; unavailable &amp; retry/);assert.ok(!note.includes('<device>'));
});
test('takeover request rejection remains an error', async () => {
  const h=session(),notices=[];h.ST.openModal=()=>{};h.ST.closeModal=()=>{};h.ST.toast=(message,kind)=>notices.push({message,kind});
  h.ST.request=async()=>{throw new Error('USB request failed');};h.MK3.command('mk3Connect');await h.element('mk3-takeover-confirm').click();
  assert.deepEqual(notices.at(-1),{message:'USB request failed',kind:'error'});
  assert.ok(!notices.some(n=>n.message==='Maschine MK3 screens taken over'));
});

(async()=>{let failed=0;for(const [name,fn] of cases){try{await fn();console.log('PASS '+name);}catch(e){failed++;console.error('FAIL '+name+'\n'+e.stack);}} console.log(`${cases.length-failed}/${cases.length} MK3 regression groups passed`); if(failed)process.exitCode=1;})();
