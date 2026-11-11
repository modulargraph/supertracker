'use strict';
// "Sometimes after stopping, the play button doesn't work": the real app transport (togglePlay/stop/
// recordWithCountIn, Space, the Play button) against a fake native bridge that models the engine:
// Play/Stop/count-in are queued and only applied when the audio thread renders, and the periodic
// status reports what AudioHost.status() publishes. Covers rapid Play/Stop, Stop then Play while the
// Stop is still being confirmed, Stop during count-in, Stop while Play waits for assets or a project
// sync, failed Stops, and status events that are stale or arrive mid-request.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
const deferred=()=>{let resolve,reject;const promise=new Promise((yes,no)=>{resolve=yes;reject=no;});return{promise,resolve,reject};};
const tick=async()=>{for(let i=0;i<40;i++)await Promise.resolve();};

// Engine model. `stale:true` is the old status contract (rendered state only); the fixed engine reports
// the requested transport while a queued Play/Stop/count-in awaits the render thread (transportPending).
function engineModel({stale=false}={}){
 const m={queue:[],rendered:{playing:false,countIn:0},rendering:true};
 m.render=()=>{if(!m.rendering)return;for(const c of m.queue.splice(0)){if(c.type==='play')m.rendered={playing:!c.countIn,countIn:c.countIn?c.countIn*4:0};else m.rendered={playing:false,countIn:0};}};
 m.status=()=>{if(!stale&&m.queue.length){const last=m.queue.at(-1);return{playing:last.type==='play'&&!last.countIn,countIn:0,transportPending:true};}return{...m.rendered,transportPending:false};};
 return m;
}
function harness({stale=false}={}){
 const nodes=new Map(),calls=[],handlers=new Map(),timers=new Map();let nextTimer=0,clock=0,document;
 function node(selector){
  const id=String(selector).replace(/^#/,'');if(nodes.has(id))return nodes.get(id);
  const el={id,tagName:id==='transport-mode'?'SELECT':id==='tracker'?'DIV':'BUTTON',value:'',type:'',dataset:{},attributes:{},style:{},innerHTML:'',textContent:'',disabled:false,tabIndex:0,hidden:false,offsetParent:{},
   classList:{toggle(){},add(){},remove(){}},setAttribute(name,value){this.attributes[name]=String(value);},removeAttribute(name){delete this.attributes[name];},addEventListener(){},querySelectorAll(){return[];},querySelector(){return null;},focus(){document.activeElement=this;},select(){},scrollIntoView(){},setSelectionRange(){},click(){return this.onclick?.({target:this});},
   closest(selector){if(selector==='.tracker')return this.id==='tracker'?this:null;return null;}};
  nodes.set(id,el);return el;
 }
 document={body:node('body'),activeElement:null,querySelector(q){return q==='.context-menu'?null:node(q);},querySelectorAll(){return[];},getElementById:node,addEventListener(){},createElement:node};document.body.tagName='BODY';
 const engine=engineModel({stale});
 // AudioHost: Play (after assets) starts the device and queues; Stop queues; replies carry status().
 handlers.set('transport.play',payload=>{engine.rendering=true;engine.queue.push({type:'play',countIn:payload.countIn||0});return engine.status();});
 handlers.set('transport.stop',()=>{engine.queue.push({type:'stop'});return engine.status();});
 handlers.set('engine.panic',()=>engine.status());
 const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(message);Promise.resolve().then(()=>handlers.has(message.command)?handlers.get(message.command)(message.payload)??{}:noteRecordingHost(window)(message)??{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));}})};
 const context=vm.createContext({window,document,console,Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1080,innerHeight:700,setTimeout(fn,delay=0){timers.set(++nextTimer,{fn,at:clock+delay});return nextTimer;},clearTimeout(id){timers.delete(id);}});
 web.load(context,'app.js');const ST=window.ST,state=ST.state;state.project=ST.demoProject();ST.refresh();
 const h={ST,state,window,node,calls,handlers,engine,
  status(payload=engine.status()){window.nativeEvent({type:'status',payload});},
  // One audio buffer, then the 50 ms status timer.
  frame(){engine.render();h.status();},
  advance(milliseconds){clock+=milliseconds;for(const [id,timer]of [...timers])if(timer.at<=clock){timers.delete(id);timer.fn();}},
  space(modifiers={}){const event={target:node('tracker'),key:' ',code:'Space',metaKey:false,ctrlKey:false,altKey:false,shiftKey:false,repeat:false,preventDefault(){this.defaultPrevented=true;},stopPropagation(){},...modifiers};ST.keyboard(event);return event;},
  starts:()=>calls.filter(x=>x.command==='transport.play').length,
  stops:()=>calls.filter(x=>x.command==='transport.stop').length,
  button:()=>node('play').textContent};
 return h;
}
const tests=[],test=(name,run)=>tests.push({name,run});

