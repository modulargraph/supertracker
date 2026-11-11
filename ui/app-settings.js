/* Studio settings (audio and MIDI devices), and the export and recording dialogs. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, esc, state, toast, request, refresh, openModal, closeModal } = app;
  let settingsOpening = 0;
  app.settingsApplyPending = null;
  // While an Apply is in flight its dialog owns the session; refusals bring it back into focus.
  window.STBusy.register('settings', {
    active: () => !!app.settingsApplyPending?.owns(),
    reason: 'Wait for audio and MIDI settings to finish applying.',
    onRefuse: () => app.settingsApplyPending.focus()
  });

  function latencyNote(info) {
    if (!info) return '';
    const ms = Number(info.milliseconds) || 0;
    return `<span>Plug-in compensation · ${ms.toFixed(2)} ms (${Math.round(Number(info.frames) || 0)} samples)</span>${info.limited ? '<p class="notice" role="alert">The chain exceeds the 2-second compensation limit. Reduce plug-in latency '
        + 'before export.</p>' : ''}`;
  }

  async function showSettings() {
    if (window.STBusy.refuse('dialog', ['settings'])) return false;
    const invocation = ++settingsOpening,
      intent = ++app.modalIntent,
      owner = state.project,
      session = app.recordArmSession,
      view = state.view;
    const mayOpen = () => invocation === settingsOpening
        && intent === app.modalIntent
        && state.project === owner
        && app.recordArmSession === session
        && state.view === view
        && !app.sessionTransition;
    try {
      const results = await Promise.allSettled([request('audio.devices'), request('midi.devices'), request('midi.clock')]);
      if (!mayOpen()) return false;
      if (results[0].status === 'fulfilled') state.audio = results[0].value;
      if (results[1].status === 'fulfilled') state.midi = results[1].value;
      const clock = results[2]?.status === 'fulfilled' ? results[2].value : null;
      const a = state.audio,
        m = state.midi,
        inputs = a.inputs || [],
        outputs = a.outputs || [],
        mi = m.inputs || [],
        mo = m.outputs || [];
      const inputDevice = a.inputDevice ?? a.defaultInput,
        outputDevice = a.outputDevice ?? a.defaultOutput;
      const option = (value, label, selected = false, disabled = false) => `<option value="${esc(String(value))}" ${selected ? 'selected' : ''} ${disabled ? 'disabled' : ''}>${esc(label)}</option>`;
      const audioOptions = (devices, current, systemDefault) => option('', 'System default (current)', current == null,
          !(Number(systemDefault) > 0))
          + (current != null && !devices.some(x => String(x.id) === String(current))
              ? option(current, `Unavailable device · ${current}`, true)
              : '')
          + devices.map(x => option(x.id, x.name, String(x.id) === String(current))).join('');
      // Native inputIDs/outputID describe connected endpoints, not all saved requests.
      // Omitting untouched selections preserves disconnected and multiple-port routing.
      const inputIDs = Array.isArray(m.inputIDs) ? m.inputIDs : [],
        midiInput = m.allInputs === true ? 'all' : inputIDs.length === 1 ? String(inputIDs[0]) : 'keep';
      const midiOutput = m.outputID == null ? 'keep' : String(m.outputID);
      const midiInputOptions = (midiInput === 'keep'
          ? option('keep',
              inputIDs.length
                  ? `Keep current inputs · ${inputIDs.length} connected`
                  : 'Keep current inputs · none connected',
              true)
          : '')
          + option('all', 'All inputs', midiInput === 'all')
          + option('none', 'None')
          + (midiInput !== 'keep' && midiInput !== 'all' && !mi.some(x => String(x.id) === midiInput)
              ? option(midiInput, `Unavailable input · ${midiInput}`, true)
              : '')
          + mi.map(x => option(x.id, x.name, String(x.id) === midiInput)).join('');
      const midiOutputOptions = (midiOutput === 'keep'
          ? option('keep', 'Keep current output · none connected', true)
          : '')
          + option('none', 'None')
          + (midiOutput !== 'keep' && !mo.some(x => String(x.id) === midiOutput)
              ? option(midiOutput, `Unavailable output · ${midiOutput}`, true)
              : '')
          + mo.map(x => option(x.id, x.name, String(x.id) === midiOutput)).join('');
      const previousApply = $('#settings-apply');
      openModal('Studio settings', `<div class="stack">${!state.native
          ? '<div class="notice">Audio and MIDI configuration is available in the SuperTracker app.</div>'
          : ''}<div `
          + `class="eyebrow">Audio device</div><div class="grid2"><label class="field"><span>Output device</span>`
          + `<select id="audio-output">${audioOptions(outputs, outputDevice, a.defaultOutput)}</select>`
          + `</label><label class="field"><span>Input device</span><select id="audio-input">${audioOptions(inputs, inputDevice, a.defaultInput)}</select>`
          + `</label><label class="field"><span>Buffer size · samples</span><select id="audio-buffer">${(a.bufferSizes || [64, 128, 256, 512, 1024, 2048]).map(n => `<option value="${n}" ${n === (a.bufferSize || 256) ? 'selected' : ''}>${n}</option>`).join('')}</select>`
          + `</label><label class="field"><span>Input monitor level</span><input id="monitor-volume" type="range"`
          + ` min="0" max="1" step=".01" value="${a.monitorVolume ?? .7}"></label></div>`
          + `<div class="grid2"><label class="field"><span>Line input · left / mono</span>`
          + `<select id="input-channel-left">${Array.from({
        length: Math.max(1, inputs.find(x => String(x.id) === String(inputDevice))?.inputChannels || 2)
      }, (_, i) => `<option value="${i}" ${i === (a.inputChannels?.[0] ?? 0) ? 'selected' : ''}>Input ${i + 1}</option>`).join('')}</select>`
          + `</label><label class="field"><span>Line input · right</span><select id="input-channel-right">`
          + `<option value="mono" ${a.inputChannels?.length === 1 ? 'selected' : ''}>Mono · use left input</option>${Array.from({
        length: Math.max(1, inputs.find(x => String(x.id) === String(inputDevice))?.inputChannels || 2)
      }, (_, i) => `<option value="${i}" ${a.inputChannels?.length !== 1 && i === (a.inputChannels?.[1] ?? 1) ? 'selected' : ''}>Input ${i + 1}</option>`).join('')}</select>`
          + `</label></div><label class="row"><input id="audio-monitor" type="checkbox" ${a.monitoring ? 'checked' : ''}> `
          + `Monitor audio input through master</label><div class="muted" style="font-size:11px">Sample rate: ${a.sampleRate || 48000}`
          + ` Hz · System default selects the device the system currently uses. Device changes can briefly interrupt playback.</div>`
          + `<div id="audio-latency" class="muted" style="font-size:11px">${latencyNote(a.latencyCompensation || state.status.latencyCompensation)}</div>`
          + `<div class="eyebrow" style="margin-top:10px">MIDI routing</div><div class="grid2">`
          + `<label class="field"><span>Input device</span><select id="midi-input">${midiInputOptions}</select>`
          + `</label><label class="field"><span>Default output device</span><select id="midi-output">${midiOutputOptions}</select>`
          + `</label><label class="field"><span>Input channel (0 = omni)</span><input id="midi-input-channel"`
          + ` type="number" min="0" max="16" value="${m.inputChannel || 0}"></label><label class="field">`
          + (`<span>Output channel</span><input id="midi-output-channel" type="number" min="1" max="16"`
          + ` value="${m.channel || 1}">`)
          + `</label></div><label class="row"><input id="midi-enabled" type="checkbox" ${m.enabled !== false ? 'checked' : ''}> `
          + `Enable MIDI input to selected track</label><div class="notice info">MIDI port selections are retained until `
          + `you change them, including disconnected ports. For external synths, create an External MIDI instrument and `
          + `choose its output port and channel. Record its audio return from the line input.</div>${globalThis.STMIDIClock?.settingsSection?.(clock) || ''}${globalThis.STMK3?.settingsNote?.() || ''}${globalThis.STAgent?.settingsField?.() || ''}</div>`, '<p id="settings-status" role="status" tabindex="-1" hidden style="margin:0;margin-right:auto;align-self:center;max-width:38ch;font-size:12px">'
          + '</p><button id="settings-cancel">Cancel</button><button class="primary" id="settings-apply">Apply settings</button>', {
        kind: 'settings',
        className: 'studio-settings-modal'
      });
      const audioFields = ['audio-output', 'audio-input', 'audio-buffer', 'monitor-volume', 'input-channel-left',
          'input-channel-right', 'audio-monitor'];
      const midiFields = ['midi-input', 'midi-output', 'midi-input-channel', 'midi-output-channel', 'midi-enabled'];
      const applyButton = $('#settings-apply'),
        cancelButton = $('#settings-cancel');
      globalThis.STAgent?.bindSettings?.();
      globalThis.STMK3?.bindSettings?.();
      if (!applyButton || applyButton === previousApply) return false; // A busy dialog may have refused replacement.
      const ownsModal = () => $('#settings-apply') === applyButton;
      const status = $('#settings-status'),
        dialog = applyButton.closest?.('.modal');
      const controls = Object.fromEntries([...audioFields, ...midiFields].map(id => [id, $('#' + id)]));
      const read = id => id === 'audio-monitor' || id === 'midi-enabled' ? controls[id].checked : controls[id].value;
      const snapshot = () => Object.fromEntries([...audioFields, ...midiFields].map(id => [id, read(id)]));
      const initial = snapshot();
      const device = (raw, fallback) => {
        const value = Number(raw || fallback);
        if (!Number.isInteger(value) || value <= 0) throw new Error('The system default audio device is unavailable. Choose a connected device.');
        return value;
      };
      $('#audio-input').onchange = () => {
        const id = $('#audio-input').value || String(a.defaultInput),
          count = Math.max(1, inputs.find(x => String(x.id) === id)?.inputChannels || 2);
        $('#input-channel-left').innerHTML = Array.from({
          length: count
        }, (_, i) => option(i, `Input ${i + 1}`, i === 0)).join('');
        $('#input-channel-right').innerHTML = option('mono', 'Mono · use left input', count === 1) + Array.from({
          length: count
        }, (_, i) => option(i, `Input ${i + 1}`, count > 1 && i === 1)).join('');
      };
      cancelButton.onclick = () => {
        if (ownsModal() && !applying) closeModal();
      };
      let applying = false;
      applyButton.onclick = async () => {
        if (applying || !ownsModal()) return;
        const submitted = snapshot(),
          changed = id => submitted[id] !== initial[id];
        const locked = [...Object.values(controls), applyButton, cancelButton,
            $('#modal-close')].filter(Boolean).map(control => [control, control.disabled]);
        const pending = { owns: ownsModal, focus: () => status?.focus({ preventScroll: true }) };
        const progress = message => {
          if (!ownsModal()) return;
          if (status) {
            status.hidden = false;
            status.tabIndex = 0;
            status.textContent = message;
          }
          dialog?.setAttribute('aria-busy', 'true');
          pending.focus();
        };
        applying = true;
        app.settingsApplyPending = pending;
        locked.forEach(([control]) => control.disabled = true);
        progress('Applying settings… This operation must finish before closing.');
        try {
          const audio = {},
            midi = {};
          if (changed('audio-output')) {
            audio.outputDevice = device(submitted['audio-output'], a.defaultOutput);
            if (!submitted['audio-output']) audio.outputSystemDefault = true;
          }
          if (changed('audio-buffer')) audio.bufferSize = Number(submitted['audio-buffer']);
          if (['audio-input', 'input-channel-left', 'input-channel-right'].some(changed)) {
            audio.inputDevice = device(submitted['audio-input'], a.defaultInput);
            if (!submitted['audio-input']) audio.inputSystemDefault = true;
            audio.inputChannels = submitted['input-channel-right'] === 'mono'
                ? [Number(submitted['input-channel-left'])]
                : [Number(submitted['input-channel-left']), Number(submitted['input-channel-right'])];
          }
          if (changed('audio-monitor') || changed('monitor-volume')) {
            audio.monitor = submitted['audio-monitor'];
            audio.monitorVolume = Number(submitted['monitor-volume']);
          }
          if (changed('midi-enabled')) midi.enabled = submitted['midi-enabled'];
          if (changed('midi-input-channel')) midi.inputChannel = Number(submitted['midi-input-channel']);
          if (changed('midi-output-channel')) midi.channel = Number(submitted['midi-output-channel']);
          if (changed('midi-input')
              && submitted['midi-input'] !== 'keep') midi.inputID = ['all', 'none'].includes(submitted['midi-input'])
              ? submitted['midi-input']
              : Number(submitted['midi-input']);
          if (changed('midi-output')
              && submitted['midi-output'] !== 'keep') midi.outputID = submitted['midi-output'] === 'none'
              ? 'none'
              : Number(submitted['midi-output']);
          if (Object.keys(audio).length) {
            progress('Applying audio settings… This operation must finish before closing.');
            state.audio = await request('audio.configure', audio);
            audioFields.forEach(id => initial[id] = submitted[id]);
          }
          if (Object.keys(midi).length) {
            progress('Applying MIDI settings… This operation must finish before closing.');
            state.midi = { ...state.midi, ...(await request('midi.configure', midi)) };
            midiFields.forEach(id => initial[id] = submitted[id]);
          }
          const clockChanges = globalThis.STMIDIClock?.changes?.();
          if (clockChanges) {
            progress('Applying MIDI clock settings…');
            globalThis.STMIDIClock.settingsSection(await request('midi.clock', clockChanges));
          }
          if (app.settingsApplyPending === pending) app.settingsApplyPending = null;
          if (ownsModal()) {
            closeModal();
            toast('Settings applied');
            refresh();
          }
        } catch (e) {
          if (ownsModal() && status) {
            status.hidden = false;
            status.tabIndex = -1;
            status.textContent = 'Could not finish applying settings: '
                + e.message
                + ' Review the selections and retry.';
          }
          toast(e.message, 'error');
        } finally {
          applying = false;
          if (app.settingsApplyPending === pending) app.settingsApplyPending = null;
          if (ownsModal()) {
            dialog?.setAttribute('aria-busy', 'false');
            locked.forEach(([control, disabled]) => control.disabled = disabled);
            applyButton.focus({ preventScroll: true });
          }
        }
      };
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  async function showExport() {
    if (window.STBusy.refuse('dialog', ['saving', 'sessionDialog'])) return false;
    return window.STExport?.show();
  }

  function audioCaptureActive() {
    return window.STBusy.isBusy('recording');
  }

  function showRecording(options = {}) {
    return window.STRecording?.show(options);
  }

  Object.assign(app, { latencyNote, showSettings, showExport, audioCaptureActive, showRecording });
})();
