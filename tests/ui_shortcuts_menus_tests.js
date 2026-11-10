'use strict';
// Global structure shortcuts (⌘[ ⌘] patterns, ⇧⌘[ ⇧⌘] instruments, ⌘T / ⇧⌘T tracks, ⇧⌘N new pattern),
// the instrument list context menu, the help sheet order and clash-free native menu key equivalents.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const web = require('./helpers/web');
const fixture = fs.readFileSync(path.join(__dirname, 'scalar_field_focus_tests.js'), 'utf8');
const harness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')) + '\nreturn harness;')(require, __dirname);
let count = 0;
async function test(name, run) { await run(); count++; console.log('PASS ' + name); }

(async () => {
  await test('global structure keys map to commands; other modifier combinations do not', () => {
    const { app } = web.app();
    const map = (key, extra = {}) => app.globalStructureCommand({ key, code: '', metaKey: true, ...extra });
    assert.equal(map('[', { code: 'BracketLeft' }), 'previousPattern');
    assert.equal(map(']', { code: 'BracketRight' }), 'nextPattern');
    assert.equal(map('{', { code: 'BracketLeft', shiftKey: true }), 'previousInstrument');
    assert.equal(map('}', { code: 'BracketRight', shiftKey: true }), 'nextInstrument');
    assert.equal(map('t'), 'addTrack');
    assert.equal(map('T', { shiftKey: true }), 'duplicateTrack');
    assert.equal(map('N', { shiftKey: true }), 'addPattern');
    assert.equal(map('n'), null, '⌘N stays New project');
    assert.equal(map('t', { altKey: true }), null, '⌥⌘ belongs to block navigation');
    assert.equal(app.globalStructureCommand({ key: '[', code: 'BracketLeft' }), null, 'plain [ stays octave down');
    assert.equal(map('[', { code: 'BracketLeft', isComposing: true }), null);
  });

  await test('⌘] / ⌘[ step through the pattern list (wrapping) without changing the octave; plain [ ] still do', () => {
    const h = harness();
    h.ST.perform('addPattern');
    const ids = h.state.project.patterns.map(p => p.id), octave = h.state.octave;
    assert(ids.length >= 2);
    h.state.project.activePattern = ids[0];
    const tracker = h.get('tracker');
    tracker.focus();
    h.key(h.document.activeElement, ']', { code: 'BracketRight', metaKey: true });
    assert.equal(h.state.project.activePattern, ids[1]);
    assert.equal(h.state.octave, octave, '⌘] does not change the octave');
    h.key(h.document.activeElement, '[', { code: 'BracketLeft', metaKey: true });
    h.key(h.document.activeElement, '[', { code: 'BracketLeft', metaKey: true });
    assert.equal(h.state.project.activePattern, ids[ids.length - 1], 'wraps to the last pattern');
    h.key(h.get('tracker'), '[', { code: 'BracketLeft' });
    assert.equal(h.state.octave, Math.max(0, octave - 1), 'plain [ is still octave down');
  });

  await test('⌘T adds a track and ⇧⌘N adds a pattern (not a new project), also from a text field', () => {
    const h = harness();
    const tracks = h.state.project.tracks.length, patterns = h.state.project.patterns.length;
    h.get('tracker').focus();
    h.key(h.document.activeElement, 't', { code: 'KeyT', metaKey: true });
    assert.equal(h.state.project.tracks.length, tracks + 1);
    const field = h.get('step-note');
    field.focus();
    h.key(field, 'N', { code: 'KeyN', metaKey: true, shiftKey: true });
    assert.equal(h.state.project.patterns.length, patterns + 1);
    assert(!h.calls.some(c => c.command === 'project.new'), '⇧⌘N no longer starts a new project');
  });

  await test('⇧⌘] / ⇧⌘[ select the next / previous instrument', () => {
    const h = harness();
    h.ST.perform('addInstrument');
    const count = h.state.project.instruments.length;
    assert(count >= 2);
    h.state.instrument = 0;
    h.get('instrument-rename').focus();
    h.key(h.document.activeElement, '}', { code: 'BracketRight', metaKey: true, shiftKey: true });
    assert.equal(h.state.instrument, 1);
    h.key(h.document.activeElement, '{', { code: 'BracketLeft', metaKey: true, shiftKey: true });
    h.key(h.document.activeElement, '{', { code: 'BracketLeft', metaKey: true, shiftKey: true });
    assert.equal(h.state.instrument, count - 1, 'wraps to the last instrument');
  });

  await test('instrument list context menu offers the instrument actions and keeps the selected track for Assign', async () => {
    const h = harness();
    h.ST.perform('addInstrument');
    const app = h.window.STApp, state = h.state;
    state.track = 0;
    const target = index => ({ closest: selector => selector === '[data-instrument]' ? { dataset: { instrument: String(index) } } : null });
    const items = app.instrumentMenu({ target: target(1) });
    assert.equal(items.exclusive, true);
    const titles = items.filter(Boolean).map(item => item.title || (item.separator ? '—' : ''));
    for (const title of ['Rename…', 'Load sample…', 'Show sample in Finder', 'Find control…', 'Type', 'Add instrument', 'Add wavetable synth', 'Remove instrument']) {
      assert(titles.includes(title), title);
    }
    const assign = items.find(item => item?.title?.startsWith('Assign to track'));
    assert(assign && assign.enabled, 'track 0 does not play instrument 1 yet');
    assert.equal(items.find(item => item?.title === 'Show sample in Finder').enabled, !!state.project.instruments[1].samplePath, 'only a sample file can be shown');
    const type = items.find(item => item?.title === 'Type');
    assert.deepEqual(JSON.parse(JSON.stringify(type.items.map(x => [x.title, !!x.checked, x.enabled]))), [['Sampler', true, false], ['Plug-in', false, true], ['External MIDI', false, true]]);
    const performed = [];
    const perform = app.perform;
    app.perform = command => { performed.push([command, state.instrument, state.track]); return true; };
    try {
      await assign.action();
      items.find(item => item?.title === 'Rename…').action();
    } finally { app.perform = perform; }
    assert.deepEqual(performed[0], ['assignInstrument', 1, 0], 'assign targets the clicked instrument on the selected track');
    assert.equal(performed[1][0], 'renameInstrument');
    assert.equal(performed[1][1], 1);
    assert.equal(app.instrumentMenu({ target: { closest: () => null } }), null);
  });

  await test('help reference starts with everyday keys and ends with the controller, listing every section once', () => {
    const { app } = web.app();
    const names = app.helpSections().map(section => section[0]);
    assert.equal(names[0], 'Getting around');
    assert.equal(names[1], 'Patterns, tracks & instruments');
    assert.equal(names.at(-1), 'Maschine MK3 controller (in development)');
    assert.equal(new Set(names).size, names.length);
  });

  await test('native menu key equivalents are unique', () => {
    // crates/st-app/src/menus.rs: `keyed(title, command, shortcut)`; a Rust test there checks each one parses.
    const menus = fs.readFileSync('crates/st-app/src/menus.rs', 'utf8');
    const seen = new Map();
    for (const m of menus.matchAll(/\bkeyed\("([^"]+)", "[^"]+", "([^"]+)"\)/g)) {
      assert(!seen.has(m[2]), `${m[1]} and ${seen.get(m[2])} share ${m[2]}`);
      seen.set(m[2], m[1]);
    }
    assert(seen.size >= 20, 'parsed ' + seen.size);
  });

  console.log(`${count} UI shortcut and menu tests passed`);
})().catch(error => { console.error(error); process.exit(1); });
