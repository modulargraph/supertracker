'use strict';
const assert = require('node:assert/strict'), fs = require('node:fs'), vm = require('node:vm');
const web = require('./helpers/web');
const context = vm.createContext({window:{}});
web.load(context, 'clip-takes-project.js');
const M = context.window.STClipTakesProject, clone = value => JSON.parse(JSON.stringify(value));
let counter = 0; const uid = () => `take-${++counter}`;
const base = () => ({id:'clip-1',track:0,name:'Phrase',path:'/old.wav',samplePath:'/old.wav',startBeat:4,durationBeats:8,sourceOffset:1,sourceDuration:12,gain:.6,fadeIn:.08,fadeOut:.1,muted:false});
const add = clip => M.add(clip,{path:'/new.wav',name:'New take',sourceDuration:10},.5,uid);
const tests = [], test = (name,fn) => tests.push([name,fn]);
test('adding an alternative is immutable, preserves original, and exposes only its canonical audio path',()=>{
 const original=base(), before=clone(original), result=add(original);assert.deepEqual(original,before);
 assert.equal(result.takeSet.takes.length,2);assert.equal(result.path,'/new.wav');assert.equal(result.samplePath,'/new.wav');assert.equal(result.sourceOffset,0);assert.equal(result.startBeat,4);assert.equal(result.durationBeats,8);assert.equal(result.gain,.6);
 assert.equal(result.takeSet.takes[0].path,'/old.wav');assert.equal(result.takeSet.takes[0].sourceOffset,1);assert.equal(result.takeSet.takes[0].sourceDuration,12);
 const reverted=M.activate(result,result.takeSet.takes[0].id,.5);assert.equal(reverted.path,'/old.wav');assert.equal(reverted.sourceOffset,1);
});
test('split regions retain independent alternatives with timeline-correct source offsets',()=>{
 const original=add(base()), before=clone(original), [left,right]=M.split(original,3,.5,uid);
 assert.deepEqual(clone(original),before);assert.equal(left.durationBeats,3);assert.equal(right.durationBeats,5);assert.equal(right.startBeat,7);assert.notEqual(right.id,left.id);assert.equal(left.fadeOut,0);assert.equal(right.fadeIn,0);
 assert.equal(right.takeSet.takes[0].sourceOffset,2.5);assert.equal(right.takeSet.takes[1].sourceOffset,1.5);assert.equal(right.sourceOffset,1.5);
 const comp=M.activate(right,right.takeSet.takes[0].id,.5);assert.equal(comp.path,'/old.wav');assert.equal(comp.sourceOffset,2.5);assert.equal(left.path,'/new.wav');
 comp.takeSet.takes[0].name='Independent';assert.notEqual(left.takeSet.takes[0].name,'Independent');
});
test('slipping one take survives cycling while alternatives retain independent alignment',()=>{
 let clip=add(base());clip.sourceOffset=2;clip=M.retainEdits(clip);const newID=clip.takeSet.activeID,oldID=clip.takeSet.takes[0].id;
 clip=M.activate(clip,oldID,.5);assert.equal(clip.sourceOffset,1);clip=M.activate(clip,newID,.5);assert.equal(clip.sourceOffset,2);
});
test('source coverage rejects short imports and unusable alternatives after length extension without shortening regions',()=>{
 const clip=base(),before=clone(clip);assert.throws(()=>M.add(clip,{path:'/short.wav',sourceDuration:3.99},.5,uid),/at least 4.00/);assert.deepEqual(clip,before);
 const result=M.add(clip,{path:'/exact.wav',sourceDuration:4},.5,uid);result.durationBeats=12;
 const old=M.activate(result,result.takeSet.takes[0].id,.5);assert.equal(old.durationBeats,12);assert.throws(()=>M.activate(old,result.takeSet.activeID,.5),/too short/);
});
test('a duration function governs coverage and split offsets',()=>{
 const secondsAt = beat => beat<=6?beat*.5:3+(beat-6);
 const duration = (start,length) => secondsAt(start+length)-secondsAt(start);
 const clip=base();assert.throws(()=>M.add(clip,{path:'/short.wav',sourceDuration:6.99},duration,uid),/7.00/);
 const result=M.add(clip,{path:'/map.wav',sourceDuration:8},duration,uid),[left,right]=M.split(result,3,duration,uid);
 assert.equal(right.sourceOffset,2);assert.equal(right.takeSet.takes[0].sourceOffset,3);assert.equal(left.durationBeats+right.durationBeats,8);
});
test('rename and removal never delete the active take or source data',()=>{
 const clip=add(base()),originalID=clip.takeSet.takes[0].id,newID=clip.takeSet.activeID;
 assert.throws(()=>M.remove(clip,newID),/Choose another/);assert.throws(()=>M.rename(clip,originalID,' '),/name/);
 const renamed=M.rename(clip,originalID,' Alternate ');assert.equal(renamed.takeSet.takes[0].name,'Alternate');assert.equal(clip.takeSet.takes[0].name,'Phrase');
 const removed=M.remove(renamed,originalID);assert.equal(removed.takeSet.takes.length,1);assert.equal(removed.path,'/new.wav');assert.equal(clip.takeSet.takes.length,2);
});
test('malformed schema, offsets, duplicate IDs and full take capacity fail before changes',()=>{
 for (const set of [{takes:[]},{activeID:'x',takes:[]},{activeID:'x',takes:[{id:'x',path:'/x.wav',sourceOffset:-1}]},{activeID:'missing',takes:[{id:'x',path:'/x.wav',sourceOffset:0}]},{activeID:'x',takes:[{id:'x',path:'/x.wav',sourceOffset:0},{id:'x',path:'/x.wav',sourceOffset:0}]}]) assert.throws(()=>M.read({...base(),takeSet:set}),/invalid|missing|unsupported/);
 let clip=base();for(let i=1;i<32;i++)clip=add(clip);const before=clone(clip);assert.throws(()=>add(clip),/32/);assert.deepEqual(clone(clip),before);
 assert.throws(()=>M.add(base(),{path:'/x',sourceDuration:Infinity},.5,uid),/readable/);assert.throws(()=>M.split(base(),0,.5,uid),/inside/);
});
test('JSON save/open preserves active choice, all sources and independent comp regions',()=>{
 const original=add(base()), [left,right]=M.split(original,4,.5,uid),comp=M.activate(right,right.takeSet.takes[0].id,.5);
 const reopened=clone({audioClips:[left,comp]});assert.equal(M.read(reopened.audioClips[0]).activeID,left.takeSet.activeID);assert.equal(M.read(reopened.audioClips[1]).activeID,comp.takeSet.activeID);
 assert.deepEqual(reopened.audioClips.map(c=>c.path),['/new.wav','/old.wav']);assert.equal(reopened.audioClips[1].sourceOffset,3);assert.equal(reopened.audioClips[1].takeSet.takes.length,2);
});
for(const [name,fn] of tests){fn();console.log('PASS '+name);}console.log(`Clip take project: ${tests.length} regression groups passed.`);
