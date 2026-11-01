'use strict';
const fs=require('node:fs'),path=require('node:path'),assert=require('node:assert/strict'),vm=require('node:vm');
const web = require('./helpers/web');
const {harness,base}=require('./helpers/studio-session');
const copy=value=>JSON.parse(JSON.stringify(value));
(async()=>{
 const h=harness(),before=copy(h.S.state.project);h.window.STLineInputs={isBusy:true,closed(){throw Error('Busy input dialog cannot close');}};h.S.state.modal='line-inputs';
 assert.equal(h.S.mutate('conflicting change',p=>p.bpm=10),false);
 for(const command of ['addTrack','undo','redo','recordInput','lineInputs','view:instrument','play','new','save'])assert.equal(await h.S.perform(command),false,command);
 assert.equal(await h.S.projectAction('new'),false);assert.equal(await h.S.projectAction('save'),false);assert.equal(h.S.openModal('Other dialog',''),false);assert.equal(h.S.closeModal(),false);
 await assert.rejects(h.S.loadSample('/cannot-load.wav'),/current audio|operation/);
 assert.deepEqual(copy(h.S.state.project),before);assert.equal(h.S.state.modal,'line-inputs');assert.equal(h.S.state.undo.length,0);assert(!h.calls.some(call=>['sample.load','project.new','project.save','transport.play','audio.record'].includes(call.command)));
 console.log('PASS busy line-input apply blocks conflicting project/history/modal/recording/instrument work before native side effects');
 h.S.mutate('owned line-input commit',p=>p.tracks[0].audioInput={enabled:false},{lineInputCommit:true});assert.equal(h.S.state.project.tracks[0].audioInput.enabled,false);assert.equal(h.S.state.undo.length,1);
 console.log('PASS acknowledged line-input owner can make its single canonical history commit');
 for(const command of ['sample.load','plugin.load','plugin.move','routing.apply','audio.configure','audio.record','audio.inputs','engine.project'])await assert.rejects(h.S.request(command,{}),/line inputs/,command);
 assert(!h.calls.some(call=>['audio.inputs','plugin.load','plugin.move','audio.record'].includes(call.command)));
 await assert.rejects(h.S.syncEngineProject(()=>copy(h.S.state.project)),/line inputs/);
 const oldCount=h.calls.filter(call=>call.command==='engine.project').length;
 assert.equal(await h.S.syncEngine(true),false);assert.equal(h.calls.filter(call=>call.command==='engine.project').length,oldCount,'ordinary queued native sync must defer during apply');
 assert.equal(await h.S.captureNativeEdits(),true,'capture must drain old queued state before line input native edit');
 assert(h.calls.filter(call=>call.command==='engine.project').length>oldCount);
 assert.notEqual(await h.S.syncEngineProject(()=>copy(h.S.state.project),{lineInputCommit:true}),false,'owned rollback publication remains available');
 await h.S.request('audio.inputs',{inputs:[]},{lineInputCommit:true});
 console.log('PASS direct graph commands and ordinary publication are gated; native capture drain and owned rollback can proceed');

 {
 const r=harness(),gate=base.deferred(),audio={inputDevice:22,inputs:[{id:22,uid:'fixture-interface',inputChannels:2,name:'Fixture'}],lineInputs:[]};
 r.handlers.set('audio.devices',()=>audio);r.handlers.set('audio.inputs',payload=>{const native=copy(r.native);for(const {track,...value} of payload.inputs)native.tracks[track].audioInput=value;r.native=native;return audio;});
 web.load(r.context, 'line-inputs.js');
 assert(await r.window.STLineInputs.show());for(let i=0;i<r.S.state.project.tracks.length;i++){r.element('line-input-gain-'+i).value='1';r.element('line-input-enabled-'+i).checked=i===0;}
 const original=r.handlers.get('engine.project');let first=true;r.handlers.set('engine.project',async payload=>{if(first){first=false;await gate.promise;}return original(payload);});
 const sync=r.S.syncEngine(true);await base.until(()=>r.calls.some(call=>call.command==='engine.project'));
 const apply=r.element('line-input-apply').onclick();await Promise.resolve();await Promise.resolve();assert(r.window.STLineInputs.isBusy);assert(!r.calls.some(call=>call.command==='audio.inputs'),'native line edit must wait for old in-flight project publication');
 gate.resolve();await sync;await apply;assert.equal(r.S.state.project.tracks[0].audioInput.enabled,true);assert.equal(r.native.tracks[0].audioInput.enabled,true);assert.equal(r.S.state.undo.length,1);
 console.log('PASS actual input apply drains an already-posted engine project before changing native inputs');
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
