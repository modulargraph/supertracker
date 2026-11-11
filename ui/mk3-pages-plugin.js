/* MK3: instrument list, plug-in slots and chains, plug-in parameter pages. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, clamp01, M, active, trackColor, flash, scheduleRefresh, perform, AR, arRef, arRecording, G,
    groupOf, ownerMixKnob, commitBursts, numKnob, actionKnob, button, fmt, instKnob, lockSpec, lockTurn, pageIndex,
    LIST_ROWS, listMain, listData, instrumentGroups
  } = mk;

  // CHANNEL-style pages: the main encoder walks the instrument list (shown on the right screen of CHANNEL).
  function listMainInstrument() {
    return {
      list: { count: P().instruments.length, rows: LIST_ROWS },
      turn(delta) {
        stepInstrument(delta);
      },
      push(down) {
        if (down) {
          audition();
        } else {
          audition(false);
        }
      },
      tilt(direction) {
        if (direction === 'left' || direction === 'right') {
          mk.arrow(direction === 'right' ? 1 : -1);
        } else {
          stepInstrument((direction === 'down' ? 1 : -1) * LIST_ROWS);
        }
      }
    };
  }
  function stepInstrument(direction) {
    const count = P().instruments.length,
      before = st().instrument;
    st().instrument = clamp(st().instrument + direction, 0, count - 1);
    if (st().instrument === before) return;
    scheduleRefresh();
    if (M.screen !== 'channel') flash(`Instrument ${String(st().instrument).padStart(2, '0')} · ${P().instruments[st().instrument].name}`, 700);
  }
  function instrumentList() {
    const p = P(), typeName = { sample: 'Sampler', au: 'Plug-in', midi: 'MIDI' };
    return listData('instruments',
        p.instruments.map((inst, i) => ({ name: `${String(i).padStart(2, '0')} ${inst.name || ''}`, right: inst.type === 'sample' && inst.mode === 'wavetable' ? 'Wavetable' : typeName[inst.type] || inst.type || '', on: p.tracks[st().track]?.instrument === i, color: p.tracks.findIndex(t => t.instrument === i) >= 0
            ? trackColor(p.tracks.findIndex(t => t.instrument === i))
            : null })),
        st().instrument, { title: 'Instruments', active: true });
  }
  function audition(on = true) {
    const inst = P().instruments[st().instrument];
    if (!inst) return;
    if (on) {
      S().noteOn(inst.rootNote ?? 60, 'mk3-audition', false);
    } else {
      S().noteOff('mk3-audition');
    }
    if (on) setTimeout(() => S().noteOff('mk3-audition'), 600);
  }
  function trackKnobs(track, kinds = ['volume', 'pan', 'delaySend', 'reverbSend']) {
    return [...kinds.map(kind => mixKnob(track, kind)), null, null, null, null].slice(0, 8);
  }
  const MIX = {
    volume: ['Volume', 0, 1.5, fmt.db, 'volume', 'Track volume', 'volume'],
    pan: ['Pan', -1, 1, fmt.pan, 'pan', 'Track pan', 'pan'],
    delaySend: ['Delay', 0, 1, fmt.pct, 'delay', 'Delay send', 'delay'],
    reverbSend: ['Reverb', 0, 1, fmt.pct, 'reverb', 'Reverb send', 'reverb']
  };
  function mixKnob(track, kind, label) {
    const t = P().tracks[track]; if (!t) return null;
    const [name, min, max, f, target, arName, lockType] = MIX[kind];
    return numKnob({ key: `mix:${track}:${kind}`, label: label || name, name: `${t.name} · ${name}`, read: p => Number(p.tracks[track]?.[kind]
        ?? (kind === 'volume' ? .8 : 0)), write: (p, v) => { if (p.tracks[track]) p.tracks[track][kind] = v; }, min, max, fmt: f, ref: arRef(track,
            target, `${t.name} · ${arName}`), lock: lockSpec(lockType) });
  }
  function masterKnob() {
    return numKnob({
      key: 'master',
      label: 'Master',
      name: 'Master volume',
      read: p => Number(p.masterVolume),
      write: (p, v) => {
        p.masterVolume = v;
      },
      min: 0,
      max: 1.5,
      fmt: fmt.db,
      ref: arRef('master', 'volume', 'Master volume')
    });
  }

  function channelMidiPage() {
    const inst = P().instruments[st().instrument], track = st().track;
    const knobs = inst?.type === 'midi'
        ? [instKnob('midiChannel'), instKnob('midiInputChannel'), null, null, ...trackKnobs(track).slice(0, 4)]
        : trackKnobs(track);
    return { id: 'channelMidi', title: 'Channel · MIDI & routing', main: listMainInstrument(),
      buttons: [button('Routing', () => perform('routing')), button('Sends FX', () => perform('sendEffects')), button('MIDI devices',
          () => perform('settings')), button('Freeze', () => perform('freezeTrack'), {
        shift: button('Unfreeze', () => perform('unfreezeTrack'))
      }), button('Bounce', () => perform('bounceTrack')), button('Dup track',
          () => perform('duplicateTrack')), button('Add track', () => perform('addTrack')), button('Back', () => mk.setScreen('channel'))],
      knobs, data: { kind: 'info', title: `${P().tracks[track]?.name} · routing`, lines: [inst?.type === 'midi'
          ? `External MIDI · out ch ${inst.midiChannel || 1} · in ch ${inst.midiInputChannel || 'omni'}`
          : `Instrument: ${inst?.name || '–'} (${inst?.type || ''})`,
          `Volume ${fmt.db(P().tracks[track]?.volume ?? .8)} · Pan ${fmt.pan(P().tracks[track]?.pan ?? 0)}`,
          `Delay send ${fmt.pct(P().tracks[track]?.delaySend || 0)} · Reverb send ${fmt.pct(P().tracks[track]?.reverbSend || 0)}`,
          window.STFreeze?.isFrozen?.(track) ? 'Frozen track' : 'Live track'] } };
  }

  // Plug-in slots: the NKS instrument console (if any) followed by the plug-in pages console's slots.
  function pluginSlots() {
    const slots = [], nks = window.STNKSControls?.controller?.view?.();
    if (nks) slots.push({ key: 'nks', label: 'Instrument', name: nks.name, nks: true });
    const view = window.STPluginPages?.controller?.view?.();
    if (view) for (const slot of view.slots) slots.push({ ...slot, nks: false });
    return { slots, nks, view };
  }
  /* The track's signal chain as one list: built-in instrument → plug-in instrument / NKS → inserts → sends.
     PLUG-IN mode walks it with the 4-D encoder (docs/MASCHINE_MK3.md "Signal chain"). */
  function trackChain() {
    const { slots, nks, view } = pluginSlots(), inst = P().instruments[st().instrument], chain = [];
    if (inst && inst.type === 'sample' && !nks) chain.push({
      key: 'builtin',
      label: 'Instrument',
      name: inst.name || 'Sampler',
      builtin: true
    });
    let insert = 0;
    for (const slot of slots) chain.push({
      ...slot,
      label: slot.nks ? 'Instrument' : slot.kind === 'effect' ? `Insert ${++insert}` : slot.label || 'Instrument'
    });
    // The instrument's own insert chain (instrument-effects.js) follows the instrument source, ahead of the track's inserts.
    const fxOwner = `instrument:${st().instrument}`, sources = chain.filter(item => item.kind !== 'effect').length;
    const instrumentFx = (window.STInstrumentEffects?.chain(P(), st().instrument) || []).map((plugin, i) => ({ key: `${fxOwner}:${plugin.instanceID}`,
      instanceID: plugin.instanceID, owner: fxOwner, slot: plugin.slot, kind: 'effect', instrumentFx: true, label: `Instr. FX ${i + 1}`, name: plugin.name || 'Effect' }));
    chain.splice(sources, 0, ...instrumentFx);
    chain.push({ key: 'sends', label: 'Sends', name: 'Volume · pan · sends', sends: true });
    return { chain, slots, nks, view };
  }
  const isBypassed = instanceID => !!instanceID && !!P().audioPlugins.find(plugin => plugin.instanceID === instanceID)?.bypass;
  function chainItems(chain) {
    return chain.map(item => ({
      name: item.name || 'Empty',
      right: item.label + (item.kind === 'effect' && isBypassed(item.instanceID) ? ' · off' : ''),
      detail: item.instrumentFx
          ? 'Instrument effect'
          : item.builtin
          ? 'Built-in instrument'
          : item.sends
          ? 'Track levels and send effects'
          : item.nks
          ? 'NKS pages'
          : item.kind === 'effect'
          ? 'Insert effect'
          : 'Plug-in instrument'
    }));
  }
  M.pluginSlot = null;
  M.pluginFocus = 'chain';
  /* 4-D encoder in PLUG-IN mode. Chain focus: turn walks the chain. Push opens the selected module's parameter
     pages (pages focus: turn = page); push again, PLUG-IN or a Back press returns to the chain. Tilt ◄ ► always
     steps the module, tilt ▲ ▼ the parameter page. */
  function chainMain({ count, move, pages, page, setPage }) {
    const focus = M.pluginFocus === 'pages' ? 'pages' : 'chain';
    return { list: focus === 'pages' ? { count: pages, rows: LIST_ROWS } : { count, rows: LIST_ROWS },
      turn(delta) { if (focus === 'pages') setPage(page + delta); else move(delta); },
      push(down) { if (down) M.pluginFocus = focus === 'pages' ? 'chain' : 'pages'; },
      tilt(direction) { if (direction === 'left'
          || direction === 'right') move(direction === 'right' ? 1 : -1); else setPage(page + (direction === 'down' ? 1 : -1)); } };
  }
  function moduleStatus({ loaded, error, pages, knobs }) {
    if (error) return { text: error, error: true };
    if (loaded === false) return { text: 'Reading parameters…', loading: true };
    if (!pages?.length || !(knobs || []).some(Boolean)) return { text: 'No parameters exposed · Editor opens its window', empty: true };
    return null;
  }
  /* Level buttons of CHANNEL and PLUG-IN: Master · Track · Instr. (display 1–3); while the cursor track is in a
     track group, Master · Group · Track · Instr. (display 1–4; Group = the group bus). Each level remembers its
     selected module. */
  const LEVELS = [['master', 'Master'], ['group', 'Group'], ['track', 'Track'], ['instrument', 'Instr.']];
  const levelList = () => LEVELS.filter(([level]) => level !== 'group' || !!groupOf());
  // The level shown: Group falls back to Track while the cursor track is outside a group.
  const currentLevel = () => M.level === 'group' && !groupOf() ? 'track' : M.level || 'instrument';
  function levelButtons() {
    return levelList().map(([level, label]) => button(label, () => setLevel(level), { on: currentLevel() === level }));
  }
  /* The level buttons followed by a page's own buttons (8 in all). With the extra Group level one of `rest` gives
     way: its first empty slot, else rest[drop] (default: the last). */
  function withLevels(rest, drop = rest.length - 1) {
    const levels = levelButtons(), extra = levels.length + rest.length - 8, list = [...rest];
    for (let n = 0; n < extra; n++) { const empty = list.indexOf(null); list.splice(empty >= 0 ? empty : Math.min(drop, list.length - 1), 1); }
    return [...levels, ...list].slice(0, 8);
  }
  function setLevel(level) {
    if (!commitBursts()) { flash('Pending controller edit could not finish · retry the gesture'); return; }
    if (level === currentLevel() && level === M.level) return;
    M.levelSlots[M.level || 'instrument'] = M.pluginSlot; M.level = level; M.pluginSlot = M.levelSlots[level] ?? null; M.pluginFocus = 'chain';
    flash({
      master: 'Master',
      group: `Group · ${groupOf()?.name || ''}`,
      track: `Track · ${P().tracks[st().track]?.name || ''}`,
      instrument: `Instrument · ${P().instruments[st().instrument]?.name || ''}`
    }[level], 600);
  }
  /* Bypass and Remove for the selected insert effect (display buttons 4 and 8, docs/MK3_DESIGN_STUDY.md §5.2), through
     the public slot API (window.STInserts, studio.js). Remove asks first: press it again within 2.5 s, or Shift + Remove. */
  M.removeArm = null;
  function insertButtons(owner, slot, name, bypassed) {
    const api = window.STInserts;
    if (!api || !owner || !Number.isInteger(Number(slot))) return { bypass: null, remove: null };
    const fail = error => flash(error?.message || String(error), 2000);
    const bypass = button(bypassed ? 'Bypassed' : 'Bypass', () => {
      if (!commitBursts()) { flash('Pending controller edit could not finish · retry'); return; }
      Promise.resolve(api.toggleBypass(owner, slot)).then(on => { if (on !== null && on !== undefined) flash(`${name} · ${on ? 'bypassed' : 'on'}`, 900); mk.schedule(); }, fail);
    }, { on: !!bypassed });
    const removeNow = () => {
      M.removeArm = null;
      if (!commitBursts()) { flash('Pending controller edit could not finish · retry'); return; }
      Promise.resolve(api.remove(owner, slot)).then(done => {
        if (!done) return;
        M.pluginFocus = 'chain';
        flash(`Removed ${name} · Undo restores it`, 1800); mk.schedule();
      }, fail);
    };
    const key = `${owner}:${slot}`;
    const armed = M.removeArm?.key === key && mk.now() < M.removeArm.until;
    const remove = button(armed ? 'Confirm' : 'Remove', () => {
      if (M.removeArm?.key === key && mk.now() < M.removeArm.until) { removeNow(); return; }
      M.removeArm = { key, until: mk.now() + 2500 };
      flash(`Remove ${name}? Press again`, 2500);
    }, { shift: button('Remove now', removeNow), on: armed });
    return { bypass, remove };
  }
  // Automation write gestures end 700 ms after the last turn: one timer per parameter, so turning a second
  // knob does not leave the first one's gesture open (in Touch mode it kept writing for the rest of the pass).
  const recTimers = new Map();
  /** Encoders for one plug-in parameter page (track or master slots): discrete detents, LOCK, automation writing. */
  function pluginParamKnobs(current, ctl, { arOwner, kind, slotIndex, lockable = true }) {
    return (current?.knobs || []).map((knob, i) => {
      if (!knob || !knob.address) return knob ? { label: knob.name, value: knob.display || 'Unassigned', norm: null, unmapped: true } : null;
      const count = Number(knob.stepCount), discrete = Number.isInteger(count) && count > 0;
      const lock = knob.writable && lockable ? lockSpec('plugin', {
        instanceID: current.instanceID,
        address: knob.address,
        name: knob.name,
        ...(discrete ? { stepCount: count } : {})
      }) : null;
      const result = {
        label: knob.name,
        name: `${current.name} · ${knob.name}`,
        value: knob.display,
        norm: knob.value,
        section: knob.section,
        lock,
        base: knob.value,
        global: true
      };
      const adjust = delta => {
        if (!knob.writable) return;
        const ref = knob.automatable ? arRef(arOwner, `plugin:${kind}:${slotIndex}:${knob.address}`, `${current.name} · ${knob.name}`) : null;
        if (arRecording(ref)) {
          let next = clamp01((knob.value ?? 0) + delta); if (discrete) next = Math.round(next * count) / count;
          const recKey = `${arOwner}:${kind}:${slotIndex}:${knob.address}`;
          AR().value(ref, next, 'mk3'); clearTimeout(recTimers.get(recKey));
          recTimers.set(recKey, setTimeout(() => { recTimers.delete(recKey); AR()?.end?.(ref); }, 700)); knob.value = next; return;
        }
        ctl.adjust(i, delta);
      };
      if (discrete) {
        result.detent = .03;
        result.step = direction => { if (active('lock') && lock) lockTurn(lock, direction / count, knob.value); else adjust(direction / count); };
      } else result.turn = adjust;
      return result;
    });
  }
  /* PLUG-IN for ordinary tracks (docs/MK3_DESIGN_STUDY.md §5.2): display buttons 1–3 pick the level, the left
     screen lists that level's chain (4-D encoder: turn walks it, push opens its pages), ◄ ► step the parameter
     pages (page dots in the title bar), encoders = the page's 8 parameters. Instrument: built-in instrument, NKS
     or plug-in instrument. Track: insert effects, then Sends. Master: master inserts, then Master volume. */
  function pluginPage() {
    const level = currentLevel();
    if (level === 'master') return masterPluginPage();
    if (level === 'group') return groupPluginPage();
    const { chain: full, nks, view } = trackChain(), track = st().track;
    const chain = full.filter(item => level === 'track' ? item.sends || item.kind === 'effect' && !item.instrumentFx : !item.sends && (item.kind !== 'effect' || item.instrumentFx));
    const browse = button('Browse', () => mk.setScreen('browser', { pluginTab: true }));
    const editorFor = (kind, slot) => () => S().request('plugin.editor', { owner: `track:${P().tracks[track]?.routingID}`, kind, slot }).catch(error => S().toast(error.message, 'error'));
    const tools = (editor, extra = []) => withLevels([editor ? button('Editor', editor, {
      shift: button('Chain', () => {
        M.overlay = 'pluginSlots';
      })
    }) : button('Chain', () => {
      M.overlay = 'pluginSlots';
    }), browse, button('+ FX', () => perform('addEffect')), ...extra, button('Audition', () => audition()), button('Automate',
        () => perform('automateParameter'))].slice(0, 5), 2);
    if (!chain.length) {
      return { id: 'plugin', title: 'Plug-in', pages: [], page: 0, main: { turn() {}, push() {}, tilt() {} }, buttons: tools(null), knobs: Array(8).fill(null),
        data: { kind: 'plugin', level, chain: listData('pluginChain', [], 0,
            { title: `Instrument · ${P().tracks[track]?.name || ''}`, empty: 'No instrument · Browse loads one', rows: 4 }), focus: 'chain', slotLabel: 'Instrument', name: 'No instrument', pages: [], page: 0, status: { text: 'Browse to load an instrument', empty: true } } };
    }
    let index = chain.findIndex(item => item.key === M.pluginSlot);
    if (index < 0) { index = chain.findIndex(item => !item.builtin && !item.sends && !item.instrumentFx); if (index < 0) index = 0; }
    const selected = chain[index]; M.pluginSlot = selected.key;
    const move = delta => {
      const next = chain[clamp(index + delta, 0, chain.length - 1)];
      if (next && next.key !== M.pluginSlot) {
        M.pluginSlot = next.key;
        flash(`${next.label} · ${next.name}`, 600);
      }
    };
    const chainTitle = `${level === 'track' ? 'Track chain' : 'Instrument'} · ${P().tracks[track]?.name || ''}`;
    const chainList = focus => listData(`pluginChain:${level}`, chainItems(chain), index, { title: chainTitle, active: focus });
    const finish = (page, pages, pageIndex, setPage, extra) => {
      page.main = chainMain({ count: chain.length, move, pages: pages.length, page: pageIndex, setPage });
      page.setPage = setPage;
      page.data = { kind: 'plugin', level, chain: chainList(M.pluginFocus !== 'pages'), focus: M.pluginFocus === 'pages'
          ? 'pages'
          : 'chain', slotLabel: selected.label, name: selected.name, pages, page: pageIndex,
        pageList: listData('pluginPages', pages.map((name, i) => ({ name: `${i + 1}  ${name}` })), pageIndex,
            { active: M.pluginFocus === 'pages' }), ...extra };
      if (page.data.bypassed) page.data.slotLabel = `${page.data.slotLabel} · Bypassed`;
      page.data.status = moduleStatus({ loaded: page.data.loaded, error: page.data.error, pages, knobs: page.knobs });
      return page;
    };
    if (selected.builtin || selected.sends) {
      const inst = P().instruments[st().instrument],
        groups = selected.sends ? [['Levels', ['volume', 'pan', 'delaySend', 'reverbSend']]] : instrumentGroups(inst);
      const pageIndex_ = pageIndex(selected.sends ? 'pluginSends' : 'pluginBuiltin', groups.length), keys = groups[pageIndex_][1];
      const setPage = i => { M.page[selected.sends ? 'pluginSends' : 'pluginBuiltin'] = clamp(i, 0, groups.length - 1); };
      const page = { id: 'plugin', title: 'Plug-in', pages: groups.map(g => g[0]), page: pageIndex_, slot: selected.key,
        buttons: selected.sends ? withLevels([button('Mixer', () => mk.setScreen('mixer')), button('Sends FX', () => perform('sendEffects')),
            button('Routing', () => perform('routing')), button('+ FX', () => perform('addEffect')), browse], 3)
          : tools(null, []).map(b => b?.label === 'Audition'
              ? button('Audition', () => audition(), { shift: button('Sample edit', () => mk.setScreen('sampling')) })
              : b),
        knobs: selected.sends ? trackKnobs(track) : keys.map(key => key ? instKnob(key) : null) };
      return finish(page, page.pages, pageIndex_, setPage, { loaded: true });
    }
    if (selected.instrumentFx) {
      // An instrument effect: its pages through the owner controller; LOCK writes a step lock that reaches this
      // track's instance of the instrument's chain.
      const ctl = window.STPluginPages?.controller?.forOwner?.(selected.owner, selected.instanceID), current = ctl?.view?.();
      const slotIndex = current?.slotIndex ?? selected.slot ?? 0;
      const knobs = current ? pluginParamKnobs(current, ctl, { arOwner: selected.owner, kind: 'effect', slotIndex }) : Array(8).fill(null);
      const fx = insertButtons(selected.owner, slotIndex, current?.name || selected.name, current?.bypass);
      const editor = () => S().request('plugin.editor', { owner: selected.owner, kind: 'effect', slot: slotIndex, ...(selected.instanceID ? { instanceID: selected.instanceID } : {}) })
          .catch(error => S().toast(error.message, 'error'));
      const pages = current?.pages || [], at = current?.page || 0;
      return finish({ id: 'plugin', title: 'Plug-in', pages, page: at, slot: selected.key,
        buttons: withLevels([fx.bypass, button('Editor', editor, { shift: button('Chain', () => { M.overlay = 'pluginSlots'; }) }), browse,
          button('+ FX', () => perform('addEffect')), fx.remove], 3), knobs },
      pages, at, i => ctl?.setPage?.(i),
      { name: current?.name || selected.name, loaded: current ? current.loaded : false, error: current?.error, bypassed: !!current?.bypass, builtin: !!current?.builtin });
    }
    if (!selected.nks && view && view.slot !== selected.key) { window.STPluginPages.controller.selectSlot(selected.key); }
    const current = selected.nks ? nks : window.STPluginPages.controller.view();
    if (!current) return finish({ id: 'plugin', title: 'Plug-in', pages: [], page: 0, slot: selected.key, buttons: tools(null), knobs: Array(8).fill(null) },
        [], 0, () => {}, { loaded: false });
    const ctl = selected.nks ? window.STNKSControls.controller : window.STPluginPages.controller;
    const kind = selected.nks ? 'instrument' : current.kind, slotIndex = selected.nks ? 0 : (selected.slot ?? 0);
    const knobs = pluginParamKnobs(current, ctl, { arOwner: track, kind, slotIndex });
    const setPage = i => ctl.setPage(i);
    const fx = kind === 'effect' ? insertButtons(current.owner, slotIndex, current.name, current.bypass) : null;
    const page = {
      id: 'plugin',
      title: 'Plug-in',
      pages: current.pages,
      page: current.page,
      slot: selected.key,
      buttons: fx ? withLevels([fx.bypass, button('Editor', editorFor(kind, slotIndex), { shift: button('Chain', () => { M.overlay = 'pluginSlots'; }) }),
        browse, button('+ FX', () => perform('addEffect')), fx.remove], 3) : tools(editorFor(kind, slotIndex)),
      knobs
    };
    return finish(page, current.pages, current.page, setPage,
        { name: current.name, loaded: current.loaded, error: current.error, nks: !!selected.nks || !!current.nks, bypassed: !!fx && !!current.bypass, builtin: !!current.builtin });
  }
  /** PLUG-IN · Master level: the master insert chain (STPluginPages owner 'master') and the master volume. */
  function masterPluginPage() {
    return ownerChainPage({ owner: 'master', level: 'master', title: 'Master chain', levelsName: 'Master volume', levelsDetail: 'Master output level',
      insertDetail: 'Master insert effect', levelKnobs: () => [masterKnob(), mk.swingKnob(), mk.tempoKnob(), null, null, null, null, null] });
  }
  /** PLUG-IN · Group level: the group bus inserts (owner 'bus:<busID>'), then Levels (bus volume and pan). */
  function groupPluginPage() {
    const group = groupOf(), owner = G().busOwner(group);
    return ownerChainPage({ owner, level: 'group', title: `Group chain · ${group.name}`, levelsName: 'Volume · pan', levelsDetail: 'Group bus level',
      insertDetail: 'Group insert effect', levelKnobs: () => [ownerMixKnob(owner, 'volume'), ownerMixKnob(owner, 'pan'), null, null, null, null, null, null] });
  }
  /* A mixer channel's chain by owner ('master' or a bus): its insert effects (STPluginPages.forOwner), then a
     Levels row. */
  function ownerChainPage({ owner, level, title, levelsName, levelsDetail, insertDetail, levelKnobs }) {
    const forOwner = id => window.STPluginPages?.controller?.forOwner?.(owner, id);
    let probe = null; try { probe = forOwner(null)?.view?.() || null; } catch { probe = null; }
    const slots = probe?.slots || [];
    const chain = [...slots.map((slot, i) => ({
      key: `${owner}:` + (slot.instanceID || slot.key),
      instanceID: slot.instanceID,
      label: `Insert ${i + 1}`,
      name: slot.name || 'Plug-in',
      kind: 'effect'
    })), { key: `${owner}:levels`, label: 'Levels', name: levelsName, levels: true }];
    let index = chain.findIndex(item => item.key === M.pluginSlot); if (index < 0) index = 0;
    const selected = chain[index]; M.pluginSlot = selected.key;
    const move = delta => {
      const next = chain[clamp(index + delta, 0, chain.length - 1)];
      if (next && next.key !== M.pluginSlot) {
        M.pluginSlot = next.key;
        flash(`${next.label} · ${next.name}`, 600);
      }
    };
    const items = chain.map(item => ({ name: item.name, right: item.label + (isBypassed(item.instanceID) ? ' · off' : ''), detail: item.levels ? levelsDetail : insertDetail }));
    const base = { id: 'plugin', title: 'Plug-in', slot: selected.key };
    const finish = (page, pages, pageIndex_, setPage, extra = {}) => {
      page.main = chainMain({ count: chain.length, move, pages: pages.length, page: pageIndex_, setPage }); page.setPage = setPage;
      page.data = { kind: 'plugin', level, owner, chain: listData(`pluginChain:${owner}`, items, index,
          { title, active: M.pluginFocus !== 'pages' }), focus: M.pluginFocus === 'pages'
              ? 'pages'
              : 'chain', slotLabel: selected.label, name: selected.name, pages, page: pageIndex_,
        pageList: listData('pluginPages', pages.map((name, i) => ({ name: `${i + 1}  ${name}` })), pageIndex_,
            { active: M.pluginFocus === 'pages' }), ...extra };
      if (page.data.bypassed) page.data.slotLabel = `${page.data.slotLabel} · Bypassed`;
      page.data.status = moduleStatus({ loaded: page.data.loaded, error: page.data.error, pages, knobs: page.knobs });
      return page;
    };
    const browse = button('Browse', () => mk.setScreen('browser', { pluginTab: true }));
    // + FX adds to this owner's chain (the group bus or Master), not to the selected member track's channel.
    const addFx = button('+ FX', () => {
      if (window.STStudio?.selectOwner) window.STStudio.selectOwner(owner); else st().mixerOwner = owner;
      perform('addEffect');
    });
    if (selected.levels) return finish({ ...base, pages: ['Levels'], page: 0, buttons: withLevels([
        button('Mixer', () => mk.setScreen('mixer')), browse, addFx, null, null]), knobs: levelKnobs() },
        ['Levels'], 0, () => {}, { loaded: true });
    const ctl = forOwner(selected.instanceID), current = ctl?.view?.();
    const editor = current ? () => S().request('plugin.editor', {
      owner,
      instanceID: current.instanceID,
      kind: current.kind,
      slot: current.slotIndex ?? 0
    }).catch(error => S().toast(error.message, 'error')) : null;
    const knobs = current ? pluginParamKnobs(current, ctl, {
      arOwner: owner,
      kind: current.kind,
      slotIndex: current.slotIndex ?? 0,
      lockable: false
    }) : Array(8).fill(null);
    const pages = current?.pages || [], page = current?.page || 0;
    const fx = current ? insertButtons(owner, current.slotIndex ?? 0, current.name, current.bypass) : { bypass: null, remove: null };
    return finish({ ...base, pages, page, buttons: withLevels([fx.bypass, button('Editor', editor), browse, addFx,
        fx.remove], 3), knobs },
        pages, page, i => ctl?.setPage?.(i),
        { name: current?.name || selected.name, loaded: current ? current.loaded : false, error: current?.error, bypassed: !!current?.bypass, builtin: !!current?.builtin });
  }
  const levelOf = item => item?.sends || item?.kind === 'effect' && !item?.instrumentFx ? 'track' : 'instrument';
  function pluginSlotsPage() {
    // Shift + PLUG-IN: the same signal chain as PLUG-IN mode, full screen; push opens the module's pages.
    const open = () => { M.overlay = null; M.pluginFocus = 'pages'; };
    // The whole track chain (instrument and track levels); choosing a module also selects its level.
    const { chain } = trackChain(), index = Math.max(0, chain.findIndex(item => item.key === M.pluginSlot));
    const pick = item => { if (!item) return; M.pluginSlot = item.key; M.level = levelOf(item); };
    const move = delta => pick(chain[clamp(index + delta, 0, chain.length - 1)]);
    return { id: 'pluginSlots', title: 'Signal chain', overlay: true,
      main: listMain({ count: chain.length, move, choose: open }),
      buttons: [...Array.from({ length: 7 }, (_, i) => chain[i] ? button(chain[i].label, () => {
        pick(chain[i]);
        open();
      }, { on: i === index }) : null), button('Back', () => {
        M.overlay = null;
      })],
      knobs: [actionKnob('Module', chain[index]?.label || '–', move, {
        options: chain.map(item => item.label),
        index
      }), null, null, null, null, null, null, null],
      data: listData('pluginSlots', chainItems(chain), index,
          { title: `Chain · ${P().tracks[st().track]?.name || ''}`, empty: 'No modules on this track', hint: 'Turn: module · push: open its pages · PLUG-IN: back' }) };
  }

  Object.assign(mk, {
    listMainInstrument, stepInstrument, instrumentList, audition, trackKnobs, MIX, mixKnob, masterKnob,
    channelMidiPage, chainMain, moduleStatus, LEVELS, levelList, currentLevel, levelButtons, withLevels, setLevel, pluginPage,
    insertButtons, ownerChainPage, pluginSlotsPage
  });
})();
