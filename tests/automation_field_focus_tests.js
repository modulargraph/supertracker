'use strict';
// Actual studio handlers with destructive DOM replacement. Native Tab's focus
// transfer is modeled explicitly; this does not claim OS/WebKit default testing.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'studio_navigation_tests.js'), 'utf8');
const base = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf("test('Left/Right")) + '\nreturn {harness,mountAutomation};')(require, __dirname);
function harness(points = [{row:0,value:.8},{row:16,value:.4}]) {
  const h = base.harness(), timers = new Map(), events = new Map(); let serial = 0, renders = 0;
  h.state.project.patterns[0].automation[0].points = points;
  h.context.setTimeout = (fn, delay = 0) => { timers.set(++serial, {fn,delay}); return serial; };
  h.context.clearTimeout = id => timers.delete(id);
  h.document.addEventListener = (type, fn) => { const list = events.get(type) || []; list.push(fn); events.set(type,list); };
  const render = base.mountAutomation(h), ST = h.context.window.ST;
  ST.refresh = () => { renders++; render(); };
  ST.mutate = (label, fn, options = {}) => { h.changes.push({label}); fn(h.state.project); if (options.render !== false) ST.refresh(); };
  const flush = () => { for (const [id,timer] of [...timers]) if (timer.delay === 0) { timers.delete(id); timer.fn(); } };
  function depart(fromID,toID,value) {
    const from = h.nodes.get(fromID), to = h.nodes.get(toID); from.focus(); from.value = String(value); from.fire('input');
    assert(!h.key(from,'Tab').defaultPrevented); h.document.activeElement = h.document.body; from.fire('change'); from.fire('blur');
    if (to.isConnected) to.focus(); return {from,to};
  }
  return {...h, ST, depart, flush, pointer(type,target) { for(const fn of events.get(type)||[]) fn({target}); }, get renders() {return renders;} };
}
{
  const h=harness();h.depart('point-row-0','point-value-0',24);
  assert.equal(h.document.activeElement.id,'point-value-0','row commit must leave native Tab destination attached');
  assert.deepEqual(Array.from(h.state.project.patterns[0].automation[0].points,p=>p.row),[16,24]);assert.equal(h.changes.length,1);
  h.flush();assert.equal(h.document.activeElement.id,'point-value-1','focus follows the moved point, not its old array index');
  console.log('PASS row commit preserves Tab and follows the same point after sorting');
}
{
  const h=harness();h.depart('point-value-1','point-row-1',.65);h.flush();assert.equal(h.document.activeElement.id,'point-row-1');
  assert.equal(h.state.project.patterns[0].automation[0].points[1].value,.65);
  const input=h.nodes.get('point-value-1');input.focus();input.value='.7';input.fire('input');input.fire('change');h.flush();assert.equal(h.document.activeElement.id,'point-value-1');
  console.log('PASS backward departure and changes retaining focus preserve their intended fields');
}
{
  const h=harness();h.depart('point-row-0','point-value-0',24);const input=h.nodes.get('point-value-0');input.value='.93';input.fire('input');const before=h.renders;h.flush();
  assert.equal(h.renders,before);assert.equal(h.document.activeElement,input);assert.equal(input.value,'.93');
  h.document.activeElement=h.document.body;input.fire('change');input.fire('blur');h.nodes.get('point-row-1').focus();h.flush();
  assert.deepEqual(Array.from(h.state.project.patterns[0].automation[0].points,p=>[p.row,p.value]),[[16,.4],[24,.93]]);
  assert.equal(h.document.activeElement.id,'point-row-0','next visible row follows its original point when the prior point moved');
  console.log('PASS an unfinished destination draft survives pending paint and edits the correct moved point');
}
{
  for(const change of [h=>h.state.view='mixer',h=>h.state.modal='dialog',h=>h.state.project=JSON.parse(JSON.stringify(h.state.project)),h=>h.state.selectedLane=1]){
    const h=harness();h.depart('point-row-0','point-value-0',8);const before=h.renders;change(h);h.flush();assert.equal(h.renders,before);
  }
  console.log('PASS deferred automation paint cannot redraw a newer project, lane, view or modal');
}
{
  const h=harness();const source=h.nodes.get('point-row-0'),target=h.groups.get('[data-remove-point]')[0];source.focus();source.value='24';source.fire('input');
  h.pointer('pointerdown',target);h.document.activeElement=h.document.body;source.fire('change');source.fire('blur');target.focus();const before=h.renders;h.flush();assert.equal(h.renders,before);
  h.pointer('pointerup',target);target.fire('click');h.flush();assert.deepEqual(Array.from(h.state.project.patterns[0].automation[0].points,p=>p.row),[16]);
  console.log('PASS occasional Remove click survives blur and acts on the original point after reordering');
}
{
  const h=harness();h.depart('point-row-0','point-value-0',16);h.flush();assert.equal(h.state.project.patterns[0].automation[0].points.length,1);assert.equal(h.document.activeElement.id,'point-value-0');
  console.log('PASS merging a breakpoint into an occupied row keeps the surviving value editor focused');
}
{
  // Rows are stored to 1/64 (recorded takes, a typed 9.5): the edited point keeps its selection and Tab's destination.
  const h=harness([{row:0,value:.8},{row:16,value:.4},{row:24,value:.6}]);h.depart('point-row-2','point-value-2',9.5);
  assert.deepEqual(Array.from(h.state.project.patterns[0].automation[0].points,p=>p.row),[0,9.5,16]);
  h.flush();assert.equal(h.document.activeElement.id,'point-value-1','focus follows the point to its new place, not the point that took its old index');
  const input=h.nodes.get('point-value-1');input.value='.3';input.fire('input');input.fire('change');h.flush();
  assert.deepEqual(Array.from(h.state.project.patterns[0].automation[0].points,p=>p.value),[.8,.3,.4],'the value typed next edits the same point');
  console.log('PASS a fractional row keeps focus and the next edit on the same point');
}
{
  // An emptied field is no edit: it used to become row 0 (replacing the point there) or the lowest value.
  const h=harness([{row:0,value:.8},{row:16,value:.4}]),before=JSON.stringify(h.state.project.patterns[0].automation[0].points);
  h.depart('point-row-1','point-value-1','');h.flush();
  assert.equal(JSON.stringify(h.state.project.patterns[0].automation[0].points),before);assert.equal(h.changes.length,0);
  assert.equal(h.nodes.get('point-row-1').value,'16','the field shows the stored row again');
  const value=h.nodes.get('point-value-1');value.focus();value.value='';value.fire('input');value.fire('change');h.flush();
  assert.equal(JSON.stringify(h.state.project.patterns[0].automation[0].points),before);assert.equal(h.changes.length,0);
  console.log('PASS an emptied row or value field leaves the point unchanged');
}
{
  // A key that cannot change the lane (← on a point at row 0, → at the last row) adds no Undo step.
  const h=harness([{row:0,value:.8},{row:63,value:.4}]),svg=h.nodes.get('automation-envelope'),rows=()=>Array.from(h.state.project.patterns[0].automation[0].points,p=>p.row);
  const press=key=>svg.fire('keydown',{key,preventDefault(){},stopPropagation(){}});
  press('Home');press('ArrowLeft');assert.deepEqual(rows(),[0,63]);assert.equal(h.changes.length,0,'no Undo step for a point already at the first row');
  press('ArrowRight');assert.deepEqual(rows(),[1,63]);assert.equal(h.changes.length,1);
  press('End');press('ArrowRight');assert.equal(h.changes.length,1,'no Undo step for a point already at the last row');
  console.log('PASS envelope keys that change nothing add no Undo step');
}
