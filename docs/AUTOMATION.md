# Automation across patterns

## How automation is stored and played

- Each pattern owns its lanes: `project.patterns[].automation = [{id, track | owner, target, name, interpolation, points:[{row, value}]}]`.
  Rows are pattern rows (live recording stores 1/64-row points); a lane holds its first value before
  its first point and its last value after its last point.
- `AudioProject.swift` sends native lanes to the engine with `st_engine_add_automation_point`;
  bus, send and plug-in lanes are mapped by `AudioRoutingAutomation` and sent as host lanes
  (`st_engine_add_host_automation_point`). The engine evaluates the lanes of the pattern that is
  playing (`TrackerEngine::automation`) and publishes parameter events, which the host applies
  sample-accurately (`AudioMixAutomation` for track and send levels).
- When the playing pattern has **no lane** for a parameter (or an empty one), the engine leaves the
  parameter where the previous pattern put it. Stop, seek and project refresh reset to the base value.
- In Song mode the engine plays `project.sequence` (a flat list of pattern ids; the arranger shows
  runs of the same id as one block with a repeat count). A pattern that appears several times plays
  the same lanes every time.

## Choice: an editing layer over pattern lanes (option b)

Song-level lanes stored on the project (option a) would need a second automation source in the
engine, in the routing/host lane path, in the mix automation, export, freeze replay, MIDI export,
plug-in step locks and live recording, plus a precedence rule users must learn ("song lane wins over
pattern lane, except…"). Every one of those paths already plays pattern lanes correctly.

So SuperTracker keeps **one** model: automation lives in pattern lanes. Automating across patterns is
an editing layer (`Resources/Web/automation-song.js`) that lays the song out as a timeline and writes
what you draw into the pattern lanes of the slots it touches. Consequences:

- Existing projects load, play and export exactly as before; no project field was added and the
  engine is unchanged.
- What you see in the song timeline is what the engine plays, including the "holds the previous
  value" rule for patterns without a lane (drawn dashed).
- A pattern that repeats shares one lane. Editing it in one slot edits every repeat. The timeline
  says so for the selected slot (and highlights the other repeats); a sweep that crosses a repeated
  pattern asks first and offers to **make those slots unique** (each gets its own copy of the pattern,
  named `Pattern · 2`, in the same undo step).

## Automation view

- **Pattern selector** (above the lanes): `‹` / `›`, a menu showing which patterns already automate
  the selected parameter (`∿ n pts`, `×n in song`), and chips `P01 P03 …` for those patterns.
  `[` / `]` switch pattern from anywhere in the view (also from the menu); text fields keep typing.
  The selected parameter is kept: the same lane is selected in the new pattern even if it sits at a
  different index. If that pattern has no lane for it, a panel offers **Create lane here**,
  **Copy lane from Pnn** (the nearest earlier pattern that has one) and **Show across song**.
- **Copy lane to…** opens a checklist of the other patterns (whether each has a lane, whether the
  track plays in it, how often it is in the song) with quick picks *Patterns using <channel>* and
  *Patterns in the song*. Rows are time, so they are kept; a shorter destination ends on the source's
  value at its last row. Destination lanes are replaced. One undo step.
  Right-click a lane tab or the envelope for the same actions, including *Copy lane to all patterns
  using <channel>* (patterns where the track has steps; for buses and Master, the song's patterns).
- **Song timeline** (`\`, or the *Song timeline* toggle): the parameter along the whole sequence,
  one slot per sequence entry, with pattern labels and `n/m` repeat counters. Keys on the timeline:
  `PageUp` / `PageDown` slot, `⌥←` / `⌥→` previous / next point anywhere in the song, `Home` / `End`
  first / last point, `←` `→` move the point by a row (`⇧` by a beat) inside its slot, `↑` `↓` value
  (`⇧` coarse), `Enter` adds a point (a slot without a lane starts at the held value), `Delete`
  removes, `U` makes the slot unique, `+` / `−` / `0` zoom. Mouse: click adds, drag moves,
  `⇧`-drag draws a sweep, right-click for *Add point here*, *Make slot unique*, *Sweep from here…*,
  *Edit this pattern's lane*.
- **Sweep across slots**: from slot/row/value to slot/row/value. Inside every slot it crosses, the
  swept rows are replaced by a straight ramp (perceptual for log-scaled parameters such as cutoff,
  with one point per beat); the curve before the sweep is kept (a hold is added in a slot that had
  no lane), and existing points after it are kept with an anchor back to the old curve.
- Palette (`⌘K`): *Automation: previous / next pattern*, *pattern lane / song timeline*,
  *copy lane to other patterns…*, *copy lane to every pattern its channel plays in*,
  *sweep across song slots…*, *make selected song slot unique*.

Every edit is one undo step; switching patterns or scope is not an edit.

## Live recording in Song mode

Live automation (Touch / Latch / Write) already writes at the playhead into the lane of the pattern
under the playhead, so recording across a Song-mode pass lands in each slot's pattern lane — the
same model. When a repeated pattern plays again in the same pass, its second pass overwrites the
first where they overlap (the last pass wins), and every repeat plays the result; the commit toast
names repeated patterns. Make slots unique before recording if repeats should differ. The song
timeline shows the in-progress take for every slot.

## Limits

- Repeats cannot differ without becoming separate patterns (by design of option b).
- Sweeps are straight ramps; curves are built from several sweeps or by editing points.
- The song timeline follows the flat sequence; the song loop and queued jumps are not drawn.
