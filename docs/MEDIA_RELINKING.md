# Missing-media relinking

The relinker inventories instrument samples, multisample zones, timeline audio clips and retained takes, and frozen audio prints. Kit pads are ordinary tracks with ordinary instruments (see [kits as track groups](KIT_GROUPS_DESIGN.md)); the kit Sound inventory below applies to `.stkit` kit files and to projects saved before the conversion. A missing external disk is reported separately from an available file. Reconnecting the disk at the same path restores availability without rewriting the project.

Search is read-only. Choose a replacement folder, or provide a root mapping such as `/Volumes/Sound Drive` → `/Volumes/Sound Drive 1`. A root mapping preserves the relative folder structure below that root. The relinker also checks the project's `.media` directory, matching filenames, and saved file sizes when a full fingerprint exists.

Candidates are suggestions until explicitly selected. A filename match is labeled unverified. A full SHA-256 match verifies the saved file contents; a collected-media filename can provide the 16-character SHA-256 prefix used by `ProjectStore`, or the 24-character prefix used inside `.stkit/Samples`. Prefix verification is confined to these known collection layouts; a hash-looking filename in an ordinary library is still unverified. Duplicate verified copies remain visible and ambiguous. Equal-ranked candidates are ordered by path so the result is deterministic. Nothing picks an arbitrary first match or silently changes the sound.

Applying selections changes a copy of the project. It preserves sample settings, zone ranges, clip offsets/fades, patterns, kit and Sound IDs, Sound channels, kit buses and routing, and plugin state. Original and replacement audio files are never modified, renamed, copied, or deleted by the relinker. The caller can audition and save the returned project normally.

If the selected file disagrees with a saved fingerprint, application fails unless the selection explicitly allows different content. This is a deliberate sound replacement, and the result includes a warning. Every selection also carries its original path; if the reference changed while a search was open, the batch is rejected. All selections validate before any project edits are returned.

## Native integration contract

`MediaRelinker` uses synchronous filesystem operations. Call searches, fingerprinting and application on a utility worker, outside the audio callback and main UI thread. Marshal progress callbacks onto the main thread as needed. The backend itself does not open dialogs, update the engine, save projects, or emit native bridge events.

```swift
MediaRelinker.inspect(project, projectURL: currentProjectURL)

try MediaRelinker.findCandidates(
    project,
    projectURL: currentProjectURL,
    searchRoots: chosenDirectories,
    relocations: [MediaRelinker.Relocation(from: oldRoot, to: newRoot)],
    cancelled: { cancellationToken.isCancelled },
    progress: { progress in /* forward a native event */ }
)

try MediaRelinker.apply(project, selections: selections, projectURL: currentProjectURL)
```

Inspection returns `references`, `missing`, `referenceCount`, `missingCount`, `presentCount`, and `warnings`. Each reference includes:

```json
{
  "id": "/instruments/0/zones/2/samplePath",
  "kind": "zone",
  "label": "Drums · Zone 3",
  "path": "/Volumes/Old Disk/Pack/Kick.wav",
  "resolvedPath": "/Volumes/Old Disk/Pack/Kick.wav",
  "status": "missing",
  "available": false
}
```

Treat reference IDs as opaque tokens. They identify actual schema fields; kit IDs also include a discriminator for the stable kit/Sound ownership. A selection captured before replacing or reordering Sounds is rejected if its owner changed, even when the old and new Sounds use the same path. Kit rows additionally include `kit`, `sound`, `kitID`, `soundID`, and `owner` (`sound:<kitID>:<soundID>`). Their existing `kind` remains `sample` or `zone`, and labels show the kit, Sound, and optional zone name.

Zone and clip `path` and `samplePath` aliases are supported; each populated field is inventoried. Selecting either zone alias repairs both linked fields. Conflicting targets or already divergent aliases reject the batch. This applies equally to ordinary instrument zones and inline kit Sound zones. Relative paths require a project-file URL and never resolve against the process working directory. Opaque plugin paths and unrelated strings are not rewritten.

