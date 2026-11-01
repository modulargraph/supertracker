'use strict';
// Line-input routes saved against a physical interface stay usable through an aggregate
// and stay saved (paused, clearly labelled) while that interface is disconnected.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const web = require('./helpers/web');
const script = web.source('line-inputs.js'),copy=x=>JSON.parse(JSON.stringify(x));
const AudioBox='AppleUSBAudioEngine:Acme:AudioBox10:00000001:1,2';
function harness(audio){
 const nodes=new Map(),calls=[],undo=[];
 const state={project:{tracks:[{name:'Synth A',audioInput:{enabled:true,deviceUID:AudioBox,channels:[4,5],gain:1}},{name:'Synth B',audioInput:{enabled:true,deviceUID:AudioBox,channels:[3],gain:1}},{name:'Lead'}]},audio:copy(audio),modal:null};
 const window={},ST={state,escapeHTML:String,refresh(){},toast(){},captureNativeEdits:async()=>true,
  request:async(command,payload)=>{calls.push({command,payload:copy(payload||{})});return copy(audio);},
  mutate(label,fn){undo.push(copy(state.project));fn(state.project);return true;},
  openModal(title,body,footer,options){state.modal=options.kind;for(const match of (body+footer).matchAll(/<([a-z]+)\b([^>]*)>([^<]*)/gi)){const attrs={};for(const a of match[2].matchAll(/([\w-]+)(?:="([^"]*)")?/g))attrs[a[1]]=a[2]||'';if(!attrs.id)continue;nodes.set(attrs.id,{id:attrs.id,value:attrs.value||'',text:match[3],checked:'checked'in attrs,disabled:false,textContent:'',addEventListener(){},focus(){}});}},
  closeModal(){window.STLineInputs.closed();state.modal=null;}};
 window.ST=ST;web.runText('line-inputs.js', script, {window,document:{getElementById:id=>nodes.get(id)}});
 return{state,calls,undo,ui:window.STLineInputs,get:id=>nodes.get(id),apply:()=>nodes.get('line-input-apply').onclick()};
}
const aggregate={id:48,uid:'~:AMS2_Aggregate:0',name:'Tracker + AudioBox',inputChannels:38};
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('routes paused while the AudioBox is absent are labelled and kept; unchanged paused lanes pass Apply, edited ones need relinking',async()=>{
 const reason='AudioBox10 not connected — returns paused';
 const h=harness({inputDevice:48,inputs:[{...aggregate,inputChannels:0}],lineInputs:[{track:0,active:false,state:'paused',status:reason},{track:1,active:false,state:'paused',status:reason}]});
 await h.ui.show();
 assert.equal(h.get('line-input-note-0').text,reason);
 assert.match(h.ui.render(),/PAUSED/);assert.match(h.ui.render(),/not connected — returns paused/);
 // Unchanged paused lanes are not re-sent; the edited lane is the only change.
 h.get('line-input-gain-1').value='0.5';h.get('line-input-enabled-1').checked=true;
 await h.apply();
 const sent=h.calls.find(c=>c.command==='audio.inputs');
 assert(!sent,'An edited paused lane on an absent interface must still require relinking');
 h.get('line-input-gain-1').value='1';await h.apply();
 assert.equal(h.calls.filter(c=>c.command==='audio.inputs').length,0);assert.equal(h.undo.length,0);assert.equal(h.state.modal,null);
 assert.deepEqual(copy(h.state.project.tracks[0].audioInput.channels),[4,5]);
});
test('routes live through an aggregate show it and apply without relinking',async()=>{
 const h=harness({inputDevice:48,inputs:[aggregate,{id:60,uid:AudioBox,name:'AudioBox10',inputChannels:10}],lineInputs:[{track:0,active:true,state:'live',via:'Tracker + AudioBox',engineChannels:[4,5]},{track:1,active:true,state:'live',via:'Tracker + AudioBox',engineChannels:[3]}]});
 await h.ui.show();
 assert.equal(h.get('line-input-note-0').text,'Live through Tracker + AudioBox');
 h.get('line-input-enabled-2').checked=true;h.get('line-input-left-2').value='12';
 await h.apply();
 const sent=h.calls.find(c=>c.command==='audio.inputs');
 assert.deepEqual(sent.payload.inputs.map(i=>i.track),[2]);
 assert.equal(h.state.project.tracks[2].audioInput.deviceUID,aggregate.uid);
 assert.equal(h.state.project.tracks[0].audioInput.deviceUID,AudioBox);
});
(async()=>{for(const {name,fn}of tests){await fn();console.log('PASS '+name);}console.log(`Line inputs aggregate UI: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
