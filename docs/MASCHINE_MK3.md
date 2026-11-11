# Maschine MK3 controller interface (in development)

> **In development, off by default.** Maschine MK3 support is experimental and may change or misbehave. Turn it
> on in **Settings → Maschine MK3 (in development) → Enable Maschine MK3 controller support**; the choice is saved
> in `maschine-mk3.json` in SuperTracker's support folder. While it is off, SuperTracker never opens the
> controller, and Native Instruments software keeps it to itself. `SUPERTRACKER_NO_MK3=1` keeps it closed at launch
> even when the setting is on. The in-app MK3 simulator (`Cmd+K` → **Maschine MK3 simulator**) works either way.

SuperTracker drives the Maschine MK3 directly over USB: it reads every button, pad (with pressure), encoder,
touch sensor and the touch strip, lights every LED (RGB pads, RGB groups, button backlights, touch-strip LEDs)
and draws both 480 × 272 colour screens. It is not a MIDI template: the controller follows the tracker's state.

## Protocol sources

- Native Instruments MK3 USB protocol notes from the open-source `cabl` project (`doc/hardware/maschine-mk3`) and
  the `Encdr` project's MK3 descriptor and hardware reference (pad tuple format, LED byte encoding, screen blit).
- Device: USB `17cc:1600`. Interface 4 (HID): input endpoint `0x83` (report `0x01` buttons/encoders/strip,
  report `0x02` pads), output endpoint `0x03` (report `0x80` button LEDs, `0x81` touch-strip + pad LEDs).
  Interface 5: bulk endpoint `0x04` for the screens (16-bit pixels, 32-byte header, `03 00 00 00 40 00 00 00`
  footer, partial updates aligned to 4 × 2 pixels).
- NI's own background agents (NIHardwareAgent, NIHostIntegrationAgent) normally own the controller. Buttons, pads
  and LEDs work alongside them; the screens need exclusive access, which SuperTracker only takes when the user
  chooses **⌘K → Maschine MK3: take over screens…**, and restores on quit.

## Connecting

Power on the MK3 and connect its USB cable to this Mac. Open the command palette with **⌘K**, search **Maschine**, choose **Maschine MK3: take over screens…**, then confirm. Takeover is successful only after both display initialization transfers finish. Missing devices and USB failures produce an error instead of a success message; the last error is also shown in Studio settings. If the MK3 is absent, NI agents are not paused. Choose **Maschine MK3: release controller** or quit to return ownership.

## Design principles

1. **The tracker is the model.** Pads, screens and LEDs always show the current project, track, pattern and row.
   Nothing on the controller has hidden state that the app does not show.
2. **Every button has one dedicated job**, and **Shift** gives it a related second job, printed on the hardware
   where implemented (e.g. *Restart / Loop*, *Tap / Metro*, *Rec / Count-in*). Remaining custom mappings are called out below.
3. **Modes are visible.** The lit mode button, the screen title bar and the pad colours always agree.
4. **Hold-to-modify buttons** (Select, Duplicate, Erase, Mute, Solo, Events, Lock, Note Repeat, Volume, Swing,
   Tempo, Auto) act while held and return to the previous state on release; a quick tap latches them.
5. **The 8 display buttons and 8 encoders always belong to what the screens show.** Their labels are drawn right
   above/below them.

## Control map

Phase 1 of the [design study](MK3_DESIGN_STUDY.md) (§9) changed Home, CHANNEL, PLUG-IN, MIXER, ◄ ►, PATTERN, FOLLOW and the VOLUME/SWING/TEMPO-style modes; the rows below reflect it.

### Left column (screen modes)

