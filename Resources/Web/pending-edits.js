/* Pending native edits: control surfaces (plug-in parameter dialog, NKS knobs, input capture, the
   agent, MK3, automation write, instrument edits) register what they still have in flight, so a
   save, undo or engine sync can ask one place before it snapshots the project.

     STPendingEdits.register(id, { hasPending(options), generation(), flush(context), order })
       hasPending  true while an edit is not yet settled (options.mk3Edit exempts the caller's own MK3 edit),
       generation  a counter that changes with every committed native edit,
       flush       settle the pending edit; false aborts the flush. context is shared by one flush run.
     STPendingEdits.hasPending(options), .generation(), .flush() aggregate the registered sources;
     flush runs them in ascending order. */
(function () {
  'use strict';

  const sources = new Map();
  const ordered = () => [...sources.entries()].sort((a, b) => (a[1].order ?? 100) - (b[1].order ?? 100));

  function register(id, source) {
    sources.set(id, source);
    return () => { if (sources.get(id) === source) sources.delete(id); };
  }

  const hasPending = (options = {}) => ordered().some(([, source]) => !!source.hasPending?.(options));

  /** One stamp for all sources: equal stamps mean no native edit landed in between. */
  const generation = () => ordered().map(([id, source]) => `${id}=${source.generation?.() ?? 0}`).join(':');

  async function flush() {
    const context = {};
    for (const [, source] of ordered()) {
      if (!source.flush) continue;
      let result = source.flush(context);
      if (typeof result?.then === 'function') result = await result;
      if (result === false) return false;
    }
    return true;
  }

  window.STPendingEdits = { register, hasPending, generation, flush };
})();
