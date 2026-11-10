/* MK3: touch strip, encoders and the input entry point. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, clamp01, copy, now, TAP_MS, M, active, pattern, playing, flash, setCursor, AR, arRef,
    arRecording, expression, padPressure, pedal, bursts, ownsBurst, burstEdit, commitBursts,
    fromNorm, sign, instKnob, lockTurn, listSteps, OVERVIEW_MS, overviewVisible, currentPage,
    HANDLERS, padDown, padUp
  } = mk;

  // ---- Touch strip -------------------------------------------------------------------------------------
  function setStripMode(mode, shifted = M.strip.shift, effect = M.perfFx) {
    if (mode === M.strip.mode && shifted === M.strip.shift && effect === M.perfFx) return;
    const touched = M.strip.touched;
    // Finish with the old mode/effect still selected: otherwise release can strand
    // pitch/CC1, a temporary audio preview, or the stutter loop on its old owner.
    stripTouch(false, true);
    M.strip.mode = mode; M.strip.shift = shifted; M.perfFx = effect;
    if (touched) stripTouch(true);
  }
  mk.stripPreview = null;
  function applyStripPreview(draft) {
    if (!mk.stripPreview) return draft;
    const value = mk.stripPreview.value;
    if (mk.stripPreview.kind === 'cutoff') for (const index of mk.stripPreview.instruments) {
      const inst = draft.instruments[index];
      if (inst) inst.cutoff = clamp((inst.cutoff || 18000) * Math.pow(2, (value - 1) * 7), 20, 20000);
    }
    if (mk.stripPreview.kind === 'fade') draft.masterVolume = (draft.masterVolume ?? .7) * value;
    if (mk.stripPreview.kind === 'echo') for (const t of draft.tracks) t.delaySend = Math.max(t.delaySend || 0, value);
    return draft;
  }
  function stripSync() {
    // Live preview through the same engine-project path as the mixer preview.
    clearTimeout(M.stripTimer);
    M.stripTimer = setTimeout(() => {
      const owner = P();
      Promise.resolve(S().syncEngineProject?.(() => {
        if (P() !== owner) return null;
        const draft = copy(owner);
        for (const burst of bursts.values()) if (burst.project === owner && ownsBurst(burst) && !burst.recorded) burst.write(draft, burst.value);
        return applyStripPreview(draft);
      })).catch(() => {});
    }, 40);
  }
  function stripTouch(touched, cancelled = false) {
    M.strip.touched = touched;
    M.strip.scrub = null;
    if (touched) { M.strip.start = M.strip.value; M.strip.owner = st().track; return; }
    const mode = M.strip.mode;
    if (mode === 'pitch' && M.strip.shift) { const amount = Math.round((M.strip.value - .5) * 24); if (amount && !cancelled) S().transpose(amount); M.strip.value = .5; return; }
    if (mode === 'pitch' || (mode === 'mod' && !M.strip.shift) || mode === 'perform') {
      // Spring back.
      M.strip.value = mode === 'perform' && M.perfFx !== 'filter' ? 0 : mode === 'mod' ? 0 : .5;
      if (mk.stripPreview) { mk.stripPreview = null; stripSync(); }
      if (mode === 'perform' && M.perfFx === 'stutter' && M.stutter) {
        M.stutter = null;
        st().patternLoop = M.stutterRestore || null;
        S().request('transport.loop', { loop: S().patternLoopFor?.() || null }).catch(() => {});
      }
      if (M.pitchRef) { AR()?.end?.(M.pitchRef); M.pitchRef = null; }
      if (M.bendTrack != null) { expression('pitchBend', 0, { track: M.bendTrack, flush: true }); M.bendTrack = null; }
      if (M.modTrack != null) { expression('cc', 0, { track: M.modTrack, controller: 1, flush: true }); M.modTrack = null; }
    }
    if (mode === 'mod' && M.strip.shift) commitBursts();
  }
  function stripMove(value) {
    M.strip.value = clamp01(value);
    if (M.autoHold && M.strip.mode) M.autoHold.used = true;
    const mode = M.strip.mode, track = M.strip.touched ? (M.strip.owner ?? st().track) : st().track, p = P();
    if (!mode) {
      const pat = pattern(), row = Math.round(value * (pat.length - 1));
      // The strip reports far more positions than the pattern has rows: act once per row.
      if (M.strip.scrub?.pattern === pat.id && M.strip.scrub.row === row) return;
      M.strip.scrub = { pattern: pat.id, row };
      if (playing() && st().playbackMode === 'pattern') S().request('transport.seek',
          { pattern: pat.id, row, sequence: st().sequence || 0 }).catch(() => {});
      else setCursor({ row });
      return;
    }
    if (mode === 'pitch' && M.strip.shift) { flash(`Transpose ${sign(Math.round((value - .5) * 24))}${Math.abs(Math.round((value - .5) * 24))} st`,
        500); return; }
    if (mode === 'pitch') {
      const bend = clamp((value - .5) * 2, -1, 1); // live pitch bend, ±2 semitones on built-in instruments
      const ref = arRef(track, 'pitch', `${p.tracks[track]?.name} · pitch`);
      // Automation write mode records the track pitch lane (semitones) instead of a live bend.
      if (arRecording(ref)) { AR().value(ref, Math.round(bend * 2 * 10) / 10, 'mk3'); M.pitchRef = ref; return; }
      if (M.bendTrack != null && M.bendTrack !== track) expression('pitchBend', 0, { track: M.bendTrack, flush: true });
      M.bendTrack = track; expression('pitchBend', bend, { track }); return;
    }
    if (mode === 'mod' && M.strip.shift) { const index = st().instrument, knob = instKnob('cutoff'); if (knob) { const o = { min: 20, max: 20000, curve: 'log' }; burstEdit(`inst:${index}:cutoff`,
        'instrument cutoff', q => Number(q.instruments[index]?.cutoff ?? 18000),
        (q, v) => { if (q.instruments[index]) q.instruments[index].cutoff = v; }, Math.round(fromNorm(value, o))); } return; }
    if (mode === 'mod') {
      // CC1 to plug-ins and external MIDI; built-in instruments close their filter (engine mod wheel).
      if (M.modTrack != null && M.modTrack !== track) expression('cc', 0, { track: M.modTrack, controller: 1, flush: true });
      M.modTrack = track; expression('cc', value, { track, controller: 1 }); return;
    }
    if (mode === 'perform') {
      if (M.perfFx === 'filter') { mk.stripPreview = { kind: 'cutoff', value: value, instruments: p.instruments.map((inst, i) => inst.type === 'au' ? -1 : i).filter(i => i >= 0) }; stripSync(); return; }
      if (M.perfFx === 'fade') { mk.stripPreview = { kind: 'fade', value: 1 - value }; stripSync(); return; }
      if (M.perfFx === 'echo') { mk.stripPreview = { kind: 'echo', value }; stripSync(); return; }
      if (M.perfFx === 'stutter') {
        if (!playing() || st().playbackMode !== 'pattern') { flash('Stutter works during Pattern playback'); return; }
        const lengths = [16, 8, 4, 2, 1], length = lengths[clamp(Math.floor(value * lengths.length), 0, lengths.length - 1)], pat = pattern();
        if (M.stutter?.length === length) return;
        if (!M.stutter) M.stutterRestore = st().patternLoop || null;
        const start = Math.floor((Number(st().status?.row) || 0) / length) * length;
        M.stutter = { length }; st().patternLoop = { pattern: pat.id, start, end: Math.min(pat.length - 1, start + length - 1) };
        S().request('transport.loop', { loop: S().patternLoopFor?.() }).catch(() => {});
      }
    }
  }

  // ---- Encoders ------------------------------------------------------------------------------------------
  function encoderTurn(index, delta) {
    const page = currentPage(), knob = page.knobs?.[index];
    if (M.autoHold) M.autoHold.used = true;
    for (const held of M.held.values()) held.used = true;
    for (const pad of M.pads.values()) pad.used = true;
    if (M.variationHeld != null) M.variationUsed = true;
    if (!knob) return;
    mk.currentEncoder = index;
    try {
      // While LOCK is active an encoder writes a step lock; a parameter without one must not silently change
      // the instrument or channel value for every step instead.
      if (active('lock') && !knob.lock && knob.global) { flash(`${knob.label} has no step lock · release LOCK to change it`, 900); return; }
      if (active('lock') && knob.lock && !knob.step) { lockTurn(knob.lock, delta * (M.shift ? .1 : 1), knob.base); return; }
      if (knob.step) {
        const acc = (M.encoderAcc.get(index) || 0) + delta, detent = knob.detent || .05;
        let steps = Math.trunc(acc / detent); M.encoderAcc.set(index, acc - steps * detent);
        steps = clamp(steps, -12, 12);
        // Navigation actions capture a list index; refresh it for each coalesced detent.
        // Parameter closures keep their local live value until the host acknowledges it.
        for (let i = 0; i < Math.abs(steps); i++) {
          const nextPage = i && knob.refreshAfterStep ? currentPage() : page;
          if (nextPage.id !== page.id) break;
          const nextKnob = nextPage.knobs?.[index];
          if (!nextKnob?.step) break;
          nextKnob.step(Math.sign(steps));
        }
        return;
      }
      knob.turn?.(delta * (M.shift ? .1 : 1));
    } finally { mk.currentEncoder = null; }
  }
  function mainTurn(delta) {
    if (M.mainTouch && !overviewVisible()) M.mainTouch.turned = true;
    if (M.autoHold) M.autoHold.used = true;
    for (const held of M.held.values()) held.used = true;
    for (const pad of M.pads.values()) pad.used = true;
    const main = currentPage().main;
    main?.turn?.(main.list ? listSteps(delta, main.list) : delta);
  }
  function touch(target, touched) {
    if (target === 'strip') { stripTouch(touched); return; }
    if (touched) M.touched.add(target); else M.touched.delete(target);
    // A turn always touches the 4-D encoder; only a still touch (or Shift + touch) opens the overview.
    if (target === 'main') {
      M.mainTouch = touched ? { at: now(), turned: false, shift: M.shift } : null;
      if (touched) setTimeout(mk.schedule, OVERVIEW_MS + 10);
    }
    if (!touched && /^encoder[1-8]$/.test(target)) {
      const index = Number(target.slice(7)) - 1;
      M.encoderAcc.delete(index);
      commitBursts(index);
      window.STPluginPages?.controller?.finish?.();
      window.STNKSControls?.controller?.finish?.();
    }
  }

  // ---- Input entry point ---------------------------------------------------------------------------------
  function input(payload = {}) {
    if (!P()) return;
    mk.syncView();
    switch (payload.type) {
      case 'button': if (payload.cancelled) { cancelButton(String(payload.name)); break; } return buttonEvent(String(payload.name),
          !!payload.pressed);
      case 'pad': {
        const pad = Number(payload.pad); if (!(pad >= 1 && pad <= 16)) return;
        if (payload.cancelled) padUp(pad, true);
        else if (payload.phase === 'down') padDown(pad, payload.velocity);
        else if (payload.phase === 'up') padUp(pad);
        else if (payload.phase === 'pressure') { const info = M.pads.get(pad); if (info) { info.pressure = clamp01(payload.pressure); padPressure(info); if (info.key && !M.fixedVel && M.padMode !== 'velocity') window.STNoteRepeat?.setVelocity?.(info.key, Math.max(1, Math.round(info.pressure * 127))); } }
        break;
      }
      case 'encoder': encoderTurn(clamp(Math.trunc(Number(payload.index)), 0, 7), Number(payload.delta) || 0); break;
      case 'main': mainTurn(Math.trunc(Number(payload.delta)) || Math.sign(Number(payload.delta)) || 0); break;
      case 'touch': if (payload.cancelled && payload.target === 'strip') stripTouch(false, true); else touch(String(payload.target),
          !!payload.touched); break;
      case 'strip': stripMove(Number(payload.value)); break;
      case 'knob': if (['master', 'headphone', 'mic'].includes(payload.name)) M.knobs[payload.name] = clamp01(payload.value); break;
      case 'pedal': pedal(!!payload.down); break;
      default: return;
    }
    mk.schedule();
  }
  function cancelButton(name) {
    const handler = HANDLERS[name], held = M.held.get(name); M.held.delete(name);
    if (M.latched === name) M.latched = null;
    if (name === 'shift') { M.shift = false; M.shiftPads = []; }
    if (name === 'auto') M.autoHold = null;
    if (name === 'encoderPush') window.STSampler?.stopAudition?.();
    if (name === 'variation') { M.variationHeld = null; M.variationUsed = false; }
    if (name === 'duplicate') M.dupSource = null;
    if (name === 'events') M.events.clear();
    if (handler?.hold && held && !held.shifted) handler.onRelease?.(false);
    if (['lock', 'volume', 'swing', 'tempo'].includes(name)) commitBursts();
  }
  function buttonEvent(name, pressed) {
    const handler = HANDLERS[name]; if (!handler) return;
    if (pressed) {
      if (name !== 'shift') for (const [other, held] of M.held) if (other !== name) held.used = true;
      if (M.variationHeld != null && name !== 'variation' && name !== 'shift') M.variationUsed = true;
      if (handler.hold) {
        if (M.shift) { handler.shiftAction?.(); M.held.set(name, { at: now(), used: true, shifted: true }); }
        else { M.held.set(name, { at: now(), used: false, unlatch: M.latched === name }); handler.onPress?.(); }
      } else { M.held.set(name, { at: now(), used: true }); handler.press?.(); }
    } else {
      const held = M.held.get(name); M.held.delete(name);
      if (handler.hold && held && !held.shifted) {
        const tap = !held.used && now() - held.at < TAP_MS;
        if (held.unlatch) M.latched = null;
        else if (tap && name === 'erase') { S().clearSelection(); flash('Step cleared', 600); }
        else if (tap) M.latched = name;
        handler.onRelease?.(tap && !held.unlatch);
        if (name === 'events' && !active('events')) M.events.clear();
        if (name === 'duplicate') M.dupSource = null;
        if (name === 'lock' || name === 'volume' || name === 'swing' || name === 'tempo') commitBursts();
      } else handler.release?.();
      if (name === 'shift') M.shiftPads = [];
    }
    mk.schedule();
  }

  Object.assign(mk, { setStripMode, applyStripPreview, stripSync, input });
})();
