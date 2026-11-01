'use strict';
const assert=require('node:assert/strict'),timing=require('../Resources/Web/project-timing.js');
const copy=value=>JSON.parse(JSON.stringify(value));
const mapped={bpm:99,timeSignature:{numerator:4,denominator:4},timing:{version:1,tempos:[{id:'a',beat:0,bpm:120},{id:'b',beat:4,bpm:60},{id:'c',beat:7.5,bpm:180}],meters:[{id:'a',beat:0,numerator:4,denominator:4},{id:'b',beat:5.5,numerator:7,denominator:8},{id:'c',beat:12.5,numerator:3,denominator:4}]}};
const near=(actual,expected,epsilon=1e-9)=>assert(Math.abs(actual-expected)<=epsilon,`${actual} != ${expected}`);
{
 const map=timing.create(mapped);assert.equal(map.mapped,true);assert.equal(map.secondsAtBeat(0),0);assert.equal(map.secondsAtBeat(4),2);assert.equal(map.secondsAtBeat(7.5),5.5);near(map.secondsAtBeat(10.5),6.5);assert.equal(map.beatAtSeconds(5.5),7.5);assert.equal(map.tempoAtBeat(3.999),120);assert.equal(map.tempoAtBeat(4),60);assert.equal(map.tempoAtBeat(7.5),180);near(map.duration(3,8.5),13/3);near(map.endingBeat(3,13/3),8.5);near(timing.durationFromBeats(mapped,3,5.5),13/3);near(timing.endingBeat(mapped,3,13/3),8.5);
 for(let i=0;i<=2000;i++){const beat=i*500;near(map.beatAtSeconds(map.secondsAtBeat(beat)),beat,2e-9);}assert.equal(map.beatAtSeconds(map.maxSeconds),1000000);
 console.log('PASS piecewise tempo integration, inverse boundaries, clip duration, and endpoint round trips');
}
{
 const map=timing.create(mapped);assert.deepEqual(map.barAtBeat(5),{bar:2,notatedBeat:2,numerator:4,denominator:4,quarterOffset:1,barStart:4,barEnd:5.5,shortBar:true});assert.deepEqual(map.barAtBeat(5.5),{bar:3,notatedBeat:1,numerator:7,denominator:8,quarterOffset:0,barStart:5.5,barEnd:9,shortBar:false});assert.equal(map.barAtBeat(9).bar,4);assert.equal(map.barAtBeat(12.5).bar,5);assert.deepEqual(map.meterAtBeat(12.5),{numerator:3,denominator:4});assert.equal(map.labelAtBeat(6),'Bar 3 · beat 2 · 7/8');
 const nearBoundary=map.barAtBeat(5.5-Number.EPSILON*4);assert.equal(nearBoundary.bar,2);assert(nearBoundary.notatedBeat<2.5);
 console.log('PASS meter events start new bars, including exact mid-bar truncation and denominator-unit labels');
}
{
 for(const bpm of [20,120,333.333,400,999]){const project={bpm},map=timing.create(project);assert.equal(map.mapped,false);assert.equal(map.secondsAtBeat(7.25),7.25*60/bpm);assert.equal(map.tempoAtBeat(100),bpm);near(map.beatAtSeconds(map.secondsAtBeat(7.25)),7.25);assert.equal(Object.hasOwn(project,'timing'),false);}
 assert.equal(timing.create({bpm:'80'}).tempoAtBeat(0),120);assert.equal(timing.create({bpm:Infinity}).tempoAtBeat(0),120);assert.equal(timing.create({bpm:1000}).tempoAtBeat(0),999);assert.equal(timing.create({bpm:0}).tempoAtBeat(0),20);assert.deepEqual(timing.create({timeSignature:{numerator:3.9,denominator:8.9}}).meterAtBeat(0),{numerator:3,denominator:8});let id=0;const seeded=timing.seed({bpm:155,timeSignature:{numerator:5,denominator:8}},()=>String(++id));assert.deepEqual(seeded,{version:1,tempos:[{id:'1',beat:0,bpm:155}],meters:[{id:'2',beat:0,numerator:5,denominator:8}]});assert.equal(timing.validate({bpm:120}),null);
 console.log('PASS absent maps retain native legacy defaults/clamps and do not mutate or create project timing');
}
{
 const bad=[p=>p.timing=null,p=>p.timing=[],p=>p.timing.version=true,p=>p.timing.version='1',p=>p.timing.version=2,p=>p.timing.tempos=[],p=>p.timing.meters=[],p=>p.timing.tempos[0].beat=.1,p=>p.timing.meters[0].beat=.1,p=>p.timing.tempos[1].beat=0,p=>p.timing.tempos.reverse(),p=>p.timing.tempos[1].beat=1000001,p=>p.timing.tempos[0].bpm=19,p=>p.timing.tempos[0].bpm=401,p=>p.timing.tempos[0].bpm=NaN,p=>p.timing.tempos[0].bpm='120',p=>p.timing.tempos[0].bpm=true,p=>p.timing.meters[0].numerator=3.5,p=>p.timing.meters[0].numerator=17,p=>p.timing.meters[0].denominator=3,p=>p.timing.meters[0].denominator='4',p=>p.timing.meters[0].beat=Infinity,p=>p.timing.tempos[0].id=' ',p=>p.timing.tempos[0].id=1,p=>p.timing.tempos[1].id='a',p=>p.timing.meters[1].id='a',p=>p.timing.tempos[0].id='é'.repeat(129),p=>p.timing.tempos=Array.from({length:1025},(_,i)=>({id:String(i),beat:i,bpm:120}))];
 for(const mutate of bad){const project=copy(mapped);mutate(project);assert.throws(()=>timing.create(project),/Tempo map:/);}
 const boundary=copy(mapped);boundary.timing.tempos[0].id='é'.repeat(128);assert.doesNotThrow(()=>timing.create(boundary));
 const map=timing.create(mapped);for(const value of [-1,1000001,Infinity,NaN,'1',true])assert.throws(()=>map.secondsAtBeat(value));for(const value of [-1,map.maxSeconds+1,Infinity,'1'])assert.throws(()=>map.beatAtSeconds(value));assert.throws(()=>map.duration(4,3));assert.throws(()=>map.endingBeat(0,-1));assert.throws(()=>timing.durationFromBeats(mapped,0,-1));
 console.log('PASS strict present-map schema, UTF-8 ID boundaries, list limits, and query-domain validation');
}
{
 const project=copy(mapped),map=timing.create(project);project.timing.tempos[0].bpm=30;project.timing.meters[0].numerator=2;assert.equal(map.secondsAtBeat(4),2);assert.equal(map.barAtBeat(3).numerator,4);assert.throws(()=>map.tempos[0].bpm=99);assert.throws(()=>map.meters.push({}));const validated=timing.validate(mapped);validated.tempos[0].bpm=30;assert.equal(mapped.timing.tempos[0].bpm,120);
 const maximum={timing:{version:1,tempos:Array.from({length:1024},(_,i)=>({id:'t'+i,beat:i*.5,bpm:20+i%381})),meters:[{id:'m',beat:0,numerator:16,denominator:16}]}};assert.equal(timing.create(maximum).tempos.length,1024);
 console.log('PASS immutable compiled snapshots and valid maximum event counts');
}
