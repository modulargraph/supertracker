/* Track Freeze / Unfreeze. Freezing prints a track's instrument and inserts to
   audio (native `freeze.track`): the song, plus every pattern the track uses
   (first pass and looping pass, so Pattern mode plays it too). Its plug-ins are
   released; the notes, instrument and plug-in state stay in the project under
   `tracks[i].frozen`. Unfreeze restores the live chain. Each is one Undo step. */
(() => {
  'use strict';
  const app = () => window.ST, state = () => app().state, project = () => state().project;
  const MIX = new Set(['volume', 'pan', 'delay', 'reverb']);
  const { clone } = window.STUtil;
  const esc = value => app()?.escapeHTML ? app().escapeHTML(String(value)) : String(value).replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
  let busy = false, statusByID = new Map(), statusTimer = null, lastStatusKey = '', observer = null;

  const trackName = index => project()?.tracks?.[index]?.name || `Track ${index + 1}`;
  const isFrozen = index => !!project()?.tracks?.[index]?.frozen;
  const ownerOf = track => track?.routingID ? 'track:' + track.routingID : null;

  /* Long native request: freezing renders the whole song offline, which can
     outlast app.js's default 15 s request timeout. */
  function nativeRequest(command, payload) {
    const st = state();
    if (!st?.native || !st.pending || !window.webkit?.messageHandlers?.supertracker) return app().request(command, payload);
    return new Promise((resolve, reject) => {
      const id = ++st.requestId;
      st.pending.set(id, { resolve, reject, timeout: null, command });
      window.webkit.messageHandlers.supertracker.postMessage({ id, command, payload });
    });
  }

  // ---- Protection ----------------------------------------------------------------------

  /** Everything printed into track `index` (mix, names and colours excluded). */
  function signature(p, index) {
    const track = p.tracks[index];
    if (!track) return null;
    const owner = ownerOf(track), instruments = new Set([track.instrument || 0]);
    const sequenced = new Set(p.sequence || [0]);
    const patterns = (p.patterns || []).map((pattern, position) => {
      const id = pattern.id ?? position;
      const rows = {};
      (pattern.steps?.[index] || []).forEach((step, row) => {
        if (!step) return;
        const copy = { ...step };
        if (copy.instrument !== undefined && copy.instrument >= 0) instruments.add(copy.instrument);
        for (const cell of copy.columns || []) if (cell?.instrument >= 0) instruments.add(cell.instrument);
        for (const key of ['fx1', 'fx2']) if (copy[key] && MIX.has(copy[key].type)) delete copy[key];
        if (Object.keys(copy).length) rows[row] = copy;
      });
      const lanes = (pattern.automation || []).filter(lane => (lane.owner ? lane.owner === owner : (lane.track || 0) === index) &&
        !(MIX.has(lane.target || 'volume') || String(lane.target || '').startsWith('send:'))).map(({ id, name, ...rest }) => rest);
      if (!sequenced.has(id) && !Object.keys(rows).length && !lanes.length) return null;
      // Sequenced patterns are in the song print; patterns with content have their own print.
      return { id, length: pattern.length, rows, lanes };
    }).filter(Boolean);
    const strip = ({ name, color, id, volume, pan, mute, solo, delaySend, reverbSend, frozen, selected, collapsed, viewState, ...rest }) => rest;
    const plugins = (p.audioPlugins || []).filter(plugin => plugin.owner ? plugin.owner === owner : (plugin.track || 0) === index)
      .map(({ state: _s, parameters, latency, latencyFrames, tailTime, stateError, name, editorOpen, windowFrame, ...rest }) => rest);
    return JSON.stringify({
      track: strip(track), patterns, plugins,
      instruments: [...instruments].sort((a, b) => a - b).map(i => p.instruments?.[i] ? (({
        name,
        id,
        plugin,
        ...rest
      }) => rest)(p.instruments[i]) : null),
      clips: (p.audioClips || []).filter(clip => (clip.track || 0) === index).map(({ id, name, ...rest }) => rest),
      timing: { bpm: p.bpm, lpb: p.linesPerBeat, swing: p.swing, timing: p.timing || null, sequence: p.sequence || [0] }
    });
  }

  /** Returns null, or {message, track, name} when `after` changes printed content. */
  function checkMutation(before, after, label, options = {}) {
    if (options.freezeCommit || !before?.tracks?.some(track => track.frozen)) return null;
    for (const [index, track] of before.tracks.entries()) {
      if (!track.frozen) continue;
      const name = track.name || `Track ${index + 1}`;
      const next = after.tracks.findIndex(t => t.frozen?.id === track.frozen.id);
      if (next < 0) {
        const still = after.tracks.some(t => (t.routingID && t.routingID === track.routingID) || t === track);
        return { track: index, name, message: still
            ? `${name} is frozen. Use Unfreeze to change its freeze.`
            : `${name} is frozen. Unfreeze it before removing the track.` };
      }
      if (signature(before, index) !== signature(after, next)) {
        const timing = JSON.stringify([before.bpm, before.linesPerBeat, before.swing, before.timing, before.sequence]) !==
          JSON.stringify([after.bpm, after.linesPerBeat, after.swing, after.timing, after.sequence]);
        return { track: index, name, message: timing
          ? `Tempo and the song arrangement are printed into frozen track ${name}. Unfreeze it to change them.`
          : `${name} is frozen: its notes, instruments and inserts are printed to audio. Unfreeze to edit them; fader, pan, mute, solo and sends stay live.` };
      }
    }
    return null;
  }

  function explain(blocked) {
    if (!blocked) return;
    const a = app();
    if (!a.openModal || state().modal) { a.toast(blocked.message, 'error'); return; }
    a.openModal('Track is frozen', `<p class="st-freeze-copy">${esc(blocked.message)}</p>`,
      `<button id="st-freeze-explain-cancel">Keep frozen</button><button class="primary" id="st-freeze-explain-unfreeze">Unfreeze ${esc(blocked.name)}…</button>`, { kind: 'freeze-explain' });
    const cancel = document.getElementById('st-freeze-explain-cancel'), go = document.getElementById('st-freeze-explain-unfreeze');
    if (cancel) cancel.onclick = () => a.closeModal();
    if (go) { go.onclick = () => { a.closeModal(); unfreeze(blocked.track); }; go.focus(); }
  }

  /* Guard edits that go through ST.mutate (modules). app.js's internal mutate()
     needs the one-line hook listed in the integration notes for full coverage. */
  function installMutateGuard() {
    const a = app();
    if (!a || a.mutate?.stFreezeGuard) return;
    const original = a.mutate;
    const guarded = function (label, fn, options = {}) {
      const p = state()?.project;
      if (options.freezeCommit || !p?.tracks?.some(track => track.frozen)) return original(label, fn, options);
      let blocked = null;
      const undoLength = state().undo.length;
      const result = original(label, draft => {
        const before = clone(draft);
        fn(draft);
        blocked = checkMutation(before, draft, label, options);
        if (blocked) { for (const key of Object.keys(draft)) delete draft[key]; Object.assign(draft, before); }
      }, options);
      if (blocked) {
        if (state().undo.length > undoLength) state().undo.pop();
        a.refresh?.();
        explain(blocked);
      }
      return result;
    };
    guarded.stFreezeGuard = true;
    a.mutate = guarded;
  }

  // ---- Commands --------------------------------------------------------------------------

  /* Hardware tracks (live line input = the synth's return, or an external MIDI
     instrument) cannot render offline: they freeze by recording one real-time pass. */
  function hardwareTrack(index) {
    const p = project(), track = p?.tracks?.[index];
    return track?.audioInput?.enabled === true || p?.instruments?.[track?.instrument || 0]?.type === 'midi';
  }
  const songSeconds = p => (p.sequence || [0]).reduce((sum, id) => {
    const pattern = (p.patterns || []).find((item, position) => (item.id ?? position) === id);
    return sum + (pattern?.length || 64) / (p.linesPerBeat || 4) * 60 / (p.bpm || 120);
  }, 0);
  const minutes = seconds => `${Math.floor(seconds / 60)}:${String(Math.round(seconds % 60)).padStart(2, '0')}`;
  let progressText = null;

  function freeze(index = state().track) {
    const a = app(), p = project();
    if (busy) { a.toast('A track freeze is already in progress.'); return; }
    if (!p?.tracks?.[index]) return;
    if (isFrozen(index)) return unfreeze(index);
    const name = trackName(index), hardware = hardwareTrack(index);
    const songTime = songSeconds(p), patterns = patternCount(index), usedPatterns = parseInt(patterns, 10) || 0;
    const patternTime = (p.patterns || []).reduce((sum, item) => sum + 3 * (item.length || 64) / (p.linesPerBeat || 4) * 60 / (p.bpm || 120), 0)
        * (usedPatterns / Math.max(1, (p.patterns || []).length));
    a.openModal(`Freeze ${name}`, `<div class="stack st-freeze-dialog">
      <p class="st-freeze-copy">Prints this track’s song performance and each pattern it plays, with its inserts, so it keeps sounding in Song `
          + `and Pattern mode. Fader, pan, mute, solo and downstream sends stay live. Its plug-ins are released to save CPU. Unfreeze to edit its sound `
          + `or notes, or to audition the instrument.</p>
      ${hardware ? `<p class="st-freeze-copy"><strong>External hardware.</strong> This track’s sound comes back through a line input, so it is frozen by `
          + `<em>recording</em>: the song plays once in real time (about ${minutes(songTime + 3)}) and the return is captured in time with the mix. `
          + `While frozen, its editor and line input are released and the synth receives no MIDI from this track, so it is free for another part. `
          + `Keep the synth’s sound and the transport untouched while recording.</p>
      <label class="check"><input id="st-freeze-patterns" type="checkbox" checked> Also record pattern prints (about ${minutes(patternTime + usedPatterns * 6)} more)</label>
      <label class="check"><input id="st-freeze-monitor" type="checkbox" checked> Listen while recording</label>` : ''}
      <dl class="st-freeze-facts"><dt>Track</dt><dd>${esc(name)}</dd><dt>Range</dt><dd>Entire song · ${patterns}</dd></dl>
      <label class="check"><input id="st-freeze-auto" type="checkbox" checked> Auto tail: end each print when it falls silent</label>
      <label class="field"><span id="st-freeze-tail-label">Longest tail · seconds</span><input id="st-freeze-tail" type="number" min="0" max="30" step=".5" value="10"></label>
      <div id="st-freeze-status" class="st-freeze-status" role="status" tabindex="-1"></div></div>`,
      `<button id="st-freeze-cancel">Cancel</button><button id="st-freeze-stop" hidden>Stop playback</button><button class="primary"`
      + ` id="st-freeze-go">${hardware ? 'Record and freeze' : 'Freeze track'}</button>`, { kind: 'freeze' });
    const get = id => document.getElementById(id);
    if (!get('st-freeze-go')) return;
    const status = get('st-freeze-status'), go = get('st-freeze-go'), stopButton = get('st-freeze-stop'), auto = get('st-freeze-auto');
    auto.onchange = () => {
      get('st-freeze-tail-label').textContent = auto.checked ? 'Longest tail · seconds' : 'Tail · seconds';
      if (!auto.checked && get('st-freeze-tail').value === '10') get('st-freeze-tail').value = '3';
    };
    const syncPlaying = () => {
      const playing = !!state().playing;
      stopButton.hidden = !playing; go.disabled = playing || busy;
      if (playing && !busy) status.textContent = 'Stop playback before freezing.';
      else if (!busy && status.textContent === 'Stop playback before freezing.') status.textContent = '';
      return playing;
    };
    syncPlaying();
    const watcher = setInterval(() => { if (state().modal !== 'freeze') { clearInterval(watcher); return; } syncPlaying(); }, 250);
    get('st-freeze-cancel').onclick = () => { if (!busy) a.closeModal(); };
    stopButton.onclick = () => { a.perform('stop'); setTimeout(syncPlaying, 150); };
    go.onclick = async () => {
      if (syncPlaying()) return;
      const field = get('st-freeze-tail'), tail = Number(field.value);
      if (!field.value.trim()
          || !Number.isFinite(tail)
          || tail < 0
          || tail > 30) { status.textContent = 'Choose a tail between 0 and 30 seconds.'; field.focus(); return; }
      busy = true; go.disabled = true; get('st-freeze-cancel').disabled = true; field.disabled = true; auto.disabled = true;
      status.textContent = hardware ? 'Recording the hardware return in real time…' : 'Capturing plug-in state and printing the song and its patterns…'; status.focus();
      progressText = text => { if (state().modal === 'freeze') status.textContent = text; };
      const owner = project();
      // Mutations edit the project in place: the print belongs to this track object at this index, not to whatever moves there.
      const placed = (target => () => project() === owner && owner.tracks.length === target.count && owner.tracks[index] === target.track)({ track: owner.tracks[index], count: owner.tracks.length });
      const payload = auto.checked ? { track: index, maxTail: tail } : { track: index, tail };
      if (hardware) Object.assign(payload, { method: 'record', patterns: get('st-freeze-patterns').checked, monitor: get('st-freeze-monitor').checked });
      try {
        await a.waitForEngineUpdate?.();
        const result = await nativeRequest('freeze.track', payload);
        if (!result?.record) throw new Error('The audio engine did not return a print.');
        if (!placed()) throw new Error('The session changed while the track was printing. The print was not applied.');
        // Fresh plug-in state belongs to the baseline (not a separate Undo step).
        if (Array.isArray(result.audioPlugins)) {
          owner.audioPlugins = result.audioPlugins;
          window.STNKSControls?.adoptPlugins?.(result.audioPlugins);
        }
        a.mutate(`freeze ${name}`, draft => { draft.tracks[index].frozen = result.record; }, { freezeCommit: true });
        busy = false; progressText = null; a.closeModal();
        const released = result.releasedPlugins || 0, skipped = result.unprintedPatterns?.length || 0;
        a.toast(`${name} frozen${released ? ` · ${released} plug-in${released === 1 ? '' : 's'} released` : ''}.` +
          (skipped
              ? ` ${skipped} pattern${skipped === 1 ? ' was' : 's were'} not printed and stay silent in Pattern mode.`
              : ' Unfreeze to edit.') + (result.notes?.length ? ' ' + result.notes.join(' ') : ''), skipped ? 'error' : 'info');
        scheduleStatus(0);
      } catch (error) {
        busy = false; progressText = null;
        if (state().modal === 'freeze') {
          status.textContent = error.message || String(error);
          go.disabled = false; get('st-freeze-cancel').disabled = false; field.disabled = false; auto.disabled = false; status.focus();
        } else a.toast(error.message || String(error), 'error');
      }
    };
    get('st-freeze-tail').focus();
  }

  /** Native `freeze.progress` {track, progress 0…1, phase, recording}. */
  function event(type, payload = {}) {
    if (type !== 'freeze.progress' || !busy || !progressText) return;
    const percent = Math.round(Math.max(0, Math.min(1, Number(payload.progress) || 0)) * 100);
    progressText(`${payload.recording ? 'Recording' : 'Printing'} ${payload.phase || 'song'} · ${percent}%`
        + (payload.recording ? ' · press Stop to cancel' : ''));
  }

  function unfreeze(index = state().track) {
    const a = app();
    if (busy) { a.toast('A track freeze is already in progress.'); return; }
    if (!isFrozen(index)) { a.toast(`${trackName(index)} is not frozen.`); return; }
    const name = trackName(index);
    a.openModal(`Unfreeze ${name}`, `<div class="stack"><p class="st-freeze-copy">Restore original notes, sources and inserts; keep your current mix. The printed file stays `
        + `on disk for Undo.</p>
      <div id="st-freeze-status" class="st-freeze-status" role="status" tabindex="-1"></div></div>`,
      `<button id="st-freeze-cancel">Cancel</button><button class="primary" id="st-freeze-go">Unfreeze track</button>`, { kind: 'freeze' });
    const get = id => document.getElementById(id);
    if (!get('st-freeze-go')) return;
    const status = get('st-freeze-status'), go = get('st-freeze-go');
    get('st-freeze-cancel').onclick = () => { if (!busy) a.closeModal(); };
    go.onclick = async () => {
      if (state().playing) { status.textContent = 'Stop playback before unfreezing.'; return; }
      busy = true; go.disabled = true; get('st-freeze-cancel').disabled = true;
      status.textContent = 'Reloading plug-ins with their saved state…'; status.focus();
      const owner = project(), track = owner.tracks[index], count = owner.tracks.length;
      try {
        await a.waitForEngineUpdate?.();
        const result = await nativeRequest('freeze.unfreeze', { track: index });
        if (project() !== owner || owner.tracks.length !== count || owner.tracks[index] !== track) throw new Error('The session changed while the track was restoring.');
        a.mutate(`unfreeze ${name}`, draft => { delete draft.tracks[index].frozen; }, { freezeCommit: true });
        busy = false; a.closeModal();
        const problems = result?.problems || [];
        a.toast(problems.length
            ? `${name} unfrozen, but ${problems.join(', ')} did not reload. Check the plug-in.`
            : `${name} unfrozen · live plug-ins restored.`,
            problems.length ? 'error' : 'info');
        scheduleStatus(0);
      } catch (error) {
        busy = false;
        if (state().modal === 'freeze') { status.textContent = error.message || String(error); go.disabled = false; get('st-freeze-cancel').disabled = false; }
        else a.toast(error.message || String(error), 'error');
      }
    };
    go.focus();
  }

  const toggle = (index = state().track) => isFrozen(index) ? unfreeze(index) : freeze(index);

  /** Patterns with printable content for the track (dialog summary). */
  function patternCount(index) {
    const p = project(), track = p?.tracks?.[index], owner = ownerOf(track);
    const used = (p?.patterns || []).filter(pattern => (pattern.steps?.[index] || []).some(step => step && Object.keys(step).some(key =>
      !((key === 'fx1' || key === 'fx2') && MIX.has(step[key]?.type)))) || (pattern.automation || []).some(lane =>
      (lane.owner ? lane.owner === owner : (lane.track || 0) === index)
          && !(MIX.has(lane.target || 'volume') || String(lane.target || '').startsWith('send:')))).length;
    if (p?.timing) return 'patterns not printed (tempo map)';
    return `${used} pattern${used === 1 ? '' : 's'}`;
  }

  /* Audition / computer-keyboard notes on a frozen track: its instrument is
     printed and its plug-ins are released, so explain instead of playing a
     silent or dry note. The native host refuses the same notes with this text. */
  const auditionMessage = index => `${trackName(index)} is frozen: its instrument is printed to audio and its plug-ins are released. Unfreeze it to audition or play notes.`;
  function blockAudition(index = state().track) {
    if (!isFrozen(index)) return false;
    app().toast(auditionMessage(index));
    return true;
  }

  // ---- Status and labels ----------------------------------------------------------------

  function label(index) {
    const track = project()?.tracks?.[index];
    if (!track?.frozen) return '';
    const status = statusByID.get(track.frozen.id);
    if (status?.available === false) return 'Frozen · unavailable';
    if (status?.stale) return 'Frozen · source changed';
    if (status?.patternPlayback === false) return 'Frozen · song only';
    return 'Frozen';
  }

  function scheduleStatus(delay = 400) {
    clearTimeout(statusTimer);
    statusTimer = setTimeout(async () => {
      if (!state()?.native || !project()?.tracks?.some(track => track.frozen)) { statusByID.clear(); decorate(); return; }
      try {
        const result = await app().request('freeze.status', {});
        statusByID = new Map((result?.tracks || []).filter(row => row.id).map(row => [row.id, row]));
      } catch { /* status is advisory */ }
      decorate();
    }, delay);
  }

  function decorate() {
    const p = project();
    if (!p) return;
    const key = JSON.stringify(p.tracks.map(track => track.frozen?.id || 0));
    if (key !== lastStatusKey) { lastStatusKey = key; scheduleStatus(); }
    document.querySelectorAll('.st-freeze-badge').forEach(node => node.remove());
    p.tracks.forEach((track, index) => {
      if (!track.frozen) return;
      const text = label(index), owner = ownerOf(track);
      const targets = [...document.querySelectorAll(`th[data-track="${index}"]`),
        ...document.querySelectorAll(`[data-mixer-channel="${index}"] .studio-channel-title`),
        ...(owner ? document.querySelectorAll(`[data-mixer-owner="${CSS.escape(owner)}"] .studio-channel-title`) : [])];
      for (const target of targets) {
        const badge = document.createElement('span'); badge.className = 'st-freeze-badge';
        badge.textContent = text; badge.title = `${text}. Right-click or ⌘K → Unfreeze to edit.`;
        target.appendChild(badge);
        badge.dataset.state = text.includes('unavailable') ? 'unavailable' : text.includes('changed') || text.includes('song only') ? 'stale' : 'ok';
        if (text.includes('song only')) badge.title = `${text}: some patterns have no print and are silent in Pattern mode. Unfreeze and freeze again to print them.`;
      }
    });
  }

  function startObserver() {
    const root = document.getElementById('app');
    if (!root || observer) return;
    let queued = false;
    observer = new MutationObserver(() => {
      if (queued) return; queued = true;
      setTimeout(() => {
        queued = false;
        observer.disconnect();
        try {
          decorate();
        } finally {
          observer.observe(root, { childList: true, subtree: true });
        }
      }, 0);
    });
    observer.observe(root, { childList: true, subtree: true });
    decorate();
  }

  // ---- Context menu ---------------------------------------------------------------------

  function trackFromTarget(target) {
    const header = target?.closest?.('th[data-track]');
    if (header) return Number(header.dataset.track);
    const flat = target?.closest?.('[data-mixer-channel]');
    if (flat && !flat.dataset.mixerOwner && /^\d+$/.test(flat.dataset.mixerChannel || '')) return Number(flat.dataset.mixerChannel);
    const owner = target?.closest?.('[data-mixer-owner]')?.dataset?.mixerOwner;
    if (owner?.startsWith('track:')) return (project()?.tracks || []).findIndex(track => ownerOf(track) === owner);
    return -1;
  }

  function contextItems({ target } = {}) {
    const index = trackFromTarget(target);
    if (index < 0 || !project()?.tracks?.[index]) return null;
    return isFrozen(index)
      ? [{ title: 'Unfreeze track…', enabled: !busy, action: () => unfreeze(index) }]
      : [{ title: 'Freeze track…', enabled: !busy, action: () => freeze(index) }];
  }

  const commands = [
    ['freezeTrack', 'Freeze selected track…', '', ['print track', 'reduce processing', 'freeze', 'save cpu']],
    ['unfreezeTrack', 'Unfreeze selected track…', '', ['unfreeze', 'restore plug-ins', 'edit frozen track']]
  ];

  window.STFreeze = {
    freeze, unfreeze, toggle, isFrozen, label, contextItems, checkMutation, explain, commands, refreshStatus: () => scheduleStatus(0),
    blockAudition, auditionMessage, event,
    get isBusy() { return busy; }
  };

  document.addEventListener('DOMContentLoaded', () => {
    installMutateGuard();
    window.STContextMenu?.register(context => contextItems(context), 79);
    startObserver();
  });
  // ST.boot() renders after DOMContentLoaded handlers are registered; retry once rendered.
  window.addEventListener('load', () => { installMutateGuard(); startObserver(); });
})();
