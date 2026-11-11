'use strict';
// Actual app/studio renderers and handlers, with destructive DOM replacement.
// Synthetic keys model handler ownership, not native macOS default behavior.
const assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm'),path=require('node:path');
const web = require('./helpers/web');
const source=fs.readFileSync(path.join(__dirname,'scalar_field_focus_tests.js'),'utf8');
const prefix=source.slice(0,source.indexOf('const tests = []')).replace('return { ...h, timers,','return { ...h, create, enhance, timers,');
const mounted=new Function('require','__dirname','process',prefix+'\nreturn harness;')(require,__dirname,process);
const copy=value=>JSON.parse(JSON.stringify(value));
const tick=async()=>{for(let i=0;i<80;i++)await Promise.resolve();};
function harness(view='instrument'){
  const h=mounted(view),{ST,state,document,window,context}=h,handlers=new Map(),listeners=new Map();
  context.requestAnimationFrame=fn=>fn();context.innerWidth=1080;context.innerHeight=700;
  window.STHostTransport.post=message=>{h.calls.push(copy(message));Promise.resolve().then(()=>handlers.get(message.command)?.(message.payload)??{}).then(result=>window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));};
  const previousListener=document.addEventListener;
  document.addEventListener=(type,fn,...rest)=>{previousListener(type,fn,...rest);(listeners.get(type)||listeners.set(type,[]).get(type)).push(fn);};
  const append=element=>{document.body.children.push(element);element.parentElement=document.body;};
  document.body.appendChild=append;
  function fragment(root){let html='';Object.defineProperty(root,'innerHTML',{configurable:true,get:()=>html,set(value){html=value;const detach=node=>{for(const child of node.children){child.isConnected=false;detach(child);}};detach(root);root.children=[];if(document.activeElement?.isConnected===false)document.activeElement=document.body;const stack=[root];let serial=0;
    for(const match of value.matchAll(/<(\/)?([a-z][\w-]*)\b((?:"[^"]*"|'[^']*'|[^'">])*)>/gi)){
      const tag=match[2].toLowerCase();if(match[1]){while(stack.length>1&&stack.pop().tagName!==tag.toUpperCase()){}continue;}
      const attrs={};for(const attr of match[3].matchAll(/([\w:-]+)(?:="([^"]*)"|='([^']*)')?/g))attrs[attr[1]]=attr[2]??attr[3]??'';
      const item=h.create(tag,attrs.id||`fragment-${++serial}`,stack.at(-1),{attrs,className:attrs.class||'',type:attrs.type||'',disabled:Object.hasOwn(attrs,'disabled'),...(attrs.tabindex!==undefined?{tabIndex:Number(attrs.tabindex)}:{})});item.value=attrs.value||'';
      if(!['input','br','hr','path'].includes(tag))stack.push(item);
    }
  }});root.remove=()=>{root.isConnected=false;root.parentElement.children=root.parentElement.children.filter(child=>child!==root);};return root;}
  fragment(h.create('div','overlay'));
  document.createElement=tag=>{const element=fragment(h.create(tag,'',null));Object.defineProperty(element,'className',{get:()=>[...element.classes].join(' '),set:value=>element.classes=new Set(value.split(' '))});return element;};
  context.setInterval=()=>0;context.clearInterval=()=>{};
  for(const file of ['routing-project.js','studio.js','insert-chain.js'])web.load(context, file);
  window.STRoutingProject.ensureTrackIDs(state.project);state.project.patterns.forEach(p=>p.automation=[]);ST.refresh();h.calls.length=0;state.undo=[];
  return {...h,handlers,async boot(){handlers.set('ready',()=>({project:state.project}));await ST.boot();state.view=view;ST.refresh();h.calls.length=0;state.undo=[];},contextAction(control){const event={target:control,clientX:40,clientY:40,preventDefault(){}};for(const fn of listeners.get('contextmenu')||[])fn(event);h.get('context-automate').click();},async plugin(){
    const owner=`track:${state.project.tracks[0].routingID}`;
    const plugin={owner,kind:'instrument',slot:0,instanceID:'original-instance',component:'clap:com.example.original-fixture',format:'CLAP',state:'state:.25',name:'Original fixture',parameters:[{address:'71',name:'Brightness',normalized:.25,writable:true,automatable:true}]};state.project.instruments[0].type='plugin';state.project.audioPlugins=[plugin];state.view='instrument';ST.refresh();
    handlers.set('plugin.parameters',()=>plugin.parameters);handlers.set('plugin.state',()=>({...plugin,state:'state:.61'}));handlers.set('engine.snapshot',()=>({audioPlugins:copy(state.project.audioPlugins)}));h.get('plugin-parameters').focus();await ST.showPluginParameters({owner,kind:'instrument',slot:0});return h.get('plugin-param-0');
  }};
}
function handedOff(h,source,target,existing){
  const lane=h.ST.pattern().automation[h.state.selectedLane];assert.equal(lane.target,target);if(existing)assert.equal(lane,existing,'select the existing object rather than replace its points');
  assert.equal(h.state.view,'automation');assert.equal(h.state.modal,null);assert.equal(h.document.activeElement.id,'automation-envelope');assert.equal(source.isConnected,false);
  const before=lane.points[0].value,undo=h.state.undo.length,other=copy(h.ST.pattern().automation.filter(item=>item!==lane));
  h.key(h.document.activeElement,'ArrowUp');assert(lane.points[0].value>before);assert.equal(h.state.undo.length,undo+1);assert.equal(h.document.activeElement.id,'automation-envelope');assert.deepEqual(copy(h.ST.pattern().automation.filter(item=>item!==lane)),other);
}
function focusedEnvelope(h){assert.equal(h.document.activeElement.id,'automation-envelope');assert(h.document.activeElement.scrolls>0,'reveal the envelope after switching workspace');}
const checkpoint=h=>({project:JSON.stringify(h.state.project),undo:JSON.stringify(h.state.undo),redo:JSON.stringify(h.state.redo),dirty:h.state.dirty});
async function pureNavigation(h,before){
  assert.deepEqual(checkpoint(h),before,'showing an existing lane must not change music, history or dirty state');
  assert(![...h.timers.values()].some(timer=>timer.delay===100||timer.delay===1800),'navigation must not schedule engine publication or autosave');
  h.flush(100);h.flush(1800);await tick();
  assert.deepEqual(checkpoint(h),before,'late parameter reads must also preserve project and edit history');
  assert(!h.calls.some(call=>['engine.project','project.autosave'].includes(call.command)),'navigation must not publish the project or autosave');
}
const tests=[],test=(name,run)=>tests.push({name,run});
for(const view of ['instrument','mixer'])for(const existing of [false,true])test(`${view} Option+A ${existing?'selects existing':'creates new'} lane and immediately edits it with arrows`,async()=>{
  const h=harness(view),target=view==='instrument'?'cutoff':'volume',id=view==='instrument'?'param-value-cutoff':'mix-vol-0';
  const prior={id:'prior',track:0,target,name:'Original lane',points:[{row:2,value:view==='instrument'?900:.3}]};h.ST.pattern().automation.push({id:'other',track:1,target:'pan',name:'Other lane',points:[{row:0,value:0}]});if(existing)h.ST.pattern().automation.push(prior);
  h.ST.refresh();const input=h.get(id);input.focus();h.state.dirty=false;h.calls.length=0;h.timers.clear();const before=checkpoint(h);const event=h.key(input,'å',{code:'KeyA',altKey:true});assert(event.defaultPrevented);assert.equal(h.state.undo.length,existing?0:1);focusedEnvelope(h);if(existing)await pureNavigation(h,before);handedOff(h,input,target,existing?prior:null);await tick();assert.equal(h.document.activeElement.id,'automation-envelope');
});
for(const existing of [false,true])test(`generic parameter context action ${existing?'selects existing':'creates new'} lane and closes its modal`,async()=>{
  const h=harness();await h.boot();const input=await h.plugin(),target='plugin:instrument:0:71';let prior;
  if(existing){prior={id:'plugin-lane',owner:`track:${h.state.project.tracks[0].routingID}`,target,name:'Brightness',points:[{row:0,value:.2}]};h.ST.pattern().automation.push(prior);}
  input.focus();h.state.dirty=false;h.calls.length=0;h.timers.clear();const before=checkpoint(h);h.contextAction(input);assert.equal(h.state.undo.length,existing?0:1);focusedEnvelope(h);if(existing)await pureNavigation(h,before);handedOff(h,input,target,prior);await tick();assert.equal(h.document.activeElement.id,'automation-envelope');
});
test('existing lanes remain pure navigation during export and routing edits, retaining clean or dirty history',async()=>{
  for(const view of ['instrument','mixer','plugin'])for(const blocker of ['export','routing'])for(const dirty of [false,true]){
    const h=harness(view==='plugin'?'instrument':view);const input=view==='plugin'?await h.plugin():h.get(view==='mixer'?'mix-vol-0':'param-value-cutoff');
    const target=view==='plugin'?'plugin:instrument:0:71':view==='mixer'?'volume':'cutoff';
    const lane={id:'existing',...(view==='plugin'?{owner:`track:${h.state.project.tracks[0].routingID}`}:{track:0}),target,name:'Do not rename',interpolation:'step',points:[{row:2,value:view==='instrument'?800:.3}]};h.ST.pattern().automation.push(lane);
    h.state.undo.push({label:'Prior musical edit',project:copy(h.state.project)});h.state.redo.push({label:'Future musical edit',project:copy(h.state.project)});h.state.dirty=dirty;
    h.window[blocker==='export'?'STExport':'STRouting']=blocker==='export'?{isBusy:true}:{isApplying:true};input.focus();h.calls.length=0;h.timers.clear();const before=checkpoint(h);
    h.key(input,'å',{code:'KeyA',altKey:true});focusedEnvelope(h);await pureNavigation(h,before);
    assert.equal(h.state.view,'automation');assert.equal(h.state.modal,null);assert.equal(h.ST.pattern().automation[h.state.selectedLane],lane);assert.equal(h.document.activeElement.id,'automation-envelope');
  }
});
test('pending generic parameter capture survives handoff without stealing envelope focus',async()=>{
  const h=harness();const input=await h.plugin();let release;h.handlers.set('plugin.parameter',()=>new Promise(resolve=>release=resolve));input.focus();input.value='.61';input.fire('input');const pending=input.onchange({target:input});await tick();assert(release);
  h.key(input,'å',{code:'KeyA',altKey:true});assert.equal(h.document.activeElement.id,'automation-envelope');assert(h.window.STPluginParameters.hasPendingEdits);assert.equal(h.ST.pattern().automation[0].points[0].value,.61);
  release({display:'.61'});await pending;await tick();assert.equal(h.state.project.audioPlugins[0].state,'state:.61');assert.equal(h.window.STPluginParameters.hasPendingEdits,false);assert.equal(h.document.activeElement.id,'automation-envelope');
});
for(const blocker of ['export','routing'])for(const view of ['instrument','automation'])test(`${blocker} rejection from ${view} keeps its current focus, lane and history`,()=>{
  const h=harness(view);if(view==='automation'){h.ST.pattern().automation.push({id:'old',track:0,target:'pan',name:'Pan',points:[{row:0,value:0}]});h.ST.refresh();}
  const input=h.get(view==='instrument'?'param-value-cutoff':'automation-search');input.focus();const before=JSON.stringify(h.state.project);
  h.window[{export:'STExport',routing:'STRouting'}[blocker]]=blocker==='routing'?{isApplying:true}:{isBusy:true};h.ST.automate(0,'cutoff','Cutoff',900);
  assert.equal(h.document.activeElement,input);assert.equal(h.state.view,view);assert.equal(JSON.stringify(h.state.project),before);assert.equal(h.state.undo.length,0);
});
test('invalid target, missing channel and unassigned instrument do not edit or steal focus',()=>{
  for(const kind of ['target','channel','unassigned']){const h=harness(),input=h.get('param-value-cutoff');input.focus();if(kind==='unassigned')h.state.instrument=1;const before=JSON.stringify(h.state.project);h.ST.automate(kind==='channel'?99:0,kind==='target'?'unsupported':'cutoff','Cutoff',900);assert.equal(h.document.activeElement,input);assert.equal(h.state.view,'instrument');assert.equal(JSON.stringify(h.state.project),before);assert.equal(h.state.undo.length,0);}
});
test('blocked generic modal handoff leaves its control and pending edit mounted',async()=>{
  const h=harness(),input=await h.plugin();input.focus();h.window.STRouting={isApplying:true};h.key(input,'å',{code:'KeyA',altKey:true});assert.equal(h.state.modal,'dialog');assert.equal(h.document.activeElement,input);assert.equal(input.isConnected,true);assert.equal(h.ST.pattern().automation.length,0);assert.equal(h.state.undo.length,0);
});
(async()=>{for(const {name,run} of tests){await run();console.log('PASS '+name);}console.log(`PASS ${tests.length} automation handoff groups`);})().catch(error=>{console.error(error);process.exitCode=1;});
