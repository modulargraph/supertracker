'use strict';
// MK3 PLUG-IN with built-in effects: curated pages (page names, ◄ ►, encoder labels in units) and the Bypass /
// Remove display buttons through the public slot API (window.STInserts), with Undo. Real app + MK3 modules;
// the native host is a small fake (tests/helpers/builtin-effects.js supplies the real effect catalogue).
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const { metadata, parameterList } = require('./helpers/builtin-effects');
const { session, groupSession } = require('./helpers/mk3-session');
const clone = value => JSON.parse(JSON.stringify(value));
const same = (actual, expected, message) => assert.equal(JSON.stringify(actual), JSON.stringify(expected), message);

function effectSession() {
  const h = session();
  for (const file of ['fine-control.js', 'effect-pages.js', 'routing-project.js', 'studio.js']) h.load(file);
  h.win.STEffectPages.setMetadata(metadata());
  const p = h.state.project;
  p.tracks[0].routingID = 'lead';
  let nativePlugins = [
    { owner: 'track:lead', kind: 'effect', slot: 0, component: 'stfx:filter', name: 'Multimode Filter', format: 'Builtin', instanceID: 'fx-filter', state: 'filter-state' },
    { owner: 'track:lead', kind: 'effect', slot: 1, component: 'stfx:beatdelay', name: 'Beat Delay', format: 'Builtin', instanceID: 'fx-delay', state: 'delay-state' }];
  p.audioPlugins = clone(nativePlugins);
  const at = payload => nativePlugins.find(x => x.slot === payload.slot && x.owner === payload.owner);
  const handlers = {
    'plugin.parameters': q => ({ parameters: parameterList(at(q)?.component || 'stfx:filter') }),
    'plugin.parameter': q => { const d = at(q); if (q.bypass !== undefined) { d.bypass = q.bypass; return clone(d); } return { normalized: q.value }; },
    'plugin.state': q => { const d = at(q); return d ? { component: d.component, instanceID: d.instanceID, state: d.state, bypass: !!d.bypass } : nativePlugins.map(clone); },
    'plugin.remove': q => { nativePlugins = nativePlugins.filter(x => x !== at(q)); return {}; },
    'engine.snapshot': () => ({ audioPlugins: nativePlugins.map(clone) }),
    'mk3.connect': q => ({ connected: true, screens: true, exclusive: !!q.exclusive }), 'mk3.status': () => ({ connected: true, screens: true })
  };
  h.win.STHostTransport.post = message => {
    h.calls.push(message);
    Promise.resolve().then(() => (handlers[message.command] || (() => ({})))(message.payload || {}))
      .then(result => h.win.STHostTransport.deliver({ id: message.id, ok: true, result }), error => h.win.STHostTransport.deliver({ id: message.id, ok: false, error: error.message }));
  };
  h.settle = async () => { for (let i = 0; i < 30; i++) { for (let j = 0; j < 30; j++) await Promise.resolve(); h.advance(100); } };
  h.shift = name => { h.input({ type: 'button', name: 'shift', pressed: true }); h.press(name); h.input({ type: 'button', name: 'shift', pressed: false }); };
  h.MK3.subscribe(() => {});
  return h;
}

const cases = [], test = (name, fn) => cases.push([name, fn]);

test('PLUG-IN · Track shows a built-in effect with its curated pages; ◄ ► step them and encoders read in units', async () => {
  const h = effectSession();
  h.press('plugin'); h.press('display2'); await h.settle();
  let page = h.MK3.page();
  assert.equal(page.data.name, 'Multimode Filter');
  same(page.pages, ['Main', 'LFO', 'Envelope', 'Sidechain']);
  same(page.knobs.slice(0, 6).map(knob => knob?.label), ['Cutoff', 'Reso', 'Mode', 'Mod amount', 'Mod source', 'Mix']);
  same(page.knobs.slice(0, 3).map(knob => knob?.value), ['16 kHz', '0%', 'Low-pass']);
  assert.equal(page.knobs[2].detent, .03, 'Mode turns in detents');
  assert.equal(page.knobs[0].section, 'Filter');
  h.press('arrowRight'); await h.settle(); page = h.MK3.page();
  assert.equal(page.page, 1); same(page.knobs.slice(0, 4).map(knob => knob?.label), ['Rate', 'Sync rate', 'LFO shape', 'LFO phase']);
  // The rendered screens carry the page name and the knob values.
  const frame = JSON.stringify(h.MK3.frame().screens);
  assert(frame.includes('LFO') && frame.includes('Sync rate'));
});

