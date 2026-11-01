'use strict';
// Actual sampler command and app transaction/bridge handlers, deterministic native replies.
// No app, native engine, audio, or file panels are executed.
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const web = require('./helpers/web');
const fixture=fs.readFileSync(path.join(__dirname,'instrument_edits_ui_tests.js'),'utf8');
const prefix=fixture.slice(0,fixture.indexOf('async function run(){')).replace('return {S,window,calls,handlers,element,plugin,','return {S,window,calls,handlers,element,plugin,context,');
const base=new Function('require','__dirname',prefix+'\nreturn {harness,deferred,until};')(require,__dirname);
const copy=value=>JSON.parse(JSON.stringify(value));
function harness(){
 const h=base.harness();h.S.state.view='pattern';h.S.state.instrument=0;
 Object.assign(h.S.state.project.instruments[0],{type:'sample',samplePath:'/original.wav',start:.2,end:.8,loopStart:.3,loopEnd:.7});
 h.S.state.samples[0]={path:'/original.wav',frames:48000};h.native=copy(h.S.state.project);
 web.load(h.context, 'sampler.js', { path: process.env.SAMPLER_TRANSACTION_SOURCE });
 h.sampler=h.window.STSampler;return h;
}
const tests=[];const test=(name,fn)=>tests.push({name,fn});
test('native sample processing and candidate ACK settle before metadata/Undo commit',async()=>{
 const h=harness(),gate=base.deferred(),oldMeta=h.S.state.samples[0];let n=0;const publish=h.handlers.get('engine.project');
 h.handlers.set('engine.project',payload=>++n===2?gate.promise:publish(payload));
 const task=h.sampler.command('trim');await base.until(()=>n===2);
 assert(h.window.STInstrumentEdits.isBusy);assert.equal(h.S.state.project.instruments[0].samplePath,'/original.wav');assert.equal(h.S.state.samples[0],oldMeta);assert.equal(h.S.state.undo.length,0);
 gate.resolve({});await task;assert.equal(h.S.state.project.instruments[0].samplePath,'/edited.wav');assert.equal(h.S.state.samples[0].path,'/edited.wav');assert.equal(h.S.state.undo.length,1);assert.equal(h.S.state.undo[0].project.instruments[0].start,.2);assert.equal(h.window.STInstrumentEdits.isBusy,false);
});
test('Save waits for a pending sampler edit and saves the accepted result',async()=>{
 const h=harness(),gate=base.deferred();h.handlers.set('sample.edit',()=>gate.promise);
 const task=h.sampler.command('reverse');await base.until(()=>h.calls.some(x=>x.command==='sample.edit'));
 const saving=h.S.projectAction('save');await Promise.resolve();assert(!h.calls.some(x=>x.command==='project.save'));
 gate.resolve({path:'/reversed.wav',frames:48000});await task;await saving;
 assert.equal(h.calls.find(x=>x.command==='project.save').payload.project.instruments[0].samplePath,'/reversed.wav');
});
test('stale markers restore the current canonical project with replace and no metadata edit',async()=>{
 const h=harness(),gate=base.deferred();h.handlers.set('sample.edit',()=>gate.promise);const task=h.sampler.command('trim');
 await base.until(()=>h.calls.some(x=>x.command==='sample.edit'));h.S.state.project.instruments[0].start=.4;gate.resolve({path:'/stale.wav'});await task;
 const restore=h.calls.find(x=>x.command==='engine.project'&&x.payload.replace===true);assert(restore);assert.equal(restore.payload.project.instruments[0].start,.4);
 assert.equal(h.S.state.project.instruments[0].samplePath,'/original.wav');assert.equal(h.S.state.samples[0].path,'/original.wav');assert.equal(h.S.state.undo.length,0);
});
test('failed restoration acknowledgment remains a Save/capture fence',async()=>{
 const h=harness();h.window.STFreeze={checkMutation:()=>({message:'locked'}),explain(){}};
 const publish=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>p.replace?false:publish(p));
 await h.sampler.command('trim');assert.equal(h.S.state.project.instruments[0].samplePath,'/original.wav');assert.equal(h.S.state.samples[0].path,'/original.wav');assert.equal(h.S.state.undo.length,0);
 await assert.rejects(h.window.STInstrumentEdits.waitForIdle(),/not acknowledged/);assert.equal(await h.S.captureNativeEdits(),false);assert.match(h.toast,/not acknowledged/);
});
test('a partial native sample error restores before releasing transaction ownership',async()=>{
 const h=harness(),restore=base.deferred();h.handlers.set('sample.edit',()=>{throw Error('Partial sample failure');});
 const publish=h.handlers.get('engine.project');h.handlers.set('engine.project',p=>p.replace?restore.promise:publish(p));
 const task=h.sampler.command('normalize');await base.until(()=>h.calls.some(x=>x.command==='engine.project'&&x.payload.replace));assert(h.window.STInstrumentEdits.isBusy);
 restore.resolve({});await task;assert.equal(h.window.STInstrumentEdits.isBusy,false);assert.equal(h.S.state.undo.length,0);assert.equal(h.S.state.samples[0].path,'/original.wav');
});
(async()=>{for(const {name,fn}of tests){await fn();console.log('PASS '+name);}console.log(`${tests.length} sampler transaction groups passed`);})().catch(error=>{console.error(error);process.exitCode=1;});
