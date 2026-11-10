'use strict';
// Behavior tests drive the real studio module; native range defaults remain unhandled intentionally.
const assert = require('node:assert/strict');
const web = require('./helpers/web');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const source = web.source('studio.js');
const copy = value => JSON.parse(JSON.stringify(value));
function harness(count = 4) {
  const nodes = new Map(), groups = new Map(), changes = [], requests = [];
  let renders = 0;
  const document = { body: {}, activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => groups.get(selector) || [], querySelector: selector => (groups.get(selector) || [])[0] || null };
  function node(id, dataset = {}, type = '', tag = 'BUTTON') {
    const item = { id, dataset, type, tagName: tag, handlers: {}, value: '.8', scrolls: 0, disabled: false, style: {}, classList: { toggle() {} }, setAttribute() {}, addEventListener(event, callback) { (this.handlers[event] ||= []).push(callback); }, focus() { document.activeElement = this; }, scrollIntoView(options) { this.scrolls++; this.scrollOptions = options; }, closest(selector) { if (selector === '[data-mixer-channel]' || selector === '[data-mixer-owner]') return this.channel || null; if (selector === '.studio-plugin > summary') return this.isSummary ? this : null; return null; }, fire(event, extra = {}) { const e = { key: '', target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; for (const fn of this.handlers[event] || []) fn(e); return e; } };
    nodes.set(id, item); return item;
  }
  const state = { view: 'mixer', track: 0, instrument: 0, keyboardPlay: true, selectedLane: 0, row: 0, project: { activePattern: 1, masterVolume: .7, tracks: Array.from({ length: count }, (_, i) => ({ name: `Track ${i + 1}`, instrument: i, volume: .8, pan: 0, delaySend: .1, reverbSend: .2 })), instruments: Array.from({ length: count }, () => ({ type: 'synth' })), audioPlugins: [], patterns: [{ id: 1, name: 'Pattern', length: 64, automation: [{ track: 0, target: 'volume', name: 'Volume', points: [{ row: 0, value: .8 }] }, { track: 1, target: 'pan', name: 'Pan', points: [{ row: 0, value: 0 }] }] }] } };
  const properties = [], toggles = [], selectButtons = [];
  for (let i = 0; i < count; i++) {
    const channel = node(`channel-${i}`, { mixerChannel: String(i), mixerOwner: `track:${i}` });
    for (const role of ['track', 'mute', 'solo', 'vol', 'pan', 'delay', 'reverb', 'slots']) {
      const prop = { vol: 'volume', pan: 'pan', delay: 'delaySend', reverb: 'reverbSend' }[role];
      const dataset = prop ? { studioProperty: prop, studioOwner: `track:${i}`, mixerRole: role } : ['mute', 'solo'].includes(role) ? { toggleOwner: `track:${i}`, toggleProperty: role, mixerRole: role } : { selectOwner: `track:${i}`, mixerRole: role };
      const control = node(`mix-${role}-${i}`, dataset, prop ? 'range' : '', prop ? 'INPUT' : 'BUTTON'); control.channel = channel;
      if (prop) { properties.push(control); node(control.id + '-out'); }
      else if (role === 'mute' || role === 'solo') toggles.push(control); else selectButtons.push(control);
    }
  }
  const master = node('mix-master', { studioProperty: 'masterVolume', studioOwner: 'master', mixerRole: 'vol' }, 'range', 'INPUT'); master.channel = node('master-channel', { mixerChannel: 'master', mixerOwner: 'master' }); node('mix-master-out'); properties.push(master); const masterTitle = node('mix-track-master', { selectOwner: 'master' }); masterTitle.channel = master.channel;
  node('mixer-channels'); node('mixer-workspace'); node('mixer-current-channel'); node('automation-envelope'); node('automation-search', {}, 'search', 'INPUT');
  groups.set('[data-studio-property]', properties); groups.set('[data-toggle-owner]', toggles); groups.set('[data-select-owner]', selectButtons);
  const laneButtons = [node('automation-lane-0', { selectLane: '0' }), node('automation-lane-1', { selectLane: '1' })]; groups.set('[data-select-lane]', laneButtons);
  const targets = [node('automation-target-volume', { chooseTarget: 'volume', targetName: 'Volume' }), node('automation-target-pan', { chooseTarget: 'pan', targetName: 'Pan' })]; groups.set('[data-choose-target]', targets);
  const ST = { state, escapeHTML: value => String(value), refresh() { renders++; }, toast() {}, mutate(label, callback) { changes.push({ label, before: copy(state.project) }); callback(state.project); }, request(command, payload) { requests.push({ command, payload }); return Promise.resolve([]); }, automate() { throw new Error('Navigation must not create an automation lane'); } };
  const context = vm.createContext({ window: { ST }, document, requestAnimationFrame: fn => fn(), setTimeout: () => 1, clearTimeout() {} }); web.load(context, 'studio.js', { text: source });
  const studio = context.window.STStudio; studio.bindMixer();
  function key(id, key, options = {}) {
    const target = typeof id === 'string' ? nodes.get(id) : id; assert(target, `Missing test node ${id}`);
    const event = target.fire ? target.fire('keydown', { key, ...options }) : { key, target, preventDefault() { this.defaultPrevented = true; }, ...options };
    event.handled = event.stopped || studio.key(event); return event;
  }
  return { studio, state, document, nodes, groups, node, key, changes, requests, context, get renders() { return renders; } };
}
const tests = [];
const test = (name, fn) => tests.push({ name, fn });

// Replace the rendered automation subtree as the app does, so removed controls
// cannot remain focusable merely because the test kept an old object around.
function mountAutomation(h) {
  h.state.view = 'automation';
  const ST = h.context.window.ST;
  const render = () => {
    const focus = h.document.activeElement?.id;
    for (const item of h.nodes.values()) item.isConnected = false;
    h.nodes.clear(); h.groups.clear(); h.document.activeElement = h.document.body;
    const elements = [];
    for (const match of h.studio.renderAutomation().matchAll(/<([a-z][a-z0-9]*)\b([^>]*)>/gi)) {
      const attributes = {}, dataset = {};
      for (const attribute of match[2].matchAll(/([\w-]+)(?:="([^"]*)")?/g)) { attributes[attribute[1]] = attribute[2] || ''; if (attribute[1].startsWith('data-')) dataset[attribute[1].slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = attribute[2] || ''; }
      if (!attributes.id && !Object.keys(dataset).length) continue;
      const item = h.node(attributes.id || `anonymous-${elements.length}`, dataset, attributes.type || '', match[1].toUpperCase());
      item.id = attributes.id || ''; item.value = attributes.value || ''; item.isConnected = true; item.disabled = Object.hasOwn(attributes, 'disabled'); item.hidden = Object.hasOwn(attributes, 'hidden');
      item.focus = function () { if (this.isConnected && !this.disabled) h.document.activeElement = this; };
      elements.push(item);
      for (const name of Object.keys(attributes).filter(name => name.startsWith('data-'))) { const selector = `[${name}]`; h.groups.set(selector, [...(h.groups.get(selector) || []), item]); }
    }
    h.groups.set('[data-point-row],[data-point-value]', elements.filter(item => item.dataset.pointRow !== undefined || item.dataset.pointValue !== undefined));
    h.studio.bindAutomation(); if (focus) h.nodes.get(focus)?.focus();
  };
  ST.refresh = render;
  ST.mutate = (label, callback) => { h.changes.push({ label, before: copy(h.state.project) }); callback(h.state.project); render(); };
  render(); return render;
}

test('Left/Right moves every channel control to the matching role without editing sound', () => {
  for (const role of ['track', 'mute', 'solo', 'vol', 'pan', 'delay', 'reverb', 'slots']) {
    const h = harness(); const before = copy(h.state.project);
    const event = h.key(`mix-${role}-0`, 'ArrowRight');
    assert(event.handled && event.defaultPrevented); assert.equal(h.state.track, 1); assert.equal(h.state.instrument, 1); assert.equal(h.document.activeElement.id, `mix-${role}-1`);
    assert.equal(h.document.activeElement.scrolls, 1); assert.equal(h.document.activeElement.scrollOptions.inline, 'nearest');
    h.key(`mix-${role}-1`, 'ArrowLeft'); assert.equal(h.state.track, 0); assert.equal(h.document.activeElement.id, `mix-${role}-0`);
    assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0); assert.equal(h.requests.length, 0);
  }
});

test('navigation starts from the actual focused strip even when selected state differs', () => {
  const h = harness(); h.state.track = 0; h.key('mix-pan-2', 'ArrowRight');
  assert.equal(h.state.track, 3); assert.equal(h.document.activeElement.id, 'mix-pan-3');
});

test('rapid consecutive arrows move focus synchronously before any animation frame', () => {
  const h = harness(), frames = [];
  h.context.requestAnimationFrame = callback => frames.push(callback);
  h.nodes.get('mix-pan-0').focus();
  h.key(h.document.activeElement, 'ArrowRight');
  h.key(h.document.activeElement, 'ArrowRight');
  assert.equal(h.state.track, 2); assert.equal(h.document.activeElement.id, 'mix-pan-2');
  assert.equal(frames.length, 0); assert.equal(h.changes.length, 0);
});

test('selected strip and body arrows work immediately while keyboard piano is enabled', () => {
  const h = harness(); h.state.track = 1;
  h.key('mixer-channels', 'ArrowRight'); assert.equal(h.state.track, 2); assert.equal(h.document.activeElement.id, 'mix-track-2');
  h.key(h.document.body, 'ArrowLeft'); assert.equal(h.state.track, 1); assert.equal(h.document.activeElement.id, 'mix-track-1');
});

test('bounds clamp, Home/End reach the edge channels, and Master takes the matching control or its header', () => {
  const h = harness(); h.key('mix-track-0', 'ArrowLeft'); assert.equal(h.state.track, 0);
  h.key('mix-track-0', 'End'); assert.equal(h.document.activeElement.id, 'mix-track-master'); assert.equal(h.state.track, 0, 'Master is not a note destination');
  h.key('mix-pan-3', 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-track-master', 'Master has no pan: focus goes to its header');
  h.key('mix-master', 'ArrowRight'); assert.equal(h.document.activeElement.id, 'mix-master');
  h.key('mix-master', 'ArrowLeft'); assert.equal(h.document.activeElement.id, 'mix-vol-3'); assert.equal(h.state.track, 3);
  h.key('mix-track-3', 'Home'); assert.equal(h.state.track, 0); assert.equal(h.document.activeElement.id, 'mix-track-0');
});

test('persistent mixer identity follows Pan/send arrows and Master without altering track state', () => {
  const h = harness(), before = copy(h.state.project), label = h.nodes.get('mixer-current-channel');
  h.key('mix-pan-2', 'ArrowRight'); assert.equal(label.textContent, '04 · Track 4');
  h.key('mix-pan-3', 'ArrowRight'); assert.equal(label.textContent, 'Master'); assert.equal(h.state.track, 3);
  h.key('mix-master', 'ArrowLeft'); assert.equal(label.textContent, '04 · Track 4');
  h.key('mix-reverb-3', 'ArrowLeft'); assert.equal(label.textContent, '03 · Track 3');
  assert.equal(label.title, label.textContent); assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
});

test('focus-only mixer identity distinguishes routed groups, returns and Master', () => {
  const h = harness(), workspace = h.nodes.get('mixer-workspace'), label = h.nodes.get('mixer-current-channel');
  h.state.project.routing = {mixModel:'standard',buses:[{id:'drums',name:'Drums',type:'group'},{id:'space',name:'Space',type:'return'}]};
  const before = copy(h.state.project), track = h.state.track;
  for (const [owner, text] of [['bus:drums','Group · Drums'],['bus:space','Return · Space'],['master','Master']]) {
    const control = h.node('control-' + owner), strip = h.node('strip-' + owner, {mixerChannel:owner,mixerOwner:owner}); control.channel = strip;
    workspace.fire('focusin', {target:control}); assert.equal(label.textContent, text);
  }
  workspace.fire('focusin', {target:h.nodes.get('mix-pan-0')}); assert.equal(label.textContent, '01 · Track 1');
  assert.equal(h.state.track, track); assert.deepEqual(h.state.project, before); assert.equal(h.changes.length, 0);
  h.state.mixerOwner = 'bus:space'; assert(h.studio.renderMixer().includes('>Return · Space</output>'));
});

test('range Up/Down/Home/End stay native and a changed fader still commits normally', () => {
  const h = harness();
  for (const key of ['ArrowUp', 'ArrowDown', 'Home', 'End']) {
    const event = h.key('mix-vol-0', key); assert(!event.handled && !event.defaultPrevented); assert.equal(h.state.track, 0);
  }
  const range = h.nodes.get('mix-vol-0'); range.value = '.91'; range.fire('change');
  assert.equal(h.state.project.tracks[0].volume, .91); assert.equal(h.changes.length, 1); assert.equal(h.document.activeElement.id, 'mix-vol-0');
});

test('text, numeric edits, selects, modifiers and other panes keep normal arrows', () => {
  const h = harness();
  for (const [type, tag] of [['text', 'INPUT'], ['number', 'INPUT'], ['search', 'INPUT'], ['', 'SELECT'], ['', 'TEXTAREA']]) {
    const node = h.node(`custom-${type}-${tag}`, {}, type, tag); node.channel = h.nodes.get('channel-0');
    assert(!h.key(node, 'ArrowRight').handled); assert.equal(h.state.track, 0);
  }
  const editable = h.node('editable'); editable.isContentEditable = true; editable.channel = h.nodes.get('channel-0'); assert(!h.key(editable, 'ArrowLeft').handled);
  for (const modifier of ['metaKey', 'ctrlKey', 'altKey', 'shiftKey']) assert(!h.key('mix-vol-0', 'ArrowRight', { [modifier]: true }).handled);
  const unrelated = h.node('toolbar-button'); assert(!h.key(unrelated, 'ArrowRight').handled);
});

test('Tab enters only selected channel controls while Enter/Space retain button activation', () => {
  const h = harness(); h.state.track = 2; const html = h.studio.renderMixer();
  for (const role of ['track', 'mute', 'solo', 'vol', 'slots']) {
    assert(html.includes(`id="mix-${role}-2" tabindex="0"`)); assert(html.includes(`id="mix-${role}-0" tabindex="-1"`));
  }
  assert(/id="mix-pan-2"[^>]*tabindex="0"/.test(html)); assert(/id="mix-pan-0"[^>]*tabindex="-1"/.test(html));
  assert(!h.key('mix-mute-0', 'Enter').handled); assert(!h.key('mix-solo-0', ' ').handled); assert(!h.key('mix-pan-0', 'Tab').handled);
  h.nodes.get('mix-mute-0').fire('click'); assert.equal(h.state.project.tracks[0].mute, true);
});

test('effect picker and disclosure arrows navigate their own pane without selecting channels', () => {
  const h = harness(); const choices = [h.node('fx-0', { addPlugin: 'a' }), h.node('fx-1', { addPlugin: 'b' })]; h.groups.set('[data-add-plugin]', choices);
  h.key('fx-0', 'ArrowDown'); assert.equal(h.document.activeElement.id, 'fx-1'); h.key('fx-1', 'Home'); assert.equal(h.document.activeElement.id, 'fx-0');
  assert(!h.key('fx-0', 'Enter').handled);
  const summaries = [h.node('summary-0'), h.node('summary-1')]; for (const summary of summaries) { summary.isSummary = true; summary.parentElement = { open: false }; }
  h.groups.set('.studio-plugin > summary', summaries);
  h.key('summary-0', 'ArrowRight'); assert.equal(summaries[0].parentElement.open, true); h.key('summary-0', 'ArrowLeft'); assert.equal(summaries[0].parentElement.open, false);
  h.key('summary-0', 'ArrowDown'); assert.equal(h.document.activeElement.id, 'summary-1'); assert.equal(h.state.track, 0); assert.equal(h.changes.length, 0);
});

test('the automation lane stack uses vertical arrows and Enter, parameter lists use vertical arrows, panes connect without edits', () => {
  const h = harness(); h.state.view = 'automation'; h.studio.bindAutomation();
  h.key('automation-lane-0', 'ArrowDown'); assert.equal(h.state.selectedLane, 1); assert.equal(h.state.track, 1); assert.equal(h.document.activeElement.id, 'automation-lane-1');
  h.key('automation-lane-1', 'ArrowUp'); assert.equal(h.state.selectedLane, 0); assert.equal(h.document.activeElement.id, 'automation-lane-0');
  h.key('automation-lane-0', 'ArrowRight'); assert.equal(h.state.selectedLane, 1, 'left and right still step lanes');
  h.key('automation-lane-1', 'Enter'); assert.equal(h.document.activeElement.id, 'automation-envelope');
  h.key('automation-lane-0', 'ArrowUp'); assert.equal(h.document.activeElement.id, 'automation-target-pan', 'up from the first lane returns to the parameters');
  h.key('automation-target-volume', 'ArrowDown'); assert.equal(h.document.activeElement.id, 'automation-target-pan');
  h.key('automation-target-pan', 'ArrowRight'); assert.equal(h.document.activeElement.id, 'automation-envelope');
  assert.equal(h.changes.length, 0); assert.equal(h.state.project.patterns[0].automation.length, 2);
});

test('actual app keyboard dispatcher delegates mixer arrows before computer-keyboard performance', () => {
  const h = harness();
  web.load(h.context, 'app.js');
  const app = h.context.window.ST; app.state.project = app.demoProject(); app.state.view = 'mixer'; app.state.track = 0; app.state.instrument = 0; app.state.keyboardPlay = true; app.state.modal = null;
  app.refresh = () => {}; // Keep this DOM boundary static while exercising the actual dispatcher + studio module.
  const event = { target: h.nodes.get('mix-pan-0'), key: 'ArrowRight', code: 'ArrowRight', preventDefault() { this.defaultPrevented = true; } };
  app.keyboard(event);
  assert(event.defaultPrevented); assert.equal(app.state.track, 1); assert.equal(h.document.activeElement.id, 'mix-pan-1');
  const up = { target: h.nodes.get('mix-vol-1'), key: 'ArrowUp', code: 'ArrowUp', preventDefault() { this.defaultPrevented = true; } };
  app.keyboard(up); assert(!up.defaultPrevented); assert.equal(app.state.track, 1);
});

test('effect picker forwards the exact selected installation for duplicate VST3 class IDs', async () => {
  const h = harness(), firstPath = '/Users/test/Library/Audio/Plug-Ins/VST3/Echo.vst3', secondPath = '/Library/Audio/Plug-Ins/VST3/Echo.vst3';
  const add = h.node('add-effect', { studioAction: 'add-plugin' });
  h.groups.set('[data-studio-action]', [add]);
  const choices = [firstPath, secondPath].map((bundlePath, i) => h.node(`version-${i}`, { addPlugin: 'vst3:echo', pluginBundlePath: bundlePath }));
  h.groups.set('[data-add-plugin]', choices);
  h.context.window.ST.request = async (command, payload) => {
    h.requests.push({ command, payload });
    if (command === 'plugins.list') return [firstPath, secondPath].map(bundlePath => ({ id: 'vst3:echo', name: 'Echo', type: 'effect', format: 'VST3', bundlePath }));
    return { ...payload, name: 'Echo', parameters: [] };
  };
  h.studio.bindMixer();
  await add.handlers.click[0]();
  const html = h.studio.renderMixer();
  assert(html.includes(`data-plugin-bundle-path="${firstPath}"`) && html.includes(`data-plugin-bundle-path="${secondPath}"`));
  await choices[1].handlers.click[0]();
  const request = h.requests.find(item => item.command === 'plugin.load');
  assert.equal(request.payload.component, 'vst3:echo');
  assert.equal(request.payload.bundlePath, secondPath);
  assert.equal(h.state.project.audioPlugins[0].bundlePath, secondPath);
});

test('deleting the last automation lane focuses parameter search after its controls leave the DOM', () => {
  const h = harness(); h.state.project.patterns[0].automation = [{ track: 2, target: 'volume', name: 'Volume', points: [{ row: 0, value: .5 }] }]; h.studio.selectOwner('track:2'); mountAutomation(h);
  const button = h.nodes.get('automation-delete-lane'); button.focus(); button.fire('click');
  assert.equal(button.isConnected, false); assert(!h.nodes.has('automation-delete-lane')); assert.equal(h.document.activeElement.id, 'automation-search');
  assert.equal(h.state.project.patterns[0].automation.length, 0); assert.equal(h.changes.length, 1); assert.equal(h.changes[0].before.patterns[0].automation.length, 1);
  assert.equal(h.studio.selectedOwner(), 'track:2'); assert.equal(h.state.track, 2); assert.equal(h.state.instrument, 2);
});

test('deleting an automation lane selects and focuses a surviving lane at its new index', () => {
  for (const selected of [0, 1, 2]) {
    const h = harness(); h.state.project.patterns[0].automation = Array.from({ length: 3 }, (_, track) => ({ track, target: 'volume', name: `Lane ${track}`, points: [{ row: 0, value: .5 }] })); h.state.selectedLane = selected; h.studio.selectOwner(`track:${selected}`); mountAutomation(h);
    const button = h.nodes.get('automation-delete-lane'); button.focus(); button.fire('click'); const next = Math.max(0, selected - 1);
    assert.equal(button.isConnected, false); assert.equal(h.state.selectedLane, next); assert.equal(h.document.activeElement.id, `automation-lane-${next}`); assert.equal(h.document.activeElement.isConnected, true);
    assert.equal(h.state.project.patterns[0].automation.length, 2); assert(!h.state.project.patterns[0].automation.some(lane => lane.name === `Lane ${selected}`)); assert.equal(h.changes.length, 1);
    const ownerTrack = h.state.project.patterns[0].automation[next].track;
    assert.equal(h.studio.selectedOwner(), `track:${ownerTrack}`); assert.equal(h.state.track, ownerTrack); assert.equal(h.state.instrument, ownerTrack);
    assert(h.studio.renderAutomation().includes(`value="track:${ownerTrack}" selected`), 'Channel selector must match the surviving visible lane');
  }
});

test('deleting a lane selects surviving bus or Master owners without assigning them as note tracks', () => {
  for (const [removedOwner, remainingOwner] of [['track:0', 'bus:group'], ['bus:group', 'bus:return'], ['bus:return', 'master'], ['master', 'track:2']]) {
    const h = harness(); h.state.project.routing = { version: 1, mixModel: 'standard', buses: [{ id: 'group', type: 'group', name: 'Drums', volume: 1, pan: 0 }, { id: 'return', type: 'return', name: 'Reverb', volume: 1, pan: 0 }], outputs: {}, sends: [], sidechains: [] };
    h.state.project.patterns[0].automation = [removedOwner, remainingOwner].map(owner => ({ owner, target: 'volume', name: owner, points: [{ row: 0, value: .5 }] }));
    h.state.track = 1; h.state.instrument = 1; h.studio.selectOwner(removedOwner);
    const previousTrack = h.state.track, previousInstrument = h.state.instrument, beforeTracks = copy(h.state.project.tracks); mountAutomation(h);
    h.nodes.get('automation-delete-lane').focus(); h.nodes.get('automation-delete-lane').fire('click');
    assert.equal(h.studio.selectedOwner(), remainingOwner); assert.equal(h.state.mixerOwner, remainingOwner); assert.equal(h.state.selectedLane, 0); assert.equal(h.document.activeElement.id, 'automation-lane-0');
    assert.equal(h.state.track, remainingOwner.startsWith('track:') ? 2 : previousTrack); assert.equal(h.state.instrument, remainingOwner.startsWith('track:') ? 2 : previousInstrument);
    assert(h.studio.renderAutomation().includes(`value="${remainingOwner}" selected`)); assert.deepEqual(copy(h.state.project.tracks), beforeTracks);
    assert.equal(h.changes.length, 1); assert.equal(h.changes[0].before.patterns[0].automation.length, 2); assert.equal(h.state.project.patterns[0].automation.length, 1);
  }
});

test('deleting the last bus lane retains its owner and note destination while returning to search', () => {
  const h = harness(); h.state.project.routing = { version: 1, mixModel: 'standard', buses: [{ id: 'group', type: 'group', name: 'Drums', volume: 1, pan: 0 }], sends: [] };
  h.state.project.patterns[0].automation = [{ owner: 'bus:group', target: 'volume', points: [{ row: 0, value: .5 }] }]; h.state.track = 2; h.state.instrument = 2; h.studio.selectOwner('bus:group'); mountAutomation(h);
  h.nodes.get('automation-delete-lane').fire('click');
  assert.equal(h.document.activeElement.id, 'automation-search'); assert.equal(h.studio.selectedOwner(), 'bus:group'); assert.equal(h.state.track, 2); assert.equal(h.state.instrument, 2); assert.equal(h.changes.length, 1);
});

test('Clear returns focus to an enabled Add point control after rebuilding and disabling the old action', () => {
  const h = harness(), beforeOtherLane = copy(h.state.project.patterns[0].automation[1]);
  h.state.project.patterns[0].automation[0].points.push({ row: 8, value: .25 }); mountAutomation(h);
  const button = h.nodes.get('automation-clear'); button.focus(); button.fire('click');
  assert.equal(button.isConnected, false); assert.equal(h.nodes.get('automation-clear').disabled, true); assert.equal(h.document.activeElement.id, 'automation-add-point'); assert.equal(h.document.activeElement.isConnected, true); assert.equal(h.document.activeElement.disabled, false);
  assert.equal(h.state.project.patterns[0].automation[0].points.length, 0); assert.deepEqual(copy(h.state.project.patterns[0].automation[1]), beforeOtherLane);
  assert.equal(h.changes.length, 1); assert.equal(h.changes[0].label, 'Clear automation points'); assert.equal(h.changes[0].before.patterns[0].automation[0].points.length, 2);
});

test('Command Delete from the final numeric breakpoint focuses Add point when its row disappears', () => {
  for (const column of ['row', 'value']) {
    const h = harness(); h.state.project.patterns[0].automation.length = 1; mountAutomation(h);
    const input = h.nodes.get(`point-${column}-0`); input.focus(); const event = input.fire('keydown', { key: 'Delete', metaKey: true });
    assert(event.defaultPrevented); assert.equal(input.isConnected, false); assert(!h.nodes.has('point-value-0')); assert.equal(h.document.activeElement.id, 'automation-add-point');
    assert.equal(h.state.project.patterns[0].automation[0].points.length, 0); assert.equal(h.changes.length, 1); assert.equal(h.changes[0].label, 'Delete automation point');
  }
});

test('activating the last Remove point button keeps focus at Add point instead of the detached button', () => {
  const h = harness(); h.state.project.patterns[0].automation.length = 1; mountAutomation(h);
  const button = h.groups.get('[data-remove-point]')[0]; button.focus(); button.fire('click');
  assert.equal(button.isConnected, false); assert.equal(h.groups.get('[data-remove-point]')?.length || 0, 0); assert.equal(h.document.activeElement.id, 'automation-add-point'); assert.equal(h.changes.length, 1);
});

test('numeric and button breakpoint deletion focus the nearest surviving point after reindexing', () => {
  for (const origin of ['numeric', 'button']) for (const index of [0, 1, 2]) {
    const h = harness(); h.state.project.patterns[0].automation.length = 1; h.state.project.patterns[0].automation[0].points = [{ row: 0, value: .2 }, { row: 8, value: .5 }, { row: 16, value: .8 }]; mountAutomation(h);
    const control = origin === 'numeric' ? h.nodes.get(`point-value-${index}`) : h.groups.get('[data-remove-point]')[index]; control.focus();
    if (origin === 'numeric') control.fire('keydown', { key: 'Backspace', metaKey: true }); else control.fire('click');
    assert.equal(control.isConnected, false); assert.equal(h.document.activeElement.id, `point-value-${Math.min(index, 1)}`); assert.equal(h.document.activeElement.isConnected, true);
    assert.deepEqual(copy(h.state.project.patterns[0].automation[0].points.map(point => point.row)), [0, 8, 16].filter((_, i) => i !== index)); assert.equal(h.changes.length, 1);
  }
});

test('late plug-in catalogue completion and selected-channel focus respect a newer modal', async () => {
  const h = harness(); let resolve; const catalogue = new Promise(done => { resolve = done; });
  const add = h.node('add-effect', { studioAction: 'add-plugin' }); h.node('studio-plugin-search', {}, 'search', 'INPUT'); h.groups.set('[data-studio-action]', [add]);
  h.context.window.ST.request = (command, payload) => { h.requests.push({ command, payload }); return command === 'plugins.list' ? catalogue : Promise.resolve([]); };
  h.studio.bindMixer(); const pending = add.handlers.click[0](); assert.equal(h.document.activeElement.id, 'studio-plugin-search');
  h.state.modal = 'export'; const modalField = h.node('export-tail', {}, 'number', 'INPUT'); modalField.focus();
  resolve([{ id: 'fixture', name: 'Fixture', type: 'effect' }]); await pending; assert.equal(h.document.activeElement, modalField);
  h.studio.focusSelectedChannel(); assert.equal(h.document.activeElement, modalField); assert.equal(h.changes.length, 0);
  h.state.modal = null; h.studio.focusSelectedChannel(); assert.equal(h.document.activeElement.id, 'mix-track-0');
});

(async () => {
  for (const { name, fn } of tests) { await fn(); console.log(`PASS ${name}`); }
  console.log(`Studio navigation: ${tests.length} regression groups passed.`);
})().catch(error => { console.error(error); process.exitCode = 1; });
