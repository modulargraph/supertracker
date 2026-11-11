#!/usr/bin/env node
'use strict';
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('routing.js');
const copy = value => JSON.parse(JSON.stringify(value));
const escapeHTML = value => String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const deferred = () => { let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return {promise,resolve,reject}; };
const drain = async () => { for(let i=0;i<15;i++)await Promise.resolve(); };
const waitUntil = async predicate => { for(let i=0;i<100;i++){if(predicate())return;await Promise.resolve();}assert(predicate(),'Asynchronous routing UI did not settle'); };

class Element {
  constructor(id,document){Object.assign(this,{id,document,value:'',checked:false,disabled:false,hidden:false,isConnected:true,dataset:{},listeners:new Map(),attributes:{},children:[],_html:''});}
  addEventListener(type,callback){const list=this.listeners.get(type)||[];list.push(callback);this.listeners.set(type,list);}
  dispatch(type,values={}){const event={target:this,type,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;},...values};for(const callback of this.listeners.get(type)||[])callback(event);return event;}
  focus(){this.document.activeElement=this;}
  scrollIntoView(){this.scrolled=true;}
  setAttribute(key,value){this.attributes[key]=String(value);}
  get innerHTML(){return this._html;}
  set innerHTML(value){this._html=value;this.children=[];for(const match of value.matchAll(/<button\b[^>]*id="([^"]+)"[^>]*data-routing-choice="(\d+)"[^>]*>/g)){const child=new Element(match[1],this.document);child.dataset.routingChoice=match[2];this.children.push(child);this.document.elements.set(child.id,child);}}
  querySelectorAll(selector){if(selector==='[data-routing-choice]')return this.children;return [];}
}

function fixture(){return {name:'Routing test',masterVolume:.7,tracks:[{routingID:'a',name:'Bass',volume:.8},{routingID:'b',name:'Keys',volume:.6}],patterns:[{id:0,automation:[]}],audioPlugins:[{instanceID:'plug-b',owner:'track:b',track:1,kind:'effect',slot:0,name:'Detector'}],routing:{version:1,mixModel:'standard',buses:[{id:'g',name:'Group',type:'group',volume:1,pan:0,mute:false,solo:false},{id:'r',name:'Return',type:'return',volume:1,pan:0,mute:false,solo:false}],outputs:{'track:a':'master','track:b':'master','bus:g':'master','bus:r':'master'},sends:[],sidechains:[]}};}
function capabilities(){return {supported:true,mixModels:['standard'],outputs:true,buses:{group:true,return:true,max:16},sends:{enabled:true,taps:['preFader','postFader'],max:128},sidechains:{enabled:true,taps:['preInserts','preFader','postFader'],inputs:[{instanceID:'plug-b',owner:'track:b',pluginName:'Detector',inputBus:2,layoutSignature:{name:'External detector',channels:2},available:true}]}};}

function harness(p=fixture()){
  const document={elements:new Map(),activeElement:null,getElementById(id){return this.elements.get(id)||null;},querySelectorAll(){return [];}};
  const state={project:p,view:'mixer',track:0,mixerOwner:'track:a',modal:null,native:true};
  const requests=[],mutations=[],notices=[],modals=[];let count=0,refreshes=0,handler=null,captureHandler=null,origin=null;
  const ST={state,escapeHTML,uid:()=>`new-${++count}`,request:async(command,payload={})=>{
    requests.push({command,payload:copy(payload)});if(handler){const result=handler(command,payload);if(result!==undefined)return result;}
    if(command==='routing.capabilities')return capabilities();if(command==='routing.validate')return {valid:true,routing:payload.project.routing,status:{unavailable:[]}};
    if(command==='routing.apply')return {project:copy(payload.project),routing:copy(payload.project.routing),status:{unavailable:[]}};if(command==='engine.project')return {};throw new Error(`Unexpected ${command}`);
  },captureNativeEdits:async()=>captureHandler?captureHandler():true,mutate:(label,fn,options)=>{
    assert(options?.routingCommit,'Structural commit must carry the parent bypass flag');assert(context.window.STRouting.isApplying);
    mutations.push({label,before:copy(state.project),options});fn(state.project);refreshes++;
  },refresh:()=>{refreshes++;},toast:(message,type)=>notices.push({message,type}),setView:view=>{state.view=view;},openModal:(title,body,footer='',options={})=>{
    origin=document.activeElement;state.modal=options.kind||'dialog';modals.push({title,body,footer});
    for(const match of `${body}${footer}`.matchAll(/<(?:input|button|div)\b[^>]*\bid="([^"]+)"[^>]*>/g)){const element=new Element(match[1],document),value=match[0].match(/\bvalue="([^"]*)"/);if(value)element.value=value[1];document.elements.set(element.id,element);}
  },closeModal:()=>{state.modal=null;origin?.focus();}};
  ST.syncEngineProject=async(makeProject,options={})=>{const project=makeProject();return project?ST.request('engine.project',{project,awaitPlugins:true,...(options.replace?{replace:true}:{})}):false;};
  const context={window:{ST,STStudio:{isEditingPlugin:false,isMovingEffect:false}},document,queueMicrotask};web.runText('routing.js', source, context,{filename:'routing.js'});
  const R=context.window.STRouting;R.event('routing.capabilities',capabilities());
  return {R,ST,state,document,requests,mutations,notices,modals,context,get refreshes(){return refreshes;},handle:fn=>{handler=fn;},capture:fn=>{captureHandler=fn;},element:id=>document.getElementById(id),focus:id=>{const element=new Element(id,document);document.elements.set(id,element);element.focus();return element;}};
}

