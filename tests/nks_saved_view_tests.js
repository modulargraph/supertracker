'use strict';
// Exercise the actual Save/capture path and NKS module against a parsed, replaceable DOM.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const { hostStub } = require('./helpers/host');
const web = require('./helpers/web');
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let n = 0; n < 80; n++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
async function bounded(promise) { let timer; try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(Error('Save-view test did not settle')), 2000); })]); } finally { clearTimeout(timer); } }
const parameters = (value = .73) => [{ address: '17', name: 'Cutoff', normalized: .21, writable: true, automatable: true }, { address: '270544960', name: 'Amount', normalized: value, writable: true, automatable: true }, { address: '99', name: 'Rate', normalized: .46, writable: true, automatable: true }];
class Element {
  constructor(tag, attrs, document) {
    this.tagName = tag.toUpperCase(); this.document = document; this.dataset = {}; this.attributes = {}; this.listeners = {}; this.value = ''; this.disabled = false; this.isConnected = true; this.style = {}; this.innerHTML = ''; this.textContent = ''; this.classList = { toggle() {}, add() {}, remove() {} };
    for (const match of (attrs || '').matchAll(/([\w:-]+)(?:="([^"]*)")?/g)) this.setAttribute(match[1], match[2] ?? '');
  }
  setAttribute(key, value) { this.attributes[key] = String(value); if (key.startsWith('data-')) this.dataset[key.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = String(value); else if (key === 'disabled') this.disabled = true; else this[key] = String(value); }
  removeAttribute(key) { delete this.attributes[key]; if (key.startsWith('data-')) delete this.dataset[key.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())]; else if (key === 'disabled') this.disabled = false; }
  getAttribute(key) { return this.attributes[key] ?? null; }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  fire(type, extra = {}) { const event = { type, target: this, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; for (const fn of this.listeners[type] || []) fn(event); this['on' + type]?.(event); return event; }
  focus() { if (!this.isConnected || this.disabled) return; const old = this.document.activeElement; if (old !== this) old?.fire?.('blur'); this.document.activeElement = this; this.fire('focus'); this.document.fire?.('focusin', { target: this }); }
  select() {} scrollIntoView() {} querySelectorAll() { return []; } querySelector() { return null; }
  click() { if (!this.disabled && this.isConnected) return this.fire('click'); }
  closest(selector) {
    if (selector.includes('input:not')) return ['INPUT', 'TEXTAREA', 'SELECT'].includes(this.tagName) && this.type !== 'range' ? this : null;
    if (selector === 'button') return this.tagName === 'BUTTON' ? this : null;
    if (selector === 'details') { for (let node = this; node; node = node.parentElement) if (node.tagName === 'DETAILS') return node; }
    if (selector === 'button,input[type=checkbox],input[type=radio],summary') { for (let node = this; node; node = node.parentElement) if (['BUTTON', 'SUMMARY'].includes(node.tagName) || node.tagName === 'INPUT' && ['checkbox', 'radio'].includes(node.type)) return node; }
    return null;
  }
}
class Root extends Element {
  constructor(html, document) {
    super('section', html.match(/<section\b([^>]*)>/)?.[1] || '', document); this.nodes = [];
    for (const match of html.matchAll(/<(input|output|select|button|span)\b([^>]*)>([^<]*)/g)) {
      const element = new Element(match[1], match[2], document); element.textContent = match[3];
      if (match[1] === 'select') { const content = html.slice(match.index, html.indexOf('</select>', match.index)); element.value = /<option value="([^"]*)"\s+selected\b/.exec(content)?.[1] ?? ''; }
      this.nodes.push(element);
    }
  }
  querySelectorAll(selector) { const match = /^\[([^=\]]+)(?:="([^"]*)")?\]$/.exec(selector); if (!match) return []; return this.nodes.filter(node => Object.hasOwn(node.attributes, match[1]) && (match[2] === undefined || node.attributes[match[1]] === match[2])); }
  querySelector(selector) { return selector.startsWith('#') ? this.nodes.find(node => node.id === selector.slice(1)) || null : this.querySelectorAll(selector)[0] || null; }
  contains(element) { return element === this || this.nodes.includes(element); }
}
async function harness() {
  const nodes = new Map(), calls = [], handlers = new Map(), timers = new Map(), listeners = new Map(); let sequence = 0, root = null;
  const overviewIDs = new Set(['plugin-overview-help', 'plugin-overview-help-summary', 'browse-plugins', 'plugin-editor', 'plugin-parameters']);
  const document = { activeElement: null, body: null,
    getElementById(id) { return id.startsWith('nks-') ? root?.nodes.find(element => element.id === id) || null : nodes.get(id) || null; },
    querySelector(selector) { if (selector === '.context-menu') return null; if (selector.startsWith('#')) return selector.startsWith('#nks-') || overviewIDs.has(selector.slice(1)) ? this.getElementById(selector.slice(1)) : node(selector.slice(1)); if (selector.startsWith('[data-nks-root')) return root; return node(selector); },
    querySelectorAll(selector) { return selector === '[data-nks-root]' && root ? [root] : []; },
    addEventListener(type, fn) { (listeners.get(type) || (listeners.set(type, []), listeners.get(type))).push(fn); },
    fire(type, event) { for (const fn of listeners.get(type) || []) fn(event); }, createElement(tag) { return new Element(tag, '', document); }
  };
  function node(id, tag = 'button') { if (!nodes.has(id)) nodes.set(id, new Element(tag, `id="${id}"`, document)); return nodes.get(id); }
  document.body = node('body', 'body'); document.activeElement = document.body;
  Object.defineProperty(node('app', 'div'), 'innerHTML', { get() { return this.html || ''; }, set(html) {
    this.html = html; if (root) { const ownedFocus = root.contains(document.activeElement); root.isConnected = false; for (const element of root.nodes) element.isConnected = false; if (ownedFocus) document.activeElement = document.body; }
    for (const id of overviewIDs) { const previous = nodes.get(id); if (previous) { previous.isConnected = false; if (document.activeElement === previous) document.activeElement = document.body; nodes.delete(id); } }
    const overview = [];
    for (const match of html.matchAll(/<(button|details|summary)\b([^>]*)>/g)) { const id = /\bid="([^"]*)"/.exec(match[2])?.[1]; if (overviewIDs.has(id)) { const element = new Element(match[1], match[2], document); element.open = /\bopen(?:\s|$)/.test(match[2]); overview.push({ element, index: match.index }); nodes.set(id, element); } }
    for (const item of overview) { const parent = overview.find(candidate => candidate.element.tagName === 'DETAILS' && item.index > candidate.index && item.index < html.indexOf('</details>', candidate.index)); item.element.parentElement = parent?.element || null; }
    const markup = /<section\b[^>]*data-nks-root="\d+"[\s\S]*?<\/section>/.exec(html)?.[0]; root = markup ? new Root(markup, document) : null;
  } });
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) { calls.push(copy(message)); Promise.resolve().then(() => handlers.get(message.command)?.(copy(message.payload)) ?? {}).then(result => window.STHostTransport.deliver({ id: message.id, ok: true, result }), error => window.STHostTransport.deliver({ id: message.id, ok: false, error: error.message })); } }) };
  const context = vm.createContext({ window, document, console, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, queueMicrotask, innerWidth: 1280, innerHeight: 800, requestAnimationFrame: fn => queueMicrotask(fn), setTimeout(fn, delay) { timers.set(++sequence, { fn, delay }); return sequence; }, clearTimeout(id) { timers.delete(id); } });
  for (const file of ['app.js', 'nks-controls.js']) web.load(context, file, { path: file === 'nks-controls.js' && process.env.SUPERTRACKER_NKS_VIEW_SOURCE });
  const ST = window.ST, project = ST.demoProject(); project.tracks = project.tracks.slice(0, 2); project.tracks[0].routingID = 'track-A'; project.tracks[1].routingID = 'track-B'; project.instruments = project.instruments.slice(0, 2); project.instruments[0].type = 'au';
  const plugin = { kind: 'instrument', slot: 0, owner: 'track:track-A', instanceID: 'nks-save-instance', component: 'vst3:SAVED-VIEW', format: 'VST3', bundlePath: '/Fixture.vst3', name: 'Save fixture', state: 'opaque:initial', nks: { path: '/Fixture.nksf', name: 'Fixture', identity: { vst3ClassID: 'SAVED-VIEW' }, pages: [{ name: 'Sound', parameters: [{ slot: 0, parameterID: 17, mapped: true, name: 'Cutoff' }] }, { name: 'Movement', parameters: [{ slot: 0, parameterID: 270544960, mapped: true, name: 'Amount' }, { slot: 1, parameterID: 99, mapped: true, name: 'Rate' }] }] } };
  project.audioPlugins = [plugin]; project.instruments[0].plugin = copy(plugin); Object.assign(ST.state, { project, track: 0, instrument: 0, view: 'instrument', engineReady: true });
  handlers.set('plugin.parameters', () => parameters()); handlers.set('engine.snapshot', () => ({ audioPlugins: copy(ST.state.project.audioPlugins) }));
  handlers.set('plugin.state', () => ({ ...copy(ST.state.project.audioPlugins[0]), state: 'opaque:captured' })); handlers.set('plugin.parameter', payload => ({ display: String(payload.value) }));
  handlers.set('project.save', payload => ({ saved: true, project: copy(payload.project), path: '/tmp/saved-view.supertracker' }));
  ST.refresh(); await tick(); assert(root, 'The actual instrument renderer must mount the NKS panel');
  const get = id => { const result = document.getElementById(id); assert(result, `Missing ${id}`); return result; };
  return { ST, window, document, plugin, project, handlers, calls, node, get, get root() { return root; },
    knob: (slot = 0, type = 'range') => get(`nks-0-${slot}-${type}`), page: () => get('nks-0-page'),
    async pageTo(index) { const input = get('nks-0-page'); input.value = String(index); input.fire('change'); await tick(); },
    fireTimer(delay) { for (const [id, timer] of [...timers]) if (timer.delay === delay) { timers.delete(id); timer.fn(); } },
    save: () => bounded(ST.projectAction('save'))
  };
}
// Fresh descriptors replace the project's as after a state capture (app-session.js): the mapped controls adopt them first.
const adoptDescriptors=(h,plugins)=>{h.window.STNKSControls.adoptPlugins(plugins);h.ST.state.project.audioPlugins=plugins;h.ST.refresh();};
const tests = [], test = (name, run) => tests.push({ name, run });
const assertPage = (h, page, address) => { assert.equal(h.page().value, String(page)); assert.equal(h.knob().dataset.nksAddress, address); };

