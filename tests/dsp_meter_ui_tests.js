const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');
const web=require('./helpers/web');
const source=web.source('app.js');
const context={formatNativeDSPUsage:web.app().app.formatNativeDSPUsage};
assert.equal(typeof context.formatNativeDSPUsage,'function','production DSP percentage formatter exists');
for(const [value,expected] of [[0,'DSP 0%'],[.1,'DSP 0%'],[.5,'DSP 1%'],[.99,'DSP 1%'],[1,'DSP 1%'],[1.01,'DSP 1%'],[12.2,'DSP 12%'],[115,'DSP 115%'],[-1,'DSP 0%'],[NaN,'DSP 0%'],[Infinity,'DSP 0%'],[undefined,'DSP 0%']]){
  assert.equal(context.formatNativeDSPUsage(value),expected,`${String(value)} is native percent, not a fraction`);
}
assert(source.includes("$('#cpu').textContent = formatNativeDSPUsage(state.status.cpu)"),'transport display uses percentage formatter');
const css=fs.readFileSync(path.join(__dirname,'../Resources/Web/styles.css'),'utf8');
const declarations=selector=>Object.fromEntries((css.match(new RegExp(selector.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')+'\\s*\\{([^}]+)\\}'))?.[1]||'').split(';').filter(value=>value.includes(':')).map(value=>value.split(':').map(part=>part.trim())));
const meter=declarations('.statusbar #cpu'),nativeMeter=declarations('body.native .statusbar #cpu');
assert(meter['font-variant-numeric']?.includes('tabular-nums'),'changing DSP digits retain equal advance widths');
assert.equal(meter['white-space'],'nowrap','the meter never splits across lines');
assert.equal(meter['text-align'],'right','DSP values align at the status bar edge');
assert.match(nativeMeter.flex,/^0 0 (\d+(?:\.\d+)?)ch$/,'native meter reserves a non-growing, non-shrinking character width');
assert.equal(nativeMeter.width,nativeMeter.flex.split(' ')[2],'intrinsic status sizing uses the same reserved meter width');
assert(Number.parseFloat(nativeMeter.width)>=10,'meter reserves enough room for normal readings and four-digit overloads');
assert.equal(nativeMeter['min-width'],'0','content-based flex minimum cannot change the reserved width');
assert(!meter.width&&!meter.flex,'interface preview keeps the full Native audio unavailable message');
console.log('PASS DSP meter: native percentage units, finite fallback, saturation and overload display (12 cases)');
console.log('PASS DSP layout contract: tabular digits, fixed native flex width and unconstrained offline message');
