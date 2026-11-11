'use strict';
// Shared built-in effect pages (ui/effect-pages.js) against the real native catalogue, and their use by
// the plug-in pages console (plugin-pages.js): curated pages, units, value curves, defaults and the console view.
const assert = require('node:assert/strict'), vm = require('node:vm');
const web = require('./helpers/web');
const { complete } = require('./helpers/project');
const { metadata, parameterList } = require('./helpers/builtin-effects');

const tests = [], test = (name, fn) => tests.push({ name, fn });
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);
const fresh = () => { const context = vm.createContext({ window: {}, console, Map, Set, Math, JSON, Number, String, Object, Array, Promise, Date }); web.load(context, 'effect-pages.js'); return context.window.STEffectPages; };

test('every built-in effect has curated pages that cover every native parameter exactly where the catalogue has it', () => {
  const FX = fresh(), list = metadata();
  assert(FX.setMetadata(list));
  assert.equal(Object.keys(FX.LAYOUTS).length, list.length, 'one description per catalogue effect');
  for (const effect of list) {
    const pages = FX.pages(effect.id);
    assert(pages?.length, `${effect.id} has pages`);
    const used = new Set();
    for (const page of pages) {
      assert(page.name && page.slots.length === 8, `${effect.id} · ${page.name}: eight knob slots`);
      const filled = page.slots.filter(Boolean);
      assert(filled.length >= 1 && filled.length <= 8);
      same(page.slots.slice(filled.length), Array(8 - filled.length).fill(null), 'knobs are packed from the left');
      for (const knob of filled) {
        const native = effect.parameters.find(p => p.id === knob.id);
        assert(native, `${effect.id}: ${knob.id} exists natively`);
        assert.equal(knob.address, String(native.address), `${effect.id}: ${knob.id} uses its native address`);
        assert(knob.name.length <= 11, `${effect.id}: short knob label "${knob.name}"`);
        used.add(knob.id);
      }
      assert(page.slots[0]?.section, `${effect.id} · ${page.name}: the first knob opens a section`);
    }
    for (const p of effect.parameters) assert(used.has(p.id), `${effect.id}: ${p.id} is on a page`);
  }
});

test('Maschine-style order: the main page leads with the musical controls', () => {
  const FX = fresh(); FX.setMetadata(metadata());
  const names = id => FX.pages(id)[0].slots.filter(Boolean).map(knob => knob.name);
  same(FX.pages('stfx:filter').map(page => page.name), ['Main', 'LFO', 'Envelope', 'Sidechain']);
  same(names('stfx:filter'), ['Cutoff', 'Reso', 'Mode', 'Mod amount', 'Mod source', 'Mix']);
  same(names('stfx:compressor').slice(0, 4), ['Threshold', 'Ratio', 'Attack', 'Release']);
  same(FX.pages('stfx:eq')[0].slots.map(knob => knob.section), ['Low', '', 'Low Mid', '', 'High Mid', '', 'High', '']);
  same(names('stfx:reverb').slice(0, 4), ['Mode', 'Reverb Time', 'Size', 'Pre delay']);
});

