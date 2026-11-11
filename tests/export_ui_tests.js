'use strict';
// Drive the real export dialog with only DOM and native host boundaries stubbed.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const installEngineSyncBoundary = require('./helpers/engine_sync_boundary');
const source = web.source('export.js');
const clone = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
function deferred() { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const decode = value => value.replace(/&(amp|lt|gt|quot|#39);/g, (_, c) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" }[c]));
// What the host answers to routing.capabilities for the fixture project, and how it confirms a delivery.
const { audioExport, exportDelivery: delivered } = require('./helpers/host');
function capabilities() {
  return { fullGraphStems: true, audioExport: audioExport(), exportTargets: [
    { owner: 'track:keys', name: 'Keys', kind: 'track', boundary: 'postFaderPreParent', available: true },
    { owner: 'track:bass', name: 'Bass', kind: 'track', boundary: 'postFaderPreParent', available: true },
    { owner: 'bus:drums', name: 'Drum group', kind: 'group', boundary: 'postFaderPreParent', available: true },
    { owner: 'bus:hall', name: 'Hall return', kind: 'return', boundary: 'postFaderPreParent', available: true },
    { owner: 'master', name: 'Master', kind: 'master', boundary: 'masterOutput', available: true }
  ] };
}
function harness(overrides = {}) {
  const nodes = new Map(), calls = [], mutations = [], toasts = [];
  const state = { native: true, engineReady: true, modal: null, view: 'song', project: { name: 'Export fixture', activePattern: 4, tracks: [{ routingID: 'keys', name: 'Keys', instrument: 0 }, { routingID: 'bass', name: 'Bass', instrument: 1 }], instruments: [{}, {}], patterns: [{ id: 4, length: 64, automation: [] }], audioPlugins: [], routing: { mixModel: 'standard', buses: [{ id: 'drums', type: 'group', name: 'Drum group' }, { id: 'hall', type: 'return', name: 'Hall return' }], sends: [], sidechains: [] } } };
  let roots = [], ui, modalHTML = '';
  const all = () => roots.flatMap(root => [root, ...descendants(root)]);
  function descendants(element) { return element.children.flatMap(child => [child, ...descendants(child)]); }
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => all().filter(item => matches(item, selector)), querySelector: selector => document.querySelectorAll(selector)[0] || null };
  function matches(element, selector) {
    if (selector.endsWith(':not(:disabled)')) return !element.disabled && matches(element, selector.slice(0, -':not(:disabled)'.length));
    if (selector.startsWith('#')) return element.id === selector.slice(1);
    if (selector.startsWith('.')) return (element.attributes.class || '').split(/\s+/).includes(selector.slice(1));
    const match = /^(\w+)?\[([^=\]]+)(?:="([^"]*)")?\]$/.exec(selector);
    if (match) return (!match[1] || element.tagName === match[1].toUpperCase()) && Object.hasOwn(element.attributes, match[2]) && (match[3] === undefined || element.attributes[match[2]] === match[3]);
    return element.tagName === selector.toUpperCase();
  }
  function parse(markup, parent = null) {
    const result = [], stack = parent ? [parent] : [];
    for (const match of markup.matchAll(/<\/?[\w:-]+\b[^>]*>/g)) {
      const raw = match[0], tagName = /^<\/?([\w:-]+)/.exec(raw)[1].toUpperCase();
      if (raw.startsWith('</')) { while (stack.length && stack.at(-1) !== parent && stack.pop().tagName !== tagName) {} continue; }
      const attributes = {}, dataset = {};
      for (const a of raw.slice(/^<[\w:-]+/.exec(raw)[0].length, -1).matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) { const name = a[1], value = decode(a[2] ?? a[3] ?? a[4] ?? ''); attributes[name] = value; if (name.startsWith('data-')) dataset[name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value; }
      const element = { id: attributes.id || '', tagName, attributes, dataset, parentElement: stack.at(-1) || null, children: [], handlers: {}, style: {}, disabled: Object.hasOwn(attributes, 'disabled'), hidden: Object.hasOwn(attributes, 'hidden'), checked: Object.hasOwn(attributes, 'checked'), value: attributes.value || '', type: attributes.type || '', textContent: '', selectionStart: 0, classList: { toggle() {}, add() {}, remove() {} },
        addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }, setAttribute(name, value) { this.attributes[name] = String(value); }, removeAttribute(name) { delete this.attributes[name]; }, focus() { document.activeElement = this; }, select() {}, setSelectionRange() {}, scrollIntoView() {},
        querySelectorAll(selector) { return descendants(this).filter(child => matches(child, selector)); }, querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }, closest(selector) { for (let current = this; current; current = current.parentElement) if (matches(current, selector)) return current; return null; },
        async fire(type, extra = {}) { const event = { target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; for (let current = this; current; current = current.parentElement) { for (const fn of current.handlers[type] || []) await fn(event); if (typeof current['on' + type] === 'function') await current['on' + type](event); if (event.stopped) break; } return event; }
      };
      Object.defineProperty(element, 'innerHTML', { get() { return this.markup || ''; }, set(value) { for (const child of descendants(this)) if (child.id && nodes.get(child.id) === child) nodes.delete(child.id); this.children = []; this.markup = value; parse(value, this); } });
      if (element.parentElement) element.parentElement.children.push(element); else result.push(element);
      if (element.id) nodes.set(element.id, element);
      if (tagName === 'OPTION' && element.parentElement?.tagName === 'SELECT' && (Object.hasOwn(attributes, 'selected') || element.parentElement.children.length === 1)) element.parentElement.value = element.value;
      if (!['INPUT', 'BR', 'HR', 'IMG', 'META', 'LINK'].includes(tagName) && !raw.endsWith('/>')) stack.push(element);
    }
    return result;
  }
  const defaults = { 'routing.capabilities': capabilities, 'dialog.export': payload => ({ path: payload.mode === 'stems' ? '/exports/stems' : '/exports/mix.wav' }), 'engine.project': () => ({}), 'audio.export': payload => ({ path: payload.path, paths: payload.mode === 'stems' ? (payload.owners || ['track:keys', 'track:bass']).map((_, i) => `/exports/stems/Stem-${i + 1}.wav`) : [payload.path], ...delivered(payload) }), 'transport.stop': () => ({}) };
  const ST = { state, escapeHTML, pattern: () => state.project.patterns[0], request(command, payload = {}) { calls.push({ command, payload: clone(payload) }); return Promise.resolve().then(() => (overrides[command] || defaults[command] || (() => ({})))(payload)); },
    waitForGraphEdit: async () => { calls.push({ command: 'waitForGraphEdit' }); }, captureNativeEdits: async () => { calls.push({ command: 'captureNativeEdits' }); return true; }, syncEngine: async () => { throw new Error('Export must use strict engine.project, not best-effort syncEngine'); },
    mutate(label, callback) { mutations.push(label); callback(state.project); }, toast(message, kind) { toasts.push({ message, kind }); },
    openModal(title, body, footer, options = {}) { state.modal = options.kind; modalHTML = body + footer; nodes.clear(); roots = parse(modalHTML); },
    closeModal() { if (ui.isBusy) return; state.modal = null; ui.closed(); nodes.clear(); roots = []; }
  };
  const studio = { isEditingPlugin: false, isPreparingEffectMove: false }, routing = { isPreparing: false }, instrumentEdits = {isBusy:false}, recording = {isBusy:false,isRecording:false};
  const syncs = installEngineSyncBoundary(ST);
  const context = vm.createContext({ window: { ST, STStudio: studio, STRouting: routing, STInstrumentEdits:instrumentEdits, STRecording:recording }, document, setTimeout: () => 1, clearTimeout() {}, requestAnimationFrame: fn => fn() }); web.load(context, 'export.js', { text: source }); ui = context.window.STExport;
  const get = id => { const element = nodes.get(id); assert(element, `Missing ${id}`); return element; };
  return { ui, ST, syncs, studio, routing, instrumentEdits, recording, state, document, nodes, calls, mutations, toasts, overrides, get, get html() { return modalHTML; }, open: () => ui.show(), start: () => get('export-start').fire('click'), change: async (id, value) => { get(id).value = value; return get(id).fire('change'); }, async settle() { for (let i = 0; i < 8; i++) await tick(); } };
}

const tests = [], test = (name, run) => tests.push({ name, run });
const stems = h => h.document.querySelectorAll('[data-export-owner]');
const stem = (h, owner) => { const element = stems(h).find(item => item.dataset.exportOwner === owner); assert(element, `Missing export owner ${owner}`); return element; };
const selected = h => stems(h).filter(item => item.checked).map(item => item.dataset.exportOwner);
const resultPath = h => h.get('export-result-path').value || h.get('export-result-path').textContent;
const exported = h => h.calls.filter(call => call.command === 'audio.export');
const successes = h => h.toasts.filter(toast => toast.kind === 'success');
async function check(h, owner, checked) { const element = stem(h, owner); element.checked = checked; await element.fire('change'); }

test('instrument edits, recording and input restoration settle before either export caller can acquire the lock',async()=>{
  for(const [surface,flag] of [['instrumentEdits','isBusy'],['recording','isBusy'],['recording','isRecording']])for(const caller of ['dialog','headless']){
    const h=harness();if(caller==='dialog')await h.open();h[surface][flag]=true;
    if(caller==='headless')await assert.rejects(h.ui.exportTo({path:'/exports/test.wav'}),/finish/);else await h.start();
    assert(!h.ui.isBusy);assert(!h.calls.some(c=>['dialog.export','captureNativeEdits','engine.project','audio.export'].includes(c.command)));
    h[surface][flag]=false;if(caller==='headless')await h.ui.exportTo({path:'/exports/test.wav'});else await h.start();assert.equal(exported(h).length,1);
  }
});
test('read-only preparation state permits prior capture only before rendering for both export callers',async()=>{
  for(const caller of ['dialog','headless']){
    const graph=deferred(),chooser=deferred(),capture=deferred(),sync=deferred(),render=deferred(),stop=deferred();
    const h=harness({'dialog.export':()=>chooser.promise,'audio.export':()=>render.promise,'transport.stop':()=>stop.promise});let waits=0;
    h.ST.waitForGraphEdit=()=>++waits===1?graph.promise:Promise.resolve();h.ST.captureNativeEdits=()=>capture.promise;h.ST.beforeEngineSync=()=>sync.promise;
    const descriptor=Object.getOwnPropertyDescriptor(h.ui,'isPreparing');assert.equal(typeof descriptor.get,'function');assert.equal(descriptor.set,undefined);assert.equal(h.ui.isPreparing,false);
    if(caller==='dialog')await h.open();
    const run=caller==='dialog'?h.start():h.ui.exportTo({path:'/exports/test.wav'});
    const settled=caller==='headless'?assert.rejects(run,/canceled/):run;
    await h.settle();assert.equal(h.ui.isBusy,false);assert.equal(h.ui.isPreparing,false);
    graph.resolve();await h.settle();
    if(caller==='dialog'){assert.equal(h.ui.isBusy,true);assert.equal(h.ui.isPreparing,false);chooser.resolve({path:'/exports/test.wav'});await h.settle();}
    assert.equal(h.ui.isBusy,true);assert.equal(h.ui.isPreparing,true);assert.equal(exported(h).length,0);
    capture.resolve(true);await h.settle();assert.equal(h.ui.isPreparing,true);assert.equal(exported(h).length,0);
    sync.resolve(true);await h.settle();assert.equal(h.ui.isBusy,true);assert.equal(h.ui.isPreparing,false);assert.equal(exported(h).length,1);
    assert.equal(h.ui.cancel(),true);assert.equal(h.ui.isPreparing,false);render.resolve({cancelled:true});await h.settle();assert.equal(h.ui.isBusy,true);assert.equal(h.ui.isPreparing,false);
    stop.resolve({});await settled;assert.equal(h.ui.isBusy,false);assert.equal(h.ui.isPreparing,false);
  }
});
test('headless export shares strict preparation and the public edit lock without creating a dialog',async()=>{
  const gate=deferred(),h=harness({'audio.export':()=>gate.promise});const run=h.ui.exportTo({mode:'song',path:'/exports/headless.wav',tail:2});await h.settle();
  assert(h.ui.isBusy);assert.equal(h.state.modal,null);assert.equal(h.calls.filter(c=>c.command==='dialog.export').length,0);
  assert.deepEqual(h.calls.filter(c=>['captureNativeEdits','engine.project','audio.export'].includes(c.command)).map(c=>c.command),['captureNativeEdits','engine.project','audio.export']);
  assert.equal(await h.ui.show(),false);await assert.rejects(h.ui.exportTo({path:'/exports/duplicate.wav'}),/current audio export/);
  h.ui.event('audio.exportProgress',{path:'/exports/headless.wav',progress:.4});assert.equal(h.ui.progress,.4);
  gate.resolve({path:'/exports/headless.wav',paths:['/exports/headless.wav'],...delivered(exported(h)[0].payload)});const result=await run;assert.deepEqual(clone(result.paths),['/exports/headless.wav']);assert(!h.ui.isBusy);assert.equal(h.ui.progress,null);assert.equal(h.state.modal,null);
});
test('headless preflight waits for graph commits before locking and rejects replaced pattern identities at every boundary',async()=>{
  for(const stage of ['wait','capture','sync'])for(const change of ['project','pattern','pattern-id']){
    const h=harness(),gate=deferred();if(stage==='wait')h.ST.waitForGraphEdit=()=>gate.promise;if(stage==='capture')h.ST.captureNativeEdits=()=>gate.promise;if(stage==='sync')h.ST.beforeEngineSync=()=>gate.promise;
    const run=h.ui.exportTo({mode:'pattern',pattern:4,path:'/exports/pattern.wav'});const rejected=assert.rejects(run,/changed/);await h.settle();assert.equal(h.ui.isBusy,stage!=='wait');
    if(change==='project')h.state.project=clone(h.state.project);else if(change==='pattern')h.state.project.patterns[0]={...h.state.project.patterns[0]};else h.state.project.patterns[0].id=5;
    gate.resolve(true);await rejected;assert.equal(exported(h).length,0);assert(!h.ui.isBusy);
  }
});
test('headless invalid, canceled, incomplete and failed native work releases the export lock for retry',async()=>{
  for(const result of [null,{cancelled:true},{success:false},{paths:[]},{paths:['/a.wav','/b.wav']}]){
    const h=harness({'audio.export':()=>result});await assert.rejects(h.ui.exportTo({path:'/exports/test.wav'}));assert(!h.ui.isBusy);assert.equal(successes(h).length,0);
  }
  for(const stage of ['capture','sync','native']){
    const h=harness();if(stage==='capture')h.ST.captureNativeEdits=async()=>false;if(stage==='sync')h.ST.syncEngineProject=async()=>false;if(stage==='native')h.overrides['audio.export']=()=>{throw Error('Render failed');};
    await assert.rejects(h.ui.exportTo({path:'/exports/test.wav'}));assert(!h.ui.isBusy);
  }
  const h=harness();await h.open();await assert.rejects(h.ui.exportTo({path:'/exports/test.wav'}),/close the current audio export/);assert.equal(exported(h).length,0);
});
test('headless cancellation is single-flight and holds the lock until both native replies settle',async()=>{
  const render=deferred(),stop=deferred(),h=harness({'audio.export':()=>render.promise,'transport.stop':()=>stop.promise});
  const run=h.ui.exportTo({path:'/exports/canceled.wav'}),rejected=assert.rejects(run,/canceled/);await h.settle();assert.equal(h.ui.cancel(),true);assert.equal(h.ui.cancel(),false);
  render.resolve({cancelled:true});await h.settle();assert(h.ui.isBusy);stop.resolve({});await rejected;assert(!h.ui.isBusy);assert.equal(h.calls.filter(c=>c.command==='transport.stop').length,1);
});
test('headless format and selected stems honor capabilities and require actual delivery confirmation',async()=>{
  const h=harness({'audio.export':p=>({path:p.path,paths:['/exports/stems/Keys.wav'],...delivered(p)})});
  await h.ui.exportTo({mode:'stems',path:'/exports/stems',owners:['track:keys'],encoding:'float32',sampleRate:96000});assert.deepEqual(exported(h)[0].payload.owners,['track:keys']);assert.equal(exported(h)[0].payload.sampleRate,96000);assert.equal(exported(h)[0].payload.dither,undefined,'a float file has no dither');
  await h.ui.exportTo({path:'/exports/defaults.wav'});assert.deepEqual([exported(h)[1].payload.encoding,exported(h)[1].payload.sampleRate,exported(h)[1].payload.dither],['pcm24',48000,'tpdf'],'the host\'s defaults');
  await h.ui.exportTo({path:'/exports/plain.wav',encoding:'pcm16',dither:'none'});assert.deepEqual([exported(h)[2].payload.encoding,exported(h)[2].payload.dither],['pcm16','none']);
  for(const unadvertised of [{encoding:'pcm32'},{sampleRate:192000},{dither:'shaped'}]){await assert.rejects(h.ui.exportTo({path:'/exports/test.wav',...unadvertised}),/does not advertise/);assert(!h.ui.isBusy);}
  assert.equal(exported(h).length,3);
  h.overrides['audio.export']=p=>({paths:['/exports/stems/Keys.wav'],...delivered(p),sampleRate:48000});await assert.rejects(h.ui.exportTo({path:'/exports/test.wav',sampleRate:96000}),/sample rate/);assert(!h.ui.isBusy);
  h.overrides['audio.export']=p=>({paths:['/exports/test.wav'],...delivered(p),dither:'none'});await assert.rejects(h.ui.exportTo({path:'/exports/test.wav'}),/dither/);assert(!h.ui.isBusy);
  await assert.rejects(h.ui.exportTo({mode:'stems',path:'/exports/stems',owners:['track:missing']}),/unavailable/);assert(!h.ui.isBusy);
  const failing=harness({'routing.capabilities':()=>{throw new Error('Mixer is rebuilding');}});await assert.rejects(failing.ui.exportTo({path:'/exports/test.wav'}),/Mixer is rebuilding/);assert(!failing.ui.isBusy);assert.equal(exported(failing).length,0);
  const silent=harness({'routing.capabilities':()=>({exportTargets:capabilities().exportTargets})});await assert.rejects(silent.ui.exportTo({path:'/exports/test.wav'}),/did not describe its export formats/);assert.equal(exported(silent).length,0);
});
test('opening advertises graph targets and selects only tracks without requesting an export or editing the project', async () => {
  const h = harness(), before = clone(h.state.project); await h.open(); await h.change('export-mode', 'stems');
  assert.equal(h.state.modal, 'export'); assert.deepEqual(stems(h).map(item => item.dataset.exportOwner), ['track:keys', 'track:bass', 'bus:drums', 'bus:hall', 'master']);
  assert.deepEqual(selected(h), ['track:keys', 'track:bass']); assert(!stems(h).some(item => item.dataset.exportOwner.startsWith('return:')));
  assert.deepEqual(h.calls.map(call => call.command), ['routing.capabilities']); assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.project, before);
});

