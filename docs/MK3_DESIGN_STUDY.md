# Maschine MK3 interface: design study (October 2026)

**Status: phase 1 implemented (October 2026, see §9); phases 2 and 3 remain proposals.**

The user owns an MK3, uses Maschine 3 every day, and doesn't much like the current SuperTracker MK3 setup. This
study describes how the controller works today, compares it with the Maschine interaction model, and proposes a
redesign in phases. It covers what is closer to Maschine and where a tracker should deliberately differ.

Sources:

- the code (`Resources/Web/mk3.js`, `mk3-screens.js`, `mk3-sim.js`, `Sources/SuperTracker/MaschineMK3*.swift`);
- `docs/MASCHINE_MK3.md`, which remains the control map of the current build;
- NI's hardware layout (printed labels, verified 25 Oct 2026:
  <https://docs.native-instruments.com/online-guides/maschine-accessibility/en/maschine-hardware-layout>);
- the Maschine MK3 / Maschine 2–3 manuals' interaction model (Group/Sound/Pattern/Scene/Section, Control/Plug-in/
  Channel modes, Browse, Pad modes, Step, Note Repeat/Arp, Lock snapshots, Events, Variation, Smart Strip).

The rendered screens referenced in §1–7 are in `test-output/mk3-design/*.png` (the interface before phase 1); the
phase-1 screens are in `test-output/mk3-design/phase1/*.png` (§9). Each PNG holds both 480 × 272 screens,
drawn by the native renderer (`MK3ScreenRenderer`, so the fonts and clipping match the hardware), with the group
(A–H) and pad LED colours beneath. The PNGs show LED colours as sent, not as quantised by the hardware palette. To
regenerate them, use `test-output/mk3-design/tools/` (`capture.js` drives the real `mk3.js` with the demo project,
and `render-main.swift` is compiled with `MaschineMK3Protocol.swift` + `MaschineMK3Screen.swift`).

---

## 1. How the current interface works

### 1.1 Model

The controller has no project state. It reads the live app state and keeps only these:

| Controller state | Values |
|---|---|
| **Screen mode** (`M.screen`) | `notes` (tracker), `macro` (step FX), `autoLanes`, `channel`, `channelMidi`, `plugin`, `arranger`, `patterns`, `mixer`, `mixerSends`, `browser`, `sampling`, `settings`. Selected by the left-hand mode buttons; pressing the lit button again returns to `notes`. Most modes also switch the app view. |
| **Overlay** (`overlayId()`) | Temporary pages over the screen mode: held/latched VOLUME, SWING, TEMPO, NOTE REPEAT, EVENTS, LOCK, VARIATION; Shift pickers (Velocity, Position, Tune, scale, chord set, perform FX, edit step, velocity curve, fixed velocity); FILE menu; STEP-pad edit. |
| **Pad mode** (`M.padMode`) | `pad` (instruments 1–16, or Kit Sounds, or slices with Shift), `keyboard`, `chords`, `step`, `scene` (song blocks), `pattern`. It is independent of the screen mode. |
| **Modifiers** | Hold buttons (Select, Duplicate, Erase, Mute, Solo, Events, Lock, Note Repeat, Volume, Swing, Tempo) act while held and latch on a quick tap. Shift gives a second function. |
| **Banks / pages** | Group bank (tracks 1–8 / 9–16), pad banks, step page, per-mode parameter page. |

Every page is one object `{title, buttons[8], knobs[8], main{turn,push,tilt}, data}`. The display-button labels
(top strip), the encoder labels (bottom strip) and the 4-D encoder therefore always belong to what is drawn. This
is the right foundation, and the redesign keeps it.

### 1.2 Screen chrome (both screens)

```
y   0–24   display-button labels (4 per screen, 120 px each, under the physical buttons)
y  24–46   title bar  L: [MODE chip] ■ 01 Track name           P01 Pattern name
                       R: 004/063   116.0 BPM      [SHIFT][AUTO][PATTERN] ● ■
y  48–202  page content
y 204–272  encoder labels: name / value / value bar (4 per screen, 120 px each)
```

### 1.3 Modes as rendered

