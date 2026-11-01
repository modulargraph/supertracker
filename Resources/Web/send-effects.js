/* Shared delay and reverb sends (Polyend-style): per-track send amounts and the two
   return processors, edited from one keyboard-first dialog. Data stays in the
   existing project fields (`tracks[i].delaySend/reverbSend`, `delay`, `reverb`);
   new return fields are `delay.sync` (division label), `delay.beats` (quarter notes,
   0 = free), `delay.tone` (0..1, 1 = open) and `reverb.predelay` (seconds). */
(() => {
  'use strict';
  const { clone } = typeof window !== 'undefined' && window.STUtil ? window.STUtil : require('./util.js');
  const DIVISIONS = [['1/32', .125], ['1/16T', 1 / 6], ['1/16', .25], ['1/16.', .375], ['1/8T', 1 / 3], ['1/8', .5], ['1/8.', .75], ['1/4T', 2 / 3],
      ['1/4', 1], ['1/4.', 1.5], ['1/2T', 4 / 3], ['1/2', 2], ['1/2.', 3], ['1/1', 4]];
  const DELAY_DEFAULTS = { time: .375, feedback: .35, mix: .3, tone: 1, sync: null, beats: 0 };
  const REVERB_DEFAULTS = { size: .7, damping: .4, mix: .25, predelay: 0 };
  const clamp = (value, lo, hi) => Math.min(hi, Math.max(lo, Number.isFinite(Number(value)) ? Number(value) : lo));
  const pct = value => Math.round(value * 100) + '%';
  const bpmOf = project => Number(project?.timing?.tempos?.[0]?.bpm ?? project?.bpm) || 120;
  const division = label => DIVISIONS.find(([name]) => name === label) || null;

  function delayState(project) { return { ...DELAY_DEFAULTS, ...(project?.delay || {}) }; }
  function reverbState(project) { return { ...REVERB_DEFAULTS, ...(project?.reverb || {}) }; }
  /* Effective delay time in seconds for the project tempo. */
  function delaySeconds(project) {
    const delay = delayState(project), synced = division(delay.sync);
    return synced ? synced[1] * 60 / bpmOf(project) : clamp(delay.time, .001, 3.99);
  }
  function setDivision(project, label) {
    const synced = division(label); if (!synced) throw new Error('Unknown delay division.');
    project.delay = { ...delayState(project), sync: synced[0], beats: synced[1] };
    project.delay.time = clamp(synced[1] * 60 / bpmOf(project), .001, 3.99);
    return project.delay;
  }
  function setFreeTime(project, seconds) {
    project.delay = { ...delayState(project), sync: null, beats: 0, time: clamp(seconds, .001, 3.99) };
    return project.delay;
  }
  /* Nearest division to a time, used when switching free -> sync. */
  function nearestDivision(project, seconds = delaySeconds(project)) {
    const beats = seconds * bpmOf(project) / 60;
    return DIVISIONS.reduce((best, item) => Math.abs(Math.log(item[1] / beats)) < Math.abs(Math.log(best[1] / beats)) ? item : best)[0];
  }
  const toneHz = tone => tone >= 1
      ? 'Open'
      : Math.round(200 * Math.pow(100, tone)) >= 1000
      ? (200 * Math.pow(100, tone) / 1000).toFixed(1) + ' kHz'
      : Math.round(200 * Math.pow(100, tone)) + ' Hz';

  /* Parameter descriptors: get/set operate on a project; step is the arrow increment. */
  const RETURNS = {
    delay: [
      { key: 'sync', label: 'Timing', kind: 'choice', get: p => delayState(p).sync ? 'sync' : 'free', text: p => delayState(p).sync
          ? 'Tempo sync'
          : 'Free (ms)', cycle: (p, delta) => { if (delayState(p).sync) setFreeTime(p, delaySeconds(p)); else setDivision(p, nearestDivision(p)); void delta; } },
      { key: 'time', label: 'Time', kind: 'time', get: p => delaySeconds(p), text: p => { const d = delayState(p); return (d.sync ? d.sync + ' · ' : '')
          + Math.round(delaySeconds(p) * 1000)
          + ' ms'; },
        step: (p, delta, size) => {
          const d = delayState(p);
          if (d.sync) {
            const index = DIVISIONS.findIndex(([name]) => name === d.sync);
            setDivision(p, DIVISIONS[clamp(index + Math.sign(delta) * (size > 1 ? 3 : 1), 0, DIVISIONS.length - 1)][0]);
          } else {
            setFreeTime(p, Math.round(delaySeconds(p) * 1000 + delta * (size > 1 ? 50 : size < 1 ? 1 : 10)) / 1000);
          }
        },
        exact: (p, text) => {
          const label = String(text).trim();
          if (division(label)) return setDivision(p, label);
          const ms = Number(label.replace(/ms$/i, ''));
          if (!Number.isFinite(ms) || ms <= 0) throw new Error('Enter milliseconds (for example 375) or a division such as 1/8.');
          return setFreeTime(p, ms / 1000);
        },
        reset: p => setDivision(p, '1/8.') },
      { key: 'feedback', label: 'Feedback', min: 0, max: .95, step: .01, get: p => delayState(p).feedback, text: p => pct(delayState(p).feedback), set: (p, v) => { p.delay = { ...delayState(p), feedback: v }; } },
      { key: 'tone', label: 'Tone', min: 0, max: 1, step: .01, get: p => delayState(p).tone, text: p => toneHz(delayState(p).tone), set: (p, v) => { p.delay = { ...delayState(p), tone: v }; } },
      { key: 'mix', label: 'Return level', min: 0, max: 1, step: .01, get: p => delayState(p).mix, text: p => pct(delayState(p).mix), set: (p, v) => { p.delay = { ...delayState(p), mix: v }; } }
    ],
    reverb: [
      { key: 'size', label: 'Size', min: 0, max: 1, step: .01, get: p => reverbState(p).size, text: p => pct(reverbState(p).size), set: (p, v) => { p.reverb = { ...reverbState(p), size: v }; } },
      { key: 'damping', label: 'Damping', min: 0, max: .99, step: .01, get: p => reverbState(p).damping, text: p => pct(reverbState(p).damping), set: (p, v) => { p.reverb = { ...reverbState(p), damping: v }; } },
      { key: 'predelay', label: 'Pre-delay', min: 0, max: .25, step: .001, unit: 'ms', get: p => reverbState(p).predelay, text: p => Math.round(reverbState(p).predelay * 1000)
          + ' ms', set: (p, v) => { p.reverb = { ...reverbState(p), predelay: v }; } },
      { key: 'mix', label: 'Return level', min: 0, max: 1, step: .01, get: p => reverbState(p).mix, text: p => pct(reverbState(p).mix), set: (p, v) => { p.reverb = { ...reverbState(p), mix: v }; } }
    ]
  };
  const DEFAULT_OF = { delay: DELAY_DEFAULTS, reverb: REVERB_DEFAULTS };
  function sendSpec(track, bus) {
    const property = bus === 'delay' ? 'delaySend' : 'reverbSend';
    return { key: property, label: bus === 'delay'
        ? 'Delay send'
        : 'Reverb send', min: 0, max: 1, step: .01, get: p => clamp(p.tracks[track]?.[property] ?? 0, 0,
            1), text: p => pct(clamp(p.tracks[track]?.[property] ?? 0, 0,
                1)), set: (p, v) => { if (p.tracks[track]) p.tracks[track][property] = v; }, defaultValue: 0 };
  }
  /* Arrow adjustment: size 10 = coarse (Shift), .1 = fine (Option). */
  function adjust(project, spec, delta, size = 1) {
    if (spec.kind === 'choice') return spec.cycle(project, delta);
    if (spec.step instanceof Function) return spec.step(project, delta, size);
    const value = spec.get(project) + delta * spec.step * size;
    spec.set(project, Math.round(clamp(value, spec.min, spec.max) * 1000) / 1000);
  }
  function setExact(project, spec, text) {
    if (spec.exact) return spec.exact(project, text);
    const raw = String(text).trim().replace(/%$/, '').replace(/ms$/i, '');
    let value = Number(raw);
    if (!Number.isFinite(value)) throw new Error(`Enter a number for ${spec.label}.`);
    if (spec.unit === 'ms') value /= 1000; else if (spec.max <= 1) value /= 100;
    spec.set(project, clamp(value, spec.min, spec.max));
  }
  function reset(project, spec, group) {
    if (spec.reset) return spec.reset(project);
    if (spec.kind === 'choice') return;
    spec.set(project, spec.defaultValue ?? DEFAULT_OF[group]?.[spec.key] ?? spec.min);
  }

  const model = { DIVISIONS, RETURNS, delaySeconds, setDivision, setFreeTime, nearestDivision, sendSpec, adjust, setExact, reset, toneHz };
  if (typeof module !== 'undefined' && module.exports) module.exports = model;
  if (typeof window === 'undefined') return;

  // ---- Dialog -------------------------------------------------------------------------
  const app = () => window.ST, state = () => app().state, project = () => state().project;
  const esc = value => app().escapeHTML(String(value ?? ''));
  const local = { burst: null, focus: null, open: false };
  const BURST_MS = 900;

  function specFor(node) {
    const group = node.dataset.sendGroup, index = Number(node.dataset.sendIndex);
    if (group === 'delay' || group === 'reverb') return { spec: RETURNS[group][index], group };
    return { spec: sendSpec(index, group === 'delay-send' ? 'delay' : 'reverb'), group };
  }
  /* One undo step per burst of arrow edits on the same control. */
  function apply(node, label, fn) {
    const id = node.id, now = Date.now(), burst = local.burst;
    if (burst && burst.id === id && burst.project === project() && now - burst.at < BURST_MS) {
      fn(project()); burst.at = now; app().markDirty(); app().syncEngine();
    } else {
      let ok = false; app().mutate(label, p => { fn(p); ok = true; }, { render: false });
      if (!ok) return false;
      local.burst = { id, at: now, project: project() };
    }
    paint();
    return true;
  }
  function row(spec, group, index, p, first) {
    const value = spec.get(p),
      range = spec.kind === 'choice' || spec.kind === 'time'
          ? ''
          : `aria-valuemin="${spec.min}" aria-valuemax="${spec.max}" aria-valuenow="${value}"`;
    const fill = spec.kind === 'choice'
        ? value === 'sync' ? 100 : 0
        : spec.kind === 'time'
        ? Math.min(100, delaySeconds(p) / 2 * 100)
        : (value - spec.min) / (spec.max - spec.min) * 100;
    return `<div class="send-row" role="slider" tabindex="${first ? 0 : -1}" id="send-${group}-${index}" data-send-group="${group}"`
        + ` data-send-index="${index}" aria-label="${esc(spec.label)}" ${range} aria-valuetext="${esc(spec.text(p))}">
      <span class="send-label">${esc(spec.label)}</span><span class="send-bar"><i style="width:${Math.max(0, Math.min(100, fill)).toFixed(1)}%">`
          + `</i></span><span class="send-value">${esc(spec.text(p))}</span></div>`;
  }
  function render() {
    const p = project();
    const tracks = p.tracks.map((track, index) => `<div class="send-track-name" style="--track-color:${esc(track.color || '#998561')}">${esc(track.name || 'Track ' + (index + 1))}</div>`).join('');
    const delaySends = p.tracks.map((_, index) => row({
      ...sendSpec(index, 'delay'),
      label: `${p.tracks[index].name || 'Track ' + (index + 1)} → Delay`
    }, 'delay-send', index, p, index === 0)).join('');
    const reverbSends = p.tracks.map((_, index) => row({
      ...sendSpec(index, 'reverb'),
      label: `${p.tracks[index].name || 'Track ' + (index + 1)} → Reverb`
    }, 'reverb-send', index, p, index === 0)).join('');
    return `<div class="send-fx" id="send-fx">
      <p class="send-help">↑↓ choose · ←→ adjust (⇧ coarse, ⌥ fine) · Return types a value · 0 resets · Tab next column · Esc closes</p>
      <div class="send-returns">
        <section class="send-return" aria-label="Delay return"><h3>Delay <span class="muted">${esc(delayState(p).sync ? 'synced to ' + bpmOf(p) + ' BPM' : 'free time')}</span>`
            + `</h3><div role="group" aria-label="Delay return parameters" class="send-group" data-send-list="delay">${RETURNS.delay.map((spec, index) => row(spec, 'delay', index, p, index === 0)).join('')}</div></section>
        <section class="send-return" aria-label="Reverb return"><h3>Reverb</h3><div role="group" aria-label="Reverb return parameters"`
            + ` class="send-group" data-send-list="reverb">${RETURNS.reverb.map((spec, index) => row(spec, 'reverb', index, p, index === 0)).join('')}</div></section>
      </div>
      <section class="send-matrix" aria-label="Track sends"><h3>Track sends</h3>
        <div class="send-matrix-grid"><div class="send-matrix-head"></div><div class="send-matrix-head">Delay</div>`
            + `<div class="send-matrix-head">Reverb</div>
          <div class="send-track-names">${tracks}</div>
          <div role="group" aria-label="Delay sends" class="send-group compact" data-send-list="delay-send">${delaySends}</div>
          <div role="group" aria-label="Reverb sends" class="send-group compact" data-send-list="reverb-send">${reverbSends}</div>
        </div>
      </section>
      <p class="send-note">Built-in instruments use the engine returns. Plug-in instrument tracks and standard-routing projects use the Apple `
          + `delay/reverb returns: there, time, sync, feedback, tone and levels apply; reverb size picks a room preset and pre-delay/damping are not applied.</p>
    </div>`;
  }
  function paint() {
    const root = document.getElementById('send-fx'); if (!root) return;
    const p = project();
    root.querySelectorAll('[data-send-group]').forEach(node => {
      const { spec } = specFor(node);
      const value = spec.get(p), text = spec.text(p);
      node.querySelector('.send-value').textContent = text;
      node.setAttribute('aria-valuetext', text);
      if (spec.kind !== 'choice' && spec.kind !== 'time') node.setAttribute('aria-valuenow', String(value));
      const fill = spec.kind === 'choice'
          ? value === 'sync' ? 100 : 0
          : spec.kind === 'time'
          ? Math.min(100, delaySeconds(p) / 2 * 100)
          : (value - spec.min) / (spec.max - spec.min) * 100;
      node.querySelector('.send-bar i').style.width = Math.max(0, Math.min(100, fill)).toFixed(1) + '%';
    });
    const heading = root.querySelector('.send-return h3 .muted');
    if (heading) heading.textContent = delayState(p).sync ? 'synced to ' + bpmOf(p) + ' BPM' : 'free time';
  }
  function exactPrompt(node) {
    const { spec } = specFor(node);
    const current = spec.kind === 'time'
        ? delayState(project()).sync || String(Math.round(delaySeconds(project()) * 1000))
        : spec.kind === 'choice'
        ? null
        : spec.unit === 'ms'
        ? String(Math.round(spec.get(project()) * 1000))
        : spec.max <= 1
        ? String(Math.round(spec.get(project()) * 100))
        : String(spec.get(project()));
    if (current == null) { apply(node, 'delay timing', p => adjust(p, spec, 1)); return; }
    const value = node.querySelector('.send-value'), input = document.createElement('input');
    input.className = 'send-input'; input.value = current; input.setAttribute('aria-label', `${spec.label} value`);
    value.replaceWith(input); input.focus(); input.select();
    let finished = false;
    const finish = commit => {
      if (finished) return; finished = true;
      if (commit) {
        try {
          const trial = clone({
            tracks: project().tracks,
            delay: project().delay,
            reverb: project().reverb,
            bpm: project().bpm,
            timing: project().timing
          });
          setExact(trial, spec, input.value);
          local.burst = null;
          apply(node, 'edit ' + spec.label.toLowerCase(), p => setExact(p, spec, input.value));
        } catch (error) {
          app().toast(error.message, 'error');
        }
      }
      input.replaceWith(value); paint(); node.focus();
    };
    input.onkeydown = event => {
      if (event.key === 'Enter') {
        event.preventDefault();
        event.stopPropagation();
        finish(true);
      } else if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        finish(false);
      }
    };
    input.onblur = () => finish(true);
  }
  function keydown(event) {
    const node = event.target.closest?.('[data-send-group]'); if (!node || event.target.tagName === 'INPUT') return;
    const list = [...node.parentElement.querySelectorAll('[data-send-group]')], index = list.indexOf(node), { spec, group } = specFor(node);
    const move = next => {
      const target = list[Math.max(0, Math.min(list.length - 1, next))];
      list.forEach(item => {
        item.tabIndex = item === target ? 0 : -1;
      });
      target.focus();
      target.scrollIntoView?.({ block: 'nearest' });
    };
    const label = group.endsWith('-send') ? spec.label.toLowerCase() : `${group} ${spec.label.toLowerCase()}`;
    if (event.metaKey || event.ctrlKey) return;
    switch (event.key) {
      case 'ArrowUp': case 'ArrowDown': event.preventDefault(); move(index + (event.key === 'ArrowUp' ? -1 : 1)); return;
      case 'Home': case 'End': event.preventDefault(); move(event.key === 'Home' ? 0 : list.length - 1); return;
      case 'ArrowLeft': case 'ArrowRight': case '-': case '=': case '+': case '[': case ']': {
        event.preventDefault(); const delta = ['ArrowLeft', '-', '['].includes(event.key) ? -1 : 1;
        apply(node, 'edit ' + label, p => adjust(p, spec, delta, event.shiftKey ? 10 : event.altKey ? .1 : 1)); return;
      }
      case 'PageUp': case 'PageDown': event.preventDefault(); apply(node, 'edit ' + label,
          p => adjust(p, spec, event.key === 'PageUp' ? 1 : -1, 10)); return;
      case 'Enter': event.preventDefault(); exactPrompt(node); return;
      case '0': case 'Backspace': case 'Delete': event.preventDefault(); local.burst = null; apply(node, 'reset ' + label,
          p => reset(p, spec, group)); return;
      default: if (/^[1-9]$/.test(event.key)
          && spec.kind !== 'choice') { event.preventDefault(); exactPrompt(node); const input = node.querySelector('.send-input'); if (input) input.value = event.key; } return;
    }
  }
  function bind() {
    const root = document.getElementById('send-fx'); if (!root) return;
    root.addEventListener('keydown', keydown);
    root.querySelectorAll('[data-send-group]').forEach(node => {
      node.addEventListener('click', event => {
        const bar = node.querySelector('.send-bar'), { spec } = specFor(node);
        if (event.target.closest('.send-bar')
            && spec.kind !== 'choice'
            && spec.kind !== 'time') { const rect = bar.getBoundingClientRect(), ratio = clamp((event.clientX - rect.left) / rect.width, 0,
                1); local.burst = null; apply(node,
            'edit ' + spec.label.toLowerCase(), p => spec.set(p, Math.round((spec.min + ratio * (spec.max - spec.min)) * 1000) / 1000)); }
        else if (spec.kind === 'choice') apply(node, 'delay timing', p => adjust(p, spec, 1));
        node.parentElement.querySelectorAll('[data-send-group]').forEach(item => { item.tabIndex = item === node ? 0 : -1; }); node.focus();
      });
      node.addEventListener('dblclick', () => exactPrompt(node));
    });
  }
  /* Open the dialog, optionally focused on a track's send ('delay'|'reverb') or a return. */
  function show(options = {}) {
    if (!project()) return false;
    local.burst = null;
    app().openModal('Send effects · delay & reverb', render(), '<button id="send-fx-close" class="primary">Done</button>', {
      kind: 'send-effects',
      className: 'send-fx-modal'
    });
    if (state().modal !== 'send-effects') return false;
    document.getElementById('send-fx-close').onclick = () => app().closeModal();
    bind();
    const track = Number.isInteger(options.track) ? options.track : null;
    const target = track != null && options.bus
        ? document.getElementById(`send-${options.bus}-send-${track}`)
        : options.focus
        ? document.getElementById(options.focus)
        : document.getElementById('send-delay-0');
    if (target) {
      target.parentElement.querySelectorAll('[data-send-group]').forEach(item => {
        item.tabIndex = item === target ? 0 : -1;
      });
      setTimeout(() => target.focus(), 0);
    }
    return true;
  }
  function setSend(track, bus, value) {
    const property = bus === 'delay' ? 'delaySend' : 'reverbSend';
    app().mutate(`${bus} send`, p => { if (p.tracks[track]) p.tracks[track][property] = clamp(value, 0, 1); });
  }
  function trackFrom(target) {
    const match = /^mix-(?:track|vol|pan|delay|reverb|mute|solo|slots|output|sends)-(\d+)$/.exec(target?.closest?.('[id^="mix-"]')?.id || '');
    if (match) return Number(match[1]);
    const select = target?.closest?.('[data-select-track]'); if (select) return Number(select.dataset.selectTrack);
    return null;
  }
  /* Right-click items for mixer channels (and their delay/reverb controls). */
  function contextItems({ target } = {}) {
    if (state().view !== 'mixer' || !target?.closest?.('#mixer-workspace')) return null;
    const track = trackFrom(target), p = project();
    const items = [{ header: true, title: 'Send effects' },
        { title: 'Delay & reverb returns…', key: '', action: () => show({ focus: 'send-delay-0' }) }];
    if (track != null && p.tracks[track]) {
      const levels = [0, .1, .25, .5, .75, 1];
      for (const bus of ['delay', 'reverb']) {
        const current = p.tracks[track][bus === 'delay' ? 'delaySend' : 'reverbSend'] || 0;
        items.push({
          title: `${bus === 'delay' ? 'Delay' : 'Reverb'} send · ${pct(current)}`,
          items: [...levels.map(level => ({
            title: pct(level),
            checked: Math.abs(current - level) < .005,
            action: () => setSend(track, bus, level)
          })), { separator: true }, { title: 'Edit in send effects…', action: () => show({ track, bus }) }]
        });
      }
    }
    return items;
  }
  let registered = false;
  function register() {
    if (registered || !window.STContextMenu?.register) return;
    registered = true;
    window.STContextMenu.register(context => contextItems(context), 55);
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => setTimeout(register, 0)); else setTimeout(register, 0);

  window.STSends = { ...model, show, contextItems, setSend, get isOpen() { return state().modal === 'send-effects'; } };
})();
