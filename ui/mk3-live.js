/* MK3: live expression (pitch bend, CC, aftertouch, sustain) and queued pattern / scene changes. */
(() => {
  'use strict';
  const mk = window.STMK3Parts;
  const { S, st, P, clamp, clamp01, now, prefs, M, flash, scheduleRefresh } = mk;

  // ---- Live expression (native 'midi.expression'): pitch bend, CC, aftertouch, sustain ------------------
  // One stream per kind/track/controller/note, quantized to MIDI resolution and rate-limited to one message
  // per EXPR_MS with a trailing send, so the last value always arrives; flush sends at once (spring-back, pedal).
  const EXPR_MS = 15;
  const exprStreams = new Map();
  // `track` is a track index; the host is told the track's channel (its owner), which stays the track's
  // while a message waits for its turn.
  function expression(kind, value, { track = st().track, controller, note, flush = false } = {}) {
    const q = kind === 'pitchBend' ? Math.round(clamp(Number(value) || 0, -1, 1) * 8191) / 8191 : Math.round(clamp01(value) * 127) / 127;
    const key = [kind, track, controller ?? '', note ?? ''].join(':');
    let stream = exprStreams.get(key);
    if (!stream) { stream = { sent: null, at: -Infinity, pending: null, timer: null }; exprStreams.set(key, stream); }
    if (!stream.pending && stream.sent === q) return false;
    const message = {
      owner: `track:${P().tracks[track]?.routingID}`,
      kind,
      value: q,
      ...(controller != null ? { controller } : {}),
      ...(note != null ? { note } : {})
    };
    const send = payload => {
      stream.sent = payload.value;
      stream.at = now();
      stream.pending = null;
      Promise.resolve(S().request('midi.expression', payload)).catch(() => {});
    };
    if (flush || now() - stream.at >= EXPR_MS) { clearTimeout(stream.timer); stream.timer = null; send(message); return true; }
    stream.pending = message;
    if (!stream.timer) stream.timer = setTimeout(() => {
      stream.timer = null;
      if (stream.pending) send(stream.pending);
    }, Math.max(1, EXPR_MS - (now() - stream.at)));
    return true;
  }
  function padPressure(info, release = false) {
    const mode = prefs.aftertouch || 'channel';
    if (mode === 'off' || info.note == null) return;
    if (mode === 'poly') { expression('polyAftertouch', release ? 0 : info.pressure,
        { track: info.track, note: info.note, flush: release }); return; }
    const value = Math.max(0,
        ...[...M.pads.values()].filter(other => other.note != null
            && other.track === info.track
            && (!release || other !== info)).map(other => other.pressure || 0));
    expression('aftertouch', value, { track: info.track, flush: release && value === 0 });
  }
  function pedal(down) {
    if (down === M.pedal) return;
    M.pedal = down;
    // Pedal up goes to every track that received pedal down, even after a track change.
    const target = st().track, tracks = down ? [target] : [...(M.pedalTracks || [target])];
    M.pedalTracks = down ? new Set(tracks) : null;
    for (const track of tracks) expression('sustain', down ? 1 : 0, { track, flush: true });
  }

  // ---- Queued pattern / scene changes (honoured by the engine at the end of the playing pattern) -----------
  function queueJump(target) {
    const status = st().status || {};
    const queued = {
      ...target,
      at: now(),
      confirmed: false,
      wasThere: target.song ? status.sequence === target.sequence : status.pattern === target.pattern
    };
    M.queued = queued;
    Promise.resolve(S().request('transport.queue', target.song ? { sequence: target.sequence } : { pattern: target.pattern }))
      .then(result => { if (M.queued === queued && result?.queued === false) { M.queued = null; target.immediate?.(); } mk.schedule(); })
      .catch(error => { if (M.queued === queued) M.queued = null; S().toast(error.message, 'error'); mk.schedule(); });
    mk.schedule();
  }
  function clearQueue() {
    if (!M.queued) return;
    M.queued = null;
    Promise.resolve(S().request('transport.queue', { clear: true })).catch(() => {});
    flash('Queue cleared', 600);
  }
  function watchQueue(status = {}) {
    const q = M.queued; if (!q) return;
    if (!status.playing) { M.queued = null; mk.schedule(); return; }
    const pending = q.song ? status.queuedSequence === q.sequence : status.queuedPattern === q.pattern;
    if (pending) { q.confirmed = true; return; }
    const arrived = q.song ? status.sequence === q.sequence : status.pattern === q.pattern;
    if (arrived && (q.confirmed || !q.wasThere)) { M.queued = null; honourQueue(q); return; }
    if (q.confirmed || now() - q.at > 3000) { M.queued = null; mk.schedule(); }
  }
  function honourQueue(q) {
    const p = P(), pat = p.patterns.find(item => item.id === q.pattern);
    if (!q.song && pat && p.activePattern !== pat.id) {
      p.activePattern = pat.id; st().row = Math.min(st().row, pat.length - 1); st().selection = null;
      if (st().patternLoop && st().patternLoop.pattern !== pat.id) {
        st().patternLoop = null;
        Promise.resolve(S().request('transport.loop', { loop: null })).catch(() => {});
      }
      S().syncEngine?.();
    }
    if (Number.isInteger(q.sequence) && q.sequence >= 0) st().sequence = q.sequence;
    scheduleRefresh(); mk.schedule();
  }

  Object.assign(mk, { exprStreams, expression, padPressure, pedal, queueJump, clearQueue, watchQueue, honourQueue });
})();