test('actual Save preserves page and focused knob identity, using only freshly read values after acknowledgment', async () => {
  const h = await harness(); await h.pageTo(1); h.knob().focus(); const old = h.knob(), owner = h.ST.state.project, read = deferred(); h.handlers.set('plugin.parameters', () => read.promise);
  assert.equal(await h.save(), true); assert.notEqual(h.ST.state.project, owner); assert.notEqual(h.knob(), old); assert(!old.isConnected); assertPage(h, 1, '270544960');
  assert(h.knob().disabled); assert.equal(h.document.activeElement, h.page(), 'A loading knob must leave usable page focus'); assert.equal(h.knob().value, '0', 'An old cached value must not be transferred as a fresh value');
  read.resolve(parameters(.88)); await tick(); assert.equal(h.knob().value, '0.88'); assert(!h.knob().disabled); assert.equal(h.document.activeElement, h.knob());
  assert(!h.calls.some(call => call.command === 'plugin.parameter'), 'Restoring page and focus cannot write parameters'); assert.equal(h.ST.state.undo.length, 0);
});

test('native capture may replace the descriptor while Save preserves its exact page and nonfirst knob focus', async () => {
  const h = await harness(); await h.pageTo(1); h.knob(1).focus(); const original = h.ST.state.project.audioPlugins[0];
  h.handlers.set('engine.snapshot', () => ({ audioPlugins: [{ ...copy(original), state: 'opaque:native-editor-change' }] }));
  const read = deferred(); h.handlers.set('plugin.parameters', () => read.promise); assert.equal(await h.save(), true); assertPage(h, 1, '270544960');
  assert.notEqual(h.ST.state.project.audioPlugins[0], original); assert.equal(h.ST.state.project.audioPlugins[0].state, 'opaque:native-editor-change'); assert.equal(h.document.activeElement, h.page());
  read.resolve(parameters(.62)); await tick(); assert.equal(h.document.activeElement, h.knob(1)); assert.equal(h.knob(1).dataset.nksAddress, '99'); assert.equal(h.knob(1).value, '0.46');
  assert.equal(h.calls.findLast(call => call.command === 'project.save').payload.project.audioPlugins[0].state, 'opaque:native-editor-change');
});

