/* Song view: the page around the song's panels. The pattern sequence is the block arranger (song-sequence.js) and
   the audio timeline is the clip arranger (clip-arranger.js); the title bar and the song settings are rendered here. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, clamp, state, mutate, bindScalarField, setPlaybackMode, togglePlay, sidebar, bindSidebar
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
        + `<div class="view-padding" data-scroll="song"><div class="sequence-layout">${window.STSong.render()}`
        + `<aside class="stack"><div class="panel"><div class="panel-head">`
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
        + ` max="1" step=".01" value="${p.metronomeVolume}"></label><label class="field">`
        + `<span>Lines per quarter note</span><select id="lines-per-beat">${[1, 2, 3, 4, 6, 8, 12, 16].map(x => `<option ${p.linesPerBeat === x ? 'selected' : ''}>${x}</option>`).join('')}</select>`
        + `</label><label class="field"><span>Swing · ${Math.round((p.swing || 0) * 100)}%</span>`
        + `<input type="range" id="swing" min="0" max=".75" step=".01" value="${p.swing || 0}">`
        + `</label><button id="song-play" class="primary">▶ Play song</button></div></div>`
        + `</aside></div>${window.STClips?.render() || ''}</div></section></div>`;
  }

  function bindSong() {
    bindSidebar();
    $('#sequence-add').onclick = () => window.STSong.actions.insertActive();
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
    window.STSong.bind();
    window.STClips?.bind();
  }

  Object.assign(app, { renderSong, bindSong });
})();
