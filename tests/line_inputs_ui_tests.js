'use strict';
// Line inputs in the page: a track's audioInput in the project is the routing, so every change is an ordinary
// edit (one Undo step) and no host command routes anything. The host's `audio.devices` says what became of it.
const assert=require('node:assert/strict');
const web = require('./helpers/web');
const script = web.source('line-inputs.js'),copy=x=>JSON.parse(JSON.stringify(x));
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const settle=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
function harness(overrides={}){
 const nodes=new Map(),calls=[],undo=[],toasts=[];
 const audio={inputDevice:22,lineInputs:[],inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}]};
 const state={project:{tracks:[{name:'First'},{name:'Second'}]},audio:{},modal:null};
 const window={},ST={state,escapeHTML:String,refresh(){},toast:(text,kind)=>toasts.push({text,kind}),waitForEngineUpdate:async()=>{},
 request:async(command,payload)=>{calls.push({command,payload:copy(payload||{})});if(overrides[command])return overrides[command](payload);return copy(audio);},
 mutate(label,fn,options){assert.equal(options,undefined,'a line-input edit is an ordinary edit');undo.push({label,project:copy(state.project)});fn(state.project);return true;},
 openModal(title,body,footer,options){state.modal=options.kind;for(const match of (body+footer).matchAll(/<([a-z]+)\b([^>]*)>([^<]*)/gi)){const attrs={};for(const a of match[2].matchAll(/([\w-]+)(?:="([^"]*)")?/g))attrs[a[1]]=a[2]||'';if(!attrs.id)continue;nodes.set(attrs.id,{id:attrs.id,value:attrs.value||'',text:match[3],checked:'checked'in attrs,disabled:false,textContent:'',listeners:{},addEventListener(type,fn){this.listeners[type]=fn;},focus(){},fire(type){return this.listeners[type]?.();}});}},
 closeModal(){if(window.STLineInputs.isBusy)return;window.STLineInputs.closed();state.modal=null;}};
 window.ST=ST;web.runText('line-inputs.js', script, {window,document:{getElementById:id=>nodes.get(id)}});
 return{state,ST,audio,ui:window.STLineInputs,calls,undo,toasts,get:id=>nodes.get(id),async show(){return window.STLineInputs.show();},apply(){return nodes.get('line-input-apply').onclick();}};
}
const mixerRoot=fields=>({querySelector:selector=>fields[/"(\w+)"/.exec(selector)[1]]});
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('mixer: the selected track shows its live input state and edits it in place as one Undo step, without a host command',async()=>{
 const h=harness();h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[]};
 let html=h.ui.mixerSection(0);assert.match(html,/Live input/);assert.match(html,/>Off</);
 h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};
 html=h.ui.mixerSection(0);assert.match(html,/Not running/,'an enabled route the host does not run is not called live');
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];assert.match(h.ui.mixerSection(0),/is-live">Live</);
 const fields={enabled:{checked:true},left:{value:'2'},right:{value:'3'}};
 assert.equal(await h.ui.applyMixer(1,mixerRoot(fields)),true);
 assert.equal(h.calls.length,0,'the project is the routing: nothing is sent to the host but the project itself');
 assert.equal(h.undo.length,1);assert.equal(h.undo[0].label,'Live input');assert.deepEqual(copy(h.state.project.tracks[1].audioInput),{gain:1,enabled:true,deviceUID:'interface',channels:[2,3]});
 fields.right.value='mono';fields.enabled.checked=false;assert.equal(await h.ui.applyMixer(1,mixerRoot(fields)),true);
 assert.deepEqual(copy(h.state.project.tracks[1].audioInput.channels),[2]);assert.equal(h.state.project.tracks[1].audioInput.enabled,false);assert.equal(h.undo.length,2);assert.equal(h.undo[1].label,'Live input off');
 fields.right.value='2';fields.enabled.checked=true;await h.ui.applyMixer(1,mixerRoot(fields));assert.deepEqual(copy(h.state.project.tracks[1].audioInput.channels),[2],'the same channel twice is mono');
});
test('mixer: Monitor is on by default and switching it off is saved as one Undo step',async()=>{
 const h=harness();h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[{track:0,state:'live',active:true}]};
 h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};
 assert.match(h.ui.mixerSection(0),/data-line-input-field="monitor" checked/);
 const fields={enabled:{checked:true},monitor:{checked:false},left:{value:'4'},right:{value:'5'}};
 assert.equal(await h.ui.applyMixer(0,mixerRoot(fields)),true);
 assert.equal(h.state.project.tracks[0].audioInput.monitor,false);assert.equal(h.undo.length,1);assert.equal(h.undo[0].label,'Monitor off');
 assert.match(h.ui.mixerSection(0),/Live · not monitored/);assert.doesNotMatch(h.ui.mixerSection(0),/data-line-input-field="monitor" checked/);
 fields.monitor.checked=true;assert.equal(await h.ui.applyMixer(0,mixerRoot(fields)),true);
 assert.equal('monitor' in h.state.project.tracks[0].audioInput,false,'monitoring on is the default and is not stored');assert.equal(h.undo.length,2);assert.equal(h.undo[1].label,'Monitor on');
});
test('mixer: without an interface nothing is edited; a refused edit (a frozen track) is not reported as applied',async()=>{
 const h=harness();h.state.audio={inputs:[],lineInputs:[]};
 const fields={enabled:{checked:true},left:{value:'0'},right:{value:'mono'}};
 assert.match(h.ui.mixerSection(0),/Choose an audio interface with inputs in Settings/);
 assert.equal(await h.ui.applyMixer(0,mixerRoot(fields)),false);assert.equal(h.undo.length,0);assert.equal(h.toasts[0].kind,'error');
 h.state.audio=copy(h.audio);h.ST.mutate=()=>false;
 assert.equal(await h.ui.applyMixer(0,mixerRoot(fields)),false);assert.equal(h.state.project.tracks[0].audioInput,undefined);
});
test('a route the host pauses is explained once the host has the edit',async()=>{
 const gate=deferred(),h=harness();h.state.audio=copy(h.audio);h.ST.waitForEngineUpdate=()=>gate.promise;
 const fields={enabled:{checked:true},left:{value:'6'},right:{value:'7'}};
 assert.equal(await h.ui.applyMixer(0,mixerRoot(fields)),true);assert.equal(h.toasts.length,0);
 // The publication's audio.devices event arrives before the publication is acknowledged.
 h.state.audio={...h.state.audio,lineInputs:[{track:0,state:'paused',active:false,status:'Eight inputs has 2 input channels: input 7 does not exist.'}]};
 gate.resolve();await settle();
 assert.deepEqual(h.toasts,[{text:'Eight inputs has 2 input channels: input 7 does not exist.',kind:'error'}]);
 assert.match(h.ui.mixerSection(0),/>Paused</);assert.match(h.ui.mixerSection(0),/input 7 does not exist/);
 // A route that runs needs no word.
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];await h.ui.applyMixer(0,mixerRoot(fields));await settle();assert.equal(h.toasts.length,1);
});
test('Recording page: an enabled route reads LIVE only while the host runs it',()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'interface',channels:[4,5],gain:1};h.state.view='recording';
 h.state.audio={inputDevice:22,inputs:[{id:22,uid:'interface',name:'Eight inputs',inputChannels:8}],lineInputs:[],lineInputError:'Microphone access is off'};
 let html=h.ui.render();assert.match(html,/NOT RUNNING/);assert.match(html,/Microphone access is off/);assert.doesNotMatch(html,/>LIVE</);
 h.state.audio.lineInputs=[{track:0,state:'live',active:true}];html=h.ui.render();assert.match(html,/>LIVE</);
 h.state.project.tracks[0].audioInput.monitor=false;assert.match(h.ui.render(),/LIVE · MUTED/);
 h.state.audio.lineInputs=[{track:0,state:'paused',active:false,status:'The track is frozen: its input is closed while it plays its print.'}];html=h.ui.render();assert.match(html,/PAUSED/);assert.match(html,/its input is closed/);
 h.state.project.tracks[0].audioInput.enabled=false;h.state.audio.lineInputs=[{track:0,state:'off',active:false}];assert.match(h.ui.render(),/<small>OFF<\/small>/);
});
test('dialog: two lanes apply their mono and stereo maps as one edit of the project',async()=>{
 const h=harness();await h.show();assert.deepEqual(h.calls.map(c=>c.command),['audio.devices']);
 h.get('line-input-enabled-0').checked=true;h.get('line-input-left-0').value='3';h.get('line-input-enabled-1').checked=true;h.get('line-input-left-1').value='4';h.get('line-input-right-1').value='5';h.get('line-input-gain-1').value='0.5';h.get('line-input-monitor-1').checked=false;
 assert.equal(h.apply(),true);
 assert.equal(h.undo.length,1);assert.equal(h.state.modal,null);
 assert.deepEqual(copy(h.state.project.tracks[0].audioInput),{enabled:true,deviceUID:'interface',channels:[3],gain:1});
 assert.deepEqual(copy(h.state.project.tracks[1].audioInput),{enabled:true,deviceUID:'interface',channels:[4,5],gain:.5,monitor:false});
 assert.deepEqual(h.calls.map(c=>c.command),['audio.devices'],'no command routes inputs');
 assert.equal(h.toasts.at(-1).text,'Line inputs updated. Record a sample to keep the incoming audio.');
});
test('dialog: invalid stereo maps and an emptied gain are refused before anything is edited',async()=>{
 const h=harness();await h.show();h.get('line-input-enabled-0').checked=true;h.get('line-input-right-0').value='0';
 assert.equal(h.apply(),false);assert.equal(h.undo.length,0);assert.match(h.get('line-input-status').textContent,/distinct/);assert.equal(h.state.modal,'line-inputs');
 h.get('line-input-right-0').value='mono';h.get('line-input-gain-0').value='';
 assert.equal(h.apply(),false);assert.equal(h.undo.length,0,'an emptied gain field does not silence the input with gain 0');assert.match(h.get('line-input-status').textContent,/Input gain must be between 0 and 2/);
 h.get('line-input-gain-0').value='0.5';assert.equal(h.apply(),true);
 assert.equal(h.state.project.tracks[0].audioInput.gain,.5);assert.equal(h.undo.length,1);
});
test('dialog: saved routes of an interface that is not connected keep their map; edited ones need relinking',async()=>{
 const reason='Missing interface is not the audio input in use. Its input is paused until it is.';
 const h=harness(),saved={enabled:true,deviceUID:'missing',channels:[12,13],gain:.8},off={enabled:false,deviceUID:'missing',channels:[3],gain:1};
 h.audio.lineInputs=[{track:0,state:'paused',active:false,status:reason},{track:1,state:'off',active:false}];h.audio.lineInputError=reason;
 h.state.project.tracks[0].audioInput=copy(saved);h.state.project.tracks[1].audioInput=copy(off);h.state.project.tracks.push({name:'Third'});
 await h.show();
 assert.equal(h.get('line-input-device-0').value,'missing');assert.equal(h.get('line-input-note-0').text,reason);assert.equal(h.get('line-input-status').text,reason,'the dialog opens on the host\'s reason');
 assert.match(h.ui.render(),/PAUSED/);assert.match(h.ui.render(),/Interface unavailable/);
 // Another lane changes: the paused and the switched-off route stay exactly as they were saved.
 h.get('line-input-enabled-2').checked=true;h.get('line-input-left-2').value='7';
 assert.equal(h.apply(),true);
 assert.deepEqual(copy(h.state.project.tracks[0].audioInput),saved);assert.deepEqual(copy(h.state.project.tracks[1].audioInput),off);
 assert.deepEqual(copy(h.state.project.tracks[2].audioInput),{enabled:true,deviceUID:'interface',channels:[7],gain:1});assert.equal(h.undo.length,1);
 // Editing the paused lane itself needs the current interface.
 await h.show();h.get('line-input-gain-0').value='0.5';
 assert.equal(h.apply(),false);assert.match(h.get('line-input-status').textContent,/current Studio interface/);assert.equal(h.undo.length,1);
 h.get('line-input-device-0').value='interface';h.get('line-input-device-0').fire('change');
 assert.equal(h.apply(),true);assert.equal(h.state.project.tracks[0].audioInput.deviceUID,'interface');assert.deepEqual(copy(h.state.project.tracks[0].audioInput.channels),[0]);
 // Switching a route of a missing interface off keeps what it names, for the day the interface is back.
 h.state.project.tracks[0].audioInput=copy(saved);await h.show();h.get('line-input-enabled-0').checked=false;
 assert.equal(h.apply(),true);assert.deepEqual(copy(h.state.project.tracks[0].audioInput),{...saved,enabled:false});
});
test('dialog: unchanged configurations close without an edit',async()=>{
 const h=harness();h.state.project.tracks[0].audioInput={enabled:false,deviceUID:'missing',channels:[3],gain:1};h.state.project.tracks[1].audioInput={enabled:true,deviceUID:'interface',channels:[3],gain:1};
 await h.show();assert.equal(h.apply(),true);assert.equal(h.undo.length,0);assert.equal(h.state.modal,null);assert.equal(h.toasts.length,0);
});
test('dialog: a refused edit and a project that changed meanwhile leave everything as it is',async()=>{
 const h=harness();await h.show();h.get('line-input-enabled-0').checked=true;h.ST.mutate=()=>false;
 assert.equal(h.apply(),false);assert.equal(h.state.project.tracks[0].audioInput,undefined);assert.equal(h.state.modal,'line-inputs');assert.match(h.get('line-input-status').textContent,/not applied/);assert.equal(h.toasts.length,0);
 const stale=harness();await stale.show();stale.get('line-input-enabled-0').checked=true;
 stale.state.project={tracks:[{name:'New project'}]};
 assert.equal(stale.apply(),false);assert.equal(stale.undo.length,0);assert.equal(stale.state.project.tracks[0].audioInput,undefined);assert.match(stale.get('line-input-status').textContent,/The project changed/);
 // A track list that changed under the dialog is not written to by index.
 const moved=harness();await moved.show();moved.get('line-input-enabled-0').checked=true;moved.state.project.tracks.splice(0,1);
 assert.equal(moved.apply(),false);assert.equal(moved.state.project.tracks[0].name,'Second');assert.equal(moved.state.project.tracks[0].audioInput,undefined,'the shifted track keeps no input');
});
test('dialog: reading the devices cannot replace a newer modal, and a refused modal open remains clean',async()=>{
 const gate=deferred(),h=harness({'audio.devices':()=>gate.promise});const pending=h.show();assert.equal(h.ui.isBusy,true);assert.equal(await h.show(),false,'one dialog at a time');
 h.state.modal='new-dialog';gate.resolve({inputs:[],lineInputs:[]});assert.equal(await pending,false);assert.equal(h.state.modal,'new-dialog');assert.equal(h.ui.isBusy,false);
 const refused=harness();refused.ST.openModal=()=>false;assert.equal(await refused.show(),false);assert.equal(refused.ui.isBusy,false);
 const failing=harness({'audio.devices':()=>{throw new Error('No audio host');}});assert.equal(await failing.show(),false);assert.equal(failing.toasts[0].kind,'error');assert.equal(failing.ui.isBusy,false);
});

(async()=>{for(const {name,fn}of tests){await fn();console.log('PASS '+name);}console.log(`Line inputs UI: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