const tests=[];function test(name,run){tests.push({name,run});}

function sidechainControls(h,plugin,route){
  const section=new Element('key-section',h.document),add=new Element('key-add',h.document),rows=[];
  section.dataset.routingPlugin=plugin.instanceID;section.dataset.routingPluginOwner=plugin.owner;add.dataset.routingKeyAction='add';
  section.querySelector=selector=>selector==='[data-routing-key-action="add"]'?add:null;section.querySelectorAll=selector=>selector==='[data-routing-key]'?rows:[];
  let tap,sourceButton;
  if(route){const row=new Element('key-row',h.document);row.dataset.routingKey=route.id;tap=new Element('key-tap',h.document);tap.dataset.routingKeyField='tap';tap.value=route.tap;sourceButton=new Element('key-source',h.document);sourceButton.dataset.routingKeyAction='source';row.querySelectorAll=selector=>selector==='[data-routing-key-field]'?[tap]:selector==='[data-routing-key-action]'?[sourceButton]:[];rows.push(row);}
  h.document.querySelectorAll=selector=>selector==='[data-routing-plugin]'?[section]:[];h.R.bindPluginSidechains(plugin.owner);return {section,add,tap,sourceButton};
}
async function chooseRouting(h,query=''){
  const input=h.element('routing-search');assert(input,'Expected searchable routing chooser');input.value=query;input.dispatch('input');assert(!h.element('routing-results').innerHTML.includes('No matching destinations'),`No choice matching ${query}`);input.dispatch('keydown',{key:'Enter'});await drain();
}
function sidechainRoute(instanceID='plug-b',source='track:a',tap='preInserts',name=''){
  return {id:'key-1',source,tap,gain:1,enabled:true,destination:{instanceID,inputBus:2,layoutSignature:{name,channels:2}}};
}
function statusNodes(h){
  const writes={text:0,hidden:0};
  const note=(id,dataset)=>{const element=new Element(id,h.document);element.dataset=dataset;let text='',hidden=true;Object.defineProperty(element,'textContent',{get:()=>text,set:value=>{text=String(value);writes.text++;}});Object.defineProperty(element,'hidden',{get:()=>hidden,set:value=>{hidden=!!value;writes.hidden++;}});h.document.elements.set(id,element);return element;};
  const issues=[note('routing-send-issue',{routingIssue:'send-1'}),note('routing-key-issue',{routingIssue:'key-1'}),note('routing-unrelated-issue',{routingIssue:'unrelated'})];
  h.document.querySelectorAll=selector=>selector==='[data-routing-issue]'?issues:[];
  const draft=h.focus('routing-send-gain-draft');draft.value='0.';draft.selectionStart=2;draft.selectionEnd=2;
  return {issues,draft,writes,count:()=>writes.text+writes.hidden};
}

test('master stays a sink and routing choices exclude known feedback paths',()=>{
  const p=fixture();p.routing.outputs['track:a']='bus:g';p.routing.outputs['bus:g']='bus:r';const h=harness(p);
  assert.deepEqual(copy(h.R.destinationOptions('master')),[]);assert(!h.R.canOutput('master'));assert(!h.R.canSend('master'));
  const destinations=h.R.destinationOptions('bus:r',p,{output:true}).map(item=>item.owner);assert(!destinations.includes('bus:g'));assert(destinations.includes('master'));
  const sources=h.R.sourceOptions('bus:g').map(item=>item.owner);assert(!sources.includes('master'));assert(!sources.includes('bus:g'));assert(!sources.includes('bus:r'));assert(sources.includes('track:b'));
});

test('capability flags hide unsupported actions while retaining saved state summaries',()=>{
  const h=harness();h.R.event('routing.capabilities',{supported:false,mixModels:[],buses:{},sends:{},sidechains:{}});
  const html=h.R.render('track:a');assert(html.includes('not supported'));assert(!html.includes('data-routing-action="output"'));assert(!html.includes('data-routing-action="add-send"'));assert(html.includes('<fieldset disabled>'));
  assert(!h.R.available());assert(!h.R.canOutput('track:a'));const exported=h.R.capabilities;exported.supported=true;assert(!h.R.available(),'Capability getter must not expose mutable internal state');
});

test('validate then apply commits the authoritative native project once with one undo record',async()=>{
  const h=harness(),original=h.state.project;
  h.handle((command,payload)=>command==='routing.apply'?{project:{...copy(payload.project),nativeRoutingRevision:7},routing:payload.project.routing,status:{latencyFrames:64}}:undefined);
  await h.R.transaction('Route bass',p=>{p.routing.outputs['track:a']='bus:g';},{owner:'track:a'});
  assert.equal(h.state.project,original,'Commit should preserve the root project object');assert.equal(h.state.project.routing.outputs['track:a'],'bus:g');assert.equal(h.state.project.nativeRoutingRevision,7);
  assert.equal(h.mutations.length,1);assert.equal(h.mutations[0].before.routing.outputs['track:a'],'master');assert.equal(h.mutations[0].options.routingCommit,true);
  assert.deepEqual(h.requests.filter(item=>item.command!=='routing.capabilities').map(item=>item.command),['engine.project','routing.validate','routing.apply']);assert(!h.R.isApplying);
});

