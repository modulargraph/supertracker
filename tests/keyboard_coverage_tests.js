'use strict';
// Every action reachable from the keyboard: palette commands are handled by perform(), their shortcuts are
// documented in the help sheet, native menu items forward known commands, and menu key equivalents match.
const assert = require('node:assert/strict'), fs = require('node:fs');
const web = require('./helpers/web');
const swift = fs.readFileSync('Sources/SuperTracker/App.swift', 'utf8');
const { app, ST } = web.app();
const source = web.source('app.js'), compact = source.replace(/\s+/g, '');
const commands = JSON.parse(JSON.stringify(ST.commands));
const helpKeys = JSON.parse(JSON.stringify(app.helpSections())).flatMap(([, rows]) => rows.map(row => row[1]));
const samplerSource = web.read('sampler.js'), samplerCommand = samplerSource.slice(samplerSource.indexOf('function command(name)'), samplerSource.indexOf('window.STSampler ='));

function handled(id) {
  if (id.startsWith('view:')) return /^view:(pattern|instrument|browser|mixer|automation|song|recording)$/.test(id);
  if (id.startsWith('instrumentType:')) return ['synth', 'sample', 'au', 'midi'].includes(id.slice(15));
  if (id.startsWith('sample:')) return id === 'sample:editor' || samplerCommand.includes(`case '${id.slice(7)}'`);
  return app.hasCommand(id);
}

{
  const ids = commands.map(c => c[0]);
  assert.deepEqual(ids.filter((id, i) => ids.indexOf(id) !== i), [], 'palette command ids are unique');
  const missing = ids.filter(id => !handled(id));
  assert.deepEqual(missing, [], 'every palette command is handled by perform()');
  for (const c of commands) { assert(c[1] && typeof c[1] === 'string', c[0]); assert.equal(typeof c[2], 'string', c[0]); }
  console.log(`PASS ${ids.length} palette commands are unique and each one is handled by perform()`);
}
{
  // Regression: these once fell through into the automation write toggle.
  for (const id of ['muteTrack', 'soloTrack', 'clearMuteSolo']) assert.equal(app.commandHandler(id), app.patternTool, `${id} routes to pattern tools`);
  assert.notEqual(app.commandHandler('automationWriteToggle'), app.patternTool);
  console.log('PASS mute, solo and clear mute/solo reach the pattern tools, not automation write');
}
{
  const tokens = new Set(helpKeys.flatMap(key => key.split(/ \/ | · | /)).map(token => token.trim()).filter(Boolean));
  const normalize = key => key === 'Esc' ? 'Escape' : key;
  const undocumented = commands.filter(c => c[2] && !tokens.has(normalize(c[2]))).map(c => `${c[0]} ${c[2]}`);
  assert.deepEqual(undocumented, [], 'every palette shortcut appears in the help sheet');
  console.log(`PASS every palette shortcut is listed in the help sheet (${tokens.size} help keys)`);
}
{
  // The same key may serve different focused contexts (pattern grid, song blocks, clip timeline, sample editor),
  // but never two global commands.
  const contextual = /^(Song|Sample editor|Audio clip|Automation):/;
  const byKey = new Map();
  for (const c of commands) if (c[2]) byKey.set(c[2], [...(byKey.get(c[2]) || []), c]);
  const clashes = [...byKey].filter(([, list]) => list.filter(c => !contextual.test(c[1])).length > 1).map(([key, list]) => key + ': ' + list.map(c => c[0]).join(', '));
  assert.deepEqual(clashes, [], 'no two global palette commands share a shortcut');
  console.log('PASS no two global palette commands share a shortcut');
}
{
  const symbols = { command: '⌘', shift: '⇧', option: '⌥', control: '⌃' };
  const items = [...swift.matchAll(/add\(\w+, "([^"]+)", ("view:" \+ command|"[^"]*"), ("[^"]*"|f6|String\(index \+ 1\))(?:, modifiers: \[?([^\])]*)\]?)?\)/g)];
  assert(items.length > 60, 'menu items parsed: ' + items.length);
  const ids = new Set(commands.map(c => c[0]));
  const menuIDs = items.filter(m => m[2].startsWith('"')).map(m => m[2].slice(1, -1));
  const unknown = menuIDs.filter(id => !(ids.has(id) || id === 'palette') || !(handled(id) || id === 'palette'));
  assert.deepEqual(unknown, [], 'menu commands are palette commands handled by perform()');
  assert(app.hasCommand('palette'));
  for (const group of ['File', 'Edit', 'View', 'Transport', 'Pattern', 'Track', 'Instrument', 'Window', 'Help']) assert(swift.includes(`submenu("${group}")`), group);
  const helpText = helpKeys.join(' ');
  for (const m of items) {
    const key = m[3];
    if (key === '""') continue;
    const mods = m[4] === undefined ? ['command'] : m[4].split(',').map(x => x.trim().replace('.', '')).filter(Boolean);
    const glyph = key === 'f6' ? 'F6' : key.startsWith('String') ? null : key.slice(1, -1).toUpperCase();
    if (!glyph) continue;
    const shortcut = mods.map(x => symbols[x]).join('') + glyph;
    const id = m[2].slice(1, -1), palette = commands.find(c => c[0] === id)?.[2] || '';
    const same = (a, b) => [...a].sort().join('') === [...b].sort().join('');
    assert(same(palette, shortcut) || helpText.split(/ \/ | · | /).some(token => same(token, shortcut)), `menu ${m[1]} ${shortcut} matches palette (${palette}) or help`);
  }
  console.log(`PASS ${items.length} native menu items forward handled commands; key equivalents match palette/help`);
}
{
  // Native menu commands arrive only when the page left the key alone; while a dialog is open only
  // editing/save/stop commands run, so a menu shortcut never acts behind a dialog.
  assert(compact.includes("if(state.modal&&!MODAL_MENU_COMMANDS.has(name))"), 'native menu commands wait for dialogs');
  // ⌃⌘F belongs to native full screen, not the web find shortcut.
  assert(compact.includes("key==='f'&&!(e.metaKey&&e.ctrlKey)"));
  for (const [id, [view, selector]] of Object.entries(app.CONTROL_COMMANDS)) {
    const source = web.read({ mixer: 'studio.js', automation: 'studio.js', song: 'clip-arranger.js' }[view]);
    const needle = selector.startsWith('#') ? `id="${selector.slice(1)}"` : selector.slice(1, -1);
    assert(source.includes(needle), `${id} control ${selector} exists in the ${view} view`);
  }
  console.log('PASS dialogs block background menu commands, full screen stays native, palette controls exist');
}