test('search and arrows navigate filtered stem checkboxes without toggling them and Space stays native', async () => {
  const h = harness(); await h.open(); await h.change('export-mode', 'stems');
  h.get('export-search').value = 'a'; await h.get('export-search').fire('input'); const visible = stems(h);
  assert(visible.length >= 2); const before = selected(h); h.get('export-search').focus(); const enter = await h.get('export-search').fire('keydown', { key: 'ArrowDown' });
  assert(enter.defaultPrevented); assert.equal(h.document.activeElement.dataset.exportOwner, visible[0].dataset.exportOwner); const down = await visible[0].fire('keydown', { key: 'ArrowDown' });
  assert(down.defaultPrevented); assert.equal(h.document.activeElement.dataset.exportOwner, visible[1].dataset.exportOwner); assert.deepEqual(selected(h), before);
  const space = await h.document.activeElement.fire('keydown', { key: ' ' }); assert(!space.defaultPrevented); assert.deepEqual(selected(h), before);
});

test('full graph stems use selected owners after ordered native preflight and confirm the completed file count', async () => {
  const h = harness(), before = clone(h.state.project); await h.open(); await h.change('export-mode', 'stems');
  await check(h, 'track:keys', false); await check(h, 'track:bass', false); await check(h, 'bus:drums', true); await check(h, 'master', true); h.get('export-tail').value = '4.5';
  await h.start();
  assert.deepEqual(h.calls.map(call => call.command), ['routing.capabilities', 'waitForGraphEdit', 'dialog.export', 'waitForGraphEdit', 'captureNativeEdits', 'waitForGraphEdit', 'engine.project', 'audio.export']);
  assert.equal(exported(h).length, 1); const payload = exported(h)[0].payload; assert.equal(payload.mode, 'stems'); assert.equal(payload.tail, 4.5); assert.equal(payload.pattern, 4); assert.deepEqual(payload.owners, ['bus:drums', 'master']);
  assert.equal(resultPath(h), '/exports/stems'); assert.match(h.get('export-progress').textContent, /2 stems exported/); assert.equal(successes(h).length, 1); assert(!h.ui.isBusy); assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.project, before);
});

