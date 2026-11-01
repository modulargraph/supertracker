'use strict';
// Display contract for track groups on the MK3 screens (real app + controller, rendered display lists only):
// a collapsed group's Home column, the Group level on CHANNEL / PLUG-IN and group strips on MIXER stay inside
// the 480 × 272 screens and show the group by name.
const assert = require('node:assert/strict');
const { groupSession } = require('./helpers/mk3-session');
const text = ops => ops.filter(op => op.op === 'text').map(op => op.text);
function bounds(screens) {
  for (const ops of Object.values(screens)) for (const op of ops) {
    for (const key of ['x', 'x1', 'x2']) if (key in op) assert(op[key] >= 0 && op[key] <= 480, `${op.op} ${key}`);
    for (const key of ['y', 'y1', 'y2']) if (key in op) assert(op[key] >= 0 && op[key] <= 272, `${op.op} ${key}`);
    if ('w' in op) assert(op.x + op.w <= 480 && op.y + op.h <= 272, `${op.op} bounds`);
    if (op.op === 'text') { assert(op.y + op.size <= 272); if (op.width) assert(op.x + op.width <= 480); assert(!op.text.startsWith('Screen error:'), op.text); }
  }
}
const h = groupSession();
h.MK3.subscribe(() => {});
const frame = () => { const { screens } = h.MK3.frame(); bounds(screens); return screens; };

// Home: expanded members are columns; collapsed, one column labelled with the group name and member hits.
let screens = frame();
assert(text(screens.left).includes('Velvet bass'));
h.win.STApp.setCollapsed(h.group().id, true);
screens = frame();
assert(text(screens.left).includes('Drums'), 'collapsed column labelled with the group');
assert(!text(screens.left).includes('Glass keys'), 'members are hidden');
const summary = h.MK3.page().data.grid.rows.map(r => r.cells[0].summary).find(Boolean);
assert(summary && text(screens.left).includes(summary), 'member hits drawn in the collapsed column');
h.win.STApp.setCollapsed(h.group().id, false);

// CHANNEL · Group and PLUG-IN · Group.
h.press('channel'); h.press('display2');
screens = frame();
assert(text(screens.left).includes('Drums'));
assert(text(screens.right).some(line => /GROUP/i.test(line)), 'level breadcrumb lists Group');
assert(text(screens.left).some(line => /Group bus/i.test(line)));
h.press('plugin');
screens = frame();
assert.equal(h.MK3.page().data.level, 'group');
assert(text(screens.left).includes('Volume · pan'), 'Levels row of the group chain');

// MIXER: the group strip.
h.press('mixer');
screens = frame();
assert(text(screens.left).includes('Drums'));
console.log('PASS MK3 screens: collapsed group column, Group level on CHANNEL / PLUG-IN and group mixer strip stay in bounds');
