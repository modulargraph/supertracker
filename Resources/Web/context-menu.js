/* Native context menus. Right-click, the Menu key or Shift+F10 open a real macOS menu
   whose items depend on what is under the pointer (or focused, from the keyboard). */
(() => {
  'use strict';
  const S = () => window.ST;
  const providers = [];
  let open = false;

  /** A provider receives {target, x, y, keyboard} and returns an array of items or null.
      Item: {title, action, key, enabled, checked, items} | {separator:true} | {header:true,title}. */
  function register(provider, priority = 0) { providers.push({ provider, priority }); providers.sort((a, b) => b.priority - a.priority); }

  function prepare(items, actions) {
    const result = [];
    for (const item of items || []) {
      if (!item) continue;
      if (item.separator) { if (result.length && !result[result.length - 1].separator) result.push({ separator: true }); continue; }
      if (item.header) { result.push({ header: true, title: String(item.title || '') }); continue; }
      const entry = { title: String(item.title || ''), enabled: item.enabled !== false };
      if (item.checked) entry.checked = true;
      if (item.key) entry.key = item.key;
      if (item.items) entry.items = prepare(item.items, actions);
      else if (item.native) entry.id = 'native:' + item.native;
      else { const id = 'a' + actions.size; actions.set(id, item.action); entry.id = id; }
      result.push(entry);
    }
    while (result.length && result[result.length - 1].separator) result.pop();
    return result;
  }

  function fallbackMenu(x, y, items, actions, restore) {
    document.querySelector('.context-menu')?.remove();
    const menu = document.createElement('div');
    menu.className = 'context-menu'; menu.setAttribute('role', 'menu');
    menu.style.left = `${Math.min(x, innerWidth - 240)}px`; menu.style.top = `${Math.min(y, innerHeight - 40)}px`;
    const flat = (list, depth = 0) => list.forEach(item => {
      if (item.separator) { menu.appendChild(document.createElement('hr')); return; }
      if (item.header || item.items) {
        const label = document.createElement('div'); label.className = 'context-menu-header'; label.textContent = item.title; menu.appendChild(label);
        if (item.items) flat(item.items, depth + 1); return;
      }
      const button = document.createElement('button'); button.setAttribute('role', 'menuitem');
      button.textContent = item.title; button.disabled = !item.enabled; button.style.paddingLeft = `${9 + depth * 12}px`;
      if (item.key) { const kbd = document.createElement('kbd'); kbd.textContent = item.key; button.appendChild(kbd); }
      button.onclick = () => { menu.remove(); restore?.focus?.({ preventScroll: true }); run(item.id, actions); };
      menu.appendChild(button);
    });
    flat(items);
    document.body.appendChild(menu);
    // Keep the whole menu inside the window (a long menu scrolls inside its own height).
    const box = menu.getBoundingClientRect?.();
    if (box?.width) {
      menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
      menu.style.top = `${Math.max(8, Math.min(y, innerHeight - box.height - 8))}px`;
    }
    menu.querySelector('button:not(:disabled)')?.focus();
    menu.onkeydown = event => {
      const buttons = [...menu.querySelectorAll('button:not(:disabled)')], index = buttons.indexOf(document.activeElement);
      if (event.key === 'Escape') { event.preventDefault(); menu.remove(); restore?.focus?.({ preventScroll: true }); }
      else if (event.key === 'ArrowDown'
          || event.key === 'ArrowUp') { event.preventDefault(); buttons[(index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length]?.focus(); }
    };
    setTimeout(() => document.addEventListener('click', () => menu.remove(), { once: true }), 0);
  }

  function run(id, actions) {
    if (!id) return;
    if (id.startsWith('native:')) { document.execCommand(id.slice(7)); return; }
    const action = actions.get(id);
    if (typeof action === 'function') {
      try { const value = action(); if (value?.catch) value.catch(error => S().toast(error.message || String(error), 'error')); }
      catch (error) { S().toast(error.message || String(error), 'error'); }
    }
  }

  async function show(x, y, items) {
    const actions = new Map(), prepared = prepare(items, actions);
    if (!prepared.length) return;
    const restore = document.activeElement;
    if (!S()?.state?.native) { fallbackMenu(x, y, prepared, actions, restore); return; }
    open = true;
    try {
      const result = await S().request('ui.contextMenu', { x, y, items: prepared });
      restore?.focus?.({ preventScroll: true });
      run(result?.id, actions);
    } catch (error) { S().toast(error.message || String(error), 'error'); }
    finally { open = false; }
  }

  function collect(context) {
    const sections = [];
    for (const { provider } of providers) {
      let items = null;
      try { items = provider(context); } catch (error) { console.error(error); }
      if (items?.length) {
        sections.push(items);
        if (items.exclusive) break;
      }
    }
    return sections.flatMap((items, index) => index ? [{ separator: true }, ...items] : items);
  }

  function openFor(target, x, y, keyboard) {
    if (open || document.querySelector('.modal-backdrop.busy')) return;
    const items = collect({ target, x, y, keyboard });
    if (items.length) void show(x, y, items);
  }

  document.addEventListener('contextmenu', event => {
    event.preventDefault(); event.stopImmediatePropagation();
    openFor(event.target, event.clientX, event.clientY, false);
  }, true);
  document.addEventListener('keydown', event => {
    if (!((event.key === 'F10' && event.shiftKey && !event.metaKey) || event.key === 'ContextMenu')) return;
    event.preventDefault(); event.stopImmediatePropagation();
    const target = document.activeElement && document.activeElement !== document.body
        ? document.activeElement
        : document.querySelector('#tracker td.selected') || document.body;
    let anchor = target;
    if (target.id === 'tracker') anchor = target.querySelector('td.selected') || target;
    const rect = anchor.getBoundingClientRect();
    openFor(anchor, Math.round(rect.left + Math.min(24, rect.width / 2)), Math.round(rect.top + Math.min(rect.height, 24)), true);
  }, true);

  // ---- Providers -------------------------------------------------------------------------
  const isText = element => element?.matches?.('input:not([type=range]):not([type=checkbox]):not([type=radio]):not([type=button]), textarea, [contenteditable=""], [contenteditable=true]');

  // Text fields: standard editing commands only.
  register(({ target }) => {
    const field = target.closest?.('input, textarea, [contenteditable]');
    if (!isText(field)) return null;
    field.focus({ preventScroll: true });
    const items = [
      { title: 'Cut', key: '⌘X', native: 'cut', enabled: !field.readOnly },
      { title: 'Copy', key: '⌘C', native: 'copy' },
      { title: 'Paste', key: '⌘V', native: 'paste', enabled: !field.readOnly },
      { separator: true },
      { title: 'Select All', key: '⌘A', native: 'selectAll' }
    ];
    items.exclusive = true;
    return items;
  }, 100);

  // Any parameter that can be automated.
  register(({ target }) => {
    const control = target.closest?.('[data-automate]');
    if (!control) return null;
    const name = control.dataset.name || control.getAttribute('aria-label') || 'parameter';
    return [
      { header: true, title: name },
      { title: 'Automate in lane…', key: '⌥A', action: () => {
        if (control.dataset.nksAddress !== undefined && window.STNKSControls?.automateControl) return window.STNKSControls.automateControl(control);
        return S().automate(control.dataset.owner ?? Number(control.dataset.track ?? S().state.track), control.dataset.automate,
            control.dataset.name, S().automateValue ? S().automateValue(control) : Number(control.value));
      } },
      control.matches('input[type=range]') && control.defaultValue !== undefined ? { title: 'Reset to default', action: () => {
        control.value = control.defaultValue;
        control.dispatchEvent(new Event('input', { bubbles: true }));
        control.dispatchEvent(new Event('change', { bubbles: true }));
      } } : null
    ];
  }, 90);

  // Pattern grid cells.
  register(({ target }) => {
    const cell = target.closest?.('#tracker td[data-cell]');
    if (!cell) return null;
    const app = S(), state = app.state;
    const [track, row] = cell.dataset.cell.split(':').map(Number);
    const bounds = app.selectedBounds?.();
    const inside = bounds && track >= bounds.t0 && track <= bounds.t1 && row >= bounds.r0 && row <= bounds.r1;
    if (!inside) {
      state.selection = null; state.track = track; state.row = row;
      const span = target.closest?.('[data-column]'), column = span?.dataset.column;
      if (column !== undefined) {
        state.column = Number(column);
        state.noteColumn = Number(span.dataset.noteColumn || 0);
      }
      state.instrument = state.project.tracks[track]?.instrument || 0;
      app.refresh();
    }
    const steps = app.pattern().steps[state.track];
    const hasNote = !!steps?.[state.row];
    const fx = state.column >= 3, tools = window.STPatternTools;
    return [
      { title: 'Cut', key: '⌘X', action: () => app.copySelection(true) },
      { title: 'Copy', key: '⌘C', action: () => app.copySelection(false) },
      { title: 'Paste', key: '⌘V', enabled: !!state.clipboard, action: () => app.pasteSelection() },
      { title: 'Clear', key: '⌫', action: () => app.clearSelection() },
      { title: 'Select all', key: '⌘A', action: () => app.perform('selectAll') },
      { separator: true },
      { title: 'Transpose', items: [
        { title: 'Up a semitone', key: '⌥↑', action: () => app.transpose(1) },
        { title: 'Down a semitone', key: '⌥↓', action: () => app.transpose(-1) },
        { title: 'Up an octave', key: '⌥⇧↑', action: () => app.transpose(12) },
        { title: 'Down an octave', key: '⌥⇧↓', action: () => app.transpose(-12) }
      ] },
      { title: 'Repeat phrase to pattern end', key: '⌘D', enabled: !!bounds, action: () => app.fillSelection() },
      window.STRenderSelection ? { title: 'Render selection to sample…', key: '⌥B', action: () => app.perform('renderSelection') } : null,
      ...(() => {
        // Pattern loop range for Pattern playback (⌥⇧L toggles).
        const loop = app.patternLoopFor?.(), range = app.selectedBounds?.();
        if (!app.loopSelection) return [];
        const pad = value => String(value).padStart(2, '0');
        const set = {
          title: range ? `Loop rows ${pad(range.r0)}–${pad(range.r1)}` : 'Loop this bar',
          key: loop ? undefined : '⌥⇧L',
          action: () => app.loopSelection(true)
        };
        return loop
            ? [{ title: `Stop looping rows ${pad(loop.start)}–${pad(loop.end)}`, key: '⌥⇧L', checked: true, action: () => app.loopSelection(false) },
                set]
            : [set];
      })(),
      // Same edit as the keys: every track of the selection (or collapsed group) shifts from the cursor row.
      { title: 'Insert row', key: '⇧↩', action: () => app.shiftPatternRows(true) },
      { title: 'Delete row', key: '⇧⌫', action: () => app.shiftPatternRows(false) },
      { separator: true },
      fx || hasNote
          ? { title: 'Choose step effect…', key: '↩', enabled: !!window.STStepFX, action: () => window.STStepFX?.show({ fx: fx ? state.column - 2 : 1 }) }
          : null,
      tools ? { title: 'Quick step effect', items: [
        ...tools.quickEffects.map(([title, type, value]) => ({ title, action: () => tools.applyEffect(type, value) })),
        { separator: true },
        { title: 'Clear step effects', key: '⌥X', action: () => tools.clearEffects() }
      ] } : null,
      hasNote ? { title: 'Audition step', action: () => app.auditionStep?.() } : null,
      tools ? { title: 'Pattern tools', items: [
        { title: fx ? `Interpolate FX ${state.column - 2} ramp` : 'Interpolate velocity', key: '⌥I', action: () => tools.interpolate() },
        { title: 'Humanize velocity', key: '⌥H', action: () => tools.humanize() },
        { title: 'Randomize velocity', key: '⌥⇧H', action: () => tools.randomizeVelocity() },
        { title: 'Velocity louder', key: '⌥.', action: () => tools.scaleVelocity(1.1) },
        { title: 'Velocity softer', key: '⌥,', action: () => tools.scaleVelocity(1 / 1.1) },
        { separator: true },
        { title: 'Reverse steps', key: '⌥R', action: () => tools.reverse() },
        { title: 'Nudge up', key: '⌘↑', action: () => tools.nudge(-1) },
        { title: 'Nudge down', key: '⌘↓', action: () => tools.nudge(1) },
        { title: 'Stretch ×2', key: '⌥E', action: () => tools.stretch(2) },
        { title: 'Compress ½', key: '⌥⇧E', action: () => tools.stretch(.5) },
        { separator: true },
        { title: 'Double pattern length', key: '⌥D', action: () => tools.resizePattern(2) },
        { title: 'Halve pattern length', key: '⌥⇧D', action: () => tools.resizePattern(.5) }
      ] } : null,
      { separator: true },
      tools ? { title: 'Automate parameter…', key: '⌥P', action: () => tools.automatePicker(state.track) } : null
    ];
  }, 80);

  // Pattern track headers.
  register(({ target }) => {
    const header = target.closest?.('th[data-track]');
    if (!header) return null;
    const app = S(), state = app.state, index = Number(header.dataset.track), track = state.project.tracks[index];
    if (!track) return null;
    const L = window.STApp || {}, lanes = L.laneSelection?.() || [index], inSelection = lanes.length > 1 && lanes.includes(index);
    if (state.track !== index && !inSelection) { state.track = index; state.instrument = track.instrument || 0; app.refresh(); }
    return [
      { header: true, title: track.name || `Track ${index + 1}` },
      window.STPatternTools
          ? { title: 'Mute', key: '⌥M', checked: !!track.mute, action: () => window.STPatternTools.toggleTrack('mute', index) }
          : null,
      window.STPatternTools
          ? { title: 'Solo', key: '⌥S', checked: !!track.solo, action: () => window.STPatternTools.toggleTrack('solo', index) }
          : null,
      window.STPatternTools && state.project.tracks.some(t => t.mute || t.solo)
          ? { title: 'Unmute all, clear solo', key: '⌥⇧M', action: () => window.STPatternTools.clearMuteSolo() }
          : null,
      window.STPatternTools ? { title: 'Automate parameter…', key: '⌥P', action: () => window.STPatternTools.automatePicker(index) } : null,
      { separator: true },
      { title: 'Rename…', action: () => app.textPrompt('Track name', track.name || '',
          name => app.mutate('rename track', p => { if (p.tracks.includes(track)) track.name = name; })) },
      { title: 'Duplicate track', key: '⌘⇧T', enabled: !window.STBusy.isBusy('trackDuplicate')
          && state.project.tracks.length < (window.TRACK_SANITY_LIMIT ?? Infinity), action: () => window.STTrackDuplicate?.duplicate(index) },
      { title: 'Add track', key: '⌘T', action: () => app.perform('addTrack') },
      { title: 'Move left', key: '⌘⇧←', enabled: index > 0, action: () => app.perform('moveTrackLeft') },
      { title: 'Move right', key: '⌘⇧→', enabled: index < state.project.tracks.length - 1, action: () => app.perform('moveTrackRight') },
      window.STGroups?.ofTrack?.(state.project, index)
          ? { title: 'Take out of group', key: '⌘⇧G', action: () => app.perform('ungroupTrack') }
          : { title: inSelection ? `Group ${lanes.length} tracks` : 'Group track', key: '⌘G', action: () => app.perform('groupTracks') },
      L.deleteGroup && header.dataset.groupOverview !== undefined
          ? { title: 'Delete group…', action: () => L.deleteGroup(header.dataset.groupOverview) }
          : L.deleteTracks
          ? { title: inSelection ? `Delete ${lanes.length} tracks…` : 'Delete track…', key: '⌘⌫', enabled: state.project.tracks.length > (inSelection ? lanes.length : 1),
            action: () => L.deleteTracks(inSelection ? lanes : [index]) }
          : { title: 'Remove track', enabled: state.project.tracks.length > 1, action: () => app.perform('removeTrack') },
      L.toggleTrackCollapsed ? { title: 'Collapse track', key: '⌥⇧J', checked: !!L.isCollapsed?.(index), action: () => L.toggleTrackCollapsed(index) } : null,
      // Note columns (app-note-columns.js): more notes per row on this track.
      L.addNoteColumn && header.dataset.groupOverview === undefined ? { title: 'Add note column', enabled: L.noteColumnCount(index) < L.MAX_NOTE_COLUMNS, action: () => L.addNoteColumn(index) } : null,
      L.removeNoteColumn && header.dataset.groupOverview === undefined ? { title: 'Remove note column…', enabled: L.noteColumnCount(index) > 1, action: () => void L.removeNoteColumn(index) } : null,
      L.setHideEmpty ? { title: 'Hide empty tracks', key: '⌥U', checked: !!L.hideEmptyOn?.(), action: () => app.perform('hideEmptyTracks') } : null,
      { separator: true },
      { title: 'Route channel…', action: () => app.perform('routing') },
      { title: 'Bounce to audio…', action: () => app.perform('bounceTrack') }
    ];
  }, 80);

  // Context contributed by other modules (browser rows, mixer strips, clips...).
  register(context => {
    for (const module of [window.STBrowser, window.STStudio, window.STClips, window.STNKSControls, window.STTrackLanes]) {
      const items = module?.contextItems?.(context);
      if (items?.length) return items;
    }
    return null;
  }, 70);

  // Application-wide commands, always last.
  register(() => {
    const app = S();
    return [
      { title: 'Undo', key: '⌘Z', action: () => app.perform('undo') },
      { title: 'Redo', key: '⇧⌘Z', action: () => app.perform('redo') },
      { separator: true },
      { title: 'Go to', items: (app.views || []).map(([id, name], index) => ({ title: name, key: '⌘' + (index + 1), checked: app.state.view === id, action: () => app.setView(id) })) },
      { title: 'Command palette…', key: '⌘K', action: () => app.openPalette() },
      { title: 'Keyboard shortcuts', key: '?', action: () => app.showHelp() }
    ];
  }, -100);

  window.STContextMenu = { register, show, get isOpen() { return open; } };
})();
