# External plug-in state in Maschine Groups

`MaschineGroupPluginState` extracts the explicit plug-in identity and saved state from a decoded `PluginHost` module. It is a file reader. It does not instantiate plug-ins, choose replacement formats, change vendor state, or establish that a preset sounds correct in SuperTracker.

## Evidence and supported layout

The member order was checked against the local Maschine **3.7.1** native serializers (`PluginHost::load`, `MASCOMMON::DATA::PluginChunk::load`, and `HOSTING::PluginChunk::read`). A read-only pass over 90 groups in the installed Deep Matter and Molten Veil libraries decoded all 48 external modules:

| Saved identity | Records | VST2 ID |
| --- | ---: | --- |
| `Native Instruments:VST:INSTR:1315523937` | 24 | `4E694D61` (`NiMa`, Massive) |
| `Native Instruments:VST:INSTR:1315525174` | 24 | `4E695236` (`NiR6`, Reaktor 6) |

These fixtures use archive versions 12, 15, and 17, PluginHost version 1, chunk version 1, and chunk type 1. Payload sizes are 3,162–12,962 bytes. The inventory was read-only; no NI application or plug-in was executed.

The proven PluginHost order is its base Module, four Boolean parameters, the unique identity string, a display name, a PluginChunk, a preset index, and later parameter/window objects. Modern Boolean records have four consecutive object identities; legacy records have three. The reader validates those identities and their Boolean values before reading the strings. It does not search a binary blob for a plausible name.

The archive supplies a bounded chunk byte count. Inside that chunk are three little-endian 32-bit fields: version, type, and payload length. The payload length must equal the chunk size minus its 12-byte header. The helper removes only this proven wrapper and returns the remaining bytes unchanged. It retains the native chunk type and preset index rather than treating all state formats as interchangeable.

## API

```swift
let state = try MaschineGroupPluginState.read(archive: archive, module: module)
let metadata = state.dictionary // JSON-safe identity, type, byte count; excludes state
let originalBytes = state.payload
let documentState = try state.documentState(bundlePath: verifiedInstallationPath)
let plugin = try state.descriptor(owner: "sound:kit-id:sound-id", slot: 0,
                                  bundlePath: verifiedInstallationPath,
                                  enabled: module.enabled)
```

`vst2UniqueID` is derived only from an explicit decimal `VST` identity with an `INSTR` or `FX` role. Display names never establish identity. Other technologies and identities remain available as their original string but do not acquire a guessed VST2 or VST3 identity. The helper does not reinterpret AU state. Metadata includes `requiresHostStateValidation: true`.

PluginHost versions other than 1 and chunk versions other than 1 are rejected. The default state limit is 32 MiB. Truncation, inconsistent lengths, invalid member IDs, noncanonical integers, invalid Boolean values, and invalid UTF-8 fail explicitly. Remaining PluginParameters and window members are not converted into controller mappings.

`documentState` accepts only an identified VST2 bank (chunk version 1, type 1), a nonempty bounded payload, and a program index representable as a signed 32-bit integer. It returns the existing `SuperTracker.VST2` property-list shape. Its data consists of the adapter's 24-byte `STV2` header followed by the original payload: version, exact VST2 identity, bank kind, saved program index, and byte count are encoded explicitly in little-endian order. Other chunk types remain extractable for inspection but cannot produce a loadable descriptor.

`descriptor` wraps that document as the normal base64 binary property-list `state`, sets the exact `vst2:XXXXXXXX` component and instrument/effect role, and preserves the source metadata and restoration warning. Its optional installation path must already be canonical and absolute; the helper never finds a bundle by display name. The existing host remains responsible for resolving the component and restoring it. Calling either helper does not execute a plug-in.

For bank restores, the VST2 adapter now applies the saved program after loading the bank, because a bank load may reset the selection. It dispatches the selection only when the program is within the loaded plug-in's current program count. The bank payload is unchanged. `copy_state` already stored the selected program separately from those bytes.

## Host integration limits

The existing read-only SuperTracker catalogue contains Massive 1.7.0 with class ID `5653544E694D616D6173736976650000` and Reaktor 6.5.0 with class ID `5653544E6952367265616B746F722036`. Those IDs explicitly encode their VST2 predecessors. This is identity evidence from the cached catalogue, not a fresh vendor load or a recall test.

