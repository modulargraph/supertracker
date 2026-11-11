/* Keyboard-first local sample, kit, NKS metadata, and plug-in browser. */
(() => {
  'use strict';
  const local = { tab: 'samples', pluginKind: 'instrument', pluginFormat: 'all', pluginQuery: '', pluginSelected: 0, slot: 0,
    marked: new Set(),
    selectionAnchor: null,
    initialized: false,
    pluginsLoaded: false,
    metadata: new Map(),
    pendingMetadata: new Set(),
    metadataRequests: new Map(),
    waveforms: new Map(),
    wavePending: new Set(),
    waveFailed: new Set(),
    waveformTimer: null,
    generation: 0,
    resultsKey: null,
    resultsStale: false,
    searchError: '',
    timer: null,
    backgroundTimer: null,
    backgroundDirty: false,
    searching: 0,
    progressTimer: null,
    filterGroups: {},
    facetQueries: {},
    facetFocus: {},
    rangeDrafts: new Map(),
    deferredPaint: false,
    rangeDeparture: false,
    inputPaintTimer: null,
    pointerHeld: false,
    inputGuardsInstalled: false,
    previewTimer: null,
    previewEndTimer: null,
    previewPending: false,
    previewGeneration: 0,
    previewPath: '',
    previewEnabled: true,
    busy: false,
    error: '',
    recent: [], stateReady: false, stateTimer: null, recentTimer: null, historyIndex: -1, hideOffline: false, offline: [] };
  // Insert slots per mixer channel (0...15), matching the audio host.
  const INSERT_SLOTS = 16;
  const app = () => window.ST;
  const html = value => app().escapeHTML(String(value ?? ''));
  function library() {
    const state = app().state;
    state.library ||= {};
    const defaults = { items: [], roots: [], query: '', category: '', kind: 'all', selected: 0, total: 0,
      scanning: false, favorites: false, offset: 0, limit: 150, categories: [], facets: {}, durationCoverage: {}, errors: [], rootReports: {}, rootErrors: {} };
    for (const [key, value] of Object.entries(defaults)) if (state.library[key] == null) state.library[key] = value;
    return state.library;
  }
  const activeTab = () => app().state.view === 'plugins' ? 'plugins' : local.tab;
  const nksBrowser = () => activeTab() === 'nks' ? window.STNKSBrowser : null;
  const kitBrowser = () => activeTab() === 'kits' ? window.STKitBrowser : null;
  function focusSearch() {
    document.getElementById(activeTab() === 'plugins'
        ? 'browser-plugin-search'
        : activeTab() === 'nks'
        ? 'nksb-search'
        : activeTab() === 'kits'
        ? 'kitb-search'
        : 'browser-search')?.focus({
      preventScroll: true
    });
  }
  const currentItem = () => resultsCurrent() ? library().items[library().selected] : undefined;
  const previewActive = () => !!(local.previewPath || local.previewPending || local.previewTimer !== null);
  const icon = kind => kind === 'nks' ? '◈' : '♫';
  function updateLibraryStatus(payload = {}) {
    const lib = library();
    if (Array.isArray(payload.errors)) lib.errors = payload.errors.filter(value => typeof value === 'string');
    const reports = Array.isArray(payload.rootReports)
        ? payload.rootReports
        : payload.rootReports && typeof payload.rootReports === 'object'
        ? Object.values(payload.rootReports)
        : [];
    for (const report of reports) if (report?.path) {
      lib.rootReports[report.path] = report;
      const root = lib.roots.find(value => value.path === report.path);
      if (root) {
        root.scanStatus = report.status;
        root.scanErrors = report.errors || [];
        if (['complete', 'empty'].includes(report.status) && report.matched != null) root.indexedCount = report.matched;
      }
    }
    const roots = Array.isArray(payload.roots) ? payload.roots : lib.roots;
    for (const root of roots) {
      if (Array.isArray(payload.roots) && root.scanStatus && !reports.some(value => value.path === root.path)) {
        lib.rootReports[root.path] = {
          ...(lib.rootReports[root.path] || {}),
          path: root.path,
          status: root.scanStatus,
          errors: root.scanErrors || [],
          ...(root.indexedCount != null ? { matched: root.indexedCount } : {})
        };
      }
      const report = reports.find(value => value.path === root.path) || lib.rootReports[root.path];
      const status = report?.status || root.scanStatus;
      const errors = [...new Set([...(root.scanErrors || []), ...(report?.errors || [])].filter(value => typeof value === 'string'))];
      if (status === 'complete' || status === 'empty') delete lib.rootErrors[root.path];
      else if (errors.length) lib.rootErrors[root.path] = errors;
    }
  }
  function rootStatus(root) {
    const lib = library(), report = lib.rootReports[root.path];
    const status = report?.status || root.scanStatus || 'unchecked';
    const clean = status === 'complete' || status === 'empty';
    const errors = clean
        ? []
        : [...new Set([...(lib.rootErrors[root.path] || []), ...(root.scanErrors || []), ...(report?.errors || []),
            ...lib.errors.filter(message => message.includes(root.path))])];
    const failed = status === 'error' || errors.length > 0;
    const count = Number(root.indexedCount ?? report?.matched ?? 0);
    const label = failed
        ? `Indexing error${count ? ` · ${count.toLocaleString()} indexed` : ''}`
        : root.available === false
        ? 'Folder unavailable'
        : status === 'scanning' || status === 'pending'
        ? 'Indexing…'
        : status === 'empty'
        ? 'No supported audio found'
        : status === 'complete'
        ? `${count.toLocaleString()} indexed`
        : `${count ? `${count.toLocaleString()} indexed · ` : ''}Not checked`;
    return { status, errors, failed, label };
  }
  function renderRoot(root) {
    const report = rootStatus(root);
    const problems = report.errors.length
        ? report.errors
        : report.failed
        ? ['The folder could not be fully indexed. Choose it again to retry access.']
        : [];
    return `<div class="browser-root ${report.failed ? 'has-error' : ''}" title="${html(root.path)}"><span class="browser-root-dot ${report.failed || root.available === false ? 'unavailable' : ''}">`
        + `</span><div class="browser-root-copy"><strong>${html(root.name || root.path.split('/').pop())}</strong><small>${html(report.label)}</small>${problems.length ? `<details class="browser-root-problem"><summary id="browser-root-details-${encodeURIComponent(root.path)}">Read / index details</summary>${problems.map(message => `<p>${html(message)}</p>`).join('')}</details>`
        + `<button class="browser-root-rechoose" id="browser-root-rechoose-${encodeURIComponent(root.path)}" data-browser-rechoose-root="${html(root.path)}">Choose `
        + `folder again…</button>` : ''}</div>`
        + `<button class="browser-root-remove" id="browser-root-remove-${encodeURIComponent(root.path)}" data-browser-remove-root="${html(root.path)}"`
        + ` aria-label="Remove ${html(root.name || root.path)} from library" title="Remove location from library (files stay on disk)">×</button></div>`;
  }
  function renderLibraryErrors(lib) {
    const failed = lib.roots.filter(root => rootStatus(root).failed);
    const errors = [...new Set([...lib.errors, ...failed.flatMap(root => rootStatus(root).errors)])];
    if (!failed.length && !errors.length) return '';
    return `<div class="browser-library-notice" role="status"><details><summary id="browser-library-error-details">${failed.length
        ? `${failed.length} library folder${failed.length === 1 ? '' : 's'} need${failed.length === 1 ? 's' : ''} attention`
        : `${errors.length} library indexing issue${errors.length === 1 ? '' : 's'}`}</summary>`
        + `<div>${errors.map(message => `<p>${html(message)}</p>`).join('')}</div></details><button id="browser-rechoose-folder">Choose folder again…</button></div>`;
  }
  function paint() {
    const focused = document.activeElement;
    // Replacing an active number field loses its unfinished native text, including invalid partial values.
    if (local.rangeDeparture
        || local.pointerHeld
        || (focused?.dataset?.browserRange && local.rangeDrafts.has(focused.id))) { local.deferredPaint = true; return; }
    local.deferredPaint = false;
    const id = focused?.id;
    const start = focused?.selectionStart;
    const end = focused?.selectionEnd;
    const inBrowser = !!focused?.closest?.('.browser-workspace') || id?.startsWith('browser-');
    const field = focused?.dataset?.browserFacet || focused?.dataset?.browserFacetSearch;
    const group = focused?.closest?.('[data-browser-filter-group]')?.dataset?.browserFilterGroup || {
      sampleTypes: 'length',
      extensions: 'tempo',
      roots: 'locations'
    }[field] || field;
    const fallbacks = [group && `browser-facet-search-${group}`, group && `browser-group-${group}`,
        activeTab() === 'plugins' ? 'browser-plugin-list' : 'browser-results-list',
        activeTab() === 'plugins' ? 'browser-plugin-search' : 'browser-search'];
    const scroll = ['browser-results-list', 'browser-plugin-list'].map(key => [key, document.getElementById(key)?.scrollTop || 0]);
    app().refresh();
    scroll.forEach(([key, value]) => { const node = document.getElementById(key); if (node) node.scrollTop = value; });
    const focusable = node => node
        && !node.disabled
        && !node.hidden
        && (typeof node.getClientRects !== 'function' || node.getClientRects().length > 0);
    const same = id && document.getElementById(id);
    const next = focusable(same) ? same : inBrowser ? fallbacks.filter(Boolean).map(key => document.getElementById(key)).find(focusable) : null;
    if (next) {
      next.focus({ preventScroll: true });
      if (next === same && typeof start === 'number' && next.setSelectionRange) next.setSelectionRange(start, end);
    }
  }
  function paintAfterInputDeparture() {
    clearTimeout(local.inputPaintTimer);
    local.inputPaintTimer = setTimeout(() => {
      local.inputPaintTimer = null; local.rangeDeparture = false;
      if (local.pointerHeld || !local.deferredPaint) return;
      if (document.querySelector('.browser-workspace')) paint(); else local.deferredPaint = false;
    }, 0);
  }
  function clearRangeDrafts(field) {
    for (const id of local.rangeDrafts.keys()) if (!field || id.startsWith(`browser-range-${field}-`)) local.rangeDrafts.delete(id);
  }
  function failure(error) { local.error = error?.message || String(error); app().toast(local.error, 'error'); paint(); }
  async function call(command, payload) {
    try { return await app().request(command, payload); }
    catch (error) { failure(error); return null; }
  }
  const bytes = n => !n ? '' : n > 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`;
  function render() {
    const lib = library();
    if (local.resultsKey === null) local.resultsKey = searchKey();
    const tab = activeTab();
    return `<section class="browser-workspace" aria-label="Sound browser">
      <header class="browser-heading"><div><h1>Library</h1><span class="browser-heading-context">${tab === 'nks'
          ? 'NKS instruments and effects by product, type and character'
          : tab === 'kits'
          ? 'Your kits and Maschine expansion groups'
          : 'Local audio and plug-ins'}</span></div>
        <div class="browser-tabs" role="tablist" aria-label="Browser type">
          <button class="browser-tab ${tab === 'samples' ? 'active' : ''}" id="browser-tab-samples" data-browser-tab="samples" tabindex="${tab === 'samples' ? 0 : -1}"`
              + ` role="tab" aria-selected="${tab === 'samples'}">Samples</button>
          <button class="browser-tab ${tab === 'kits' ? 'active' : ''}" id="browser-tab-kits" data-browser-tab="kits" tabindex="${tab === 'kits' ? 0 : -1}"`
              + ` role="tab" aria-selected="${tab === 'kits'}">Kits &amp; Groups</button>
          <button class="browser-tab ${tab === 'nks' ? 'active' : ''}" id="browser-tab-nks" data-browser-tab="nks" tabindex="${tab === 'nks' ? 0 : -1}"`
              + ` role="tab" aria-selected="${tab === 'nks'}">NKS presets</button>
          <button class="browser-tab ${tab === 'plugins' ? 'active' : ''}" id="browser-tab-plugins" data-browser-tab="plugins" tabindex="${tab === 'plugins' ? 0 : -1}"`
              + ` role="tab" aria-selected="${tab === 'plugins'}">Plug-ins <span>${(app().state.plugins || []).length}</span></button>
        </div>
      </header>
      ${tab === 'plugins'
          ? renderPlugins()
          : tab === 'nks'
          ? window.STNKSBrowser?.render() || '<div class="browser-empty">NKS browser unavailable.</div>'
          : tab === 'kits'
          ? window.STKitBrowser?.render() || '<div class="browser-empty">Kit browser unavailable.</div>'
          : renderSamples(lib)}
      <footer class="browser-shortcuts">${tab === 'kits' && window.STKitBrowser ? window.STKitBrowser.shortcuts() : tab === 'nks' && window.STNKSBrowser ? window.STNKSBrowser.shortcuts() : `<span><kbd>⌘ F</kbd> Search</span><span><kbd>↑ ↓</kbd> Choices</span><span><kbd>← →</kbd> Panels</span><span>`
          + `<kbd>↵</kbd> Load</span>${tab === 'samples' ? `<span><kbd>⌥ ↵</kbd> New instrument</span><span><kbd>Space</kbd> Preview / select</span>`
          + `<span><kbd>F</kbd> Favorite</span><span><kbd>⌥ ↑ ↓</kbd> Recent</span>` : ''}<span>`
          + `<kbd>Tab</kbd> Groups</span>`}</footer>
    </section>`;
  }
  const arrayFilters = ['categories','collections','roots','packs','keys','sampleTypes','extensions','tags'];
  const filterNames = {
    categories: 'Category',
    collections: 'Source',
    roots: 'Location',
    packs: 'Pack',
    keys: 'Key',
    sampleTypes: 'Type',
    extensions: 'Format',
    tags: 'Tag',
    duration: 'Length',
    bpm: 'Tempo'
  };
  const typeNames = {loop:'Loops',oneshot:'One-shots',unknown:'Unknown'};
  function filters() {
    const lib=library();lib.filters ||= {};
    for(const field of arrayFilters)if(!Array.isArray(lib.filters[field]))lib.filters[field]=[];
    for(const field of ['duration','bpm'])if(!lib.filters[field]||typeof lib.filters[field]!=='object')lib.filters[field]={};
    if(lib.category){if(!lib.filters.categories.includes(lib.category))lib.filters.categories.push(lib.category);lib.category='';}
    return lib.filters;
  }
  function rangeIsActive(range){return Number.isFinite(range.min)||Number.isFinite(range.max)||range.includeUnknown===true;}
  function filterPayload() {
    const values = filters(),
      result = {};
    for (const field of arrayFilters) if (values[field].length) result[field] = [...values[field]];
    for (const field of ['duration', 'bpm']) if (rangeIsActive(values[field])) result[field] = { ...values[field] };
    return result;
  }
  function searchKey() {
    const lib = library();
    return JSON.stringify({
      query: lib.query,
      kind: lib.kind,
      filters: filterPayload(),
      favorites: !!lib.favorites,
      offset: lib.offset,
      limit: lib.limit,
      ...(local.hideOffline ? { hideOffline: true } : {})
    });
  }
  // Remembered browser state: the last query/filters and recent searches survive restarts (native browser-state.json).
  function cleanFilters(value){
    const result={};if(!value||typeof value!=='object')return result;
    for (const field of arrayFilters) if (Array.isArray(value[field])) {
      const list = value[field].filter(item => typeof item === 'string' && item && item.length <= 2048).slice(0, 128);
      if (list.length) result[field] = list;
    }
    for (const field of ['duration', 'bpm']) {
      const range = value[field];
      if (!range || typeof range !== 'object') continue;
      const clean = {};
      for (const bound of ['min', 'max']) if (Number.isFinite(range[bound]) && range[bound] >= 0) clean[bound] = range[bound];
      if (range.includeUnknown === true) clean.includeUnknown = true;
      if (Object.keys(clean).length) result[field] = clean;
    }
    return result;
  }
  function cleanEntry(value) {
    if (!value || typeof value !== 'object') return null;
    return {
      query: typeof value.query === 'string' ? value.query.slice(0, 1024) : '',
      kind: ['all', 'sample', 'nks'].includes(value.kind) ? value.kind : 'all',
      filters: cleanFilters(value.filters),
      favorites: value.favorites === true
    };
  }
  const recentKey=entry=>JSON.stringify([entry.query.trim().toLocaleLowerCase(),entry.kind,entry.filters,!!entry.favorites]);
  function persistableState() {
    const lib = library();
    return {
      version: 1,
      query: lib.query,
      kind: lib.kind,
      filters: filterPayload(),
      favorites: !!lib.favorites,
      hideOffline: !!local.hideOffline,
      previewEnabled: local.previewEnabled,
      recent: local.recent.slice(0, 12),
      ...(window.STKitBrowser ? { kits: window.STKitBrowser.settings() } : {})
    };
  }
  function saveState(delay=800){
    if(!local.stateReady)return;
    clearTimeout(local.stateTimer);
    local.stateTimer = setTimeout(() => {
      local.stateTimer = null;
      Promise.resolve(app().request('library.state.set', { state: persistableState() })).catch(() => {});
    }, delay);
  }
  async function restoreState(){
    const before=searchKey(), beforeKits=JSON.stringify(window.STKitBrowser?.settings());
    let saved=null;try{saved=await app().request('library.state.get',{});}catch{}
    if(saved&&typeof saved==='object'){
      // Searches remembered while the state loaded stay newest.
      const seen=new Set();
      local.recent = [...local.recent,
          ...(Array.isArray(saved.recent) ? saved.recent : []).map(cleanEntry).filter(Boolean)].filter(entry => !seen.has(recentKey(entry))
              && seen.add(recentKey(entry))).slice(0,
          12);
      // The user may already have typed while the state loaded; never overwrite that.
      if(searchKey()===before){
        const entry=cleanEntry(saved),lib=library();
        if(entry){applyEntry(entry,false);}
        local.hideOffline=saved.hideOffline===true;
        if(typeof saved.previewEnabled==='boolean')local.previewEnabled=saved.previewEnabled;
        lib.offset=0;lib.selected=0;
      }
      if(saved.kits && beforeKits===JSON.stringify(window.STKitBrowser?.settings()))window.STKitBrowser?.restoreSettings(saved.kits);
    }
    local.stateReady=true;
  }
  function applyEntry(entry,search=true){
    const lib=library();clearRangeDrafts();
    for(const field of arrayFilters)filters()[field]=[...(entry.filters[field]||[])];
    filters().duration={...(entry.filters.duration||{})};filters().bpm={...(entry.filters.bpm||{})};
    lib.query=entry.query;lib.kind=entry.kind;lib.favorites=entry.favorites;lib.category='';
    if(search)changedFilter();
  }
  function rememberSearch(){
    clearTimeout(local.recentTimer);local.recentTimer=null;
    // A search recalled from history is already listed; reordering it would disturb Option-Up/Down.
    const lib=library();if(!resultsCurrent()||!lib.total||local.historyIndex>=0)return;
    const entry={query:lib.query.trim(),kind:lib.kind,filters:filterPayload(),favorites:!!lib.favorites};
    if(!entry.query&&!Object.keys(entry.filters).length&&!entry.favorites&&entry.kind==='all')return;
    const key=recentKey(entry);
    if(local.recent[0]&&recentKey(local.recent[0])===key)return;
    local.recent=[entry,...local.recent.filter(value=>recentKey(value)!==key)].slice(0,12);
    saveState(200);
    if(document.querySelector('.browser-workspace'))paint();
  }
  function recentLabel(entry){
    const parts=[];if(entry.query.trim())parts.push(`“${entry.query.trim()}”`);
    for(const field of arrayFilters)for(const value of entry.filters[field]||[])parts.push(optionLabel(field,value));
    for(const field of ['duration','bpm'])if(entry.filters[field])parts.push(rangeLabel(field,entry.filters[field]));
    if(entry.kind!=='all')parts.push(entry.kind==='nks'?'NKS':'Samples');if(entry.favorites)parts.push('★');
    return parts.join(' · ')||'All sounds';
  }
  function applyRecent(index){const entry=local.recent[index];if(!entry)return;applyEntry(cleanEntry(entry));local.historyIndex=index;}
  function resultsCurrent(){return !local.resultsStale&&(local.resultsKey===null||local.resultsKey===searchKey());}
  function invalidateResults(key){
    if(local.resultsKey===key)return false;
    const changed=!local.resultsStale;local.resultsStale=true;local.marked.clear();local.selectionAnchor=null;
    clearTimeout(local.waveformTimer);local.waveformTimer=null;
    if(previewActive())stopPreview();
    return changed;
  }
  function activeFilterCount(){return Object.values(filterPayload()).reduce((sum,value)=>sum+(Array.isArray(value)?value.length:1),0);}
  function changedFilter(){const lib=library();lib.offset=0;lib.selected=0;local.historyIndex=-1;queueSearch(0);paint();saveState();}
  function toggleFilter(field, value) {
    if (!arrayFilters.includes(field)) return;
    const list = filters()[field],
      index = list.indexOf(value);
    if (index >= 0) {
      list.splice(index, 1);
    } else {
      if (list.length >= 128) {
        app().toast('Choose up to 128 values in one filter.', 'info');
        return;
      }
      list.push(value);
    }
    changedFilter();
  }
  function clearFilters(includeQuery = true) {
    const lib = library();
    clearRangeDrafts();
    for (const field of arrayFilters) filters()[field] = [];
    filters().duration = {};
    filters().bpm = {};
    lib.category = '';
    lib.kind = 'all';
    lib.favorites = false;
    if (includeQuery) lib.query = '';
    changedFilter();
  }
  function prettyKey(value) {
    if (value === 'unknown') return 'Unknown key';
    return value.replace(/([A-G])b/g, '$1♭').replace(/#/g, '♯').replace(/minor/ig, ' minor').replace(/major/ig, ' major').replace(/\s+/g, ' ').trim();
  }
  function optionLabel(field, value) {
    if (field === 'sampleTypes') return typeNames[value] || value;
    if (field === 'keys') return prettyKey(value);
    if (field === 'extensions') return value.toUpperCase();
    if (field === 'roots') return library().roots.find(root => root.path === value)?.name || value.split('/').filter(Boolean).pop() || value;
    return value;
  }
  function facetOptions(field){
    const lib=library(),selected=filters()[field],raw=lib.facets?.[field]||(field==='categories'?lib.categories:field==='packs'?lib.packs:null)||[];
    const options = raw.map(item => typeof item === 'string' ? { value: item, label: optionLabel(field, item), count: null } : {
      value: String(item.value ?? item.name ?? ''),
      label: ['keys', 'sampleTypes', 'extensions'].includes(field)
          ? optionLabel(field, String(item.value ?? item.name ?? ''))
          : item.label || optionLabel(field, String(item.value ?? item.name ?? '')),
      count: item.count
    });
    if (field === 'sampleTypes') for (const value of ['loop', 'oneshot', 'unknown']) if (!options.some(item => item.value === value)) options.push({
      value,
      label: typeNames[value],
      count: 0
    });
    for(const value of selected)if(!options.some(item=>item.value===value))options.push({value,label:optionLabel(field,value),count:0});
    return options.filter(item=>item.value).sort((a,b)=>Number(selected.includes(b.value))-Number(selected.includes(a.value))
        ||(field==='categories'?(Number(b.count)||0)-(Number(a.count)||0):0)
        ||a.label.localeCompare(b.label));
  }
  function filterOptions(field){
    const query = (local.facetQueries[field] || '').trim().toLocaleLowerCase(),
      options = facetOptions(field).filter(item => !query || `${item.label} ${item.value}`.toLocaleLowerCase().includes(query)),
      selected = filters()[field];
    const focusValue=options.slice(0,160).some(item=>item.value===local.facetFocus[field])?local.facetFocus[field]:options[0]?.value;
    return `<div class="browser-facet-options ${field==='keys'||field==='extensions'?'compact':''}" data-facet-options="${field}">${options.slice(0,160).map(item=>`<button type="button" id="browser-facet-${field}-${encodeURIComponent(item.value)}" class="browser-facet-option ${selected.includes(item.value)?'chosen':''}"`
        + ` role="checkbox" tabindex="${item.value===focusValue?0:-1}" aria-checked="${selected.includes(item.value)}" data-browser-facet="${field}"`
        + ` data-browser-value="${html(item.value)}" title="${html(item.value)}"><span class="browser-check" aria-hidden="true">${selected.includes(item.value)?'✓':''}</span>`
        + `<span>${html(item.label)}</span>${item.count!=null?`<small>${Number(item.count).toLocaleString()}</small>`:''}</button>`).join('')||'<p class="browser-facet-empty">'
            +(query?'No matches in this list.':'Available choices appear after indexing.')
            +'</p>'}${options.length>160?'<p class="browser-facet-empty">Type above to narrow these choices.</p>':''}</div>`;
  }
  function facetGroup(field,label,searchable=false,open=false){
    const count=filters()[field].length,isOpen=local.filterGroups[field]??open;
    return `<details class="browser-facet" data-browser-filter-group="${field}" ${isOpen?'open':''}><summary id="browser-group-${field}">`
        + `<span>${label}</span>${count?`<small>${count} selected</small>`:''}</summary>${searchable?`<input class="browser-facet-search" id="browser-facet-search-${field}" data-browser-facet-search="${field}" type="search"`
        + ` value="${html(local.facetQueries[field]||'')}" placeholder="Find ${label.toLowerCase()}…" aria-label="Find ${label.toLowerCase()} filter"`
        + ` autocomplete="off">`:''}${filterOptions(field)}</details>`;
  }
  function rangeLabel(field, value) {
    const unit = field === 'duration' ? ' s' : ' BPM',
      bounds = Number.isFinite(value.min) && Number.isFinite(value.max)
          ? `${value.min}–${value.max}`
          : Number.isFinite(value.min)
          ? `${value.min}+`
          : Number.isFinite(value.max)
          ? `≤ ${value.max}`
          : 'Unknown';
    return bounds + (bounds === 'Unknown' ? '' : unit) + (bounds !== 'Unknown' && value.includeUnknown ? ' + unknown' : '');
  }
  function rangeControls(field){
    const value=filters()[field],hasBounds=Number.isFinite(value.min)||Number.isFinite(value.max),unit=field==='duration'?'seconds':'BPM';
    const draft = bound => html(local.rangeDrafts.get(`browser-range-${field}-${bound}`) ?? value[bound] ?? '');
    return `<div class="browser-range-fields"><label><span>Min ${unit}</span><input id="browser-range-${field}-min" data-browser-range="${field}"`
        + ` data-bound="min" type="number" min="0" max="${field==='duration'?86400:999}" step="${field==='duration'?'.1':'1'}" value="${draft('min')}"`
        + ` placeholder="Any"></label><span aria-hidden="true">–</span><label><span>Max ${unit}</span><input id="browser-range-${field}-max"`
        + ` data-browser-range="${field}" data-bound="max" type="number" min="0" max="${field==='duration'?86400:999}" step="${field==='duration'?'.1':'1'}"`
        + ` value="${draft('max')}" placeholder="Any"></label></div><label class="browser-unknown-choice"><input id="browser-unknown-${field}"`
        + ` data-browser-range-unknown="${field}" type="checkbox" ${value.includeUnknown?'checked':''}>${hasBounds
            ?'Also show unknown '+(field==='duration'?'lengths':'tempos')
            :'Unknown '+(field==='duration'?'lengths':'tempos')+' only'}</label>`;
  }
  function renderFilterPanel(lib){
    const duration = filters().duration,
      typeCount = filters().sampleTypes.length + (rangeIsActive(duration) ? 1 : 0),
      tempoCount = filters().extensions.length + (rangeIsActive(filters().bpm) ? 1 : 0);
    const preset = JSON.stringify(duration),
      presets = [['Any', {}], ['≤ 1 s', { max: 1 }], ['1–5 s', { min: 1, max: 5 }], ['5–30 s', { min: 5, max: 30 }], ['30 s +', { min: 30 }]];
    return `<aside class="browser-sidebar panel" aria-label="Library filters" data-scroll="library-filters"><div class="panel-head">`
        + `<h2>Filter sounds</h2><button class="browser-icon-btn" id="browser-add-root" aria-label="Add sample folder" title="Add sample folder">＋</button>`
        + `</div><div class="browser-kind-options" aria-label="Sound kind">${[['all','All'],['sample','Samples'],['nks','NKS']].map(([value,name])=>`<button type="button" id="browser-kind-${value}" tabindex="${lib.kind===value?0:-1}" data-browser-kind="${value}" aria-pressed="${lib.kind===value}"`
        + ` class="${lib.kind===value?'chosen':''}">${name}</button>`).join('')}</div>`
        + `<button class="browser-favorites-choice ${lib.favorites?'chosen':''}" id="browser-favorites" aria-pressed="${!!lib.favorites}">`
        + `<span>${lib.favorites?'★':'☆'}</span> Favorites only</button>
      ${local.recent.length?`<details class="browser-facet browser-recent" data-browser-filter-group="recent" ${local.filterGroups.recent??true?'open':''}>`
          + `<summary id="browser-group-recent"><span>Recent searches</span><small>⌥↑↓</small></summary><div class="browser-facet-options"`
          + ` data-facet-options="recent">${local.recent.map((entry,index)=>`<button type="button" id="browser-recent-${index}" class="browser-facet-option browser-recent-option" tabindex="${index===0?0:-1}"`
          + ` data-browser-recent="${index}" title="${html(recentLabel(entry))}"><span class="browser-check" aria-hidden="true">↺</span>`
          + `<span>${html(recentLabel(entry))}</span></button>`).join('')}<button `
          + `type="button" id="browser-recent-clear" class="browser-recent-clear">Clear recent searches</button></div></details>`:''}
      ${facetGroup('collections','Source',false,true)}${facetGroup('categories','Category',false,true)}${facetGroup('packs','Pack',true)}${facetGroup('keys','Key',true)}
      <details class="browser-facet" data-browser-filter-group="length" ${local.filterGroups.length?'open':''}>`
          + `<summary id="browser-group-length"><span>Type & length</span>${typeCount?`<small>${typeCount} active</small>`:''}</summary>`
          + `<div class="browser-facet-caption">Sample type</div>${filterOptions('sampleTypes')}<div class="browser-facet-caption">Length</div>`
          + `<div class="browser-duration-presets">${presets.map(([label,value],index)=>`<button type="button" id="browser-duration-preset-${index}" data-browser-duration-preset="${index}" aria-pressed="${preset===JSON.stringify(value)}">${label}</button>`).join('')}</div>${rangeControls('duration')}<p `
          + `class="browser-facet-help">Lengths are read in the background. Unknown means metadata is not available yet.</p></details>
      <details class="browser-facet" data-browser-filter-group="tempo" ${local.filterGroups.tempo?'open':''}><summary id="browser-group-tempo">`
          + `<span>Tempo & format</span>${tempoCount?`<small>${tempoCount} active</small>`:''}</summary>${rangeControls('bpm')}<div class="browser-facet-caption">File `
          + `format</div>${filterOptions('extensions')}</details>${facetGroup('tags','Tags',true)}
      <details class="browser-facet browser-location-group" data-browser-filter-group="locations" ${local.filterGroups.locations?'open':''}>`
          + `<summary id="browser-group-locations"><span>Locations</span>${filters().roots.length?`<small>${filters().roots.length} selected</small>`:''}</summary>`
          + `<div class="browser-roots">${lib.roots.length?lib.roots.map(root=>`<div class="browser-root-filter"><button type="button" id="browser-root-filter-${encodeURIComponent(root.path)}" class="browser-root-select"`
          + ` role="checkbox" aria-checked="${filters().roots.includes(root.path)}" data-browser-facet="roots" data-browser-value="${html(root.path)}"`
          + ` aria-label="Filter to ${html(root.name||root.path)}"><span class="browser-check" aria-hidden="true">${filters().roots.includes(root.path)?'✓':''}</span>`
          + `</button>${renderRoot(root)}</div>`).join(''):'<p class="browser-small muted">Add your Maschine expansions, Loopcloud folder, or any sample collection.</p>'}</div>`
          + `<button class="btn browser-scan-btn" id="browser-scan">${lib.scanning?'Stop indexing':'Rescan library'}</button>`
          + `</details>${lib.scanning
              ?`<div class="browser-indexing"><span class="browser-pulse"></span> Indexing · ${(lib.count||0).toLocaleString()} sounds</div>`
              :''}</aside>`;
  }
  function renderFilterChips(lib){
    const chips = [];
    if (lib.query.trim()) chips.push({ field: 'query', value: '', label: `Text: ${lib.query}` });
    if (lib.kind !== 'all') chips.push({ field: 'kind', value: '', label: lib.kind === 'nks' ? 'NKS presets' : 'Samples' });
    if (lib.favorites) chips.push({ field: 'favorites', value: '', label: 'Favorites' });
    for (const field of arrayFilters) for (const value of filters()[field]) chips.push({
      field,
      value,
      label: `${filterNames[field]}: ${optionLabel(field, value)}`
    });
    for (const field of ['duration', 'bpm']) if (rangeIsActive(filters()[field])) chips.push({
      field,
      value: '',
      label: `${filterNames[field]}: ${rangeLabel(field, filters()[field])}`
    });
    if(!chips.length)return '';
    return `<div class="browser-active-filters" aria-label="Active search filters"><div>${chips.map((chip,index)=>`<button type="button" id="browser-filter-chip-${index}" data-browser-remove-filter="${chip.field}" data-browser-remove-value="${html(chip.value)}"`
        + ` title="${html(chip.label)}" aria-label="Remove filter ${html(chip.label)}"><span>${html(chip.label)}</span>`
        + `<span aria-hidden="true">×</span></button>`).join('')}</div>`
        + `<button type="button" id="browser-clear-all" class="browser-clear-all">Clear all</button></div>`;
  }
  function bindFilters(){
    document.querySelectorAll('[data-browser-filter-group]').forEach(detail => detail.addEventListener('toggle', () => {
      local.filterGroups[detail.dataset.browserFilterGroup] = detail.open;
    }));
    document.querySelectorAll('[data-browser-facet]').forEach(button=>{
      button.addEventListener('click',()=>toggleFilter(button.dataset.browserFacet,button.dataset.browserValue));
      button.addEventListener('focus', () => {
        local.facetFocus[button.dataset.browserFacet] = button.dataset.browserValue;
        const group = button.closest?.('[data-facet-options]');
        group?.querySelectorAll('[data-browser-facet]').forEach(option => {
          option.tabIndex = option === button ? 0 : -1;
        });
      });
    });
    document.querySelectorAll('[data-browser-facet-search]').forEach(input=>{
      input.addEventListener('input',()=>{local.facetQueries[input.dataset.browserFacetSearch]=input.value;paint();});
      input.addEventListener('keydown', event => {
        if (event.isComposing || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
        if (event.key === 'ArrowDown') {
          event.preventDefault();
          document.querySelector(`[data-facet-options="${input.dataset.browserFacetSearch}"] button`)?.focus();
        }
        if (event.key === 'Escape') {
          if (previewActive()) return;
          event.preventDefault();
          event.stopPropagation();
          local.facetQueries[input.dataset.browserFacetSearch] = '';
          paint();
          document.getElementById(`browser-group-${input.dataset.browserFacetSearch}`)?.focus();
        }
      });
    });
    document.querySelectorAll('[data-browser-duration-preset]').forEach(button => button.addEventListener('click', () => {
      clearRangeDrafts('duration');
      filters().duration = [{}, { max: 1 }, { min: 1, max: 5 }, { min: 5, max: 30 }, { min: 30 }][Number(button.dataset.browserDurationPreset)];
      changedFilter();
    }));
    document.querySelectorAll('[data-browser-range]').forEach(input=>{
      input.addEventListener('input',()=>local.rangeDrafts.set(input.id,input.value));
      input.addEventListener('keydown',event=>{if(event.key==='Tab'){local.rangeDeparture=true;paintAfterInputDeparture();}});
      input.addEventListener('blur',()=>{local.rangeDrafts.delete(input.id);paintAfterInputDeparture();});
      input.addEventListener('change',()=>{
      local.rangeDrafts.delete(input.id);
      const field=input.dataset.browserRange,bound=input.dataset.bound,value=input.value.trim()===''?null:Number(input.value),range=filters()[field];
      if(value==null)delete range[bound];else if(Number.isFinite(value)&&value>=0)range[bound]=Math.min(value,
          field==='duration'?86400:999);else{app().toast('Enter a positive number or leave the range blank.',
          'info');paint();return;}
      if(Number.isFinite(range.min)&&Number.isFinite(range.max)&&range.min>range.max){if(bound==='min')range.max=range.min;else range.min=range.max;}
      changedFilter();
      });
    });
    document.querySelectorAll('[data-browser-range-unknown]').forEach(input => input.addEventListener('change', () => {
      filters()[input.dataset.browserRangeUnknown].includeUnknown = input.checked;
      changedFilter();
    }));
    document.querySelectorAll('[data-browser-remove-filter]').forEach(button=>button.addEventListener('click',()=>{
      const field = button.dataset.browserRemoveFilter,
        value = button.dataset.browserRemoveValue;
      if (arrayFilters.includes(field)) {
        filters()[field] = filters()[field].filter(item => item !== value);
      } else if (field === 'duration' || field === 'bpm') {
        clearRangeDrafts(field);
        filters()[field] = {};
      } else if (field === 'query') {
        library().query = '';
      } else if (field === 'kind') {
        library().kind = 'all';
      } else if (field === 'favorites') library().favorites = false;
      changedFilter();document.getElementById('browser-clear-all')?.focus({preventScroll:true});
    }));
    document.getElementById('browser-clear-all')?.addEventListener('click',()=>clearFilters());
    document.querySelectorAll('[data-browser-recent]').forEach(button=>{
      button.addEventListener('click',()=>applyRecent(Number(button.dataset.browserRecent)));
      button.addEventListener('focus', () => {
        button.closest?.('[data-facet-options]')?.querySelectorAll('[data-browser-recent]').forEach(option => {
          option.tabIndex = option === button ? 0 : -1;
        });
      });
    });
    document.getElementById('browser-recent-clear')?.addEventListener('click', () => {
      local.recent = [];
      local.historyIndex = -1;
      saveState(0);
      paint();
      document.getElementById('browser-search')?.focus({ preventScroll: true });
    });
    document.getElementById('browser-hide-offline')?.addEventListener('click',()=>{local.hideOffline=!local.hideOffline;changedFilter();});
  }
  function renderSamples(lib) {
    const selected = currentItem();
    const current = resultsCurrent();
    const showing = local.searchError
        ? local.searchError
        : !current
        ? 'Updating results…'
        : lib.total
        ? `${lib.offset + 1}–${Math.min(lib.offset + lib.items.length, lib.total)} of ${lib.total.toLocaleString()}`
        : '0 sounds';
    return `<div class="browser-layout">
      ${renderFilterPanel(lib)}
      <section class="browser-results panel" aria-label="Library results">
        <div class="browser-search-wrap"><span aria-hidden="true">⌕</span><input id="browser-search" class="browser-search" type="search"`
            + ` value="${html(lib.query)}" placeholder="Search sounds, packs, tags…" aria-label="Search local sample and NKS library" autocomplete="off"`
            + ` spellcheck="false"><kbd>⌘ F</kbd></div>
        ${renderFilterChips(lib)}<div class="browser-result-summary"><span role="status">${html(showing)}${activeFilterCount()?` · ${activeFilterCount()} filters`: ''}${local.searchError?' <button type="button" id="browser-search-retry">Retry search</button>':''}</span>`
            + `<label class="browser-audition-toggle"><input id="browser-audition" type="checkbox" ${local.previewEnabled ? 'checked' : ''}> Auto preview</label></div>
        ${renderLibraryErrors(lib)}${renderOffline()}
        <div class="browser-result-header" aria-hidden="true"><span>Sound</span><span>Pack / category</span><span>BPM</span>`
            + `<span>Key</span><span></span></div>
        <div id="browser-results-list" class="browser-result-list" data-scroll="browser-results" tabindex="0" role="listbox" aria-label="Sounds"`
            + ` aria-multiselectable="true" aria-busy="${!current}" ${selected ? `aria-activedescendant="browser-result-${lib.selected}"` : ''}>
          ${lib.items.length ? lib.items.map((item, index) => `<div class="browser-result ${current && index === lib.selected ? 'selected' : ''} ${current && local.marked.has(item.path) ? 'marked' : ''}`
              + ` ${item.available === false ? 'offline' : ''}" id="browser-result-${index}" role="option" aria-selected="${current && (local.marked.size ? local.marked.has(item.path) : index === lib.selected)}"`
              + ` aria-disabled="${!current}" data-browser-item="${index}"><div class="browser-sound-name"><span class="browser-file-icon ${item.kind === 'nks' ? 'nks' : ''}">${icon(item.kind)}</span>`
              + `<div><strong>${html(item.name)}</strong><small>${item.available === false ? '<span class="browser-offline-badge">Offline</span> ' : ''}${html((item.tags || []).slice(0, 3).join(' · ') || item.extension?.toUpperCase() || item.kind)}</small>`
              + `</div></div><div class="browser-result-pack"><span>${html(item.pack || 'Local sounds')}</span><small>${html(item.category || '')}</small>`
              + `</div><span class="browser-number">${html(item.bpm || '—')}</span><span class="browser-number">${html(item.key || '—')}</span>`
              + `<button class="browser-star ${item.favorite ? 'is-favorite' : ''}" data-browser-star="${index}" aria-label="${item.favorite ? 'Remove from' : 'Add to'}`
              + ` favorites: ${html(item.name)}" tabindex="-1" ${current?'':'disabled'}>${item.favorite ? '★' : '☆'}</button>`
              + `</div>`).join('') : renderEmpty(lib)}
        </div>
        <div class="browser-metadata-toolbar"><span role="status">${local.marked.size
            ? `${local.marked.size} selected · Escape clears`
            : 'Shift + arrows selects sounds'}</span>`
            + `<button id="browser-edit-metadata" aria-keyshortcuts="E" ${!current || !selected ? 'disabled' : ''}>Edit ${local.marked.size > 1 ? `${local.marked.size} sounds` : 'details'}`
            + ` <kbd>E</kbd></button></div>
        <div class="browser-list-footer"><span>${Number(lib.durationCoverage?.pending)>0
            ?`Reading sample lengths · ${Number(lib.durationCoverage.pending).toLocaleString()} queued`
            : 'Select multiple choices to combine filters'}</span>`
            + `<div><button class="browser-icon-btn" id="browser-page-prev" aria-label="Previous results" ${lib.offset === 0 ? 'disabled' : ''}>←</button>`
            + `<button class="browser-icon-btn" id="browser-page-next" aria-label="Next results" ${lib.offset + lib.items.length >= lib.total ? 'disabled' : ''}>→</button></div></div>
      </section>
      <aside class="browser-detail browser-sample-detail panel" aria-label="Selected sound and load destination">${renderDetail(selected)}</aside>
    </div>`;
  }
  function renderOffline() {
    const offline = local.offline.filter(root => Number(root.matching) > 0);
    if (!offline.length && !local.hideOffline) return '';
    const volumes = [...new Set(local.offline.map(root => root.volume || root.name))];
    const count = offline.reduce((sum, root) => sum + Number(root.matching || 0), 0);
    const name = volumes.length ? volumes.map(value => `“${html(value)}”`).join(', ') : 'A library folder';
    const message = local.hideOffline ? `${count
        ? `${count.toLocaleString()} matching sounds on ${name} are hidden while ${volumes.length === 1 ? 'it is' : 'they are'} offline.`
        : 'Showing available sounds only.'}` : `${name} ${volumes.length === 1 ? 'is' : 'are'} offline · ${count.toLocaleString()} matching sounds can’t play until reconnected. Your `
        + `index and favorites are kept.`;
    return `<div class="browser-offline-notice" role="status"><span aria-hidden="true">⏏</span><p>${message}</p><button type="button"`
        + ` id="browser-hide-offline" aria-pressed="${local.hideOffline}">${local.hideOffline ? 'Show offline sounds' : 'Hide offline sounds'}</button></div>`;
  }
  function renderEmpty(lib) {
    if (!resultsCurrent()) return '<div class="browser-empty" role="status"><h3>Updating results…</h3><p>Waiting for sounds matching the current search and filters.</p></div>';
    if (lib.scanning) return '<div class="browser-empty"><h3>Indexing library</h3><p>Search results appear as files are indexed.</p></div>';
    if (lib.query
        || activeFilterCount()
        || lib.favorites
        || lib.kind !== 'all') return '<div class="browser-empty"><span class="browser-empty-symbol">⌕</span><h3>No matching sounds</h3><p>Try a shorter query or clear the filters.</p>'
        + '<button class="btn" id="browser-clear-filters">Clear search & filters</button></div>';
    return '<div class="browser-empty"><h3>No indexed audio</h3><p>Add a sample folder or scan the known library locations.</p>'
        + '<button class="btn primary" id="browser-empty-add">Add sample folder</button><button class="btn" id="browser-empty-scan">Scan known locations</button></div>';
  }
  function renderDestination(item) {
    const S=app();
    const ready=!!item&&resultsCurrent()&&item.available!==false&&!local.busy,isNKS=item?.kind==='nks';
    const title=S.state.project?.instruments[S.state.instrument]?.name||'Instrument';
    return `<section class="browser-destination" aria-label="Load destination">
      <label class="browser-destination-label" for="browser-destination-instrument">Instrument to replace (optional)</label>`
      + `<select id="browser-destination-instrument">${(S.state.project?.instruments||[]).map((inst,index)=>`<option value="${index}" ${index===S.state.instrument?'selected':''}>${String(index).padStart(2,'0')} · ${html(inst.name)}</option>`).join('')}</select>
      <p class="browser-destination-name" title="${html(title)}">${html(title)}</p>
      <button class="btn primary" id="browser-load" ${ready
          &&(isNKS||(S.state.project?.tracks.length||0)<(window.TRACK_SANITY_LIMIT??Infinity))
          ?''
          :'disabled'}>${local.busy?'Loading…':isNKS?'Load preset':'Add sample on new track'}`
          + ` <kbd>↵</kbd></button>
      <button class="btn" id="browser-load-new" ${ready?'':'disabled'}>${isNKS?'Add preset on new track':'Replace selected instrument'}`
          + ` <kbd>⌥ ↵</kbd></button><p class="browser-small">${isNKS
              ?'Add keeps existing instruments.'
              :'Return adds and selects a linked track. Option-Return replaces the instrument chosen above.'}</p>
    </section>`;
  }
  function renderDetail(item) {
    return renderDestination(item) + `<div class="browser-detail-content" data-scroll="browser-detail">${renderDetailContent(item)}</div>`;
  }
  function renderDetailContent(item) {
    if (!resultsCurrent()) return '<div class="browser-empty browser-detail-empty"><h3>Updating results…</h3><p>Select a sound when the current search finishes.</p></div>';
    if (!item) return '<div class="browser-empty browser-detail-empty"><h3>No selection</h3><p>↑ ↓ select and audition<br>Return loads the selected sample<br>Choose '
        + 'a destination above</p></div>';
    const meta = local.metadata.get(item.path);
    const isNKS = item.kind === 'nks';
    const previewPath = item.available === false ? null : item.previewPath || meta?.previewPath || (!isNKS ? item.path : null);
    return `<div class="browser-detail-top"><span class="eyebrow">${isNKS ? 'NKS preset' : html(item.category || 'Sample')}</span>`
        + `<h2>${html(item.name)}</h2><p>${html(item.pack || meta?.vendor || 'Local library')}</p></div>${!isNKS ? renderSampleWaveform(item) : ''}
      <div class="browser-tags">${(item.tags || []).map(tag => `<button id="browser-tag-${encodeURIComponent(tag)}" data-browser-tag="${html(tag)}">${html(tag)}</button>`).join('')}</div>
      ${item.available === false ? `<p class="browser-small browser-detail-offline" role="status">${html(offlineMessage(item))}</p>` : ''}<div `
          + `class="browser-detail-actions"><button class="btn ${previewPath ? '' : 'disabled'}" id="browser-preview" ${!previewPath ? 'disabled' : ''}>${local.previewPath === previewPath ? '■ Stop preview' : '▶ Audition'}`
          + ` <kbd>Space</kbd></button>
      ${isNKS ? `<button class="btn" id="browser-inspect-nks">${local.pendingMetadata.has(item.path)
          ? 'Reading preset…'
          : meta
          ? 'Refresh preset details'
          : 'Inspect NKS preset'}</button>`
          + `<button class="btn" id="browser-open-nks-item">Browse in NKS presets</button><p class="browser-small muted">Loads into the compatible installed `
          + `plug-in with its mapped control pages.</p>` : ''}</div>
      <dl class="browser-properties">${[[!isNKS && 'Length',
          Number.isFinite(item.duration)
              ?`${item.duration.toFixed(2)} s`
              :local.waveforms.get(item.path)?.duration
              ?`${Number(local.waveforms.get(item.path).duration).toFixed(2)} s`
              :'Unknown'],
          [!isNKS && 'Type', typeNames[item.sampleType]||'Unknown'],[item.collection && 'Source',item.collection],
          [item.bpm && 'Tempo', item.bpm && `${item.bpm} BPM`], [item.key && 'Key', item.key],
          [item.extension && 'Format', item.extension?.toUpperCase()], [item.size && 'Size', bytes(item.size)],
          [meta?.vendor && 'Vendor', meta?.vendor], [meta?.author && 'Author', meta?.author]].filter(([label]) => label).map(([label, value]) => `<div><dt>${html(label)}</dt><dd>${html(value)}</dd></div>`).join('')}</dl>
      ${isNKS && meta ? renderNKS(meta) : ''}
      <p class="browser-metadata-origin">${Object.keys(item.userMetadata || {}).length
          ? 'Your metadata overrides are active. Edit details to restore inferred values.'
          : `Metadata from ${html(item.metadataSource || 'the filename and folders')}.`}</p>
      <div class="browser-path"><span class="eyebrow">File location</span><p title="${html(item.path)}">${html(item.path)}</p></div>`;
  }
  function renderSampleWaveform(item) {
    const value = local.waveforms.get(item.path), peaks = value?.peaks || value?.peaks512 || [];
    let path = '';
    for (let x = 0; x < 320 && peaks.length; x += 2) {
      const point = peaks[Math.min(peaks.length - 1, Math.floor(x / 320 * peaks.length))];
      const amplitude = Math.min(1, Array.isArray(point) ? Math.max(...point.map(Math.abs)) : Math.abs(Number(point || 0))) * 29;
      path += `M${x} ${35 - amplitude}V${35 + amplitude}`;
    }
    return `<div class="browser-sample-wave"><svg viewBox="0 0 320 70" preserveAspectRatio="none" role="img" aria-label="${peaks.length ? 'Sample waveform' : 'Waveform loading'}">`
        + `<path d="M0 35H320" stroke="#3c3b34" stroke-width=".7"/><path d="${path}" stroke="#a49a82" stroke-width="1.4"/>`
        + `</svg><div>${value?.duration ? `<span>${Number(value.duration).toFixed(2)} s</span><span>${Math.round((value.sampleRate || 0) / 100) / 10} kHz · ${value.channels || 1}`
        + ` ch</span>` : `<span>${local.wavePending.has(item.path) ? 'Reading waveform…' : 'Select to read waveform'}</span>`}</div></div>`;
  }
  async function loadWaveform(item) {
    if (!item
        || item.kind === 'nks'
        || item.available === false
        || local.waveforms.has(item.path)
        || local.wavePending.has(item.path)
        || local.waveFailed.has(item.path)) return;
    local.wavePending.add(item.path);
    try { const value = await app().request('audio.clipInfo',
        { path: item.path }); if (value?.duration) local.waveforms.set(item.path, value); else local.waveFailed.add(item.path); }
    catch { local.waveFailed.add(item.path); }
    finally { local.wavePending.delete(item.path); if (currentItem()?.path === item.path
        && document.querySelector('.browser-workspace')
        && !refreshDetail()) paint(); }
  }
  function renderNKS(meta) {
    const mappings = meta.controllerMappings;
    const rows = [];
    function flatten(value, path = '', depth = 0) {
      if (rows.length >= 64 || depth > 5) return;
      if (value == null) return;
      if (typeof value !== 'object') { rows.push([path, String(value)]); return; }
      Object.entries(value).forEach(([key, child]) => flatten(child, path ? `${path} · ${key}` : key, depth + 1));
    }
    if (mappings) flatten(mappings);
    const pages = (meta.controllerPages || []).filter(page => page.parameters?.length);
    const controllerView = pages.length ? pages.map(page => `<details open class="browser-controller-page"><summary>${html(page.name || `Page ${page.index + 1}`)}<span>${page.parameters.length}`
        + ` mapped controls</span></summary><div class="browser-controller-grid">${page.parameters.map(parameter => `<div title="${html(parameter.id == null ? 'Preset metadata mapping' : `Parameter ID: ${parameter.id}`)}">`
        + `<span>${Number(parameter.slot ?? 0) + 1}</span><strong>${html(parameter.name || 'Unlabeled control')}</strong>`
        + `</div>`).join('')}</div>`
        + `</details>`).join('') : rows.length ? `<details open><summary>Controller map <span>${rows.length} fields</span></summary><dl class="browser-mapping">${rows.map(([key, value]) => `<div><dt>${html(key)}</dt><dd>${html(value)}</dd></div>`).join('')}</dl>`
        + `</details>` : '<p class="browser-small muted">No readable controller mapping in this preset.</p>';
    return `<div class="browser-nks-info"><h3>Preset information</h3><p class="browser-small">${meta.hasPluginState
        ? 'Embedded plug-in state detected.'
        : 'No supported embedded plug-in state detected.'}</p>${meta.pluginID
        ? `<p class="browser-small muted">Plug-in ID: ${html(typeof meta.pluginID === 'object' ? JSON.stringify(meta.pluginID) : meta.pluginID)}</p>`
        : ''}${controllerView}${[...(meta.warnings || []), ...(meta.limitations || [])].map(message => `<p class="browser-small browser-notice">${html(message)}</p>`).join('')}</div>`;
  }
  function effectSlots() {
    const state = app().state;
    const effects = [];
    const owner = `track:${state.project?.tracks[state.track]?.routingID}`;
    (state.project?.audioPlugins || []).filter(value => value.owner === owner && value.kind === 'effect').forEach(value => { effects[value.slot] = value; });
    return effects;
  }
  const pluginFormat = plugin => plugin.format.toUpperCase();
  function filteredPlugins() {
    const query = local.pluginQuery.toLocaleLowerCase().trim();
    const matches = (app().state.plugins || []).filter(plugin => plugin.type === local.pluginKind
        && (local.pluginFormat === 'all' || pluginFormat(plugin) === local.pluginFormat)
        && (!query || `${plugin.name} ${plugin.manufacturer}`.toLocaleLowerCase().includes(query)));
    return (local.pluginFormat === 'all' ? preferredFormats(matches) : matches).sort((a, b) => a.name.localeCompare(b.name));
  }
  // One entry per plug-in when showing all formats: Audio Unit, else VST3, else CLAP.
  // The format buttons still list every installed version.
  const formatRank = { AU: 0, VST3: 1, CLAP: 2 };
  const pluginKey = plugin => `${String(plugin.name || '').toLowerCase().replace(/[^a-z0-9]+/g, '')}|${String(plugin.manufacturer || '').toLowerCase().replace(/[^a-z0-9 ]+/g, '').trim().split(/\s+/)[0]
      || ''}|${plugin.type}`;
  function preferredFormats(list) {
    const best = new Map();
    for (const plugin of list) {
      const key = pluginKey(plugin), current = best.get(key);
      if (!current || (formatRank[pluginFormat(plugin)] ?? 9) < (formatRank[pluginFormat(current)] ?? 9)) best.set(key, plugin);
    }
    return [...best.values()];
  }
  function renderPlugins() {
    const plugins = filteredPlugins();
    local.pluginSelected = Math.max(0, Math.min(local.pluginSelected, plugins.length - 1));
    const plugin = plugins[local.pluginSelected];
    const track = app().state.project?.tracks[app().state.track];
    const ownerName = track?.name || `Track ${Number(app().state.track || 0) + 1}`;
    const effects = effectSlots();
    const slots = Math.min(INSERT_SLOTS, Math.max(1, effects.length + 1));
    local.slot = Math.max(0, Math.min(local.slot, slots - 1));
    return `<div class="browser-plugins-layout">
      <aside class="browser-sidebar panel"><div class="panel-head"><h2>Plug-ins</h2></div><p class="browser-small muted">${local.pluginScan?.scanning
          ?`Scanning plug-ins · ${local.pluginScan.completed||0} of ${local.pluginScan.total||'…'}`
          :'Audio Unit, VST3 and CLAP instruments and effects installed on this computer.'}</p>`
          + `<div class="browser-filter-group">${[['instrument', 'Instruments', '◈'], ['effect', 'Audio effects', '≋']].map(([kind, name, symbol]) => `<button class="browser-filter ${local.pluginKind === kind ? 'active' : ''}" id="browser-plugin-kind-${kind}" tabindex="${local.pluginKind===kind?0:-1}"`
          + ` aria-pressed="${local.pluginKind===kind}" data-browser-plugin-kind="${kind}"><span>${name}</span><span>${symbol}</span>`
          + `</button>`).join('')}</div>`
          + `<div class="browser-plugin-formats" aria-label="Plug-in format">${[['all','All'],['AU','AU'],['VST3','VST3'],['CLAP','CLAP']].map(([value,label])=>`<button type="button" id="browser-plugin-format-${value}" tabindex="${local.pluginFormat===value?0:-1}" data-browser-plugin-format="${value}"`
          + ` aria-pressed="${local.pluginFormat===value}">${label}</button>`).join('')}</div>`
          + `<button class="btn browser-scan-btn" id="browser-plugin-rescan">Refresh plug-ins</button><div class="browser-plugin-tip">`
          + `<span class="eyebrow">NKS presets</span><p class="browser-small muted">Browse NKS instruments and effects by product, bank, type and character `
          + `in the NKS presets tab. Their eight-knob control pages appear when loaded.</p><button class="btn browser-scan-btn" id="browser-open-nks">Open `
          + `NKS presets</button></div></aside>
      <section class="browser-results panel"><div class="browser-search-wrap"><span>⌕</span><input id="browser-plugin-search" class="browser-search"`
          + ` type="search" value="${html(local.pluginQuery)}" placeholder="Find an instrument or manufacturer…" aria-label="Search plug-ins"`
          + ` autocomplete="off"><kbd>⌘ F</kbd></div><div class="browser-result-summary"><span>${plugins.length} ${local.pluginKind === 'instrument' ? 'instruments' : 'effects'}</span>`
          + `<span>${local.pluginFormat==='all'?'AU + VST3 + CLAP':local.pluginFormat} · native host</span></div><div class="browser-plugin-list"`
          + ` id="browser-plugin-list" role="listbox" aria-label="Plug-ins" tabindex="0" ${plugin ? `aria-activedescendant="browser-plugin-${local.pluginSelected}"` : ''}>${plugins.length ? plugins.map((value, index) => `<div id="browser-plugin-${index}" class="browser-plugin-row ${index === local.pluginSelected ? 'selected' : ''}" data-browser-plugin="${index}"`
          + ` role="option" aria-selected="${index === local.pluginSelected}"><span class="browser-plugin-symbol">${local.pluginKind === 'instrument' ? '◈' : '≋'}</span>`
          + `<div><strong>${html(value.name)}</strong><small>${html(value.manufacturer)}</small></div><span class="browser-format-badge">${html(pluginFormat(value))}</span>`
          + `<span class="browser-plugin-type">${value.hasCustomView ? 'Custom editor' : 'Parameters'}</span></div>`).join('') : '<div class="browser-empty"><span class="browser-empty-symbol">◈</span><h3>No matching plug-ins</h3><p>Try another search, or refresh the '
          + 'list after installing a plug-in.</p></div>'}</div></section>
      <aside class="browser-detail panel">${plugin ? `<div class="browser-detail-top"><div class="browser-artwork nks" aria-hidden="true">${local.pluginKind === 'instrument' ? '◈' : '≋'}</div>`
          + `<span class="eyebrow">${html(pluginFormat(plugin))} ${html(local.pluginKind)}</span><h2>${html(plugin.name)}</h2>`
          + `<p>${html(plugin.manufacturer)}</p></div><div class="browser-detail-actions"><p class="browser-small muted">Destination: ${html(ownerName)}${local.pluginKind === 'instrument' ? ` · ${html('New track (existing sounds kept)')}` : ''}</p>${local.pluginKind === 'effect' ? `<label class="browser-slot-label">Effect slot<select id="browser-plugin-slot" class="field">${Array.from({ length: slots },
              (_, index) => `<option value="${index}" ${local.slot === index ? 'selected' : ''}>${index + 1} · ${html(effects[index]?.name || 'New effect')}</option>`).join('')}</select>`
          + `</label>` : ''}<button `
          + `class="btn primary" id="browser-plugin-load" ${local.busy
              || (local.pluginKind === 'instrument' && (app().state.project?.tracks.length||0)>=(window.TRACK_SANITY_LIMIT??Infinity))
              ? 'disabled'
              : ''}>${local.busy
                  ? 'Loading plug-in…'
                  : local.pluginKind === 'instrument'
                  ? 'Add plug-in on new track'
                  : 'Load effect'}`
          + ` <kbd>↵</kbd></button>${local.pluginKind === 'instrument' ? `<button class="btn" id="browser-plugin-replace" ${local.busy?'disabled':''}>Replace selected instrument <kbd>⌥ ↵</kbd>`
          + `</button>` : ''}<p `
          + `class="browser-small muted">${local.pluginKind === 'instrument'
              ? 'Adds and selects a new linked track. Option-Return replaces the selected instrument on this track.'
              : 'The effect processes the selected track. Its parameters are available for automation.'}</p>`
          + `</div><dl class="browser-properties"><div><dt>Format</dt><dd>${html(pluginFormat(plugin))}</dd></div><div>`
          + `<dt>Version</dt><dd>${html(plugin.version ?? '—')}</dd></div><div><dt>Interface</dt><dd>${plugin.hasCustomView ? 'Custom editor' : 'Parameter controls'}</dd>`
          + `</div></dl>` : '<div class="browser-empty"><h3>Pick your instrument</h3><p>Use the arrows to browse and Return to load on the selected track.</p></div>'}</aside>
    </div>`;
  }
  async function search() {
    // An explicit search consumes the scheduled copy (for example after a filter change).
    clearTimeout(local.timer); local.timer = null;
    clearTimeout(local.backgroundTimer); local.backgroundTimer = null;
    if (activeTab() !== 'samples') { local.backgroundDirty = true; return; }
    const lib = library(), key = searchKey(), hadError = !!local.searchError;
    local.searchError = '';
    if (invalidateResults(key) || hadError) paint();
    const generation = ++local.generation;
    const payload = { query: lib.query, kind: lib.kind, filters: filterPayload(),
      favorites: !!lib.favorites, offset: lib.offset, limit: lib.limit, ...(local.hideOffline ? { hideOffline: true } : {}) };
    const stillCurrent = () => generation === local.generation && key === searchKey() && activeTab() === 'samples';
    local.searching++; local.backgroundDirty = false;
    let result;
    try {
      // Native workers coalesce searches; a late-arriving older request can cancel
      // the newest UI request. Retry only this still-current query, with a fixed cap.
      for (let attempt = 0; attempt < 3; attempt++) {
        result = await call('library.search', payload);
        if (!stillCurrent()) return;
        if (!result?.cancelled) break;
        if (!document.querySelector('.browser-workspace')) return;
      }
    } finally {
      local.searching--;
      if (local.backgroundDirty) queueBackgroundSearch();
    }
    if (!stillCurrent()) return;
    if (!result || result.cancelled) {
      local.searchError = result?.cancelled ? 'Search interrupted.' : 'Search failed.';
      paint(); return;
    }
    // Read selection after the reply: navigation during a background search owns the current selection.
    const selectedPath = currentItem()?.path;
    const anchorPath = local.selectionAnchor === null ? null : lib.items[local.selectionAnchor]?.path;
    local.resultsKey = key; local.resultsStale = false;
    lib.items = result.items || [];
    local.marked = new Set(lib.items.filter(item => local.marked.has(item.path)).map(item => item.path));
    const anchorIndex = lib.items.findIndex(item => item.path === anchorPath);
    local.selectionAnchor = local.marked.size && anchorIndex >= 0 ? anchorIndex : null;
    lib.total = result.total || 0;
    lib.scanning = !!result.scanning;
    if (result.categories) lib.categories = result.categories;
    if (result.packs) lib.packs = result.packs;
    if (result.facets) lib.facets = result.facets;
    if (result.durationCoverage) lib.durationCoverage = result.durationCoverage;
    local.offline = Array.isArray(result.offline) ? result.offline : [];
    updateLibraryStatus(result);
    // Background refreshes (header reads, scans) of the same search must not postpone remembering it.
    if (local.recentTimer === null || local.recentPendingKey !== key) {
      clearTimeout(local.recentTimer);
      local.recentPendingKey = key;
      local.recentTimer = setTimeout(rememberSearch, 2500);
    }
    const oldSelection = lib.items.findIndex(item => item.path === selectedPath);
    lib.selected = Math.max(0, Math.min(oldSelection < 0 ? lib.selected : oldSelection, lib.items.length - 1));
    paint();
    queuePrefetch();
  }
  function queueSearch(delay = 160) {
    local.searchError = '';
    // Invalidate the previous query immediately, including during the typing debounce.
    const changed = invalidateResults(searchKey());
    local.generation++;
    clearTimeout(local.timer); clearTimeout(local.backgroundTimer); local.backgroundTimer = null;
    local.timer = setTimeout(() => { local.timer = null; search(); }, delay);
    if (changed) paint();
  }
  function queueBackgroundSearch(delay = 450) {
    local.backgroundDirty = true;
    // Scan bursts may request another refresh, but must never postpone user input.
    if (local.timer !== null
        || local.backgroundTimer !== null
        || local.searching
        || activeTab() !== 'samples'
        || !document.querySelector('.browser-workspace')) return;
    local.backgroundTimer = setTimeout(() => {
      local.backgroundTimer = null;
      if (local.timer !== null || local.searching || activeTab() !== 'samples') return;
      search();
    }, delay);
  }
  async function pageTo(offset, position = 'first') {
    const lib = library();
    lib.offset = Math.max(0, offset); lib.selected = 0;
    const key = searchKey();
    await search();
    if (key !== searchKey() || !resultsCurrent() || !lib.items.length) return;
    select(position === 'last' ? lib.items.length - 1 : 0);
    document.getElementById('browser-results-list')?.focus({ preventScroll: true });
  }
  async function inspect(item = currentItem()) {
    if (!item || item.kind !== 'nks') return null;
    if (local.metadataRequests.has(item.path)) return local.metadataRequests.get(item.path);
    local.pendingMetadata.add(item.path); paint();
    const pending = (async () => {
      try {
        const result = await call('library.nks', { path: item.path });
        if (result) local.metadata.set(item.path, result);
        return result;
      } finally {
        local.pendingMetadata.delete(item.path); local.metadataRequests.delete(item.path); paint();
      }
    })();
    local.metadataRequests.set(item.path, pending);
    return pending;
  }
  const offlineMessage = item => `${item.volume ? `“${item.volume}”` : 'This library folder'} is offline. Reconnect the drive to preview or load ${item.name}.`;
  // Background-decode the previews around the selection (preview-prefetch.js) while auto preview is on.
  const prefetchEntry = item => {
    const path = item.previewPath || local.metadata.get(item.path)?.previewPath || (item.kind !== 'nks' ? item.path : null);
    return path ? { path } : null;
  };
  function queuePrefetch() {
    if (activeTab() !== 'samples' || !local.previewEnabled || !resultsCurrent()) return;
    const lib = library(); window.STPreviewPrefetch?.schedule('samples', { items: lib.items, selected: lib.selected, entry: prefetchEntry });
  }
  async function preview(item = currentItem(), toggle = false) {
    if (!item || !resultsCurrent()) return;
    if (item.available === false) { if (toggle) app().toast(offlineMessage(item), 'info'); return; }
    if (toggle) rememberSearch();
    const generation = ++local.previewGeneration;
    local.previewPending = true;
    let path = item.previewPath || local.metadata.get(item.path)?.previewPath || (item.kind !== 'nks' ? item.path : null);
    if (!path && item.kind === 'nks' && !local.metadata.has(item.path)) path = (await inspect(item))?.previewPath;
    if (generation !== local.previewGeneration) return;
    if (!path) { await stopPreview(); if (toggle) app().toast('This preset has no available audio preview. Inspect its metadata or open it in its plug-in.',
        'info'); return; }
    if (toggle && local.previewPath === path) { await stopPreview(); return; }
    clearTimeout(local.previewEndTimer);
    const result = await call('sample.preview', { path });
    if (generation === local.previewGeneration) local.previewPending = false;
    if (result !== null && generation === local.previewGeneration) {
      local.previewPath = path;
      if (Number(result.duration) > 0) local.previewEndTimer = setTimeout(() => {
        if (generation === local.previewGeneration) {
          local.previewPath = '';
          if (document.querySelector('.browser-workspace') && !refreshDetail()) paint();
        }
      }, Math.min(Number(result.duration) * 1000 + 80, 2147480000));
    }
    if (!refreshDetail()) paint();
  }
  async function stopPreview() {
    local.previewGeneration++; clearTimeout(local.previewTimer); clearTimeout(local.previewEndTimer);
    local.previewTimer = null; local.previewPending = false; local.previewPath = '';
    await call('sample.stopPreview', {});
    if (document.querySelector('.browser-workspace')) paint();
  }
  async function load(newInstrument) {
    if (activeTab() === 'plugins') return loadPlugin(newInstrument);
    const item = currentItem();
    if (!item || local.busy) return;
    if (newInstrument === undefined) newInstrument = item.kind !== 'nks';
    if (item.available === false) { app().toast(offlineMessage(item), 'error'); return; }
    rememberSearch();
    const destination = {
      project: app().state.project,
      track: app().state.track,
      instrument: app().state.instrument
    };
    // A deletion can leave the cursor on the same index, now occupied by another destination.
    const trackObject = destination.project?.tracks[destination.track],
      instrumentObject = destination.project?.instruments[destination.instrument];
    const destinationCurrent = () => app().state.project === destination.project
        && app().state.track === destination.track
        && app().state.instrument === destination.instrument
        && destination.project?.tracks[destination.track] === trackObject
        && destination.project?.instruments[destination.instrument] === instrumentObject;
    local.busy = true;
    try {
      if (item.kind === 'nks') {
        if (!window.STNKS?.load) throw new Error('NKS loading is unavailable in this build. Inspect the preset or open it in Komplete Kontrol.');
        const meta = local.metadata.get(item.path) || await inspect(item);
        if (!meta) return;
        const device = String(meta.deviceType || '').toUpperCase(), effect = device === 'FX' || (device !== 'INST' && item.extension === 'nksfx');
        await stopPreview();
        if (!destinationCurrent()) { app().toast('The destination changed. Load the preset again in the intended track.', 'info'); return; }
        // NKS metadata identifies effects even when a vendor uses the .nksf extension.
        if (effect) { const effects = effectSlots(), free = effects.findIndex(value => !value); await window.STNKS.load(item.path,
            { kind: 'effect', slot: Math.min(INSERT_SLOTS - 1, free < 0 ? effects.length : free) }); }
        else await window.STNKS.load(item.path, { newInstrument: !!newInstrument });
        return;
      }
      await stopPreview();
      if (!destinationCurrent()) { app().toast('The destination changed. Load the sound again in the intended instrument.', 'info'); return; }
      if (newInstrument) {
        if (!app().loadSampleAsNew) throw new Error('Adding a sample instrument is unavailable in this build.');
        return await app().loadSampleAsNew(item.path);
      }
      return await app().loadSample(item.path, destination.instrument);
    } catch (error) { failure(error); }
    finally { local.busy = false; paint(); }
  }
  async function favorite(item = currentItem()) {
    if (!item || !resultsCurrent()) return;
    const favorite = !item.favorite;
    const result = await call('library.favorite', { path: item.path, favorite });
    if (result !== null) { item.favorite = favorite; if (library().favorites && !favorite) search(); else paint(); }
  }
  async function roots() {
    const result = await call('library.roots', {});
    if (Array.isArray(result)) library().roots = result;
    else if (result?.roots) library().roots = result.roots;
    updateLibraryStatus({ roots: library().roots });
    paint();
  }
  async function scan() {
    const lib = library();
    if (lib.scanning) { await call('library.cancel', {}); lib.scanning = false; paint(); return; }
    const result = await call('library.scan', {});
    if (result) { lib.scanning = !!result.scanning; lib.count = result.count || 0; updateLibraryStatus(result); await roots(); queueSearch(0); }
    paint();
  }
  async function addRoot(path) {
    const result = await call('library.chooseRoot', typeof path === 'string' ? { path } : {});
    if (!result || result.cancelled) return;
    updateLibraryStatus(result);
    await roots(); queueSearch(0);
  }
  async function refreshPlugins(scan = true) {
    if (scan && app().state.native) { try { await call('plugins.scan', {}); } catch {} }
    const result = await call('plugins.list', {});
    if (result) app().state.plugins = Array.isArray(result) ? result : result.plugins || [];
    local.pluginsLoaded = true; paint();
  }
  async function loadPlugin(asNew) {
    asNew = typeof asNew === 'boolean' ? asNew : local.pluginKind === 'instrument';
    const plugin = filteredPlugins()[local.pluginSelected];
    if (!plugin || local.busy) return;
    const state = app().state;
    const destinationProject = state.project, instrumentIndex = state.instrument;
    const destinationInstrument = destinationProject.instruments[instrumentIndex], destinationTrack = destinationProject.tracks[state.track];
    const createInstrument = local.pluginKind === 'instrument' && asNew;
    local.busy = true; paint();
    const trackIndex = Number(state.track || 0);
    const target = {
      owner: `track:${destinationTrack.routingID}`,
      kind: local.pluginKind,
      slot: local.pluginKind === 'instrument' ? 0 : local.slot,
      component: plugin.id
    };
    if (plugin.bundlePath) target.bundlePath = plugin.bundlePath;
    const stillValid = () => state.project === destinationProject
        && destinationProject.tracks[trackIndex] === destinationTrack
        && (target.kind !== 'instrument' || destinationProject.instruments[instrumentIndex] === destinationInstrument);
    const stale = async () => {
      await app().syncEngineProject(() => window.STUtil.clone(state.project));
      app().toast('Plug-in attachment canceled because its project or destination changed.', 'info');
    };
    try {
      if (target.kind === 'instrument' && window.STNKS?.loadInstrument && window.STInstrumentEdits?.run) {
        return await window.STNKS.loadInstrument('plugin.load', target,
            { name: plugin.name, stay: true, newInstrument: asNew, replaceInstrument: !asNew });
      }
      const result = await call('plugin.load', target);
      if (result) {
        if (!stillValid()) { await stale(); return; }
        const snapshot = await call('plugin.state', {});
        if (!stillValid()) { await stale(); return; }
        app().mutate(`Load ${plugin.name}`, () => {
          // The track's previous plug-in instrument stays available when the track moves on to a new one.
          if (target.kind === 'instrument' && createInstrument) window.STRoutingProject.keepInstrumentPlugins(state.project, [destinationTrack]);
          state.project.audioPlugins = snapshot;
          const track = destinationTrack;
          if (target.kind === 'instrument') {
            const destination = createInstrument ? state.project.instruments.length : instrumentIndex;
            if (createInstrument) state.project.instruments.push(app().instrumentDefaults(destination, plugin.name, 'plugin'));
            const instrument = state.project.instruments[destination];
            if (instrument) {
              instrument.type = 'plugin';
              delete instrument.plugin;
              instrument.name = plugin.name;
              track.instrument = destination;
              if (state.track === trackIndex && state.instrument === instrumentIndex) state.instrument = destination;
            }
          }
        });
        app().toast(`${plugin.name} loaded on ${destinationTrack.name || 'track'}`, 'success');
      }
    } catch (error) { failure(error); }
    finally { local.busy = false; paint(); }
  }
  function editMetadata() {
    if (activeTab() !== 'samples' || !resultsCurrent() || !currentItem()) return;
    const selected = local.marked.size ? library().items.filter(item => local.marked.has(item.path)) : [currentItem()];
    const paths = selected.map(item => item.path), batch = paths.length > 1;
    if (!paths.length) return;
    if (paths.length > 500) { app().toast('Select up to 500 sounds to edit at once.', 'info'); return; }
    stopPreview();
    const fields = [['tags', 'Tags'], ['category', 'Category'], ['key', 'Musical key'], ['bpm', 'Tempo · BPM'],
        ...(selected.every(item => item.kind === 'sample') ? [['sampleType', 'Sound type']] : [])];
    const valueOf = (item, field) => field === 'tags' ? (item.tags || []).join(', ') : item[field] ?? '';
    const rows = fields.map(([field, label]) => {
      const first = valueOf(selected[0], field), common = selected.every(item => valueOf(item, field) === first), value = common ? first : '';
      const overrideCount = selected.filter(item => Object.prototype.hasOwnProperty.call(item.userMetadata || {}, field)).length;
      const inferred = batch ? '' : valueOf(selected[0].inferredMetadata || {}, field);
      const options = [['keep', 'Keep current'], ...(field === 'tags' ? [['add', 'Add tags']] : []),
          ['set', field === 'tags' ? 'Replace tags' : 'Use value'], ['infer', 'Use inferred']];
      const control = field === 'sampleType' ? `<select id="metadata-value-${field}" aria-label="${label}">${[['unknown', 'Unknown'], ['oneshot', 'One-shot'], ['loop', 'Loop']].map(([v, text]) => `<option value="${v}" ${v === value ? 'selected' : ''}>${text}</option>`).join('')}</select>` : `<input id="metadata-value-${field}" aria-label="${label}" ${field === 'bpm'
          ? 'type="number" min="1" max="999" step="any"'
          : `type="text" maxlength="${field === 'tags' ? 5000 : field === 'key' ? 24 : 80}"`}`
          + ` value="${html(value)}" placeholder="${!common
              ? 'Mixed values'
              : field === 'tags'
              ? 'Comma-separated tags'
              : field === 'key'
              ? 'C, F# minor, Bb major'
              : field === 'bpm'
              ? 'Unknown'
              : 'Category'}">`;
      return `<div class="browser-metadata-field"><label for="metadata-value-${field}">${label}</label><div>${control}<select id="metadata-mode-${field}"`
          + ` aria-label="${label} change">${options.map(([v, text]) => `<option value="${v}">${text}</option>`).join('')}</select>`
          + `</div><small id="metadata-note-${field}">${overrideCount ? `${overrideCount} ${batch ? 'sounds have' : 'sound has'} your override. ` : ''}${batch
              ? 'Unchanged fields keep each sound’s current value.'
              : `Inferred: ${html(field === 'sampleType' ? typeNames[inferred] || 'Unknown' : inferred || 'Unknown')}`}</small></div>`;
    }).join('');
    app().openModal(batch
        ? `Edit ${paths.length} sounds`
        : `Edit ${selected[0].name}`, `<p class="browser-metadata-description">Your library labels are saved on this Mac. Sample and preset files stay unchanged.</p>`
        + `<div class="browser-metadata-fields">${rows}</div><p class="browser-small muted">Blank key or tempo means unknown. Tags use commas. `
        + `Return saves; Escape cancels.</p>`, '<span id="metadata-status" role="status" aria-live="polite"></span><button id="metadata-restore">Use all inferred</button>'
        + '<button id="metadata-cancel">Cancel</button><button id="metadata-save" class="primary">Save details</button>', {
      kind: 'libraryMetadata',
      className: 'browser-metadata-modal'
    });
    const get = id => document.getElementById(id), save = get('metadata-save');
    if (!save) return;
    const owns = () => get('metadata-save') === save;
    const controls = fields.map(([field]) => ({ field, value: get(`metadata-value-${field}`), mode: get(`metadata-mode-${field}`) }));
    const status = get('metadata-status'), cancel = get('metadata-cancel'), restore = get('metadata-restore'), close = get('modal-close');
    let saving = false;
    const editable = () => !saving && owns();
    for (const control of controls) {
      const { field, value, mode } = control;
      const changed = () => {
        if (!editable()) return;
        if (mode.value === 'keep' || mode.value === 'infer') mode.value = field === 'tags' && batch ? 'add' : 'set';
        status.textContent = '';
      };
      value.addEventListener(field === 'sampleType' ? 'change' : 'input', changed);
      value.addEventListener('keydown', event => {
        if (event.key === 'Enter' && !event.isComposing && !event.metaKey && !event.ctrlKey && !event.altKey) {
          event.preventDefault();
          save.click();
        }
      });
    }
    cancel.onclick = () => { if (editable()) app().closeModal(); };
    restore.onclick = () => {
      if (!editable()) return;
      for (const control of controls) control.mode.value = 'infer';
      status.textContent = 'Save to restore inferred values for every field.';
    };
    save.onclick = async () => {
      if (!editable()) return;
      const changes = {};
      for (const { field, value, mode } of controls) {
        if (mode.value === 'keep') continue;
        if (mode.value === 'infer') { changes[field] = null; continue; }
        const text = value.value.trim();
        if (field === 'bpm') {
          if (value.validity?.badInput) { status.textContent = 'Finish entering the tempo, or clear the field for unknown.'; value.focus(); return; }
          const bpm = text === '' ? 0 : Number(text);
          if (!Number.isFinite(bpm)
              || (bpm !== 0 && (bpm < 1 || bpm > 999))) { status.textContent = 'Tempo must be 1–999 BPM, or blank for unknown.'; value.focus(); return; }
          changes.bpm = bpm;
        } else if (field === 'tags') changes[mode.value === 'add' ? 'addTags' : 'tags'] = text.split(',').map(tag => tag.trim()).filter(Boolean);
        else if (field === 'category' && !text) { status.textContent = 'Enter a category or choose Use inferred.'; value.focus(); return; }
        else changes[field] = text;
      }
      if (!Object.keys(changes).length) { app().closeModal(); return; }
      saving = true; status.textContent = 'Saving details…';
      const buttons = [save, cancel, restore, close].filter(Boolean);
      for (const control of [...buttons, ...controls.flatMap(control => [control.value, control.mode])]) control.disabled = true;
      try {
        await app().request('library.metadata.edit', { paths, changes });
        if (owns()) { app().closeModal(); app().toast(`Saved details for ${paths.length === 1 ? '1 sound' : `${paths.length} sounds`}`); }
        if (document.querySelector('.browser-workspace')) await search();
      } catch (error) {
        if (owns()) status.textContent = error?.message || 'Could not save library details. Your changes are still here.';
        else app().toast(error?.message || 'Could not save library details.', 'error');
      } finally {
        saving = false;
        if (owns()) for (const control of [...buttons, ...controls.flatMap(control => [control.value, control.mode])]) control.disabled = false;
      }
    };
    setTimeout(() => { if (owns()) controls[0]?.value.focus(); }, 0);
  }
  function select(index, audition = true, selection = '') {
    if (activeTab() === 'plugins') {
      const size = filteredPlugins().length;
      local.pluginSelected = Math.max(0, Math.min(index, size - 1)); paint();
      document.getElementById(`browser-plugin-${local.pluginSelected}`)?.scrollIntoView({ block: 'nearest' });
      return;
    }
    if (!resultsCurrent()) return;
    const lib = library(), destination = Math.max(0, Math.min(index, lib.items.length - 1));
    if (selection === 'range') {
      if (local.selectionAnchor === null) local.selectionAnchor = lib.selected;
      local.marked = new Set(lib.items.slice(Math.min(local.selectionAnchor, destination),
          Math.max(local.selectionAnchor, destination) + 1).map(item => item.path));
    } else if (selection === 'toggle') {
      if (!local.marked.size && lib.items[lib.selected]) local.marked.add(lib.items[lib.selected].path);
      const path = lib.items[destination]?.path;
      if (path) { if (local.marked.has(path)) local.marked.delete(path); else local.marked.add(path); }
      local.selectionAnchor = destination;
    } else { local.marked.clear(); local.selectionAnchor = null; }
    if (selection && previewActive()) stopPreview();
    const previous = lib.selected;
    lib.selected = destination;
    if (selection || !fastSelect(previous, destination)) paint();
    document.getElementById(`browser-result-${lib.selected}`)?.scrollIntoView({ block: 'nearest' });
    clearTimeout(local.previewTimer);
    clearTimeout(local.waveformTimer); local.waveformTimer = setTimeout(() => { local.waveformTimer = null; loadWaveform(currentItem()); }, 180);
    local.previewTimer = null;
    if (audition && local.previewEnabled) {
      if (currentItem()?.available === false) { if (previewActive()) stopPreview(); }
      else local.previewTimer = setTimeout(() => { local.previewTimer = null; preview(); }, 65);
    }
    queuePrefetch();
  }
  function bindDetail() {
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    document.querySelectorAll('[data-browser-tag]').forEach(button => button.addEventListener('click', () => {
      toggleFilter('tags', button.dataset.browserTag);
    }));
    const destinationProject=app().state.project,instruments=destinationProject?.instruments.slice()||[];
    on('browser-destination-instrument','change',event=>{
      const index=Number(event.target.value);
      if(app().state.project!==destinationProject
          ||!Number.isInteger(index)
          ||!instruments[index]
          ||destinationProject.instruments[index]!==instruments[index])return;
      app().stopHeld?.();if(app().selectInstrument)app().selectInstrument(index);else app().state.instrument=index;paint();
    });
    on('browser-preview', 'click', () => preview(currentItem(), true));
    on('browser-load', 'click', () => load()); on('browser-load-new', 'click', () => load(currentItem()?.kind === 'nks'));
    on('browser-inspect-nks', 'click', () => inspect());
    on('browser-open-nks-item', 'click', () => showNKS(currentItem()?.extension === 'nksfx' ? 'effect' : 'instrument'));
  }
  // Moving the selection repaints only the two rows and the detail panel (a full repaint re-renders
  // every filter list). Falls back to a full paint whenever the DOM is not the expected shape.
  function refreshDetail() {
    const detail = document.querySelector('.browser-detail'), focused = document.activeElement;
    if (!detail || activeTab() !== 'samples' || detail.contains?.(focused) || typeof detail.innerHTML !== 'string') return false;
    detail.innerHTML = renderDetail(currentItem()); bindDetail();
    return true;
  }
  function fastSelect(previous, next) {
    const lib = library(), list = document.getElementById('browser-results-list');
    const before = document.getElementById(`browser-result-${previous}`), after = document.getElementById(`browser-result-${next}`);
    if (local.marked.size || !list || !after?.classList || (before && !before.classList) || !resultsCurrent()) return false;
    if (before && before !== after) { before.classList.remove('selected'); before.setAttribute('aria-selected', 'false'); }
    after.classList.add('selected'); after.setAttribute('aria-selected', 'true');
    list.setAttribute('aria-activedescendant', `browser-result-${next}`);
    const edit = document.getElementById('browser-edit-metadata'); if (edit) edit.disabled = !lib.items[next];
    return refreshDetail();
  }
  function bind() {
    if (!local.inputGuardsInstalled) {
      local.inputGuardsInstalled = true;
      document.addEventListener?.('pointerdown',event=>{if(event.target?.closest?.('.browser-workspace'))local.pointerHeld=true;},true);
      for (const name of ['pointerup', 'pointercancel']) document.addEventListener?.(name, () => {
        local.pointerHeld = false;
        paintAfterInputDeparture();
      }, true);
      window.addEventListener?.('blur',()=>{local.pointerHeld=false;paintAfterInputDeparture();});
    }
    bindFilters();
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    document.querySelectorAll('[data-browser-tab]').forEach(button => button.addEventListener('click', () => {
      if (local.tab !== button.dataset.browserTab) {
        stopPreview();
        window.STNKSBrowser?.stopPreview();
        window.STKitBrowser?.stopPreview();
      }
      local.tab = button.dataset.browserTab;
      if (app().state.view === 'plugins') app().state.view = 'browser';
      paint();
      if (local.tab === 'samples' && local.backgroundDirty) queueBackgroundSearch(0);
    }));
    document.querySelectorAll('[data-browser-kind]').forEach(button => button.addEventListener('click', () => {
      library().kind = button.dataset.browserKind;
      changedFilter();
    }));
    bindDetail();
    document.querySelectorAll('[data-browser-remove-root]').forEach(button => button.addEventListener('click', async () => {
      const result = await call('library.removeRoot', { path: button.dataset.browserRemoveRoot });
      if (result !== null) { delete library().rootReports[button.dataset.browserRemoveRoot]; delete library().rootErrors[button.dataset.browserRemoveRoot]; library().errors = library().errors.filter(message => !message.includes(button.dataset.browserRemoveRoot)); await roots(); await search(); app().toast('Location removed from the library. Your files are unchanged.',
          'info'); }
    }));
    document.querySelectorAll('[data-browser-rechoose-root]').forEach(button => button.addEventListener('click',
        () => addRoot(button.dataset.browserRechooseRoot)));
    document.querySelectorAll('[data-browser-item]').forEach(row => {
      row.addEventListener('click', event => {
        if (!event.target.closest('[data-browser-star]')) {
          select(Number(row.dataset.browserItem), !event.shiftKey && !event.metaKey && !event.ctrlKey,
              event.shiftKey ? 'range' : event.metaKey || event.ctrlKey ? 'toggle' : '');
          document.getElementById('browser-results-list')?.focus({ preventScroll: true });
        }
      });
      row.addEventListener('dblclick', () => load());
    });
    document.querySelectorAll('[data-browser-star]').forEach(button => button.addEventListener('click', event => {
      event.stopPropagation();
      favorite(library().items[Number(button.dataset.browserStar)]);
    }));
    document.querySelectorAll('[data-browser-plugin-kind]').forEach(button => button.addEventListener('click', () => {
      local.pluginKind = button.dataset.browserPluginKind;
      local.pluginSelected = 0;
      const effects = effectSlots();
      local.slot = Math.min(INSERT_SLOTS - 1, effects.findIndex(value => !value) < 0 ? effects.length : effects.findIndex(value => !value));
      paint();
    }));
    document.querySelectorAll('[data-browser-plugin-format]').forEach(button => button.addEventListener('click', () => {
      local.pluginFormat = button.dataset.browserPluginFormat;
      local.pluginSelected = 0;
      paint();
    }));
    document.querySelectorAll('[data-browser-plugin]').forEach(row => {
      row.addEventListener('click', () => {
        select(Number(row.dataset.browserPlugin), false);
        document.getElementById('browser-plugin-list')?.focus({ preventScroll: true });
      });
      row.addEventListener('dblclick', () => loadPlugin());
    });
    on('browser-search-retry', 'click', search);
    on('browser-search', 'input', event => {
      library().query = event.target.value;
      library().offset = 0;
      library().selected = 0;
      local.historyIndex = -1;
      queueSearch();
      saveState();
    });
    on('browser-plugin-search', 'input', event => { local.pluginQuery = event.target.value; local.pluginSelected = 0; paint(); });
    on('browser-audition', 'change', event => {
      local.previewEnabled = event.target.checked;
      if (!local.previewEnabled) stopPreview();
      saveState();
    });
    on('browser-favorites', 'click', () => { library().favorites = !library().favorites; changedFilter(); });
    on('browser-clear-filters', 'click', () => clearFilters());
    on('browser-add-root', 'click', addRoot); on('browser-empty-add', 'click', addRoot);
    on('browser-rechoose-folder', 'click', () => addRoot());
    on('browser-scan', 'click', scan); on('browser-empty-scan', 'click', scan);
    on('browser-edit-metadata', 'click', editMetadata);
    on('browser-page-prev', 'click', () => pageTo(library().offset - library().limit, 'first'));
    on('browser-page-next', 'click', () => pageTo(library().offset + library().limit, 'first'));
    on('browser-plugin-rescan', 'click', () => refreshPlugins(true));
    on('browser-plugin-load', 'click', () => loadPlugin());
    on('browser-plugin-replace', 'click', () => loadPlugin(false));
    on('browser-open-nks', 'click', () => showNKS(local.pluginKind));
    on('browser-plugin-slot', 'change', event => { local.slot = Number(event.target.value); });
    if (activeTab() === 'nks') window.STNKSBrowser?.bind();
    if (activeTab() === 'kits') window.STKitBrowser?.bind();
    if (!local.initialized) {
      local.initialized = true;
      queueMicrotask(async () => {
        await restoreState();
        roots();
        if (activeTab() === 'samples') {
          search();
        } else {
          local.backgroundDirty = true;
        }
        if (document.querySelector('.browser-workspace')) paint();
      });
    }
    if (activeTab() === 'plugins' && !local.pluginsLoaded) {
      local.pluginsLoaded = true;
      if (!(app().state.plugins || []).length) queueMicrotask(refreshPlugins);
    }
    const item = currentItem();
    if (activeTab() === 'samples'
        && item?.kind !== 'nks'
        && item?.path
        && !local.waveforms.has(item.path)
        && !local.wavePending.has(item.path)
        && !local.waveFailed.has(item.path)
        && local.waveformTimer === null) {
      local.waveformTimer = setTimeout(() => { local.waveformTimer = null; loadWaveform(currentItem()); }, 180);
    }
  }
  function key(event) {
    if (!document.querySelector('.browser-workspace')) return false;
    if (event.isComposing) return false;
    // The NKS tab owns its keys; the tab strip keeps the shared arrow handling below.
    if (nksBrowser() && !event.target?.closest?.('[data-browser-tab]')) return nksBrowser().key(event) || false;
    if (kitBrowser() && !event.target?.closest?.('[data-browser-tab]')) return kitBrowser().key(event) || false;
    const pluginTab = activeTab() === 'plugins';
    const searchID = pluginTab
        ? 'browser-plugin-search'
        : activeTab() === 'nks'
        ? 'nksb-search'
        : activeTab() === 'kits'
        ? 'kitb-search'
        : 'browser-search';
    const listID = pluginTab ? 'browser-plugin-list' : 'browser-results-list';
    const target = event.target;
    const plain = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
    const focusSearch = () => document.getElementById(searchID)?.focus();
    const focusResults = () => document.getElementById(listID)?.focus();
    if ((event.metaKey || event.ctrlKey)
        && event.key.toLowerCase() === 'f') { event.preventDefault(); document.getElementById(searchID)?.focus(); document.getElementById(searchID)?.select(); return true; }
    if (event.defaultPrevented && event.key !== 'Escape') return true;
    if (!pluginTab && event.key === 'Escape' && previewActive()) { event.preventDefault(); stopPreview(); return true; }
    const editing = /INPUT|TEXTAREA|SELECT/.test(event.target?.tagName || '') || event.target?.isContentEditable;
    if (editing) {
      if(event.target?.type==='checkbox'&&(event.key===' '||event.code==='Space'))return true;
      if (!pluginTab
          && target.id === searchID
          && event.altKey
          && !event.metaKey
          && !event.ctrlKey
          && !event.shiftKey
          && ['ArrowUp', 'ArrowDown'].includes(event.key)) {
        // Option-Up/Down walks remembered searches, newest first.
        event.preventDefault();
        let next = Math.max(0, Math.min(local.recent.length - 1, local.historyIndex + (event.key === 'ArrowUp' ? 1 : -1)));
        const lib = library(), current = { query: lib.query.trim(), kind: lib.kind, filters: filterPayload(), favorites: !!lib.favorites };
        // Skip the entry that is already on screen.
        if (local.historyIndex < 0
            && local.recent[next]
            && recentKey(local.recent[next]) === recentKey(current)) next = Math.min(local.recent.length - 1, next + 1);
        if (local.recent.length) applyRecent(next);
        return true;
      }
      if (target.id === searchID
          && event.key === 'ArrowDown'
          && plain) { event.preventDefault(); focusResults(); select(pluginTab ? local.pluginSelected : library().selected); return true; }
      if (target.id === searchID && event.key === 'Escape' && plain) { event.preventDefault(); focusResults(); return true; }
      return false;
    }
    // Choice groups keep their own arrows; a repaint must retain focus on the newly selected control.
    const choices = ['browser-tab', 'browser-kind', 'browser-plugin-kind', 'browser-plugin-format', 'browser-duration-preset'];
    for (const attribute of choices) {
      const selector = `[data-${attribute}]`, button = target?.closest?.(selector);
      if (!button || !plain) continue;
      const vertical = attribute === 'browser-plugin-kind';
      const previous = vertical ? 'ArrowUp' : 'ArrowLeft', next = vertical ? 'ArrowDown' : 'ArrowRight';
      if ([previous, next, 'Home', 'End'].includes(event.key)) {
        const buttons = Array.from(document.querySelectorAll(selector)).filter(value => !value.disabled && !value.hidden);
        const index = buttons.indexOf(button);
        if (index < 0 || !buttons.length) return false;
        const destination = event.key === 'Home'
            ? 0
            : event.key === 'End'
            ? buttons.length - 1
            : (index + (event.key === next ? 1 : -1) + buttons.length) % buttons.length;
        event.preventDefault();
        buttons[destination].focus(); buttons[destination].click();
        document.getElementById(buttons[destination].id)?.focus({ preventScroll: true });
        return true;
      }
      if (event.key === (vertical ? 'ArrowRight' : 'ArrowDown')) {
        event.preventDefault();
        if (attribute === 'browser-tab') focusSearch(); else focusResults();
        return true;
      }
      if (event.key === (vertical ? 'ArrowLeft' : 'ArrowUp')) {
        event.preventDefault();
        if (vertical) document.getElementById(`browser-tab-${activeTab()}`)?.focus(); else focusSearch();
        return true;
      }
    }
    const facet=event.target?.closest?.('.browser-facet');
    if(facet&&plain&&['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End'].includes(event.key)){
      const options=Array.from(facet.querySelectorAll('[data-browser-facet],[data-browser-recent]')),index=options.indexOf(event.target);
      const isSummary = event.target === facet.querySelector('summary');
      if(index<0&&!isSummary){if(event.key==='ArrowRight'){event.preventDefault();focusResults();return true;}return false;}
      event.preventDefault();
      if(event.key==='ArrowLeft'){facet.open=false;facet.querySelector('summary')?.focus();}
      else if(event.key==='ArrowRight'){if(index>=0)focusResults();else{facet.open=true;(facet.querySelector('[data-browser-facet-search]')||options[0])?.focus();}}
      else if(index>=0){const next=event.key==='Home'
          ?0
          :event.key==='End'
          ?options.length-1
          :Math.max(0,Math.min(options.length-1,index+(event.key==='ArrowDown'?1:-1)));options[next]?.focus();}
      else if(event.key==='ArrowDown'&&facet.open)options[0]?.focus();
      return true;
    }
    if (event.target?.closest?.('button, a, summary') && (event.key === 'Enter' || event.key === ' ' || event.code === 'Space')) return true;
    const inResults = target?.id === listID || (!!target?.closest?.(`#${listID}`) && !target.closest('button, a, summary'));
    if (!pluginTab && inResults && !event.altKey && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'a') {
      event.preventDefault(); if (resultsCurrent()) { local.marked = new Set(library().items.map(item => item.path)); local.selectionAnchor = library().selected; paint(); } return true;
    }
    if (event.metaKey || event.ctrlKey) return false;
    if (!pluginTab
        && inResults
        && plain
        && event.key === 'Escape'
        && local.marked.size) { event.preventDefault(); local.marked.clear(); local.selectionAnchor = null; paint(); return true; }
    if (!pluginTab && inResults && plain && event.key.toLowerCase() === 'e') { event.preventDefault(); editMetadata(); return true; }
    if (plain
        && !inResults
        && ((event.key === 'ArrowRight' && target?.closest?.('.browser-sidebar')) || (event.key === 'ArrowLeft' && target?.closest?.('.browser-detail')))) {
      event.preventDefault(); focusResults(); return true;
    }
    const selected = pluginTab ? local.pluginSelected : library().selected;
    const total = pluginTab ? filteredPlugins().length : library().items.length;
    if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End'].includes(event.key)) {
      const range = !pluginTab && event.shiftKey && !event.altKey;
      if (!inResults || (!plain && !range)) return false;
      event.preventDefault();
      const lib = library();
      // Keyboard traversal continues across result pages instead of stopping at the 150th sound.
      if (!pluginTab
          && !range
          && ['ArrowDown', 'PageDown'].includes(event.key)
          && total
          && selected >= total - 1
          && lib.offset + total < lib.total) { pageTo(lib.offset + lib.limit, 'first'); return true; }
      if (!pluginTab
          && !range
          && ['ArrowUp', 'PageUp'].includes(event.key)
          && selected <= 0
          && lib.offset > 0) { pageTo(lib.offset - lib.limit, 'last'); return true; }
      if (!range && event.key === 'ArrowUp' && selected <= 0) { focusSearch(); return true; }
      if (!total) return true;
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? total - 1 : selected + {
        ArrowUp: -1,
        ArrowDown: 1,
        PageUp: -12,
        PageDown: 12
      }[event.key];
      select(next, !range, range ? 'range' : ''); return true;
    }
    if (inResults && plain && ['ArrowLeft','ArrowRight'].includes(event.key)) {
      event.preventDefault();
      const id = event.key === 'ArrowLeft'
          ? pluginTab ? `browser-plugin-kind-${local.pluginKind}` : `browser-kind-${library().kind}`
          : pluginTab
          ? 'browser-plugin-load'
          : 'browser-load';
      const control = document.getElementById(id); if (!control?.disabled) control?.focus();
      return true;
    }
    if (event.key === 'Enter') { event.preventDefault(); load(activeTab() === 'plugins'
        ? !event.altKey
        : activeTab() === 'samples' && currentItem()?.kind !== 'nks'
        ? !event.altKey
        : event.altKey); return true; }
    if (!pluginTab && event.code === 'Space') { event.preventDefault(); preview(currentItem(), true); return true; }
    if (!pluginTab && event.key.toLowerCase() === 'f' && !event.altKey) { event.preventDefault(); favorite(); return true; }
    return false;
  }
  function event(type, payload = {}) {
    window.STNKSBrowser?.event(type, payload);
    window.STKitBrowser?.event(type, payload);
    if(type==='library.metadata'){if(payload.durationCoverage)library().durationCoverage=payload.durationCoverage;queueBackgroundSearch(300);return;}
    if (type === 'library.progress' || type === 'library.results') {
      const lib = library(); lib.scanning = !!payload.scanning; lib.count = payload.count ?? lib.count;
      updateLibraryStatus(payload);
      if (type === 'library.results') queueBackgroundSearch(payload.scanning ? 450 : 0);
      // The cached index (loaded after launch) and drive mounts change root counts and availability.
      if (type === 'library.results' && (payload.cached || payload.volumes) && local.initialized) roots();
      else if (document.querySelector('.browser-workspace') && local.progressTimer === null) {
        local.progressTimer = setTimeout(() => { local.progressTimer = null; if (document.querySelector('.browser-workspace')) paint(); }, 250);
      }
    }
    if (type === 'plugins.catalogue') {
      local.pluginScan = payload;
      if (!payload.scanning) { clearTimeout(local.pluginListTimer); local.pluginListTimer = setTimeout(() => { local.pluginListTimer = null; refreshPlugins(false); },
          50); }
      else if (document.querySelector('.browser-plugins-layout')
          && !local.pluginListTimer) local.pluginListTimer = setTimeout(() => { local.pluginListTimer = null; refreshPlugins(false); },
          1500);
      return;
    }
    if (type === 'sample.previewEnded') {
      local.previewPath = '';
      if (document.querySelector('.browser-workspace')) paint();
    }
  }
  function showPlugins(kind = 'instrument') {
    stopPreview(); window.STNKSBrowser?.stopPreview(); window.STKitBrowser?.stopPreview();
    local.tab = 'plugins'; local.pluginKind = kind === 'effect' ? 'effect' : 'instrument';
    local.pluginQuery = ''; local.pluginFormat = 'all'; local.pluginSelected = 0;
    const effects = effectSlots();
    local.slot = Math.min(INSERT_SLOTS - 1, effects.findIndex(value => !value) < 0 ? effects.length : effects.findIndex(value => !value));
    app().state.view = 'browser'; paint();
    document.getElementById('browser-plugin-search')?.focus();
  }
  function showNKS(kind = 'instrument') {
    stopPreview(); window.STKitBrowser?.stopPreview(); local.tab = 'nks'; app().state.view = 'browser';
    window.STNKSBrowser?.show(kind === 'effect' ? 'effect' : 'instrument'); paint();
    window.STNKSBrowser?.focusSearch();
  }
  function showKits() {
    stopPreview(); window.STNKSBrowser?.stopPreview(); local.tab = 'kits'; app().state.view = 'browser'; paint();
    window.STKitBrowser?.focusSearch();
  }
  function contextItems({ target }) {
    if (target.closest?.('.kitb-layout')) return window.STKitBrowser?.contextItems({ target }) || null;
    if (target.closest?.('.nksb-layout')) return window.STNKSBrowser?.contextItems({ target }) || null;
    const pluginRow = target.closest?.('[data-browser-plugin]');
    if (pluginRow) {
      local.pluginSelected = Number(pluginRow.dataset.browserPlugin); paint();
      const plugin = filteredPlugins()[local.pluginSelected];
      if (!plugin) return null;
      const project = app().state.project, track = app().state.track, instrument = app().state.instrument;
      return [{ header: true, title: plugin.name },
        { title: local.pluginKind === 'instrument'
            ? 'Add plug-in on new track'
            : 'Load effect', key: '↩', enabled: !local.busy, action: () => {
          const selected = filteredPlugins()[local.pluginSelected];
          if (activeTab() !== 'plugins'
              || selected?.id !== plugin.id
              || selected?.bundlePath !== plugin.bundlePath
              || app().state.project !== project
              || app().state.track !== track
              || app().state.instrument !== instrument) { app().toast('The plug-in selection or destination changed. Choose it again.',
                  'info'); return; }
          return loadPlugin();
        } },
        plugin.bundlePath
            ? { title: 'Show in Finder', action: () => app().request('app.external', { path: plugin.bundlePath, reveal: true }) }
            : null,
        { title: 'Rescan plug-ins', action: () => refreshPlugins(true) }];
    }
    const row = target.closest?.('[data-browser-item]');
    if (!row) return null;
    const index = Number(row.dataset.browserItem);
    if (library().selected !== index) select(index, false);
    const item = currentItem();
    if (!item) return null;
    const nks = item.kind === 'nks';
    const project = app().state.project,
      track = app().state.track,
      instrument = app().state.instrument;
    const forSelection = action => () => {
      if (activeTab() !== 'samples'
          || currentItem()?.path !== item.path
          || app().state.project !== project
          || app().state.track !== track
          || app().state.instrument !== instrument) { app().toast('The sound selection or destination changed. Choose it again.', 'info'); return; }
      return action();
    };
    return [{ header: true, title: item.name },
      { title: nks
          ? 'Load preset'
          : 'Add sample on new track', key: '↩', enabled: !local.busy, action: forSelection(() => load()) },
      { title: nks
          ? 'Load preset as new instrument'
          : 'Replace selected instrument', key: '⌥↩', enabled: !local.busy, action: forSelection(() => load(nks)) },
      { title: local.previewPath === item.path ? 'Stop preview' : 'Preview', key: 'Space', action: () => local.previewPath === item.path
          ? stopPreview()
          : preview(item, true) },
      { separator: true },
      { title: item.favorite ? 'Remove from favorites' : 'Add to favorites', key: 'F', action: () => favorite(item) },
      { title: 'Edit details…', key: 'E', action: forSelection(editMetadata) },
      { title: 'Show in Finder', action: () => app().request('app.external', { path: item.path, reveal: true }) }];
  }
  const searchActive = () => activeTab() === 'kits'
      ? window.STKitBrowser?.search()
      : activeTab() === 'nks'
      ? window.STNKSBrowser?.search()
      : search();
  const stopAllPreviews = () => {
    window.STPreviewPrefetch?.cancel();
    window.STNKSBrowser?.stopPreview();
    window.STKitBrowser?.stopPreview();
    return stopPreview();
  };
  // Hardware controllers (mk3.js): the same results, filters, previews and loads as the keyboard.
  // Single-choice filter fields a controller encoder steps through (none → each facet value → none).
  const controllerFields = ['categories', 'sampleTypes', 'keys', 'packs', 'collections'];
  const controller = {
    get tab() { return activeTab(); },
    setTab(tab) {
      if (tab === 'kits') return showKits();
      if (tab === 'nks') return showNKS(window.STNKSBrowser?._local?.kind || 'instrument');
      if (tab === 'plugins') return showPlugins(local.pluginKind);
      if (local.tab !== tab) { stopPreview(); window.STNKSBrowser?.stopPreview(); window.STKitBrowser?.stopPreview(); }
      local.tab = 'samples'; if (app().state.view !== 'browser') app().setView('browser'); else paint();
      if (local.backgroundDirty) queueBackgroundSearch(0);
    },
    get fields() {
      return kitBrowser()?.controller.fields || controllerFields;
    },
    filterLabel: field => kitBrowser()?.controller.filterLabel(field) || filterNames[field] || field,
    filterValue(field) {
      if (kitBrowser()) return kitBrowser().controller.filterValue(field);
      const value = filters()[field]?.[0];
      return value ? optionLabel(field, value) + (filters()[field].length > 1 ? ` +${filters()[field].length - 1}` : '') : 'All';
    },
    cycleFilter(field, delta) {
      if (kitBrowser()) return kitBrowser().controller.cycle(field, delta);
      if (!controllerFields.includes(field)) return;
      const options = controller.options(field).map(item => item.value),
        current = filters()[field][0],
        index = current ? options.indexOf(current) : -1;
      const next = index + Math.sign(delta), value = next < 0 || next >= options.length ? null : options[next];
      filters()[field] = value ? [value] : []; changedFilter();
    },
    // MK3 filter lists: stable alphabetical order (the panel lists selected values first, which would move the
    // controller's cursor), the applied single value, and choosing one value (null = all).
    options(field) {
      if (kitBrowser()) return kitBrowser().controller.options?.(field) || [];
      if (!controllerFields.includes(field)) return [];
      return facetOptions(field).sort((a, b) => a.label.localeCompare(b.label));
    },
    current(field) { if (kitBrowser()) return kitBrowser().controller.current?.(field) ?? null; return filters()[field]?.[0] ?? null; },
    choose(field, value) {
      if (kitBrowser()) return kitBrowser().controller.choose?.(field, value);
      if (!controllerFields.includes(field)) return;
      filters()[field] = value ? [value] : [];
      changedFilter();
    },
    get kind() { return activeTab() === 'plugins' ? local.pluginKind : library().kind; },
    cycleKind(delta) {
      if (activeTab() === 'plugins') { local.pluginKind = local.pluginKind === 'instrument'
          ? 'effect'
          : 'instrument'; local.pluginSelected = 0; paint(); return; }
      const kinds = ['all', 'sample', 'nks'],
        lib = library();
      lib.kind = kinds[(kinds.indexOf(lib.kind) + Math.sign(delta) + kinds.length) % kinds.length];
      changedFilter();
    },
    get format() { return local.pluginFormat; },
    cycleFormat(delta) {
      const formats = ['all', 'AU', 'VST3', 'CLAP'];
      local.pluginFormat = formats[(formats.indexOf(local.pluginFormat) + Math.sign(delta) + formats.length) % formats.length];
      local.pluginSelected = 0;
      paint();
    },
    list() {
      if (kitBrowser()) return kitBrowser().controller.list();
      if (activeTab() === 'plugins') {
        const items = filteredPlugins();
        return { tab: 'plugins', items: items.map(plugin => ({ name: plugin.name, detail: `${plugin.manufacturer || ''} · ${pluginFormat(plugin)}` })), selected: local.pluginSelected, total: items.length, offset: 0, busy: local.busy };
      }
      const lib = library(), current = resultsCurrent();
      return { tab: 'samples', items: current
          ? lib.items.map(item => ({ name: item.name, detail: [item.pack || item.collection,
              item.category].filter(Boolean).join(' · '), favorite: !!item.favorite, kind: item.kind, bpm: item.bpm, key: item.key, offline: item.available === false }))
          : [],
        selected: lib.selected, total: lib.total, offset: lib.offset, busy: local.busy || !current, favorites: !!lib.favorites, preview: !!local.previewPath, autoPreview: local.previewEnabled, query: lib.query, filters: activeFilterCount() };
    },
    move(delta) {
      if (kitBrowser()) return kitBrowser().controller.move(delta);
      if (activeTab() === 'plugins') { select(local.pluginSelected + delta); return; }
      const lib = library(), total = lib.items.length, selected = lib.selected;
      if (delta > 0 && total && selected >= total - 1 && lib.offset + total < lib.total) { pageTo(lib.offset + lib.limit, 'first'); return; }
      if (delta < 0 && selected <= 0 && lib.offset > 0) { pageTo(lib.offset - lib.limit, 'last'); return; }
      if (total) select(selected + delta);
    },
    page(delta) {
      if (kitBrowser()) return kitBrowser().controller.page(delta);
      if (activeTab() === 'plugins') {
        select(local.pluginSelected + delta * 12);
        return;
      }
      const lib = library();
      if (delta > 0 && lib.offset + lib.items.length < lib.total) {
        pageTo(lib.offset + lib.limit);
      } else if (delta < 0 && lib.offset > 0) pageTo(lib.offset - lib.limit);
    },
    preview: () => kitBrowser() ? kitBrowser().controller.preview() : preview(currentItem(), true),
    load: (asNew) => kitBrowser() ? kitBrowser().controller.load(asNew ? 'open' : 'stay') : load(asNew),
    favorite: () => kitBrowser() ? kitBrowser().controller.favorite() : favorite(),
    toggleFavorites() {
      if (kitBrowser()) return kitBrowser().controller.toggleFavorites();
      const lib = library();
      lib.favorites = !lib.favorites;
      changedFilter();
    },
    toggleAutoPreview() {
      if (kitBrowser()) return kitBrowser().controller.toggleAutoPreview();
      local.previewEnabled = !local.previewEnabled;
      if (!local.previewEnabled) stopPreview();
      saveState();
      paint();
    },
    clearFilters: () => kitBrowser() ? kitBrowser().controller.clearFilters() : clearFilters(false)
  };
  window.STBrowser = {
    render,
    bind,
    key,
    search: searchActive,
    focusSearch,
    event,
    showPlugins,
    showNKS,
    showKits,
    saveState,
    stopPreview: stopAllPreviews,
    contextItems,
    controller
  };
})();
