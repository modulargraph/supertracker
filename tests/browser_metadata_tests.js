'use strict';
// Actual browser module, with an isolated form DOM and bridge. No sample/device operations.
const assert = require('node:assert/strict'), fs = require('node:fs'), vm = require('node:vm');
const web = require('./helpers/web');
const source = web.source('browser.js');
const plain = value => JSON.parse(JSON.stringify(value));
const sample = (n, extra = {}) => ({ name:`Sound ${n}`,path:`/fixture/${n}.wav`,kind:'sample',category:'Drums',tags:['Drums',`Tag ${n}`],key:'C minor',bpm:120,sampleType:'oneshot',inferredMetadata:{category:'Drums',tags:['Drums',`Tag ${n}`],key:'C minor',bpm:120,sampleType:'oneshot'},userMetadata:{},...extra });
const escaped = text => String(text).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;').replaceAll('"','&quot;');
const decode = text => text.replaceAll('&quot;','"').replaceAll('&lt;','<').replaceAll('&gt;','>').replaceAll('&amp;','&');
function harness(items=[sample(1),sample(2),sample(3)]) {
  const nodes=new Map(), calls=[],toasts=[],timers=new Map();let timer=0, modal=null, closes=0, reply, results;
  const state={view:'browser',instrument:0,track:0,project:{instruments:[{name:'Sampler'}],tracks:[{}]},library:{items:plain(items),selected:0,total:items.length}};
  const document={activeElement:null,getElementById:id=>nodes.get(id)||null,querySelector:selector=>selector==='.browser-workspace'?{}:selector==='.modal'?modal:null,querySelectorAll:selector=>selector==='[data-browser-item]'?[...nodes.values()].filter(n=>n.dataset.browserItem!==undefined):[],addEventListener(){}};
  function node(id,tagName='BUTTON',extra={}) {
    const handlers={}; const value={id,tagName,value:'',textContent:'',dataset:{},disabled:false,handlers,addEventListener(name,fn){handlers[name]=fn;},focus(){document.activeElement=this;},select(){},scrollIntoView(){},closest(selector){return selector==='.browser-workspace'||selector==='#browser-results-list'?this.id.startsWith('browser-')?{}:null:null;},fire(name,event={}){return handlers[name]?.({target:this,preventDefault(){},stopPropagation(){},...event});},click(){return this.onclick?this.onclick():this.fire('click');},...extra};nodes.set(id,value);return value;
  }
  function close(){for(const key of [...nodes.keys()])if(key.startsWith('metadata-')||key==='modal-close')nodes.delete(key);modal=null;state.modal=null;closes++;}
  function open(title,body,footer,options){if(modal)close();modal={title,body,footer,options};state.modal=options.kind;
    for(const [,id,attributes,contents]of (body+footer).matchAll(/<select\s+id="([^"]+)"([^>]*)>(.*?)<\/select>/gs)){const options=[...contents.matchAll(/<option value="([^"]+)"([^>]*)>/g)];node(id,'SELECT',{value:decode((options.find(x=>x[2].includes('selected'))||options[0])[1])});}
    for(const [,id,attributes]of (body+footer).matchAll(/<input\s+id="([^"]+)"([^>]*)>/g))node(id,'INPUT',{value:decode(attributes.match(/value="([^"]*)"/)?.[1]||'')});
    for(const [,id]of footer.matchAll(/<(?:button|span)\s+id="([^"]+)"/g))node(id);
    node('modal-close');
  }
  const ST={state,escapeHTML:escaped,refresh(){},toast:(message,type)=>toasts.push({message,type}),openModal:open,closeModal:close,async request(command,payload){calls.push({command,payload:plain(payload??{})});if(command==='library.search')return {items:plain(results || state.library.items),total:(results || state.library.items).length};if(command==='library.metadata.edit'&&reply)return reply(payload);return {};}};
  const context={window:{ST},document,Map,Set,Promise,queueMicrotask(){},setTimeout(fn){const id=++timer;timers.set(id,fn);return id;},clearTimeout(id){timers.delete(id);}};vm.createContext(context);web.load(context, 'browser.js', { text: source });const browser=context.window.STBrowser;
  node('browser-results-list','DIV');node('browser-search','INPUT');node('browser-edit-metadata');
  items.forEach((_,i)=>node(`browser-result-${i}`,'DIV',{dataset:{browserItem:String(i)}}));
  browser.render();browser.bind();
  function key(name,extra={}){let prevented=false;const handled=browser.key({target:nodes.get('browser-results-list'),key:name,code:name===' '?'Space':name,preventDefault(){prevented=true;},...extra});return {handled,prevented};}
  return {browser,state,nodes,calls,toasts,document,node,key,open:()=>nodes.get('browser-edit-metadata').click(),get modal(){return modal;},get closes(){return closes;},set reply(value){reply=value;},set results(value){results=value;},change(field,value){const n=nodes.get('metadata-value-'+field);n.value=value;n.fire(n.tagName==='SELECT'?'change':'input');},mode(field,value){nodes.get('metadata-mode-'+field).value=value;},save:()=>nodes.get('metadata-save').click(),close,replacement(){open('Replacement','', '<button id="metadata-save">Replacement</button>',{kind:'other'});},edits:()=>calls.filter(x=>x.command==='library.metadata.edit'),async tick(){for(const [id,fn]of [...timers]){timers.delete(id);fn();}for(let i=0;i<8;i++)await Promise.resolve();}};
}
(async()=>{
 let h=harness();assert(h.browser.render().includes('aria-multiselectable="true"'));h.key('E');assert.equal(h.modal.options.kind,'libraryMetadata');await h.tick();assert.equal(h.document.activeElement.id,'metadata-value-tags');assert.match(h.modal.body,/Sample and preset files stay unchanged/);await h.save();assert.equal(h.edits().length,0);assert.equal(h.closes,1);
 h=harness([sample(1,{bpm:undefined,inferredMetadata:{bpm:0,key:'',tags:[],category:'Other',sampleType:'unknown'}})]);h.open();assert.match(h.modal.body,/id="metadata-note-bpm">Inferred: Unknown</);h.close();
 console.log('PASS editor keyboard entry, first field focus, unknown inferred tempo and untouched Save no-op');
 h=harness();h.open();h.change('tags',' Warm , pad , Warm ');h.change('key','F# minor');h.change('bpm','172.5');h.change('sampleType','loop');await h.save();assert.deepEqual(h.edits()[0].payload,{paths:['/fixture/1.wav'],changes:{tags:['Warm','pad','Warm'],key:'F# minor',bpm:172.5,sampleType:'loop'}});assert.equal(h.closes,1);assert(h.calls.some(x=>x.command==='library.search'));assert.equal(h.state.project.instruments[0].name,'Sampler');
 console.log('PASS single edit submits only changed fields, refreshes search and leaves project untouched');
 h=harness();h.key('ArrowDown',{shiftKey:true});h.key('ArrowDown',{shiftKey:true});assert.match(h.browser.render(),/3 selected/);h.open();assert.equal(h.modal.title,'Edit 3 sounds');assert.match(h.modal.body,/Mixed values/);h.change('tags','DNB, Break');h.change('bpm','174');await h.save();assert.deepEqual(h.edits()[0].payload,{paths:['/fixture/1.wav','/fixture/2.wav','/fixture/3.wav'],changes:{addTags:['DNB','Break'],bpm:174}});assert(!h.calls.some(x=>x.command==='sample.preview'));
 console.log('PASS Shift-arrow batch selection adds tags without replacing each sound’s existing tags or auditioning');
 h=harness();h.key('a',{metaKey:true});assert.match(h.browser.render(),/3 selected/);h.nodes.get('browser-result-1').fire('click',{metaKey:true});assert.match(h.browser.render(),/2 selected/);h.open();h.mode('tags','set');h.change('tags','Replacement');await h.save();assert.deepEqual(h.edits()[0].payload,{paths:['/fixture/1.wav','/fixture/3.wav'],changes:{tags:['Replacement']}});
 console.log('PASS Cmd-A selects the current page; Cmd-click toggles one sound; explicit Replace tags works');
 h=harness();h.key('ArrowDown',{shiftKey:true});h.results=[sample(0),sample(1),sample(2),sample(3)];await h.browser.search();h.key('ArrowDown',{shiftKey:true});h.open();h.change('category','Break');await h.save();assert.deepEqual(h.edits()[0].payload.paths,['/fixture/1.wav','/fixture/2.wav','/fixture/3.wav']);
 console.log('PASS a background refresh keeps the range anchor by path when rows shift');
 h=harness();h.key('End',{shiftKey:true});assert.match(h.browser.render(),/3 selected/);h.key('Escape');assert(!h.browser.render().includes('3 selected'));h.key('ArrowUp');h.open();assert.equal(h.modal.title,'Edit Sound 2');h.close();h.key('End',{shiftKey:true});h.state.library.query='changed';h.browser.bind();h.browser.key({target:h.nodes.get('browser-search'),key:'e',preventDefault(){}});assert.equal(h.modal,null);h.open();assert.equal(h.modal,null);
 console.log('PASS Escape clears batch selection; plain arrows return to single audition; stale results cannot open editor');
 h=harness();h.open();h.change('key','');h.change('bpm','');h.change('tags','');h.change('sampleType','unknown');await h.save();assert.deepEqual(h.edits()[0].payload.changes,{tags:[],key:'',bpm:0,sampleType:'unknown'});
 h=harness();h.open();h.nodes.get('metadata-restore').click();await h.save();assert.deepEqual(h.edits()[0].payload.changes,{tags:null,category:null,key:null,bpm:null,sampleType:null});
 console.log('PASS explicit unknown/empty values remain distinct from restore-all inference');
 h=harness();h.open();h.change('bpm','1000');await h.save();assert.equal(h.edits().length,0);assert.match(h.nodes.get('metadata-status').textContent,/1–999/);assert.equal(h.document.activeElement.id,'metadata-value-bpm');h.mode('bpm','keep');h.change('category','');await h.save();assert.equal(h.edits().length,0);assert.match(h.nodes.get('metadata-status').textContent,/category/);h.mode('category','infer');h.reply=async()=>{throw Error('Disk is full');};await h.save();assert.equal(h.closes,0);assert.equal(h.nodes.get('metadata-save').disabled,false);assert.equal(h.nodes.get('metadata-mode-category').value,'infer');assert.match(h.nodes.get('metadata-status').textContent,/Disk is full/);h.reply=async()=>({updated:1});await h.save();assert.equal(h.closes,1);
 console.log('PASS validation retains focus/drafts, native failure remains editable and retry succeeds');
 h=harness();h.open();h.change('bpm','');h.nodes.get('metadata-value-bpm').validity={badInput:true};await h.save();assert.equal(h.edits().length,0);assert.match(h.nodes.get('metadata-status').textContent,/Finish entering/);h.nodes.get('metadata-value-bpm').validity.badInput=false;await h.save();assert.equal(h.edits()[0].payload.changes.bpm,0);
 console.log('PASS unfinished native number drafts cannot silently become an unknown tempo');
 let finish;h=harness();h.open();h.change('category','Break');h.reply=()=>new Promise(resolve=>{finish=resolve;});const pending=h.save();assert.equal(h.nodes.get('metadata-save').disabled,true);await h.save();assert.equal(h.edits().length,1);h.replacement();const replacement=h.nodes.get('metadata-save');replacement.disabled=true;finish({updated:1});await pending;assert.equal(h.modal.title,'Replacement');assert.equal(replacement.disabled,true);assert.deepEqual(h.edits()[0].payload.changes,{category:'Break'});
 console.log('PASS in-flight edit uses captured paths/draft and cannot close or re-enable a replacement dialog');
 h=harness([sample(1,{name:'<Bright & soft>',kind:'nks',path:'/fixture/keys.nksf',userMetadata:{tags:['Custom']}})]);h.open();assert(!h.nodes.has('metadata-value-sampleType'));assert(h.modal.body.includes('your override'));h.change('tags','NKS favorite');await h.save();assert.deepEqual(h.edits()[0].payload,{paths:['/fixture/keys.nksf'],changes:{tags:['NKS favorite']}});assert(!h.calls.some(x=>x.command==='nks.load'||x.command==='plugin.load'));
 console.log('PASS NKS metadata edits remain local labels and hide audio-only type');
 h=harness();h.open();h.change('key','Bb major');const enter=h.nodes.get('metadata-value-key').fire('keydown',{key:'Enter'});await enter;await h.tick();assert.equal(h.edits().length,1);assert.equal(h.edits()[0].payload.changes.key,'Bb major');
 console.log('PASS Return from an editor text field saves exactly once');
})().catch(error=>{console.error(error);process.exitCode=1;});
