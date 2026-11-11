/* Typed value entry, tracker style. In the pattern grid the INS, VEL and FX columns take typed values instead of
   playing notes (only the NOTE column plays and enters notes): INS takes two digits, VEL up to three, and an FX
   column takes an effect code letter followed by its value in the effect's own units (R04 ratchet ×4, Y50
   probability 50%, P-30 pan 30% left …). An entry commits as one Undo step when its digits are complete (or on
   Enter, Tab, an arrow key or a click), then the cursor advances by the edit step. The same codes are typed into
   the step editor's FX code boxes ("R4", "rat 4", "rev"), and its Ins box takes a number or a name prefix. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, clamp, esc, state, pattern, mutate, refresh, openModal } = app;

  /* One code table for every step effect. unit: how the value is typed and shown.
       percent  0–100 (pan −100–100) of the stored 0–1 value · semis  whole semitones · hz  cutoff in Hz
       count    whole numbers · hex  two hex digits (arpeggio: +x +y semitones) · bool  0 or 1
     Codes follow Renoise where they exist (A arpeggio, B backwards, G glide, R retrigger, Y maybe, Q delay, P pan,
     L level). */
  const FX_CODES = [
    ['A', 'arpeggio', 'Arpeggio', 'hex', 'two hex digits: +x +y semitones (37 = minor, 47 = major)'],
    ['B', 'reverse', 'Reverse (backwards)', 'bool', '1 backwards, 0 forwards'],
    ['C', 'cutoff', 'Filter cutoff', 'hz', '20–20000 Hz'],
    ['D', 'delay', 'Delay send', 'percent', '0–100%'],
    ['E', 'resonance', 'Filter resonance (emphasis)', 'percent', '0–95%'],
    ['G', 'glide', 'Pitch glide', 'semis', '−24…24 semitones'],
    ['H', 'reverb', 'Reverb send (hall)', 'percent', '0–100%'],
    ['L', 'volume', 'Volume (level)', 'percent', '0–100%'],
    ['O', 'offset', 'Sample offset', 'percent', '0–99% of the sample'],
    ['P', 'pan', 'Pan', 'percent', '−100 left … 100 right'],
    ['Q', 'microtiming', 'Microtiming (delay the note)', 'percent', '0–95% of the row'],
    ['R', 'ratchet', 'Ratchet (retrigger)', 'count', '1–8 hits per row'],
    ['S', 'slice', 'Sample slice', 'count', '1–128'],
    ['T', 'pitch', 'Pitch (transpose)', 'semis', '−24…24 semitones'],
    ['W', 'position', 'Wavetable position', 'percent', '0–100%'],
    ['X', 'plugin', 'Plug-in parameter lock (value)', 'percent', '0–100% of an existing lock; choose the parameter with Enter'],
    ['Y', 'probability', 'Probability (maybe)', 'percent', '0–100% chance']
  ].map(([code, type, label, unit, range]) => ({ code, type, label, unit, range }));
  const BY_CODE = new Map(FX_CODES.map(c => [c.code, c])), BY_TYPE = new Map(FX_CODES.map(c => [c.type, c]));
  const fxCode = type => BY_TYPE.get(type) || null;

  // Typed number ↔ stored value.
  const limits = info => info.type === 'plugin' ? [0, 1] : app.FX[info.type] || [0, 1];
  function typedRange(info) {
    const [min, max] = limits(info);
    if (info.unit === 'percent') return [Math.round(min * 100), Math.round(max * 100)];
    if (info.unit === 'hex') return [0, 255];
    return [min, max];
  }
  function toValue(info, typed) {
    const [min, max] = limits(info);
    if (info.unit === 'percent') return clamp(Math.round(typed) / 100, min, info.type === 'offset' ? .99 : max);
    return clamp(typed, min, max);
  }
  function typedOf(info, value) {
    const v = Number(value) || 0;
    if (info.unit === 'percent') return Math.round(v * 100);
    return Math.round(v);
  }
  const pad2 = n => String(Math.abs(n)).padStart(2, '0');
  function valueText(info, value) {
    const n = typedOf(info, value);
    if (info.unit === 'hex') return clamp(n, 0, 255).toString(16).toUpperCase().padStart(2, '0');
    if (info.unit === 'hz') return String(n);
    return (n < 0 ? '-' : '') + pad2(n);
  }
  // Cell text: code + value, e.g. R04, Y50, P-30, C2400, A37.
  function fxCodeText(fx) {
    if (!fx) return '·· ··';
    const info = fxCode(fx.type);
    return info ? info.code + valueText(info, fx.value) : String(fx.type).slice(0, 3).toUpperCase();
  }
  function unitText(info, value) {
    const n = typedOf(info, value);
    return info.unit === 'percent' ? n + '%' : info.unit === 'semis' ? (n > 0 ? '+' : '') + n + ' st' : info.unit === 'hz' ? n + ' Hz'
        : info.unit === 'hex' ? `+${n >> 4} +${n & 15}` : String(n);
  }
  function fxTitle(fx) {
    if (!fx) return '';
    const info = fxCode(fx.type);
    if (!info) return fx.type;
    return `${fx.type === 'plugin' ? fx.name || info.label : info.label} ${unitText(info, fx.value)} · code ${info.code} (${info.range})`;
  }

  /* Parse a typed effect: "R4", "R04", "Y50", "P-30", "rat 4", "ratchet 4", "rev", "reverse 1". A bare name or
     code without a value uses a sensible default (Reverse 1, Ratchet 2, others the middle). Returns { fx } or
     { error } with a hint. */
  function parseFX(text, existing = null) {
    const raw = String(text ?? '').trim();
    if (!raw) return { clear: true };
    let match = raw.match(/^([A-Za-z])\s*(-?[0-9A-Fa-f]*)$/), info = null, digits = '';
    if (match && BY_CODE.has(match[1].toUpperCase())) {
      info = BY_CODE.get(match[1].toUpperCase());
      digits = match[2];
    } else {
      match = raw.match(/^([A-Za-zµ ]+?)\s*(-?[0-9A-Fa-f]*)$/);
      const word = (match?.[1] || raw).trim().toLowerCase();
      const found = FX_CODES.filter(c => c.type.startsWith(word) || c.label.toLowerCase().startsWith(word));
      if (!match || !word || !found.length) return { error: `Unknown effect “${raw}”. Type a code and value like R4 or Y50, or a name like “rat 4”. ${codesHint()}` };
      if (found.length > 1 && !found.some(c => c.type === word) && !ALIASES[word]) {
        return { error: `“${word}” matches ${found.map(c => c.label).join(', ')}. Type more letters or the code (${found.map(c => c.code).join(', ')}).` };
      }
      info = found.find(c => c.type === word) || BY_CODE.get(ALIASES[word]) || found[0];
      digits = match[2];
    }
    if (info.type === 'plugin' && existing?.type !== 'plugin') {
      return { error: 'X sets the value of a plug-in parameter lock already on this step. Choose the parameter with Enter (FX picker) first.' };
    }
    if (digits && info.unit !== 'hex' && !/^-?\d+$/.test(digits)) return { error: `${info.label} takes ${info.range}.` };
    let typed;
    if (!digits) typed = info.type === 'reverse' ? 1 : info.type === 'ratchet' ? 2 : info.type === 'cutoff' ? 1000
        : info.type === 'plugin' ? typedOf(info, existing.value) : Math.round((typedRange(info)[0] + typedRange(info)[1]) / 2);
    else typed = info.unit === 'hex' ? parseInt(digits, 16) : Number(digits);
    const [min, max] = typedRange(info);
    if (!Number.isFinite(typed) || typed < min || typed > max) return { error: `${info.label} takes ${info.range} (got ${digits}).` };
    const fx = info.type === 'plugin' ? { ...existing, value: toValue(info, typed) } : { type: info.type, value: toValue(info, typed) };
    return { fx };
  }
  // Short names that would otherwise be ambiguous ("rev" is Reverse, not Reverb).
  const ALIASES = { rev: 'B', back: 'B', verb: 'H', del: 'D', vol: 'L', res: 'E', pit: 'T', pos: 'W', off: 'O', prob: 'Y', micro: 'Q' };
  const codesHint = () => 'Codes: ' + FX_CODES.map(c => c.code + ' ' + c.type).join(', ') + '.';

  // Instrument by typed number ("3", "03") or name prefix ("bass", "03 Velvet bass").
  function parseInstrument(text, track = state.track, project = state.project) {
    const raw = String(text ?? '').trim(), instruments = project.instruments;
    if (!raw) return { error: 'Type an instrument number or the start of its name.' };
    let index = -1;
    const number = raw.match(/^(\d{1,3})(?:\s.*)?$/);
    if (number) {
      index = Number(number[1]);
      if (index >= instruments.length) return { error: `Instrument ${number[1]} does not exist. Use 00–${String(instruments.length - 1).padStart(2, '0')}.` };
    } else {
      const word = raw.toLowerCase(), found = instruments.map((inst, i) => [inst, i]).filter(([inst]) => String(inst.name || '').toLowerCase().startsWith(word));
      if (!found.length) return { error: `No instrument name starts with “${raw}”.` };
      index = found[0][1];
    }
    const inst = instruments[index];
    if (inst?.type === 'plugin' && index !== (project.tracks[track]?.instrument ?? 0)) {
      return { error: `Instrument ${String(index).padStart(2, '0')} “${inst.name}” is a plug-in on another track; plug-ins play only on their own track.` };
    }
    return { index };
  }

  // ---------------------------------------------------------------- grid entry
  let entry = null;   // { column, noteColumn, track, row, patternID, code?, text }

  // The cell's span for the entry's sub-column (extra note columns carry data-note-column, app-note-columns.js).
  const cellSpan = () => {
    if (!entry) return null;
    if (!entry.noteColumn) return document.querySelector?.(`[data-cell="${entry.track}:${entry.row}"] [data-column="${entry.column}"]`);
    return document.querySelector?.(`[data-cell="${entry.track}:${entry.row}"] [data-column="${entry.column}"][data-note-column="${entry.noteColumn}"]`);
  };
  function paintPending() {
    const span = cellSpan();
    if (!span) return;
    span.textContent = (entry.code || '') + entry.text + '_';
    span.classList?.add('typing');
  }
  const fieldName = column => column === 1 ? 'instrument' : column === 2 ? 'velocity' : 'fx' + (column - 2);

  function advance() {
    const pat = pattern();
    if (state.editStep) state.row = (state.row + state.editStep) % pat.length;
  }

  // Commit the pending entry (one Undo step). Returns true when it changed the song.
  function finishEntry(options = {}) {
    const current = entry;
    entry = null;
    if (!current) return false;
    const pat = state.project.patterns.find(p => p.id === current.patternID), steps = pat?.steps[current.track];
    if (!steps || state.project.tracks[current.track] == null) return false;
    const step = steps[current.row];
    const reject = message => {
      app.toast(message, 'info');
      if (options.render !== false) refresh();
      return false;
    };
    if (current.column === 1 || current.column === 2) {
      if (!current.text) return options.render !== false ? (refresh(), false) : false;
      const n = current.noteColumn || 0, cell = n ? app.noteCell(step, n) : app.isColumnHolder?.(step) ? null : step;
      if (!cell) return reject(`Enter a note on this row first (NOTE column), then type its ${current.column === 1 ? 'instrument' : 'velocity'}.`);
      // An extra note column's cell (app-note-columns.js), or the step itself.
      const write = values => n ? app.setColumnCell(steps, current.row, n, { ...cell, ...values }) : steps[current.row] = { ...steps[current.row], ...values };
      if (current.column === 1) {
        const result = parseInstrument(current.text, current.track);
        if (result.error) return reject(result.error);
        mutate('type instrument', () => { write({ instrument: result.index }); });
      } else {
        const velocity = Number(current.text);
        if (!(velocity >= 1 && velocity <= 127)) return reject(`Velocity is 1–127 (got ${current.text}).`);
        mutate('type velocity', () => { write({ velocity }); });
      }
    } else {
      const key = fieldName(current.column), info = BY_CODE.get(current.code);
      if (!info) return false;
      if (!current.text && info.unit !== 'bool' && info.type !== 'ratchet' && info.type !== 'plugin') {
        return reject(`Type a value after ${info.code}: ${info.label}, ${info.range}.`);
      }
      const parsed = parseFX(info.code + current.text, step?.[key]);
      if (parsed.error) return reject(parsed.error);
      mutate('type step effect', () => {
        const target = steps[current.row] || { note: -3, instrument: state.project.tracks[current.track].instrument, velocity: 100, gate: 0 };
        steps[current.row] = { ...target, [key]: parsed.fx };
      });
      if (!(step?.note >= 0) && ['probability', 'glide', 'microtiming', 'arpeggio', 'slice'].includes(info.type)) {
        app.toast(`${info.label} modifies a note trigger. Add a note on this row to hear it.`, 'info');
      }
    }
    if (options.advance !== false && state.track === current.track && state.row === current.row) advance();
    if (options.render !== false) refresh();
    return true;
  }

  // Digits are complete when no further digit could stay in range (leading zeros do not count: R04 = R4).
  function wholeNumberComplete(digits, max) {
    const significant = digits.replace(/^0+/, '');
    return significant.length > 0 && (significant.length >= String(max).length || Number(digits + '0') > max);
  }
  function complete(info, text) {
    const digits = text.replace('-', '');
    if (info.unit === 'hex') return digits.length >= 2;
    if (info.unit === 'bool') return digits.length >= 1;
    return wholeNumberComplete(digits, Math.max(Math.abs(typedRange(info)[0]), Math.abs(typedRange(info)[1])));
  }

  /* The grid key hook (app-keyboard.js), before the piano: true when the key was handled. Only the NOTE column
     plays notes; INS / VEL / FX swallow piano keys, so typing a value never sounds a note. */
  function gridEntryKey(e) {
    if (state.view !== 'pattern' || state.column < 1 || state.column > 4) return finishOn(e);
    const target = e.target;
    if (!(target?.id === 'tracker' || target === document.body || target?.closest?.('.tracker'))) return false;
    if (e.metaKey || e.ctrlKey || e.altKey) return finishOn(e);
    if (window.STGroups?.ofTrack(state.project, state.track)?.collapsed) return false;
    const column = state.column, key = e.key, upper = key.length === 1 ? key.toUpperCase() : '';
    const noteColumn = app.cursorNoteColumn?.() || 0;
    const here = entry && entry.column === column && (entry.noteColumn || 0) === noteColumn && entry.track === state.track && entry.row === state.row
        && entry.patternID === pattern().id;
    if (entry && !here) finishEntry({ advance: false, render: false });
    const begin = (extra = {}) => {
      entry = { column, noteColumn, track: state.track, row: state.row, patternID: pattern().id, text: '', ...extra };
    };
    if (column <= 2) {
      if (/^[0-9]$/.test(key)) {
        e.preventDefault();
        if (!here || !entry) begin();
        entry.text += key;
        const max = column === 1 ? state.project.instruments.length - 1 : 127, width = column === 1 ? 2 : 3;
        if (entry.text.length >= width || wholeNumberComplete(entry.text, max)) finishEntry();
        else paintPending();
        return true;
      }
      if (entry && (key === 'Backspace' || key === 'Delete')) {
        e.preventDefault();
        entry.text = entry.text.slice(0, -1);
        if (!entry.text) { entry = null; refresh(); } else paintPending();
        return true;
      }
      if (entry && (key === 'Enter' || key === 'Tab' || key === 'Escape')) {
        e.preventDefault();
        if (key === 'Escape') { entry = null; refresh(); } else finishEntry();
        return true;
      }
      if (entry) finishEntry({ advance: false });
      // A piano key in a value column never plays a note.
      if (state.keyboardPlay && app.keyNotes[key.toLowerCase()] !== undefined && key !== ' ') {
        e.preventDefault();
        return true;
      }
      return false;
    }
    // FX columns: a code letter, then the value.
    const typing = here && entry?.code;
    const info = typing ? BY_CODE.get(entry.code) : null;
    if (typing && (info.unit === 'hex' ? /^[0-9a-fA-F]$/.test(key) : /^[0-9]$/.test(key) || key === '-' && !entry.text && info.unit === 'semis'
        || key === '-' && !entry.text && info.type === 'pan')) {
      e.preventDefault();
      entry.text += key.toUpperCase();
      if (complete(info, entry.text)) finishEntry();
      else paintPending();
      return true;
    }
    if (/^[a-zA-Z]$/.test(key)) {
      e.preventDefault();
      if (here && entry) finishEntry({ advance: false, render: false });
      if (!BY_CODE.has(upper)) {
        app.toast(`No effect code “${upper}”. ${codesHint()} ⌘K “Effect codes” lists them.`, 'info');
        return true;
      }
      const step = pattern().steps[state.track]?.[state.row], code = BY_CODE.get(upper), slot = step?.['fx' + (column - 2)];
      if (code.type === 'plugin' && slot?.type !== 'plugin') {
        app.toast('X sets the value of a plug-in parameter lock already on this step. Press Enter to choose a plug-in parameter.', 'info');
        return true;
      }
      begin({ code: upper });
      paintPending();
      return true;
    }
    if (typing && (key === 'Backspace' || key === 'Delete')) {
      e.preventDefault();
      if (entry.text) entry.text = entry.text.slice(0, -1);
      else entry.code = '';
      if (!entry.code) { entry = null; refresh(); } else paintPending();
      return true;
    }
    if (typing && (key === 'Enter' || key === 'Tab')) {
      e.preventDefault();
      finishEntry();
      return true;
    }
    if (typing && key === 'Escape') {
      e.preventDefault();
      entry = null;
      refresh();
      return true;
    }
    if (typing) finishEntry({ advance: false });
    // Backspace / Delete on an FX cell (no selection) clears just that effect.
    if ((key === 'Backspace' || key === 'Delete') && !e.shiftKey && !state.selection) {
      const steps = pattern().steps[state.track], step = steps?.[state.row], slot = 'fx' + (column - 2);
      if (step?.[slot]) {
        e.preventDefault();
        mutate('clear step effect', () => {
          const next = { ...steps[state.row] };
          delete next[slot];
          steps[state.row] = app.dropEmptyStep(next);
        });
        return true;
      }
    }
    if (/^[0-9]$/.test(key) || state.keyboardPlay && app.keyNotes[key.toLowerCase()] !== undefined && key !== ' ') {
      e.preventDefault();
      if (/^[0-9]$/.test(key)) app.toast('Type an effect code letter first, then its value (e.g. R4, Y50). ⌘K “Effect codes” lists them.', 'info');
      return true;
    }
    return false;
  }
  function cancelEntry() {
    if (!entry) return false;
    entry = null;
    refresh();
    return true;
  }
  // Any other key, or a key outside the value columns, completes a pending entry first.
  function finishOn() {
    if (entry) finishEntry({ advance: false });
    return false;
  }

  // ---------------------------------------------------------------- step editor
  function renderCodeField(number, fx) {
    const value = fx ? fxCodeText(fx) : '';
    return `<input id="step-fx${number}-code" class="step-fx-code" type="text" list="step-fx-codes" autocomplete="off" spellcheck="false"`
        + ` aria-label="Effect ${number} code: type a code and value like R4 or Y50, or a name like rat 4"`
        + ` title="${esc(fx ? fxTitle(fx) : 'Type an effect: code + value (R4, Y50, P-30) or a name (rat 4, rev). Empty removes it.')}"`
        + ` placeholder="R4…" value="${esc(value)}" size="5">`;
  }
  function codeDatalist() {
    return `<datalist id="step-fx-codes">${FX_CODES.filter(c => c.type !== 'plugin').map(c => `<option value="${c.code}">${esc(c.label)} · ${esc(c.range)}</option>`
        + `<option value="${esc(c.type)} ">${c.code} · ${esc(c.range)}</option>`).join('')}</datalist>`;
  }
  function instrumentDatalist() {
    return `<datalist id="step-instrument-list">${state.project.instruments.map((inst, n) => `<option value="${String(n).padStart(2, '0')} ${esc(inst.name)}"></option>`).join('')}</datalist>`;
  }
  function instrumentText(index) {
    const inst = state.project.instruments[index];
    return `${String(index).padStart(2, '0')} ${inst?.name || ''}`.trim();
  }

  function bindStepEditor() {
    const step = () => pattern().steps[state.track]?.[state.row];
    const owner = { pattern: pattern(), track: state.track, row: state.row };
    const owns = () => pattern() === owner.pattern && state.track === owner.track && state.row === owner.row;
    const leave = (input, e) => {
      if (e.key === 'Enter' || e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation?.();
        if (e.key === 'Escape') input.value = input.defaultValue ?? input.value;
        input.dataset.escaped = e.key === 'Escape' ? '1' : '';
        input.blur();
        $('#tracker')?.focus({ preventScroll: true });
      }
    };
    const ins = $('#step-instrument');
    if (ins && ins.tagName !== 'SELECT') {
      ins.onkeydown = e => leave(ins, e);
      ins.onchange = () => {
        if (ins.dataset.escaped === '1' || !owns()) { ins.dataset.escaped = ''; return; }
        const result = parseInstrument(ins.value);
        if (result.error) {
          app.toast(result.error, 'info');
          ins.value = instrumentText(app.cursorCell()?.instrument ?? state.instrument);
          return;
        }
        mutate('edit step', () => { app.ensureCell().instrument = result.index; });
      };
    }
    for (const number of [1, 2]) {
      const input = $(`#step-fx${number}-code`);
      if (!input) continue;
      input.onkeydown = e => leave(input, e);
      input.onchange = () => {
        if (input.dataset.escaped === '1' || !owns()) { input.dataset.escaped = ''; return; }
        const key = 'fx' + number, current = step()?.[key] || null;
        if (current && input.value.trim() === fxCodeText(current)) return;
        const parsed = parseFX(input.value, current);
        if (parsed.error) {
          app.toast(parsed.error, 'info');
          input.value = current ? fxCodeText(current) : '';
          return;
        }
        if (parsed.clear) {
          if (!current) return;
          mutate('remove step effect', () => {
            const steps = pattern().steps[state.track], next = { ...steps[state.row] };
            delete next[key];
            steps[state.row] = app.dropEmptyStep(next);
          });
          return;
        }
        mutate('set step effect', () => { app.ensureStep(-3)[key] = parsed.fx; });
      };
    }
  }

  // ---------------------------------------------------------------- reference
  const effectCodeRows = () => FX_CODES.map(c => [`${c.label} · ${c.range}`, c.code]);
  function showEffectCodes() {
    openModal('Effect codes', `<p class="muted">In an FX column type the code, then the value; it commits when its digits are complete or on `
        + `Enter, and the cursor moves on by the edit step. The step editor's FX boxes take the same codes or names (“rat 4”, “rev”). `
        + `Delete clears the effect. Enter on an FX cell opens the picker.</p><table class="effect-codes"><tbody>${FX_CODES.map(c => `<tr><th>${c.code}</th>`
        + `<td>${esc(c.label)}</td><td>${esc(c.range)}</td><td><code>${esc(c.code + (c.unit === 'bool' ? '1' : c.unit === 'hex' ? '37' : c.type === 'ratchet' ? '04'
            : c.type === 'pan' ? '-30' : c.unit === 'hz' ? '2400' : c.unit === 'semis' ? '-12' : '50'))}</code></td></tr>`).join('')}</tbody></table>`, '', { kind: 'help' });
    return true;
  }

  app.defineCommands?.({ effectCodes: showEffectCodes });

  Object.assign(app, {
    FX_CODES, cancelEntry, fxCode, fxCodeText, fxTitle, parseFX, parseInstrument, gridEntryKey, finishEntry, renderCodeField, codeDatalist,
    instrumentDatalist, instrumentText, bindStepEditor, showEffectCodes, effectCodeRows
  });
  // The half-typed grid value, if any (read-only copy).
  Object.defineProperty(app, 'gridEntry', { configurable: true, get: () => entry ? { ...entry } : null });
})();