test('display 4 bypasses the selected insert (one Undo step) and shows its state', async () => {
  const h = effectSession();
  h.press('plugin'); h.press('display2'); await h.settle();
  let page = h.MK3.page();
  same(page.buttons.map(button => button?.label), ['Master', 'Track', 'Instr.', 'Bypass', 'Editor', 'Browse', '+ FX', 'Remove']);
  const undo = h.state.undo.length;
  h.press('display4'); await h.settle();
  assert.equal(h.state.project.audioPlugins.find(x => x.instanceID === 'fx-filter').bypass, true);
  assert.equal(h.state.undo.length, undo + 1); assert.equal(h.state.undo.at(-1).label, 'Bypass plug-in effect');
  page = h.MK3.page(); assert.equal(page.buttons[3].label, 'Bypassed'); assert.equal(page.buttons[3].on, true); assert.equal(page.data.bypassed, true);
  h.press('display4'); await h.settle();
  assert.equal(h.state.project.audioPlugins.find(x => x.instanceID === 'fx-filter').bypass, false);
  await h.ST.perform('undo'); await h.settle();
  assert.equal(h.state.project.audioPlugins.find(x => x.instanceID === 'fx-filter').bypass, true);
});

test('display 8 asks before removing: a second press (or Shift + Remove) removes, Undo restores', async () => {
  const h = effectSession();
  h.press('plugin'); h.press('display2'); await h.settle();
  h.press('display8');
  assert.match(h.MK3.state.flash.text, /Remove Multimode Filter\? Press again/); assert.equal(h.MK3.page().buttons[7].label, 'Confirm');
  assert.equal(h.MK3.page().buttons[7].on, true);
  await h.settle();
  assert.equal(h.state.project.audioPlugins.length, 2, 'one press only asks');
  h.advance(3000); h.press('display8'); await h.settle();
  assert.equal(h.state.project.audioPlugins.length, 2, 'the question expires');
  h.press('display8'); h.press('display8'); await h.settle();
  same(h.state.project.audioPlugins.map(x => x.name), ['Beat Delay']);
  assert.equal(h.state.undo.at(-1).label, 'Remove plug-in effect');
  assert.match(h.MK3.state.flash.text, /Removed Multimode Filter · Undo restores it/);
  // The chain moves on to the next module, Beat Delay, with its own pages.
  assert.equal(h.MK3.page().data.name, 'Beat Delay'); same(h.MK3.page().pages, ['Main', 'Split']);
  h.shift('display8'); await h.settle();
  same(h.state.project.audioPlugins, [], 'Shift + Remove removes at once');
  await h.ST.perform('undo'); await h.settle(); await h.ST.perform('undo'); await h.settle();
  same(h.state.project.audioPlugins.map(x => x.name).sort(), ['Beat Delay', 'Multimode Filter']);
});

test('PLUG-IN · Master uses the same buttons for master inserts', async () => {
  const h = effectSession();
  h.state.project.audioPlugins.push({ owner: 'master', kind: 'effect', slot: 0, component: 'stfx:limiter', name: 'Peak Limiter', format: 'Builtin', instanceID: 'fx-limiter' });
  h.press('plugin'); h.press('display1'); await h.settle();
  const page = h.MK3.page();
  assert.equal(page.data.name, 'Peak Limiter'); same(page.pages, ['Main', 'Sidechain']);
  same(page.buttons.map(button => button?.label), ['Master', 'Track', 'Instr.', 'Bypass', 'Editor', 'Browse', '+ FX', 'Remove']);
});

(async () => {
  let failed = 0;
  for (const [name, fn] of cases) {
    try { await fn(); console.log(`ok - ${name}`); }
    catch (error) { failed++; console.error(`not ok - ${name}\n${error.stack}`); }
  }
  if (failed) { console.error(`${failed} of ${cases.length} MK3 effect page tests failed`); process.exit(1); }
  console.log(`${cases.length} MK3 effect page tests passed`);
})();
