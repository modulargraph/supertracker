/* Maschine-style NKS browser: Product → Bank → Type / Sub-type → Character, with audio previews.
   Rendered inside the Library (STBrowser) as its "NKS" tab; every action is reachable by keyboard. */
(() => {
  'use strict';
  const S = () => window.ST;
  const html = value => S().escapeHTML(String(value ?? ''));
  const SEPARATOR = '\u001f';
  const LIMIT = 200;
  const blankFilters = () => ({ vendor: null, product: null, bank: null, subBank: null, types: new Set(), subtypes: new Set(), characters: new Set(), selected: 0 });
  const local = {
    kind: 'instrument', query: '', favorites: false, autoPreview: true, filters: { instrument: blankFilters(), effect: blankFilters() },
    items: [], total: 0, facets: { products: [], banks: [], subBanks: [], vendors: [], types: [], subtypes: [], characters: [] }, counts: { instrument: 0, effect: 0 },
    status: {}, loaded: false, searching: false, error: '', generation: 0, resultsKey: null, appending: false, timer: null, refreshTimer: null, refreshNeeded: false, lastRevision: null,
    chipFocus: {}, moreCharacters: false, typeahead: '', typeaheadTimer: null, busy: false, slot: null,
    previewPath: '', previewPending: false, previewGeneration: 0, previewTimer: null, previewEnd: null,
    pages: new Map(), pagesPending: new Set(), pagesTimer: null,
    art: { tile: new Map(), banner: new Map(), logo: new Map() }, artPending: new Set(), artTimer: null };
  const filters = () => local.filters[local.kind];
  const mounted = () => !!document.querySelector?.('.nksb-layout');
  const resultsKey = () => JSON.stringify(payload());
  const resultsCurrent = () => local.resultsKey === resultsKey();
  const current = () => resultsCurrent() ? local.items[filters().selected] : undefined;
  const offlineMessage = item => `${item.volume ? `“${item.volume}”` : 'This preset’s folder'} is unavailable. Reconnect the drive to preview or load ${item.name}.`;
  const previewActive = () => !!(local.previewPath || local.previewPending || local.previewTimer !== null);
  const plural = (count, word) => `${Number(count || 0).toLocaleString()} ${word}${count === 1 ? '' : 's'}`;
  const subtypeLabel = value => String(value).split(SEPARATOR).pop();
  const typeText = item => [...(item.types || []).filter(type => !(item.subtypes || []).some(sub => sub.startsWith(type + SEPARATOR))),
    ...(item.subtypes || []).map(sub => sub.split(SEPARATOR).join(' › '))].join(', ');

  // ---- Data ---------------------------------------------------------------------------------
  function payload(offset = 0) {
    const value = filters();
    return { kind: local.kind, query: local.query, ...(value.vendor ? { vendor: value.vendor } : {}), ...(value.product !== null ? { product: value.product } : {}), ...(value.bank !== null ? { bank: value.bank } : {}), ...(value.bank !== null && value.subBank !== null ? { subBank: value.subBank } : {}),
      types: [...value.types], subtypes: [...value.subtypes], characters: [...value.characters], favorites: local.favorites, offset, limit: LIMIT };
  }
  async function search({ append = false, keep = true } = {}) {
    if (append && (local.appending || !resultsCurrent() || local.items.length >= local.total)) return;
    clearTimeout(local.timer); local.timer = null;
    const generation = ++local.generation, kind = local.kind, previous = current()?.path, key = resultsKey(), revision = local.lastRevision;
    local.searching = true; local.appending = append; local.refreshNeeded = false;
    let result;
    // A refresh of the results on screen reloads every page already shown, so the selection and scroll position stay.
    const limit = !append && keep && local.resultsKey === key ? Math.min(1000, Math.max(LIMIT, local.items.length)) : LIMIT;
    try { result = await S().request('library.nks.browse', { ...payload(append ? local.items.length : 0), limit }); }
    catch (error) { if (generation === local.generation) { local.searching = false; local.appending = false; local.resultsKey = null; local.error = error?.message || String(error); paint(); } return; }
    if (generation !== local.generation || kind !== local.kind || key !== resultsKey()) return;
    local.searching = false; local.appending = false;
    // Offsets belong to one catalogue revision. Restart instead of joining shifted pages.
    if (append && revision != null && result?.status?.revision != null && revision !== result.status.revision) return search({ keep: true });
    local.error = ''; local.loaded = true; local.resultsKey = key;
    const items = Array.isArray(result?.items) ? result.items : [];
    local.items = append ? [...local.items, ...items.filter(item => !local.items.some(existing => existing.path === item.path))] : items;
    local.total = Number(result?.total) || 0;
    for (const key of ['products', 'banks', 'subBanks', 'vendors', 'types', 'subtypes', 'characters']) local.facets[key] = Array.isArray(result?.[key]) ? result[key] : [];
    local.counts = result?.counts || local.counts;
    if (result?.status) { local.status = result.status; local.lastRevision = result.status.revision; }
    if (!append) {
      const index = keep && previous ? local.items.findIndex(item => item.path === previous) : -1;
      filters().selected = index >= 0 ? index : Math.min(filters().selected, Math.max(0, local.items.length - 1));
      if (!keep) filters().selected = 0;
    }
    paint();
    if (!append) revealSelection();
    queuePages(); queuePrefetch();
  }
  function queueSearch(delay = 90, options = {}) { clearTimeout(local.timer); local.timer = setTimeout(() => { local.timer = null; search(options); }, delay); }
  function changed(delay = 0) {
    // Invalidate before the debounce: old replies and actions must not belong to a new filter.
    local.generation++; local.resultsKey = null; local.appending = false; local.searching = true; local.error = '';
    filters().selected = 0; if (previewActive()) stopPreview();
    queueSearch(delay, { keep: false }); paint();
  }

  // ---- Artwork ------------------------------------------------------------------------------
  // Product and expansion pictures arrive as bounded data URLs from library.nks.artwork, keyed by
  // the artwork folder. Missing pictures fall back to the product colour and its initials.
  const ART_VARIANTS = ['tile', 'banner', 'logo'];
  const artURL = (key, variant = 'tile') => { const value = key ? local.art[variant]?.get(key) : ''; return typeof value === 'string' && value.startsWith('data:image/') ? value : ''; };
  const initials = name => String(name || '').replace(/[^\p{L}\p{N} ]/gu, ' ').split(/\s+/).filter(Boolean).slice(0, 2).map(word => word[0]).join('').toUpperCase() || '◈';
  const safeColor = value => /^#[0-9a-f]{6}$/i.test(value || '') ? value : '';
  function art(key, name, { variant = 'tile', color = '', className = '' } = {}) {
    const url = artURL(key, variant), tint = safeColor(color);
    const style = [tint ? `--nksb-tint:${tint}` : '', url ? `background-image:url('${url}')` : ''].filter(Boolean).join(';');
    return `<span class="nksb-art ${className} ${url ? 'has-art' : ''}" aria-hidden="true"${key ? ` data-nksb-art="${html(key)}" data-nksb-variant="${variant}"` : ''}${style ? ` style="${style}"` : ''}><span>${html(initials(name))}</span></span>`;
  }
  // Fetch the pictures the mounted view shows, in one batched request per variant.
  function queueArt() {
    if (local.artTimer !== null) return;
    local.artTimer = setTimeout(async () => {
      local.artTimer = null;
      const wanted = { tile: new Set(), banner: new Set(), logo: new Set() };
      for (const element of document.querySelectorAll?.('[data-nksb-art]') || []) {
        const key = element.dataset.nksbArt, variant = ART_VARIANTS.includes(element.dataset.nksbVariant) ? element.dataset.nksbVariant : 'tile';
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
    for (const element of document.querySelectorAll?.('[data-nksb-art]') || []) {
      const url = artURL(element.dataset.nksbArt, element.dataset.nksbVariant || 'tile');
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
  function renderVendors() {
    const value = filters(), vendors = [...local.facets.vendors];
    if (value.vendor && !vendors.some(vendor => vendor.name === value.vendor)) vendors.unshift({ name: value.vendor, count: 0 });
    if (vendors.length < 2 && !value.vendor) return '';
    return `<label class="nksb-vendor"><span>Vendor</span><select id="nksb-vendor" aria-label="Filter products by vendor"><option value="">All vendors</option>${vendors.map(vendor => `<option value="${html(vendor.name)}" ${vendor.name === value.vendor ? 'selected' : ''}>${html(vendor.name)} · ${Number(vendor.count || 0).toLocaleString()}</option>`).join('')}</select></label>`;
  }
  function renderProducts() {
    const list = productList(), active = productIndex(), total = local.facets.products.reduce((sum, product) => sum + Number(product.count || 0), 0);
    const all = local.kind === 'effect' ? 'All effects' : 'All instruments';
    const row = (index, product, detail, count, title) => `<div role="option" id="nksb-product-${index}" class="nksb-product ${index === active ? 'selected' : ''}" data-nksb-product="${index}" aria-selected="${index === active}" title="${html(title)}">${index === 0 ? `<span class="nksb-art nksb-art-all" aria-hidden="true"><span>${local.kind === 'effect' ? '≋' : '◈'}</span></span>` : art(product.art, product.name, { color: product.color })}<span class="nksb-product-name">${html(index === 0 ? all : product.name)}</span><small>${html(detail)}</small><span class="nksb-count">${Number(count).toLocaleString()}</span></div>`;
    return `<div id="nksb-products" class="nksb-product-list" role="listbox" tabindex="0" aria-label="${local.kind === 'effect' ? 'Effect' : 'Instrument'} products" aria-activedescendant="nksb-product-${active}" data-scroll="nksb-products">${row(0, null, filters().vendor || 'Every product', total, 'Show every product')}${list.map((product, index) => row(index + 1, product, product.vendor || 'NKS', product.count, `${product.name}${product.vendor ? ` · ${product.vendor}` : ''}`)).join('')}${local.loaded && !list.length ? '<p class="nksb-filter-empty nksb-products-empty">No products match these filters.</p>' : ''}</div>`;
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
      const on = selected.has(option.name), text = option.name === '' ? (group === 'banks' ? 'No bank' : 'None') : labelOf(option);
      const count = option.count != null ? `<small>${Number(option.count).toLocaleString()}</small>` : '';
      const body = tiles ? `${art(option.art || chosenProduct()?.art, text, { color: option.color || chosenProduct()?.color, className: option.art ? '' : 'nksb-art-fallback' })}<span class="nksb-tile-name">${html(text)}</span>${count}` : `${html(text)}${count}`;
      return `<button type="button" id="nksb-chip-${group}-${encodeURIComponent(option.name).replace(/[^A-Za-z0-9_-]/g, '_')}" class="nksb-chip ${tiles ? 'nksb-tile' : ''} ${on ? 'chosen' : ''}" role="${single ? 'radio' : 'checkbox'}" aria-checked="${on}" tabindex="${option.name === focus ? 0 : -1}" data-nksb-chip="${group}" data-nksb-value="${html(option.name)}" title="${html(option.type ? `${option.type} › ${text}` : text)}">${body}</button>`;
    }).join('');
    const more = limit && (hidden || local.moreCharacters) ? `<button type="button" class="nksb-chip nksb-more" id="nksb-more-${group}" data-nksb-chip="${group}" data-nksb-more="true" tabindex="-1">${local.moreCharacters ? 'Fewer' : `${hidden} more…`}</button>` : '';
    return `<div class="nksb-chip-group ${tiles ? 'nksb-tiles' : ''}" data-nksb-group="${group}" role="group" aria-label="${html(label)}${single ? '' : ', multiple choice'}"><span class="nksb-chip-label">${html(label)}${selected.size ? `<small>${selected.size}</small>` : ''}</span><div class="nksb-chips">${buttons}${more}</div></div>`;
  }
  // Product › Bank › Sub-bank, each level clickable to step back out.
  function renderCrumbs() {
    const value = filters(), levels = [[local.kind === 'effect' ? 'All effects' : 'All instruments', 'all']];
    if (value.product !== null) levels.push([value.product, 'product']);
    if (value.product !== null && value.bank !== null) levels.push([value.bank || 'No bank', 'bank']);
    if (value.product !== null && value.bank !== null && value.subBank !== null) levels.push([value.subBank || 'None', 'subBank']);
    return `<nav class="nksb-crumbs" aria-label="Bank chain">${levels.map(([name, level], index) => index === levels.length - 1 ? `<span aria-current="location">${html(name)}</span>` : `<button type="button" id="nksb-crumb-${level}" data-nksb-crumb="${level}" tabindex="-1">${html(name)}</button><i aria-hidden="true">›</i>`).join('')}</nav>`;
  }
  function renderHero() {
    const value = filters(), product = chosenProduct();
    if (!product) return '';
    const bank = value.bank !== null ? local.facets.banks.find(option => option.name === value.bank) : null;
    const key = bank?.art || product.art, name = bank?.art ? bank.name : product.name;
    const count = local.loaded && resultsCurrent() ? `${local.total.toLocaleString()} preset${local.total === 1 ? '' : 's'}` : '';
    return `<header class="nksb-hero" style="${safeColor(bank?.color || product.color) ? `--nksb-tint:${safeColor(bank?.color || product.color)}` : ''}">${art(key, name, { variant: 'banner', className: 'nksb-hero-art' })}<div class="nksb-hero-copy">${renderCrumbs()}<h2>${html(value.bank && bank?.art ? value.bank : product.name)}</h2><p>${html([bank?.art ? product.name : '', product.vendor, count].filter(Boolean).join(' · '))}</p></div></header>`;
  }
  function renderFilters() {
    const value = filters(), banks = local.facets.banks, tiles = banks.some(option => option.art);
    const groups = [
      value.product !== null && banks.length ? chips('banks', tiles ? 'Expansion' : 'Bank', banks, { single: true, tiles }) : '',
      value.product !== null && value.bank !== null && (local.facets.subBanks.length || value.subBank !== null) ? chips('subBanks', 'Sub-bank', local.facets.subBanks, { single: true }) : '',
      chips('types', 'Type', local.facets.types),
      value.types.size || value.subtypes.size ? chips('subtypes', 'Sub-type', local.facets.subtypes, { labelOf: option => option.label || subtypeLabel(option.name) }) : '',
      chips('characters', 'Character', local.facets.characters, { limit: 16 })].filter(Boolean).join('');
    return `<div class="nksb-filters" aria-label="Preset filters">${groups || `<p class="nksb-filter-empty">${local.loaded ? 'No type or character tags in these presets.' : 'Reading NKS tags…'}</p>`}</div>`;
  }
  function activeFilterCount() { const value = filters(); return !!value.vendor + (value.product !== null) + (value.bank !== null) + (value.bank !== null && value.subBank !== null) + value.types.size + value.subtypes.size + value.characters.size + local.favorites + !!local.query.trim(); }
  const skeleton = () => Array.from({ length: 9 }, (_, index) => `<div class="nksb-row nksb-skeleton" aria-hidden="true" style="--delay:${index * 60}ms"><span></span><span class="nksb-art"></span><div class="nksb-row-name"><strong></strong><small></small></div><span class="nksb-row-type"></span><span></span></div>`).join('');
  function renderRows() {
    if (!resultsCurrent()) return local.error ? `<div class="browser-empty" role="status"><h3>Could not search presets</h3><p>${html(local.error)}</p><button class="btn" id="nksb-retry">Retry search</button></div>` : `<div role="status" class="nksb-loading"><span class="nksb-sr">Searching presets…</span>${skeleton()}</div>`;
    if (!local.items.length) {
      if (!local.loaded || (local.status?.indexing && !activeFilterCount())) return `<div role="status" class="nksb-loading"><div class="browser-empty"><h3>Reading NKS presets…</h3><p>The first visit indexes installed presets in the background. Results appear as they are read.</p></div>${skeleton()}</div>`;
      if (activeFilterCount()) return '<div class="browser-empty"><h3>No matching presets</h3><p>Remove a filter or shorten the search.</p><button class="btn" id="nksb-clear-empty">Clear search & filters</button></div>';
      return `<div class="browser-empty"><h3>No NKS ${local.kind === 'effect' ? 'effects' : 'instruments'} found</h3><p>Installed NKS content registered with Native Access, and .nksf/.nksfx presets in your library folders, appear here.</p><button class="btn" id="nksb-rescan-empty">Rescan NKS presets</button></div>`;
    }
    const selected = filters().selected, product = filters().product;
    return local.items.map((item, index) => {
      const playing = !!local.previewPath && local.previewPath === item.previewPath;
      const chain = [product === null ? item.product : '', item.bank, item.subBank].filter(Boolean).join(' › ');
      return `<div class="nksb-row ${index === selected ? 'selected' : ''} ${item.available === false ? 'offline' : ''}" id="nksb-row-${index}" role="option" aria-selected="${index === selected}" data-nksb-row="${index}"><button type="button" class="nksb-play ${playing ? 'playing' : ''}" data-nksb-play="${index}" tabindex="-1" ${item.previewPath ? '' : 'disabled'} aria-label="${playing ? 'Stop preview' : 'Preview'}: ${html(item.name)}">${playing ? '■' : item.previewPath ? '▶' : ''}</button>${art(item.art, item.bank || item.product, { color: item.color, className: 'nksb-row-art' })}<div class="nksb-row-name"><strong>${html(item.name)}</strong><small>${item.available === false ? '<span class="browser-offline-badge">Offline</span> ' : ''}${html(chain || item.vendor || '')}</small></div><span class="nksb-row-type">${html(typeText(item) || '—')}</span><button class="browser-star ${item.favorite ? 'is-favorite' : ''}" data-nksb-star="${index}" tabindex="-1" aria-label="${item.favorite ? 'Remove from' : 'Add to'} favorites: ${html(item.name)}">${item.favorite ? '★' : '☆'}</button></div>`;
    }).join('') +
      (local.items.length < local.total ? `<button type="button" class="nksb-more-rows" id="nksb-more-rows" tabindex="-1">Show more · ${(local.total - local.items.length).toLocaleString()} remaining</button>` : '');
  }
  function effectSlots() {
    const state = S().state, effects = [];
    (state.project?.audioPlugins || []).filter(plugin => plugin.owner === `track:${state.project.tracks?.[state.track]?.routingID}` && plugin.kind === 'effect').forEach(plugin => { effects[plugin.slot ?? 0] = plugin; });
    return effects;
  }
  function slotChoice() {
    const effects = effectSlots(), empty = effects.findIndex(value => !value);
    const fallback = Math.min(7, empty < 0 ? effects.length : empty);
    const slot = local.slot === null ? fallback : Math.max(0, Math.min(7, local.slot));
    return { effects, slot, count: Math.min(8, Math.max(slot + 1, effects.length + 1)) };
  }
  function renderPages(item) {
    const pages = local.pages.get(item.path);
    if (!item.hasControls && !pages?.length) return '<p class="browser-small muted nksb-pages-note">This preset has no NKS control pages. After loading, its plug-in parameters are grouped into eight-knob pages automatically.</p>';
    if (!pages) return `<p class="browser-small muted nksb-pages-note">${local.pagesPending.has(item.path) ? 'Reading control pages…' : 'Control pages load with the selection.'}</p>`;
    return `<div class="nksb-pages"><h3>Control pages <span>${pages.length}</span></h3>${pages.slice(0, 12).map((page, index) => `<div class="nksb-page"><strong>${html(page.name || `Page ${index + 1}`)}</strong><ol>${Array.from({ length: 8 }, (_, slot) => { const parameter = (page.parameters || []).find(value => Number(value.slot) === slot); return `<li class="${parameter ? '' : 'empty'}">${html(parameter?.name || '—')}</li>`; }).join('')}</ol></div>`).join('')}${pages.length > 12 ? `<p class="browser-small muted">${pages.length - 12} more pages after loading.</p>` : ''}</div>`;
  }
  function renderDetail() {
    const item = current(), state = S().state;
    if (!item) return '<div class="browser-empty browser-detail-empty"><h3>No preset selected</h3><p>← → move between products, filters and presets<br>↑ ↓ browse · Space previews<br>Return loads</p></div>';
    const track = state.project?.tracks?.[state.track], trackName = track?.name || `Track ${Number(state.track || 0) + 1}`;
    const instrument = state.project?.instruments?.[state.instrument], playing = local.previewPath && local.previewPath === item.previewPath;
    const tags = [...(item.types || []).map(value => ['types', value, value]), ...(item.subtypes || []).map(value => ['subtypes', value, subtypeLabel(value)]), ...(item.characters || []).map(value => ['characters', value, value])]
      .filter(([group, , label], index, all) => all.findIndex(other => other[0] === group && other[2] === label) === index);
    const slot = slotChoice();
    const actions = item.effect
      ? `<label class="browser-slot-label">Insert slot on ${html(trackName)}<select id="nksb-slot" class="field">${Array.from({ length: slot.count }, (_, index) => `<option value="${index}" ${index === slot.slot ? 'selected' : ''}>FX ${index + 1} · ${html(slot.effects[index]?.nks?.name || slot.effects[index]?.name || 'Empty')}</option>`).join('')}</select></label><button class="btn primary" id="nksb-load" ${local.busy || item.available === false ? 'disabled' : ''}>${local.busy ? 'Loading effect…' : `Load into FX ${slot.slot + 1}`} <kbd>↵</kbd></button>`
      : `<button class="btn primary" id="nksb-load" ${local.busy || item.available === false ? 'disabled' : ''}>${local.busy ? 'Loading preset…' : 'Load and keep browsing'} <kbd>↵</kbd></button><button class="btn" id="nksb-load-open" ${local.busy || item.available === false ? 'disabled' : ''}>Load and edit controls <kbd>⇧ ↵</kbd></button><button class="btn" id="nksb-load-new" ${local.busy || item.available === false ? 'disabled' : ''}>Load as new instrument <kbd>⌥ ↵</kbd></button><p class="browser-small muted">${instrument?.type !== 'au' ? 'Adds a new instrument and track; existing sounds are kept.' : `Replaces ${html(instrument?.name || 'the current instrument')} on ${html(trackName)}.`}</p>`;
    return `<div class="nksb-detail-art" style="${safeColor(item.color) ? `--nksb-tint:${safeColor(item.color)}` : ''}">${art(item.art, item.bank || item.product, { variant: 'banner', className: 'nksb-detail-banner' })}${item.productArt ? art(item.productArt, item.product, { variant: 'logo', className: 'nksb-detail-logo' }) : ''}</div>
      <div class="browser-detail-top"><span class="eyebrow">${item.effect ? 'NKS effect' : 'NKS instrument'}${item.format ? ` · ${html(item.format)}` : ''}</span><h2>${html(item.name)}</h2><p class="nksb-detail-chain">${[item.product, item.bank, item.subBank].filter(Boolean).map(html).join(' <i aria-hidden="true">›</i> ')}</p>${item.vendor ? `<p>${html(item.vendor)}</p>` : ''}</div>
      ${item.available === false ? `<p role="status">${html(offlineMessage(item))}</p>` : ''}
      ${tags.length ? `<div class="browser-tags">${tags.map(([group, value, label], index) => `<button id="nksb-tag-${index}" data-nksb-tag="${group}" data-nksb-value="${html(value)}" title="Filter by ${html(label)}">${html(label)}</button>`).join('')}</div>` : ''}
      <div class="browser-detail-actions"><button class="btn" id="nksb-preview" ${item.previewPath && item.available !== false ? '' : 'disabled'}>${playing ? '■ Stop preview' : item.previewPath ? '▶ Preview' : 'No preview available'} <kbd>Space</kbd></button>${actions}</div>
      <dl class="browser-properties">${[['Author', item.author], ['Format', item.format && `${item.format} plug-in`]].filter(([, value]) => value).map(([label, value]) => `<div><dt>${label}</dt><dd>${html(value)}</dd></div>`).join('')}</dl>
      ${item.comment ? `<p class="nksb-comment">${html(item.comment)}</p>` : ''}
      ${renderPages(item)}
      <div class="browser-path"><span class="eyebrow">File location</span><p title="${html(item.path)}">${html(item.path)}</p></div>`;
  }
  function renderStatus() {
    const status = local.status || {};
    if (status.indexing) return `<div class="nksb-status" role="status"><span class="browser-pulse"></span> Indexing NKS presets${status.total ? ` · ${Number(status.done || 0).toLocaleString()} of ${Number(status.total).toLocaleString()}` : '…'}</div>`;
    return `<div class="nksb-status"><div class="browser-root-copy"><span>${status.count != null ? plural(status.count, 'preset') : ''}</span>${status.error ? `<details class="browser-root-problem"><summary id="nksb-root-errors">Folder / index issues</summary>${String(status.error).split('\n').map(message => `<p>${html(message)}</p>`).join('')}</details>` : ''}</div><button class="browser-icon-btn" id="nksb-rescan" title="Rescan installed NKS presets" aria-label="Rescan installed NKS presets">↻</button></div>`;
  }
  function render() {
    const value = filters(), kinds = [['instrument', 'Instruments'], ['effect', 'Effects']];
    const showing = local.error ? local.error : !local.loaded ? 'Loading…' : `${local.total.toLocaleString()} ${local.kind === 'effect' ? 'effect' : 'instrument'} preset${local.total === 1 ? '' : 's'}`;
    return `<div class="nksb-layout">
      <aside class="browser-sidebar panel nksb-sidebar" aria-label="NKS products">
        <div class="nksb-kinds" role="tablist" aria-label="Preset kind">${kinds.map(([kind, name]) => `<button type="button" role="tab" id="nksb-kind-${kind}" data-nksb-kind="${kind}" aria-selected="${local.kind === kind}" tabindex="${local.kind === kind ? 0 : -1}" class="${local.kind === kind ? 'active' : ''}">${name}<small>${Number(local.counts[kind] ?? 0).toLocaleString()}</small></button>`).join('')}</div>
        ${renderVendors()}
        ${renderProducts()}
        ${renderStatus()}
      </aside>
      <section class="browser-results panel nksb-main" aria-label="NKS presets">
        <div class="browser-search-wrap"><span aria-hidden="true">⌕</span><input id="nksb-search" class="browser-search" type="search" value="${html(local.query)}" placeholder="Search ${local.kind === 'effect' ? 'effect' : 'instrument'} presets, products, types…" aria-label="Search NKS presets" autocomplete="off" spellcheck="false"><kbd>⌘ F</kbd></div>
        ${renderHero()}
        ${renderFilters()}
        <div class="browser-result-summary"><span role="status">${html(showing)}${value.product === null && value.vendor ? ` · ${html(value.vendor)}` : ''}${activeFilterCount() ? ` <button type="button" id="nksb-clear" class="nksb-clear">Clear ${activeFilterCount() === 1 ? 'filter' : 'filters'}</button>` : ''}</span><span class="nksb-toggles"><button type="button" id="nksb-favorites" class="nksb-toggle ${local.favorites ? 'chosen' : ''}" aria-pressed="${local.favorites}">${local.favorites ? '★' : '☆'} Favorites</button><label class="browser-audition-toggle"><input id="nksb-autopreview" type="checkbox" ${local.autoPreview ? 'checked' : ''}> Auto preview</label></span></div>
        <div id="nksb-results" class="browser-result-list nksb-result-list" data-scroll="nksb-results" tabindex="0" role="listbox" aria-label="${local.kind === 'effect' ? 'Effect' : 'Instrument'} presets" aria-busy="${local.searching}" ${current() ? `aria-activedescendant="nksb-row-${value.selected}"` : ''}>${renderRows()}</div>
      </section>
      <aside class="browser-detail panel nksb-detail" id="nksb-detail" aria-label="Selected preset">${renderDetail()}</aside>
    </div>`;
  }
  function shortcuts() {
    return '<span><kbd>⌘ F</kbd> Search</span><span><kbd>← →</kbd> Products · presets · details</span><span><kbd>↑ ↓</kbd> Browse</span><span><kbd>Space</kbd> Preview</span><span><kbd>↵</kbd> Load</span><span><kbd>⇧ ↵</kbd> Load & edit</span><span><kbd>F</kbd> Favorite</span><span><kbd>⌫</kbd> Up a level</span><span><kbd>Tab</kbd> Filters</span>';
  }

  // ---- Painting -----------------------------------------------------------------------------
  function paint() {
    if (!mounted()) return;
    const focused = document.activeElement, id = focused?.id, start = focused?.selectionStart, end = focused?.selectionEnd;
    S().refresh();
    const next = id && document.getElementById(id);
    if (next && next !== document.activeElement && !next.disabled) next.focus({ preventScroll: true });
    else if (id?.startsWith('nksb-') && (!next || next.disabled)) {
      const sibling = focused?.dataset?.nksbChip && document.querySelector(`[data-nksb-group="${focused.dataset.nksbChip}"] [tabindex="0"]`);
      (sibling || document.getElementById('nksb-results'))?.focus({ preventScroll: true });
    }
    if (next && typeof start === 'number' && next.setSelectionRange) try { next.setSelectionRange(start, end); } catch {}
    queueArt();
  }
  function revealSelection() { document.getElementById(`nksb-row-${filters().selected}`)?.scrollIntoView?.({ block: 'nearest' }); }
  // Selection changes update only the rows and detail panel, so arrows stay instant on long lists.
  function updateSelection(previous) {
    const list = document.getElementById('nksb-results'), detail = document.getElementById('nksb-detail');
    if (!list || !detail) { paint(); return; }
    const index = filters().selected;
    for (const row of [document.getElementById(`nksb-row-${previous}`), document.getElementById(`nksb-row-${index}`)]) {
      if (!row) continue;
      const on = Number(row.dataset.nksbRow) === index;
      row.classList?.toggle('selected', on); row.setAttribute?.('aria-selected', String(on));
    }
    list.setAttribute?.('aria-activedescendant', `nksb-row-${index}`);
    replaceDetail();
    revealSelection();
  }
  function replaceDetail() {
    const detail = document.getElementById('nksb-detail');
    if (!detail) return;
    const focused = document.activeElement, inside = !!focused?.closest?.('#nksb-detail') ? focused.id : null;
    detail.innerHTML = renderDetail(); bindDetail(); queueArt();
    if (inside) (document.getElementById(inside) || document.getElementById('nksb-results'))?.focus({ preventScroll: true });
  }
  function updatePreviewMarks() {
    for (const row of document.querySelectorAll?.('[data-nksb-row]') || []) {
      const item = local.items[Number(row.dataset.nksbRow)], mark = row.querySelector?.('.nksb-play');
      if (!item || !mark) continue;
      const playing = !!local.previewPath && local.previewPath === item.previewPath;
      mark.textContent = playing ? '■' : item.previewPath ? '▶' : ''; mark.classList?.toggle('playing', playing);
    }
    const button = document.getElementById('nksb-preview'), item = current();
    if (button && item) button.innerHTML = `${local.previewPath && local.previewPath === item.previewPath ? '■ Stop preview' : item.previewPath ? '▶ Preview' : 'No preview available'} <kbd>Space</kbd>`;
  }

  // ---- Actions ------------------------------------------------------------------------------
  function select(index, audition = true) {
    if (!resultsCurrent() || !local.items.length) return;
    const previous = filters().selected, next = Math.max(0, Math.min(index, local.items.length - 1));
    filters().selected = next;
    if (next !== previous) updateSelection(previous); else revealSelection();
    if (index >= local.items.length - 1 && local.items.length < local.total) search({ append: true });
    clearTimeout(local.previewTimer); local.previewTimer = null;
    if (audition && local.autoPreview && next !== previous) {
      if (current()?.previewPath && current()?.available !== false) local.previewTimer = setTimeout(() => { local.previewTimer = null; preview(current()); }, 110);
      else if (previewActive()) stopPreview();
    }
    queuePages(); queuePrefetch();
  }
  // Background-decode the .ogg previews around the selection while auto preview is on.
  const prefetchEntry = item => item.previewPath ? { path: item.previewPath } : null;
  function queuePrefetch() {
    if (!local.autoPreview || !resultsCurrent()) return;
    window.STPreviewPrefetch?.schedule('nks', { items: local.items, selected: filters().selected, entry: prefetchEntry });
  }
  function setProduct(index, { immediate = false } = {}) {
    const list = productList(), next = Math.max(0, Math.min(index, list.length));
    const name = next === 0 ? null : list[next - 1].name;
    if (name === filters().product) return;
    filters().product = name; filters().bank = null; filters().subBank = null; local.chipFocus.banks = undefined;
    const container = document.getElementById('nksb-products');
    if (container) {
      for (const row of container.querySelectorAll?.('[data-nksb-product]') || []) { const on = Number(row.dataset.nksbProduct) === next; row.classList?.toggle('selected', on); row.setAttribute?.('aria-selected', String(on)); }
      container.setAttribute?.('aria-activedescendant', `nksb-product-${next}`);
      document.getElementById(`nksb-product-${next}`)?.scrollIntoView?.({ block: 'nearest' });
    }
    changed(immediate ? 0 : 110);
  }
  function setKind(kind) {
    if (kind === local.kind || !['instrument', 'effect'].includes(kind)) return;
    stopPreview(); local.kind = kind; local.items = []; local.total = 0; local.loaded = false; local.moreCharacters = false;
    paint(); document.getElementById('nksb-products')?.scrollTo?.(0, 0); search({ keep: false });
  }
  function toggleChip(group, value) {
    const state = filters();
    if (group === 'banks') { state.bank = state.bank === value ? null : value; state.subBank = null; }
    else if (group === 'subBanks') state.subBank = state.subBank === value ? null : value;
    else if (state[group]) {
      if (state[group].has(value)) state[group].delete(value); else state[group].add(value);
      // Removing a type also removes its sub-types.
      if (group === 'types' && !state.types.has(value)) for (const sub of [...state.subtypes]) if (sub.startsWith(value + SEPARATOR)) state.subtypes.delete(sub);
    }
    local.chipFocus[group] = value; changed();
  }
  // Sub-bank → bank → product → everything, like the hardware browser's back button.
  function up(level) {
    const state = filters();
    if (level === 'all' || (level === undefined && state.bank === null)) { if (state.product === null) return false; setProduct(0, { immediate: true }); return true; }
    if (level === 'product' || (level === undefined && state.subBank === null)) { if (state.bank === null) return false; state.bank = null; state.subBank = null; changed(); return true; }
    if (state.subBank === null) return false;
    state.subBank = null; changed(); return true;
  }
  function setVendor(vendor) {
    const state = filters(), next = vendor || null;
    if (next === state.vendor) return;
    state.vendor = next;
    // A product from another vendor would hide every result.
    if (next && state.product !== null && chosenProduct()?.vendor && chosenProduct().vendor !== next) { state.product = null; state.bank = null; state.subBank = null; }
    changed();
  }
  // First clear keeps the chosen product; clearing again (or with only a product) shows every product.
  function clearFilters() {
    const state = filters(), onlyProduct = state.product !== null && activeFilterCount() === 1;
    local.filters[local.kind] = { ...blankFilters(), product: onlyProduct ? null : state.product };
    local.query = ''; local.favorites = false;
    changed();
  }
  async function preview(item = current(), toggle = false) {
    if (!item || !resultsCurrent()) return;
    if (item.available === false) { if (toggle) S().toast(offlineMessage(item), 'info'); return; }
    const path = item.previewPath;
    if (!path) { if (toggle) S().toast('This preset has no audio preview.', 'info'); return; }
    if (toggle && local.previewPath === path) { await stopPreview(); return; }
    const generation = ++local.previewGeneration;
    local.previewPending = true; clearTimeout(local.previewEnd);
    try {
      const result = await S().request('sample.preview', { path });
      if (generation !== local.previewGeneration) return;
      local.previewPath = path;
      const duration = Number(result?.duration);
      if (duration > 0) local.previewEnd = setTimeout(() => { if (generation === local.previewGeneration) { local.previewPath = ''; updatePreviewMarks(); } }, Math.min(duration * 1000 + 80, 600000));
    } catch (error) { if (generation === local.previewGeneration && toggle) S().toast(error?.message || 'Preview failed.', 'error'); }
    finally { if (generation === local.previewGeneration) { local.previewPending = false; updatePreviewMarks(); } }
  }
  async function stopPreview() {
    const wasActive = previewActive();
    local.previewGeneration++; clearTimeout(local.previewTimer); clearTimeout(local.previewEnd);
    local.previewTimer = null; local.previewPending = false; local.previewPath = '';
    if (wasActive) { try { await S().request('sample.stopPreview', {}); } catch {} }
    updatePreviewMarks();
  }
  async function load(mode = 'stay', expected = current(), expectedSlot = null) {
    const item = current();
    if (item?.path !== expected?.path) { S().toast('The preset selection changed. Choose the preset again.', 'info'); return false; }
    if (!item || local.busy) return false;
    if (item.effect && expectedSlot !== null && expectedSlot !== slotChoice().slot) { S().toast('The effect slot changed. Choose the destination again.', 'info'); return false; }
    if (item.available === false) { S().toast(offlineMessage(item), 'error'); return false; }
    if (!window.STNKS?.load) { S().toast('NKS loading is unavailable in this build.', 'error'); return false; }
    const state = S().state, project = state.project, track = state.track, instrument = state.instrument, destinationSlot = slotChoice().slot;
    local.busy = true; updateSelection(filters().selected);
    let loaded = false;
    try {
      await stopPreview();
      if (S().state.project !== project || S().state.track !== track || S().state.instrument !== instrument) { S().toast('The destination changed. Load the preset again in the intended channel.', 'info'); return false; }
      if (item.effect) {
        loaded = await window.STNKS.load(item.path, { kind: 'effect', slot: destinationSlot });
        if (loaded) local.slot = null;
      } else loaded = await window.STNKS.load(item.path, { newInstrument: mode === 'new', stay: mode !== 'open' });
    } finally {
      local.busy = false;
      if (mounted()) paint();
    }
    return loaded;
  }
  async function favorite(item = current()) {
    if (!item) return;
    const favorite = !item.favorite;
    try {
      await S().request('library.favorite', { path: item.path, favorite });
      item.favorite = favorite;
      if (local.favorites && !favorite) search(); else paint();
    } catch (error) { S().toast(error?.message || 'Could not change the favorite.', 'error'); }
  }
  async function rescan() {
    try { local.status = { ...(await S().request('library.nks.rescan', {})) }; } catch (error) { S().toast(error?.message || 'Could not rescan NKS presets.', 'error'); }
    paint();
  }
  function queuePages() {
    clearTimeout(local.pagesTimer);
    local.pagesTimer = setTimeout(async () => {
      local.pagesTimer = null;
      const item = current();
      if (item?.available === false || !item?.hasControls || local.pages.has(item.path) || local.pagesPending.has(item.path)) return;
      local.pagesPending.add(item.path);
      try {
        const meta = await S().request('library.nks', { path: item.path });
        local.pages.set(item.path, (meta?.controllerPages || []).filter(page => page.parameters?.length));
        if (local.pages.size > 400) local.pages.delete(local.pages.keys().next().value);
      } catch { local.pages.set(item.path, []); }
      finally { local.pagesPending.delete(item.path); }
      if (current()?.path === item.path && mounted()) replaceDetail();
    }, 260);
  }

  // ---- Binding ------------------------------------------------------------------------------
  function bindDetail() {
    const on = (id, fn, type = 'click') => document.getElementById(id)?.addEventListener(type, fn);
    on('nksb-preview', () => preview(current(), true));
    on('nksb-load', () => load('stay')); on('nksb-load-open', () => load('open')); on('nksb-load-new', () => load('new'));
    on('nksb-slot', event => { local.slot = Number(event.target.value); const button = document.getElementById('nksb-load'); if (button && !local.busy) button.innerHTML = `Load into FX ${local.slot + 1} <kbd>↵</kbd>`; }, 'change');
    document.querySelectorAll('[data-nksb-tag]').forEach(button => button.addEventListener('click', () => {
      const group = button.dataset.nksbTag, value = button.dataset.nksbValue, state = filters();
      if (group === 'subtypes') state.types.add(value.split(SEPARATOR)[0]);
      if (!state[group]?.has(value)) toggleChip(group, value);
    }));
  }
  function bind() {
    const on = (id, fn, type = 'click') => document.getElementById(id)?.addEventListener(type, fn);
    document.querySelectorAll('[data-nksb-kind]').forEach(button => button.addEventListener('click', () => setKind(button.dataset.nksbKind)));
    document.querySelectorAll('[data-nksb-product]').forEach(row => {
      row.addEventListener('click', () => { setProduct(Number(row.dataset.nksbProduct), { immediate: true }); document.getElementById('nksb-products')?.focus({ preventScroll: true }); });
      row.addEventListener('dblclick', () => document.getElementById('nksb-results')?.focus());
    });
    document.querySelectorAll('[data-nksb-chip]').forEach(button => {
      button.addEventListener('click', () => { if (button.dataset.nksbMore) { local.moreCharacters = !local.moreCharacters; paint(); return; } toggleChip(button.dataset.nksbChip, button.dataset.nksbValue); });
      button.addEventListener('focus', () => {
        if (button.dataset.nksbMore) return;
        local.chipFocus[button.dataset.nksbChip] = button.dataset.nksbValue;
        button.closest?.('[data-nksb-group]')?.querySelectorAll('[data-nksb-chip]').forEach(chip => { chip.tabIndex = chip === button ? 0 : -1; });
      });
    });
    document.querySelectorAll('[data-nksb-row]').forEach(row => {
      row.addEventListener('click', event => { if (event.target?.closest?.('[data-nksb-star], [data-nksb-play]')) return; select(Number(row.dataset.nksbRow), true); document.getElementById('nksb-results')?.focus({ preventScroll: true }); });
      row.addEventListener('dblclick', () => load('stay'));
    });
    document.querySelectorAll('[data-nksb-star]').forEach(button => button.addEventListener('click', event => { event.stopPropagation?.(); favorite(local.items[Number(button.dataset.nksbStar)]); }));
    document.querySelectorAll('[data-nksb-play]').forEach(button => button.addEventListener('click', event => {
      event.stopPropagation?.(); const index = Number(button.dataset.nksbPlay);
      if (filters().selected !== index) select(index, false);
      preview(local.items[index], true); document.getElementById('nksb-results')?.focus({ preventScroll: true });
    }));
    document.querySelectorAll('[data-nksb-crumb]').forEach(button => button.addEventListener('click', () => up(button.dataset.nksbCrumb)));
    on('nksb-vendor', event => setVendor(event.target.value), 'change');
    on('nksb-search', event => { local.query = event.target.value; changed(120); }, 'input');
    on('nksb-favorites', () => { local.favorites = !local.favorites; changed(); });
    on('nksb-autopreview', event => { local.autoPreview = event.target.checked; if (!local.autoPreview) stopPreview(); }, 'change');
    on('nksb-clear', clearFilters); on('nksb-clear-empty', clearFilters); on('nksb-retry', () => search({ keep: false }));
    on('nksb-rescan', rescan); on('nksb-rescan-empty', rescan);
    on('nksb-more-rows', () => search({ append: true }));
    bindDetail();
    // A failed browse stays failed until the user searches again; retrying on every repaint would spin forever.
    if ((!local.loaded || local.refreshNeeded) && !local.searching && !local.error && local.timer === null && local.refreshTimer === null) queueMicrotask(() => search({ keep: false }));
  }

  // ---- Keyboard -----------------------------------------------------------------------------
  const groupOrder = () => Array.from(document.querySelectorAll('[data-nksb-group]'));
  function focusGroup(group, last = false) {
    const chips = Array.from(group?.querySelectorAll('[data-nksb-chip]') || []);
    (chips.find(chip => chip.tabIndex === 0 && !chip.dataset.nksbMore) || (last ? chips.at(-1) : chips[0]))?.focus();
  }
  const focusResults = () => document.getElementById('nksb-results')?.focus();
  const focusProducts = () => document.getElementById('nksb-products')?.focus();
  function focusSearch(selectAll = false) { const input = document.getElementById('nksb-search'); input?.focus(); if (selectAll) input?.select?.(); }
  function typeahead(character) {
    clearTimeout(local.typeaheadTimer);
    local.typeahead += character.toLocaleLowerCase();
    local.typeaheadTimer = setTimeout(() => { local.typeahead = ''; }, 800);
    const list = productList(), text = local.typeahead;
    let index = list.findIndex(product => product.name.toLocaleLowerCase().startsWith(text));
    if (index < 0) index = list.findIndex(product => product.name.toLocaleLowerCase().includes(text));
    if (index >= 0) setProduct(index + 1);
  }
  function key(event) {
    if (!mounted() || event.isComposing) return false;
    const target = event.target, keyName = event.key, plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
    if ((event.metaKey || event.ctrlKey) && keyName?.toLowerCase() === 'f') { event.preventDefault(); focusSearch(true); return true; }
    if (keyName === 'Escape' && previewActive() && plain) { event.preventDefault(); stopPreview(); return true; }
    const editing = /INPUT|TEXTAREA|SELECT/.test(target?.tagName || '') || target?.isContentEditable;
    if (editing) {
      if (target.type === 'checkbox' && (keyName === ' ' || event.code === 'Space')) return true;
      if (target.id === 'nksb-search' && plain) {
        if (keyName === 'ArrowDown' || keyName === 'Enter') { event.preventDefault(); if (local.timer !== null) search({ keep: false }); focusResults(); return true; }
        if (keyName === 'Escape') { event.preventDefault(); if (local.query) { local.query = ''; target.value = ''; changed(); } else focusResults(); return true; }
      }
      if ((target.id === 'nksb-slot' || target.id === 'nksb-vendor') && plain && keyName === 'Escape') { event.preventDefault(); focusResults(); return true; }
      return false;
    }
    if (event.metaKey || event.ctrlKey) return false;
    const vertical = { ArrowUp: -1, ArrowDown: 1, PageUp: -12, PageDown: 12 };
    // Instruments / Effects tabs.
    if (target?.closest?.('[data-nksb-kind]') && plain) {
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(keyName)) {
        event.preventDefault();
        const kind = keyName === 'ArrowLeft' || keyName === 'Home' ? 'instrument' : 'effect';
        setKind(kind); document.getElementById(`nksb-kind-${kind}`)?.focus(); return true;
      }
      if (keyName === 'ArrowDown') { event.preventDefault(); focusProducts(); return true; }
      if (keyName === 'ArrowUp') { event.preventDefault(); document.getElementById('browser-tab-nks')?.focus(); return true; }
      return keyName === 'Enter' || keyName === ' ';
    }
    // Product list.
    if (target?.id === 'nksb-products') {
      const index = productIndex(), size = productList().length;
      if (plain && (vertical[keyName] || keyName === 'Home' || keyName === 'End')) {
        event.preventDefault();
        if (keyName === 'ArrowUp' && index === 0) { document.getElementById(`nksb-kind-${local.kind}`)?.focus(); return true; }
        setProduct(keyName === 'Home' ? 0 : keyName === 'End' ? size : index + vertical[keyName]); return true;
      }
      if (plain && (keyName === 'ArrowRight' || keyName === 'Enter')) { event.preventDefault(); if (local.timer !== null) search({ keep: false }); focusResults(); return true; }
      if (plain && (keyName === 'Escape' || keyName === 'Backspace')) { event.preventDefault(); setProduct(0, { immediate: true }); return true; }
      if (keyName === ' ' || event.code === 'Space') { event.preventDefault(); preview(current(), true); return true; }
      if (!event.altKey && keyName?.length === 1 && /\S/.test(keyName)) { event.preventDefault(); typeahead(keyName); return true; }
      return false;
    }
    // Filter chips: ← → within a group, ↑ ↓ between groups, Space/Return toggles.
    const chip = target?.closest?.('[data-nksb-chip]');
    if (chip && plain) {
      const group = chip.closest('[data-nksb-group]'), chips = Array.from(group?.querySelectorAll('[data-nksb-chip]') || []), index = chips.indexOf(chip);
      if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(keyName)) {
        event.preventDefault();
        const next = keyName === 'Home' ? 0 : keyName === 'End' ? chips.length - 1 : Math.max(0, Math.min(chips.length - 1, index + (keyName === 'ArrowRight' ? 1 : -1)));
        if (keyName === 'ArrowLeft' && index === 0) { focusProducts(); return true; }
        chips[next]?.focus(); return true;
      }
      if (keyName === 'ArrowUp' || keyName === 'ArrowDown') {
        event.preventDefault();
        const groups = groupOrder(), position = groups.indexOf(group), next = groups[position + (keyName === 'ArrowDown' ? 1 : -1)];
        if (next) focusGroup(next); else if (keyName === 'ArrowDown') focusResults(); else focusSearch();
        return true;
      }
      return keyName === 'Enter' || keyName === ' ';
    }
    // Preset results.
    const inResults = target?.id === 'nksb-results' || (!!target?.closest?.('#nksb-results') && !target.closest('button'));
    if (inResults) {
      const selected = filters().selected, total = local.items.length;
      if (plain && (vertical[keyName] || keyName === 'Home' || keyName === 'End')) {
        event.preventDefault();
        if (keyName === 'ArrowUp' && selected <= 0) { const groups = groupOrder(); if (groups.length) focusGroup(groups.at(-1)); else focusSearch(); return true; }
        if (total) select(keyName === 'Home' ? 0 : keyName === 'End' ? total - 1 : selected + vertical[keyName]);
        return true;
      }
      if (plain && keyName === 'ArrowLeft') { event.preventDefault(); focusProducts(); return true; }
      if (plain && keyName === 'ArrowRight') { event.preventDefault(); const button = document.getElementById('nksb-load'); (button && !button.disabled ? button : document.getElementById('nksb-preview'))?.focus(); return true; }
      if (keyName === 'Enter' && !event.metaKey && !event.ctrlKey) { event.preventDefault(); load(event.altKey ? 'new' : event.shiftKey ? 'open' : 'stay'); return true; }
      if (plain && (keyName === ' ' || event.code === 'Space')) { event.preventDefault(); preview(current(), true); return true; }
      if (plain && keyName?.toLowerCase() === 'f') { event.preventDefault(); favorite(); return true; }
      if (plain && keyName === 'Backspace') { event.preventDefault(); up(); return true; }
      return false;
    }
    if (target?.closest?.('.nksb-detail') && plain && keyName === 'ArrowLeft') { event.preventDefault(); focusResults(); return true; }
    if (target?.closest?.('button, summary') && (keyName === 'Enter' || keyName === ' ' || event.code === 'Space')) return true;
    return false;
  }

  // ---- Integration --------------------------------------------------------------------------
  function event(type, value = {}) {
    if (type === 'library.nks.progress') {
      const revisionChanged = value.revision !== undefined && value.revision !== local.lastRevision;
      local.status = value;
      if (revisionChanged) local.refreshNeeded = true;
      if (!mounted()) return;
      if (revisionChanged && !local.searching && local.refreshTimer === null) {
        // Bursts during indexing refresh results at most every 900 ms, keeping the selection.
        local.refreshTimer = setTimeout(() => { local.refreshTimer = null; if (mounted()) search({ keep: true }); }, value.indexing ? 900 : 60);
      } else if (!revisionChanged) {
        const status = document.querySelector('.nksb-status');
        // Replacing the row must not drop keyboard focus from its rescan button or issue details.
        const focus = status?.contains?.(document.activeElement) ? document.activeElement.id : '';
        if (status) status.outerHTML = renderStatus();
        document.getElementById('nksb-rescan')?.addEventListener('click', rescan);
        if (focus) document.getElementById(focus)?.focus({ preventScroll: true });
      }
      return;
    }
    if (type === 'library.results' && value.volumes) { local.refreshNeeded = true; if (mounted()) queueSearch(60); }
    if (type === 'sample.previewEnded') { local.previewGeneration++; local.previewPending = false; local.previewPath = ''; clearTimeout(local.previewTimer); clearTimeout(local.previewEnd); local.previewTimer = null; if (mounted()) updatePreviewMarks(); }
  }
  function contextItems({ target }) {
    const row = target?.closest?.('[data-nksb-row]'), product = target?.closest?.('[data-nksb-product]');
    if (product) {
      const index = Number(product.dataset.nksbProduct), name = index === 0 ? null : productList()[index - 1]?.name;
      return [{ header: true, title: name || (local.kind === 'effect' ? 'All effects' : 'All instruments') },
        { title: 'Show presets', key: '↩', action: () => { filters().product = name ?? null; filters().bank = null; filters().subBank = null; changed(); focusResults(); } },
        { title: 'Rescan NKS presets', action: rescan }];
    }
    if (!row) return null;
    const index = Number(row.dataset.nksbRow);
    if (filters().selected !== index) select(index, false);
    const item = current();
    if (!item) return null;
    const menuSlot = slotChoice().slot;
    const project = S().state.project, track = S().state.track, instrument = S().state.instrument;
    const loadHere = mode => () => {
      if (S().state.project !== project || S().state.track !== track || S().state.instrument !== instrument) { S().toast('The preset destination changed. Choose it again.', 'info'); return false; }
      return load(mode, item, item.effect ? menuSlot : null);
    };
    return [{ header: true, title: item.name },
      ...(item.effect ? [{ title: `Load into FX ${menuSlot + 1}`, key: '↩', enabled: !local.busy && item.available !== false, action: loadHere('stay') }]
        : [{ title: 'Load and keep browsing', key: '↩', enabled: !local.busy && item.available !== false, action: loadHere('stay') },
          { title: 'Load and edit controls', key: '⇧↩', enabled: !local.busy && item.available !== false, action: loadHere('open') },
          { title: 'Load as new instrument', key: '⌥↩', enabled: !local.busy && item.available !== false, action: loadHere('new') }]),
      { title: local.previewPath && local.previewPath === item.previewPath ? 'Stop preview' : 'Preview', key: 'Space', enabled: !!item.previewPath && item.available !== false, action: () => preview(item, true) },
      { separator: true },
      { title: item.favorite ? 'Remove from favorites' : 'Add to favorites', key: 'F', action: () => favorite(item) },
      { title: `Show all ${item.product} presets`, action: () => { filters().product = item.product; filters().bank = null; filters().subBank = null; changed(); } },
      ...(item.bank ? [{ title: `Show ${item.bank} presets`, action: () => { Object.assign(filters(), { product: item.product, bank: item.bank, subBank: null }); changed(); } }] : []),
      { title: 'Show in Finder', action: () => S().request('app.external', { path: item.path, reveal: true }) }];
  }
  function show(kind) { if (kind) setKind(kind); }
  // Hardware controllers (mk3.js): Product → Bank → Type → Character as single-choice encoder steps.
  const facetName = option => typeof option === 'string' ? option : option?.name ?? option?.value ?? '';
  const controller = {
    get kind() { return local.kind; }, setKind,
    toggleKind() { setKind(local.kind === 'instrument' ? 'effect' : 'instrument'); },
    value(group) {
      const value = filters();
      if (group === 'product') return value.product ?? 'All products';
      if (group === 'bank') return value.bank ?? 'All banks';
      const set = value[group]; if (!set?.size) return 'All';
      const first = [...set][0]; return subtypeLabel(first) + (set.size > 1 ? ` +${set.size - 1}` : '');
    },
    cycle(group, delta) {
      const step = Math.sign(delta); if (!step) return;
      if (group === 'product') { setProduct(productIndex() + step, { immediate: true }); return; }
      const value = filters(), options = (group === 'bank' ? local.facets.banks : local.facets[group] || []).map(facetName).filter(Boolean);
      const current = group === 'bank' ? value.bank : [...(value[group] || [])][0] ?? null, index = current === null ? -1 : options.indexOf(current), next = index + step;
      const chosen = next < 0 || next >= options.length ? null : options[next];
      if (group === 'bank') { value.bank = chosen; value.subBank = null; } else { value[group] = new Set(chosen ? [chosen] : []); if (group === 'types') value.subtypes.clear(); }
      changed();
    },
    // MK3 filter lists: the same facets as the panel, the applied value and a single-value choice (null = all).
    options(group) {
      if (group === 'product') return productList().map(product => ({ value: product.name, label: product.name, detail: product.vendor || '', count: product.count }));
      const list = group === 'bank' ? local.facets.banks : local.facets[group] || [];
      return list.map(option => ({ value: facetName(option), label: group === 'subtypes' ? option?.label || subtypeLabel(facetName(option)) : facetName(option), count: option?.count })).filter(option => option.value);
    },
    current(group) { const value = filters(); if (group === 'product') return value.product ?? null; if (group === 'bank') return value.bank ?? null; return value[group]?.size ? [...value[group]][0] : null; },
    choose(group, chosen) {
      if (group === 'product') { setProduct(chosen == null ? 0 : productList().findIndex(item => item.name === chosen) + 1, { immediate: true }); return; }
      const value = filters();
      if (group === 'bank') { value.bank = chosen ?? null; value.subBank = null; } else { value[group] = new Set(chosen != null ? [chosen] : []); if (group === 'types') value.subtypes.clear(); }
      changed();
    },
    list() { return { tab: 'nks', kind: local.kind, items: (resultsCurrent() ? local.items : []).map(item => ({ name: item.name, detail: [item.product, item.bank].filter(Boolean).join(' · '), favorite: !!item.favorite, preview: !!item.previewPath })), selected: filters().selected, total: local.total, offset: 0, busy: local.busy || local.searching, preview: !!local.previewPath, autoPreview: local.autoPreview, favorites: local.favorites, query: local.query, filters: activeFilterCount() }; },
    move(delta) { if (local.items.length) select(filters().selected + delta); },
    preview: () => preview(current(), true),
    load: (mode = 'stay') => load(mode),
    favorite: () => favorite(),
    toggleFavorites() { local.favorites = !local.favorites; changed(); },
    toggleAutoPreview() { local.autoPreview = !local.autoPreview; if (!local.autoPreview) stopPreview(); paint(); },
    clearFilters
  };
  window.STNKSBrowser = { render, bind, key, event, search: () => search({ keep: true }), stopPreview, contextItems, shortcuts, show, controller,
    focusSearch: () => focusSearch(), get previewActive() { return previewActive(); }, _local: local };
})();
