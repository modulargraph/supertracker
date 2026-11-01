# SuperTracker design brief

The user accepted the functional direction and rejected the initial interface as too recognizably generated. This revision keeps the musical workflows and gives them a deliberate desktop instrument design. The work surface should feel calm, precise and useful during a long session. It should immediately communicate **where the music is, where the keyboard is, and what will change next**.

This is an original product brief informed by the sources below, not a request to install or execute an external skill. Research and the baseline native-window inspection were performed on 9 October 2026.

## Research translated into product decisions

| Primary source | Relevant guidance | Application here |
| --- | --- | --- |
| [Apple: Designing for macOS](https://developer.apple.com/design/human-interface-guidelines/designing-for-macos/) | Use available space and keyboard interaction for sustained desktop work, with familiar menus and adaptable windows. | Spend space on tracks, samples and parameters. Keep standard file/edit commands and stable resizable panes. |
| [Apple: Focus and selection](https://developer.apple.com/design/human-interface-guidelines/focus-and-selection/) | Focus identifies the target of interaction; fields and lists use appropriate, distinct treatments. | Separate keyboard focus, selected musical data and the moving playhead. A selection must remain visible when its pane loses focus. |
| [W3C: ARIA grid pattern](https://www.w3.org/WAI/ARIA/apg/patterns/grid/) | A composite grid manages directional navigation internally and occupies a compact tab sequence. | The tracker is one navigational surface with a clear exit to its inspector. Moving through hundreds of notes must not require hundreds of Tab presses. |
| [W3C: Text contrast](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html) | Ordinary text requires 4.5:1 contrast; larger text has a lower threshold. | Every useful label, filename, value and shortcut uses at least 4.5:1. Small text is not made faint to establish hierarchy. |
| [W3C: Focus appearance](https://www.w3.org/WAI/WCAG22/Understanding/focus-appearance) | The enhanced focus benchmark combines visible area equivalent to a two-pixel perimeter and a 3:1 state contrast change. | Use this stronger benchmark for the primary keyboard target. Evaluate it on selected rows as well as neutral surfaces. |
| [W3C: Focus not obscured](https://www.w3.org/WAI/WCAG22/Understanding/focus-not-obscured-minimum.html) | Sticky content and overlays must not hide the focused control. | Reveal focused grid cells, inspector fields and modal actions beneath sticky headers/footers. |
| [NN/G: Complex-application design](https://www.nngroup.com/articles/strategies-complex-application-design/) | Specialized tools need domain and workflow research, with realistic constraints and contextual testing. | Judge the redesign by editing music, comparing sounds and finding automation—not by a hero screenshot alone. |
| [Anthropic: frontend-design source](https://github.com/anthropics/skills/blob/main/skills/frontend-design/SKILL.md) | Ground visual choices in the subject, establish a deliberate token system, critique generic structure and let wording serve the task. | The distinctive element is the musical work surface. Give it careful alignment, numerical rhythm and material restraint. Marketing composition, display typography and page-load theater are inappropriate here. |

The sources do not prescribe this palette, layout, sizing or shortcuts. Those are SuperTracker-specific design choices and acceptance targets.

## Baseline findings from the actual app

The initial native Browser was inspected at 1500 × 940 logical pixels. Approximately nine complete results were visible. A large introductory heading, repeated note-icon tiles, a large generic inspector illustration and generous row spacing consumed space that could show more sounds. Collection and category areas had competing scroll regions. Muted labels were harder to read than their importance justified. The design repeated rounded bordered panels with similar emphasis across unrelated functions.

The initial stylesheet also allocated 76 pixels to the top bar, 47 to navigation and 33 to the footer before workspace controls. The tracker used 27-pixel rows and wide track columns. These observations motivate structural changes; recoloring alone does not resolve them.

## Visual direction: a modern music console

Use warm graphite surfaces, bone-colored content and restrained orange for the active editing context. The reference is the organization of a well-made musical instrument: clear signal paths, useful scales, stable locations and readable values. Avoid imitating worn hardware, fake screws, fake wood, neon glow or a physical device's small display limitations.

The musical content carries the personality. A tracker pattern should be recognizable through its row rhythm, column alignment and concise musical notation. A browser should feel like an efficient sound library. A mixer should read as a connected bank of strips. None needs a promotional introduction.

- One compact application bar holds project name, transport, position and tempo. Reserve macOS window-control space explicitly.
- One modest view switcher stays in the same location. Its selected state uses an underline or edge and a weight change.
- The workspace starts immediately with its actual controls and content. Names such as **Pattern**, **Sample**, **Audio Units** and **Automation** are sufficient.
- Group related parameters by alignment, proximity and a small heading. Borders mark meaningful boundaries; they do not wrap every small group in a card.
- Keep corners modest: roughly 3–4 pixels on controls, 5–7 on transient dialogs, and square shared work-surface edges. This is a hierarchy, not a universal radius applied everywhere.
- Do not enlarge empty artwork or repeat decorative file tiles. If a preview is shown, make it a waveform or useful source identity.

## Proposed tokens and typography

These tokens are a contrast-checked reference, not a second competing theme. The implementation can choose nearby values if it checks the actual combinations.

| Role | Example | Purpose |
| --- | --- | --- |
| Canvas | `#181A1B` | Quiet application base |
| Surface | `#222426` | Main working regions |
| Raised control | `#2B2D2F` | Editable fields and transient controls |
| Decorative divider | `#414346` | Nonessential panel/row separation only |
| Primary content | `#E7E2D8` | Notes, names and important values |
| Secondary content | `#ADA79C` | Readable labels and metadata |
| Active accent | `#E9A066` | Focus/active edit indication |
| Recording/error | `#EF8F85` | Semantic state with text/icon |
| Playback/status | `#A4BC92` | Subordinate to focus, with a distinct shape |

Calculated against the raised-control example, primary text is 10.71:1, secondary text 5.78:1 and orange 6.38:1. The decorative divider is only 1.39:1 and **must not** be the sole visible boundary of a required control. These calculations assume opaque colors; opacity, blending, selection fills and disabled states require separate checks. A palette calculation is not a whole-app accessibility certification.

Use the macOS system sans face for interface language and a system monospace face for aligned musical/numeric data. This use of system typography is deliberate: unfamiliar display faces add little to a dense editing tool. Keep tabular numbers in counters and numeric fields.

| Text role | Target size at 100% | Treatment |
| --- | --- | --- |
| Note/value and ordinary controls | 12–13 px | Regular/medium, high contrast |
| Section/control labels | 11–12 px | Sentence case, medium where useful |
| Secondary metadata/shortcuts | At least 11 px | Normal letter spacing, readable contrast |
| Workspace title | 15–17 px | One concise line, semibold |
| Transport position/tempo | 15–18 px | Tabular/monospaced numbers |

Avoid 8–9 pixel functional labels, widely tracked capitalized headings, large slogan text and oversized shortcut pills. Row numbers are useful sequence information and should remain; arbitrary section numbering is unnecessary.

## Density and resizing targets

Measure in logical/CSS pixels at 100% scale, not Retina screenshot pixels. Required accessibility zoom should remain available; a denser default must not prevent increasing type size.

| Acceptance target | 1500 × 940 | 1080 × 700 |
| --- | --- | --- |
| Combined persistent application bar, navigation and status | At most 116 px | At most 108 px |
| Tracker | At least 7 complete tracks and 24 rows with standard columns | At least 5 complete tracks and 16 rows; local horizontal scroll for more |
| Browser results | At least 16 complete, readable results | At least 10 complete, readable results |
| Browser workspace | Filters, results and compact inspector can coexist | Results stay useful; optional detail/filter collapse has a keyboard action |
| Instrument editor | Source/waveform and main sound controls visible together | Sections scroll within workspace; labels, values and actions do not overlap |
| Mixer | Continuous track strips; master always identifiable | Local horizontal strip scrolling; selected strip revealed |
| Automation | Target list plus envelope/point controls | Point-table editing remains reachable without precision dragging |
| Whole window | No body-level horizontal scrolling or clipped transport | No clipped transport, invisible dialog buttons or accidental pane overflow |

Browser rows should normally be 32–38 pixels depending on whether a second metadata line is shown. Tracker rows can be 22–24 pixels while retaining a 12-pixel note font. Numeric parameter rows should align their label, editable value and unit. A reduction in padding is preferable to shrinking the text.

Persistent controls must not jump when values grow: reserve widths for tempo, position, meters and status. Long library/plug-in names truncate in their allotted column and expose the full name through focus/accessibility or an inspector. Empty and populated states use the same basic geometry.

## Focus, selection, motion and feedback

Focus answers **where a key goes**; selection answers **which music is selected**; the playhead answers **what is sounding**. Give them different geometry:

- Focused control/cell: a clear two-pixel outline or equivalent high-contrast edge.
- Selected row/range: a quiet fill that remains recognizable after focus leaves.
- Playhead: a restrained horizontal rule and row-number marker; it must not erase the focused-cell outline.
- Recording: an explicit red indicator and label. Piano-enabled state is independently labeled.
- Mute/solo/automation: compact text or symbols plus color; color alone does not convey state.

Tab follows stable pane/control order; internal arrows handle lists and the tracker. The tracker documents `Ctrl+Tab` to leave its internal track navigation. Dialogs retain focus and restore it to their invoking control. Search updates must preserve text selection and result identity, and status updates must not replace the focused node.

No automatic entrance animation, animated gradient, pulsing decoration or hover scaling. The playhead/meters are meaningful motion. Respect reduced-motion settings for nonessential transitions. Selection and focus update immediately; progress appears in one stable location. Library indexing, plug-in loading and export report their own operation and relevant error, rather than telling the user to check an unrelated audio device.

## Reusable design prompt

```text
Improve the existing SuperTracker macOS DAW interface while preserving its working
audio, project, keyboard, browser and automation behavior. Read docs/DESIGN.md and
the current source before changing anything. Inspect the actual app at 1500x940
and 1080x700; record concrete problems rather than describing a generic aesthetic.

Design for someone editing music for hours with a computer keyboard. Give the
pattern grid, sound results, parameters and envelope the space and hierarchy they
need. Use a restrained warm-graphite music-console direction with bone text and a
small orange focus accent. Keep macOS system typography for controls and aligned
monospace musical data. Make meaningful names and values readable at a glance.

Remove promotional headings, repeated icon artwork, excessive card wrappers and
space that forces unnecessary scrolling. Do not substitute another website
template, add ornamental imagery, or make a large headline the focal point. The
music is the focal point. Color changes alone do not count as a redesign.

Separate keyboard focus, selected data, playback and recording visually. Preserve
the current shortcuts and expose a clear way in and out of every composite grid.
Keep selected rows and input focus stable through updates. Use contextual actions
and editable numeric values, with optional trackpad gestures as equivalents.

State a small token/layout plan, implement it in the existing files, then inspect
all six workspaces with realistic populated data at both sizes. Check text and
focus contrast, visible row/track counts, dialogs, long names, disabled/empty/error
states and playing/recording states. Run the existing behavior tests. Fix failures
and report measured improvements plus any remaining visual or functional limits.
```

## Review checklist

- [ ] Compare before/after at identical window dimensions and realistic content.
- [ ] Verify density targets without shrinking useful type below the scale above.
- [ ] Inspect every workspace, command palette, settings, export and recording dialogs.
- [ ] Check the active cell over beat stripes, range selection and the moving playhead.
- [ ] Complete browse → load → enter notes → adjust instrument → automate → arrange → save using the keyboard.
- [ ] Resize while a field is focused and while a long list/parameter panel is scrolled.
- [ ] Test long plug-in/library names, no results, offline library and progress/error states.
- [ ] Measure actual computed color pairs, including selected and disabled states; do not rely on appearance alone.
- [ ] Confirm meters, indexing and transport updates do not steal focus or cause layout movement.
- [ ] Re-run regression tests and preserve the user's accepted functionality.