test('song and pattern exports omit stem owners and use the actual host output path', async () => {
  for (const mode of ['song', 'pattern']) {
    const h = harness({ 'audio.export': payload => ({ path: '/actual/renamed.wav', paths: ['/actual/renamed.wav'], ...delivered(payload) }) }); await h.open(); await h.change('export-mode', mode); await h.start();
    assert.equal(exported(h).length, 1); assert.equal(exported(h)[0].payload.mode, mode); assert.equal(exported(h)[0].payload.owners, undefined); assert(resultPath(h).includes('/actual/renamed.wav')); assert(!resultPath(h).includes('/exports/mix.wav'));
  }
});

test('empty stem selection stops before choosing a destination', async () => {
  const h = harness(); await h.open(); await h.change('export-mode', 'stems'); for (const item of stems(h)) if (item.checked) await check(h, item.dataset.exportOwner, false);
  await h.start(); assert(!h.calls.some(call => call.command === 'dialog.export')); assert.equal(exported(h).length, 0); assert(!h.ui.isBusy); assert.equal(h.mutations.length, 0);
});

test('cancelled chooser, failed graph wait, capture rejection and failed strict sync never render or report success', async () => {
  for (const failure of ['chooser', 'graph', 'capture-false', 'capture-throw', 'sync']) {
    const overrides = failure === 'chooser' ? { 'dialog.export': () => ({ cancelled: true }) } : failure === 'sync' ? { 'engine.project': () => { throw new Error('Strict graph sync failed'); } } : {};
    const h = harness(overrides); await h.open();
    if (failure === 'graph') h.ST.waitForGraphEdit = async () => { throw new Error('Graph edit failed'); };
    if (failure.startsWith('capture')) h.ST.captureNativeEdits = async () => { if (failure === 'capture-throw') throw new Error('Opaque state capture failed'); return false; };
    await h.start(); assert.equal(exported(h).length, 0, failure); assert(!h.ui.isBusy, failure); assert.equal(successes(h).length, 0, failure); assert.equal(h.mutations.length, 0, failure);
  }
});

