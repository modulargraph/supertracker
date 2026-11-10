/* Maschine-style eight-knob control pages for every plug-in slot on the selected track.
   NKS effects use their verified NKS pages. Other AU / VST3 / CLAP plug-ins get pages generated
   from their parameter list, plus user pages (pinned / assigned parameters) saved in the project
   under project.pluginPages[instanceID]. The NKS instrument console (nks-controls.js) is unchanged. */
(() => {
  'use strict';
  const S = () => window.ST;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const finite = value => value !== '' && value !== null && value !== undefined && Number.isFinite(Number(value));
  const clamp01 = value => Math.max(0, Math.min(1, Number(value)));
  const printable = value => String(Number(Number(value).toFixed(3)));
  // Knob position as a percentage, unless the plug-in's own display already is one.
  // One decimal so ⌥ 0.1% steps stay visible.
  const percent = value => !value || /%\s*$/.test(String(value.display)) ? '' : `${Number((value.value * 100).toFixed(1))}%`;
  const entries = new Map();
  const FX = () => window.STEffectPages;
  /** Built-in effects show values in their own units (effect-pages.js); others use the plug-in's text. */
  const shown = (entry, address, value, fallback) => (FX()?.isBuiltin(entry.component) && FX().display(entry.component, address, value, (id, other) => entry.values.get(other)?.value)) || fallback;
  /** Values shown in combination with another parameter (Beat Delay's Time × Unit) follow that parameter's changes. */
  function relink(entry) {
    if (!FX()?.isBuiltin(entry.component)) return;
    for (const address of FX().linkedAddresses?.(entry.component) || []) {
      const cached = entry.values.get(address);
      if (cached) cached.display = shown(entry, address, cached.value, cached.display);
    }
  }
  const updateListeners = new Set();
  // A parameter read that never answers must not leave the pages on "Reading parameters…".
  const READ_TIMEOUT = 8000, RETRY_AFTER = 3000;
  const local = { slotKey: new Map(), page: new Map(), poll: null, editGeneration: 0, commitTimer: null, adjusting: null };
  let mountedRoot = null;

  // ---- Model ----------------------------------------------------------------------------------
  const slotKey = plugin => `${plugin.kind}:${plugin.slot ?? 0}`;
  const kindLabel = plugin => plugin.kind === 'instrument' ? 'Instrument' : `FX ${(plugin.slot ?? 0) + 1}`;
  const pluginName = plugin => plugin?.nks?.name || plugin?.name || plugin?.component || 'Plug-in';
  const targetOf = plugin => ({ ...(plugin.owner ? {owner:plugin.owner} : {track:plugin.track}), kind: plugin.kind, slot: plugin.slot ?? 0, ...(plugin.instanceID ? { instanceID: plugin.instanceID } : {}) });
  const addressOf = parameter => String(parameter?.address ?? parameter?.parameterID ?? '');
  const valueOf = parameter => finite(parameter.normalized) ? clamp01(parameter.normalized) : finite(parameter.value) && Number(parameter.max) > Number(parameter.min) ? clamp01((Number(parameter.value) - Number(parameter.min)) / (Number(parameter.max) - Number(parameter.min))) : null;
  /** Slots this console controls: the instrument (when it has no NKS console) and the track's inserts. */
  const pluginOwner = plugin => plugin.owner || (Number.isInteger(plugin.track) ? window.STRoutingProject?.trackOwner(S().state.project?.tracks?.[plugin.track]) : null);
  function slotsForOwner(owner) { return (S().state.project?.audioPlugins||[]).filter(plugin=>plugin.instanceID && pluginOwner(plugin)===owner).sort((a,b)=>(a.kind==='instrument'?-1:a.slot??0)-(b.kind==='instrument'?-1:b.slot??0)); }
  function slotsFor(inst, instrumentPlugin) {
    const state = S().state, track = Number(state.track || 0);
    const plugins = (state.project?.audioPlugins || []).filter(plugin => plugin.track === track && plugin.instanceID);
    const effects = plugins.filter(plugin => plugin.kind === 'effect').sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
    const instrument = instrumentPlugin && !instrumentPlugin.nks?.pages?.length && instrumentPlugin.instanceID ? [instrumentPlugin] : [];
    return [...instrument, ...effects];
  }
  function entryFor(plugin) {
    const project = S().state.project;
    let entry = entries.get(plugin.instanceID);
    if (!entry || entry.project !== project) {
      entry = { project, id: plugin.instanceID, component: plugin.component || '', values: new Map(), params: [], loaded: false, loading: false, pending: new Map(), editedAt: new Map(), timer: null,
        chain: Promise.resolve(), revision: 0, committed: 0, queued: 0, error: '', lastRead: 0, lastAttempt: 0 };
      remember(entry, plugin.parameters || [], true);
      entries.set(plugin.instanceID, entry);
    }
    return entry;
  }
  const descriptor = entry => S().state.project === entry.project ? entry.project.audioPlugins?.find(plugin => plugin.instanceID === entry.id) || null : null;
  function remember(entry, list, full = false) {
    if (!Array.isArray(list)) return;
    if (full && list.length) entry.params = list.filter(parameter => /^\d+$/.test(addressOf(parameter))).map(parameter => ({ address: addressOf(parameter), id: parameter.id == null ? '' : String(parameter.id), name: String(parameter.name || `Parameter ${addressOf(parameter)}`), unit: parameter.unitName || '', writable: parameter.writable !== false, automatable: parameter.automatable !== false }));
    const now = Date.now();
    for (const parameter of list) {
      const address = addressOf(parameter), value = valueOf(parameter);
      if (!/^\d+$/.test(address) || value === null) continue;
      // A local edit wins until the host has seen it.
      if (entry.pending.has(address) || now - (entry.editedAt.get(address) || 0) < 600) continue;
      entry.values.set(address, { value, display: shown(entry, address, value, parameter.display || printable(value)), writable: parameter.writable !== false, automatable: parameter.automatable !== false, name: parameter.name, stepCount:window.STFine?.parameterSteps(parameter)||undefined, normalizedStep:window.STFine?.parameterSteps(parameter)?window.STFine.parameterStep(parameter):undefined });
     }
    relink(entry);
  }
  function savedPages(plugin) {
    const saved = S().state.project?.pluginPages?.[plugin.instanceID]?.pages;
    return Array.isArray(saved) ? saved.filter(page => page && Array.isArray(page.addresses)) : [];
  }
  /** Curated pages of a built-in effect. Without native metadata yet, addresses come from the read parameter ids. */
  function builtinPages(plugin, entry) {
    if (!FX()?.isBuiltin(plugin.component)) return null;
    if (!FX().ready) FX().load(S().request?.bind(S()));
    const pages = FX().pages(plugin.component);
    if (!pages) return null;
    const byID = new Map(entry.params.filter(parameter => parameter.id).map(parameter => [parameter.id, parameter.address]));
    for (const page of pages) for (const slot of page.slots) if (slot && !slot.address) slot.address = byID.get(slot.id) || null;
    return pages;
  }
  /** Pages: NKS pages; a built-in effect's curated pages; or the user's pages followed by automatically generated ones. */
  function pagesFor(plugin, entry) {
    if (plugin.nks?.pages?.length) return plugin.nks.pages.map((page, index) => {
      const slots = Array.from({ length: 8 }, () => null);
      for (const parameter of page.parameters || []) {
        if (!Number.isInteger(parameter.slot) || parameter.slot < 0 || parameter.slot > 7) continue;
        const usable = parameter.mapped === true && Number.isInteger(Number(parameter.parameterID)) && parameter.parameterID !== null;
        slots[parameter.slot] = { address: usable ? String(Number(parameter.parameterID)) : null, name: parameter.name || `Control ${parameter.slot + 1}`, section: parameter.section || '' };
      }
      return { name: page.name || `Page ${index + 1}`, kind: 'nks', slots };
    });
    const names = new Map(entry.params.map(parameter => [parameter.address, parameter]));
    const user = savedPages(plugin).map((page, index) => ({ name: String(page.name || `My page ${index + 1}`), kind: 'user', index,
      slots: Array.from({ length: 8 }, (_, slot) => { const address = page.addresses[slot] == null ? null : String(page.addresses[slot]); return address ? { address, name: names.get(address)?.name || `Parameter ${address}`, section: names.get(address)?.unit || '' } : null; }) }));
    const curated = builtinPages(plugin, entry);
    if (curated) return [...curated, ...user];
    const writable = entry.params.filter(parameter => parameter.writable);
    const auto = [];
    for (let start = 0; start < writable.length; start += 8) {
      const chunk = writable.slice(start, start + 8);
      auto.push({ name: chunk.length > 1 ? `${chunk[0].name} – ${chunk.at(-1).name}` : chunk[0].name, kind: 'auto',
        slots: Array.from({ length: 8 }, (_, slot) => chunk[slot] ? { address: chunk[slot].address, name: chunk[slot].name, section: chunk[slot].unit } : null) });
    }
    return [...user, ...auto];
  }
  function selection(inst, instrumentPlugin) {
    const slots = slotsFor(inst, instrumentPlugin), track = Number(S().state.track || 0);
    if (!slots.length) return null;
    const wanted = local.slotKey.get(track), plugin = slots.find(item => slotKey(item) === wanted) || slots[0];
    const entry = entryFor(plugin), pages = pagesFor(plugin, entry);
    const page = Math.max(0, Math.min(pages.length - 1, local.page.get(plugin.instanceID) ?? 0));
    return { slots, plugin, entry, pages, page, track };
  }

  function selectionForOwner(owner, instanceID) {
    const slots=slotsForOwner(owner); if(!slots.length) return null;
    const wanted=local.slotKey.get(owner), plugin=slots.find(item=>item.instanceID===instanceID)||slots.find(item=>slotKey(item)===wanted)||slots[0];
    const entry=entryFor(plugin),pages=pagesFor(plugin,entry),page=Math.max(0,Math.min(pages.length-1,local.page.get(plugin.instanceID)??0));
    return {slots,plugin,entry,pages,page,track:owner,owner};
  }

  // ---- Rendering ------------------------------------------------------------------------------
  function knob(view, slot, index) {
    const { entry, plugin, track } = view, base = `pp-${track}-${index}`;
    // A built-in effect's curated page leaves its unused knobs blank, as the mixer's insert chain does.
    if (!slot && view.pages?.[view.page]?.kind === 'builtin') return `<div class="nks-control nks-unmapped pp-blank" data-pp-slot="${index}" aria-hidden="true"><div class="nks-control-caption"><span class="nks-slot-number">${index + 1}</span><span class="pp-blank-name">–</span></div><span class="nks-section"> </span><div class="nks-readout"><output> </output></div><span class="pp-empty-line"></span></div>`;
    if (!slot?.address) {
      const editable = !plugin.nks?.pages?.length && view.pages?.[view.page]?.kind !== 'builtin';
      return `<div class="nks-control nks-unmapped" data-pp-slot="${index}"><div class="nks-control-caption"><span class="nks-slot-number" aria-hidden="true">${index + 1}</span><label for="${base}">${esc(slot?.name || 'Empty')}</label></div><span class="nks-section">${esc(slot?.section || ' ')}</span><div class="nks-readout"><output>${slot ? 'Parameter unavailable' : 'Unassigned'}</output></div>${editable ? `<button type="button" class="pp-assign" id="${base}" data-pp-knob="${index}" data-pp-empty="true" aria-label="Assign a parameter to knob ${index + 1}">Assign… <kbd>↵</kbd></button>` : `<span class="pp-empty-line" id="${base}" aria-hidden="true"></span>`}</div>`;
    }
    const value = entry.values.get(slot.address), name = slot.name, writable = !!value && value.writable !== false;
    const automate = writable && value.automatable !== false ? `data-automate="plugin:${plugin.kind}:${plugin.slot ?? 0}:${slot.address}" ${plugin.owner ? `data-owner="${esc(plugin.owner)}"` : `data-track="${plugin.track}"`} data-name="${esc(`${pluginName(plugin)} · ${name}`)}"` : '';
    const display = value ? value.display : entry.loaded ? 'Parameter unavailable' : entry.error ? 'Unavailable' : 'Loading…';
    return `<div class="nks-control" data-pp-slot="${index}"><div class="nks-control-caption"><span class="nks-slot-number" aria-hidden="true">${index + 1}</span><label for="${base}">${esc(name)}</label></div><span class="nks-section" title="${esc(slot.section || '')}">${esc(slot.section || ' ')}</span><div class="nks-readout"><output id="${base}-display" data-pp-display="${slot.address}">${esc(display)}</output><span class="pp-percent" data-pp-percent="${slot.address}">${percent(value)}</span></div><input id="${base}" type="range" min="0" max="1" step="${window.STFine?.parameterStep(value) || .001}" data-parameter-steps="${window.STFine?.parameterSteps(value) || 0}" value="${value?.value ?? 0}" data-pp-knob="${index}" data-pp-address="${slot.address}" data-pp-name="${esc(name)}" ${automate} ${writable ? '' : 'disabled'} aria-label="${esc(name)}, knob ${index + 1}" aria-valuetext="${esc(display)}" aria-describedby="pp-${track}-hint"></div>`;
  }
  /** No knobs yet: a clear loading, error (with Retry) or no-parameters state. */
  const emptyState = entry => entry.loaded ? 'empty' : entry.error ? 'error' : 'loading';
  function emptyBody(entry, track) {
    const state = emptyState(entry);
    if (state === 'error') return `<div class="pp-empty pp-empty-error" data-pp-state="error" role="alert"><p class="pp-error-text">${esc(entry.error)}</p><button type="button" id="pp-${track}-retry" data-pp-action="retry">Retry</button></div>`;
    return `<p class="muted pp-empty" data-pp-state="${state}">${state === 'empty' ? 'This plug-in exposes no host parameters. Use its editor.' : 'Reading parameters…'}</p>`;
  }
  function render(inst, instrumentPlugin) {
    const view = selection(inst, instrumentPlugin);
    if (!view) return '';
    const { slots, plugin, entry, pages, page, track } = view, current = pages[page], user = current?.kind === 'user', nks = !!plugin.nks?.pages?.length;
    const tabs = slots.map(item => `<button type="button" role="tab" id="pp-${track}-tab-${slotKey(item).replace(':', '-')}" data-pp-tab="${slotKey(item)}" aria-selected="${item === plugin}" tabindex="${item === plugin ? 0 : -1}" class="pp-tab ${item === plugin ? 'pp-tab-on' : ''}"><small>${esc(kindLabel(item))}</small><span>${esc(pluginName(item))}</span></button>`).join('');
    const pager = pages.length ? `<div class="nks-pages"><button id="pp-${track}-previous" data-pp-page-step="-1" aria-label="Previous page" ${page === 0 ? 'disabled' : ''}>‹</button><label class="nks-page-label" for="pp-${track}-page">Page<select id="pp-${track}-page" data-pp-page aria-label="Control page">${pages.map((item, index) => `<option value="${index}" ${index === page ? 'selected' : ''}>${index + 1}. ${item.kind === 'user' ? '★ ' : ''}${esc(item.name)}</option>`).join('')}</select></label><span class="nks-page-count">${page + 1} / ${pages.length}</span><button id="pp-${track}-next" data-pp-page-step="1" aria-label="Next page" ${page === pages.length - 1 ? 'disabled' : ''}>›</button></div>` : '';
    const actions = nks ? '' : `<div class="pp-actions"><button type="button" id="pp-${track}-find" data-pp-action="find" title="Find a parameter · /">Find <kbd>/</kbd></button><button type="button" id="pp-${track}-new" data-pp-action="new" title="Add an empty page of your own">New page</button>${user ? `<button type="button" id="pp-${track}-rename" data-pp-action="rename">Rename</button><button type="button" id="pp-${track}-delete" data-pp-action="delete">Remove page</button>` : ''}</div>`;
    const body = !current ? emptyBody(entry, track) : `<div class="nks-control-grid">${current.slots.map((slot, index) => knob(view, slot, index)).join('')}</div>`;
    const hint = `<kbd>← →</kbd> knob · <kbd>↑ ↓</kbd> adjust 1% / one choice · <kbd>⇧</kbd> coarse · <kbd>⌥</kbd> fine · <kbd>⇧ ← →</kbd> page · <kbd>← →</kbd> on the slot tabs: plug-in · <kbd>⌥ arrows</kbd> blocks · <kbd>A</kbd> automate${nks ? '' : ' · <kbd>↵</kbd> assign · <kbd>P</kbd> pin · <kbd>⌫</kbd> clear'}`;
    return `<section class="panel wide nks-console pp-console" data-pp-root="${track}" aria-label="Plug-in control pages"><div class="panel-head nks-heading pp-heading"><div><div class="eyebrow">${nks ? 'NKS controls' : current?.kind === 'user' ? 'Your page' : current?.kind === 'builtin' ? 'Effect pages' : 'Plug-in controls'}</div><h2>${esc(pluginName(plugin))}</h2></div>${pager}</div><div class="pp-tabs ${slots.length > 9 ? 'pp-tabs-many' : ''}" role="tablist" aria-label="Plug-in slots on this track">${tabs}</div><div class="panel-body">${body}<div class="nks-footer"><span id="pp-${track}-hint">${hint}</span>${actions}<span data-pp-status role="status" aria-live="polite">${current ? esc(entry.error) : ''}</span></div></div></section>`;
  }

  // ---- Host communication --------------------------------------------------------------------
  function update(entry) {
    relink(entry);
    for (const fn of updateListeners) try { fn(entry.id); } catch {}
    const root = mountedRoot;
    if (!root?.isConnected) return;
    for (const input of root.querySelectorAll('[data-pp-address]')) {
      const value = entry.values.get(input.dataset.ppAddress);
      if (!value || !entries.has(entry.id) || root.dataset.ppInstance !== entry.id) continue;
      input.disabled = value.writable === false;
      input.step=String(window.STFine?.parameterStep(value)||.001);input.dataset.parameterSteps=String(window.STFine?.parameterSteps(value)||0);
      if (document.activeElement !== input || !entry.pending.has(input.dataset.ppAddress)) input.value = String(value.value);
      input.setAttribute('aria-valuetext', value.display);
    }
    if (root.dataset.ppInstance !== entry.id) return;
    for (const output of root.querySelectorAll('[data-pp-display]')) { const value = entry.values.get(output.dataset.ppDisplay); output.textContent = value ? value.display : entry.loaded ? 'Parameter unavailable' : entry.error ? 'Unavailable' : 'Loading…'; }
    for (const output of root.querySelectorAll('[data-pp-percent]')) output.textContent = percent(entry.values.get(output.dataset.ppPercent));
    // The empty-state body already shows a read error; the footer repeats only edit/poll errors.
    const status = root.querySelector('[data-pp-status]'); if (status) status.textContent = root.querySelector('[data-pp-state="error"]') ? '' : entry.error;
  }
  const mountedFor = entry => mountedRoot?.isConnected && mountedRoot.dataset.ppInstance === entry.id ? mountedRoot : null;
  /** Re-renders the console when its empty body (loading / error / no parameters) is out of date. */
  function redrawEmpty(entry) {
    const root = mountedFor(entry);
    if (!root || root.querySelector('[data-pp-address]')) return false;
    const shown = root.querySelector('[data-pp-state]');
    if (shown?.dataset.ppState === emptyState(entry) && (emptyState(entry) !== 'error' || shown.textContent.includes(entry.error))) return false;
    if (root.contains(document.activeElement)) refreshKeepingFocus(local.wantKnob ?? null); else S().refresh();
    return true;
  }
  function fail(entry, message) {
    entry.error = String(message || 'Could not read plug-in parameters.');
    if (!redrawEmpty(entry)) update(entry);
  }
  function withTimeout(promise, plugin) {
    let timer;
    const timeout = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${pluginName(plugin)} did not report its parameters within ${READ_TIMEOUT / 1000} seconds. Retry, or open its editor.`)), READ_TIMEOUT); });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
  }
  async function read(entry, addresses = null) {
    const plugin = descriptor(entry);
    if (!plugin) { if (S().state.project === entry.project && !entry.loaded) fail(entry, 'This plug-in is no longer in the project.'); return; }
    if (entry.loading || entry.pending.size) return;
    entry.loading = true; entry.lastAttempt = Date.now();
    try {
      const response = await withTimeout(S().request('plugin.parameters', { ...targetOf(plugin), ...(addresses?.length ? { addresses } : {}) }), plugin);
      if (descriptor(entry) !== plugin && !descriptor(entry)) return;
      const list = Array.isArray(response) ? response : response?.parameters;
      if (!Array.isArray(list)) throw new Error('The audio engine did not return a parameter list for this plug-in.');
      const first = !entry.loaded, recovered = !!entry.error;
      remember(entry, list, !addresses);
      entry.loaded = true; entry.error = ''; entry.lastRead = Date.now();
      // The first full read creates the generated pages (or the no-parameters notice).
      if ((first || recovered) && !addresses && redrawEmpty(entry)) return;
      update(entry);
    } catch (error) { if (descriptor(entry)) fail(entry, error?.message || 'Could not read plug-in parameters.'); }
    finally { entry.loading = false; }
  }
  /** Reads when never read, stale, or (after an error) once the retry interval has passed. */
  function wantsRead(entry, stale) {
    if (entry.loading || entry.pending.size) return false;
    if (entry.error) return Date.now() - entry.lastAttempt >= RETRY_AFTER;
    return !entry.loaded || Date.now() - entry.lastRead > stale;
  }
  function serial(entry, work) {
    const result = entry.chain.catch(() => {}).then(async () => { await S().waitForGraphEdit?.(); await S().waitForEngineUpdate?.(); if (descriptor(entry)) return work(); });
    entry.chain = result;
    result.catch(error => { entry.error = error?.message || String(error); update(entry); S().toast(entry.error, 'error'); });
    return result;
  }
  function flush(entry) {
    clearTimeout(entry.timer); entry.timer = null;
    const pending = [...entry.pending]; entry.pending.clear();
    return serial(entry, async () => {
      for (const [address, value] of pending) {
        const plugin = descriptor(entry); if (!plugin) return;
        const response = await S().request('plugin.parameter', { ...targetOf(plugin), address, value, normalized: true });
        const cached = entry.values.get(address);
        if (cached && !entry.pending.has(address) && cached.value === value) { cached.value = valueOf(response || {}) ?? value; cached.display = shown(entry, address, cached.value, response?.display || printable(value)); }
      }
      update(entry);
    });
  }
  function preview(entry, address, value) {
    const previous = entry.values.get(address);
    if (!previous || previous.writable === false || !descriptor(entry)) return false;
    value = window.STFine?.snapParameter(value,previous) ?? clamp01(value);
    entry.values.set(address, { ...previous, value, display: shown(entry, address, value, printable(value)) });
    entry.pending.set(address, value); entry.editedAt.set(address, Date.now()); entry.revision++; local.editGeneration++;
    S().state.dirty = true; S().markDirty?.();
    clearTimeout(entry.timer); entry.timer = setTimeout(() => flush(entry), 35);
    update(entry); return true;
  }
  /** Writes the final values, captures plug-in state and records one Undo step for the gesture. */
  function commit(entry, name = 'plug-in control') {
    if (entry.revision <= entry.committed || entry.revision <= entry.queued) return entry.chain;
    const revision = entry.queued = entry.revision;
    const applied = flush(entry);
    const capture = serial(entry, async () => {
      await applied;
      const plugin = descriptor(entry); if (!plugin) return;
      const snapshot = await S().request('plugin.state', targetOf(plugin));
      if (snapshot?.stateError) throw new Error(String(snapshot.stateError));
      if (!snapshot || typeof snapshot.state !== 'string' || !snapshot.state) throw new Error('The plug-in did not return a state snapshot.');
      if (snapshot.instanceID && snapshot.instanceID !== entry.id) throw new Error('The plug-in changed before its controls could be saved.');
      if (entry.revision !== revision) return; // A later gesture captures the complete state.
      let committed = false;
      S().mutate(`Adjust ${name}`, project => {
        const index = project.audioPlugins?.findIndex(item => item.instanceID === entry.id) ?? -1;
        if (index < 0) return;
        const old = project.audioPlugins[index], replacement = { ...old, ...snapshot, nks: old.nks, track: old.track, owner:old.owner, kind: old.kind, slot: old.slot };
        project.audioPlugins[index] = replacement;
        const track = project.tracks?.[old.track];
        if (old.kind === 'instrument') { const inst = project.instruments?.[track?.instrument]; if (inst?.plugin?.instanceID === entry.id || inst?.type === 'au') inst.plugin = { ...(inst.plugin || {}), ...replacement }; }
        else if (Array.isArray(track?.plugins) && track.plugins[old.slot ?? 0]) track.plugins[old.slot ?? 0] = { ...track.plugins[old.slot ?? 0], ...replacement };
        committed = true;
      }, { render: false, nativeCapture: true });
      if (!committed) throw new Error('The control edit is still pending. Finish the current operation and try again.');
      entry.committed = revision; entry.error = ''; update(entry);
    });
    capture.catch(() => { if (entry.queued === revision) entry.queued = entry.committed; });
    return capture;
  }
  async function flushEdits() {
    try {
      for (const entry of entries.values()) {
        if (entry.project !== S().state.project) continue;
        if (entry.revision > entry.committed) await commit(entry);
        if (entry.revision > entry.committed) return false;
      }
      return true;
    } catch { return false; }
  }
  const hasPendingEdits = () => [...entries.values()].some(entry => entry.project === S().state.project && (entry.pending.size || entry.revision > entry.committed));

  // ---- Page editing (persisted in the project) ----------------------------------------------
  function editPages(plugin, label, change) {
    S().mutate(label, project => {
      project.pluginPages ||= {};
      const record = project.pluginPages[plugin.instanceID] ||= { pages: [] };
      record.pages = (record.pages || []).map(page => ({ name: page.name, addresses: Array.from({ length: 8 }, (_, slot) => page.addresses?.[slot] ?? null) }));
      record.name = pluginName(plugin);
      change(record.pages);
      if (!record.pages.length) delete project.pluginPages[plugin.instanceID];
    });
  }
  /** Where the user's pages start: a built-in effect's curated pages come first, generated pages come last. */
  const userPageOffset = view => view.pages.filter(page => page.kind === 'builtin').length;
  function assign(view, knobIndex, address) {
    const { plugin, pages, page } = view, current = pages[page];
    if (current?.kind === 'user') editPages(plugin, 'Assign control', list => { list[current.index].addresses[knobIndex] = address; });
    else {
      // Editing a generated page makes it yours, so the change has a place to live.
      const addresses = Array.from({ length: 8 }, (_, slot) => current?.slots[slot]?.address ?? null);
      addresses[knobIndex] = address;
      let index = 0;
      editPages(plugin, 'Assign control', list => { index = list.length; list.push({ name: `My page ${list.length + 1}`, addresses }); });
      local.page.set(plugin.instanceID, userPageOffset(view) + index);
    }
    refreshKeepingFocus(knobIndex);
  }
  function pin(view, address) {
    const { plugin } = view, saved = savedPages(plugin);
    let pageIndex = saved.findIndex(page => page.name === 'Pinned'), slot = -1;
    if (pageIndex >= 0 && saved[pageIndex].addresses.map(String).includes(String(address))) { S().toast('Already pinned.'); return; }
    if (pageIndex >= 0) slot = Array.from({ length: 8 }, (_, index) => saved[pageIndex].addresses[index] ?? null).findIndex(value => value == null);
    if (pageIndex >= 0 && slot < 0) { S().toast('The Pinned page is full. Clear a knob on it first.', 'info'); return; }
    editPages(plugin, 'Pin control', list => {
      let target = list.findIndex(page => page.name === 'Pinned');
      if (target < 0) { list.unshift({ name: 'Pinned', addresses: Array.from({ length: 8 }, () => null) }); target = 0; if (view.page >= userPageOffset(view)) local.page.set(plugin.instanceID, view.page + 1); }
      list[target].addresses[list[target].addresses.findIndex(value => value == null)] = address;
    });
    S().toast(`Pinned ${view.entry.params.find(parameter => parameter.address === String(address))?.name || 'parameter'} to the Pinned page`);
  }
  function clearKnob(view, knobIndex) {
    const current = view.pages[view.page];
    if (current?.kind !== 'user') { S().toast('Generated pages follow the plug-in. Assign a knob to make the page yours.', 'info'); return; }
    editPages(view.plugin, 'Clear control', list => { list[current.index].addresses[knobIndex] = null; });
    refreshKeepingFocus(knobIndex);
  }
  function pageAction(view, action) {
    const { plugin, pages, page } = view, current = pages[page];
    if (action === 'retry') { view.entry.error = ''; view.entry.lastAttempt = 0; refreshKeepingFocus(); if (!mountedFor(view.entry)) read(view.entry); return; }
    if (action === 'find') return openPicker(view, null);
    if (action === 'new') {
      let index = 0;
      editPages(plugin, 'Add control page', list => { index = list.length; list.push({ name: `My page ${list.length + 1}`, addresses: Array.from({ length: 8 }, () => null) }); });
      local.page.set(plugin.instanceID, userPageOffset(view) + index); refreshKeepingFocus(0); return;
    }
    if (current?.kind !== 'user') return;
    if (action === 'rename') return S().textPrompt('Page name', current.name, name => { if (name?.trim()) editPages(plugin, 'Rename control page', list => { list[current.index].name = name.trim().slice(0, 60); }); });
    if (action === 'delete') { editPages(plugin, 'Remove control page', list => { list.splice(current.index, 1); }); local.page.set(plugin.instanceID, Math.max(0, page - 1)); refreshKeepingFocus(); }
  }
  /** Parameter search. With a knob index it assigns; without, it jumps to the parameter's page. */
  function openPicker(view, knobIndex) {
    const { entry, plugin } = view;
    if (!entry.params.length) { S().toast(entry.loaded ? 'This plug-in exposes no host parameters.' : 'Parameters are still loading.', 'info'); return; }
    const assigning = knobIndex !== null && knobIndex !== undefined;
    S().openModal(assigning ? `Assign knob ${knobIndex + 1} · ${pluginName(plugin)}` : `Find parameter · ${pluginName(plugin)}`,
      `<div class="pp-picker"><input id="pp-picker-search" type="search" placeholder="Search ${entry.params.length} parameters…" autocomplete="off" spellcheck="false" aria-label="Search parameters"><div id="pp-picker-list" class="pp-picker-list" role="listbox" tabindex="-1" aria-label="Parameters"></div><p class="small-note">↑ ↓ choose · Return ${assigning ? 'assigns to the knob' : 'shows its page'} · ⌥P pins · Escape closes</p></div>`,
      '<button id="pp-picker-cancel">Cancel</button>', { kind: 'pluginPages', className: 'pp-picker-modal' });
    const input = document.getElementById('pp-picker-search'), list = document.getElementById('pp-picker-list');
    if (!input || !list) return;
    let matches = [], selected = 0;
    const draw = () => {
      const words = input.value.toLocaleLowerCase().split(/\s+/).filter(Boolean);
      matches = entry.params.filter(parameter => words.every(word => parameter.name.toLocaleLowerCase().includes(word))).slice(0, 300);
      selected = Math.max(0, Math.min(selected, matches.length - 1));
      list.innerHTML = matches.map((parameter, index) => `<div role="option" id="pp-picker-${index}" class="pp-picker-row ${index === selected ? 'selected' : ''}" aria-selected="${index === selected}" data-pp-pick="${index}"><span>${esc(parameter.name)}</span><small>${parameter.writable ? esc(entry.values.get(parameter.address)?.display || '') : 'Read-only'}</small></div>`).join('') || '<p class="muted">No matching parameters.</p>';
      list.setAttribute('aria-activedescendant', `pp-picker-${selected}`);
      document.getElementById(`pp-picker-${selected}`)?.scrollIntoView?.({ block: 'nearest' });
    };
    const choose = index => {
      const parameter = matches[index]; if (!parameter) return;
      S().closeModal();
      if (assigning) { assign(view, knobIndex, parameter.address); return; }
      const pages = pagesFor(view.plugin, entry), target = pages.findIndex(page => page.slots.some(slot => slot?.address === parameter.address));
      if (target >= 0) { local.page.set(view.plugin.instanceID, target); refreshKeepingFocus(pages[target].slots.findIndex(slot => slot?.address === parameter.address)); }
      else S().toast(`${parameter.name} is read-only and has no knob.`, 'info');
    };
    input.addEventListener('input', () => { selected = 0; draw(); });
    input.addEventListener('keydown', event => {
      if (event.isComposing) return;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); selected = Math.max(0, Math.min(matches.length - 1, selected + (event.key === 'ArrowDown' ? 1 : -1))); draw(); }
      else if (event.key === 'Enter') { event.preventDefault(); choose(selected); }
      else if ((event.key === 'p' || event.key === 'P') && event.altKey) { event.preventDefault(); const parameter = matches[selected]; if (parameter) { S().closeModal(); pin(view, parameter.address); } }
    });
    list.addEventListener('click', event => { const row = event.target.closest?.('[data-pp-pick]'); if (row) choose(Number(row.dataset.ppPick)); });
    document.getElementById('pp-picker-cancel')?.addEventListener('click', () => S().closeModal());
    draw(); setTimeout(() => input.focus(), 0);
  }

  // ---- Focus and binding ---------------------------------------------------------------------
  function refreshKeepingFocus(knobIndex = null) {
    const active = document.activeElement, index = knobIndex ?? (active?.dataset?.ppKnob !== undefined ? Number(active.dataset.ppKnob) : null), id = active?.id;
    S().refresh();
    const root = document.querySelector('[data-pp-root]');
    const next = index !== null && index >= 0 ? root?.querySelector(`[data-pp-knob="${index}"]`) : id ? document.getElementById(id) : null;
    // A knob requested before the plug-in's parameters arrive is focused after the first read.
    local.wantKnob = index !== null && index >= 0 && !next ? index : null;
    (next && !next.disabled ? next : root?.querySelector('[data-pp-knob]:not([disabled])') || root?.querySelector('[data-pp-tab][tabindex="0"]'))?.focus({ preventScroll: true });
  }
  function currentView() {
    const state = S().state, track = state.project?.tracks?.[state.track];
    const instrumentPlugin = track?.instrument === state.instrument ? state.project.audioPlugins?.find(plugin => plugin.track === state.track && plugin.kind === 'instrument') : null;
    return selection(state.project?.instruments?.[state.instrument], instrumentPlugin);
  }
  function automateKnob(input) {
    if (!input?.dataset?.automate) { S().toast('This parameter cannot be automated.', 'info'); return; }
    const value = Number(input.value), parameter = input.dataset.automate, name = input.dataset.name, track = input.dataset.owner || Number(input.dataset.track);
    flushEdits().then(ok => { if (ok) S().automate(track, parameter, name, value); });
  }
  function adjust(entry, input, delta, modifiers = {}) {
    if (input.disabled) return;
    const value = window.STFine?.stepParameter(input.value,Math.sign(delta),modifiers,entry.values.get(input.dataset.ppAddress)) ?? clamp01(Number(input.value) + delta);
    input.value = String(value);
    preview(entry, input.dataset.ppAddress, value);
    local.adjusting = { entry, name: input.dataset.ppName };
    clearTimeout(local.commitTimer);
    local.commitTimer = setTimeout(() => finishAdjust(), 700);
  }
  function finishAdjust() {
    clearTimeout(local.commitTimer); local.commitTimer = null;
    const adjusting = local.adjusting; local.adjusting = null;
    if (adjusting) commit(adjusting.entry, adjusting.name);
  }
  function bind(scope = document) {
    const root = scope.querySelector?.('[data-pp-root]');
    clearTimeout(local.poll); local.poll = null;
    if (!root) { mountedRoot = null; return; }
    const view = currentView();
    if (!view) return;
    const { entry, plugin } = view;
    mountedRoot = root; root.dataset.ppInstance = entry.id;
    for (const input of root.querySelectorAll('[data-pp-address]')) {
      input.addEventListener('input', () => preview(entry, input.dataset.ppAddress, Number(input.value)));
      input.addEventListener('change', () => commit(entry, input.dataset.ppName));
      window.STFine?.bindDrag(input);  // ⌥-drag 1/10 speed, ⇧ 1/4, ⌥⇧ 1/50
    }
    for (const button of root.querySelectorAll('[data-pp-empty]')) button.addEventListener('click', () => openPicker(currentView() || view, Number(button.dataset.ppKnob)));
    for (const tab of root.querySelectorAll('[data-pp-tab]')) tab.addEventListener('click', () => { local.slotKey.set(view.track, tab.dataset.ppTab); refreshKeepingFocus(); root.isConnected || document.querySelector(`[data-pp-tab="${tab.dataset.ppTab}"]`)?.focus(); });
    root.querySelector('[data-pp-page]')?.addEventListener('change', event => { finishAdjust(); local.page.set(plugin.instanceID, Number(event.target.value)); refreshKeepingFocus(); });
    for (const button of root.querySelectorAll('[data-pp-page-step]')) button.addEventListener('click', () => { finishAdjust(); local.page.set(plugin.instanceID, view.page + Number(button.dataset.ppPageStep)); refreshKeepingFocus(); });
    for (const button of root.querySelectorAll('[data-pp-action]')) button.addEventListener('click', () => pageAction(currentView() || view, button.dataset.ppAction));
    root.addEventListener('keyup', event => { if (local.adjusting && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)) finishAdjust(); });
    root.addEventListener('keydown', event => key(event, root));
    if (!entry.error && wantsRead(entry, 2000)) read(entry);
    const poll = () => {
      if (!root.isConnected) { mountedRoot = mountedRoot === root ? null : mountedRoot; finishAdjust(); return; }
      const addresses = Array.from(root.querySelectorAll('[data-pp-address]')).map(input => input.dataset.ppAddress);
      // A failed read retries on its own, so a slot that finishes loading later still appears.
      if (entry.error && wantsRead(entry, 0)) read(entry, entry.loaded && addresses.length ? addresses : null);
      else if (addresses.length && !entry.pending.size) read(entry, addresses);
      local.poll = setTimeout(poll, 750);
    };
    local.poll = setTimeout(poll, 750);
  }
  function key(event, root) {
    if (!root.isConnected || event.isComposing || event.metaKey || event.ctrlKey) return;
    const target = event.target, view = currentView();
    if (!view) return;
    const stop = () => { event.preventDefault(); event.stopPropagation(); };
    const knobs = Array.from(root.querySelectorAll('[data-pp-knob]'));
    const tabs = Array.from(root.querySelectorAll('[data-pp-tab]'));
    // Plug-in slot tabs and ⌥ ← → from anywhere in the console.
    if ((target?.dataset?.ppTab !== undefined && !event.altKey && !event.shiftKey && ['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) || (event.altKey && !event.shiftKey && ['ArrowLeft', 'ArrowRight'].includes(event.key))) {
      stop(); if (tabs.length < 2) return;
      const index = tabs.findIndex(tab => tab.getAttribute('aria-selected') === 'true');
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : Math.max(0, Math.min(tabs.length - 1, index + (event.key === 'ArrowRight' ? 1 : -1)));
      finishAdjust(); local.slotKey.set(view.track, tabs[next].dataset.ppTab);
      const onTab = target?.dataset?.ppTab !== undefined;
      refreshKeepingFocus(onTab ? -1 : null);
      if (onTab) document.querySelector(`[data-pp-tab="${tabs[next].dataset.ppTab}"]`)?.focus();
      return;
    }
    if (target?.dataset?.ppTab !== undefined && event.key === 'ArrowDown' && !event.altKey) { stop(); (knobs.find(knob => !knob.disabled))?.focus(); return; }
    if (event.shiftKey && !event.altKey && ['ArrowLeft', 'ArrowRight'].includes(event.key) && root.contains(target) && target?.tagName !== 'SELECT') {
      stop(); finishAdjust();
      local.page.set(view.plugin.instanceID, Math.max(0, Math.min(view.pages.length - 1, view.page + (event.key === 'ArrowRight' ? 1 : -1))));
      refreshKeepingFocus(); return;
    }
    if (target?.dataset?.ppKnob === undefined) return;
    const index = Number(target.dataset.ppKnob), isRange = target.type === 'range';
    if (!event.altKey && !event.shiftKey && ['ArrowLeft', 'ArrowRight'].includes(event.key)) {
      stop(); finishAdjust(); knobs[Math.max(0, Math.min(knobs.length - 1, knobs.indexOf(target) + (event.key === 'ArrowRight' ? 1 : -1)))]?.focus(); return;
    }
    if (target.dataset.ppKnob !== undefined && event.key === 'ArrowUp' && !isRange) { stop(); tabs.find(tab => tab.tabIndex === 0)?.focus(); return; }
    if (isRange && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)) {
      stop();
      const step = event.key.startsWith('Page') ? 0.1 : event.shiftKey ? 0.1 : event.altKey ? 0.001 : 0.01;
      adjust(view.entry, target, (event.key === 'ArrowUp' || event.key === 'PageUp' ? 1 : -1) * step, {coarse:event.shiftKey||event.key.startsWith('Page'),fine:event.altKey}); return;
    }
    if (event.altKey || event.shiftKey) return;
    if (/^[1-8]$/.test(event.key) && !S().state.keyboardPlay) { stop(); knobs[Number(event.key) - 1]?.focus(); return; }
    if (event.key.toLowerCase() === 'a' && isRange) { stop(); if (!event.repeat) { finishAdjust(); automateKnob(target); } return; }
    if (view.plugin.nks?.pages?.length) return;
    if (event.key === 'Enter') { stop(); finishAdjust(); openPicker(view, index); return; }
    if (event.key === '/') { stop(); openPicker(view, null); return; }
    if (event.key.toLowerCase() === 'p' && isRange) { stop(); pin(view, target.dataset.ppAddress); return; }
    if ((event.key === 'Backspace' || event.key === 'Delete') && isRange) { stop(); clearKnob(view, index); }
  }

  function event(type, payload = {}) {
    if (type === 'project.loaded' || type === 'project.changed') { entries.clear(); local.page.clear(); return; }
    if (!['plugin.parameter', 'plugin.parameters', 'plugin.changed'].includes(type) || !mountedRoot?.isConnected) return;
    const entry = entries.get(mountedRoot.dataset.ppInstance);
    if (entry && Date.now() - entry.lastRead > 350) read(entry);
  }
  // Hardware controllers (mk3.js): the same slots, pages, previews and one-Undo-per-gesture commits as the keyboard.
  const makeController = (resolve=currentView, choose=null) => ({
    view() {
      const view = resolve(); if (!view) return null;
      const { slots, plugin, entry, pages, page } = view;
      if (wantsRead(entry, 1500)) read(entry);
      return { slots: slots.map(item => ({ key: slotKey(item), label: kindLabel(item), name: pluginName(item), instanceID: item.instanceID, kind: item.kind, slot: item.slot ?? 0, track: item.track })),
        slot: slotKey(plugin), slotIndex:plugin.slot??0, owner:pluginOwner(plugin), name: pluginName(plugin), kind: plugin.kind, instanceID: plugin.instanceID, track: plugin.track, nks: !!plugin.nks?.pages?.length,
        component: plugin.component || '', builtin: !!FX()?.isBuiltin(plugin.component), bypass: !!plugin.bypass, pageKinds: pages.map(item => item.kind),
        pages: pages.map(item => item.name), page, loaded: entry.loaded, error: entry.error,
        knobs: (pages[page]?.slots || Array(8).fill(null)).map(slot => { const value = slot?.address ? entry.values.get(slot.address) : null; return slot ? { name: slot.name, id: slot.id || '', fullName: slot.fullName || slot.name, defaultValue: slot.defaultValue, section: slot.section || '', address: slot.address, value: value ? value.value : null, display: value ? value.display : slot.address ? (entry.loaded || entry.error ? 'Unavailable' : 'Loading…') : '', writable: !!value && value.writable !== false, automatable: !!value && value.automatable !== false, stepCount:value?.stepCount ?? (slot.stepCount || undefined), normalizedStep:value?.normalizedStep ?? (slot.stepCount ? 1 / slot.stepCount : undefined) } : null; }) };
    },
    selectSlot(key) { const view = resolve(); if (!view) return false; finishAdjust(); local.slotKey.set(view.track, key); const selected=view.slots.find(item=>slotKey(item)===key); if(selected) choose?.(selected.instanceID); if (S().state.view === 'instrument') S().refresh(); return true; },
    setPage(index) { const view = resolve(); if (!view) return false; finishAdjust(); local.page.set(view.plugin.instanceID, Math.max(0, Math.min(view.pages.length - 1, index))); if (S().state.view === 'instrument') S().refresh(); return true; },
    adjust(knob, delta) {
      const view = resolve(), slot = view?.pages[view.page]?.slots[knob]; if (!slot?.address) return null;
      const value = view.entry.values.get(slot.address); if (!value || value.writable === false) return null;
      const next = window.STFine?.snapParameter(value.value+delta,value) ?? clamp01(value.value + delta);
      preview(view.entry, slot.address, next);
      local.adjusting = { entry: view.entry, name: slot.name };
      clearTimeout(local.commitTimer); local.commitTimer = setTimeout(() => finishAdjust(), 700);
      return next;
    },
    /** Sets one knob to an absolute normalized value (reset, typed values); one Undo step like a turn. */
    set(knob, value) {
      const view = resolve(), slot = view?.pages[view.page]?.slots[knob]; if (!slot?.address) return null;
      const current = view.entry.values.get(slot.address); if (!current || current.writable === false) return null;
      const next = window.STFine?.snapParameter(value, current) ?? clamp01(value);
      preview(view.entry, slot.address, next);
      local.adjusting = { entry: view.entry, name: slot.name };
      finishAdjust();
      return next;
    },
    /** Back to the parameter's default (built-in effects; other plug-ins report no default). */
    reset(knob) {
      const view = resolve(), slot = view?.pages[view.page]?.slots[knob];
      return Number.isFinite(slot?.defaultValue) ? this.set(knob, slot.defaultValue) : null;
    },
    finish: () => finishAdjust()
  });
  const controller=makeController();
  controller.forOwner=(owner,instanceID)=>{ let selected=instanceID; return makeController(()=>selectionForOwner(owner,selected),id=>{selected=id;}); };
  const onUpdate = fn => { updateListeners.add(fn); return () => updateListeners.delete(fn); };
  window.STEffectPages?.onChange(() => { for (const entry of entries.values()) if (FX().isBuiltin(entry.component)) { for (const [address, value] of entry.values) value.display = shown(entry, address, value.value, value.display); relink(entry); } if (mountedRoot?.isConnected) S().refresh(); });
  window.STPluginPages = { render, bind, event, flushEdits, controller, onUpdate, get hasPendingEdits() { return hasPendingEdits(); }, get editGeneration() { return local.editGeneration; },
    _test: { pagesFor, entryFor, slotsFor, slotsForOwner, selectionForOwner, targetOf, savedPages, read, READ_TIMEOUT, RETRY_AFTER } };
})();
