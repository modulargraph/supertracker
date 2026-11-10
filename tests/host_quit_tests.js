'use strict';
// The page's side of the quit handshake: the host sends `app.quitRequested`, the page answers with
// `app.quitAnswer` (deciding, then stay or proceed with the final project). See st_host::Lifecycle.
const assert = require('node:assert/strict'), vm = require('node:vm');
const web = require('./helpers/web');
const { hostStub } = require('./helpers/host');

function harness() {
  const nodes = new Map(), calls = [], handlers = new Map();
  function element(id) {
    if (!nodes.has(id)) nodes.set(id, { id, innerHTML: '', value: '', dataset: {}, attributes: {}, classList: { toggle() {}, add() {}, remove() {} },
      focus() {}, select() {}, scrollIntoView() {}, closest() { return null; }, querySelectorAll() { return []; }, addEventListener() {},
      setAttribute(name, value) { this.attributes[name] = String(value); }, click() { return this.onclick?.(); } });
    return nodes.get(id);
  }
  const document = { body: element('body'), activeElement: null, querySelector: element, querySelectorAll: () => [], getElementById: element, addEventListener() {}, createElement: element };
  const window = { addEventListener() {}, STHostTransport: hostStub({ post(message) {
    calls.push(message);
    Promise.resolve().then(() => handlers.get(message.command)?.(message.payload) ?? {})
      .then(result => window.STHostTransport.deliver({ id: message.id, ok: true, result }), error => window.STHostTransport.deliver({ id: message.id, ok: false, error: error.message }));
  } }) };
  const context = vm.createContext({ window, document, console, setTimeout: () => 0, clearTimeout() {}, Date, Map, Set, Math, JSON, Number, String, Object, Array, Promise, innerWidth: 1280, innerHeight: 800 });
  web.load(context, 'app.js');
  web.pendingEditsStub(window, 'nksControls', 'STNKSControls', 30);
  const ST = window.ST;
  ST.state.project = ST.demoProject(); ST.state.project.name = 'Last session'; ST.state.engineReady = true;
  const answers = () => calls.filter(call => call.command === 'app.quitAnswer').map(call => call.payload);
  const ask = () => window.nativeEvent({ type: 'app.quitRequested', payload: {} });
  return { ST, window, calls, handlers, element, answers, ask };
}
async function settle() { for (let i = 0; i < 40; i++) await Promise.resolve(); }

(async () => {
  {
    const h = harness(); h.ST.state.dirty = false;
    h.ask(); await settle();
    const answers = h.answers();
    assert.deepEqual(answers.map(answer => answer.state), ['deciding', 'proceed']);
    assert.equal(answers[1].project.name, 'Last session');
    assert.equal(h.ST.state.modal, null, 'a saved session quits without a question');
    h.ask(); await settle();
    assert.equal(h.answers().length, 4, 'the next request is answered afresh');
    console.log('PASS a saved session answers deciding, then proceed with its final project');
  }
  {
    const h = harness(); h.ST.state.dirty = true;
    h.ask(); h.ask(); await settle();
    assert.deepEqual(h.answers().map(answer => answer.state), ['deciding'], 'a second request while deciding is ignored');
    assert.equal(h.ST.state.modal, 'unsaved');
    assert.equal(h.ST.isChangingSession, true, 'no other session change may start while the question is open');
    h.element('#unsaved-cancel').click(); await settle();
    assert.deepEqual(h.answers().map(answer => answer.state), ['deciding', 'stay']);
    assert.equal(h.ST.isChangingSession, false);
    assert.equal(h.calls.some(call => call.command === 'project.save'), false);
    console.log('PASS unsaved changes ask Save / Discard / Cancel, and Cancel keeps the application running');
  }
  {
    const h = harness(); h.ST.state.dirty = true;
    h.ask(); await settle(); h.element('#unsaved-discard').click(); await settle();
    const answers = h.answers();
    assert.deepEqual(answers.map(answer => answer.state), ['deciding', 'proceed']);
    assert.equal(answers[1].project.name, 'Last session', 'discarded changes still reach the recovery copy');
    assert.equal(h.calls.some(call => call.command === 'project.save'), false);
    console.log('PASS Discard quits without saving, with the project handed to the host for its recovery copy');
  }
  {
    const h = harness(); h.ST.state.dirty = true;
    h.handlers.set('project.save', payload => ({ saved: true, project: payload.project, path: '/tmp/last.supertracker' }));
    h.ask(); await settle(); h.element('#unsaved-save').click(); await settle();
    const order = h.calls.filter(call => ['project.save', 'app.quitAnswer'].includes(call.command)).map(call => call.command + (call.payload.state ? ':' + call.payload.state : ''));
    assert.deepEqual(order, ['app.quitAnswer:deciding', 'project.save', 'app.quitAnswer:proceed']);
    console.log('PASS Save saves first, then quits');
  }
  {
    const h = harness(); h.ST.state.dirty = true;
    h.handlers.set('project.save', () => ({ cancelled: true }));
    h.ask(); await settle(); h.element('#unsaved-save').click(); await settle();
    assert.deepEqual(h.answers().map(answer => answer.state), ['deciding', 'stay'], 'a cancelled Save window cancels quitting');
    console.log('PASS cancelling the Save window keeps the application running');
  }
  {
    const h = harness(); h.ST.state.dirty = false;
    h.window.STExport = { isBusy: true, event() {} };
    h.ask(); await settle();
    const answers = h.answers();
    assert.deepEqual(answers.map(answer => answer.state), ['deciding', 'stay']);
    assert.equal(answers[1].reason, 'Finish or cancel the audio export before quitting.');
    console.log('PASS an operation that must finish first refuses to quit, with its reason');
  }
  {
    const h = harness(); h.ST.state.dirty = false;
    let attempts = 0;
    h.handlers.set('app.quitAnswer', payload => {
      if (payload.state === 'proceed' && !payload.force && attempts++ === 0) throw new Error('Your latest changes could not be secured: the disk is full.');
      return {};
    });
    h.ask(); await settle();
    assert.equal(h.ST.state.modal, 'confirm', 'the page asks whether to quit anyway');
    h.element('#confirm-accept').click(); await settle();
    assert.deepEqual(h.answers().map(answer => [answer.state, !!answer.force]), [['deciding', false], ['proceed', false], ['proceed', true]]);
    const kept = harness(); kept.ST.state.dirty = false;
    kept.handlers.set('app.quitAnswer', payload => { if (payload.state === 'proceed') throw new Error('Your latest changes could not be secured.'); return {}; });
    kept.ask(); await settle(); kept.element('#confirm-cancel').click(); await settle();
    assert.deepEqual(kept.answers().map(answer => answer.state), ['deciding', 'proceed', 'stay']);
    console.log('PASS when the host cannot secure the session, the person chooses between Quit Anyway and staying');
  }
})().catch(error => { console.error(error); process.exit(1); });