test('Shift-Space starts the current pattern at row zero and Song at its first block',async()=>{
 for(const mode of ['pattern','song']){
  const h=harness();h.state.playbackMode=mode;h.state.row=12;h.state.sequence=1;h.state.project.activePattern=1;
  h.space({shiftKey:true});await tick();
  const play=h.calls.find(c=>c.command==='transport.play').payload;
  assert.equal(play.row,0);assert.equal(play.mode,mode);assert.equal(play.sequence,mode==='song'?0:1);
  assert.equal(play.pattern,mode==='song'?h.state.project.sequence[0]:h.ST.pattern().id);
  assert.match(h.node('play').title,/⇧Space/);
 }
});
test('play from start while playing waits for confirmed Stop and retains its original start target',async()=>{
 const h=harness(),gate=deferred();h.state.row=8;await h.ST.perform('play');h.frame();h.handlers.set('transport.stop',()=>gate.promise);
 const restart=h.ST.perform('playFromStart');await tick();assert.equal(h.starts(),1);
 h.state.project.activePattern=1;h.state.row=15;gate.resolve({});assert.equal(await restart,true);
 assert.equal(h.starts(),2);const play=h.calls.filter(c=>c.command==='transport.play').at(-1).payload;assert.equal(play.pattern,0);assert.equal(play.row,0);
});
test('explicit restart replaces an earlier queued cursor start while Stop is pending',async()=>{
 const h=harness(),gate=deferred();h.handlers.set('transport.stop',()=>gate.promise);
 const stopping=h.ST.perform('stop');await tick();h.state.row=13;
 const ordinary=h.ST.perform('play');const beginning=h.ST.perform('playFromStart');
 gate.resolve({});await stopping;await ordinary;await beginning;
 assert.equal(h.starts(),1);assert.equal(h.calls.find(c=>c.command==='transport.play').payload.row,0);
});
test('restart queued during Stop is canceled by later Stop, failed Stop, or project replacement',async()=>{
 for(const action of ['cancel','fail','replace']){
  const h=harness(),gate=deferred();await h.ST.perform('play');h.frame();h.handlers.set('transport.stop',()=>gate.promise);
  const restart=h.ST.perform('playFromStart');await tick();
  let cancel;if(action==='cancel')cancel=h.ST.perform('stop');if(action==='replace')h.state.project=h.ST.demoProject();
  if(action==='fail')gate.reject(new Error('stop failed'));else gate.resolve({});
  assert.equal(await restart,false);if(cancel)await cancel;assert.equal(h.starts(),1);
 }
});
test('restart supersedes a pending prepared Play without allowing the old request to restart',async()=>{
 const h=harness(),gate=deferred();let first=true;
 h.handlers.set('transport.play',payload=>{if(first){first=false;return gate.promise;}return {};});
 h.state.row=9;const old=h.ST.perform('play');await tick();assert.equal(h.starts(),1);
 const restart=h.ST.perform('playFromStart');assert.equal(await restart,true);gate.resolve({});assert.equal(await old,false);
 assert.equal(h.starts(),2);assert.equal(h.calls.filter(c=>c.command==='transport.play').at(-1).payload.row,0);
});

