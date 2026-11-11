'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const { hostStub, noteRecordingHost } = require('./helpers/host');
const web = require('./helpers/web');
function harness(){
 const nodes=new Map(),groups=new Map(),calls=[],timers=new Map();let context,nextTimer=0;
 function node(id,dataset={},tagName='BUTTON'){
  if(nodes.has(id))return nodes.get(id);
  const el={id,dataset,tagName,value:'',type:'',style:{},innerHTML:'',classList:{toggle(){},add(){},remove(){}},setAttribute(){},addEventListener(){},querySelectorAll(){return[];},focus(){document.activeElement=this;},select(){},scrollIntoView(){},click(){this.onclick?.({target:this});},closest(selector){
   if(selector.includes('input:not'))return ['INPUT','SELECT','TEXTAREA'].includes(this.tagName)&&this.type!=='range'?this:null;
   if(selector==='button,select'||selector==='button,input,select'||selector==='button')return this.tagName==='BUTTON'?this:null;
   if(selector==='.tracker')return this.id==='tracker'?this:null;
   if(selector==='#sequence-list')return this.sequence?nodes.get('sequence-list'):null;
   return null;
  }};nodes.set(id,el);return el;
 }
 const document={body:null,activeElement:null,querySelector(q){if(q==='.context-menu')return null;return q[0]==='#'?node(q.slice(1)):(groups.get(q)||[])[0]||node(q);},querySelectorAll:q=>groups.get(q)||[],getElementById:id=>nodes.get(id)||null,createElement:node,addEventListener(){}};document.body=node('body',{},'BODY');
 const window={addEventListener(){},STHostTransport:hostStub({post(message){calls.push(message);window.STHostTransport.deliver({id:message.id,ok:true,result:recording(message)??{}});}})};
 const recording=noteRecordingHost(window);
 context=vm.createContext({window,document,console,Date,Map,Set,Math,JSON,Number,String,Object,Array,Promise,innerWidth:1280,innerHeight:800,setTimeout(fn){timers.set(++nextTimer,fn);return nextTimer;},clearTimeout(id){timers.delete(id);}});
 web.load(context, 'app.js');
 const ST=window.ST,state=ST.state;state.project=ST.demoProject();
 function group(selector,items){groups.set(selector,items);return items;}
 const nav=group('[data-view]',['pattern','instrument','browser','mixer','automation','song'].map(view=>node('view-'+view,{view})));
 const patterns=group('[data-pattern]',state.project.patterns.map(p=>node('sidebar-pattern-'+p.id,{pattern:String(p.id)})));
 const instruments=group('[data-instrument]',state.project.instruments.map((_,i)=>node('sidebar-instrument-'+i,{instrument:String(i)})));
 const types=group('[data-inst-type]',['sample','plugin','midi'].map(instType=>node('instrument-type-'+instType,{instType})));
 const piano=group('[data-piano]',[60,62,64,61,63].map(n=>node('piano-'+n,{piano:String(n)})));
 const tracker=node('tracker',{},'DIV');const sequence=node('sequence-list',{},'DIV');sequence.sequence=true;
 function key(target,key,extra={}){const e={target:typeof target==='string'?node(target):target,key,code:key,preventDefault(){this.defaultPrevented=true;},...extra};ST.keyboard(e);return e;}
 ST.refresh();return {ST,state,calls,window,document,node,group,nav,patterns,instruments,types,piano,key,timers};
}
{
 const h=harness();h.ST.setView('mixer');assert.equal(h.document.activeElement.id,'mix-track-0');
 h.ST.setView('automation');assert.equal(h.document.activeElement.id,'automation-search');
 h.ST.setView('instrument');assert.equal(h.document.activeElement.id,'instrument-rename');
 const before=JSON.stringify(h.state.project);h.key(h.nav[1],'ArrowRight');assert.equal(h.document.activeElement.id,'view-browser');assert.equal(h.state.view,'instrument');
 h.key(h.types[0],'ArrowRight');assert.equal(h.document.activeElement.id,'instrument-type-plugin');assert.equal(h.state.project.instruments[0].type,'sample');assert.equal(JSON.stringify(h.state.project),before);
 h.key(h.piano[0],'ArrowRight');assert.equal(h.document.activeElement.id,'piano-61');assert.equal(h.calls.filter(x=>x.command==='note.on').length,0);
 console.log('PASS workspace entry and segmented/piano arrows focus without changing sound');
}
{
 const h=harness();h.key(h.nav[0],'6',{metaKey:true});assert.equal(h.state.view,'song');assert.equal(h.document.activeElement.id,'sequence-list');assert.match(h.node('app').innerHTML,/<div class="sequence-list"[^>]*role="listbox"[^>]*tabindex="0"[^>]*id="sequence-list"/);h.ST.refresh();assert.equal(h.document.activeElement.id,'sequence-list');h.key(h.patterns[0],'ArrowDown');assert.equal(h.state.project.activePattern,1);assert.equal(h.state.sequence,0);assert.equal(h.document.activeElement.id,'sidebar-pattern-1');
 h.key(h.patterns[1],'ArrowRight');assert.equal(h.document.activeElement.id,'sequence-list');
 const before=JSON.stringify(h.state.project);h.key('song-export','ArrowDown');h.key('song-export','ArrowUp',{altKey:true});h.key('song-export','d',{metaKey:true});assert.equal(h.state.sequence,0);assert.equal(JSON.stringify(h.state.project),before);
 h.key('sequence-list','ArrowDown');assert.equal(h.state.sequence,1);
 h.ST.setView('instrument');h.key(h.instruments[0],'ArrowDown');assert.equal(h.state.instrument,1);assert.equal(h.document.activeElement.id,'sidebar-instrument-1');
 console.log('PASS sidebar selection and sequence shortcuts stay within their own area');
}
{
 const h=harness();let local=0;h.window.STStudio={renderMixer(){return '';},bindMixer(){},key(e){if(e.key==='Escape'){local++;e.preventDefault();return true;}return false;}};h.state.view='mixer';h.key('mix-slots-0','Escape');assert.equal(local,1);assert.equal(h.calls.filter(x=>x.command==='transport.stop').length,0);
 h.window.STStudio.key=()=>false;h.key('mix-track-0','Escape');assert.equal(h.calls.filter(x=>x.command==='transport.stop').length,1);
 const count=h.calls.length;h.key('tracker','ArrowDown',{defaultPrevented:true});assert.equal(h.calls.length,count);assert.equal(h.state.row,0);
 console.log('PASS Escape dismisses local panels before global Stop; local handled keys do not run twice');
}
{
 const h=harness();h.state.recording=true;h.state.playing=true;h.state.keyboardPlay=true;h.state.chord='major';h.state.scale='minor';h.state.row=0;
 const step={note:60,instrument:0,velocity:47,chord:[0,3,7],gate:.8,fx1:{type:'slice',value:4},fx2:{type:'reverse',value:1}};h.state.project.patterns[0].steps[0][0]=step;
 h.key('tracker','ArrowUp',{altKey:true});assert.equal(step.note,61);assert.equal(h.state.row,0);assert.equal(h.state.undo.length,1);
 assert.equal(h.state.lastInputNote,61);assert.equal(h.node('status-selection').textContent,'Velvet bass · C♯4');
 let preview=h.calls.filter(x=>x.command==='step.preview').at(-1);assert.deepEqual(JSON.parse(JSON.stringify(preview.payload)),{track:0,instrument:step.instrument,step:{...step},duration:.35});
 h.state.lastInputNote=60;h.key('tracker','Enter');assert.equal(h.calls.filter(x=>x.command==='step.preview').length,2);assert.equal(h.calls.filter(x=>x.command==='note.on').length,0);assert.equal(h.node('status-selection').textContent,'Velvet bass · C♯4');
 h.key('tracker','ArrowDown',{altKey:true,shiftKey:true});assert.equal(step.note,49);
 h.ST.stopHeld();preview=h.calls.filter(x=>x.command==='step.preview').at(-1);assert.equal(preview.payload.step.note,-1);
 assert.equal(h.calls.filter(x=>x.command.startsWith('transport.')).length,0);assert.equal(h.state.recording,true);
 console.log('PASS transpose and Enter audition exact step velocity/chord/slice without recording, scale quantization or cursor changes');
}
{
 const h=harness();h.state.selection={track:0,row:1};h.state.project.patterns[0].steps[0][0]={note:127,instrument:0};h.state.project.patterns[0].steps[0][1]={note:-1,instrument:0};h.ST.transpose(12);assert.equal(h.state.undo.length,0);assert.equal(h.calls.filter(x=>x.command==='step.preview').length,0);
 h.state.project.patterns[0].steps[0][0]=null;h.state.project.patterns[0].steps[0][1]={note:60,instrument:0};h.ST.transpose(1);assert.equal(h.calls.filter(x=>x.command==='step.preview').at(-1).payload.step.note,61);
 assert.deepEqual(Array.from(h.window.__supertracker.FX.slice),[1,128,1]);
 const checkbox=h.node('loop-song',{},'INPUT');checkbox.type='checkbox';const originalClosest=checkbox.closest;checkbox.closest=function(selector){if(selector.includes('input[type=checkbox]'))return this;return originalClosest.call(this,selector);};h.state.view='song';const space=h.key(checkbox,' ');assert(!space.defaultPrevented);assert.equal(h.calls.filter(x=>x.command.startsWith('transport.')).length,0);
 console.log('PASS transpose leaves OFF/CUT and limits unchanged, and auditions a selected note when the focused cell is empty');
}
{
 const h=harness();h.state.view='instrument';let localKeys=0;
 h.window.STSampler={stopAudition(){h.calls.push({command:'sampler.stop'});},key(e){if(['Enter',' ','Escape'].includes(e.key)){localKeys++;e.preventDefault();return true;}return false;}};
 const start=h.calls.length;h.ST.noteOn(60,'ownership-test',false);assert.deepEqual(h.calls.slice(start).map(x=>x.command),['sampler.stop','note.on']);
 const next=h.calls.length;h.ST.auditionStep({note:64,instrument:0},0);assert.deepEqual(h.calls.slice(next).map(x=>x.command),['sampler.stop','step.preview']);
 const before=h.calls.length;for(const key of ['Enter',' ','Escape'])assert(h.key('sampler-audition-0',key).defaultPrevented);assert.equal(localKeys,3);assert.equal(h.calls.length,before);
 const leave=h.calls.length;h.ST.setView('song');assert.equal(h.calls[leave].command,'sampler.stop');assert.equal(h.calls.filter(x=>x.command==='transport.stop').length,0);
 const modal=h.calls.length;h.ST.openModal('Test','');assert.equal(h.calls[modal].command,'sampler.stop');assert.equal(h.state.modal,'dialog');
 console.log('PASS slice ownership cancels before piano/step sound and local keys precede transport; navigation/modal stop owned previews');
}
{
 const controls=[{view:'instrument',target:'cutoff',track:0,type:'number',value:'1200'},{view:'mixer',target:'volume',track:2,type:'range',value:'.6'},{view:'instrument',target:'plugin:instrument:0:17',track:0,type:'range',value:'.37'},{view:'mixer',target:'plugin:effect:3:700',track:1,type:'range',value:'.42',modal:'dialog'}];
 for(const item of controls)for(const [key,code,shiftKey] of [['å','KeyA',false],['Å','KeyA',true],['a','',false],['A','KeyA',true]]){
  const h=harness();h.state.view=item.view;h.state.modal=item.modal||null;h.state.keyboardPlay=true;h.state.recording=true;h.state.project.patterns[0].automation=[];
  const input=h.node('option-parameter',{automate:item.target,track:String(item.track),name:'Focused parameter'},'INPUT');input.type=item.type;input.value=item.value;
  const event=h.key(input,key,{code,altKey:true,shiftKey});assert.equal(event.defaultPrevented,true,`${item.target}: ${key}/${code}`);
  const lanes=h.state.project.patterns[0].automation;assert.equal(lanes.length,1);assert.equal(lanes[0].target,item.target);
  // A plug-in lane names the track's channel; a lane of the track's own parameters names its index.
  if(item.target.startsWith('plugin:'))assert.equal(lanes[0].owner,`track:${h.state.project.tracks[item.track].routingID}`);else assert.equal(lanes[0].track,item.track);assert.equal(lanes[0].points[0].value,Number(item.value));
  assert.equal(h.state.view,'automation');assert.equal(h.state.modal,null);assert.equal(h.state.undo.length,1);assert.equal(h.calls.some(x=>x.command==='note.on'),false);
 }
 console.log('PASS macOS Option and Shift-Option character events automate instrument, mixer, NKS and modal parameters');
}
{
 for(const shiftKey of [false,true]){
  const h=harness();h.state.view='mixer';h.state.track=2;h.state.project.patterns[0].automation=[];h.state.project.routing={buses:[{id:'bus',name:'Group'}]};
  h.window.STStudio={target:()=>({available:true,value:.6}),renderAutomation:()=>'',bindAutomation(){}};
  const input=h.node('bus-parameter',{automate:'volume',owner:'bus:bus',track:'0',name:'Bus level'},'INPUT');input.type='range';input.value='.8';
  h.key(input,shiftKey?'Å':'å',{code:'KeyA',altKey:true,shiftKey});const lane=h.state.project.patterns[0].automation[0];
  assert.equal(lane.owner,'bus:bus');assert.equal(lane.track,undefined);assert.equal(h.state.track,2);assert.equal(h.state.mixerOwner,'bus:bus');
 }
 console.log('PASS macOS Option automation honors bus ownership without changing the note track');
}
{
 const h=harness();h.state.view='instrument';h.state.keyboardPlay=true;const before=JSON.stringify(h.state.project);
 const text=h.node('ordinary-text',{},'INPUT');text.type='text';text.value='Existing text';
 for(const [key,extra] of [['å',{altKey:true,code:'KeyA'}],['Å',{altKey:true,shiftKey:true,code:'KeyA'}],['a',{code:'KeyA'}]])assert(!h.key(text,key,extra).defaultPrevented);
 const parameter=h.node('nonshortcut-parameter',{automate:'volume',track:'0'},'INPUT');parameter.type='range';parameter.value='.5';
 for(const extra of [{},{altKey:true,code:'KeyB'},{altKey:true,isComposing:true},{altKey:true,metaKey:true},{altKey:true,ctrlKey:true}])assert(!h.key(parameter,'å',{code:'KeyA',...extra}).defaultPrevented);
 assert.equal(JSON.stringify(h.state.project),before);assert.equal(h.calls.some(x=>x.command==='note.on'),false);
 h.key('instrument-rename','z',{code:'KeyQ'});assert.equal(h.calls.filter(x=>x.command==='note.on').at(-1).payload.note,60);h.ST.noteOff('KeyQ');
 h.state.view='pattern';h.state.keyboardPlay=false;h.key('tracker','a',{metaKey:true,code:'KeyQ'});assert.equal(h.state.selection.track,0);assert.equal(h.state.track,h.state.project.tracks.length-1);
 const count=h.calls.filter(x=>x.command==='note.on').length;h.key('tracker','p',{metaKey:true,code:'KeyA'});assert.equal(h.state.keyboardPlay,true);assert.equal(h.calls.filter(x=>x.command==='note.on').length,count);
 assert.equal(text.value,'Existing text');console.log('PASS Option automation leaves text, composition, command shortcuts and logical piano notes unchanged');
}
{
 const h=harness(),opened=[];h.window.STStepFX={show(options){opened.push(JSON.parse(JSON.stringify(options)));}};
 const before=JSON.stringify(h.state.project);h.state.row=0;h.state.track=0;
 for(const column of [3,4]){h.state.column=column;assert(h.key('tracker','Enter').defaultPrevented);}
 assert.deepEqual(opened,[{fx:1},{fx:2}]);assert.equal(h.calls.filter(x=>x.command==='step.preview').length,0);assert.equal(JSON.stringify(h.state.project),before);assert.equal(h.state.undo.length,0);
 h.state.column=0;h.key('tracker','Enter');assert.equal(h.calls.filter(x=>x.command==='step.preview').length,1);assert.equal(opened.length,2);
 h.state.column=3;const buttonEvent=h.key('step-clear','Enter');assert(!buttonEvent.defaultPrevented);assert.equal(opened.length,2,'Enter on an inspector button must retain its own activation');
 console.log('PASS grid Enter opens the selected FX slot while the note column keeps exact-step audition');
}
{
 const h=harness(),step={note:-3,instrument:0,fx1:{type:'cutoff',value:900},fx2:{type:'delay',value:.35}};h.state.project.patterns[0].steps[0][0]=step;
 h.ST.refresh();const html=h.node('app').innerHTML,cell=html.match(/<td data-cell="0:0"[^>]*>([\s\S]*?)<\/td>/)?.[1];
 assert(cell&&/data-column="0"[^>]*>···<\/span>/.test(cell),'An FX-only row must render an empty note rather than OFF, CUT, or an invalid note name');
 const inspector=html.match(/<input id="step-note"[^>]*>/)?.[0];assert(inspector&&/min="-3"/.test(inspector)&&/value="-3"/.test(inspector));
 const before=JSON.stringify(step),undo=h.state.undo.length;h.ST.auditionStep(step);h.ST.transpose(1);h.key('tracker','Enter');
 assert.equal(JSON.stringify(step),before);assert.equal(h.state.undo.length,undo);assert.equal(h.calls.filter(x=>x.command==='step.preview'||x.command==='note.on').length,0);assert.equal(h.document.activeElement.id,'step-note');
 const opened=[];h.window.STStepFX={show(options){opened.push(options.fx);}};h.state.column=4;h.key('tracker','Enter');assert.deepEqual(opened,[2]);assert.equal(h.calls.filter(x=>x.command==='step.preview').length,0);
 console.log('PASS FX-only note -3 renders blank, remains editable, and never auditions or transposes');
}
{
 const h=harness(),plugin={type:'plugin',instanceID:'00000000-0000-0000-0000-000000000017',address:'18446744073709551615',value:.4,name:'Frequency'},second={type:'reverb',value:.23};
 h.state.project.patterns[0].steps[0][0]={note:60,instrument:0,velocity:88,fx1:plugin,fx2:second};const rendered=[],bound=new Map();let bindings=0;
 h.window.STStepFX={renderField(n,fx){rendered.push({n,fx:JSON.parse(JSON.stringify(fx))});return `<label data-module-fx="${n}"><input id="step-fx${n}-value" value="${fx.value}"></label>`;},bind(){bindings++;for(const n of [1,2]){const input=h.node(`step-fx${n}-value`);input.tagName='INPUT';input.type='number';const handler=()=>h.ST.mutate('fixture module FX value',p=>{const step=p.patterns[0].steps[0][0];step['fx'+n]={...step['fx'+n],value:Number(input.value)};});input.onchange=handler;bound.set(n,handler);}}};
 h.ST.refresh();assert.deepEqual(rendered.slice(-2),[{n:1,fx:plugin},{n:2,fx:second}]);assert(bindings>0);assert(h.node('app').innerHTML.includes('data-module-fx="1"'));assert(!h.node('app').innerHTML.includes('<select id="step-fx1">'));
 const value=h.node('step-fx1-value');assert.equal(value.onchange,bound.get(1),'the inspector must not bind over the mounted module');value.value='.72';value.onchange();
 assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.patterns[0].steps[0][0].fx1)),{...plugin,value:.72});assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.patterns[0].steps[0][0].fx2)),second);assert.equal(h.state.undo.length,1);
 console.log('PASS step inspector delegates rendering and binding without erasing a plug-in instance or full-width parameter address');
}
{
 const h=harness();assert.equal(h.window.STStepFX,undefined);h.ST.refresh();const html=h.node('app').innerHTML;assert(html.includes('<select id="step-fx1">')&&html.includes('<select id="step-fx2">'));
 const type=h.node('step-fx1'),value=h.node('step-fx1-value');type.value='cutoff';value.value='2750';type.onchange();assert.equal(h.state.project.patterns[0].steps[0][0].fx1.value,2750);
 value.value='3100';value.onchange();assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.patterns[0].steps[0][0].fx1)),{type:'cutoff',value:3100});
 console.log('PASS built-in inspector fields retain working bindings when the step FX module is absent');
}
{
 for(const mime of ['application/x-supertracker-steps','text/plain']){
  const h=harness(),step={note:-3,instrument:0,fx1:{type:'plugin',instanceID:'00000000-0000-0000-0000-000000000017',address:'18446744073709551615',value:.4,name:'Frequency'},fx2:{type:'pan',value:-.2}};
  h.state.project.patterns[0].steps[0][0]=step;h.node('tracker').focus();const copied=new Map();const event=type=>({type,preventDefault(){this.defaultPrevented=true;},clipboardData:{setData(format,value){copied.set(format,value);},getData(format){return format===mime?copied.get(format)||'':'';}}});
  const copyEvent=event('copy');h.ST.clipboardEvent(copyEvent);assert(copyEvent.defaultPrevented);assert.deepEqual(JSON.parse(copied.get(mime)).steps,[[step]]);
  h.state.clipboard=null;h.state.row=2;const pasteEvent=event('paste');h.ST.clipboardEvent(pasteEvent);assert(pasteEvent.defaultPrevented);assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.patterns[0].steps[0][2])),step);assert.equal(h.state.undo.length,1);
  h.state.project.patterns[0].steps[0][2].fx1.value=.9;assert.equal(step.fx1.value,.4,'Pasting must deep-copy plug-in FX metadata');
  copied.set(mime,JSON.stringify({supertracker:1,steps:[[{note:-4,fx1:{type:'cutoff',value:1000}}]]}));h.state.row=3;const before=JSON.stringify(h.state.project);h.ST.clipboardEvent(event('paste'));assert.equal(JSON.stringify(h.state.project),before);assert.equal(h.state.undo.length,1);
 }
 console.log('PASS FX-only steps round-trip through custom and text clipboard formats with exact plug-in metadata and valid sentinel limits');
}
{
 const h=harness(),fx1={type:'plugin',instanceID:'00000000-0000-0000-0000-000000000017',address:'18446744073709551615',value:.4,name:'Frequency'},fx2={type:'reverse',value:1};
 h.state.project.patterns[0].steps[0][0]={note:-3,instrument:0,fx1,fx2};h.state.recording=true;h.state.playing=true;
 const first=h.ST.recordLiveStep(60,70,0,0,0,0),second=h.ST.recordLiveStep(64,90,0,0,0,0);const live=h.state.project.patterns[0].steps[0][0];
 assert.equal(live.note,60);assert.deepEqual(Array.from(live.chord),[0,4]);assert.deepEqual(JSON.parse(JSON.stringify(live.fx1)),fx1);assert.deepEqual(JSON.parse(JSON.stringify(live.fx2)),fx2);
 h.ST.finishLiveRecord(second);h.ST.finishLiveRecord(first);assert.deepEqual(JSON.parse(JSON.stringify(h.state.project.patterns[0].steps[0][0].fx1)),fx1);
 console.log('PASS a live chord retains the plug-in and sample effects of an FX-only row');
}
{
 const plugin={type:'plugin',instanceID:'00000000-0000-0000-0000-000000000017',address:'18446744073709551615',value:.4,name:'Frequency'},builtin={type:'pan',value:-.2};
 for(const originalNote of [-3,60,-1,-2])for(const cut of [false,true])for(const pluginSlot of [1,2]){
  const h=harness(),fx1=pluginSlot===1?plugin:builtin,fx2=pluginSlot===2?plugin:builtin;
  h.state.keyboardPlay=false;h.state.column=0;h.state.row=0;h.state.editStep=1;
  h.state.project.patterns[0].steps[0][0]={note:originalNote,instrument:0,velocity:83,gate:4,chord:[0,4,7],fx1,fx2};
  const before=JSON.parse(JSON.stringify(h.state.project));const event=h.key('tracker',cut?'!':'1',{code:'Digit1',shiftKey:cut});const step=h.state.project.patterns[0].steps[0][0];
  assert(event.defaultPrevented);assert.equal(step.note,cut?-2:-1);assert(step.fx1&&step.fx2,'OFF/CUT must preserve both existing effect slots');assert.deepEqual(JSON.parse(JSON.stringify(step.fx1)),fx1);assert.deepEqual(JSON.parse(JSON.stringify(step.fx2)),fx2);
  assert.equal(step.chord,undefined);assert.equal(step.gate,undefined);assert.equal(h.state.row,1);assert.equal(h.state.undo.length,1);
  assert.equal(h.calls.some(call=>call.command==='note.on'||call.command==='step.preview'),false);
  h.ST.perform('undo');assert.deepEqual(JSON.parse(JSON.stringify(h.state.project)),before);
 }
 console.log('PASS OFF and CUT keyboard entry preserve both effect slots and exact plug-in locks with one reversible edit');
}
{
 for(const activeSearch of ['browser-search','browser-plugin-search']){
  const h=harness(),calls=[];h.window.STBrowser={render:()=>'',bind(){},focusSearch(){calls.push('focus');h.node(activeSearch,{},'INPUT').focus();},search(){calls.push('search');assert.equal(h.document.activeElement.id,activeSearch);}};
  const event=h.key('tracker','3',{metaKey:true,code:'Digit3'});assert(event.defaultPrevented);assert.equal(h.state.view,'browser');assert.equal(h.document.activeElement.id,activeSearch);assert.deepEqual(calls,['focus','search']);
 }
 console.log('PASS Command-3 focuses the active Browser search before starting asynchronous search');
}
{
 const h=harness(),actions=[];let recording=false,busy=false;
 h.window.STRecording={toggleCapture(){actions.push('toggle');},show(options){actions.push(JSON.parse(JSON.stringify(options)));},inputSummary(){return 'Hardware <interface> · Input 3 · mono';},get isRecording(){return recording;},get isBusy(){return busy;}};
 h.state.project.instruments[0].type='midi';h.ST.setView('instrument');const before=JSON.stringify(h.state.project);
 assert(h.node('app').innerHTML.includes('Hardware &lt;interface&gt; · Input 3 · mono'));assert(h.node('app').innerHTML.includes('Sample this synth…'));
 h.node('instrument-record-input').click();assert.deepEqual(actions,[{hardwareSample:true}]);assert.equal(JSON.stringify(h.state.project),before);
 h.key('tracker','R',{metaKey:true,shiftKey:true,code:'KeyR'});assert.equal(actions.at(-1),'toggle');assert.equal(h.state.recording,false);
 const count=actions.length;h.key('tracker','R',{metaKey:true,shiftKey:true,repeat:true,code:'KeyR'});assert.equal(actions.length,count);
 h.state.modal='recording';recording=true;const field=h.node('record-name',{},'INPUT');
 assert(h.key(field,'R',{metaKey:true,shiftKey:true,code:'KeyR'}).defaultPrevented);assert.equal(actions.length,count+1);
 h.state.modal='settings';const unrelated=h.key(field,'R',{metaKey:true,shiftKey:true,code:'KeyR'});assert(!unrelated.defaultPrevented);assert.equal(actions.length,count+1);
 h.state.modal=null;recording=false;h.window.STExport={isBusy:true};h.key('tracker','R',{metaKey:true,shiftKey:true,code:'KeyR'});assert.equal(actions.length,count+1);
 h.window.STExport.isBusy=false;h.key('tracker','r',{metaKey:true,code:'KeyR'});assert.equal(h.window.STApp.recordArmDesired,true,'Cmd-R remains note-record arm');
 console.log('PASS hardware capture summary/button and Command-Shift-R route to the recorder without changing note-arm, hijacking other dialogs or repeating');
}
console.log('Workspace navigation and step audition: 18 regression groups passed.');
