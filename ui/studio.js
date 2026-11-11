/* Keyboard-first mixer and pattern automation. No external UI dependencies. */
(() => {
  'use strict';
  const ui = {
    pluginMoving: false,
    effectMoveDone: null,
    pluginEditing: 0,
    pluginGeneration: 0,
    pluginPicker: false,
    pluginQuery: '',
    plugins: null,
    pluginBusy: false,
    parameters: new Map(),
    parameterOwners: new Map(),
    parameterErrors: new Map(),
    pending: new Set(),
    point: 0,
    clipboard: null,
    peaks: [],
    peak: 0,
    previewTimer: null,
    previewSerial: 0,
    busPeaks: {},
    parameterTimers: new Map(),
    openPlugins: new Map(),
    ref: null,
    scope: 'pattern',
    contextRegistered: false
  };
  // Insert slots per mixer channel (tracks, group and return buses, Master): 0...15.
  const INSERT_SLOTS = 16;
  const S = () => window.ST;
  const state = () => S().state;
  const project = () => state().project;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const clamp = (v, min, max) => Math.max(min, Math.min(max, Number(v) || 0));
  const number = (v, fallback = 0) => Number.isFinite(Number(v)) ? Number(v) : fallback;
  const trackIndex = () => clamp(state().track, 0, Math.max(0, project().tracks.length - 1));
  const selectTrack = index => {
    state().track = Number(index);
    state().instrument = project().tracks[state().track]?.instrument || 0;
    state().mixerOwner = trackOwner(state().track);
  };
  const pattern = () => project().patterns.find(item => item.id === project().activePattern) || project().patterns[0];
  const lanes = () => pattern().automation;
  const selectedLane = () => lanes()[clamp(state().selectedLane, 0, Math.max(0, lanes().length - 1))];
  const trackOwner = (index, p = project()) => `track:${p.tracks[index]?.routingID}`;
  const normalizeOwner = (owner, p = project()) => typeof owner === 'number' || /^\d+$/.test(String(owner))
      ? trackOwner(Number(owner), p)
      : String(owner);
  // Tracks in track order; a track group shows its bus strip followed by its member strips
  // (hidden while the group is collapsed in the mixer). Other buses and Master follow.
  function channels(p = project()) {
    const groups = window.STGroups.list(p),
      groupBuses = new Set(groups.map(group => group.busID)),
      result = [];
    p.tracks.forEach((track, index) => {
      const group = groups.length ? window.STGroups.ofTrack(p, index) : null;
      if (group && window.STGroups.members(p, group)[0] === index) {
        const bus = p.routing.buses.find(item => item.id === group.busID);
        if (bus) result.push({ owner: `bus:${bus.id}`, type: 'group', name: bus.name, entity: bus, token: `bus-${bus.id}`, group, groupHead: true });
      }
      result.push({
        owner: trackOwner(index, p),
        type: 'track',
        name: track.name,
        entity: track,
        track: index,
        token: String(index),
        group: group || null,
        hiddenByGroup: !!group?.mixerCollapsed
      });
    });
    for (const type of ['group', 'return']) for (const bus of p.routing.buses) if (bus.type === type && !groupBuses.has(bus.id)) result.push({
      owner: `bus:${bus.id}`,
      type,
      name: bus.name,
      entity: bus,
      token: `bus-${bus.id}`
    });
    result.push({ owner: 'master', type: 'master', name: 'Master', entity: p, token: 'master' }); return result;
  }
  // An instrument's insert chain ('instrument:<n>', see instrument-effects.js) is an insert owner but not a mixer strip.
  const instrumentChannel = (owner, p) => {
    const match = /^instrument:(\d+)$/.exec(String(owner ?? '')), index = match ? Number(match[1]) : -1, inst = p.instruments[index];
    return inst ? { owner: `instrument:${index}`, type: 'instrument', name: inst.name || `Instrument ${String(index).padStart(2, '0')}`, entity: inst, instrument: index } : null;
  };
  const channelFor = (owner, p = project()) => String(owner ?? '').startsWith('instrument:')
      ? instrumentChannel(owner, p)
      : channels(p).find(channel => channel.owner === normalizeOwner(owner, p));
  // Strips shown in the mixer: a group collapsed in the mixer hides its member strips (except the selected one).
  const visibleChannels = (owner = selectedOwner()) => channels().filter(c => !c.hiddenByGroup || c.owner === owner);
  function selectedOwner() {
    const selected = channelFor(state().mixerOwner);
    if (selected
        && selected.type !== 'instrument'
        && (selected.type !== 'track' || selected.track === trackIndex())) return selected.owner;
    return trackOwner(trackIndex());
  }
  function selectOwner(owner) {
    const channel = channelFor(owner); if (!channel || channel.type === 'instrument') return false;
    state().mixerOwner = channel.owner;
    if (channel.type === 'track') selectTrack(channel.track);
    return true;
  }
  const channelTitle = owner => channelFor(owner)?.name || 'Unavailable channel';
  function mixerIdentity(owner = selectedOwner()) {
    const channel = channelFor(owner);
    if (!channel) return 'Unavailable channel';
    if (channel.type === 'master') return 'Master';
    const prefix = channel.type === 'track'
        ? String(channel.track + 1).padStart(2, '0')
        : channel.type === 'group'
        ? 'Group'
        : 'Return';
    return `${prefix} · ${channel.name}`;
  }
  function updateMixerIdentity(node) {
    const strip = node?.closest?.('[data-mixer-channel]');
    const owner = strip?.dataset.mixerOwner ?? strip?.dataset.mixerChannel ?? selectedOwner();
    const output = document.getElementById('mixer-current-channel');
    if (output) { output.textContent = mixerIdentity(owner); output.title = output.textContent; }
  }
  const mixerHeading = () => `<h2>Mixer<output id="mixer-current-channel" aria-label="Current mixer channel" title="${esc(mixerIdentity())}">${esc(mixerIdentity())}</output></h2>`;
  const ownerForPlugin = plugin => plugin.owner;
  const ownerForLane = lane => normalizeOwner(lane.owner ?? lane.track);
  const ownerPayload = owner => ({ owner: normalizeOwner(owner) });
  const pluginKey = (owner, slot, kind = 'effect') => `${normalizeOwner(owner)}:${kind}:${slot}`;
  const pluginsFor = owner => project().audioPlugins.filter(plugin => ownerForPlugin(plugin) === normalizeOwner(owner));
  const effectsFor = owner => pluginsFor(owner).filter(plugin => plugin.kind === 'effect').sort((a, b) => a.slot - b.slot);
  const effectTarget = (slot, address) => `plugin:effect:${slot}:${address}`;
  const focusId = id => { if (!state().modal) document.getElementById(id)?.focus(); };
  const notifyError = error => S().toast(error?.message || String(error), 'error');
  const mutate = (label, fn, focus) => { S().mutate(label, fn); if (focus) focusId(focus); };
  function settleAutomationEdit(pointRow) {
    const recorder = window.STAutomationRecord;
    if (!recorder?.hasPendingEdits) return true;
    const lane = selectedLane(), row = Number.isFinite(pointRow) ? pointRow : lane && ordered(livePoints(lane) || lane.points)[ui.point]?.row;
    if (recorder.finishPass({render:false}) === false) return false;
    if (lane && lane === selectedLane() && Number.isFinite(row)) {
      const points = ordered(lane.points);
      ui.point = points.reduce((best, point, index) => Math.abs(point.row - row) < Math.abs(points[best].row - row) ? index : best, 0);
    }
    return true;
  }
  let pointFields = null, pointPointerHeld = false, pointPointerGuards = false;
  function validPointFields(scope) {
    return scope === pointFields
        && scope.project === project()
        && scope.pattern === pattern()
        && scope.lane === selectedLane()
        && state().view === 'automation'
        && !state().modal
        && scope.anchor?.isConnected !== false;
  }
  function paintPointFields(scope) {
    scope.pending = true; clearTimeout(scope.timer);
    scope.timer = setTimeout(() => {
      if (!validPointFields(scope)) return;
      const active = document.activeElement;
      if (pointPointerHeld || scope.drafts.has(active)) return;
      scope.pending = false;
      const binding = scope.bindings.get(active), index = binding ? ordered(scope.lane.points).findIndex(point => point.row === binding.row) : -1;
      const focus = index >= 0 ? `point-${binding.role}-${index}` : active?.id;
      S().refresh();
      if (focus && !state().modal) document.getElementById(focus)?.focus({preventScroll:true});
    }, 0);
  }
  function bindPointFields() {
    const scope = pointFields = {
      project: project(),
      pattern: pattern(),
      lane: selectedLane(),
      anchor: document.getElementById('automation-envelope'),
      bindings: new Map(),
      drafts: new Set(),
      pending: false,
      timer: null
    };
    if (!pointPointerGuards && document.addEventListener) {
      pointPointerGuards = true;
      document.addEventListener('pointerdown', () => { pointPointerHeld = true; }, true);
      const released = () => { pointPointerHeld = false; if (pointFields?.pending) paintPointFields(pointFields); };
      for (const type of ['pointerup','pointercancel']) document.addEventListener(type,released,true);
      window.addEventListener?.('blur',released);
    }
    return scope;
  }
  const db = v => Number(v) <= 0 ? '−∞ dB' : `${(20 * Math.log10(v)).toFixed(1)} dB`;
  const gain = v => `${Math.round(number(v) * 100)}%`;
  const pan = v => Math.abs(number(v)) < .01 ? 'Center' : `${Math.round(Math.abs(v) * 100)}${v < 0 ? ' L' : ' R'}`;
  const duration = v => v < 1 ? `${Math.round(v * 1000)} ms` : `${number(v).toFixed(2)} s`;

  function builtInTargets(owner, includeUnavailable = false) {
    const channel = channelFor(owner), track = channel?.track;
    if (channel?.type === 'instrument') return [];
    if (channel?.type === 'master') return [{ target: 'volume', name: 'Master volume', group: 'Master', min: 0, max: 1.5, step: .01, value: number(project().masterVolume, .7), format: db, available: true }];
    if (channel
        && channel.type !== 'track') return [{ target: 'volume', name: 'Channel volume', group: channel.type === 'group'
            ? 'Group'
            : 'Return', min: 0, max: 2, step: .01, value: number(channel.entity.volume, 1), format: db, available: true },
        { target: 'pan', name: 'Channel pan', group: 'Mixer', min: -1, max: 1, step: .01, value: number(channel.entity.pan), format: pan, available: true }];
    const t = project().tracks[track] || {};
    const inst = project().instruments[t.instrument] || {};
    const targets = [
      { target: 'volume', name: 'Track volume', group: 'Mixer', min: 0, max: 1.5, step: .01, value: number(t.volume, .8), format: db },
      { target: 'pan', name: 'Track pan', group: 'Mixer', min: -1, max: 1, step: .01, value: number(t.pan), format: pan },
      { target: 'delay', name: 'Delay send', group: 'Sends', min: 0, max: 1, step: .01, value: number(t.delaySend), format: gain },
      { target: 'reverb', name: 'Reverb send', group: 'Sends', min: 0, max: 1, step: .01, value: number(t.reverbSend), format: gain },
      { target: 'cutoff', name: 'Filter cutoff', group: 'Instrument', min: 20, max: 20000, step: 1, value: number(inst.cutoff, 18000), format: v => v >= 1000
          ? `${(v / 1000).toFixed(2)} kHz`
          : `${Math.round(v)} Hz`, log: true },
      { target: 'resonance', name: 'Filter resonance', group: 'Instrument', min: 0, max: .95, step: .01, value: number(inst.resonance), format: gain },
      { target: 'pitch', name: 'Track pitch offset', group: 'Track modulation', min: -48, max: 48, step: .1, value: 0, format: v => `${number(v).toFixed(1)} st` },
      { target: 'instrumentVolume', name: 'Instrument level', group: 'Instrument output', min: 0, max: 1, step: .01, value: number(inst.volume, .7), format: db },
      { target: 'instrumentPan', name: 'Instrument pan', group: 'Instrument output', min: -1, max: 1, step: .01, value: number(inst.pan), format: pan },
      { target: 'tune', name: 'Instrument tuning', group: 'Instrument output', min: -48, max: 48, step: .01, value: number(inst.tune), format: v => `${number(v).toFixed(2)} st` },
      { target: 'attack', name: 'Envelope attack', group: 'Amplitude envelope', min: 0, max: 60, step: .001, value: number(inst.attack, .005), format: duration, fineCurve: { knee: .01, floor: .01 } },
      { target: 'decay', name: 'Envelope decay', group: 'Amplitude envelope', min: 0, max: 60, step: .001, value: number(inst.decay, .2), format: duration, fineCurve: { knee: .01, floor: .01 } },
      { target: 'sustain', name: 'Envelope sustain', group: 'Amplitude envelope', min: 0, max: 1, step: .01, value: number(inst.sustain, .7), format: gain },
      { target: 'release', name: 'Envelope release', group: 'Amplitude envelope', min: .0001, max: 60, step: .0001, value: number(inst.release, .2), format: duration, fineCurve: { knee: .01, floor: .01 } },
      { target: 'drive', name: 'Saturation drive', group: 'Sound shaping', min: 0, max: 1, step: .01, value: number(inst.drive), format: gain },
      { target: 'bitDepth', name: 'Bit depth', group: 'Sound shaping', min: 2, max: 16, step: 1, value: number(inst.bitDepth, 16), format: v => `${Math.round(v)} bit` },
      { target: 'lfoRate', name: 'LFO rate', group: 'Modulation', min: 0, max: 100, step: .01, value: number(inst.lfoRate), format: v => `${number(v).toFixed(2)} Hz` },
      { target: 'lfoBeats', name: 'LFO 1 cycle', group: 'LFO 1', min: 1 / 64, max: 64, step: 1 / 64, value: number(inst.lfoBeats, 1), format: v => `${number(v).toFixed(3).replace(/\.?0+$/, '')} beats` },
      { target: 'lfo2Beats', name: 'LFO 2 cycle', group: 'LFO 2', min: 1 / 64, max: 64, step: 1 / 64, value: number(inst.lfo2Beats, 1), format: v => `${number(v).toFixed(3).replace(/\.?0+$/, '')} beats` },
      { target: 'lfoDepth', name: 'LFO depth', group: 'Modulation', min: 0, max: 1, step: .01, value: number(inst.lfoDepth), format: gain },
      { target: 'start', name: 'Sample start', group: 'Sampler', min: 0, max: .999, step: .001, value: number(inst.start), format: gain },
      { target: 'end', name: 'Sample end', group: 'Sampler', min: .001, max: 1, step: .001, value: number(inst.end, 1), format: gain },
      { target: 'loopStart', name: 'Loop start', group: 'Sampler', min: 0, max: .999, step: .001, value: number(inst.loopStart), format: gain },
      { target: 'loopEnd', name: 'Loop end', group: 'Sampler', min: .001, max: 1, step: .001, value: number(inst.loopEnd, 1), format: gain },
      { target: 'grainSize', name: 'Grain size', group: 'Granular sampler', min: .002, max: .5, step: .001, value: number(inst.grainSize, .08), format: duration, fineCurve: { knee: .01, floor: .01 } },
      { target: 'grainPosition', name: 'Grain position', group: 'Granular sampler', min: 0, max: 1, step: .001, value: number(inst.grainPosition, .5), format: gain },
      { target: 'grainDensity', name: 'Grain density', group: 'Granular sampler', min: 1, max: 8, step: 1, value: number(inst.grainDensity, 2), format: v => `${Math.round(v)} grains` },
      { target: 'grainSpray', name: 'Grain spray', group: 'Granular sampler', min: 0, max: 1, step: .001, value: number(inst.grainSpray, 0), format: duration, fineCurve: { knee: .02, floor: .01 } },
      { target: 'grainPitchSpread', name: 'Grain pitch spread', group: 'Granular sampler', min: 0, max: 24, step: .1, value: number(inst.grainPitchSpread, 0), format: v => `±${Number(number(v).toFixed(2))} st` },
      { target: 'grainScan', name: 'Grain scan speed', group: 'Granular sampler', min: -4, max: 4, step: .01, value: number(inst.grainScan, 0), format: v => `${Number(number(v).toFixed(2))}×` },
      { target: 'grainReverse', name: 'Grain reverse chance', group: 'Granular sampler', min: 0, max: 1, step: .01, value: number(inst.grainReverse, 0), format: gain },
      { target: 'grainStereo', name: 'Grain stereo spread', group: 'Granular sampler', min: 0, max: 1, step: .01, value: number(inst.grainStereo, 0), format: gain },
      { target: 'wavetablePosition', name: 'Wavetable position', group: 'Wavetable', min: 0, max: 1, step: .001, value: number(inst.wavetablePosition, 0), format: gain },
      { target: 'sliceCount', name: 'Slice count', group: 'Slice sampler', min: 1, max: 128, step: 1, value: number(inst.sliceCount, 16), format: v => `${Math.round(v)} slices` }
    ];
    for (const descriptor of window.STModulation?.targets(track, true) || []) {
      const i = targets.findIndex(target => target.target === descriptor.target);
      if (i >= 0) {
        targets[i] = descriptor;
      } else {
        targets.push(descriptor);
      }
    }
    const type = inst.type || 'sample';
    const mixTargets = new Set(['volume', 'pan', 'delay', 'reverb']);
    const auTargets = new Set([...mixTargets, 'instrumentVolume', 'instrumentPan']);
    const samplerTargets = new Set(['start', 'end', 'loopStart', 'loopEnd', 'grainSize', 'grainPosition', 'sliceCount', 'wavetablePosition',
      'grainDensity', 'grainSpray', 'grainPitchSpread', 'grainScan', 'grainReverse', 'grainStereo']);
    for (const target of targets) {
      target.available = !!channel
          && (type === 'sample' || (type === 'plugin'
              ? auTargets.has(target.target)
              : type === 'midi'
              ? mixTargets.has(target.target)
              : !samplerTargets.has(target.target)));
      if (type === 'midi' && mixTargets.has(target.target)) target.group = 'Track audio · not MIDI hardware';
      if (!target.available) target.unavailableReason = !channel ? 'The original channel is no longer present.' : type === 'plugin' ? 'The assigned plug-in does not use the native sampler controls. Automate its exposed plug-in parameters instead.' : type === 'midi' ? 'This parameter does not control external MIDI hardware. It becomes available when a native sampler is assigned to '
          + 'this track.' : 'This parameter requires a sampler instrument assigned to this track.';
    }
    return includeUnavailable ? targets : targets.filter(target => target.available);
  }

  function allTargets(track) {
    const results = builtInTargets(track);
    for (const send of project().routing.sends) if (normalizeOwner(send.source) === normalizeOwner(track)) results.push({
      target: `send:${send.id}`,
      name: `Send to ${channelTitle(send.destination)}`,
      group: send.tap === 'preFader' ? 'Pre-fader sends' : 'Post-fader sends',
      min: 0,
      max: 2,
      step: .01,
      value: number(send.gain),
      format: db,
      available: true
    });
    for (const plugin of pluginsFor(track)) {
      const key = pluginKey(track, plugin.slot, plugin.kind);
      const params = (ui.parameterOwners.get(key) === plugin ? ui.parameters.get(key) : null) || plugin.parameters || [];
      const mapped = window.STNKSControls?.automationMetadata?.(plugin);
      for (const parameter of params) {
        if (parameter.writable === false || parameter.automatable === false) continue;
        const kind = plugin.kind || 'effect';
        const target = kind === 'instrument' ? `plugin:instrument:${plugin.slot}:${parameter.address}` : effectTarget(plugin.slot, parameter.address);
        const mapping = mapped?.get(String(parameter.address));
        results.push({
          target,
          name: parameter.name || parameter.id,
          group: plugin.name || 'plug-in',
          ...mapping,
          search: `${mapping?.search || ''} ${parameter.name || ''} ${parameter.id || ''} ${plugin.name || ''}`,
          min: 0,
          max: 1,
          step: window.STFine?.parameterStep(parameter) || .001,
          value: number(parameter.normalized),
          format: v => `${(v * 100).toFixed(1)}%`,
          plugin: true,
          parameter
        });
      }
    }
    return results;
  }

  function targetInfo(lane) {
    return allTargets(ownerForLane(lane)).find(item => item.target === lane.target)
        || builtInTargets(ownerForLane(lane), true).find(item => item.target === lane.target)
        || { target: lane.target, name: lane.name || lane.target, min: 0, max: 1, step: .01, value: 0, format: gain, available: false, unavailableReason: lane.target.startsWith('plugin:')
            ? 'This parameter is not currently exposed. Load the matching plug-in and refresh its parameters to use this lane.'
            : 'The current audio engine does not recognize this automation target.' };
  }

  function automateAttributes(owner, target, name) {
    const channel = channelFor(owner);
    return `data-automate="${esc(target)}" data-owner="${esc(normalizeOwner(owner))}" ${channel?.track !== undefined ? `data-track="${channel.track}"` : ''}`
        + ` data-name="${esc(name)}"`;
  }

  function range({ id, label, value, min = 0, max = 1, step = .01, format = gain, track, target, property, group }) {
    return `<label class="studio-knob" for="${id}"><span>${esc(label)}</span><output id="${id}-out">${esc(format(value))}</output>`
        + `<input type="range" id="${id}" min="${min}" max="${max}" step="${step}" value="${number(value)}" data-studio-property="${esc(property)}"`
        + ` ${group ? `data-studio-group="${group}"` : ''} ${track !== undefined
            ? `data-studio-track="${track}" tabindex="${Number(track) === trackIndex() ? 0 : -1}"`
            : ''}`
        + ` ${target ? automateAttributes(track, target, label) : ''} aria-label="${esc(label)}" aria-valuetext="${esc(format(value))}"></label>`;
  }

  /* Wide mixers (no track limit): only strips overlapping the viewport plus a small overscan, and always the
     selected strip, are built; spacers keep the scroll width. Meter updates therefore touch only built strips.
     A horizontal scroll that changes the visible range re-renders (rAF-throttled). */
  const MIXER_VIRTUAL_MIN = 40, STRIP_WIDTH = 128, STRIP_OVERSCAN = 2;
  let mixerRendered = null;
  const mixerVirtual = count => count > MIXER_VIRTUAL_MIN && !!window.STVirtual;
  function mixerRange(count, box, keep = []) {
    return window.STVirtual.segments(new Array(count).fill(STRIP_WIDTH), box ? box.scrollLeft : 0,
        box && Number(box.clientWidth) || Number(window.innerWidth) || 1280, keep, STRIP_OVERSCAN);
  }
  // Called while the outgoing mixer is still in the DOM; refresh() restores its scroll position on the new one.
  function mixerStrips(count, keep, render) {
    if (!mixerVirtual(count)) { mixerRendered = null; return Array.from({ length: count }, (_, i) => render(i)).join(''); }
    const layout = mixerRange(count, document.getElementById('mixer-channels'), [keep]);
    mixerRendered = { key: layout.key, count };
    return layout.segments.map(x => x.spacer ? `<div class="studio-channel-spacer" aria-hidden="true" style="width:${x.width}px;flex-basis:${x.width}px">`
        + `</div>` : render(x.index)).join('');
  }
  function bindMixerViewport() {
    const box = document.getElementById('mixer-channels'); if (!box || !mixerRendered) return;
    let queued = false;
    box.addEventListener('scroll', () => {
      if (queued) return;
      queued = true;
      requestAnimationFrame(() => {
        queued = false;
        if (box.isConnected === false || state().view !== 'mixer' || !mixerRendered) return;
        if (mixerRange(mixerRendered.count, box).key !== mixerRendered.key) S().refresh();
      });
    }, { passive: true });
  }
  const renderMixer = () => renderRoutedMixer();

  function routedRange(channel, role, label, value, property, max = 1, min = 0, format = gain) {
    const id = channelControlId(role, channel);
    return `<label class="studio-knob" for="${id}"><span>${esc(label)}</span><output id="${id}-out">${esc(format(value))}</output>`
        + `<input type="range" id="${id}" min="${min}" max="${max}" step=".01" value="${number(value)}" data-studio-owner="${esc(channel.owner)}"`
        + ` data-mixer-role="${role}" data-studio-property="${property}" tabindex="${channel.owner === selectedOwner() ? 0 : -1}" ${automateAttributes(channel.owner, role === 'vol' ? 'volume' : role, label)}`
        + ` aria-label="${esc(channel.name)} ${esc(label)}" aria-valuetext="${esc(format(value))}"></label>`;
  }
  function renderRoutedStrip(channel) {
    const selected = channel.owner === selectedOwner(),
      master = channel.type === 'master',
      track = channel.type === 'track',
      entity = channel.entity;
    const level = master ? number(project().masterVolume, .7) : number(entity.volume, track ? .8 : 1);
    const peak = master ? ui.peak : track ? number(ui.peaks[channel.track]) : Math.max(...(ui.busPeaks[channel.owner] || [0]));
    const sendCount = project().routing.sends.filter(send => send.source === channel.owner).length;
    const output = project().routing.outputs[channel.owner], outputName = output === null ? 'No output' : channelTitle(output || 'master');
    const tab = selected ? 0 : -1,
      prefix = track
          ? String(channel.track + 1).padStart(2, '0')
          : master
          ? 'OUT'
          : channel.groupHead
          ? 'GRP'
          : channel.type === 'group'
          ? 'G'
          : 'R';
    const group = channel.group, groupColor = group && /^#[0-9a-f]{3,8}$/i.test(group.color || '') ? group.color : null;
    return `<section class="studio-channel studio-routed-channel ${master ? 'studio-master' : ''} ${channel.groupHead ? 'group-strip' : group ? 'group-member-strip' : ''} ${selected ? 'is-selected' : ''}"`
        + ` style="--track-color:${/^#[0-9a-f]{3,8}$/i.test(entity.color || '') ? entity.color : groupColor || (master ? 'var(--studio-accent)' : '#92998f')}${groupColor ? `;--group:${groupColor}` : ''}"`
        + ` data-mixer-owner="${esc(channel.owner)}" data-mixer-channel="${channel.token}" aria-label="${esc(channel.name)} ${channel.type} channel">
      <button class="studio-channel-title" id="${channelControlId('track', channel)}" tabindex="${tab}" data-select-owner="${esc(channel.owner)}"`
          + (` data-mixer-role="track" aria-pressed="${selected}"><span class="studio-channel-number">${prefix}</span>`
          + `<strong>${esc(channel.name)}</strong></button>${track || channel.groupHead
              ? window.STTrackLanes?.stripDeleteButton(channel.owner, channel.name, track ? 'track' : 'group', tab) || '' : ''}
      <div class="studio-track-state">${master ? '<span class="studio-track-type">Master · stereo</span>' : `<button class="btn studio-small ${entity.mute ? 'is-active' : ''}" id="${channelControlId('mute', channel)}" tabindex="${tab}"`
          + ` data-toggle-owner="${esc(channel.owner)}" data-mixer-role="mute" data-toggle-property="mute" aria-pressed="${!!entity.mute}"`
          + ` aria-label="Mute ${esc(channel.name)}">M</button><button class="btn studio-small ${entity.solo ? 'is-active' : ''}" id="${channelControlId('solo', channel)}"`
          + ` tabindex="${tab}" data-toggle-owner="${esc(channel.owner)}" data-mixer-role="solo" data-toggle-property="solo" aria-pressed="${!!entity.solo}"`
          + ` aria-label="Solo ${esc(channel.name)}">S</button><span class="studio-track-type">${track
              ? esc(project().instruments[entity.instrument || 0]?.type || 'sample')
              : channel.type === 'group'
              ? 'Group'
              : 'Return'}</span>`}</div>
      <div class="studio-fader-well"><div class="studio-meter" aria-hidden="true"><div ${master
          ? 'data-master-meter'
          : track
          ? `data-track-meter="${channel.track}"`
          : `data-bus-meter="${esc(channel.owner)}"`}`)
          + ` style="transform:scaleY(${clamp(peak,0,1)})"></div></div><label for="${channelControlId('vol',channel)}" class="studio-volume">`
          + `<output id="${channelControlId('vol',channel)}-out">${db(level)}</output><input id="${channelControlId('vol',channel)}" tabindex="${tab}"`
          + ` type="range" min="0" max="${track || master ? 1.5 : 2}" step=".01" value="${level}" data-studio-property="${master ? 'masterVolume' : 'volume'}"`
          + ` data-studio-owner="${esc(channel.owner)}" data-mixer-role="vol" ${automateAttributes(channel.owner,'volume',master ? 'Master volume' : 'Channel volume')}`
          + ` aria-label="${esc(channel.name)} volume" aria-valuetext="${db(level)}"></label></div>
      ${master
          ? '<p class="studio-hint">Stereo output<br>Final output level</p>'
          : routedRange(channel,'pan','Pan',number(entity.pan),'pan',1,-1,pan)}
      ${track
          ? `<div class="studio-send-label">Built-in sends</div>${routedRange(channel,'delay','Delay',number(entity.delaySend),'delaySend')}${routedRange(channel,'reverb','Reverb',number(entity.reverbSend),'reverbSend')}`
          : '<div class="studio-routed-spacer"></div>'}
      ${master ? '' : `<button class="studio-output-summary" id="${channelControlId('output',channel)}" tabindex="${tab}" data-route-output="${esc(channel.owner)}"`
          + ` data-mixer-role="output" ${window.STRouting?.canOutput?.(channel.owner) ? '' : 'disabled'} title="Change output">`
          + `<span>Output</span><strong>${esc(outputName)}</strong></button><button class="studio-send-summary" id="${channelControlId('sends',channel)}"`
          + ` tabindex="${tab}" data-route-sends="${esc(channel.owner)}" data-mixer-role="sends" ${window.STRouting?.canSend?.(channel.owner) ? '' : 'disabled'}>${sendCount ? `${sendCount} custom send${sendCount === 1 ? '' : 's'}` : '+ Custom send'}</button>`}
      ${channel.groupHead ? (() => { const count = window.STGroups.members(project(), group).length;
          return `<button class="group-strip-expand" id="${channelControlId('expand', channel)}" data-group-mixer-expand="${esc(group.id)}" tabindex="${tab}" aria-expanded="${!group.mixerCollapsed}">${group.mixerCollapsed ? 'Show' : 'Hide'} ${count} track${count === 1 ? '' : 's'}</button>`; })() : ''}
      <button `
          + `class="studio-slot-summary" id="${channelControlId('slots',channel)}" tabindex="${tab}" data-select-owner="${esc(channel.owner)}"`
          + ` data-mixer-role="slots">${effectsFor(channel.owner).length
              ? `${effectsFor(channel.owner).length} insert effect${effectsFor(channel.owner).length === 1 ? '' : 's'}`
              : '+ Insert effect'}</button>
    </section>`;
  }
  function renderRoutedMixer() {
    const p = project(),
      owner = selectedOwner(),
      effects = effectsFor(owner),
      strips = visibleChannels(owner),
      delay = { time: .375, feedback: .35, mix: .3, ...p.delay },
      reverb = { size: .7, damping: .4, mix: .25, ...p.reverb };
    return `<div class="studio-mixer studio-routed-mixer" id="mixer-workspace" data-scroll="mixer-workspace"><div class="panel-head">`
        + `<div>${mixerHeading()}<p class="muted">← → channel · ↑ ↓ adjust · Tab controls · ⌥A automate</p></div><div class="studio-mixer-actions">${window.STRouting?.capabilities
            ? '<button class="btn" data-studio-action="new-group">+ Group</button><button class="btn" data-studio-action="new-return">+ Return</button>'
            : ''}<button `
        + `class="btn" data-studio-action="panic">Silence all</button></div></div>
    <div class="studio-channels${mixerVirtual(strips.length) ? ' is-virtual' : ''}" id="mixer-channels" data-scroll="mixer-channels"`
        + ` tabindex="0" aria-label="Mixer channels. Left and right select tracks, groups, returns and Master.">${mixerStrips(strips.length,
            strips.findIndex(c => c.owner === owner), i => renderRoutedStrip(strips[i]))}</div>
    <div class="studio-bottom studio-bottom-routed"><section class="panel studio-rack ${effects.length > 8 ? 'is-long' : ''}">`
        + renderRack(owner, effects) + `</section>`
        + `<div class="studio-side"><section class="studio-routing-panel" aria-label="Selected channel routing">${window.STRouting?.render(owner) || ''}</section>
    <section class="panel studio-returns studio-fixed-returns"><div class="panel-head"><h3>Built-in returns</h3>`
        + `</div><div class="studio-return-pair"><div><h4>Delay</h4>${range({id:'delay-time',label:'Time',value:delay.time,min:.01,max:2,step:.005,format:v=>`${Math.round(v*1000)} ms`,group:'delay',property:'time'})}${range({id:'delay-feedback',label:'Feedback',value:delay.feedback,max:.95,group:'delay',property:'feedback'})}${range({id:'delay-mix',label:'Return',value:delay.mix,group:'delay',property:'mix'})}</div>`
        + `<div><h4>Reverb</h4>${range({id:'reverb-size',label:'Room size',value:reverb.size,group:'reverb',property:'size'})}${range({id:'reverb-damping',label:'Damping',value:reverb.damping,group:'reverb',property:'damping'})}${range({id:'reverb-mix',label:'Return',value:reverb.mix,group:'reverb',property:'mix'})}</div>`
        + `</div></section></div></div></div>`;
  }

  /** The selected channel's rack: its insert chain (insert-chain.js), the plug-in search while it is open, and each
      insert's details (sidechain inputs, factory presets, editor, every parameter) folded away underneath. */
  function renderRack(owner, effects) {
    return window.STInsertChain.render(owner, { prefix: 'mixfx', search: true, title: channelTitle(owner) }) + (ui.pluginPicker ? renderPluginPicker() : '')
        + (effects.length ? `<details class="studio-rack-details" ${ui.rackDetails ? 'open' : ''}><summary><span>Plug-in details</span><span class="muted">sidechain inputs · presets · editor · every parameter</span>`
        + `<span class="studio-reorder-hint" role="status">${ui.pluginMoving ? 'Moving effect…' : '⌥ ↑ / ↓ reorder'}</span></summary>${effects.map(renderPlugin).join('')}</details>` : '');
  }
  function renderPlugin(plugin) {
    const paramKey = pluginKey(ownerForPlugin(plugin), plugin.slot, plugin.kind),
      params = ui.parameters.get(paramKey) || plugin.parameters || [],
      paramError = ui.parameterErrors.get(paramKey);
    const pluginOwner = ownerForPlugin(plugin), chain = effectsFor(pluginOwner), position = chain.findIndex(item => item.slot === plugin.slot);
    return `<details class="studio-plugin ${plugin.bypass ? 'is-bypassed' : ''}" data-plugin-owner="${esc(pluginOwner)}"`
        + ` data-plugin-slot="${plugin.slot}" ${(ui.openPlugins.get(pluginKey(pluginOwner, plugin.slot)) ?? plugin.slot === 0) ? 'open' : ''}>`
        + `<summary id="plugin-header-${plugin.slot}" aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown" title="Option Up / Down moves this effect in the `
        + `chain"><span class="studio-slot-index">${String(plugin.slot + 1).padStart(2, '0')}</span><strong>${esc(plugin.name || 'plug-in')}${plugin.bypass ? ' · bypassed' : ''}</strong>`
        + `<span class="muted">${esc(plugin.manufacturer || '')}</span></summary><div class="studio-plugin-body">${window.STRouting?.renderPluginSidechain(plugin) || ''}<div `
        + `class="studio-plugin-actions"><button class="btn studio-small" id="plugin-editor-${plugin.slot}" data-plugin-action="editor"`
        + ` data-slot="${plugin.slot}" ${ui.pluginMoving ? 'disabled' : ''}>Open editor ↗</button><button class="btn studio-small ${plugin.bypass ? 'is-active' : ''}"`
        + ` id="plugin-bypass-${plugin.slot}" data-plugin-action="bypass" data-slot="${plugin.slot}" ${ui.pluginMoving ? 'disabled' : ''} aria-pressed="${!!plugin.bypass}">Bypass</button>`
        + `<button class="btn studio-small" id="plugin-refresh-${plugin.slot}" data-plugin-action="refresh" data-slot="${plugin.slot}"`
        + ` ${ui.pluginMoving ? 'disabled' : ''}>Refresh parameters</button><button class="btn studio-small studio-danger" id="plugin-remove-${plugin.slot}"`
        + ` data-plugin-action="remove" data-slot="${plugin.slot}" ${ui.pluginMoving ? 'disabled' : ''}>Remove</button>`
        + `<span class="studio-plugin-reorder"><button class="btn studio-small" id="plugin-move-up-${plugin.slot}" data-plugin-move="-1"`
        + ` data-slot="${plugin.slot}" aria-label="Move ${esc(plugin.name || 'effect')} earlier" ${ui.pluginMoving || position <= 0 ? 'disabled' : ''}>↑ `
        + `Move up</button><button class="btn studio-small" id="plugin-move-down-${plugin.slot}" data-plugin-move="1" data-slot="${plugin.slot}"`
        + ` aria-label="Move ${esc(plugin.name || 'effect')} later" ${ui.pluginMoving || position === chain.length - 1 ? 'disabled' : ''}>↓ Move down</button>`
        + `</span></div>${plugin.presets?.length ? `<label class="studio-plugin-preset">Factory preset<select class="field" id="plugin-preset-${plugin.slot}" data-plugin-preset="${plugin.slot}"`
        + ` ${ui.pluginMoving ? 'disabled' : ''} aria-label="${esc(plugin.name)} factory preset"><option value="">${plugin.currentPreset ? 'Choose preset…' : 'Custom state'}</option>${plugin.presets.map(preset => `<option value="${preset.number}" ${plugin.currentPreset?.number === preset.number ? 'selected' : ''}>${esc(preset.name)}</option>`).join('')}</select>`
        + `</label>` : ''}${paramError ? `<p class="studio-plugin-error" role="alert">${esc(paramError)} <span class="muted">Refresh parameters retries.</span>`
        + `</p>` : ''}${params.length ? `<div class="studio-plugin-params" data-scroll="plugin-params-${plugin.slot}">${params.map((param, i) => `<label class="studio-knob" for="plugin-${plugin.slot}-${i}"><span title="${esc(param.name)}">${esc(param.name || param.id)}</span>`
        + `<output id="plugin-${plugin.slot}-${i}-out">${esc(param.display || `${(number(param.normalized) * 100).toFixed(1)}%`)}</output>`
        + `<input type="range" id="plugin-${plugin.slot}-${i}" min="0" max="1" step="${window.STFine?.parameterStep(param) || .001}"`
        + ` data-parameter-steps="${window.STFine?.parameterSteps(param) || 0}" value="${number(param.normalized)}" data-plugin-address="${esc(param.address)}"`
        + ` data-slot="${plugin.slot}" ${param.automatable !== false && param.writable !== false
            ? automateAttributes(pluginOwner, effectTarget(plugin.slot, param.address), param.name)
            : ''}`
        + ` ${param.writable === false || ui.pluginMoving ? 'disabled' : ''} aria-label="${esc(param.name)}"></label>`).join('')}</div>` : paramError ? '' : ui.pending.has(paramKey) ? '<p class="muted studio-hint">Reading parameters…</p>' : '<p class="muted studio-hint">Open the editor for the plug-in interface, or refresh to load exposed parameters.</p>'}</div></details>`;
  }

  function renderPluginPicker() {
    const query = ui.pluginQuery.toLowerCase();
    const available = (ui.plugins || state().plugins || []).filter(p => p.type === 'effect'
        && `${p.name} ${p.manufacturer}`.toLowerCase().includes(query));
    return `<div class="studio-plugin-picker" role="region" aria-label="Choose plug-in effect"><div class="studio-picker-search">`
        + `<input type="search" class="field" id="studio-plugin-search" placeholder="Search plug-in effects…" value="${esc(ui.pluginQuery)}"`
        + ` aria-label="Search plug-in effects"><button class="btn studio-small" data-studio-action="close-plugin-picker">Close</button>`
        + `</div><div class="studio-picker-results">${ui.pluginBusy ? '<p class="muted">Loading plug-ins…</p>' : available.length ? available.map(p => `<button class="studio-plugin-choice" data-add-plugin="${esc(p.id)}" data-plugin-bundle-path="${esc(p.bundlePath || '')}">`
        + `<strong>${esc(p.name)}</strong><span>${esc(p.format + (p.manufacturer ? ' · ' + p.manufacturer : ''))}</span>`
        + `<span>＋</span></button>`).join('') : '<p class="muted">No matching plug-in effects. Installed system and third-party plug-ins appear here.</p>'}</div></div>`;
  }

  function formatMixerValue(input) {
    const value = Number(input.value);
    if (input.dataset.studioProperty === 'volume' || input.dataset.studioProperty === 'masterVolume') return db(value);
    if (input.dataset.studioProperty === 'pan') return pan(value);
    if (input.dataset.studioGroup === 'delay' && input.dataset.studioProperty === 'time') return `${Math.round(value * 1000)} ms`;
    return gain(value);
  }

  async function ensureParameters(track, force = false) {
    if (ui.pluginMoving || window.STBusy.isBusy('routing')) return;
    const owner = project(), generation = ui.pluginGeneration;
    let changed = false;
    for (const plugin of pluginsFor(track)) {
      if (generation !== ui.pluginGeneration || project() !== owner) break;
      const key = pluginKey(track, plugin.slot, plugin.kind);
      if ((!force && ui.parameters.has(key) && ui.parameterOwners.get(key) === plugin) || ui.pending.has(key)) continue;
      ui.pending.add(key);
      try {
        const result = await S().request('plugin.parameters', { ...ownerPayload(track), kind: plugin.kind, slot: plugin.slot });
        if (generation !== ui.pluginGeneration || project() !== owner) continue;
        if (!pluginsFor(track).includes(plugin)) { changed = true; continue; }
        const list = Array.isArray(result) ? result : result.parameters || [];
        ui.parameters.set(key, list); ui.parameterOwners.set(key, plugin); ui.parameterErrors.delete(key);
        changed = true;
      } catch (error) {
        // Show why an insert has no controls instead of silently leaving the hint.
        const message = error?.message || String(error);
        if (generation === ui.pluginGeneration && project() === owner && ui.parameterErrors.get(key) !== message) {
          ui.parameterErrors.set(key, message);
          changed = true;
        }
        if (force) notifyError(error);
      }
      finally { ui.pending.delete(key); }
    }
    if ((changed || generation !== ui.pluginGeneration)
        && !ui.pluginMoving
        && (state().view === 'mixer' || state().view === 'automation')) S().refresh();
  }

  const pluginContext = owner => {
    const p = project(),
      name = normalizeOwner(owner),
      entity = channelFor(name)?.entity;
    return { project: p, owner: name, entity, valid: () => project() === p && channelFor(name)?.entity === entity };
  };
  async function rememberPlugin(result, label, context = pluginContext(ownerForPlugin(result))) {
    if (ui.pluginMoving || !context.valid()) throw new Error('Plug-in edit was canceled because its channel changed.');
    // Never turn a failed opaque-state capture into a successful edit.
    const snapshot = await S().request('plugin.state', { ...ownerPayload(context.owner), kind: result.kind, slot: result.slot });
    if (ui.pluginMoving || !context.valid()) throw new Error('Plug-in edit was canceled because its channel changed.');
    mutate(label, p => {
      const i = p.audioPlugins.findIndex(item => ownerForPlugin(item) === context.owner && item.kind === result.kind && item.slot === result.slot);
      // The parameter list is a UI cache (ui.parameters); captured host descriptors carry none, so keeping one
      // here would make the next capture look like an edit of this insert.
      const next = i >= 0 ? { ...p.audioPlugins[i], ...result, ...snapshot } : { ...result, ...snapshot };
      if (result.parameters === undefined) delete next.parameters;
      if (i >= 0) p.audioPlugins[i] = next; else p.audioPlugins.push(next);
    });
  }

  const savedEffects = (p, owner) => p.audioPlugins.filter(item => ownerForPlugin(item) === normalizeOwner(owner, p)
      && item.kind === 'effect').sort((a, b) => a.slot - b.slot);
  const replaceEffects = (p, owner, effects) => {
    p.audioPlugins = [...p.audioPlugins.filter(item => !(ownerForPlugin(item) === normalizeOwner(owner, p) && item.kind === 'effect')),
        ...effects];
  };
  async function moveEffect(slot, direction) {
    if (direction !== -1 && direction !== 1) return false;
    const track = selectedOwner(), chain = savedEffects(project(), track), index = chain.findIndex(item => Number(item.slot) === Number(slot));
    const destination = chain[index + direction]; if (index < 0 || !destination) return false;
    return moveInsert(track, Number(slot), Number(destination.slot), { focus: true });
  }
  /** Moves the insert in fromSlot to the chain position of toSlot (the others shift), as one Undo step. */
  async function moveInsert(track, fromSlot, toSlot, { focus = false, label = 'Reorder insert effect' } = {}) {
    if (ui.pluginMoving || window.STBusy.refuse('move', ['routing'])) return false;
    if (ui.pluginEditing) { S().toast('Finish the current plug-in edit before moving an effect.'); return false; }
    track = normalizeOwner(track);
    const owner = project(), trackObject = channelFor(track)?.entity;
    const original = savedEffects(owner, track), index = original.findIndex(item => Number(item.slot) === Number(fromSlot));
    const toIndex = original.findIndex(item => Number(item.slot) === Number(toSlot));
    if (index < 0 || toIndex < 0 || index === toIndex) return false;
    fromSlot = Number(fromSlot); toSlot = Number(toSlot);
    const topology = effects => effects.map(item => [Number(item.slot), item.component, item.bundlePath || '', item.instanceID]);
    const originalTopology = topology(original);
    const current = () => project() === owner && channelFor(track, owner)?.entity === trackObject;
    let sent = false, committed = false, signature = '', focusSlot = fromSlot;
    const originFocus = document.activeElement?.id || '';
    clearTimeout(ui.previewTimer); ui.previewTimer = null; ui.previewSerial++;
    ui.pluginMoving = true; ui.pluginGeneration++;
    for (const timer of ui.parameterTimers.values()) clearTimeout(timer); ui.parameterTimers.clear();
    S().refresh();
    try {
      if (await S().captureNativeEdits?.() === false) return;
      if (!current()) return;
      const captured = topology(savedEffects(owner, track));
      if (captured.length !== originalTopology.length
          || captured.some((item, i) => item[0] !== originalTopology[i][0]
              || item[1] !== originalTopology[i][1]
              || item[2] !== originalTopology[i][2]
              || originalTopology[i][3]
                  && item[3] !== originalTopology[i][3])) throw new Error('Effect move canceled because its insert chain changed.');
      // The chain's topology, not its opaque states: the host republishes descriptors (plugins.changed)
      // with freshly captured state while the pre-move sync runs, which is no reason to cancel a reorder.
      const chainSignature = () => JSON.stringify(topology(savedEffects(owner, track)));
      signature = chainSignature();
      const synchronized = await S().syncEngineProject(() => current() && chainSignature() === signature
          ? window.STUtil.clone(owner)
          : null);
      if (synchronized === false
          || !current()
          || chainSignature() !== signature) throw new Error('Effect move canceled because its project or insert chain changed.');
      let complete; const promise = new Promise(resolve => { complete = resolve; });
      ui.effectMoveDone = { promise, complete };
      sent = true;
      const result = await S().request('plugin.move', { ...ownerPayload(track), fromSlot, toSlot });
      if (!current()
          || chainSignature() !== signature) throw new Error('Effect move canceled because its project or insert chain changed.');
      if (!result
          || !Array.isArray(result.plugins)
          || !Array.isArray(result.previousPlugins)
          || !Array.isArray(result.slotMap)) throw new Error('The host returned an incomplete effect order.');
      const before = result.previousPlugins, after = result.plugins, baseline = savedEffects(owner, track);
      const slots = baseline.map(item => Number(item.slot));
      const order = slots.slice(), moved = order.splice(index, 1)[0]; order.splice(toIndex, 0, moved);
      const expected = new Map(order.map((oldSlot, i) => [oldSlot, slots[i]]));
      const map = new Map(result.slotMap.map(item => [Number(item.fromSlot), Number(item.toSlot)]));
      const validDescriptor = item => ownerForPlugin(item) === track
          && item.kind === 'effect'
          && Number.isInteger(Number(item.slot))
          && Number(item.slot) >= 0
          && Number(item.slot) < INSERT_SLOTS
          && typeof item.instanceID === 'string'
          && item.instanceID.length > 0;
      const sameInstallation = (a, b) => a.component === b.component
          && (a.bundlePath || '') === (b.bundlePath || '')
          && a.instanceID === b.instanceID;
      const valid = result.moved === true && ownerForPlugin(result) === track && Number(result.fromSlot) === fromSlot && Number(result.toSlot) === toSlot &&
        before.length === baseline.length && after.length === before.length && result.slotMap.length === before.length && map.size === before.length &&
        new Set(before.map(item => Number(item.slot))).size === before.length && new Set(after.map(item => Number(item.slot))).size === after.length && new Set(before.map(item => item.instanceID)).size === before.length &&
        before.every(validDescriptor) && after.every(validDescriptor) &&
        baseline.every(item => before.some(prior => Number(prior.slot) === Number(item.slot) && sameInstallation(item, prior))) &&
        before.every(item => map.get(Number(item.slot)) === expected.get(Number(item.slot))
            && after.some(next => Number(next.slot) === map.get(Number(item.slot)) && sameInstallation(item, next))) &&
        result.slotMap.every(item => before.some(prior => Number(prior.slot) === Number(item.fromSlot) && item.instanceID === prior.instanceID));
      if (!valid) throw new Error('The host returned a mismatched effect chain.');
      // Native snapshots supply stable instance IDs and the exact pre-move opaque state for Undo.
      replaceEffects(owner, track, before);
      const open = new Map();
      for (const item of before) {
        const oldKey = pluginKey(track, item.slot);
        open.set(pluginKey(track, map.get(Number(item.slot)) ?? item.slot), ui.openPlugins.get(oldKey) ?? item.slot === 0);
        ui.openPlugins.delete(oldKey);
        ui.parameters.delete(oldKey);
        ui.parameterOwners.delete(oldKey);
      }
      for (const [key, value] of open) ui.openPlugins.set(key, value);
      for (const item of after) if (item.parameters) {
        const key = pluginKey(track, item.slot);
        ui.parameters.set(key, item.parameters);
        ui.parameterOwners.set(key, item);
      }
      mutate(label, p => {
        replaceEffects(p, track, after);
        for (const pattern of p.patterns) for (const lane of pattern.automation) {
          if (lane.owner !== track) continue;
          const match = /^plugin:effect:(\d+):(.+)$/.exec(lane.target || '');
          if (match && map.has(Number(match[1]))) lane.target = effectTarget(map.get(Number(match[1])), match[2]);
        }
      });
      focusSlot = toSlot; committed = true;
    } catch (error) {
      if (sent && !committed) {
        // The rollback must not wait on the move promise owned by this caller.
        // Keep pluginMoving set so further rack edits remain unavailable.
        ui.effectMoveDone?.complete(); ui.effectMoveDone = null;
        try { if (await S().syncEngineProject(() => window.STUtil.clone(project()),
            { replace: true }) === false) throw new Error('Audio restoration was canceled because the session changed again.'); }
        catch (restoreError) { notifyError(restoreError); }
      }
      notifyError(error);
    } finally {
      ui.pluginMoving = false;
      ui.effectMoveDone?.complete(); ui.effectMoveDone = null;
      const followFocus = (document.activeElement?.id || '') === originFocus;
      if (state().view === 'mixer') {
        S().refresh();
        if (focus && followFocus && current() && selectedOwner() === track) {
          const header = document.getElementById(`plugin-header-${focusSlot}`);
          header?.focus();
          header?.scrollIntoView?.({ block: 'nearest' });
        }
      } else if (state().view === 'instrument') S().refresh();
    }
    return committed;
  }

  // ---- Insert slot API (window.STInserts): add, remove, bypass, move and duplicate by owner + slot -----------
  // Shared by the mixer rack, the insert chain (insert-chain.js) and the MK3 PLUG-IN page.
  const insertsIdle = () => !(ui.pluginMoving || ui.pluginEditing || window.STBusy.isBusy('routing'));
  function freeSlot(track) {
    const used = new Set(effectsFor(track).map(item => Number(item.slot)));
    let slot = 0; while (used.has(slot)) slot++;
    return slot < INSERT_SLOTS ? slot : -1;
  }
  const insertAt = (track, slot) => effectsFor(normalizeOwner(track)).find(item => Number(item.slot) === Number(slot)) || null;
  function refuseInsertEdit() {
    if (insertsIdle()) return false;
    S().toast(ui.pluginMoving ? 'Wait for the insert move to finish.'
        : ui.pluginEditing ? `A plug-in change is still in progress (${ui.pluginEditing}). Wait a moment; if this persists, save and restart SuperTracker.`
        : 'The audio routing is still being applied. Try again in a moment.');
    return true;
  }
  /** Loads an effect into the first free insert slot of an owner (or the given free slot); resolves to its descriptor (null when refused). */
  async function addInsert(track, component, { bundlePath = '', state = '', name = '', bypass = undefined, label = 'Add plug-in effect', slot: requested } = {}) {
    track = normalizeOwner(track);
    if (refuseInsertEdit()) return null;
    const slot = requested === undefined ? freeSlot(track) : Number(requested), context = pluginContext(track);
    if (requested !== undefined && (!Number.isInteger(slot) || slot < 0 || slot >= INSERT_SLOTS || insertAt(track, slot))) throw new Error(`Insert slot ${requested} is not a free slot 0–${INSERT_SLOTS - 1}.`);
    if (slot < 0) { S().toast(`This channel already holds ${INSERT_SLOTS} insert effects.`); return null; }
    ui.pluginEditing++;
    try {
      // Publish every committed edit first: a debounced engine update sent while the load is in flight would
      // describe the chain without the new insert and remove it again.
      if (S().syncEngineProject) {
        S().cancelPendingEngineSync?.();
        if (await S().syncEngineProject(() => context.valid() ? window.STUtil.clone(project()) : null) === false || !context.valid()) return null;
      }
      const target = { ...ownerPayload(track), kind: 'effect', slot, component, ...(bundlePath ? { bundlePath } : {}), ...(state ? { state } : {}),
        ...(name ? { name } : {}), ...(bypass !== undefined ? { bypass: !!bypass } : {}) };
      const result = await S().request('plugin.load', target);
      if (!context.valid()) throw new Error('Plug-in load was canceled because its channel changed.');
      ui.openPlugins.set(pluginKey(track, slot), true);
      ui.parameters.set(pluginKey(track, slot), result.parameters || []); ui.pluginPicker = false;
      // The parameter list stays a UI cache (ui.parameters): the host's own descriptors carry none, and keeping one
      // here would make the next host update look like an edit of this insert.
      await rememberPlugin({ ...result, parameters: undefined, ...ownerPayload(track), kind: 'effect', slot, ...(bypass !== undefined ? { bypass: !!bypass } : {}) }, label, context);
      return insertAt(track, slot);
    } finally { ui.pluginEditing--; }
  }
  /** Bypass on / off (undefined toggles); one Undo step. Resolves to the new state. */
  async function bypassInsert(track, slot, bypass, context = pluginContext(normalizeOwner(track)), { label = 'Bypass plug-in effect' } = {}) {
    track = normalizeOwner(track); slot = Number(slot);
    if (refuseInsertEdit()) return null;
    const plugin = insertAt(track, slot); if (!plugin) throw new Error('This insert effect is no longer present.');
    const on = bypass === undefined ? !plugin.bypass : !!bypass;
    ui.pluginEditing++;
    try {
      const result = await S().request('plugin.parameter', { ...ownerPayload(track), kind: 'effect', slot, bypass: on });
      await rememberPlugin({ ...result, parameters: undefined, bypass: on }, label, context);
      return on;
    } finally { ui.pluginEditing--; }
  }
  /** Removes an insert with its automation lanes and sidechain routes; one Undo step restores all of it. */
  async function removeInsert(track, slot, context = pluginContext(normalizeOwner(track)), { label = 'Remove plug-in effect' } = {}) {
    track = normalizeOwner(track); slot = Number(slot);
    if (refuseInsertEdit()) return false;
    const payload = { ...ownerPayload(track), kind: 'effect', slot };
    ui.pluginEditing++;
    try {
      if (await S().captureNativeEdits?.() === false) return false;
      if (!context.valid()) throw new Error('Plug-in removal was canceled because its channel changed.');
      const removed = insertAt(track, slot);
      if (!removed) throw new Error('This insert effect is no longer present.');
      await S().request('plugin.remove', payload);
      if (!context.valid()) throw new Error('Plug-in removal was canceled because its channel changed.');
      const current = insertAt(track, slot);
      if (current?.instanceID !== removed.instanceID) throw new Error('The insert changed while removal was pending.');
      ui.parameters.delete(pluginKey(track, slot));
      mutate(label, p => {
        p.audioPlugins = p.audioPlugins.filter(item => !(ownerForPlugin(item) === track && item.kind === 'effect' && item.slot === slot));
        p.routing.sidechains = p.routing.sidechains.filter(route => route.destination !== removed.instanceID);
        delete p.pluginPages[removed.instanceID];
        for (const pattern of p.patterns) pattern.automation = pattern.automation.filter(lane => !(lane.owner === track && lane.target?.startsWith(`plugin:effect:${slot}:`)));
      });
      window.STRouting?.refreshCapabilities();
      return true;
    } finally { ui.pluginEditing--; }
  }
  /** Copies an insert (its exact state and bypass) to the slot right after it; one Undo step. */
  async function duplicateInsert(track, slot) {
    track = normalizeOwner(track); slot = Number(slot);
    if (refuseInsertEdit()) return null;
    const source = insertAt(track, slot); if (!source) throw new Error('This insert effect is no longer present.');
    if (freeSlot(track) < 0) { S().toast(`This channel already holds ${INSERT_SLOTS} insert effects.`); return null; }
    if (await S().captureNativeEdits?.() === false) return null;
    const snapshot = await S().request('plugin.state', { ...ownerPayload(track), kind: 'effect', slot, instanceID: source.instanceID });
    if (snapshot?.stateError) throw new Error(String(snapshot.stateError));
    const history = state().undo, top = history?.at(-1);
    const copy = await addInsert(track, source.component, { bundlePath: source.bundlePath || '', state: snapshot?.state || source.state || '', name: source.name || '',
      bypass: !!source.bypass, label: 'Duplicate insert effect' });
    if (!copy) return null;
    const chain = effectsFor(track), at = chain.findIndex(item => item.slot === slot), next = chain[at + 1];
    if (next && next.slot !== copy.slot) {
      // The add and the move are one Undo step: drop the move's entry when it sits right on the add's (by identity,
      // not by position: a full history drops its oldest entry on every push).
      const addEntry = history?.at(-1) !== top ? history.at(-1) : null;
      if (await moveInsert(track, copy.slot, next.slot, { label: 'Duplicate insert effect' }) && addEntry && history.at(-1) !== addEntry && history.at(-2) === addEntry) history.pop();
    }
    return effectsFor(track).find(item => item.instanceID === copy.instanceID) || copy;
  }
  /** The owner's insert effects in chain order (descriptors from project.audioPlugins). */
  const insertChain = track => effectsFor(normalizeOwner(track));

  function setMixerValue(p, input, value) {
    const key = input.dataset.studioProperty, group = input.dataset.studioGroup, owner = input.dataset.studioOwner, track = input.dataset.studioTrack;
    if (group) { p[group] ||= {}; p[group][key] = value; }
    else if (owner !== undefined) { const channel = channelFor(owner,p); if (channel) channel.entity[key] = value; }
    else if (track !== undefined) p.tracks[Number(track)][key] = value;
    else p[key] = value;
  }
  function bindMixer() {
    bindMixerViewport();
    window.STMixerSplitter?.bind();
    document.querySelectorAll('[data-group-mixer-expand]').forEach(button => button.onclick = () => {
      const group = window.STGroups.byID(project(), button.dataset.groupMixerExpand);
      if (group) window.STApp?.setViewPreference?.(group.id, 'mixerCollapsed', !group.mixerCollapsed);
      S().refresh();
      focusMixerControl(button.id); // the strip is re-rendered: keep the keyboard on Show / Hide
    });
    document.getElementById('mixer-workspace')?.addEventListener('focusin', event => updateMixerIdentity(event.target));
    window.STRouting?.bind(selectedOwner()); window.STRouting?.bindPluginSidechains(selectedOwner());
    document.querySelectorAll('[data-select-owner]').forEach(button => button.addEventListener('click', () => {
      selectOwner(button.dataset.selectOwner);
      S().refresh();
      focusMixerControl(button.id);
    }));
    document.querySelectorAll('[data-toggle-owner]').forEach(button => button.addEventListener('click', () => {
      const owner = button.dataset.toggleOwner,
        property = button.dataset.toggleProperty;
      mutate(`${property === 'mute' ? 'Mute' : 'Solo'} channel`, p => {
        const channel = channelFor(owner, p);
        if (channel && channel.type !== 'master') channel.entity[property] = !channel.entity[property];
      }, button.id);
    }));
    document.querySelectorAll('[data-route-output]').forEach(button => button.addEventListener('click', () => {
      selectOwner(button.dataset.routeOutput);
      S().refresh();
      window.STRouting?.showOutput?.(selectedOwner(), button.id);
    }));
    document.querySelectorAll('[data-route-sends]').forEach(button => button.addEventListener('click', () => {
      selectOwner(button.dataset.routeSends);
      S().refresh();
      window.STRouting?.showAddSend?.(selectedOwner(), button.id);
    }));
    document.querySelector('.studio-rack-details')?.addEventListener('toggle', event => { ui.rackDetails = !!event.target?.open; });
    window.STInsertChain.bind(document, selectedOwner(), { prefix: 'mixfx', search: true, title: channelTitle(selectedOwner()) });
    document.querySelectorAll('.studio-plugin').forEach(details => details.addEventListener('toggle', () => {
      ui.openPlugins.set(pluginKey(details.dataset.pluginOwner, Number(details.dataset.pluginSlot)),
          details.open);
    }));
    document.querySelectorAll('[data-studio-property]').forEach(input => {
      input.addEventListener('input', () => {
        const text = formatMixerValue(input);
        document.getElementById(`${input.id}-out`).textContent = text;
        input.setAttribute('aria-valuetext', text);
        clearTimeout(ui.previewTimer);
        const owner = project(), serial = ++ui.previewSerial;
        ui.previewTimer = setTimeout(async () => {
          if (ui.effectMoveDone) await ui.effectMoveDone.promise;
          if (window.STBusy.isBusy('routing')) await window.STRouting.waitForApply();
          if (serial !== ui.previewSerial || project() !== owner || input.isConnected === false) return;
          await S().syncEngineProject(() => {
            if (serial !== ui.previewSerial || project() !== owner || input.isConnected === false) return null;
            const preview = window.STUtil.clone(owner);
            setMixerValue(preview, input, Number(input.value)); return preview;
          }).catch(notifyError);
        }, 70);
      });
      input.addEventListener('change', () => {
        clearTimeout(ui.previewTimer); ui.previewSerial++;
        const key = input.dataset.studioProperty;
        mutate(`Adjust ${key}`, p => setMixerValue(p, input, Number(input.value)), input.id);
      });
    });
    document.querySelectorAll('[data-studio-action]').forEach(button => button.addEventListener('click', async () => {
      const action = button.dataset.studioAction;
      if (action === 'new-group' || action === 'new-return') { window.STRouting?.createBus(action === 'new-group' ? 'group' : 'return',
          {assignOwner:selectedOwner()}); return; }
      if (action === 'panic') { await S().request('engine.panic', {}).catch(notifyError); return; }
      if (action === 'close-plugin-picker') { ui.pluginPicker = false; S().refresh(); return; }
      if (action === 'add-plugin') {
        if (ui.pluginMoving || window.STBusy.isBusy('routing')) return;
        ui.pluginPicker = true; ui.pluginBusy = !ui.plugins; S().refresh(); focusId('studio-plugin-search');
        if (!ui.plugins) {
          try { const response = await S().request('plugins.list', {}); ui.plugins = Array.isArray(response) ? response : response.plugins || []; }
          catch (error) { notifyError(error); }
          ui.pluginBusy = false; S().refresh(); focusId('studio-plugin-search');
        }
      }
    }));
    document.getElementById('studio-plugin-search')?.addEventListener('keydown', event => {
      if (!event.metaKey && !event.ctrlKey && !event.altKey && event.key === 'ArrowDown') {
        event.preventDefault();
        document.querySelector('[data-add-plugin]')?.focus();
      }
    });
    document.getElementById('studio-plugin-search')?.addEventListener('input', event => {
      const start = event.target.selectionStart;
      ui.pluginQuery = event.target.value;
      S().refresh();
      const field = document.getElementById('studio-plugin-search');
      field?.focus();
      field?.setSelectionRange(start, start);
    });
    document.querySelectorAll('[data-add-plugin]').forEach(button => button.addEventListener('click', async () => {
      if (!insertsIdle() || freeSlot(selectedOwner()) < 0) return;
      button.disabled = true;
      try {
        const added = await addInsert(selectedOwner(), button.dataset.addPlugin, { bundlePath: button.dataset.pluginBundlePath || '' });
        if (added) S().toast(`${added.name || 'plug-in'} added`);
      } catch (error) { notifyError(error); button.disabled = false; }
    }));
    document.querySelectorAll('[data-plugin-move]').forEach(button => button.addEventListener('click',
        () => moveEffect(Number(button.dataset.slot), Number(button.dataset.pluginMove))));
    document.querySelectorAll('[data-plugin-action]').forEach(button => button.addEventListener('click', async () => {
      if (ui.pluginMoving || ui.pluginEditing || window.STBusy.isBusy('routing')) return;
      const track = selectedOwner(), context = pluginContext(track), slot = Number(button.dataset.slot), action = button.dataset.pluginAction;
      const payload = { ...ownerPayload(track), kind: 'effect', slot };
      if (action === 'bypass' || action === 'remove') {
        try { await (action === 'bypass' ? bypassInsert(track, slot, undefined, context) : removeInsert(track, slot, context)); }
        catch (error) { notifyError(error); }
        return;
      }
      ui.pluginEditing++;
      try {
        if (action === 'editor') await S().request('plugin.editor', payload);
        if (action === 'refresh') await ensureParameters(track, true);
      } catch (error) { notifyError(error); }
      finally { ui.pluginEditing--; }
    }));
    document.querySelectorAll('[data-plugin-preset]').forEach(select => select.addEventListener('change', async () => {
      if (select.value === '' || ui.pluginMoving || ui.pluginEditing || window.STBusy.isBusy('routing')) return;
      const track = selectedOwner(), context = pluginContext(track), slot = Number(select.dataset.pluginPreset);
      ui.pluginEditing++;
      try {
        const result = await S().request('plugin.preset', { ...ownerPayload(track), kind: 'effect', slot, number: Number(select.value) });
        if (!context.valid()) throw new Error('Preset selection was canceled because its channel changed.');
        ui.parameters.set(pluginKey(track, slot), result.parameters || []);
        await rememberPlugin(result, 'Load plug-in preset', context);
      } catch (error) { notifyError(error); }
      finally { ui.pluginEditing--; }
    }));
    document.querySelectorAll('[data-plugin-address]').forEach(input => {
      let keyTimer = null, keyEdited = false;
      const finishKeys = () => {
        clearTimeout(keyTimer);
        if (keyEdited && input.isConnected !== false) {
          keyEdited = false;
          input.dispatchEvent(new Event('change', { bubbles: true }));
        }
      };
      input.addEventListener('keydown', event => {
        if (!window.STFine?.parameterSteps(input)
            || event.defaultPrevented
            || event.isComposing
            || event.metaKey
            || event.ctrlKey
            || input.disabled
            || !['ArrowUp','ArrowDown','PageUp','PageDown'].includes(event.key)) return;
        event.preventDefault(); event.stopPropagation();
        const value = window.STFine.stepParameter(input.value, ['ArrowUp', 'PageUp'].includes(event.key) ? 1 : -1, {
          fine: event.altKey,
          coarse: event.shiftKey || event.key.startsWith('Page')
        }, input);
        if (value !== Number(input.value)) { input.value=String(value); keyEdited=true; input.dispatchEvent(new Event('input',{bubbles:true})); }
        clearTimeout(keyTimer);keyTimer=setTimeout(finishKeys,700);
      });
      input.addEventListener('keyup',event=>{if(['ArrowUp','ArrowDown','PageUp','PageDown'].includes(event.key))finishKeys();});
      input.addEventListener('blur',finishKeys);
      window.STFine?.bindDrag(input);
      input.addEventListener('input', () => {
        if (ui.pluginMoving || ui.pluginEditing || window.STBusy.isBusy('routing')) return;
        input.value=String(window.STFine?.snapParameter(input.value,input) ?? Number(input.value));
        document.getElementById(`${input.id}-out`).textContent = `${(Number(input.value) * 100).toFixed(1)}%`;
        const id = input.id,
          track = selectedOwner(),
          context = pluginContext(track),
          generation = ui.pluginGeneration,
          slot = Number(input.dataset.slot),
          address = input.dataset.pluginAddress;
        clearTimeout(ui.parameterTimers.get(id));
        ui.parameterTimers.set(id, setTimeout(() => {
          if (!context.valid() || generation !== ui.pluginGeneration || input.isConnected === false || window.STBusy.isBusy('routing')) return;
          S().request('plugin.parameter', {
            ...ownerPayload(track),
            kind: 'effect',
            slot,
            address,
            value: Number(input.value),
            normalized: true
          }).catch(notifyError);
        }, 35));
      });
      input.addEventListener('change', async () => {
        if (ui.pluginMoving || ui.pluginEditing || window.STBusy.isBusy('routing')) return;
        clearTimeout(ui.parameterTimers.get(input.id));
        input.value=String(window.STFine?.snapParameter(input.value,input) ?? Number(input.value));
        const track = selectedOwner(), context = pluginContext(track), slot = Number(input.dataset.slot), address = input.dataset.pluginAddress;
        ui.pluginEditing++;
        try {
          const result = await S().request('plugin.parameter',
              { ...ownerPayload(track), kind: 'effect', slot, address, value: Number(input.value), normalized: true });
          if (!context.valid()) throw new Error('Parameter edit was canceled because its channel changed.');
          const params = ui.parameters.get(pluginKey(track, slot)) || [];
          const param = params.find(item => String(item.address) === address); if (param) Object.assign(param, result);
          const plugin = effectsFor(track).find(item => item.slot === slot);
          if (plugin) await rememberPlugin(plugin, 'Adjust plug-in parameter', context);
          focusId(input.id);
        } catch (error) { notifyError(error); }
        finally { ui.pluginEditing--; }
      });
    });
    ensureParameters(selectedOwner());
  }

  function toUnit(value, info) {
    if (info.fineCurve && window.STFine) return window.STFine.toPosition(value, {...info, ...info.fineCurve});
    if (!(info.max > info.min)) return 0;
    return info.log && info.min > 0
        ? Math.log(Math.max(info.min, value) / info.min) / Math.log(info.max / info.min)
        : (value - info.min) / (info.max - info.min);
  }
  function fromUnit(value, info) {
    if (info.fineCurve && window.STFine) { const spec = {...info, ...info.fineCurve}; return window.STFine.quantize(window.STFine.fromPosition(value, spec),
        spec); }
    const raw = info.log && info.min > 0 ? info.min * Math.pow(info.max / info.min, value) : info.min + value * (info.max - info.min);
    return clamp(info.step > 0 ? Math.round(raw / info.step) * info.step : raw, info.min, info.max);
  }
  // Editing/drag previews must not change project objects before mutate captures undo.
  const ordered = points => (points || []).map(point => ({ ...point })).sort((a, b) => a.row - b.row);
  const normalizedPoints = (points, info) => {
    const unique = new Map();
    // Live recording stores sub-row points (1/64 row); keyboard and table edits stay on whole rows.
    for (const point of points) unique.set(Math.round(clamp(point.row, 0, Math.max(pattern().length - 1, pattern().length - 1 / 64)) * 64) / 64,
        clamp(point.value, info.min, info.max));
    return [...unique].sort((a, b) => a[0] - b[0]).map(([row, value]) => ({ row, value }));
  };

  const livePoints = lane => window.STAutomationRecord?.livePoints?.(pattern().id, ownerForLane(lane), lane.target) || null;
  function envelopeMarkup(lane, info, suppliedPoints, previewPoint) {
    const live = suppliedPoints ? null : livePoints(lane);
    const points = ordered(suppliedPoints || live || lane.points), last = Math.max(1, pattern().length);
    const selected = previewPoint ? points.findIndex(point => point.row === previewPoint.row && point.value === previewPoint.value) : ui.point;
    const left = 96, right = 876, top = 28, bottom = 236;
    const x = row => left + row / last * (right - left), y = value => bottom - clamp(toUnit(value, info), 0, 1) * (bottom - top);
    let path = '';
    points.forEach((point, i) => {
      path += i === 0
          ? `M ${left} ${y(point.value)} L ${x(point.row)} ${y(point.value)}`
          : lane.interpolation === 'step'
          ? ` H ${x(point.row)} V ${y(point.value)}`
          : ` L ${x(point.row)} ${y(point.value)}`;
    });
    if (points.length) path += ` H ${right}`;
    const division = Math.max(1, Math.ceil(pattern().length / 16));
    let grid = '';
    for (let row = 0; row <= last; row += division) grid += `<line x1="${x(row)}" x2="${x(row)}" y1="${top}" y2="${bottom}" class="studio-grid-line"/>`;
    [0, .25, .5, .75, 1].forEach(unit => {
      const height = bottom - unit * (bottom - top);
      grid += `<line x1="${left}" x2="${right}" y1="${height}" y2="${height}" class="studio-grid-line"/>`;
    });
    return `<rect x="96" y="28" width="780" height="208" class="studio-plot-bg"/>${grid}${path
        ? `<path d="${path} L ${right} ${bottom} L ${left} ${bottom} Z" class="studio-envelope-area"/><path d="${path}" class="studio-envelope-line"/>`
        : ''}${points.map((point, i) => `<circle class="studio-envelope-point ${i === selected ? 'is-selected' : ''}" cx="${x(point.row)}" cy="${y(point.value)}"`
        + ` r="${i === selected ? 6 : 4.5}" data-envelope-point="${i}"><title>Row ${Number(point.row.toFixed(3))}: ${esc(info.format(point.value))}</title>`
        + `</circle>`).join('')}`;
  }

  function envelopeLabels(info) {
    const last = Math.max(1, pattern().length), division = Math.max(1, Math.ceil(pattern().length / 16));
    let labels = '';
    for (let row = 0; row < last; row += division) labels += `<span class="studio-axis-row" style="left:${(96 + row / last * 780) / 9}%;top:${256 / 274 * 100}%">${String(row).padStart(2, '0')}</span>`;
    labels += `<span class="studio-axis-row is-end" style="left:${876 / 9}%;top:${256 / 274 * 100}%" title="End of pattern">${last}</span>`;
    for (const unit of [0, .25, .5, .75, 1]) labels += `<span class="studio-axis-value" style="right:${(900 - 84) / 9}%;top:${(236 - unit * 208) / 274 * 100}%">${esc(info.format(fromUnit(unit, info)))}</span>`;
    return `<div class="studio-envelope-labels" aria-hidden="true">${labels}</div>`;
  }

  function automationPlayback(payload = state().status || {}) {
    const current = pattern(), playing = payload.playing === true;
    const hasPattern = Number.isFinite(payload.pattern), matches = hasPattern && payload.pattern === Number(current.id);
    const row = Number.isFinite(payload.row) ? payload.row : 0;
    const position = Math.max(0, Number.isFinite(payload.position) ? payload.position : row);
    const rowLabel = value => value.toFixed(2).padStart(5, '0');
    if (!playing) return { visible: false, mode: 'stopped', label: `Stopped · Edit row ${String(state().row || 0).padStart(2, '0')}`, detail: 'Playback line appears when this pattern plays', left: 96 / 9 };
    if (!hasPattern) return { visible: false, mode: 'waiting', label: 'Playing · Waiting for pattern position', detail: '', left: 96 / 9 };
    const audible = project().patterns.find(item => Number(item.id) === payload.pattern);
    if (!matches) return { visible: false, mode: 'other-pattern', label: `Row ${rowLabel(position)} · Playing ${audible?.name || `pattern ${payload.pattern + 1}`}`, detail: `Viewing ${current.name || 'another pattern'}`, left: 96 / 9 };
    const bounded = clamp(position, 0, current.length);
    return { visible: true, mode: 'playing', label: `Playing · Row ${rowLabel(bounded)} / ${current.length}`, detail: 'Playback', left: (96 + bounded / Math.max(1, current.length) * 780) / 9 };
  }

  function automationPlaybackMarkup() {
    const playback = automationPlayback();
    return `<div class="studio-automation-transport" id="automation-transport" data-mode="${playback.mode}"><output id="automation-position"`
        + ` aria-label="Automation transport position" aria-live="off" title="${esc(playback.label)}">${esc(playback.label)}</output>`
        + `<span id="automation-playback-context" title="${esc(playback.detail)}">${esc(playback.detail)}</span></div>`;
  }

  function automationPlayheadMarkup() {
    const playback = automationPlayback();
    return `<div id="automation-playhead" class="studio-automation-playhead" aria-hidden="true" ${playback.visible ? '' : 'hidden'} style="left:${playback.left}%;top:${28 / 274 * 100}%;height:${208 / 274 * 100}%"></div>`;
  }

  function updateAutomationPlayback(payload) {
    const output = document.getElementById('automation-position'); if (!output) return;
    const playback = automationPlayback(payload), head = document.getElementById('automation-playhead');
    // Never refresh, scroll or change focus/selection on a transport tick.
    if (output.textContent !== playback.label) { output.textContent = playback.label; output.title = playback.label; }
    const context = document.getElementById('automation-playback-context');
    if (context && context.textContent !== playback.detail) { context.textContent = playback.detail; context.title = playback.detail; }
    const strip = document.getElementById('automation-transport'); if (strip) strip.dataset.mode = playback.mode;
    if (head) { head.hidden = !playback.visible; head.style.left = `${playback.left}%`; }
  }

  function updatePointSelection() {
    const lane = selectedLane(); if (!lane) return;
    document.querySelectorAll('.studio-point-row').forEach((row, i) => row.classList.toggle('is-selected', i === ui.point));
    const svg = document.getElementById('automation-envelope');
    if (svg) svg.innerHTML = envelopeMarkup(lane, targetInfo(lane));
  }

  // ---- Pattern switching and the Song timeline (automation-song.js) --------------------
  // ui.ref is the selected parameter {owner, target, name}. It survives pattern switches: a
  // pattern without that lane shows a "ghost" panel instead of jumping to another lane.
  const refKey = r => r ? `${r.owner}|${r.target}` : '';
  const laneRef = lane => lane ? { owner: ownerForLane(lane), target: lane.target, name: lane.name || lane.target } : null;
  const laneIndexFor = r => r ? lanes().findIndex(lane => ownerForLane(lane) === r.owner && lane.target === r.target) : -1;
  const pad2 = n => String(n).padStart(2, '0');
  /** Align the lane selection with ui.ref; returns the ref when this pattern has no lane for it. */
  function syncRef() {
    const id = pattern().id, actual = () => lanes().length ? selectedLane() : null;
    let r = ui.ref;
    if (r && r.pattern !== id) {
      const index = laneIndexFor(r);
      if (index >= 0) { state().selectedLane = index; ui.point = 0; }
      r.pattern = id; r.seen = refKey(laneRef(actual()));
    } else if (r && refKey(laneRef(actual())) !== r.seen) r = null; // selection changed elsewhere (e.g. ⌥A)
    if (!r) { const lane = actual(); r = lane ? { ...laneRef(lane), pattern: id } : null; }
    if (r) r.seen = refKey(laneRef(actual()));
    ui.ref = r;
    return r && laneIndexFor(r) < 0 ? r : null;
  }
  const lanePoints = (patternItem, r) => {
    const lane = r && patternItem.automation.find(item => ownerForLane(item) === r.owner && item.target === r.target);
    return lane ? lane.points.length : -1;
  };
  function switchPattern(id, focus) {
    const p = project(), next = p.patterns.find(item => Number(item.id) === Number(id)); if (!next) return false;
    const was = document.activeElement?.id || '';
    syncRef();
    if (Number(pattern().id) !== Number(next.id)) { if (S().selectPattern) S().selectPattern(next.id); else { p.activePattern = next.id; S().refresh(); } }
    else S().refresh();
    // Keep keyboard focus on the same kind of control; from the lane (or nowhere) land on the new lane.
    const lane = ui.scope === 'song'
        ? 'automation-song-envelope'
        : document.getElementById('automation-envelope')
        ? 'automation-envelope'
        : 'automation-ghost-create';
    const wanted = focus
        || (!was || ['automation-envelope', 'automation-ghost-create', 'automation-song-envelope'].includes(was) || !document.getElementById(was)
            ? lane
            : was);
    focusId(document.getElementById(wanted) ? wanted : document.getElementById(lane) ? lane : 'automation-pattern');
    return true;
  }
  /* Patterns in song order (first appearance in the sequence), then the ones the song does not use. */
  function songOrderedPatterns(p = project()) {
    const list = p.patterns, seen = new Set(), ordered = [];
    for (const id of p.sequence) {
      const item = list.find(entry => Number(entry.id) === Number(id));
      if (item && !seen.has(item.id)) { seen.add(item.id); ordered.push(item); }
    }
    return { inSong: ordered, rest: list.filter(item => !seen.has(item.id)) };
  }
  function stepPattern(delta) {
    const order = songOrderedPatterns(), list = [...order.inSong, ...order.rest], index = Math.max(0, list.findIndex(item => item.id === pattern().id));
    if (list.length < 2) { S().toast('This session has one pattern.'); return false; }
    const next = list[(index + delta + list.length) % list.length];
    const switched = switchPattern(next.id);
    if (ui.scope === 'song'
        && !project().sequence.some(id => Number(id) === Number(next.id))) S().toast(`${next.name || 'This pattern'} is not in the song.`);
    return switched;
  }
  function setScope(scope) {
    if (!window.STAutomationSong || (scope !== 'song' && scope !== 'pattern')) return false;
    syncRef();
    if (ui.scope !== scope) { ui.scope = scope; if (scope === 'song') window.STAutomationSong.enter(); }
    S().refresh();
    const prefer = scope === 'song'
        ? ['automation-song-envelope', 'automation-scope-song']
        : ['automation-envelope', 'automation-ghost-create', 'automation-scope-pattern'];
    focusId(prefer.find(id => document.getElementById(id)) || prefer[prefer.length - 1]);
    return true;
  }
  function songContext(r) {
    const info = targetInfo({ owner: r.owner, target: r.target, name: r.name });
    return { ref: r, info, channel: channelTitle(r.owner), livePoints: id => window.STAutomationRecord?.livePoints?.(id, r.owner, r.target)
        || null, isWriting: !!window.STAutomationRecord?.isWriting?.(r.owner, r.target),
      selectPattern: id => switchPattern(id,
          'automation-song-envelope'), showPattern: id => { ui.scope = 'pattern'; switchPattern(id, 'automation-envelope'); } };
  }
  function patternBar(r, lane) {
    const p = project(), list = p.patterns, song = !!window.STAutomationSong, current = pattern();
    const inSong = id => p.sequence.filter(item => Number(item) === Number(id)).length;
    // Song order, so stepping through the selector follows the arrangement; numbers stay the pattern list's.
    const order = songOrderedPatterns(p), number = item => list.indexOf(item);
    const option = item => {
      const n = lanePoints(item, r),
        times = inSong(item.id);
      return `<option value="${esc(item.id)}" ${item.id === current.id ? 'selected' : ''}>${pad2(number(item) + 1)} · ${esc(item.name || 'Pattern')}${n >= 0 ? ` · ∿ ${n} pt${n === 1 ? '' : 's'}` : ''}${times ? ` · ×${times} in song` : ''}</option>`;
    };
    const options = order.inSong.length && order.rest.length
        ? `<optgroup label="In the song">${order.inSong.map(option).join('')}</optgroup><optgroup label="Not in the song">${order.rest.map(option).join('')}</optgroup>`
        : [...order.inSong, ...order.rest].map(option).join('');
    const chips = r ? [...order.inSong, ...order.rest].map(item => ({ item, i: number(item), n: lanePoints(item, r) })).filter(entry => entry.n >= 0).map(({
      item,
      i,
      n
    }) => `<button class="studio-pattern-chip ${item.id === current.id ? 'is-current' : ''}" id="automation-goto-${esc(item.id)}"`
        + ` data-automation-goto="${esc(item.id)}" title="${esc(item.name || '')} · ${n} point${n === 1 ? '' : 's'}" aria-label="Go to pattern ${pad2(i + 1)}`
        + ` ${esc(item.name || '')}, ${n} points">P${pad2(i + 1)}<small>${n}</small></button>`).join('') : '';
    return `<div class="studio-automation-scope" role="toolbar" aria-label="Automation pattern. [ and ] switch patterns${song ? '; backslash toggles the song timeline' : ''}.">${song ? `<div class="studio-scope-toggle" role="group" aria-label="Automation scope"><button class="${ui.scope === 'pattern' ? 'is-selected' : ''}"`
        + ` id="automation-scope-pattern" aria-pressed="${ui.scope === 'pattern'}" title="Edit one pattern's lane · \\">Pattern</button>`
        + `<button class="${ui.scope === 'song' ? 'is-selected' : ''}" id="automation-scope-song" aria-pressed="${ui.scope === 'song'}"`
        + ` title="See and edit this parameter along the song · \\">Song timeline</button></div>` : ''}
      <div class="studio-pattern-switch"><button class="btn studio-small" id="automation-pattern-prev" aria-label="Previous pattern"`
          + ` title="Previous pattern · [">‹</button><select class="field" id="automation-pattern" aria-label="Pattern to automate. [ and ] switch patterns `
          + `and keep the selected parameter.">${options}</select><button class="btn studio-small" id="automation-pattern-next" aria-label="Next pattern"`
          + ` title="Next pattern · ]">›</button></div>
      ${r
          ? `<div class="studio-lane-presence" role="group" aria-label="Patterns that automate ${esc(r.name)}"><span class="muted">${chips ? `${esc(r.name)} in` : `${esc(r.name)}: no lanes yet`}</span>${chips}</div>`
          : ''}
      ${song ? `<button class="btn studio-small" id="automation-copy-lane" ${lane && ui.scope === 'pattern' ? '' : 'disabled'} title="Copy this lane to `
          + `other patterns">Copy lane to…</button>` : ''}</div>`;
  }
  function ghostPanel(r) {
    const list = project().patterns, current = pattern(), here = list.indexOf(current);
    const others = list.map((item, i) => ({ item, i, n: lanePoints(item, r) })).filter(entry => entry.n >= 0);
    const source = [...others].reverse().find(entry => entry.i < here) || others[0];
    return `<section class="panel studio-automation-empty studio-lane-ghost" id="automation-ghost"><h3>${esc(r.name)} has no automation in ${esc(current.name || 'this pattern')}</h3>`
        + `<p>${others.length ? `Automated in ${others.map(entry => `P${pad2(entry.i + 1)}`).join(', ')}. While this pattern plays, the value holds where the previous `
        + `pattern left it.` : 'No pattern automates this parameter yet.'}</p>
      <div class="studio-ghost-actions"><button class="btn" id="automation-ghost-create" title="Add a lane for this parameter in this pattern">Create `
          + `lane here</button>${source
              ? `<button class="btn" id="automation-ghost-copy" data-from="${esc(source.item.id)}">Copy lane from P${pad2(source.i + 1)}</button>`
              : ''}${window.STAutomationSong ? '<button class="btn" id="automation-ghost-song">Show across song</button>' : ''}</div>
      <div class="studio-empty-tips"><span><kbd>[</kbd> <kbd>]</kbd> Switch pattern</span>${window.STAutomationSong ? '<span><kbd>\\</kbd> Song timeline</span>' : ''}</div></section>`;
  }
  function copyLaneTo(ids, label) {
    if (!settleAutomationEdit()) return 0;
    const M = window.STAutomationSong, lane = selectedLane(); if (!M || !lane || syncRef()) return 0;
    const r = laneRef(lane), from = pattern().id; let count = 0;
    const undo = state().undo, top = undo?.[undo.length - 1];
    S().mutate(label || 'Copy automation lane', p => { count = M.copyLane(p, from, r.owner, r.target, ids, { uid: S().uid }); });
    if (state().undo && state().undo[state().undo.length - 1] === top) return 0; // refused (frozen track, export…)
    if (count) S().toast(`${r.name} copied to ${count} pattern${count === 1 ? '' : 's'}. ⌘Z undoes it.`);
    else S().toast('No other patterns were chosen.');
    return count;
  }
  function copyLaneDialog() {
    const M = window.STAutomationSong, lane = selectedLane();
    if (!M || !lane || syncRef()) { S().toast('Select a lane in this pattern first.'); return false; }
    const r = laneRef(lane), p = project(), current = pattern(), rows = M.summary(p, r.owner, r.target).filter(item => item.id !== current.id);
    if (!rows.length) { S().toast('This session has one pattern.'); return false; }
    const using = new Set(M.patternsUsing(p, r.owner).map(Number)), channel = channelTitle(r.owner);
    const body = `<p class="muted">Copy <strong>${esc(r.name)}</strong> (${lane.points.length} points) from ${esc(current.name || 'this pattern')}. `
        + `Lanes in the chosen patterns are replaced; rows stay where they are, so a shorter pattern ends on the lane's value at its last row.</p>
      <div class="studio-copy-quick"><button class="btn studio-small" id="automation-copy-using">Patterns using ${esc(channel)}</button>`
          + `<button class="btn studio-small" id="automation-copy-song">Patterns in the song</button><button class="btn studio-small"`
          + ` id="automation-copy-none">None</button></div>
      <div class="studio-copy-list" role="group" aria-label="Destination patterns">${rows.map(item => `<label class="studio-copy-row"><input type="checkbox" data-copy-pattern="${esc(item.id)}"><span>${pad2(item.number)} · ${esc(item.name)}</span>`
          + `<small>${[item.lane ? `replaces ${item.points} point${item.points === 1 ? '' : 's'}` : 'no lane',
              item.plays === true ? 'track plays' : item.plays === false ? 'track silent' : '',
              item.inSong ? `×${item.inSong} in song` : 'not in song', item.length !== current.length ? `${item.length} rows` : ''].filter(Boolean).join(' · ')}</small>`
          + `</label>`).join('')}</div>`;
    S().openModal('Copy automation lane', body, '<button class="btn" id="automation-copy-cancel">Cancel</button><button class="btn primary" id="automation-copy-apply">Copy lane</button>', {
      kind: 'automation-copy'
    });
    const boxes = () => [...document.querySelectorAll('[data-copy-pattern]')];
    const pick = test => boxes().forEach(box => { box.checked = test(Number(box.dataset.copyPattern)); });
    const on = (id, fn) => { const node = document.getElementById(id); if (node) node.onclick = fn; };
    on('automation-copy-using', () => pick(id => using.has(id)));
    on('automation-copy-song', () => pick(id => project().sequence.some(item => Number(item) === id)));
    on('automation-copy-none', () => pick(() => false));
    on('automation-copy-cancel', () => S().closeModal());
    on('automation-copy-apply', () => {
      const ids = boxes().filter(box => box.checked).map(box => Number(box.dataset.copyPattern));
      if (!ids.length) { S().toast('Choose at least one pattern.'); return; }
      S().closeModal();
      copyLaneTo(ids, `Copy automation lane to ${ids.length} pattern${ids.length === 1 ? '' : 's'}`);
      focusId('automation-copy-lane');
    });
    return true;
  }
  function copyLaneToUsing() {
    const M = window.STAutomationSong, lane = selectedLane(); if (!M || !lane || syncRef()) { S().toast('Select a lane in this pattern first.'); return 0; }
    const ids = M.patternsUsing(project(), ownerForLane(lane)).filter(id => Number(id) !== Number(pattern().id));
    if (!ids.length) { S().toast('No other pattern uses this channel.'); return 0; }
    return copyLaneTo(ids, `Copy automation lane to ${ids.length} pattern${ids.length === 1 ? '' : 's'}`);
  }
  function bindPatternBar() {
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    on('automation-pattern', 'change', event => switchPattern(event.target.value, 'automation-pattern'));
    on('automation-pattern-prev', 'click', () => { stepPattern(-1); focusId('automation-pattern-prev'); });
    on('automation-pattern-next', 'click', () => { stepPattern(1); focusId('automation-pattern-next'); });
    on('automation-scope-pattern', 'click', () => setScope('pattern'));
    on('automation-scope-song', 'click', () => setScope('song'));
    on('automation-copy-lane', 'click', () => copyLaneDialog());
    on('automation-ghost-create', 'click', () => { const r = ui.ref; if (r) S().automate(r.owner, r.target, r.name); });
    on('automation-ghost-song', 'click', () => setScope('song'));
    on('automation-ghost-copy', 'click', event => {
      if (!settleAutomationEdit()) return;
      const M = window.STAutomationSong, r = ui.ref, from = event.currentTarget?.dataset?.from
          ?? document.getElementById('automation-ghost-copy')?.dataset.from; if (!M || !r) return;
      const to = pattern().id;
      S().mutate('Copy automation lane', p => { M.copyLane(p, from, r.owner, r.target, [to], { uid: S().uid }); });
      focusId(document.getElementById('automation-envelope') ? 'automation-envelope' : 'automation-ghost-create');
    });
    document.querySelectorAll('[data-automation-goto]').forEach(button => button.addEventListener('click',
        () => switchPattern(button.dataset.automationGoto, `automation-goto-${button.dataset.automationGoto}`)));
    if (!ui.contextRegistered && window.STContextMenu?.register) {
      ui.contextRegistered = true;
      window.STContextMenu.register(({ target } = {}) => {
        if (state().view !== 'automation'
            || !target?.closest?.('[data-select-lane],#automation-envelope,#automation-ghost,.studio-automation-scope')) return null;
        const tab = target.closest('[data-select-lane]');
        if (tab && Number(tab.dataset.selectLane) !== state().selectedLane) {
          ui.ref = null;
          state().selectedLane = Number(tab.dataset.selectLane);
          ui.point = 0;
          S().refresh();
        }
        const ghost = syncRef(), lane = ghost ? null : selectedLane(), song = !!window.STAutomationSong;
        return [
          { header: true, title: ui.ref ? `${ui.ref.name} · ${pattern().name || 'Pattern'}` : 'Automation' },
          { title: 'Previous pattern', key: '[', action: () => stepPattern(-1) },
          { title: 'Next pattern', key: ']', action: () => stepPattern(1) },
          { separator: true },
          { title: 'Copy lane to patterns…', enabled: song && !!lane, action: () => copyLaneDialog() },
          { title: `Copy lane to all patterns using ${lane ? channelTitle(ownerForLane(lane)) : 'this channel'}`, enabled: song && !!lane, action: () => copyLaneToUsing() },
          { title: 'Show across song', key: '\\', enabled: song && !!ui.ref, action: () => setScope('song') }
        ];
      }, 65);
    }
  }
  /** Palette entry points (app.js perform → automation* commands). */
  function command(name) {
    switch (name) {
      case 'automationPatternPrev': return stepPattern(-1);
      case 'automationPatternNext': return stepPattern(1);
      case 'automationScope': return setScope(ui.scope === 'song' ? 'pattern' : 'song');
      case 'automationCopyLane': if (ui.scope === 'song') setScope('pattern'); return copyLaneDialog();
      case 'automationCopyLaneUsing': return copyLaneToUsing();
      case 'automationSweep': if (!ui.ref) { S().toast('Select a parameter first.'); return false; } if (ui.scope !== 'song') setScope('song'); return window.STAutomationSong?.command('sweep',
          songContext(ui.ref));
      case 'automationMakeUnique': if (!ui.ref) { S().toast('Select a parameter first.'); return false; } if (ui.scope !== 'song') setScope('song'); return window.STAutomationSong?.command('unique',
          songContext(ui.ref));
    }
    return false;
  }

  function renderAutomation() {
    const ghost = syncRef(), ref = ui.ref, songScope = ui.scope === 'song' && !!window.STAutomationSong;
    const p = project(), track = selectedOwner(), existing = lanes(), lane = ghost ? null : selectedLane();
    const query = (state().automationQuery || '').toLowerCase();
    const exposed = allTargets(track);
    for (const item of existing.filter(item => ownerForLane(item) === track)) if (!exposed.some(target => target.target === item.target)) exposed.push(targetInfo(item));
    const targets = exposed.map(target => ({
      ...target,
      lane: existing.findIndex(l => ownerForLane(l) === track && l.target === target.target)
    })).filter(target => `${target.name} ${target.group} ${target.search || ''}`.toLowerCase().includes(query));
    targets.sort((a, b) => Number(b.lane >= 0) - Number(a.lane >= 0));
    const used = targets.filter(target => target.lane >= 0), unused = targets.filter(target => target.lane < 0);
    const targetButton = item => `<button class="studio-target ${ref && ref.owner === track && ref.target === item.target ? 'is-selected' : ''}" id="automation-target-${esc(item.target)}"`
        + ` data-choose-target="${esc(item.target)}" data-target-name="${esc(item.name)}" aria-label="${item.lane >= 0 ? 'Select automation for' : 'Automate'}`
        + ` ${esc(item.name)}${item.available === false ? ' (unavailable)' : item.inactive ? ' (inactive in current rate mode)' : ''}"`
        + ` ${item.available === false || item.inactive ? `title="${esc(item.unavailableReason || item.inactiveReason)}"` : ''}>`
        + `<span class="studio-target-icon">${item.available === false ? '!' : item.lane >= 0 ? '∿' : '+'}</span>`
        + `<span><strong>${esc(item.name)}</strong><small>${item.available === false ? 'Unavailable · ' : item.inactive ? 'Inactive · ' : ''}${esc(item.group || 'Saved parameter')}</small>`
        + `</span>${item.lane >= 0 ? `<span class="studio-target-count">${existing[item.lane].points.length}</span>` : ''}</button>`;
    return `<div class="studio-automation" data-scroll="automation-workspace"><div class="panel-head"><div><h2>Automation</h2>`
        + `<p class="muted">${esc(pattern().name || 'Current pattern')} · ${pattern().length} rows · ${songScope ? 'Song timeline' : 'Pattern automation'}</p>`
        + `</div><span class="chip">${existing.length} lane${existing.length === 1 ? '' : 's'}</span></div>
      ${patternBar(ref, lane)}
      <div class="studio-automation-layout"><aside class="panel studio-target-panel"><label class="studio-control-label" for="automation-track">Channel</label>`
          + `<select class="field" id="automation-track">${channels().map(channel => `<option value="${esc(channel.owner)}" ${channel.owner === track ? 'selected' : ''}>${channel.type === 'track'
              ? String(channel.track + 1).padStart(2, '0')
              : channel.type === 'master'
              ? 'Out'
              : channel.type === 'group'
              ? 'Group'
              : 'Return'}`
          + ` · ${esc(channel.name)}</option>`).join('')}</select>`
          + `<input type="search" id="automation-search" class="field" placeholder="Find a parameter…" value="${esc(state().automationQuery || '')}"`
          + ` aria-label="Search automation parameters"><div class="studio-target-list" data-scroll="automation-targets">${used.length
              ? `<div class="studio-list-caption">Already automated <span>${used.length}</span></div>${used.map(targetButton).join('')}`
              : ''}${unused.length
              ? `<div class="studio-list-caption">Available parameters <span>${unused.length}</span></div>${unused.map(targetButton).join('')}`
              : ''}${!targets.length ? '<p class="muted studio-hint">No parameters match your search.</p>' : ''}</div>`
          + `<p class="studio-hint muted">From any parameter: Option A.<br>Or use its context menu.</p></aside>
      <div class="studio-lane-workspace">${laneStack(existing, lane, songScope)}${songScope ? (ref ? window.STAutomationSong.render(songContext(ref)) : '<section class="panel studio-automation-empty"><h3>Choose a parameter</h3><p>Select a parameter to see its automation along the song.</p>'
          + '</section>') : lane ? renderLane(lane) : ghost ? ghostPanel(ghost) : `<section class="panel studio-automation-empty"><h3>No automation lanes</h3><p>Select a parameter to add its first point.<br>Existing lanes `
          + `appear above available parameters.</p><div class="studio-empty-tips"><span><kbd>Tab</kbd> Move between controls</span>`
          + `<span><kbd>⌥ A</kbd> Automate focused parameter</span><span><kbd>Enter</kbd> Select a parameter</span></div>`
          + `</section>`}</div></div></div>`;
  }

  /* One box per automated parameter of this pattern, stacked and scrollable, every box the same height (stable
     locations). A box shows the lane read-only; the selected lane is edited in the docked editor below the stack.
     In the song scope a box shows the parameter along the whole song. Up and down choose a lane, Enter enters its
     envelope. */
  function laneStack(existing, lane, songScope) {
    if (!existing.length) return '';
    const A = window.STApp;
    const box = (item, i) => {
      const owner = ownerForLane(item), channel = channelFor(owner), info = targetInfo(item), selected = item === lane;
      const ref = { owner, target: item.target, name: item.name || item.target };
      const list = A?.automationLaneSlots?.(ref, info, songScope ? 'song' : 'pattern');
      const plot = list?.length ? A.automationLaneMarkup(list, info, 1, { height: 36, top: 3, bottom: 33, dots: false, stretch: true }) : '';
      return `<div class="studio-lane-box ${selected ? 'is-selected' : ''}" id="automation-lane-${i}" role="option" aria-selected="${selected}"`
          + ` tabindex="${selected ? 0 : -1}" data-select-lane="${i}"><div class="studio-lane-box-head"><span>${channel?.type === 'track'
              ? String(channel.track + 1).padStart(2, '0')
              : esc(channel?.type || '?')}</span><strong>${esc(item.name || item.target)}</strong><small>${esc(channelTitle(owner))}</small>`
          + `<em>${item.points.length} pt${item.points.length === 1 ? '' : 's'}</em></div><div class="studio-lane-box-plot">${plot}</div></div>`;
    };
    return `<div class="studio-lane-stack" id="automation-lane-stack" data-scroll="automation-lanes" role="listbox" aria-label="Automation lanes`
        + ` in this pattern. Up and down choose a lane; Enter enters its envelope.">${existing.map(box).join('')}</div>`;
  }
  /** Scroll the selected lane's box into view (the parameter list chooses lanes without leaving the list). */
  const revealLane = () => document.getElementById(`automation-lane-${state().selectedLane}`)?.scrollIntoView?.({ block: 'nearest' });

  function renderLane(lane) {
    const info = targetInfo(lane), points = ordered(lane.points);
    ui.point = clamp(ui.point, 0, Math.max(0, points.length - 1));
    return `<section class="panel studio-lane-panel"><div class="studio-lane-header"><div><label class="studio-control-label" for="automation-name">Lane `
        + `name</label><input class="field studio-lane-name" id="automation-name" value="${esc(lane.name || info.name)}" aria-label="Automation lane `
        + `name"><div class="studio-lane-source muted">${esc(channelTitle(ownerForLane(lane)))} / ${esc(info.name)}</div>`
        + `</div><div class="studio-lane-header-actions">${recordControls(lane)}<label for="automation-interpolation" class="studio-control-label">Interpolation</label>`
        + `<select class="field" id="automation-interpolation"><option value="linear" ${lane.interpolation !== 'step' ? 'selected' : ''}>Linear</option>`
        + `<option value="step" ${lane.interpolation === 'step' ? 'selected' : ''}>Step / hold</option></select><button class="btn studio-small studio-danger"`
        + ` id="automation-delete-lane" data-automation-action="delete-lane" aria-label="Delete automation lane">Delete lane</button></div></div>
      ${info.available === false ? `<p class="notice" role="status"><strong>Parameter unavailable for this track.</strong> ${esc(info.unavailableReason)} Your saved points `
          + `are preserved and remain editable.</p>` : info.inactive ? `<p class="notice" role="status">${esc(info.inactiveReason)} Saved points remain editable.</p>` : ''}
      ${automationPlaybackMarkup()}
      <div class="studio-envelope-wrap${window.STAutomationRecord?.isWriting?.(ownerForLane(lane), lane.target) ? ' is-recording' : ''}"`
          + ` id="automation-envelope-wrap"><svg id="automation-envelope" viewBox="0 0 900 274" tabindex="0" role="application" aria-label="${esc(lane.name)}`
          + ` envelope. Left and right move the selected point by a row; up and down adjust its value. Option plus left or right selects the previous `
          + `or next point. Home and End select the first or last point. Delete removes a point. Insert or Enter adds a point. Tab reaches the precise `
          + `point editor." preserveAspectRatio="none">${envelopeMarkup(lane, info)}</svg>${envelopeLabels(info)}${automationPlayheadMarkup()}</div>`
          + `<div class="studio-envelope-hint"><span><span class="studio-dot"></span>${info.log || info.fineCurve ? 'Perceptual value scale' : 'Value over pattern rows'}</span>`
          + `<span>Arrows edit · ⌥← / ⌥→ select point · Enter adds · Shift for larger steps</span></div>
      <div class="studio-point-toolbar"><h4>Breakpoints <span class="muted">${points.length}</span></h4><div><button class="btn studio-small"`
          + ` id="automation-add-point" data-automation-action="add-point">+ Add point</button><button class="btn studio-small" id="automation-copy"`
          + ` data-automation-action="copy" ${!points.length ? 'disabled' : ''}>Copy</button><button class="btn studio-small" id="automation-paste"`
          + ` data-automation-action="paste" ${!ui.clipboard ? 'disabled' : ''}>Paste</button><button class="btn studio-small" id="automation-clear"`
          + ` data-automation-action="clear" ${!points.length ? 'disabled' : ''}>Clear</button></div></div>
      <div class="studio-points-table" data-scroll="automation-points" role="table" aria-label="Automation points">`
          + `<div class="studio-points-head" role="row"><span>POINT</span><span>ROW</span><span>VALUE ${info.plugin ? '· NORMALIZED' : ''}</span>`
          + `<span>DISPLAY</span><span></span></div>${points.length ? points.map((point, i) => `<div class="studio-point-row ${ui.point === i ? 'is-selected' : ''}" role="row"><span class="studio-point-number">${String(i + 1).padStart(2, '0')}</span>`
          + `<input type="number" class="field" id="point-row-${i}" min="0" max="${pattern().length - 1}" step="any" value="${Number(number(point.row).toFixed(4))}"`
          + ` data-point-row="${i}" aria-label="Point ${i + 1} row"><input type="number" class="field" id="point-value-${i}" min="${info.min}"`
          + ` max="${info.max}" step="${info.step}" value="${Number(number(point.value).toFixed(4))}" data-point-value="${i}" aria-label="Point ${i + 1}`
          + ` value"><output>${esc(info.format(point.value))}</output><button class="studio-remove-point" id="point-remove-${i}" data-remove-point="${i}"`
          + ` aria-label="Remove point ${i + 1}">×</button></div>`).join('') : '<div class="studio-empty"><p>No points yet. Add a point or click the envelope.</p></div>'}</div></section>`;
  }

  // Live automation write controls (automation-record.js); absent module renders nothing.
  function recordControls(lane) {
    const recorder = window.STAutomationRecord; if (!recorder) return '';
    const ref = { owner: ownerForLane(lane), target: lane.target }, armed = recorder.isArmed(ref);
    return `<div class="studio-record-controls"><label for="automation-write-mode" class="studio-control-label">Live write</label>`
        + `<select class="field" id="automation-write-mode" aria-label="Automation write mode. Option W toggles Read and the last write mode."`
        + ` title="⌥W Read ⇄ ${esc(recorder.labels[recorder.lastWriteMode])} · ⌥⇧W cycle">${recorder.modes.map(mode => `<option value="${mode}" ${recorder.mode === mode ? 'selected' : ''}>${esc(recorder.labels[mode])}</option>`).join('')}</select>`
        + `<button class="btn studio-small studio-arm ${armed ? 'is-armed' : ''}" id="automation-arm-lane" aria-pressed="${armed}" title="Arm only `
        + `this parameter for automation write. With nothing armed, every parameter can write.">${armed ? '● Armed' : 'Arm'}</button></div>`;
  }
  function refreshLiveAutomation() {
    if (state().view !== 'automation') return;
    if (ui.scope === 'song' && ui.ref && window.STAutomationSong) { window.STAutomationSong.refresh(songContext(ui.ref)); return; }
    const lane = selectedLane(), svg = document.getElementById('automation-envelope'); if (!lane || !svg) return;
    svg.innerHTML = envelopeMarkup(lane, targetInfo(lane));
    const wrap = document.getElementById('automation-envelope-wrap');
    wrap?.classList?.toggle('is-recording', !!window.STAutomationRecord?.isWriting?.(ownerForLane(lane), lane.target));
    const recorder = window.STAutomationRecord,
      mode = document.getElementById('automation-write-mode'),
      arm = document.getElementById('automation-arm-lane');
    if (recorder && mode && mode.value !== recorder.mode && document.activeElement !== mode) mode.value = recorder.mode;
    if (recorder && arm) {
      const armed = recorder.isArmed({ owner: ownerForLane(lane), target: lane.target });
      arm.setAttribute('aria-pressed', String(armed));
      arm.textContent = armed ? '● Armed' : 'Arm';
      arm.classList?.toggle('is-armed', armed);
    }
  }

  function updatePoints(label, next, focus) {
    const lane = selectedLane(); if (!lane) return;
    const values = normalizedPoints(next, targetInfo(lane));
    ui.point = clamp(ui.point, 0, Math.max(0, values.length - 1));
    // An edit that changes nothing (a point already at the first or last row, a value at its limit) is not an Undo step.
    if (JSON.stringify(values) === JSON.stringify(lane.points)) { updatePointSelection(); if (focus) focusId(focus); return; }
    mutate(label, () => { lane.points = values; }, focus);
  }

  function addPoint() {
    if (!settleAutomationEdit()) return;
    const lane = selectedLane(); if (!lane) return;
    const info = targetInfo(lane), points = ordered(lane.points), occupied = new Set(points.map(p => p.row));
    let row = points.length ? Math.min(pattern().length - 1, points[ui.point]?.row + project().linesPerBeat) : 0;
    if (occupied.has(row)) { row = 0; while (occupied.has(row) && row < pattern().length) row++; }
    if (row >= pattern().length) { S().toast('Every row already has an automation point.'); return; }
    points.push({ row, value: points[ui.point]?.value ?? info.value });
    points.sort((a, b) => a.row - b.row); ui.point = points.findIndex(p => p.row === row);
    updatePoints('Add automation point', points, `point-value-${ui.point}`);
  }

  function bindAutomation() {
    const fields = bindPointFields(), renderedPoints = ordered(selectedLane()?.points);
    document.getElementById('automation-track')?.addEventListener('change', event => {
      ui.ref = null;
      selectOwner(event.target.value);
      S().refresh();
      focusId('automation-track');
    });
    document.getElementById('automation-search')?.addEventListener('input', event => {
      const cursor = event.target.selectionStart;
      state().automationQuery = event.target.value;
      S().refresh();
      const input = document.getElementById('automation-search');
      input?.focus();
      input?.setSelectionRange(cursor, cursor);
    });
    document.getElementById('automation-search')?.addEventListener('keydown', event => {
      if (event.key === 'ArrowDown' || event.key === 'Enter') {
        event.preventDefault();
        document.querySelector('[data-choose-target]')?.focus();
      }
    });
    const chooseTarget = button => {
      const track = selectedOwner(), target = button.dataset.chooseTarget;
      const found = lanes().findIndex(lane => ownerForLane(lane) === track && lane.target === target);
      // The song timeline can show a parameter without creating a lane in this pattern.
      if (ui.scope === 'song'
          && window.STAutomationSong) { if (found >= 0) state().selectedLane = found; ui.ref = { owner: track, target, name: button.dataset.targetName || target, pattern: pattern().id, seen: refKey(laneRef(lanes().length ? selectedLane() : null)) }; S().refresh(); revealLane(); return; }
      if (found >= 0) {
        ui.ref = null; state().selectedLane = found; ui.point = 0; S().refresh();
        focusId(button.id); revealLane();
      } else { S().automate(track, target, button.dataset.targetName); revealLane(); }
    };
    document.querySelectorAll('[data-choose-target]').forEach(button => button.addEventListener('click', () => chooseTarget(button)));
    document.querySelectorAll('[data-select-lane]').forEach(button => button.addEventListener('click', () => {
      ui.ref = null;
      state().selectedLane = Number(button.dataset.selectLane);
      selectOwner(ownerForLane(lanes()[state().selectedLane]));
      ui.point = 0;
      S().refresh();
    }));
    document.querySelectorAll('[data-choose-target]').forEach(button => button.addEventListener('keydown', event => {
      // Enter chooses the parameter (as a click) and enters its lane's envelope; a click stays in the list.
      if (event.key === 'Enter' && !event.metaKey && !event.ctrlKey && !event.altKey) {
        event.preventDefault(); event.stopPropagation();
        chooseTarget(button);
        if (ui.scope !== 'song') focusId('automation-envelope');
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault(); event.stopPropagation();
      const items = [...document.querySelectorAll('[data-choose-target]')], index = items.indexOf(button);
      items[event.key === 'Home'
          ? 0
          : event.key === 'End'
          ? items.length - 1
          : clamp(index + (event.key === 'ArrowDown' ? 1 : -1), 0, items.length - 1)]?.focus();
    }));
    document.querySelectorAll('[data-select-lane]').forEach(button => button.addEventListener('keydown', event => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === 'Enter') {
        event.preventDefault(); event.stopPropagation();
        focusId(ui.scope === 'song' ? 'automation-song-envelope' : 'automation-envelope');
        return;
      }
      if (!['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      const current = Number(button.dataset.selectLane), back = event.key === 'ArrowUp' || event.key === 'ArrowLeft';
      // Up from the first lane returns to the parameter list.
      if (event.key === 'ArrowUp' && current === 0) return;
      event.preventDefault(); event.stopPropagation();
      const index = event.key === 'Home'
          ? 0
          : event.key === 'End'
          ? lanes().length - 1
          : clamp(current + (back ? -1 : 1), 0, lanes().length - 1);
      ui.ref = null;
      state().selectedLane = index;
      selectOwner(ownerForLane(lanes()[index]));
      ui.point = 0;
      S().refresh();
      focusId(`automation-lane-${index}`);
      revealLane();
    }));
    document.getElementById('automation-name')?.addEventListener('change', event => {
      if (!settleAutomationEdit()) return;
      mutate('Rename automation lane', () => {
        selectedLane().name = event.target.value.trim() || targetInfo(selectedLane()).name;
      }, 'automation-name');
    });
    document.getElementById('automation-write-mode')?.addEventListener('change', event => {
      window.STAutomationRecord?.setMode(event.target.value);
      focusId('automation-write-mode');
    });
    document.getElementById('automation-arm-lane')?.addEventListener('click', () => {
      const lane = selectedLane();
      if (lane) window.STAutomationRecord?.toggleArm(window.STAutomationRecord.makeRef(ownerForLane(lane), lane.target,
          lane.name || targetInfo(lane).name));
      refreshLiveAutomation();
    });
    document.getElementById('automation-interpolation')?.addEventListener('change', event => {
      if (!settleAutomationEdit()) return;
      mutate('Set interpolation', () => {
        selectedLane().interpolation = event.target.value;
      }, 'automation-interpolation');
    });
    document.querySelectorAll('[data-automation-action]').forEach(button => button.addEventListener('click', () => {
      if (!settleAutomationEdit()) return;
      const action = button.dataset.automationAction, lane = selectedLane(); if (!lane) return;
      if (action === 'delete-lane') {
        const index = lanes().indexOf(lane),
          next = Math.max(0, index - 1),
          focus = lanes().length > 1 ? `automation-lane-${next}` : 'automation-search';
        mutate('Delete automation lane', () => {
          pattern().automation.splice(index, 1); state().selectedLane = next; ui.point = 0;
          const surviving = selectedLane(); if (surviving) selectOwner(ownerForLane(surviving));
        }, focus);
      }
      if (action === 'add-point') addPoint();
      if (action === 'copy') {
        ui.clipboard = { points: ordered(lane.points), info: targetInfo(lane), length: pattern().length };
        S().toast('Automation points copied');
        S().refresh();
      }
      if (action === 'paste' && ui.clipboard) {
        const info = targetInfo(lane);
        updatePoints('Paste automation points', ui.clipboard.points.map(point => ({
          row: Math.round(point.row / Math.max(1, ui.clipboard.length - 1) * (pattern().length - 1)),
          value: fromUnit(toUnit(point.value, ui.clipboard.info), info)
        })));
      }
      if (action === 'clear') updatePoints('Clear automation points', [], 'automation-add-point');
    }));
    document.querySelectorAll('[data-point-row],[data-point-value]').forEach(input => {
      const index = Number(input.dataset.pointRow ?? input.dataset.pointValue);
      const binding = { row: renderedPoints[index]?.row, role: input.dataset.pointRow !== undefined ? 'row' : 'value' };
      fields.bindings.set(input, binding);
      input.addEventListener('focus', () => {
        ui.point = Math.max(0, ordered(fields.lane?.points).findIndex(point => point.row === binding.row));
        updatePointSelection();
      });
      input.addEventListener('input', () => fields.drafts.add(input));
      input.addEventListener('blur', () => { fields.drafts.delete(input); if (fields.pending) paintPointFields(fields); });
      input.addEventListener('change', () => {
        if (!validPointFields(fields)) return;
        // An emptied field is not a row or a value: the point stays as it is (the field shows it again).
        if (!String(input.value).trim() || !Number.isFinite(Number(input.value))) { fields.drafts.delete(input); paintPointFields(fields); return; }
        if (!settleAutomationEdit()) return;
        const points = ordered(fields.lane.points), index = points.findIndex(point => point.row === binding.row); if (index < 0) return;
        const key = binding.role, info = targetInfo(fields.lane), oldRow = binding.row, point = points.splice(index,1)[0];
        point[key] = clamp(input.value,key === 'row' ? 0 : info.min,key === 'row' ? pattern().length - 1 : info.max); points.push(point);
        // The row as it is stored (1/64 row): a typed 9.5 or a recorded sub-row point keeps its selection and focus.
        const next = normalizedPoints(points,info), newRow = Math.round(point.row * 64) / 64; ui.point = next.findIndex(point => point.row === newRow);
        S().mutate('Edit automation point', () => { fields.lane.points = next; }, {render:false});
        if (fields.lane.points === next) for (const value of fields.bindings.values()) if (value.row === oldRow) value.row = newRow;
        fields.drafts.delete(input); paintPointFields(fields);
      });
      input.addEventListener('keydown', event => {
        if ((event.metaKey || event.ctrlKey) && ['Backspace', 'Delete'].includes(event.key)) {
          event.preventDefault();
          if (!settleAutomationEdit()) return;
          const points = ordered(selectedLane().points),
            index = points.findIndex(point => point.row === binding.row);
          if (index < 0) return;
          points.splice(index, 1);
          updatePoints('Delete automation point', points,
              points.length ? `point-value-${Math.min(index, points.length - 1)}` : 'automation-add-point');
        }
      });
    });
    document.querySelectorAll('[data-remove-point]').forEach(button => {
      const binding = {row:renderedPoints[Number(button.dataset.removePoint)]?.row,role:'remove'}; fields.bindings.set(button,binding);
      button.addEventListener('click', () => {
        if (!validPointFields(fields) || !settleAutomationEdit()) return;
        const points = ordered(selectedLane().points),
          index = points.findIndex(point => point.row === binding.row);
        if (index < 0) return;
        points.splice(index, 1);
        updatePoints('Delete automation point', points, points.length ? `point-value-${Math.min(index, points.length - 1)}` : 'automation-add-point');
      });
    });
    bindEnvelope();
    bindPatternBar();
    if (ui.scope === 'song' && ui.ref && window.STAutomationSong) window.STAutomationSong.bind(songContext(ui.ref));
    updateAutomationPlayback();
    ensureParameters(selectedOwner());
  }

  function bindEnvelope() {
    const svg = document.getElementById('automation-envelope'); if (!svg) return;
    svg.addEventListener('pointerdown', event => {
      if (event.button !== 0) return;
      event.preventDefault(); svg.focus();
      const hadTake = window.STAutomationRecord?.hasPendingEdits, hit = Number(event.target.dataset.envelopePoint);
      const hitRow = Number.isInteger(hit) ? ordered(livePoints(selectedLane()) || selectedLane().points)[hit]?.row : undefined;
      if (!settleAutomationEdit(hitRow)) return;
      const lane = selectedLane(), info = targetInfo(lane), points = ordered(lane.points);
      const recordingGeneration = window.STAutomationRecord?.editGeneration;
      const previousSelection = ui.point;
      const pointAt = e => {
        const box = svg.getBoundingClientRect();
        const x = (e.clientX - box.left) / box.width * 900, y = (e.clientY - box.top) / box.height * 274;
        return { row: clamp(Math.round(clamp((x - 96) / 780, 0, 1) * pattern().length), 0,
            pattern().length - 1), value: fromUnit(clamp((236 - y) / 208, 0, 1), info) };
      };
      let index = event.target.dataset.envelopePoint === undefined ? -1 : hadTake ? ui.point : Number(event.target.dataset.envelopePoint);
      const isNewPoint = index < 0;
      if (index < 0) { points.push(pointAt(event)); index = points.length - 1; }
      ui.point = index;
      let moved = false;
      svg.setPointerCapture?.(event.pointerId);
      const preview = e => { points[index] = pointAt(e); moved = true; svg.innerHTML = envelopeMarkup(lane, info, points, points[index]); };
      const finish = e => {
        svg.removeEventListener('pointermove', preview);
        svg.removeEventListener('pointerup', finish);
        svg.removeEventListener('pointercancel', cancel);
        if (e.type === 'pointercancel') return;
        if (!isNewPoint && !moved) { updatePointSelection(); return; }
        if (window.STAutomationRecord?.hasPendingEdits
            && recordingGeneration !== window.STAutomationRecord.editGeneration) { S().toast('Automation changed while dragging. Try the point edit again.'); updatePointSelection(); return; }
        const value = points[index], next = normalizedPoints(points.filter((_, i) => i !== index).concat([value]), info);
        ui.point = next.findIndex(p => p.row === value.row);
        updatePoints(isNewPoint ? 'Add automation point' : 'Move automation point', next, 'automation-envelope');
      };
      const cancel = e => { finish(e); ui.point = clamp(previousSelection, 0, Math.max(0, lane.points.length - 1)); updatePointSelection(); };
      svg.addEventListener('pointermove', preview); svg.addEventListener('pointerup', finish); svg.addEventListener('pointercancel', cancel);
      svg.innerHTML = envelopeMarkup(lane, info, points, points[index]);
    });
    svg.addEventListener('keydown', event => {
      if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Delete', 'Backspace', 'Insert', 'Enter', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault(); event.stopPropagation();
      const navigation = ['Home','End'].includes(event.key) || (event.altKey && ['ArrowLeft','ArrowRight'].includes(event.key));
      if (!navigation && !settleAutomationEdit()) return;
      const lane = selectedLane(), points = ordered(lane.points), info = targetInfo(lane);
      if (event.key === 'Insert' || event.key === 'Enter') { addPoint(); return; }
      if (!points.length) return;
      ui.point = clamp(ui.point, 0, points.length - 1);
      if (event.key === 'Home' || event.key === 'End') { ui.point = event.key === 'Home' ? 0 : points.length - 1; updatePointSelection(); return; }
      if (event.altKey
          && (event.key === 'ArrowLeft' || event.key === 'ArrowRight')) { ui.point = clamp(ui.point + (event.key === 'ArrowRight' ? 1 : -1),
          0, points.length - 1); updatePointSelection(); return; }
      const point = points[ui.point];
      if (['Delete', 'Backspace'].includes(event.key)) points.splice(ui.point, 1);
      else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') point.row = clamp(point.row
          + (event.key === 'ArrowRight' ? 1 : -1) * (event.shiftKey ? project().linesPerBeat : 1),
          0, pattern().length - 1);
      else { const delta = (event.key === 'ArrowUp' ? 1 : -1) * (event.shiftKey ? .1 : .01); point.value = fromUnit(clamp(toUnit(point.value, info) + delta,
          0, 1),
          info); }
      const next = normalizedPoints(['Delete', 'Backspace'].includes(event.key) ? points : points.filter((_, i) => i !== ui.point).concat([point]),
          info);
      if (!['Delete', 'Backspace'].includes(event.key)) ui.point = next.findIndex(p => p.row === point.row);
      updatePoints('Edit automation point', next, 'automation-envelope');
    });
  }

  const channelControlId = (role, channel) => role === 'vol' && channel?.type === 'master'
      ? 'mix-master'
      : `mix-${role}-${channel?.token ?? trackIndex()}`;
  function focusMixerControl(id) {
    // refresh() installs the new DOM synchronously; do not defer the next keyboard target.
    if (state().modal) return;
    const node = document.getElementById(id); node?.focus({ preventScroll: true });
    updateMixerIdentity(node);
    node?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' });
  }
  function focusAdjacent(event, selector, keys = ['ArrowUp', 'ArrowDown']) {
    if (![...keys, 'Home', 'End'].includes(event.key)) return false;
    const items = Array.from(document.querySelectorAll(selector)).filter(item => !item.disabled);
    const index = items.indexOf(event.target); if (index < 0) return false;
    const next = event.key === 'Home'
        ? 0
        : event.key === 'End'
        ? items.length - 1
        : clamp(index + (event.key === keys[1] ? 1 : -1), 0, items.length - 1);
    event.preventDefault(); items[next]?.focus(); items[next]?.scrollIntoView?.({ block: 'nearest', inline: 'nearest' }); return true;
  }
  function key(event) {
    if (event.defaultPrevented) return true;
    if (window.STRouting?.key?.(event)) return true;
    if (state().view === 'mixer'
        && event.key === 'Escape'
        && ui.pluginPicker) { event.preventDefault(); ui.pluginPicker = false; S().refresh(); focusId(channelControlId('slots',
            channelFor(selectedOwner()))); return true; }
    const effectHeader = event.target?.closest?.('.studio-plugin > summary');
    if (state().view === 'mixer'
        && effectHeader
        && event.altKey
        && !event.metaKey
        && !event.ctrlKey
        && !event.shiftKey
        && ['ArrowUp', 'ArrowDown'].includes(event.key)) { event.preventDefault(); moveEffect(Number(effectHeader.parentElement.dataset.pluginSlot),
            event.key === 'ArrowUp' ? -1 : 1); return true; }
    if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return false;
    // The pattern menu takes [ and ] too (a select would only type-ahead on them).
    if (state().view === 'automation'
        && event.target?.id === 'automation-pattern'
        && (event.key === '[' || event.key === ']')) { event.preventDefault(); stepPattern(event.key === ']' ? 1 : -1); return true; }
    const target = event.target, tag = target?.tagName || '';
    const editing = target?.isContentEditable
        || /TEXTAREA|SELECT/.test(tag)
        || tag === 'INPUT' && !['range', 'checkbox', 'button'].includes(target.type);
    if (editing) return false;
    if (state().view === 'mixer') {
      if (focusAdjacent(event, '[data-add-plugin]')) return true;
      const summary = target?.closest?.('.studio-plugin > summary');
      if (summary) {
        if (event.key === 'ArrowLeft'
            || event.key === 'ArrowRight') { event.preventDefault(); summary.parentElement.open = event.key === 'ArrowRight'; return true; }
        if (focusAdjacent(event, '.studio-plugin > summary')) return true;
      }
      {
        const channelNode = target?.closest?.('[data-mixer-owner]'),
          role = target?.dataset?.mixerRole
              || /^mix-(track|mute|solo|vol|pan|delay|reverb|slots|output|sends)-/.exec(target?.id || '')?.[1]
              || (target?.id === 'mix-master' ? 'vol' : 'track');
        const surface = channelNode || ['mixer-channels','mixer-workspace','main'].includes(target?.id) || target === document.body;
        if (surface && (['ArrowLeft','ArrowRight'].includes(event.key) || ['Home','End'].includes(event.key) && target?.type !== 'range')) {
          const list = visibleChannels(),
            source = Math.max(0, list.findIndex(channel => channel.owner === (channelNode?.dataset.mixerOwner || selectedOwner())));
          const next = event.key === 'Home'
              ? 0
              : event.key === 'End'
              ? list.length - 1
              : clamp(source + (event.key === 'ArrowRight' ? 1 : -1), 0, list.length - 1);
          event.preventDefault(); selectOwner(list[next].owner); S().refresh();
          const desired = channelControlId(role,list[next]); focusMixerControl(document.getElementById(desired)
              ? desired
              : channelControlId('track',list[next])); return true;
        }
      }
    }
    if (state().view === 'automation') {
      if (event.key === '[' || event.key === ']') { event.preventDefault(); stepPattern(event.key === ']' ? 1 : -1); return true; }
      if (event.key === '\\' && window.STAutomationSong) { event.preventDefault(); setScope(ui.scope === 'song' ? 'pattern' : 'song'); return true; }
      const laneEntry = ui.scope === 'song' ? 'automation-song-envelope' : 'automation-envelope';
      if (target?.dataset?.selectLane !== undefined
          && event.key === 'ArrowUp') { event.preventDefault(); const lane = selectedLane(); (document.getElementById('automation-target-' + lane?.target) || document.getElementById('automation-search'))?.focus(); return true; }
      if (target?.dataset?.chooseTarget !== undefined
          && event.key === 'ArrowRight'
          && ui.scope === 'song'
          && document.getElementById(laneEntry)) { event.preventDefault(); document.getElementById(laneEntry).focus(); return true; }
      if (target?.dataset?.chooseTarget !== undefined
          && event.key === 'ArrowRight'
          && selectedLane()) { event.preventDefault(); document.getElementById('automation-envelope')?.focus(); return true; }
    }
    return false;
  }

  function nativeEvent(type, payload) {
    if (type === 'status') {
      updateAutomationPlayback(payload);
      if (ui.scope === 'song' && state().view === 'automation') window.STAutomationSong?.paintPlayhead?.(payload);
      ui.peaks = payload.trackPeaks || []; ui.busPeaks = payload.busPeaks || {};
      document.querySelectorAll('[data-bus-meter]').forEach(meter => {
        const peak = clamp(Math.max(...(ui.busPeaks[meter.dataset.busMeter] || [0])), 0, 1);
        meter.style.transform = `scaleY(${peak})`;
        meter.classList.toggle('is-clipping', peak >= .98);
      });
      document.querySelectorAll('[data-group-meter]').forEach(meter => {
        const peak = clamp(Math.max(...(ui.busPeaks[meter.dataset.groupMeter] || [0])), 0, 1);
        meter.style.transform = `scaleX(${peak})`;
        meter.classList.toggle('is-clipping', peak >= .98);
      });
      ui.peak = clamp(Math.max(number(payload.peakL), number(payload.peakR)), 0, 1);
      document.querySelectorAll('[data-track-meter]').forEach(meter => {
        const peak = clamp(ui.peaks[Number(meter.dataset.trackMeter)], 0, 1);
        meter.style.transform = `scaleY(${peak})`;
        meter.classList.toggle('is-clipping', peak >= .98);
      });
      document.querySelectorAll('[data-master-meter]').forEach(meter => {
        meter.style.transform = `scaleY(${ui.peak})`;
        meter.classList.toggle('is-clipping', ui.peak >= .98);
      });
    }
    window.STAutomationRecord?.event?.(type, payload);
    window.STRouting?.event(type,payload);
    if (type === 'project.loaded') {
      ui.pluginGeneration++;
      ui.previewSerial++;
      clearTimeout(ui.previewTimer);
      for (const timer of ui.parameterTimers.values()) clearTimeout(timer);
      ui.parameterTimers.clear();
      ui.parameters.clear();
      ui.parameterOwners.clear();
      ui.parameterErrors.clear();
      ui.openPlugins.clear();
      ui.point = 0;
      ui.pluginPicker = false;
      ui.ref = null;
    }
  }

  // Quick "Automate parameter…" picker data for a tracker track: existing lanes first, then the rest.
  function automationChoices(index) {
    const owner = normalizeOwner(index), existing = lanes().filter(lane => ownerForLane(lane) === owner);
    const targets = allTargets(owner);
    for (const lane of existing) if (!targets.some(target => target.target === lane.target)) targets.push(targetInfo(lane));
    return targets.map(target => { const lane = existing.find(item => item.target === target.target); return { target: target.target, name: target.name, group: target.group || '', search: target.search || '', value: target.value, available: target.available !== false, unavailableReason: target.unavailableReason, automated: !!lane, points: lane?.points.length || 0 }; })
      .sort((a, b) => Number(b.automated) - Number(a.automated));
  }
  window.STStudio = {
    automationChoices,
    loadAutomationParameters: index => ensureParameters(normalizeOwner(index)),
    selectedOwner,
    selectOwner,
    channels,
    get isEditingPlugin() {
      return ui.pluginEditing > 0;
    },
    get isPreparingEffectMove() {
      return ui.pluginMoving && !ui.effectMoveDone;
    },
    focusSelectedChannel: () => focusMixerControl(channelControlId('track', channelFor(selectedOwner()))),
    get isMovingEffect() {
      return !!ui.effectMoveDone;
    },
    waitForEffectMove: () => ui.effectMoveDone?.promise || Promise.resolve(),
    renderMixer,
    bindMixer,
    renderAutomation,
    bindAutomation,
    refreshLiveAutomation,
    key,
    command,
    switchPattern,
    setScope,
    get automationScope() {
      return ui.scope;
    },
    event: nativeEvent,
    target: (track, target) => allTargets(track).find(item => item.target === target),
    laneInfo: lane => targetInfo(lane),
    channelTitle
  };
  /* Public insert slot API by owner ('track:<id>', 'bus:<id>', 'master') and slot number.
     Each edit is one Undo step and resolves after the audio host and the project agree. */
  window.STInserts = {
    SLOTS: INSERT_SLOTS,
    chain: insertChain,
    at: insertAt,
    freeSlot: owner => freeSlot(normalizeOwner(owner)),
    get busy() { return !insertsIdle(); },
    ownerOf: plugin => ownerForPlugin(plugin),
    channelName: owner => channelFor(owner)?.name || '',
    add: addInsert,
    // options.label names the Undo step (the AI API passes "AI: …").
    remove: (owner, slot, options) => removeInsert(owner, slot, undefined, options),
    setBypass: (owner, slot, bypass, options) => bypassInsert(owner, slot, bypass, undefined, options),
    toggleBypass: (owner, slot) => bypassInsert(owner, slot),
    move: (owner, fromSlot, toSlot, options) => moveInsert(owner, fromSlot, toSlot, options),
    duplicate: duplicateInsert,
    editor: (owner, slot) => S().request('plugin.editor', { ...ownerPayload(normalizeOwner(owner)), kind: 'effect', slot: Number(slot) }),
    catalogue: async () => {
      if (!ui.plugins) { const response = await S().request('plugins.list', {}); ui.plugins = Array.isArray(response) ? response : response?.plugins || []; }
      return ui.plugins;
    }
  };
})();
