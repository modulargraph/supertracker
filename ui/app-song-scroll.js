/* Song scroll: the pattern grid shows the song's slots in song order as one continuous list (⌥⇧F, or the
   "Song scroll" toolbar button). Each slot starts with a header row (pattern colour, number and name).

   Only the current slot (state.sequence, playing the active pattern) is the live, editable section: its cells are
   the ordinary data-cell cells, so editing, selection and the grid mouse work unchanged. The other slots are
   read-only context rows (tr[data-context], no data-cell); a click on one makes that slot current at that row, and
   ↑ ↓ PageUp PageDown past a pattern edge continue in the neighbouring slot (⌘[ ⌘] step a slot). Every slot is
   always rendered in the same order, so changing the live slot does not move the list. With Follow on, playback
   makes the playing slot current; otherwise only its row is marked.

   Long songs render the slots around the current one up to MAX_ROWS rows, with a note row for the rest.
   The switch is a saved view preference, like Hide empty: project.viewState.songScroll. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, refresh, pattern, clamp } = app;
  const SM = () => window.STSongModel;
  const MAX_ROWS = 1500;
  const pad2 = n => String(n).padStart(2, '0');

  const songScrollOn = (p = state.project) => !!p?.viewState.songScroll;

  /** The song as slots on one row axis: [{flat, pattern, start, length, occurrence, occurrences}]. */
  function slots(p = state.project) {
    const counts = new Map(), seen = new Map();
    for (const id of p.sequence || []) counts.set(id, (counts.get(id) || 0) + 1);
    let start = 0;
    return (p.sequence || []).map((id, flat) => {
      const item = p.patterns.find(x => x.id === id), length = item?.length || 0, occurrence = (seen.get(id) || 0) + 1;
      seen.set(id, occurrence);
      const slot = { flat, pattern: id, item, start, length, occurrence, occurrences: counts.get(id) };
      start += length;
      return slot;
    });
  }

  /** The live slot: the song position when it plays the active pattern, else the active pattern's first slot (-1: not in the song). */
  function liveSlot(p = state.project) {
    const sequence = p.sequence || [];
    if (sequence[state.sequence] === p.activePattern) return state.sequence;
    return sequence.indexOf(p.activePattern);
  }

  /** Slots to render: around the live one, at most MAX_ROWS rows (the live slot always). */
  function windowOf(list, live) {
    let first = live, last = live, rows = list[live].length;
    for (let grow = 1; first > 0 || last < list.length - 1; grow++) {
      const before = list[first - 1], after = list[last + 1];
      if (grow % 2 && after && rows + after.length <= MAX_ROWS) { last++; rows += after.length; }
      else if (before && rows + before.length <= MAX_ROWS) { first--; rows += before.length; }
      else if (after && rows + after.length <= MAX_ROWS) { last++; rows += after.length; }
      else break;
    }
    return [first, last];
  }

  /** Rows of the bound context pattern rendered as the active one; `row(r, attrs, live)` is the grid's row renderer. */
  function asContext(slot, row) {
    const p = state.project, active = p.activePattern, cursor = state.row;
    p.activePattern = slot.pattern;
    state.row = -1;
    try {
      // Context cells are not grid cells: without data-cell no pointer, edit or paint code reaches them.
      return Array.from({ length: slot.length }, (_, r) => row(r, ` data-slot="${slot.flat}" data-context`, false)
          .replace(/ data-cell="/g, ' data-context-cell="')).join('');
    } finally {
      p.activePattern = active;
      state.row = cursor;
    }
  }

  function headRow(slot, live, width) {
    const p = state.project;
    return `<tr class="song-scroll-head${live ? ' is-live' : ''}" data-slot-head="${slot.flat}" style="--pattern-color:${SM()?.patternColor?.(p, slot.pattern) || 'var(--amber)'}">`
        + `<td colspan="${width}"><span class="song-scroll-label"><b>P${pad2(SM()?.patternNumber?.(p, slot.pattern) || 0)}</b> ${esc(slot.item?.name || 'Missing pattern')}`
        + ` <span>slot ${pad2(slot.flat + 1)} · ${slot.length} rows${slot.occurrences > 1 ? ` · repeat ${slot.occurrence}/${slot.occurrences}` : ''}</span></span></td></tr>`;
  }

  /** tbody rows for the grid, or null when song scroll is off (or the active pattern is not in the song). */
  function songScrollRows(row, width) {
    if (!songScrollOn() || state.view !== 'pattern') return null;
    const list = slots(), live = liveSlot();
    if (live < 0) return `<tr class="song-scroll-note"><td colspan="${width}">This pattern is not in the song · song scroll shows it alone</td></tr>`
        + Array.from({ length: pattern().length }, (_, r) => row(r)).join('');
    const [first, last] = windowOf(list, live);
    let html = first > 0 ? `<tr class="song-scroll-note"><td colspan="${width}">${first} earlier slot${first === 1 ? '' : 's'} · ⌘[ steps back</td></tr>` : '';
    for (let i = first; i <= last; i++) {
      const slot = list[i];
      html += headRow(slot, i === live, width);
      html += i === live ? Array.from({ length: slot.length }, (_, r) => row(r, ` data-slot="${slot.flat}"`)).join('') : asContext(slot, row);
    }
    const rest = list.length - 1 - last;
    if (rest > 0) html += `<tr class="song-scroll-note"><td colspan="${width}">${rest} later slot${rest === 1 ? '' : 's'} · ⌘] steps on</td></tr>`;
    return html;
  }

  /** Make a slot current at `row` (no refresh). */
  function enter(slot, row) {
    state.sequence = slot.flat;
    if (slot.pattern !== state.project.activePattern) {
      state.project.activePattern = slot.pattern;
      state.selection = null;
      if (state.patternLoop?.pattern !== slot.pattern) app.clearPatternLoop?.();
    }
    state.row = clamp(row, 0, Math.max(0, slot.length - 1));
  }

  /** A cursor move to `row` of the live pattern: past its edges it continues in the neighbouring slots. */
  function songScrollStep(row) {
    if (!songScrollOn() || state.view !== 'pattern') return false;
    const list = slots(), live = liveSlot();
    if (live < 0 || row >= 0 && row < list[live].length) return false;
    let index = live, rest = row;
    while (rest < 0 && index > 0) { index--; rest += list[index].length; }
    while (index < list.length && rest >= list[index].length && index < list.length - 1) { rest -= list[index].length; index++; }
    if (index === live) return false;
    enter(list[index], rest);
    return true;
  }

  /** ⌘[ ⌘]: the previous / next song slot, keeping the row where it fits. */
  function songScrollSlot(delta) {
    const list = slots(), live = liveSlot(), next = list[clamp((live < 0 ? 0 : live) + delta, 0, list.length - 1)];
    if (!next || next.flat === live) return false;
    enter(next, state.row);
    refresh();
    $('#tracker')?.focus({ preventScroll: true });
    revealCursor();
    return true;
  }

  const revealCursor = () => $('#tracker td.selected')?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });

  /** With Follow on, playback in Song mode makes the playing slot current (repeats included). */
  function songScrollFollow(s) {
    if (!songScrollOn() || !s.playing || !s.songMode || !state.follow || state.view !== 'pattern'
        || state.recording && state.recordPlacement !== 'live' || !Number.isInteger(s.sequence)) return false;
    const slot = slots()[s.sequence];
    if (!slot || slot.flat === liveSlot() && slot.pattern === state.project.activePattern) return false;
    const row = state.row, selection = state.selection;
    enter(slot, row);
    if (selection && slot.pattern === state.project.activePattern) state.selection = selection;
    refresh();
    return true;
  }

  function setSongScroll(on = !songScrollOn()) {
    app.setViewState(view => { view.songScroll = on || undefined; });
    if (on && liveSlot() >= 0) state.sequence = liveSlot();
    refresh();
    $('#tracker')?.focus({ preventScroll: true });
    revealCursor();
    return true;
  }

  function bindSongScroll() {
    if ($('#song-scroll')) $('#song-scroll').onclick = () => setSongScroll();
    if (!songScrollOn()) return;
    const list = slots();
    for (const tr of $$('#tracker tr[data-context]')) tr.onclick = event => {
      const slot = list[Number(tr.dataset.slot)];
      if (!slot) return;
      app.finishEntry?.({ advance: false, render: false });
      enter(slot, Number(tr.dataset.row));
      // The context cell's track and sub-column, like a click on a live cell.
      const cell = event.target?.closest?.('td[data-context-cell]');
      if (cell) state.track = Number(cell.dataset.contextCell.split(':')[0]);
      const column = event.target?.dataset?.column;
      if (column !== undefined) state.column = Number(column);
      refresh();
      $('#tracker')?.focus({ preventScroll: true });
    };
  }

  function songScrollButton() {
    const on = songScrollOn();
    return `<button id="song-scroll" class="${on ? 'active' : ''}" aria-pressed="${on}" title="Song scroll: the grid runs through the song's`
        + ` patterns in song order as one list · ⌥⇧F">Song scroll</button>`;
  }

  app.defineCommands({ songScroll: () => { if (state.view !== 'pattern') app.setView('pattern'); return setSongScroll(); } });

  Object.assign(app, {
    songScrollOn, songScrollRows, songScrollStep, songScrollSlot, songScrollFollow, setSongScroll, bindSongScroll, songScrollButton
  });
})();
