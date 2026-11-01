'use strict';
// Real arranger module; no audio, DOM rendering, or application launch.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web = require('./helpers/web');
const copy=value=>JSON.parse(JSON.stringify(value));
function deferred(){let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};}
function harness(){
 const calls=[],messages=[],history=[],handlers=new Map(),state={view:'song',track:0,sequence:0,project:{bpm:120,linesPerBeat:4,tracks:[{name:'Original',routingID:'a'}],patterns:[{id:0,length:4}],sequence:[0],audioClips:[]}};
 const ST={state,escapeHTML:String,refresh(){},toast:(message,kind)=>messages.push({message,kind}),mutate(label,fn){history.push(copy(state.project));fn(state.project);},async request(command,payload){calls.push({command,payload});return handlers.has(command)?handlers.get(command)(payload):{duration:2,name:'Capture'};}};
 const context=vm.createContext({window:{ST},document:{activeElement:null,getElementById(){return null;}},queueMicrotask(){},setTimeout(){},clearTimeout(){}});
 web.load(context, 'clip-arranger.js');
 return {ST,state,calls,messages,history,handlers,context,ui:context.window.STClips};
}
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('refused and rolled-back import commits retain the file without reporting success',async()=>{
 for(const refusal of ['blocked','rollback']){
  const h=harness(),before=copy(h.state.project);h.ST.mutate=(label,fn)=>{if(refusal==='rollback'){fn(h.state.project);h.state.project=copy(before);}};
  assert.equal(await h.ui.addFile('/capture.wav'),null,refusal);assert.deepEqual(copy(h.state.project),before);
  assert(!h.messages.some(x=>/Audio added/.test(x.message)));assert.match(h.messages.at(-1).message,/capture.wav/);
 }
});
test('a delayed import cannot follow a removed or replaced track index',async()=>{
 for(const change of ['replace','remove','dialog']){
  const h=harness(),gate=deferred();h.state.project.tracks.push({name:'Other',routingID:'b'});
  h.handlers.set(change==='dialog'?'dialog.openSample':'audio.clipInfo',()=>gate.promise);
  const pending=change==='dialog'?h.ui.importFile():h.ui.addFile('/capture.wav');
  if(change==='replace')h.state.project.tracks[0]={...h.state.project.tracks[0]};else h.state.project.tracks.shift();
  const before=copy(h.state.project);gate.resolve(change==='dialog'?{path:'/capture.wav'}:{duration:2});
  assert.equal(await pending,null,change);assert.deepEqual(copy(h.state.project),before);assert.equal(h.history.length,0);
 }
});
test('clip capacity and operation locks are rechecked after asynchronous metadata',async()=>{
 for(const lock of ['capacity','save','export','routing']){
  const h=harness(),gate=deferred();h.handlers.set('audio.clipInfo',()=>gate.promise);const pending=h.ui.addFile('/capture.wav');
  if(lock==='capacity')h.state.project.audioClips=Array.from({length:256},(_,i)=>({id:String(i),track:0}));
  if(lock==='save')h.ST.isChangingSession=true;if(lock==='export')h.context.window.STExport={isBusy:true};if(lock==='routing')h.context.window.STRouting={isApplying:true};
  const before=copy(h.state.project);gate.resolve({duration:2});assert.equal(await pending,null,lock);assert.deepEqual(copy(h.state.project),before);assert.equal(h.history.length,0);
 }
});
test('successful imports retain one undo snapshot and cannot overlap another import',async()=>{
 const h=harness(),gate=deferred();h.handlers.set('audio.clipInfo',()=>gate.promise);const pending=h.ui.addFile('/capture.wav');
 const duplicate=h.ui.addFile('/other.wav');assert.equal(h.calls.length,1);assert.equal(await duplicate,null);
 gate.resolve({duration:2});const clip=await pending;assert(clip);assert.equal(h.state.project.audioClips[0],clip);assert.equal(h.history.length,1);assert.deepEqual(h.history[0].audioClips,[]);assert.equal(h.state.clipSelected,clip.id);
});
(async()=>{let failures=0;for(const {name,fn} of tests){try{await fn();console.log('PASS '+name);}catch(error){failures++;console.error('FAIL '+name+'\n'+error.stack);}}console.log(`Clip import commits: ${tests.length-failures}/${tests.length} groups passed.`);if(failures)process.exitCode=1;})();
