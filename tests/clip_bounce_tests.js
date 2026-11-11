'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const installEngineSyncBoundary = require('./helpers/engine_sync_boundary');
const copy = value => JSON.parse(JSON.stringify(value));
const source = web.source('clip-arranger.js');
const deferred = () => { let resolve, reject; const promise = new Promise((yes,no) => { resolve=yes;reject=no; }); return {promise,resolve,reject}; };
async function tick() { await new Promise(resolve=>setImmediate(resolve)); }
function harness({standard=false}={}) {
  const nodes=new Map(),calls=[],messages=[],undo=[],handlers=new Map(),order=[];let serial=0,body='',title='',footer='';
  const document={activeElement:null,getElementById:id=>nodes.get(id)||null,querySelectorAll:()=>[]};
  const state={view:'song',track:0,sequence:0,modal:null,project:{name:'Bounce fixture',bpm:120,linesPerBeat:4,activePattern:0,masterVolume:.6,
    tracks:[{id:0,routingID:'source',name:'Source',instrument:0,volume:.65,pan:.25,mute:false,solo:true,delaySend:.35,reverbSend:.4}],
    instruments:[{id:0,type:'synth'}],audioPlugins:[{owner:'track:source',track:0,kind:'effect',slot:0,state:'original patch'}],
    patterns:[{id:0,length:4,steps:[[{note:60},null,null,null]],automation:[{track:0,target:'volume',points:[{row:0,value:.65}]}]}],sequence:[0],audioClips:[],
    routing:{version:1,mixModel:'standard',buses:[],outputs:{'track:source':'master'},sends:[],sidechains:[]}}};
  if(standard)state.project.routing={version:1,mixModel:'standard',buses:[{id:'group',name:'Group',type:'group',volume:.8,pan:0,mute:false,solo:false}],outputs:{'track:source':'bus:group','bus:group':'master'},sends:[{id:'send',source:'track:source',destination:'bus:group',tap:'preFader',gain:.2,enabled:true}],sidechains:[]};
  const ST={state,escapeHTML:String,uid:()=>`bounce-track-${++serial}`,refresh(){},toast:(message,type)=>messages.push({message,type}),
    openModal(name,html,foot,options={}){title=name;body=html;footer=foot;state.modal=options.kind||'dialog';nodes.clear();for(const id of ['clips-bounce-tail','clips-bounce-render','clips-bounce-cancel','clips-bounce-status'])nodes.set(id,{id,value:id==='clips-bounce-tail'?'3':'',disabled:false,textContent:'',focus(){document.activeElement=this;}});},
    closeModal(){state.modal=null;nodes.clear();document.activeElement=null;},
    mutate(label,fn){undo.push({label,project:copy(state.project)});fn(state.project);},
    async waitForGraphEdit(){order.push('wait');if(handlers.has('wait'))await handlers.get('wait')();},
    async captureNativeEdits(){order.push('capture');return handlers.has('capture')?handlers.get('capture')():true;},
    cancelPendingEngineSync(){order.push('cancel-sync');},
    async request(command,payload={}){calls.push({command,payload:copy(payload)});order.push(command);if(handlers.has(command))return handlers.get(command)(payload);if(command==='audio.bounceTrack')return {path:'/render/Source.wav',clip:{track:0,startBeat:0,durationBeats:8,sourceOffset:0,gain:1,fadeIn:0,fadeOut:0,muted:false}};return {};}};
  const syncs=installEngineSyncBoundary(ST);
  const context=vm.createContext({window:{ST,TRACK_SANITY_LIMIT:1024},document,queueMicrotask(){},setTimeout(){},clearTimeout(){}});
  web.load(context, 'clip-arranger.js', { text: source });
  return {ST,syncs,state,nodes,calls,messages,undo,handlers,order,context,clips:context.window.STClips,get body(){return body;},get title(){return title;},get footer(){return footer;},start(){return nodes.get('clips-bounce-render').onclick();},restore(){state.project=copy(undo.pop().project);}};
}
const tests=[];const test=(name,fn)=>tests.push({name,fn});