| PNG | Mode | Left screen | Right screen | Encoders |
|---|---|---|---|---|
| 01 | NOTES (home) | Tracker grid: **3 tracks × 9 rows** | Cursor step detail, or "Empty" with help text | Note, Inst, Velocity, Gate, Pattern, Length, Edit step, Octave |
| 02 | NOTES + touch main encoder | grid | Pattern overview (density, playhead, loop) | same |
| 04 | MACRO | grid | FX1/FX2 boxes | FX1 type/value, FX2 type/value, Prob, Ratchet, µTime, Gate |
| 05 | Shift+MACRO | automation lane list | selected lane info | Parameter |
| 06–07 | CHANNEL | instrument name + envelope/filter/LFO drawing | instrument list (main encoder walks it) | 8 parameters of the page (Main, Envelopes, Filter·LFO, LFO 2, Play mode/Osc) |
| 08 | Shift+CHANNEL | routing info lines | – | volume, pan, sends (MIDI channels) |
| 09–10 | PLUG-IN | signal chain list | module name, parameter page list | 8 parameters of the page |
| 12–13 | ARRANGER / Shift | song blocks list | block detail | Block, Pattern, Repeats, Move, Colour, Loop song, Swing, Tempo |
| 14–15 | MIXER / Shift | 4 strips | 4 strips | volumes (Shift: pan / delay / reverb pages) |
| 16 | BROWSER | focused filter list | results + footer | filters 1–7, Results |
| 17 | SAMPLING | waveform (both screens) with markers | | Marker, Position, Fine, Zoom, Scroll, Play mode, Root, Slices |
| 18–19 | SETTINGS, FILE | list | detail | prefs |
| 20–22 | hold VOLUME / TEMPO / NOTE REPEAT | big value | hints | values |
| 23–24 | Shift+KEYBOARD / CHORDS | scale / chord-set list | chord pads | Root, Scale, Pads, Octave |
| 25–26 | STEP pads, hold step pad | 16 step cells | step detail | Note, Velocity, Gate, FX1/2 type/value, Inst |
| 27–28 | hold EVENTS / LOCK | step cells / page | | Nudge, Transpose, Velocity, Length, µTiming |
| 40–43 | Kit context | grid of Sound lanes, Sound chain, Sound mixer | | |

### 1.4 What gets in the way (observed in the renders and code)

1. **The tracker, the main thing, is small.** The home screen shows 3 tracks × 9 rows on one screen. The other
   screen is a large "Empty" detail panel for most rows (PNG 01). Encoders 2–4 are blank on empty rows.
2. **The 4-D encoder's touch sensor swaps the right screen to the overview.** Turning the encoder always touches
   it, so the step detail is never visible while you scroll rows (PNG 02).
3. **Printed legends are used for different jobs:**
   - NOTES is the fourth *Smart Strip* button (Pitch, Mod, Perform, Notes; strums pads in Maschine), but here it
     is the tracker/home button and the note-entry arm.
   - MACRO [Set] is a step-FX editor.
   - LOCK is step locks rather than snapshots.
   - FIXED VEL [16 Velocities], MUTE [Choke] and ERASE [Replace] have custom Shift functions.
   - Shift+ERASE is destructive (clears the whole track pattern) where Maschine's *Replace* is a recording mode.
4. **CHANNEL and PLUG-IN overlap.** The built-in instrument's parameter pages appear in both (PNG 06 vs 09–10). In
   Maschine, CHANNEL is channel properties (input/output/groove/MIDI/macros) and PLUG-IN is the slot chain.
5. **Mixer display buttons mute.** In Maschine, the display buttons *select* the channel; mute is MUTE + group/pad.
6. **Page navigation is inconsistent:**
   - CHANNEL pages are on display buttons 1–5, plug-in pages on display buttons 1–8.
   - The kit pages use "‹ Page / Page ›" buttons.
   - The ◄ ► arrows mean pages, browser tabs, octave, step page or pad bank depending on mode.
   - Maschine always uses ◄ ► for parameter pages and shows a page indicator.
7. **Overlays replace the whole page.** Holding VOLUME, TEMPO or NOTE REPEAT draws a big number over both screens
   (PNG 20–22). Maschine keeps the context and repurposes only the encoders/4-D encoder plus a small header.
8. **LEDs collapsed on the hardware** (fixed in this round, see §8). The muted track palette quantised to *white*
   for 5 of 8 groups. Selected/assigned/unassigned levels fell into the same intensity step.
9. **No macro layer, no snapshot layer, no 16 Velocities, no Choke, no Replace.** These are performance features
   Maschine users reach for every day.

## 2. The Maschine interaction model (reference)

- **Hierarchy:** Project → Groups (A–H per bank, unlimited banks) → 16 Sounds (one per pad). Each Sound, Group and
  the Master has a plug-in chain and channel properties. Group buttons select the focused group. Pads select and
  play Sounds.