test('reproduction: a status taken before the render applied Stop flipped the next Play into another Stop',async()=>{
 // Old contract: proves the failure mode this suite guards against.
 {const h=harness({stale:true});await h.ST.perform('play');h.frame();assert.equal(h.state.playing,true);
  await h.ST.perform('stop');assert.equal(h.state.playing,false);h.status();assert.equal(h.state.playing,true,'stale rendered status still says playing');
  await h.ST.perform('play');assert.equal(h.starts(),1,'the Play press only sent another Stop');assert.equal(h.stops(),2);}
 // Fixed contract: the same timing plays.
 {const h=harness();await h.ST.perform('play');h.frame();await h.ST.perform('stop');h.status();assert.equal(h.state.playing,false);assert.equal(h.button(),'▶');
  assert.equal(await h.ST.perform('play'),true);assert.equal(h.starts(),2);assert.equal(h.stops(),1);h.frame();assert.equal(h.state.playing,true);}
});

test('audio device stopped rendering: Stop sticks across status ticks and Play restarts the device',async()=>{
 const h=harness();await h.ST.perform('play');h.frame();h.engine.rendering=false;// device change/sleep: no more renders
 await h.ST.perform('stop');for(let i=0;i<5;i++){h.frame();assert.equal(h.state.playing,false);assert.equal(h.button(),'▶');}
 assert.equal(await h.ST.perform('play'),true);assert.equal(h.starts(),2);for(let i=0;i<3;i++){h.frame();assert.equal(h.state.playing,true);}
 assert.equal(await h.ST.perform('play'),true,'Space/Play now pauses');assert.equal(h.state.playing,false);h.frame();assert.equal(h.state.playing,false);
});

test('Play pressed while Stop and panic are still being confirmed starts once they are (button, Space, palette)',async()=>{
 for(const press of ['button','space','perform']){
  const h=harness(),stop=deferred(),panic=deferred();await h.ST.perform('play');h.frame();
  h.handlers.set('transport.stop',()=>stop.promise);h.handlers.set('engine.panic',()=>panic.promise);
  const stopping=h.ST.perform('stop');await tick();
  const pressPlay=()=>press==='button'?h.node('play').click():press==='space'?(h.space(),null):h.ST.perform('play');
  const first=pressPlay();h.frame();const second=pressPlay();await tick();assert.equal(h.starts(),1,'nothing starts before Stop is confirmed');
  h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());await tick();assert.equal(h.starts(),1,'nor before the panic reply');
  panic.resolve({});assert.equal(await stopping,true);if(first)assert.equal(await first,true);if(second)assert.equal(await second,true);await tick();
  assert.equal(h.starts(),2,press+': exactly one restart');assert.equal(h.state.playing,true);h.frame();assert.equal(h.state.playing,true);
 }
});

test('rapid Play/Stop presses always settle, and a final Play plays',async()=>{
 const h=harness(),pending=[];
 for(let i=0;i<8;i++){pending.push(h.ST.perform('play'));if(i%2)h.frame();pending.push(h.ST.perform('stop'));if(i%3===0)h.frame();await Promise.resolve();}
 await Promise.all(pending);await tick();h.frame();assert.equal(h.state.playing,false);assert.equal(h.button(),'▶');
 assert.equal(await h.ST.perform('play'),true);h.frame();assert.equal(h.state.playing,true);assert.equal(h.calls.at(-1).command,'transport.play');
 // Play/Play/Play toggles cleanly too.
 for(let i=0;i<5;i++){await h.ST.perform('play');h.frame();}assert.equal(h.state.playing,false);assert.equal(await h.ST.perform('play'),true);h.frame();assert.equal(h.state.playing,true);
});