| Button | Press | Shift + press |
|---|---|---|
| CHANNEL (MIDI) | Display buttons 1–3 pick the level (Master · Track · Instr.; while the cursor track is in a track group 1–4: Master · Group · Track · Instr.), shared with PLUG-IN. Instr.: the instrument pages (Volume/Pan/Tune, Filter, Envelopes, LFO, Drive, Play mode); Track: channel strip (Volume, Pan, Delay, Reverb, Mute, Solo; on a sample track also Voice — Poly / Mono / Legato — and Choke group Off / 1–16, the track's Voice behavior, so kit pads such as open and closed hats choke each other); Group: the group bus (Volume, Pan, Mute, Solo; tilt ◄ ► walks the group's tracks); Master: master output. ◄ ► step pages | Instrument MIDI/external routing page |
| PLUG-IN (Instance) | The chosen level's chain on the left (Instr.: instrument; Track: inserts then Sends; Group: the group bus inserts then Levels (volume, pan); Master: master inserts then master volume); the main encoder walks it, push opens its pages, ◄ ► step pages with page dots. The display buttons after the level buttons: Editor, Browse, + FX, Audition, Automate | Full-screen chain list (Instance) |
| ARRANGER | Song arranger on the screens: blocks, repeats, section loop | Pattern list / sequence overview |
| MIXER | Mixer of the group-button bank: one strip per entry (a track's channel, or a track group's bus); 8 encoders = volumes; display buttons select (a group: its last selected track; Shift or MUTE held: mute; SOLO held: solo); ◄ ► page Volume / Pan / Delay / Reverb (group buses: volume and pan only) | Pan / send page (encoders = pan, delay send, reverb send by page) |
| BROWSER (+Plug-in) | Library browser: left screen = filter lists (Expansion, Product, Bank, Type…), right = results; encoders = filters, main encoder = focused column | Plug-in browser for the selected slot |
| SAMPLING | Sample editor + recorder for the selected instrument | Start/stop audio input recording |
| ◄ / ► | Previous / next parameter page on CHANNEL, PLUG-IN and MIXER; elsewhere the pad mode's page (step page, octave, pad bank; PAD MODE in a group: its tracks 1–16 / 17–32 …; 16 Velocities: the note a semitone down / up) | Previous / next group-button bank (entries 1–8 / 9–16 …) |
| FILE (Save) | File menu list: Save, Save as, Open, New, Export audio/MIDI, Import MIDI, Rename, Demo; turn chooses, push runs, FILE closes | Save project |
| SETTINGS | Controller settings list (pad sensitivity, velocity curve, LED/screen brightness, pad pressure): turn selects, tilt ◄ ► changes; take-over buttons | App settings dialog |
| AUTO | Tap: automation write Read ⇄ last write mode (Touch/Latch/Write). Hold from Read and turn an encoder, the 4-D encoder or the touch strip: writes only while held, release returns to Read (*written*); held without turning it stays in write like a tap | Cycle write mode |
| MACRO (Set) | Step FX editor for the cursor step: encoders set FX1/FX2 type and value, probability, ratchet, microtiming, glide, gate | Automation lane picker on the screens (automated parameters first) |

### Screens, display buttons and encoders

