/* Keyboard: the global key handler, workspace navigation keys and text-field exceptions. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    $, $$, clamp, views, state, pattern, mutate, setView, refresh, toggleNoteRecordingFromUI,
    togglePlay, stop, stepInstrument, copySelection, pasteSelection, clearSelection, shiftPatternRows,
    fillSelection, auditionStep, transpose, moveGrid, keyNotes, keyboardNote, cycleChordSet, noteOn,
    PARAM_ARROWS, endParamBurst, moveSequence, removeSequence, closeModal, showHelp, openPalette, perform,
    patternTool, showExport, automate, queueStoppedGridKey, preservedStepFX,
    automateValue
  } = app;

  function isTextInput(el) {
    return !!el?.closest('input:not([type=range]):not([type=checkbox]):not([type=button]),textarea,select,[contenteditable=true]');
  }

  function usesNativeTextUndo(el) {
    if (el?.isContentEditable) return true;
    const field = el?.closest('input,textarea');
    if (!field || field.disabled || field.readOnly) return false;
    // Selects suppress piano typing, but have no editable text history. Keep
    // their Command-Z on the session history, along with buttons and sliders.
    if (field.tagName !== 'TEXTAREA'
        && ['range', 'checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'color', 'image',
            'hidden'].includes(field.type)) return false;
    // A field still showing its rendered value holds no draft, so WebKit has no
    // text history for it: native Undo would silently do nothing. This happens
    // whenever code focuses a freshly rendered field (e.g. sample Start after a
    // recording loads); Command-Z must then undo the session edit instead.
    return !(typeof field.defaultValue === 'string' && field.value === field.defaultValue);
  }

  function optionLetter(e, letter) {
    return e.altKey
        && !e.metaKey
        && !e.ctrlKey
        && !e.isComposing
        && (e.key.toLowerCase() === letter || e.code === 'Key' + letter.toUpperCase());
  }

  function workspaceNavigation(e) {
    const target = e.target;
    function focusGroup(selector, vertical = false, sort = null) {
      const keys = vertical ? ['ArrowUp', 'ArrowDown'] : ['ArrowLeft', 'ArrowRight'];
      if (![...keys, 'Home', 'End'].includes(e.key)) return false;
      const items = $$(selector).filter(x => !x.disabled);
      if (sort) items.sort(sort);
      const current = items.indexOf(target);
      if (current < 0) return false;
      e.preventDefault();
      const next = e.key === 'Home'
          ? 0
          : e.key === 'End'
          ? items.length - 1
          : clamp(current + (e.key === keys[1] ? 1 : -1), 0, items.length - 1);
      items.forEach((x, i) => x.tabIndex = i === next ? 0 : -1);
      items[next]?.focus();
      items[next]?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
      return true;
    }
    if (target?.dataset?.view !== undefined) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setView(state.view);
        return true;
      }
      return focusGroup('[data-view]');
    }
    if (target?.dataset?.instType !== undefined) return focusGroup('[data-inst-type]');
    if (target?.dataset?.piano !== undefined) return focusGroup('[data-piano]', false,
        (a, b) => Number(a.dataset.piano) - Number(b.dataset.piano));
    const type = target?.dataset?.pattern !== undefined
        ? 'pattern'
        : target?.dataset?.instrument !== undefined
        ? 'instrument'
        : null;
    if (!type) return false;
    if (e.key === 'ArrowRight') {
      e.preventDefault();
      const selector = state.view === 'pattern'
          ? '#tracker'
          : state.view === 'song'
          ? '#sequence-list'
          : '#instrument-rename';
      $(selector)?.focus();
      return true;
    }
    if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(e.key)) return false;
    const items = $$(`[data-${type}]`),
      index = items.indexOf(target);
    if (index < 0) return false;
    e.preventDefault();
    const next = e.key === 'Home'
        ? 0
        : e.key === 'End'
        ? items.length - 1
        : clamp(index + (e.key === 'ArrowDown' ? 1 : -1), 0, items.length - 1);
    const id = items[next]?.id;
    items[next]?.click();
    document.getElementById(id)?.focus();
    document.getElementById(id)?.scrollIntoView?.({ block: 'nearest' });
    return true;
  }

  /** ⌘[ ⌘] previous / next pattern, ⇧⌘[ ⇧⌘] previous / next instrument, ⌘T add track, ⇧⌘T duplicate track,
      ⇧⌘N new pattern. Global like ⌘S: they also act from text fields, and the native menus show the same keys. */
  function globalStructureCommand(e) {
    if (!(e.metaKey || e.ctrlKey) || e.altKey || e.isComposing) return null;
    const key = String(e.key || '').toLowerCase();
    const bracket = e.code === 'BracketLeft' || e.code === 'BracketRight' || ['[', ']', '{', '}'].includes(key);
    if (bracket) {
      const next = e.code === 'BracketRight' || key === ']' || key === '}';
      return e.shiftKey ? (next ? 'nextInstrument' : 'previousInstrument') : (next ? 'nextPattern' : 'previousPattern');
    }
    if (key === 't') return e.shiftKey ? 'duplicateTrack' : 'addTrack';
    if (key === 'n' && e.shiftKey) return 'addPattern';
    return null;
  }

  function keyboard(e) {
    if (app.paramBurst
        && (e.target !== app.paramBurst.input || !PARAM_ARROWS.includes(e.key) || e.metaKey || e.ctrlKey || e.altKey)) endParamBurst();
    if (e.defaultPrevented) return;
    if (window.STBusy.isBusy('export')) {
      if (e.key === 'Escape') {
        e.preventDefault();
        window.STExport.cancel();
        return;
      }
      if (e.metaKey || e.ctrlKey) {
        e.preventDefault();
        window.STBusy.refuse('keys', ['export']);
        return;
      }
    }
    const cmd = e.metaKey || e.ctrlKey,
      key = e.key.toLowerCase(),
      input = isTextInput(e.target);
    if (cmd && e.shiftKey && key === 'r' && (!state.modal || state.modal === 'recording')) {
      e.preventDefault();
      if (!e.repeat) perform('recordInput');
      return;
    }
    if (cmd && key === 'k') {
      e.preventDefault();
      if (window.STBusy.refuse('palette', ['recording'])) return;
      state.modal === 'palette' ? closeModal() : openPalette();
      return;
    }
    if (e.key === 'Escape') {
      // Escape first abandons a half-typed grid value (app-entry.js) instead of stopping playback.
      if (!state.modal && !input && app.cancelEntry?.()) {
        e.preventDefault();
        return;
      }
      if (!state.modal) {
        const menu = $('.context-menu');
        if (menu) {
          e.preventDefault();
          menu.remove();
          return;
        }
        if (state.view === 'browser' && window.STBrowser?.key(e)) return;
        if (state.view === 'instrument' && window.STSampler?.key?.(e)) return;
        if ((state.view === 'mixer' || state.view === 'automation') && window.STStudio?.key(e)) return;
        // Inside a block, Esc first backs out to the block; at block level (or while playing) it stops as before.
        if (window.STBlockNav?.escape(e)) return;
      }
      e.preventDefault();
      if (state.modal) {
        if (window.STBusy.refuse('close', ['recording'])) return;
        closeModal();
      } else {
        stop();
      }
      return;
    }
    if (window.STRecording?.inlineActive
        && window.STRecording.isRecording
        && !input
        && !cmd
        && !e.altKey
        && keyNotes[key] !== undefined) {
      e.preventDefault();
      if (!e.repeat && $('#record-keyboard')?.checked) {
        const target = window.STRecording.keyboardTarget?.();
        if (!target?.blocked) noteOn(keyboardNote(keyNotes[key]), e.code, false,
            target?.instrument ?? state.instrument, target?.track ?? state.track, true);
      }
      return;
    }
    if (state.modal) {
      if (state.modal === 'recording'
          && window.STRecording?.isRecording
          && !input
          && !cmd
          && !e.altKey
          && $('#record-keyboard')?.checked
          && keyNotes[key] !== undefined) {
        e.preventDefault();
        if (!e.repeat) {
          const target = window.STRecording?.keyboardTarget?.();
          if (target?.blocked) return;
          noteOn(keyboardNote(keyNotes[key]), e.code, false, target?.instrument ?? state.instrument,
              target?.track ?? state.track, true);
        }
        return;
      }
      if (e.key === 'Tab') {
        const els = $$('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea,[tabindex="0"]',
            $('.modal')).filter(x => x.tabIndex >= 0 && !x.hidden && x.offsetParent !== null);
        const first = els[0],
          last = els[els.length - 1];
        // Focus trap: Tab from outside the dialog (e.g. after a click on its background) comes back into it.
        const dialog = $('.modal');
        if (dialog?.contains && !dialog.contains(document.activeElement)) {
          e.preventDefault();
          (e.shiftKey ? last : first)?.focus();
        } else if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last?.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first?.focus();
        }
      }
      if (optionLetter(e, 'a') && e.target.dataset.automate) {
        e.preventDefault();
        automate(e.target.dataset.owner ?? Number(e.target.dataset.track ?? state.track), e.target.dataset.automate,
            e.target.dataset.name, automateValue(e.target));
      }
      return;
    }
    if (window.STPaneNavigation?.key(e)) return;
    if (state.view === 'instrument' && window.STInstrumentPages?.key(e)) return;
    if (!cmd && !e.altKey && workspaceNavigation(e)) return;
    if (cmd) {
      if (key === 'x' && e.shiftKey && !e.altKey) {
        e.preventDefault();
        perform('sendEffects');
        return;
      }
      if (key === 'm' && e.shiftKey) {
        e.preventDefault();
        perform('metronome');
        return;
      }
      if (key === 'b' && e.shiftKey && !e.altKey) {
        e.preventDefault();
        perform('browseNKS');
        return;
      }
      // ⌘0: back to the pattern grid's cursor cell from anywhere (pane-navigation.js).
      if (key === '0' && !e.shiftKey && !e.altKey) {
        e.preventDefault();
        perform('focusGrid');
        return;
      }
      if (/^[1-7]$/.test(key)) {
        e.preventDefault();
        setView(views[Number(key) - 1][0]);
        return;
      }
      // ⌘⌫ deletes the selected tracks (pattern grid) or the selected strip's track or group (mixer strips).
      if ((e.key === 'Backspace' || e.key === 'Delete') && !e.shiftKey && !e.altKey && !input
          && (state.view === 'pattern' && (e.target === document.body || e.target.id === 'tracker' || e.target.closest?.('.tracker'))
              || state.view === 'mixer' && (e.target === document.body || e.target.closest?.('#mixer-channels')))) {
        e.preventDefault();
        if (!e.repeat) perform('deleteTracks');
        return;
      }
      // Structure shortcuts that work from every view (app-command-catalog.js GLOBAL_SHORTCUTS).
      const structure = globalStructureCommand(e);
      if (structure) {
        e.preventDefault();
        if (!e.repeat || /^(previous|next)/.test(structure)) perform(structure);
        return;
      }
      const actions = {
        s: e.shiftKey ? 'saveAs' : 'save',
        o: 'open',
        n: 'new',
        k: 'palette',
        p: 'keyboard',
        r: 'record',
        z: e.shiftKey ? 'redo' : 'undo'
      };
      if (actions[key] && !(key === 'z' && usesNativeTextUndo(e.target))) {
        e.preventDefault();
        if (actions[key] !== 'record' || !e.repeat) perform(actions[key]);
        return;
      }
      if (key === 'e' && e.shiftKey) {
        e.preventDefault();
        showExport();
        return;
      }
      if (key === 'f' && !(e.metaKey && e.ctrlKey)) {
        e.preventDefault();
        if (state.view === 'instrument' && window.STInstrumentNavigation) {
          window.STInstrumentNavigation.show();
          return;
        }
        if (state.view === 'browser') {
          window.STBrowser?.key(e);
          return;
        }
        if (state.view === 'automation') {
          const el = $('input[type=search]') || $('input[placeholder*="Search"]');
          el?.focus();
          el?.select();
          return;
        }
        openPalette();
        return;
      }
      if (!input && state.view === 'pattern') {
        if (key === 'c') {
          e.preventDefault();
          copySelection();
          return;
        }
        if (key === 'x') {
          e.preventDefault();
          copySelection(true);
          return;
        }
        if (key === 'v') {
          e.preventDefault();
          pasteSelection();
          return;
        }
        if (key === 'd') {
          e.preventDefault();
          fillSelection();
          return;
        }
        if (key === 'a') {
          e.preventDefault();
          state.selection = { track: 0, row: 0 };
          state.track = state.project.tracks.length - 1;
          state.row = pattern().length - 1;
          refresh();
          return;
        }
      }
    }
    if (optionLetter(e, 'a') && e.target.dataset.automate) {
      e.preventDefault();
      automate(e.target.dataset.owner ?? Number(e.target.dataset.track ?? state.track), e.target.dataset.automate,
          e.target.dataset.name, automateValue(e.target));
      return;
    }
    if (state.view === 'browser' && window.STBrowser?.key(e)) return;
    if (state.view === 'instrument' && window.STSampler?.key?.(e)) return;
    if ((state.view === 'mixer' || state.view === 'automation') && window.STStudio?.key(e)) return;
    if (state.view === 'song' && window.STClips?.key?.(e)) return;
    if (input) return;
    if (e.altKey && !e.metaKey && !e.ctrlKey && e.code === 'KeyG') {
      e.preventDefault();
      if (!e.repeat) cycleChordSet(e.shiftKey ? -1 : 1);
      return;
    }
    // INS / VEL / FX columns take typed values (app-entry.js); only the NOTE column plays notes.
    if (app.gridEntryKey?.(e)) return;
    if (window.STPatternTools?.key(e)) return;
    if (state.view === 'pattern' && (e.target.id === 'tracker' || e.target === document.body || e.target.closest('.tracker'))) {
      const arrows = e.key === 'ArrowLeft' || e.key === 'ArrowRight';
      if (cmd && e.shiftKey && !e.altKey && arrows) {
        e.preventDefault();
        if (!e.repeat) perform(e.key === 'ArrowLeft' ? 'moveTrackLeft' : 'moveTrackRight');
        return;
      }
      if (cmd && !e.altKey && e.code === 'KeyG') {
        e.preventDefault();
        if (!e.repeat) perform(e.shiftKey ? 'ungroupTrack' : 'groupTracks');
        return;
      }
      if (e.altKey && !cmd && (e.shiftKey && e.code === 'KeyJ' || !e.shiftKey && e.code === 'KeyU')) {
        e.preventDefault();
        if (!e.repeat) perform(e.code === 'KeyJ' ? 'toggleTrackCollapsed' : 'hideEmptyTracks');
        return;
      }
      if (e.altKey && !cmd && !e.shiftKey && (e.code === 'KeyJ' || e.code === 'KeyZ')) {
        e.preventDefault();
        if (!e.repeat) perform(e.code === 'KeyJ' ? 'toggleGroup' : 'padKeys');
        return;
      }
      if (e.key === 'Enter' && !cmd && !e.altKey && !e.shiftKey && window.STGroups?.ofTrack(state.project, state.track)?.collapsed) {
        e.preventDefault();
        app.toggleCollapse();
        return;
      }
    }
    if (state.keyboardPlay && !cmd && !e.altKey && keyNotes[key] !== undefined) {
      e.preventDefault();
      if (!e.repeat) {
        const note = keyboardNote(keyNotes[key]);
        if (!app.padKeyNote(note, e.code, true)) noteOn(note, e.code, !app.blocksRecording());
      }
      return;
    }
    if (e.key === ' ') {
      if (!e.shiftKey && e.target.closest('button,input[type=checkbox],input[type=radio],summary')) return;
      e.preventDefault();
      if (!e.repeat) {
        if (e.shiftKey) {
          perform('playFromStart');
        } else {
          togglePlay();
        }
      }
      return;
    }
    if (key === '?' || key === 'h' && e.shiftKey) {
      e.preventDefault();
      showHelp();
      return;
    }
    if ((key === '[' || key === ']') && !cmd) {
      e.preventDefault();
      state.octave = clamp(state.octave + (key === '[' ? -1 : 1), 0, 8);
      refresh();
      return;
    }
    if (key === 'r' && !cmd) {
      e.preventDefault();
      if (!e.repeat) toggleNoteRecordingFromUI();
      return;
    }
    if (state.view === 'pattern') {
      if (e.ctrlKey && e.key === 'Tab') {
        e.preventDefault();
        $('#step-note')?.focus();
        return;
      }
      const isGrid = e.target.id === 'tracker' || e.target === document.body || e.target.closest('.tracker');
      if (!isGrid && e.target.closest('button,input,select')) return;
      if (!cmd
          && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End'].includes(e.key)
          && (!e.altKey || e.key === 'ArrowUp' || e.key === 'ArrowDown')
          && queueStoppedGridKey(e.key, e.altKey, e.shiftKey)) {
        e.preventDefault();
        return;
      }
      if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
        e.preventDefault();
        transpose((e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 12 : 1));
        return;
      }
      if (['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown', 'Home', 'End'].includes(e.key)) {
        e.preventDefault();
        const dr = e.key === 'ArrowUp'
            ? -1
            : e.key === 'ArrowDown'
            ? 1
            : e.key === 'PageUp'
            ? -16
            : e.key === 'PageDown'
            ? 16
            : e.key === 'Home'
            ? -state.row
            : e.key === 'End'
            ? pattern().length - state.row - 1
            : 0;
        const dc = e.key === 'ArrowLeft' ? -1 : e.key === 'ArrowRight' ? 1 : 0;
        moveGrid(0, dr, dc, e.shiftKey);
        return;
      }
      if (e.key === 'Tab' && isGrid && !cmd && !e.altKey) {
        e.preventDefault();
        moveGrid(e.shiftKey ? -1 : 1, 0);
        return;
      }
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault();
        if (e.shiftKey) {
          shiftPatternRows(false);
        } else {
          clearSelection();
        }
        return;
      }
      if (e.key === 'Insert' || e.key === 'Enter' && e.shiftKey && !cmd && !e.altKey) {
        e.preventDefault();
        shiftPatternRows(true);
        return;
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        if (state.column >= 3 && window.STStepFX) {
          window.STStepFX.show({ fx: state.column - 2 });
          return;
        }
        // The cursor's note column with the row's FX (app-note-columns.js).
        const s = app.cellStep(pattern().steps[state.track][state.row], app.cursorNoteColumn());
        if (s && s.note >= 0) {
          auditionStep(s);
        } else {
          $('#step-note').focus();
        }
        return;
      }
      if ((key === '1' || key === '!') && state.column === 0 && !cmd && !e.altKey) {
        e.preventDefault();
        app.insertNoteOff(e.shiftKey);
        return;
      }
    }
    if (state.view === 'song' && !input && e.target.closest?.('#sequence-list') && !e.target.closest('button,select')) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const delta = e.key === 'ArrowDown' ? 1 : -1;
        if (e.altKey) {
          moveSequence(state.sequence, delta);
        } else {
          state.sequence = clamp(state.sequence + delta, 0, state.project.sequence.length - 1);
          refresh();
          $('#sequence-list').focus({ preventScroll: true });
        }
        return;
      }
      if (e.key === 'Enter' && !e.target.closest('button,select')) {
        e.preventDefault();
        state.project.activePattern = state.project.sequence[state.sequence];
        setView('pattern');
        return;
      }
      if ((e.key === 'Delete' || e.key === 'Backspace') && !e.target.closest('button')) {
        e.preventDefault();
        removeSequence(state.sequence);
        return;
      }
      if (cmd && key === 'd') {
        e.preventDefault();
        mutate('duplicate sequence position', p => {
          p.sequence.splice(state.sequence + 1, 0, p.sequence[state.sequence]);
          state.sequence++;
        });
      }
    }
    // Arrows nobody used on a button-like control: the nearest control in that direction within its block.
    window.STBlockNav?.arrow(e);
  }

  Object.assign(app, { isTextInput, usesNativeTextUndo, keyboard, globalStructureCommand });
})();