- **Patterns** belong to a Group (per-group clips). **Scenes** (Ideas view) combine one pattern per group.
  **Sections** (Song view) reference Scenes on the timeline. SCENE pads pick scenes, Shift = Section; PATTERN pads
  pick the focused group's patterns. Pattern Length, Start and Grid sit on the Pattern page encoders.
- **Control area:** two screens, 8 display buttons and 8 touch knobs. Every mode follows the same grammar:
  - the top-left display buttons pick the **level**: Master / Group / Sound;
  - the left screen shows the context (slot list / channel tabs, focus);
  - the right screen and the knobs show the **parameter page**;
  - ◄ ► step pages, with page dots or page names on screen;
  - touching a knob shows its full name/value.
- **CHANNEL [MIDI]:** channel properties (Input, Output, Groove = per-sound swing, Macro); Shift = MIDI settings.
  **PLUG-IN [Instance]:** slot chain + plug-in parameter pages (NKS pages); Shift = choose instance.
  **MIXER:** levels of Sounds or Groups; the display buttons select; knobs = volume/pan; ◄ ► banks.
- **BROWSER [+Plug-in]:**
  - page 1: content type on the display buttons (Groups, Sounds, Instruments, Effects, Loops, One-shots, Samples)
    and Factory/User;
  - knobs: Product category, Product, Bank, Type, Sub-type, Mode/Character;
  - 4-D encoder: results, push to load;
  - prehear.
- **SAMPLING:** Record / Edit / Slice / Zone pages.
- **ARRANGER:** Ideas view (scenes) ⇄ Song view (sections, length, loop).
- **FILE [Save], SETTINGS:** menus and hardware preferences.
- **AUTO:** hold + turn knobs = write modulation into the pattern (pinnable).
- **MACRO [Set]:** 8 macro knobs per level, assigned to any parameters.
- **Pad input modes:**
  - PAD MODE (Group mode, pads = Sounds);
  - KEYBOARD (pads = scale notes of the focused Sound; settings: Root, Scale, Chord mode/type, Octave);
  - CHORDS (chord sets);
  - STEP (16 steps of the focused Sound; ◄ ► step pages; hold step pad + knobs = edit that step: velocity,
    length, position, pitch, and per-step modulation);
  - FIXED VEL [16 Velocities].
- **Encoder modes VOLUME [Velocity] / SWING [Position] / TEMPO [Tune]:** the 4-D encoder (and knobs) adjust the
  focused level's volume, swing, tempo. With Shift they adjust velocity, position or tune of the selected events.
- **Performance:**
  - NOTE REPEAT [Arpeggiator]: hold + pads; the display buttons pick the rate; the Arp page has Type, Rate,
    Sequence, Octaves, Dynamic, Gate, Retrigger.
  - LOCK [Exit]: Lock snapshots of parameter states, recall and morph; Extended Lock.
- **Smart Strip:** PITCH, MOD, PERFORM (FX Select), NOTES (strum).
- **Pad Mode zone modifiers:**
  - SCENE [Section], PATTERN, EVENTS;
  - VARIATION [Navigate]: humanize/random; Navigate = page/zoom the editor;
  - DUPLICATE [Double], SELECT, SOLO, MUTE [Choke].
  - They work while held, and many are pinnable.
- **Transport:**
  - RESTART [Loop], ERASE [Replace], TAP [Metronome], FOLLOW [Grid];
  - PLAY, REC [Count In], STOP, SHIFT;
  - Shift + pads = Undo / Redo / Step Undo / Step Redo / Quantize / Quantize 50% / Nudge / Clear / Clear Auto /
    Copy / Paste / Semitone ± / Octave ±.

## 3. Mapping the two worlds

| Maschine | SuperTracker today | Should a tracker differ? |
|---|---|---|
| Group | Track (A–H, banks of 8). In Kit lanes: Kit | Keep. A track is a "group" with one instrument; a Kit lane is a real group with 16 Sounds. |
| Sound | Instrument (ordinary track) / Kit Sound | Keep. Pads = Sounds only in Kit lanes or Pad mode. |
| Pattern (per group) | Pattern = **all tracks** (tracker pattern) | **Yes, differ.** A tracker pattern is Maschine's *Scene*. PATTERN pads pick tracker patterns. Don't fake per-group patterns. |
| Scene / Section | Song blocks / block loop | Partly. SCENE = song blocks; Section = *named ranges of blocks* (new, lightweight). |
| Pattern editor (piano roll / step) | Vertical tracker grid | **Yes, differ.** The tracker grid is home. Maschine-style step mode stays a pad mode. |
| Step modulation (hold step + knob) | LOCK + encoder writes FX-column locks; step-pad hold edits note fields | Merge: both paths write the same FX-column locks. |
| Lock snapshots | – | Later, optional (phase 3). |
| Macro | – (MACRO = step FX) | Adopt Maschine (phase 3). |
| Channel properties | Built-in instrument params (duplicate of PLUG-IN) | Adopt Maschine: CHANNEL = levels, sends, output/routing, groove/swing, MIDI. |

