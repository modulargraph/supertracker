/* Pattern grid columns: track headers, the track-group header row, collapsed group overviews
   and the cell markup of one column, plus the per-track source label and effect text. */
(function () {
  'use strict';

  const app = window.STApp;
  const { clamp, esc, noteText, colors, state, pattern } = app;

  function trackSilenced(p, i) {
    const group = window.STGroups?.ofTrack(p, i),
      bus = group && p.routing.buses.find(item => item.id === group.busID);
    if (bus?.mute || bus && !bus.solo && p.routing.buses.some(item => item.type === 'group' && item.solo)
        && !p.tracks[i]?.solo) return true;
    return !!p.tracks[i]?.mute || p.tracks.some(t => t.solo) && !p.tracks[i]?.solo;
  }

  app.perfToolbarOpen = (() => {
    try {
      return localStorage.getItem('st.pattern.perfOpen') === '1';
    } catch {
      return false;
    }
  })();

  function setPerfToolbarOpen(open) {
    app.perfToolbarOpen = !!open;
    try {
      localStorage.setItem('st.pattern.perfOpen', app.perfToolbarOpen ? '1' : '0');
    } catch {}
  }

  // Track header: identity (number · colour · name · M/S) over sound source (type · slot · name).
  const SOURCE_TYPES = {
    sample: ['SMP', 'Sampler'],
    wavetable: ['WTB', 'Wavetable'],
    au: ['PLG', 'Plug-in'],
    midi: ['MIDI', 'External MIDI']
  };

  function trackSource(t) {
    const p = state.project;
    const inst = p.instruments[t.instrument] || {},
      type = inst.type === 'sample' && inst.mode === 'wavetable' ? 'wavetable' : SOURCE_TYPES[inst.type] ? inst.type : 'sample',
      [badge, kind] = SOURCE_TYPES[type];
    return {
      type,
      badge,
      kind,
      slot: String(t.instrument).padStart(2, '0'),
      name: inst.name || 'No instrument',
      full: `${kind} ${String(t.instrument).padStart(2, '0')} ${inst.name || ''}`
    };
  }

  function trackHeader(t, i) {
    const p = state.project,
      src = trackSource(t),
      num = String(i + 1).padStart(2, '0'),
      group = window.STGroups?.ofTrack(p, i),
      // The cursor track's header buttons are in the Tab order; the others are reached with arrows (block navigation).
      // Stable ids keep keyboard focus on a button when it re-renders the grid (refresh restores focus by id).
      tab = i === state.track ? 0 : -1;
    return `<th class="track-head ${group ? 'group-member' : ''} ${i === state.track ? 'selected-track' : ''} ${trackSilenced(p, i) ? 'track-silenced' : ''}"`
        + ` data-track="${i}" style="--lane:${esc(t.color || colors[i % 8])}${group ? `;--group:${esc(group.color)}` : ''}"><div class="track-title">`
        + `<span class="track-num" aria-label="Track ${i + 1}">${num}</span><span class="track-name grow"`
        + ` title="${esc(t.name)} · double-click to rename">${esc(t.name)}</span><span class="track-ms-pair">`
        + `<button class="track-ms ${t.mute ? 'on mute' : ''}" data-track-toggle="mute" data-toggle-index="${i}"`
        + ` id="track-${i}-mute" tabindex="${tab}" aria-pressed="${!!t.mute}" aria-label="Mute ${esc(t.name)}" title="Mute track · ⌥M">M</button>`
        + `<button class="track-ms ${t.solo ? 'on solo' : ''}" data-track-toggle="solo" data-toggle-index="${i}"`
        + ` id="track-${i}-solo" tabindex="${tab}" aria-pressed="${!!t.solo}" aria-label="Solo ${esc(t.name)}"`
        + ` title="Solo track · ⌥S">S</button></span>${app.headerActions?.(t, i) || ''}${app.noteColumnButtons?.(t, i) || ''}</div><button class="track-source" data-track-source="${i}"`
        + ` data-source-type="${src.type}" id="track-${i}-source" tabindex="${tab}" aria-label="${esc(src.full)} · change instrument"`
        + ` title="${esc(src.full)} · click to change the assigned instrument (F6 → Note destination)">`
        + `<span class="source-badge">${src.badge}</span>${src.slot ? `<span class="source-slot">${src.slot}</span>` : ''}<span `
        + `class="source-name">${esc(src.name)}</span></button>${trackColumnLabels(i)}</th>`;
  }

  // Sub-column labels; a track with several note columns shows NOTE INS VEL for each, then FX, then a GATE for each.
  function trackColumnLabels(i) {
    const count = app.noteColumnCount?.(i) || 1,
      gate = n => `<span${n ? ` class="note-column-label" data-note-column="${n}"` : ''} title="Note length in pattern rows (— none: plays until the `
          + `next note, OFF or CUT); edit Gate below">GATE</span>`;
    if (count === 1) return `<div class="track-columns"><span>NOTE</span><span>INS</span><span>VEL</span><span>FX 1</span>`
        + `<span>FX 2</span>${app.gateColumnShown() ? gate(0) : ''}</div>`;
    const group = n => n ? `<span class="note-column-label" data-note-column="${n}" title="Note column ${n + 1}">N${n + 1}</span>`
        + `<span class="note-column-label">INS</span><span class="note-column-label">VEL</span>` : '<span>NOTE</span><span>INS</span><span>VEL</span>';
    return `<div class="track-columns has-note-columns"${app.cellGridStyle(i)}>${Array.from({ length: count }, (_, n) => group(n)).join('')}`
        + `<span>FX 1</span><span>FX 2</span>${app.gateColumnShown() ? Array.from({ length: count }, (_, n) => gate(n)).join('') : ''}</div>`;
  }

  // Cells show effect codes (app-entry.js): R04, Y50, P-30 … The table below is the fallback before it loads.
  function fxText(fx) {
    if (app.fxCodeText) return app.fxCodeText(fx);
    if (!fx) return '·· ··';
    const short = {
      plugin: 'PLG',
      volume: 'VOL',
      pan: 'PAN',
      pitch: 'PIT',
      cutoff: 'CUT',
      resonance: 'RES',
      delay: 'DLY',
      reverb: 'REV',
      probability: 'PRB',
      ratchet: 'RAT',
      reverse: 'RVS',
      offset: 'OFF',
      glide: 'GLD',
      microtiming: 'µTM',
      arpeggio: 'ARP',
      slice: 'SLC',
      position: 'POS'
    };
    if (fx.type === 'arpeggio') return 'ARP ' + Math.round(fx.value).toString(16).toUpperCase().padStart(2, '0');
    if (fx.type === 'position') return 'POS ' + Math.round(clamp(Number(fx.value) || 0, 0, 1) * 100);
    return (short[fx.type] || fx.type.slice(0, 3))
        + ' '
        + (fx.type === 'plugin' || Math.abs(fx.value) < 1 ? Math.round(fx.value * 100) : Math.round(fx.value));
  }

  function patternHitNotes(hit) {
    return hit?.chord?.length > 1
        ? hit.chord.map(offset => noteText(clamp(hit.note + offset, 0, 127))).join(' ')
        : noteText(hit?.note);
  }

  /* Every track is a column; a collapsed track group is one overview column (index = its first member). Hidden
     empty tracks drop out and collapsed tracks are slim columns ({ slim: true }, app-track-lanes.js). */
  function patternColumns() {
    const p = state.project,
      columns = window.STGroups ? window.STGroups.columns(p) : p.tracks.map((track, index) => ({ track, index, group: null }));
    return app.laneColumns ? app.laneColumns(columns) : columns;
  }

  // The grid column that shows a track: its own column, or its collapsed group.
  function columnFor(columns, track = state.track) {
    return columns.findIndex(c => c.index === track || c.members?.includes(track));
  }

  const showsGroupRow = p => !!window.STGroups?.list(p).length;

  // The row above the track headers: one cell per group spanning its columns.
  function patternGroupHeaders(columns = patternColumns(), segments = null) {
    const items = segments || columns.map((_, index) => ({ index }));
    let html = '';
    for (let i = 0; i < items.length; i++) {
      const x = items[i];
      if (x.spacer) {
        html += '<th class="tracker-spacer" aria-hidden="true"></th>';
        continue;
      }
      const group = columns[x.index].group;
      let span = 1;
      while (group && items[i + 1] && !items[i + 1].spacer && columns[items[i + 1].index].group === group) {
        span++;
        i++;
      }
      html += group ? groupHeader(group, span) : `<th class="group-spacer${columns[x.index].slim ? ' is-slim' : ''}"></th>`;
    }
    return html;
  }

  function groupHeader(group, span) {
    const p = state.project,
      members = window.STGroups.members(p, group),
      bus = p.routing.buses.find(item => item.id === group.busID) || {},
      owner = window.STGroups.busOwner(group),
      selected = members.includes(state.track),
      label = `${group.collapsed ? 'Expand' : 'Collapse'} ${esc(group.name)}`,
      tab = selected ? 0 : -1, id = esc(group.id);
    return `<th class="group-head ${selected ? 'is-selected' : ''} ${group.collapsed ? 'is-collapsed' : ''}" colspan="${span}"`
        + ` style="--group:${esc(group.color)}" data-group-id="${esc(group.id)}"><div class="group-head-row">`
        + `<button class="group-collapse" data-group-collapse="${esc(group.id)}" id="group-${id}-collapse" tabindex="${tab}" aria-expanded="${!group.collapsed}"`
        + ` aria-label="${label}" title="${label} · ⌥J">${group.collapsed ? '▸' : '▾'}</button>`
        + `<span class="group-name" data-group-rename="${esc(group.id)}" title="${esc(group.name)} · double-click to rename">${esc(group.name)}</span>`
        + `<span class="group-count">${members.length} track${members.length === 1 ? '' : 's'}</span>${app.groupHiddenChip?.(group) || ''}`
        + `<span class="group-meter" aria-hidden="true"><span data-group-meter="${esc(owner)}"></span></span>`
        + `<span class="track-ms-pair"><button class="track-ms ${bus.mute ? 'on mute' : ''}" data-group-toggle="mute"`
        + ` data-group-id="${esc(group.id)}" id="group-${id}-mute" tabindex="${tab}" aria-pressed="${!!bus.mute}" aria-label="Mute group ${esc(group.name)}"`
        + ` title="Mute the group bus">M</button><button class="track-ms ${bus.solo ? 'on solo' : ''}" data-group-toggle="solo"`
        + ` data-group-id="${esc(group.id)}" id="group-${id}-solo" tabindex="${tab}" aria-pressed="${!!bus.solo}" aria-label="Solo group ${esc(group.name)}"`
        + ` title="Solo the group bus">S</button></span>${app.groupHeaderActions?.(group) || ''}</div></th>`;
  }

  // A collapsed group: one read-only overview of which members hit on each row.
  function groupOverviewHeader({ index, group, members }) {
    const selected = members.includes(state.track);
    return `<th class="track-head group-overview ${selected ? 'selected-track' : ''}" data-track="${index}"`
        + ` data-group-overview="${esc(group.id)}" style="--group:${esc(group.color)}"><div class="track-title">`
        + `<span class="track-name grow" title="${esc(group.name)}">${esc(group.name)}</span></div>`
        + `<div class="group-overview-sub">${members.length} tracks · click a hit to edit</div></th>`;
  }

  function groupOverviewCell({ index, members }, r, sel, silenced) {
    const p = state.project,
      steps = pattern().steps,
      active = r === state.row && members.includes(state.track),
      isSel = sel && r >= sel.r0 && r <= sel.r1 && members.some(m => m >= sel.t0 && m <= sel.t1),
      hits = members.flatMap(m => steps[m]?.[r] ? [[m, steps[m][r]]] : []);
    const chips = hits.map(([m, step]) => {
      const track = p.tracks[m], played = app.playedCells?.(step) || [], note = played[0]?.cell.note ?? step.note,
        pitch = (note < 0 || note !== (track.padNote ?? 60) ? ' ' + noteText(note) : '') + (played.length > 1 ? '+' : '');
      return `<button class="group-hit ${active && state.track === m ? 'is-selected' : ''}" data-member-track="${m}" tabindex="-1"`
          + ` style="--lane:${esc(track.color || colors[m % 8])}" title="${esc(track.name)} · ${noteText(step.note)} · v${esc(step.velocity ?? 100)} · click to edit">`
          + `${esc(track.name)}${pitch}</button>`;
    }).join('');
    return `<td data-cell="${index}:${r}" class="group-overview-cell ${active ? 'selected' : ''} ${isSel ? 'selection' : ''}`
        + ` ${members.every(m => silenced[m]) ? 'track-silenced' : ''}"><div class="group-hits">${chips || '<span class="group-empty-step">···</span>'}</div></td>`;
  }

  function patternColumnHeader(column) {
    return column.collapsed ? groupOverviewHeader(column) : column.slim ? app.slimHeader(column) : trackHeader(column.track, column.index);
  }

  function patternColumnCell(column, r, sel, silenced) {
    if (column.collapsed) return groupOverviewCell(column, r, sel, silenced);
    if (column.slim) return app.slimCell(column, r, sel, silenced);
    const { track: t, index: i } = column,
      s = pattern().steps[i]?.[r],
      active = r === state.row && i === state.track,
      isSel = sel && r >= sel.r0 && r <= sel.r1 && i >= sel.t0 && i <= sel.t1;
    return `<td data-cell="${i}:${r}" class="${active ? 'selected' : ''} ${isSel ? 'selection' : ''} ${silenced[i] ? 'track-silenced' : ''}`
        + ` ${s?.chord?.length > 1 ? 'has-chord' : ''}"${s?.chord?.length > 1 ? ' title="Chord ' + esc(patternHitNotes(s)) + '"' : ''}>`
        + `<div class="tracker-cell"${app.cellGridStyle?.(i) || ''}>${app.renderPatternCellContent(s, t, i, active)}</div></td>`;
  }

  Object.assign(app, {
    trackSilenced, setPerfToolbarOpen, fxText, patternColumns, patternGroupHeaders, patternColumnHeader,
    patternColumnCell, columnFor, showsGroupRow
  });
})();
