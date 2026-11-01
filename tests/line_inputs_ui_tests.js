'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const web = require('./helpers/web');
const script = web.source('line-inputs.js'),copy=x=>JSON.parse(JSON.stringify(x));
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
function harness(overrides={}){
 const nodes=new Map(),calls=[],undo=[],toasts=[];
 const audio={inputDevice:22,lineInputs:[],inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}]};
 const state={project:{tracks:[{name:'First'},{name:'Second'}]},audio:{},modal:null};
 const window={},ST={state,escapeHTML:String,refresh(){},toast:t=>toasts.push(t),captureNativeEdits:async()=>true,
 request:async(command,payload)=>{calls.push({command,payload:copy(payload||{})});if(overrides[command])return overrides[command](payload);return copy(audio);},
 mutate(label,fn,options){assert.equal(options.lineInputCommit,true);undo.push(copy(state.project));fn(state.project);return true;},
 openModal(title,body,footer,options){state.modal=options.kind;for(const match of (body+footer).matchAll(/<([a-z]+)\b([^>]*)>/gi)){const attrs={};for(const a of match[2].matchAll(/([\w-]+)(?:="([^"]*)")?/g))attrs[a[1]]=a[2]||'';if(!attrs.id)continue;nodes.set(attrs.id,{id:attrs.id,value:attrs.value||'',checked:'checked'in attrs,disabled:false,textContent:'',listeners:{},addEventListener(type,fn){this.listeners[type]=fn;},focus(){},fire(type){return this.listeners[type]?.();}});}},
 closeModal(){if(window.STLineInputs.isBusy)return;window.STLineInputs.closed();state.modal=null;},syncEngineProject:async factory=>{calls.push({command:'sync',payload:factory()});return true;}};
 window.ST=ST;web.runText('line-inputs.js', script, {window,document:{getElementById:id=>nodes.get(id)}});
 return{state,ST,ui:window.STLineInputs,calls,undo,toasts,get:id=>nodes.get(id),async show(){return window.STLineInputs.show();},apply(){return nodes.get('line-input-apply').onclick();}};
}
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('mixer: the selected track shows its live input state and edits it in place as one Undo step',async()=>{
 const h=harness();h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[]};
 let html=h.ui.mixerSection(0);assert.match(html,/Live input/);assert.match(html,/>Off</);
 h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};
 html=h.ui.mixerSection(0);assert.match(html,/Not running/,'an enabled route the host does not run is not called live');
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];assert.match(h.ui.mixerSection(0),/is-live">Live</);
 const fields={enabled:{checked:true},left:{value:'2'},right:{value:'3'}},root={querySelector:selector=>fields[/"(\w+)"/.exec(selector)[1]]};
 assert.equal(await h.ui.applyMixer(1,root),true);
 const sent=h.calls.find(c=>c.command==='audio.inputs');assert.deepEqual(sent.payload.inputs,[{track:1,gain:1,enabled:true,deviceUID:'interface',channels:[2,3]}]);
 assert.equal(h.undo.length,1);assert.deepEqual(copy(h.state.project.tracks[1].audioInput),{gain:1,enabled:true,deviceUID:'interface',channels:[2,3]});
 fields.right.value='mono';fields.enabled.checked=false;assert.equal(await h.ui.applyMixer(1,root),true);
 assert.deepEqual(copy(h.state.project.tracks[1].audioInput.channels),[2]);assert.equal(h.state.project.tracks[1].audioInput.enabled,false);assert.equal(h.undo.length,2);
});
test('mixer: Monitor is on by default and switching it off is saved and sent as one Undo step',async()=>{
 const h=harness();h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[{track:0,state:'live',active:true}]};
 h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};
 assert.match(h.ui.mixerSection(0),/data-line-input-field="monitor" checked/);
 const fields={enabled:{checked:true},monitor:{checked:false},left:{value:'4'},right:{value:'5'}},root={querySelector:selector=>fields[/"(\w+)"/.exec(selector)[1]]};
 assert.equal(await h.ui.applyMixer(0,root),true);
 assert.equal(h.calls.find(c=>c.command==='audio.inputs').payload.inputs[0].monitor,false);
 assert.equal(h.state.project.tracks[0].audioInput.monitor,false);assert.equal(h.undo.length,1);
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];assert.match(h.ui.mixerSection(0),/Live · not monitored/);assert.doesNotMatch(h.ui.mixerSection(0),/data-line-input-field="monitor" checked/);
 fields.monitor.checked=true;assert.equal(await h.ui.applyMixer(0,root),true);
 assert.equal('monitor' in h.state.project.tracks[0].audioInput,false,'monitoring on is the default and is not stored');assert.equal(h.undo.length,2);
});
test('Recording page: an enabled route reads LIVE only while the host runs it',()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};h.state.view='recording';
 h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[],lineInputError:'Microphone access is off'};
 let html=h.ui.render();assert.match(html,/NOT RUNNING/);assert.match(html,/Microphone access is off/);assert.doesNotMatch(html,/>LIVE</);
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];html=h.ui.render();assert.match(html,/>LIVE</);
});
test('two live lanes apply mono/stereo maps in one acknowledged history edit',async()=>{
 const h=harness();await h.show();h.get('line-input-enabled-0').checked=true;h.get('line-input-left-0').value='3';h.get('line-input-enabled-1').checked=true;h.get('line-input-left-1').value='4';h.get('line-input-right-1').value='5';h.get('line-input-gain-1').value='0.5';await h.apply();
 assert.equal(h.undo.length,1);assert.equal(h.state.modal,null);assert.deepEqual(copy(h.state.project.tracks[0].audioInput.channels),[3]);assert.deepEqual(copy(h.state.project.tracks[1].audioInput.channels),[4,5]);assert.equal(h.state.project.tracks[1].audioInput.gain,.5);
});
test('pending native permission keeps project unchanged and prevents duplicate requests/close',async()=>{
 const gate=deferred(),h=harness({'audio.inputs':()=>gate.promise});await h.show();h.get('line-input-enabled-0').checked=true;const pending=h.apply();for(let i=0;i<12;i++)await Promise.resolve();assert(h.ui.isBusy);assert.equal(h.undo.length,0);assert.equal(h.ui.closed(),false);await h.apply();assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,1);gate.resolve({inputs:[],lineInputs:[]});await pending;assert.equal(h.undo.length,1);
});
test('invalid stereo maps fail before requesting microphone access',async()=>{
 const h=harness();await h.show();h.get('line-input-enabled-0').checked=true;h.get('line-input-right-0').value='0';await h.apply();assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,0);assert.match(h.get('line-input-status').textContent,/distinct/);
});
test('disabled missing interfaces retain their UID/channel metadata when another lane changes',async()=>{
 const h=harness(),saved={enabled:false,deviceUID:'missing',channels:[12,13],gain:.8};h.state.project.tracks[0].audioInput=copy(saved);await h.show();assert.equal(h.get('line-input-device-0').value,'missing');h.get('line-input-enabled-1').checked=true;await h.apply();assert.deepEqual(copy(h.state.project.tracks[0].audioInput),saved);assert.equal(h.calls.find(c=>c.command==='audio.inputs').payload.inputs.length,1);
});
test('unavailable enabled device fails clearly until explicitly relinked',async()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'missing',channels:[1],gain:1};await h.show();await h.apply();assert.match(h.get('line-input-status').textContent,/current Studio interface/);h.get('line-input-device-0').value='interface';h.get('line-input-device-0').fire('change');await h.apply();assert.equal(h.state.project.tracks[0].audioInput.deviceUID,'interface');assert.deepEqual(copy(h.state.project.tracks[0].audioInput.channels),[0]);
});
test('native rejection and refused history commits restore canonical native bindings',async()=>{
 for(const refusal of ['native','edit']){let count=0;const h=harness({'audio.inputs':()=>{if(refusal==='native'&&count++===0)throw new Error('Permission denied');return {inputs:[],lineInputs:[]};}});await h.show();h.get('line-input-enabled-0').checked=true;if(refusal==='edit')h.ST.mutate=()=>false;await h.apply();assert.equal(h.undo.length,0);assert.equal(h.state.project.tracks[0].audioInput,undefined);assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,2);assert.equal(h.ui.isBusy,false);assert.equal(h.state.modal,'line-inputs');}
});
test('stale project reply cannot alter replacement project and restores its current native state',async()=>{
 const gate=deferred(),h=harness({'audio.inputs':()=>gate.promise});await h.show();h.get('line-input-enabled-0').checked=true;const pending=h.apply();for(let i=0;i<12;i++)await Promise.resolve();h.state.project={tracks:[{name:'New project'}]};gate.resolve({inputs:[],lineInputs:[]});await pending;assert.equal(h.undo.length,0);assert.equal(h.state.project.tracks[0].audioInput,undefined);assert(h.calls.some(c=>c.command==='sync'));
});
test('false native acknowledgement never creates a history edit or claims success',async()=>{
 const h=harness({'audio.inputs':()=>false});await h.show();h.get('line-input-enabled-0').checked=true;await h.apply();assert.equal(h.undo.length,0);assert.equal(h.state.modal,'line-inputs');assert.match(h.get('line-input-status').textContent,/did not confirm/);assert.equal(h.toasts.length,0);
});
test('device discovery cannot replace a newer modal and a refused modal open remains clean',async()=>{
 const gate=deferred(),h=harness({'audio.devices':()=>gate.promise});const pending=h.show();h.state.modal='new-dialog';gate.resolve({inputs:[],lineInputs:[]});assert.equal(await pending,false);assert.equal(h.state.modal,'new-dialog');assert.equal(h.ui.isBusy,false);
 const refused=harness();refused.ST.openModal=()=>false;assert.equal(await refused.show(),false);assert.equal(refused.ui.isBusy,false);
});
test('saved enabled but inactive routes can be explicitly retried without changing channels',async()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[3],gain:1};await h.show();await h.apply();assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,1);assert.equal(h.state.modal,null);assert.deepEqual(copy(h.state.project.tracks[0].audioInput.channels),[3]);
});
test('unchanged disabled configurations close without native input or permission requests',async()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:false,deviceUID:'missing',channels:[3],gain:1};await h.show();await h.apply();assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,0);assert.equal(h.undo.length,0);assert.equal(h.state.modal,null);
});
test('mixer: a track deleted during the native input request never passes its route to the track that moved into its index',async()=>{
 const gate=deferred(),h=harness({'audio.inputs':payload=>payload.inputs[0].enabled?gate.promise:copy({inputs:[],lineInputs:[]})});
 h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[]};
 const fields={enabled:{checked:true},left:{value:'2'},right:{value:'3'}},root={querySelector:selector=>fields[/"(\w+)"/.exec(selector)[1]]};
 const pending=h.ui.applyMixer(0,root);for(let i=0;i<12;i++)await Promise.resolve();
 h.state.project.tracks.splice(0,1);gate.resolve({inputs:[],lineInputs:[]});
 assert.equal(await pending,false);assert.equal(h.undo.length,0);
 assert.equal(h.state.project.tracks[0].name,'Second');assert.equal(h.state.project.tracks[0].audioInput,undefined,'the shifted track keeps no input');
});
(async()=>{for(const {name,fn}of tests){await fn();console.log('PASS '+name);}console.log(`Line inputs UI: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
