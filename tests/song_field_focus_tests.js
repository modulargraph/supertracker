'use strict';
// Actual Song bindings with destructive render and explicitly modeled native Tab.
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const fixture=fs.readFileSync(path.join(__dirname,'song_sequence_tests.js'),'utf8');
const localRequire=name=>name==='node:fs'?{...fs,readFileSync(file,...args){return fs.readFileSync(process.env.SUPERTRACKER_SONG_SOURCE&&path.basename(String(file))==='song-sequence.js'?process.env.SUPERTRACKER_SONG_SOURCE:file,...args);}}:require(name);
const base=new Function('require','__dirname',fixture.slice(0,fixture.indexOf("test('arranger keys")).replace('return { song, state, key, labels, toasts, ST };','return { song, state, key, labels, toasts, ST, document, nodes, context };')+'\nreturn harness;')(localRequire,__dirname);
function harness(){
 const h=base([0,1,2]),timers=new Map(),events=new Map();let serial=0,renders=0;
 h.context.setTimeout=fn=>{timers.set(++serial,fn);return serial;};h.context.clearTimeout=id=>timers.delete(id);
 h.document.body={id:'body'};h.document.addEventListener=(type,fn)=>{const fns=events.get(type)||[];fns.push(fn);events.set(type,fns);};
 function render(){
  const focus=h.document.activeElement?.id;for(const node of h.nodes.values())node.isConnected=false;h.nodes.clear();h.document.activeElement=h.document.body;renders++;
  for(const id of ['sequence-list','song-block-pattern','song-block-repeats','song-insert'])h.nodes.set(id,{id,value:'2',dataset:{},isConnected:true,focus(){if(this.isConnected)h.document.activeElement=this;},setAttribute(){},classList:{toggle(){}},closest(){return null;}});
  h.song.bind();if(focus)h.nodes.get(focus)?.focus();
 }
 h.ST.refresh=render;h.ST.mutate=(label,fn,options={})=>{h.state.undo.push({label});fn(h.state.project);if(options.render!==false)render();};render();
 function depart(id,next,value){const field=h.nodes.get(id),target=h.nodes.get(next);field.focus();field.value=String(value);field.oninput?.();h.document.activeElement=h.document.body;field.onchange({target:field});field.onblur?.();target.focus();return target;}
 return {...h,depart,get renders(){return renders;},flush(){for(const [id,fn] of [...timers]){timers.delete(id);fn();}},pointer(type){for(const fn of events.get(type)||[])fn();}};
}
for(const [field,next,value] of [['song-block-repeats','song-insert',3],['song-block-pattern','song-block-repeats',2],['song-block-repeats','song-block-pattern',4]]){
 const h=harness(),target=h.depart(field,next,value);assert.equal(h.document.activeElement,target,'Tab destination must remain connected during commit');assert.equal(h.state.undo.length,1);h.flush();assert.equal(h.document.activeElement.id,next);assert.equal(h.state.undo.length,1);
 console.log('PASS Song field Tab: '+field+' → '+next);
}
{
 const h=harness();h.depart('song-block-pattern','song-block-repeats',2);const field=h.nodes.get('song-block-repeats');field.value='12';field.oninput();const before=h.renders;h.flush();assert.equal(h.renders,before);assert.equal(field.value,'12');assert.equal(h.document.activeElement,field);
 field.onchange({target:field});field.onblur();h.nodes.get('song-insert').focus();h.flush();assert.equal(h.document.activeElement.id,'song-insert');assert.equal(h.state.project.sequence.filter(id=>id===2).length,13);console.log('PASS pending Song repaint preserves a newer numeric draft');
}
{
 const h=harness();h.pointer('pointerdown');h.depart('song-block-repeats','song-insert',3);const target=h.document.activeElement,before=h.renders;h.flush();assert.equal(h.renders,before);assert(target.isConnected);h.pointer('pointerup');h.flush();assert.equal(h.document.activeElement.id,'song-insert');console.log('PASS pending Song repaint leaves the pointer click target connected');
}
{
 const h=harness();h.depart('song-block-repeats','song-insert',3);const before=h.renders;h.state.view='mixer';h.flush();assert.equal(h.renders,before);console.log('PASS late Song repaint cannot redraw a newer workspace');
}
{
 const h=harness();h.ST.mutate=()=>{};const field=h.nodes.get('song-block-repeats');field.focus();field.value='3';field.onchange({target:field});h.flush();assert.equal(h.document.activeElement,field);assert.equal(h.state.undo.length,0);console.log('PASS refused Song field edit leaves its control focused');
}