test('a project replaced after the chooser or during preflight cannot be rendered', async () => {
  for (const phase of ['chooser', 'graph', 'capture', 'sync']) {
    const gate = deferred(), h = harness(phase === 'chooser' ? { 'dialog.export': () => gate.promise } : phase === 'sync' ? { 'engine.project': () => gate.promise } : {}); await h.open();
    if (phase === 'graph') h.ST.waitForGraphEdit = () => gate.promise;
    if (phase === 'capture') h.ST.captureNativeEdits = () => gate.promise;
    const start = h.start(); await h.settle(); h.state.project = { ...clone(h.state.project), name: 'Replacement session' }; gate.resolve(phase === 'chooser' ? { path: '/exports/mix.wav' } : true); await start;
    assert.equal(exported(h).length, 0, phase); assert.equal(successes(h).length, 0, phase); assert(!h.ui.isBusy, phase); assert.equal(h.state.project.name, 'Replacement session');
  }
});

test('busy chooser suppresses duplicate starts and closing without sending transport cancellation', async () => {
  const gate = deferred(), h = harness({ 'dialog.export': () => gate.promise }); await h.open(); const first = h.start(); await h.settle();
  assert(h.ui.isBusy); await h.start(); h.ST.closeModal(); assert.equal(h.state.modal, 'export'); assert.equal(h.calls.filter(call => call.command === 'dialog.export').length, 1);
  await h.ui.cancel(); assert(!h.calls.some(call => call.command === 'transport.stop')); assert(h.ui.isBusy);
  gate.resolve({ cancelled: true }); await first; assert(!h.ui.isBusy); assert.equal(exported(h).length, 0);
});

test('render cancellation sends Stop once and stays busy until the export response settles', async () => {
  for (const lateSuccess of [false, true]) {
    const gate = deferred(), h = harness({ 'audio.export': () => gate.promise }); await h.open(); const start = h.start(); await h.settle(); assert.equal(exported(h).length, 1); assert(h.ui.isBusy);
    await h.ui.cancel(); await h.ui.cancel(); h.ST.closeModal(); assert.equal(h.state.modal, 'export'); assert(h.ui.isBusy); assert.equal(h.calls.filter(call => call.command === 'transport.stop').length, 1);
    gate.resolve(lateSuccess ? { path: '/exports/mix.wav', paths: ['/exports/mix.wav'], ...delivered(exported(h)[0].payload) } : { cancelled: true }); await start;
    assert(!h.ui.isBusy); assert.equal(successes(h).length, lateSuccess ? 1 : 0); assert.equal(h.mutations.length, 0);
    if (lateSuccess) assert.match(h.get('export-progress').textContent, /finished before cancellation/); else assert.match(h.get('export-progress').textContent, /cancel/i);
  }
});

test('missing, malformed or incomplete output file lists never claim export success', async () => {
  const results = [null, {}, { path: '/exports/stems' }, { paths: [] }, { paths: ['/exports/one.wav'] }, { paths: ['/exports/one.wav', ''] }, { paths: ['/exports/same.wav', '/exports/same.wav'] }];
  for (const response of results) {
    const h = harness({ 'audio.export': () => response }); await h.open(); await h.change('export-mode', 'stems'); await h.start();
    assert.equal(exported(h).length, 1); assert(!h.ui.isBusy); assert.equal(successes(h).length, 0, JSON.stringify(response));
  }
});

test('export options that cannot be read keep every range from starting and say why', async () => {
  for (const [cap, message] of [['unavailable', /could not be read: The mixer is being rebuilt\./], [null, /did not describe its export formats/], [{ exportTargets: capabilities().exportTargets }, /did not describe its export formats/],
      [{ ...capabilities(), audioExport: { ...audioExport(), container: 'aiff' } }, /did not describe/], [{ ...capabilities(), audioExport: { ...audioExport(), encodings: ['float32'] } }, /did not describe/],
      [{ ...capabilities(), audioExport: { ...audioExport(), sampleRates: [44100] } }, /did not describe/], [{ ...capabilities(), audioExport: { ...audioExport(), defaultDither: 'shaped' } }, /did not describe/]]) {
    const h = harness({ 'routing.capabilities': () => { if (cap === 'unavailable') throw new Error('The mixer is being rebuilt.'); return cap; } }); await h.open();
    assert.match(h.get('export-progress').textContent, message); assert.match(h.get('export-progress').textContent, /open Export again/);
    for (const mode of ['song', 'pattern', 'stems']) { await h.change('export-mode', mode); assert(h.get('export-start').disabled, mode); await h.start(); }
    for (const id of ['export-encoding', 'export-sample-rate', 'export-dither']) assert(h.get(id).disabled, id);
    assert.equal(stems(h).length, 0); assert(!h.calls.some(call => ['dialog.export', 'engine.project', 'audio.export'].includes(call.command))); assert.equal(successes(h).length, 0); assert(!h.ui.isBusy);
    h.ST.closeModal(); assert.equal(h.state.modal, null);
  }
});

test('malformed target metadata is filtered and an empty advertised graph cannot start stems', async () => {
  const badTargets = [null, {}, { ...capabilities().exportTargets[0], owner: '' }, { ...capabilities().exportTargets[0], name: '' }, { ...capabilities().exportTargets[0], kind: 'unknown' }, { ...capabilities().exportTargets[0], boundary: 'masterOutput' }, { owner: 'master', name: 'Bad master', kind: 'master', boundary: 'postFaderPreParent', available: true }, { owner: 'return:chorus', name: 'Unadvertisable return', kind: 'return', boundary: 'postFaderPreParent', available: true }];
  for (const exportTargets of ['malformed', badTargets]) {
    const h = harness({ 'routing.capabilities': () => ({ ...capabilities(), exportTargets }) }); await h.open(); await h.change('export-mode', 'stems'); await h.start();
    assert.equal(stems(h).length, 0); assert(h.get('export-start').disabled); assert(!h.calls.some(call => call.command === 'dialog.export')); assert.equal(successes(h).length, 0);
    await h.change('export-mode', 'song'); await h.start(); assert.equal(exported(h).length, 1); assert.equal(successes(h).length, 1);
  }
});

test('optional built-in returns are offered only when advertised and unavailable or duplicate targets cannot add extra stems', async () => {
  const cap = capabilities(); cap.exportTargets.push({ owner: 'return:delay', name: 'Built-in delay', kind: 'return', boundary: 'postFaderPreParent', available: true }, { owner: 'return:reverb', name: 'Built-in reverb', kind: 'return', boundary: 'postFaderPreParent', available: false }, { ...cap.exportTargets[0], name: 'Duplicate Keys' });
  const h = harness({ 'routing.capabilities': () => cap }); await h.open(); await h.change('export-mode', 'stems');
  assert.equal(stems(h).filter(item => item.dataset.exportOwner === 'track:keys').length, 1); assert(stem(h, 'return:reverb').disabled); assert.deepEqual(selected(h), ['track:keys', 'track:bass']);
  await check(h, 'return:delay', true); await check(h, 'return:reverb', true); await h.start();
  assert.deepEqual(exported(h)[0].payload.owners, ['track:keys', 'track:bass', 'return:delay']); assert.equal(successes(h).length, 1);
});

