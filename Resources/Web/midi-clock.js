/* MIDI clock output in Studio settings: Send MIDI clock, and per MIDI output Clock and Start/Stop. Clock sets a
   device's tempo (arpeggiators, synced LFOs and delays) without running its sequencer; Start/Stop also follows
   SuperTracker's transport. Applied with the rest of the settings ('midi.clock'). */
(() => {
  'use strict';
  const esc = value => (window.ST?.escapeHTML || String)(String(value ?? ''));
  let shown = null;

  function settingsSection(clock) {
    shown = clock && Array.isArray(clock.devices) ? clock : null;
    if (!shown) return '';
    const rows = shown.devices.map(device => `<tr><td>${esc(device.name)}${device.offline ? ' <span class="muted">· offline</span>' : ''}</td>`
        + `<td><input type="checkbox" data-midi-clock="clock" data-id="${Number(device.id)}" ${device.clock ? 'checked' : ''} aria-label="Send clock to ${esc(device.name)}"></td>`
        + `<td><input type="checkbox" data-midi-clock="transport" data-id="${Number(device.id)}" ${device.transport ? 'checked' : ''} aria-label="Send Start and Stop to ${esc(device.name)}"></td></tr>`).join('');
    return `<div class="eyebrow" style="margin-top:10px">MIDI clock</div>`
      + `<label class="row"><input id="midi-clock-enabled" type="checkbox" ${shown.enabled ? 'checked' : ''}> Send MIDI clock</label>`
      + (rows ? `<table class="midi-clock-table"><thead><tr><th>Output</th><th>Clock</th><th>Start/Stop</th></tr></thead><tbody>${rows}</tbody></table>`
        : '<div class="muted" style="font-size:11px">No MIDI outputs connected.</div>')
      + `<label class="row">Clock lead <input id="midi-clock-lead" type="number" min="0" max="500" step="1" style="width:70px" `
      + `value="${shown.leadMilliseconds ?? ''}" placeholder="${Number(shown.automaticLeadMilliseconds) || 0}"> ms `
      + `<span class="muted">(empty: automatic, ${Number(shown.automaticLeadMilliseconds) || 0} ms)</span></label>`
      + '<div class="muted" style="font-size:11px">Clock lead sends the clock early, so a device heard through your audio interface plays in time. Raise it if a device sounds late, lower it if it sounds early. '
      + 'Clock sets each device’s tempo (arpeggiators, synced LFOs, delays) without starting its '
      + 'sequencer, and keeps running while stopped. Turn on Start/Stop only for devices whose sequencer should play along.</div>';
  }

  function read(root = document) {
    if (!shown) return null;
    const enabled = root.getElementById?.('midi-clock-enabled')?.checked ?? shown.enabled;
    const leadField = root.getElementById?.('midi-clock-lead'), leadText = leadField ? String(leadField.value).trim() : null;
    const leadMilliseconds = leadText == null ? shown.leadMilliseconds ?? null : leadText === '' ? null : Math.max(0, Math.min(500, Number(leadText) || 0));
    const devices = shown.devices.map(device => {
      const box = kind => root.querySelector?.(`[data-midi-clock="${kind}"][data-id="${Number(device.id)}"]`);
      return { id: Number(device.id), clock: box('clock')?.checked ?? device.clock, transport: box('transport')?.checked ?? device.transport };
    });
    return { enabled, devices, leadMilliseconds };
  }

  /* The midi.clock request for what changed since the section was shown, or null. */
  function changes(root = document) {
    const next = read(root);
    if (!next) return null;
    const devices = next.devices.filter(item => {
      const before = shown.devices.find(device => Number(device.id) === item.id);
      return !before || before.clock !== item.clock || before.transport !== item.transport;
    });
    const leadChanged = (next.leadMilliseconds ?? null) !== (shown.leadMilliseconds ?? null);
    if (next.enabled === !!shown.enabled && !devices.length && !leadChanged) return null;
    return { enabled: next.enabled, devices, ...(leadChanged ? { leadMilliseconds: next.leadMilliseconds } : {}) };
  }

  window.STMIDIClock = { settingsSection, read, changes };
})();
