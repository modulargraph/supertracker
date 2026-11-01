'use strict';
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web = require('./helpers/web');
const fixture=fs.readFileSync(path.join(__dirname,'scalar_field_focus_tests.js'),'utf8');
const harness=new Function('require','__dirname',fixture.slice(0,fixture.indexOf('const tests = [], test ='))+'\nreturn harness;')(require,__dirname);
const h=harness();let mounted=0,left=0,remembered=0,busy=false;
h.window.STRecording={mountInline(){mounted++;},leaveInline(){left++;return !busy;},rememberInline(){remembered++;},get inlineActive(){return h.state.view==='recording';},get isRecording(){return busy;},get isBusy(){return false;}};
web.load(h.context, 'line-inputs.js');
h.key(h.get('tracker'),'7',{code:'Digit7',metaKey:true});
assert.equal(h.state.view,'recording');assert.equal(mounted,1);assert(h.get('view-recording'));assert(h.get('recording-inline-host'));assert(h.get('recording-routing-configure'));assert(h.app.innerHTML.includes('recording-capture-panel'));
assert.equal(h.state.modal,null);
console.log('PASS Recording tab mounts the inline capture workspace with routing panels and no popup');
h.ST.refresh();assert.equal(mounted,2);assert(remembered>=2);
busy=true;const projectBefore=JSON.stringify(h.state.project);h.ST.setView('pattern');assert.equal(h.state.view,'recording');assert.equal(left,1);h.ST.openModal('Settings','test','',{kind:'settings'});assert.equal(h.state.modal,null);assert.equal(h.ST.mutate('bad edit',p=>p.name='Changed'),false);assert.equal(JSON.stringify(h.state.project),projectBefore);
console.log('PASS actual app view, modal and mutation guards protect an active inline capture');
busy=false;h.ST.setView('pattern');assert.equal(h.state.view,'pattern');assert.equal(h.document.activeElement,h.get('tracker'));h.ST.perform('view:recording');assert.equal(h.state.view,'recording');
console.log('PASS idle Recording workspace remains keyboard-navigable via Cmd7 and command palette');
// Opening Recording with Keys on must not route computer-keyboard notes into the name field.
h.window.STRecording.mountInline=()=>{mounted++;h.node('input','record-name',h.get('recording-inline-host'),{type:'text'});h.node('input','record-keyboard',h.get('recording-inline-host'),{type:'checkbox'});};
for(const keys of [true,false]){h.ST.setView('pattern');h.state.keyboardPlay=keys;h.ST.setView('recording');assert.equal(h.document.activeElement?.id,keys?'keyboard-play':'record-name');}
// The inline checkbox mirrors the toolbar toggle (⌘P) while Recording is open.
h.get('record-keyboard').checked=false;h.ST.perform('keyboard');assert.equal(h.state.keyboardPlay,true);assert.equal(h.get('record-keyboard').checked,true);
h.ST.perform('keyboard');assert.equal(h.state.keyboardPlay,false);assert.equal(h.get('record-keyboard').checked,false);
console.log('PASS Recording focus respects Keys on and the inline keys checkbox follows the toolbar');
