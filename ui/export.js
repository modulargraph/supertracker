/* Offline rendering with explicit stem boundaries and guarded native completion. */
(() => {
  'use strict';
  let current = null, external = null;
  const S = () => window.ST;
  const get = id => document.getElementById(id);
  const esc = value => S().escapeHTML(String(value ?? ''));
  const { clone } = window.STUtil;
  const encodingNames = Object.freeze({pcm16:'PCM 16-bit',pcm24:'PCM 24-bit',float32:'32-bit float'});
  const ditherNames = Object.freeze({tpdf:'TPDF',none:'None'});
  // Dither belongs to the integer encodings; the host writes a float file as rendered.
  const dithered = encoding => encoding !== 'float32';
  // Host default for stems without explicit owners: one stem per track (st_audio::export).
  const defaultStemCount = project => project.tracks.length;
  const rateName = value => `${value/1000} kHz`;
  const formatName = format => `WAV · ${encodingNames[format.encoding]} · ${rateName(format.sampleRate)}`
      + (dithered(format.encoding) ? format.dither === 'none' ? ' · no dither' : ` · ${ditherNames[format.dither]} dither` : '');
  /* The delivery formats the host writes (audioExport in the answer to routing.capabilities): the choices of
     the three format fields, each with the host's default. */
  function exportFormats(capabilities) {
    const format = capabilities?.audioExport, list = value => Array.isArray(value) ? value : [];
    const formats = {
      encodings: Object.keys(encodingNames).filter(encoding => list(format?.encodings).includes(encoding)),
      sampleRates: list(format?.sampleRates).filter(rate => Number.isInteger(rate) && rate > 0).sort((a, b) => a - b),
      dithers: Object.keys(ditherNames).filter(dither => list(format?.dithers).includes(dither)),
      encoding: format?.defaultEncoding, sampleRate: format?.defaultSampleRate, dither: format?.defaultDither
    };
    if (format?.container !== 'wav'
        || !formats.encodings.includes(formats.encoding)
        || !formats.sampleRates.includes(formats.sampleRate)
        || !formats.dithers.includes(formats.dither)) throw new Error('The audio host did not describe its export formats.');
    return formats;
  }
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
  const kindName = target => ({
    track: 'Track',
    group: 'Group',
    return: 'Return',
    master: 'Master'
  })[target.kind];
  const selectedTargets = session => session.targets.filter(target => session.selected.has(target.owner));
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
    // Formats and stem channels come from the host: without its answer nothing can be exported.
    const locked = session.busy || session.loading || session.waiting || !session.formats;
    // Range and tail do not depend on what the host offers. Keep them usable
    // while it is asked so the opening keyboard focus has a field.
    for (const id of ['export-mode','export-tail']) if (get(id)) get(id).disabled = session.busy || session.waiting;
    for (const id of ['export-search','export-select-visible','export-clear','export-encoding','export-sample-rate']) if (get(id)) get(id).disabled = locked;
    get('export-dither').disabled = locked || !dithered(get('export-encoding').value);
    for (const input of document.querySelectorAll('[data-export-owner]')) input.disabled = locked;
    get('export-stems').hidden = session.mode !== 'stems';
    get('export-start').disabled = locked || !S().state.native || (session.mode === 'stems' && !selectedTargets(session).length);
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
    get('export-result').hidden = !session.path;
    get('export-result-path').value = session.result?.path || session.path || '';
  }
  function renderTargets(session) {
    if (!active(session)) return;
    const list = get('export-targets'), targets = visibleTargets(session);
    const focused = document.activeElement?.dataset?.exportOwner;
    list.innerHTML = targets.length ? targets.map(target => {
      const index = session.targets.indexOf(target), id = `export-stem-${index}`;
      return `<label class="export-target" for="${id}"><input type="checkbox" id="${id}" data-export-owner="${esc(target.owner)}"`
          + ` ${session.selected.has(target.owner) ? 'checked' : ''} ${session.busy || session.loading ? 'disabled' : ''}>`
          + `<span class="export-target-name">${esc(target.name)}</span><span class="export-target-kind">${esc(kindName(target))}</span></label>`;
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
  /* The channels the host can write as stems: every strip of its mixer, `{owner, name, kind}` with kind track,
     group, return or master. A track, group or return stem is taken after its fader and before its output
     bus; the master is the complete mix. The built-in delay and reverb are part of the master only. */
  const exportTargets = capabilities => capabilities.exportTargets.map(({ owner, name, kind }) => ({ owner, name, kind }));
  // Fill the three format fields with what the host offers; the host's defaults are selected.
  function showFormats(session) {
    const formats = session.formats;
    const options = (id, values, selected, name) => {
      get(id).innerHTML = values.map(value => `<option value="${value}" ${value === selected ? 'selected' : ''}>${name(value)}${value === selected ? ' · default' : ''}</option>`).join('');
      get(id).value = String(selected);
    };
    options('export-encoding', formats.encodings, formats.encoding, encoding => encodingNames[encoding]);
    options('export-sample-rate', formats.sampleRates, formats.sampleRate, rateName);
    options('export-dither', formats.dithers, formats.dither, dither => ditherNames[dither]);
  }
  function requireProject(session) {
    if (!active(session) || !sameProject(session)) throw new Error('The project changed. Reopen Export for the current session.');
  }
  function completedResult(session, result, expected, format) {
    if (result?.cancelled === true
        || result?.canceled === true) throw new Error('Export canceled. Completed or partial files may remain at the chosen destination.');
    if (!result || result.success === false || result.error) throw new Error(result?.error || 'The host did not confirm a completed export.');
    const paths = Array.isArray(result.paths) ? result.paths : Array.isArray(result.stems) ? result.stems.map(stem => stem.path) : [];
    if (!paths.length
        || paths.some(path => typeof path !== 'string' || !path.trim())
        || new Set(paths).size !== paths.length) throw new Error('The host returned no complete exported file list. Check the destination before trying again.');
    if (paths.length !== expected) throw new Error(`Export incomplete: the host reported ${paths.length} of ${expected} expected files. Check the destination before trying again.`);
    if (result.container !== 'wav'
        || result.encoding !== format.encoding
        || result.sampleRate !== format.sampleRate) throw new Error(`The host did not confirm the requested WAV encoding and ${rateName(format.sampleRate)} sample rate. Check the exported files before using them.`);
    if (dithered(format.encoding)
        && result.dither !== format.dither) throw new Error('The host did not confirm the requested dither. Check the exported files before using them.');
    return {...result,paths,path:typeof result.path === 'string' && result.path ? result.path : session.path};
  }
  async function start(session) {
    if (external || !active(session) || session.busy || session.loading || session.waiting || !session.formats || !S().state.native) return;
    const preparation = pendingPreparation(); if (preparation) { status(session,preparation); return; }
    session.mode = get('export-mode').value;
    const rawTail = String(get('export-tail').value), tail = Number(rawTail), owners = selectedTargets(session).map(target => target.owner);
    if (!['song','pattern','stems'].includes(session.mode)
        || !rawTail.trim()
        || !Number.isFinite(tail)
        || tail < 0
        || tail > 30) { status(session,'Choose a valid export range and a tail between 0 and 30 seconds.','error'); return; }
    if (session.mode === 'stems' && !owners.length) { status(session,'Select at least one available stem channel.',
        'error'); get('export-search')?.focus(); return; }
    const encoding = get('export-encoding').value;
    if (!session.formats.encodings.includes(encoding)) { status(session,'Choose a WAV encoding supported by this audio host.',
        'error'); get('export-encoding').focus(); return; }
    const rawRate = get('export-sample-rate').value, sampleRate = Number(rawRate);
    if (typeof rawRate !== 'string'
        || String(sampleRate) !== rawRate
        || !session.formats.sampleRates.includes(sampleRate)) { status(session,'Choose a sample rate advertised by this audio host.',
            'error'); get('export-sample-rate').focus(); return; }
    const dither = get('export-dither').value;
    if (dithered(encoding) && !session.formats.dithers.includes(dither)) { status(session,'Choose a dither supported by this audio host.',
        'error'); get('export-dither').focus(); return; }
    const mode = session.mode,
      pattern = session.project.activePattern,
      format = {encoding,sampleRate,dither},
      expected = mode === 'stems' ? owners.length : 1;
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
      const payload = {mode,path,tail,pattern,encoding,sampleRate};
      if (dithered(encoding)) payload.dither = dither;
      if (mode === 'stems') payload.owners = owners;
      session.renderSampleRate = sampleRate;
      session.phase = 'rendering';
      get('export-meter').value = 0;
      controls(session);
      status(session, `Rendering audio · ${rateName(sampleRate)}…`);
      get('export-cancel').focus();
      const result = await S().request('audio.export',payload);
      if (session.cancelPromise) await session.cancelPromise;
      requireProject(session);
      session.result = completedResult(session,result,expected,format);
      const count = session.result.paths.length,
        label = mode === 'stems' ? `${count} stem${count === 1 ? '' : 's'} exported.` : 'Audio export complete.';
      status(session,
          label
              + ` ${formatName(format)}.`
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
    if (!['song', 'pattern', 'stems'].includes(mode)
        || typeof path !== 'string'
        || !path.startsWith('/')
        || !path.trim()) throw new Error('Choose an absolute export destination and a valid range.');
    if (typeof tail !== 'number'
        || !Number.isFinite(tail)
        || tail < 0
        || tail > 30) throw new Error('Choose an effect tail between 0 and 30 seconds.');
    if (mode === 'pattern' && !patternObject) throw new Error('The selected export pattern does not exist.');
    const owners = options.owners === undefined ? null : options.owners;
    if (owners !== null
        && (mode !== 'stems' || !Array.isArray(owners) || !owners.length || owners.some(owner => typeof owner !== 'string' || !owner) || new Set(owners).size !== owners.length)) throw new Error('Select unique stem channel owners.');
    const selectedOwners = owners && [...owners];
    const session = { project, path, busy:false, phase:'waiting', cancelRequested:false, cancelPromise:null, progress:0 };
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
      const capabilities = await S().request('routing.capabilities',{});
      requireCurrent();
      const formats = exportFormats(capabilities);
      const format = {
        encoding: options.encoding ?? formats.encoding,
        sampleRate: options.sampleRate ?? formats.sampleRate,
        dither: options.dither ?? formats.dither
      };
      if (!formats.encodings.includes(format.encoding)
          || !formats.sampleRates.includes(format.sampleRate)
          || !formats.dithers.includes(format.dither)) throw new Error('The host does not advertise the requested WAV encoding, sample rate and dither.');
      if (selectedOwners) {
        const targets = exportTargets(capabilities);
        if (selectedOwners.some(owner => !targets.some(target => target.owner === owner))) throw new Error('One or more selected stem channels are unavailable.');
      }
      await S().waitForGraphEdit?.(); requireCurrent();
      if (await S().captureNativeEdits?.() === false) throw new Error('Current plug-in settings could not be captured. Nothing was exported.');
      requireCurrent(); await S().waitForGraphEdit?.(); requireCurrent();
      S().cancelPendingEngineSync?.();
      const synced = await S().syncEngineProject(() => { requireCurrent(); return clone(project); }, {capture:true});
      requireCurrent();
      if (synced === false) throw new Error('Audio preparation was canceled. Nothing was exported.');
      const payload = {mode,path,tail,pattern,encoding:format.encoding,sampleRate:format.sampleRate};
      if (dithered(format.encoding)) payload.dither = format.dither;
      if (selectedOwners) payload.owners = selectedOwners;
      const expected = mode === 'stems' ? selectedOwners?.length || defaultStemCount(project) : 1;
      if (!expected) throw new Error('There are no tracks to export.');
      session.phase = 'rendering';
      const result = await S().request('audio.export',payload);
      if (session.cancelPromise) await session.cancelPromise;
      requireCurrent();
      return completedResult(session,result,expected,format);
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
      formats: null,
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
        + ` aria-describedby="export-format-note"></select>`
        + `</label><label class="field"><span>Sample rate</span><select id="export-sample-rate" aria-describedby="export-format-note">`
        + `</select></label><label class="field"><span>Dither</span><select id="export-dither" aria-describedby="export-format-note">`
        + `</select></label><p id="export-format-note" class="export-description">Stereo WAV delivery · engine and track bounce remain at `
        + `48 kHz · dither applies to the PCM encodings</p></div><section id="export-stems" class="export-stems" hidden>`
        + `<div class="export-selection-tools"><input type="search" id="export-search" placeholder="Find a stem channel…" aria-label="Search stem `
        + `channels"><output id="export-selected-count" aria-live="polite">0 selected</output></div><div class="export-selection-actions">`
        + `<span>↑ ↓ move · Space select</span><div><button id="export-select-visible">Select visible</button><button id="export-clear">Clear `
        + `all</button></div></div><div id="export-targets" class="export-targets" role="group" aria-label="Stem channels">`
        + `</div><p id="export-boundary" class="export-description export-boundary">Track, group and return stems are taken after their `
        + `faders, before their output buses. The complete graph keeps running, including sidechains. Parent and child stems overlap; combining `
        + `them can double-count audio. Master contains the complete mix.</p></section><p class="export-hardware-note">External hardware instruments need an audio recording before offline export.</p>`
        + `<div class="export-status"><progress id="export-meter" max="1" value="0" hidden aria-label="Export progress">`
        + `</progress><p id="export-progress" role="status" aria-live="polite">Checking export options…</p></div><label id="export-result"`
        + ` class="field export-result" hidden><span>Destination</span><input id="export-result-path" type="text" readonly aria-label="Export `
        + `destination"></label></div>`, '<button id="export-cancel">Close</button><button id="export-start" class="primary">Choose destination & export…</button>', {
      kind: 'export',
      className: 'export-modal'
    });
    if (S().state.modal !== 'export') return false;
    current = session;
    get('export-mode').addEventListener('change',()=>{session.mode=get('export-mode').value;controls(session);});
    get('export-encoding').addEventListener('change',()=>controls(session));
    get('export-search').addEventListener('input',()=>{session.query=get('export-search').value;renderTargets(session);});
    get('export-search').addEventListener('keydown', event => {
      if (event.key === 'ArrowDown' && !event.altKey && !event.ctrlKey && !event.metaKey) {
        event.preventDefault();
        get('export-targets').querySelector('[data-export-owner]:not(:disabled)')?.focus();
      }
    });
    get('export-select-visible').addEventListener('click', () => {
      for (const target of visibleTargets(session)) session.selected.add(target.owner);
      renderTargets(session);
    });
    get('export-clear').addEventListener('click',()=>{session.selected.clear();renderTargets(session);});
    get('export-start').addEventListener('click',()=>start(session));
    get('export-cancel').addEventListener('click',()=>{if(session.busy)cancel();else S().closeModal();});
    controls(session); get('export-mode').focus();
    try {
      const capabilities = await S().request('routing.capabilities',{});
      if (!active(session) || !sameProject(session)) return false;
      session.formats = exportFormats(capabilities);
      showFormats(session);
      session.targets = exportTargets(capabilities);
      session.selected = new Set(session.targets.filter(target=>target.kind==='track').map(target=>target.owner));
      status(session, 'Ready to export.');
    } catch (error) {
      if (!active(session)) return false;
      status(session,`Export options could not be read: ${error?.message || error} Close this dialog and open Export again.`,'error');
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
