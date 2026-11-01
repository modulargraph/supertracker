// Actual app + live recorder save/history/quit boundaries; no native I/O.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const web = require('./helpers/web');
function harness(){
  const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let context,clock=0,nextTimer=1;
  function advance(milliseconds){clock+=milliseconds;for(const [id,timer] of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}}
  function element(id){if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',value:'',dataset:{},attributes:{},classList:{toggle(){},add(){},remove(){}},focus(){},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(name,value){this.attributes[name]=String(value);},click(){return this.onclick?.();}});return nodes.get(id);}
  const document={body:element('body'),activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  const window={addEventListener(){},webkit:{messageHandlers:{supertracker:{postMessage(message){calls.push(message);const handler=handlers.get(message.command);Promise.resolve().then(()=>handler?handler(message.payload):{}).then(result=>window.nativeReceive({id:message.id,ok:true,result}),error=>window.nativeReceive({id:message.id,ok:false,error:error.message}));}}}}};
  context=vm.createContext({window,document,console,setTimeout:(fn,delay=0)=>{const id=nextTimer++;timers.set(id,{fn,at:clock+delay});return id;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800});
  web.load(context, 'app.js', { path: process.env.ST_APP_SOURCE });
  const ST=window.ST;ST.state.project=ST.demoProject();ST.state.project.name='Unsaved piece';ST.state.dirty=true;
  handlers.set('project.new',()=>null);handlers.set('project.open',()=>({project:{name:'Opened piece'}}));handlers.set('project.importMIDI',()=>({project:{name:'Imported MIDI'}}));handlers.set('project.save',payload=>({saved:true,project:payload.project,path:'/tmp/piece.supertracker'}));
  return {ST,window,document,calls,handlers,element,advance,context,load(name){web.load(context, name);}};
}
async function tick(){for(let i=0;i<12;i++)await Promise.resolve();}
const tests=[];const test=(name,run)=>tests.push({name,run});const copy=x=>JSON.parse(JSON.stringify(x));
function recording(){
 const h=harness(),s=h.ST.state;
 h.window.STStudio={renderMixer:()=>'',bindMixer(){},event(){},target:(owner,target)=>target==='volume'?{target,name:'Volume',min:0,max:1.5,step:.01,value:.8}:null};
 h.load('automation-record.js');s.view='mixer';s.project.patterns.forEach(p=>p.automation=[]);s.undo=[];s.redo=[];s.dirty=false;
 const r=h.window.STAutomationRecord;
 const write=value=>{s.playing=true;s.status={playing:true,pattern:h.ST.pattern().id,row:2,position:2,bpm:120};s.lastStatusAt=Date.now();r.setMode('touch',true);const ref=r.makeRef(0,'volume','Volume');assert(r.begin(ref,value));s.status.row=4;s.status.position=4;assert(r.value(ref,value+.1));r.end(ref);};
 return {...h,r,write};
}
function recorded(p){return p.patterns.find(x=>x.id===p.activePattern).automation.find(x=>x.target==='volume');}
function quitScript(name){const swift=fs.readFileSync(process.env.ST_BRIDGE_SOURCE||path.join(__dirname,'../Sources/SuperTracker/Bridge.swift'),'utf8');const marker=`private static let ${name} = """`;const start=swift.indexOf(marker);assert(start>=0);return swift.slice(start+marker.length,swift.indexOf('"""',start+marker.length));}
test('Save captures a pending automation take as one history entry',async()=>{
 const h=recording();h.write(.31);assert.equal(recorded(h.ST.state.project),undefined);assert.equal(await h.ST.projectAction('save'),true);
 const saved=h.calls.find(x=>x.command==='project.save');assert(recorded(saved.payload.project),'Save omitted the current automation take');assert(recorded(saved.payload.project).points.some(x=>Math.abs(x.value-.41)<.001));assert.equal(h.ST.state.undo.length,1);assert.equal(h.r.hasPendingEdits,false);
});
test('Undo first commits then undoes only the pending take',async()=>{
 const h=recording();h.ST.mutate('Earlier name edit',p=>p.name='Keep this name');h.write(.4);await h.ST.perform('undo');
 assert.equal(h.ST.state.project.name,'Keep this name','Undo skipped the pending take and undid the preceding edit');assert.equal(recorded(h.ST.state.project),undefined);assert.equal(h.ST.state.undo.length,1);assert.match(h.ST.state.redo[0].label,/Record automation/);
});
test('Refused automation settlement prevents Save and preserves the take',async()=>{
 const h=recording();h.write(.3);h.window.STRouting={isApplying:true};assert.equal(await h.ST.projectAction('save'),false);assert.equal(h.calls.some(x=>x.command==='project.save'),false);assert(h.r.hasPendingEdits);assert.equal(h.ST.state.undo.length,0);
});
test('Export preparation captures automation without rendering or discarding the take',async()=>{
 const h=recording();h.write(.5);h.window.STExport={isBusy:true};assert.equal(await h.ST.captureNativeEdits(),true);assert(recorded(h.ST.state.project));assert.equal(h.r.hasPendingEdits,false);assert.equal(h.ST.state.undo.length,1);
});
test('Recording arriving during Save keeps newer edits dirty and retains their owner',async()=>{
 const h=recording();h.write(.2);const owner=h.ST.state.project;let resolveSave;h.handlers.set('project.save',p=>new Promise(resolve=>resolveSave=()=>resolve({saved:true,project:copy(p.project),path:'/tmp/take.supertracker'})));
 const saving=h.ST.projectAction('save');for(let i=0;i<40&&!resolveSave;i++)await tick();assert(resolveSave);h.write(.7);resolveSave();assert.equal(await saving,false);assert.equal(h.ST.state.project,owner);assert.equal(h.ST.state.dirty,true);assert(h.r.hasPendingEdits);
});
test('Normal engine previews preserve a single pending take',async()=>{
 const h=recording();h.write(.3);await h.ST.syncEngineProject(()=>copy(h.ST.state.project));assert(h.r.hasPendingEdits);assert.equal(h.ST.state.undo.length,0);
});
test('Native quit flush commits the pending take before its checkpoint',async()=>{
 const h=recording();h.write(.35);await vm.runInContext('(async()=>{'+quitScript('flushEditsScript')+'})()',h.context);assert(recorded(h.ST.state.project),'Quit checkpoint would omit automation');
 h.window.__supertracker={getProject:()=>copy(h.ST.state.project)};const result=vm.runInContext(quitScript('projectSnapshotScript'),h.context);assert.equal(result.ok,true);assert(recorded(result.project));
});
test('Native quit refuses an automation take that cannot commit',async()=>{
 const h=recording();h.write(.35);h.window.STFreeze={checkMutation:()=>({message:'Unfreeze this track first.'})};await vm.runInContext('(async()=>{'+quitScript('flushEditsScript')+'})()',h.context);h.window.__supertracker={getProject:()=>copy(h.ST.state.project)};
 const result=vm.runInContext(quitScript('projectSnapshotScript'),h.context);assert.equal(result.ok,false,'Quit accepted a checkpoint omitting the pending take');assert(h.r.hasPendingEdits);
});
(async()=>{let failed=0;for(const t of tests){try{await t.run();console.log('PASS '+t.name);}catch(e){failed++;console.error('FAIL '+t.name+'\n'+e.stack);}}console.log(`${tests.length-failed}/${tests.length} automation persistence groups passed`);process.exitCode=failed?1:0;})().catch(e=>{console.error(e);process.exitCode=1;});
