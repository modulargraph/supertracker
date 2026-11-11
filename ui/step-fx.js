/* Step effects are pattern data. Choosing a parameter never changes its live value. */
(function () {
  'use strict';
  const S = () => window.ST, state = () => S().state;
  const $ = id => document.getElementById(id), esc = value => S().escapeHTML(value);
  const { clone: copy } = window.STUtil, signature = value => JSON.stringify(value ?? null);
  const clamp = (value, min, max) => Math.max(min, Math.min(max, value));
  const labels = {
    volume: 'Volume',
    pan: 'Pan',
    pitch: 'Pitch',
    cutoff: 'Filter cutoff',
    resonance: 'Filter resonance',
    delay: 'Delay send',
    reverb: 'Reverb send',
    probability: 'Probability',
    ratchet: 'Ratchet',
    reverse: 'Reverse',
    offset: 'Sample offset',
    glide: 'Pitch glide',
    microtiming: 'Microtiming',
    arpeggio: 'Arpeggio',
    slice: 'Sample slice',
    position: 'Wavetable position'
  };
  const noteEffects = new Set(['probability', 'glide', 'microtiming', 'arpeggio', 'slice']);
  let current = null;
  const canonicalAddress = value => typeof value === 'string'
      && value.length <= 20
      && /^(0|[1-9][0-9]*)$/.test(value)
      && BigInt(value) <= 18446744073709551615n;
  const instanceID = value => typeof value === 'string' && value.length > 0 && value.length <= 128;
  const owner = (project, track) => `track:${project.tracks[track]?.routingID}`;
  // A track also runs its assigned instrument's insert chain (instrument-effects.js); locks on it reach this track's instance.
  function onTrack(plugin, project, track) {
    return plugin.owner === owner(project, track) || plugin.owner === `instrument:${project.tracks[track]?.instrument ?? 0}`;
  }
  function resolvePlugin(project, track, id) {
    if (!instanceID(id)) return null;
    const matches = project.audioPlugins.filter(plugin => plugin.instanceID === id);
    return matches.length === 1 && onTrack(matches[0], project, track) && ['instrument', 'effect'].includes(matches[0].kind) ? matches[0] : null;
  }
  function context() {
    const s = state(),
      project = s.project,
      pattern = S().pattern(),
      track = project.tracks[s.track],
      rootCell = pattern.steps[s.track]?.[s.row] ?? null;
    return { project, pattern, track, trackIndex: s.track, row: s.row, cell: rootCell, rootCell, signature: signature(rootCell) };
  }
  function unchanged(entry) {
    const s = state();
    return s.project === entry.project
        && S().pattern() === entry.pattern
        && s.track === entry.trackIndex
        && s.row === entry.row
        && entry.project.tracks[entry.trackIndex] === entry.track
        && entry.pattern.steps[entry.trackIndex]?.[entry.row] === entry.rootCell
        && signature(entry.rootCell) === entry.signature;
  }
  function live(entry) {
    if (current !== entry || state().modal !== 'stepFX' || $('step-fx-dialog') !== entry.element) return false;
    if (unchanged(entry)) return true;
    $('step-fx-status').textContent = 'The original step changed. Cancel and reopen the picker on the step you want to edit.';
    for (const id of ['step-fx-apply', 'step-fx-remove', 'step-fx-value']) $(id).disabled = true;
    return false;
  }
  function blocked() {
    return window.STBusy.any(['session', 'kits', 'export', 'recording', 'pluginEdit', 'effectMovePrep', 'routingPrep']);
  }
  function parameterOf(fx, project = state().project, track = state().track) {
    return fx?.type === 'plugin'
        ? resolvePlugin(project, track, fx.instanceID)?.parameters?.find(parameter => parameter.address === fx.address)
        : null;
  }
  function range(fx) {
    return fx?.type === 'plugin' ? [0, 1, window.STFine?.parameterStep(parameterOf(fx)) ?? .001] : S().FX[fx?.type] || [-1e9, 1e9, .01];
  }
  function label(fx) { return fx?.type === 'plugin' ? fx.name || 'Plug-in parameter' : labels[fx?.type] || fx?.type || 'Choose effect…'; }
  function unavailable(fx, project = state().project, track = state().track) {
    return fx?.type === 'plugin' && (!canonicalAddress(fx.address) || !resolvePlugin(project, track, fx.instanceID));
  }
  function renderField(number, fx) {
    const [min, max, step] = range(fx), missing = unavailable(fx), title = `${label(fx)}${missing ? ' · unavailable; retained in the pattern' : ''}`;
    return `<span class="step-fx-field"><span>FX ${number}</span><button id="step-fx${number}" class="step-fx-open ${missing ? 'step-fx-missing' : ''}"`
        + ` title="${esc(title)}" aria-label="Choose step effect ${number}: ${esc(title)}">${esc(label(fx))}${missing ? ' · unavailable' : ''}</button>`
        + `<input id="step-fx${number}-value" aria-label="Effect ${number} value" type="number" min="${min}" max="${max}" step="${step}"`
        + ` value="${esc(fx?.value ?? .5)}" ${fx ? '' : 'disabled'}></span>`;
  }
  function write(entry, number, fx) {
    if (!unchanged(entry) || blocked() || window.STBusy.any(['effectMove', 'routing'])) return false;
    let committed = false;
    S().mutate(fx ? 'set step effect' : 'remove step effect', () => {
      const steps = entry.pattern.steps[entry.trackIndex];
      if (!steps[entry.row] && fx) steps[entry.row] = { note: -3, instrument: entry.track.instrument, velocity: 100, gate: 0 };
      if (steps[entry.row]) steps[entry.row]['fx' + number] = fx;
      committed = true;
    });
    return committed;
  }
  function bind() {
    for (const number of [1, 2]) {
      const button = $('step-fx' + number), input = $('step-fx' + number + '-value'), entry = context(), fx = entry.cell?.['fx' + number];
      if (button) button.onclick = () => show({ fx: number });
      if (input && S().bindScalarField) {
        S().bindScalarField(input,'set step effect',()=>{
          const [min,max]=range(fx),value=Number(input.value);
          const next = fx.type === 'plugin'
              ? window.STFine?.snapParameter(value, parameterOf(fx, entry.project, entry.trackIndex)) ?? clamp(value, min, max)
              : clamp(value, min, max);
          entry.cell['fx'+number]={...fx,value:next};
          entry.signature=signature(entry.rootCell);
        },()=>!!fx&&unchanged(entry)&&!blocked()&&!window.STBusy.isBusy('effectMove')&&!window.STBusy.isBusy('routing'));
        continue;
      }
      if (input) input.onchange = () => {
        const value = Number(input.value);
        if (!fx || input.value.trim() === '' || !Number.isFinite(value)) { S().refresh(); return; }
        const [min, max] = range(fx);
        const next = fx.type === 'plugin'
            ? window.STFine?.snapParameter(value, parameterOf(fx, entry.project, entry.trackIndex)) ?? clamp(value, min, max)
            : clamp(value, min, max);
        if (!write(entry, number, { ...fx, value: next })) {
          S().toast('The step changed or the project is busy. Select the step again before editing its effect.');
          S().refresh();
        }
      };
    }
  }
  function ranking(entry, fx) {
    let rank = 0;
    for (const pattern of entry.project.patterns) for (const step of pattern.steps[entry.trackIndex]) for (const key of ['fx1', 'fx2']) {
      const old = step?.[key];
      if (old && old.type === fx.type && (fx.type !== 'plugin' || old.instanceID === fx.instanceID && old.address === fx.address)) rank = 2;
    }
    if (fx.type === 'plugin') {
      const plugin = resolvePlugin(entry.project, entry.trackIndex, fx.instanceID);
      if (plugin
          && entry.pattern.automation.some(lane => lane.owner === plugin.owner
              && lane.target === `plugin:${plugin.kind}:${plugin.slot}:${fx.address}`)) rank = Math.max(rank, 1);
    }
    return rank;
  }
  const keyOf = fx => fx.type === 'plugin' ? `plugin:${fx.instanceID}:${fx.address}` : fx.type;
  // The typed code (app-entry.js) beside each built-in effect: type it in an FX column, e.g. R04.
  const codeOf = type => window.STApp?.fxCode?.(type)?.code || '';
  const codeBadge = item => item.fx?.type && item.fx.type !== 'plugin' && codeOf(item.fx.type)
      ? `<kbd class="step-fx-code-badge" title="Type ${codeOf(item.fx.type)} and a value in an FX column">${codeOf(item.fx.type)}</kbd>` : '';
  function builtins(entry) {
    return Object.entries(S().FX).map(([type, limits]) => ({ key: type, name: labels[type] || type, group: 'Step effects', search: codeOf(type), fx: { type, value: type === 'cutoff'
        ? 1000
        : clamp(.5, limits[0], limits[1]) }, limits, rank: ranking(entry, { type }) }));
  }
  function filtered(entry) {
    const words = entry.query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
    return entry.items.filter(item => words.every(word => `${item.name} ${item.group} ${item.search || ''}`.toLocaleLowerCase().includes(word))).sort((a, b) => b.rank - a.rank
        || a.group.localeCompare(b.group)
        || a.name.localeCompare(b.name));
  }
  function paintResults(entry) {
    if (!live(entry)) return;
    const list = $('step-fx-results'),
      items = filtered(entry),
      focusedKey = Array.from(list.querySelectorAll('[data-fx-result]')).includes(document.activeElement)
          ? document.activeElement.dataset.fxKey
          : null,
      scrollTop = list.scrollTop;
    if (!items.some(item => item.key === entry.highlight)) entry.highlight = items[0]?.key;
    list.innerHTML = items.map((item, index) => `<button id="step-fx-result-${index}" class="step-fx-result ${item.key === entry.highlight ? 'selected' : ''}" data-fx-result="${index}"`
        + ` data-fx-key="${esc(item.key)}" role="option" aria-selected="${item.key === entry.highlight}" tabindex="${item.key === entry.highlight ? 0 : -1}">`
        + `<span>${codeBadge(item)}<strong>${esc(item.name)}</strong><small>${esc(item.group)}</small></span><span class="step-fx-result-hint">${item.unavailable ? 'Unavailable' : item.rank === 2 ? 'Used' : item.rank === 1 ? 'Automated' : ''}</span>`
        + `</button>`).join('') || '<p class="step-fx-empty">No matching effects or exposed parameters.</p>';
    list.querySelectorAll('[data-fx-result]').forEach(button => {
      const item = items[Number(button.dataset.fxResult)];
      button.onclick = () => choose(entry, item);
      button.onfocus = () => {
        entry.highlight = item.key;
        list.querySelectorAll('[data-fx-result]').forEach(other => {
          const selected = other === button;
          other.tabIndex = selected ? 0 : -1;
          other.classList.toggle('selected', selected);
          other.setAttribute('aria-selected', String(selected));
        });
      };
      button.onkeydown = event => {
        if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault();
          const index = items.indexOf(item),
            next = event.key === 'Home'
                ? 0
                : event.key === 'End'
                ? items.length - 1
                : clamp(index + (event.key === 'ArrowDown' ? 1 : -1), 0, items.length - 1);
          $('step-fx-result-' + next)?.focus(); $('step-fx-result-' + next)?.scrollIntoView({ block: 'nearest' });
        } else if (event.key === 'Enter') { event.preventDefault(); choose(entry, item); }
      };
    });
    list.scrollTop = scrollTop;
    if (focusedKey) {
      const index = items.findIndex(item => item.key === focusedKey);
      (index < 0 ? $('step-fx-search') : $('step-fx-result-' + index))?.focus({ preventScroll: true });
    }
  }
  function paintChoice(entry, focus = false) {
    if (!live(entry)) return;
    const item = entry.choice, input = $('step-fx-value');
    $('step-fx-choice').textContent = item ? `${item.name} · ${item.group}` : 'Choose an effect or parameter';
    $('step-fx-choice-note').textContent = item?.unavailable ? 'This saved target is unavailable. Its exact identity is retained; editing its value does not reconnect it to another plug-in.' : item?.fx.type === 'plugin' ? 'Normalized value from 0 to 1. This step overrides its automation lane for one row; without a lane, the last value is held.' : item?.fx.type === 'ratchet' ? 'Repeats the last note or chord at this many hits per row, including empty rows. A new note replaces the chord; another effect or note-off '
        + 'stops the repeats. 1 = off. Voice mode controls overlap: Polyphonic layers voices; Monophonic retriggers; Legato can keep the sample '
        + 'running.' : item?.fx.type === 'reverse' ? '1 plays backward; 0 plays forward. Without a note, changes the direction of all playing sample voices from this lane/Sound, including '
        + 'release tails, without restarting them. Applies to sample playback and slices.' : item?.fx.type === 'offset' ? 'Sample position from 0 to 1 within the selected region, measured from the playback direction. Without a note, moves the currently '
        + 'playing sample playhead. Granular and wavetable modes use their position parameter.' : item?.fx.type === 'position' ? 'Wavetable position from 0 to 1 (0–100%) for samplers in Wavetable mode. '
        + `Moves the playing voices of this track to the new frame${entry.value != null && Number.isFinite(Number(entry.value)) ? ` · ${Math.round(clamp(Number(entry.value), 0, 1) * 100)}%` : ''}.` : item?.fx.type === 'arpeggio' ? 'Arpeggio cycles root, +x and +y semitones: value = 16 × x + y (cells show it as hex xy). 55 = +3 +7 minor, 71 = +4 +7 major, 0 = off. '
        + 'Needs a note on this row.' : item && noteEffects.has(item.fx.type) ? 'This effect modifies a note trigger. Add a note on this row to hear it.' : 'A row without a note changes the effect without playing a new note.';
    const limits = item?.limits || [0, 1, .001];
    [input.min, input.max, input.step] = limits; input.value = entry.value ?? ''; input.disabled = !item || entry.applying;
    $('step-fx-slot').disabled = entry.applying;
    $('step-fx-apply').disabled = !item || entry.applying;
    if (focus && item) { input.focus(); input.select(); }
  }
  function choose(entry, item) {
    if (!live(entry) || entry.applying) return;
    entry.choice = item;
    entry.valueDirty = false;
    entry.value = item.key === keyOf(entry.existing || {}) ? entry.existing.value : item.fx.value;
    paintChoice(entry, true);
  }
  function payload(plugin, entry) {
    return {
      owner: plugin.owner,
      kind: plugin.kind,
      slot: plugin.slot,
      instanceID: plugin.instanceID
    };
  }
  function validParameter(parameter) { return canonicalAddress(parameter.address) && parameter.writable === true && parameter.automatable === true; }
  function parameterList(result) { return Array.isArray(result) ? result : result?.parameters || []; }
  function locate(entry, snapshot) {
    const plugin = snapshot.plugin, found = resolvePlugin(entry.project, entry.trackIndex, snapshot.instanceID);
    return found === plugin && pluginIdentity(found) === snapshot.signature ? found : null;
  }
  const pluginIdentity = plugin => signature([plugin.instanceID, plugin.kind, plugin.slot, plugin.component, plugin.owner,
      plugin.bundlePath]);
  const pluginSnapshot = plugin => ({ plugin, instanceID: plugin.instanceID, signature: pluginIdentity(plugin) });
  async function load(entry) {
    try {
      await S().waitForGraphEdit?.();
      if (!live(entry)) return;
      if (blocked()) throw new Error('Wait for the current project or plug-in change, then reopen the picker.');
      const plugins = entry.project.audioPlugins.filter(plugin => onTrack(plugin, entry.project, entry.trackIndex));
      const errors = [];
      for (const original of plugins) {
        await S().waitForGraphEdit?.();
        if (!live(entry)) return;
        if (blocked()) throw new Error('Wait for the current project or plug-in change, then reopen the picker.');
        const plugin = resolvePlugin(entry.project, entry.trackIndex, original.instanceID);
        if (!plugin) { errors.push(`${original.name || 'Plug-in'} has no unique saved instance identity.`); continue; }
        const requestTarget = pluginSnapshot(plugin);
        try {
          const result = await S().request('plugin.parameters', payload(plugin, entry));
          await S().waitForGraphEdit?.();
          if (!live(entry)) return;
          if (!locate(entry, requestTarget)) { errors.push('The plug-in chain changed. Reopen to refresh its parameters.'); continue; }
          const metadata = window.STNKSControls?.automationMetadata?.(plugin);
          for (const parameter of parameterList(result).filter(validParameter)) {
            const mapping = metadata?.get(parameter.address),
              fx = {
                type: 'plugin',
                instanceID: plugin.instanceID,
                address: parameter.address,
                value: clamp(Number(parameter.normalized) || 0, 0, 1),
                name: mapping?.name || parameter.name || parameter.address
              };
            const item = {
              key: keyOf(fx),
              name: fx.name,
              group: `${plugin.name || 'Plug-in'}${mapping?.group
                  ? ' · ' + mapping.group
                  : plugin.kind === 'instrument'
                  ? ' · Instrument'
                  : ' · Insert ' + (Number(plugin.slot) + 1)}`,
              search: `${mapping?.search || ''} ${parameter.name || ''} ${parameter.address}`,
              fx,
              parameter,
              limits: [0, 1, window.STFine?.parameterStep(parameter) ?? .001],
              rank: ranking(entry, fx)
            };
            entry.items = entry.items.filter(old => old.key !== item.key); entry.items.push(item);
          }
        } catch (error) { errors.push(`${plugin.name || 'Plug-in'}: ${error.message}`); }
      }
      if (!live(entry)) return;
      const requested = entry.requested || entry.existing,
        item = requested && entry.items.find(candidate => candidate.key === keyOf({ type: 'plugin', ...requested }));
      if (item && (!entry.choice || entry.choice.unavailable && entry.choice.key === item.key)) {
        entry.choice = item;
        entry.highlight = item.key;
        if (!entry.valueDirty) entry.value = entry.existing && keyOf(entry.existing) === item.key ? entry.existing.value : item.fx.value;
      }
      if (entry.requested && !item) errors.unshift('This parameter is not currently exposed as writable and automatable on this track.');
      $('step-fx-status').textContent = errors.join(' ')
          || 'Search step effects and exposed plug-in parameters. ↑ ↓ browse · Enter choose · Tab move controls';
      paintResults(entry); paintChoice(entry);
    } catch (error) { if (live(entry)) $('step-fx-status').textContent = error.message; }
  }
  async function apply(entry, remove = false) {
    if (entry.applying || !live(entry)) return;
    const input = $('step-fx-value'), item = entry.choice;
    let value = Number(input.value);
    if (!remove
        && (!item || input.value.trim() === '' || !Number.isFinite(value) || value < item.limits[0] || value > item.limits[1])) { $('step-fx-status').textContent = 'Enter a value within the displayed range.'; input.focus(); return; }
    entry.value = value; entry.applying = true; paintChoice(entry); $('step-fx-remove').disabled = true;
    try {
      await S().waitForGraphEdit?.();
      if (!live(entry)) return;
      if (blocked()) throw new Error('Wait for the current project or plug-in change to finish.');
      if (!remove && item.fx.type === 'plugin' && !item.unavailable) {
        const plugin = resolvePlugin(entry.project, entry.trackIndex, item.fx.instanceID);
        if (!plugin) throw new Error('This plug-in is no longer uniquely available on the original track.');
        const before = pluginSnapshot(plugin), result = await S().request('plugin.parameters', payload(plugin, entry));
        await S().waitForGraphEdit?.();
        if (!live(entry)) return;
        const parameter = parameterList(result).find(parameter => validParameter(parameter) && parameter.address === item.fx.address);
        if (!locate(entry, before) || !parameter) throw new Error('This parameter changed or is no longer writable and automatable. Reopen the picker.');
        value = window.STFine?.snapParameter(value, parameter) ?? value;
      }
      if (!live(entry)) return;
      const fx = remove ? null : { ...copy(item.fx), value };
      if (signature(entry.existing) !== signature(fx)
          && !write(entry, entry.number, fx)) throw new Error('The project changed. Reopen this step to edit its effect.');
      current = null; S().closeModal();
    } catch (error) { if (live(entry)) $('step-fx-status').textContent = error.message; }
    finally { entry.applying = false; if (live(entry)) { paintChoice(entry); $('step-fx-remove').disabled = !entry.existing; } }
  }
  function selectSlot(entry, number) {
    entry.number = number; entry.existing = entry.cell?.['fx' + number] ? copy(entry.cell['fx' + number]) : null;
    entry.choice = null; entry.value = null; entry.valueDirty = false; entry.requested = null;
    if (entry.existing) {
      const key = keyOf(entry.existing); let item = entry.items.find(candidate => candidate.key === key);
      if (!item) {
        item = {
          key,
          name: label(entry.existing),
          group: 'Saved step target',
          fx: copy(entry.existing),
          limits: range(entry.existing),
          rank: 3,
          unavailable: true
        };
        entry.items.push(item);
      }
      entry.choice = item; entry.value = entry.existing.value;
    }
    entry.highlight = entry.choice?.key || ''; $('step-fx-remove').disabled = !entry.existing; paintResults(entry); paintChoice(entry);
  }
  async function show(options = {}) {
    if (blocked()) { S().toast('Wait for the current project, recording, or plug-in change to finish.'); return false; }
    const entry = {
      ...context(),
      number: options.fx === 2 || !options.fx && state().column === 4 ? 2 : 1,
      query: '',
      items: [],
      highlight: '',
      choice: null,
      applying: false
    };
    entry.items = builtins(entry);
    S().openModal('Step effect', `<div id="step-fx-dialog" class="step-fx-dialog"><div class="step-fx-location"><span>${esc(entry.track.name)} · ${esc(entry.pattern.name)}`
        + ` · step ${String(entry.row).padStart(2, '0')}</span><label>Column <select id="step-fx-slot" aria-label="Effect column">`
        + `<option value="1" ${entry.number === 1 ? 'selected' : ''}>FX 1</option><option value="2" ${entry.number === 2 ? 'selected' : ''}>FX `
        + `2</option></select></label></div><input id="step-fx-search" type="search" placeholder="Search effects, plug-ins or NKS controls…"`
        + ` aria-label="Search step effects" autocomplete="off"><p id="step-fx-status" class="step-fx-status" role="status">Reading exposed plug-in `
        + `parameters…</p><div id="step-fx-results" class="step-fx-results" role="listbox" aria-label="Effects and plug-in parameters">`
        + `</div><div class="step-fx-value-row"><label for="step-fx-value"><strong id="step-fx-choice">Choose an effect or parameter</strong>`
        + `<small id="step-fx-choice-note"></small></label><input id="step-fx-value" aria-label="Step effect value" type="number"`
        + ` min="0" max="1" step=".001" disabled></div></div>`, '<button id="step-fx-remove">Remove effect</button><span class="grow"></span><button id="step-fx-cancel">Cancel</button>'
        + '<button id="step-fx-apply" class="primary" disabled>Apply</button>', {
      kind: 'stepFX',
      className: 'step-fx-modal'
    });
    if (state().modal !== 'stepFX' || !$('step-fx-dialog')) return false;
    entry.element = $('step-fx-dialog'); current = entry; selectSlot(entry, entry.number); entry.requested = options.plugin;
    if (entry.requested) { entry.choice = null; entry.value = null; paintChoice(entry); }
    $('step-fx-cancel').onclick = () => S().closeModal();
    $('step-fx-apply').onclick = () => apply(entry);
    $('step-fx-remove').onclick = () => apply(entry, true);
    $('step-fx-slot').onchange = event => { if (!entry.applying) selectSlot(entry, Number(event.target.value)); };
    $('step-fx-value').oninput = event => { entry.value = event.target.value; entry.valueDirty = true; };
    $('step-fx-value').onkeydown = event => {
      if (event.key === 'Enter') { event.preventDefault(); apply(entry); }
      else if (!event.metaKey
          && !event.ctrlKey
          && window.STFine?.parameterSteps(entry.choice?.parameter)
          && ['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown'].includes(event.key)
          && live(entry)
          && !entry.applying) {
        event.preventDefault();
        const input = $('step-fx-value');
        entry.value = window.STFine.stepParameter(Number(input.value), event.key === 'ArrowUp' || event.key === 'PageUp' ? 1 : -1, {
          fine: event.altKey,
          coarse: event.shiftKey || event.key.startsWith('Page')
        }, entry.choice.parameter);
        entry.valueDirty = true; input.value = String(entry.value);
      }
    };
    $('step-fx-search').oninput = event => { entry.query = event.target.value; entry.highlight = ''; paintResults(entry); };
    $('step-fx-search').onkeydown = event => {
      const items = filtered(entry);
      if ((event.key === 'ArrowDown' || event.key === 'ArrowUp')
          && items.length) { event.preventDefault(); const index = event.key === 'ArrowDown'
              ? 0
              : items.length - 1; $('step-fx-result-' + index)?.focus(); }
      else if (event.key === 'Enter' && items.length) { event.preventDefault(); choose(entry,
          items.find(item => item.key === entry.highlight) || items[0]); }
    };
    $('step-fx-search').focus(); await load(entry); return live(entry);
  }
  window.STStepFX = { show, renderField, bind };
})();