The two screens form one 960 × 272 canvas. A title bar shows the mode, project, track, pattern, row/position and
transport. The 8 display buttons and 8 touch-sensitive encoders are labelled on screen; touching an encoder shows
its full name and value. **Home** spans both screens: eight 120 px columns, each under its display button and
above its encoder. The columns are the pattern editor's columns: every track, and a **collapsed** track group as
one column labelled with the group's name and colour, whose rows list the group's tracks that hit there (track
number in the group, plus the pitch when it is not the track's pad note). The window starts at the group-button
bank and scrolls to keep the cursor column in view. The encoders edit the cursor step: Note, Inst, Velocity, Gate,
FX 1, FX 2, Probability, Pattern. Display buttons select a column (a collapsed group: its last selected track;
Shift or MUTE held: mute, SOLO held: solo; on a collapsed group they mute / solo the group bus); with SELECT held
they carry the pattern tools (Copy, Paste, Interp,
Clear, Double, Stretch, Note off, Step FX). VOLUME, SWING, TEMPO, NOTE REPEAT, EVENTS and LOCK keep the current
page and only swap the encoders, with a small header showing the mode and value.

### 4-D encoder (turn, push, tilt, touch)

| Gesture | Pattern view | Lists (browser, pickers, menus) | PLUG-IN signal chain | Value focus |
|---|---|---|---|---|
| Turn | Move cursor row (Shift: by edit step × 4) | Move the cursor; fast turns accelerate on long lists; Shift + turn jumps a page (7 rows) | Chain focus: next/previous module · pages focus: parameter page | Adjust the focused value (Shift fine) |
| Tilt ◄ ► | Previous / next column (note, ins, vel, FX1, FX2), crossing tracks | Previous / next column (browser: filter lists ↔ results; scale: root; chords: chord) | Previous / next module | – |
| Tilt ▲ ▼ | Previous / next track | Page up / down | Previous / next parameter page | – |
| Push | Audition / enter value edit on the cursor cell | Choose / load (browser filter: apply now and return to the results) | Chain ⇄ pages focus | Confirm |
| Touch | After 400 ms of touch without turning (or at once with Shift), shows a pattern overview (whole pattern, playhead, loop range) until release | – | – | – |

Back is always the screen's mode button (MACRO, PLUG-IN, FILE, SETTINGS, ARRANGER, Shift + KEYBOARD/CHORDS…)
or a display button labelled **Back**. Picker overlays whose turn applies live (scale, chord set, perform FX,
plug-in chain) also close with push.

### Lists

Every list on the screens uses one component (`drawList` in `mk3-screens.js`, `listData`/`listView`/`listMain`
in `mk3-lists.js`):

- **7 visible rows** of 19 px with 14 px text under an 18 px header (6 rows when a footer line is shown, 4–5 in
  split layouts). The header shows the title and the position, e.g. `12 / 340`; long lists show a scroll bar.
- **Scrolloff:** while the list fits, the cursor moves freely. In a longer list the cursor travels until it is
  2 rows from the bottom (or top) edge, then the list scrolls under it; at either end the cursor reaches the
  first/last row. The window is remembered per list, so changing direction never makes the list jump.
- The **active** list's cursor is filled with the accent colour; an inactive list (the other column) keeps an
  outlined cursor. A small accent bar marks the applied value (current filter value, active pattern, automated
  parameter). The second column holds counts, vendors, types or values. Empty lists say so; searching lists show
  `Searching…`.
- **Main encoder:** quick consecutive detents on lists longer than three windows accelerate (×2 … ×8, capped by
  the list length); Shift + turn and tilt ▲ ▼ jump one page.
- **List parameters on encoders** (filter type, play mode, LFO destination, FX type, pattern, block, root,
  chord, velocity curve, pressure, note-repeat rate and mode…): touching or turning the encoder shows a 5-row choice list above it instead of
  a single value. Browser filter encoders open their full list on the left screen instead.

Lists using it: browser filters and results (Samples / NKS / Plug-ins / Kits), PLUG-IN signal chain and parameter
pages (track, group bus and master chains), Shift + PLUG-IN chain, CHANNEL instrument list, ARRANGER blocks, Shift + ARRANGER patterns,
Shift + MACRO automation lanes, FILE menu, SETTINGS, Shift + KEYBOARD scales, Shift + CHORDS chord sets,
Shift + PERFORM effects, Shift + NOTE REPEAT arp modes.

### Browser columns

The browser has one column per list filter and a results column. Kits: Source, Expansion, Type; NKS: Product,
Bank, Type, Sub-type, Character; Samples: Category, Type, Key, Pack, Source. Plug-ins have only results (Kind and
Format stay on encoders 1–2).

- The **left screen** shows the focused filter as a list (`All expansions` first, then every value with its
  count). With the results focused it shows a filter overview (each filter and its value).
- The **right screen** shows the results (6 rows) and a footer with the selected item's details
  (pack / vendor · BPM · key, ▶ while previewing).
- Turning a filter encoder focuses that filter and moves its cursor; touching it previews its list. The filter
  follows the cursor after a 260 ms pause, so fast scrolling does not start a search per detent; push applies at
  once and returns to the results. Encoder 8 moves the results.
- Tilt ◄ from the results re-enters the last filter; tilt ◄ ► walks the columns. ◄ / ► arrow buttons and display
  buttons 1–4 switch tabs.

### Signal chain (PLUG-IN)

PLUG-IN shows the selected track's chain as a list on the left screen and the selected module's parameter pages
on the right; the 8 encoders always control the selected module's current page.

- Ordinary tracks: **Instrument** (built-in synth/sampler, using the CHANNEL parameter groups), the plug-in
  instrument or NKS instrument, **Insert 1…n**, and **Sends** (track volume, pan, delay send, reverb send).
- Group level (cursor track in a track group): the group bus inserts, then **Levels** (bus volume and pan; a group
  bus has no built-in sends). Master level: master inserts, then the master volume.
- PLUG-IN opens in chain focus: turn (or tilt ◄ ►) walks the modules. Push opens the module's parameter pages
  (pages focus: turn and tilt ▲ ▼ change the page); push again or PLUG-IN returns to the chain; PLUG-IN in chain
  focus leaves the mode. Shift + PLUG-IN shows the same chain full screen.
- A module still reading its parameters shows *Reading parameters…*; a module without mapped parameters shows
  *No parameters exposed* with Editor / Chain / Browse on the display buttons; errors are shown in red.

### Performance column

| Button | Press / hold | Shift |
|---|---|---|
| NOTES | Open Notes view; press again to toggle note-entry arm. The button is brighter on this page; its protocol controls brightness, not orange colour. REC indicates recording arm. | Insert note-off at the cursor |
| VOLUME (Velocity) | Hold: main encoder = selected track volume, encoders = track volumes | Cursor note velocity, main encoder or encoder 1, 1–127 |
| SWING (Position) | Hold: main encoder = project swing | Cursor note microtiming, main encoder or encoder 1 |
| TEMPO (Tune) | Hold: main encoder = BPM (Shift fine 0.1) | Selected native sample/synth tuning, ±48 semitones; encoder 2 or Shift + main = cents |
| NOTE REPEAT (Arp) | Hold: pads repeat at the rate chosen on the display buttons; encoders Rate, Mode, Octaves, Gate; tap latches | **Arp page**: display buttons pick the mode (Repeat, Up, Down, Up-down, Random, As played) and turn repeat on/off; encoders Mode, Rate, Octaves (1–4, arp modes only), Gate (10–100 % of the rate); the 4-D encoder walks the modes; push, Back or Shift + NOTE REPEAT closes |
| LOCK (Exit) | Hold + turn encoders: write the values as step locks at the cursor step | Exit the lock page, preserving written locks |
| PITCH | Touch strip = live pitch bend for the selected track's instrument (native `midi.expression`, ±2 semitones on built-in instruments, springs back). In automation write mode it records the track pitch lane instead | Touch strip = transpose selection (±12) |
| MOD | Touch strip = modulation wheel: CC1 to plug-ins and external MIDI; built-in instruments close their filter (engine mod wheel, up to −5.25 octaves); springs back | Touch strip = cutoff of the selected track |
| PERFORM (FX Select) | Touch strip = performance filter/stutter on the master (springs back) | Choose the performance effect |

### Pad section

| Button | Function | Shift |
|---|---|---|
| PAD MODE | Cursor track in a track group: pads = the group's tracks in track order (pad 1 = first track; more than 16: ◄ ► pages). A pad moves the cursor to that track, auditions it at its pad note (C-4 unless set) and, while recording, enters that note on that track at the cursor row; bright = cursor track, dim = other tracks. Outside a group: pads = instruments 1–16 (each pad plays its instrument at its root note and records into the cursor track) | Pads = slices of the selected sample instrument |
| KEYBOARD | Pads = chromatic/scale keyboard of the selected instrument (uses Key/Scale; ◄ ► shift octave). Pressed again while lit: the key & scale page (root, scale, pad layout, octave); KEYBOARD again closes it | Choose root/scale on the screens |
| CHORDS | Pads = the 16 chords of the active chord set (Maschine-style) | Choose chord set / chord mode on the screens |
| STEP | Pads = 16-step sequencer for the selected track (rows of the current 16-row page; ◄ ► page). Held pad + encoders = edit that step (note, velocity, gate, FX). With FOLLOW on, the page follows the playhead | Plain STEP |
| FIXED VEL | Toggle fixed velocity (value set on the screens) | **16 Velocities**: pads play the cursor track's note (the last one played on it, else its pad note or the instrument root) at velocities 8, 16 … 127, pad 1 softest; the pads light from dim to full in the track colour and FIXED VEL is lit; ◄ ► move the note a semitone; recording enters the pad's velocity. Shift + FIXED VEL again (or any pad mode button) returns to the previous pad mode. The velocity curve is on SETTINGS |
| SCENE | Pads = song blocks. Stopped: select the block. Playing: queue it (Song playback) or its pattern (Pattern playback) for the end of the current pattern; the queued pad blinks, pressing it again cancels | Loop the pressed blocks |
| PATTERN | Pads = patterns 1–16. Stopped or Song playback: select the pattern. Pattern playback: queue it for the end of the playing pattern (blinks until it starts; again = cancel). The screens show the PATTERN page (Pattern, Length, Lines/beat, Colour on the encoders); PATTERN again returns to the previous pad mode | Create a new pattern |
| EVENTS | Hold + pads/encoders: select steps and nudge, transpose, change velocity and length of the selection | Quantize / reset microtiming |
| VARIATION (Navigate) | Humanize velocities of the selection or pattern; encoders choose amount; Randomize remains on its display button | Open tracker navigation: 4-D encoder moves rows, columns and tracks |
| DUPLICATE (Double) | Hold + group = duplicate the track (a group: its selected track); hold + pad (Pattern mode) = duplicate pattern | Double the pattern length |
| SELECT | Hold + pad/group = select the group's track / instrument / track without playing it | Select all steps |
| SOLO | Hold + group = solo the track or the group bus; hold + pad = solo the group's track (outside a group: the instrument's tracks) | Clear all solos (tracks and group buses) |
| MUTE (Choke) | Hold + group = mute the track or the group bus; hold + pad = mute the group's track (outside a group: the instrument's tracks) | Clear all mutes (tracks and group buses) |

