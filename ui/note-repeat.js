/* Note repeat and arpeggiator for live computer-keyboard playing (Maschine-style).
   Held keys register their (chord/scale-processed) notes here instead of sounding once;
   a tempo-synced clock retriggers them. It is a performance layer: armed recording keeps
   SuperTracker's exact note entry, so repeated hits are heard but not written. */
(function () {
  'use strict';
  const S = () => window.ST, state = () => S().state;
  const modes = [
    ['repeat', 'Repeat'], ['up', 'Arp up'], ['down', 'Arp down'], ['updown', 'Arp up-down'], ['random', 'Arp random'], ['played', 'Arp as played']
  ];
  const rates = [
    ['1/4', 1], ['1/8', .5], ['1/8T', 1 / 3], ['1/16', .25], ['1/16T', 1 / 6], ['1/32', .125]
  ];
  // gate: share of the rate a repeat sounds (0.1–1); octaves: range of the arpeggio modes (1–4, not Repeat).
  const settings = { enabled: false, mode: 'repeat', rate: '1/16', gate: .5, octaves: 1 };
  const GATE = [.1, 1], OCTAVES = [1, 4];
  const pressed = new Map(); // key -> {notes, track, instrument, owner, velocity, order, project, soundInstrument}
  let sounding = [], timer = null, offTimer = null, step = 0, order = 0;

  const modeName = id => (modes.find(([key]) => key === id) || modes[0])[1];
  const rateBeats = id => (rates.find(([key]) => key === id) || rates[3])[1];
  function tempo() { const p = state().project; return Number(p?.timing?.tempos?.[0]?.bpm ?? p?.bpm) || 120; }
  function interval() { return 60000 / tempo() * rateBeats(settings.rate); }

  function send(command, voice) {
    const payload = { track: voice.track, ...(voice.owner ? { owner: voice.owner } : { instrument: voice.instrument }), note: voice.note };
    if (command === 'note.on') payload.velocity = voice.velocity;
    S().request(command, payload).catch(() => {});
  }
  function releaseSounding() {
    clearTimeout(offTimer); offTimer = null;
    for (const voice of sounding) send('note.off', voice);
    sounding = [];
  }
  const voiceID = voice => `${voice.owner || 'track:' + voice.track}:${voice.note}`;
  const current = entry => entry.project === state().project;
  // Unique voices, ordered by pitch, or by press order for "as played".
  function pool() {
    const seen = new Set(), voices = [];
    for (const [key, entry] of pressed) if (!current(entry)) pressed.delete(key);
    const entries = [...pressed.values()].sort((a, b) => a.order - b.order);
    for (const entry of entries) for (const note of entry.notes) {
      const id = voiceID({ ...entry, note });
      if (seen.has(id)) continue;
      seen.add(id); voices.push({ note, track: entry.track, instrument: entry.instrument, owner: entry.owner, velocity: entry.velocity });
    }
    if (settings.mode !== 'played') voices.sort((a, b) => a.note - b.note);
    // Arp octaves: the held notes again one, two or three octaves up (as played: each pass an octave higher).
    if (settings.mode === 'repeat' || settings.octaves <= 1) return voices;
    const spread = [];
    for (let octave = 0; octave < settings.octaves; octave++) for (const voice of voices) {
      const note = voice.note + 12 * octave;
      if (note <= 127) spread.push({ ...voice, note });
    }
    if (settings.mode !== 'played') spread.sort((a, b) => a.note - b.note);
    return spread;
  }
  function choose(voices) {
    if (settings.mode === 'repeat' || voices.length < 2) return voices;
    const n = voices.length;
    if (settings.mode === 'up' || settings.mode === 'played') return [voices[step % n]];
    if (settings.mode === 'down') return [voices[n - 1 - step % n]];
    if (settings.mode === 'random') return [voices[Math.floor(Math.random() * n)]];
    const cycle = 2 * n - 2, position = step % cycle; // up-down without repeating the ends
    return [voices[position < n ? position : cycle - position]];
  }
  /* Repeats sit on the tempo grid (the rate's divisions of the beat), not on when the pad was hit: while the
     transport plays, the grid is the song position; while stopped, a free-running grid at the project tempo. */
  function beatNow() {
    const s = state(), status = s.status || {};
    if (s.playing && Number.isFinite(status.beat) && s.lastStatusAt) return status.beat + (Date.now() - s.lastStatusAt) / 60000 * tempo();
    return performance.now() / 60000 * tempo();
  }
  // Milliseconds to the next grid line; a line closer than a third of the rate is skipped (no flam after a hit).
  function untilNextLine(minimum = 0) {
    const rate = rateBeats(settings.rate), beat = beatNow();
    let next = Math.ceil(beat / rate - 1e-6) * rate;
    if (next - beat < rate * minimum) next += rate;
    return (next - beat) * 60000 / tempo();
  }
  function tick(first = false) {
    releaseSounding();
    const voices = choose(pool());
    if (!pressed.size) { timer = null; return; }
    for (const voice of voices) send('note.on', voice);
    sounding = voices; step++;
    offTimer = setTimeout(releaseSounding, Math.max(10, interval() * settings.gate));
    timer = setTimeout(() => tick(), Math.max(1, untilNextLine(first ? 1 / 3 : .5)));
  }
  // The hit sounds at once; the repeats that follow lock to the grid.
  function start() { stopClock(); step = 0; tick(true); }
  function setVelocity(key, velocity) {
    const entry = pressed.get(key);
    if (entry && Number.isFinite(velocity)) entry.velocity = Math.max(1, Math.min(127, Math.round(velocity)));
  }
  function stopClock() { clearTimeout(timer); timer = null; releaseSounding(); }

  function press(key, entry) {
    const first = !pressed.size;
    pressed.set(key, { ...entry, project: state().project, order: ++order });
    if (first) start();
  }
  function release(key) {
    pressed.delete(key);
    if (!pressed.size) stopClock();
    else {
      const remaining = new Set(pool().map(voiceID));
      sounding = sounding.filter(voice => { if (remaining.has(voiceID(voice))) return true; send('note.off', voice); return false; });
    }
  }
  function stopAll() { pressed.clear(); stopClock(); }

  function changed(announce) {
    if (!settings.enabled) stopAll();
    const app = S();
    if (state().view === 'pattern') app.refresh(); else app.updateTransport();
    if (announce) app.toast(settings.enabled ? `Note repeat on · ${label()}` : 'Note repeat off');
  }
  function label() { return `${modeName(settings.mode)} ${settings.rate}${settings.mode !== 'repeat' && settings.octaves > 1 ? ` ×${settings.octaves} oct` : ''}`; }
  function set(changes, announce = true) {
    Object.assign(settings, changes);
    settings.gate = Math.max(GATE[0], Math.min(GATE[1], Math.round((Number(settings.gate) || .5) * 100) / 100));
    settings.octaves = Math.max(OCTAVES[0], Math.min(OCTAVES[1], Math.round(Number(settings.octaves) || 1)));
    changed(announce);
  }
  function toggle() { set({ enabled: !settings.enabled }); }
  function cycleMode(direction = 1) {
    const index = modes.findIndex(([key]) => key === settings.mode);
    set({ enabled: true, mode: modes[(index + direction + modes.length) % modes.length][0] });
  }
  function cycleRate(direction = 1) {
    const index = rates.findIndex(([key]) => key === settings.rate);
    set({ rate: rates[Math.max(0, Math.min(rates.length - 1, index + direction))][0] });
  }

  function renderControls() {
    const esc = S().escapeHTML;
    return `<label for="note-repeat">Repeat</label><select id="note-repeat" title="Note repeat / arpeggiator for live keyboard playing · ⌥N toggle · ⌥⇧N mode"><option value="off" ${settings.enabled ? '' : 'selected'}>Off</option>${modes.map(([key, name]) => `<option value="${key}" ${settings.enabled && settings.mode === key ? 'selected' : ''}>${esc(name)}</option>`).join('')}</select><select id="note-repeat-rate" aria-label="Note repeat rate" title="Repeat rate · ⌥- slower · ⌥= faster">${rates.map(([key]) => `<option ${settings.rate === key ? 'selected' : ''}>${key}</option>`).join('')}</select>`;
  }
  function bindControls() {
    const mode = document.getElementById('note-repeat'), rate = document.getElementById('note-repeat-rate');
    if (mode) mode.onchange = () => mode.value === 'off' ? set({ enabled: false }, false) : set({ enabled: true, mode: mode.value }, false);
    if (rate) rate.onchange = () => set({ rate: rate.value }, false);
  }

  window.STNoteRepeat = {
    get enabled() { return settings.enabled; }, get mode() { return settings.mode; }, get rate() { return settings.rate; },
    get gate() { return settings.gate; }, get octaves() { return settings.octaves; },
    press, release, setVelocity, stopAll, toggle, cycleMode, cycleRate, set, label, renderControls, bindControls, modes, rates,
    get active() { return pressed.size > 0; }
  };
})();