test('a late native read from the old Save owner cannot overwrite the saved owner or its focus', async () => {
  const h = await harness(); await h.pageTo(1); h.knob().focus(); const oldRead = deferred(), newRead = deferred(); let reads = 0;
  h.handlers.set('plugin.parameters', () => ++reads === 1 ? oldRead.promise : newRead.promise); h.fireTimer(750); await tick(); assert.equal(reads, 1);
  assert.equal(await h.save(), true); assert.equal(reads, 2); assertPage(h, 1, '270544960'); oldRead.resolve(parameters(.04)); await tick();
  assert(h.knob().disabled); assert.equal(h.knob().value, '0'); assert.equal(h.document.activeElement, h.page());
  newRead.resolve(parameters(.92)); await tick(); assert.equal(h.knob().value, '0.92'); assert.equal(h.document.activeElement, h.knob());
});

test('delayed post-Save reads do not steal focus after the user moves to another control', async () => {
  for (const destination of ['outside', 'page-change']) {
    const h = await harness(); await h.pageTo(1); h.knob(1).focus(); const read = deferred(); h.handlers.set('plugin.parameters', () => read.promise); assert.equal(await h.save(), true);
    assert.equal(h.document.activeElement, h.page());
    if (destination === 'outside') h.node('instrument-rename').focus(); else { h.page().value = '0'; h.page().fire('change'); }
    const focus = h.document.activeElement; read.resolve(parameters(.65)); await tick(); assert.equal(h.document.activeElement, focus, destination); if (destination === 'page-change') assertPage(h, 0, '17');
  }
});

