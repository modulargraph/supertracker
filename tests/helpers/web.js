'use strict';
// Shared loader for the web UI sources (ui) in node vm contexts.
//
// - Script order comes from index.html, so tests load exactly what the app loads.
// - 'app.js' and 'mk3.js' are group names: they expand to the split module sets
//   (app-*.js and mk3-*.js, in index.html order). A frozen single-file baseline can
//   still be loaded with SUPERTRACKER_APP_SOURCE / ST_APP_SOURCE or ST_MK3_SOURCE.
// - The shared foundation scripts (util.js, busy.js, …) are loaded into a context
//   once, before the first module, exactly as index.html loads them first.
const fs = require('node:fs'), path = require('node:path'), vm = require('node:vm');

const WEB = path.join(__dirname, '../../ui');
const file = name => path.join(WEB, name);
const read = name => fs.readFileSync(file(name), 'utf8');

let order = null;
function scripts() {
  if (!order) order = [...read('index.html').matchAll(/<script src="([^"]+)"><\/script>/g)].map(m => m[1]);
  return order;
}

// Foundation scripts: shared helpers and registries every module may use.
const FOUNDATION = ['util.js', 'busy.js', 'native-events.js', 'pending-edits.js', 'routing-project.js', 'track-groups.js', 'project-timing.js'];
const foundation = () => FOUNDATION.filter(name => scripts().includes(name));

const GROUPS = {
  'app.js': () => scripts().filter(name => /^app-.*\.js$/.test(name)),
  'mk3.js': () => scripts().filter(name => /^mk3-.*\.js$/.test(name) && !['mk3-screens.js', 'mk3-sim.js'].includes(name))
};
const OVERRIDES = {
  'app.js': () => process.env.SUPERTRACKER_APP_SOURCE || process.env.ST_APP_SOURCE,
  'mk3.js': () => process.env.ST_MK3_SOURCE
};

/** The files a name stands for, in load order ('app.js' → every app-*.js module). */
function files(name) {
  const group = GROUPS[name]?.();
  return group && group.length ? group : [name];
}

/** Source text of a name (a group is concatenated in load order). */
function source(name) {
  const override = OVERRIDES[name]?.();
  if (override) return fs.readFileSync(override, 'utf8');
  return files(name).map(read).join('\n');
}

const prepared = new WeakSet();
/** Load the foundation scripts into a context once. */
function prepare(context) {
  if (prepared.has(context)) return context;
  prepared.add(context);
  for (const name of foundation()) vm.runInContext(read(name), context, { filename: name });
  return context;
}

/**
 * Run a web script (or group) in a vm context, after the foundation.
 * options.path runs a different file in its place (frozen baselines);
 * options.text runs the given source text in its place.
 */
function load(context, name, options = {}) {
  prepare(context);
  const override = options.path || OVERRIDES[name]?.();
  if (options.text != null) return vm.runInContext(options.text, context, { filename: name });
  if (override) return vm.runInContext(fs.readFileSync(override, 'utf8'), context, { filename: name });
  let result;
  for (const part of files(name)) result = vm.runInContext(read(part), context, { filename: part });
  return result;
}

/** Load several scripts in order. */
function loadAll(context, names, options = {}) {
  for (const name of names) load(context, name, options[name] || {});
  return context;
}

/** Create a context from a sandbox object and load scripts into it. */
function run(names, sandbox = {}) {
  const context = vm.isContext(sandbox) ? sandbox : vm.createContext(sandbox);
  loadAll(context, Array.isArray(names) ? names : [names]);
  return context;
}

/** vm.runInNewContext replacement: run source text for a web file in a fresh context. */
function runText(name, text, sandbox = {}, options) {
  const context = vm.isContext(sandbox) ? sandbox : vm.createContext(sandbox);
  prepare(context);
  return vm.runInContext(text, context, { filename: name, ...(typeof options === 'object' ? options : {}) });
}

/**
 * Load the app modules with some internals replaced. App modules destructure what earlier
 * modules export, so stubs are written onto window.STApp after every module: later modules
 * see the stubs, and the module that owns a stubbed name keeps its own definition.
 */
function loadAppWith(context, stubs = {}) {
  prepare(context);
  const window = context.window;
  for (const part of files('app.js')) {
    vm.runInContext(read(part), context, { filename: part });
    Object.assign(window.STApp, stubs);
  }
  return window.STApp;
}

/**
 * The app modules loaded into a bare context (no DOM, not booted): for tests that
 * inspect the command catalogue, the command registry or the public ST surface.
 */
function app(sandbox = {}) {
  const window = { addEventListener() {}, ...sandbox.window };
  const context = run('app.js', { console, ...sandbox, window });
  return { context, window, ST: window.ST, app: window.STApp };
}

/**
 * Register a hand-made stand-in for a control surface (window[name] with hasPendingEdits,
 * editGeneration and flushEdits) as a pending-edits source, as its own module would.
 */
function pendingEditsStub(window, id, name, order) {
  window.STPendingEdits.register(id, {
    order,
    hasPending: () => !!window[name]?.hasPendingEdits,
    generation: () => window[name]?.editGeneration ?? 0,
    flush: () => window[name]?.flushEdits?.()
  });
}

module.exports = { pendingEditsStub, app, loadAppWith, runText, WEB, file, read, scripts, files, source, prepare, load, loadAll, run, FOUNDATION };
