'use strict';
// Real application + NKS + automation modules; bridge responses are controlled
// here. NativeUINKSAutomationTests supplies the separate real WebKit/audio check.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
async function tick() { for (let i=0;i<60;i++) await Promise.resolve(); }
async function bounded(promise) {
  let timeout;
  try { return await Promise.race([promise, new Promise((_, reject) => { timeout=setTimeout(() => reject(Error('NKS workflow did not settle within two seconds')), 2000); })]); }
  finally { clearTimeout(timeout); }
}
function harness() {
  const nodes=new Map(), calls=[], handlers=new Map(), timers=new Map(); let timerID=0;
  const document={activeElement:null};
  function node(id) {
    if (!nodes.has(id)) nodes.set(id,{id,value:'',dataset:{},disabled:false,isConnected:true,innerHTML:'',style:{},listeners:{},classList:{toggle(){},add(){},remove(){}},
      addEventListener(type,fn){(this.listeners[type]??=[]).push(fn);},setAttribute(){},focus(){document.activeElement=this;},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return [];},
      fire(type,extra={}){const event={target:this,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;},...extra};for(const fn of this.listeners[type]||[])fn(event);return event;}});
    return nodes.get(id);
  }
  const root=node('nks-root');root.dataset.nksRoot='0';const knob=node('nks-0-0-range');knob.type='range';knob.dataset={nksAddress:'270544960',nksName:'Brightness'};
  root.querySelectorAll=selector=>selector==='[data-nks-address]'?[knob]:[];root.querySelector=()=>null;
  document.body=node('body');document.querySelector=id=>node(id);document.querySelectorAll=()=>[];document.getElementById=node;document.addEventListener=()=>{};document.createElement=node;
  const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(copy(message));Promise.resolve().then(()=>handlers.get(message.command)?.(message.payload)??{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
  const context=vm.createContext({window,document,console,Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,queueMicrotask,innerWidth:1280,innerHeight:800,setTimeout:(fn,delay)=>{timers.set(++timerID,{fn,delay});return timerID;},clearTimeout:id=>timers.delete(id),requestAnimationFrame:fn=>fn()});
  for (const file of ['app.js','nks-controls.js','studio.js']) web.load(context, file);
  const ST=window.ST, p=ST.demoProject();p.audioPlugins=[];p.tracks=p.tracks.slice(0,1);p.tracks[0].instrument=0;p.instruments=p.instruments.slice(0,1);p.instruments[0].type='au';p.patterns[0].automation=[];
  const plugin={owner:'track:'+p.tracks[0].routingID,track:0,kind:'instrument',slot:0,instanceID:'fixture-instance',component:'fixture',name:'Original fixture',state:'state:0.25',nks:{name:'Original mapping fixture',path:'/fixture.nksf',pages:[{name:'Colour',parameters:[{slot:0,parameterID:270544960,mapped:true,name:'Brightness',section:'Filter'}]},{name:'Tone alias',parameters:[{slot:0,parameterID:270544960,mapped:true,name:'Sheen',section:'Air'}]}]}};
  p.audioPlugins=[plugin];p.instruments[0].plugin=copy(plugin);ST.state.project=p;ST.state.view='instrument';ST.state.track=0;ST.state.instrument=0;ST.state.engineReady=true;
  let value=.25;
  const parameters=()=>[{address:'270544960',name:'Generic Macro 7',normalized:value,writable:true,automatable:true}];
  handlers.set('plugin.parameters',parameters);handlers.set('plugin.parameter',payload=>{value=payload.value;return {display:String(value)};});
  handlers.set('plugin.state',()=>({...copy(plugin),state:`state:${value}`}));handlers.set('engine.snapshot',()=>({audioPlugins:copy(ST.state.project.audioPlugins)}));
  handlers.set('project.save',payload=>({saved:true,project:payload.project,path:'/tmp/nks-fixture.supertracker'}));
  const controls=window.STNKSControls;controls.render(p.instruments[0],plugin);controls.bind({querySelectorAll:()=>[root]});
  return {window,ST,p,plugin,controls,calls,handlers,root,knob,document,node,loadModule(file){web.load(context, file);},fireTimer(delay){let fired=0;for(const [id,timer] of [...timers])if(timer.delay===delay){timers.delete(id);timer.fn();fired++;}return fired;},edit(v){knob.value=String(v);knob.fire('input');},get value(){return value;}};
}
function prepareSampler(h) {
  h.loadModule('sampler.js');h.root.isConnected=false;
  h.p.instruments.push({id:1,type:'sample',samplePath:'/fixture/break.wav',rootNote:72,start:0,end:1,sliceCount:4,slicePoints:[0,.25,.5,.75],mode:'slice'});
  h.ST.state.instrument=1;h.ST.state.view='instrument';
  const button=h.node('sampler-audition-0');button.dataset.samplerAudition='0';
  h.node('sampler-slice-map').contains=element=>element===button;button.focus();
  return ()=>h.window.STSampler.key({key:'Enter',target:button,preventDefault(){},stopPropagation(){}});
}
// A routing change through the real transaction: the group's output moves from "no output" to Master.
function prepareRoutingChange(h) {
  h.loadModule('routing.js');h.p.tracks[0].routingID='fixture-tone';
  for(const descriptor of [h.p.audioPlugins[0],h.p.instruments[0].plugin,h.plugin])descriptor.owner='track:fixture-tone';
  h.p.routing={version:1,mixModel:'standard',buses:[{id:'repair-group',name:'Repair group',type:'group',volume:1,pan:0,mute:false,solo:false}],outputs:{'track:fixture-tone':'bus:repair-group','bus:repair-group':null},sends:[],sidechains:[]};
  const capabilities={supported:true,outputs:true,buses:{group:true,return:true,max:16},sends:{enabled:true,taps:['preFader','postFader'],max:128},sidechains:{enabled:true,taps:['preInserts','preFader','postFader'],inputs:[]}};
  h.handlers.set('routing.capabilities',()=>capabilities);
  h.handlers.set('engine.snapshot',()=>copy(h.p));
  h.handlers.set('routing.validate',()=>({valid:true}));
  h.handlers.set('routing.apply',payload=>({project:copy(payload.project),capabilities,status:{}}));
  return async()=>{await h.window.STRouting.refreshCapabilities();return h.window.STRouting.transaction('Change group output',project=>{project.routing.outputs['bus:repair-group']='master';},{owner:'bus:repair-group'});};
}
(async()=>{
  {
    const h=harness();await tick();h.edit(.62);h.window.STExport={isBusy:true};
    const originalNote=h.p.patterns[0].steps[0][0].note;h.ST.mutate('blocked edit',p=>p.patterns[0].steps[0][0].note=127);
    assert.equal(h.p.patterns[0].steps[0][0].note,originalNote);
    assert.equal(await h.ST.captureNativeEdits(),true);assert.equal(h.p.audioPlugins[0].state,'state:0.62');
    assert.deepEqual(h.calls.filter(x=>['plugin.parameter','plugin.state','engine.project','engine.snapshot'].includes(x.command)).map(x=>x.command),['plugin.parameter','plugin.state','engine.project','engine.snapshot']);
    assert.equal(h.calls.find(x=>x.command==='engine.project').payload.project.audioPlugins[0].state,'state:0.62');assert.equal(h.ST.state.undo.length,1);
    console.log('PASS locked export preparation drains coalesced NKS writes and snapshot before strict engine synchronization, while normal edits stay blocked');
  }
  for (const failure of ['plugin.parameter','plugin.state']) {
    const h=harness();await tick();const original=h.handlers.get(failure);h.handlers.set(failure,()=>{throw Error('fixture rejection');});h.edit(.71);h.window.STExport={isBusy:true};
    assert.equal(await h.ST.captureNativeEdits(),false);assert.equal(h.p.audioPlugins[0].state,'state:0.25');assert.equal(h.ST.state.undo.length,0);assert(!h.calls.some(x=>x.command==='engine.snapshot'));
    h.handlers.set(failure,original);assert.equal(await h.ST.captureNativeEdits(),true);assert.equal(h.p.audioPlugins[0].state,'state:0.71');assert.equal(h.ST.state.undo.length,1);
    console.log(`PASS ${failure} failure blocks export preparation and retains an explicitly retryable pending NKS edit`);
  }
  {
    const h=harness();await tick();h.edit(.73);h.ST.state.row=9;
    const event=h.root.fire('keydown',{target:h.knob,key:'å',code:'KeyA',altKey:true});await tick();
    assert(event.prevented&&event.stopped);assert.equal(h.ST.state.view,'automation');
    const lane=h.p.patterns[0].automation[0];assert.equal(lane.target,'plugin:instrument:0:270544960');assert.equal(lane.name,'Brightness');assert.deepEqual(copy(lane.points),[{row:9,value:.73}]);
    h.ST.state.automationQuery='Sheen';let html=h.window.STStudio.renderAutomation();assert(html.includes('Brightness'));assert(html.includes('Already automated'));assert(!html.includes('No parameters match'));
    h.ST.state.automationQuery='Generic Macro 7';html=h.window.STStudio.renderAutomation();assert(html.includes('Brightness'),'original host name remains searchable');
    h.ST.state.automationQuery='Colour';assert(h.window.STStudio.renderAutomation().includes('Brightness'),'mapped page name remains searchable');
    assert.equal(await h.ST.projectAction('save'),true);const saved=h.calls.findLast(x=>x.command==='project.save').payload.project;
    assert.equal(saved.audioPlugins[0].state,'state:0.73');assert.equal(saved.audioPlugins[0].nks.pages.length,2);assert.equal(saved.patterns[0].automation[0].target,'plugin:instrument:0:270544960');
    console.log('PASS mapped Option+A uses exact non-index ParamID, current value, NKS labels/page aliases and original host search; Save preserves mapping and lane');
  }
  {
    const h=harness();await tick();h.edit(.74);const promise=h.controls.automateControl(h.knob);await promise;
    assert.equal(h.ST.state.view,'automation');assert.equal(h.p.patterns[0].automation[0].points[0].value,.74);assert.equal(h.p.audioPlugins[0].state,'state:0.74');
    console.log('PASS context-action NKS delegate settles the same pending control before creating its lane');
  }
  {
    const h=harness();await tick();h.edit(.76);const original=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>({...original(),instanceID:'replacement-in-same-slot'}));
    assert.equal(await h.ST.captureNativeEdits(),false);assert.equal(h.p.audioPlugins[0].instanceID,'fixture-instance');assert.equal(h.p.audioPlugins[0].state,'state:0.25');
    assert(h.calls.filter(x=>['plugin.parameter','plugin.parameters','plugin.state'].includes(x.command)).every(x=>x.payload.instanceID==='fixture-instance'));
    h.handlers.set('plugin.state',original);assert.equal(await h.ST.captureNativeEdits(),true);assert.equal(h.p.audioPlugins[0].state,'state:0.76');
    console.log('PASS NKS reads/writes/captures carry stable instance identity and refuse a replacement snapshot from the same slot');
  }
  {
    const h=harness();await tick();let release;const graph=new Promise(resolve=>release=resolve);h.ST.waitForGraphEdit=()=>graph;h.edit(.77);
    const save=h.ST.captureNativeEdits();await tick();assert(!h.calls.some(x=>x.command==='plugin.parameter'));
    h.p.audioPlugins[0]={...h.p.audioPlugins[0],instanceID:'new-instance'};release();await save;
    assert(!h.calls.some(x=>x.command==='plugin.parameter'),'a pending graph transition must recheck the exact edited instance before posting writes');
    console.log('PASS pending graph changes hold NKS writes and a replaced instrument cancels them safely');
  }
  {
    const h=harness();await tick();h.ST.state.view='automation';h.window.STStudio.bindAutomation();await tick();
    assert(h.window.STStudio.target(0,'plugin:instrument:0:270544960'));
    h.p.audioPlugins[0]={...h.p.audioPlugins[0],instanceID:'replacement',parameters:[{address:'77',name:'New macro',normalized:.3}],nks:{pages:[{name:'New page',parameters:[{slot:0,parameterID:77,mapped:true,name:'New colour'}]}]}};
    assert.equal(h.window.STStudio.target(0,'plugin:instrument:0:270544960'),undefined,'same-slot old parameter cache must not create lanes against a replacement instrument');
    assert.equal(h.window.STStudio.target(0,'plugin:instrument:0:77').name,'New colour');
    console.log('PASS replacing an NKS instrument cannot reuse parameter addresses cached for the preceding instance');
  }
  {
    const h=harness();await tick();let captures=0,releaseFirst,releaseSecond,rejectSecond;
    const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>{
      captures++;
      if(captures===1)return new Promise(resolve=>{releaseFirst=()=>resolve(currentState());});
      if(captures===2)return new Promise((resolve,reject)=>{releaseSecond=()=>resolve(currentState());rejectSecond=reject;});
      return currentState();
    });
    h.handlers.set('engine.project',payload=>{
      // A native project update restores its supplied opaque state and invalidates
      // a profiled VST3 capture that is still awaiting its settlement interval.
      h.handlers.get('plugin.parameter')({value:Number(payload.project.audioPlugins[0].state.split(':')[1])});
      rejectSecond?.(Error('The project changed while plug-in edits were being saved. Retry saving.'));
      return {};
    });
    h.edit(.4);h.knob.fire('change');await tick();assert.equal(captures,1);
    h.edit(.6);h.knob.fire('change');await tick();releaseFirst();await tick();
    assert.equal(h.value,.6);assert.equal(captures,2);
    // Also cover an ordinary queued project sync: it must not overtake either
    // the preceding NKS snapshot's sync or the latest coalesced native edit.
    h.ST.syncEngine();assert(h.fireTimer(100)>0);await tick();
    assert.equal(h.value,.6,'an older project snapshot must not restore over a newer NKS write');
    assert.equal(h.calls.filter(call=>call.command==='engine.project').length,0,'engine updates must wait for pending mapped-control capture');
    releaseSecond();await bounded(h.controls.flushEdits());await tick();
    h.fireTimer(100);await tick();
    assert.equal(h.p.audioPlugins[0].state,'state:0.6');assert.equal(h.value,.6);
    const updates=h.calls.filter(call=>call.command==='engine.project');assert(updates.length>0);
    assert(updates.every(call=>call.payload.project.audioPlugins[0].state==='state:0.6'));
    assert(updates.every(call=>call.payload.awaitPlugins===true),'the barrier requires an acknowledgment after native plug-in restoration settles');
    console.log('PASS a queued engine update cannot restore an older snapshot over a newer mapped-control capture');
  }
  {
    const h=harness();await tick();const currentState=h.handlers.get('plugin.state');let rejectCapture=true;
    h.handlers.set('plugin.state',()=>{
      if(rejectCapture){h.handlers.get('plugin.parameter')({value:.25});throw Error('The graph changed while controls were being captured.');}
      return currentState();
    });
    h.edit(.64);assert.equal(await bounded(h.controls.flushEdits()),false);
    assert.equal(h.value,.25);assert.equal(h.p.audioPlugins[0].state,'state:0.25');
    rejectCapture=false;assert.equal(await bounded(h.controls.flushEdits()),true);
    assert.equal(h.value,.64,'retry must replay the final explicit intent after native capture failure');
    assert.equal(h.p.audioPlugins[0].state,'state:0.64');
    assert.deepEqual(h.calls.filter(call=>call.command==='plugin.parameter').map(call=>call.payload.value),[.64,.64]);
    console.log('PASS a failed capture retries the latest explicit mapped-control value even if native state was restored meanwhile');
  }
  {
    const h=harness();await tick();let finishSave;
    h.handlers.set('project.save',payload=>new Promise(resolve=>{finishSave=()=>resolve({saved:true,project:copy(payload.project),path:'/tmp/nks-fixture.supertracker'});}));
    const save=h.ST.projectAction('save');await tick();assert.equal(typeof finishSave,'function');
    const owner=h.ST.state.project;h.edit(.83);finishSave();await bounded(save);await tick();
    assert.equal(h.ST.state.project,owner,'a Save acknowledgment must preserve the owner of newer coalesced edits');
    assert.equal(h.ST.state.dirty,true,'newer mapped-control input must remain visibly unsaved');
    assert.equal(await bounded(h.controls.flushEdits()),true);assert.equal(h.value,.83);assert.equal(owner.audioPlugins[0].state,'state:0.83');
    h.handlers.set('project.save',payload=>({saved:true,project:copy(payload.project),path:'/tmp/nks-fixture.supertracker'}));
    assert.equal(await bounded(h.ST.projectAction('save')),true);
    assert.equal(h.calls.findLast(call=>call.command==='project.save').payload.project.audioPlugins[0].state,'state:0.83');assert.equal(h.ST.state.dirty,false);
    console.log('PASS input made while Save awaits its acknowledgment keeps its project identity, dirty state and explicit value for the next Save');
  }
  {
    const h=harness();await tick();let finishCapture;const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    h.edit(.52);h.knob.fire('change');await tick();assert.equal(typeof finishCapture,'function');
    h.ST.mutate('change an unrelated note',project=>{project.patterns[0].steps[0][0].note=72;});
    assert(h.fireTimer(100)>0);await tick();
    assert.equal(h.calls.filter(call=>call.command==='engine.project').length,0);
    finishCapture();assert.equal(await bounded(h.controls.flushEdits()),true);await tick();
    const update=h.calls.find(call=>call.command==='engine.project');assert(update);
    assert.equal(update.payload.project.audioPlugins[0].state,'state:0.52');assert.equal(update.payload.project.patterns[0].steps[0][0].note,72);
    console.log('PASS an unrelated note edit waits for the current mapped-control capture and publishes both edits together');
  }
  {
    const h=harness();await tick();let finishUpdate;
    h.handlers.set('engine.project',payload=>new Promise(resolve=>{finishUpdate=()=>{
      h.handlers.get('plugin.parameter')({value:Number(payload.project.audioPlugins[0].state.split(':')[1])});resolve({});
    };}));
    h.ST.mutate('rename the session',project=>{project.name='New session name';});assert(h.fireTimer(100)>0);await tick();
    assert.equal(typeof finishUpdate,'function');assert.equal(h.calls.find(call=>call.command==='engine.project').payload.awaitPlugins,true);
    h.edit(.57);h.knob.fire('change');await tick();
    assert.equal(h.calls.filter(call=>call.command==='plugin.parameter').length,0,'mapped writes must wait for a native engine update already in flight');
    finishUpdate();assert.equal(await bounded(h.controls.flushEdits()),true);await tick();
    assert.equal(h.value,.57);assert.equal(h.p.audioPlugins[0].state,'state:0.57');assert.equal(h.p.name,'New session name');
    console.log('PASS mapped-control writes wait for a previously posted native project update before applying their final value');
  }
  {
    const h=harness();await tick();let finishSnapshot;
    h.handlers.set('engine.snapshot',()=>new Promise(resolve=>{finishSnapshot=()=>resolve({audioPlugins:[{...copy(h.plugin),state:'state:0.26'}]});}));
    const owner=h.ST.state.project,save=h.ST.projectAction('save');await tick();assert.equal(typeof finishSnapshot,'function');
    h.edit(.84);finishSnapshot();assert.equal(await bounded(save),false);
    assert.equal(h.ST.state.project,owner);assert.equal(owner.audioPlugins[0],h.plugin,'an older native snapshot must not invalidate the newly edited plug-in entry');
    assert.equal(h.ST.state.dirty,true);assert(!h.calls.some(call=>call.command==='project.save'));
    assert.equal(await bounded(h.controls.flushEdits()),true);assert.equal(h.value,.84);assert.equal(owner.audioPlugins[0].state,'state:0.84');
    console.log('PASS input made while the native snapshot is pending prevents that older snapshot from invalidating the new mapped edit');
  }
  {
    const h=harness();await tick();h.ST.mutate('rename the session',project=>{project.name='Keep this earlier edit';});
    let finishCapture;const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    h.edit(.7);h.knob.fire('change');await tick();assert.equal(typeof finishCapture,'function');
    const undo=h.ST.perform('undo'),again=h.ST.perform('undo');await tick();
    assert.equal(h.ST.state.project.name,'Keep this earlier edit','pending Undo must not pop unrelated prior history');
    assert.equal(h.ST.state.undo.length,1);finishCapture();await bounded(Promise.all([undo,again]));await tick();
    assert.equal(h.ST.state.project.audioPlugins[0].state,'state:0.25');assert.equal(h.ST.state.project.name,'Keep this earlier edit');
    assert.equal(h.ST.state.undo.length,1);assert.equal(h.ST.state.redo.length,1,'repeated Undo while capture waits coalesces into one action');
    h.ST.perform('redo');assert.equal(h.ST.state.project.audioPlugins[0].state,'state:0.7');assert.equal(h.ST.state.project.name,'Keep this earlier edit');
    console.log('PASS Undo waits for a pending mapped gesture, preserves earlier history, coalesces repeated requests and retains exact Redo');
  }
  {
    const h=harness();await tick();h.ST.mutate('rename the session',project=>{project.name='Keep history after failure';});
    const currentState=h.handlers.get('plugin.state');h.handlers.set('plugin.state',()=>{throw Error('Capture failed');});h.edit(.78);
    await bounded(h.ST.perform('undo'));await tick();
    assert.equal(h.ST.state.project.name,'Keep history after failure');assert.equal(h.ST.state.undo.length,1);assert.equal(h.ST.state.redo.length,0);
    assert.equal(h.controls.hasPendingEdits,true);assert.equal(h.ST.state.dirty,true);
    h.handlers.set('plugin.state',currentState);await bounded(h.ST.perform('undo'));await tick();
    assert.equal(h.ST.state.project.name,'Keep history after failure');assert.equal(h.ST.state.project.audioPlugins[0].state,'state:0.25');
    assert.equal(h.ST.state.undo.length,1);assert.equal(h.ST.state.redo.length,1);
    h.ST.perform('redo');assert.equal(h.ST.state.project.audioPlugins[0].state,'state:0.78');
    console.log('PASS a failed capture leaves the mapped draft and unrelated Undo history intact until explicit retry succeeds');
  }
  {
    const h=harness();await tick();h.edit(.82);const incoming={...copy(h.plugin),state:'state:0.25'};
    h.window.nativeEvent({type:'plugins.changed',payload:{plugins:[incoming]}});
    assert.equal(h.controls.hasPendingEdits,true,'a late clone of the same native instance must adopt the current gesture');
    assert.equal(await bounded(h.controls.flushEdits()),true);
    assert.equal(h.value,.82);assert.equal(h.p.audioPlugins[0].state,'state:0.82');assert.equal(h.p.instruments[0].plugin.state,'state:0.82');
    assert(h.calls.filter(call=>call.command==='plugin.parameter').every(call=>call.payload.instanceID===h.plugin.instanceID));
    console.log('PASS a late native descriptor notification for the same exact instance preserves and captures the newer mapped-control input');
  }
  {
    const h=harness();await tick();let finishCapture;const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    h.edit(.66);const flush=h.controls.flushEdits();await tick();assert.equal(typeof finishCapture,'function');
    h.window.nativeEvent({type:'plugins.changed',payload:{plugins:[{...copy(h.plugin),state:'state:0.25'}]}});
    finishCapture();assert.equal(await bounded(flush),true);assert.equal(h.p.audioPlugins[0].state,'state:0.66');assert.equal(h.value,.66);
    console.log('PASS same-instance descriptor adoption also preserves an already in-flight mapped-control state capture');
  }
  {
    for(const replacementKind of ['instance','component','route','mapping','duplicate']){
      const h=harness();await tick();h.edit(.81);const replacement=copy(h.plugin);
      if(replacementKind==='instance')replacement.instanceID='another-instance';
      if(replacementKind==='component')replacement.component='another-component';
      if(replacementKind==='route')replacement.track=1;
      if(replacementKind==='mapping')replacement.nks.path='/different-preset.nksf';
      const next=replacementKind==='duplicate'?[replacement,copy(replacement)]:[replacement];
      h.window.nativeEvent({type:'plugins.changed',payload:{plugins:next}});
      assert.equal(await bounded(h.controls.flushEdits()),false,`${replacementKind}: save must first report canceled old-target input`);
      assert.equal(h.controls.hasPendingEdits,false);assert(!h.calls.some(call=>call.command==='plugin.parameter'));
      assert.equal(h.ST.state.dirty,true);assert.equal(await bounded(h.controls.flushEdits()),true,'an explicit later retry may inspect the replacement');
    }
    console.log('PASS replaced, moved, remapped or ambiguous native identities cancel old-target drafts explicitly without writing another plug-in');
  }
  {
    const h=harness();await tick();let finishCapture;const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    h.edit(.67);const first=h.controls.flushEdits(),second=h.controls.flushEdits();await tick();
    const replacement={...copy(h.plugin),instanceID:'replacement-after-capture-started',state:'replacement-state'};
    h.window.nativeEvent({type:'plugins.changed',payload:{plugins:[replacement]}});finishCapture();
    assert.deepEqual(await bounded(Promise.all([first,second])),[false,false]);assert.equal(h.p.audioPlugins[0],replacement);assert.equal(h.p.audioPlugins[0].state,'replacement-state');
    assert.equal(h.ST.state.undo.length,0);assert.equal(await bounded(h.controls.flushEdits()),true);
    console.log('PASS replacing an instance during capture rejects every waiting save without installing the retired instance snapshot');
  }
  {
    const h=harness();await tick();h.edit(.86);h.root.isConnected=false;h.ST.state.view='mixer';
    let finishCapture,factories=0;const currentState=h.handlers.get('plugin.state'),originalVolume=h.p.tracks[0].volume;
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    const sync=h.ST.syncEngineProject(()=>{factories++;const draft=copy(h.ST.state.project);draft.tracks[0].volume=.42;return draft;});await tick();
    assert.equal(factories,0,'preview cloning must wait for an unmounted control to commit its final native state');
    finishCapture();await bounded(sync);assert.equal(factories,1);
    const update=h.calls.findLast(call=>call.command==='engine.project');assert.equal(update.payload.project.audioPlugins[0].state,'state:0.86');
    assert.equal(update.payload.project.tracks[0].volume,.42);assert.equal(update.payload.awaitPlugins,true);
    assert.equal(h.p.tracks[0].volume,originalVolume,'a preview factory must not mutate its source project');
    console.log('PASS the shared engine helper builds a preview from the latest state only after unmounted mapped controls have settled');
  }
  {
    for(const cancellation of ['factory','owner','generation']){
      const h=harness();await tick();h.edit(.88);let finishCapture,factories=0;const currentState=h.handlers.get('plugin.state');
      h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
      const sync=h.ST.syncEngineProject(()=>{factories++;return cancellation==='factory'?null:copy(h.ST.state.project);});await tick();
      if(cancellation==='owner')h.ST.state.project=copy(h.p);
      if(cancellation==='generation')h.ST.cancelPendingEngineSync();
      finishCapture();assert.equal(await bounded(sync),false,`${cancellation}: a stale preview must cancel`);
      assert.equal(factories,cancellation==='factory'?1:0);assert(!h.calls.some(call=>call.command==='engine.project'));
    }
    console.log('PASS stale preview factories, project replacements and synchronization cancellation cannot post a late audio project');
  }
  {
    for(const failure of ['plugin.state','engine.project']){
      const h=harness();await tick();h.edit(.89);let factories=0;
      h.handlers.set(failure,()=>{throw Error('Required synchronization failed');});
      await assert.rejects(bounded(h.ST.syncEngineProject(()=>{factories++;return copy(h.ST.state.project);})),/failed|captur/i);
      assert.equal(factories,failure==='plugin.state'?0:1);
      if(failure==='plugin.state')assert(!h.calls.some(call=>call.command==='engine.project'));
    }
    console.log('PASS the shared engine helper propagates mapped-capture and native project failures instead of claiming successful synchronization');
  }
  {
    for(const cancel of [false,true]){
      const h=harness();await tick();h.edit(.91);h.root.isConnected=false;h.ST.state.view='mixer';
      let finishCapture;const currentState=h.handlers.get('plugin.state');h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
      const input=h.node('mix-volume-0');input.value='.37';input.dataset={studioProperty:'volume',studioTrack:'0'};
      h.document.querySelectorAll=selector=>selector==='[data-studio-property]'?[input]:[];
      h.window.STStudio.bindMixer();input.fire('input');assert(h.fireTimer(70)>0);await tick();assert.equal(typeof finishCapture,'function');
      if(cancel)input.isConnected=false;
      finishCapture();await tick();await bounded(h.ST.waitForEngineUpdate());await tick();
      const updates=h.calls.filter(call=>call.command==='engine.project');
      if(cancel)assert.equal(updates.length,0,'a detached mixer control must cancel its delayed preview');
      else {
        assert.equal(updates.length,1);assert.equal(updates[0].payload.project.audioPlugins[0].state,'state:0.91');assert.equal(updates[0].payload.project.tracks[0].volume,.37);
        h.fireTimer(100);await tick();
        assert(h.calls.filter(call=>call.command==='engine.project').every(call=>call.payload.project.tracks[0].volume===.37),'the capture commit debounce must not restore over an active mixer preview');
      }
    }
    console.log('PASS the actual mixer preview serializes settled mapped state and cancels controls detached during capture');
  }
  {
    const h=harness();await tick();h.edit(.93);const audition=prepareSampler(h);
    let finishCapture,finishUpdate;const currentState=h.handlers.get('plugin.state');h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    h.handlers.set('engine.project',()=>new Promise(resolve=>{finishUpdate=()=>resolve({ok:true});}));
    assert.equal(audition(),true);await tick();assert.equal(typeof finishCapture,'function');assert(!h.calls.some(call=>call.command==='engine.project'));
    finishCapture();await tick();assert.equal(typeof finishUpdate,'function');
    const update=h.calls.find(call=>call.command==='engine.project');assert.equal(update.payload.project.audioPlugins[0].state,'state:0.93');assert.equal(update.payload.awaitPlugins,true);
    assert(!h.calls.some(call=>call.command==='step.preview'),'sample audition must await native plug-in restoration acknowledgment');
    finishUpdate();await bounded(h.ST.waitForEngineUpdate());await tick();
    const preview=h.calls.find(call=>call.command==='step.preview'&&call.payload.step.note>=0);assert(preview);assert.equal(preview.payload.step.instrument,1);assert.equal(preview.payload.step.fx1.value,1);
    console.log('PASS actual sampler audition preserves an unmounted NKS draft and waits for settled native restoration before sounding');
  }
  {
    for(const mode of ['focus-cancel','native-failure']){
      const h=harness();await tick();h.edit(.94);const audition=prepareSampler(h);
      let finishCapture;const currentState=h.handlers.get('plugin.state');h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
      h.handlers.set('engine.project',()=>{throw Error('Sampler synchronization rejected');});
      assert.equal(audition(),true);await tick();if(mode==='focus-cancel')h.node('outside-map').focus();
      finishCapture();await tick();await bounded(h.ST.waitForEngineUpdate());await tick();
      assert(!h.calls.some(call=>call.command==='step.preview'&&call.payload.step.note>=0));
      if(mode==='focus-cancel')assert(!h.calls.some(call=>call.command==='engine.project'));
      else assert(h.node('sampler-audition-status').textContent.includes('Sampler synchronization rejected'));
    }
    console.log('PASS leaving sampler focus or failing required synchronization cannot start a delayed slice preview');
  }
  {
    const h=harness();await tick();h.edit(.95);let finishCapture;const currentState=h.handlers.get('plugin.state');
    h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
    const preview=h.ST.syncEngineProject(()=>{const draft=copy(h.ST.state.project);draft.tracks[0].volume=.31;return draft;});await tick();
    h.ST.syncEngine(false,true);assert(h.fireTimer(100)>0);await tick();
    finishCapture();await bounded(preview);await tick();h.fireTimer(100);await tick();
    const updates=h.calls.filter(call=>call.command==='engine.project');assert.equal(updates.length,1,'a queued normal job already represented by the fresh preview must not revert it later');
    assert.equal(updates[0].payload.replace,true,'the represented normal job must retain its required Undo state restore');
    assert.equal(updates[0].payload.project.tracks[0].volume,.31);assert.equal(updates[0].payload.project.audioPlugins[0].state,'state:0.95');
    console.log('PASS a normal update whose timer fires during capture is represented once by the fresh preview without losing its Undo restore flag');
  }
  {
    const h=harness();await tick();h.edit(.96);h.ST.syncEngine(false,true);
    assert.equal(await bounded(h.ST.syncEngineProject(()=>null)),false);assert(!h.calls.some(call=>call.command==='engine.project'));
    assert(h.fireTimer(100)>0,'canceling a preview must leave the committed project update scheduled');await tick();
    const update=h.calls.find(call=>call.command==='engine.project');assert(update);assert.equal(update.payload.replace,true);assert.equal(update.payload.project.audioPlugins[0].state,'state:0.96');
    console.log('PASS a canceled preview preserves the queued committed update and its required Undo restore');
  }
  {
    for(const cachedIdentity of [true,false]){
      const h=harness();await tick();h.loadModule('nks-loader.js');if(!cachedIdentity)delete h.p.instruments[0].plugin.instanceID;h.edit(.87);
      h.handlers.set('plugin.loadNKS',()=>({...copy(h.plugin),instanceID:'next-preset-instance',state:'next-preset-state',nks:{...copy(h.plugin.nks),name:'Next preset',path:'/next.nksf'}}));
      assert.equal(await bounded(h.window.STNKS.load('/next.nksf')),true,'settling current mapped edits must not be mistaken for a changed preset destination');
      assert.equal(h.calls.filter(call=>call.command==='plugin.loadNKS').length,1);
      assert.equal(h.p.audioPlugins[0].instanceID,'next-preset-instance');assert.equal(h.p.instruments[0].plugin.state,'next-preset-state');
      assert(h.ST.state.undo.some(item=>item.label==='load NKS preset'&&item.project.audioPlugins[0].state==='state:0.87'),'Undo of a new preset must retain the final edited previous preset');
    }
    console.log('PASS a new NKS preset waits for the previous pending gesture and preserves that final previous patch for Undo');
  }
  {
    for(const change of ['rename','assignment']){
      const h=harness();await tick();h.loadModule('nks-loader.js');h.edit(.79);
      let finishCapture;const currentState=h.handlers.get('plugin.state');h.handlers.set('plugin.state',()=>new Promise(resolve=>{finishCapture=()=>resolve(currentState());}));
      const load=h.window.STNKS.load('/next.nksf');await tick();assert.equal(typeof finishCapture,'function');
      if(change==='rename')h.p.instruments[0].name='Renamed while settling';
      else {h.p.instruments.push({id:1,type:'synth',name:'Another instrument'});h.p.tracks[0].instrument=1;}
      finishCapture();assert.equal(await bounded(load),false);assert(!h.calls.some(call=>call.command==='plugin.loadNKS'));
      if(change==='rename')assert.equal(h.p.instruments[0].name,'Renamed while settling');else assert.equal(h.p.tracks[0].instrument,1);
    }
    console.log('PASS genuine instrument edits or reassignment during NKS settlement still cancel loading into the changed destination');
  }
  {
    const h=harness();await tick();const repair=prepareRoutingChange(h),before=copy(h.p);
    h.handlers.set('engine.project',()=>{throw Error('Ordinary project synchronization failed');});
    await assert.rejects(bounded(repair()),/Ordinary project synchronization failed/);
    assert.deepEqual(copy(h.p),before);assert(!h.calls.some(call=>['routing.validate','routing.apply'].includes(call.command)));assert.equal(h.ST.state.undo.length,0);
    console.log('PASS a routing change stops at a failed project synchronization before it is validated or applied');
  }
  {
    for(const change of ['new-draft','replacement']){
      const h=harness();await tick();const repair=prepareRoutingChange(h),before=copy(h.p);let finishSnapshot;
      h.handlers.set('engine.snapshot',()=>new Promise(resolve=>{finishSnapshot=()=>resolve(copy(before));}));
      const pending=repair();await tick();assert.equal(typeof finishSnapshot,'function');
      if(change==='new-draft')h.edit(.92);else h.ST.state.project={...copy(h.p),name:'A newer session'};
      const currentOwner=h.ST.state.project;finishSnapshot();await assert.rejects(bounded(pending));
      assert.equal(h.ST.state.project,currentOwner);assert(!h.calls.some(call=>['routing.validate','routing.apply'].includes(call.command)));
      if(change==='new-draft'){assert.equal(h.controls.hasPendingEdits,true);assert.equal(h.p.audioPlugins[0].state,before.audioPlugins[0].state);}
      else assert.equal(h.ST.state.project.name,'A newer session');
    }
    console.log('PASS a new mapped draft or session replacement during the capture before a routing change prevents a stale change');
  }
  {
    for(const phase of ['routing.validate','routing.apply']){
      const h=harness();await tick();const repair=prepareRoutingChange(h),owner=h.p;let finish;
      h.handlers.set(phase,payload=>new Promise(resolve=>{finish=()=>resolve(phase==='routing.validate'?{valid:true}:{project:copy(payload.project),status:{}});}));
      const pending=repair();await tick();assert.equal(typeof finish,'function');h.edit(.92);finish();await assert.rejects(bounded(pending));
      assert.equal(h.ST.state.project,owner);assert.equal(owner.routing.outputs['bus:repair-group'],null,'a canceled change keeps the old route');
      if(phase==='routing.validate'){
        assert(!h.calls.some(call=>call.command==='routing.apply'));assert.equal(h.controls.hasPendingEdits,true);assert.equal(owner.audioPlugins[0].state,'state:0.25');
      }else{
        const rollback=h.calls.findLast(call=>call.command==='engine.project');assert(rollback);assert.equal(rollback.payload.replace,true);
        assert.equal(rollback.payload.project.audioPlugins[0].state,'state:0.92');assert.equal(rollback.payload.project.routing.outputs['bus:repair-group'],null);
        assert.equal(owner.audioPlugins[0].state,'state:0.92');assert.equal(h.controls.hasPendingEdits,false);assert.equal(h.ST.state.undo.length,1);
      }
    }
    console.log('PASS a new mapped draft during routing validation cancels before apply; during apply it is captured into the explicit current-project rollback');
  }
  {
    const h=harness();await tick();const repair=prepareRoutingChange(h),owner=h.p;let finish;
    h.handlers.set('routing.apply',payload=>new Promise(resolve=>{finish=()=>resolve({project:copy(payload.project),status:{}});}));
    const pending=repair();await tick();h.edit(.97);h.handlers.set('plugin.state',()=>{throw Error('Mapped rollback state capture failed');});finish();
    await assert.rejects(bounded(pending),/Restoring current audio state failed/);
    assert.equal(h.ST.state.project,owner);assert.equal(owner.audioPlugins[0].state,'state:0.25');assert.equal(owner.routing.outputs['bus:repair-group'],null);
    assert.equal(h.controls.hasPendingEdits,true);assert.equal(h.ST.state.undo.length,0);assert(!h.calls.some(call=>call.command==='engine.project'&&call.payload.replace));
    h.handlers.set('plugin.state',()=>({...copy(h.plugin),state:`state:${h.value}`}));assert.equal(await bounded(h.controls.flushEdits()),true);assert.equal(owner.audioPlugins[0].state,'state:0.97');
    console.log('PASS a failed rollback capture after a routing change keeps the latest mapped intent retryable without installing stale blobs or a false routing Undo');
  }
  console.log('NKS automation workflow: 34 application integration groups passed (simulated bridge, no audio claim).');
})().catch(error=>{console.error(error);process.exitCode=1;});
