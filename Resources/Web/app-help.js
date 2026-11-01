/* Keyboard shortcut reference (Help). */
(function () {
  'use strict';

  const app = window.STApp;
  const { $, $$, openModal } = app;

  // Reading order of the reference: everyday keys first, specialised editors next, the controller last.
  const SECTION_ORDER = ['Getting around', 'Patterns, tracks & instruments', 'Playing & recording', 'Pattern editing',
    'Typing values · pattern grid', 'Pattern tools · selection or whole track', 'Track groups and kits',
    'Keyboard performance', 'Song arranger', 'Instrument pages · Sampler', 'Sample editor · waveform focused',
    'Slice map', 'Mixer', 'Send effects', 'Automation lanes', 'Audio clips · Song timeline focused',
    'Project & automation', 'Blocks · keyboard navigation', 'Effect codes', 'Maschine MK3 controller (in development)'];

  /** Sections in SECTION_ORDER; any section not listed keeps its place among the others, before the controller. */
  function ordered(sections) {
    const rank = name => {
      const index = SECTION_ORDER.indexOf(name);
      return index < 0 ? SECTION_ORDER.length - 1.5 : index;
    };
    return sections.map((section, index) => ({ section, index }))
        .sort((a, b) => rank(a.section[0]) - rank(b.section[0]) || a.index - b.index)
        .map(x => x.section);
  }

  /** The shortcut reference: [section, [[action, keys], …]]. */
  function helpSections() {
    return ordered(rawSections());
  }

  function rawSections() {
    return [['Patterns, tracks & instruments',
        [['Previous / next pattern', '⌘[ / ⌘]'], ['Previous / next instrument', '⌘⇧[ / ⌘⇧]'],
            ['New pattern', '⌘⇧N'], ['Add track / duplicate selected track', '⌘T / ⌘⇧T'],
            ['Pattern or instrument actions (rename, duplicate, type, remove…)', 'Right-click in the list · ⇧F10']]],
        ['Maschine MK3 controller (in development)',
        (globalThis.STMK3?.CONTROL_MAP || []).map(([control, press, shift]) => [`${control} · ${press}${shift ? ` · Shift: ${shift}` : ''}`,
            control])],
        ['Song arranger',
            [['Select / extend blocks', '↑ ↓ / ⇧↑ ↓'], ['Change pattern', '← → / type number'],
                ['Fewer / more repeats', '− / +'], ['Move blocks', '⌥↑ ↓'], ['Insert next / new pattern', 'I / N'],
                ['Duplicate / clone as new pattern', '⌘D / ⇧⌘D'], ['Remove blocks', 'Delete'],
                ['Loop section', 'L / ⌥⇧L'], ['Play from block', 'P / ⇧Return'],
                ['Edit / rename / colour', 'Return / E / C'], ['Follow playhead', 'F']]],
        ['Send effects',
            [['Delay & reverb returns', '⌘⇧X'], ['Choose / adjust', '↑↓ / ←→ (⇧ coarse, ⌥ fine)'],
                ['Type value / reset', 'Return / 0']]],
        ['Getting around',
            [['Command palette', '⌘K'], ['Pattern / Instrument / Browser', '⌘1 / ⌘2 / ⌘3'],
                ['Mixer / Automation / Song', '⌘4 / ⌘5 / ⌘6'], ['Recording', '⌘7'],
                ['Next / previous workspace section', 'F6 / ⇧F6'], ['Move within lists / control groups', 'Arrows'],
                ['Next control in section', 'Tab'], ['Close / stop / panic', 'Escape'],
                ['Find in current view · instrument control', '⌘F'],
                ['Context menu for focused item', '⇧F10 / Menu key'], ['Audio & MIDI settings', '⌘,'],
                ['Full screen', '⌃⌘F']]],
        ['Blocks · keyboard navigation',
            [['Move to the block left / right / above / below · anywhere', '⌥⌘← / ⌥⌘→ / ⌥⌘↑ / ⌥⌘↓'],
                ['Same, on Pattern, Instrument, Mixer, Browser, Song, Recording, Automation and Settings', '⌥← → ↑ ↓'],
                ['Pattern grid keeps all its keys (⌥↑↓ transpose, Tab, Esc): leave it with', '⌥⌘ + arrows · F6'],
                ['Back to the pattern grid at the cursor cell · from anywhere', '⌘0'],
                ['Pattern track headers: M S ◂ × and instrument of every track, group ▾ M S · press', '← → ↑ ↓ · Return / Space'],
                ['…except where the focused control uses ⌥ + arrows (text fields, song blocks, clips, envelopes, waveform, insert tiles, plug-in tabs) once you used it', '⌥⌘ + arrows'],
                ['Sliders and knobs: fine adjustment while you are adjusting (after ↑ ↓ or a click)', '⌥↑ ⌥↓'],
                ['Instrument: next / previous control in a block · adjust the slider', '← → · ↑ ↓ (⇧ coarse)'],
                ['Instrument: type a value for the slider · back to the slider', 'Return · Return'],
                ['Next / previous block in reading order', 'F6 / ⇧F6'],
                ['Next / previous control in a block', 'Tab / ⇧Tab · arrows'],
                ['Back out of a control to its block · then as before (stop)', 'Escape'],
                ['Go into the selected block', 'Return'],
                ['Dialogs: blocks stay inside; Tab cycles; Esc closes', '⌥⌘ + arrows · Tab · Escape']]],
        ['Pattern editing',
            [['Move cell / column', '↑ ↓ ← →'], ['Move track / leave grid', 'Tab / ⌃Tab'],
                ['Select steps', '⇧ + arrows'], ['Copy / cut / paste', '⌘C / ⌘X / ⌘V'],
                ['Repeat phrase to pattern end', '⌘D'], ['Transpose & hear · semitone / octave', '⌥↑↓ / ⌥⇧↑↓'],
                ['Insert / remove row', 'Insert or ⇧Return / ⇧Delete'], ['Clear selection', 'Delete'],
                ['Choose step effect (FX column)', 'Enter'], ['Edit step down / up', '{ / }'],
                ['Follow playback', '⌥F'], ['Loop selected rows / stop looping', '⌥⇧L'],
                ['Mute / solo track', '⌥M / ⌥S'], ['Unmute all, clear solo', '⌥⇧M'],
                ['Automate parameter…', '⌥P / right-click'], ['Select all steps', '⌘A'],
                ['Rename track / pattern · set length', '⌘K “Rename” · ⌘K “Pattern length”']]],
        ['Track groups and kits',
            [['Group selected tracks / take track out of group', '⌘G / ⌘⇧G'],
                ['Collapse / expand group (Return on a collapsed group expands it)', '⌥J'],
                ['Move track left / right (into or out of a group)', '⌘⇧← / ⌘⇧→'],
                ['Reorder tracks and groups: drag a track or group header (also mixer strips); Esc cancels', 'Drag'],
                ['Hide / show empty tracks', '⌥U'],
                ['Collapse / expand the track to a slim column (or double-click its header)', '⌥⇧J'],
                ['Delete selected tracks (asks first when they have notes)', '⌘⌫'],
                ['Add / remove a note column (more notes per row): + / − on the track header, or ⌘K', '+ / −'],
                ['Pad keys: Z S X D C V … play and enter the group’s tracks', '⌥Z']]],
        ['Pattern tools · selection or whole track',
            [['Interpolate velocity / FX ramp', '⌥I'], ['Humanize / randomize velocity', '⌥H / ⌥⇧H'],
                ['Velocity louder / softer', '⌥. / ⌥,'], ['Reverse steps', '⌥R'], ['Nudge steps up / down', '⌘↑ / ⌘↓'],
                ['Stretch ×2 / compress ½', '⌥E / ⌥⇧E'], ['Double / halve pattern', '⌥D / ⌥⇧D'],
                ['Clear step effects', '⌥X'], ['Render selection to sample (bounce in place)', '⌥B'],
                ['Quick step effects', 'Right-click / ⌘K “Step effect”']]],
        ['Keyboard performance',
            [['Next / previous chord', '⌥C / ⌥⇧C'], ['Next / previous chord set · Off is in the cycle', '⌥G / ⌥⇧G'],
                ['Chord set: chord 1, 2, 3… (MIDI from C3)', 'Z S X D C V G B …'],
                ['Choose a chord set by name', '⌘K → Chord set'],
                ['Chord settings on MIDI keyboard', '⌘K → Apply to MIDI'], ['Next / previous scale', '⌥K / ⌥⇧K'],
                ['Scale root up / down', '⌥T / ⌥⇧T'], ['Snap to key / scale degrees', '⌥L'],
                ['Keyboard velocity', '⌥V'], ['Note repeat / arp on-off', '⌥N'], ['Next repeat / arp mode', '⌥⇧N'],
                ['Repeat rate slower / faster', '⌥− / ⌥=']]],
        ['Playing & recording',
            [['Start / stop playback', 'Space'], ['Play from start (pattern / song)', '⇧Space'], ['Metronome', '⌘⇧M'],
                ['Tempo', '⌘K “Set tempo”'], ['Song / Pattern playback mode', 'Transport Mode · Tab + arrows'],
                ['Find playback mode actions', '⌘K → Playback mode'],
                ['Arm recording + keyboard piano', '⌘R / R (piano off)'],
                ['Open audio recorder / start / stop capture', '⌘⇧R'], ['Toggle keyboard piano', '⌘P'],
                ['Lower octave notes', 'Z S X D C V G B H N J M'], ['Upper octave notes', 'Q 2 W 3 E R 5 T 6 Y 7 U'],
                ['Octave down / up', '[ / ]'], ['Note off / cut step · also OFF / CUT below the grid', '1 / ⇧1 (note column)'],
                ['Show / hide the optional Gate column', '⌘K “Gate column” · Gate button'],
                ['Audition selected note (note column)', 'Enter']]],
        ['Typing values · pattern grid',
            [['INS column: instrument number (two digits)', '0–9'], ['VEL column: velocity 1–127', '0–9 · Enter'],
                ['FX column: effect code, then value (R04, Y50, P-30)', 'Letter · 0–9 · −'],
                ['Clear the effect under the cursor', 'Delete'], ['Cancel a half-typed value', 'Escape'],
                ['All effect codes', '⌘K “Effect codes”']]],
        ['Effect codes', (app.effectCodeRows?.() || [])],
        ['Instrument pages · Sampler',
            [['Sampler: Sample / Slices / Zones / Modulation / Voice & Record', '⌥1 / ⌥2 / ⌥3 / ⌥4 / ⌥5'],
                ['Wavetable synth: new sampler in Wavetable mode', '⌘K “Add wavetable synth” · + Wavetable'],
                ['Wavetable Position (Sample page) · automate', '↑ ↓ ⇧ / ⌥A'],
                ['Next / previous page', '⌃Tab / ⌃⇧Tab'],
                ['Page tabs focused · choose / enter page', '← → Home End / ↓'],
                ['Jump to any control on any page', '⌘F']]],
        ['Slice map',
            [['Choose slice from its audition button', '↑ ↓ / Home / End'],
                ['Audition slice · up to 2 seconds', 'Space / Enter'], ['Edit selected slice start', 'Tab'],
                ['Commit start and hear slice', 'Enter'], ['Stop slice preview', 'Escape']]],
        ['Mixer',
            [['Previous / next channel · first / last', '← → / Home / End'], ['Adjust focused fader / send', '↑ ↓'],
                ['Next control in channel', 'Tab'], ['Open / close focused insert effect', '→ / ←'],
                ['Move insert effect up / down', '⌥↑ / ⌥↓'], ['Add insert effect', '⌘K “Add insert effect”'],
                ['Close effect picker', 'Escape']]],
        ['Automation lanes',
            [['Parameters → lanes → envelope', '↓ / →'], ['Previous / next pattern · keeps the parameter', '[ / ]'],
                ['Pattern lane ⇄ song timeline', '\\'], ['Song timeline: previous / next slot', 'PageUp / PageDown'],
                ['Song timeline: make slot unique · zoom', 'U · + / − / 0'],
                ['Song timeline: draw a sweep across slots', '⇧-drag · ⌘K “Sweep”'],
                ['Copy lane to other patterns', '⌘K “Copy lane” · right-click lane'],
                ['Add point (envelope focused)', 'Insert / Enter'],
                ['Previous / next point · first / last', '⌥← / ⌥→ · Home / End'],
                ['Move point · by beat', '← → / ⇧← →'], ['Point value · coarse', '↑ ↓ / ⇧↑ ↓'],
                ['Delete point', 'Delete'], ['Copy / paste / clear / delete lane', '⌘K “Automation”']]],
        ['Audio clips · Song timeline focused',
            [['Select clip · first / last', '↑ ↓ / Home / End'], ['Move / trim end / slip audio', '← → / ⇧← → / ⌥← →'],
                ['Move clip to track above / below', '⌥↑ / ⌥↓'], ['Split at edit cursor', 'S'],
                ['Duplicate / delete clip', '⌘D / Delete'], ['Edit cursor back / forward', '[ / ]'],
                ['Takes: open / previous / next', 'T / PageUp / PageDown']]],
        ['Sample editor · waveform focused',
            [['Choose marker', '↑ ↓'], ['Nudge marker · coarse / one frame', '← → / ⇧← → / ⌥← →'],
                ['Marker to its limit', 'Home / End'], ['Zoom in / out / whole / region', '+ / − / 0 / F'],
                ['Scroll view', ', / .'], ['Audition / stop', 'Space / Escape'],
                ['Trim / fade in / fade out', 'T / I / O'], ['Normalize / reverse / gain', 'N / R / G'],
                ['Zero-crossing snap / snap marker', 'Z / ⇧Z'], ['Add / remove slice', 'A / Delete'],
                ['Slices to pattern', 'P'], ['Next / previous play mode', 'M / ⇧M'],
                ['All editor actions', '⌘K “Sample editor”']]],
        ['Project & automation',
            [['Save / save as', '⌘S / ⌘⇧S'], ['Open / new project', '⌘O / ⌘N'], ['Undo / redo', '⌘Z / ⌘⇧Z'],
                ['Export audio', '⌘⇧E'],
                ['Freeze / unfreeze selected track', '⌘K “Freeze” · right-click track header or mixer strip'],
                ['Adjust focused instrument control · fine / coarse', '↑↓ / ⇧↑↓'],
                ['Automate focused parameter', '⌥A / right-click'],
                ['Automation write: Read ⇄ Touch/Latch/Write', '⌥W'],
                ['Cycle write mode Touch → Latch → Write', '⌥⇧W'],
                ['Arm parameter for write / learn MIDI CC', 'right-click a control'],
                ['Open this reference', '? / ⌘/'], ['Browse NKS instruments', '⌘⇧B'],
                ['Browser load / new instrument', 'Enter / ⌥Enter'], ['Browser preview / favorite', 'Space / F'],
                ['Browser tabs: Samples / NKS / Plug-ins', '← → on the tabs'],
                ['Browse plug-ins', '⌘K “Browse plug-ins”']]]];
  }

  function showHelp() {
    const sections = helpSections();
    openModal('Make music without reaching for the mouse', `<input id="help-search" class="palette-input help-search" type="search" placeholder="Filter shortcuts…"`
        + ` autocomplete="off" aria-label="Filter shortcuts" aria-controls="help-grid">`
        + `<div class="shortcut-grid" id="help-grid" tabindex="0" aria-label="Keyboard shortcuts. Arrow keys scroll.">${sections.map(([name, rows]) => `<section><h3 style="color:var(--teal);margin-bottom:15px">${name}</h3>${rows.map(([text, key]) => `<div class="shortcut-item"><span>${text}</span><kbd>${key}</kbd></div>`).join('')}</section>`).join('')}</div>`
        + `<div class="notice info" style="margin-top:24px">Arm ● to enable note recording and keyboard piano together. `
        + `Use ⌘P to play freely without recording. Text fields always keep normal typing. As in a classic tracker a `
        + `note plays until the next note on its track, an OFF (1) or a CUT (⇧1); a one-shot sample ends by itself. With `
        + `transport stopped, notes advance by the edit step. The optional Gate column (Gate button) gives a note a length `
        + `in rows; while it is shown, key release during playback records the held length, and notes entered in one `
        + `tracker step share a gate that ends when the last chord key is released. Key, scale and chord are saved with the song. Scale degrees mode plays only notes in `
        + `key, one degree per key. Note repeat and the arpeggiator are for live playing; while recording is armed, `
        + `keys enter notes normally. In the sample editor letters play notes while the keyboard piano is on.</div>`, '', {
      kind: 'help'
    });
    const filter = $('#help-search');
    if (filter) {
      filter.oninput = () => {
        const words = app.commandSearchText(filter.value).split(' ').filter(Boolean);
        for (const section of $$('#help-grid section')) {
          let shown = 0;
          const heading = app.commandSearchText(section.querySelector('h3')?.textContent || '');
          for (const item of section.querySelectorAll('.shortcut-item')) {
            const text = heading + ' ' + app.commandSearchText(item.textContent);
            const match = words.every(word => text.includes(word));
            item.hidden = !match;
            if (match) shown++;
          }
          section.hidden = !shown;
        }
      };
      filter.onkeydown = e => {
        if (e.key === 'ArrowDown' || e.key === 'PageDown') {
          e.preventDefault();
          $('#help-grid')?.focus();
        }
      };
    }
  }

  Object.assign(app, { helpSections, showHelp });
})();
