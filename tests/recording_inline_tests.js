'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path');
const fixture=fs.readFileSync(path.join(__dirname,'recording_ui_tests.js'),'utf8');
const base=new Function('require','__dirname',fixture.slice(0,fixture.indexOf('const tests = [], test ='))+'\nreturn {harness,wait};')(require,__dirname);
const copy=x=>JSON.parse(JSON.stringify(x));
function harness(overrides={}){
 const h=base.harness(overrides),parse=h.ST.openModal;
 const host={id:'recording-inline-host'};
 Object.defineProperty(host,'innerHTML',{set(html){parse('Inline',html,'',{kind:null});h.nodes.set(host.id,host);}});
 h.state.view='recording';h.nodes.set(host.id,host);
 return {...h,async mount(){return h.ui.mountInline();},remount(){h.ui.rememberInline();return h.ui.mountInline();}};
}
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('Recording workspace mounts real capture controls without a modal or permission request',async()=>{
 const h=harness();await h.mount();assert.equal(h.state.modal,null);assert.equal(h.get('record-destination').value,'newTrack');assert.equal(h.get('record-start').disabled,false);assert(!h.calls.some(c=>c.command==='audio.record'||c.command==='audio.configure'));assert(h.ui.inlineActive);
});
test('optional playback controls start collapsed and preserve expansion across refresh',async()=>{
 const h=harness();await h.mount();assert.equal(h.get('record-advanced').open,false);assert.equal(h.get('record-advanced-summary').textContent,'Monitor off');h.get('record-advanced').open=true;h.get('record-advanced').fire('toggle');h.remount();assert.equal(h.get('record-advanced').open,true);h.get('record-monitor').checked=true;h.get('record-monitor').fire('change');assert.equal(h.get('record-advanced-summary').textContent,'Monitor on');
});
test('keys checkbox sits outside the collapsed section and always mirrors and toggles Keys on/off',async()=>{
 const h=harness(),performed=[];h.ST.perform=command=>{performed.push(command);if(command==='keyboard')h.state.keyboardPlay=!h.state.keyboardPlay;return true;};
 await h.mount();const order=[...h.nodes.keys()];assert(order.indexOf('record-keyboard')<order.indexOf('record-advanced'),'visible without expanding Playback & monitoring');
 assert.equal(h.get('record-keyboard').checked,true);h.get('record-keyboard').checked=false;h.get('record-keyboard').fire('change');assert.deepEqual(performed,['keyboard']);assert.equal(h.state.keyboardPlay,false);
 // A remount (refresh) reflects the live toggle, not the checkbox state remembered in the form.
 h.state.keyboardPlay=true;h.remount();assert.equal(h.get('record-keyboard').checked,true);h.state.keyboardPlay=false;h.remount();assert.equal(h.get('record-keyboard').checked,false);
 // A refused toggle (e.g. a busy guard) snaps the checkbox back.
 h.ST.perform=()=>false;h.get('record-keyboard').checked=true;h.get('record-keyboard').fire('change');assert.equal(h.get('record-keyboard').checked,false);
});
test('refresh preserves edited source, channel map, name, root and checked options',async()=>{
 const h=harness();await h.mount();h.change('record-source','master');h.get('record-name').value='Resampled phrase';h.get('record-name').fire('input');h.get('record-root-note').value='53';h.get('record-root-note').fire('change');h.get('record-start-playback').checked=false;h.get('record-start-playback').fire('change');h.remount();
 assert.equal(h.get('record-source').value,'master');assert.equal(h.get('record-name').value,'Resampled phrase');assert.equal(h.get('record-root-note').value,'53');assert.equal(h.get('record-start-playback').checked,false);assert.equal(h.state.modal,null);
});
test('inline master capture survives repaint, blocks leaving, and loads one new sampler lane',async()=>{
 const h=harness();await h.mount();h.change('record-source','master');const before=copy(h.state.project);await h.startStop();assert(h.ui.isRecording);assert.equal(h.ui.leaveInline(),false);h.ui.event('status',{recording:true,recordingDuration:.3,peakL:.25,peakR:.1});h.remount();assert(h.ui.isRecording);assert.equal(h.get('record-duration').textContent,'0.3 s');assert.match(h.get('record-start').textContent,/Stop/);await h.startStop();
 assert.equal(h.state.modal,null);assert.equal(h.state.view,'instrument');assert.equal(h.state.project.tracks.length,before.tracks.length+1);assert.deepEqual(copy(h.state.project.instruments.slice(0,before.instruments.length)),before.instruments);assert.equal(h.ui.inlineActive,false);assert.equal(h.edits.length,1);
});
test('failed inline capture remains retryable and input check restores when leaving idle',async()=>{
 const h=harness({'audio.record':()=>{throw new Error('Cannot open input');}});await h.mount();await h.startStop();assert(!h.ui.isRecording);assert.match(h.get('record-status').textContent,/Cannot open input/);h.remount();assert.match(h.get('record-status').textContent,/Cannot open input/);await h.get('record-meter-enable').fire('click');assert.equal(h.ui.leaveInline(),true);for(let i=0;i<8;i++)await Promise.resolve();assert.equal(h.audio.inputMetering,false);assert.equal(h.ui.inlineActive,false);
});
test('decoding a saved inline take blocks reset and leaving until its guarded attachment settles',async()=>{
 const gate=base.wait(),h=harness({'sample.load':()=>gate.promise});await h.mount();await h.startStop();const stop=h.startStop();for(let i=0;i<20;i++)await Promise.resolve();assert(h.ui.isBusy);assert.equal(h.ui.leaveInline(),false);assert(h.get('record-close').disabled);gate.resolve({path:'/recorded.wav',duration:1});await stop;assert.equal(h.state.view,'instrument');assert(!h.ui.isBusy);
});
test('finalized partial event before rejected stop loads preserved audio on a new lane',async()=>{
 let h;h=harness({'audio.record':()=>({recording:true,path:'/partial.wav'}),'audio.stopRecord':()=>{h.ui.event('audio.recording',{recording:false,path:'/partial.wav',frames:4800,duration:.1,partial:true,error:'Source clock gap'});throw new Error('Stop interrupted');}});
 await h.mount();const before=copy(h.state.project);await h.startStop();await h.startStop();assert.equal(h.state.project.tracks.length,before.tracks.length+1);assert.deepEqual(copy(h.state.project.instruments.slice(0,before.instruments.length)),before.instruments);assert.equal(h.state.view,'instrument');assert.equal(h.calls.filter(c=>c.command==='audio.stopRecord').length,1);
});
test('partial decode failure retains all metadata and retries load without another stop',async()=>{
 let h,attempts=0;h=harness({'audio.record':()=>({recording:true,path:'/partial.wav'}),'audio.stopRecord':()=>{h.ui.event('audio.recording',{recording:false,path:'/partial.wav',frames:4800,duration:.1,partial:true,error:'Source clock gap'});throw new Error('Stop interrupted');},'sample.load':()=>{if(++attempts===1)throw new Error('Decode temporarily failed');return {path:'/partial.wav',duration:.1};}});
 await h.mount();await h.startStop();await h.startStop();assert.equal(h.ui.lastRecording.frames,4800);assert.equal(h.ui.lastRecording.duration,.1);assert.equal(h.ui.lastRecording.error,'Source clock gap');assert.equal(h.ui.lastRecording.partial,true);assert.match(h.get('record-status').textContent,/Decode temporarily failed/);await h.startStop();assert.equal(attempts,2);assert.equal(h.calls.filter(c=>c.command==='audio.stopRecord').length,1);assert.equal(h.state.view,'instrument');
});
test('zero-frame final event preserves file but offers retry rather than loading an empty sample',async()=>{
 let h;h=harness({'audio.record':()=>({recording:true,path:'/empty.wav'}),'audio.stopRecord':()=>{h.ui.event('audio.recording',{recording:false,path:'/empty.wav',frames:0,duration:0,partial:true,error:'Clock gap'});throw new Error('Stop interrupted');}});await h.mount();const before=copy(h.state.project);await h.startStop();await h.startStop();assert.deepEqual(copy(h.state.project),before);assert.equal(h.ui.lastRecording.frames,0);assert.match(h.get('record-status').textContent,/No audio captured/);assert.match(h.get('record-start').textContent,/Retry recording/);assert.equal(h.get('record-saved-path').value,'/empty.wav');assert(!h.calls.some(c=>c.command==='sample.load'));await h.startStop();assert(h.ui.isRecording);assert.equal(h.calls.filter(c=>c.command==='audio.record').length,2);
});
test('late final event from another capture cannot stop or replace the current take',async()=>{
 const h=harness();await h.mount();await h.startStop();h.ui.event('audio.recording',{recording:false,path:'/previous.wav',frames:100,duration:1,partial:true});assert(h.ui.isRecording);assert.notEqual(h.ui.lastRecording?.path,'/previous.wav');await h.startStop();assert.equal(h.state.view,'instrument');
});
(async()=>{for(const {name,fn}of tests){await fn();console.log('PASS '+name);}console.log(`Inline recording: ${tests.length} groups passed.`);})().catch(error=>{console.error(error);process.exitCode=1;});