### Remaining custom Shift mappings

The October 11 layer review corrects Save, Velocity, Position, Tune, Exit and Navigate.
The app still uses custom actions for **Shift + Mute** (clear mutes rather than Choke), and **Shift + Erase**
(clear track pattern rather than Replace). Scene/Section uses SuperTracker song blocks
and block loops. These are not complete reproductions of the corresponding Maschine modes.
They remain explicit in the control map; do not assume the printed secondary legend applies.

Velocity and Position edit the note at the cursor (one step per track and row). Empty steps
show unavailable feedback. Tune is available only for native
sample/synth sources; AU and MIDI tuning stays with the instrument itself. Exit a value
page with the main encoder or its Back display button. Touch-strip mode changes end the
previous pitch/modulation/performance gesture and reset its original destination.

### Shift + pad legends

Pad 1 is the **bottom-left** pad, with numbering left to right and then upwards.
The shortcuts follow the printed MK3 legends, verified against Native Instruments'
[hardware layout](https://docs.native-instruments.com/ni-tech-manuals/maschine-accessibility/en/maschine-hardware-layout).

| Left | | | Right |
|---|---|---|---|
| 13 · Semitone − | 14 · Semitone + | 15 · Octave − | 16 · Octave + |
| 9 · Clear | 10 · Clear Auto | 11 · Copy | 12 · Paste |
| 5 · Quantize | 6 · Quantize 50% | 7 · Nudge ← | 8 · Nudge → |
| 1 · Undo | 2 · Redo | 3 · Step Undo | 4 · Step Redo |

Undo/Redo and Step Undo/Redo both use SuperTracker's existing edit history; it does
not maintain NI's separate take-level history. The screens name the step that was undone or redone
(*Undo · clear track pattern*), or say *Nothing to undo*. Clear, Copy, Paste and transpose use
the Pattern selection, or the cursor cell when there is no selection. Quantize removes
microtiming; Quantize 50% halves it. Nudge moves events earlier/later in the vertical
tracker. In Scene mode, Shift + pads retains its explicit song-block loop operation.
Quantize affects the cursor track's steps in the selected rows; other tracks (also the other
tracks of a group) keep their timing. Clear Auto clears the current Pattern's automation of the
cursor track. A no-op displays the reason and creates no Undo entry.
Fill remains on Shift + the Notes screen's Paste button; Select All remains Shift + SELECT.

