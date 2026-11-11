/* Song arranger: Polyend-style pattern sequence edited entirely from the keyboard.
   Blocks are runs of one pattern (×repeats) over the flat `project.sequence`.
   See song-model.js for the data rules; this file owns markup, keys and playhead. */
(() => {
  'use strict';
  const M = () => window.STSongModel;
  const app = () => window.ST, state = () => app().state, project = () => state().project;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const local = { anchor: null, follow: true, typed: '', typedAt: 0, playing: -1, contextRegistered: false };
  const TYPE_TIMEOUT = 900;

  const blockList = () => M().blocks(project().sequence);
  const selectedBlock = () => M().blockAt(project().sequence, state().sequence || 0);
  function range() {
    const current = selectedBlock(), count = blockList().length;
    const anchor = local.anchor == null ? current : Math.min(Math.max(0, local.anchor), count - 1);
    return [Math.min(anchor, current), Math.max(anchor, current)];
  }
  function selectBlock(index, { extend = false, focus = true } = {}) {
    const list = blockList(), next = Math.min(Math.max(0, index), list.length - 1);
    if (extend) { if (local.anchor == null) local.anchor = selectedBlock(); }
    else local.anchor = null;
    state().sequence = list[next].start;
    paintSelection();
    if (focus) document.getElementById('sequence-list')?.focus({ preventScroll: true });
    document.getElementById(`song-block-${next}`)?.scrollIntoView?.({ block: 'nearest' });
  }
  function patternOf(id) { return project().patterns.find(pattern => pattern.id === id); }
  function lengthLabel(pattern) {
    const p = project(), rows = pattern?.length || 0;
    const bars = rows / ((p.linesPerBeat || 4) * (app().beatsPerBar?.(p) || 4));
    return (Number.isInteger(bars) ? bars : bars.toFixed(2)) + (bars === 1 ? ' bar' : ' bars');
  }
  function barLabel(flat) {
    const p = project(), beat = M().startBeat(p, flat);
    const perBar = app().beatsPerBar?.(p) || 4;
    return 'bar ' + (Math.floor(beat / perBar + 1e-9) + 1);
  }
  function mini(pattern) {
    if (!pattern) return '';
    const steps = pattern.steps || [], tracks = Math.max(steps.length, 1);
    return Array.from({ length: 24 }, (_, bin) => {
      let count = 0; const row = Math.floor(bin / 24 * pattern.length);
      for (let track = 0; track < steps.length; track++) if (steps[track]?.[row]) count++;
      return `<i style="height:${Math.max(2, Math.round(count / tracks * 14))}px"></i>`;
    }).join('');
  }

  function renderPanel() {
    const p = project(), list = blockList(), current = selectedBlock(), [a, b] = range(), loop = M().loopRange(p);
    const loopLabel = loop ? `${String(M().blockAt(p.sequence, loop.start) + 1).padStart(2, '0')}–${String(M().blockAt(p.sequence, loop.end) + 1).padStart(2, '0')}` : '';
    const block = list[current];
    return `<div class="panel song-seq" data-song-panel>
      <div class="panel-head"><h2>Sequence</h2><div class="row song-seq-chips"><span class="chip">${list.length} ${list.length === 1 ? 'BLOCK' : 'BLOCKS'} · ${p.sequence.length} ${p.sequence.length === 1 ? 'SLOT' : 'SLOTS'}</span>${loop ? `<span class="chip song-loop-chip ${p.loopSong ? '' : 'inactive'}" title="${p.loopSong ? 'Song playback loops this section · L clears' : 'Loop song is off, so this section is not looping'}">⟲ ${loopLabel}</span>` : ''}<button id="song-follow" class="quiet ${local.follow ? 'active' : ''}" aria-pressed="${local.follow}" title="Follow the song playhead · F">Follow</button></div></div>
      <div class="song-seq-tools" role="toolbar" aria-label="Selected block">
        <label class="song-seq-field">Pattern<select id="song-block-pattern" aria-label="Pattern for the selected block">${p.patterns.map((item, index) => `<option value="${item.id}" ${item.id === block?.pattern ? 'selected' : ''}>${String(index + 1).padStart(2, '0')} · ${esc(item.name)}</option>`).join('')}</select></label>
        <label class="song-seq-field">Repeat<input id="song-block-repeats" type="number" min="1" max="${M().MAX_REPEATS}" step="1" value="${block?.count || 1}" aria-label="Repeat count for the selected block"></label>
        <button id="song-insert" title="Insert the next pattern after this block · I">+ Insert</button>
        <button id="song-new" title="Insert a new empty pattern · N">+ New</button>
        <button id="song-clone" title="Clone this pattern as a new variation · ⇧⌘D">Clone</button>
        <button id="song-rename" title="Rename pattern · E">Rename</button>
        <button id="song-loop" class="${loop ? 'active' : ''}" aria-pressed="${!!loop}" title="Loop the selected blocks in Song playback · L">⟲ Loop</button>
        <button id="song-play-from" title="Play the song from this block · P or ⇧Return">▶ From here</button>
      </div>
      <div class="sequence-list song-blocks" id="sequence-list" data-song-arranger="1" data-scroll="song-sequence" role="listbox" aria-multiselectable="true" tabindex="0"
        aria-label="Song sequence. Up and down select blocks, shift extends. Left and right or typed digits change the pattern. Minus and plus change repeats. Option up and down move. I inserts, N adds a new pattern, Command D duplicates, Delete removes, L loops, P plays from here, Return edits."
        aria-activedescendant="song-block-${current}">
        ${list.map((item, index) => {
          const pat = patternOf(item.pattern), number = M().patternNumber(p, item.pattern), inLoop = loop && item.start >= loop.start && item.start <= loop.end;
          const loopEdge = inLoop ? `${item.start === loop.start ? ' loop-start' : ''}${item.start + item.count - 1 >= loop.end ? ' loop-end' : ''}` : '';
          return `<div class="song-block ${index === current ? 'current' : ''} ${index >= a && index <= b ? 'selected' : ''} ${inLoop ? 'in-loop' : ''}${loopEdge} ${pat ? '' : 'missing'}" id="song-block-${index}" data-song-block="${index}" role="option" aria-selected="${index >= a && index <= b}" style="--pattern-color:${M().patternColor(p, item.pattern)}">
            <span class="song-pos">${String(index + 1).padStart(2, '0')}</span>
            <span class="song-swatch" aria-hidden="true"></span>
            <span class="song-pnum">P${String(number || 0).padStart(2, '0')}</span>
            <span class="song-name">${esc(pat?.name || 'Missing pattern')}</span>
            <span class="song-mini" aria-hidden="true">${mini(pat)}</span>
            <span class="song-repeat ${item.count > 1 ? 'multi' : ''}" title="Repeats · − / +">×${item.count}</span>
            <span class="song-progress" aria-hidden="true"></span>
            <span class="song-len">${esc(lengthLabel(pat))}</span>
            <span class="song-at">${esc(barLabel(item.start))}</span>
          </div>`;
        }).join('')}
      </div>
      <div class="song-seq-keys" aria-hidden="true"><span><kbd>↑↓</kbd> select <kbd>⇧</kbd> extend</span><span><kbd>←→</kbd>/<kbd>0-9</kbd> pattern</span><span><kbd>−</kbd><kbd>+</kbd> repeats</span><span><kbd>⌥↑↓</kbd> move</span><span><kbd>I</kbd> insert <kbd>N</kbd> new</span><span><kbd>⌘D</kbd> duplicate</span><span><kbd>⌫</kbd> remove</span><span><kbd>L</kbd> loop</span><span><kbd>P</kbd> play here</span><span><kbd>E</kbd> rename <kbd>C</kbd> colour</span><span><kbd>↩</kbd> edit</span></div>
    </div>`;
  }

  /* Validate on a light copy first: a rejected edit must not leave an empty undo step. */
  function edit(label, op, { focus = true, render = true } = {}) {
    const p = project();
    const trial = { sequence: p.sequence.slice(), songLoop: p.songLoop ? { ...p.songLoop } : undefined, loopSong: p.loopSong, linesPerBeat: p.linesPerBeat, tracks: p.tracks, patterns: p.patterns.map(pattern => ({ id: pattern.id, name: pattern.name, length: pattern.length, color: pattern.color })) };
    if (!trial.songLoop) delete trial.songLoop;
    try { op(trial); } catch (error) { app().toast(error.message || String(error), 'error'); return false; }
    let result, committed = false;
    app().mutate(label, target => { result = op(target); committed = true; }, {render});
    if (!committed || project() !== p) return false;
    if (focus) document.getElementById('sequence-list')?.focus({ preventScroll: true });
    return result ?? true;
  }

  const actions = {
    changePattern(delta) { const block = selectedBlock(), id = blockList()[block].pattern; return edit('change sequence pattern', p => { const next = M().setPattern(p, block, M().nextPatternId(p, id, delta)); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[next].start; } }); },
    setPatternId(id, options) { const block = selectedBlock(); return edit('change sequence pattern', p => { const next = M().setPattern(p, block, id); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[next].start; } }, options); },
    repeats(delta, absolute = null, options) { const block = selectedBlock(), count = blockList()[block].count; const next = absolute ?? count + delta; if (next === count) return false; if (next < 1 || next > M().MAX_REPEATS) { app().toast(next < 1 ? 'Use Delete to remove a block.' : `Up to ${M().MAX_REPEATS} repeats per block.`); return false; } return edit('change sequence repeats', p => { const at = M().setRepeats(p, block, next); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start; } }, options); },
    insertNext() { const block = selectedBlock(), list = blockList(), id = list[block].pattern, after = list[block + 1]?.pattern, p0 = project(); let next = M().nextPatternId(p0, id, 1); for (let step = 2; (next === id || next === after) && step <= p0.patterns.length; step++) next = M().nextPatternId(p0, id, step); if (next === id || next === after) return actions.insertNew(); return edit('insert sequence block', p => { const at = M().insert(p, block, next); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start; } }); },
    insertPattern(id) { const block = selectedBlock(); return edit('insert sequence block', p => { const at = M().insert(p, block, id); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start; } }); },
    insertNew() {
      const block = selectedBlock(), id = blockList()[block].pattern, length = patternOf(id)?.length || 64;
      return edit('insert new pattern', p => { const pattern = M().blankPattern(p, length); p.patterns.push(pattern); const at = M().insert(p, block, pattern.id); if (p === project()) { p.activePattern = pattern.id; local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start; } });
    },
    clone() {
      const block = selectedBlock(), id = blockList()[block].pattern;
      if (project().patterns.length >= 256) { app().toast('This session supports up to 256 patterns.', 'error'); return false; }
      return edit('clone pattern into sequence', p => {
        if (p !== project()) { const free = M().freePatternId(p); if (free < 0) throw new Error('This session supports up to 256 patterns.'); p.patterns.push({ id: free }); M().insert(p, block, free); return; }
        const copy = M().clonePattern(p, id, app().uid); p.patterns.push(copy); const at = M().insert(p, block, copy.id); p.activePattern = copy.id; local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start;
      });
    },
    duplicate() { const [a, b] = range(); return edit('duplicate sequence blocks', p => { const at = M().duplicate(p, a, b); if (p === project()) { const list = M().blocks(p.sequence); local.anchor = at; state().sequence = list[Math.min(list.length - 1, at + (b - a))].start; } }); },
    remove() { const [a, b] = range(); return edit(a === b ? 'remove sequence block' : 'remove sequence blocks', p => { const at = M().remove(p, a, b); if (p === project()) { local.anchor = null; state().sequence = M().blocks(p.sequence)[at].start; } }); },
    move(delta) {
      const [a, b] = range(), current = selectedBlock(), anchored = local.anchor != null;
      const list = blockList(); if ((delta < 0 && a === 0) || (delta > 0 && b === list.length - 1)) return false;
      return edit('move sequence blocks', p => { const first = M().move(p, a, b, delta); if (p === project() && first >= 0) { const after = M().blocks(p.sequence); const offset = current - a; state().sequence = after[Math.min(after.length - 1, first + offset)].start; local.anchor = anchored ? first + (current === a ? b - a : 0) : null; } });
    },
    toggleLoop() {
      const [a, b] = range(), p0 = project(), loop = M().loopRange(p0);
      const list = blockList(), start = list[a].start, end = list[b].start + list[b].count - 1;
      if (loop && loop.start === start && loop.end === end) return edit('clear song loop', p => { M().clearLoop(p); });
      return edit('loop song section', p => { M().setLoop(p, a, b); });
    },
    clearLoop() { if (!project().songLoop) return false; return edit('clear song loop', p => { M().clearLoop(p); }); },
    rename() {
      const id = blockList()[selectedBlock()].pattern, pattern = patternOf(id); if (!pattern) return false;
      app().textPrompt('Rename pattern', pattern.name || '', value => { app().mutate('rename pattern', p => { const target = p.patterns.find(item => item.id === id); if (target) target.name = value.slice(0, 120); }); document.getElementById('sequence-list')?.focus({ preventScroll: true }); });
      return true;
    },
    color(delta = 1) { const id = blockList()[selectedBlock()].pattern; return edit('pattern colour', p => { M().cycleColor(p, id, delta); }); },
    editPattern() { const id = blockList()[selectedBlock()].pattern; if (!patternOf(id)) return false; project().activePattern = id; state().row = 0; app().setView('pattern'); return true; },
    toggleFollow() { local.follow = !local.follow; const button = document.getElementById('song-follow'); if (button) { button.classList.toggle('active', local.follow); button.setAttribute('aria-pressed', String(local.follow)); } app().toast(local.follow ? 'Song follow on · the selection tracks the playhead' : 'Song follow off'); return true; },
    async playFrom(block = selectedBlock()) {
      const s = state(), list = blockList(), item = list[Math.min(Math.max(0, block), list.length - 1)];
      local.anchor = null; s.sequence = item.start; paintSelection();
      if (s.playing && s.status?.songMode) { try { await app().request('transport.seek', { pattern: item.pattern, row: 0, sequence: item.start }); } catch (error) { app().toast(error.message, 'error'); } return true; }
      const list0 = document.getElementById('sequence-list');
      if (s.playing) { await app().perform('play'); if (state().playing) return false; }
      if (s.playbackMode !== 'song') { if (!app().perform('modeSong')) return false; (document.getElementById('sequence-list') || list0)?.focus({ preventScroll: true }); }
      s.sequence = item.start; s.row = 0;
      return app().perform('play');
    },
    /* Maschine-style scene change: while the song plays, jump to the block at the end of the current
       pattern (sample-accurate, queued in the engine). Otherwise the same as Play from here. */
    async queueFrom(block = selectedBlock()) {
      const s = state(), list = blockList(), item = list[Math.min(Math.max(0, block), list.length - 1)];
      if (!(s.playing && s.status?.songMode)) return actions.playFrom(block);
      local.anchor = null; s.sequence = item.start; paintSelection();
      try { const result = await app().request('transport.queue', { sequence: item.start }); if (result?.queued === false) return actions.playFrom(block); app().toast(`Block ${block + 1} plays at the end of this pattern`); return true; }
      catch (error) { app().toast(error.message, 'error'); return false; }
    }
  };

  function typeDigit(digit) {
    const now = Date.now(), p = project();
    let buffer = now - local.typedAt < TYPE_TIMEOUT ? local.typed + digit : digit;
    if (!M().patternByNumber(p, Number(buffer))) buffer = digit;
    local.typed = buffer; local.typedAt = now;
    const pattern = M().patternByNumber(p, Number(buffer));
    if (!pattern) { app().toast(`Pattern ${buffer} does not exist · N adds a new pattern`); return true; }
    if (pattern.id !== blockList()[selectedBlock()].pattern) actions.setPatternId(pattern.id);
    return true;
  }

  /* Keys for the focused sequence list. Returns true when handled. Letter and
     digit commands yield to the computer-keyboard piano while it is on. */
  function key(event) {
    if (event.defaultPrevented || state().view !== 'song' || state().modal) return false;
    const list = event.target?.closest?.('#sequence-list');
    if (!list || event.target !== list && event.target.closest?.('button,select,input')) return false;
    const cmd = event.metaKey || event.ctrlKey, k = event.key, lower = String(k).toLowerCase(), piano = !!state().keyboardPlay;
    const done = () => { event.preventDefault(); return true; };
    const count = blockList().length, current = selectedBlock();
    if (cmd) {
      if (lower === 'd' && event.shiftKey) { done(); actions.clone(); return true; }
      if (lower === 'd') { done(); actions.duplicate(); return true; }
      return false;
    }
    if (k === 'ArrowUp' || k === 'ArrowDown') {
      const delta = k === 'ArrowUp' ? -1 : 1; done();
      if (event.altKey) actions.move(delta); else selectBlock(current + delta, { extend: event.shiftKey });
      return true;
    }
    if ((k === 'Home' || k === 'End') && !event.altKey) { done(); selectBlock(k === 'Home' ? 0 : count - 1, { extend: event.shiftKey }); return true; }
    if ((k === 'PageUp' || k === 'PageDown') && !event.altKey) { done(); selectBlock(current + (k === 'PageUp' ? -8 : 8), { extend: event.shiftKey }); return true; }
    if ((k === 'ArrowLeft' || k === 'ArrowRight') && !event.altKey) { done(); actions.changePattern((k === 'ArrowLeft' ? -1 : 1) * (event.shiftKey ? 10 : 1)); return true; }
    if (k === '-' || k === '_' || k === '=' || k === '+') { done(); actions.repeats(k === '-' || k === '_' ? -1 : 1); return true; }
    if (k === 'Delete' || k === 'Backspace') { done(); actions.remove(); return true; }
    if (k === 'Insert') { done(); actions.insertNext(); return true; }
    if (k === 'Enter') { done(); if (event.shiftKey) actions.playFrom(); else actions.editPattern(); return true; }
    if (k === 'F2') { done(); actions.rename(); return true; }
    if (event.altKey && event.shiftKey && lower === 'l' || event.altKey && event.shiftKey && event.code === 'KeyL') { done(); actions.toggleLoop(); return true; }
    if (event.altKey) return false;
    if (piano) return false;
    if (/^[0-9]$/.test(k)) { done(); return typeDigit(k); }
    if (event.shiftKey && lower === 'c') { done(); actions.color(-1); return true; }
    if (event.shiftKey) return false;
    switch (lower) {
      case 'i': done(); actions.insertNext(); return true;
      case 'n': done(); actions.insertNew(); return true;
      case 'l': done(); actions.toggleLoop(); return true;
      case 'p': done(); actions.playFrom(); return true;
      case 'q': done(); actions.queueFrom(); return true;
      case 'e': done(); actions.rename(); return true;
      case 'c': done(); actions.color(1); return true;
      case 'f': done(); actions.toggleFollow(); return true;
      default: return false;
    }
  }

  function paintSelection() {
    const listNode = document.getElementById('sequence-list'); if (!listNode) return;
    const current = selectedBlock(), [a, b] = range();
    document.querySelectorAll('[data-song-block]').forEach(node => {
      const index = Number(node.dataset.songBlock), selected = index >= a && index <= b;
      node.classList.toggle('selected', selected); node.classList.toggle('current', index === current); node.setAttribute('aria-selected', String(selected));
    });
    listNode.setAttribute('aria-activedescendant', `song-block-${current}`);
    const block = blockList()[current], select = document.getElementById('song-block-pattern'), repeats = document.getElementById('song-block-repeats');
    if (select && block && document.activeElement !== select) select.value = String(block.pattern);
    if (repeats && block && document.activeElement !== repeats) repeats.value = String(block.count);
    app().updateTransport?.();
  }

  function paintPlayhead(payload = state().status || {}) {
    if (state().view !== 'song') return;
    const p = project(), playing = !!(payload.playing && payload.songMode && Number.isInteger(payload.sequence));
    const list = blockList(), at = playing ? M().blockAt(p.sequence, payload.sequence) : -1;
    document.querySelectorAll('[data-song-block]').forEach(node => {
      const index = Number(node.dataset.songBlock), active = index === at;
      node.classList.toggle('playing', active);
      const progress = node.querySelector?.('.song-progress');
      if (progress) progress.textContent = active && list[index].count > 1 ? `${payload.sequence - list[index].start + 1}/${list[index].count}` : '';
    });
    if (playing && at !== local.playing && local.follow && !state().modal) {
      const focusInside = document.activeElement?.closest?.('.song-seq-tools');
      if (!focusInside) { local.anchor = null; state().sequence = list[at].start; paintSelection(); document.getElementById(`song-block-${at}`)?.scrollIntoView?.({ block: 'nearest' }); }
    }
    local.playing = at;
  }

  let fieldScope = null, pointerHeld = false, fieldPointerBound = false;
  function bindFields() {
    const scope = fieldScope = {project:project(),list:document.getElementById('sequence-list'),drafts:new Set(),pending:false,timer:null};
    const paint = () => {
      scope.pending = true; clearTimeout(scope.timer);
      scope.timer = setTimeout(() => {
        if (scope !== fieldScope || project() !== scope.project || state().view !== 'song' || state().modal || pointerHeld || scope.drafts.has(document.activeElement)) return;
        scope.pending = false; const focused = document.activeElement?.id;
        app().refresh(); if (focused) document.getElementById(focused)?.focus({preventScroll:true});
      },0);
    };
    scope.paint = paint;
    if (!fieldPointerBound && document.addEventListener) {
      fieldPointerBound = true;
      document.addEventListener('pointerdown', () => {pointerHeld=true;}, true);
      const release = () => {pointerHeld=false;if(fieldScope?.pending)fieldScope.paint();};
      for (const type of ['pointerup','pointercancel']) document.addEventListener(type, release, true);
      window.addEventListener?.('blur', release);
    }
    for (const id of ['song-block-pattern','song-block-repeats']) {
      const field = document.getElementById(id); if (!field) continue;
      field.oninput = () => scope.drafts.add(field);
      field.onblur = () => {scope.drafts.delete(field);if(scope.pending)paint();};
    }
    return {change(field, action) { if (scope !== fieldScope || project() !== scope.project || state().modal) return; scope.drafts.delete(field); if (action() !== false) paint(); }};
  }
  function bindPanel() {
    const fields = bindFields(), fieldOptions = {focus:false,render:false};
    const on = (id, type, fn) => { const node = document.getElementById(id); if (node) node[type] = fn; };
    document.querySelectorAll('[data-song-block]').forEach(node => {
      node.onclick = event => selectBlock(Number(node.dataset.songBlock), { extend: event.shiftKey });
      node.ondblclick = () => { selectBlock(Number(node.dataset.songBlock)); actions.editPattern(); };
    });
    on('song-follow', 'onclick', () => actions.toggleFollow());
    on('song-insert', 'onclick', () => actions.insertNext());
    on('song-new', 'onclick', () => actions.insertNew());
    on('song-clone', 'onclick', () => actions.clone());
    on('song-rename', 'onclick', () => actions.rename());
    on('song-loop', 'onclick', () => actions.toggleLoop());
    on('song-play-from', 'onclick', () => actions.playFrom());
    on('song-block-pattern', 'onchange', event => fields.change(event.target, () => actions.setPatternId(Number(event.target.value), fieldOptions)));
    on('song-block-repeats', 'onchange', event => { const value = Math.trunc(Number(event.target.value)); if (Number.isFinite(value) && value >= 1 && value <= M().MAX_REPEATS) fields.change(event.target, () => actions.repeats(0, value, fieldOptions)); else { app().toast(`Enter a repeat count from 1 to ${M().MAX_REPEATS}.`, 'error'); event.target.value = String(blockList()[selectedBlock()].count); } });
    const repeats = document.getElementById('song-block-repeats');
    if (repeats) repeats.onkeydown = event => { if (event.key === 'Enter') { event.preventDefault(); repeats.dispatchEvent(new Event('change')); } };
    // The view title's "+ Add pattern" inserts the active pattern after the selected block, never inside a repeat run.
    const add = document.getElementById('sequence-add');
    if (add) add.onclick = () => actions.insertPattern(project().patterns.some(item => item.id === project().activePattern) ? project().activePattern : blockList()[selectedBlock()].pattern);
    paintPlayhead();
    registerContextMenu();
  }

  /* Replace the legacy sequence panel rendered by app.js (until app.js renders renderPanel() itself). */
  function mount() {
    if (!M()) return false;
    const list = document.getElementById('sequence-list');
    if (!list) return false;
    if (!list.dataset?.songArranger) {
      const panel = list.closest?.('.panel');
      if (!panel || !('outerHTML' in panel)) return false;
      panel.outerHTML = renderPanel();
    }
    bindPanel();
    return true;
  }

  function contextItems({ target } = {}) {
    const node = target?.closest?.('[data-song-block]') || (target?.closest?.('#sequence-list') ? document.getElementById(`song-block-${selectedBlock()}`) : null);
    if (!node || state().view !== 'song') return null;
    const index = Number(node.dataset.songBlock), [a, b] = range();
    if (index < a || index > b) selectBlock(index, { focus: false });
    const p = project(), list = blockList(), block = list[selectedBlock()], pattern = patternOf(block.pattern), loop = M().loopRange(p);
    const [ra, rb] = range(), multi = rb > ra;
    return [
      { header: true, title: multi ? `Blocks ${ra + 1}–${rb + 1}` : `Block ${String(selectedBlock() + 1).padStart(2, '0')} · ${pattern?.name || 'Missing pattern'}` },
      { title: 'Play song from here', key: 'P', action: () => actions.playFrom() },
      { title: 'Play from here at pattern end', key: 'Q', enabled: !!(state().playing && state().status?.songMode), action: () => actions.queueFrom() },
      { title: 'Edit pattern', key: '↩', enabled: !!pattern, action: () => actions.editPattern() },
      { title: 'Rename pattern…', key: 'E', enabled: !!pattern, action: () => actions.rename() },
      { title: 'Pattern', items: p.patterns.map((item, number) => ({ title: `${String(number + 1).padStart(2, '0')} · ${item.name}`, checked: item.id === block.pattern, action: () => actions.setPatternId(item.id) })) },
      { title: 'Repeat', items: [1, 2, 3, 4, 8, 16].map(count => ({ title: `×${count}`, checked: block.count === count, action: () => actions.repeats(0, count) })) },
      { title: 'Next colour', key: 'C', action: () => actions.color(1) },
      { separator: true },
      { title: 'Insert next pattern after', key: 'I', action: () => actions.insertNext() },
      { title: 'Insert new empty pattern', key: 'N', action: () => actions.insertNew() },
      { title: multi ? 'Duplicate selected blocks' : 'Duplicate block', key: '⌘D', action: () => actions.duplicate() },
      { title: 'Clone as new pattern', key: '⇧⌘D', action: () => actions.clone() },
      { title: 'Move up', key: '⌥↑', enabled: ra > 0, action: () => actions.move(-1) },
      { title: 'Move down', key: '⌥↓', enabled: rb < list.length - 1, action: () => actions.move(1) },
      { separator: true },
      { title: multi ? 'Loop selected blocks' : 'Loop this block', key: 'L', action: () => actions.toggleLoop() },
      loop ? { title: 'Clear song loop', action: () => actions.clearLoop() } : null,
      { separator: true },
      { title: multi ? 'Remove blocks' : 'Remove block', key: '⌫', enabled: list.length > rb - ra + 1, action: () => actions.remove() }
    ];
  }
  function registerContextMenu() {
    if (local.contextRegistered || !window.STContextMenu?.register) return;
    local.contextRegistered = true;
    window.STContextMenu.register(context => contextItems(context), 60);
  }

  /* Palette-friendly commands; each acts on the selected block from any view. */
  function command(name) {
    if (state().view !== 'song') app().setView('song');
    const map = { playFrom: () => actions.playFrom(), queueFrom: () => actions.queueFrom(), loop: () => actions.toggleLoop(), clearLoop: () => actions.clearLoop(), insert: () => actions.insertNext(), insertNew: () => actions.insertNew(), duplicate: () => actions.duplicate(), clone: () => actions.clone(), remove: () => actions.remove(), rename: () => actions.rename(), repeatUp: () => actions.repeats(1), repeatDown: () => actions.repeats(-1), follow: () => actions.toggleFollow(), color: () => actions.color(1) };
    return map[name] ? map[name]() : false;
  }

  function event(type, payload = {}) {
    if (type === 'status' || type === 'transport.status') paintPlayhead(payload);
    if (type === 'project.loaded') { local.anchor = null; local.playing = -1; }
  }

  window.STSong = { renderPanel, mount, key, event, contextItems, command, actions, selectBlock, get follow() { return local.follow; } };
})();