test('values follow the native curves and read in their own units', () => {
  const FX = fresh(); FX.setMetadata(metadata());
  const show = (id, parameter, value) => FX.display(id, parameter, value);
  assert.equal(show('stfx:filter', 'cutoff', 0), '20 Hz');
  assert.equal(show('stfx:filter', 'cutoff', 1), '20 kHz');
  assert.equal(show('stfx:filter', 'cutoff', .5), '632 Hz', 'logarithmic: the middle is the geometric mean');
  assert.equal(show('stfx:filter', 'mode', 1 / 3), 'Band-pass');
  assert.equal(show('stfx:compressor', 'threshold', .7), '−18 dB');
  assert.equal(show('stfx:eq', 'lowgain', .75), '+12 dB');
  assert.equal(show('stfx:eq', 'lowgain', .5), '0 dB');
  assert.equal(show('stfx:compressor', 'ratio', FX.normalized(FX.parameter('stfx:compressor', 'ratio'), 4)), '4:1');
  assert.equal(show('stfx:reverb', 'time', 0), '500 ms');
  assert.equal(show('stfx:performfilter', 'position', .65), '65%', 'perform effects store percent as 0..1');
  assert.equal(show('stfx:burstecho', 'time', FX.normalized(FX.parameter('stfx:burstecho', 'time'), .5)), '1/8');
  assert.equal(show('stfx:burstecho', 'time', FX.normalized(FX.parameter('stfx:burstecho', 'time'), .75)), '1/8.');
  assert.equal(show('stfx:compressor', 'scfilter', 1), 'On');
  assert.equal(show('stfx:freqshifter', 'coarse', 1), '+5000 Hz');
  const cutoff = FX.parameter('stfx:filter', 'cutoff');
  for (const value of [20, 100, 1000, 16000]) assert(Math.abs(FX.physical(cutoff, FX.normalized(cutoff, value)) - value) < 1e-6 * value);
  const knob = FX.pages('stfx:filter')[0].slots[0];
  assert(Math.abs(knob.defaultValue - FX.normalized(cutoff, 16000)) < 1e-12, 'default for double-click reset');
  assert.equal(FX.pages('stfx:filter')[0].slots[2].stepCount, 3, 'Mode has four choices');
  assert.equal(FX.display('vst3:other', 0, .5), '', 'other plug-ins keep their own text');
});

test('values that only make sense with another parameter show the combination (Beat Delay Time × Unit)', () => {
  const FX = fresh(); FX.setMetadata(metadata());
  const at = (id, key, value) => FX.normalized(FX.parameter(id, key), value);
  const values = { unit: at('stfx:beatdelay', 'unit', 2), time_mode: 0 };
  const sibling = id => values[id];
  assert.equal(FX.display('stfx:beatdelay', 'time', at('stfx:beatdelay', 'time', 7)), '4', 'alone it is a bare count');
  assert.equal(FX.display('stfx:beatdelay', 'time', at('stfx:beatdelay', 'time', 7), sibling), '4 × 1/16');
  values.unit = at('stfx:beatdelay', 'unit', 6);
  assert.equal(FX.display('stfx:beatdelay', 'time', at('stfx:beatdelay', 'time', 0), sibling), '0.5 × 1/8T');
  assert.equal(FX.display('stfx:beatdelay', 'unit', values.unit, sibling), '1/8T', 'the unit itself is unchanged');
  const time = at('stfx:burstecho', 'time', .5);
  values.time_mode = at('stfx:burstecho', 'time_mode', 0); assert.equal(FX.display('stfx:burstecho', 'time', time, sibling), '1/8');
  values.time_mode = at('stfx:burstecho', 'time_mode', 1); assert.equal(FX.display('stfx:burstecho', 'time', time, sibling), '1/8 dotted');
  values.time_mode = at('stfx:burstecho', 'time_mode', 2); assert.equal(FX.display('stfx:burstecho', 'time', time, sibling), '1/8 triplet');
  same(FX.linkedAddresses('stfx:beatdelay'), [String(FX.parameter('stfx:beatdelay', 'time').address)]);
  same(FX.linkedAddresses('stfx:compressor'), []);
});

test('add list is grouped in Maschine categories', () => {
  const FX = fresh(); FX.setMetadata(metadata());
  const groups = FX.categories();
  same(groups.map(group => group.name), ['Dynamics', 'Filter & EQ', 'Modulation', 'Reverb & Space', 'Delay', 'Distortion', 'Perform']);
  assert.equal(groups.reduce((n, group) => n + group.effects.length, 0), metadata().length);
  assert(groups.find(group => group.name === 'Reverb & Space').effects.some(effect => effect.id === 'stfx:plate'));
  assert.equal(FX.categoryOf('stfx:compressor'), 'Dynamics');
});

test('metadata loads once from the bridge and late metadata relabels pages', async () => {
  const FX = fresh(); const calls = [];
  assert.equal(FX.ready, false);
  assert.equal(FX.pages('stfx:filter')[0].slots[0].address, null, 'without metadata the address comes from the read parameters');
  let changed = 0; FX.onChange(() => changed++);
  assert.equal(await FX.load(command => { calls.push(command); return metadata(); }), true);
  assert.equal(await FX.load(command => { calls.push(command); return metadata(); }), true);
  same(calls, ['builtin.effects']); assert.equal(changed, 1);
});

