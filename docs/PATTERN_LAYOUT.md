# Pattern view layout: lane headers, destination strip and toolbar

Status: implemented (ui: `app-pattern-columns.js` `trackSource` / `trackHeader`, `app-pattern-view.js` `renderPattern`, `app-pattern-bind.js` `bindPattern`,
`styles.css` "Pattern header" block, `pattern-destination.css`).

## Why

User feedback: "I like seeing the instrument and lane thingy, but I'm not sure about the layout."
The content was right (which lane, which sound) but it was scattered and unevenly weighted.

## Audit (before)

| Problem | Where |
| --- | --- |
| **The column header never said what a lane plays.** It showed a 5px colour dot, the track name, M/S and the lane number (at the far right, after M/S). The instrument appeared only in the destination strip, for the *selected* lane. Scanning eight lanes for "which one is the bass sampler?" meant clicking each one. | `th[data-track]` |
| **Lane identity was split across both ends.** Number at the right, colour dot at the left, name in between. The number, the most stable identifier in a tracker, had the least weight. | track header |
| **Kit lanes looked like tracker lanes.** Only the `SND` column label and a disabled S told them apart. Nothing showed the kit's current Sound in the grid. | kit lane header |
| **Destination strip used three rows for two choices.** Labels above full-width selects, then the help line: about 80px before the grid even started. | `.pattern-destination` |
| **The Play toolbar was the densest part of the view.** Oct, Key, Scale, Snap, Chord, Set, MIDI, Repeat, rate and Vel (11 controls) were always visible although most change rarely and all have ⌥ shortcuts. At 1080px it wrapped to three lines and pushed the grid about 55px down. | `.perf-group` |
| **Weak grouping.** Pattern settings and keyboard-performance settings sat on one line, separated only by a small amber "Play" pseudo-label. | `.pattern-toolbar` |
| **Duplicate read-out.** The performance summary was in the footer, far away from the controls that set it. | `.pattern-foot` |
| **Loud M/S.** Full-contrast bordered M/S buttons on every lane competed with the name. | `.track-ms` |

## Design

### Lane header: three fixed rows, same for every lane type

```
┌ lane colour stripe (2px, 3px when selected) ─────┐
│ 01  Velvet bass                           [M][S] │  identity: number (lane colour) · name · compact M/S
│ [SYN] 00  Velvet bass                            │  sound source: type badge · slot · name (button)
│ NOTE INS VEL  FX 1  FX 2                         │  column labels, same grid as .tracker-cell
└──────────────────────────────────────────────────┘
```

- **Identity**: the lane number comes first and takes the lane colour, followed by the name (12px, medium weight).
  The colour also runs as a stripe along the top edge of the header, so lanes can be told apart at a glance
  even when names are truncated. M/S are smaller and quieter until they are on.
- **Sound source**: a button showing a type badge (`SYN` synth, `SMP` sampler, `PLG` plug-in, `DRM` drum synth,
  `MIDI` external), the instrument slot and the instrument name. Kit pads are ordinary tracks, so they show
  their own sampler. Clicking it selects the lane and focuses the destination strip's *Assigned instrument*
  select, so you change a lane's sound where you see it. The keyboard path is unchanged: F6 → Note destination →
  Tab to the select. The header buttons keep `tabindex="-1"` so Tab does not stop on every lane (the grid stays a
  single tab stop; see DESIGN.md).
- **Consistent height**: every lane renders the same three rows, so the sticky header is 68px high. A track
  group (kit) adds one group header row above its tracks (`groups.css`); collapsed, the group is one overview column.
- **Column labels** keep the shared grid template (`.tracker-cell` / `.track-columns`), so each label still
  sits over its column.

### Destination strip: one row plus help

Labels now sit beside their selects ("Selected lane" and "Assigned instrument"),
followed by *Arm & enter notes* and *Play pattern*. The contextual help line stays below. This saves about 25px.
All ids, `laneLabel` text and help strings are unchanged.

### Toolbar: Pattern settings | Play chip

- **Pattern** group (caption `PATTERN`): Length, Step, Follow, Fill, Gate. These change pattern data or editing
  behaviour and stay visible.
- **Gate** (`#gate-column`, `aria-pressed`) shows or hides the optional GATE column (`app.gateColumnShown`,
  stored as `project.viewState.gateColumn`: saved with the session, written into Undo/Redo snapshots, never an
  Undo step). Hidden by default: notes play until the next note, OFF or CUT on their track, so the column
  labels and cells are `NOTE INS VEL FX 1 FX 2` (`table.tracker[data-gate-column="hidden"]` drops the sixth grid
  column). Shown, each cell adds the gate in rows, or `—` for none, and the step editor's **Gate (rows)** field
  becomes editable (empty = none); hidden, the step editor shows the gate read-only. The step editor also has
  **OFF** / **CUT** buttons, and the footer hint reads `1 OFF · ⇧1 CUT`.
- FX cells show effect codes (`R04`, `Y50`, `P-30`; table in `app-entry.js` `FX_CODES`, documented in USER_GUIDE
  "Typing values") with the effect name and units as a tooltip. INS / VEL / FX columns take typed values
  (`app.gridEntryKey`, before the keyboard piano); a half-typed value shows in its cell with an outline. The step
  editor's Ins is a typed input with a datalist; each FX has a code box (`#step-fx1-code`) beside its picker button.
- **Play** group: a single chip, `Play C Chromatic · vel 100 · oct 4 ›`, that holds `#perf-summary` (moved from the
  footer, so the read-out sits beside its controls). Clicking the chip (`#perf-toggle`, `aria-expanded`) expands
  Oct, Key, Scale, Snap/Degrees, Chord, Set, MIDI, Repeat, rate and Vel inline. When expanded the chip shows just
  `Play ‹`. The chip gets an amber border when key, scale, chord or set differ from the defaults, so a
  non-chromatic setup is still visible while collapsed.
- The expanded/collapsed choice is a per-viewer convenience kept in `localStorage` (`st.pattern.perfOpen`, wrapped
  in try/catch). The default is collapsed.
- Collapsed controls stay in the DOM (`hidden`), so every ⌥ shortcut, palette command, agent API path and test
  that reads them works unchanged. The chip summary updates live through the existing
  `updateTransport → #perf-summary` path.

## What did not change

Every id (`pattern-length`, `edit-step`, `follow`, `fill`, `octave`, `root`, `scale`, `scale-mode`, `chord`,
`chord-set`, `midi-apply`, `note-repeat`, `note-repeat-rate`, `key-velocity`, `perf-summary`, all
`pattern-destination-*`), the `th[data-track]` click / double-click rename / right-click menu, `data-track-toggle`
M/S buttons, keyboard shortcuts, pane navigation sections (`.workspace-main > .toolbar`, `.pattern-destination`)
and the step editor. Double-clicking the sound button does not trigger rename.

## Results

- 1500×940: the grid starts at the same height as before, but every lane now shows its sound source, and the
  toolbar fits on one line.
- 1080×700: the grid starts about 55px higher (two more visible rows) because the toolbar no longer wraps to three
  lines.
- Test: `tests/pattern_destination_tests.js` covers the header markup (identity, source badge) and the
  Play chip expand/collapse.
