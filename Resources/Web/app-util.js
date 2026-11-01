/* Shared helpers for the app modules: DOM lookup, escaping, note names, toasts and the
   window.STApp namespace the app-*.js modules share. Modules destructure what earlier modules
   export (index.html order) and reach anything defined later through app.name at call time. */
(function () {
  'use strict';

  const app = window.STApp = {};
  const $ = (q, root = document) => root.querySelector(q);
  const $$ = (q, root = document) => Array.from(root.querySelectorAll(q));
  const { clone, clamp, escapeHTML: esc } = window.STUtil;
  const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 7);
  /* Tracks have no product limit. One generous sanity bound guards runaway growth; it matches the engine's
     ST_TRACK_SANITY_LIMIT and the Swift validators. Other modules read window.TRACK_SANITY_LIMIT
     (test harnesses that stub app.js define it on their window). */
  const TRACK_SANITY_LIMIT = 1024;
  const trackLimitMessage = () => `This session already has ${TRACK_SANITY_LIMIT} tracks.`;
  window.TRACK_SANITY_LIMIT = TRACK_SANITY_LIMIT;


  const noteNames = ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'];

  const noteText = n => n === -1
      ? 'OFF'
      : n === -2
      ? 'CUT'
      : n == null || n === -3
      ? '···'
      : noteNames[n % 12].padEnd(2, '–') + (Math.floor(n / 12) - 1);

  const colors = ['#bba374', '#a0adb1', '#b0af89', '#c58b69', '#b79583', '#9da688', '#94a39a', '#aaa0ad'];
  let toastTimer;

  function toast(message, type = 'info') {
    clearTimeout(toastTimer);
    $('#toast-region').innerHTML = `<div class="toast ${esc(type)}">${esc(message)}</div>`;
    toastTimer = setTimeout(() => {
      $('#toast-region').innerHTML = '';
    }, type === 'error' ? 7000 : 3500);
  }

  function download(data, name, type) {
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([data], { type }));
    a.download = name;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  }

  function stableJSON(value) {
    if (value === null || typeof value !== 'object') return JSON.stringify(value);
    if (Array.isArray(value)) return '[' + value.map(stableJSON).join(',') + ']';
    return '{'
        + Object.keys(value).sort().map(key => JSON.stringify(key) + ':' + stableJSON(value[key])).join(',')
        + '}';
  }

  // Busy refusals (busy.js) are shown as toasts; app.toast may be replaced in tests.
  window.STBusy.setReporter(message => app.toast(message));

  Object.assign(app, {
    $, $$, clone, clamp, uid, TRACK_SANITY_LIMIT, trackLimitMessage, esc, noteNames, noteText, colors, toast,
    download, stableJSON
  });
})();
