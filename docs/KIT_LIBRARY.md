# Kit library API and package storage

`LibraryService` indexes Maschine `.mxgrp` files and SuperTracker `.stkit` package directories with `library.scan`. Existing cancellation, cache, progress/error events, favorites, search paging and offline-root handling apply. Kits are returned only for `library.search` with `kind:"kit"`; the legacy All sounds view retains sample/NKS activation behavior.

Kit search uses the existing OR-within/AND-across structured filters: `collections` for source, `packs` for expansion, `categories` for type, and `tags`. The user-only filter is `extensions:["stkit"]`. Text includes name, hierarchy, type, tags and expansion. Results add `kitFormat` (`supertracker-kit` or `maschine-group`), `source`, `expansion`, `type`, `user`, optional `soundCount`, optional `previewPath` and optional `metadataError`. Existing `available:false` marks a disconnected root. Malformed metadata stays visible with its error; it does not silently disappear.

Default roots include the installed Native Instruments content folders (from Native Access registrations and `~/Documents/Native Instruments`) and `~/Documents/SuperTracker/Kits`; other folders are added with **Add group / kit folder**. With `SUPERTRACKER_SUPPORT_DIR`, the user-kit root is `<override>/Kits`; explicit fixture roots never discover the user's libraries. The destination folder is created by the caller when saving a first kit.

Older version-1 indexes can contain samples and NKS presets but no kits. Loading one now starts a cancellable kit-only upgrade in the background; cached sounds, favorites and user labels remain available. The pass inspects `.mxgrp` paths and small `.stkit` headers without reading sample audio, NKS state or kit project state. Successful roots are recorded in the optional `kitIndexedRoots` cache field, including roots that legitimately contain no kits, so a completed upgrade is not repeated at every launch. Offline, failed and cancelled roots are not marked complete. Mounting a previously offline root or searching after it becomes available retries discovery; an explicitly cancelled upgrade stays stopped until manual Scan or a later launch. Existing root sound counts are preserved during this kit-only pass.

Search, status and progress/results events include `kitIndexUpgradePending`, `kitIndexUpgradeScanning`, and `kitIndexUpgradeOfflineRoots`. Pending includes offline, failed and cancelled work; only the scanning flag means an upgrade is currently running. A missing kit index never requires deleting the existing cache.

## File-only Swift API

```swift
KitLibrary.metadata(at: URL, root: URL? = nil) throws -> [String: Any]
KitLibrary.read(at: URL) throws -> [String: Any]
KitLibrary.save(project: [String: Any], metadata: [String: Any], to: URL,
                copySamples: Bool = true, relativeTo: URL? = nil,
                overwrite: Bool = false, cancelled: () -> Bool = { false }) throws -> [String: Any]
KitLibrary.previewURL(for: URL) -> URL?
KitLibrary.userDirectory // URL
```

`metadata` reads only the small package header; for `.mxgrp`, it currently infers filename/folder metadata and finds an exact associated preview. It does not decode the group or assess plug-in compatibility. `library.kit {path}` exposes this metadata asynchronously. Root Bridge's kit detail/load operations combine this with the separate Maschine importer or project validator.

Kit files keep the sixteen-Sound layout below. In a session a kit is a track group: the Bridge's `kit.read` adds `groupProject` (the kit converted by `KitGroups.groupProject`), and `kit.save` takes a `groupID` and converts the group back (`KitGroups.kitProject(fromGroup:)`) before this module saves it.

`read` supports `.stkit` and returns `{path,metadata,project,sounds,warnings}`. Sound summaries contain zero-based `slot`, `id` when present, `name` and `type`. It resolves package-relative sample paths while preserving the saved project subset and opaque plug-in state. The caller validates musical ownership/routing and decides how to apply it; this module never changes a running session.

`save` receives a captured, caller-validated project subset, including `kits[].sounds[].instrument`, channel settings and `audioPlugins`. `relativeTo` is the original project **file URL**, used only for existing relative sample references. Results contain `{saved,path,metadata,copiedSamples,mediaBytes}`. `metadata.previewPath`, when supplied, copies that existing audio preview into the package; absence means no preview. No kit rendering or playback happens here.

## Package format and bounds

A `.stkit` is a directory containing:

- `kit.json`: at most 256 KiB of metadata, with `format:"supertracker-kit"`, numeric `version:1`, `projectFile:"project.json"`, name/type/tags, sound count and optional `previewFile`.
- `project.json`: at most 64 MiB of captured project subset, including opaque plug-in states.
- Optional `Samples/` with content-addressed collected media, and an optional preview file.

Indexing skips all package descendants and reads only `kit.json`; it never indexes the package's collected sample copies or reads its plug-in states. Existing cached kit fields survive offline rescans and inference upgrades.

Collection handles the explicit instrument sample path, both zone path aliases, inline kit Sound instruments and clip/take media. Strings inside opaque plug-in state are untouched: plug-in-managed assets are not automatically collected. Without collection, sample references become absolute and retain their original locations. With collection, missing/unreadable/empty media causes an error rather than silently claiming a portable save.

Media copies use 1 MiB chunks, cancellation checks and a 4 GiB cumulative limit. Source-size/time changes abort the save. Metadata and project JSON are written into a staging package; exclusive rename publishes a new destination, and atomic exchange replaces an explicitly approved existing valid kit. A failed or cancelled pre-publication operation leaves the destination intact. Reference-only replacement refuses samples inside the package being replaced, since discarding those members would break the new references; collect them or choose another destination. Package-relative traversal and symlink escapes are rejected when reading.

Preview lookup checks a fixed candidate list next to the group, `.previews` and `Previews`, using exact `name.mxgrp.ogg`/`name.ogg` and supported audio-extension variants. It never enumerates the preview directory, decodes audio or chooses a similarly named kit. The browser/native preview controller owns request-generation cancellation and playback; missing/offline previews remain unavailable.

## Generated-fixture verification

Run `bash scripts/test-kit-library.sh`. Its two executables create and remove private temporary fixtures only. They cover copied root/zone paths, portable moves, exact opaque-state preservation, sample-name collisions, overwrite/cancellation/copy failures, malformed versions/JSON, path/symlink escapes, size bounds, exact preview discovery, metadata-only indexing, facets/text/user-only/favorites/paging, cache reload and offline/cancelled scans. No NI application, vendor plug-in, audio host or user library is executed or modified.

`bash scripts/test-kit-library-migration.sh` adds generated checks for upgrading a sample-only v1 cache without a Scan command. It verifies kit discovery, exact original sample/NKS cache fields, unchanged favorite/metadata/browser files, completed-root reuse, same-session offline reconnection, and cancellation followed by a complete retry on relaunch. Deterministic stale-candidate cases remove or complete a root between discovery and scan startup: automatic work cannot re-register or unexclude it, while an explicit user Scan retains intentional re-add behavior. Roots and caches are private temporary directories.