test('blank numeric drafts stay with the old entry and are not carried through Save', async () => {
  const h = await harness(); await h.pageTo(1); const number = h.knob(0, 'number'); number.focus(); number.value = ''; number.fire('input'); assert.equal(number.value, '');
  const read = deferred(); h.handlers.set('plugin.parameters', () => read.promise); assert.equal(await h.save(), true); assertPage(h, 1, '270544960');
  read.resolve(parameters(.81)); await tick(); assert.equal(h.knob(0, 'number').value, '0.81'); assert.equal(h.document.activeElement, h.knob(0, 'number')); assert(!h.calls.some(call => call.command === 'plugin.parameter')); assert.equal(h.window.STNKSControls.hasPendingEdits, false);
});

test('canceled, capture-blocked and export-blocked Saves keep the existing page, control, and raw draft', async () => {
  for (const reason of ['cancel', 'capture', 'export']) {
    const h = await harness(); await h.pageTo(1); const number = h.knob(0, 'number'); number.focus(); number.value = ''; number.fire('input'); const owner = h.ST.state.project;
    if (reason === 'cancel') h.handlers.set('project.save', () => ({ cancelled: true }));
    if (reason === 'capture') h.handlers.set('engine.snapshot', () => { throw Error('Fixture capture blocked'); });
    if (reason === 'export') h.window.STExport = { isBusy: true };
    assert.equal(await h.save(), false); assert.equal(h.ST.state.project, owner); assert.equal(h.knob(0, 'number'), number); assert.equal(h.document.activeElement, number); assert.equal(number.value, ''); assertPage(h, 1, '270544960');
    if (reason !== 'cancel') assert(!h.calls.some(call => call.command === 'project.save'));
  }
});