test('a refused or rolled-back stem attachment reports the retained file and keeps the dialog open',async()=>{
 for(const refusal of ['blocked','rollback']){
  const h=harness(),before=copy(h.state.project);h.ST.mutate=(label,fn)=>{if(refusal==='rollback'){fn(h.state.project);h.state.project=copy(before);}};
  h.clips.bounceTrack();await h.start();assert.deepEqual(copy(h.state.project),before);assert.equal(h.undo.length,0);
  assert.equal(h.state.modal,'clip-bounce');assert.equal(h.messages.at(-1).type,'error');assert.match(h.messages.at(-1).message,/\/render\/Source.wav/);
 }
});
test('the capture describes its boundary and adds a muted neutral stem with a stable identity, without altering the source mix',async()=>{
  const h=harness({standard:true}),before=copy(h.state.project);h.clips.bounceTrack();
  assert.match(h.title,/stem for audition/);assert.match(h.body,/before its parent bus/);assert.match(h.body,/Shared returns and downstream bus or Master processing are excluded/);assert.match(h.footer,/Render muted stem/);
  await h.start();const p=h.state.project,stem=p.tracks[1];
  assert.deepEqual(copy(p.tracks[0]),before.tracks[0]);assert.equal(stem.mute,true);assert.equal(stem.solo,false);assert.equal(stem.volume,1);assert.equal(stem.pan,0);assert.equal(stem.delaySend,0);assert.equal(stem.reverbSend,0);
  assert.equal(stem.routingID,'bounce-track-1');assert.equal(p.patterns[0].steps[1].length,4);assert.equal(p.audioClips[0].track,1);assert.equal(p.audioClips[0].path,'/render/Source.wav');
  assert.equal(p.routing.outputs['track:'+stem.routingID],'master');assert.equal(p.routing.outputs['track:source'],'bus:group');
  for(const key of ['buses','sends','sidechains'])assert.deepEqual(copy(p.routing[key]),before.routing[key]);
  assert.deepEqual(copy(p.audioPlugins),before.audioPlugins);assert.deepEqual(copy(p.patterns[0].automation),before.patterns[0].automation);
  assert.equal(h.undo.length,1);assert.match(h.messages.at(-1).message,/Muted stem added/);assert.equal(h.state.track,1);
  h.restore();assert.deepEqual(copy(h.state.project),before);
});

test('render waits for actual graph completion and captures current settings before strict synchronization',async()=>{
  const h=harness({standard:true}),gate=deferred();h.handlers.set('wait',()=>gate.promise);h.handlers.set('capture',()=>{h.state.project.audioPlugins[0].state='latest editor patch';return true;});
  h.clips.bounceTrack();const render=h.start();await tick();assert.equal(h.calls.length,0);assert.equal(h.undo.length,0);
  gate.resolve();await render;assert.deepEqual(h.order,['wait','capture','wait','cancel-sync','engine.project','audio.bounceTrack']);
  assert.equal(h.calls[0].payload.project.audioPlugins[0].state,'latest editor patch');assert.equal(h.calls[1].payload.track,0);assert.equal(h.calls[1].payload.tail,3);
});

test('bounce snapshots after shared synchronization waits and awaits native restoration',async()=>{
  const h=harness({standard:true}),edits=deferred(),applied=deferred();h.ST.beforeEngineSync=()=>edits.promise;h.handlers.set('engine.project',()=>applied.promise);
  h.clips.bounceTrack();const render=h.start();await tick();assert.equal(h.syncs.length,1);assert.equal(h.syncs[0].options.capture,undefined);assert.equal(h.calls.length,0);
  h.state.project.audioPlugins[0].state='settled mapped edit';edits.resolve();await tick();
  assert.equal(h.calls[0].command,'engine.project');assert.equal(h.calls[0].payload.project.audioPlugins[0].state,'settled mapped edit');
  assert.equal(h.calls.some(x=>x.command==='audio.bounceTrack'),false);applied.resolve({});await render;
  assert.equal(h.undo.length,1);assert.equal(h.state.project.audioPlugins[0].state,'settled mapped edit');
});

test('shared sync cancellation and source changes during its wait never render',async()=>{
  for(const change of ['canceled','source','close','project','saving']){
    const h=harness({standard:true}),gate=deferred();h.ST.beforeEngineSync=()=>gate.promise;
    if(change==='canceled')h.ST.syncEngineProject=async()=>false;
    h.clips.bounceTrack();const render=h.start();await tick();
    if(change==='source')h.state.project.tracks[0]={...h.state.project.tracks[0]};if(change==='close')h.ST.closeModal();if(change==='project')h.state.project=copy(h.state.project);if(change==='saving')h.ST.isChangingSession=true;
    gate.resolve();await render;assert.equal(h.calls.length,0,change);assert.equal(h.undo.length,0,change);
  }
});

test('a closed preflight or replaced source never starts an outdated render',async()=>{
  for(const change of ['close','project','source']){
    const h=harness({standard:true}),gate=deferred();h.handlers.set('wait',()=>gate.promise);h.clips.bounceTrack();const render=h.start();await tick();
    if(change==='close')h.ST.closeModal();if(change==='project')h.state.project=copy(h.state.project);if(change==='source')h.state.project.tracks[0]={...h.state.project.tracks[0]};
    gate.resolve();await render;assert.equal(h.calls.length,0);assert.equal(h.undo.length,0);
  }
});

