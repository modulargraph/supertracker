/* Find the instrument's rendered controls without changing their values. */
(() => {
  'use strict';
  const kind = 'instrument-navigation', controls = 'input,select,textarea,button,summary,[role="button"],[role="slider"]';
  const S = () => window.ST, $ = id => document.getElementById(id);
  const clean = value => String(value || '').replace(/\s+/g, ' ').trim();
  const fold = value => clean(value).normalize('NFKD').replace(/[\u0300-\u036f]/g, '').toLocaleLowerCase();
  const esc = value => S().escapeHTML(value);
  let active = null;

  // ---- Instrument pages (Maschine/Polyend-style): one focused page at a time. ----
  // Every page stays mounted (hidden), so ids, automation and controller hooks keep
  // working; switching pages only toggles visibility and never edits the sound.
  const PAGES = {
    sample: [['sample', 'Sample'], ['slices', 'Slices'], ['zones', 'Zones'], ['modulation', 'Modulation'], ['voice', 'Voice & Record']]
  };
  const PAGE_FOCUS = {
    sample: ['#param-value-wavetablePosition', '#sampler-wave-canvas'], voice: ['#sampler-voice-mode'],
    slices: ['[data-sampler-audition][tabindex="0"]:not(:disabled)', '#sampler-detect-transients:not(:disabled)', '#sampler-equal-count'],
    zones: ['[data-sampler-zone-field]', '#sampler-add-zone'], modulation: ['input[type="number"]', 'select']
  };
  const selectedPage = { sample: 'sample' };
  const currentInstrument = () => S().state.project?.instruments[S().state.instrument];
  const pageFocusable = node => node && !node.disabled && visible(node) && !node.closest('[hidden]') && Number(node.tabIndex ?? 0) >= 0;
  const pages = {
    list(inst = currentInstrument()) { return PAGES[inst?.type] || []; },
    current(inst = currentInstrument()) {
      const list = pages.list(inst);
      return list.some(([id]) => id === selectedPage[inst?.type]) ? selectedPage[inst.type] : list[0]?.[0] || '';
    },
    label(id, inst = currentInstrument()) { return pages.list(inst).find(([key]) => key === id)?.[1] || ''; },
    // Attributes for a page container; app.js renders the panels inside it.
    attributes(inst, id) {
      const label = pages.label(id, inst), shown = pages.current(inst) === id;
      return `id="inst-page-${id}" class="inst-page inst-page-${id}" data-inst-page="${id}" data-inst-page-label="${esc(label)}" role="tabpanel" aria-label="${esc(label)} page" aria-labelledby="inst-page-tab-${id}"${shown ? '' : ' hidden'}`;
    },
    renderBar(inst) {
      const list = pages.list(inst); if (!list.length) return '';
      const current = pages.current(inst);
      const markers = inst.slicePoints?.length || Number(inst.sliceCount) || 16;
      const counts = { slices: inst.mode === 'slice' ? markers : 0, zones: (inst.zones || []).length };
      return `<div class="inst-pages" role="tablist" aria-label="Instrument pages · ⌥1–${list.length} or ⌃Tab">${list.map(([id, label], index) => `<button id="inst-page-tab-${id}" role="tab" data-inst-page-tab="${id}" aria-selected="${id === current}" aria-controls="inst-page-${id}" aria-keyshortcuts="Alt+${index + 1}" tabindex="${id === current ? 0 : -1}" class="${id === current ? 'active' : ''}" title="${esc(label)} page · ⌥${index + 1}"><kbd>${index + 1}</kbd>${esc(label)}${counts[id] ? `<span class="inst-page-count">${counts[id]}</span>` : ''}</button>`).join('')}</div>`;
    },
    // target: page id or 1-based page number. focus: 'page' (primary control), 'tab', or false.
    show(target, { focus = 'page' } = {}) {
      const state = S().state, inst = currentInstrument(), list = pages.list(inst);
      if (state.view !== 'instrument' || !list.length) return false;
      const entry = typeof target === 'number' ? list[target - 1] : list.find(([id]) => id === target);
      if (!entry) return false;
      const id = entry[0], previous = document.activeElement;
      selectedPage[inst.type] = id;
      const containers = Array.from(document.querySelectorAll('[data-inst-page]'));
      containers.forEach(page => { page.hidden = page.getAttribute('data-inst-page') !== id; });
      document.querySelectorAll('[data-inst-page-tab]').forEach(tab => {
        const on = tab.getAttribute('data-inst-page-tab') === id;
        tab.setAttribute('aria-selected', String(on)); tab.tabIndex = on ? 0 : -1; tab.classList?.toggle?.('active', on);
      });
      const tab = $('inst-page-tab-' + id), page = $('inst-page-' + id);
      if (id === 'sample') window.STSampler?.redraw?.();
      // Pages only toggle visibility (no render), so the waveform's live playheads start or stop here.
      window.STSamplerPlayheads?.sync?.();
      let destination = null;
      if (focus === 'page' && page) {
        for (const selector of [...(PAGE_FOCUS[id] || []), 'input:not([type="range"]),select,button,[tabindex="0"]']) {
          destination = Array.from(page.querySelectorAll(selector)).find(pageFocusable); if (destination) break;
        }
      }
      if (!destination && (focus === 'tab' || focus === 'page' || (previous && previous.closest?.('[data-inst-page]')?.hidden))) destination = tab;
      if (page && focus !== false) { const scroller = $('main')?.querySelector?.('[data-scroll="instrument"]'); if (scroller) scroller.scrollTop = 0; }
      if (destination) { destination.focus?.({ preventScroll: true }); destination.scrollIntoView?.({ block: 'nearest', inline: 'nearest' }); }
      return true;
    },
    step(delta, options) {
      const list = pages.list(); if (!list.length) return false;
      const index = list.findIndex(([id]) => id === pages.current());
      return pages.show(list[(index + delta + list.length) % list.length][0], options);
    },
    // Opens the page holding node (Find control, F6, fixtures); returns true when it switched.
    reveal(node) {
      const page = node?.closest?.('[data-inst-page]');
      if (!page || !(page.hidden === true || page.matches?.('[hidden]'))) return false;
      return pages.show(page.getAttribute('data-inst-page'), { focus: false });
    },
    key(event) {
      const state = S().state;
      if (state.view !== 'instrument' || state.modal || event.defaultPrevented || event.isComposing || !pages.list().length) return false;
      const done = () => { event.preventDefault(); event.stopPropagation?.(); return true; };
      const digit = /^Digit([1-9])$/.exec(event.code || '')?.[1];
      if (digit && event.altKey && !event.metaKey && !event.ctrlKey && !event.shiftKey) {
        if (Number(digit) > pages.list().length) return false;
        pages.show(Number(digit)); return done();
      }
      if (event.key === 'Tab' && event.ctrlKey && !event.metaKey && !event.altKey) { pages.step(event.shiftKey ? -1 : 1); return done(); }
      const tab = event.target?.dataset?.instPageTab;
      if (tab !== undefined && !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
        const list = pages.list(), index = list.findIndex(([id]) => id === tab);
        const next = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: list.length - 1 }[event.key];
        if (next !== undefined) { pages.show(list[(next + list.length) % list.length][0], { focus: 'tab' }); return done(); }
        if (event.key === 'ArrowDown') { pages.show(tab, { focus: 'page' }); return done(); }
      }
      return false;
    },
    bind() {
      document.querySelectorAll('[data-inst-page-tab]').forEach(tab => { tab.onclick = () => pages.show(tab.getAttribute('data-inst-page-tab'), { focus: 'tab' }); });
    }
  };

  function visible(node) {
    if (node.isConnected === false || node.type === 'hidden') return false;
    // Controls on another instrument page stay discoverable: a jump opens that page first.
    const page = node.closest('[data-inst-page]');
    for (let parent = node; parent; parent = parent.parentElement) {
      if (parent === page) continue;
      if (parent.matches?.('[hidden],[aria-hidden="true"],[inert]')) return false;
      const style = window.getComputedStyle?.(parent);
      if (style?.display === 'none' || style?.visibility === 'hidden') return false;
    }
    return true; // Closed <details> remain discoverable and are opened on a jump.
  }
  const disabled = node => !!node.disabled || !!node.closest('[aria-disabled="true"],fieldset[disabled]');
  function labelText(label) {
    const copy = label.cloneNode(true);
    copy.querySelectorAll('input,select,textarea,button,output,[aria-hidden="true"]').forEach(node => node.remove());
    return clean(copy.textContent);
  }
  function nameOf(node) {
    const labelled = clean(node.getAttribute('aria-labelledby')).split(' ').filter(Boolean).map(id => $(id)?.textContent || '').join(' ');
    const labels = Array.from(node.labels || []), wrapping = node.closest('label');
    if (wrapping && !labels.includes(wrapping)) labels.push(wrapping);
    return clean(node.getAttribute('aria-label') || labelled || node.dataset.name || node.dataset.nksName || labels.map(labelText).join(' ') ||
      (['BUTTON', 'SUMMARY'].includes(node.tagName) ? node.textContent : '') || node.getAttribute('title') ||
      node.id.replace(/^(inst|instrument|param|sampler|mod)-/, '').replace(/([a-z])([A-Z])/g, '$1 $2').replace(/[-_]/g, ' '));
  }
  function contextOf(node) {
    const panel = node.closest('.panel,section,fieldset'), heading = panel?.querySelector('h2,h3,h4,legend');
    const group = node.closest('[data-mod-lfo]')?.querySelector('.mod-lfo-name')?.textContent;
    const nks = node.closest('[data-nks-root]'), page = nks?.querySelector('[data-nks-page]');
    const instrumentPage = node.closest('[data-inst-page]')?.getAttribute('data-inst-page-label');
    return [...new Set([instrumentPage, panel?.getAttribute('aria-label'), heading?.textContent, group, page?.selectedOptions?.[0]?.textContent].map(clean).filter(Boolean))].join(' · ');
  }
  function controlKey(node) {
    if (node.dataset.instParam) return 'instrument:' + node.dataset.instParam;
    if (node.dataset.modParam) return 'mod:' + node.dataset.modInstrument + ':' + node.dataset.modParam;
    if (node.dataset.nksAddress) return 'nks:' + node.closest('[data-nks-root]')?.dataset.nksRoot + ':' + node.dataset.nksAddress;
    return node;
  }
  function discover() {
    const root = document.querySelector('#main .workspace-main');
    if (!root) return [];
    const entries = [], keys = new Map();
    for (const node of root.querySelectorAll('h2,h3,h4,legend,' + controls)) {
      if (!visible(node) || node.dataset.piano !== undefined) continue;
      // Roving lists already expose their current row. Their section remains a
      // single useful destination even when a break has 128 slice markers.
      if (node.closest('[data-sampler-row]') && node.getAttribute('tabindex') === '-1') continue;
      const section = /^H[234]$/.test(node.tagName) || node.tagName === 'LEGEND';
      const name = section ? clean(node.textContent) : nameOf(node);
      if (!name) continue;
      const group = contextOf(node), item = { node, name, group, section, unavailable: !section && disabled(node), order: entries.length };
      item.search = fold([name, group, node.getAttribute('title'), node.id, node.id.replace(/([a-z])([A-Z])/g, '$1 $2')].join(' '));
      const key = section ? node : controlKey(node), old = keys.get(key);
      if (old) {
        old.search += ' ' + item.search;
        if (node.type === 'number' && old.node.type !== 'number') Object.assign(old, item, { order: old.order, search: old.search });
      } else { keys.set(key, item); entries.push(item); }
    }
    return entries;
  }
  const valid = entry => active === entry && S().state.modal === kind && S().state.view === 'instrument' &&
    S().state.project === entry.project && S().state.instrument === entry.index && S().state.track === entry.track && entry.project.instruments[entry.index] === entry.instrument;
  function focusTarget(item) {
    let node = item.node;
    pages.reveal(node);
    if (item.section) {
      const panel = node.closest('.panel,section,fieldset'), candidates = Array.from(panel?.querySelectorAll(controls) || []).filter(control => visible(control) && !disabled(control) && control.dataset.piano === undefined);
      node = candidates.find(control => control.getAttribute('tabindex') === '0') || candidates.find(control => ['INPUT','SELECT','TEXTAREA'].includes(control.tagName) && control.type !== 'range') || candidates[0] || node;
    }
    for (let parent = node.parentElement; parent; parent = parent.parentElement) if (parent.tagName === 'DETAILS') parent.open = true;
    if (!node.matches(controls)) node.setAttribute('tabindex', '-1');
    node.scrollIntoView({ block: 'center', inline: 'nearest' });
    node.focus({ preventScroll: true });
  }
  function choose(entry, item) {
    if (!valid(entry)) { if (S().state.modal === kind) S().closeModal(); return; }
    if (!item || item.node.isConnected === false || !visible(item.node)) { S().toast('The instrument controls changed. Search again.'); paint(entry); return; }
    if (!item.section && disabled(item.node)) { S().toast(`${item.name} is currently unavailable.`); return; }
    S().closeModal(); active = null;
    focusTarget(item); // No click, input, change or audio request is dispatched.
  }
  function select(entry, index) {
    entry.selected = Math.max(0, Math.min(index, entry.matches.length - 1));
    $('instrument-find-results')?.querySelectorAll('[data-instrument-jump]').forEach((button, i) => {
      button.classList.toggle('active', i === entry.selected); button.setAttribute('aria-selected', String(i === entry.selected));
    });
    const selected = $('instrument-find-result-' + entry.selected), input = $('instrument-find-search');
    if (selected) { input.setAttribute('aria-activedescendant', selected.id); selected.scrollIntoView({ block: 'nearest' }); }
    else input.removeAttribute('aria-activedescendant');
  }
  function paint(entry) {
    if (!valid(entry)) return;
    const query = fold($('instrument-find-search').value), words = query.split(/\s+/).filter(Boolean);
    entry.matches = discover().filter(item => words.every(word => item.search.includes(word) || (word.endsWith('ing') && item.search.includes(word.slice(0, -3))))).sort((a, b) => {
      const score = item => query ? (fold(item.name) === query ? 0 : fold(item.name).startsWith(query) ? 1 : 2) : item.section ? 0 : 1;
      return score(a) - score(b) || Number(a.unavailable) - Number(b.unavailable) || a.order - b.order;
    });
    $('instrument-find-results').innerHTML = entry.matches.map((item, i) => `<button id="instrument-find-result-${i}" class="palette-item" data-instrument-jump="${i}" role="option" aria-selected="false" ${item.unavailable ? 'aria-disabled="true"' : ''} tabindex="-1"><span>${esc(item.name)}<small class="muted" style="display:block;margin-top:3px;font-size:10px">${esc(item.section ? 'Section' : item.group || 'Instrument')}</small></span><span class="muted">${item.unavailable ? 'Unavailable' : item.section ? 'Section' : 'Jump'}</span></button>`).join('') || '<div class="empty">No matching instrument controls</div>';
    $('instrument-find-results').querySelectorAll('[data-instrument-jump]').forEach(button => { button.onclick = () => choose(entry, entry.matches[Number(button.dataset.instrumentJump)]); });
    $('instrument-find-count').textContent = `${entry.matches.length} ${entry.matches.length === 1 ? 'destination' : 'destinations'}`;
    select(entry, 0);
  }
  function show() {
    const state = S().state;
    if (state.view !== 'instrument' || state.modal && state.modal !== kind) return false;
    if (state.modal === kind) { $('instrument-find-search')?.focus(); $('instrument-find-search')?.select(); return true; }
    const entry = { project: state.project, instrument: state.project.instruments[state.instrument], index: state.instrument, track: state.track, matches: [], selected: 0 };
    S().openModal('Find instrument control', '<input id="instrument-find-search" class="palette-input" type="search" placeholder="Find a control or section…" autocomplete="off" aria-label="Find instrument control" role="combobox" aria-autocomplete="list" aria-expanded="true" aria-controls="instrument-find-results"><div id="instrument-find-results" class="palette-list" role="listbox" aria-label="Instrument controls and sections"></div><p class="muted" style="padding:10px 16px;font-size:11px"><span id="instrument-find-count" role="status" aria-live="polite"></span> · ↑↓ choose · Return jump · Escape back</p>', '', { kind, className: 'palette', noHeader: true });
    if (state.modal !== kind) return false;
    active = entry;
    const input = $('instrument-find-search'); input.oninput = () => paint(entry);
    input.onkeydown = event => {
      if (event.isComposing) return;
      if (!['ArrowDown','ArrowUp','Enter','Escape'].includes(event.key)) return;
      event.preventDefault(); event.stopPropagation();
      if (event.key === 'Escape') { S().closeModal(); active = null; }
      else if (event.key === 'Enter') choose(entry, entry.matches[entry.selected]);
      else select(entry, entry.selected + (event.key === 'ArrowDown' ? 1 : -1));
    };
    paint(entry); input.focus(); return true;
  }
  function key(event) {
    if (event.defaultPrevented || event.isComposing || !(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey || !(event.code === 'KeyF' || event.key?.toLocaleLowerCase() === 'f')) return false;
    if (!show()) return false;
    event.preventDefault(); event.stopPropagation?.(); return true;
  }
  window.STInstrumentNavigation = { show, key };
  window.STInstrumentPages = pages;
})();
