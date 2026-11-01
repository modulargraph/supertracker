'use strict';
// Real app, mixer (studio.js), routing and kit-group modules with a synthetic asynchronous bridge
// (built on the instrument_edits_ui_tests.js harness). Shared by the mixer, routing, kit and browser UI tests.
const fs = require('node:fs'), path = require('node:path');
const web = require('./web');
const template = fs.readFileSync(path.join(__dirname, '..', 'instrument_edits_ui_tests.js'), 'utf8');
const prefix = template.slice(0, template.indexOf('async function run(){'))
    .replace('return {S,window,calls,handlers,element,plugin,', 'return {S,window,calls,handlers,element,plugin,context,timers,document,')
    .replace("{id,innerHTML:'',dataset:{}", "{id,style:{},innerHTML:'',dataset:{}")
    .replace('focus(){},', 'focus(){document.activeElement=this;},')
    .replace('addEventListener(){},setAttribute', 'addEventListener(type,fn){(this.listeners||={})[type]=fn;},setAttribute')
    .replace('setTimeout:(f)=>{timers.set(next,f);', 'setTimeout:(f,delay=0)=>{timers.set(next,{f,delay});');
// The template's own requires are relative to tests/.
const testsRequire = name => require(name.startsWith('./helpers/') ? './' + name.slice(10) : name);
const base = new Function('require', '__dirname', prefix + '\nreturn {harness,deferred,until};')(testsRequire, path.join(__dirname, '..'));

function harness() {
  const h = base.harness();
  for (const file of ['routing-project.js', 'kit-groups.js', 'studio.js', 'routing.js']) web.load(h.context, file);
  h.G = h.window.STGroups;
  h.KG = h.window.STKitGroups;
  h.S.state.project.routing = { version: 1, mixModel: 'standard', buses: [], outputs: {}, sends: [], sidechains: [] };
  h.S.state.view = 'pattern';
  h.context.queueMicrotask = queueMicrotask;
  return h;
}

module.exports = { harness, base, deferred: base.deferred, until: base.until };
