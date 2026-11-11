// Exercise the actual app functions against a deterministic asynchronous native
// boundary. No audio, app, device, or plug-in is launched by this suite.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const copy=x=>JSON.parse(JSON.stringify(x));
function harness(){
  const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let next=1;
  const element=id=>{if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',dataset:{},attributes:{},classList:{toggle(){},add(){},remove(){}},focus(){},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(k,v){this.attributes[k]=v;}});return nodes.get(id);};
  const document={body:element('body'),activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  const window={addEventListener(){},STHostTransport:hostStub({post(m){calls.push(copy(m));Promise.resolve().then(()=>handlers.has(m.command)?handlers.get(m.command)(m.payload)??{}:noteRecordingHost(window)(m)??{}).then(result=>window.STHostTransport.deliver({id:m.id,ok:true,result}),error=>window.STHostTransport.deliver({id:m.id,ok:false,error:error.message}));}})};
  const context=vm.createContext({window,document,console,setTimeout:(f)=>{timers.set(next,f);return next++;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1200,innerHeight:800});
  web.load(context, 'app.js');
  const S=window.ST;S.state.project=S.demoProject();S.state.project.audioPlugins=[];
  let native=copy(S.state.project),failPublish=false;
  handlers.set('engine.project',p=>{if(failPublish&&!p.replace)return false;native=copy(p.project);return {};});
  handlers.set('engine.snapshot',()=>copy(native));
  handlers.set('plugin.state',p=>copy(native.audioPlugins.find(x=>x.owner===p.owner&&x.kind===p.kind)||{}));
  handlers.set('plugin.remove',p=>{native.audioPlugins=native.audioPlugins.filter(x=>x.owner!==p.owner||x.kind!==p.kind);return {};});
  handlers.set('plugin.load',p=>{native.audioPlugins=native.audioPlugins.filter(x=>x.owner!==p.owner||x.kind!==p.kind);const v={...p,state:'loaded state'};native.audioPlugins.push(v);return v;});
  handlers.set('sample.load',p=>({path:p.path,name:'New sample',frames:32}));
  handlers.set('sample.edit',()=>({path:'/edited.wav',frames:16}));
  handlers.set('project.save',p=>({saved:true,project:p.project}));
  // A track's mixer channel: what plug-in descriptors and plug-in commands name as the owner.
  const own=track=>`track:${S.state.project.tracks[track].routingID}`;
  function plugin(track=0,component='old'){
    S.state.project.instruments[track].type='au';S.state.project.tracks[track].instrument=track;
    const plugin={owner:own(track),kind:'instrument',slot:0,instanceID:component+track,component,state:'saved state'};
    S.state.project.audioPlugins.push(plugin);native=copy(S.state.project);return plugin;
  }
  return {S,window,calls,handlers,element,plugin,own,get native(){return native;},set native(v){native=v;},get toast(){return element('#toast-region').innerHTML;},failPublish:()=>failPublish=true};
}
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
async function until(test){for(let i=0;i<250;i++){if(test())return;await Promise.resolve();}assert.fail('asynchronous operation did not reach expected boundary');}
async function run(){
  {
    const h=harness(),before=copy(h.S.state.project),id=before.instruments.length,track=before.tracks.length;
    assert.equal(h.S.addInstrument(),id);assert.equal(h.S.state.track,track);assert.equal(h.S.state.instrument,id);
    assert.equal(h.S.state.project.tracks[track].instrument,id);h.S.selectInstrument(0);assert.equal(h.S.state.track,0);h.S.selectInstrument(id);assert.equal(h.S.state.track,track);assert.deepEqual(copy(h.S.state.project.tracks.slice(0,track)),before.tracks);
    assert(h.S.state.project.patterns.every(p=>p.steps.length===track+1));assert.equal(h.S.state.undo.length,1);
    await h.S.perform('undo');assert.deepEqual(copy(h.S.state.project),before);
    console.log('PASS Add instrument creates a selected linked lane across all patterns with one Undo');
  }
  {
    const h=harness(),old=h.plugin(),before=copy(h.S.state.project),id=before.instruments.length,track=before.tracks.length;
    await h.S.loadSampleAsNew('/added.wav');assert.equal(h.S.state.instrument,id);assert.equal(h.S.state.track,track);
    assert.equal(h.S.state.project.tracks[track].instrument,id);assert.equal(h.S.state.project.instruments[id].samplePath,'/added.wav');
    assert.deepEqual(copy(h.S.state.project.tracks.slice(0,track)),before.tracks);assert.deepEqual(copy(h.S.state.project.audioPlugins),before.audioPlugins);
    assert(!h.calls.some(c=>c.command==='plugin.remove'));assert.equal(h.native.tracks[track].instrument,id);assert.equal(h.S.state.undo.length,1);
    await h.S.perform('undo');assert.deepEqual(copy(h.S.state.project),before);
    console.log('PASS Add sample creates a playable lane and keeps the existing instrument plug-in and notes intact');
  }
  {
    const h=harness();assert.equal(h.window.TRACK_SANITY_LIMIT,1024);while(h.S.state.project.tracks.length<h.window.TRACK_SANITY_LIMIT)h.S.appendInstrumentTrack(h.S.state.project,0);
    const before=copy(h.S.state.project);assert.equal(h.S.addInstrument(),false);assert.equal(await h.S.loadSampleAsNew('/overflow.wav'),false);
    assert.deepEqual(copy(h.S.state.project),before);assert.equal(h.S.state.undo.length,0);assert(!h.calls.some(c=>c.command==='sample.load'));
    console.log('PASS track capacity rejects additions before native loading or orphan instruments');
  }

  {
    const h=harness(),old=h.plugin();h.handlers.set('plugin.state',()=>({...old,state:'live editor state'}));
    const oldMeta={path:'/old.wav'};h.S.state.project.instruments[0].samplePath=oldMeta.path;h.S.state.samples[0]=oldMeta;
    const painted=[];h.window.STSampler={bind(){},parts(){painted.push(h.S.state.samples[0]?.path);return {};}};h.S.setView('instrument');
    h.handlers.set('sample.load',p=>{h.window.nativeEvent({type:'sample.loaded',payload:{instrument:0,sample:{path:p.path,frames:32}}});assert.equal(h.S.state.samples[0],oldMeta);return {path:p.path,name:'New sample'};});
    await h.S.loadSample('/new.wav');assert.equal(h.S.state.project.instruments[0].type,'sample');assert.equal(h.S.state.project.audioPlugins.length,0);assert.equal(h.S.state.undo.length,1);
    assert.equal(h.S.state.undo[0].project.audioPlugins[0].state,'live editor state');assert.equal(h.S.state.undo[0].project.instruments[0].type,'au');assert.equal(h.S.state.samples[0].path,'/new.wav');assert.equal(h.native.audioPlugins.length,0);assert.equal(painted.at(-1),'/new.wav','accepted waveform metadata must be painted after its deferred installation');
    await h.S.perform('undo');assert.equal(h.S.state.project.audioPlugins[0].state,'live editor state');
    console.log('PASS sample replacement captures live opaque state before removal, delays metadata, and preserves one exact Undo');
  }
  {
    for(const operation of ['load','edit','type','assign']){
      const h=harness();h.plugin();if(operation==='edit'){h.S.state.project.instruments[0].samplePath='/old.wav';h.S.state.samples[0]={path:'/old.wav'};}
      if(operation==='assign')h.S.state.instrument=1;
      h.window.STFreeze={checkMutation:()=> 'locked',explain(){}};
      const action=operation==='load'?h.S.loadSample('/new.wav').catch(e=>e):operation==='edit'?h.S.editSample('reverse'):h.S.perform(operation==='type'?'instrumentType:midi':'assignInstrument');
      await action;assert.equal(h.S.state.undo.length,0,operation);assert.equal(h.S.state.project.audioPlugins[0].component,'old',operation);assert.equal(h.native.audioPlugins[0].component,'old',operation);assert.equal(h.native.instruments[0].type,'au',operation);
      assert(h.calls.some(x=>x.command==='engine.project'&&x.payload.replace===true),operation);assert.doesNotMatch(h.toast,/Loaded|complete\./,operation);
      if(operation==='edit')assert.equal(h.S.state.samples[0].path,'/old.wav');
    }
    console.log('PASS Freeze mutation refusal restores native state for load/edit/type/assignment without Undo or success');
  }
  {
    for(const change of ['project','instrument','track','same-object']){
      const h=harness(),gate=deferred();h.plugin();h.handlers.set('sample.load',()=>gate.promise);
      const task=h.S.loadSample('/new.wav').catch(e=>e);await until(()=>h.calls.some(x=>x.command==='sample.load'));
      if(change==='project')h.S.state.project=h.S.normalize({name:'Replacement'});
      if(change==='instrument')h.S.state.project.instruments[0]={...h.S.state.project.instruments[0],name:'New identity'};
      if(change==='track')h.S.state.project.tracks[0]={...h.S.state.project.tracks[0],name:'New track identity'};
      if(change==='same-object')h.S.state.project.instruments[0].samplePath='/newer.wav';
      const expected=copy(h.S.state.project);gate.resolve({path:'/new.wav'});const error=await task;
      assert.match(error.message,/destination changed/);assert.deepEqual(copy(h.S.state.project),expected);assert.deepEqual(h.native,expected);assert.equal(h.S.state.undo.length,0);assert(!h.calls.some(x=>x.command==='plugin.remove'));assert.doesNotMatch(h.toast,/Loaded/);
    }
    console.log('PASS stale project, instrument, track, and in-place destination changes restore the current canonical owner');
  }
  {
    const h=harness(),gate=deferred();h.S.state.project.instruments[0].samplePath='/old.wav';h.S.state.samples[0]={path:'/old.wav'};h.handlers.set('sample.edit',()=>gate.promise);
    const task=h.S.editSample('trim');await until(()=>h.calls.some(x=>x.command==='sample.edit'));h.S.state.project.instruments[0].start=.4;gate.resolve({path:'/edited.wav'});assert.equal(await task,false);assert.equal(h.S.state.samples[0].path,'/old.wav');assert.equal(h.native.instruments[0].start,.4);assert.equal(h.native.instruments[0].samplePath,'/old.wav');
    console.log('PASS sample edit does not overwrite a region changed while rendering or publish its metadata');
  }
  {
    const h=harness();h.plugin();let n=0;const sync=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>{n++;return n===2?false:sync(p);});
    await assert.rejects(h.S.loadSample('/new.wav'),/not accepted/);assert.equal(h.S.state.project.audioPlugins[0].component,'old');assert.equal(h.S.state.undo.length,0);assert.equal(h.native.audioPlugins[0].component,'old');
    console.log('PASS false native candidate acknowledgement cannot commit or claim success');
  }
  {
    const h=harness();h.plugin();h.window.STFreeze={checkMutation:()=> 'locked',explain(){}};const sync=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>p.replace?false:sync(p));
    await assert.rejects(h.S.loadSample('/new.wav'),/restoration was not acknowledged/);await assert.rejects(h.window.STInstrumentEdits.waitForIdle(),/restoration was not acknowledged/);assert.equal(await h.S.captureNativeEdits(),false);assert.match(h.toast,/not acknowledged/);
    console.log('PASS failed rollback remains an explicit unresolved error for Save/capture rather than false success');
  }
  {
    const h=harness();h.plugin(0);h.plugin(1,'shared');h.S.state.project.tracks[1].instrument=0;
    h.S.state.project.routing={sidechains:[{id:'remove',destination:'old0'},{id:'keep',destination:'fx'}]};
    assert.equal(await h.S.perform('instrumentType:midi'),true);assert.equal(h.S.state.project.audioPlugins.length,0);assert.deepEqual(h.calls.filter(x=>x.command==='plugin.remove').map(x=>x.payload.owner),[h.own(0),h.own(1)]);assert.equal(h.S.state.project.routing.sidechains.length,1);assert.equal(h.S.state.undo[0].project.routing.sidechains.length,2);
    console.log('PASS shared instrument conversion removes every assigned native instance and its sidechains in the same Undo');
  }
  {
    const h=harness(),target=h.plugin(0,'target');h.plugin(1,'source');h.S.state.instrument=1;
    const capture=h.handlers.get('plugin.state');h.handlers.set('plugin.state',p=>{const v=capture(p);return {...v,state:p.owner===h.own(0)&&v.component==='target'?'live target':v.state};});
    assert.equal(await h.S.perform('assignInstrument'),true);assert.equal(h.S.state.project.tracks[0].instrument,1);assert.equal(h.S.state.project.audioPlugins.find(x=>x.owner===h.own(0)).component,'source');assert.equal(h.S.state.undo[0].project.audioPlugins.find(x=>x.owner===h.own(0)).state,'live target');assert.equal(h.S.state.undo[0].project.tracks[0].instrument,0);
    // No track plays the retired instrument now: it keeps its plug-in, as captured, without a channel or an instance.
    const kept=h.S.state.project.instruments[0].plugin;assert.equal(kept.component,'target');assert.equal(kept.state,'live target');assert.equal(kept.owner,undefined);assert.equal(kept.instanceID,undefined);
    assert.equal(Object.hasOwn(h.S.state.project.instruments[1],'plugin'),false,'an instrument a track plays keeps no copy');
    // Assigned again, it is loaded from what it kept, and stops keeping it.
    h.S.state.instrument=0;assert.equal(await h.S.perform('assignInstrument'),true);const back=h.S.state.project.audioPlugins.find(x=>x.owner===h.own(0));
    assert.equal(back.component,'target');assert.equal(h.S.state.project.tracks[0].instrument,0);assert.equal(Object.hasOwn(h.S.state.project.instruments[0],'plugin'),false);
    console.log('PASS assigning an AU captures the retiring target and commits the replacement with correct Undo; the retired instrument keeps its plug-in until it is assigned again');
  }
  {
    const h=harness(),gate=deferred();h.handlers.set('sample.load',()=>gate.promise);const task=h.S.loadSample('/new.wav');await until(()=>h.calls.some(x=>x.command==='sample.load'));
    const saving=h.S.projectAction('save');await Promise.resolve();assert(!h.calls.some(x=>x.command==='project.save'));
    await assert.rejects(h.S.loadSample('/second.wav'),/current audio/);gate.resolve({path:'/new.wav',name:'New'});await task;await saving;assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.instruments[0].samplePath,'/new.wav');assert.equal(h.window.STInstrumentEdits.isBusy,false);
    console.log('PASS Save waits for accepted pending instrument edits without self-deadlock and concurrent edits cannot overlap');
  }
  {
    const h=harness(),gate=deferred();h.plugin();let n=0;const sync=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>++n===2?gate.promise:sync(p));
    const task=h.S.loadSample('/new.wav').catch(e=>e);await until(()=>n===2);h.S.state.project.name='Newer unrelated title';gate.resolve({});const error=await task;assert.match(error.message,/project changed/);assert.equal(h.S.state.project.instruments[0].type,'au');assert.equal(h.native.name,'Newer unrelated title');assert.equal(h.S.state.undo.length,0);
    console.log('PASS canonical edits during candidate publication invalidate the commit and restore their current values');
  }
  {
    for(const kind of ['throw','false']){const h=harness();h.plugin();const remove=h.handlers.get('plugin.remove');h.handlers.set('plugin.remove',p=>{remove(p);if(kind==='throw')throw Error('Partial failure');return false;});assert.equal(await h.S.perform('instrumentType:sample'),false);assert.equal(h.native.audioPlugins[0].component,'old');assert.equal(h.S.state.undo.length,0);}
    console.log('PASS partial native removal failure and false acknowledgement both restore captured opaque state');
  }
  {
    const h=harness();h.plugin(0,'target');h.plugin(1,'source');h.S.state.instrument=1;
    h.window.STFreeze={checkMutation:()=> 'locked',explain(){}};
    assert.equal(await h.S.perform('assignInstrument'),false);assert.equal(h.S.state.undo.length,0);assert.equal(h.S.state.project.tracks[0].instrument,0);assert.equal(h.native.audioPlugins.find(x=>x.owner===h.own(0)).component,'target');
    const read=h.handlers.get('plugin.state');h.window.STFreeze=null;h.handlers.set('plugin.state',p=>{const result=read(p);if(p.owner===h.own(0)&&result.component==='source')throw Error('Replacement state unavailable');return result;});
    assert.equal(await h.S.perform('assignInstrument'),false);assert.equal(h.S.state.undo.length,0);assert.equal(h.native.audioPlugins.find(x=>x.owner===h.own(0)).component,'target');
    console.log('PASS AU replacement refusal and failed post-load state capture restore the retired instance without committing');
  }
  {
    const h=harness();h.S.state.project.instruments[0].samplePath='/old.wav';let transforms=0;
    await h.window.STInstrumentEdits.run(0,[],async edit=>{
      const result=await edit.native('sample.edit',{path:'/old.wav',instrument:0,operation:'trim'});
      await edit.commit('sample module edit',p=>{transforms++;p.instruments[0].samplePath=result.path;});
      h.S.state.samples[0]=result;
    });
    assert.equal(transforms,2);assert.equal(h.S.state.samples[0].path,'/edited.wav');assert.equal(h.S.state.project.instruments[0].samplePath,'/edited.wav');assert.equal(h.S.state.undo[0].project.instruments[0].samplePath,'/old.wav');assert.equal(h.native.instruments[0].samplePath,'/edited.wav');
    console.log('PASS shared module API applies a pure proposal then canonical edit with one Undo and acknowledged PCM metadata');
  }
  console.log('PASS 15 instrument transaction regression groups');
}
run().catch(error=>{console.error(error);process.exitCode=1;});