function capabilityFocusFixture(){
  const h=harness(),id='routing-output-track_a',body=new Element('body',h.document);
  h.document.body=body;h.focus(id);
  h.ST.refresh=()=>{
    const old=h.element(id);if(old){old.isConnected=false;h.document.elements.delete(id);if(h.document.activeElement===old)h.document.activeElement=body;}
    if(h.R.canOutput('track:a')&&!h.R.isApplying)h.document.elements.set(id,new Element(id,h.document));
  };
  return {...h,id,body};
}

test('committed routing restores its control only after asynchronous capabilities recreate it',async()=>{
  const h=capabilityFocusFixture(),reply=deferred();h.handle(command=>command==='routing.capabilities'?reply.promise:undefined);
  const pending=h.R.transaction('Route',p=>{p.routing.outputs['track:a']='bus:g';},{owner:'track:a',focusID:h.id});
  await waitUntil(()=>h.requests.some(x=>x.command==='routing.capabilities'));
  assert.equal(h.document.activeElement,h.body);assert.equal(h.element(h.id),null);
  reply.resolve(capabilities());await pending;await drain();
  assert.equal(h.document.activeElement,h.element(h.id));assert.equal(h.document.activeElement.id,h.id);
});

test('capability focus return does not steal a newer field, channel, view, modal or project',async()=>{
  for(const destination of ['field','channel','view','modal','project']){
    const h=capabilityFocusFixture(),reply=deferred();h.handle(command=>command==='routing.capabilities'?reply.promise:undefined);
    const pending=h.R.transaction('Route',p=>{p.routing.outputs['track:a']='bus:g';},{owner:'track:a',focusID:h.id});
    await waitUntil(()=>h.requests.some(x=>x.command==='routing.capabilities'));
    const newer=h.focus('newer-user-control');
    if(destination==='channel')h.state.mixerOwner='track:b';
    if(destination==='view')h.state.view='instrument';
    if(destination==='modal')h.state.modal='palette';
    if(destination==='project')h.state.project=fixture();
    reply.resolve(capabilities());await pending;await drain();
    assert.equal(h.document.activeElement,newer,destination);
  }
});

test('routing focus waits past an obsolete capability request for the committed generation',async()=>{
  const h=capabilityFocusFixture(),old=deferred(),fresh=deferred();let calls=0;
  h.handle(command=>command==='routing.capabilities'?(++calls===1?old.promise:fresh.promise):undefined);
  const oldLoad=h.R.refreshCapabilities();
  const pending=h.R.transaction('Route',p=>{p.routing.outputs['track:a']='bus:g';},{owner:'track:a',focusID:h.id});
  await waitUntil(()=>h.mutations.length===1&&!h.R.isApplying);
  old.resolve(capabilities());await oldLoad;await waitUntil(()=>calls===2);
  assert.equal(h.document.activeElement,h.body);
  fresh.resolve(capabilities());await pending;await drain();assert.equal(h.document.activeElement?.id,h.id);
});

test('validation and native application failures preserve project and undo history',async()=>{
  for(const failure of ['routing.validate','routing.apply']){const h=harness(),before=copy(h.state.project);h.handle(command=>{if(command===failure)throw new Error('Native graph rejected');});
    await assert.rejects(h.R.transaction('Bad route',p=>{p.routing.outputs['track:a']='bus:g';}),/Native graph rejected/);assert.deepEqual(h.state.project,before);assert.equal(h.mutations.length,0);assert(!h.R.isApplying);
    assert.equal(h.requests.filter(item=>item.command==='engine.project'&&item.payload.replace).length,0,'Native rejection is transactional and should not trigger a guessed rollback');
  }
});

test('a routing change refuses failed capture, a replaced project and a plug-in draft arriving while the project is synchronized',async()=>{
  for(const change of ['capture-failure','owner','mapped-draft']){
    const h=harness(),waiting=deferred();
    if(change==='capture-failure')h.capture(()=>false);
    h.handle(command=>command==='engine.project'?waiting.promise:undefined);
    const pending=h.R.transaction('Reroute',p=>{p.routing.outputs['bus:g']='master';});
    if(change==='capture-failure')await assert.rejects(pending,/could not be captured/);
    else{
      await waitUntil(()=>h.requests.some(item=>item.command==='engine.project'));
      if(change==='owner')h.state.project=fixture();
      if(change==='mapped-draft')h.context.window.STNKSControls={editGeneration:1,hasPendingEdits:true};
      waiting.resolve({});await assert.rejects(pending,/changed/);
    }
    assert.equal(h.mutations.length,0);assert(!h.requests.some(item=>item.command==='routing.apply'));assert(!h.R.isApplying);
  }
});

