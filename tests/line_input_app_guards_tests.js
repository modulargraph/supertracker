'use strict';
// The app around line inputs: while the Line inputs dialog reads the devices nothing else changes the session,
// and applying the dialog is an ordinary project edit that the next publication carries to the host.
const assert=require('node:assert/strict');
const web = require('./helpers/web');
const {harness}=require('./helpers/studio-session');
const copy=value=>JSON.parse(JSON.stringify(value));
(async()=>{
 const h=harness(),before=copy(h.S.state.project);h.window.STLineInputs={isBusy:true,closed(){throw Error('Busy input dialog cannot close');}};h.S.state.modal='line-inputs';
 assert.equal(h.S.mutate('conflicting change',p=>p.bpm=10),false);
 for(const command of ['addTrack','undo','redo','recordInput','lineInputs','view:instrument','play','new','save'])assert.equal(await h.S.perform(command),false,command);
 assert.equal(await h.S.projectAction('new'),false);assert.equal(await h.S.projectAction('save'),false);assert.equal(h.S.openModal('Other dialog',''),false);assert.equal(h.S.closeModal(),false);
 await assert.rejects(h.S.loadSample('/cannot-load.wav'),/current audio|operation/);
 assert.deepEqual(copy(h.S.state.project),before);assert.equal(h.S.state.modal,'line-inputs');assert.equal(h.S.state.undo.length,0);assert(!h.calls.some(call=>['sample.load','project.new','project.save','transport.play','audio.record'].includes(call.command)));
 console.log('PASS a busy Line inputs dialog blocks conflicting project/history/modal/recording/instrument work before native side effects');
 for(const command of ['sample.load','plugin.load','plugin.move','routing.apply','audio.configure','audio.record','engine.project'])await assert.rejects(h.S.request(command,{}),/line inputs/,command);
 assert(!h.calls.some(call=>['plugin.load','plugin.move','audio.record'].includes(call.command)));
 await assert.rejects(h.S.syncEngineProject(()=>copy(h.S.state.project)),/line inputs/);
 const oldCount=h.calls.filter(call=>call.command==='engine.project').length;
 assert.equal(await h.S.syncEngine(true),false);assert.equal(h.calls.filter(call=>call.command==='engine.project').length,oldCount,'an ordinary queued publication waits');
 assert.equal(await h.S.captureNativeEdits(),true,'a capture drains what was queued');
 assert(h.calls.filter(call=>call.command==='engine.project').length>oldCount);
 console.log('PASS direct graph commands and ordinary publication are gated; a native capture can proceed');

 {
 const r=harness(),audio={inputDevice:22,inputs:[{id:22,uid:'fixture-interface',inputChannels:2,name:'Fixture'}],lineInputs:[]};
 r.handlers.set('audio.devices',()=>audio);
 web.load(r.context, 'line-inputs.js');
 assert(await r.window.STLineInputs.show());assert.equal(r.window.STLineInputs.isBusy,false,'an open dialog holds nothing up');
 for(let i=0;i<r.S.state.project.tracks.length;i++){r.element('line-input-gain-'+i).value='1';r.element('line-input-left-'+i).value='1';r.element('line-input-right-'+i).value='mono';r.element('line-input-device-'+i).value='fixture-interface';r.element('line-input-monitor-'+i).checked=true;r.element('line-input-enabled-'+i).checked=i===0;}
 assert.equal(r.element('line-input-apply').onclick(),true);
 assert.deepEqual(copy(r.S.state.project.tracks[0].audioInput),{enabled:true,deviceUID:'fixture-interface',channels:[1],gain:1});assert.equal(r.S.state.project.tracks[1].audioInput,undefined);
 assert.equal(r.S.state.undo.length,1);assert.equal(r.S.state.modal,null);
 // The routing reaches the host as part of the project, with the next publication, and by no other command.
 await r.S.syncEngine(true);
 assert.deepEqual(copy(r.native.tracks[0].audioInput),{enabled:true,deviceUID:'fixture-interface',channels:[1],gain:1});
 assert.deepEqual([...new Set(r.calls.map(call=>call.command).filter(command=>command.startsWith('audio.')))],['audio.devices']);
 // What the host says became of it arrives as an event and is what the views show.
 r.window.nativeEvent({type:'audio.devices',payload:{lineInputs:[{track:0,state:'live',active:true,monitoring:true}],lineInputError:null}});
 assert.match(r.window.STLineInputs.mixerSection(0),/is-live">Live</);
 // Undo is an ordinary Undo, and the next publication takes the route away again.
 assert.notEqual(await r.S.perform('undo'),false);assert.equal(r.S.state.project.tracks[0].audioInput,undefined);
 await r.S.syncEngine(true);assert.equal(r.native.tracks[0].audioInput,undefined);
 console.log('PASS applying the dialog is one project edit that a publication carries to the host; Undo takes it back');
 }
})().catch(error=>{console.error(error);process.exitCode=1;});