## 4. Redesign principles

1. **Printed legends win.** A button does what its label says (main or Shift) unless the feature does not exist in
   a tracker. In that case the closest tracker equivalent is used and documented, e.g. PATTERN = tracker
   patterns.
2. **One grammar for every parameter mode** (CHANNEL, PLUG-IN, MIXER, SAMPLING, MACRO):
   - display buttons 1–3 = level (Master · Track/Group · Instrument/Sound) when applicable, 4 = mode action;
   - 5–8 = page-local actions;
   - ◄ ► = parameter pages, always;
   - left screen = context (chain, list, waveform), right screen = page header + page dots;
   - encoders = 8 parameters.
3. **Home is the tracker**, spread over *both* screens. No mode button lit = home. Pressing a lit mode button
   returns home (already the case).
4. **Encoder modes, not overlays.** VOLUME, SWING, TEMPO, NOTE REPEAT and EVENTS keep the current screen. They
   change the encoder labels and add a header strip; they don't replace the page.
5. **Hold = momentary, quick tap = pin** (already the rule). A pinned mode shows a pin glyph in the title chip.
6. **Tracker strengths stay first-class:** FX columns, step locks, edit step, lines per beat, pattern-wide
   operations, song blocks.

## 5. Proposed screens

### 5.1 Home: tracker across both screens (replaces NOTES page)

Eight track columns of 120 px, so each column sits exactly above its encoder and below its display button, and
lines up with group buttons A–H of the current bank. Eleven rows at 13 px (mono 12 px).

```
┌──────────── left screen (tracks 1–4) ─────────────┐┌──────────── right screen (tracks 5–8) ────────────┐
│ VelvetBass │ GlassKeys │ WarmPad  │ Kick          ││ Snare    │ ClosedHat│ OpenHat  │ Pulse          │ ← display btn = select track (Shift: mute)
│[HOME] P01 Night drive · row 004/063 · ins 00 ·    ││ 116.0 BPM · LPB 4 · step 1 · oct 4 · ▶ ● PATTERN │
│000 D-2 98  │ D-5 84    │ D-4 65   │ C-4 ·· VOL40  ││ ...                                              │
│001 ···     │ ···       │ ···      │ ···           ││                                                  │
│...  (11 rows, cursor row highlighted, playhead green, bar lines)                                     │
│004 [···]   │ F-5 72    │          │               ││                                                  │
├────────────┴───────────┴──────────┴───────────────┤├──────────────────────────────────────────────────┤
│ Note C-4   │ Ins 00    │ Vel 100  │ Gate 0.80     ││ FX1 VOL 40 │ FX2 ···  │ Prob off │ Pattern 01     │ ← encoders: cursor step (fields of the cursor column first)
└───────────────────────────────────────────────────┘└──────────────────────────────────────────────────┘
```

- Column content adapts to width:
  - `note vel`, plus FX abbreviation when the cursor column is FX;
  - Kit lanes show the selected Sound column, as now (Summary view keeps the hit list).
- The encoders are the **cursor step**: Note, Instrument/Sound, Velocity, Gate, FX1, FX2, Probability, plus
  Pattern on encoder 8. On an empty row, turning Note creates the step, as now. The other encoders show
  greyed-out defaults instead of blanks, so every encoder has a label.
- Touching the 4-D encoder no longer swaps screens. The overview appears only when you **touch without turning
  for 400 ms**, or with Shift+touch. While turning, a 3 px pattern-position bar in the title shows where you are.
- Alternative layout (Shift+◄ / ►): 4 tracks wide with full columns (note · ins · vel · fx1 · fx2), for FX-heavy
  editing.

### 5.2 PLUG-IN [Instance]: the single home of instrument and effect parameters

