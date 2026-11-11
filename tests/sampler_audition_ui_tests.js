'use strict';
// Production sampler module with an event/focus DOM and deferred native bridge.
// No audio device, vendor plug-in, app process or sample file is opened.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const web = require('./helpers/web');
const source = web.source('sampler.js');
const clone=value=>JSON.parse(JSON.stringify(value));
const deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b;});return{promise,resolve,reject};};
const flush=async()=>{for(let i=0;i<16;i++)await Promise.resolve();};
function harness(count=4){
 const nodes=new Map(),elements=[],requests=[],mutations=[],notices=[],timers=new Map();let html='',nextTimer=0,gate=Promise.resolve(),cancelSyncs=0,syncCalls=0,pendingEdits=async()=>{};
 const handlers=new Map();
 const document={activeElement:null,getElementById:id=>nodes.get(id)||null,querySelectorAll(selector){const attr=selector.slice(1,-1);return elements.filter(el=>Object.hasOwn(el.attrs,attr));}};
 class Element{
  constructor(tagName,attrs='',parent=null){
   this.tagName=tagName.toUpperCase();this.parent=parent;this.attrs={};this.dataset={};this.listeners=new Map();this.isConnected=true;this.textContent='';this.value='';this.disabled=/\sdisabled(?:\s|$)/.test(attrs);this.hidden=/\shidden(?:\s|$)/.test(attrs);this.tabIndex=0;
   for(const match of attrs.matchAll(/([\w-]+)="([^"]*)"/g)){this.attrs[match[1]]=match[2];if(match[1]==='value')this.value=match[2];if(match[1]==='tabindex')this.tabIndex=Number(match[2]);if(match[1].startsWith('data-'))this.dataset[match[1].slice(5).replace(/-([a-z])/g,(_,c)=>c.toUpperCase())]=match[2];}
   this.id=this.attrs.id||`anonymous-${elements.length}`;const classes=new Set((this.attrs.class||'').split(' '));this.classList={toggle(name,on){if(on)classes.add(name);else classes.delete(name);},contains(name){return classes.has(name);}};
  }
  addEventListener(type,fn){if(!this.listeners.has(type))this.listeners.set(type,[]);this.listeners.get(type).push(fn);}
  fire(type,event={}){const e={target:this,...event};return Promise.all((this.listeners.get(type)||[]).map(fn=>fn(e)));}
  focus(){if(this.disabled||!this.isConnected)return;const old=document.activeElement;if(old===this)return;document.activeElement=this;if(old){void old.fire('blur',{relatedTarget:this});for(let node=old;node;node=node.parent)void node.fire('focusout',{target:old,relatedTarget:this});}void this.fire('focus');}
  contains(node){for(;node;node=node.parent)if(node===this)return true;return false;}
  scrollIntoView(){this.scrolled=true;}
  setAttribute(key,value){this.attrs[key]=String(value);}
 }
 const points=Array.from({length:count},(_,i)=>i/count);
 const instrument={id:1,type:'sample',samplePath:'/fixture/break.wav',rootNote:72,start:.1,end:.9,sliceCount:count,slicePoints:points,mode:'oneshot'};
 const state={view:'instrument',modal:null,track:0,instrument:1,recording:true,playing:true,row:13,column:2,selection:{row:4,track:0},project:{bpm:174,instruments:[{id:0,type:'synth'},instrument],tracks:[{instrument:0,effects:[]}],patterns:[{steps:[[{note:50}]]}],audioPlugins:[]}};
 const ST={state,escapeHTML:value=>String(value).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c])),
  mutate(label,fn){mutations.push({label,before:clone(state.project)});fn(state.project);mount();},
  request(command,payload){requests.push({command,payload:clone(payload)});try{return Promise.resolve(handlers.has(command)?handlers.get(command)(payload):command==='step.preview'?{auditioning:payload.step.note>=0}:{ok:true});}catch(e){return Promise.reject(e);}},
  async syncEngineProject(makeProject){syncCalls++;const owner=state.project;await gate;await pendingEdits();if(state.project!==owner)return false;const project=makeProject();if(project==null)return false;return ST.request('engine.project',{project });},
  waitForGraphEdit(){return gate;},cancelPendingEngineSync(){cancelSyncs++;},toast(message,type){notices.push({message,type});},showRecording(){},refresh(){mount();}};
 const window={ST};web.runText('sampler.js', source, {window,document,queueMicrotask,setTimeout(fn,ms){timers.set(++nextTimer,{fn,ms});return nextTimer;},clearTimeout(id){timers.delete(id);}},{filename:'sampler.js'});
 const module=window.STSampler;
 function mount(){
  const focused=document.activeElement?.id;elements.forEach(el=>el.isConnected=false);nodes.clear();elements.length=0;document.activeElement=null;html=module.render(state.project.instruments[state.instrument]);const stack=[];
  for(const match of html.matchAll(/<(\/)?([a-z][\w-]*)\b([^>]*)>/gi)){
   const closing=!!match[1],tag=match[2].toLowerCase();if(closing){while(stack.length&&stack.pop().tagName!==tag.toUpperCase()){}continue;}
   const el=new Element(tag,match[3],stack.at(-1));elements.push(el);nodes.set(el.id,el);if(!['input','br','hr','img'].includes(tag))stack.push(el);
  }
  module.bind();if(focused)nodes.get(focused)?.focus();
 }
 mount();
 const outside=new Element('button',' id="outside"');
 function key(id,key,extra={}){const target=nodes.get(id);assert(target,`Missing ${id}`);target.focus();const event={key,target,preventDefault(){this.defaultPrevented=true;},stopPropagation(){this.propagationStopped=true;},...extra};event.handled=module.key(event);return event;}
 return{module,ST,state,instrument,nodes,elements,requests,mutations,notices,timers,document,outside,mount,key,
  get html(){return html;},get cancelSyncs(){return cancelSyncs;},get syncCalls(){return syncCalls;},pendingEdits(fn){pendingEdits=fn;},gate(value){gate=value.promise;},handle(command,fn){handlers.set(command,fn);},
  async click(id){const target=nodes.get(id);assert(target,`Missing ${id}`);target.focus();return target.fire('click');},
  previews(){return requests.filter(x=>x.command==='step.preview'&&x.payload.step.note>=0);},stops(){return requests.filter(x=>x.command==='step.preview'&&x.payload.step.note<0);}};
}
const tests=[];const test=(name,fn)=>tests.push({name,fn});
test('128 slices use one audition tab stop and distinct selection/focus; numeric arrows remain native',async()=>{
 const h=harness(128);assert.equal(h.document.querySelectorAll('[data-sampler-audition]').filter(el=>el.tabIndex===0).length,1);
 assert(h.key('sampler-audition-0','End').handled);assert.equal(h.document.activeElement.id,'sampler-audition-127');assert(h.nodes.get('sampler-row-127').classList.contains('is-selected'));
 assert.equal(h.nodes.get('sampler-marker-127').tabIndex,0);assert.equal(h.nodes.get('sampler-marker-1').tabIndex,-1);
 assert.equal(h.document.querySelectorAll('[data-sampler-audition]').filter(el=>el.tabIndex===0).length,1);assert.equal(h.requests.length,0);
 assert(h.key('sampler-audition-127','ArrowUp').handled);assert.equal(h.document.activeElement.id,'sampler-audition-126');
 const before=h.nodes.get('sampler-marker-126').value;assert(!h.key('sampler-marker-126','ArrowDown').handled);assert.equal(h.nodes.get('sampler-marker-126').value,before);assert.equal(h.mutations.length,0);
 assert(h.key('sampler-audition-126','Home').handled);assert.equal(h.document.activeElement.id,'sampler-audition-0');assert(h.nodes.get('sampler-marker-0').disabled);
});
test('audition selects exact native instrument and slice at root pitch without recording or transport changes',async()=>{
 const h=harness(128);h.instrument.rootNote=127;h.mount();const before=clone(h.state);await h.click('sampler-audition-127');
 assert.deepEqual(h.requests.map(x=>x.command),['engine.project','step.preview']);assert.equal(h.cancelSyncs,0);assert.equal(h.syncCalls,1);
 assert.deepEqual(h.previews()[0].payload,{track:0,step:{note:127,instrument:1,velocity:100,fx1:{type:'slice',value:128}},duration:2});
 assert.deepEqual(clone(h.state),before);assert.equal(h.mutations.length,0);assert.equal(h.timers.size,1);assert.equal([...h.timers.values()][0].ms,2000);
 assert(h.nodes.get('sampler-audition-status').textContent.includes('Auditioning slice 128'));
 [...h.timers.values()][0].fn();assert.equal(h.stops().length,1);assert.equal(h.stops()[0].payload.track,0);assert.equal(h.timers.size,0);
});
test('Return commits a marker once, synchronizes the changed map, hears it, and retains field focus',async()=>{
 const h=harness();h.nodes.get('sampler-marker-1').value='31.25';const event=h.key('sampler-marker-1','Enter');await flush();
 assert(event.handled&&event.defaultPrevented);assert.equal(h.mutations.length,1);assert.equal(h.instrument.slicePoints[1],.3125);assert.equal(h.instrument.mode,'slice');assert.equal(h.state.selection,null);
 assert.equal(h.requests.find(x=>x.command==='engine.project').payload.project.instruments[1].slicePoints[1],.3125);
 assert.equal(h.previews()[0].payload.step.fx1.value,2);assert.equal(h.document.activeElement.id,'sampler-marker-1');assert.equal(h.state.row,13);assert(h.state.recording);
 h.key('sampler-marker-1','Enter');await flush();assert.equal(h.mutations.length,1);assert.equal(h.previews().length,2);
 h.nodes.get('sampler-marker-1').value='';h.key('sampler-marker-1','Enter');await flush();assert.equal(h.previews().length,2);assert.equal(h.notices.at(-1).type,'error');
});
test('Space and Return never repeat; numeric change commits without automatically playing',async()=>{
 const h=harness();h.key('sampler-audition-0',' ',{repeat:true});await flush();assert.equal(h.requests.length,0);
 h.key('sampler-audition-0',' ');await flush();assert.equal(h.previews().length,1);h.key('sampler-audition-0','Enter',{repeat:true});await flush();assert.equal(h.previews().length,1);
 const field=h.nodes.get('sampler-marker-1');field.focus();field.value='30';await field.fire('change');await flush();assert.equal(h.mutations.length,1);assert.equal(h.previews().length,1);assert.equal(h.stops().length,1);
});
test('rapid requests wait for the graph and clone its latest finished rack; only newest slice sounds',async()=>{
 const h=harness(),gate=deferred();h.gate(gate);const first=h.click('sampler-audition-0'),second=h.click('sampler-audition-2');assert.equal(h.requests.length,0);
 h.state.project.audioPlugins=[{owner:'track:'+h.state.project.tracks[0].routingID,kind:'effect',slot:1,instanceID:'reordered',component:'clap:com.example.plugin',format:'CLAP'}];gate.resolve();await Promise.all([first,second]);
 assert.equal(h.requests.filter(x=>x.command==='engine.project').length,1);assert.equal(h.requests[0].payload.project.audioPlugins[0].instanceID,'reordered');assert.equal(h.previews()[0].payload.step.fx1.value,3);assert.equal(h.stops().length,0);
});
test('superseded sync and preview replies cannot start late sound or cancel the replacement',async()=>{
 const h=harness(),firstSync=deferred();let syncs=0;h.handle('engine.project',()=>++syncs===1?firstSync.promise:{ok:true});const first=h.click('sampler-audition-0');await flush();
 await h.click('sampler-audition-1');firstSync.resolve({ok:true});await first;assert.equal(h.previews().length,1);assert.equal(h.previews()[0].payload.step.fx1.value,2);assert.equal(h.stops().length,0);
 const second=harness(),pending=deferred();second.handle('step.preview',payload=>payload.step.note>=0&&payload.step.fx1.value===1?pending.promise:{auditioning:payload.step.note>=0});
 const old=second.click('sampler-audition-0');await flush();await second.click('sampler-audition-3');assert.equal(second.stops().length,1);pending.resolve({auditioning:true});await old;
 assert.equal(second.stops().length,1);assert.equal(second.previews().length,2);assert(second.nodes.get('sampler-audition-status').textContent.includes('slice 4'));assert.equal(second.timers.size,1);
});
test('Escape cancels graph waiting or native asset loading without stopping transport',async()=>{
 const h=harness(),gate=deferred();h.gate(gate);const waiting=h.click('sampler-audition-1');const esc=h.key('sampler-audition-1','Escape');assert(esc.handled);gate.resolve();await waiting;assert.equal(h.requests.length,0);assert(h.state.playing);
 const native=harness(),pending=deferred();native.handle('step.preview',payload=>payload.step.note>=0?pending.promise:{auditioning:false});const running=native.click('sampler-audition-2');await flush();
 native.key('sampler-audition-2','Escape');assert.equal(native.stops().length,1);pending.resolve({auditioning:true});await running;assert.equal(native.stops().length,1);assert.equal(native.timers.size,0);assert(native.state.playing&&native.state.recording);
 assert(!native.requests.some(x=>x.command.startsWith('transport.')||x.command==='engine.panic'||x.command.startsWith('note.')));
});
test('focus leaving the map cancels promptly; focus changes inside it preserve an active selected slice',async()=>{
 const h=harness();await h.click('sampler-audition-1');h.nodes.get('sampler-marker-1').focus();await flush();assert.equal(h.stops().length,0);
 h.outside.focus();await flush();assert.equal(h.stops().length,1);assert.equal(h.timers.size,0);
 const pending=harness(),gate=deferred();pending.gate(gate);const run=pending.click('sampler-audition-0');pending.outside.focus();await flush();gate.resolve();await run;assert.equal(pending.requests.length,0);
});
test('project, instrument, source, map, root, selected track, modal and navigation changes invalidate queued sound',async()=>{
 const changes=[h=>h.state.project=clone(h.state.project),h=>h.state.project.instruments[1]={...h.instrument},h=>h.state.instrument=0,h=>h.instrument.samplePath='/new.wav',h=>h.instrument.start=.2,h=>h.instrument.end=.8,h=>h.instrument.slicePoints=[0,.3,.6],h=>h.instrument.sliceCount=8,h=>h.instrument.rootNote=60,h=>h.instrument.zones=[{samplePath:'/zone.wav'}],h=>h.state.project.tracks[0]={instrument:1},h=>h.state.track=1,h=>h.state.modal='dialog',h=>h.state.view='pattern'];
 for(const change of changes){
  for(const phase of ['graph','sync']){const h=harness(),wait=deferred();if(phase==='graph')h.gate(wait);else h.handle('engine.project',()=>wait.promise);const run=h.click('sampler-audition-1');await flush();change(h);wait.resolve({ok:true});await run;assert.equal(h.previews().length,0,`${phase}: ${change}`);assert.equal(h.stops().length,0);}
 }
});
test('rebuild cancels changed maps and stop always uses original track, never later preview ownership',async()=>{
 const h=harness();await h.click('sampler-audition-1');h.instrument.slicePoints=[0,.1,.5];h.mount();assert.equal(h.stops().length,1);assert.equal(h.timers.size,0);
 const pending=harness(),wait=deferred();pending.handle('step.preview',payload=>payload.step.note>=0?wait.promise:{auditioning:false});const run=pending.click('sampler-audition-1');await flush();pending.state.track=9;pending.module.stopAudition();assert.equal(pending.stops()[0].payload.track,0);
 pending.requests.push({command:'step.preview',payload:{track:9,step:{note:60}}});wait.resolve({auditioning:true});await run;assert.equal(pending.stops().length,1);pending.module.stopAudition();assert.equal(pending.stops().length,1);
});
test('strict sync failure blocks preview and shows a recoverable error; canceled publication is quiet',async()=>{
 const invalid=harness();invalid.handle('engine.project',()=>null);await invalid.click('sampler-audition-0');assert.equal(invalid.previews().length,0);assert.equal(invalid.notices.at(-1).type,'error');assert(invalid.nodes.get('sampler-audition-status').classList.contains('is-error'));
 const canceled=harness();canceled.ST.syncEngineProject=async()=>false;await canceled.click('sampler-audition-0');assert.equal(canceled.previews().length,0);assert.equal(canceled.notices.length,0);
 const h=harness();h.handle('engine.project',()=>{throw new Error('Native sync failed');});await h.click('sampler-audition-0');assert.equal(h.previews().length,0);assert(h.nodes.get('sampler-audition-status').textContent.includes('Native sync failed'));
 h.instrument.samplePath='';h.mount();assert(h.nodes.get('sampler-audition-0').disabled);h.key('sampler-audition-0','Enter');await flush();assert.equal(h.previews().length,0);
});
test('pending plug-in edits flush before the sampler payload is built, and audition awaits native restoration',async()=>{
 const h=harness(),edits=deferred(),restored=deferred();h.state.project.audioPlugins=[{instanceID:'keys',state:'old'}];
 h.pendingEdits(async()=>{await edits.promise;h.state.project.audioPlugins[0].state='new';});h.handle('engine.project',()=>restored.promise);
 const run=h.click('sampler-audition-2');await flush();assert.equal(h.requests.length,0);assert.equal(h.syncCalls,1);
 edits.resolve();await flush();assert.equal(h.requests.length,1);assert.equal(h.requests[0].payload.project.audioPlugins[0].state,'new');assert.equal(h.previews().length,0);
 restored.resolve({ok:true});await run;assert.equal(h.previews().length,1);assert.equal(h.previews()[0].payload.step.fx1.value,3);assert.equal(h.cancelSyncs,0);
});
test('cancel, navigation, or failed pending plug-in edits cannot publish a stale audition project',async()=>{
 for(const change of ['cancel','project','sample','focus','error']){
  const h=harness(),edits=deferred();h.pendingEdits(()=>edits.promise);const run=h.click('sampler-audition-1');await flush();
  if(change==='cancel')h.module.stopAudition();if(change==='project')h.state.project=clone(h.state.project);if(change==='sample')h.instrument.samplePath='/replacement.wav';if(change==='focus')h.outside.focus();
  if(change==='error')edits.reject(new Error('Pending control edit could not be saved'));else edits.resolve();await run;
  assert.equal(h.requests.length,0,change);assert.equal(h.previews().length,0,change);assert.equal(h.cancelSyncs,0);
  assert.equal(h.notices.length,change==='error'?1:0,change);
 }
});
test('audition with either record-arm setting leaves project, cursor, selection and undo exactly unchanged',async()=>{
 for(const recording of [true,false]){const h=harness();h.state.recording=recording;const before=clone(h.state);h.key('sampler-audition-0','ArrowDown');h.key('sampler-audition-1','Enter');await flush();h.key('sampler-audition-1','Escape');assert.deepEqual(clone(h.state),before);assert.equal(h.mutations.length,0);assert.equal(h.previews().length,1);assert.equal(h.stops().length,1);}
 const h=harness();assert.equal(h.module.stopAudition(),false);assert.equal(h.requests.length,0);
});
(async()=>{for(const {name,fn} of tests){await fn();console.log(`PASS ${name}`);}console.log(`Sampler direct audition: ${tests.length} regression groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
