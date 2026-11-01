/* MK3: screen models and publishing (throttled, only on change). */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, now, FRAME_MS, prefs, M, active, pattern, trackColor, patternIndex, AR, exprStreams, expression,
    padPressure, pedal, bursts, commitBursts, currentPage, HANDLERS, stripSync, computeLeds
  } = mk;

  // ---- Output: screens ----------------------------------------------------------------------------------
  function titleModel(page) {
    const state = st(), p = P(), pat = pattern(), status = state.status || {};
    const songMode = state.playbackMode === 'song';
    const pos = state.playing
        ? `${songMode ? 'S' + String((status.sequence ?? 0) + 1).padStart(2, '0') + ' · ' : ''}${String(status.row ?? 0).padStart(3, '0')}`
        : `${String(state.row).padStart(3, '0')}/${String(pat.length - 1).padStart(3, '0')}`;
    return { mode: page.title, track: { index: state.track, name: p.tracks[state.track]?.name || '', color: trackColor(state.track) }, pattern: { number: patternIndex() + 1, name: pat.name }, position: pos,
      bpm: Number(p.timing?.tempos?.[0]?.bpm ?? p.bpm) || 120,
      playing: !!state.playing,
      recording: !!state.recording,
      songMode,
      dirty: !!state.dirty,
      shift: M.shift,
      project: p.name,
      countIn: M.countIn ? M.countIn.remaining || Math.max(1, Math.round(S().beatsPerBar(p)) * (M.countIn.bars || 1)) : null,
      auto: AR()?.mode || 'read',
      // Page indicator (◄ ► grammar) and, on tracker pages, the pattern-position bar under the title.
      pages: page.pages?.length > 1 ? {
        count: page.pages.length,
        index: clamp(Number(page.page) || 0, 0, page.pages.length - 1),
        name: String(page.pages[clamp(Number(page.page) || 0, 0, page.pages.length - 1)] ?? '')
      } : null,
      progress: page.data?.kind === 'grid'
          ? { row: state.row, length: pat.length, playhead: state.playing && status.pattern === pat.id ? Number(status.row) || 0 : -1 }
          : null };
  }
  function screenModel(page = currentPage()) {
    const touchedIndex = [...M.touched].filter(name => /^encoder[1-8]$/.test(name)).map(name => Number(name.slice(7)) - 1).pop();
    const knob = touchedIndex != null ? page.knobs?.[touchedIndex] : null;
    if (M.flash && M.flash.until < now()) M.flash = null;
    const buttons = (page.buttons || []).slice(0, 8).map(def => {
      const chosen = M.shift && def?.shift ? def.shift : def;
      return chosen ? {
        label: chosen.label || '',
        on: !!chosen.on,
        enabled: !!chosen.action,
        color: chosen.color || null,
        mute: !!chosen.mute,
        ...(chosen.select ? { select: true, solo: !!chosen.solo } : {})
      } : null;
    });
    while (buttons.length < 8) buttons.push(null);
    const knobs = (page.knobs || []).slice(0, 8).map(k => k ? {
      label: k.label,
      value: k.value,
      norm: k.norm ?? null,
      bipolar: !!k.bipolar,
      lockable: active('lock') && !!k.lock,
      ...(k.dim ? { dim: true } : {})
    } : null);
    while (knobs.length < 8) knobs.push(null);
    return { page: { id: page.id, title: page.title, data: page.data || {}, overlay: !!page.overlay }, title: titleModel(page), buttons, knobs,
      touch: knob ? {
        index: touchedIndex,
        name: knob.name || knob.label,
        value: knob.value,
        norm: knob.norm ?? null,
        ...(knob.list ? { list: true } : {}),
        ...(Array.isArray(knob.options) && knob.options.length > 2 ? {
          options: knob.options.map(String),
          selected: clamp(Number(knob.index) || 0, 0, knob.options.length - 1)
        } : {})
      } : null,
      flash: M.flash ? M.flash.text : null, lock: active('lock'), brightness: prefs.screenBrightness, padMode: M.padMode, strip: M.strip.mode, held: [...M.held.keys()] };
  }
  function frame() {
    syncView();
    const page = currentPage();
    const leds = computeLeds(page);
    const screens = window.STMK3Screens ? window.STMK3Screens.render(screenModel(page)) : { left: [], right: [] };
    return { leds, screens };
  }

  // ---- Publishing (throttled, only on change) ----------------------------------------------------------
  const sent = { leds: null, screen: '', left: '', right: '' };
  const listeners = new Set();
  let frameTimer = null;
  let ticker = null;
  function wanted() { return M.connected || listeners.size > 0; }
  function schedule() { if (frameTimer || !wanted()) return; frameTimer = setTimeout(publish, FRAME_MS); }
  /* Pages and banks are controller state over project collections that can shrink under them (shorter pattern,
     fewer tracks, instruments, patterns or song blocks, another project): keep them in range, or the pads and
     groups go dark and stop responding. The group bank follows a track selected anywhere in the app. */
  function clampPages() {
    const p = P(), pat = pattern(); if (!p || !pat) return;
    const pages = (count, size) => Math.max(0, Math.ceil(count / size) - 1);
    M.stepPage = clamp(M.stepPage, 0, pages(pat.length, 16));
    // Banks of eight group-button entries (tracks and track groups); the bank follows the cursor track's entry.
    const list = mk.entries(p), banks = pages(list.length, 8), group = mk.groupOf(st().track, p);
    const trackKey = `${st().track}:${p.tracks[st().track]?.routingID || ''}`;
    if (trackKey !== M.lastTrack) {
      M.lastTrack = trackKey;
      if (Number.isInteger(st().track)) M.bank = Math.max(0, Math.floor(mk.entryIndex(st().track, list) / 8));
      // A group's pads follow its member under the cursor, and the group remembers it for its group button.
      if (group) { M.padBank.pad = Math.floor(Math.max(0, mk.membersOf(group, p).indexOf(st().track)) / 16); mk.rememberMember(st().track, p); }
    }
    M.bank = clamp(M.bank, 0, banks);
    M.padBank.pad = clamp(M.padBank.pad, 0, pages(group ? mk.membersOf(group, p).length : p.instruments.length, 16));
    M.padBank.pattern = clamp(M.padBank.pattern, 0, pages(p.patterns.length + 1, 16));
    M.padBank.scene = clamp(M.padBank.scene, 0, pages(window.STSongModel?.blocks(p.sequence).length || 0, 16));
  }
  function syncView() {
    clampPages();
    // The app view changed outside the controller: follow it so screens show what the app shows.
    const view = st().view; if (view === M.lastView) return; M.lastView = view;
    const map = {
      pattern: ['notes', 'macro', 'autoLanes'],
      instrument: ['channel', 'channelMidi', 'plugin', 'sampling'],
      song: ['arranger', 'patterns'],
      mixer: ['mixer', 'mixerSends'],
      browser: ['browser']
    }[view];
    if (map && !map.includes(M.screen)) { M.screen = map[0]; M.overlay = null; }
  }
  function publish() {
    frameTimer = null;
    if (!P()) return;
    syncView();
    let output;
    try { output = frame(); } catch (error) { console.error?.('MK3 frame failed', error); return; }
    const { leds, screens } = output;
    // LEDs: only changed parts.
    const delta = {}, last = sent.leds;
    if (!last) Object.assign(delta, leds);
    else {
      const buttons = {}; for (const [name, value] of Object.entries(leds.buttons)) if (last.buttons[name] !== value) buttons[name] = value;
      if (Object.keys(buttons).length) delta.buttons = buttons;
      for (const key of ['groups', 'pads', 'strip', 'nav']) if (JSON.stringify(last[key]) !== JSON.stringify(leds[key])) delta[key] = leds[key];
    }
    // Screens: only the side that changed (the native side renders and diffs each screen it receives).
    const sides = {}, json = {};
    for (const side of ['left', 'right']) { json[side] = JSON.stringify(screens[side]); if (json[side] !== sent[side]) sides[side] = screens[side]; }
    const screenChanged = Object.keys(sides).length > 0;
    sent.leds = leds;
    if (screenChanged) { sent.left = json.left; sent.right = json.right; sent.screen = json.left + json.right; }
    if (st().native && M.connected) {
      if (Object.keys(delta).length) S().request('mk3.leds', delta).catch(() => {});
      if (screenChanged && M.screens) S().request('mk3.screen', sides).catch(() => {});
    }
    for (const listener of listeners) { try { listener({ leds, screens, delta, screenChanged }); } catch { /* listener failures are local */ } }
  }
  function startTicker() {
    if (ticker) return;
    ticker = setInterval(() => {
      if (wanted()) {
        schedule();
      } else {
        clearInterval(ticker);
        ticker = null;
      }
    }, 100);
  }

  // Cancel physical gestures without interpreting synthetic releases as taps (Erase, File, step pads).
  function cancelGestures({ projectChanged = false } = {}) {
    clearTimeout(M.stripTimer); clearTimeout(mk.previewTimer); mk.previewTimer = null;
    for (const stream of exprStreams.values()) clearTimeout(stream.timer);
    exprStreams.clear();
    for (const [pad, info] of M.pads) {
      M.pads.delete(pad);
      if (info.key) {
        if (info.pressure > 0) padPressure(info, true);
        if (info.release) {
          info.release();
        } else {
          S().noteOff(info.key);
        }
      }
    }
    if (M.pedal) pedal(false);
    if (M.bendTrack != null) expression('pitchBend', 0, { track: M.bendTrack, flush: true });
    if (M.modTrack != null) expression('cc', 0, { track: M.modTrack, controller: 1, flush: true });
    if (M.pitchRef) AR()?.end?.(M.pitchRef);
    M.bendTrack = M.modTrack = M.pitchRef = null;
    const repeated = M.held.has('noteRepeat') || M.latched === 'noteRepeat';
    M.held.clear(); M.latched = null;
    if (repeated) HANDLERS.noteRepeat.onRelease(false);
    M.shift = false; M.fileMenu = null; M.overlay = null; M.variationHeld = null; M.variationUsed = false; M.autoHold = null;
    M.touched.clear(); M.mainTouch = null; M.encoderAcc.clear(); M.events.clear(); M.shiftPads = []; M.dupSource = null; M.queued = null;
    M.strip.touched = false; M.strip.shift = false; M.strip.owner = null;
    if (M.stutter && !projectChanged) {
      st().patternLoop = M.stutterRestore || null;
      S().request('transport.loop', { loop: S().patternLoopFor?.() || null }).catch(() => {});
    }
    M.stutter = M.stutterRestore = null;
    const hadPreview = !!mk.stripPreview; mk.stripPreview = null;
    if (projectChanged) { for (const burst of bursts.values()) clearTimeout(burst.timer); bursts.clear(); mk.editGeneration++; }
    else { commitBursts(); if (hadPreview) stripSync(); }
    schedule();
  }

  Object.assign(mk, { screenModel, frame, sent, listeners, schedule, syncView, publish, startTicker, cancelGestures });
})();