test('in-flight gate rejects a second change and waitForApply resolves on completion',async()=>{
  const h=harness(),waiting=deferred();h.handle(command=>command==='routing.apply'?waiting.promise:undefined);
  const first=h.R.transaction('Route',p=>{p.routing.outputs['track:a']='bus:g';});await drain();assert(h.R.isApplying);
  let released=false;h.R.waitForApply().then(()=>released=true);await assert.rejects(h.R.transaction('Other',()=>{}),/already being applied/);assert(!released);
  const submitted=h.requests.find(item=>item.command==='routing.apply').payload.project;waiting.resolve({project:submitted});await first;await drain();assert(released);assert(!h.R.isApplying);assert.equal(h.mutations.length,1);
});

test('project replacement after native success restores latest audio without overwriting UI',async()=>{
  const h=harness(),waiting=deferred();h.handle(command=>command==='routing.apply'?waiting.promise:undefined);
  const pending=h.R.transaction('Old route',p=>{p.routing.outputs['track:a']='bus:g';});await drain();const oldDraft=h.requests.find(item=>item.command==='routing.apply').payload.project;
  const replacement=fixture();replacement.name='Replacement session';h.state.project=replacement;waiting.resolve({project:oldDraft});await assert.rejects(pending,/project changed while routing/i);
  assert.equal(h.state.project,replacement);assert.equal(h.mutations.length,0);const restored=h.requests.find(item=>item.command==='engine.project'&&item.payload.replace);assert.equal(restored.payload.project.name,'Replacement session');assert.equal(restored.payload.replace,true);
});

test('same-object concurrent edits are preserved and validation cannot commit stale drafts',async()=>{
  const h=harness(),waiting=deferred();h.handle(command=>command==='routing.validate'?waiting.promise:undefined);
  const pending=h.R.transaction('Route',p=>{p.routing.outputs['track:a']='bus:g';},{owner:'track:a'});await drain();h.state.project.tracks[0].volume=.42;waiting.resolve({valid:true});
  await assert.rejects(pending,/project changed before routing/i);assert.equal(h.state.project.tracks[0].volume,.42);assert.equal(h.requests.filter(item=>item.command==='routing.apply').length,0);assert.equal(h.mutations.length,0);
});

test('plug-in capture occurs before the routing gate and active rack edits block transactions',async()=>{
  const h=harness(),order=[];h.capture(()=>{assert(!h.R.isApplying);order.push('capture');h.state.project.audioPlugins[0].state='fresh-native-state';return true;});
  const sync=h.ST.syncEngineProject;h.ST.syncEngineProject=async makeProject=>{assert(!h.R.isApplying);order.push('flush');return sync(makeProject);};
  h.handle(command=>{if(command==='routing.validate')order.push('validate');});
  await h.R.transaction('Route',()=>{});assert.equal(h.requests[0].payload.project.audioPlugins[0].state,'fresh-native-state');assert.equal(h.mutations[0].before.audioPlugins[0].state,'fresh-native-state');
  assert.deepEqual(order,['capture','flush','validate']);await h.R.refreshCapabilities();
  h.context.window.STStudio.isEditingPlugin=true;await assert.rejects(h.R.transaction('Blocked',()=>{}),/Finish the plug-in rack/);assert.equal(h.mutations.length,1);
});

test('send gain exposes owner automation metadata and sidechain layouts are never retargeted silently',()=>{
  const p=fixture();p.routing.sends=[{id:'s1',source:'track:a',destination:'bus:r',tap:'preFader',gain:.5,enabled:true}];p.routing.sidechains=[{id:'k1',source:'track:a',tap:'postFader',destination:{instanceID:'plug-b',inputBus:2,layoutSignature:{name:'Old detector',channels:1}},gain:1,enabled:true}];
  const h=harness(p),before=copy(p);const sends=h.R.render('track:a');assert(sends.includes('data-owner="track:a" data-automate="send:s1"'));assert(sends.includes('max="2"'));const keys=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(keys.includes('saved input layout is unavailable'));assert(keys.includes('Old detector'));assert.deepEqual(p,before);
});

test('search chooser supports filtering, arrows, Return, Escape and focus return',async()=>{
  const h=harness(),origin=h.focus('strip-output-track_a');await h.R.showOutput('track:a',origin.id);const search=h.element('routing-search');assert.equal(h.document.activeElement,search);
  assert(h.element('routing-results').innerHTML.includes('tabindex="-1"'),'Options retain keyboard navigation in the combobox instead of creating mismatched Tab focus');
  search.value='group';search.dispatch('input');assert(h.element('routing-results').innerHTML.includes('Group'));assert(!h.element('routing-results').innerHTML.includes('Return'));
  search.dispatch('keydown',{key:'ArrowDown'});const entered=search.dispatch('keydown',{key:'Enter'});assert(entered.prevented&&entered.stopped);await drain();assert.equal(h.state.project.routing.outputs['track:a'],'bus:g');assert.equal(h.document.activeElement.id,origin.id);
  await h.R.showOutput('track:a',origin.id);h.element('routing-search').dispatch('keydown',{key:'Escape'});await drain();assert.equal(h.state.modal,null);assert.equal(h.document.activeElement.id,origin.id);
});

