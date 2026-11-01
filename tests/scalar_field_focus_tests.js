'use strict';
// Run the actual app renderer, bindings, keyboard, history and engine scheduling.
// Reuse the mounted-selector VM fixture; parse every renderShell result and
// detach all replaced nodes so a lost native Tab/pointer target is observable.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'pane_navigation_tests.js'), 'utf8');
const exportLine = 'return { ST, state, window, document, calls, nodes, node, clear, sidebar, song, key,';
assert(fixture.includes(exportLine));
// A frozen baseline can be checked read-only with SUPERTRACKER_APP_SOURCE.
// The normal run always reads the working app.js without transforming its code.
const fixtureRequire = name => name === 'node:fs' ? { ...fs, readFileSync(filename, ...options) { return fs.readFileSync(process.env.SUPERTRACKER_APP_SOURCE && path.basename(String(filename)) === 'app.js' ? process.env.SUPERTRACKER_APP_SOURCE : filename, ...options); } } : require(name);
const makeHarness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('function unchanged(')).replace(exportLine, exportLine.replace('return {', 'return { context,')) + '\nreturn harness;')(fixtureRequire, __dirname);
const copy = value => JSON.parse(JSON.stringify(value));
const tick = async () => { for (let index = 0; index < 32; index++) await Promise.resolve(); };