### Groups A–H

The group buttons show **entries** in track order, eight per bank: each ungrouped track is one entry and each track
group (kit) is one entry for all its tracks. Shift + ◄ ► page the banks; the bank follows the entry of the cursor
track. Press: a track entry selects the track; a group entry selects its track selected last (if it is still in
the group), else its first track. Colour = track colour or the group's colour; bright = holds the cursor track,
dim = other entries, off = muted (a group: its bus muted, or another entry soloed), pulsing = recording into it.
MUTE / SOLO held + group = mute / solo the group bus (one Undo step) or the track. ERASE held + group = clear the
current pattern of the track, or of every track of the group in one Undo step. SELECT / DUPLICATE held + group
act on the track (a group: its selected track). Shift + group = arm/disarm recording on it.

### Touch strip

Its LEDs show the playhead position inside the pattern (or the loop range while Restart is held), or the value
being controlled in Pitch / Mod / Perform modes. Swiping in the default state scrubs the cursor row.

### Transport

| Button | Press | Shift |
|---|---|---|
| RESTART (Loop) | Restart playback from the pattern/song start | Toggle pattern loop range (selection or current bar) |
| ERASE (Replace) | Hold + pad = erase that instrument's notes in the pattern (PAD MODE in a group: that track's notes); hold + group = clear the track / group pattern; alone = clear cursor step | Clear the whole track pattern |
| TAP (Metro) | Tap tempo | Metronome on/off |
| FOLLOW (Grid) | Follow playhead on/off (also makes the STEP page follow) | Edit step (1, 2, 4, 8, 16) on the screens |
| PLAY | Play / pause (Pattern or Song as set in the app) | Toggle Song / Pattern playback mode |
| REC (Count-in) | Arm note recording (during a count-in: cancel it) | Count-in recording: arm, one bar of engine clicks, then Play and live recording start on the downbeat (while playing: just arm) |
| STOP | Stop | Panic (all notes off) |
| SHIFT | Modifier, lit while held | – |

