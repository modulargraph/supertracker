# Keyboard navigation: blocks

Every page and dialog is made of **blocks**: regions such as the browser's filter column, the results list or the song
settings. One shared module, `ui/block-nav.js` (`window.STBlockNav`), moves focus between them; the blocks
of each page are declared in `ui/pane-navigation.js`. Frame and hint styles are in `block-nav.css`.

## Keys

| Keys | Where | What |
|---|---|---|
| ⌥⌘ ← → ↑ ↓ | everywhere, dialogs included | Move to the block in that direction (by on-screen position). |
| ⌥ ← → ↑ ↓ | audited pages: Pattern, Instrument, Mixer, Browser, Song, Recording, Automation, Settings dialog | Same, unless the focused control uses ⌥ + that arrow itself and you have used it (see below). The pattern grid always keeps them. |
| ⌘0 | anywhere but dialogs | Back to the pattern grid at its cursor cell (switches to Pattern from other pages). |
| F6 / ⇧F6 | pages (not dialogs) | Next / previous block in reading order. |
| Tab / ⇧Tab, arrows | inside a block | Next / previous control. Arrows on a button, checkbox or tab move to the nearest control in that direction inside the block, unless the control's own arrows apply (lists, tab strips, fields). |
| Esc | inside a block (audited pages) | Back out to the block: the block itself is focused and the frame thickens. Not in the pattern grid (see Pattern below). |
| Esc | at block level | What it always did (stop, close a menu, stop a preview). While the transport plays, the first Esc already stops. |
| Return, or an arrow | at block level | Go back into the block (its first control). |
| Esc | dialogs | Closes the dialog at once (no back-out step). Tab and ⇧Tab stay inside the dialog. |
| ← → | Instrument: on a slider or menu | Previous / next control of the block in reading order (value fields skipped; they are for typing). Stops at the ends. |
| ↑ ↓ | Instrument and Mixer: on a slider or knob | Adjust (⇧ coarse; ⌥ fine while adjusting; insert-chain knobs: ⇧ or ⌥ fine). |
| Return | Instrument: on a slider | Type a value: its value field is focused and selected. Return there commits and goes back to the slider. |

Moves prefer blocks in the same row (← →) or column (↑ ↓) and the one nearest the focused control; moving straight
back returns to the block you came from. With nothing ahead, the move wraps to the far end of the same row or column;
a full-width bar (transport, views) has nothing beside it and stays put (the frame pulses). Blocks wholly diagonal count
when nothing shares the row or column.

