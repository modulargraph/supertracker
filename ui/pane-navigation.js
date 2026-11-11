/* The blocks of each page and dialog for block navigation (block-nav.js), and the F6 / ⇧F6 facade.
   Order is reading order (F6); ⌥ + arrows use on-screen position. Pages marked { altArrows, escape } have been
   audited for ⌥ + arrow conflicts (docs/KEYBOARD_NAVIGATION.md). */
(() => {
  'use strict';
  const nav = window.STBlockNav;
  const S = () => window.ST;
  const audited = { altArrows: true, escape: true };
  const controls = 'button,input,select,textarea,summary,[tabindex]';
  const near = (selector, ancestor) => () => {
    const node = document.querySelector?.(selector);
    return node ? [node.closest?.(ancestor) || node] : [];
  };

  // Transport and view switcher come first on every page, matching their place at the top of the window.
  nav.register('*', () => [
    { id: 'transport', label: 'Transport', selector: '.topbar', prefer: ['#play'], keys: 'Space: play / stop' },
    { id: 'views', label: 'Views', selector: '.nav', prefer: ['.nav-button[tabindex="0"]', '.nav-button.active'], keys: '← →: choose view · ↓: open' },
    { id: 'sidebar', label: S().state.view === 'instrument' ? 'Instruments' : S().state.view === 'recording' ? 'Recording' : 'Patterns',
      selector: '.sidebar:not(.left-pane)', prefer: ['[data-pattern][tabindex="0"],[data-instrument][tabindex="0"]'], keys: '↑ ↓: choose · →: open' }
  ]);

  nav.register('song', [
    // View header buttons (export, add pattern) sit between the sidebar and the main content.
    { id: 'view-actions', label: 'Song actions', selector: '.workspace-main > .view-title' },
    { id: 'sequence', label: 'Pattern sequence', selector: '#sequence-list', keys: '↑ ↓: select · ← →: pattern · ⌥↑↓: move · Enter: edit' },
    { id: 'song-block', label: 'Selected song block', selector: '.song-seq > .panel-head,.song-seq-tools', prefer: ['#song-block-pattern'] },
    { id: 'song-settings', label: 'Song settings', selector: '.song-options', prefer: ['#loop-song'] },
    // Clip buttons keep native arrow behaviour (arrows never move focus off them).
    { id: 'clip-actions', label: 'Audio clip actions', selector: '.clips-panel > .panel-head', prefer: ['#clips-import'], arrows: false },
    { id: 'clip-tools', label: 'Audio edit cursor', selector: '.clips-toolbar', prefer: ['#clips-cursor'], arrows: false },
    { id: 'timeline', label: 'Audio timeline', selector: '#clips-timeline', keys: '↑ ↓: clip · ← →: move · S: split' },
    { id: 'clip-inspector', label: 'Clip inspector', selector: '.clips-inspector', prefer: ['#clips-name'], arrows: false }
  ], audited);

  nav.register('browser', [
    { id: 'browser-tabs', label: 'Browser tabs', selector: '.browser-tabs', prefer: ['[data-browser-tab][aria-selected="true"]', '[data-browser-tab][tabindex="0"]'],
      keys: '← →: Samples / Kits / NKS / Plug-ins' },
    { id: 'browser-search', label: 'Library search', selector: '.browser-search-wrap', prefer: ['input[type="search"]', 'input'],
      keys: 'type to search · ↓: results · ⌥↑↓: recent searches' },
    // Kits and NKS: library / product sidebar (its header buttons and the product list).
    { id: 'browser-products', label: 'Products', selector: '.nksb-sidebar', prefer: ['.nksb-product-list'], keys: '↑ ↓: choose product' },
    { id: 'browser-filters', label: 'Library filters', selector: '.browser-sidebar:not(.nksb-sidebar),.nksb-filters',
      prefer: ['[data-browser-plugin-kind][tabindex="0"]', '#browser-group-collections', '[data-browser-kind][tabindex="0"]'],
      keys: '↑ ↓: choose · → / Enter: open group' },
    { id: 'browser-summary', label: 'Result options', selector: '.browser-result-summary', prefer: ['#browser-audition'] },
    { id: 'browser-results', label: 'Library results', selector: '.browser-result-list,.browser-plugin-list',
      keys: '↑ ↓: select · Space: preview · Enter: load · ⌥Enter: new instrument' },
    { id: 'browser-pages', label: 'Result pages', selector: '.browser-list-footer' },
    { id: 'browser-detail', label: 'Sound details', selector: '.browser-detail', prefer: ['#browser-load,#browser-plugin-load,#nksb-load,#kitb-load'],
      keys: 'Enter: load' }
  ], audited);

  nav.register('recording', [
    { id: 'record-details', label: 'Recording details', selector: '#recording-setup .recording-content > .recording-grid,#recording-setup .recording-keys', prefer: ['#record-name'] },
    { id: 'record-input', label: 'Input device', selector: '#recording-setup #record-input-settings' },
    { id: 'record-meter', label: 'Input levels', selector: '#recording-setup .recording-meter' },
    { id: 'record-advanced', label: 'Playback & monitoring', selector: '#recording-setup #record-advanced', keys: 'Enter / Space: open' },
    { id: 'record-capture', label: 'Capture', selector: '#recording-setup .recording-inline-foot', prefer: ['#record-start'], keys: '⌘⇧R: start / stop' },
    // Before the inline recorder mounts, the whole capture panel is one block.
    { id: 'recording-actions', label: 'Recording setup', roots: () => document.querySelector?.('#recording-setup .recording-content') ? [] : [document.querySelector?.('#recording-setup')].filter(Boolean), prefer: ['#record-name'] },
    { id: 'recording-interface', label: 'Audio interface', selector: '.recording-interface' },
    { id: 'recording-inputs', label: 'Input routing', selector: '#recording-routing', prefer: ['#recording-routing-configure'] },
    { id: 'recording-after', label: 'After capture', selector: '.recording-capture-note' }
  ], audited);

  nav.register('automation', [
    { id: 'automation-scope', label: 'Pattern / song scope', selector: '.studio-automation-scope', prefer: ['#automation-pattern'], keys: '[ ]: pattern · \\: song timeline' },
    { id: 'automation-targets', label: 'Automation parameters', selector: '.studio-target-panel', prefer: ['#automation-search'], keys: '↑ ↓: choose · Enter: add lane' },
    { id: 'automation-lanes', label: 'Automation lanes', selector: '.studio-lane-stack', prefer: ['[data-select-lane][aria-selected="true"]'], keys: '↑ ↓: lane · Enter: envelope' },
    { id: 'automation-lane-settings', label: 'Lane settings', selector: '.studio-lane-header', prefer: ['#automation-write-mode'] },
    { id: 'automation-envelope', label: 'Automation envelope', selector: '#automation-envelope,#automation-song-envelope',
      keys: '← →: move point · ⌥← →: previous / next point · Enter: add' },
    { id: 'automation-points', label: 'Automation points', selector: '.studio-point-toolbar,.studio-points-table',
      prefer: ['.studio-point-row.is-selected [data-point-row]', '[data-point-row]', '#automation-add-point'] }
  ], audited);

  nav.register('modal:settings', () => [
    { id: 'settings-audio', label: 'Audio devices', roots: () => ['#audio-output', '#input-channel-left', '#audio-monitor']
        .flatMap(selector => near(selector, '.grid2,.row')()), prefer: ['#audio-output'] },
    { id: 'settings-midi', label: 'MIDI routing', roots: () => ['#midi-input', '#midi-enabled'].flatMap(selector => near(selector, '.grid2,.row')()),
      prefer: ['#midi-input'] },
    { id: 'settings-agent', label: 'AI assistant', selector: '.modal-body .agent-settings', prefer: ['#agent-enabled'] },
    { id: 'settings-actions', label: 'Apply or cancel', selector: '.modal-foot', prefer: ['#settings-apply'], keys: 'Enter: apply' }
  ], { altArrows: true });

  nav.register('modal:palette', [
    { id: 'palette-search', label: 'Search commands', selector: '#palette-search', keys: '↑ ↓: choose · Enter: run' },
    { id: 'palette-list', label: 'Commands', selector: '#palette-list' }
  ]);

  // ---- Pattern ----
  // The grid keeps every key it had: plain arrows, Tab, ⌥ + all four arrows (⌥↑↓ transpose, ⌥←→ cursor), Esc (cancel
  // a typed value, stop). From the grid, ⌥⌘ + arrows and F6 move between blocks. Elsewhere plain ⌥ + arrows do.
  // ⌘0 (command focusGrid) comes back to the grid's cursor cell from anywhere.
  const grid = () => document.getElementById?.('tracker');
  const cursorCell = () => grid()?.querySelector?.('tbody td.selected') || null;
  const box = node => { const r = node?.getBoundingClientRect?.(); return r && (r.width || r.height) ? r : null; };
  const clip = (r, c) => r && c ? { left: Math.max(r.left, c.left), top: Math.max(r.top, c.top), right: Math.min(r.right, c.right), bottom: Math.min(r.bottom, c.bottom) } : null;
  // The grid scrolls inside .tracker-wrap and its header row sticks to the top: blocks are their visible parts.
  function gridRect() {
    const table = grid(), wrap = box(table?.closest?.('.tracker-wrap')), head = box(table?.querySelector?.('thead'));
    const rect = clip(box(table), wrap);
    if (rect && head) rect.top = Math.min(rect.bottom - 1, Math.max(rect.top, head.bottom));
    return rect;
  }
  function headerRect() {
    const table = grid(), head = box(table?.querySelector?.('thead'));
    return clip(head, box(table?.closest?.('.tracker-wrap')));
  }
  // Strict ⌥ + arrow owner: the grid (and the page itself with nothing focused, where keys go to the grid).
  function gridOwns(target) {
    if (!target || target === document.body) return S().state.view === 'pattern';
    return !!target.closest?.('#tracker') && !target.closest('thead');
  }
  nav.register('pattern', [
    // The left pane: instruments over the pattern matrix (app-pattern-matrix.js).
    { id: 'instruments', label: 'Instruments', selector: '.left-pane-instruments', prefer: ['[data-instrument][tabindex="0"]'], keys: '↑ ↓: choose · →: grid' },
    { id: 'pattern-matrix', label: 'Patterns', selector: '.left-pane-patterns', prefer: ['#pattern-matrix'],
      keys: '← → on tabs: All patterns / Current song · ↑ ↓: choose · ⌥↑↓: move · N new · ⌘D duplicate · ⌫ remove · Enter: grid', esc: 'Esc: tabs' },
    { id: 'view-actions', label: 'Pattern actions', selector: '.workspace-main > .view-title', keys: 'arrows: next button · Enter / Space: press' },
    { id: 'pattern-destination', label: 'Selected track', selector: '.pattern-destination', prefer: ['#pattern-destination-lane'],
      keys: 'menus: ↑ ↓ choose · arrows on buttons: next button', esc: 'Esc in a menu: grid' },
    { id: 'pattern-tools', label: 'Pattern settings', selector: '.pattern-toolbar .pattern-group', prefer: ['#pattern-length'],
      keys: 'Length · Step · Follow · Fill · Gate · Hide empty · Pad keys · arrows on buttons: next button' },
    { id: 'pattern-perform', label: 'Keyboard performance', selector: '.pattern-toolbar .perf-group', prefer: ['#perf-toggle'],
      keys: 'Enter on Play: show / hide octave, key, scale, chord, velocity' },
    { id: 'track-headers', label: 'Track headers', selector: '#tracker thead', stops: 'button', rect: headerRect,
      prefer: ['.track-head.selected-track button[tabindex="0"]', '.group-head.is-selected button[tabindex="0"]', 'button[tabindex="0"]'],
      keys: '← → ↑ ↓: header buttons (M S ◂ × instrument, group ▾ M S) · Enter / Space: press · Home / End: first / last' },
    { id: 'pattern-grid', label: 'Pattern grid', selector: '#tracker', rect: gridRect, anchor: cursorCell, reveal: cursorCell, ownKeys: true,
      keys: 'arrows: cursor · Tab: track · ⌥↑↓: transpose · Enter: hear / FX · ⌃Tab: step editor', esc: 'Esc: cancel / stop' },
    { id: 'automation-panel', label: 'Automation panel', selector: '#auto-panel', prefer: ['#auto-panel-envelope'],
      keys: '← →: row · ↑ ↓: value · Enter: add point · ⌫: remove · ⌥⇧A / Esc: grid' },
    { id: 'step-inspector', label: 'Step editor', selector: '.step-editor', prefer: ['#step-note'], keys: 'Enter: commit and back to the grid', esc: 'Esc in a field: grid' }
  ], { altArrows: true, escape: true, owns: gridOwns });
  /** ⌘0: the pattern grid at its cursor cell, from any page (switches to Pattern) or block. */
  function focusGrid() {
    if (S().state.modal) return false;
    if (S().state.view !== 'pattern') S().setView?.('pattern');
    if (S().state.view !== 'pattern' || !grid()) return false;
    return nav.focusBlock('pattern-grid');
  }

  // ---- Instrument (all sampler pages, plug-in and MIDI instruments) ----
  const SLIDER_KEYS = '↑ ↓: adjust (⇧ coarse, ⌥ fine while adjusting) · ← →: next control · Enter: type a value';
  const KNOB_KEYS = '← →: knob · ↑ ↓: adjust (⇧ coarse, ⌥ fine) · ⇧← →: page · A: automate';
  // An insert chain (instrument effects, mixer inserts): the module strip and the selected module's knob page.
  const chain = (scope, id, label, keys) => [
    { id, label, roots: () => Array.from(document.querySelectorAll?.(`${scope} .fx-chain-head, ${scope} .fx-tiles, ${scope} .fx-add-panel`) || []),
      prefer: ['.fx-tile.is-selected', '.fx-add-head input', '[data-fx-tile][tabindex="0"]', '.fx-add'],
      keys: keys || '← →: module · ⌥← →: reorder (once used) · ↓ / Enter: knobs · B: bypass · D: duplicate · ⌫: remove' },
    { id: id + '-module', label: 'Selected ' + (label === 'Insert effects' ? 'insert' : 'effect'), selector: `${scope} .fx-module`, prefer: ['[data-fx-dial]', '[data-fx-page].is-on'],
      keys: '← →: knob · ↑ ↓: adjust (⇧ fine) · ⇧← →: page · ⌫: default · A: automate', esc: 'Esc: modules' }
  ];
  const instrumentBlocks = [
    { id: 'wavetable', label: 'Wavetable', selector: '#inst-group-wavetable', prefer: ['#param-wavetablePosition'], keys: SLIDER_KEYS },
    { id: 'granular', label: 'Granular', selector: '#inst-group-granular', prefer: ['#param-grainPosition'], keys: SLIDER_KEYS },
    { id: 'sample-file', label: 'Sample', selector: '#sampler-editor > .panel-head', prefer: ['#sampler-editor-audition:not(:disabled)'], keys: 'Space / Enter (hold): audition' },
    { id: 'play-mode', label: 'Play mode', selector: '#sampler-editor .sampler-modes,#sampler-editor .sampler-mode-params', prefer: ['[data-sampler-mode][tabindex="0"]'],
      keys: '← →: play mode' },
    { id: 'waveform', label: 'Waveform', selector: '#sampler-editor .sampler-editor-strip,#sampler-editor .sampler-wave-wrap', prefer: ['#sampler-wave-canvas'],
      keys: '← →: nudge marker (⇧ coarse · ⌥ one frame once used) · ↑ ↓: marker · + − 0 F: zoom · Space: audition · T I O N R G: edits · A: slice' },
    { id: 'sample-edits', label: 'Region edits & view', selector: '#sampler-editor .sampler-editor-row,#sampler-editor .sampler-editor-foot', keys: 'arrows: next button · Enter: apply' },
    { id: 'level', label: 'Volume · Pan · Tune', selector: '#inst-group-level', keys: SLIDER_KEYS },
    { id: 'filter', label: 'Filter · Drive', selector: '#inst-group-filter', prefer: ['#param-cutoff'], keys: SLIDER_KEYS },
    { id: 'amp', label: 'Amp envelope', selector: '#inst-group-amp', keys: SLIDER_KEYS },
    { id: 'region', label: 'Region · Root note', selector: '#inst-group-playback', keys: SLIDER_KEYS },
    { id: 'slice-tools', label: 'Slice detection', selector: '#sampler-slice-map .sampler-slice-tools', prefer: ['#sampler-detect-transients:not(:disabled)', '#sampler-equal-count'] },
    { id: 'slice-list', label: 'Slices', selector: '#sampler-slice-map .sampler-slice-list', prefer: ['[data-sampler-audition][tabindex="0"]:not(:disabled)'], keys: '↑ ↓: slice · Space: audition' },
    { id: 'zones', label: 'Zones', selector: '.sampler-zones', prefer: ['[data-sampler-zone-field]', '#sampler-add-zone'] },
    { id: 'mod-envelope', label: 'Mod envelope (filter)', selector: '.modulation-console .mod-filter-section', prefer: ['input[type="range"]'], keys: SLIDER_KEYS },
    { id: 'lfo-1', label: 'LFO 1', selector: '.modulation-console [data-mod-lfo="lfo"]', prefer: ['select'], keys: SLIDER_KEYS },
    { id: 'lfo-2', label: 'LFO 2', selector: '.modulation-console [data-mod-lfo="lfo2"]', prefer: ['select'], keys: SLIDER_KEYS },
    { id: 'voices', label: 'Voices', selector: '.sampler-voices', prefer: ['#sampler-voice-mode'] },
    { id: 'record', label: 'Record into this sampler', selector: '.sampler-capture' },
    { id: 'plugin', label: 'Plug-in instrument', selector: '.instrument-plugin-overview', prefer: ['#plugin-editor', '#browse-plugins'] },
    // Plug-in console: ← → on the slot tabs switch plug-in, so its knobs keep ⌥ ← → for moving between blocks.
    // The page menu sits above the slot tabs: one block, so the knobs below stay a plain rectangle.
    { id: 'plugin-slots', label: 'Plug-in slots', selector: '[data-pp-root] > .panel-head,[data-pp-root] .pp-tabs', prefer: ['[data-pp-tab][tabindex="0"]'],
      keys: '← →: plug-in · ↓: knobs · page menu: control page' },
    { id: 'plugin-controls', label: 'Plug-in controls', selector: '[data-pp-root] > .panel-body',
      prefer: ['[data-pp-knob]:not(:disabled)'], keys: KNOB_KEYS + ' · Enter: assign · /: find' },
    { id: 'nks-controls', label: 'NKS controls', selector: '[data-nks-root]', prefer: ['[data-nks-address][type="range"]:not(:disabled)', '[data-nks-address][type="number"]'], keys: KNOB_KEYS },
    ...chain('#instrument-effects', 'instrument-effects', 'Instrument effects'),
    { id: 'perform', label: 'Play & perform', selector: '.inst-perform', prefer: ['#instrument-octave'], keys: 'Z–M, Q–U: play (⌘P) · [ ]: octave' }
  ];
  nav.register('instrument', () => {
    const blocks = [{ id: 'view-actions', label: 'Instrument actions', selector: '.workspace-main > .view-title' },
      { id: 'instrument-type', label: 'Instrument type', selector: '.workspace-main > .toolbar .instrument-type', prefer: ['.instrument-type [tabindex="0"]'] },
      { id: 'instrument-pages', label: 'Instrument pages', selector: '.workspace-main > .toolbar .inst-pages', prefer: ['[role="tab"][tabindex="0"]'],
        keys: '← →: page · ↓: open · ⌥1–5 or ⌃Tab from anywhere' }];
    // The panels in reading order: a panel split into named blocks is replaced by them; any other panel is one block.
    const panels = Array.from(document.querySelectorAll?.('.instrument-layout > .panel, .instrument-layout > .inst-page > .panel') || []);
    const named = instrumentBlocks.map(spec => ({ spec, roots: spec.roots ? spec.roots() : Array.from(document.querySelectorAll?.(spec.selector) || []) }));
    const placed = new Set();
    panels.forEach((panel, index) => {
      const inside = named.filter(entry => !placed.has(entry.spec) && entry.roots.some(root => panel === root || panel.contains?.(root)));
      inside.forEach(entry => { placed.add(entry.spec); blocks.push(entry.spec); });
      if (inside.length) return;
      const label = panel.getAttribute?.('aria-label') || panel.querySelector('h2,h3')?.textContent?.trim() || `Instrument section ${index + 1}`;
      blocks.push({ id: 'instrument-' + index, label, roots: () => [panel],
        prefer: ['[data-nks-address][type="number"]', '[data-sampler-audition][tabindex="0"]', '#inst-waveform,#sampler-wave-canvas,#inst-midiInput,#browse-plugins',
          'input[type="number"],select', '[data-piano][tabindex="0"]', controls] });
    });
    return blocks;
  }, { altArrows: true, escape: true, sliders: true });

  // ---- Mixer ----
  nav.register('mixer', () => [
    { id: 'mixer-actions', label: 'Mixer actions', selector: '#mixer-workspace > .panel-head' },
    { id: 'channels', label: 'Mixer channels', selector: '#mixer-channels', prefer: ['.studio-channel.is-selected .studio-channel-title', '.studio-channel-title[tabindex="0"]'],
      keys: '← →: channel (same control) · ↑ ↓: fader / knob · Tab: next control · Home / End: first / last' },
    { id: 'splitter', label: 'Channel / detail divider', selector: '#mixer-splitter', keys: '↑ ↓: resize (⇧ larger steps) · Home: default size' },
    ...(document.querySelector?.('.studio-rack .fx-chain')
      ? chain('.studio-rack', 'inserts', 'Insert effects')
      : [{ id: 'inserts', label: 'Insert effects', selector: '.studio-rack', prefer: ['#studio-plugin-search', 'summary[id^="plugin-header-"]', '[data-studio-action="add-plugin"]'] }]),
    { id: 'plugin-picker', label: 'Search plug-ins', selector: '.studio-rack .studio-plugin-picker', prefer: ['#studio-plugin-search'], keys: 'type to search · ↓: results · Enter: add' },
    { id: 'insert-details', label: 'Plug-in details', selector: '.studio-rack .studio-rack-details', keys: '→ ←: open / close · ↑ ↓: next insert · ⌥↑↓: reorder (once used)' },
    { id: 'routing', label: 'Channel routing', selector: '.studio-routing-panel' },
    { id: 'returns', label: 'Send returns', selector: '.studio-returns', prefer: ['#delay-time'], keys: '↑ ↓ or ← →: adjust · Tab: next control' }
  ], audited);

  const sections = () => S().state.modal ? [] : nav.blocks();
  window.STPaneNavigation = { key: nav.key, next: (direction, from) => !S().state.modal && nav.next(direction, from), sections, focusGrid };
})();
