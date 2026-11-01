# Kits as track groups

Status: implemented (October 2026). It replaced kit lanes, Sound columns and stacked hits. The user-facing description is in [Kits](KITS.md).

## Why

The kit lane packed up to 16 Sounds into one tracker lane. Every step held a hidden stack of hits, each with its own Sound, pitch, velocity, FX and gate, and the editor faked columns on top of that stack. Notes landed on the "wrong" pad, every selection/copy/drag/transpose path needed kit special cases, and the engine, MK3, MIDI export, freeze and routing all carried a second code path for Sound channels.

## The model

**A kit is a group of ordinary tracks with its own bus**, like a Maschine Group with its Sounds.

Project data (`project.groups[]`, members contiguous in track order):

| Field | Meaning |
| --- | --- |
| `groups[].id`, `name`, `color` | Identity and header. |
| `groups[].busID` | The group bus: `routing.buses[] {id, type: "group", groupID}`. Members output to `bus:<busID>` under standard routing. |
| `groups[].collapsed`, `mixerCollapsed` | View preferences. They are written into the Undo snapshots without adding an Undo step. |
| `groups[].libraryPath` | The `.stkit` package that Save kit overwrites (a session record, not an Undo step). |
| `groups[].importNotes` | Notes from a Maschine import or a migration. |
| `tracks[].groupID` | Membership. |
| `tracks[].pad` (0–15), `padNote` (default 60) | Pad position and the note Pad keys enter. |
| `tracks[].chokeGroup` (0–16), `voiceMode` (`poly`, `mono`, `legato`) | Per-track voice rules. Choke works across tracks *within the same group*. |

- **Member tracks** are ordinary tracks: their own sampler or plug-in, pattern column (Note, Ins, Vel, FX 1, FX 2, Gate), mixer channel and inserts.
- **Group bus** with fader, mute/solo, inserts and sends, routed to master. Adding a track to the group or removing one re-routes it.
- **Pattern editor.** A group header carries the name, colour, collapse/expand, bus mute/solo and a meter. Collapsed, the group is one overview column listing the pads that hit on each row. Expanded, the members sit side by side. Editing is the ordinary track behaviour everywhere.
- **Pads.** The MK3 group buttons select groups and the pads select and trigger members. On the computer, **Pad keys** (⌥Z) maps Z S X D C V … to the cursor group's members. While recording, a key enters the member's pad note on that member's row. With Pad keys off, keys play pitch on the cursor track.
- **Engine.** `st_engine_set_track_voice(engine, track, chokeGroup, chokeScope, voiceMode)` sets the per-track voice rules. The scope is 1 + the group index, or 0 when the track is ungrouped, so the same choke number in two kits does not interact. The separate Sound channels are gone. The channel layout is: tracks, then the return bus at `max(16, trackCount)`.

## Kit files and the browser

- **Load kit**: `kit.read` converts the `.stkit` or `.mxgrp` into a group project natively (`KitGroups.groupProject`). The UI inserts it with fresh identities (`STKitGroups.insertGroup`), publishes the candidate to the audio host, then commits it as one Undo step (`load kit`).
- **Replace kit** swaps the members' sources by pad and keeps the pattern columns (one Undo step, `replace kit`).
- **Save kit / Save As** writes the group as a sixteen-slot kit (`KitGroups.kitProject(fromGroup:)`), so the `.stkit` format is unchanged. Members beyond sixteen pads are left out and reported.
- **Maschine groups (.mxgrp)** go through the same path.

## Migration

`ProjectMigration` calls `KitGroups.migrate` when a project opens. `ProjectStore.lastMigrationNotes` reach the UI through `project.open` (`migrationNotes`), which shows them in a toast.

- Each kit becomes a group. It sits at its first lane's position, or at the end when it had no lane.
- Each Sound that has a source, hits or plug-ins becomes a member track, with routing ID `<kitID>-<soundID>`.
- Stacked hits (`kitEvents`) move to their member's row with pitch, velocity, gate, chord and FX. A lane-wide OFF reaches every member that was sounding.
- The Sound's mix, inserts and automation move to the member. The kit's group mix and inserts stay on the group bus.
- Choke groups and mono or legato modes become track properties.
- The file on disk is not changed until you save.

`tests/KitGroupsMigrationTests.swift` converts a real installed Maschine group ("1988 Kit") and, when `SUPERTRACKER_KIT_MIGRATION_FIXTURE` names one, a saved session with a kit lane. It also checks that Save kit and Load kit round-trip the same group.

## Loading while playing

Loading a sixteen-pad kit while playing extends the running routing graph incrementally: no rebuild and no pause. `--kits-self-test` checks this offline: the tracks already playing never drop out, no events are dropped, and all pads sound live. `--ui-kits-self-test` checks it on the real-time engine: one Undo step, incremental routing, about 0.2 s for the whole UI load.

Three host fixes made this work:

- **Channel source wiring.** A new track's channel source is now wired *before* its strip joins the running graph. Before, a channel realized while playing (beyond the sixteen made at launch) joined empty, and AVAudioEngine did not render the input added to it afterwards. It also refused later joins from the same branch.
- **Pending installs settle.** A full install made while running used to stay "pending" until a stopped commit, so every later change was also a full rebuild. Once the engine runs again, that install now stands.
- **Refused changes fall back.** When AVAudioEngine refuses a live connection, it raises an Objective-C exception. That exception used to end the request silently, so the UI hung. `st_objc_try` now catches it, and the change falls back to a full rebuild. `routing.status` reports `lastFullRebuildReason`.

Two related fixes:

- **Plug-ins follow their tracks.** When tracks are reordered (Move track, which also moves members within a group), a track plug-in keeps its owner and moves to that owner's new channel. Before, the routing owners were reassigned by channel first, so two moved tracks swapped plug-ins and both were reloaded.
- **Staged candidates stay out of the session.** While a kit load is staged in the host, the host's `plugins.changed` report describes that candidate, so the UI no longer adopts it into the session. Before, a kit with bus inserts (for example the "1988 Kit" Maschine group) always failed with "The session changed while the kit was loading". The UI also ignores descriptors whose owner the session does not have.