function harness(view = 'pattern') {
  const h = makeHarness(view), { ST, state, document } = h, timers = new Map(), documentEvents = new Map(), windowEvents = new Map();
  let nextTimer = 0, renders = 0;
  h.context.setTimeout = (fn, delay = 0) => { timers.set(++nextTimer, { fn, delay }); return nextTimer; };
  h.context.clearTimeout = id => timers.delete(id); h.context.queueMicrotask = queueMicrotask;
  function addListener(map, type, fn, options = {}) { const list = map.get(type) || []; list.push({ fn, capture: options === true || options.capture === true }); map.set(type, list); }
  document.addEventListener = (type, fn, options) => addListener(documentEvents, type, fn, options);
  h.window.addEventListener = (type, fn, options) => addListener(windowEvents, type, fn, options);
  function enhance(element) {
    Object.assign(element, { events: new Map(), style: {}, textContent: '', value: '', min: '', max: '', step: '', scrollTop: 0, scrollLeft: 0 });
    element.classList = { contains: name => element.classes.has(name), add: name => element.classes.add(name), remove: name => element.classes.delete(name), toggle(name, force) { if (force === undefined ? !element.classes.has(name) : force) element.classes.add(name); else element.classes.delete(name); } };
    element.addEventListener = (type, fn) => { const list = element.events.get(type) || []; list.push(fn); element.events.set(type, list); };
    element.setAttribute = (name, value) => { element.attrs[name] = String(value); };
    element.removeAttribute = name => { delete element.attrs[name]; };
    element.fire = (type, extra = {}) => {
      const event = { type, target: element, currentTarget: element, preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...extra };
      for (const listener of documentEvents.get(type) || []) if (listener.capture) listener.fn(event);
      if (type === 'input') element.userEdited = true;
      if (type === 'change') { element.committedValue = element.value; element.userEdited = false; }
      for (let current = element; current; current = ['focus', 'blur'].includes(type) ? null : current.parentElement) {
        event.currentTarget = current; current['on' + type]?.(event);
        for (const fn of current.events?.get(type) || []) fn(event);
        if (event.stopped) break;
      }
      if (!event.stopped) for (const listener of documentEvents.get(type) || []) if (!listener.capture) listener.fn(event);
      return event;
    };
    element.blur = () => {
      if (document.activeElement !== element) return;
      document.activeElement = document.body;
      if (['INPUT', 'SELECT', 'TEXTAREA'].includes(element.tagName) && element.userEdited && element.value !== element.committedValue) element.fire('change');
      element.fire('blur'); element.fire('focusout');
    };
    element.focus = () => {
      if (!element.isConnected || element.disabled || document.activeElement === element) return;
      document.activeElement?.blur?.();
      if (!element.isConnected) return;
      document.activeElement = element; element.fire('focus'); element.fire('focusin');
    };
    element.select = () => {}; element.setSelectionRange = () => {}; element.setPointerCapture = () => {};
    element.getBoundingClientRect = () => ({ top: 0, left: 0, width: 100, height: 50 });
    element.click = () => { if (element.isConnected && !element.disabled) return element.fire('click'); };
    return element;
  }
  enhance(document.body);
  const create = (tag, id, parent, options) => enhance(h.node(tag, id, parent, options));
  const app = create('div', 'app'), toast = create('div', 'toast-region'), outside = create('button', 'outside');
  create('div', 'modal-root'); document.createElement = tag => create(tag, '', null);
  function parse(html) {
    renders++;
    for (const element of app.querySelectorAll('*')) element.isConnected = false;
    // The selector fixture intentionally supports only used CSS. Traverse nodes
    // directly for removal, including anonymous elements and text containers.
    function detach(root) { for (const child of root.children) { child.isConnected = false; detach(child); } }
    detach(app); app.children = [];
    if (document.activeElement?.isConnected === false) document.activeElement = document.body;
    const stack = [app]; let anonymous = 0;
    for (const match of html.matchAll(/<(\/)?([a-z][\w-]*)\b((?:"[^"]*"|'[^']*'|[^'">])*)>/gi)) {
      const tag = match[2].toLowerCase();
      if (match[1]) { while (stack.length > 1 && stack.pop().tagName !== tag.toUpperCase()) {} continue; }
      const attrs = {};
      for (const attribute of match[3].matchAll(/([\w:-]+)(?:="([^"]*)"|='([^']*)')?/g)) attrs[attribute[1]] = attribute[2] ?? attribute[3] ?? '';
      const element = create(tag, attrs.id || `render-${renders}-${anonymous++}`, stack.at(-1), { attrs, className: attrs.class || '', type: attrs.type || '', disabled: Object.hasOwn(attrs, 'disabled'), hidden: Object.hasOwn(attrs, 'hidden'), ...(attrs.tabindex !== undefined ? { tabIndex: Number(attrs.tabindex) } : {}) });
      for (const key of ['value', 'min', 'max', 'step']) if (attrs[key] !== undefined) element[key] = attrs[key];
      element.checked = Object.hasOwn(attrs, 'checked'); element.selected = Object.hasOwn(attrs, 'selected');
      if (tag === 'option') { const end = html.indexOf('</option>', match.index); element.textContent = html.slice(match.index + match[0].length, end); if (!Object.hasOwn(attrs, 'value')) element.value = element.textContent; }
      if (!['input', 'br', 'hr', 'img', 'meta', 'link', 'path'].includes(tag) && !/\/\s*$/.test(match[3])) stack.push(element);
    }
    for (const select of app.querySelectorAll('select')) { const options = select.querySelectorAll('option'); select.value = (options.find(option => option.selected) || options[0])?.value || ''; }
    for (const input of app.querySelectorAll('input,select,textarea')) input.committedValue = input.value;
  }
  let renderedHTML = '';
  Object.defineProperty(app, 'innerHTML', { get: () => renderedHTML, set(html) { renderedHTML = html; parse(html); } });
  Object.assign(state, { project: ST.demoProject(), view, keyboardPlay: false, recording: false, engineReady: true });
  // Keep production markup realistic while avoiding hundreds of irrelevant cells.
  for (const pattern of state.project.patterns) { pattern.length = 8; pattern.steps.forEach(steps => { steps.length = 8; }); }
  state.project.instruments[0].type = 'sample'; state.project.instruments[0].samplePath = '/fixture/sample.wav';
  // The Gate field is editable while the optional Gate column is shown (a saved view preference).
  state.project.viewState = { gateColumn: true };
  ST.refresh(); h.calls.length = 0; timers.clear();
  function get(id) { const element = document.getElementById(id); assert(element, `Missing mounted field ${id}`); return element; }
  function key(input, key, options = {}) { const event = input.fire('keydown', { key, code: key, ...options }); if (!event.stopped) ST.keyboard(event); return event; }
  function edit(id, value) { const input = get(id); input.focus(); input.value = String(value); input.fire('input'); return input; }
  function depart(id, targetID, value, shiftKey = false) {
    const input = edit(id, value), destination = get(targetID), event = key(input, 'Tab', { shiftKey });
    assert(!event.defaultPrevented, 'The browser retains native Tab navigation');
    input.blur(); if (destination.isConnected) destination.focus(); return { input, destination };
  }
  function flush(delay = 0) { for (const [id, timer] of [...timers]) if (timer.delay === delay) { timers.delete(id); timer.fn(); } }
  return { ...h, timers, app, toast, outside, get, key, edit, depart, flush, get renders() { return renders; }, pointer(type, target) { target.fire(type); } };
}

