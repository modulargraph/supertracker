/* Background preview warming for the sample, NKS and kit browsers (and the MK3
   browser pages, which drive the same modules). After the selection settles it
   asks the native host to decode the previews around it, nearest first and
   biased toward the direction of travel. Each request replaces the previous
   native queue, so jumping elsewhere cancels stale work. */
(() => {
  'use strict';
  const AUDIO = /\.(wav|wave|aif|aiff|aifc|flac|mp3|m4a|caf|ogg|aac)$/i;
  const state = { timer: null, sentKey: '', pendingKey: '', last: new Map(), delay: 90, radius: 3, max: 7 };

  // Indices to warm: selected, then alternating ahead/behind out to `radius`.
  function order(selected, count, direction = 1, radius = state.radius) {
    if (!Number.isInteger(selected) || selected < 0 || selected >= count) return [];
    const result = [selected], ahead = direction < 0 ? -1 : 1;
    for (let distance = 1; distance <= Math.max(1, radius); distance++) {
      for (const index of [selected + ahead * distance, selected - ahead * distance]) if (index >= 0 && index < count) result.push(index);
    }
    return result;
  }
  const audioPath = path => typeof path === 'string' && AUDIO.test(path) ? path : '';

  // Native requests ({path} or {kit}) for the items around `selected`. Offline
  // items and items without a usable preview are skipped, never retried here.
  function plan({ items, selected, direction = 1, radius = state.radius, entry }) {
    if (!Array.isArray(items) || typeof entry !== 'function') return [];
    const requests = [], seen = new Set();
    for (const index of order(selected, items.length, direction, radius)) {
      const item = items[index];
      if (!item || item.available === false) continue;
      const request = entry(item);
      if (!request) continue;
      const path = audioPath(request.path), kit = !path && typeof request.kit === 'string' ? request.kit : '';
      const key = path || (kit && 'kit:' + kit);
      if (!key || seen.has(key)) continue;
      seen.add(key); requests.push(path ? { path } : { kit });
      if (requests.length >= state.max) break;
    }
    return requests;
  }

  function send(requests) {
    const key = JSON.stringify(requests);
    if (key === state.sentKey) return;
    state.sentKey = key;
    const request = window.ST?.request;
    if (typeof request !== 'function') return;
    try { Promise.resolve(request.call(window.ST, 'sample.prefetchPreviews', { items: requests })).catch(() => {}); } catch {}
  }

  // owner: a stable list name ('samples', 'nks', 'kits'); options as for plan().
  function schedule(owner, options = {}) {
    const previous = state.last.get(owner);
    const selected = Number(options.selected);
    const direction = options.direction ?? (previous && previous.list === options.items && selected < previous.selected ? -1 : 1);
    state.last.set(owner, { selected, list: options.items });
    const requests = plan({ ...options, selected, direction });
    const key = JSON.stringify(requests);
    if (key === state.sentKey || key === state.pendingKey) return requests;
    clearTimeout(state.timer); state.pendingKey = key;
    state.timer = setTimeout(() => { state.timer = null; state.pendingKey = ''; send(requests); }, state.delay);
    return requests;
  }

  // Drops pending work and tells the host to abandon its queue.
  function cancel() {
    clearTimeout(state.timer); state.timer = null; state.pendingKey = '';
    if (state.sentKey && state.sentKey !== '[]') send([]);
  }

  window.STPreviewPrefetch = { order, plan, schedule, cancel, audioPath, _state: state };
})();
