/* MK3: overlays (held / latched modifiers, Shift pickers), EVENTS and screen modes. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, clamp, NOTE_NAMES, noteText, PERF_FX, EDIT_STEPS, FILE_ITEMS, QUICK_SCALES,
    M, isHeld, active, pattern, trackColor, flash, scheduleRefresh, selectTrack, ensureView, perform,
    stepAt, patternIndex, groupOf, membersOf, entries, entryName, bursts, burstValue, burstEdit, commitBurst, commitBursts, enumKnob, actionKnob,
    button, fmt, instKnob, listMain, listData, stepKnobs, fxKnobs, typedFxKnob, notesPage, macroPage, autoLanesPage,
    channelPage, mixKnob, masterKnob, channelMidiPage, pluginPage, pluginSlotsPage, arrangerPage,
    patternsPage, swingKnob, tempoKnob, mixerPage, browserPage, samplingPage, settingsPage, encoderMode
  } = mk;

  // ---- Overlays (held / latched modifiers and Shift pickers) -----------------------------------------
  function overlayPage(id) {
    const p = P(), R = window.STNoteRepeat;
    const choice = (items, current, set) => items.map(([value, label]) => button(label, () => set(value), { on: value === current }));
    switch (id) {
      case 'velocity': case 'position': case 'tune': {
        const inst = p.instruments[st().instrument];
        const title = { velocity: 'Velocity', position: 'Position', tune: 'Tune' }[id];
        const supported = id !== 'tune' || inst?.type === 'sample';
        const knobs = id === 'velocity'
            ? [stepKnobs()[2]]
            : id === 'position'
            ? [stepAt()?.note >= 0 ? typedFxKnob('microtiming', 'Position') : null]
            : supported
            ? [instKnob('tuneSemi'), instKnob('tuneFine')]
            : [];
        const close = () => { commitBursts(); M.overlay = null; };
        const target = id === 'tune'
            ? supported ? inst.name || 'Instrument' : 'Native samplers only'
            : knobs[0]
            ? `${p.tracks[st().track]?.name || ''} · row ${String(st().row).padStart(3, '0')}`
            : 'No note on the cursor row';
        const hint = id === 'tune'
            ? supported ? 'Main: semitones · Shift + main: cents · push: close' : 'Use the plug-in or hardware tuning'
            : id === 'position'
            ? 'Delay within the step, 0–95% · push: close'
            : 'Note velocity 1–127 · push: close';
        return encoderMode(id, title, { title: title.toUpperCase(), target, value: knobs[0]?.value || 'Unavailable', hint }, {
          main: {
            turn(delta) {
              const knob = id === 'tune' && M.shift ? knobs[1] : knobs[0];
              for (let i = 0; i < Math.min(128, Math.abs(delta)); i++) knob?.step?.(Math.sign(delta));
            },
            push(down) {
              if (down) close();
            },
            tilt() {}
          },
          knobs, buttons: [null, null, null, null, null, null, null, button('Back', close)] });
      }
      case 'file': {
        const items = FILE_ITEMS, index = clamp(M.lists.file || 0, 0, items.length - 1);
        const run = i => {
          const [command, label] = items[i];
          M.overlay = null;
          perform(command);
          if (command === 'save') {
            flash('Saving…', 800);
          } else if (label.endsWith('…')) flash(label.replace('…', ''), 600);
        };
        return { id, title: 'File', overlay: true, main: listMain({ count: items.length, move: delta => { M.lists.file = clamp(index + delta, 0,
            items.length - 1); }, choose: () => run(index) }),
          buttons: [button('New', () => perform('new')), button('Open…', () => perform('open')), button('Export…', () => perform('export')),
              button('Export MIDI', () => perform('exportMIDI')), button('Import MIDI', () => perform('importMIDI')),
              button('Save as…', () => perform('saveAs')), button('Rename', () => perform('renameProject')), button('Demo', () => perform('demo'))],
          knobs: Array(8).fill(null), data: listData('file',
              items.map(([command, label, detail]) => ({ name: label, right: command === 'save' && st().dirty ? 'unsaved' : '', detail })), index,
              { title: p.name || 'Untitled', hint: 'Turn: choose · push: run · FILE: back · Shift + FILE: save' }) };
      }
      case 'volume': case 'master': {
        const master = id === 'master', track = p.tracks[st().track];
        const value = master
            ? fmt.db(burstValue('master', q => Number(q.masterVolume ?? .7)))
            : fmt.db(burstValue(`mix:${st().track}:volume`, q => Number(q.tracks[st().track]?.volume ?? .8)));
        return encoderMode(id, master ? 'Master volume' : 'Volume', { title: master ? 'MASTER' : 'VOLUME', target: master
            ? 'Master'
            : `${String(st().track + 1).padStart(2, '0')} ${track?.name || ''}`, value, hint: master
                ? '4-D: master volume'
                : '4-D: selected · Shift + 4-D: master' }, {
          main: {
            turn(delta) {
              (master || M.shift ? masterKnob() : mixKnob(st().track, 'volume'))?.turn(delta / 40);
            },
            push() {},
            tilt(direction) {
              if (direction === 'left' || direction === 'right') selectTrack(st().track + (direction === 'right' ? 1 : -1));
            }
          },
          // Encoders: the volumes of the group-button bank (track channels and group buses), as on the MIXER page.
          knobs: master ? [masterKnob()] : mk.mixerPage(false).knobs.map(k => k && { ...k, label: k.label.slice(0, 10) }) });
      }
      case 'swing': return encoderMode(id, 'Swing', { title: 'SWING', target: 'Project', value: fmt.pct(burstValue('swing',
          q => Number(q.swing || 0))), hint: '4-D or encoder 1 · swing applies to all tracks' }, {
        main: { turn(delta) { swingKnob().turn(delta / 75); }, push() {}, tilt() {} },
        buttons: [0, .08, .16, .25, .33, .5].map(v => button(`${Math.round(v * 100)}%`, () => burstEdit('swing', 'swing', q => Number(q.swing || 0), (q, x) => {
          q.swing = x;
        }, v), { on: Math.abs((p.swing || 0) - v) < .005 })).concat([null, null]),
        knobs: [swingKnob(),
            enumKnob({ key: 'lpb', label: 'Lines/beat', read: q => q.linesPerBeat || 4, write: (q, v) => { q.linesPerBeat = v; }, options: [1, 2, 3, 4, 6, 8, 12, 16].map(v => [v, String(v)]) })] });
      case 'tempo': return encoderMode(id, 'Tempo', { title: 'TEMPO', target: 'Project', value: `${fmt.bpm(burstValue('tempo', q => Number(q.bpm ?? 120)))} BPM`, hint: '4-D: 1 BPM (Shift: 0.1) · TAP: tap tempo' }, {
        main: {
          turn(delta) {
            const current = burstValue('tempo', q => Number(q.bpm ?? 120));
            burstEdit('tempo', 'tempo', q => Number(q.bpm ?? 120), (q, v) => { q.bpm = v; }, clamp(Number((current + delta * (M.shift ? .1 : 1)).toFixed(1)), 20, 400), { commit: tempoCommit });
          },
          push() {},
          tilt() {}
        },
        buttons: [80, 90, 100, 110, 120, 128, 140, 174].map(v => button(String(v), () => burstEdit('tempo', 'tempo', q => Number(q.bpm), (q, x) => {
          q.bpm = x;
        }, v, { commit: tempoCommit }))),
        knobs: [tempoKnob(), tempoKnob(true)] });
      case 'noteRepeat': {
        const rates = R?.rates || [];
        return encoderMode(id, 'Note repeat', { title: 'NOTE REPEAT', target: R ? R.label() : 'Note repeat', value: R?.enabled ? 'On' : 'Off', hint: M.latched === 'noteRepeat'
            ? 'Latched · tap NOTE REPEAT to stop'
            : 'Hold and play pads · tap to latch' }, {
          main: { turn(delta) { R?.cycleRate(delta > 0 ? 1 : -1); }, push() {}, tilt() {} },
          buttons: [...rates.map(([rate]) => button(rate, () => R.set({ rate }, false), { on: R.rate === rate })),
              button(R?.enabled ? 'On' : 'Off', () => R?.toggle(), { on: !!R?.enabled }), button('Mode', () => R?.cycleMode(1))].slice(0,
              8),
          knobs: repeatKnobs(R, ['rate', 'mode', 'octaves', 'gate']) });
      }
      // Shift + NOTE REPEAT (Arp): the arpeggiator page. Display buttons pick the mode; encoders set Mode, Rate,
      // Octaves and Gate. Hold NOTE REPEAT (or tap to latch) to play with these settings.
      case 'arp': {
        const modes = R?.modes || [], index = Math.max(0, modes.findIndex(([key]) => key === R?.mode)), close = () => { M.overlay = null; };
        const pick = i => R?.set({ mode: modes[clamp(i, 0, modes.length - 1)][0] }, false);
        const DETAIL = { repeat: 'Every held note on each step (drum rolls)', up: 'Held notes low to high', down: 'Held notes high to low',
          updown: 'Up, then down, without repeating the ends', random: 'A random held note each step', played: 'In the order the pads were pressed' };
        return { id, title: 'Arp', overlay: true, main: listMain({ count: modes.length, move: delta => pick(index + delta), choose: close }),
          buttons: [...modes.slice(0, 6).map(([key, name]) => button(name.replace(/^Arp (.)/, (_, c) => c.toUpperCase()), () => R.set({ mode: key }, false), { on: R?.mode === key })),
              button(R?.enabled ? 'On' : 'Off', () => R?.toggle(), { on: !!R?.enabled }), button('Back', close)],
          knobs: repeatKnobs(R, ['mode', 'rate', 'octaves', 'gate']),
          data: listData('arp', modes.map(([key, name]) => ({ name, detail: DETAIL[key] || '' })), index,
            { title: 'Arp', big: R ? R.label() : 'Note repeat', lines: [`Rate ${R?.rate || ''} · gate ${Math.round((R?.gate ?? .5) * 100)}% · ${R?.octaves || 1} octave${(R?.octaves || 1) > 1 ? 's' : ''}`,
                'Hold NOTE REPEAT and play pads · tap NOTE REPEAT to latch', 'Turn: mode · push or Shift + NOTE REPEAT: back'] }) };
      }
      case 'lock': { const base = baseScreenPage(), hint = `Turn encoders to lock values on row ${String(st().row).padStart(2, '0')}`;
        return { ...base, id: 'lock', title: 'Lock', overlayOf: M.screen, overlay: true, data: { ...(base.data || {}), lockHint: hint, header: { title: 'LOCK', target: `row ${String(st().row).padStart(3, '0')}`, value: '', hint: 'Encoders write step locks · Shift + LOCK: exit', lock: true } } }; }
      case 'scale': {
        const scales = Object.keys(S().scales), index = Math.max(0, scales.indexOf(st().scale)), close = () => { M.overlay = null; };
        const setScale = i => S().setPerformance({ scale: scales[clamp(i, 0, scales.length - 1)] }, false),
          setRoot = d => S().setPerformance({ root: (st().root + d + 12) % 12 }, false);
        return { id, title: 'Key & scale', overlay: true, main: listMain({ count: scales.length, move: delta => setScale(index + delta), choose: close, column: setRoot }),
          buttons: QUICK_SCALES.map(key => button((S().scaleNames[key] || key).replace(' pentatonic', ' pent.'), () => S().setPerformance({
            scale: key
          }, false), { on: st().scale === key })),
          knobs: [actionKnob('Root', NOTE_NAMES[st().root], setRoot, {
            detent: .07,
            options: NOTE_NAMES,
            index: st().root
          }), actionKnob('Scale', S().scaleNames[st().scale], direction => setScale(index + direction), {
            detent: .07,
            list: true
          }), actionKnob('Pads', st().scaleMode === 'degrees' ? 'Degrees' : 'Snap', () => S().setPerformance({
            scaleMode: st().scaleMode === 'degrees' ? 'snap' : 'degrees'
          }, false), { detent: .1 }), actionKnob('Octave', st().octave, direction => S().cyclePerformance('octave', direction),
              { detent: .1 }), null, null, null, null],
          data: listData('scale', scales.map(key => ({ name: S().scaleNames[key] || key, right: `${(S().scales[key] || []).length} notes` })), index,
            { title: 'Scale', big: `${NOTE_NAMES[st().root]} ${S().scaleNames[st().scale]}`, lines: ['Keyboard pads play this key; root notes are lit brightest.',
                'Turn: scale · tilt ◄ ►: root · push or KEYBOARD: back'] }) };
      }
      case 'chordSet': {
        const sets = S().chordSets, ids = ['', ...sets.map(set => set.id)], chordKeys = Object.keys(S().chords), set = S().activeChordSet();
        const pickSet = idx => S().setPerformance({ chordSet: ids[clamp(idx, 0, ids.length - 1)] }, false),
          index = Math.max(0, ids.indexOf(st().chordSet || ''));
        return { id, title: 'Chords', overlay: true, main: listMain({ count: ids.length, move: delta => pickSet(index + delta), choose: () => { M.overlay = null; }, column: direction => S().cyclePerformance('chord',
            direction) }),
          buttons: [button('Set off', () => pickSet(0), {
            on: !set
          }), button('‹ Set', () => S().cycleChordSet(-1)), button('Set ›', () => S().cycleChordSet(1)), button('‹ Chord',
              () => S().cyclePerformance('chord', -1)), button('Chord ›',
                  () => S().cyclePerformance('chord', 1)), button('Triad', () => S().setPerformance({
            chord: 'scaleTriad',
            chordSet: ''
          }, false), { on: st().chord === 'scaleTriad' && !set }), button('7th',
              () => S().setPerformance({ chord: 'scaleSeventh', chordSet: '' },
                  false), { on: st().chord === 'scaleSeventh' && !set }), button('MIDI',
                  () => perform('midiPerformance'), { on: st().midiApply !== false })],
          knobs: [actionKnob('Chord set', set ? set.name : 'Off', direction => pickSet(index + direction), {
            detent: .07,
            list: true
          }), actionKnob('Chord', S().chords[st().chord]?.name, direction => S().setPerformance({
            chord: chordKeys[clamp(chordKeys.indexOf(st().chord) + direction, 0, chordKeys.length - 1)],
            chordSet: ''
          }, false), {
            detent: .07,
            options: chordKeys.map(key => S().chords[key]?.name || key),
            index: Math.max(0, chordKeys.indexOf(st().chord))
          }), actionKnob('Root', NOTE_NAMES[st().root], direction => S().setPerformance({ root: (st().root + direction + 12) % 12 }, false), {
            detent: .07,
            options: NOTE_NAMES,
            index: st().root
          }), actionKnob('Octave', st().octave, direction => S().cyclePerformance('octave', direction), { detent: .1 }), null, null, null, null],
          data: listData('chordSet', [{ name: 'Off · scale chords', right: S().chords[st().chord]?.name || '', detail: 'CHORDS pads harmonise scale degrees' },
              ...sets.map(item => ({ name: item.name, right: `${item.chords.length} chords`, detail: item.mood || '' }))], index,
            { title: 'Chord set', big: set ? set.name : S().chords[st().chord]?.name, chords: set
                ? set.chords.map((_, i) => S().chordSetChordName(set, i))
                : null, lines: set
                ? [set.mood || '']
                : ['No chord set: CHORDS pads harmonise scale degrees', 'Turn: set · tilt ◄ ►: chord · push: back'] }) };
      }
      case 'fixedVel': return { id, title: 'Fixed velocity', overlay: true, main: { turn(delta) { M.fixedVelValue = clamp(M.fixedVelValue + delta, 1, 127); }, push(down) { if (down) M.overlay = null; }, tilt() {} },
        buttons: [127, 110, 100, 90, 80, 64, 40, 20].map(v => button(String(v), () => {
          M.fixedVelValue = v;
          M.fixedVel = true;
        }, { on: M.fixedVel && M.fixedVelValue === v })),
        knobs: [actionKnob('Velocity', M.fixedVelValue, direction => {
          M.fixedVelValue = clamp(M.fixedVelValue + direction, 1, 127);
        }, { detent: .008, norm: M.fixedVelValue / 127 }), null, null, null, null, null, null, null],
        data: { kind: 'overlay', big: M.fixedVel
            ? `Fixed ${M.fixedVelValue}`
            : 'Dynamic', lines: ['FIXED VEL toggles · Shift + FIXED VEL: 16 velocities', 'Velocity curve and pad sensitivity: SETTINGS'] } };
      case 'editStep': return { id, title: 'Edit step', overlay: true, main: { turn(delta) { S().cyclePerformance('editStep',
          delta > 0 ? 1 : -1); }, push(down) { if (down) M.overlay = null; }, tilt() {} },
        buttons: EDIT_STEPS.map(value => button(String(value), () => {
          st().editStep = value;
          scheduleRefresh();
        }, { on: st().editStep === value })).concat([button('Back', () => {
          M.overlay = null;
        })]),
        knobs: [actionKnob('Edit step', st().editStep, direction => S().cyclePerformance('editStep', direction), {
          detent: .1
        }), null, null, null, null, null, null, null],
        data: { kind: 'overlay', big: `Step ${st().editStep}`, lines: ['Rows the cursor advances after each entered note'] } };
      case 'perfFx': {
        const index = Math.max(0, PERF_FX.findIndex(([key]) => key === M.perfFx)),
          pick = i => mk.setStripMode(M.strip.mode, M.strip.shift, PERF_FX[clamp(i, 0, PERF_FX.length - 1)][0]);
        return { id, title: 'Perform FX', overlay: true, main: listMain({ count: PERF_FX.length, move: delta => pick(index + delta), choose: () => { M.overlay = null; } }),
          buttons: [...choice(PERF_FX, M.perfFx,
              value => mk.setStripMode('perform', M.strip.shift, value)), null, null, null, button('Back', () => {
            M.overlay = null;
          })],
          knobs: [actionKnob('Effect', PERF_FX[index][1], direction => pick(index + direction), {
            detent: .1,
            list: true
          }), null, null, null, null, null, null, null],
          data: listData('perfFx', PERF_FX.map(([key, label]) => ({ name: label, detail: { filter: 'Low-pass sweep of every built-in instrument', stutter: 'Loops 16 → 1 rows of the playing pattern', fade: 'Master volume fade-out', echo: 'Throws every track into the delay' }[key] })), index,
            { title: 'Perform FX', big: PERF_FX[index][1], lines: ['Touch strip in PERFORM mode plays this effect; it springs back on release.',
                'Turn: effect · push: back'] }) };
      }
      case 'events': {
        const rows = eventRows(), label = M.events.size ? `${M.events.size} steps` : st().selection ? 'selection' : 'track page';
        const kn = (name, fn, detent = .05) => actionKnob(name, label, direction => fn(direction), { detent });
        const shown = rows.slice(0, 6).map(r => String(r).padStart(2, '0')).join(' ') + (rows.length > 6 ? ' …' : '');
        return encoderMode(id, 'Events', { title: 'EVENTS', target: label, value: shown, hint: 'Pads select steps · encoders edit · Shift + EVENTS: quantize' }, {
          main: { turn(delta) { nudgeRows(rows, delta); }, push() {}, tilt() {} },
          buttons: [button('Nudge ‹', () => nudgeRows(rows, -1)), button('Nudge ›', () => nudgeRows(rows, 1)), button('Semi −',
              () => transposeRows(rows, -1)), button('Semi +', () => transposeRows(rows, 1)), button('Oct −',
                  () => transposeRows(rows, -12)), button('Oct +', () => transposeRows(rows, 12)), button('Quantize',
                      () => quantizeRows(rows, 0)), button('Clear sel', () => {
            M.events.clear();
          })],
          knobs: [kn('Nudge', direction => nudgeRows(rows, direction), .06), kn('Transpose', direction => transposeRows(rows, direction), .03),
              kn('Velocity', direction => scaleRows(rows, 'velocity', direction), .02),
              kn('Length', direction => scaleRows(rows, 'gate', direction), .03), kn('µTiming', direction => microRows(rows, direction), .03)],
          data: { events: [...M.events] } });
      }
      case 'variation': return { id, title: 'Variation', overlay: true, main: { turn(delta) { M.variationAmount = clamp(M.variationAmount + delta, 1, 64); }, push() {}, tilt() {} },
        buttons: [button('Humanize', () => window.STPatternTools?.humanize(M.variationAmount)),
            button('Randomize', () => perform('randomizeVelocity')), button('Louder', () => perform('louder')),
            button('Softer', () => perform('softer')), button('Reverse', () => perform('reverseSteps')),
            button('Interp', () => perform('interpolate')), button('Nudge ↑', () => perform('nudgeUp')),
            button('Nudge ↓', () => perform('nudgeDown'))],
        knobs: [actionKnob('Amount', `±${M.variationAmount}`, direction => {
          M.variationAmount = clamp(M.variationAmount + direction, 1, 64);
        }, { detent: .02, norm: M.variationAmount / 64 }), null, null, null, null, null, null, null],
        data: { kind: 'overlay', big: `Humanize ±${M.variationAmount}`, lines: ['Acts on the selection, or the cursor track\'s whole pattern',
            'VARIATION humanizes · Shift + VARIATION navigates'] } };
      case 'stepEdit': {
        const rows = [...M.pads.values()].filter(pad => pad.stepRow != null).map(pad => pad.stepRow).sort((a, b) => a - b);
        const base = stepKnobs(rows), [t1, v1] = fxKnobs(1, rows), [t2, v2] = fxKnobs(2, rows);
        return { id, title: 'Step edit', overlay: true, main: { turn(delta) { base[0]?.step?.(delta); }, push() {}, tilt() {} }, buttons: Array(8).fill(null),
          knobs: [base[0], base[2], base[3], t1, v1, t2, v2,
              base[1]], data: { kind: 'step', title: `Step ${rows.map(r => String(r).padStart(2, '0')).join(', ')}`, grid: stepGrid(), held: rows, step: stepAt(st().track, rows[0]) } };
      }
      default: return null;
    }
  }
  /** Note repeat / arp encoders: Rate, Mode, Octaves (arp modes only) and Gate, in the order given. */
  function repeatKnobs(R, order) {
    if (!R) return [];
    const modes = R.modes || [], arp = R.mode !== 'repeat';
    const knobs = {
      rate: actionKnob('Rate', R.rate, direction => R.cycleRate(direction), { detent: .1, options: (R.rates || []).map(([rate]) => rate),
        index: Math.max(0, (R.rates || []).findIndex(([rate]) => rate === R.rate)) }),
      mode: actionKnob('Mode', (modes.find(([key]) => key === R.mode) || ['', ''])[1], direction => {
        const at = modes.findIndex(([key]) => key === R.mode);
        R.set({ mode: modes[clamp(at + direction, 0, modes.length - 1)][0] }, false);
      }, { detent: .1, options: modes.map(([, name]) => name), index: Math.max(0, modes.findIndex(([key]) => key === R.mode)) }),
      octaves: arp
          ? actionKnob('Octaves', String(R.octaves), direction => R.set({ octaves: R.octaves + direction }, false), { detent: .12, norm: (R.octaves - 1) / 3 })
          : { label: 'Octaves', name: 'Octaves (arp modes)', value: '—', norm: null, dim: true },
      gate: actionKnob('Gate', `${Math.round(R.gate * 100)}%`, direction => R.set({ gate: R.gate + direction * .05 }, false), { detent: .03, norm: (R.gate - .1) / .9 })
    };
    return order.map(key => knobs[key]);
  }
  function tempoCommit(v, options) {
    return S().mutate('tempo', p => {
      p.bpm = v;
    }, { ...options, render: false });
  }

  function currentScreenPage() {
    switch (M.screen) {
      case 'macro': return macroPage();
      case 'autoLanes': return autoLanesPage();
      case 'channel': return channelPage();
      case 'channelMidi': return channelMidiPage();
      case 'plugin': return pluginPage();
      case 'arranger': return arrangerPage();
      case 'patterns': return patternsPage();
      case 'mixer': return mixerPage(false);
      case 'mixerSends': return mixerPage(true);
      case 'browser': return browserPage();
      case 'sampling': return samplingPage();
      case 'settings': return settingsPage();
      default: return notesPage();
    }
  }
  function overlayId() {
    if ([...M.pads.values()].some(pad => pad.stepRow != null) && M.padMode === 'step' && !active('events')) return 'stepEdit';
    for (const name of ['volume', 'swing', 'tempo', 'noteRepeat', 'events']) if (isHeld(name)) return name;
    if (isHeld('lock')) return 'lock';
    if (isHeld('variation')) return 'variation';
    if (M.overlay) return M.overlay;
    for (const name of ['volume', 'swing', 'tempo', 'noteRepeat', 'events']) if (M.latched === name) return name;
    if (M.latched === 'lock') return 'lock';
    return null;
  }
  /** The page under encoder modes and LOCK: the PATTERN page while it is shown, else the screen mode's page. */
  function baseScreenPage() { return mk.patternPageVisible() ? patternsPage(true) : currentScreenPage(); }
  function currentPage() {
    const id = overlayId();
    if (id === 'pluginSlots') return pluginSlotsPage();
    if (id === 'autoLanes') return autoLanesPage();
    const page = id ? overlayPage(id) : null;
    return page || baseScreenPage();
  }

  function stepGrid() {
    const pat = pattern(), track = st().track, start = M.stepPage * 16, state = st();
    const play = state.playing && state.status?.pattern === pat.id ? Number(state.status.row) : -1;
    return { color: trackColor(track), cells: Array.from({ length: 16 },
        (_, i) => { const row = start + i, s = row < pat.length
            ? stepAt(track, row, pat)
            : null; return { row, note: s ? noteText(s.note) : '', velocity: s && s.note >= 0
                ? s.velocity ?? 100
                : 0, playhead: row === play, cursor: row === state.row, outside: row >= pat.length }; }) };
  }
  // ---- Events (EVENTS hold) ------------------------------------------------------------------------------
  function eventRows() {
    if (M.events.size) return [...M.events].sort((a, b) => a - b);
    const bounds = S().selectedBounds();
    if (bounds) return Array.from({ length: bounds.r1 - bounds.r0 + 1 }, (_, i) => bounds.r0 + i);
    const start = M.stepPage * 16; return Array.from({ length: 16 }, (_, i) => start + i).filter(r => r < pattern().length);
  }
  function settleEventOrder(patternId, track, moving) {
    // Nudge snapshots the whole lane; field edits read particular rows. Complete
    // the other operation first so neither reads or overwrites stale row content.
    for (const [key] of [...bursts]) {
      const parts = key.split(':'), kind = parts[0], offset = kind === 'events' ? 2 : 1;
      const rowEdit = ['step', 'lock', 'typed', 'fxtype', 'fxval', 'events'].includes(kind), isMove = kind === 'events' && parts[1] === 'nudge';
      if (!rowEdit || parts[offset] !== String(patternId) || parts[offset + 1] !== String(track) || isMove === moving) continue;
      if (!commitBurst(key)) { flash('Pending step edit could not finish · retry the gesture'); return false; }
    }
    return true;
  }
  function editRows(label, rows, fn, fields) {
    const track = st().track, patternId = pattern().id;
    if (!settleEventOrder(patternId, track, false)) return;
    const key = `events:${label}:${patternId}:${track}:${rows.join(',')}`;
    const read = p => JSON.stringify(rows.map(r => p.patterns.find(item => item.id === patternId)?.steps?.[track]?.[r] || null));
    const before = bursts.get(key)?.value ?? read(P());
    const draft = JSON.parse(before), next = fn(draft) || draft;
    const value = JSON.stringify(next); if (value === before) return;
    burstEdit(key, label, read, (p, value) => {
      const steps = p.patterns.find(item => item.id === patternId)?.steps?.[track]; if (!steps) return;
      JSON.parse(value).forEach((step, i) => {
        const current = steps[rows[i]]; if (!step || !current) return;
        // Keep simultaneous edits from the other EVENTS encoders.
        const updated = { ...current };
        for (const field of fields) { if (Object.hasOwn(step, field)) updated[field] = step[field]; else delete updated[field]; }
        steps[rows[i]] = updated;
      });
    }, value);
  }
  function nudgeRows(rows, direction) {
    if (!rows.length) return;
    const track = st().track, pat = pattern(), patternId = pat.id, len = pat.length, d = Math.sign(direction);
    if (!settleEventOrder(patternId, track, true)) return;
    const read = p => JSON.stringify(p.patterns.find(item => item.id === patternId)?.steps?.[track] ?? []);
    const key = `events:nudge:${patternId}:${track}`;
    const steps = JSON.parse(bursts.get(key)?.value ?? read(P()));
    const moving = rows.map(r => steps[r]);
    for (const r of rows) steps[r] = null;
    rows.forEach((r, i) => { const target = (r + d + len) % len; if (moving[i]) steps[target] = moving[i]; });
    const shifted = rows.map(r => (r + d + len) % len);
    if (M.events.size) M.events = new Set(shifted);
    burstEdit(key, 'nudge steps', read, (p, value) => {
      const target = p.patterns.find(item => item.id === patternId);
      if (target) target.steps[track] = JSON.parse(value);
    }, JSON.stringify(steps));
  }
  function transposeRows(rows, amount) {
    editRows('transpose steps', rows, steps => steps.map(s => s && s.note >= 0 ? { ...s, note: clamp(s.note + amount, 0, 127) } : s), ['note']);
  }
  function scaleRows(rows, field, direction) {
    editRows(field === 'gate' ? 'step length' : 'step velocity', rows, steps => steps.map(s => {
      if (!s || !(s.note >= 0)) return s;
      if (field === 'velocity') return { ...s, velocity: clamp(Math.round((s.velocity ?? 100) + direction * 2), 1, 127) };
      // Length scales a note's gate; a note without one (plays until the next note) gets none to scale.
      const gate = S().stepGate(s);
      if (gate === null) return s;
      return { ...s, gate: clamp(Number((gate * (direction > 0 ? 1.08 : 1 / 1.08)).toFixed(3)), .01, 256) };
    }), [field]);
  }
  function microRows(rows, direction) {
    editRows('step microtiming', rows, steps => steps.map(s => {
      if (!s || !(s.note >= 0)) return s;
      const key = ['fx1', 'fx2'].find(k => s[k]?.type === 'microtiming') || ['fx1', 'fx2'].find(k => !s[k]);
      if (!key) return s;
      const value = clamp(Number(((s[key]?.value || 0) + direction * .05).toFixed(2)), 0, .95);
      const next = { ...s };
      if (value <= 0) {
        delete next[key];
      } else {
        next[key] = { type: 'microtiming', value };
      }
      return next;
    }), ['fx1', 'fx2']);
  }
  function quantizeRows(rows, keep = 0) {
    const track = st().track, pat = pattern();
    const eventAt = row => pat.steps[track]?.[row];
    let count = 0;
    for (const r of rows) for (const k of ['fx1', 'fx2']) if (eventAt(r)?.[k]?.type === 'microtiming') count++;
    if (!count) { flash('No microtiming to quantize'); return false; }
    if (S().mutate(keep ? 'quantize 50%' : 'quantize',
        () => { for (const r of rows) { const s = eventAt(r); if (!s) continue; for (const k of ['fx1', 'fx2']) if (s[k]?.type === 'microtiming') { if (keep) s[k].value = Number((s[k].value * keep).toFixed(3)); else delete s[k]; } } }) === false) return false;
    flash(keep ? 'Quantized 50%' : `Quantized ${count} step${count === 1 ? '' : 's'}`);
  }

  // ---- Screen modes --------------------------------------------------------------------------------------
  function setScreen(screen, options = {}) {
    commitBursts(); M.overlay = null; M.patternPage = false;
    if (screen === 'patterns' && M.screen !== 'patterns') M.lists.patterns = Math.max(0, patternIndex());
    if (M.screen !== screen) { M.screen = screen; M.page[screen] = M.page[screen] || 0; }
    ensureView(screen);
    if (screen === 'browser' && options.pluginTab) window.STBrowser?.showPlugins?.('instrument');
    mk.schedule();
  }
  /* ◄ ►: on every page with parameter pages (CHANNEL, PLUG-IN, MIXER) they step pages, shown by the page dots in
     the title bar; the browser steps tabs. Pages without pages serve the pad mode (step page, octave, pad bank).
     Shift + ◄ ►: group-button bank (eight entries: tracks and track groups). */
  function arrow(direction) {
    const page = currentPage();
    const paged = page.setPage && page.pages?.length > 0, parameterMode = ['channel', 'plugin', 'mixer', 'mixerSends'].includes(page.id);
    if (M.shift) {
      const list = entries(), banks = Math.ceil(list.length / 8);
      M.bank = clamp(M.bank + direction, 0, Math.max(0, banks - 1));
      const shown = list.slice(M.bank * 8, M.bank * 8 + 8);
      flash(shown.length ? `Groups ${M.bank * 8 + 1}–${M.bank * 8 + shown.length} · ${entryName(shown[0])}` : 'No tracks', 700); return;
    }
    if (paged && (parameterMode || page.pages.length > 1)) { page.setPage(clamp((page.page || 0) + direction, 0, page.pages.length - 1)); return; }
    if (parameterMode) return;
    if (page.id === 'browser') { page.setTab(direction); return; }
    if (page.id === 'sampling') { window.STSampler?.controller?.scroll(direction * .25); return; }
    // Pages without sub-pages: arrows serve the pad mode.
    if (M.padMode === 'step') { M.stepPage = clamp(M.stepPage + direction, 0,
        Math.max(0, Math.ceil(pattern().length / 16) - 1)); flash(`Steps ${M.stepPage * 16}–${Math.min(pattern().length, M.stepPage * 16 + 16) - 1}`,
        700); return; }
    if (M.padMode === 'keyboard' || M.padMode === 'chords') { S().cyclePerformance('octave', direction); return; }
    // 16 Velocities: ◄ ► move the note the pads play by a semitone.
    if (M.padMode === 'velocity') {
      const target = mk.velocityTarget(), note = clamp(target.note + direction, 0, 127);
      M.lastNote.set(target.track, note); flash(`16 velocities · ${noteText(note)}`, 700); return;
    }
    if (M.padMode === 'pattern' || M.padMode === 'scene' || M.padMode === 'pad') {
      // PAD MODE with the cursor in a track group pages its members (16 per page), otherwise the instruments.
      const key = M.padMode, group = key === 'pad' ? groupOf() : null,
        count = key === 'pattern'
            ? P().patterns.length + 1
            : key === 'scene'
            ? window.STSongModel?.blocks(P().sequence).length || 0
            : group
            ? membersOf(group).length
            : P().instruments.length;
      M.padBank[key] = clamp(M.padBank[key] + direction, 0, Math.max(0, Math.ceil(count / 16) - 1));
      flash(`${key === 'pad' ? group ? `${group.name} pads` : 'Instruments' : key === 'scene' ? 'Blocks' : 'Patterns'} ${M.padBank[key] * 16 + 1}–${M.padBank[key] * 16 + 16}`, 700);
    }
  }

  Object.assign(mk, { tempoCommit, baseScreenPage, currentPage, eventRows, quantizeRows, setScreen, arrow });
})();
