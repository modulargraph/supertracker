'use strict';
// The real native built-in effect catalogue (ranges, units, curves, labels) as JSON, in the shape of the
// `builtin.effects` bridge command: tests/builtin_effect_metadata_dump.cpp compiled against the engine sources.
const fs = require('node:fs'), path = require('node:path'), { execFileSync } = require('node:child_process');

const ROOT = path.join(__dirname, '../..');
const ENGINE = path.join(ROOT, 'Sources/TrackerEngine');
const SOURCES = [path.join(ROOT, 'tests/builtin_effect_metadata_dump.cpp'), path.join(ENGINE, 'BuiltinEffects.cpp'), path.join(ENGINE, 'include/STBuiltinEffects.h'),
  ...fs.readdirSync(ENGINE).filter(name => /^Effects.*\.hpp$|^BuiltinEffectsCommon\.hpp$/.test(name)).map(name => path.join(ENGINE, name))];
const BINARY = path.join(ROOT, '.build/test-bin/builtin-effect-metadata');
let cached = null;

function metadata() {
  if (cached) return cached;
  const newest = Math.max(...SOURCES.map(file => fs.statSync(file).mtimeMs));
  if (!fs.existsSync(BINARY) || fs.statSync(BINARY).mtimeMs < newest) {
    fs.mkdirSync(path.dirname(BINARY), { recursive: true });
    const temporary = `${BINARY}.${process.pid}`;
    execFileSync('clang++', ['-std=c++17', '-O1', '-I', path.join(ENGINE, 'include'), '-I', ENGINE, SOURCES[0], SOURCES[1], '-o', temporary], { stdio: 'inherit' });
    fs.renameSync(temporary, BINARY);
  }
  cached = JSON.parse(execFileSync(BINARY, { encoding: 'utf8' }));
  return cached;
}

/** A plug-in parameter list for one built-in effect at its defaults, as plugin.parameters returns it. */
function parameterList(component, values = {}) {
  const effect = metadata().find(item => item.id === component);
  if (!effect) throw new Error(`Unknown built-in effect ${component}`);
  const normalized = p => {
    const v = Math.max(p.min, Math.min(p.max, values[p.id] ?? p.defaultValue));
    return p.max === p.min ? 0 : p.logarithmic && p.min > 0 ? Math.log(v / p.min) / Math.log(p.max / p.min) : (v - p.min) / (p.max - p.min);
  };
  return effect.parameters.map(p => ({ address: String(p.address), id: p.id, name: p.name, min: 0, max: 1, normalized: normalized(p), value: normalized(p),
    display: `${p.defaultValue} ${p.unit}`, unitName: p.unit, writable: true, automatable: true, ...(p.toggle ? { stepCount: 1 } : p.integer ? { stepCount: Math.round(p.max - p.min) } : {}) }));
}

module.exports = { metadata, parameterList };