```
┌─ left ──────────────────────────────────────────┐┌─ right ─────────────────────────────────────────┐
│ MASTER │ TRACK  │ INSTR. │ Bypass ││ Editor │ Browse │  + FX  │ Remove                             │
│[PLUG-IN] 01 Velvet bass                          ││ Velvet bass · Main        ● ○ ○ ○ ○  page 1/5   │
│  1 ▸ Velvet bass        Synth                    ││  [envelope / filter drawing of this page]       │
│  2   Delay              Insert 1                 ││                                                 │
│  3   + empty slot                                ││                                                 │
├──────────────────────────────────────────────────┤├─────────────────────────────────────────────────┤
│ Volume │ Pan │ Tune │ Fine                       ││ Cutoff │ Reso │ Drive │ Bits                    │
└──────────────────────────────────────────────────┘└─────────────────────────────────────────────────┘
```

- The 4-D encoder walks the slots. Push = Instance (choose / replace plug-in). ◄ ► = pages, with page dots.
- Display buttons 1–3 = level:
  - Master;
  - Track (or Group for Kit lanes);
  - Instrument (or Sound).
- The chain shown is that level's chain. This is what Kit pages already do; ordinary tracks get the same model
  (Master inserts, track inserts, instrument).

### 5.3 CHANNEL [MIDI]: channel properties, not instrument parameters

Pages (◄ ►):

- **Levels:** Volume, Pan, Delay send, Reverb send, Mute, Solo.
- **Output:** Route, Bus, Sidechain source.
- **Groove:** per-track swing amount if the engine adds it; otherwise project swing + LPB.
- **MIDI** (Shift+CHANNEL): in/out ports, channels, root.
- **Freeze:** Freeze, Bounce, Duplicate.

The left screen shows the same level selector and the track's channel strip with a meter.

### 5.4 MIXER

- Display buttons **select** the strip (bright = selected).
- Shift + display = mute, MUTE + display = mute, SOLO + display = solo.
- Encoders: page 1 Volume, page 2 Pan, page 3/4 sends (◄ ►). Shift+◄ ► = bank.
- Kit lanes: display button 1 toggles **Sounds ⇄ Groups**, like Maschine's mixer level button.

### 5.5 Encoder modes (VOLUME / SWING / TEMPO, and Shift = Velocity / Position / Tune)

Keep the current page. Replace only the encoder strip and add a 20 px header:

```
│ VOLUME ▸ 01 Velvet bass  −8.4 dB    (4-D: selected · Shift+4-D: master) │
│ Velvet │ Glass │ Warm │ Kick ││ Snare │ CHat │ OHat │ Pulse │   ← encoders = 8 track volumes of the bank
```

Shift+VOLUME (Velocity) applies to the selection, not just the cursor hit, so it works like Maschine on selected
events.

### 5.6 Pad mode settings (KEYBOARD / CHORDS)

Pressing the lit KEYBOARD (or CHORDS) button opens the Maschine-style **pad mode page**:

- KEYBOARD knobs: Root, Scale, Octave, Pad layout (Snap / Degrees), Velocity curve.
- CHORDS knobs: Mode (Harmonizer/Chord set), Chord type/set, Position.
- The left screen shows a 4 × 4 pad map with note names, laid out like the pads.

Shift+pads 13–16 = semitone/octave already works.

### 5.7 STEP

- Same as now (16 steps of the selected track or Sound, ◄ ► page).
- **FOLLOW** makes the step page follow the playhead (Maschine), instead of Shift+STEP.
- **Hold step pad + turn any parameter encoder of the current PLUG-IN/CHANNEL page** writes a step lock (Maschine
  step modulation).
- Display button 1 "Event" toggles the knob set back to Note/Vel/Gate/Position. Today, holding a pad always swaps
  in the event fields.

### 5.8 NOTE REPEAT [Arp]

- Hold: rates on display buttons (as now).
- Shift+NOTE REPEAT opens the **Arp page** with knobs Type, Rate, Octaves, Gate, Dynamic, Retrigger. This needs
  STNoteRepeat to expose octave/gate.
- Pin: tap.

### 5.9 PATTERN / SCENE [Section]

- PATTERN, held or pinned:
  - pads = patterns (as now);
  - screen = pattern list + **Length, LPB, Start, Name, Colour** on the encoders (moved off the home page);
  - Shift+PATTERN = new pattern.
- SCENE: pads = song blocks (as now).
  - Shift+SCENE = **Section** (named loop ranges of blocks). Pads = sections; pressing one sets the song loop to
    it, and the change is queued at pattern end while playing.

### 5.10 MACRO [Set] and LOCK [Exit]