The focused block gets a two-ring frame (amber inside, dark outside, visible on light and dark backgrounds) with its
label on a tab, and a hint line at the bottom with the keys that work inside it. While the frame shows, the focused control
is marked `.block-nav-focus` and gets an amber ring (a slider's label and thumb turn amber too), so focus is visible on
sliders, knobs, canvases and value fields even after a programmatic focus. The frame appears with keyboard
navigation (⌥ arrows, F6, Tab, Esc) and hides on a mouse click; a click still sets the page's current block.
Each page (and dialog kind) remembers its last block, set by focus or a click: ⌥ + arrows continue from it when focus is lost (a re-render, a click on empty space) and the frame shows it. ⌘1–⌘7 keep their documented entry control (`NativeUIAcceptanceTests`); `STBlockNav.restore(view, id)` is there for pages that want to return to the remembered block instead.

### ⌥ + arrow conflicts

Plain ⌥ + arrows go to the focused control first when it **owns** them, but only once you have used it (pressed a
key in it or clicked it). Arriving somewhere by keyboard (a block move, Tab, a view switch, ← → between sliders) does
not count as use, so repeated ⌥ + arrows never get stuck. Focus that a control moves with its own key (← → on insert
tiles re-render the strip and focus the next tile) keeps counting as use. ⌥⌘ + arrows always move between blocks;
⌥⇧ + arrows are never block moves.

Sliders, knobs and value fields are stricter: they own ⌥ ↑ ↓ (fine adjustment) only **while adjusting**, i.e. the last
key pressed in them was ↑ ↓ / Page Up / Page Down or you clicked or dragged them. After typing a value, Return, or
arriving with ← →, ⌥ ↑ ↓ move between blocks again. They never own ⌥ ← →.

| Owner | Axis | Meaning |
|---|---|---|
| Text fields with text | ← → | caret by word |
| Textareas, contenteditable | all | caret by word / paragraph |
| Library search (`#browser-search`) | ↑ ↓ | recent searches |
| Song sequence (`#sequence-list`, not its buttons) | ↑ ↓ | move blocks |
| Clip timeline, clips | all | slip audio, move clip to another track |
| Automation envelopes (pattern and song) | all | previous / next point, move point |
| Pattern grid (`#tracker`, not its header row), and the Pattern page with nothing focused | all, **always** (also on arrival) | ⌥↑↓ transpose, ⌥←→ cursor |
| Insert tiles (`[data-fx-tile]`, the tile strip only) | ← → | reorder effects |
| Older mixer rack headers (`.studio-plugin > summary`) | ↑ ↓ | move insert |
| Plug-in slot tabs (`[data-pp-tab]`) | ← → | switch plug-in (on the knobs ⌥ ← → move between blocks) |
| Sliders, knobs, value fields (`.parameter`, `.mod-parameter`, NKS, plug-in pages, insert knobs, plug-in parameters) | ↑ ↓, while adjusting | fine adjustment |
| Sample waveform | ← → | nudge marker one frame |
| Any element with `data-alt-arrows="horizontal|vertical|both"` | as declared | |

MK3 has no ⌥ + arrow mapping. Octave keys (`[` `]`), ⌥1–5 (instrument pages) and ⌥ letter commands are unaffected.

## API

```js
// Blocks of a page ('pattern'), of every page ('*'), or of a dialog ('modal:<kind>' — state.modal).
const remove = STBlockNav.register('pattern', [
  { id: 'pattern-grid', label: 'Pattern grid', selector: '#tracker', keys: '↑↓←→: move · Enter: audition' },
  { id: 'step-editor', label: 'Step editor', selector: '.step-editor', prefer: ['#step-note'] },
  // roots() instead of selector for regions without one container; several roots form one block.
  { id: 'toolbar', label: 'Pattern settings', roots: () => [$('#pattern-length').closest('.toolbar')] }
], { altArrows: true, escape: true }); // after auditing ⌥ + arrow owners on the page

STBlockNav.claimAltArrows('.my-reorderable-list', 'vertical');   // or markup: data-alt-arrows="vertical"
```

* A block spec: `id`, `label`, `selector` (all matches) or `roots()`, `prefer` (selectors for the control focused on
  entry, first match wins; otherwise the root if it is a control, otherwise its first focusable control), `keys`
  (hint text), `arrows: false` (plain arrows on its buttons do nothing, as the clip toolbar requires). A block without a focusable control is skipped. Array order is the F6 reading order; a function may be
  passed instead of an array to build blocks at the time of the move.
* Markup alternative: any element with `data-nav-block="id"` (plus `data-nav-label`, `data-nav-keys`,
  `data-nav-entry`) inside `#app`, or inside `.modal` for dialogs, is a block. Nested blocks work; the innermost wins.
* A spec may also give `esc` (hint text) when its controls use Esc themselves (insert knobs: Esc goes to the modules).
* Options: `altArrows` (plain ⌥ + arrows; also enables Tab showing the frame and arrows between buttons),
  `escape` (Esc backs out to block level), `f6` (default true), `sliders` (Instrument: ← → step through sliders and
  menus, Return types a value), `owns(target, key)` (the page's strict ⌥ + arrow owners: they keep them even on
  arrival; the pattern grid).
* More spec fields (Pattern): `stops` (a selector: plain arrows, Tab / ⇧Tab, Home / End and Return on these buttons
  are the block's own, handled in the capture phase before the widget around them; stops may have tabindex -1),
  `rect()` (geometry instead of the roots' box, e.g. the visible part of a scrolled grid), `anchor()` / `reveal()`
  (the node a move is measured from / brought into view on arrival: the cursor cell), `ownKeys` (the block keeps Tab
  and ⌥ + arrows; its hint says ⌥⌘ arrows or F6). Dialogs without a registration get title / body /
  buttons blocks and ⌥⌘ + arrows.
* Queries and actions: `blocks()`, `blockOf(node)`, `current()`, `remembered(scope)`, `move(direction)`,
  `next(±1)`, `focusBlock(id)`, `backOut()`, `enter()`, `owns(node, key)`, `pick(rect, candidates, direction)`
  (pure geometry, used by the tests).
* Hooks in the app: a capture-phase `keydown` listener (⌥ / ⌥⌘ arrows, Return / arrows at block level),
  `STBlockNav.key` (F6, via `STPaneNavigation`), `STBlockNav.escape` (in the Esc branch of `app-keyboard.js`, after
  the page's own Esc handling), `STBlockNav.arrow` (last in `keyboard()`).
  Palette commands `blockLeft` / `blockRight` / `blockUp` / `blockDown` (⌥⌘ arrows) call `move`.

## Pages

| Page | Blocks (reading order) |
|---|---|
| All pages | Transport, Views, Sidebar (Patterns / Instruments / Recording) |
| Browser | Tabs · Search · Products (Kits & NKS sidebar) · Filters (sample / plug-in filter column, NKS tag filters) · Result options · Results · Result pages · Details |
| Song | Song actions · Pattern sequence · Selected song block · Song settings · Audio clip actions · Audio edit cursor · Audio timeline · Clip inspector |
| Recording | Recording details · Input device · Input levels · Playback & monitoring · Capture · Audio interface · Input routing · After capture |
| Automation | Pattern / song scope · Parameters · Lanes · Lane settings · Envelope (pattern or song) · Points |
| Settings dialog | Audio devices · MIDI routing · AI assistant · Apply or cancel |
| Command palette | Search · Commands (Esc closes) |
| Instrument | Instrument actions · Instrument type · Instrument pages · then the visible page (below) · Plug-in instrument · Plug-in slots (page menu and slot tabs) · Plug-in controls · NKS controls · Instrument effects (tile strip) · Selected effect (knob page) · Play & perform. A panel not named here is one block. |
| Instrument · Sample | Wavetable or Granular panel (in those modes) · Sample (file, audition) · Play mode · Waveform (strip and canvas) · Region edits & view · Volume · Pan · Tune · Filter · Drive · Amp envelope · Region · Root note |
| Instrument · Slices / Zones / Modulation / Voice & Record | Slice detection · Slices / Zones / Mod envelope (filter) · LFO 1 · LFO 2 / Voices · Record into this sampler |
| Pattern | Pattern actions · Selected track (note destination bar) · Pattern settings (length, step, Follow, Fill, Gate, Hide empty, Pad keys) · Keyboard performance (Play chip and its controls) · Track headers · Pattern grid · Step editor |
| Mixer | Mixer actions · Mixer channels (one roving block: ← → channel, same control) · Channel / detail divider (↑ ↓ resize) · Insert effects (tile strip) · Selected insert (knob page) · Search plug-ins · Plug-in details · Channel routing · Send returns |

Notes on the layouts: the waveform is as wide as the page, so ⌥ ← → from it wrap to the sidebar; ⌥ ↓ / ↑ walk the Sample
page top to bottom. The plug-in console's page menu sits above its slot tabs and shares their block, so the knob block
below is a plain rectangle. Without inserts the Mixer has no Selected insert block; Master has no insert chain.

### Pattern

The grid keeps every key it had: arrows, Tab / ⇧Tab (track), ⌥↑↓ (transpose), ⌥←→ (cursor, as before), ⌘⇧←→ (move
track), typed entry, Return, Delete and Esc (first cancels a half-typed value, then stops; there is no back-out step
in the grid, so a single Esc still stops playback). It is a strict ⌥ + arrow owner (`owns` option), also right after
arriving and when nothing is focused (keys then go to the grid). **Leave the grid with ⌥⌘ + arrows or F6**;
⌃Tab still jumps to the step editor. Everywhere else on the page plain ⌥ + arrows move between blocks.

**Back to the grid: ⌘0** (palette: “Go to the pattern grid”), from any block or page; dialogs excepted. ⌘0 had no
binding (no menu item or key handler) and continues ⌘1–⌘7. Esc at block level keeps its documented meaning
(stop), and ⌥⌘G was not used because ⌘G / ⌘⇧G / ⌥G are group and chord-set keys. Return / Esc in the step editor
fields and the Selected-track menus also return to the grid, as before. Arriving in the grid (⌥ / ⌥⌘ arrows, F6, ⌘0)
focuses `#tracker` and scrolls its **cursor cell** into view; while the frame shows, the cursor cell gets the amber
ring. Moves from the grid measure from the cursor cell. The grid block is its visible rows (below the sticky header,
clipped to the scroll box), the header block the header rows.

**Track headers** are one block inside the grid table. The cursor track's buttons (M, S, ◂ collapse, × delete,
instrument) and the cursor's group header (▾, M, S) are in the Tab order; every other track's and group's buttons
(also × group delete and the hidden-tracks chip) are reached with arrows: ← → ↑ ↓ go to the nearest header button in
that direction, Home / End to the first / last, Tab / ⇧Tab to the next / previous. Return or Space presses (not
audition). Arrows there never move the grid cursor. Esc backs out to the header block. Header buttons have stable
ids (`track-<i>-mute|solo|source|collapse|delete`, `group-<id>-collapse|mute|solo|delete|reveal`), so pressing one
keeps keyboard focus on it after the grid re-renders. Header buttons do not own ⌥ + arrows.

## Tests

* `tests/block_navigation_tests.js`: geometry (rows / columns, anchor, way back, wrap, edges), remembered blocks,
  Esc / Return semantics, conflict rules and pass-through, dialogs and the focus trap, and the block layout of the
  Browser (all four tabs), Song, Recording, Automation and Settings.
* `tests/block_navigation_editor_tests.js`: Instrument (every sampler page, granular, plug-in and NKS consoles,
  instrument effects) and Mixer layouts, ⌥ moves, the adjusting rule, slider ← → and Return, waveform and divider keys,
  tile reorder after a re-render, focus marking.
* `tests/block_navigation_pattern_tests.js`: Pattern layout (real 1500×940 geometry), ⌥ / ⌥⌘ moves, grid keys
  unchanged (arrows, Tab, transpose, Esc), header button reachability and Return, cursor-cell focus restore, ⌘0.
* `tests/pane_navigation_tests.js`: F6 order and robustness, unchanged.
* `tests/helpers/fake-dom.js`: a small DOM with layout boxes for such tests.
