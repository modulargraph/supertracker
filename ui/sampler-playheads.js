/* Live playheads on the sample editor waveform. While the waveform is visible the host polls the engine's
   voice snapshot (~30 Hz) for the selected instrument and sends 'sampler.playheads' events; this module
   draws them on an overlay canvas (requestAnimationFrame, the waveform itself is never redrawn):
     loops / one-shots / slices  a line per voice with a direction arrow (back, forth, ping-pong)
     Wavetable                   the current frame, shaded
     Granular                    the scan position, the grain cloud and every sounding grain as a tick
   Released voices fade with their envelope. Polling stops when the waveform is hidden. */
(() => {
  'use strict';
  const ST = () => window.ST;
  const MODES = ['oneshot', 'forward', 'reverse', 'pingpong', 'slice', 'wavetable', 'granular'];
  const EXTRAPOLATE_MS = 60;
  const live = { watching: null, voices: [], at: 0, previous: new Map(), frame: 0, timer: null, sent: 'off' };

  function selected() {
    const s = ST()?.state;
    return s?.project?.instruments[s.instrument] || null;
  }
  function canvases() {
    const wave = document.getElementById('sampler-wave-canvas'), overlay = document.getElementById('sampler-playhead-canvas');
    return wave && overlay ? { wave, overlay } : null;
  }
  /** Whether the waveform of a sampler with a sample is on screen (instrument view, Sample page, window visible). */
  function visible() {
    const s = ST()?.state, inst = selected(), c = canvases();
    if (!s?.native || s.view !== 'instrument' || !inst || inst.type !== 'sample' || !inst.samplePath || !c) return false;
    if (c.wave.isConnected === false || document.hidden) return false;
    const rects = c.wave.getClientRects?.();
    return !rects || rects.length > 0;
  }

  /** Start, retarget or stop the host's poll to match what is on screen. Cheap; call it after every render. */
  function sync() {
    const want = visible() ? ST().state.instrument : null, key = want === null ? 'off' : `on:${want}`;
    if (key !== live.sent) {
      live.sent = key; live.watching = want; live.voices = []; live.previous.clear(); clear();
      ST().request('sampler.playheads', want === null ? { enabled: false } : { enabled: true, instrument: want }).catch(() => { live.sent = ''; });
    }
    clearTimeout(live.timer);
    // While watching, notice when the page, view or window hides the waveform (no render tells us).
    if (want !== null) live.timer = setTimeout(sync, 400);
  }

  function receive(payload) {
    if (live.watching === null) return;
    const now = performance.now(), voices = (Array.isArray(payload?.voices) ? payload.voices : [])
      .filter(v => v && v.instrument === live.watching && !v.zone && Number(v.frames) > 0);
    // Speed per voice (frames/ms) from the previous snapshot, for smooth motion between ~30 Hz events.
    const next = new Map();
    for (const [index, v] of voices.entries()) {
      const id = `${v.track}:${v.note}:${index}`, before = live.previous.get(id);
      let speed = 0;
      if (before && before.direction === v.direction && v.mode !== 6 && v.mode !== 5) {
        const delta = (v.position - before.position) / Math.max(1, now - before.at);
        if (Math.sign(delta) === Math.sign(v.direction) && Math.abs(delta) * 50 < v.frames * .25) speed = delta;
      }
      v.speed = speed; next.set(id, { position: v.position, direction: v.direction, at: now });
    }
    live.previous = next; live.voices = voices; live.at = now;
    schedule();
  }

  function schedule() {
    if (!live.frame) live.frame = requestAnimationFrame(() => { live.frame = 0; draw(); if (live.voices.length && performance.now() - live.at < 500) schedule(); });
  }

  function clear() {
    const c = canvases();
    c?.overlay.getContext?.('2d')?.clearRect(0, 0, c.overlay.width, c.overlay.height);
  }

  /** Paint the overlay for the latest snapshot (also called after the waveform redraws, e.g. on zoom). */
  function draw() {
    const c = canvases(), inst = selected();
    if (!c || !inst || !c.overlay.getContext) return;
    const ratio = window.devicePixelRatio || 1, width = Math.max(200, Math.round(c.wave.clientWidth || 800)), height = Math.max(80, Math.round(c.wave.clientHeight || 190));
    if (c.overlay.width !== width * ratio || c.overlay.height !== height * ratio) { c.overlay.width = width * ratio; c.overlay.height = height * ratio; }
    c.overlay.style.width = `${width}px`; c.overlay.style.height = `${height}px`;
    const g = c.overlay.getContext('2d'); g.setTransform(ratio, 0, 0, ratio, 0, 0); g.clearRect(0, 0, width, height);
    if (!live.voices.length) return;
    const [from, to] = window.STSampler?.view?.(inst) || [0, 1], span = Math.max(1e-9, to - from), wave = height - 14;
    const x = (frames, total) => (frames / total - from) / span * width;
    const age = Math.min(EXTRAPOLATE_MS, performance.now() - live.at);
    for (const v of live.voices) {
      const total = Number(v.frames), alpha = Math.max(.25, Math.min(1, Number(v.level) || 0)) * (v.releasing ? .6 : 1);
      const mode = MODES[v.mode] || 'oneshot';
      g.globalAlpha = alpha;
      if (mode === 'granular') {
        const left = x(v.windowStart, total), right = x(v.windowEnd, total);
        g.fillStyle = '#5fb6ad'; g.globalAlpha = alpha * .14; g.fillRect(Math.min(left, right), 0, Math.max(1, Math.abs(right - left)), wave);
        for (const [position, level] of v.grains || []) {
          const at = x(position, total), h = Math.max(6, (Number(level) || 0) * wave * .8);
          g.globalAlpha = alpha * (.35 + .65 * (Number(level) || 0)); g.fillStyle = '#9fe6dc';
          g.fillRect(Math.round(at) - 1, (wave - h) / 2, 3, h);
        }
        g.globalAlpha = alpha; line(g, x(v.position, total), wave, '#5fb6ad', 2);
        if (v.direction) arrow(g, x(v.position, total), v.direction, '#5fb6ad');
        continue;
      }
      if (mode === 'wavetable') {
        const left = x(v.windowStart, total), right = x(v.windowEnd, total);
        g.fillStyle = '#f1c56b'; g.globalAlpha = alpha * .28; g.fillRect(left, 0, Math.max(2, right - left), wave);
        g.globalAlpha = alpha; g.strokeStyle = '#f1c56b'; g.lineWidth = 1; g.strokeRect(left + .5, .5, Math.max(2, right - left) - 1, wave - 1);
        continue;
      }
      const position = v.position + (v.speed || 0) * age;
      line(g, x(position, total), wave, '#fff1d2', 2); arrow(g, x(position, total), v.direction, '#fff1d2');
    }
    g.globalAlpha = 1;
  }
  function line(g, at, wave, color, width) {
    g.strokeStyle = color; g.lineWidth = width; g.beginPath(); g.moveTo(Math.round(at) + .5, 0); g.lineTo(Math.round(at) + .5, wave); g.stroke();
  }
  function arrow(g, at, direction, color) {
    const d = direction < 0 ? -1 : 1, y = 7;
    g.fillStyle = color; g.beginPath(); g.moveTo(at + d * 7, y); g.lineTo(at, y - 5); g.lineTo(at, y + 5); g.closePath(); g.fill();
  }

  // Hiding the window stops the poll; showing it again restarts it (no render happens then).
  document.addEventListener?.('visibilitychange', () => sync());
  window.STSamplerPlayheads = { sync, receive, draw, visible, get voices() { return live.voices; }, get watching() { return live.watching; } };
})();