- **MACRO:** 8 macro knobs per track/Kit level (and Master). Shift+MACRO (Set) + touch a parameter encoder in
  PLUG-IN/CHANNEL + touch a macro slot = assign.
- The step-FX editor moves into Home: the cursor on an FX column shows FX type/value encoders, and Shift + display
  buttons give the quick effects.
- **LOCK:** keep "hold LOCK + turn = step lock". It is a tracker strength and already coherent. Shift+LOCK = Exit,
  as printed.
- Lock *snapshots* (Maschine) are a phase-3 option: 16 snapshots per project on pads while LOCK is pinned, morph
  on the strip.

### 5.11 NOTES, ERASE [Replace], FIXED VEL [16 Vel], MUTE [Choke]

- NOTES becomes the **Notes strip mode** (strum through the current pad layout / scale) as printed. Home no longer
  needs a button (§4.3). Note-entry arm stays on REC (Shift+REC = count-in).
- Shift+ERASE = **Replace** recording toggle (incoming notes overwrite the rows they pass) instead of clearing the
  pattern. Clear-track moves to ERASE + group button (exists).
- Shift+FIXED VEL = **16 Velocities**: pads play the selected Sound/instrument at 16 velocity levels. The velocity
  curve moves to SETTINGS.
- Shift+MUTE = **Choke**: MUTE+Shift+pad/group cuts the voice immediately (engine note-off/cut). Clear-all-mutes
  moves to Shift+MUTE+MUTE-display, or stays on Shift+SOLO-style long-press.

### 5.12 AUTO

Hold AUTO + turn = momentary automation write (Maschine). Tap = pin write mode. Shift+AUTO = Touch / Latch / Write
cycle. Today a tap toggles and there is no momentary write.

## 6. Proposed button map (changes only)

| Control | Today | Proposed | Phase |
|---|---|---|---|
| NOTES | Home / tracker; again = note-entry arm | Notes strip mode (strum) | 2 |
| Home | NOTES | No lit mode = home; lit mode button returns home | 1 |
| CHANNEL | Built-in instrument pages | Channel properties (Levels/Output/Groove/Freeze); Shift = MIDI | 2 |
| PLUG-IN | Chain + pages (instrument duplicates CHANNEL) | Level buttons Master/Track/Instrument; only home of parameters | 1 |
| ◄ ► | Pages / tabs / octave / step page / pad bank | Always pages (browser: tabs = pages). Pad-mode bank moves to Shift+◄ ► only when no pages; octave = Shift+pads 15/16 | 1 |
| MIXER display buttons | Mute | Select; Shift = mute | 1 |
| VOLUME/SWING/TEMPO hold | Full-screen overlay | Encoder mode over current page | 1 |
| Shift+FIXED VEL | Velocity curve | 16 Velocities | 2 |
| Shift+ERASE | Clear track pattern | Replace recording | 2 |
| Shift+MUTE | Clear mutes | Choke | 3 |
| MACRO / Shift | Step FX / automation lanes | Macros / Set | 3 |
| Shift+NOTE REPEAT | Cycle arp mode | Arp page | 2 |
| Shift+SCENE | Loop blocks via Shift+pads | Sections | 3 |
| PATTERN page | Length on home encoders | Pattern page encoders (Length, LPB, Name, Colour) | 1 |
| Shift+STEP | Step page follows | (FOLLOW does it) | 1 |
| AUTO | Toggle | Hold = write, tap = pin | 2 |
| 4-D touch | Overview immediately | Overview after 400 ms still touch | 1 |

## 7. Phases and effort

The estimates are focused engineering days, including tests in the existing `tests/mk3_*` style and simulator
checks. Hardware acceptance by the user comes after each phase.

**Phase 1: layout and grammar (≈ 6–8 days). Highest value, no new engine features.**

1. Home across both screens, 8 × 120 px columns, encoders = cursor step, overview on still-touch: 2.5 d.
2. Unified page grammar: ◄ ► always pages, page dots, level buttons on PLUG-IN for ordinary tracks
   (Master/Track/Instrument chains already exist in data): 2 d.
3. Encoder modes instead of full-screen overlays (VOLUME/SWING/TEMPO/NOTE REPEAT/EVENTS header strip): 1 d.
4. Mixer: display buttons select, Shift mute: 0.5 d.
5. PATTERN page encoders; FOLLOW drives the step page: 0.5 d.
6. Docs/help/control-map update: 0.5 d.

**Phase 2: printed legends (≈ 6–9 days). Needs small app/engine hooks owned by other areas.**