test('pending capability discovery keeps range focused and tail drafts usable without allowing export', async () => {
  const gate=deferred(),h=harness({'routing.capabilities':()=>gate.promise}),opening=h.open(); await h.settle();
  assert(!h.get('export-mode').disabled); assert(!h.get('export-tail').disabled);
  assert(h.get('export-start').disabled); assert(h.get('export-encoding').disabled);
  assert.equal(h.document.activeElement.id,'export-mode');
  await h.change('export-mode','stems'); h.get('export-tail').value='4.5'; h.get('export-tail').focus();
  gate.resolve(capabilities()); await opening;
  assert.equal(h.document.activeElement.id,'export-tail'); assert.equal(h.get('export-tail').value,'4.5');
  assert.equal(h.get('export-mode').value,'stems'); assert(!h.get('export-start').disabled);
});

test('a late capability response cannot replace or refocus a newer export dialog', async () => {
  const gate = deferred(), h = harness({ 'routing.capabilities': () => gate.promise }); const opening = h.open(); await h.settle(); assert.equal(h.state.modal, 'export');
  h.ST.closeModal(); assert.equal(h.state.modal, null);
  const fresh = capabilities(); fresh.exportTargets = [fresh.exportTargets[4]]; h.overrides['routing.capabilities'] = () => fresh; await h.open(); h.get('export-tail').focus();
  gate.resolve(capabilities()); assert.equal(await opening, false); assert.deepEqual(stems(h).map(item => item.dataset.exportOwner), ['master']); assert.equal(h.document.activeElement.id, 'export-tail'); assert.equal(h.state.modal, 'export');
});

test('matching progress updates only an active render and cancellation waits for both native replies', async () => {
  const render = deferred(), stop = deferred(), h = harness({ 'audio.export': () => render.promise, 'transport.stop': () => stop.promise }); await h.open();
  h.ui.event('audio.exportProgress', { path: '/exports/mix.wav', progress: .3 }); assert.equal(h.get('export-meter').value, '0');
  const starting = h.start(); await h.settle();
  h.ui.event('audio.exportProgress', { path: '/elsewhere/mix.wav', progress: .9 }); assert.equal(h.get('export-meter').value, 0);
  h.ui.event('audio.exportProgress', { path: '/exports/mix.wav', progress: .65 }); assert.equal(h.get('export-meter').value, .65); assert.match(h.get('export-progress').textContent, /65%/);
  h.ui.event('audio.exportProgress', { path: '/exports/mix.wav', progress: 'invalid' }); assert.equal(h.get('export-meter').value, .65);
  assert.equal(h.ui.cancel(), true); assert.equal(h.ui.cancel(), false); render.resolve({ path: '/exports/mix.wav', paths: ['/exports/mix.wav'], ...delivered(exported(h)[0].payload) }); await h.settle();
  assert(h.ui.isBusy); assert.equal(successes(h).length, 0); assert.equal(h.ui.closed(), false);
  stop.resolve({}); await starting; assert(!h.ui.isBusy); assert.equal(successes(h).length, 1); assert.match(h.get('export-progress').textContent, /finished before cancellation/);
  const completed = h.get('export-progress').textContent; h.ui.event('audio.exportProgress', { progress: .1 }); assert.equal(h.get('export-progress').textContent, completed);
});

test('invalid mode or tail values stop before any destination or native preparation', async () => {
  for (const value of ['', '-1', '31', 'not-a-number', 'Infinity']) {
    const h = harness(); await h.open(); h.get('export-tail').value = value; await h.start(); assert.equal(h.calls.length, 1); assert(!h.ui.isBusy); assert.equal(successes(h).length, 0);
  }
  const h = harness(); await h.open(); h.get('export-mode').value = 'unknown'; await h.start(); assert.equal(h.calls.length, 1); assert.equal(exported(h).length, 0);
});

test('strict synchronization includes freshly captured native state and a late render cannot report success for a replacement project', async () => {
  const gate = deferred(), h = harness({ 'audio.export': () => gate.promise });
  h.state.project.audioPlugins = [{ owner: 'master', kind: 'effect', slot: 0, component: 'fixture', state: 'previous' }];
  h.ST.captureNativeEdits = async () => { h.calls.push({ command: 'captureNativeEdits' }); h.state.project.audioPlugins[0].state = 'fresh editor state'; return true; };
  await h.open(); const rendering = h.start(); await h.settle();
  assert.equal(h.calls.find(call => call.command === 'engine.project').payload.project.audioPlugins[0].state, 'fresh editor state');
  h.state.project = { ...clone(h.state.project), name: 'A different session' }; gate.resolve({ path: '/exports/mix.wav', paths: ['/exports/mix.wav'] }); await rendering;
  assert.equal(successes(h).length, 0); assert.equal(h.mutations.length, 0); assert(!h.ui.isBusy); assert.match(h.get('export-progress').textContent, /project changed/i);
});

test('export builds fresh state after shared synchronization waits and waits for native restoration', async () => {
  const edits = deferred(), applied = deferred(), h = harness({ 'engine.project': () => applied.promise });
  h.state.project.audioPlugins = [{ state: 'before mapped edit' }]; h.ST.beforeEngineSync = () => edits.promise;
  await h.open(); const running = h.start(); await h.settle();
  assert.equal(h.syncs.length, 1); assert.equal(h.syncs[0].options.capture, true, 'Export preparation must pass its own export lock');
  assert(!h.calls.some(call => call.command === 'engine.project')); assert.equal(exported(h).length, 0);
  h.state.project.audioPlugins[0].state = 'settled mapped edit'; edits.resolve(); await h.settle();
  const prepared = h.calls.find(call => call.command === 'engine.project').payload;
  assert.equal(prepared.project.audioPlugins[0].state, 'settled mapped edit'); assert.equal(prepared.awaitPlugins, true);
  assert.equal(exported(h).length, 0, 'Export cannot outrun pending plug-in restoration');
  applied.resolve({}); await running; assert.equal(exported(h).length, 1); assert.equal(successes(h).length, 1);
});

test('canceled synchronization or a replaced export dialog never starts rendering', async () => {
  for (const reason of ['canceled', 'dialog', 'project']) {
    const h = harness(), gate = deferred(); await h.open();
    if (reason === 'canceled') h.ST.syncEngineProject = async () => false;
    else h.ST.beforeEngineSync = () => gate.promise;
    const running = h.start(); await h.settle();
    if (reason === 'dialog') h.state.modal = 'settings';
    if (reason === 'project') h.state.project = clone(h.state.project);
    gate.resolve(); await running;
    assert(!h.calls.some(call => call.command === 'engine.project'), reason); assert.equal(exported(h).length, 0, reason); assert.equal(successes(h).length, 0, reason); assert(!h.ui.isBusy);
  }
});

test('pending graph commits finish before the public export lock and duplicate waiting starts do not choose twice', async () => {
  const h = harness(), gate = deferred(); let waits = 0, blockedMutations = 0;
  // Model app.js's real public guard: native graph completion still needs an undo mutation.
  const mutate = h.ST.mutate.bind(h.ST); h.ST.mutate = (label, callback) => { if (h.ui.isBusy) { blockedMutations++; return; } mutate(label, callback); };
  h.ST.waitForGraphEdit = async () => { h.calls.push({ command: 'waitForGraphEdit' }); if (++waits === 1) await gate.promise; };
  await h.open(); const pending = h.start(); await h.settle(); assert(!h.ui.isBusy); assert(!h.calls.some(call => call.command === 'dialog.export'));
  await h.start(); assert.equal(waits, 1, 'Duplicate Start must not create a second waiting export');
  h.ST.mutate('Finish routing transaction', project => { project.routing.outputs = { 'track:keys': 'bus:drums' }; }); gate.resolve(); await pending;
  assert.equal(blockedMutations, 0); assert.deepEqual(h.mutations, ['Finish routing transaction']); assert.equal(h.calls.filter(call => call.command === 'dialog.export').length, 1); assert.equal(exported(h).length, 1);
  assert.equal(h.calls.find(call => call.command === 'engine.project').payload.project.routing.outputs['track:keys'], 'bus:drums'); assert.equal(successes(h).length, 1);
});

