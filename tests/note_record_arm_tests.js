'use strict';
// Actual app entry points with deliberately delayed host acknowledgments. No audio is run.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const clone=x=>JSON.parse(JSON.stringify(x));
const tick=async()=>{for(let i=0;i<12;i++)await Promise.resolve();};
function harness({supported=true,native=true}={}){
  const nodes=new Map(),calls=[],arms=[];let ST;
  const document={activeElement:null,querySelector:s=>node(s.startsWith('#')?s.slice(1):s),querySelectorAll:()=>[],getElementById:id=>node(id),createElement:id=>node(id),addEventListener(){}};
  function node(id){if(!nodes.has(id)){const classes=new Set();nodes.set(id,{id,dataset:{},innerHTML:'',textContent:'',value:'',style:{},attributes:{},classList:{toggle(k,on){on?classes.add(k):classes.delete(k);},add:k=>classes.add(k),remove:k=>classes.delete(k),contains:k=>classes.has(k)},setAttribute(k,v){this.attributes[k]=v;},getAttribute(k){return this.attributes[k];},addEventListener(){},querySelectorAll(){return [];},scrollIntoView(){},select(){},focus(){document.activeElement=this;},closest(selector){return selector==='.tracker'&&id==='tracker'?this:null;}});}return nodes.get(id);}
  document.body=node('body');
  const window={addEventListener(){}};
  if(native)window.STHostTransport=hostStub({post(message){calls.push(clone(message));if(message.command==='transport.recordArm'){arms.push(message);return;}window.STHostTransport.deliver({id:message.id,ok:true,result:message.command==='engine.snapshot'?clone(ST.state.project):{}});}});
  const timers=new Map();let timerID=0;
  const context=vm.createContext({window,document,console,setTimeout(fn){const id=++timerID;timers.set(id,fn);return id;},clearTimeout:id=>timers.delete(id),innerWidth:1280,innerHeight:800,requestAnimationFrame:fn=>fn()});
  web.load(context,'app.js');ST=window.ST;
  Object.assign(ST.state,{project:ST.demoProject(),view:'pattern',track:0,instrument:0,row:9,status:supported?{recordArmSupported:true,noteRecordArmed:false}:{}});
  ST.refresh();node('tracker').focus();
  const key=(key,code,mods={})=>{const event={key,code,target:node('tracker'),metaKey:false,ctrlKey:false,altKey:false,shiftKey:false,repeat:false,preventDefault(){this.defaultPrevented=true;},...mods};ST.keyboard(event);return event;};
  const reply=(index,armed,err)=>{const message=arms[index];assert(message,'Expected arm request '+index);window.STHostTransport.deliver({id:message.id,ok:!err,...(err?{error:err}:{result:{armed}})});};
  const status=payload=>window.nativeEvent({type:'status',payload:{recordArmSupported:true,noteRecordArmed:false,playing:false,...payload}});
  return{ST,state:ST.state,window,document,node,calls,arms,key,reply,status};
}
const tests=[],test=(name,run)=>tests.push({name,run});
test('topbar waits for accepted native arm with grid focus ready for computer notes',async()=>{
 const h=harness();h.node('record').focus();h.node('record').onclick();await tick();
 assert.equal(h.arms.length,1);assert.equal(h.arms[0].payload.armed,true);assert.equal(h.state.recording,false);assert.equal(h.state.keyboardPlay,false);assert.equal(h.node('record').attributes['aria-busy'],'true');assert.equal(h.node('record').attributes['aria-pressed'],'false');assert.match(h.node('status-message').textContent,/Arming/);
 h.ST.noteOn(65,'KeyF');h.ST.noteOff('KeyF');assert.equal(h.state.undo.length,0,'Unaccepted arming must not insert notes');
 h.reply(0,true);await tick();assert.equal(h.state.recording,true);assert.equal(h.state.keyboardPlay,true);assert.equal(h.node('record').attributes['aria-busy'],'false');assert.equal(h.node('record').attributes['aria-pressed'],'true');assert.equal(h.document.activeElement.id,'tracker');assert.match(h.node('keyboard-play').textContent,/Keys on/);
});
test('failed supported arm stays disarmed and exposes a retryable error',async()=>{
 const h=harness();const request=h.ST.perform('record');await tick();h.reply(0,null,'Rejected by host');assert.equal(await request,false);assert.equal(h.state.recording,false);assert.equal(h.state.keyboardPlay,false);assert.match(h.node('toast-region').innerHTML,/Rejected by host/);assert.equal(h.node('record').attributes['aria-busy'],'false');
 const retry=h.ST.perform('record');await tick();assert.equal(h.arms[1].payload.armed,true);h.reply(1,true);await retry;assert.equal(h.state.recording,true);
});
test('rapid opposite intent serializes and never briefly enables note insertion',async()=>{
 const h=harness();h.ST.perform('record');await tick();const done=h.ST.perform('record');assert.equal(h.arms.length,1);h.reply(0,true);await tick();assert.equal(h.arms.length,2);assert.equal(h.arms[1].payload.armed,false);assert.equal(h.state.recording,false);h.reply(1,false);await done;assert.equal(h.state.recording,false);assert.equal(h.state.keyboardPlay,false);
});
test('rapid true false true coalesces to the already accepted in-flight intent',async()=>{
 const h=harness();h.ST.perform('record');await tick();h.ST.perform('record');const done=h.ST.perform('record');h.reply(0,true);await done;assert.equal(h.arms.length,1);assert.equal(h.state.recording,true);
});
test('a toggle queued as the acknowledgment settles is not stranded between drain and cleanup',async()=>{
 const h=harness();h.ST.perform('record');await tick();h.reply(0,true);await Promise.resolve();h.ST.perform('record');await tick();assert.equal(h.arms.length,2);assert.equal(h.arms[1].payload.armed,false);h.reply(1,false);await tick();assert.equal(h.state.recording,false);assert.equal(h.node('record').attributes['aria-busy'],'false');
});
test('latest intent continues after an earlier rejected command',async()=>{
 const h=harness();h.ST.perform('record');await tick();const done=h.ST.perform('record');h.reply(0,null,'Transient failure');await tick();assert.equal(h.arms.length,2);assert.equal(h.arms[1].payload.armed,false);h.reply(1,false);await done;assert.equal(h.state.recording,false);
});
test('new session disarms behind an old request and ignores old acknowledgments and status',async()=>{
 const h=harness();h.ST.perform('record');await tick();const p=h.ST.demoProject();p.name='New song';h.ST.acceptProject(p);assert.equal(h.state.recording,false);h.status({noteRecordArmed:true});assert.equal(h.state.recording,false);h.reply(0,true);await tick();assert.equal(h.state.recording,false);assert.equal(h.arms.length,2);assert.equal(h.arms[1].payload.armed,false);h.reply(1,false);await tick();h.status({noteRecordArmed:true});assert.equal(h.state.recording,false);assert.equal(h.state.project.name,'New song');assert.equal(h.node('record').attributes['aria-busy'],'false');
});
test('failed new-session disarm cannot re-enable local capture via status',async()=>{
 const h=harness();const armed=h.ST.perform('record');await tick();h.reply(0,true);await armed;h.ST.acceptProject(h.ST.demoProject());await tick();assert.equal(h.state.recording,false);h.reply(1,null,'Disarm unavailable');await tick();h.status({noteRecordArmed:true});assert.equal(h.state.recording,false);assert.match(h.node('toast-region').innerHTML,/Disarm unavailable/);
});
test('Save or Undo object replacement within the session does not invalidate accepted arm',async()=>{
 const h=harness(),done=h.ST.perform('record');await tick();h.state.project=h.ST.normalize(clone(h.state.project));h.reply(0,true);await done;assert.equal(h.state.recording,true);assert.equal(h.arms.length,1);
});
test('palette, Cmd-R and plain R share arming, held-key repeats do not retoggle',async()=>{
 const h=harness();h.key('r','KeyR');await tick();assert.equal(h.arms.length,1);h.key('r','KeyR',{repeat:true});assert.equal(h.arms.length,1);h.reply(0,true);await tick();h.key('r','KeyR',{metaKey:true});await tick();assert.equal(h.arms.length,2);assert.equal(h.arms[1].payload.armed,false);h.key('r','KeyR',{metaKey:true,repeat:true});h.reply(1,false);await tick();assert.equal(h.arms.length,2);assert.equal(h.state.recording,false);
});
test('plain R remains a piano note when keys are on and typed R remains text',async()=>{
 const h=harness();h.state.keyboardPlay=true;h.key('r','KeyR');await tick();assert.equal(h.arms.length,0);assert(h.calls.some(x=>x.command==='note.on'));h.ST.noteOff('KeyR');h.state.keyboardPlay=false;const input=h.node('text');input.tagName='INPUT';input.type='text';input.closest=selector=>selector.startsWith('input:not(')?input:null;h.key('r','KeyR',{target:input});await tick();assert.equal(h.arms.length,0);
});
test('older native hosts and browser preview preserve immediate local-only arming',async()=>{
 for(const native of [true,false]){const h=harness({supported:false,native});const done=h.ST.perform('record');assert.equal(h.state.recording,true);assert.equal(h.state.keyboardPlay,true);assert.equal(await done,true);assert.equal(h.arms.length,0);await h.ST.perform('record');assert.equal(h.state.recording,false);h.ST.acceptProject(h.ST.demoProject());assert.equal(h.state.recording,false);assert.equal(h.arms.length,0);}
});
test('host capability arrival synchronizes legacy local intent; idle host disarm is honored',async()=>{
 const h=harness({supported:false});await h.ST.perform('record');assert.equal(h.state.recording,true);h.status({noteRecordArmed:false});assert.equal(h.state.recording,false);await tick();assert.equal(h.arms.length,1);h.reply(0,true);await tick();assert.equal(h.state.recording,true);h.status({noteRecordArmed:false});assert.equal(h.state.recording,false);
});
test('invalid acknowledgement never falsely arms',async()=>{
 const h=harness();const done=h.ST.perform('record');await tick();assert.match(h.node('status-message').textContent,/Arming note recording/);h.reply(0,undefined);await done;assert.equal(h.state.recording,false);assert.match(h.node('toast-region').innerHTML,/did not confirm/);
});
(async()=>{for(const {name,run} of tests){await run();console.log('PASS '+name);}console.log(`${tests.length} note-record arm groups passed.`);})().catch(e=>{console.error(e);process.exitCode=1;});
