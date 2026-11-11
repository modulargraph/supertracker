/* MK3: native events, commands, the control map and the public window.STMK3 API. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const {
    S, st, P, BUTTONS, LED_BUTTONS, prefs, M, entries, groupOf, expression, watchQueue, bursts, commitBursts, flushEdits,
    LIST_ROWS, LIST_MARGIN, listWindow, currentPage, HANDLERS, SHIFT_PADS, input, screenModel, frame, sent, listeners,
    schedule, publish, startTicker, cancelGestures
  } = mk;

  // ---- Native events and commands -----------------------------------------------------------------------
  // `source` is 'simulator' for the on-screen MK3 (mk3-sim.js): its input drives the pages like the hardware's, but
  // is no sign of a connected controller.
  function event(type, payload = {}, source = 'native') {
    if (type === 'project.loaded') { cancelGestures({ projectChanged: true }); return; }
    if (type === 'status') { watchQueue(payload); return; }
    if (type === 'mk3.input') { if (payload.cancelled
        && !payload.targetOnly) { cancelGestures(); return; } if (!M.connected && st()?.native && source !== 'simulator') { M.connected = true; sent.leds = null; sent.screen = sent.left = sent.right = ''; } input(payload); startTicker(); return; }
    if (type === 'mk3.status') {
      const wasConnected = M.connected;
      const hadScreens = M.screens;
      M.connected = !!payload.connected;
      M.screens = !!payload.screens;
      M.exclusive = !!payload.exclusive;
      if ('enabled' in payload) M.enabled = payload.enabled === true;
      M.error = payload.error ? String(payload.error) : '';
      // Newly opened screens show their current content immediately.
      if (M.screens && !hadScreens) sent.screen = sent.left = sent.right = '';
      if (M.connected && !wasConnected) {
        sent.leds = null;
        sent.screen = sent.left = sent.right = '';
        startTicker();
        S()?.toast?.('Maschine MK3 connected');
      }
      if (!M.connected && wasConnected) { cancelGestures(); S()?.toast?.('Maschine MK3 disconnected'); }
      if (M.error) S()?.toast?.(`Maschine MK3: ${M.error}`, 'error');
      schedule();
    }
  }
  function command(name) {
    switch (name) {
      case 'mk3Simulator': return window.STMK3Sim?.toggle?.();
      case 'mk3Connect':
        if (M.enabled === false) { S().toast('Maschine MK3 support is in development and off. Turn it on in Settings first.', 'error'); return true; }
        return confirmTakeOver();
      case 'mk3Release': return S().request('mk3.disconnect').then(() => { M.exclusive = false; S().toast('Maschine MK3 released to NI software'); schedule(); }).catch(error => S().toast(error.message, 'error'));
      case 'mk3Status': return S().request('mk3.status').then(status => event('mk3.status', status || {})).catch(error => S().toast(error.message, 'error'));
      default: return false;
    }
  }
  function confirmTakeOver() {
    const app = S();
    app.openModal('Take over Maschine MK3 screens', `<p class="muted" style="line-height:1.7">SuperTracker will draw both MK3 screens. To do that it pauses Native Instruments' background `
        + `agents (NIHardwareAgent and NIHostIntegrationAgent) while SuperTracker runs, so Maschine and Komplete Kontrol cannot use the controller `
        + `until you release it or quit SuperTracker. The agents are restored when SuperTracker quits or when you choose Maschine MK3: release `
        + `controller.</p>`, '<button id="mk3-takeover-cancel">Cancel</button><button id="mk3-takeover-confirm" class="primary">Take over screens</button>', {
      kind: 'mk3-takeover'
    });
    document.getElementById('mk3-takeover-cancel')?.addEventListener('click', () => app.closeModal());
    document.getElementById('mk3-takeover-confirm')?.addEventListener('click', async () => {
      app.closeModal();
      app.toast('Connecting to Maschine MK3…');
      try {
        const status = await app.request('mk3.connect', { exclusive: true });
        if (status && typeof status === 'object') event('mk3.status', status);
        // The driver returns status on both success and recoverable failure.
        // An acknowledged request is not proof that either screen opened.
        if (status?.exclusive === true && status?.screens === true && !status.error) {
          app.toast('Maschine MK3 screens taken over');
        } else {
          app.toast(status?.error
              || (status?.connected
                  ? 'Maschine MK3 is connected, but its screens could not be opened. Release the controller in other music apps and try again.'
                  : 'No Maschine MK3 connected. Power it on and connect its USB cable to this Mac, then try again.'),
              'error');
        }
      } catch (error) { app.toast(error.message || 'Could not connect to Maschine MK3.', 'error'); }
    });
  }
  // Maschine MK3 support is in development: off by default, turned on here (mk3.configure).
  function settingsNote() {
    const on = M.enabled === true;
    return `<div id="mk3-settings"><div class="eyebrow">Maschine MK3 (in development)</div><label class="row"><input type="checkbox" id="mk3-enabled" ${on ? 'checked' : ''}>`
        + ` Enable Maschine MK3 controller support (experimental)</label><div class="notice info" id="mk3-settings-note">`
        + (on ? `${M.connected ? 'Maschine MK3 connected.' : 'No Maschine MK3 connected.'}`
          + ` Pads, buttons and LEDs work alongside Native Instruments software. To draw the controller's screens, choose <strong>Maschine MK3: take `
          + `over screens…</strong> in the command palette (⌘K); <strong>release controller</strong> hands it back. SETTINGS on the controller sets `
          + `pad sensitivity, velocity curve and brightness.`
          : 'Off. Support for the Native Instruments Maschine MK3 controller is still in development and may change or misbehave.'
          + ' Turn it on to use the pads, buttons and screens with SuperTracker.')
        + `</div>${M.error ? `<p class="notice error" role="alert">${S().escapeHTML(M.error)}</p>` : ''}</div>`;
  }
  function bindSettings() {
    const box = document.getElementById?.('mk3-enabled'); if (!box) return;
    box.onchange = async () => {
      const enabled = box.checked; box.disabled = true;
      try {
        event('mk3.status', await S().request('mk3.configure', { enabled }) || {});
        S().toast(enabled ? 'Maschine MK3 support turned on (in development)' : 'Maschine MK3 support turned off');
      } catch (error) { box.checked = !enabled; S().toast(`Could not change Maschine MK3 support: ${error.message}`, 'error'); }
      finally {
        const section = document.getElementById?.('mk3-settings');
        if (section) { section.outerHTML = settingsNote(); bindSettings(); } else box.disabled = false;
      }
    };
  }
  // Control map for help and docs: [control, press, shift].
  const CONTROL_MAP = [
    ['Home', 'No lit mode button: tracker across both screens, 8 columns under display buttons / encoders', 'Lit mode button again returns Home'],
    ['Display buttons (Home)', 'Select the column above (MUTE / SOLO held: mute / solo)', 'Mute the column; with SELECT held: pattern tools'],
    ['Encoders (Home)', 'Cursor step: Note · Inst · Velocity · Gate · FX 1 · FX 2 · Probability · Pattern', 'Fine'],
    ['4-D encoder (Home)', 'Turn: rows · tilt: columns, tracks · push: hear · still touch 0.4 s: overview', 'Shift + touch: overview at once'],
    ['CHANNEL', 'Display buttons: Master / (Group) / Track / Instr. level; instrument pages (◄ ►)', 'MIDI & routing page'], ['PLUG-IN',
        'Display buttons: Master / (Group) / Track / Instr. chain; 4-D: chain, push: pages; ◄ ►: pages', 'Full-screen chain'],
    ['ARRANGER', 'Song blocks, repeats, section loop', 'Pattern list'], ['MIXER',
        'Display buttons select (MUTE / SOLO held: mute / solo); ◄ ►: Volume / Pan / Delay / Reverb',
        'Mute with display buttons; Shift + MIXER: Pan page'],
    ['BROWSER', 'Filter lists (left) + results (right); tilt: columns',
        'Plug-in browser'], ['SAMPLING', 'Sample editor (markers, edits, slices)', 'Start / stop audio recording'],
    ['◄ / ►', 'Parameter page (page dots in the title) / browser tab; pages without pages: step page / octave / pad bank',
        'Group-button bank of 8 entries (tracks and track groups)'], ['FILE', 'File menu list (turn, push runs)', 'Save'],
    ['SETTINGS', 'Controller settings', 'App settings'], ['AUTO', 'Tap: automation Read ⇄ write; hold + turn: write while held',
        'Cycle Touch / Latch / Write'], ['MACRO', 'Step FX editor', 'Automation lane picker'],
    ['NOTES', 'Home; again: step-entry arm', 'Note-off at cursor'], ['VOLUME / SWING / TEMPO',
        'Hold or tap-latch: encoder mode over the current page (header strip)', 'Velocity / position / tuning of the cursor note'],
    ['NOTE REPEAT', 'Hold: pads repeat, rates on display buttons; tap latches',
        'Arp page: mode, rate, octaves, gate'], ['LOCK', 'Hold + encoders: step locks', 'Exit without deleting locks'],
    ['PITCH / MOD / PERFORM', 'Touch strip mode', 'Transpose / cutoff / choose effect'], ['PAD MODE / KEYBOARD / CHORDS',
        'Instruments (in a track group: its tracks) / scale keys (lit: key & scale page) / chord set', 'Slices / key & scale / chord set picker'],
    ['STEP', '16-step sequencer, hold pad + encoders edits',
        '(FOLLOW makes the page follow the playhead)'], ['FIXED VEL', 'Fixed velocity on/off', '16 Velocities (◄ ►: note); curve on SETTINGS'],
    ['SCENE', 'Pads = song blocks', 'Loop blocks'], ['PATTERN', 'Pads = patterns + PATTERN page (Length, Lines/beat, Colour); again: back',
        'New pattern'], ['EVENTS', 'Hold + pads/encoders: nudge, transpose, velocity, length (header strip)', 'Quantize'],
    ['VARIATION', 'Humanize (amount on encoder 1)', 'Navigate with 4-D encoder'], ['DUPLICATE', 'Hold + group/pad: duplicate', 'Double pattern'],
    ['SELECT / SOLO / MUTE', 'Hold + group/pad (SELECT on Home: pattern tools on display buttons)',
        'Select all / clear solos / clear mutes'], ['Groups A–H', 'Select a track or track group (MUTE / SOLO held: the group bus)', 'Arm the track'],
    ['Shift + pads 1–4', 'Undo · Redo · Step Undo · Step Redo', 'Shared edit history; the screens name the step'],
    ['Shift + pads 5–8', 'Quantize · Quantize 50% · Nudge earlier · Nudge later', ''],
    ['Shift + pads 9–12', 'Clear · Clear Auto · Copy · Paste', ''],
    ['Shift + pads 13–16', 'Semitone − · Semitone + · Octave − · Octave +', ''],
    ['RESTART', 'Restart', 'Loop range'], ['ERASE', 'Tap: clear step; hold + pad: erase notes',
        'Clear track pattern'], ['TAP', 'Tap tempo', 'Metronome'],
    ['FOLLOW', 'Follow playhead (also the STEP page)',
        'Edit step'], ['PLAY', 'Play / pause', 'Song ⇄ Pattern mode'], ['REC', 'Arm note recording', 'Count-in'], ['STOP', 'Stop', 'Panic']
  ];

  const api = {
    commitBursts: options => commitBursts(undefined, options), flushEdits, cancelGestures,
    get hasPendingEdits() { return [...bursts.values()].some(burst => burst.project === P()); },
    hasPendingEditsExcept(edit) { return [...bursts.values()].some(burst => burst.project === P() && burst !== edit); },
    ownsEdit(edit) { return !!edit && bursts.get(edit.key) === edit && edit.project === P(); },
    get editGeneration() { return mk.editGeneration; },
    event, command, settingsNote, bindSettings, CONTROL_MAP, BUTTONS, LED_BUTTONS, SHIFT_PADS, HANDLERS,
    get state() { return M; }, get prefs() { return prefs; },
    page: () => currentPage(), frame, screenModel, publish, entries, groupOf, listWindow, LIST_ROWS, LIST_MARGIN,
    subscribe(listener) { listeners.add(listener); startTicker(); schedule(); return () => { listeners.delete(listener); }; },
    _clock: null, _commitBursts: commitBursts, _bursts: bursts, _expression: expression
  };
  window.STMK3 = api;
  // A held encoder or pad edit is pending until its burst commits; the caller's own edit (mk3Edit) is exempt.
  window.STPendingEdits.register('mk3', {
    order: 50,
    hasPending: options => window.STMK3?.hasPendingEditsExcept
      ? window.STMK3.hasPendingEditsExcept(options.mk3Edit)
      : !!window.STMK3?.hasPendingEdits,
    generation: () => window.STMK3?.editGeneration ?? 0
  });

  Object.assign(mk, { command, api });
})();
