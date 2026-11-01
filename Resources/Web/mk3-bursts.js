/* MK3: burst edits (preview live, commit one Undo step per gesture), knob factories and formatting. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, P, clamp, clamp01, copy, noteName, BURST_MS, flash, scheduleRefresh, AR, arRecording, channelOf
  } = mk;

  // ---- Burst edits: preview live, commit one Undo step per gesture ------------------------------
  const bursts = new Map();
  mk.previewTimer = null;
  mk.currentEncoder = null;
  mk.editGeneration = 0;
  function burstScope(key, project) {
    const parts = key.split(':'), kind = parts[0], scope = [];
    const at = (collection, index) => scope.push({ collection, index, owner: project[collection][index] });
    if (kind === 'ownerMix') { const owner = decodeURIComponent(parts[1]), resolve = p => channelOf(owner, p); scope.push({ owner: resolve(project), resolve }); }
    else if (kind === 'inst') at('instruments', Number(parts[1]));
    else if (kind === 'mix') at('tracks', Number(parts[1]));
    else if (['step', 'lock', 'typed', 'fxtype', 'fxval', 'length', 'events'].includes(kind)) {
      const offset = kind === 'events' ? 2 : 1;
      at('patterns', project.patterns.findIndex(item => String(item.id) === parts[offset]));
      if (kind !== 'length') at('tracks', Number(parts[offset + 1]));
    }
    return scope;
  }
  const ownsBurst = burst => burst.project === P()
      && burst.scope.every(item => item.owner && (item.resolve ? item.resolve(P()) : P()[item.collection][item.index]) === item.owner);
  function burstValue(key, read) { const burst = bursts.get(key); return burst && ownsBurst(burst) ? burst.value : read(P()); }
  function burstEdit(key, label, read, write, value, options = {}) {
    let burst = bursts.get(key);
    if (burst && !ownsBurst(burst)) { clearTimeout(burst.timer); bursts.delete(key); burst = null; }
    if (!burst) {
      burst = {
        key,
        label,
        read,
        write,
        project: P(),
        scope: burstScope(key, P()),
        ref: options.ref || null,
        commit: options.commit || null,
        encoder: mk.currentEncoder,
        recorded: false
      };
      bursts.set(key, burst);
    }
    burst.value = value; mk.editGeneration++;
    if (arRecording(burst.ref)) { AR().value(burst.ref, value, 'mk3'); burst.recorded = true; }
    else schedulePreview();
    clearTimeout(burst.timer); burst.timer = setTimeout(() => commitBurst(key), BURST_MS);
    mk.schedule(); return value;
  }
  function commitBurst(key, options = {}) {
    const burst = bursts.get(key); if (!burst) return true;
    clearTimeout(burst.timer);
    if (burst.pending) return false;
    if (burst.ref && burst.recorded) { AR()?.end?.(burst.ref); bursts.delete(key); mk.schedule(); return true; }
    if (!ownsBurst(burst)) { bursts.delete(key); flash('Controller target changed · repeat the edit'); return true; }
    const value = burst.value;
    if (JSON.stringify(burst.read(P())) === JSON.stringify(value)) { bursts.delete(key); return true; }
    const before = JSON.stringify(P());
    const finish = accepted => {
      if (bursts.get(key) !== burst) return accepted !== false;
      burst.pending = null;
      if (accepted === false) {
        // Freeze validation rolls back by replacing the project with an unchanged clone.
        // Retain the gesture on that clone, but never attach it to a different session.
        if (P() !== burst.project && JSON.stringify(P()) === before) {
          const replaced = burst.project;
          for (const item of bursts.values()) if (item.project === replaced) { item.project = P(); item.scope = burstScope(item.key, P()); }
        }
        mk.schedule(); return false;
      }
      if (burst.value === value) bursts.delete(key);
      scheduleRefresh(); mk.schedule(); return true;
    };
    try {
      const result = burst.commit
          ? burst.commit(value, { ...options, mk3Edit: burst })
          : S().mutate(burst.label, p => burst.write(p, value), { ...options, render: false });
      if (result && typeof result.then === 'function') {
        burst.pending = Promise.resolve(result).then(finish, error => { S().toast(error.message, 'error'); return finish(false); });
        return false;
      }
      return finish(result);
    } catch (error) { S().toast(error.message, 'error'); return finish(false); }
  }
  function commitBursts(encoder, options = {}) {
    let accepted = true;
    for (const [key, burst] of [...bursts]) if ((encoder === undefined || burst.encoder === encoder) && !commitBurst(key, options)) accepted = false;
    return accepted;
  }
  async function flushEdits(options = {}) {
    const owner = P();
    for (const [key, burst] of [...bursts]) {
      if (burst === options.mk3Edit && burst.pending) continue;
      if (burst.pending) { if (await burst.pending === false) return false; }
      if (!commitBurst(key, options)) {
        const pending = bursts.get(key)?.pending;
        if (!pending || await pending === false) return false;
      }
      if (P() !== owner && bursts.size) return false;
    }
    return ![...bursts.values()].some(burst => burst.project === P() && burst !== options.mk3Edit);
  }
  function liveBurst(key, burst, owner) {
    const match = /^inst:(\d+):(\w+)$/.exec(key);
    if (!match) return false;
    const index = Number(match[1]);
    return !!S().liveInstParam?.(index, owner.instruments[index], match[2], burst.value);
  }
  function schedulePreview() {
    if (mk.previewTimer) return;
    mk.previewTimer = setTimeout(() => {
      mk.previewTimer = null;
      const owner = P(), pending = [...bursts.entries()].filter(([, burst]) => burst.project === owner && ownsBurst(burst) && !burst.recorded)
        // Sampler parameters are heard live on sounding voices (engine.instrumentParameter); the rest republish a draft.
        .filter(([key, burst]) => !liveBurst(key, burst, owner)).map(([, burst]) => burst);
      if (!pending.length || !S().syncEngineProject) return;
      Promise.resolve(S().syncEngineProject(() => {
        if (P() !== owner) return null;
        const draft = copy(owner);
        for (const burst of bursts.values()) if (burst.project === owner && ownsBurst(burst) && !burst.recorded) burst.write(draft, burst.value);
        mk.applyStripPreview(draft);
        return draft;
      })).catch(() => {});
    }, 60);
  }

  // ---- Knob factories ------------------------------------------------------------------------------
  // A small offset gives zero-time envelopes a finite instant endpoint and useful fine control.
  const logOffset = o => o.min === 0 ? .001 : 0;
  // Knobs that change an instrument, channel or project value (not the cursor step): LOCK must not edit these.
  const isGlobalKey = key => /^(inst|mix|ownerMix|length):/.test(key) || ['master', 'swing', 'tempo', 'lpb', 'loopSong'].includes(key);
  const toNorm = (v, o) => o.curve === 'log'
      ? Math.log((Math.max(o.min, v) + logOffset(o)) / (o.min + logOffset(o))) / Math.log((o.max + logOffset(o)) / (o.min + logOffset(o)))
      : (v - o.min) / (o.max - o.min);
  const fromNorm = (n, o) => o.curve === 'log'
      ? (o.min + logOffset(o)) * Math.pow((o.max + logOffset(o)) / (o.min + logOffset(o)), n) - logOffset(o)
      : o.min + n * (o.max - o.min);
  function numKnob(o) {
    const value = burstValue(o.key, o.read);
    return { label: o.label, name: o.name || o.label, value: o.fmt(value), norm: clamp01(toNorm(value, o)), base: value, lock: o.lock || null, bipolar: o.min < 0 && o.max > 0, global: isGlobalKey(o.key),
      turn(delta) {
        const current = burstValue(o.key, o.read);
        let next = fromNorm(clamp01(toNorm(current, o) + delta * (o.speed || 1)), o);
        if (o.step) next = Math.round(next / o.step) * o.step;
        next = Number(clamp(next, o.min, o.max).toFixed(6));
        burstEdit(o.key, o.label, o.read, o.write, next, { ref: o.ref, commit: o.commit });
      } };
  }
  function intKnob(o) {
    const value = burstValue(o.key, o.read);
    return { label: o.label, name: o.name || o.label, value: o.fmt ? o.fmt(value) : String(value), norm: clamp01((value - o.min) / Math.max(1, o.max - o.min)), detent: o.detent || .03, lock: o.lock || null, global: isGlobalKey(o.key),
      step(direction) { const next = clamp(Math.round(burstValue(o.key, o.read)) + direction * (o.stride || 1), o.min,
          o.max); burstEdit(o.key, o.label, o.read, o.write, next, { ref: o.ref, commit: o.commit }); } };
  }
  function enumKnob(o) {
    const value = burstValue(o.key, o.read), index = Math.max(0, o.options.findIndex(([key]) => key === value));
    return { label: o.label, name: o.name || o.label, value: (o.options[index] || ['', '–'])[1], norm: o.options.length > 1
        ? index / (o.options.length - 1)
        : 0, detent: o.detent || .07, options: o.options.map(option => option[1]), index, global: isGlobalKey(o.key),
      step(direction) { const current = Math.max(0, o.options.findIndex(([key]) => key === burstValue(o.key, o.read))), next = o.wrap
          ? (current + direction + o.options.length) % o.options.length
          : clamp(current + direction, 0, o.options.length - 1); burstEdit(o.key, o.label, o.read, o.write, o.options[next][0],
              { commit: o.commit }); } };
  }
  function actionKnob(label, value, step, extra = {}) {
    return {
      label,
      name: extra.name || label,
      value: String(value ?? ''),
      norm: extra.norm ?? null,
      detent: extra.detent || .06,
      step,
      refreshAfterStep: true,
      ...extra
    };
  }
  function button(label, action, extra = {}) { return { label, action, ...extra }; }

  // ---- Formatting ------------------------------------------------------------------------------------
  const sign = x => x > 0 ? '+' : x < 0 ? '−' : '';
  const fmt = {
    time: v => v < .01 ? `${Number((v * 1000).toFixed(1))} ms` : v < 1 ? `${Math.round(v * 1000)} ms` : `${v.toFixed(v < 10 ? 2 : 1)} s`,
    hz: v => v < 1000 ? `${Math.round(v)} Hz` : `${(v / 1000).toFixed(v < 10000 ? 2 : 1)} kHz`,
    rate: v => v <= 0 ? 'Off' : `${v.toFixed(v < 10 ? 2 : 1)} Hz`,
    db: v => { if (v <= .0001) return '−∞ dB'; const r = Math.round(20 * Math.log10(v) * 10) / 10; return `${sign(r)}${Math.abs(r).toFixed(1)} dB`; },
    pan: v => Math.abs(v) < .005 ? 'Center' : `${Math.round(Math.abs(v) * 100)}% ${v < 0 ? 'L' : 'R'}`,
    pct: v => `${Math.round(v * 100)}%`,
    semi: v => { const r = Math.round(v * 10) / 10; return `${sign(r)}${Math.abs(r)} st`; },
    cents: v => { const r = Math.round(v); return `${sign(r)}${Math.abs(r)} ct`; },
    note: v => `${noteName(Math.round(v))}`,
    bits: v => v >= 16 ? '16-bit' : `${Math.round(v)}-bit`,
    beats: v => `${Number(v.toFixed(3))} b`,
    int: v => String(Math.round(v)),
    bpm: v => `${Number(v).toFixed(1)}`
  };

  Object.assign(mk, {
    bursts, ownsBurst, burstValue, burstEdit, commitBurst, commitBursts, flushEdits, toNorm, fromNorm, numKnob,
    intKnob, enumKnob, actionKnob, button, sign, fmt
  });
})();
