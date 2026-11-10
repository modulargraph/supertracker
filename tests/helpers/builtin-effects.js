'use strict';
// The real built-in effect catalogue (ranges, units, curves, labels) as JSON, in the shape of the
// `builtin.effects` bridge command: tests/fixtures/builtin-effects.json, which the engine crate generates
// from its catalogue and keeps up to date (crates/tracker-engine/tests/effects_metadata.rs fails when the
// file is stale; UPDATE_EFFECT_METADATA=1 cargo test -p tracker-engine --test effects_metadata rewrites it).
const fs = require('node:fs'), path = require('node:path');

const FIXTURE = path.join(__dirname, '../fixtures/builtin-effects.json');
let cached = null;

function metadata() {
  if (!cached) cached = JSON.parse(fs.readFileSync(FIXTURE, 'utf8'));
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
