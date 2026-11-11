/* MK3: LED output. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, clamp01, now, GROUPS, LED_BUTTONS, SCREEN_BUTTON, prefs, M, isHeld, active,
    pattern, trackColor, stepAt, AR, entries, entryColor, entryFlags, anySolo, toNorm, keyboardPitch, padEntries,
    padMember, memberPads
  } = mk;

  // ---- Output: LEDs ----------------------------------------------------------------------------------------
  /* LED colour: the colour's hue and saturation at brightness `scale`. The colour is first brought to full value,
     so a muted track colour (the app palette tops out around 70 %) still reaches the hardware's brightest
     intensity when selected and its scaled levels stay distinct after the native 4-level quantization. */
  function hex(color, scale = 1) {
    const value = String(color || '#000000').replace('#', ''),
      n = value.length === 3 ? value.split('').map(c => c + c).join('') : value.padEnd(6, '0');
    const raw = [0, 2, 4].map(i => parseInt(n.slice(i, i + 2), 16) || 0), full = Math.max(...raw) ? 255 / Math.max(...raw) : 0;
    const parts = raw.map(v => clamp(Math.round(v * full * scale * prefs.ledBrightness), 0, 255));
    return '#' + parts.map(v => v.toString(16).padStart(2, '0')).join('');
  }
  const pulse = (period = 600) => Math.floor(now() / period) % 2 === 0;
  function computeLeds(page) {
    const state = st(), p = P(), b = {}, set = (name, value) => { b[name] = Number(clamp(value, 0, 1).toFixed(2)); };
    const screenOn = name => SCREEN_BUTTON[M.screen] === name;
    for (const name of LED_BUTTONS) set(name, 0);
    for (const name of ['channel', 'plugin', 'arranger', 'mixer', 'browser', 'sampling', 'settings', 'macro',
        'notes']) set(name, screenOn(name) ? 1 : .2);
    if (window.STRecording?.isRecording) set('sampling', pulse(400) ? 1 : .4);
    const scroll = page.id === 'channel' ? page.pages?.length > 1 : page.id === 'plugin' ? page.pages?.length > 1 : true;
    set('arrowLeft', isHeld('arrowLeft') ? 1 : scroll ? .45 : .1); set('arrowRight', isHeld('arrowRight') ? 1 : scroll ? .45 : .1);
    set('file', M.overlay === 'file' ? 1 : state.dirty ? .6 : .2);
    set('auto', AR()?.mode && AR().mode !== 'read' ? (AR().isRecording && pulse(300) ? .5 : 1) : .2);
    page.buttons?.forEach((def, i) => {
      const chosen = M.shift && def?.shift ? def.shift : def;
      set('display' + (i + 1), !chosen?.action ? 0 : chosen.on ? 1 : .3);
    });
    for (const name of ['volume', 'swing', 'tempo', 'lock', 'events', 'duplicate', 'select', 'solo', 'mute',
        'erase']) set(name, active(name) ? 1 : .2);
    if (M.overlay === 'master') set('volume', 1);
    set('noteRepeat', window.STNoteRepeat?.enabled ? (M.latched === 'noteRepeat' ? 1 : .8) : M.overlay === 'arp' ? .6 : .2);
    set('variation', M.variationHeld != null ? 1 : .2);
    set('pitch', M.strip.mode === 'pitch' ? 1 : .2);
    set('mod', M.strip.mode === 'mod' ? 1 : .2);
    set('perform', M.strip.mode === 'perform' || M.overlay === 'perfFx' ? 1 : .2);
    set('padMode', M.padMode === 'pad' ? 1 : .2);
    set('keyboard', M.padMode === 'keyboard' ? 1 : .2);
    set('chords', M.padMode === 'chords' ? 1 : .2);
    set('step', M.padMode === 'step' ? 1 : .2);
    set('scene', M.padMode === 'scene' ? 1 : .2); set('pattern', M.padMode === 'pattern' ? 1 : .2); set('fixedVel', M.fixedVel || M.padMode === 'velocity' ? 1 : .2);
    set('restart', S().patternLoopFor?.() || p.songLoop ? 1 : .3);
    const beatMs = 60000 / (Number(p.bpm) || 120);
    set('tap', state.playing ? (Number(state.status?.seconds) * 1000 % beatMs < beatMs / 4 ? 1 : .25) : .25);
    set('follow', state.follow ? 1 : .2);
    set('play', state.playing ? 1 : .25);
    set('rec', M.countIn ? (pulse(150) ? 1 : .2) : state.recording ? 1 : .25);
    set('stop', state.playing ? .4 : .25);
    set('shift', M.shift ? 1 : .2);
    // Group buttons: the bank's entries (track or track group); bright = holds the cursor track, off = muted.
    const list = entries(), soloing = anySolo(p);
    const groups = GROUPS.map((_, i) => {
      const entry = list[M.bank * 8 + i]; if (!entry) return null;
      const flags = entryFlags(entry, p), soloed = flags.solo || (entry.group && entry.members.some(n => p.tracks[n]?.solo));
      if (flags.mute || (soloing && !soloed)) return null;
      const selected = entry.members.includes(state.track);
      if (selected && state.recording) return hex(entryColor(entry), pulse(350) ? 1 : .35);
      return hex(entryColor(entry), selected ? 1 : .25);
    });
    const pads = padColors();
    const nav = {
      up: page.main?.tilt ? hex('#ffffff', .4) : null,
      down: page.main?.tilt ? hex('#ffffff', .4) : null,
      left: page.main?.tilt ? hex('#ffffff', .4) : null,
      right: page.main?.tilt ? hex('#ffffff', .4) : null
    };
    return { buttons: b, groups, pads, strip: stripLeds(), nav };
  }
  function padColors() {
    const state = st(), p = P(), pads = Array(16).fill(null), pressed = pad => M.pads.has(pad), group = memberPads();
    const white = scale => hex('#ffffff', scale);
    const mark = colors => colors.map((color, i) => pressed(i + 1)
        ? color ? hex(color.replace('#', '').length ? color : '#ffffff', 1.6) : white(.7)
        : color);
    if (M.countIn) { const remaining = M.countIn.remaining || 0; return pads.map((_, i) => i < remaining
        ? (i === remaining - 1 ? hex('#ff4040', 1) : white(.7))
        : null); }
    if (M.shift && !active('events') && M.padMode !== 'scene') return pads.map((_, i) => hex(i < 2 ? '#e0c060' : i < 8 ? '#80b0ff' : '#c08060', .5));
    if (active('mute') || active('solo')) {
      const prop = active('mute') ? 'mute' : 'solo';
      if (group) return mark(pads.map((_, i) => { const member = padMember(i, group), t = p.tracks[member]; return t
          ? hex(trackColor(member), prop === 'mute' ? (t.mute ? .08 : .9) : (t.solo ? 1 : .15))
          : null; }));
      if (M.padMode === 'pad' && !M.padAlt) return mark(pads.map((_, i) => { const index = M.padBank.pad * 16 + i, tracks = p.tracks.filter(t => t.instrument === index); if (!tracks.length) return null; const on = tracks.some(t => t[prop]); return hex(p.tracks.indexOf(tracks[0]) >= 0
          ? trackColor(p.tracks.indexOf(tracks[0]))
          : '#888888',
          prop === 'mute' ? (on ? .08 : .9) : (on ? 1 : .15)); }));
      const list = padEntries();
      return mark(pads.map((_, i) => { const entry = list[i]; if (!entry) return null; const flags = entryFlags(entry, p); return hex(entryColor(entry),
          prop === 'mute' ? (flags.mute ? .08 : .9) : (flags.solo ? 1 : .15)); }));
    }
    const stepColors = (highlight) => {
      const pat = pattern(), track = state.track, color = trackColor(track), start = M.stepPage * 16;
      const play = state.playing && state.status?.pattern === pat.id ? Number(state.status.row) : -1;
      return pads.map((_, i) => { const row = start + i; if (row >= pat.length) return null; if (row === play) return white(.9); if (highlight?.has(row)) return hex('#ffd060', .9); const s = stepAt(track, row, pat); if (s && s.note >= 0) return hex(color, row === state.row ? 1 : .7); if (s) return hex(color, .3); return row % 4 === 0 ? white(.06) : null; });
    };
    if (active('events')) return mark(stepColors(M.events));
    // FOLLOW (the app's follow-playhead) also makes the STEP page follow the playhead, as in Maschine.
    if (state.follow
        && M.padMode === 'step'
        && state.playing
        && state.status?.pattern === pattern().id) M.stepPage = Math.floor(Number(state.status.row) / 16);
    switch (M.padMode) {
      case 'keyboard': case 'chords': {
        const set = S().activeChordSet(), color = trackColor(state.track);
        if (M.padMode === 'chords' && set) return mark(pads.map((_, i) => hex(color,
            i < set.chords.length ? (i % set.chords.length === 0 ? 1 : .55) : .2)));
        const scale = S().scales[state.scale] || S().scales.chromatic;
        return mark(pads.map((_, i) => { const pitch = keyboardPitch(i), pc = ((pitch - state.root) % 12 + 12) % 12; if (pc === 0) return hex(color, 1); return state.scale === 'chromatic'
            ? ([1, 3, 6, 8, 10].includes(pc) ? white(.08) : hex(color, .35))
            : (scale.includes(pc) ? hex(color, .4) : null); }));
      }
      case 'step': return mark(stepColors(null));
      // 16 Velocities: the cursor track's colour from dim (pad 1, softest) to full (pad 16, 127).
      case 'velocity': return mark(pads.map((_, i) => hex(trackColor(state.track), .12 + .88 * i / 15)));
      case 'scene': {
        const M_ = window.STSongModel,
          blocks = M_?.blocks(p.sequence) || [],
          current = M_?.blockAt(p.sequence, state.sequence || 0),
          loop = M_?.loopRange(p);
        const playingBlock = state.playing && state.status?.songMode && Number.isInteger(state.status.sequence)
            ? M_.blockAt(p.sequence, state.status.sequence)
            : -1;
        return mark(pads.map((_, i) => { const index = M.padBank.scene * 16 + i, block = blocks[index]; if (!block) return null; if (M.queued?.kind === 'scene'
            && M.queued.index === index) return pulse(125)
            ? hex(M_.patternColor(p, block.pattern), 1)
            : white(.15); if (index === playingBlock) return pulse(300)
                ? white(1)
                : hex(M_.patternColor(p, block.pattern), 1); const inLoop = loop
                    && block.start >= loop.start
                    && block.start <= loop.end; return hex(M_.patternColor(p, block.pattern),
            index === current ? 1 : inLoop ? .6 : .3); }));
      }
      case 'pattern': {
        const playingId = state.playing ? state.status?.pattern : null;
        return mark(pads.map((_, i) => { const index = M.padBank.pattern * 16 + i, pat = p.patterns[index]; if (!pat) return index === p.patterns.length
            ? white(.08)
            : null; const color = window.STSongModel?.patternColor(p, pat.id)
            || '#a0a0a0'; if (M.queued?.kind === 'pattern'
            && M.queued.pattern === pat.id) return pulse(125) ? hex(color, 1) : white(.15); if (pat.id === playingId
                && pat.id !== p.activePattern) return pulse(300) ? hex(color, 1) : white(.5); return hex(color,
                    pat.id === p.activePattern ? 1 : .3); }));
      }
      default: {
        // Cursor in a track group: pads = its members (bright = cursor track, dim = other members, off = none).
        if (group) return mark(pads.map((_, i) => { const member = padMember(i, group), t = p.tracks[member]; if (!t) return null; return hex(trackColor(member),
            member === state.track ? 1 : t.mute ? .08 : .3); }));
        if (M.padAlt) { const inst = p.instruments[state.instrument], count = inst?.type === 'sample'
            ? (inst.slicePoints?.length || inst.sliceCount || 16)
            : 16; return mark(pads.map((_, i) => i < count ? hex(trackColor(state.track), i === 0 ? 1 : .45) : null)); }
        return mark(pads.map((_, i) => { const index = M.padBank.pad * 16 + i, inst = p.instruments[index]; if (!inst) return null; const track = p.tracks.findIndex(t => t.instrument === index); const color = track >= 0
            ? trackColor(track)
            : '#9a9a9a'; return hex(color,
            index === state.instrument ? 1 : track >= 0 ? .4 : .15); }));
      }
    }
  }
  function stripLeds() {
    const leds = Array(25).fill(0), state = st();
    if (M.strip.mode === 'pitch'
        || (M.strip.mode === 'mod' && !M.strip.shift)
        || M.strip.mode === 'perform'
        || (M.strip.mode === 'mod' && M.strip.shift)) {
      const value = M.strip.mode === 'mod' && M.strip.shift ? clamp01(toNorm(Number(P().instruments[st().instrument]?.cutoff ?? 18000), {
        min: 20,
        max: 20000,
        curve: 'log'
      })) : M.strip.value;
      const at = Math.round(value * 24);
      if (M.strip.mode === 'pitch') { const lo = Math.min(at, 12), hi = Math.max(at, 12); for (let i = lo; i <= hi; i++) leds[i] = i === at ? 1 : .35; leds[12] = Math.max(leds[12], .5); }
      else for (let i = 0; i <= at; i++) leds[i] = i === at ? 1 : .3;
      return leds;
    }
    const pat = pattern();
    if (isHeld('restart')) { const loop = S().patternLoopFor?.(pat); if (loop) for (let i = 0; i < 25; i++) { const row = i / 24 * (pat.length - 1); leds[i] = row >= loop.start && row <= loop.end
        ? .8
        : 0; } return leds; }
    const row = state.playing && state.status?.pattern === pat.id ? Number(state.status.row) : state.row;
    const at = Math.round(row / Math.max(1, pat.length - 1) * 24);
    for (let i = 0; i < 25; i++) leds[i] = i === at ? 1 : i < at && state.playing ? .15 : 0;
    return leds;
  }

  Object.assign(mk, { computeLeds });
})();
