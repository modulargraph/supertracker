# Running the tests

`scripts/test.sh` runs every test step in parallel (one job per test binary or script) and caches compiled test
binaries, so unchanged tests are not rebuilt. The steps themselves are listed, in plain shell, in
`scripts/test-steps.sh`; add new tests there.

| Command | What it does |
|---|---|
| `scripts/test.sh` | Full suite, parallel (about 4 minutes on a 10-core Mac; was 15–30 minutes serially). |
| `scripts/test.sh --changed` | Only the jobs whose inputs changed (working tree vs HEAD, plus the branch vs master). Changes to the runner, the step list or files no job names fall back to a full run. |
| `scripts/test.sh --only mk3` | Jobs whose name or command contains the pattern. |
| `scripts/test.sh --list` | List jobs and how many input files each one depends on. |
| `scripts/test.sh -j 4` / `--serial` | Limit parallelism. |
| `scripts/test.sh --native` | Also run the slow native app self-tests (`scripts/test-native.sh`) after the suite. |

Each job's output is in `.build/test-logs/<job>.log`; failures print the last lines of their log. Jobs that take
longer than 20 seconds are listed as they finish. Timing-sensitive assertions must tolerate parallel load: compare
against generous windows or a fixed clock, never a few milliseconds of wall time.

How `scripts/testrun.py` reads `scripts/test-steps.sh`:

- Lines before the first test (module maps, shared object files, the `freeze_test_support` array) run once, first.
- A `swiftc`/`clang++ … -o .build/test-bin/NAME` line starts a job; later lines that start with
  `.build/test-bin/NAME` run in that job. Put the compile and the run on separate lines: the compile is skipped when
  its command and input files are unchanged, and a run written on the compile line is skipped with it.
- Every other line (`bash scripts/…`, `python3 tests/…`) is its own job, and each `for SCRIPT in …` loop becomes one
  job per file.

### Temporary output

Test scripts that need a scratch folder create it with `mktemp -d` and remove it with an `EXIT` trap. Scripts that
build under `test-output/` (`test-kit-library-migration.sh`, `test-vst2-*.sh`, `test-vst3-*.sh`, `test-native.sh`)
delete the folder when they pass and keep it, with its path printed, when they fail. Other scripts write to fixed
folders under `test-output/` or `.build/test-output/` that each run overwrites. Both folders are gitignored; nothing in
them is kept as evidence, so docs should not link into them.

## Native self-tests

`scripts/test-native.sh` (or `scripts/test.sh --native`) builds a private debug app in a new
`test-output/native-regression.*` folder and runs its
self-test modes (`--self-test`, `--kits-self-test`, `--inserts-self-test`, `--ui-bridge-self-test`, `--audio-host-fixes-self-test` with a freshly built original VST3 fixture, …) with private
support folders and `SUPERTRACKER_NO_MK3=1`. The self-test code (`Native*Tests.swift`, `NativeSelfTest.swift`,
`NativeUIBridgeSelfTest.swift`, `NativeUIDuplicateFixture.swift`) is wrapped in `#if SELFTESTS`:

Self-tests that need installed third-party content find it on their own where they can and otherwise take its location
from the environment: `--maschine-kits-self-test` looks up the Deep Matter expansion through Native Instruments'
registrations (or `SUPERTRACKER_NI_CONTENT_DIR`, the folder holding expansions), the VST2 NKS stage of
`--vst2-self-test` needs `SUPERTRACKER_DX7_NKS_PRESET`, and `scripts/test-kit-groups-migration.sh` imports a real kit
only when `SUPERTRACKER_1988_KIT` names it. Maschine MK3 support is off by default (Settings), so the controller is
never opened unless a test's support folder turns it on.

| Build | Self-tests |
|---|---|
| `scripts/build.sh debug`, `swift build` (debug) | Included (`SELFTESTS` is defined for the debug configuration in `Package.swift`). |
| `scripts/build.sh release` | Left out. A `--…-self-test` flag exits with status 64 instead of opening the app. |
| `scripts/build.sh selftest [App.app]` | Optimised build with the self-tests, for performance runs such as `--kits-performance` and `--render-performance` (full-graph render cost of a kit session and 64 tracks, in percent of real time). Uses its own `.build-selftest` tree so the release objects stay warm. |

