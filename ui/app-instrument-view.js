/* Instrument view: sampler parameter groups, the envelope drawing, the piano and the
   instrument page markup. */
(function () {
  'use strict';

  const app = window.STApp;
  const {
    esc, noteText, state, currentInstrument, sidebar, chords, PLAY_MODE_LABELS, LOOPING_PLAY_MODES,
    tuneParts, parameter
  } = app;

  function renderSoundGroups(inst) {
    const isSample = inst.type === 'sample',
      mode = PLAY_MODE_LABELS[inst.mode] ? inst.mode : 'oneshot',
      tune = tuneParts(inst),
      tuneValue = Number((Number(inst.tune) || 0).toFixed(4));
    const group = (id, title, head, body, wide = false) => `<section class="panel inst-group${wide ? ' wide' : ''}" id="inst-group-${id}" aria-label="${esc(title)}">`
        + `<div class="panel-head"><h2>${esc(title)}</h2>${head || ''}</div><div class="panel-body">${body}</div></section>`;
    const hint = '<span class="inst-hint" aria-hidden="true">↑↓ fine · ⌥ finer · ⇧ coarse · ⌥A automate</span>';
    // Play mode itself is chosen in the Sample editor (one control); this group holds the numeric, automatable region.
    const playback = isSample ? group('playback', 'Region · Root note', `<span class="chip">${esc((PLAY_MODE_LABELS[mode] || mode).toUpperCase())}</span>`, `<div class="inst-region-grid">${parameter('rootNote', 'Root note', 0, 127, 1, inst.rootNote)}${parameter('start', 'Start', 0, .999, .001, inst.start, 'start', {
      name: 'Sample start'
    })}${parameter('end', 'End', .001, 1, .001, inst.end, 'end', {
      name: 'Sample end'
    })}${LOOPING_PLAY_MODES.has(mode)
        ? parameter('loopStart', 'Loop start', 0, .999, .001, inst.loopStart)
            + parameter('loopEnd', 'Loop end', .001, 1, .001, inst.loopEnd)
        : ''}${mode === 'slice' ? parameter('sliceCount', 'Slices', 1, 128, 1, inst.sliceCount) : ''}</div>`, true) : '';
    // Granular controls live in the Granular panel at the top of the Sample page (granular.js).
    const level = group('level', 'Volume · Pan · Tune', hint, `<div class="grid2">${parameter('volume', 'Volume', 0, 1, .01, inst.volume, 'instrumentVolume', {
      name: 'Instrument level'
    })}${parameter('pan', 'Pan', -1, 1, .01, inst.pan, 'instrumentPan', {
      name: 'Instrument pan'
    })}${parameter('tuneSemi', 'Tune', -48, 48, 1, tune.semis, 'tune', {
      name: 'Tune',
      automateValue: tuneValue
    })}${parameter('tuneFine', 'Fine', -99, 99, 1, tune.cents, 'tune', {
      name: 'Tune',
      automateValue: tuneValue
    })}</div>`);
    // Drive and bit depth sit with the filter: they colour the same tone chain (drive runs before the filter).
    const filter = group('filter', 'Filter · Drive', `<select data-inst-select="filterType" id="inst-filterType" aria-label="Filter type">`
        + `<option value="lowpass" ${inst.filterType === 'lowpass' ? 'selected' : ''}>Low-pass</option>`
        + `<option value="highpass" ${inst.filterType === 'highpass' ? 'selected' : ''}>High-pass</option>`
        + `<option value="bandpass" ${inst.filterType === 'bandpass' ? 'selected' : ''}>Band-pass</option>`
        + `</select>`, `<div class="grid2">${parameter('cutoff', 'Cutoff', 20, 20000, 1, inst.cutoff, 'cutoff', {
      name: 'Filter cutoff'
    })}${parameter('resonance', 'Resonance', 0, .95, .01, inst.resonance, 'resonance')}${parameter('drive', 'Drive', 0, 1, .01, inst.drive)}${parameter('bitDepth', 'Bit depth', 2, 16, 1, inst.bitDepth)}</div>`
        + `<p class="inst-mode-help inst-caption">Drive saturates before the filter; lower bit depths add lo-fi grit. `
        + `16-bit is clean.</p>`);
    const amp = group('amp', 'Amp envelope', '<span class="chip">ADSR</span>',
        `${renderEnvelope(inst)}<div class="grid2">${parameter('attack', 'Attack', 0, 60, .001, inst.attack)}${parameter('decay', 'Decay', 0, 60, .001, inst.decay)}${parameter('sustain', 'Sustain', 0, 1, .01, inst.sustain)}${parameter('release', 'Release', .0001, 60, .001, inst.release)}</div>`);
    const modulation = window.STModulation
        ? group('modulation', 'Filter envelope · LFOs', '', window.STModulation.render(inst), true)
        : '';
    return { playback, level, filter, amp, modulation };
  }

  // Sampler instruments are split into focused pages (⌥1–5 · ⌃Tab); see docs/INSTRUMENT_LAYOUT.md.
  // In Wavetable mode the Wavetable panel (Position, table, frame size) leads the Sample page; in Granular mode the Granular panel.
  function renderInstrumentPages(inst) {
    const g = renderSoundGroups(inst),
      parts = window.STSampler?.parts?.(inst) || {},
      pages = window.STInstrumentPages,
      wavetable = window.STWavetables?.render(inst, state.samples[state.instrument]) || '',
      granular = window.STGranular?.render(inst, state.samples[state.instrument]) || '';
    const page = (id, body) => pages ? `<div ${pages.attributes(inst, id)}>${body}</div>` : body;
    const voice = page('voice', `${parts.voice || ''}${parts.record || ''}`);
    return page('sample', `${wavetable}${granular}${parts.editor || ''}${g.level}${g.filter}${g.amp}${g.playback}`)
        + page('slices', parts.slices || '')
        + page('zones', parts.zones || '')
        + page('modulation', g.modulation)
        + voice;
  }

  function renderEnvelope(inst) {
    const max = Math.max(inst.attack + inst.decay + inst.release + .5, 1),
      a = inst.attack / max * 380,
      d = inst.decay / max * 380,
      r = inst.release / max * 380,
      s = 80 - inst.sustain * 64;
    return `<svg class="envelope" viewBox="0 0 420 88" preserveAspectRatio="none" role="img" aria-label="Volume ADSR `
        + `envelope"><path d="M10 80H410M10 48H410M10 16H410" stroke="#3d3f2d" stroke-width=".6"/>`
        + `<path d="M10 80L${10 + a} 16L${10 + a + d} ${s}H${410 - r}L410 80Z" fill="#403b26"/>`
        + `<path d="M10 80L${10 + a} 16L${10 + a + d} ${s}H${410 - r}L410 80" fill="none" stroke="#ed7144"`
        + ` stroke-width="2"/></svg><div class="envelope-labels"><span>Attack</span><span>Decay</span>`
        + `<span>Sustain</span><span>Release</span></div>`;
  }

  function renderPiano() {
    const base = (state.octave + 1) * 12,
      white = [0, 2, 4, 5, 7, 9, 11, 12, 14, 16, 17, 19, 21, 23],
      black = [1, 3, 6, 8, 10, 13, 15, 18, 20, 22],
      keys = ['Z', 'X', 'C', 'V', 'B', 'N', 'M', 'Q', 'W', 'E', 'R', 'T', 'Y', 'U'];
    return `<div class="piano" aria-label="Piano keyboard">${white.map((n, i) => `<button class="piano-key" id="piano-${base + n}" data-piano="${base + n}" tabindex="${n === 0 ? 0 : -1}"`
        + ` aria-label="Play ${noteText(base + n)}">${keys[i]}</button>`).join('')}${black.map(n => {
      const left = white.filter(w => w < n).length / white.length * 100 - 2.5;
      return `<button class="piano-key black" style="left:${left}%" id="piano-${base + n}" data-piano="${base + n}"`
          + ` tabindex="-1" aria-label="Play ${noteText(base + n)}"></button>`;
    }).join('')}</div>`;
  }

  function renderInstrument() {
    const inst = currentInstrument(),
      p = state.project;
    const plugin = p.tracks[state.track].instrument === state.instrument
        ? p.audioPlugins.find(x => x.owner === `track:${p.tracks[state.track].routingID}` && x.kind === 'instrument')
        : null;
    const midiInputs = state.midi.inputs || state.midi.sources || [],
      midiOutputs = state.midi.outputs || state.midi.destinations || [];
    return `<div class="workspace">${sidebar('instruments')}<section class="workspace-main">`
        + `<div class="view-title"><div><div class="eyebrow">Instrument ${String(state.instrument).padStart(2, '0')}</div>`
        + `<h1 style="margin-top:7px">${esc(inst.name)}</h1></div><div class="row"><button class="quiet"`
        + ` id="instrument-find" title="Find an instrument control or section · ⌘F">Find control <kbd>⌘F</kbd>`
        + `</button><button class="quiet" id="instrument-rename" title="Rename this instrument · right-click it in the list for more">Rename</button>`
        + `<button id="assign-instrument" title="Play this instrument on the selected track (${esc(p.tracks[state.track].name)})">Assign `
        + `to track “${esc(p.tracks[state.track].name)}”</button><button id="load-sample" title="Load an audio file into this sampler">Load sample…</button>`
        + `</div></div><div class="toolbar"><div class="instrument-type">${[['sample', 'Sampler'], ['au', 'Plug-in'], ['midi', 'External MIDI']].map(([k, n]) => `<button id="instrument-type-${k}" data-inst-type="${k}" tabindex="${inst.type === k ? 0 : -1}"`
        + ` aria-pressed="${inst.type === k}" class="${inst.type === k ? 'active' : ''}">${n}</button>`).join('')}</div>${window.STInstrumentPages?.renderBar(inst) || ''}<span `
        + `class="grow"></span><span class="muted inst-toolbar-hint"><kbd>⌥A</kbd> Automate focused parameter · Context `
        + `menu available</span></div><div class="view-padding" data-scroll="instrument">`
        + `<div class="instrument-layout">${inst.type === 'midi' ? `<div class="panel wide"><div class="panel-head"><h2>Hardware connection</h2>`
        + `<button id="midi-refresh">Refresh devices</button></div><div class="panel-body stack">`
        + `<div class="grid2"><label class="field"><span>MIDI input</span><select id="inst-midiInput"`
        + ` data-inst-select="midiInput"><option value="0">All enabled inputs</option>${inst.midiInput && !midiInputs.some(x => String(x.id) === String(inst.midiInput))
            ? `<option value="${esc(String(inst.midiInput))}" selected>Unavailable input · ${esc(String(inst.midiInput))}</option>`
            : ''}${midiInputs.map(x => `<option value="${x.id}" ${String(inst.midiInput) === String(x.id) ? 'selected' : ''}>${esc(x.name)}</option>`).join('')}</select>`
        + `</label><label class="field"><span>MIDI output</span><select id="inst-midiOutput" data-inst-select="midiOutput">`
        + `<option value="0">Studio default output</option>${inst.midiOutput && !midiOutputs.some(x => String(x.id) === String(inst.midiOutput))
            ? `<option value="${esc(String(inst.midiOutput))}" selected>Unavailable output · ${esc(String(inst.midiOutput))}</option>`
            : ''}${midiOutputs.map(x => `<option value="${x.id}" ${String(inst.midiOutput) === String(x.id) ? 'selected' : ''}>${esc(x.name)}</option>`).join('')}</select>`
        + `</label><label class="field"><span>Input channel (0 = omni)</span><input type="number" id="inst-midiInputChannel"`
        + ` data-inst-param="midiInputChannel" min="0" max="16" value="${inst.midiInputChannel || 0}">`
        + `</label><label class="field"><span>Output channel</span><input type="number" id="inst-midiChannel"`
        + ` data-inst-param="midiChannel" min="1" max="16" value="${inst.midiChannel || 1}">`
        + `</label></div><div class="notice info">Pattern notes and the computer keyboard send MIDI to your hardware. `
        + `Sample a patch to a new sampler track while keeping this hardware connection.</div>`
        + `<p id="hardware-input-summary" class="small-note">${esc(window.STRecording?.inputSummary(inst) || 'Current Studio input · no saved interface')}</p>`
        + `<div class="row"><button id="instrument-monitor">${state.audio.monitoring ? 'Disable' : 'Enable'}`
        + ` Studio input monitoring</button><button id="instrument-record-input">Sample this synth… <kbd>⌘⇧R</kbd>`
        + `</button><button id="instrument-line-inputs">Line inputs…</button><button id="instrument-audio-settings">Audio `
        + `settings</button></div></div></div>` : inst.type === 'au' ? plugin ? `<div class="panel wide instrument-plugin-overview" aria-label="Plug-in instrument">`
        + `<div class="plugin-overview-main"><div class="plugin-overview-name"><span>Plug-in instrument${plugin.format ? ' · ' + esc(plugin.format) : ''}</span>`
        + `<h2>${esc(plugin.name || inst.name)}</h2></div><div class="plugin-overview-actions">`
        + `<button id="browse-plugins">Browse instruments</button><button id="plugin-editor">Open plug-in editor</button>`
        + `<button id="plugin-parameters">Parameters & presets</button></div>`
        + `</div><details class="plugin-overview-help" id="plugin-overview-help"><summary id="plugin-overview-help-summary">About `
        + `instrument controls</summary><p>Each track hosts one instrument plug-in. Assign it to a track before entering `
        + `notes; hosted plug-ins cannot switch per step. Use the native editor for the complete interface, or the accessible `
        + `parameter list for keyboard editing and automation. Browse NKS presets in the Library; supported instruments `
        + `expose their mapped controls below.</p></details></div>` : `<div class="panel wide"><div class="panel-head"><h2>Plug-in instrument</h2>`
        + `<span class="chip teal">${plugin ? 'LOADED' : 'NO INSTRUMENT'}</span></div>`
        + `<div class="panel-body stack"><div><h2>${esc(plugin?.name || inst.plugin?.name || 'Choose your sound')}</h2>`
        + `<p class="muted" style="margin-top:8px;font-size:12px;line-height:1.6">Each track hosts one instrument plug-in. `
        + `Assign it to a track before entering notes; hosted plug-ins cannot switch per step. Use the native editor `
        + `for the complete interface, or the accessible parameter list for keyboard editing and automation.</p>`
        + `</div><div class="row"><button id="browse-plugins">Browse instruments</button>${plugin
            ? '<button id="plugin-editor">Open plug-in editor</button><button id="plugin-parameters">Parameters & presets</button>'
            : ''}</div>`
        + `<div class="notice info">Browse NKS presets in the Library. Compatibility is checked when you load a preset; `
        + `supported instruments expose their mapped controls here.</div></div></div>` : renderInstrumentPages(inst)}${window.STNKSControls?.render(inst, plugin) || ''}${window.STInstrumentEffects?.render(inst, state.instrument) || ''}<div `
        + `class="panel wide inst-perform" aria-label="Play & perform"><div class="panel-head">`
        + `<h2>Play & perform</h2><div class="row"><label class="muted" style="font-size:11px">Octave</label>`
        + `<select id="instrument-octave">${[0, 1, 2, 3, 4, 5, 6, 7, 8].map(x => `<option ${x === state.octave ? 'selected' : ''}>${x}</option>`).join('')}</select>`
        + `<select id="instrument-chord" aria-label="Chord mode">${Object.entries(chords).map(([k, v]) => `<option value="${k}" ${state.chord === k ? 'selected' : ''}>${v.name}</option>`).join('')}</select>`
        + `</div></div><div class="panel-body">${renderPiano()}<p class="muted" style="font-size:11px;margin-top:12px">Toggle `
        + `computer keyboard with ⌘P. Z–M plays the lower octave; Q–U the upper. [ / ] shifts octave. Chords and scale `
        + `lock apply to live playing and recording.</p></div></div></div></div></section></div>`;
  }

  Object.assign(app, { renderInstrument });
})();