Search adds `candidates`, `candidateCount`, `resolution`, `ambiguous`, and `requiresSelection` to each missing reference. Candidate fields are `path`, `size`, `match`, `verified`, `contentMismatch`, `score`, and `reasons`, plus `sha256` when computed. Match types include `sha256`, `hash-prefix`, `relocation`, and `name`. The original `status` remains the file-availability status; `resolution` describes the suggestions.

An application selection is explicit:

```json
{
  "id": "/instruments/0/zones/2/samplePath",
  "from": "/Volumes/Old Disk/Pack/Kick.wav",
  "to": "/Volumes/New Disk/Pack/Kick.wav",
  "allowDifferentContent": false
}
```

`from` must match the reference's original `path` exactly. `allowDifferentContent` is required for a known mismatch or conflicting fingerprint, not merely because a name-only suggestion is unverified. Application returns `project`, `relinked`, `changed`, `remaining`, and `warnings`. The caller should apply the returned project to the engine only after successful validation and should discard results if the active project changed during the operation.

## Fingerprints and portable projects

`MediaRelinker.fingerprint(project, projectURL:limits:cancelled:)` adds a `mediaManifest` while originals are available and returns a new project. Manifest entries contain `path`, `size`, `modified`, and a full `sha256`. This is optional and read-only; it does not save the project.

`ProjectStore` already rewrites media `path` fields during collection, so manifest paths follow their audio files into `Session.media`. When a portable project folder moves, its relative sample and manifest paths resolve together. Full hashes permit finding a renamed file with the same saved size. Without a manifest, collected `.media` filenames still provide their existing SHA-256 prefix; ordinary filename-only references cannot establish content identity after the original disappears.

Collection and fingerprints include `kits[].sounds[].instrument.samplePath` and each inline instrument zone's `path` and `samplePath`. Multiple references to one source share a collected file and one fingerprint. A zone-only repair leaves the Sound's root sample and other Sounds unchanged.

Search defaults are bounded to 250,000 files, 20,000 candidate paths, 16 displayed candidates per reference, 128 hash attempts, 256 MiB per hashed file, 1 GiB of total hash reads, and 30 seconds. Hidden files, AppleDouble files, and symlink traversal are skipped. Reaching a limit sets `truncated` and warns; unchecked files are never labeled verified. Narrowing the folders or increasing `MediaRelinker.Limits` can complete a wider search. Cancellation is checked during enumeration and every 1 MiB of hashing. Application independently rechecks selected files, with a default 4 GiB total verification budget.

Hashes detect ordinary mismatches and changes during reading. They do not validate audio decoding; the audio host still reports corrupt or unsupported media after loading.

## Verification

`MediaRelinkerTests` passes sample/zone/clip inventory, root relocation, full SHA-256 and collected-prefix matching, relative paths, renamed-file recovery, duplicate ambiguity, deterministic ordering, explicit mismatched-content override, stale selection rejection, transactional batch failure, bounded search, cancellation, symlink-loop exclusion, and source-content preservation.

`MediaPortableTests` uses the real `ProjectStore`: it collects a fingerprinted project, moves the project directory, verifies relative manifest paths, recovers a missing collected file from its original library by full hash, validates the repaired schema, and checks the original bytes remain unchanged.

`KitMediaPackageRecoveryTests` covers collected media in `.stkit` packages. Projects no longer contain kit Sounds (they convert to track groups on open), so the former project-level `KitMediaPortableTests` was removed; ordinary instrument media is covered by `MediaPortableTests`.

`KitMediaPackageRecoveryTests` uses real `KitLibrary` save/read operations, moves the package away, and repairs root/zone references from the original filenames using their saved 24-character prefixes. Wrong content is rejected. Reference-only imports with hash-looking names outside the exact `.stkit/Samples` layout remain unverified.

`MediaRelinkServiceTests` checks utility-worker execution, unsaved and kit snapshots, explicit proposals, coalesced searches, cancellation when a nested Sound path changes, progress IDs, and filesystem bounds. The service's search key includes the entire project snapshot, so inline kit changes cannot reuse an old search result.

These tests are registered in `bash scripts/test.sh`, including the routing/freeze support dependencies required by current project validation.