test('Stop (or Play) during a count-in leaves nothing armed, and Play then plays without a count-in',async()=>{
 for(const cancel of ['stop','play']){
  const h=harness();assert.equal(await h.ST.recordWithCountIn(1),true);assert(h.state.countIn);h.frame();assert.equal(h.state.countIn.remaining,4);assert.equal(h.state.playing,false);
  if(cancel==='play')await h.ST.perform('play');else await h.ST.perform('stop');
  assert.equal(h.state.countIn,null);
  // A count-in status taken before the cancel arrives late: ignored.
  h.status({playing:false,countIn:3});assert.equal(h.state.countIn,null);h.frame();
  assert.equal(await h.ST.perform('play'),true,cancel);const play=h.calls.filter(x=>x.command==='transport.play').at(-1);assert.equal('countIn' in play.payload,false);
  h.frame();assert.equal(h.state.playing,true);
 }
});

test('Stop while Play waits for project assets: the superseded native Play is silent and the next Play works',async()=>{
 const h=harness(),assets=deferred();let first=true;
 h.handlers.set('transport.play',payload=>{if(first){first=false;return assets.promise;}h.engine.queue.push({type:'play',countIn:0});return h.engine.status();});
 const starting=h.ST.perform('play');await tick();assert.equal(h.button(),'…');
 assert.equal(await h.ST.perform('stop'),true);assert.equal(h.button(),'▶');
 assets.reject(Error('The Play request was superseded by a transport command or project change.'));assert.equal(await starting,false);
 assert.doesNotMatch(h.node('toast-region').innerHTML,/superseded/);
 assert.equal(await h.ST.perform('play'),true);h.frame();assert.equal(h.state.playing,true);
});

test('Space while Play waits for assets cancels; the next Space plays',async()=>{
 const h=harness(),assets=deferred();let first=true;
 h.handlers.set('transport.play',()=>{if(first){first=false;return assets.promise;}h.engine.queue.push({type:'play',countIn:0});return h.engine.status();});
 const starting=h.ST.perform('play');await tick();h.space();await tick();assert.equal(h.stops(),1);assets.reject(Error('superseded'));assert.equal(await starting,false);await tick();
 h.space();await tick();h.frame();assert.equal(h.state.playing,true);assert.equal(h.starts(),2);
});

test('Stop while a project sync is in flight, then Play works',async()=>{
 const h=harness(),sync=deferred();let first=true;h.handlers.set('engine.project',()=>{if(first){first=false;return sync.promise;}return{};});
 const starting=h.ST.perform('play');await tick();assert.equal(await h.ST.perform('stop'),true);sync.resolve({});assert.equal(await starting,false);assert.equal(h.starts(),0);
 assert.equal(await h.ST.perform('play'),true);h.frame();assert.equal(h.state.playing,true);
});

test('status events delivered while Stop is in flight or out of order cannot strand the transport',async()=>{
 const h=harness(),stop=deferred(),panic=deferred();await h.ST.perform('play');h.frame();
 h.handlers.set('transport.stop',()=>stop.promise);h.handlers.set('engine.panic',()=>panic.promise);const stopping=h.ST.perform('stop');await tick();
 h.status({playing:true,row:4,countIn:0});// taken before Stop was handled
 h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());await tick();h.status();h.status({playing:true,row:5});// older one delivered late
 panic.resolve({});assert.equal(await stopping,true);assert.equal(h.state.playing,false);h.frame();assert.equal(h.state.playing,false);
 assert.equal(await h.ST.perform('play'),true);assert.equal(h.starts(),2);h.status({playing:false,transportPending:false});// pre-Play status delivered after the reply
 h.frame();assert.equal(h.state.playing,true);
});

