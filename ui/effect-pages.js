/* Shared descriptions of the built-in (stfx:) effects: Maschine-style control pages.
   Each effect has named pages of up to eight parameters in a musical order, grouped in sections, with short
   display names. Ranges, units, value curves (linear / logarithmic / stepped), defaults and choice labels come
   from the host's catalogue (bridge command `builtin.effects`, tracker-engine), so this file
   only holds what a person curates: which knob goes where and what it is called.
   Used by the plug-in pages console (plugin-pages.js), the insert chain (insert-chain.js) and the MK3
   PLUG-IN pages. Plug-ins that are not built in keep the generic eight-at-a-time pages. */
(() => {
  'use strict';
  const root = typeof window !== 'undefined' ? window : globalThis;
  const SC = ['Sidechain', ['Detector', 'scgain=Gain', 'scfilter=Filter', 'sccenter=Freq', 'scwidth=Width']];
  const LFO = ['Modulation', ['Source', 'source=Source'], ['LFO', 'speed=Rate', 'beats=Sync rate'], ['Envelope', 'envshape=Env shape']];
  /* Page = [name, ...sections]; section = [section name, ...parameter ids]; 'id=Label' renames a knob. */
  const LAYOUTS = {
    // Dynamics
    compressor: [['Main', ['Compressor', 'threshold', 'ratio', 'attack', 'release'], ['Output', 'gain=Makeup', 'mix'], ['Character', 'knee', 'mode']], SC],
    gate: [['Main', ['Gate', 'threshold', 'attack', 'hold', 'release'], ['Output', 'mix']], SC],
    transient: [['Main', ['Shape', 'attack', 'sustain'], ['Level', 'gain=Input', 'limit']]],
    limiter: [['Main', ['Limiter', 'threshold', 'ceiling', 'release'], ['Character', 'mode']], SC],
    maximizer: [['Main', ['Maximizer', 'amount', 'curve', 'turbo']], SC],
    // Filtering
    eq: [['Bands', ['Low', 'lowgain=Gain', 'lowfreq=Freq'], ['Low Mid', 'lowmidgain=Gain', 'lowmidfreq=Freq'], ['High Mid', 'highmidgain=Gain', 'highmidfreq=Freq'], ['High', 'highgain=Gain', 'highfreq=Freq']],
      ['Width', ['Low Mid', 'lowmidwidth=Width'], ['High Mid', 'highmidwidth=Width'], ['Output', 'gain=Output']]],
    filter: [['Main', ['Filter', 'cutoff', 'resonance=Reso', 'mode'], ['Modulation', 'amount=Mod amount', 'source=Mod source'], ['Output', 'mix']],
      ['LFO', ['LFO', 'speed=Rate', 'beats=Sync rate', 'shape', 'phase'], ['Depth', 'amount=Mod amount']],
      ['Envelope', ['Envelope', 'decay=Decay', 'smooth=Smooth', 'envshape=Shape'], ['Depth', 'amount=Mod amount']], SC],
    cabinet: [['Main', ['Cabinet', 'cabinet=Type', 'microphone=Mic', 'distance'], ['Output', 'mix']]],
    // Modulation
    chorus: [['Main', ['Chorus', 'rate', 'amount', 'stereo'], ['Output', 'mix']]],
    flanger: [['Main', ['Flanger', 'frequency=Freq', 'feedback', 'amount', 'invert'], ['Output', 'stereo', 'mix']], LFO],
    fm: [['Main', ['FM', 'rate', 'amount', 'split', 'contour'], ['Output', 'mix']]],
    freqshifter: [['Main', ['Shift', 'coarse', 'fine', 'feedback', 'invert'], ['Output', 'stereo', 'mix']]],
    phaser: [['Main', ['Phaser', 'frequency=Freq', 'feedback', 'amount', 'eightpole=8 poles'], ['Output', 'stereo', 'mix']], LFO],
    // Space
    ice: [['Main', ['Ice', 'ice', 'color', 'size'], ['Output', 'mix']]],
    metaverb: [['Main', ['Metaverb', 'size'], ['EQ', 'low', 'high'], ['Output', 'pan=Dry pan', 'mix']]],
    reflex: [['Main', ['Reflex', 'color', 'smooth', 'size'], ['Output', 'mix']]],
    reverblegacy: [['Main', ['Room', 'room=Type', 'size', 'stereo', 'freeze'], ['EQ', 'low', 'high'], ['Output', 'pan=Dry pan', 'mix']]],
    reverb: [['Main', ['Reverb', 'mode', 'time', 'size', 'predelay=Pre delay'], ['Character', 'damping', 'diffusion=Diffusion', 'modulation=Mod'], ['Output', 'mix']],
      ['Tone', ['EQ', 'highcut=High cut', 'lowshelf=Low shelf']]],
    plate: [['Main', ['Plate', 'decay', 'predelay=Pre delay', 'highdamp=Damping'], ['EQ', 'lowshelf=Low shelf', 'highcut=High cut'], ['Output', 'mix']]],
    // Delay
    beatdelay: [['Main', ['Time', 'time', 'unit', 'offset=Groove'], ['Echoes', 'feedback', 'color', 'crossover=X-over'], ['Output', 'stereo', 'mix']], ['Split', ['Stereo', 'split']]],
    graindelay: [['Main', ['Grains', 'pitch', 'size', 'density', 'jitter'], ['Space', 'space', 'reverse'], ['Output', 'stereo', 'mix']], ['Modulation', ['Grains', 'mod']]],
    grainstretch: [['Main', ['Capture', 'on=Capture', 'loop'], ['Stretch', 'stretch', 'pitch', 'link', 'size'], ['Output', 'mix']]],
    resochord: [['Main', ['Chord', 'mode', 'style', 'chord=Root', 'tune'], ['Resonator', 'brightness=Bright', 'feedback', 'decay'], ['Output', 'mix']], ['Stereo', ['Output', 'spread']]],
    // Distortion
    distortion: [['Main', ['Distortion', 'mode', 'drive', 'tone'], ['Output', 'output', 'mix']],
      ['Fuzz', ['Fuzz', 'color=Color', 'feedback=Feedback', 'tone_mod=Tone mod', 'mod_rate=Mod rate'], ['Gate', 'gate', 'gate_threshold=Threshold', 'gate_release=Release']],
      ['Analog', ['Analog', 'bass=Bass', 'focus=Focus', 'definition=Definition', 'saturation=Saturation']]],
    lofi: [['Main', ['Lofi', 'sample_rate=Rate', 'bits', 'smooth', 'jitter=Jitter'], ['Level', 'input', 'output'], ['Output', 'stereo=Width', 'mix']]],
    saturator: [['Main', ['Saturator', 'mode', 'drive', 'contour', 'input'], ['Output', 'output', 'mix']],
      ['Tube', ['Tube', 'charge=Charge', 'overload=Overload'], ['EQ', 'bass', 'treble', 'eq_bypass=EQ off']]],
    // Perform
    performfilter: [['Main', ['Perform', 'engage', 'position=Freq', 'type', 'resonance=Reso'], ['Output', 'saturation=Drive', 'output', 'mix']],
      ['Range', ['Range', 'min_frequency=Min freq', 'max_frequency=Max freq', 'response']]],
    performflanger: [['Main', ['Perform', 'engage', 'frequency=Freq', 'decay', 'saturation=Drive'], ['Character', 'polarity', 'tone', 'stereo'], ['Output', 'mix']],
      ['Motion', ['Motion', 'rate=Rate', 'depth=Depth']]],
    burstecho: [['Main', ['Perform', 'engage', 'position=Position', 'assignment=Controls'], ['Echo', 'time', 'feedback', 'filter=FB filter'], ['Output', 'wet', 'mix_mode=Mix mode']],
      ['Timing', ['Timing', 'time_mode=Time mode', 'free_time=Free time', 'feed=Burst'], ['Output', 'saturation=Drive', 'stereo=Ping-pong', 'output']]],
    resoecho: [['Main', ['Perform', 'engage', 'frequency=Freq', 'time', 'feedback'], ['Resonator', 'resonance=Reso', 'pump'], ['Output', 'wet', 'mix_mode=Mix mode']],
      ['Timing', ['Timing', 'sync', 'free_time=Free time'], ['Output', 'saturation=Drive', 'spread=Spread']]],
    ring: [['Main', ['Perform', 'engage', 'frequency=Freq', 'spread=Spread', 'glide'], ['Reverb', 'reverb_time=Rev time', 'reverb_mix=Rev mix'], ['Output', 'mix', 'mix_mode=Mix mode']],
      ['Output', ['Output', 'output']]],
    stutter: [['Main', ['Perform', 'engage', 'position=Position', 'assignment=Controls', 'length'], ['Loop', 'pitch', 'gate', 'direction'], ['Output', 'mix']],
      ['Timing', ['Timing', 'quantize=Quantize', 'free_length=Free length', 'fade']]],
    tremolo: [['Main', ['Perform', 'engage', 'position=Position', 'blend=Trem/Vib', 'vibrato_mode=Vib mode'], ['Rate', 'rate_min=Rate min', 'rate_max=Rate max'], ['Depth', 'depth_min=Depth min', 'depth_max=Depth max']],
      ['Timing', ['Timing', 'sync', 'division=Period', 'response=Att/Rel'], ['Output', 'stereo=Stereo', 'output']]],
    scratcher: [['Main', ['Perform', 'engage', 'position=Position', 'brake', 'capture=Capture'], ['Swirl', 'delay=Swirl time', 'feedback', 'frequency_shift=Shift', 'wet=Swirl level']],
      ['Output', ['Output', 'mix', 'output']]]
  };
  /* Add-effect list order and names, Maschine-style. Catalogue categories map onto these. */
  const CATEGORIES = [['Dynamics', ['Dynamics']], ['Filter & EQ', ['Filtering']], ['Modulation', ['Modulation']],
    ['Reverb & Space', ['Spatial', 'Reverb']], ['Delay', ['Delay']], ['Distortion', ['Distortion']], ['Perform', ['Perform']]];

  const metadata = new Map();
  let loading = null;
  const listeners = new Set();
  const dspID = component => String(component || '').startsWith('stfx:') ? String(component).slice(5) : null;
  const isBuiltin = plugin => !!dspID(typeof plugin === 'string' ? plugin : plugin?.component || plugin?.id);

  /** Accepts the native `builtin.effects` list (or a test fixture of the same shape). */
  function setMetadata(list) {
    if (!Array.isArray(list)) return false;
    metadata.clear();
    for (const effect of list) if (effect && dspID(effect.id)) metadata.set(dspID(effect.id), { ...effect, parameters: (effect.parameters || []).map((p, i) => ({ address: i, ...p })) });
    for (const fn of listeners) try { fn(); } catch {}
    return metadata.size > 0;
  }
  /** Reads the native catalogue once (the macOS app); safe to call repeatedly. */
  let attempted = 0;
  function load(request = root.ST?.request) {
    if (metadata.size) return Promise.resolve(true);
    if (loading) return loading;
    if (typeof request !== 'function' || Date.now() - attempted < 5000) return Promise.resolve(false);
    attempted = Date.now();
    loading = Promise.resolve().then(() => request('builtin.effects', {})).then(setMetadata).catch(() => false).finally(() => { loading = null; });
    return loading;
  }
  const onChange = fn => { listeners.add(fn); return () => listeners.delete(fn); };

  // ---- Values -------------------------------------------------------------------------------
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v));
  const stepCount = p => p.toggle ? 1 : p.integer ? Math.max(1, Math.round(p.max - p.min)) : 0;
  const logarithmic = p => !!p.logarithmic && p.min > 0 && p.max > p.min;
  function physical(p, normalized) {
    const n = clamp(Number.isFinite(Number(normalized)) ? Number(normalized) : 0, 0, 1);
    let v = logarithmic(p) ? p.min * Math.pow(p.max / p.min, n) : p.min + (p.max - p.min) * n;
    if (p.integer || p.toggle) v = Math.round(v);
    return clamp(v, p.min, p.max);
  }
  function normalized(p, value) {
    let v = clamp(Number.isFinite(Number(value)) ? Number(value) : p.defaultValue, p.min, p.max);
    if (p.integer || p.toggle) v = Math.round(v);
    if (p.max === p.min) return 0;
    return clamp(logarithmic(p) ? Math.log(v / p.min) / Math.log(p.max / p.min) : (v - p.min) / (p.max - p.min), 0, 1);
  }

  // ---- Formatting ---------------------------------------------------------------------------
  const trim = (v, digits) => { const s = Number(v).toFixed(digits); return s.includes('.') ? s.replace(/\.?0+$/, '') || '0' : s; };
  const signed = (v, digits, plus = true) => { const t = trim(Math.abs(v), digits); return `${t === '0' ? '' : v > 0 ? (plus ? '+' : '') : v < 0 ? '−' : ''}${t}`; };
  const precise = v => Math.abs(v) >= 100 ? trim(v, 0) : Math.abs(v) >= 10 ? trim(v, 1) : trim(v, 2);
  function hz(v) { return v >= 1000 ? `${v >= 10000 ? trim(v / 1000, 1) : trim(v / 1000, 2)} kHz` : v >= 100 ? `${Math.round(v)} Hz` : `${trim(v, v >= 10 ? 1 : 2)} Hz`; }
  function ms(v) { return v >= 1000 ? `${trim(v / 1000, 2)} s` : `${precise(v)} ms`; }
  /** Beats as a note value (1/8, 3/16, 1/4.) when it is one, else beats. */
  function beats(v) {
    const whole = v / 4;
    for (const d of [1, 2, 4, 8, 16, 32, 64, 128]) {
      const n = whole * d;
      if (Math.abs(n - Math.round(n)) < 1e-3 * Math.max(1, n) && Math.round(n) >= 1) { const k = Math.round(n); return k % d === 0 ? `${k / d} bar${k / d === 1 ? '' : 's'}` : (() => { const g = gcd(k, d); return `${k / g}/${d / g}`; })(); }
      const dotted = n / 1.5;
      if (Math.round(dotted) === 1 && Math.abs(dotted - 1) < 1e-3) return `1/${d}.`;
    }
    return `${trim(v, 2)} beats`;
  }
  const gcd = (a, b) => b ? gcd(b, a % b) : a;
  /** The display text of a physical value, in the parameter's own unit. */
  function formatValue(p, v) {
    if (!p) return '';
    if (p.labels?.length) return String(p.labels[clamp(Math.round(v - p.min), 0, p.labels.length - 1)] ?? trim(v, 0));
    if (p.toggle) return v >= .5 ? 'On' : 'Off';
    const bipolar = p.min < 0 && p.max > 0;
    switch (p.unit) {
      case 'Hz': return bipolar ? `${signed(v, Math.abs(v) >= 100 ? 0 : 1)} Hz` : hz(v);
      case 'dB': return `${signed(v, 1, bipolar)} dB`;
      case 'ms': return ms(v);
      case 's': return v < 1 ? `${precise(v * 1000)} ms` : `${trim(v, 2)} s`;
      case '%': { const pct = p.max <= 1.05 ? v * 100 : v; return `${bipolar ? signed(pct, 0) : trim(pct, 0)}%`; }
      case '% unit': return `${signed(v, 0)}%`;
      case ':1': return `${trim(v, 1)}:1`;
      case 'oct': return `${signed(v, 2, bipolar)} oct`;
      case 'deg': return `${Math.round(v)}°`;
      case 'st': return `${signed(v, Math.abs(v - Math.round(v)) < .005 ? 0 : 2)} st`;
      case 'beats': return beats(v);
      case 'bits': return `${trim(v, 1)} bit`;
      case '1/16 steps': return `${Math.round(v)}/16`;
      default: return p.integer ? String(Math.round(v)) : signed(v, 2, false);
    }
  }
  /* Values that only make sense together with another parameter: the knob shows the combination.
     Beat Delay's Time counts Units ("4 × 1/16"); Burst Echo's Time is played straight, dotted or as a triplet. */
  const LINKED = {
    beatdelay: { time: { with: 'unit', text: (time, unit) => `${time} × ${unit}` } },
    burstecho: { time: { with: 'time_mode', text: (time, mode) => mode === 'Dotted' ? `${time} dotted` : mode === 'Triplet' ? `${time} triplet` : time } }
  };
  /** Display text for a normalized value of an effect's parameter, or '' when the metadata is unknown.
      sibling(id, address) returns another parameter's normalized value, for values shown in combination (LINKED). */
  function display(component, idOrAddress, value, sibling = null) {
    const p = parameter(component, idOrAddress);
    if (!p) return '';
    const text = formatValue(p, physical(p, value)), link = LINKED[dspID(component)]?.[p.id];
    if (!link || typeof sibling !== 'function') return text;
    const other = parameter(component, link.with), v = other ? Number(sibling(other.id, String(other.address))) : NaN;
    return other && Number.isFinite(v) ? link.text(text, formatValue(other, physical(other, v))) : text;
  }
  /** Addresses of the parameters whose display combines another parameter's value (relabel them when it changes). */
  const linkedAddresses = component => Object.keys(LINKED[dspID(component)] || {}).map(key => parameter(component, key)).filter(Boolean).map(p => String(p.address));

  // ---- Descriptions -------------------------------------------------------------------------
  function parameter(component, idOrAddress) {
    const effect = metadata.get(dspID(component)); if (!effect) return null;
    return effect.parameters.find(p => p.id === idOrAddress) || (/^\d+$/.test(String(idOrAddress)) ? effect.parameters[Number(idOrAddress)] : null) || null;
  }
  function knobOf(effect, token, section) {
    const [id, label] = token.split('=');
    const p = effect?.parameters.find(item => item.id === id);
    return { id, name: label || p?.name || id, fullName: p?.name || label || id, section, address: p ? String(p.address) : null,
      ...(p ? { unit: p.unit, stepCount: stepCount(p), defaultValue: normalized(p, p.defaultValue), labels: p.labels || [] } : {}) };
  }
  /** Named pages of up to eight knobs (null = empty knob) for a built-in effect, or null. */
  function pages(component) {
    const id = dspID(component), layout = LAYOUTS[id]; if (!layout) return null;
    const effect = metadata.get(id);
    return layout.map(([name, ...sections]) => {
      const slots = [];
      for (const [section, ...tokens] of sections) tokens.forEach((token, i) => slots.push(knobOf(effect, token, i === 0 ? section : '')));
      return { name, kind: 'builtin', slots: Array.from({ length: 8 }, (_, i) => slots[i] || null) };
    });
  }
  /** One description per effect: id, name, category and pages. */
  function describe(component) {
    const id = dspID(component); if (!LAYOUTS[id]) return null;
    const effect = metadata.get(id);
    return { id: 'stfx:' + id, name: effect?.name || id, category: effect?.category || '', pages: pages(component), parameters: effect?.parameters || [] };
  }
  /** Built-in effects grouped for the add list, from a plug-in catalogue (state.plugins) or the metadata. */
  function categories(catalogue = null) {
    const list = (Array.isArray(catalogue) && catalogue.some(isBuiltin) ? catalogue.filter(isBuiltin) : [...metadata.values()].map(e => ({ id: e.id, name: e.name, category: e.category })))
      .filter(item => LAYOUTS[dspID(item.id)]);
    return CATEGORIES.map(([name, sources]) => ({ name, effects: list.filter(item => sources.includes(item.category)).map(item => ({ id: item.id, name: item.name, category: name })) }))
      .filter(group => group.effects.length);
  }
  const categoryOf = component => { const c = metadata.get(dspID(component))?.category; return CATEGORIES.find(([, sources]) => sources.includes(c))?.[0] || ''; };

  root.STEffectPages = { LAYOUTS, CATEGORIES, load, setMetadata, onChange, get ready() { return metadata.size > 0; }, isBuiltin, dspID,
    pages, describe, categories, categoryOf, parameter, physical, normalized, formatValue, display, linkedAddresses, stepCount };
  if (typeof module !== 'undefined') module.exports = root.STEffectPages;
})();
