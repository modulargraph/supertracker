'use strict';
// Actual app/modal/keyboard lifecycle with deferred native replies. The reused
// parser models focus and detached nodes; no native device operations execute.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
let fixture = fs.readFileSync(path.join(__dirname, 'mounted_song_tests.js'), 'utf8');
fixture = fixture.slice(0, fixture.indexOf('const tests=[],test='));
// Browser properties absent from the smaller Song view fixture are relevant here.
const fieldDefaults = "node.value=attrs.value||'';";
assert(fixture.includes(fieldDefaults));
fixture = fixture.replace(fieldDefaults, "let tabIndex=node.tabIndex;Object.defineProperty(node,'tabIndex',{get:()=>tabIndex,set(value){tabIndex=Number(value);node.attrs.tabindex=String(value);}});const originalGetAttribute=node.getAttribute.bind(node);node.getAttribute=name=>name==='tabindex'?(Object.hasOwn(node.attrs,'tabindex')?String(node.attrs.tabindex):null):originalGetAttribute(name);if(attrs.tabindex!==undefined)node.tabIndex=Number(attrs.tabindex);node.hidden=Object.hasOwn(attrs,'hidden');node.checked=Object.hasOwn(attrs,'checked');node.classList={contains:value=>node.classes.has(value),add:value=>node.classes.add(value),remove:value=>node.classes.delete(value)};node.setAttribute=(name,value)=>{node.attrs[name]=String(value);if(name==='tabindex')node.tabIndex=Number(value);};node.removeAttribute=name=>{delete node.attrs[name];};" + fieldDefaults);
fixture = fixture.replace('detach(this);this.children=[];', 'detach(this);this.children=[];if(h.document.activeElement?.isConnected===false)h.document.activeElement=h.document.body;');
const makeHarness = new Function('require', '__dirname', fixture + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let i = 0; i < 80; i++) await Promise.resolve(); };
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const audio = { inputs: [{ id: 11, name: 'Line in', inputChannels: 2 }], outputs: [{ id: 22, name: 'Studio output' }], inputDevice: 11, outputDevice: 22, defaultInput: 11, defaultOutput: 22, inputChannels: [0, 1], bufferSizes: [128, 256, 512], bufferSize: 256, sampleRate: 48000, monitorVolume: .7, monitoring: false };
const midi = { inputs: [{ id: -7, name: 'Keys' }], outputs: [{ id: -9, name: 'Synth' }], inputIDs: [-7], outputID: -9, allInputs: false, inputChannel: 0, channel: 3, enabled: true };
function harness() {
  const h = makeHarness('pattern');
  h.handlers.set('audio.devices', () => copy(audio)); h.handlers.set('midi.devices', () => copy(midi));
  h.handlers.set('audio.configure', payload => ({ ...copy(audio), ...payload })); h.handlers.set('midi.configure', payload => ({ ...copy(midi), ...payload }));
  return { ...h,
    key(key, extra = {}) { const event = { key, code: key, target: h.document.activeElement, preventDefault() { this.defaultPrevented = true; }, ...extra }; h.ST.keyboard(event); return event; },
    async show() { h.get('settings').focus(); await h.ST.showSettings(); h.flush(); },
    writes: () => h.calls.filter(call => call.command.endsWith('.configure')),
  };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('delayed Settings discovery cannot reappear after a newer modal opens and closes', async () => {
  const h = harness(), gate = deferred(); h.handlers.set('audio.devices', () => gate.promise); h.get('settings').focus(); const opening = h.ST.showSettings(); await tick();
  h.ST.openModal('Newer intent', '<button id="newer-action">Continue</button>'); h.flush(); h.ST.closeModal(); assert.equal(h.state.modal, null);
  gate.resolve(copy(audio)); await opening; h.flush(); assert.equal(h.state.modal, null); assert(h.document.getElementById('settings-apply') === null, 'No stale Settings controls should be mounted'); assert.equal(h.document.activeElement.id, 'settings'); assert.equal(h.writes().length, 0);
});

test('the newest Settings invocation wins even when discovery resolves out of order', async () => {
  const h = harness(), first = deferred(), second = deferred(); let calls = 0;
  h.handlers.set('audio.devices', () => (++calls === 1 ? first.promise : second.promise));
  const oldOpening = h.ST.showSettings(); await tick(); const newOpening = h.ST.showSettings(); await tick(); second.resolve({ ...copy(audio), bufferSize: 512 }); await newOpening; h.flush();
  const button = h.get('settings-apply'); assert.equal(h.get('audio-buffer').value, '512'); first.resolve({ ...copy(audio), bufferSize: 128 }); await oldOpening; h.flush();
  assert(h.get('settings-apply') === button, "h.get('settings-apply') should retain the expected owner/focus"); assert.equal(h.get('audio-buffer').value, '512'); assert.equal(h.state.audio.bufferSize, 512); assert.equal(h.writes().length, 0);
});

test('project replacement suppresses stale Settings discovery without stealing the replacement focus', async () => {
  const h = harness(), gate = deferred(); h.handlers.set('audio.devices', () => gate.promise); const opening = h.ST.showSettings(); await tick();
  h.ST.acceptProject(h.ST.demoProject()); h.get('tracker').focus(); const project = h.state.project; gate.resolve(copy(audio)); await opening; h.flush();
  assert(h.state.project === project, 'h.state.project should retain the expected owner/focus'); assert.equal(h.state.modal, null); assert.equal(h.document.activeElement.id, 'tracker'); assert(h.document.getElementById('settings-apply') === null, 'No stale Settings controls should be mounted');
});

test('pending Apply focuses its visible status and traps Tab while every dismissal and replacement path stays locked', async () => {
  const h = harness(), gate = deferred(); await h.show(); h.get('audio-buffer').value = '128'; h.handlers.set('audio.configure', () => gate.promise);
  const apply = h.get('settings-apply'), applying = apply.onclick(); await tick();
  const status = h.get('settings-status'); assert.equal(status.tabIndex, 0); assert.equal(status.hidden, false); assert(status.getClientRects().length); assert.match(status.textContent || status.innerHTML, /applying|please wait/i); assert(h.document.activeElement === status, 'h.document.activeElement should retain the expected owner/focus');
  for (const id of ['audio-buffer', 'midi-output-channel', 'settings-apply', 'settings-cancel', 'modal-close']) assert.equal(h.get(id).disabled, true, id);
  for (const shiftKey of [false, true]) { assert(h.key('Tab', { shiftKey }).defaultPrevented); assert(h.document.activeElement === status, 'h.document.activeElement should retain the expected owner/focus'); }
  const scrim = h.document.querySelector('.modal-scrim');
  for (const dismiss of [() => h.key('Escape'), () => scrim.onclick({ target: scrim }), () => h.ST.closeModal(), () => h.get('modal-close').onclick(), () => h.get('settings-cancel').onclick(), () => h.ST.openModal('Other', '<button id="other">Wrong</button>')]) {
    dismiss(); assert.equal(h.state.modal, 'settings'); assert(h.get('settings-apply') === apply, "h.get('settings-apply') should retain the expected owner/focus"); assert(h.document.activeElement === status, 'h.document.activeElement should retain the expected owner/focus');
  }
  const project=h.state.project; await h.ST.perform('new'); await h.ST.projectAction('new'); await tick(); assert(h.state.project===project, 'Pending Apply prevents session replacement'); assert.equal(h.state.modal,'settings'); assert.equal(h.calls.filter(call=>call.command==='project.new').length,0);
  gate.resolve({ ...copy(audio), bufferSize: 128 }); await applying; h.flush(); assert.equal(h.state.modal, null); assert(h.document.activeElement === h.get('settings'), 'h.document.activeElement should retain the expected owner/focus'); assert.equal(h.writes().length, 1);
});

test('successful Apply restores the current invoker after the actual app refresh replaces its DOM', async () => {
  const h = harness(); const oldInvoker = h.get('settings'); await h.show(); h.get('audio-buffer').value = '512'; await h.get('settings-apply').onclick(); h.flush();
  assert.equal(h.state.modal, null); assert(h.document.activeElement === h.get('settings'), 'h.document.activeElement should retain the expected owner/focus'); assert.equal(h.document.activeElement.isConnected, true); assert.equal(oldInvoker.isConnected, false); assert.equal(h.state.audio.bufferSize, 512);
});

test('partial MIDI failure keeps its error and focus in the same dialog; retry omits acknowledged audio changes', async () => {
  const h = harness(); let rejectMIDI = true; await h.show(); h.get('audio-buffer').value = '128'; h.get('midi-output-channel').value = '9';
  h.handlers.set('midi.configure', payload => { if (rejectMIDI) throw Error('MIDI port is disconnected'); return { ...copy(midi), ...payload }; });
  const apply = h.get('settings-apply'); await apply.onclick(); h.flush(); assert.equal(h.state.modal, 'settings'); assert(h.get('settings-apply') === apply, "h.get('settings-apply') should retain the expected owner/focus");
  const status = h.get('settings-status'); assert.equal(status.hidden, false); assert.match(status.textContent || status.innerHTML, /MIDI port is disconnected/); assert(h.document.activeElement === apply, 'h.document.activeElement should retain the expected owner/focus');
  for (const id of ['audio-buffer', 'midi-output-channel', 'settings-apply', 'settings-cancel', 'modal-close']) assert.equal(h.get(id).disabled, false, id);
  assert.deepEqual(h.writes().map(call => [call.command, call.payload]), [['audio.configure', { bufferSize: 128 }], ['midi.configure', { channel: 9 }]]);
  rejectMIDI = false; await apply.onclick(); h.flush(); assert.equal(h.state.modal, null); assert.equal(h.writes().filter(call => call.command === 'audio.configure').length, 1); assert.equal(h.writes().filter(call => call.command === 'midi.configure').length, 2); assert(h.document.activeElement === h.get('settings'), 'h.document.activeElement should retain the expected owner/focus');
  const escaping=harness();await escaping.show();escaping.get('audio-buffer').value='128';escaping.handlers.set('audio.configure',()=>{throw Error('Audio interface unavailable');});await escaping.get('settings-apply').onclick();escaping.flush();assert.equal(escaping.state.modal,'settings');assert(escaping.key('Escape').defaultPrevented);assert.equal(escaping.state.modal,null);assert(escaping.document.activeElement===escaping.get('settings'),'Escape after error restores the invoker');
});

(async () => { let passed = 0; for (const { name, run } of tests) { let timeout; try { await Promise.race([run(), new Promise((_, reject) => { timeout = setTimeout(() => reject(Error('Timed out: Settings did not settle')), 4000); })]); passed++; console.log('PASS ' + name); } catch (error) { console.error('FAIL ' + name + '\n' + error.stack); } finally { clearTimeout(timeout); } } console.log(`Settings lifecycle: ${passed}/${tests.length} regression groups passed.`); if (passed !== tests.length) process.exitCode = 1; })();
