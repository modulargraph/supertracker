// What keeps the application from quitting: every operation that must finish or be cancelled first
// names itself in the reason the host is given (app-session.js quitBlocker, used by the quit handshake).
const web = require('./helpers/web');
const vm = require('node:vm');
const assert = require('node:assert/strict');

const window = {};
const context = vm.createContext({ window, document: {}, console, Map, Set, Date, Math, JSON, Number, String, Object, Array, Promise });
web.load(context, 'app.js');
const app = window.STApp;
window.ST.state.project = window.ST.demoProject();
assert.equal(app.quitBlocker(), null, 'An idle session may quit');
for (const [module, property, message] of [['STStudio', 'isMovingEffect', 'effect move'], ['STRouting', 'isApplying', 'routing change'],
    ['STTrackDuplicate', 'isApplying', 'track duplication'], ['STExport', 'isBusy', 'audio export'], ['STRecording', 'isBusy', 'audio recording'],
    ['STRecording', 'isRecording', 'audio recording'], ['STInstrumentEdits', 'isBusy', 'instrument or sample change'],
    ['STMK3', 'hasPendingEdits', 'Maschine control edit'], ['STAutomationRecord', 'hasPendingEdits', 'automation take'],
    ['STInputCapture', 'hasPendingEdits', 'input recording edge'], ['STNKSControls', 'hasPendingEdits', 'plug-in control edit'],
    ['STPluginParameters', 'hasPendingEdits', 'plug-in control edit'], ['STAgent', 'hasPendingEdits', 'plug-in control edit']]) {
  const previous = window[module];
  window[module] = { [property]: true };
  const reason = app.quitBlocker();
  assert.ok(reason && reason.includes(message), `${module}.${property} blocks quitting with a reason that names it (got ${reason})`);
  window[module][property] = false;
  assert.equal(app.quitBlocker(), null, 'An idle operation does not block quitting');
  if (previous === undefined) delete window[module]; else window[module] = previous;
}
app.savingProject = true;
assert.match(app.quitBlocker(), /save or project change/);
app.savingProject = false;
assert.equal(app.quitBlocker(), null);
console.log('PASS quit blocker: every busy operation blocks quitting with its own reason, and an idle session may quit');
