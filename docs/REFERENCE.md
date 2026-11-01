# Reference research and interaction design

Research checked on 9 October 2026. This document separates source observations from proposed Supertracker behavior. It is a design reference, not a claim that every feature below is implemented.

## What to take from Polyend Tracker

The official Tracker manual describes instruments that combine a sample, playback settings and sound parameters. Playback includes one-shot, forward/backward/ping-pong loops, slice/beat-slice, wavetable and granular modes. Instrument shaping includes volume, pan, tuning, resonant low/high/band-pass filtering, drive and bit reduction. Envelope/LFO modulation reaches several sound parameters. Delay and reverb are shared effects with instrument send amounts. The sequencer exposes note, instrument and two step-effect columns, alongside song arrangement and performance functions. These are useful building blocks without reproducing the hardware's track, memory or screen limits. [Polyend Tracker manual, instrument chapter and step FX reference](https://polyend-website.fra1.digitaloceanspaces.com/wp-content/uploads/2023/11/12115402/Polyend-Tracker-Manual-1v9v0d.pdf)

**Design implication:** make an instrument a reusable sound source, a track the place where events and processing live, and a pattern the editable musical phrase. The user should be able to change the source without rewriting notes. Keep expressive step commands near notes, with readable parameter names and musical units in an inspector. Render a numeric shortcut beside a name rather than requiring memorized effect codes.

## What to take from Maschine

Maschine's Browser combines product/bank, type/character tags, favorites, free-text search and auditioning. Search spans more than filenames, and previews let the user hear a candidate before loading it. [Maschine Browser](https://docs.native-instruments.com/ni-tech-manuals/maschine-software-manual/en/browser)

**Design implication:** one browser should search samples, instruments and effects, while preserving the current query and selection across view changes. Selection auditions; Return loads. Loading is an undoable action. A sample's waveform, duration, format, inferred tempo/key, library and path should remain visible beside its name. Do not replace the current instrument merely because the user moves through results.

Maschine's sample editor offers recording, destructive editing, slices based on transients/equal divisions/grid/manual positions, and sample mapping. Slice boundaries can snap to zero crossings. Its stretch/pitch controls can operate independently. [Maschine sampling and sample mapping](https://docs.native-instruments.com/ni-tech-manuals/maschine-software-manual/en/sampling-and-sample-mapping)

**Design implication:** edits should start as reversible region metadata, with explicit rendered operations when audio must change. Slice audition, start/end nudging and zooming need keyboard equivalents. An edit should audition immediately without discarding focus or interrupting song playback.

