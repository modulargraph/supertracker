/* Shared helpers for every web module (window.STUtil; also require()-able for node tests).
   clone() is the one deep copy for project data: plain JSON values only, as the bridge sends them. */
(function (root) {
  'use strict';

  const clone = value => JSON.parse(JSON.stringify(value));
  const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
  const escapeHTML = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

  /* The length in pattern rows of a step that has no gate key (app-project-model.js says what a gate is;
     DEFAULT_GATE in crates/st-project/src/model/pattern.rs is the same number). It is defined here because the
     controller's screens (mk3-screens.js) show a gate without the app's modules. */
  const DEFAULT_GATE = .8;

  const STUtil = { clone, clamp, escapeHTML, DEFAULT_GATE };
  if (typeof module === 'object' && module.exports) module.exports = STUtil;
  if (root) root.STUtil = STUtil;
})(typeof window !== 'undefined' ? window : null);
