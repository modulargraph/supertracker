/* Maschine-style kit browser: Library / My kits → Product (expansion) → Bank → Sub-bank, Type / Sub-type / Character,
   expansion artwork, group audio previews and a 4 × 4 pad map. Rendered inside the Library (STBrowser) as its "Kits"
   tab and shares the NKS browser's artwork index and styling. Sound routing belongs to the imported kit, never to the
   lane used to browse it; auditioning never imports. Every action is reachable by keyboard. */
(() => {
  'use strict';
  const S = () => window.ST;
  const html = value => S().escapeHTML(String(value ?? ''));
  const SEPARATOR = '\u001f';
  const LIMIT = 200;
  const blankFilters = () => ({ product: null, bank: null, subBank: null, types: new Set(), subtypes: new Set(), characters: new Set(), selected: 0 });
  const local = {
    user: false, query: '', favorites: false, autoPreview: true, filters: { library: blankFilters(), user: blankFilters() },
    items: [], total: 0, facets: { products: [], banks: [], subBanks: [], types: [], subtypes: [], characters: [] }, counts: { library: 0, user: 0 },
    status: {}, loaded: false, searching: false, error: '', generation: 0, resultsKey: null, appending: false, timer: null, refreshTimer: null, refreshNeeded: false, lastRevision: null,
    chipFocus: {}, moreCharacters: false, typeahead: '', typeaheadTimer: null, busy: false, helpOpen: false,
    metadata: new Map(), pendingMetadata: new Map(), detailTimer: null,
    previewPath: '', previewPending: false, previewGeneration: 0, previewTimer: null, previewEnd: null,
    art: { tile: new Map(), banner: new Map(), logo: new Map() }, artPending: new Set(), artTimer: null };
  const filters = () => local.filters[local.user ? 'user' : 'library'];
  const mounted = () => !!document.querySelector?.('.kitb-layout');
  const resultsKey = () => JSON.stringify(payload());
  const resultsCurrent = () => local.resultsKey === resultsKey();
  const current = () => resultsCurrent() ? local.items[filters().selected] : undefined;
  const metadata = item => item ? local.metadata.get(item.path) : undefined;
  const canImport = item => !!item && item.available !== false && !metadata(item)?.error && metadata(item)?.compatibility?.loadable !== false && item.compatibility?.loadable !== false;
  const previewPath = item => item?.previewPath || metadata(item)?.previewPath || '';
  const previewActive = () => !!(local.previewPath || local.previewPending || local.previewTimer !== null);
  const offlineMessage = item => `The location for ${item.name} is unavailable${item.volume ? ` (“${item.volume}”)` : ''}. Reconnect its drive to preview or load the kit.`;
  const plural = (count, word) => `${Number(count || 0).toLocaleString()} ${word}${count === 1 ? '' : 's'}`;
  const subtypeLabel = value => String(value).split(SEPARATOR).pop();
  const typeText = item => (item.subtypes || []).length ? item.subtypes.map(subtypeLabel).join(', ') : (item.types || []).join(', ');
  // NI headers sometimes misspell the author ("Naitve Instruments"); letters in another order are the same name.
  const sameName = (a, b) => [...String(a).toLowerCase()].sort().join('') === [...String(b || '').toLowerCase()].sort().join('');
  const chain = item => [item.product, item.bank, item.subBank].filter(Boolean);
  const allLabel = () => local.user ? 'All my kits' : 'All expansions';

  // ---- Data ---------------------------------------------------------------------------------
  function payload(offset = 0) {
    const value = filters();
    return { user: local.user, query: local.query, ...(value.product !== null ? { product: value.product } : {}), ...(value.product !== null && value.bank !== null ? { bank: value.bank } : {}),
      ...(value.product !== null && value.bank !== null && value.subBank !== null ? { subBank: value.subBank } : {}),
      types: [...value.types], subtypes: [...value.subtypes], characters: [...value.characters], favorites: local.favorites, offset, limit: LIMIT };
  }
  async function search({ append = false, keep = true } = {}) {
    if (append && (local.appending || !resultsCurrent() || local.items.length >= local.total)) return;
    clearTimeout(local.timer); local.timer = null; clearTimeout(local.refreshTimer); local.refreshTimer = null;
    const generation = ++local.generation, previous = current()?.path, key = resultsKey(), revision = local.lastRevision;
    local.searching = true; local.appending = append; local.refreshNeeded = false;
    let result;
    // A refresh of the results on screen reloads every page already shown, so the selection and scroll position stay.
    const limit = !append && keep && local.resultsKey === key ? Math.min(1000, Math.max(LIMIT, local.items.length)) : LIMIT;
    try { result = await S().request('library.kits.browse', { ...payload(append ? local.items.length : 0), limit }); if (!result || typeof result !== 'object') throw new Error('Kit browsing returned no results. Retry the search.'); }
    catch (error) {
      if (generation !== local.generation || key !== resultsKey()) return;
      local.searching = false; local.appending = false; local.resultsKey = null; local.error = error?.message || 'Could not browse kits.'; paint(); return;
    }
    if (generation !== local.generation || key !== resultsKey()) return;
    local.searching = false; local.appending = false;
    // Offsets belong to one header revision; restart instead of joining shifted pages.
    if (append && revision != null && result.status?.revision != null && revision !== result.status.revision) return search({ keep: true });
    local.error = ''; local.loaded = true; local.resultsKey = key;
    const items = (Array.isArray(result.items) ? result.items : []).filter(item => item?.path);
    local.items = append ? [...local.items, ...items.filter(item => !local.items.some(existing => existing.path === item.path))] : items;
    local.total = Math.max(0, Number(result.total) || 0);
    for (const name of Object.keys(local.facets)) local.facets[name] = Array.isArray(result[name]) ? result[name] : [];
    local.counts = result.counts || local.counts;
    local.status = { ...(result.status || {}), scanning: !!result.scanning, errors: result.errors || [], kitIndexUpgradePending: !!result.kitIndexUpgradePending,
      kitIndexUpgradeScanning: !!result.kitIndexUpgradeScanning, kitIndexUpgradeOfflineRoots: Array.isArray(result.kitIndexUpgradeOfflineRoots) ? result.kitIndexUpgradeOfflineRoots : [] };
    local.lastRevision = result.status?.revision ?? null;
    if (!append) {
      const index = keep && previous ? local.items.findIndex(item => item.path === previous) : -1;
      filters().selected = index >= 0 ? index : keep ? Math.min(filters().selected, Math.max(0, local.items.length - 1)) : 0;
      if (previous && previous !== current()?.path) stopPreview();
    }
    paint();
    if (!append) { revealSelection(); document.getElementById(`kitb-product-${productIndex()}`)?.scrollIntoView?.({ block: 'nearest' }); }
    queueDetail(); queuePrefetch();
    if (local.refreshNeeded) refreshSoon();
  }
  function refreshSoon(delay = 350) {
    local.refreshNeeded = true;
    if (!mounted() || local.timer !== null || local.refreshTimer !== null || local.searching) return;
    local.refreshTimer = setTimeout(() => { local.refreshTimer = null; search(); }, delay);
  }
  function changed(delay = 0) {
    // Invalidate before the debounce: old replies and actions must not belong to the new filter.
    local.generation++; local.resultsKey = null; local.appending = false; local.searching = true; local.error = '';
    filters().selected = 0; clearTimeout(local.detailTimer); local.detailTimer = null; stopPreview();
    clearTimeout(local.timer); local.timer = setTimeout(() => { local.timer = null; search({ keep: false }); }, delay);
    paint(); window.STBrowser?.saveState?.();
  }

  // ---- Settings -----------------------------------------------------------------------------
  const strings = (value, limit = 128) => Array.isArray(value) ? [...new Set(value.filter(item => typeof item === 'string' && item && item.length <= 2048))].slice(0, limit) : [];
  const text = value => typeof value === 'string' && value.length <= 2048 ? value : null;
  function settings() {
    const value = filters();
    return { query: local.query, user: local.user, userOnly: local.user, product: value.product, bank: value.bank, subBank: value.subBank, types: [...value.types], subtypes: [...value.subtypes],
      characters: [...value.characters], favorites: local.favorites, autoPreview: local.autoPreview };
  }
  // Also accepts the earlier { filters: { expansions, tags }, userOnly } shape.
  function restoreSettings(value) {
    if (!value || typeof value !== 'object') return;
    local.query = typeof value.query === 'string' ? value.query.slice(0, 1024) : '';
    local.user = value.user === true || value.userOnly === true;
    const next = blankFilters();
    next.product = text(value.product) ?? strings(value.filters?.expansions)[0] ?? null;
    next.bank = next.product !== null ? text(value.bank) : null; next.subBank = next.bank !== null ? text(value.subBank) : null;
    next.types = new Set(strings(value.types)); next.subtypes = new Set(strings(value.subtypes)); next.characters = new Set(strings(value.characters ?? value.filters?.tags));
    local.filters[local.user ? 'user' : 'library'] = next;
    local.favorites = value.favorites === true;
    if (typeof value.autoPreview === 'boolean') local.autoPreview = value.autoPreview;
    local.resultsKey = null; local.loaded = false;
    if (mounted()) changed();
  }

  // ---- Artwork ------------------------------------------------------------------------------
  // Expansion pictures come from the NKS artwork index (library.nks.artwork) as bounded data URLs keyed by folder.
  // User kits and expansions without pictures get their colour, or a stable hue from the name, with initials.
  const ART_VARIANTS = ['tile', 'banner', 'logo'];
  const artURL = (key, variant = 'tile') => { const value = key ? local.art[variant]?.get(key) : ''; return typeof value === 'string' && value.startsWith('data:image/') ? value : ''; };
  const initials = name => String(name || '').replace(/[^\p{L}\p{N} ]/gu, ' ').split(/\s+/).filter(Boolean).slice(0, 2).map(word => word[0]).join('').toUpperCase() || '◈';
  const safeColor = value => /^#[0-9a-f]{6}$/i.test(value || '') ? value : '';
  function nameColor(name) {
    let hash = 0; for (const char of String(name || '')) hash = (hash * 31 + char.codePointAt(0)) >>> 0;
    const hue = hash % 360, [s, l] = [0.42, 0.36], k = n => (n + hue / 30) % 12, a = s * Math.min(l, 1 - l);
    const channel = n => Math.round(255 * (l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1))))).toString(16).padStart(2, '0');
    return `#${channel(0)}${channel(8)}${channel(4)}`;
  }
  function art(key, name, { variant = 'tile', color = '', className = '' } = {}) {
    const url = artURL(key, variant), tint = safeColor(color) || (key ? '' : nameColor(name));
    const style = [tint ? `--nksb-tint:${tint}` : '', url ? `background-image:url('${url}')` : ''].filter(Boolean).join(';');
    return `<span class="nksb-art ${className} ${url ? 'has-art' : ''}" aria-hidden="true"${key ? ` data-kitb-art="${html(key)}" data-kitb-variant="${variant}"` : ''}${style ? ` style="${style}"` : ''}><span>${html(initials(name))}</span></span>`;
  }
  function queueArt() {
    if (local.artTimer !== null) return;
    local.artTimer = setTimeout(async () => {
      local.artTimer = null;
      const wanted = { tile: new Set(), banner: new Set(), logo: new Set() };
      for (const element of document.querySelectorAll?.('[data-kitb-art]') || []) {
        const key = element.dataset.kitbArt, variant = ART_VARIANTS.includes(element.dataset.kitbVariant) ? element.dataset.kitbVariant : 'tile';
        if (key && !local.art[variant].has(key) && !local.artPending.has(variant + '#' + key)) wanted[variant].add(key);
      }
      await Promise.all(ART_VARIANTS.filter(variant => wanted[variant].size).map(async variant => {
        const keys = [...wanted[variant]].slice(0, 200);
        keys.forEach(key => local.artPending.add(variant + '#' + key));
        try {
          const images = (await S().request('library.nks.artwork', { keys, variant }))?.images || {};
          for (const key of keys) local.art[variant].set(key, typeof images[key] === 'string' ? images[key] : '');
        } catch { for (const key of keys) local.art[variant].set(key, ''); }
        finally { keys.forEach(key => local.artPending.delete(variant + '#' + key)); }
      }));
      applyArt();
    }, 0);
  }
  function applyArt() {
    if (!mounted()) return;
    for (const element of document.querySelectorAll?.('[data-kitb-art]') || []) {
      const url = artURL(element.dataset.kitbArt, element.dataset.kitbVariant || 'tile');
      if (url && !element.classList?.contains('has-art')) { element.style.backgroundImage = `url('${url}')`; element.classList?.add('has-art'); }
    }
  }

  // ---- Rendering ----------------------------------------------------------------------------
  function productList() {
    const value = filters(), list = [...local.facets.products];
    if (value.product !== null && !list.some(product => product.name === value.product)) list.unshift({ name: value.product, vendor: '', count: 0 });
    return list;
  }
  const productIndex = () => { const product = filters().product; return product === null ? 0 : productList().findIndex(item => item.name === product) + 1; };
  const chosenProduct = () => { const product = filters().product; return product === null ? null : productList().find(item => item.name === product) || { name: product }; };
  function renderProducts() {
    const list = productList(), active = productIndex(), total = local.facets.products.reduce((sum, product) => sum + Number(product.count || 0), 0);
    const row = (index, product, detail, count, title) => `<div role="option" id="kitb-product-${index}" class="nksb-product kitb-product ${index === active ? 'selected' : ''}" data-kitb-product="${index}" aria-selected="${index === active}" title="${html(title)}">${index === 0 ? `<span class="nksb-art nksb-art-all" aria-hidden="true"><span>${local.user ? '☰' : '◈'}</span></span>` : art(product.art, product.name, { color: product.color })}<span class="nksb-product-name">${html(index === 0 ? allLabel() : product.name)}</span><small>${html(detail)}</small><span class="nksb-count">${Number(count).toLocaleString()}</span></div>`;
    return `<div id="kitb-products" class="nksb-product-list kitb-product-list" role="listbox" tabindex="0" aria-label="${local.user ? 'Your kit folders' : 'Expansions'}" aria-activedescendant="kitb-product-${active}">${row(0, null, local.user ? 'Every saved kit' : 'Every expansion', total, local.user ? 'Show all your kits' : 'Show every expansion')}${list.map((product, index) => row(index + 1, product, local.user ? (product.name === 'Saved kits' ? 'Kits folder' : 'Sub-folder') : product.name === 'Maschine' ? 'Factory library' : product.vendor && product.vendor !== 'Native Instruments' ? product.vendor : 'Expansion', product.count, `${product.name}${product.vendor ? ` · ${product.vendor}` : ''}`)).join('')}${local.loaded && !list.length ? `<p class="nksb-filter-empty nksb-products-empty">${local.user ? 'No saved kits yet.' : 'No expansions match.'}</p>` : ''}</div>`;
  }
  const singles = { banks: 'bank', subBanks: 'subBank' };
  function chips(group, label, options, { single = false, labelOf = option => option.name, limit = 0, tiles = false } = {}) {
    const value = filters(), selected = singles[group] ? new Set(value[singles[group]] === null ? [] : [value[singles[group]]]) : value[group];
    let list = [...options];
    for (const name of selected) if (!list.some(option => option.name === name)) list.unshift({ name, count: 0 });
    const hidden = limit && !local.moreCharacters ? Math.max(0, list.length - limit) : 0;
    if (hidden) list = list.slice(0, limit);
    if (!list.length) return '';
    const focus = list.some(option => option.name === local.chipFocus[group]) ? local.chipFocus[group] : list[0].name;
    const buttons = list.map(option => {
      const on = selected.has(option.name), name = option.name === '' ? (group === 'banks' ? 'No bank' : 'None') : labelOf(option);
      const count = option.count != null ? `<small>${Number(option.count).toLocaleString()}</small>` : '';
      const body = tiles ? `${art(option.art || chosenProduct()?.art, name, { color: option.color || chosenProduct()?.color, className: option.art ? '' : 'nksb-art-fallback' })}<span class="nksb-tile-name">${html(name)}</span>${count}` : `${html(name)}${count}`;
      return `<button type="button" id="kitb-chip-${group}-${encodeURIComponent(option.name).replace(/[^A-Za-z0-9_-]/g, '_')}" class="nksb-chip ${tiles ? 'nksb-tile' : ''} ${on ? 'chosen' : ''}" role="${single ? 'radio' : 'checkbox'}" aria-checked="${on}" tabindex="${option.name === focus ? 0 : -1}" data-kitb-chip="${group}" data-kitb-value="${html(option.name)}" title="${html(option.type ? `${option.type} › ${name}` : name)}">${body}</button>`;
    }).join('');
    const more = limit && (hidden || local.moreCharacters) ? `<button type="button" class="nksb-chip nksb-more" id="kitb-more-${group}" data-kitb-chip="${group}" data-kitb-more="true" tabindex="-1">${local.moreCharacters ? 'Fewer' : `${hidden} more…`}</button>` : '';
    return `<div class="nksb-chip-group ${tiles ? 'nksb-tiles' : ''}" data-kitb-group="${group}" role="group" aria-label="${html(label)}${single ? '' : ', multiple choice'}"><span class="nksb-chip-label">${html(label)}${selected.size ? `<small>${selected.size}</small>` : ''}</span><div class="nksb-chips">${buttons}${more}</div></div>`;
  }
  // Product › Bank › Sub-bank, each level clickable to step back out (Backspace does the same).
  function renderCrumbs() {
    const value = filters(), levels = [[allLabel(), 'all']];
    if (value.product !== null) levels.push([value.product, 'product']);
    if (value.product !== null && value.bank !== null) levels.push([value.bank || 'No bank', 'bank']);
    if (value.product !== null && value.bank !== null && value.subBank !== null) levels.push([value.subBank || 'None', 'subBank']);
    return `<nav class="nksb-crumbs" aria-label="Bank chain">${levels.map(([name, level], index) => index === levels.length - 1 ? `<span aria-current="location">${html(name)}</span>` : `<button type="button" id="kitb-crumb-${level}" data-kitb-crumb="${level}" tabindex="-1">${html(name)}</button><i aria-hidden="true">›</i>`).join('')}</nav>`;
  }
  function renderHero() {
    const value = filters(), product = chosenProduct();
    if (!product) return '';
    const bank = value.bank !== null ? local.facets.banks.find(option => option.name === value.bank) : null;
    const key = bank?.art || product.art, count = local.loaded && resultsCurrent() ? plural(local.total, local.user ? 'kit' : 'group') : '';
    const tint = safeColor(bank?.color || product.color) || (key ? '' : nameColor(product.name));
    return `<header class="nksb-hero kitb-hero" style="${tint ? `--nksb-tint:${tint}` : ''}">${art(key, product.name, { variant: 'banner', color: tint, className: 'nksb-hero-art' })}<div class="nksb-hero-copy">${renderCrumbs()}<h2>${html(value.bank && bank?.art ? value.bank : product.name)}</h2><p>${html([value.bank && !bank?.art ? value.bank : '', product.vendor, count].filter(Boolean).join(' · '))}</p></div></header>`;
  }
  function renderFilters() {
    const value = filters(), banks = local.facets.banks, tiles = banks.some(option => option.art);
    const groups = [
      value.product !== null && banks.length > 1 || value.bank !== null ? chips('banks', local.user ? 'Folder' : 'Bank', banks, { single: true, tiles }) : '',
      value.product !== null && value.bank !== null && (local.facets.subBanks.length || value.subBank !== null) ? chips('subBanks', 'Sub-bank', local.facets.subBanks, { single: true }) : '',
      local.facets.types.length > 1 || value.types.size ? chips('types', 'Type', local.facets.types) : '',
      chips('subtypes', 'Sub-type', local.facets.subtypes, { labelOf: option => option.label || subtypeLabel(option.name) }),
      chips('characters', 'Character', local.facets.characters, { limit: 16 })].filter(Boolean).join('');
    return groups ? `<div class="nksb-filters kitb-filters" aria-label="Kit filters">${groups}</div>` : '';
  }
  function activeFilterCount() { const value = filters(); return (value.product !== null) + (value.bank !== null) + (value.bank !== null && value.subBank !== null) + value.types.size + value.subtypes.size + value.characters.size + local.favorites + !!local.query.trim(); }
  const skeleton = () => Array.from({ length: 9 }, (_, index) => `<div class="nksb-row kitb-row nksb-skeleton" aria-hidden="true" style="--delay:${index * 60}ms"><span></span><span class="nksb-art"></span><div class="nksb-row-name"><strong></strong><small></small></div><span class="nksb-row-type"></span><span></span></div>`).join('');
  function renderEmpty() {
    const roots = S().state.library?.roots || [], status = local.status, updating = status.scanning || status.kitIndexUpgradeScanning;
    const offline = roots.some(root => root.available === false) || !!status.kitIndexUpgradeOfflineRoots?.length, knownRoots = roots.length > 0 || !!status.kitIndexUpgradePending;
    if (activeFilterCount()) return '<div class="browser-empty"><h3>No matching groups or kits</h3><p>Try a shorter search or remove a filter.</p><div class="kitb-empty-actions"><button class="btn" id="kitb-clear-empty">Clear search &amp; filters</button></div></div>';
    if (local.user) return '<div class="browser-empty"><h3>No saved kits yet</h3><p>Save a kit from the Kits view and it appears here, or add a folder that holds .stkit kits or your own Maschine groups.</p><div class="kitb-empty-actions"><button class="btn primary" id="kitb-new-empty">New empty kit…</button><button class="btn" id="kitb-add-empty">Add group / kit folder…</button></div></div>';
    const title = updating ? 'Finding Maschine groups…' : knownRoots ? 'No groups indexed yet' : 'Find your Maschine groups';
    const explanation = updating ? 'Discovering Maschine groups in your existing library folders. Results appear as the scan progresses.' : knownRoots ? `Rescan your existing library to find Maschine groups (.mxgrp).${offline ? ' Reconnect unavailable drives first.' : ''} Add another folder only if its expansion is stored elsewhere.` : 'Add your Native Access Content folder or an expansion’s Groups folder. Saved SuperTracker kits appear under My kits.';
    return `<div class="browser-empty"><h3>${title}</h3><p>${explanation}</p><div class="kitb-empty-actions">${knownRoots ? `<button class="btn primary" id="kitb-rescan-empty" ${updating ? 'disabled' : ''}>${updating ? 'Scanning library…' : 'Rescan library'}</button>` : ''}<button class="btn" id="kitb-add-empty">Add group / kit folder…</button></div></div>`;
  }
  function renderRows() {
    if (!resultsCurrent()) return local.error ? `<div class="browser-empty" role="status"><h3>Could not search kits</h3><p>${html(local.error)}</p><button class="btn" id="kitb-retry">Retry search</button></div>` : `<div role="status" class="nksb-loading"><span class="nksb-sr">Searching kits…</span>${skeleton()}</div>`;
    if (!local.items.length) return renderEmpty();
    const selected = filters().selected, product = filters().product;
    return local.items.map((item, index) => {
      const path = previewPath(item), playing = !!local.previewPath && local.previewPath === path;
      const where = [product === null ? item.product : '', item.bank, item.subBank].filter(Boolean).join(' › ');
      const detail = [where, item.user && item.extension === 'stkit' && item.soundCount != null ? plural(item.soundCount, 'sound') : ''].filter(Boolean).join(' · ');
      return `<div class="nksb-row kitb-row ${index === selected ? 'selected' : ''} ${item.available === false ? 'offline' : ''}" id="kitb-row-${index}" role="option" aria-selected="${index === selected}" data-kitb-row="${index}"><button type="button" class="nksb-play kitb-play ${playing ? 'playing' : ''}" data-kitb-play="${index}" tabindex="-1" ${path ? '' : 'disabled'} aria-label="${playing ? 'Stop group preview' : 'Group preview'}: ${html(item.name)}">${playing ? '■' : path ? '▶' : ''}</button>${art(item.art, item.product || item.name, { color: item.color, className: 'nksb-row-art' })}<div class="nksb-row-name"><strong>${html(item.name)}</strong><small>${item.available === false ? '<span class="browser-offline-badge">Offline</span> ' : ''}${html(detail || (item.user ? 'Your kit' : ''))}</small></div><span class="nksb-row-type">${html(typeText(item) || (item.extension === 'stkit' ? 'Kit' : 'Group'))}</span><button type="button" class="browser-star ${item.favorite ? 'is-favorite' : ''}" data-kitb-star="${index}" tabindex="-1" aria-label="${item.favorite ? 'Remove from' : 'Add to'} favorites: ${html(item.name)}">${item.favorite ? '★' : '☆'}</button></div>`;
    }).join('') + (local.items.length < local.total ? `<button type="button" class="nksb-more-rows" id="kitb-more-rows" tabindex="-1">Show more · ${(local.total - local.items.length).toLocaleString()} remaining</button>` : '');
  }
  function compatibility(item, meta) {
    const value = meta?.compatibility || item?.compatibility;
    const issues = [...(value?.warnings || []), ...(value?.missingPlugins || []).map(name => `Missing plug-in: ${name}`)];
    const summary = typeof value === 'string' ? value : value?.summary;
    if (!summary && !issues.length) return '';
    return `<div class="kitb-compatibility" role="status">${summary ? `<p>${html(summary)}</p>` : ''}${issues.length ? `<details><summary id="kitb-compatibility-details">${issues.length} import ${issues.length === 1 ? 'note' : 'notes'}</summary><ul>${issues.map(issue => `<li>${html(typeof issue === 'string' ? issue : issue.message || issue.name || '')}</li>`).join('')}</ul></details>` : ''}</div>`;
  }
  // Maschine numbers pads from the bottom left: the top row shows 13–16.
  function renderPads(item, meta) {
    const sounds = Array.isArray(meta?.sounds) ? meta.sounds : [];
    const slots = Array.from({ length: 16 }, (_, index) => {
      const slot = (3 - Math.floor(index / 4)) * 4 + index % 4;
      const sound = sounds.find(value => Number(value.slot ?? value.index) === slot) || (sounds[slot]?.slot == null && sounds[slot]?.index == null ? sounds[slot] : null);
      return { slot, sound: sound?.empty ? null : sound };
    });
    const filled = sounds.filter(sound => sound && !sound.empty).length;
    const count = sounds.length ? `${filled} / 16` : local.pendingMetadata.has(item.path) ? 'Reading…' : item.soundCount != null ? `${item.soundCount} / 16` : '16 pads';
    const tint = safeColor(item.color) || nameColor(item.product || item.name);
    return `<div class="kitb-sounds" style="--nksb-tint:${tint}"><h3>Pads <span>${count}</span></h3><ol class="kitb-pad-grid ${!sounds.length && local.pendingMetadata.has(item.path) ? 'reading' : ''}" aria-label="Kit pads, numbered from bottom left like Maschine">${slots.map(({ sound, slot }) => `<li value="${slot + 1}" class="${sound ? 'filled' : 'empty'} ${sound?.missing ? 'missing' : ''}" title="${html(sound ? [`Pad ${slot + 1}`, sound.name, sound.plugin || (sound.type && sound.type !== 'sample' ? sound.type : '')].filter(Boolean).join(' · ') : `Pad ${slot + 1} · empty`)}"><span>${slot + 1}</span><strong>${html(sound?.name || '')}</strong>${sound?.missing ? '<small>Missing</small>' : ''}</li>`).join('')}</ol>${meta?.error ? `<p class="kitb-filter-empty">${html(meta.error)}</p><button class="btn" id="kitb-detail-retry">Read details again</button>` : ''}</div>`;
  }
  function renderDetail() {
    const item = current();
    if (!item) return '<div class="browser-empty browser-detail-empty"><h3>Select a kit</h3><p>← → move between expansions, filters and kits<br>Space previews the group · Return adds the kit<br>Shift Return adds it and opens its sounds</p></div>';
    const meta = metadata(item), path = previewPath(item), playing = local.previewPath && local.previewPath === path;
    const blocked = !canImport(item), projectKits = window.STGroups?.list(S().state.project) || [], laneKit = window.STKitGroups?.cursorGroup?.();
    const tags = [...(item.types || []).filter(type => !(item.subtypes || []).some(sub => sub.startsWith(type + SEPARATOR))).map(value => ['types', value, value]),
      ...(item.subtypes || []).map(value => ['subtypes', value, subtypeLabel(value)]), ...(item.characters || []).map(value => ['characters', value, value])];
    const tint = safeColor(item.color) || (item.art ? '' : nameColor(item.product || item.name));
    return `<div class="nksb-detail-art kitb-detail-art" style="${tint ? `--nksb-tint:${tint}` : ''}">${art(item.art, item.art ? item.product : item.name, { variant: 'banner', color: tint, className: 'nksb-detail-banner' })}${item.productArt ? art(item.productArt, item.product, { variant: 'logo', className: 'nksb-detail-logo' }) : ''}</div>
      <div class="browser-detail-top"><span class="eyebrow">${item.extension === 'stkit' ? 'Your SuperTracker kit' : item.user ? 'Your Maschine group' : item.product === 'Maschine' ? 'Maschine factory group' : 'Maschine expansion group'}</span><h2>${html(item.name)}</h2><p class="nksb-detail-chain">${chain(item).map(html).join(' <i aria-hidden="true">›</i> ')}</p>${item.author || item.vendor ? `<p>${html([item.vendor, item.author && !sameName(item.author, item.vendor) ? item.author : ''].filter(Boolean).join(' · '))}</p>` : ''}</div>
      ${item.available === false ? `<p class="browser-small browser-detail-offline" role="status">${html(offlineMessage(item))}</p>` : ''}
      ${tags.length ? `<div class="browser-tags">${tags.map(([group, value, label], index) => `<button id="kitb-tag-${index}" data-kitb-tag="${group}" data-kitb-value="${html(value)}" title="Filter by ${html(label)}">${html(label)}</button>`).join('')}</div>` : ''}
      <div class="browser-detail-actions"><button type="button" class="btn" id="kitb-preview" ${path && item.available !== false || !path && item.available !== false && !meta ? '' : 'disabled'}>${playing ? '■ Stop group preview' : path ? '▶ Group preview' : meta ? 'No group preview' : '▶ Group preview'} <kbd>Space</kbd></button><button type="button" class="btn primary" id="kitb-load" ${local.busy || blocked ? 'disabled' : ''}>${local.busy ? 'Loading kit…' : 'Load new kit'} <kbd>↵</kbd></button><button type="button" class="btn" id="kitb-load-open" ${local.busy || blocked ? 'disabled' : ''}>Add and edit sounds <kbd>⇧ ↵</kbd></button>${projectKits.length ? `<label class="browser-small kitb-replace-target">Replace target <select id="kitb-replacement-target" ${local.busy ? 'disabled' : ''}><option value="">Choose kit (group)…</option>${projectKits.map(kit => `<option value="${html(kit.id)}" ${kit.id === laneKit?.id ? 'selected' : ''}>${html(kit.name)}</option>`).join('')}</select></label>` : ''}<button type="button" class="btn" id="kitb-replace" ${local.busy || blocked || !laneKit ? 'disabled' : ''}>Replace selected kit</button><p class="browser-small muted">${laneKit ? 'Replace ' + html(laneKit.name) + ' pad by pad, keeping its tracks, patterns and mixer routing.' : (projectKits.length ? 'Choose a kit above to replace it. ' : 'This session has no kit to replace yet. ') + 'Load new kit adds a group with one track per loaded pad.'}</p></div>
      ${compatibility(item, meta)}${renderPads(item, meta)}
      <div class="browser-path"><span class="eyebrow">File location</span><p title="${html(item.path)}">${html(item.path)}</p></div>`;
  }
  function renderStatus() {
    const status = local.status || {};
    const line = status.reading ? `<span class="browser-pulse"></span> Reading groups${status.pending ? ` ${Number(status.read || 0).toLocaleString()} / ${Number(status.pending).toLocaleString()}` : '…'}`
      : status.scanning || status.kitIndexUpgradeScanning ? '<span class="browser-pulse"></span> Scanning…' : `<span>${plural((local.counts.library || 0) + (local.counts.user || 0), 'kit')}</span>`;
    return `<div class="nksb-status kitb-status" role="status"><div class="browser-root-copy">${line}</div><span class="kitb-status-actions"><button class="browser-icon-btn" id="kitb-new" title="New empty kit…" aria-label="New empty kit">＋</button><button class="browser-icon-btn" id="kitb-add" title="Add group / kit folder…" aria-label="Add group or kit folder">⊕</button><button class="browser-icon-btn" id="kitb-scan" title="Rescan library" aria-label="Rescan library" ${status.scanning ? 'disabled' : ''}>↻</button></span></div>`;
  }
  function render() {
    const value = filters(), tabs = [['library', 'Library', local.counts.library], ['user', 'My kits', local.counts.user]];
    const showing = local.error ? 'Search failed' : !local.loaded || !resultsCurrent() ? 'Searching…' : plural(local.total, local.user ? 'kit' : 'group');
    return `<div class="kitb-layout">
      <aside id="kitb-sidebar" class="browser-sidebar panel nksb-sidebar kitb-sidebar" aria-label="Kit libraries and expansions">
        <div class="nksb-kinds kitb-kinds" role="tablist" aria-label="Kit content">${tabs.map(([kind, name, count]) => `<button type="button" role="tab" id="kitb-tab-${kind}" data-kitb-kind="${kind}" aria-selected="${local.user === (kind === 'user')}" tabindex="${local.user === (kind === 'user') ? 0 : -1}" class="${local.user === (kind === 'user') ? 'active' : ''}">${name}<small>${Number(count || 0).toLocaleString()}</small></button>`).join('')}</div>
        ${renderProducts()}
        ${renderStatus()}
      </aside>
      <section class="browser-results panel nksb-main kitb-main" aria-label="Kits">
        <div class="browser-search-wrap"><span aria-hidden="true">⌕</span><input id="kitb-search" class="browser-search" type="search" value="${html(local.query)}" placeholder="${local.user ? 'Search your kits…' : 'Search groups, expansions, types…'}" aria-label="Search groups and kits" autocomplete="off" spellcheck="false"><kbd>⌘ F</kbd></div>
        ${renderHero()}
        ${renderFilters()}
        <div class="browser-result-summary"><span role="status">${html(showing)}${activeFilterCount() ? ` <button type="button" id="kitb-clear" class="nksb-clear">Clear ${activeFilterCount() === 1 ? 'filter' : 'filters'}</button>` : ''}</span><span class="nksb-toggles"><details id="kitb-help" class="kitb-help" ${local.helpOpen ? 'open' : ''}><summary id="kitb-help-toggle">Where are my groups?</summary><p>Library lists the Maschine groups in your indexed folders by expansion, as in Maschine. If an expansion is stored elsewhere, use <strong>Add group / kit folder</strong> (⊕) and choose its Native Access Content or Groups folder, then rescan (↻). Saved SuperTracker kits are under <strong>My kits</strong>.</p></details><button type="button" id="kitb-favorites" class="nksb-toggle ${local.favorites ? 'chosen' : ''}" aria-pressed="${local.favorites}">${local.favorites ? '★' : '☆'} Favorites</button><label class="browser-audition-toggle"><input id="kitb-auto-preview" type="checkbox" ${local.autoPreview ? 'checked' : ''}> Auto preview</label></span></div>
        <div id="kitb-results" class="browser-result-list nksb-result-list kitb-result-list" tabindex="0" role="listbox" aria-label="Kits" aria-busy="${local.searching}" ${current() ? `aria-activedescendant="kitb-row-${value.selected}"` : ''}>${renderRows()}</div>
      </section>
      <aside id="kitb-detail" class="browser-detail panel nksb-detail kitb-detail" aria-label="Selected kit">${renderDetail()}</aside>
    </div>`;
  }
  const shortcuts = () => '<span><kbd>⌘ F</kbd> Search</span><span><kbd>← →</kbd> Expansions · kits · details</span><span><kbd>↑ ↓</kbd> Browse</span><span><kbd>Space</kbd> Group preview</span><span><kbd>↵</kbd> Add kit</span><span><kbd>⇧ ↵</kbd> Add & edit</span><span><kbd>F</kbd> Favorite</span><span><kbd>⌫</kbd> Up a level</span><span><kbd>Tab</kbd> Filters</span>';

  // ---- Painting -----------------------------------------------------------------------------
  // Repaints keep the focused control, caret and list scroll positions.
  function paint() {
    if (!mounted()) return;
    const focused = document.activeElement, id = focused?.id, start = focused?.selectionStart, end = focused?.selectionEnd;
    const scroll = ['kitb-results', 'kitb-products', 'kitb-detail'].map(key => [key, document.getElementById(key)?.scrollTop || 0]);
    S().refresh();
    for (const [key, value] of scroll) { const node = document.getElementById(key); if (node) node.scrollTop = value; }
    const next = id && document.getElementById(id);
    if (next && next !== document.activeElement && !next.disabled) next.focus({ preventScroll: true });
    else if (id?.startsWith('kitb-') && (!next || next.disabled)) {
      const sibling = focused?.dataset?.kitbChip && document.querySelector?.(`[data-kitb-group="${focused.dataset.kitbChip}"] [tabindex="0"]`);
      (sibling || document.getElementById('kitb-results'))?.focus({ preventScroll: true });
    }
    if (next && typeof start === 'number' && next.setSelectionRange) try { next.setSelectionRange(start, end); } catch {}
    queueArt();
  }
  function revealSelection() { document.getElementById(`kitb-row-${filters().selected}`)?.scrollIntoView?.({ block: 'nearest' }); }
  // Selection changes update only the rows and the detail panel, so arrows stay instant on long lists.
  function updateSelection(previous) {
    const list = document.getElementById('kitb-results'), detail = document.getElementById('kitb-detail');
    if (!list || !detail || !list.querySelector) { paint(); return; }
    const index = filters().selected;
    for (const row of [document.getElementById(`kitb-row-${previous}`), document.getElementById(`kitb-row-${index}`)]) {
      if (!row) continue;
      const on = Number(row.dataset.kitbRow) === index;
      row.classList?.toggle('selected', on); row.setAttribute?.('aria-selected', String(on));
    }
    list.setAttribute?.('aria-activedescendant', `kitb-row-${index}`);
    replaceDetail(); revealSelection();
  }
  function replaceDetail() {
    const detail = document.getElementById('kitb-detail');
    if (!detail) return;
    if (!detail.querySelector) { paint(); return; }
    const focused = document.activeElement, inside = focused?.closest?.('#kitb-detail') ? focused.id : null;
    detail.innerHTML = renderDetail(); bindDetail(); queueArt();
    if (inside) (document.getElementById(inside) || document.getElementById('kitb-results'))?.focus({ preventScroll: true });
  }
  function updatePreviewMarks() {
    if (!mounted()) return;
    let found = false;
    for (const row of document.querySelectorAll?.('[data-kitb-row]') || []) {
      const item = local.items[Number(row.dataset.kitbRow)], mark = row.querySelector?.('.kitb-play'); found = true;
      if (!item || !mark) continue;
      const path = previewPath(item), playing = !!local.previewPath && local.previewPath === path;
      mark.textContent = playing ? '■' : path ? '▶' : ''; mark.classList?.toggle('playing', playing);
    }
    if (found) replaceDetail(); else paint();
  }

  // ---- Actions ------------------------------------------------------------------------------
  function select(index, audition = true) {
    if (!resultsCurrent() || !local.items.length) return;
    const previous = filters().selected, next = Math.max(0, Math.min(index, local.items.length - 1));
    if (next !== previous) stopPreview({ quiet: true });
    filters().selected = next;
    if (next !== previous) updateSelection(previous); else revealSelection();
    if (index >= local.items.length - 1 && local.items.length < local.total) search({ append: true });
    queueDetail();
    const item = current();
    if (audition && local.autoPreview && item?.available !== false && next !== previous) {
      clearTimeout(local.previewTimer);
      local.previewTimer = setTimeout(() => { local.previewTimer = null; if (current()?.path === item.path && mounted()) preview(item); }, 110);
    }
    queuePrefetch();
  }
  function move(delta) {
    if (!resultsCurrent() || !local.items.length) return;
    select(filters().selected + delta);
  }
  // Background-decode group previews around the selection; kits without a known preview path resolve natively.
  const prefetchEntry = item => previewPath(item) ? { path: previewPath(item) } : item.available === false ? null : { kit: item.path };
  function queuePrefetch() {
    if (!local.autoPreview || !resultsCurrent() || !mounted()) return;
    window.STPreviewPrefetch?.schedule('kits', { items: local.items, selected: filters().selected, entry: prefetchEntry });
  }
  function setProduct(index, { immediate = false } = {}) {
    const list = productList(), next = Math.max(0, Math.min(index, list.length));
    const name = next === 0 ? null : list[next - 1].name;
    if (name === filters().product) return;
    Object.assign(filters(), { product: name, bank: null, subBank: null }); local.chipFocus.banks = undefined;
    changed(immediate ? 0 : 110);
  }
  function setUser(user) {
    user = !!user;
    if (user === local.user) return;
    stopPreview(); local.user = user; local.items = []; local.total = 0; local.loaded = false; local.moreCharacters = false;
    changed(); document.getElementById('kitb-products')?.scrollTo?.(0, 0);
  }
  function toggleChip(group, value) {
    const state = filters();
    if (group === 'banks') { state.bank = state.bank === value ? null : value; state.subBank = null; }
    else if (group === 'subBanks') state.subBank = state.subBank === value ? null : value;
    else if (state[group]) {
      if (state[group].has(value)) state[group].delete(value); else if (state[group].size < 128) state[group].add(value);
      if (group === 'types' && !state.types.has(value)) for (const sub of [...state.subtypes]) if (sub.startsWith(value + SEPARATOR)) state.subtypes.delete(sub);
    } else return;
    local.chipFocus[group] = value; changed();
  }
  // Sub-bank → bank → expansion → everything, like the hardware browser's back button.
  function up(level) {
    const state = filters();
    if (level === 'all' || (level === undefined && state.bank === null)) { if (state.product === null) return false; setProduct(0, { immediate: true }); return true; }
    if (level === 'product' || (level === undefined && state.subBank === null)) { if (state.bank === null) return false; state.bank = null; state.subBank = null; changed(); return true; }
    if (state.subBank === null) return false;
    state.subBank = null; changed(); return true;
  }
  // First clear keeps the chosen expansion; clearing again (or with only an expansion) shows every expansion.
  function clearFilters() {
    const state = filters(), onlyProduct = state.product !== null && activeFilterCount() === 1;
    local.filters[local.user ? 'user' : 'library'] = { ...blankFilters(), product: onlyProduct ? null : state.product };
    local.query = ''; local.favorites = false;
    changed();
  }
  async function inspect(item = current(), retry = false) {
    if (!item || item.available === false) return null;
    if (local.pendingMetadata.has(item.path)) return local.pendingMetadata.get(item.path);
    if (!retry && local.metadata.has(item.path)) return metadata(item);
    const pending = (async () => {
      let result;
      try { result = await S().request('kit.detail', { path: item.path }); }
      catch (error) { result = { error: error?.message || 'Could not read kit details.' }; }
      local.metadata.set(item.path, result || {});
      if (local.metadata.size > 256) local.metadata.delete(local.metadata.keys().next().value);
      local.pendingMetadata.delete(item.path);
      if (current()?.path === item.path && mounted()) replaceDetail();
      return result;
    })();
    local.pendingMetadata.set(item.path, pending);
    if (current()?.path === item.path && mounted()) replaceDetail();
    return pending;
  }
  function queueDetail() {
    clearTimeout(local.detailTimer);
    local.detailTimer = setTimeout(() => { local.detailTimer = null; if (mounted()) inspect(); }, 180);
  }
  async function preview(item = current(), toggle = false) {
    if (!item || !resultsCurrent() || current()?.path !== item.path) return;
    if (item.available === false) { if (toggle) S().toast(offlineMessage(item), 'info'); return; }
    if (toggle && local.previewPath && local.previewPath === previewPath(item)) return stopPreview();
    clearTimeout(local.previewTimer); local.previewTimer = null; clearTimeout(local.previewEnd);
    const generation = ++local.previewGeneration; local.previewPending = true;
    const path = previewPath(item) || (await inspect(item))?.previewPath;
    if (generation !== local.previewGeneration || current()?.path !== item.path || !mounted()) return;
    if (!path) { local.previewPending = false; if (toggle) S().toast('This kit has no group audio preview.', 'info'); updatePreviewMarks(); return; }
    try {
      const result = await S().request('sample.preview', { path });
      if (generation !== local.previewGeneration) return;
      local.previewPath = path;
      if (Number(result?.duration) > 0) local.previewEnd = setTimeout(() => {
        if (generation === local.previewGeneration) { local.previewPath = ''; local.previewEnd = null; updatePreviewMarks(); }
      }, Math.min(Number(result.duration) * 1000 + 80, 600000));
    } catch (error) { if (generation === local.previewGeneration && toggle) S().toast(error?.message || 'Group preview failed.', 'error'); }
    finally { if (generation === local.previewGeneration) { local.previewPending = false; updatePreviewMarks(); } }
  }
  async function stopPreview({ quiet = false } = {}) {
    const active = previewActive();
    local.previewGeneration++; clearTimeout(local.previewTimer); clearTimeout(local.previewEnd);
    local.previewTimer = null; local.previewEnd = null; local.previewPending = false; local.previewPath = '';
    if (active) { try { await S().request('sample.stopPreview', {}); } catch {} }
    if (active && !quiet) updatePreviewMarks();
  }
  async function load(mode = 'stay', expected = current()) {
    const item = current();
    if (!item || local.busy) return false;
    if (item.path !== expected?.path) { S().toast('The kit selection changed. Choose the kit again.', 'info'); return false; }
    if (item.available === false) { S().toast(offlineMessage(item), 'info'); return false; }
    if (!canImport(item)) { S().toast('This kit cannot be imported yet. Read its details or use Read details again.', 'info'); return false; }
    if (!window.STKitGroups?.loadKit) { S().toast('Kit loading is unavailable in this build.', 'error'); return false; }
    const project = S().state.project, key = resultsKey(), target = mode === 'replace' ? window.STKitGroups.cursorGroup() : null;
    if (mode === 'replace' && !target) { S().toast('Move the cursor to a group (kit) to replace it.', 'info'); return false; }
    local.busy = true; replaceDetail();
    try {
      await stopPreview();
      if (S().state.project !== project || key !== resultsKey() || current()?.path !== item.path || !mounted()) { S().toast('The project or kit selection changed. Add the kit again in the intended project.', 'info'); return false; }
      if (mode === 'replace' && window.STKitGroups.cursorGroup()?.id !== target.id) { S().toast('The replacement kit changed. Select it again.', 'info'); return false; }
      if (!canImport(item)) { S().toast('This kit cannot be imported yet. Read its details or use Read details again.', 'info'); return false; }
      return await window.STKitGroups.loadKit(item.path, { stay: mode !== 'open', ...(mode === 'replace' ? { replace: true } : {}) });
    } catch (error) { S().toast(error?.message || 'Could not add the kit.', 'error'); return false; }
    finally { local.busy = false; if (mounted()) paint(); }
  }
  async function favorite(item = current()) {
    if (!item) return;
    const value = !item.favorite;
    try {
      await S().request('library.favorite', { path: item.path, favorite: value }); item.favorite = value;
      if (local.favorites && !value) { stopPreview(); await search(); } else paint();
    } catch (error) { S().toast(error?.message || 'Could not update the favorite.', 'error'); }
  }
  async function rescan() {
    try { await S().request('library.scan', {}); local.metadata.clear(); await search(); }
    catch (error) { S().toast(error?.message || 'Could not scan kit locations.', 'error'); }
  }
  async function addRoot() {
    try { const result = await S().request('library.chooseRoot', {}); if (result?.cancelled) return; await search(); }
    catch (error) { S().toast(error?.message || 'Could not add the kit folder.', 'error'); }
  }

  // ---- Binding ------------------------------------------------------------------------------
  function bindDetail() {
    const on = (id, fn, type = 'click') => document.getElementById(id)?.addEventListener(type, fn);
    on('kitb-preview', () => preview(current(), true)); on('kitb-load', () => load()); on('kitb-replace', () => load('replace')); on('kitb-load-open', () => load('open'));
    on('kitb-detail-retry', () => inspect(current(), true));
    const boundProject = S().state.project;
    on('kitb-replacement-target', event => {
      if (local.busy || S().state.project !== boundProject) return;
      const members = window.STGroups?.members(boundProject, event.target.value) || [];
      if (members.length) { S().state.track = members[0]; S().state.instrument = boundProject.tracks[members[0]].instrument || 0; }
      replaceDetail();
    }, 'change');
    document.querySelectorAll('[data-kitb-tag]').forEach(button => button.addEventListener('click', () => {
      const group = button.dataset.kitbTag, value = button.dataset.kitbValue, state = filters();
      if (group === 'subtypes') state.types.add(value.split(SEPARATOR)[0]);
      if (!state[group]?.has(value)) toggleChip(group, value);
    }));
  }
  function bind() {
    const on = (id, fn, type = 'click') => document.getElementById(id)?.addEventListener(type, fn);
    document.querySelectorAll('[data-kitb-kind]').forEach(button => button.addEventListener('click', () => setUser(button.dataset.kitbKind === 'user')));
    document.querySelectorAll('[data-kitb-product]').forEach(row => {
      row.addEventListener('click', () => { setProduct(Number(row.dataset.kitbProduct), { immediate: true }); document.getElementById('kitb-products')?.focus({ preventScroll: true }); });
      row.addEventListener('dblclick', () => document.getElementById('kitb-results')?.focus());
    });
    document.querySelectorAll('[data-kitb-chip]').forEach(button => {
      button.addEventListener('click', () => { if (button.dataset.kitbMore) { local.moreCharacters = !local.moreCharacters; paint(); return; } toggleChip(button.dataset.kitbChip, button.dataset.kitbValue); });
      button.addEventListener('focus', () => {
        if (button.dataset.kitbMore) return;
        local.chipFocus[button.dataset.kitbChip] = button.dataset.kitbValue;
        button.closest?.('[data-kitb-group]')?.querySelectorAll('[data-kitb-chip]').forEach(chip => { chip.tabIndex = chip === button ? 0 : -1; });
      });
    });
    document.querySelectorAll('[data-kitb-row]').forEach(row => {
      row.addEventListener('click', event => { if (event.target?.closest?.('[data-kitb-star], [data-kitb-play]')) return; select(Number(row.dataset.kitbRow)); document.getElementById('kitb-results')?.focus({ preventScroll: true }); });
      row.addEventListener('dblclick', () => load());
    });
    document.querySelectorAll('[data-kitb-star]').forEach(button => button.addEventListener('click', event => { event.stopPropagation?.(); favorite(local.items[Number(button.dataset.kitbStar)]); }));
    document.querySelectorAll('[data-kitb-play]').forEach(button => button.addEventListener('click', event => {
      event.stopPropagation?.(); const index = Number(button.dataset.kitbPlay);
      if (filters().selected !== index) select(index, false);
      preview(local.items[index], true); document.getElementById('kitb-results')?.focus({ preventScroll: true });
    }));
    document.querySelectorAll('[data-kitb-crumb]').forEach(button => button.addEventListener('click', () => up(button.dataset.kitbCrumb)));
    on('kitb-search', event => { local.query = event.target.value; changed(120); }, 'input');
    on('kitb-favorites', () => { local.favorites = !local.favorites; changed(); });
    on('kitb-auto-preview', event => { local.autoPreview = event.target.checked; if (!local.autoPreview) stopPreview(); window.STBrowser?.saveState?.(); }, 'change');
    on('kitb-clear', clearFilters); on('kitb-clear-empty', clearFilters); on('kitb-retry', () => search());
    on('kitb-scan', rescan); on('kitb-rescan-empty', rescan); on('kitb-add', addRoot); on('kitb-add-empty', addRoot);
    on('kitb-new', () => window.STKitGroups?.createKit?.()); on('kitb-new-empty', () => window.STKitGroups?.createKit?.());
    on('kitb-help', event => { local.helpOpen = event.target.open; }, 'toggle');
    on('kitb-more-rows', () => search({ append: true }));
    bindDetail();
    // A failed search stays failed until the user retries; retrying on every repaint would spin forever.
    if ((!local.loaded || local.refreshNeeded) && !local.searching && !local.error && local.timer === null && local.refreshTimer === null) queueMicrotask(() => search({ keep: local.loaded }));
  }

  // ---- Keyboard -----------------------------------------------------------------------------
  const groupOrder = () => Array.from(document.querySelectorAll('[data-kitb-group]'));
  function focusGroup(group, last = false) {
    const list = Array.from(group?.querySelectorAll('[data-kitb-chip]') || []);
    (list.find(chip => chip.tabIndex === 0 && !chip.dataset.kitbMore) || (last ? list.at(-1) : list[0]))?.focus();
  }
  const focusResults = () => document.getElementById('kitb-results')?.focus({ preventScroll: true });
  const focusProducts = () => document.getElementById('kitb-products')?.focus({ preventScroll: true });
  function focusSearch(selectAll = false) { const input = document.getElementById('kitb-search'); input?.focus({ preventScroll: true }); if (selectAll) input?.select?.(); }
  function typeahead(character) {
    clearTimeout(local.typeaheadTimer);
    local.typeahead += character.toLocaleLowerCase();
    local.typeaheadTimer = setTimeout(() => { local.typeahead = ''; }, 800);
    const list = productList(), value = local.typeahead;
    let index = list.findIndex(product => product.name.toLocaleLowerCase().startsWith(value));
    if (index < 0) index = list.findIndex(product => product.name.toLocaleLowerCase().includes(value));
    if (index >= 0) setProduct(index + 1);
  }
  function key(event) {
    if (!mounted() || event.isComposing) return false;
    const target = event.target, name = event.key, plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
    const handled = fn => { event.preventDefault(); fn?.(); return true; };
    if ((event.metaKey || event.ctrlKey) && name?.toLowerCase() === 'f') return handled(() => focusSearch(true));
    if (event.defaultPrevented && name !== 'Escape') return true;
    if (plain && name === 'Escape' && previewActive()) return handled(stopPreview);
    if (/INPUT|TEXTAREA|SELECT/.test(target?.tagName || '') || target?.isContentEditable) {
      if (target.type === 'checkbox' && (name === ' ' || event.code === 'Space')) return true;
      if (target.id === 'kitb-search' && plain) {
        if (name === 'ArrowDown' || name === 'Enter') return handled(() => { if (local.timer !== null) search({ keep: false }); focusResults(); });
        if (name === 'Escape') return handled(() => { if (local.query) { local.query = ''; target.value = ''; changed(); } else focusResults(); });
      }
      if (target.id === 'kitb-replacement-target' && plain && name === 'Escape') return handled(focusResults);
      return false;
    }
    if (event.metaKey || event.ctrlKey) return false;
    const vertical = { ArrowUp: -1, ArrowDown: 1, PageUp: -12, PageDown: 12 };
    // Library / My kits tabs.
    if (target?.closest?.('[data-kitb-kind]') && plain) {
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(name)) return handled(() => { const user = name === 'ArrowRight' || name === 'End'; setUser(user); document.getElementById(`kitb-tab-${user ? 'user' : 'library'}`)?.focus(); });
      if (name === 'ArrowDown') return handled(focusProducts);
      if (name === 'ArrowUp') return handled(() => document.getElementById('browser-tab-kits')?.focus());
      return name === 'Enter' || name === ' ';
    }
    // Expansion list.
    if (target?.id === 'kitb-products') {
      const index = productIndex(), size = productList().length;
      if (plain && (vertical[name] || name === 'Home' || name === 'End')) return handled(() => {
        if (name === 'ArrowUp' && index === 0) { document.getElementById(`kitb-tab-${local.user ? 'user' : 'library'}`)?.focus(); return; }
        setProduct(name === 'Home' ? 0 : name === 'End' ? size : index + vertical[name]);
        document.getElementById(`kitb-product-${productIndex()}`)?.scrollIntoView?.({ block: 'nearest' });
      });
      if (plain && (name === 'ArrowRight' || name === 'Enter')) return handled(() => { if (local.timer !== null) search({ keep: false }); focusResults(); });
      if (plain && (name === 'Escape' || name === 'Backspace')) return handled(() => setProduct(0, { immediate: true }));
      if (name === ' ' || event.code === 'Space') return handled(() => preview(current(), true));
      if (!event.altKey && name?.length === 1 && /\S/.test(name)) return handled(() => typeahead(name));
      return false;
    }
    // Filter chips: ← → within a group, ↑ ↓ between groups, Space / Return toggles.
    const chip = target?.closest?.('[data-kitb-chip]');
    if (chip && plain) {
      const group = chip.closest('[data-kitb-group]'), list = Array.from(group?.querySelectorAll('[data-kitb-chip]') || []), index = list.indexOf(chip);
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(name)) return handled(() => {
        if (name === 'ArrowLeft' && index === 0) { focusProducts(); return; }
        list[name === 'Home' ? 0 : name === 'End' ? list.length - 1 : Math.max(0, Math.min(list.length - 1, index + (name === 'ArrowRight' ? 1 : -1)))]?.focus();
      });
      if (name === 'ArrowUp' || name === 'ArrowDown') return handled(() => {
        const groups = groupOrder(), next = groups[groups.indexOf(group) + (name === 'ArrowDown' ? 1 : -1)];
        if (next) focusGroup(next); else if (name === 'ArrowDown') focusResults(); else focusSearch();
      });
      if (name === 'Backspace') return handled(() => up());
      return name === 'Enter' || name === ' ';
    }
    if (target?.closest?.('button, a, summary') && !target?.closest?.('#kitb-results') && (name === 'Enter' || name === ' ' || event.code === 'Space')) return true;
    if (plain && name === 'ArrowLeft' && target?.closest?.('.kitb-detail')) return handled(focusResults);
    const inResults = target?.id === 'kitb-results' || (!!target?.closest?.('#kitb-results') && !target.closest('button, a, summary'));
    if (!inResults) return false;
    const selected = filters().selected, total = local.items.length;
    if (plain && (vertical[name] || name === 'Home' || name === 'End')) return handled(() => {
      if (name === 'ArrowUp' && selected <= 0) { const groups = groupOrder(); if (groups.length) focusGroup(groups.at(-1)); else focusSearch(); return; }
      if (total) select(name === 'Home' ? 0 : name === 'End' ? total - 1 : selected + vertical[name]);
    });
    if (plain && name === 'ArrowLeft') return handled(focusProducts);
    if (plain && name === 'ArrowRight') return handled(() => { const node = document.getElementById('kitb-load'); (node && !node.disabled ? node : document.getElementById('kitb-preview'))?.focus(); });
    if (name === 'Enter' && !event.altKey) return handled(() => load(event.shiftKey ? 'open' : 'stay'));
    if (plain && (name === ' ' || event.code === 'Space')) return handled(() => preview(current(), true));
    if (plain && name?.toLowerCase() === 'f') return handled(() => favorite());
    if (plain && name === 'Backspace') return handled(() => up());
    return false;
  }

  // ---- Integration --------------------------------------------------------------------------
  function event(type, value = {}) {
    if (type === 'library.kits.progress') {
      local.status = { ...local.status, ...value };
      if (value.revision !== undefined && value.revision !== local.lastRevision) refreshSoon(value.reading ? 900 : 60);
      else if (mounted()) { const node = document.querySelector?.('.kitb-status'); if (node?.querySelector) {
        // Replacing the row must not drop keyboard focus from one of its buttons.
        const focus = node.contains?.(document.activeElement) ? document.activeElement.id : '';
        node.outerHTML = renderStatus(); const on = (id, fn) => document.getElementById(id)?.addEventListener('click', fn); on('kitb-scan', rescan); on('kitb-add', addRoot); on('kitb-new', () => window.STKitGroups?.createKit?.());
        if (focus) document.getElementById(focus)?.focus({ preventScroll: true });
      } }
      return;
    }
    if (type === 'library.results' || type === 'library.metadata') { if (value.volumes || !value.scanning) local.metadata.clear(); refreshSoon(value.scanning ? 600 : 100); }
    if (type === 'library.progress' || type === 'library.results') {
      if (value.scanning != null) local.status.scanning = !!value.scanning;
      if (value.kitIndexUpgradePending != null) local.status.kitIndexUpgradePending = !!value.kitIndexUpgradePending;
      if (value.kitIndexUpgradeScanning != null) local.status.kitIndexUpgradeScanning = !!value.kitIndexUpgradeScanning;
      if (Array.isArray(value.kitIndexUpgradeOfflineRoots)) local.status.kitIndexUpgradeOfflineRoots = value.kitIndexUpgradeOfflineRoots;
      if (mounted()) paint();
    }
    if ((type === 'sample.previewEnded' || type === 'sample.preview.stopped') && local.previewPath && (!value.path || value.path === local.previewPath)) { local.previewPath = ''; updatePreviewMarks(); }
  }
  function contextItems({ target }) {
    const row = target?.closest?.('[data-kitb-row]'), product = target?.closest?.('[data-kitb-product]');
    if (product) {
      const index = Number(product.dataset.kitbProduct), name = index === 0 ? null : productList()[index - 1]?.name;
      return [{ header: true, title: name || allLabel() }, { title: 'Show kits', key: '↩', action: () => { setProduct(index, { immediate: true }); focusResults(); } }, { title: 'Rescan library', action: rescan }];
    }
    if (!row) return null;
    select(Number(row.dataset.kitbRow), false); const item = current(); if (!item) return null;
    const project = S().state.project, replacementTarget = window.STKitGroups?.cursorGroup?.();
    const guarded = action => () => { if (!mounted() || current()?.path !== item.path || S().state.project !== project) { S().toast('The kit selection or project changed. Choose it again.', 'info'); return; } return action(); };
    return [{ header: true, title: item.name },
      { title: 'Load new kit', key: '↩', enabled: !local.busy && canImport(item), action: guarded(() => load('stay', item)) },
      { title: 'Replace selected kit — keep patterns', enabled: !local.busy && canImport(item) && !!replacementTarget, action: guarded(() => { if (window.STKitGroups?.cursorGroup?.()?.id !== replacementTarget?.id) { S().toast('The replacement kit changed. Select it again.', 'info'); return false; } return load('replace', item); }) },
      { title: 'Add and edit sounds', key: '⇧↩', enabled: !local.busy && canImport(item), action: guarded(() => load('open', item)) },
      { title: local.previewPath && local.previewPath === previewPath(item) ? 'Stop group preview' : 'Group preview', key: 'Space', enabled: item.available !== false, action: guarded(() => preview(item, true)) },
      { separator: true }, { title: item.favorite ? 'Remove from favorites' : 'Add to favorites', key: 'F', action: () => favorite(item) },
      ...(item.product ? [{ title: `Show all ${item.product} kits`, action: () => { Object.assign(filters(), { product: item.product, bank: null, subBank: null }); changed(); } }] : []),
      ...(item.bank ? [{ title: `Show ${item.bank}`, action: () => { Object.assign(filters(), { product: item.product, bank: item.bank, subBank: null }); changed(); } }] : []),
      { title: 'Show in Finder', action: () => S().request('app.external', { path: item.path, reveal: true }) }];
  }
  // Hardware controllers (mk3.js) use three filter columns: Source → Type, Expansion → product, Type → sub-type.
  const fields = ['sources', 'expansions', 'tags'];
  const fieldGroups = { sources: 'types', expansions: 'product', tags: 'subtypes' };
  const labels = { sources: 'Type', expansions: 'Expansion', tags: 'Sub-type' };
  const facetName = option => typeof option === 'string' ? option : option?.name ?? option?.value ?? '';
  function fieldOptions(field) {
    const group = fieldGroups[field];
    if (group === 'product') return productList().map(product => ({ value: product.name, label: product.name, detail: product.vendor || '', count: product.count }));
    return (local.facets[group] || []).map(option => ({ value: facetName(option), label: group === 'subtypes' ? option?.label || subtypeLabel(facetName(option)) : facetName(option), count: option?.count })).filter(option => option.value);
  }
  function fieldCurrent(field) { const group = fieldGroups[field], value = filters(); if (!group) return null; return group === 'product' ? value.product : value[group].size ? [...value[group]][0] : null; }
  function choose(field, chosen) {
    const group = fieldGroups[field]; if (!group) return;
    const value = filters();
    if (group === 'product') { Object.assign(value, { product: chosen ?? null, bank: null, subBank: null }); changed(); return; }
    value[group] = new Set(chosen != null ? [chosen] : []); if (group === 'types') value.subtypes.clear();
    changed();
  }
  const controller = {
    fields, filterLabel: field => labels[field] || field,
    filterValue(field) {
      const group = fieldGroups[field], value = filters(); if (!group) return 'All';
      if (group === 'product') return value.product ?? 'All';
      const set = value[group]; return set.size ? `${subtypeLabel([...set][0])}${set.size > 1 ? ` +${set.size - 1}` : ''}` : 'All';
    },
    value(field) { return this.filterValue(field); },
    options: field => fieldGroups[field] ? fieldOptions(field) : [], current: fieldCurrent, choose,
    cycle(field, delta) { const list = fieldOptions(field).map(option => option.value), step = Math.sign(delta); if (!step) return; const index = list.indexOf(fieldCurrent(field)), next = (fieldCurrent(field) === null ? -1 : index) + step; choose(field, next >= 0 && next < list.length ? list[next] : null); },
    toggle(field, value) { const group = fieldGroups[field] || field; if (group === 'product') { choose('expansions', filters().product === value ? null : value); return; } if (['types', 'subtypes', 'characters', 'banks', 'subBanks'].includes(group)) toggleChip(group, value); },
    list() { return { tab: 'kits', items: resultsCurrent() ? local.items.map(item => ({ name: item.name, detail: [item.product, item.bank].filter(Boolean).join(' · '), favorite: !!item.favorite, offline: item.available === false, preview: !!previewPath(item) })) : [], selected: filters().selected, total: local.total, offset: 0, busy: local.busy || !resultsCurrent(), preview: previewActive(), autoPreview: local.autoPreview, query: local.query, filters: activeFilterCount(), favorites: local.favorites, userOnly: local.user }; },
    move, page: delta => move(Math.sign(delta) * 16), load, preview: () => preview(current(), true), favorite: () => favorite(),
    toggleFavorites() { local.favorites = !local.favorites; changed(); }, toggleUserOnly() { setUser(!local.user); }, setUser,
    toggleAutoPreview() { local.autoPreview = !local.autoPreview; if (!local.autoPreview) stopPreview(); paint(); window.STBrowser?.saveState?.(); },
    up, clear: () => clearFilters(), clearFilters: () => clearFilters()
  };
  window.STKitBrowser = { render, bind, key, event, search: () => search({ keep: true }), focusSearch, stopPreview: () => stopPreview(), contextItems, controller, settings, restoreSettings, shortcuts,
    get previewActive() { return previewActive(); }, _local: local };
})();