test('closing, replacing or changing the project while awaiting a graph edit invalidates that export start', async () => {
  for (const action of ['close', 'replacement-modal', 'project', 'reopen']) {
    const h = harness(), gate = deferred(); h.ST.waitForGraphEdit = () => gate.promise; await h.open(); const pending = h.start(); await h.settle(); assert(!h.ui.isBusy);
    if (action === 'project') h.state.project = { ...clone(h.state.project), name: 'Replacement project' };
    else { h.ST.closeModal(); if (action === 'replacement-modal') h.state.modal = 'settings'; if (action === 'reopen') { await h.open(); h.get('export-tail').focus(); } }
    gate.resolve(); await pending;
    assert(!h.calls.some(call => call.command === 'dialog.export'), action); assert.equal(exported(h).length, 0, action); assert.equal(successes(h).length, 0, action); assert(!h.ui.isBusy, action);
    if (action === 'replacement-modal') assert.equal(h.state.modal, 'settings');
    if (action === 'reopen') { assert.equal(h.state.modal, 'export'); assert.equal(h.document.activeElement.id, 'export-tail'); }
  }
});

test('rack edits, graph preparation and session changes reject export before locking or choosing and allow retry', async () => {
  for (const [surface, flag] of [['studio', 'isEditingPlugin'], ['studio', 'isPreparingEffectMove'], ['routing', 'isPreparing'], ['ST', 'isChangingSession']]) {
    const h = harness(); await h.open(); h[surface][flag] = true; await h.start();
    assert(!h.ui.isBusy, flag); assert(!h.calls.some(call => call.command === 'dialog.export'), flag); assert.equal(exported(h).length, 0, flag); assert.equal(h.mutations.length, 0, flag);
    assert.match(h.get('export-progress').textContent, /finish|wait|retry|try again/i, flag);
    h[surface][flag] = false; await h.start(); assert.equal(exported(h).length, 1, flag); assert.equal(successes(h).length, 1, flag);
  }
});

test('a save or session transition beginning during the initial graph wait stops export before the chooser', async () => {
  const h = harness(), gate = deferred(); h.ST.waitForGraphEdit = () => gate.promise; await h.open();
  const starting = h.start(); await h.settle(); assert(!h.ui.isBusy); h.ST.isChangingSession = true; gate.resolve(); await starting;
  assert(!h.calls.some(call => call.command === 'dialog.export')); assert.equal(exported(h).length, 0); assert(!h.ui.isBusy); assert.match(h.get('export-progress').textContent, /save or project change/);
  h.ST.isChangingSession = false; await h.start(); assert.equal(exported(h).length, 1); assert.equal(successes(h).length, 1);
});

function formatCapabilities(overrides = {}) {
  return {...capabilities(), audioExport:{...audioExport(), ...overrides}};
}
function deliveredResult(payload) {
  return {path:payload.path, paths:payload.mode === 'stems' ? payload.owners.map((_, i) => `${payload.path}/${i}.wav`) : [payload.path], ...delivered(payload)};
}

test('the host\'s encodings are offered with its default selected and native selector keyboard behavior', async () => {
  const h = harness(); await h.open();
  const select = h.get('export-encoding');
  assert.deepEqual(select.children.map(option => option.value), ['pcm16','pcm24','float32']);
  assert.equal(select.value,'pcm24'); assert(!select.disabled); assert.equal(select.attributes['aria-describedby'],'export-format-note');
  assert.match(select.innerHTML,/PCM 24-bit · default/); assert.equal((select.innerHTML.match(/· default/g) || []).length,1);
  const other = harness({'routing.capabilities':()=>formatCapabilities({encodings:['float32','pcm16'],defaultEncoding:'pcm16'})}); await other.open();
  assert.deepEqual(other.get('export-encoding').children.map(option => option.value), ['pcm16','float32']); assert.equal(other.get('export-encoding').value,'pcm16'); assert.match(other.get('export-encoding').innerHTML,/PCM 16-bit · default/);
  select.focus(); await h.change('export-encoding','float32');
  for (const key of ['ArrowUp','ArrowDown',' ','Enter','Tab']) { const event = await select.fire('keydown',{key}); assert(!event.defaultPrevented,key); }
  await h.change('export-mode','stems'); await h.change('export-tail','7.5');
  assert.equal(h.document.activeElement,select); assert.equal(select.value,'float32'); assert.equal(h.get('export-tail').value,'7.5');
  assert.deepEqual(h.calls.map(call => call.command),['routing.capabilities']); assert.equal(h.mutations.length,0);
});

test('each advertised encoding reaches song pattern and stem export with confirmed metadata and unchanged names', async () => {
  for (const encoding of ['pcm16','pcm24','float32']) for (const mode of ['song','pattern','stems']) {
    const h = harness({'audio.export':deliveredResult}); const before=clone(h.state.project);
    await h.open(); await h.change('export-mode',mode); await h.change('export-encoding',encoding); await h.start();
    assert.equal(exported(h).length,1); const payload=exported(h)[0].payload;
    assert.equal(payload.encoding,encoding); assert.equal(payload.sampleRate,48000); assert.equal(payload.mode,mode);
    assert.equal(payload.path,mode==='stems'?'/exports/stems':'/exports/mix.wav'); assert.equal(successes(h).length,1);
    assert.match(h.get('export-progress').textContent,/WAV.*48 kHz/); assert.equal(h.mutations.length,0); assert.deepEqual(h.state.project,before);
  }
});

test('encodings the page cannot name are not offered and an unadvertised or malformed encoding stops before the chooser', async () => {
  for (const choice of ['pcm32','PCM24',' pcm24','',true,24,'float32']) {
    const h=harness({'routing.capabilities':()=>formatCapabilities({encodings:['pcm24','pcm16','bogus','<script>']})}); await h.open();
    const select=h.get('export-encoding'); assert.deepEqual(select.children.map(option=>option.value),['pcm16','pcm24']);
    select.value=choice; await h.start(); assert(!h.calls.some(call=>call.command==='dialog.export')); assert.equal(exported(h).length,0); assert.equal(successes(h).length,0); assert.match(h.get('export-progress').textContent,/supported/);
  }
});

test('the chosen encoding is immutable while a chooser or graph wait is pending and a canceled chooser preserves its draft', async () => {
  for (const phase of ['graph','chooser']) {
    const gate=deferred(),h=harness({'dialog.export':phase==='chooser'?()=>gate.promise:()=>({path:'/exports/mix.wav'}),'audio.export':deliveredResult});
    if(phase==='graph')h.ST.waitForGraphEdit=()=>gate.promise;
    await h.open(); await h.change('export-encoding','pcm16'); const pending=h.start(); await h.settle(); assert(h.get('export-encoding').disabled);
    h.get('export-encoding').value='float32'; gate.resolve(phase==='chooser'?{path:'/exports/mix.wav'}:undefined); await pending;
    assert.equal(exported(h)[0].payload.encoding,'pcm16'); assert.equal(successes(h).length,1);
  }
  const h=harness({'dialog.export':()=>({cancelled:true})}); await h.open(); await h.change('export-encoding','float32'); h.get('export-tail').value='4.5'; await h.start();
  assert.equal(h.get('export-encoding').value,'float32'); assert.equal(h.get('export-tail').value,'4.5'); assert(!h.get('export-encoding').disabled); assert.equal(exported(h).length,0);
});