1. CHANNEL as channel properties (levels/output/freeze/MIDI); remove the instrument-page duplicate: 1.5 d.
2. NOTES strip strum mode: 1 d.
3. 16 Velocities: 1 d.
4. Replace recording: 1.5 d. Needs an app recording flag (pattern UI / engine owners).
5. Arp page: 1 d. Needs STNoteRepeat octave/gate.
6. AUTO hold-to-write: 0.5 d.
7. Pad mode settings page for KEYBOARD/CHORDS: 1 d.

**Phase 3: new performance features (≈ 8–12 days, needs project-model work).**

1. Macros (8 per level, assignment flow, persistence, automation): 4–5 d with project/engine owners.
2. Choke: 1 d. Needs an engine voice-cut command.
3. Sections (named block ranges) on Shift+SCENE: 1.5 d. Song model owner.
4. Optional Lock snapshots + strip morph: 3–4 d.

Where a tracker should keep differing:

- Pattern = all tracks;
- vertical grid as home;
- FX columns and LOCK step locks;
- edit step / LPB;
- song blocks instead of a timeline.

These are the reasons to use SuperTracker. The redesign makes the *controls* Maschine-native around them.

## 8. Fixes made during this study (already implemented)

See the final report for file/line detail. All of these are covered by `tests/mk3_state_sync_tests.js` and
`tests/MaschineMK3ProtocolTests.swift`:

- LED palette:
  - muted track colours no longer quantise to white on groups/pads;
  - LED colours are normalised to full value before brightness scaling;
  - dim 6 % markers light.
- Stale controller state:
  - step page clamps when the pattern gets shorter;
  - group bank follows the selected track and clamps;
  - pad banks clamp.
- Shift+ARRANGER pattern list opens on the active pattern.
- Touch-strip scrub seeks once per row, not once per strip report.
- Screens:
  - only the changed screen is sent;
  - the page is built once per frame;
  - a failed USB screen write is retried instead of leaving a stale screen.
- Holding LOCK no longer silently edits non-lockable instrument/channel parameters for every step.
- The plug-in / Kit page list no longer runs into the encoder labels.
- The simulator places NOTES with the strip mode buttons, as on the hardware. Gate is shown in rows everywhere.

## 9. Phase 1 as implemented

All five phase-1 items are in `Resources/Web/mk3.js` (controller pages) and `Resources/Web/mk3-screens.js` (display
lists); the native side (`MaschineMK3*.swift`) needed no change, since the new screens use the existing draw ops.
Tests: `tests/mk3_phase1_tests.js` (one group per item) plus the updated `tests/mk3_*` suites
(`./scripts/test.sh --only mk3`). Renders: `test-output/mk3-design/phase1/*.png`, regenerated with
`node test-output/mk3-design/tools/capture-phase1.js out.json`, then `render-main.swift` as described at the top (copy it to
`main.swift` when compiling it with `swiftc`).

### 9.1 Home across both screens (PNG 01–08, 42, 50, 54)

- Eight 120 px columns, four per screen, each under its display button and above its encoder. Ordinary tracks
  follow the group bank, so column *n* is group button *n* (Shift + ◄ ► changes the bank). Eleven 14 px rows
  around the cursor (or the followed playhead), row numbers on the left screen, a faint tint on the selected
  column.
- A cell shows `note velocity FX`. The cursor cell shows the field under the column cursor, highlighted:
  instrument replaces velocity, and FX 2 replaces FX 1.
- The display buttons carry the column names with a colour strip and M / S flags:
  - press: select the column;
  - Shift + press, or MUTE held: mute;
  - SOLO held: solo.
- **Choice:** the old Notes tools (Copy, Paste, Interp, Clear, Double, Stretch, Note off, Step FX, with their
  Shift variants) appear on the display buttons while SELECT is held or latched.
- Encoders (cursor step): Note, Inst/Sound, Velocity, Gate, FX 1, FX 2, Probability, Pattern.
  - FX 1 / FX 2 edit the value of an effect on the step. When the column is empty they pick its type.
  - On an empty row every encoder still has a label. Inst, Velocity, Gate, FX and Prob show their defaults greyed
    out and do nothing; Note creates the step.
  - FX-only rows (no note) are made on MACRO.
- Length, Edit step and Octave left Home:
  - Length is on the PATTERN page;
  - Edit step is on Shift + FOLLOW;
  - Octave is on ◄ ► in KEYBOARD/CHORDS and on Shift + pads 15/16.
