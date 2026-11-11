'use strict';
// Actual app renderer, scalar handlers, history and native sync queue for the Song view and the
// transport's tempo/meter fields.
// Native replies are controlled fixtures, not audio tests. settings_lifecycle_tests.js reuses the harness.
const assert=require('node:assert/strict'),fs=require('node:fs'),path=require('node:path'),vm=require('node:vm');
const web = require('./helpers/web');
const fixture=fs.readFileSync(path.join(__dirname,'scalar_field_focus_tests.js'),'utf8');
const makeHarness=new Function('require','__dirname',fixture.slice(0,fixture.indexOf('const tests = [], test ='))+'\nreturn harness;')(require,__dirname);
const copy=value=>JSON.parse(JSON.stringify(value)),tick=async()=>{for(let n=0;n<80;n++)await Promise.resolve();};
const deferred=()=>{let resolve;const promise=new Promise(done=>resolve=done);return {promise,resolve};};
function harness(view='song'){
 const h=makeHarness(view),handlers=new Map();web.pendingEditsStub(h.window,'nksControls','STNKSControls',30);
 h.state.project.audioPlugins=[];h.state.project.bpm=120;h.state.project.timeSignature={numerator:4,denominator:4};
 h.window.STProjectTiming=require('../ui/project-timing.js');
 // Parse production modal markup, preserving detached-node behavior on repaint.
 const overlay=h.node('div','overlay');let serial=0;
 function makeContent(root){Object.defineProperty(root,'innerHTML',{configurable:true,get(){return this.html||'';},set(html){this.html=html;function detach(node){for(const child of node.children){child.isConnected=false;detach(child);}}detach(this);this.children=[];const stack=[this];for(const match of html.matchAll(/<(\/)?([a-z][\w-]*)\b((?:"[^"]*"|'[^']*'|[^'">])*)>/gi)){const tag=match[2].toLowerCase();if(match[1]){while(stack.length>1&&stack.pop().tagName!==tag.toUpperCase()){}continue;}const attrs={};for(const a of match[3].matchAll(/([\w:-]+)(?:="([^"]*)"|='([^']*)')?/g))attrs[a[1]]=a[2]??a[3]??'';const node=h.node(tag,attrs.id||'modal-node-'+(++serial),stack.at(-1),{attrs,className:attrs.class||'',type:attrs.type||'',disabled:Object.hasOwn(attrs,'disabled')});node.value=attrs.value||'';node.readOnly=Object.hasOwn(attrs,'readonly');node.hasAttribute=name=>Object.hasOwn(node.attrs,name);node.select=()=>{};node.style={};node.offsetParent=overlay;makeContent(node);if(!['input','br','hr','img','path'].includes(tag))stack.push(node);}for(const select of this.querySelectorAll('select'))select.value=(select.querySelectorAll('option').find(x=>x.hasAttribute('selected'))||select.querySelectorAll('option')[0])?.value||'';}});}
 makeContent(overlay);
 h.window.STHostTransport.post=message=>{h.calls.push(copy(message));Promise.resolve().then(()=>handlers.has(message.command)?handlers.get(message.command)(message.payload):({})).then(result=>h.window.STHostTransport.deliver({id:message.id,ok:true,result}),error=>h.window.STHostTransport.deliver({id:message.id,ok:false,error:error.message}));};
 handlers.set('engine.project',()=>({}));handlers.set('engine.snapshot',()=>copy(h.state.project));handlers.set('project.save',payload=>({saved:true,project:copy(payload.project)}));handlers.set('project.new',()=>({project:h.ST.demoProject()}));
 h.ST.refresh();h.calls.length=0;h.timers.clear();
 return {...h,handlers};
}
async function until(h,command){await tick();assert(h.calls.some(x=>x.command===command),'Expected '+command);}
const tests=[],test=(name,fn)=>tests.push({name,fn});
test('the tempo & meter editor is gone: no Song button, command, menu item or scripts',async()=>{
 const html=fs.readFileSync(path.join(__dirname,'../ui/index.html'),'utf8');assert(!html.includes('timing-editor'));assert(html.indexOf('src="project-timing.js"')<html.indexOf('src="app-util.js"'));
 assert(!fs.existsSync(path.join(__dirname,'../ui/timing-editor.js')));
 const h=harness();assert.throws(()=>h.get('song-timing'),/Missing mounted field/);assert(!h.ST.commands.some(command=>command[0]==='timing'));
 assert.equal(h.ST.perform('timing'),undefined);assert.equal(h.state.modal,null);assert.equal(h.window.STTimingEditor,undefined);assert.equal(h.window.STBusy.isBusy('timing'),false);
 assert(!fs.readFileSync(path.join(__dirname,'../crates/st-app/src/menus.rs'),'utf8').includes('"timing"'));
});
test('scalar tempo and meter edits are immediate and never invent a map',()=>{
 const h=harness('song');h.depart('tempo','metronome',137);assert.equal(h.state.project.bpm,137);assert.equal(h.state.undo.length,1);
 h.depart('signature-numerator','metronome',3);assert.equal(h.state.project.timeSignature.numerator,3);assert.equal(h.state.undo.length,2);
 assert(!h.calls.some(x=>x.command.startsWith('timing.')));
});
(async()=>{let passed=0;for(const {name,fn}of tests){let timer;try{await Promise.race([fn(),new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('Timed out: possible operation-gate deadlock')),4000);})]);passed++;console.log('PASS '+name);}catch(error){console.error('FAIL '+name+'\n'+error.stack);}finally{clearTimeout(timer);}}console.log(`Mounted Song view: ${passed}/${tests.length} groups passed.`);if(passed!==tests.length)process.exitCode=1;})().catch(error=>{console.error(error);process.exitCode=1;});
