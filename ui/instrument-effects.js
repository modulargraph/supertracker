/* Instrument insert chains. An instrument owns effects (built-ins and AU/VST3/CLAP) as project plug-in
   descriptors with owner 'instrument:<index>' (kind 'effect', slots 0–15), edited on the Instrument page with
   the mixer's insert-chain component (insert-chain.js) through window.STInserts. The audio host runs one
   instance of the chain at the start of every track that plays the instrument, before the track's own inserts
   (st-audio's mixer graph). A track's channel carries every voice the track plays, so the
   chain of the track's assigned instrument applies to the whole track, including notes whose steps name
   another instrument. Automation lanes and step-FX locks address instrument effects like any insert; lanes
   use owner 'instrument:<index>' and reach every track that plays the instrument.

   Copies of an instrument (duplicates, kits, pads) take its chain with fresh instance IDs (copy). */
(() => {
  'use strict';
  const S = () => window.ST;
  const esc = value => S().escapeHTML(String(value ?? ''));
  const OWNER = /^instrument:(\d+)$/;
  const owner = index => `instrument:${Number(index)}`;
  const indexOf = value => { const match = OWNER.exec(String(value ?? '')); return match ? Number(match[1]) : -1; };
  const isOwner = value => OWNER.test(String(value ?? ''));
  const uid = () => globalThis.crypto?.randomUUID?.() || `ifx-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  const clone = value => JSON.parse(JSON.stringify(value));

  /** The instrument's effects in chain order (project descriptors). */
  const chain = (p, index) => p.audioPlugins.filter(item => item.owner === owner(index) && (item.kind || 'effect') === 'effect')
      .sort((a, b) => (a.slot ?? 0) - (b.slot ?? 0));
  /** Indices of the tracks whose assigned instrument is `index` (each runs an instance of the chain). */
  const tracksPlaying = (p, index) => p.tracks.flatMap((track, i) => Number(track.instrument) === Number(index) ? [i] : []);

  /** Copies instrument `from`'s chain (of `source`, default `p`) to instrument `to` in `p`: fresh instance IDs,
      the same states, bypass and curated page settings. Returns the copies. */
  function copy(p, from, to, source = p) {
    const copies = chain(source, from).map(item => {
      const next = { ...clone(item), owner: owner(to), instanceID: uid() };
      delete next.parameters;
      if (source.pluginPages[item.instanceID]) p.pluginPages[next.instanceID] = clone(source.pluginPages[item.instanceID]);
      return next;
    });
    if (!copies.length) return copies;
    p.audioPlugins = [...p.audioPlugins.filter(item => item.owner !== owner(to)), ...copies];
    return copies;
  }

  /** Before instrument `removed` is spliced out: drop its chain and lanes, and move later instruments' chains
      and lanes to their new index. */
  function reindexAfterRemoval(p, removed) {
    const shift = value => { const i = indexOf(value); return i > removed ? owner(i - 1) : value; };
    const gone = owner(removed);
    const ids = new Set(p.audioPlugins.filter(item => item.owner === gone).map(item => item.instanceID).filter(Boolean));
    p.audioPlugins = p.audioPlugins.filter(item => item.owner !== gone).map(item => isOwner(item.owner) ? { ...item, owner: shift(item.owner) } : item);
    for (const id of ids) delete p.pluginPages[id];
    for (const pattern of p.patterns) {
      pattern.automation = pattern.automation.filter(lane => lane.owner !== gone);
      for (const lane of pattern.automation) if (isOwner(lane.owner)) lane.owner = shift(lane.owner);
    }
  }

  /** Moves the chains of a group project's instruments (a kit being inserted) to their new indices:
      map is old index -> new index. Descriptors of unmapped instruments are dropped. */
  function remap(plugins, map) {
    return (plugins || []).flatMap(item => {
      if (!isOwner(item.owner)) return [item];
      const next = map.get(indexOf(item.owner));
      return next === undefined ? [] : [{ ...item, owner: owner(next) }];
    });
  }

  // ---- Instrument page ------------------------------------------------------------------------------------------
  const PREFIX = 'instfx';
  function sourceTile(inst) {
    const kind = inst.type === 'plugin' ? 'Plug-in' : inst.type === 'midi' ? 'External MIDI' : 'Sampler';
    return { id: `${PREFIX}-source`, label: 'Source', name: inst.name || 'Instrument', detail: kind, selected: false,
      attributes: 'title="The instrument itself; its effects follow in signal order" aria-label="Source: the instrument"' };
  }
  function render(inst, index) {
    const p = S().state.project;
    if (!inst || inst.type === 'midi' || !window.STInserts) return '';
    const playing = tracksPlaying(p, index), names = playing.slice(0, 3).map(i => p.tracks[i]?.name || `Track ${i + 1}`);
    const where = playing.length
        ? `Runs on ${playing.length === 1 ? 'track' : `${playing.length} tracks`} ${esc(names.join(', '))}${playing.length > 3 ? '…' : ''}, before the track's own inserts.`
        : 'No track plays this instrument yet: assign it to a track to hear and edit its effects.';
    return `<section class="panel wide instrument-effects" id="instrument-effects" aria-label="Instrument effects">`
      + `<div class="panel-head"><h2>Effects</h2><span class="muted instrument-effects-where">${where}</span></div>`
      + `<div class="panel-body">${window.STInsertChain.render(owner(index), { prefix: PREFIX, title: inst.name || 'Instrument', source: [sourceTile(inst)],
          hint: 'Saved with the instrument' })}`
      + `<p class="inst-caption instrument-effects-note">Every track playing this instrument gets its own instance of the chain. `
      + `Notes a track plays with another instrument number also pass through it.</p></div></section>`;
  }
  function bind(index) {
    const root = document.getElementById('instrument-effects');
    if (!root) return;
    window.STInsertChain.bind(root, owner(index), { prefix: PREFIX, title: S().state.project?.instruments[index]?.name || 'Instrument',
      source: [{ id: `${PREFIX}-source` }], hint: 'Saved with the instrument' });
    // Ask the host for the parameter lists once, so the knob pages and automation targets are ready.
    if (chain(S().state.project, index).length && tracksPlaying(S().state.project, index).length) window.STStudio?.loadAutomationParameters?.(owner(index))?.catch?.(() => {});
  }

  window.STInstrumentEffects = { owner, indexOf, isOwner, chain, tracksPlaying, copy, reindexAfterRemoval, remap, render, bind };
})();
