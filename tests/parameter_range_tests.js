'use strict';
// Production app renderer and bindings, with the mounted scalar DOM fixture.
const assert = require('node:assert/strict'), fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');
const web = require('./helpers/web');
const fixture = fs.readFileSync(path.join(__dirname,'scalar_field_focus_tests.js'),'utf8');
const makeHarness = new Function('require','__dirname',fixture.slice(0,fixture.indexOf('const tests = [], test ='))+'\nreturn harness;')(require,__dirname);
function harness() {
  const h = makeHarness('instrument');
  h.context.Event = class { constructor(type,options) {this.type=type;Object.assign(this,options);} };
  web.load(h.context, 'fine-control.js');
  h.ST.refresh();
  for (const el of h.app.querySelectorAll('input')) el.dispatchEvent = event => el.fire(event.type);
  return h;
}
const tests = [], test = (name,run) => tests.push({name,run});
test('typed native parameters clamp to visible bounds and integer controls stay integral', () => {
  for (const [key,value,expected] of [['volume',99,1],['cutoff',-50,20],['cutoff',1e20,20000],['rootNote',61.8,62],['bitDepth',7.8,8],['pan',-8,-1],['attack',99,60],['release',-5,.0001]]) {
    const h=harness(),el=h.edit('param-value-'+key,value);el.fire('change');
    assert.equal(h.state.project.instruments[0][key],expected,key);
  }
});
test('amplitude envelopes expose the engine range without clamping saved values during rendering', () => {
  const h=harness();Object.assign(h.state.project.instruments[0],{attack:25,decay:30,release:45});h.ST.refresh();
  for(const key of ['attack','decay','release']){const number=h.get('param-value-'+key),slider=h.get('param-'+key);assert.equal(number.max,'60',key);assert.equal(Number(number.value),h.state.project.instruments[0][key]);assert(Number(slider.value)>=0&&Number(slider.value)<=1);}
  assert.equal(h.get('param-value-attack').min,'0');assert.equal(h.get('param-value-release').min,'0.0001');
});
test('frequency and envelope sliders reserve useful travel for low values while numbers remain engine units', () => {
  const h=harness(),F=h.window.STFine;
  for(const [key,limit]of[['cutoff',1000],['attack',1],['decay',1],['release',1]]){
    const slider=h.get('param-'+key);assert(F.curved(slider),key);slider.value='.5';slider.fire('input');
    const actual=Number(h.get('param-value-'+key).value);assert(actual>0&&actual<limit,key+': midpoint '+actual);
    slider.fire('change');assert.equal(h.state.project.instruments[0][key],actual,key);
  }
});
test('Option arrows make precise edits, Shift is coarse, and caret or automate shortcuts are untouched', () => {
  const h=harness(),el=h.get('param-value-cutoff');el.value='125';el.fire('input');
  assert(h.key(el,'ArrowUp',{altKey:true}).defaultPrevented);assert.equal(Number(el.value),126);
  assert(h.key(el,'ArrowUp',{shiftKey:true}).defaultPrevented);assert.equal(Number(el.value),200);
  assert(!h.key(el,'ArrowLeft').defaultPrevented);assert(!el.fire('keydown',{key:'a',altKey:true}).defaultPrevented);
  el.fire('blur');assert.equal(h.state.project.instruments[0].cutoff,200);assert.equal(h.state.undo.length,1);
});
test('curved keyboard edits write Hertz, not slider positions, into project and number twin', () => {
  const h=harness(),F=h.window.STFine,el=h.get('param-cutoff');el.value=F.elementFromLane(el,125);el.fire('input');
  h.key(el,'ArrowUp',{altKey:true});assert.equal(Number(h.get('param-value-cutoff').value),126);el.fire('blur');
  assert.equal(h.state.project.instruments[0].cutoff,126);
});
test('automation lane and recorder conversions agree with perceptual control travel including zero endpoints', () => {
  const h=harness();
  const source = process.env.SUPERTRACKER_STUDIO_SOURCE ? fs.readFileSync(process.env.SUPERTRACKER_STUDIO_SOURCE, 'utf8') : web.source('studio.js');
  web.load(h.context, 'studio.js', { text: source });
  web.load(h.context, 'automation-song.js', { path: process.env.SUPERTRACKER_AUTOMATION_SONG_SOURCE });
  web.load(h.context, 'modulation.js', { path: process.env.SUPERTRACKER_MODULATION_SOURCE });
  web.load(h.context, 'automation-record.js');
  const functions=source.slice(source.indexOf('  function toUnit(value, info)'),source.indexOf('  function toUnit(value, info)')+3000);
  const end=functions.indexOf('\n  function ',functions.indexOf('  function fromUnit')+10);
  vm.runInContext('(()=>{const clamp=(v,a,b)=>Math.max(a,Math.min(b,v));'+functions.slice(0,end)+';window.__rangeTest={toUnit,fromUnit};})()',h.context);
  for(const key of ['attack','decay','release','grainSize','filterRelease','lfoRate','lfoDepth']){
    const spec=h.window.STStudio.target(0,key),F=h.window.STFine,convert=h.window.__rangeTest,record=h.window.STAutomationRecord._test;
    assert(spec.fineCurve,key);assert.equal(convert.fromUnit(0,spec),spec.min);assert.equal(convert.fromUnit(1,spec),spec.max);
    for(const position of [0,.1,.25,.5,.75,1]){
      const expected=F.quantize(F.fromPosition(position,{...spec,...spec.fineCurve}),{...spec,...spec.fineCurve});
      assert.equal(h.window.STAutomationSong.fromUnit(position,spec),expected,'Song '+key);
      assert.equal(convert.fromUnit(position,spec),expected,key);
      assert.equal(record.fromUnit(position,spec,spec.fineCurve),expected,key);
      assert(Number.isFinite(convert.toUnit(expected,spec)),key);
    }
  }
  assert.equal(h.window.STStudio.target(0,'attack').min,0);assert.equal(h.window.STStudio.target(0,'release').min,.0001);
});
(async()=>{let failed=0;for(const{name,run}of tests){try{await run();console.log('PASS '+name);}catch(error){failed++;console.error('FAIL '+name+'\n'+error.stack);}}console.log(`${tests.length-failed}/${tests.length} parameter range groups passed`);if(failed)process.exitCode=1;})();
