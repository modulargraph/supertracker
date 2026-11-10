// Actual app + Maschine controller save/history/quit boundaries; no native I/O.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
function harness(){
  const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let context,clock=0,nextTimer=1;
  function advance(milliseconds){clock+=milliseconds;for(const [id,timer] of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}}
  function element(id){if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',value:'',dataset:{},attributes:{},classList:{toggle(){},add(){},remove(){}},focus(){},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(name,value){this.attributes[name]=String(value);},click(){return this.onclick?.();}});return nodes.get(id);}
  const document={body:element('body'),activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(message);const handler=handlers.get(message.command);Promise.resolve().then(()=>handler?handler(message.payload):{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
  context=vm.createContext({window,document,console,TextEncoder,TextDecoder,setTimeout:(fn,delay=0)=>{const id=nextTimer++;timers.set(id,{fn,at:clock+delay});return id;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800});
  web.load(context, 'app.js', { path: process.env.ST_APP_SOURCE });
  const ST=window.ST;ST.state.project=ST.demoProject();ST.state.project.name='Unsaved piece';ST.state.dirty=true;
  handlers.set('project.new',()=>null);handlers.set('project.open',()=>({project:{name:'Opened piece'}}));handlers.set('project.importMIDI',()=>({project:{name:'Imported MIDI'}}));handlers.set('project.save',payload=>({saved:true,project:payload.project,path:'/tmp/piece.supertracker'}));
  return {ST,window,document,calls,handlers,element,advance,context,load(name){web.load(context, name);}};
}
async function tick(){for(let i=0;i<12;i++)await Promise.resolve();}
const tests=[];const test=(name,run)=>tests.push({name,run});const copy=x=>JSON.parse(JSON.stringify(x));
function controller(){
 const h=harness(),s=h.ST.state;
 h.window.STStudio={renderMixer:()=>'',bindMixer(){},event(){}};
 h.load('mk3-screens.js'); h.load('mk3.js'); const m=h.window.STMK3; s.view='mixer';m.state.screen='mixer';m.state.lastView='mixer';s.undo=[];s.redo=[];s.dirty=false;
 const write=delta=>m.page().knobs[0].turn(delta);
 return {...h,m,write};
}
function quitScript(name){const swift=fs.readFileSync(process.env.ST_BRIDGE_SOURCE||path.join(__dirname,'../Sources/SuperTracker/Bridge.swift'),'utf8');const marker=`private static let ${name} = """`;const start=swift.indexOf(marker);assert(start>=0);return swift.slice(start+marker.length,swift.indexOf('"""',start+marker.length));}
test('Save settles an unfinished MK3 gesture exactly once',async()=>{
 const h=controller(),before=h.ST.state.project.tracks[0].volume;h.write(-.2);assert.equal(h.ST.state.project.tracks[0].volume,before);
 assert.equal(await h.ST.projectAction('save'),true);const saved=h.calls.find(x=>x.command==='project.save');assert(saved.payload.project.tracks[0].volume<before);assert.equal(h.ST.state.undo.length,1);assert.equal(h.m.hasPendingEdits,false);
});
test('Undo captures then undoes the unfinished knob gesture',async()=>{
 const h=controller(),before=h.ST.state.project.tracks[0].volume;h.ST.mutate('Earlier edit',p=>p.name='Keep name');h.write(-.2);await h.ST.perform('undo');
 assert.equal(h.ST.state.project.name,'Keep name');assert.equal(h.ST.state.project.tracks[0].volume,before);assert.equal(h.ST.state.undo.length,1);assert.equal(h.m.hasPendingEdits,false);
});
test('Rejected MK3 capture prevents Save and preserves the draft for retry',async()=>{
 const h=controller();h.write(-.2);h.window.STRouting={isApplying:true};assert.equal(await h.ST.projectAction('save'),false);assert(h.m.hasPendingEdits);assert.equal(h.calls.some(x=>x.command==='project.save'),false);
 h.window.STRouting.isApplying=false;assert.equal(await h.ST.projectAction('save'),true);assert.equal(h.m.hasPendingEdits,false);assert.equal(h.ST.state.undo.length,1);
});
test('Normal native previews do not fragment one knob gesture into Undo steps',async()=>{
 const h=controller();h.write(-.1);await h.ST.syncEngineProject(()=>copy(h.ST.state.project));h.advance(60);await tick();h.write(-.1);await h.ST.syncEngineProject(()=>copy(h.ST.state.project));
 assert(h.m.hasPendingEdits);assert.equal(h.ST.state.undo.length,0);assert.equal(await h.m.flushEdits(),true);assert.equal(h.ST.state.undo.length,1);
});
test('Knob activity arriving during Save stays dirty and remains pending',async()=>{
 const h=controller();h.write(-.1);const owner=h.ST.state.project;let resolveSave;h.handlers.set('project.save',p=>new Promise(resolve=>resolveSave=()=>resolve({saved:true,project:copy(p.project),path:'/tmp/piece.supertracker'})));
 const saving=h.ST.projectAction('save');for(let i=0;i<40&&!resolveSave;i++)await tick();assert(resolveSave);h.write(-.1);resolveSave();assert.equal(await saving,false);assert.equal(h.ST.state.project,owner);assert.equal(h.ST.state.dirty,true);assert(h.m.hasPendingEdits);
});
test('Native quit flush captures an unfinished MK3 gesture before snapshot',async()=>{
 const h=controller(),before=h.ST.state.project.tracks[0].volume;h.write(-.2);await vm.runInContext('(async()=>{'+quitScript('flushEditsScript')+'})()',h.context);
 h.window.__supertracker={getProject:()=>copy(h.ST.state.project)};const result=vm.runInContext(quitScript('projectSnapshotScript'),h.context);assert.equal(result.ok,true);assert(result.project.tracks[0].volume<before);assert.equal(h.ST.state.undo.length,1);
});
test('Native quit refuses a checkpoint when controller validation still rejects',async()=>{
 const h=controller();h.write(-.2);h.window.STFreeze={checkMutation:()=>({message:'Unfreeze first'}),explain(){}};await vm.runInContext('(async()=>{'+quitScript('flushEditsScript')+'})()',h.context);h.window.__supertracker={getProject:()=>copy(h.ST.state.project)};
 const result=vm.runInContext(quitScript('projectSnapshotScript'),h.context);assert.equal(result.ok,false);assert(h.m.hasPendingEdits);
});

// Constant tempo (tempo maps were removed): the MK3 tempo overlay edits project.bpm as one pending gesture.
function timedController(){
 const h=controller();h.ST.state.project.bpm=120;
 h.m.state.overlay='tempo';h.m.page().main.turn(5);return h;
}
test('MK3 tempo capture settles its own pending controller gesture',async()=>{
 const h=timedController();let settled=false,result;h.m.flushEdits().then(x=>{settled=true;result=x;});
 for(let i=0;i<100&&!settled;i++)await tick();
 assert.equal(settled,true,'MK3 tempo capture deadlocked');
 assert.equal(result,true,h.element('#toast-region').innerHTML+' '+JSON.stringify(h.calls.map(x=>x.command)));assert.equal(h.ST.state.project.bpm,125);assert.equal(h.ST.state.undo.length,1);
});
test('Save can capture a pending MK3 tempo gesture',async()=>{
 const h=timedController();let settled=false,result;h.ST.projectAction('save').then(x=>{settled=true;result=x;});
 for(let i=0;i<100&&!settled;i++)await tick();assert.equal(settled,true);assert.equal(result,true);
 assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.bpm,125);
});

test('pending tempo settles during Export preparation',async()=>{
 const h=timedController();h.window.STExport={isBusy:true,isPreparing:true};
 assert.equal(await h.ST.captureNativeEdits(),true);assert.equal(h.ST.state.project.bpm,125);assert.equal(h.m.hasPendingEdits,false);
});
test('export preparation captures pending controller tempo before rendering',async()=>{
 const h=timedController();h.load('export.js');let sawRender=false;
 h.handlers.set('audio.export',payload=>{sawRender=true;assert.equal(h.window.STExport.isPreparing,false);assert.equal(h.ST.state.project.bpm,125);return {paths:[payload.path]};});
 const result=await h.window.STExport.exportTo({path:'/tmp/mk3-tempo.wav'});
 assert(sawRender);assert.equal(result.paths[0],'/tmp/mk3-tempo.wav');assert.equal(h.m.hasPendingEdits,false);assert.equal(h.ST.state.undo.length,1);
 assert.equal(h.window.STExport.isBusy,false);assert.equal(h.window.STExport.isPreparing,false);
});
(async()=>{let failed=0;for(const t of tests){try{await t.run();console.log('PASS '+t.name);}catch(e){failed++;console.error('FAIL '+t.name+'\n'+e.stack);}}console.log(`${tests.length-failed}/${tests.length} MK3 persistence groups passed`);process.exitCode=failed?1:0;})().catch(e=>{console.error(e);process.exitCode=1;});
