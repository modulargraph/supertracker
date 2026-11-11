/* Keyboard-accessible review of missing project media and replacement proposals. */
(() => {
  'use strict';
  let currentSession = null;
  const app = () => window.ST;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const { clone } = window.STUtil;
  const fingerprint = value => JSON.stringify(value);
  const active = session => currentSession === session && app().state.project === session.project && app().state.modal === 'media-relink';
  const referenceName = reference => reference.label || reference.path?.split('/').pop() || reference.id;
  const selectedReference = session => session.references.find(reference => reference.id === session.selectedID);
  const prettySize = size => Number.isFinite(Number(size))
      ? Number(size) >= 1048576 ? `${(Number(size) / 1048576).toFixed(1)} MB` : `${Math.round(Number(size) / 1024)} KB`
      : '';
  const eligible = selection => !!selection?.to && (!selection.contentMismatch || selection.allowDifferentContent);
  function status(session, message, error = false) {
    session.message = message; session.error = error;
    if (active(session)) {
      const element = document.getElementById('media-relink-status');
      if (element) { element.textContent = message; element.classList.toggle('is-error', error); }
    }
  }
  function render(session) {
    if (!active(session)) return;
    const focused = document.activeElement?.id;
    const cursor = document.activeElement?.selectionStart;
    const scroll = document.getElementById('media-reference-list')?.scrollTop || 0;
    const body = renderBody(session);
    const count = [...session.selections.values()].filter(eligible).length;
    const footer = `<button id="media-relink-close">Close</button><button id="media-relink-inspect" ${session.busy ? 'disabled' : ''}>Check again</button>`
        + `<button id="media-relink-apply" class="primary" ${session.busy || !count ? 'disabled' : ''}>Apply ${count || ''} replacement${count === 1 ? '' : 's'}</button>`;
    const modal = document.querySelector('.media-relink-modal');
    if (modal?.querySelector('.modal-body')) {
      // Keep the dialog itself alive so native focus trapping and keyboard position survive updates.
      const top = modal.scrollTop;
      modal.querySelector('.modal-body').innerHTML = body;
      let foot = modal.querySelector('.modal-foot');
      if (!foot) { foot = document.createElement('footer'); foot.className = 'modal-foot'; modal.appendChild(foot); }
      foot.innerHTML = footer; modal.scrollTop = top;
    } else app().openModal('Relink project media', body, footer, { kind: 'media-relink', className: 'media-relink-modal' });
    bind(session);
    const list = document.getElementById('media-reference-list'); if (list) list.scrollTop = scroll;
    if (focused) {
      const next = document.getElementById(focused); next?.focus({ preventScroll: true });
      if (typeof cursor === 'number' && next?.setSelectionRange) { try { next.setSelectionRange(cursor, cursor); } catch {} }
    }
  }
  function renderBody(session) {
    const missing = session.references.length;
    return `<div class="media-relink-content"><div class="media-relink-summary"><div><strong>${session.inspectDone
        ? `${missing} missing · ${session.presentCount} available`
        : 'Checking audio references…'}</strong>`
        + `<p>Replacements update this project’s file references. Source files are not modified.</p></div><span class="media-relink-count">Samples `
        + `· zones · audio clips</span></div>
      <details class="media-search-settings" ${missing || !session.inspectDone ? 'open' : ''}><summary>Search locations and relocated folders</summary>`
          + `<div class="media-search-settings-body"><div class="media-roots-head"><span>Search these folders</span><button id="media-root-refresh"`
          + ` ${session.busy ? 'disabled' : ''}>Refresh locations</button></div><div class="media-search-roots">${session.roots.length ? session.roots.map((root, index) => `<label><input type="checkbox" data-media-root="${index}" ${session.rootSelection.has(root.path) ? 'checked' : ''} ${root.available === false || session.busy ? 'disabled' : ''}>`
          + `<span><strong>${esc(root.name || root.path.split('/').pop())}</strong><small>${esc(root.path)}${root.available === false ? ' · unavailable' : ''}</small>`
          + `</span></label>`).join('') : '<p>No known locations. Add an absolute folder path below.</p>'}</div>`
          + `<div class="media-add-root"><label class="field"><span>Additional search folder</span><input id="media-new-root" value="${esc(session.newRoot)}"`
          + ` placeholder="/Volumes/Sound Drive/Samples" ${session.busy ? 'disabled' : ''}></label><button id="media-add-root" ${session.busy ? 'disabled' : ''}>Add `
          + `folder</button></div>
      <div class="media-relocations"><div class="media-roots-head"><span>Folder relocation rules</span><button id="media-add-relocation"`
          + ` ${session.busy ? 'disabled' : ''}>Add rule</button></div>${session.relocations.length ? session.relocations.map((rule, index) => `<div class="media-relocation-row"><label class="field"><span>Previous folder</span><input data-media-relocation="${index}"`
          + ` data-media-part="from" value="${esc(rule.from)}" placeholder="/Volumes/OldDrive/Library" ${session.busy ? 'disabled' : ''}>`
          + `</label><span class="media-relocation-arrow" aria-hidden="true">→</span><label class="field"><span>Current folder</span>`
          + `<input data-media-relocation="${index}" data-media-part="to" value="${esc(rule.to)}" placeholder="/Volumes/NewDrive/Library"`
          + ` ${session.busy ? 'disabled' : ''}></label><button data-media-remove-relocation="${index}" aria-label="Remove relocation rule ${index + 1}"`
          + ` ${session.busy ? 'disabled' : ''}>×</button></div>`).join('') : '<p class="media-relink-help">Use a relocation rule when a drive or parent folder was renamed.</p>'}</div>
      <div class="media-search-actions"><span>Search finds candidates. You choose each replacement before applying.</span>`
          + `<button id="media-search" ${session.busy
              || !missing
              || (!session.rootSelection.size && !session.relocations.some(rule => rule.from && rule.to))
              ? 'disabled'
              : ''}>${session.busy === 'search' ? 'Searching…' : 'Find replacement files'}</button>`
          + `</div></div></details>
      ${missing ? `<div class="media-relink-review"><section class="media-reference-pane"><label class="media-reference-filter">`
          + `<span class="sr-only">Filter missing references</span><input id="media-reference-filter" type="search" placeholder="Filter missing files…"`
          + ` value="${esc(session.query)}"></label><div class="media-reference-list" id="media-reference-list" role="listbox" tabindex="0"`
          + ` aria-label="Missing media" ${session.selectedID
              ? `aria-activedescendant="media-reference-${session.references.findIndex(reference => reference.id === session.selectedID)}"`
              : ''}>${session.references.filter(reference => `${referenceName(reference)} ${reference.path} ${reference.kind}`.toLowerCase().includes(session.query.toLowerCase())).map(reference => { const index = session.references.indexOf(reference), selection = session.selections.get(reference.id), candidates = reference.candidates || []; return `<button id="media-reference-${index}" class="media-reference ${reference.id === session.selectedID ? 'selected' : ''}" data-media-reference="${index}"`
          + (` role="option" aria-selected="${reference.id === session.selectedID}"><span class="media-reference-top">`
          + `<strong>${esc(referenceName(reference))}</strong>`)
          + `<small>${esc(reference.kind || 'audio')}</small></span><span class="media-reference-path">${esc(reference.path)}</span>`
          + `<span class="media-reference-status">${eligible(selection)
              ? 'Replacement selected'
              : selection
              ? 'Content choice needed'
              : candidates.length
              ? `${candidates.length} candidate${candidates.length === 1 ? '' : 's'}`
              : 'No replacement selected'}</span>`
          + `</button>`; }).join('')}</div>`
          + `</section><section class="media-candidate-pane">${renderCandidates(session)}</section></div>` : session.inspectDone ? '<div class="media-relink-complete"><strong>All referenced media is available.</strong><p>Reconnect an external drive before checking again '
          + 'if you want to verify its files.</p></div>' : ''}
      <div id="media-relink-status" class="media-relink-status ${session.error ? 'is-error' : ''}" role="status">${esc(session.message || '')}</div>${session.warnings.length
          ? `<details class="media-relink-warnings"><summary>${session.warnings.length} inspection note${session.warnings.length === 1 ? '' : 's'}</summary>${session.warnings.map(warning => `<p>${esc(typeof warning === 'string' ? warning : warning.message || JSON.stringify(warning))}</p>`).join('')}</details>`
          : ''}</div>`;
  }
  function renderCandidates(session) {
    const reference = selectedReference(session);
    if (!reference) return '<p class="media-relink-help">Select a missing file to review candidates or choose a replacement.</p>';
    const selection = session.selections.get(reference.id);
    const candidates = reference.candidates || [];
    return `<div class="media-candidate-heading"><h3>${esc(referenceName(reference))}</h3><p>${esc(reference.path)}</p>${reference.expectedSHA256
        ? '<span class="media-match-badge">Project has a SHA-256 fingerprint</span>'
        : reference.expectedHashPrefix
        ? '<span class="media-match-badge">Project has a hash prefix</span>'
        : '<span class="media-match-badge">No stored content fingerprint</span>'}</div>`
        + `<fieldset class="media-candidate-list"><legend>Replacement candidates</legend>${candidates.length ? candidates.map((candidate, index) => `<label class="media-candidate ${selection?.to === candidate.path ? 'selected' : ''}"><input type="radio" name="media-candidate"`
        + ` id="media-candidate-${index}" data-media-candidate="${index}" ${selection?.to === candidate.path ? 'checked' : ''} ${session.busy ? 'disabled' : ''}>`
        + `<span><strong>${esc(candidate.path.split('/').pop())}</strong><span class="media-candidate-path">${esc(candidate.path)}</span>`
        + `<span class="media-candidate-meta"><span class="media-match-badge ${candidate.verified ? 'verified' : ''}">${esc(candidate.contentMismatch
            ? 'Different content'
            : candidate.verified
            ? candidate.match === 'hash-prefix' ? 'Verified hash prefix' : 'Verified content'
            : candidate.match === 'relocation'
            ? 'Relocated path · unverified'
            : 'Name match · unverified')}</span>${candidate.size != null ? `<span>${prettySize(candidate.size)}</span>` : ''}</span>${candidate.reasons?.length ? `<small>${candidate.reasons.map(esc).join(' · ')}</small>` : ''}</span>`
        + `</label>`).join('') : `<p class="media-relink-help">${session.searchDone
            ? 'No candidates found in the selected locations.'
            : 'Search the selected locations or choose the replacement directly.'}</p>`}</fieldset>`
        + `<div class="media-candidate-browse"><button id="media-browse" ${session.busy ? 'disabled' : ''}>Choose replacement file…</button>${selection ? '<button id="media-clear-selection">Clear selection</button>' : ''}</div>${selection ? `<div class="media-selected-file"><strong>Selected replacement</strong><p>${esc(selection.to)}</p>${selection.verified ? '<span class="media-match-badge verified">Content verified</span>' : selection.contentMismatch ? `<label class="media-allow-content"><input type="checkbox" id="media-allow-content" ${selection.allowDifferentContent ? 'checked' : ''}`
        + ` ${session.busy ? 'disabled' : ''}><span>Use this file despite the saved content mismatch</span></label><p class="media-relink-help">This `
        + `replacement changes the audio associated with this reference.</p>` : '<span class="media-match-badge">Unverified content</span><p class="media-relink-help">The chosen file will be checked against any saved '
        + 'fingerprint when applied.</p>'}</div>` : ''}`;
  }
  async function inspect(session) {
    if (!active(session)) return;
    session.busy = 'inspect'; status(session, 'Checking project audio references…'); render(session);
    try {
      const result = await app().request('project.media.inspect', { project: clone(session.project) });
      if (!active(session)) return;
      session.references = result.missing; session.presentCount = Number(result.presentCount || 0); session.inspectDone = true;
      session.warnings = result.warnings || [];
      session.selectedID = session.references.some(reference => reference.id === session.selectedID)
          ? session.selectedID
          : session.references[0]?.id || '';
      const remaining = new Set(session.references.map(reference => reference.id));
      for (const id of session.selections.keys()) if (!remaining.has(id)) session.selections.delete(id);
      status(session, session.references.length ? `${session.references.length} missing reference${session.references.length === 1 ? '' : 's'} need a replacement or a reconnected `
          + `drive.` : 'Every referenced audio file is available.');
    } catch (error) { if (active(session)) status(session, error.message || String(error), true); }
    finally { session.busy = ''; if (active(session)) render(session); }
  }
  async function refreshRoots(session) {
    try {
      const result = await app().request('library.roots', {});
      if (!active(session)) return;
      const roots = Array.isArray(result) ? result : result?.roots || [];
      const existing = new Set(session.roots.map(root => root.path));
      for (const root of roots) {
        if (!existing.has(root.path)) { session.roots.push(root); existing.add(root.path); }
        else Object.assign(session.roots.find(value => value.path === root.path), root);
        if (root.available === false) session.rootSelection.delete(root.path);
        else if (!session.rootsInitialized) session.rootSelection.add(root.path);
      }
      session.rootsInitialized = true; render(session);
    } catch (error) { if (active(session)) status(session, `Could not read known search folders: ${error.message || error}`, true); }
  }
  async function search(session) {
    if (!active(session) || session.busy) return;
    const relocations = session.relocations.filter(rule => rule.from.trim() && rule.to.trim()).map(rule => ({
      from: rule.from.trim(),
      to: rule.to.trim()
    }));
    session.busy = 'search'; status(session, 'Searching for candidate files…'); render(session);
    try {
      const result = await app().request('project.media.search', { project: clone(session.project), roots: [...session.rootSelection], relocations });
      if (!active(session)) return;
      if (result.cancelled) { status(session, 'Search canceled. Existing selections are unchanged.'); return; }
      const found = result.missing;
      for (const reference of session.references) {
        const update = found.find(candidate => candidate.id === reference.id); if (update) Object.assign(reference, update);
      }
      session.searchDone = true; session.warnings = result.warnings || [];
      const candidates = session.references.reduce((count, reference) => count + (reference.candidates?.length || 0), 0);
      status(session, `${candidates} candidate${candidates === 1 ? '' : 's'} found. Review and select replacements before applying.`);
    } catch (error) { if (active(session)) status(session, error.message || String(error), true); }
    finally { session.busy = ''; if (active(session)) render(session); }
  }
  async function browse(session) {
    const reference = selectedReference(session); if (!reference || session.busy || !active(session)) return;
    const referenceID = reference.id, originalPath = reference.path;
    try {
      const result = await app().request('dialog.openSample', {});
      if (!active(session)) return;
      const path = typeof result === 'string' ? result : result?.path; if (!path || result?.cancelled) return;
      const stillCurrent = session.references.find(value => value.id === referenceID && value.path === originalPath); if (!stillCurrent) return;
      session.selections.set(referenceID, {
        id: referenceID,
        from: originalPath,
        to: path,
        verified: false,
        contentMismatch: false,
        allowDifferentContent: false,
        manual: true
      });
      session.selectedID = referenceID; render(session); document.getElementById('media-allow-content')?.focus();
    } catch (error) { if (active(session)) status(session, error.message || String(error), true); }
  }
  async function apply(session) {
    if (!active(session) || session.busy) return;
    const selections = [...session.selections.values()].filter(eligible).map(selection => ({
      id: selection.id,
      from: selection.from,
      to: selection.to,
      ...(selection.allowDifferentContent ? { allowDifferentContent: true } : {})
    }));
    if (!selections.length) return;
    const before = fingerprint(session.project);
    session.busy = 'apply'; status(session, 'Validating and applying selected replacements…'); render(session);
    try {
      const result = await app().request('project.media.apply', { project: clone(session.project), selections });
      // Closing this dialog does not undo an explicitly requested apply. A project change does.
      if (app().state.project !== session.project) {
        await app().syncEngineProject(() => clone(app().state.project));
        return;
      }
      if (!result?.project) throw new Error('The media service did not return an updated project.');
      if (fingerprint(session.project) !== before) {
        await app().syncEngineProject(() => clone(app().state.project));
        throw new Error('The session changed while replacements were being applied. Check the references again before applying.');
      }
      const replacement = app().normalize(clone(result.project));
      app().state.samples = {};
      app().mutate('relink project media', project => {
        for (const key of Object.keys(project)) delete project[key];
        Object.assign(project, replacement);
      });
      if (!active(session)) return;
      session.selections.clear(); session.warnings = result.warnings || [];
      const count = result.relinked.length;
      app().toast(`${count} media reference${count === 1 ? '' : 's'} relinked.`, 'success');
      session.busy = ''; await inspect(session);
    } catch (error) {
      const message = error.message || String(error);
      for (const reference of session.references) {
        if (message.startsWith(`The selected file for ${reference.label} does not match its saved content fingerprint`)) {
          const selection = session.selections.get(reference.id);
          if (selection) { selection.contentMismatch = true; selection.allowDifferentContent = false; }
        }
      }
      if (active(session)) status(session, message, true); else app().toast(message, 'error');
    }
    finally { session.busy = ''; if (active(session)) render(session); }
  }
  function bind(session) {
    const on = (id, type, fn) => document.getElementById(id)?.addEventListener(type, fn);
    on('media-relink-close', 'click', () => { currentSession = null; app().closeModal(); });
    on('media-relink-inspect', 'click', () => inspect(session)); on('media-root-refresh', 'click', () => refreshRoots(session));
    on('media-search', 'click', () => search(session));
    on('media-relink-apply', 'click', () => apply(session));
    on('media-browse', 'click', () => browse(session));
    on('media-reference-filter', 'input', event => {
      session.query = event.target.value;
      const visible = session.references.filter(reference => `${referenceName(reference)} ${reference.path} ${reference.kind}`.toLowerCase().includes(session.query.toLowerCase()));
      if (!visible.some(reference => reference.id === session.selectedID)) session.selectedID = visible[0]?.id || '';
      render(session);
    });
    on('media-new-root', 'input', event => { session.newRoot = event.target.value; });
    on('media-add-root', 'click', () => {
      const path = session.newRoot.trim(); if (!path) return;
      if (!path.startsWith('/')) { status(session, 'Enter an absolute folder path, such as /Volumes/Sound Drive/Samples.', true); return; }
      if (!session.roots.some(root => root.path === path)) session.roots.push({
        path,
        name: path.split('/').filter(Boolean).pop() || '/',
        available: true
      });
      session.rootSelection.add(path); session.newRoot = ''; render(session);
    });
    document.querySelectorAll('[data-media-root]').forEach(input => input.addEventListener('change', () => {
      const path = session.roots[Number(input.dataset.mediaRoot)].path;
      if (input.checked) {
        session.rootSelection.add(path);
      } else {
        session.rootSelection.delete(path);
      }
      render(session);
    }));
    on('media-add-relocation', 'click', () => {
      session.relocations.push({ from: '', to: '' });
      render(session);
      document.querySelector(`[data-media-relocation="${session.relocations.length - 1}"]`)?.focus();
    });
    document.querySelectorAll('[data-media-relocation]').forEach(input => input.addEventListener('input', () => {
      session.relocations[Number(input.dataset.mediaRelocation)][input.dataset.mediaPart] = input.value;
      const button = document.getElementById('media-search');
      if (button) button.disabled = !!session.busy
          || !session.references.length
          || !session.rootSelection.size && !session.relocations.some(rule => rule.from && rule.to);
    }));
    document.querySelectorAll('[data-media-remove-relocation]').forEach(button => button.addEventListener('click', () => {
      session.relocations.splice(Number(button.dataset.mediaRemoveRelocation), 1);
      render(session);
    }));
    document.querySelectorAll('[data-media-reference]').forEach(button => button.addEventListener('click', () => {
      session.selectedID = session.references[Number(button.dataset.mediaReference)].id;
      render(session);
      document.getElementById(button.id)?.focus();
    }));
    document.querySelectorAll('[data-media-candidate]').forEach(input => input.addEventListener('change', () => {
      const reference = selectedReference(session), candidate = reference?.candidates?.[Number(input.dataset.mediaCandidate)]; if (!candidate) return;
      session.selections.set(reference.id, {
        id: reference.id,
        from: reference.path,
        to: candidate.path,
        verified: candidate.verified === true,
        contentMismatch: candidate.contentMismatch === true,
        allowDifferentContent: false
      });
      render(session); document.getElementById(input.id)?.focus();
    }));
    on('media-allow-content', 'change', event => {
      const selection = session.selections.get(session.selectedID);
      if (selection) selection.allowDifferentContent = event.target.checked;
      render(session);
    });
    on('media-clear-selection', 'click', () => { session.selections.delete(session.selectedID); render(session); });
    on('media-reference-list', 'keydown', event => {
      if (!['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const filtered = session.references.filter(reference => `${referenceName(reference)} ${reference.path} ${reference.kind}`.toLowerCase().includes(session.query.toLowerCase()));
      const index = filtered.findIndex(reference => reference.id === session.selectedID);
      const next = event.key === 'Home'
          ? 0
          : event.key === 'End'
          ? filtered.length - 1
          : Math.max(0, Math.min(filtered.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)));
      if (filtered[next]) {
        session.selectedID = filtered[next].id;
        render(session);
        const node = document.getElementById(`media-reference-${session.references.indexOf(filtered[next])}`);
        node?.focus({ preventScroll: true });
        node?.scrollIntoView({ block: 'nearest' });
      }
    });
  }
  async function open() {
    if (!app()?.state?.project) return;
    const session = {
      project: app().state.project,
      references: [],
      presentCount: 0,
      roots: [],
      rootSelection: new Set(),
      rootsInitialized: false,
      relocations: [],
      selections: new Map(),
      selectedID: '',
      query: '',
      newRoot: '',
      warnings: [],
      busy: '',
      message: '',
      error: false,
      inspectDone: false,
      searchDone: false
    };
    currentSession = session;
    app().openModal('Relink project media', '<div class="media-relink-status">Checking project audio references…</div>', '', {
      kind: 'media-relink',
      className: 'media-relink-modal'
    });
    await Promise.allSettled([inspect(session), refreshRoots(session)]);
  }
  function event(type, payload = {}) {
    const session = currentSession; if (!session || !active(session)) return;
    if (type === 'project.media.progress' && session.busy === 'search') status(session,
        payload.message
            || `Searching${payload.root ? ` ${payload.root}` : ''}${Number.isFinite(payload.scanned) ? ` · ${payload.scanned} files checked` : ''}…`);
  }
  window.STMedia = { open, event };
})();
