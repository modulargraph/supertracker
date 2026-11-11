/* Drag and drop reordering of tracks and groups, in the pattern editor (track and group headers) and the mixer
   (strip titles). A drop between members of an expanded group adds the track to it, a drop outside takes it out;
   a group header moves the whole group. One Undo step per move; notes, instrument, inserts, routing, sends and
   automation follow (STGroups.moveTracks). Esc cancels; dragging near an edge scrolls.
   Also: × delete buttons on mixer strips and context menu items for group headers and mixer strips. */
(function () {
  'use strict';

  const app = window.STApp;
  const { state, mutate, clone, esc } = app;
  const G = () => window.STGroups;
  const THRESHOLD = 5, EDGE = 56, MAX_SPEED = 22;
  let drag = null, suppressClick = false;
  const raf = fn => typeof requestAnimationFrame === 'function' ? requestAnimationFrame(fn) : setTimeout(fn, 16);
  const unraf = id => typeof cancelAnimationFrame === 'function' ? cancelAnimationFrame(id) : clearTimeout(id);

  /* ---------- drop geometry (pure) ---------- */

  /* columns: the drop targets in visual order, { left, right, kind, track?, group? }:
       kind 'track'     a track column / strip (group: its expanded group id or null)
       kind 'overview'  a collapsed group (pattern overview; mixer group strip with hidden members: closed)
       kind 'groupHead' an expanded group's own strip in the mixer (it precedes the members)
       kind 'end'       Master and return strips: a drop there goes after the last track
     what: { tracks: [indices], group: id | null } (group: a whole group is dragged).
     Returns { before, group, x } (insert before track `before`, membership afterwards, indicator x) or null. */
  function dropTarget(p, columns, x, what) {
    const cols = columns.filter(c => c && c.right > c.left);
    if (!cols.length || !what?.tracks?.length) return null;
    const n = p.tracks.length, members = id => G().members(p, id);
    let c = cols.find(col => x >= col.left && x < col.right);
    if (!c) c = x < cols[0].left ? cols[0] : cols[cols.length - 1];
    const after = x >= (c.left + c.right) / 2;
    let before, group = null;
    if (c.kind === 'end') {
      before = n;
    } else if (c.kind === 'overview' || c.kind === 'groupHead') {
      const m = members(c.group);
      if (!m.length) return null;
      if (c.kind === 'groupHead' && after) { before = m[0]; group = c.group; } else before = after ? m[m.length - 1] + 1 : m[0];
    } else {
      before = after ? c.track + 1 : c.track;
      group = c.group || null;
    }
    if (what.group) {
      // A group lands between tracks and groups, never inside another group.
      if (group && group !== what.group) {
        const m = members(group);
        before = before - m[0] <= m[m.length - 1] + 1 - before ? m[0] : m[m.length - 1] + 1;
      }
      group = what.group;
    }
    return { before, group, x: indicatorX(p, cols, before, group) };
  }

  // Where the drop line goes: the left edge of the first column at or after `before`.
  function indicatorX(p, cols, before, group) {
    const first = col => col.kind === 'track' ? col.track : col.kind === 'end' ? Infinity : G().members(p, col.group)[0] ?? Infinity;
    for (const col of cols) {
      if (col.kind === 'groupHead' && col.group === group && first(col) === before) return col.right;
      if (first(col) >= before) return col.left;
    }
    return cols[cols.length - 1].right;
  }

  /* ---------- moving ---------- */

  const signature = p => p.tracks.map(t => `${t.routingID}:${t.groupID || ''}`).join('|');

  /* Moves tracks (or a whole group) to `before` with membership `group`; one Undo step. False when the drop
     changes nothing. The cursor and selection follow the moved tracks. */
  function moveLanes(tracks, before, group = null, options = {}) {
    const p = state.project;
    tracks = [...new Set(tracks)].filter(i => Number.isInteger(i) && p.tracks[i]).sort((a, b) => a - b);
    if (!tracks.length) return false;
    const trial = clone({ tracks: p.tracks, groups: p.groups, routing: p.routing, patterns: [], audioClips: [] });
    G().moveTracks(trial, tracks, before, group);
    if (signature(trial) === signature(p)) return false;
    const cursor = p.tracks[state.track], anchor = state.selection ? p.tracks[state.selection.track] : null;
    const label = options.group ? 'move group' : tracks.length > 1 ? 'move tracks' : 'move track';
    return mutate(label, q => {
      const map = G().moveTracks(q, tracks, before, group);
      if (options.select !== false && !options.group && tracks.length === 1) {
        state.track = map[tracks[0]];
        state.selection = null;
      } else {
        state.track = Math.max(0, q.tracks.indexOf(cursor));
        if (anchor && state.selection) state.selection = { ...state.selection, track: Math.max(0, q.tracks.indexOf(anchor)) };
      }
      state.instrument = q.tracks[state.track].instrument;
    });
  }

  /* ---------- what a pointer grabs ---------- */

  const closest = (node, selector) => node?.closest?.(selector) || null;
  const trackOfOwner = (p, owner) => p.tracks.findIndex(t => `track:${t.routingID}` === owner);

  function sourceFor(target) {
    const p = state.project;
    if (!p || state.modal) return null;
    if (closest(target, 'input,select,textarea,.lane-delete,[data-group-reveal]')) return null;
    if (state.view === 'pattern') {
      const groupHead = closest(target, '#tracker th.group-head[data-group-id]') || closest(target, '#tracker th[data-group-overview]');
      if (groupHead) {
        const id = groupHead.dataset.groupId || groupHead.dataset.groupOverview, members = G().members(p, id);
        return members.length ? { surface: 'pattern', tracks: members, group: id, label: G().byID(p, id)?.name } : null;
      }
      const head = closest(target, '#tracker th[data-track]');
      if (!head) return null;
      const index = Number(head.dataset.track);
      if (!p.tracks[index]) return null;
      const selected = app.laneSelection?.() || [index];
      const tracks = selected.length > 1 && selected.includes(index) ? selected : [index];
      return { surface: 'pattern', tracks, group: null, label: tracks.length > 1 ? `${tracks.length} tracks` : p.tracks[index].name };
    }
    if (state.view === 'mixer') {
      const title = closest(target, '#mixer-channels .studio-channel-title');
      const strip = title && closest(title, '[data-mixer-channel]');
      if (!strip) return null;
      const owner = strip.dataset.mixerOwner;
      if (owner?.startsWith('bus:')) {
        const group = G().groupOfBus(p, owner);
        return group ? { surface: 'mixer', tracks: G().members(p, group), group: group.id, label: group.name } : null;
      }
      const index = owner ? trackOfOwner(p, owner) : Number(strip.dataset.mixerChannel);
      return Number.isInteger(index) && p.tracks[index] ? { surface: 'mixer', tracks: [index], group: null, label: p.tracks[index].name } : null;
    }
    return null;
  }

  const rect = node => node.getBoundingClientRect();

  function patternColumns() {
    const p = state.project;
    return Array.from(document.querySelectorAll('#tracker thead tr:last-child th[data-track]')).map(th => {
      const box = rect(th), index = Number(th.dataset.track);
      return th.dataset.groupOverview !== undefined
          ? { left: box.left, right: box.right, kind: 'overview', group: th.dataset.groupOverview }
          : { left: box.left, right: box.right, kind: 'track', track: index, group: G().ofTrack(p, index)?.id || null };
    });
  }

  function mixerColumns() {
    const p = state.project;
    return Array.from(document.querySelectorAll('#mixer-channels [data-mixer-channel]')).map(strip => {
      const box = rect(strip), owner = strip.dataset.mixerOwner;
      if (owner?.startsWith('bus:')) {
        const group = G().groupOfBus(p, owner);
        if (!group) return { left: box.left, right: box.right, kind: 'end' };
        const shown = G().members(p, group).some(i => document.querySelector(`#mixer-channels [data-mixer-owner="track:${p.tracks[i].routingID}"]`));
        return { left: box.left, right: box.right, kind: group.mixerCollapsed && !shown ? 'overview' : 'groupHead', group: group.id };
      }
      const index = owner ? trackOfOwner(p, owner) : Number(strip.dataset.mixerChannel);
      if (!Number.isInteger(index) || !p.tracks[index]) return { left: box.left, right: box.right, kind: 'end' };
      return { left: box.left, right: box.right, kind: 'track', track: index, group: G().ofTrack(p, index)?.id || null };
    });
  }

  const container = surface => document.querySelector(surface === 'mixer' ? '#mixer-channels' : '.tracker-wrap');

  /* ---------- pointer gesture ---------- */

  function overlay() {
    if (drag.indicator) return;
    const line = document.createElement('div');
    line.className = 'lane-drop-indicator';
    line.setAttribute('aria-hidden', 'true');
    const tag = document.createElement('span');
    tag.className = 'lane-drop-label';
    line.appendChild(tag);
    const ghost = document.createElement('div');
    ghost.className = 'lane-drag-ghost';
    ghost.setAttribute('aria-hidden', 'true');
    ghost.textContent = drag.label || 'Track';
    document.body.appendChild(line);
    document.body.appendChild(ghost);
    Object.assign(drag, { indicator: line, tag, ghost });
    document.body.classList?.add('lane-dragging');
  }

  function paint() {
    const p = state.project, box = container(drag.surface);
    const columns = drag.surface === 'mixer' ? mixerColumns() : patternColumns();
    drag.target = dropTarget(p, columns, drag.x, drag);
    for (const node of document.querySelectorAll('.lane-drag-source')) node.classList.remove('lane-drag-source');
    const sources = drag.group
        ? (drag.surface === 'mixer' ? [] : document.querySelectorAll(`#tracker th[data-group-id="${drag.group}"],#tracker th[data-group-overview="${drag.group}"]`))
        : drag.tracks.map(i => document.querySelector(drag.surface === 'mixer'
            ? `#mixer-channels [data-mixer-owner="track:${p.tracks[i]?.routingID}"],#mixer-channels [data-mixer-channel="${i}"]`
            : `#tracker th[data-track="${i}"]`));
    for (const node of sources) node?.classList?.add('lane-drag-source');
    if (drag.ghost) {
      drag.ghost.style.left = `${drag.x + 12}px`;
      drag.ghost.style.top = `${drag.y + 10}px`;
    }
    const line = drag.indicator;
    if (!line) return;
    if (!drag.target) { line.hidden = true; return; }
    const area = box ? rect(box) : { top: 0, height: window.innerHeight || 600 };
    const group = drag.target.group && !drag.group ? G().byID(p, drag.target.group) : null;
    const leaving = !drag.group && drag.tracks.some(i => p.tracks[i]?.groupID && p.tracks[i].groupID !== drag.target.group);
    line.hidden = false;
    line.style.left = `${Math.round(drag.target.x) - 1}px`;
    line.style.top = `${Math.round(area.top)}px`;
    line.style.height = `${Math.round(area.height)}px`;
    line.style.setProperty?.('--group', group?.color || 'var(--teal)');
    line.classList.toggle('is-group', !!group);
    drag.tag.textContent = group ? `into ${group.name}` : leaving ? `out of ${G().ofTrack(p, drag.tracks[0])?.name || 'group'}` : '';
  }

  // Near the left or right edge of the grid (or mixer) the view scrolls; the loop runs while it can still scroll.
  function autoscroll() {
    if (!drag?.active) return;
    drag.frame = null;
    const box = container(drag.surface);
    if (!box) return;
    const area = rect(box), left = drag.x - area.left, right = area.right - drag.x;
    const speed = left < EDGE ? -Math.ceil(MAX_SPEED * (1 - Math.max(0, left) / EDGE))
        : right < EDGE ? Math.ceil(MAX_SPEED * (1 - Math.max(0, right) / EDGE)) : 0;
    if (!speed) return;
    const was = box.scrollLeft;
    box.scrollLeft = was + speed;
    if (box.scrollLeft === was) return;
    paint();
    drag.frame = raf(autoscroll);
  }

  function start(e) {
    if (e.button !== 0 || drag || e.ctrlKey || e.metaKey) return;
    const source = sourceFor(e.target);
    if (!source) return;
    drag = { ...source, x0: e.clientX, y0: e.clientY, x: e.clientX, y: e.clientY, active: false, target: null };
    window.addEventListener?.('pointermove', move, true);
    window.addEventListener?.('pointerup', end, true);
    window.addEventListener?.('pointercancel', cancel, true);
    window.addEventListener?.('keydown', key, true);
    window.addEventListener?.('blur', cancel);
  }

  function move(e) {
    if (!drag) return;
    drag.x = e.clientX;
    drag.y = e.clientY;
    if (!drag.active) {
      if (Math.hypot(drag.x - drag.x0, drag.y - drag.y0) < THRESHOLD) return;
      drag.active = true;
      app.finishEntry?.({ advance: false, render: false });
      overlay();
    }
    e.preventDefault?.();
    paint();
    if (!drag.frame) drag.frame = raf(autoscroll);
  }

  function cleanup() {
    window.removeEventListener?.('pointermove', move, true);
    window.removeEventListener?.('pointerup', end, true);
    window.removeEventListener?.('pointercancel', cancel, true);
    window.removeEventListener?.('keydown', key, true);
    window.removeEventListener?.('blur', cancel);
    if (!drag) return;
    if (drag.frame) unraf(drag.frame);
    drag.indicator?.remove();
    drag.ghost?.remove();
    document.body.classList?.remove('lane-dragging');
    for (const node of document.querySelectorAll('.lane-drag-source')) node.classList.remove('lane-drag-source');
  }

  function end(e) {
    const done = drag;
    if (!done) return;
    if (done.active) {
      e?.preventDefault?.();
      e?.stopPropagation?.();
      // The click that follows a drag must not select or toggle anything.
      suppressClick = true;
      setTimeout(() => { suppressClick = false; }, 0);
      done.x = e?.clientX ?? done.x;
      paint();
    }
    cleanup();
    drag = null;
    if (!done.active || !done.target) return false;
    return moveLanes(done.tracks, done.target.before, done.target.group, { group: done.group });
  }

  function cancel() {
    if (!drag) return;
    cleanup();
    drag = null;
  }

  function key(e) {
    if (e.key !== 'Escape' || !drag) return;
    e.preventDefault();
    e.stopPropagation?.();
    e.stopImmediatePropagation?.();
    cancel();
  }

  const doc = typeof document === 'undefined' ? null : document;
  doc?.addEventListener?.('pointerdown', start, true);
  doc?.addEventListener?.('click', e => {
    if (suppressClick) {
      suppressClick = false;
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const button = closest(e.target, '[data-strip-delete]');
    if (button) {
      e.preventDefault();
      e.stopPropagation();
      deleteStrip(button.dataset.stripDelete);
    }
  }, true);

  /* ---------- mixer delete and context menus ---------- */

  // A mixer strip's × or menu: a track strip deletes its track, a group strip asks about the group.
  function deleteStrip(owner) {
    const p = state.project;
    if (/^\d+$/.test(String(owner))) return app.deleteTracks([Number(owner)]);
    if (String(owner).startsWith('bus:')) {
      const group = G().groupOfBus(p, owner);
      return group ? app.deleteGroup(group.id) : Promise.resolve(false);
    }
    const index = trackOfOwner(p, owner);
    return index >= 0 ? app.deleteTracks([index]) : Promise.resolve(false);
  }

  function contextItems({ target }) {
    const p = state.project;
    if (!p) return null;
    const head = closest(target, '#tracker th.group-head[data-group-id]');
    if (head) {
      const group = G().byID(p, head.dataset.groupId);
      if (!group) return null;
      return [{ header: true, title: group.name },
        { title: group.collapsed ? 'Expand group' : 'Collapse group', key: '⌥J', action: () => app.setCollapsed(group.id, !group.collapsed) },
        { title: 'Rename group…', action: () => app.renameGroup(group.id) },
        { title: 'Add track to group', action: () => app.addGroupTrack(group.id) },
        app.hideEmptyOn() ? { title: 'Show hidden tracks of this group', action: () => app.revealGroup(group.id, true) } : null,
        { separator: true },
        { title: 'Ungroup (keep tracks)', action: () => app.mutate('ungroup', q => G().dissolve(q, group.id)) },
        { title: 'Delete group…', action: () => app.deleteGroup(group.id) }];
    }
    const strip = state.view === 'mixer' && closest(target, '#mixer-channels [data-mixer-channel]');
    if (!strip) return null;
    const owner = strip.dataset.mixerOwner, group = owner?.startsWith('bus:') ? G().groupOfBus(p, owner) : null;
    const index = group ? -1 : owner ? trackOfOwner(p, owner) : Number(strip.dataset.mixerChannel);
    if (group) return [{ header: true, title: group.name },
      { title: 'Delete group…', key: '⌘⌫', action: () => app.deleteGroup(group.id) }];
    if (!p.tracks[index]) return null;
    return [{ header: true, title: p.tracks[index].name },
      { title: 'Move left', key: '⌘⇧←', enabled: index > 0, action: () => moveLanes([index], index - 1, G().ofTrack(p, index - 1)?.id || null) },
      { title: 'Move right', key: '⌘⇧→', enabled: index < p.tracks.length - 1, action: () => moveLanes([index], index + 2, G().ofTrack(p, index + 1)?.id || null) },
      { title: 'Delete track…', key: '⌘⌫', enabled: p.tracks.length > 1, action: () => app.deleteTracks([index]) }];
  }

  // Mixer strip delete button (studio.js markup).
  const stripDeleteButton = (owner, name, kind, tab) => `<button class="lane-action lane-delete strip-delete" data-strip-delete="${esc(owner)}"`
      + ` tabindex="${tab}" aria-label="Delete ${esc(name)}" title="Delete ${kind} · ⌘⌫ (asks first when it has notes)">×</button>`;

  Object.assign(app, { dropTarget, moveLanes, deleteStrip, stripDeleteButton });
  window.STTrackLanes = {
    dropTarget, moveLanes, deleteStrip, contextItems, stripDeleteButton,
    // Pointer handlers, for tests.
    gesture: { start, move, end, cancel, key, get active() { return drag; } }
  };
})();
