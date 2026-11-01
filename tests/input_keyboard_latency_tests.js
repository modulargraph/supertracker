'use strict';
// Production keyboard/capture handlers with controllable native replies. No native audio runs.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path');
const fixture = fs.readFileSync(path.join(__dirname, 'input_ack_ui_tests.js'), 'utf8');
const harness = new Function('require', '__dirname', fixture.slice(0, fixture.indexOf('\nconst tests = [], test =')) + '\nreturn harness;')(require, __dirname);
const tick = async () => { for (let i = 0; i < 100; i++) await Promise.resolve(); };
const stopped = { playing: false, recordingClock: undefined };
const tests = [], test = (name, run) => tests.push({ name, run });
async function setup() { const h = await harness(); h.state.playing = false; h.state.status.playing = false; return h; }

test('rapid Z followed by Up and Option-Up selects and transposes the recorded note', async () => {
  const h = await setup(); h.down('z', 'KeyZ'); h.down('ArrowUp', 'ArrowUp'); h.down('ArrowUp', 'ArrowUp', { altKey: true });
  h.reply(h.captures()[0], stopped); await tick();
  assert.equal(h.state.row, 0, 'Up must apply after the note advances its edit row');
  assert.equal(h.step(0)?.note, 61, 'Transpose must apply to the newly recorded step');
  assert.equal(h.state.undo.length, 2); assert.equal(h.audible(true).length, 1);
});

test('rapid Z followed by Down retains the note and moves past its advanced cursor', async () => {
  const h = await setup(); h.down('z', 'KeyZ'); h.down('ArrowDown', 'ArrowDown'); h.reply(h.captures()[0], stopped); await tick();
  assert.equal(h.step(0)?.note, 60, 'Navigation must not invalidate the acknowledged note');
  assert.equal(h.state.row, 2); assert.equal(h.state.undo.length, 1);
});

test('rapid Z and X reserve consecutive rows and navigation follows both acknowledgements', async () => {
  const h = await setup(); h.down('z', 'KeyZ'); h.down('x', 'KeyX'); h.down('ArrowUp', 'ArrowUp'); h.down('ArrowUp', 'ArrowUp', { altKey: true });
  h.reply(h.captures()[1], stopped); await tick(); assert.equal(h.state.undo.length, 0);
  h.reply(h.captures()[0], stopped); await tick();
  assert.equal(h.step(0)?.note, 60); assert.equal(h.step(1)?.note, 63); assert.equal(h.state.row, 1); assert.equal(h.state.undo.length, 3);
});

test('notes after queued navigation record at its resulting row while keyups remain immediate', async () => {
  for(const key of ['ArrowUp','ArrowDown']){
    const h=await setup();h.down('z','KeyZ');h.up('z','KeyZ');h.down(key,key);h.down('x','KeyX');h.up('x','KeyX');
    assert.equal(h.audible(true).length,2);assert.equal(h.audible(false).length,2,'Pending grid work must never postpone physical note-off');
    for(const message of [...h.captures(),...h.captures(false)].reverse())h.reply(message,stopped);
    await tick();assert.equal(h.step(key==='ArrowUp'?0:2)?.note,62,key);assert.equal(h.state.row,key==='ArrowUp'?1:3,key);assert.equal(h.state.undo.length,2);
    assert(!h.window.STInputCapture.hasPendingEdits);assert.equal(h.audible(true).length,2,'Draining the command stream must never replay audition');
  }
});

test('horizontal track navigation retains the accepted original note without moving the new selection',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('Tab','Tab');assert.equal(h.state.track,1);h.down('x','KeyX');
  assert.deepEqual(h.captures().map(message=>message.payload.track),[0,1],'Immediate audition follows the selected track');
  h.reply(h.captures()[0],stopped);await tick();assert.equal(h.step(0,7,0)?.note,60);assert.equal(h.state.track,1);assert.equal(h.state.row,0,'The old track must not advance the new cursor');
  h.reply(h.captures()[1],stopped);await tick();assert.equal(h.step(0,7,1)?.note,62);assert.equal(h.state.track,1);assert.equal(h.state.row,1);assert.equal(h.state.undo.length,2);
});

