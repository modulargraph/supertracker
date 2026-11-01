'use strict';
const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict'),path=require('node:path');
const original=fs.readFileSync(path.join(__dirname,'instrument_persistence_tests.js'),'utf8');
const {harness,tick}=new Function('require','__dirname',original.slice(0,original.indexOf('const tests=[]'))+'\nreturn {harness,tick};')(require,__dirname);
const copy=x=>JSON.parse(JSON.stringify(x));
(async()=>{
 const h=harness();h.ST.state.dirty=false;const originalProject=copy(h.ST.state.project);
 const template=copy(originalProject);template.name='Hardware template';template.tracks=template.tracks.slice(0,8);template.tracks.forEach((t,i)=>t.instrument=i<3?i:-1);template.instruments=template.instruments.slice(0,3);template.patterns.forEach(p=>{p.steps=template.tracks.map(()=>Array(p.length).fill(null));p.automation=[];});
 h.handlers.set('project.new',payload=>payload.blank?null:{project:copy(template)});
 assert.equal(await h.ST.projectAction('new'),true);assert.equal(h.ST.state.project.name,template.name);assert.equal(h.ST.state.project.tracks.length,8);assert.deepEqual(h.ST.state.project.tracks.slice(3).map(t=>t.instrument),[-1,-1,-1,-1,-1]);
 h.ST.state.dirty=false;assert.equal(await h.ST.projectAction('new','hardware'),true);assert.equal(h.calls.filter(x=>x.command==='project.new').at(-1).payload.templateID,'hardware');
 h.ST.state.dirty=false;await h.ST.projectAction('demo');assert.equal(h.calls.filter(x=>x.command==='project.new').at(-1).payload.blank,true);
 console.log('PASS New adopts default or explicit native template, keeps eight lanes/five unassigned, Demo bypasses default');
 h.load('project-templates.js');h.handlers.set('project.templates.list',()=>({templates:[{id:'hardware',name:'Hardware Synths'}],defaultID:'hardware'}));
 const before=copy(h.ST.state.project);await h.window.STProjectTemplates.show();const modal=h.element('#overlay').innerHTML;assert.match(modal,/Hardware Synths/);assert.match(modal,/selected/);
 const choice=h.element('template-default');choice.value='';await choice.onchange({target:choice});assert.equal(h.calls.filter(x=>x.command==='project.templates.setDefault').at(-1).payload.id,null);assert.deepEqual(copy(h.ST.state.project),before);
 h.element('template-name').value='My starting patch';await h.element('template-save').onclick();const save=h.calls.find(x=>x.command==='project.templates.save');assert.equal(save.payload.name,'My starting patch');assert.deepEqual(save.payload.project,before);assert.deepEqual(copy(h.ST.state.project),before);
 console.log('PASS template manager selects defaults and saves captured reusable project without replacing current document');

 // A late list response must not replace a newer dialog or a newly selected project.
 {
 const r=harness();r.load('project-templates.js');let resolve;
 r.handlers.set('project.templates.list',()=>new Promise(done=>resolve=done));
 const opening=r.window.STProjectTemplates.show();await tick();r.ST.openModal('Other dialog','Keep this',{},{kind:'other'});resolve({templates:[]});assert.equal(await opening,false);assert.equal(r.ST.state.modal,'other');
 const next=r.window.STProjectTemplates.show();await tick();r.ST.state.project=copy(r.ST.state.project);resolve({templates:[]});assert.equal(await next,false);
 console.log('PASS delayed template listing preserves newer dialogs and project changes');
 }
 {
 const r=harness();r.load('project-templates.js');r.handlers.set('project.templates.list',()=>({templates:[],defaultID:'original'}));await r.window.STProjectTemplates.show();
 const field=r.element('template-default');field.value='latest';await field.onchange({target:field});r.handlers.set('project.templates.setDefault',()=>{throw Error('Write failed');});field.value='failed';await field.onchange({target:field});assert.equal(field.value,'latest');
 const oldSave=r.element('template-save').onclick;r.element('template-name').value='Snapshot';r.ST.closeModal();await oldSave();assert(!r.calls.some(c=>c.command==='project.templates.save'));
 console.log('PASS failed default update restores latest acknowledged choice and detached Save is inert');
 }
 {
 const r=harness();r.load('project-templates.js');r.handlers.set('project.templates.list',()=>({templates:[]}));await r.window.STProjectTemplates.show();let finish;
 r.handlers.set('project.templates.save',()=>new Promise(done=>finish=done));r.element('template-name').value='Snapshot';const saving=r.element('template-save').onclick();for(let i=0;i<30&&!finish;i++)await tick();assert(finish);r.ST.closeModal();r.ST.openModal('Other dialog','Keep this','',{kind:'other'});finish({});await saving;assert.equal(r.ST.state.modal,'other');assert.equal(r.calls.filter(c=>c.command==='project.templates.list').length,1);assert.equal(r.window.STProjectTemplates.isBusy,false);
 console.log('PASS completed template save never reopens a dismissed manager');
 }
 h.window.STBasePatch={isBusy:true};const old=h.ST.state.project;assert.equal(await h.ST.projectAction('new'),false);assert.equal(await h.ST.projectAction('save'),false);assert.equal(h.ST.state.project,old);
 console.log('PASS base-patch transaction blocks competing session replacement and save');
})().catch(error=>{console.error(error);process.exitCode=1;});
