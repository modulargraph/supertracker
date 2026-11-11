/* Song view: the pattern sequence, song settings and the audio clip timeline host. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, esc, state, beatsPerBar, toast, mutate,
    bindScalarField, refresh, setPlaybackMode, togglePlay, sidebar, bindSidebar
  } = app;

  function songDurationSeconds(p) {
    const beats = p.sequence.reduce((n, id) => n + (p.patterns.find(x => x.id === id)?.length || 0) / p.linesPerBeat,
        0);
    try {
      return window.STProjectTiming.create(p).duration(0, beats);
    } catch {
      return null;
    }
  }

  function renderSong() {
    const p = state.project,
      seconds = songDurationSeconds(p);
    return `<div class="workspace">${sidebar('patterns')}<section class="workspace-main">`
        + `<div class="view-title"><div><div class="eyebrow">Song arranger</div><h1 style="margin-top:7px">Song arrangement</h1>`
        + `<div class="subtitle">Pattern sequence and audio timeline · F6 next section · Shift F6 previous</div>`
        + `</div><div class="row"><button id="song-export">Export audio…</button>`
        + `<button id="sequence-add" class="primary">+ Add pattern</button></div></div>`
        + `<div class="view-padding" data-scroll="song"><div class="sequence-layout"><div class="panel">`
        + `<div class="panel-head"><h2>Sequence</h2><span class="chip">${p.sequence.length} PATTERNS</span>`
        + `</div><div class="sequence-list" role="listbox" aria-label="Song sequence" tabindex="0" id="sequence-list">${p.sequence.map((id, i) => {
      const pat = p.patterns.find(x => x.id === id) || p.patterns[0];
      return `<div class="sequence-item ${state.sequence === i ? 'selected' : ''}" data-sequence="${i}"`
          + ` role="option" aria-selected="${state.sequence === i}"><span class="num">${String(i + 1).padStart(2, '0')}</span>`
          + `<div><select data-sequence-pattern="${i}" aria-label="Pattern in sequence position ${i + 1}">${p.patterns.map(q => `<option value="${q.id}" ${q.id === id ? 'selected' : ''}>${esc(q.name)}</option>`).join('')}</select>`
          + `<div class="sequence-mini">${Array.from({
        length: 32
      }, (_, r) => {
        let count = 0;
        for (let t = 0; t < pat.steps.length; t++) if (pat.steps[t]?.[Math.floor(r / 32 * pat.length)]) count++;
        return `<i style="height:${Math.max(3, count / Math.max(pat.steps.length, 1) * 19)}px;align-self:flex-end"></i>`;
      }).join('')}</div></div><span class="bars">${(pat.length / (p.linesPerBeat * beatsPerBar(p))).toFixed(1) + ' bars'}</span>`
          + `<div class="row"><button data-sequence-up="${i}" aria-label="Move up" ${i === 0 ? 'disabled' : ''}>↑</button>`
          + `<button data-sequence-down="${i}" aria-label="Move down" ${i === p.sequence.length - 1 ? 'disabled' : ''}>↓</button>`
          + `<button data-sequence-remove="${i}" aria-label="Remove pattern from sequence" ${p.sequence.length === 1 ? 'disabled' : ''}>×</button></div></div>`;
    }).join('')}</div></div><aside class="stack"><div class="panel"><div class="panel-head">`
        + `<h2>Song settings</h2></div><div class="song-options"><div><span class="eyebrow">Estimated duration</span>`
        + `<div class="song-duration">${seconds === null
            ? 'Unavailable'
            : Math.floor(seconds / 60) + ':' + String(Math.floor(seconds % 60)).padStart(2, '0')}</div>`
        + `<span class="muted" style="font-size:11px">Before effect tails · ${p.bpm + ' BPM'}</span>`
        + `</div><label class="row"><input type="checkbox" id="loop-song" ${p.loopSong ? 'checked' : ''}>Loop song</label>`
        + `<div class="grid2"><label class="field"><span>Beats per bar</span><input id="signature-numerator"`
        + ` type="number" min="1" max="16" step="1" value="${p.timeSignature.numerator}">`
        + `</label><label class="field"><span>Beat unit</span><select id="signature-denominator">${[2, 4, 8, 16].map(x => `<option value="${x}" ${x === p.timeSignature.denominator ? 'selected' : ''}>1/${x}</option>`).join('')}</select>`
        + `</label></div><label class="row"><input id="song-metronome" type="checkbox" ${p.metronome ? 'checked' : ''}>Metronome</label>`
        + `<label class="field"><span>Click level</span><input id="metronome-volume" type="range" min="0"`
        + ` max="1" step=".01" value="${p.metronomeVolume ?? .4}"></label><label class="field">`
        + `<span>Lines per quarter note</span><select id="lines-per-beat">${[1, 2, 3, 4, 6, 8, 12, 16].map(x => `<option ${p.linesPerBeat === x ? 'selected' : ''}>${x}</option>`).join('')}</select>`
        + `</label><label class="field"><span>Swing · ${Math.round((p.swing || 0) * 100)}%</span>`
        + `<input type="range" id="swing" min="0" max=".75" step=".01" value="${p.swing || 0}">`
        + `</label><button id="song-play" class="primary">▶ Play song</button></div></div>`
        + `<div class="panel"><div class="panel-body"><h3>Arrangement keys</h3><div class="shortcut-item">`
        + `<span>Next / previous section</span><kbd>F6 / ⇧F6</kbd></div><div class="shortcut-item">`
        + `<span>Select a position</span><kbd>↑ ↓</kbd></div><div class="shortcut-item">`
        + `<span>Move position</span><kbd>⌥ ↑ ↓</kbd></div><div class="shortcut-item">`
        + `<span>Duplicate position</span><kbd>⌘D</kbd></div><div class="shortcut-item">`
        + `<span>Edit pattern</span><kbd>Enter</kbd></div><div class="shortcut-item"><span>Remove position</span>`
        + `<kbd>Delete</kbd></div></div></div></aside></div>${window.STClips?.render() || ''}</div></section></div>`;
  }

  function bindSong() {
    bindSidebar();
    $('#sequence-add').onclick = () => mutate('add sequence position', p => {
      p.sequence.splice(state.sequence + 1, 0, p.activePattern);
      state.sequence++;
    });
    $('#song-export').onclick = app.showExport;
    bindScalarField($('#signature-numerator'), 'time signature', (p, e) => p.timeSignature = {
      numerator: clamp(Math.round(Number(e.target.value)), 1, 16),
      denominator: p.timeSignature.denominator
    });
    $('#signature-denominator').onchange = e => {
      mutate('time signature', p => p.timeSignature = {
        numerator: p.timeSignature.numerator,
        denominator: Number(e.target.value)
      });
    };
    $('#song-metronome').onchange = e => mutate('metronome', p => p.metronome = e.target.checked);
    bindScalarField($('#metronome-volume'), 'metronome level', (p, e) => p.metronomeVolume = Number(e.target.value));
    $('#loop-song').onchange = e => mutate('song loop', p => p.loopSong = e.target.checked);
    $('#lines-per-beat').onchange = e => mutate('grid division', p => p.linesPerBeat = Number(e.target.value));
    bindScalarField($('#swing'), 'swing', (p, e) => p.swing = Number(e.target.value));
    $('#song-play').onclick = async () => {
      if (!state.playing && !app.pendingPlayback) {
        if (!setPlaybackMode('song')) return false;
        state.row = 0;
      }
      return togglePlay();
    };
    for (const el of $$('[data-sequence]')) el.onclick = e => {
      if (e.target.closest('select,button')) return;
      state.sequence = Number(el.dataset.sequence);
      refresh();
      $('#sequence-list').focus({ preventScroll: true });
    };
    for (const el of $$('[data-sequence-pattern]')) el.onchange = () => mutate('sequence pattern',
        p => p.sequence[Number(el.dataset.sequencePattern)] = Number(el.value));
    for (const el of $$('[data-sequence-up]')) el.onclick = () => moveSequence(Number(el.dataset.sequenceUp), -1);
    for (const el of $$('[data-sequence-down]')) el.onclick = () => moveSequence(Number(el.dataset.sequenceDown), 1);
    for (const el of $$('[data-sequence-remove]')) el.onclick = () => removeSequence(Number(el.dataset.sequenceRemove));
    // Last: the block arranger replaces the legacy list and owns "+ Add pattern" (inserted after the selected block, never inside a repeat run).
    window.STClips?.bind();
  }

  function moveSequence(index, delta) {
    const dst = index + delta;
    if (dst < 0 || dst >= state.project.sequence.length) return;
    mutate('move sequence position', p => {
      [p.sequence[index], p.sequence[dst]] = [p.sequence[dst], p.sequence[index]];
      state.sequence = dst;
    });
  }

  function removeSequence(index) {
    if (state.project.sequence.length === 1) return toast('The song needs at least one pattern.');
    mutate('remove sequence position', p => {
      p.sequence.splice(index, 1);
      state.sequence = Math.min(index, p.sequence.length - 1);
    });
  }

  Object.assign(app, { renderSong, bindSong, moveSequence, removeSequence });
})();
