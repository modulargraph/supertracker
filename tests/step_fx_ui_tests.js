'use strict';
// Real step-FX module; only the modal DOM and native parameter-read boundary are stubbed.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const source = process.env.SUPERTRACKER_STEP_FX_SOURCE ? fs.readFileSync(process.env.SUPERTRACKER_STEP_FX_SOURCE, 'utf8') : web.source('step-fx.js');
const fineSource = web.source('fine-control.js');
const copy = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const escapeHTML = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const decode = value => value.replace(/&(amp|lt|gt|quot|#39);/g, (_, c) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" }[c]));
const ID = '00000000-0000-0000-0000-000000000017', OTHER = '00000000-0000-0000-0000-000000000018', MAX = '18446744073709551615';
const parameter = (address = MAX, extra = {}) => ({ address, name: 'Native frequency', writable: true, automatable: true, normalized: .27, ...extra });
const plugin = extra => ({ instanceID: ID, owner: 'track:keys', kind: 'effect', slot: 2, component: 'AU:fixture:one', name: 'Fixture Echo', ...extra });
const saved = extra => ({ type: 'plugin', instanceID: ID, address: MAX, name: 'Saved frequency', value: .4, ...extra });
function fixture() {
  const pattern = id => ({ id, name: `Pattern ${id}`, length: 8, steps: [Array(8).fill(null), Array(8).fill(null)], automation: [] });
  return { activePattern: 0, tracks: [{ routingID: 'keys', name: 'Keys', instrument: 0 }, { routingID: 'bass', name: 'Bass', instrument: 1 }], instruments: [{ type: 'synth' }, { type: 'sample' }], patterns: [pattern(0), pattern(1)], audioPlugins: [plugin()] };
}
function harness(options = {}) {
  const nodes = new Map(), calls = [], mutations = [], notices = [], metadataCalls = [];
  const state = { project: options.project || fixture(), track: 0, row: 0, column: 3, modal: null };
  let roots = [], reads = options.read || (() => ({ parameters: [parameter()] })), waits = options.wait || (() => Promise.resolve()), refreshes = 0;
  const descendants = element => element.children.flatMap(child => [child, ...descendants(child)]);
  const all = () => roots.flatMap(root => [root, ...descendants(root)]);
  const document = { activeElement: null, getElementById: id => nodes.get(id) || null, querySelectorAll: selector => all().filter(item => matches(item, selector)) };
  const matches = (element, selector) => selector.startsWith('#') ? element.id === selector.slice(1) : selector.startsWith('[') ? Object.hasOwn(element.attributes, selector.slice(1, -1)) : element.tagName === selector.toUpperCase();
  function remove(element) { element.isConnected = false; if (nodes.get(element.id) === element) nodes.delete(element.id); for (const child of element.children) remove(child); }
  function parse(markup, parent = null) {
    const result = [], stack = parent ? [parent] : [];
    for (const match of markup.matchAll(/<\/?[\w:-]+\b[^>]*>/g)) {
      const raw = match[0], tagName = /^<\/?([\w:-]+)/.exec(raw)[1].toUpperCase();
      if (raw.startsWith('</')) { while (stack.length && stack.at(-1) !== parent && stack.pop().tagName !== tagName) {} continue; }
      const attributes = {}, dataset = {};
      for (const a of raw.slice(/^<[\w:-]+/.exec(raw)[0].length, -1).matchAll(/([^\s=/>]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)) { const name = a[1], value = decode(a[2] ?? a[3] ?? a[4] ?? ''); attributes[name] = value; if (name.startsWith('data-')) dataset[name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value; }
      const element = { id: attributes.id || '', tagName, attributes, dataset, parentElement: stack.at(-1) || null, children: [], isConnected: true, disabled: Object.hasOwn(attributes, 'disabled'), value: attributes.value || '', type: attributes.type || '', textContent: '', classList: { toggle() {}, add() {}, remove() {} },
        setAttribute(name, value) { this.attributes[name] = String(value); }, focus() { if (!this.disabled && this.isConnected) { document.activeElement = this; this.onfocus?.({ target: this }); } }, select() { this.selectedText = true; }, scrollIntoView() { this.scrolled = true; },
        querySelectorAll(selector) { return descendants(this).filter(child => matches(child, selector)); },
        async fire(type, extra = {}) { const event = { target: this, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra }; if (!this.disabled && this.isConnected) await this['on' + type]?.(event); return event; }
      };
      Object.defineProperty(element, 'innerHTML', { get() { return this.markup || ''; }, set(value) { this.children.forEach(remove); this.children = []; this.markup = value; parse(value, this); } });
      if (element.parentElement) element.parentElement.children.push(element); else result.push(element);
      if (element.id) nodes.set(element.id, element);
      if (tagName === 'OPTION' && element.parentElement?.tagName === 'SELECT' && (Object.hasOwn(attributes, 'selected') || element.parentElement.children.length === 1)) element.parentElement.value = element.value;
      if (!['INPUT', 'BR', 'HR', 'IMG'].includes(tagName)) stack.push(element);
    }
    return result;
  }
  const ST = { state, FX: { volume: [0, 1, .01], pan: [-1, 1, .01], cutoff: [20, 20000, 1], probability: [0, 1, .01], slice: [1, 128, 1] }, escapeHTML, pattern: () => state.project.patterns.find(p => p.id === state.project.activePattern),
    request(command, payload) { calls.push({ command, payload: copy(payload) }); assert.equal(command, 'plugin.parameters', 'The picker must never write a live plug-in parameter or sound a note'); return Promise.resolve().then(() => reads(payload)); },
    waitForGraphEdit() { return waits(); }, mutate(label, fn) { mutations.push({ label, before: copy(state.project) }); fn(state.project); }, toast(message) { notices.push(message); }, refresh() { refreshes++; },
    openModal(title, body, footer = '', settings = {}) { roots.forEach(remove); nodes.clear(); roots = parse(body + footer); state.modal = settings.kind; },
    closeModal() { state.modal = null; roots.forEach(remove); roots = []; nodes.clear(); document.activeElement = null; }
  };
  const studio = { isEditingPlugin: false, isPreparingEffectMove: false, isMovingEffect: false }, routing = { isPreparing: false, isApplying: false };
  const context = vm.createContext({ window: { ST, STStudio: studio, STRouting: routing, STNKSControls: { automationMetadata(instance) { metadataCalls.push(instance); return options.metadata || new Map(); } } }, document });
  web.load(context, 'fine-control.js', { text: fineSource }); web.load(context, 'step-fx.js', { text: source }); const ui = context.window.STStepFX;
  const get = id => { const element = nodes.get(id); assert(element, `Missing ${id}`); return element; };
  return { context, ui, ST, state, studio, routing, document, calls, mutations, notices, metadataCalls, get, read: fn => { reads = fn; }, wait: fn => { waits = fn; }, get refreshes() { return refreshes; },
    mountInspector() { roots.forEach(remove); roots = parse([1, 2].map(n => ui.renderField(n, ST.pattern().steps[state.track][state.row]?.['fx' + n])).join('')); ui.bind(); },
    async search(query) { get('step-fx-search').value = query; await get('step-fx-search').fire('input'); return get('step-fx-results').innerHTML; },
    async choose(query) { await this.search(query); assert(get('step-fx-results').querySelectorAll('[data-fx-result]').length, `No result for ${query}`); await get('step-fx-search').fire('keydown', { key: 'Enter' }); },
    async apply(value) { if (value !== undefined) get('step-fx-value').value = value; await get('step-fx-apply').fire('click'); },
    async settle() { for (let n = 0; n < 4; n++) await tick(); }
  };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('a delayed inspector change cannot edit a newly selected row or replacement project', async () => {
  for (const replacement of [false, true]) {
    const h = harness(); h.ST.pattern().steps[0][0] = { note: -3, fx1: saved() }; h.ST.pattern().steps[0][1] = { note: 64, fx1: { type: 'volume', value: .2 } };
    h.mountInspector(); const input = h.get('step-fx1-value'); input.value = '.9';
    if (replacement) h.state.project = copy(h.state.project); else h.state.row = 1;
    const before = copy(h.state.project); await input.fire('change');
    assert.deepEqual(h.state.project, before); assert.equal(h.mutations.length, 0); assert.match(h.notices.at(-1), /step changed|busy/);
  }
});

test('search combines built-ins, live parameter names and NKS labels without touching the project', async () => {
  const metadata = new Map([[MAX, { name: 'Color', group: 'NKS Tone', search: 'Brightness Macro Bank 2' }]]), h = harness({ metadata }), before = copy(h.state.project);
  await h.ui.show(); assert.equal(h.document.activeElement.id, 'step-fx-search'); assert.match(await h.search('filter cutoff'), /Filter cutoff/);
  assert.match(await h.search('brightness fixture'), /Color/); assert.match(h.get('step-fx-results').innerHTML, /NKS Tone/); assert.match(await h.search('native frequency'), /Color/); assert.match(await h.search(MAX), /Color/);
  assert.match(await h.search('not-a-parameter'), /No matching/); assert.equal(h.metadataCalls.length, 1); assert.deepEqual(h.state.project, before); assert.equal(h.mutations.length, 0); assert.deepEqual(h.calls[0].payload, { owner: 'track:keys', kind: 'effect', slot: 2, instanceID: ID });
});

test('only explicit writable and automatable parameters with canonical UInt64 string addresses are offered', async () => {
  const rejected = [parameter('2', { writable: false }), parameter('3', { automatable: false }), parameter('4', { writable: undefined }), parameter('5', { automatable: 1 }), parameter(6), parameter('007'), parameter('-1'), parameter('18446744073709551616'), parameter('1.0'), parameter('1e2')];
  const h = harness({ read: () => [parameter(MAX, { name: 'Valid maximum' }), parameter('0', { name: 'Valid zero' }), ...rejected.map((p, index) => ({ ...p, name: `Rejected ${index}` }))] });
  await h.ui.show(); const markup = await h.search('valid'); assert.match(markup, /Valid maximum/); assert.match(markup, /Valid zero/); assert.doesNotMatch(await h.search('rejected'), /Rejected/);
  await h.choose('maximum'); await h.apply('.73'); const fx = h.ST.pattern().steps[0][0].fx1; assert.equal(fx.address, MAX); assert.equal(typeof fx.address, 'string'); assert.equal(fx.instanceID, ID); assert.equal(fx.value, .73); assert.equal(h.mutations.length, 1);
});

test('only unique same-track instances are read; saved missing, duplicate and other-track targets stay unavailable', async () => {
  for (const kind of ['missing', 'duplicate', 'other-track', 'invalid-id']) {
    const p = fixture(); p.patterns[0].steps[0][0] = { note: -3, instrument: 0, fx1: saved() };
    if (kind === 'missing') p.audioPlugins = [];
    if (kind === 'duplicate') p.audioPlugins.push(plugin({ slot: 3 }));
    if (kind === 'other-track') p.audioPlugins[0].owner = 'track:bass';
    if (kind === 'invalid-id') { p.audioPlugins[0].instanceID = ''; p.patterns[0].steps[0][0].fx1.instanceID = ''; }
    const h = harness({ project: p }), before = copy(p); await h.ui.show(); assert.match(h.ui.renderField(1, p.patterns[0].steps[0][0].fx1), /unavailable/, kind); assert.match(h.get('step-fx-choice-note').textContent, /unavailable/, kind); assert.equal(h.calls.length, 0, kind); assert.deepEqual(p, before); assert.equal(h.mutations.length, 0);
  }
});

test('explicit value editing of an unavailable saved target retains identity and never reconnects it', async () => {
  const p = fixture(); p.audioPlugins = [plugin({ instanceID: OTHER })]; const original = saved({ extraSavedField: 'keep' }); p.patterns[0].steps[0][0] = { note: -3, instrument: 0, fx1: original };
  const h = harness({ project: p }); await h.ui.show(); assert.match(h.get('step-fx-choice-note').textContent, /exact identity is retained/); await h.apply('.8');
  assert.deepEqual(copy(p.patterns[0].steps[0][0].fx1), { ...original, value: .8 }); assert.equal(h.mutations.length, 1); assert.equal(p.patterns[0].steps[0][0].note, -3);
  h.mountInspector(); const field = h.get('step-fx1-value'); field.value = '.6'; await field.fire('change'); assert.deepEqual(copy(p.patterns[0].steps[0][0].fx1), { ...original, value: .6 }); assert.equal(h.mutations.length, 2);
});

test('legacy saved instance strings remain supported with exact case-sensitive identity', async () => {
  for (const id of ['stable-effect', 'Stable-effect']) {
    const p = fixture(); p.audioPlugins = [plugin({ instanceID: id })]; const h = harness({ project: p }); await h.ui.show(); await h.choose('native frequency'); await h.apply('.6'); assert.equal(p.patterns[0].steps[0][0].fx1.instanceID, id); assert.equal(h.mutations.length, 1);
  }
  const p = fixture(); p.audioPlugins = [plugin({ instanceID: 'stable-effect' })]; p.patterns[0].steps[0][0] = { note: -3, fx1: saved({ instanceID: 'Stable-effect' }) }; const h = harness({ project: p }); await h.ui.show(); assert.match(h.get('step-fx-choice-note').textContent, /unavailable/); await h.apply('.65'); assert.equal(p.patterns[0].steps[0][0].fx1.instanceID, 'Stable-effect');
});

test('invalid saved addresses remain visible and value-editable but never become new live choices', async () => {
  for (const address of ['18446744073709551616', '007', 9007199254740992]) {
    const p = fixture(), original = saved({ address }); p.patterns[0].steps[0][0] = { note: -3, fx1: original }; const h = harness({ project: p }); await h.ui.show(); assert.match(h.ui.renderField(1, original), /unavailable/); assert.match(h.get('step-fx-choice-note').textContent, /exact identity/); await h.apply('.55'); assert.deepEqual(copy(p.patterns[0].steps[0][0].fx1), { ...original, value: .55 });
    h.state.row = 1; await h.ui.show({ plugin: { instanceID: ID, address } }); assert.equal(h.get('step-fx-apply').disabled, true); assert.match(h.get('step-fx-status').textContent, /not currently exposed/); assert.equal(p.patterns[0].steps[0][1], null); assert.equal(h.mutations.length, 1);
  }
});

test('an explicitly requested NKS parameter replaces the occupied-slot choice only after a valid read', async () => {
  const gate = deferred(), metadata = new Map([[MAX, { name: 'Tone macro', group: 'NKS Filter', search: 'Color' }]]), h = harness({ read: () => gate.promise, metadata });
  h.ST.pattern().steps[0][0] = { note: 60, velocity: 47, fx1: { type: 'pan', value: -.8 }, fx2: { type: 'slice', value: 7 } };
  const pending = h.ui.show({ fx: 1, plugin: { instanceID: ID, address: MAX } }); await h.settle(); assert.equal(h.get('step-fx-apply').disabled, true); assert.equal(h.get('step-fx-value').disabled, true);
  gate.resolve({ parameters: [parameter()] }); await pending; assert.match(h.get('step-fx-choice').textContent, /Tone macro/); assert.equal(h.get('step-fx-value').value, .27); await h.apply('.62');
  const step = h.ST.pattern().steps[0][0]; assert.deepEqual(copy(step.fx1), saved({ name: 'Tone macro', value: .62 })); assert.deepEqual(copy(step.fx2), { type: 'slice', value: 7 }); assert.equal(step.note, 60); assert.equal(step.velocity, 47); assert.equal(h.mutations.length, 1);
});

test('delayed parameter results preserve the same focused built-in result after repainting the list', async () => {
  const gate = deferred(), h = harness({ read: () => gate.promise }), pending = h.ui.show(); await h.settle();
  await h.search('pan'); await h.get('step-fx-search').fire('keydown', { key: 'ArrowDown' }); const old = h.document.activeElement; assert.equal(old.id, 'step-fx-result-0');
  gate.resolve({ parameters: [parameter()] }); await pending; assert.equal(old.isConnected, false); assert.equal(h.document.activeElement.isConnected, true); assert.equal(h.document.activeElement.id, 'step-fx-result-0'); assert.notEqual(h.document.activeElement, old); assert.match(h.get('step-fx-results').innerHTML, /Pan/); assert.equal(h.mutations.length, 0);
});

test('a numeric draft survives promotion of its saved unavailable parameter when a delayed read completes', async () => {
  const p = fixture(); p.patterns[0].steps[0][0] = { note: -3, fx1: saved() }; const gate = deferred(), h = harness({ project: p, read: () => gate.promise }), pending = h.ui.show(); await h.settle();
  const input = h.get('step-fx-value'); assert.match(h.get('step-fx-choice-note').textContent, /unavailable/); input.value = '.713'; input.focus(); await input.fire('input');
  gate.resolve({ parameters: [parameter()] }); await pending; assert.equal(h.document.activeElement, input); assert.equal(Number(input.value), .713); assert.doesNotMatch(h.get('step-fx-choice-note').textContent, /unavailable/); assert.equal(p.patterns[0].steps[0][0].fx1.value, .4); assert.equal(h.mutations.length, 0);
  await h.apply(); assert.equal(p.patterns[0].steps[0][0].fx1.value, .713); assert.equal(h.mutations.length, 1);
});

test('search arrows and Return choose, then value Return commits one FX-only row without a live parameter write', async () => {
  const h = harness(); await h.ui.show(); await h.search('filter'); const down = await h.get('step-fx-search').fire('keydown', { key: 'ArrowDown' }); assert(down.defaultPrevented); assert.equal(h.document.activeElement.id, 'step-fx-result-0');
  const enter = await h.document.activeElement.fire('keydown', { key: 'Enter' }); assert(enter.defaultPrevented); assert.equal(h.document.activeElement.id, 'step-fx-value'); assert.equal(h.get('step-fx-value').value, 1000); assert(h.get('step-fx-value').selectedText);
  h.get('step-fx-value').value = '2400'; await h.get('step-fx-value').fire('keydown', { key: 'Enter' }); await h.settle();
  const step = h.ST.pattern().steps[0][0]; assert.deepEqual(copy(step), { note: -3, instrument: 0, velocity: 100, gate: 0, fx1: { type: 'cutoff', value: 2400 } }); assert.equal(h.mutations.length, 1); assert.equal(h.mutations[0].before.patterns[0].steps[0][0], null); assert.equal(h.state.modal, null);
});

test('FX1 and FX2 switching edits only the selected slot and retains note, velocity, chord and other FX', async () => {
  const p = fixture(), step = { note: 61, instrument: 0, velocity: 53, gate: 2, chord: [0, 3, 7], fx1: { type: 'pan', value: -.2 }, fx2: saved() }; p.patterns[0].steps[0][0] = step;
  const h = harness({ project: p }); h.state.column = 4; await h.ui.show(); assert.equal(h.get('step-fx-slot').value, '2'); assert.equal(h.get('step-fx-value').value, .4);
  h.get('step-fx-slot').value = '1'; await h.get('step-fx-slot').fire('change'); assert.equal(h.get('step-fx-value').value, -.2); await h.apply('.1');
  assert.deepEqual(copy(step), { note: 61, instrument: 0, velocity: 53, gate: 2, chord: [0, 3, 7], fx1: { type: 'pan', value: .1 }, fx2: saved() }); assert.equal(h.mutations.length, 1);
  await h.ui.show({ fx: 2 }); await h.get('step-fx-remove').fire('click'); assert.equal(step.fx2, null); assert.equal(step.note, 61); assert.deepEqual(copy(step.fx1), { type: 'pan', value: .1 }); assert.equal(h.mutations.length, 2);
});

test('invalid numeric drafts cannot apply or create an undo entry', async () => {
  for (const value of ['', ' ', 'NaN', 'Infinity', '1.1', '-.01']) { const h = harness(); await h.ui.show(); await h.choose('native frequency'); const before = copy(h.state.project); await h.apply(value); assert.equal(h.state.modal, 'stepFX', value); assert.match(h.get('step-fx-status').textContent, /within the displayed range/, value); assert.equal(h.mutations.length, 0, value); assert.deepEqual(h.state.project, before); assert.equal(h.calls.length, 1, 'Invalid input must not trigger another native read'); }
});

test('cancel or close during a delayed catalogue graph wait or read prevents later commits', async () => {
  for (const phase of ['graph', 'read']) for (const method of ['cancel', 'close']) {
    const gate = deferred(), h = harness(phase === 'graph' ? { wait: () => gate.promise } : { read: () => gate.promise }), before = copy(h.state.project), pending = h.ui.show(); await h.settle();
    if (method === 'cancel') await h.get('step-fx-cancel').fire('click'); else h.ST.closeModal(); gate.resolve(phase === 'graph' ? undefined : { parameters: [parameter()] }); assert.equal(await pending, false);
    assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.project, before); assert.equal(h.state.modal, null); if (phase === 'graph') assert.equal(h.calls.length, 0);
  }
});

test('cancel or replacing the dialog during apply waits prevents a late read from committing', async () => {
  for (const phase of ['graph', 'read']) for (const replacement of [false, true]) {
    const h = harness(); await h.ui.show(); await h.choose('native frequency'); const gate = deferred(), before = copy(h.state.project);
    if (phase === 'graph') h.wait(() => gate.promise); else h.read(() => gate.promise);
    const applying = h.apply('.8'); await h.settle();
    if (replacement) h.ST.openModal('Replacement', '<div id="other-dialog"></div>', '', { kind: 'other' }); else await h.get('step-fx-cancel').fire('click');
    gate.resolve(phase === 'graph' ? undefined : { parameters: [parameter()] }); await applying; assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.project, before); assert.equal(h.state.modal, replacement ? 'other' : null);
  }
});

test('project, pattern, track, row and cell changes during reads cannot attach an effect to a new context', async () => {
  const changes = { project: h => { h.state.project = copy(h.state.project); }, pattern: h => { h.state.project.activePattern = 1; }, patternObject: h => { h.state.project.patterns[0] = { ...h.state.project.patterns[0] }; }, track: h => { h.state.track = 1; }, row: h => { h.state.row = 1; }, trackObject: h => { h.state.project.tracks[0] = { ...h.state.project.tracks[0] }; }, cell: h => { h.ST.pattern().steps[0][0] = { note: 70 }; }, cellContents: h => { h.ST.pattern().steps[0][0].velocity = 93; } };
  for (const phase of ['load', 'apply']) for (const [name, change] of Object.entries(changes)) {
    const h = harness(); h.ST.pattern().steps[0][0] = { note: 60, velocity: 80 }; const gate = deferred(); let pending;
    if (phase === 'load') { h.read(() => gate.promise); pending = h.ui.show(); } else { await h.ui.show(); await h.choose('native frequency'); h.read(() => gate.promise); pending = h.apply('.6'); }
    await h.settle(); change(h); const after = copy(h.state.project); gate.resolve({ parameters: [parameter()] }); await pending; assert.equal(h.mutations.length, 0, `${phase}:${name}`); assert.deepEqual(h.state.project, after, `${phase}:${name}`);
  }
});

test('moved, replaced, duplicated or reassigned plug-ins during a read never receive a stale attachment', async () => {
  const changes = { slot: h => { h.state.project.audioPlugins[0].slot++; }, kind: h => { h.state.project.audioPlugins[0].kind = 'instrument'; }, id: h => { h.state.project.audioPlugins[0].instanceID = OTHER; }, track: h => { h.state.project.audioPlugins[0].owner = 'track:bass'; }, duplicate: h => { h.state.project.audioPlugins.push(plugin({ slot: 5 })); }, component: h => { h.state.project.audioPlugins[0] = plugin({ component: 'AU:replacement:two' }); } };
  for (const phase of ['load', 'apply']) for (const [name, change] of Object.entries(changes)) {
    const h = harness(), gate = deferred(); let pending;
    if (phase === 'load') { h.read(() => gate.promise); pending = h.ui.show(); } else { await h.ui.show(); await h.choose('native frequency'); h.read(() => gate.promise); pending = h.apply('.8'); }
    await h.settle(); change(h); gate.resolve({ parameters: [parameter()] }); await pending;
    if (phase === 'load') assert.doesNotMatch(await h.search('native frequency'), /Native frequency/, name);
    assert.equal(h.mutations.length, 0, `${phase}:${name}`); assert.equal(h.ST.pattern().steps[0][0], null, `${phase}:${name}`);
  }
});

test('an apply reread that withdraws writability or the exact address keeps the saved row untouched', async () => {
  for (const result of [[], [parameter('17')], [parameter(MAX, { writable: false })], [parameter(MAX, { automatable: false })]]) {
    const h = harness(); await h.ui.show(); await h.choose('native frequency'); h.read(() => result); await h.apply('.8'); assert.equal(h.mutations.length, 0); assert.equal(h.ST.pattern().steps[0][0], null); assert.match(h.get('step-fx-status').textContent, /no longer writable|changed/);
  }
});

test('a completed effect move follows exact identity at the new slot when Apply rereads its parameters', async () => {
  const h = harness(); await h.ui.show({ fx: 1, plugin: { instanceID: ID, address: MAX } });
  assert.equal(h.get('step-fx-apply').disabled, false); assert.equal(h.calls.length, 1); assert.equal(h.calls[0].payload.slot, 2);
  h.state.project.audioPlugins[0] = { ...h.state.project.audioPlugins[0], slot: 6 };
  await h.apply('.61'); assert.deepEqual(h.calls.map(call => call.payload.slot), [2, 6]); assert(h.calls.every(call => call.payload.instanceID === ID));
  assert.deepEqual(copy(h.ST.pattern().steps[0][0].fx1), { type: 'plugin', instanceID: ID, address: MAX, name: 'Native frequency', value: .61 });
  assert.equal(h.mutations.length, 1); assert.equal(h.mutations[0].before.audioPlugins[0].slot, 6); assert.equal(h.state.modal, null);
});

test('inspector rendering escapes malformed saved value text without creating attributes or elements', () => {
  const h = harness(), value = '\" autofocus onfocus=\"alert(1)\"><svg onload=\"alert(2)\">';
  h.ST.pattern().steps[0][0] = { note: -3, fx1: { type: 'cutoff', value } };
  const markup = h.ui.renderField(1, h.ST.pattern().steps[0][0].fx1); assert(markup.includes(`value="${escapeHTML(value)}"`)); assert(!markup.includes('<svg'));
  h.mountInspector(); const input = h.get('step-fx1-value'); assert.equal(input.attributes.value, value); assert.equal(input.attributes.autofocus, undefined); assert.equal(input.attributes.onfocus, undefined); assert.equal(h.document.querySelectorAll('SVG').length, 0); assert.equal(h.mutations.length, 0);
});

test('a rejected root mutation or active routing application cannot close the dialog as a successful edit', async () => {
  for (const reason of ['root-mutation', 'routing']) {
    const h = harness(); await h.ui.show(); await h.choose('filter cutoff'); const before = copy(h.state.project);
    if (reason === 'root-mutation') h.ST.mutate = () => false; else h.routing.isApplying = true;
    await h.apply('2000'); assert.equal(h.state.modal, 'stepFX', reason); assert.equal(h.mutations.length, 0); assert.deepEqual(h.state.project, before); assert.match(h.get('step-fx-status').textContent, /changed|finish|Wait|Reopen/);
  }
});

test('discrete plug-in arrows advance one legal choice even with Option and Enter commits one undo', async () => {
  const h = harness({ read: () => ({ parameters: [parameter(MAX, { stepCount: 3, normalizedStep: 1 / 3, normalized: 0 })] }) });
  await h.ui.show({ plugin: { instanceID: ID, address: MAX } });
  const input = h.get('step-fx-value'); assert.equal(Number(input.step), 1 / 3);
  const key = await input.fire('keydown', { key: 'ArrowUp', altKey: true }); assert(key.defaultPrevented); assert.equal(Number(input.value), 1 / 3); assert.equal(h.mutations.length, 0);
  await input.fire('keydown', { key: 'Enter' }); await h.settle();
  assert.equal(h.ST.pattern().steps[0][0].fx1.value, 1 / 3); assert.equal(h.mutations.length, 1); assert.equal(h.state.modal, null);
});

test('explicit discrete edits snap against current metadata while opening retains saved off-grid values', async () => {
  const p = fixture(); p.audioPlugins[0].parameters = [parameter(MAX, { stepCount: 3 })]; p.patterns[0].steps[0][0] = { note: -3, fx1: saved({ value: .53 }) };
  const h = harness({ project: p, read: () => ({ parameters: [parameter(MAX, { stepCount: 3 })] }) });
  await h.ui.show(); assert.equal(Number(h.get('step-fx-value').value), .53); assert.equal(p.patterns[0].steps[0][0].fx1.value, .53); assert.equal(h.mutations.length, 0);
  // Metadata can change when the plug-in changes modes while the picker is open.
  h.read(() => ({ parameters: [parameter(MAX, { stepCount: 1 })] })); await h.apply('.53');
  assert.equal(p.patterns[0].steps[0][0].fx1.value, 1); assert.equal(h.mutations.length, 1);
  h.mountInspector(); const input = h.get('step-fx1-value'); assert.equal(Number(input.attributes.step), 1 / 3); input.value = '.53'; await input.fire('change');
  assert.equal(p.patterns[0].steps[0][0].fx1.value, 2 / 3); assert.equal(h.mutations.length, 2);
});

(async () => { for (const { name, run } of tests) { await run(); console.log(`PASS ${name}`); } console.log(`${tests.length} step FX UI suites passed.`); })().catch(error => { console.error(error); process.exitCode = 1; });
