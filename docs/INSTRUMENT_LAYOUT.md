# Instrument view layout: pages for Synth and Sampler

Status: implemented (ui: `instrument-navigation.js`, `app-instrument-view.js` `renderInstrument` / `renderSoundGroups` / `renderInstrumentPages`, `sampler.js`, `sampler.css`, `styles.css`, `pane-navigation.js`).

## Why

User feedback: "the interface is really great, but all the options for samples start being a bit cluttered."
The aim was to keep every feature and reorganise it in the spirit of the Polyend Tracker (one focused
instrument page at a time) and Maschine MK3 (Sample editing split into Edit / Slice / Zone / Record pages),
staying keyboard-first.

## Audit of the old Sampler view (before)

The Sampler was one long scrolling column: more than 3,000px, about four and a half screens at 1500×940.

| Problem | Where |
| --- | --- |
| **Two waveforms.** A static SVG overview at the top and the real zoomable editor canvas about 1.5 screens lower. | "Sample" panel, then "Sample editor" |
| **Play mode chosen in two places.** A `<select>` in "Play mode & loop" and a 7-button radio group in the editor, plus the `M` key. | "Play mode & loop", "Sample editor" |
| **Start/End/Loop edited in three places.** Sliders in "Play mode & loop", marker chips with the canvas in the editor, and click/⇧/⌥-click on the SVG overview. | three panels |
| **Two audition buttons.** "▶ Audition" (600 ms note) in the overview, and "Hold to audition" in the editor. | overview, editor |
| **The most-used controls were split up.** Waveform, then mode/region, then level, filter, amp, drive, modulation, and only then the editor, so the editor was below the fold. | whole page |
| **Rarely used things were shown first.** Record, Voice behaviour and the 16-option choke list came before slicing and zones. Drive/Bit depth had its own half-width panel. | middle of page |
| **Always-visible help text.** About 9 help paragraphs (editor keys, slice play rules, beat-grid note, zones, voice...) took more height than the controls. | throughout |
| **Mixed section styles.** `inst-group` panels used 13px headings, filled chips and 15px padding. Sampler panels used flat 11px headings, chips with no background, and a different surface colour. "Record a sample" had no header at all. | sampler.css vs styles.css |
| **Lopsided grid.** "Slice map" was half width with an empty column beside it. Its marker list was capped at 170px inside a page that already scrolled. | Slice map |
| **Duplicate heading.** "Filter envelope · LFOs" (panel) directly over "Modulation" (console header). | Modulation |
| **Long tab chain and F6 path.** F6 visited about 12 instrument sections, and Tab walked every slider and number pair across all of them. | keyboard |

Plug-in and External MIDI pages were already short (overview, NKS controls, Play & perform). They only got the shared section style.

## New structure (after)

The page tabs sit in the existing instrument toolbar, beside the type switch, so they take no extra vertical space:

```
[Synth | Sampler | Plug-in | External MIDI] │ [1 Sample] [2 Slices 16] [3 Zones 2] [4 Modulation] [5 Voice & Record]      ⌥A Automate…
```

Counts show the number of slices (in Slice mode) and the number of zones.

### Sampler: 5 pages

| ⌥ | Page | Contents |
| --- | --- | --- |
| 1 | **Sample** (the main page) | **Sample editor**: the one waveform. Its header shows file name, channels, rate, length and frames, plus Show in Finder and Hold to audition. Below the header: the play-mode radio group with its help line, marker chips with the view readout, the canvas, then one tools row (Trim, Fades and curve, Normalize, Reverse, Gain, Stretch…, Add slice, Slices → pattern, and view zoom/snap). The waveform key reference is folded into a "Waveform keys" disclosure. Under the editor sit three columns: **Volume · Pan · Tune**, **Filter · Drive** (cutoff, resonance, drive, bit depth), and **Amp envelope** (compact graph plus ADSR). Then **Region · Root note**: the automatable numeric root/start/end/loop/slices/grain fields, with the current play mode as a chip. |
| 2 | **Slices** | **Slice map**, now full width in two columns. Left: Detect transients (with Detection settings), Chop evenly (equal slices / add marker / beat grid), and a "How slices play" disclosure. Right: slice ruler, audition status, and a much taller marker list. |
| 3 | **Zones** | **Multisample zones**, full width. |
| 4 | **Modulation** | **Filter envelope · LFOs** (the duplicate inner heading is hidden). |
| 5 | **Voice & Record** | **Voice behavior** (voice mode, choke group) and **Record a sample**, which now has a proper header. |

### Synth: 3 pages

| ⌥ | Page | Contents |
| --- | --- | --- |
| 1 | **Sound** | Oscillator (shape, audition, waveform), Volume · Pan · Tune, Filter · Drive, Amp envelope |
| 2 | **Modulation** | Filter envelope · LFOs |
| 3 | **Voice & Record** | Voice behavior, Record a sample |

