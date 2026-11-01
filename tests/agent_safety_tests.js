'use strict';
// Actual renderer + STAgent handlers, with controlled native promises. No app/audio/vendor execution.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const web = require('./helpers/web');
const fixture=fs.readFileSync(path.join(__dirname,'agent_api_tests.js'),'utf8');
const makeHarness=new Function('require','__dirname',fixture.slice(0,fixture.indexOf('\nconst tests = [], test ='))+'\nreturn harness;')(require,__dirname);
const clone=x=>JSON.parse(JSON.stringify(x)),tick=async()=>{for(let i=0;i<60;i++)await Promise.resolve()};
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no});return{promise,resolve,reject}};
const parameters=[{address:'7',name:'Cutoff',id:'cutoff',min:0,max:100,normalized:.2,writable:true},{address:'8',name:'Resonance',id:'res',min:0,max:1,normalized:0,writable:true}];
function harness(){const h=makeHarness();web.load(h.context, 'export.js');h.state.native=true;h.ST.captureNativeEdits=async()=>true;h.ST.waitForGraphEdit=async()=>{};h.ST.waitForEngineUpdate=async()=>{};faithfulReads(h);return h;}
// Like a real plug-in, parameter reads report the values written before (setPluginParameters reads them back).
function faithfulReads(h){
 const live=new Map();let request=h.ST.request;
 Object.defineProperty(h.ST,'request',{configurable:true,get:()=>request,set:fn=>{request=async(command,payload)=>{const result=await fn(command,payload);
  if(command==='plugin.parameter'&&payload?.address!==undefined&&Number.isFinite(result?.normalized))live.set(String(payload.address),result.normalized);
  return command==='plugin.parameters'&&Array.isArray(result)?result.map(item=>live.has(String(item.address))?{...item,normalized:live.get(String(item.address))}:item):result;};}});
}
function effect(h){const descriptor={track:1,kind:'effect',slot:0,instanceID:'fx-1',component:'aufx:test',name:'Test FX',state:'ORIGINAL'};h.state.project.audioPlugins=[descriptor];h.state.project.tracks[1].plugins=[{name:'Test FX'}];h.state.dirty=false;return descriptor;}
const tests=[],test=(name,run)=>tests.push({name,run});

