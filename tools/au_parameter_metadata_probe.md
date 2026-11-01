# Audio Unit parameter metadata probe

Run from the repository with exact installed AU names (case insensitive):

```sh
scripts/probe-au-parameter-metadata.sh Digitalis 'ARP ODYSSEY'
```

This opt-in probe loads only the named plug-ins, each in a separate process with a 40-second timeout. It calls the production parameter metadata and generic-editor display helpers for every exposed parameter. It creates no audio engine, opens no audio device, and plays no audio. The result is JSON with parameter counts, success, and subprocess exit codes; the command exits nonzero for missing/ambiguous plug-ins, failures, crashes, or timeouts. Compilation needs the macOS Swift toolchain; component discovery may require running outside a restricted sandbox.

The probe covers the October 23, 2026 Digitalis and ARP ODYSSEY crashes: the AUv2 bridge forwarded a null `string(fromValue:)` value pointer to vendor code. Production formatting now supplies the current value explicitly. The normal test suite exercises this pointer contract without requiring vendor plug-ins. This optional probe verifies the installed implementations and is intentionally excluded from the default suite.