// ---- The plug-in pages console uses the curated pages --------------------------------------------------------------
function console_() {
  const calls = [], undo = [];
  const project = complete({ tracks: [{ name: 'Drums', instrument: 0, routingID: 'drums' }], instruments: [{ type: 'synth' }], patterns: [{ id: 0, length: 16, steps: [[]], automation: [] }],
    audioPlugins: [{ owner: 'track:drums', kind: 'effect', slot: 0, component: 'stfx:filter', name: 'Multimode Filter', format: 'Builtin', instanceID: 'fx-filter' },
      { owner: 'track:drums', kind: 'effect', slot: 1, component: 'aufx:dly', name: 'Some Delay', instanceID: 'fx-other' }] });
  const values = new Map();
  const ST = { state: { project, track: 0, instrument: 0, view: 'pattern' }, escapeHTML: value => String(value), toast() {}, refresh() {}, markDirty() {},
    request(command, payload) {
      calls.push({ command, payload });
      if (command === 'plugin.parameters') {
        if (payload.slot === 1) return Promise.resolve([{ address: '0', name: 'Time', normalized: .5, display: '250 ms' }, { address: '1', name: 'Feedback', normalized: .3, display: '30 %' }]);
        return Promise.resolve(parameterList('stfx:filter').map(p => values.has(p.address) ? { ...p, normalized: values.get(p.address) } : p));
      }
      if (command === 'plugin.parameter') { values.set(String(payload.address), payload.value); return Promise.resolve({ normalized: payload.value }); }
      if (command === 'plugin.state') return Promise.resolve({ state: 'captured', instanceID: payload.instanceID || 'fx-filter' });
      if (command === 'builtin.effects') return Promise.resolve(metadata());
      return Promise.resolve({});
    },
    mutate(label, fn) { undo.push({ label, project: JSON.parse(JSON.stringify(project)) }); fn(project); return true; } };
  const timers = [];
  const context = vm.createContext({ window: { ST }, document: { activeElement: null, querySelector: () => null, getElementById: () => null }, console, Map, Set, Math, JSON, Number, String, Object, Array, Promise, Date,
    setTimeout: (fn) => { timers.push(fn); return timers.length; }, clearTimeout() {} });
  web.load(context, 'fine-control.js'); web.load(context, 'effect-pages.js'); web.load(context, 'plugin-pages.js');
  const flush = async () => { for (let i = 0; i < 6; i++) { while (timers.length) timers.shift()(); for (let j = 0; j < 20; j++) await Promise.resolve(); } };
  return { window: context.window, ST, calls, undo, values, flush };
}

test('console and controller show curated pages, unit text and defaults for built-in effects; others keep generic pages', async () => {
  const h = console_(), ctl = h.window.STPluginPages.controller.forOwner('track:drums', 'fx-filter');
  ctl.view(); await h.flush();
  assert(h.calls.some(call => call.command === 'builtin.effects'), 'the console asks the bridge for the catalogue once');
  let view = ctl.view(); await h.flush(); view = ctl.view();
  same(view.pages, ['Main', 'LFO', 'Envelope', 'Sidechain']);
  assert.equal(view.builtin, true);
  same(view.knobs.slice(0, 3).map(knob => [knob.name, knob.section, knob.display]), [['Cutoff', 'Filter', '16 kHz'], ['Reso', '', '0%'], ['Mode', '', 'Low-pass']]);
  assert.equal(view.knobs[6], null);
  // Turning and resetting: unit text while previewing, one Undo step per gesture, back to the default.
  ctl.adjust(0, -.2); view = ctl.view();
  assert.match(view.knobs[0].display, /kHz|Hz/); assert.notEqual(view.knobs[0].display, '16 kHz');
  ctl.finish(); await h.flush();
  assert.equal(h.undo.length, 1); assert.equal(h.undo[0].label, 'Adjust Cutoff');
  ctl.reset(0); await h.flush();
  assert.equal(ctl.view().knobs[0].display, '16 kHz'); assert.equal(h.undo.length, 2);
  ctl.setPage(1); same(ctl.view().knobs.slice(0, 4).map(knob => knob.name), ['Rate', 'Sync rate', 'LFO shape', 'LFO phase']);
  // A plug-in that is not built in keeps the generated eight-at-a-time page and its own text.
  const other = h.window.STPluginPages.controller.forOwner('track:drums', 'fx-other');
  other.view(); await h.flush();
  const generic = other.view();
  assert.equal(generic.builtin, false); same(generic.pages, ['Time – Feedback']); assert.equal(generic.knobs[0].display, '250 ms');
});

