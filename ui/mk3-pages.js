/* MK3: pages for notes, macros, automation lanes, channel and levels. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, now, noteText, fxText, M, active, pattern, trackColor, playing,
    patternRow, flash, scheduleRefresh, selectTrack, setCursor, homeColumns, perform, stepAt, AR, G, app, groupOf,
    membersOf, busOf, channelOf, ownerMixKnob, burstValue, enumKnob, actionKnob, button, fmt,
    instKnob, clearLocks, pageIndex, listMain, listData, cursorMain, stepKnobs, fxKnobs, typedFxKnob,
    patternToolButtons, columnButtons, homeKnobs
  } = mk;

  // The 4-D encoder's touch sensor shows the pattern overview only after a still touch (no turn) of
  // OVERVIEW_MS, or at once with Shift; turning never swaps the screen.
  const OVERVIEW_MS = 400;
  function overviewVisible() {
    const t = M.mainTouch; if (!t) return false;
    // Once shown it stays until release, so the cursor can be turned with the overview in view.
    if (!t.shown && (t.shift || (!t.turned && now() - t.at >= OVERVIEW_MS))) t.shown = true;
    return !!t.shown;
  }
  function notesPage() {
    const { columns } = homeColumns(8);
    return { id: 'notes', title: 'Home', main: cursorMain(),
      buttons: active('select') ? patternToolButtons() : columnButtons(columns),
      knobs: homeKnobs(),
      data: { kind: 'grid', grid: gridData(8), step: stepAt(), overview: overviewVisible()
          ? overviewData()
          : null, perf: S().performanceSummary() } };
  }
  function writeNote(value) {
    const track = st().track, row = st().row;
    S().mutate(value === -1 ? 'insert note off' : 'insert note cut', p => {
      const pat = p.patterns.find(item => item.id === p.activePattern);
      pat.steps[track][row] = { ...(pat.steps[track][row] || {}), note: value, instrument: p.tracks[track]?.instrument ?? 0, velocity: 100, gate: 0 };
      // An OFF / CUT is no chord: the row's FX and extra note columns stay, the replaced note's chord does not.
      delete pat.steps[track][row].chord;
      if (!st().playing) st().row = (row + (st().editStep || 0)) % pat.length;
    });
  }
  function selectPattern(index) {
    const p = P(), next = p.patterns[clamp(index, 0, p.patterns.length - 1)]; if (!next) return;
    if (next.id === p.activePattern) return;
    p.activePattern = next.id; st().row = Math.min(st().row, next.length - 1); st().selection = null; M.lists.patterns = p.patterns.indexOf(next);
    if (st().patternLoop && st().patternLoop.pattern !== next.id) {
      st().patternLoop = null;
      if (playing()) S().request('transport.loop', { loop: null }).catch(() => {});
    }
    S().syncEngine?.();
    if (playing() && st().playbackMode === 'pattern') S().request('transport.seek', {
      pattern: next.id,
      row: (Number(st().status?.row) || 0) % next.length,
      sequence: st().sequence || 0
    }).catch(error => S().toast(error.message, 'error'));
    scheduleRefresh(); flash(`Pattern ${String(index + 1).padStart(2, '0')} · ${next.name}`, 700);
  }
  /** Grid data: HOME_ROWS rows around the cursor (or the followed playhead) for `width` columns. */
  const HOME_ROWS = 11;
  function gridData(width = 8) {
    const p = P(), pat = pattern(), state = st(), rows = HOME_ROWS, half = Math.floor(HOME_ROWS / 2);
    const center = state.playing && state.follow && state.status?.pattern === pat.id ? patternRow() : state.row;
    const start = clamp(center - half, 0, Math.max(0, pat.length - rows));
    const tracks = homeColumns(width).columns;
    const sel = S().selectedBounds(), loop = S().patternLoopFor?.(pat), lpb = p.linesPerBeat, bar = lpb * S().beatsPerBar(p);
    const playRow = state.playing && state.status?.pattern === pat.id ? Number(state.status.row) : -1;
    // EVENTS: the rows the encoders edit are marked on the cursor column.
    const events = active('events') ? new Set(mk.eventRows()) : null;
    const current = c => c.index === state.track || !!c.members?.includes(state.track);
    return { length: pat.length, cursor: { track: state.track, row: state.row, column: state.column },
      tracks: tracks.map(c => ({ index: c.index, name: c.name, color: c.color, mute: c.mute, solo: c.solo, current: current(c),
        ...(c.collapsed ? { collapsed: true, members: c.members } : {}) })),
      rows: Array.from({ length: Math.min(rows, pat.length) }, (_, i) => {
        const r = start + i;
        return { row: r, beat: r % lpb === 0, bar: r % bar === 0, playhead: r === playRow, loop: !!loop && r >= loop.start && r <= loop.end,
          cells: tracks.map(c => {
            const t = c.index, inSelection = n => !!sel && r >= sel.r0 && r <= sel.r1 && n >= sel.t0 && n <= sel.t1;
            // A collapsed group: the members that hit on this row (member number, and the pitch when it is not the pad note).
            if (c.collapsed) return { track: t, current: current(c), selected: c.members.some(inSelection), summary: memberHits(p, pat, c.members, r) };
            const s = pat.steps[t]?.[r];
            return { track: t, current: current(c), selected: inSelection(t), chord: s?.chord?.length > 1,
              ...(events?.has(r) && t === state.track ? { event: true } : {}),
              text: [noteText(s?.note), s ? String(s.instrument ?? p.tracks[t].instrument).padStart(2, '0') : '··',
                s ? String(s.velocity ?? 100).padStart(3, '0') : '···', fxText(s?.fx1), fxText(s?.fx2)] };
          }) };
      }) };
  }
  function memberHits(p, pat, members, row) {
    return members.map((track, n) => {
      const s = pat.steps[track]?.[row]; if (!s) return null;
      const padNote = Number.isInteger(p.tracks[track]?.padNote) ? p.tracks[track].padNote : 60;
      return s.note === padNote ? String(n + 1) : `${n + 1}:${noteText(s.note)}`;
    }).filter(Boolean).join(' ');
  }
  function overviewData() {
    const pat = pattern(), track = st().track, bins = Math.min(64, pat.length);
    const density = Array.from({ length: bins }, (_, b) => {
      let n = 0;
      const r0 = Math.floor(b / bins * pat.length),
        r1 = Math.floor((b + 1) / bins * pat.length);
      for (let r = r0; r < Math.max(r1, r0 + 1); r++) for (let t = 0; t < pat.steps.length; t++) if (pat.steps[t]?.[r]) n += t === track ? 2 : 1;
      return n;
    });
    const loop = S().patternLoopFor?.(pat);
    return { length: pat.length, density, max: Math.max(1, ...density), playhead: playing() && st().status?.pattern === pat.id
        ? Number(st().status.row)
        : -1, cursor: st().row, loop };
  }

  function macroPage() {
    const [t1, v1] = fxKnobs(1), [t2, v2] = fxKnobs(2);
    const quick = (label, type, value) => button(label, () => {
      if (!stepAt() || !(stepAt().note >= 0)) {
        flash('Step effects need a note on the cursor row');
        return;
      }
      withSelection(() => window.STPatternTools?.applyEffect(type, value));
    });
    return { id: 'macro', title: 'Step FX', main: cursorMain(),
      buttons: [quick('Prob 50%', 'probability', .5), quick('Ratchet ×2', 'ratchet', 2), quick('µTime 25%', 'microtiming', .25),
          quick('Glide −12', 'glide', -12), quick('Reverse', 'reverse', 1), quick('Offset 50%', 'offset', .5), quick('Arp maj', 'arpeggio', 0x47),
          button('Clear FX', () => clearLocks())].map((b, i) => ({
        ...b,
        shift: [button('Prob 25%', () => quick('', 'probability', .25).action()), button('Ratchet ×4', () => quick('', 'ratchet', 4).action()),
            button('µTime 50%', () => quick('', 'microtiming', .5).action()), button('Arp min', () => quick('', 'arpeggio', 0x37).action()),
            button('Offset 25%', () => quick('', 'offset', .25).action()), button('Interp FX', () => perform('interpolate')),
            button('FX picker', () => perform('stepEffect')), button('Clear sel FX', () => perform('clearEffects'))][i]
      })),
      knobs: [t1, v1, t2, v2, typedFxKnob('probability', 'Prob'), typedFxKnob('ratchet', 'Ratchet'), typedFxKnob('microtiming', 'µTime'),
          stepKnobs()[3]],
      data: { kind: 'grid', grid: gridData(4), step: stepAt(), fx: true } };
  }
  // Pattern tools act on the app selection or the whole track; a quick step effect applies to the cursor row.
  function withSelection(fn) {
    const state = st(),
      had = state.selection;
    if (!had) state.selection = { track: state.track, row: state.row };
    try {
      return fn();
    } finally {
      if (!had) state.selection = null;
    }
  }

  function autoLanesPage() {
    const track = st().track,
      name = P().tracks[track]?.name,
      choices = (window.STStudio?.automationChoices?.(track) || []).filter(item => item.available);
    const index = clamp(M.lists.autoLanes, 0, Math.max(0, choices.length - 1)); M.lists.autoLanes = index;
    const choose = () => { const item = choices[index]; if (!item) return; S().automate(track, item.target, `${name} · ${item.name}`, item.value); };
    const move = delta => { M.lists.autoLanes = clamp(index + delta, 0, Math.max(0, choices.length - 1)); };
    return { id: 'autoLanes', title: 'Automation lanes', overlay: true,
      main: listMain({ count: choices.length, move, choose }),
      buttons: [button('Open lane', choose), button('Picker', () => perform('automateParameter')), button(`Write: ${AR()?.mode || 'read'}`, () => perform('automationWriteToggle'), {
        on: AR()?.mode && AR().mode !== 'read'
      }), button('Mode', () => perform('automationWriteCycle')), button('Disarm all',
          () => perform('automationClearArms')), null, null, button('Back', () => {
        M.overlay = null;
      })],
      knobs: [actionKnob('Parameter', choices[index]?.name || '–', move, { detent: .04 }), null, null, null, null, null, null, null],
      data: listData('autoLanes', choices.map(item => ({ name: item.name, right: item.automated
          ? `${item.points} pts`
          : item.group, detail: item.group
          + (item.automated ? ` · ${item.points} points` : ''), on: item.automated })), index,
        { title: `Automate · ${name}`, empty: 'No automatable parameters', hint: 'Turn: parameter · push: open lane · MACRO: back' }) };
  }

  // Sampler pages (CHANNEL and PLUG-IN · Instr.).
  function instrumentGroups(inst) {
    const mode = inst.mode, loop = ['forward', 'reverse', 'pingpong'].includes(mode);
    const play = mode === 'wavetable'
        ? ['wavetablePosition', 'mode', 'rootNote', 'wavetableFrameSize', 'start', 'end', null, null]
        : mode === 'granular'
        ? ['grainPosition', 'grainSize', 'grainDensity', 'grainSpray', 'grainScan', 'grainPitchSpread', 'grainReverse', 'grainStereo']
        : ['mode', 'rootNote', 'start', 'end', loop ? 'loopStart' : mode === 'slice' ? 'sliceCount' : null,
            loop ? 'loopEnd' : null, null, null];
    const groups = [['Main', ['volume', 'pan', 'tuneSemi', 'tuneFine', 'cutoff', 'resonance', 'drive', 'bitDepth']],
      ['Envelopes', ['attack', 'decay', 'sustain', 'release', 'filterAttack', 'filterDecay', 'filterSustain', 'filterRelease']],
      ['Filter · LFO',
          ['filterType', 'cutoff', 'resonance', 'filterEnvAmount', inst.lfoSync ? 'lfoBeats' : 'lfoRate', 'lfoDepth', 'lfoTarget', 'lfoShape']],
      ['LFO 2', [inst.lfo2Sync ? 'lfo2Beats' : 'lfo2Rate', 'lfo2Depth', 'lfo2Target', 'lfo2Shape', 'lfoSync', 'lfo2Sync', null, null]],
      [mode === 'wavetable' ? 'Wavetable' : mode === 'granular' ? 'Granular' : 'Play mode', play]];
    // Granular keeps its region and play mode on a page before the Granular page.
    if (mode === 'granular') groups.splice(groups.length - 1, 0, ['Play mode', ['mode', 'rootNote', 'start', 'end', null, null, null, null]]);
    // In Wavetable and Granular mode the last page holds the mode's controls, with Position on its first knob.
    return groups;
  }
  /* CHANNEL: display buttons pick the level (Master · Group · Track · Instr.; Group only while the cursor track is
     in a track group), ◄ ► step pages (page dots in the title). Instrument keeps the built-in instrument pages;
     PLUG-IN · Instr. shows the same pages and is their home. Track: levels and sends. Group: the group bus.
     Master: master volume. */
  function channelPage() {
    const level = mk.currentLevel();
    if (level !== 'instrument') return levelsPage(level);
    const inst = P().instruments[st().instrument], track = st().track, t = P().tracks[track];
    if (!inst) return mk.genericPage('channel', 'Channel', []);
    const isSample = inst.type === 'sample';
    const instButtons = [button('‹ Inst', () => mk.stepInstrument(-1), {
      shift: button('Add inst', () => perform('addInstrument'))
    }), button('Inst ›', () => mk.stepInstrument(1), { shift: button('Assign', () => perform('assignInstrument')) })];
    if (inst.type === 'plugin' || inst.type === 'midi') {
      // Plug-in / hardware instruments: track channel controls here, plug-in pages on PLUG-IN.
      return { id: 'channel', title: 'Channel', main: mk.listMainInstrument(),
        buttons: mk.withLevels([button('Editor', () => S().request('plugin.editor', {
          owner: `track:${t.routingID}`,
          kind: 'instrument',
          slot: 0
        }).catch(error => S().toast(error.message, 'error'))), button('Plug-in',
            () => mk.setScreen('plugin')), button('Audition', () => mk.audition()), ...instButtons], 2),
        knobs: mk.trackKnobs(track), data: { kind: 'channel', inst: { name: inst.name, type: inst.type, index: st().instrument }, track: { name: t?.name, color: trackColor(track) }, group: inst.type === 'plugin'
            ? 'Plug-in instrument · knobs set the track channel'
            : 'External MIDI · knobs set the track channel', pages: [], instruments: mk.instrumentList() } };
    }
    const groups = instrumentGroups(inst);
    const page = pageIndex('channel', groups.length), [name, keys] = groups[page];
    return { id: 'channel', title: 'Channel', pages: groups.map(g => g[0]), page, main: mk.listMainInstrument(), setPage: i => { M.page.channel = clamp(i, 0, groups.length - 1); },
      buttons: mk.withLevels([button('Audition', () => mk.audition(), {
        shift: button('Sample edit', () => mk.setScreen('sampling'))
      }), button('Plug-in', () => mk.setScreen('plugin')), button('Mixer', () => mk.setScreen('mixer')), ...instButtons], 2),
      knobs: keys.map(key => key ? instKnob(key) : null),
      data: { kind: 'channel', inst: { name: inst.name, type: inst.type, index: st().instrument, mode: inst.mode, wavetablePosition: inst.wavetablePosition, attack: inst.attack, decay: inst.decay, sustain: inst.sustain, release: inst.release, cutoff: inst.cutoff, resonance: inst.resonance, filterType: inst.filterType }, track: { name: t?.name, color: trackColor(track) }, group: name, pages: groups.map(g => g[0]), page, assigned: t?.instrument === st().instrument, sample: isSample
          ? (st().samples?.[st().instrument]?.peaks || null)
          : null, instruments: mk.instrumentList() } };
  }
  /** CHANNEL · Track / Group / Master levels: the selected track's channel, its group bus or the master output. */
  function levelsPage(level) {
    const p = P(), track = st().track, t = p.tracks[track];
    if (level === 'group') return groupLevelsPage();
    const flag = property => enumKnob({
      key: `mix:${track}:${property}`,
      label: property === 'mute' ? 'Mute' : 'Solo',
      name: `${t?.name} · ${property}`,
      read: q => !!q.tracks[track]?.[property],
      write: (q, v) => {
        if (q.tracks[track]) q.tracks[track][property] = v;
      },
      options: [[false, 'Off'], [true, 'On']]
    });
    if (level === 'master') {
      return { id: 'channel', title: 'Channel', pages: ['Master'], page: 0, main: { turn(delta) { mk.masterKnob()?.turn(delta / 40); }, push() {}, tilt() {} },
        buttons: mk.withLevels([button('Mixer', () => mk.setScreen('mixer')), button('Plug-in', () => mk.setScreen('plugin')),
            button('Sends FX', () => perform('sendEffects')), null, null]),
        knobs: [mk.masterKnob(), mk.swingKnob(), mk.tempoKnob(), null, null, null, null, null],
        data: { kind: 'levels', level, levels: mk.levelList(), title: 'Master', subtitle: 'Master output', color: C_MASTER, lines: [`Volume ${fmt.db(burstValue('master', q => Number(q.masterVolume)))}`,
            `Swing ${fmt.pct(Number(p.swing || 0))} · ${Number(p.bpm).toFixed(1)} BPM`, 'Master inserts: PLUG-IN · Master'] } };
    }
    return { id: 'channel', title: 'Channel', pages: ['Levels'], page: 0, main: { turn(delta) { mk.mixKnob(track, 'volume')?.turn(delta / 40); }, push() {}, tilt(direction) { if (direction === 'left'
        || direction === 'right') selectTrack(track + (direction === 'right' ? 1 : -1)); } },
      buttons: mk.withLevels([button('Mixer', () => mk.setScreen('mixer')), button('Routing', () => perform('routing')), button('Sends FX',
          () => perform('sendEffects')), button('Freeze', () => perform('freezeTrack'), {
        shift: button('Unfreeze', () => perform('unfreezeTrack'))
      }), button('Plug-in', () => mk.setScreen('plugin'))], 2),
      knobs: [...mk.trackKnobs(track).slice(0, 4), flag('mute'), flag('solo'), ...voiceKnobs(track)],
      data: { kind: 'levels', level, levels: mk.levelList(), title: `${String(track + 1).padStart(2, '0')} ${t?.name || ''}`, subtitle: 'Track channel', color: trackColor(track), lines: [`Volume ${fmt.db(Number(t?.volume ?? .8))} · Pan ${fmt.pan(Number(t?.pan || 0))}`,
          `Delay ${fmt.pct(Number(t?.delaySend || 0))} · Reverb ${fmt.pct(Number(t?.reverbSend || 0))}`,
          t?.mute ? 'Muted' : t?.solo ? 'Soloed' : window.STFreeze?.isFrozen?.(track) ? 'Frozen track' : 'Live track'] } };
  }
  /* Voice mode and choke group of a track playing a sample instrument (Instrument → Voice behavior): kit pads
     choke each other (open / closed hat) from the controller. Other sources have neither. */
  function voiceKnobs(track) {
    const p = P(), t = p.tracks[track], inst = p.instruments[t?.instrument ?? -1];
    if (!t || inst?.type !== 'sample') return [null, null];
    const instOf = q => q.instruments[q.tracks[track]?.instrument ?? -1];
    return [enumKnob({ key: `mix:${track}:voiceMode`, label: 'Voice', name: `${t.name} · voice mode`,
      read: q => q.tracks[track]?.voiceMode || instOf(q)?.voiceMode || 'poly',
      write: (q, v) => { if (q.tracks[track]) q.tracks[track].voiceMode = v; },
      options: [['poly', 'Poly'], ['mono', 'Mono'], ['legato', 'Legato']] }),
    enumKnob({ key: `mix:${track}:chokeGroup`, label: 'Choke', name: `${t.name} · choke group`,
      read: q => Number(q.tracks[track]?.chokeGroup ?? instOf(q)?.chokeGroup ?? 0),
      write: (q, v) => {
        const target = q.tracks[track]; if (!target) return;
        if (v) target.chokeGroup = v; else delete target.chokeGroup;
        // As the instrument page does: the track's value replaces the instrument-wide choke group.
        if (instOf(q)) delete instOf(q).chokeGroup;
      },
      options: Array.from({ length: 17 }, (_, i) => [i, i ? `Group ${i}` : 'Off']) })];
  }
  const C_MASTER = '#cfad67';
  /* CHANNEL · Group: the cursor group's bus (volume, pan, mute, solo). Tilt ◄ ► walks the group's members. */
  function groupLevelsPage() {
    const p = P(), group = groupOf(), owner = G().busOwner(group), members = membersOf(group), bus = busOf(group) || {};
    const flag = property => enumKnob({
      key: `ownerMix:${encodeURIComponent(owner)}:${property}`,
      label: property === 'mute' ? 'Mute' : 'Solo',
      name: `${group.name} · ${property}`,
      read: q => !!channelOf(owner, q)?.[property],
      write: (q, v) => { const c = channelOf(owner, q); if (c) c[property] = v; },
      options: [[false, 'Off'], [true, 'On']]
    });
    const at = members.indexOf(st().track);
    return { id: 'channel', title: 'Channel', pages: ['Group'], page: 0,
      main: { turn(delta) { ownerMixKnob(owner, 'volume', group.name)?.turn(delta / 40); }, push() {}, tilt(direction) {
        if (direction === 'left' || direction === 'right') setCursor({ track: members[clamp(at + (direction === 'right' ? 1 : -1), 0, members.length - 1)] });
      } },
      buttons: mk.withLevels([button('Mixer', () => mk.setScreen('mixer')), button('Plug-in', () => mk.setScreen('plugin')),
          button(group.collapsed ? 'Expand' : 'Collapse', () => app()?.setCollapsed?.(group.id, !group.collapsed)),
          button('Routing', () => perform('routing')), null]),
      knobs: [ownerMixKnob(owner, 'volume'), ownerMixKnob(owner, 'pan'), null, null, flag('mute'), flag('solo'), null, null],
      data: { kind: 'levels', level: 'group', levels: mk.levelList(), title: group.name, subtitle: `Group bus · ${members.length} track${members.length === 1 ? '' : 's'}`,
        color: group.color, lines: [`Volume ${fmt.db(burstValue(`ownerMix:${encodeURIComponent(owner)}:volume`, q => Number(channelOf(owner, q)?.volume ?? 1)))} · Pan ${fmt.pan(Number(bus.pan || 0))}`,
          `Members: ${members.map(i => p.tracks[i]?.name).join(', ')}`.slice(0, 120),
          bus.mute ? 'Group muted' : bus.solo ? 'Group soloed' : 'Group bus → Master'] } };
  }

  Object.assign(mk, {
    OVERVIEW_MS, overviewVisible, notesPage, writeNote, selectPattern, macroPage, autoLanesPage, instrumentGroups,
    channelPage
  });
})();
