'use strict';
// The project's tempo and meter (ui/project-timing.js): beats, bars and seconds.
const assert=require('node:assert/strict'),timing=require('../ui/project-timing.js');
const near=(actual,expected,epsilon=1e-9)=>assert(Math.abs(actual-expected)<=epsilon,`${actual} != ${expected}`);
{
 for(const bpm of [20,120,333.333,400]){const project={bpm},map=timing.create(project);assert.equal(map.bpm,bpm);assert.equal(timing.tempo(project),bpm);assert.equal(map.secondsAtBeat(7.25),7.25*60/bpm);near(map.beatAtSeconds(map.secondsAtBeat(7.25)),7.25);assert.deepEqual(Object.keys(project),['bpm']);}
 const map=timing.create({bpm:90});assert.equal(map.secondsAtBeat(0),0);assert.equal(map.secondsAtBeat(3),2);assert.equal(map.beatAtSeconds(2),3);near(map.duration(3,8.5),5.5*60/90);near(map.endingBeat(3,2),6);
 near(timing.durationFromBeats({bpm:90},3,5.5),5.5*60/90);near(timing.endingBeat({bpm:90},3,2),6);
 for(let i=0;i<=2000;i++){const beat=i*500;near(map.beatAtSeconds(map.secondsAtBeat(beat)),beat,2e-9);}assert.equal(map.beatAtSeconds(map.maxSeconds),1000000);assert.equal(map.secondsAtBeat(1000000),map.maxSeconds);
 console.log('PASS seconds follow the project tempo, the inverse returns the beat, and clip durations and ends agree');
}
{
 const map=timing.create({bpm:120,timeSignature:{numerator:7,denominator:8}});assert.equal(map.quartersPerBar,3.5);
 assert.deepEqual(map.barAtBeat(0),{bar:1,notatedBeat:1,numerator:7,denominator:8,quarterOffset:0,barStart:0,barEnd:3.5});
 assert.deepEqual(map.barAtBeat(4.5),{bar:2,notatedBeat:3,numerator:7,denominator:8,quarterOffset:1,barStart:3.5,barEnd:7});
 assert.equal(map.barAtBeat(7).bar,3);assert.equal(map.barAtBeat(1000000).bar,285715);
 const before=map.barAtBeat(3.5-Number.EPSILON*2);assert.equal(before.bar,1);assert(before.notatedBeat<8);assert(before.notatedBeat>7.99);
 const plain=timing.create({});assert.deepEqual(plain.barAtBeat(5),{bar:2,notatedBeat:2,numerator:4,denominator:4,quarterOffset:1,barStart:4,barEnd:8});
 console.log('PASS bars follow the project meter, count beats in its denominator and keep a position before a bar line in its bar');
}
{
 assert.equal(timing.tempo({}),120);assert.equal(timing.tempo(null),120);assert.equal(timing.tempo({bpm:'80'}),120);assert.equal(timing.tempo({bpm:Infinity}),120);assert.equal(timing.tempo({bpm:1000}),400);assert.equal(timing.tempo({bpm:0}),20);
 assert.deepEqual(timing.meter({}),{numerator:4,denominator:4});assert.deepEqual(timing.meter({timeSignature:{numerator:3.9,denominator:8.9}}),{numerator:3,denominator:8});
 assert.deepEqual(timing.meter({timeSignature:{numerator:99,denominator:3}}),{numerator:16,denominator:4});assert.deepEqual(timing.meter({timeSignature:null}),{numerator:4,denominator:4});
 console.log('PASS a missing or unusable tempo or meter takes the default, one outside the project format the nearest limit');
}
{
 const map=timing.create({bpm:120});for(const value of [-1,1000001,Infinity,NaN,'1',true])assert.throws(()=>map.secondsAtBeat(value),/Timing:/);for(const value of [-1,map.maxSeconds+1,Infinity,'1'])assert.throws(()=>map.beatAtSeconds(value),/Timing:/);
 assert.throws(()=>map.barAtBeat(-1));assert.throws(()=>map.duration(4,3));assert.throws(()=>map.endingBeat(0,-1));assert.throws(()=>map.endingBeat(999999,2));assert.throws(()=>timing.durationFromBeats({bpm:120},0,-1));
 console.log('PASS positions and durations outside the supported song length are refused');
}
{
 const project={bpm:120,timeSignature:{numerator:4,denominator:4}},map=timing.create(project);project.bpm=30;project.timeSignature.numerator=2;
 assert.equal(map.secondsAtBeat(4),2);assert.equal(map.barAtBeat(3).numerator,4);assert.throws(()=>{map.bpm=99;});assert.equal(timing.create(project).secondsAtBeat(4),8);
 console.log('PASS a created timing is a snapshot: later edits of the project do not change it');
}
