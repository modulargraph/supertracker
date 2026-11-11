'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
// Reuse the clip DOM fixture, replacing its no-op renderer with destructive
// replacement. Production bind/change handlers run unmodified. Tab's focus
// transfer is modeled explicitly; a web view's own Tab handling is not tested.
const navigation=fs.readFileSync(path.join(__dirname,'clip_navigation_tests.js'),'utf8');
const makeHarness=new Function('require','__dirname',navigation.slice(0,navigation.indexOf('const tests = [];'))+'\nreturn harness;')(require,__dirname);
function harness(){
 const h=makeHarness(),ST=h.context.window.ST,timers=new Map(),documentEvents=new Map();let nextTimer=0,renders=0;
 h.context.setTimeout=(fn,delay=0)=>{timers.set(++nextTimer,{fn,delay});return nextTimer;};h.context.clearTimeout=id=>timers.delete(id);
 h.document.addEventListener=(type,fn)=>{const list=documentEvents.get(type)||[];list.push(fn);documentEvents.set(type,list);};
 for(const [field,value,min,max]of [['startBeat',4,0,1000000],['durationBeats',4,.001,1000000],['sourceOffset',0,0,86400],['gain',1,0,4],['fadeIn',0,0,2],['fadeOut',0,0,2]]){
  const input=h.node('clips-'+field,h.panel,'INPUT',{clipsField:field});Object.assign(input,{type:'number',value:String(value),min:String(min),max:String(max)});
 }
 Object.assign(h.nodes.get('clips-cursor'),{type:'number',value:'0'});h.node('clips-muted',h.panel,'INPUT').type='checkbox';
 const originalMutate=ST.mutate;ST.mutate=(label,fn,options={})=>{originalMutate(label,fn);if(options.render!==false)ST.refresh();};
 ST.refresh=()=>{
  renders++;const old=[...h.nodes.values()],parents=new Map(old.map(node=>[node.id,node.parentElement?.id]));h.document.activeElement=h.document.body;
  for(const node of old){if(node===h.document.body)continue;const fresh=h.node(node.id,null,node.tagName,{...node.dataset},[...node.classes]);for(const key of ['type','value','min','max','checked','disabled'])if(node[key]!==undefined)fresh[key]=node[key];}
  for(const node of h.nodes.values())if(node!==h.document.body)node.parentElement=h.nodes.get(parents.get(node.id))||h.document.body;
  h.clips.bind();
 };
 ST.refresh();
 function commitWithDeparture(sourceID,destinationID,value){
  const source=h.nodes.get(sourceID),destination=h.nodes.get(destinationID);source.focus();source.value=String(value);source.fire('input');
  const key=source.fire('keydown',{key:'Tab'});assert(!key.defaultPrevented,'Tab keeps its native default');
  // Native change runs while the old field is losing focus, before the browser
  // has focused the adjacent destination. Replacing it here loses that target.
  h.document.activeElement=h.document.body;source.fire('change');source.fire('blur');
  if(h.nodes.get(destinationID)===destination)destination.focus();
  return {source,destination};
 }
 function flush(){for(const[id,timer]of [...timers])if(timer.delay===0){timers.delete(id);timer.fn();}}
 return{...h,ST,commitWithDeparture,flush,pointer(type,target){for(const fn of documentEvents.get(type)||[])fn({target});},get renders(){return renders;}};
}
{
 const h=harness();h.commitWithDeparture('clips-cursor','clips-cursor-position',2);assert.equal(h.document.activeElement.id,'clips-cursor-position','cursor commit must not detach the native Tab destination');h.flush();assert.equal(h.document.activeElement.id,'clips-cursor-position');assert.match(h.clips.render(),/id="clips-cursor"[^>]*value="2"/);assert.equal(h.changes.length,0);
 console.log('PASS edit-cursor commit preserves native Tab destination before and after repaint');
}
{
 const h=harness();h.commitWithDeparture('clips-durationBeats','clips-sourceOffset',3);assert.equal(h.document.activeElement.id,'clips-sourceOffset');h.flush();assert.equal(h.document.activeElement.id,'clips-sourceOffset');assert.equal(h.state.project.audioClips[0].durationBeats,3);assert.deepEqual(h.changes,['edit audio clip']);
 console.log('PASS clip numeric commit is one synchronous model edit while Tab reaches the next inspector field');
}
{
 const h=harness();h.commitWithDeparture('clips-sourceOffset','clips-durationBeats',.25);h.flush();assert.equal(h.document.activeElement.id,'clips-durationBeats');assert.equal(h.state.project.audioClips[0].sourceOffset,.25);
 console.log('PASS backward field departure retains the previous inspector destination');
}
{
 const h=harness(),input=h.nodes.get('clips-gain');input.focus();input.value='1.75';input.fire('input');input.fire('change');h.flush();assert.equal(h.document.activeElement.id,'clips-gain');assert.equal(h.state.project.audioClips[0].gain,1.75);
 console.log('PASS Enter/native number adjustment commits retain the field when focus stays put');
}
{
 const h=harness();h.commitWithDeparture('clips-gain','clips-fadeIn',1.3);const renders=h.renders;h.state.view='mixer';h.flush();assert.equal(h.renders,renders);assert.equal(h.state.view,'mixer');
 const next=harness();next.commitWithDeparture('clips-gain','clips-fadeIn',1.3);const before=next.renders;next.state.project=JSON.parse(JSON.stringify(next.state.project));next.flush();assert.equal(next.renders,before);
 console.log('PASS deferred clip paint never restores a replaced session or steals focus after leaving Song');
}

{
 const h=harness(),input=h.nodes.get('clips-gain');input.focus();input.value='1.35';input.fire('input');const before=h.renders;
 h.clips.event('audio.clipLoaded',{path:'/fixture/a.wav',duration:8,peaks:[]});assert.equal(h.renders,before);assert.equal(h.document.activeElement,input);assert.equal(input.value,'1.35');assert.equal(h.state.project.audioClips[0].gain,undefined);
 h.commitWithDeparture('clips-gain','clips-fadeIn',1.35);h.flush();assert.equal(h.document.activeElement.id,'clips-fadeIn');assert.equal(h.state.project.audioClips[0].gain,1.35);
 console.log('PASS waveform completion cannot detach an unfinished numeric draft; committed repaint follows its destination');
}

{
 const h=harness(),source=h.nodes.get('clips-cursor'),destination=h.nodes.get('clips-zoom-in');source.focus();source.value='2';source.fire('input');h.pointer('pointerdown',destination);h.document.activeElement=h.document.body;source.fire('change');source.fire('blur');destination.focus();const before=h.renders;h.flush();assert.equal(h.renders,before,'pointer target must survive until its click');let clicks=0;destination.addEventListener('click',()=>clicks++);h.pointer('pointerup',destination);destination.fire('click');assert.equal(clicks,1);h.flush();assert.equal(h.document.activeElement.id,'clips-zoom-in');
 console.log('PASS field commit preserves an occasional pointer gesture until its intended button receives click');
}