test('a rejected onset never creates a stopped step or an audition replay', async () => {
  const h = await setup(); h.down('z', 'KeyZ'); h.down('ArrowUp', 'ArrowUp'); h.down('ArrowUp', 'ArrowUp', { altKey: true });
  h.reply(h.captures()[0], { ...stopped, status: 'unavailable', applied: false }); await tick();
  assert.equal(h.step(0), null); assert.equal(h.state.undo.length, 0); assert.equal(h.audible(true).length, 1);
});

test('a replacement session cannot receive delayed navigation, transpose, or note data', async () => {
  const h = await setup(); h.down('z', 'KeyZ'); h.down('ArrowDown', 'ArrowDown'); h.down('ArrowUp', 'ArrowUp', { altKey: true });
  h.state.project = JSON.parse(JSON.stringify(h.project)); h.state.row = 4;
  const before = JSON.stringify(h.state.project); h.reply(h.captures()[0], stopped); await tick();
  assert.equal(JSON.stringify(h.state.project), before); assert.equal(h.state.row, 4); assert.equal(h.state.undo.length, 0); assert.equal(h.audible(true).length, 1);
});

test('missing replies bound queued commands and never create delayed notes after timeout',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');
  assert(h.window.STInputCapture.hasPendingEdits);h.fireTimers(1500);await h.window.STInputCapture.flushEdits();
  assert.equal(h.state.undo.length,0);assert.equal(h.step(0),null);assert.equal(h.state.row,0);assert(!h.window.STInputCapture.hasPendingEdits);
  for(const message of h.captures())h.reply(message,stopped);await tick();assert.equal(h.state.undo.length,0);assert.equal(h.audible(true).length,2);
});

test('Save waits for queued navigation and transpose and serializes the accepted chord',async()=>{
  const h=await setup();h.state.chord='major';h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('ArrowUp','ArrowUp',{altKey:true});
  const saving=h.ST.perform('save');await tick();assert(!h.calls.some(message=>message.command==='project.save'));assert(h.window.STInputCapture.hasPendingEdits);
  h.reply(h.captures()[0],stopped);await saving;await tick();
  const saved=h.calls.find(message=>message.command==='project.save')?.payload.project.patterns[0].steps[0][0];assert.equal(saved.note,61);assert.deepEqual(saved.chord,[0,4,7]);assert(!h.window.STInputCapture.hasPendingEdits);assert.equal(h.state.row,0);
});