test('group creation leaves track assignment opt-in and removal reparents children safely',async()=>{
  const h=harness(),focused=[];h.context.window.STStudio.selectOwner=owner=>{h.state.mixerOwner=owner;};h.context.window.STStudio.focusSelectedChannel=()=>focused.push(h.state.mixerOwner);
  await h.R.createBus('group',{assignOwner:'track:a'});assert.equal(h.element('routing-bus-assign').checked,false);h.element('routing-bus-name').value='Drum group';h.element('routing-bus-create').dispatch('click');await drain();
  const created=h.state.project.routing.buses.find(bus=>bus.name==='Drum group');assert(created);assert.equal(h.state.project.routing.outputs['track:a'],'master');assert.equal(h.state.project.routing.outputs[`bus:${created.id}`],'master');
  await waitUntil(()=>focused.includes(`bus:${created.id}`));assert.equal(h.state.mixerOwner,`bus:${created.id}`);
  h.state.project.routing.outputs['track:a']=`bus:${created.id}`;h.state.project.routing.outputs[`bus:${created.id}`]='bus:g';h.state.project.routing.sends.push({id:'from-group',source:`bus:${created.id}`,destination:'bus:r',gain:1,enabled:true,tap:'postFader'},{id:'into-group',source:'track:b',destination:`bus:${created.id}`,gain:1,enabled:true,tap:'postFader'});
  h.state.project.patterns[0].automation=[{owner:'track:b',target:'send:into-group',points:[{row:0,value:.5}]},{owner:`bus:${created.id}`,target:'send:from-group',points:[{row:0,value:.2}]},{owner:'track:b',target:'volume',points:[{row:0,value:.8}]}];
  h.R.removeBus(`bus:${created.id}`);h.element('routing-delete-apply').dispatch('click');await waitUntil(()=>focused.includes('bus:g'));assert.equal(h.state.project.routing.outputs['track:a'],'bus:g');assert(!h.state.project.routing.buses.some(bus=>bus.id===created.id));assert(!h.state.project.routing.sends.some(send=>send.id==='from-group'));assert.deepEqual(copy(h.state.project.patterns[0].automation.map(lane=>lane.target)),['volume']);assert.equal(h.state.mixerOwner,'bus:g');
});

test('capability replies from an obsolete plug-in generation are discarded and refreshed',async()=>{
  const h=harness(),old=deferred(),fresh=capabilities(),seen=[];fresh.sidechains.inputs[0].layoutSignature.name='Current detector';let count=0;
  h.handle(command=>command==='routing.capabilities'?(++count===1?old.promise:fresh):undefined);
  h.ST.refresh=()=>{const value=h.R.capabilities;if(value)seen.push(value.sidechains.inputs[0]?.layoutSignature.name);};
  const pending=h.R.refreshCapabilities();h.R.event('plugins.changed');old.resolve(capabilities());await pending;await drain();
  assert.equal(count,2);assert.equal(h.R.capabilities.sidechains.inputs[0].layoutSignature.name,'Current detector');assert(!seen.includes('External detector'),'An obsolete reply must never be published to the selector');
});

test('routing editors opened across a capability wait reject replaced projects or owner entities',async()=>{
  for(const method of ['showOutput','showAddSend','createBus']){
    const h=harness(),waiting=deferred();h.R.event('project.loaded');h.handle(command=>command==='routing.capabilities'?waiting.promise:undefined);
    const opened=method==='createBus'?h.R.createBus('group',{assignOwner:'track:a'}):h.R[method]('track:a');
    if(method==='showAddSend')h.state.project.tracks[0]={...h.state.project.tracks[0]};else{h.state.project=fixture();h.R.event('project.loaded');}
    waiting.resolve(capabilities());await opened;await drain();assert.equal(h.modals.length,0,method);assert(h.notices.some(notice=>notice.message.includes('selected channel changed')),method);
  }
});

test('return creation never offers dry-output assignment',async()=>{
  const p=fixture(),output=p.routing.outputs['track:a'];const h=harness(p);await h.R.createBus('return',{assignOwner:'track:a'});
  assert.equal(h.state.modal,'routing-bus');assert.equal(h.modals.at(-1).title,'Create return bus');assert(!h.modals.at(-1).body.includes('routing-bus-assign'));
  h.element('routing-bus-name').value='Tape return';h.element('routing-bus-create').dispatch('click');await drain();
  assert(h.state.project.routing.buses.some(bus=>bus.type==='return'&&bus.name==='Tape return'));assert.equal(h.state.project.routing.outputs['track:a'],output,'The dry output is not moved to a return');assert.equal(h.mutations.length,1);
});

test('the routing command renders the selected owner before focusing its output control',()=>{
  const h=harness(),events=[];h.ST.setView=view=>{h.state.view=view;events.push('view');h.ST.refresh();};
  h.context.window.STStudio.selectOwner=owner=>{h.state.mixerOwner=owner;events.push(`select:${owner}`);};
  h.ST.refresh=()=>{events.push(`render:${h.state.mixerOwner}`);h.focus(`routing-output-${h.state.mixerOwner.replace(':','_')}`);};
  h.R.show('bus:g');assert.deepEqual(events,['view','render:track:a','select:bus:g','render:bus:g']);assert.equal(h.document.activeElement.id,'routing-output-bus_g');
});

