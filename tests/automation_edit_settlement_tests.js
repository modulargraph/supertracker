'use strict';
// Real recorder + studio/song handlers. DOM replacement is modeled; no native/audio execution.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web = require('./helpers/web');
const fixture=fs.readFileSync(path.join(__dirname,'studio_navigation_tests.js'),'utf8');
const loadRequire=name=>name==='node:fs'?{...fs,readFileSync(file,...rest){return fs.readFileSync(process.env.SETTLEMENT_STUDIO_SOURCE&&path.basename(String(file))==='studio.js'?process.env.SETTLEMENT_STUDIO_SOURCE:file,...rest);}}:require(name);
const base=new Function('require','__dirname',fixture.slice(0,fixture.indexOf("test('Left/Right"))+'\nreturn {harness,mountAutomation};')(loadRequire,__dirname);
const copy=x=>JSON.parse(JSON.stringify(x));
function harness(){
 const h=base.harness(),ST=h.context.window.ST,toasts=[];let uid=0;
 h.state.undo=[];h.state.project.bpm=120;h.state.project.linesPerBeat=4;h.state.project.sequence=[1,2];
 h.state.project.patterns[0].automation[0].points=[{row:0,value:.8},{row:16,value:.4}];
 h.state.project.patterns.push({id:2,name:'Verse',length:64,automation:[],steps:[[{note:60}],[],[],[]]});
 h.context.Date={now:()=>100000};ST.uid=()=>`settlement-${++uid}`;ST.toast=message=>toasts.push(message);ST.syncEngineProject=()=>Promise.resolve(true);
 for(const name of ['song-model.js','automation-song.js','automation-record.js'])web.load(h.context,name,{path:name==='automation-song.js'&&process.env.SETTLEMENT_SONG_SOURCE});
 const render=base.mountAutomation(h);ST.refresh=render;
 ST.mutate=(label,fn,options={})=>{const before=copy(h.state.project);h.changes.push({label,before});h.state.undo.push({label,project:before});fn(h.state.project);if(options.render!==false)render();return true;};
 const R=h.context.window.STAutomationRecord;
 const status=(position,playing=true)=>{Object.assign(h.state,{playing,lastStatusAt:100000,status:{playing,pattern:1,position,row:Math.floor(position),bpm:120}});R.event('status',h.state.status);};
 const record=()=>{R.setMode('touch',true);status(4);const ref=R.makeRef(0,'volume','Volume');R.begin(ref,.2);status(6);R.value(ref,.9);R.end(ref);assert(R.hasPendingEdits);assert.equal(h.changes.length,0);};
 return {...h,ST,R,record,status,render,toasts,lane:()=>h.state.project.patterns[0].automation.find(l=>l.target==='volume'),click:id=>h.nodes.get(id).fire('click')};
}
const tests=[];const test=(name,fn)=>tests.push({name,fn});
for(const action of ['clear','delete'])test(`${action} settles the take first and Stop cannot resurrect it`,()=>{
 const h=harness();h.record();h.nodes.get(action==='clear'?'automation-clear':'automation-delete-lane').focus();h.click(action==='clear'?'automation-clear':'automation-delete-lane');
 assert.equal(h.R.hasPendingEdits,false);assert.equal(h.changes.length,2);assert.match(h.changes[0].label,/Record automation/);
 assert(h.changes[1].before.patterns[0].automation[0].points.some(p=>p.value===.9),'Undo of the edit restores the recorded take');
 if(action==='clear')assert.equal(h.lane().points.length,0);else assert.equal(h.lane(),undefined);
 const edited=copy(h.state.project);h.status(0,false);assert.deepEqual(copy(h.state.project),edited);assert.equal(h.changes.length,2);
});
for(const action of ['arrow','number','add'])test(`${action} reads settled points rather than discarding the new take`,()=>{
 const h=harness();h.record();
 if(action==='arrow'){h.nodes.get('automation-envelope').focus();h.key('automation-envelope','ArrowUp');}
 else if(action==='number'){const field=h.nodes.get('point-value-0');field.focus();field.value='.3';field.fire('change');}
 else h.click('automation-add-point');
 assert.equal(h.R.hasPendingEdits,false);assert.equal(h.changes.length,2);assert(h.lane().points.some(p=>p.value===.9));
 if(action==='number')assert.equal(h.lane().points[0].value,.3);if(action==='arrow')assert(h.lane().points[0].value>.8);
 const edited=copy(h.state.project);h.status(0,false);assert.deepEqual(copy(h.state.project),edited);
});
test('copy to another pattern includes the buffered recording',()=>{
 const h=harness();h.record();assert.equal(h.studio.command('automationCopyLaneUsing'),1);const copied=h.state.project.patterns[1].automation[0];
 assert(copied.points.some(p=>p.value===.9));assert.deepEqual(copy(copied.points),copy(h.lane().points));assert.equal(h.changes.length,2);assert.equal(h.R.hasPendingEdits,false);
});
test('song sweep settles first; Undo restores the take and later Stop preserves the sweep',()=>{
 const h=harness();h.record();h.studio.setScope('song');
 const values={'automation-sweep-from-slot':0,'automation-sweep-from-row':0,'automation-sweep-from-value':.1,'automation-sweep-to-slot':1,'automation-sweep-to-row':20,'automation-sweep-to-value':.7};
 for(const [id,value]of Object.entries(values))h.nodes.get(id).value=String(value);
 h.nodes.get('automation-sweep').fire('submit');assert.equal(h.R.hasPendingEdits,false);assert.equal(h.changes.length,2);
 assert(h.changes[1].before.patterns[0].automation[0].points.some(p=>p.value===.9));assert.equal(h.lane().points[0].value,.1);
 const swept=copy(h.state.project);h.status(0,false);assert.deepEqual(copy(h.state.project),swept);
});
test('song point edits preserve other newly recorded points',()=>{
 const h=harness();h.record();h.studio.setScope('song');h.nodes.get('automation-song-envelope').focus();h.nodes.get('automation-song-envelope').fire('keydown',{key:'ArrowUp'});
 assert.equal(h.changes.length,2);assert(h.lane().points.some(p=>p.value===.9));assert.equal(h.R.hasPendingEdits,false);
});
for(const action of ['clear','delete','number','arrow','copy','sweep'])test(`refused settlement leaves ${action} data/history/focus intact`,()=>{
 const h=harness();h.record();if(action==='sweep')h.studio.setScope('song');
 h.context.window.STRouting={isApplying:true};const before=copy(h.state.project);
 const id={clear:'automation-clear',delete:'automation-delete-lane',number:'point-value-0',arrow:'automation-envelope',copy:'automation-copy-lane',sweep:'automation-sweep-from-value'}[action];const control=h.nodes.get(id);control.focus();
 if(action==='number'){control.value='.3';control.fire('change');}
 else if(action==='arrow')h.key(control,'ArrowUp');else if(action==='copy')h.studio.command('automationCopyLaneUsing');
 else if(action==='sweep'){for(const [id,value]of Object.entries({'automation-sweep-from-slot':0,'automation-sweep-from-row':0,'automation-sweep-from-value':.1,'automation-sweep-to-slot':1,'automation-sweep-to-row':20,'automation-sweep-to-value':.7}))h.nodes.get(id).value=String(value);h.nodes.get('automation-sweep').fire('submit');}
 else h.click(id);
 assert.deepEqual(copy(h.state.project),before);assert.equal(h.changes.length,0);assert(h.R.hasPendingEdits);assert.equal(h.document.activeElement,control);assert(h.toasts.some(message=>/pending/.test(message)));
});
test('point navigation alone does not commit the live take or create Undo',()=>{
 const h=harness();h.record();h.key('automation-envelope','End');h.key('automation-envelope','ArrowLeft',{altKey:true});
 assert(h.R.hasPendingEdits);assert.equal(h.changes.length,0);
});
let failed=0;for(const {name,fn}of tests){try{fn();console.log('PASS '+name);}catch(error){failed++;console.error('FAIL '+name+'\n'+error.stack);}}
if(failed)process.exitCode=1;else console.log(`${tests.length} automation edit settlement groups passed`);
