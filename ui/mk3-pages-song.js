/* MK3: arranger, patterns, mixer, browser, sampling and settings pages. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, VEL_CURVES, AFTERTOUCH, prefs, savePrefs, M, active, trackColor, playing, flash, scheduleRefresh,
    selectTrack, perform, patternIndex, G, entries, entryColor, entryName, entryFlags, toggleEntry, memberFor,
    rememberMember, busOf, ownerMixKnob, burstValue, commitBursts, numKnob, intKnob, enumKnob, actionKnob, button, fmt, instKnob, LIST_ROWS, listView,
    listMain, listData, selectPattern, listMainInstrument, stepInstrument, mixKnob, masterKnob
  } = mk;

  function arrangerPage() {
    const p = P(), M_ = window.STSongModel, Song = window.STSong;
    const blocks = M_ ? M_.blocks(p.sequence) : [], current = M_ ? M_.blockAt(p.sequence, st().sequence || 0) : 0, block = blocks[current];
    const loop = M_?.loopRange(p),
      playingBlock = playing() && st().status?.songMode && Number.isInteger(st().status.sequence) ? M_.blockAt(p.sequence, st().status.sequence) : -1;
    const selectBlock = index => {
      const next = blocks[clamp(index, 0, blocks.length - 1)];
      if (next) {
        st().sequence = next.start;
        scheduleRefresh();
      }
    };
    const act = name => () => Song?.actions?.[name]?.();
    return { id: 'arranger', title: 'Arranger',
      main: listMain({
        count: blocks.length,
        move: delta => selectBlock(current + delta),
        choose: () => Song?.actions?.editPattern?.(),
        column: direction => Song?.actions?.changePattern?.(direction)
      }),
      buttons: [button('Insert', act('insertNext'), { shift: button('New', act('insertNew')) }), button('Duplicate', act('duplicate'), {
        shift: button('Clone', act('clone'))
      }), button('Remove', act('remove')), button('Loop', act('toggleLoop'), {
        on: !!loop,
        shift: button('Clear loop', act('clearLoop'))
      }), button('Play here', act('playFrom')), button('Follow', act('toggleFollow'), { on: !!Song?.follow }), button('Rename', act('rename'),
          { shift: button('Colour', () => Song?.actions?.color?.(1)) }), button('Edit', act('editPattern'))],
      knobs: [actionKnob('Block', `${current + 1} / ${blocks.length}`, direction => selectBlock(current + direction),
          { detent: .05, options: blocks.map((item, i) => `${i + 1} · P${String(M_.patternNumber(p, item.pattern)).padStart(2, '0')} ${p.patterns.find(x => x.id === item.pattern)?.name || ''}`), index: current }),
        actionKnob('Pattern', block ? `P${String(M_.patternNumber(p, block.pattern)).padStart(2, '0')}` : '–',
            direction => Song?.actions?.changePattern?.(direction), { detent: .07 }),
        actionKnob('Repeats', block ? `×${block.count}` : '–', direction => Song?.actions?.repeats?.(direction), { detent: .07 }),
        actionKnob('Move', 'block', direction => Song?.actions?.move?.(direction), { detent: .12 }),
        actionKnob('Colour', '', direction => Song?.actions?.color?.(direction), { detent: .1 }),
        enumKnob({ key: 'loopSong', label: 'Loop song', read: q => !!q.loopSong, write: (q, v) => { q.loopSong = v; }, options: [[false, 'Off'], [true, 'On']] }),
        swingKnob(), tempoKnob()],
      data: { kind: 'arranger', blocks: blocks.map((item, index) => { const pat = p.patterns.find(x => x.id === item.pattern); return { index, number: M_.patternNumber(p, item.pattern), name: pat?.name || 'Missing', count: item.count, color: M_.patternColor(p, item.pattern), loop: !!loop
          && item.start >= loop.start
          && item.start <= loop.end, playing: index === playingBlock, progress: index === playingBlock
          ? `${st().status.sequence - item.start + 1}/${item.count}`
          : '', length: pat?.length || 0 }; }), selected: current, ...listView('arranger', blocks.length,
              current), loop: !!loop, loopSong: !!p.loopSong, mode: st().playbackMode, slots: p.sequence.length } };
  }
  /* Pattern list (Shift + ARRANGER) and, with `padMode`, the PATTERN page shown while PATTERN pad mode is pinned:
     pattern Length and Lines/beat live here (docs/MK3_DESIGN_STUDY.md §5.9). */
  function patternsPage(padMode = false) {
    const p = P(), index = clamp(M.lists.patterns ?? patternIndex(), 0, p.patterns.length - 1), pat = p.patterns[index];
    M.lists.patterns = index;
    const select = i => { M.lists.patterns = clamp(i, 0, p.patterns.length - 1); };
    const open = () => { selectPattern(M.lists.patterns); };
    return { id: padMode ? 'pattern' : 'patterns', title: padMode ? 'Pattern' : 'Patterns',
      main: listMain({ count: p.patterns.length, move: delta => select(index + delta), choose: open }),
      buttons: [button('Select', open), button('Add', () => perform('addPattern')), button('Duplicate', () => {
        selectPattern(index);
        perform('duplicatePattern');
      }), button('Rename', () => {
        selectPattern(index);
        perform('renamePattern');
      }), button('Double', () => {
        selectPattern(index);
        perform('doublePattern');
      }), button('Halve', () => {
        selectPattern(index);
        perform('halvePattern');
      }), button('To song', () => window.STSong?.actions?.insertPattern?.(pat.id)), button('Remove', () => {
        selectPattern(index);
        perform('removePattern');
      })],
      knobs: [actionKnob('Pattern', `${String(index + 1).padStart(2, '0')}`, direction => select(index + direction), { detent: .05, list: true }),
        intKnob({ key: `length:${pat.id}`, label: 'Length', read: q => q.patterns.find(item => item.id === pat.id)?.length
            || 64, write: (q, v) => { const target = q.patterns.find(item => item.id === pat.id); if (!target) return; target.length = v; target.steps.forEach(steps => { steps.length = v; for (let r = 0; r < v; r++) steps[r] = steps[r] || null; }); target.automation.forEach(lane => { lane.points = lane.points.filter(point => point.row < v); }); }, min: 1, max: 256, fmt: v => `${v} rows`, detent: .02 }),
        enumKnob({ key: 'lpb', label: 'Lines/beat', read: q => q.linesPerBeat, write: (q, v) => { q.linesPerBeat = v; }, options: [1, 2, 3, 4, 6, 8, 12, 16].map(v => [v, String(v)]) }),
        actionKnob('Colour', '', direction => S().mutate('pattern colour', q => window.STSongModel?.cycleColor(q, pat.id, direction)),
            { detent: .1 }),
        null, null, swingKnob(), tempoKnob()],
      data: listData('patterns', p.patterns.map((item, i) => ({ name: `${String(i + 1).padStart(2, '0')} ${item.name}`, right: `${item.length} rows`, detail: `${item.length} rows · ${item.automation.length} automation lanes`, on: item.id === p.activePattern, color: window.STSongModel?.patternColor(p,
          item.id) })), index,
        { title: 'Patterns', hint: padMode ? 'Pads or push: select · PATTERN: back' : 'Turn: pattern · push: open · ARRANGER: song' }) };
  }
  function swingKnob() {
    return numKnob({
      key: 'swing',
      label: 'Swing',
      read: p => Number(p.swing || 0),
      write: (p, v) => {
        p.swing = v;
      },
      min: 0,
      max: .75,
      fmt: fmt.pct,
      step: .01
    });
  }
  function tempoKnob(fine = false) {
    return numKnob({ key: 'tempo', label: fine ? 'Fine' : 'Tempo', name: 'Tempo (BPM)', read: p => Number(p.bpm ?? 120),
      write: (p, v) => { p.bpm = v; }, min: 20, max: 400, fmt: fmt.bpm, speed: fine ? .1 : 1, step: .1,
      commit: (v, options) => S().mutate('tempo', p => { p.bpm = v; }, { ...options, render: false }) });
  }

  /* MIXER pages (◄ ►): Volume, then Pan / Delay / Reverb (the Shift + MIXER screen, which opens on Pan). */
  const MIXER_PAGES = ['Volume', 'Pan', 'Delay', 'Reverb'];
  function setMixerPage(index) {
    const page = clamp(index, 0, MIXER_PAGES.length - 1);
    if (!commitBursts()) { flash('Pending controller edit could not finish · retry the gesture'); return; }
    if (page === 0) M.screen = 'mixer'; else { M.screen = 'mixerSends'; M.sendPage = page - 1; }
    mk.schedule();
  }
  function mixerPage(sends = false) {
    /* Strips = the group-button bank's eight entries: an ungrouped track's channel, or a track group's bus
       (volume and pan; a group bus has no built-in sends). */
    const p = P(), first = M.bank * 8, kinds = ['pan', 'delaySend', 'reverbSend'], kind = sends ? kinds[clamp(M.sendPage, 0, 2)] : 'volume';
    const bank = entries().slice(first, first + 8);
    const strip = (entry, slot) => {
      if (entry.group) {
        const owner = G().busOwner(entry.group), key = field => `ownerMix:${encodeURIComponent(owner)}:${field}`;
        return { index: first + slot, track: entry.index, group: entry.group.id, name: entry.group.name, color: entryColor(entry),
          volume: burstValue(key('volume'), q => Number(busOf(entry.group, q)?.volume ?? 1)), pan: burstValue(key('pan'), q => Number(busOf(entry.group, q)?.pan ?? 0)),
          delay: 0, reverb: 0, value: kind === 'volume' ? null : kind === 'pan' ? burstValue(key('pan'), q => Number(busOf(entry.group, q)?.pan ?? 0)) : 0,
          ...entryFlags(entry, p), selected: entry.members.includes(st().track), peak: Number(st().status?.ownerPeaks?.[owner] || 0) };
      }
      const i = entry.index, t = p.tracks[i];
      return {
        index: first + slot,
        track: i,
        name: t.name,
        color: trackColor(i),
        volume: burstValue(`mix:${i}:volume`, q => Number(q.tracks[i]?.volume ?? .8)),
        pan: burstValue(`mix:${i}:pan`, q => Number(q.tracks[i]?.pan ?? 0)),
        delay: Number(t.delaySend || 0),
        reverb: Number(t.reverbSend || 0),
        value: kind === 'volume' ? null : burstValue(`mix:${i}:${kind}`, q => Number(q.tracks[i]?.[kind] || 0)),
        mute: !!t.mute,
        solo: !!t.solo,
        selected: i === st().track,
        peak: Number(st().status?.trackPeaks?.[i] || 0)
      };
    };
    const toggle = (entry, property) => () => toggleEntry(entry, property);
    const choose = entry => { const target = entry.group ? memberFor(entry.group) : entry.index; selectTrack(target, { silent: true }); rememberMember(target); };
    const knob = (entry, slot) => {
      if (!entry) return null;
      const label = entryName(entry, p).slice(0, 14);
      if (!entry.group) return mixKnob(entry.index, kind, label);
      return ['volume', 'pan'].includes(kind) ? ownerMixKnob(G().busOwner(entry.group), kind, label) : null;
    };
    return { id: sends ? 'mixerSends' : 'mixer', title: sends
        ? `Mixer · ${['Pan', 'Delay send', 'Reverb send'][clamp(M.sendPage, 0, 2)]}`
        : 'Mixer', pages: MIXER_PAGES, page: sends ? clamp(M.sendPage, 0, 2) + 1 : 0, setPage: setMixerPage,
      main: {
        turn(delta) {
          const knob = M.shift ? masterKnob() : mixKnob(st().track, 'volume');
          knob?.turn(delta / 40);
        },
        push(down) {
          if (down) mk.setScreen(sends ? 'mixer' : 'mixerSends');
        },
        tilt(direction) {
          if (direction === 'left' || direction === 'right') {
            selectTrack(st().track + (direction === 'right' ? 1 : -1));
          } else if (sends) M.sendPage = clamp(M.sendPage + (direction === 'down' ? 1 : -1), 0, 2);
        }
      },
      // Display buttons select the track (bright = selected); Shift or MUTE held: mute; SOLO held: solo.
      buttons: Array.from({ length: 8 }, (_, i) => { const entry = bank[i]; if (!entry) return null; const flags = entryFlags(entry, p), name = entryName(entry, p).slice(0, 16);
        const style = { on: entry.members.includes(st().track), color: entryColor(entry), mute: flags.mute, solo: flags.solo, select: true };
        return button(name,
            () => { if (active('mute')) toggle(entry, 'mute')(); else if (active('solo')) toggle(entry, 'solo')(); else choose(entry); },
            { ...style, shift: button(name, toggle(entry, 'mute'), style) }); }),
      knobs: Array.from({ length: 8 }, (_, i) => knob(bank[i], i)),
      data: { kind: 'mixer', param: kind, strips: bank.map(strip), master: burstValue('master',
          q => Number(q.masterVolume ?? .7)), bank: M.bank } };
  }

  /* Browser columns: every list filter (Source, Expansion, Product, Bank, Type, …) and the results. The left
     screen shows the focused (or touched) filter as a list, the right screen the results; tilt ◄ ► moves between
     columns, ◄ / ► (arrow buttons) and display buttons 1–4 switch tabs. A filter follows its cursor after a
     short pause (push applies it at once and returns to the results). */
  const plural = label => /y$/.test(label) ? label.slice(0, -1) + 'ies' : label + 's';
  function browserFilters(tab, B, N, K) {
    const make = (ctl, id, label, value) => ({
      id,
      label,
      value,
      ctl,
      list: typeof ctl?.options === 'function' && typeof ctl?.choose === 'function' && typeof ctl?.current === 'function'
    });
    if (tab === 'kits' && K) return [['sources', 'Source'], ['expansions', 'Expansion'],
        ['tags', 'Type']].map(([id, label]) => make(K, id, label, K.filterValue(id)));
    if (tab === 'nks' && N) return [['product', 'Product'], ['bank', 'Bank'], ['types', 'Type'], ['subtypes', 'Sub-type'],
        ['characters', 'Character']].map(([id, label]) => make(N, id, label, N.value(id)));
    if (tab === 'samples' && B) return (B.fields || []).map(id => make(B, id, B.filterLabel(id), B.filterValue(id)));
    return [];
  }
  function filterOptions(tab, filter) {
    let options = []; try { options = filter.ctl.options(filter.id) || []; } catch { options = []; }
    return [{ value: null, name: `All ${plural(filter.label).toLowerCase()}`, right: '' },
        ...options.filter(option => option
            && option.value != null
            && option.value !== '').map(option => ({ value: option.value, name: String(option.label ?? option.value), right: option.count != null
                && option.count !== ''
            ? Number(option.count).toLocaleString('en-US')
            : option.detail || '', detail: option.detail || '' }))];
  }
  function browserPage() {
    const B = window.STBrowser?.controller, tab = B?.tab || 'samples', N = window.STNKSBrowser?.controller, K = window.STKitBrowser?.controller;
    const ctl = tab === 'kits' ? K : tab === 'nks' ? N : B;
    const list = ctl?.list(), b = M.browser;
    const move = delta => ctl?.move(delta);
    const load = asNew => tab === 'kits' ? K?.load(asNew ? 'open' : 'stay') : tab === 'nks' ? N?.load(asNew ? 'new' : 'stay') : B?.load(asNew);
    const filters = browserFilters(tab, B, N, K), listFilters = filters.filter(f => f.list);
    const columns = [...listFilters.map(f => f.id), 'results'];
    if (b.tab !== tab) { b.tab = tab; b.focus = 'results'; }
    if (!columns.includes(b.focus)) b.focus = 'results';
    const keyOf = id => `${tab}:${id}`;
    const filterById = id => listFilters.find(f => f.id === id);
    const cursorValue = filter => Object.hasOwn(b.cursor, keyOf(filter.id)) ? b.cursor[keyOf(filter.id)] : (filter.ctl.current(filter.id) ?? null);
    const apply = (filter, value) => {
      clearTimeout(b.pending?.timer);
      b.pending = null;
      if ((filter.ctl.current(filter.id) ?? null) !== value) filter.ctl.choose(filter.id, value);
      mk.schedule();
    };
    const focus = id => {
      if (id !== 'results') {
        const filter = filterById(id);
        if (!filter) return;
        b.last[tab] = id;
        if (b.focus !== id) b.cursor[keyOf(id)] = filter.ctl.current(id) ?? null;
      }
      b.focus = id;
    };
    const moveFilter = (id, delta) => {
      const filter = filterById(id); if (!filter) return;
      focus(id);
      const options = filterOptions(tab, filter),
        index = Math.max(0, options.findIndex(o => o.value === cursorValue(filter))),
        next = options[clamp(index + delta, 0, options.length - 1)];
      b.cursor[keyOf(id)] = next.value;
      clearTimeout(b.pending?.timer);
      b.pending = { key: keyOf(id), timer: setTimeout(() => { if (b.pending?.key === keyOf(id)) apply(filter, next.value); }, 260) };
    };
    const column = direction => {
      if (b.focus === 'results' && direction < 0) { focus(b.last[tab] && filterById(b.last[tab])
          ? b.last[tab]
          : columns[Math.max(0, columns.length - 2)] || 'results'); return; }
      const index = columns.indexOf(b.focus); focus(columns[clamp(index + direction, 0, columns.length - 1)]);
    };
    const knobs = [];
    if (tab === 'nks' && N) knobs.push(actionKnob('Kind', N.kind === 'effect' ? 'Effects' : 'Instruments', () => N.toggleKind(), { detent: .1 }));
    else if (tab === 'plugins' && B) knobs.push(actionKnob('Kind', B.kind === 'effect' ? 'Effects' : 'Instruments',
        direction => B.cycleKind(direction), { detent: .1 }),
        actionKnob('Format', B.format === 'all' ? 'All' : B.format, direction => B.cycleFormat(direction), { detent: .1 }));
    else if (tab !== 'kits' && B) knobs.push(actionKnob('Kind', { all: 'All', sample: 'Samples', nks: 'NKS' }[B.kind] || B.kind,
        direction => B.cycleKind(direction), { detent: .1 }));
    // Kits keep Source / Expansion / Type on encoders 1–3 and Content on 4, as on the Kit browser panel.
    for (const filter of filters) {
      if (!filter.list) { knobs.push(actionKnob(filter.label, filter.value,
          direction => (tab === 'nks' || tab === 'kits' ? filter.ctl.cycle(filter.id, direction) : B.cycleFilter(filter.id, direction)),
          { detent: .08 })); continue; }
      const pending = Object.hasOwn(b.cursor, keyOf(filter.id)) && b.focus === filter.id
          ? filterOptions(tab, filter).find(o => o.value === cursorValue(filter))
          : null;
      knobs.push(actionKnob(filter.label, pending
          ? pending.value == null ? 'All' : pending.name
          : filter.value, direction => moveFilter(filter.id, direction), {
        detent: .08,
        list: true,
        filterId: filter.id
      }));
    }
    if (tab === 'kits' && K) knobs.push(actionKnob('Content', list?.userOnly ? 'User' : 'All', () => K.toggleUserOnly(), { detent: .1 }));
    while (knobs.length < 7) knobs.push(null);
    knobs.length = 7;
    const results = list || { items: [], selected: 0, total: 0, offset: 0 };
    knobs.push(actionKnob('Results', list ? `${list.items.length ? list.offset + list.selected + 1 : 0} / ${list.total}` : '–', direction => {
      b.focus = 'results';
      move(direction);
    }, { detent: .02, list: true }));
    const tabs = ['samples', 'nks', 'plugins', ...(K ? ['kits'] : [])];
    const tabButtons = [button('Samples', () => B?.setTab('samples'), { on: tab === 'samples' }),
        button('NKS', () => B?.setTab('nks'), { on: tab === 'nks' }), button('Plug-ins', () => B?.setTab('plugins'), { on: tab === 'plugins' })];
    if (K) tabButtons.push(button('Kits', () => B?.setTab('kits'), {
      on: tab === 'kits',
      shift: button('User kits', () => {
        B?.setTab('kits');
        K.toggleUserOnly();
      }, { on: tab === 'kits' && !!list?.userOnly })
    }));
    // The touched filter encoder previews its list on the left screen; otherwise the focused filter.
    const touchedIndex = [...M.touched].filter(name => /^encoder[1-8]$/.test(name)).map(name => Number(name.slice(7)) - 1).pop();
    const shownId = knobs[touchedIndex]?.filterId || (b.focus !== 'results' ? b.focus : null), shown = shownId && filterById(shownId);
    let filterList = null;
    if (shown) {
      const options = filterOptions(tab, shown), applied = shown.ctl.current(shown.id) ?? null;
      const index = Math.max(0, options.findIndex(o => o.value === (shown.id === b.focus ? cursorValue(shown) : applied)));
      filterList = listData(`filter:${keyOf(shown.id)}`, options.map(o => ({ ...o, on: o.value === applied })), index,
          { title: shown.label, active: b.focus === shown.id, empty: 'No choices' });
    }
    const focused = b.focus !== 'results' && filterById(b.focus);
    const filterCount = focused ? filterOptions(tab, focused).length : 0;
    const resultItems = (results.items || []).map(item => ({
      name: `${item.favorite ? '★ ' : ''}${item.name}`,
      right: item.detail ? String(item.detail).split(' · ')[0] : '',
      dim: !!item.offline,
      detail: [item.detail, item.bpm ? `${item.bpm} BPM` : '', item.key || ''].filter(Boolean).join(' · ')
    }));
    const title = {
      samples: 'Samples',
      nks: `NKS ${results.kind === 'effect' ? 'effects' : 'instruments'}`,
      plugins: `Plug-ins · ${B?.kind === 'effect' ? 'effects' : 'instruments'}`,
      kits: 'Kits'
    }[tab] || tab;
    return { id: 'browser', title: 'Browser', setTab: direction => B?.setTab(tabs[(tabs.indexOf(tab) + direction + tabs.length) % tabs.length]),
      main: focused ? { list: { count: filterCount, rows: LIST_ROWS }, turn(delta) { moveFilter(focused.id, delta); }, push(down) { if (down) { apply(focused, cursorValue(focused)); b.focus = 'results'; } }, tilt(direction) { if (direction === 'up'
          || direction === 'down') moveFilter(focused.id,
          (direction === 'down' ? 1 : -1) * LIST_ROWS); else column(direction === 'right' ? 1 : -1); } }
        : { list: { count: Number(results.total)
            || results.items?.length
            || 0, rows: LIST_ROWS - 1 }, turn(delta) { move(delta); }, push(down) { if (down) load(M.shift); }, tilt(direction) { if (direction === 'up'
                || direction === 'down') { if (ctl?.page && tab !== 'nks') ctl.page(direction === 'down' ? 1 : -1); else move((direction === 'down' ? 1 : -1) * (LIST_ROWS - 1)); } else column(direction === 'right' ? 1 : -1); } },
      buttons: [...tabButtons,
        button('Favorites', () => ctl?.toggleFavorites?.(),
            { on: !!list?.favorites, shift: button('Star', () => ctl?.favorite?.()) }), button('Auto prev',
            () => ctl?.toggleAutoPreview?.(),
            { on: !!list?.autoPreview, shift: button('Clear filt', () => tab === 'kits' ? K?.clear() : ctl?.clearFilters?.()) }),
        button(list?.preview ? 'Stop' : 'Preview', () => ctl?.preview?.(),
            { on: !!list?.preview }), ...(!K ? [button('Load new', () => load(true))] : []), button('Load',
            () => load(false), { shift: button(tab === 'kits' ? 'Load + edit' : 'Load new', () => load(true)) })],
      knobs, data: { kind: 'browser', tab, focus: b.focus, list: results, filterList,
        results: listData(`browser:${tab}`, resultItems, results.selected || 0, {
          rows: LIST_ROWS - 1,
          title,
          active: b.focus === 'results',
          total: Number(results.total) || 0,
          offset: Number(results.offset) || 0,
          busy: !!results.busy,
          empty: results.busy ? 'Searching…' : 'No results'
        }),
        overview: listData(`browserFilters:${tab}`, knobs.slice(0, 7).map(knob => knob && {
          name: knob.label,
          right: knob.value,
          filterId: knob.filterId
        }).filter(Boolean), Math.max(0,
            knobs.slice(0, 7).filter(Boolean).findIndex(knob => knob.filterId
                && knob.filterId === (filterById(b.last[tab]) ? b.last[tab] : listFilters[listFilters.length - 1]?.id))), {
          title: `Filters${results.filters ? ` · ${results.filters} active` : ''}${results.favorites ? ' · ★' : ''}`,
          active: false
        }),
        filters: knobs.slice(0, 7).filter(Boolean).map(knob => ({ label: knob.label, value: knob.value })) } };
  }

  function samplingPage() {
    const C = window.STSampler?.controller, info = C?.info(960);
    if (!info) return { id: 'sampling', title: 'Sampling', main: listMainInstrument(), buttons: [button('Record', () => perform('recordInput')),
        button('Load', () => perform('loadSample')), button('Browse', () => mk.setScreen('browser')),
        button('Sampler', () => perform('instrumentType:sample')), null, null, button('‹ Inst', () => stepInstrument(-1)),
        button('Inst ›',
            () => stepInstrument(1))], knobs: Array(8).fill(null), data: { kind: 'info', title: 'No sampler instrument selected', lines: ['Choose a sampler instrument (‹ Inst / Inst ›), load a sample from BROWSER,',
            'or press Shift + SAMPLING to record audio input.'] } };
    const command = name => () => window.STSampler.command(name);
    const marker = info.markers.findIndex(item => item.key === info.marker);
    return { id: 'sampling', title: 'Sampling',
      main: {
        turn(delta) {
          C.nudge(Math.sign(delta), M.shift ? 'frame' : 'fine');
        },
        push(down) {
          C.audition(down);
        },
        tilt(direction) {
          if (direction === 'up' || direction === 'down') {
            C.selectMarker(direction === 'down' ? 1 : -1);
          } else {
            C.nudge(direction === 'right' ? 1 : -1, 'coarse');
          }
        }
      },
      buttons: [button('Trim', command('trim'), { shift: button('Gain…', command('gain')) }), button('Normalize', command('normalize'),
          { shift: button('Remove DC', command('removeDC')) }), button('Reverse', command('reverse'),
              { shift: button('Silence', command('silence')) }), button('Fade in', command('fadeIn'), {
        shift: button(info.snap ? 'Snap on' : 'Snap off', () => C.toggleSnap(), { on: info.snap })
      }), button('Fade out', command('fadeOut'), { shift: button('Snap now', () => C.snapNow()) }), button('Add slice', command('addSlice'), {
        shift: button('Del slice', () => C.removeSlice())
      }), button('Detect', command('detectTransients'), {
        shift: button('Zoom region', () => C.showRegion())
      }), button('→ Pattern', command('slicesToPattern'), { shift: button('Zoom all', () => C.showAll()) })],
      knobs: [actionKnob('Marker', info.markerLabel, direction => C.selectMarker(direction), {
        detent: .07
      }), actionKnob('Position', info.time, direction => C.nudge(direction, 'coarse'), {
        detent: .03
      }), actionKnob('Fine', info.time, direction => C.nudge(direction, 'fine'), {
        detent: .01
      }), actionKnob('Zoom', `${Math.round(1 / Math.max(1e-6, info.view[1] - info.view[0]))}×`, direction => C.zoom(direction > 0 ? .8 : 1.25, {
        center: true
      }), { detent: .04 }), actionKnob('Scroll', `${Math.round(info.view[0] * 100)}%`, direction => C.scroll(direction * .05), {
        detent: .02
      }), actionKnob('Play mode', info.modeLabel, direction => C.cycleMode(direction), {
        detent: .1
      }), instKnob('rootNote'), info.mode === 'slice' ? instKnob('sliceCount') : instKnob('volume')],
      data: { kind: 'sample', info, selectedMarker: marker, recording: !!window.STRecording?.isRecording } };
  }

  function settingsPage() {
    const knob = (key, label, min, max, f, step = .05) => ({
      label,
      value: f(prefs[key]),
      norm: (prefs[key] - min) / (max - min),
      turn(delta) {
        prefs[key] = Number(clamp(prefs[key] + delta * (max - min), min, max).toFixed(3));
        savePrefs();
      },
      adjust(direction) {
        prefs[key] = Number(clamp(prefs[key] + direction * step * (max - min), min, max).toFixed(3));
        savePrefs();
      }
    });
    const curveIndex = VEL_CURVES.findIndex(([key]) => key === prefs.velocityCurve);
    const knobValue = name => M.knobs[name] == null ? 'n/a' : `${Math.round(M.knobs[name] * 100)}%`;
    const curve = direction => { prefs.velocityCurve = VEL_CURVES[clamp(Math.max(0, curveIndex) + direction, 0, 2)][0]; savePrefs(); };
    const pressure = direction => {
      const at = AFTERTOUCH.findIndex(([key]) => key === prefs.aftertouch);
      prefs.aftertouch = AFTERTOUCH[clamp((at < 0 ? 1 : at) + direction, 0, AFTERTOUCH.length - 1)][0];
      savePrefs();
      flash(`Pad pressure: ${prefs.aftertouch === 'off' ? 'off' : prefs.aftertouch + ' aftertouch'}`, 800);
    };
    const knobs = [knob('sensitivity', 'Pad sens', 0, 1, fmt.pct), actionKnob('Vel curve', VEL_CURVES[Math.max(0, curveIndex)][1], curve, {
      detent: .1,
      options: VEL_CURVES.map(c => c[1]),
      index: Math.max(0, curveIndex)
    }), knob('ledBrightness', 'LED bright', .1, 1,
        fmt.pct), knob('screenBrightness', 'Screen', .2, 1, fmt.pct), actionKnob('Master', knobValue('master'), () => {}), actionKnob('Phones',
        knobValue('headphone'), () => {}), actionKnob('Mic gain', knobValue('mic'),
            () => {}), actionKnob('Pressure', (AFTERTOUCH.find(([key]) => key === prefs.aftertouch) || AFTERTOUCH[1])[1]
        + (M.pedal ? ' · ped' : ''), pressure, {
      detent: .1,
      options: AFTERTOUCH.map(a => a[1]),
      index: Math.max(0, AFTERTOUCH.findIndex(([key]) => key === prefs.aftertouch))
    })];
    // The main encoder walks the editable settings; tilt ◄ ► (or push: next value) changes the selected one.
    const rows = [['Pad sensitivity', knobs[0], d => knobs[0].adjust(d)], ['Velocity curve', knobs[1], curve],
        ['LED brightness', knobs[2], d => knobs[2].adjust(d)], ['Screen brightness', knobs[3], d => knobs[3].adjust(d)],
        ['Pad pressure', knobs[7], pressure]];
    const index = clamp(M.lists.settings || 0, 0, rows.length - 1); M.lists.settings = index;
    return { id: 'settings', title: 'Controller settings',
      main: listMain({
        count: rows.length,
        move: delta => {
          M.lists.settings = clamp(index + delta, 0, rows.length - 1);
        },
        choose: () => {
          if (rows[index][1].options) {
            const options = rows[index][1].options.length;
            rows[index][2](rows[index][1].index >= options - 1 ? -(options - 1) : 1);
          } else {
            rows[index][2](1);
          }
        },
        column: direction => rows[index][2](direction)
      }),
      buttons: [button(M.exclusive ? 'Screens: SuperTracker' : 'Take over', () => mk.command('mk3Connect'), {
        on: M.exclusive
      }), button('Release', () => mk.command('mk3Release')), button('Reconnect', () => S().request('mk3.connect', {
        exclusive: M.exclusive
      }).catch(error => S().toast(error.message, 'error'))), button('Simulator',
          () => mk.command('mk3Simulator')), button('Help', () => perform('help')), button('App settings',
              () => perform('settings')), null, button('Back', () => mk.setScreen('notes'))],
      knobs,
      data: listData('settings', rows.map(([name, k]) => ({ name, right: k.value })), index, { title: 'Maschine MK3', rows: 5, hint: 'Turn: setting · tilt ◄ ►: change · SETTINGS: back', info: true,
        lines: [M.connected ? 'Controller connected over USB' : 'Controller not connected',
            M.screens
                ? (M.exclusive ? 'Screens: drawn by SuperTracker (NI agents paused)' : 'Screens: available')
                : 'Screens: owned by NI agents · Take over to draw them',
            M.error ? 'Driver: ' + M.error : 'Pads, buttons and LEDs work alongside NI software.',
            `Master ${knobValue('master')} · phones ${knobValue('headphone')} · mic ${knobValue('mic')} (audio interface)`,
            'Pedal acts as sustain for pads.'] }) };
  }

  function genericPage(id, title, lines) {
    return {
      id,
      title,
      main: { turn() {}, push() {}, tilt() {} },
      buttons: Array(8).fill(null),
      knobs: Array(8).fill(null),
      data: { kind: 'info', title, lines }
    };
  }

  /* Encoder modes (VOLUME, SWING, TEMPO, NOTE REPEAT, EVENTS and the Shift variants Velocity / Position / Tune):
     the current page stays on both screens; only the encoders (and the 4-D encoder, display buttons where the mode
     has its own) change, and a header strip above the encoder labels names the mode (docs/MK3_DESIGN_STUDY.md §5.5). */
  function encoderMode(id, title, header, { main, knobs = [], buttons, data = {} }) {
    const base = mk.baseScreenPage(), list = [...knobs]; while (list.length < 8) list.push(null);
    return { ...base, id, title, overlay: true, encoderMode: true, overlayOf: M.screen, main, buttons: buttons || base.buttons, knobs: list.slice(0, 8), data: { ...(base.data || {}), ...data, header } };
  }

  Object.assign(mk, {
    arrangerPage, patternsPage, swingKnob, tempoKnob, MIXER_PAGES, setMixerPage, mixerPage, browserPage, samplingPage,
    settingsPage, genericPage, encoderMode
  });
})();