test('removing a send deletes only its exact automation target across all patterns in the same undo',async()=>{
  const p=fixture();p.routing.sends=[{id:'send-1',source:'track:a',destination:'bus:r',gain:1,tap:'postFader',enabled:true}];
  p.patterns=[{id:0,automation:[{owner:'track:a',target:'send:send-1'},{owner:'track:a',target:'send:send-10'}]},{id:1,automation:[{owner:'track:a',target:'send:send-1'},{owner:'track:a',target:'volume'}]}];
  const h=harness(p),panel=new Element('panel',h.document),row=new Element('row',h.document),remove=new Element('remove',h.document);
  panel.dataset.routingOwner='track:a';row.dataset.routingSend='send-1';remove.dataset.routingSendAction='remove';
  panel.querySelectorAll=selector=>selector==='[data-routing-send]'?[row]:[];row.querySelectorAll=selector=>selector==='[data-routing-send-action]'?[remove]:[];
  h.document.querySelectorAll=selector=>selector==='[data-routing-owner]'?[panel]:[];h.R.bind('track:a');remove.dispatch('click');await drain();
  assert.equal(h.state.project.routing.sends.length,0);assert.deepEqual(copy(h.state.project.patterns.map(pattern=>pattern.automation.map(lane=>lane.target))),[['send:send-10'],['volume']]);assert.equal(h.mutations.length,1);assert.equal(h.mutations[0].before.routing.sends.length,1);assert.equal(h.mutations[0].before.patterns[0].automation.length,2);
});

test('advertised tap choices are respected without relabelling saved unsupported taps',()=>{
  const p=fixture();p.routing.sends=[{id:'s1',source:'track:a',destination:'bus:r',gain:1,tap:'preFader',enabled:true}];const h=harness(p),caps=capabilities();caps.sends.taps=['postFader'];h.R.event('routing.capabilities',caps);
  const markup=h.R.render('track:a');assert(markup.includes('value="preFader" selected disabled>Pre-fader · unavailable'));assert(markup.includes('value="postFader"'));assert.equal(h.state.project.routing.sends[0].tap,'preFader');
});

test('unnamed auxiliary inputs remain available, show their actual bus index and retain an exact empty-name signature',async()=>{
  const p=fixture(),caps=capabilities();caps.sidechains.inputs[0].layoutSignature.name='';const h=harness(p);h.R.event('routing.capabilities',caps);
  p.routing.sidechains=[sidechainRoute()];const saved=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(saved.includes('Input 2'));assert(!saved.includes('saved input layout is unavailable'));assert(saved.includes('data-routing-key-action="add"'));assert.equal(p.routing.sidechains[0].destination.layoutSignature.name,'');
  p.routing.sidechains=[];const {add}=sidechainControls(h,p.audioPlugins[0]);add.dispatch('click');assert.equal(h.modals.at(-1).title,'Sidechain input');assert(h.element('routing-results').innerHTML.includes('Input 2'));assert(!h.element('routing-results').innerHTML.includes('Input 0'));
  await chooseRouting(h,'Input 2');assert.equal(h.modals.at(-1).title,'Sidechain tap');await chooseRouting(h,'pre-insert');assert.equal(h.modals.at(-1).title,'Sidechain source');await chooseRouting(h,'Keys');
  assert.equal(h.mutations.length,1);const route=h.state.project.routing.sidechains[0];assert.equal(route.source,'track:b');assert.equal(route.tap,'preInserts');assert.deepEqual(copy(route.destination),{instanceID:'plug-b',inputBus:2,layoutSignature:{name:'',channels:2}});
  assert.deepEqual(h.requests.find(request=>request.command==='routing.apply').payload.project.routing.sidechains[0].destination,{instanceID:'plug-b',inputBus:2,layoutSignature:{name:'',channels:2}});
});

test('sidechain taps use their own capability list and preserve unsupported saved taps without borrowing send choices',()=>{
  const p=fixture(),caps=capabilities();p.routing.sends=[{id:'s1',source:'track:a',destination:'bus:r',tap:'preFader',gain:.5,enabled:true}];p.routing.sidechains=[sidechainRoute('plug-b','track:a','preInserts','External detector')];const h=harness(p);h.R.event('routing.capabilities',caps);
  const sends=h.R.render('track:a'),keys=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(!sends.includes('value="preInserts"'));assert(keys.includes('value="preInserts" selected'));assert(keys.includes('value="preFader"'));assert(keys.includes('value="postFader"'));assert(keys.includes('Pre-insert'));
  caps.sends.taps=['postFader'];caps.sidechains.taps=['preInserts'];p.routing.sidechains[0].tap='preFader';h.R.event('routing.capabilities',caps);const restricted=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(restricted.includes('value="preFader" selected disabled'));assert(restricted.includes('value="preInserts"'));assert(!restricted.includes('value="postFader"'));assert.equal(p.routing.sidechains[0].tap,'preFader');
  delete caps.sidechains.taps;h.R.event('routing.capabilities',caps);const unsupported=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(!unsupported.includes('value="postFader"'),'Send taps must not become implicit sidechain taps');assert(!unsupported.includes('data-routing-key-action="add"'),'No advertised key taps means no key-creation action');
});

