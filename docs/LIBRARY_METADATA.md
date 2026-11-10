# Local library metadata

This workflow is included in Candidate0050. It edits the browser’s local labels; it does not rename, retag or rewrite sample or NKS files, and it does not change an instrument’s playback settings.

In the browser’s Sounds list, select a sound and press **E**, or choose **Edit details** below the results. Edit tags, category, key, tempo and, for samples, one-shot/loop type. Editing a value marks only that field for saving. An untouched field keeps its current value. Return from a text field saves; Escape cancels.

Each field also has a change menu:

- **Keep current** leaves each sound’s value unchanged.
- **Use value** applies the entered value. For tags this is **Replace tags**.
- **Use inferred** removes your override and uses the original filename/folder or NKS metadata again.
- **Add tags** combines tags with each sound’s current labels. It is the default when typing tags for several sounds.

Blank key or tempo explicitly marks the value unknown; empty replacement tags remove all visible tags. These are different from restoring inferred metadata. **Use all inferred** prepares every field for restoration; Save commits that choice. The single-sound editor shows the inferred values below the controls.

Use **Shift+Up/Down**, **Shift+Home/End**, or Shift-click for a range. Cmd-click adds/removes individual sounds. **Cmd+A** selects the current result page, and Escape clears the batch selection after stopping any active preview. Ordinary arrows return to one-sound selection and normal audition. Changing search, filters or page clears the batch selection. Background metadata updates preserve only selected paths still visible in the result page. Loading a sample still loads the focused sound, regardless of the batch selection.

Several choices within a filter remain OR matches; groups remain AND matches. Saved labels immediately feed text search, tags, category, key, tempo and sound-type filters. Existing samples may disappear from the current results after an edit if they no longer match. Length remains measured from the audio header and cannot be overwritten in this editor.

Overrides live in `~/Library/Application Support/SuperTracker/Library/metadata-v1.json`, separately from the disposable inferred index. Edits are tied to each absolute file path and survive rescans, reindexing and disconnected libraries. Moving or renaming a source creates a different path and does not transfer its overrides automatically. The metadata belongs to this Mac’s browser, not to a project or its Undo history. Restore inferred values to remove an override.

A batch is validated and written atomically before publishing any new search result. A failed write leaves the live index unchanged and the editor draft available to retry. An unreadable or unsupported saved metadata file is preserved and blocks further edits rather than replacing earlier labels. Values are limited to 500 indexed sounds per operation, 64 tags per sound, 80 characters per category/tag, and 1–999 BPM (blank is unknown). Musical keys accept note names with optional sharp/flat and major/minor, for example `C`, `F# minor`, `Bb major` or `Cm`.
