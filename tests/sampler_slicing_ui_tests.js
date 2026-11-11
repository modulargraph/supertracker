'use strict';
// Actual sampler module at the DOM/bridge boundary; no audio devices or sample writes.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('sampler.js');
const clone = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char]));
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; };
function harness() {
  const nodes = new Map(), groups = new Map(), mutations = [], requests = [], notices = [];
  let requestResult = Promise.resolve({ slicePoints:[0,.2,.7], sliceCount:3 }), recordingEntries=0, html='';
  const document = {activeElement:null,getElementById:id=>nodes.get(id)||null,querySelectorAll:selector=>groups.get(selector)||[]};
  class Element {
    constructor(id, attrs='') {
      this.id=id;this.attrs={};this.dataset={};this.classList={toggle(){}};this.listeners=new Map();this.value='';this.textContent='';this.disabled=/\sdisabled(?:\s|$)/.test(attrs);this.hidden=/\shidden(?:\s|$)/.test(attrs);this.open=/\sopen(?:\s|$)/.test(attrs);
      for(const match of attrs.matchAll(/([\w-]+)="([^"]*)"/g)) {this.attrs[match[1]]=match[2]; if(match[1]==='value')this.value=match[2];if(match[1].startsWith('data-'))this.dataset[match[1].slice(5).replace(/-([a-z])/g,(_,x)=>x.toUpperCase())]=match[2];}
    }
    addEventListener(type,fn){const values=this.listeners.get(type)||[];values.push(fn);this.listeners.set(type,values);}
    async fire(type){for(const fn of this.listeners.get(type)||[])await fn({target:this});}
    focus(){if(!this.disabled)document.activeElement=this;}
    setAttribute(key,value){this.attrs[key]=value;}
  }
  const instrument={id:0,type:'sample',name:'Break',samplePath:'/fixture/break.wav',rootNote:60,start:.1,end:.9,slicePoints:[0,.5],sliceCount:2,mode:'oneshot'};
  const state={view:'instrument',modal:null,native:true,instrument:0,selection:{row:1},project:{bpm:120,instruments:[instrument,{id:1,type:'sample',samplePath:'/fixture/other.wav',start:0,end:1}],tracks:[{instrument:0}]} };
  const ST={state,escapeHTML,mutate(label,fn){mutations.push({label,before:clone(state.project)});fn(state.project);mount();},refresh(){mount();},toast(message,type){notices.push({message,type});},request(command,payload){requests.push({command,payload:clone(payload)});return requestResult;},showRecording(){recordingEntries++;}};
  const context={window:{ST},document,setTimeout:()=>1,clearTimeout(){},queueMicrotask};web.runText('sampler.js', source, context,{filename:'sampler.js'});
  const module=context.window.STSampler;
  function mount() {
    const focus=document.activeElement?.id;nodes.clear();groups.clear();
    html=module.render(state.project.instruments[state.instrument]);
    for(const match of html.matchAll(/<(input|button|select|details|output|p|div)\b([^>]*)>/g)){
      const id=match[2].match(/\bid="([^"]+)"/)?.[1]||`anonymous-${nodes.size}`,el=new Element(id,match[2]);nodes.set(id,el);
      for(const key of ['samplerMarker','samplerRemoveMarker'])if(el.dataset[key]!==undefined){const selector=key==='samplerMarker'?'[data-sampler-marker]':'[data-sampler-remove-marker]',values=groups.get(selector)||[];values.push(el);groups.set(selector,values);}
    }
    module.bind(); if(focus)nodes.get(focus)?.focus();
  }
  mount();
  return {module,state,instrument,nodes,document,mutations,requests,notices,mount,get html(){return html;},get recordingEntries(){return recordingEntries;},setResult(value){requestResult=Promise.resolve(value);},setPending(value){requestResult=value.promise;},
    async change(id,value,event='change'){const el=nodes.get(id);assert(el,`Missing ${id}`);el.value=String(value);await el.fire(event);},
    click(id){const el=nodes.get(id);assert(el,`Missing ${id}`);el.focus();return el.fire('click');}};
}
const tests=[];const test=(name,fn)=>tests.push({name,fn});