test('same-owner source choices allow only advertised pre-insert track or bus effects and always exclude Master',()=>{
  const h=harness(),caps=capabilities();h.R.event('routing.capabilities',caps);
  for(const owner of ['track:b','bus:g','master'])for(const kind of ['effect','instrument'])for(const tap of ['preInserts','preFader','postFader']){
    const plugin={instanceID:'matrix',owner,kind,slot:0},options=h.R.sourceOptions(owner,h.state.project,undefined,{tap,plugin});
    assert.equal(options.some(option=>option.owner===owner),owner!=='master'&&kind==='effect'&&tap==='preInserts',`${owner} ${kind} ${tap}`);assert(!options.some(option=>option.owner==='master'));
  }
  assert(!h.R.sourceOptions('track:b').some(option=>option.owner==='track:b'),'Default source picker must not silently permit self-routing');
  assert(!h.R.sourceOptions('track:b',h.state.project,undefined,{tap:'preInserts',plugin:{owner:'track:a',kind:'effect'}}).some(option=>option.owner==='track:b'));
  caps.sidechains.taps=['postFader'];h.R.event('routing.capabilities',caps);assert(!h.R.sourceOptions('track:b',h.state.project,undefined,{tap:'preInserts',plugin:h.state.project.audioPlugins[0]}).some(option=>option.owner==='track:b'));
});

test('same-owner pre-insert keys restrict tap edits while absent auxiliary metadata never guesses an input',()=>{
  const p=fixture(),caps=capabilities();p.routing.sidechains=[sidechainRoute('plug-b','track:b','preInserts','External detector')];const h=harness(p);h.R.event('routing.capabilities',caps);
  const keys=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(keys.includes('value="preInserts" selected'));assert(!keys.includes('value="preFader"'));assert(!keys.includes('value="postFader"'));
  p.routing.sidechains=[];caps.sidechains.inputs=[];h.R.event('routing.capabilities',caps);assert.equal(h.R.renderPluginSidechain(p.audioPlugins[0]),'');
  caps.sidechains.inputs=[{instanceID:'plug-b',owner:'track:b',inputBus:2,available:false,layoutSignature:{name:'',channels:2}}];h.R.event('routing.capabilities',caps);assert.equal(h.R.renderPluginSidechain(p.audioPlugins[0]),'');assert.equal(h.requests.length,0);assert.equal(h.mutations.length,0);
});

test('native cycle rejection from a sidechain chooser preserves project and undo without applying a guessed graph',async()=>{
  const h=harness(),before=copy(h.state.project),{add}=sidechainControls(h,h.state.project.audioPlugins[0]);h.handle(command=>command==='routing.validate'?{valid:false,error:'Sidechain dependency would create a cycle'}:undefined);
  add.dispatch('click');await chooseRouting(h);await chooseRouting(h,'pre-insert');await chooseRouting(h,'Bass');
  assert.deepEqual(h.state.project,before);assert.equal(h.mutations.length,0);assert.equal(h.requests.filter(request=>request.command==='routing.validate').length,1);assert(!h.requests.some(request=>request.command==='routing.apply'||request.command==='engine.project'&&request.payload.replace));assert(h.notices.some(notice=>/cycle/.test(notice.message)));assert(!h.R.isApplying);
});

test('connections the host could not make are noted on their rows without rebuilding a focused numeric draft',()=>{
  const p=fixture();p.routing.sends=[{id:'send-1',source:'track:a',destination:'bus:r',gain:.5,tap:'postFader',enabled:true}];p.routing.sidechains=[sidechainRoute('plug-b','track:a','postFader','External detector')];
  const h=harness(p),dom=statusNodes(h),before=copy(p),refreshes=h.refreshes;
  h.R.event('status',{seconds:4,trackPeaks:[.2,.5],routing:{standard:true,strips:[],unavailableSidechains:[{id:'send-1',reason:'Return output is missing'},{id:'key-1',reason:'Detector input <is> unavailable'}]}});
  assert.equal(dom.issues[0].hidden,false);assert.equal(dom.issues[0].textContent,'Return output is missing');assert.equal(dom.issues[1].hidden,false);assert.equal(dom.issues[1].textContent,'Detector input <is> unavailable');assert.equal(dom.issues[2].hidden,true);
  const channel=h.R.render('track:a'),insert=h.R.renderPluginSidechain(p.audioPlugins[0]);assert(!channel.includes('data-routing-error'));assert(channel.includes('data-routing-issue="send-1"'));assert(channel.includes('Return output is missing'));assert(insert.includes('data-routing-issue="key-1"'));assert(insert.includes('Detector input &lt;is&gt; unavailable'));
  assert.equal(h.refreshes,refreshes);assert.equal(h.document.activeElement,dom.draft);assert.equal(dom.draft.value,'0.');assert.equal(dom.draft.selectionStart,2);assert.equal(dom.draft.selectionEnd,2);assert.deepEqual(p,before);assert.equal(h.mutations.length,0);
});

