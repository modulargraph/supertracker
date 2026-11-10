/* MK3: button handlers. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, now, noteText, GROUPS, DISPLAY, PERF_FX, M, active, pattern, playing, flash, selectTrack, perform,
    patternIndex, AR, groupOf, entries, entryName, toggleEntry, memberFor, rememberMember, burstEdit, commitBursts,
    writeNote, tempoCommit, currentPage, eventRows, quantizeRows, setScreen, arrow
  } = mk;

  // ---- Buttons -------------------------------------------------------------------------------------------
  function setPadMode(mode) { if (mode !== 'pattern') M.patternPage = false; M.padMode = mode; if (mode !== 'pad') M.padAlt = false; }
  const patternPageVisible = () => M.padMode === 'pattern' && M.patternPage;
  const modeButton = (screen, shiftScreen) => ({
    press() {
      if (M.shift && shiftScreen) {
        if (typeof shiftScreen === 'function') {
          shiftScreen();
        } else {
          setScreen(M.screen === shiftScreen ? screen : shiftScreen);
        }
      } else {
        setScreen(M.screen === screen && !M.overlay && !patternPageVisible() ? 'notes' : screen);
      }
    }
  });
  const padModeButton = (mode, shiftAction) => ({
    press() {
      if (M.shift && shiftAction) {
        shiftAction();
        return;
      }
      // KEYBOARD while lit: its pad mode page (key, scale, layout, octave), as on Maschine; again closes it.
      if (mode === 'keyboard' && M.padMode === 'keyboard') {
        M.overlay = M.overlay === 'scale' ? null : 'scale';
        return;
      }
      if (mode === 'chords' && M.padMode === 'chords') {
        S().cycleChordSet(1);
        return;
      }
      setPadMode(mode);
      flash(padModeName(), 700);
    }
  });
  const holdButton = (name, shiftAction, onPress, onRelease) => ({ hold: true, shiftAction, onPress, onRelease });
  function toggleParameterOverlay(id) {
    if (!commitBursts()) { flash('Pending controller edit could not finish · retry the gesture'); return; }
    M.overlay = M.overlay === id ? null : id;
  }
  const HANDLERS = {
    channel: modeButton('channel', 'channelMidi'),
    plugin: {
      press() {
        if (M.shift) {
          setScreen('plugin');
          M.overlay = 'pluginSlots';
          return;
        }
        const shown = M.screen === 'plugin' && !M.overlay && !patternPageVisible();
        if (shown && M.pluginFocus === 'pages') {
          M.pluginFocus = 'chain';
          return;
        }
        if (M.screen !== 'plugin') M.pluginFocus = 'chain';
        setScreen(shown ? 'notes' : 'plugin');
      }
    },
    arranger: modeButton('arranger', 'patterns'),
    mixer: modeButton('mixer', 'mixerSends'),
    browser: modeButton('browser', () => setScreen('browser', { pluginTab: true })),
    sampling: modeButton('sampling', () => { window.STRecording?.toggleCapture({}); }),
    settings: modeButton('settings', () => perform('settings')),
    macro: modeButton('macro', () => { setScreen('macro'); M.overlay = 'autoLanes'; }),
    notes: {
      press() {
        if (M.shift) {
          writeNote(-1);
          return;
        }
        if (M.screen === 'notes' && !M.overlay && !patternPageVisible()) {
          S().setNoteRecording();
          return;
        }
        setScreen('notes');
      }
    },
    arrowLeft: { press() { arrow(-1); } }, arrowRight: { press() { arrow(1); } },
    file: {
      press() {
        commitBursts();
        M.latched = null;
        if (M.shift) {
          M.overlay = null;
          perform('save');
          flash('Saving…', 800);
          return;
        }
        M.overlay = M.overlay === 'file' ? null : 'file';
      }
    },
    /* AUTO: a tap toggles Read ⇄ the last write mode. Held from Read, it writes only while held (Maschine): turning
       an encoder, the 4-D encoder or the touch strip meanwhile makes release return to Read. Shift: cycle mode. */
    auto: {
      press() {
        if (M.shift) { M.autoHold = null; perform('automationWriteCycle'); flash(autoLabel(), 900); return; }
        const fromRead = (AR()?.mode || 'read') === 'read';
        perform('automationWriteToggle');
        M.autoHold = fromRead && AR()?.mode && AR().mode !== 'read' ? { at: now(), used: false } : null;
        flash(autoLabel(M.autoHold ? ' · hold + turn to write' : ''), 900);
      },
      release() {
        const hold = M.autoHold; M.autoHold = null;
        if (!hold?.used || (AR()?.mode || 'read') === 'read') return;
        commitBursts();
        AR()?.setMode?.('read', true);
        flash(autoLabel(' · written'), 700);
      }
    },
    volume: holdButton('volume', () => toggleParameterOverlay('velocity')),
    swing: holdButton('swing', () => toggleParameterOverlay('position')),
    tempo: holdButton('tempo', () => toggleParameterOverlay('tune')),
    // Shift + NOTE REPEAT (Arp): the arpeggiator page (mode, rate, octaves, gate); again closes it.
    noteRepeat: holdButton('noteRepeat', () => toggleParameterOverlay('arp'), () => {
      const R = window.STNoteRepeat;
      M.repeatWasOn = !!R?.enabled;
      if (R && !R.enabled) R.set({ enabled: true }, false);
    }, tap => {
      const R = window.STNoteRepeat;
      if (!R) return;
      if (M.latched === 'noteRepeat') return;
      if (!tap || M.repeatWasOn) R.set({ enabled: false }, false);
    }),
    lock: holdButton('lock', () => { commitBursts(); M.latched = null; M.overlay = null; flash('Lock exited · values kept'); }),
    pitch: {
      press() {
        mk.setStripMode(M.strip.mode === 'pitch' ? null : 'pitch', M.shift);
        if (M.strip.mode) flash(M.shift ? 'Strip: transpose selection' : 'Strip: pitch bend', 700);
      }
    },
    mod: {
      press() {
        mk.setStripMode(M.strip.mode === 'mod' ? null : 'mod', M.shift);
        const source = P().instruments[st().instrument];
        if (M.strip.mode) flash(M.shift && source && ['midi', 'au'].includes(source.type)
            ? 'No native cutoff · use MOD for modulation'
            : M.shift
            ? 'Strip: track cutoff'
            : 'Strip: modulation',
            700);
      }
    },
    perform: {
      press() {
        if (M.shift) {
          M.overlay = M.overlay === 'perfFx' ? null : 'perfFx';
          return;
        }
        mk.setStripMode(M.strip.mode === 'perform' ? null : 'perform');
        if (M.strip.mode) flash(`Strip: ${(PERF_FX.find(([key]) => key === M.perfFx) || PERF_FX[0])[1]}`, 700);
      }
    },
    padMode: padModeButton('pad', () => {
      M.padMode = 'pad';
      M.padAlt = !M.padAlt;
      flash(padLabel(), 700);
    }),
    keyboard: padModeButton('keyboard', () => { M.padMode = 'keyboard'; M.overlay = M.overlay === 'scale' ? null : 'scale'; }),
    chords: padModeButton('chords', () => { M.padMode = 'chords'; M.overlay = M.overlay === 'chordSet' ? null : 'chordSet'; }),
    step: padModeButton('step'),
    fixedVel: {
      press() {
        // Shift + FIXED VEL (16 Vel): pads play the cursor track's note at 16 velocities; again returns to the
        // previous pad mode. The velocity curve lives on SETTINGS.
        if (M.shift) {
          if (M.padMode === 'velocity') { setPadMode(M.prevVelocityMode || 'pad'); flash(padModeName(), 700); return; }
          M.prevVelocityMode = M.padMode === 'pattern' ? 'pad' : M.padMode;
          setPadMode('velocity');
          const target = mk.velocityTarget();
          flash(`16 velocities · ${noteText(target.note)} · ${P().tracks[target.track]?.name || ''}`, 1100);
          return;
        }
        M.fixedVel = !M.fixedVel;
        M.overlay = M.fixedVel ? 'fixedVel' : M.overlay === 'fixedVel' ? null : M.overlay;
        flash(M.fixedVel ? `Fixed velocity ${M.fixedVelValue}` : 'Dynamic velocity', 700);
      }
    },
    scene: padModeButton('scene', () => { M.padMode = 'scene'; flash('Shift + pads: loop blocks', 900); }),
    // PATTERN: pads = patterns and the screens show the PATTERN page (list, Length, Lines/beat, Colour);
    // pressing it again returns to the previous pad mode and screen. Shift: new pattern.
    pattern: { press() {
      if (M.shift) { perform('addPattern'); setPadMode('pattern'); M.patternPage = true; M.lists.patterns = Math.max(0, patternIndex()); flash('New pattern', 700); return; }
      if (patternPageVisible()) { setPadMode(M.prevPadMode && M.prevPadMode !== 'pattern'
          ? M.prevPadMode
          : 'pad'); flash('Pattern page closed', 600); return; }
      if (M.padMode !== 'pattern') M.prevPadMode = M.padMode;
      if (!commitBursts()) { flash('Pending controller edit could not finish · retry the gesture'); return; }
      M.overlay = null; setPadMode('pattern'); M.patternPage = true; M.lists.patterns = Math.max(0, patternIndex()); flash('Pads: patterns', 700);
    } },
    events: holdButton('events', () => quantizeRows(eventRows(), 0)),
    variation: {
      press() {
        if (M.shift) {
          M.variationHeld = null;
          M.variationUsed = false;
          M.latched = null;
          setScreen('notes');
          flash('Navigate · turn for rows · tilt for columns and tracks');
          return;
        }
        M.variationHeld = now();
      },
      release() {
        const at = M.variationHeld;
        M.variationHeld = null;
        if (at != null && !M.variationUsed) window.STPatternTools?.humanize(M.variationAmount);
        M.variationUsed = false;
      }
    },
    duplicate: holdButton('duplicate', () => perform('doublePattern')),
    select: holdButton('select', () => perform('selectAll')),
    solo: holdButton('solo', () => { clearFlags('solo'); flash('Solo cleared'); }),
    mute: holdButton('mute', () => { clearFlags('mute'); flash('Mutes cleared'); }),
    restart: { press() { if (M.shift) { perform('loopSelection'); return; } restart(); } },
    erase: holdButton('erase', () => clearTrackPattern()),
    tap: { press() { if (M.shift) { perform('metronome'); flash(P().metronome ? 'Metronome on' : 'Metronome off', 700); return; } tapTempo(); } },
    follow: {
      press() {
        if (M.shift) {
          M.overlay = M.overlay === 'editStep' ? null : 'editStep';
          return;
        }
        perform('follow');
        flash(st().follow ? 'Follow on · step page follows' : 'Follow off', 700);
      }
    },
    play: {
      press() {
        if (M.shift) {
          const song = st().playbackMode !== 'song';
          perform(song ? 'modeSong' : 'modePattern');
          flash(`Playback: ${st().playbackMode === 'song' ? 'Song' : 'Pattern'}`, 800);
          return;
        }
        perform('play');
      }
    },
    rec: {
      press() {
        if (M.shift) {
          countIn();
          return;
        }
        if (M.countIn) {
          perform('stop');
          flash('Count-in cancelled', 700);
          return;
        }
        S().setNoteRecording();
      }
    },
    stop: {
      press() {
        if (M.shift) {
          S().stopHeld();
          S().request('engine.panic').catch(() => {});
          flash('All notes off', 800);
          return;
        }
        perform('stop');
      }
    },
    shift: { press() { M.shift = true; }, release() { M.shift = false; } },
    encoderPush: { press() { currentPage().main?.push?.(true); }, release() { currentPage().main?.push?.(false); } },
    encoderUp: { press() { currentPage().main?.tilt?.('up'); } }, encoderDown: { press() { currentPage().main?.tilt?.('down'); } },
    encoderLeft: { press() { currentPage().main?.tilt?.('left'); } }, encoderRight: { press() { currentPage().main?.tilt?.('right'); } }
  };
  DISPLAY.forEach((name, i) => { HANDLERS[name] = { press() { pressDisplay(i); } }; });
  GROUPS.forEach((name, i) => { HANDLERS[name] = { press() { pressGroup(i); } }; });

  function pressDisplay(i) {
    const page = currentPage(), def = page.buttons?.[i];
    const chosen = M.shift && def?.shift ? def.shift : def;
    if (!chosen?.action) return;
    chosen.action();
  }
  const autoLabel = (suffix = '') => `Automation ${AR()?.labels?.[AR()?.mode] || AR()?.mode || ''}${suffix}`;
  const padModeName = () => ({
    keyboard: 'Pads: keyboard', chords: 'Pads: chords', step: 'Pads: step sequencer', scene: 'Pads: song blocks', pattern: 'Pads: patterns'
  }[M.padMode] || padLabel());
  const padLabel = () => M.padAlt ? 'Pads: slices' : groupOf() ? `Pads: ${groupOf().name}` : 'Pads: instruments';
  // Shift + MUTE / SOLO: clear every track and group-bus flag (one Undo step).
  function clearFlags(property) {
    const p = P(), buses = (p.routing?.buses || []).filter(bus => bus.type === 'group');
    if (!p.tracks.some(t => t[property]) && !buses.some(bus => bus[property])) return;
    S().mutate(property === 'mute' ? 'clear mutes' : 'clear solo', q => {
      q.tracks.forEach(t => { t[property] = false; });
      (q.routing?.buses || []).forEach(bus => { if (bus.type === 'group') bus[property] = false; });
    });
  }
  /* Group buttons A–H: the bank's eight entries (an ungrouped track, or a whole track group). A group selects its
     last selected member (else its first); MUTE / SOLO held toggle the group bus. */
  function pressGroup(i) {
    const list = entries(), entry = list[M.bank * 8 + i], p = P(); if (!entry) return;
    const target = entry.group ? memberFor(entry.group) : entry.index;
    if (active('mute') || active('solo')) { toggleEntry(entry, active('mute') ? 'mute' : 'solo'); return; }
    // DUPLICATE + group: a group entry duplicates the whole group (members, bus and its inserts), as Maschine does.
    if (active('duplicate')) {
      if (entry.group) { duplicateGroup(entry.group); return; }
      selectTrack(target, { silent: true }); perform('duplicateTrack'); return;
    }
    if (active('erase')) { clearTrackPattern(entry.members); return; }
    if (active('select')) { selectTrack(target, { silent: true }); rememberMember(target); return; }
    if (M.shift) { const arming = !(st().recording && entry.members.includes(st().track)); selectTrack(target, { silent: true }); rememberMember(target); S().setNoteRecording(arming); flash(`${entryName(entry, p)}: ${arming ? 'armed' : 'disarmed'}`,
        800); return; }
    selectTrack(target, { silent: !!entry.group });
    rememberMember(target);
    if (entry.group) flash(`${entry.group.name} · ${p.tracks[target]?.name || ''}`, 700);
  }
  function duplicateGroup(group) {
    const run = window.STTrackDuplicate?.duplicateGroup;
    if (!run) { flash('Group duplication is unavailable'); return; }
    flash(`Duplicating ${group.name}…`, 900);
    Promise.resolve(run(group.id)).then(done => { if (done) flash(`${group.name} duplicated`, 900); }).catch(() => {});
  }
  // Clears the current pattern of one track or several (a group's members) in one Undo step.
  function clearTrackPattern(tracks = st().track) {
    const list = [].concat(tracks), pat = pattern();
    if (!list.some(track => pat.steps[track]?.some(Boolean))) { flash('Track pattern already empty'); return; }
    S().mutate('clear track pattern', () => { for (const track of list) pattern().steps[track] = Array(pattern().length).fill(null); });
    flash(`Cleared ${list.length > 1 ? groupOf(list[0])?.name || `${list.length} tracks` : P().tracks[list[0]]?.name}`);
  }
  function restart() {
    const state = st();
    if (state.playing) {
      const pat = P().patterns.find(item => item.id === state.status?.pattern) || pattern(), loop = S().patternLoopFor?.(pat);
      const song = !!state.status?.songMode, sequence = song ? (P().songLoop?.start ?? 0) : state.sequence || 0;
      const target = song ? P().patterns.find(item => item.id === P().sequence[sequence]) || pat : pat;
      S().request('transport.seek', {
        pattern: target.id,
        row: song ? 0 : loop ? loop.start : 0,
        sequence
      }).catch(error => S().toast(error.message, 'error'));
    } else { state.row = 0; state.sequence = 0; perform('play'); }
    flash('Restart', 600);
  }
  function tapTempo() {
    const t = now(); M.taps = M.taps.filter(at => t - at < 2000); M.taps.push(t);
    if (M.taps.length >= 2) {
      const intervals = M.taps.slice(1).map((at, i) => at - M.taps[i]).slice(-4), average = intervals.reduce((a, b) => a + b, 0) / intervals.length;
      const bpm = clamp(Math.round(60000 / average * 10) / 10, 20, 400);
      burstEdit('tempo', 'tap tempo', q => Number(q.timing?.tempos?.[0]?.bpm ?? q.bpm ?? 120), (q, v) => {
        q.bpm = v;
        if (q.timing?.tempos?.[0]) q.timing.tempos[0].bpm = v;
      }, bpm, { commit: tempoCommit });
      flash(`Tap ${bpm.toFixed(1)} BPM`, 900);
    } else flash('Tap tempo…', 900);
  }
  // Shift+REC: one-bar count-in recording (engine clicks, then Play and live recording start on the downbeat).
  function countIn() {
    if (playing()) { S().setNoteRecording(true); flash('Recording', 700); return; }
    if (M.countIn) return;
    Promise.resolve(S().recordWithCountIn?.(1)).then(() => mk.schedule()).catch(error => S().toast(error.message, 'error'));
    flash('Count-in · 1 bar', 900);
  }

  Object.assign(mk, { patternPageVisible, HANDLERS });
})();
