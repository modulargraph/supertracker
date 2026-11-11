/* Explicit, transactional routing edits. Native validation remains authoritative. */
(() => {
  'use strict';
  const ui = {
    capabilities: null,
    capabilityError: '',
    capabilityGeneration: 0,
    focusGeneration: 0,
    loading: null,
    preparing: false,
    application: null,
    status: null
  };
  const S = () => window.ST;
  const project = () => S().state.project;
  const { clone } = window.STUtil;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const key = value => String(value).replace(/[^a-zA-Z0-9_-]/g, '_');
  const number = (value, fallback = 0) => Number.isFinite(Number(value)) ? Number(value) : fallback;
  const gain = value => Math.max(0, Math.min(2, number(value, 1)));
  // An emptied gain field is not a level (Number('') is 0, which would silence the send): the field shows the stored gain again.
  const typedGain = value => String(value ?? '').trim() !== '' && Number.isFinite(Number(value));
  const uid = () => globalThis.crypto?.randomUUID?.() || S().uid();
  const stable = value => value === null || typeof value !== 'object'
      ? JSON.stringify(value)
      : Array.isArray(value)
      ? `[${value.map(stable).join(',')}]`
      : `{${Object.keys(value).sort().map(name => `${JSON.stringify(name)}:${stable(value[name])}`).join(',')}}`;
  const currentOwner = () => S().state.mixerOwner || endpoints(project()).find(item => item.track === S().state.track)?.owner || 'master';
  const routing = (p = project()) => p.routing;
  // What the host said about routing (routing.capabilities): the limits of the project format, the taps,
  // and the sidechain inputs of the inserts in use. Edits wait for it.
  const available = () => !!ui.capabilities;
  const unavailable = () => ui.capabilityError || 'The audio host has not answered about routing yet. Try again.';
  const canOutput = owner => available() && owner !== 'master' && !!ownerEntity(owner);
  const canSend = owner => available() && taps().length > 0 && owner !== 'master' && !!ownerEntity(owner);
  const taps = () => (ui.capabilities?.sends?.taps || []).filter(tap => ['preFader', 'postFader'].includes(tap));
  const keyTaps = () => (ui.capabilities?.sidechains?.taps || []).filter(tap => ['preInserts', 'preFader', 'postFader'].includes(tap));
  const tapName = tap => ({preInserts:'Pre-inserts',preFader:'Pre-fader',postFader:'Post-fader'}[tap] || String(tap));
  const report = error => S().toast(error?.message || String(error), 'error');
  const restoreFocus = (id, allowed = () => !S().state.modal) => {
    if (id) queueMicrotask(() => {
      if (allowed()) document.getElementById(id)?.focus({ preventScroll: true });
    });
  };
  const perform = promise => promise.catch(report);

  function endpoints(p = project()) {
    if (!p) return [];
    const result = (p.tracks || []).map((entity, track) => ({
      owner: `track:${entity.routingID}`,
      id: entity.routingID,
      name: entity.name || `Track ${track + 1}`,
      type: 'track',
      track,
      entity
    }));
    for (const entity of p.routing.buses || []) result.push({
      owner: `bus:${entity.id}`,
      id: entity.id,
      name: entity.name || (entity.type === 'return' ? 'Return' : 'Group'),
      type: entity.type,
      entity
    });
    result.push({ owner: 'master', id: 'master', name: 'Master', type: 'master', entity: p });
    return result;
  }

  const ownerEntity = (owner, p = project()) => endpoints(p).find(item => item.owner === owner)?.entity || null;
  // The selected track's live interface input (line-inputs.js), shown below its output.
  function lineInputSection(owner, p) {
    if (!String(owner).startsWith('track:')) return '';
    const index = (p.tracks || []).findIndex(track => 'track:' + track.routingID === owner);
    return index < 0 ? '' : window.STLineInputs?.mixerSection?.(index) || '';
  }
  const channelName = (owner, p = project()) => owner === null
      ? 'No output'
      : endpoints(p).find(item => item.owner === owner)?.name || 'Unavailable channel';
  const pluginOwner = plugin => plugin.owner;
  // An insert reads at most one sidechain, in stereo. The host lists the inserts in use that read one.
  const readsSidechain = instanceID => !!ui.capabilities?.sidechains.inputs.some(input => input.instanceID === instanceID);
  // An owner-level self edge is valid only from the earlier insert-input tap to
  // an effect input. The native planner still validates the precise dependency.
  const sameOwnerKey = (owner, tap, plugin, p = project()) => owner !== 'master'
      && tap === 'preInserts'
      && keyTaps().includes(tap)
      && plugin?.kind === 'effect'
      && pluginOwner(plugin) === owner;
  const routeTaps = (route, plugin, p = project()) => route.source === pluginOwner(plugin)
      ? keyTaps().filter(tap => sameOwnerKey(route.source, tap, plugin, p))
      : keyTaps();
  const activeOutput = (owner, p = project()) => Object.hasOwn(routing(p).outputs || {}, owner) ? routing(p).outputs[owner] : 'master';

  function graphEdges(p, omitID, omitOutput) {
    const r = routing(p), edges = [];
    for (const item of endpoints(p)) if (item.owner !== 'master' && item.owner !== omitOutput) {
      const destination = activeOutput(item.owner, p); if (destination) edges.push([item.owner, destination]);
    }
    for (const send of r.sends || []) if (send.id !== omitID && send.enabled !== false) edges.push([send.source, send.destination]);
    for (const route of r.sidechains || []) if (route.id !== omitID && route.enabled !== false) {
      const plugin = p.audioPlugins.find(item => item.instanceID === route.destination);
      const destination = plugin && pluginOwner(plugin); if (destination) edges.push([route.source, destination]);
    }
    return edges;
  }

  function wouldCycle(source, destination, edges) {
    if (source === destination) return true;
    const pending = [destination], seen = new Set();
    while (pending.length) {
      const owner = pending.pop(); if (owner === source) return true;
      if (seen.has(owner)) continue; seen.add(owner);
      for (const [from, to] of edges) if (from === owner) pending.push(to);
    }
    return false;
  }

  function destinationOptions(owner, p = project(), { omitID, output = false } = {}) {
    if (owner === 'master') return [];
    const edges = graphEdges(p, omitID, output ? owner : undefined);
    return endpoints(p).filter(item => ['group', 'return', 'master'].includes(item.type) && !wouldCycle(owner, item.owner, edges));
  }

  function sourceOptions(destinationOwner, p = project(), omitID, {tap,plugin} = {}) {
    const edges = graphEdges(p, omitID);
    return endpoints(p).filter(item => item.owner !== 'master'
        && (item.owner === destinationOwner ? sameOwnerKey(destinationOwner,tap,plugin,p) : !wouldCycle(item.owner, destinationOwner, edges)));
  }

  async function refreshCapabilities() {
    if (ui.loading) return ui.loading;
    const original = project(), generation = ui.capabilityGeneration;
    let stale = false;
    ui.loading = (async () => {
      try {
        const result = await S().request('routing.capabilities', {});
        if (project() !== original || generation !== ui.capabilityGeneration) { stale = true; return null; }
        ui.capabilities = result; ui.capabilityError = ''; return result;
      } catch (error) { if(project()!==original
          ||generation!==ui.capabilityGeneration)stale=true;else{ui.capabilityError=error.message||String(error);ui.capabilities=null;}return null; }
      finally { ui.loading = null; S().refresh(); if(stale&&S().state.view==='mixer')queueMicrotask(()=>{if(!ui.loading)refreshCapabilities();}); }
    })();
    return ui.loading;
  }


  async function transaction(label, edit, options = {}) {
    if (ui.application || ui.preparing) throw new Error('A routing change is already being applied.');
    if (!available()) throw new Error(unavailable());
    if (window.STStudio?.isEditingPlugin
        || window.STStudio?.isMovingEffect) throw new Error('Finish the plug-in rack change before changing routing.');
    const startingProject = project();
    const focusContext = {
      project: startingProject,
      view: S().state.view,
      owner: currentOwner(),
      node: document.activeElement,
      id: document.activeElement?.id,
      generation: ++ui.focusGeneration
    };
    ui.preparing = true;
    try {
      if (S().captureNativeEdits
          && await S().captureNativeEdits() === false) throw new Error('Current plug-in edits could not be captured. Routing was not changed.');
      if (project() !== startingProject) throw new Error('The project changed while preparing routing. Try again.');
      const synchronized = await S().syncEngineProject(() => project() === startingProject ? clone(startingProject) : null);
      if (synchronized === false || project() !== startingProject) throw new Error('The project changed while preparing routing. Try again.');
      if (window.STStudio?.isEditingPlugin
          || window.STStudio?.isMovingEffect) throw new Error('The plug-in rack changed while preparing routing. Try again.');
    } finally { ui.preparing = false; }
    const original = project(),
      snapshot = stable(original),
      nativeEditGeneration = window.STNKSControls?.editGeneration,
      owner = options.owner,
      entity = owner ? ownerEntity(owner, original) : null;
    if (owner && !entity) throw new Error('The selected channel is no longer available.');
    let finish; const gate = new Promise(resolve => { finish = resolve; }); ui.application = gate;
    const valid = () => project() === original && stable(original) === snapshot && (!owner || ownerEntity(owner, original) === entity) &&
      window.STNKSControls?.editGeneration === nativeEditGeneration && !window.STNKSControls?.hasPendingEdits;
    let nativeCommitted = false;
    try {
      const draft = clone(original);
      // An edit works on a routing table whose four collections exist.
      window.STGroups.ensureRouting(draft);
      edit(draft, { owner });
      S().refresh();
      const validation = await S().request('routing.validate', { project: draft });
      if (validation?.valid !== true) throw new Error(validation?.error || 'The audio host did not validate this routing change.');
      if (!valid()) throw new Error('The project changed before routing could be applied. Try the change again.');
      const response = await S().request('routing.apply', { project: draft }); nativeCommitted = true;
      if (!response?.project) throw new Error('The audio host did not return the committed routing project.');
      if (!valid()) throw new Error('The project changed while routing was being applied. The latest project has been preserved.');
      const committed = clone(response.project);
      S().mutate(label, current => {
        for (const name of Object.keys(current)) delete current[name];
        Object.assign(current, committed);
      }, { routingCommit: true });
      ui.capabilityGeneration++;ui.capabilities=response.capabilities||null;ui.capabilityError='';
      ui.status = statusValue(response.status);
      return response;
    } catch (error) {
      if (nativeCommitted) {
        ui.preparing = true; ui.application = null; finish();
        try { if (await S().syncEngineProject(() => clone(project()),
            { replace: true }) === false) throw new Error('Audio restoration was canceled because the session changed again.'); }
        catch (restoreError) { throw new Error(`${error.message} Restoring current audio state failed: ${restoreError.message}`); }
      }
      throw error;
    } finally {
      ui.application = null; ui.preparing = false; finish(); S().refresh();
      const sameWorkspace = () => project() === focusContext.project
          && S().state.view === focusContext.view
          && currentOwner() === focusContext.owner
          && ui.focusGeneration === focusContext.generation
          && !S().state.modal;
      // Native apply may return only the committed project. The controls are
      // temporarily absent until fresh capabilities arrive; focus them afterward.
      // A first wait can be an obsolete request from the preceding generation.
      for (let attempt = 0; attempt < 2
          && nativeCommitted
          && !ui.capabilities
          && !ui.capabilityError
          && S().state.view === 'mixer'
          && sameWorkspace(); attempt++) await refreshCapabilities();
      const canReturnFocus = () => {
        const active = document.activeElement;
        return sameWorkspace()
            && !ui.application
            && (!active || active === document.body || active.isConnected === false || active === focusContext.node || (active.id && [focusContext.id, options.focusID].includes(active.id)));
      };
      restoreFocus(options.focusID, canReturnFocus);
    }
  }

  function tapSelect(id, current, attributes, allowed = taps(), label = 'Send tap') {
    return `<select id="${id}" ${attributes} aria-label="${label}">${!allowed.includes(current)
        ? `<option value="${esc(current)}" selected disabled>${esc(tapName(current))} · unavailable</option>`
        : ''}${allowed.map(tap => `<option value="${tap}" ${current === tap ? 'selected' : ''}>${tapName(tap)}</option>`).join('')}</select>`;
  }

  // The mixer's status as the page uses it: the sidechain connections the host could not make, with its reason.
  function statusValue(value) {
    const source=value&&typeof value==='object'&&!Array.isArray(value)?value:{};
    const unavailable = (Array.isArray(source.unavailableSidechains) ? source.unavailableSidechains : []).filter(item => item
        && typeof item.id === 'string'
        && typeof item.reason === 'string').slice(0,
        128).map(item => ({
      id: item.id,
      reason: item.reason
    })).sort((a, b) => a.id.localeCompare(b.id) || a.reason.localeCompare(b.reason));
    return {unavailable};
  }
  function routeIssue(id) {
    return (ui.status?.unavailable || []).find(item => item.id === id)?.reason || '';
  }
  function routeStatus(id) {
    const message=routeIssue(id);
    return `<p class="routing-warning" data-routing-issue="${esc(id)}" ${message?'':'hidden'}>${esc(message)}</p>`;
  }
  function receiveStatus(value) {
    const next=statusValue(value);
    if(stable(next)===stable(ui.status))return;
    ui.status=next;
    if(S().state.view!=='mixer')return;
    // Patch only informational nodes. Rebuilding the mixer would discard live
    // numeric drafts or a fader gesture merely because a status tick arrived.
    for (const node of document.querySelectorAll('[data-routing-issue]')) {
      const reason = routeIssue(node.dataset.routingIssue);
      node.textContent = reason;
      node.hidden = !reason;
    }
  }

  function render(owner = currentOwner()) {
    const p = project(), endpoint = endpoints(p).find(item => item.owner === owner); if (!endpoint) return '';
    const token = key(owner), caps = ui.capabilities, pending = !!ui.application;
    const status = caps ? '' : ui.capabilityError || 'Asking the audio host about routing…';
    const sends = (routing(p).sends || []).filter(send => send.source === owner);
    const canSend = owner !== 'master' && !!caps && taps().length;
    return `<section class="routing-panel" data-routing-owner="${esc(owner)}" aria-busy="${pending}"><div class="routing-heading">`
        + `<h3>Routing</h3><span>${pending ? 'Applying…' : endpoint.type === 'master' ? 'Stereo output' : 'Standard mix'}</span>`
        + `</div>${status ? `<p class="routing-description">${esc(status)}</p>` : ''}<fieldset ${pending || !available() ? 'disabled' : ''}>
      ${owner === 'master' ? '<p class="routing-description">Master is the final audio destination.</p>' : `<div class="routing-output"><span>Output</span>${caps ? `<button id="routing-output-${token}" data-routing-action="output" aria-label="Output destination for ${esc(endpoint.name)}">${esc(channelName(activeOutput(owner,p),p))}<span>⌄</span>`
          + `</button>` : `<strong>${esc(channelName(activeOutput(owner,p),p))}</strong>`}</div>`}
      ${lineInputSection(owner,p)}
      ${sends.length || canSend ? `<div class="routing-subhead"><h4>Sends</h4>${canSend
          ? `<button id="routing-add-send-${token}" data-routing-action="add-send" ${routeCount(p) >= routeLimit() ? 'disabled' : ''}>+ Send</button>`
          : ''}</div>`
          + `<div class="routing-send-list">${sends.map(send => `<div class="routing-send" data-routing-send="${esc(send.id)}"><button id="routing-send-enable-${key(send.id)}" class="routing-toggle"`
          + ` data-routing-send-action="enable" aria-label="${send.enabled === false ? 'Enable' : 'Disable'} send to ${esc(channelName(send.destination,p))}"`
          + ` aria-pressed="${send.enabled !== false}" ${!canSend ? 'disabled' : ''}>${send.enabled === false ? '○' : '●'}</button>`
          + `<button id="routing-send-dest-${key(send.id)}" data-routing-send-action="destination" class="routing-destination" ${!canSend ? 'disabled' : ''}>${esc(channelName(send.destination,p))}</button>${tapSelect(`routing-send-tap-${key(send.id)}`,
              send.tap,`data-routing-send-field="tap" ${!canSend ? 'disabled' : ''}`)}<label `
          + `class="routing-gain">Gain<input id="routing-send-gain-${key(send.id)}" type="number" min="0" max="2" step=".01" value="${gain(send.gain)}"`
          + ` data-routing-send-field="gain" data-owner="${esc(owner)}" data-automate="send:${esc(send.id)}" data-name="Send to ${esc(channelName(send.destination,p))}"`
          + ` aria-label="Send gain to ${esc(channelName(send.destination,p))}" ${!canSend ? 'disabled' : ''}></label>`
          + `<button id="routing-send-remove-${key(send.id)}" class="routing-remove" data-routing-send-action="remove" aria-label="Remove send to ${esc(channelName(send.destination,p))}">×</button>${routeStatus(send.id)}</div>`).join('')}${!sends.length ? '<p class="routing-description">No custom sends.</p>' : ''}</div>` : ''}
      ${caps
          ? `<div class="routing-bus-actions"><button id="routing-new-group-${token}" data-routing-action="new-group">+ Group</button>`
              + `<button id="routing-new-return-${token}" data-routing-action="new-return">+ Return</button>${endpoint.type === 'group' || endpoint.type === 'return'
              ? `<button id="routing-remove-bus-${token}" data-routing-action="remove-bus">Remove bus…</button>`
              : ''}</div>`
          : ''}</fieldset></section>`;
  }

  const routeCount = p => (routing(p).sends || []).length + (routing(p).sidechains || []).length;
  const routeLimit = () => Math.min(128, number(ui.capabilities?.sends?.max,128));

  function choose(title, options, onChoose, focusID) {
    const openingProject = project();
    S().openModal(title, '<div class="routing-picker"><input id="routing-search" type="search" placeholder="Search channels…" aria-label="Search channels"'
        + ' autocomplete="off"><div id="routing-results" role="listbox" aria-label="Routing destinations"></div>'
        + '<p class="routing-description">↑ ↓ select · Return apply · Escape cancel</p></div>', '', {
      kind: 'routing-selector'
    });
    const input = document.getElementById('routing-search'), list = document.getElementById('routing-results');
    if (!input || !list) return;
    let selected = 0, filtered = options;
    const activate = index => {
      const item = filtered[index]; if (!item) return;
      S().closeModal(); restoreFocus(focusID);
      if (project() !== openingProject) { S().toast('The project changed. Choose a destination again.'); return; }
      perform(Promise.resolve().then(() => onChoose(item)));
    };
    const repaint = () => {
      const query = input.value.trim().toLowerCase();
      filtered = options.filter(option => `${option.name} ${option.type || ''}`.toLowerCase().includes(query));
      selected = Math.max(0,Math.min(selected,filtered.length-1));
      list.innerHTML = filtered.length ? filtered.map((option, index) => `<button id="routing-choice-${index}" type="button" role="option" tabindex="-1" aria-selected="${index === selected}"`
          + ` data-routing-choice="${index}"><span>${esc(option.name)}</span><small>${esc(option.type || '')}</small>`
          + `</button>`).join('') : '<p class="routing-description">No matching destinations.</p>';
      input.setAttribute('aria-activedescendant',filtered.length ? `routing-choice-${selected}` : '');
      list.querySelectorAll('[data-routing-choice]').forEach(button => button.addEventListener('click',
          () => activate(Number(button.dataset.routingChoice))));
    };
    input.setAttribute('role','combobox'); input.setAttribute('aria-controls','routing-results'); input.setAttribute('aria-expanded','true');
    input.addEventListener('input',() => { selected=0; repaint(); });
    const navigate = event => {
      if (event.key === 'Escape') { event.preventDefault();event.stopPropagation();S().closeModal();restoreFocus(focusID); }
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        event.stopPropagation();
        selected = Math.max(0, Math.min(filtered.length - 1, selected + (event.key === 'ArrowDown' ? 1 : -1)));
        repaint();
        document.getElementById(`routing-choice-${selected}`)?.scrollIntoView?.({ block: 'nearest' });
      }
      if (event.key === 'Enter') { event.preventDefault();event.stopPropagation();activate(selected); }
    };
    input.addEventListener('keydown',navigate);list.addEventListener('keydown',navigate);repaint();input.focus();
  }

  function setOutput(owner, focusID) {
    choose('Output destination', [{ owner: null, name: 'No output', type: 'Disconnected' },
        ...destinationOptions(owner, project(),
            { output: true })], destination => transaction('Change channel output', (p, { owner: source }) => {
      p.routing.outputs[source] = destination.owner;
    }, { owner, focusID }), focusID);
  }

  function editSend(owner,id,edit,label,focusID) {
    return transaction(label,
        p => { const send=p.routing.sends.find(item=>item.id===id);if(!send)throw new Error('This send no longer exists.');edit(send,p); },
        {owner,focusID});
  }

  function removeSendLanes(p,ids) {
    const targets=new Set([...ids].map(id=>`send:${id}`));
    for(const pattern of p.patterns||[])if(pattern.automation)pattern.automation=pattern.automation.filter(lane=>!targets.has(lane.target));
  }

  function addSend(owner,focusID) {
    if(routeCount(project())>=routeLimit()){S().toast('The project has reached its send and sidechain limit.');return;}
    choose('Send destination', destinationOptions(owner), destination => transaction('Add send', (p, { owner: source }) => {
      p.routing.sends.push({
        id: uid(),
        source,
        tap: taps().includes('postFader') ? 'postFader' : taps()[0],
        destination: destination.owner,
        gain: 0,
        enabled: true
      });
    }, { owner, focusID }), focusID);
  }

  async function createBus(kind, options = {}) {
    const openingProject=project(),owner=options.assignOwner||currentOwner(),openingEntity=ownerEntity(owner,openingProject);
    if(!openingEntity)return S().toast('This channel is no longer available.');
    if(!ui.capabilities)await refreshCapabilities();
    if(project()!==openingProject||ownerEntity(owner)!==openingEntity)return S().toast('The selected channel changed. Open bus creation again.');
    if(!available())return S().toast(unavailable());
    if(!['group','return'].includes(kind))return;
    const p=project();
    if((routing(p).buses||[]).length>=Math.min(window.TRACK_SANITY_LIMIT??Infinity,
        number(ui.capabilities.buses.max,window.TRACK_SANITY_LIMIT??Infinity)))return S().toast('This project has reached its bus limit.');
    const selected=endpoints(p).find(item=>item.owner===owner), canAssign=kind==='group'&&selected?.type==='track';
    S().openModal(`Create ${kind === 'group' ? 'group' : 'return'} bus`, `<div class="routing-dialog"><label>Name<input id="routing-bus-name" value="${kind === 'group' ? 'Group' : 'Return'} ${(routing(p).buses || []).filter(bus => bus.type === kind).length + 1}"`
        + ` maxlength="80"></label>${canAssign
            ? `<label class="routing-check"><input id="routing-bus-assign" type="checkbox">Route ${esc(selected.name)} into this bus</label>`
            : ''}<p `
        + `class="routing-description">The new bus outputs to Master. Add insert effects in its channel strip.</p>`
        + `</div>`, '<button id="routing-bus-cancel">Cancel</button><button id="routing-bus-create" class="primary">Create bus</button>', {
      kind: 'routing-bus'
    });
    document.getElementById('routing-bus-cancel')?.addEventListener('click',()=>S().closeModal());
    const submit = () => {
      const name = document.getElementById('routing-bus-name')?.value.trim(),
        assign = document.getElementById('routing-bus-assign')?.checked;
      if (!name) {
        document.getElementById('routing-bus-name')?.focus();
        return;
      }
      S().closeModal();
      if (project() !== p) return;
      let createdOwner;
      perform(transaction(`Create ${kind} bus`, (draft, { owner: source }) => {
        const id = uid();
        createdOwner = `bus:${id}`;
        draft.routing.buses.push({ id, name, type: kind, volume: 1, pan: 0, mute: false, solo: false });
        draft.routing.outputs[createdOwner] = 'master';
        if (assign) draft.routing.outputs[source] = createdOwner;
      }, { owner }).then(() => {
        if (project() === p) selectChannel(createdOwner, true);
      }));
    };
    document.getElementById('routing-bus-create')?.addEventListener('click',submit);
    document.getElementById('routing-bus-name')?.addEventListener('keydown', event => {
      if (event.key === 'Enter') {
        event.preventDefault();
        event.stopPropagation();
        submit();
      }
    });
  }

  function removeBus(owner) {
    const p=project(), bus=ownerEntity(owner,p);if(!bus||!owner.startsWith('bus:'))return;
    // A track group's bus goes with its group (Ungroup or Delete group): removing only the bus would leave the
    // group, its pattern header and its mute / solo without a channel.
    const group=window.STGroups?.groupOfBus?.(p,owner);
    if(group){
      if(window.STApp?.deleteGroup)void window.STApp.deleteGroup(group.id);
      else S().toast(`${bus.name} is the bus of the track group ${group.name}. Ungroup or delete the group to remove it.`);
      return;
    }
    const destination=activeOutput(owner,p), children=Object.entries(routing(p).outputs||{}).filter(([,to])=>to===owner).length;
    S().openModal('Remove bus', `<div class="routing-dialog"><p>Remove ${esc(bus.name)} and its insert effects?</p><p>${children} output connection${children === 1 ? '' : 's'}`
        + ` will move to ${esc(channelName(destination, p))}. Sends and sidechains connected to this bus will be removed.</p>`
        + `</div>`, '<button id="routing-delete-cancel">Cancel</button><button id="routing-delete-apply">Remove bus</button>', {
      kind: 'routing-delete'
    });
    document.getElementById('routing-delete-cancel')?.addEventListener('click',()=>S().closeModal());
    document.getElementById('routing-delete-apply')?.focus?.();
    document.getElementById('routing-delete-apply')?.addEventListener('click',()=>{S().closeModal();if(project()!==p)return;perform(transaction('Remove bus',draft=>{
      const r = draft.routing,
        replacement = activeOutput(owner, draft),
        removed = new Set((draft.audioPlugins || []).filter(plugin => pluginOwner(plugin) === owner).map(plugin => plugin.instanceID));
      r.buses = r.buses.filter(item => `bus:${item.id}` !== owner);
      delete r.outputs[owner];
      for (const source of Object.keys(r.outputs)) if (r.outputs[source] === owner) r.outputs[source] = replacement;
      const removedSends = new Set(r.sends.filter(send => send.source === owner || send.destination === owner).map(send => send.id));
      removeSendLanes(draft, removedSends);
      r.sends = r.sends.filter(send => !removedSends.has(send.id));
      r.sidechains = r.sidechains.filter(route => route.source !== owner && !removed.has(route.destination));
      draft.audioPlugins=(draft.audioPlugins||[]).filter(plugin=>pluginOwner(plugin)!==owner);
      for(const pattern of draft.patterns||[])if(pattern.automation)pattern.automation=pattern.automation.filter(lane=>lane.owner!==owner);
    },{owner}).then(()=>{if(project()===p)selectChannel(destination||'master',true);}));});
  }

  function bind(owner = currentOwner()) {
    const panel=[...document.querySelectorAll('[data-routing-owner]')].find(element=>element.dataset.routingOwner===owner);if(!panel)return;
    if(!ui.capabilities&&!ui.loading&&!ui.capabilityError)refreshCapabilities();
    if(panel.dataset.routingBound==='true')return;panel.dataset.routingBound='true';
    panel.querySelectorAll('[data-routing-action]').forEach(button=>button.addEventListener('click',()=>{
      const action=button.dataset.routingAction;
      if(action==='output')setOutput(owner,button.id);if(action==='add-send')addSend(owner,button.id);
      if (action === 'new-group' || action === 'new-return') createBus(action === 'new-group' ? 'group' : 'return', { assignOwner: owner });
      if (action === 'remove-bus') removeBus(owner);
    }));
    panel.querySelectorAll('[data-routing-send]').forEach(row=>{
      const id=row.dataset.routingSend;
      row.querySelectorAll('[data-routing-send-action]').forEach(button=>button.addEventListener('click',()=>{
        if (button.dataset.routingSendAction === 'remove') perform(transaction('Remove send', p => {
          p.routing.sends = p.routing.sends.filter(send => send.id !== id);
          removeSendLanes(p, new Set([id]));
        }, { owner, focusID: `routing-add-send-${key(owner)}` }));
        if(button.dataset.routingSendAction==='enable')perform(editSend(owner,id,send=>{send.enabled=send.enabled===false;},'Toggle send',button.id));
        if (button.dataset.routingSendAction === 'destination') choose('Send destination', destinationOptions(owner, project(), {
          omitID: id
        }), destination => editSend(owner, id, send => {
          send.destination = destination.owner;
        }, 'Change send destination', button.id), button.id);
      }));
      row.querySelectorAll('[data-routing-send-field]').forEach(input => input.addEventListener('change', () => {
        if (input.dataset.routingSendField === 'gain' && !typedGain(input.value)) { input.value = String(gain(routing().sends?.find(send => send.id === id)?.gain)); return; }
        perform(editSend(owner, id, send => {
          send[input.dataset.routingSendField] = input.dataset.routingSendField === 'gain' ? gain(input.value) : input.value;
        }, 'Adjust send', input.id));
      }));
    });
  }

  function renderPluginSidechain(plugin) {
    if(!plugin?.instanceID)return '';
    const reads = readsSidechain(plugin.instanceID),
      saved = routing().sidechains.filter(route => route.destination === plugin.instanceID),
      supported = available();
    if(!saved.length&&(!supported||!reads||!keyTaps().length))return '';
    const owner=pluginOwner(plugin), token=key(plugin.instanceID);
    return `<section class="routing-sidechains" data-routing-plugin="${esc(plugin.instanceID)}" data-routing-plugin-owner="${esc(owner)}">`
        + `<div class="routing-subhead"><h4>Sidechain input</h4>${supported&&reads&&keyTaps().length?`<button id="routing-key-add-${token}" data-routing-key-action="add" ${ui.application||routeCount(project())>=routeLimit()?'disabled':''}>+ `
        + `Key input</button>`:''}</div>`
        + `<fieldset ${ui.application||!available()?'disabled':''}>${saved.map(route=>{
      const advertised = reads;
      return `<div class="routing-key" data-routing-key="${esc(route.id)}"><span class="routing-key-input">Key<small>stereo</small></span><button id="routing-key-source-${key(route.id)}" data-routing-key-action="source" ${!supported||!advertised?'disabled':''}>${esc(channelName(route.source))}</button>${tapSelect(`routing-key-tap-${key(route.id)}`,
              route.tap,`data-routing-key-field="tap" ${!supported||!advertised?'disabled':''}`,routeTaps(route,plugin),'Sidechain tap')}<label `
          + `class="routing-gain">Gain<input id="routing-key-gain-${key(route.id)}" type="number" min="0" max="2" step=".01" value="${gain(route.gain)}"`
          + ` data-routing-key-field="gain" ${!supported||!advertised?'disabled':''}></label><button id="routing-key-enable-${key(route.id)}"`
          + ` data-routing-key-action="enable" aria-pressed="${route.enabled!==false}" ${!supported||!advertised?'disabled':''}>${route.enabled===false?'Off':'On'}</button>`
          + (`<button id="routing-key-remove-${key(route.id)}" data-routing-key-action="remove" class="routing-remove" aria-label="Remove sidechain `
          + `connection">×</button>${!advertised?'<p class="routing-warning">This insert is not reading a sidechain now. The connection is kept until it does or you '
          + 'remove it.</p>':routeStatus(route.id)}</div>`);
    }).join('')}${!saved.length?'<p class="routing-description">No external key signal.</p>':''}</fieldset></section>`;
  }

  function bindPluginSidechains(owner = currentOwner()) {
    for(const section of document.querySelectorAll('[data-routing-plugin]')){
      if(section.dataset.routingPluginOwner!==owner||section.dataset.routingBound==='true')continue;
      section.dataset.routingBound='true';const instanceID=section.dataset.routingPlugin;
      const currentPlugin=(p=project())=>(p.audioPlugins||[]).find(plugin=>plugin.instanceID===instanceID);
      section.querySelector('[data-routing-key-action="add"]')?.addEventListener('click',()=>{
        if(!available()||!keyTaps().length)return;
        const focusID=`routing-key-add-${key(instanceID)}`;
        {
          const tapOptions = [...keyTaps()].sort((a, b) => a === 'postFader' ? -1 : b === 'postFader' ? 1 : 0).map(tap => ({
            tap,
            name: tapName(tap),
            type: tap === 'preInserts'
                ? 'Before the source channel’s insert chain'
                : tap === 'preFader'
                ? 'After inserts, before the source fader'
                : 'After the source channel’s fader'
          }));
          choose('Sidechain tap',tapOptions,choice=>{
            choose('Sidechain source',sourceOptions(owner,project(),undefined,
                {tap:choice.tap,plugin:currentPlugin()}),source=>transaction('Add sidechain',p=>{
              if (!readsSidechain(instanceID)) throw new Error('This insert no longer reads a sidechain.');
              if (!keyTaps().includes(choice.tap) || !currentPlugin(p) || !sourceOptions(owner, p, undefined, {
                tap: choice.tap,
                plugin: currentPlugin(p)
              }).some(item => item.owner === source.owner)) throw new Error('This sidechain source or tap is no longer available. Choose it again.');
              p.routing.sidechains.push({
                id: uid(),
                source: source.owner,
                tap: choice.tap,
                destination: instanceID,
                gain: 1,
                enabled: true
              });
            },{owner,focusID}),focusID);
          },focusID);
        }
      });
      for(const row of section.querySelectorAll('[data-routing-key]')){
        const id = row.dataset.routingKey,
          edit = (label, fn, focusID) => transaction(label, p => {
            const route = p.routing.sidechains.find(item => item.id === id);
            if (!route) throw new Error('The sidechain connection no longer exists.');
            fn(route, p);
          }, { owner, focusID });
        row.querySelectorAll('[data-routing-key-action]').forEach(button=>button.addEventListener('click',()=>{
          const action=button.dataset.routingKeyAction;
          if (action === 'remove') perform(transaction('Remove sidechain', p => {
            p.routing.sidechains = p.routing.sidechains.filter(route => route.id !== id);
          }, { owner, focusID: `routing-key-add-${key(instanceID)}` }));
          if(action==='enable')perform(edit('Toggle sidechain',route=>{route.enabled=route.enabled===false;},button.id));
          if(action==='source'){
            const route=routing().sidechains.find(item=>item.id===id);if(!route)return;
            choose('Sidechain source',sourceOptions(owner,project(),id,
                {tap:route.tap,plugin:currentPlugin()}),source=>edit('Change sidechain source',(item,p)=>{
              if (!sourceOptions(owner, p, id, {
                tap: item.tap,
                plugin: currentPlugin(p)
              }).some(option => option.owner === source.owner)) throw new Error('This sidechain source is no longer available. Choose it again.');
              item.source=source.owner;
            },button.id),button.id);
          }
        }));
        row.querySelectorAll('[data-routing-key-field]').forEach(input=>input.addEventListener('change',()=>{
          if(input.dataset.routingKeyField==='gain'&&!typedGain(input.value)){input.value=String(gain(routing().sidechains?.find(route=>route.id===id)?.gain));return;}
          perform(edit('Adjust sidechain',(route,p)=>{
            const field=input.dataset.routingKeyField;
            if (field === 'tap'
                && (!currentPlugin(p) || !routeTaps(route, currentPlugin(p), p).includes(input.value))) throw new Error('This tap is not available for the selected sidechain source.');
            route[field]=field==='gain'?gain(input.value):input.value;
          },input.id));
        }));
      }
    }
  }

  function event(type,payload={}) {
    if(type==='status'){
      if(Object.hasOwn(payload,'routing'))receiveStatus(payload.routing);
      return;
    }
    // The page replaced its project (studio.js passes the notice on): what the host said about the old one is void.
    if(type==='project.loaded'){ui.capabilityGeneration++;ui.status=null;ui.capabilities=null;ui.capabilityError='';}
  }

  async function showOutput(owner = currentOwner(),focusID) {
    const openingProject=project(),openingEntity=ownerEntity(owner);
    if(!ui.capabilities)await refreshCapabilities();
    if(project()!==openingProject||ownerEntity(owner)!==openingEntity)return S().toast('The selected channel changed. Choose its output again.');
    if(!available())return S().toast(unavailable());
    if(owner==='master')return S().toast('Master is the final audio destination.');
    if(!ownerEntity(owner))return S().toast('This channel is no longer available.');
    setOutput(owner,focusID||`routing-output-${key(owner)}`);
  }

  async function showAddSend(owner = currentOwner(),focusID) {
    const openingProject=project(),openingEntity=ownerEntity(owner);
    if(!ui.capabilities)await refreshCapabilities();
    if(project()!==openingProject||ownerEntity(owner)!==openingEntity)return S().toast('The selected channel changed. Add its send again.');
    if(!available()||!taps().length)return S().toast(unavailable());
    if(owner==='master')return S().toast('Master cannot feed a send.');
    if(!ownerEntity(owner))return S().toast('This channel is no longer available.');
    addSend(owner,focusID||`routing-add-send-${key(owner)}`);
  }

  function selectChannel(owner,focus=false) {
    if(window.STStudio?.selectOwner)window.STStudio.selectOwner(owner);else S().state.mixerOwner=owner;
    S().refresh();
    if(focus)queueMicrotask(()=>window.STStudio?.focusSelectedChannel?.());
  }

  function show(owner = currentOwner()) {
    if(!ownerEntity(owner))return S().toast('This channel is no longer available.');
    S().setView?.('mixer');
    selectChannel(owner);
    document.getElementById(`routing-output-${key(owner)}`)?.focus();
    if(!ui.capabilities)refreshCapabilities();
  }

  function keyboard(event) {
    if(event.key==='Escape'&&S().state.modal==='routing-selector'){event.preventDefault();S().closeModal();return true;}
    return false;
  }

  window.STRouting = {
    render,
    bind,
    createBus,
    removeBus,
    renderPluginSidechain,
    bindPluginSidechains,
    available,
    canOutput,
    canSend,
    event,
    endpoints,
    ownerEntity,
    channelName,
    transaction,
    refreshCapabilities,
    destinationOptions,
    sourceOptions,
    showOutput,
    showAddSend,
    show,
    key: keyboard,
    get capabilities() {
      return ui.capabilities ? clone(ui.capabilities) : null;
    },
    get isPreparing() {
      return ui.preparing;
    },
    get isApplying() {
      return !!ui.application;
    },
    waitForApply: () => ui.application || Promise.resolve()
  };
})();
