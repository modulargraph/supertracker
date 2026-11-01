'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const web=require('./helpers/web');
// Execute the actual dialog function; emulate only the browser's form defaults.
const clone=value=>JSON.parse(JSON.stringify(value));
// Matches AudioHost.audioDevices and MIDIHost.deviceSnapshot, including signed
// Core MIDI unique IDs. Snapshot IDs are connected endpoints, not saved requests.
const audio={inputs:[{id:11,name:'Built-in input',inputChannels:1},{id:22,name:'Interface input',inputChannels:4}],outputs:[{id:33,name:'Built-in output'},{id:44,name:'Interface output'}],defaultInput:11,defaultOutput:33,inputDevice:22,outputDevice:44,inputChannels:[2,3],bufferSizes:[128,256,512],bufferSize:256,monitorVolume:0,monitoring:false,sampleRate:48000};
const midi={inputs:[{id:-2147483647,name:'Synth keys',connected:true},{id:42,name:'Pads',connected:false}],outputs:[{id:-7654,name:'Synth',selected:true},{id:77,name:'Drums',selected:false}],inputIDs:[-2147483647],allInputs:false,outputID:-7654,enabled:true,inputChannel:0,channel:3};
function options(html){return [...html.matchAll(/<option\b([^>]*)>(.*?)<\/option>/gs)].map(([,attrs,label])=>({value:attrs.match(/value="([^"]*)"/)?.[1]||'',selected:/\bselected\b/.test(attrs),disabled:/\bdisabled\b/.test(attrs),label}));}
async function harness(overrides={}){
 const nodes=new Map(),calls=[],toasts=[];let modal,closed=0,refreshed=0;
 function node(id){if(!nodes.has(id)){let content='';const el={id,value:'',checked:false,disabled:false,focus(){}};Object.defineProperty(el,'innerHTML',{get:()=>content,set(html){content=html;el.options=options(html);el.value=(el.options.filter(o=>o.selected).at(-1)||el.options.find(o=>!o.disabled))?.value||'';}});nodes.set(id,el);}return nodes.get(id);}
 const state={native:true,audio:{},midi:{},status:{},track:2,instrument:1};
 const a={...clone(audio),...overrides.audio},m={...clone(midi),...overrides.midi};
 const context={window:{},Promise,Number,String,Object,Array,Error},stubs={state,modalIntent:0,settingsApplyPending:null,recordArmSession:0,sessionTransition:false,esc:String,latencyNote:()=>'',refresh(){refreshed++;},toast:(message,type)=>toasts.push({message,type}),closeModal(){closed++;nodes.clear();},$:selector=>nodes.get(selector.slice(1)),async request(command,payload){calls.push({command,payload:payload===undefined?undefined:clone(payload)});if(command==='audio.devices')return a;if(command==='midi.devices')return m;await overrides.wait?.(command,payload);if(overrides.fail?.(command))throw Error('Saved MIDI port is disconnected');return command==='audio.configure'?a:m;},openModal(title,html,footer,config){modal={title,html,config};for(const [,id,body] of html.matchAll(/<select\b[^>]*id="([^"]+)"[^>]*>(.*?)<\/select>/gs))node(id).innerHTML=body;for(const [,attrs,id] of html.matchAll(/<input\b([^>]*\bid="([^"]+)"[^>]*)>/g)){node(id).value=attrs.match(/value="([^"]*)"/)?.[1]||'';node(id).checked=/\bchecked\b/.test(attrs);}for(const [,id] of footer.matchAll(/id="([^"]+)"/g))node(id);}};
 vm.createContext(context);const app=web.loadAppWith(context,stubs);await app.showSettings();
 return {node,calls,toasts,modal,state,get closed(){return closed;},get refreshed(){return refreshed;},dismiss(){nodes.clear();},set(id,value,change=false){node(id)[typeof value==='boolean'?'checked':'value']=value; if(change)node(id).onchange?.();},apply:()=>node('settings-apply').onclick(),writes:()=>calls.filter(x=>x.command.endsWith('.configure'))};
}
(async()=>{
 let h=await harness();assert.equal(h.node('midi-input').value,'-2147483647');assert.equal(h.node('midi-output').value,'-7654');assert.deepEqual(clone(h.modal.config),{kind:'settings',className:'studio-settings-modal'});await h.apply();assert.deepEqual(h.writes(),[]);assert.equal(h.closed,1);
 console.log('PASS native signed IDs selected; untouched Apply performs no hardware configuration');
 h=await harness();h.set('audio-buffer','512');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{bufferSize:512}}]);
 h=await harness();h.set('audio-output','33');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{outputDevice:33}}]);
 console.log('PASS buffer/output changes omit MIDI and input authorization fields');
 h=await harness();h.set('midi-output-channel','8');await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{channel:8}}]);
 h=await harness();h.set('midi-enabled',false);await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{enabled:false}}]);
 console.log('PASS MIDI-only changes omit audio configuration and unchanged ports');
 h=await harness({midi:{inputIDs:[-2147483647,42],allInputs:false}});assert.equal(h.node('midi-input').value,'keep');assert.match(h.modal.html,/Keep current inputs · 2 connected/);h.set('midi-input-channel','5');await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{inputChannel:5}}]);
 h=await harness({midi:{inputIDs:[],allInputs:false,outputID:null}});assert.equal(h.node('midi-input').value,'keep');assert.equal(h.node('midi-output').value,'keep');assert.match(h.modal.html,/none connected/);h.set('audio-buffer','128');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{bufferSize:128}}]);
 console.log('PASS multiple inputs and disconnected saved requests survive unrelated Apply');
 h=await harness({midi:{inputIDs:[],allInputs:false,outputID:null}});h.set('midi-input','none');h.set('midi-output','none');await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{inputID:'none',outputID:'none'}}]);
 h=await harness({midi:{inputIDs:[],allInputs:false}});h.set('midi-input','all');await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{inputID:'all'}}]);
 h=await harness({midi:{inputIDs:[-2147483647,42],allInputs:true,outputID:null}});assert.equal(h.node('midi-input').value,'all');h.set('midi-input','-2147483647');h.set('midi-output','-7654');await h.apply();assert.deepEqual(h.writes(),[{command:'midi.configure',payload:{inputID:-2147483647,outputID:-7654}}]);
 console.log('PASS deliberate none/all/single changes follow native configure schema without losing signed IDs');
 h=await harness();h.set('audio-output','');h.set('audio-input','',true);assert.equal(h.node('input-channel-right').value,'mono');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{outputDevice:33,outputSystemDefault:true,inputDevice:11,inputSystemDefault:true,inputChannels:[0]}}]);
 h=await harness();h.set('input-channel-right','mono');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{inputDevice:22,inputChannels:[2]}}]);
 h=await harness({audio:{inputChannels:[2]}});assert.equal(h.node('input-channel-right').value,'mono');await h.apply();assert.deepEqual(h.writes(),[]);
 console.log('PASS system default resolves a real device; deliberate input map changes retain mono/stereo semantics');
 h=await harness();h.set('monitor-volume','0.65');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{monitor:false,monitorVolume:.65}}]);
 h=await harness();h.set('audio-monitor',true);h.set('monitor-volume','0.65');await h.apply();assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{monitor:true,monitorVolume:.65}}]);
 console.log('PASS monitor volume includes required monitor state without implicit input-device changes');
 let fail=true;h=await harness({fail:command=>command==='midi.configure'&&fail});h.set('audio-buffer','128');h.set('midi-output-channel','9');await h.apply();assert.equal(h.closed,0);assert.equal(h.toasts.at(-1).type,'error');fail=false;await h.apply();assert.equal(h.writes().filter(x=>x.command==='audio.configure').length,1);assert.equal(h.closed,1);
 console.log('PASS native offline routing errors stay visible; retry does not reconfigure already-applied audio');
 const deferred=()=>{let resolve;const promise=new Promise(done=>resolve=done);return {promise,resolve};};
 let gate=deferred();h=await harness({wait:command=>command==='audio.configure'?gate.promise:undefined});h.set('audio-buffer','128');h.set('midi-output-channel','9');
 let pending=h.apply();assert.equal(h.node('settings-cancel').disabled,true);assert.equal(h.node('midi-output-channel').disabled,true);assert.equal(h.writes().length,1);
 // Escape/replacement removes the old controls while a native operation is pending.
 h.dismiss();const replacement=h.node('settings-apply');replacement.disabled=true;h.node('midi-output-channel').value='14';gate.resolve();await pending;
 assert.deepEqual(h.writes(),[{command:'audio.configure',payload:{bufferSize:128}},{command:'midi.configure',payload:{channel:9}}]);assert.equal(h.closed,0);assert.equal(h.refreshed,0);assert.equal(replacement.disabled,true);assert.equal(h.toasts.length,0);
 console.log('PASS delayed Apply finishes captured commands without reading, closing or enabling a replacement dialog');
 gate=deferred();let firstAudio=true;fail=true;h=await harness({wait:command=>{if(command==='audio.configure'&&firstAudio){firstAudio=false;return gate.promise;}},fail:command=>command==='midi.configure'&&fail});h.set('audio-buffer','128');h.set('midi-output-channel','9');pending=h.apply();
 // Even a programmatic draft update cannot be mistaken for an acknowledged value.
 h.set('audio-buffer','512');gate.resolve();await pending;assert.equal(h.closed,0);assert.equal(h.node('audio-buffer').disabled,false);assert.equal(h.node('settings-cancel').disabled,false);fail=false;await h.apply();
 assert.deepEqual(h.writes().filter(x=>x.command==='audio.configure').map(x=>x.payload),[{bufferSize:128},{bufferSize:512}]);assert.equal(h.closed,1);
 console.log('PASS partial-failure retry compares the new draft against submitted, acknowledged values');

})().catch(error=>{console.error(error);process.exitCode=1;});