- Kit lanes page their Sound columns by eight around the cursor Sound. The columns come from
  `STKits.patternSounds(track)` (used and loaded pads, or all pads), with a `'used'` lane-view fallback. Compact
  lanes keep one summary column.
- MACRO keeps the four-column grid on the left and the step FX on the right.
- **Not done:** the alternative 4-wide full-column layout (Shift + ◄ ►, which stays the track bank).

### 9.2 No accidental overview (PNG 03, 04)

- Turning always touches the 4-D encoder, so a touch alone no longer swaps the right screen.
- The overview appears after 400 ms of touch without turning, or at once with Shift + touch.
- Once shown, it stays until release, so the cursor can be turned with it in view.
- A 3 px pattern-position bar under the title of every grid page marks the cursor and the playhead across both
  screens.

### 9.3 One page grammar (PNG 20–25, 51–52)

- Display buttons 1–3 pick the level:
  - ordinary tracks: Master · Track · Instr.;
  - Kit lanes: Master · Group · Sound (the order changed from Sound · Group · Master).
- The level is shared by CHANNEL and PLUG-IN.
- PLUG-IN lists that level's chain on the left. The 4-D encoder turns through the chain; push opens its pages.
  - Instr.: the built-in instrument (all its pages), NKS or the plug-in instrument.
  - Track: the insert effects, then Sends.
  - Master: the master inserts (`STPluginPages.controller.forOwner('master')`), then Master volume / swing / tempo.
- Display buttons 4–8 on PLUG-IN are Editor (Shift: Chain), Browse, + FX, Audition, Automate.
- ◄ ► always step the parameter pages of CHANNEL, PLUG-IN, MIXER and the Kit pages. Every page with more than
  one page shows page dots in the title bar ("3 / 20" beyond eight). There are no page buttons any more.
  - Pages without parameter pages keep ◄ ► for the pad mode: step page, octave, pad bank.
  - Shift + ◄ ► is always the track bank (Kit: group).
- **Choice:** CHANNEL keeps the instrument pages at its Instr. level until phase 2 turns it into channel
  properties; PLUG-IN · Instr. shows the same pages. CHANNEL · Track is the channel strip (Volume, Pan, Delay,
  Reverb, Mute, Solo) and CHANNEL · Master the master output.
- Bypass and Remove (added later, through the public slot API `window.STInserts` in `studio.js`): when the selected
  module is an insert effect (Track, Master, Kit Sound / Group), display button 4 is Bypass (lit and labelled
  "Bypassed" while off; the chain list shows "· off") and display button 8 is Remove. Remove asks first: the button
  turns into a lit "Confirm" with the flash "Remove *name*? Press again" for 2.5 s; Shift + Remove removes at once.
  Both are one Undo step. Built-in effects show their curated pages (`effect-pages.js`). Tests:
  `tests/mk3_effect_pages_tests.js`; renders: `node test-output/mk3-design/tools/capture-effects.js out.json`.

### 9.4 Encoder modes, no full-screen overlays (PNG 10–16, 32)

- These modes keep the current page:
  - VOLUME, SWING, TEMPO, NOTE REPEAT and EVENTS;
  - the Shift variants Velocity / Position / Tune;
  - LOCK.
- They replace the encoders (and the display buttons when the mode has its own, such as rates, presets or
  EVENTS tools).
- A 22 px header above the encoder labels shows the mode chip, target and value on the left, and what the 4-D
  encoder does on the right.
- EVENTS marks the rows it edits on the cursor column.
- FILE, the Shift pickers (scale, chord set, perform FX, edit step, velocity curve, fixed velocity) and VARIATION
  are still overlays.

### 9.5 Smaller moves (PNG 30, 31, 40)

- MIXER:
  - the display buttons select (bright = selected);
  - Shift + display, or MUTE held, mutes; SOLO held solos;
  - ◄ ► page through Volume / Pan / Delay / Reverb (Shift + MIXER opens on Pan);
  - Kit mixers select Sounds the same way.
- PATTERN pins PATTERN pad mode and shows the PATTERN page:
  - the pattern list with Pattern, Length, Lines/beat and Colour (then Swing, Tempo) on the encoders;
  - pressing PATTERN again returns to the previous pad mode and screen;
  - a mode button leaves the page, and the pads stay patterns;
  - Shift + PATTERN adds a pattern.
  - **Not done:** the momentary hold.
- FOLLOW makes the STEP page follow the playhead. Shift + STEP is plain STEP.

