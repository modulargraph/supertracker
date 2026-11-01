'use strict';
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const web = require('./helpers/web');
const {harness,base}=require('./helpers/studio-session');
const {groupProject}=require('./helpers/kit-group-fixture');
const copy=x=>JSON.parse(JSON.stringify(x));
function setup(){const h=harness();web.load(h.context, 'nks-loader.js');h.S.state.view='browser';h.S.state.project.tracks.forEach((track,i)=>track.routingID='track-'+i);h.native=copy(h.S.state.project);return h;}
async function load(h){return h.window.STNKS.loadInstrument('plugin.load',{component:'vst3:test'},{name:'Test synth',stay:true});}
(async()=>{
 {
 const h=setup();await h.S.loadSample('/Kick.wav',0);await h.S.loadSample('/Snare.wav',1);const before=copy(h.S.state.project),count=before.instruments.length,tracks=before.tracks.length,undo=h.S.state.undo.length;
 // The cursor moves to the new track after the commit, so the UI must repaint once more for it.
 const repaint=h.S.refresh,painted=[];h.S.refresh=()=>{painted.push(h.S.state.track);return repaint();};
 assert(await load(h));h.S.refresh=repaint;assert.equal(painted.at(-1),tracks,'headers and sidebar repaint for the new track');const p=h.S.state.project;assert.deepEqual(copy(p.instruments.slice(0,count)),before.instruments);assert.deepEqual(copy(p.tracks.slice(0,tracks)),before.tracks);assert.equal(p.tracks.length,tracks+1);assert.equal(p.instruments.length,count+1);assert.equal(h.S.state.track,tracks);assert.equal(h.S.state.instrument,count);assert.equal(p.audioPlugins.at(-1).track,tracks);assert.equal(p.instruments[count].plugin.component,'vst3:test');assert.equal(h.S.state.undo.length,undo+1);assert.deepEqual(h.native,copy(p));
 h.S.noteOn(60,'test',false);await base.until(()=>h.calls.some(x=>x.command==='note.on'));assert.equal(h.calls.find(x=>x.command==='note.on').payload.track,tracks);assert.equal(h.calls.find(x=>x.command==='note.on').payload.instrument,count);h.S.stopHeld();
 await h.S.perform('undo');assert.deepEqual(copy(h.S.state.project),before);await h.S.perform('redo');assert.equal(h.S.state.project.tracks.length,tracks+1);
 console.log('PASS two samples retain their instruments and lanes; plug-in gets a playable new track and one Undo');
 }
 for(const failure of ['load','capture','publish']){
 const h=setup(),before=copy(h.S.state.project),selection=[h.S.state.track,h.S.state.instrument];
 if(failure==='load')h.handlers.set('plugin.load',()=>{throw Error('Load failed');});
 if(failure==='capture')h.handlers.set('plugin.state',()=>({stateError:'capture failed'}));
 if(failure==='publish'){const original=h.handlers.get('engine.project');let n=0;h.handlers.set('engine.project',p=>++n===3?false:original(p));}
 await assert.rejects(load(h));assert.deepEqual(copy(h.S.state.project),before,failure);assert.deepEqual(h.native,before,failure);assert.equal(h.S.state.undo.length,0);assert.deepEqual([h.S.state.track,h.S.state.instrument],selection);
 }
 console.log('PASS load, capture and final ACK failures remove staged native track without project/history/selection damage');
 {
 const h=setup(),gate=base.deferred(),before=copy(h.S.state.project);const original=h.handlers.get('plugin.load');h.handlers.set('plugin.load',async p=>{await gate.promise;return original(p);});const pending=load(h);await base.until(()=>h.calls.some(x=>x.command==='plugin.load'));assert.deepEqual(copy(h.S.state.project),before);h.S.state.instrument=1;gate.resolve();await pending;assert.equal(h.S.state.instrument,1);assert.equal(h.S.state.track,0);
 console.log('PASS pending native track stays private and completion preserves a newer selection');
 }
 {
 const h=setup();h.plugin();const count=h.S.state.project.instruments.length,tracks=h.S.state.project.tracks.length;await h.window.STNKS.loadInstrument('plugin.load',{component:'vst3:test'},{stay:true,replaceInstrument:true});assert.equal(h.S.state.project.tracks.length,tracks);assert.equal(h.S.state.project.instruments.length,count);assert.equal(h.S.state.project.audioPlugins[0].component,'vst3:test');
 console.log('PASS an explicitly selected existing plug-in destination is replaced without adding lanes');
 }
 {
 const h=setup(),warnings=Array.from({length:80},(_,i)=>`Unsupported <setting ${i}> was omitted.`);h.handlers.set('kit.read',()=>({groupProject:groupProject({name:'Large'}),warnings:[...warnings,warnings[0]]}));assert(await h.KG.loadKit('/large.stkit'));assert(h.toast.length<250);assert.match(h.toast,/80 import notes/);assert.equal(h.S.state.modal,null);const group=h.G.list(h.S.state.project).at(-1);assert.equal(group.importNotes.length,80);assert.equal(group.libraryPath,'/large.stkit');
 console.log('PASS large kit reports use a concise status and complete escaped collapsed details');
 }
 {
 const h=setup();h.handlers.set('plugin.loadNKS',p=>h.handlers.get('plugin.load')({...p,component:'vst3:nks',nks:{name:'Mapped'}}));const count=h.S.state.project.tracks.length;assert(await h.window.STNKS.load('/mapped.nksf',{stay:true}));assert.equal(h.S.state.track,count);assert.equal(h.S.state.project.audioPlugins.at(-1).nks.name,'Mapped');
 console.log('PASS NKS uses the same new-track transaction and binds the playable destination');
 }
 {
 const h=setup(),source=h.plugin(0,'vst3:source');source.owner='track:track-0';h.S.state.project.instruments[0].plugin=copy(source);h.native=copy(h.S.state.project);h.S.state.track=1;h.S.state.instrument=0;
 const original=h.handlers.get('plugin.load');h.handlers.set('plugin.load',p=>{assert.equal(p.owner,undefined,'source owner must not override destination track');assert.equal(p.instanceID,undefined,'fresh lane needs independent identity');return original({...p,owner:'track:track-1',instanceID:'fresh-target'});});
 assert(await h.S.perform('assignInstrument'));assert.equal(h.S.state.project.audioPlugins.find(p=>p.track===1).owner,'track:track-1');assert.equal(h.S.state.project.audioPlugins.find(p=>p.track===0).instanceID,source.instanceID);
 console.log('PASS assigning a plug-in strips old owner and instance identity, preserving source and binding destination');
 }
 {
 const h=setup();h.S.state.project.tracks.forEach(track=>delete track.routingID);h.native=copy(h.S.state.project);await load(h);assert.deepEqual(h.native,copy(h.S.state.project));assert(h.S.state.project.tracks.every(track=>track.routingID));
 console.log('PASS legacy missing routing IDs are stable across native proposal and canonical commit');
 }
 {
 const h=setup();h.plugin(0,'vst3:source');h.S.state.track=1;h.S.state.instrument=0;
 const publish=h.handlers.get('engine.project');let revision=0;
 h.handlers.set('engine.project',payload=>{const result=publish(payload);const plugins=copy(h.native.audioPlugins).map(plugin=>plugin.track===0?{...plugin,latency:++revision}:plugin);h.window.nativeEvent({type:'plugins.changed',payload:{plugins}});return result;});
 assert(await h.S.perform('assignInstrument'),'source descriptor refresh during candidate publication must not invalidate assignment');assert.equal(h.S.state.project.audioPlugins.filter(p=>p.kind==='instrument').length,2);assert.equal(h.S.state.project.tracks[1].instrument,0);
 console.log('PASS source and destination remain protected against native descriptor refresh during assignment publication');
 }
 {
 const h=setup();await h.S.loadSample('/replace-me.wav',0);const before=copy(h.S.state.project),undo=h.S.state.undo.length;
 assert(await h.window.STNKS.loadInstrument('plugin.load',{component:'vst3:replacement'},{stay:true,replaceInstrument:true}));
 assert.equal(h.S.state.project.tracks.length,before.tracks.length);assert.equal(h.S.state.project.instruments.length,before.instruments.length);assert.equal(h.S.state.project.instruments[0].type,'au');assert.equal(h.S.state.project.audioPlugins[0].track,0);assert.equal(h.S.state.undo.length,undo+1);
 await h.S.perform('undo');assert.deepEqual(copy(h.S.state.project),before);
 console.log('PASS explicit plug-in replacement uses selected sample lane and one Undo restores the sample');
 }
 {
 const h=setup();h.plugin();const before=copy(h.S.state.project);
 assert(await h.window.STNKS.loadInstrument('plugin.load',{component:'vst3:added'},{stay:true,newInstrument:true,replaceInstrument:false}));
 assert.equal(h.S.state.project.tracks.length,before.tracks.length+1);assert.equal(h.S.state.project.instruments.length,before.instruments.length+1);assert.equal(h.S.state.project.audioPlugins[0].component,before.audioPlugins[0].component);assert.equal(h.S.state.project.audioPlugins.length,2);
 console.log('PASS primary browser plug-in options add beside an existing AU without replacing its lane');
 }
})().catch(e=>{console.error(e);process.exitCode=1;});
