/* Verified NKS pages for the loaded instrument; no parameter-index guessing. */
(() => {
  'use strict';
  const S = () => window.ST;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const entries = new WeakMap(), bindings = new Set(), editingEntries = new Set();
  let savedViews = new WeakMap();
  let editGeneration = 0;
  const cancellations = new WeakMap(), unacknowledgedCancellations = new WeakSet();
  const finite = value => value !== '' && value !== null && value !== undefined && Number.isFinite(Number(value));
  const normalized = value => Math.max(0, Math.min(1, Number(value)));
  const printable = value => String(Number(value.toFixed(4)));
  const controlNumber = value => value?.stepCount ? String(value.value) : printable(value.value);
  const pagesOf = plugin => Array.isArray(plugin?.nks?.pages) ? plugin.nks.pages : [];
  const identity = plugin => JSON.stringify([plugin?.instanceID, plugin?.component, plugin?.format, plugin?.bundlePath, plugin?.nks?.path,
      plugin?.nks?.identity]);
  const stable = value => JSON.stringify(value,
      (_key, item) => item && typeof item === 'object' && !Array.isArray(item)
          ? Object.fromEntries(Object.keys(item).sort().map(key => [key, item[key]]))
          : item);
  const savedIdentity = plugin => stable([plugin?.instanceID, plugin?.component, plugin?.format, plugin?.bundlePath, plugin?.nks?.path,
      plugin?.nks?.identity]);
  const target = (track, plugin) => ({ track, kind: 'instrument', slot: 0, ...(plugin?.instanceID ? { instanceID: plugin.instanceID } : {}) });
  const addressOf = parameter => String(parameter.address ?? parameter.parameterID ?? (/^\d+$/.test(String(parameter.id)) ? parameter.id : ''));
  const valueOf = parameter => finite(parameter.normalized)
      ? normalized(parameter.normalized)
      : finite(parameter.value)
      ? normalized(Number(parameter.max) > Number(parameter.min)
          ? (Number(parameter.value) - Number(parameter.min)) / (Number(parameter.max) - Number(parameter.min))
          : parameter.value)
      : null;

  function entryFor(inst, plugin) {
    let entry = entries.get(plugin);
    if (!entry || entry.revoked || entry.project !== S().state.project) {
      const saved = savedViews.get(plugin); savedViews.delete(plugin);
      const view = saved?.project === S().state.project
          && saved.track === S().state.track
          && saved.trackObject === S().state.project.tracks[saved.track]
          && saved.inst === inst
          && saved.identity === savedIdentity(plugin)
          && saved.mapping === stable(pagesOf(plugin))
          ? saved
          : null;
      entry = {
        project: S().state.project,
        plugin,
        inst,
        track: S().state.track,
        trackObject: S().state.project.tracks[S().state.track],
        identity: identity(plugin),
        mapping: stable(pagesOf(plugin)),
        page: view?.page ?? 0,
        savedFocus: view?.focus || null,
        values: new Map(),
        drafts: new Map(),
        editing: new Map(),
        pending: new Map(),
        chain: Promise.resolve(),
        timer: null,
        loading: false,
        loaded: false,
        error: '',
        revision: 0,
        queuedRevision: 0,
        lastRead: 0
      };
      for (const parameter of plugin.parameters || []) remember(entry, parameter);
      entries.set(plugin, entry);
    }
    return entry;
  }
  function valid(entry) {
    const state = S().state;
    return !entry.revoked
        && state.project === entry.project
        && stable(pagesOf(entry.plugin)) === entry.mapping
        && entry.project.tracks[entry.track] === entry.trackObject
        && entry.project.instruments[entry.trackObject.instrument] === entry.inst
        && entry.project.audioPlugins?.includes(entry.plugin)
        && identity(entry.plugin) === entry.identity;
  }
  function mounted(binding) { return binding.root.isConnected && valid(binding.entry); }
  function adoptSavedView(previous, next) {
    if (S().state.project !== previous || previous === next || !Array.isArray(next?.audioPlugins)) return;
    const oldPlugins = previous.audioPlugins || [], candidates = new Set(oldPlugins.map(plugin => entries.get(plugin)).filter(Boolean));
    // A previous Save may have retained a page while this instrument was not
    // mounted. Carry that view forward through another acknowledged Save; do
    // not manufacture or revive its old parameter cache, drafts or async work.
    for (const plugin of oldPlugins) {
      const saved = savedViews.get(plugin);
      if (saved?.project === previous && saved.identity === savedIdentity(plugin) && saved.mapping === stable(pagesOf(plugin))) candidates.add({
        project: previous,
        plugin,
        inst: saved.inst,
        track: saved.track,
        trackObject: saved.trackObject,
        identity: identity(plugin),
        mapping: saved.mapping,
        page: saved.page
      });
    }
    // Capturing a native editor change may have replaced the descriptor before
    // Save. Its mounted view is eligible only after the same strict identity test.
    for (const binding of bindings) if (binding.entry.project === previous) candidates.add(binding.entry);
    for (const entry of candidates) {
      if (entry.project !== previous
          || identity(entry.plugin) !== entry.identity
          || stable(pagesOf(entry.plugin)) !== entry.mapping
          || entry.editing?.size
          || entry.pending?.size
          || previous.tracks[entry.track] !== entry.trackObject
          || previous.instruments[entry.trackObject.instrument] !== entry.inst) continue;
      const id = entry.plugin.instanceID;
      if (typeof id !== 'string' || !id || id.length > 128) continue;
      const oldMatches = oldPlugins.filter(plugin => plugin.instanceID === id),
        newMatches = next.audioPlugins.filter(plugin => plugin.instanceID === id);
      if (oldMatches.length !== 1 || newMatches.length !== 1) continue;
      const old = oldMatches[0], replacement = newMatches[0], track = next.tracks?.[entry.track], inst = next.instruments?.[track?.instrument];
      const sameSlot = plugin => plugin.kind === 'instrument'
          && plugin.track === entry.track
          && (plugin.slot ?? 0) === 0
          && plugin.owner === entry.plugin.owner;
      if (!sameSlot(entry.plugin) || !sameSlot(old) || !sameSlot(replacement) || !old.nks || !replacement.nks || !track || !inst) continue;
      if (track.routingID !== entry.trackObject.routingID
          || track.id !== entry.trackObject.id
          || track.instrument !== entry.trackObject.instrument
          || inst.id !== entry.inst.id
          || inst.type !== entry.inst.type) continue;
      if (savedIdentity(entry.plugin) !== savedIdentity(old)
          || savedIdentity(old) !== savedIdentity(replacement)
          || stable(pagesOf(entry.plugin)) !== stable(pagesOf(old))
          || stable(pagesOf(old)) !== stable(pagesOf(replacement))) continue;
      const active = document.activeElement, binding = [...bindings].find(binding => binding.entry === entry && binding.root.isConnected);
      const focus = S().state.view === 'instrument'
          && S().state.track === entry.track
          && S().state.instrument === entry.trackObject.instrument
          && binding
          && Array.from(binding.root.querySelectorAll('[data-nks-address]')).includes(active) ? {
        id: active.id,
        address: active.dataset.nksAddress,
        page: entry.page
      } : null;
      savedViews.set(replacement, {
        project: next,
        inst,
        track: entry.track,
        trackObject: track,
        identity: savedIdentity(replacement),
        mapping: stable(pagesOf(replacement)),
        page: entry.page,
        focus
      });
    }
  }
  function restoreSavedFocus(binding, initial = false) {
    const { entry, root } = binding, focus = entry.savedFocus, state = S().state;
    if (!focus) return;
    if (!mounted(binding)
        || state.view !== 'instrument'
        || state.track !== entry.track
        || state.instrument !== entry.trackObject.instrument
        || entry.page !== focus.page
        || state.modal) { entry.savedFocus = null; return; }
    const input = Array.from(root.querySelectorAll('[data-nks-address]')).find(input => input.id === focus.id
        && input.dataset.nksAddress === focus.address);
    if (!initial && document.activeElement !== focus.fallback) { entry.savedFocus = null; return; }
    if (input && !input.disabled) { input.focus({ preventScroll: true }); entry.savedFocus = null; return; }
    if (initial) { focus.fallback = root.querySelector('[data-nks-page]'); focus.fallback?.focus({ preventScroll: true }); }
    if (entry.loaded || entry.error || !focus.fallback) entry.savedFocus = null;
  }
  function remember(entry, parameter) {
    const id = addressOf(parameter), value = valueOf(parameter);
    if (!/^\d+$/.test(id) || value === null || entry.editing.has(id)) return;
    entry.values.set(id, {
      value,
      display: parameter.display || printable(value),
      writable: parameter.writable !== false,
      automatable: parameter.automatable !== false,
      stepCount: window.STFine?.parameterSteps(parameter) || undefined,
      normalizedStep: window.STFine?.parameterSteps(parameter) ? window.STFine.parameterStep(parameter) : undefined
    });
  }
  function slotsOf(page) {
    const slots = Array.from({ length: 8 }, (_, slot) => ({ slot, name: 'Unassigned', mapped: false, mappingStatus: 'unassigned' }));
    for (const parameter of page?.parameters || []) if (Number.isInteger(parameter.slot)
        && parameter.slot >= 0
        && parameter.slot < 8) slots[parameter.slot] = parameter;
    return slots;
  }
  function usable(slot) {
    return slot.mapped === true
        && Number.isInteger(Number(slot.parameterID))
        && Number(slot.parameterID) >= 0
        && Number(slot.parameterID) <= 0xffffffff
        && slot.parameterID !== null
        && slot.parameterID !== undefined;
  }
  function reason(slot) {
    return slot.mappingStatus === 'identityMismatch'
        ? 'Different plug-in'
        : slot.mappingStatus === 'parameterUnavailable' || slot.mapped === true
        ? 'Parameter unavailable'
        : 'Unassigned';
  }
  // The native NKS reader validates identity and exact ParamIDs. Names only enrich
  // the picker; they never invent an address or make an unavailable parameter writable.
  function automationMetadata(plugin) {
    const result = new Map();
    for (const [pageIndex, page] of pagesOf(plugin).entries()) for (const slot of slotsOf(page)) {
      if (!usable(slot)) continue;
      const id = String(Number(slot.parameterID)), name = slot.name || `Control ${slot.slot + 1}`;
      const pageName = page.name || `Page ${pageIndex + 1}`, context = [pageName, slot.section].filter(Boolean).join(' · ');
      const existing = result.get(id);
      if (existing) existing.search += ` ${name} ${context}`;
      else result.set(id, { name, group: `NKS · ${context}`, search: `${name} ${context}`, nks: true });
    }
    return result;
  }
  function automationAttributes(input, entry, value) {
    if (value?.writable && value.automatable !== false) {
      input.dataset.automate = `plugin:instrument:0:${input.dataset.nksAddress}`;
      input.dataset.track = String(entry.track); input.dataset.name = input.dataset.nksName;
    } else { delete input.dataset.automate; delete input.dataset.track; delete input.dataset.name; }
  }
  function control(entry, slot) {
    const id = String(slot.parameterID),
      known = usable(slot),
      value = known ? entry.values.get(id) : null,
      enabled = value && value.writable,
      name = slot.name || `Control ${slot.slot + 1}`;
    const base = `nks-${entry.track}-${slot.slot}`,
      attributes = known
          ? `data-nks-address="${id}" data-nks-name="${esc(name)}" ${!enabled || value?.automatable === false
              ? ''
              : `data-automate="plugin:instrument:0:${id}" data-track="${entry.track}" data-name="${esc(name)}"`}`
          : '';
    const display = !known ? reason(slot) : value ? value.display : entry.loaded ? 'Parameter unavailable' : entry.error ? 'Unavailable' : 'Loading…';
    return `<div class="nks-control ${known ? '' : 'nks-unmapped'}" data-nks-slot="${slot.slot}"><div class="nks-control-caption">`
        + `<span class="nks-slot-number" aria-hidden="true">${slot.slot + 1}</span><label for="${base}-number">${esc(name)}</label>`
        + `</div><span class="nks-section" title="${esc(slot.section || '')}">${esc(slot.section || '\u00a0')}</span>`
        + `<div class="nks-readout"><output id="${base}-display" data-nks-display="${known ? id : ''}">${esc(display)}</output>`
        + `<div class="nks-normalized"><input id="${base}-number" type="number" min="0" max="1" step="${window.STFine?.parameterStep(value) || .001}"`
        + ` data-parameter-steps="${window.STFine?.parameterSteps(value) || 0}" value="${esc(entry.drafts.get(`${base}-number`) ?? (value ? controlNumber(value) : ''))}"`
        + ` ${attributes} ${enabled ? '' : 'disabled'} aria-label="${esc(name)} normalized value" aria-describedby="${base}-display nks-${entry.track}-numeric-hint">`
        + `</div></div><input id="${base}-range" type="range" min="0" max="1" step="${window.STFine?.parameterStep(value) || .001}"`
        + ` data-parameter-steps="${window.STFine?.parameterSteps(value) || 0}" value="${value?.value ?? 0}" ${attributes} ${enabled ? '' : 'disabled'}`
        + ` aria-label="${esc(name)}" aria-valuetext="${esc(display)}" aria-describedby="${base}-display nks-${entry.track}-keyboard-hint">${known ? `<button type="button" class="nks-step-action" id="${base}-step" data-nks-step="${id}" data-nks-name="${esc(name)}" aria-label="Use ${esc(name)}`
        + ` on the selected tracker step" ${enabled && value?.automatable !== false && entry.plugin.instanceID && window.STStepFX?.show
            ? ''
            : 'disabled'}>Use `
        + `on step…</button>` : ''}</div>`;
  }
  // The NKS instrument console, followed by control pages for the track's other plug-in slots.
  function render(inst, plugin) {
    const pages = window.STPluginPages?.render(inst, plugin) || '';
    return renderNKS(inst, plugin) + pages;
  }
  function renderNKS(inst, plugin) {
    if (!inst || !plugin?.nks || !S().state.project?.audioPlugins?.includes(plugin)) return '';
    const entry = entryFor(inst, plugin), pages = pagesOf(plugin);
    if (!valid(entry)) return '';
    entry.page = Math.max(0, Math.min(entry.page, pages.length - 1));
    const page = pages[entry.page];
    const controlHelp = pages.some(page => slotsOf(page).some(usable))
        ? '<p class="small-note" data-nks-control-help><kbd>⌥A</kbd> adds an automation lane. Use on step… locks a value to the selected step.</p>'
        : '';
    return `<section class="panel wide nks-console" data-nks-root="${entry.track}" aria-label="NKS mapped controls">`
        + `<div class="panel-head nks-heading"><div><div class="eyebrow">NKS controls</div><h2>${esc(plugin.nks.name || plugin.name || inst.name)}</h2>`
        + `</div>${pages.length ? `<div class="nks-pages"><button id="nks-${entry.track}-previous" data-nks-page-step="-1" aria-label="Previous NKS page" ${entry.page === 0 ? 'disabled' : ''}>‹</button>`
        + `<label class="nks-page-label" for="nks-${entry.track}-page">Page<select id="nks-${entry.track}-page" data-nks-page aria-label="NKS controller `
        + `page">${pages.map((item, i) => `<option value="${i}" ${i === entry.page ? 'selected' : ''}>${i + 1}. ${esc(item.name || `Page ${i + 1}`)}</option>`).join('')}</select>`
        + `</label><span class="nks-page-count">${entry.page + 1} / ${pages.length}</span><button id="nks-${entry.track}-next" data-nks-page-step="1"`
        + ` aria-label="Next NKS page" ${entry.page === pages.length - 1 ? 'disabled' : ''}>›</button></div>` : ''}</div>`
        + `<div class="panel-body">${controlHelp}${pages.length ? `<div class="nks-control-grid">${slotsOf(page).map(slot => control(entry, slot)).join('')}</div><div class="nks-footer">`
        + `<span id="nks-${entry.track}-numeric-hint">Numeric entry: 0–1</span><span id="nks-${entry.track}-keyboard-hint">Knobs: <kbd>← →</kbd> move `
        + `· <kbd>↑ ↓</kbd> adjust 1% / one choice · <kbd>⇧</kbd> 10% · <kbd>⌥</kbd> fine · <kbd>1–8</kbd> knob · <kbd>⇧ ← →</kbd> or <kbd>Ctrl Page `
        + `Up / Down</kbd> page · <kbd>A</kbd> automate</span><span data-nks-status role="status" aria-live="polite">${esc(entry.error)}</span>`
        + `</div>` : '<p class="muted">This preset has no NKS controller pages.</p>'}</div></section>`;
  }
  function update(entry) {
    for (const binding of bindings) {
      if (binding.entry !== entry || !mounted(binding)) continue;
      const root = binding.root;
      for (const input of root.querySelectorAll('[data-nks-address]')) {
        const id = input.dataset.nksAddress, value = entry.values.get(id);
        automationAttributes(input, entry, value);
        if (!value) { input.disabled = true; continue; }
        input.disabled = !value.writable;
        input.step=String(window.STFine?.parameterStep(value)||.001);input.dataset.parameterSteps=String(window.STFine?.parameterSteps(value)||0);
        if (!(document.activeElement === input && input.type === 'number' && (entry.drafts.has(input.id) || entry.editing.has(id)))) input.value = controlNumber(value);
        input.setAttribute('aria-valuetext', value.display);
      }
      for (const button of root.querySelectorAll('[data-nks-step]')) {
        const value = entry.values.get(button.dataset.nksStep);
        button.disabled = !(value?.writable && value.automatable !== false && entry.plugin.instanceID && window.STStepFX?.show);
      }
      for (const output of root.querySelectorAll('[data-nks-display]')) {
        const value = entry.values.get(output.dataset.nksDisplay);
        if (value) output.textContent = value.display;
        else if (output.dataset.nksDisplay && entry.loaded) output.textContent = 'Parameter unavailable';
      }
      const status = root.querySelector('[data-nks-status]'); if (status) status.textContent = entry.error;
      restoreSavedFocus(binding);
    }
  }
  async function read(entry) {
    if (!valid(entry) || entry.loading || entry.pending.size || entry.editing.size) return;
    entry.loading = true;
    const generation = entry.readGeneration || 0;
    try {
      const response = await S().request('plugin.parameters', target(entry.track, entry.plugin));
      if (!valid(entry) || (entry.readGeneration || 0) !== generation) return;
      const list = Array.isArray(response) ? response : response?.parameters || [];
      for (const parameter of list) remember(entry, parameter);
      const available = new Set(list.map(addressOf));
      for (const id of entry.values.keys()) if (!available.has(id) && !entry.editing.has(id)) entry.values.delete(id);
      entry.loaded = true; entry.error = ''; entry.lastRead = Date.now(); update(entry);
    } catch (error) { if (valid(entry)
        && (entry.readGeneration || 0) === generation) { entry.error = error.message || 'Could not read plug-in controls.'; update(entry); } }
    finally { if ((entry.readGeneration || 0) === generation) entry.loading = false; }
  }
  function serial(entry, work) {
    const result = entry.chain.catch(() => {}).then(async () => {
      await S().waitForGraphEdit?.();
      await S().waitForEngineUpdate?.();
      if (valid(entry)) return work();
    });
    entry.chain = result;
    result.catch(error => { if (valid(entry)) { entry.error = error.message || String(error); update(entry); S().toast(entry.error, 'error'); } });
    return result;
  }
  function replayEdited(entry) {
    if (!valid(entry)) return;
    for (const [address, revision] of entry.editing) {
      const value = entry.values.get(address)?.value;
      if (finite(value)) entry.pending.set(address, { value, revision });
    }
  }
  function flush(entry) {
    clearTimeout(entry.timer); entry.timer = null;
    const pending = [...entry.pending]; entry.pending.clear();
    return serial(entry, async () => {
      for (const [address, item] of pending) {
        if (!valid(entry)) return;
        let response;
        try { response = await S().request('plugin.parameter',
            { ...target(entry.track, entry.plugin), address, value: item.value, normalized: true }); }
        catch (error) {
          // Preserve the final requested writes for an explicit retry. A failed write
          // must not be replaced by a successful snapshot of the preceding value.
          replayEdited(entry);
          throw error;
        }
        if (!valid(entry)) return;
        if (entry.editing.get(address) === item.revision) {
          const cached = entry.values.get(address);
          if (cached) { cached.value = valueOf(response || {}) ?? item.value; cached.display = response?.display || printable(item.value); }
        }
      }
      update(entry);
    });
  }
  function preview(entry, input) {
    if (!valid(entry) || input.disabled || !finite(input.value)) return false;
    const id = input.dataset.nksAddress,
      previous = entry.values.get(id),
      value = window.STFine?.snapParameter(input.value, previous) ?? normalized(input.value);
    if (!previous?.writable) return false;
    const revision = ++entry.revision; editGeneration++;
    S().state.dirty = true; S().markDirty?.();
    entry.values.set(id, { ...previous, value, display: printable(value) });
    entry.editing.set(id, revision); entry.pending.set(id, { value, revision }); editingEntries.add(entry);
    clearTimeout(entry.timer); entry.timer = setTimeout(() => flush(entry), 35); update(entry); return true;
  }
  function commitEdits(entry, name = 'NKS controls') {
    if (!valid(entry) || !entry.editing.size || entry.revision <= entry.queuedRevision) return entry.chain;
    const revision = entry.revision;
    entry.queuedRevision = revision;
    const applied = flush(entry);
    const releaseEdits = () => {
      for (const [id, edited] of entry.editing) if (edited <= revision) entry.editing.delete(id);
      for (const [id, item] of entry.pending) if (item.revision <= revision) entry.pending.delete(id);
    };
    const capture = serial(entry, async () => {
      // A failed final write must never create an undo snapshot claiming success.
      await applied;
      if (!valid(entry)) return;
      const snapshot = await S().request('plugin.state', target(entry.track, entry.plugin));
      if (!valid(entry)) return;
      if (snapshot?.stateError) throw new Error(String(snapshot.stateError));
      if (!snapshot
          || typeof snapshot !== 'object'
          || Array.isArray(snapshot)
          || typeof snapshot.state !== 'string'
          || !snapshot.state) throw new Error('The plug-in did not return a state snapshot.');
      if (entry.plugin.instanceID && snapshot.instanceID !== entry.plugin.instanceID
          || snapshot.component && snapshot.component !== entry.plugin.component
          || snapshot.track !== undefined && Number(snapshot.track) !== entry.track
          || snapshot.kind && snapshot.kind !== 'instrument'
          || snapshot.slot !== undefined && Number(snapshot.slot) !== 0) throw new Error('The plug-in changed before its controls could be saved.');
      // A later edit already superseded this snapshot. Keep the original Undo
      // baseline and let the latest queued capture commit the complete gesture.
      // Publishing an older snapshot here would schedule engine.project while
      // the later value is being applied or captured.
      if (entry.revision !== revision) return;
      const old = entry.plugin; let committed = false;
      S().mutate(`Adjust ${name}`, project => {
        if (!valid(entry)) return;
        const replacement = { ...old, ...snapshot, nks: old.nks };
        project.audioPlugins[project.audioPlugins.indexOf(old)] = replacement;
        entry.inst.plugin = { ...entry.inst.plugin, ...replacement };
        entry.plugin = replacement; entries.set(replacement, entry); committed = true;
      }, { render: false, nativeCapture: true });
      if (!committed) throw new Error('The control edit is still pending. Finish the current operation before saving.');
      releaseEdits(); if (!entry.editing.size) editingEntries.delete(entry); entry.error = ''; update(entry);
    });
    capture.catch(() => {
      replayEdited(entry);
      if (entry.queuedRevision === revision) entry.queuedRevision = 0;
    });
    return capture;
  }
  function cancelEditingEntry(entry) {
    if (!entry.editing.size) { editingEntries.delete(entry); return; }
    clearTimeout(entry.timer); entry.pending.clear(); editingEntries.delete(entry);
    if (entry.project !== S().state.project) return;
    const message = 'A mapped control edit was canceled because its plug-in changed. Review the replacement before saving again.';
    cancellations.set(entry.project, (cancellations.get(entry.project) || 0) + 1);
    unacknowledgedCancellations.add(entry.project); editGeneration++;
    S().state.dirty = true; S().markDirty?.(); S().toast(message, 'error');
  }
  function suspendControls(entry, message, revoke = false) {
    for (const binding of bindings) {
      if (binding.entry !== entry || !binding.root.isConnected) continue;
      for (const input of binding.root.querySelectorAll('[data-nks-address]')) {
        input.disabled = true; automationAttributes(input, entry, null);
      }
      for (const button of binding.root.querySelectorAll('[data-nks-step]')) button.disabled = true;
      if (revoke) {
        clearTimeout(binding.poll);
        for (const control of [...binding.root.querySelectorAll('[data-nks-page]'),
            ...binding.root.querySelectorAll('[data-nks-page-step]')]) control.disabled = true;
      }
      const status = binding.root.querySelector('[data-nks-status]'); if (status) status.textContent = message;
    }
  }
  function adoptPlugins(next) {
    if (!Array.isArray(next)) return next;
    const project = S().state.project, candidates = new Set(editingEntries);
    // A native snapshot also replaces settled descriptors. Their mounted
    // handlers must follow the same exact instance, or become visibly inert.
    for (const binding of bindings) if (binding.root.isConnected) candidates.add(binding.entry);
    const revoke = entry => {
      cancelEditingEntry(entry); entry.revoked = true; entry.readGeneration = (entry.readGeneration || 0) + 1;
      clearTimeout(entry.timer); entry.pending.clear(); entry.editing.clear(); entry.drafts.clear();
      suspendControls(entry, 'Plug-in controls changed. Reopen the instrument view to refresh them.', true);
    };
    for (const entry of candidates) {
      if (entry.project !== project) continue;
      if (!valid(entry)) { revoke(entry); continue; }
      const id = entry.plugin.instanceID;
      const matches = typeof id === 'string' && id ? next.filter(plugin => plugin.instanceID === id) : [];
      const replacement = matches.length === 1 ? matches[0] : null;
      const uniqueOld = id && project.audioPlugins.filter(plugin => plugin.instanceID === id).length === 1;
      const sameRoute = replacement
          && replacement.owner === entry.plugin.owner
          && replacement.track === entry.plugin.track
          && replacement.kind === entry.plugin.kind
          && (replacement.slot ?? 0) === (entry.plugin.slot ?? 0);
      if (!uniqueOld || !sameRoute || savedIdentity(replacement) !== savedIdentity(entry.plugin) || stable(pagesOf(replacement)) !== entry.mapping) {
        revoke(entry); continue;
      }
      const refreshValues = !entry.editing.size && entry.plugin.state !== replacement.state;
      entry.plugin = replacement; entry.identity = identity(replacement); entries.set(replacement, entry);
      entry.inst.plugin = { ...entry.inst.plugin, ...replacement };
      if (refreshValues) {
        // Discard reads/drafts from the preceding opaque state. Leave the
        // existing DOM clearly loading until the exact instance is read again.
        entry.readGeneration = (entry.readGeneration || 0) + 1; entry.loading = false;
        entry.values.clear(); entry.drafts.clear(); entry.loaded = false; entry.error = '';
        suspendControls(entry, 'Refreshing plug-in controls…');
        Promise.resolve().then(() => { if (valid(entry)) { update(entry); void read(entry); } });
      }
    }
    return next;
  }
  async function flushEdits() {
    if (await flushNKSEdits() === false) return false;
    return await window.STPluginPages?.flushEdits?.() !== false;
  }
  async function flushNKSEdits() {
    const owner = S().state.project, cancellation = cancellations.get(owner) || 0;
    if (unacknowledgedCancellations.has(owner)) { unacknowledgedCancellations.delete(owner); return false; }
    try {
      // Include controls no longer mounted: navigation can happen before the
      // coalescing timer or native state capture finishes.
      while (true) {
        for (const entry of [...editingEntries]) if (!valid(entry)) cancelEditingEntry(entry);
        if ((cancellations.get(owner) || 0) !== cancellation) { unacknowledgedCancellations.delete(owner); return false; }
        const pending = [...editingEntries].filter(entry => entry.project === owner);
        if (!pending.length) return S().state.project === owner;
        await Promise.all(pending.map(entry => commitEdits(entry)));
        if (S().state.project !== owner) return false;
        if ((cancellations.get(owner) || 0) !== cancellation) { unacknowledgedCancellations.delete(owner); return false; }
      }
    } catch { return false; }
  }
  async function automateEntryControl(entry, input) {
    const state = S().state, owner = state.project, patternID = owner.activePattern, row = state.row;
    const action = entry.actionGeneration = (entry.actionGeneration || 0) + 1;
    const parameter = input.dataset.automate, name = input.dataset.nksName, value = normalized(input.value);
    if (!parameter || input.disabled || !finite(input.value) || !valid(entry)) return;
    if (!await flushEdits()) return;
    if (entry.actionGeneration !== action
        || !valid(entry)
        || state.project !== owner
        || state.view !== 'instrument'
        || state.track !== entry.track
        || owner.activePattern !== patternID
        || state.row !== row) return;
    S().automate(entry.track, parameter, name, value);
    if (state.view === 'automation') document.getElementById?.('automation-envelope')?.focus();
  }
  async function useOnStep(entry, button) {
    const state = S().state, owner = state.project, patternID = owner.activePattern, row = state.row;
    const action = entry.actionGeneration = (entry.actionGeneration || 0) + 1;
    const address = button.dataset.nksStep, name = button.dataset.nksName;
    if (button.disabled || !entry.plugin.instanceID || !window.STStepFX?.show || !valid(entry)) return;
    if (!await flushEdits()) return;
    if (entry.actionGeneration !== action
        || !valid(entry)
        || state.project !== owner
        || state.view !== 'instrument'
        || state.track !== entry.track
        || owner.activePattern !== patternID
        || state.row !== row) return;
    await window.STStepFX.show({ plugin: { instanceID: entry.plugin.instanceID, address, name } });
  }
  async function automateControl(input) {
    const binding = [...bindings].find(binding => mounted(binding)
        && Array.from(binding.root.querySelectorAll('[data-nks-address]')).includes(input));
    if (binding) return automateEntryControl(binding.entry, input);
  }
  function commit(entry, input) {
    if (!preview(entry, input)) { const value = entry.values.get(input.dataset.nksAddress); if (value) input.value = controlNumber(value); commitEdits(entry,
        input.dataset.nksName); return; }
    commitEdits(entry, input.dataset.nksName);
  }
  function pageTo(entry, page) {
    if (!valid(entry) || !Number.isInteger(page)) return;
    const next = Math.max(0, Math.min(pagesOf(entry.plugin).length - 1, page));
    if (next === entry.page) return;
    const focusID = document.activeElement?.id;
    commitEdits(entry); entry.drafts.clear(); entry.savedFocus = null; entry.page = next; S().refresh();
    if (focusID) {
      const previousControl = document.getElementById?.(focusID);
      const nextControl = previousControl && !previousControl.disabled ? previousControl : document.getElementById?.(`nks-${entry.track}-page`);
      nextControl?.focus({ preventScroll: true });
    }
  }
  // Keyboard adjustment previews each step and commits one Undo step when the key is released.
  let adjusting = null, adjustTimer = null;
  function adjust(entry, input, delta, modifiers = {}) {
    input.value = String(window.STFine?.stepParameter(input.value, Math.sign(delta), modifiers, entry.values.get(input.dataset.nksAddress))
        ?? normalized(Number(input.value) + delta));
    if (!preview(entry, input)) return;
    adjusting = { entry, input }; clearTimeout(adjustTimer); adjustTimer = setTimeout(finishAdjust, 700);
  }
  function finishAdjust() {
    clearTimeout(adjustTimer); adjustTimer = null;
    const value = adjusting; adjusting = null;
    if (value && valid(value.entry)) commitEdits(value.entry, value.input.dataset.nksName);
  }
  function bind(scope = document) {
    window.STPluginPages?.bind(scope);
    for (const binding of bindings) if (!mounted(binding)) { clearTimeout(binding.poll); commitEdits(binding.entry); bindings.delete(binding); }
    for (const root of scope.querySelectorAll('[data-nks-root]')) {
      if (root.dataset.nksBound === 'true') continue;
      const state = S().state,
        track = Number(root.dataset.nksRoot),
        plugin = state.project.audioPlugins?.find(item => item.track === track && item.kind === 'instrument' && (item.slot ?? 0) === 0),
        inst = state.project.instruments[state.project.tracks[track]?.instrument];
      if (!plugin?.nks || !inst) continue;
      const entry = entryFor(inst, plugin), binding = { root, entry, poll: null }; root.dataset.nksBound = 'true'; bindings.add(binding);
      for (const input of root.querySelectorAll('[data-nks-address]')) {
        if (input.type === 'number') {
          input.addEventListener('blur', () => { if (mounted(binding)) { entry.drafts.delete(input.id); update(entry); } });
        }
        input.addEventListener('input', () => {
          if (mounted(binding)) {
            if (input.type === 'number') entry.drafts.set(input.id, input.value);
            preview(entry, input);
          }
        });
        input.addEventListener('change', () => { if (mounted(binding)) { entry.drafts.delete(input.id); commit(entry, input); } });
        if (input.type === 'range') window.STFine?.bindDrag(input);  // ⌥-drag 1/10 speed, ⇧ 1/4, ⌥⇧ 1/50
      }
      for (const button of root.querySelectorAll('[data-nks-step]')) button.addEventListener('click', () => {
        if (mounted(binding)) void useOnStep(entry, button);
      });
      root.querySelector('[data-nks-page]')?.addEventListener('change', event => pageTo(entry, Number(event.target.value)));
      for (const button of root.querySelectorAll('[data-nks-page-step]')) button.addEventListener('click',
          () => pageTo(entry, entry.page + Number(button.dataset.nksPageStep)));
      root.addEventListener('keydown', event => {
        if (!mounted(binding) || event.isComposing) return;
        if (event.altKey
            && !event.metaKey
            && !event.ctrlKey
            && (event.key?.toLowerCase() === 'a' || event.code === 'KeyA')
            && event.target?.dataset?.nksAddress !== undefined) {
          event.preventDefault(); event.stopPropagation();
          if (!event.repeat) void automateEntryControl(entry, event.target); return;
        }
        if (event.ctrlKey
            && !event.metaKey
            && !event.altKey
            && (event.key === 'PageUp' || event.key === 'PageDown')) { event.preventDefault(); event.stopPropagation(); pageTo(entry,
                entry.page + (event.key === 'PageUp' ? -1 : 1)); return; }
        const knob = event.target?.type === 'range' && event.target.dataset?.nksAddress !== undefined && !event.target.disabled;
        // Shift ← → pages; ↑ ↓ adjust by 1% (Shift 10%, Option 0.1%); Page Up / Down by 10%.
        if (knob
            && event.shiftKey
            && !event.metaKey
            && !event.ctrlKey
            && !event.altKey
            && ['ArrowLeft', 'ArrowRight'].includes(event.key)) { event.preventDefault(); event.stopPropagation(); finishAdjust(); pageTo(entry,
                entry.page + (event.key === 'ArrowLeft' ? -1 : 1)); return; }
        if (knob
            && !event.metaKey
            && !event.ctrlKey
            && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)
            && !(event.altKey && event.shiftKey)) {
          event.preventDefault(); event.stopPropagation();
          const step = event.key.startsWith('Page') || event.shiftKey ? 0.1 : event.altKey ? 0.001 : 0.01;
          adjust(entry, event.target, (event.key === 'ArrowUp' || event.key === 'PageUp' ? 1 : -1) * step,
              {coarse:event.shiftKey||event.key.startsWith('Page'),fine:event.altKey}); return;
        }
        if (knob && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
          if (/^[1-8]$/.test(event.key) && !S().state.keyboardPlay) {
            const target = root.querySelector(`[data-nks-slot="${Number(event.key) - 1}"] input[type=range]`);
            if (target && !target.disabled) { event.preventDefault(); event.stopPropagation(); finishAdjust(); target.focus(); }
            return;
          }
          if (event.key?.toLowerCase() === 'a') { event.preventDefault(); event.stopPropagation(); if (!event.repeat) { finishAdjust(); void automateEntryControl(entry,
              event.target); } return; }
        }
        if (event.target?.type !== 'range'
            || event.metaKey
            || event.ctrlKey
            || event.altKey
            || event.shiftKey
            || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
        const knobs = Array.from(root.querySelectorAll('[data-nks-address]')).filter(input => input.type === 'range'
            && !input.disabled
            && entry.values.get(input.dataset.nksAddress)?.writable);
        const index = knobs.indexOf(event.target);
        if (index < 0) return;
        // Navigation never emits input/change or edits the value; Up/Down retain native range adjustment.
        event.preventDefault(); event.stopPropagation(); finishAdjust();
        knobs[Math.max(0, Math.min(knobs.length - 1, index + (event.key === 'ArrowRight' ? 1 : -1)))].focus();
      });
      root.addEventListener('keyup', event => {
        if (adjusting && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)) finishAdjust();
      });
      restoreSavedFocus(binding, true);
      const poll = () => {
        if (!mounted(binding)) {
          commitEdits(entry);
          bindings.delete(binding);
          return;
        }
        read(entry);
        binding.poll = setTimeout(poll, 750);
      };
      poll();
    }
  }
  function event(type, payload = {}) {
    window.STPluginPages?.event(type, payload);
    if (type === 'project.loaded') {
      savedViews = new WeakMap();
      for (const binding of bindings) { clearTimeout(binding.poll); clearTimeout(binding.entry.timer); binding.entry.pending.clear(); }
      bindings.clear(); return;
    }
    if (!['status', 'plugin.parameter', 'plugin.changed'].includes(type)) return;
    for (const binding of bindings) {
      if (!mounted(binding) || (payload.track !== undefined && Number(payload.track) !== binding.entry.track)) continue;
      if (Date.now() - binding.entry.lastRead > 350) read(binding.entry);
    }
  }
  // Hardware controllers (mk3.js): the selected track's NKS instrument pages, edited through the same
  // preview / one-Undo-per-gesture path as the on-screen knobs.
  function controllerEntry() {
    const state = S().state, track = state.project?.tracks?.[state.track];
    const plugin = state.project?.audioPlugins?.find(item => item.track === state.track && item.kind === 'instrument' && (item.slot ?? 0) === 0);
    const inst = state.project?.instruments?.[track?.instrument];
    if (!plugin?.nks || !pagesOf(plugin).length || !inst) return null;
    const entry = entryFor(inst, plugin);
    return valid(entry) ? entry : null;
  }
  let controllerAdjust = null, controllerTimer = null;
  function controllerFinish() {
    clearTimeout(controllerTimer);
    controllerTimer = null;
    const value = controllerAdjust;
    controllerAdjust = null;
    if (value && valid(value.entry)) commitEdits(value.entry, value.name);
  }
  const controller = {
    view() {
      const entry = controllerEntry(); if (!entry) return null;
      if ((!entry.loaded || Date.now() - entry.lastRead > 1500) && !entry.loading) read(entry);
      const pages = pagesOf(entry.plugin); entry.page = Math.max(0, Math.min(entry.page, pages.length - 1));
      return { name: entry.plugin.nks.name
          || entry.plugin.name
          || entry.inst.name, pages: pages.map((page, index) => page.name || `Page ${index + 1}`), page: entry.page, loaded: entry.loaded, error: entry.error, instanceID: entry.plugin.instanceID, track: entry.track,
        knobs: slotsOf(pages[entry.page]).map(slot => { const known = usable(slot), id = known
            ? String(Number(slot.parameterID))
            : null, value = id ? entry.values.get(id) : null; return { name: slot.name || `Control ${slot.slot + 1}`, section: slot.section || '', address: id, value: value ? value.value : null, display: !known
            ? reason(slot)
            : value
            ? value.display
            : entry.loaded
            ? 'Unavailable'
            : 'Loading…', writable: !!value?.writable, automatable: !!value && value.automatable !== false, stepCount:value?.stepCount, normalizedStep:value?.normalizedStep }; }) };
    },
    setPage(index) {
      const entry = controllerEntry();
      if (!entry) return false;
      controllerFinish();
      const pages = pagesOf(entry.plugin);
      const next = Math.max(0, Math.min(pages.length - 1, index));
      if (next !== entry.page) {
        commitEdits(entry);
        entry.drafts.clear();
        entry.page = next;
        if (S().state.view === 'instrument') S().refresh();
      }
      return true;
    },
    adjust(knob, delta) {
      const entry = controllerEntry(); if (!entry) return null;
      const slot = slotsOf(pagesOf(entry.plugin)[entry.page])[knob]; if (!slot || !usable(slot)) return null;
      const id = String(Number(slot.parameterID)), value = entry.values.get(id); if (!value?.writable) return null;
      const input = {
        value: String(window.STFine?.snapParameter(value.value + delta, value) ?? normalized(value.value + delta)),
        disabled: false,
        dataset: { nksAddress: id, nksName: slot.name || `Control ${slot.slot + 1}` }
      };
      if (!preview(entry, input)) return null;
      controllerAdjust = { entry, name: input.dataset.nksName }; clearTimeout(controllerTimer); controllerTimer = setTimeout(controllerFinish, 700);
      return Number(input.value);
    },
    finish: controllerFinish
  };
  window.STNKSControls = { render, bind, event, flushEdits, automationMetadata, automateControl, adoptPlugins, adoptSavedView, controller,
    get editGeneration() { return editGeneration + (window.STPluginPages?.editGeneration || 0); },
    get hasPendingEdits() { return [...editingEntries].some(entry => valid(entry) && entry.editing.size)
        || !!window.STPluginPages?.hasPendingEdits; } };
  window.STPendingEdits.register('nksControls', {
    order: 30,
    hasPending: () => !!window.STNKSControls?.hasPendingEdits,
    generation: () => window.STNKSControls?.editGeneration ?? 0,
    flush: () => window.STNKSControls?.flushEdits?.()
  });
})();
