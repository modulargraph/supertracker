# Missing-media relinking

A project refers to audio files: instrument samples, multisample zones, timeline audio clips and their takes, and
the prints of frozen tracks. When a file is gone (a disk that is not connected, a folder that was renamed, a library
that moved), **Relink missing project media…** in the command palette lists what is missing, searches for it and
replaces the references the user chooses.

Kit pads are ordinary tracks with ordinary instruments (see [kits](KITS.md)), so their samples are instrument
samples and zones like any other. `builtin:` references name content of the application and are never media.

## What the user sees

The dialog (`ui/media-relink.js`) checks the project as it is in the page, saved or not.

- **Missing references.** One row per reference whose file is not usable, with its kind, label and path. A file that
  is available again when the disk is reconnected needs no change: **Check again** reads the state anew, and the
  project is not rewritten.
- **Where to search.** The sound library's folders are offered and ticked; a folder can be added by typing its
  absolute path. A **relocation rule** says "files under this folder are now under that one" and keeps the path
  below it, for a drive or a parent folder that was renamed.
- **Candidates.** A search only finds candidates. Each is labelled *Verified content* (its SHA-256 equals a saved
  fingerprint), *Verified hash prefix* (it matches the hash in a collected file's name), *Relocated path ·
  unverified* or *Name match · unverified*. Nothing is selected by the search; the user chooses a candidate for each
  reference, or picks a file directly with **Choose replacement file…**.
- **Different content.** When the chosen file is known not to be the saved one, the replacement needs the checkbox
  *Use this file despite the saved content mismatch*. That is a deliberate change of sound, and the result carries a
  warning. A frozen print never accepts other content than the one that was printed.
- **Apply** replaces the chosen references as one Undo step ("relink project media"). Audio files are never
  modified, renamed, copied or deleted by relinking. Saving the project afterwards is the user's decision.

A reply that arrives after the user opened another project, or after the project was edited while the host was
working, is not applied: the dialog says so and the references are checked again.

## Commands

The host's part is `crates/st-host/src/media.rs`, on top of `st_project::relink`. The commands are in
[HOST.md](HOST.md); each takes the page's `project` (without one it uses the session's).

| Command | What it does |
|---|---|
| `project.media.inspect` | Lists every media reference and whether its file is there. |
| `project.media.search` | `{roots, relocations}`: inspects, then looks for the missing files. One search runs at a time; a newer one replaces the one still running. Progress arrives as `project.media.progress` events about four times a second. |
| `project.media.apply` | `{selections}`: validates every selection, then returns the relinked project. The session changes when the page publishes that project. |
| `project.media.cancel` | Stops the running search and every inspection or application in progress. |
| `project.media.chooseRoot` | A folder chooser. The dialog does not call it yet: folders are typed. |

Roots and relocations must be absolute paths, at most 64 of each. Searching, hashing and applying are blocking file
work and run on the host's worker threads, never on the audio thread.

### References

`inspect` returns `references`, `missing`, `referenceCount`, `missingCount`, `presentCount` and `warnings`. A
reference:

```json
{
  "id": "/instruments/0/zones/2/samplePath",
  "kind": "zone",
  "label": "Drums · Zone 3",
  "path": "/Volumes/Old Disk/Pack/Kick.wav",
  "resolvedPath": "/Volumes/Old Disk/Pack/Kick.wav",
  "status": "missing",
  "available": false,
  "fingerprintConflict": false
}
```

- `id` names the field of the project that holds the path (`/instruments/<i>/samplePath`,
  `/instruments/<i>/zones/<z>/samplePath` or `/path`, `/audioClips/<c>/path` or `/samplePath`,
  `/audioClips/<c>/takeSet/takes/<t>/path`, `/tracks/<t>/frozen/path`, `/tracks/<t>/frozen/patternPrints/<p>/path`
  and `/loop/path`). Treat it as an opaque token.
- `kind` is `sample`, `zone`, `clip`, `take` or `freeze`.
- `status` is `available`, `missing`, `notFile`, `unreadable`, `empty`, `unresolvedRelativePath` (a relative path
  and no project file to resolve it against; paths never resolve against the working directory) or, for frozen
  prints, `contentMismatch`, `unverified` and `invalidFreeze`.
- Where they are known: `size`, `expectedSHA256`, `expectedHashPrefix`, `expectedSize`, the indices `instrument`,
  `zone`, `clip`, `take`, `track`, and `requiresExactContent` for frozen prints.

Which fields are media is defined by the project's schema (`crates/st-project/src/media.rs`), not by a key name or
an extension: plug-in state and unknown fields are never read as paths or rewritten.

### Search

