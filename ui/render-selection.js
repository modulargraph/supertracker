/* Render selection to sample (⌥B, palette, right-click on the selection).
   The selected pattern cells (tracks × rows; a kit is a group of ordinary tracks, so
   selecting its pads works the same) are rendered offline by the host in a private
   graph: only those tracks' notes that start inside the rows, through their own
   instrument, inserts, sends, group bus and returns, and the master when "Through
   master" is chosen. The render starts clean at the first selected row, releases notes
   still held at its end and keeps the tail until it falls below −70 dBFS for 200 ms
   (at most the maximum tail). Live playback is not paused.
   The result is a new sampler instrument (1-Shot, root C-4) whose file lives with the
   recordings and is collected into the project's media on Save. "Replace selection"
   also clears (or keeps and mutes) the originals and places one C-4 trigger at the
   first row on a new track or the first selected track — all one Undo step. */
(() => {
  'use strict';
  const app = () => window.ST, state = () => app().state, project = () => state().project;
  const esc = value => app()?.escapeHTML ? app().escapeHTML(String(value ?? '')) : String(value ?? '').replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
  const pad = value => String(value).padStart(2, '0');
  const RATES = [[48000, '48 kHz (project)'], [44100, '44.1 kHz'], [96000, '96 kHz']];
  const DEFAULTS = Object.freeze({ mode: 'master', tail: 10, sampleRate: 48000, channels: 2, normalize: false,
    replace: false, destination: 'new', originals: 'clear' });
  let options = { ...DEFAULTS };
  let job = null; // { cancelled, phase, progress, native }

  // ---- Pure model (node tests) -------------------------------------------------------

  // A note in column 0 or in an extra note column (step.columns, app-note-columns.js).
  const hasNote = step => !!step && (Number.isFinite(step.note) && step.note >= 0 || (step.columns || []).some(cell => cell?.note >= 0));
  const patternId = (p, pat) => pat?.id ?? Math.max(0, (p?.patterns || []).indexOf(pat));

  /** The render request for a selection: {pattern, tracks, firstRow, lastRow}. */
  function selection(p, pat, bounds) {
    if (!p || !pat || !bounds) return null;
    const tracks = [];
    for (let t = Math.max(0, bounds.t0); t <= Math.min(bounds.t1, p.tracks.length - 1); t++) tracks.push(t);
    const firstRow = Math.max(0, bounds.r0), lastRow = Math.min(bounds.r1, pat.length - 1);
    if (!tracks.length || lastRow < firstRow) return null;
    return { pattern: patternId(p, pat), tracks, firstRow, lastRow };
  }

  /** Tracks of the selection with a note starting inside its rows. */
  function soundingTracks(pat, sel) {
    return sel.tracks.filter(t => (pat.steps?.[t] || []).slice(sel.firstRow, sel.lastRow + 1).some(hasNote));
  }

  /** "Kick+Snare 00–15": the tracks that play (a whole kit by its name), then the rows. */
  function defaultName(p, pat, sel) {
    const rows = `${pad(sel.firstRow)}–${pad(sel.lastRow)}`;
    const groupIDs = new Set(sel.tracks.map(t => p.tracks[t]?.groupID || null));
    if (groupIDs.size === 1 && !groupIDs.has(null)) {
      const id = [...groupIDs][0], group = (p.groups || []).find(g => g.id === id);
      const members = p.tracks.filter(t => t.groupID === id).length;
      if (group?.name && members === sel.tracks.length && members > 1) return `${group.name} ${rows}`;
    }
    const playing = soundingTracks(pat, sel), list = playing.length ? playing : sel.tracks;
    const names = list.map(t => p.tracks[t]?.name || `Track ${pad(t + 1)}`);
    return `${names.length > 3 ? names.slice(0, 3).join('+') + `+${names.length - 3}` : names.join('+')} ${rows}`;
  }

  /** The sampler instrument for a rendered file: plays it back unchanged at C-4. */
  function renderedInstrument(base, name, path) {
    return Object.assign(base, {
      type: 'sample', name, samplePath: path, rootNote: 60, mode: 'oneshot', volume: 1, pan: 0, tune: 0,
      zones: [], slices: [], slicePoints: [], start: 0, end: 1, loopStart: 0, loopEnd: 1,
      attack: 0, decay: 0, sustain: 1, release: .05, cutoff: 20000, resonance: 0, drive: 0, lfoDepth: 0, lfo2Depth: 0
    });
  }

  /** Apply a finished render to project `p` (inside one mutate): the new instrument and, for
      Replace selection, the cleared or muted originals and one C-4 trigger. Returns
      {instrument, track} (track: the trigger's track, or null). */
  function apply(p, sel, result, opts, helpers) {
    const index = p.instruments.length;
    p.instruments.push(renderedInstrument(helpers.instrument(index, result.name), result.name, result.path));
    if (!opts.replace) return { instrument: index, track: null };
    const pat = p.patterns.find((item, position) => (item.id ?? position) === sel.pattern);
    if (!pat) throw new Error('The rendered pattern no longer exists.');
    if (opts.originals === 'mute') for (const t of sel.tracks) p.tracks[t].mute = true;
    else for (const t of sel.tracks) for (let r = sel.firstRow; r <= sel.lastRow; r++) pat.steps[t][r] = null;
    let track = sel.tracks[0];
    // Kept originals are muted with their tracks, so the trigger cannot live on one of them: it would be silent
    // and replace the note it was meant to keep.
    if (opts.destination === 'new' || opts.originals === 'mute') {
      track = helpers.appendTrack(p, index, result.name);
      Object.assign(p.tracks[track], { volume: 1, pan: 0, mute: false, solo: false });
    }
    pat.steps[track][sel.firstRow] = { note: 60, instrument: index, velocity: 127 };
    return { instrument: index, track };
  }

  // ---- Native request ------------------------------------------------------------------

  /* Long native request without the bridge's default timeout (rendering can take a while). */
  function nativeRequest(command, payload) {
    const st = app();
    return st.longRequest ? st.longRequest(command, payload) : st.request(command, payload);
  }

  window.STNativeEvents?.intercept('render.selectionProgress', payload => {
    if (!job) return true;
    job.phase = payload.phase || job.phase;
    if (Number.isFinite(Number(payload.progress))) job.progress = Number(payload.progress);
    paint();
    return true;
  });

  // ---- Dialog ------------------------------------------------------------------------------

  const get = id => document.getElementById(id);
  const PHASES = { capturing: 'Capturing plug-in state…', loading: 'Loading the selection’s instruments and effects…',
    rendering: 'Rendering…', writing: 'Writing the sample…', done: 'Loading the sample…' };

  function paint() {
    if (state()?.modal !== 'render-selection') return;
    const busy = !!job;
    for (const id of ['rs-name', 'rs-mode', 'rs-tail', 'rs-rate', 'rs-channels', 'rs-normalize', 'rs-replace', 'rs-destination', 'rs-originals']) {
      if (get(id)) get(id).disabled = busy;
    }
    const replace = get('rs-replace')?.checked;
    if (get('rs-replace-options')) get('rs-replace-options').hidden = !replace;
    // Muted originals keep their tracks: the trigger then goes on a new track.
    if (get('rs-destination') && get('rs-originals')?.value === 'mute') { get('rs-destination').value = 'new'; get('rs-destination').disabled = true; }
    if (get('rs-master-note')) get('rs-master-note').hidden = !(replace && get('rs-mode')?.value === 'master');
    if (get('rs-go')) { get('rs-go').disabled = busy; get('rs-go').textContent = busy ? 'Rendering…' : 'Render'; }
    if (get('rs-cancel')) get('rs-cancel').textContent = busy ? (job.cancelled ? 'Cancelling…' : 'Cancel render') : 'Close';
    if (get('rs-cancel')) get('rs-cancel').disabled = !!job?.cancelled;
    const meter = get('rs-meter');
    if (meter) { meter.hidden = !busy; meter.value = job?.progress || 0; }
    if (busy && get('rs-status') && !job.cancelled) get('rs-status').textContent = PHASES[job.phase] || 'Rendering…';
  }

  function cancel() {
    if (!job || job.cancelled) return;
    job.cancelled = true; paint();
    nativeRequest('render.cancelSelection', {}).catch(() => {});
  }

  function read() {
    const tail = Number(get('rs-tail')?.value);
    return {
      name: (get('rs-name')?.value || '').trim(),
      mode: get('rs-mode')?.value === 'track' ? 'track' : 'master',
      tail, sampleRate: Number(get('rs-rate')?.value) || 48000, channels: get('rs-channels')?.value === '1' ? 1 : 2,
      normalize: !!get('rs-normalize')?.checked, replace: !!get('rs-replace')?.checked,
      destination: get('rs-destination')?.value === 'track' ? 'track' : 'new',
      originals: get('rs-originals')?.value === 'mute' ? 'mute' : 'clear'
    };
  }

  function open() {
    const a = app(), p = project();
    if (job) { a.toast('A selection render is already running.'); return false; }
    const pat = a.pattern?.(), sel = selection(p, pat, a.selectedBounds?.() || window.STApp?.cursorBounds?.());
    if (!sel) { a.toast('Select pattern cells to render.'); return false; }
    if (!soundingTracks(pat, sel).length) { a.toast('The selection has no notes to render. Select cells with notes (Shift + arrows).'); return false; }
    const name = defaultName(p, pat, sel);
    const trackNames = sel.tracks.map(t => p.tracks[t]?.name || `Track ${pad(t + 1)}`);
    const tracksText = trackNames.length > 4 ? `${trackNames.slice(0, 4).join(', ')} +${trackNames.length - 4}` : trackNames.join(', ');
    const o = options, selected = (value, current) => String(value) === String(current) ? ' selected' : '';
    const opened = a.openModal('Render selection to sample', `<div class="stack st-render-selection">
      <dl class="st-rs-facts"><dt>Tracks</dt><dd>${esc(tracksText)}</dd><dt>Rows</dt><dd>${pad(sel.firstRow)}–${pad(sel.lastRow)}`
        + ` · ${esc(pat.name || `Pattern ${pad(sel.pattern)}`)}</dd></dl>
      <label class="field"><span>Sample name</span><input id="rs-name" value="${esc(name)}" maxlength="120" spellcheck="false"></label>
      <div class="st-rs-grid">
        <label class="field"><span>Signal</span><select id="rs-mode"><option value="master"${selected('master', o.mode)}>Through master (what you hear)</option>`
        + `<option value="track"${selected('track', o.mode)}>Track only (pre-master)</option></select></label>
        <label class="field"><span>Maximum tail · seconds</span><input id="rs-tail" type="number" min="0" max="60" step="0.5" value="${o.tail}"></label>
        <label class="field"><span>Sample rate</span><select id="rs-rate">${RATES.map(([value, label]) => `<option value="${value}"${selected(value, o.sampleRate)}>${label}</option>`).join('')}</select></label>
        <label class="field"><span>Channels</span><select id="rs-channels"><option value="2"${selected(2, o.channels)}>Stereo</option><option value="1"${selected(1, o.channels)}>Mono</option></select></label>
      </div>
      <label class="st-rs-check"><input id="rs-normalize" type="checkbox"${o.normalize ? ' checked' : ''}> Normalize (peak −0.3 dBFS)</label>
      <label class="st-rs-check"><input id="rs-replace" type="checkbox"${o.replace ? ' checked' : ''}> Replace selection with the rendered sample (bounce in place)</label>
      <div id="rs-replace-options" class="st-rs-grid"${o.replace ? '' : ' hidden'}>
        <label class="field"><span>Trigger at row ${pad(sel.firstRow)} on</span><select id="rs-destination"><option value="new"${selected('new', o.destination)}>A new track</option>`
        + `<option value="track"${selected('track', o.destination)}>${esc(trackNames[0])} (first selected track)</option></select></label>
        <label class="field"><span>Original notes</span><select id="rs-originals"><option value="clear"${selected('clear', o.originals)}>Clear the selected cells</option>`
        + `<option value="mute"${selected('mute', o.originals)}>Keep them, mute the original tracks</option></select></label>
      </div>
      <p id="rs-master-note" class="st-rs-note" hidden>The trigger plays through the master again. Choose Track only to avoid processing the master twice.</p>
      <p class="st-rs-note">Starts clean at row ${pad(sel.firstRow)}: notes started earlier are left out. Notes still held after row ${pad(sel.lastRow)} are released and the tail is kept until it stays below −70 dBFS for 200 ms. Other tracks are left out; playback keeps running.</p>
      <progress id="rs-meter" max="1" value="0" hidden></progress>
      <div id="rs-status" class="st-rs-status" role="status" tabindex="-1"></div></div>`,
      `<button id="rs-cancel">Close</button><button class="primary" id="rs-go">Render</button>`, { kind: 'render-selection' });
    if (opened === false || !get('rs-go')) return false;
    // Closing the dialog (Esc, ×, another dialog) cancels a running render.
    if (window.STApp) window.STApp.modalDismiss = () => cancel();
    for (const id of ['rs-replace', 'rs-mode', 'rs-originals']) get(id).onchange = paint;
    get('rs-cancel').onclick = () => { if (job) cancel(); else a.closeModal(); };
    get('rs-go').onclick = () => run(sel);
    get('rs-name').addEventListener('keydown', event => { if (event.key === 'Enter') { event.preventDefault(); run(sel); } });
    paint();
    get('rs-name').focus(); get('rs-name').select?.();
    return true;
  }

  async function run(sel) {
    if (job) return;
    const a = app(), chosen = read(), status = get('rs-status');
    if (!Number.isFinite(chosen.tail) || chosen.tail < 0 || chosen.tail > 60 || get('rs-tail').value.trim() === '') {
      status.textContent = 'Choose a maximum tail between 0 and 60 seconds.'; get('rs-tail').focus(); return;
    }
    if (!chosen.name) { status.textContent = 'Name the sample.'; get('rs-name').focus(); return; }
    options = { ...options, ...chosen }; delete options.name;
    const owner = project();
    // Edits change the project in place: a bounce in place belongs to these track and pattern objects, not to
    // whatever has moved to their indices by the time the render returns.
    const rendered = sel.tracks.map(t => owner.tracks[t]), renderedPattern = owner.patterns.find((item, position) => (item.id ?? position) === sel.pattern);
    const placed = () => owner.patterns.includes(renderedPattern) && rendered.every((track, i) => track && owner.tracks[sel.tracks[i]] === track);
    job = { cancelled: false, phase: 'capturing', progress: 0 };
    paint();
    let result = null;
    try {
      await a.waitForEngineUpdate?.();
      result = await nativeRequest('render.selection', { ...sel, mode: chosen.mode, tail: chosen.tail, sampleRate: chosen.sampleRate,
        channels: chosen.channels, normalize: chosen.normalize, name: chosen.name });
      if (!result?.path) throw new Error('The audio engine did not return a sample.');
      if (project() !== owner) throw new Error(`The session changed while rendering. The sample was kept at ${result.path}.`);
      job.phase = 'done'; paint();
      const index = owner.instruments.length;
      const sample = await a.request('sample.load', { path: result.path, instrument: index });
      if (!sample?.path) throw new Error('The host could not load the rendered sample.');
      if (project() !== owner || owner.instruments.length !== index) throw new Error(`The project changed while loading. The sample was kept at ${result.path}.`);
      if (chosen.replace && !placed()) throw new Error(`The selected tracks or pattern changed while rendering. The sample was kept at ${result.path}.`);
      let applied = null;
      const label = chosen.replace ? 'bounce selection in place' : 'render selection to sample';
      const done = a.mutate(label, p => {
        applied = apply(p, sel, { name: chosen.name, path: sample.path }, chosen, {
          instrument: (id, name) => a.normalize({ instruments: [{ id, type: 'sample', name }] }).instruments[0],
          appendTrack: (draft, instrument, name) => a.appendInstrumentTrack(draft, instrument, name)
        });
      });
      if (done === false || !applied) throw new Error(`The edit was not applied. The sample was kept at ${result.path}.`);
      const st = state();
      st.samples ||= {}; st.samples[applied.instrument] = sample;
      st.instrument = applied.instrument;
      if (applied.track !== null) { st.track = applied.track; st.row = sel.firstRow; st.selection = null; }
      job = null;
      a.closeModal();
      a.refresh?.();
      const seconds = (result.frames / result.sampleRate).toFixed(2);
      const tail = result.tailEnd === 'maximum' ? ' · tail cut at the maximum' : '';
      a.toast(`Rendered “${chosen.name}” · ${seconds} s${tail} · instrument ${pad(applied.instrument)}${chosen.replace ? ' · selection replaced (⌘Z undoes)' : ''}.`
        + (result.warnings?.length ? ' ' + result.warnings.join(' ') : ''));
    } catch (error) {
      const message = error?.message || String(error);
      job = null;
      if (state()?.modal === 'render-selection') { paint(); status.textContent = message; status.focus?.(); }
      else if (!/cancelled/i.test(message)) a.toast(message, 'error');
    }
  }

  // ⌥B in the pattern editor (the palette and the selection's context menu run the same command).
  document.addEventListener('keydown', event => {
    if (!event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || event.isComposing || event.code !== 'KeyB') return;
    const st = state();
    if (!st || st.modal || st.view !== 'pattern' || window.STApp?.isTextInput?.(document.activeElement)) return;
    const target = event.target;
    if (!(target === document.body || target?.id === 'tracker' || target?.closest?.('.tracker'))) return;
    event.preventDefault();
    if (!event.repeat) app().perform('renderSelection');
  });

  window.STBusy?.register('renderSelection', { active: () => !!job, reason: 'Wait for the selection render to finish.' });

  window.STRenderSelection = {
    open, cancel,
    get isBusy() { return !!job; },
    model: { selection, soundingTracks, defaultName, renderedInstrument, apply }
  };
})();
