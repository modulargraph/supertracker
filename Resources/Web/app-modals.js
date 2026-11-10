/* Modal dialogs: one overlay at a time, focus return, text prompts and confirmations. */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, stopHeld } = app;
  const busy = window.STBusy;

  function openModal(title, body, footer = '', options = {}) {
    if (busy.refuse('dialog', ['inlineInput', 'lineInputs'])) return false;
    app.modalIntent++;
    if (busy.refuse('dialog', ['settings', 'export'])) return;
    if (state.modal === 'line-inputs') window.STLineInputs?.closed?.();
    if (state.modal === 'clip-takes') window.STClipTakes?.closed?.();
    if (state.modal === 'export' && options.kind !== 'export') window.STExport?.closed?.();
    if (state.modal === 'recording' && options.kind !== 'recording') {
      if (busy.refuse('dialog', ['recording'])) return;
      window.STRecording?.closed();
    }
    if (!state.modal) {
      const origin = document.activeElement;
      modalReturnFocus = origin && origin !== document.body ? { node: origin, id: origin.id } : null;
    }
    if (app.modalDismiss) {
      const dismiss = app.modalDismiss;
      app.modalDismiss = null;
      dismiss();
    }
    stopHeld();
    state.modal = options.kind || 'dialog';
    $('#overlay').innerHTML = `<div class="modal-scrim"><section class="modal ${options.className || ''}" role="dialog" aria-modal="true"`
        + ` aria-label="${esc(title)}">${options.noHeader ? '' : `<header class="modal-head"><h2>${esc(title)}</h2><button id="modal-close" class="quiet" aria-label="Close `
        + `dialog">×</button></header>`}<div `
        + `class="${options.noHeader ? '' : 'modal-body'}">${body}</div>${footer ? `<footer class="modal-foot">${footer}</footer>` : ''}</section></div>`;
    if ($('#modal-close')) $('#modal-close').onclick = closeModal;
    $('.modal-scrim').onclick = e => {
      if (e.target.classList.contains('modal-scrim')) closeModal();
    };
    const dialog = $('.modal');
    setTimeout(() => {
      if (!state.modal || $('.modal') !== dialog || dialog?.contains?.(document.activeElement)) return;
      const controls = $$('input:not(:disabled):not([type=hidden]),select:not(:disabled),textarea:not(:disabled),button:not(:disabled),[tabindex="0"]',
          dialog).filter(el => el.tabIndex >= 0 && !el.hidden && el.offsetParent !== null);
      const field = el => /INPUT|SELECT|TEXTAREA/.test(el.tagName);
      const target = controls.find(el => el.closest('.modal-body') && field(el))
          || controls.find(el => el.closest('.modal-body'))
          || controls.find(field)
          // Only text in the body: the default action rather than × (a destructive default waits for its own Tab).
          || controls.find(el => el.closest('.modal-foot') && el.classList.contains('primary') && !el.classList.contains('danger'))
          || controls.find(el => el.closest('.modal-foot'))
          || controls[0];
      target?.focus();
    }, 0);
  }

  function closeModal(options = {}) {
    if (busy.refuse('close', ['lineInputs'])) return false;
    app.modalIntent++;
    if (busy.refuse('close', [
      'settings',
      state.modal === 'export' && 'export',
      state.modal === 'recording' && 'recording'
    ])) return;
    if (state.modal === 'line-inputs') window.STLineInputs?.closed?.();
    if (state.modal === 'clip-takes') window.STClipTakes?.closed?.();
    if (state.modal === 'recording') window.STRecording?.closed();
    if (state.modal === 'export') window.STExport?.closed?.();
    const dismiss = app.modalDismiss,
      origin = modalReturnFocus;
    app.modalDismiss = null;
    modalReturnFocus = null;
    state.modal = null;
    $('#overlay').innerHTML = '';
    const replacement = origin?.id ? document.getElementById(origin.id) : null;
    const target = origin?.node?.isConnected !== false && origin?.node && !origin.node.disabled
        ? origin.node
        : replacement;
    if (target && target.isConnected !== false && !target.disabled && !target.hidden) {
      target.focus({ preventScroll: true });
    } else {
      const fallback = {
        pattern: '#tracker',
        browser: '#browser-search',
        automation: '#automation-search',
        song: '#sequence-list',
        mixer: `#mix-track-${state.track}`,
        instrument: '#main input'
      };
      if (state.view === 'mixer' && window.STStudio?.focusSelectedChannel) {
        window.STStudio.focusSelectedChannel();
      } else {
        $(fallback[state.view] || '#main button')?.focus({ preventScroll: true });
      }
    }
    dismiss?.();
  }

  /* Enter in a text prompt accepts it. Accepting closes the dialog and returns focus to the control that opened it;
     the key is cancelled so its keypress does not press that button and open the prompt again. An Enter that
     confirms an input-method composition belongs to the text. */
  function acceptPromptOnEnter(e) {
    if (e.key !== 'Enter' || e.isComposing) return;
    e.preventDefault();
    $('#prompt-accept').click();
  }

  // Promise dialogs for module flows: resolve with the entered text (or null) / true when accepted; closing the dialog any other way resolves null / false.
  function askText(title, value, accept = 'Save') {
    return new Promise(resolve => {
      let settled = false;
      const finish = result => {
        if (settled) return;
        settled = true;
        app.modalDismiss = null;
        closeModal();
        resolve(result);
      };
      if (openModal(title, `<label class="field"><span>${esc(title)}</span><input id="text-prompt" value="${esc(value)}"`
          + ` maxlength="120"></label>`, `<button id="prompt-cancel">Cancel</button><button id="prompt-accept" class="primary">${esc(accept)}</button>`) === false || !state.modal) {
        resolve(null);
        return;
      }
      app.modalDismiss = () => {
        if (!settled) {
          settled = true;
          resolve(null);
        }
      };
      $('#prompt-cancel').onclick = () => finish(null);
      $('#prompt-accept').onclick = () => {
        const text = $('#text-prompt').value.trim();
        if (text) finish(text);
      };
      $('#text-prompt').onkeydown = e => acceptPromptOnEnter(e);
      setTimeout(() => {
        $('#text-prompt')?.focus();
        $('#text-prompt')?.select?.();
      }, 10);
    });
  }

  function confirmChoice(title, message, accept = 'Continue') {
    return new Promise(resolve => {
      let settled = false;
      const finish = result => {
        if (settled) return;
        settled = true;
        app.modalDismiss = null;
        closeModal();
        resolve(result);
      };
      if (openModal(title, `<p class="muted" style="line-height:1.7">${esc(message)}</p>`, `<button id="confirm-cancel">Cancel</button><button id="confirm-accept" class="primary">${esc(accept)}</button>`, {
        kind: 'confirm'
      }) === false || !state.modal) {
        resolve(false);
        return;
      }
      app.modalDismiss = () => {
        if (!settled) {
          settled = true;
          resolve(false);
        }
      };
      $('#confirm-cancel').onclick = () => finish(false);
      $('#confirm-accept').onclick = () => finish(true);
      setTimeout(() => $('#confirm-accept')?.focus(), 0);
    });
  }

  function textPrompt(title, value, callback) {
    openModal(title, `<label class="field"><span>${esc(title)}</span><input id="text-prompt" value="${esc(value)}"`
        + ` maxlength="120"></label>`, '<button id="prompt-cancel">Cancel</button><button id="prompt-accept" class="primary">Save</button>');
    $('#prompt-cancel').onclick = closeModal;
    $('#prompt-accept').onclick = () => {
      const value = $('#text-prompt').value.trim();
      if (value) {
        closeModal();
        callback(value);
      }
    };
    $('#text-prompt').onkeydown = e => acceptPromptOnEnter(e);
    setTimeout(() => {
      $('#text-prompt').focus();
      $('#text-prompt').select();
    }, 10);
  }

  app.modalDismiss = null;
  let modalReturnFocus = null;
  app.modalIntent = 0;

  Object.assign(app, { openModal, closeModal, askText, confirmChoice, textPrompt });
})();