test('unchanged connection notes ignore changing transport, peak and latency telemetry without DOM writes',()=>{
  const h=harness(),dom=statusNodes(h),routing={unavailableSidechains:[{id:'send-1',reason:'Send destination unavailable'},{id:'key-1',reason:'Key input unavailable'}]};
  h.R.event('status',{seconds:1,peakL:.2,routing:{...routing,latencyFrames:64}});const writes=dom.count(),refreshes=h.refreshes;
  for(let i=0;i<8;i++)h.R.event('status',{seconds:100+i,peakL:i/10,trackPeaks:[i/10,.8],latencyCompensation:{maximumFrames:128+i},routing:{...copy(routing),unavailableSidechains:i%2?copy(routing.unavailableSidechains).reverse():copy(routing.unavailableSidechains),latencyFrames:256+i,inserts:[{slot:i}]}});
  h.R.event('status',{seconds:500,trackPeaks:[1,1]});
  assert.equal(dom.count(),writes);assert.equal(h.refreshes,refreshes);assert.equal(h.document.activeElement,dom.draft);assert.equal(dom.draft.value,'0.');assert.equal(dom.issues[0].textContent,'Send destination unavailable');
});

test('changed, cleared and malformed connection reports update the notes in place and retain control identity',()=>{
  const h=harness(),dom=statusNodes(h),refreshes=h.refreshes;
  h.R.event('status',{routing:{unavailableSidechains:[{id:'send-1',reason:'Old send issue'}]}});
  h.R.event('status',{routing:{unavailableSidechains:[{id:'key-1',reason:'New key issue'}]}});
  assert.equal(dom.issues[0].hidden,true);assert.equal(dom.issues[0].textContent,'');assert.equal(dom.issues[1].hidden,false);assert.equal(dom.issues[1].textContent,'New key issue');
  for(const unavailableSidechains of [null,undefined,{},'invalid',42,[null,{id:7,reason:'x'},{id:'key-1'}]]){
    h.R.event('status',{routing:{unavailableSidechains:[{id:'key-1',reason:'Again'}]}});assert.equal(dom.issues[1].textContent,'Again');
    assert.doesNotThrow(()=>h.R.event('status',{routing:{unavailableSidechains}}));assert.doesNotThrow(()=>h.R.render('track:a'));
    assert(dom.issues.every(issue=>issue.hidden&&issue.textContent===''));
  }
  h.R.event('status',{routing:{unavailableSidechains:[{id:'key-1',reason:'Back'}]}});h.R.event('status',{routing:null});assert(dom.issues.every(issue=>issue.hidden&&issue.textContent===''));
  assert.equal(h.refreshes,refreshes);assert.equal(h.document.activeElement,dom.draft);assert.equal(h.element(dom.draft.id),dom.draft);assert.equal(dom.draft.value,'0.');assert.equal(dom.draft.selectionStart,2);assert.equal(h.mutations.length,0);
});

test('an emptied send gain field keeps the stored gain; a typed gain is applied',async()=>{
  const p=fixture();p.routing.sends=[{id:'send-1',source:'track:a',destination:'bus:r',gain:.5,tap:'postFader',enabled:true}];
  const h=harness(p),panel=new Element('panel',h.document),row=new Element('row',h.document),field=new Element('routing-send-gain-send-1',h.document);
  panel.dataset.routingOwner='track:a';row.dataset.routingSend='send-1';field.dataset.routingSendField='gain';field.value='0.5';
  panel.querySelectorAll=selector=>selector==='[data-routing-send]'?[row]:[];row.querySelectorAll=selector=>selector==='[data-routing-send-field]'?[field]:[];
  h.document.querySelectorAll=selector=>selector==='[data-routing-owner]'?[panel]:[];h.R.bind('track:a');
  const before=h.requests.length;field.value='';field.dispatch('change');await drain();
  assert.equal(h.state.project.routing.sends[0].gain,.5,'an empty field must not silence the send');assert.equal(h.mutations.length,0);
  assert.equal(h.requests.length,before,'nothing is sent to the audio host');assert.equal(field.value,'0.5','the field shows the stored gain again');
  field.value='1.25';field.dispatch('change');await waitUntil(()=>h.mutations.length===1);assert.equal(h.state.project.routing.sends[0].gain,1.25);
});

test('Remove bus on a track group\'s bus opens the group dialog instead of leaving the group without a bus',async()=>{
  const p=fixture();p.groups=[{id:'kit',name:'Kit',busID:'g'}];p.routing.buses[0].groupID='kit';p.tracks[0].groupID='kit';p.routing.outputs['track:a']='bus:g';
  const h=harness(p),asked=[];h.context.window.STApp={deleteGroup:id=>{asked.push(id);return Promise.resolve(false);}};
  h.R.removeBus('bus:g');await drain();
  assert.deepEqual(asked,['kit'],'the group decides: ungroup (bus removed, tracks to Master) or delete');
  assert.equal(h.modals.length,0,'no bus-only removal is offered');assert.equal(h.mutations.length,0);
  assert.deepEqual(copy(h.state.project.routing.buses.map(bus=>bus.id)),['g','r']);
  h.R.removeBus('bus:r');assert.equal(h.modals.at(-1).title,'Remove bus','an ordinary return bus is still removed here');
  assert.equal(h.document.activeElement?.id,'routing-delete-apply','the confirmation, not the close button, has focus');
});

(async()=>{for(const {name,run}of tests){await run();process.stdout.write(`PASS ${name}\n`);}process.stdout.write(`${tests.length} routing UI suites passed.\n`);})().catch(error=>{console.error(error);process.exitCode=1;});
