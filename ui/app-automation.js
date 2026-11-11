/* Automation entry points: automate a parameter from the UI, the context menu and the NKS
   browser shortcut. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, uid, state, pattern, toast, mutate, setView, refresh, closeModal } = app;

  function browseNKS(kind = 'instrument') {
    if (!window.STBrowser?.showNKS) {
      setView('browser');
      return false;
    }
    if (state.view !== 'browser') setView('browser');
    window.STBrowser.showNKS(kind);
    return true;
  }

  function automate(track, target, name, valueOverride) {
    const p = state.project,
      ownerArgument = typeof track === 'string'
          && (track === 'master' || track.startsWith('track:') || track.startsWith('bus:') || track.startsWith('instrument:'));
    const owner = ownerArgument ? track : window.STRoutingProject?.trackOwner(p.tracks[Number(track)]);
    const index = ownerArgument
        ? owner.startsWith('track:')
            ? p.tracks.findIndex(item => window.STRoutingProject?.trackOwner(item) === owner)
            : -1
        : Number(track);
    const validOwner = owner === 'master'
        || owner?.startsWith('bus:') && p.routing.buses.some(bus => `bus:${bus.id}` === owner)
        // An instrument's insert chain (instrument-effects.js): its lanes reach every track that plays it.
        || /^instrument:\d+$/.test(owner || '') && !!p.instruments[Number(owner.slice(11))] && target.startsWith('plugin:effect:')
        || index >= 0 && !!p.tracks[index];
    if (!validOwner) {
      toast('The channel for this parameter is no longer available.', 'error');
      return;
    }
    if (index >= 0
        && state.view === 'instrument'
        && !target.startsWith('plugin:')
        && p.tracks[index]?.instrument !== state.instrument) {
      toast('Assign this instrument to the selected track before adding its automation.');
      return;
    }
    const supported = ['volume', 'pan', 'cutoff', 'resonance', 'delay', 'reverb', 'pitch', 'attack', 'decay',
        'sustain', 'release', 'drive', 'bitDepth', 'lfoRate', 'lfoDepth', 'start', 'end', 'loopStart', 'loopEnd',
        'grainSize', 'grainPosition', 'sliceCount', 'instrumentVolume', 'instrumentPan', 'tune', 'filterAttack',
        'filterDecay', 'filterSustain', 'filterRelease', 'filterEnvAmount', 'lfo2Rate', 'lfo2Depth', 'lfoBeats',
        'lfo2Beats', 'wavetablePosition', 'grainDensity', 'grainSpray', 'grainPitchSpread', 'grainScan', 'grainReverse', 'grainStereo'];
    // A lane names its channel by owner; only the engine's own track parameters name the track by index.
    const routeLane = index < 0 || target.startsWith('send:'), ownerLane = routeLane || target.startsWith('plugin:'),
      descriptor = window.STStudio?.target?.(ownerArgument ? owner : index, target);
    const send = target.startsWith('send:')
        && p.routing.sends.find(item => item.id === target.slice(5) && item.source === owner);
    if (!target.startsWith('plugin:') && !supported.includes(target) && !send
        || descriptor?.available === false
        || routeLane && !descriptor) {
      toast(descriptor?.unavailableReason || 'This parameter does not currently support a tracker automation lane.',
          'error');
      return;
    }
    const pat = pattern();
    let selected = null;
    const select = lane => {
      selected = lane;
      state.selectedLane = pat.automation.indexOf(lane);
      if (index >= 0) state.track = index;
      if (owner) state.mixerOwner = owner;
      state.view = 'automation';
    };
    const existing = pat.automation.find(item => item.target === target
        && (ownerLane ? item.owner === owner : !item.owner && item.track === index));
    if (existing) {
      select(existing);
      refresh();
    } else {
      mutate('add automation', () => {
        const channel = p.tracks[index],
          inst = p.instruments[channel?.instrument || 0];
        const value = Number.isFinite(valueOverride)
            ? valueOverride
            : descriptor?.value
                ?? (send
                    ? send.gain
                    : target.startsWith('plugin:')
                    ? .5
                    : target === 'delay'
                    ? channel?.delaySend
                    : target === 'reverb'
                    ? channel?.reverbSend
                    : channel?.[target] ?? inst?.[target] ?? .5);
        const lane = {
          id: uid(),
          ...(ownerLane ? { owner } : { track: index }),
          target,
          name: name || target,
          interpolation: 'linear',
          points: [{ row: state.row, value }]
        };
        pat.automation.push(lane);
        select(lane);
      });
    }
    // A rejected mutation must leave its invoker alone. On success, refresh has
    // replaced that control; hand keyboard editing to the exact selected lane.
    const ownsLane = () => selected
        && state.project === p
        && pattern() === pat
        && state.view === 'automation'
        && pat.automation[state.selectedLane] === selected;
    if (!ownsLane()) return;
    if (state.modal) closeModal();
    if (!ownsLane() || state.modal) return;
    const envelope = document.getElementById('automation-envelope');
    envelope?.focus({ preventScroll: true });
    envelope?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    // The lane's box in the Automation view's lane stack.
    document.getElementById(`automation-lane-${state.selectedLane}`)?.scrollIntoView?.({ block: 'nearest' });
  }

  function contextMenu(e) {
    if (window.STTrackDuplicate?.contextMenu(e)) return;
    const target = e.target.closest('[data-automate]');
    if (!target) return;
    e.preventDefault();
    $('.context-menu')?.remove();
    const menu = document.createElement('div');
    menu.className = 'context-menu';
    menu.style.left = Math.min(e.clientX, innerWidth - 240) + 'px';
    menu.style.top = Math.min(e.clientY, innerHeight - 100) + 'px';
    menu.innerHTML = `<button id="context-automate">Create / show automation <kbd>⌥A</kbd></button>`;
    document.body.appendChild(menu);
    $('#context-automate').onclick = () => {
      menu.remove();
      if (target.dataset.nksAddress !== undefined && window.STNKSControls?.automateControl) {
        void window.STNKSControls.automateControl(target);
        return;
      }
      automate(target.dataset.owner ?? Number(target.dataset.track ?? state.track), target.dataset.automate,
          target.dataset.name, automateValue(target));
    };
    setTimeout(() => document.addEventListener('click', () => menu.remove(), { once: true }), 0);
  }

  // Option changes the text in key on macOS; use physical code only for this chord.
  // Controls that show a derived value (e.g. Fine tune in cents) carry the automation target's raw value separately.
  function automateValue(el) {
    const raw = el?.dataset?.automateValue;
    if (raw !== undefined && raw !== '') return Number(raw);
    return window.STFine?.has(el) ? window.STFine.laneFromElement(el) : Number(el?.value);
  }

  Object.assign(app, { browseNKS, automate, contextMenu, automateValue });
})();
