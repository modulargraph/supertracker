/* MK3: pads (play, step, scene, pattern, mute/solo, erase, duplicate, select, Shift pads). */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, copy, now, NOTE_NAMES, HOLD, prefs, M, active, pattern, playing, flash, scheduleRefresh,
    withState, setCursor, selectTrack, perform, stepAt, app, groupOf, membersOf, entries, toggleEntry, rememberMember,
    padPressure, queueJump, clearQueue, honourQueue, commitBursts, addStep, selectPattern, quantizeRows
  } = mk;

  // ---- Pads ----------------------------------------------------------------------------------------------
  function velocityOf(raw) {
    if (M.fixedVel) return M.fixedVelValue;
    let v = clamp((Number(raw) || 100) / 127, 0, 1);
    v = clamp(v * (.6 + prefs.sensitivity * .8), 0, 1);
    if (prefs.velocityCurve === 'soft') v = Math.sqrt(v); else if (prefs.velocityCurve === 'hard') v = v * v;
    return clamp(Math.round(v * 127), 1, 127);
  }
  function keyboardPitch(i, scale = st().scale) {
    const state = st(), base = (state.octave + 1) * 12 + state.root, intervals = S().scales[scale] || S().scales.chromatic;
    if (scale === 'chromatic') return clamp(base + i, 0, 127);
    return clamp(base + intervals[i % intervals.length] + 12 * Math.floor(i / intervals.length), 0, 127);
  }
  // 16 Velocities (Shift + FIXED VEL): pad n plays the cursor track's note at velocity n × 127 / 16.
  const sixteenVelocity = i => clamp(Math.round((i + 1) * 127 / 16), 1, 127);
  function velocityTarget() {
    const p = P(), track = st().track, t = p.tracks[track], instrument = groupOf() ? t?.instrument ?? 0 : st().instrument;
    const note = M.lastNote.get(track) ?? (Number.isInteger(t?.padNote) ? t.padNote : p.instruments[instrument]?.rootNote ?? 60);
    return { track, instrument, note };
  }
  function play(pad, note, velocity, { instrument = st().instrument, track = st().track, voicing = {} } = {}) {
    const key = 'mk3-pad-' + pad, held = M.pads.get(pad);
    if (held) { held.note = note; held.track = track; }
    withState({ keyVelocity: velocity, ...voicing }, () => S().noteOn(note, key, true, instrument, track, true));
    if (st().recording && !playing()) scheduleRefresh();
    M.lastNote.set(track, note);
    return key;
  }
  function padDown(pad, rawVelocity) {
    const i = pad - 1, velocity = velocityOf(rawVelocity), info = { at: now(), velocity, pressure: 0, used: false };
    if (M.pads.has(pad)) return;
    M.pads.set(pad, info);
    for (const [name, held] of M.held) if (HOLD.has(name)) held.used = true;
    if (M.variationHeld != null) M.variationUsed = true;
    if (M.shift
        && !active('events')
        && !active('mute')
        && !active('solo')
        && !active('select')
        && !active('duplicate')
        && !active('erase')
        && M.padMode !== 'scene') { info.shiftAction = true; shiftPad(pad); return; }
    if (active('mute') || active('solo')) { padMuteSolo(i, active('mute') ? 'mute' : 'solo'); return; }
    if (active('events')) { const row = M.stepPage * 16 + i; if (row < pattern().length) { if (M.events.has(row)) M.events.delete(row); else M.events.add(row); } return; }
    if (active('erase')) { padErase(i); return; }
    if (active('duplicate')) { padDuplicate(i, pad); return; }
    if (active('select')) { padSelect(i); return; }
    switch (M.padMode) {
      case 'keyboard': info.key = play(pad, keyboardPitch(i), velocity, { voicing: { chordSet: '' } }); return;
      case 'chords': {
        const set = S().activeChordSet();
        if (set) info.key = play(pad, (st().octave + 1) * 12 + i, velocity);
        else { const scale = st().scale === 'chromatic' ? 'major' : st().scale; info.key = play(pad, keyboardPitch(i, scale), velocity,
            { voicing: { chordSet: '', chord: st().chord === 'single' ? 'scaleTriad' : st().chord, scale } }); }
        return;
      }
      case 'velocity': {
        const target = velocityTarget(), key = 'mk3-pad-' + pad;
        Object.assign(info, { key, note: target.note, track: target.track, velocity: sixteenVelocity(i) });
        // Plain note (no scale / chord voicing) at the pad's velocity; note repeat still applies.
        S().noteOn(target.note, key, true, target.instrument, target.track, false, info.velocity, true);
        if (st().recording && !playing()) scheduleRefresh();
        return;
      }
      case 'step': stepPadDown(pad, i, info); return;
      case 'scene': scenePad(i); return;
      case 'pattern': patternPad(i); return;
      default: {
        const group = groupOf();
        if (group && !M.padAlt) { playMember(pad, i, velocity, info, group); return; }
        if (M.padAlt) { const inst = P().instruments[st().instrument]; info.key = play(pad, clamp((inst?.rootNote ?? 60) + i, 0, 127), velocity,
            { voicing: { chordSet: '', chord: 'single', scale: 'chromatic' } }); return; }
        const index = M.padBank.pad * 16 + i, inst = P().instruments[index]; if (!inst) return;
        st().instrument = index;
        info.key = play(pad, inst.rootNote ?? 60, velocity, { instrument: index, voicing: { chordSet: '', chord: 'single', scale: 'chromatic' } });
        mk.schedule();
      }
    }
  }
  function padUp(pad, cancelled = false) {
    const info = M.pads.get(pad);
    if (!info) return;
    if (info.key && info.pressure > 0) padPressure(info, true);
    M.pads.delete(pad);
    if (info.key) { if (info.release) info.release(); else S().noteOff(info.key); }
    if (info.stepRow != null) {
      if (!cancelled
          && !info.used
          && info.removeOnRelease
          && now() - info.at < 450
          && P() === info.project
          && P().patterns.includes(info.pattern)
          && P().tracks[info.stepTrack] === info.trackOwner
          && info.pattern.steps[info.stepTrack]?.[info.stepRow] === info.stepOwner) {
        const row = info.stepRow,
          track = info.stepTrack;
        S().mutate('remove step', () => {
          info.pattern.steps[track][row] = null;
        }, { render: false });
        scheduleRefresh();
      }
      commitBursts();
    }
  }
  function stepPadDown(pad, i, info) {
    const track = st().track, row = M.stepPage * 16 + i, pat = pattern();
    if (row >= pat.length) return;
    Object.assign(info, {
      stepRow: row,
      stepTrack: track,
      project: P(),
      pattern: pat,
      trackOwner: P().tracks[track],
      stepOwner: stepAt(track, row, pat)
    });
    const s = stepAt(track, row, pat);
    if (s && s.note >= 0) info.removeOnRelease = true; else addStep(track, row);
    st().row = row; scheduleRefresh();
  }
  function scenePad(i) {
    const M_ = window.STSongModel, blocks = M_?.blocks(P().sequence) || [], index = M.padBank.scene * 16 + i, block = blocks[index];
    if (!block) return;
    if (M.shift) {
      M.shiftPads.push(index); const a = Math.min(...M.shiftPads), b = Math.max(...M.shiftPads);
      S().mutate('loop song section', p => M_.setLoop(p, a, b)); flash(`Loop blocks ${a + 1}–${b + 1}`, 900); return;
    }
    // Playing: queue the block (Song mode) or its pattern (Pattern mode) for the end of the current pattern;
    // pressing the queued pad again cancels. Stopped: select the block at once.
    if (playing()) {
      if (M.queued?.kind === 'scene' && M.queued.index === index) { clearQueue(); return; }
      const song = !!st().status?.songMode;
      queueJump({
        kind: 'scene',
        index,
        song,
        sequence: block.start,
        pattern: block.pattern,
        immediate: () => {
          st().sequence = block.start;
          scheduleRefresh();
        }
      });
      flash(`Block ${index + 1} queued`, 700); return;
    }
    st().sequence = block.start; scheduleRefresh(); flash(`Block ${index + 1}`, 600);
  }
  function patternPad(i) {
    const index = M.padBank.pattern * 16 + i, p = P();
    if (index === p.patterns.length) { perform('addPattern'); return; }
    if (!p.patterns[index]) return;
    // Pattern playback: queue the pattern for the end of the playing one (again = cancel). Otherwise select now.
    const pat = p.patterns[index];
    if (playing() && !st().status?.songMode && st().playbackMode === 'pattern') {
      if (M.queued?.kind === 'pattern' && M.queued.pattern === pat.id) { clearQueue(); return; }
      if (pat.id === st().status?.pattern) { if (M.queued) clearQueue(); if (p.activePattern !== pat.id) honourQueue({ pattern: pat.id }); return; }
      queueJump({ kind: 'pattern', song: false, pattern: pat.id, immediate: () => selectPattern(index) });
      flash(`Pattern ${String(index + 1).padStart(2, '0')} queued`, 700); return;
    }
    selectPattern(index);
  }
  /* Tracks have no fixed limit: group buttons show the current bank of eight entries (an ungrouped track or a
     whole track group; Shift + ◄ ► pages banks), and the sixteen mute/solo pads show the two banks (sixteen
     entries) that contain it. */
  const padTrackBase = () => Math.floor(M.bank / 2) * 16;
  const padEntries = () => entries().slice(padTrackBase(), padTrackBase() + 16);
  // PAD MODE with the cursor in a group: pad n plays (and, recording, enters) member n at its pad note.
  const memberPads = () => M.padMode === 'pad' && !M.padAlt ? groupOf() : null;
  function padMember(i, group = groupOf()) { return membersOf(group)[M.padBank.pad * 16 + i]; }
  function playMember(pad, i, velocity, info, group) {
    const index = M.padBank.pad * 16 + i, member = membersOf(group)[index];
    if (member === undefined) return;
    const key = 'mk3-pad-' + pad, t = P().tracks[member];
    info.key = key; info.track = member;
    info.note = Number.isInteger(t.padNote) ? t.padNote : 60;
    app()?.playPad?.(index, key, { record: true, velocity, group });
    rememberMember(member);
    M.lastNote.set(member, info.note);
    if (st().recording && !playing()) scheduleRefresh();
    mk.schedule();
  }
  function padMuteSolo(i, property) {
    const p = P(), group = memberPads();
    if (group) { const member = padMember(i, group); if (member !== undefined) window.STPatternTools?.toggleTrack(property, member); mk.schedule(); return; }
    if (M.padMode === 'pad' && !M.padAlt) {
      const index = M.padBank.pad * 16 + i, tracks = p.tracks.map((t, n) => t.instrument === index ? n : -1).filter(n => n >= 0);
      if (!tracks.length) { flash('No track plays this instrument'); return; }
      const next = !p.tracks[tracks[0]][property];
      S().mutate(`${property} instrument`, q => { for (const n of tracks) q.tracks[n][property] = next; }); return;
    }
    toggleEntry(padEntries()[i], property);
  }
  function padErase(i) {
    const group = memberPads();
    if (group) { const member = padMember(i, group); if (member === undefined) return; clearTrackNotes(member); return; }
    const p = P(), pat = pattern(), track = st().track;
    if (M.padMode === 'step') { const row = M.stepPage * 16 + i; if (pat.steps[track][row]) S().mutate('erase step',
        () => { pattern().steps[track][row] = null; }); return; }
    if (M.padMode === 'scene') { const blocks = window.STSongModel?.blocks(p.sequence)
        || [], block = blocks[M.padBank.scene * 16 + i]; if (block) { st().sequence = block.start; window.STSong?.actions?.remove(); } return; }
    if (M.padMode === 'pattern') { const target = p.patterns[M.padBank.pattern * 16 + i]; if (target
        && target.steps.some(steps => steps.some(Boolean))) S().mutate('erase pattern',
        q => { const item = q.patterns.find(x => x.id === target.id); item.steps = item.steps.map(steps => Array(steps.length).fill(null)); }); return; }
    if (M.padMode === 'keyboard' || M.padMode === 'chords' || M.padMode === 'velocity') {
      const pitch = M.padMode === 'keyboard' ? keyboardPitch(i) : M.padMode === 'velocity' ? velocityTarget().note : null;
      const match = s => s && (pitch === null ? s.note >= 0 && s.chord?.length > 1 : s.note === pitch);
      if (!pat.steps[track].some(match)) { flash('Nothing to erase'); return; }
      S().mutate('erase notes', () => { const steps = pattern().steps[track]; steps.forEach((s, r) => { if (match(s)) steps[r] = null; }); }); return;
    }
    const index = M.padAlt ? null : M.padBank.pad * 16 + i;
    const match = s => s
        && s.note >= 0
        && (index === null
            ? s.instrument === st().instrument && s.note === (P().instruments[st().instrument]?.rootNote ?? 60) + i
            : (s.instrument ?? -1) === index);
    if (!pat.steps.some(steps => steps.some(match))) { flash('No notes of this instrument'); return; }
    S().mutate('erase instrument notes', () => { for (const steps of pattern().steps) steps.forEach((s, r) => { if (match(s)) steps[r] = null; }); });
    flash('Erased');
  }
  function padDuplicate(i) {
    const group = memberPads();
    if (group) { const member = padMember(i, group); if (member === undefined) return; selectTrack(member, { silent: true }); perform('duplicateTrack'); return; }
    const p = P();
    if (M.padMode === 'pattern') { const target = p.patterns[M.padBank.pattern * 16 + i]; if (target) { selectPattern(M.padBank.pattern * 16 + i); perform('duplicatePattern'); } return; }
    if (M.padMode === 'scene') { const blocks = window.STSongModel?.blocks(p.sequence)
        || [], block = blocks[M.padBank.scene * 16 + i]; if (block) { st().sequence = block.start; window.STSong?.actions?.duplicate(); } return; }
    if (M.padMode === 'step') {
      const track = st().track, row = M.stepPage * 16 + i, pat = pattern();
      if (M.dupSource == null) { if (!pat.steps[track][row]) { flash('Choose a step to copy'); return; } M.dupSource = row; flash(`Copy row ${row} → press target pad`,
          900); return; }
      const from = M.dupSource; M.dupSource = null;
      S().mutate('duplicate step', () => { pattern().steps[track][row] = copy(pattern().steps[track][from]); }); return;
    }
    if (M.padMode === 'pad' && !M.padAlt) {
      const index = M.padBank.pad * 16 + i, inst = p.instruments[index]; if (!inst) return;
      if (inst.type === 'au') { flash('Plug-in instruments duplicate with their track (DUPLICATE + group)'); return; }
      S().mutate('duplicate instrument', q => {
        const next = copy(q.instruments[index]);
        next.id = q.instruments.length;
        next.name = `${inst.name} copy`;
        q.instruments.push(next);
        window.STInstrumentEffects?.copy(q, index, next.id);
      });
      flash(`Duplicated ${inst.name}`); return;
    }
    flash('DUPLICATE + pad works on patterns, blocks, steps and instruments');
  }
  function padSelect(i) {
    const group = memberPads();
    if (group) { const member = padMember(i, group); if (member !== undefined) { selectTrack(member); rememberMember(member); } return; }
    const p = P();
    switch (M.padMode) {
      case 'step': { const row = M.stepPage * 16 + i; if (row < pattern().length) setCursor({ row }); return; }
      case 'pattern': { const index = M.padBank.pattern * 16 + i; if (p.patterns[index]) { p.activePattern = p.patterns[index].id; scheduleRefresh(); } return; }
      case 'scene': { const blocks = window.STSongModel?.blocks(p.sequence)
          || [], block = blocks[M.padBank.scene * 16 + i]; if (block) { st().sequence = block.start; scheduleRefresh(); } return; }
      case 'keyboard': S().setPerformance({ root: keyboardPitch(i) % 12 }, false); flash(`Key ${NOTE_NAMES[keyboardPitch(i) % 12]}`, 700); return;
      case 'velocity': return;
      case 'chords': { const sets = S().chordSets; if (sets[i]) { S().setPerformance({ chordSet: sets[i].id },
          false); flash(`Chord set ${sets[i].name}`, 800); } return; }
      default: { const index = M.padBank.pad * 16 + i; if (p.instruments[index] && !M.padAlt) { st().instrument = index; scheduleRefresh(); flash(`Instrument ${String(index).padStart(2, '0')} ${p.instruments[index].name}`,
          700); } }
    }
  }
  // Physical MK3 legends, pad 1 at bottom left. Step Undo/Redo use the app's
  // edit history too; there is no separate Maschine Take Undo history.
  const SHIFT_PADS = ['Undo', 'Redo', 'Step Undo', 'Step Redo', 'Quantize', 'Quantize 50%', 'Nudge ←', 'Nudge →', 'Clear', 'Clear Auto', 'Copy',
      'Paste', 'Semitone −', 'Semitone +', 'Octave −', 'Octave +'];
  function shiftPad(pad) {
    const rows = () => { const b = S().selectedBounds(); return b ? Array.from({ length: b.r1 - b.r0 + 1 }, (_, i) => b.r0 + i) : [st().row]; };
    const actions = [() => historyStep(false), () => historyStep(true), () => historyStep(false), () => historyStep(true),
      () => quantizeRows(rows(), 0), () => quantizeRows(rows(), .5), () => perform('nudgeUp'), () => perform('nudgeDown'),
      () => S().clearSelection(), () => clearAutomation(), () => S().copySelection(), () => S().pasteSelection(),
      () => S().transpose(-1), () => S().transpose(1), () => S().transpose(-12), () => S().transpose(12)];
    if (actions[pad - 1]?.() !== false) flash(SHIFT_PADS[pad - 1], 700);
  }
  // Undo / Redo name the step on the screens (the app's toast is not visible from the controller).
  function historyStep(redo) {
    const landing = () => redo ? st().undo : st().redo, before = landing().length, top = landing().at(-1);
    Promise.resolve(perform(redo ? 'redo' : 'undo')).then(() => {
      const list = landing(), item = list.at(-1);
      if (list.length > before || (item && item !== top)) flash(`${redo ? 'Redo' : 'Undo'} · ${item.label || 'edit'}`, 1000);
      else flash(redo ? 'Nothing to redo' : 'Nothing to undo', 700);
    }, () => {});
    return false;
  }
  function clearAutomation() {
    const track = st().track, pat = pattern();
    const lanes = pat.automation.filter(lane => (lane.track ?? -1) === track || lane.owner === `track:${P().tracks[track]?.routingID}`);
    if (!lanes.length) { flash('No automation on this track'); return false; }
    const removed = new Set(lanes);
    return S().mutate('clear track automation',
        () => { pattern().automation = pattern().automation.filter(lane => !removed.has(lane)); });
  }

  function clearTrackNotes(track) {
    const pat = pattern(); if (!pat.steps[track]?.some(Boolean)) { flash('Nothing to erase'); return; }
    S().mutate('erase track notes', () => { pattern().steps[track] = Array(pattern().length).fill(null); });
    flash(`Erased ${P().tracks[track]?.name || ''}`);
  }

  Object.assign(mk, { keyboardPitch, sixteenVelocity, velocityTarget, padDown, padUp, padTrackBase, padEntries, padMember, memberPads, SHIFT_PADS });
})();
