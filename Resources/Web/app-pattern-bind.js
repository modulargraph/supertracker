/* Pattern view events: pointer, keyboard and field handlers for the tracker grid,
   toolbar and step editor. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, FX, state, pattern, mutate, bindScalarField, refresh, updateTransport, bindSidebar,
    addPattern, setPerfToolbarOpen, bindPatternViewport, addTrack, updatePlayhead
  } = app;

  function bindPattern() {
    bindPatternViewport();
    const patternOwner = pattern(),
      stepTrack = state.track,
      stepRow = state.row;
    bindSidebar();
    app.bindPatternMatrix();
    window.STPatternDestination?.bind();
    if ($('#pattern-loop')) $('#pattern-loop').onclick = () => app.loopSelection();
    $('#pattern-rename').onclick = () => app.textPrompt('Rename pattern', pattern().name,
        name => mutate('rename pattern', () => pattern().name = name));
    $('#pattern-duplicate').onclick = () => addPattern(true);
    $('#track-add').onclick = addTrack;
    if ($('#group-create')) $('#group-create').onclick = () => app.perform('groupTracks');
    if ($('#kit-create')) $('#kit-create').onclick = () => app.perform('browseKits');
    $('#track-duplicate').onclick = () => window.STTrackDuplicate?.duplicate();
    bindScalarField($('#pattern-length'), 'pattern length', (p, e) => {
      const length = clamp(Math.round(Number(e.target.value)), 1, 256),
        pat = patternOwner;
      pat.length = length;
      pat.steps.forEach(s => {
        s.length = length;
        for (let r = 0; r < length; r++) s[r] = s[r] || null;
      });
      pat.automation.forEach(l => l.points = l.points.filter(x => x.row < length));
    }, () => pattern() === patternOwner);
    $('#edit-step').onchange = e => state.editStep = Number(e.target.value);
    $('#octave').onchange = e => {
      state.octave = Number(e.target.value);
      updateTransport();
    };
    $('#chord').onchange = e => app.setPerformance({ chord: e.target.value, chordSet: '' }, false);
    $('#scale').onchange = e => app.setPerformance({ scale: e.target.value }, false);
    $('#root').onchange = e => app.setPerformance({ root: Number(e.target.value) }, false);
    if ($('#chord-set')) $('#chord-set').onchange = e => app.setPerformance({ chordSet: e.target.value }, false);
    if ($('#midi-apply')) $('#midi-apply').onchange = e => app.setPerformance({ midiApply: e.target.checked }, false);
    $('#scale-mode').onchange = e => app.setPerformance({ scaleMode: e.target.value }, false);
    $('#key-velocity').onchange = e => {
      state.keyVelocity = Number(e.target.value);
      updateTransport();
    };
    window.STNoteRepeat?.bindControls();
    for (const id of ['edit-step', 'octave', 'root', 'scale', 'scale-mode', 'chord', 'chord-set', 'midi-apply',
        'key-velocity', 'note-repeat', 'note-repeat-rate']) $('#' + id)?.addEventListener('change', () => setTimeout(() => $('#tracker')?.focus({
      preventScroll: true
    }), 0));
    for (const button of $$('[data-track-toggle]')) button.onclick = e => {
      e.stopPropagation();
      window.STPatternTools?.toggleTrack(button.dataset.trackToggle, Number(button.dataset.toggleIndex));
    };
    $('#follow').onclick = () => app.perform('follow');
    if ($('#auto-panel-toggle')) $('#auto-panel-toggle').onclick = () => app.perform(app.automationPanelShown() ? 'automationPanelHide' : 'automationPanel');
    app.bindAutomationPanel();
    app.bindSongScroll();
    $('#fill').onclick = app.fillSelection;
    if ($('#gate-column')) $('#gate-column').onclick = () => app.perform('toggleGateColumn');
    for (const el of $$('[data-cell]')) el.onclick = e => {
      app.finishEntry?.({ advance: false, render: false });
      const [t, r] = el.dataset.cell.split(':').map(Number);
      if (e.shiftKey) {
        if (!state.selection) state.selection = { track: state.track, row: state.row };
      } else {
        state.selection = null;
      }
      const column = app.patternColumns().find(c => c.index === t);
      state.track = column?.collapsed && column.members.includes(state.track) ? state.track : t;
      state.row = r;
      state.column = column?.slim ? 0 : Number(e.target.dataset.column ?? state.column);
      // An extra note column's NOTE / INS / VEL / GATE (app-note-columns.js); FX belong to column 0.
      if (column?.slim || e.target.dataset.column !== undefined || e.target.hasAttribute?.('data-gate-inspector')) {
        state.noteColumn = column?.slim ? 0 : Number(e.target.dataset.noteColumn ?? 0);
      }
      state.instrument = state.project.tracks[state.track].instrument || 0;
      refresh();
      $(e.target.hasAttribute?.('data-gate-inspector') ? '#step-gate' : '#tracker')?.focus({ preventScroll: true });
    };
    if ($('#perf-toggle')) $('#perf-toggle').onclick = () => {
      setPerfToolbarOpen(!app.perfToolbarOpen);
      refresh();
      $('#perf-toggle')?.focus({ preventScroll: true });
    };
    for (const button of $$('[data-track-source]')) {
      button.ondblclick = e => e.stopPropagation();
      button.onclick = e => {
        e.stopPropagation();
        const index = Number(button.dataset.trackSource);
        if (!state.project.tracks[index]) return;
        if (index !== state.track) {
          app.stopHeld();
          state.track = index;
          state.instrument = state.project.tracks[index].instrument || 0;
          state.selection = null;
          refresh();
        }
        $('#pattern-destination-instrument')?.focus({ preventScroll: true });
      };
    }
    for (const el of $$('th[data-track]')) {
      el.onclick = () => {
        state.track = Number(el.dataset.track);
        state.instrument = state.project.tracks[state.track].instrument || 0;
        refresh();
      };
      // Double-click the name to rename; elsewhere on the header (or a slim column) to collapse / expand the track.
      el.ondblclick = e => {
        if (el.dataset.groupOverview !== undefined) return;
        if (app.toggleTrackCollapsed && (el.dataset.trackSlim !== undefined || !e?.target?.closest?.('.track-name'))) {
          app.toggleTrackCollapsed(Number(el.dataset.track));
          return;
        }
        // The track the prompt was opened for, not whatever moves into its index while it is open.
        const track = state.project.tracks[Number(el.dataset.track)];
        if (!track) return;
        app.textPrompt('Track name', track.name,
            name => mutate('rename track', p => { if (p.tracks.includes(track)) track.name = name; }));
      };
    }
    for (const key of ['note', 'instrument', 'velocity', 'gate']) {
      if (!$('#step-' + key)) continue;
      if (key === 'instrument') {
        continue; // typed number or name prefix: app.bindStepEditor (app-entry.js)
      } else {
        bindScalarField($('#step-' + key), 'edit step', (p, e) => {
          const s = app.ensureCell();
          const value = Number(e.target.value);
          s[key] = key === 'gate'
              ? (e.target.value.trim() === '' || !(value > 0) ? 0 : clamp(value, .01, 256))
              : key === 'velocity'
              ? clamp(Math.round(value), 1, 127)
              : clamp(Math.round(value), -3, 127);
          // -3 empties an extra note column.
          if (app.cursorNoteColumn() && s.note === -3) app.clearCell(pattern().steps[state.track], state.row, app.cursorNoteColumn());
        }, () => pattern() === patternOwner
            && state.track === stepTrack
            && state.row === stepRow, null, { allowEmpty: key === 'gate' });
      }
      // Enter commits and Escape abandons the typed value; both return to the grid. Keys typed here never play notes.
      $('#step-' + key).onkeydown = e => {
        if (e.key === 'Enter' || e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation?.();
          if (e.key === 'Escape') e.target.value = e.target.defaultValue ?? e.target.value;
          e.target.blur();
          $('#tracker')?.focus({ preventScroll: true });
        }
      };
    }
    if (window.STStepFX) {
      window.STStepFX.bind();
    } else {
      for (const n of [1, 2]) {
        const type = $('#step-fx' + n),
          value = $('#step-fx' + n + '-value');
        type.onchange = () => mutate('step effect', () => app.ensureStep(-3)['fx' + n] = type.value ? {
          type: type.value,
          value: clamp(Number(value.value), FX[type.value][0], FX[type.value][1])
        } : null);
        value.onchange = () => {
          if (type.value) mutate('step effect', () => app.ensureStep(-3)['fx' + n] = {
            type: type.value,
            value: clamp(Number(value.value), FX[type.value][0], FX[type.value][1])
          });
        };
      }
    }
    $('#step-clear').onclick = app.clearSelection;
    app.bindStepEditor?.();
    if ($('#step-off')) $('#step-off').onclick = () => app.perform('noteOff');
    if ($('#step-cut')) $('#step-cut').onclick = () => app.perform('noteCut');
    app.bindPatternGroups();
    app.bindTrackLanes?.();
    app.bindNoteColumns?.();
    updatePlayhead(state.status);
  }

  Object.assign(app, { bindPattern });
})();
