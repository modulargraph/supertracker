'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web=name=>fs.readFileSync(path.join(process.env.ST_DISCRETE_SOURCE||path.join(__dirname,'../Resources/Web'),name),'utf8');
const fixtureRequire=name=>name==='node:fs'?{...fs,readFileSync(filename,...options){const base=path.basename(String(filename));return fs.readFileSync(process.env.ST_DISCRETE_SOURCE&&['nks-controls.js','plugin-pages.js','app.js','fine-control.js','automation-record.js'].includes(base)?path.join(process.env.ST_DISCRETE_SOURCE,base):filename,...options);}}:require(name);
function fixture(name,addition=''){const src=fs.readFileSync(path.join(__dirname,name),'utf8').replace(/^#!.*\n/,'');return new Function('require','__dirname',src.slice(0,src.indexOf('(async'))+addition+'\nreturn harness;')(fixtureRequire,__dirname);}
const nksHarness=fixture('nks_controls_ui_tests.js');
const ppSource=fs.readFileSync(path.join(__dirname,'plugin_pages_tests.js'),'utf8');
const pagesHarness=new Function('require','__dirname',ppSource.slice(0,ppSource.indexOf('(async')).replace('return { api: context.window.STPluginPages,','return { window: context.window, api: context.window.STPluginPages,')+'\nreturn harness;')(fixtureRequire,__dirname);
const dialogSource=fs.readFileSync(path.join(__dirname,'plugin_parameter_dialog_tests.js'),'utf8');
const dialogHarness=new Function('require','__dirname',dialogSource.slice(0,dialogSource.indexOf('const tests=[]'))+'\nreturn harness;')(fixtureRequire,__dirname);
const recorderSource=fs.readFileSync(path.join(__dirname,'automation_record_tests.js'),'utf8');
const recorderHarness=new Function('require','__dirname',recorderSource.slice(0,recorderSource.indexOf('const tests = []'))+'\nreturn harness;')(fixtureRequire,__dirname);
const loadFine=window=>vm.runInNewContext(web('fine-control.js'),{window});
const tick=async()=>{for(let i=0;i<45;i++)await Promise.resolve();};
const tests=[],test=(name,run)=>tests.push({name,run});
test('binary NKS keyboard and Option step changes one legal value immediately',async()=>{
 const h=nksHarness();loadFine(h.window);Object.assign(h.parameters[0],{stepCount:1,normalizedStep:1,normalized:0});h.mount();await tick();const el=h.input(0,'range');
 assert.equal(Number(el.step),1);assert.equal(el.dataset.parameterSteps,'1');
 h.root.fire('keydown',{target:el,key:'ArrowUp',altKey:true});assert.equal(Number(el.value),1);h.root.fire('keyup',{target:el,key:'ArrowUp'});await tick();
 assert(h.requests.some(x=>x.command==='plugin.parameter'&&x.payload.value===1));
 assert.equal(h.api.controller.view().knobs[0].stepCount,1);
});
test('typed NKS values and hardware adjustments snap to the native enum grid',async()=>{
 const h=nksHarness();loadFine(h.window);Object.assign(h.parameters[0],{stepCount:3,normalizedStep:1/3,normalized:0});h.mount();await tick();h.change(.53);await tick();
 const values=h.requests.filter(x=>x.command==='plugin.parameter').map(x=>x.payload.value);assert(Math.abs(values.at(-1)-2/3)<1e-12,String(values));
 assert(Math.abs(h.api.controller.view().knobs[0].normalizedStep-1/3)<1e-12);
 h.api.controller.adjust(0,-1/3);h.api.controller.finish();await tick();assert(Math.abs(h.requests.filter(x=>x.command==='plugin.parameter').at(-1).payload.value-1/3)<1e-12);
});
test('generic pages expose metadata and snap preview and hardware values',async()=>{
 const h=pagesHarness();loadFine(h.window);Object.assign(h.instrument.parameters[0],{stepCount:3,normalizedStep:1/3,normalized:0});const entry=h.api._test.entryFor(h.instrument);
 const markup=h.api.render(null,h.instrument);assert.match(markup,/data-parameter-steps="3"/);
 h.api.controller.adjust(0,.53);h.api.controller.finish();await h.api.flushEdits();assert(Math.abs(h.calls.filter(x=>x.command==='plugin.parameter').at(-1).payload.value-2/3)<1e-12);
 const knob=h.api.controller.view().knobs[0];assert.equal(knob.stepCount,3);assert.equal(knob.normalizedStep,1/3);
});
test('generic parameter dialog arrows and pointer edits snap before sending native writes',async()=>{
 const h=dialogHarness();loadFine(h.window);Object.assign(h.native.parameters[0],{stepCount:3,normalizedStep:1/3,normalized:0,value:0});await h.open();
 assert.equal(h.input.dataset.parameterSteps,'3');
 const event={key:'ArrowUp',altKey:true,preventDefault(){this.prevented=true;},stopPropagation(){}};h.input.onkeydown(event);assert(event.prevented);assert.equal(Number(h.input.value),1/3);h.input.onkeyup({key:'ArrowUp'});await tick();
 assert.equal(h.calls.filter(x=>x.command==='plugin.parameter').at(-1).payload.value,1/3);
 await h.inputValue(.8);assert.equal(h.calls.filter(x=>x.command==='plugin.parameter').at(-1).payload.value,2/3);
});
test('automation writes the legal discrete keyboard value without changing instrument base controls',()=>{
 const h=recorderHarness({fine:true}),target='plugin:instrument:0:7';h.R.setMode('touch',true);h.status(4);
 const el=h.element({target,value:0,min:0,max:1,step:1,dataset:{parameterSteps:'1'}});
 const event=h.fire('keydown',{key:'ArrowUp',altKey:true,target:el});assert(event.prevented);assert.equal(Number(el.value),1);
 h.status(5);h.advance(700);h.status(6);h.stop();assert(h.lane(target).points.some(p=>p.value===1));assert.equal(h.mutations.length,1);
});
test('discrete helper reaches endpoints without dead fine steps or accumulated drift',()=>{
 const window={};loadFine(window);const F=window.STFine;
 for(const count of [1,2,3,7,127]){const info={stepCount:count};let v=0;for(let i=0;i<count;i++)v=F.stepParameter(v,1,{fine:true},info);assert.equal(v,1);for(let i=0;i<count;i++)v=F.stepParameter(v,-1,{fine:true},info);assert.equal(v,0);}
 assert.equal(F.stepParameter(.25,1,{},{}),.26);assert.equal(F.stepParameter(.25,1,{fine:true},{}),.251);assert.equal(F.stepParameter(.25,1,{coarse:true},{}),.35);
});
(async()=>{let failed=0;for(const{name,run}of tests){try{await run();console.log('PASS '+name);}catch(error){failed++;console.error('FAIL '+name+'\n'+error.stack);}}console.log(`${tests.length-failed}/${tests.length} plug-in discrete range groups passed`);if(failed)process.exitCode=1;})();
