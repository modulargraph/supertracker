#!/usr/bin/env node
'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const source = web.source('nks-controls.js');
const copy = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const tick = async () => { for (let n = 0; n < 30; n++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; };
class Element {
  constructor(tag, attrs = '', text = '') {
    this.tag=tag; this.dataset={}; this.attributes={}; this.listeners={}; this.textContent=text; this.value=''; this.disabled=/\sdisabled(?:\s|$)/.test(attrs); this.isConnected=true;
    for (const match of attrs.matchAll(/([\w-]+)="([^"]*)"/g)) { this.attributes[match[1]]=match[2]; if (match[1].startsWith('data-')) this.dataset[match[1].slice(5).replace(/-([a-z])/g,(_,c)=>c.toUpperCase())]=match[2]; else this[match[1]]=match[2]; }
  }
  setAttribute(key,value){this.attributes[key]=String(value);}
  addEventListener(type,fn){(this.listeners[type]??=[]).push(fn);}
  fire(type,extra={}){const event={type,target:this,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;},...extra};for(const fn of this.listeners[type]||[])fn(event);return event;}
  focus(){this.document.activeElement=this;this.fire('focus');}
}
class Root extends Element {
  constructor(html,document) {
    super('section',html.match(/<section\b([^>]*)>/)?.[1]||''); this.document=document; this.nodes=[];
    for (const match of html.matchAll(/<(input|output|select|button|span)\b([^>]*)>([^<]*)/g)) { const node=new Element(match[1],match[2],match[3]); node.document=document; this.nodes.push(node); }
  }
  querySelectorAll(selector) {
    const match=selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/); if(!match)return [];
    return this.nodes.filter(node=>Object.hasOwn(node.attributes,match[1])&&(match[2]===undefined||node.attributes[match[1]]===match[2]));
  }
  querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
}
function harness() {
  const pages=[{index:0,name:'Sound <one>',parameters:[{slot:0,name:'Cutoff <bright>',section:'Filter',parameterID:0,mapped:true,mappingStatus:'mapped'},{slot:1,name:'Read only',parameterID:91,mapped:true},{slot:2,name:'Unmapped',parameterID:999,mapped:false,mappingStatus:'parameterUnavailable'},{slot:3,name:'No automation',parameterID:17,mapped:true}]},{index:1,name:'Movement',parameters:[{slot:0,name:'Amount',parameterID:123,mapped:true},{slot:7,name:'Unknown',mapped:false}]}];
  const plugin={owner:'track:t0',kind:'instrument',slot:0,component:'vst3:ABCD',format:'VST3',bundlePath:'/Plugin.vst3',name:'Synth',state:'baseline',custom:'preserved',nks:{name:'Preset',path:'/Preset.nksf',identity:{format:'vst3',vst3ClassID:'ABCD'},pages}};
  const inst={id:0,type:'vst3',name:'Synth',plugin:copy(plugin)},project={tracks:[{instrument:0,routingID:'t0'}],instruments:[inst],audioPlugins:[plugin]},state={project,track:0,instrument:0,view:'instrument'};
  const parameters=[{address:'0',name:'Filter',normalized:.25,display:'1.2 kHz',writable:true,automatable:true},{address:'91',normalized:.3,display:'read',writable:false},{address:'17',normalized:.5,automatable:false},{address:'123',normalized:.75,display:'75%'}];
  const requests=[],mutations=[],notices=[],timers=new Map(),handlers=new Map(); let sequence=0,root=null,refreshes=0,restoreFocus=true;
  const document={activeElement:null,querySelectorAll:selector=>selector==='[data-nks-root]'&&root?[root]:[],getElementById:id=>root?.nodes.find(node=>node.id===id)};
  const ST={state,escapeHTML,request:async(command,payload)=>{requests.push({command,payload:copy(payload)});if(handlers.has(command))return handlers.get(command)(payload);if(command==='plugin.parameters')return copy(parameters);if(command==='plugin.parameter'){const p=parameters.find(p=>p.address===payload.address);if(p)p.normalized=payload.value;return {display:`value ${payload.value}`};}if(command==='plugin.state')return {component:'vst3:ABCD',state:`saved:${parameters[0].normalized}`};throw Error(command);},mutate(label,fn,options){mutations.push({label,before:copy(state.project),options});fn(state.project);},toast(message,type){notices.push({message,type});},refresh(){refreshes++;mount();}};
  const context={window:{ST},document,Date,setTimeout:(fn,delay)=>{timers.set(++sequence,{fn,delay});return sequence;},clearTimeout:id=>timers.delete(id)};
  web.runText('nks-controls.js', source, context,{filename:'nks-controls.js'}); const api=context.window.STNKSControls;
  function mount(){const focused=document.activeElement?.id;if(root)root.isConnected=false;root=new Root(api.render(inst,state.project.audioPlugins[0]),document);api.bind();if(focused&&restoreFocus){const next=root.nodes.find(n=>n.id===focused);if(next)document.activeElement=next;}return root;}
  return {api,ST,state,project,plugin,inst,parameters,window:context.window,requests,mutations,notices,handlers,document,mount,get root(){return root;},get refreshes(){return refreshes;},set restoreFocusOnMount(value){restoreFocus=value;},input(slot=0,type='number'){return root.nodes.find(n=>n.id===`nks-0-${slot}-${type}`);},flush(delay){for(const [id,timer] of [...timers])if(timer.delay===delay){timers.delete(id);timer.fn();}},change(value,slot=0){const input=this.input(slot);input.value=String(value);input.fire('input');input.fire('change');}};
}
(async()=>{
  {
    const h=harness(),html=h.api.render(h.inst,h.plugin);
    assert.equal((html.match(/data-nks-slot=/g)||[]).length,8);assert(html.includes('Cutoff &lt;bright&gt;'));assert(!html.includes('Cutoff <bright>'));
    assert(!html.includes('data-automate="plugin:instrument:0:0"'),'unread mappings cannot advertise automation before native parameters arrive');assert(!html.includes('data-automate="plugin:instrument:0:999"'));assert(html.includes('Parameter unavailable'));assert(html.includes('aria-label="NKS controller page"'));
    h.mount();await tick();assert.equal(h.input().value,'0.25');assert.equal(h.input().disabled,false);assert.equal(h.input(1).disabled,true);assert.equal(h.input(3).dataset.automate,undefined);assert.equal(h.input().dataset.automate,'plugin:instrument:0:0');
    assert.equal(h.root.querySelector('[data-nks-display="0"]').textContent,'1.2 kHz');assert.equal(h.mutations.length,0);
    assert(html.includes('data-nks-control-help'));
    assert(html.includes('adds an automation lane. Use on step… locks a value to the selected step.'));
    assert(!html.includes('data-nks-compatibility-note'));
    const massive=harness();massive.plugin.component='vst3:5653544E6924486D6173736976652078';
    const note=massive.api.render(massive.inst,massive.plugin);
    assert(note.includes('data-nks-control-help'));
    for(const engineering of ['1.7.1','Contender','tested','unverified','about one second','failed project recall']) assert(!note.includes(engineering));
    massive.plugin.version='2.0';assert(massive.api.render(massive.inst,massive.plugin).includes('data-nks-control-help'));
    const unmapped=harness();for(const page of unmapped.plugin.nks.pages)for(const parameter of page.parameters)parameter.mapped=false;
    assert(!unmapped.api.render(unmapped.inst,unmapped.plugin).includes('data-nks-control-help'));
    const noPages=harness();noPages.plugin.nks.pages=[];assert(!noPages.api.render(noPages.inst,noPages.plugin).includes('data-nks-control-help'));
  }
  {
    const h=harness();h.mount();await tick();const input=h.input();input.focus();
    for(const value of [.3,.4,.6]){input.value=String(value);input.fire('input');}
    assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').length,0);h.flush(35);await tick();assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').length,1);assert.equal(h.requests.at(-1).payload.value,.6);assert.equal(h.mutations.length,0);
    input.fire('change');await tick();assert.equal(h.mutations.length,1);assert.equal(h.mutations[0].before.audioPlugins[0].state,'baseline');assert.equal(h.state.project.audioPlugins[0].state,'saved:0.6');assert.equal(h.state.project.audioPlugins[0].custom,'preserved');assert.deepEqual(h.state.project.audioPlugins[0].nks,h.plugin.nks);assert.equal(h.inst.plugin.state,'saved:0.6');assert.equal(h.document.activeElement,input);assert.equal(h.mutations[0].options.render,false);
    input.value='';input.fire('change');await tick();assert.equal(h.mutations.length,1);assert.equal(input.value,'0.6');
  }
  {
    const h=harness();h.mount();await tick();h.input().focus();const e=h.root.fire('keydown',{key:'PageDown',ctrlKey:true,metaKey:false,altKey:false});await tick();assert(e.prevented&&e.stopped);assert.equal(h.refreshes,1);assert.equal(h.document.activeElement.id,'nks-0-0-number');assert.equal(h.input().dataset.nksAddress,'123');assert.equal(h.input().value,'0.75');assert.equal(h.mutations.length,0);
  }
  {
    const h=harness();h.mount();await tick();h.input().value='.8';h.input().fire('input');h.state.project.audioPlugins[0]={...h.plugin,component:'vst3:REPLACEMENT'};h.flush(35);await tick();assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').length,0);assert.equal(h.mutations.length,0);
  }
  {
    const h=harness();h.mount();await tick();const snapshot=deferred();h.handlers.set('plugin.state',()=>snapshot.promise);h.change(.8);await tick();h.state.project=copy(h.project);snapshot.resolve({component:'vst3:ABCD',state:'stale'});await tick();assert.equal(h.mutations.length,0);assert.equal(h.state.project.audioPlugins[0].state,'baseline');
  }
  {
    const h=harness();h.mount();await tick();const write=deferred(),seen=[];h.handlers.set('plugin.parameter',payload=>{seen.push(payload.value);if(seen.length===1)return write.promise;h.parameters[0].normalized=payload.value;return {display:String(payload.value)};});
    const input=h.input();input.value='.2';input.fire('input');h.flush(35);await tick();h.change(.9);await tick();assert.deepEqual(seen,[.2]);write.resolve({display:'.2'});await tick();assert.deepEqual(seen,[.2,.9]);assert.equal(h.mutations.length,1);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.9');assert.equal(input.value,'0.9');
  }
  {
    const h=harness();h.mount();await tick();h.handlers.set('plugin.parameter',()=>Promise.reject(Error('Plugin unavailable')));h.change(.8);await tick();assert.equal(h.mutations.length,0);assert(h.notices.some(n=>n.message==='Plugin unavailable'));assert.equal(h.state.project.audioPlugins[0].state,'baseline');
  }
  {
    const h=harness();h.mount();await tick();h.parameters[0].normalized=.8;h.parameters[0].display='8 kHz';h.flush(750);await tick();assert.equal(h.input().value,'0.8');assert.equal(h.mutations.length,0);
    h.input().value='.6';h.input().fire('input');h.parameters[0].normalized=.2;h.flush(750);await tick();assert.equal(h.input().value,'0.6');h.api.event('project.loaded');h.flush(35);await tick();assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').length,0);
  }
  {
    const h=harness();h.mount();await tick();h.handlers.set('plugin.state',()=>({component:'vst3:DIFFERENT',state:'wrong'}));h.change(.7);await tick();assert.equal(h.mutations.length,0);assert.equal(h.state.project.audioPlugins[0].state,'baseline');
  }
  {
    const h=harness();h.mount();await tick();h.input().focus();h.input().value='.42';h.input().fire('input');
    h.root.fire('keydown',{key:'PageDown',ctrlKey:true,metaKey:false,altKey:false});await tick();
    assert.equal(h.mutations.length,1);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.42');assert.equal(h.input().dataset.nksAddress,'123');
    h.parameters[3].normalized=.25;h.flush(750);await tick();assert.equal(h.input().value,'0.25');
  }
  {
    const h=harness();h.mount();await tick();h.input().value='.35';h.input().fire('input');h.root.isConnected=false;h.flush(750);await tick();
    assert.equal(h.mutations.length,1);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.35');
  }
  {
    const h=harness();h.mount();await tick();h.input().focus();h.input().value='.63';h.input().fire('input');h.flush(35);await tick();
    h.input().value='';h.input().fire('change');await tick();assert.equal(h.mutations.length,1);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.63');
    h.parameters[0].normalized=.2;h.flush(750);await tick();assert.equal(h.input().value,'0.2');
    h.input(3).focus();h.root.fire('keydown',{key:'PageDown',ctrlKey:true});await tick();assert.equal(h.document.activeElement.id,'nks-0-page');
  }
  {
    const h=harness();h.mount();await tick();const original=copy(h.state.project),left=h.input(0,'range'),right=h.input(3,'range');left.focus();
    let e=h.root.fire('keydown',{target:left,key:'ArrowRight'});assert(e.prevented&&e.stopped);assert.equal(h.document.activeElement,right,'navigation skips read-only and unmapped slots but keeps writable non-automatable controls');
    e=h.root.fire('keydown',{target:right,key:'ArrowRight'});assert(e.prevented&&e.stopped);assert.equal(h.document.activeElement,right,'last knob stays selected');
    e=h.root.fire('keydown',{target:right,key:'ArrowLeft'});assert(e.prevented&&e.stopped);assert.equal(h.document.activeElement,left);
    h.root.fire('keydown',{target:left,key:'ArrowLeft'});assert.equal(h.document.activeElement,left,'first knob stays selected');
    h.flush(35);await tick();assert.deepEqual(h.state.project,original);assert.equal(left.value,'0.25');assert.equal(right.value,'0.5');assert.equal(h.mutations.length,0);
    assert(!h.requests.some(r=>['plugin.parameter','plugin.state'].includes(r.command)),'navigation must never synthesize a write or undo snapshot');
    const html=h.api.render(h.inst,h.plugin);assert(html.includes('Knobs: <kbd>← →</kbd> move'));assert(html.includes('aria-describedby="nks-0-0-display nks-0-keyboard-hint"'));
    assert(!html.includes('tabindex='),'range navigation does not change native Tab order');
  }
  {
    const h=harness();h.mount();await tick();const range=h.input(0,'range'),number=h.input();number.focus();
    for(const key of ['ArrowLeft','ArrowRight','ArrowUp','ArrowDown','Home','End']){const e=h.root.fire('keydown',{target:number,key});assert(!e.prevented&&!e.stopped);assert.equal(h.document.activeElement,number);}
    range.focus();
    let e=h.root.fire('keydown',{target:range,key:'ArrowUp'});assert(e.prevented&&e.stopped,'Up adjusts by a coarse 1% step');assert.equal(range.value,'0.26');
    e=h.root.fire('keydown',{target:range,key:'ArrowDown',shiftKey:true});assert(e.prevented&&e.stopped);assert.equal(range.value,'0.16','Shift adjusts by 10%');
    e=h.root.fire('keydown',{target:range,key:'ArrowUp',altKey:true});assert(e.prevented&&e.stopped);assert.equal(range.value,'0.161','Option adjusts finely');
    assert.equal(h.mutations.length,0,'adjusting previews without committing');h.root.fire('keyup',{target:range,key:'ArrowUp'});await tick();
    assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').at(-1).payload.value,.161);assert.equal(h.mutations.length,1,'releasing the key commits one Undo step');
    for(const modifier of ['metaKey','ctrlKey','altKey','isComposing']){const e=h.root.fire('keydown',{target:range,key:'ArrowRight',[modifier]:true});assert(!e.prevented&&!e.stopped);assert.equal(h.document.activeElement,range);}
    range.value='.251';range.fire('input');range.fire('change');await tick();assert.equal(h.requests.filter(r=>r.command==='plugin.parameter').at(-1).payload.value,.251);assert.equal(h.mutations.length,2,'native range changes keep the existing commit path');
    const paged=h.root.fire('keydown',{target:range,key:'ArrowRight',shiftKey:true});await tick();assert(paged.prevented&&paged.stopped,'Shift Right pages');assert.equal(h.refreshes,1);
  }
  {
    const h=harness();h.mount();await tick();h.restoreFocusOnMount=false;const old=h.input(0,'range');old.focus();
    const down=h.root.fire('keydown',{target:old,key:'PageDown',ctrlKey:true});await tick();assert(down.prevented&&down.stopped);assert.equal(h.document.activeElement,h.input(0,'range'));assert.notEqual(h.document.activeElement,old,'page navigation explicitly restores a new mounted control');
    assert.equal(h.input(0,'range').dataset.nksAddress,'123');assert.equal(h.input(0,'range').value,'0.75');
    h.root.fire('keydown',{target:h.input(0,'range'),key:'PageUp',ctrlKey:true});await tick();assert.equal(h.document.activeElement,h.input(0,'range'));assert.equal(h.input(0,'range').dataset.nksAddress,'0');
    h.input(3,'range').focus();h.root.fire('keydown',{target:h.input(3,'range'),key:'PageDown',ctrlKey:true});await tick();assert.equal(h.document.activeElement.id,'nks-0-page','an unavailable same-slot control falls back to page selection');
    assert.equal(h.mutations.length,0);assert(!h.requests.some(r=>['plugin.parameter','plugin.state'].includes(r.command)));
  }
  {
    const h=harness();h.mount();await tick();h.parameters.find(p=>p.address==='17').writable=false;h.flush(750);await tick();
    const only=h.input(0,'range');only.focus();h.root.fire('keydown',{target:only,key:'ArrowRight'});assert.equal(h.document.activeElement,only,'live read-only changes are respected');
    const disabled=h.input(3,'range');assert(disabled.disabled);const ignored=h.root.fire('keydown',{target:disabled,key:'ArrowLeft'});assert(!ignored.prevented&&!ignored.stopped);
    h.root.isConnected=false;const stale=h.root.fire('keydown',{target:only,key:'ArrowRight'});assert(!stale.prevented&&!stale.stopped,'disconnected panels cannot steal focus');
    assert(!h.requests.some(r=>['plugin.parameter','plugin.state'].includes(r.command)));
  }
  {
    const h=harness();h.mount();await tick();const input=h.input();input.focus();input.value='';input.fire('input');
    h.parameters[0].normalized=.8;h.parameters[0].display='8 kHz';h.flush(750);await tick();
    assert.equal(input.value,'','native polling must preserve a focused blank/partial number draft');assert.equal(h.input(0,'range').value,'0.8','live parameter display continues independently of the numeric draft');
    h.mount();await tick();assert.equal(h.input().value,'','same-page redraw preserves the raw draft');
    h.input().fire('change');await tick();assert.equal(h.input().value,'0.8','invalid blank commit keeps the existing restore-to-known-value behavior');
    assert.equal(h.mutations.length,0);assert(!h.requests.some(r=>['plugin.parameter','plugin.state'].includes(r.command)),'focus/draft preservation adds no native writes');
  }
  {
    const h=harness();h.mount();await tick();const pending=deferred();h.handlers.set('plugin.parameters',()=>pending.promise);h.flush(750);await tick();
    const input=h.input();input.focus();input.value='';input.fire('input');pending.resolve(h.parameters.map(p=>p.address==='0'?{...p,normalized:.65,display:'65%'}:p));await tick();
    assert.equal(input.value,'','a read already in flight cannot replace a newer draft');assert.equal(h.input(0,'range').value,'0.65');
    h.document.activeElement=h.input(3);input.fire('blur');assert.equal(input.value,'0.65','leaving an uncommitted blank restores the live value without writing');
    assert.equal(h.document.activeElement,h.input(3),'blur cleanup cannot move focus back');assert.equal(h.mutations.length,0);
  }
  {
    const h=harness();h.mount();await tick();const input=h.input();input.focus();input.value='';input.fire('input');
    h.root.fire('keydown',{target:input,key:'PageDown',ctrlKey:true});await tick();
    assert.equal(h.input().dataset.nksAddress,'123');assert.equal(h.input().value,'0.75','draft from the previous page must not leak into its same-slot successor');
    h.root.fire('keydown',{target:h.input(),key:'PageUp',ctrlKey:true});await tick();assert.equal(h.input().value,'0.25');
    assert.equal(h.mutations.length,0);assert(!h.requests.some(r=>['plugin.parameter','plugin.state'].includes(r.command)));
  }
  {
    const h=harness();h.mount();await tick();const saved=copy(h.state.project);
    h.handlers.set('plugin.state',()=>({component:'vst3:ABCD',state:'stale-fallback-blob',stateError:'The plug-in failed to capture its edited state.'}));
    h.change(.8);await tick();assert.equal(h.mutations.length,0,'stateError must reject a fallback even when its component and opaque state look valid');
    assert.deepEqual(h.state.project,saved);assert.equal(h.inst.plugin.state,'baseline');assert(h.notices.some(notice=>notice.message==='The plug-in failed to capture its edited state.'));
    h.handlers.delete('plugin.state');h.change(.6);await tick();assert.equal(h.mutations.length,1,'snapshot failure must not block a later successful edit');assert.equal(h.state.project.audioPlugins[0].state,'saved:0.6');
  }
  {
    const h=harness();h.mount();await tick();
    h.parameters[0].automatable=false;h.flush(750);await tick();assert.equal(h.input().dataset.automate,undefined);
    h.parameters[0].automatable=true;h.flush(750);await tick();assert.equal(h.input().dataset.automate,'plugin:instrument:0:0','automation must be restored after the native flag returns');
    h.parameters[0].writable=false;h.flush(750);await tick();assert.equal(h.input().disabled,true);assert.equal(h.input().dataset.automate,undefined);
    h.parameters[0].writable=true;h.flush(750);await tick();assert.equal(h.input().dataset.automate,'plugin:instrument:0:0');
    h.parameters.shift();h.flush(750);await tick();assert.equal(h.input().disabled,true);assert.equal(h.input().dataset.automate,undefined,'removed native parameters cannot retain stale automation actions');
  }
  {
    const h=harness();h.mount();await tick();const input=h.input();input.value='.63';input.fire('input');
    assert.equal(h.requests.filter(x=>x.command==='plugin.parameter').length,0);
    assert.equal(await h.api.flushEdits(),true);assert.equal(h.parameters[0].normalized,.63);
    assert.equal(h.state.project.audioPlugins[0].state,'saved:0.63');assert.equal(h.mutations.length,1);
    assert.equal(await h.api.flushEdits(),true);assert.equal(h.mutations.length,1,'clean saves add no redundant undo capture');
  }
  {
    const h=harness();h.mount();await tick();h.handlers.set('plugin.parameter',()=>{throw Error('write rejected');});
    const input=h.input();input.value='.64';input.fire('input');
    assert.equal(await h.api.flushEdits(),false);assert.equal(h.mutations.length,0);assert.equal(h.parameters[0].normalized,.25);
    h.handlers.delete('plugin.parameter');assert.equal(await h.api.flushEdits(),true,'explicit retry must replay the failed final write');
    assert.equal(h.parameters[0].normalized,.64);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.64');
  }
  {
    const h=harness();h.mount();await tick();h.handlers.set('plugin.state',()=>{throw Error('capture rejected');});
    const input=h.input();input.value='.65';input.fire('input');
    assert.equal(await h.api.flushEdits(),false);assert.equal(h.mutations.length,0);assert.equal(h.state.project.audioPlugins[0].state,'baseline');
    h.handlers.delete('plugin.state');assert.equal(await h.api.flushEdits(),true,'failed state capture remains pending until successfully retried');
    assert.equal(h.state.project.audioPlugins[0].state,'saved:0.65');assert.equal(h.mutations.length,1);
  }
  {
    const h=harness();h.mount();await tick();const calls=[],waiting=deferred();h.ST.automate=(...args)=>calls.push(args);h.handlers.set('plugin.state',()=>waiting.promise);
    const input=h.input(0,'range');input.value='.67';input.fire('input');input.focus();
    const key=h.root.fire('keydown',{target:input,key:'å',code:'KeyA',altKey:true});await tick();
    assert(key.prevented&&key.stopped);assert.equal(calls.length,0,'the global action must not outrun final native state capture');
    waiting.resolve({component:'vst3:ABCD',state:'saved:0.67'});await tick();
    assert.deepEqual(calls,[['track:t0','plugin:instrument:0:0','Cutoff &lt;bright&gt;',.67]]);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.67');
  }
  {
    const h=harness();h.mount();await tick();const calls=[],waiting=deferred();h.ST.automate=(...args)=>calls.push(args);h.handlers.set('plugin.state',()=>waiting.promise);
    const input=h.input();input.value='.68';input.fire('input');h.root.fire('keydown',{target:input,key:'å',code:'KeyA',altKey:true});await tick();
    h.state.view='mixer';waiting.resolve({component:'vst3:ABCD',state:'saved:0.68'});await tick();
    assert.equal(calls.length,0,'completed capture must not undo navigation made while waiting');assert.equal(h.state.project.audioPlugins[0].state,'saved:0.68');
  }
  {
    const h=harness();h.mount();await tick();const input=h.input();input.value='.69';input.fire('input');h.root.isConnected=false;
    h.flush(750);await tick();assert.equal(await h.api.flushEdits(),true);assert.equal(h.state.project.audioPlugins[0].state,'saved:0.69','unmounted pending knobs must still settle before save');
    const metadata=h.api.automationMetadata(h.plugin);assert.equal(metadata.get('0').name,'Cutoff <bright>');assert.equal(metadata.get('0').group,'NKS · Sound <one> · Filter');assert(!metadata.has('999'));
    h.plugin.nks.pages.push({name:'Second alias',parameters:[{slot:1,name:'Brightness',parameterID:0,mapped:true}]});assert(h.api.automationMetadata(h.plugin).get('0').search.includes('Brightness'));
  }
  {
    const h=harness(),calls=[];h.plugin.instanceID='exact-nks-instance';h.window.STStepFX={show:async options=>calls.push(copy(options))};
    h.handlers.set('plugin.state',()=>({component:'vst3:ABCD',instanceID:'exact-nks-instance',state:`saved:${h.parameters[0].normalized}`}));
    h.mount();await tick();const button=h.root.querySelector('[data-nks-step="0"]');assert.equal(button.disabled,false);
    assert.equal(h.root.querySelector('[data-nks-step="91"]').disabled,true);assert.equal(h.root.querySelector('[data-nks-step="17"]').disabled,true);
    const input=h.input();input.value='.72';input.fire('input');button.fire('click');await tick();
    assert.equal(h.state.project.audioPlugins[0].state,'saved:0.72');assert.deepEqual(calls,[{plugin:{instanceID:'exact-nks-instance',address:'0',name:'Cutoff &lt;bright&gt;'}}]);
    assert.equal(h.mutations.length,1,'opening a step assignment captures its pending native edit but never writes a pattern effect itself');
  }
  {
    const h=harness(),calls=[];h.window.STStepFX={show:async options=>calls.push(copy(options))};h.mount();await tick();
    assert.equal(h.root.querySelector('[data-nks-step="0"]').disabled,true,'legacy unassigned plugin identities cannot create locks by slot guessing');
    h.state.project.audioPlugins[0]={...h.plugin,instanceID:'exact'};h.api.event('project.loaded');h.mount();await tick();const input=h.input();input.value='.73';input.fire('input');h.handlers.set('plugin.state',()=>{throw Error('snapshot failure');});
    h.root.querySelector('[data-nks-step="0"]').fire('click');await tick();assert.equal(calls.length,0,'a failed native capture must not open a step assignment');
  }
  console.log('NKS controls: 29 suites passed (verified IDs, knob navigation without writes, numeric drafts across polling/redraw, native editing, keyboard pages/focus, normalized values, coalescing, undo/state, stateError rejection, async guards, errors, native synchronization).');
})().catch(error=>{console.error(error);process.exitCode=1;});
