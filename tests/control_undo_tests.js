'use strict';
// Reuse actual app rendering/bindings with destructive DOM replacement. The
// optional SUPERTRACKER_APP_SOURCE override is inherited for read-only baseline
// comparisons; no production function or history handler is replaced.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'scalar_field_focus_tests.js'), 'utf8');
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')) + '\nreturn harness;')(require, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
function harness(view = 'pattern') {
  const h = makeHarness(view), nativeEdits = [];
  h.document.execCommand = command => { nativeEdits.push(command); return true; };
  function key(key, extra = {}) {
    const event = { key, code: key === 'z' ? 'KeyZ' : key, target: h.document.activeElement, preventDefault() { this.defaultPrevented = true; }, ...extra };
    h.ST.keyboard(event); return event;
  }
  return { ...h, nativeEdits, key };
}
const tests = [], test = (name, run) => tests.push({ name, run });

test('Undo from the pattern Step select restores truncated notes and automation; Redo reapplies rounded length', () => {
  const h = harness(), pattern = h.ST.pattern();
  pattern.steps[0][6] = { note: 74, instrument: 0, velocity: 92, gate: .6, fx1: { type: 'pan', value: -.3 } };
  pattern.automation = [{ id: 'retained-lane', track: 0, target: 'cutoff', interpolation: 'linear', points: [{ row: 0, value: 800 }, { row: 6, value: 2300 }, { row: 7, value: 900 }] }];
  h.ST.refresh(); const original = copy(h.state.project);
  h.depart('pattern-length', 'edit-step', 3.5); h.flush();
  assert.equal(h.ST.pattern().length, 4); assert.equal(h.document.activeElement.id, 'edit-step');
  assert.deepEqual(copy(h.ST.pattern().automation[0].points), [{ row: 0, value: 800 }]); const shortened = copy(h.state.project);
  const undo = h.key('z', { metaKey: true });
  assert(undo.defaultPrevented, 'Cmd-Z on a SELECT must use project history');
  assert.deepEqual(copy(h.state.project), original); assert.equal(h.state.undo.length, 0); assert.equal(h.state.redo.length, 1); assert.equal(h.document.activeElement.id, 'edit-step');
  const redo = h.key('z', { metaKey: true, shiftKey: true });
  assert(redo.defaultPrevented); assert.deepEqual(copy(h.state.project), shortened); assert.equal(h.state.undo.length, 1); assert.equal(h.state.redo.length, 0); assert.equal(h.document.activeElement.id, 'edit-step');
  assert.deepEqual(h.nativeEdits, []); assert(!h.calls.some(call => /^(note\.|transport\.)/.test(call.command)));
});

test('selects, buttons, ranges, checkboxes and radios use project Undo/Redo for keyboard, command and perform entry points', () => {
  const controls = [
    ['select', 'pattern', h => h.get('edit-step')],
    ['button', 'pattern', h => h.get('metronome')],
    ['range', 'instrument', h => h.get('param-attack')],
    ['checkbox', 'song', h => h.get('loop-song')],
    ['radio', 'pattern', h => h.node('input', 'fixture-radio', h.document.body, { type: 'radio', attrs: { type: 'radio' } })],
  ];
  for (const [kind, view, control] of controls) for (const entry of ['keyboard', 'command', 'perform']) {
    const h = harness(view), originalTempo = h.state.project.bpm;
    h.ST.mutate('set tempo', project => { project.bpm = 132; }); control(h).focus();
    const dispatch = redo => {
      if (entry === 'keyboard') assert(h.key('z', { metaKey: true, shiftKey: redo }).defaultPrevented, `${kind} ${entry}`);
      else if (entry === 'command') h.window.nativeEvent({ type: 'command', payload: { name: redo ? 'redo' : 'undo' } });
      else h.ST.perform(redo ? 'redo' : 'undo');
    };
    dispatch(false); assert.equal(h.state.project.bpm, originalTempo, `${kind} ${entry} Undo`); assert.equal(h.state.undo.length, 0); assert.equal(h.state.redo.length, 1);
    dispatch(true); assert.equal(h.state.project.bpm, 132, `${kind} ${entry} Redo`); assert.equal(h.state.undo.length, 1); assert.equal(h.state.redo.length, 0);
    assert.deepEqual(h.nativeEdits, [], kind); assert(!h.calls.some(call => /^(note\.|transport\.)/.test(call.command)));
  }
});

