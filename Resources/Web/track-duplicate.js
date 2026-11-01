/* Prepare an independent track copy before making one undoable project edit. */
(() => {
  'use strict';
  const app = () => window.ST;
  const { clone: copy } = window.STUtil;
  const stable = value => value === null || typeof value !== 'object'
      ? JSON.stringify(value)
      : Array.isArray(value)
      ? '[' + value.map(stable).join(',') + ']'
      : '{' + Object.keys(value).sort().map(key => JSON.stringify(key) + ':' + stable(value[key])).join(',') + '}';
  const identity = plugin => stable([plugin.instanceID, plugin.owner, plugin.track, plugin.kind, plugin.slot, plugin.component, plugin.bundlePath]);
  const identities = plugins => Array.isArray(plugins) ? plugins.map(identity).sort().join('\n') : null;
  let operation = null;

  function blocked() {
    return window.STBusy.any(['session', 'export', 'recording', 'pluginEdit', 'effectMovePrep', 'routingPrep']);
  }
  function updateButtons() {
    for (const button of document.querySelectorAll('[data-duplicate-track]')) {
      button.disabled = !!operation || app().state.project.tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity);
      button.setAttribute('aria-busy', operation ? 'true' : 'false');
    }
  }
  function interceptPluginEvent(plugins) {
    // A native proposal must not overwrite the original model before its undo
    // snapshot exists. Unrelated publications still reach their usual owner.
    if (!operation?.proposal || identities(plugins) !== operation.identities) return false;
    operation.pluginEvent = plugins;
    return true;
  }
  /* One duplication (a track or a whole group): build the proposal, publish it to the audio host, check every new
     plug-in instance loaded, then commit it as one Undo step. spec: sameSource(), build(), commit(project, proposal),
     noun ('track' / 'group'), label (Undo), done (toast). */
  async function run(spec) {
    if (operation) return false;
    if (blocked()) { app().toast(`Finish the current audio or project operation before duplicating a ${spec.noun}.`); return false; }
    const state = app().state, owner = state.project, view = state.view;
    let release;
    const entry = { phase: 'preparing', proposal: null, promise: new Promise(resolve => { release = resolve; }) };
    operation = entry; updateButtons();
    let attempted = false, committed = false, baseline, nativeGeneration, proposal;
    const sameSource = () => state.project === owner && spec.sameSource();
    const unchanged = () => sameSource()
        && stable(owner) === baseline
        && window.STNKSControls?.editGeneration === nativeGeneration
        && !window.STNKSControls?.hasPendingEdits;
    const releaseGate = () => { entry.phase = 'settled'; release(); };
    try {
      // Capacity/shape errors need no state capture or native graph mutation.
      spec.build();
      await app().waitForGraphEdit?.();
      if (!sameSource()) throw new Error(`The source ${spec.noun} changed. Duplicate it again from the current project.`);
      if (await app().captureNativeEdits?.() === false) throw new Error(`Current plug-in edits could not be captured. The ${spec.noun} was not duplicated.`);
      if (!sameSource() || blocked()) throw new Error('The project or audio operation changed while preparing the duplicate. Try again.');
      const result = await app().syncEngineProject(() => {
        if (!sameSource() || blocked()) return null;
        proposal = spec.build();
        baseline = stable(owner); nativeGeneration = window.STNKSControls?.editGeneration;
        entry.proposal = proposal.project; entry.identities = identities(proposal.project.audioPlugins || []);
        // The strict helper has passed its graph gate before invoking this factory.
        // Hold later native edits until verification and UI commit are complete.
        entry.phase = 'applying'; attempted = true;
        return copy(proposal.project);
      });
      if (result === false || !proposal) throw new Error(`${spec.noun === 'group' ? 'Group' : 'Track'} duplication was canceled before the audio project was prepared.`);
      if (!unchanged()) throw new Error('The project changed while duplicating. Your newer edits were preserved.');
      const freshIDs = new Set(proposal.newPluginInstanceIDs || Object.values(proposal.instanceMap));
      for (const plugin of proposal.project.audioPlugins || []) if (freshIDs.has(plugin.instanceID)) {
        await app().request('plugin.parameters',
            { track: plugin.track, ...(plugin.owner
                ? { owner: plugin.owner }
                : {}), kind: plugin.kind, slot: plugin.slot, instanceID: plugin.instanceID, addresses: [] });
        if (!unchanged()) throw new Error('The project changed while verifying the copied plug-ins. Your newer edits were preserved.');
      }
      const captureFailure = entry.pluginEvent?.find(plugin => plugin.stateError);
      if (captureFailure) throw new Error(`${captureFailure.name || 'A copied plug-in'}: ${captureFailure.stateError}`);
      if (window.STBusy.any(['routing', 'export'])) throw new Error(`Another audio operation started. The ${spec.noun} was not duplicated.`);
      app().mutate(spec.label, project => {
        for (const key of Object.keys(project)) delete project[key];
        Object.assign(project, copy(proposal.project));
        spec.commit(project, proposal);
        state.instrument = project.tracks[state.track].instrument ?? 0;
        state.mixerOwner = `track:${project.tracks[state.track].routingID}`; state.selection = null;
        committed = true;
      });
      if (!committed) throw new Error(`The ${spec.noun} could not be committed. Your original project was preserved.`);
      if (state.view === view && !state.modal) {
        if (view === 'mixer') window.STStudio?.focusSelectedChannel?.();
        else if (view === 'pattern') { const grid = document.getElementById('tracker'); grid?.focus({ preventScroll: true }); document.querySelector(`[data-cell="${state.track}:${state.row}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' }); }
      }
      app().toast(spec.done, 'success');
      return true;
    } catch (error) {
      let message = error.message || String(error);
      // Release our gate before asking the shared helper to restore the latest
      // model. Its factory must see edits made while the proposal was in flight.
      releaseGate();
      if (attempted && !committed) {
        try { if (await app().syncEngineProject(() => copy(app().state.project),
            { replace: true }) === false) throw new Error('The session changed again before audio restoration completed.'); }
        catch (restoreError) { message += ` Could not restore current audio: ${restoreError.message || restoreError}`; }
      }
      app().toast(message, 'error');
      return false;
    } finally { releaseGate(); if (operation === entry) operation = null; updateButtons(); }
  }
  function duplicate(index = app().state.track) {
    const state = app().state, owner = state.project, source = owner.tracks[index];
    return run({
      noun: 'track', label: 'Duplicate track', done: `Duplicated ${source?.name || 'track'} with independent instruments.`,
      sameSource: () => owner.tracks[index] === source,
      build: () => window.STTrackDuplicateProject.build(owner, index, app().uid),
      commit(project, proposal) {
        state.track = proposal.trackIndex;
        // A copied group member stays in its group, right after its source track.
        const groups = window.STGroups;
        if (groups?.ofTrack(project, proposal.trackIndex)) {
          const order = project.tracks.map((_, i) => i).filter(i => i !== proposal.trackIndex);
          order.splice(order.indexOf(proposal.sourceTrackIndex) + 1, 0, proposal.trackIndex);
          state.track = groups.reorder(project, order)[proposal.trackIndex];
        }
      }
    });
  }
  /* Duplicate a whole track group (a kit): every member track and the group bus with its inserts, sends and
     automation, as a new group right after the source group (Maschine's DUPLICATE + group). One Undo step. */
  function duplicateGroup(groupID) {
    const state = app().state, owner = state.project, groups = window.STGroups, group = groups?.byID(owner, groupID);
    if (!group) return Promise.resolve(false);
    const members = groups.members(owner, group).map(i => owner.tracks[i]), cursor = Math.max(0, groups.members(owner, group).indexOf(state.track));
    return run({
      noun: 'group', label: 'Duplicate group', done: `Duplicated ${group.name || 'group'} with ${members.length} track${members.length === 1 ? '' : 's'} and its bus.`,
      sameSource: () => groups.byID(owner, groupID) === group && groups.members(owner, group).map(i => owner.tracks[i]).every((track, i) => track === members[i]),
      build: () => window.STTrackDuplicateProject.buildGroup(owner, groupID, app().uid),
      commit(project, proposal) {
        const copies = new Set(proposal.trackIndices), order = project.tracks.map((_, i) => i).filter(i => !copies.has(i));
        order.splice(order.indexOf(proposal.sourceTrackIndices.at(-1)) + 1, 0, ...proposal.trackIndices);
        const map = groups.reorder(project, order);
        state.track = map[proposal.trackIndices[Math.min(cursor, proposal.trackIndices.length - 1)]];
      }
    });
  }
  function contextMenu(event) {
    const header = event.target.closest?.('th[data-track]');
    if (!header) return false;
    const owner = app().state.project, index = Number(header.dataset.track), source = owner.tracks[index];
    event.preventDefault(); document.querySelector('.context-menu')?.remove();
    const menu = document.createElement('div'), button = document.createElement('button');
    menu.className = 'context-menu';
    menu.style.left = `${Math.min(event.clientX, innerWidth - 240)}px`;
    menu.style.top = `${Math.min(event.clientY, innerHeight - 100)}px`;
    button.textContent = 'Duplicate this track';
    button.disabled = !!operation || app().state.project.tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity);
    button.onclick = () => {
      menu.remove();
      if (app().state.project !== owner || owner.tracks[index] !== source) { app().toast('The source track changed. Open its menu again.'); return; }
      void duplicate(index);
    };
    menu.appendChild(button); document.body.appendChild(menu); button.focus();
    menu.onkeydown = key => { if (key.key === 'Escape') { key.preventDefault(); menu.remove(); document.getElementById('tracker')?.focus(); } };
    setTimeout(() => document.addEventListener('click', () => menu.remove(), { once: true }), 0);
    return true;
  }
  window.STTrackDuplicate = {
    duplicate,
    duplicateGroup,
    contextMenu,
    interceptPluginEvent,
    get isBusy() {
      return !!operation;
    },
    get isApplying() {
      return operation?.phase === 'applying';
    },
    waitForApply: () => operation?.promise || Promise.resolve()
  };
  // A duplication owns the plug-in events of the copies it is creating.
  window.STNativeEvents.intercept(window.STNativeEvents.PLUGIN_EVENTS,
      payload => window.STTrackDuplicate?.interceptPluginEvent(window.STNativeEvents.plugins(payload)), 50);
})();