`search` adds to each missing reference `candidates` (best first, at most 16), `candidateCount`, `resolution`
(`unresolved`, `verified`, `ambiguous`, `suggested`), `ambiguous` and `requiresSelection`, and to the report
`scannedFiles`, `hashedFiles`, `hashBytes`, `truncated`, `requestId` and `operationId`. A candidate has `path`,
`size`, `match` (`sha256`, `hash-prefix`, `relocation`, `name`), `verified`, `contentMismatch`, `score`, `reasons`
and, when it was hashed, `sha256`.

The search looks where a relocation rule puts the file, in the project's `.media` folder, and through the chosen
folders for files of the same name or, when a full fingerprint is saved, the same size. A file name is never proof.
A candidate is verified only by content:

- its SHA-256 equals the fingerprint saved in the project's `mediaManifest`, or the hash recorded with a frozen
  print;
- or its SHA-256 starts with the hash prefix in the name of a collected file. Only the two known layouts count:
  `<Project>.media/<16 hex digits>-<name>` and `<Kit>.stkit/Samples/<24 hex digits>-<name>`. A hash-looking name
  anywhere else proves nothing.

Several verified copies stay visible and the reference stays ambiguous. Candidates of equal rank are ordered by
path, so the same search gives the same answer. Nothing picks a first match.

Limits (`relink::Limits`): 250,000 files looked at, 20,000 candidate files, 16 candidates per reference, 128 files
hashed, 256 MiB per hashed file, 1 GiB hashed in total, 30 seconds. Hidden files and symbolic links are skipped.
Reaching a limit sets `truncated` and adds a warning; a file that was not checked is never labelled verified.
Narrower folders complete a wider search. Cancellation is checked while walking and after every mebibyte of
hashing.

### Apply

A selection is explicit:

```json
{
  "id": "/instruments/0/zones/2/samplePath",
  "from": "/Volumes/Old Disk/Pack/Kick.wav",
  "to": "/Volumes/New Disk/Pack/Kick.wav",
  "allowDifferentContent": false
}
```

Every selection is checked before anything changes; one bad selection rejects the batch:

- `id` must name a current reference and `from` must equal its path. A selection made before the project changed is
  stale and refused.
- `to` must be an absolute path to a readable file with content.
- A reference whose original file is available again is not replaced.
- Fields that belong together move together and must agree before: a zone's `path` and `samplePath`, a clip's path
  and its active take.
- A file whose content differs from the saved fingerprint needs `allowDifferentContent`. An unverified name match
  does not need it: it is checked against the fingerprint when applied. A frozen print only accepts the exact
  printed content.

The result is `project`, `relinked` (`[{id, from, to, differentContent}]`), `changed`, `remaining` and `warnings`.
In the returned project the paths are replaced, `mediaManifest` describes the new files, and the operation is
appended to `mediaRelinkHistory` (the last 64 are kept). Sample settings, zone ranges, clip offsets and fades,
patterns, routing and plug-in state are untouched. Verification reads at most 4 GiB per application; one
application takes at most 16,384 selections.

## Fingerprints and collected media

Saving a project collects its media into the folder next to it (`Song.media`), each file named
`<first 16 hex digits of its SHA-256>-<original name>` (`crates/st-project/src/files.rs`). Inside the project file
the media paths are relative to it, so the project and its folder can move together. That name is what lets a
collected file be recognised by content later, with or without a manifest entry.

`mediaManifest` holds full fingerprints: `path`, `size`, `modified` and `sha256`. A relink writes the entries of the
files it put in. `relink::fingerprint` records them for every available file of a project; no command calls it yet,
so a project that was never relinked has only the hash prefixes of its collected files to go by. Without either, a
reference can only be matched by name, and stays unverified.

Hashes tell whether a file is the saved one. They do not tell whether it decodes: the audio host still reports a
corrupt or unsupported file when it loads it.

## Tests

- `cargo test -p st-project --test relink`: a moved root and a batch that is refused as a whole, ambiguity and
  deliberately different content, collected hash prefixes and relative paths, a renamed file found by content,
  limits, progress, linked zone and clip fields, takes, a portable project recovered from its library, frozen
  prints, built-in references. Unit tests in `relink.rs` cover the two collection layouts.
- `cargo test -p st-host --test host`: the commands against a project on disk
  (`missing_media_is_inspected_searched_and_relinked`, `a_media_search_stops_when_it_is_cancelled`) and the folder
  chooser.
- `node tests/media_relink_ui_tests.js`: the dialog with a simulated host. Inspection of the unsaved project, late
  replies after a project switch or a concurrent edit, no automatic selection, the content-mismatch checkbox, a
  manually chosen file, Undo.

Not exercised: the dialog against the real host in a browser, and relinking media on an external disk.