### Analog knobs and pedal

The master, headphone and microphone knobs belong to the MK3's audio interface and are shown on the settings page
only. The pedal jack acts as sustain for pads and keyboard: the host defers the release of every note the app plays
until the pedal comes up, whatever the instrument (the pedal itself is not passed on to plug-ins or external MIDI).
Pad pressure sends channel aftertouch (default) or poly aftertouch with the pad's note to the selected track's
instrument (a plug-in, or an external MIDI instrument on its channel); choose Off / Channel / Poly on SETTINGS
(encoder 8, "Pressure"). Built-in instruments ignore aftertouch.

## Software contract (native driver ⇄ interface)

Native → interface event `mk3.input` (payload):
- `{type:"button", name, pressed}` — names: `channel plugin arranger mixer browser sampling arrowLeft arrowRight
  file settings auto macro display1…display8 notes volume swing tempo noteRepeat lock pitch mod perform padMode
  keyboard chords step fixedVel scene pattern events variation duplicate select solo mute groupA…groupH restart
  erase tap follow play rec stop shift encoderPush encoderUp encoderDown encoderLeft encoderRight`
- `{type:"pad", pad:1…16 (1 = bottom-left, Maschine numbering), phase:"down"|"pressure"|"up", pressure:0…1,
  velocity:1…127 (down only)}`
- `{type:"encoder", index:0…7, delta}` — delta in revolutions-ish units: ±1.0 ≈ full sweep of a parameter
- `{type:"main", delta:±n}` — detents of the 4-D encoder
- `{type:"touch", target:"encoder1"…"encoder8"|"main"|"strip", touched}`
- `{type:"strip", value:0…1}`; `{type:"knob", name:"master"|"headphone"|"mic", value:0…1}`;
  `{type:"pedal", down}`
