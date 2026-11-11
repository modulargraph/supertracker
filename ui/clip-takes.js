/* Keyboard-first alternatives and comp regions. A switch is one ordinary project
 * edit; original recordings remain available to every split region. */
(() => {
  'use strict';
  const app = () => window.ST, model = () => window.STClipTakesProject;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const get = id => document.getElementById(id);
  const selected = () => app().state.project.audioClips.find(clip => String(clip.id) === String(app().state.clipSelected));
  const timing = project => (start, length) => window.STProjectTiming.durationFromBeats(project, start, length);
  let pending = false, session = null, previewJob = null, previewGeneration = 0;
  const warn = error => app().toast(error.message || String(error), 'error');
  function available(clip, take, project = app().state.project) { return model().fits(clip, take, timing(project)); }
  function assign(clip, proposal) { for (const key of Object.keys(clip)) if (!(key in proposal)) delete clip[key]; Object.assign(clip, proposal); }
  function commit(clip, proposal, label) {
    if (window.STBusy.any(['session', 'export', 'routing'])) throw new Error('Finish the current project operation before changing takes.');
    if (!app().state.project.audioClips.includes(clip)) throw new Error('The destination clip changed. Select it again.');
    const ownerProject = app().state.project;
    let applied = false;
    app().mutate(label, () => { assign(clip, proposal); applied = true; });
    if (!applied
        || app().state.project !== ownerProject
        || !ownerProject.audioClips.includes(clip)
        || JSON.stringify(clip) !== JSON.stringify(proposal)) throw new Error('The take edit was not applied. Your source file is preserved.');
  }
  function focusClip() { const clip = selected(); (clip && get(`clip-${clip.id}`) || get('clips-timeline'))?.focus({ preventScroll: true }); }
  function caption(clip) {
    try { const set = model().read(clip); return set.takes.length > 1
        ? ` · Take ${set.takes.findIndex(take => take.id === set.activeID) + 1}/${set.takes.length}`
        : ''; } catch { return ' · Check takes'; }
  }
  function render(clip) {
    try {
      const set = model().read(clip), full = set.takes.length >= model().maxTakes;
      return `<div class="clips-takes"><label class="field"><span>Active take · ${set.takes.length} retained</span><select id="clips-active-take"`
          + ` aria-label="Active take for this region">${set.takes.map((take, index) => `<option value="${esc(take.id)}" ${take.id === set.activeID ? 'selected' : ''} ${available(clip, take) ? '' : 'disabled'}>${index + 1}. ${esc(take.name || 'Untitled take')}${available(clip, take) ? '' : ' · too short'}</option>`).join('')}</select>`
          + `</label><div class="clips-take-buttons"><button id="clips-take-prev" title="Previous available take · Page Up" ${set.takes.length < 2 ? 'disabled' : ''}>← `
          + `Take</button><button id="clips-take-next" title="Next available take · Page Down" ${set.takes.length < 2 ? 'disabled' : ''}>Take →</button>`
          + `<button id="clips-take-manage" title="Names, source alignment and retained alternatives · T">Manage takes…</button>`
          + `<button id="clips-take-import" ${pending || full ? 'disabled' : ''}>${pending ? 'Reading take…' : 'Import take…'}</button>`
          + `<button id="clips-take-record" ${pending || full ? 'disabled' : ''}>Record take…</button></div><p>Split at the edit cursor, then choose `
          + `a take for each region. Source files and the other takes stay intact. <kbd>PgUp / PgDn</kbd> changes take.</p></div>`;
    } catch (error) { return `<p class="clips-original-speed is-error">${esc(error.message)}</p>`; }
  }
  function use(id, clip = selected(), focus = true) {
    if (!clip) return;
    try {
      const set = model().read(clip); if (id === set.activeID) return;
      commit(clip, model().activate(clip, id, timing(app().state.project)), 'choose audio take');
      if (focus) focusClip();
      app().toast(`Active take: ${model().read(clip).takes.find(take => take.id === id).name}`);
    } catch (error) { warn(error); }
  }
  function cycle(direction) {
    const clip = selected(); if (!clip) return;
    try {
      const set = model().read(clip), index = set.takes.findIndex(take => take.id === set.activeID);
      for (let step = 1; step < set.takes.length; step++) {
        const take = set.takes[(index + direction * step + set.takes.length) % set.takes.length];
        if (available(clip, take)) { use(take.id, clip); return; }
      }
      app().toast('No other take covers this region. Import or record an alternative.', 'info');
    } catch (error) { warn(error); }
  }
  function target() {
    const clip = selected(); if (!clip) throw new Error('Select a Song audio clip first.');
    const project = app().state.project;
    const set = model().read(clip);
    if (set.takes.length >= model().maxTakes) throw new Error('This clip already has 32 takes. Remove an unused alternative first.');
    return { project, clip, fingerprint: JSON.stringify(clip) };
  }
  function requireTarget(owner) {
    if (!owner
        || app().state.project !== owner.project
        || !owner.project.audioClips.includes(owner.clip)
        || JSON.stringify(owner.clip) !== owner.fingerprint) throw new Error('The project or destination clip changed. Your source file is preserved; select the clip and add the take again.');
  }
  async function addFile(path, owner = target(), name) {
    requireTarget(owner);
    const activePath = owner.clip.path || owner.clip.samplePath;
    const [meta, previous] = await Promise.all([app().request('audio.clipInfo', { path }),
        activePath === path ? Promise.resolve(null) : app().request('audio.clipInfo', { path: activePath })]); requireTarget(owner);
    if (!Number.isFinite(Number((previous || meta)?.duration))
        || Number((previous || meta)?.duration) <= 0) throw new Error('The current take’s source duration could not be checked. Your new file is preserved.');
    const proposal = model().add(owner.clip, {
      path,
      name: name || meta?.name || path.split('/').pop(),
      sourceDuration: Number(meta?.duration),
      currentSourceDuration: Number((previous || meta)?.duration)
    }, timing(owner.project), app().uid);
    commit(owner.clip, proposal, 'add audio take'); app().state.clipSelected = owner.clip.id;
    window.STClips?.event('audio.clipLoaded', { ...meta, path });
    return proposal;
  }
  async function importFile() {
    if (pending) return;
    let owner;
    try { owner = target(); } catch (error) { warn(error); return; }
    pending = true; app().refresh();
    try {
      const result = await app().request('dialog.openSample', {}); requireTarget(owner);
      const path = typeof result === 'string' ? result : result?.path;
      if (!path || result?.cancelled) return;
      await addFile(path, owner); app().toast('Alternative added. Other takes are retained.');
    } catch (error) { warn(error); }
    finally { pending = false; app().refresh(); focusClip(); }
  }
  function record() {
    try { const owner = target(); return window.STRecording?.show({ takeTarget: owner }); } catch (error) { warn(error); }
  }
  function active(owner) {
    return session === owner
        && app().state.modal === 'clip-takes'
        && app().state.project === owner.project
        && owner.project.audioClips.includes(owner.clip);
  }
  function show() {
    const clip = selected(); if (!clip) { app().toast('Select a Song audio clip first.', 'info'); return; }
    try { model().read(clip); } catch (error) { warn(error); return; }
    const owner = { project: app().state.project, clip, selectedID: model().read(clip).activeID, query: '' };
    app().openModal('Takes for this region', `<div class="clips-takes-dialog"><p>Choose which recording plays in this region. Other split regions keep their own choices. Files `
        + `are never deleted here.</p><label class="field"><span>Find a take</span><input id="takes-search" type="search" placeholder="Filter `
        + `take names or files…"></label><label class="field"><span>Retained takes</span><select id="takes-list" size="6" aria-label="Retained `
        + `takes"></select></label><div class="clips-take-details"><label class="field"><span>Take name</span><input id="takes-name"`
        + ` maxlength="160"></label><button id="takes-rename">Rename take</button><label class="field"><span>Source offset · seconds</span>`
        + `<input id="takes-offset" type="number" min="0" step=".01"></label><button id="takes-align">Set source offset</button>`
        + `</div><div id="takes-path" class="clips-take-path"></div><p id="takes-coverage" role="status"></p><div class="row">`
        + `<button id="takes-preview">Preview source file</button><button id="takes-stop-preview">Stop preview</button>`
        + `<button id="takes-remove" class="danger">Remove alternative</button></div><p>Source offset aligns this take independently. The region `
        + `length stays fixed. Files must cover the complete region; trim the region first to use a shorter take.</p>`
        + `</div>`, '<button id="takes-close">Close</button><button id="takes-use" class="primary">Use selected take</button>', {
      kind: 'clip-takes',
      className: 'clip-takes-modal'
    });
    if (app().state.modal !== 'clip-takes' || !get('takes-list')) return;
    session = owner;
    const safe = fn => () => {
      if (!active(owner)) return;
      try {
        fn();
      } catch (error) {
        get('takes-coverage').textContent = error.message || String(error);
      }
    };
    get('takes-search').oninput = safe(() => { stopPreview(); owner.query = get('takes-search').value; fill(owner); });
    get('takes-list').onchange = safe(() => { stopPreview(); owner.selectedID = get('takes-list').value; details(owner); });
    get('takes-list').onkeydown = event => {
      if (event.key === 'Enter' && !event.metaKey && !event.ctrlKey && !event.altKey && active(owner)) {
        event.preventDefault();
        get('takes-use').click();
      }
    };
    get('takes-close').onclick = app().closeModal;
    get('takes-use').onclick = safe(() => {
      stopPreview();
      const proposal = model().activate(owner.clip, owner.selectedID, timing(owner.project));
      if (model().read(owner.clip).activeID !== owner.selectedID) commit(owner.clip, proposal, 'choose audio take');
      app().closeModal();
      focusClip();
    });
    get('takes-rename').onclick = safe(() => {
      commit(owner.clip, model().rename(owner.clip, owner.selectedID, get('takes-name').value), 'rename audio take');
      fill(owner);
    });
    get('takes-align').onclick = safe(() => {
      stopPreview();
      const text = get('takes-offset').value.trim(),
        offset = Number(text),
        set = model().read(owner.clip),
        take = set.takes.find(item => item.id === owner.selectedID);
      if (!take || !text || !Number.isFinite(offset) || offset < 0) throw new Error('Choose a valid source offset in seconds.');
      take.sourceOffset = offset;
      if (!available(owner.clip, take, owner.project)) throw new Error('This offset leaves too little audio for the region.');
      const proposal = model().clone(owner.clip); proposal.takeSet = set;
      if (take.id === set.activeID) proposal.sourceOffset = offset;
      commit(owner.clip, proposal, 'align audio take'); fill(owner);
    });
    get('takes-remove').onclick = safe(() => {
      stopPreview();
      const proposal = model().remove(owner.clip, owner.selectedID);
      commit(owner.clip, proposal, 'remove audio alternative');
      owner.selectedID = proposal.takeSet.activeID;
      fill(owner);
    });
    get('takes-preview').onclick = safe(() => { void preview(owner); });
    get('takes-stop-preview').onclick = safe(stopPreview);
    fill(owner); get('takes-list').focus();
  }
  function fill(owner) {
    if (!active(owner)) return;
    const set = model().read(owner.clip), query = owner.query.trim().toLowerCase();
    const filtered = set.takes.filter(take => `${take.name} ${take.path}`.toLowerCase().includes(query));
    if (!filtered.some(take => take.id === owner.selectedID)) owner.selectedID = filtered[0]?.id || '';
    get('takes-list').innerHTML = filtered.map(take => `<option value="${esc(take.id)}" ${take.id === owner.selectedID ? 'selected' : ''}>${take.id === set.activeID ? 'Playing · ' : ''}${esc(take.name || 'Untitled take')}${available(owner.clip, take, owner.project) ? '' : ' · too short'}</option>`).join('');
    get('takes-list').value = owner.selectedID; details(owner);
  }
  function details(owner) {
    if (!active(owner)) return;
    const set = model().read(owner.clip),
      take = set.takes.find(item => item.id === owner.selectedID),
      fits = take && available(owner.clip, take, owner.project);
    get('takes-name').value = take?.name || ''; get('takes-offset').value = take ? String(take.sourceOffset) : '';
    get('takes-path').textContent = take?.path || 'No matching takes.';
    get('takes-coverage').textContent = take
        ? `${take.id === set.activeID ? 'Currently playing. ' : ''}${take.sourceDuration == null
            ? 'Source length has not been checked.'
            : fits
            ? 'Covers this region.'
            : 'Too short for this region.'}${take.sourceDuration != null ? ` Source: ${take.sourceDuration.toFixed(2)} s.` : ''}`
        : 'Change the filter to see retained takes.';
    for (const id of ['takes-name', 'takes-offset', 'takes-rename', 'takes-align', 'takes-preview']) get(id).disabled = !take;
    get('takes-use').disabled = !fits; get('takes-remove').disabled = !take || take.id === set.activeID;
  }
  function stopPreview() {
    const job = previewJob; previewJob = null; previewGeneration++;
    // The native preview request is synchronous on its host actor. Sending stop
    // now queues cancellation after an in-flight start, before any newer preview.
    if (job?.sent) app().request('sample.stopPreview', {}).catch(() => {});
  }
  function closed() { stopPreview(); session = null; }
  async function preview(owner) {
    if (!active(owner)) return;
    const take = model().read(owner.clip).takes.find(item => item.id === owner.selectedID); if (!take) return;
    stopPreview();
    const job = { owner, id: take.id, path: take.path, generation: previewGeneration, sent: true }; previewJob = job;
    try {
      await app().request('sample.preview', { path: take.path });
      if (previewJob !== job || job.generation !== previewGeneration) return;
      if (!active(owner)
          || owner.selectedID !== job.id
          || model().read(owner.clip).takes.find(item => item.id === job.id)?.path !== job.path) { stopPreview(); return; }
      get('takes-coverage').textContent = 'Previewing the source file. Stop preview or close this window to stop.';
    } catch (error) { if (previewJob === job && active(owner)) { previewJob = null; get('takes-coverage').textContent = error.message || String(error); } }
  }
  function bind() {
    const on = (id, fn) => { if (get(id)) get(id).onclick = fn; };
    if (get('clips-active-take')) get('clips-active-take').onchange = event => use(event.target.value, selected(), false);
    on('clips-take-prev', () => cycle(-1));
    on('clips-take-next', () => cycle(1));
    on('clips-take-manage', show);
    on('clips-take-import', importFile);
    on('clips-take-record', record);
  }
  window.STClipTakes = {
    render,
    bind,
    caption,
    cycle,
    show,
    use,
    importFile,
    record,
    addFile,
    target,
    requireTarget,
    timing,
    closed,
    stopPreview,
    get isBusy() {
      return pending;
    }
  };
})();