test('export pins the original project at each preparation await',async()=>{
 for(const stage of ['graph','capture','sync']){
  const h=harness(),gate=deferred(),calls=[];h.ST.cancelPendingEngineSync=()=>{};
  h.ST.waitForGraphEdit=()=>stage==='graph'?gate.promise:Promise.resolve();h.ST.captureNativeEdits=()=>stage==='capture'?gate.promise:Promise.resolve(true);
  h.ST.syncEngineProject=async make=>{if(stage==='sync')await gate.promise;make();return true};h.ST.request=async(command,payload)=>{calls.push(command);return{success:true,path:payload.path}};
  const job=h.call('exportAudio',{mode:'pattern',path:'/private/tmp/never-created.wav'});await tick();const replacement=clone(h.state.project);replacement.name='Replacement';h.state.project=replacement;gate.resolve(true);
  const reply=await job;assert.equal(reply.ok,false,stage);assert.match(reply.error,/changed/);assert.deepEqual(calls.filter(command=>command==='audio.export'),[],stage+': never exports replacement');
 }
});
test('export rejects a replaced pattern object even when its ID remains the same',async()=>{
 const h=harness(),gate=deferred();h.ST.waitForGraphEdit=()=>gate.promise;let exports=0;h.ST.request=async command=>{if(command==='audio.export')exports++;return{}};const job=h.call('exportAudio',{path:'/private/tmp/never-created.wav',mode:'pattern'});await tick();h.state.project.patterns[0]=clone(h.state.project.patterns[0]);gate.resolve();assert.equal((await job).ok,false);assert.equal(exports,0);
});
test('parameter discovery does not write to a replacement project, replaced slot or moved slot',async()=>{
 for(const replacement of ['project','slot','move']){
  const h=harness();effect(h);const gate=deferred(),calls=[];h.ST.request=async(command)=>{calls.push(command);if(command==='plugin.parameters')return gate.promise;throw Error('Unexpected native mutation '+command)};
  const job=h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});await tick();if(replacement==='project')h.state.project=clone(h.state.project);else if(replacement==='slot')h.state.project.audioPlugins[0]=clone(h.state.project.audioPlugins[0]);else h.state.project.audioPlugins[0].slot=2;const before=clone(h.state.project);gate.resolve(parameters);
  assert.equal((await job).ok,false);assert.deepEqual(calls,['plugin.parameters']);assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,0);assert.equal(h.agent.hasPendingEdits,false);assert.equal(h.state.dirty,false);
 }
});
test('late captured state cannot overwrite another saved copy with identical instance IDs',async()=>{
 const h=harness(),plugin=effect(h),gate=deferred();h.ST.request=async(command,payload)=>command==='plugin.parameters'?parameters:command==='plugin.parameter'?{normalized:payload.value}:gate.promise;
 const job=h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});await tick();assert.equal(h.agent.hasPendingEdits,true);
 const replacement=clone(h.state.project);replacement.audioPlugins[0].state='REPLACEMENT';h.state.project=replacement;h.state.dirty=false;gate.resolve({...plugin,state:'OLD-CAPTURE'});
 assert.equal((await job).ok,false);assert.equal(replacement.audioPlugins[0].state,'REPLACEMENT');assert.equal(h.state.undo.length,0);assert.equal(h.state.dirty,false);assert.equal(h.agent.hasPendingEdits,false);
});
test('late captured state cannot overwrite a moved or replaced slot in the same project',async()=>{
 const h=harness(),plugin=effect(h),gate=deferred();h.ST.request=async(command,payload)=>command==='plugin.parameters'?parameters:command==='plugin.parameter'?{normalized:payload.value}:gate.promise;
 const job=h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});await tick();h.state.project.audioPlugins[0]={...plugin,state:'LATEST'};gate.resolve({...plugin,state:'OLD-CAPTURE'});
 assert.equal((await job).ok,false);assert.equal(h.state.project.audioPlugins[0].state,'LATEST');assert.equal(h.state.undo.length,0);assert.equal(h.agent.hasPendingEdits,true,'one explicit cancellation acknowledgement blocks saving');assert.equal(await h.agent.flushEdits(),false);assert.equal(h.agent.hasPendingEdits,false);
});
test('instrument reassignment during capture never attaches old state to the new instrument',async()=>{
 const h=harness(),plugin=effect(h),gate=deferred();plugin.kind='instrument';h.state.project.tracks[1].instrument=1;h.state.project.instruments[1].type='au';h.state.project.instruments[2].type='au';h.state.project.instruments[2].plugin={state:'NEW-INSTRUMENT'};
 h.ST.request=async(command,payload)=>command==='plugin.parameters'?parameters:command==='plugin.parameter'?{normalized:payload.value}:gate.promise;
 const job=h.call('setPluginParameters',{track:1,values:{Cutoff:50}});await tick();h.state.project.tracks[1].instrument=2;gate.resolve({...plugin,state:'OLD-INSTRUMENT'});
 assert.equal((await job).ok,false);assert.equal(h.state.project.instruments[2].plugin.state,'NEW-INSTRUMENT');assert.equal(h.state.undo.length,0);
});
test('a partial parameter failure captures the actual subset as one undoable edit',async()=>{
 const h=harness(),plugin=effect(h),calls=[];h.ST.request=async(command,payload)=>{calls.push(command);if(command==='plugin.parameters')return parameters;if(command==='plugin.parameter'){if(payload.address==='8')throw Error('second rejected');return{normalized:payload.value}}return{...plugin,state:'ACTUAL-FIRST-ONLY'}};
 const reply=await h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50,Resonance:.5}});
 assert.equal(reply.ok,false);assert.match(reply.error,/actual resulting state was captured as one undoable edit/);assert.equal(h.state.project.audioPlugins[0].state,'ACTUAL-FIRST-ONLY');assert.equal(h.state.undo.length,1);assert.equal(h.state.undo[0].project.audioPlugins[0].state,'ORIGINAL');assert.equal(h.state.dirty,true);assert.equal(h.agent.hasPendingEdits,false);assert.equal(await h.agent.flushEdits(),true);assert.equal(calls.filter(x=>x==='plugin.parameter').length,2);
});
test('failed capture stays pending and retries capture only, without resending parameters',async()=>{
 const h=harness(),plugin=effect(h);let writes=0,captures=0,failCapture=true;
 h.ST.request=async(command,payload)=>{if(command==='plugin.parameters')return parameters;if(command==='plugin.parameter'){writes++;return{normalized:payload.value}}captures++;if(failCapture)throw Error('capture unavailable');return{...plugin,state:'ACTUAL'}};
 const generation=h.agent.editGeneration,reply=await h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});
 assert.equal(reply.ok,false);assert.equal(h.agent.hasPendingEdits,true);assert(h.agent.editGeneration>generation);assert.equal(h.state.dirty,true);assert.equal(h.state.undo.length,0);assert.equal(await h.agent.flushEdits(),false);failCapture=false;
 assert.equal(await h.agent.flushEdits(),true);assert.equal(h.agent.hasPendingEdits,false);assert.equal(writes,1);assert.equal(captures,3);assert.equal(h.state.undo.length,1);assert.equal(h.state.project.audioPlugins[0].state,'ACTUAL');
});
test('opaque fallback with stateError and wrong instance are not accepted as fresh captures',async()=>{
 for(const bad of [{stateError:'capture failed'},{instanceID:'other'},{slot:3},{component:'different'},{state:''}]){
  const h=harness(),plugin=effect(h);let correct=false;h.ST.request=async(command,payload)=>command==='plugin.parameters'?parameters:command==='plugin.parameter'?{normalized:payload.value}:{...plugin,state:'FRESH',...(correct?{}:bad)};
  assert.equal((await h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}})).ok,false);assert.equal(h.state.project.audioPlugins[0].state,'ORIGINAL');assert.equal(h.state.undo.length,0);assert.equal(h.agent.hasPendingEdits,true);correct=true;assert.equal(await h.agent.flushEdits(),true);assert.equal(h.state.project.audioPlugins[0].state,'FRESH');
 }
});
test('flush awaits an already issued native write without a self-deadlock or replay',async()=>{
 const h=harness(),plugin=effect(h),gate=deferred();let writes=0,settled=false;h.ST.request=async(command)=>{if(command==='plugin.parameters')return parameters;if(command==='plugin.parameter'){writes++;return gate.promise}return{...plugin,state:'FRESH'}};
 const job=h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});await tick();const flush=h.agent.flushEdits().then(result=>{settled=true;return result});await tick();assert.equal(settled,false);gate.resolve({normalized:.5});assert.equal((await job).ok,true);assert.equal(await flush,true);assert.equal(writes,1);assert.equal(h.state.undo.length,1);
});
test('a preflight capture may settle an equivalent descriptor, but not a new instance',async()=>{
 for(const different of [false,true]){
  const h=harness(),plugin=effect(h);let writes=0;h.ST.captureNativeEdits=async()=>{h.state.project.audioPlugins[0]={...plugin,instanceID:different?'new':'fx-1',state:'SETTLED'};return true};h.ST.request=async(command,payload)=>{if(command==='plugin.parameters')return parameters;if(command==='plugin.parameter'){writes++;return{normalized:payload.value}}return{...plugin,state:'FRESH'}};
  const reply=await h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});assert.equal(reply.ok,!different);assert.equal(writes,different?0:1);
 }
});
test('default effect load fills a free sparse slot; a full rack requires explicit replacement',async()=>{
 for(const full of [false,true]){
  const h=harness();h.state.plugins=[{id:'aufx:new',name:'New FX',type:'effect',format:'AU'}];const old=(full?Array.from({length:16},(_,i)=>i):[0,2,8,15]).map(slot=>({track:0,kind:'effect',slot,instanceID:'old'+slot,name:'Old'+slot,state:'KEEP'}));h.state.project.audioPlugins=clone(old);let loaded;
  h.ST.request=async(command,payload)=>{if(command==='plugin.load'){loaded={...payload,name:'New FX',instanceID:'new',state:'NEW'};return loaded}if(command==='plugin.state')return[...old,loaded];throw Error(command)};
  const reply=await h.call('loadPlugin',{plugin:'New FX',track:0,kind:'effect'});assert.equal(reply.ok,!full);if(full){assert.equal(loaded,undefined);assert.match(reply.error,/occupied/);assert.equal(h.state.undo.length,0)}else{assert.equal(loaded.slot,1);assert.equal(h.state.project.audioPlugins.find(x=>x.slot===2).state,'KEEP')}
 }
});
test('transport validates all fields before stop, mode or selection mutation',async()=>{
 for(const invalid of [{row:9999},{block:9999},{loop:{from:0,to:9999}},{loop:3}]){
  const h=harness(),p=h.state.project,pat=clone(p.patterns[0]);pat.id=42;p.patterns.push(pat);p.activePattern=0;h.state.row=3;h.state.playing=true;h.state.playbackMode='song';const calls=[];h.ST.perform=async name=>{calls.push(name);if(name==='stop')h.state.playing=false;return true};
  const reply=await h.call('transport',{action:'playPattern',pattern:42,...invalid});assert.equal(reply.ok,false);assert.equal(p.activePattern,0);assert.equal(h.state.row,3);assert.equal(h.state.playbackMode,'song');assert.deepEqual(calls,[]);
 }
});
test('transport does not start a replacement project after awaiting Stop',async()=>{
 const h=harness(),gate=deferred(),calls=[];h.state.playing=true;h.ST.perform=async name=>{calls.push(name);if(name==='stop')return gate.promise;throw Error('unexpected '+name)};
 const job=h.call('transport',{action:'playPattern',row:2});await tick();h.state.project=clone(h.state.project);h.state.playing=false;gate.resolve(true);const reply=await job;assert.equal(reply.ok,false);assert.match(reply.error,/changed/);assert.deepEqual(calls,['stop']);
});
test('actual Save waits for an issued agent capture and saves its state without a deadlock',async()=>{
 let h;const native=[];h=makeHarness('pattern',{'engine.snapshot':()=>({audioPlugins:clone(h.state.project.audioPlugins)}),'project.saveAs':payload=>{native.push(clone(payload));return{saved:true,path:payload.path,project:clone(payload.project)}}});
 faithfulReads(h);const plugin=effect(h),gate=deferred();h.ST.request=async(command,payload)=>command==='plugin.parameters'?parameters:command==='plugin.parameter'?{normalized:payload.value}:gate.promise;
 const edit=h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}});await tick();assert.equal(h.agent.hasPendingEdits,true);
 const save=h.ST.projectAction('saveAs','/private/tmp/never-created.supertracker');await tick();assert.equal(native.length,0);gate.resolve({...plugin,state:'CAPTURED-BEFORE-SAVE'});
 assert.equal((await edit).ok,true);assert.equal(await save,true);assert.equal(native.length,1);assert.equal(native[0].project.audioPlugins[0].state,'CAPTURED-BEFORE-SAVE');assert.equal(h.agent.hasPendingEdits,false);assert.equal(h.state.dirty,false);
});
test('actual Undo retries failed capture before undoing the live edit, with no repeated native write',async()=>{
 let h;h=makeHarness('pattern',{'engine.snapshot':()=>({audioPlugins:clone(h.state.project.audioPlugins)})});const plugin=effect(h);let good=false,writes=0;
 faithfulReads(h);h.ST.request=async(command,payload)=>{if(command==='plugin.parameters')return parameters;if(command==='plugin.parameter'){writes++;return{normalized:payload.value}}if(!good)throw Error('capture unavailable');return{...plugin,state:'CAPTURED'}};
 assert.equal((await h.call('setPluginParameters',{track:1,kind:'effect',values:{Cutoff:50}})).ok,false);assert.equal(h.agent.hasPendingEdits,true);good=true;
 await h.ST.perform('undo');assert.equal(writes,1);assert.equal(h.agent.hasPendingEdits,false);assert.equal(h.state.project.audioPlugins[0].state,'ORIGINAL');assert.equal(h.state.redo.length,1);
});
test('Agent export shares the UI lock through native render and blocks edits, Undo and Save',async()=>{
 const h=harness(),render=deferred(),calls=[];await h.ok('writeSteps',{steps:[{track:0,row:0,note:60}]});h.ST.cancelPendingEngineSync=()=>{};h.ST.syncEngineProject=async make=>{make();return true};
 h.ST.request=async(command,payload)=>{calls.push({command,payload});if(command==='audio.export')return render.promise;return{}};
 const job=h.call('exportAudio',{path:'/private/tmp/never-created.wav',mode:'pattern'});await tick();assert.equal(h.window.STExport.isBusy,true);assert.equal(calls.filter(x=>x.command==='audio.export').length,1);
 const before=clone(h.state.project),undo=h.state.undo.length;assert.equal(h.ST.mutate('must not edit',p=>p.name='WRONG'),false);await h.ST.perform('undo');assert.equal(await h.ST.projectAction('saveAs','/private/tmp/never-created.supertracker'),false);assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,undo);assert.equal(h.calls.some(x=>x.command==='project.saveAs'),false);
 render.resolve({success:true,paths:['/private/tmp/never-created.wav']});assert.equal((await job).ok,true);assert.equal(h.window.STExport.isBusy,false);
});
test('Agent export cancellation retains the shared lock until both native promises settle',async()=>{
 const h=harness(),render=deferred(),stop=deferred();h.ST.cancelPendingEngineSync=()=>{};h.ST.syncEngineProject=async make=>{make();return true};h.ST.request=async command=>command==='audio.export'?render.promise:command==='transport.stop'?stop.promise:{};
 const job=h.call('exportAudio',{path:'/private/tmp/never-created.wav'});await tick();assert.equal(h.window.STExport.isBusy,true);assert.equal(h.window.STExport.cancel(),true);render.resolve({cancelled:true});await tick();assert.equal(h.window.STExport.isBusy,true);assert.equal(h.window.STExport.cancel(),false);stop.resolve({ok:true});const reply=await job;assert.equal(reply.ok,false);assert.match(reply.error,/canceled/);assert.equal(h.window.STExport.isBusy,false);
});
test('Agent export rejects incomplete output and failed preparation while releasing the lock',async()=>{
 for(const outcome of ['capture','native','empty','cancelled']){
  const h=harness();h.ST.cancelPendingEngineSync=()=>{};h.ST.syncEngineProject=async make=>{make();return true};if(outcome==='capture')h.ST.captureNativeEdits=async()=>false;
  h.ST.request=async command=>{if(command!=='audio.export')return{};if(outcome==='native')throw Error('render failed');return outcome==='cancelled'?{cancelled:true}:{success:true,path:'/private/tmp/never-created.wav'}};
  const reply=await h.call('exportAudio',{path:'/private/tmp/never-created.wav'});assert.equal(reply.ok,false,outcome);assert.equal(h.window.STExport.isBusy,false,outcome);
 }
});
test('quit admission rejects post-capture edits and resumes without changing AI preference',async()=>{
 const h=harness();h.agent.setConnection({available:true,enabled:true,clients:1});const before=clone(h.state.project),undo=h.state.undo.length;
 assert.equal(await h.agent.suspendForTermination(),true);assert.equal((await h.call('writeSteps',{steps:[{track:0,row:0,note:60}]})).ok,false);assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,undo);assert(h.agent.settingsField().includes('checked'));
 h.agent.resumeAfterTermination();assert.equal((await h.call('writeSteps',{steps:[{track:0,row:0,note:61}]})).ok,true);assert.equal(h.ST.pattern().steps[0][0].note,61);
});
test('quit settles the already running command but cancels queued project edits',async()=>{
 const h=harness(),gate=deferred();effect(h);h.ST.request=async()=>gate.promise;const active=h.call('pluginParameters',{track:1,kind:'effect'});await tick();const queued=h.call('writeSteps',{steps:[{track:0,row:0,note:61}]});const before=clone(h.state.project);let settled=false;const suspend=h.agent.suspendForTermination().then(x=>{settled=true;return x});await tick();assert.equal(settled,false);gate.resolve(parameters);assert.equal((await active).ok,true);assert.equal((await queued).ok,false);assert.equal(await suspend,true);assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,0);
});
test('quit timeout stays closed and old queued commands cannot revive after Cancel',async()=>{
 const h=harness(),gate=deferred();effect(h);h.ST.request=async()=>gate.promise;const active=h.call('pluginParameters',{track:1,kind:'effect'});await tick();const queued=h.call('writeSteps',{steps:[{track:0,row:0,note:61}]});const before=clone(h.state.project),suspend=h.agent.suspendForTermination(3000);h.flush(3000);assert.equal(await suspend,false);
 const during=h.call('writeSteps',{steps:[{track:0,row:1,note:62}]});h.agent.resumeAfterTermination();gate.resolve(parameters);assert.equal((await active).ok,true);assert.equal((await queued).ok,false);assert.equal((await during).ok,false);assert.deepEqual(clone(h.state.project),before);assert.equal(h.state.undo.length,0);assert.equal((await h.call('writeSteps',{steps:[{track:0,row:2,note:63}]})).ok,true);
});
(async()=>{let failures=0;for(const {name,run}of tests){try{await run();console.log('PASS '+name)}catch(e){failures++;console.error('FAIL '+name+'\n'+e.stack)}}console.log(`Agent safety: ${tests.length-failures}/${tests.length} groups passed.`);if(failures)process.exitCode=1})();
