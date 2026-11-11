/* Note columns: a track can hold up to eight notes per row, like Renoise note columns, on every track type.
   Column 0 is the step itself (pattern.steps[track][row]: note, instrument, velocity, gate, chord, FX 1 / FX 2).
   Extra columns live on the step as step.columns: [{ note, instrument?, velocity?, gate? } | null, …], where
   columns[i] is note column i + 1; a row with notes only in extra columns keeps a holder step { note: -3, columns }.
   A track's count is track.noteColumns (2..8, absent for 1). Each column is its own voice lane in the engine:
   a new note or an OFF / CUT in a column ends only that column's notes. The row's FX belong to column 0 and apply
   to every column's note.

   The cursor's note column is state.noteColumn (0 = the step); state.column 0..2 is NOTE / INS / VEL within it,
   3 / 4 are FX 1 / FX 2 (always column 0). Left / Right walk NOTE INS VEL of each note column, then FX 1, FX 2. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clamp, esc, state, pattern, toast, mutate, refresh } = app;
  const MAX_NOTE_COLUMNS = 8;

  /* ---------- model ---------- */

  const trackOf = (track, p = state.project) => typeof track === 'number' ? p?.tracks[track] : track;
  const noteColumnCount = (track = state.track, p = state.project) =>
    clamp(Math.round(Number(trackOf(track, p)?.noteColumns) || 1), 1, MAX_NOTE_COLUMNS);
  const noteCell = (step, column = 0) => column ? step?.columns?.[column - 1] || null : step || null;
  const hasColumnNotes = step => !!step?.columns?.some(Boolean);
  // A step with no note, no effect and no extra-column note is empty.
  const isEmptyStep = step => !step || step.note === -3 && !step.fx1 && !step.fx2 && !hasColumnNotes(step);
  const dropEmptyStep = step => isEmptyStep(step) ? null : step;
  // Column 0 shows nothing on a holder row (a step that only carries extra-column notes).
  const isColumnHolder = step => !!step && step.note === -3 && !step.fx1 && !step.fx2 && hasColumnNotes(step);

  // Every note cell of a step that plays a pitch: [{ cell, column }].
  function playedCells(step) {
    const out = [];
    if (step?.note >= 0) out.push({ cell: step, column: 0 });
    (step?.columns || []).forEach((cell, i) => { if (cell?.note >= 0) out.push({ cell, column: i + 1 }); });
    return out;
  }

  // Trailing empty columns go; an empty columns list goes; an empty holder step becomes null.
  function tidyStep(step) {
    if (!step) return null;
    if (step.columns) {
      const columns = step.columns.map(cell => cell && Number.isFinite(cell.note) && cell.note >= -2 && cell.note <= 127 ? cell : null);
      while (columns.length && !columns.at(-1)) columns.pop();
      if (columns.length) step.columns = columns; else delete step.columns;
    }
    return dropEmptyStep(step);
  }

  /* Write note column `column` (> 0) of steps[row]; null clears it. Returns the step. An existing step is edited in
     place, so column 0 (and live recording that holds it, app-live-record.js) keeps its identity while other columns
     change. Call inside mutate(). */
  function setColumnCell(steps, row, column, cell) {
    if (!column) return steps[row] = dropEmptyStep(cell ? { ...cell, ...(steps[row]?.columns ? { columns: steps[row].columns } : {}) } : null);
    const prior = steps[row], columns = [...(prior?.columns || [])];
    while (columns.length < column) columns.push(null);
    columns[column - 1] = cell ? Object.fromEntries(['note', 'instrument', 'velocity', 'gate'].filter(key => cell[key] !== undefined).map(key => [key, cell[key]])) : null;
    const step = prior || { note: -3 };
    step.columns = columns;
    return steps[row] = tidyStep(step);
  }
  // What a note column holds, as text for change checks: an extra column's cell, or column 0 without the other columns.
  function cellValue(step, column = 0) {
    if (column) return JSON.stringify(noteCell(step, column));
    if (!step?.columns) return JSON.stringify(step ?? null);
    const { columns, ...rest } = step;
    return JSON.stringify(rest);
  }

  // Clear what the cursor addresses on one row: an extra column's cell, or column 0 (its note and FX; extra columns stay).
  function clearCell(steps, row, column) {
    const prior = steps[row];
    if (!prior) return;
    if (column) return setColumnCell(steps, row, column, null);
    steps[row] = prior.columns ? tidyStep({ note: -3, columns: prior.columns }) : null;
  }

  // A previewable step for a cell: the cell's note with the row's FX (slice, offset, pitch …).
  function cellStep(step, column = 0) {
    if (!column) return step;
    const cell = noteCell(step, column);
    if (!cell) return null;
    // A cell without a gate key has no gate (a step without one is DEFAULT_GATE rows long: app-project-model.js).
    const out = { ...cell, gate: cell.gate ?? 0 };
    for (const key of ['fx1', 'fx2']) if (step?.[key]) out[key] = step[key];
    return out;
  }

  // The note column under the cursor (0 on FX columns, or when the track has one note column).
  const cursorNoteColumn = (track = state.track) => state.column > 2 ? 0 : clamp(Math.round(Number(state.noteColumn) || 0), 0, noteColumnCount(track) - 1);
  const cursorCell = (pat = pattern(), track = state.track, row = state.row) => noteCell(pat?.steps[track]?.[row], cursorNoteColumn(track));

  // Note columns with a note (or OFF / CUT) anywhere in the song, beyond the first `keep` columns of a track.
  function columnsInUse(p, track, keep) {
    let count = 0;
    for (const pat of p.patterns) for (const step of pat.steps[track]) {
      (step?.columns || []).forEach((cell, i) => { if (cell && i + 1 >= keep) count++; });
    }
    return count;
  }

  /* Normalization (project load, app.normalize): clamp noteColumns, drop cells beyond a track's count, tidy holders.
     A project without note columns is left exactly as it is. */
  function normalizeNoteColumns(p) {
    const counts = p.tracks.map(track => {
      if (track.noteColumns === undefined) return 1;
      const count = clamp(Math.round(Number(track.noteColumns) || 1), 1, MAX_NOTE_COLUMNS);
      if (count > 1) track.noteColumns = count; else delete track.noteColumns;
      return count;
    });
    for (const pat of p.patterns) pat.steps.forEach((rows, t) => {
      rows.forEach((step, r) => {
        if (!step || step.columns === undefined) return;
        if (!Array.isArray(step.columns)) delete step.columns;
        else step.columns = step.columns.slice(0, (counts[t] || 1) - 1);
        rows[r] = tidyStep(step);
      });
    });
    return p;
  }

  /* Steps pasted or moved onto a track keep their extra note columns: the track grows to show them (inside the same
     edit, so one Undo step). Cells beyond eight columns cannot exist. */
  function growNoteColumns(p, index, step) {
    const track = p.tracks[index], needed = Math.min(MAX_NOTE_COLUMNS, (step?.columns?.length || 0) + 1);
    if (!track || !hasColumnNotes(step) || needed <= noteColumnCount(track)) return;
    track.noteColumns = needed;
  }

  /* ---------- adding and removing note columns ---------- */

  function setNoteColumns(index, count) {
    const p = state.project, track = p.tracks[index];
    if (!track) return false;
    count = clamp(Math.round(count), 1, MAX_NOTE_COLUMNS);
    const before = noteColumnCount(index);
    if (count === before) return false;
    return mutate(count > before ? 'add note column' : 'remove note column', q => {
      const t = q.tracks[index];
      if (count > 1) t.noteColumns = count; else delete t.noteColumns;
      if (count < before) for (const pat of q.patterns) {
        pat.steps[index].forEach((step, r, rows) => {
          if (!step?.columns) return;
          rows[r] = tidyStep({ ...step, columns: step.columns.slice(0, count - 1) });
        });
      }
      if (state.track === index && state.noteColumn >= count) state.noteColumn = count - 1;
    });
  }

  function addNoteColumn(index = state.track) {
    if (noteColumnCount(index) >= MAX_NOTE_COLUMNS) return toast(`A track has at most ${MAX_NOTE_COLUMNS} note columns.`);
    const done = setNoteColumns(index, noteColumnCount(index) + 1);
    if (done && index === state.track) {
      state.noteColumn = noteColumnCount(index) - 1;
      if (state.column > 2) state.column = 0;
      refresh();
    }
    return done;
  }

  // Removes the last note column; one that holds notes asks first. One Undo step.
  async function removeNoteColumn(index = state.track) {
    const p = state.project, count = noteColumnCount(index), track = p.tracks[index];
    if (!track) return false;
    if (count <= 1) return toast(`${track.name || 'This track'} has one note column.`);
    const used = columnsInUse(p, index, count - 1);
    if (used) {
      const ok = await app.confirmChoice('Remove note column?', `Note column ${count} of ${track.name || `Track ${index + 1}`} has `
          + `${used} note${used === 1 ? '' : 's'}. ⌘Z restores ${used === 1 ? 'it' : 'them'}.`, 'Remove');
      // The confirmation stays open while tracks can be deleted or reordered (AI control, MK3): never drop the
      // column, and its notes, on the track that moved into this index.
      if (!ok || state.project !== p || p.tracks[index] !== track || noteColumnCount(index) !== count) return false;
    }
    return setNoteColumns(index, count - 1);
  }

  /* ---------- grid ---------- */

  // Header buttons: add / remove a note column.
  function noteColumnButtons(t, i) {
    const count = noteColumnCount(i), tab = i === state.track ? 0 : -1;
    return `<span class="note-column-actions">${count > 1 ? `<button class="lane-action" data-note-column-remove="${i}" id="track-${i}-note-column-remove"`
        + ` tabindex="${tab}" aria-label="Remove note column ${count} of ${esc(t.name)}" title="Remove the last note column (asks first when it has notes)">−</button>` : ''}`
        + `${count < MAX_NOTE_COLUMNS ? `<button class="lane-action" data-note-column-add="${i}" id="track-${i}-note-column-add" tabindex="${tab}"`
        + ` aria-label="Add a note column to ${esc(t.name)}" title="Add a note column: another note on the same row (up to ${MAX_NOTE_COLUMNS})">+</button>` : ''}</span>`;
  }

  /* The sub-column grid of a track cell / header: NOTE INS VEL per note column, FX 1, FX 2, then a GATE per note column
     when the Gate column is shown. One note column keeps the stylesheet's layout. */
  function cellGridStyle(index) {
    const count = noteColumnCount(index);
    if (count === 1) return '';
    return ` style="grid-template-columns:repeat(${count},var(--note-w,31px) var(--ins-w,18px) 25px) 37px 37px${app.gateColumnShown() ? ` repeat(${count},42px)` : ''}"`;
  }
  // Extra width of a track column with more than one note column (virtual layout).
  const extraWidth = index => (noteColumnCount(index) - 1) * (31 + 18 + 25 + 12 + (app.gateColumnShown() ? 46 : 0));

  // Left / Right order inside a track: [noteColumn, column] for NOTE INS VEL of each note column, then FX 1, FX 2.
  function cursorStops(track) {
    const stops = [];
    for (let n = 0; n < noteColumnCount(track); n++) for (let c = 0; c < 3; c++) stops.push([n, c]);
    stops.push([0, 3], [0, 4]);
    return stops;
  }
  // Move the cursor by dc stops within the track; returns the overflow in tracks (-1, 0, 1).
  function stepCursor(dc) {
    const stops = cursorStops(state.track), here = stops.findIndex(([n, c]) => n === cursorNoteColumn() && c === state.column);
    let next = (here < 0 ? 0 : here) + dc, overflow = 0;
    if (next < 0) { overflow = -1; next = stops.length - 1; }
    if (next >= stops.length) { overflow = 1; next = 0; }
    [state.noteColumn, state.column] = stops[next];
    return overflow;
  }

  function bindNoteColumns() {
    for (const [attribute, action] of [['noteColumnAdd', addNoteColumn], ['noteColumnRemove', removeNoteColumn]]) {
      for (const button of app.$$(`[data-${attribute.replace(/[A-Z]/g, x => '-' + x.toLowerCase())}]`)) {
        button.ondblclick = e => e.stopPropagation();
        button.onclick = e => {
          e.stopPropagation();
          const index = Number(button.dataset[attribute]);
          if (state.track !== index) {
            state.track = index;
            state.instrument = state.project.tracks[index]?.instrument || 0;
          }
          void action(index);
        };
      }
    }
  }

  /* ---------- recording into free note columns ---------- */

  /* While notes are held, each new recorded note on a track with more than one note column takes the next note
     column not held by another note (Renoise): held chords and overlapping notes sound together. Columns are
     limited to the track's current count (they never grow it); with every column held, a further note is entered as on
     a one-column track (column 0 at the cursor row). A chord from the Chord setting stays a chord in column 0. */
  const heldColumns = new Map(); // track → Map(column → owner)
  function claimColumn(track, owner, preferred = cursorNoteColumn(track)) {
    const count = noteColumnCount(track);
    if (count <= 1) return 0;
    const held = heldColumns.get(track) || new Map();
    heldColumns.set(track, held);
    for (const [column, holder] of held) if (holder === owner) return column;
    for (let n = 0; n < count; n++) {
      const column = (preferred + n) % count;
      if (!held.has(column)) {
        held.set(column, owner);
        return column;
      }
    }
    return null;
  }
  function releaseColumn(track, owner) {
    const held = heldColumns.get(track);
    if (!held) return;
    for (const [column, holder] of held) if (holder === owner) held.delete(column);
    if (!held.size) heldColumns.delete(track);
  }
  const heldColumnCount = track => heldColumns.get(track)?.size || 0;
  const clearHeldColumns = () => heldColumns.clear();

  Object.assign(app, {
    MAX_NOTE_COLUMNS, growNoteColumns, noteColumnCount, noteCell, hasColumnNotes, isEmptyStep, dropEmptyStep, isColumnHolder, playedCells, tidyStep,
    setColumnCell, cellValue, clearCell, cellStep, cursorNoteColumn, cursorCell, columnsInUse, normalizeNoteColumns, setNoteColumns,
    addNoteColumn, removeNoteColumn, noteColumnButtons, cellGridStyle, noteColumnExtraWidth: extraWidth, cursorStops,
    stepCursor, bindNoteColumns, claimColumn, releaseColumn, heldColumnCount, clearHeldColumns
  });

  app.defineCommands?.({
    addNoteColumn: () => addNoteColumn(),
    // The confirmation is a dialog the user answers; the command itself returns once it is shown.
    removeNoteColumn: () => {
      void removeNoteColumn();
      return true;
    }
  });
})();
