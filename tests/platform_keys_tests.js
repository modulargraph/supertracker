'use strict';
// Shortcut labels away from a Mac: the Mac's symbols and names become Ctrl / Alt / Shift.
const assert = require('node:assert/strict');
const { translate, isMac, install } = require('../ui/platform-keys.js');

const cases = [
  ['⌘K', 'Ctrl+K'], ['⌘⇧Z', 'Ctrl+Shift+Z'], ['Save project · ⌘S', 'Save project · Ctrl+S'], ['⌥A automate', 'Alt+A automate'],
  ['⌥ ↑ / ↓ reorder', 'Alt+↑ / ↓ reorder'], ['⌘ + K', 'Ctrl+K'], ['⌃⌥⌘F', 'Ctrl+Alt+F'], ['⇧+Tab', 'Shift+Tab'],
  ['Cmd+K', 'Ctrl+K'], ['Cmd+Shift+E', 'Ctrl+Shift+E'], ['Option+Left/Right', 'Alt+Left/Right'], ['Command-Delete', 'Ctrl-Delete'],
  ['← → channel · ↑ ↓ adjust · Tab controls · ⌥A automate', '← → channel · ↑ ↓ adjust · Tab controls · Alt+A automate'],
  // Words that only look like key names, and text without shortcuts, stay.
  ['Command palette', 'Command palette'], ['Optional tail', 'Optional tail'], ['Options', 'Options'], ['Track 01', 'Track 01'], ['', ''],
];
for (const [from, to] of cases) assert.equal(translate(from), to, JSON.stringify(from));
assert.equal(translate(undefined), undefined);
console.log(`PASS ${cases.length} labels translate (symbols, runs of modifiers, names) and plain text is untouched`);

assert.equal(isMac('MacIntel'), true); assert.equal(isMac('macOS'), true); assert.equal(isMac('iPhone'), true);
assert.equal(isMac('Linux x86_64'), false); assert.equal(isMac('Win32'), false); assert.equal(isMac(''), false);
assert.equal(install({}, 'MacIntel'), false, 'nothing is installed on a Mac');
console.log('PASS the platform decides: a Mac keeps its symbols');
