/* Shell: the top bar, view navigation and the render cycle (refresh → renderShell →
   renderView → bind*). */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, esc, noteText, views, state, reconcileSampleMetadata, beginScalarFields, bindScalarField,
    updateMIDIRoute
  } = app;

  function setView(view) {
    if (view !== state.view
        && state.view === 'recording'
        && window.STRecording?.leaveInline?.() === false) return false;
    if (view !== state.view) app.detachStoppedInputCursor();
    app.stopHeld();
    if (state.view === 'browser' && view !== 'browser') window.STBrowser?.stopPreview?.();
    state.view = view;
    state.selection = null;
    refresh();
    if (view === 'pattern') $('#tracker')?.focus({ preventScroll: true });
    if (view === 'recording') $(state.keyboardPlay ? '#keyboard-play' : '#record-name')?.focus({ preventScroll: true });
    if (view === 'song') $('#sequence-list')?.focus({ preventScroll: true });
    if (view === 'browser') {
      window.STBrowser?.focusSearch?.();
      window.STBrowser?.search();
    }
    if (view === 'mixer') {
      if (window.STStudio?.focusSelectedChannel) {
        window.STStudio.focusSelectedChannel();
      } else {
        $(`#mix-track-${state.track}`)?.focus({ preventScroll: true });
      }
    }
    if (view === 'automation') $('#automation-search')?.focus({ preventScroll: true });
    if (view === 'instrument') $('#instrument-rename')?.focus({
      preventScroll: true
    });
  }

  function refresh() {
    if (!state.project) return;
    window.STRecording?.rememberInline?.();
    reconcileSampleMetadata();
    updateMIDIRoute();
    const focused = document.activeElement;
    const focusId = focused?.id;
    const selection = focused && 'selectionStart' in focused ? [focused.selectionStart, focused.selectionEnd] : null;
    const oldMain = $('main');
    const scroll = oldMain
        ? Object.fromEntries($$('[data-scroll]', oldMain).map(el => [el.dataset.scroll, [el.scrollTop, el.scrollLeft]]))
        : {};
    $('#app').innerHTML = renderShell();
    bindShell();
    if (state.view === 'pattern') app.bindPattern();
    if (state.view === 'instrument') app.bindInstrument();
    if (state.view === 'song') app.bindSong();
    if (state.view === 'browser') window.STBrowser?.bind();
    if (state.view === 'recording') window.STLineInputs?.bind();
    if (state.view === 'mixer') window.STStudio?.bindMixer();
    if (state.view === 'automation') window.STStudio?.bindAutomation();
    window.STBasePatch?.bind();
    $$('[data-scroll]').forEach(el => {
      if (scroll[el.dataset.scroll]) {
        el.scrollTop = scroll[el.dataset.scroll][0];
        el.scrollLeft = scroll[el.dataset.scroll][1];
      }
    });
    if (focusId) {
      const el = document.getElementById(focusId);
      if (el) {
        el.focus({ preventScroll: true });
        if (selection && el.setSelectionRange && el.type !== 'number') {
          try {
            el.setSelectionRange(...selection);
          } catch {}
        }
      }
    }
    app.updateTransport();
  }

  function renderShell() {
    const p = state.project;
    return `<header class="topbar"><div class="brand"><div class="brand-icon"><i></i><i>`
        + (`</i><i></i></div><div><div class="brand-name">SuperTracker</div><div class="project-name"`
        + ` id="project-name">${esc(p.name)}${state.dirty ? ' · edited' : ''}</div>`)
        + `</div></div><div class="transport"><label class="transport-mode" title="Choose playback mode while stopped">`
        + `<span>Mode</span><select id="transport-mode" aria-label="Playback mode"><option value="song"`
        + ` ${state.playbackMode === 'song' ? 'selected' : ''}>Song</option><option value="pattern" ${state.playbackMode === 'pattern' ? 'selected' : ''}>Pattern</option>`
        + `</select></label><button class="transport-button play" id="play" title="Play / pause · Space"`
        + ` aria-label="Play">${state.playing ? 'Ⅱ' : '▶'}</button><button class="transport-button" id="stop"`
        + ` title="Stop · Escape" aria-label="Stop">■</button><button class="transport-button record ${state.recording ? 'active' : ''}"`
        + ` id="record" title="Step / live record arm · ⌘R" aria-label="Arm note recording" aria-pressed="${state.recording}">●</button>`
        + `<div class="readout"><span class="label">Seq : step</span><span class="value" id="position">${String(state.sequence + 1).padStart(2, '0')}`
        + ` : ${String(state.row).padStart(2, '0')}</span></div><div class="separator">`
        + `</div><div class="readout"><span class="label">Tempo</span><input class="tempo" id="tempo"`
        + ` aria-label="Tempo in beats per minute" type="number" min="20" max="400" value="${p.bpm}"`
        + ` title="Song tempo">`
        + `</div><div class="readout"><span class="label">Grid</span><span class="value" style="font-size:12px">1/${p.linesPerBeat * 4}</span>`
        + `</div><button id="metronome" class="metronome ${p.metronome ? 'active' : ''}" aria-label="Metronome"`
        + ` aria-pressed="${!!p.metronome}" title="Toggle metronome · ⌘⇧M"><svg viewBox="0 0 20 22" aria-hidden="true">`
        + `<path d="M6 2h8l4 18H2L6 2Z M10 17L16 5 M7 17h6"/></svg><span>${p.timeSignature?.numerator || 4}/${p.timeSignature?.denominator || 4}</span>`
        + `</button></div><div class="top-actions"><span class="native-badge ${state.native ? '' : 'preview'}">${state.native ? '● Core Audio' : 'Interface preview'}</span>`
        + `<button class="quiet" id="save" title="Save project · ⌘S">Save</button><button id="palette"`
        + ` class="quiet" title="Command palette · ⌘K">⌘ K</button><button id="settings" class="quiet"`
        + ` title="Studio settings">⚙</button></div></header><nav class="nav" aria-label="Studio views">${views.map(([id, label], i) => `<button class="nav-button ${state.view === id ? 'active' : ''}" id="view-${id}" data-view="${id}"`
        + ` tabindex="${state.view === id ? 0 : -1}" aria-current="${state.view === id ? 'page' : 'false'}">${label}<kbd>⌘${i + 1}</kbd>`
        + `</button>`).join('')}<div `
        + `class="nav-end"><button class="quiet" id="pane-next" aria-label="Next workspace section" aria-keyshortcuts="F6 `
        + `Shift+F6" title="Next section: F6 · Previous: Shift F6 (use Fn on media-key keyboards)">Sections <kbd>F6</kbd>`
        + (`</button><button class="keyplay ${state.keyboardPlay ? 'active' : ''}" id="keyboard-play"`
        + ` title="Toggle computer-keyboard `)
        + `piano · ⌘P">⌨ ${state.keyboardPlay ? 'Keys on' : 'Keys off'}</button><button class="quiet"`
        + ` id="help" aria-label="Keyboard shortcuts">?</button></div></nav><main id="main">${renderView()}</main>`
        + `<footer class="statusbar"><span><i class="status-led"></i><span id="status-message" role="status">${state.recording
            ? state.playing ? 'Recording live notes' : 'Step recording armed'
            : 'Ready to make something'}</span>`
        + `</span><span id="status-selection">${esc(p.tracks[state.track]?.name)} · ${noteText(state.lastInputNote)}</span>`
        + `<div class="end"><span id="status-shortcuts">${state.keyboardPlay ? `Keys · ${esc(app.performanceSummary())}` : 'Space play · ⌘K commands'}</span>${globalThis.STAgent?.statusBadge?.() || ''}<span `
        + `id="cpu">${state.native ? 'DSP 0%' : 'Native audio unavailable'}</span></div></footer>`;
  }

  function renderView() {
    switch (state.view) {
      case 'pattern':
        return app.renderPattern();
      case 'instrument':
        return app.renderInstrument();
      case 'recording':
        return window.STLineInputs?.render() || '';
      case 'browser':
        return window.STBrowser?.render() || '<div class="empty">Loading sample browser…</div>';
      case 'mixer':
        return window.STStudio?.renderMixer() || '';
      case 'automation':
        return window.STStudio?.renderAutomation() || '';
      case 'song':
        return app.renderSong();
      default:
        return '';
    }
  }

  function toggleKeyboardPlay() {
    state.keyboardPlay = !state.keyboardPlay;
    if (!state.keyboardPlay) app.stopHeld();
    app.updateTransport();
    // Safari does not focus clicked buttons: release a previously focused text
    // field so the first piano key is heard after enabling computer-keyboard play.
    if (state.keyboardPlay) (state.view === 'pattern' ? $('#tracker') : $('#keyboard-play'))?.focus({
      preventScroll: true
    });
  }

  function toggleNoteRecordingFromUI() {
    const result = app.setNoteRecording();
    if (app.recordArmDesired && state.view === 'pattern' && !state.modal) $('#tracker')?.focus({ preventScroll: true });
    return result;
  }

  function bindShell() {
    beginScalarFields();
    for (const el of $$('[data-view]')) el.onclick = () => setView(el.dataset.view);
    $('#play').onclick = app.togglePlay;
    $('#transport-mode').onchange = e => app.setPlaybackMode(e.target.value);
    $('#pane-next').onclick = e => window.STPaneNavigation?.next(e?.shiftKey ? -1 : 1);
    $('#metronome').onclick = () => app.perform('metronome');
    $('#stop').onclick = app.stop;
    $('#record').onclick = toggleNoteRecordingFromUI;
    bindScalarField($('#tempo'), 'tempo', (p, e) => {
      // bindScalarField only commits a number: one outside 20–400 (0 too) goes to the nearest limit.
      p.bpm = clamp(Number(e.target.value), 20, 400);
    });
    $('#save').onclick = () => app.perform('save');
    $('#palette').onclick = app.openPalette;
    $('#help').onclick = app.showHelp;
    $('#settings').onclick = app.showSettings;
    $('#keyboard-play').onclick = toggleKeyboardPlay;
  }

  Object.assign(app, { setView, refresh, toggleKeyboardPlay, toggleNoteRecordingFromUI });
})();
