(() => {
  'use strict';
  const local = {
    pixels: 22,
    snap: .25,
    cursor: 0,
    metadata: new Map(),
    loading: new Set(),
    requests: new Map(),
    failed: new Set(),
    preview: '',
    busy: false,
    fieldPaintTimer: null,
    fieldPaintPending: false,
    deferredPaint: false,
    fieldDrafts: new Map(),
    pointerHeld: false,
    fieldGuardsInstalled: false
  };
  const app = () => window.ST, state = () => app().state, project = () => state().project;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const clamp = (value, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(Number(value)) ? Number(value) : lo));
  const uid = () => `clip-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  const clips = () => project().audioClips || [];
  const timingSignature = value => JSON.stringify([value.bpm, value.timeSignature]);
  const maxBeat = () => window.STProjectTiming.MAX_BEAT;
  let receivedStatus = null;
  let timingCache = null;
  // The project's tempo and meter (project-timing.js), computed again only when one of them changed.
  function timing(destination = project()) {
    const signature = timingSignature(destination);
    if (timingCache?.owner === destination && timingCache.signature === signature) return timingCache.value;
    const value = window.STProjectTiming.create(destination);
    timingCache = { owner: destination, signature, value }; return value;
  }
  function seconds(clip, length = Number(clip.durationBeats || 0), destination = project()) {
    const start = Number(clip.startBeat || 0); return timing(destination).duration(start, start + length);
  }
  function beatsFromSeconds(start, duration, destination = project()) { return timing(destination).endingBeat(start, duration) - start; }
  // A clip keeps its length in beats wherever it starts.
  function moveTo(clip, start) { clip.startBeat = Math.max(0, start); }
  const clipPath = clip => clip.path || clip.samplePath || '';
  const selected = () => clips().find(clip => String(clip.id) === String(state().clipSelected));
  function sequenceBeat(index = state().sequence || 0) {
    return (project().sequence || []).slice(0, index).reduce((sum, id) => sum
        + (project().patterns.find(pattern => pattern.id === id)?.length || 0) / (project().linesPerBeat || 4),
        0);
  }
  function playheadBeat(payload = state().status || {}) {
    // The host's beat is absolute song time (including pattern occurrence origin).
    // A status received before the project or its tempo changed cannot supply it.
    if (receivedStatus?.owner === project() && receivedStatus.payload === payload && receivedStatus.signature === timingSignature(project()) &&
        typeof payload.beat === 'number' && Number.isFinite(payload.beat) && payload.beat >= 0 && payload.beat <= maxBeat()) return payload.beat;
    const occurrence = Number.isInteger(payload.sequence) && payload.sequence >= 0 && payload.sequence < project().sequence.length
        ? payload.sequence
        : state().sequence || 0;
    const position = typeof payload.position === 'number' && Number.isFinite(payload.position) ? payload.position : Number(payload.row || 0);
    if (payload.pattern !== undefined && payload.pattern !== project().sequence[occurrence]) return sequenceBeat(state().sequence || 0)
        + (state().row || 0) / (project().linesPerBeat || 4);
    return clamp(sequenceBeat(occurrence) + Math.max(0, position) / (project().linesPerBeat || 4), 0, maxBeat());
  }
  function barAt(value) { return timing().barAtBeat(clamp(value, 0, maxBeat())); }
  function totalBeats() {
    const end = Math.max(16, sequenceBeat(project().sequence.length),
        ...clips().map(clip => Number(clip.startBeat || 0) + Number(clip.durationBeats || 0)));
    if (!Number.isFinite(end) || end > maxBeat()) throw new Error('The audio arrangement is longer than the supported song length.');
    const bar = barAt(end), firstEnd = Math.min(maxBeat(),end === bar.barStart ? end : bar.barEnd);
    return Math.min(maxBeat(), barAt(firstEnd).barEnd);
  }
  const snapValue = (at = local.cursor) => local.snap === 'bar'
      ? Math.min(maxBeat(), barAt(at).barEnd) - barAt(at).barStart
      : Number(local.snap || 0);
  function snap(value) {
    value = clamp(value, 0, maxBeat());
    if (local.snap === 'bar') { const bar = barAt(value), end = Math.min(maxBeat(), bar.barEnd); return value - bar.barStart < end - value
        ? bar.barStart
        : end; }
    const grid = snapValue(); return grid ? clamp(Math.round(value / grid) * grid, 0, maxBeat()) : value;
  }
  function neighbor(value, direction) {
    value = clamp(value, 0, maxBeat());
    if (local.snap !== 'bar') return clamp(value + direction * (snapValue() || .0625), 0, maxBeat());
    const bar = barAt(value);
    if (direction > 0) return Math.min(maxBeat(), bar.barEnd);
    // On a bar line the previous bar starts one bar earlier.
    return value > bar.barStart ? bar.barStart : Math.max(0, bar.barStart - timing().quartersPerBar);
  }
  function positionLabel(value) {
    const bar = barAt(value),
      beat = Math.floor(bar.notatedBeat * 100) / 100;
    return `${bar.bar}:${beat.toFixed(2).replace(/\.00$/, '')}`;
  }
  function positionDescription(value) {
    const bar = barAt(value);
    return `Bar ${bar.bar} · beat ${Math.floor(bar.notatedBeat * 1000) / 1000} · ${bar.numerator}/${bar.denominator}`;
  }
  // Bar numbers of the ruler: at most 2048 ticks, and no closer together than their labels need.
  function rulerTicks(end) {
    const quartersPerBar = timing().quartersPerBar,
      stride = Math.max(1, Math.ceil(Math.ceil(end / quartersPerBar) / 2048), Math.ceil(44 / (local.pixels * quartersPerBar)));
    const ticks = [];
    for (let offset = 0; offset * quartersPerBar < end; offset += stride) ticks.push({ beat: offset * quartersPerBar, bar: offset + 1 });
    for (let i = 0; i < ticks.length; i++) {
      const width = ((ticks[i + 1]?.beat ?? end) - ticks[i].beat) * local.pixels;
      ticks[i].labelVisible = width >= 34;
      // The first bar number carries the meter.
      ticks[i].meterVisible = i === 0 && width >= 70;
    }
    return ticks;
  }
  const clipName = clip => clip.name || local.metadata.get(clipPath(clip))?.name || clipPath(clip).split('/').pop() || 'Audio clip';
  async function info(path) {
    if (local.metadata.has(path)) return local.metadata.get(path);
    if (local.requests.has(path)) return local.requests.get(path);
    local.loading.add(path);
    const task = (async () => {
      try {
        const result = await app().request('audio.clipInfo', { path });
        if (!result || !Number.isFinite(Number(result.duration))) throw new Error('Cannot read this audio file’s duration.');
        local.metadata.set(path, result); local.failed.delete(path); return result;
      } catch (error) { local.failed.add(path); app().toast(error.message || String(error), 'error'); return null; }
      finally { local.loading.delete(path); local.requests.delete(path); }
    })();
    local.requests.set(path, task); return task;
  }
  function refresh() {
    const active = document.activeElement, draft = local.fieldDrafts.get(active?.id);
    if (local.pointerHeld || local.fieldPaintPending || draft?.node === active && draft.project === project()) { local.deferredPaint = true; return; }
    local.deferredPaint = false;
    const focused = active?.id;
    app().refresh();
    if (focused) document.getElementById(focused)?.focus({ preventScroll: true });
  }
  function refreshAfterFieldCommit(input) {
    if (input) local.fieldDrafts.delete(input.id);
    const owner = project(); local.fieldPaintPending = true;
    clearTimeout(local.fieldPaintTimer);
    // change can run inside a native blur/Tab transfer while activeElement is
    // still body. Keep the destination DOM alive until that transfer completes.
    local.fieldPaintTimer = setTimeout(() => {
      local.fieldPaintTimer = null; local.fieldPaintPending = false;
      if (project() === owner && state().view === 'song') refresh();
    }, 0);
  }
  function commitField(input, label, fn) {
    edit(label, fn, { render: false }); refreshAfterFieldCommit(input);
  }
  async function loadWaveforms() {
    const missing = [...new Set(clips().map(clipPath))].filter(path => path
        && !local.metadata.has(path)
        && !local.loading.has(path)
        && !local.failed.has(path));
    if (!missing.length) return;
    const chunk = missing.slice(0, 2);
    await Promise.allSettled(chunk.map(info));
    if (state().view === 'song') refresh();
    if (missing.length > chunk.length) queueMicrotask(loadWaveforms);
  }
  function waveform(clip) {
    const meta = local.metadata.get(clipPath(clip)), peaks = meta?.peaks || meta?.peaks512 || [];
    if (!peaks.length || !meta.duration) return '<div class="clips-wave-pending">Waveform pending</div>';
    const sourceStart = Number(clip.sourceOffset || 0), length = seconds(clip), width = 360;
    const map = timing(), start = Number(clip.startBeat || 0), durationBeats = Number(clip.durationBeats || 0);
    let path = '';
    for (let x = 0; x < width; x += 3) {
      const offset = sourceStart + map.duration(start, start + x / width * durationBeats), index = Math.floor(offset / meta.duration * peaks.length);
      const point = peaks[index];
      const value = Array.isArray(point) ? Math.max(...point.map(Math.abs)) : Math.abs(Number(point || 0));
      const height = clamp(value, 0, 1) * 14;
      path += `M${x} ${18 - height}V${18 + height}`;
    }
    const fadeIn = clamp((map.endingBeat(start, clamp(clip.fadeIn || 0, 0, length)) - start) / durationBeats * width, 0, width),
      fadeOut = clamp(width - (map.endingBeat(start, Math.max(0, length - clamp(clip.fadeOut || 0, 0, length))) - start) / durationBeats * width, 0,
          width);
    return `<svg class="clips-waveform" viewBox="0 0 ${width} 36" preserveAspectRatio="none" aria-hidden="true"><path d="${path}" fill="none"`
        + ` stroke="currentColor" stroke-width="1.5"/><path d="M0 34L${fadeIn} 2H${width - fadeOut}L${width} 34" fill="none" stroke="#ded3b8"`
        + ` stroke-width=".9" opacity=".5"/></svg>`;
  }
  function render() {
    try { return renderTimeline(); } catch(error) { return `<section class="panel clips-panel" aria-label="Audio clip arrangement"><div class="clips-timing-error" role="alert">`
        + `<strong>The audio arrangement cannot be shown</strong><p>${esc(error.message || String(error))}</p><p>The saved clips and their source `
        + `files are unchanged.</p></div></section>`; }
  }
  function renderTimeline() {
    const all = clips(), active = selected(), beats = totalBeats(), width = beats * local.pixels;
    const map = timing(), ticks = rulerTicks(beats);
    return `<section class="panel clips-panel" aria-label="Audio clip arrangement"><div class="panel-head"><div><h2>Audio arrangement</h2>`
        + `<p class="clips-subtitle">Song timeline · recorded audio and loops</p></div><div class="row"><span class="chip">${all.length} / 256 CLIPS</span>`
        + `<button id="clips-import" class="primary" ${local.busy || all.length >= 256 ? 'disabled' : ''}>${local.busy ? 'Reading audio…' : '+ Import audio'}</button></div></div>
      <div class="clips-toolbar"><label class="clips-toolbar-field">Snap<select id="clips-snap" aria-label="Timeline snap">${[[0, 'Off'],
          [.0625, '1/64'], [.125, '1/32'], [.25, '1/16'], [.5, '1/8'], [1, '1/4'], ['bar', '1 bar']].map(([value, name]) => `<option value="${value}" ${local.snap === value ? 'selected' : ''}>${name}</option>`).join('')}</select>`
          + `</label><label class="clips-toolbar-field">Edit cursor<input id="clips-cursor" type="number" min="0" max="1000000" step="${snapValue() || .01}"`
          + ` value="${Number(local.cursor.toFixed(4))}" aria-label="Edit cursor in beats from song start"></label><span class="clips-position"`
          + ` title="${esc(positionDescription(local.cursor))}" aria-label="${esc(positionDescription(local.cursor))}">${positionLabel(local.cursor)}</span>`
          + `<button id="clips-cursor-position" title="Move the edit cursor to the current transport position">From playhead</button>`
          + `<button id="clips-import-cursor" ${local.busy || all.length >= 256 ? 'disabled' : ''}>Import at cursor</button>`
          + `<button id="clips-bounce" title="${project().tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity)
              ? `Bounce needs a new track; the project already has ${project().tracks.length} tracks.`
              : 'Render the selected track with its effects into a new audio track'}"`
          + ` ${local.busy || all.length >= 256 || project().tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity) ? 'disabled' : ''}>Render stem…</button>`
          + `<span class="grow"></span><button id="clips-zoom-out" aria-label="Zoom audio timeline out" ${local.pixels <= 6 ? 'disabled' : ''}>−</button>`
          + `<span class="clips-zoom-label">${Math.round(local.pixels / 22 * 100)}%</span><button id="clips-zoom-in" aria-label="Zoom audio timeline `
          + `in" ${local.pixels >= 88 ? 'disabled' : ''}>＋</button></div>
      <div class="clips-timeline" id="clips-timeline" data-scroll="audio-clips" tabindex="0" role="group" aria-label="Audio timeline. Up and `
          + `down select clips; Home and End select the first or last clip. Left and right nudge; shift trims length; option slips source. Tab leaves `
          + `the timeline."><div class="clips-timeline-content" style="width:${width + 132}px"><div class="clips-ruler-row">`
          + `<div class="clips-track-heading">TRACK / BAR</div><div class="clips-ruler" style="width:${width}px">${ticks.map((tick, index) => `<span data-bar="${tick.bar}" data-beat="${tick.beat}" class="${index ? '' : 'clips-ruler-first'}" style="left:${tick.beat * local.pixels}px"`
          + ` title="${esc(positionDescription(tick.beat))}" aria-label="${esc(positionDescription(tick.beat))}">${tick.labelVisible
              ? `${tick.bar}${tick.meterVisible ? `<small>${map.numerator}/${map.denominator}</small>` : ''}`
              : ''}</span>`).join('')}</div>`
          + `</div><div class="clips-meter-grid" aria-hidden="true" style="width:${width}px;--clips-beat:${local.pixels * 4 / map.denominator}px;--clips-bar:${local.pixels * map.quartersPerBar}px">`
          + `</div>${project().tracks.map((track, trackIndex) => `<div class="clips-lane-row"><button class="clips-track-label ${state().track === trackIndex ? 'selected' : ''}" id="clips-track-label-${trackIndex}"`
          + ` data-clips-track="${trackIndex}"><i style="background:${esc(track.color || '#7dafaa')}"></i><span>${esc(track.name || `Track ${trackIndex + 1}`)}</span>`
          + `</button><div class="clips-lane" data-clips-lane="${trackIndex}" style="width:${width}px">${all.filter(clip => Number(clip.track) === trackIndex).map(clip => `<button class="clips-clip ${String(clip.id) === String(state().clipSelected) ? 'selected' : ''} ${clip.muted ? 'muted' : ''} ${local.failed.has(clipPath(clip)) ? 'offline' : ''}"`
          + ` id="clip-${esc(clip.id)}" data-clip-id="${esc(clip.id)}" style="left:${Number(clip.startBeat || 0) * local.pixels}px;width:${Math.max(9, Number(clip.durationBeats || 0) * local.pixels)}px"`
          + ` title="${esc(clipName(clip))} · ${positionLabel(Number(clip.startBeat || 0))} · ${Number(clip.durationBeats || 0).toFixed(2)} beats"`
          + ` aria-label="${esc(clipName(clip))}, ${esc(track.name)}, ${esc(positionDescription(Number(clip.startBeat || 0)))}${clip.muted ? ', muted' : ''}">`
          + `<span class="clips-clip-title">${clip.muted ? '○ ' : ''}${esc(clipName(clip))}${esc(window.STClipTakes?.caption(clip) || '')}</span>${waveform(clip)}</button>`).join('')}</div>`
          + `</div>`).join('')}<div `
          + `class="clips-edit-cursor" style="left:${132 + local.cursor * local.pixels}px"></div><div class="clips-playhead" id="clips-playhead"`
          + ` style="left:${132 + playheadBeat() * local.pixels}px;display:${state().playing ? 'block' : 'none'}"></div></div></div>
      ${all.length ? renderInspector(active) : '<div class="clips-empty"><strong>No audio clips</strong><span>Import a file onto the selected track at the selected pattern’s song position. '
          + 'Sources keep their original speed.</span></div>'}
      <div class="clips-shortcuts"><span>Timeline keys</span><span><kbd>F6 / ⇧F6</kbd> Next / previous section</span>`
          + `<span><kbd>↑ ↓</kbd> Select clip</span><span><kbd>← →</kbd> Move clip</span><span><kbd>⇧ ← →</kbd> Trim end</span>`
          + `<span><kbd>⌥ ← →</kbd> Slip audio</span><span><kbd>⌥ ↑ ↓</kbd> Change track</span><span><kbd>⌘ D</kbd> Duplicate</span>`
          + `<span><kbd>S</kbd> Split at cursor</span><span><kbd>PgUp / PgDn</kbd> Change take</span><span><kbd>T</kbd> Manage takes</span>`
          + `<span><kbd>Delete</kbd> Remove</span></div></section>`;
  }
  function renderInspector(clip) {
    if (!clip) return '<div class="clips-empty"><strong>Select a clip to edit</strong><span>Use F6 to reach the timeline, then ↑ and ↓ to select clips. Your '
        + 'source files stay unchanged.</span></div>';
    const meta = local.metadata.get(clipPath(clip)), duration = seconds(clip);
    return `<div class="clips-inspector"><div class="clips-inspector-title"><label class="field"><span>Clip name</span>`
        + `<input id="clips-name" value="${esc(clipName(clip))}" maxlength="160"></label><div class="clips-file-path" title="${esc(clipPath(clip))}">${esc(clipPath(clip))}</div>`
        + `<div class="row"><button id="clips-preview">▶ Preview file</button><button id="clips-stop-preview">■ Stop preview</button>`
        + `</div></div><div class="clips-properties"><label class="field"><span>Track</span><select id="clips-track">${project().tracks.map((track, index) => `<option value="${index}" ${Number(clip.track) === index ? 'selected' : ''}>${esc(track.name)}</option>`).join('')}</select>`
        + `</label>${[['startBeat', 'Start · quarter beats', 0, 1000000, snapValue() || .01, clip.startBeat || 0],
            ['durationBeats', 'Length · quarter beats', .001, 1000000, snapValue() || .01, clip.durationBeats || 1],
            ['sourceOffset', 'Source offset · sec', 0, meta?.duration || 86400, .01, clip.sourceOffset || 0],
            ['gain', 'Level · gain', 0, 4, .01, clip.gain ?? 1], ['fadeIn', 'Fade in · sec', 0, duration, .01, clip.fadeIn || 0],
            ['fadeOut', 'Fade out · sec', 0, duration, .01, clip.fadeOut || 0]].map(([field, label, min, max, step, value]) => `<label class="field"><span>${label}</span><input data-clips-field="${field}" id="clips-${field}" type="number" min="${min}"`
        + ` max="${max}" step="${step}" value="${Number(Number(value).toFixed(4))}"></label>`).join('')}<label `
        + `class="clips-mute"><input id="clips-muted" type="checkbox" ${clip.muted ? 'checked' : ''}> Mute clip</label>`
        + `</div><div class="clips-actions"><button id="clips-duplicate" ${clips().length >= 256 ? 'disabled' : ''}>Duplicate after</button>`
        + `<button id="clips-split" ${clips().length >= 256 || local.cursor <= clip.startBeat || local.cursor >= clip.startBeat + clip.durationBeats
            ? 'disabled'
            : ''}>Split `
        + `at cursor</button><button id="clips-source" title="Show the source file in Finder">Show source</button><button id="clips-delete"`
        + ` class="danger">Remove clip</button></div>${window.STClipTakes?.render(clip) || ''}<p class="clips-original-speed">Original-speed audio `
        + `· changing length trims the clip; changing source offset slips the audio. Use sample time stretching before import when tempo matching `
        + `is needed.</p></div>`;
  }
  async function addFile(path, target = {}) {
    if (local.busy) { app().toast('Finish the current audio operation first. Your source file is preserved.', 'info'); return null; }
    const destinationProject = target.project || project(),
      track = Number(target.track ?? state().track ?? 0),
      startBeat = Number(target.startBeat ?? sequenceBeat());
    const trackObject = target.trackObject || destinationProject.tracks[track];
    const requireDestination = () => {
      if (project() !== destinationProject
          || !trackObject
          || destinationProject.tracks[track] !== trackObject) throw new Error('The project or destination track changed.');
      if (window.STBusy.any(['session', 'export', 'routing'])) throw new Error('Finish the current project operation before importing audio.');
      if ((destinationProject.audioClips || []).length >= 256) throw new Error('A project supports up to 256 audio clips.');
      if (!Number.isInteger(track)
          || !Number.isFinite(startBeat)
          || startBeat < 0
          || startBeat > maxBeat()) throw new Error('Choose a valid destination track and song position.');
    };
    local.busy = true; refresh();
    try {
      requireDestination();
      const metadata = await info(path); requireDestination();
      const duration = Number(metadata?.duration || target.duration);
      if (!Number.isFinite(duration) || duration <= 0) throw new Error('Cannot read this audio file’s duration.');
      const clip = {
        id: uid(),
        track,
        path,
        name: metadata?.name || path.split('/').pop(),
        startBeat,
        durationBeats: beatsFromSeconds(startBeat, duration, destinationProject),
        sourceDuration: duration,
        sourceOffset: 0,
        gain: 1,
        fadeIn: 0,
        fadeOut: 0,
        muted: false
      };
      let applied = false;
      app().mutate('import audio clip', value => { value.audioClips ||= []; value.audioClips.push(clip); applied = true; });
      // A mutation callback may run and then be rolled back by frozen-source
      // protection. Only a clip retained in the authoritative project is added.
      if (!applied
          || project() !== destinationProject
          || !destinationProject.audioClips?.includes(clip)) throw new Error('The audio import was not applied.');
      state().clipSelected = clip.id; local.cursor = startBeat; app().toast(`Audio added to ${trackObject.name}`); return clip;
    } catch (error) { app().toast(`${error.message || String(error)} Your source is preserved at ${path}`, 'error'); return null; }
    finally { local.busy = false; refresh(); }
  }
  async function importFile(atCursor = false) {
    if (local.busy) return null;
    const target = { project: project(), track: Number(state().track || 0), startBeat: atCursor ? local.cursor : sequenceBeat() };
    target.trackObject = target.project.tracks[target.track];
    try { const result = await app().request('dialog.openSample', {}); const path = typeof result === 'string'
        ? result
        : result?.path; if (path && !result?.cancelled) return addFile(path, target); }
    catch (error) { app().toast(error.message, 'error'); }
    return null;
  }
  function bounceTrack() {
    if (local.busy) { app().toast('Finish the current audio operation first.', 'info'); return; }
    const destinationProject = project(), sourceIndex = Number(state().track || 0), sourceTrack = destinationProject.tracks[sourceIndex];
    if (!sourceTrack) return;
    if (destinationProject.tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity)) { app().toast(`Bounce needs a new track. This project already has ${destinationProject.tracks.length} tracks.`,
        'info'); return; }
    if (clips().length >= 256) { app().toast('Remove an unused audio clip before bouncing; this project has 256 clips.', 'info'); return; }
    const explanation = `Capture <strong>${esc(sourceTrack.name)}</strong> after its inserts, fader and pan, before its parent bus. Shared returns and downstream `
        + `bus or Master processing are excluded. The source stays unchanged. The rendered stem is added on a new, muted track routed directly to `
        + `Master, with no sends. Use the mixer to audition it and decide how to use it; it is not an automatic replacement for the source mix.`;
    app().openModal('Render track stem for audition', `<div class="stack"><p class="clips-bounce-help">${explanation}</p><label class="field"><span>Effect tail · seconds</span>`
        + `<input id="clips-bounce-tail" type="number" min="0" max="30" step=".5" value="3"></label><p class="clips-bounce-help">Undo restores `
        + `the previous arrangement. External hardware must be recorded before rendering.</p><div id="clips-bounce-status" role="status"`
        + ` class="clips-bounce-help"></div></div>`, `<button id="clips-bounce-cancel">Cancel</button><button class="primary" id="clips-bounce-render">Render muted stem</button>`, {
      kind: 'clip-bounce'
    });
    const get = id => document.getElementById(id);
    if (state().modal !== 'clip-bounce' || !get('clips-bounce-render')) return;
    const renderButton = get('clips-bounce-render');
    const active = () => state().modal === 'clip-bounce' && get('clips-bounce-render') === renderButton;
    const current = () => project() === destinationProject && destinationProject.tracks[sourceIndex] === sourceTrack;
    const requireCurrent = () => {
      if (!active() || !current()) throw new Error('The project or selected source changed. Reopen the render dialog.');
    };
    const requireIdle = () => {
      if (window.STBusy.any(['session', 'export', 'pluginEdit', 'effectMovePrep',
          'routingPrep'])) throw new Error('Finish the current save, project or plug-in change before rendering.');
    };
    get('clips-bounce-cancel').onclick = app().closeModal;
    renderButton.onclick = async () => {
      if (local.busy || !active()) return;
      const tail = Number(get('clips-bounce-tail').value);
      if (!get('clips-bounce-tail').value.trim()
          || !Number.isFinite(tail)
          || tail < 0
          || tail > 30) { get('clips-bounce-status').textContent = 'Choose an effect tail between 0 and 30 seconds.'; return; }
      renderButton.disabled = true; get('clips-bounce-status').textContent = 'Preparing the track and current plug-in settings…'; local.busy = true;
      try {
        await app().waitForGraphEdit?.(); requireCurrent();
        requireIdle();
        if (await app().captureNativeEdits?.() === false) throw new Error('Current plug-in settings could not be captured. Nothing was rendered.');
        requireCurrent(); await app().waitForGraphEdit?.(); requireCurrent(); requireIdle();
        let prepared;
        app().cancelPendingEngineSync?.();
        const synced = await app().syncEngineProject(() => {
          requireCurrent(); requireIdle();
          prepared = JSON.stringify(destinationProject);
          return JSON.parse(prepared);
        });
        requireCurrent();
        if (synced === false) throw new Error('Audio preparation was canceled. Nothing was rendered.');
        if (JSON.stringify(destinationProject) !== prepared) throw new Error('The project changed during preparation. Reopen the render dialog.');
        get('clips-bounce-cancel').disabled = true;
        get('clips-bounce-status').textContent = 'Rendering a muted stem for audition…';
        const result = await app().request('audio.bounceTrack', { track: sourceIndex, tail });
        if (!result?.clip || !result.path) throw new Error('The audio engine did not return a bounced clip.');
        if (!active()
            || !current()
            || JSON.stringify(destinationProject) !== prepared) throw new Error(`The project or render dialog changed. Your audio is saved at ${result.path}`);
        if (window.STBusy.any(['session', 'export', 'routing'])) throw new Error(`Another project operation is in progress. Your audio is saved at ${result.path}`);
        if (destinationProject.tracks.length >= (window.TRACK_SANITY_LIMIT ?? Infinity)) throw new Error(`No free track remains. Your bounce is saved at ${result.path}`);
        if (clips().length >= 256) throw new Error(`No free clip remains. Your audio is saved at ${result.path}`);
        let addedTrack, addedClip;
        app().mutate('add rendered audition stem', value => {
          const newIndex = value.tracks.length;
          const track = {
            id: newIndex,
            name: `${sourceTrack.name} · Stem`,
            color: sourceTrack.color || '#88b5a5',
            volume: 1,
            pan: 0,
            mute: true,
            solo: false,
            instrument: 0,
            delaySend: 0,
            reverbSend: 0,
            plugins: []
          };
          window.STRoutingProject.appendTrack(value, track, app().uid || uid);
          value.patterns.forEach(pattern => { pattern.steps ||= []; pattern.steps.push(Array.from({ length: pattern.length }, () => null)); });
          const clip = { ...result.clip, id: uid(), track: newIndex, path: result.path, name: `${sourceTrack.name} stem` };
          value.audioClips ||= []; value.audioClips.push(clip); addedTrack = track; addedClip = clip;
        });
        if (!addedClip
            || project() !== destinationProject
            || !destinationProject.tracks.includes(addedTrack)
            || !destinationProject.audioClips?.includes(addedClip)) throw new Error(`The rendered clip was not applied. Your audio is saved at ${result.path}`);
        state().track = destinationProject.tracks.indexOf(addedTrack); state().clipSelected = addedClip.id;
        app().closeModal();
        app().toast('Muted stem added for audition. The source and its routing are unchanged.');
      } catch (error) { app().toast(error.message || String(error),
          'error'); if (active()) { renderButton.disabled = false; get('clips-bounce-cancel').disabled = false; get('clips-bounce-status').textContent = error.message || String(error); } }
      finally { local.busy = false; if (state().view === 'song') refresh(); }
    };
    get('clips-bounce-render').focus();
  }
  function edit(label, fn, options = {}) {
    const clip = selected(); if (!clip) return;
    try {
      const proposal = window.STUtil.clone(clip);
      if (window.STClipTakesProject && proposal.takeSet) window.STClipTakesProject.read(proposal);
      fn(proposal);
      if (window.STClipTakesProject && proposal.takeSet) Object.assign(proposal, window.STClipTakesProject.retainEdits(proposal));
      // A key that changes nothing (⌥↑ on the first track, a trim at the end of the source) is not an Undo step.
      if (JSON.stringify(proposal) === JSON.stringify(clip)) return;
      app().mutate(label, () => Object.assign(clip, proposal), options);
    } catch (error) { app().toast(error.message, 'error'); }
  }
  function constrain(clip) {
    clip.startBeat = Math.max(0, Number(clip.startBeat || 0)); clip.durationBeats = Math.max(.001, Number(clip.durationBeats || .001));
    // Validate the requested region before changing it: a position outside the song's range
    // must not leave a partially edited clip in history.
    seconds(clip);
    clip.sourceOffset = Math.max(0, Number(clip.sourceOffset || 0));
    const sourceLength = local.metadata.get(clipPath(clip))?.duration ?? clip.sourceDuration;
    if (Number.isFinite(sourceLength) && sourceLength > 0) {
      clip.sourceOffset = Math.min(clip.sourceOffset, Math.max(0, sourceLength - .001));
      const available = Math.max(1e-9, sourceLength - clip.sourceOffset);
      if (seconds(clip) > available) clip.durationBeats = beatsFromSeconds(clip.startBeat, available);
    }
    const duration = seconds(clip); clip.fadeIn = clamp(clip.fadeIn || 0, 0, duration); clip.fadeOut = clamp(clip.fadeOut || 0, 0, duration);
  }
  function duplicate() {
    const clip = selected(); if (!clip || clips().length >= 256) return;
    let copy;
    try { copy = { ...window.STUtil.clone(clip), id: uid() }; moveTo(copy, clip.startBeat + clip.durationBeats); constrain(copy); }
    catch (error) { app().toast(error.message, 'error'); return; }
    releaseClipFocus();
    app().mutate('duplicate audio clip', value => { value.audioClips.push(copy); state().clipSelected = copy.id; });
    focusSelected();
  }
  function split() {
    const clip = selected(); if (!clip || clips().length >= 256) return;
    const point = local.cursor - clip.startBeat;
    if (point <= .001 || point >= clip.durationBeats - .001) { app().toast('Place the edit cursor inside the selected clip to split it.',
        'info'); return; }
    let parts;
    try {
      if (window.STClipTakesProject) parts = window.STClipTakesProject.split(clip, point,
          (start, length) => timing().duration(start, start + length), uid);
      else {
        const left = window.STUtil.clone(clip);
        const right = {
          ...window.STUtil.clone(clip),
          id: uid(),
          startBeat: local.cursor,
          durationBeats: clip.durationBeats - point,
          sourceOffset: Number(clip.sourceOffset || 0) + seconds(clip, point),
          fadeIn: 0
        };
        left.durationBeats = point; left.fadeOut = 0; parts = [left, right];
      }
      constrain(parts[0]); constrain(parts[1]);
    } catch (error) { app().toast(error.message, 'error'); return; }
    releaseClipFocus();
    app().mutate('split audio clip', value => {
      Object.assign(clip, parts[0]); value.audioClips.push(parts[1]); state().clipSelected = parts[1].id;
    }); focusSelected();
  }
  function remove() {
    const clip = selected(); if (!clip) return;
    app().mutate('remove audio clip', value => {
      value.audioClips = value.audioClips.filter(item => item.id !== clip.id);
      const fallback = value.audioClips[0];
      state().clipSelected = fallback?.id || null;
      if (fallback) state().track = fallback.track;
    });
    focusSelected();
  }
  function focusSelected() {
    const node = document.getElementById(`clip-${state().clipSelected}`);
    (node || document.getElementById('clips-timeline'))?.focus({ preventScroll: true });
    node?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }
  /* A re-render gives focus back to the control that had it, and a clip that receives focus becomes the selection
     (bind). Before the selection moves to another clip, focus leaves the old one, or the re-render would select it again. */
  function releaseClipFocus() {
    if (document.activeElement?.closest?.('[data-clip-id]')) document.getElementById('clips-timeline')?.focus({ preventScroll: true });
  }
  function select(id, focus = true) {
    state().clipSelected = id;
    const clip = selected();
    if (clip) state().track = clip.track;
    if (focus) document.getElementById(`clip-${id}`)?.focus({ preventScroll: true });
    refresh();
    if (focus) focusSelected();
  }
  function bind() {
    // The song sequence panel above the timeline is owned by song-sequence.js.
    window.STSong?.mount?.();
    if (!local.fieldGuardsInstalled && document.addEventListener) {
      local.fieldGuardsInstalled = true;
      document.addEventListener('pointerdown', event => {
        if (event.target?.closest?.('.clips-panel') || document.activeElement?.closest?.('.clips-panel')) local.pointerHeld = true;
      }, true);
      const released = () => {
        local.pointerHeld = false;
        if (local.deferredPaint || local.fieldPaintPending) refreshAfterFieldCommit();
      };
      for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, released, true);
      window.addEventListener?.('blur', released);
    }
    const on = (id, event, fn) => document.getElementById(id)?.addEventListener(event, fn);
    on('clips-import', 'click', () => importFile()); on('clips-import-cursor', 'click', () => importFile(true));
    on('clips-bounce', 'click', bounceTrack);
    on('clips-snap', 'change', event => {
      local.snap = event.target.value === 'bar' ? 'bar' : Number(event.target.value);
      refreshAfterFieldCommit(event.target);
    });
    on('clips-cursor', 'change', event => { local.cursor = clamp(event.target.value, 0, 1000000); refreshAfterFieldCommit(event.target); });
    on('clips-cursor-position', 'click', () => { local.cursor = snap(playheadBeat()); refresh(); });
    on('clips-zoom-in', 'click', () => { local.pixels = Math.min(88, local.pixels * 1.4); refresh(); });
    on('clips-zoom-out', 'click', () => { local.pixels = Math.max(6, local.pixels / 1.4); refresh(); });
    document.querySelectorAll('[data-clips-track]').forEach(button => button.addEventListener('click', () => {
      state().track = Number(button.dataset.clipsTrack);
      refresh();
    }));
    document.querySelectorAll('[data-clip-id]').forEach(button => {
      button.addEventListener('click', event => { event.stopPropagation(); select(button.dataset.clipId); });
      button.addEventListener('focus', () => {
        if (String(state().clipSelected) !== button.dataset.clipId) select(button.dataset.clipId, false);
      });
    });
    document.querySelectorAll('[data-clips-lane]').forEach(lane => lane.addEventListener('click', event => {
      if (event.target.closest('[data-clip-id]')) return;
      local.cursor = Math.max(0, snap((event.clientX - lane.getBoundingClientRect().left) / local.pixels));
      state().track = Number(lane.dataset.clipsLane);
      refresh();
      document.getElementById('clips-timeline')?.focus();
    }));
    for (const input of [document.getElementById('clips-cursor'), document.getElementById('clips-name'),
        ...document.querySelectorAll('[data-clips-field]')].filter(Boolean)) {
      input.addEventListener('input', () => local.fieldDrafts.set(input.id, { node: input, project: project() }));
      input.addEventListener('blur', () => { local.fieldDrafts.delete(input.id); if (local.deferredPaint) refreshAfterFieldCommit(input); });
    }
    on('clips-name', 'change', event => commitField(event.target, 'rename audio clip', clip => {
      clip.name = event.target.value.trim() || clipName(clip);
    }));
    on('clips-track', 'change', event => commitField(event.target, 'move audio clip track', clip => { clip.track = Number(event.target.value); }));
    document.querySelectorAll('[data-clips-field]').forEach(input => input.addEventListener('change', () => commitField(input, 'edit audio clip', clip => {
      // An emptied field is not a value: the clip keeps what it had (the field shows it again).
      if (!String(input.value).trim() || !Number.isFinite(Number(input.value))) return;
      const value = clamp(input.value, Number(input.min), Number(input.max));
      if (input.dataset.clipsField === 'startBeat') {
        moveTo(clip, value);
      } else {
        clip[input.dataset.clipsField] = value;
      }
      constrain(clip);
    })));
    on('clips-muted', 'change', event => commitField(event.target, 'mute audio clip', clip => { clip.muted = event.target.checked; }));
    on('clips-duplicate', 'click', duplicate); on('clips-split', 'click', split); on('clips-delete', 'click', remove);
    on('clips-preview', 'click', () => {
      const clip = selected();
      if (clip) {
        local.preview = clipPath(clip);
        app().request('sample.preview', { path: local.preview }).catch(error => app().toast(error.message, 'error'));
      }
    });
    on('clips-stop-preview', 'click', () => app().request('sample.stopPreview', {}).catch(error => app().toast(error.message, 'error')));
    on('clips-source', 'click', () => {
      const clip = selected();
      if (clip) app().request('app.external', { path: clipPath(clip), reveal: true }).catch(error => app().toast(error.message, 'error'));
    });
    window.STClipTakes?.bind();
    queueMicrotask(loadWaveforms);
  }
  function key(event) {
    if (window.STSong?.key?.(event)) return true;
    if (event.defaultPrevented || !event.target?.closest?.('.clips-panel')) return false;
    if (/INPUT|TEXTAREA|SELECT/.test(event.target.tagName) || event.target.isContentEditable) return false;
    // Toolbar, track-label and inspector buttons keep their native interaction.
    // Only the timeline itself or an actual clip owns clip-editing shortcuts.
    const focusedClip = event.target.closest('[data-clip-id]');
    if (event.target.id !== 'clips-timeline' && !focusedClip) return false;
    const name = event.key.toLowerCase();
    const command = event.metaKey || event.ctrlKey;
    const editing = command
        ? name === 'd'
        : ['delete', 'backspace', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', '[', ']', 'pageup', 'pagedown', 't'].includes(name)
            || name === 's' && !event.altKey
            || ['home', 'end'].includes(name) && !event.altKey && !event.shiftKey;
    if (!editing) return false;
    // Tab can focus a clip other than the prior selection. Resolve the operation
    // from that actual focused clip even when an inactive WebView omits focus events.
    if (focusedClip && String(state().clipSelected) !== focusedClip.dataset.clipId) {
      if (!clips().some(clip => String(clip.id) === focusedClip.dataset.clipId)) return false;
      select(focusedClip.dataset.clipId, false);
    }
    if ((event.metaKey || event.ctrlKey) && name === 'd') { event.preventDefault(); duplicate(); return true; }
    if (event.metaKey || event.ctrlKey) return false;
    if (name === 'pageup'
        || name === 'pagedown') { if (event.altKey || event.shiftKey) return false; event.preventDefault(); window.STClipTakes?.cycle(name === 'pageup' ? -1 : 1); return true; }
    if (name === 't' && !event.altKey && !event.shiftKey) { event.preventDefault(); window.STClipTakes?.show(); return true; }
    if (name === 'delete' || name === 'backspace') { event.preventDefault(); remove(); return true; }
    if (name === 's' && !event.altKey) { event.preventDefault(); split(); return true; }
    const clip = selected();
    if (name === 'home' || name === 'end') {
      event.preventDefault();
      const ordered = clips().slice().sort((a, b) => a.track - b.track || a.startBeat - b.startBeat);
      const next = name === 'home' ? ordered[0] : ordered[ordered.length - 1];
      if (next) select(next.id);
      return true;
    }
    if (['ArrowUp', 'ArrowDown'].includes(event.key)) {
      event.preventDefault();
      const delta = event.key === 'ArrowUp' ? -1 : 1;
      if (event.altKey && clip) edit('move audio clip track',
          value => { value.track = clamp(value.track + delta, 0, project().tracks.length - 1); state().track = value.track; });
      else { const ordered = clips().slice().sort((a, b) => a.track - b.track
          || a.startBeat - b.startBeat); const index = ordered.findIndex(value => value.id === clip?.id); const next = ordered[index < 0
          ? (delta < 0 ? ordered.length - 1 : 0)
          : clamp(index + delta, 0, ordered.length - 1)]; if (next) select(next.id); }
      focusSelected(); return true;
    }
    if (['ArrowLeft', 'ArrowRight'].includes(event.key)) {
      event.preventDefault(); const direction = event.key === 'ArrowLeft' ? -1 : 1;
      let delta; try { const anchor = clip
          ? clip.startBeat + (event.shiftKey && !event.altKey ? clip.durationBeats : 0)
          : local.cursor; delta = local.snap === 'bar'
              ? neighbor(anchor,direction) - anchor
              : direction * (snapValue() || .0625); } catch(error) { app().toast(error.message,'error'); return true; }
      if (clip) edit(event.altKey ? 'slip audio clip' : event.shiftKey ? 'trim audio clip' : 'move audio clip', value => {
        if (event.altKey) value.sourceOffset = Number(value.sourceOffset || 0) + Math.sign(delta) * seconds(value, Math.abs(delta));
        else if (event.shiftKey) value.durationBeats += delta;
        else moveTo(value, value.startBeat + delta);
        constrain(value);
      }); else { local.cursor = clamp(local.cursor + delta,0,maxBeat()); refresh(); }
      focusSelected(); return true;
    }
    if (name === '[' || name === ']') { event.preventDefault(); try { local.cursor = local.snap === 'bar'
        ? neighbor(local.cursor,name === '[' ? -1 : 1)
        : clamp(local.cursor + (name === '[' ? -1 : 1) * (snapValue() || .25),0,
            maxBeat()); refresh(); focusSelected(); } catch(error) { app().toast(error.message,'error'); } return true; }
    return false;
  }
  function event(type, payload = {}) {
    window.STSong?.event?.(type, payload);
    if (type === 'status') {
      receivedStatus={owner:project(),signature:timingSignature(project()),payload};
      const line = document.getElementById('clips-playhead');
      if (line) {
        line.style.left = `${132 + playheadBeat(payload) * local.pixels}px`;
        line.style.display = payload.playing ? 'block' : 'none';
      }
    }
    if (type === 'audio.clipLoaded') {
      if (payload.path && payload.duration) {
        local.metadata.set(payload.path, payload);
        local.failed.delete(payload.path);
      }
      if (state().view === 'song') refresh();
    }
    if (type === 'audio.clipError') {
      if (payload.path) local.failed.add(payload.path);
      app().toast(payload.error || 'An audio clip could not be loaded.', 'error');
      if (state().view === 'song') refresh();
    }
  }
  window.STClips = { render, bind, key, event, importFile, addFile, bounceTrack };
})();
