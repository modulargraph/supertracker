# SuperTracker user guide

This guide describes the current source of SuperTracker. It covers every workspace and the exact keyboard shortcuts. Press `?` in the app for the built-in keyboard reference, and `Cmd+K` to search every command by name.

[FEATURE_STATUS.md](FEATURE_STATUS.md) records how each feature was verified and what has not been tested yet.

## Build and launch

You need Rust (stable, from rustup.rs); on a Mac also Apple's command-line developer tools, on Linux the packages listed in the README. Build and open it:

```sh
./scripts/build.sh
open build/SuperTracker.app        # Linux: build/supertracker/supertracker
```

`./scripts/build.sh` builds a release app at `build/SuperTracker.app`; `./scripts/run.sh` builds a debug host and runs it from the repository. All builds share recovery storage under `~/Library/Application Support/SuperTracker`, so save and quit the running app before you switch builds.

The interface is bundled into the native app. This is a locally signed development build, not a notarized distribution. Opening the bundled HTML file directly only shows an interface preview without audio.

Live audio and hardware use still need real-world testing: offline rendering is verified, but a past live test received no audio callbacks from the device.

## Project and navigation

The seven main workspaces are Pattern, Instrument, Browser, Mixer, Automation, Song and Recording. Their navigation buttons display `Cmd+1` through `Cmd+7` in that order. The toolbar shows the current project, transport, tempo, grid and computer-keyboard piano state.

Use the File menu or `Cmd+N`, `Cmd+O`, `Cmd+S` and `Cmd+Shift+S` for new/open/save/save-as.

These structure keys work from every workspace (also while a text field has focus) and appear in the Pattern, Track and Instrument menus:

| Keys | Action |
| --- | --- |
| `Cmd+[` / `Cmd+]` | Previous / next pattern in the pattern list (wraps around; in Automation it keeps the parameter, like `[` / `]`) |
| `Cmd+Shift+[` / `Cmd+Shift+]` | Previous / next instrument (as a click in the instrument list) |
| `Cmd+Shift+N` | New pattern |
| `Cmd+T` / `Cmd+Shift+T` | Add track / duplicate the selected track |

Plain `[` / `]` still change the keyboard octave. Projects use the `.supertracker` extension. Saving collects referenced sample files into a neighboring `.media` folder by default; keep that folder beside the project when moving it. Recovery data is stored under `~/Library/Application Support/SuperTracker`.

New/Open, MIDI import and loading the demo offer Save, Discard or Cancel when the session has changes. Save completes before replacement; a canceled or failed save leaves the current session. Edits made in a vendor AU window are captured for this decision. A failed or malformed native state capture prevents an unsafe save or session replacement. Opening a project from Finder uses the same flow and waits for application startup or the current session transition. Hidden-WebKit/Bridge tests verify save/open, media collection and recovery. The physical QA run canceled a native Save panel and saved a private fixture; its destination field used accessibility entry. Complete file/close-dialog and Finder workflows remain unverified.

`Cmd+K` opens the searchable command palette. `Cmd+/` opens keyboard help. Text-entry fields receive typed text rather than musical notes. F6 moves to the next workspace section and Shift-F6 to the previous one (use Fn if macOS reserves those keys). Tab moves through ordinary controls; a focused grid/list handles its own arrows. In the tracker, Tab changes tracks and `Ctrl+Tab` leaves the grid. In the mixer, Left/Right moves between channels at the same control. Escape dismisses an ordinary dialog; in the browser or Slice Map it first stops that local preview. Outside these contexts it stops playback and releases notes. Finish an active audio recording through its Stop button first.

### Context menus

Right-click anything to open a native macOS menu for it. From the keyboard, press `Shift+F10` or the Menu key to open the menu for the focused element. Menus exist for pattern cells and selections (cut/copy/paste, select all, insert/delete row, pattern tools, quick step effects), track headers (mute/solo, rename, duplicate, move left/right, group, collapse, note columns, freeze), the pattern and instrument lists, automatable controls (Automate, arm for automation write, MIDI learn), browser rows, plug-ins, song blocks, the sample editor waveform and text fields. Interface text is not selectable outside text fields, so dragging or double-clicking does not highlight labels.

The instrument list (beside the Instrument view) has its own menu for the instrument you right-click: Edit instrument, Rename…, Assign to the selected track, Load sample… and Show sample in Finder (samplers), Find control…, Type (Sampler / Plug-in / External MIDI), Add instrument, Add wavetable synth and Remove instrument. Insert row and Delete row in the grid menu act like `Shift+Return` and `Shift+Delete`: every track of the selection (or of a collapsed group) shifts from the cursor row.

To start a short pattern: choose Pattern, enable keyboard audition with `Cmd+P`, select a sound, then arm note entry with `Cmd+R`. While stopped, each note advances by the selected edit step. Disarm before auditioning changes. Space starts/stops transport when the focus is not a text field, browser result or button with its own Space action.

Choose **Song** or **Pattern** using **Mode** beside Play. It stays selected when you move to Mixer, Instrument or another workspace, and after Stop. `Cmd+K` → **Playback mode: Song** or **Playback mode: Pattern** gives the same keyboard action. Stop before changing modes; the selector explains when preparation or stopping is pending. A new/opened project starts in Pattern mode. Song starts at the selected sequence position; Pattern starts at the selected step. The Song view’s **Play song** button explicitly chooses Song. Playback uses the selection made when Play was pressed, so later navigation cannot silently change a pending start.

## Browse sounds

Open Browser (`Cmd+3`); search receives focus. Scan known locations or add a folder. The default roots are the standard locations that exist on your Mac: Apple Loops, `~/Music/Loopcloud`, the content folders Native Access installed (expansions and NKS libraries), `~/Documents/Native Instruments` user content and your SuperTracker kits folder. Use **Add folder** for anything else, such as a sample drive; an added folder stays listed (as Offline) while its drive is disconnected. Browsing reads those libraries; it does not rename or modify their files.

A development scan of a large library indexed about 164,000 supported audio/NKS entries. Scan diagnostics retain per-root counts and errors; an empty root or an unexpectedly small count does not establish that all files were readable. Previewing/loading representative formats and reconnecting the drive are separate acceptance checks.

`Cmd+F` focuses search. In search, Down moves to results; arrows select a result. Auto preview auditions selected samples when enabled. With the result list focused, `Space` auditions/stops preview, `Return` loads the sample, `Option+Return` creates a new sample instrument and `F` toggles favorite. Browser Space auditions; it does not toggle transport. Escape cancels queued, loading or playing preview before changing a search/filter or stopping the song. After changing a query/filter, old results cannot load or preview until matching results arrive. If the latest search was canceled during a change, it is retried so clearing filters can show current results.

The browser has collapsible Source, Category, Pack, Key, Type & length, Tempo & format, Tags and Locations filters. Select several choices in one group to match any of them; selections across groups must all match. For example, Kick + Snare and Loopcloud + a Maschine expansion means either drum category from either selected source. Active chips remove individual choices; Clear all resets filters and search.

Tab enters each choice list once, then Up/Down/Home/End moves between options and Space toggles a choice. Left collapses and Right expands a filter group. Pack/key/tag filters also have their own search. Duration/BPM ranges can include unknown metadata; with no bounds, the unknown checkbox means unknown-only. Length metadata loads in bounded background batches, so unknown lengths may become known during browsing. Incoming results keep an unfinished numeric filter draft and its focus; commit the field before the filter changes. Loop/one-shot labels and filename-derived tempo/key remain hints, not audio analysis.

Search examples supported by the index:

```text
kick analog
pack:"Anima Ascent" category:drums
bpm:120-130 key:Am
kind:sample ext:wav
tag:warm
```

Words are combined, and quoted field values preserve spaces. Filename/folder-derived tempo, key and category are hints, not audio analysis. NKS fields come from readable preset metadata. Offline libraries keep their cached entries; reconnect the drive and rescan before loading unavailable files.

Select a range in Sounds with Shift-arrows, or select the current page with Command-A. Press **E** for Edit details. Tags, category, key, tempo and sample type are editable; unchanged fields keep their existing values. Batch tags add by default. Use inferred removes an override. The browser saves local metadata separately from source samples and NKS files. See [local library metadata](LIBRARY_METADATA.md) for restore, persistence and moved-file behavior.

### Sample browser on real libraries (in progress)

Previewing, loading and favoriting sounds across the real Native Access and Loopcloud libraries is being verified now. This section will be completed when that work lands.

## Instruments and sample editing