test('the host must confirm the actual encoding and rate before success is reported', async () => {
  for (const mismatch of [{encoding:undefined},{encoding:'pcm24'},{sampleRate:44100},{sampleRate:'48000'},{sampleRate:true},{container:'aiff'},{container:undefined}]) {
    const h=harness({'audio.export':payload=>({...deliveredResult(payload),...mismatch})}); await h.open(); await h.change('export-encoding','float32'); await h.start();
    assert.equal(exported(h).length,1); assert.equal(successes(h).length,0); assert.match(h.get('export-progress').textContent,/did not confirm.*encoding/); assert(!h.ui.isBusy);
  }
});

test('progress and cancel leave the captured encoding intact and never turn a canceled render into success', async () => {
  const gate=deferred(),h=harness({'audio.export':()=>gate.promise}); await h.open(); await h.change('export-encoding','float32');
  const pending=h.start();await h.settle();h.ui.event('audio.exportProgress',{path:'/exports/mix.wav',progress:.5});
  assert.equal(h.get('export-meter').value,.5); assert.equal(h.get('export-encoding').value,'float32'); assert(h.get('export-encoding').disabled);
  assert(h.ui.cancel());gate.resolve({cancelled:true});await pending;
  assert.equal(successes(h).length,0); assert.equal(h.calls.filter(call=>call.command==='transport.stop').length,1); assert.equal(h.get('export-encoding').value,'float32');assert(!h.get('export-encoding').disabled);
});

test('the host\'s delivery rates are offered with its default selected and native keyboard selection', async () => {
  const h=harness();await h.open();const rate=h.get('export-sample-rate');
  assert.deepEqual(rate.children.map(option=>option.value),['44100','48000','96000']);assert.equal(rate.value,'48000');assert(!rate.disabled);assert.match(rate.innerHTML,/48 kHz · default/);
  const other=harness({'routing.capabilities':()=>formatCapabilities({sampleRates:[96000,44100],defaultSampleRate:96000})});await other.open();
  assert.deepEqual(other.get('export-sample-rate').children.map(option=>option.value),['44100','96000']);assert.equal(other.get('export-sample-rate').value,'96000');assert.match(other.get('export-sample-rate').innerHTML,/96 kHz · default/);
  assert.equal(rate.attributes['aria-describedby'],'export-format-note');assert.match(h.html,/<span>Sample rate<\/span>/);assert.match(h.html,/id="export-format-note"[^>]*>Stereo WAV delivery · engine and track bounce remain at 48 kHz/);
  rate.focus();for(const key of ['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Enter',' ','Tab'])assert(!(await rate.fire('keydown',{key})).defaultPrevented,key);
  assert.equal(h.document.activeElement,rate);assert.deepEqual(h.calls.map(c=>c.command),['routing.capabilities']);assert.equal(h.mutations.length,0);
});

test('all advertised rates and encodings reach all three export ranges and confirm the selected delivery rate', async () => {
  for(const sampleRate of [44100,48000,96000])for(const encoding of ['pcm16','pcm24','float32'])for(const mode of ['song','pattern','stems']){
    const h=harness({'audio.export':deliveredResult}),before=clone(h.state.project);await h.open();await h.change('export-mode',mode);await h.change('export-encoding',encoding);await h.change('export-sample-rate',String(sampleRate));await h.start();
    assert.equal(exported(h).length,1);assert.equal(exported(h)[0].payload.sampleRate,sampleRate);assert.equal(exported(h)[0].payload.encoding,encoding);assert.equal(successes(h).length,1);assert(h.get('export-progress').textContent.includes(`${sampleRate/1000} kHz`));assert.equal(h.mutations.length,0);assert.deepEqual(h.state.project,before);
  }
});

test('rate options reject malformed numeric capabilities and unadvertised values before destination or preparation', async () => {
  const h=harness({'routing.capabilities':()=>formatCapabilities({sampleRates:[96000,44100,48000,'96000',true,NaN,Infinity,48000.5,null]})});await h.open();assert.deepEqual(h.get('export-sample-rate').children.map(option=>option.value),['44100','48000','96000']);
  for(const value of ['96000','48000.0',' 48000','48e3','0xBB80','48000 ','NaN','Infinity','',true,48000,'192000','44100.5']){
    const h=harness({'routing.capabilities':()=>formatCapabilities({sampleRates:[44100,48000]})});await h.open();h.get('export-sample-rate').value=value;await h.start();
    assert(!h.calls.some(c=>['dialog.export','engine.project','audio.export'].includes(c.command)),String(value));assert.equal(successes(h).length,0);assert.match(h.get('export-progress').textContent,/sample rate advertised/);
  }
});

test('capability arrival and stem searches preserve current focus and format/range drafts', async () => {
  const gate=deferred(),h=harness({'routing.capabilities':()=>gate.promise}),opening=h.open();await h.settle();
  await h.change('export-mode','stems');h.get('export-tail').value='5.5';h.get('export-tail').focus();gate.resolve(capabilities());await opening;
  assert.equal(h.document.activeElement.id,'export-tail');assert.equal(h.get('export-tail').value,'5.5');assert.equal(h.get('export-mode').value,'stems');
  await h.change('export-sample-rate','96000');await h.change('export-encoding','float32');const rate=h.get('export-sample-rate');rate.focus();await h.change('export-mode','pattern');assert.equal(h.document.activeElement,rate);
  await h.change('export-mode','stems');h.get('export-search').focus();h.get('export-search').value='Bass';await h.get('export-search').fire('input');
  assert.equal(h.document.activeElement.id,'export-search');assert.equal(h.get('export-sample-rate'),rate);assert.equal(rate.value,'96000');assert.equal(h.get('export-encoding').value,'float32');assert.equal(h.get('export-tail').value,'5.5');
});

test('rate is captured before a graph wait or chooser and survives canceled destination selection', async () => {
  for(const phase of ['graph','chooser']){
    const gate=deferred(),h=harness({'dialog.export':phase==='chooser'?()=>gate.promise:()=>({path:'/exports/mix.wav'}),'audio.export':deliveredResult});
    if(phase==='graph')h.ST.waitForGraphEdit=()=>gate.promise;await h.open();await h.change('export-sample-rate','44100');const pending=h.start();await h.settle();assert(h.get('export-sample-rate').disabled);
    h.get('export-sample-rate').value='96000';gate.resolve(phase==='chooser'?{path:'/exports/mix.wav'}:undefined);await pending;assert.equal(exported(h)[0].payload.sampleRate,44100);assert.match(h.get('export-progress').textContent,/44\.1 kHz/);assert.equal(successes(h).length,1);
  }
  const h=harness({'dialog.export':()=>({cancelled:true})});await h.open();await h.change('export-sample-rate','96000');await h.start();assert.equal(h.get('export-sample-rate').value,'96000');assert(!h.get('export-sample-rate').disabled);assert.equal(exported(h).length,0);
});

test('wrong or missing delivery-rate acknowledgment never claims success', async () => {
  for(const requested of [44100,96000])for(const actual of [48000,String(requested),undefined,true,null,NaN]){
    const h=harness({'audio.export':payload=>({...deliveredResult(payload),sampleRate:actual})});await h.open();await h.change('export-sample-rate',String(requested));await h.start();
    assert.equal(exported(h).length,1);assert.equal(successes(h).length,0);assert.match(h.get('export-progress').textContent,/did not confirm.*sample rate/);assert(h.get('export-progress').textContent.includes(`${requested/1000} kHz`));assert(!h.ui.isBusy);
  }
});

