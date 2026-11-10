/* Offline rendering with explicit stem boundaries and guarded native completion. */
(() => {
  'use strict';
  let current = null, external = null;
  const S = () => window.ST;
  const get = id => document.getElementById(id);
  const esc = value => S().escapeHTML(String(value ?? ''));
  const { clone } = window.STUtil;
  const encodingNames = Object.freeze({pcm16:'PCM 16-bit',pcm24:'PCM 24-bit',float32:'32-bit float'});
  const defaultSampleRate = 48000;
  // Host default for stems without explicit owners: one stem per track (AudioHost.renderRoutingExport).
  const defaultStemCount = project => project.tracks.length;
  const knownSampleRates = [44100,48000,96000];
  const rateName = value => `${value/1000} kHz`;
  const active = session => current === session && S().state.modal === 'export';
  const sameProject = session => S().state.project === session.project;
  const pendingPreparation = () => window.STBusy.isBusy('session')
      ? 'Wait for the current save or project change to finish.'
      : window.STBusy.isBusy('instrumentEdits')
      ? 'Wait for the instrument edit to finish, then export again.'
      : window.STBusy.isBusy('recording')
      ? 'Wait for recording and input restoration to finish, then export again.'
      : window.STBusy.isBusy('pluginEdit')
      ? 'Wait for the plug-in edit to finish, then export again.'
      : window.STBusy.any(['effectMovePrep', 'routingPrep'])
      ? 'Wait for the routing change to finish preparing, then export again.'
      : '';
  const kindName = target => target.owner.startsWith('return:') ? 'Built-in return' : {
    track: 'Track',
    group: 'Group',
    return: 'Return',
    master: 'Master'
  }[target.kind];
  const selectedTargets = session => session.targets.filter(target => target.available && session.selected.has(target.owner));
  const visibleTargets = session => {
    const query = session.query.trim().toLowerCase();
    return session.targets.filter(target => `${target.name} ${kindName(target)}`.toLowerCase().includes(query));
  };
  function status(session, message, kind = '') {
    session.message = message; session.messageKind = kind;
    if (!active(session)) return;
    const node = get('export-progress'); if (node) { node.textContent = message; node.classList.toggle('is-error', kind === 'error'); }
  }
  function controls(session) {
    if (!active(session)) return;
    const locked = session.busy || session.loading || session.waiting;
    // Range and tail do not depend on asynchronous channel capabilities. Keep
    // them usable during discovery so the opening keyboard focus has a field.
    for (const id of ['export-mode','export-tail']) if (get(id)) get(id).disabled = session.busy || session.waiting;
    for (const id of ['export-search','export-select-visible','export-clear']) if (get(id)) get(id).disabled = locked;
    get('export-encoding').disabled = locked || !session.encodingSupported;
    get('export-sample-rate').disabled = locked || !session.sampleRateSupported;
    for (const input of document.querySelectorAll('[data-export-owner]')) input.disabled = locked
        || !session.targets.find(target => target.owner === input.dataset.exportOwner)?.available;
    get('export-stems').hidden = session.mode !== 'stems';
    get('export-stem-picker').hidden = !session.fullGraph;
    get('export-legacy-stems').hidden = session.fullGraph;
    get('export-start').disabled = locked || !S().state.native || (session.mode === 'stems' && session.fullGraph && !selectedTargets(session).length);
    get('export-start').textContent = session.waiting
        ? 'Waiting for routing…'
        : session.busy
        ? session.phase === 'choosing' ? 'Choosing destination…' : 'Exporting…'
        : session.result
        ? 'Export again…'
        : 'Choose destination & export…';
    get('export-cancel').disabled = session.busy && session.phase !== 'rendering';
    get('export-cancel').textContent = session.phase === 'canceling' ? 'Canceling…' : session.phase === 'rendering' ? 'Cancel render' : 'Close';
    if (get('modal-close')) get('modal-close').disabled = session.busy;
    const meter = get('export-meter'); if (meter) meter.hidden = !['rendering','canceling'].includes(session.phase);
    const chosen = selectedTargets(session).length, shown = visibleTargets(session).length;
    get('export-selected-count').textContent = `${chosen} selected${session.query.trim() ? ` · ${shown} shown` : ''}`;
    get('export-boundary').textContent = session.fullGraph
      ? 'Track, group and return stems are taken after their faders, before their output buses. The complete graph keeps running, including sidechains. '
          + 'Parent and child stems overlap; combining them can double-count audio. Master contains the complete mix.'
      : `All ${session.project.tracks.length} tracks will be exported into separate files.`;
    get('export-legacy-stems').textContent = `${session.project.tracks.length} track stems · all tracks`;
    get('export-result').hidden = !session.path;
    get('export-result-path').value = session.result?.path || session.path || '';
  }
  function renderTargets(session) {
    if (!active(session)) return;
    const list = get('export-targets'), targets = visibleTargets(session);
    const focused = document.activeElement?.dataset?.exportOwner;
    list.innerHTML = targets.length ? targets.map(target => {
      const index = session.targets.indexOf(target), id = `export-stem-${index}`;
      return `<label class="export-target ${target.available ? '' : 'is-unavailable'}" for="${id}"><input type="checkbox" id="${id}" data-export-owner="${esc(target.owner)}"`
          + ` ${session.selected.has(target.owner) ? 'checked' : ''} ${!target.available || session.busy || session.loading ? 'disabled' : ''}>`
          + `<span class="export-target-name">${esc(target.name)}</span><span class="export-target-kind">${esc(kindName(target))}${target.available ? '' : ' · unavailable'}</span></label>`;
    }).join('') : '<p class="export-empty">No matching stem channels.</p>';
    list.querySelectorAll('[data-export-owner]').forEach(input => {
      input.addEventListener('change', () => {
        if (session.busy || session.loading || !active(session)) return;
        if (input.checked) session.selected.add(input.dataset.exportOwner); else session.selected.delete(input.dataset.exportOwner);
        controls(session);
      });
      input.addEventListener('keydown', event => {
        if (event.altKey || event.metaKey || event.ctrlKey || event.shiftKey || !['ArrowUp','ArrowDown','Home','End'].includes(event.key)) return;
        const choices = [...list.querySelectorAll('[data-export-owner]')].filter(item => !item.disabled), index = choices.indexOf(input);
        if (index < 0) return;
        const next = event.key === 'Home'
            ? 0
            : event.key === 'End'
            ? choices.length - 1
            : Math.max(0, Math.min(choices.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)));
        event.preventDefault(); event.stopPropagation(); choices[next]?.focus(); choices[next]?.scrollIntoView?.({block:'nearest'});
        // Space remains the checkbox's native toggle; arrows move focus only.
      });
    });
    if (focused) [...list.querySelectorAll('[data-export-owner]')].find(input => input.dataset.exportOwner === focused && !input.disabled)?.focus({
      preventScroll: true
    });
    controls(session);
  }
  function parseTargets(result) {
    const used = new Set();
    return (Array.isArray(result?.exportTargets) ? result.exportTargets : []).filter(target => {
      if (!target
          || typeof target.owner !== 'string'
          || !target.owner
          || typeof target.name !== 'string'
          || !target.name
          || !['track','group','return','master'].includes(target.kind)) return false;
      if (target.kind === 'track' && !/^track:.+/.test(target.owner)
          || target.kind === 'group' && !/^bus:.+/.test(target.owner)
          || target.kind === 'return' && !/^(?:bus|return):.+/.test(target.owner)) return false;
      if (!['postFaderPreParent','masterOutput'].includes(target.boundary)
          || target.kind === 'master' && (target.owner !== 'master' || target.boundary !== 'masterOutput')
          || target.kind !== 'master' && target.boundary !== 'postFaderPreParent'
          || used.has(target.owner)) return false;
      if (target.owner.startsWith('return:') && !['return:delay','return:reverb'].includes(target.owner)) return false;
      used.add(target.owner); return true;
    }).map(target => ({owner:target.owner,name:target.name,kind:target.kind,boundary:target.boundary,available:target.available === true}));
  }
  function configureEncoding(session, capabilities) {
    const format = capabilities?.audioExport;
    // A legacy host must never be offered an encoding it will silently ignore.
    if (format?.container === 'wav'
        && format.sampleRate === defaultSampleRate
        && format.defaultEncoding === 'pcm24'
        && Array.isArray(format.encodings)
        && format.encodings.includes('pcm24')) {
      session.encodings = Object.keys(encodingNames).filter(encoding => format.encodings.includes(encoding));
      session.encodingSupported = true;
    }
    if (session.encodingSupported
        && format.defaultSampleRate === defaultSampleRate
        && Array.isArray(format.sampleRates)
        && format.sampleRates.includes(defaultSampleRate)) {
      session.sampleRates = knownSampleRates.filter(rate => format.sampleRates.includes(rate));
      session.sampleRateSupported = session.sampleRates.length > 1;
    }
    const encodingDraft = get('export-encoding').value, rateDraft = get('export-sample-rate').value;
    const selectedEncoding = session.encodings.includes(encodingDraft) ? encodingDraft : 'pcm24';
    const selectedRate = session.sampleRates.find(rate => String(rate) === rateDraft) ?? defaultSampleRate;
    get('export-encoding').innerHTML = session.encodings.map(encoding => `<option value="${encoding}" ${encoding === selectedEncoding ? 'selected' : ''}>${encodingNames[encoding]}${encoding === 'pcm24' ? ' · default' : ''}</option>`).join('');
    get('export-sample-rate').innerHTML = session.sampleRates.map(rate => `<option value="${rate}" ${rate === selectedRate ? 'selected' : ''}>${rateName(rate)}${rate === defaultSampleRate ? ' · default' : ''}</option>`).join('');
    get('export-format-note').textContent = session.sampleRateSupported
        ? 'Stereo WAV delivery · engine and track bounce remain at 48 kHz'
        : session.encodingSupported
        ? 'WAV files · fixed 48 kHz stereo'
        : 'WAV files · fixed 48 kHz stereo · this host supports PCM 24-bit export';
  }
  function requireProject(session) {
    if (!active(session) || !sameProject(session)) throw new Error('The project changed. Reopen Export for the current session.');
  }
  function completedResult(session, result, expected, encoding, sampleRate) {
    if (result?.cancelled === true
        || result?.canceled === true) throw new Error('Export canceled. Completed or partial files may remain at the chosen destination.');
    if (!result || result.success === false || result.error) throw new Error(result?.error || 'The host did not confirm a completed export.');
    const paths = Array.isArray(result.paths) ? result.paths : Array.isArray(result.stems) ? result.stems.map(stem => stem.path) : [];
    if (!paths.length
        || paths.some(path => typeof path !== 'string' || !path.trim())
        || new Set(paths).size !== paths.length) throw new Error('The host returned no complete exported file list. Check the destination before trying again.');
    if (paths.length !== expected) throw new Error(`Export incomplete: the host reported ${paths.length} of ${expected} expected files. Check the destination before trying again.`);
    if (session.encodingSupported
        && (result.container !== 'wav' || result.encoding !== encoding || result.sampleRate !== sampleRate)) throw new Error(`The host did not confirm the requested WAV encoding and ${rateName(sampleRate)} sample rate. Check the exported files before using them.`);
    return {...result,paths,path:typeof result.path === 'string' && result.path ? result.path : session.path};
  }
  async function start(session) {
    if (external || !active(session) || session.busy || session.loading || session.waiting || !S().state.native) return;
    const preparation = pendingPreparation(); if (preparation) { status(session,preparation); return; }
    session.mode = get('export-mode').value;
    const rawTail = String(get('export-tail').value), tail = Number(rawTail), owners = selectedTargets(session).map(target => target.owner);
    if (!['song','pattern','stems'].includes(session.mode)
        || !rawTail.trim()
        || !Number.isFinite(tail)
        || tail < 0
        || tail > 30) { status(session,'Choose a valid export range and a tail between 0 and 30 seconds.','error'); return; }
    if (session.mode === 'stems' && session.fullGraph && !owners.length) { status(session,'Select at least one available stem channel.',
        'error'); get('export-search')?.focus(); return; }
    const encoding = get('export-encoding').value;
    if (!session.encodings.includes(encoding)) { status(session,'Choose a WAV encoding supported by this audio host.',
        'error'); get('export-encoding').focus(); return; }
    const rawRate = get('export-sample-rate').value, sampleRate = Number(rawRate);
    if (typeof rawRate !== 'string'
        || String(sampleRate) !== rawRate
        || !Number.isFinite(sampleRate)
        || !Number.isInteger(sampleRate)
        || !session.sampleRates.includes(sampleRate)) { status(session,'Choose a sample rate advertised by this audio host.',
            'error'); get('export-sample-rate').focus(); return; }
    const mode = session.mode,
      pattern = session.project.activePattern,
      expected = mode === 'stems' ? session.fullGraph ? owners.length : session.project.tracks.length : 1;
    if (!expected) { status(session,'There are no tracks to export.','error'); return; }
    // Existing graph edits must be allowed to commit before the application-wide
    // export lock starts. This wait is closable and cannot start a second export.
    session.waiting = true;
    session.phase = 'waiting';
    session.cancelRequested = false;
    session.cancelPromise = null;
    session.result = null;
    session.path = '';
    controls(session);
    status(session, 'Waiting for routing changes to finish…');
    try {
      requireProject(session);
      await S().waitForGraphEdit?.(); requireProject(session);
      const preparation = pendingPreparation(); if (preparation) { status(session,preparation); return; }
      session.waiting = false; session.busy = true; session.phase = 'choosing'; controls(session); status(session,'Choose an export destination.');
      const destination = await S().request('dialog.export',{name:session.project.name,mode});
      requireProject(session);
      if (destination?.cancelled === true || destination?.canceled === true) { status(session,'Destination selection canceled.'); return; }
      const path = typeof destination === 'string' ? destination : destination?.path;
      if (typeof path !== 'string' || !path.trim()) throw new Error('No export destination was returned.');
      session.path = path; session.phase = 'preparing'; controls(session); status(session,'Preparing audio and plug-in state…');
      await S().waitForGraphEdit?.(); requireProject(session);
      if (await S().captureNativeEdits?.() === false) throw new Error('Current plug-in settings could not be captured. Nothing was exported.');
      requireProject(session);
      await S().waitForGraphEdit?.(); requireProject(session);
      // Capture after pending native edits settle, and propagate preparation failures.
      S().cancelPendingEngineSync?.();
      const synced = await S().syncEngineProject(() => { requireProject(session); return clone(session.project); }, {capture:true});
      requireProject(session);
      if (synced === false) throw new Error('Audio preparation was canceled. Nothing was exported.');
      const payload = {mode,path,tail,pattern};
      if (session.encodingSupported) Object.assign(payload,{encoding,sampleRate});
      if (mode === 'stems' && session.fullGraph) payload.owners = owners;
      session.renderSampleRate = sampleRate;
      session.phase = 'rendering';
      get('export-meter').value = 0;
      controls(session);
      status(session, `Rendering audio · ${rateName(sampleRate)}…`);
      get('export-cancel').focus();
      const result = await S().request('audio.export',payload);
      if (session.cancelPromise) await session.cancelPromise;
      requireProject(session);
      session.result = completedResult(session,result,expected,encoding,sampleRate);
      const count = session.result.paths.length,
        label = mode === 'stems' ? `${count} stem${count === 1 ? '' : 's'} exported.` : 'Audio export complete.';
      status(session,
          label
              + ` WAV · ${encodingNames[encoding]} · ${rateName(sampleRate)}.`
              + (session.cancelRequested ? ' The render finished before cancellation took effect.' : '')
              + (result.warning ? ` ${result.warning}` : ''));
      S().toast(label,'success');
    } catch (error) {
      if (session.cancelPromise) await session.cancelPromise;
      const message = error?.message || String(error), canceled = /cancel(?:led|ed|ation)|aborted/i.test(message);
      status(session,message,canceled ? '' : 'error');
    } finally { session.waiting = false; session.busy = false; session.phase = 'idle'; controls(session); }
  }
  // Headless callers share the dialog's exclusion and strict native preparation.
  // This operation never creates a modal or changes its controls.
  async function exportTo(options = {}) {
    if (external || current && active(current)) throw new Error('Finish or close the current audio export before starting another.');
    if (!S().state.native) throw new Error('Audio export requires the native app.');
    const project = S().state.project, mode = options.mode || 'song', path = options.path;
    const tail = options.tail ?? 3, pattern = options.pattern ?? project.activePattern;
    const patternObject = mode === 'pattern' ? project.patterns.find(item => item.id === pattern) : null;
    const encoding = options.encoding ?? 'pcm24', sampleRate = options.sampleRate ?? defaultSampleRate;
    if (!['song', 'pattern', 'stems'].includes(mode)
        || typeof path !== 'string'
        || !path.startsWith('/')
        || !path.trim()) throw new Error('Choose an absolute export destination and a valid range.');
    if (typeof tail !== 'number'
        || !Number.isFinite(tail)
        || tail < 0
        || tail > 30) throw new Error('Choose an effect tail between 0 and 30 seconds.');
    if (mode === 'pattern' && !patternObject) throw new Error('The selected export pattern does not exist.');
    if (!Object.hasOwn(encodingNames, encoding)
        || !knownSampleRates.includes(sampleRate)) throw new Error('Choose a supported WAV encoding and sample rate.');
    const owners = options.owners === undefined ? null : options.owners;
    if (owners !== null
        && (mode !== 'stems' || !Array.isArray(owners) || !owners.length || owners.some(owner => typeof owner !== 'string' || !owner) || new Set(owners).size !== owners.length)) throw new Error('Select unique stem channel owners.');
    const selectedOwners = owners && [...owners];
    const session = { project, path, busy:false, phase:'waiting', encodingSupported:false, cancelRequested:false, cancelPromise:null, progress:0 };
    const requireCurrent = () => {
      if (external !== session
          || S().state.project !== project
          || patternObject
              && (patternObject.id !== pattern || !project.patterns.includes(patternObject))) throw new Error('The project or selected pattern changed. Start the export again.');
      const preparation = pendingPreparation(); if (preparation) throw new Error(preparation);
    };
    external = session;
    try {
      requireCurrent(); await S().waitForGraphEdit?.(); requireCurrent();
      session.busy = true; session.phase = 'preparing';
      let capabilities;
      try { capabilities = await S().request('routing.capabilities',{}); } catch { capabilities = null; }
      requireCurrent();
      const format = capabilities?.audioExport;
      session.encodingSupported = format?.container === 'wav'
          && format.sampleRate === defaultSampleRate
          && format.defaultEncoding === 'pcm24'
          && Array.isArray(format.encodings)
          && format.encodings.includes('pcm24');
      const rates = session.encodingSupported
          && format.defaultSampleRate === defaultSampleRate
          && Array.isArray(format.sampleRates)
          && format.sampleRates.includes(defaultSampleRate)
          ? knownSampleRates.filter(rate => format.sampleRates.includes(rate))
          : [defaultSampleRate];
      if (session.encodingSupported
          ? !format.encodings.includes(encoding) || !rates.includes(sampleRate)
          : encoding !== 'pcm24'
              || sampleRate !== defaultSampleRate) throw new Error('The host does not advertise the requested WAV encoding and sample rate.');
      if (selectedOwners) {
        const targets = parseTargets(capabilities);
        if (capabilities?.fullGraphStems !== true
            || selectedOwners.some(owner => !targets.some(target => target.owner === owner
                && target.available))) throw new Error('One or more selected stem channels are unavailable.');
      }
      await S().waitForGraphEdit?.(); requireCurrent();
      if (await S().captureNativeEdits?.() === false) throw new Error('Current plug-in settings could not be captured. Nothing was exported.');
      requireCurrent(); await S().waitForGraphEdit?.(); requireCurrent();
      S().cancelPendingEngineSync?.();
      const synced = await S().syncEngineProject(() => { requireCurrent(); return clone(project); }, {capture:true});
      requireCurrent();
      if (synced === false) throw new Error('Audio preparation was canceled. Nothing was exported.');
      const payload = {mode,path,tail,pattern};
      if (selectedOwners) payload.owners = selectedOwners;
      if (session.encodingSupported) Object.assign(payload,{encoding,sampleRate});
      const expected = mode === 'stems' ? selectedOwners?.length || defaultStemCount(project) : 1;
      if (!expected) throw new Error('There are no tracks to export.');
      session.phase = 'rendering';
      const result = await S().request('audio.export',payload);
      if (session.cancelPromise) await session.cancelPromise;
      requireCurrent();
      return completedResult(session,result,expected,encoding,sampleRate);
    } finally {
      if (session.cancelPromise) await session.cancelPromise;
      session.busy = false; session.phase = 'idle';
      if (external === session) external = null;
    }
  }
  function cancel() {
    if (external) {
      const session = external;
      if (session.phase !== 'rendering' || session.cancelRequested) return false;
      session.cancelRequested = true; session.phase = 'canceling';
      session.cancelPromise = S().request('transport.stop',{}).catch(error => {
        if (external !== session || !session.busy) return;
        session.cancelRequested = false; session.phase = 'rendering'; session.cancelError = error.message || String(error);
      });
      return true;
    }
    const session = current;
    if (!session || !active(session) || session.phase !== 'rendering' || session.cancelRequested) return false;
    session.cancelRequested = true; session.phase = 'canceling'; controls(session); status(session,'Canceling export…');
    session.cancelPromise = S().request('transport.stop',{}).catch(error => {
      if (!session.busy || !active(session)) return;
      session.cancelRequested = false;
      session.phase = 'rendering';
      status(session, `Could not cancel the render: ${error.message || error}`, 'error');
      controls(session);
    });
    return true;
  }
  function closed() { if (current?.busy) return false; current = null; return true; }
  function event(type,payload = {}) {
    if (external && ['rendering','canceling'].includes(external.phase) && type === 'audio.exportProgress') {
      const pathMatches = !payload.path
          || typeof payload.path === 'string' && (payload.path === external.path || payload.path.startsWith(external.path.replace(/\/$/, '') + '/'));
      if (pathMatches && Number.isFinite(Number(payload.progress))) external.progress = Math.max(0,Math.min(1,Number(payload.progress)));
    }
    const session = current; if (!session || !active(session) || !['rendering','canceling'].includes(session.phase)) return;
    if (type !== 'audio.exportProgress') return;
    if (payload.path
        && session.path
        && (typeof payload.path !== 'string' || payload.path !== session.path
            && !payload.path.startsWith(session.path.replace(/\/$/,'')+'/'))) return;
    const progress = Number(payload.progress); if (!Number.isFinite(progress)) return;
    get('export-meter').value = Math.max(0,Math.min(1,progress));
    if (!session.cancelRequested) status(session,
        `Rendering audio · ${rateName(session.renderSampleRate)} · ${Math.round(Math.max(0, Math.min(1, progress)) * 100)}%`);
  }
  async function show() {
    if (external || current?.busy) return false;
    if (current && active(current)) { get('export-mode')?.focus(); return true; }
    const session = {
      project: S().state.project,
      mode: 'song',
      selected: new Set(),
      targets: [],
      query: '',
      fullGraph: false,
      encodings: ['pcm24'],
      encodingSupported: false,
      sampleRates: [defaultSampleRate],
      sampleRateSupported: false,
      loading: true,
      busy: false,
      waiting: false,
      phase: 'idle',
      path: '',
      result: null,
      cancelRequested: false,
      cancelPromise: null
    };
    S().openModal('Export audio', `<div class="export-content"><p class="export-description">Render the song, current pattern, or stems to audio.</p>`
        + `<div class="export-options"><label class="field"><span>Export range</span><select id="export-mode"><option value="song">Full song</option>`
        + `<option value="pattern">Current pattern</option><option value="stems">Stems</option></select></label>`
        + `<label class="field"><span>Effect tail · seconds</span><input id="export-tail" type="number" min="0" max="30" step=".5"`
        + ` value="3"></label></div><div class="export-format"><label class="field"><span>WAV encoding</span><select id="export-encoding"`
        + ` aria-describedby="export-format-note"><option value="pcm24" selected>PCM 24-bit · default</option></select>`
        + `</label><label class="field"><span>Sample rate</span><select id="export-sample-rate" aria-describedby="export-format-note">`
        + `<option value="48000" selected>48 kHz · default</option></select></label><p id="export-format-note" class="export-description">WAV `
        + `files · fixed 48 kHz stereo</p></div><section id="export-stems" class="export-stems" hidden><div id="export-stem-picker">`
        + `<div class="export-selection-tools"><input type="search" id="export-search" placeholder="Find a stem channel…" aria-label="Search stem `
        + `channels"><output id="export-selected-count" aria-live="polite">0 selected</output></div><div class="export-selection-actions">`
        + `<span>↑ ↓ move · Space select</span><div><button id="export-select-visible">Select visible</button><button id="export-clear">Clear `
        + `all</button></div></div><div id="export-targets" class="export-targets" role="group" aria-label="Stem channels">`
        + `</div></div><p id="export-legacy-stems" class="export-description"></p><p id="export-boundary" class="export-description export-boundary">`
        + `</p></section><p class="export-hardware-note">External hardware instruments need an audio recording before offline export.</p>`
        + `<div class="export-status"><progress id="export-meter" max="1" value="0" hidden aria-label="Export progress">`
        + `</progress><p id="export-progress" role="status" aria-live="polite">Checking stem options…</p></div><label id="export-result"`
        + ` class="field export-result" hidden><span>Destination</span><input id="export-result-path" type="text" readonly aria-label="Export `
        + `destination"></label></div>`, '<button id="export-cancel">Close</button><button id="export-start" class="primary">Choose destination & export…</button>', {
      kind: 'export',
      className: 'export-modal'
    });
    if (S().state.modal !== 'export') return false;
    current = session;
    get('export-mode').addEventListener('change',()=>{session.mode=get('export-mode').value;controls(session);});
    get('export-search').addEventListener('input',()=>{session.query=get('export-search').value;renderTargets(session);});
    get('export-search').addEventListener('keydown', event => {
      if (event.key === 'ArrowDown' && !event.altKey && !event.ctrlKey && !event.metaKey) {
        event.preventDefault();
        get('export-targets').querySelector('[data-export-owner]:not(:disabled)')?.focus();
      }
    });
    get('export-select-visible').addEventListener('click', () => {
      for (const target of visibleTargets(session)) if (target.available) session.selected.add(target.owner);
      renderTargets(session);
    });
    get('export-clear').addEventListener('click',()=>{session.selected.clear();renderTargets(session);});
    get('export-start').addEventListener('click',()=>start(session));
    get('export-cancel').addEventListener('click',()=>{if(session.busy)cancel();else S().closeModal();});
    controls(session); get('export-mode').focus();
    try {
      const capabilities = await S().request('routing.capabilities',{});
      if (!active(session) || !sameProject(session)) return false;
      configureEncoding(session,capabilities);
      session.fullGraph = capabilities?.fullGraphStems === true;
      session.targets = session.fullGraph ? parseTargets(capabilities) : [];
      session.selected = new Set(session.targets.filter(target=>target.available&&target.kind==='track').map(target=>target.owner));
      status(session,
          session.fullGraph && !session.targets.some(target => target.available)
              ? 'The host reported no available stem channels. Song and pattern export remain available.'
              : 'Ready to export.');
    } catch (error) {
      if (!active(session)) return false;
      configureEncoding(session,null);
      status(session,'Channel selection is unavailable. You can still export the song, current pattern, or all tracks.');
    } finally { if (active(session)) { session.loading=false;renderTargets(session);controls(session); } }
    return true;
  }
  window.STExport = {
    show,
    exportTo,
    cancel,
    closed,
    event,
    get isBusy() {
      return !!current?.busy || !!external?.busy;
    },
    get isPreparing() {
      return current?.busy && current.phase === 'preparing' || external?.busy && external.phase === 'preparing' || false;
    },
    get progress() {
      return external?.progress ?? null;
    }
  };
})();
