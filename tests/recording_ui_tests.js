'use strict';
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const { routingTable } = require('./helpers/project');
const script = web.source('recording.js');
const routingScript = web.source('routing-project.js');
const installEngineSyncBoundary = require('./helpers/engine_sync_boundary');
const copy = value => JSON.parse(JSON.stringify(value));
const wait = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
function configureAudio(audio, payload) {
  for (const key of ['inputDevice', 'inputChannels', 'monitorVolume']) if (key in payload) audio[key] = copy(payload[key]);
  if ('inputMeter' in payload) audio.inputMetering = !!payload.inputMeter;
  if ('monitor' in payload) audio.monitoring = !!payload.monitor;
  return copy(audio);
}
function harness(overrides = {}) {
  const nodes = new Map(), calls = [], edits = [], toasts = [];
  const audio = { inputs: [{ id: 22, uid: 'interface-stable-uid', name: 'Interface', inputChannels: 8 }, { id: 33, uid: 'second-stable-uid', name: 'Second input', inputChannels: 2 }], inputDevice: 22, inputChannels: [2, 3], inputMetering: false, monitoring: false, monitorVolume: .6 };
  const state = { native: true, modal: null, view: 'instrument', track: 0, instrument: 0, sequence: 0, row: 8, playing: false, keyboardPlay: true, audio: copy(audio), samples: {}, project: { name: 'Session', bpm: 120, activePattern: 4, sequence: [4], tracks: [{ id: 0, routingID: 'midi-track', name: 'Hardware', instrument: 0, volume: .8, pan: -.2, mute: false, solo: false, delaySend: .25, reverbSend: .4 }], instruments: [{ id: 0, name: 'Synth', rootNote: 60, zones: [{ samplePath: '/old.wav' }], slices: [.2, .6], slicePoints: [.2, .6], mode: 'slice' }], patterns: [{ id: 4, length: 4, steps: [[{ note: 60, instrument: 0 }, null, null, null]], automation: [{ track: 0, target: 'volume', points: [{ row: 0, value: .8 }] }] }, { id: 5, length: 8, steps: [Array(8).fill(null)], automation: [] }], audioPlugins: [], audioClips: [], routing: routingTable() } };
  let ui, nextID = 0;
  const document = { getElementById: id => nodes.get(id) || null, activeElement: null };
  const trimStart = { id: 'param-value-start', focus() { document.activeElement = this; } };
  const defaults = { 'audio.devices': () => copy(audio), 'audio.configure': payload => configureAudio(audio, payload), 'audio.record': () => ({ recording: true, path: '/recordings/Synth-take-uuid.wav' }), 'audio.stopRecord': () => ({ recording: false, path: '/recordings/Synth-take-uuid.wav', duration: 2, frames: 96000 }), 'sample.load': payload => ({ path: payload.path, duration: 2, name: 'Native basename' }), 'audio.clipInfo': () => ({ duration: 2 }), 'transport.seek': () => ({}), 'transport.play': () => ({}), 'transport.stop': () => ({}) };
  const ST = { state, escapeHTML: value => String(value).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'), uid: () => `recording-id-${++nextID}`, songBeat: () => 16, normalize: p => p, stopHeld() { calls.push({ command: 'stopHeld' }); }, captureNativeEdits: async () => true, syncEngine: async () => { calls.push({ command: 'engine.sync' }); }, refresh() { if (!state.modal) nodes.set(trimStart.id, trimStart); }, toast(message, kind) { toasts.push({ message, kind }); }, mutate(label, fn) { edits.push({ label, before: copy(state.project) }); fn(state.project); }, request(command, payload = {}) { calls.push({ command, payload: copy(payload) }); return Promise.resolve((overrides[command] || defaults[command] || (() => ({})))(payload)); }, openModal(title, body, footer, options) {
    state.modal = options.kind; nodes.clear();
    for (const match of (body + footer).matchAll(/<([a-z][a-z0-9]*)\b([^>]*)>/gi)) {
      const attrs = {}; for (const value of match[2].matchAll(/([\w-]+)(?:="([^"]*)")?/g)) attrs[value[1]] = value[2] || '';
      if (!attrs.id) continue;
      nodes.set(attrs.id, { id: attrs.id, value: attrs.value || '', checked: 'checked' in attrs, disabled: 'disabled' in attrs, hidden: 'hidden' in attrs, style: {}, textContent: '', classList: { toggle() {} }, listeners: {}, addEventListener(type, fn) { (this.listeners[type] ||= []).push(fn); }, focus() { document.activeElement = this; }, select() {}, fire(type) { let result;for(const fn of this.listeners[type]||[]){const value=fn({target:this});if(value!==undefined)result=value;}return result; } });
    }
    for (const match of body.matchAll(/<select\b[^>]*id="([^"]+)"[^>]*>([\s\S]*?)<\/select>/g)) {
      const options = [...match[2].matchAll(/<option\b([^>]*)>([\s\S]*?)<\/option>/g)];
      const selected = options.find(option => /\bselected\b/.test(option[1])) || options[0];
      if (selected) nodes.get(match[1]).value = /\bvalue="([^"]*)"/.exec(selected[1])?.[1] || selected[2];
    }
  }, closeModal() { if (ui.isBusy || ui.isRecording) return; state.modal = null; ui.closed(); } };
  const syncs = installEngineSyncBoundary(ST);
  const context = vm.createContext({ window: { ST, TRACK_SANITY_LIMIT: 1024 }, document, setTimeout: fn => fn() }); web.load(context, 'routing-project.js', { text: routingScript }); for (const name of ['clip-takes-project.js', 'clip-takes.js']) web.load(context, name); web.load(context, 'recording.js', { text: script }); ui = context.window.STRecording;
  const get = id => { const node = nodes.get(id); assert(node, `Missing ${id}`); return node; };
  const startStop = () => get('record-start').fire('click');
  return { nodes, ui, context, takes: context.window.STClipTakes, state, audio, calls, syncs, edits, toasts, get, startStop, document, ST, async open(options) { return ui.show(options); }, change(id, value) { get(id).value = value; return get(id).fire('change'); } };
}
function hardware(overrides = {}) {
  const h = harness(overrides);
  Object.assign(h.state.project.instruments[0], { type: 'midi', rootNote: 66, midiOutput: 'hardware-output', midiInput: 'hardware-input', midiChannel: 7, midiInputChannel: 3 });
  return h;
}
const tests = [], test = (name, fn) => tests.push({ name, fn });
test('default sample recording preserves every existing instrument and track from every workspace',async()=>{
 for(const view of ['pattern','instrument','song','recording']){
  const h=harness();h.state.view=view;const tracks=copy(h.state.project.tracks),instruments=copy(h.state.project.instruments);
  await h.open();assert.equal(h.get('record-destination').value,'newTrack');await h.startStop();await h.startStop();
  assert.deepEqual(copy(h.state.project.tracks.slice(0,tracks.length)),tracks);assert.deepEqual(copy(h.state.project.instruments.slice(0,instruments.length)),instruments);assert.equal(h.state.project.tracks.length,tracks.length+1);
 }
});
test('recording source selected on the main page initializes master resampling without input configuration',async()=>{
 const h=harness();await h.open({source:'master'});assert.equal(h.get('record-source').value,'master');await h.startStop();assert.equal(h.calls.find(c=>c.command==='audio.record').payload.source,'master');assert(!h.calls.some(c=>c.command==='audio.configure'));
});
test('sampling a live input lane restores its source and creates a separate playable sampler track', async () => {
  const h=harness(), source=h.state.project.tracks[0];
  source.audioInput={enabled:true,deviceUID:'interface-stable-uid',channels:[4,5],gain:.7};
  h.state.project.audioPlugins=[{track:0,kind:'instrument',instanceID:'source-instrument',slot:0}];
  const before=copy(source), sourceInstrument=copy(h.state.project.instruments[0]);
  await h.open(); assert.equal(h.get('record-destination').value,'newTrack');
  assert.equal(h.get('record-input-device').value,'22'); assert.equal(h.get('record-input-left').value,'4'); assert.equal(h.get('record-input-right').value,'5');
  await h.startStop(); await h.startStop();
  assert.equal(h.state.modal,null);assert.deepEqual(copy(h.state.project.tracks[0]),before);assert.deepEqual(copy(h.state.project.instruments[0]),sourceInstrument);
  assert.equal(h.state.project.tracks.length,2);assert.equal(h.state.track,1);assert.equal(h.state.project.tracks[1].audioInput,undefined);
  assert.equal(h.state.project.instruments[h.state.instrument].type,'sample');assert.equal(h.state.project.instruments[h.state.instrument].samplePath,'/recordings/Synth-take-uuid.wav');
  assert(!h.calls.some(c=>c.command==='plugin.remove'));assert.equal(h.state.project.audioPlugins[0].instanceID,'source-instrument');
  assert.deepEqual(copy(h.audio.inputChannels),[2,3]);
  for(const pattern of h.state.project.patterns)assert.deepEqual(copy(pattern.steps[1]),Array(pattern.length).fill(null));
});
test('missing live input UID blocks capture instead of silently recording a different source', async () => {
  const h=harness();h.state.project.tracks[0].audioInput={enabled:true,deviceUID:'missing-interface',channels:[4,5],gain:1};
  await h.open();await h.startStop();assert.match(h.get('record-status').textContent,/unavailable/);assert(!h.calls.some(c=>c.command==='audio.record'));
  h.change('record-input-device','33');h.get('record-input-right').value='mono';await h.startStop();assert(h.ui.isRecording);
});
test('new sampler track capacity is checked before recording and again before saved attachment', async () => {
  for(const stage of ['before','after']){
    const h=harness();await h.open();h.change('record-destination','newTrack');
    if(stage==='after')await h.startStop();
    while(h.state.project.tracks.length<1024)h.state.project.tracks.push({id:h.state.project.tracks.length});
    await h.startStop();assert.match(h.get('record-status').textContent,/1024 tracks/);
    assert.equal(h.calls.filter(c=>c.command==='audio.record').length,stage==='after'?1:0);
    if(stage==='after')assert.equal(h.ui.lastRecording.path,'/recordings/Synth-take-uuid.wav');
    assert.equal(h.state.project.instruments.length,1);
  }
});
test('sampler recording removes only sidechains belonging to the replaced instrument plug-in', async () => {
  for (const destination of ['current']) {
    const h = harness();
    h.state.project.audioPlugins = [{track:0,kind:'instrument',instanceID:'old-instrument',slot:0},{track:0,kind:'effect',instanceID:'keep-effect',slot:1}];
    h.state.project.routing = {version:1,outputs:{'track:midi-track':'master'},sidechains:[{destination:{instanceID:'old-instrument'}},{destination:{instanceID:'keep-effect'}}]};
    await h.open(); h.change('record-destination',destination); await h.startStop(); await h.startStop();
    assert.equal(h.state.modal,null);
    assert.deepEqual(copy(h.state.project.audioPlugins).map(p=>p.instanceID),['keep-effect']);
    assert.deepEqual(copy(h.state.project.routing.sidechains),[{destination:{instanceID:'keep-effect'}}]);
    assert.equal(h.state.project.instruments[h.state.instrument].samplePath,'/recordings/Synth-take-uuid.wav');
  }
});
test('refused and rolled-back recording attachments preserve the saved capture and never claim success',async()=>{
 for(const destination of ['clip','newTrack','current'])for(const refusal of ['blocked','rollback']){
  const h=harness();await h.open();h.change('record-destination',destination);await h.startStop();
  const before=copy(h.state.project);h.ST.mutate=(label,fn)=>{if(refusal==='rollback'){fn(h.state.project);h.state.project=copy(before);}};
  await h.startStop();assert.deepEqual(copy(h.state.project),before,destination+' '+refusal);
  assert.equal(h.state.modal,'recording');assert.equal(h.ui.lastRecording.path,'/recordings/Synth-take-uuid.wav');
  assert.equal(h.get('record-saved').hidden,false);assert.match(h.get('record-status').textContent,/not applied/);assert(!h.toasts.some(t=>t.kind==='success'));
 }
});
test('a refused sampler attach restores canonical native state and retries the same file without recapturing',async()=>{
 for(const destination of ['clip','newTrack','current']){
  const h=harness();await h.open();h.change('record-destination',destination);await h.startStop();
  const before=copy(h.state.project),mutate=h.ST.mutate;h.ST.mutate=()=>{};await h.startStop();
  const restores=h.calls.filter(c=>c.command==='engine.project'&&c.payload.replace===true);
  assert.equal(restores.length,destination==='clip'?0:1);if(restores.length)assert.deepEqual(restores[0].payload.project,before);
  h.ST.mutate=mutate;await h.startStop();assert.equal(h.state.modal,null);assert.equal(h.ui.lastRecording,null);
  assert.equal(h.calls.filter(c=>c.command==='audio.record').length,1);assert.equal(h.calls.filter(c=>c.command==='audio.stopRecord').length,1);assert.equal(h.edits.length,1);
 }
});
test('authorized footer and native modal close keep restoration busy through the page quit gate without stranding the dialog',async()=>{
  for(const close of ['footer','native']){
    const gate=wait();let configurations=0;
    const h=harness({'audio.configure':async payload=>{if(++configurations===2)await gate.promise;return payload;}});
    const app=web.loadAppWith(vm.createContext({window:Object.create(h.context.window)}),{state:h.state,settingsApplyPending:null,audioCaptureActive:()=>h.ui.isBusy||h.ui.isRecording,toast(){},$:()=>({innerHTML:'',focus(){}})});
    h.ST.closeModal=()=>app.closeModal();
    h.context.window.__supertracker={getProject:()=>h.state.project};
    await h.open();await h.get('record-meter-enable').fire('click');
    if(close==='footer')await h.get('record-close').fire('click');else h.ST.closeModal();
    assert.equal(h.state.modal,null,'An authorized close must dismiss the dialog');assert.equal(configurations,2);assert(h.ui.isBusy,'Pending input restoration still owns the audio-settings gate');
    assert.match(app.quitBlocker(),/audio recording/,'Quit must wait for the accepted input restoration');
    gate.resolve();for(let i=0;i<12;i++)await Promise.resolve();assert(!h.ui.isBusy);assert.equal(app.quitBlocker(),null);
    await h.open();assert.equal(h.state.modal,'recording');
  }
});
test('recording name receives initial focus and stays editable while device discovery is pending', async () => {
  const devices = wait(), h = harness({ 'audio.devices': () => devices.promise });
  const opening = h.open();
  assert.equal(h.get('record-name').disabled, false);
  assert.equal(h.document.activeElement.id, 'record-name');
  assert.equal(h.get('record-start').disabled, true);
  h.get('record-name').value = 'Hardware note named while loading';
  assert(h.calls.every(call => call.command === 'audio.devices'));
  devices.resolve(h.audio); await opening;
  assert.equal(h.get('record-name').value, 'Hardware note named while loading');
  assert.equal(h.document.activeElement.id, 'record-name');
  assert.equal(h.get('record-start').disabled, false);
});
test('strict project preparation failures cannot start recording, input configuration or playback', async () => {
  for (const source of ['input','master']) for (const failure of ['native','canceled']) {
    const h = harness({ 'engine.project': () => { throw new Error('Recording graph rejected'); } }); await h.open(); h.change('record-source', source);
    if (failure === 'canceled') h.ST.syncEngineProject = async () => false;
    await h.startStop(); assert(!h.ui.isBusy && !h.ui.isRecording);
    assert(!h.calls.some(c => ['audio.record','audio.configure','transport.seek','transport.play'].includes(c.command)));
    assert.match(h.get('record-status').textContent, /rejected|canceled/);
    await h.get('record-playback').fire('click'); assert(!h.calls.some(c => c.command === 'transport.play'));
    h.state.playing = true; await h.get('record-playback').fire('click'); assert(h.calls.some(c => c.command === 'transport.stop')); assert(!h.state.playing);
  }
});
test('recording waits for settled project and native acknowledgment and cancels a changed destination', async () => {
  for (const changed of [false,true]) {
    const edits = wait(), native = wait(), h = harness({ 'engine.project': () => native.promise }); h.ST.beforeEngineSync = () => edits.promise;
    await h.open(); const starting = h.startStop(); await new Promise(resolve => setImmediate(resolve));
    assert(!h.calls.some(c => c.command === 'engine.project')); h.state.project.audioPlugins = [{state:'settled mapped edit'}];
    if (changed) h.state.project.tracks[0] = {...h.state.project.tracks[0]};
    edits.resolve(); await new Promise(resolve => setImmediate(resolve));
    assert(!h.calls.some(c => c.command === 'audio.record'));
    if (!changed) { const sync = h.calls.find(c => c.command === 'engine.project'); assert.equal(sync.payload.project.audioPlugins[0].state,'settled mapped edit'); assert.equal(sync.payload.awaitPlugins,true); }
    else assert(!h.calls.some(c => c.command === 'engine.project'));
    native.resolve({}); await starting; assert.equal(h.ui.isRecording,!changed);
  }
});
test('failed attachment restoration preserves the capture path and reports its strict native failure', async () => {
  let syncs = 0, h;
  h = harness({ 'engine.project': () => { if (++syncs > 1) throw Error('Restore graph rejected'); return {}; }, 'plugin.remove': () => { h.state.project = copy(h.state.project); h.state.project.name = 'Replacement project'; return {}; } });
  h.state.project.audioPlugins = [{track:0,kind:'instrument',slot:0,state:'preserved patch'}];
  await h.open();h.change('record-destination','current'); await h.startStop(); await h.startStop();
  assert.equal(h.edits.length,0); assert(h.ui.lastRecording.path); assert(!h.ui.isRecording && !h.ui.isBusy);
  const restored = h.calls.filter(c => c.command === 'engine.project').at(-1);
  assert.equal(restored.payload.project.name,'Replacement project'); assert.equal(restored.payload.replace,true);
  assert.match(h.get('record-status').textContent,/destination track changed.*Could not restore.*Restore graph rejected/);
  assert.equal(h.state.project.audioPlugins[0].state,'preserved patch');
});
test('opening never requests microphone access or starts capture; input check is explicit', async () => {
  const h = harness(); await h.open(); assert.deepEqual(h.calls.map(c => c.command), ['audio.devices']); assert.equal(h.document.activeElement.id, 'record-name');
  await h.get('record-meter-enable').fire('click'); const config = h.calls.find(c => c.command === 'audio.configure'); assert.equal(config.payload.inputMeter, true); assert.deepEqual(config.payload.inputChannels, [2, 3]); assert.equal(config.payload.monitor, false);
  h.ui.event('status', { inputPeakLeft: .5, inputPeakRight: 1.2 }); assert.equal(h.get('record-meter-left').style.transform, 'scaleX(0.5)'); assert.equal(h.get('record-level-left').textContent, '-6.0 dB');
  h.get('record-close').fire('click'); for (let i = 0; i < 5; i++) await Promise.resolve(); assert.equal(h.calls.filter(c => c.command === 'audio.configure').at(-1).payload.inputMeter, false);
});
test('named mono capture produces one new sampler with the chosen root and no inherited zones', async () => {
  const h = harness(); await h.open(); h.get('record-name').value = 'Juno F sharp'; h.get('record-root-note').value = '66'; h.get('record-input-left').value = '4'; h.get('record-input-right').value = 'mono';
  await h.startStop(); assert(h.ui.isRecording); assert.equal(h.get('record-source').disabled, true); assert.equal(h.get('record-keyboard').checked, true);
  const config = h.calls.find(c => c.command === 'audio.configure'); assert.deepEqual(config.payload.inputChannels, [4]);
  assert.equal(h.calls.find(c => c.command === 'audio.record').payload.name, 'Juno F sharp');
  h.ui.event('status', { recording: true, recordingDuration: 1.25, inputPeakLeft: .2, inputPeakRight: .2 }); assert.equal(h.get('record-duration').textContent, '1.3 s');
  await h.startStop(); assert(!h.ui.isRecording && !h.ui.isBusy); assert.equal(h.state.modal, null); assert.equal(h.state.project.instruments.length, 2);
  const sample = h.state.project.instruments[1]; assert.equal(sample.name, 'Juno F sharp'); assert.equal(sample.rootNote, 66); assert.equal(sample.mode, 'oneshot'); assert.deepEqual(copy(sample.zones), []); assert.deepEqual(copy(sample.slices), []); assert.deepEqual(copy(sample.slicePoints), []); assert.equal(h.edits.length, 1);
});
test('replacing current sampler clears stale zones and slices but preserves unrelated controls', async () => {
  const h = harness(); h.state.project.instruments[0].cutoff = 1234; await h.open(); h.change('record-destination', 'current'); await h.startStop(); await h.startStop();
  const sample = h.state.project.instruments[0]; assert.equal(h.state.project.instruments.length, 1); assert.deepEqual(copy(sample.zones), []); assert.deepEqual(copy(sample.slices), []); assert.deepEqual(copy(sample.slicePoints), []); assert.equal(sample.cutoff, 1234); assert.equal(sample.mode, 'oneshot');
});
test('empty name errors happen before permission or capture; 64 instruments is no longer a limit', async () => {
  const h = harness(); await h.open(); h.state.project.instruments = Array.from({ length: 64 }, (_, id) => ({ id })); await h.startStop(); assert(h.calls.some(c => c.command === 'audio.record'), 'no 64-instrument cap');
  const b = harness(); await b.open(); b.get('record-name').value = ''; await b.startStop(); assert(!b.calls.some(c => c.command === 'audio.record')); assert(b.get('record-status').textContent.includes('name'));
});
test('permission denial preserves project and leaves recording retry available', async () => {
  const h = harness({ 'audio.configure': () => Promise.reject(new Error('Microphone permission denied')) }); await h.open(); await h.startStop(); assert(!h.ui.isRecording && !h.ui.isBusy); assert.equal(h.edits.length, 0); assert(!h.calls.some(c => c.command === 'audio.record')); assert(h.get('record-status').textContent.includes('denied'));
});
test('permission wait blocks duplicate starts and close until native result arrives', async () => {
  const pending = wait(); const h = harness({ 'audio.record': () => pending.promise }); await h.open(); const first = h.startStop(); await new Promise(resolve => setImmediate(resolve));
  assert(h.ui.isBusy); await h.startStop(); h.ST.closeModal(); assert.equal(h.state.modal, 'recording'); assert.equal(h.calls.filter(c => c.command === 'audio.record').length, 1);
  pending.resolve({ recording: true, path: '/capture.wav' }); await first; assert(h.ui.isRecording); h.ST.closeModal(); assert.equal(h.state.modal, 'recording');
});
test('failed decode retains capture path and retries loading without a second recording or empty instrument', async () => {
  let attempts = 0; const h = harness({ 'sample.load': payload => ++attempts === 1 ? Promise.reject(new Error('Decoder unavailable')) : { path: payload.path } }); await h.open(); await h.startStop(); await h.startStop();
  assert.equal(h.state.project.instruments.length, 1); assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.match(h.get('record-start').textContent, /^Load saved recording/); assert.equal(h.get('record-saved').hidden, false);
  await h.startStop(); assert.equal(h.calls.filter(c => c.command === 'audio.record').length, 1); assert.equal(h.calls.filter(c => c.command === 'audio.stopRecord').length, 1); assert.equal(h.state.project.instruments.length, 2);
});
test('a project switch during decode cannot attach the recording to the new project', async () => {
  const pending = wait(); const h = harness({ 'sample.load': () => pending.promise }); await h.open(); await h.startStop(); const finish = h.startStop(); for (let i = 0; i < 6; i++) await Promise.resolve();
  const replacement = { tracks: [{}], instruments: [{ name: 'Unrelated' }], audioClips: [] }; h.state.project = replacement; pending.resolve({ path: '/capture.wav' }); await finish;
  assert.equal(h.state.project.instruments[0].name, 'Unrelated'); assert.equal(h.edits.length, 0); assert(h.ui.lastRecording.path); assert(!h.ui.isRecording);
});
test('master capture avoids input permission and retains playback seek/play controls', async () => {
  const h = harness(); await h.open(); h.change('record-source', 'master'); await h.startStop();
  assert(!h.calls.some(c => c.command === 'audio.configure')); assert(h.calls.some(c => c.command === 'transport.seek' && c.payload.pattern === 4 && c.payload.row === 8)); assert(h.calls.some(c => c.command === 'transport.play' && c.payload.mode === 'pattern')); assert(h.state.playing);
  h.ui.event('status', { peakL: .25, peakR: .1 }); assert.equal(h.get('record-level-left').textContent, '-12.0 dB');
  await h.get('record-playback').fire('click'); assert(h.calls.some(c => c.command === 'transport.stop')); assert(!h.state.playing);
});
test('timeline destination uses captured track/beat, duration and user name', async () => {
  const h = harness(); h.state.view = 'song'; await h.open(); h.change('record-destination','clip'); h.get('record-name').value = 'Hardware phrase'; await h.startStop(); await h.startStop();
  const clip = h.state.project.audioClips[0]; assert.equal(clip.name, 'Hardware phrase'); assert.equal(clip.track, 0); assert.equal(clip.startBeat, 16); assert.equal(clip.durationBeats, 4); assert.equal(h.state.view, 'song'); assert.equal(h.state.project.instruments.length, 1);
});
test('hardware sampling requires a MIDI instrument and opens without requesting input access', async () => {
  const sample = harness(); assert.equal(await sample.open({ hardwareSample: true }), false); assert.equal(sample.state.modal, null);
  assert(!sample.calls.some(call => ['audio.configure', 'audio.record'].includes(call.command)));
  const h = hardware(); h.state.view = 'song'; h.audio.monitoring = true; h.state.audio.monitoring = true;
  await h.open({ hardwareSample: true });
  assert.deepEqual(h.calls.map(call => call.command), ['audio.devices']); assert.equal(h.edits.length, 0);
  assert.equal(h.get('record-source').value, 'input'); assert(h.get('record-source').disabled);
  assert.equal(h.get('record-destination').value, 'new'); assert(h.get('record-destination').disabled);
  assert.equal(h.get('record-root-note').value, '66'); assert.equal(h.get('record-monitor').checked, false);
  assert.match(h.get('record-input-summary').textContent, /current (Studio|global) input/i);
  h.change('record-input-device', '33');
  assert.equal(h.get('record-input-left').value, '0'); assert.equal(h.get('record-input-right').value, '1');
  assert(!h.calls.some(call => ['audio.configure', 'audio.record'].includes(call.command))); assert.equal(h.edits.length, 0);
});
test('saved hardware input follows its UID after numeric device IDs change', async () => {
  for (const channels of [[4], [4, 5]]) {
    const h = hardware(); h.state.project.instruments[0].recordingInput = { deviceUID: 'interface-stable-uid', deviceName: 'Old interface name', channels };
    h.audio.inputs[0] = { ...h.audio.inputs[0], id: 77, name: 'Interface renamed' }; h.audio.inputDevice = 33; h.audio.inputChannels = [0, 1];
    await h.open({ hardwareSample: true });
    assert.equal(h.get('record-input-device').value, '77'); assert.equal(h.get('record-input-left').value, '4'); assert.equal(h.get('record-input-right').value, channels.length === 1 ? 'mono' : '5');
    assert(!h.calls.some(call => call.command === 'audio.configure'));
    await h.startStop();
    const config = h.calls.find(call => call.command === 'audio.configure'); assert.equal(config.payload.inputDevice, 77); assert.deepEqual(config.payload.inputChannels, channels);
    assert.deepEqual(copy(h.state.project.instruments[0].recordingInput), { deviceUID: 'interface-stable-uid', deviceName: 'Interface renamed', channels });
  }
});
test('a missing saved hardware device blocks capture until the user explicitly chooses an input', async () => {
  const h = hardware(); h.state.project.instruments[0].recordingInput = { deviceUID: 'disconnected-uid', deviceName: 'Disconnected synth input', channels: [6, 7] };
  await h.open({ hardwareSample: true });
  assert.match(h.get('record-input-summary').textContent, /missing|unavailable|not (connected|available)|choose/i);
  await h.startStop();
  assert(!h.calls.some(call => ['audio.record', 'audio.configure'].includes(call.command))); assert.equal(h.edits.length, 0);
  h.change('record-input-device', '33'); assert.equal(h.get('record-input-left').value, '0'); assert.equal(h.get('record-input-right').value, '1');
  await h.startStop(); assert(h.ui.isRecording);
  assert.equal(h.calls.find(call => call.command === 'audio.configure').payload.inputDevice, 33);
  assert.deepEqual(copy(h.state.project.instruments[0].recordingInput), { deviceUID: 'second-stable-uid', deviceName: 'Second input', channels: [0, 1] });
});
test('legacy hardware preferences use a clearly named current input and device changes reset an incompatible map', async () => {
  const h = hardware(); h.state.project.instruments[0].recordingInput = { deviceName: 'Legacy name without UID', channels: [6, 7] };
  await h.open({ hardwareSample: true });
  assert.match(h.get('record-input-summary').textContent, /current (Studio|global) input/i); assert.equal(h.get('record-input-device').value, '22');
  assert.equal(h.get('record-input-left').value, '2'); assert.equal(h.get('record-input-right').value, '3');
  h.get('record-input-left').value = '6'; h.get('record-input-right').value = '7'; h.change('record-input-device', '33');
  assert.equal(h.get('record-input-left').value, '0'); assert.equal(h.get('record-input-right').value, '1');
  assert(!h.calls.some(call => call.command === 'audio.configure')); assert.equal(h.edits.length, 0);
});
test('saved channels that are unavailable are shown and rejected until the user chooses a valid map', async () => {
  const h = hardware(); h.state.project.instruments[0].recordingInput = { deviceUID: 'second-stable-uid', deviceName: 'Second input', channels: [6, 7] };
  await h.open({ hardwareSample: true });
  assert.equal(h.get('record-input-device').value, '33'); assert.equal(h.get('record-input-left').value, '6'); assert.equal(h.get('record-input-right').value, '7');
  assert.match(h.get('record-input-left').innerHTML, /Unavailable input 7/); assert.match(h.get('record-input-right').innerHTML, /Unavailable input 8/);
  await h.get('record-meter-enable').fire('click'); await h.startStop();
  assert(!h.calls.some(call => ['audio.configure', 'audio.record'].includes(call.command))); assert.equal(h.edits.length, 0); assert.match(h.get('record-status').textContent, /channels.*unavailable/i);
  h.change('record-input-device', '33'); await h.startStop(); assert(h.ui.isRecording);
  assert.deepEqual(h.calls.find(call => call.command === 'audio.configure').payload.inputChannels, [0, 1]);
});
test('hardware input and root are remembered in a separate undoable edit before capture configuration', async () => {
  let h, sawRemembered = false;
  h = hardware({ 'audio.configure': payload => {
    if (!sawRemembered) {
      assert.equal(h.edits.at(-1).label, 'Remember hardware input'); assert.equal(h.state.project.instruments[0].rootNote, 69);
      assert.deepEqual(copy(h.state.project.instruments[0].recordingInput), { deviceUID: 'second-stable-uid', deviceName: 'Second input', channels: [1] });
      assert(!h.calls.some(call => call.command === 'audio.record')); sawRemembered = true;
    }
    return { ...h.audio, inputDevice: payload.inputDevice, inputChannels: payload.inputChannels };
  } });
  const before = copy(h.state.project.instruments[0]); await h.open({ hardwareSample: true });
  h.change('record-input-device', '33'); h.get('record-input-left').value = '1'; h.get('record-input-right').value = 'mono'; h.get('record-root-note').value = '69'; h.get('record-monitor').checked = true;
  assert.equal(h.edits.length, 0); await h.startStop(); assert(sawRemembered);
  const remembered = h.edits.find(edit => edit.label === 'Remember hardware input'); assert.deepEqual(remembered.before.instruments[0], before);
  assert.equal(h.state.project.instruments[0].type, 'midi'); assert.equal(h.state.project.instruments[0].midiChannel, 7);
  assert.equal(Object.hasOwn(h.state.project.instruments[0].recordingInput, 'monitor'), false);
  assert.equal(Object.hasOwn(h.state.project.instruments[0].recordingInput, 'inputDevice'), false);
});
test('hardware capture adds a fresh sampler track and empty pattern columns while preserving the MIDI source and routes', async () => {
  const h = hardware(), project = h.state.project, originalTrack = project.tracks[0], originalInstrument = project.instruments[0];
  project.routing = { version: 1, mixModel: 'standard', outputs: { 'track:midi-track': 'bus:group' }, buses: [{ id: 'group', type: 'group' }], sends: [{ id: 'original-send', source: 'track:midi-track', destination: 'bus:return', gain: .3 }], sidechains: [] };
  project.audioPlugins = [{ track: 0, kind: 'effect', slot: 0, state: 'source effect preserved' }];
  const before = copy(project); await h.open({ hardwareSample: true }); h.get('record-name').value = 'Hardware C sharp';
  await h.startStop(); const rememberedSource = copy(originalInstrument); await h.startStop();
  assert.equal(h.state.modal, null); assert.equal(project.tracks.length, 2); assert.equal(project.instruments.length, 2);
  assert.equal(project.tracks[0], originalTrack); assert.deepEqual(copy(originalTrack), before.tracks[0]);
  assert.equal(project.instruments[0], originalInstrument); assert.deepEqual(copy(originalInstrument), rememberedSource); assert.equal(originalInstrument.type, 'midi');
  assert.deepEqual(copy(project.audioPlugins), before.audioPlugins); assert(!h.calls.some(call => call.command === 'plugin.remove'));
  const added = project.tracks[1], sample = project.instruments[1]; assert.equal(added.instrument, 1); assert.notEqual(added.routingID, originalTrack.routingID);
  assert.equal(added.volume, 1); assert.equal(added.pan, 0); assert.equal(added.delaySend, 0); assert.equal(added.reverbSend, 0);
  assert.equal(project.routing.outputs[`track:${added.routingID}`], 'master'); assert.equal(project.routing.outputs['track:midi-track'], 'bus:group'); assert.deepEqual(copy(project.routing.sends), before.routing.sends);
  for (let index = 0; index < project.patterns.length; index++) {
    assert.deepEqual(copy(project.patterns[index].steps[0]), before.patterns[index].steps[0]);
    assert.deepEqual(copy(project.patterns[index].steps[1]), Array(project.patterns[index].length).fill(null));
    assert.deepEqual(copy(project.patterns[index].automation), before.patterns[index].automation);
  }
  assert.equal(sample.type, 'sample'); assert.equal(sample.rootNote, 66); assert.equal(sample.name, 'Hardware C sharp'); assert.equal(sample.samplePath, '/recordings/Synth-take-uuid.wav');
  assert.deepEqual(copy(sample.zones), []); assert.deepEqual(copy(sample.slices), []); assert.deepEqual(copy(sample.slicePoints), []);
  assert.equal(h.edits.length, 2); assert.equal(h.edits[0].label, 'Remember hardware input');
  assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1); assert.equal(h.state.view, 'instrument'); assert.equal(h.document.activeElement.id, 'param-value-start');
});
test('hardware capture toggle starts and stops the existing dialog without reopening it', async () => {
  const h = hardware(); await h.open({ hardwareSample: true });
  await h.ui.toggleCapture(); assert(h.ui.isRecording); await h.ui.toggleCapture();
  assert.equal(h.state.modal, null); assert.equal(h.state.project.tracks.length, 2);
  assert.equal(h.calls.filter(call => call.command === 'audio.devices').length, 1);
  assert.equal(h.calls.filter(call => call.command === 'audio.record').length, 1); assert.equal(h.calls.filter(call => call.command === 'audio.stopRecord').length, 1);
});
test('hardware setup never silently persists an input without a stable device identity', async () => {
  const h = hardware(); delete h.audio.inputs[0].uid;
  await h.open({hardwareSample:true}); await h.startStop();
  assert.match(h.get('record-status').textContent,/persistent device identity/);
  assert.equal(h.edits.length,0); assert(!h.calls.some(call => ['audio.configure','audio.record'].includes(call.command)));
});
test('hardware track capacity is checked before permission or capture (instruments have no cap)', async () => {
  for (const [collection, limit] of [['tracks', 1024]]) {
    const h = hardware(); while (h.state.project[collection].length < limit) h.state.project[collection].push({ id: h.state.project[collection].length });
    await h.open({ hardwareSample: true }); await h.startStop();
    assert(!h.calls.some(call => ['audio.configure', 'audio.record'].includes(call.command))); assert.equal(h.edits.length, 0);
    assert.match(h.get('record-status').textContent, new RegExp(String(limit))); assert(!h.ui.isBusy && !h.ui.isRecording);
  }
});
test('hardware capacity changes during decoding preserve the saved capture and never partially attach it', async () => {
  for (const [collection, limit] of [['tracks', 1024]]) {
    const pending = wait(), h = hardware({ 'sample.load': () => pending.promise }); await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); await h.startStop();
    const finishing = h.startStop(); await new Promise(resolve => setImmediate(resolve)); assert(h.calls.some(call => call.command === 'sample.load'));
    while (h.state.project[collection].length < limit) h.state.project[collection].push({ id: h.state.project[collection].length });
    pending.resolve({ path: '/recordings/Synth-take-uuid.wav' }); await finishing;
    assert.equal(h.state.project[collection].length, limit); assert.equal(h.state.project[collection === 'tracks' ? 'instruments' : 'tracks'].length, 1);
    assert.equal(h.state.project.instruments[0].type, 'midi'); assert.equal(h.state.project.tracks[0].instrument, 0); assert.equal(h.edits.length, 1);
    assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.equal(h.get('record-saved').hidden, false); assert(!h.ui.isBusy && !h.ui.isRecording);
    const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
    assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]);
  }
});
test('capacity filled during hardware capture still stops safely and retains the file before decoding', async () => {
  for (const [collection, limit] of [['tracks', 1024]]) {
    const h = hardware(); await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); await h.startStop(); assert(h.ui.isRecording);
    while (h.state.project[collection].length < limit) h.state.project[collection].push({ id: h.state.project[collection].length });
    await h.ui.toggleCapture();
    assert(!h.ui.isRecording && !h.ui.isBusy); assert.equal(h.calls.filter(call => call.command === 'audio.stopRecord').length, 1);
    assert(!h.calls.some(call => call.command === 'sample.load')); assert.equal(h.edits.length, 1);
    assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.equal(h.get('record-saved-path').value, '/recordings/Synth-take-uuid.wav');
    const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
    assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]);
  }
});
test('changed MIDI source identity or assignment cannot receive a saved hardware take', async () => {
  for (const change of ['instrument', 'type', 'assignment', 'project']) {
    const pending = wait(), h = hardware({ 'sample.load': () => pending.promise }); await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); await h.startStop();
    const finishing = h.startStop(); await new Promise(resolve => setImmediate(resolve));
    if (change === 'instrument') h.state.project.instruments[0] = { ...h.state.project.instruments[0] };
    if (change === 'type') h.state.project.instruments[0].type = 'sample';
    if (change === 'assignment') h.state.project.tracks[0].instrument = 1;
    if (change === 'project') h.state.project = copy(h.state.project);
    pending.resolve({ path: '/recordings/Synth-take-uuid.wav' }); await finishing;
    assert.equal(h.state.project.tracks.length, 1); assert.equal(h.state.project.instruments.length, 1); assert.equal(h.edits.length, 1);
    assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.equal(h.get('record-saved').hidden, false); assert(!h.ui.isRecording);
    const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
    assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]);
  }
});
test('hardware input check and completed capture restore the prior global device, channel map, metering and monitor state', async () => {
  for (const action of ['check-close', 'capture']) {
    const h = hardware(); h.audio.monitoring = true; h.audio.inputMetering = true; h.state.audio = copy(h.audio);
    await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); h.get('record-input-left').value = '1'; h.get('record-input-right').value = 'mono';
    assert.equal(h.get('record-monitor').checked, false);
    if (action === 'check-close') { await h.get('record-meter-enable').fire('click'); await h.get('record-close').fire('click'); }
    else { await h.startStop(); await h.startStop(); }
    for (let i = 0; i < 5; i++) await Promise.resolve();
    const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
    assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]); assert.equal(restore.inputMeter, true); assert.equal(restore.monitor, true); assert.equal(restore.monitorVolume, .6);
    assert.equal(h.state.audio.inputDevice, 22); assert.deepEqual(copy(h.state.audio.inputChannels), [2, 3]);
    if (action === 'check-close') assert.equal(h.edits.length, 0);
  }
});
test('failed hardware decoding retains the take for retry without altering the MIDI track or recording twice', async () => {
  let attempts = 0;
  const h = hardware({ 'sample.load': payload => ++attempts === 1 ? Promise.reject(new Error('Hardware take decoder unavailable')) : { path: payload.path } });
  await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); await h.startStop(); await h.startStop();
  assert.equal(h.state.project.tracks.length, 1); assert.equal(h.state.project.instruments.length, 1); assert.equal(h.state.project.instruments[0].type, 'midi');
  assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.match(h.get('record-start').textContent, /^Load saved recording/);
  const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
  assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]);
  await h.ui.toggleCapture();
  assert.equal(h.calls.filter(call => call.command === 'audio.record').length, 1); assert.equal(h.calls.filter(call => call.command === 'audio.stopRecord').length, 1);
  assert.equal(h.state.project.tracks.length, 2); assert.equal(h.state.project.instruments.length, 2); assert.equal(h.edits.length, 2); assert.equal(h.state.project.instruments[0].type, 'midi');
});
test('closing a failed hardware attachment restores the original input without discarding the capture file', async () => {
  const h = hardware({ 'sample.load': () => { throw new Error('Cannot decode the captured hardware take'); } });
  await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); h.get('record-input-right').value = 'mono';
  await h.startStop(); await h.startStop(); assert.equal(h.get('record-saved-path').value, '/recordings/Synth-take-uuid.wav');
  await h.get('record-close').fire('click'); for (let i = 0; i < 5; i++) await Promise.resolve();
  assert.equal(h.state.modal, null); assert.equal(h.state.project.tracks.length, 1); assert.equal(h.state.project.instruments.length, 1);
  const restore = h.calls.filter(call => call.command === 'audio.configure').at(-1).payload;
  assert.equal(restore.inputDevice, 22); assert.deepEqual(restore.inputChannels, [2, 3]); assert.equal(restore.inputMeter, false); assert.equal(restore.monitor, false);
  assert.equal(h.calls.filter(call => call.command === 'audio.record').length, 1); assert.equal(h.calls.filter(call => call.command === 'audio.stopRecord').length, 1);
  assert(!h.calls.some(call => /delete|remove/i.test(call.command)));
});
test('reopening the recorder waits for the previous input restoration before taking a new device snapshot', async () => {
  const pending = wait(); let h, configurations = 0;
  h = hardware({ 'audio.configure': payload => {
    const apply = () => { Object.assign(h.audio, { inputDevice: payload.inputDevice, inputChannels: copy(payload.inputChannels), inputMetering: !!payload.inputMeter, monitoring: !!payload.monitor }); return copy(h.audio); };
    configurations++;
    return configurations === 2 ? pending.promise.then(apply) : apply();
  } });
  await h.open({ hardwareSample: true }); h.change('record-input-device', '33'); await h.get('record-meter-enable').fire('click');
  assert.equal(h.state.audio.inputDevice, 33); await h.get('record-close').fire('click');
  assert.equal(configurations, 2); assert.equal(h.state.modal, null);
  const opening = h.open({ hardwareSample: true }); await new Promise(resolve => setImmediate(resolve));
  assert.equal(h.calls.filter(call => call.command === 'audio.devices').length, 1); assert.equal(h.state.modal, null);
  pending.resolve(); await opening;
  assert.equal(h.calls.filter(call => call.command === 'audio.devices').length, 2); assert.equal(h.state.modal, 'recording');
  assert.equal(h.get('record-input-device').value, '22'); assert.equal(h.get('record-input-left').value, '2'); assert.equal(h.get('record-input-right').value, '3');
  assert.equal(h.get('record-monitor').checked, false); assert.equal(h.state.audio.inputMetering, false); assert.equal(h.edits.length, 0);
});
test('the implicit Studio stereo default becomes mono on a one-channel device and restores valid mono settings', async () => {
  for (const hardwareSample of [false, true]) {
    const h = (hardwareSample ? hardware : harness)();
    h.audio.inputs[0].inputChannels = 1; h.audio.inputChannels = [0, 1]; h.state.audio = copy(h.audio);
    if (hardwareSample) delete h.state.project.instruments[0].recordingInput;
    await h.open({ hardwareSample });
    assert.equal(h.get('record-input-left').value, '0'); assert.equal(h.get('record-input-right').value, 'mono');
    assert(!h.calls.some(call => call.command === 'audio.configure' || call.command === 'audio.record'));
    await h.startStop(); assert(h.ui.isRecording); await h.startStop();
    const settings = h.calls.filter(call => call.command === 'audio.configure');
    assert.equal(settings.length, 2); assert.deepEqual(settings[0].payload.inputChannels, [0]); assert.deepEqual(settings[1].payload.inputChannels, [0]);
    if (hardwareSample) assert.deepEqual(copy(h.state.project.instruments[0].recordingInput.channels), [0]);
  }
});
test('an explicit unavailable or malformed saved hardware channel map is never replaced by the Studio default', async () => {
  for (const channels of [[0, 1], [], null, ['0'], [0, 1, 2]]) {
    const h = hardware(); h.audio.inputs[0].inputChannels = 1; h.audio.inputChannels = [0, 1]; h.state.audio = copy(h.audio);
    h.state.project.instruments[0].recordingInput = { deviceUID: h.audio.inputs[0].uid, deviceName: 'Saved mono device', channels };
    await h.open({ hardwareSample: true }); await h.startStop();
    assert(!h.ui.isRecording); assert(!h.calls.some(call => call.command === 'audio.configure' || call.command === 'audio.record'));
    assert.match(h.get('record-status').textContent, /saved input channels are unavailable/);
    assert.deepEqual(copy(h.state.project.instruments[0].recordingInput.channels), channels);
  }
});
test('a rejected recording start restores every prior input setting without requiring a capture file', async () => {
  for (const hardwareSample of [false, true]) {
    const h = (hardwareSample ? hardware : harness)({ 'audio.record': () => { throw new Error('Capture writer unavailable'); } });
    h.audio.inputMetering = true; h.audio.monitoring = true; h.state.audio = copy(h.audio);
    await h.open({ hardwareSample }); h.change('record-input-device', '33'); h.get('record-input-left').value = '1'; h.get('record-input-right').value = 'mono'; h.get('record-monitor').checked = false;
    await h.startStop();
    const configurations = h.calls.filter(call => call.command === 'audio.configure'); assert.equal(configurations.length, 2);
    assert.equal(configurations[0].payload.inputDevice, 33); assert.deepEqual(configurations[0].payload.inputChannels, [1]); assert.equal(configurations[0].payload.monitor, false);
    assert.deepEqual(configurations[1].payload, { inputDevice: 22, inputChannels: [2, 3], inputMeter: true, monitor: true, monitorVolume: .6 });
    assert.equal(h.audio.inputDevice, 22); assert.deepEqual(h.audio.inputChannels, [2, 3]); assert.equal(h.audio.inputMetering, true); assert.equal(h.audio.monitoring, true);
    assert.equal(h.state.audio.inputDevice, 22); assert.deepEqual(copy(h.state.audio.inputChannels), [2, 3]);
    assert.equal(h.ui.lastRecording, null); assert(!h.ui.isBusy && !h.ui.isRecording); assert.match(h.get('record-status').textContent, /Capture writer unavailable/);
    assert.equal(h.calls.filter(call => call.command === 'audio.record').length, 1); assert(!h.calls.some(call => call.command === 'audio.stopRecord'));
  }
});
test('partially applied input configuration is restored when either capture or input checking fails', async () => {
  for (const hardwareSample of [false, true]) for (const action of ['record', 'check']) {
    let h, configurations = 0;
    h = (hardwareSample ? hardware : harness)({ 'audio.configure': payload => {
      const result = configureAudio(h.audio, payload);
      if (++configurations === 1) { assert.equal(h.audio.inputDevice, 33); throw new Error('Input stream failed after device selection'); }
      return result;
    } });
    h.audio.inputMetering = true; h.audio.monitoring = true; h.state.audio = copy(h.audio);
    await h.open({ hardwareSample }); h.change('record-input-device', '33'); h.get('record-input-right').value = 'mono'; h.get('record-monitor').checked = false;
    if (action === 'record') await h.startStop(); else await h.get('record-meter-enable').fire('click');
    assert.equal(configurations, 2); assert(!h.calls.some(call => call.command === 'audio.record'));
    assert.deepEqual(h.calls.filter(call => call.command === 'audio.configure')[1].payload, { inputDevice: 22, inputChannels: [2, 3], inputMeter: true, monitor: true, monitorVolume: .6 });
    assert.equal(h.audio.inputDevice, 22); assert.deepEqual(h.audio.inputChannels, [2, 3]); assert.equal(h.audio.inputMetering, true); assert.equal(h.audio.monitoring, true);
    assert.equal(h.state.audio.inputDevice, 22); assert(!h.ui.isBusy && !h.ui.isRecording); assert.equal(h.ui.lastRecording, null);
    assert.match(h.get('record-status').textContent, /Input stream failed after device selection/);
  }
});
test('a successful retry configures input anew and restores it again after an earlier recording failure', async () => {
  for (const hardwareSample of [false, true]) for (const retry of ['check-close', 'record-stop']) {
    let attempts = 0;
    const h = (hardwareSample ? hardware : harness)({ 'audio.record': () => {
      if (++attempts === 1) throw new Error('First capture start rejected');
      return { recording: true, path: '/recordings/Synth-take-uuid.wav' };
    } });
    await h.open({ hardwareSample }); h.change('record-input-device', '33'); await h.startStop();
    assert.equal(h.audio.inputDevice, 22); assert.equal(h.calls.filter(call => call.command === 'audio.configure').length, 2);
    if (retry === 'check-close') {
      await h.get('record-meter-enable').fire('click'); assert.equal(h.audio.inputDevice, 33);
      await h.get('record-close').fire('click'); for (let i = 0; i < 5; i++) await Promise.resolve();
    } else { await h.startStop(); assert(h.ui.isRecording); assert.equal(h.audio.inputDevice, 33); await h.startStop(); }
    const configurations = h.calls.filter(call => call.command === 'audio.configure');
    assert.deepEqual(configurations.map(call => call.payload.inputDevice), [33, 22, 33, 22]);
    assert.deepEqual(configurations[3].payload.inputChannels, [2, 3]); assert.equal(configurations[3].payload.inputMeter, false); assert.equal(configurations[3].payload.monitor, false);
    assert.equal(h.audio.inputDevice, 22); assert.deepEqual(h.audio.inputChannels, [2, 3]); assert.equal(h.state.modal, null); assert(!h.ui.isBusy && !h.ui.isRecording);
  }
});
test('input restoration failure stays visible and close or reopen does not repeat it for the same configuration attempt', async () => {
  for (const hardwareSample of [false, true]) for (const failure of ['record', 'check', 'decode']) {
    let h;
    h = (hardwareSample ? hardware : harness)({
      'audio.configure': payload => {
        if (payload.inputDevice === 22) throw new Error('Original interface cannot be restored');
        const result = configureAudio(h.audio, payload);
        if (failure === 'check') throw new Error('Input check failed after device selection');
        return result;
      },
      'audio.record': () => { if (failure === 'record') throw new Error('Capture start rejected'); return { recording: true, path: '/recordings/Synth-take-uuid.wav' }; },
      'sample.load': () => { throw new Error('Saved take decoder unavailable'); }
    });
    await h.open({ hardwareSample }); h.change('record-input-device', '33');
    if (failure === 'check') await h.get('record-meter-enable').fire('click');
    else { await h.startStop(); if (failure === 'decode') await h.startStop(); }
    assert.equal(h.calls.filter(call => call.command === 'audio.configure').length, 2);
    assert.match(h.get('record-status').textContent, /Original interface cannot be restored/);
    assert.match(h.get('record-status').textContent, failure === 'record' ? /Capture start rejected/ : failure === 'check' ? /Input check failed/ : /Saved take decoder unavailable/);
    assert.equal(h.toasts.filter(toast => /Original interface cannot be restored/.test(toast.message)).length, 1);
    assert(!h.ui.isBusy && !h.ui.isRecording);
    if (failure === 'decode') { assert.equal(h.ui.lastRecording.path, '/recordings/Synth-take-uuid.wav'); assert.equal(h.get('record-saved-path').value, '/recordings/Synth-take-uuid.wav'); }
    else assert.equal(h.ui.lastRecording, null);
    await h.get('record-close').fire('click'); for (let i = 0; i < 5; i++) await Promise.resolve();
    await h.open({ hardwareSample }); await h.get('record-close').fire('click'); for (let i = 0; i < 5; i++) await Promise.resolve();
    assert.equal(h.calls.filter(call => call.command === 'audio.configure').length, 2);
    assert.equal(h.toasts.filter(toast => /Original interface cannot be restored/.test(toast.message)).length, 1);
    assert.equal(h.state.project.instruments.length, 1); assert.equal(h.state.project.tracks.length, 1);
    assert(!h.calls.some(call => /delete|remove/i.test(call.command)));
  }
});

