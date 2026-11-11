/* Engine synchronization: queue the project for the native engine (coalesced, one at a
   time, after graph edits settle) and publish the MIDI input route for the selected sound. */
(function () {
  'use strict';

  const app = window.STApp;
  const { state, toast, request, stableJSON } = app;
  let syncTimer;
  let lastMIDIRoute = '';
  let syncGeneration = 0;
  let midiRouteProject = null;
  let midiRoutePending = null;
  let midiRouteEpoch = 0;

  async function waitForEffectMove() {
    const busy = window.STBusy;
    while (busy.any(['effectMove', 'routing', 'trackDuplicateApply'])) {
      if (busy.isBusy('effectMove')) await window.STStudio.waitForEffectMove();
      if (busy.isBusy('routing')) await window.STRouting.waitForApply();
      if (busy.isBusy('trackDuplicateApply')) await window.STTrackDuplicate.waitForApply();
    }
  }

  let pendingEngineSync = null;
  const runningEngineSyncs = new Set();
  let engineSyncTail = null;

  async function waitForEngineUpdate() {
    // NKS capture may itself be what a waiting sync needs to finish. Only native
    // updates already posted to the host participate in this direction of the gate.
    while (true) {
      const applying = Array.from(runningEngineSyncs).filter(job => job.phase === 'applying');
      if (!applying.length) return;
      await Promise.all(applying.map(job => job.promise.catch(() => undefined)));
    }
  }

  function cancelPendingEngineSync() {
    clearTimeout(syncTimer);
    pendingEngineSync = null;
    syncGeneration++;
  }

  function queueEngineSync(job) {
    clearTimeout(syncTimer);
    if (pendingEngineSync?.owner === job.owner
        && pendingEngineSync.generation === job.generation) job.replace ||= pendingEngineSync.replace;
    pendingEngineSync = job;
    syncTimer = setTimeout(() => startEngineSync(job), 100);
  }

  function startEngineSync(job, capture = false) {
    if (pendingEngineSync === job) {
      clearTimeout(syncTimer);
      pendingEngineSync = null;
    }
    job.capture ||= capture;
    const previous = engineSyncTail;
    job.promise = (async () => {
      if (previous) await previous.catch(() => undefined);
      await waitForEffectMove();
      if (job.superseded || job.generation !== syncGeneration || job.owner !== state.project) return false;
      // A busy operation defers the sync (it runs again when queued work drains); a sync that must
      // produce a project now fails with the reason. Export preparation may explicitly drain edits
      // queued before its lock; otherwise they wait until the export finishes or is canceled.
      const deferral = window.STBusy.blocker('sync', [
        !job.lineInputCommit && !job.capture && 'lineInputs',
        !job.kitEdit && 'kitChange',
        !job.instrumentEdit && 'instrumentApply',
        !job.capture && 'export'
      ]);
      if (deferral) {
        if (job.makeProject) throw new Error(deferral.message);
        queueEngineSync({ ...job, promise: null });
        return false;
      }
      job.phase = 'waiting-native-edits';
      const captured = window.STPendingEdits.hasPending() ? await window.STPendingEdits.flush() : true;
      if (job.superseded || job.generation !== syncGeneration || job.owner !== state.project) return false;
      if (captured === false) throw new Error('Pending plug-in controls could not be captured. The audio project was not replaced.');
      const project = job.makeProject ? job.makeProject() : job.owner;
      if (project == null) return false;
      if (typeof project !== 'object'
          || Array.isArray(project)
          || typeof project.then === 'function') throw new Error('An audio project update requires a synchronous project snapshot.');
      if (job.makeProject) {
        // The factory just cloned authoritative state, including edits captured
        // while it waited. Do not let their older normal sync undo a live preview.
        const represented = other => other !== job
            && !other.makeProject
            && other.phase !== 'applying'
            && other.owner === job.owner
            && other.generation === job.generation;
        if (pendingEngineSync && represented(pendingEngineSync)) {
          job.replace ||= pendingEngineSync.replace;
          clearTimeout(syncTimer);
          pendingEngineSync = null;
        }
        for (const other of runningEngineSyncs) if (represented(other)) {
          job.replace ||= other.replace;
          other.superseded = true;
        }
      }
      job.phase = 'applying';
      const publishedSamplePaths = project.instruments.map(inst => inst.samplePath);
      const result = await request('engine.project', {
        project,
        ...(job.replace ? { replace: true } : {})
      }, {
        instrumentEdit: job.instrumentEdit === true,
        kitEdit: job.kitEdit === true,
        lineInputCommit: job.lineInputCommit === true || job.capture === true
      });
      if (result !== false) {
        app.hydrateSampleMetadata(job.owner, publishedSamplePaths);
        if (job.owner === state.project && job.generation === syncGeneration) updateMIDIRoute();
      }
      return result;
    })();
    runningEngineSyncs.add(job);
    engineSyncTail = job.promise;
    const finished = () => {
      runningEngineSyncs.delete(job);
      if (engineSyncTail === job.promise) engineSyncTail = null;
    };
    job.promise.then(finished, error => {
      finished();
      toast(error.message, 'error');
    });
    return job.promise;
  }

  function syncEngineProject(makeProject, options = {}) {
    if (typeof makeProject !== 'function') return Promise.reject(new Error('Provide a factory for the current audio project.'));
    const owner = state.project;
    const replace = options.replace === true
        || !!(pendingEngineSync?.owner === owner && pendingEngineSync.generation === syncGeneration && pendingEngineSync.replace);
    return startEngineSync({
      owner,
      generation: syncGeneration,
      replace,
      makeProject,
      lineInputCommit: options.lineInputCommit === true,
      instrumentEdit: options.instrumentEdit === true,
      kitEdit: options.kitEdit === true
    }, options.capture === true);
  }

  function syncEngine(immediate = false, replace = false) {
    clearTimeout(syncTimer);
    const job = {
      owner: state.project,
      generation: syncGeneration,
      replace: replace || !!(pendingEngineSync?.owner === state.project && pendingEngineSync.replace)
    };
    pendingEngineSync = null;
    if (immediate) return startEngineSync(job).catch(() => undefined);
    queueEngineSync(job);
  }

  async function drainEngineSync() {
    const failures = [];
    do {
      const jobs = Array.from(runningEngineSyncs);
      for (const job of jobs) job.capture = true;
      const pending = pendingEngineSync;
      if (pending) {
        startEngineSync(pending, true);
        jobs.push(pending);
      }
      if (jobs.length) await Promise.all(jobs.map(job => job.promise.catch(error => {
        failures.push(error);
      })));
      await waitForEffectMove();
    } while (pendingEngineSync || runningEngineSyncs.size);
    return failures;
  }

  function resetMIDIRoutePublication() {
    midiRouteEpoch++;
    midiRouteProject = null;
    lastMIDIRoute = '';
  }

  function adoptSavedMIDIRoute(previous, saved) {
    // Save replaces the canonical object without replacing the native session.
    // Retain only destinations actually published by an acknowledged engine job.
    if (midiRouteProject === previous
        && stableJSON(previous.tracks.map(t => t.routingID)) === stableJSON(saved.tracks.map(t => t.routingID))) midiRouteProject = saved;
  }

  function desiredMIDIRoute() {
    if (!state.native || !state.engineReady || !state.project) return null;
    return {
      project: state.project,
      key: state.track + ':' + state.instrument,
      epoch: midiRouteEpoch,
      payload: { track: state.track, instrument: state.instrument }
    };
  }

  function updateMIDIRoute() {
    const target = desiredMIDIRoute();
    if (!target || midiRoutePending || midiRouteProject === target.project && lastMIDIRoute === target.key) return;
    midiRoutePending = target;
    request('midi.configure', target.payload).then(result => {
      if (result === false) throw Error('The MIDI destination was not accepted.');
      if (state.project === target.project && midiRouteEpoch === target.epoch) {
        midiRouteProject = target.project;
        lastMIDIRoute = target.key;
      }
    }).catch(error => {
      const current = desiredMIDIRoute();
      if (current?.project === target.project
          && current.key === target.key
          && current.epoch === target.epoch) toast(error.message, 'error');
    }).finally(() => {
      midiRoutePending = null;
      const current = desiredMIDIRoute();
      // Coalesce selection changes made during the request, but don't loop-retry a
      // rejected destination. A later refresh or successful project ACK can retry.
      if (current
          && (current.project !== target.project || current.key !== target.key || current.epoch !== target.epoch)) updateMIDIRoute();
    });
  }

  Object.assign(app, {
    waitForEffectMove, waitForEngineUpdate, cancelPendingEngineSync,
    syncEngineProject, syncEngine, drainEngineSync, resetMIDIRoutePublication, adoptSavedMIDIRoute,
    updateMIDIRoute
  });
})();
