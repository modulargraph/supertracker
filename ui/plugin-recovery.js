/* Explicit review of plug-ins associated with an interrupted previous session. */
(() => {
  'use strict';
  let current = null;
  const app = () => window.ST;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const pending = report => (Array.isArray(report?.suspects) ? report.suspects : []).filter(item => item.requiresReview === true && typeof item.identity?.key === 'string' && item.identity.key);
  // owns: the open dialog is this session's. active: and its project is still the one it was opened for.
  const owns = session => current === session && app().state.modal === 'plugin-recovery';
  const active = session => owns(session) && app().state.project === session.project;
  // A dialog whose project was replaced (undo, reload) is stale: close it rather than leave it inert.
  const settle = session => { if (active(session)) render(session); else if (owns(session)) app().closeModal(); };
  const name = item => item.identity.name || item.identity.componentID || 'Unknown plug-in';
  const notes = values => (Array.isArray(values) ? values : []).map(value => typeof value === 'string' ? value : value?.message || '').filter(Boolean);
  function setStatus(session, text, error = false) {
    session.message = text; session.error = error;
    if (!active(session)) return;
    const node = document.getElementById('plugin-recovery-status');
    if (node) { node.textContent = text; node.classList.toggle('is-error', error); }
  }
  function updateControls(session) {
    if (!active(session)) return;
    const button = document.getElementById('plugin-recovery-retry');
    if (button) { button.disabled = session.busy || !session.selected.size; button.textContent = session.busy ? 'Retrying…' : `Retry selected${session.selected.size ? ` (${session.selected.size})` : ''}`; }
    document.querySelectorAll('[data-plugin-recovery-key], [data-plugin-recovery-enable]').forEach(input => { input.disabled = session.busy; });
    document.getElementById('plugin-recovery-list')?.setAttribute('aria-busy', String(session.busy));
  }
  function render(session, focusID = 'plugin-recovery-keep') {
    if (!active(session)) return;
    const suspects = pending(session.report), warnings = notes(session.report?.warnings), errors = session.retryErrors || [];
    const body = `<div class="plugin-recovery-content">
      <p class="plugin-recovery-intro">${session.loading ? 'Checking the previous session…' : !session.report ? 'The recovery report is unavailable.' : suspects.length ? 'The previous session ended unexpectedly. These plug-ins did not finish loading or releasing. Involvement is suspected, not proven.' : 'No plug-ins are waiting for review.'}</p>
      ${suspects.length ? `<p class="plugin-recovery-preserved">Skipped plug-ins remain in your project with their settings.</p><fieldset class="plugin-recovery-list" id="plugin-recovery-list" aria-busy="${session.busy}"><legend>Select plug-ins to retry</legend>${suspects.map((item, index) => `<div class="plugin-recovery-row"><label for="plugin-recovery-choice-${index}"><input type="checkbox" id="plugin-recovery-choice-${index}" data-plugin-recovery-key="${esc(item.identity.key)}" ${session.selected.has(item.identity.key) ? 'checked' : ''} ${session.busy ? 'disabled' : ''}><span class="plugin-recovery-name">${esc(name(item))}</span><span class="plugin-recovery-format">${esc(item.identity.format || 'Plug-in')}</span></label><button id="plugin-recovery-enable-${index}" data-plugin-recovery-enable="${esc(item.identity.key)}" ${session.busy ? 'disabled' : ''}>Enable / Retry</button>${notes(item.reasons).length || item.identity.bundlePath ? `<details><summary>Why listed</summary>${notes(item.reasons).map(reason => `<p>${esc(reason)}</p>`).join('')}${item.identity.bundlePath ? `<p class="plugin-recovery-path">${esc(item.identity.bundlePath)}</p>` : ''}</details>` : ''}</div>`).join('')}</fieldset><p class="plugin-recovery-keys">↑ ↓ move · Space select · ⌘ Return retry selected</p>` : ''}
      <div id="plugin-recovery-status" class="plugin-recovery-status ${session.error ? 'is-error' : ''}" role="status" aria-live="polite">${esc(session.message || '')}</div>
      ${errors.length ? `<div class="plugin-recovery-errors">${errors.map(error => `<p><strong>${esc(error.name || 'Plug-in')}</strong> ${esc(error.message || 'Could not be restored.')}</p>`).join('')}</div>` : ''}
      ${warnings.length ? `<details class="plugin-recovery-notes"><summary>Session notes (${warnings.length})</summary>${warnings.map(warning => `<p>${esc(warning)}</p>`).join('')}</details>` : ''}</div>`;
    const footer = `<button id="plugin-recovery-keep">${suspects.length ? 'Keep disabled' : 'Close'}</button>${suspects.length ? `<button id="plugin-recovery-retry" class="primary" ${session.busy || !session.selected.size ? 'disabled' : ''}>${session.busy ? 'Retrying…' : `Retry selected${session.selected.size ? ` (${session.selected.size})` : ''}`}</button>` : ''}`;
    app().openModal('Plug-in recovery', body, footer, { kind: 'plugin-recovery', className: 'plugin-recovery-modal' });
    bind(session);
    setTimeout(() => { if (active(session)) document.getElementById(focusID)?.focus(); }, 0);
  }
  function bind(session) {
    document.getElementById('plugin-recovery-keep')?.addEventListener('click', () => { if (owns(session)) app().closeModal(); });
    document.getElementById('plugin-recovery-retry')?.addEventListener('click', () => retry(session));
    document.querySelectorAll('[data-plugin-recovery-enable]').forEach(button => {
      button.addEventListener('click', () => retry(session, [button.dataset.pluginRecoveryEnable]));
    });
    const choices = Array.from(document.querySelectorAll('[data-plugin-recovery-key]'));
    choices.forEach((input, index) => {
      input.addEventListener('change', () => {
        if (!active(session) || session.busy) return;
        if (input.checked) session.selected.add(input.dataset.pluginRecoveryKey); else session.selected.delete(input.dataset.pluginRecoveryKey);
        updateControls(session);
      });
      input.addEventListener('keydown', event => {
        if (!active(session) || session.busy || event.metaKey || event.ctrlKey || !['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
        event.preventDefault();
        const next = event.key === 'Home' ? 0 : event.key === 'End' ? choices.length - 1 : Math.max(0, Math.min(choices.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)));
        choices[next]?.focus();
      });
    });
    document.querySelector('.plugin-recovery-modal')?.addEventListener('keydown', event => {
      if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') { event.preventDefault(); event.stopPropagation(); retry(session); }
    });
  }
  async function retry(session, requestedKeys = [...session.selected]) {
    if (!active(session) || session.busy) return;
    const allowed = new Set(pending(session.report).map(item => item.identity.key));
    const keys = requestedKeys.filter(key => allowed.has(key));
    if (!keys.length) return;
    session.busy = true; updateControls(session); setStatus(session, `Retrying ${keys.length} plug-in${keys.length === 1 ? '' : 's'}…`);
    try {
      const report = await app().request('plugins.recovery.retry', { keys });
      if (!active(session)) return;
      if (!report || !Array.isArray(report.suspects)) throw new Error('The host did not return a recovery report. Check recovery again before retrying.');
      session.report = report; session.retryErrors = Array.isArray(report.retryErrors) ? report.retryErrors : []; session.selected.clear();
      if (!session.retryErrors.length && !pending(report).length) {
        app().closeModal(); app().toast(`Retried ${keys.length} plug-in${keys.length === 1 ? '' : 's'}.`, 'success'); return;
      }
      setStatus(session, session.retryErrors.length ? `${session.retryErrors.length} plug-in${session.retryErrors.length === 1 ? '' : 's'} could not be restored. Their saved settings are preserved.` : `Retried ${keys.length} plug-in${keys.length === 1 ? '' : 's'}. ${pending(report).length} remain disabled.`, session.retryErrors.length > 0);
    } catch (error) { if (active(session)) setStatus(session, error.message || String(error), true); }
    finally { session.busy = false; settle(session); }
  }
  async function show(report) {
    const session = { project: app().state.project, report: report || null, selected: new Set(), busy: false, loading: !report, retryErrors: [], message: '', error: false };
    current = session;
    // Set ownership before render, while still using the shared modal's focus return and trap.
    app().openModal('Plug-in recovery', '<p class="plugin-recovery-loading">Checking the previous session…</p>', '<button id="plugin-recovery-keep">Close</button>', { kind: 'plugin-recovery', className: 'plugin-recovery-modal' });
    bind(session);
    if (report) { session.loading = false; render(session); return report; }
    try {
      const result = await app().request('plugins.recovery', {});
      if (!active(session)) return null;
      if (!result || !Array.isArray(result.suspects)) throw new Error('The host did not return a recovery report.');
      session.report = result; return result;
    } catch (error) { if (active(session)) setStatus(session, error.message || String(error), true); return null; }
    finally { session.loading = false; settle(session); }
  }
  window.STPluginRecovery = { show };
})();
