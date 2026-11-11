'use strict';
const fs=require('node:fs'),path=require('node:path'),vm=require('node:vm'),assert=require('node:assert/strict');
const web = require('./helpers/web');
const {harness:baseHarness,base}=require('./helpers/studio-session');
const copy=x=>JSON.parse(JSON.stringify(x)),tests=[],test=(name,fn)=>tests.push({name,fn});
function harness(){const h=baseHarness();h.handlers.set('transport.recordArm',payload=>({armed:payload.armed===true}));web.load(h.context, 'pattern-destination.js');h.P=h.window.STPatternDestination;h.S.state.view='pattern';h.S.refresh();return h;}
function event(target,key){return{target,key,preventDefault(){this.prevented=true;},stopPropagation(){this.stopped=true;}};}
test('destination selects retain native arrows and Enter/Escape return to the grid without transport',()=>{
 const h=harness(),select=h.element('pattern-destination-lane');select.focus();const up=event(select,'ArrowUp');select.onkeydown(up);assert(!up.prevented);
 for(const name of ['Enter','Escape']){const e=event(select,name);select.onkeydown(e);assert(e.prevented&&e.stopped);assert.equal(h.document.activeElement.id,'tracker');}assert(!h.calls.some(x=>x.command==='transport.stop'));
});
test('instrument assignment uses acknowledged transaction, one Undo, and preserves old step instrument IDs',async()=>{
 const h=harness(),before=copy(h.S.pattern().steps),undo=h.S.state.undo.length,select=h.element('pattern-destination-instrument');select.focus();select.value='1';await select.onchange({target:select});assert.equal(h.S.state.project.tracks[0].instrument,1);assert.equal(h.S.state.instrument,1);assert.equal(h.S.state.undo.length,undo+1);assert.deepEqual(copy(h.S.pattern().steps),before);assert.equal(h.document.activeElement.id,select.id);
 await h.S.perform('undo');assert.equal(h.S.state.project.tracks[0].instrument,0);
});
test('rejected assignment restores selector/current instrument without leaving an Undo',async()=>{
 const h=harness(),before=copy(h.S.state.project);h.failPublish();const select=h.element('pattern-destination-instrument');select.focus();select.value='1';assert.equal(await select.onchange({target:select}),false);assert.deepEqual(copy(h.S.state.project),before);assert.equal(h.S.state.instrument,0);assert.equal(h.S.state.undo.length,0);assert.equal(h.document.activeElement.id,select.id);
});
test('Arm action awaits accepted recording then reveals grid; failed arm cannot claim ready',async()=>{
 const h=harness(),button=h.element('pattern-enter-notes');const gate=base.deferred();h.handlers.set('transport.recordArm',()=>gate.promise);button.focus();const job=button.onclick();await base.until(()=>h.calls.some(x=>x.command==='transport.recordArm'));assert.equal(h.S.state.recording,false);assert.equal(button.disabled,true);gate.resolve({armed:true});assert.equal(await job,true);assert.equal(h.S.state.recording,true);assert.equal(h.S.state.keyboardPlay,true);assert.equal(h.document.activeElement.id,'tracker');
 const f=harness();f.handlers.set('transport.recordArm',()=>{throw Error('Arm rejected');});const rejected=f.element('pattern-enter-notes');rejected.focus();assert.equal(await rejected.onclick(),false);assert.equal(f.S.state.recording,false);assert.equal(f.document.activeElement.id,rejected.id);
});
test('Play pattern explicitly chooses Pattern independently of Song mode, and Stop remains available',async()=>{
 const h=harness();h.S.state.playbackMode='song';const button=h.element('pattern-play-current');button.focus();assert.equal(await button.onclick(),true);assert.equal(h.S.state.playbackMode,'pattern');assert.equal(h.calls.find(x=>x.command==='transport.play').payload.mode,'pattern');assert.equal(h.document.activeElement.id,'tracker');assert.equal(button.textContent,'Stop playback');await button.onclick();assert(h.calls.some(x=>x.command==='transport.stop'));assert.equal(h.S.state.playing,false);
});
test('programmatic/native-button activation focuses grid after arm even when button did not receive DOM focus',async()=>{
 const h=harness();h.element('pattern-destination-lane').focus();assert.equal(await h.element('pattern-enter-notes').onclick(),true);assert.equal(h.document.activeElement.id,'tracker');
});
test('native disabled-button focus loss still hands an accepted arm to the owned grid',async()=>{
 const h=harness();const button=h.element('pattern-enter-notes'),gate=base.deferred();let disabled=false;
 Object.defineProperty(button,'disabled',{get(){return disabled;},set(value){disabled=value;if(value&&h.document.activeElement===button)h.document.activeElement=h.document.body;}});
 h.handlers.set('transport.recordArm',()=>gate.promise);button.focus();const task=button.onclick();await base.until(()=>h.calls.some(x=>x.command==='transport.recordArm'));
 assert.equal(h.document.activeElement,h.document.body,'WebKit removes focus from the newly disabled control');gate.resolve({armed:true});assert.equal(await task,true);assert.equal(h.document.activeElement.id,'tracker');
});
test('arm completion preserves a newer control, modal or project instead of stealing focus',async()=>{
 for(const change of ['control','modal','project']){const h=harness();const button=h.element('pattern-enter-notes'),gate=base.deferred();h.handlers.set('transport.recordArm',()=>gate.promise);button.focus();const task=button.onclick();await base.until(()=>h.calls.some(x=>x.command==='transport.recordArm'));
 const other=h.element(change==='modal'?'modal-cancel':'pattern-destination-sound');other.focus();if(change==='modal')h.S.state.modal='help';if(change==='project')h.S.state.project=h.S.normalize({name:'Next'});gate.resolve({armed:true});await task;assert.equal(h.document.activeElement,other,change);}
});
test('async arm completion does not steal focus after user selects a different lane',async()=>{
 const h=harness();const gate=base.deferred();h.handlers.set('transport.recordArm',()=>gate.promise);const button=h.element('pattern-enter-notes');button.focus();const job=button.onclick();await base.until(()=>h.calls.some(x=>x.command==='transport.recordArm'));h.S.state.track=1;const other=h.element('pattern-destination-lane');other.focus();gate.resolve({armed:true});await job;assert.equal(h.document.activeElement,other);assert.equal(h.S.state.track,1);
});
test('track headers show lane identity over sound source and the Play chip collapses performance controls',async()=>{
 const h=harness();let html=h.element('#app').innerHTML;assert.match(html,/data-track-source="0" data-source-type="wavetable"[^>]*><span class="source-badge">WTB<\/span><span class="source-slot">00<\/span>/);assert.match(html,/class="track-num" aria-label="Track 1">01</);assert.match(html,/data-track-toggle="mute" data-toggle-index="0"/);
 assert.match(html,/id="perf-toggle" aria-expanded="false"/);assert.match(html,/id="perf-controls" hidden/);assert.match(html,/id="chord-set"/);assert.match(html,/id="perf-summary">C Chromatic/);
 h.element('#perf-toggle').onclick();html=h.element('#app').innerHTML;assert.match(html,/id="perf-toggle" aria-expanded="true"/);assert.doesNotMatch(html,/id="perf-controls" hidden/);h.element('#perf-toggle').onclick();
 assert(h.KG.createKit('1988'));h.S.state.view='pattern';h.S.refresh();html=h.element('#app').innerHTML;const header=html.match(/<th[^>]*class="([^"]*)"[^>]*data-track="8"[^>]*>/)?.[1]||'';assert(header.split(/\s+/).includes('group-member'));assert(header.split(/\s+/).includes('selected-track'));assert.match(html,/class="group-head[^"]*" colspan="1"[^>]*data-group-id/);assert.match(h.P.render(),/09 · Pad 01 · Group 1988/);
});
(async()=>{for(const t of tests){await t.fn();console.log('PASS '+t.name);}console.log(`${tests.length} pattern destination groups passed`);})().catch(e=>{console.error(e);process.exitCode=1;});