test('Save never transfers view state to a different instance, mapping, component, track identity, or ambiguous instance ID', async () => {
  for (const change of ['instance', 'mapping', 'component', 'track', 'duplicate']) {
    const h = await harness(); await h.pageTo(1); h.knob().focus();
    h.handlers.set('project.save', payload => { const next = copy(payload.project); if (change === 'instance') next.audioPlugins[0].instanceID = 'replacement-instance'; if (change === 'component') next.audioPlugins[0].component = 'vst3:REPLACEMENT'; if (change === 'mapping') next.audioPlugins[0].nks.pages[1].parameters[0].parameterID = 99; if (change === 'track') { next.tracks[0].routingID = 'replacement-track'; next.audioPlugins[0].owner = 'track:replacement-track'; } if (change === 'duplicate') next.audioPlugins.push({ ...copy(next.audioPlugins[0]), kind: 'effect' }); return { saved: true, project: next }; });
    assert.equal(await h.save(), true); await tick(); assertPage(h, 0, '17'); assert(!h.calls.some(call => call.command === 'plugin.parameter'));
  }
});

test('a response without an explicit saved acknowledgment cannot adopt the old page', async () => {
  const h = await harness(); await h.pageTo(1); h.knob().focus(); h.handlers.set('project.save', payload => ({ project: copy(payload.project) }));
  await h.save(); await tick(); assertPage(h, 0, '17'); assert(!h.calls.some(call => call.command === 'plugin.parameter'));
});

test('in-place descriptor identity or mapping changes invalidate the old mounted view before Save', async () => {
  for (const change of ['identity', 'mapping']) {
    const h = await harness(); await h.pageTo(1); h.knob().focus(); const plugin = h.ST.state.project.audioPlugins[0];
    if (change === 'identity') plugin.instanceID = 'mutated-instance'; else plugin.nks.pages[1].parameters[0].parameterID = 99;
    assert.equal(await h.save(), true); await tick(); assertPage(h, 0, '17'); assert(!h.calls.some(call => call.command === 'plugin.parameter'));
  }
});

test('an unmounted NKS page survives consecutive actual Saves before returning from Song to Instrument', async () => {
  const h = await harness(); await h.pageTo(1); h.knob(1).focus(); h.ST.setView('song'); assert.equal(h.root, null);
  assert.equal(await h.save(), true); const firstSaved = h.ST.state.project; assert.equal(await h.save(), true); assert.notEqual(h.ST.state.project, firstSaved);
  const readsBeforeReturn = h.calls.filter(call => call.command === 'plugin.parameters').length; h.handlers.set('plugin.parameters', () => parameters(.94));
  h.ST.setView('instrument'); await tick(); assertPage(h, 1, '270544960'); assert.equal(h.knob().value, '0.94'); assert.equal(h.document.activeElement, h.node('instrument-rename'), 'An unmounted view transfers page intent without restoring stale knob focus');
  assert(h.calls.filter(call => call.command === 'plugin.parameters').length > readsBeforeReturn); assert(!h.calls.some(call => call.command === 'plugin.parameter'));
});

test('consecutive unmounted Saves drop page intent if the saved mapping, instance, or track changes', async () => {
  for (const change of ['mapping', 'instance', 'track']) {
    const h = await harness(); await h.pageTo(1); h.ST.setView('song'); assert.equal(await h.save(), true);
    h.handlers.set('project.save', payload => { const next = copy(payload.project); if (change === 'mapping') next.audioPlugins[0].nks.pages[1].parameters[0].parameterID = 99; if (change === 'instance') next.audioPlugins[0].instanceID = 'changed-after-first-save'; if (change === 'track') { next.tracks[0].routingID = 'changed-after-first-save'; next.audioPlugins[0].owner = 'track:changed-after-first-save'; } return { saved: true, project: next }; });
    assert.equal(await h.save(), true); h.ST.setView('instrument'); await tick(); assertPage(h, 0, '17'); assert(!h.calls.some(call => call.command === 'plugin.parameter'));
  }
});