test('Undo waits for the command stream then undoes transpose without discarding the accepted note',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('ArrowUp','ArrowUp',{altKey:true});
  h.down('z','KeyZ',{metaKey:true});await tick();h.reply(h.captures()[0],stopped);await tick();
  assert.equal(h.step(0)?.note,60);assert.equal(h.state.undo.length,1);assert.equal(h.state.redo.length,1);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('freeze refusal cannot insert data or run queued transpose after rollback',async()=>{
  const h=await setup();h.window.STFreeze={checkMutation(){return 'frozen';},explain(){}};h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('ArrowUp','ArrowUp',{altKey:true});
  h.reply(h.captures()[0],stopped);await tick();assert.equal(h.step(0),null);assert.equal(h.state.undo.length,0);assert.equal(h.state.row,0);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('an intervening manual edit cancels queued mutations without discarding an otherwise accepted note',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');
  h.ST.mutate('manual note',()=>h.project.patterns[0].steps[0][3]={note:77,instrument:0,velocity:90,gate:.8},{render:false});
  h.reply(h.captures()[0],stopped);h.reply(h.captures()[1],stopped);await tick();assert.equal(h.step(0)?.note,60);assert.equal(h.step(3)?.note,77);assert.equal(h.state.undo.length,2);assert.equal(h.state.row,1);
});

test('native playing acknowledgement keeps the exact live row and cancels stopped-grid intentions',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowDown','ArrowDown');
  h.reply(h.captures()[0],{playing:true,row:5});await tick();assert.equal(h.step(5)?.note,60);assert.equal(h.step(0),null);assert.equal(h.state.row,0);assert.equal(h.state.undo.length,1);
});

test('already-issued old-track vertical commands and notes drain after Tab without moving the new selection',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');h.down('Tab','Tab');
  h.reply(h.captures()[1],stopped);h.reply(h.captures()[0],stopped);await tick();
  assert.equal(h.step(0,7,0)?.note,62,'X must finish at the old track row selected by queued Up');assert.equal(h.state.track,1);assert.equal(h.state.row,0);assert.equal(h.state.undo.length,2);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('view navigation preserves an accepted pending note and releases its audition immediately',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('4','Digit4',{metaKey:true});
  assert.equal(h.state.view,'mixer');assert.equal(h.audible(false).length,1,'Leaving the view releases held audition immediately');
  h.reply(h.captures()[0],stopped);h.reply(h.captures(false)[0],stopped);await tick();
  assert.equal(h.step(0)?.note,60);assert.equal(h.state.row,0);assert.equal(h.state.view,'mixer');assert.equal(h.state.undo.length,1);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('detached transpose edits its original track and preserves visible mixer focus',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('ArrowUp','ArrowUp',{altKey:true});h.down('4','Digit4',{metaKey:true});
  const focused=h.context.document.activeElement;h.reply(h.captures()[0],stopped);h.reply(h.captures(false)[0],stopped);await tick();
  assert.equal(h.step(0)?.note,61);assert.equal(h.state.view,'mixer');assert.equal(h.state.row,0);assert.equal(h.context.document.activeElement,focused);assert.equal(h.state.undo.length,2);
});

test('three detached track streams retain their notes and immediate Save waits for all of them',async()=>{
  const h=await setup();
  h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');h.down('Tab','Tab');
  h.down('c','KeyC');h.down('ArrowDown','ArrowDown');h.down('v','KeyV');h.down('Tab','Tab');
  h.down('b','KeyB');h.down('ArrowUp','ArrowUp');h.down('n','KeyN');h.down('4','Digit4',{metaKey:true});
  assert.deepEqual(h.captures().map(message=>message.payload.track),[0,0,1,1,2,2]);assert.equal(h.audible(false).length,6);
  const saving=h.ST.perform('save');await tick();assert(!h.calls.some(message=>message.command==='project.save'));
  for(const message of [...h.captures(),...h.captures(false)].reverse())h.reply(message,stopped);
  await saving;await tick();const saved=h.calls.find(message=>message.command==='project.save')?.payload.project.patterns[0].steps;
  assert.equal(saved[0][0]?.note,62);assert.equal(saved[1][0]?.note,64);assert.equal(saved[1][2]?.note,65);assert.equal(saved[2][0]?.note,69);
  assert.equal(h.state.track,2);assert.equal(h.state.row,0);assert.equal(h.state.view,'mixer');assert.equal(h.state.undo.length,6);assert(!h.window.STInputCapture.hasPendingEdits);assert.equal(h.audible(true).length,6);
});

test('rejecting a detached note never inserts it or cancels an independent accepted track',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');h.down('Tab','Tab');h.down('c','KeyC');h.down('ArrowDown','ArrowDown');h.down('v','KeyV');
  const messages=h.captures();h.reply(messages[0],stopped);h.reply(messages[1],{...stopped,status:'unavailable',applied:false});h.reply(messages[2],stopped);h.reply(messages[3],stopped);await tick();
  assert.equal(h.step(0,7,0)?.note,60);assert.equal(h.step(0,7,1)?.note,64);assert.equal(h.step(2,7,1)?.note,65);assert.equal(h.state.undo.length,3);assert.equal(h.state.track,1);assert.equal(h.state.row,3);
});

test('detached Shift navigation transposes its captured selection without changing or auditioning the new track',async()=>{
  const h=await setup();h.project.patterns[0].steps[0][1]={note:64,instrument:0};h.project.patterns[0].steps[0][2]={note:67,instrument:0};
  h.down('z','KeyZ');h.down('ArrowDown','ArrowDown',{shiftKey:true});h.down('ArrowUp','ArrowUp',{altKey:true});h.down('Tab','Tab');
  h.reply(h.captures()[0],stopped);await tick();
  assert.equal(h.step(0)?.note,60);assert.equal(h.step(1)?.note,65);assert.equal(h.step(2)?.note,68);assert.equal(h.state.track,1);assert.equal(h.state.selection,null);assert.equal(h.state.row,0);assert.equal(h.state.undo.length,2);
  assert.equal(h.calls.filter(message=>message.command==='step.preview').length,0,'Detached edits must not begin late previews after leaving their target');
});

test('replacing the captured pattern object invalidates detached work even when its timing and ID match',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');h.down('Tab','Tab');
  h.project.patterns[0]=JSON.parse(JSON.stringify(h.project.patterns[0]));
  for(const message of h.captures())h.reply(message,stopped);await tick();assert.equal(h.step(0),null);assert.equal(h.state.undo.length,0);assert.equal(h.state.track,1);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('manual edits invalidate detached intentions while accepted independent cell input remains owned',async()=>{
  const h=await setup();h.down('z','KeyZ');h.down('ArrowUp','ArrowUp');h.down('x','KeyX');h.down('Tab','Tab');h.down('c','KeyC');h.down('ArrowDown','ArrowDown');h.down('v','KeyV');
  h.ST.mutate('manual edit',()=>h.project.patterns[0].steps[0][0]={note:77,instrument:0},{render:false});
  for(const message of h.captures())h.reply(message,stopped);await tick();assert.equal(h.step(0)?.note,77);assert.equal(h.step(0,7,1)?.note,64);assert.equal(h.step(2,7,1),null);assert.equal(h.state.undo.length,2);assert(!h.window.STInputCapture.hasPendingEdits);
});

test('new-track row navigation before its first note never adopts the detached old-track cursor',async()=>{
  const h=await setup();h.state.row=2;h.down('z','KeyZ');h.down('Tab','Tab');h.down('ArrowDown','ArrowDown');h.down('x','KeyX');
  for(const message of h.captures())h.reply(message,stopped);await tick();assert.equal(h.step(2,7,0)?.note,60);assert.equal(h.step(3,7,1)?.note,62);assert.equal(h.state.track,1);assert.equal(h.state.row,4);assert.equal(h.state.undo.length,2);
});

test('returning from another view starts fresh navigation while the accepted old cell drains',async()=>{
  const h=await setup();h.state.row=2;h.down('z','KeyZ');h.down('4','Digit4',{metaKey:true});h.down('1','Digit1',{metaKey:true});h.down('ArrowDown','ArrowDown');h.down('x','KeyX');
  for(const message of [...h.captures(),...h.captures(false)])h.reply(message,stopped);await tick();assert.equal(h.step(2)?.note,60);assert.equal(h.step(3)?.note,62);assert.equal(h.state.view,'pattern');assert.equal(h.state.row,4);assert.equal(h.state.undo.length,2);
});

(async () => { let failed = 0; for (const {name, run} of tests) { try { await run(); console.log('PASS ' + name); } catch (error) { failed++; console.error('FAIL ' + name + '\n' + error.stack); } } console.log(`${tests.length-failed}/${tests.length} input keyboard latency groups passed`); if (failed) process.exitCode = 1; })();