Diagnostic builders that compile these files directly with `swiftc` (`tools/build_*_diagnostic.py`,
`scripts/test-native-host-regressions.py`, …) pass `-DSELFTESTS`. The hidden UI script
runner (`DevHarness.swift`) and `--ui-keyboard-session` stay in every build.

## Builds

Every build reuses one warm build tree: `scripts/build.sh debug` and `scripts/build.sh release` both use
`--scratch-path .build` (objects in `.build/debug` and `.build/release`), whatever app path you build to, and
`scripts/test-track-freeze.sh` shares `.build` too. Don't build into a fresh scratch path unless you really need a cold
build; `scripts/build.sh` prints how long the compile took.

The Swift sources all live in `Sources/SuperTracker`, but `Package.swift` splits them into modules with disjoint source
lists, so an edit only recompiles its own module and the modules that depend on it:

| Module | Contents | Depends on |
|---|---|---|
| `SuperTrackerCore` | project model and store, media relinking, export, samples, MIDI files, Maschine group mappings | C++ targets |
| `SuperTrackerLibrary` | `LibraryService`, NKS metadata and presets, kit library | Core |
| `SuperTrackerMK3` | Maschine MK3 driver, protocol, screen and probe | — |
| `SuperTrackerAgent` | agent control protocol, MCP stdio proxy | — |
| `SuperTracker` | the app: audio host, plug-in hosting, bridge, UI, self-tests | all of the above |

The libraries are imported implicitly as `@testable` (`-testable-import-module` in `Package.swift`) and built with
`-enable-testing`, so moving a file between modules needs no `public` annotations or import lines, and every file
still compiles as part of a single module with plain `swiftc` (which the test scripts do). To move a file into a
library, add it to that library's list in `Package.swift`; it may only use code from the modules listed under
"Depends on".

Measured on a 10-core Apple Silicon Mac (Swift 6.4 toolchain), October 2026, before and after the split and the
self-test exclusion:

| | Before | After |
|---|---|---|
| Clean release build (`swift build -c release`, cold scratch path) | 586 s | 124 s |
| Release rebuild after editing one app file (`Bridge.swift`) | 338 s | 111 s |
| Release rebuild after editing a Core / Library / Agent / MK3 file | 338 s | 125 / 126 / 120 / 6 s |
| Release app binary | 14.4 MB | 9.6 MB |

Debug rebuilds of a single edited file usually take 3–10 s; the build system sometimes recompiles the whole app module
instead (about 90 s).

## Everyday workflow regressions

