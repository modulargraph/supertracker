'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web = require('./helpers/web');
const copy=value=>JSON.parse(JSON.stringify(value));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
function harness(overrides={}){
 const calls=[],changes=[],toasts=[],nodes=new Map();let counter=0,ui;
 const state={project:{bpm:120,tracks:[{name:'Audio'}],audioClips:[{id:'one',track:0,name:'Phrase',path:'/original.wav',sourceDuration:8,startBeat:4,durationBeats:4,sourceOffset:1,fadeIn:0,fadeOut:0}]},clipSelected:'one',view:'song',modal:null};
 const document={activeElement:null,getElementById:id=>nodes.get(id)||null};
 function node(id,attrs={}){const n={id,value:'',textContent:'',innerHTML:'',disabled:false,...attrs,focus(){document.activeElement=this;},click(){if(!this.disabled)return this.onclick?.({target:this});}};nodes.set(id,n);return n;}
 node('clip-one');node('clips-timeline');
 const ST={state,uid:()=>`id-${++counter}`,escapeHTML:s=>String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/"/g,'&quot;'),toast:(message,kind)=>toasts.push({message,kind}),refresh(){},mutate(label,fn){changes.push({label,before:copy(state.project)});fn(state.project);},request(command,payload={}){calls.push({command,payload:copy(payload)});try{return Promise.resolve(overrides[command]?overrides[command](payload):command==='audio.clipInfo'?{duration:8,name:payload.path.split('/').pop()}:command==='dialog.openSample'?{path:'/alternate.wav'}:{});}catch(error){return Promise.reject(error);}},openModal(title,body,footer,options){if(state.modal==='clip-takes')ui.closed();state.modal=options.kind;for(const match of (body+footer).matchAll(/<([a-z][a-z0-9]*)\b([^>]*)>/gi)){const id=/\bid="([^"]+)"/.exec(match[2])?.[1];if(id)node(id,{tagName:match[1].toUpperCase(),value:/\bvalue="([^"]*)"/.exec(match[2])?.[1]||''});}},closeModal(){if(state.modal==='clip-takes')ui.closed();state.modal=null;}};
 const context=vm.createContext({window:{ST},document});for(const name of ['clip-takes-project.js','clip-takes.js'])web.load(context, name);
 ui=context.window.STClipTakes;const M=context.window.STClipTakesProject;
 const get=id=>{assert(nodes.has(id),'missing '+id);return nodes.get(id);};
 const select=id=>{get('takes-list').value=id;get('takes-list').onchange();};
 return {ui,M,ST,state,context,calls,changes,toasts,document,get,select,clip:()=>state.project.audioClips[0],add:async()=>{await ui.addFile('/alternate.wav',ui.target(),'Second take');}};
}
const tests=[],test=(name,fn)=>tests.push([name,fn]);
test('a frozen-source rollback cannot acknowledge promotion of a detached take object',async()=>{
 const h=harness(),before=copy(h.state.project);h.ST.mutate=(label,fn)=>{fn(h.state.project);h.state.project=copy(before);};
 await assert.rejects(h.ui.addFile('/alternate.wav',h.ui.target()),/not applied/);
 assert.deepEqual(copy(h.state.project),before);assert.equal(h.changes.length,0);
});
test('import validates both source lengths before a single immutable-history edit and preserves all alternatives',async()=>{
 const h=harness();await h.ui.importFile();assert.equal(h.clip().takeSet.takes.length,2);assert.equal(h.clip().path,'/alternate.wav');assert.equal(h.changes.length,1);assert.equal(h.changes[0].before.audioClips[0].path,'/original.wav');assert(!h.changes[0].before.audioClips[0].takeSet);
 assert.deepEqual(h.calls.map(c=>c.command),['dialog.openSample','audio.clipInfo','audio.clipInfo']);assert.equal(h.document.activeElement.id,'clip-one');assert.equal(h.ui.isBusy,false);
});
test('pending import is single-flight and changed destinations preserve source without applying it',async()=>{
 for(const change of ['edit','remove','project']){
  const pending=deferred(),h=harness({'dialog.openSample':()=>pending.promise}),before=copy(h.state.project),task=h.ui.importFile();assert(h.ui.isBusy);await h.ui.importFile();
  if(change==='edit')h.clip().durationBeats=3;else if(change==='remove')h.state.project.audioClips=[];else h.state.project=copy(before);
  pending.resolve({path:'/alternate.wav'});await task;assert.equal(h.changes.length,0);assert.equal(h.calls.length,1);assert.match(h.toasts.at(-1).message,/changed/);assert(!h.ui.isBusy);
 }
});
test('source discovery failures and unknown or short lengths never replace the active take',async()=>{
 for(const duration of [0,NaN,Infinity,.25]){
  const h=harness({'audio.clipInfo':p=>({duration:p.path==='/original.wav'?8:duration})}),before=copy(h.clip());await h.ui.importFile();assert.deepEqual(copy(h.clip()),before);assert.equal(h.changes.length,0);assert(h.toasts.some(t=>t.kind==='error'));
 }
 const h=harness({'audio.clipInfo':p=>({duration:p.path==='/original.wav'?NaN:8})}),before=copy(h.clip());await h.ui.importFile();assert.deepEqual(copy(h.clip()),before);assert.match(h.toasts.at(-1).message,/current take/);
});
test('manager search and selection stay clean, rename and per-take alignment are explicit edits',async()=>{
 const h=harness();await h.add();h.changes.length=0;h.ui.show();assert.equal(h.document.activeElement.id,'takes-list');assert(h.get('takes-remove').disabled);
 const original=h.clip().takeSet.takes[0].id;h.get('takes-search').value='original';h.get('takes-search').oninput();assert.equal(h.get('takes-list').value,original);assert.equal(h.changes.length,0);assert(!h.get('takes-remove').disabled);
 h.get('takes-name').value='First performance';h.get('takes-rename').click();assert.equal(h.clip().takeSet.takes[0].name,'First performance');
 h.get('takes-offset').value='2';h.get('takes-align').click();assert.equal(h.clip().takeSet.takes[0].sourceOffset,2);assert.equal(h.clip().sourceOffset,0);
 h.get('takes-use').click();assert.equal(h.clip().path,'/original.wav');assert.equal(h.clip().sourceOffset,2);assert.equal(h.state.modal,null);assert.equal(h.changes.length,3);
});
test('manager rejects unusable alignment, empty name and active deletion without extra edits',async()=>{
 const h=harness();await h.add();h.ui.show();const count=h.changes.length,before=copy(h.clip());
 h.get('takes-offset').value='7';h.get('takes-align').click();assert.match(h.get('takes-coverage').textContent,/too little/);assert.deepEqual(copy(h.clip()),before);
 h.get('takes-offset').value='';h.get('takes-align').click();assert.match(h.get('takes-coverage').textContent,/valid/);
 h.get('takes-name').value=' ';h.get('takes-rename').click();assert.match(h.get('takes-coverage').textContent,/name/);h.get('takes-remove').click();assert.equal(h.changes.length,count);
});
test('manager can remove an inactive alternative and no-results filter cannot edit anything',async()=>{
 const h=harness();await h.add();h.ui.show();h.select(h.clip().takeSet.takes[0].id);h.get('takes-remove').click();assert.equal(h.clip().takeSet.takes.length,1);assert.equal(h.clip().path,'/alternate.wav');assert(h.get('takes-remove').disabled);
 h.get('takes-search').value='zz-no-match';h.get('takes-search').oninput();assert(h.get('takes-use').disabled);assert(h.get('takes-rename').disabled);assert(h.get('takes-preview').disabled);assert.equal(h.get('takes-list').value,'');
});
test('close cancels an in-flight preview once and a late acknowledgment cannot restart it or alter another dialog',async()=>{
 const pending=deferred(),h=harness({'sample.preview':()=>pending.promise});await h.add();h.ui.show();h.calls.length=0;h.get('takes-preview').click();h.get('takes-close').click();assert.equal(h.state.modal,null);
 assert.deepEqual(h.calls.map(c=>c.command),['sample.preview','sample.stopPreview']);h.ST.openModal('Other','<p id="other-status">Other</p>','',{kind:'other'});pending.resolve({playing:true});await Promise.resolve();await Promise.resolve();
 assert.equal(h.state.modal,'other');assert.deepEqual(h.calls.map(c=>c.command),['sample.preview','sample.stopPreview']);
});
test('changing take or replacing a manager cancels its preview without stopping a newer one on stale completion',async()=>{
 const pending=deferred(),h=harness({'sample.preview':()=>pending.promise});await h.add();h.ui.show();h.calls.length=0;h.get('takes-preview').click();h.select(h.clip().takeSet.takes[0].id);h.get('takes-preview').click();
 assert.deepEqual(h.calls.map(c=>c.command),['sample.preview','sample.stopPreview','sample.preview']);h.ui.show();assert.equal(h.calls.at(-1).command,'sample.stopPreview');
 pending.resolve({playing:true});await Promise.resolve();await Promise.resolve();assert.equal(h.calls.filter(c=>c.command==='sample.stopPreview').length,2);assert.equal(h.state.modal,'clip-takes');
});
test('stale project or clip identity cannot accept manager edits and malformed take metadata is reported',async()=>{
 const h=harness();await h.add();h.ui.show();const count=h.changes.length;h.state.project=copy(h.state.project);h.get('takes-name').value='Wrong';h.get('takes-rename').click();assert.equal(h.changes.length,count);
 h.clip().takeSet.activeID='missing';h.ui.show();assert.match(h.toasts.at(-1).message,/missing/);
});
test('refused mutate and a routing change during metadata lookup cannot report a take as added',async()=>{
 const h=harness();h.ST.mutate=()=>{};await h.ui.importFile();assert(!h.clip().takeSet);assert.match(h.toasts.at(-1).message,/not applied/);assert(!h.toasts.some(t=>/Alternative added/.test(t.message)));
 const pending=deferred(),other=harness({'audio.clipInfo':()=>pending.promise}),task=other.ui.importFile();await Promise.resolve();await Promise.resolve();other.context.window.STRouting={isApplying:true};pending.resolve({duration:8});await task;assert(!other.clip().takeSet);assert.equal(other.changes.length,0);assert.match(other.toasts.at(-1).message,/project operation/);
});
test('tempo helper is used for take coverage and invalid maps are not silently interpreted as fixed tempo',async()=>{
 const h=harness();h.context.window.STProjectTiming={durationFromBeats:()=>9};await h.ui.importFile();assert(!h.clip().takeSet);assert.match(h.toasts.at(-1).message,/9.00/);
 h.context.window.STProjectTiming.durationFromBeats=()=>{throw Error('Timing rejected');};await h.ui.importFile();assert(!h.clip().takeSet);assert.match(h.toasts.at(-1).message,/Timing rejected/);
});
(async()=>{for(const [name,fn] of tests){await fn();console.log('PASS '+name);}console.log(`Clip takes UI: ${tests.length} regression groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
