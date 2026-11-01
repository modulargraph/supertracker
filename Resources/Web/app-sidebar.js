/* Sidebar: the pattern and instrument lists beside the Song and Instrument views, and the Pattern view's left pane. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, syncEngine, refresh } = app;

  const patternItems = p => p.patterns.map((x, i) => `<button class="sidebar-item ${x.id === p.activePattern ? 'active' : ''}" id="sidebar-pattern-${x.id}"`
      + ` data-pattern="${x.id}" draggable="true" title="Drag or ⌥↑↓ to reorder · right-click for more" tabindex="${x.id === p.activePattern ? 0 : -1}"><span class="sidebar-number">${String(i + 1).padStart(2, '0')}</span>`
      + `<span class="grow">${esc(x.name)}<div class="sub">${x.length} steps · ${x.automation.length} automation</div>`
      + `</span></button>`).join('');

  const instrumentItems = p => p.instruments.map((x, i) => `<button class="sidebar-item ${i === state.instrument ? 'active' : ''}" id="sidebar-instrument-${i}"`
      + ` data-instrument="${i}" title="Right-click for more" tabindex="${i === state.instrument ? 0 : -1}"><span class="sidebar-number">${String(i).padStart(2, '0')}</span>`
      + `<span class="grow">${esc(x.name)}<div class="sub">${x.type === 'au'
          ? x.plugin?.format || 'Plug-in'
          : x.type === 'midi'
          ? 'External MIDI'
          : x.mode === 'wavetable'
          ? 'wavetable' + (window.STWavetables?.displayName(x.samplePath) ? ' · ' + esc(window.STWavetables.displayName(x.samplePath)) : '')
          : x.samplePath ? x.mode || 'sample' : 'empty sampler'}</div>`
      + `</span>${i === state.project.tracks[state.track]?.instrument ? '<span class="color-dot"></span>' : ''}</button>`).join('');

  const addButton = kind => `<button class="quiet" data-add="${kind}" title="Add ${kind === 'patterns' ? 'pattern' : 'instrument'}"`
      + ` style="padding:2px 6px">+</button>`;

  function sidebar(kind) {
    const p = state.project;
    return `<aside class="sidebar"><div class="sidebar-heading"><span class="eyebrow">${kind === 'patterns' ? 'Patterns' : 'Instruments'}</span>`
        + `${addButton(kind)}</div>${kind === 'patterns' ? '' : '<button class="quiet sidebar-add-wavetable" data-add="wavetable"'
        + ' id="add-wavetable-instrument" title="Add a wavetable synth: a sampler in Wavetable mode on the Saw table">+ Wavetable</button>'}${kind === 'patterns' ? patternItems(p) : instrumentItems(p)}<div `
        + `class="sidebar-section"><div class="sidebar-heading"><span class="eyebrow">Session</span>`
        + `</div><div class="small-note">${p.tracks.length} tracks<br>${p.instruments.length} instruments<br>${p.patterns.length}`
        + ` patterns<br><br><kbd>⌘K</kbd> Find any action</div></div>${!state.native ? '<div class="small-note" style="color:var(--amber)">Interface preview<br>Audio, plug-ins and disk browsing '
        + 'run in the macOS app.</div>' : ''}</aside>`;
  }

  /* The Pattern view's left pane (like Renoise's instrument box over its pattern matrix): the instrument list on
     top, the pattern matrix (app-pattern-matrix.js) below. Each section scrolls on its own. */
  function leftPane() {
    const p = state.project;
    return `<aside class="sidebar left-pane"><section class="left-pane-instruments" aria-label="Instruments">`
        + `<div class="sidebar-heading"><span class="eyebrow">Instruments</span>${addButton('instruments')}</div>`
        + `<div class="left-pane-scroll" data-scroll="left-instruments">${instrumentItems(p)}</div></section>`
        + `<section class="left-pane-patterns" aria-label="Patterns">${app.patternMatrix()}</section></aside>`;
  }

  function selectPattern(id) {
    if (!state.project.patterns.some(x => x.id === Number(id))) return false;
    state.project.activePattern = Number(id);
    state.row = 0;
    state.selection = null;
    if (state.patternLoop?.pattern !== state.project.activePattern) app.clearPatternLoop();
    syncEngine();
    refresh();
    return true;
  }

  /* Reorders the pattern list (one undo step). The song refers to patterns by id, so only the list order changes. */
  function movePattern(id, to) {
    const patterns = state.project.patterns, from = patterns.findIndex(x => x.id === Number(id));
    const target = Math.max(0, Math.min(patterns.length - 1, to));
    if (from < 0 || target === from) return false;
    app.mutate('move pattern', p => { const [moved] = p.patterns.splice(from, 1); p.patterns.splice(target, 0, moved); });
    requestAnimationFrame?.(() => $(`#sidebar-pattern-${Number(id)}`)?.focus());
    return true;
  }
  const patternIndex = id => state.project.patterns.findIndex(x => x.id === Number(id));
  // Runs a pattern command on the right-clicked pattern: it becomes the active one first.
  const onPattern = (id, command) => () => { selectPattern(id); return app.perform(command); };

  function patternMenu({ target }) {
    const item = target.closest?.('[data-pattern]');
    if (!item) return null;
    const id = Number(item.dataset.pattern), index = patternIndex(id), count = state.project.patterns.length;
    const pat = state.project.patterns[index];
    if (!pat) return null;
    const items = [
      { header: true, title: pat.name },
      { title: 'Rename…', action: onPattern(id, 'renamePattern') },
      { title: 'Duplicate', action: onPattern(id, 'duplicatePattern') },
      { title: 'New pattern', key: '⌘⇧N', action: () => app.perform('addPattern') },
      { separator: true },
      { title: 'Move up', key: '⌥↑', enabled: index > 0, action: () => movePattern(id, index - 1) },
      { title: 'Move down', key: '⌥↓', enabled: index < count - 1, action: () => movePattern(id, index + 1) },
      { separator: true },
      { title: 'Delete pattern', enabled: count > 1, action: onPattern(id, 'removePattern') }
    ];
    items.exclusive = true;
    return items;
  }
  let menuRegistered = false;

  // Runs an instrument command on the right-clicked instrument: it becomes the current one first.
  const onInstrument = (index, command) => () => { selectInstrument(index); return app.perform(command); };
  const INSTRUMENT_TYPES = [['sample', 'Sampler'], ['au', 'Plug-in'], ['midi', 'External MIDI']];

  /* Right-click (or ⇧F10) on an instrument in the sidebar: the actions of the Instrument menu for that instrument. */
  function instrumentMenu({ target }) {
    const item = target.closest?.('[data-instrument]');
    if (!item) return null;
    const p = state.project, index = Number(item.dataset.instrument), inst = p.instruments[index];
    if (!inst) return null;
    const trackIndex = state.track, track = p.tracks[trackIndex], sampler = inst.type === 'sample';
    // Selecting an instrument may move the track cursor to a track that plays it; assigning keeps the selected track.
    const assign = () => {
      app.stopHeld();
      state.instrument = index;
      state.track = trackIndex;
      refresh();
      return app.perform('assignInstrument');
    };
    const items = [
      { header: true, title: `${String(index).padStart(2, '0')} · ${inst.name}` },
      state.view !== 'instrument'
          ? { title: 'Edit instrument', key: '⌘2', action: () => { selectInstrument(index); app.setView('instrument'); } }
          : null,
      { title: 'Rename…', action: onInstrument(index, 'renameInstrument') },
      track ? { title: `Assign to track “${track.name}”`, enabled: track.instrument !== index, checked: track.instrument === index,
        action: assign } : null,
      { separator: true },
      sampler ? { title: 'Load sample…', action: onInstrument(index, 'loadSample') } : null,
      sampler ? { title: 'Show sample in Finder', enabled: !!inst.samplePath, action: onInstrument(index, 'revealSample') } : null,
      { title: 'Find control…', key: '⌘F', action: onInstrument(index, 'findControl') },
      { title: 'Type', items: INSTRUMENT_TYPES.map(([type, label]) => ({ title: label, checked: inst.type === type,
        enabled: inst.type !== type, action: onInstrument(index, 'instrumentType:' + type) })) },
      { separator: true },
      { title: 'Add instrument', action: () => app.perform('addInstrument') },
      { title: 'Add wavetable synth', action: () => app.perform('addWavetable') },
      { separator: true },
      { title: 'Remove instrument', enabled: p.instruments.length > 1, action: onInstrument(index, 'removeInstrument') }
    ];
    items.exclusive = true;
    return items;
  }

  function selectInstrument(index) {
    if (!Number.isInteger(index) || !state.project.instruments[index]) return false;
    app.stopHeld();
    state.instrument = index;
    const current = state.project.tracks[state.track];
    if (current?.instrument !== index) {
      const track = state.project.tracks.findIndex(t => t.instrument === index);
      if (track >= 0) {
        state.track = track;
        state.selection = null;
      }
    }
    refresh();
    return true;
  }

  function bindSidebar() {
    for (const b of $$('[data-pattern]')) {
      b.onclick = () => selectPattern(b.dataset.pattern);
      b.onkeydown = event => {
        if (!event.altKey || (event.key !== 'ArrowUp' && event.key !== 'ArrowDown')) return;
        event.preventDefault(); event.stopPropagation();
        movePattern(b.dataset.pattern, patternIndex(b.dataset.pattern) + (event.key === 'ArrowUp' ? -1 : 1));
      };
      b.ondragstart = event => { event.dataTransfer?.setData('application/x-st-pattern', b.dataset.pattern); if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move'; };
      b.ondragover = event => { if ([...(event.dataTransfer?.types || [])].includes('application/x-st-pattern')) { event.preventDefault(); b.classList.add('drop-target'); } };
      b.ondragleave = () => b.classList.remove('drop-target');
      b.ondrop = event => {
        b.classList.remove('drop-target');
        const id = event.dataTransfer?.getData('application/x-st-pattern');
        if (id === undefined || id === '') return;
        event.preventDefault();
        movePattern(id, patternIndex(b.dataset.pattern));
      };
    }
    if (!menuRegistered && window.STContextMenu?.register) { menuRegistered = true; window.STContextMenu.register(patternMenu, 70); window.STContextMenu.register(instrumentMenu, 70); }
    for (const b of $$('[data-instrument]')) b.onclick = () => selectInstrument(Number(b.dataset.instrument));
    for (const add of $$('[data-add]')) add.onclick = () => add.dataset.add === 'patterns'
        ? app.addPattern()
        : add.dataset.add === 'wavetable' ? app.perform('addWavetable') : app.addInstrument();
  }

  Object.assign(app, { sidebar, leftPane, selectPattern, selectInstrument, bindSidebar, movePattern, patternMenu, instrumentMenu });
})();
