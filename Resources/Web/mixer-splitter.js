/* Mixer splitter: a draggable horizontal divider between the channel strips and the selected channel's detail panel
   (insert chain, module knobs, routing, returns). Drag it (or focus it and press ↑ / ↓, ⇧ for larger steps) to give
   either part more height; double-click (or Home) returns to the default layout. The detail height is a per-user
   interface preference (localStorage), never a project edit or Undo step. Heights are clamped so the strips keep
   their title, M/S buttons and a usable fader, and the detail keeps its chain tiles; the clamp follows window resizes. */
(() => {
  'use strict';
  const KEY = 'st.mixer.detailHeight';
  const MIN_STRIPS = 190, MIN_DETAIL = 132, STEP = 24, BIG_STEP = 96;
  let resizeBound = false;

  function load() {
    try { const value = Number(localStorage.getItem(KEY)); return Number.isFinite(value) && value > 0 ? value : null; } catch { return null; }
  }
  function save(value) {
    try { if (value === null) localStorage.removeItem(KEY); else localStorage.setItem(KEY, String(Math.round(value))); } catch { /* storage unavailable */ }
  }
  const parts = () => {
    const root = document.getElementById('mixer-workspace');
    return root ? { root, head: root.querySelector(':scope > .panel-head'), strips: root.querySelector(':scope > .studio-channels'),
      bottom: root.querySelector(':scope > .studio-bottom'), splitter: root.querySelector(':scope > .studio-splitter') } : null;
  };
  /** The allowed detail heights for the current window: [min, max]. */
  function limits(p = parts()) {
    if (!p?.bottom) return [MIN_DETAIL, MIN_DETAIL];
    const total = p.root.clientHeight, head = p.head?.offsetHeight || 0, bar = p.splitter?.offsetHeight || 0;
    const max = Math.max(MIN_DETAIL, total - head - bar - MIN_STRIPS);
    return [MIN_DETAIL, max];
  }
  const clampHeight = (value, p) => { const [min, max] = limits(p); return Math.round(Math.max(min, Math.min(max, value))); };
  /** Applies a detail height (null: the default layout). Returns the applied height. */
  function apply(value, p = parts()) {
    if (!p?.bottom) return null;
    if (value === null) {
      p.bottom.classList.remove('is-sized'); p.bottom.style.removeProperty('--mixer-detail-height');
      p.root.classList.remove('has-sized-detail');
    } else {
      value = clampHeight(value, p);
      p.bottom.classList.add('is-sized'); p.bottom.style.setProperty('--mixer-detail-height', `${value}px`);
      p.root.classList.add('has-sized-detail');
    }
    const [min, max] = limits(p), now = Math.round(p.bottom.getBoundingClientRect().height || value || 0);
    if (p.splitter) {
      p.splitter.setAttribute('aria-valuemin', String(min)); p.splitter.setAttribute('aria-valuemax', String(max));
      p.splitter.setAttribute('aria-valuenow', String(value ?? now));
      p.splitter.setAttribute('aria-valuetext', `Channel detail ${value ?? now} pixels high${value === null ? ' (default)' : ''}`);
    }
    return value;
  }
  const currentHeight = p => Math.round(p.bottom.getBoundingClientRect().height);
  function set(value) { const p = parts(); if (!p) return null; const applied = apply(value, p); save(applied); return applied; }
  function reset() { const p = parts(); save(null); apply(null, p); }

  /** Called after each mixer render: mounts the splitter above the detail panel and restores the saved height. */
  function bind() {
    const root = document.getElementById('mixer-workspace'), bottom = root?.querySelector(':scope > .studio-bottom');
    if (!root || !bottom) return;
    let splitter = root.querySelector(':scope > .studio-splitter');
    if (!splitter) {
      splitter = document.createElement('div');
      splitter.className = 'studio-splitter'; splitter.id = 'mixer-splitter';
      splitter.setAttribute('role', 'separator'); splitter.setAttribute('aria-orientation', 'horizontal'); splitter.tabIndex = 0;
      splitter.setAttribute('aria-label', 'Resize the channel detail panel. Up and down arrows resize, Home restores the default.');
      splitter.title = 'Drag to resize · double-click to reset';
      splitter.innerHTML = '<span class="studio-splitter-grip" aria-hidden="true"></span>';
      root.insertBefore(splitter, bottom);
    }
    apply(load());
    splitter.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      const p = parts(); if (!p) return;
      event.preventDefault();
      const startY = event.clientY, start = currentHeight(p);
      splitter.classList.add('is-dragging'); document.body.classList.add('is-resizing-mixer');
      try { splitter.setPointerCapture?.(event.pointerId); } catch { /* synthetic events */ }
      const move = e => apply(start - (e.clientY - startY), p);
      const up = e => {
        splitter.removeEventListener('pointermove', move); splitter.removeEventListener('pointerup', up); splitter.removeEventListener('pointercancel', up);
        splitter.classList.remove('is-dragging'); document.body.classList.remove('is-resizing-mixer');
        if (e.type === 'pointerup') save(apply(start - (e.clientY - startY), p));
        else apply(load(), p);
      };
      splitter.addEventListener('pointermove', move); splitter.addEventListener('pointerup', up); splitter.addEventListener('pointercancel', up);
    });
    splitter.addEventListener('dblclick', event => { event.preventDefault(); reset(); });
    splitter.addEventListener('keydown', event => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const p = parts(); if (!p) return;
      if (event.key === 'ArrowUp' || event.key === 'ArrowDown') {
        event.preventDefault(); event.stopPropagation();
        const step = (event.shiftKey ? BIG_STEP : STEP) * (event.key === 'ArrowUp' ? 1 : -1);
        set(currentHeight(p) + step);
      } else if (event.key === 'Home') { event.preventDefault(); event.stopPropagation(); reset(); }
    });
    if (!resizeBound) {
      resizeBound = true;
      window.addEventListener('resize', () => { const saved = load(); if (saved !== null && parts()) apply(saved); });
    }
  }

  window.STMixerSplitter = { bind, set, reset, limits, apply, KEY, MIN_STRIPS, MIN_DETAIL };
})();