An instrument supplies a sound; a track supplies its sequenced events and processing. Instrument (`Cmd+2`) lets you select/assign a synth, sample, AU/VST3/CLAP or external MIDI source. Plug-in instruments use their exposed parameters for internal sound controls (see [Plug-in control pages](#plug-in-control-pages)).

### Instrument page

Built-in synths and samplers show their controls in Polyend-style groups: **Play mode & loop** (samplers: play mode, root note, start/end and loop points), **Volume · Pan · Tune · Fine**, **Filter** (low-pass, high-pass or band-pass; cutoff and resonance), **Amp envelope** (ADSR), **Drive · Bit depth**, and **Filter envelope · LFOs**. Every value is shown in readable units: milliseconds/seconds, Hz/kHz, dB (with −∞), pan as Center or a percentage left/right, semitones (st), cents (ct), note names, bits and beats. Sample positions show a percentage and the time in the sample.

Focus a control and use Up/Down to make a fine change; hold Shift or use Page Up/Page Down for a coarse change. Left/Right also adjusts sliders. Times and frequencies move in musical proportions: cutoff moves by a semitone (coarse: an octave), tune by a semitone (coarse: an octave), fine tune by one cent (coarse: ten), percentages by 1% (coarse: 10%). A burst of arrow presses on one control is a single Undo step. `Option+A` on any of these controls opens its automation lane.

Press `Cmd+F` or **Find control** to search the Instrument view's current controls and sections. Up/Down selects a result; Return jumps to it and opens any collapsed settings it needs. Escape cancels search and returns to the previous control. Try `slice map`, `minimum gap`, `filter` or `volume`. The jump only focuses the target: it does not change a value, activate a button or audition a sound. Numeric fields take priority over their matching sliders; unavailable controls are labeled and cannot be selected as a destination. This searches the rendered Instrument view, not every hidden page in a vendor editor.

Open **Studio settings** with `Cmd+,`. Existing MIDI selections are retained unless you change them, including multiple or disconnected ports shown as **Keep current**. Applying an output-device or buffer change leaves MIDI and audio input untouched. **System default (current)** selects the device currently used by macOS; it does not establish automatic following of later system changes. Apply and Cancel remain visible while the settings scroll.

**MIDI clock.** Studio settings (⌘,) → **MIDI clock** → **Send MIDI clock** sends 24 pulses per beat to your MIDI outputs. Clock alone sets a device's tempo (arpeggiators, synced LFOs, delays) without starting its sequencer, and keeps running at the project tempo while stopped (whenever audio is running). Per output, untick **Clock** to leave it out, or tick **Start/Stop** to also run its own sequencer with SuperTracker's transport (Play from the start sends Start; Play mid-song sends Song Position and Continue).

For a hardware synth, select **External MIDI** and set its MIDI input/output ports, input channel (`0` listens to all channels) and output channel (`1–16`). Pattern notes and the computer keyboard address that instrument. Choose the interface in Audio & MIDI settings, then use **Line inputs…** above the pattern grid to route individual mono channels or stereo pairs through separate tracks. Several inputs can run simultaneously. Use **Record sample…** to capture the selected return into a new sampler track. See [hardware inputs and MIDI output](HARDWARE_INPUT_OUTPUT.md). MIDI carries performance messages; an offline export includes the hardware sound only after its audio has been recorded.

Incoming sustain belongs to its MIDI source and channel. A pedal on another source/channel cannot hold or release that note. Key releases and pedal-up for already-held notes still reach their original destination after the selected target’s input filter changes; new notes obey the new filter. Five production-Host ingress tests verify these rules with offline synth audio, alongside seven pure state tests. They do not use external inputs or MIDI broadcasts and do not establish physical-controller, pedal or CoreMIDI parser acceptance.

The Modulation section gives native synths/samplers a separate filter ADSR and two independent LFOs. Filter Amount is a signed cutoff offset in semitones: positive opens the filter, negative closes it, and zero disables the envelope's effect. Each LFO chooses sine, triangle, saw, square or random/hold, plus cutoff, pitch, level, pan or sample-position destination. Position applies to samplers. Depth is normalized 0–1; its description translates depth into the destination's range.

Each LFO's **Timing** menu selects Free · Hz or Tempo sync. Sync offers straight, dotted and triplet note divisions plus an editable Cycle value in quarter-note beats: an eighth note is `0.5`, a dotted eighth is `0.75`, and an eighth-note triplet is `1/3`. These values work independently of project meter. The range is 1/64–64 quarter-note beats per cycle. In the Cycle field, Up/Down adjusts by 1/64 beat; Shift uses one beat. `Option+A` creates a beat-period lane. The old Hz value and Hz lane remain stored but inactive during sync; switching back restores their meaning. The automation picker labels inactive rate targets and keeps their points editable. Beat automation is inactive in Hz mode.

Phase is per voice: a fresh note starts its LFO cycle, while legato and live tempo/division/mode changes preserve phase. Tempo sync controls cycle speed; it does not lock every voice to the song playhead. Numeric envelope/rate/depth/cycle fields support `Option+A`; waveform, destination and timing-mode choices are instrument settings. Native PCM tests verify both slots and beat automation. Hidden WebKit also checks exact triplets, beat arrows, Undo, Option-A and focus at both target sizes. OS-level LFO select-menu/Tab behavior and a complete audible modulation workflow remain unverified.

For samples, use Load Sample or the browser. Playback controls include start/end and loop-start/loop-end numeric fields, so precise regions do not require dragging the waveform. Play mode chooses **1-Shot** (plays the region once), **Forward loop** (plays from Start, then repeats Loop start → Loop end while the note is held), **Backward loop** (plays backwards from End, then keeps looping backwards between the loop points), **Ping-pong** (bounces between the loop points), **Slice** (each key up from the root plays the next slice at its original pitch), **Wavetable** (treats the region as 2048-frame single cycles; Wavetable position morphs between them) or **Granular** (overlapping grains around Grain position; Grain size sets their length). One-shot and slice edges are declicked and loop seams are smoothed. Voice Behavior selects polyphonic, monophonic or legato playing; instruments in the same nonzero choke group cut each other off.

Use **Cmd-Shift-R** to open the recorder. With an External MIDI instrument selected, it opens **Sample [instrument name]**. Choose the audio interface, one input for mono or two ordered inputs for stereo, and the pitch you will record as Root note. Starting capture remembers that interface's persistent identity, channels and root note for this hardware instrument. A mono interface starts with its single valid input. Missing saved interfaces or invalid saved channels require an explicit replacement. Opening the dialog does not start recording or monitoring. The name field is ready for typing while devices load. Start/Stop, Close, recording time and status stay visible while you scroll the settings.

Press **Cmd-Shift-R** again to start, play the note, then press it once more to stop and load. Hardware capture adds a new sampler instrument and neutral track; the hardware instrument, its track and MIDI settings stay intact. Focus moves to Sample start for trimming silence. **Cmd-F** can jump to Slice Map, Equal slices or transient detection. The previous input device, channels, metering and monitoring are restored when capture finishes, fails or the dialog closes. Failed restoration is reported; a saved take remains available if loading fails. Mono input produces a one-channel WAV; stereo input and master resampling produce two channels. Other recorder contexts can replace the current sample or add an audio clip. Generated PCM and UI tests pass; actual line-input recording and the complete musical hardware workflow still need acceptance.

Slice Map offers three starting points: Equal slices with a count; Chop by beats with an explicit region length and note grid; or Detect transients. For an eight-beat break, a sixteenth-note grid makes 32 equal slices. Beat-grid chopping does not detect tempo or stretch the audio. Transient detection uses sensitivity, minimum gap and maximum-slice settings within the selected playback region; it adds markers without writing the source. Cancel keeps existing markers, and results are discarded if the destination or map changed during analysis. Each applied map is one undo step. Five standalone detector suites and the slicing UI regressions pass; the complete native record/chop workflow still awaits acceptance.

Add Marker splits the widest slice. In Slice Map, focus a slice’s ▶ button and use Up/Down or Home/End to choose a slice; Space or Return auditions it at Root note for up to two seconds. Tab reaches the selected slice’s start field; arrows edit its percentage, and Return commits and hears the edit. The first marker stays at zero. Escape stops this preview without stopping the song, and leaving the map cancels it, including preparation still waiting on the audio graph. Audition does not enter notes, move the pattern cursor or change record arm; a marker edit remains one undo step. Hidden WebKit/Bridge tests cover slice 128 at root 127, edits, focus and cancellation at both target sizes. Physical QA verified editing slice 2 to 8%, requesting audition and saving that marker, with the master muted. Listening, physical recording and the complete musical workflow remain unverified.

In Slice play mode, notes ascending from the root select successive slices. A step's **Slice** effect (`SLC`, 1–128) explicitly chooses a slice while the note transposes that same slice relative to Root note. It works with a note even outside Slice mode; an FX-only cell does not retrigger. Values beyond the current map select its last slice.

Multisample Zones adds individual files with root note, key range, velocity range, tuning and gain. Overlapping zones layer together; with no zones, the main sample supplies the keyboard. Saving collects the zone files. Round robin and zone crossfades are not implemented.

Normalize, Reverse Audio, Trim to Selection and fades write a new audio file. The original library file remains intact. Undo restores the project reference to the preceding file; it does not delete the generated file. Project saving collects the referenced edited file.

Time Stretch opens a duration-ratio and source/target-tempo dialog. Its operation currently stretches the whole main sample and leaves multisample-zone files unchanged. A ratio of `2` means twice as long at the same pitch; changing a 120 BPM loop to 90 BPM needs `120/90`, about `1.333`. Choose Calculate Ratio From Tempos, review the duration, then Create Stretched Sample. The supported ratio is 0.25–4; this offline process does not add realtime warping or formant correction.

### Sample editor

Samplers show a **Sample editor** with a zoomable waveform. Click or Tab to the waveform to edit by keyboard:

| Key | Action |
| --- | --- |
| Up / Down | Choose the marker: Start, End, Loop start, Loop end or a slice |
| Left / Right | Nudge the marker (Shift: coarse, Option: one frame) |
| Home / End | Move the marker to its lower / upper limit |
| `+` / `-` | Zoom in / out |
| `0` | Show the whole sample |
| `F` | Zoom to the Start–End region |
| `,` / `.` | Scroll left / right |
| `Z` | Zero-crossing snap on/off (nudges then land on zero crossings) |
| `Shift+Z` | Snap the selected marker to the nearest zero crossing now |
| Space / Return (hold) | Audition; release to stop. Escape also stops |
| `M` / `Shift+M` | Next / previous play mode |
| `T` | Trim to the Start–End region |
| `I` / `O` | Fade in / fade out (the fade shape menu sets the curve) |
| `N` | Normalize region |
| `R` | Reverse region |
| `G` | Gain… |
| `A` | Add a slice at the selected marker |
| Delete | Remove the selected slice |
| `P` | Slices → new pattern that triggers each slice at its position (assign the instrument to the selected track first) |

Letter keys play notes instead while the computer-keyboard piano (`Cmd+P`) is on. The waveform's context menu offers the same commands plus Remove DC offset and Silence region. Region edits apply to the Start–End region, write a new audio file and never change your library files; `Cmd+Z` restores the previous audio. The play-mode buttons above the waveform also respond to Left/Right.

### Computer-keyboard piano

The computer-keyboard piano is explicitly enabled/disabled by the Keys control (`Cmd+P`). The chromatic lower row is `Z S X D C V G B H N J M`; the upper row is `Q 2 W 3 E R 5 T 6 Y 7 U`. `[` and `]` change octave. Recording arm and piano enable are separate states; `Cmd+R` arms recording and enables the piano. During playback, held-note release records the gate duration. Outside piano mode, plain `R` also toggles record arm. Notes entered in one tracker step share one gate, which ends when the last chord key is released; use separate tracks for independent chord-note lengths.

### Scales, chords and velocity

These settings live in the toolbar performance controls and the Instrument view's **Play & perform** panel. They work in every view, are saved with the song, and are written into patterns exactly as you hear them (a chord becomes a step chord).

| Key | Action |
| --- | --- |
| `Option+K` / `Option+Shift+K` | Next / previous scale |
| `Option+T` / `Option+Shift+T` | Scale root up / down |
| `Option+L` | Switch between **Snap to key** and **Scale degrees** |
| `Option+C` / `Option+Shift+C` | Next / previous chord |
| `Option+V` / `Option+Shift+V` | Cycle keyboard velocity: 127, 110, 100, 80, 60, 40 |

The 15 scales are Chromatic, Major, Minor, Harmonic minor, Melodic minor, Dorian, Phrygian, Lydian, Mixolydian, Locrian, Major pentatonic, Minor pentatonic, Blues, Whole tone and Japanese (In). **Snap to key** moves out-of-key notes into the scale. **Scale degrees** maps one key to one scale note, so every key you press is in key.

Chords come in two kinds. Fixed shapes (Major, Minor, Sus 2, Sus 4, Diminished, Augmented, Major 7, Minor 7, Dominant 7, Add 9, Minor 9, Power fifth, Octaves) always use the same intervals. **Chords in key** (Scale triad, Scale 7th, Scale 9th, Scale sus 4, Scale open 1-5-10) stack notes from the selected scale, so every key plays a chord that fits the key, as on Maschine.

### Chord sets

A chord set turns each key into the next chord. There are 31 sets in four groups:

- **Major harmony** and **Minor harmony** (16 sets of 12 chords): the chords of one key or mode, such as Major triads, Secondary dominants, Mixolydian rock, Harmonic minor, Dorian groove and Minor to major. Each chord has its bass note below and close voicing above.
- **Major style** and **Minor style** (15 sets): progressions for a genre, such as Pop anthem, Neo-soul 9ths, Jazz ii–V–I, Deep house and Ambient lydian.

Projects from earlier builds that used a former factory set (Major or Minor 1–8) open on the harmony set in the same position. The default is Off.

- `Option+G` / `Option+Shift+G`: next / previous chord set. **Off** is part of the cycle.
- `Cmd+K` → **Chord set**: choose a set by name, or **Chord set off**.
- With a set active, `Z S X D C V G B …` play chord 1, 2, 3 … in keyboard order; keys past the last chord repeat the set an octave up. An on-screen chord strip shows each chord's name and key.
- The Key root (`Option+T`) transposes the set; roots above F♯ transpose down so the set stays centred. Scale does not change a chord set.

Every set is recorded through the keyboard handler and compared with its MIDI map in Node tests. Audible and physical-MIDI acceptance are not claimed.

### MIDI keyboards

Scale, chord and chord-set settings also apply to notes from a MIDI keyboard. With a chord set active, MIDI C3 plays chord 1 and the following keys play the next chords. Turn this off with `Cmd+K` → **Apply key / chord / chord set to MIDI keyboard: on / off** (the performance controls also have a **MIDI** checkbox). This has been tested with simulated MIDI input only, not with a real controller.

### Note repeat and arpeggiator

Note repeat retriggers held notes on a tempo-synced clock; the arpeggiator plays held notes one at a time.

| Key | Action |
| --- | --- |
| `Option+N` | Note repeat / arp on or off |
| `Option+Shift+N` | Next mode: Repeat, Arp up, Arp down, Arp up-down, Arp random, Arp as played |
| `Option+-` / `Option+=` | Rate slower / faster: 1/4, 1/8, 1/8T, 1/16, 1/16T, 1/32 |

Note repeat and the arpeggiator are for live playing. They work with chords, scales and chord sets. While recording is armed, keys enter notes normally and repeated hits are heard but not written.

On the Maschine MK3 (in development: turn it on in Settings), hold **NOTE REPEAT** and play pads (a tap latches it; tap again to stop); the display buttons choose the rate. **Shift + NOTE REPEAT** opens the Arp page: the display buttons choose the mode, and the encoders set Mode, Rate, **Octaves** (the arp modes play the held notes again up to three octaves higher) and **Gate** (how long each repeat sounds, 10–100 % of the rate). Octaves and Gate are set on the controller; the toolbar shows the mode and rate.

## Patterns and song

Pattern (`Cmd+1`) displays rows across tracks. Each event has note, instrument, velocity, two effect slots and an optional gate. Like a classic tracker, a note plays until the next note on its track, an OFF (`1`) or a CUT (`Shift+1`), or until a one-shot sample ends; new notes have no gate. The **Gate** toolbar button (or `Cmd+K` → **Show / hide Gate column**) shows the optional **GATE** column, a per-project view setting saved with the session (not an Undo step). With it shown, click a gate value or use **Gate (rows)** in the inspector below the grid: `1` holds one row, `4` holds four rows, and an empty field (shown as `—`) is no gate. With the column hidden the inspector shows the gate read-only. `Cmd+K` → **Remove gates from selected notes** clears gates in the selection. The MK3 Gate knob edits this same value (turned fully down: none). Songs saved before gates became optional keep their gates and sound unchanged. The five keyboard-edit columns (note, instrument, velocity, FX 1, FX 2) keep their existing arrow and Return behavior; Tab reaches the Gate inspector. Arrows move the active cell; Shift-arrows select a rectangle. `Cmd+C/X/V` copies/cuts/pastes, `Cmd+D` fills from the selection, and Option-Up/Down transposes by semitone (Shift adds octaves). Transpose and Return on a note/instrument/velocity column audition the actual step, including velocity, chord and Slice FX, without recording extra notes or moving the cursor. Return on FX 1 or FX 2 opens the Step effect picker. Automated native workflow tests cover browser loading, note/chord entry, kit effects, live track addition, rendered PCM, and save/reopen. Their keyboard events are synthetic; they do not establish physical-keyboard or listening acceptance. Change pattern length and edit step from the toolbar; duplicate a pattern to make an independent variation. Live recording uses the current row grid; while the Gate column is shown it also stores held-note lengths as gates (otherwise recorded notes have no gate). This is not a complete quantization/groove editor.

### Left pane, pattern matrix and song scroll

The Pattern view's left pane holds the **instrument list** on top and the **pattern matrix** below it (like Renoise's instrument box and pattern matrix). The matrix has two tabs: **All patterns** (the session's pattern list) and **Current song** (the song's blocks in order, in the Song view's colours and with their repeat counts). In the list, Up/Down choose a pattern or block (the grid follows), `Option+Up/Down` move it, `Cmd+D` duplicates, `Delete` removes, `N` adds a new pattern, Return goes to the grid and Escape back to the tabs. Current song also takes the arranger's keys: `I` insert, `Shift+Cmd+D` clone, `-`/`+` repeats, `L` loop, `P` play from here. `Shift+Return` in All patterns adds the pattern to the song after the current block. Song edits are the arranger's own edits: one Undo step each.

**Song scroll** (toolbar, `Option+Shift+F`) makes the grid run through the song's patterns in song order as one list, each slot under a header row. The current slot is editable; the other slots are shown read-only. Click one to edit it, or move the cursor past a pattern's first or last row to continue in the neighbouring slot; `Cmd+[`/`Cmd+]` step a slot. With Follow on, playback in Song mode moves the editable slot along. Very long songs show the slots around the current one (about 1,500 rows) with a note for the rest. Song scroll is a view setting saved with the session.

**Automation panel** (toolbar, `Option+Shift+A`) shows one parameter's lane under the grid with a faint line per row, stronger on beats and bars. Rows are linked: hovering a row in either place lights the other in pale yellow, and the grid cursor row is drawn in the panel. **Pattern** shows the current pattern; **Song** lays the song out horizontally with the slots' pattern names, from start to end. Drag adds or moves a point, `Shift`-drag draws a straight line and `Cmd`-drag draws freehand. Both may cross pattern boundaries: each slot's part is written into that slot's pattern lane in one Undo step. A repeated pattern changes in every repeat, and a message says so. `Option`-click removes a point. With the lane focused, Left/Right move the shared cursor row (`Shift` by a beat), Up/Down change the value there, Return adds a point, Delete removes it, and Escape or `Option+Shift+A` returns to the grid.

### Managing tracks: hide, collapse, reorder, delete

These work on every track (sample, plug-in, MIDI, line input) and on groups and kits.

- **Hide empty tracks** (toolbar **Hide empty**, `Option+U`, or `Cmd+K` → **Hide / show empty tracks**) hides tracks without steps in the current pattern. The selector next to the button switches to **whole song** (a track is empty when no pattern uses it). The button shows how many tracks are hidden. Inside a group, empty members hide and the group header shows **+N hidden**; click it to show them (and **hide N** to hide them again). An expanded group always keeps one column. A hidden track comes back as soon as it gets a note (typing, recording, Pad keys, paste, MK3 or an agent). The cursor track is never hidden, and arrows, Tab and `Cmd+Shift+Left/Right` step over hidden tracks. It is a per-project view setting saved with the session, not an Undo step.
- **Collapse a track** to a slim column showing only its note names: the ◂ button on its header, a double-click on the header (double-click the name itself to rename), `Option+Shift+J`, or the header's context menu. Expand with ▸, a double-click or `Option+Shift+J`; `Cmd+K` → **Expand all collapsed tracks** opens them all. Notes can still be entered in the note column. Saved with the session, not an Undo step.
- **Reorder** by dragging a track header left or right; a line shows where it lands. Dropping between members of an expanded group adds the track to the group (the line says "into …"), dropping outside takes it out. Drag a group header (or a collapsed group's column) to move the whole group. With several tracks selected, dragging one of them moves them all. Mixer strips drag the same way by their name; a group strip's right half drops into the group. Dragging near the edge scrolls; `Esc` cancels. Each move is one Undo step; notes, instrument, inserts, routing, sends and automation stay with the track. `Cmd+Shift+Left/Right` still moves the cursor track one step.
- **Delete** with the × on a track header or mixer strip (shown on hover and on the selected track), the context menu, `Cmd+Backspace`, or `Cmd+K` → **Delete selected tracks…**. An empty track is deleted at once; a track with content first asks, with a summary such as "Kick has 24 notes in 3 patterns and 2 automation lanes". With several tracks selected they are deleted together with one confirmation. Deleting a group (× on its header, `Cmd+K` → **Delete group…**, or `Cmd+Backspace` on a collapsed group or a group strip) asks whether to **Ungroup, keep tracks** or **Delete group and tracks**. Deleting removes the tracks' plug-ins, routing, sends and automation; their instruments stay in the instrument list. One `Cmd+Z` restores everything.

### Note columns (several notes per row)

A track can play up to eight notes on the same row, like note columns in Renoise. This works on every track type (sampler, plug-in, external MIDI). Click **+** on a track header (shown on hover and on the selected track), use the header's context menu, or `Cmd+K` → **Add note column to selected track**; **−** or **Remove note column from selected track…** removes the last one, asking first when it holds notes. Each change is one Undo step. There are no default shortcuts for these.

- **Layout.** Each extra note column shows its own NOTE, INS and VEL (labelled N2, N3 …) after the first, then FX 1 and FX 2 for the whole row, then a GATE per note column when the Gate column is shown.
- **Each column is its own voice.** A new note in a column releases only that column's earlier note; an OFF (`1`) or CUT (`Shift+1`) in a column ends only that column's notes; each column has its own gate. Notes in different columns sound together, on the track's one mixer channel.
- **Effects belong to the row.** FX 1 / FX 2 sit in the first note column and apply to every note on the row: channel effects (volume, pan, cutoff …) as always, and note effects (ratchet, probability with one roll for the whole row, microtiming, offset, reverse, glide, arpeggio, slice, pitch) on each column's note. A ratchet that keeps repeating on the empty rows after it repeats only the first column's note.
- **Editing.** Left/Right walk NOTE, INS, VEL of every note column, then FX 1 and FX 2. Typing a note, OFF/CUT, instrument digits and velocity digits work in any note column, and a typed note is previewed with the row's effects. Return auditions the note under the cursor. Without a selection, Delete clears just the note column under the cursor (in the first column: its note and the row's effects; the other columns stay) and `Option+Up/Down` transposes just that column. A selection still covers whole tracks: it copies, cuts, pastes, clears and transposes every note column. Pasting or dragging steps with more note columns onto a track adds the columns it needs, in the same Undo step. The step editor below the grid edits the cursor's note column (its heading shows **N2**, **N3** …); its FX fields always edit the row.
- **Recording.** While notes are held, each new recorded note takes the next note column that is not held (starting at the cursor's column), so chords and overlapping notes from a MIDI keyboard or the computer keyboard sound together. With the transport stopped, held notes share the cursor row. Recording never adds columns: with every column held, a further note is entered as on a one-column track. A chord from the Chord setting stays a chord in the first column. Live recording with the Gate column shown measures each column's held length separately.
- **Elsewhere.** MIDI export writes each column as its own voice, and MIDI import puts overlapping notes on one track into note columns (up to eight per track; a different microtiming goes to another track). Collapsed (slim) tracks and collapsed groups show the row's first note with a `+` when there are more. The MK3 shows the first note column only and keeps the other columns when it edits a step. The agent API (`write_steps` with `column`, `read_pattern`) reads and writes extra columns.

### Typing values (INS, VEL, FX)

Only the NOTE column plays and enters notes. In the other columns keys type values, tracker style, and never sound a note (with the keyboard piano on or off, armed or not):

- **INS**: the instrument number, two digits (`03`). With fewer than ten instruments one digit is enough. A number without an instrument is refused with a hint.
- **VEL**: velocity 1–127. It commits as soon as no further digit could fit (`90`, `127`), or on Enter/Tab/an arrow key.
- **FX 1 / FX 2**: an effect code letter, then its value in the effect's own units; the cell shows the same text. Examples: `R04` ratchet ×4, `Y50` probability 50%, `P-30` pan 30% left, `T-12` pitch down an octave, `C2400` cutoff 2400 Hz, `A37` arpeggio +3 +7, `B1` reverse. `Delete` clears the effect under the cursor; `Escape` abandons a half-typed value; Enter on an FX cell without typing opens the effect picker.

Each typed value is one Undo step, and the cursor then moves on by the edit step. While a step-editor box (Note, Ins, Vel, Gate, FX) has focus, keys type into the box and never play notes; Enter commits and Escape cancels, both returning to the grid. The step editor's **Ins** box takes a number or the start of an instrument name (with suggestions), and each FX has a code box that takes `R4`, `Y50`, or a name such as `rat 4` or `rev`; an empty code box removes the effect. The **Choose effect…** picker remains and lists each code.

| Code | Effect | Value |
|---|---|---|
| A | Arpeggio | two hex digits: +x +y semitones (37 minor, 47 major) |
| B | Reverse (backwards) | 1 backwards, 0 forwards |
| C | Filter cutoff | 20–20000 Hz |
| D | Delay send | 0–100% |
| E | Filter resonance (emphasis) | 0–95% |
| G | Pitch glide | −24…24 semitones |
| H | Reverb send (hall) | 0–100% |
| L | Volume (level) | 0–100% |
| O | Sample offset | 0–99% |
| P | Pan | −100 left … 100 right |
| Q | Microtiming (delay the note) | 0–95% of the row |
| R | Ratchet (retrigger) | 1–8 |
| S | Sample slice | 1–128 |
| T | Pitch (transpose) | −24…24 semitones |
| W | Wavetable position | 0–100% |
| X | Plug-in parameter lock | 0–100% of a lock already on the step (choose the parameter with the picker) |
| Y | Probability (maybe) | 0–100% |

The letters follow Renoise where it has the same command (A, B, G, R, Y, Q, P, L). `Cmd+K` → **Effect codes reference** and the shortcut sheet list them.

To duplicate a whole track, select it, press `Cmd+K`, search **Duplicate selected track**, and press Return. The copy is appended and selected. It includes notes from every pattern, audio clips, automation and independent instrument/effect state; shared sample files remain intact. `Cmd+Z` undoes the complete operation. It requires a free track and enough instrument slots for independent copies. See [Track duplication](TRACK_DUPLICATION.md) for routing, hardware MIDI and failure behavior.

### Pattern loop range

Press `Option+Shift+L` in the grid to loop the selected rows during Pattern playback; with no selection it loops the bar under the cursor. The **⟲ Loop** button above the grid shows the range. Press `Option+Shift+L` again to stop looping; playback then continues through the whole pattern. If transport is stopped and Song mode is selected, the loop switches playback to Pattern mode; during Song playback the loop waits for Pattern playback. The loop range is a working aid: it is not saved with the project and clears when you switch patterns. (In the Song view the same keys loop the selected blocks; see below.)

### Mute, solo and pattern tools

With the tracker grid focused:

| Key | Action |
| --- | --- |
| `Option+M` / `Option+S` | Mute / solo the cursor's track |
| `Option+Shift+M` | Unmute all tracks and clear solo |
| `Option+F` | Follow playback on/off |
| `{` / `}` | Edit step down / up |
| `Option+I` | Interpolate: ramp velocities between the first and last note, or ramp an FX value between matching effects at the start and end (FX column) |
| `Option+H` / `Option+Shift+H` | Humanize velocity (small random changes) / randomize velocity |
| `Option+.` / `Option+,` | Velocity louder / softer by 10% |
| `Option+R` | Reverse the steps |
| `Cmd+Up` / `Cmd+Down` | Nudge steps up / down (rotates within the range) |
| `Option+E` / `Option+Shift+E` | Stretch ×2 (half speed) / compress ½ (double speed) |
| `Option+D` / `Option+Shift+D` | Double the pattern length (repeating its content) / halve it |
| `Option+X` | Clear step effects |
| `Option+P` | Automate parameter… (see below) |

Tools act on the selection, or on the cursor track's whole pattern when nothing is selected. Each tool is one Undo step. Right-click the grid (or `Shift+F10`) for the same tools plus **quick step effects**: probability 75/50/25%, ratchet ×2/×3/×4, microtiming late 25/50%, reverse playback, sample offset 25/50%, glide from −12 and minor/major arpeggios. Quick effects apply to every note in the range, use a free FX column, and skip notes whose two FX columns are already in use. `Cmd+K` → "Step effect" finds them too.

### Automate parameter picker

`Option+P` in the grid (or right-click → Automate parameter…) opens a searchable list of every automatable parameter on the selected track, including plug-in parameters. Parameters that already have a lane in this pattern are listed first under **Already automated**. Type to filter, Up/Down to choose and Return to create or open the lane. Plug-in parameter lists load in the background and appear when ready.

### Step effect picker

The **Step effect** picker also opens from the FX buttons below the tracker. Search built-in effects or exposed AU/VST3/CLAP parameters by effect, plug-in, parameter or mapped NKS name. Up/Down/Home/End browses results; Return chooses one and focuses its value. Enter the value and press Return or Apply. The Column control selects FX 1 or FX 2; Remove effect clears only that slot. Escape/Cancel leaves the step unchanged. Effects already used on the track rank first. Opening or searching the picker does not change the live parameter; Apply checks that the project, row and plug-in target still match.

Applying a parameter effect to an empty row leaves the note as dots; it changes a parameter without playing a new note. Adding a note later preserves its effects, and OFF/CUT rows can carry locks while releasing/cutting their note. Slice and probability require a note on the row. Ratchet can repeat the last note or chord through empty rows. Reverse and sample offset can change currently playing sample voices on an effect-only row; they apply to the addressed track.

Plug-in effect values are normalized from 0 to 1. A lock overrides the matching automation lane for that row; the lane resumes on the next row even if its value is unchanged. Without a lane, the parameter returns on the next row to the value you last set yourself. If both FX columns target the same plug-in parameter, FX 2 wins. Locks follow the exact saved instance through insert reordering. Missing/replaced instances or locks pasted to another track remain saved but inactive; reopen the picker to select an appropriate destination. The picker addresses instruments/inserts on the selected tracker track; group/return/Master lanes remain separate.

### Song arranger

Song (`Cmd+6`) arranges references to patterns as a list of blocks. A block is one pattern played ×N times; consecutive slots of the same pattern always form one block, so the saved sequence is still a plain ordered list of patterns. Each block shows its position, pattern number and colour, name, an activity preview, repeat count, length and starting bar. With the sequence list focused:

- Up/Down (Home/End, Page Up/Down) select a block; Shift extends a range.
- Left/Right step the block's pattern; typing digits picks a pattern number (for example `1` `2` for pattern 12).
- `-` / `+` change the repeat count (1–64). Option-Up/Down move the selected blocks.
- `I` (or Insert) inserts the next pattern after the block; `N` inserts a new empty pattern; `Cmd+D` duplicates the selected blocks; `Shift+Cmd+D` clones the pattern as an independent variation; Delete removes the selected blocks.
- `L` (or `Option+Shift+L`) loops the selected blocks in Song playback and turns Loop song on; press it again to clear the section and restore the previous Loop song setting. Export always renders the whole song.
- `P` or Shift+Return plays the song from the block (while Song playback runs it jumps there immediately). Return opens the pattern for editing. `E` (or F2) renames the pattern; `C` / Shift+C cycle its colour; `F` toggles follow, which moves the selection with the playhead and shows repeat progress such as 2/4.

Letter and digit commands yield to the computer-keyboard piano while it is on. The toolbar above the list offers the same actions by pointer, and right-click (or Shift+F10) on a block opens them as a menu. Repeated references share the same pattern: clone the pattern when the musical content should become independent.

Song settings provide lines per beat, swing, looping and a project meter/metronome (`Cmd+Shift+M` toggles the metronome). A project has one tempo and one meter for its whole length.

The Audio Arrangement below the sequence plays audio clips alongside patterns in Song mode. Import Audio inserts a file at the selected sequence position, or set the numeric edit cursor and use Import At Cursor. Select clips with Up/Down in the timeline. Left/Right nudges start, Shift-Left/Right trims the end, Option-Left/Right slips the source, and Option-Up/Down changes track. `Cmd+D` duplicates after the clip; `S` splits at the edit cursor; Delete removes the clip reference. When a clip remains, Delete focuses the fallback clip and selects its track, so the next import uses that track. Deleting the last clip focuses the empty timeline and retains the current valid track. Numeric fields expose start/length in beats, source offset/fades in seconds and gain. Source files stay unchanged. Clips play at their original speed; changing project tempo changes beat placement and is not automatic audio stretching.

**Render stem…** captures the selected track after its inserts, fader and pan, before its parent bus. Shared returns and downstream group/Master processing are excluded. It adds a **muted** track with neutral gain/pan, no sends and direct output to Master; the original track stays unchanged. Audition the new stem in the mixer before deciding how to use it.

The render allows a tail, requires a free track and makes one undoable arrangement change. It is not a linked freeze (see Track Freeze). Take comping is available separately through audio-clip alternatives. Song exports include timeline clips; pattern-only exports exclude them.

### Track freeze

**Freeze Track…** (track menu, command palette) prints the track's instrument and inserts to audio and releases its plug-ins to save CPU; **Unfreeze Track…** restores the live chain with its exact plug-in state. The print covers the song and each pattern the track plays (first pass and looping pass), so the frozen track sounds the same in Song and Pattern mode, including tails over a loop or a pattern change. Fader, pan, mute, solo, sends and mix automation stay live. With **Auto tail** (default) each print ends where it falls silent, up to the longest tail you set; turn it off for a fixed tail. Delay compensation follows automatically: a frozen track no longer delays the rest of the mix.

Tracks that play external hardware (a line input returning a synth, or an external MIDI instrument) cannot be rendered offline. Their dialog offers **Record and freeze**: the song (and optionally each pattern) plays once in real time and the return is recorded in time with the mix. While frozen the synth gets no MIDI from that track and its line input is closed, so the synth is free for another part. Details: `docs/TRACK_FREEZE_UX.md`.

### Audio takes and comp regions

Select a Song clip, then Import take or Record take. Each alternative must cover the region; the recorder keeps short or unattached captures available for retry. Page Up/Down changes the active take; T opens a searchable manager with rename, preview and source offset. Split at the edit cursor with S, then choose a take independently for each region. Undo/Redo preserves the alternatives; portable Save collects inactive sources too. Alignment and fades are manual. Punch ranges and automatic loop-record splitting remain outstanding. See [audio takes](AUDIO_TAKES.md).

## Mixer and automation

Mixer (`Cmd+4`) exposes track, group, return and Master controls. **Left/Right moves to the matching control on the adjacent channel**; Tab moves among controls on the current channel. Up/Down adjusts a focused fader. Selecting a group, return or Master keeps the tracker’s note-entry track unchanged. On a supported channel control, `Option+A` opens automation for that channel, including buses and Master.

**Send effects** (`Cmd+Shift+X`, `Cmd+K` → *Send effects: delay & reverb…*, or right-click a mixer channel → *Delay & reverb returns…*) edits the shared delay and reverb in one keyboard dialog. Up/Down choose a parameter, Left/Right adjust it (Shift coarse, Option fine), Return or a digit types an exact value (milliseconds, a division such as `1/8.`, or a percentage), `0` resets, and Tab moves between the Delay return, Reverb return, Delay sends and Reverb sends columns. Delay: Timing (tempo sync or free), Time (divisions from 1/32 to 1/1 including dotted and triplet, or milliseconds), Feedback, Tone (feedback low-pass, Open to 200 Hz) and Return level. Reverb: Size, Damping, Pre-delay (0–250 ms) and Return level. A synced delay follows tempo changes. Edits are saved with the project, and each burst of arrow presses is one Undo step.

Every song is mixed the same way: track faders come after the track's inserts, and Master gain after the whole routing graph. In the selected channel's **Routing** panel, add a named group/return (also **+ Group** and **+ Return** above the strips), choose a searchable Output destination, or add a pre/post-fader send. **No output** disconnects the channel's main path while allowing it to feed a send or sidechain.

An insert's **Sidechain inputs** section offers the auxiliary buses the plug-in actually exposes. Choose an input, source, tap and gain. Cycles are rejected. Missing plug-ins or changed input layouts keep their saved connection inactive until the matching input returns or you remove it. If a saved project has invalid routing, the project and plug-in state remain available to save, audio pauses, and the status points to Mixer for repair. Multi-output instruments are not supported yet.

In an insert's header, `Option+Up/Down` moves it earlier/later; Move up/down buttons do the same. A move preserves the live instance, bypass and automation targets and forms one undo step. Native identity/state/rollback and UI tests pass; complete physical keyboard/vendor-editor acceptance remains open. Automation (`Cmd+5`) searches targets with existing lanes first. Each lane belongs to a pattern and a track, bus or Master and contains row/value points with hold or linear interpolation. Every lane of the pattern is shown as a box in a scrolling stack, and the selected lane is edited in the envelope editor below it. In the stack, Up/Down choose a lane and Return enters its envelope. Choosing a parameter in the target list selects its lane and scrolls the stack to it, while focus stays in the list; Return also enters the envelope.

The envelope editor has a numeric point table and keyboard editing. With the envelope focused, Return adds a point, Option-Left/Right selects a point, Left/Right moves its row, Up/Down adjusts value and Delete removes it. Shift uses larger changes; Home/End select the first/last point. A pale playback line and row readout follow fractional playback through the whole pattern; the line hides when stopped or when another pattern plays, with a text explanation. Playback preserves the selected point, focus and scroll position. Hidden native-interface tests verify both target window sizes; physical QA also verified envelope editing, fractional row feedback and a moving playhead. Complete keyboard/accessibility acceptance remains open.

Focus a supported parameter and press `Option+A`, or use its context menu, to create/select its lane. The envelope is focused and revealed immediately: Up/Down edits the selected point’s value, and Option-Left/Right selects another point. Opening an existing lane is navigation only: it adds no Undo step, does not dirty the project and does not republish it to the audio engine. Creating a lane and editing a point remain undoable changes; a rejected target keeps focus on the invoking control. The handoff and clip-selection fixture passed 45 synthetic-event WebKit checks across both target sizes, including a synthetic generic plug-in parameter dialog; it does not establish physical-keyboard or audible automation acceptance. The target list shows existing automation first and can be searched; unavailable targets explain why they do not apply to the current instrument. Native sampler controls do not change a third-party plug-in's internal sound.

Step FX provide event-level changes beside notes. Native volume/pan/filter/resonance/send/pitch row locks take precedence until the next row, when a lane resumes. Plug-in step effects use the exact instance and parameter address, with the hold/precedence behavior described above; plug-in lanes retain their owner/slot/address targets. Stepped AU/mix automation is scheduled at its event frame; linear AU lanes update every 32 audio frames (about 0.67 ms at 48 kHz), while native DSP lanes evaluate per sample. Removing a lane restores its configured base value through timed engine defaults. Arbitrary assignable macros are not implemented.

### Live automation recording

You can record automation by moving controls while the song plays. The status bar chip shows the current mode (**Auto Read**, **Auto Touch**, and so on) and how many lanes are being written.

| Key | Action |
| --- | --- |
| `Option+W` | Switch between Read and the last write mode |
| `Option+Shift+W` | Cycle the write mode: Touch → Latch → Write |
| Right-click a control | Arm / disarm it for automation write, arm all parameters on its channel, choose the write mode, or Learn MIDI controller… |

- **Read**: lanes play back; moving a control edits its base value.
- **Touch**: while playing, moving a control writes automation until you let go, then the existing lane (or base value) resumes.
- **Latch**: while playing, moving a control writes automation and holds its last value until playback stops.
- **Write**: while playing, armed parameters are overwritten from the start of playback; any control you touch latches until stop.

With nothing armed, Touch and Latch record any automatable control you move. Arming one or more parameters limits recording to those. Recording accepts keyboard arrows, pointer drags, a learned MIDI CC and the eight-knob NKS/plug-in pages. Writing happens at the playhead into the playing pattern's lane for that track, bus or Master. The project is not changed during a pass: you hear the working lanes, and stopping playback (or switching to Read) commits every lane written in that pass as one Undo step. Recorded movements are thinned to keep lanes editable.

**Learn MIDI controller…** waits for the next MIDI CC (Escape cancels); that CC then controls the parameter, recording automation when a write mode applies and moving the base value otherwise. **Forget MIDI controller** in the same menu removes it. Sustain (CC 64) and channel-mode messages cannot be learned. `Cmd+K` → **Disarm all automation write parameters** clears all arms. Arms are cleared when another project opens. Live recording is verified by UI tests; it has not yet been checked by ear with live audio or with a real MIDI controller.

Plug-in delay compensation aligns native samples, AU instruments, insert chains and fixed return paths using each AU's declared algorithmic latency. Audio Settings shows the compensation amount. The capacity is two seconds; exports reject chains exceeding it. Creative delay echoes and reverb tails remain part of the sound. Latency is checked every 0.5 seconds and after explicit plug-in operations; delay changes crossfade over 256 samples. Changing latency during playback may cause brief transitions; exact tests use settled chains/clean exports. Stop playback before changing a latency-affecting plug-in mode when exact alignment matters. Incorrectly reported latency and physical recording roundtrip alignment remain unverified.

## Plug-ins and NKS

Browser → Plug-ins lists instruments and effects with All, AU, VST3 and CLAP format filters. Plug-ins are scanned in the background after launch, in separate processes, so a plug-in that fails to scan cannot stop the app from starting; the browser shows **Scanning plug-ins · n of m** while this runs, and **Refresh plug-ins** rescans. Choose Instruments or Audio Effects, search by name/manufacturer, select a destination and press Return to load. Instruments play on the selected track; effects occupy the selected channel's insert slots. Open the vendor editor for its full interface or use the parameter controls for keyboard-accessible editing. Third-party editor accessibility depends on its vendor.

### Plug-ins run outside the app

Every plug-in runs in a separate container process (one per plug-in by default). If a plug-in crashes or stops responding, its track falls silent or passes audio through, the rest of the song keeps playing, and the plug-in review offers **Retry** to start the plug-in again with its saved state. A plug-in that crashes twice in a row while loading is held back at the next start until you retry it.

### NKS browser

`Cmd+Shift+B` (or `Cmd+K` → *Browse NKS instruments* / *Browse NKS effects*) opens the **NKS** tab of the Browser, a Maschine-style browser over every installed NKS preset. Choose **Instruments** or **Effects**, then narrow by **Product → Bank → Type / Sub-type → Character**, or search by text.

| Key | Action |
| --- | --- |
| `Cmd+F` | Search |
| Left / Right | Move between products, presets and the detail panel |
| Up / Down, Page Up / Page Down, Home / End | Browse the focused list |
| Typing (in Products) | Jump to a product by name |
| Tab | Move into the filter chips; Left/Right within a group, Up/Down between groups, Space or Return toggles |
| Space | Preview / stop the preset's audio preview |
| Return | Load and keep browsing |
| `Shift+Return` | Load and edit controls (opens Instrument) |
| `Option+Return` | Load as a new instrument |
| `F` | Favorite |
| Escape | Stop the preview; in search, clear the text; in Products, clear the product filter |

Instrument presets replace the instrument on the selected track (or create a new one with `Option+Return`). Effect presets (`.nksfx`) load into an insert slot of the selected track; choose the slot in the detail panel's **Insert slot** menu. The track's instrument is not changed.

### How NKS presets load

NKS metadata, tags, banks and previews are separate from preset compatibility. Direct loading reads the preset's exact plug-in identity and saved state. Presets made for a VST3 load into that VST3. Presets made for a VST2 load into the vendor's VST2-compatible VST3 release if one is installed, which accepts the VST2 state and parameter IDs (VST2 plug-ins themselves are not hosted). Return/Load requests native loading and Option-Return requests a new instrument; Inspect remains available separately. Mappings bind only when the class identity and actual exposed parameter IDs agree. A preset's controller IDs are never assumed to be AU addresses.

### Plug-in control pages

After a validated NKS load, Instrument offers eight named controls per NKS page. Save retains the selected page and knob, including repeated saves while another view is open; opening another project or replacing the plug-in resets that view state. Left/Right selects a knob; Up/Down adjusts it (Shift: 10%, Option: fine); `Shift+Left/Right` or `Ctrl+Page Up/Down` changes pages; digits `1`–`8` jump to a knob while the keyboard piano is off; `A` automates the focused knob. `Option+A` or the parameter context action finishes a pending edit and creates/selects its lane at the selected pattern row. Existing lanes rank first; search accepts mapped names, page names, aliases and the plug-in's own parameter names. **Use on step…** opens the picker for that exact parameter; choose the FX column/value and Apply. Opening it creates no note and changes no live value. Cancel preserves the step, Undo history and prior focus. Unassigned, read-only, non-automatable or unavailable controls cannot be used for these actions.

Every other plug-in on the selected track (AU, VST3 or CLAP instruments without NKS pages, and every insert effect) gets the same eight-knob console with pages generated from its parameter list. NKS effects use their own NKS pages. Tabs across the top select the plug-in: Left/Right on the tabs, or `Option+Left/Right` from anywhere in the console. On a knob, Up/Down adjusts by 1% (Shift 10%, Option fine), `Shift+Left/Right` changes page, `1`–`8` jump to a knob and `A` automates it. On generated pages you can also make your own: Return assigns a parameter to the focused knob, `/` finds a parameter, `P` pins the knob's parameter to your page, Delete clears the knob, and **New page** / **Rename** manage user pages. User pages are saved with the project.

Save and Undo wait for pending mapped edits, including a gesture made just before leaving Instrument. Undo removes the completed gesture and Redo restores it. Failed writes/captures retain a retryable edit; replacing its plug-in cancels the old target with an explanation. New input cannot be overwritten by an older capture. If you continue editing during Save, another save may be needed. Previews and dependent project changes wait for mapped edits and plug-in restoration before using the new graph.

No installed NKS product has been loaded through the 0.2 host yet; the loader is tested with generated presets and test plug-ins.

The installed **Komplete Kontrol AU** is a possible route to authentic NKS sounds: load it and use its own preset browser. Its compatible instruments and licensing remain inside that host. This route has not passed the application's playback/mapping/save acceptance tests, and SuperTracker does not claim to open a selected NKS catalog entry directly in Komplete Kontrol. This fallback also does not establish direct NKS loading or mapped control support. Komplete Kontrol AU does not provide MIDI output for capturing its generated arpeggiator notes. See [NI’s MIDI-control manual](https://docs.native-instruments.com/online-guides/komplete-kontrol-manual/en/automation-and-midi-control.html). Native tests cover Apple DLSMusicDevice, AUDelay wet/bypass/time changes, effect-order differences, AU send bounces and basic state save/reopen; this does not establish compatibility with every installed AU.

After an unclean session, Plug-in recovery can keep suspect plug-ins disabled while preserving their saved settings. A suspect is an association with an interrupted operation, not proof of the crash's cause. Select individual entries with arrows and Space, then choose Retry selected (`Cmd+Return`), or Keep disabled. The command palette can reopen recovery. Native and hidden-WebKit fixtures verify exact-identity gating and explicit retry; this does not isolate a running plug-in crash.

## Recording, saving and export

Open the **Recording** main tab (`Cmd+7`) for input routing and sample capture. The capture form is embedded directly in the tab: choose **Audio interface input** or **Master output / resample**, device, mono/stereo channels, name, root note and monitoring, then **Start recording**. Meters and the pinned record/stop action remain in the workspace without a popup. **Stop & load** creates a new sampler instrument with a new ordinary tracker lane by default, preserving all existing instruments, kit tracks and live-input lanes—even when opened from Song or a kit.

To overwrite audio, explicitly choose the named **Replace instrument** destination; for a kit pad, put the cursor on that pad's track first. Song clips remain an explicit recorder destination, and Record take retains its selected-clip target. Failed attachment keeps the captured file available for retry.

**Configure line inputs…** on the Recording tab assigns separate mono/stereo interface channels to multiple mixer tracks. Live lanes monitor through their track effects as soon as the project opens, without pressing Play; recording them defaults to a separate sampler lane. Each live input has a **Monitor** switch (on by default), also in the mixer's channel panel under **Live input**: turn it off when you listen to the synth through the interface's direct monitoring instead, and the route stays set up.

Studio Settings offers audio input/output, buffer size, monitoring and MIDI routing. Native recording supports input or master-output capture, with a sample or timeline-clip destination. Microphone permission is requested for input capture/monitoring. These hardware recording/device workflows have not yet been tested on an audio interface. External MIDI hardware must have its audio return recorded before an offline export or bounce can include it.

Use File → Export Audio (`Cmd+Shift+E`) for song, active-pattern or stem WAVs with a chosen tail. For stems, choose any of the track, group, return and Master channels. They are captured together through the complete graph, preserving sidechain behavior and common timing. A channel stem is its post-fader output before its parent; it does not include downstream processing or other returns. Select Master for the complete mix. Stems need not sum to Master when downstream processing is nonlinear.

Choose **PCM 16-bit**, **PCM 24-bit** (default), or **32-bit float** in the WAV encoding field. Choose **44.1, 48** (default), or **96 kHz** in Sample rate. **Dither** offers TPDF (default) or None for the two PCM encodings; a 32-bit float file is written as rendered. Song, pattern and stem exports are stereo; internal track bounces always remain 32-bit float at 48 kHz. The range and tail fields remain usable while channel choices load; select arrows retain normal keyboard behavior. Export removes compensation pre-roll and renders delayed frames to retain the requested duration/tail. The complete mix or stem set is published only after every file is verified and transport is restored. Cancel or failure preserves existing output files; stems use a new or empty folder. Large exports beyond the supported WAV size are rejected before rendering. Native tests verify aligned selected stems at all three rates, sidechains, nonlinear Master preservation, clip placement and insert/send processing. General multi-output instruments remain unsupported.

File → Import MIDI File creates a project from supported Standard MIDI Files; Export MIDI File writes the current arrangement. Import uses audible placeholder synths, so assign intended sounds afterward. Changing tempos are converted to one project tempo while preserving elapsed event timing; unsupported expression/state events produce warnings. See [MIDI_FILES.md](MIDI_FILES.md) for exact behavior. Current-source CoreMIDI input/output passed a virtual-endpoint test; physical-controller playing, hardware timing, clock synchronization and the complete MIDI-recording workflow remain unverified.

Save before moving media or changing plug-in installations, and keep the collected `.media` folder beside the project. Samples, zones and clips use collected relative paths on disk. A project can have any number of tracks and additional buses (1024 each as a sanity bound), 64 instruments, 256 patterns of 256 rows, 256 clips, 128 zones/slices per instrument, sixteen inserts per channel and 128 combined custom sends/sidechain connections. Audio is decoded into RAM; an individual source is limited to 100 million frames.

Relink Missing Project Media in the command palette opens the review UI. Inspect missing references, search selected folders or specify old/new root mappings, review a replacement for each file, then apply one undoable project change. A filename match is not a content match: verified hashes are labeled, and a saved-content mismatch requires an explicit choice. The native Bridge and asynchronous service are implemented; filesystem work runs away from the main thread and repeated identical searches share one operation. Backend/service/UI tests pass. An actual hidden-WebKit test also verifies this dialog, relink/Undo/Redo and real bridge Save/Open with collected media and unchanged source audio. The visible keyboard/dialog workflow still awaits acceptance.

For complete, evidence-based current limits, see [FEATURE_STATUS.md](FEATURE_STATUS.md). Missing broad DAW capabilities remain active work rather than implied support.

### Adding sounds without manual assignment

**Add instrument** creates and selects an instrument with its own linked track.
In the sample browser, **Return**, double-click, or **Add sample on new track** adds
a sample and selects its new track. **Replace selected instrument** / **Option-Return**
explicitly replaces the selected sound; to load onto a kit pad, put the cursor on that
pad's track first. Selecting an instrument in the sidebar selects its existing track, so audition
and pattern entry use its destination. Additions have one Undo and preserve existing
tracks, notes and plug-ins. At the 1024-track sanity bound, use explicit replacement or remove a
track rather than creating an unassigned instrument.

### Note endings, ratchets and reverse

A note without a gate sounds until the next note on its track (which sends it a
note-off: it fades with the instrument's release envelope, like a classic tracker),
an OFF or a CUT, the end of a one-shot sample, or Stop. Notes of one chord on one
row leave each other sounding. A note with a gate also ends after its gate.

While live recording without the Gate column, releasing a key records an OFF on the row nearest the release (at least the next row), in the note's own column. No OFF is written when your next note in that column already starts by then, since it ends the held note anyway. With the Gate column shown, the held length becomes the note's gate instead.

In the pattern note column, press **1** for OFF or **Shift-1**
for CUT, or use the **OFF** / **CUT** buttons below the grid or `Cmd+K` →
**Insert note off** / **Insert note cut**. The step Note field also accepts **-1**
and **-2**. OFF releases voices using their release envelope; CUT ends built-in
voices immediately (with a short declick). Both cancel that lane's pending repeats.
External MIDI and plug-ins receive note-off messages; the receiving instrument
controls its release tail. On the MK3, hold SELECT on Home for **Note off**
(Shift: **Note cut**), or press Shift+NOTES.

Ratchets trigger notes using the instrument's Voice mode (Instrument → Voice &
Record). Samples you load and kit sounds start Monophonic: a new note cuts the
previous one, so a looping break or a retriggered hit never overlaps its own tail.
A chord (different pitches starting together) stays whole, and on a track with
several note columns a note cuts only its own column. Polyphonic can overlap notes
and release tails. Monophonic restarts the voice each hit; Legato may reuse a held voice without restarting playback. A
ratchet continues through empty rows only: any new note on the track stops it (that
note plays once unless it has its own Ratchet), and another effect row, OFF or CUT
also stops it; rate 1 also disables repeating. Each track keeps its own ratchet, so
kit pads (tracks of a group) behave the same per pad. Each repeat of a note without
a gate lasts until the next repeat (the row divided by the ratchet count). Mono affects ordinary notes too, so it also removes chord
polyphony.

Reverse 1 means backward and Reverse 0 means forward. On a note row it sets the
direction of newly triggered samples. On an effect-only row it reverses all active
sample voices from that track, including their release tails, at their
current positions. It does not reverse unrelated tracks, live audition voices, or
a hosted plug-in's internal audio.

### Kits and track groups

A kit is a group of ordinary tracks (one per loaded pad) with its own group bus. See [Kits](KITS.md) for the full description.

- The group header above its tracks collapses the group into one overview column (⌥J) that lists the pads hitting on each row, or expands it to show the member tracks.
- Members are ordinary tracks, so selection, copy/paste, transpose, step FX and automation work as on any track.
- **Hide empty** (`Option+U`) hides the pads a pattern does not use; the group header shows **+N hidden** to bring them back. Drag pads to reorder them or into another group; × or `Cmd+Backspace` deletes a pad (see [Managing tracks](#managing-tracks-hide-collapse-reorder-delete)).
- **Pad keys** (⌥Z) maps Z S X D C V … to the members of the cursor's group. While recording, a key enters that pad's note on its own row. With Pad keys off, keys play pitch on the cursor track.
- **Group** (⌘G) puts the selected tracks in a new group with a bus. **Ungroup** (⌘⇧G) takes the cursor track out of its group.
- Choke group and voice mode are track settings. An open hat chokes a closed hat on another track of the same kit.

### Replacing a kit

In the kit browser, **Load new kit** adds a new group. To keep an existing rhythm, put the cursor in that kit (or choose it under **Replace target**), then use **Replace selected kit**. Replacement maps the incoming sounds by pad position and keeps the member tracks' patterns, notes and step FX. Undo restores the previous kit.

Plug-in parameter automation stays connected when replacement uses the same processor in the same slot. If the incoming kit uses a different processor, saved parameter data is retained but is not translated to unrelated controls.

### Note recording placement

Pattern’s **Record placement** menu defaults to **Selected step**. With note recording armed, computer keys and MIDI write to the editing cursor even while playback runs. The cursor stays on that row during playback so you can build a kit row pad by pad with Pad keys; while stopped, notes advance by **Edit step** as usual. **Live playhead** records notes where the transport is playing and measures held-note gates. This preference is remembered independently of the Record arm and Keys switches.

### Where an automated plug-in parameter returns to

A lane or a step lock moves a plug-in parameter only while it applies. The value you last set yourself (in the plug-in’s editor, on its parameter pages, with a preset or a loaded state) is the parameter’s base value, for every plug-in and built-in effect; there is nothing to capture. The parameter is back at its base value on the row after a lock when no lane takes over, and whenever playback starts or jumps. Stopping leaves the plug-in as the song left it, so a tail rings out unchanged. Turning a control while a lock or lane holds it changes the base value. Saving while stopped stores the plug-in as you designed it (the base values are restored for the save); a project saved while the song plays stores the plug-in as it sounds at that moment.
