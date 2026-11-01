'use strict';
// Insert chain (insert-chain.js) and the public slot API (window.STInserts, studio.js) on the real app modules:
// add from the categorized list, select, knob turns and reset, keyboard, drag reorder, bypass, duplicate and
// remove, each one Undo step. Only the native message transport and a small DOM are stand-ins.
const assert = require('node:assert/strict'), vm = require('node:vm');
const web = require('./helpers/web');
const { metadata, parameterList } = require('./helpers/builtin-effects');
const clone = value => JSON.parse(JSON.stringify(value));
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const tick = () => new Promise(resolve => setImmediate(resolve));

// ---- A small DOM: enough HTML parsing and selector matching for the chain's own markup -------------------------
const decode = value => value.replace(/&(amp|lt|gt|quot|#39);/g, (_, e) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" }[e]));
const VOID = new Set(['INPUT', 'BR', 'IMG', 'PATH', 'CIRCLE', 'LINE']);
function compound(node, part) {
  if (!node || !node.attributes) return false;
  for (const token of part.match(/#[\w-]+|\.[\w-]+|\[[^\]]+\]|^[a-z][\w-]*/gi) || []) {
    if (token[0] === '#') { if (node.id !== token.slice(1)) return false; }
    else if (token[0] === '.') { if (!node.classes.has(token.slice(1))) return false; }
    else if (token[0] === '[') { const [, name, value] = /^\[([^=\]]+)(?:="([^"]*)")?\]$/.exec(token); if (!Object.hasOwn(node.attributes, name) || (value !== undefined && node.attributes[name] !== value)) return false; }
    else if (node.tagName !== token.toUpperCase()) return false;
  }
  return true;
}
function matches(node, selector) {
  const parts = selector.trim().split(/\s*(>)\s*|\s+/).filter(Boolean);
  const walk = (item, index) => {
    if (!compound(item, parts[index])) return false;
    if (index === 0) return true;
    if (parts[index - 1] === '>') return walk(item.parentElement, index - 2);
    for (let parent = item.parentElement; parent; parent = parent.parentElement) if (walk(parent, index - 1)) return true;
    return false;
  };
  return walk(node, parts.length - 1);
}
function makeDocument() {
  let all = [];
  const stubs = new Map();
  const stub = id => { if (!stubs.has(id)) stubs.set(id, { id, innerHTML: '', value: '', dataset: {}, style: {}, classList: { toggle() {}, add() {}, remove() {} }, focus() { doc.activeElement = this; }, select() {}, scrollIntoView() {}, closest() { return null; }, querySelector() { return null; }, querySelectorAll() { return []; }, addEventListener() {}, removeEventListener() {}, setAttribute() {}, getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 20 }), isConnected: false }); return stubs.get(id); };
  const doc = { activeElement: null, body: stub('body'), readyState: 'complete', addEventListener() {}, removeEventListener() {}, createElement: tag => stub('new-' + tag + Math.random()),
    getElementById: id => all.find(node => node.id === id) || stub(id),
    querySelector: selector => all.find(node => matches(node, selector)) || (/^#[\w-]+$/.test(selector) ? stub(selector.slice(1)) : null),
    querySelectorAll: selector => all.filter(node => matches(node, selector)),
    mount(markup) {
      for (const node of all) node.isConnected = false;
      all = []; const stack = [];
      for (const token of markup.matchAll(/<\/?[\w:-]+\b[^>]*>|[^<]+/g)) {
        const raw = token[0];
        if (!raw.startsWith('<')) { if (stack.length) stack.at(-1).textContent += decode(raw); continue; }
        const tagName = /^<\/?([\w:-]+)/.exec(raw)[1].toUpperCase();
        if (raw.startsWith('</')) { while (stack.length && stack.pop().tagName !== tagName) {} continue; }
        const attributes = {}, dataset = {};
        for (const m of raw.slice(/^<[\w:-]+/.exec(raw)[0].length, -1).matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'))?/g)) {
          const value = decode(m[2] ?? m[3] ?? ''); attributes[m[1]] = value;
          if (m[1].startsWith('data-')) dataset[m[1].slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
        }
        const node = { id: attributes.id || '', tagName, attributes, dataset, parentElement: stack.at(-1) || null, classes: new Set((attributes.class || '').split(/\s+/).filter(Boolean)), handlers: {}, isConnected: true, ownerDocument: doc,
          textContent: '', value: attributes.value || '', disabled: Object.hasOwn(attributes, 'disabled'), style: {}, selectionStart: 0,
          get open() { return Object.hasOwn(this.attributes, 'open'); },
          classList: { add: (...c) => c.forEach(x => node.classes.add(x)), remove: (...c) => c.forEach(x => node.classes.delete(x)), contains: c => node.classes.has(c), toggle() {} },
          setAttribute(name, value) { this.attributes[name] = String(value); }, getAttribute(name) { return this.attributes[name] ?? null; },
          addEventListener(type, fn) { (this.handlers[type] ||= []).push(fn); }, removeEventListener(type, fn) { this.handlers[type] = (this.handlers[type] || []).filter(x => x !== fn); },
          focus() { doc.activeElement = this; }, setPointerCapture() {}, setSelectionRange() {}, scrollIntoView() {},
          closest(selector) { for (let item = this; item; item = item.parentElement) if (matches(item, selector)) return item; return null; },
          querySelector(selector) { return all.find(item => item !== this && matches(item, selector) && contains(this, item)) || null; },
          querySelectorAll(selector) { return all.filter(item => item !== this && matches(item, selector) && contains(this, item)); },
          click() { return this.fire('click'); },
          fire(type, extra = {}) { const event = { type, target: this, button: 0, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; let result; for (const fn of [...(this.handlers[type] || [])]) result = fn(event); return Promise.resolve(result).then(() => event); },
          dispatchEvent(event) { return this.fire(event.type, event); } };
        if (stack.length) stack.at(-1).textContent;
        all.push(node);
        if (!VOID.has(tagName) && !raw.endsWith('/>')) stack.push(node);
      }
    } };
  const contains = (parent, item) => { for (let p = item.parentElement; p; p = p.parentElement) if (p === parent) return true; return false; };
  return doc;
}

// ---- The app with a fake audio host --------------------------------------------------------------------------------
function harness() {
  const document = makeDocument(), calls = [], timers = new Map();
  let clock = 0, nextTimer = 1, nextInstance = 1, nativePlugins = [];
  const project = {
    name: 'Chain', activePattern: 0, masterVolume: .7,
    tracks: [{ routingID: 'keys', name: 'Keys', instrument: 0, volume: .8, pan: 0 }],
    instruments: [{ type: 'synth' }],
    routing: { mixModel: 'standard', buses: [], outputs: { 'track:keys': 'master' }, sends: [], sidechains: [] },
    audioPlugins: [],
    patterns: [{ id: 0, name: 'A', length: 16, automation: [] }]
  };
  const descriptorFor = payload => nativePlugins.find(p => p.owner === payload.owner && p.kind === payload.kind && p.slot === payload.slot);
  const handlers = {
    'plugin.load': p => {
      const meta = metadata().find(effect => effect.id === p.component);
      const descriptor = { owner: p.owner, kind: p.kind, slot: p.slot, component: p.component, name: meta?.name || p.component, format: 'Builtin', manufacturer: 'SuperTracker', instanceID: `fx-${nextInstance++}`, bypass: !!p.bypass, state: p.state || 'fresh', parameters: parameterList(p.component) };
      nativePlugins = nativePlugins.filter(x => !(x.owner === p.owner && x.slot === p.slot)).concat(descriptor);
      return clone(descriptor);
    },
    'plugin.state': p => { const d = p.owner ? descriptorFor(p) : null; return d ? { component: d.component, instanceID: d.instanceID, state: d.state, bypass: d.bypass, parameters: d.parameters } : nativePlugins.map(clone); },
    'plugin.parameters': p => ({ parameters: clone(descriptorFor(p)?.parameters || []) }),
    'plugin.parameter': p => {
      const d = descriptorFor(p); if (!d) throw new Error('There is no Audio Unit in this slot.');
      if (p.bypass !== undefined) { d.bypass = p.bypass; const { parameters, ...rest } = d; return clone(rest); }
      const parameter = d.parameters.find(x => x.address === String(p.address)); parameter.normalized = parameter.value = p.value; d.state = `edited-${p.address}-${p.value}`; return { normalized: p.value };
    },
    'plugin.remove': p => { nativePlugins = nativePlugins.filter(x => !(x.owner === p.owner && x.slot === p.slot)); return {}; },
    'plugin.move': p => {
      const before = nativePlugins.filter(x => x.owner === p.owner && x.kind === 'effect').sort((a, b) => a.slot - b.slot).map(x => { const { parameters, ...rest } = x; return clone(rest); });
      const slots = before.map(x => x.slot), order = before.slice(); order.splice(slots.indexOf(p.toSlot), 0, order.splice(slots.indexOf(p.fromSlot), 1)[0]);
      const after = order.map((x, i) => ({ ...x, slot: slots[i] }));
      for (const x of after) nativePlugins.find(n => n.instanceID === x.instanceID).slot = x.slot;
      return { moved: true, owner: p.owner, fromSlot: p.fromSlot, toSlot: p.toSlot, previousPlugins: before, plugins: after, slotMap: before.map(x => ({ fromSlot: x.slot, toSlot: after.find(n => n.instanceID === x.instanceID).slot, instanceID: x.instanceID })) };
    },
    'engine.project': p => { const wanted = new Set((p.project.audioPlugins || []).map(x => x.instanceID)); nativePlugins = nativePlugins.filter(x => wanted.has(x.instanceID)); for (const x of p.project.audioPlugins || []) { const n = nativePlugins.find(y => y.instanceID === x.instanceID); if (n) { n.slot = x.slot; n.bypass = !!x.bypass; } else nativePlugins.push({ ...clone(x), parameters: parameterList(x.component) }); } return {}; },
    'engine.snapshot': () => ({ audioPlugins: nativePlugins.map(x => { const { parameters, ...rest } = x; return clone(rest); }) }),
    'builtin.effects': () => metadata(),
    'plugins.list': () => metadata().map(e => ({ id: e.id, name: e.name, type: 'effect', category: e.category, format: 'Builtin' })).concat([{ id: 'aufx:dely:appl', name: 'AUDelay', manufacturer: 'Apple', type: 'effect', format: 'AU' }])
  };
  const window = { addEventListener() {}, removeEventListener() {}, localStorage: { getItem() { return null; }, setItem() {} }, location: { search: '' }, webkit: { messageHandlers: { supertracker: { postMessage(message) {
    const request = clone(message); calls.push(request);
    Promise.resolve().then(() => (handlers[request.command] || (() => ({})))(request.payload || {}))
      .then(result => window.nativeReceive({ id: request.id, ok: true, result }), error => window.nativeReceive({ id: request.id, ok: false, error: error.message }));
  } } } } };
  const context = vm.createContext({ window, document, console, Date, Map, Set, WeakMap, WeakSet, Math, JSON, Number, String, Object, Array, Promise, Error, innerWidth: 1400, innerHeight: 900,
    setTimeout: (fn, delay = 0) => { const id = nextTimer++; timers.set(id, { fn, at: clock + Number(delay || 0) }); return id; }, clearTimeout: id => timers.delete(id), setInterval: () => 0, clearInterval() {},
    requestAnimationFrame: fn => fn(), KeyboardEvent: class { constructor(type, init) { Object.assign(this, init, { type }); } }, Event: class { constructor(type) { this.type = type; } } });
  context.globalThis = window;
  web.load(context, 'app.js');
  for (const file of ['fine-control.js', 'effect-pages.js', 'routing-project.js', 'routing.js', 'studio.js', 'plugin-pages.js', 'insert-chain.js']) web.load(context, file);
  const ST = window.ST;
  ST.state.project = ST.normalize(project); ST.state.view = 'pattern'; ST.state.engineReady = true; ST.state.native = true;
  window.STEffectPages.setMetadata(metadata());
  const owner = 'track:keys', options = { prefix: 'fx' };
  const mount = () => { document.mount(window.STInsertChain.render(owner, options)); window.STInsertChain.bind(document, owner, options); };
  const realRefresh = ST.refresh; ST.refresh = () => { realRefresh(); mount(); };
  const advance = ms => { clock += ms; for (const [id, timer] of [...timers].sort((a, b) => a[1].at - b[1].at)) if (timer.at <= clock) { timers.delete(id); timer.fn(); } };
  const settle = async () => { for (let i = 0; i < 40; i++) { await tick(); advance(50); } await tick(); mount(); };
  const chain = () => window.STInserts.chain(owner).map(p => `${p.name}${p.bypass ? ' (off)' : ''}`);
  const undo = async () => { await ST.perform('undo'); await settle(); };
  mount();
  return { window, ST, document, calls, owner, mount, settle, chain, undo, el: id => document.getElementById(id), native: () => nativePlugins };
}

const tests = [], test = (name, fn) => tests.push({ name, fn });

test('empty chain offers + with the categorized built-in list; choosing adds the effect as one Undo step', async () => {
  const h = harness();
  assert.match(h.document.querySelector('.fx-empty').textContent, /No insert effects yet/);
  await h.el('fx-add').click(); await h.settle();
  const columns = h.document.querySelectorAll('.fx-add-column').map(column => column.querySelector('h5').textContent);
  same(columns, ['Dynamics', 'Filter & EQ', 'Modulation', 'Reverb & Space', 'Delay', 'Distortion', 'Perform', 'Plug-ins']);
  assert(h.document.querySelector('[data-fx-choose="aufx:dely:appl"]'), 'installed plug-in effects are listed too');
  // Search narrows; Return adds the first match.
  const search = h.el('fx-add-search'); search.value = 'filt'; await search.fire('input');
  same(h.document.querySelectorAll('[data-fx-choose]').map(node => node.dataset.fxChoose), ['stfx:filter', 'stfx:performfilter']);
  await h.el('fx-add-search').fire('keydown', { key: 'Enter' }); await h.settle();
  same(h.chain(), ['Multimode Filter']);
  assert.equal(h.ST.state.undo.at(-1).label, 'Add plug-in effect');
  assert.equal(h.ST.state.undo.length, 1);
  assert(!h.document.querySelector('.fx-add-panel'), 'the list closes after adding');
  assert.equal(h.ST.state.project.audioPlugins[0].parameters, undefined, 'the stored descriptor carries no parameter cache');
  await h.undo(); same(h.chain(), []);
});

test('the selected module shows curated pages as eight labelled knobs; turning, fine drag and reset are single Undo steps', async () => {
  const h = harness();
  await h.window.STInserts.add(h.owner, 'stfx:compressor'); await h.window.STInserts.add(h.owner, 'stfx:filter'); await h.settle();
  await h.el('fx-tile-1').click(); await h.settle();
  assert.equal(h.document.querySelector('.fx-tile.is-selected').id, 'fx-tile-1');
  same(h.document.querySelectorAll('[data-fx-page]').map(tab => tab.textContent), ['Main', 'LFO', 'Envelope', 'Sidechain']);
  same(h.document.querySelectorAll('.fx-knob-name').map(node => node.textContent).slice(0, 6), ['Cutoff', 'Reso', 'Mode', 'Mod amount', 'Mod source', 'Mix']);
  same(h.document.querySelectorAll('.fx-section').map(node => node.textContent.trim()).filter(Boolean), ['Filter', 'Modulation', 'Output']);
  assert.equal(h.document.querySelector('[data-fx-value="0"]').textContent, '16 kHz');
  const undo = h.ST.state.undo.length;
  // Vertical drag: 160 px is the whole range; with Shift ten times finer.
  const dial = h.el('fx-knob-0');
  await dial.fire('pointerdown', { clientY: 200, pointerId: 1 });
  await dial.fire('pointermove', { clientY: 216 });          // −10 %
  await dial.fire('pointermove', { clientY: 232, shiftKey: true }); // −1 % more
  await dial.fire('pointerup', {});
  await h.settle();
  const cutoff = h.native().find(p => p.component === 'stfx:filter').parameters[1];
  assert(Math.abs(cutoff.normalized - (parameterList('stfx:filter')[1].normalized - .11)) < 1e-6, `fine drag reached ${cutoff.normalized}`);
  assert.equal(h.ST.state.undo.length, undo + 1); assert.equal(h.ST.state.undo.at(-1).label, 'Adjust Cutoff');
  assert.notEqual(h.document.querySelector('[data-fx-value="0"]').textContent, '16 kHz');
  // Double-click resets to the default.
  await h.el('fx-knob-0').fire('dblclick'); await h.settle();
  assert.equal(h.document.querySelector('[data-fx-value="0"]').textContent, '16 kHz');
  assert.equal(h.ST.state.undo.length, undo + 2);
  // Stepped knob: one arrow is one choice.
  await h.el('fx-knob-2').fire('keydown', { key: 'ArrowUp' }); await h.settle();
  assert.equal(h.document.querySelector('[data-fx-value="2"]').textContent, 'Band-pass');
  // Page tabs.
  await h.el('fx-page-1').click(); await h.settle();
  same(h.document.querySelectorAll('.fx-knob-name').map(node => node.textContent).slice(0, 4), ['Rate', 'Sync rate', 'LFO shape', 'LFO phase']);
  await h.el('fx-knob-0').fire('keydown', { key: 'ArrowRight', shiftKey: true }); await h.settle();
  assert.equal(h.document.querySelector('[data-fx-page].is-on').textContent, 'Envelope', '⇧ → turns the page from a knob');
});

test('keyboard on tiles: arrows select, ⌥ arrows reorder, B bypasses, D duplicates, ⌫ removes; Undo restores each', async () => {
  const h = harness();
  for (const id of ['stfx:compressor', 'stfx:filter', 'stfx:beatdelay']) await h.window.STInserts.add(h.owner, id);
  await h.settle();
  await h.el('fx-tile-0').fire('keydown', { key: 'ArrowRight' }); await h.settle();
  assert.equal(h.document.querySelector('.fx-tile.is-selected').id, 'fx-tile-1'); assert.equal(h.document.activeElement.id, 'fx-tile-1');
  await h.el('fx-tile-1').fire('keydown', { key: 'ArrowLeft', altKey: true }); await h.settle();
  same(h.chain(), ['Multimode Filter', 'Compressor', 'Beat Delay']); assert.equal(h.ST.state.undo.at(-1).label, 'Reorder insert effect');
  assert.equal(h.document.activeElement.id, 'fx-tile-0', 'focus follows the moved module');
  await h.el('fx-tile-0').fire('keydown', { key: 'b' }); await h.settle();
  same(h.chain(), ['Multimode Filter (off)', 'Compressor', 'Beat Delay']); assert(h.el('fx-tile-0').classes.has('is-bypassed'));
  assert.equal(h.el('fx-tile-0').querySelector('[data-fx-power]').attributes['aria-pressed'], 'false');
  await h.el('fx-tile-0').fire('keydown', { key: 'd' }); await h.settle();
  same(h.chain(), ['Multimode Filter (off)', 'Multimode Filter (off)', 'Compressor', 'Beat Delay'], 'the copy lands right after its source, bypass included');
  assert.equal(h.ST.state.undo.at(-1).label, 'Duplicate insert effect');
  const copy = h.window.STInserts.chain(h.owner)[1];
  assert.equal(h.native().find(p => p.instanceID === copy.instanceID).state, h.native().find(p => p.instanceID === h.window.STInserts.chain(h.owner)[0].instanceID).state, 'the copy loads the exact state');
  await h.el('fx-tile-' + copy.slot).fire('keydown', { key: 'Backspace' }); await h.settle();
  same(h.chain(), ['Multimode Filter (off)', 'Compressor', 'Beat Delay']); assert.equal(h.ST.state.undo.at(-1).label, 'Remove plug-in effect');
  await h.undo(); same(h.chain(), ['Multimode Filter (off)', 'Multimode Filter (off)', 'Compressor', 'Beat Delay']);
  await h.undo(); same(h.chain(), ['Multimode Filter (off)', 'Compressor', 'Beat Delay'], 'duplicate is one Undo step');
  await h.undo(); same(h.chain(), ['Multimode Filter', 'Compressor', 'Beat Delay']);
  await h.undo(); same(h.chain(), ['Compressor', 'Multimode Filter', 'Beat Delay']);
});

test('Beat Delay shows Time with its Unit ("4 × 1/16") and follows Unit changes', async () => {
  const h = harness();
  await h.window.STInserts.add(h.owner, 'stfx:beatdelay'); await h.settle();
  await h.el('fx-tile-0').click(); await h.settle();
  same(h.document.querySelectorAll('.fx-knob-name').map(node => node.textContent).slice(0, 2), ['Time', 'Unit']);
  assert.equal(h.document.querySelector('[data-fx-value="0"]').textContent, '4 × 1/16');
  assert.equal(h.document.querySelector('[data-fx-value="1"]').textContent, '1/16');
  await h.el('fx-knob-1').fire('keydown', { key: 'ArrowUp' }); await h.settle();
  assert.equal(h.document.querySelector('[data-fx-value="1"]').textContent, '1/8');
  assert.equal(h.document.querySelector('[data-fx-value="0"]').textContent, '4 × 1/8', 'Time follows the Unit');
  assert.equal(h.el('fx-knob-0').attributes['aria-valuetext'], '4 × 1/8');
  await h.el('fx-knob-0').fire('keydown', { key: 'ArrowUp' }); await h.settle();
  assert.equal(h.document.querySelector('[data-fx-value="0"]').textContent, '4.5 × 1/8');
});

test('drag a tile onto another to reorder; the power button bypasses without selecting', async () => {
  const h = harness();
  for (const id of ['stfx:eq', 'stfx:saturator', 'stfx:plate']) await h.window.STInserts.add(h.owner, id);
  await h.settle();
  const before = h.ST.state.undo.length;
  await h.el('fx-tile-0').fire('dragstart', { dataTransfer: { setData() {} } });
  await h.el('fx-tile-2').fire('dragover', {}); assert(h.el('fx-tile-2').classes.has('is-drop-after'));
  await h.el('fx-tile-2').fire('drop', {}); await h.settle();
  same(h.chain(), ['Saturator', 'Plate Reverb', 'Four-band EQ']);
  assert.equal(h.ST.state.undo.length, before + 1);
  const selected = h.document.querySelector('.fx-tile.is-selected').id;
  await h.document.querySelector('[data-fx-power="1"]').click(); await h.settle();
  same(h.chain(), ['Saturator', 'Plate Reverb (off)', 'Four-band EQ']);
  assert.equal(h.document.querySelector('.fx-tile.is-selected').id, selected);
  await h.undo(); same(h.chain(), ['Saturator', 'Plate Reverb', 'Four-band EQ']);
  await h.undo(); same(h.chain(), ['Four-band EQ', 'Saturator', 'Plate Reverb']);
});

test('slot API works for buses and master too, and refuses a seventeenth insert', async () => {
  const h = harness(), api = h.window.STInserts;
  h.ST.mutate('group', p => { p.routing.buses.push({ id: 'g1', name: 'Drums', type: 'group', volume: 1, pan: 0 }); p.routing.outputs['bus:g1'] = 'master'; });
  const bus = await api.add('bus:g1', 'stfx:compressor'), master = await api.add('master', 'stfx:limiter');
  assert.equal(bus.owner, 'bus:g1'); assert.equal(master.owner, 'master');
  assert.equal(await api.setBypass('master', master.slot, true), true);
  assert.equal(api.at('master', master.slot).bypass, true);
  assert.equal(await api.remove('bus:g1', bus.slot), true); same(api.chain('bus:g1'), []);
  for (let i = 0; i < 15; i++) await api.add('master', 'stfx:eq');
  assert.equal(api.chain('master').length, 16); assert.equal(api.freeSlot('master'), -1);
  assert.equal(await api.add('master', 'stfx:eq'), null);
  assert.equal(api.chain('master').length, 16);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} insert chain tests failed`); process.exit(1); }
  console.log(`${tests.length} insert chain tests passed`);
})();
