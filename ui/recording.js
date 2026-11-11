/* Named sample / timeline capture, with explicit input metering and guarded destinations. */
(() => {
  'use strict';
  let current = null;
  let pendingInputRestore = null;
  const app = () => window.ST;
  const get = id => document.getElementById(id);
  const esc = value => app().escapeHTML(String(value ?? ''));
  const clamp = (value, low, high) => Math.min(high, Math.max(low, Number(value) || 0));
  const active = session => current === session
      && (session.inline
          ? app().state.view === 'recording' && !app().state.modal && !!get('recording-inline-host')
          : app().state.modal === 'recording');
  const sameProject = session => app().state.project === session.project && session.project.tracks[session.track] === session.trackObject;
  const noteName = midi => ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][midi % 12] + (Math.floor(midi / 12) - 1);
  const db = peak => peak > 0 ? `${Math.max(-96, 20 * Math.log10(peak)).toFixed(1)} dB` : '−∞ dB';
  const createsTrack = session => session.hardwareSample || session.destination === 'newTrack';
  const createsInstrument = session => ['new', 'newTrack'].includes(session.destination);
  const savedInput = session => session.liveInput || (session.hardwareSample && session.instrumentObject.recordingInput);
  const inputDevice = session => (session.audio.inputs || []).find(device => Number(device.id) === session.inputDevice);
  const channelDescription = channels => channels?.length === 1
      ? `Input ${channels[0] + 1} · mono`
      : channels?.length === 2
      ? `Inputs ${channels[0] + 1} / ${channels[1] + 1} · stereo`
      : 'Choose input channels';
  function studioInputChannels(audio) {
    const channels = audio.inputChannels || [0, 1];
    const device = (audio.inputs || []).find(item => Number(item.id) === Number(audio.inputDevice || audio.defaultInput));
    // The host's initial stereo pair is implicit on a one-channel device.
    // Saved per-instrument profiles remain exact and are validated separately.
    return Number(device?.inputChannels) === 1 && channels.length === 2 && channels[0] === 0 && channels[1] === 1 ? [0] : [...channels];
  }
  function inputSummary(instrument) {
    const profile = instrument?.recordingInput;
    return profile?.deviceUID
        ? `${profile.deviceName || 'Saved audio interface'} · ${channelDescription(profile.channels)} · Root ${noteName(instrument.rootNote ?? 60)}`
        : `Current Studio input · no saved interface · Root ${noteName(instrument?.rootNote ?? 60)}`;
  }
  function resolveSavedInput(session) {
    const profile = savedInput(session);
    const devices = session.audio.inputs || [];
    const selected = profile?.deviceUID
        ? devices.find(device => device.uid === profile.deviceUID)
        : devices.find(device => Number(device.id) === Number(session.audio.inputDevice || session.audio.defaultInput));
    session.inputDevice = Number(selected?.id || 0);
    session.inputChannels = profile?.deviceUID
        ? Array.isArray(profile.channels) && [1, 2].includes(profile.channels.length) && profile.channels.every(Number.isInteger)
            ? [...profile.channels]
            : [-1, -1]
        : studioInputChannels(session.audio);
    session.missingSavedInput = !!profile?.deviceUID && !selected;
  }
  function requireInput(session) {
    const device = inputDevice(session);
    if (!device) throw new Error(session.missingSavedInput
        ? `Saved input ${savedInput(session)?.deviceName || 'interface'} is unavailable. Choose an input explicitly before recording.`
        : 'Choose an available audio input before recording.');
    if (![1, 2].includes(session.inputChannels.length)
        || !session.inputChannels.every(channel => Number.isInteger(channel)
            && channel >= 0
            && channel < Number(device.inputChannels))) throw new Error('The saved input channels are unavailable. Choose valid left and right inputs, or mono.');
    return device;
  }
  function rememberHardwareInput(session) {
    if (!session.hardwareSample) return;
    requireDestination(session);
    const device = requireInput(session),
      profile = { deviceUID: device.uid || '', deviceName: device.name || 'Audio input', channels: [...session.inputChannels] };
    if (!profile.deviceUID) throw new Error('This input did not provide a persistent device identity. Refresh the recorder or choose another input before saving hardware setup.');
    const instrument = session.instrumentObject;
    if (JSON.stringify(instrument.recordingInput) === JSON.stringify(profile) && instrument.rootNote === session.rootNote) return;
    app().mutate('Remember hardware input', () => {
      instrument.recordingInput = profile;
      instrument.rootNote = session.rootNote;
    }, { render: false, recordingCommit: true });
  }
  function status(session, message, error = false) {
    session.message = message;
    if (active(session) && get('record-status')) {
      get('record-status').textContent = message;
      get('record-status').classList.toggle('is-error', error);
    }
  }
  function readForm(session) {
    if (!active(session)) return;
    session.name = (get('record-name')?.value || '').trim().slice(0, 100);
    session.rootNote = Math.round(clamp(get('record-root-note')?.value, 0, 127));
    session.source = session.hardwareSample ? 'input' : get('record-source')?.value || 'input';
    session.destination = session.takeTarget ? 'take' : session.hardwareSample ? 'new' : get('record-destination')?.value || 'new';
    session.inputDevice = Number(get('record-input-device')?.value || 0);
    session.inputChannels = get('record-input-right')?.value === 'mono'
        ? [Number(get('record-input-left')?.value || 0)]
        : [Number(get('record-input-left')?.value || 0), Number(get('record-input-right')?.value || 0)];
    session.monitor = !!get('record-monitor')?.checked; session.playMode = get('record-play-mode')?.value || 'pattern';
    session.startPlayback = !!get('record-start-playback')?.checked;
  }
  function controls(session) {
    if (!active(session)) return;
    const locked = session.loading || session.busy || session.recording || !!session.saved;
    for (const id of ['record-name', 'record-root-note', 'record-source', 'record-destination', 'record-input-device', 'record-input-left',
        'record-input-right', 'record-monitor', 'record-start-playback', 'record-play-mode']) if (get(id)) get(id).disabled = locked;
    // Naming does not depend on device discovery. Keep the initial keyboard
    // destination available while the asynchronous input list is loading.
    get('record-name').disabled = session.busy || session.recording || !!session.saved;
    if (session.hardwareSample) { get('record-source').disabled = true; get('record-destination').disabled = true; }
    if (session.takeTarget) get('record-destination').disabled = true;
    get('record-input-settings').hidden = session.source !== 'input';
    get('record-root-note').disabled = locked || session.destination === 'clip' || session.destination === 'take';
    get('record-monitor').disabled = locked || session.source !== 'input';
    get('record-meter-enable').hidden = session.source !== 'input';
    get('record-meter-enable').disabled = session.busy || session.recording || session.loading || !app().state.native;
    get('record-meter-enable').textContent = session.metering ? 'Stop input check' : 'Check input levels';
    get('record-start').disabled = session.busy || session.loading || !app().state.native;
    get('record-start').textContent = session.busy
        ? session.recording ? 'Finishing…' : 'Please wait…'
        : session.recording
        ? '■ Stop & load · ⌘⇧R'
        : session.saved
        ? 'Load saved recording · ⌘⇧R'
        : session.emptyFile
        ? '● Retry recording · ⌘⇧R'
        : '● Start recording · ⌘⇧R';
    get('record-close').disabled = session.busy || session.recording;
    if (session.inline) for (const id of ['recording-input-settings', 'recording-routing-configure',
        'recording-studio-settings']) if (get(id)) get(id).disabled = session.busy || session.recording || session.metering;
    get('record-playback').disabled = session.busy || !app().state.native;
    get('record-playback').textContent = app().state.playing ? 'Pause playback' : 'Play selection';
    if(get('record-advanced-summary'))get('record-advanced-summary').textContent=session.monitor&&session.source==='input'?'Monitor on':'Monitor off';
    get('record-meter-label').textContent = session.source === 'master' ? 'Master output' : 'Audio input';
    if (get('record-saved')) {
      get('record-saved').hidden = !(session.saved || session.emptyFile)?.path;
      get('record-saved-path').value = (session.saved || session.emptyFile)?.path || '';
    }
  }
  function fillDevices(session) {
    const inputs = session.audio.inputs || [], selected = session.inputDevice;
    get('record-input-device').innerHTML = (selected
        ? ''
        : `<option value="">${inputs.length ? 'Choose audio input…' : 'No audio input found'}</option>`)
        + inputs.map(device => `<option value="${Number(device.id)}" ${String(device.id) === String(selected) ? 'selected' : ''}>${esc(device.name)}</option>`).join('');
    get('record-input-device').value = selected ? String(selected) : '';
    fillChannels(session, session.inputChannels);
    inputStatus(session);
  }
  function inputStatus(session) {
    if (!get('record-input-summary')) return;
    const profile = savedInput(session);
    get('record-input-summary').textContent = session.missingSavedInput
        ? `Saved input ${profile.deviceName || 'interface'} is unavailable. Select an input explicitly.`
        : session.hardwareSample
        ? profile?.deviceUID
            ? `Saved input for ${session.instrumentObject.name}. Changes and root note are remembered when recording starts.`
            : 'Using the current Studio input. Choose an interface and channels; they will be remembered for this synth when recording starts.'
        : 'Choose one input for mono, or two inputs for stereo.';
  }
  function fillChannels(session, selected) {
    const device = (session.audio.inputs || []).find(item => String(item.id) === get('record-input-device').value);
    const count = Math.max(0, Math.min(128, Number(device?.inputChannels) || 0));
    const left = selected?.[0] ?? 0, right = selected?.length === 1 || !selected && count === 1 ? 'mono' : String(selected?.[1] ?? 1);
    const options = Array.from({ length: count }, (_, i) => `<option value="${i}">Input ${i + 1}</option>`).join('');
    const unavailable = value => Number(value) < 0
        ? `<option value="${esc(value)}">Choose input…</option>`
        : Number.isInteger(Number(value)) && Number(value) >= 0 && Number(value) < count
        ? ''
        : `<option value="${esc(value)}">Unavailable input ${Number(value) + 1}</option>`;
    get('record-input-left').innerHTML = options + unavailable(left); get('record-input-left').value = String(left);
    get('record-input-right').innerHTML = '<option value="mono">Mono · use left</option>' + options + (right === 'mono' ? '' : unavailable(right));
    get('record-input-right').value = right;
  }
  async function configureInput(session, metering = session.metering) {
    requireInput(session);
    if(session.inline&&(!session.configured||session.restored)){
      const value=await app().request('audio.devices',{});if(!active(session))throw new Error('The recording workspace changed.');
      if (value) {
        session.initialMetering = !!value.inputMetering;
        session.initialMonitor = !!(value.monitoring || value.monitor);
        session.initialMonitorVolume = value.monitorVolume ?? .7;
        session.initialInputDevice = value.inputDevice || value.defaultInput;
        session.initialInputChannels = studioInputChannels(value);
      }
    }
    const payload = {
      inputChannels: session.inputChannels,
      inputMeter: metering,
      monitor: session.monitor,
      monitorVolume: session.audio.monitorVolume ?? .7
    };
    if (Number.isFinite(session.inputDevice) && session.inputDevice > 0) payload.inputDevice = session.inputDevice;
    // Native configuration can change the device before a later channel check fails.
    // Every attempt owns one restoration, including a retry after a restored attempt.
    session.configured = true; session.restored = false; session.restoreAttempted = false; session.restoreError = null;
    const audio = await app().request('audio.configure', payload);
    if (audio && active(session)) { session.audio = { ...session.audio, ...audio }; app().state.audio = { ...app().state.audio, ...audio }; }
  }
  async function checkInput(session) {
    if (!active(session) || session.busy || session.recording) return;
    readForm(session); session.busy = true; controls(session);
    try {
      await configureInput(session, !session.metering);
      if (!active(session)) return;
      session.metering = !session.metering;
      status(session, session.metering ? 'Play your synth to check its input level.' : 'Input check stopped.');
    } catch (error) {
      const restoreError = await restoreInput(session);
      status(session, `${error.message || String(error)}${restoreError ? ` ${restoreError}` : ''}`, true);
    }
    finally { session.busy = false; controls(session); }
  }
  async function prepareProject(session) {
    const snapshot = () => {
      if (!active(session)) throw new Error('The recording window changed. Reopen it before starting audio.');
      requireDestination(session);
      return window.STUtil.clone(session.project);
    };
    if (await app().syncEngineProject(snapshot) === false) throw new Error('Audio preparation was canceled. Recording and playback were not started.');
    snapshot();
  }
  async function playSelection(session, prepared = false) {
    if (!active(session) || !sameProject(session)) return;
    if (app().state.playing) { await app().request('transport.stop', {}); app().state.playing = false; }
    else {
      if (!prepared) await prepareProject(session);
      if (!active(session) || !sameProject(session)) return;
      const song = session.playMode === 'song', pattern = song ? session.project.sequence[session.sequence] : session.pattern;
      await app().request('transport.seek', { pattern, row: song ? 0 : session.row, sequence: session.sequence });
      if (!active(session) || !sameProject(session)) return;
      await app().request('transport.play', { mode: session.playMode }); app().state.playing = true;
    }
    controls(session);
  }
  function requireDestination(session) {
    if (window.STBusy.any(['session', 'export', 'routing'])) throw new Error('Finish the current project operation. Your recording has not been attached.');
    if (!sameProject(session)) throw new Error('The project or destination track changed. The recording has not been attached.');
    if (session.takeTarget) window.STClipTakes.requireTarget(session.takeTarget);
    if (!session.hardwareSample
        && session.destination === 'new') throw new Error('Choose a new sampler instrument and track, or explicitly replace an instrument.');
    if (session.hardwareSample) {
      if (session.project.instruments[session.instrument] !== session.instrumentObject
          || session.instrumentObject.type !== 'midi'
          || session.trackObject.instrument !== session.sourceAssignment) throw new Error('The hardware instrument or its track assignment changed. The recording has not been attached.');
    }
    if (createsTrack(session)
        && session.project.tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity)) throw new Error(`Sampling into a new track needs a new track. This project already has ${session.project.tracks.length} tracks. Free a track before `
        + `recording or loading the saved take.`);
    if (session.destination === 'clip'
        && (session.project.audioClips || []).length >= 256) throw new Error('This project already has 256 audio clips. Free a clip slot before recording.');
    if (session.destination === 'current'
        && (session.project.instruments[session.instrument] !== session.instrumentObject || session.trackObject.instrument !== session.sourceAssignment)) throw new Error('The destination instrument changed. The recording has not been attached.');
  }
  async function attach(session) {
    requireDestination(session);
    const file = session.saved;
    if (file && Object.hasOwn(file, 'frames') && Number(file.frames) === 0) {
      session.emptyFile = file;
      session.saved = null;
      throw new Error('No audio captured. Check the input and retry recording. The empty capture file is preserved.');
    }
    const captureWarning = session.captureWarning || (file?.partial ? file.error || 'The recording was interrupted.' : '');
    if (!file?.path) throw new Error('No capture file was returned by the audio host.');
    if (session.destination === 'take') {
      await window.STClipTakes.addFile(file.path, session.takeTarget, session.name);
      app().state.view = 'song';
    } else if (session.destination === 'clip') {
      const metadata = await app().request('audio.clipInfo', { path: file.path }); requireDestination(session);
      const duration = Number(metadata?.duration || file.duration);
      if (!(duration > 0)) throw new Error('The recording contains no audio frames.');
      const durationBeats = window.STProjectTiming.endingBeat(session.project, session.startBeat, duration) - session.startBeat;
      const clip = {
        id: app().uid(),
        name: session.name,
        track: session.track,
        path: file.path,
        startBeat: session.startBeat,
        durationBeats,
        sourceDuration: duration,
        sourceOffset: 0,
        gain: 1,
        fadeIn: 0,
        fadeOut: 0,
        muted: false
      };
      let applied = false;
      app().mutate('Record audio clip', p => { p.audioClips ||= []; p.audioClips.push(clip); applied = true; },{recordingCommit:true});
      if (!applied
          || !sameProject(session)
          || !session.project.audioClips?.includes(clip)) throw new Error('The recording edit was not applied. Your capture file is available below.');
      app().state.clipSelected = clip.id; app().state.view = 'song';
    } else {
      if (await app().captureNativeEdits?.() === false) throw new Error('Could not preserve the current plug-in settings. Your capture file is available below.');
      requireDestination(session);
      const index = createsInstrument(session) ? session.project.instruments.length : session.instrument;
      const sample = await app().request('sample.load', { path: file.path, instrument: index });
      // sample.load changes the native instrument as well as decoding the file.
      // Any subsequent rejected attachment must restore canonical native state.
      if (sample?.path) session.removedPlugin = true;
      requireDestination(session);
      if (!sample?.path) throw new Error('The host could not decode the capture file.');
      if (createsInstrument(session)
          && session.project.instruments.length !== index) throw new Error('The instrument list changed while loading. Your capture file is available below.');
      const hosted = (session.project.audioPlugins || []).find(plugin => plugin.track === session.track && plugin.kind === 'instrument');
      if (hosted && !createsTrack(session)) { await app().request('plugin.remove',
          { track: session.track, kind: 'instrument', slot: hosted.slot || 0 }); session.removedPlugin = true; requireDestination(session); }
      let attachedInstrument, attachedTrack;
      app().mutate('Record sampler instrument', p => {
        if (createsInstrument(session)) p.instruments.push(app().normalize({
          instruments: [{ id: index, type: 'sample', name: session.name }]
        }).instruments[0]);
        const instrument = p.instruments[index];
        Object.assign(instrument, {
          type: 'sample',
          plugin: null,
          name: session.name,
          samplePath: sample.path,
          rootNote: session.rootNote,
          mode: 'oneshot',
          zones: [],
          slices: [],
          slicePoints: [],
          start: 0,
          end: 1,
          loopStart: 0,
          loopEnd: 1
        });
        if (createsTrack(session)) {
          const trackIndex = p.tracks.length;
          const track = {
            id: trackIndex,
            name: session.name,
            color: session.trackObject.color || '#88b5a5',
            instrument: index,
            volume: 1,
            pan: 0,
            mute: false,
            solo: false,
            delaySend: 0,
            reverbSend: 0,
            plugins: []
          };
          window.STRoutingProject.appendTrack(p,track,app().uid);
          for (const pattern of p.patterns || []) { pattern.steps ||= []; pattern.steps.push(Array.from({length:pattern.length},()=>null)); }
          attachedTrack = track;
        } else {
          const removed = new Set((p.audioPlugins || []).filter(plugin => plugin.track === session.track
              && plugin.kind === 'instrument').map(plugin => plugin.instanceID).filter(Boolean));
          p.audioPlugins = (p.audioPlugins || []).filter(plugin => !(plugin.track === session.track && plugin.kind === 'instrument'));
          if (p.routing.sidechains) p.routing.sidechains = p.routing.sidechains.filter(send => !removed.has(send.destination?.instanceID));
          session.trackObject.instrument = index; attachedTrack = session.trackObject;
        }
        attachedInstrument = instrument;
      },{recordingCommit:true});
      if (!attachedInstrument
          || !sameProject(session)
          || session.project.instruments[index] !== attachedInstrument
          || !session.project.tracks.includes(attachedTrack)) throw new Error('The recording edit was not applied. Your capture file is available below.');
      app().state.track = session.project.tracks.indexOf(attachedTrack); app().state.instrument = index;
      app().state.samples ||= {}; app().state.samples[index] = sample; app().state.view = 'instrument';
    }
    session.saved = null; session.removedPlugin = false;
    await restoreInput(session); session.busy = false; if (!session.inline && !active(session)) return;
    if(session.inline){if(current===session)current=null;}else app().closeModal(); app().refresh();
    if (!['clip', 'take'].includes(session.destination)) {
      get('param-value-start')?.scrollIntoView?.({ block: 'center' });
      get('param-value-start')?.focus({ preventScroll: true });
    }
    app().toast(session.destination === 'take'
        ? 'Recording added as the active take. Other alternatives are retained.'
        : session.destination === 'clip'
        ? 'Recording added to the song timeline.'
        : `${session.name} loaded${createsTrack(session) ? ' on a new sampler track' : ''} · root ${noteName(session.rootNote)}.`,
        'success');
    if (captureWarning) app().toast(`Partial recording loaded; the captured audio was preserved. ${captureWarning}`, 'error');
  }
  async function record(session) {
    if (!active(session) || session.busy || session.loading || !app().state.native) return;
    if (!session.saved && !session.recording) {
      readForm(session);
      if (!session.name) { status(session, 'Give this recording a name.', true); get('record-name').focus(); return; }
    }
    rememberForm(session);session.busy = true; controls(session);
    try {
      if (session.saved) { await attach(session); return; }
      if (!session.recording) {
        requireDestination(session);
        if (session.source === 'input') requireInput(session);
        rememberHardwareInput(session);
        await prepareProject(session);
        session.startBeat = app().songBeat();
        if (session.source === 'input') await configureInput(session);
        requireDestination(session);
        const result = await app().request('audio.record',
            { source: session.source, name: session.name, monitor: session.source === 'input'
                && session.monitor, monitorVolume: session.audio.monitorVolume ?? .7 });
        if (!result?.recording) throw new Error('The audio host did not start recording.');
        session.recording = true; session.captureWarning=''; session.capturePath = result.path || '';
        if (!active(session) || !sameProject(session)) {
          const saved = await app().request('audio.stopRecord', {}); session.recording = false; session.saved = saved;
          throw new Error(`The session changed before recording started. Capture preserved at ${saved.path || session.capturePath}`);
        }
        status(session, 'Recording. Play your note, then choose Stop & load.');
        if (session.startPlayback && !app().state.playing) await playSelection(session, true);
      } else {
        app().stopHeld();
        let result;
        try { result = await app().request('audio.stopRecord', {}); }
        catch (error) {
          // Native publishes its finalized file before rejecting an interrupted stop.
          // Only that authoritative result can authorize loading the preserved prefix.
          if (!session.saved?.path || session.recording || !(Number(session.saved.frames) > 0 || Number(session.saved.duration) > 0)) throw error;
          session.captureWarning = session.saved.error || error.message || String(error);
          result = session.saved;
        }
        session.recording = false;
        session.saved = result?.path
            ? { ...session.saved, ...result }
            : session.saved || (session.capturePath ? { path: session.capturePath, duration: 0 } : null);
        await attach(session);
      }
    } catch (error) {
      let message = session.emptyFile && !session.saved && !session.recording
          ? 'No audio captured. Check the input and retry recording. The empty capture file is preserved.'
          : error.message || String(error);
      if (session.removedPlugin) {
        session.removedPlugin = false;
        try { await app().syncEngineProject(() => window.STUtil.clone(app().state.project), {replace:true}); }
        catch (restoreError) { message += ` Could not restore the audio project: ${restoreError.message || restoreError}`; }
      }
      if (!session.recording) {
        const restoreError = await restoreInput(session);
        if (restoreError) message += ` ${restoreError}`;
      }
      status(session, message, true);
    }
    finally { session.busy = false; controls(session); }
  }
  async function restoreInput(session) {
    if (!session.configured || session.restored) return null;
    if (session.restoring) return session.restoring;
    if (session.restoreAttempted) return session.restoreError;
    session.restoreAttempted = true;
    const payload = { inputMeter: session.initialMetering, monitor: session.initialMonitor, monitorVolume: session.initialMonitorVolume };
    if (session.initialInputDevice) { payload.inputDevice = session.initialInputDevice; payload.inputChannels = session.initialInputChannels; }
    const restoring = (async () => {
      try {
        const result = await app().request('audio.configure', payload);
        if (result) { session.audio = { ...session.audio, ...result }; app().state.audio = { ...app().state.audio, ...result }; }
        session.restored = true; session.metering = session.initialMetering;
        return null;
      } catch (error) {
        session.restoreError = `Could not restore the previous audio input settings: ${error.message || error}. Check Studio audio settings before continuing.`;
        app().toast(session.restoreError, 'error');
        return session.restoreError;
      }
    })();
    session.restoring = restoring; pendingInputRestore = restoring;
    try { return await restoring; }
    finally { session.restoring = null; if (pendingInputRestore === restoring) pendingInputRestore = null; }
  }
  function closed() {
    const session = current; if (!session || session.busy || session.recording) return false;
    current = null; app().stopHeld(); restoreInput(session); return true;
  }
  function event(type, payload = {}) {
    const session = current; if (!session || !active(session)) return;
    if (type !== 'status' && type !== 'audio.recording') return;
    if(type==='audio.recording' && payload.recording===false && (!session.capturePath || payload.path && payload.path!==session.capturePath)) return;
    if (payload.recording === true) session.seenRecording = true;
    if (type === 'audio.recording' && payload.path && payload.recording) session.capturePath = payload.path;
    if (payload.recording === false && (type === 'audio.recording' || session.recording && session.seenRecording)) {
      session.recording = false;
      if (type === 'audio.recording' && (payload.path || session.capturePath)) {
        session.saved = {...session.saved,...payload,path:payload.path || session.capturePath};
        if(payload.partial || payload.error) session.captureWarning=payload.error || 'The recording was interrupted.';
        if (Number.isFinite(Number(payload.duration))
            && get('record-duration')) get('record-duration').textContent = `${Number(payload.duration).toFixed(1)} s`;
        if(Object.hasOwn(payload,'frames') && Number(payload.frames)===0){session.emptyFile=session.saved;session.saved=null;status(session,
            'No audio captured. Check the input and retry recording. The empty capture file is preserved.',true);}
        else if(session.captureWarning) status(session,`Capture preserved. Load the saved recording to recover its audio. ${session.captureWarning}`,
            true);
      }
      controls(session);
    }
    session.meterState ||= {};
    for (const key of ['peakL', 'peakR', 'inputPeakLeft', 'inputPeakRight',
        'recordingDuration']) if (payload[key] !== undefined) session.meterState[key] = payload[key];
    const left = Number(session.source === 'master' ? payload.peakL : payload.inputPeakLeft),
      right = Number(session.source === 'master' ? payload.peakR : payload.inputPeakRight);
    for (const [side, peak] of [['left', left], ['right', right]]) if (Number.isFinite(peak)) {
      const bar = get(`record-meter-${side}`);
      if (bar) {
        bar.style.transform = `scaleX(${clamp(peak, 0, 1)})`;
        bar.classList.toggle('clipped', peak >= 1);
      }
      if (get(`record-level-${side}`)) get(`record-level-${side}`).textContent = db(peak);
    }
    if (session.recording
        && Number.isFinite(Number(payload.recordingDuration))) get('record-duration').textContent = `${Number(payload.recordingDuration).toFixed(1)} s`;
    if (get('record-playback')) get('record-playback').textContent = app().state.playing ? 'Pause playback' : 'Play selection';
  }
  async function show(options = {}) {
    if (current?.busy || current?.recording) return false;
    if (pendingInputRestore) { await pendingInputRestore; if (current?.busy || current?.recording) return false; }
    if (current) { const previous = current; previous.busy = true; controls(previous); await restoreInput(previous); previous.busy = false; current = null; }
    const state = app().state, project = state.project;
    const instrument = project.instruments[state.instrument];
    const takeTarget = options.takeTarget || null;
    if (takeTarget) { try { window.STClipTakes.requireTarget(takeTarget); } catch (error) { app().toast(error.message, 'error'); return false; } }
    const hardwareSample = !takeTarget && options.hardwareSample === true;
    const targetTrack = takeTarget ? Number(takeTarget.clip.track) : state.track;
    if (hardwareSample && instrument?.type !== 'midi') { app().toast('Select an External MIDI instrument before sampling hardware.',
        'error'); return false; }
    const liveInput = !takeTarget && project.tracks[targetTrack]?.audioInput?.enabled
        ? window.STUtil.clone(project.tracks[targetTrack].audioInput)
        : null;
    const audio = state.audio || {}, channels = studioInputChannels(audio);
    const session = {
      inline: options.inline === true,
      project,
      liveInput,
      hardwareSample,
      takeTarget,
      track: targetTrack,
      trackObject: project.tracks[targetTrack],
      sourceAssignment: project.tracks[state.track]?.instrument,
      instrument: state.instrument,
      instrumentObject: instrument,
      pattern: project.activePattern,
      sequence: state.sequence || 0,
      row: state.row || 0,
      audio,
      source: options.source === 'master' && !hardwareSample ? 'master' : 'input',
      destination: takeTarget ? 'take' : hardwareSample ? 'new' : 'newTrack',
      name: takeTarget
          ? `${takeTarget.clip.name || 'Audio'} take ${window.STClipTakesProject.read(takeTarget.clip).takes.length + 1}`
          : `${instrument?.name || 'Synth'} take`,
      rootNote: instrument?.rootNote ?? 60,
      inputDevice: audio.inputDevice || audio.defaultInput,
      inputChannels: channels,
      monitor: !hardwareSample && !!(audio.monitoring || audio.monitor),
      initialMonitor: !!(audio.monitoring || audio.monitor),
      initialMonitorVolume: audio.monitorVolume ?? .7,
      initialMetering: !!audio.inputMetering,
      initialInputDevice: audio.inputDevice || audio.defaultInput,
      initialInputChannels: [...channels],
      metering: !!audio.inputMetering,
      busy: false,
      recording: false,
      loading: true,
      saved: null,
      configured: false,
      restored: false
    };
    resolveSavedInput(session);
    current = session;
    const body = `<div class="recording-content"><div class="recording-grid"><label class="field"><span>Recording name</span>`
        + `<input id="record-name" maxlength="100" value="${esc(session.name)}" autocomplete="off"></label><label class="field">`
        + `<span>Root note</span><select id="record-root-note">${Array.from({ length: 128 },
            (_, note) => `<option value="${note}" ${note === session.rootNote ? 'selected' : ''}>${noteName(note)} · MIDI ${note}</option>`).join('')}</select>`
        + `</label><label class="field"><span>Source</span><select id="record-source"><option value="input" ${session.source === 'input' ? 'selected' : ''}>Audio `
        + `interface input</option><option value="master" ${session.source === 'master' ? 'selected' : ''}>Master output / resample</option>`
        + `</select></label><label class="field"><span>After recording</span><select id="record-destination">${takeTarget ? '<option value="take" selected>Alternative for selected audio region</option>' : ''}<option value="${hardwareSample ? 'new' : 'newTrack'}" selected>New sampler instrument + track</option>`
            + `<option value="current">Replace instrument · ${esc(instrument?.name || 'Instrument')} (${state.instrument + 1})</option>`
            + `<option value="clip" ${session.destination === 'clip' ? 'selected' : ''}>Audio clip in song</option></select></label></div>
      <div class="recording-options recording-keys"><label title="Same as Keys on/off in the toolbar · ⌘P"><input id="record-keyboard"`
          + ` type="checkbox" ${state.keyboardPlay ? 'checked' : ''}>Play notes with computer keys</label></div>
      <div id="record-input-settings" class="recording-input"><label class="field"><span>Input device</span><select id="record-input-device">`
          + `</select></label><div class="recording-grid"><label class="field"><span>Left / mono</span><select id="record-input-left">`
          + `</select></label><label class="field"><span>Right</span><select id="record-input-right"></select></label>`
          + `</div><p id="record-input-summary" class="small-note" role="status"></p></div>
      <div class="recording-meter"><div class="recording-meter-heading"><span id="record-meter-label">Audio input</span>`
          + `<button id="record-meter-enable">Check input levels</button></div>${['left', 'right'].map((side, index) => `<div class="recording-meter-row"><span>${index ? 'R' : 'L'}</span><div class="recording-meter-well"><i id="record-meter-${side}">`
          + `</i></div><output id="record-level-${side}">−∞ dB</output></div>`).join('')}</div>
      <details class="recording-advanced" id="record-advanced" ${session.inline ? '' : 'open'}><summary>Playback &amp; monitoring <span id="record-advanced-summary">Monitor `
          + `off</span></summary><div class="recording-advanced-body"><div class="recording-playback"><label class="field">`
          + `<span>Playback selection</span><select id="record-play-mode"><option value="pattern" ${state.view !== 'song' ? 'selected' : ''}>Pattern `
          + `from cursor</option><option value="song" ${state.view === 'song' ? 'selected' : ''}>Song from selected position</option>`
          + `</select></label><button id="record-playback">${state.playing ? 'Pause playback' : 'Play selection'}</button>`
          + `</div><div class="recording-options"><label><input id="record-start-playback" type="checkbox" ${session.source === 'master' && !state.playing ? 'checked' : ''}>Start `
          + `playback with recording</label><label><input id="record-monitor" type="checkbox" ${session.monitor ? 'checked' : ''}>Monitor input while `
          + `recording</label></div></div></details>
      <div id="record-saved" class="recording-saved" hidden><label class="field"><span>Capture file</span><input id="record-saved-path"`
          + ` readonly></label><button id="record-reveal">Show in Finder</button></div></div>`;
    const hint = takeTarget ? `Record at least ${window.STClipTakes.timing(project)(takeTarget.clip.startBeat || 0, takeTarget.clip.durationBeats).toFixed(2)} seconds `
        + `for this region. Stop & load retains the previous take. Use Manage takes → Source offset to align the recording; capture is started `
        + `and stopped manually.` : hardwareSample ? 'The hardware instrument and its track stay intact. Stop & load creates a new sampler track, ready to trim and chop.' : 'Stop & load opens the recorded sample or timeline clip.';
    const footer = `<div class="recording-status-row"><div id="record-status" role="status">${state.native
        ? 'Choose the note you will play as the root note.'
        : 'Recording requires the SuperTracker app.'}</div>`
        + `<output id="record-duration" aria-label="Recorded duration">0.0 s</output></div><button id="record-close">Close</button>`
        + `<button id="record-start" class="primary">● Start recording · ⌘⇧R</button>`;
    session.markup = body + `<div class="recording-inline-foot">${footer}</div>`;
    if(session.inline){const host=get('recording-inline-host');if(!host){current=null;return false;}host.innerHTML=session.markup;}
    else app().openModal(takeTarget ? 'Record an alternative take' : hardwareSample ? `Sample ${instrument.name}` : 'Record audio',
        `<p class="small-note">${hint} <kbd>⌘⇧R</kbd> starts or stops capture.</p>` + body, footer,
        { kind: 'recording', className: 'recording-modal' });
    bindSession(session);
    setTimeout(() => { if (active(session) && !session.inline) { get('record-name')?.focus(); get('record-name')?.select(); } }, 0);
    try { const value = await app().request('audio.devices', {}); if (active(session) && value) { session.audio = value; session.initialMetering = session.metering = !!value.inputMetering; session.initialMonitor = !!(value.monitoring || value.monitor); session.initialMonitorVolume = value.monitorVolume ?? .7; session.initialInputDevice = value.inputDevice || value.defaultInput; session.initialInputChannels = studioInputChannels(value); app().state.audio = value; resolveSavedInput(session); fillDevices(session); window.STLineInputs?.updateRecordingSummary?.(); } }
    catch (error) { status(session, error.message || String(error), true); }
    finally { session.loading = false; controls(session); }
    return true;
  }
  function bindSession(session){
    const state=app().state;
    fillDevices(session);
    for (const [id, value] of [['record-name', session.name], ['record-root-note', session.rootNote], ['record-source', session.source],
        ['record-destination', session.destination]]) if (get(id)) get(id).value = String(value);
    if (session.form) {
      for (const [id, value] of Object.entries(session.form)) {
        const field = get(id);
        if (id !== 'record-keyboard' && field) {
          if (typeof value === 'boolean') {
            field.checked = value;
          } else {
            field.value = value;
          }
        }
      }
    }
    // The checkbox is the toolbar's Keys on/off: it always mirrors and toggles state.keyboardPlay.
    const keys = get('record-keyboard');
    if (keys) {
      keys.checked = !!state.keyboardPlay;
      keys.addEventListener('change', () => {
        if (keys.checked !== !!state.keyboardPlay) app().perform('keyboard');
        keys.checked = !!state.keyboardPlay;
      });
    }
    controls(session);
    if(session.message)status(session,session.message);
    if(session.meterState)event('status',session.meterState);
    if(session.inline)get('record-close').textContent='Reset setup';
    if (get('record-advanced')) {
      get('record-advanced').open = session.advancedOpen ?? !session.inline;
      get('record-advanced').addEventListener('toggle', () => {
        session.advancedOpen = !!get('record-advanced')?.open;
      });
    }
    get('record-source').addEventListener('change', () => {
      readForm(session);
      if (session.source === 'master' && !state.playing) get('record-start-playback').checked = true;
      controls(session);
    });
    get('record-destination').addEventListener('change', () => { readForm(session); controls(session); });
    get('record-input-device').addEventListener('change', () => {
      session.inputDevice = Number(get('record-input-device').value || 0);
      session.missingSavedInput = false;
      fillChannels(session);
      readForm(session);
      inputStatus(session);
    });
    get('record-meter-enable').addEventListener('click', () => checkInput(session));
    get('record-playback').addEventListener('click', async () => {
      if (session.busy) return;
      readForm(session);
      session.busy = true;
      controls(session);
      try {
        await playSelection(session);
      } catch (error) {
        status(session, error.message || String(error), true);
      } finally {
        session.busy = false;
        controls(session);
      }
    });
    get('record-start').addEventListener('click', () => record(session));
    get('record-close').addEventListener('click', () => {
      if (!session.busy && !session.recording) {
        if (session.inline) {
          closed();
          mountInline();
        } else {
          app().closeModal();
        }
      }
    });
    get('record-reveal').addEventListener('click', () => {
      if ((session.saved || session.emptyFile)?.path) app().request('app.external', {
        path: (session.saved || session.emptyFile).path,
        reveal: true
      }).catch(error => status(session, error.message, true));
    });
    for (const id of ['record-name', 'record-root-note', 'record-source', 'record-destination', 'record-input-device', 'record-input-left',
        'record-input-right', 'record-monitor', 'record-play-mode', 'record-start-playback']) for (const type of ['input', 'change']) get(id)?.addEventListener(type,
            () => rememberForm(session));
  }
  function rememberForm(session){
    if(!active(session))return;
    session.advancedOpen=!!get('record-advanced')?.open;
    session.form={};
    for (const id of ['record-name', 'record-root-note', 'record-source', 'record-destination', 'record-input-device', 'record-input-left',
        'record-input-right', 'record-monitor', 'record-play-mode', 'record-start-playback']) {
      const field = get(id);
      if (field) session.form[id] = ['record-monitor', 'record-start-playback'].includes(id) ? !!field.checked : field.value;
    }
    readForm(session);controls(session);
  }
  function mountInline(){
    if(current?.inline&&sameProject(current)){
      const host=get('recording-inline-host');if(!host)return false;
      host.innerHTML=current.markup;bindSession(current);return true;
    }
    return show({inline:true});
  }
  function leaveInline(){
    if(!current?.inline)return true;
    if(current.busy||current.recording){app().toast('Stop and load the recording before leaving Recording.');return false;}
    return closed();
  }
  function toggleCapture(options = {}) {
    if (current && active(current)) return record(current);
    return show({
      ...options,
      hardwareSample: app().state.project.instruments[app().state.instrument]?.type === 'midi'
    });
  }
  function keyboardTarget() {
    const session = current; if (!session || !active(session)) return null;
    try { requireDestination(session); } catch { return {blocked:true}; }
    return { instrument: session.instrument, track: session.track };
  }
  window.STRecording = {
    mountInline,
    leaveInline,
    rememberInline: () => {
      if (current?.inline) rememberForm(current);
    },
    keyboardTarget,
    show,
    event,
    closed,
    toggleCapture,
    inputSummary,
    get lastRecording() {
      return current?.saved || current?.emptyFile || (current?.capturePath ? { path: current.capturePath } : null);
    },
    get inlineActive() {
      return !!current?.inline;
    },
    get checkingInlineInput() {
      return !!current?.inline && current.configured && !current.restored && current.metering;
    },
    get isBusy() {
      return !!current?.busy || !!pendingInputRestore;
    },
    get isRecording() {
      return !!current?.recording;
    }
  };
})();