test('a queued Play is canceled by a later Stop, a failed Stop or a project change, and never leaks',async()=>{
 for(const cancel of ['stop','fail','project']){
  const h=harness(),stop=deferred();await h.ST.perform('play');h.frame();h.handlers.set('transport.stop',()=>stop.promise);
  const stopping=h.ST.perform('stop');await tick();const queued=h.ST.perform('play');
  if(cancel==='stop')h.ST.perform('stop');if(cancel==='project')h.ST.acceptProject(h.ST.demoProject());
  if(cancel==='fail')stop.reject(Error('Fixture stop failure'));else{h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());}
  await stopping;assert.equal(await queued,false,cancel);await tick();assert.equal(h.starts(),1,cancel+': no restart');
  h.handlers.delete('transport.stop');h.handlers.set('transport.stop',()=>{h.engine.queue.push({type:'stop'});return h.engine.status();});
  if(cancel==='fail'){assert.match(h.node('toast-region').innerHTML,/stop could not be confirmed/);assert.equal(await h.ST.perform('stop'),true);}
  h.frame();assert.equal(await h.ST.perform('play'),true,cancel+': explicit Play works');h.frame();assert.equal(h.state.playing,true);
 }
});

test('Play pressed during the cancellation of a timed-out Play starts after it, once',async()=>{
 const h=harness(),play=deferred(),stop=deferred();let firstPlay=true,firstStop=true;
 h.handlers.set('transport.play',()=>{if(firstPlay){firstPlay=false;return play.promise;}h.engine.queue.push({type:'play',countIn:0});return h.engine.status();});
 h.handlers.set('transport.stop',()=>{if(firstStop){firstStop=false;return stop.promise;}h.engine.queue.push({type:'stop'});return h.engine.status();});
 const starting=h.ST.perform('play');await tick();h.advance(15000);await tick();assert.equal(h.stops(),1);
 const queued=h.ST.perform('play');await tick();assert.equal(h.starts(),1);
 h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());assert.equal(await starting,false);assert.match(h.node('toast-region').innerHTML,/took too long/i);
 assert.equal(await queued,true);assert.equal(h.starts(),2);h.frame();assert.equal(h.state.playing,true);
 play.resolve({});await tick();assert.equal(h.state.playing,true);assert.equal(h.starts(),2,'the late reply restarts nothing');
});

test('queued Play retains the pattern, row and mode selected when it was pressed',async()=>{
 const h=harness(),stop=deferred();await h.ST.perform('play');h.frame();h.handlers.set('transport.stop',()=>stop.promise);
 const stopping=h.ST.perform('stop');await tick();const owner=h.ST.pattern();h.state.row=6;const queued=h.ST.perform('play');
 const other=JSON.parse(JSON.stringify(owner));other.id=42;h.state.project.patterns.push(other);h.state.project.activePattern=42;h.state.row=12;
 h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());assert.equal(await stopping,true);assert.equal(await queued,true);
 const start=h.calls.filter(x=>x.command==='transport.play').at(-1).payload;assert.equal(start.pattern,owner.id);assert.equal(start.row,6);assert.equal(start.mode,'pattern');
});
test('queued Play refuses a removed or shortened captured pattern',async()=>{
 for(const change of ['remove','shorten']){const h=harness(),stop=deferred();await h.ST.perform('play');h.frame();h.handlers.set('transport.stop',()=>stop.promise);
 const stopping=h.ST.perform('stop');await tick();const owner=h.ST.pattern();h.state.row=12;const queued=h.ST.perform('play');
 if(change==='remove'){const other=JSON.parse(JSON.stringify(owner));other.id=42;h.state.project.patterns=h.state.project.patterns.filter(x=>x!==owner);h.state.project.patterns.push(other);h.state.project.activePattern=42;}else owner.length=4;
 h.engine.queue.push({type:'stop'});stop.resolve(h.engine.status());await stopping;assert.equal(await queued,false);assert.equal(h.starts(),1);}
});

let finished=false;
process.on('exit',()=>{if(!finished){console.error('FAIL transport restart: a test never settled (a Play/Stop promise hung)');process.exitCode=1;}});
(async()=>{let failures=0;for(const {name,run} of tests){try{await run();console.log('PASS '+name);}catch(error){failures++;console.error('FAIL '+name+'\n'+error.stack);}}finished=true;if(failures)throw Error(`${failures} transport restart groups failed`);console.log(`Transport restart: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