test('native plug-in help summary keeps Space and Enter defaults without playing notes or transport', async () => {
  const h = await harness(), summary = h.get('plugin-overview-help-summary'), help = h.get('plugin-overview-help');
  assert.equal(summary.tagName, 'SUMMARY'); assert.equal(summary.closest('details'), help); assert.equal(help.open, false); summary.focus();
  for (const keyboardPlay of [false, true]) for (const key of [' ', 'Enter']) {
    h.ST.state.keyboardPlay = keyboardPlay; const event = { target: summary, key, code: key === ' ' ? 'Space' : 'Enter', preventDefault() { this.defaultPrevented = true; } };
    h.ST.keyboard(event); assert(!event.defaultPrevented, `${key} must remain a native disclosure action`); assert.equal(h.document.activeElement, summary);
  }
  assert(!h.calls.some(call => call.command.startsWith('note.') || call.command.startsWith('transport.') || call.command === 'step.preview'));
  h.ST.refresh(); assert.notEqual(h.get('plugin-overview-help-summary'), summary); assert.equal(summary.isConnected, false); assert.equal(h.document.activeElement, h.get('plugin-overview-help-summary'), 'The disclosure retains focus after replacing the instrument DOM');
});

test('loaded plug-in editor, parameter list and browser stay usable while help is collapsed', async () => {
  const h = await harness(), help = h.get('plugin-overview-help'); assert.equal(help.open, false);
  for (const id of ['browse-plugins', 'plugin-editor', 'plugin-parameters']) { const button = h.get(id); assert(button.isConnected && !button.disabled); assert.equal(button.closest('details'), null, `${id} must remain available without expanding help`); button.focus(); assert.equal(h.document.activeElement, button); }
  h.get('plugin-editor').click(); await tick(); assert.deepEqual(h.calls.findLast(call => call.command === 'plugin.editor')?.payload, { owner: 'track:track-A', kind: 'instrument', slot: 0 }); assert.equal(help.open, false);
  h.get('plugin-parameters').focus(); h.get('plugin-parameters').click(); await tick(); assert.equal(h.ST.state.modal, 'dialog'); assert(h.node('overlay').innerHTML.includes('plugin-param-list')); assert.equal(h.calls.findLast(call => call.command === 'plugin.parameters').payload.instanceID, 'nks-save-instance'); assert.equal(help.open, false); h.ST.closeModal(); assert.equal(h.document.activeElement, h.get('plugin-parameters'));
  const selected = []; h.window.STBrowser = { render: () => '', bind() {}, focusSearch() {}, search() {}, showPlugins: kind => selected.push(kind) };
  h.get('browse-plugins').click(); await tick(); assert.equal(h.ST.state.view, 'browser'); assert.deepEqual(selected, ['instrument']); assert(!h.calls.some(call => call.command === 'plugin.parameter' || call.command.startsWith('note.')));
});

test('Undo, actual Open, and instance replacement reset page instead of inheriting a Save-only view', async () => {
  for (const action of ['undo', 'open', 'replacement']) {
    const h = await harness(); await h.pageTo(1); h.knob().focus(); assert.equal(await h.save(), true); await tick(); assertPage(h, 1, '270544960');
    if (action === 'undo') { h.ST.mutate('rename fixture', p => { p.name = 'Undo this name'; }); h.node('instrument-rename').focus(); await h.ST.perform('undo'); }
    if (action === 'open') { h.handlers.set('project.open', () => ({ project: copy(h.ST.state.project) })); assert.equal(await bounded(h.ST.projectAction('open')), true); }
    if (action === 'replacement') { const plugins = copy(h.ST.state.project.audioPlugins); plugins[0].instanceID = 'new-instance'; adoptDescriptors(h,plugins); }
    await tick(); assertPage(h, 0, '17');
  }
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log(`PASS ${name}`); } catch (error) { failures++; console.error(`FAIL ${name}\n${error.stack}`); } } console.log(`${tests.length - failures}/${tests.length} NKS saved-view suites passed.`); if (failures) process.exitCode = 1; })();