const tests = [], test = (name, run) => tests.push({ name, run });
test('scalar Tab commits model/history immediately while keeping the actual next destination connected', async () => {
  const cases = [
    ['pattern', 'tempo', 'metronome', 137, h => h.state.project.bpm],
    ['pattern', 'pattern-length', 'edit-step', 12, h => h.ST.pattern().length],
    ['pattern', 'step-note', 'step-instrument', 67, h => h.ST.pattern().steps[0][0].note],
    ['pattern', 'step-velocity', 'step-gate', 77, h => h.ST.pattern().steps[0][0].velocity],
    ['instrument', 'param-value-attack', 'param-attack', .23, h => h.state.project.instruments[0].attack],
    ['instrument', 'param-value-rootNote', 'param-rootNote', 72, h => h.state.project.instruments[0].rootNote],
    ['song', 'signature-numerator', 'signature-denominator', 7, h => h.state.project.timeSignature.numerator],
  ];
  for (const [view, source, target, value, read] of cases) {
    const h = harness(view), original = copy(h.state.project), renders = h.renders;
    const { destination } = h.depart(source, target, value);
    assert.equal(read(h), value, source); assert.equal(h.state.undo.length, 1, source); assert.deepEqual(copy(h.state.undo[0].project), original);
    assert(h.state.dirty); assert([...h.timers.values()].some(timer => timer.delay === 100), source + ' must schedule engine sync');
    assert.equal(h.renders, renders, source + ' must not repaint during native departure'); assert.equal(h.document.activeElement, destination, source);
    h.flush(); assert.equal(h.document.activeElement.id, target, source); assert.equal(h.state.undo.length, 1);
    h.flush(100); await tick(); const sync = h.calls.find(call => call.command === 'engine.project'); assert(sync, source + ' must publish the committed model'); assert.equal(h.calls.filter(call => /^(note\.|transport\.)/.test(call.command)).length, 0);
  }
});
test('backward Tab and range changes retain their intended focus without extra edits', () => {
  const h = harness(); h.depart('step-gate', 'step-velocity', 1.2, true); assert.equal(h.document.activeElement.id, 'step-velocity'); h.flush(); assert.equal(h.document.activeElement.id, 'step-velocity'); assert.equal(h.ST.pattern().steps[0][0].gate, 1.2); assert.equal(h.state.undo.length, 1);
  const instrument = harness('instrument'), range = instrument.edit('param-attack', .18); range.fire('change'); assert.equal(instrument.state.project.instruments[0].attack, .18); instrument.flush(); assert.equal(instrument.document.activeElement.id, 'param-attack'); assert.equal(instrument.get('param-value-attack').value, '0.18'); assert.equal(instrument.state.undo.length, 1);
});
test('pattern length rounds fractional input and invalid numeric drafts never create history or native changes', () => {
  const fractional = harness(); fractional.depart('pattern-length', 'edit-step', 3.5);
  assert.equal(fractional.ST.pattern().length, 4); assert(fractional.ST.pattern().steps.every(steps => steps.length === 4)); assert.equal(fractional.state.undo.length, 1); fractional.flush(); assert.equal(fractional.get('pattern-length').value, '4'); assert.equal(fractional.document.activeElement.id, 'edit-step');
  for (const [view, id] of [['pattern', 'tempo'], ['pattern', 'pattern-length'], ['pattern', 'step-note'], ['pattern', 'step-velocity'], ['pattern', 'step-gate'], ['instrument', 'param-value-attack'], ['instrument', 'param-value-rootNote'], ['song', 'signature-numerator']]) {
    // An empty Gate field is a value: no gate (covered below).
    for (const value of id === 'step-gate' ? ['NaN', 'Infinity'] : ['', 'NaN', 'Infinity']) {
      const h = harness(view), before = copy(h.state.project), oldValue = h.get(id).value, input = h.edit(id, value); input.fire('change');
      assert.deepEqual(copy(h.state.project), before, id + ' rejects ' + JSON.stringify(value)); assert.equal(h.state.undo.length, 0); assert.equal(h.state.dirty, false); assert(![...h.timers.values()].some(timer => timer.delay === 100));
      h.flush(); assert.equal(h.document.activeElement.id, id); assert.equal(h.get(id).value, oldValue); assert.equal(h.calls.length, 0);
    }
  }
});
test('an empty Gate field removes the gate: the note plays until the next note, OFF or CUT', () => {
  const h = harness(); h.ST.pattern().steps[0][0] = { note: 60, instrument: 0, velocity: 100, gate: 2 }; h.ST.refresh(); h.flush();
  assert.equal(h.get('step-gate').value, '2');
  const input = h.edit('step-gate', ''); input.fire('change');
  assert.equal(h.ST.pattern().steps[0][0].gate, 0); assert.equal(h.ST.stepGate(h.ST.pattern().steps[0][0]), null); assert.equal(h.state.undo.length, 1);
  h.outside.focus(); h.flush(); assert.equal(h.get('step-gate').value, ''); assert.equal(h.get('step-gate').attrs?.placeholder ?? '—', '—');
});
test('pending repaint protects a newly focused numeric draft until it is committed', () => {
  const h = harness(); h.depart('step-velocity', 'step-gate', 82); assert.equal(h.document.activeElement.id, 'step-gate');
  assert([...h.timers.values()].some(timer => timer.delay === 0)); const input = h.edit('step-gate', ''); const renders = h.renders; h.flush();
  assert.equal(h.renders, renders); assert.equal(h.document.activeElement, input); assert.equal(input.value, ''); assert.equal(h.state.undo.length, 1);
  input.value = '1.5'; input.fire('input'); input.fire('change'); assert.equal(h.ST.pattern().steps[0][0].gate, 1.5); h.outside.focus(); h.flush(); assert.equal(h.document.activeElement, h.outside); assert.equal(h.get('step-gate').value, '1.5'); assert.equal(h.state.undo.length, 2);
});
test('pointer departure preserves a real metronome button until its click is delivered', () => {
  const h = harness(), input = h.edit('tempo', 132), destination = h.get('metronome'); h.pointer('pointerdown', destination); input.blur(); if (destination.isConnected) destination.focus();
  const renders = h.renders; h.flush(); assert.equal(h.renders, renders); assert(destination.isConnected, 'Scalar repaint must not destroy a held pointer target');
  h.pointer('pointerup', destination); destination.click(); assert.equal(h.state.project.metronome, true); assert.equal(h.state.project.bpm, 132); assert.equal(h.state.undo.length, 2); h.flush(); assert.equal(h.document.activeElement.id, 'metronome');
});
test('Enter in a changed step field commits synchronously and returns to the tracker', () => {
  const h = harness(), input = h.edit('step-note', 69), event = h.key(input, 'Enter');
  assert(event.defaultPrevented); assert.equal(h.ST.pattern().steps[0][0].note, 69); assert.equal(h.state.undo.length, 1); assert.equal(h.document.activeElement.id, 'tracker'); h.flush(); assert.equal(h.document.activeElement.id, 'tracker'); assert.equal(h.calls.length, 0);
});
test('opening a keyboard modal still commits the owned field on native blur without repainting or stealing dialog focus', () => {
  const h = harness(), input = h.edit('tempo', 132), before = copy(h.state.project), renders = h.renders;
  // openModal marks state.modal before its deferred focus transfer. Model that
  // ordering without substituting any scalar binding or change handler.
  h.state.modal = 'palette'; input.blur(); h.outside.focus();
  assert.equal(h.state.project.bpm, 132); assert.equal(h.state.undo.length, 1); assert.deepEqual(copy(h.state.undo[0].project), before);
  assert([...h.timers.values()].some(timer => timer.delay === 100));
  h.flush(); assert.equal(h.renders, renders); assert.equal(h.document.activeElement, h.outside); assert.equal(h.state.modal, 'palette');
});
test('stale session, view, instrument, pattern or cell cannot receive deferred paint or steal focus', () => {
  for (const [view, source, target, value, change] of [
    ['pattern', 'step-note', 'step-instrument', 67, h => { h.state.project = copy(h.state.project); }],
    ['pattern', 'step-note', 'step-instrument', 67, h => { h.state.view = 'song'; }],
    ['pattern', 'step-note', 'step-instrument', 67, h => { h.state.row = 1; }],
    ['pattern', 'step-note', 'step-instrument', 67, h => { h.state.track = 1; }],
    ['pattern', 'step-note', 'step-instrument', 67, h => { h.state.project.activePattern = 1; }],
    ['instrument', 'param-value-attack', 'param-attack', .2, h => { h.state.instrument = 1; }],
    ['instrument', 'param-value-attack', 'param-attack', .2, h => { h.state.project.instruments[0] = { ...h.state.project.instruments[0] }; }],
    ['pattern', 'tempo', 'metronome', 132, h => { h.state.modal = 'dialog'; }],
  ]) {
    const h = harness(view); h.depart(source, target, value); assert([...h.timers.values()].some(timer => timer.delay === 0)); h.outside.focus(); const renders = h.renders; change(h); h.flush(); assert.equal(h.renders, renders); assert.equal(h.document.activeElement, h.outside); assert.equal(h.state.undo.length, 1);
  }
});

