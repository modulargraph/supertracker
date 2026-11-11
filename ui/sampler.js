(() => {
  'use strict';
  const ST = () => window.ST;
  const state = () => ST().state;
  const current = () => state().project.instruments[state().instrument];
  function destination(inst = current()) {
    const s = state(), project = s.project, instrumentID = project.instruments.indexOf(inst);
    return { project, instrument: inst, instrumentID, track: s.track ?? 0, trackObject: project.tracks[s.track ?? 0] };
  }
  const resolveInstrument = (context, project = state().project) => project.instruments[context.instrumentID];
  const ownsDestination = (context, selected = false) => state().project === context.project && resolveInstrument(context) === context.instrument &&
    (!selected || current() === context.instrument
        && (state().track ?? 0) === context.track
        && context.project.tracks[context.track] === context.trackObject);
  const nativeTarget = context => {
    if (context.instrumentID < 0) throw new Error('The sample destination is no longer available.');
    return { instrument: context.instrumentID };
  };
  function publishMetadata(context, result) {
    state().samples ||= {};
    state().samples[context.instrumentID] = result;
  }
  const previewStep = (context, note, extra = {}) => ({
    track: context.track,
    step: { note, ...(note < 0 ? {} : { instrument: context.instrumentID }), ...extra }
  });
  const esc = value => ST().escapeHTML(String(value ?? ''));
  const voiceTrack = () => state().project.tracks[state().track];
  const limit = (value, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(Number(value)) ? Number(value) : lo));
  const note = value => `${['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][((value % 12) + 12) % 12]}${Math.floor(value / 12) - 1}`;
  const sliceNote = (root, index, count) => { let value = root + index; while (value > 127) value -= count; return note(value); };
  const fileName = path => String(path || '').split('/').pop();
  const markerList = inst => inst.slicePoints || [];
  const slicing = {
    sensitivity: .5,
    minimumGapSeconds: .04,
    maximumSlices: 128,
    beats: 4,
    division: 16,
    analysis: null,
    feedback: null,
    settingsOpen: false
  };
  const audition = { instrument: null, selected: 0, generation: 0, job: null, timer: null, message: '', error: false };
  let fieldScope = null, fieldPointerHeld = false, fieldPointerGuards = false;
  function validFields(scope) {
    return scope === fieldScope
        && scope.project === state().project
        && scope.instrument === current()
        && state().view === 'instrument'
        && !state().modal
        && scope.anchor?.isConnected !== false;
  }
  function paintFields(scope) {
    scope.pending = true; clearTimeout(scope.timer);
    scope.timer = setTimeout(() => {
      if (!validFields(scope) || fieldPointerHeld || scope.drafts.has(document.activeElement)) return;
      scope.pending = false;
      const focus = document.activeElement?.id;
      ST().refresh();
      if (focus && !state().modal) document.getElementById(focus)?.focus({preventScroll:true});
    },0);
  }
  function bindFields() {
    const scope = fieldScope = {
      project: state().project,
      instrument: current(),
      anchor: document.getElementById('sampler-voice-mode'),
      drafts: new Set(),
      pending: false,
      timer: null
    };
    if (!fieldPointerGuards && document.addEventListener) {
      fieldPointerGuards = true;
      document.addEventListener('pointerdown',() => { fieldPointerHeld = true; },true);
      const released = () => { fieldPointerHeld = false; if (fieldScope?.pending) paintFields(fieldScope); };
      for (const type of ['pointerup','pointercancel']) document.addEventListener(type,released,true);
      window.addEventListener?.('blur',released);
    }
    for (const selector of ['[data-sampler-marker]','[data-sampler-zone-field]']) document.querySelectorAll(selector).forEach(input => {
      input.addEventListener('input',() => scope.drafts.add(input));
      input.addEventListener('blur',() => { scope.drafts.delete(input); if(scope.pending)paintFields(scope); });
    });
    return scope;
  }
  function slicePoints(inst) {
    const values = markerList(inst).filter(value => Number.isFinite(value) && value >= 0 && value < 1);
    return values.length
        ? [...new Set([0, ...values])].sort((a, b) => a - b).slice(0, 128)
        : Array.from({ length: Math.max(1, Math.min(128, inst.sliceCount || 16)) },
            (_, index) => index / Math.max(1, Math.min(128, inst.sliceCount || 16)));
  }
  function saveMarkers(points, label, focusID, destination = current(), options = {}) {
    stopAudition();
    const values = [...new Set([0,
        ...points.map(Number).filter(value => Number.isFinite(value) && value > 0 && value < 1)])].sort((a, b) => a - b).slice(0,
        128);
    ST().mutate(label, () => {
      destination.slicePoints = values;
      destination.sliceCount = values.length;
      destination.mode = 'slice';
      state().selection = null;
    }, options);
    if (options.render !== false
        && focusID
        && state().view === 'instrument'
        && current() === destination
        && !state().modal) document.getElementById(focusID)?.focus({
      preventScroll: true
    });
  }
  const sliceSignature = inst => JSON.stringify([inst.type, inst.samplePath, inst.start, inst.end, inst.mode, inst.sliceCount,
      inst.slicePoints]);
  const auditionSignature = inst => JSON.stringify([sliceSignature(inst), inst.rootNote, inst.zones || []]);
  function selectedSlice(inst = current()) {
    if (audition.instrument !== inst) { audition.instrument = inst; audition.selected = 0; audition.message = ''; audition.error = false; }
    audition.selected = Math.max(0, Math.min(slicePoints(inst).length - 1, audition.selected));
    return audition.selected;
  }
  function auditionStatus() {
    const inst = current();
    if (!inst || inst.type !== 'sample') return '';
    const index = selectedSlice(inst);
    if (audition.message) return audition.message;
    return `Slice ${index + 1} · ${note(inst.rootNote)} · Preview up to 2 seconds`;
  }
  function updateSliceSelection() {
    const inst = current(); if (!inst || inst.type !== 'sample') return;
    const index = selectedSlice(inst);
    document.querySelectorAll('[data-sampler-row]').forEach(row => row.classList.toggle('is-selected',Number(row.dataset.samplerRow) === index));
    document.querySelectorAll('[data-sampler-slice-region]').forEach(region => region.classList.toggle('is-selected',
        Number(region.dataset.samplerSliceRegion) === index));
    for (const [selector, field] of [['[data-sampler-audition]','samplerAudition'],['[data-sampler-marker]','samplerMarker'],
        ['[data-sampler-remove-marker]','samplerRemoveMarker']]) {
      document.querySelectorAll(selector).forEach(control => {
        const selected = Number(control.dataset[field]) === index;
        control.tabIndex = selected && !control.disabled ? 0 : -1;
        if (field === 'samplerAudition') control.setAttribute('aria-current',String(selected));
      });
    }
    const status = document.getElementById('sampler-audition-status');
    if (status) { status.textContent = auditionStatus(); status.classList.toggle('is-error',audition.error); }
    const stop = document.getElementById('sampler-stop-audition'); if (stop) stop.disabled = !audition.job;
  }
  function stopAudition(message = '') {
    const editorStopped = stopEditorAudition();
    const job = audition.job;
    audition.generation++; audition.job = null; clearTimeout(audition.timer); audition.timer = null;
    audition.message = typeof message === 'string' ? message : ''; audition.error = false;
    // Cancel only the preview owned here. This command also invalidates a native
    // audition that is still waiting for sample loading; it never stops transport.
    if (job?.sent) ST().request('step.preview',{...previewStep(job,-1),duration:.03}).catch(() => {});
    updateSliceSelection();
    return !!job || editorStopped;
  }
  function selectSlice(index, focus = false) {
    const inst = current(); if (!inst || inst.type !== 'sample') return;
    const selected = Math.max(0,Math.min(slicePoints(inst).length - 1,index));
    if (selected !== selectedSlice(inst)) stopAudition();
    audition.selected = selected; audition.message = ''; audition.error = false; updateSliceSelection();
    if (focus) {
      const button = document.getElementById(`sampler-audition-${selected}`);
      button?.focus({ preventScroll: true });
      button?.scrollIntoView?.({ block: 'nearest' });
    }
  }
  function validAuditionOwner(job) {
    const s = state();
    return audition.job === job && audition.generation === job.generation && ownsDestination(job,true) &&
      s.project.tracks[job.track] === job.trackObject && (s.track ?? 0) === job.track && s.view === 'instrument' && !s.modal &&
      auditionSignature(job.instrument) === job.signature && selectedSlice(job.instrument) === job.index;
  }
  function validAudition(job) {
    const panel = document.getElementById('sampler-slice-map');
    return validAuditionOwner(job) && !!panel && panel.isConnected !== false && panel.contains(document.activeElement);
  }
  async function auditionSlice(index) {
    const inst = current();
    if (!inst || inst.type !== 'sample' || !inst.samplePath || !Number.isInteger(index) || index < 0 || index >= slicePoints(inst).length) return;
    stopAudition(); selectSlice(index);
    const job = {...destination(inst),generation:audition.generation,index,signature:auditionSignature(inst),sent:false};
    audition.job = job; audition.message = `Preparing slice ${index + 1}…`; updateSliceSelection();
    try {
      // Build only after pending parameter edits and graph work settle. The shared
      // helper tracks native restoration, so a later control edit cannot overtake it.
      const result = await ST().syncEngineProject(() => validAudition(job) ? window.STUtil.clone(state().project) : null);
      if (result === false) return;
      if (result == null) throw new Error('Could not synchronize the sample before auditioning.');
      if (!validAudition(job)) return;
      const root = Math.round(limit(inst.rootNote,0,127));
      job.sent = true;
      const response = await ST().request('step.preview',{...previewStep(job,root,{velocity:100,fx1:{type:'slice',value:index + 1}}),duration:2});
      if (!validAudition(job)) return;
      if (response?.auditioning === false) throw new Error('Slice preview did not start.');
      audition.message = `Auditioning slice ${index + 1} · ${note(root)} · up to 2 seconds`; updateSliceSelection();
      audition.timer = setTimeout(() => { if (audition.job === job) stopAudition('Preview finished · Return or Space to hear again'); },2000);
    } catch (error) {
      if (audition.job === job) {
        stopAudition();
        audition.message = error.message || 'Slice audition failed.';
        audition.error = true;
        updateSliceSelection();
        ST().toast(audition.message, 'error');
      }
    } finally {
      if (audition.job === job && !audition.timer) stopAudition();
    }
  }
  function commitMarker(input, scope = null) {
    if (scope && !validFields(scope)) return false;
    const points = slicePoints(current()), index = Number(input.dataset.samplerMarker);
    if (!index) return true;
    if (!String(input.value).trim()
        || !Number.isFinite(Number(input.value))) { input.value = (points[index] * 100).toFixed(3); ST().toast('Enter a slice start percentage.',
            'error'); return false; }
    const value = limit(Number(input.value) / 100,(points[index - 1] ?? 0) + .00001,(points[index + 1] ?? 1) - .00001);
    if (Math.abs(value - points[index]) > 1e-12) { points[index] = value; saveMarkers(points,'move slice marker',input.id,current(),
        scope ? {render:false} : {}); }
    else input.value = (points[index] * 100).toFixed(3);
    if (scope) { scope.drafts.delete(input); paintFields(scope); }
    return true;
  }
  function key(event) {
    if (state().view !== 'instrument' || state().modal || event.isComposing) return false;
    if (editorKey(event)) return true;
    if (event.metaKey || event.ctrlKey || event.altKey) return false;
    if (event.key === 'Escape' && audition.job) { event.preventDefault(); event.stopPropagation?.(); stopAudition('Preview stopped'); return true; }
    const target = event.target;
    if (target?.dataset?.samplerAudition !== undefined) {
      const index = Number(target.dataset.samplerAudition), count = slicePoints(current()).length;
      if (!event.shiftKey && ['ArrowUp','ArrowDown','Home','End'].includes(event.key)) {
        event.preventDefault(); event.stopPropagation?.(); selectSlice(event.key === 'Home'
            ? 0
            : event.key === 'End'
            ? count - 1
            : index + (event.key === 'ArrowDown' ? 1 : -1),
            true); return true;
      }
      if (!event.shiftKey
          && ['Enter',' '].includes(event.key)) { event.preventDefault(); event.stopPropagation?.(); if (!event.repeat) void auditionSlice(index); return true; }
    }
    if (target?.dataset?.samplerMarker !== undefined && event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault(); event.stopPropagation?.();
      if (!event.repeat && commitMarker(target)) void auditionSlice(Number(target.dataset.samplerMarker));
      return true;
    }
    return false;
  }
  function sliceFeedback(inst = current()) {
    if (slicing.analysis?.instrument === inst) return slicing.analysis.cancelled
        ? 'Canceled · finishing background analysis…'
        : 'Detecting transients in the playback region…';
    return slicing.feedback?.instrument === inst ? slicing.feedback.message : 'Detection adds markers only; your source audio stays unchanged.';
  }
  function updateSliceStatus() {
    const busy = !!slicing.analysis, inst = current();
    const button = document.getElementById('sampler-detect-transients');
    if (button) { button.disabled = busy || !inst?.samplePath; button.textContent = busy ? 'Analyzing…' : 'Detect transients'; }
    const cancel = document.getElementById('sampler-cancel-analysis');
    if (cancel) { cancel.hidden = !busy; cancel.disabled = !!slicing.analysis?.cancelled; }
    const status = document.getElementById('sampler-slice-status'); if (status) status.textContent = sliceFeedback(inst);
    document.getElementById('sampler-transient-tools')?.setAttribute('aria-busy', String(busy));
  }
  async function detectTransients() {
    if (slicing.analysis) return;
    const instrument = current();
    if (!instrument?.samplePath) { ST().toast('Load or record a sample before detecting transients.'); return; }
    const job = { ...destination(instrument), signature: sliceSignature(instrument), cancelled: false };
    slicing.analysis = job; slicing.feedback = null; updateSliceStatus();
    try {
      if (!ownsDestination(job,true)) return;
      const result = await ST().request('sample.analyzeSlices', { ...nativeTarget(job), path: instrument.samplePath,
        start: instrument.start ?? 0, end: instrument.end ?? 1, sensitivity: slicing.sensitivity,
        minimumGapSeconds: slicing.minimumGapSeconds, maximumSlices: slicing.maximumSlices });
      if (job.cancelled) { slicing.feedback = { instrument, message: 'Detection canceled. Existing markers kept.' }; return; }
      if (!ownsDestination(job,true) || sliceSignature(instrument) !== job.signature) {
        slicing.feedback = { instrument, message: 'Detection discarded because its sample, region or markers changed.' };
        ST().toast('Slice detection was discarded because its destination changed. Existing edits were kept.', 'info'); return;
      }
      const points = result?.slicePoints;
      if (!Array.isArray(points)
          || !points.length
          || points.length > 128
          || points[0] !== 0
          || points.some((point, index) => !Number.isFinite(point) || point < 0 || point >= 1 || (index > 0 && point <= points[index - 1]))) {
        throw new Error('The audio engine returned invalid slice markers. Existing markers were kept.');
      }
      slicing.feedback = {
        instrument,
        message: `${points.length} slice${points.length === 1 ? '' : 's'} detected.${result.capped
            ? ' Maximum reached; adjust the limit or sensitivity to change the map.'
            : points.length === 1
            ? ' Increase sensitivity to find quieter hits.'
            : ' Adjust any marker below.'}`
      };
      const focusID = document.activeElement?.id;
      slicing.analysis = null;
      saveMarkers(points, 'detect sample transients', focusID || 'sampler-detect-transients', instrument);
    } catch (error) {
      slicing.feedback = {
        instrument,
        message: job.cancelled ? 'Detection canceled. Existing markers kept.' : error.message || 'Slice detection failed. Existing markers were kept.'
      };
      if (!job.cancelled) ST().toast(slicing.feedback.message, 'error');
    } finally {
      if (slicing.analysis === job) slicing.analysis = null;
      updateSliceStatus();
    }
  }
  function beatSliceCount() { return slicing.beats * slicing.division / 4; }
  function beatEstimate() {
    const count = beatSliceCount();
    return Number.isInteger(count) && count >= 1 && count <= 128
        ? `${count} equal slices across the playback region`
        : 'Choose a beat length and grid yielding 1–128 whole slices.';
  }
  function updateBeatEstimate() {
    const output = document.getElementById('sampler-beat-estimate');
    if (output) output.textContent = beatEstimate();
  }
  // Sampler sections as separate pieces, so the Instrument view can place them on pages
  // (Sample · Slices · Zones · Voice & Record). render() keeps the full stack in order.
  function parts(inst) {
    if (!inst || inst.type !== 'sample') return {};
    const isSample = inst.type === 'sample';
    return {
      editor: isSample ? renderEditor(inst) : '',
      record: `<section class="panel wide sampler-capture" aria-label="Record a sample"><div class="panel-head"><h2>Record a sample</h2>`
          + `<span class="chip">LINE IN</span></div><div class="panel-body sampler-capture-row"><p class="sampler-help">Record one note or a break `
          + `from line input, then set its root note and playback region before chopping.</p><button id="sampler-record-input">Record line input…</button></div></section>`,
      voice: `<section class="panel wide sampler-voices" aria-label="Voice behavior"><div class="panel-head"><h2>Voice behavior</h2>`
          + `<span class="chip">VOICE</span></div><div class="panel-body stack"><div class="grid2"><label class="field">`
          + `<span>Voice mode · this track</span><select id="sampler-voice-mode">${[['poly', 'Polyphonic'], ['mono', 'Monophonic'], ['legato', 'Legato']].map(([value, label]) => `<option value="${value}" ${(voiceTrack()?.voiceMode || 'poly') === value ? 'selected' : ''}>${label}</option>`).join('')}</select>`
          + `</label><label class="field"><span>Choke group · this track</span><select id="sampler-choke">${Array.from({
        length: 17
      }, (_, index) => `<option value="${index}" ${Number(voiceTrack()?.chokeGroup ?? 0) === index ? 'selected' : ''}>${index === 0 ? 'Off · independent' : `Group ${index}`}</option>`).join('')}</select>`
          + `</label></div><p class="sampler-help">Voice mode and choke group belong to the track. Polyphonic overlaps notes and ratchet hits. Mono retriggers `
          + `the sample for each note or ratchet hit. Legato joins held notes without restarting the sample or envelope. Tracks in one choke group `
          + `cut each other off (within their track group: an open hat chokes a closed hat on another track).</p></div></section>`,
      slices: isSample ? renderSlices(inst) : '',
      zones: isSample ? renderZones(inst) : ''
    };
  }
  function render(inst) {
    const part = parts(inst);
    return part.record ? `${part.editor}${part.record}${part.voice}${part.slices}${part.zones}` : '';
  }
  function renderSlices(inst) {
    const points = slicePoints(inst), manual = markerList(inst).length > 0, selected = selectedSlice(inst);
    const markerRows = points.map((value, index) => `<div id="sampler-row-${index}" data-sampler-row="${index}" class="sampler-marker${index === selected ? ' is-selected' : ''}">`
        + `<button id="sampler-audition-${index}" data-sampler-audition="${index}" class="sampler-slice-audition" tabindex="${index === selected ? 0 : -1}"`
        + ` aria-current="${index === selected}" aria-label="Audition slice ${index + 1} at ${note(inst.rootNote)}" aria-describedby="sampler-audition-help"`
        + ` ${inst.samplePath ? '' : 'disabled'}>▶</button><span class="sampler-marker-index">${String(index + 1).padStart(2, '0')}</span>`
        + `<span class="sampler-marker-note">${esc(sliceNote(inst.rootNote, index, points.length))}</span>`
        + `<label><span class="sr-only">Slice ${index + 1} start percentage</span><input type="number" id="sampler-marker-${index}"`
        + ` data-sampler-marker="${index}" tabindex="${index === selected && index > 0 ? 0 : -1}" min="${index ? ((points[index - 1] + .00001) * 100).toFixed(3) : '0'}"`
        + ` max="${((points[index + 1] ?? 1) * 100 - .001).toFixed(3)}" step=".1" value="${(value * 100).toFixed(3)}" ${index === 0 ? 'disabled' : ''}>`
        + `<span>%</span></label><button id="sampler-remove-marker-${index}" data-sampler-remove-marker="${index}" tabindex="${index === selected && index > 0 ? 0 : -1}"`
        + ` aria-label="Remove slice ${index + 1}" ${index === 0 ? 'disabled' : ''}>×</button></div>`).join('');
    return `<section id="sampler-slice-map" class="panel wide sampler-slices" aria-label="Slice map"><div class="panel-head">`
        + `<h2>Slice map</h2><span class="chip">${points.length} SLICES${inst.mode === 'slice' ? '' : ' · SLICE MODE OFF'}</span>`
        + `</div><div class="panel-body sampler-slice-layout">
      <div class="sampler-slice-tools"><p class="sampler-help">Chop the playback region (${(inst.start * 100).toFixed(1)}–${(inst.end * 100).toFixed(1)}% `
          + `of the source). Set Start/End on the Sample page first to leave out silence. Editing markers enables Slice play mode.</p>
      <div id="sampler-transient-tools" class="sampler-transient-tools" aria-busy="${!!slicing.analysis}"><h3 class="sampler-subhead">Detect `
          + `transients</h3><div class="sampler-inline"><button id="sampler-detect-transients" ${!inst.samplePath || slicing.analysis ? 'disabled' : ''}>${slicing.analysis ? 'Analyzing…' : 'Detect transients'}</button>`
          + `<button id="sampler-cancel-analysis" ${slicing.analysis ? '' : 'hidden'} ${slicing.analysis?.cancelled ? 'disabled' : ''}>Cancel</button>`
          + `<label class="field sampler-sensitivity"><span>Sensitivity <output id="sampler-sensitivity-value">${Math.round(slicing.sensitivity * 100)}%</output>`
          + (`</span><input id="sampler-transient-sensitivity" type="range" min="0" max="1" step=".05" value="${slicing.sensitivity}"`
          + ` aria-label="Transient `)
          + `sensitivity" aria-valuetext="${Math.round(slicing.sensitivity * 100)} percent"></label></div>
      <details id="sampler-detection-settings" class="sampler-more" ${slicing.settingsOpen ? 'open' : ''}><summary>Detection settings</summary>`
          + `<div class="grid2"><label class="field"><span>Minimum gap · ms</span><input id="sampler-transient-gap" type="number" min="5"`
          + ` max="2000" step="5" value="${Math.round(slicing.minimumGapSeconds * 1000)}"></label><label class="field">`
          + `<span>Maximum slices</span><input id="sampler-transient-limit" type="number" min="1" max="128" step="1" value="${slicing.maximumSlices}">`
          + `</label></div><p class="sampler-help">Higher sensitivity includes quieter hits. A longer minimum gap avoids extra markers within one hit.</p>`
          + `</details><p id="sampler-slice-status" class="sampler-help sampler-slice-status" role="status" aria-live="polite">${esc(sliceFeedback(inst))}</p></div>
      <div class="sampler-chop-tools"><h3 class="sampler-subhead">Chop evenly</h3><div class="sampler-inline"><label class="field">`
          + `<span>Equal slices</span><input id="sampler-equal-count" type="number" min="1" max="128" step="1" value="${points.length}">`
          + `</label><button id="sampler-equal">Apply equal slices</button><button id="sampler-add-marker" ${points.length >= 128 ? 'disabled' : ''}>Add `
          + `marker</button></div><div class="sampler-inline sampler-beat-grid"><label class="field"><span>Region length · beats</span>`
          + `<input id="sampler-region-beats" type="number" min=".25" max="128" step=".25" value="${slicing.beats}"></label>`
          + `<label class="field"><span>Beat grid</span><select id="sampler-beat-grid">${[4, 8, 16, 32].map(value => `<option value="${value}" ${slicing.division === value ? 'selected' : ''}>1/${value} notes</option>`).join('')}</select>`
          + `</label><button id="sampler-apply-beat-grid">Chop by beats</button></div><p id="sampler-beat-estimate" class="sampler-help">${beatEstimate()}</p></div>
      <details id="sampler-slice-help" class="sampler-more"><summary>How slices play</summary><p class="sampler-help">Enter the region’s length `
          + `in quarter-note beats. This creates equal markers; it does not detect tempo or stretch audio.</p><p class="sampler-help">Slice starts are `
          + `percentages within the sample start/end range. Play ${note(inst.rootNote)} upward to trigger successive slices. For independent pitch, `
          + `put Slice 1–${points.length} in either step FX slot beside a note; the note transposes that slice from the root note. A Slice FX without `
          + `a note does not retrigger.</p></details></div>
      <div class="sampler-slice-list"><div class="sampler-slice-ruler" aria-label="Slice start positions">${points.map((value, index) => `<span data-sampler-slice-region="${index}" class="${index === selected ? 'is-selected' : ''}" style="left:${value * 100}%;width:${((points[index + 1] ?? 1) - value) * 100}%"`
          + ` title="Slice ${index + 1}: ${(value * 100).toFixed(2)}%"><i>${index + 1}</i></span>`).join('')}</div>`
          + `<div class="sampler-audition-toolbar"><p id="sampler-audition-status" class="sampler-help${audition.error ? ' is-error' : ''}"`
          + ` role="status" aria-live="polite">${esc(auditionStatus())}</p><button id="sampler-stop-audition" ${audition.job ? '' : 'disabled'}>Stop `
          + `preview</button></div><p id="sampler-audition-help" class="sampler-help">↑↓ choose a slice · Space or Return to hear it · Tab to edit its `
          + `start · Return commits and hears · Escape stops preview</p><div class="sampler-marker-list" data-scroll="sample-markers"`
          + ` aria-label="Slice markers">${markerRows}</div><p class="sampler-help sampler-slice-actions">${manual
              ? 'Manual markers · arrow keys adjust focused values'
              : 'Evenly spaced · editable with keyboard'}</p>`
          + `</div></div></section>`;
  }
  const zoneNumber = (index, key, value, min, max, step, label) => `<input type="number" data-sampler-zone="${index}" data-sampler-zone-field="${key}" id="sampler-zone-${index}-${key}" min="${min}"`
      + ` max="${max}" step="${step}" value="${value}" aria-label="Zone ${index + 1} ${label}" aria-describedby="sampler-zone-name-${index}">`;
  function renderZones(inst) {
    const zones = inst.zones || [];
    return `<section class="panel wide sampler-zones"><div class="panel-head"><h2>Multisample zones</h2><div class="row">`
        + `<span class="chip">${zones.length} / 128</span><button id="sampler-add-zone" ${zones.length >= 128 ? 'disabled' : ''}>+ Add sample zone</button>`
        + `</div></div><div class="panel-body"><div class="sampler-zones-toolbar"><p class="sampler-help">Each zone has its own pitch, key range, `
        + `and velocity range. Overlapping zones layer together. With no zones, the main sample plays across the keyboard.</p>${zones.length > 1 ? '<div class="row"><button id="sampler-distribute-keys">Distribute key ranges</button><button id="sampler-distribute-velocity">Distribute '
        + 'velocities</button></div>' : ''}</div>${zones.length ? `<div id="sampler-zone-scroll" class="sampler-zone-scroll" data-scroll="sample-zones"><table class="sampler-zone-table">`
        + `<thead><tr><th>Sample</th><th>Root</th><th>Key low</th><th>Key high</th><th>Vel low</th><th>Vel high</th>`
        + `<th>Tune · st</th><th>Level</th><th></th></tr></thead><tbody>${zones.map((zone, index) => `<tr><td class="sampler-zone-file"><div><button class="sampler-zone-preview" data-sampler-zone-preview="${index}" aria-label="Audition `
        + `zone ${index + 1}">▶</button><button id="sampler-zone-name-${index}" class="sampler-zone-name" data-sampler-zone-replace="${index}"`
        + ` title="${esc(zone.samplePath)}" aria-label="Replace sample for zone ${index + 1}: ${esc(fileName(zone.samplePath))}">${esc(fileName(zone.samplePath))}</button>`
        + `</div></td><td><div class="sampler-zone-root">${zoneNumber(index, 'rootNote', zone.rootNote ?? 60, 0, 127, 1, 'root note')}<small>${note(zone.rootNote ?? 60)}</small>`
        + `</div></td><td>${zoneNumber(index, 'keyLow', zone.keyLow ?? 0, 0, 127, 1, 'lowest key')}</td><td>${zoneNumber(index, 'keyHigh', zone.keyHigh ?? 127, 0, 127, 1, 'highest key')}</td>`
        + `<td>${zoneNumber(index, 'velocityLow', zone.velocityLow ?? 0, 0, 127, 1, 'lowest velocity')}</td><td>${zoneNumber(index, 'velocityHigh', zone.velocityHigh ?? 127, 0, 127, 1, 'highest velocity')}</td>`
        + `<td>${zoneNumber(index, 'tune', zone.tune ?? 0, -48, 48, .01, 'tuning in semitones')}</td><td>${zoneNumber(index, 'volume', zone.volume ?? 1, 0, 2, .01, 'volume multiplier')}</td>`
        + `<td><button data-sampler-zone-remove="${index}" aria-label="Remove zone ${index + 1}">×</button></td></tr>`).join('')}</tbody>`
        + `</table></div>` : '<div class="sampler-zone-empty"><span>◎</span><div><strong>No multisample zones</strong><p>Add a sample, then set its key and velocity '
        + 'ranges.</p></div></div>'}</div></section>`;
  }
  async function chooseZone(replace) {
    const context = destination();
    try {
      const result = await ST().request('dialog.openSample', {});
      const path = typeof result === 'string' ? result : result?.path;
      if (!path || result?.cancelled) return;
      if (!ownsDestination(context,true)) { ST().toast('Zone import canceled because its project or instrument changed.', 'info'); return; }
      ST().mutate(replace == null ? 'add multisample zone' : 'replace zone sample', project => {
        const inst = resolveInstrument(context, project); if (!inst) return;
        inst.zones ||= [];
        if (replace != null && inst.zones[replace]) inst.zones[replace].samplePath = path;
        else if (inst.zones.length < 128) inst.zones.push({ samplePath: path, keyLow: 0, keyHigh: 127, velocityLow: 0, velocityHigh: 127, rootNote: inst.rootNote, tune: 0, volume: 1 });
      });
      ST().toast(`${replace == null ? 'Zone added' : 'Zone sample replaced'}. Audio loads in the background.`);
    } catch (error) { ST().toast(error.message, 'error'); }
  }
  function stretchDialog() {
    const context = destination(),
      instrumentID = context.instrumentID,
      inst = context.instrument,
      path = inst.samplePath,
      destinationProject = context.project;
    if (!path) return;
    const duration = sampleMeta(inst)?.duration;
    const bpm = Number(state().project.bpm);
    ST().openModal('Time stretch sample', `<div class="stack"><p class="sampler-help">Create a new audio file with a different duration while preserving pitch. The original `
        + `stays unchanged. This edits the main sample; multisample zone files stay as they are.</p><label class="field">`
        + `<span>Length ratio · 0.25 to 4.00</span><input id="sampler-stretch-ratio" type="number" min=".25" max="4" step=".01"`
        + ` value="1"></label><div class="grid2"><label class="field"><span>Source tempo · BPM</span><input id="sampler-source-bpm"`
        + ` type="number" min="20" max="999" step=".1" value="${bpm}"></label><label class="field"><span>Target tempo · BPM</span>`
        + `<input id="sampler-target-bpm" type="number" min="20" max="999" step=".1" value="${bpm}"></label></div>`
        + `<button id="sampler-match-tempo">Calculate ratio from tempos</button><div id="sampler-stretch-estimate" class="sampler-help">${duration
            ? `Current duration: ${duration.toFixed(2)} seconds.`
            : 'Ratio 2.00 doubles the duration; ratio 0.50 halves it.'}</div>`
        + `<div id="sampler-stretch-progress" class="sampler-help" role="status"></div></div>`, '<button id="sampler-stretch-cancel">Cancel</button><button class="primary" id="sampler-stretch-apply">Create stretched sample</button>');
    const get = id => document.getElementById(id);
    const applyButton = get('sampler-stretch-apply');
    const ownsDialog = () => get('sampler-stretch-apply') === applyButton;
    function estimate() {
      const ratio = Number(get('sampler-stretch-ratio').value);
      get('sampler-stretch-estimate').textContent = duration
          ? `${duration.toFixed(2)} seconds → ${(duration * ratio).toFixed(2)} seconds`
          : `New length: ${(ratio * 100).toFixed(0)}% of the original.`;
    }
    get('sampler-stretch-ratio').oninput = estimate;
    get('sampler-match-tempo').onclick = () => {
      get('sampler-stretch-ratio').value = limit(Number(get('sampler-source-bpm').value) / Number(get('sampler-target-bpm').value), .25,
          4).toFixed(3);
      estimate();
    };
    get('sampler-stretch-cancel').onclick = ST().closeModal;
    applyButton.onclick = async () => {
      if (!ownsDialog() || applyButton.disabled) return;
      const ratio = Number(get('sampler-stretch-ratio').value);
      if (!Number.isFinite(ratio) || ratio < .25 || ratio > 4) { ST().toast('Choose a length ratio between 0.25 and 4.00.', 'error'); return; }
      get('sampler-stretch-apply').disabled = true; get('sampler-stretch-progress').textContent = 'Stretching audio and writing a new sample…';
      try {
        const result = await ST().request('sample.edit', { path, instrument: instrumentID, operation: 'stretch', ratio });
        if (!result?.path) throw new Error('The audio engine did not return a stretched sample.');
        await ST().waitForGraphEdit?.();
        if (state().project !== destinationProject || destinationProject.instruments[instrumentID] !== inst || inst.samplePath !== path) {
          try {
            const restored = await ST().syncEngineProject(() => window.STUtil.clone(state().project));
            if (restored === false) throw new Error('Restoration was canceled because the project changed again.');
            if (restored == null) throw new Error('The audio engine did not confirm restoration.');
          } catch (error) { throw new Error(`The destination changed during stretching. ${error.message} The rendered file is saved at ${result.path}`); }
          throw new Error(`The destination changed during stretching. The rendered file is saved at ${result.path}`);
        }
        state().samples ||= {}; state().samples[instrumentID] = result;
        ST().mutate('time stretch sample', project => {
          const target = project.instruments[instrumentID];
          if (target) {
            target.samplePath = result.path;
          }
        });
        if (ownsDialog()) ST().closeModal();
        ST().toast('Time stretch complete. The original sample is preserved.');
      } catch (error) { ST().toast(error.message, 'error'); if (ownsDialog()) { applyButton.disabled = false; get('sampler-stretch-progress').textContent = error.message; } }
    };
    get('sampler-stretch-ratio').focus(); get('sampler-stretch-ratio').select();
  }
  function bind() {
    const fields = bindFields();
    if (editorHeld && !validEditorAudition(editorHeld)) stopEditorAudition();
    if (audition.job && !validAuditionOwner(audition.job)) stopAudition();
    const on = (id, event, fn) => document.getElementById(id)?.addEventListener(event, fn);
    on('sampler-record-input', 'click', () => ST().showRecording());
    // Voice mode and choke group are properties of the selected track (they work across tracks of a group).
    on('sampler-voice-mode', 'change', event => ST().mutate('voice mode', p => {
      const track = p.tracks[state().track];
      if (track) track.voiceMode = event.target.value;
    }));
    on('sampler-choke', 'change', event => ST().mutate('choke group', p => {
      const track = p.tracks[state().track], value = limit(event.target.value, 0, 16);
      if (!track) return;
      if (value) track.chokeGroup = value; else delete track.chokeGroup;
    }));
    on('sampler-detect-transients', 'click', detectTransients);
    on('sampler-cancel-analysis', 'click', () => { if (slicing.analysis) { slicing.analysis.cancelled = true; updateSliceStatus(); } });
    on('sampler-transient-sensitivity', 'input', event => {
      slicing.sensitivity = limit(event.target.value, 0, 1);
      const value = `${Math.round(slicing.sensitivity * 100)}%`;
      document.getElementById('sampler-sensitivity-value').textContent = value;
      event.target.setAttribute('aria-valuetext', value);
    });
    on('sampler-transient-gap', 'change', event => {
      slicing.minimumGapSeconds = limit(event.target.value, 5, 2000) / 1000;
      event.target.value = Math.round(slicing.minimumGapSeconds * 1000);
    });
    on('sampler-transient-limit', 'change', event => {
      slicing.maximumSlices = Math.round(limit(event.target.value, 1, 128));
      event.target.value = slicing.maximumSlices;
    });
    on('sampler-detection-settings', 'toggle', event => { slicing.settingsOpen = event.target.open; });
    on('sampler-region-beats', 'input', event => { slicing.beats = Number(event.target.value); updateBeatEstimate(); });
    on('sampler-beat-grid', 'change', event => { slicing.division = Number(event.target.value); updateBeatEstimate(); });
    on('sampler-apply-beat-grid', 'click', () => {
      const count = beatSliceCount();
      if (!Number.isFinite(count)
          || !Number.isInteger(count)
          || count < 1
          || count > 128
          || ![4, 8, 16, 32].includes(slicing.division)
          || slicing.beats < .25
          || slicing.beats > 128) { ST().toast('Choose a region length and beat grid yielding 1–128 whole slices.', 'error'); return; }
      saveMarkers(Array.from({ length: count }, (_, index) => index / count), 'beat-grid sample slices', 'sampler-apply-beat-grid');
    });
    on('sampler-equal', 'click', () => {
      const count = Number(document.getElementById('sampler-equal-count').value);
      if (!Number.isInteger(count) || count < 1 || count > 128) {
        ST().toast('Choose between 1 and 128 whole slices.', 'error');
        return;
      }
      saveMarkers(Array.from({ length: count }, (_, index) => index / count), 'equal sample slices', 'sampler-equal');
    });
    on('sampler-add-marker', 'click', () => {
      const points = slicePoints(current()); if (points.length >= 128) return;
      let widest = 0, selected = 0;
      points.forEach((value, index) => { const width = (points[index + 1] ?? 1) - value; if (width > widest) { widest = width; selected = index; } });
      points.push(points[selected] + widest / 2); saveMarkers(points, 'add slice marker', `sampler-marker-${selected + 1}`);
    });
    document.querySelectorAll('[data-sampler-marker]').forEach(input => input.addEventListener('change', () => commitMarker(input,fields)));
    document.querySelectorAll('[data-sampler-audition]').forEach(button => button.addEventListener('click', () => {
      button.focus({preventScroll:true}); return auditionSlice(Number(button.dataset.samplerAudition));
    }));
    for (const [selector,field] of [['[data-sampler-audition]','samplerAudition'],['[data-sampler-marker]','samplerMarker'],
        ['[data-sampler-remove-marker]','samplerRemoveMarker']]) {
      document.querySelectorAll(selector).forEach(control => control.addEventListener('focus', () => selectSlice(Number(control.dataset[field]))));
    }
    on('sampler-stop-audition','click',() => stopAudition('Preview stopped'));
    on('sampler-slice-map','focusout',() => {
      // Marker commits rebuild this panel and restore its focused field. Wait
      // until that restoration finishes before deciding that focus has left.
      queueMicrotask(() => { if (audition.job && !document.getElementById('sampler-slice-map')?.contains(document.activeElement)) stopAudition(); });
    });
    updateSliceSelection();
    document.querySelectorAll('[data-sampler-remove-marker]').forEach(button => button.addEventListener('click', () => {
      const index = Number(button.dataset.samplerRemoveMarker);
      if (!index) return;
      const points = slicePoints(current());
      points.splice(index, 1);
      saveMarkers(points, 'remove slice marker', points.length > 1 ? `sampler-marker-${Math.max(1, index - 1)}` : 'sampler-add-marker');
    }));
    // Keep the existing table viewport (and sticky column labels) in the outer
    // Instrument scroller when focus enters a lower row. No focus/model changes.
    on('sampler-zone-scroll', 'focusin', event => {
      if (!validFields(fields) || document.activeElement !== event.target) return;
      event.target.scrollIntoView?.({block:'nearest',inline:'nearest'});
      document.getElementById('sampler-zone-scroll')?.scrollIntoView?.({block:'nearest',inline:'nearest'});
    });
    on('sampler-add-zone', 'click', () => chooseZone());
    document.querySelectorAll('[data-sampler-zone-replace]').forEach(button => button.addEventListener('click',
        () => chooseZone(Number(button.dataset.samplerZoneReplace))));
    document.querySelectorAll('[data-sampler-zone-remove]').forEach(button => button.addEventListener('click', () => ST().mutate('remove multisample zone', () => {
      current().zones.splice(Number(button.dataset.samplerZoneRemove), 1);
    })));
    document.querySelectorAll('[data-sampler-zone-preview]').forEach(button => button.addEventListener('click', () => {
      const zone = current().zones[Number(button.dataset.samplerZonePreview)];
      ST().request('sample.preview', { path: zone.samplePath }).catch(error => ST().toast(error.message, 'error'));
    }));
    document.querySelectorAll('[data-sampler-zone-field]').forEach(input => input.addEventListener('change', () => {
      if (!validFields(fields)) return;
      ST().mutate('edit multisample zone', () => {
      const zone = current().zones[Number(input.dataset.samplerZone)], field = input.dataset.samplerZoneField;
      zone[field] = limit(input.value, Number(input.min), Number(input.max));
      if (!['tune', 'volume'].includes(field)) zone[field] = Math.round(zone[field]);
      if (field === 'keyLow' && zone.keyLow > (zone.keyHigh ?? 127)) zone.keyHigh = zone.keyLow;
      if (field === 'keyHigh' && zone.keyHigh < (zone.keyLow ?? 0)) zone.keyLow = zone.keyHigh;
      if (field === 'velocityLow' && zone.velocityLow > (zone.velocityHigh ?? 127)) zone.velocityHigh = zone.velocityLow;
      if (field === 'velocityHigh' && zone.velocityHigh < (zone.velocityLow ?? 0)) zone.velocityLow = zone.velocityHigh;
      },{render:false});
      fields.drafts.delete(input); paintFields(fields);
    }));
    on('sampler-distribute-keys', 'click', () => ST().mutate('distribute zone keys', () => {
      const zones = [...current().zones].sort((a, b) => (a.rootNote ?? 60) - (b.rootNote ?? 60));
      const groups = [...new Set(zones.map(zone => zone.rootNote ?? 60))];
      zones.forEach(zone => {
        const root = zone.rootNote ?? 60,
          index = groups.indexOf(root);
        zone.keyLow = index ? Math.floor((groups[index - 1] + root) / 2) + 1 : 0;
        zone.keyHigh = index < groups.length - 1 ? Math.floor((root + groups[index + 1]) / 2) : 127;
      });
      current().zones = zones;
    }));
    on('sampler-distribute-velocity', 'click', () => ST().mutate('distribute zone velocities', () => {
      const zones = current().zones;
      zones.forEach((zone, index) => {
        zone.velocityLow = Math.floor(index * 128 / zones.length);
        zone.velocityHigh = Math.floor((index + 1) * 128 / zones.length) - 1;
      });
    }));
    on('sampler-stretch', 'click', stretchDialog);
    if (current()?.type === 'sample') bindEditor();
  }
  // ---- Sample editor: zoomable waveform, keyboard markers with zero-crossing snap, region edits, slicing. ----
  // Edits write new files through sample.edit (source library files are never touched) and are one Undo step.
  const PLAY_MODES = [['oneshot', '1-Shot'], ['forward', 'Forward loop'], ['reverse', 'Backward loop'], ['pingpong', 'Ping-pong'],
      ['slice', 'Slice'], ['wavetable', 'Wavetable'], ['granular', 'Granular']];
  const MODE_HELP = {
    oneshot: 'Plays the region once, start to end.',
    forward: 'Plays from start, then repeats loop start → loop end while the note is held.',
    reverse: 'Plays backwards from the end, then loops backwards between the loop points.',
    pingpong: 'Bounces between loop start and loop end.',
    slice: 'Each key from the root note triggers the next slice at its original pitch.',
    wavetable: 'Plays single-cycle frames of the region (a wavetable); Position morphs between them.',
    granular: 'Overlapping grains around Position; shape them in the Granular panel above.'
  };
  const LOOP_MODES = new Set(['forward', 'reverse', 'pingpong']);
  const FADE_CURVES = [['linear', 'Linear'], ['smooth', 'S-curve'], ['exponential', 'Exponential']];
  const EDIT_LABELS = {
    trim: 'Trim to region',
    fadeIn: 'Fade in',
    fadeOut: 'Fade out',
    normalize: 'Normalize',
    reverse: 'Reverse region',
    gain: 'Gain',
    removeDC: 'Remove DC offset',
    silence: 'Silence region'
  };
  const editors = new WeakMap(), editorJob = { current: null };
  let editorBurst = null, editorRefreshTimer = null, editorDrag = null, editorHeld = null, editorGlobals = false;
  function editorState(inst) {
    let value = editors.get(inst);
    if (!value) {
      value = { view: [0, 1], marker: 'start', snap: false, curve: 'linear', gain: -3, peaks: null, pending: null, message: '' };
      editors.set(inst, value);
    }
    return value;
  }
  const sampleMeta = inst => {
    const context = destination(inst);
    return state().samples?.[context.instrumentID] || null;
  };
  function sampleFrames(inst) {
    const meta = sampleMeta(inst);
    return Math.max(0, Math.round(Number(meta?.frames) || (meta?.duration && meta?.sampleRate ? meta.duration * meta.sampleRate : 0)));
  }
  function sampleSeconds(inst) {
    const meta = sampleMeta(inst);
    return Number(meta?.duration) || (meta?.frames && meta?.sampleRate ? meta.frames / meta.sampleRate : 0);
  }
  const frameSize = inst => 1 / Math.max(1, sampleFrames(inst) || 100000);
  function timeText(inst, position) {
    const seconds = sampleSeconds(inst);
    return seconds ? `${(position * seconds).toFixed(seconds * position >= 10 ? 2 : 3)} s` : `${(position * 100).toFixed(2)}%`;
  }
  function editorMarkers(inst) {
    const start = inst.start, end = inst.end;
    const list = [{ key: 'start', label: 'Start', pos: start }, { key: 'end', label: 'End', pos: end }];
    if (LOOP_MODES.has(inst.mode)) list.push({ key: 'loopStart', label: 'Loop start', pos: inst.loopStart },
        { key: 'loopEnd', label: 'Loop end', pos: inst.loopEnd });
    if (inst.mode === 'slice') slicePoints(inst).forEach((point, index) => {
      if (index) list.push({ key: `slice:${index}`, label: `Slice ${index + 1}`, pos: start + point * (end - start), slice: index });
    });
    return list;
  }
  function selectedMarker(inst) {
    const markers = editorMarkers(inst), s = editorState(inst);
    let marker = markers.find(item => item.key === s.marker);
    if (!marker) { marker = markers[0]; s.marker = marker.key; }
    return marker;
  }
  function markerBounds(inst, key) {
    const frame = frameSize(inst), start = inst.start, end = inst.end;
    if (key === 'start') return [0, end - frame];
    if (key === 'end') return [start + frame, 1];
    if (key === 'loopStart') return [Math.min(start, inst.loopStart), inst.loopEnd - frame];
    if (key === 'loopEnd') return [inst.loopStart + frame, Math.max(end, inst.loopEnd)];
    const points = slicePoints(inst), index = Number(key.split(':')[1]), width = end - start;
    return [start + points[index - 1] * width + frame, start + (points[index + 1] ?? 1) * width - frame];
  }
  // Sets one marker on the instrument (no Undo bookkeeping); positions land on whole frames.
  function placeMarker(inst, key, position) {
    const frames = sampleFrames(inst), [lo, hi] = markerBounds(inst, key);
    let value = limit(position, lo, Math.max(lo, hi));
    if (frames) value = limit(Math.round(value * frames) / frames, lo, Math.max(lo, hi));
    if (key.startsWith('slice:')) {
      const points = slicePoints(inst), index = Number(key.split(':')[1]), start = inst.start, end = inst.end;
      points[index] = (value - start) / (end - start);
      inst.slicePoints = points; inst.sliceCount = points.length;
      return;
    }
    inst[key] = value;
    if ((key === 'start' || key === 'end') && LOOP_MODES.has(inst.mode)) {
      // Loop points live inside the playback region, as on the hardware.
      const frame = frameSize(inst);
      inst.loopStart = limit(inst.loopStart, inst.start, inst.end - frame);
      inst.loopEnd = limit(inst.loopEnd, inst.loopStart + frame, inst.end);
    }
  }
  function scheduleEditorRefresh() {
    clearTimeout(editorRefreshTimer);
    editorRefreshTimer = setTimeout(() => {
      if (editorDrag) { scheduleEditorRefresh(); return; }
      if (state().view !== 'instrument' || state().modal || !document.getElementById('sampler-editor')) return;
      ST().refresh();
    }, 650);
  }
  // A held or repeated nudge on one marker is a single Undo step.
  function commitEditorMarker(inst, key, position, label = 'move sample marker') {
    const s = state(),
      beforeTop = s.undo?.[s.undo.length - 1],
      burst = editorBurst
          && editorBurst.top === beforeTop
          && editorBurst.inst === inst
          && editorBurst.key === key
          && editorBurst.label === label
          && editorBurst.project === s.project
          && Date.now() - editorBurst.at < 1500;
    const previousUndo = burst && Array.isArray(s.undo) ? s.undo.slice() : null;
    ST().mutate(label, () => placeMarker(inst, key, position), { render: false });
    if (previousUndo
        && ownsDestination(destination(inst))
        && s.undo?.[s.undo.length - 1] !== beforeTop) s.undo.splice(0, s.undo.length, ...previousUndo);
    if (ownsDestination(destination(inst)) && (s.undo?.[s.undo.length - 1] !== beforeTop || burst)) editorBurst = {
      inst,
      key,
      label,
      project: s.project,
      top: s.undo?.[s.undo.length - 1],
      at: Date.now()
    };
    revealPosition(inst, selectedMarker(inst).pos); drawEditor(); scheduleEditorRefresh();
  }
  async function snapPosition(inst, position, direction) {
    if (!state().native || !inst.samplePath) return position;
    const context = destination(inst);
    if (!ownsDestination(context,true)) throw new Error('The sample destination changed.');
    const result = await ST().request('sample.peaks',
        { ...nativeTarget(context), path: inst.samplePath, start: 0, end: 0, bins: 1, snap: [position], direction });
    const value = Number(result?.snapped?.[0]);
    return Number.isFinite(value) ? value : position;
  }
  const markerSignature = inst => JSON.stringify([inst.samplePath, inst.start, inst.end, inst.loopStart, inst.loopEnd, inst.mode, inst.sliceCount,
      inst.slicePoints]);
  function markerOwner(inst, key) {
    const owner = state().project, signature = markerSignature(inst);
    return () => state().project === owner
        && current() === inst
        && state().view === 'instrument'
        && !state().modal
        && selectedMarker(inst).key === key
        && markerSignature(inst) === signature;
  }
  async function nudgeMarker(inst, direction, size) {
    const s = editorState(inst),
      marker = selectedMarker(inst),
      valid = markerOwner(inst, marker.key),
      frame = frameSize(inst),
      span = s.view[1] - s.view[0];
    const columns = Math.max(200, document.getElementById('sampler-wave-canvas')?.clientWidth || 800);
    const step = size === 'frame' ? frame : size === 'coarse' ? span / 10 : Math.max(frame, span / columns);
    let target = marker.pos + direction * step;
    if (s.snap && size !== 'frame' && sampleFrames(inst)) {
      if (s.snapping) return; s.snapping = true;
      try {
        let snapped = await snapPosition(inst, target, 0);
        if ((snapped - marker.pos) * direction <= frame / 2) snapped = await snapPosition(inst, marker.pos + direction * frame, direction);
        target = snapped;
      } catch { /* No sample in the engine yet: move without snapping. */ }
      finally { s.snapping = false; }
      if (!valid()) return;
    }
    commitEditorMarker(inst, marker.key, target, `move ${marker.label.toLowerCase()}`);
  }
  async function snapSelectedMarker(inst) {
    const marker = selectedMarker(inst), valid = markerOwner(inst, marker.key);
    try { const value = await snapPosition(inst, marker.pos, 0); if (!valid()) return; if (Math.abs(value - marker.pos) > 1e-12) commitEditorMarker(inst,
        marker.key, value, `snap ${marker.label.toLowerCase()} to zero crossing`); setEditorMessage(`${marker.label} at the nearest zero crossing`); }
    catch (error) { if (valid()) setEditorMessage(error.message || 'Zero-crossing snap is unavailable.'); }
  }
  function setEditorView(inst, from, to) {
    const s = editorState(inst), minimum = Math.min(1, Math.max(24 / Math.max(1, sampleFrames(inst) || 1e6), 1e-6));
    let span = limit(to - from, minimum, 1); from = limit(from, 0, 1 - span);
    s.view = [from, from + span];
  }
  function zoomEditor(inst, factor, anchor = null) {
    const s = editorState(inst), [from, to] = s.view, span = to - from, focus = anchor ?? selectedMarker(inst).pos;
    const point = focus >= from && focus <= to ? focus : (from + to) / 2, ratio = (point - from) / span, next = span * factor;
    setEditorView(inst, point - ratio * next, point - ratio * next + next); drawEditor();
  }
  function revealPosition(inst, position) {
    const s = editorState(inst), [from, to] = s.view, span = to - from, margin = span * .04;
    if (position < from + margin) setEditorView(inst, position - margin, position - margin + span);
    else if (position > to - margin) setEditorView(inst, position + margin - span, position + margin);
  }
  function setEditorMessage(message) {
    const inst = current(); if (inst) editorState(inst).message = message;
    const output = document.getElementById('sampler-editor-message'); if (output) output.textContent = message;
  }
  // Waveform columns for the visible window: native min/max when available, else the 512-bin overview.
  function editorColumns(inst, s, width) {
    const key = `${inst.samplePath}|${s.view[0].toFixed(8)}|${s.view[1].toFixed(8)}|${width}`;
    if (s.peaks?.key === key) return s.peaks;
    requestPeaks(inst, s, key, width);
    const overview = sampleMeta(inst)?.peaks || [], min = new Array(width).fill(0), max = new Array(width).fill(0);
    if (overview.length) for (let column = 0; column < width; column++) {
      const position = s.view[0] + (column + .5) / width * (s.view[1] - s.view[0]),
        value = overview[Math.min(overview.length - 1, Math.floor(position * overview.length))];
      const peak = Array.isArray(value) ? Math.max(Math.abs(value[0]), Math.abs(value[1])) : Math.abs(Number(value) || 0);
      min[column] = -peak; max[column] = peak;
    }
    return { min, max, approximate: true };
  }
  function requestPeaks(inst, s, key, width) {
    if (!state().native || !inst.samplePath || s.pending === key) return;
    const context = destination(inst);
    s.pending = key; clearTimeout(s.peakTimer);
    s.peakTimer = setTimeout(async () => {
      try {
        if (!ownsDestination(context,true)) return;
        const result = await ST().request('sample.peaks',
            { ...nativeTarget(context), path: inst.samplePath, start: s.view[0], end: s.view[1], bins: width });
        if (s.pending !== key || !ownsDestination(context,true)) return;
        if (Array.isArray(result?.min) && Array.isArray(result?.max)) { s.peaks = { key, min: result.min, max: result.max }; drawEditor(); }
      } catch { /* Sample still loading: keep the overview until the next draw. */ }
      finally { if (s.pending === key) s.pending = null; }
    }, 30);
  }
  function drawEditor() {
    const canvas = document.getElementById('sampler-wave-canvas'), inst = current();
    if (!canvas?.getContext || !inst || inst.type !== 'sample') return;
    const s = editorState(inst), ratio = window.devicePixelRatio || 1;
    const width = Math.max(200, Math.round(canvas.clientWidth || 800)), height = Math.max(80, Math.round(canvas.clientHeight || 190));
    if (canvas.width !== width * ratio || canvas.height !== height * ratio) { canvas.width = width * ratio; canvas.height = height * ratio; }
    const g = canvas.getContext('2d'); g.setTransform(ratio, 0, 0, ratio, 0, 0);
    const [from, to] = s.view, span = to - from, x = position => (position - from) / span * width, wave = height - 14, middle = wave / 2;
    g.fillStyle = '#161913'; g.fillRect(0, 0, width, height);
    const start = inst.start, end = inst.end;
    g.fillStyle = '#463621'; g.globalAlpha = .28; g.fillRect(x(start), 0, x(end) - x(start), wave); g.globalAlpha = 1;
    if (LOOP_MODES.has(inst.mode)) {
      g.fillStyle = '#cfad67';
      g.globalAlpha = .13;
      g.fillRect(x(inst.loopStart), 0, x(inst.loopEnd) - x(inst.loopStart), wave);
      g.globalAlpha = 1;
    }
    g.strokeStyle = '#42432e'; g.lineWidth = 1; g.beginPath(); g.moveTo(0, middle + .5); g.lineTo(width, middle + .5); g.stroke();
    if (inst.samplePath) {
      const columns = editorColumns(inst, s, width);
      g.strokeStyle = columns.approximate ? '#8f8768' : '#c5bb94'; g.beginPath();
      for (let column = 0; column < width; column++) {
        const top = middle - limit(columns.max[column], -1, 1) * middle * .92, bottom = middle - limit(columns.min[column], -1, 1) * middle * .92;
        g.moveTo(column + .5, top); g.lineTo(column + .5, Math.max(bottom, top + 1));
      }
      g.stroke();
      // Dim audio outside the playback region.
      g.fillStyle = '#0b0c09';
      g.globalAlpha = .45;
      if (x(start) > 0) g.fillRect(0, 0, x(start), wave);
      if (x(end) < width) g.fillRect(x(end), 0, width - x(end), wave);
      g.globalAlpha = 1;
    } else { g.fillStyle = '#b0aa9d'; g.font = '12px -apple-system,sans-serif'; g.fillText('Load a sample to edit it', 12, middle); }
    const selected = selectedMarker(inst);
    g.font = '10px ui-monospace,Menlo,monospace';
    for (const marker of editorMarkers(inst)) {
      const at = Math.round(x(marker.pos)) + .5; if (at < -40 || at > width + 40) continue;
      const active = marker.key === selected.key,
        color = marker.key === 'start' || marker.key === 'end' ? '#e88b51' : marker.slice ? '#b9965a' : '#d0a867';
      g.strokeStyle = active ? '#fff1d2' : color; g.lineWidth = active ? 2 : 1; g.globalAlpha = marker.slice && !active ? .7 : 1;
      g.beginPath(); g.moveTo(at, 0); g.lineTo(at, wave); g.stroke(); g.globalAlpha = 1;
      const tag = marker.slice
          ? String(marker.slice + 1)
          : marker.key === 'start'
          ? 'S'
          : marker.key === 'end'
          ? 'E'
          : marker.key === 'loopStart'
          ? 'L'
          : 'L·';
      g.fillStyle = active ? '#fff1d2' : color;
      const tagWidth = g.measureText(tag).width + 6,
        left = marker.key === 'end' || marker.key === 'loopEnd' ? at - tagWidth : at;
      g.fillRect(left, marker.key.startsWith('loop') ? wave - 14 : 0, tagWidth, 13);
      g.fillStyle = '#161913';
      g.fillText(tag, left + 3, (marker.key.startsWith('loop') ? wave - 14 : 0) + 10);
    }
    // Overview strip: where the visible window sits in the whole sample.
    g.fillStyle = '#23231f';
    g.fillRect(0, wave + 4, width, 8);
    g.fillStyle = '#6d6249';
    g.fillRect(from * width, wave + 4, Math.max(2, span * width), 8);
    g.fillStyle = '#e88b51'; g.fillRect(start * width, wave + 4, 1, 8); g.fillRect(Math.min(width - 1, end * width), wave + 4, 1, 8);
    updateEditorReadout(inst);
    window.STSamplerPlayheads?.draw();
  }
  function updateEditorReadout(inst) {
    const s = editorState(inst), marker = selectedMarker(inst), canvas = document.getElementById('sampler-wave-canvas'), frames = sampleFrames(inst);
    const text = `${marker.label} ${timeText(inst, marker.pos)}${frames ? ` · frame ${Math.round(marker.pos * frames)}` : ''}`;
    canvas?.setAttribute?.('aria-valuenow', (marker.pos * 100).toFixed(3));
    canvas?.setAttribute?.('aria-valuetext', `${text}${s.snap ? ', zero-crossing snap on' : ''}`);
    document.querySelectorAll('[data-sampler-select-marker]').forEach(button => {
      const item = editorMarkers(inst).find(entry => entry.key === button.dataset.samplerSelectMarker); if (!item) return;
      button.setAttribute?.('aria-pressed', String(item.key === marker.key)); button.classList?.toggle?.('is-selected', item.key === marker.key);
      const value = button.querySelector?.('b'); if (value) value.textContent = timeText(inst, item.pos);
    });
    const view = document.getElementById('sampler-editor-view');
    if (view) view.textContent = `View ${timeText(inst, s.view[0])} – ${timeText(inst, s.view[1])} · ${(1 / (s.view[1] - s.view[0])).toFixed(1)}× · Region ${timeText(inst, inst.end - inst.start)}`;
    const snap = document.getElementById('sampler-editor-snap');
    if (snap) {
      snap.setAttribute?.('aria-pressed', String(!!s.snap));
      snap.textContent = s.snap ? 'Zero snap: on' : 'Zero snap: off';
    }
  }
  function selectEditorMarker(inst, key) { editorState(inst).marker = key; revealPosition(inst, selectedMarker(inst).pos); drawEditor(); }
  function cycleMarker(inst, delta) {
    const markers = editorMarkers(inst), index = markers.findIndex(item => item.key === selectedMarker(inst).key);
    selectEditorMarker(inst, markers[(index + delta + markers.length) % markers.length].key);
  }
  function validEditorAudition(job) {
    const s = state();
    return editorHeld === job && ownsDestination(job,true) &&
      s.track === job.track && s.project.tracks[job.track] === job.trackObject && s.view === 'instrument' && !s.modal &&
      auditionSignature(job.instrument) === job.signature && selectedMarker(job.instrument).key === job.marker;
  }
  function startEditorAudition(inst) {
    if (editorHeld || !inst.samplePath || window.STFreeze?.blockAudition?.(state().track)) return;
    const marker = selectedMarker(inst), root = Math.round(limit(inst.rootNote, 0, 127));
    stopAudition();
    const job = {...destination(inst),signature:auditionSignature(inst),marker:marker.key,slice:inst.mode === 'slice',sent:false};
    if (!job.slice) {
      // noteOn cancels earlier sampler previews itself; register this hold after it.
      ST().noteOn?.(root, 'sampler-editor', false); editorHeld = job;
      setEditorMessage(LOOP_MODES.has(inst.mode) ? 'Auditioning · release to stop' : 'Auditioning the region · release to stop');
      return;
    }
    editorHeld = job;
    setEditorMessage(job.slice ? `Preparing slice ${(marker.slice || 0) + 1}…` : 'Preparing region preview…');
    void (async () => {
      try {
          const result = await ST().syncEngineProject(() => validEditorAudition(job) ? window.STUtil.clone(state().project) : null);
        if (result === false || !validEditorAudition(job)) return;
        if (result == null) throw new Error('Could not synchronize the sample before auditioning.');
        if (window.STFreeze?.blockAudition?.(job.track)) return;
        job.sent = true;
        const response = await ST().request('step.preview',
            {...previewStep(job,root,{velocity:100,...(job.slice ? {fx1:{type:'slice',value:(marker.slice || 0) + 1}} : {})}),duration:2});
        if (!validEditorAudition(job)) return;
        if (response?.auditioning === false) throw new Error('Slice preview did not start.');
        setEditorMessage(job.slice
            ? `Auditioning slice ${(marker.slice || 0) + 1} · release to stop · up to 2 seconds`
            : 'Auditioning the region · release to stop · up to 2 seconds');
        job.timer = setTimeout(() => {
          if (editorHeld === job) {
            stopEditorAudition();
            setEditorMessage('Preview finished · Return or Space to hear again');
          }
        }, 2000);
      } catch (error) {
        if (editorHeld === job) {
          stopEditorAudition();
          setEditorMessage(error.message || 'Slice audition failed');
          ST().toast(error.message || 'Slice audition failed', 'error');
        }
      } finally {
        if (editorHeld === job && !job.timer) stopEditorAudition();
      }
    })();
  }
  function stopEditorAudition() {
    const job = editorHeld; if (!job) return false;
    editorHeld = null; clearTimeout(job.timer);
    if (job.slice) {
      if (job.sent) ST().request('step.preview',{...previewStep(job,-1),duration:.03}).catch(() => {});
    } else ST().noteOff?.('sampler-editor');
    return true;
  }
  async function editRegion(operation, extra = {}) {
    const s = state(), id = s.instrument, inst = current(), label = EDIT_LABELS[operation] || operation;
    if (!inst?.samplePath) { ST().toast('Load a sample first.'); return; }
    if (editorJob.current) { ST().toast('Wait for the current sample edit to finish.'); return; }
    const transaction = window.STInstrumentEdits;
    if (!transaction?.run) { ST().toast('The sample editing service is unavailable. Reopen the app before processing audio.',
        'error'); return; }
    const start = inst.start, end = inst.end, path = inst.samplePath, job = { operation }, origin = document.activeElement;
    const context = destination(inst), owns = () => ownsDestination(context);
    const visible = () => owns() && current() === inst && state().view === 'instrument' && !state().modal;
    editorJob.current = job; stopEditorAudition(); setEditorMessage(`${label}… writing a new audio file`); updateEditorBusy();
    let focusAfter = false, result;
    try {
      const apply = (p,value) => {
        const target = resolveInstrument(context,p); target.samplePath = value.path;
        if (operation === 'trim') {
          const width = Math.max(1e-9,end-start), map = value => limit((value-start)/width,0,1);
          target.loopStart = map(target.loopStart);
          target.loopEnd = Math.min(1, Math.max(map(target.loopEnd), target.loopStart + 1e-6));
          target.start = 0;
          target.end = 1;
        }
      };
      await transaction.run(id, [], async edit => {
        result = await edit.native('sample.edit',{path,instrument:id,operation,start,end,...extra});
        if (!result?.path) throw new Error('The audio engine did not return an edited sample.');
        focusAfter = visible() && document.activeElement === origin;
        await edit.commit(`${label.toLowerCase()} sample`,p=>apply(p,result));
      });
      if (!owns()) return;
      // Publishing can wait on the host. Keep a newer control focused even if
      // the original editor node was replaced by the accepted project repaint.
      focusAfter = focusAfter && (document.activeElement === origin || !!origin?.id && document.activeElement?.id === origin.id);
      if (operation === 'trim') editorState(inst).view = [0, 1];
      publishMetadata(context,result);
      if (visible()) { ST().refresh(); setEditorMessage(`${label} done · new file written, original untouched · ⌘Z restores the previous audio`); }
    } catch (error) {
      focusAfter = false;
      if (visible()) setEditorMessage(error.message || `${label} failed.`);
      if (result?.path) ST().toast(`${label} was not applied. The new file is at ${result.path}. ${error.message || ''}`, 'error');
    } finally {
      if (editorJob.current === job) editorJob.current = null;
      updateEditorBusy();
      if (focusAfter && visible()) document.getElementById('sampler-wave-canvas')?.focus?.({ preventScroll: true });
    }
  }
  function updateEditorBusy() {
    document.querySelectorAll('[data-sampler-edit-op]').forEach(button => {
      button.disabled = !!editorJob.current || !current()?.samplePath;
    });
  }
  function gainDialog() {
    const inst = current(); if (!inst?.samplePath) return;
    const s = editorState(inst), context = destination(inst);
    ST().textPrompt('Gain for the region · dB (−24 to +24)', String(s.gain), value => {
      if (!ownsDestination(context,true)) { ST().toast('The destination sample changed. Open the gain control again.','info'); return; }
      const decibels = Number(String(value).replace('−', '-'));
      if (!Number.isFinite(decibels)
          || decibels < -24
          || decibels > 24
          || decibels === 0) { ST().toast('Enter a non-zero gain between −24 and +24 dB.', 'error'); return; }
      s.gain = decibels; void editRegion('gain', { decibels });
    });
  }
  function addSliceAtMarker(inst) {
    const points = slicePoints(inst), marker = selectedMarker(inst), start = inst.start, end = inst.end;
    if (points.length >= 128) { ST().toast('An instrument holds up to 128 slices.'); return; }
    let value;
    if (marker.key === 'start' || marker.slice) { const index = marker.slice || 0; value = (points[index] + (points[index + 1] ?? 1)) / 2; }
    else if (marker.key === 'end') value = (points[points.length - 1] + 1) / 2;
    else value = (marker.pos - start) / (end - start);
    if (!(value > 0 && value < 1)
        || points.some(point => Math.abs(point - value) < 1e-6)) { ST().toast('Choose Start, a slice or a loop marker inside the region to split there.'); return; }
    const next = [...points, value].sort((a, b) => a - b);
    editorState(inst).marker = `slice:${next.indexOf(value)}`;
    saveMarkers(next, 'add slice marker', 'sampler-wave-canvas', inst);
    setEditorMessage(`Slice ${next.indexOf(value) + 1} added · ←→ to place it`);
  }
  function removeSelectedSlice(inst) {
    const marker = selectedMarker(inst); if (!marker.slice) return false;
    const points = slicePoints(inst); points.splice(marker.slice, 1);
    editorState(inst).marker = marker.slice > 1 ? `slice:${marker.slice - 1}` : 'start';
    saveMarkers(points, 'remove slice marker', 'sampler-wave-canvas', inst); setEditorMessage(`${marker.label} removed`);
    return true;
  }
  // One note per slice in a new pattern on the current track: slice starts land on their time in the region
  // at the project tempo when they fit, otherwise one slice per row.
  function slicesToPattern() {
    const s = state(), p = s.project, inst = current(), context = destination(inst), id = context.instrumentID, track = s.track ?? 0;
    if (!inst?.samplePath) { ST().toast('Load a sample first.'); return; }
    if (p.patterns.length >= 256) { ST().toast('This session supports up to 256 patterns.', 'error'); return; }
    if (p.tracks[track]?.instrument !== id) { ST().toast(`Assign this instrument to ${p.tracks[track]?.name || 'the track'} first, so the pattern plays it.`); return; }
    const points = slicePoints(inst), count = points.length, root = Math.round(limit(inst.rootNote, 0, 127));
    const regionSeconds = sampleSeconds(inst) * (inst.end - inst.start),
      rowSeconds = 60 / Number(p.bpm) / Number(p.linesPerBeat);
    let rows = points.map((point, index) => index);
    const timed = regionSeconds > 0 ? points.map(point => Math.round(point * regionSeconds / rowSeconds)) : null;
    if (timed && timed.every((row, index) => !index || row > timed[index - 1]) && regionSeconds / rowSeconds <= 256) rows = timed;
    const lastRow = rows === timed ? Math.ceil(regionSeconds / rowSeconds) : count;
    const length = Math.min(256, Math.max(16, Math.ceil(Math.max(lastRow, rows[count - 1] + 1) / 16) * 16));
    let created = null;
    ST().mutate('slices to pattern', project => {
      const patternID = Array.from({ length: 256 }, (_, index) => index).find(index => !project.patterns.some(item => item.id === index));
      const steps = Array.from({ length: project.tracks.length }, () => Array(length).fill(null));
      rows.forEach((row, index) => {
        if (row >= length) return;
        const next = index + 1 < count ? rows[index + 1] : Math.min(length, lastRow), gate = Math.max(.25, Math.min(256, next - row));
        const target = { instrument: id };
        steps[track][row] = root + index <= 127
            ? { note: root + index, ...target, velocity: 100, gate }
            : { note: root, ...target, velocity: 100, gate, fx1: { type: 'slice', value: index + 1 } };
      });
      if (inst.mode !== 'slice') { inst.mode = 'slice'; inst.slicePoints = [...points]; inst.sliceCount = count; }
      created = { id: patternID, name: `${inst.name || 'Slices'} slices`.slice(0, 60), length, steps, automation: [] };
      project.patterns.push(created); project.activePattern = patternID; s.row = 0;
    });
    if (created) ST().toast(`Created “${created.name}”: ${count} slice notes on ${p.tracks[track]?.name || 'the track'}${rows === timed ? ' at their original timing' : ', one per row'}. `
        + `Open Pattern view (⌘1) to play it.`);
  }
  function cyclePlayMode(inst, delta) {
    const index = Math.max(0, PLAY_MODES.findIndex(([mode]) => mode === inst.mode)),
      next = PLAY_MODES[(index + delta + PLAY_MODES.length) % PLAY_MODES.length];
    ST().mutate('play mode', () => { inst.mode = next[0]; }); setEditorMessage(`Play mode: ${next[1]} · ${MODE_HELP[next[0]]}`);
    document.getElementById('sampler-wave-canvas')?.focus?.({ preventScroll: true });
  }
  function editorKey(event) {
    const target = event.target;
    if (target?.id !== 'sampler-wave-canvas' || event.metaKey || event.ctrlKey || event.isComposing) return false;
    const inst = current(); if (!inst || inst.type !== 'sample') return false;
    const s = editorState(inst), key = event.key, letter = key.length === 1 ? key.toLowerCase() : '';
    const done = () => { event.preventDefault(); event.stopPropagation?.(); return true; };
    if (key === 'ArrowLeft' || key === 'ArrowRight') { void nudgeMarker(inst, key === 'ArrowRight' ? 1 : -1,
        event.altKey ? 'frame' : event.shiftKey ? 'coarse' : 'fine'); return done(); }
    if (event.altKey) return false;
    if (key === 'ArrowUp' || key === 'ArrowDown') { cycleMarker(inst, key === 'ArrowDown' ? 1 : -1); return done(); }
    if (key === 'Home' || key === 'End') { const marker = selectedMarker(inst), [lo, hi] = markerBounds(inst, marker.key); commitEditorMarker(inst,
        marker.key, key === 'Home' ? lo : hi, `move ${marker.label.toLowerCase()}`); return done(); }
    if (key === '+' || key === '=') { zoomEditor(inst, .5); return done(); }
    if (key === '-' || key === '_') { zoomEditor(inst, 2); return done(); }
    if (key === '0') { setEditorView(inst, 0, 1); drawEditor(); return done(); }
    // , and . are note keys too while the computer keyboard (⌘P) is on.
    if ((key === ',' || key === '.') && !state().keyboardPlay) { const [from, to] = s.view, shift = (to - from) * .25 * (key === '.' ? 1 : -1); setEditorView(inst,
        from + shift, to + shift); drawEditor(); return done(); }
    if (key === ' ' || key === 'Enter') { if (!event.repeat) startEditorAudition(inst); return done(); }
    if (key === 'Escape' && stopEditorAudition()) { setEditorMessage('Audition stopped'); return done(); }
    if ((key === 'Delete' || key === 'Backspace') && removeSelectedSlice(inst)) return done();
    if (!letter || event.repeat || state().keyboardPlay) return false;  // Letters play notes while the computer keyboard (⌘P) is on.
    if (letter === 'f') { const pad = (inst.end - inst.start) * .03; setEditorView(inst, inst.start - pad,
        inst.end + pad); drawEditor(); return done(); }
    if (letter === 'z') { if (event.shiftKey) void snapSelectedMarker(inst); else { s.snap = !s.snap; setEditorMessage(s.snap
        ? 'Zero-crossing snap on · nudges land on zero crossings'
        : 'Zero-crossing snap off'); drawEditor(); } return done(); }
    const edits = { t: 'trim', i: 'fadeIn', o: 'fadeOut', n: 'normalize', r: 'reverse' };
    if (edits[letter] && !event.shiftKey) { void editRegion(edits[letter],
        letter === 'i' || letter === 'o' ? { curve: s.curve } : {}); return done(); }
    if (letter === 'g') { gainDialog(); return done(); }
    if (letter === 'a') { addSliceAtMarker(inst); return done(); }
    if (letter === 'p') { slicesToPattern(); return done(); }
    if (letter === 'm') { cyclePlayMode(inst, event.shiftKey ? -1 : 1); return done(); }
    return false;
  }
  function editorPosition(event) {
    const canvas = document.getElementById('sampler-wave-canvas'), rect = canvas.getBoundingClientRect(), s = editorState(current());
    return { position: s.view[0] + limit((event.clientX - rect.left) / Math.max(1, rect.width), 0, 1) * (s.view[1] - s.view[0]), width: rect.width };
  }
  function bindEditor() {
    const canvas = document.getElementById('sampler-wave-canvas'), inst = current();
    updateEditorBusy();
    document.querySelectorAll('[data-sampler-mode]').forEach(button => button.addEventListener('click', () => {
      const mode = button.dataset.samplerMode; if (current() !== inst || inst.mode === mode) return;
      ST().mutate('play mode', () => {
        inst.mode = mode;
      });
      setEditorMessage(`Play mode: ${PLAY_MODES.find(([value]) => value === mode)?.[1]} · ${MODE_HELP[mode]}`);
      document.getElementById(`sampler-mode-${mode}`)?.focus?.({ preventScroll: true });
    }));
    document.querySelectorAll('[data-sampler-mode]').forEach(button => button.addEventListener('keydown', event => {
      if (!['ArrowLeft', 'ArrowRight'].includes(event.key) || event.altKey || event.metaKey || event.ctrlKey) return;
      event.preventDefault(); event.stopPropagation(); cyclePlayMode(inst, event.key === 'ArrowRight' ? 1 : -1);
      document.getElementById(`sampler-mode-${current()?.mode}`)?.focus?.({ preventScroll: true });
    }));
    document.querySelectorAll('[data-sampler-select-marker]').forEach(button => button.addEventListener('click', () => {
      selectEditorMarker(inst, button.dataset.samplerSelectMarker);
      canvas?.focus?.({ preventScroll: true });
    }));
    document.querySelectorAll('[data-sampler-edit-op]').forEach(button => button.addEventListener('click', () => {
      const operation = button.dataset.samplerEditOp;
      if (operation === 'gain') {
        gainDialog();
      } else {
        void editRegion(operation, operation === 'fadeIn' || operation === 'fadeOut' ? { curve: editorState(inst).curve } : {});
      }
    }));
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    on('sampler-fade-curve', 'change', event => {
      editorState(inst).curve = FADE_CURVES.some(([value]) => value === event.target.value) ? event.target.value : 'linear';
    });
    on('sampler-use-wavetable', 'click', () => {
      if (current() !== inst) return;
      window.STWavetables?.chooseTable(inst, window.STWavetables.pathFor('Saw'));
      setEditorMessage('Wavetable mode on the Saw table · move Position to morph');
    });
    on('sampler-editor-zoom-in', 'click', () => zoomEditor(inst, .5));
    on('sampler-editor-zoom-out', 'click', () => zoomEditor(inst, 2));
    on('sampler-editor-zoom-fit', 'click', () => { setEditorView(inst, 0, 1); drawEditor(); });
    on('sampler-editor-snap', 'click', () => { const s = editorState(inst); s.snap = !s.snap; drawEditor(); });
    on('sampler-editor-add-slice', 'click', () => addSliceAtMarker(inst));
    on('sampler-editor-pattern', 'click', slicesToPattern);
    on('sampler-editor-audition', 'pointerdown', event => { event.preventDefault?.(); startEditorAudition(inst); });
    for (const type of ['pointerup', 'pointerleave', 'pointercancel']) on('sampler-editor-audition', type, () => { stopEditorAudition(); });
    on('sampler-editor-audition', 'keydown', event => {
      if ((event.key === ' ' || event.key === 'Enter') && !event.repeat) {
        event.preventDefault();
        event.stopPropagation();
        startEditorAudition(inst);
      }
    });
    on('sampler-editor-audition', 'keyup', event => {
      if (event.key === ' ' || event.key === 'Enter') {
        event.preventDefault();
        stopEditorAudition();
      }
    });
    on('sampler-editor-audition', 'blur', () => { stopEditorAudition(); });
    if (canvas?.getContext) {
      canvas.addEventListener('keyup', event => {
        if ((event.key === ' ' || event.key === 'Enter') && stopEditorAudition()) {
          event.preventDefault();
          setEditorMessage('');
        }
      });
      canvas.addEventListener('blur', () => { stopEditorAudition(); });
      canvas.addEventListener('pointerdown', event => {
        if (event.button !== 0) return;
        canvas.focus({ preventScroll: true });
        const { position, width } = editorPosition(event), s = editorState(inst), span = s.view[1] - s.view[0];
        if (event.shiftKey || event.altKey) {
          // ⇧-click places End; ⌥-click Loop start; ⌥⇧-click Loop end (then drag to refine).
          const key = event.altKey ? (event.shiftKey ? 'loopEnd' : 'loopStart') : 'end';
          if (key !== 'end'
              && !LOOP_MODES.has(inst.mode)) { setEditorMessage('Loop markers belong to the loop play modes · M changes play mode'); return; }
          const label = `move ${editorMarkers(inst).find(item => item.key === key)?.label.toLowerCase() || key}`;
          s.marker = key; commitEditorMarker(inst, key, position, label);
          editorDrag = { inst, key, label }; canvas.setPointerCapture?.(event.pointerId); return;
        }
        let nearest = null, distance = Infinity;
        for (const marker of editorMarkers(inst)) {
          const pixels = Math.abs(marker.pos - position) / span * width;
          if (pixels < distance) {
            distance = pixels;
            nearest = marker;
          }
        }
        if (!nearest) return;
        s.marker = nearest.key;
        if (distance <= 8) {
          editorDrag = { inst, key: nearest.key, label: `move ${nearest.label.toLowerCase()}` };
          canvas.setPointerCapture?.(event.pointerId);
        }
        drawEditor();
      });
      canvas.addEventListener('pointermove', event => {
        // A repaint during the drag replaces the canvas that captured the pointer; a release outside it is then never seen.
        if (editorDrag && event.buttons === 0) { editorDrag = null; scheduleEditorRefresh(); return; }
        if (editorDrag && editorDrag.inst === current()) commitEditorMarker(editorDrag.inst, editorDrag.key, editorPosition(event).position,
            editorDrag.label);
      });
      for (const type of ['pointerup', 'pointercancel']) canvas.addEventListener(type, () => {
        if (editorDrag) {
          editorDrag = null;
          scheduleEditorRefresh();
        }
      });
      canvas.addEventListener('wheel', event => {
        const s = editorState(inst);
        if (event.ctrlKey || event.metaKey) { event.preventDefault(); zoomEditor(inst, Math.exp(event.deltaY * .01),
            editorPosition(event).position); return; }
        const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.shiftKey ? event.deltaY : 0;
        if (!delta || s.view[1] - s.view[0] >= 1) return;
        event.preventDefault();
        const span = s.view[1] - s.view[0],
          shift = delta / Math.max(1, canvas.clientWidth) * span;
        setEditorView(inst, s.view[0] + shift, s.view[1] + shift);
        drawEditor();
      }, { passive: false });
      if (!editorGlobals) {
        editorGlobals = true;
        window.addEventListener?.('resize', () => drawEditor());
        window.STContextMenu?.register?.(contextItems, 75);
      }
      drawEditor();
    }
  }
  function contextItems({ target }) {
    if (!target?.closest?.('#sampler-editor')) return null;
    const inst = current(); if (!inst || inst.type !== 'sample') return null;
    const s = editorState(inst), ready = !!inst.samplePath && !editorJob.current, marker = selectedMarker(inst), curve = { curve: s.curve };
    return [
      { header: true, title: `Sample editor · ${marker.label}` },
      { title: 'Trim to region', key: 'T', enabled: ready, action: () => editRegion('trim') },
      { title: 'Fade in', key: 'I', enabled: ready, action: () => editRegion('fadeIn', curve) },
      { title: 'Fade out', key: 'O', enabled: ready, action: () => editRegion('fadeOut', curve) },
      { title: 'Normalize region', key: 'N', enabled: ready, action: () => editRegion('normalize') },
      { title: 'Reverse region', key: 'R', enabled: ready, action: () => editRegion('reverse') },
      { title: 'Gain…', key: 'G', enabled: ready, action: gainDialog },
      { title: 'Remove DC offset', enabled: ready, action: () => editRegion('removeDC') },
      { title: 'Silence region', enabled: ready, action: () => editRegion('silence') },
      { separator: true },
      { title: 'Snap marker to zero crossing', key: '⇧Z', enabled: !!inst.samplePath, action: () => snapSelectedMarker(inst) },
      { title: 'Zero-crossing snap', key: 'Z', checked: !!s.snap, action: () => { s.snap = !s.snap; drawEditor(); } },
      { title: 'Zoom to region', key: 'F', action: () => { setEditorView(inst, inst.start, inst.end); drawEditor(); } },
      { title: 'Show whole sample', key: '0', action: () => { setEditorView(inst, 0, 1); drawEditor(); } },
      { separator: true },
      { title: 'Add slice here', key: 'A', enabled: !!inst.samplePath, action: () => addSliceAtMarker(inst) },
      { title: 'Remove slice', key: '⌫', enabled: !!marker.slice, action: () => removeSelectedSlice(inst) },
      { title: 'Slices to new pattern', key: 'P', enabled: !!inst.samplePath, action: slicesToPattern },
      { title: 'Play mode', items: PLAY_MODES.map(([mode, label]) => ({ title: label, checked: inst.mode === mode, action: () => ST().mutate('play mode',
          () => { inst.mode = mode; }) })) }
    ];
  }
  function renderEditor(inst) {
    const s = editorState(inst),
      seconds = sampleSeconds(inst),
      frames = sampleFrames(inst),
      marker = selectedMarker(inst),
      ready = !!inst.samplePath && !editorJob.current;
    const mode = PLAY_MODES.some(([value]) => value === inst.mode) ? inst.mode : 'oneshot';
    // Wavetable mode: frames, Position, table and frame size live in the Wavetable panel above (wavetables.js).
    const frameSize = Number(inst.wavetableFrameSize) > 0 ? Number(inst.wavetableFrameSize) : Number(sampleMeta(inst)?.wavetableFrameSize) || 2048,
      cycles = Math.floor(frames * (inst.end - inst.start) / frameSize);
    const modeParams = mode === 'wavetable' ? `<div class="sampler-mode-params"><p class="sampler-help">${frames ? `${Math.max(1, cycles)} frame${cycles === 1 ? '' : 's'}`
        + ` of ${frameSize} samples in the region. ` : ''}Pitch follows the played note; Position (Wavetable panel, automation, `
        + `LFO → Position or the POS step effect) morphs between adjacent frames.</p></div>` : '';
    const builtin = !inst.samplePath
        ? '<button id="sampler-use-wavetable" title="Make this sampler a wavetable synth on the built-in Saw table">Use a built-in wavetable</button>'
        : '';
    const markers = editorMarkers(inst).filter(item => !item.slice || item.key === marker.key || editorMarkers(inst).length <= 12);
    const button = (op, label, keyName) => `<button data-sampler-edit-op="${op}" id="sampler-edit-${op}" ${ready ? '' : 'disabled'} title="${esc(label)} · ${keyName}">${esc(label)}`
        + ` <kbd>${keyName}</kbd></button>`;
    const meta = sampleMeta(inst),
      facts = frames
          ? [meta?.channels ? `${meta.channels} CH` : '', meta?.sampleRate ? `${(meta.sampleRate / 1000).toFixed(1)} KHZ` : '',
              `${seconds.toFixed(3)} S`, `${frames} FRAMES`].filter(Boolean).join(' · ')
          : inst.samplePath
          ? 'LOADING…'
          : 'NO SAMPLE';
    return `<section id="sampler-editor" class="panel wide sampler-editor" aria-label="Sample editor"><div class="panel-head">`
        + `<div class="sampler-editor-title"><h2>Sample</h2><span class="sampler-file" title="${esc(inst.samplePath)}">${esc(inst.samplePath ? window.STWavetables?.displayName(inst.samplePath) || fileName(inst.samplePath) : 'Load or record a sample to begin')}</span>`
        + `<span class="chip">${facts}</span></div><div class="sampler-editor-head-actions">${builtin}${inst.samplePath && !window.STWavetables?.parse(inst.samplePath) ? '<button id="sample-reveal" class="quiet">Show in Finder</button>' : ''}<button `
        + `id="sampler-editor-audition" ${inst.samplePath ? '' : 'disabled'} title="Hold to audition · Space on the waveform">▶ Hold to audition</button>`
        + `</div></div><div class="panel-body stack">
      <div class="sampler-modes" role="radiogroup" aria-label="Play mode">${PLAY_MODES.map(([value, label]) => `<button id="sampler-mode-${value}" data-sampler-mode="${value}" role="radio" aria-checked="${mode === value}" tabindex="${mode === value ? 0 : -1}"`
          + ` class="${mode === value ? 'is-selected' : ''}">${esc(label)}</button>`).join('')}<span `
          + `class="sampler-help sampler-mode-help">${esc(MODE_HELP[mode])}</span></div>${modeParams}
      <div class="sampler-editor-strip"><div class="sampler-editor-markers" role="group" aria-label="Markers">${markers.map(item => `<button data-sampler-select-marker="${item.key}" id="sampler-select-${item.key.replace(':', '-')}" aria-pressed="${item.key === marker.key}"`
          + ` class="${item.key === marker.key ? 'is-selected' : ''}" tabindex="-1">${esc(item.label)} <b>${esc(timeText(inst, item.pos))}</b>`
          + `</button>`).join('')}</div>`
          + `<span id="sampler-editor-view" class="sampler-help"></span></div>
      <div class="sampler-wave-wrap"><canvas id="sampler-wave-canvas" class="sampler-wave" tabindex="0" role="slider" aria-label="Sample waveform and markers"`
          + ` aria-describedby="sampler-editor-keys" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${(marker.pos * 100).toFixed(3)}"`
          + ` aria-valuetext="${esc(`${marker.label} ${timeText(inst, marker.pos)}`)}" title="Drag a marker · ⇧-click sets End · ⌥-click Loop start `
          + `· ⌥⇧-click Loop end · ⌘-scroll zooms"></canvas><canvas id="sampler-playhead-canvas" class="sampler-playheads" aria-hidden="true"></canvas></div>
      <div class="sampler-editor-row"><div class="sampler-editor-tools" role="group" aria-label="Region edits">${button('trim', 'Trim', 'T')}${button('fadeIn', 'Fade in', 'I')}${button('fadeOut', 'Fade out', 'O')}<label `
          + `class="sampler-fade-curve"><select id="sampler-fade-curve" aria-label="Fade shape">${FADE_CURVES.map(([value, label]) => `<option value="${value}" ${s.curve === value ? 'selected' : ''}>${label}</option>`).join('')}</select>`
          + `</label>${button('normalize', 'Normalize', 'N')}${button('reverse', 'Reverse', 'R')}${button('gain', 'Gain…', 'G')}<button id="sampler-stretch"`
          + ` ${!inst.samplePath ? 'disabled' : ''} title="Time stretch · new file, pitch preserved">Stretch…</button>`
          + `<span class="sampler-tool-gap" aria-hidden="true"></span><button id="sampler-editor-add-slice" ${inst.samplePath ? '' : 'disabled'}>Add `
          + `slice <kbd>A</kbd></button><button id="sampler-editor-pattern" ${inst.samplePath ? '' : 'disabled'}>Slices → pattern <kbd>P</kbd>`
          + `</button></div><span class="grow"></span><div class="sampler-editor-bar" role="group" aria-label="Waveform view">`
          + `<button id="sampler-editor-zoom-out" title="Zoom out · −" aria-label="Zoom out">−</button><button id="sampler-editor-zoom-fit"`
          + ` title="Whole sample · 0">Fit</button><button id="sampler-editor-zoom-in" title="Zoom in · +" aria-label="Zoom in">+</button>`
          + `<button id="sampler-editor-snap" aria-pressed="${!!s.snap}" title="Zero-crossing snap · Z">${s.snap ? 'Zero snap: on' : 'Zero snap: off'}</button></div></div>
      <div class="sampler-editor-foot"><p id="sampler-editor-message" class="sampler-help sampler-editor-message" role="status"`
          + ` aria-live="polite">${esc(s.message
              || (inst.samplePath ? 'Edits never change your library files; ⌘Z restores the previous audio.' : 'Load a sample to edit it.'))}</p>`
          + `<details id="sampler-editor-help" class="sampler-more"><summary>Waveform keys</summary><p id="sampler-editor-keys" class="sampler-help">Focus `
          + `the waveform: ↑↓ choose marker · ←→ nudge (⇧ coarse, ⌥ one frame) · Home/End limits · + − zoom · 0 whole · F region · , . scroll · Z zero `
          + `snap (⇧Z snap now) · Space hold to audition · M play mode. Edits apply to the Start–End region and write a new file: T trim · I/O fades `
          + `· N normalize · R reverse · G gain. A add slice · ⌫ remove slice · P slices → new pattern. Mouse: drag a marker; ⇧-click sets End, ⌥-click `
          + `Loop start, ⌥⇧-click Loop end.</p></details></div>
    </div></section>`;
  }
  // Palette and menu entry points for the sample editor actions (same operations as its keys and context menu).
  function command(name) {
    const inst = current();
    if (!inst || inst.type !== 'sample') { ST().toast('Choose a sampler instrument first.'); return false; }
    // Show the page an action belongs to, so its result is visible.
    window.STInstrumentPages?.show?.(name === 'detectTransients' ? 'slices' : 'sample', { focus: false });
    if (name === 'editor') { const canvas = document.getElementById('sampler-wave-canvas'); if (!canvas) { ST().toast('Load or record a sample to open the editor.'); return false; } canvas.focus({ preventScroll: true }); canvas.scrollIntoView?.({ block: 'nearest' }); return true; }
    if (!inst.samplePath) { ST().toast('Load or record a sample first.'); return false; }
    switch (name) {
      case 'trim': case 'normalize': case 'reverse': case 'removeDC': case 'silence': return editRegion(name);
      case 'fadeIn': case 'fadeOut': return editRegion(name, { curve: editorState(inst).curve });
      case 'gain': return gainDialog();
      case 'addSlice': return addSliceAtMarker(inst);
      case 'slicesToPattern': return slicesToPattern();
      case 'detectTransients': return detectTransients();
    }
    return false;
  }
  // Hardware controllers (mk3.js) drive the same editor state the keyboard edits.
  const controller = {
    supportsOwner: true,
    info(columns = 0) {
      const inst = current(); if (!inst || inst.type !== 'sample') return null;
      const s = editorState(inst), marker = selectedMarker(inst);
      return { name: inst.name, path: inst.samplePath, mode: inst.mode, modeLabel: (PLAY_MODES.find(([mode]) => mode === inst.mode) || PLAY_MODES[0])[1], rootNote: inst.rootNote,
        start: inst.start,
        end: inst.end,
        loopStart: inst.loopStart,
        loopEnd: inst.loopEnd,
        loop: LOOP_MODES.has(inst.mode),
        seconds: sampleSeconds(inst),
        frames: sampleFrames(inst),
        markers: editorMarkers(inst),
        marker: marker.key,
        markerLabel: marker.label,
        position: marker.pos,
        time: timeText(inst, marker.pos),
        view: s.view.slice(),
        snap: !!s.snap,
        message: s.message || '',
        slices: slicePoints(inst).length, busy: !!editorJob.current, auditioning: !!editorHeld, columns: columns
            ? editorColumns(inst, s, columns)
            : null };
    },
    selectMarker(delta) { const inst = current(); if (inst?.type === 'sample') cycleMarker(inst, delta); },
    nudge(direction, size = 'fine') { const inst = current(); if (inst?.type === 'sample') return nudgeMarker(inst, direction, size); },
    moveTo(position) {
      const inst = current();
      if (inst?.type !== 'sample') return;
      const marker = selectedMarker(inst);
      commitEditorMarker(inst, marker.key, position, `move ${marker.label.toLowerCase()}`);
    },
    zoom(factor, options = {}) {
      const inst = current(); if (inst?.type !== 'sample') return;
      if (!options.center) { zoomEditor(inst, factor); return; }
      // Controller zoom keeps the selected marker in the middle of the view.
      const s = editorState(inst), span = (s.view[1] - s.view[0]) * factor, focus = selectedMarker(inst).pos;
      setEditorView(inst, focus - span / 2, focus + span / 2); drawEditor();
    },
    scroll(amount) {
      const inst = current();
      if (inst?.type !== 'sample') return;
      const [from, to] = editorState(inst).view,
        shift = (to - from) * amount;
      setEditorView(inst, from + shift, to + shift);
      drawEditor();
    },
    showAll() { const inst = current(); if (inst?.type === 'sample') { setEditorView(inst, 0, 1); drawEditor(); } },
    showRegion() {
      const inst = current();
      if (inst?.type !== 'sample') return;
      const pad = (inst.end - inst.start) * .03;
      setEditorView(inst, inst.start - pad, inst.end + pad);
      drawEditor();
    },
    toggleSnap() {
      const inst = current();
      if (inst?.type !== 'sample') return;
      const s = editorState(inst);
      s.snap = !s.snap;
      setEditorMessage(s.snap ? 'Zero-crossing snap on' : 'Zero-crossing snap off');
      drawEditor();
    },
    snapNow() { const inst = current(); if (inst?.type === 'sample') return snapSelectedMarker(inst); },
    cycleMode(delta) { const inst = current(); if (inst?.type === 'sample') cyclePlayMode(inst, delta); },
    removeSlice() { const inst = current(); return inst?.type === 'sample' ? removeSelectedSlice(inst) : false; },
    audition(on) { const inst = current(); if (inst?.type !== 'sample') return; if (on) startEditorAudition(inst); else stopEditorAudition(); }
  };
  /** The waveform's visible window (0..1 of the sample) for overlays such as the live playheads. */
  const view = inst => editorState(inst).view.slice();
  window.STSampler = { render, parts, bind, stretchDialog, key, stopAudition, contextItems, command, controller, redraw: () => drawEditor(), view };
})();