test('rate-aware progress and cancellation keep the locked draft and cannot report canceled success', async () => {
  const gate=deferred(),h=harness({'audio.export':()=>gate.promise});await h.open();await h.change('export-sample-rate','96000');const pending=h.start();await h.settle();
  h.ui.event('audio.exportProgress',{path:'/exports/mix.wav',progress:.25});assert.match(h.get('export-progress').textContent,/96 kHz.*25%/);assert(h.get('export-sample-rate').disabled);assert.equal(h.get('export-sample-rate').value,'96000');
  assert(h.ui.cancel());gate.resolve({cancelled:true});await pending;assert.equal(successes(h).length,0);assert.equal(h.get('export-sample-rate').value,'96000');assert(!h.get('export-sample-rate').disabled);
});

test('the Dither choice offers the host\'s dithers with its default selected and belongs to the PCM encodings', async () => {
  const h=harness();await h.open();const dither=h.get('export-dither');
  assert.match(h.html,/<span>Dither<\/span>/);assert.deepEqual(dither.children.map(option=>option.value),['tpdf','none']);assert.equal(dither.value,'tpdf');
  assert.match(dither.innerHTML,/>TPDF · default</);assert.match(dither.innerHTML,/>None</);assert.equal(dither.attributes['aria-describedby'],'export-format-note');assert(!dither.disabled);
  await h.change('export-encoding','float32');assert(dither.disabled,'a float file is written as rendered');assert.equal(dither.value,'tpdf');
  await h.change('export-encoding','pcm16');assert(!dither.disabled);
  dither.focus();for(const key of ['ArrowUp','ArrowDown','Enter',' ','Tab'])assert(!(await dither.fire('keydown',{key})).defaultPrevented,key);
  assert.deepEqual(h.calls.map(c=>c.command),['routing.capabilities']);assert.equal(h.mutations.length,0);
  const plain=harness({'routing.capabilities':()=>formatCapabilities({dithers:['none'],defaultDither:'none'})});await plain.open();
  assert.deepEqual(plain.get('export-dither').children.map(option=>option.value),['none']);assert.equal(plain.get('export-dither').value,'none');assert.match(plain.get('export-dither').innerHTML,/>None · default</);
  const pending=deferred(),loading=harness({'routing.capabilities':()=>pending.promise}),opening=loading.open();await loading.settle();assert(loading.get('export-dither').disabled);pending.resolve(capabilities());await opening;assert(!loading.get('export-dither').disabled);
});

test('the chosen dither reaches every export range for the PCM encodings, and a float export sends none', async () => {
  for(const mode of ['song','pattern','stems'])for(const encoding of ['pcm16','pcm24','float32'])for(const choice of ['tpdf','none']){
    const h=harness({'audio.export':deliveredResult}),before=clone(h.state.project);await h.open();await h.change('export-mode',mode);await h.change('export-encoding',encoding);h.get('export-dither').value=choice;await h.start();
    assert.equal(exported(h).length,1);const payload=exported(h)[0].payload,message=h.get('export-progress').textContent;
    assert.equal(payload.dither,encoding==='float32'?undefined:choice,`${mode} ${encoding} ${choice}`);assert.equal(payload.encoding,encoding);assert.equal(successes(h).length,1);
    if(encoding==='float32')assert(!/dither/i.test(message),message);else assert.match(message,choice==='tpdf'?/kHz · TPDF dither\./:/kHz · no dither\./);
    assert.equal(h.mutations.length,0);assert.deepEqual(h.state.project,before);
  }
  const h=harness({'audio.export':deliveredResult});await h.open();await h.start();assert.equal(exported(h)[0].payload.dither,'tpdf','the host default when the field is left alone');
});

test('a dither the host does not offer stops before the chooser, except for a float export that has none', async () => {
  for(const choice of ['shaped','TPDF',' tpdf','',true,null]){
    const h=harness();await h.open();h.get('export-dither').value=choice;await h.start();
    assert(!h.calls.some(c=>['dialog.export','engine.project','audio.export'].includes(c.command)),String(choice));assert.equal(successes(h).length,0);assert.match(h.get('export-progress').textContent,/dither supported/);assert.equal(h.document.activeElement.id,'export-dither');
  }
  const narrowed=harness({'routing.capabilities':()=>formatCapabilities({dithers:['tpdf']})});await narrowed.open();narrowed.get('export-dither').value='none';await narrowed.start();assert.equal(exported(narrowed).length,0);
  const float=harness({'audio.export':deliveredResult});await float.open();await float.change('export-encoding','float32');float.get('export-dither').value='shaped';await float.start();assert.equal(exported(float).length,1);assert.equal(exported(float)[0].payload.dither,undefined);assert.equal(successes(float).length,1);
});

test('the dither is captured before a graph wait or chooser and the host must confirm it', async () => {
  for(const phase of ['graph','chooser']){
    const gate=deferred(),h=harness({'dialog.export':phase==='chooser'?()=>gate.promise:()=>({path:'/exports/mix.wav'}),'audio.export':deliveredResult});
    if(phase==='graph')h.ST.waitForGraphEdit=()=>gate.promise;await h.open();await h.change('export-dither','none');const pending=h.start();await h.settle();assert(h.get('export-dither').disabled);
    h.get('export-dither').value='tpdf';gate.resolve(phase==='chooser'?{path:'/exports/mix.wav'}:undefined);await pending;assert.equal(exported(h)[0].payload.dither,'none');assert.match(h.get('export-progress').textContent,/no dither/);assert.equal(successes(h).length,1);assert(!h.get('export-dither').disabled);
  }
  for(const requested of ['tpdf','none'])for(const actual of [requested==='tpdf'?'none':'tpdf',undefined,null,true,'shaped']){
    const h=harness({'audio.export':payload=>({...deliveredResult(payload),dither:actual})});await h.open();await h.change('export-dither',requested);await h.start();
    assert.equal(exported(h).length,1);assert.equal(successes(h).length,0,`${requested} answered ${actual}`);assert.match(h.get('export-progress').textContent,/did not confirm the requested dither/);assert(!h.ui.isBusy);
  }
  const float=harness({'audio.export':payload=>({...deliveredResult(payload),dither:undefined})});await float.open();await float.change('export-encoding','float32');await float.start();assert.equal(successes(float).length,1,'a float export has no dither to confirm');
});

test('headless stems without chosen channels expect one file per track, group members included, like the host default', async () => {
  const h=harness({'audio.export':payload=>({path:payload.path,paths:['keys','bass','kick'].map(name=>`/exports/stems/${name}.wav`),...delivered(payload)})});
  h.state.project.routing={version:1,mixModel:'standard',buses:[{id:'g0',type:'group',groupID:'k0'}]};
  h.state.project.tracks.push({routingID:'kick',name:'Kick',groupID:'k0',instrument:0});
  h.state.project.groups=[{id:'k0',name:'Kit',busID:'g0'}];
  const result=await h.ui.exportTo({mode:'stems',path:'/exports/stems'});
  assert.equal(result.paths.length,3);assert.equal(exported(h).length,1);assert.equal(exported(h)[0].payload.owners,undefined);
  const flat=harness({'audio.export':payload=>({path:payload.path,paths:['keys','bass'].map(name=>`/exports/stems/${name}.wav`),...delivered(payload)})});
  assert.equal((await flat.ui.exportTo({mode:'stems',path:'/exports/stems'})).paths.length,2);
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) { try { await run(); console.log(`PASS ${name}`); } catch (error) { failures++; console.error(`FAIL ${name}\n${error.stack}`); } }
  if (failures) throw new Error(`${failures} export test group(s) failed`);
  console.log(`Export UI: ${tests.length} regression groups passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
