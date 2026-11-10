/* Shared helpers for every web module (window.STUtil; also require()-able for node tests).
   clone() is the one deep copy for project data: plain JSON values only, as the bridge sends them. */
(function (root) {
  'use strict';

  const clone = value => JSON.parse(JSON.stringify(value));
  const clamp = (value, low, high) => Math.max(low, Math.min(high, value));
  const escapeHTML = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

  const STUtil = { clone, clamp, escapeHTML };
  if (typeof module === 'object' && module.exports) module.exports = STUtil;
  if (root) root.STUtil = STUtil;
})(typeof window !== 'undefined' ? window : null);
