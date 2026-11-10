/* Wide sessions (no track limit): the grid builds only the columns overlapping the
   viewport plus a small overscan, and always the cursor column so keyboard moves can scroll it into view.
   Spacer cells keep the full scroll width; a horizontal scroll that changes the visible range re-renders. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, clamp, state, refresh, patternColumns, columnFor } = app;
  // Wide sessions (no track limit): the grid builds only the columns overlapping the viewport plus a small
  // overscan, and always the cursor column so keyboard moves can scroll it into view. Spacer cells keep the
  // full scroll width; a horizontal scroll that changes the visible range re-renders (rAF-throttled).
  const PATTERN_VIRTUAL_MIN_COLUMNS = 32;
  const PATTERN_OVERSCAN = 2;
  let patternViewportWidth = 0;
  let patternRenderedRange = null;

  function virtualSegments(widths, left, width, keep = [], overscan = 2) {
    const n = widths.length,
      starts = new Array(n);
    let x = 0;
    for (let i = 0; i < n; i++) {
      starts[i] = x;
      x += widths[i];
    }
    width = Math.max(1, Number(width) || 1);
    left = clamp(Number(left) || 0, 0, Math.max(0, x - width));
    let first = 0;
    while (first < n - 1 && starts[first] + widths[first] <= left) first++;
    let last = first;
    while (last < n - 1 && starts[last + 1] < left + width) last++;
    first = Math.max(0, first - overscan);
    last = Math.min(n - 1, last + overscan);
    const extra = new Set(keep.filter(k => Number.isInteger(k) && k >= 0 && k < n && (k < first || k > last))),
      segments = [];
    let gap = 0;
    for (let i = 0; i < n; i++) {
      if (i >= first && i <= last || extra.has(i)) {
        if (gap) segments.push({ spacer: true, width: gap });
        gap = 0;
        segments.push({ index: i, width: widths[i] });
      } else {
        gap += widths[i];
      }
    }
    if (gap) segments.push({ spacer: true, width: gap });
    return { first, last, total: x, segments, key: `${n}:${first}:${last}` };
  }

  window.STVirtual = { segments: virtualSegments };

  // A collapsed track group's overview column: a little wider than a track column so a row's pad hits fit as
  // compact chips. Used by the virtual grid and, as a fixed <col> width, by the ordinary grid.
  const GROUP_OVERVIEW_WIDTH = 260;
  const patternColumnWidth = c => c.collapsed ? GROUP_OVERVIEW_WIDTH : c.slim ? app.SLIM_WIDTH || 58 : 182 + (app.noteColumnExtraWidth?.(c.index) || 0);

  const patternCursorColumn = columns => columnFor(columns, state.track);

  function patternVisibleRange(columns, wrap, keep = []) {
    const width = wrap && Number(wrap.clientWidth) || patternViewportWidth || Number(window.innerWidth) || 1280;
    patternViewportWidth = width;
    return virtualSegments(columns.map(patternColumnWidth), wrap ? wrap.scrollLeft : 0, width - 40, keep,
        PATTERN_OVERSCAN);
  }

  // Reads the outgoing grid's scroll position: refresh() restores exactly that position on the new grid.
  function patternLayout(columns) {
    if (columns.length <= PATTERN_VIRTUAL_MIN_COLUMNS) {
      patternRenderedRange = null;
      return null;
    }
    const layout = patternVisibleRange(columns, $('.tracker-wrap'), [patternCursorColumn(columns)]);
    patternRenderedRange = layout.key;
    return layout;
  }

  function checkPatternViewport() {
    if (state.view !== 'pattern' || !patternRenderedRange) return false;
    const wrap = $('.tracker-wrap');
    if (!wrap) return false;
    if (patternVisibleRange(patternColumns(), wrap).key === patternRenderedRange) return false;
    refresh();
    return true;
  }

  function bindPatternViewport() {
    const wrap = $('.tracker-wrap');
    if (!wrap || !patternRenderedRange) return;
    let queued = false;
    wrap.addEventListener('scroll', () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        if (wrap.isConnected !== false) checkPatternViewport();
      });
    }, { passive: true });
  }

  window.addEventListener?.('resize', () => {
    if (patternRenderedRange) requestAnimationFrame(checkPatternViewport);
  });

  Object.assign(app, { patternLayout, bindPatternViewport, patternColumnWidth, GROUP_OVERVIEW_WIDTH });
})();
