/* Plug-in parameter dialog: an accessible list of a plug-in's parameters with previewed
   edits that commit as one undo step. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, esc, state, toast, request, mutate, waitForEffectMove, waitForEngineUpdate,
    updateTransport, instParamElementValue, PARAM_ARROWS, fireParamEvent, openModal, closeModal
  } = app;
  // Generic controls retain their edit queue after the dialog closes. Save, Undo,
  // and project synchronization must settle it before replacing native state.
  const pluginParameterEdits = new Set();
  const pluginParameterCancellations = new WeakSet();
  app.pluginParameterGeneration = 0;
  let pluginParameterOpen = 0;
  let pluginParameterDialog = null;

  // The track whose mixer channel is `owner` (`track:<routingID>`); null for a bus, Master or an instrument chain.
  const trackOf = (project, owner) => project.tracks.find(track => `track:${track.routingID}` === owner) || null;

  function pluginParameterCurrent(edit) {
    if (state.project !== edit.project
        || edit.trackObject && trackOf(edit.project, edit.target.owner) !== edit.trackObject) return null;
    return edit.project.audioPlugins?.find(plugin => plugin.instanceID === edit.target.instanceID
        && plugin.component === edit.component
        && plugin.kind === edit.target.kind
        && (plugin.slot ?? 0) === (edit.target.slot ?? 0)
        && plugin.owner === edit.target.owner)
        || null;
  }

  function cancelPluginParameterEdit(edit) {
    if (edit.cancelled) return;
    edit.cancelled = true;
    clearTimeout(edit.timer);
    pluginParameterEdits.delete(edit);
    if (edit.revision > edit.committed && state.project === edit.project) {
      pluginParameterCancellations.add(edit.project);
      app.pluginParameterGeneration++;
      state.dirty = true;
      toast('A plug-in control edit was canceled because its instance or channel changed. Review the replacement before '
          + 'saving again.', 'error');
    }
  }

  function pendingPluginParameterEdits() {
    for (const edit of pluginParameterEdits) if (!pluginParameterCurrent(edit)) cancelPluginParameterEdit(edit);
    return pluginParameterCancellations.has(state.project)
        || [...pluginParameterEdits].some(edit => edit.project === state.project && edit.revision > edit.committed);
  }

  function updatePluginParameterStatus(edit, message) {
    edit.error = message;
    if (edit.mounted()) $('#plugin-param-status').textContent = message;
  }

  function previewPluginParameterEdit(edit) {
    const revision = edit.revision,
      values = [...edit.values];
    const job = edit.chain.catch(() => {}).then(async () => {
      await waitForEffectMove();
      await waitForEngineUpdate();
      if (!pluginParameterCurrent(edit)) {
        cancelPluginParameterEdit(edit);
        return false;
      }
      if (edit.revision !== revision || edit.committed >= revision) return true;
      for (const [address, value] of values) {
        const result = await request('plugin.parameter', { ...edit.target, address, value, normalized: true });
        if (!pluginParameterCurrent(edit)) {
          cancelPluginParameterEdit(edit);
          return false;
        }
        if (edit.revision === revision && edit.mounted()) {
          const label = $(`[data-param-label="${address}"]`);
          if (label) label.textContent = result?.display || value.toFixed(3);
        }
      }
      return true;
    });
    edit.chain = job;
    job.catch(error => {
      if (pluginParameterCurrent(edit)) {
        updatePluginParameterStatus(edit, error.message || 'Could not apply the control edit.');
        toast(edit.error, 'error');
      }
    });
    return job;
  }

  function rebindSavedPluginParameterDialog(previous, next) {
    const edit = pluginParameterDialog;
    if (!edit || edit.project !== previous || !edit.mounted() || edit.revision !== edit.committed) return;
    const replacement = next.audioPlugins?.find(plugin => plugin.instanceID === edit.target.instanceID
        && plugin.component === edit.component
        && plugin.kind === edit.target.kind
        && (plugin.slot ?? 0) === (edit.target.slot ?? 0)
        && plugin.owner === edit.target.owner);
    if (!replacement) {
      closeModal();
      return;
    }
    // Only the acknowledged Save replacement can retain the existing dialog.
    // Undo, Open and arbitrary same-slot replacements still cancel its ownership.
    edit.project = next;
    edit.trackObject = trackOf(next, edit.target.owner);
  }

  function commitPluginParameterEdit(edit) {
    clearTimeout(edit.timer);
    edit.timer = null;
    if (!pluginParameterCurrent(edit)) {
      cancelPluginParameterEdit(edit);
      return Promise.resolve(false);
    }
    if (edit.revision <= edit.committed || edit.queued >= edit.revision) return edit.chain;
    const revision = edit.revision,
      values = [...edit.values],
      preset = edit.preset;
    edit.queued = revision;
    const current = () => {
      if (pluginParameterCurrent(edit)) return true;
      cancelPluginParameterEdit(edit);
      return false;
    };
    const job = edit.chain.catch(() => {}).then(async () => {
      await waitForEffectMove();
      await waitForEngineUpdate();
      if (!current()) return false;
      if (preset !== null) {
        await request('plugin.preset', { ...edit.target, number: preset });
        if (!current()) return false;
      }
      for (const [address, value] of values) {
        const result = await request('plugin.parameter', { ...edit.target, address, value, normalized: true });
        if (!current()) return false;
        if (edit.revision === revision && edit.mounted()) {
          const label = $(`[data-param-label="${address}"]`);
          if (label) label.textContent = result?.display || value.toFixed(3);
        }
      }
      const snapshot = await request('plugin.state', edit.target);
      if (!current()) return false;
      if (snapshot?.stateError) throw new Error(String(snapshot.stateError));
      if (!snapshot
          || typeof snapshot !== 'object'
          || Array.isArray(snapshot)
          || typeof snapshot.state !== 'string'
          || !snapshot.state) throw new Error('The plug-in did not return a saved state. Retry the control edit before saving.');
      if (snapshot.instanceID !== edit.target.instanceID
          || snapshot.component !== edit.component
          || snapshot.kind !== edit.target.kind
          || (snapshot.slot ?? 0) !== (edit.target.slot ?? 0)
          || snapshot.owner !== edit.target.owner) throw new Error('The plug-in changed before its control edit could be saved.');
      // A newer gesture owns the final desired values; never publish an older
      // capture that would schedule engine.project over those pending writes.
      if (edit.revision !== revision) return true;
      let committed = false;
      mutate(preset === null ? 'plug-in parameter' : 'plug-in preset', project => {
        const old = pluginParameterCurrent(edit);
        if (!old) return;
        const replacement = { ...old, ...snapshot };
        project.audioPlugins[project.audioPlugins.indexOf(old)] = replacement;
        committed = true;
      }, { render: false, nativeCapture: true });
      if (!committed) throw new Error('The control edit is still pending. Finish the current operation and retry.');
      edit.committed = revision;
      edit.values.clear();
      edit.preset = null;
      pluginParameterEdits.delete(edit);
      updatePluginParameterStatus(edit, '');
      return true;
    });
    edit.chain = job;
    job.catch(error => {
      if (edit.queued === revision) edit.queued = edit.committed;
      if (pluginParameterCurrent(edit)) {
        updatePluginParameterStatus(edit, error.message || 'Could not save the control edit.');
        toast(edit.error, 'error');
      }
    });
    return job;
  }

  async function flushPluginParameterEdits() {
    const owner = state.project;
    try {
      while (true) {
        pendingPluginParameterEdits();
        if (pluginParameterCancellations.has(owner)) {
          pluginParameterCancellations.delete(owner);
          return false;
        }
        const pending = [...pluginParameterEdits].filter(edit => edit.project === owner);
        if (!pending.length) return state.project === owner;
        const results = await Promise.all(pending.map(commitPluginParameterEdit));
        if (state.project !== owner || results.includes(false)) return false;
      }
    } catch {
      return false;
    }
  }

  // Acknowledged input can carry parameter edits, so after an input flush the dialog flushes as well.
  window.STPendingEdits.register('pluginParameters', {
    order: 20,
    hasPending: () => pendingPluginParameterEdits(),
    generation: () => app.pluginParameterGeneration,
    flush: context => context.hadInput || pendingPluginParameterEdits() ? flushPluginParameterEdits() : true
  });

  window.STPluginParameters = {
    flushEdits: flushPluginParameterEdits,
    get hasPendingEdits() {
      return pendingPluginParameterEdits();
    },
    get editGeneration() {
      return app.pluginParameterGeneration;
    }
  };

  // Plug-in parameter keys (normalized 0..1): ↑↓ 1%, ⌥ 0.1%, ⇧ / Page 10%. A burst previews each step and commits once on release.
  let pluginParameterBurst = null;

  function endPluginParameterBurst() {
    const burst = pluginParameterBurst;
    if (!burst) return;
    pluginParameterBurst = null;
    clearTimeout(burst.timer);
    if (burst.input.isConnected !== false) fireParamEvent(burst.input, 'change');
  }

  function pluginParameterKey(e, input) {
    if (e.defaultPrevented
        || e.metaKey
        || e.ctrlKey
        || e.isComposing
        || input.disabled
        || !['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown'].includes(e.key)) return;
    e.preventDefault();
    e.stopPropagation?.();
    const direction = ['ArrowUp', 'ArrowRight', 'PageUp'].includes(e.key) ? 1 : -1,
      step = e.shiftKey || e.key.startsWith('Page') ? .1 : e.altKey ? .001 : .01;
    const current = Number.isFinite(Number(input.value)) ? Number(input.value) : 0,
      next = window.STFine?.stepParameter(current, direction, {
        fine: e.altKey,
        coarse: e.shiftKey || e.key.startsWith('Page')
      }, input) ?? Number(clamp(current + direction * step, 0, 1).toFixed(4));
    if (pluginParameterBurst && pluginParameterBurst.input !== input) endPluginParameterBurst();
    if (!pluginParameterBurst) pluginParameterBurst = { input, timer: null };
    clearTimeout(pluginParameterBurst.timer);
    pluginParameterBurst.timer = setTimeout(endPluginParameterBurst, 700);
    if (next === current) return;
    input.value = instParamElementValue(input, next);
    fireParamEvent(input, 'input');
  }

  async function showPluginParameters(target) {
    const owner = state.project,
      view = state.view,
      opening = ++pluginParameterOpen,
      previousDialog = $('.modal');
    const matches = plugin => plugin.kind === target.kind
        && (plugin.slot ?? 0) === (target.slot ?? 0)
        && plugin.owner === target.owner;
    const initial = owner.audioPlugins?.find(matches),
      instanceID = initial?.instanceID;
    try {
      if (!instanceID) throw new Error('The plug-in instance is not ready. Wait for it to finish loading, then reopen its parameters.');
      const edit = {
        project: owner,
        trackObject: trackOf(owner, target.owner),
        target: { ...target, instanceID },
        component: initial.component,
        revision: 0,
        committed: 0,
        queued: 0,
        values: new Map(),
        lastValues: new Map(),
        preset: null,
        chain: Promise.resolve(true),
        timer: null,
        error: '',
        mounted: () => false
      };
      const canOpen = () => opening === pluginParameterOpen
          && state.view === view
          && $('.modal') === previousDialog
          && !!pluginParameterCurrent(edit);
      if (!(await window.STPendingEdits.flush())) return;
      await waitForEffectMove();
      await waitForEngineUpdate();
      if (!canOpen()) return;
      const result = await request('plugin.parameters', edit.target);
      if (!canOpen()) return;
      const list = Array.isArray(result) ? result : result?.parameters || [],
        presets = result?.presets || pluginParameterCurrent(edit).presets || [];
      openModal('Plug-in parameters', `<div class="stack"><div class="row"><input id="plugin-param-search" class="grow" placeholder="Search parameters…">${presets.length
          ? `<select id="plugin-preset"><option value="">Factory preset…</option>${presets.map(p => `<option value="${p.number ?? p.id}">${esc(p.name)}</option>`).join('')}</select>`
          : ''}</div>`
          + `<div id="plugin-param-list" class="plugin-parameters">${list.map((param, index) => `<label class="parameter plugin-param-item" data-search="${esc(String(param.name || '').toLowerCase())}">`
          + `<span class="parameter-top"><span class="parameter-label">${esc(param.name)}</span>`
          + `<span class="muted mono" data-param-label="${param.address ?? param.id}" style="font-size:11px">${esc(param.display || param.value)}</span>`
          + `</span><input id="plugin-param-${index}" type="range" ${param.writable === false ? 'disabled' : ''} data-plugin-address="${param.address ?? param.id}"`
          + ` ${param.automatable === false || param.writable === false ? '' : `data-automate="plugin:${target.kind}:${target.slot ?? 0}:${param.address ?? param.id}" data-owner="${esc(target.owner)}"`
          + ` data-name="${esc(param.name)}"`}`
          + ` min="0" max="1" step="${window.STFine?.parameterStep(param) || .001}" data-parameter-steps="${window.STFine?.parameterSteps(param) || 0}"`
          + ` value="${param.normalized ?? ((param.value - param.min) / (param.max - param.min) || 0)}">`
          + `</label>`).join('')}</div>${!list.length
              ? '<div class="empty">This plug-in exposes no host parameters. Use its native editor.</div>'
              : ''}<p `
          + `id="plugin-param-status" role="status"></p></div>`);
      const root = $('#plugin-param-list');
      pluginParameterDialog = edit;
      edit.mounted = () => !!state.modal && $('#plugin-param-list') === root && !!pluginParameterCurrent(edit);
      $('#plugin-param-search').oninput = () => {
        if (!edit.mounted()) return;
        const q = $('#plugin-param-search').value.toLowerCase();
        $$('.plugin-param-item').forEach(el => el.hidden = !el.dataset.search.includes(q));
      };
      const stage = input => {
        if (!edit.mounted()
            || input.disabled
            || input.value === ''
            || !Number.isFinite(Number(input.value))) return false;
        const value = window.STFine?.snapParameter(input.value, input) ?? clamp(Number(input.value), 0, 1),
          address = input.dataset.pluginAddress;
        input.value = String(value);
        if (edit.lastValues.get(address) === value) return edit.revision > edit.committed;
        edit.lastValues.set(address, value);
        edit.values.set(address, value);
        edit.revision++;
        app.pluginParameterGeneration++;
        pluginParameterEdits.add(edit);
        state.dirty = true;
        updateTransport();
        updatePluginParameterStatus(edit, 'Applying control edit…');
        return true;
      };
      for (const input of $$('[data-plugin-address]', root)) {
        edit.lastValues.set(input.dataset.pluginAddress, Number(input.value));
        input.oninput = () => {
          if (!stage(input)) return;
          clearTimeout(edit.timer);
          edit.timer = setTimeout(() => {
            edit.timer = null;
            void previewPluginParameterEdit(edit);
          }, 35);
        };
        input.onchange = () => {
          if (stage(input)) return commitPluginParameterEdit(edit).catch(() => false);
        };
        input.onkeydown = e => pluginParameterKey(e, input);
        input.onkeyup = e => {
          if (pluginParameterBurst?.input === input && PARAM_ARROWS.includes(e.key)) endPluginParameterBurst();
        };
        input.onblur = () => {
          if (pluginParameterBurst?.input === input) endPluginParameterBurst();
        };
        window.STFine?.bindDrag(input);
      }
      if (presets.length) $('#plugin-preset').onchange = async () => {
        if (!edit.mounted() || $('#plugin-preset').value === '') return;
        edit.preset = Number($('#plugin-preset').value);
        edit.values.clear();
        edit.revision++;
        app.pluginParameterGeneration++;
        pluginParameterEdits.add(edit);
        state.dirty = true;
        updateTransport();
        updatePluginParameterStatus(edit, 'Applying preset…');
        try {
          if ((await commitPluginParameterEdit(edit)) && edit.mounted() && edit.revision === edit.committed) {
            closeModal();
            await showPluginParameters(edit.target);
          }
        } catch {}
      };
    } catch (error) {
      if (state.project === owner && opening === pluginParameterOpen) toast(error.message, 'error');
    }
  }

  Object.assign(app, {
    pendingPluginParameterEdits, rebindSavedPluginParameterDialog, flushPluginParameterEdits,
    showPluginParameters
  });
})();