function takeFixture(h) {
  h.state.view = 'song';
  const clip = {id:'region',track:0,name:'Synth phrase',path:'/original.wav',startBeat:4,durationBeats:4,sourceOffset:0,gain:.7,fadeIn:0,fadeOut:0,muted:false};
  h.state.project.audioClips.push(clip); h.state.clipSelected = clip.id;
  return clip;
}
test('record alternative locks its destination and attaches one take without new clips or sampler changes', async () => {
  const h = harness(), clip = takeFixture(h); await h.open({takeTarget:h.takes.target()});
  assert.equal(h.get('record-destination').value,'take'); assert(h.get('record-destination').disabled); assert(h.get('record-root-note').disabled);
  assert.deepEqual(h.calls.map(c=>c.command),['audio.devices']);h.get('record-name').value='Second performance';
  await h.startStop(); await h.startStop();
  assert.equal(h.state.project.audioClips.length,1); assert.equal(h.state.project.instruments.length,1);assert.equal(clip.takeSet.takes.length,2);
  assert.equal(clip.path,'/recordings/Synth-take-uuid.wav');assert.equal(clip.takeSet.takes[0].path,'/original.wav');assert.equal(clip.takeSet.takes[0].sourceDuration,2);assert.equal(clip.takeSet.takes[1].name,'Second performance');
  assert.equal(clip.startBeat,4);assert.equal(clip.durationBeats,4);assert.equal(clip.gain,.7);assert.equal(h.edits.length,1);assert.equal(h.state.modal,null);
});
test('an alternative can be recorded with all 256 clip slots occupied', async () => {
  const h=harness(),clip=takeFixture(h);for(let i=1;i<256;i++)h.state.project.audioClips.push({...clip,id:'other-'+i});
  await h.open({takeTarget:h.takes.target()});await h.startStop();await h.startStop();assert.equal(h.state.project.audioClips.length,256);assert.equal(clip.takeSet.takes.length,2);
});
test('short alternative capture is retained for recovery and never shrinks or replaces the region', async () => {
  const h=harness({'audio.clipInfo':p=>({duration:p.path==='/original.wav'?2:1})}),clip=takeFixture(h),before=copy(clip);
  await h.open({takeTarget:h.takes.target()});await h.startStop();await h.startStop();assert.deepEqual(clip,before);assert.equal(h.edits.length,0);assert(h.ui.lastRecording.path);assert.match(h.get('record-status').textContent,/at least 2.00/);assert.equal(h.get('record-saved').hidden,false);
  await h.startStop();assert.equal(h.calls.filter(c=>c.command==='audio.record').length,1);assert.deepEqual(clip,before);
});
test('transient alternate metadata failure retries attachment without recording again', async () => {
  let reads=0;const h=harness({'audio.clipInfo':p=>{if(p.path!=='/original.wav'&&++reads===1)throw Error('Metadata not ready');return{duration:2};}}),clip=takeFixture(h);
  await h.open({takeTarget:h.takes.target()});await h.startStop();await h.startStop();assert(!clip.takeSet);assert(h.ui.lastRecording.path);
  await h.startStop();assert.equal(clip.takeSet.takes.length,2);assert.equal(h.calls.filter(c=>c.command==='audio.record').length,1);assert.equal(h.state.modal,null);
});
test('a changed or removed destination during alternate capture retains the file and never attaches elsewhere', async () => {
 for(const change of ['edit','remove','project']){
  const h=harness(),clip=takeFixture(h);await h.open({takeTarget:h.takes.target()});await h.startStop();
  if(change==='edit')clip.durationBeats=3;else if(change==='remove')h.state.project.audioClips=[];else h.state.project=copy(h.state.project);
  await h.startStop();assert(h.ui.lastRecording.path);assert.equal(h.edits.length,0);assert(!clip.takeSet);assert.match(h.get('record-status').textContent,/changed/);
 }
});
test('a destination edit while alternate metadata is pending cannot be overwritten', async () => {
 const pending=wait(),h=harness({'audio.clipInfo':p=>p.path==='/original.wav'?{duration:2}:pending.promise}),clip=takeFixture(h);
 await h.open({takeTarget:h.takes.target()});await h.startStop();const stopping=h.startStop();await new Promise(resolve=>setImmediate(resolve));
 clip.name='Newer region name';pending.resolve({duration:2});await stopping;assert.equal(clip.name,'Newer region name');assert(!clip.takeSet);assert(h.ui.lastRecording.path);assert.equal(h.edits.length,0);
});
test('failed project operation cannot clear a captured alternate before its edit is applied', async () => {
 const h=harness(),clip=takeFixture(h);await h.open({takeTarget:h.takes.target()});await h.startStop();h.ST.isChangingSession=true;
 await h.startStop();assert(!clip.takeSet);assert(h.ui.lastRecording.path);assert.equal(h.edits.length,0);assert.match(h.get('record-status').textContent,/project operation/);
 h.ST.isChangingSession=false;await h.startStop();assert.equal(clip.takeSet.takes.length,2);assert.equal(h.calls.filter(c=>c.command==='audio.record').length,1);
});


test('recorded timeline audio gets its length in beats from its duration at the project tempo',async()=>{
 const h=harness();h.state.view='song';h.state.project.bpm=90;
 await h.open();h.change('record-destination','clip');await h.startStop();await h.startStop();const clip=h.state.project.audioClips[0];assert.equal(clip.startBeat,16);assert.equal(clip.durationBeats,3);assert.equal(clip.sourceDuration,2);
});

test('empty recording name returns keyboard focus to the enabled name field without preparing audio', async () => {
  const h = harness(); await h.open(); const name = h.get('record-name'); name.value = '   ';
  name.focus = function () { if (!this.disabled) h.document.activeElement = this; };
  h.get('record-start').focus(); await h.startStop();
  assert.equal(h.document.activeElement, name); assert.equal(name.disabled, false); assert.equal(h.ui.isBusy, false);
  assert.match(h.get('record-status').textContent, /Give this recording a name/);
  assert.deepEqual(h.calls.map(call => call.command), ['audio.devices']); assert.equal(h.edits.length, 0);
});

(async () => { for (const item of tests) { await item.fn(); console.log(`PASS ${item.name}`); } console.log(`Recording UI: ${tests.length} regression groups passed.`); })().catch(error => { console.error(error); process.exitCode = 1; });
