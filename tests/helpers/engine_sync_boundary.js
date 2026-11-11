'use strict';
const assert = require('node:assert/strict');

// Caller tests control the shared synchronizer's wait boundary. The production
// synchronizer's queue, graph and NKS settlement behavior is tested separately.
module.exports = function installEngineSyncBoundary(ST) {
  const syncs = [];
  ST.syncEngineProject = async (makeProject, options = {}) => {
    assert.equal(typeof makeProject, 'function', 'Project payloads must be built after synchronization waits');
    const owner = ST.state.project;
    syncs.push({ owner, options });
    await ST.beforeEngineSync?.();
    if (ST.state.project !== owner) return false;
    const project = makeProject();
    if (project == null) return false;
    return ST.request('engine.project', { project, ...(options.replace ? { replace: true } : {}) });
  };
  return syncs;
};
