/* Pattern view: the tracker table, the step editor below it, recorded-cell repaints and
   the playhead. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, TRACK_SANITY_LIMIT, esc, noteNames, noteText, FX, state, pattern, beatsPerBar,
    currentInstrument, refresh, updateTransport, trackSilenced, fxText, patternColumns,
    patternGroupHeaders, patternColumnHeader, patternColumnCell, patternLayout, showsGroupRow
  } = app;

  function renderPatternTable(p, pat, sel, loopRange, silenced, columns) {
    const layout = patternLayout(columns),
      segments = layout?.segments || [],
      // Collapsed group overviews keep their own width (app.patternColumnWidth); track columns share the rest of a
      // wide window. Without any track column a filler column takes the rest, so an overview never stretches.
      fill = !layout && columns.length > 0 && columns.every(column => column.collapsed || column.slim),
      filler = tag => fill ? `<${tag} class="tracker-fill" aria-hidden="true"></${tag}>` : '';
    // One grid row of the active pattern; `attrs` and `live: false` serve the song scroll's context rows (app-song-scroll.js).
    const row = (r, attrs = '', live = true) => `<tr data-row="${r}"${attrs} class="${r % (p.linesPerBeat * 4 / p.timeSignature.denominator) === 0 ? 'beat' : ''}`
        + ` ${r % (p.linesPerBeat * beatsPerBar(p)) === 0 ? 'bar' : ''}${live && loopRange && r >= loopRange.start && r <= loopRange.end
            ? ' loop-row' + (r === loopRange.start ? ' loop-start' : '') + (r === loopRange.end ? ' loop-end' : '')
            : ''}">`
        + `<td class="row-number">${String(r).padStart(2, '0')}</td>${layout
            ? segments.map(x => x.spacer
                ? '<td class="tracker-spacer" aria-hidden="true"></td>'
                : patternColumnCell(columns[x.index], r, live ? sel : null, silenced)).join('')
            : columns.map(column => patternColumnCell(column, r, live ? sel : null, silenced)).join('') + filler('td')}</tr>`;
    const width = 1 + (layout ? segments.length : columns.length + (fill ? 1 : 0)),
      body = app.songScrollRows?.(row, width) ?? Array.from({ length: pat.length }, (_, r) => row(r)).join('');
    return `<table class="tracker${layout ? ' is-virtual' : ''}" id="tracker" tabindex="0" data-gate-column="${app.gateColumnShown() ? 'shown' : 'hidden'}"${layout ? ` data-virtual-columns="${columns.length}" style="width:${40 + layout.total}px"` : ''}`
        + ` aria-label="Pattern grid. Arrows move, computer keys enter notes when recording is armed, Delete clears.">${layout
            ? `<colgroup><col style="width:40px">${segments.map(x => `<col style="width:${x.width}px">`).join('')}</colgroup>`
            : `<colgroup><col style="width:40px">${columns.map(column => column.collapsed
                ? `<col class="group-overview-col" style="width:${app.patternColumnWidth(column)}px">`
                : column.slim ? `<col class="track-slim-col" style="width:${app.patternColumnWidth(column)}px">` : '<col>').join('')}${fill ? '<col>' : ''}</colgroup>`}<thead>${showsGroupRow(p)
            ? '<tr class="pattern-groups"><th></th>'
                + patternGroupHeaders(columns, layout ? segments : null)
                + filler('th') + '</tr>'
            : ''}<tr>`
        + `<th>STEP</th>${layout
            ? segments.map(x => x.spacer
                ? '<th class="tracker-spacer" aria-hidden="true"></th>'
                : patternColumnHeader(columns[x.index])).join('')
            : columns.map(patternColumnHeader).join('') + filler('th')}</tr>`
        + `</thead><tbody>${body}</tbody></table>`;
  }

  function renderPattern() {
    const p = state.project,
      pat = pattern(),
      sel = app.selectedBounds(),
      loopRange = app.patternLoopFor(pat),
      silenced = p.tracks.map((_, i) => trackSilenced(p, i)),
      columns = patternColumns();
    app.rememberRendered?.();
    return `<div class="workspace has-left-pane">${app.leftPane()}<section class="workspace-main">`
        + `<div class="view-title"><div><div class="eyebrow">Pattern editor</div><h1 style="margin-top:7px">${esc(pat.name)}</h1>`
        + `</div><div class="row">${app.patternLoopButton()}<button class="quiet" id="pattern-rename"`
        + ` title="Rename pattern">Rename</button><button id="pattern-duplicate">Duplicate</button>`
        + `<button id="track-duplicate" data-duplicate-track title="Duplicate the selected track with independent instruments, `
        + `plug-ins and automation" ${p.tracks.length >= TRACK_SANITY_LIMIT || window.STBusy.isBusy('trackDuplicate') ? 'disabled' : ''}`
        + ` aria-busy="${window.STBusy.isBusy('trackDuplicate') ? 'true' : 'false'}">Duplicate track</button>`
        + `<button id="track-add">+ Track</button><button id="group-create" title="Group the selected tracks with a group bus · ⌘G">Group</button>`
        + `<button id="kit-create" title="Browse kits (⌘3 → Kits & Groups) or start an empty kit group">+ Kit</button>`
        + `</div></div>${window.STPatternDestination?.render() || ''}<div class="toolbar pattern-toolbar">`
        + `<div class="toolbar-group pattern-group" role="group" aria-label="Pattern settings">`
        + `<span class="toolbar-caption" aria-hidden="true">Pattern</span><label for="pattern-length">Length</label>`
        + `<input id="pattern-length" type="number" min="1" max="256" value="${pat.length}" style="width:52px"`
        + ` title="Pattern length · ⌥D double · ⌥⇧D halve"><label for="edit-step" title="Cursor advance after entering `
        + `a note · { and } change it">Step</label><select id="edit-step" title="Edit step · { / }">${[0, 1, 2, 3, 4, 8, 16].map(x => `<option value="${x}" ${x === state.editStep ? 'selected' : ''}>${x}</option>`).join('')}</select>`
        + `<button id="follow" class="${state.follow ? 'active' : ''}" aria-pressed="${state.follow}"`
        + ` title="Follow playback · ⌥F">Follow</button>${app.songScrollButton()}<button id="auto-panel-toggle" class="${app.automationPanelShown() ? 'active' : ''}"`
        + ` aria-pressed="${app.automationPanelShown()}" title="Automation panel under the grid · ⌥⇧A">Automation</button><button id="fill" title="Repeat selection · ⌘D">Fill ↓</button>`
        + `<button id="gate-column" class="${app.gateColumnShown() ? 'active' : ''}" aria-pressed="${app.gateColumnShown()}"`
        + ` title="Show the Gate column: an optional note length in rows. Without a gate a note plays until the next note,`
        + ` OFF or CUT on its track · ⌘K “Gate column”">Gate</button>`
        + `${app.hideEmptyControls?.() || ''}${window.STGroups?.list(p).length ? app.padKeysButton() : ''}`
        + `</div><div class="toolbar-group perf-group ${app.perfToolbarOpen ? 'is-open' : ''} ${app.performanceIsDefault() ? '' : 'is-custom'}"`
        + ` role="group" aria-label="Keyboard performance"><button type="button" class="perf-chip" id="perf-toggle"`
        + ` aria-expanded="${app.perfToolbarOpen}" aria-controls="perf-controls" title="Keyboard performance: octave, `
        + `key, scale, chord, chord set, MIDI, repeat and velocity · click to ${app.perfToolbarOpen ? 'collapse' : 'expand'}">`
        + `<span class="perf-chip-label">Play</span><span class="perf-summary" id="perf-summary">${esc(app.performanceSummary())}</span>`
        + `<span class="perf-chip-caret" aria-hidden="true">${app.perfToolbarOpen ? '‹' : '›'}</span>`
        + `</button><div class="perf-controls" id="perf-controls" ${app.perfToolbarOpen ? '' : 'hidden'}>`
        + `<label for="octave">Oct</label><select id="octave" title="Octave · [ / ]">${[0, 1, 2, 3, 4, 5, 6, 7, 8].map(x => `<option ${x === state.octave ? 'selected' : ''}>${x}</option>`).join('')}</select>`
        + `<label for="root">Key</label><select id="root" aria-label="Scale root" title="Root · ⌥T / ⌥⇧T">${noteNames.map((n, i) => `<option value="${i}" ${state.root === i ? 'selected' : ''}>${n}</option>`).join('')}</select>`
        + `<select id="scale" aria-label="Scale" title="Scale · ⌥K / ⌥⇧K">${Object.keys(app.scales).map(k => `<option value="${k}" ${state.scale === k ? 'selected' : ''}>${app.scaleNames[k] || k}</option>`).join('')}</select>`
        + `<select id="scale-mode" aria-label="Scale keyboard mode" title="Snap: out-of-key notes move up into the scale. `
        + `Degrees: each key plays the next scale note. ⌥L" ${state.scale === 'chromatic' ? 'disabled' : ''}>`
        + `<option value="snap" ${state.scaleMode !== 'degrees' ? 'selected' : ''}>Snap to key</option>`
        + `<option value="degrees" ${state.scaleMode === 'degrees' ? 'selected' : ''}>Scale degrees</option>`
        + `</select><label for="chord">Chord</label><select id="chord" title="Chord · ⌥C / ⌥⇧C">`
        + `<optgroup label="Chords">${Object.entries(app.chords).filter(([, v]) => !v.degrees).map(([k, v]) => `<option value="${k}" ${state.chord === k ? 'selected' : ''}>${v.name}</option>`).join('')}</optgroup>`
        + `<optgroup label="Chords in key">${Object.entries(app.chords).filter(([, v]) => v.degrees).map(([k, v]) => `<option value="${k}" ${state.chord === k ? 'selected' : ''}>${v.name}</option>`).join('')}</optgroup>`
        + `</select><label for="chord-set">Set</label><select id="chord-set" title="Chord set: each key plays the next `
        + `chord · Key transposes, Scale is ignored · ⌥G next · `
        + `⌥⇧G previous"><option value="" ${app.activeChordSet() ? '' : 'selected'}>Off</option>${[['Major harmony', false, 'harmony'],
            ['Minor harmony', true, 'harmony'], ['Major style', false, 'style'], ['Minor style', true, 'style']].map(([label, minor, group]) => `<optgroup label="${label} sets">${app.chordSets.filter(set => set.minor === minor && set.group === group).map(set => `<option value="${set.id}" ${state.chordSet === set.id ? 'selected' : ''}>${esc(set.name)} · ${esc(set.mood)}</option>`).join('')}</optgroup>`).join('')}</select>`
        + `<label class="midi-apply" title="Apply key, scale, chord and chord set to notes from the MIDI keyboard">`
        + `<input type="checkbox" id="midi-apply" ${state.midiApply !== false ? 'checked' : ''}>MIDI</label>${window.STNoteRepeat ? window.STNoteRepeat.renderControls() : ''}<label `
        + `for="key-velocity">Vel</label><select id="key-velocity" title="Computer-keyboard velocity · ⌥V">${app.keyVelocities.map(v => `<option ${v === state.keyVelocity ? 'selected' : ''}>${v}</option>`).join('')}</select>`
        + `</div></div></div><div class="tracker-wrap" data-scroll="tracker">${renderPatternTable(p, pat, sel, loopRange, silenced, columns)}</div>${app.chordSetStrip()}`
        // The automation panel takes the hint line's place (the help sheet lists the same keys).
        + `${app.automationPanel() || `<div class="pattern-foot"><span><strong>${String(state.row).padStart(2, '0')}</strong> / ${String(pat.length - 1).padStart(2, '0')}`
        + ` steps</span><span>${esc(currentInstrument().name)}</span><span class="grow">`
        + `</span><span>Enter edit FX / hear note</span><span title="Note column: 1 inserts a note-off (release), ⇧1 a note cut">1 OFF · ⇧1 CUT</span>`
        + `<span>⌃Tab exit grid</span><span>⇧ Arrows select</span>`
        + `<span>⌘C / ⌘V</span><span>⌥ ↑↓ transpose</span></div>`}${renderStepEditor()}</section></div>`;
  }

  // Repaint recorded cells without replacing the grid, focus, or an active pointer target.
  function patternGateText(step) {
    return step && step.note >= 0 ? app.gateText(step) : '···';
  }

  function renderPatternCellContent(s, t, i, active) {
    if ((app.noteColumnCount?.(i) || 1) > 1) return renderNoteColumnsContent(s, t, i, active);
    return `${[noteText(s?.note),
        s ? String(s.instrument ?? t.instrument).padStart(2, '0') : '··',
        s ? String(s.velocity ?? 100).padStart(3, '0') : '···', fxText(s?.fx1), fxText(s?.fx2)].map((v, c) => `<span data-column="${c}"${c >= 3 && s?.['fx' + (c - 2)] && app.fxTitle
            ? ` title="${esc(app.fxTitle(s['fx' + (c - 2)]))}"` : ''} class="${s ? c === 0 ? 'has-note' : c === 1 ? 'has-instrument' : c === 2 ? 'has-velocity' : 'has-fx' : ''}`
        + ` ${active && state.column === c ? 'active-column' : ''}">${esc(v)}</span>`).join('')}${app.gateColumnShown() ? `<span `
        + `class="pattern-gate ${app.stepGate(s) !== null ? 'has-gate' : ''}" data-gate-inspector title="Gate: ${s && s.note >= 0
            ? (app.stepGate(s) === null ? 'none, plays until the next note, OFF or CUT' : patternGateText(s) + ' pattern rows') + ' · click to edit'
            : 'select this row and edit Gate below'}">${patternGateText(s)}</span>` : ''}`;
  }

  /* A track with several note columns: NOTE INS VEL of each column, FX 1, FX 2, then each column's GATE. Column 0 of
     a row that only holds extra-column notes shows dots. */
  function renderNoteColumnsContent(s, t, i, active) {
    const count = app.noteColumnCount(i), here = active ? app.cursorNoteColumn(i) : -1, gate = app.gateColumnShown();
    const span = (n, c, text, cls, extra = '') => `<span data-column="${c}"${n ? ` data-note-column="${n}"` : ''}${extra} class="${cls}`
        + `${n ? ' note-column-cell' : ''} ${here === n && state.column === c ? 'active-column' : ''}">${esc(text)}</span>`;
    let html = '';
    for (let n = 0; n < count; n++) {
      const cell = n ? app.noteCell(s, n) : app.isColumnHolder(s) ? null : s, pitched = n ? cell?.note >= 0 : !!cell;
      html += span(n, 0, noteText(cell?.note), cell ? 'has-note' : '')
          + span(n, 1, pitched ? String(cell.instrument ?? t.instrument).padStart(2, '0') : '··', pitched ? 'has-instrument' : '')
          + span(n, 2, pitched ? String(cell.velocity ?? 100).padStart(3, '0') : '···', pitched ? 'has-velocity' : '');
    }
    for (const c of [3, 4]) {
      const fx = s?.['fx' + (c - 2)];
      html += span(0, c, fxText(fx), s && !app.isColumnHolder(s) ? 'has-fx' : '', fx && app.fxTitle ? ` title="${esc(app.fxTitle(fx))}"` : '');
    }
    if (gate) for (let n = 0; n < count; n++) {
      const cell = n ? app.noteCell(s, n) : s, step = n ? cell && { gate: cell.gate ?? 0, note: cell.note } : cell;
      html += `<span class="pattern-gate${n ? ' note-column-cell' : ''} ${app.stepGate(step) !== null ? 'has-gate' : ''}" data-gate-inspector`
          + `${n ? ` data-note-column="${n}"` : ''} title="Gate${n ? ` of note column ${n + 1}` : ''}: ${step && step.note >= 0
            ? (app.stepGate(step) === null ? 'none, plays until the next note, OFF or CUT' : patternGateText(step) + ' pattern rows') + ' · click to edit'
            : 'select this row and edit Gate below'}">${patternGateText(step)}</span>`;
    }
    return html;
  }

  function paintRecordedCell(pat, track, row) {
    if (state.view !== 'pattern' || pattern() !== pat) return;
    // A note on a track hidden as empty (app-track-lanes.js) brings it back: the grid renders again.
    if (app.wasHidden?.(track)) return refresh();
    const group = window.STGroups?.ofTrack(state.project, track);
    if (group?.collapsed) {
      const column = patternColumns().find(c => c.collapsed && c.group.id === group.id),
        node = column && $(`[data-cell="${column.index}:${row}"]`);
      if (node) {
        const html = patternColumnCell(column, row, app.selectedBounds(), state.project.tracks.map((_, i) => trackSilenced(state.project, i)));
        node.innerHTML = html.slice(html.indexOf('>') + 1, html.lastIndexOf('</td>'));
      }
      return;
    }
    const cell = $(`[data-cell="${track}:${row}"]`);
    if (!cell) return;
    if (app.isCollapsed?.(track)) {
      const html = app.slimCell({ index: track }, row, app.selectedBounds(), state.project.tracks.map((_, i) => trackSilenced(state.project, i)));
      cell.innerHTML = html.slice(html.indexOf('>') + 1, html.lastIndexOf('</td>'));
      return;
    }
    const step = pat.steps[track]?.[row],
      t = state.project.tracks[track];
    const content = cell.querySelector?.('.tracker-cell');
    if (!content) return;
    content.innerHTML = renderPatternCellContent(step, t, track, state.track === track && state.row === row);
    const chord = step?.chord?.length > 1;
    cell.classList.toggle('has-chord', !!chord);
    if (chord) {
      cell.setAttribute('title', 'Chord ' + step.chord.map(x => noteText(clamp(step.note + x, 0, 127))).join(' '));
    } else {
      cell.removeAttribute('title');
    }
  }

  function renderStepEditor() {
    // Note, Ins, Vel and Gate edit the cursor's note column; the FX belong to the row (column 0).
    const s = pattern().steps[state.track]?.[state.row], n = app.cursorNoteColumn?.() || 0,
      cell = n ? app.noteCell(s, n) && { ...app.noteCell(s, n), gate: app.noteCell(s, n).gate ?? 0 } : app.isColumnHolder?.(s) ? null : s;
    return `<div class="step-editor"><span class="step-heading">STEP ${String(state.row).padStart(2, '0')}${n ? ` · N${n + 1}` : ''}</span><label>Note `
        + `<input id="step-note" type="number" min="-3" max="127" title="MIDI note, -1 off, -2 cut, -3 ${n ? 'empty' : 'effects only'}"`
        + ` value="${esc(cell?.note ?? 60)}"></label><label>Ins <input id="step-instrument" type="text" list="step-instrument-list" autocomplete="off"`
        + ` spellcheck="false" size="10" aria-label="Step instrument: type its number or the start of its name"`
        + ` title="Type the instrument number (03) or the start of its name; Enter confirms, Esc cancels"`
        + ` value="${esc(app.instrumentText(cell?.instrument ?? state.instrument))}">${app.instrumentDatalist()}`
        + `</label><label>Vel `
        + `<input id="step-velocity" type="number" min="1" max="127" value="${esc(cell?.velocity ?? 100)}">`
        + `</label>${renderGateField(cell)}${app.codeDatalist()}${[1, 2].map(n => app.renderCodeField(n, s?.['fx' + n]) + (window.STStepFX ? window.STStepFX.renderField(n, s?.['fx' + n]) : `<label>FX ${n}<select id="step-fx${n}"><option value="">None</option>${Object.keys(FX).map(f => `<option value="${f}" ${s?.['fx' + n]?.type === f ? 'selected' : ''}>${f}</option>`).join('')}</select>`
        + `<input id="step-fx${n}-value" aria-label="Effect ${n} value" type="number" step="any" value="${s?.['fx' + n]?.value ?? .5}">`
        + `</label>`)).join('')}<button class="quiet" id="step-off" title="Note off: the track's notes release `
        + `with their envelope · 1 in the note column">OFF</button><button class="quiet" id="step-cut" title="Note cut: `
        + `the track's notes stop at once · ⇧1 in the note column">CUT</button><button `
        + `class="quiet" id="step-clear" style="font-size:11px">Clear</button></div>`;
  }

  // Gate: optional note length in rows. "—" is no gate (the note plays until the next note, OFF or CUT on its
  // track). Editable while the Gate column is shown; an empty field removes the gate.
  function renderGateField(s) {
    const gate = app.stepGate(s), shown = app.gateColumnShown();
    if (!shown) {
      const meaning = gate === null ? 'none, the note plays until the next note, OFF or CUT on its track' : gate + ' pattern rows';
      return `<span class="step-gate-label step-gate-readout" title="Gate: ${meaning} · show the Gate column to edit">Gate `
          + `<output id="step-gate-readout">${s && s.note >= 0 ? app.gateText(s) : '—'}</output></span>`;
    }
    return `<label class="step-gate-label" title="Note length in pattern rows. 1 holds one row; 4 holds four rows. `
        + `Empty (—) is no gate: the note plays until the next note, OFF or CUT on its track.">Gate (rows) <input `
        + `id="step-gate" aria-label="Selected note gate in pattern rows, empty for none" type="number" min="0" max="256"`
        + ` step=".05" placeholder="—" value="${gate ?? ''}"></label>`;
  }

  function updatePlayhead(s) {
    // Song scroll follows the playing slot, also through repeats of one pattern (app-song-scroll.js).
    if (app.songScrollFollow?.(s)) return;
    if (s.playing
        && state.follow
        && !(state.recording && state.recordPlacement !== 'live')
        && state.view === 'pattern'
        && s.pattern !== undefined
        && s.pattern !== pattern().id
        && state.project.patterns.some(p => p.id === s.pattern)) {
      state.project.activePattern = s.pattern;
      const last = pattern().length - 1;
      state.row = clamp(state.row, 0, last);
      if (state.selection) state.selection.row = clamp(state.selection.row, 0, last);
      refresh();
      return;
    }
    for (const tr of $$('tr.playhead')) tr.classList.remove('playhead');
    if (s.playing && (s.pattern === pattern().id || app.songScrollOn?.() && s.songMode)) {
      const tr = $(app.songScrollOn?.() && s.songMode ? `#tracker tr[data-slot="${s.sequence}"][data-row="${s.row}"]` : `#tracker tr[data-row="${s.row}"]:not([data-context])`);
      tr?.classList.add('playhead');
      if (state.follow && tr) tr.scrollIntoView({ block: 'nearest' });
    }
    app.paintAutomationPanelPlayhead(s);
    app.paintMatrixPlayhead(s);
    updateTransport();
  }

  Object.assign(app, {
    renderPattern, patternGateText, renderPatternCellContent, paintRecordedCell, updatePlayhead
  });
})();
