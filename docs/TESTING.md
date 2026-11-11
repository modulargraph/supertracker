# Running the tests

```sh
scripts/test.sh          # everything below, in this order
scripts/test.sh rust     # the Rust crates and the plug-in container
scripts/test.sh ui       # the interface tests (node)
scripts/test.sh e2e      # the interface in Chromium against the real host
```

No test needs a sound card, a MIDI device, a display or an installed plug-in. Tests run the host with
`SUPERTRACKER_AUDIO_DEVICE=null` (a device that renders to nowhere at a fixed rate) and private support folders, so
they never touch the user's settings, library or recovery files.

## Suites

| Suite | Command | What it covers |
|---|---|---|
| Engine | `cargo test -p tracker-engine` (add `--release` for the build the recorded PCM hashes and CPU budgets refer to) | Sequencer, voices, sampler modes, clock and tempo map, built-in effects and drum models, freeze spans, input acknowledgements. A counting allocator fails a test when a render call allocates. |
| Host crates | `cargo test --workspace` | Project files and validation (`st-project`), decoding, edits, slicing and export (`st-samples`), library index, NKS and kits (`st-library`), MIDI with in-memory ports (`st-midi`), the Maschine MK3 protocol and screens against an in-memory controller (`st-mk3`), control socket and MCP (`st-agent`), mixer graph, offline render, recording, freeze and plug-ins in the graph (`st-audio`), every bridge command (`st-host`), command line, WebSocket and menus (`st-app`). |
| Plug-in container | `cargo test --manifest-path crates/plugin-host/Cargo.toml` | Shared-memory transport, supervisor, crash and hang recovery, the container binary with CLAP and VST3 test plug-ins built by the same workspace. Audio Unit hosting is compiled on macOS only and has no test that runs elsewhere. |
| Interface | `node tests/<name>_tests.js` (all: `scripts/test.sh ui`) | The interface's scripts loaded into a simulated page (`tests/helpers/`): keyboard handling, state changes, Undo, rendering. No browser, no audio. |
| Browser end to end | `node tests/host_e2e/run.js` ([details](../tests/host_e2e/README.md)) | The real page in Chromium against `supertracker --serve`: boot, save and open through scripted file choosers, playback on the null device, the library, a plug-in in a container, AI control over `--mcp` and `--ctl`, clean shutdown on signals. Set `SUPERTRACKER_BIN` to the executable to test; otherwise the script looks in `target/`. |
| Window | `tests/host_e2e/window_smoke.sh` | Linux only, under Xvfb with `xdotool` and `node`: the real window loads the page, the message channel, a menu shortcut, a native context menu and file chooser, screenshots for AI clients (`--ctl` and `--mcp`; the PNG files are decoded and checked), Quit. Not part of `scripts/test.sh`. |

Benchmarks are `#[ignore]`d tests that print timings and are run one at a time, for example
`cargo test -p tracker-engine --release --test engine_benchmark -- --ignored --nocapture`, and
`cargo run --release -p st-audio --example bench` for the full graph.

## Generated files that tests guard

| File | Regenerate with |
|---|---|
| The command tables in `docs/HOST.md` | `UPDATE_HOST_DOC=1 cargo test -p st-host --test host_doc` |
| `tests/fixtures/builtin-effects.json` (the built-in effect catalogue the interface tests read) | `UPDATE_EFFECT_METADATA=1 cargo test -p tracker-engine --test effects_metadata` |

`cargo test` fails while either file disagrees with the code.

## Checking the macOS code from Linux

The macOS-only code (the window's AppKit parts, Core Audio through `cpal`, CoreMIDI through `midir`, Audio Unit
hosting in the container) can be type-checked without a Mac or an SDK:

```sh
rustup target add aarch64-apple-darwin
cargo check -p st-audio -p st-host --target aarch64-apple-darwin
CC_aarch64_apple_darwin=clang cargo check -p st-app --target aarch64-apple-darwin
CC_aarch64_apple_darwin=clang cargo check --manifest-path crates/plugin-host/Cargo.toml --target aarch64-apple-darwin --all-targets
```

A type check is not a test. Nothing macOS-specific has been linked or run; the first run on a Mac should follow
the checklists in [PLUGIN_CONTAINER.md](PLUGIN_CONTAINER.md) and the list of unexercised parts in
[HOST.md](HOST.md#what-has-and-has-not-been-exercised).

## Writing tests

- Timing-sensitive assertions must tolerate a loaded machine: compare against generous windows or a fixed clock,
  never a few milliseconds of wall time.
- A test that needs a folder creates a private temporary one and removes it; `st_host::testing::TempDir` and
  `TestHost` do this for host tests.
- File choosers cannot open in a test. `--dialog-answers <file>` (or `SUPERTRACKER_DIALOG_ANSWERS`) answers them
  from a JSON array: each chooser takes the next element, a path or `null` for Cancel.
- Interface tests play the host through `tests/helpers/host.js`; a new host command needs its answer there only
  when a test depends on it.

## Not covered by any test

Audio on a real device, real MIDI devices, a Maschine MK3 controller, third-party plug-ins, Audio Units, and
everything else that only exists on macOS. Nobody has listened to the output: the engine tests measure rendered
audio, they do not hear it.
