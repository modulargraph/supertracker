/* Insert chain: a Maschine-style module strip and eight-knob editor for the inserts of one owner (track,
   group or return bus, master). Modules are tiles in signal order: + adds from a categorized list, drag (or ⌥ ← →)
   reorders, the power button bypasses, and Duplicate / Remove act on the selected module. The selected module's
   pages (built-in effects: their curated pages from effect-pages.js; other plug-ins: generated pages) show as
   eight labelled knobs with page tabs. Drag a knob vertically, ⇧-drag for fine control, double-click to reset.
   Every edit goes through window.STInserts (studio.js) and STPluginPages (values, one Undo step per gesture). */
(() => {
  'use strict';
  const S = () => window.ST;
  const FX = () => window.STEffectPages;
  const API = () => window.STInserts;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const local = { selected: new Map(), adding: null, query: '', plugins: null, pluginsLoading: false, poll: null, drag: null, focus: null, unsubscribe: null };
  let mounted = null; // { root, owner, options }

  // ---- Model ----------------------------------------------------------------------------------------------------
  const chainOf = owner => API()?.chain(owner) || [];
  function selectedOf(owner, chain, options = {}) {
    if (options.selected !== undefined) return chain.find(item => item.instanceID && item.instanceID === options.selected) || null;
    const wanted = local.selected.get(owner);
    return chain.find(item => item.instanceID === wanted) || chain[0] || null;
  }
  const controllerFor = (owner, plugin) => plugin?.instanceID ? window.STPluginPages?.controller?.forOwner?.(owner, plugin.instanceID) : null;
  function moduleView(owner, plugin) {
    const ctl = controllerFor(owner, plugin);
    let view = null; try { view = ctl?.view?.() || null; } catch { view = null; }
    return view && view.instanceID === plugin.instanceID ? { ctl, view } : { ctl, view: null };
  }
  const category = plugin => FX()?.isBuiltin(plugin.component) ? FX().categoryOf(plugin.component) || 'Effect' : plugin.manufacturer || plugin.format || 'Plug-in';
  const displayName = plugin => plugin?.nks?.name || plugin?.name || plugin?.component || 'Effect';

  // ---- Rendering ------------------------------------------------------------------------------------------------
  const ARC = { cx: 24, cy: 24, r: 18 };
  const point = turn => { const a = (-135 + 270 * turn) * Math.PI / 180; return [ARC.cx + ARC.r * Math.sin(a), ARC.cy - ARC.r * Math.cos(a)]; };
  function arc(from, to) {
    if (Math.abs(to - from) < 1e-4) return '';
    const [a, b] = [Math.min(from, to), Math.max(from, to)], [x1, y1] = point(a), [x2, y2] = point(b);
    return `M${x1.toFixed(2)} ${y1.toFixed(2)}A${ARC.r} ${ARC.r} 0 ${(b - a) * 270 > 180 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
  }
  /** The value arc starts at the centre for bipolar parameters (pan, gain, pitch) and at the left end otherwise. */
  function origin(view, knob) {
    const p = view?.builtin ? FX()?.parameter(view.component, knob.id) : null;
    return p && p.min < 0 && p.max > 0 && !p.labels?.length ? FX().normalized(p, 0) : 0;
  }
  function dial(view, knob, index, prefix) {
    const value = Number.isFinite(knob?.value) ? clamp(knob.value, 0, 1) : null, start = origin(view, knob), [px, py] = point(value ?? 0);
    const steps = Number(knob?.stepCount) > 0 ? Number(knob.stepCount) : 0;
    return `<div class="fx-dial${steps ? ' is-stepped' : ''}" id="${prefix}-knob-${index}" role="slider" tabindex="0" data-fx-dial="${index}" aria-label="${esc(knob.fullName || knob.name)}"`
      + ` aria-valuemin="0" aria-valuemax="100" aria-valuenow="${value === null ? '' : Math.round(value * 100)}" aria-valuetext="${esc(knob.display || '')}"`
      + `${knob.writable ? '' : ' aria-disabled="true"'} title="Drag · ⇧ fine · double-click resets">`
      + `<svg viewBox="0 0 48 48" aria-hidden="true"><path class="fx-dial-track" d="${arc(0, 1)}"/><path class="fx-dial-value" data-fx-arc d="${value === null ? '' : arc(start, value)}"/>`
      + `<circle class="fx-dial-cap" cx="24" cy="24" r="11"/><line class="fx-dial-pointer" data-fx-pointer x1="24" y1="24" x2="${px.toFixed(2)}" y2="${py.toFixed(2)}"${value === null ? ' style="display:none"' : ''}/></svg></div>`;
  }
  function knobCell(view, knob, index, prefix) {
    if (!knob) return `<div class="fx-knob is-empty" data-fx-knob="${index}"><span class="fx-section"> </span><div class="fx-dial-blank" aria-hidden="true"></div><span class="fx-knob-name">–</span><output> </output></div>`;
    if (!knob.address) return `<div class="fx-knob is-empty" data-fx-knob="${index}"><span class="fx-section">${esc(knob.section || ' ')}</span><div class="fx-dial-blank" aria-hidden="true"></div><span class="fx-knob-name">${esc(knob.name)}</span><output>Unavailable</output></div>`;
    const automate = knob.automatable && knob.writable ? ` data-automate="plugin:effect:${view.slotIndex}:${esc(knob.address)}" data-owner="${esc(view.owner)}" data-name="${esc(`${view.name} · ${knob.fullName || knob.name}`)}"` : '';
    return `<div class="fx-knob${knob.section ? ' has-section' : ''}" data-fx-knob="${index}"${automate}><span class="fx-section">${esc(knob.section || ' ')}</span>${dial(view, knob, index, prefix)}`
      + `<label class="fx-knob-name" for="${prefix}-knob-${index}" title="${esc(knob.fullName || knob.name)}">${esc(knob.name)}</label><output data-fx-value="${index}">${esc(knob.display || '')}</output></div>`;
  }
  function tile(owner, plugin, index, selected, prefix) {
    const on = selected?.instanceID === plugin.instanceID;
    return `<div class="fx-tile${on ? ' is-selected' : ''}${plugin.bypass ? ' is-bypassed' : ''}" id="${prefix}-tile-${plugin.slot}" role="option" aria-selected="${on}"`
      + ` tabindex="${on ? 0 : -1}" draggable="true" data-fx-tile="${plugin.slot}" data-fx-instance="${esc(plugin.instanceID || '')}" title="${esc(displayName(plugin))} · drag to reorder"`
      + ` aria-keyshortcuts="Alt+ArrowLeft Alt+ArrowRight B D Delete"><span class="fx-tile-top"><span class="fx-tile-index">${index + 1}</span><span class="fx-tile-category">${esc(category(plugin))}</span>`
      + `<button type="button" class="fx-power" tabindex="-1" data-fx-power="${plugin.slot}" aria-pressed="${!plugin.bypass}" aria-label="${plugin.bypass ? 'Turn on' : 'Bypass'} ${esc(displayName(plugin))}" title="${plugin.bypass ? 'Bypassed · click to turn on' : 'On · click to bypass'} (B)">`
      + `<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 2v6M4.6 4.2a5 5 0 1 0 6.8 0"/></svg></button></span><strong class="fx-tile-name">${esc(displayName(plugin))}</strong>`
      + `<span class="fx-tile-state">${plugin.bypass ? 'Bypassed' : FX()?.isBuiltin(plugin.component) ? 'Built-in' : esc(plugin.format || 'Plug-in')}</span></div>`;
  }
  function addPanel(owner, prefix) {
    const query = local.query.trim().toLowerCase(), match = name => !query || String(name).toLowerCase().includes(query);
    const groups = (FX()?.categories(S().state.plugins) || []).map(group => ({ ...group, effects: group.effects.filter(effect => match(effect.name)) })).filter(group => group.effects.length);
    const others = (local.plugins || []).filter(item => !FX()?.isBuiltin(item.id) && (item.type || item.kind) === 'effect' && match(`${item.name} ${item.manufacturer || ''}`)).slice(0, 60);
    const column = (name, items) => `<div class="fx-add-column" role="group" aria-label="${esc(name)}"><h5>${esc(name)}</h5>${items.join('')}</div>`;
    const choice = (id, name, detail, bundle = '') => `<button type="button" class="fx-add-choice" data-fx-choose="${esc(id)}" data-fx-bundle="${esc(bundle)}"><span>${esc(name)}</span>${detail ? `<small>${esc(detail)}</small>` : ''}</button>`;
    const columns = groups.map(group => column(group.name, group.effects.map(effect => choice(effect.id, effect.name, ''))));
    columns.push(column('Plug-ins', local.plugins ? others.length ? others.map(item => choice(item.id, item.name, item.manufacturer || item.format || '', item.bundlePath || '')) : ['<p class="muted fx-add-note">No matching plug-in effects.</p>']
      : [`<p class="muted fx-add-note">${local.pluginsLoading ? 'Reading installed plug-ins…' : 'Installed AU / VST3 effects'}</p>`]));
    return `<div class="fx-add-panel" id="${prefix}-add-panel" role="dialog" aria-label="Add an insert effect"><div class="fx-add-head"><input type="search" id="${prefix}-add-search" class="field" placeholder="Search effects…" value="${esc(local.query)}" aria-label="Search effects" autocomplete="off" spellcheck="false">`
      + `<span class="muted">↑ ↓ ← → choose · Return adds · Esc closes</span><button type="button" data-fx-action="close-add">Close</button></div><div class="fx-add-columns">${columns.join('')}</div></div>`;
  }
  function moduleEditor(owner, plugin, prefix, options) {
    if (!plugin) return '';
    const { view } = moduleView(owner, plugin), chain = chainOf(owner), position = chain.findIndex(item => item.instanceID === plugin.instanceID);
    const pages = view?.pages || [], page = view?.page || 0, knobs = view?.knobs || Array(8).fill(null);
    const status = !view ? 'Loading…' : view.error ? view.error : !view.loaded && !knobs.some(knob => knob?.address) ? 'Reading parameters…' : !pages.length ? 'No host parameters · open the editor' : '';
    const tabs = pages.length > 1 ? `<div class="fx-pages" role="tablist" aria-label="Parameter pages">${pages.map((name, i) => `<button type="button" role="tab" id="${prefix}-page-${i}" data-fx-page="${i}" aria-selected="${i === page}" tabindex="${i === page ? 0 : -1}" class="${i === page ? 'is-on' : ''}">${esc(name)}</button>`).join('')}</div>`
      : pages.length ? `<div class="fx-pages"><span class="fx-page-single">${esc(pages[0])}</span></div>` : '<div class="fx-pages"></div>';
    const builtin = FX()?.isBuiltin(plugin.component);
    return `<div class="fx-module${plugin.bypass ? ' is-bypassed' : ''}" data-fx-module="${esc(plugin.instanceID || '')}"><div class="fx-module-head"><div class="fx-module-title"><small>Insert ${position + 1} · ${esc(category(plugin))}</small><h4>${esc(displayName(plugin))}</h4></div>${tabs}`
      + `<div class="fx-module-actions"><button type="button" id="${prefix}-bypass" data-fx-action="bypass" aria-pressed="${!!plugin.bypass}" class="${plugin.bypass ? 'is-active' : ''}" title="B">${plugin.bypass ? 'Bypassed' : 'Bypass'}</button>`
      + `<button type="button" id="${prefix}-duplicate" data-fx-action="duplicate" title="D" ${chain.length >= (API()?.SLOTS || 16) ? 'disabled' : ''}>Duplicate</button>${builtin ? '' : `<button type="button" id="${prefix}-editor" data-fx-action="editor">Editor ↗</button>`}`
      + `<button type="button" id="${prefix}-remove" data-fx-action="remove" class="danger" title="⌫">Remove</button></div></div>`
      + (status && !knobs.some(knob => knob?.address) ? `<p class="fx-status muted" role="status">${esc(status)}</p>` : `<div class="fx-knobs" role="group" aria-label="${esc(pages[page] || 'Parameters')}">${knobs.map((knob, i) => knobCell(view, knob, i, prefix)).join('')}</div>`)
      + `<p class="fx-hint"><kbd>↑ ↓</kbd> adjust · <kbd>⇧</kbd> fine · <kbd>← →</kbd> knob · <kbd>⇧ ← →</kbd> page · <kbd>⌫</kbd> default · <kbd>A</kbd> automate${options.hint ? ' · ' + options.hint : ''}</p></div>`;
  }
  /** The chain of one owner. options: prefix (element ids), title, source tiles before the inserts ([{id, label, name,
      selected}], e.g. an instrument), selected / onSelect (an external selection, by instance ID),
      search (show the mixer's plug-in search button), editor (false: tiles only). */
  function render(owner, options = {}) {
    if (!API() || !owner) return '';
    const prefix = options.prefix || 'fx', chain = chainOf(owner), selected = selectedOf(owner, chain, options), slots = API().SLOTS || 16;
    const sources = (options.source || []).map(item => `<button type="button" class="fx-tile fx-source${item.selected ? ' is-selected' : ''}" id="${esc(item.id)}" ${item.attributes || ''} aria-pressed="${!!item.selected}" tabindex="${item.selected ? 0 : -1}">`
      + `<span class="fx-tile-top"><span class="fx-tile-category">${esc(item.label)}</span></span><strong class="fx-tile-name">${esc(item.name)}</strong><span class="fx-tile-state">${esc(item.detail || 'Source')}</span></button>`).join('');
    const full = chain.length >= slots;
    return `<section class="fx-chain${options.className ? ' ' + options.className : ''}" data-fx-root="${esc(owner)}" data-fx-prefix="${esc(prefix)}" aria-label="Insert effects">`
      + `<div class="fx-chain-head"><div class="fx-chain-title"><span class="eyebrow">Insert chain</span><h3>${esc(options.title || API().channelName(owner) || 'Channel')}</h3><span class="fx-count">${chain.length}/${slots}</span></div>`
      + `<span class="fx-chain-hint muted">← → module · ⌥ ← → or drag to reorder · B bypass · D duplicate · ⌫ remove</span>`
      + `${options.search ? `<button type="button" class="btn" data-studio-action="add-plugin" ${full ? 'disabled' : ''}>Search plug-ins…</button>` : ''}</div>`
      + `<div class="fx-tiles" role="listbox" aria-orientation="horizontal" aria-label="Signal chain">${sources}${chain.map((plugin, i) => tile(owner, plugin, i, selected, prefix)).join('')}`
      + `<button type="button" class="fx-tile fx-add" id="${prefix}-add" data-fx-action="add" ${full ? `disabled title="This channel holds ${slots} inserts"` : 'title="Add an effect"'} aria-expanded="${local.adding === owner}"><span class="fx-add-plus">+</span><span>${chain.length ? 'Add' : 'Add effect'}</span></button></div>`
      + `${local.adding === owner ? addPanel(owner, prefix) : ''}`
      + `${options.editor === false || (chain.length && !selected) ? '' : chain.length ? moduleEditor(owner, selected, prefix, options) : `<p class="fx-empty muted">No insert effects yet. <kbd>+</kbd> adds one: dynamics, EQ and filters, modulation, reverb, delay, distortion or perform effects.</p>`}`
      + `</section>`;
  }

  // ---- Updating values in place -----------------------------------------------------------------------------------
  function paint() {
    const m = mounted; if (!m?.root?.isConnected) return;
    const plugin = selectedOf(m.owner, chainOf(m.owner), m.options); if (!plugin) return;
    const { view } = moduleView(m.owner, plugin); if (!view) return;
    (view.knobs || []).forEach((knob, i) => {
      const cell = m.root.querySelector(`[data-fx-knob="${i}"]`); if (!cell || !knob?.address) return;
      const value = Number.isFinite(knob.value) ? clamp(knob.value, 0, 1) : null;
      const dialNode = cell.querySelector('[data-fx-dial]'), out = cell.querySelector('[data-fx-value]');
      if (out && out.textContent !== knob.display) out.textContent = knob.display || '';
      if (dialNode) { dialNode.setAttribute('aria-valuenow', value === null ? '' : String(Math.round(value * 100))); dialNode.setAttribute('aria-valuetext', knob.display || ''); }
      const path = cell.querySelector('[data-fx-arc]'), pointer = cell.querySelector('[data-fx-pointer]');
      if (path && value !== null) path.setAttribute('d', arc(origin(view, knob), value));
      if (pointer && value !== null) { const [x, y] = point(value); pointer.setAttribute('x2', x.toFixed(2)); pointer.setAttribute('y2', y.toFixed(2)); pointer.style.display = ''; }
    });
    // A first parameter read replaces the loading notice with knobs.
    if (m.root.querySelector('.fx-status') && (view.knobs || []).some(knob => knob?.address)) refresh();
  }
  function refresh(focusID = null) {
    const id = focusID || document.activeElement?.id || null;
    S().refresh();
    const next = id && document.getElementById(id);
    if (next) next.focus({ preventScroll: true });
  }

  // ---- Actions ------------------------------------------------------------------------------------------------------
  const run = (promise, after) => Promise.resolve(promise).then(result => { after?.(result); return result; }, error => { S().toast(error?.message || String(error), 'error'); return null; });
  function select(owner, plugin, options, focus = true) {
    if (!plugin) return;
    window.STPluginPages?.controller?.finish?.();
    if (options.onSelect) options.onSelect(plugin.instanceID); else local.selected.set(owner, plugin.instanceID);
    const prefix = options.prefix || 'fx';
    refresh(focus ? `${prefix}-tile-${plugin.slot}` : null);
  }
  function act(owner, options, action, plugin) {
    const prefix = options.prefix || 'fx';
    if (action === 'add') { local.adding = local.adding === owner ? null : owner; local.query = ''; refresh(); if (local.adding) { loadPlugins(); setTimeout(() => document.getElementById(`${prefix}-add-search`)?.focus(), 0); } return; }
    if (action === 'close-add') { local.adding = null; refresh(`${prefix}-add`); return; }
    if (!plugin) return;
    const name = displayName(plugin);
    if (action === 'bypass') return run(API().toggleBypass(owner, plugin.slot), on => { if (on !== null) S().toast(`${name} ${on ? 'bypassed' : 'on'}`); refresh(`${prefix}-tile-${plugin.slot}`); });
    if (action === 'remove') {
      const chain = chainOf(owner), index = chain.indexOf(plugin), neighbour = chain[index + 1] || chain[index - 1] || null;
      return run(API().remove(owner, plugin.slot), done => {
        if (!done) return;
        if (neighbour) { if (options.onSelect) options.onSelect(neighbour.instanceID); else local.selected.set(owner, neighbour.instanceID); }
        S().toast(`Removed ${name} · ⌘Z restores it`);
        const after = chainOf(owner).find(item => item.instanceID === neighbour?.instanceID);
        refresh(after ? `${prefix}-tile-${after.slot}` : `${prefix}-add`);
      });
    }
    if (action === 'duplicate') return run(API().duplicate(owner, plugin.slot), copy => { if (!copy) return; if (options.onSelect) options.onSelect(copy.instanceID); else local.selected.set(owner, copy.instanceID); refresh(`${prefix}-tile-${copy.slot}`); });
    if (action === 'editor') return run(API().editor(owner, plugin.slot));
  }
  function add(owner, options, component, bundlePath) {
    const prefix = options.prefix || 'fx';
    local.adding = null;
    return run(API().add(owner, component, { bundlePath }), added => {
      if (!added) { refresh(); return; }
      if (options.onSelect) options.onSelect(added.instanceID); else local.selected.set(owner, added.instanceID);
      S().toast(`${displayName(added)} added`);
      refresh(`${prefix}-tile-${added.slot}`);
    });
  }
  function move(owner, options, plugin, toSlot) {
    const prefix = options.prefix || 'fx', id = plugin.instanceID;
    return run(API().move(owner, plugin.slot, toSlot), moved => {
      const now = chainOf(owner).find(item => item.instanceID === id);
      refresh(now ? `${prefix}-tile-${now.slot}` : null);
      return moved;
    });
  }
  async function loadPlugins() {
    if (local.plugins || local.pluginsLoading) return;
    local.pluginsLoading = true;
    try { local.plugins = await API().catalogue(); } catch { local.plugins = []; }
    local.pluginsLoading = false;
    if (local.adding && mounted?.root?.isConnected) { const search = document.activeElement?.id; refresh(search); }
  }

  // ---- Binding ----------------------------------------------------------------------------------------------------
  /** Binds the chain rendered for `owner` (call after render, with the same options). */
  function bind(scope = document, owner = null, options = {}) {
    clearInterval(local.poll); local.poll = null;
    const root = scope.querySelector?.(`[data-fx-root]`);
    if (!root || (owner && root.dataset.fxRoot !== owner)) { mounted = null; return; }
    owner = root.dataset.fxRoot;
    mounted = { root, owner, options };
    const prefix = options.prefix || 'fx', chain = () => chainOf(owner), byTile = node => chain().find(item => String(item.slot) === node?.dataset?.fxTile) || null;
    const current = () => selectedOf(owner, chain(), options);
    local.unsubscribe ||= window.STPluginPages?.onUpdate?.(id => { if (mounted && selectedOf(mounted.owner, chainOf(mounted.owner), mounted.options)?.instanceID === id) paint(); }) || null;
    FX()?.load?.(S().request?.bind(S()));

    // Tiles: select, power, keyboard and drag to reorder.
    const tiles = [...root.querySelectorAll('[data-fx-tile]')];
    for (const node of tiles) {
      node.addEventListener('click', event => { if (event.target.closest?.('[data-fx-power]')) return; const plugin = byTile(node); if (plugin && plugin.instanceID !== current()?.instanceID) select(owner, plugin, options); else node.focus(); });
      node.addEventListener('keydown', event => tileKey(event, node));
      node.addEventListener('dragstart', event => { local.drag = { owner, slot: Number(node.dataset.fxTile) }; node.classList.add('is-dragging'); try { event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', node.dataset.fxTile); } catch {} });
      node.addEventListener('dragend', () => { node.classList.remove('is-dragging'); local.drag = null; for (const t of tiles) t.classList.remove('is-drop-before', 'is-drop-after'); });
      node.addEventListener('dragover', event => {
        if (!local.drag || local.drag.owner !== owner || String(local.drag.slot) === node.dataset.fxTile) return;
        event.preventDefault();
        const order = chain(), from = order.findIndex(item => item.slot === local.drag.slot), to = order.findIndex(item => String(item.slot) === node.dataset.fxTile);
        for (const t of tiles) t.classList.remove('is-drop-before', 'is-drop-after');
        node.classList.add(to > from ? 'is-drop-after' : 'is-drop-before');
      });
      node.addEventListener('dragleave', () => node.classList.remove('is-drop-before', 'is-drop-after'));
      node.addEventListener('drop', event => {
        event.preventDefault();
        const drag = local.drag; local.drag = null; const plugin = drag && chain().find(item => item.slot === drag.slot);
        if (plugin && drag.owner === owner && String(plugin.slot) !== node.dataset.fxTile) move(owner, options, plugin, Number(node.dataset.fxTile));
      });
    }
    for (const button of root.querySelectorAll('[data-fx-power]')) button.addEventListener('click', event => { event.stopPropagation?.(); const plugin = chain().find(item => String(item.slot) === button.dataset.fxPower); act(owner, options, 'bypass', plugin); });
    for (const button of root.querySelectorAll('[data-fx-action]')) button.addEventListener('click', () => act(owner, options, button.dataset.fxAction, current()));
    for (const source of options.source || []) { const node = document.getElementById(source.id); if (!node || !source.onSelect) continue; node.addEventListener('click', () => source.onSelect()); node.addEventListener('keydown', event => tileKey(event, node)); }

    // Add panel.
    const search = root.querySelector(`#${prefix}-add-search`);
    if (search) {
      search.addEventListener('input', () => { const at = search.selectionStart; local.query = search.value; refresh(search.id); document.getElementById(search.id)?.setSelectionRange?.(at, at); });
      search.addEventListener('keydown', event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); act(owner, options, 'close-add'); }
        else if (event.key === 'ArrowDown' || event.key === 'Enter') { event.preventDefault(); const first = root.querySelector('[data-fx-choose]'); if (event.key === 'Enter' && first) first.click(); else first?.focus(); }
      });
    }
    const choices = [...root.querySelectorAll('[data-fx-choose]')];
    for (const choice of choices) {
      choice.addEventListener('click', () => add(owner, options, choice.dataset.fxChoose, choice.dataset.fxBundle || ''));
      choice.addEventListener('keydown', event => {
        if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); act(owner, options, 'close-add'); return; }
        const column = choice.closest('.fx-add-column'), columns = [...root.querySelectorAll('.fx-add-column')], inColumn = [...column.querySelectorAll('[data-fx-choose]')], at = inColumn.indexOf(choice);
        let next = null;
        if (event.key === 'ArrowDown') next = inColumn[at + 1];
        else if (event.key === 'ArrowUp') next = at > 0 ? inColumn[at - 1] : search;
        else if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') { const c = columns[columns.indexOf(column) + (event.key === 'ArrowRight' ? 1 : -1)]; const list = c ? [...c.querySelectorAll('[data-fx-choose]')] : []; next = list[Math.min(at, list.length - 1)]; }
        if (next) { event.preventDefault(); event.stopPropagation(); next.focus(); }
      });
    }

    // Module: page tabs and knobs.
    const plugin = current();
    if (!plugin) return;
    const { ctl } = moduleView(owner, plugin);
    for (const tab of root.querySelectorAll('[data-fx-page]')) {
      tab.addEventListener('click', () => { ctl?.setPage(Number(tab.dataset.fxPage)); refresh(tab.id); });
      tab.addEventListener('keydown', event => {
        if (!['ArrowLeft', 'ArrowRight', 'ArrowDown', 'ArrowUp'].includes(event.key)) return;
        event.preventDefault(); event.stopPropagation();
        if (event.key === 'ArrowDown') { root.querySelector('[data-fx-dial]')?.focus(); return; }
        if (event.key === 'ArrowUp') { root.querySelector('.fx-tile.is-selected')?.focus(); return; }
        const tabsList = [...root.querySelectorAll('[data-fx-page]')], next = clamp(tabsList.indexOf(tab) + (event.key === 'ArrowRight' ? 1 : -1), 0, tabsList.length - 1);
        ctl?.setPage(next); refresh(`${prefix}-page-${next}`);
      });
    }
    for (const node of root.querySelectorAll('[data-fx-dial]')) bindDial(node, Number(node.dataset.fxDial), ctl, owner, options);
    local.poll = setInterval(() => { if (!root.isConnected) { clearInterval(local.poll); local.poll = null; return; } paint(); }, 750);
    paint();

    function tileKey(event, node) {
      if (event.metaKey || event.ctrlKey || event.isComposing) return;
      const all = [...root.querySelectorAll('.fx-tiles > .fx-tile')], at = all.indexOf(node), plugin = byTile(node);
      const stop = () => { event.preventDefault(); event.stopPropagation(); };
      if (event.altKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight') && plugin) {
        stop(); const order = chain(), index = order.indexOf(plugin), target = order[index + (event.key === 'ArrowRight' ? 1 : -1)];
        if (target) move(owner, options, plugin, target.slot); return;
      }
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key) && !event.altKey && !event.shiftKey) {
        stop(); const next = all[event.key === 'Home' ? 0 : event.key === 'End' ? all.length - 1 : clamp(at + (event.key === 'ArrowRight' ? 1 : -1), 0, all.length - 1)];
        if (!next) return;
        const target = byTile(next);
        if (target && target.instanceID !== current()?.instanceID) select(owner, target, options);
        else if (next.dataset.fxTile === undefined && next.dataset.fxAction !== 'add' && next.click && next !== node) { next.click(); document.getElementById(next.id)?.focus(); }
        else next.focus();
        return;
      }
      if (event.key === 'ArrowDown' && !event.altKey && plugin) { stop(); (root.querySelector('[data-fx-page].is-on') || root.querySelector('[data-fx-dial]'))?.focus(); return; }
      if (!plugin || event.altKey) return;
      const key = event.key.toLowerCase();
      if (key === 'b') { stop(); act(owner, options, 'bypass', plugin); }
      else if (key === 'd' && !event.shiftKey) { stop(); act(owner, options, 'duplicate', plugin); }
      else if (event.key === 'Backspace' || event.key === 'Delete') { stop(); act(owner, options, 'remove', plugin); }
      else if (event.key === 'Enter') { stop(); root.querySelector('[data-fx-dial]')?.focus(); }
    }
  }
  /** One rotary knob: vertical drag (⇧ or ⌥ = fine), wheel, arrows, double-click / ⌫ reset, A automate. */
  function bindDial(node, index, ctl, owner, options) {
    const knob = () => ctl?.view?.()?.knobs?.[index] || null;
    const steps = () => Number(knob()?.stepCount) > 0 ? Number(knob().stepCount) : 0;
    const nudge = delta => { const k = knob(); if (!k?.writable) return; ctl.adjust(index, delta); paint(); };
    const setTo = value => { const k = knob(); if (!k?.writable) return; const target = steps() ? Math.round(clamp(value, 0, 1) * steps()) / steps() : clamp(value, 0, 1); if (Math.abs(target - (k.value ?? 0)) > 1e-6) ctl.adjust(index, target - (k.value ?? 0)); paint(); };
    node.addEventListener('pointerdown', event => {
      if (event.button !== 0 || !knob()?.writable) return;
      event.preventDefault(); node.focus({ preventScroll: true });
      try { node.setPointerCapture(event.pointerId); } catch {}
      const start = { y: event.clientY, value: knob()?.value ?? 0, travel: 0 };
      let lastY = event.clientY;
      node.classList.add('is-turning');
      const move = e => {
        // 160 px for the full range, ⇧ / ⌥ ten times finer; the reference moves so modifiers can change mid-drag.
        const speed = (e.shiftKey || e.altKey ? .1 : 1) / 160;
        start.travel += (lastY - e.clientY) * speed; lastY = e.clientY;
        setTo(start.value + start.travel);
      };
      const up = () => { node.classList.remove('is-turning'); node.removeEventListener('pointermove', move); node.removeEventListener('pointerup', up); node.removeEventListener('pointercancel', up); ctl.finish(); };
      node.addEventListener('pointermove', move); node.addEventListener('pointerup', up); node.addEventListener('pointercancel', up);
    });
    node.addEventListener('dblclick', event => { event.preventDefault(); if (ctl.reset(index) === null) S().toast('This parameter has no default.', 'info'); paint(); });
    node.addEventListener('wheel', event => { if (!knob()?.writable) return; event.preventDefault(); const dir = event.deltaY < 0 ? 1 : -1; nudge(steps() ? dir / steps() : dir * (event.shiftKey ? .001 : .01)); }, { passive: false });
    node.addEventListener('keydown', event => {
      if (event.metaKey || event.ctrlKey || event.isComposing) return;
      const stop = () => { event.preventDefault(); event.stopPropagation(); };
      const root = node.closest('[data-fx-root]'), dials = [...root.querySelectorAll('[data-fx-dial]')], at = dials.indexOf(node);
      if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)) {
        stop(); const dir = event.key === 'ArrowUp' || event.key === 'PageUp' ? 1 : -1, count = steps();
        nudge(count ? dir * (event.key.startsWith('Page') ? Math.max(1, Math.round(count / 4)) : 1) / count : dir * (event.key.startsWith('Page') ? .1 : event.shiftKey || event.altKey ? .001 : .01));
        return;
      }
      if (event.shiftKey && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) {
        stop(); ctl.finish(); const view = ctl.view(); if (!view) return; const page = clamp(view.page + (event.key === 'ArrowRight' ? 1 : -1), 0, view.pages.length - 1);
        if (page !== view.page) { ctl.setPage(page); refresh(); root.ownerDocument?.querySelector?.('[data-fx-root] [data-fx-dial]')?.focus(); } return;
      }
      if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { stop(); ctl.finish(); dials[clamp(at + (event.key === 'ArrowRight' ? 1 : -1), 0, dials.length - 1)]?.focus(); return; }
      if (event.key === 'Home' || event.key === 'End') { stop(); setTo(event.key === 'Home' ? 0 : 1); ctl.finish(); return; }
      if (event.key === 'Backspace' || event.key === 'Delete') { stop(); ctl.reset(index); paint(); return; }
      if (event.key === 'Escape' || (event.key === 'Tab' && event.shiftKey && at === 0)) { if (event.key === 'Escape') { stop(); root.querySelector('.fx-tile.is-selected')?.focus(); } return; }
      if (event.key.toLowerCase() === 'a' && !event.altKey && !event.shiftKey) {
        stop(); ctl.finish(); const cell = node.closest('[data-fx-knob]'), k = knob();
        if (!cell?.dataset?.automate) { S().toast('This parameter cannot be automated.', 'info'); return; }
        window.STPluginPages?.flushEdits?.().then(ok => { if (ok !== false) S().automate(cell.dataset.owner, cell.dataset.automate, cell.dataset.name, k?.value ?? 0); });
      }
    });
  }

  function event(type) {
    if (type === 'project.loaded') { local.selected.clear(); local.adding = null; }
  }
  window.STInsertChain = { render, bind, event, _test: { local, arc, point, selectedOf } };
})();
