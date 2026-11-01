'use strict';
// Actual browser/App handlers; synthetic bridge only, no filesystem or audio execution.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const web = require('./helpers/web');
const browserTemplate=fs.readFileSync(path.join(__dirname,'browser_filters_ui_tests.js'),'utf8');
const {harness:browserHarness}=new Function('require','__dirname',browserTemplate.slice(0,browserTemplate.indexOf('(async () => {'))+'\nreturn {harness};')(require,__dirname);
const {harness:appHarness,base}=require('./helpers/studio-session');
const copy=x=>JSON.parse(JSON.stringify(x)),tests=[],test=(name,fn)=>tests.push({name,fn});
const sample=(name)=>({path:`/${name}.wav`,name,kind:'sample',available:true});
async function actualBrowser(){const h=appHarness();h.S.state.view='browser';h.S.state.library={items:[],query:'',selected:0};h.handlers.set('library.search',()=>({items:[sample('Kick'),sample('Snare')],total:2}));web.load(h.context, 'browser.js');h.B=h.window.STBrowser;await h.B.search();return h;}
function key(h,target,key,extra={}){const event={target,key,code:key,preventDefault(){this.defaultPrevented=true;},...extra};return {handled:h.browser.key(event),event};}
test('destination stays available without results and ordinary selection uses native select arrows without editing the project',async()=>{
 const h=browserHarness();h.state.project.instruments=[{name:'First'},{name:'Bass & bells'}];h.browser.render();
 assert.match(h.browser.render(),/id="browser-destination-instrument"/);assert.match(h.browser.render(),/Add sample on new track/);assert.match(h.browser.render(),/Bass &amp; bells/);
 const select=h.node('browser-destination-instrument',{},null,{tagName:'SELECT',value:'1'});h.browser.bind();const before=copy(h.state.project);let held=0;h.window.ST.stopHeld=()=>held++;
 select.focus();select.fire('change');assert.equal(h.state.instrument,1);assert.deepEqual(copy(h.state.project),before);assert.equal(h.document.activeElement,select);assert.equal(held,1);
 assert.equal(key(h,select,'ArrowDown').handled,false,'select retains its native arrow behavior');
 const old=h.state.project;h.state.project=copy(old);select.value='0';select.fire('change');assert.equal(h.state.instrument,1,'detached destination control cannot select a new session');
});
test('browser replacement destination selection follows the instrument’s linked lane',async()=>{
 const h=await actualBrowser();await h.S.loadSampleAsNew('/linked.wav');const target=h.S.state.instrument,lane=h.S.state.track;h.S.state.track=0;h.S.state.instrument=0;
 const select=h.element('browser-destination-instrument');select.value=String(target);h.B.bind();select.listeners.change({target:select});
 assert.equal(h.S.state.instrument,target);assert.equal(h.S.state.track,lane);
});
test('Add twice creates distinct samples with one Undo each, stays in the browser and keeps all existing instruments',async()=>{
 const h=await actualBrowser(),original=copy(h.S.state.project.instruments),count=original.length,undo=h.S.state.undo.length;
 assert((await h.B.controller.load(true)).path);h.S.state.library.selected=1;assert((await h.B.controller.load(true)).path);
 assert.equal(h.S.state.view,'browser');assert.equal(h.S.state.project.instruments.length,count+2);assert.deepEqual(copy(h.S.state.project.instruments.slice(0,count)),original);
 assert.equal(h.S.state.project.instruments[count].samplePath,'/Kick.wav');assert.equal(h.S.state.project.instruments[count+1].samplePath,'/Snare.wav');assert.equal(h.S.state.undo.length,undo+2);assert.equal(h.S.state.instrument,count+1);
 await h.S.perform('undo');assert.equal(h.S.state.project.instruments.length,count+1);assert.equal(h.S.state.project.instruments[count].samplePath,'/Kick.wav');await h.S.perform('redo');assert.equal(h.S.state.project.instruments.length,count+2);
});
test('Add works with an empty track selected (no instrument) and in a project without instruments',async()=>{
 const h=await actualBrowser(),count=h.S.state.project.instruments.length;h.S.state.project.tracks[0].instrument=-1;h.S.state.track=0;h.S.state.instrument=-1;
 assert((await h.B.controller.load(true)).path);assert.equal(h.S.state.project.instruments.length,count+1);assert.equal(h.S.state.instrument,count);assert.doesNotMatch(h.toast,/no longer exists/);
 const e=await actualBrowser();e.S.state.project.instruments=[];e.S.state.project.tracks.forEach(t=>t.instrument=-1);e.S.state.instrument=-1;
 assert((await e.B.controller.load(true)).path);assert.equal(e.S.state.project.instruments.length,1);assert.equal(e.S.state.project.instruments[0].samplePath,'/Kick.wav');
 await e.S.perform('undo');assert.equal(e.S.state.project.instruments.length,0);
});
test('decode failure and rejected publication leave no empty new instrument, assignment or Undo',async()=>{
 for(const fault of ['decode','ack']){const h=await actualBrowser(),before=copy(h.S.state.project),count=h.S.state.undo.length;
 if(fault==='decode')h.handlers.set('sample.load',()=>{throw new Error('Unreadable sample');});else{let n=0;const original=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>++n===2?false:original(p));}
 await h.B.controller.load(true);assert.deepEqual(copy(h.S.state.project),before,fault);assert.equal(h.S.state.undo.length,count);assert.deepEqual(h.native,before);assert.doesNotMatch(h.toast,/Added .*instrument/);
 }
});
test('pending Add is private until ACK, prevents duplicates, and never attaches to a replacement project',async()=>{
 const h=await actualBrowser(),gate=base.deferred(),before=copy(h.S.state.project);h.handlers.set('sample.load',()=>gate.promise);
 const task=h.B.controller.load(true);await base.until(()=>h.calls.some(x=>x.command==='sample.load'));assert.deepEqual(copy(h.S.state.project),before);await h.B.controller.load(true);assert.equal(h.calls.filter(x=>x.command==='sample.load').length,1);
 const next=h.S.normalize({name:'Other session'});h.S.state.project=next;gate.resolve({path:'/Kick.wav',name:'Kick'});await task;assert.equal(h.S.state.project,next);assert.equal(next.instruments.length,1);assert.equal(h.S.state.undo.length,0);assert.deepEqual(h.native,copy(next));
});
test('there is no 64-instrument cap and replacing an existing instrument never adds another',async()=>{
 const h=await actualBrowser();while(h.S.state.project.instruments.length<64)h.S.state.project.instruments.push(h.S.instrumentDefaults(h.S.state.project.instruments.length,'Existing','sample'));
 await h.B.controller.load(true);assert(h.calls.some(x=>x.command==='sample.load'));assert.equal(h.S.state.project.instruments.length,65);assert.equal(h.S.state.project.instruments[64].samplePath,'/Kick.wav');
 h.S.state.track=0;h.S.state.instrument=0;await h.B.controller.load(false);assert.equal(h.S.state.project.instruments.length,65);assert.equal(h.S.state.project.instruments[0].samplePath,'/Kick.wav');assert.equal(h.S.state.view,'browser');
});
test('new sample preserves existing track plug-in and adds an independent lane',async()=>{
 const h=await actualBrowser(),old=h.plugin(0,'original');h.handlers.set('plugin.state',()=>({...old,state:'live editor knobs'}));const count=h.S.state.project.instruments.length;
 await h.B.controller.load(true);assert.equal(h.S.state.project.instruments.length,count+1);assert.equal(h.S.state.project.instruments[0].type,'au');assert.equal(h.S.state.project.audioPlugins.length,1);assert.equal(h.S.state.undo.length,1);assert.equal(h.S.state.undo[0].project.audioPlugins[0].state,old.state);
 await h.S.perform('undo');assert.equal(h.S.state.project.instruments.length,count);assert.equal(h.S.state.project.audioPlugins[0].state,old.state);
});
test('changing destination during pending Add does not overwrite or reselect the newer instrument',async()=>{
 const h=await actualBrowser(),gate=base.deferred(),count=h.S.state.project.instruments.length;h.handlers.set('sample.load',()=>gate.promise);
 const task=h.B.controller.load(true);await base.until(()=>h.calls.some(x=>x.command==='sample.load'));h.S.state.instrument=1;const selected=copy(h.S.state.project.instruments[1]);gate.resolve({path:'/Kick.wav',name:'Kick'});await task;
 assert.equal(h.S.state.instrument,1);assert.deepEqual(copy(h.S.state.project.instruments[1]),selected);assert.equal(h.S.state.project.instruments[count].samplePath,'/Kick.wav');
});
test('sample replacement preserves newer selection while unchanged selection follows the loaded destination',async()=>{
 for(const scenario of [{name:'browser selection changed',target:0,next:1,expected:1},{name:'browser selection unchanged',target:0,expected:0},{name:'explicit target selection unchanged',target:2,explicit:true,expected:2},{name:'explicit target selection changed',target:2,explicit:true,next:1,expected:1}]){
  const h=await actualBrowser(),gate=base.deferred(),before=copy(h.S.state.project.instruments),undo=h.S.state.undo.length;h.handlers.set('sample.load',()=>gate.promise);
  const task=scenario.explicit?h.S.loadSample('/Kick.wav',scenario.target):h.B.controller.load(false);await base.until(()=>h.calls.some(x=>x.command==='sample.load'));
  if(scenario.next!==undefined)h.S.state.instrument=scenario.next;
  gate.resolve({path:'/Kick.wav',name:'Kick'});await task;
  assert.equal(h.S.state.instrument,scenario.expected,scenario.name);assert.equal(h.S.state.project.instruments[scenario.target].samplePath,'/Kick.wav');assert.equal(h.native.instruments[scenario.target].samplePath,'/Kick.wav');assert.equal(h.S.state.project.tracks[0].instrument,scenario.target);
  for(let i=0;i<before.length;i++)if(i!==scenario.target)assert.deepEqual(copy(h.S.state.project.instruments[i]),before[i],`${scenario.name}: unrelated instrument ${i}`);
  assert.equal(h.S.state.undo.length,undo+1);assert.equal(h.S.state.view,'browser');
 }
});
test('removing an instrument or track while preview cancellation awaits cannot load into its numeric-index successor',async()=>{
 for(const action of ['removeInstrument','removeTrack']){
  const h=await actualBrowser(),gate=base.deferred();h.S.state.project.tracks[1].instrument=0;h.handlers.set('sample.stopPreview',()=>gate.promise);
  const task=h.B.controller.load();await base.until(()=>h.calls.some(x=>x.command==='sample.stopPreview'));
  await h.S.perform(action);const expected=copy(h.S.state.project),undo=h.S.state.undo.length;
  assert.equal(h.S.state.track,0);assert.equal(h.S.state.instrument,0,'replacement still occupies the same numeric destination');
  gate.resolve({});await task;
  assert(!h.calls.some(x=>x.command==='sample.load'),`${action}: deleted destination must cancel before native decoding`);
  assert.deepEqual(copy(h.S.state.project),expected);assert.equal(h.S.state.undo.length,undo);assert.match(h.toast,/destination changed/);
 }
});
test('ordinary sample primary, Return and double-click add; explicit secondary and Option-Return replace',async()=>{
 const h=browserHarness(),actions=[];h.window.ST.loadSampleAsNew=async path=>{actions.push(['add',path]);return true;};h.window.ST.loadSample=async(path,index)=>{actions.push(['replace',path,index]);return true;};
 h.reply=()=>({items:[sample('Kick')],total:1});await h.browser.search();const primary=h.node('browser-load'),secondary=h.node('browser-load-new'),row=h.node('browser-result-0',{browserItem:'0'},'[data-browser-item]',{tagName:'DIV'});h.browser.bind();
 const settle=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
 primary.fire('click');await settle();secondary.fire('click');await settle();key(h,row,'Enter');await settle();key(h,row,'Enter',{altKey:true});await settle();row.fire('dblclick');await settle();
 assert.deepEqual(actions.map(action=>action[0]),['add','replace','add','replace','add']);assert.match(h.browser.render(),/Add sample on new track/);assert.match(h.browser.render(),/Replace selected instrument/);
});
test('plugin button, Return and double-click add consistently; replacement gestures pass an explicit replacement option',async()=>{
 const h=browserHarness(),actions=[];h.state.plugins=[{id:'au:fixture',name:'Fixture',type:'instrument'}];h.state.project.instruments[0].type='au';h.state.project.tracks[0].instrument=0;
 h.window.STInstrumentEdits={run(){}};h.window.STNKS={async loadInstrument(command,target,options){actions.push(options);return true;}};
 const primary=h.node('browser-plugin-load'),secondary=h.node('browser-plugin-replace'),row=h.node('browser-plugin-0',{browserPlugin:'0'},'[data-browser-plugin]',{tagName:'DIV'});h.browser.showPlugins('instrument');h.browser.bind();const settle=async()=>{for(let i=0;i<30;i++)await Promise.resolve();};
 primary.fire('click');await settle();key(h,row,'Enter');await settle();row.fire('dblclick');await settle();secondary.fire('click');await settle();key(h,row,'Enter',{altKey:true});await settle();
 assert.deepEqual(actions.map(action=>action.newInstrument),[true,true,true,false,false]);assert.deepEqual(actions.map(action=>action.replaceInstrument),[false,false,false,true,true]);
 h.state.project.instruments[0].type='sample';secondary.fire('click');await settle();assert.equal(actions.at(-1).replaceInstrument,true);assert.equal(actions.at(-1).newInstrument,false);
 assert.match(h.browser.render(),/Add plug-in on new track/);assert.match(h.browser.render(),/id="browser-plugin-replace"/);
 // Sixteen tracks is not a limit (no fixed track count); only the sanity bound disables adding a track.
 while(h.state.project.tracks.length<16)h.state.project.tracks.push({});assert.doesNotMatch(h.browser.render(),/id="browser-plugin-load" disabled/);
 h.window.TRACK_SANITY_LIMIT=16;const full=h.browser.render();delete h.window.TRACK_SANITY_LIMIT;assert.match(full,/id="browser-plugin-load" disabled/);assert.doesNotMatch(full,/id="browser-plugin-replace" disabled/);
});
(async()=>{for(const t of tests){await t.fn();console.log('PASS '+t.name);}console.log(`${tests.length} browser destination groups passed`);})().catch(e=>{console.error(e);process.exitCode=1;});
