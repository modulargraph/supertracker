/* MK3: page model and lists (one list behaviour for every screen), step and effect knobs. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, now, noteText, FX_SHORT, FX_NAMES, M, active, pattern, flash, scheduleRefresh, setCursor,
    selectColumn, toggleColumn, moveNoteLane, moveColumn, perform, ensureStep, stepAt, patternIndex, burstValue,
    burstEdit, numKnob, intKnob, enumKnob, actionKnob, button, fmt
  } = mk;

  // ---- Pages --------------------------------------------------------------------------------------------
  // Each page: { id, title, buttons[8], knobs[8], main:{turn,push,tilt}, data } — the screens and the
  // display buttons / encoders always belong to the same page object.
  function pageIndex(id, count) { const value = clamp(M.page[id] || 0, 0, Math.max(0, count - 1)); M.page[id] = value; return value; }

  // ---- Lists (one behaviour for every list on the screens; docs/MASCHINE_MK3.md "Lists") ---------------
  // LIST_ROWS rows are visible. While a list fits, the cursor moves freely; a longer list keeps the cursor
  // LIST_MARGIN rows away from the top/bottom edge and scrolls instead ("scrolloff"), except at the ends.
  const LIST_ROWS = window.STMK3Screens.LIST_ROWS;
  const LIST_MARGIN = 2;
  /** First visible row for `cursor` in a list of `count`, continuing from the previous `top` (undefined: centre). */
  function listWindow(count, cursor, top, rows = LIST_ROWS, margin = LIST_MARGIN) {
    count = Math.max(0, Math.trunc(Number(count) || 0)); rows = Math.max(1, Math.trunc(Number(rows) || 1));
    if (count <= rows) return 0;
    const m = clamp(Math.trunc(Number(margin) || 0), 0, Math.floor((rows - 1) / 2)), c = clamp(Math.trunc(Number(cursor) || 0), 0, count - 1);
    let t = Number.isFinite(top) ? Math.trunc(top) : c - Math.floor(rows / 2);
    if (c < t + m) t = c - m;
    if (c > t + rows - 1 - m) t = c - (rows - 1 - m);
    return clamp(t, 0, count - rows);
  }
  /** Window of the list `key` (remembered between frames, so the cursor travels before the list scrolls). */
  function listView(key, count, cursor, rows = LIST_ROWS) {
    const top = listWindow(count, cursor, M.listTops.get(key), rows);
    M.listTops.set(key, top);
    return { top, rows, listKey: key };
  }
  /** Main-encoder detents → list steps: Shift jumps a page; quick consecutive turns accelerate on long lists. */
  function listSteps(delta, list) {
    const direction = Math.sign(delta), rows = list.rows || LIST_ROWS, count = Number(list.count) || 0;
    if (!direction) return 0;
    if (M.shift) return delta * rows;
    const run = M.mainRun, t = now(), gap = t - run.at;
    run.count = direction === run.direction && gap < 160 ? run.count + 1 : 1;
    run.at = t; run.direction = direction;
    if (count <= rows * 3 || run.count < 3) return delta;
    const factor = gap < 25 ? 8 : gap < 45 ? 4 : gap < 80 ? 2 : 1;
    return delta * Math.min(factor, Math.max(2, Math.floor(count / 25)));
  }
  /** The 4-D encoder on a list page: turn moves (accelerated), push chooses, tilt ▲▼ pages, tilt ◄► changes column. */
  function listMain({ count, rows = LIST_ROWS, move, choose, release, column }) {
    return { list: { count, rows },
      turn(delta) { move(delta); },
      push(down) { if (down) choose?.(); else release?.(); },
      tilt(direction) { if (direction === 'up'
          || direction === 'down') move((direction === 'down' ? 1 : -1) * rows); else column?.(direction === 'right' ? 1 : -1); } };
  }
  /** List data for the screens: items plus the visible window, cursor and the index label ("12 / 340"). */
  function listData(key, items, selected, extra = {}) {
    const rows = extra.rows || LIST_ROWS, count = items.length, cursor = clamp(Number(selected) || 0, 0, Math.max(0, count - 1));
    return { kind: 'list', items, selected: cursor, ...listView(key, count, cursor, rows), ...extra };
  }
  function cursorMain() {
    return {
      turn(delta) { const step = M.shift ? 4 * Math.max(1, st().editStep || 1) : 1; setCursor({ row: st().row + delta * step }); },
      push(down) { if (down) S().auditionStep(); },
      tilt(direction) {
        if (direction === 'left' || direction === 'right') {
          moveColumn(direction === 'right' ? 1 : -1);
        } else {
          moveNoteLane(direction === 'down' ? 1 : -1);
        }
      }
    };
  }
  function stepKnobs(rows = null) {
    // Note / instrument / velocity / gate of the cursor step (or of held step pads).
    const track = st().track, pat = pattern(), patternId = pat.id, targets = rows || [st().row];
    const first = targets[0];
    const read = field => p => {
      const s = p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first];
      return s ? s[field] : null;
    };
    const writeAll = (field, transform) => (p, value) => {
      const target = p.patterns.find(item => item.id === patternId);
      if (!target) return;
      for (const row of targets) {
        const s = target.steps[track][row];
        if (s) transform(s, value, field);
      }
    };
    const key = field => `step:${patternId}:${track}:${targets.join(',')}:${field}`;
    const s = stepAt(track, first, pat);
    const knobs = [];
    knobs.push(s && s.note >= 0 ? intKnob({
      key: key('note'),
      label: 'Note',
      read: p => read('note')(p) ?? 60,
      write: writeAll('note', (st_, v) => {
        if (st_.note >= 0) st_.note = v;
      }),
      min: 0,
      max: 127,
      fmt: noteText,
      detent: .025
    }) : actionKnob('Note', s ? noteText(s.note) : '···', direction => {
      if (!s) {
        addStep(track, first);
      }
    }));
    knobs.push(s
        ? intKnob({ key: key('instrument'), label: 'Inst', read: p => read('instrument')(p) ?? 0, write: writeAll('instrument',
            (step, v) => { step.instrument = v; }), min: 0, max: P().instruments.length - 1, fmt: v => `${String(v).padStart(2, '0')} ${P().instruments[v]?.name || ''}`.slice(0,
            14), detent: .05 })
        : null);
    knobs.push(s ? intKnob({
      key: key('velocity'),
      label: 'Velocity',
      read: p => read('velocity')(p) ?? 100,
      write: writeAll('velocity', (step, v) => {
        step.velocity = v;
      }),
      min: 1,
      max: 127,
      fmt: fmt.int,
      detent: .008
    }) : null);
    // Gate is optional: fully down is none (the note plays until the next note, OFF or CUT on its track).
    const NO_GATE = .01;
    knobs.push(s ? numKnob({
      key: key('gate'),
      label: 'Gate',
      read: p => S().stepGate(p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first]) ?? NO_GATE,
      write: writeAll('gate', (step, v) => {
        step.gate = v <= NO_GATE ? 0 : v;
      }),
      min: NO_GATE,
      max: 256,
      curve: 'log',
      fmt: v => v <= NO_GATE ? '— none' : `${v.toFixed(2)} rows`
    }) : null);
    return knobs;
  }
  function fxKnobs(column, rows = null) {
    const track = st().track,
      pat = pattern(),
      patternId = pat.id,
      targets = rows || [st().row],
      first = targets[0],
      fxKey = 'fx' + column;
    const s = stepAt(track, first, pat), fx = s?.[fxKey];
    const types = ['', ...Object.keys(S().FX)];
    const typeKnob = enumKnob({ key: `fxtype:${patternId}:${track}:${targets.join(',')}:${column}`, label: `FX${column} type`, read: p => p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first]?.[fxKey]?.type
        || '', options: types.map(type => [type, type ? FX_NAMES[type] || type : 'None']), detent: .06,
      write: (p, type) => { const target = p.patterns.find(item => item.id === patternId); if (!target) return; for (const row of targets) { let step = target.steps[track][row]; if (!type) { if (step) { delete step[fxKey]; if (step.note === -3
          && !step.fx1
          && !step.fx2 && !step.columns) target.steps[track][row] = null; } continue; } if (!step) step = ensureStep(p, target, track, row); if (step[fxKey]?.type === type) continue; const range = S().FX[type]; step[fxKey] = { type, value: type === 'ratchet'
          ? 2
          : type === 'probability'
          ? .5
          : clamp(range[0] < 0 ? 0 : range[0] + (range[1] - range[0]) / 2, range[0], range[1]) }; } } });
    let valueKnob = null;
    if (fx && fx.type !== 'plugin') {
      const [min, max, step] = S().FX[fx.type] || [0, 1, .01];
      const read = p => Number(p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first]?.[fxKey]?.value ?? min);
      const write = (p, v) => {
        const target = p.patterns.find(item => item.id === patternId);
        for (const row of targets) {
          const st_ = target?.steps[track][row];
          if (st_?.[fxKey]?.type === fx.type) st_[fxKey].value = v;
        }
      };
      const label = `FX${column} ${FX_SHORT[fx.type] || ''}`;
      valueKnob = step >= 1 ? intKnob({ key: `fxval:${patternId}:${track}:${targets.join(',')}:${column}`, label, read, write, min, max, fmt: v => fx.type === 'arpeggio'
          ? Math.round(v).toString(16).toUpperCase().padStart(2, '0')
          : String(v), detent: fx.type === 'arpeggio' ? .012 : .04 })
        : numKnob({ key: `fxval:${patternId}:${track}:${targets.join(',')}:${column}`, label, read, write, min, max, curve: fx.type === 'cutoff' ? 'log' : 'lin', step, fmt: v => fx.type === 'cutoff'
            ? fmt.hz(v)
            : fx.type === 'pan'
            ? fmt.pan(v)
            : `${Math.round(v * 100)}%` });
    } else if (fx?.type === 'plugin') {
      valueKnob = numKnob({
        key: `fxval:${patternId}:${track}:${first}:${column}`,
        label: `FX${column} PLG`,
        name: fx.name || 'Plug-in parameter',
        read: p => Number(p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first]?.[fxKey]?.value ?? 0),
        write: (p, v) => {
          const st_ = p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[first];
          if (st_?.[fxKey]) st_[fxKey].value = v;
        },
        min: 0,
        max: 1,
        fmt: fmt.pct
      });
    }
    return [typeKnob, valueKnob];
  }
  function typedFxKnob(type, label) {
    // Edit (or create in a free FX column) the effect of one type on the cursor step; None at minimum.
    const track = st().track, pat = pattern(), patternId = pat.id, row = st().row;
    const [min, max, step] = S().FX[type];
    const find = s => ['fx1', 'fx2'].find(key => s?.[key]?.type === type);
    const read = p => {
      const s = p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[row];
      const key = find(s);
      return key ? s[key].value : null;
    };
    const value = burstValue(`typed:${patternId}:${track}:${row}:${type}`, read);
    const write = (p, v) => {
      const target = p.patterns.find(item => item.id === patternId); if (!target) return;
      let s = target.steps[track][row]; const key = find(s);
      if (v === null) { if (key) { delete s[key]; if (s.note === -3 && !s.fx1 && !s.fx2 && !s.columns) target.steps[track][row] = null; } return; }
      if (!s) { if (['probability', 'ratchet', 'microtiming', 'glide', 'reverse', 'offset', 'arpeggio',
          'slice'].includes(type)) return; s = ensureStep(p, target, track, row); }
      const slot = key || ['fx1', 'fx2'].find(k => !s[k]); if (!slot) return;
      s[slot] = { type, value: v };
    };
    const text = value === null
        ? 'Off'
        : type === 'probability'
        ? `${Math.round(value * 100)}%`
        : type === 'ratchet'
        ? `×${value}`
        : type === 'microtiming'
        ? `${Math.round(value * 100)}%`
        : String(value);
    const spec = { probability: [.05, 1, .05, .5], ratchet: [2, max, 1, 2], microtiming: [.05, max, .05, .05] }[type] || [min, max, step, min];
    return { label, name: `${FX_NAMES[type]} on row ${String(row).padStart(2, '0')}`, value: text, norm: value === null
        ? 0
        : (value - min) / (max - min), detent: step >= 1 ? .06 : .025,
      step(direction) {
        const key = `typed:${patternId}:${track}:${row}:${type}`, current = burstValue(key, read), s = stepAt(track, row, pat);
        let next;
        if (current === null) {
          if (direction < 0) return;
          if (!(s && s.note >= 0)) { flash(`${FX_NAMES[type]} needs a note on this row`); return; }
          if (!find(s) && s.fx1 && s.fx2) { flash('Both FX columns of this step are in use'); return; }
          next = spec[3];
        } else {
          next = Number((current + direction * spec[2]).toFixed(3));
          next = next < spec[0] - 1e-9 ? null : Math.min(next, spec[1]);
        }
        burstEdit(key, 'step effect', read, write, next);
      } };
  }
  function addStep(track, row, note) {
    const inst = P().tracks[track]?.instrument ?? 0;
    const steps = pattern().steps[track] || [],
      previous = [...steps.slice(0, row)].reverse().find(s => s?.note >= 0) || steps.find(s => s?.note >= 0);
    const padNote = Number.isInteger(P().tracks[track]?.padNote) ? P().tracks[track].padNote : null;
    const value = note ?? M.lastNote.get(track) ?? previous?.note ?? padNote ?? P().instruments[inst]?.rootNote ?? 60;
    S().mutate('add step', p => {
      const pat = p.patterns.find(item => item.id === p.activePattern);
      // Extra note columns of the row (app-note-columns.js) stay.
      if (pat) pat.steps[track][row] = { note: value, instrument: inst, velocity: M.fixedVel ? M.fixedVelValue : 100, gate: 0, ...(pat.steps[track][row]?.columns ? { columns: pat.steps[track][row].columns } : {}) };
    }, { render: false });
    scheduleRefresh();
  }

  /* Home: the tracker across both screens (docs/MK3_DESIGN_STUDY.md §5.1). Display buttons select the column
     above them (Shift, or MUTE held: mute; SOLO held: solo); with SELECT held or latched they show the pattern
     tools instead. Encoders always edit the cursor step: Note, Instrument, Velocity, Gate, FX 1, FX 2,
     Probability, Pattern; on an empty row the fields show their defaults greyed out (Note creates the step). */
  function patternToolButtons() {
    return [button('Copy', () => S().copySelection(), { shift: button('Cut', () => S().copySelection(true)) }),
      button('Paste', () => S().pasteSelection(), { shift: button('Fill ↓', () => S().fillSelection()) }),
      button('Interp', () => perform('interpolate'), { shift: button('Reverse', () => perform('reverseSteps')) }),
      button('Clear', () => S().clearSelection(), { shift: button('Clear FX', () => perform('clearEffects')) }),
      button('Double', () => perform('doublePattern'), { shift: button('Halve', () => perform('halvePattern')) }),
      button('Stretch', () => perform('stretch'), { shift: button('Compress', () => perform('compress')) }),
      button('Note off', () => mk.writeNote(-1), { shift: button('Note cut', () => mk.writeNote(-2)) }),
      button('Step FX', () => mk.setScreen('macro'), { shift: button('FX picker', () => perform('stepEffect')) })];
  }
  function columnButtons(columns) {
    const current = c => c.index === st().track || !!c.members?.includes(st().track);
    const label = c => c.name;
    const style = c => ({ on: current(c), color: c.color, mute: !!c.mute, solo: !!c.solo, select: true });
    const press = c => () => { if (active('mute')) toggleColumn(c, 'mute'); else if (active('solo')) toggleColumn(c, 'solo'); else selectColumn(c); };
    return Array.from({ length: 8 },
        (_, i) => { const c = columns[i]; return c
            ? button(label(c), press(c), { ...style(c), shift: button(label(c), () => toggleColumn(c, 'mute'), style(c)) })
            : null; });
  }
  /** A greyed-out encoder that shows the default a field will get (no blank encoder slots on Home). */
  const ghostKnob = (label, value) => ({ label, name: label, value: String(value), norm: null, dim: true });
  function homeKnobs() {
    const p = P(), pat = pattern(), s = stepAt(), track = st().track;
    const k = stepKnobs(), inst = p.tracks[track]?.instrument ?? 0;
    if (!k[1]) k[1] = ghostKnob('Inst', `${String(inst).padStart(2, '0')} ${p.instruments[inst]?.name || ''}`.slice(0, 14));
    if (!k[2]) k[2] = ghostKnob('Velocity', M.fixedVel ? M.fixedVelValue : 100);
    if (!k[3]) k[3] = ghostKnob('Gate', '— none');
    // FX 1 / FX 2: the value of an effect on the step, or its type while the column is empty (choosing one adds
    // it). On an empty row they are greyed defaults like the other fields (FX-only rows: MACRO).
    const fx = n => {
      if (!s) return ghostKnob(`FX${n}`, 'None');
      const [type, value] = fxKnobs(n);
      if (s['fx' + n] && value) return value;
      return { ...type, label: `FX${n}` };
    };
    return [...k, fx(1), fx(2), s && s.note >= 0 ? typedFxKnob('probability', 'Prob') : ghostKnob('Prob', 'Off'),
      actionKnob('Pattern', `${String(patternIndex() + 1).padStart(2, '0')} ${pat.name}`, direction => mk.selectPattern(patternIndex() + direction),
          { detent: .07, options: p.patterns.map((item, i) => `${String(i + 1).padStart(2, '0')} ${item.name}`), index: patternIndex() })];
  }

  Object.assign(mk, {
    pageIndex, LIST_ROWS, LIST_MARGIN, listWindow, listView, listSteps, listMain, listData, cursorMain, stepKnobs,
    fxKnobs, typedFxKnob, addStep, patternToolButtons, columnButtons, homeKnobs
  });
})();
