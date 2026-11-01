'use strict';
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const web = require('./helpers/web');
const fixture = fs.readFileSync(path.join(__dirname, 'scalar_field_focus_tests.js'), 'utf8');
const harness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('const tests = [], test =')) + '\nreturn harness;')(require, __dirname);
let count = 0;
function test(name, run) { run(); count++; console.log('PASS '+name); }
test('one click enables piano after an input retains Safari focus and the first key sounds', () => {
  const h=harness(), field=h.get('step-note'), button=h.get('keyboard-play'), tracker=h.get('tracker'), renders=h.renders;
  field.focus(); button.click();
  assert.equal(h.state.keyboardPlay,true); assert.equal(h.document.activeElement,tracker); assert.equal(button.isConnected,true); assert.equal(h.renders,renders);
  assert.equal(button.attrs['aria-pressed'],'true');
  h.key(h.document.activeElement,'z',{code:'KeyZ'});
  assert.equal(h.calls.filter(c=>c.command==='note.on').length,1);
  h.ST.noteOff('KeyZ');
});
test('successive pointer clicks toggle every time without replacing the button and disable releases held notes', () => {
  const h=harness(), button=h.get('keyboard-play');
  for(let i=0;i<6;i++) { button.click(); assert.equal(h.state.keyboardPlay,i%2===0); assert.equal(h.get('keyboard-play'),button); if(i===0)h.key(h.document.activeElement,'z',{code:'KeyZ'}); }
  assert.equal(h.calls.filter(c=>c.command==='note.off').length,1);
});
test('keyboard toggle command exits text entry when enabling and permits immediate piano input', () => {
  const h=harness(), field=h.get('step-note'); field.focus(); h.key(field,'p',{code:'KeyP',metaKey:true});
  assert.equal(h.state.keyboardPlay,true); assert.equal(h.document.activeElement,h.get('tracker'));
  h.key(h.document.activeElement,'z',{code:'KeyZ'}); assert.equal(h.calls.filter(c=>c.command==='note.on').length,1);
});
test('Record click from a retained numeric input focuses grid and the first key inserts and advances', () => {
  const h=harness(), field=h.get('step-note'); field.focus();
  h.get('record').click();
  assert.equal(h.state.recording,true); assert.equal(h.state.keyboardPlay,true); assert.equal(h.document.activeElement,h.get('tracker'));
  const row=h.state.row;
  h.key(h.document.activeElement,'z',{code:'KeyZ'});
  assert.equal(h.ST.pattern().steps[0][row].note,60); assert.equal(h.state.row,row+1);
  assert.equal(h.document.activeElement,h.get('tracker'));
});
test('Keys preview visibly explains record arming and does not overwrite notes', () => {
  const h=harness(), before=JSON.stringify(h.ST.pattern().steps);
  h.get('keyboard-play').click();
  assert.match(h.get('status-message').textContent,/preview.*⌘R.*enter notes/);
  h.key(h.document.activeElement,'z',{code:'KeyZ'}); h.ST.noteOff('KeyZ');
  assert.equal(JSON.stringify(h.ST.pattern().steps),before);
  h.get('record').click(); assert.match(h.get('status-message').textContent,/Step recording armed/);
});

test('OFF and CUT work in the note column with piano on or off without sounding a note',()=>{
 for(const enabled of [false,true])for(const cut of [false,true]){
  const h=harness();Object.assign(h.state,{keyboardPlay:enabled,view:'pattern',column:0,row:4});
  h.get('tracker').focus();h.key(h.get('tracker'),cut?'!':'1',{code:'Digit1',shiftKey:cut});
  assert.equal(h.ST.pattern().steps[h.state.track][4].note,cut?-2:-1);
  assert.equal(h.calls.filter(c=>c.command==='note.on').length,0);assert.equal(h.state.keyboardPlay,enabled);
 }
});

test('FX blur preserves the first Keys click and the edited step',()=>{
 const h=harness();for(const file of ['step-fx.js','pattern-destination.js'])web.load(h.context,file,{path:file==='step-fx.js'&&process.env.SUPERTRACKER_STEP_FX_SOURCE});
 const cell={note:60,instrument:0,velocity:100,fx1:{type:'pan',value:0}};h.ST.pattern().steps[0][0]=cell;h.ST.refresh();
 const field=h.get('step-fx1-value'),button=h.get('keyboard-play');field.focus();field.value='.7';field.fire('input');button.fire('pointerdown',{button:0});field.blur();button.fire('pointerup',{button:0});button.click();
 assert.equal(h.state.keyboardPlay,true);assert.equal(button.isConnected,true);assert.equal(h.document.activeElement,h.get('tracker'));assert.equal(cell.fx1.value,.7);
 h.key(h.document.activeElement,'z',{code:'KeyZ'});assert.equal(h.calls.filter(c=>c.command==='note.on').length,1);button.click();assert.equal(h.state.keyboardPlay,false);assert.equal(h.calls.filter(c=>c.command==='note.off').length,1);
 h.get('record').click();const row=h.state.row;h.key(h.document.activeElement,'x',{code:'KeyX'});assert(h.ST.pattern().steps[0][row]);assert.equal(h.state.row,row+1);
});

test('Record placement defaults to Selected step and visible control persists Live preference',()=>{
 const h=harness(),saved=new Map();h.context.localStorage={setItem:(key,value)=>saved.set(key,value)};assert.equal(h.state.recordPlacement,'selected');web.load(h.context, 'pattern-destination.js');h.ST.refresh();const field=h.get('pattern-record-placement');assert.equal(field.value,'selected');field.value='live';field.fire('change');assert.equal(h.state.recordPlacement,'live');assert.equal(saved.get('st.recordPlacement'),'live');assert.equal(h.document.activeElement,h.get('tracker'));
});
console.log(`Keyboard toggle: ${count} regression groups passed.`);
