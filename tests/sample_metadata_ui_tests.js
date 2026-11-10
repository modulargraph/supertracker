// Sample metadata ownership/cache hydration against the actual app, with native requests mocked.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
function harness(){
  const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let context,clock=0,nextTimer=1;
  function advance(milliseconds){clock+=milliseconds;for(const [id,timer] of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}}
  function element(id){if(!nodes.has(id))nodes.set(id,{id,innerHTML:'',value:'',dataset:{},attributes:{},classList:{toggle(){},add(){},remove(){}},focus(){},select(){},scrollIntoView(){},closest(){return null;},querySelectorAll(){return[];},addEventListener(){},setAttribute(name,value){this.attributes[name]=String(value);},click(){return this.onclick?.();}});return nodes.get(id);}
  const document={body:element('body'),activeElement:null,querySelector:element,querySelectorAll:()=>[],getElementById:element,addEventListener(){},createElement:element};
  const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(message);const handler=handlers.get(message.command);Promise.resolve().then(()=>handler?handler(message.payload):{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
  context=vm.createContext({window,document,console,setTimeout:(fn,delay=0)=>{const id=nextTimer++;timers.set(id,{fn,at:clock+delay});return id;},clearTimeout:id=>timers.delete(id),Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800});
  web.load(context, 'app.js');
  const ST=window.ST;ST.state.project=ST.demoProject();ST.state.project.instruments.forEach(inst=>{inst.samplePath='';});/* The demo's built-in wavetables would hydrate too; these tests own every sample path. */ST.state.project.name='Unsaved piece';ST.state.dirty=true;
  handlers.set('project.new',()=>null);handlers.set('project.open',()=>({project:{name:'Opened piece'}}));handlers.set('project.importMIDI',()=>({project:{name:'Imported MIDI'}}));handlers.set('project.save',payload=>({saved:true,project:payload.project,path:'/tmp/piece.supertracker'}));
  return {ST,window,document,calls,handlers,element,advance,load(name){web.load(context, name);}};
}
async function tick(){for(let i=0;i<12;i++)await Promise.resolve();}
const copy=x=>JSON.parse(JSON.stringify(x));
const metadata=(path,frames=48000)=>({path,name:path.split('/').pop(),frames,sampleRate:48000,duration:frames/48000,channels:2,peaks:[.1,.2,.4]});
const sample=(h,index,path)=>Object.assign(h.ST.state.project.instruments[index],{type:'sample',samplePath:path});
async function settle(){for(let i=0;i<80;i++)await Promise.resolve();}
async function publish(h){await h.ST.syncEngineProject(()=>copy(h.ST.state.project));await settle();}
async function run(){
  {
    const h=harness(),meta=metadata('/same.wav');sample(h,1,meta.path);h.ST.state.samples[1]=meta;
    const replacement=copy(h.ST.state.project);replacement.instruments[0].type='sample';replacement.instruments[0].samplePath=meta.path;replacement.instruments[1].samplePath='';
    h.ST.acceptProject(replacement);assert.equal(h.ST.state.samples[0],meta);assert.equal(h.ST.state.samples[1],undefined);await publish(h);assert.equal(h.calls.filter(x=>x.command==='sample.info').length,0);
    console.log('PASS same-path session replacement reuses metadata across instrument indices without native decoder events');
  }
  {
    const h=harness(),meta=metadata('/second.wav');sample(h,1,meta.path);h.ST.state.samples[1]=meta;h.ST.state.instrument=0;
    h.ST.perform('removeInstrument');assert.equal(h.ST.state.project.instruments[0].samplePath,meta.path);assert.equal(h.ST.state.samples[0],meta);
    await h.ST.perform('undo');assert.equal(h.ST.state.project.instruments[1].samplePath,meta.path);assert.equal(h.ST.state.samples[1],meta);
    console.log('PASS instrument removal and Undo reindex cached metadata by canonical path');
  }
  {
    const h=harness(),old=metadata('/old.wav'),next=metadata('/new.wav',96000);sample(h,0,old.path);h.ST.state.samples[0]=old;h.ST.refresh();
    h.ST.mutate('different sample',p=>p.instruments[0].samplePath=next.path);assert.equal(h.ST.state.samples[0],undefined);
    h.window.nativeEvent({type:'sample.loaded',payload:{instrument:0,sample:next}});assert.equal(h.ST.state.samples[0],next);
    await h.ST.perform('undo');assert.equal(h.ST.state.samples[0],old);await h.ST.perform('redo');assert.equal(h.ST.state.samples[0],next);
    console.log('PASS sample path Undo and Redo restore the corresponding waveform rather than the last slot metadata');
  }
  {
    const h=harness(),meta=metadata('/current.wav');sample(h,0,meta.path);h.ST.state.samples[0]=meta;
    for(const payload of [{instrument:0,sample:metadata('/previous.wav')},{instrument:9,sample:meta},{instrument:0,sample:{frames:12}},{instrument:'0',sample:meta}])h.window.nativeEvent({type:'sample.loaded',payload});
    assert.equal(h.ST.state.samples[0],meta);assert.equal(h.ST.state.samples[9],undefined);
    console.log('PASS late wrong-path, wrong-slot, missing-path and malformed-index sample events are ignored');
  }
  {
    const h=harness(),meta=metadata('/native-cache.wav');sample(h,0,meta.path);h.handlers.set('sample.info',p=>p.instrument===0&&p.path===meta.path?meta:{available:false});
    await publish(h);assert.equal(h.ST.state.samples[0],meta);assert.equal(h.calls.filter(x=>x.command==='sample.info').length,1);
    for(let i=0;i<3;i++){h.ST.state.project.instruments[0].cutoff=1000+i;await publish(h);}assert.equal(h.calls.filter(x=>x.command==='sample.info').length,1);
    console.log('PASS acknowledged project hydrates an already-cached native sample once, without per-parameter metadata queries');
  }
  {
    const h=harness(),meta=metadata('/shared.wav');sample(h,0,meta.path);sample(h,1,meta.path);h.handlers.set('sample.info',()=>meta);await publish(h);
    assert.equal(h.calls.filter(x=>x.command==='sample.info').length,1);assert.equal(h.ST.state.samples[0],meta);assert.equal(h.ST.state.samples[1],meta);
    const latest=metadata(meta.path,96000);h.window.nativeEvent({type:'sample.loaded',payload:{instrument:0,sample:latest}});assert.equal(h.ST.state.samples[0],latest);assert.equal(h.ST.state.samples[1],latest);
    console.log('PASS identical paths share one metadata query and hydrate every canonical instrument slot');
  }
  {
    const h=harness();sample(h,0,'/pending.wav');let finish;h.handlers.set('sample.info',()=>new Promise(resolve=>finish=resolve));await publish(h);assert(finish);
    const replacement=copy(h.ST.state.project);h.ST.acceptProject(replacement);finish(metadata('/pending.wav',123));await settle();assert.equal(h.ST.state.samples[0],undefined);
    const fresh=metadata('/pending.wav',456);h.handlers.set('sample.info',()=>fresh);await publish(h);assert.equal(h.ST.state.samples[0],fresh);
    console.log('PASS late same-path hydration cannot cross project ownership; the replacement requests its own metadata');
  }
  {
    const h=harness();sample(h,0,'/before.wav');let finish;h.handlers.set('sample.info',()=>new Promise(resolve=>finish=resolve));await publish(h);
    h.ST.state.project.instruments[0].samplePath='/after.wav';finish(metadata('/before.wav'));await settle();assert.equal(h.ST.state.samples[0],undefined);
    h.handlers.set('sample.info',()=>({available:false}));await publish(h);await publish(h);assert.equal(h.calls.filter(x=>x.command==='sample.info').length,2);
    h.window.nativeEvent({type:'sample.loaded',payload:{instrument:0,sample:metadata('/after.wav')}});assert.equal(h.ST.state.samples[0].path,'/after.wav');
    console.log('PASS changed-path results are rejected and a pending native decode hydrates through its eventual event without query flooding');
  }
  {
    const h=harness();sample(h,0,'/source.wav');h.ST.state.samples[0]=metadata('/source.wav');
    h.handlers.set('project.save',payload=>{const p=copy(payload.project);p.instruments[0].samplePath='/Project Samples/collected.wav';return {saved:true,project:p};});
    h.handlers.set('sample.info',payload=>metadata(payload.path,32000));
    assert.equal(await h.ST.projectAction('save'),true);assert.equal(h.ST.state.samples[0],undefined,'old path metadata must not survive collected-path adoption');
    h.advance(100);await settle();assert.equal(h.calls.filter(x=>x.command==='engine.project').at(-1).payload.project.instruments[0].samplePath,'/Project Samples/collected.wav');assert.equal(h.ST.state.samples[0].path,'/Project Samples/collected.wav');assert.equal(h.ST.state.dirty,false);
    console.log('PASS Save collected-path adoption republishes the canonical path and hydrates it without marking the saved session dirty');
  }
  {
    const h=harness();sample(h,0,'/typing.wav');let paints=0,redraws=0,blur;
    h.window.STSampler={parts(){paints++;return {};},bind(){},redraw(){redraws++;}};h.ST.setView('instrument');
    const field=h.element('#inst-start');field.closest=()=>({});field.value='0.123';field.addEventListener=(name,fn)=>{if(name==='blur')blur=fn;};h.document.activeElement=field;
    const before=paints;h.handlers.set('sample.info',()=>metadata('/typing.wav'));await publish(h);
    assert.equal(paints,before);assert.equal(field.value,'0.123');assert.equal(redraws,1);assert(blur);h.document.activeElement=null;blur();assert.equal(paints,before,'blur must not repaint inside the pointerdown that moved focus');h.advance(0);assert.equal(paints,before+1);
    console.log('PASS hydration preserves a focused field draft and paints metadata on blur while redrawing the waveform in place');
  }
  {
    const h=harness();sample(h,0,'/refused.wav');h.handlers.set('engine.project',()=>false);await publish(h);assert.equal(h.calls.filter(x=>x.command==='sample.info').length,0);
    console.log('PASS a refused audio publication cannot trigger metadata hydration for an unaccepted project');
  }
  console.log('PASS 11 sample metadata regression groups');
}
run().catch(error=>{console.error(error);process.exitCode=1;});