test('recording and chopping controls are discoverable with explicit region/beat semantics',async()=>{
 const h=harness(); assert(h.html.includes('Record line input…'));assert(h.html.includes('10.0–90.0%'));assert(h.html.includes('does not detect tempo or stretch audio'));
 assert(h.html.includes('Slice 1–2'));assert(h.html.includes('without a note does not retrigger'));
 await h.click('sampler-record-input');assert.equal(h.recordingEntries,1);
 h.instrument.type='synth';h.mount();assert.equal(h.html,'','retired synth instruments have no sampler sections');
});
test('transient detection sends bounded full-file region and commits relative starts once',async()=>{
 const h=harness(),before=clone(h.instrument);await h.click('sampler-detect-transients');
 assert.deepEqual(h.requests,[{command:'sample.analyzeSlices',payload:{instrument:0,path:'/fixture/break.wav',start:.1,end:.9,sensitivity:.5,minimumGapSeconds:.04,maximumSlices:128}}]);
 assert.equal(h.mutations.length,1);assert.deepEqual(h.mutations[0].before.instruments[0],before);
 assert.deepEqual(clone(h.instrument.slicePoints),[0,.2,.7]);assert.equal(h.instrument.slices,undefined,'the markers have one name');assert.equal(h.instrument.sliceCount,3);assert.equal(h.instrument.mode,'slice');
 assert.equal(h.instrument.samplePath,before.samplePath);assert.equal(h.instrument.start,.1);assert.equal(h.instrument.end,.9);assert.equal(h.state.selection,null);assert.equal(h.document.activeElement.id,'sampler-detect-transients');
 assert.equal(h.nodes.get('sampler-detect-transients').disabled,false);assert(h.nodes.get('sampler-slice-status').textContent.includes('3 slices detected'));
});
test('sensitivity/gap/limit settings are bounded and analysis progress is explicit',async()=>{
 const h=harness(),pending=deferred();h.setPending(pending);
 await h.change('sampler-transient-sensitivity',.85,'input');await h.change('sampler-transient-gap',1);await h.change('sampler-transient-limit',300);
 const run=h.click('sampler-detect-transients');assert.equal(h.requests[0].payload.sensitivity,.85);assert.equal(h.requests[0].payload.minimumGapSeconds,.005);assert.equal(h.requests[0].payload.maximumSlices,128);
 assert.equal(h.nodes.get('sampler-detect-transients').disabled,true);assert.equal(h.nodes.get('sampler-cancel-analysis').hidden,false);assert(h.nodes.get('sampler-slice-status').textContent.includes('Detecting'));
 pending.resolve({slicePoints:[0,.2],capped:true});await run;assert(h.nodes.get('sampler-slice-status').textContent.includes('Maximum reached'));
});
test('cancel discards the analysis without spawning a second worker or changing markers',async()=>{
 const h=harness(),before=clone(h.instrument),pending=deferred();h.setPending(pending);const run=h.click('sampler-detect-transients');
 await h.click('sampler-cancel-analysis');await h.click('sampler-detect-transients');assert.equal(h.requests.length,1);
 pending.resolve({slicePoints:[0,.2]});await run;assert.deepEqual(clone(h.instrument),before);assert.equal(h.mutations.length,0);assert(h.nodes.get('sampler-slice-status').textContent.includes('canceled'));
});
test('failure or invalid markers keep the source map and permit retry',async()=>{
 for(const result of [null,{slicePoints:[]},{slicePoints:[0,.3,.3]},{slicePoints:[0,1]},{slicePoints:[.2,.7]},{slicePoints:[0,NaN]},{slicePoints:Array.from({length:129},(_,i)=>i/130)}]){
  const h=harness(),before=clone(h.instrument);h.setResult(result);await h.click('sampler-detect-transients');assert.deepEqual(clone(h.instrument),before);assert.equal(h.mutations.length,0);assert.equal(h.nodes.get('sampler-detect-transients').disabled,false);assert.equal(h.notices.at(-1).type,'error');
 }
 const h=harness(),pending=deferred();h.setPending(pending);const run=h.click('sampler-detect-transients');pending.reject(new Error('Sample is offline'));await run;assert.equal(h.mutations.length,0);assert(h.nodes.get('sampler-slice-status').textContent.includes('offline'));
});
test('changed file, region, markers, instrument or project cannot receive a stale result',async()=>{
 const changes=[h=>h.instrument.samplePath='/new.wav',h=>h.instrument.start=.2,h=>h.instrument.slicePoints=[0,.1,.8],h=>h.state.instrument=1,h=>h.state.project={...h.state.project,instruments:[{...h.instrument}]}];
 for(const change of changes){const h=harness(),pending=deferred();h.setPending(pending);const run=h.click('sampler-detect-transients');change(h);const before=clone(h.state.project);pending.resolve({slicePoints:[0,.2,.7]});await run;assert.deepEqual(clone(h.state.project),before);assert.equal(h.mutations.length,0);}
});
test('successful background analysis preserves current keyboard focus and navigation',async()=>{
 const h=harness(),pending=deferred();h.setPending(pending);const run=h.click('sampler-detect-transients');
 h.nodes.get('sampler-equal-count').focus();pending.resolve({slicePoints:[0,.25,.5,.75]});await run;assert.equal(h.document.activeElement.id,'sampler-equal-count');
 const second=deferred();h.setPending(second);const run2=h.click('sampler-detect-transients');h.state.view='mixer';h.document.activeElement={id:'mixer-volume'};second.resolve({slicePoints:[0,.3]});await run2;assert.equal(h.state.view,'mixer');assert.equal(h.document.activeElement.id,'mixer-volume');
});
test('equal slicing validates count, enables slices and preserves audio/region',async()=>{
 const h=harness();await h.change('sampler-equal-count',16);await h.click('sampler-equal');assert.equal(h.instrument.slicePoints.length,16);assert.equal(h.instrument.slicePoints[15],15/16);assert.equal(h.mutations.length,1);assert.equal(h.document.activeElement.id,'sampler-equal');
 for(const value of ['',0,2.5,129]){await h.change('sampler-equal-count',value);await h.click('sampler-equal');}assert.equal(h.mutations.length,1);assert.equal(h.instrument.samplePath,'/fixture/break.wav');
});
test('beat slicing computes explicit quarter-note subdivisions and refuses oversized grids',async()=>{
 const h=harness();await h.change('sampler-region-beats',8,'input');await h.change('sampler-beat-grid',16);assert(h.nodes.get('sampler-beat-estimate').textContent.startsWith('32 equal slices'));
 await h.click('sampler-apply-beat-grid');assert.equal(h.instrument.sliceCount,32);assert.equal(h.instrument.slicePoints[31],31/32);assert.equal(h.document.activeElement.id,'sampler-apply-beat-grid');
 await h.change('sampler-region-beats',128,'input');await h.change('sampler-beat-grid',32);await h.click('sampler-apply-beat-grid');assert.equal(h.mutations.length,1);
 await h.change('sampler-region-beats',.3,'input');await h.change('sampler-beat-grid',4);await h.click('sampler-apply-beat-grid');assert.equal(h.mutations.length,1);
 // A cleared or negative length shows the same hint after a repaint as while typing (never "0 equal slices").
 for(const value of ['',-4]){await h.change('sampler-region-beats',value,'input');assert.match(h.nodes.get('sampler-beat-estimate').textContent,/^Choose a beat length/);
  assert.match(h.module.render(h.instrument),/id="sampler-beat-estimate"[^>]*>Choose a beat length/);}
});
test('manual markers retain ordering, protect first marker and keep keyboard focus',async()=>{
 const h=harness();await h.click('sampler-add-marker');assert.deepEqual(clone(h.instrument.slicePoints),[0,.25,.5]);assert.equal(h.document.activeElement.id,'sampler-marker-1');
 await h.change('sampler-marker-1',40);assert.deepEqual(clone(h.instrument.slicePoints),[0,.4,.5]);assert.equal(h.document.activeElement.id,'sampler-marker-1');
 await h.change('sampler-marker-1','');assert.deepEqual(clone(h.instrument.slicePoints),[0,.4,.5]);assert.equal(h.mutations.length,2);
 await h.change('sampler-marker-0',90);assert.equal(h.mutations.length,2);
 const remove=h.nodes.get([...h.nodes.keys()].find(key=>h.nodes.get(key).dataset.samplerRemoveMarker==='1'));await remove.fire('click');assert.deepEqual(clone(h.instrument.slicePoints),[0,.5]);assert.equal(h.document.activeElement.id,'sampler-marker-1');
});
(async()=>{let failed=0;for(const {name,fn} of tests){try{await fn();console.log(`PASS ${name}`);}catch(error){failed++;console.error(`FAIL ${name}\n${error.stack}`);}}if(failed)process.exitCode=1;else console.log(`Sampler slicing UI: ${tests.length} groups passed.`);})();
