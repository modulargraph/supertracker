// Production app.js with a deterministic DOM and deferred native bridge; no audio or vendors.
'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const copy=value=>JSON.parse(JSON.stringify(value));
function deferred(){let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};}
async function tick(){for(let n=0;n<40;n++)await Promise.resolve();}
function harness(){
 const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let modal=null,inputs=[],labels=[],timerID=0,native;
 const document={activeElement:null,body:null,querySelector:query,getElementById:id=>nodes.get(id)||null,querySelectorAll:all,createElement:id=>node(id),addEventListener(){},execCommand(){}};
 function node(id,tagName='DIV'){
  const item={id,tagName,dataset:{},type:'',value:'',disabled:false,hidden:false,isConnected:true,tabIndex:0,style:{},textContent:'',classList:{add(){},remove(){},toggle(){},contains(){return false;}},addEventListener(){},setAttribute(){},focus(){document.activeElement=this;},select(){},scrollIntoView(){},setSelectionRange(){},click(){return this.onclick?.();},dispatchEvent(event){this['on'+event.type]?.(event);return true;},querySelector:query,querySelectorAll:all,contains:el=>inputs.includes(el),closest(selector){if(selector.includes('input:not')&&this.tagName==='INPUT'&&this.type!=='range')return this;if(selector==='button,input,select'&&['BUTTON','INPUT','SELECT'].includes(this.tagName))return this;return null;}};
  Object.defineProperty(item,'innerHTML',{get(){return this.html||'';},set(value){this.html=value;if(id==='overlay')installModal(value);}});nodes.set(id,item);return item;
 }
 function query(selector){if(selector==='.modal')return modal;if(selector.startsWith('[data-param-label='))return labels.find(x=>x.dataset.paramLabel===selector.match(/"([^"]+)"/)?.[1])||null;const id=selector.startsWith('#')?selector.slice(1):selector;return nodes.get(id)||node(id);}
 function all(selector){if(selector==='[data-plugin-address]')return inputs;if(selector==='.plugin-param-item')return labels;return [];}
 function installModal(html){
  for(const item of [...inputs,...labels,modal,nodes.get('plugin-param-list')].filter(Boolean))item.isConnected=false;
  for(const id of ['plugin-param-list','plugin-param-search','plugin-preset','plugin-param-status'])nodes.delete(id);
  inputs=[];labels=[];modal=html?node('dialog'):null;if(!html)return;
  for(const match of html.matchAll(/<(input|select|div|p|button)[^>]*\bid="([^"]+)"[^>]*>/g)){
   const [markup,tag,id]=match,item=node(id,tag.toUpperCase());item.type=markup.match(/\btype="([^"]+)"/)?.[1]||'';item.value=markup.match(/\bvalue="([^"]*)"/)?.[1]||'';item.disabled=/\sdisabled(?:\s|>)/.test(markup);
   for(const data of markup.matchAll(/data-([a-z-]+)="([^"]*)"/g))item.dataset[data[1].replace(/-([a-z])/g,(_,x)=>x.toUpperCase())]=data[2];
   if(item.dataset.pluginAddress)inputs.push(item);
  }
  for(const match of html.matchAll(/data-param-label="([^"]+)"/g)){const label=node('label-'+match[1]);label.dataset.paramLabel=match[1];labels.push(label);}
 }
 document.body=node('body','BODY');
 const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(copy(message));Promise.resolve().then(()=>handlers.get(message.command)?.(message.payload)??{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
 const context=vm.createContext({Event:class{constructor(type,fields){this.type=type;Object.assign(this,fields);}},window,document,console,Date,Math,Map,Set,WeakSet,Number,Array,Object,String,JSON,Promise,innerWidth:1280,innerHeight:800,setTimeout(fn,delay=0){timers.set(++timerID,{fn,delay});return timerID;},clearTimeout(id){timers.delete(id);}});
 web.load(context, 'app.js');
 const ST=window.ST,state=ST.state;state.project=ST.demoProject();state.view='instrument';state.engineReady=true;
 const owner=`track:${state.project.tracks[0].routingID}`,target={owner,kind:'instrument',slot:0,instanceID:'instance-B'},plugin={...target,component:'clap:com.example.fixture-b',format:'CLAP',name:'Original fixture',state:'B:.2',parameters:[{address:'17',name:'Cutoff',min:0,max:1,value:.2,normalized:.2,writable:true}],presets:[{number:1,name:'Preset one'}]};
 state.project.audioPlugins=[copy(plugin)];state.project.instruments[0].type='plugin';native=copy(plugin);
 handlers.set('plugin.parameters',()=>copy(native.parameters));
 handlers.set('plugin.parameter',payload=>{assert.equal(payload.instanceID,native.instanceID);native.state='B:'+payload.value;native.parameters[0].value=native.parameters[0].normalized=payload.value;return{display:String(payload.value)};});
 handlers.set('plugin.state',payload=>{assert.equal(payload.instanceID,native.instanceID);return copy(native);});
 handlers.set('plugin.preset',payload=>{assert.equal(payload.instanceID,native.instanceID);native.state='preset:'+payload.number;native.parameters[0].normalized=.9;return copy(native);});
 handlers.set('engine.project',payload=>{native=copy(payload.project.audioPlugins[0]);return{};});
 handlers.set('engine.snapshot',()=>({audioPlugins:[copy(native)]}));
 handlers.set('project.save',payload=>({project:copy(payload.project),path:'/private/fixture.supertracker'}));
 return{ST,state,window,document,calls,handlers,target,plugin,nodes,get input(){return inputs[0];},get native(){return native;},set native(value){native=value;},async open(){await ST.showPluginParameters({owner,kind:'instrument',slot:0});assert.equal(state.modal,'dialog');assert(inputs.length);},async inputValue(value,commit=true){this.input.value=String(value);this.input.oninput();if(commit)return this.input.onchange();},runTimers(delay=35){for(const[id,timer]of [...timers])if(timer.delay===delay){timers.delete(id);timer.fn();}}};
}
const tests=[];function test(name,body){tests.push([name,body]);}
test('pins read, writes and captures to the exact instance and commits one undoable gesture',async()=>{
 const h=harness();await h.open();assert.equal(h.calls.find(x=>x.command==='plugin.parameters').payload.instanceID,'instance-B');
 await h.inputValue(.6);assert.equal(h.state.project.audioPlugins[0].state,'B:0.6');assert.equal(h.state.undo.length,1);assert.equal(h.window.STPluginParameters.hasPendingEdits,false);
 h.document.activeElement=h.input;await h.ST.perform('undo');assert.equal(h.state.project.audioPlugins[0].state,'B:.2');await h.ST.perform('redo');assert.equal(h.state.project.audioPlugins[0].state,'B:0.6');
});
test('late read cannot open a dialog for a replaced project or instance',async()=>{
 for(const change of ['project','instance','slot','track']){
  const h=harness(),gate=deferred();h.handlers.set('plugin.parameters',()=>gate.promise);const opening=h.ST.showPluginParameters(h.target);await tick();
  if(change==='project')h.state.project=copy(h.state.project);else if(change==='instance')h.state.project.audioPlugins[0].instanceID='replacement';else if(change==='slot')h.state.project.audioPlugins[0].slot=1;else h.state.project.tracks[0]={...h.state.project.tracks[0]};
  gate.resolve(h.plugin.parameters);await opening;assert.equal(h.state.modal,null,change);assert.equal(h.calls.filter(x=>x.command==='plugin.parameter').length,0);
 }
});
test('late opaque captures never overwrite replacement projects, slots, instances or detached tracks',async()=>{
 for(const change of ['project','instance','slot','track']){
  const h=harness();await h.open();const gate=deferred();h.handlers.set('plugin.state',()=>gate.promise);const pending=h.inputValue(.6);await tick();
  assert(h.calls.some(x=>x.command==='plugin.state'));h.ST.closeModal();
  if(change==='project')h.state.project=copy(h.state.project);else if(change==='instance')h.state.project.audioPlugins[0].instanceID='replacement';else if(change==='slot')h.state.project.audioPlugins[0].slot=1;else h.state.project.tracks[0]={...h.state.project.tracks[0]};
  h.state.project.audioPlugins[0].state='replacement-preserved';const before=JSON.stringify(h.state.project);gate.resolve(copy(h.native));await pending;
  assert.equal(JSON.stringify(h.state.project),before,change);assert.equal(h.state.undo.length,0,change);
 }
});
test('Undo waits for pending capture, then restores prior state without late reapplication',async()=>{
 const h=harness();await h.open();const gate=deferred();h.handlers.set('plugin.state',()=>gate.promise);const pending=h.inputValue(.6);await tick();h.ST.closeModal();const undo=h.ST.perform('undo');await tick();assert.equal(h.state.undo.length,0);
 gate.resolve(copy(h.native));await pending;await undo;assert.equal(h.state.project.audioPlugins[0].state,'B:.2');assert.equal(h.state.redo.length,1);assert.equal(h.window.STPluginParameters.hasPendingEdits,false);
});
test('Save settles a closed dialog edit before engine capture and file serialization',async()=>{
 const h=harness();await h.open();const gate=deferred();h.handlers.set('plugin.state',()=>gate.promise);const pending=h.inputValue(.6);await tick();h.ST.closeModal();const save=h.ST.perform('save');await tick();assert(!h.calls.some(x=>x.command==='project.save'||x.command==='engine.project'));
 gate.resolve(copy(h.native));await pending;assert.equal(await save,true);assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.audioPlugins[0].state,'B:0.6');assert.equal(h.state.dirty,false);
});
test('strict synchronization flushes an input-only edit without self-deadlock',async()=>{
 const h=harness();await h.open();await h.inputValue(.7,false);assert(h.window.STPluginParameters.hasPendingEdits);
 await h.ST.syncEngineProject(()=>copy(h.state.project));const apply=h.calls.filter(x=>x.command==='engine.project').at(-1);assert.equal(apply.payload.project.audioPlugins[0].state,'B:0.7');assert.equal(h.window.STPluginParameters.hasPendingEdits,false);
});
test('successive edits serialize and never publish the older capture over the final gesture',async()=>{
 const h=harness();await h.open();const gate=deferred();let captures=0;h.handlers.set('plugin.state',()=>++captures===1?gate.promise:copy(h.native));
 const first=h.inputValue(.4);await tick();const second=h.inputValue(.8);await tick();assert.equal(h.calls.filter(x=>x.command==='plugin.parameter').length,1);gate.resolve({...copy(h.native),state:'B:0.4'});await first;await second;
 assert.equal(h.state.project.audioPlugins[0].state,'B:0.8');assert.equal(h.state.undo.length,1);assert.equal(h.state.undo[0].project.audioPlugins[0].state,'B:.2');
});
test('write and capture failures retain desired values for explicit retry and prevent Save/Undo',async()=>{
 for(const fail of ['write','state','malformed','identity']){
  const h=harness();await h.open();const original=h.handlers.get(fail==='write'?'plugin.parameter':'plugin.state');let failing=true;
  h.handlers.set(fail==='write'?'plugin.parameter':'plugin.state',payload=>{if(!failing)return original(payload);if(fail==='malformed')return{};if(fail==='identity')return{...copy(h.native),instanceID:'wrong'};throw Error('Expected '+fail+' failure');});
  await h.inputValue(.6);assert.equal(h.state.undo.length,0);assert(h.window.STPluginParameters.hasPendingEdits);assert.equal(await h.ST.perform('save'),false);assert(!h.calls.some(x=>x.command==='project.save'));
  const before=JSON.stringify(h.state.project);assert.equal(await h.ST.perform('undo'),false);assert.equal(JSON.stringify(h.state.project),before);
  failing=false;assert.equal(await h.window.STPluginParameters.flushEdits(),true);assert.equal(h.state.project.audioPlugins[0].state,'B:0.6');assert.equal(h.state.undo.length,1);assert.equal(h.window.STPluginParameters.hasPendingEdits,false);
 }
});
test('same-instance descriptor refresh remains editable, but a mismatched capture is never committed',async()=>{
 const h=harness();await h.open();h.state.project.audioPlugins=h.state.project.audioPlugins.map(copy);await h.inputValue(.5);assert.equal(h.state.project.audioPlugins[0].state,'B:0.5');assert.equal(h.state.undo.length,1);
});
test('preview writes are coalesced without creating Undo until the final change',async()=>{
 const h=harness();await h.open();await h.inputValue(.3,false);h.runTimers();await tick();assert.equal(h.native.state,'B:0.3');assert.equal(h.state.undo.length,0);assert.equal(h.calls.filter(x=>x.command==='plugin.state').length,0);
 await h.inputValue(.4,false);h.runTimers();await tick();assert.equal(h.native.state,'B:0.4');assert.equal(h.state.undo.length,0);await h.input.onchange();assert.equal(h.state.undo.length,1);assert.equal(h.state.undo[0].project.audioPlugins[0].state,'B:.2');await h.input.onchange();assert.equal(h.state.undo.length,1,'duplicate final change is not another gesture');
});
test('an acknowledged Save preserves the open exact-instance dialog for further edits',async()=>{
 const h=harness();await h.open();await h.inputValue(.6);const before=h.state.project;assert.equal(await h.ST.perform('save'),true);assert.notEqual(h.state.project,before);assert.equal(h.state.modal,'dialog');await h.inputValue(.8);assert.equal(h.state.project.audioPlugins[0].state,'B:0.8');assert.equal(h.state.dirty,true);
});
test('new input during Save acknowledgement stays dirty and prevents stale dialog rebinding',async()=>{
 const h=harness();await h.open();const gate=deferred();let saved;h.handlers.set('project.save',payload=>{saved=copy(payload.project);return gate.promise;});await h.inputValue(.4);const save=h.ST.perform('save');await tick();assert(saved);const owner=h.state.project;await h.inputValue(.8,false);gate.resolve({project:saved});assert.equal(await save,false);assert.equal(h.state.project,owner);assert.equal(h.state.dirty,true);assert(h.window.STPluginParameters.hasPendingEdits);await h.window.STPluginParameters.flushEdits();assert.equal(h.state.project.audioPlugins[0].state,'B:0.8');
});
test('late preset completion cannot close an unrelated modal or resurrect the old project',async()=>{
 const h=harness();await h.open();const gate=deferred();h.handlers.set('plugin.state',()=>gate.promise);h.nodes.get('plugin-preset').value='1';const preset=h.nodes.get('plugin-preset').onchange();await tick();h.ST.closeModal();h.ST.openModal('Unrelated','Other dialog');h.state.project=copy(h.state.project);h.state.project.audioPlugins[0].state='keep-current';gate.resolve(copy(h.native));await preset;
 assert.equal(h.state.modal,'dialog');assert.match(h.nodes.get('overlay').innerHTML,/Unrelated/);assert.equal(h.state.project.audioPlugins[0].state,'keep-current');assert.equal(h.state.undo.length,0);
});
test('keyboard steps: 1%, ⌥ 0.1%, ⇧ 10%, previewed per press and committed as one Undo step',async()=>{
 const h=harness();await h.open();const key=(fields)=>{const event={defaultPrevented:false,preventDefault(){this.defaultPrevented=true;},stopPropagation(){},...fields};h.input.onkeydown(event);return event;};
 assert(key({key:'ArrowUp',altKey:true}).defaultPrevented);assert.equal(h.input.value,'0.201');key({key:'ArrowUp',altKey:true});assert.equal(h.input.value,'0.202');
 key({key:'ArrowUp'});assert.equal(h.input.value,'0.212');key({key:'ArrowDown',shiftKey:true});assert.equal(h.input.value,'0.112');key({key:'PageUp'});assert.equal(h.input.value,'0.212');
 h.runTimers();await tick();assert.equal(h.native.state,'B:0.212','each press previews');assert.equal(h.state.undo.length,0);
 h.input.onkeyup({key:'ArrowUp'});await tick();await tick();assert.equal(h.state.undo.length,1,'one Undo step per burst');assert.equal(h.state.project.audioPlugins[0].state,'B:0.212');
 h.input.value='0.9995';key({key:'ArrowUp',altKey:true});assert.equal(h.input.value,'1');key({key:'ArrowUp'});assert.equal(h.input.value,'1','clamped to the normalized range');
 assert(!key({key:'ArrowUp',metaKey:true}).defaultPrevented);assert(!key({key:'a'}).defaultPrevented);
});
(async()=>{for(const[name,body]of tests){await body();console.log('PASS '+name);}console.log(`Generic plug-in parameter dialog: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