Maschine's VST3 migration implementation first wraps legacy state using the bundled Steinberg SDK's `VST3::writeVst2State`, then attempts the raw stream as a fallback. The SDK source is `Vendor/vst3sdk/public.sdk/source/vst/utility/vst2persistence.{h,cpp}`. SuperTracker's existing `st_vst3_restore_component_state` still sends the supplied stream directly, including its existing NKS path.

The separate `st_vst3_restore_vst2_bank` C API and `VST3AudioUnit.restoreLegacyVST2Bank(_:uniqueID:program:)` now offer an explicit migration route. They accept only a nonempty bank of at most 32 MiB, a nonzero exact VST2 identity and a nonnegative saved program. The loaded class ID must begin with `565354` followed by that exact eight-digit predecessor identity. A matching display name does not authorize migration. Selection of an installed replacement and validation of the saved `STV2` descriptor belong to the rack before it calls this method.

The helper writes Steinberg's FXB bank wrapper, including bypass metadata, exact predecessor identity and selected program, around unchanged vendor bytes. It sends the stream through the current component/controller restoration lifecycle. Only an explicit component rejection followed by successful lifecycle checks allows one retry with the original bytes. Controller synchronization, audio layout, suspension/reactivation or reentrant-operation failures stop instead of retrying. The returned Boolean records a successful raw fallback; failures do not report success. The raw route cannot carry a separate program index; selected-program behavior then depends on the vendor chunk and must be checked during native recall validation. Rendering must be paused and drained by the caller. A subsequent native VST3 capture is needed to persist the migrated instance in its own format.

A failed restoration blocks capture for every VST3 class, including classes without a special state-settling profile. This prevents saving a known incomplete restore. Reloading the component or explicitly restoring a valid state is required before capture becomes available again.

This is a compatibility path, not proof of vendor recall. The installed Intel-only Massive and Reaktor VST2 bundles did not load in the native test process, which motivated this separately validated VST3 route. Actual installed VST3 restoration and audible recall remain gated by a controlled native check.

Some Reaktor payloads contain original Windows paths to `Monark.ens`. These paths remain opaque and unchanged. A matching Reaktor identity alone cannot demonstrate that its ensemble/content dependencies resolve. No claim of working external-instrument group recall follows from successful extraction.

## Tests

`scripts/test-maschine-plugin-state.sh` passes 1,122 generated, file-only checks with no installed NI dependency. These cover modern and legacy member order, first-use class records, exact identities including signed-bit IDs, unchanged payloads, the binary property-list and every `STV2` header field, owner/slot bounds, unknown formats, and malformed or oversized records.

`scripts/test-vst2-bank-state.sh` passes 149 checks through the public host API using an original private VST2 fixture. It proves selection happens after a bank reset for programs 0, 3, and 7, rejects dispatch for -1, 8, and `INT_MAX`, retains exact bank bytes through save/restore, and renders no audio. A private copy of the pre-fix host fails at program 3 selection, confirming the regression detects the defect. These tests do not establish installed Massive or Reaktor recall; that requires a separate controlled native check. The installed-library inventory above remains read-only research evidence and is not part of the default test suite.

`scripts/test-vst3-legacy-bank.sh` passes 703 checks with original in-memory components/controllers/processors; it loads no bundle and renders no audio. It validates the SDK wrapper's complete identity/program/payload, controller stream synchronization, one raw retry after component rejection, ordinary VST3 state capture/restore after migration, invalid identity/size/program bounds, and no retry on controller, layout or lifecycle failure. It also proves failed restores block unprofiled capture without requesting component state and that an explicit valid restore re-enables native state capture. Existing component/controller state, core host and save-flush regressions pass on the updated host.

`scripts/test-vst3-module-exit.sh` verifies final module cleanup using an original private VST3 fixture and normal process exits. Explicit success and error paths release the factory and call `bundleExit` before plug-in static destruction; the missing-cleanup negative control demonstrates the reverse ordering. Both instances remain usable until released, and repeated cleanup is idempotent. No audio is rendered. The Maschine diagnostic now explicitly releases unused modules on both success and error, matching the normal app quit path and existing VST3 diagnostic. Functional state/PCM checks alone do not establish clean process teardown; the installed native fixture must also exit successfully.
