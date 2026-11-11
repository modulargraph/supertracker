# Kits

A kit is a **group of ordinary tracks with its own group bus**, like a Maschine Group and its Sounds. Each loaded pad is one track (a member), with its own sampler or instrument plug-in, pattern column, mixer channel, inserts and sends. Every member is routed automatically to the group bus, which has its own fader, mute and solo, inserts and sends and goes to the master. Moving a track into or out of the group re-routes it.

There is no fixed number of kits or pads in a session; a kit file holds sixteen pads.

## In the pattern editor

A group has a header with the kit name, colour, a collapse/expand button, bus mute/solo and a level meter.

- **Expanded**, the members sit side by side as ordinary tracks. Selection, copy/paste, drag, transpose, insert/delete rows, step FX and automation all work as for any track.
- **Collapsed** (⌥J, or Return on the collapsed column), the group is one overview column. Each row lists the pads that hit there, so you can read the whole beat. Entering notes on a collapsed group needs Pad keys or expanding it.
- **Pad keys** (⌥Z, or the **Pad keys** button) maps Z S X D C V G B H N J M , L . ; to the members of the cursor's group, in pad order. A key auditions that pad. While recording, it enters the pad's note (C-4 unless the pad says otherwise) on that member's row. With Pad keys off, keys play pitch on the cursor track as usual.
- **Group** (⌘G) groups the selected tracks with a new group bus. **Ungroup** (⌘⇧G) takes the cursor track out of its group. **Dissolve group** keeps the tracks and removes the group. **Add track to group** adds an empty pad. **Move track left/right** (⌘⇧←/→) moves a track within the session: a neighbour at a group's edge joins the group, an edge member leaves it, and a collapsed group is stepped over.
- **Choke group** and **voice mode** (poly, mono, legato) are track properties in the sampler (on the Maschine MK3: CHANNEL · Track, encoders 7 and 8). Tracks with the same choke number in one group cut each other off (open and closed hi-hat). The same number in another group does not interact.

## In the mixer

A group strip comes before its members. Collapse it to hide the members and keep only the group strip. The group strip's inserts and sends process the whole kit.

## Loading, replacing and saving

Open Library (⌘3) and choose **Kits & Groups**. It lists Maschine expansion groups and saved SuperTracker kits. Choose one or more sources on the left, or search by name. With the results focused, Up/Down selects, Space auditions the group preview, Return adds the kit, and Shift-Return adds it and opens the pattern. **+ Kit** in the pattern toolbar opens this browser. **New empty kit** starts a group with one empty pad.

- **Load kit** adds a new group: one member per loaded pad, plus the bus. The kit is read and prepared before the session changes. A progress message names each step, samples finish decoding in the background, and the kit is added as one undoable edit. Playback keeps running while a kit loads. If the kit cannot be prepared, or the session is edited meanwhile, the session is left as it was and the reason is shown.
- **Replace kit** (with the cursor in a group) swaps the members' sounds pad by pad and keeps their pattern notes.
- **Save kit** writes the group into your kit library: member instruments and plug-in states, inserts, mix, choke groups and the bus. Sample files are collected into a portable `.stkit` package. After the first save, Save kit overwrites that package; **Save kit as** asks for a new name. Saving renders a preview of the group bus from the selected pattern into the kit (`Preview.wav`): up to ten seconds or eight bars, plus a one-second tail, with the pads' instruments, effects and plug-ins and the bus's own processing, and nothing of the rest of the session. Playback keeps running while it is rendered. This is what Space auditions in the kit browser. A kit whose pads play no note in the selected pattern is saved without a preview, and the message says so; put a few hits in the pattern and save again. External MIDI pads are not in the preview (they need recorded audio), nor is a session with a live line input rendered.

The default kit folder is `~/Documents/SuperTracker/Kits`. **Rescan library** refreshes the configured folders, and **Add group / kit folder** includes an expansion stored elsewhere. Loading the same kit twice creates two independent groups.

To load a sample onto one pad, put the cursor on that member track and use the Samples browser's **Replace selected instrument** (Option-Return). **Add sample on new track** (Return) adds an ordinary track.

## Older projects

Projects saved with the earlier kit lanes are converted when they open:

- Each kit becomes a group.
- Each Sound that had a source or notes becomes a member track.
- Stacked hits become notes on their member's row, with pitch, velocity, gate, chords and FX.
- Sound mix, inserts and automation move to the member. The kit's group mix and inserts stay on the group bus.
- Choke and mono settings carry over.

A message lists what was converted. The file on disk is unchanged until you save.

## Maschine compatibility

Maschine `.mxgrp` support imports verified portions of the native archive. The browser shows compatibility notes before loading, and the imported group keeps them as import notes. Samples, zone ranges and supported processor controls become editable SuperTracker data.

The built-in studio effects and DrumSynths are independent implementations. Their names describe the corresponding effect families; they are not NI binaries and have not been established as sonically identical. Verified parameter conversions are used where available; approximate or default mappings are reported.

See [built-in effects](BUILTIN_EFFECTS.md) for processor coverage and `cargo doc -p st-library` for the kit package format and the data model.

## MK3 (in development)

Maschine MK3 support is experimental and off by default; turn it on in Settings (see [Maschine MK3](MASCHINE_MK3.md)).

Group buttons A–H select groups; pads select and play the members of the selected group. Sound / Group / Master change the editing scope (the member track, the group bus, the master). See [Maschine MK3](MASCHINE_MK3.md).