test('instrument-view console: empty knobs of a curated built-in page are blank, not "Assign…"', async () => {
  const h = console_();
  h.window.STPluginPages.render(h.ST.state.project.instruments[0], null); await h.flush();
  const markup = h.window.STPluginPages.render(h.ST.state.project.instruments[0], null);
  assert.match(markup, /Effect pages/);
  assert.equal((markup.match(/data-pp-slot=/g) || []).length, 8);
  assert.equal((markup.match(/pp-blank"/g) || []).length, 2, 'Main has six knobs; the other two are blank');
  assert.doesNotMatch(markup, /Assign…|data-pp-empty|Unassigned/);
  // A plug-in that is not built in still offers Assign… on its generated page's empty knobs.
  h.window.STPluginPages.controller.forOwner('track:drums', 'fx-other').view(); await h.flush();
  h.window.STPluginPages.controller.selectSlot('effect:1');
  assert.match(h.window.STPluginPages.render(h.ST.state.project.instruments[0], null), /Assign… <kbd>↵<\/kbd>/);
});

test('instrument-view console: your pages on a built-in effect come after its curated pages (pin, New page)', async () => {
  const h = console_(), api = h.window.STPluginPages, inst = h.ST.state.project.instruments[0];
  api.render(inst, null); await h.flush();
  const node = dataset => ({ dataset, type: 'range', value: '.5', on: {}, addEventListener(type, fn) { this.on[type] = fn; } });
  const knob = node({ ppKnob: '1', ppAddress: api.controller.view().knobs[1].address, ppName: 'Reso' }), create = node({ ppAction: 'new' }), listeners = {};
  const root = { isConnected: true, dataset: {}, contains: () => true, querySelector: () => null, addEventListener(type, fn) { listeners[type] = fn; },
    querySelectorAll: query => query === '[data-pp-action]' ? [create] : query === '[data-pp-address]' || query === '[data-pp-knob]' ? [knob] : [] };
  api.bind({ querySelector: query => query === '[data-pp-root]' ? root : null });
  // P pins the knob: the Pinned page is added after the four curated pages, so the open page does not move.
  listeners.keydown({ key: 'p', target: knob, preventDefault() {}, stopPropagation() {} });
  let view = api.controller.view();
  same(view.pages, ['Main', 'LFO', 'Envelope', 'Sidechain', 'Pinned']); assert.equal(view.page, 0, 'pinning keeps the curated page open');
  // New page opens the page it created, not a curated page with the same index among your pages.
  create.on.click(); view = api.controller.view();
  assert.equal(view.pages[view.page], 'My page 2', 'the new page is shown'); assert.equal(view.page, 5);
  // On one of your pages, pinning into a new Pinned page still follows the page as it shifts by one.
  h.ST.state.project.pluginPages['fx-filter'].pages.shift(); api.controller.setPage(4);
  assert.equal(api.controller.view().pages[4], 'My page 2');
  listeners.keydown({ key: 'p', target: knob, preventDefault() {}, stopPropagation() {} });
  view = api.controller.view(); assert.equal(view.pages[view.page], 'My page 2'); assert.equal(view.page, 5);
});

(async () => {
  let failed = 0;
  for (const { name, fn } of tests) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
  }
  if (failed) { console.error(`${failed} of ${tests.length} effect page tests failed`); process.exit(1); }
  console.log(`${tests.length} effect page tests passed`);
})();
