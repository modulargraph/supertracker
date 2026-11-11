/* Pattern editing tools: mute/solo, interpolate (ramp), humanize, reverse, nudge, stretch,
   double/halve, quick step effects and a searchable "Automate parameter…" picker.
   Every tool is one undo step. Without a selection, tools work on the cursor track's whole pattern. */
(function () {
  'use strict';
  const S = () => window.ST, state = () => S().state;
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const { clone: copy } = window.STUtil;
  const noteEffects = new Set(['probability', 'ratchet', 'reverse', 'offset', 'glide', 'microtiming', 'arpeggio', 'slice']);
  const hasNote = step => !!step && Number.isFinite(step.note) && step.note >= 0;
  const hits = cell => cell ? [cell] : [];
  // Every note of a step: column 0 and its extra note columns (app-note-columns.js).
  const noteHits = step => window.STApp?.playedCells ? window.STApp.playedCells(step).map(item => item.cell) : hits(step).filter(hasNote);
  const firstNote = cell => noteHits(cell)[0];

  function range() {
    const bounds = S().selectedBounds();
    if (bounds) return { ...bounds, label: 'selection' };
    const s = state();
    return { r0: 0, r1: S().pattern().length - 1, t0: s.track, t1: s.track, label: (s.project.tracks[s.track]?.name || 'track')
        + ' (whole pattern)' };
  }
  function forTracks(bounds, fn) { const pattern = S().pattern(); for (let t = bounds.t0; t <= bounds.t1; t++) fn(pattern.steps[t], t, pattern); }
  function done(message) { S().toast(message); }

  // ---- Mixer state from the tracker -------------------------------------------------
  function toggleTrack(property, index = state().track) {
    const track = state().project.tracks[index]; if (!track) return;
    const next = !track[property];
    S().mutate(`${next ? '' : 'un'}${property} track`, p => { p.tracks[index][property] = next; });
    done(`${track.name}: ${property === 'mute' ? (next ? 'muted' : 'unmuted') : (next ? 'soloed' : 'solo off')}`);
  }
  function clearMuteSolo() {
    if (!state().project.tracks.some(t => t.mute || t.solo)) return done('No tracks are muted or soloed.');
    S().mutate('clear mute and solo', p => p.tracks.forEach(t => {
      t.mute = false;
      t.solo = false;
    }));
    done('All tracks unmuted, solo cleared');
  }

  // ---- Interpolate --------------------------------------------------------------------
  // Velocity columns ramp note velocities between the first and last note. FX columns ramp
  // the value of the first effect towards the last matching effect, filling the rows between.
  function interpolate() {
    const bounds = range(), column = state().column, project = state().project;
    if (bounds.r1 <= bounds.r0) return done('Select at least two rows to interpolate.');
    let changed = 0;
    const plan = [];
    forTracks(bounds, (steps, t) => {
      if (column >= 3) {
        const key = 'fx' + (column - 2), list = [];
        for (let r = bounds.r0; r <= bounds.r1; r++) if (steps[r]?.[key]) list.push({ row: r, effect: steps[r][key] });
        if (list.length < 2) return;
        const first = list[0], last = [...list].reverse().find(item => sameTarget(item.effect, first.effect));
        if (!last || last.row === first.row) return;
        plan.push({ t, key, from: first.row, to: last.row, a: first.effect, b: last.effect });
      } else {
        const rows = [];
        for (let r = bounds.r0; r <= bounds.r1; r++) if (firstNote(steps[r])) rows.push(r);
        if (rows.length < 3) return;
        plan.push({
          t,
          key: 'velocity',
          rows,
          a: firstNote(steps[rows[0]]).velocity ?? 100,
          b: firstNote(steps[rows[rows.length - 1]]).velocity ?? 100
        });
      }
    });
    if (!plan.length) return done(column >= 3
        ? 'Put the same effect at the start and end of the range, then interpolate.'
        : 'Interpolate needs at least three notes in the range.');
    S().mutate('interpolate', () => {
      const pattern = S().pattern();
      for (const item of plan) {
        const steps = pattern.steps[item.t];
        if (item.key === 'velocity') {
          const start = item.rows[0], span = item.rows[item.rows.length - 1] - start;
          for (const r of item.rows) for (const hit of noteHits(steps[r])) {
            hit.velocity = clamp(Math.round(item.a + (item.b - item.a) * (r - start) / span), 1, 127);
            changed++;
          }
          continue;
        }
        const limits = item.a.type === 'plugin' ? [0, 1, .001] : S().FX[item.a.type] || [0, 1, .01];
        const span = item.to - item.from;
        for (let r = item.from; r <= item.to; r++) {
          let targets = hits(steps[r]);
          if (noteEffects.has(item.a.type)) targets=targets.filter(hasNote);
          const raw = item.a.value + (item.b.value - item.a.value) * (r - item.from) / span;
          const value = clamp(Math.round(raw / limits[2]) * limits[2], limits[0], limits[1]);
          if (!targets.length && !noteEffects.has(item.a.type)) {
            const target = { note: -3, instrument: project.tracks[item.t]?.instrument ?? 0, velocity: 100, gate: 0 };
            if (steps[r]) continue;
            steps[r] = target;
            targets = [target];
          }
          for (const target of targets) { target[item.key]={...copy(item.a),value:Number(value.toFixed(4))};changed++; }
        }
      }
    });
    done(`Interpolated ${changed} step${changed === 1 ? '' : 's'} · ${column >= 3 ? 'FX ' + (column - 2) : 'velocity'}`);
  }
  function sameTarget(a, b) {
    return !!a && !!b && a.type === b.type && (a.type !== 'plugin' || a.instanceID === b.instanceID && a.address === b.address);
  }

  // ---- Velocity ------------------------------------------------------------------------
  function velocities(label, fn) {
    const bounds = range(); let count = 0;
    forTracks(bounds, steps => { for (let r = bounds.r0; r <= bounds.r1; r++) count += noteHits(steps[r]).length; });
    if (!count) return done('No notes in the ' + bounds.label + '.');
    S().mutate(label, () => forTracks(bounds, steps => {
      for (let r = bounds.r0; r <= bounds.r1; r++) for (const hit of noteHits(steps[r])) hit.velocity = clamp(Math.round(fn(hit.velocity ?? 100)),
          1, 127);
    }));
    done(`${label[0].toUpperCase() + label.slice(1)} · ${count} note${count === 1 ? '' : 's'} in ${bounds.label}`);
  }
  const humanize = (amount = 12) => velocities('humanize velocity', v => v + (Math.random() * 2 - 1) * amount);
  const randomizeVelocity = () => velocities('randomize velocity', () => 40 + Math.random() * 87);
  const scaleVelocity = factor => velocities(factor > 1 ? 'louder velocity' : 'softer velocity', v => v * factor);

  // ---- Time operations -------------------------------------------------------------------
  function rows(label, transform) {
    const bounds = range();
    // Nothing to move in an empty range: say so instead of adding an Undo step that changes nothing.
    let filled = false;
    forTracks(bounds, steps => { for (let r = bounds.r0; r <= bounds.r1 && !filled; r++) filled = !!steps?.[r]; });
    if (!filled) return done('No steps in the ' + bounds.label + '.');
    S().mutate(label, () => forTracks(bounds, steps => {
      const slice = steps.slice(bounds.r0, bounds.r1 + 1), result = transform(slice);
      for (let i = 0; i < slice.length; i++) steps[bounds.r0 + i] = result[i] ?? null;
    }));
    done(`${label[0].toUpperCase() + label.slice(1)} · ${bounds.label}`);
  }
  const reverse = () => rows('reverse steps', slice => slice.slice().reverse());
  function nudge(delta) {
    rows(delta < 0 ? 'nudge steps up' : 'nudge steps down',
        slice => slice.map((_, i) => slice[((i - delta) % slice.length + slice.length) % slice.length]));
  }
  function stretch(factor) {
    rows(factor > 1 ? 'stretch steps' : 'compress steps', slice => {
      const result = Array(slice.length).fill(null);
      slice.forEach((step, i) => {
        if (!step) return;
        const target = Math.floor(i * factor);
        if (target >= result.length || result[target]) return; // keep the earlier step on collision
        result[target] = copy(step);
        if (result[target].gate > 0) result[target].gate = clamp(Number((result[target].gate * factor).toFixed(3)), .01, 256);
      });
      return result;
    });
  }
  function resizePattern(factor) {
    const pattern = S().pattern(), length = pattern.length, next = factor > 1 ? length * 2 : Math.floor(length / 2);
    if (next > 256) return done('Patterns can be up to 256 steps.');
    if (next < 1) return done('The pattern is already one step long.');
    S().mutate(factor > 1 ? 'double pattern' : 'halve pattern', () => {
      const target = S().pattern();
      target.steps = target.steps.map(steps => factor > 1
          ? [...steps.slice(0, length), ...steps.slice(0, length).map(step => step ? copy(step) : null)]
          : steps.slice(0, next));
      for (const lane of target.automation) {
        lane.points = factor > 1 ? [...lane.points, ...lane.points.map(point => ({
          ...point,
          row: point.row + length
        }))] : lane.points.filter(point => point.row < next);
      }
      target.length = next;
      const s = state(); s.row = Math.min(s.row, next - 1); s.selection = null;
    });
    done(factor > 1 ? `Pattern doubled to ${next} steps (content repeated)` : `Pattern halved to ${next} steps`);
  }

  // ---- Quick step effects ---------------------------------------------------------------
  const quickEffects = [
    ['Probability 75%', 'probability', .75], ['Probability 50%', 'probability', .5], ['Probability 25%', 'probability', .25],
    ['Ratchet ×2', 'ratchet', 2], ['Ratchet ×3', 'ratchet', 3], ['Ratchet ×4', 'ratchet', 4],
    ['Microtiming late 25%', 'microtiming', .25], ['Microtiming late 50%', 'microtiming', .5],
    ['Reverse playback', 'reverse', 1], ['Sample offset 25%', 'offset', .25], ['Sample offset 50%', 'offset', .5],
    ['Glide from −12', 'glide', -12], ['Arpeggio +3 +7 (minor)', 'arpeggio', 0x37], ['Arpeggio +4 +7 (major)', 'arpeggio', 0x47]
  ];
  function applyEffect(type, value) {
    const bounds = range(); let applied = 0, skipped = 0;
    const plan = [];
    forTracks(bounds, (steps, t) => { for (let r = bounds.r0; r <= bounds.r1; r++) {
      const step = steps[r];
      if (!noteHits(step).length) continue;
      const slot = ['fx1', 'fx2'].find(key => step[key]?.type === type) || ['fx1', 'fx2'].find(key => !step[key]);
      if (slot) plan.push([t, r, slot]); else skipped++;
    } });
    if (!plan.length) return done(skipped
        ? 'Both FX columns are in use on these notes.'
        : 'Step effects like this need notes in the ' + bounds.label + '.');
    S().mutate('set step effect', () => {
      const pattern = S().pattern();
      for (const [t, r, slot] of plan) {
        pattern.steps[t][r][slot] = { type, value };
        applied++;
      }
    });
    done(`${type} ${value} on ${applied} note${applied === 1 ? '' : 's'}${skipped ? ` · ${skipped} skipped (FX columns full)` : ''}`);
  }
  function clearEffects() {
    const bounds = range(); let count = 0;
    forTracks(bounds, steps => { for (let r = bounds.r0; r <= bounds.r1; r++) count += hits(steps[r]).filter(hit => hit.fx1 || hit.fx2).length; });
    if (!count) return done('No step effects in the ' + bounds.label + '.');
    S().mutate('clear step effects', () => forTracks(bounds, steps => {
      for (let r = bounds.r0; r <= bounds.r1; r++) if (steps[r]) {
        delete steps[r].fx1;
        delete steps[r].fx2;
        if (steps[r].note === -3) steps[r] = window.STApp?.dropEmptyStep ? window.STApp.dropEmptyStep(steps[r]) : null;
      }
    }));
    done(`Cleared effects on ${count} step${count === 1 ? '' : 's'}`);
  }

  // ---- Automate parameter picker ------------------------------------------------------
  function automatePicker(index = state().track) {
    const app=S(),project=state().project,track=project.tracks[index];
    if (!track || !window.STStudio?.automationChoices) return;
    const owner = index, ownerName = track.name;
    app.openModal('Automate parameter', `<div class="automate-picker"><div class="automate-picker-head">Automate on <strong>${app.escapeHTML(ownerName)}</strong> · ${app.escapeHTML(app.pattern().name)}</div>`
        + `<input id="automate-search" class="palette-input" placeholder="Search parameters…" autocomplete="off" role="combobox"`
        + ` aria-expanded="true" aria-controls="automate-list" aria-label="Search parameters to automate"><div id="automate-list"`
        + ` class="palette-list" role="listbox" aria-label="Parameters"></div><div class="automate-picker-foot muted">↑ ↓ choose · Enter automate `
        + `· Already automated parameters are listed first</div></div>`, '', {
      kind: 'automatePicker',
      className: 'palette',
      noHeader: true
    });
    const search = document.getElementById('automate-search'); if (!search) return;
    let index_ = 0, matches = [];
    const paint = () => {
      const list = document.getElementById('automate-list'); if (!list) return;
      const words = search.value.toLowerCase().split(/\s+/).filter(Boolean);
      matches = window.STStudio.automationChoices(owner).filter(item => item.available
          && words.every(word => `${item.name} ${item.group} ${item.search}`.toLowerCase().includes(word)));
      index_ = clamp(index_, 0, Math.max(0, matches.length - 1));
      const automated = matches.filter(item => item.automated).length;
      list.innerHTML = matches.length ? matches.map((item, i) => `${i === 0 && automated ? `<div class="palette-caption">Already automated · ${automated}</div>` : ''}${i === automated && i < matches.length
          ? '<div class="palette-caption">Available parameters</div>'
          : ''}<button `
          + `id="automate-option-${i}" class="palette-item ${i === index_ ? 'active' : ''}" data-automate-option="${i}" role="option"`
          + ` tabindex="-1" aria-selected="${i === index_}"><span>${item.automated ? '∿ ' : ''}${app.escapeHTML(item.name)}<small class="muted"> `
          + `· ${app.escapeHTML(item.group)}</small></span>${item.automated ? `<kbd>${item.points} pt</kbd>` : ''}</button>`).join('') : '<div class="empty" role="status">No matching parameter</div>';
      const active = document.getElementById('automate-option-' + index_);
      if (active) { search.setAttribute('aria-activedescendant', active.id); active.scrollIntoView({ block: 'nearest' }); }
      list.querySelectorAll('[data-automate-option]').forEach(button => {
        button.onmousedown = e => e.preventDefault();
        button.onclick = () => choose(matches[Number(button.dataset.automateOption)]);
      });
    };
    const choose = item => {
      if (!item) return;
      if (state().project !== project
          || project.tracks[index] !== track) {
        app.closeModal();
        return done('The automation destination changed. Open the picker again.');
      }
      app.closeModal();
      state().track = index;
      app.automate(owner, item.target, `${ownerName} · ${item.name}`, item.value);
    };
    search.oninput = () => { index_ = 0; paint(); };
    search.onkeydown = event => {
      if (event.key === 'ArrowDown'
          || event.key === 'ArrowUp') { event.preventDefault(); index_ = clamp(index_ + (event.key === 'ArrowDown' ? 1 : -1),
          0, Math.max(0, matches.length - 1)); paint(); }
      else if (event.key === 'Enter') { event.preventDefault(); choose(matches[index_]); }
    };
    paint();
    // Plug-in parameters are fetched lazily; repaint when they arrive.
    Promise.resolve(window.STStudio.loadAutomationParameters?.(owner)).then(() => {
      if (state().modal === 'automatePicker') paint();
    }).catch(() => {});
  }

  // ---- Keyboard -------------------------------------------------------------------------
  // Option + letter uses the physical key code because macOS Option changes e.key.
  const letter = (event, l) => event.code === 'Key' + l.toUpperCase() || event.key.toLowerCase() === l;
  function key(event) {
    const s = state(), app = S();
    if (event.defaultPrevented || event.isComposing || s.modal || event.ctrlKey) return false;
    const option = event.altKey && !event.metaKey, plain = !event.altKey && !event.metaKey;
    const run = fn => { event.preventDefault(); if (!event.repeat) fn(); return true; };
    // Performance settings work in every view (keyboard piano is global).
    if (option) {
      if (letter(event, 'c')) return run(() => app.cyclePerformance('chord', event.shiftKey ? -1 : 1));
      if (letter(event, 'k')) return run(() => app.cyclePerformance('scale', event.shiftKey ? -1 : 1));
      if (letter(event, 't')) return run(() => app.cyclePerformance('root', event.shiftKey ? -1 : 1));
      if (letter(event, 'l') && event.shiftKey) return run(() => app.perform('loopSelection'));
      if (letter(event, 'l')) return run(() => app.cyclePerformance('scaleMode'));
      if (letter(event, 'v')) return run(() => app.cyclePerformance('velocity', event.shiftKey ? -1 : 1));
      if (letter(event, 'n')) return run(() => event.shiftKey ? window.STNoteRepeat?.cycleMode(1) : window.STNoteRepeat?.toggle());
      if (event.code === 'Minus' || event.code === 'Equal') return run(() => window.STNoteRepeat?.cycleRate(event.code === 'Equal' ? 1 : -1));
    }
    if (s.view !== 'pattern') return false;
    const target = event.target, grid = target === document.body || target?.id === 'tracker' || target?.closest?.('.tracker');
    if (!grid) return false;
    if (plain && (event.key === '{' || event.key === '}')) return run(() => app.cyclePerformance('editStep', event.key === '}' ? 1 : -1));
    if (event.metaKey
        && !event.altKey
        && !event.shiftKey
        && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) return run(() => nudge(event.key === 'ArrowUp' ? -1 : 1));
    if (!option) return false;
    if (letter(event, 'm')) return run(() => event.shiftKey ? clearMuteSolo() : toggleTrack('mute'));
    if (letter(event, 's')) return run(() => toggleTrack('solo'));
    if (letter(event, 'i')) return run(interpolate);
    if (letter(event, 'h')) return run(() => event.shiftKey ? randomizeVelocity() : humanize());
    if (letter(event, 'r')) return run(reverse);
    if (letter(event, 'e')) return run(() => stretch(event.shiftKey ? .5 : 2));
    if (letter(event, 'd')) return run(() => resizePattern(event.shiftKey ? .5 : 2));
    if (letter(event, 'f') && event.shiftKey) return run(() => app.perform('songScroll'));
    if (letter(event, 'a') && event.shiftKey) return run(() => app.perform('automationPanel'));
    if (letter(event, 'f')) return run(() => app.perform('follow'));
    if (letter(event, 'p')) return run(() => automatePicker());
    if (letter(event, 'x')) return run(clearEffects);
    if (event.code === 'Period' || event.code === 'Comma') return run(() => scaleVelocity(event.code === 'Period' ? 1.1 : 1 / 1.1));
    return false;
  }

  window.STPatternTools = {
    toggleTrack,
    clearMuteSolo,
    interpolate,
    humanize,
    randomizeVelocity,
    scaleVelocity,
    reverse,
    nudge,
    stretch,
    resizePattern,
    applyEffect,
    clearEffects,
    quickEffects,
    automatePicker,
    key
  };
})();
