# Audio takes and comp regions

Song audio clips can retain up to 32 source takes. The selected take supplies the existing clip audio path and source offset; the other takes remain available in the project. A take switch is one undoable edit and does not create extra playing clips.

Select an audio clip in Song, then use **Import take…** or **Record take…** in its inspector. Importing checks the source durations and retains the prior source. Recording opens the existing recorder with a locked **Alternative for selected audio region** destination. Start and stop capture manually. The recorder displays the minimum duration needed for that region. A failed decode, changed destination, unavailable project operation or short recording leaves the capture file available in the recorder; it does not replace the original clip or start a second capture when retrying attachment.

All alternatives must cover the region when they are added. A new recording begins at source offset zero. Use **Manage takes → Source offset** to align it by ear. There is no automatic latency correction, punch range, loop-record take splitting, time stretch or transient alignment in this workflow.

## Keyboard workflow

- Focus a timeline clip and press **Page Up / Page Down** to select the previous or next available take. On a compact Mac keyboard these are normally **Fn–Up / Fn–Down**. Clip position and duration stay unchanged.
- Set the edit cursor using its numeric field or **[ / ]**, then press **S** while the clip is focused. Both resulting regions retain every take. Each take's source offset advances by the elapsed audio time in the right region.
- Choose a different take in either resulting region to build a comp. Names, alignment and active choice are independent between regions. Existing clip fade fields remain available for manual boundary adjustments.
- Press **T** to open **Manage takes**, or find “Manage selected audio clip takes” in **⌘K**. The manager supports text filtering, a native arrow-key list, explicit rename, independent source offsets, source-file preview and removal of inactive alternatives. **Enter** on its list chooses the selected take. Close/Escape returns focus to the invoking control; choosing a take returns focus to the region.
- **⌘Z / ⌘⇧Z** restore complete prior take and comp state. A duplicated clip or track has independent take data.

The active-take dropdown and visible Previous/Next controls offer the same choice as the timeline shortcuts. Text and numeric fields retain their ordinary arrow and editing keys. Selection and filtering do not dirty the project. Source previews are stopped when leaving the manager, changing its selection/filter/alignment, or replacing the dialog. A late preview response cannot revive the old selection. Accepted Undo/Redo commands, including native menu commands, close the manager and stop its preview before replacing the project. A blocked Undo or failed pending plug-in edit leaves the current manager intact.

## Saved media and compatibility

Each clip may contain `takeSet: { version: 1, activeID, takes }`. A take stores `id`, `name`, `path`, `sourceOffset` and, for newly created groups, verified `sourceDuration` in seconds. The clip's ordinary `path` and `sourceOffset` always describe the active take, so the current audio engine requires no take-specific rendering path. Split and duplicate operations copy the arrays rather than sharing mutable data.

Portable Save collects both active and inactive take source files; shared sources across split regions are copied once. Open resolves their relative paths. Missing-media inspection and relinking include inactive alternatives. Relinking an active reference keeps the outer clip and embedded active take path consistent. Conflicting replacements for those linked references are rejected before application. Removing an alternative removes only its project reference and never deletes its source file.

Take coverage, source bounds, fades, waveform geometry, splitting and import/recorded duration conversion use the project's tempo (`ui/project-timing.js`). Moving and duplicating a region keeps its length in beats; source slip moves by the duration of the grid step. Conversions beyond the supported song length are rejected before an edit is committed. Imported take metadata without a known source duration is labeled as unchecked in the manager. Unsupported or malformed take lists show an actionable error rather than silently discarding alternatives.

## Verification scope

Focused JavaScript checks cover immutable proposals, source coverage, per-take slip, split offsets, split/duplicate independence, keyboard focus, async import cancellation, manager editing, preview cancellation and alternate-recording retry/stale-destination behavior.
