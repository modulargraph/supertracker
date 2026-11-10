/* Block navigation: pages and dialogs declare blocks (regions of controls). ⌥⌘ + arrows move between blocks by their
   on-screen position everywhere; plain ⌥ + arrows do the same on pages that opt in, unless the focused control uses
   ⌥ + that arrow itself. F6 / ⇧F6 cycle blocks in reading order, Esc backs out of a block, Enter goes back in.
   The focused block gets a frame and a hint line. API and planned page layouts: docs/KEYBOARD_NAVIGATION.md. */
(() => {
  'use strict';
  const S = () => window.ST;
  const CONTROLS = 'button,input,select,textarea,summary,a[href],[tabindex],[contenteditable="true"]';
  const TEXT_FIELDS = 'input:not([type]),input[type=text],input[type=search],input[type=url],input[type=email],'
      + 'input[type=password],textarea,[contenteditable="true"]';
  const VALUES = '.parameter,.mod-parameter,[data-nks-address],[data-pp-knob],[data-fx-dial],[data-plugin-address]';
  const ADJUST_KEYS = ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'pointer'];
  const DIRECTIONS = { ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down' };
  const OPPOSITE = { left: 'right', right: 'left', up: 'down', down: 'up' };

  function visible(node) {
    if (!node || node.isConnected === false || node.hidden || node.type === 'hidden'
        || node.closest?.('[hidden],[inert],[aria-hidden="true"]')) return false;
    if (typeof node.getClientRects === 'function' && !node.getClientRects().length) return false;
    return true;
  }
  const focusable = node => visible(node) && !node.disabled && node.getAttribute?.('aria-disabled') !== 'true'
      && Number(node.tabIndex ?? 0) >= 0;
  const isControl = node => !!node?.matches?.(CONTROLS) && !node.hasAttribute?.('data-nav-root');

  /* ---------- registry ---------- */

  // scope ('*' for every page, a view name, or 'modal:<kind>') → { blocks, options }.
  const registry = new Map();
  // Owners of ⌥ + arrows: [selector, 'vertical' | 'horizontal' | 'both', descendants that do not own them, condition].
  const owners = [
    ['textarea,[contenteditable="true"]', 'both'], // caret by word / paragraph
    [TEXT_FIELDS, 'horizontal', '', node => !!node.value], // caret by word, once there is text to move through
    ['#browser-search', 'vertical'], // walk recent searches
    ['#sequence-list', 'vertical', 'button,select,input'], // move song blocks
    ['#clips-timeline,[data-clip-id]', 'both'], // slip audio / move a clip to another track
    ['#automation-envelope,#automation-song-envelope', 'both'], // previous / next point, move point
    // The Pattern page does not enable plain ⌥ + arrows yet but still declares its owner (see the doc).
    // The Pattern page's own `owns` option makes the grid keep all four ⌥ arrows unconditionally (pane-navigation.js);
    // this entry remains for grids elsewhere. Track header buttons (thead) never own them.
    ['#tracker,.tracker', 'vertical', 'thead'], // transpose
    ['[data-fx-tile]', 'horizontal'], // reorder insert effects (the tile strip, not the knobs below it)
    ['.studio-plugin > summary', 'vertical'], // move an insert (older mixer rack)
    ['[data-pp-tab]', 'horizontal'], // switch plug-in (the slot tabs only; ⌥ ← → on knobs move between blocks)
    // Fine adjustment (⌥ ↑ ↓) of sliders, knobs and value fields: owned only while you are adjusting the control,
    // i.e. the last key pressed in it was an adjustment or you clicked / dragged it. ⌥ ← → are never theirs.
    [VALUES, 'vertical', '', node => adjusting(node)],
    ['#sampler-wave-canvas,.sampler-wave', 'horizontal'] // nudge a marker by one frame
  ];
  const DEFAULTS = { altArrows: false, escape: false, f6: true, sliders: false, owns: null };

  /** Declare the blocks of a page ('pattern'), of every page ('*') or of a dialog ('modal:settings').
      blocks: an array (or a function returning one) of { id, label, selector | roots(), prefer: [selector…],
      keys: 'hint text', esc: 'Esc hint' when the block's controls use Esc themselves, arrows: false to leave plain arrows on its buttons alone,
      stops: selector (arrows, Tab, Home / End and Return on these controls are the block's own: roving focus, tabindex -1 included),
      rect() (geometry instead of the roots' box, e.g. the visible part of a scrolled grid), anchor() / reveal() (the node
      to measure moves from / to scroll into view on arrival, e.g. the cursor cell), ownKeys: true (the block keeps Tab
      and ⌥ + arrows; the hint says ⌥⌘) }. options: { altArrows, escape, f6, sliders, owns(target, key) — the page's
      controls that always keep ⌥ + arrows, even on arrival }. Returns a function that removes the registration. */
  function register(scope, blocks, options = {}) {
    registry.set(scope, { blocks, options: { ...DEFAULTS, ...options } });
    return () => registry.delete(scope);
  }
  /** Declare that a control owns ⌥ + arrows (axis 'vertical', 'horizontal' or 'both'), e.g. a reorderable list. */
  function claimAltArrows(selector, axis = 'both', except = '', when = null) { owners.push([selector, axis, except, when]); }

  function scope() {
    const state = S()?.state;
    if (!state) return null;
    return state.modal ? 'modal:' + state.modal : state.view;
  }
  function options(name = scope()) {
    if (!name) return { ...DEFAULTS, f6: false };
    const own = registry.get(name)?.options;
    if (own) return own;
    // Dialogs without their own registration get header / body / footer blocks and ⌥⌘ + arrows.
    return name.startsWith('modal:') ? { ...DEFAULTS, f6: false } : DEFAULTS;
  }
  function container(name) {
    return name?.startsWith('modal:') ? document.querySelector?.('.modal') : null;
  }
  const genericModal = [
    { id: 'modal-head', label: 'Dialog title', selector: '.modal-head' },
    { id: 'modal-body', label: 'Dialog', selector: '.modal-body,.modal > div:not(.modal-head):not(.modal-foot)' },
    { id: 'modal-foot', label: 'Dialog buttons', selector: '.modal-foot', prefer: ['button.primary', 'button'] }
  ];
  function specs(name) {
    const read = entry => (typeof entry?.blocks === 'function' ? entry.blocks() : entry?.blocks) || [];
    const own = registry.get(name);
    const list = name?.startsWith('modal:') ? (own ? read(own) : genericModal) : [...read(registry.get('*')), ...read(own)];
    // Markup blocks: any element with data-nav-block (label in data-nav-label, hint in data-nav-keys).
    const root = container(name) || document.getElementById?.('app');
    const marked = root?.querySelectorAll ? Array.from(root.querySelectorAll('[data-nav-block]')) : [];
    for (const node of marked) {
      const id = node.getAttribute('data-nav-block');
      if (list.some(spec => spec.id === id)) continue;
      list.push({ id, label: node.getAttribute('data-nav-label') || node.getAttribute('aria-label') || id,
        keys: node.getAttribute('data-nav-keys') || '', roots: () => [node],
        prefer: node.getAttribute('data-nav-entry') ? [node.getAttribute('data-nav-entry')] : [] });
    }
    return list.filter(Boolean);
  }
  function resolve(spec, within) {
    let roots = spec.roots ? (spec.roots() || []) : Array.from(document.querySelectorAll?.(spec.selector) || []);
    roots = roots.filter(root => root && visible(root) && (!within || within.contains(root)));
    if (!roots.length) return null;
    const candidates = selector => roots.flatMap(root => [...(root.matches?.(selector) ? [root] : []),
      ...Array.from(root.querySelectorAll(selector))]);
    let target;
    for (const selector of spec.prefer || []) {
      target = candidates(selector).find(focusable);
      if (target) break;
    }
    target ||= roots.find(root => focusable(root) && isControl(root));
    target ||= roots.flatMap(root => Array.from(root.querySelectorAll(CONTROLS))).find(focusable);
    return target ? { id: spec.id, key: spec.id, label: spec.label || spec.id, keys: spec.keys || '', esc: spec.esc || '', arrows: spec.arrows !== false,
      stops: spec.stops || null, ownKeys: !!spec.ownKeys, area: () => spec.rect?.() || rectOf(roots), anchor: spec.anchor || null, reveal: spec.reveal || null, roots, target } : null;
  }
  /** The blocks of the current page or dialog, in declaration (reading) order. */
  function blocks(name = scope()) {
    if (!name) return [];
    const within = container(name);
    if (name.startsWith('modal:') && !within) return [];
    const seen = new Set();
    return specs(name).map(spec => resolve(spec, within)).filter(block => block && !seen.has(block.id) && seen.add(block.id));
  }
  /** The block containing a node: the innermost one when blocks nest. */
  function blockOf(node, list = blocks()) {
    let best = null, bestRoot = null;
    for (const block of list) for (const root of block.roots) {
      if (root !== node && !root.contains?.(node)) continue;
      if (!bestRoot || bestRoot.contains?.(root)) { best = block; bestRoot = root; }
    }
    return best;
  }

  /* ---------- geometry ---------- */

  function rectOf(nodes) {
    let rect = null;
    for (const node of nodes) {
      const r = node?.getBoundingClientRect?.();
      if (!r || (!r.width && !r.height)) continue;
      rect = rect ? { left: Math.min(rect.left, r.left), top: Math.min(rect.top, r.top), right: Math.max(rect.right, r.right),
        bottom: Math.max(rect.bottom, r.bottom) } : { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
    }
    return rect;
  }
  /** Pure geometry: the candidate ({ id, rect }) to move to from rect `from` in a direction. Candidates that share the
      row (←→) or column (↑↓) win; the nearest edge wins, then the closest to `from.anchor` (the focused control).
      `prefer` (the block you came from) wins a near tie, so a move straight back returns. When nothing lies ahead,
      the move wraps to the far end of the same row / column; null when there is none. */
  function pick(from, candidates, direction, prefer = null) {
    const horizontal = direction === 'left' || direction === 'right', sign = direction === 'right' || direction === 'down' ? 1 : -1;
    const lo = r => horizontal ? r.left : r.top, hi = r => horizontal ? r.right : r.bottom;
    const olo = r => horizontal ? r.top : r.left, ohi = r => horizontal ? r.bottom : r.right;
    const mid = r => (lo(r) + hi(r)) / 2, omid = r => (olo(r) + ohi(r)) / 2;
    const anchor = from.anchor ? (horizontal ? from.anchor.y : from.anchor.x) : omid(from);
    const overlap = r => Math.min(ohi(r), ohi(from)) - Math.max(olo(r), olo(from));
    // Ahead: both the near edge and the centre lie further in the direction (a wider block above is not "to the right").
    const ahead = r => sign > 0 ? lo(r) > lo(from) + 4 && mid(r) > mid(from) + 4 : hi(r) < hi(from) - 4 && mid(r) < mid(from) - 4;
    const gap = r => Math.max(0, sign > 0 ? lo(r) - hi(from) : lo(from) - hi(r));
    const reach = r => anchor < olo(r) ? olo(r) - anchor : anchor > ohi(r) ? anchor - ohi(r) : 0;
    const score = r => gap(r) + Math.max(0, -overlap(r)) * 2 + reach(r) * 0.5;
    const all = candidates.filter(c => c?.rect);
    let pool = all.filter(c => ahead(c.rect));
    const beam = pool.filter(c => overlap(c.rect) > 0);
    // Outside the row / column only blocks wholly beyond this one count (diagonal neighbours).
    pool = beam.length ? beam : pool.filter(c => sign > 0 ? lo(c.rect) >= hi(from) - 4 : hi(c.rect) <= lo(from) + 4);
    if (!pool.length) {
      const row = all.filter(c => overlap(c.rect) > 0 && Math.abs(mid(c.rect) - mid(from)) > 1);
      if (!row.length) return null;
      row.sort((a, b) => sign * (mid(a.rect) - mid(b.rect)) || reach(a.rect) - reach(b.rect));
      return { ...row[0], wrapped: true };
    }
    pool.sort((a, b) => score(a.rect) - score(b.rect));
    const back = prefer && pool.find(c => c.id === prefer);
    if (back && overlap(back.rect) > 0 && gap(back.rect) <= gap(pool[0].rect) + 24) return back;
    return pool[0];
  }
  function center(node) {
    const r = node?.getBoundingClientRect?.();
    return r && (r.width || r.height) ? { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 } : null;
  }

  /* ---------- state ---------- */

  const memory = new Map(); // scope → last focused block id
  // The control that keyboard navigation (a block move, Tab, a view switch) just focused: until it is used (a key
  // pressed in it, or a click), ⌥ + arrows keep moving between blocks even when it owns them.
  let arrival = null, used = null, pointer = false;
  let lastKey = null; // the last key pressed in `used` ('pointer' for a click or drag)
  let keyAt = 0; // when a control last handled a plain key
  // A value control is being adjusted: the last key in it (or in its value field) was an adjustment, or it was clicked.
  const adjusting = owner => !!used && (owner === used || owner.contains?.(used)) && ADJUST_KEYS.includes(lastKey);
  let back = null; // { id, from, direction }: the reverse of the last move
  let showing = false; // keyboard mode: frame and hint visible
  let level = null; // { scope, id } while a block itself (not a control in it) has focus

  function remember(node) {
    const name = scope();
    if (!name) return;
    const block = blockOf(node);
    if (block) memory.set(name, block.id);
  }
  /** The last focused block of a page (or dialog). */
  const remembered = (name = scope()) => memory.get(name) ?? null;
  function current(list = blocks()) {
    const active = document.activeElement;
    const focused = active && active !== document.body ? blockOf(active, list) : null;
    return focused || list.find(block => block.id === remembered()) || null;
  }

  function land(block, how) {
    const name = scope(), view = S().state.view, modal = S().state.modal;
    let target = block.target;
    target.focus({ preventScroll: true });
    // A numeric field's blur/change can replace the workspace synchronously.
    // Resolve the intended block again instead of focusing a detached node.
    if (S().state.modal !== modal || S().state.view !== view) return true;
    const again = blocks(name).find(entry => entry.id === block.id), fresh = again?.target;
    if (fresh && (fresh !== target || document.activeElement !== fresh)) { fresh.focus({ preventScroll: true }); target = fresh; }
    // A block may name what to bring into view (the grid: its cursor cell, not the whole table).
    const shown = (again || block).reveal?.() || target;
    if (shown?.isConnected !== false) shown.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
    memory.set(name, block.id);
    level = null;
    arrival = document.activeElement;
    show();
    return true;
  }
  /** F6 order: the next (1) or previous (-1) block. */
  function next(direction = 1, from = document.activeElement) {
    const name = scope();
    if (!name || !options(name).f6) return false;
    const list = blocks(name);
    if (!list.length) return false;
    const here = blockOf(from, list), index = list.indexOf(here);
    const step = direction < 0 ? -1 : 1;
    const target = index < 0 ? (step < 0 ? list.length - 1 : 0) : (index + step + list.length) % list.length;
    return land(list[target], 'cycle');
  }
  /** Spatial move: 'left' | 'right' | 'up' | 'down'. True when handled (also at an edge with nowhere to go). */
  function move(direction) {
    const name = scope(), list = blocks(name);
    if (!list.length) return false;
    const here = current(list);
    if (!here) return land(list[0], 'move');
    const rects = list.map(block => ({ id: block.id, rect: block.area() }));
    let destination;
    if (rects.some(entry => !entry.rect)) {
      // No layout (tests, hidden window): reading order.
      const index = list.indexOf(here), step = direction === 'left' || direction === 'up' ? -1 : 1;
      destination = list[(index + step + list.length) % list.length];
    } else {
      const from = { ...rects[list.indexOf(here)].rect };
      if (here.roots.some(root => root.contains?.(document.activeElement))) from.anchor = center(here.anchor?.() || document.activeElement);
      const prefer = back && back.from === here.id && back.direction === direction ? back.id : null;
      const choice = pick(from, rects.filter(entry => entry.id !== here.id), direction, prefer);
      destination = choice && list.find(block => block.id === choice.id);
    }
    if (!destination || destination === here) { flash(); return true; }
    back = { id: here.id, from: destination.id, direction: OPPOSITE[direction] };
    return land(destination, 'move');
  }
  /** Focus a block by id. */
  function focusBlock(id) {
    const block = blocks().find(entry => entry.id === id);
    return block ? land(block, 'move') : false;
  }
  /** Esc inside a block: focus the block itself (block level). False when already there. */
  function backOut() {
    const name = scope(), list = blocks(name), active = document.activeElement;
    const block = active && active !== document.body ? blockOf(active, list) : null;
    if (!block || block.roots.includes(active)) return false;
    const root = block.roots[0];
    if (!(Number(root.tabIndex ?? -1) >= 0) || !root.hasAttribute?.('tabindex')) {
      root.setAttribute?.('tabindex', '-1');
      root.setAttribute?.('data-nav-root', '');
    }
    root.focus?.({ preventScroll: true });
    if (document.activeElement !== root) return false;
    memory.set(name, block.id);
    level = { scope: name, id: block.id };
    arrival = null;
    show();
    return true;
  }
  /** Enter at block level: focus the block's first control again. */
  function enter() {
    const name = scope(), active = document.activeElement;
    if (!active?.hasAttribute?.('data-nav-root')) return false;
    const block = blocks(name).find(entry => entry.roots.includes(active));
    return block ? land(block, 'enter') : false;
  }
  /** Restore a page's remembered block after a view switch (see setView). */
  function restore(view, id) {
    if (!id || scope() !== view || !options(view).altArrows) return false;
    const list = blocks(view), block = list.find(entry => entry.id === id);
    if (!block || blockOf(document.activeElement, list) === block) return false;
    block.target.focus({ preventScroll: true });
    memory.set(view, id);
    return true;
  }

  /** True when the focused control uses ⌥ + this arrow itself. */
  function owns(target, key) {
    const axis = key === 'ArrowUp' || key === 'ArrowDown' ? 'vertical' : 'horizontal';
    const marked = target?.closest?.('[data-alt-arrows]');
    if (marked) {
      const value = marked.getAttribute('data-alt-arrows') || 'both';
      return value === 'both' || value === axis;
    }
    return owners.some(([selector, owned, except, when]) => {
      if (owned !== 'both' && owned !== axis) return false;
      const owner = target?.closest?.(selector);
      if (!owner || (when && !when(owner))) return false;
      return !except || target === owner || !target.closest(except) || !owner.contains(target.closest(except));
    });
  }

  /* ---------- keys ---------- */

  /** Capture phase, before any widget: ⌥ / ⌥⌘ + arrows and the keys of a focused block. */
  function capture(event) {
    if (event.defaultPrevented || event.isComposing) return false;
    const name = scope();
    const direction = DIRECTIONS[event.key];
    const alt = event.altKey && !event.ctrlKey && !event.shiftKey;
    if (direction && alt) {
      const active = document.activeElement;
      // A page's strict owners (the pattern grid) keep ⌥ + arrows even on arrival; ⌥⌘ + arrows always move.
      const strict = !event.metaKey && !!options(name).owns?.(event.target, event.key);
      const allowed = event.metaKey
          || (options(name).altArrows && !strict && (active === arrival || active?.hasAttribute?.('data-nav-root') || !owns(event.target, event.key)));
      if (!allowed || !move(direction)) return false;
      event.preventDefault(); event.stopPropagation?.();
      return true;
    }
    if (!['Alt', 'Meta', 'Shift', 'Control', 'Tab', 'F6'].includes(event.key)) {
      arrival = null; used = event.target; lastKey = event.key;
      // A control's own key that moves focus (← → between insert tiles, a re-render) hands its use on (see focusin).
      if (!event.metaKey && !event.ctrlKey) keyAt = Date.now();
    }
    const atBlock = document.activeElement?.hasAttribute?.('data-nav-root');
    if (atBlock && !event.metaKey && !event.ctrlKey && !event.altKey
        && (event.key === 'Enter' || (direction && !event.shiftKey)) && enter()) {
      event.preventDefault(); event.stopPropagation?.();
      return true;
    }
    if (stopKey(event)) { event.preventDefault(); event.stopPropagation?.(); return true; }
    if (event.key === 'Tab' && options(name).altArrows) show();
    if (options(name).sliders && sliderKey(event)) { event.preventDefault(); event.stopPropagation?.(); return true; }
    return false;
  }
  /* Blocks with `stops` (the pattern track headers): their buttons sit inside a widget that uses every plain key (the
     grid), so the block handles them itself. Arrows go to the nearest stop in that direction (none: stay), Tab / ⇧Tab
     and Home / End to the next / previous / first / last stop in reading order, Return presses the button. Stops may
     have tabindex -1, so only the selected track's buttons are in the Tab order of the page. */
  const STOP_KEYS = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'Tab', 'Enter'];
  function stopKey(event) {
    const target = event.target;
    if (!STOP_KEYS.includes(event.key) || event.altKey || event.metaKey || event.ctrlKey || event.isComposing
        || (event.shiftKey && event.key !== 'Tab') || !target?.matches?.('button,[role=button]')) return false;
    const block = blockOf(target);
    if (!block?.stops || !target.matches(block.stops)) return false;
    if (event.key === 'Enter') { target.click?.(); show(); return true; }
    const stops = block.roots.flatMap(root => Array.from(root.querySelectorAll(block.stops)))
      .filter(node => node === target || (visible(node) && !node.disabled));
    let next = null;
    if (event.key === 'Home' || event.key === 'End') next = stops[event.key === 'Home' ? 0 : stops.length - 1];
    else if (event.key === 'Tab') next = stops[stops.indexOf(target) + (event.shiftKey ? -1 : 1)];
    else {
      const from = target.getBoundingClientRect?.(), others = stops.filter(node => node !== target);
      const choice = from && pick({ left: from.left, top: from.top, right: from.right, bottom: from.bottom },
          others.map((node, id) => ({ id, rect: rectOf([node]) })), DIRECTIONS[event.key]);
      next = choice && !choice.wrapped ? others[choice.id] : null;
    }
    if (next && next !== target) { next.focus({ preventScroll: true }); next.scrollIntoView?.({ block: 'nearest', inline: 'nearest' }); }
    show();
    return true; // at the ends: stay (the keys never reach the widget around the block)
  }

  /* Pages with { sliders: true } (Instrument): ← → on a slider go to the previous / next control of the block in
     reading order (value fields are skipped: they are for typing), ↑ ↓ stay the slider's own (adjust). Return on a
     slider types a value: its value field is focused and selected; Return there commits and goes back to the slider. */
  const SLIDERS = 'input[type=range][data-inst-param],input[type=range][data-mod-param]';
  const STOPS = 'input[type=range],input[type=checkbox],input[type=radio],select,button,summary,canvas[tabindex],[role=slider],[role=tab],[role=radio],[role=option],[tabindex="0"]';
  const pair = (node, type) => node.closest?.('.parameter,.mod-parameter')?.querySelector?.(`input[type=${type}]`) || null;
  function sliderKey(event) {
    const target = event.target;
    if (event.altKey || event.metaKey || event.ctrlKey || event.isComposing || !target?.matches) return false;
    if (event.key === 'Enter' && !event.shiftKey && target.matches('input[type=number]') && target.closest('.parameter,.mod-parameter')) {
      const slider = pair(target, 'range');
      if (!slider || !focusable(slider)) return false;
      const id = slider.id;
      slider.focus({ preventScroll: true }); // blur commits the typed value (change)
      if (document.activeElement !== slider && id) document.getElementById?.(id)?.focus?.({ preventScroll: true });
      return true;
    }
    // Menus have no use for ← →: they step through the block as on a slider.
    if (target.matches('select') && (event.key === 'ArrowLeft' || event.key === 'ArrowRight') && !event.shiftKey) return step(target, event.key);
    if (!target.matches(SLIDERS)) return false;
    if (event.key === 'Enter' && !event.shiftKey) {
      const field = pair(target, 'number');
      if (!field || !focusable(field)) return false;
      field.focus({ preventScroll: true });
      field.select?.();
      return true;
    }
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return false;
    return step(target, event.key);
  }
  function step(target, key) {
    const block = blockOf(target);
    if (!block) return false;
    const stops = block.roots.flatMap(root => Array.from(root.querySelectorAll(STOPS)))
      .filter(node => node === target || (focusable(node) && !node.matches('[data-piano]')));
    const next = stops[stops.indexOf(target) + (key === 'ArrowRight' ? 1 : -1)];
    if (next) { next.focus(); next.scrollIntoView?.({ block: 'nearest', inline: 'nearest' }); show(); }
    return true; // at the ends: stay (the slider never moves sideways)
  }
  /** F6 / ⇧F6 (called from the app's key handler). */
  function key(event) {
    if (event.defaultPrevented || event.isComposing || event.key !== 'F6' || event.metaKey || event.ctrlKey || event.altKey
        || S().state.modal) return false;
    if (!next(event.shiftKey ? -1 : 1, event.target)) return false;
    event.preventDefault(); return true;
  }
  /** Esc after the page had its turn: back out of a control to its block. While playing, Esc still stops at once. */
  function escape(event) {
    if (event.defaultPrevented || S().state.modal || !options().escape) return false;
    if (!backOut()) return false;
    if (S().state.playing) return false;
    event.preventDefault(); return true;
  }
  /** Plain arrows on a button-like control nobody else used: the nearest control in that direction inside the block. */
  function arrow(event) {
    const direction = DIRECTIONS[event.key];
    if (!direction || event.defaultPrevented || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey
        || !options().altArrows) return false;
    const target = event.target;
    if (!target?.matches?.('button,summary,a[href],input[type=checkbox],input[type=radio],[role=tab],[role=button]')) return false;
    const block = blockOf(target);
    if (!block?.arrows) return false;
    const controls = block.roots.flatMap(root => Array.from(root.querySelectorAll(CONTROLS))).filter(node => node !== target && focusable(node));
    const from = target.getBoundingClientRect?.();
    if (!from || !controls.length) return false;
    const choice = pick({ left: from.left, top: from.top, right: from.right, bottom: from.bottom },
        controls.map((node, id) => ({ id, rect: rectOf([node]) })), direction);
    if (!choice || choice.wrapped) return false;
    event.preventDefault();
    controls[choice.id].focus();
    controls[choice.id].scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
    return true;
  }

  /* ---------- frame and hint ---------- */

  let frame = null, hint = null, pending = false, timer = null;
  const later = callback => (window.requestAnimationFrame || (fn => setTimeout(fn, 16)))(callback);
  // While keyboard navigation shows the frame, the focused control is marked .block-nav-focus and gets a ring
  // (block-nav.css), also after a programmatic focus that WebKit would not mark :focus-visible.
  let marked = null; // the focused control, marked .block-nav-focus while the frame shows
  function mark(node) {
    if (marked === node) return;
    marked?.classList?.remove('block-nav-focus');
    marked = node || null;
    marked?.classList?.add('block-nav-focus');
  }
  function show() { showing = true; document.documentElement?.classList?.add('block-nav-on'); paint(); tick(); }
  function hide() {
    showing = false; level = null; mark(null);
    document.documentElement?.classList?.remove('block-nav-on');
    if (timer !== null && typeof clearInterval === 'function') clearInterval(timer);
    timer = null;
    if (frame) frame.hidden = true;
    if (hint) hint.hidden = true;
  }
  function schedule() {
    if (pending) return;
    pending = true;
    later(() => { pending = false; paint(); });
  }
  function flash() {
    show();
    if (!frame) return;
    frame.classList.remove('is-edge');
    void frame.offsetWidth;
    frame.classList.add('is-edge');
  }
  function element(className) {
    if (!document.createElement || !document.body?.appendChild) return null;
    const node = document.createElement('div');
    node.className = className;
    node.hidden = true;
    node.setAttribute('aria-hidden', 'true');
    document.body.appendChild(node);
    return node;
  }
  function hintText(block) {
    const atBlock = document.activeElement?.hasAttribute?.('data-nav-root');
    const alt = options().altArrows ? '⌥' : '⌥⌘';
    const esc = S().state.modal ? 'Esc: close' : block.esc || (options().escape ? 'Esc: back to block' : '');
    const parts = atBlock
        ? ['Enter or arrows: go in', `${alt} arrows: blocks`, 'Esc: stop']
        : block.ownKeys ? [block.keys, block.esc, '⌥⌘ arrows or F6: blocks']
        : [block.keys, 'Tab: next control', esc, `${alt} arrows: blocks`];
    return parts.filter(Boolean).join(' · ');
  }
  function paint() {
    if (!showing) return;
    const list = blocks(), block = current(list);
    const rect = block && block.area();
    frame ||= element('block-nav-frame');
    hint ||= element('block-nav-hint');
    if (!frame || !hint) return;
    const active = document.activeElement;
    mark(active && active !== document.body && !active.hasAttribute?.('data-nav-root') ? active : null);
    if (!block || !rect) { frame.hidden = true; hint.hidden = true; return; }
    const width = window.innerWidth || document.documentElement?.clientWidth || 0;
    const height = window.innerHeight || document.documentElement?.clientHeight || 0;
    // A little outside the block so the frame does not cover its edge content; clipped to the window.
    const pad = 3, left = Math.max(1, rect.left - pad), top = Math.max(1, rect.top - pad);
    const right = width ? Math.min(width - 1, rect.right + pad) : rect.right + pad, bottom = height ? Math.min(height - 1, rect.bottom + pad) : rect.bottom + pad;
    Object.assign(frame.style, { left: left + 'px', top: top + 'px', width: Math.max(0, right - left) + 'px',
      height: Math.max(0, bottom - top) + 'px' });
    frame.classList.toggle('is-level', !!document.activeElement?.hasAttribute?.('data-nav-root'));
    frame.classList.toggle('label-inside', top < 18);
    frame.dataset.label = block.label;
    frame.hidden = false;
    hint.innerHTML = '';
    const label = document.createElement('strong');
    label.textContent = block.label;
    const keys = document.createElement('span');
    keys.textContent = hintText(block);
    hint.append(label, keys);
    hint.hidden = false;
    hint.dataset.block = block.id;
  }
  // Pages re-render without notice: while the frame shows, follow its block a few times a second.
  function tick() {
    if (timer !== null || typeof setInterval !== 'function') return;
    timer = setInterval(() => { if (showing) paint(); }, 300);
    timer?.unref?.(); // node test contexts: never keep the process alive
  }

  function install() {
    if (typeof document === 'undefined') return;
    window.addEventListener?.('keydown', capture, true);
    document.addEventListener?.('focusin', event => {
      remember(event.target);
      // A page re-rendering and focusing the field you are typing in again is not an arrival.
      const again = used && (used === event.target || (used.id && used.id === event.target?.id));
      // Focus moved by the control's own key (a tile strip selecting the next tile and re-rendering): still in use.
      const own = !pointer && !again && Date.now() - keyAt < 250;
      if (own) { used = event.target; lastKey = null; }
      else if (!pointer && !again) arrival = event.target;
      if (showing) schedule();
    }, true);
    document.addEventListener?.('pointerdown', event => {
      remember(event.target); arrival = null; used = event.target; lastKey = 'pointer'; pointer = true; hide();
      setTimeout(() => { pointer = false; }, 0);
    }, true);
    window.addEventListener?.('resize', () => showing && schedule());
    document.addEventListener?.('scroll', () => showing && schedule(), true);
  }
  install();

  window.STBlockNav = {
    register, claimAltArrows, blocks, blockOf, current, remembered, restore, move, next, focusBlock, backOut, enter,
    owns, pick, capture, key, escape, arrow, options, scope, hide,
    get showing() { return showing; }, get level() { return level; }, get arrival() { return arrival; }
  };
})();