test('native capture, synchronization and render failures preserve source and arrangement',async()=>{
  for(const stage of ['capture','engine.project','audio.bounceTrack']){
    const h=harness({standard:true}),before=copy(h.state.project);h.handlers.set(stage,()=>{if(stage==='capture')return false;throw new Error(stage+' failed');});h.clips.bounceTrack();await h.start();
    assert.deepEqual(copy(h.state.project),before);assert.equal(h.undo.length,0);assert.equal(h.nodes.get('clips-bounce-render').disabled,false);assert.equal(h.messages.at(-1).type,'error');
    if(stage!=='audio.bounceTrack')assert.equal(h.calls.some(x=>x.command==='audio.bounceTrack'),false);
  }
});

test('duplicate starts and reopening during rendering cannot issue another native render',async()=>{
  const h=harness(),gate=deferred();h.handlers.set('audio.bounceTrack',()=>gate.promise);h.clips.bounceTrack();const button=h.nodes.get('clips-bounce-render'),render=h.start();await tick();
  await button.onclick();h.clips.bounceTrack();assert.equal(h.calls.filter(x=>x.command==='audio.bounceTrack').length,1);assert.equal(h.nodes.get('clips-bounce-render'),button);
  gate.resolve({path:'/render/kept.wav',clip:{startBeat:0,durationBeats:4}});await render;assert.equal(h.undo.length,1);
});

test('late render results preserve newer edits and report the retained audio file',async()=>{
  for(const change of ['project','notes','close','track-capacity','clip-capacity']){
    const h=harness({standard:true}),gate=deferred();h.handlers.set('audio.bounceTrack',()=>gate.promise);h.clips.bounceTrack();const render=h.start();await tick();
    assert.equal(h.calls.filter(x=>x.command==='audio.bounceTrack').length,1,'The deferred native render must start before simulating a late edit');
    if(change==='project')h.state.project=copy(h.state.project);if(change==='notes')h.state.project.patterns[0].steps[0][0].note=72;if(change==='close')h.ST.closeModal();
    if(change==='track-capacity')while(h.state.project.tracks.length<1024)h.state.project.tracks.push({routingID:'other-'+h.state.project.tracks.length});
    if(change==='clip-capacity')h.state.project.audioClips=Array.from({length:256},(_,i)=>({id:String(i),track:0}));
    const current=copy(h.state.project);gate.resolve({path:'/render/retained.wav',clip:{startBeat:0,durationBeats:4}});await render;
    assert.deepEqual(copy(h.state.project),current);assert.equal(h.undo.length,0);assert.match(h.messages.at(-1).message,/\/render\/retained.wav/);
  }
});

test('invalid tails and full sessions stop before native work',async()=>{
  for(const tail of ['', '-1', '31', 'not-a-number']){
    const h=harness();h.clips.bounceTrack();h.nodes.get('clips-bounce-tail').value=tail;await h.start();assert.equal(h.calls.length,0);assert.equal(h.undo.length,0);
  }
  const full=harness();while(full.state.project.tracks.length<1024)full.state.project.tracks.push({name:'Track'});full.clips.bounceTrack();assert.equal(full.state.modal,null);assert.equal(full.calls.length,0);
});

test('save and plug-in preparation races ask for retry without starting a render',async()=>{
  for(const pending of ['save','rack','move','routing']){
    const h=harness();if(pending==='save')h.ST.isChangingSession=true;
    h.context.window.STStudio={isEditingPlugin:pending==='rack',isPreparingEffectMove:pending==='move'};h.context.window.STRouting={isPreparing:pending==='routing'};
    h.clips.bounceTrack();await h.start();assert.equal(h.calls.length,0);assert.equal(h.undo.length,0);assert.match(h.messages.at(-1).message,/Finish the current/);
  }
  const h=harness(),capture=deferred();h.handlers.set('capture',()=>capture.promise);h.clips.bounceTrack();const render=h.start();await tick();h.ST.isChangingSession=true;capture.resolve(true);await render;assert.equal(h.calls.length,0);assert.equal(h.undo.length,0);
});
test('the Bounce button stays enabled past 16 tracks; only the track sanity limit disables it',()=>{
  const h=harness(),button=()=>/<button id="clips-bounce"[^>]*>/.exec(h.clips.render())[0];
  for(let i=1;i<20;i++)h.state.project.tracks.push({...copy(h.state.project.tracks[0]),id:i,routingID:'t'+i,solo:false});
  assert.doesNotMatch(button(),/disabled/,'20 tracks: bounce adds the 21st');
  h.context.window.TRACK_SANITY_LIMIT=20;assert.match(button(),/ disabled>/);assert.match(button(),/already has 20 tracks/);
});

(async()=>{for(const {name,fn} of tests){await fn();console.log('PASS '+name);}console.log(`Clip bounce: ${tests.length} regression groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
