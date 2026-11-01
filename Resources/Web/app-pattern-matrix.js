/* Pattern matrix: the pattern box under the instrument list in the Pattern view (like Renoise's pattern matrix).
   Two tabs: "All patterns" (the session's pattern list) and "Current song" (the song's blocks in order). Rows reuse
   the song arranger's block look (song-sequence.css). Song edits go through STSong.actions, so undo labels, repeat
   runs and the section loop behave exactly as in the Song view; pattern-list edits use the existing pattern
   commands. The list is one tab stop (role=listbox): ↑ ↓ choose, ⌥↑↓ move, ⌘D duplicate, ⌫ remove, N new, Enter
   edits in the grid, Esc returns to the tabs. Current song also takes the arranger's keys (I insert, ⇧⌘D clone,
   − + repeats, L loop, P play from here). The chosen tab is a session preference (state.patternMatrixTab), never
   saved, and never changes on its own. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, refresh } = app;
  const M = () => window.STSongModel;
  const Song = () => window.STSong;
  const pad2 = n => String(n).padStart(2, '0');

  const tab = () => state.patternMatrixTab === 'all' ? 'all' : 'song';
  const blocks = () => M()?.blocks(state.project.sequence) || [];
  const currentBlock = () => M() ? M().blockAt(state.project.sequence, state.sequence || 0) : 0;
  const color = id => M()?.patternColor?.(state.project, id) || 'var(--amber)';
  const songCount = id => state.project.sequence.filter(x => x === id).length;

  function row({ id, index, current, pattern, number, count, extra }) {
    return `<div class="song-block matrix-block ${current ? 'current selected' : ''} ${pattern ? '' : 'missing'}" id="${id}" role="option"`
        + ` aria-selected="${current}" ${extra} style="--pattern-color:${color(pattern?.id)}"><span class="song-pos">${index}</span>`
        + `<span class="song-swatch" aria-hidden="true"></span><span class="song-pnum">P${pad2(number)}</span>`
        + `<span class="song-name">${esc(pattern?.name || 'Missing pattern')}</span>`
        + `<span class="song-repeat ${count > 1 ? 'multi' : ''}">${count ? '×' + count : ''}</span></div>`;
  }

  function rows() {
    const p = state.project;
    if (tab() === 'all') return p.patterns.map((pattern, i) => row({
      id: `matrix-pattern-${pattern.id}`, index: pad2(i + 1), current: pattern.id === p.activePattern, pattern, number: i + 1,
      count: songCount(pattern.id), extra: `data-matrix-pattern="${pattern.id}" title="${esc(pattern.name)} · ${pattern.length} rows`
          + ` · ${songCount(pattern.id) ? `${songCount(pattern.id)}× in the song` : 'not in the song'}"`
    })).join('');
    const at = currentBlock();
    return blocks().map((block, i) => {
      const pattern = p.patterns.find(x => x.id === block.pattern);
      return row({
        id: `matrix-block-${i}`, index: pad2(i + 1), current: i === at, pattern, number: M().patternNumber(p, block.pattern),
        count: block.count, extra: `data-matrix-block="${i}" title="Song block ${i + 1} · ${esc(pattern?.name || 'Missing pattern')} · ×${block.count}"`
      });
    }).join('');
  }

  function patternMatrix() {
    const all = tab() === 'all', active = all ? `matrix-pattern-${state.project.activePattern}` : `matrix-block-${currentBlock()}`;
    const tabButton = (id, label) => `<button type="button" role="tab" id="matrix-tab-${id}" data-matrix-tab="${id}"`
        + ` aria-selected="${tab() === id}" aria-controls="pattern-matrix" tabindex="${tab() === id ? 0 : -1}"`
        + ` class="${tab() === id ? 'is-on' : ''}">${label}</button>`;
    const tool = (action, label, title) => `<button class="quiet" data-matrix-action="${action}" title="${title}">${label}</button>`;
    return `<div class="fx-pages matrix-tabs" role="tablist" aria-label="Pattern list">${tabButton('all', 'All patterns')}${tabButton('song', 'Current song')}</div>`
        + `<div class="matrix-tools" role="toolbar" aria-label="${all ? 'Patterns' : 'Song blocks'}">`
        + (all
            ? tool('add', '+ New', 'New pattern · N') + tool('duplicate', 'Duplicate', 'Duplicate pattern · ⌘D')
                + tool('to-song', '→ Song', 'Add this pattern to the song after the current block · ⇧Return')
                + tool('remove', 'Delete', 'Delete pattern · ⌫')
            : tool('add', '+ New', 'Insert a new empty pattern after this block · N') + tool('duplicate', 'Duplicate', 'Duplicate block (one more repeat) · ⌘D')
                + tool('clone', 'Clone', 'Clone as a new pattern after this block · ⇧⌘D') + tool('remove', 'Remove', 'Remove block from the song · ⌫'))
        + `</div><div class="matrix-list" id="pattern-matrix" role="listbox" tabindex="0" data-scroll="pattern-matrix"`
        + ` aria-label="${all ? 'All patterns' : 'Current song'}. Up and down choose, Option up and down move, Command D duplicates,`
        + ` Delete removes, N adds a new pattern, Return edits in the grid, Escape returns to the tabs." aria-activedescendant="${active}">${rows()}</div>`;
  }

  function focusList() {
    $('#pattern-matrix')?.focus({ preventScroll: true });
    $('#pattern-matrix [aria-selected="true"]')?.scrollIntoView?.({ block: 'nearest' });
  }

  /* Choose a row: the grid shows its pattern; a song block also becomes the song position (state.sequence). */
  function choose(index) {
    if (tab() === 'all') {
      const list = state.project.patterns, next = list[Math.max(0, Math.min(list.length - 1, index))];
      if (next && next.id !== state.project.activePattern) app.selectPattern(next.id);
    } else {
      const list = blocks(), block = list[Math.max(0, Math.min(list.length - 1, index))];
      if (!block) return;
      state.sequence = block.start;
      if (block.pattern !== state.project.activePattern) app.selectPattern(block.pattern);
      else refresh();
    }
    focusList();
  }

  const selectedIndex = () => tab() === 'all'
      ? state.project.patterns.findIndex(x => x.id === state.project.activePattern)
      : currentBlock();

  /* Song edits act on the block at state.sequence; a block edit keeps the grid on the edited block's pattern. */
  function songAction(name) {
    const actions = Song()?.actions;
    if (!actions) return false;
    // The matrix selects one block: drop a range the arranger kept (e.g. after a duplicate).
    Song().selectBlock(currentBlock(), { focus: false });
    const result = name === 'add' ? actions.insertNew()
        : name === 'duplicate' ? actions.duplicate()
        : name === 'clone' ? actions.clone()
        : name === 'remove' ? actions.remove()
        : name === 'up' ? actions.move(-1)
        : name === 'down' ? actions.move(1)
        : name === 'insert' ? actions.insertNext()
        : name === 'more' ? actions.repeats(1)
        : name === 'fewer' ? actions.repeats(-1)
        : name === 'loop' ? actions.toggleLoop()
        : name === 'play' ? actions.playFrom()
        : false;
    const block = blocks()[currentBlock()];
    if (block && block.pattern !== state.project.activePattern && state.project.patterns.some(x => x.id === block.pattern)) {
      state.project.activePattern = block.pattern;
      state.row = 0;
      refresh();
    }
    return result;
  }

  function patternAction(name) {
    const p = state.project, index = selectedIndex();
    if (name === 'add') app.perform('addPattern');
    else if (name === 'duplicate') app.perform('duplicatePattern');
    else if (name === 'remove') app.perform('removePattern');
    else if (name === 'up' || name === 'down') return app.movePattern(p.activePattern, index + (name === 'up' ? -1 : 1));
    else if (name === 'to-song') return Song()?.actions.insertPattern(p.activePattern);
    return true;
  }

  function act(name) {
    const result = tab() === 'all' ? patternAction(name) : songAction(name);
    focusList();
    return result;
  }

  function setTab(next, focus = true) {
    state.patternMatrixTab = next === 'all' ? 'all' : 'song';
    refresh();
    if (focus) $(`#matrix-tab-${tab()}`)?.focus({ preventScroll: true });
    return true;
  }

  function listKey(event) {
    const k = event.key, cmd = event.metaKey || event.ctrlKey, count = tab() === 'all' ? state.project.patterns.length : blocks().length;
    const done = () => { event.preventDefault(); event.stopPropagation?.(); return true; };
    if (cmd && String(k).toLowerCase() === 'd') { done(); act(event.shiftKey && tab() === 'song' ? 'clone' : 'duplicate'); return true; }
    if (cmd) return false;
    if (event.altKey && (k === 'ArrowUp' || k === 'ArrowDown')) { done(); act(k === 'ArrowUp' ? 'up' : 'down'); return true; }
    if (event.altKey) return false;
    if (k === 'ArrowUp' || k === 'ArrowDown') { done(); choose(selectedIndex() + (k === 'ArrowUp' ? -1 : 1)); return true; }
    if (k === 'Home' || k === 'End') { done(); choose(k === 'Home' ? 0 : count - 1); return true; }
    if (k === 'Delete' || k === 'Backspace') { done(); act('remove'); return true; }
    if (k === 'Escape') { done(); $(`#matrix-tab-${tab()}`)?.focus({ preventScroll: true }); return true; }
    if (event.shiftKey && k !== 'Enter' && k !== '+') return false;
    const lower = String(k).toLowerCase(), song = tab() === 'song';
    // Letters yield to the computer-keyboard piano, as in the Song view's arranger.
    if (state.keyboardPlay && /^[a-z]$/.test(lower)) return false;
    if (lower === 'n') { done(); act('add'); return true; }
    if (song && lower === 'i') { done(); act('insert'); return true; }
    if (song && (k === '+' || k === '=')) { done(); act('more'); return true; }
    if (song && (k === '-' || k === '_')) { done(); act('fewer'); return true; }
    if (song && lower === 'l') { done(); act('loop'); return true; }
    if (song && lower === 'p') { done(); act('play'); return true; }
    if (k === 'Enter' && event.shiftKey && tab() === 'all') { done(); act('to-song'); return true; }
    if (k === 'Enter' || k === 'ArrowRight') { done(); $('#tracker')?.focus({ preventScroll: true }); return true; }
    return false;
  }

  function bindPatternMatrix() {
    for (const button of $$('[data-matrix-tab]')) {
      button.onclick = () => setTab(button.dataset.matrixTab);
      button.onkeydown = event => {
        if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
          event.preventDefault();
          setTab(event.key === 'ArrowLeft' ? 'all' : 'song');
        } else if (event.key === 'ArrowDown') {
          event.preventDefault();
          focusList();
        }
      };
    }
    for (const button of $$('[data-matrix-action]')) button.onclick = () => act(button.dataset.matrixAction);
    for (const node of $$('[data-matrix-pattern],[data-matrix-block]')) {
      const index = () => node.dataset.matrixPattern !== undefined
          ? state.project.patterns.findIndex(x => x.id === Number(node.dataset.matrixPattern))
          : Number(node.dataset.matrixBlock);
      node.onclick = () => choose(index());
      node.ondblclick = () => { choose(index()); $('#tracker')?.focus({ preventScroll: true }); };
    }
    const list = $('#pattern-matrix');
    if (list) list.onkeydown = listKey;
  }

  /** The song block playing now, painted without a refresh (transport ticks). */
  function paintMatrixPlayhead(status = state.status || {}) {
    const playing = status.playing && status.songMode && Number.isInteger(status.sequence) && M()
        ? M().blockAt(state.project.sequence, status.sequence)
        : -1;
    for (const node of $$('[data-matrix-block]')) node.classList.toggle('playing', Number(node.dataset.matrixBlock) === playing);
  }

  Object.assign(app, { patternMatrix, bindPatternMatrix, setPatternMatrixTab: setTab, patternMatrixAction: act, paintMatrixPlayhead });
})();