test('instrument arrow bursts: readable units, fine/coarse steps, one undo step per burst, focus kept', () => {
  const h = harness('instrument'), inst = h.state.project.instruments[0];
  h.context.Event = class { constructor(type) { this.type = type; } };
  const live = id => { const el = h.get(id); el.dispatchEvent = event => el.fire(event.type); return el; };
  const press = (el, key, extra = {}) => { const event = el.fire('keydown', { key, ...extra }); if (!event.stopped) h.ST.keyboard(event); return event; };
  // Sound groups on the Sample page; play mode is chosen in the Sample editor and named on the Region group.
  const html = h.app.innerHTML;
  for (const title of ['Region · Root note', 'Volume · Pan · Tune', 'Filter · Drive', 'Amp envelope']) assert(html.includes(`<h2>${title}</h2>`), title);
  for (const id of ['param-drive', 'param-bitDepth', 'param-tuneFine', 'param-start', 'param-end']) assert(html.includes(`id="${id}"`), id);
  inst.mode = 'reverse'; h.ST.refresh();
  assert.match(h.app.innerHTML, /<h2>Region · Root note<\/h2><span class="chip">BACKWARD LOOP<\/span>/);
  inst.cutoff = 1000; inst.attack = .05; inst.tune = 0; h.ST.refresh();
  assert.equal(h.get('param-cutoff').attrs['aria-valuetext'], '1.00 kHz');
  // Cutoff: three fine semitone steps and one coarse octave form ONE undo step.
  const undo = h.state.undo.length, cutoff = live('param-cutoff'); cutoff.focus();
  for (let i = 0; i < 3; i++) assert(press(cutoff, 'ArrowUp').defaultPrevented);
  press(cutoff, 'ArrowUp', { shiftKey: true });
  assert.equal(h.state.undo.length, undo, 'No history entry while the burst is live');
  assert.equal(Number(cutoff.value), Math.round(1000 * 2 ** .25 * 2)); assert.equal(h.get('param-value-cutoff').value, cutoff.value);
  h.flush(700); assert.equal(h.state.undo.length, undo + 1); assert.equal(inst.cutoff, Number(cutoff.value));
  h.flush(); assert.equal(h.document.activeElement.id, 'param-cutoff');
  // Attack: millisecond steps; Fine tune in cents carries the raw semitone value for automation.
  const attack = live('param-value-attack'); attack.focus(); press(attack, 'ArrowDown'); press(attack, 'ArrowDown', { shiftKey: true }); h.flush(700);
  assert.equal(inst.attack, .039); assert.equal(h.state.undo.length, undo + 2);
  h.flush(); const fine = live('param-value-tuneFine'); fine.focus(); for (let i = 0; i < 25; i++) press(fine, 'ArrowUp');
  assert.equal(fine.dataset.automateValue, '0.25'); assert.equal(h.ST.automateValue(fine), .25);
  press(fine, 'Tab'); // Any other key ends the burst before acting on it.
  assert.equal(inst.tune, .25); assert.equal(h.state.undo.length, undo + 3);
});

(async () => { let failures = 0; for (const { name, run } of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failures++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`Scalar field focus: ${tests.length - failures}/${tests.length} regression groups passed.`); if (failures) process.exitCode = 1; })();