Run `./scripts/test.sh --native` after changes to editing, loading or audio hosting.
The regular suite includes delayed-response browser navigation, first-click keyboard
and record focus, single-cell/block drag and copy, instrument destination preservation,
track/kit ownership after deletion, persistent ratchets, sample reverse/position, and
MIDI export parity. Native tests add actual WebKit layout/hit testing and bridge
acknowledgements, two samples followed by a fixture AU on a new lane, independent
plug-in reassignment, recovery Enable / Retry, externally stopped audio restart, and (`--render-restart-self-test`,
on the real output device, muted) a rack pause such as an insert reorder or VST3 state capture, which restarts the
mixer clock at zero and must not freeze the transport or note entry. `--live-publish-self-test` (original VST3
fixture, offline graph) edits notes, captures plug-in state as autosave does and publishes the adopted snapshot while
playing a 16-pad group with 53 inserts: the rack must issue no graph pause, routing must not be re-planned for
note-only edits, transport must advance exactly once per block, and a delivered VST3 edit must be in the next live capture.
`--deferred-parameters-self-test` (offline graph; skips unless NI Raum, or the AU named by
`SUPERTRACKER_DEFERRED_PARAMETER_AU`, is installed) loads it on a return bus nothing is routed to: the insert must
render, a burst of parameter writes must read back after rendering (Raum applies writes on its next render and the AUv2
bridge's cached values never follow), and the state captured then must hold them.
Each native mode has a ten-minute timeout and uses private project/recovery storage.

The WebKit report records which interactions use synthetic DOM events; these verify
real mounted handlers and native responses but do not claim physical keyboard or
hardware hotplug coverage. Screenshots and JSON reports are written to the
`test-output/native-regression.*` folder, which is deleted when every mode passes and kept when one fails. `webkit-interface-report.json` includes
`patternWorkflows`, and `pattern-workflows.png` shows the resulting notes.

## Hardware I/O regressions

The native runner also executes `--line-input-self-test` and
`--midi-output-self-test`. These use synthetic six-channel audio and private MIDI
endpoints, so they need macOS Audio Unit/CoreMIDI access but no connected hardware.
They check independent mono/stereo routes, track mix and routing rebuilds, exact
recording channel extraction, sequenced/live MIDI output, route changes and panic.

`line_inputs_ui_tests.js` and `line_input_app_guards_tests.js` cover permission waits,
rejected acknowledgements, inactive saved-route retries, stale sessions, graph update
ordering, and undoable input edits. Mounted WebKit checks open both hardware dialogs
through their actual pattern buttons and verify canceling does not activate capture.

### Recording workspace and physical input regression

The `--ui-kits-self-test` run now exercises the embedded Recording workspace,
checks the controls at 1500×940 and 1100×760, saves screenshots, and presses Record
and Stop to capture master audio into a new sample lane. It checks navigation
protection, draft preservation, Undo, and preservation of existing instruments.

Synthetic channel fixtures do not test macOS device selection. To explicitly test
the current physical microphone with a debug build (`scripts/build.sh debug build/Debug/SuperTracker.app`), run:

```sh
SUPERTRACKER_SUPPORT_DIR="$PWD/test-output/input-private-support" SUPERTRACKER_NO_MK3=1 \
  build/Debug/SuperTracker.app/Contents/MacOS/SuperTracker \
  --audio-input-self-test test-output/physical-input
```

This opt-in test requests microphone access if needed and records two twelve-second takes
locally with monitoring off. It verifies that selecting/reselecting the
microphone preserves the output device, creates a nonempty mono WAV, and restores
input settings. It writes `hardware-input-report.json` and the capture in the
specified output folder. It is deliberately not part of unattended default tests.

To cover physical input through the mounted Recording UI and its sample attachment,
set `SUPERTRACKER_TEST_MIC_CAPTURE=1` when running `--ui-kits-self-test`. This opt-in
variant records eight seconds from the selected input using the actual Record and
Stop controls, verifies new sample/lane creation and Undo, and retains other kit
interaction checks. The ordinary mode continues to use a brief master capture.

### Continuous project-building acceptance workflow

`--ui-workflow-self-test <output-directory>` runs the production WebKit interface
against an isolated native Bridge and audio engine. It starts with New Session,
searches generated samples in the real browser, adds independent instruments,
edits sampler controls, records notes and chords, applies FX, duplicates a pattern,
creates a kit group with two pads, loads a sample into each through the browser,
records a hit on each with Pad keys, collapses and expands the group, and adds a track during playback. It then
checks Undo/Redo, renders both patterns to PCM, and saves/reopens the project.
Reports and screenshots are written under `mounted-workflow/`. The serial native
runner includes this workflow. Keyboard events are synthetic DOM events; file
Save/Open use explicit fixture paths rather than automating system file panels.
Audio assertions use native meters and decoded PCM, not subjective listening.

The native runner uses `scripts/run-native-test.py` to bound each suite. A timeout
captures the still-running process with macOS `sample` into `native-timeout.txt`
before terminating only that test process. Existing JSON reports and screenshots
remain available alongside the stack sample. This makes audio/render deadlocks
diagnosable without leaving a stuck test app running.

### Kits (track groups)

- Node tests:
  - `tests/track_groups_model_tests.js` covers the group model (create, contiguity, reorder, move, dissolve).
  - `tests/track_groups_ui_tests.js` covers the header, collapse, Pad keys, ordinary editing across members and the commands.
  - `tests/track_groups_mixer_tests.js` covers group strips.
  - `tests/kit_groups_ui_tests.js` covers Load/Replace/New/Remove kit as single Undo steps.
  - MK3 group behaviour is in `tests/mk3_groups_tests.js` and its siblings.
- `scripts/test-kit-groups-migration.sh` converts old kit-lane projects. It uses the installed "1988 Kit" Maschine group when present, and `SUPERTRACKER_KIT_MIGRATION_FIXTURE` for a saved session. It also round-trips Save kit and Load kit.
- `tests/engine_track_choke_tests.cpp` checks per-track choke and voice modes.
- `tests/AudioRoutingIncrementalTests.swift` adds a kit (a new group bus and sixteen tracks) to a rendering graph with delay and reverb returns, and also adds tracks to an existing group bus.
- `--kits-self-test` (`NativeKitHostTests`) runs offline. It loads a sixteen-pad kit while four tracks play and requires all of the following:
  - incremental routing, with no rebuild;
  - no dropped events and no gap in the playing tracks;
  - every pad sounding live;
  - member stems that sum to the bus;
  - choke within the group, group preview, and insert identity across a group move;
  - plain-project transitions;
  - a migrated kit-lane project.
- `--ui-kits-self-test` (`NativeUIKitsTests`, also part of `--ui-bridge-self-test`) runs on the real-time engine. It covers:
  - New kit with sixteen pads, then Save kit into the isolated library;
  - Load kit while playing: one Undo step, incremental routing;
  - Undo/Redo in the native graph;
  - Replace kit keeping the notes;
  - group preview;
  - collapsed, expanded and mixer screenshots in `mounted-kits/`.
- When a live change falls back to a full rebuild, `routing.status` reports why in `lastFullRebuildReason`.

### Insert chains and the lean routed graph

- `scripts/test-builtin-effect-adapter.sh` (`tests/BuiltinEffectsAudioUnitTests.swift`) renders every built-in effect
  in chains and as separate nodes with randomized parameters, scheduled automation (absolute, immediate, ramps), bypass
  toggles, UI parameter changes, in-place reorder/remove/add and state round trips: the chained output must equal the
  per-node output (it is bit-identical). It also checks that `scheduleParameterBlock` events land exactly where
  AUAudioUnit's own event delivery would.
- `tests/AudioRoutingIncrementalTests.swift` covers direct-source strips (rewired and grown live) and delay/reverb send
  routes joining running returns.
- `--insert-chain-self-test` (`NativeInsertChainTests`, in `scripts/test-native.sh`) renders a routed project with
  lanes, a sidechained and a latent insert, bypass, parameter changes and solo through chains and through the per-node
  graph and requires identical output, meters, states, latency and automation offsets; then loads, replaces, removes
  and reorders built-in inserts while playing with no pause and no rebuild, and checks send routes.
- `--bus-insert-automation-self-test` (`NativeBusInsertAutomationTests`, in `scripts/test-native.sh`) puts lanes on
  bus and master inserts in slots > 0 whose neighbours share the parameter address (five chained built-ins, an Apple AU
  ahead of built-ins, master inserts), loaded and while playing after a return bus and its insert are added live: each
  lane must move its own insert and leave the neighbour untouched. It also loads an insert on a new bus that sorts ahead
  of an automated bus (renumbering the host destinations behind it) and renders between the routing table and the
  engine project publication: the old-numbered lane must not reach the new insert (STEvent.hostMap).
- `--kit-dsp-measurement` takes `ST_KIT_DSP_BLOCKS` (for example `128,256,512`) and prints attached and rendered node
  counts; `--render-performance` prints rendered nodes too.

## Manual tests, benchmarks and diagnostics

These are not part of `scripts/test.sh`, usually because they need installed plug-ins, hardware, a full app build,
or produce deliberate crashes or timings rather than pass/fail results.

| File | Purpose |
|---|---|
| `scripts/test-track-freeze.sh` | Builds the app and runs the native Freeze/Unfreeze checks. |
| `scripts/test-native-host-regressions.py`, `scripts/test-native-agent-termination.py` | Original-fixture host regressions and agent termination, built from private source snapshots. |
| `scripts/benchmark-engine-dsp.py` (`tests/engine_dsp_benchmark.cpp`), `tests/engine_benchmark.cpp`, `tests/LibraryIndexBenchmark.swift` | Engine DSP and library index benchmarks. |
| `tests/vst3_smoke.mm` + `tests/vst3_smoke_runner.py` | Standalone smoke test of one installed VST3 (see [VST3_HOST.md](VST3_HOST.md)). |
| `tests/vst3_guard_tests.py` | Checks that the diagnostic fatal-signal guard (`tests/vst3_signal_guard.h`) reports crashes as failures. It crashes on purpose, which leaves macOS crash reports. |
| `tests/vst3_capture_fixture.sh` | Builds the asynchronous-capture VST3 fixture for `tools/run_vst3_native_test.py --capture-fixture-bundle`. |
| `tools/build_*_diagnostic.py`, `tools/run_*.py` | Diagnostic executables with self-tests for NKS, routing and VST3 timing work. |

`tests/build_unit_timing.sh`, `tests/build_unit_timing_events.sh` and their sources belong to the parked tempo-map
feature and go when it is removed.