**Play & perform** (octave, chord, piano) stays below every page for every instrument type.

### Removed duplicates (same functionality, one control each)

* **SVG overview waveform** (sampler only). Its click gestures moved onto the editor canvas: ⇧-click sets End, ⌥-click sets Loop start, ⌥⇧-click sets Loop end, and plain click/drag picks and moves the nearest marker, as before. Plain click on the overview used to set Start; on the canvas you drag the S marker (or ←/→ with Start selected). The synth keeps its oscillator SVG.
* **`#inst-mode` select.** Play mode is chosen with the editor radio group (←/→, click, `M`/`⇧M` on the waveform, the context menu, or the MK3).
* **Sampler "▶ Audition" button.** Hold to audition in the editor header, Space on the waveform, and the slice ▶ buttons cover it. The synth keeps `#audition`.
* **"Drive · Bit depth" panel**, merged into Filter · Drive with the same ids (`param-drive`, `param-bitDepth`).
* **Time stretch** moved from the bottom of the Slice map to the editor tools ("Stretch…", same `#sampler-stretch`), because it is a whole-sample edit.

Every other id, `data-automate` attribute, context-menu item, palette command and MK3 hook (`STSampler.controller`) is unchanged.

### Fit

* **1500×940:** the Sample page shows the waveform, play mode, start/end/loop markers, volume/tune, filter and the full amp envelope without scrolling, and the Region row too. Content is 894px in a 749px viewport, down from more than 3,000px.
* **1080×700:** the editor (canvas 128–150px tall when the window is short) and the top of the sound strip are visible. The three columns become two, with Amp spanning both rows (ADSR in a single row). The slice page stacks to one column below 1180px.

## Keyboard model

* **⌥1 – ⌥5**: open page *n* and focus its primary control: the waveform canvas, the selected slice ▶, the first zone field, the first modulation number, the voice mode, or the oscillator shape. It works from anywhere in the Instrument view, including number fields (it is matched on `event.code`, so ⌥-symbols are never typed).
* **⌃Tab / ⌃⇧Tab**: next / previous page.
* **Page tabs** are a WAI-ARIA tablist with a roving tabindex. ←/→ and Home/End move and activate (focus stays on the tab), ↓ enters the page, and Tab moves into the page normally.
* **F6 / ⇧F6** sections in the Instrument view: Transport → Views → Instruments → Instrument actions → **Instrument type** → **Instrument pages** → *each panel on the visible page* → Play & perform. Hidden pages drop out, so the cycle is short (Sample page: editor, level, filter, amp, region, perform).
* **⌘F Find control** still searches every control on every page. Results show the page name ("Voice & Record · Voice behavior"), and Return opens that page and focuses the control (`STInstrumentPages.reveal`).
* **Command palette**: "Instrument page 1–5", "Next / Previous instrument page". The editor and slice palette commands (`sample:*`) open the page their result appears on.
* **Help sheet**: new "Instrument pages · Synth & Sampler" section.
* All existing waveform keys (←→ nudge, ↑↓ marker, T/I/O/N/R/G, A, P, M, Z, Space…), slice-list keys and ⌥A automation are unchanged.

The selected page is remembered per instrument type (Sampler / Synth) across re-renders and instrument changes.

## Implementation notes

* All pages stay mounted; inactive ones carry `hidden`. Switching pages only toggles visibility and focus. It does not re-render, mutate, or send audio requests, so element ids, automation targets, focus restoration and MK3/controller state are unaffected. The waveform redraws when its page becomes visible (`STSampler.redraw`).
* `STSampler.parts(inst)` returns the editor, record, voice, slices and zones sections separately. `STSampler.render(inst)` still returns the full stack (used by tests).
* `STInstrumentPages` (in `instrument-navigation.js`) provides `list`, `current`, `attributes`, `renderBar`, `show(id|n, {focus})`, `step`, `reveal(node)`, `key` and `bind`.
* Section style is unified: every instrument panel uses the `.instrument-layout` panel (#23231f surface, #2b2923 header, 13px headings with .04em tracking, default chips, 12px/14px padding). The sampler-only header and chip overrides were removed.

## Deliberately left as is

* Plug-in (overview + NKS pages) and External MIDI: already focused, no page bar.
* The Modulation console internals (`modulation.js` / `.css`); another stream is working on LFO rate controls. Only the duplicate heading is hidden, via `styles.css`.
* Play & perform below every page. It is the mouse piano and works on any page.
* The slice map has no waveform of its own: the Sample page canvas shows slice markers in Slice mode and remains the place to place them by ear.
