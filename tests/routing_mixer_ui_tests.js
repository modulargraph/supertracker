'use strict';
// Exercise the real mixer/automation module with the native boundary and routing editor stubbed.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const { complete, routingTable } = require('./helpers/project');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = web.source('studio.js');
const clone = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const escapeHTML = value => String(value).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const decode = value => value.replace(/&(amp|lt|gt|quot|#39);/g, (_, entity) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" }[entity]));

function fixture() {
  return complete({
    activePattern: 0, masterVolume: .73,
    tracks: [
      { routingID: 'keys', name: 'Keys', instrument: 0, volume: .8, pan: -.1, delaySend: .1, reverbSend: .2 },
      { routingID: 'bass', name: 'Bass', instrument: 1, volume: .6, pan: .1, delaySend: 0, reverbSend: .05 }
    ],
    instruments: [{ type: 'synth' }, { type: 'sample' }],
    routing: {
      // Deliberately not in display order: tracks, groups, returns, Master.
      buses: [{ id: 'verb', name: 'Hall', type: 'return', volume: .5, pan: .25 }, { id: 'drums', name: 'Drum group', type: 'group', volume: 1.1, pan: -.2 }],
      outputs: { 'track:keys': 'bus:drums', 'track:bass': null, 'bus:drums': 'master', 'bus:verb': 'master' },
      sends: [{ id: 'keys-hall', source: 'track:keys', destination: 'bus:verb', tap: 'postFader', gain: .3 }, { id: 'drums-hall', source: 'bus:drums', destination: 'bus:verb', tap: 'preFader', gain: .45 }]
    },
    audioPlugins: [],
    patterns: [{ id: 0, name: 'Pattern A', length: 64, automation: [
      { track: 0, target: 'volume', name: 'Keys level', points: [{ row: 0, value: .8 }] },
      { owner: 'bus:drums', target: 'volume', name: 'Group level', points: [{ row: 0, value: 1.1 }] },
      { owner: 'master', target: 'volume', name: 'Main level', points: [{ row: 0, value: .73 }] }
    ] }, { id: 1, name: 'Pattern B', length: 32, automation: [] }]
  });
}

function effect(owner, slot, suffix = String(slot)) {
  return { owner, kind: 'effect', slot, name: `Effect ${suffix}`, component: `vst3:fx-${suffix}`, bundlePath: '/Library/Audio/Plug-Ins/VST3/Fixture.vst3', instanceID: `${owner}-${suffix}`, state: `state-${suffix}`, bypass: slot === 3, parameters: [{ address: 42, name: `Amount ${suffix}`, normalized: .35 }] };
}

function moveResponse(project, owner, fromSlot, toSlot) {
  const previousPlugins = clone(project.audioPlugins.filter(p => p.owner === owner && p.kind === 'effect').sort((a, b) => a.slot - b.slot));
  const slots = previousPlugins.map(p => p.slot), reordered = previousPlugins.slice();
  reordered.splice(slots.indexOf(toSlot), 0, reordered.splice(slots.indexOf(fromSlot), 1)[0]);
  const plugins = reordered.map((p, i) => ({ ...p, slot: slots[i] }));
  return { moved: true, owner, fromSlot, toSlot, previousPlugins, plugins, slotMap: previousPlugins.map(p => ({ fromSlot: p.slot, toSlot: plugins.find(next => next.instanceID === p.instanceID).slot, instanceID: p.instanceID })) };
}

function harness(setup = () => {}) {
  const state = { project: fixture(), view: 'mixer', track: 0, instrument: 0, selectedLane: 0, keyboardPlay: true, plugins: [] };
  setup(state);
  const nodes = new Map(), calls = [], changes = [], notices = [], routeCalls = [], automationCalls = [], timers = new Map();
  let elements = [], html = '', timerID = 0, studio, refreshes = 0, native;
  const document = { body: { tagName: 'BODY' }, activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => elements.filter(item => matches(item, selector)), querySelector: selector => document.querySelectorAll(selector)[0] || null };
  function matches(item, selector) {
    if (selector === '.studio-plugin > summary') return item.tagName === 'SUMMARY' && matches(item.parentElement, '.studio-plugin');
    if (selector.startsWith('.')) return !!item?.classes?.has(selector.slice(1));
    const attribute = /^\[([^=\]]+)(?:="([^"]*)")?\]$/.exec(selector);
    return attribute ? !!item && Object.hasOwn(item.attributes, attribute[1]) && (attribute[2] === undefined || item.attributes[attribute[1]] === attribute[2]) : false;
  }
  function mountHTML(markup) {
    const oldFocus = document.activeElement?.id;
    nodes.clear(); elements = []; const stack = [];
    for (const token of markup.matchAll(/<\/?[\w:-]+\b[^>]*>/g)) {
      const raw = token[0], tagName = /^<\/?([\w:-]+)/.exec(raw)[1].toUpperCase();
      if (raw.startsWith('</')) { while (stack.length && stack.pop().tagName !== tagName) {} continue; }
      const attributes = {}, dataset = {};
      const attrs = raw.slice(/^<[\w:-]+/.exec(raw)[0].length, -1);
      for (const match of attrs.matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) {
        const name = match[1], value = decode(match[2] ?? match[3] ?? match[4] ?? ''); attributes[name] = value;
        if (name.startsWith('data-')) dataset[name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
      }
      const item = { id: attributes.id || '', tagName, attributes, dataset, parentElement: stack.at(-1) || null, classes: new Set((attributes.class || '').split(/\s+/)), handlers: {}, disabled: Object.hasOwn(attributes, 'disabled'), value: attributes.value || '', type: attributes.type || '', style: {}, textContent: '', scrolls: 0,
        classList: { toggle() {} }, setAttribute(name, value) { this.attributes[name] = String(value); }, addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); },
        focus() { document.activeElement = this; }, scrollIntoView(options) { this.scrolls++; this.scrollOptions = options; }, setSelectionRange() {},
        closest(selector) { for (let item = this; item; item = item.parentElement) if (matches(item, selector)) return item; return null; },
        async fire(type, extra = {}) { const event = { target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; for (const fn of this.handlers[type] || []) await fn(event); return event; }
      };
      elements.push(item); if (item.id) nodes.set(item.id, item);
      if (!['INPUT', 'BR', 'HR', 'IMG', 'META', 'LINK'].includes(tagName) && !raw.endsWith('/>')) stack.push(item);
    }
    document.activeElement = oldFocus && nodes.has(oldFocus) ? nodes.get(oldFocus) : document.body;
  }
  const loaded = new Map();
  function defaultNative(command, payload) {
    const key = `${payload.owner}:${payload.kind}:${payload.slot}`, plugin = loaded.get(key) || state.project.audioPlugins.find(p => p.owner === payload.owner && p.kind === payload.kind && p.slot === payload.slot);
    if (command === 'plugin.parameters') return { parameters: plugin?.parameters || [] };
    if (command === 'plugin.load') { const result = { ...effect(payload.owner, payload.slot, 'loaded'), ...payload }; loaded.set(key, result); return result; }
    if (command === 'plugin.state') return clone(plugin || {});
    if (command === 'plugin.parameter') { const result = { ...plugin, ...(payload.bypass !== undefined ? { bypass: payload.bypass } : {}) }; loaded.set(key, result); return result; }
    if (command === 'plugin.move') return moveResponse(state.project, payload.owner, payload.fromSlot, payload.toSlot);
    if (command === 'plugins.list') return [{ id: 'vst3:fixture', name: 'Fixture effect', type: 'effect', format: 'VST3', bundlePath: '/Library/Audio/Plug-Ins/VST3/Fixture.vst3' }];
    return {};
  }
  native = defaultNative;
  const routing = { isApplying: false, render(owner) { return `<div data-routing-fixture="${owner}">Routing</div>`; }, bind(owner) { routeCalls.push(['bind', owner]); }, bindPluginSidechains(owner) { routeCalls.push(['sidechains', owner]); }, renderPluginSidechain() { return ''; }, refreshCapabilities() { routeCalls.push(['refreshCapabilities']); }, key() { return false; }, showOutput(owner) { routeCalls.push(['output', owner]); }, showAddSend(owner) { routeCalls.push(['send', owner]); }, createBus(type, options) { routeCalls.push(['create', type, clone(options)]); }, waitForApply: async () => {} };
  const ST = { state, escapeHTML, toast(message, kind) { notices.push({ message, kind }); }, captureNativeEdits: async () => { calls.push({ command: 'capture' }); return true; }, syncEngine: async () => { calls.push({ command: 'sync' }); },
    request(command, payload = {}) { calls.push({ command, payload: clone(payload) }); return Promise.resolve().then(() => native(command, payload)); },
    mutate(label, fn) { changes.push({ label, before: clone(state.project) }); fn(state.project); this.refresh(); },
    automate(owner, target, name) { automationCalls.push({ owner, target, name }); },
    refresh() { refreshes++; html = state.view === 'mixer' ? studio.renderMixer() : studio.renderAutomation(); mountHTML(html); if (state.view === 'mixer') studio.bindMixer(); else studio.bindAutomation(); }
  };
  ST.syncEngineProject=async(makeProject,options={})=>{const project=makeProject();return project?ST.request('engine.project',{project,...(options.replace?{replace:true}:{})}):false;};
  const context = vm.createContext({ window: { ST, STRouting: routing }, document, requestAnimationFrame: fn => fn(), setTimeout(fn) { const id = ++timerID; timers.set(id, fn); return id; }, clearTimeout(id) { timers.delete(id); } });
  web.load(context, 'studio.js', { text: source }); studio = context.window.STStudio; ST.refresh();
  async function settle() { for (let i = 0; i < 12; i++) await tick(); }
  function key(id, key, options = {}) {
    const target = typeof id === 'string' ? nodes.get(id) : id; assert(target, `Missing control ${id}`);
    const event = { target, key, preventDefault() { this.defaultPrevented = true; }, ...options }; event.handled = studio.key(event); return event;
  }
  function select(owner) { assert(studio.selectOwner(owner)); ST.refresh(); }
  return { state, ST, studio, routing, document, nodes, calls, changes, notices, routeCalls, automationCalls, timers, key, select, settle, defaultNative, get html() { return html; }, get refreshes() { return refreshes; }, set native(fn) { native = fn; } };
}

const tests = [], test = (name, run) => tests.push({ name, run });

test('standard strips order tracks, groups, returns and Master with owner-aware controls and outputs', () => {
  const h = harness();
  assert.deepEqual(clone(h.studio.channels().map(c => [c.owner, c.type])), [['track:keys', 'track'], ['track:bass', 'track'], ['bus:drums', 'group'], ['bus:verb', 'return'], ['master', 'master']]);
  assert.match(h.html, /data-mixer-owner="bus:drums"[^>]*aria-label="Drum group group channel"/);
  assert.match(h.html, /data-mixer-owner="bus:verb"[^>]*aria-label="Hall return channel"/);
  assert.match(h.html, /id="mix-output-0"[^>]*>[\s\S]*?<strong>Drum group<\/strong>/);
  assert.match(h.html, /id="mix-output-1"[^>]*>[\s\S]*?<strong>No output<\/strong>/);
  assert.equal(h.nodes.get('mix-vol-bus-drums').dataset.studioOwner, 'bus:drums');
  assert.equal(h.nodes.get('mix-master').dataset.studioProperty, 'masterVolume');
  assert.equal(h.nodes.get('mix-pan-bus-verb').dataset.mixerRole, 'pan');
  assert.equal(h.nodes.get('mix-vol-0').attributes.tabindex, '0'); assert.equal(h.nodes.get('mix-vol-bus-drums').attributes.tabindex, '-1');
  assert(!h.nodes.has('mix-pan-master')); assert(!h.nodes.has('mix-output-master'));
});

test('the selected channel detail is a fixed panel under the strips: chain beside routing and returns, nothing below the fold', () => {
  const h = harness();
  // Markup: strips, then one detail row: the insert rack, and a side column holding routing and the returns.
  const order = ['id="mixer-channels"', 'class="studio-bottom studio-bottom-routed"', 'class="panel studio-rack', 'class="studio-side"', 'class="studio-routing-panel"', 'studio-fixed-returns'];
  const at = order.map(token => h.html.indexOf(token));
  assert(at.every(i => i >= 0), `all parts present: ${at}`);
  assert.deepEqual([...at].sort((a, b) => a - b), at, 'in that order');
  const side = h.html.slice(h.html.indexOf('class="studio-side"'));
  assert(side.indexOf('studio-fixed-returns') < side.indexOf('</section></div></div></div>') + 30, 'returns sit in the side column');
  // Styles: the workspace no longer scrolls as a whole; the strips give way to the detail panel, whose columns scroll.
  const css = require('node:fs').readFileSync(require('node:path').join(__dirname, '../ui/studio.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
  const last = selector => { const rules = [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)].filter(m => m[1].split(',').map(x => x.trim()).includes(selector)); return rules.map(m => m[2]).join(';'); };
  const value = (selector, property) => { const found = [...last(selector).matchAll(new RegExp(`(?:^|;)\\s*${property}:([^;]+)`, 'g'))]; return found.at(-1)?.[1].trim(); };
  assert.equal(value('.studio-mixer', 'display'), 'flex'); assert.equal(value('.studio-mixer', 'flex-direction'), 'column'); assert.equal(value('.studio-mixer', 'overflow'), 'hidden');
  assert.equal(value('.studio-mixer>.studio-bottom', 'flex'), '0 0 auto', 'the detail panel keeps its height');
  assert.equal(value('.studio-mixer>.studio-channels', 'flex'), '0 1 auto', 'the strips give way');
  assert.equal(value('.studio-bottom>.studio-rack', 'overflow-y'), 'auto');
  assert.match(value('.studio-channel>.studio-fader-well', 'flex'), /^1 1/, 'faders shorten before the strips scroll');
});

test('Left/Right carries fader focus through every channel without changing the note destination on buses', () => {
  const h = harness(), original = clone(h.state.project);
  const ids = ['mix-vol-0', 'mix-vol-1', 'mix-vol-bus-drums', 'mix-vol-bus-verb', 'mix-master'];
  h.nodes.get(ids[0]).focus();
  for (let i = 1; i < ids.length; i++) {
    const event = h.key(h.document.activeElement, 'ArrowRight'); assert(event.handled && event.defaultPrevented);
    assert.equal(h.document.activeElement.id, ids[i]); assert.equal(h.document.activeElement.scrollOptions.inline, 'nearest');
    assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1);
  }
  assert.equal(h.studio.selectedOwner(), 'master'); h.key(h.document.activeElement, 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-master');
  for (let i = ids.length - 2; i >= 0; i--) { h.key(h.document.activeElement, 'ArrowLeft'); assert.equal(h.document.activeElement.id, ids[i]); }
  assert.equal(h.state.track, 0); assert.equal(h.state.instrument, 0); assert.deepEqual(h.state.project, original); assert.equal(h.changes.length, 0);
});

test('pan and unsupported roles preserve a matching control or fall back to the channel header', () => {
  const h = harness();
  h.key('mix-pan-1', 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-pan-bus-drums');
  h.key(h.document.activeElement, 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-pan-bus-verb');
  h.key(h.document.activeElement, 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-track-master');
  h.key('mix-delay-1', 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-track-bus-drums');
  h.key('mix-track-0', 'End'); assert.equal(h.studio.selectedOwner(), 'master'); assert.equal(h.document.activeElement.id, 'mix-track-master');
  h.key(h.document.activeElement, 'Home'); assert.equal(h.studio.selectedOwner(), 'track:keys');
  for (const key of ['ArrowUp', 'ArrowDown', 'Home', 'End']) assert(!h.key('mix-vol-bus-drums', key).handled);
});

test('mixer surface navigation starts at the selected bus and a removed selection falls back to its valid track', () => {
  const h = harness(); h.state.track = 1; h.state.instrument = 1; h.select('bus:drums');
  h.key(h.document.body, 'ArrowRight'); assert.equal(h.studio.selectedOwner(), 'bus:verb'); assert.equal(h.document.activeElement.id, 'mix-track-bus-verb');
  h.key('mixer-channels', 'ArrowRight'); assert.equal(h.studio.selectedOwner(), 'master'); assert.equal(h.document.activeElement.id, 'mix-track-master');
  h.select('bus:drums'); h.state.project.routing.buses = h.state.project.routing.buses.filter(bus => bus.id !== 'drums'); h.ST.refresh();
  assert.equal(h.studio.selectedOwner(), 'track:bass'); assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1);
  h.key('mixer-workspace', 'ArrowLeft'); assert.equal(h.studio.selectedOwner(), 'track:keys'); assert.equal(h.state.track, 0); assert.equal(h.state.instrument, 0);
});

test('bus selection keeps tracker track and instrument valid and routes output/send actions to that bus', async () => {
  const h = harness(); h.state.track = 1; h.state.instrument = 1; h.ST.refresh();
  await h.nodes.get('mix-track-bus-drums').fire('click');
  assert.equal(h.studio.selectedOwner(), 'bus:drums'); assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1);
  assert.equal(h.nodes.get('mix-vol-bus-drums').attributes.tabindex, '0'); assert.equal(h.nodes.get('mix-vol-1').attributes.tabindex, '-1');
  await h.nodes.get('mix-output-bus-drums').fire('click'); await h.nodes.get('mix-sends-bus-drums').fire('click');
  assert(h.routeCalls.some(c => c[0] === 'output' && c[1] === 'bus:drums')); assert(h.routeCalls.some(c => c[0] === 'send' && c[1] === 'bus:drums'));
  assert.equal(h.changes.length, 0);
});

test('bus level, pan, mute and solo each modify only the intended bus with one undo snapshot', async () => {
  for (const [role, property, value] of [['vol', 'volume', .64], ['pan', 'pan', -.73], ['mute', 'mute', true], ['solo', 'solo', true]]) {
    const h = harness(), before = clone(h.state.project); h.select('bus:drums'); const control = h.nodes.get(`mix-${role}-bus-drums`);
    if (typeof value === 'number') { control.value = String(value); await control.fire('change'); } else await control.fire('click');
    const expected = clone(before); expected.routing.buses.find(b => b.id === 'drums')[property] = value;
    assert.deepEqual(h.state.project, expected); assert.equal(h.changes.length, 1); assert.deepEqual(h.changes[0].before, before);
    assert.equal(h.document.activeElement.id, `mix-${role}-bus-drums`); assert.equal(h.state.track, 0);
  }
});

test('Master fader changes project masterVolume and keeps unrelated bus and track gain intact', async () => {
  const h = harness(), before = clone(h.state.project); h.select('master'); const master = h.nodes.get('mix-master'); master.value = '.42'; await master.fire('change');
  const expected = clone(before); expected.masterVolume = .42; assert.deepEqual(h.state.project, expected); assert.equal(h.changes.length, 1);
});

test('bus preview uses a copied project and commits no undo before change', async () => {
  const h = harness(), before = clone(h.state.project); h.select('bus:verb'); const pan = h.nodes.get('mix-pan-bus-verb'); pan.value = '-.6'; await pan.fire('input');
  await h.timers.values().next().value(); await h.settle();
  const preview = h.calls.find(c => c.command === 'engine.project'); assert(preview); assert.equal(preview.payload.project.routing.buses[0].pan, -.6);
  assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
});

test('automation descriptors expose channel, Master, sends and owner-specific plug-in parameters', () => {
  const h = harness(state => { state.project.audioPlugins = [effect('bus:drums', 3, 'group'), effect('master', 0, 'master')]; });
  assert.equal(h.studio.target('bus:drums', 'volume').max, 2); assert.equal(h.studio.target('bus:drums', 'pan').min, -1);
  assert.equal(h.studio.target('master', 'volume').name, 'Master volume'); assert.equal(h.studio.target('master', 'pan'), undefined);
  const send = h.studio.target('bus:drums', 'send:drums-hall'); assert.equal(send.group, 'Pre-fader sends'); assert.equal(send.value, .45); assert.equal(send.name, 'Send to Hall');
  assert.equal(h.studio.target('track:keys', 'send:keys-hall').group, 'Post-fader sends'); assert.equal(h.studio.target('track:bass', 'send:keys-hall'), undefined);
  assert.equal(h.studio.target('bus:drums', 'plugin:effect:3:42').name, 'Amount group'); assert.equal(h.studio.target('master', 'plugin:effect:0:42').name, 'Amount master');
  assert.equal(h.studio.target('track:keys', 'plugin:effect:3:42'), undefined); assert.equal(h.studio.target('bus:drums', 'cutoff'), undefined);
  h.select('bus:drums');
  const parameter = h.nodes.get('plugin-3-0'); assert.equal(parameter.dataset.owner, 'bus:drums'); assert.equal(parameter.dataset.automate, 'plugin:effect:3:42'); assert.equal(parameter.dataset.track, undefined);
  assert.equal(h.nodes.get('mix-vol-bus-drums').dataset.owner, 'bus:drums'); assert.equal(h.nodes.get('mix-master').dataset.owner, 'master');
});

test('automation selection lists channels, puts existing owner lanes first and creates no duplicate lane', async () => {
  const h = harness(); h.state.view = 'automation'; h.select('bus:drums');
  assert.match(h.html, /<option value="bus:drums" selected>Group · Drum group<\/option>/); assert.match(h.html, /<option value="master" >Out · Master<\/option>/);
  assert(h.html.indexOf('Already automated') < h.html.indexOf('Available parameters')); assert(!h.nodes.has('automation-target-cutoff'));
  assert(h.nodes.has('automation-target-pan')); assert(h.nodes.has('automation-target-send:drums-hall')); assert(!h.nodes.has('automation-target-send:keys-hall'));
  const count = h.state.project.patterns[0].automation.length; await h.nodes.get('automation-target-volume').fire('click');
  assert.equal(h.state.selectedLane, 1); assert.equal(h.automationCalls.length, 0); assert.equal(h.state.project.patterns[0].automation.length, count);
  await h.nodes.get('automation-target-pan').fire('click'); assert.deepEqual(h.automationCalls, [{ owner: 'bus:drums', target: 'pan', name: 'Channel pan' }]);
  assert.equal(h.state.track, 0); assert.equal(h.state.instrument, 0);
});

test('automation lane navigation selects bus and Master owners without assigning them as note tracks', async () => {
  const h = harness(); h.state.track = 1; h.state.instrument = 1; h.state.view = 'automation'; h.ST.refresh();
  await h.nodes.get('automation-lane-1').fire('click'); assert.equal(h.studio.selectedOwner(), 'bus:drums'); assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1);
  await h.nodes.get('automation-lane-1').fire('keydown', { key: 'ArrowRight' }); assert.equal(h.studio.selectedOwner(), 'master'); assert.equal(h.state.selectedLane, 2); assert.equal(h.document.activeElement.id, 'automation-lane-2');
  assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1); assert.equal(h.changes.length, 0);
  h.state.automationQuery = 'Master'; h.ST.refresh(); assert(h.nodes.has('automation-target-volume')); assert(!h.nodes.has('automation-target-pan'));
});

async function openEffectPicker(h, owner) {
  h.select(owner); const button = h.document.querySelectorAll('[data-studio-action]').find(b => b.dataset.studioAction === 'add-plugin'); await button.fire('click');
  const choice = h.document.querySelector('[data-add-plugin]'); assert(choice); return choice;
}

test('bus and Master plug-in load/state requests carry owner and commit exact captured state once', async () => {
  for (const owner of ['bus:drums', 'master', 'track:keys']) {
    const h = harness(); const choice = await openEffectPicker(h, owner); await choice.fire('click'); await h.settle();
    const requests = h.calls.filter(c => ['plugin.load', 'plugin.state'].includes(c.command)); assert.equal(requests.length, 2);
    for (const request of requests) { assert.equal(request.payload.owner, owner); assert.equal(request.payload.track, undefined); assert.equal(request.payload.kind, 'effect'); }
    assert.equal(requests[0].payload.bundlePath, '/Library/Audio/Plug-Ins/VST3/Fixture.vst3'); assert.equal(h.changes.length, 1);
    assert.equal(h.state.project.audioPlugins[0].owner, owner); assert.equal(h.state.project.audioPlugins[0].state, 'state-loaded'); assert.equal(h.state.project.audioPlugins[0].instanceID, `${owner}-loaded`);
  }
});

test('a channel holds sixteen inserts: the fifteenth free slot loads, then the rack reports 16/16 and stops adding', async () => {
  for (const owner of ['bus:drums', 'master', 'track:keys']) {
    const h = harness(state => { state.project.audioPlugins = Array.from({ length: 15 }, (_, slot) => effect(owner, slot, 'old' + slot)); });
    h.select(owner);
    const add = () => h.document.querySelectorAll('[data-studio-action]').find(b => b.dataset.studioAction === 'add-plugin');
    assert.equal(add().disabled, false, `${owner}: fifteen inserts leave a free slot`);
    assert.match(h.html, /15\/16/); assert.match(h.html, /studio-rack is-long/);
    const choice = await openEffectPicker(h, owner); await choice.fire('click'); await h.settle();
    const load = h.calls.find(c => c.command === 'plugin.load'); assert.equal(load.payload.slot, 15, `${owner}: the new effect takes slot 16`);
    assert.equal(h.state.project.audioPlugins.filter(p => p.owner === owner && p.kind === 'effect').length, 16);
    h.ST.refresh(); assert.equal(add().disabled, true, `${owner}: a full rack cannot add`); assert.match(h.html, /16\/16/);
    assert(h.nodes.has('plugin-header-15') && h.nodes.has('plugin-header-8'), 'Slots 9...16 render in the rack');
  }
});

test('failed state capture creates no successful plug-in edit or undo entry', async () => {
  const h = harness(), before = clone(h.state.project); h.native = (command, payload) => { if (command === 'plugin.state') throw new Error('Fixture cannot capture opaque state'); return h.defaultNative(command, payload); };
  await (await openEffectPicker(h, 'bus:drums')).fire('click'); await h.settle();
  assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0); assert(h.notices.some(n => /cannot capture opaque state/.test(n.message))); assert.equal(h.studio.isEditingPlugin, false);
});

test('a late bus plug-in load cannot attach after that bus is removed or replaced', async () => {
  for (const remove of [true, false]) {
    const h = harness(), gate = deferred(); h.native = (command, payload) => command === 'plugin.load' ? gate.promise : h.defaultNative(command, payload);
    const choice = await openEffectPicker(h, 'bus:drums'), pending = choice.fire('click'); await tick();
    if (remove) h.state.project.routing.buses = h.state.project.routing.buses.filter(b => b.id !== 'drums'); else h.state.project.routing.buses[1] = { ...h.state.project.routing.buses[1], name: 'Replacement' };
    gate.resolve(effect('bus:drums', 0)); await pending;
    assert.equal(h.state.project.audioPlugins.length, 0); assert.equal(h.changes.length, 0); assert(h.notices.some(n => /channel changed/.test(n.message)));
  }
});

test('bus reorder sends owner, preserves other chains and remaps only that owner in every pattern in one undo', async () => {
  const h = harness(state => {
    state.project.audioPlugins = [effect('bus:drums', 0, 'eq'), effect('bus:drums', 3, 'comp'), effect('master', 0, 'limit'), effect('track:keys', 0, 'keys')];
    state.project.patterns[0].automation.push({ owner: 'bus:drums', target: 'plugin:effect:0:42', name: 'Group EQ', points: [{ row: 0, value: .3 }] }, { owner: 'master', target: 'plugin:effect:0:42', points: [] });
    state.project.patterns[1].automation.push({ owner: 'bus:drums', target: 'plugin:effect:3:42', points: [{ row: 6, value: .7 }] }, { owner: 'track:keys', target: 'plugin:effect:0:42', points: [] });
  });
  h.select('bus:drums'); await h.settle(); const before = clone(h.state.project); h.nodes.get('plugin-header-3').focus();
  const event = h.key('plugin-header-3', 'ArrowUp', { altKey: true }); assert(event.handled && event.defaultPrevented); await h.settle();
  const move = h.calls.find(c => c.command === 'plugin.move'); assert.deepEqual(move.payload, { owner: 'bus:drums', fromSlot: 3, toSlot: 0 }); assert.equal(h.changes.length, 1);
  assert.deepEqual(clone(h.state.project.audioPlugins.filter(p => p.owner === 'bus:drums').map(p => [p.slot, p.component, p.state, p.bypass])), [[0, 'vst3:fx-comp', 'state-comp', true], [3, 'vst3:fx-eq', 'state-eq', false]]);
  assert.deepEqual(clone(h.state.project.audioPlugins.filter(p => p.owner !== 'bus:drums')), before.audioPlugins.filter(p => p.owner !== 'bus:drums'));
  assert.equal(h.state.project.patterns[0].automation[3].target, 'plugin:effect:3:42'); assert.equal(h.state.project.patterns[1].automation[0].target, 'plugin:effect:0:42');
  assert.equal(h.state.project.patterns[0].automation[4].target, 'plugin:effect:0:42'); assert.equal(h.state.project.patterns[1].automation[1].target, 'plugin:effect:0:42');
  assert.deepEqual(h.changes[0].before.patterns, before.patterns); assert.equal(h.document.activeElement.id, 'plugin-header-0'); assert.equal(h.state.track, 0); assert.equal(h.studio.selectedOwner(), 'bus:drums');
});

test('return and Master effect reorders target their own chains without coercing owners into track indices', async () => {
  for (const owner of ['bus:verb', 'master']) {
    const h = harness(state => {
      state.project.audioPlugins = [effect(owner, 1, 'first'), effect(owner, 6, 'second'), effect('track:keys', 1, 'untouched')];
      state.project.patterns[0].automation.push({ owner, target: 'plugin:effect:1:42', points: [{ row: 8, value: .5 }] });
    });
    h.select(owner); await h.settle(); h.nodes.get('plugin-move-down-1').focus(); await h.nodes.get('plugin-move-down-1').fire('click'); await h.settle();
    assert.deepEqual(h.calls.find(c => c.command === 'plugin.move').payload, { owner, fromSlot: 1, toSlot: 6 });
    assert.equal(h.changes.length, 1); assert.equal(h.state.project.patterns[0].automation.at(-1).target, 'plugin:effect:6:42');
    assert.equal(h.state.project.audioPlugins.find(p => p.owner === 'track:keys').instanceID, 'track:keys-untouched');
    assert.equal(h.document.activeElement.id, 'plugin-header-6'); assert.equal(h.studio.selectedOwner(), owner); assert.equal(h.state.track, 0); assert.equal(h.state.instrument, 0);
  }
});

test('routing changes in flight suppress native insert edits until the channel graph is ready', async () => {
  const h = harness(state => { state.project.audioPlugins = [effect('bus:drums', 0), effect('bus:drums', 3)]; });
  h.select('bus:drums'); await h.settle(); const before = clone(h.state.project), count = h.calls.length; h.routing.isApplying = true;
  await h.nodes.get('plugin-bypass-0').fire('click'); await h.nodes.get('plugin-remove-0').fire('click'); await h.nodes.get('plugin-0-0').fire('change');
  h.key('plugin-header-3', 'ArrowUp', { altKey: true }); await h.settle();
  assert.equal(h.calls.length, count); assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
});

test('effect removal captures state and atomically cleans exact instance routes and owner slot lanes with complete undo', async () => {
  for (const owner of ['bus:drums', 'track:keys']) {
    const removedID = `${owner}-removed`;
    const h = harness(state => {
      state.project.audioPlugins = [effect(owner, 3, 'removed'), effect(owner, 0, 'kept'), effect('bus:verb', 3, 'return'), effect('track:bass', 3, 'bass'), { ...effect('track:keys', 0, 'instrument'), kind: 'instrument' }];
      const route = (id, instanceID, source = 'track:bass') => ({ id, source, tap: 'postFader', gain: 1, enabled: true, destination: instanceID });
      state.project.routing.sidechains = [route('removed-key', removedID), route('second-removed-key', removedID, 'bus:verb'), route('same-source-keep', `${owner}-kept`), route('prefix-keep', `${removedID}-other`), route('other-owner-keep', 'bus:verb-return', owner)];
      const lane = (name, target, laneOwner = owner) => ({ owner: laneOwner, name, target, points: [{ row: 4, value: .4 }] });
      state.project.patterns[0].automation.push(lane('remove-modern', 'plugin:effect:3:42'), lane('keep-neighbor-slot', 'plugin:effect:30:42'), lane('keep-instrument', 'plugin:instrument:3:42'), lane('keep-other-bus', 'plugin:effect:3:42', 'bus:verb'));
      state.project.patterns[1].automation.push(lane('remove-other-parameter', 'plugin:effect:3:99'), lane('keep-slot-zero', 'plugin:effect:0:42'), lane('keep-other-track', 'plugin:effect:3:42', 'track:bass'));
    });
    h.select(owner); await h.settle(); let captured;
    h.ST.captureNativeEdits = async () => { h.calls.push({ command: 'capture' }); h.state.project.audioPlugins.find(p => p.instanceID === removedID).state = 'fresh opaque editor state'; captured = clone(h.state.project); return true; };
    h.native = (command, payload) => { if (command === 'plugin.remove') { assert(captured, 'Capture must complete before native removal'); assert.deepEqual(h.state.project, captured); return { removed: true }; } return h.defaultNative(command, payload); };
    await h.nodes.get('plugin-remove-3').fire('click'); await h.settle();
    assert.deepEqual(h.calls.filter(c => ['capture', 'plugin.remove'].includes(c.command)).map(c => c.command), ['capture', 'plugin.remove']);
    assert.deepEqual(h.calls.find(c => c.command === 'plugin.remove').payload, { owner, kind: 'effect', slot: 3 });
    const expected = clone(captured); expected.audioPlugins = expected.audioPlugins.filter(p => p.instanceID !== removedID); expected.routing.sidechains = expected.routing.sidechains.filter(r => !['removed-key', 'second-removed-key'].includes(r.id));
    for (const pattern of expected.patterns) pattern.automation = pattern.automation.filter(lane => !lane.name?.startsWith('remove-'));
    assert.deepEqual(h.state.project, expected); assert.equal(h.changes.length, 1); assert.equal(h.changes[0].label, 'Remove plug-in effect'); assert.deepEqual(h.changes[0].before, captured);
    assert.equal(h.changes[0].before.audioPlugins.find(p => p.instanceID === removedID).state, 'fresh opaque editor state');
    assert(h.routeCalls.some(call => call[0] === 'refreshCapabilities')); assert.equal(h.notices.length, 0); assert.equal(h.studio.isEditingPlugin, false);
  }
});

test('failed capture blocks removal and an in-flight removal rejects overlapping rack mutations', async () => {
  for (const failure of ['false', 'throw']) {
    const h = harness(state => { state.project.audioPlugins = [effect('bus:drums', 0), effect('bus:drums', 3)]; }); h.select('bus:drums'); await h.settle(); const before = clone(h.state.project);
    h.ST.captureNativeEdits = async () => { if (failure === 'throw') throw new Error('Cannot capture native state'); return false; };
    await h.nodes.get('plugin-remove-3').fire('click');
    assert(!h.calls.some(c => c.command === 'plugin.remove')); assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0); assert.equal(h.studio.isEditingPlugin, false);
    if (failure === 'throw') assert(h.notices.some(n => /Cannot capture native state/.test(n.message)));
  }
  const h = harness(state => { state.project.audioPlugins = [effect('bus:drums', 0), effect('bus:drums', 3)]; });
  const choice = await openEffectPicker(h, 'bus:drums'); await h.settle(); const gate = deferred();
  h.native = (command, payload) => command === 'plugin.remove' ? gate.promise : h.defaultNative(command, payload);
  const removing = h.nodes.get('plugin-remove-3').fire('click'); await tick(); assert(h.studio.isEditingPlugin);
  await h.nodes.get('plugin-remove-0').fire('click'); await h.nodes.get('plugin-bypass-0').fire('click'); await h.nodes.get('plugin-0-0').fire('change'); await choice.fire('click');
  h.key('plugin-header-3', 'ArrowUp', { altKey: true }); await tick();
  const edits = h.calls.filter(c => ['plugin.remove', 'plugin.parameter', 'plugin.load', 'plugin.move'].includes(c.command)); assert.deepEqual(edits.map(c => c.command), ['plugin.remove']); assert.equal(h.changes.length, 0);
  gate.resolve({ removed: true }); await removing; assert.equal(h.changes.length, 1); assert.equal(h.studio.isEditingPlugin, false);
});

(async () => {
  let failures = 0;
  for (const { name, run } of tests) { try { await run(); console.log(`PASS ${name}`); } catch (error) { failures++; console.error(`FAIL ${name}\n${error.stack}`); } }
  if (failures) throw new Error(`${failures} routing mixer test group(s) failed`);
  console.log(`Routing mixer: ${tests.length} regression groups passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