Native → interface event `mk3.status`: `{connected, screens, exclusive, error?}`.

Interface → native commands:
- `mk3.leds {buttons:{name: 0…1}, groups:[8 × "#rrggbb"|null], pads:[16 × "#rrggbb"|null] (index 0 = pad 1),
  strip:[25 × 0…1], nav:{up,down,left,right: "#rrggbb"|null}}` — partial objects update only what they contain.
- `mk3.screen {left:[ops], right:[ops]}` — display list per 480 × 272 screen. Ops:
  `{op:"clear", color}`, `{op:"rect", x,y,w,h, color, radius?}`, `{op:"frame", x,y,w,h, color, width?}`,
  `{op:"line", x1,y1,x2,y2, color, width?}`, `{op:"text", x,y, text, size, color, weight?:"regular"|"bold",
  align?:"left"|"center"|"right", width?, mono?}`, `{op:"bar", x,y,w,h, value:0…1, color, background}`,
  `{op:"arc", x,y, r, value:0…1, color, background, width?}` (knob ring). Native renders, diffs and sends only
  changed regions, at most ~30 frames per second.
- `mk3.connect {exclusive?:bool}`, `mk3.disconnect`, `mk3.status`.

Interface → audio host commands used by the controller (also available to the rest of the app):
- `midi.expression {owner, kind:"pitchBend"|"cc"|"aftertouch"|"polyAftertouch"|"sustain", value, controller?, note?}` —
  live expression for the track's instrument through the same destinations as hardware MIDI input: a plug-in gets the
  message with its next block (as MIDI, or in its format's own terms), an external MIDI instrument gets it on its
  device and channel, built-in instruments get pitch bend (±2 semitones) and CC1 (the mod wheel, filter close) and
  ignore the rest (the reply's `sent` is then false). `value` is −1…1 for pitch bend, 0…1 otherwise; `sustain` (or
  CC64) holds the app's own notes in the host. The controller drops repeated identical values, quantizes to
  MIDI resolution and sends at most one message per stream every 15 ms with a trailing latest value.
- `transport.queue {pattern}|{sequence}|{clear:true}` — queued jump honoured by the engine at the next pattern (or
  pattern-loop-range) end, sample-accurately (`queue_jump`). Ignored while stopped (`queued:false`).
  Status carries `queuedPattern`/`queuedSequence` (−1 = none). The song arranger also offers it: **Q** / "Play from
  here at pattern end" while the song plays.
- `transport.play {…, countIn:bars}` — the engine clicks `bars` bars at the start position's tempo/meter (accented
  downbeats, metronome volume, even with the metronome off), then starts Play exactly one bar later
  (`play_count_in`). Status carries `countIn` (clicks left). The app's `ST.recordWithCountIn()` and the
  palette command **Record with count-in** arm note recording first; notes played during the count-in sound but are
  not step-entered, acknowledged input after the downbeat records live.

### Track groups (kits)

A kit is a track group ([KITS.md](KITS.md)): ordinary member tracks, one per loaded pad, routed to the
group's bus. The MK3 follows Maschine's model: group buttons A–H choose tracks and groups, PAD MODE pads choose and
play the tracks of the cursor's group, and CHANNEL / PLUG-IN add a **Group** level for the group bus. Steps are
plain tracker cells: each member track has its own column with Note, Ins, Vel, FX 1, FX 2 and Gate, and every
editing gesture (encoders, STEP, EVENTS, LOCK, Shift + pads) acts on the cursor track as for any track.

The Gate encoder is measured in pattern rows (0.01–256), not seconds; the display labels it **rows**. Turned fully down it shows **— none**: the note has no gate and plays until the next note on its track, an OFF or a CUT (new notes start that way). A gate controls note duration before note-off; the instrument’s envelope/release and sample playback mode determine how long the resulting sound remains audible.
