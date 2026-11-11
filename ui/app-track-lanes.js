/* Track lanes in the pattern editor: hide empty tracks, collapse a track to a slim column, and delete tracks
   (with a short summary of what goes) and groups (delete with their tracks, or ungroup). Drag and drop
   reordering lives in app-track-drag.js; the project model in track-groups.js (STGroups.moveTracks /
   removeTracks).

   View state (project.viewState, saved with the session, written into the Undo/Redo snapshots, never an
   Undo step, like the Gate column and group collapse):
     hideEmpty: true            hide tracks without steps
     hideEmptyScope: 'song'     a track counts as used when any pattern has a step on it (default: this pattern)
     collapsedTracks: [routingID…]  tracks shown as slim columns
   The cursor track and the selection anchor are never hidden, an expanded group always keeps one column, and
   a group's hidden members can be revealed from its header (“+N hidden”, until Hide empty is toggled). */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, esc, state, toast, mutate, refresh, updateTransport, pattern, noteText, clamp } = app;
  const G = () => window.STGroups;

  const SLIM_WIDTH = 58;
  const revealed = new Set(); // group IDs whose hidden members are shown
  let renderedHidden = new Set();
  let songCache = null;

  /* ---------- view state ---------- */

  function setViewState(edit) {
    let changed = false;
    for (const p of [state.project, ...state.undo.map(x => x.project), ...state.redo.map(x => x.project)]) {
      if (!p) continue;
      const before = JSON.stringify(p.viewState), next = { ...p.viewState };
      edit(next, p);
      for (const key of Object.keys(next)) if (next[key] === undefined || next[key] === false
          || Array.isArray(next[key]) && !next[key].length) delete next[key];
      if (JSON.stringify(next) === before) continue;
      p.viewState = next;
      if (p === state.project) changed = true;
    }
    if (changed) {
      state.dirty = true;
      updateTransport();
      app.scheduleAutosave?.();
    }
    return changed;
  }

  const hideEmptyOn = (p = state.project) => !!p?.viewState.hideEmpty;
  const hideScope = (p = state.project) => p?.viewState.hideEmptyScope === 'song' ? 'song' : 'pattern';
  const collapsedIDs = (p = state.project) => p?.viewState.collapsedTracks || [];
  const trackKey = track => typeof track?.routingID === 'string' && track.routingID ? track.routingID : null;
  const isCollapsed = (index, p = state.project) => {
    const key = trackKey(p?.tracks?.[index]);
    return !!key && collapsedIDs(p).includes(key);
  };

  /* ---------- hide empty tracks ---------- */

  const hasSteps = steps => Array.isArray(steps) && steps.some(step => step != null);

  // Tracks with a step anywhere in the song (or an audio clip). Cached per edit generation: hiding is computed on
  // every grid render and viewport scroll.
  function songUsed(p) {
    const key = [p, app.inputEditGeneration, state.undo.length, state.redo.length, p.patterns.length, p.tracks.length];
    if (songCache && songCache.key.every((value, i) => value === key[i])) return songCache.used;
    const used = new Set();
    for (const clip of p.audioClips || []) if (Number.isInteger(clip?.track)) used.add(clip.track);
    p.tracks.forEach((_, index) => {
      if (!used.has(index) && p.patterns.some(pat => hasSteps(pat.steps?.[index]))) used.add(index);
    });
    songCache = { key, used };
    return used;
  }

  function trackUsed(p, index, scope = hideScope(p)) {
    if (scope === 'song') return songUsed(p).has(index);
    const pat = p.patterns.find(x => x.id === p.activePattern) || p.patterns[0];
    return hasSteps(pat?.steps?.[index]);
  }

  // Indices of the tracks hidden right now.
  function hiddenTracks(p = state.project) {
    const hidden = new Set();
    if (!p || !hideEmptyOn(p)) return hidden;
    const scope = hideScope(p), keep = new Set([state.track, state.selection?.track]);
    p.tracks.forEach((_, index) => {
      if (keep.has(index)) return;
      const group = G()?.ofTrack(p, index);
      if (group && (group.collapsed || revealed.has(group.id))) return;
      if (!trackUsed(p, index, scope)) hidden.add(index);
    });
    // An expanded group keeps one column, so its header (and “+N hidden”) stays reachable.
    for (const group of G()?.list(p) || []) {
      const members = G().members(p, group);
      if (members.length && members.every(index => hidden.has(index))) hidden.delete(members[0]);
    }
    return hidden;
  }

  const isHidden = (index, p = state.project) => hiddenTracks(p).has(index);

  /* Visible grid columns: hidden tracks drop out, collapsed tracks become slim columns. Called by
     app.patternColumns() so the grid, keyboard navigation, the virtual layout and MK3 agree. */
  function laneColumns(columns) {
    const p = state.project, hidden = hiddenTracks(p), slim = new Set(collapsedIDs(p));
    if (!hidden.size && !slim.size) return columns;
    const result = [];
    for (const column of columns) {
      if (column.collapsed) { result.push(column); continue; }
      if (hidden.has(column.index)) continue;
      result.push(slim.has(trackKey(column.track)) ? { ...column, slim: true } : column);
    }
    return result;
  }

  // Called while the grid renders: a recorded note on a track hidden in that render re-renders the grid.
  function rememberRendered() { renderedHidden = hiddenTracks(); }
  const wasHidden = index => renderedHidden.has(index);

  function setHideEmpty(on = !hideEmptyOn()) {
    revealed.clear();
    setViewState(view => { view.hideEmpty = !!on || undefined; });
    if (state.view === 'pattern') refresh();
    const count = hiddenTracks().size;
    toast(on
        ? `Hiding empty tracks (${hideScope() === 'song' ? 'no steps in any pattern' : 'no steps in this pattern'}) · ${count} hidden · ⌥U shows them`
        : 'Showing every track.');
    return true;
  }

  function setHideScope(scope = hideScope() === 'song' ? 'pattern' : 'song') {
    setViewState(view => { view.hideEmptyScope = scope === 'song' ? 'song' : undefined; });
    if (state.view === 'pattern') refresh();
    toast(scope === 'song' ? 'Hide empty tracks: a track is empty when no pattern in the song uses it.'
        : 'Hide empty tracks: a track is empty when this pattern has no steps on it.');
    return true;
  }

  function revealGroup(groupID, show = !revealed.has(groupID)) {
    if (show) revealed.add(groupID); else revealed.delete(groupID);
    if (state.view === 'pattern') refresh();
    return true;
  }

  function groupHiddenChip(group) {
    if (!hideEmptyOn() || group.collapsed) return '';
    if (revealed.has(group.id)) {
      const empty = G().members(state.project, group).filter(index => !trackUsed(state.project, index)).length;
      return empty ? `<button class="lane-hidden-chip is-revealed" data-group-reveal="${esc(group.id)}" id="group-${esc(group.id)}-reveal" tabindex="-1"`
          + ` title="Hide the empty tracks of ${esc(group.name)} again">hide ${empty}</button>` : '';
    }
    const count = G().members(state.project, group).filter(index => renderedHidden.has(index)).length;
    return count ? `<button class="lane-hidden-chip" data-group-reveal="${esc(group.id)}" id="group-${esc(group.id)}-reveal" tabindex="-1"`
        + ` title="Show the ${count} empty track${count === 1 ? '' : 's'} of ${esc(group.name)}">+${count} hidden</button>` : '';
  }

  function hideEmptyControls() {
    const on = hideEmptyOn(), count = on ? hiddenTracks().size : 0;
    return `<button id="hide-empty" class="${on ? 'active' : ''}" aria-pressed="${on}" title="Hide empty tracks: tracks`
        + ` without steps ${hideScope() === 'song' ? 'anywhere in the song' : 'in this pattern'} · ⌥U">Hide empty${on && count
            ? ` <span class="lane-hidden-count">${count}</span>` : ''}</button>${on ? `<select id="hide-empty-scope"`
        + ` aria-label="Hide empty tracks based on" title="Empty in this pattern, or in the whole song">`
        + `<option value="pattern" ${hideScope() === 'pattern' ? 'selected' : ''}>this pattern</option>`
        + `<option value="song" ${hideScope() === 'song' ? 'selected' : ''}>whole song</option></select>` : ''}`;
  }

  /* ---------- collapsed (slim) tracks ---------- */

  function setTrackCollapsed(index, collapsed) {
    const key = trackKey(state.project.tracks[index]);
    if (!key) return false;
    setViewState(view => {
      const list = (view.collapsedTracks || []).filter(id => id !== key);
      if (collapsed) list.push(key);
      view.collapsedTracks = list;
    });
    if (collapsed && state.track === index) state.column = 0;
    if (state.view === 'pattern') refresh();
    return true;
  }

  function toggleTrackCollapsed(index = state.track) {
    if (!state.project.tracks[index]) return false;
    const group = G()?.ofTrack(state.project, index);
    if (group?.collapsed) return app.toggleCollapse(group.id);
    return setTrackCollapsed(index, !isCollapsed(index));
  }

  function expandAllTracks() {
    if (!collapsedIDs().length) return toast('No collapsed tracks.');
    setViewState(view => { view.collapsedTracks = []; });
    if (state.view === 'pattern') refresh();
    return true;
  }

  function slimHeader({ track: t, index: i, group }) {
    return `<th class="track-head track-slim ${group ? 'group-member' : ''} ${i === state.track ? 'selected-track' : ''}`
        + ` ${app.trackSilenced(state.project, i) ? 'track-silenced' : ''}" data-track="${i}" data-track-slim`
        + ` style="--lane:${esc(t.color || app.colors[i % 8])}${group ? `;--group:${esc(group.color)}` : ''}"`
        + ` title="${esc(t.name)} · collapsed · double-click or ⌥⇧J to expand"><div class="track-slim-top">`
        + `<span class="track-num">${String(i + 1).padStart(2, '0')}</span><button class="lane-action" data-track-collapse="${i}"`
        + ` id="track-${i}-collapse" tabindex="${i === state.track ? 0 : -1}" aria-expanded="false" aria-label="Expand ${esc(t.name)}">▸</button></div>`
        + `<span class="track-slim-name">${esc(t.name)}</span></th>`;
  }

  function slimCell({ index: i }, r, sel, silenced) {
    const s = pattern().steps[i]?.[r],
      active = r === state.row && i === state.track,
      isSel = sel && r >= sel.r0 && r <= sel.r1 && i >= sel.t0 && i <= sel.t1;
    const played = app.playedCells?.(s) || [], more = played.length > 1 || s?.chord?.length > 1 ? '+' : '';
    const text = !s ? '·' : played.length ? noteText(played[0].cell.note) + more : s.note < -2 ? 'fx' : noteText(s.note);
    return `<td data-cell="${i}:${r}" class="track-slim-cell ${active ? 'selected' : ''} ${isSel ? 'selection' : ''}`
        + ` ${silenced[i] ? 'track-silenced' : ''}"><span class="slim-hit ${s ? 'has-note' : ''}">${esc(text)}</span></td>`;
  }

  // Collapse and delete buttons on a track header (shown on hover and on the cursor track).
  function headerActions(t, i) {
    const tab = i === state.track ? 0 : -1;
    return `<span class="lane-actions"><button class="lane-action" data-track-collapse="${i}" id="track-${i}-collapse" tabindex="${tab}"`
        + ` aria-expanded="true" aria-label="Collapse ${esc(t.name)}" title="Collapse to a slim column · ⌥⇧J · double-click the header">◂</button>`
        + `<button class="lane-action lane-delete" data-track-delete="${i}" id="track-${i}-delete" tabindex="${tab}" aria-label="Delete ${esc(t.name)}"`
        + ` title="Delete track · ⌘⌫ (asks first when it has notes)">×</button></span>`;
  }

  function groupHeaderActions(group) {
    return `<button class="lane-action lane-delete group-delete" data-group-delete="${esc(group.id)}" id="group-${esc(group.id)}-delete" tabindex="-1"`
        + ` aria-label="Delete group ${esc(group.name)}" title="Delete the group, or ungroup and keep its tracks">×</button>`;
  }

  /* ---------- delete ---------- */

  const plural = (count, word, suffix = 's') => `${count} ${word}${count === 1 ? '' : suffix}`;

  // What deleting these tracks removes: notes, patterns, automation lanes, clips and plug-ins.
  function contentSummary(p, indices) {
    const chosen = new Set(indices), owners = new Set(indices.map(i => G().trackOwner(p.tracks[i])).filter(Boolean));
    // A lane names its channel by owner, or (the engine's own track parameters) its track by index.
    const belongs = item => item && (item.owner ? owners.has(item.owner) : chosen.has(item.track));
    let notes = 0, steps = 0, lanes = 0;
    const patterns = new Set();
    for (const pat of p.patterns || []) {
      for (const index of indices) for (const step of pat.steps?.[index] || []) if (step) {
        steps++;
        notes += app.playedCells ? app.playedCells(step).length : step.note >= 0 ? 1 : 0;
        patterns.add(pat.id);
      }
      lanes += (pat.automation || []).filter(belongs).length;
    }
    const clips = [...(p.audioClips || []), ...(p.patternClips || [])].filter(item => chosen.has(item?.track)).length;
    const plugins = (p.audioPlugins || []).filter(item => owners.has(item?.owner)).length;
    return { notes, steps, patterns: patterns.size, lanes, clips, plugins, total: steps + lanes + clips + plugins };
  }

  function describeContent(summary) {
    const parts = [];
    if (summary.steps) {
      const what = summary.notes ? plural(summary.notes, 'note') : plural(summary.steps, 'step');
      parts.push(`${what} in ${plural(summary.patterns, 'pattern')}`);
    }
    if (summary.lanes) parts.push(plural(summary.lanes, 'automation lane'));
    if (summary.clips) parts.push(plural(summary.clips, 'audio clip'));
    if (summary.plugins) parts.push(plural(summary.plugins, 'plug-in'));
    return parts.length > 1 ? parts.slice(0, -1).join(', ') + ' and ' + parts.at(-1) : parts[0] || 'nothing';
  }

  function trackNames(p, indices) {
    const names = indices.map(i => p.tracks[i]?.name || `Track ${i + 1}`);
    return names.length <= 3 ? names.join(', ') : `${names.slice(0, 3).join(', ')} and ${names.length - 3} more`;
  }

  // The tracks a delete acts on: the selected tracks (visible ones), or the cursor track.
  function laneSelection() {
    const hidden = hiddenTracks();
    if (state.selection && state.selection.track !== state.track) {
      const t0 = Math.min(state.selection.track, state.track), t1 = Math.max(state.selection.track, state.track);
      return Array.from({ length: t1 - t0 + 1 }, (_, i) => t0 + i).filter(i => !hidden.has(i));
    }
    return [state.track];
  }

  function commitDelete(tracks) {
    const p = state.project, indices = tracks.map(track => p.tracks.indexOf(track)).filter(i => i >= 0);
    if (!indices.length) return false;
    if (indices.length >= p.tracks.length) {
      toast('Keep at least one track in a session.');
      return false;
    }
    const names = trackNames(p, indices), first = Math.min(...indices);
    const done = mutate(indices.length > 1 ? 'delete tracks' : 'delete track', q => {
      G().removeTracks(q, indices);
      state.track = clamp(first, 0, q.tracks.length - 1);
      state.instrument = q.tracks[state.track].instrument || 0;
      state.selection = null;
      state.column = 0;
      state.noteColumn = 0;
      if (state.mixerOwner && !String(state.mixerOwner).startsWith('bus:')) state.mixerOwner = null;
    });
    if (done) toast(`Deleted ${names} · ⌘Z restores`);
    return done;
  }

  /* Deletes tracks (default: the selected tracks). Empty tracks go at once; tracks with notes, automation, clips
     or plug-ins ask first with a summary. On a collapsed group (no selection) it deletes the group instead. */
  async function deleteTracks(indices = null) {
    const p = state.project;
    if (!p) return false;
    if (!indices) {
      const group = G()?.ofTrack(p, state.track);
      if (group?.collapsed && !(state.selection && state.selection.track !== state.track)) return deleteGroup(group.id);
      indices = laneSelection();
    }
    indices = [...new Set(indices)].filter(i => Number.isInteger(i) && p.tracks[i]).sort((a, b) => a - b);
    if (!indices.length) return false;
    if (indices.length >= p.tracks.length) {
      toast('Keep at least one track in a session.');
      return false;
    }
    const tracks = indices.map(i => p.tracks[i]), summary = contentSummary(p, indices);
    if (summary.total) {
      const subject = indices.length === 1 ? `${p.tracks[indices[0]].name || `Track ${indices[0] + 1}`} has`
          : `${indices.length} tracks (${trackNames(p, indices)}) have`;
      const ok = await app.confirmChoice(indices.length === 1 ? 'Delete track?' : `Delete ${indices.length} tracks?`,
          `${subject} ${describeContent(summary)}. ⌘Z restores ${indices.length === 1 ? 'it' : 'them'}.`, 'Delete');
      if (!ok || state.project !== p) return false;
    }
    return commitDelete(tracks);
  }

  /* A group: delete it with its tracks, or ungroup (the tracks stay and route to Master; the group bus with its
     inserts and sends goes). Resolves to 'delete', 'ungroup' or false. */
  function deleteGroup(groupID) {
    const p = state.project, group = G()?.byID(p, groupID);
    if (!group) {
      toast('Move the cursor to a group to delete it.');
      return Promise.resolve(false);
    }
    const members = G().members(p, group), summary = contentSummary(p, members), all = members.length >= p.tracks.length;
    const body = `<p class="muted" style="line-height:1.7">${esc(group.name)} has ${plural(members.length, 'track')}`
        + `${summary.total ? ` with ${esc(describeContent(summary))}` : ''}.</p><p class="muted" style="line-height:1.7">`
        + `<strong>Ungroup</strong> keeps the tracks (they route to Master) and removes the group bus. <strong>Delete`
        + ` group and tracks</strong> removes both. ⌘Z restores either.${all ? ' A session keeps at least one track, so this'
            + ' group can only be ungrouped.' : ''}</p>`;
    return new Promise(resolve => {
      let settled = false;
      const finish = choice => {
        if (settled) return;
        settled = true;
        app.modalDismiss = null;
        app.closeModal();
        if (choice === 'ungroup' && state.project === p) {
          resolve(mutate('ungroup', q => G().dissolve(q, groupID)) ? 'ungroup' : false);
        } else if (choice === 'delete' && state.project === p) {
          const tracks = G().members(p, groupID).map(i => p.tracks[i]);
          resolve(commitDelete(tracks) ? 'delete' : false);
        } else resolve(false);
      };
      if (app.openModal(`Delete group “${group.name}”?`, body, `<button id="group-delete-cancel">Cancel</button>`
          + `<button id="group-delete-ungroup">Ungroup, keep tracks</button><button id="group-delete-all" class="primary danger"`
          + ` ${all ? 'disabled' : ''}>Delete group and tracks</button>`, { kind: 'confirm' }) === false || !state.modal) {
        resolve(false);
        return;
      }
      app.modalDismiss = () => {
        if (!settled) {
          settled = true;
          resolve(false);
        }
      };
      $('#group-delete-cancel').onclick = () => finish(false);
      $('#group-delete-ungroup').onclick = () => finish('ungroup');
      $('#group-delete-all').onclick = () => finish('delete');
      setTimeout(() => $('#group-delete-ungroup')?.focus(), 0);
    });
  }

  // ⌘⌫ in the mixer: the selected strip's track, or its group.
  function deleteMixerSelection() {
    const owner = String(state.mixerOwner || '');
    if (owner.startsWith('bus:')) {
      const group = G()?.groupOfBus(state.project, owner);
      return group ? deleteGroup(group.id) : (toast('Only tracks and groups can be deleted here.'), Promise.resolve(false));
    }
    return deleteTracks([state.track]);
  }

  /* ---------- moving with ⌘⇧←/→ skips hidden tracks ---------- */

  function moveSkippingHidden(delta) {
    if (!hideEmptyOn()) return app.moveTrack(delta);
    const p = state.project, track = p.tracks[state.track];
    if (!track) return false;
    const hidden = new Set([...hiddenTracks(p)].map(i => p.tracks[i])), undoBefore = state.undo.length, from = state.track;
    const step = () => {
      try { return app.moveTrack(delta); } catch { return false; }
    };
    let moved = step();
    // Keep stepping while the move only swapped the track with a hidden one (a group edge step keeps the order).
    for (let guard = 0, last = from; moved && guard < p.tracks.length; guard++) {
      const q = state.project, now = q.tracks.indexOf(track), passed = q.tracks[now - delta];
      if (now === last || !passed || !hidden.has(passed)) break;
      last = now;
      if (!step()) break;
    }
    // One Undo step for the whole move: the first entry holds the state before it.
    if (state.undo.length > undoBefore + 1) state.undo.splice(undoBefore + 1);
    return moved;
  }

  /* ---------- binding ---------- */

  function bindTrackLanes() {
    for (const button of $$('[data-track-collapse]')) {
      button.ondblclick = e => e.stopPropagation();
      button.onclick = e => {
        e.stopPropagation();
        toggleTrackCollapsed(Number(button.dataset.trackCollapse));
      };
    }
    for (const button of $$('[data-track-delete]')) {
      button.ondblclick = e => e.stopPropagation();
      button.onclick = e => {
        e.stopPropagation();
        const index = Number(button.dataset.trackDelete);
        const selected = laneSelection();
        void deleteTracks(selected.includes(index) ? selected : [index]);
      };
    }
    for (const button of $$('[data-group-delete]')) {
      button.ondblclick = e => e.stopPropagation();
      button.onclick = e => {
        e.stopPropagation();
        void deleteGroup(button.dataset.groupDelete);
      };
    }
    for (const button of $$('[data-group-reveal]')) button.onclick = e => {
      e.stopPropagation();
      revealGroup(button.dataset.groupReveal);
    };
    if ($('#hide-empty')) $('#hide-empty').onclick = () => app.perform('hideEmptyTracks');
    if ($('#hide-empty-scope')) $('#hide-empty-scope').onchange = e => {
      setHideScope(e.target.value);
      $('#tracker')?.focus({ preventScroll: true });
    };
  }

  Object.assign(app, {
    SLIM_WIDTH, laneColumns, hiddenTracks, isHidden, trackUsed, hideEmptyOn, hideScope, setHideEmpty, setHideScope,
    revealGroup, groupHiddenChip, hideEmptyControls, rememberRendered, wasHidden, isCollapsed, setTrackCollapsed,
    toggleTrackCollapsed, expandAllTracks, slimHeader, slimCell, headerActions, groupHeaderActions, contentSummary,
    describeContent, laneSelection, deleteTracks, moveSkippingHidden, deleteGroup, deleteMixerSelection, bindTrackLanes, setViewState
  });
})();

(function () {
  'use strict';
  const app = window.STApp;
  app.defineCommands({
    hideEmptyTracks: () => app.setHideEmpty(),
    hideEmptyScope: () => app.setHideScope(),
    toggleTrackCollapsed: () => app.toggleTrackCollapsed(),
    expandAllTracks: () => app.expandAllTracks(),
    // The confirmation is a dialog the user answers; the command itself returns once it is shown.
    deleteTracks: () => {
      void (app.state.view === 'mixer' ? app.deleteMixerSelection() : app.deleteTracks());
      return true;
    },
    deleteGroup: () => {
      const group = window.STGroups?.ofTrack(app.state.project, app.state.track);
      if (!group) return app.toast('Move the cursor to a group to delete it.');
      void app.deleteGroup(group.id);
      return true;
    }
  });
})();