Maschine organizes plug-in parameters into named pages of eight controls, with custom mapping and modulation. [Maschine plug-ins](https://docs.native-instruments.com/ni-tech-manuals/maschine-software-manual/en/working-with-plug-ins) Its controller offers scales and chord generation, including harmonizer and chord-set approaches. Generated chords can be recorded as notes. [Maschine MK3 performance controls](https://docs.native-instruments.com/ni-tech-manuals/maschine-mk3-manual/en/playing-on-the-controller)

**Design implication:** eight named macros form a stable instrument surface. A separate searchable parameter list gives full access. Scale, octave, chord type, inversion and velocity should be visible when computer-keyboard playing is enabled. Record the generated notes so the resulting harmony remains editable.

## AU and NKS: distinct integration layers

Audio Units cover both instruments and effects, with system discovery, instantiation, parameters, presets and plug-in views. AU is an appropriate first native format for a macOS-only application. The transport, sampler, sequencing, mixing and DSP can remain an original engine while AU is the plug-in boundary. [Apple AU host sample](https://developer.apple.com/documentation/audiotoolbox/incorporating-audio-effects-and-instruments)

NKS supplies tagged preset browsing and parameter mappings within Native Instruments' ecosystem. NI describes it as wrapping VST and invites developers to request its Developer Kit through a partnership. A public, complete host interoperability specification was not located in this research. This does **not** establish that all NKS integration is impossible; it establishes that arbitrary preset loading and complete mapping compatibility must be proven, not inferred from file extensions. [NI NKS developer information](https://www.native-instruments.com/pages/nks-developers)

The practical first route is hosting the installed **Komplete Kontrol AU**, allowing its own browser and preset loader to handle NKS. Komplete Kontrol itself hosts VST plug-ins; those required plug-ins and their licensed content must be installed. [NI plug-ins in Komplete Kontrol](https://support.native-instruments.com/support/solutions/articles/69000879674-plug-ins-in-komplete-kontrol)

Local `.nksf` indexing and audio-preview discovery are separately useful, but must be labeled **catalog/preview support** until loading and mappings work. Do not send a VST preset's opaque state directly to an AU and assume compatible IDs or state format. Do not describe AU parameter enumeration as NKS support. A future direct loader needs format-aware state conversion, plug-in identity matching, parameter-ID mapping and representative restore tests. A hosted Komplete Kontrol editor may require trackpad interaction even when Supertracker's own parameter controls are keyboard accessible.

## Library roots

The browser indexes these locations when they exist on the Mac, and anything the user adds with **Add folder**. Read-only inspection of a large development library guided the defaults.

| Root | Typical organization | Browser use |
| --- | --- | --- |
| `/Library/Audio/Apple Loops` | Apple Loops installed with GarageBand or Logic Pro (`.caf`) | Optional sample root |
| `~/Music/Loopcloud` | Pack folders; filenames often include tempo, key and instrument; many `._*` files | Optional sample root; derive pack tags from parent folders |
| Native Access content folders (from Native Instruments' product registrations) | Expansions contain `Samples/Drums`, `Samples/Instruments`, `Samples/Loops`, `Samples/One Shots`, and `Groups`/`Kits` | Find audio under each `Samples` subtree; preserve expansion name as library. Products installed side by side are indexed through their shared parent folder |
| `~/Documents/Native Instruments/Maschine 3/Samples`, `…/Recordings`, `…/Sounds`, `…/Groups` | Maschine user content | Optional local sample and kit roots |
| `~/Documents/Native Instruments/User Content` | User plug-in preset folders | Optional user preset root |
| `~/Documents/SuperTracker/Kits` | Kits saved from SuperTracker | Always present (created on first use) |

For example, `E1_96J_170_F_Elec_Bass_01.wav` provides plausible tempo/key/bass hints. Treat them as **inferred**, editable metadata, not analyzed musical truth. One expansion uses a misspelled `Samples/Insruments` directory; discover audio by extensions and ancestry rather than a rigid folder-name whitelist.

NKS presets (`.nksf`) and their `.previews` directories live in each product's content folder, found through Native Instruments' registrations in `/Library/Preferences` and `~/Library/Preferences`; artwork comes from `/Users/Shared/NI Resources`. Presence on disk is not a successful host/load test.

Skip `._*`, `.DS_Store`, recycle/trash directories and backup bundles. Never scan a whole external drive by default: unrelated project and recovery folders may be present. Read sample-library files without modifying them. Cache indexes, tags, favorites and peaks in application data. A disconnected root should remain visible as **Offline**, and reconnecting should reconcile paths without deleting favorites.

## Proposed keyboard grammar

These are design recommendations. The shipped shortcut reference must describe the actual bindings.

| Intent | Suggested binding | Behavior |
| --- | --- | --- |
| Go to major workspace | `Cmd+1…6` | Pattern, Song, Instrument, Browser, Mixer, Automation |
| Find any command | `Cmd+K` | Search names and aliases; display bindings and disabled reasons |
| Search current surface | `Cmd+F` | Focus local search without erasing its query |
| Traverse panes | `Tab` / `Shift+Tab` | Stable visible focus; modal dialogs contain focus |
| Move within a pane | Arrow keys | Grid cells, browser results, mixer tracks or parameter rows |
| Edit or commit | `Return` | Type a value in place; validate and keep focus |
| Cancel/back | `Esc` | Close nearest overlay; restore originating focus |
| Transport | `Space` | Play/stop except while typing text |
| Adjust focused numeric value | `Option+↑/↓` | Fine increment; Shift modifier for coarse change |
| Automate focused parameter | `Option+A` | Reveal existing lane or create/select one |
| Open contextual actions | `Shift+F10` | Same actions as secondary click |
| Undo/redo | `Cmd+Z` / `Cmd+Shift+Z` | Meaningful musical edit groups |
| Help | `?` from command context | Searchable, context-sensitive shortcuts |

Reserve bare letter keys for note entry only inside an explicit, clearly marked play/record context. Text fields, search boxes, dialogs and composition events always take priority. Focus loss releases held notes. Changing octave while a key is held must still release the original note. Avoid relying on function keys, which commonly share macOS hardware controls.

## Interaction details that protect the musical flow

- **Persistent context:** changing views preserves selected track, instrument, pattern, row, browser query and scroll position. One status line states current focus and the next useful keys.
- **Readable density:** a quiet, large central pattern; restrained track colors; consistent alignment; beat/bar hierarchy; note-offs distinguished from empty steps. Increase visible density through a setting rather than tiny default text.
- **Automation discovery:** search by track, device, parameter and macro alias. Existing lanes first, then pinned/macros, then all remaining parameters. A touched parameter becomes the default candidate. The action is available through both context menu and keyboard.
- **Automation ownership:** display base value and automated value separately. Define precedence: lane establishes a value, a step lock temporarily overrides the documented interval, then the lane resumes. Show that interval in the editor. Interpolate continuous parameters; hold enumerated switches.
- **Musical editing:** selection copy/paste, duplicate, transpose, quantize, humanize, Euclidean fill, probability, ratchets and microtiming should act on a clear selection and be undoable. Preview generated edits before committing.
- **Auditioning:** independent preview gain and cancelable preview; selecting a new result stops the old one. Limit scan/analysis work away from the audio callback.
- **Recovery:** autosave, missing-file relinking and plug-in failure messages must preserve the song. Distinguish a missing asset from an intentionally empty instrument.
- **Honest capabilities:** disable unavailable actions with a reason. A decorative control that silently changes nothing is worse than a clear unsupported state.