test('editable text, number, search, textarea and contenteditable drafts keep native Undo/Redo', () => {
  const editors = [
    ['text', h => h.node('input', 'draft-text', h.document.body, { type: 'text', attrs: { type: 'text' } })],
    ['number', h => h.get('tempo')],
    ['search', h => h.node('input', 'draft-search', h.document.body, { type: 'search', attrs: { type: 'search' } })],
    ['textarea', h => h.node('textarea', 'draft-textarea', h.document.body)],
    ['contenteditable descendant', h => { const editor = h.node('div', 'draft-editable', h.document.body, { attrs: { contenteditable: 'true' }, tabIndex: 0 }); const child = h.node('span', 'draft-span', editor, { tabIndex: 0 }); editor.isContentEditable = child.isContentEditable = true; return child; }],
  ];
  for (const [kind, create] of editors) {
    const h = harness(); h.ST.mutate('set tempo', project => { project.bpm = 132; }); const before = copy(h.state.project), input = create(h); input.focus(); input.value = kind === 'number' ? '145' : 'Uncommitted draft';
    for (const shiftKey of [false, true]) {
      const event = h.key('z', { metaKey: true, shiftKey }); assert(!event.defaultPrevented, `${kind} keeps browser keyboard Undo/Redo`);
      assert.deepEqual(copy(h.state.project), before); assert.equal(h.state.undo.length, 1); assert.equal(h.state.redo.length, 0);
    }
    assert.deepEqual(h.nativeEdits, [], 'Unprevented keyboard events are handled by the browser itself');
    h.ST.perform('undo'); h.ST.perform('redo'); assert.deepEqual(h.nativeEdits, ['undo', 'redo'], kind);
    h.window.nativeEvent({ type: 'command', payload: { name: 'undo' } }); h.window.nativeEvent({ type: 'command', payload: { name: 'redo' } });
    assert.deepEqual(h.nativeEdits, ['undo', 'redo', 'undo', 'redo']); assert.deepEqual(copy(h.state.project), before); assert.equal(h.document.activeElement, input);
    assert.equal(h.calls.length, 0, 'Draft Undo must not send native audio requests');
  }
});

test('a programmatically focused, unedited number field (sample Start after a recording) uses project Undo/Redo', () => {
  for (const entry of ['keyboard', 'command', 'perform']) {
    const h = harness('instrument'), tracks = h.state.project.tracks.length;
    h.ST.mutate('Record sampler instrument', project => { project.tracks.push({ ...copy(project.tracks[0]), id: tracks, name: 'Recorded lane' }); });
    // Mirrors recording.js attach(): refresh, then focus the freshly rendered Start field.
    // A real DOM field rendered with value="…" reports that markup as defaultValue;
    // the fixture DOM does not, so restate it for every re-rendered field.
    const focusStart = value => {
      const field = h.get('param-value-start'); assert(field, 'instrument view renders the sample Start number field');
      field.type = 'number'; field.defaultValue = '0'; field.value = value; field.focus(); assert.equal(h.document.activeElement, field);
    };
    const dispatch = redo => {
      focusStart('0');
      if (entry === 'keyboard') assert(h.key('z', { metaKey: true, shiftKey: redo }).defaultPrevented, `${entry} must not fall through to empty text history`);
      else if (entry === 'command') h.window.nativeEvent({ type: 'command', payload: { name: redo ? 'redo' : 'undo' } });
      else assert.notEqual(h.ST.perform(redo ? 'redo' : 'undo'), false, entry);
    };
    dispatch(false); assert.equal(h.state.project.tracks.length, tracks, `${entry} Undo removes the recorded lane`); assert.equal(h.state.redo.length, 1);
    dispatch(true); assert.equal(h.state.project.tracks.length, tracks + 1, `${entry} Redo restores the recorded lane`); assert.equal(h.state.undo.length, 1);
    assert.deepEqual(h.nativeEdits, [], entry);
    // Once the user types a draft, the same field keeps native text Undo.
    focusStart('0.25');
    assert(!h.key('z', { metaKey: true }).defaultPrevented); h.ST.perform('undo'); assert.deepEqual(h.nativeEdits, ['undo']); assert.equal(h.state.project.tracks.length, tracks + 1);
  }
});

test('a focused select keeps native typeahead and arrows and never plays keyboard-piano notes', () => {
  const h = harness(), select = h.get('edit-step'), before = copy(h.state.project); select.focus(); h.state.keyboardPlay = true; h.state.recording = true;
  for (const key of ['z', 'q', 'ArrowDown', 'ArrowUp']) assert(!h.key(key).defaultPrevented, key);
  assert.equal(h.document.activeElement, select); assert.deepEqual(copy(h.state.project), before); assert.equal(h.state.undo.length, 0); assert.equal(h.calls.length, 0);
});

let failures = 0;
for (const { name, run } of tests) { try { run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } }
console.log(`Control Undo: ${tests.length - failures}/${tests.length} regression groups passed.`); if (failures) process.exitCode = 1;
