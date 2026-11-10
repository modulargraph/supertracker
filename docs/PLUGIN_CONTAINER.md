# Plug-in container: out-of-process plug-in hosting

Plug-ins run in separate processes ("containers"), as in Bitwig Studio, so that a crashing or hanging plug-in cannot take the application down. The code is the cargo workspace `rust/plugin-host/` (excluded from `rust/Cargo.toml`). Everything is Rust; there is no C API.

Status in one paragraph: the cross-platform core (transport, host-side client, container runtime, container executable with test plug-ins and a CLAP backend) is built and **verified by running on Linux x86-64** (57 tests, see [Verification](#verification-on-linux)). For macOS the whole workspace **type-checks** (`cargo check --target aarch64-apple-darwin --all-targets`), and nothing has run there; the macOS-only code paths are listed under [macOS](#macos-what-a-rust-host-needs-to-know). VST3 and Audio Unit backends, embedded editor windows and plug-in scanning are not part of this work.

## Overview

```text
 host application (Rust)                         supertracker-plugin-container (1..n processes)
┌───────────────────────────────┐               ┌──────────────────────────────────────────────┐
│ stpc-client                   │  Unix socket  │ stpc-runtime                                 │
│  Supervisor ── control ───────┼──────────────▶│  I/O thread ─▶ main thread: Plugin methods,  │
│   spawns, watches, restarts   │◀──────────────┼─ responses      editor windows, event loop   │
│                               │               │                                              │
│  Instance::process()          │ shared memory │  real-time thread per instance:              │
│   audio thread: write block, ─┼──── wake ────▶│   Processor::process() reads and writes the  │
│   wait (deadline), read block │◀─── wake ─────┼─  shared region directly                     │
│                               │               │                                              │
│  housekeeping thread ◀────────┼─ notification queue (in the region) ◀─ plug-in, any thread   │
└───────────────────────────────┘               │  backends: "test", "clap" (later vst3, au)   │
                                                └──────────────────────────────────────────────┘
```

| Crate (`rust/plugin-host/crates/`) | Kind | Contents |
|---|---|---|
| `transport` (`stpc-transport`) | lib | Shared region layout, notification queue, wake-up primitive, control channel, protocol messages and encoding, anonymous shared memory. |
| `client` (`stpc-client`) | lib | What the host application links: `Supervisor`, `Instance`, events. |
| `runtime` (`stpc-runtime`) | lib | What a container program links: `Runtime`, the `PluginBackend` / `Plugin` / `Processor` traits, `HostHandle`. |
| `container` (`stpc-container`) | bin + lib | The `supertracker-plugin-container` executable; backends `test` (`test_backend.rs`) and `clap` (`clap_backend.rs`); all integration tests. |
| `clap-test-plugin` | cdylib | A CLAP plug-in library (gain effect, sine instrument) used to test the CLAP backend. |

Dependencies: `libc` and `clap-sys` (both MIT OR Apache-2.0, no transitive dependencies). The workspace is GPL-3.0-or-later like the application.

## Processes and grouping policies

The host creates one `Supervisor`. It starts `supertracker-plugin-container` processes on demand and places each new instance by the current `GroupingPolicy`:

| Policy | Container key | Trade-off |
|---|---|---|
| `Shared` | one for everything | least overhead; one crash loses all plug-ins |
| `PerManufacturer` | `InstanceConfig::manufacturer` | a vendor's plug-ins fail together |
| `PerPlugin` (default) | format + plug-in id | all instances of one plug-in share a process |
| `PerInstance` | the instance | best isolation, one process per instance |

`set_policy` affects instances created afterwards; existing ones stay where they are. Containers inherit the host's standard output and error, so what a plug-in prints ends up in the host's log. A container process ends when its last instance is destroyed. The executable is found at `SupervisorConfig::container_path`, else `$STPC_CONTAINER_PATH`, else next to the host's own executable; it is always this standalone binary.

Start-up of a container: the supervisor binds a Unix socket at a fresh path inside its private directory (`stpc-<pid>-<random>`, mode 0700, under `socket_dir`, `$XDG_RUNTIME_DIR` or the temporary directory), starts the process with `--stpc-endpoint <path> --stpc-token <random>`, accepts one connection and removes the path at once. The container sends `Hello` (magic, protocol version, pid, token, wake-up capabilities, formats); the supervisor answers `Welcome` or a refusal. A version or token mismatch is refused, on both sides. Directories left by a host process that no longer exists are removed when a supervisor starts.

## Transport

### One shared-memory region per instance

Created by the host, handed to the container as a file descriptor over the control socket (`SCM_RIGHTS`). On Linux it is a sealed `memfd` (no name at all; the seals stop the plug-in process from shrinking it under the host). Elsewhere, and on Linux with `STPC_SHM=posix`, it is a POSIX shared-memory object whose name is unlinked in the creating call. Either way no named object outlives a process, whatever way the processes die.

```text
 Header (4096 bytes)
   static block     magic, protocol version, sizes               host, written once
   request block    req seq, frames, flags, event count,
                    input silence mask, transport                host, per block
   response block   rsp seq, flags, out-event count,
                    output silence mask, plug-in time (ns)       container, per block
   queue indices    notification queue head / tail
 events             [Event; event_capacity]          host -> plug-in, per block
 out events         [Event; out_event_capacity]      plug-in -> host, per block
 notifications      [cell; notification_capacity]    plug-in -> host, lock-free queue
 input channels     all input buses, max_frames f32 each, 64-byte aligned
 output channels    all output buses, likewise
```

* **Buses.** The host fixes the layout at creation: channels per input bus and per output bus (up to 8 buses and 64 channels per direction). Input bus 0 is the main input; further input buses are sidechains. A backend adapts the plug-in to this layout and reports how many channels of each bus the plug-in really uses.
* **Events** (`Event`, 32 bytes): MIDI (1 to 3 bytes, port, "sequenced" flag, optional note id) or a parameter change (64-bit id, value in the parameter's own range, optional ramp length), each with a sample offset inside the block. Lists are sorted by offset; the client clamps offsets to the block. Sysex is not carried.
* **Transport** per block: flags (playing, recording, looping, validity bits), tempo, time signature, sample position, musical position, bar start, loop range.
* **Notifications** (plug-in to host, independent of blocks): parameter gesture begin/end and value changes made in the plug-in's editor, latency change, tail change, "rescan parameters", "state dirty", editor closed/resized, restart request. Multi-producer in the container (lock-free, real-time safe), drained by the host's housekeeping thread every 10 ms (configurable) and turned into `HostEvent`s.

The host never trusts the region for memory safety (a crashing plug-in can scribble over it): it keeps its own copy of the layout and clamps every count it reads.

### The block handshake: synchronous, no added latency

`Instance::process` on the host's audio thread:

1. copies the inputs, events and transport into the region and stores the next block number into `req.seq`;
2. wakes the instance's real-time thread in the container;
3. sleeps until `rsp.seq` equals that number, or the deadline passes;
4. copies the outputs and out events back.

The container's real-time thread calls `Processor::process` with buffers that point straight into the region (no copy on that side). The block is processed **within** the host's call, so **the transport adds no latency**: the latency an instance reports is exactly the plug-in's own (verified: a 64-frame delay plug-in comes back delayed by exactly 64 frames across blocks of 1 to 4096 frames).

### Wake-up primitive

One abstraction (`stpc_transport::wake`), two interchangeable mechanisms; the handshake state is always the sequence word in the region, the primitive only sleeps and wakes.

| Mode | Linux | macOS |
|---|---|---|
| `Address` (default) | `futex(2)`, shared variant, on the sequence word | `os_sync_wait_on_address` with the SHARED flag (public API, macOS 14.4+), else `__ulock_wait2` / `__ulock_wake` (macOS 11+), resolved with `dlsym`. **Type-checked only.** |
| `Pipe` (fallback, or forced with `WakePreference::Pipe`) | one pipe per direction, `write` / `ppoll` | same code with `poll` (millisecond timeouts). Plain POSIX; verified on Linux, **type-checked only** on macOS. |

`WakePreference::Auto` uses address waits when both processes announce them, else pipes. In pipe mode the read end also reports a dead peer at once (hang-up).

## Control channel and protocol

Length-prefixed frames over the Unix stream socket: `[u32 length][u8 kind][u64 id][payload]`, little-endian, hand-written encoding with bounds checks (`wire.rs`; a corrupt frame is an error, never a panic or a huge allocation). Kinds: handshake, request, response, notification. Every request gets exactly one response with the same id. A Windows port would implement `channel.rs` over a named pipe (the endpoint string becomes the pipe name) and `shm.rs` over a named file mapping; nothing else is platform specific.

`PROTOCOL_VERSION` (currently 1) covers the messages **and** the region layout. It is checked in the handshake (host and container each refuse the other) and again in the region header when a container maps a region.

Requests (`messages.rs`): `Ping` (answered by the I/O thread), `PingMain` (answered by the main thread), `Shutdown`, `CreateInstance` (with the region), `DestroyInstance`, `Activate`, `Deactivate`, `Reset`, `SetOffline`, `SetBypass`, `Reconfigure` (new sample rate / block size, new region), `GetInfo`, `GetParameters`, `GetParameterValues`, `GetParameter`, `SetParameter`, `FormatParameter`, `ParseParameter`, `GetState`, `SetState` (opaque bytes, megabytes are fine), `GetPresets`, `LoadPreset`, `OpenEditor` (with a title), `CloseEditor`, `FocusEditor`, `SetEditorTitle`.

## Host side: `stpc-client`

```rust
let supervisor = Supervisor::new(SupervisorConfig { policy: GroupingPolicy::PerPlugin, ..Default::default() })?;
let instance = supervisor.create_instance(InstanceConfig {
    manufacturer: "Vendor".into(),
    options: "/path/to/Plugin.clap".into(),      // backend-specific
    max_dry_delay_frames: 8192,                  // latency-matched dry fallback
    ..InstanceConfig::effect("clap", "com.vendor.plugin")
})?;
instance.activate()?;

// audio thread
let outcome = instance.process(&mut ProcessBuffers { frames, inputs: &ins, outputs: &mut outs, events: &events,
                                                     out_events: &mut midi_out, transport: Some(&transport), deadline: None });
// control threads
while let Some(event) = supervisor.poll_event() { /* HostEvent::InstanceLost { .. } => ..., */ }
```

### `Instance::process` (and `process_raw` for raw channel pointers)

Real-time safe: no allocation (verified with a counting allocator in every path), no lock, one wake-up system call and one bounded sleep. It always fills the outputs and says what they are:

| Flag | Meaning |
|---|---|
| `PROCESS_OK` | the plug-in's audio is in the outputs |
| `PROCESS_FALLBACK` | the outputs hold the fallback, for the reason given by one of the next flags |
| `PROCESS_LATE` | the container missed the deadline, in this block or an earlier one it has not finished |
| `PROCESS_LOST` | the container is gone |
| `PROCESS_INACTIVE` / `PROCESS_BUSY` / `PROCESS_BYPASSED` | not activated / a control operation holds the instance / client-side bypass |
| `PROCESS_RESYNCED` | first processed block after dropped ones (the plug-in sees `discontinuity`) |
| `PROCESS_EVENTS_DROPPED`, `PROCESS_OUT_EVENTS_DROPPED`, `PROCESS_PLUGIN_ERROR`, `PROCESS_INVALID` | capacity exceeded, plug-in error, bad arguments |

* **Fallback**: silence for instruments, the dry main input on the main output for effects (`Fallback` can override). With `max_dry_delay_frames > 0` the dry signal is delayed by the plug-in's reported latency, so the host's delay compensation stays valid while a plug-in is bypassed, late or lost.
* **Deadline**: `InstanceConfig::deadline`, or per block `ProcessBuffers::deadline` (e.g. the time left in the audio cycle). Default: the block's duration, at least 2 ms.
* **After a miss** the call does not wait again: every following block returns the fallback immediately (the region still belongs to the container) until the container has finished the late block; then processing resumes by itself (`PROCESS_RESYNCED`). If the container does not finish within `hang_timeout` it is reported as hung and, with `kill_hung` (default), killed.
* **After a loss** the call returns the fallback immediately until `restore`.
* **Offline rendering** (`set_offline(true)`): no deadline, so an export never drops a plug-in; the wait still ends at once if the container dies. `offline_hang_timeout` can bound it (default: unbounded).
* One audio thread per instance at a time. `Instance` is `Send + Sync`; control calls may run concurrently with `process`. Do not drop an `Instance` on the audio thread (dropping destroys the plug-in through a blocking request).

`status()`, `latency()`, `tail()`, `stats()` and `set_deadline()` are real-time safe too.

### Control calls

Blocking requests with timeouts (`request_timeout`, `create_timeout`, `state_timeout`): `info`, `refresh_info`, `activate`, `deactivate`, `reset`, `set_offline`, `set_bypass` (plug-in bypass if it has one, else client-side bypass by the fallback path), `reconfigure(sample_rate, max_frames)`, `parameters` (id, names, group, unit label, flags, min/max/default, step count, value strings), `parameter_values`, `parameter`, `set_parameter`, `format_parameter`, `parse_parameter`, `state`, `set_state`, `last_known_state`, `presets`, `load_preset`, `open_editor(title)`, `close_editor`, `focus_editor`, `set_editor_title`, `ping`, `restore`.

Errors distinguish `Timeout` (the plug-in's main thread is busy or hung; the container is alive), `ContainerDied`, `InstanceLost`, `Unsupported`, `Plugin(message)`, `UnknownFormat`, `Rejected`.

### Events

`Supervisor::poll_event()` and/or `set_event_callback` (called on a supervisor thread, never the audio thread): `ContainerStarted`, `ContainerExited { reason, lost }`, `InstanceLost`, `DeadlineMissed`, `InstanceHung`, `InstanceRestored`, `InstanceRestoreFailed`, `ParameterGesture`, `ParameterChanged`, `LatencyChanged`, `TailChanged`, `ParametersRescan`, `StateDirty`, `EditorClosed`, `EditorResized`, `RestartRequested`, `Log`.

## Container side: `stpc-runtime`

```rust
fn main() {
    stpc_runtime::abort_on_panic();
    let mut runtime = Runtime::new(RuntimeConfig::from_args(std::env::args().skip(1))?);
    runtime.register_backend(Box::new(MyBackend));
    runtime.connect()?;
    runtime.run();
}
```

### Threads and the contract backends rely on

| Thread | Runs |
|---|---|
| **Main thread** (owns the `Runtime`) | every control request; every `PluginBackend` and `Plugin` method; `Processor::reset`; editor windows. `Plugin` objects never leave it and need not be `Send`. |
| **I/O thread** | reads the control socket, answers `Ping` itself, queues everything else for the main thread. The main thread is never blocked on I/O. If the host disappears, this thread ends the process at once. |
| **Real-time thread**, one per instance | `Processor::process` only. Real-time scheduling class where permitted; denormals flushed to zero (the host's setting does not cross the process boundary). |

`process` never overlaps `Plugin::activate`, `deactivate`, `set_offline` or `Processor::reset`: while one of those runs, requested blocks are answered "busy" and the host plays its fallback, so the host's audio thread never waits for the container's main thread. All other `Plugin` methods can run while `process` runs, as in an in-process host.

`Plugin::activate` hands out a `Box<dyn Processor>` (the audio half, `Send`), `deactivate` takes it back. A plug-in talks to the host through its `HostHandle`: `notify(..)` (lock-free, any thread) and `request_main_callback()` (schedules `Plugin::idle` on the main thread).

### Main loop and editor windows

Editors are created by the container process itself, on its main thread (`Plugin::open_editor(title)`), which is why the control I/O lives on its own thread. `Runtime::run()` is a plain blocking loop, enough for backends without windows. A container with real editor windows must run the platform's event loop on the main thread instead (AppKit requires it): install `Runtime::set_main_waker(..)` (called from other threads when work is pending; it must make the event loop call `Runtime::run_pending()` on the main thread, e.g. by signalling a run-loop source) and leave the loop when `run_pending()` returns `LoopState::Exit`.

### Backends

* **`test`** (`test_backend.rs`): `copy`, `gain` (sample-accurate parameter, bypass, presets, simulated editor), `delay` (reports latency), `ducker` (sidechain), `sine` (MIDI instrument, MIDI thru), `probe` (reports transport and block flags), `bigstate` (megabyte state, 64 parameters), `crash` (abort, segfault or exit, on the main or real-time thread, in create/activate/state), `hang` (stall or hang in process or on the main thread), `slow` (slow instantiation).
* **`clap`** (`clap_backend.rs`): `plugin_id` is the CLAP plug-in id, `options` the path of the `.clap` file. Implements the CLAP life cycle and threading rules, 32-bit audio on all ports (ports the host did not allot get scratch buffers), notes and MIDI in, sample-accurate parameter events, transport, output events (MIDI; parameter changes and gestures become notifications), `clap.params`, `clap.state`, `clap.latency` (including the restart protocol: `request_restart` becomes `HostEvent::RestartRequested`, and the host answers with `deactivate()` + `activate()`), `clap.tail`, `clap.audio-ports`, `clap.note-ports`, bypass through the parameter flagged as bypass, `request_callback`, `params.request_flush`, `clap.thread-check`, `clap.log`, and **floating** editor windows via `clap.gui`. Untimed `set_parameter` on an active plug-in is queued and delivered with the next block (CLAP's rule), so a value read back before that block still shows the old value. Not implemented: embedded editors, 64-bit audio, sysex, MIDI 2, note expressions, polyphonic modulation, timer and fd extensions, plug-in scanning. Exercised by the test plug-in (`tests/clap.rs`): everything listed except `clap.tail`, `params.request_flush`, `clap.thread-check`, `clap.log`, gesture events and MIDI / note *output* events, which are written from the CLAP headers and have not run.

Adding a format (VST3, Audio Units through AudioToolbox) means one more `PluginBackend` in the container crate and one `register_backend` line in `main.rs`; nothing in the transport or the client changes.

## Failure handling

| Failure | Detection | What the host sees |
|---|---|---|
| Container crashes (signal) or exits | the control socket closes; the reader thread kills and reaps the process | requests in flight fail with `ContainerDied`; a waiting `process` call is released and returns the fallback; every later block returns `PROCESS_LOST` at once; `InstanceLost` per instance and `ContainerExited { reason: Crashed { signal } \| Exited { code }, lost }` |
| Plug-in stalls in `process` | deadline in `process` | `PROCESS_LATE` blocks without waiting; `DeadlineMissed`; automatic resume when the container catches up |
| Plug-in hangs in `process` | late for longer than `hang_timeout` (housekeeping thread) | `InstanceHung`; with `kill_hung` the container is killed: `InstanceLost { reason: Hung }` |
| Plug-in hangs on its main thread | requests time out (`Error::Timeout`); `ping(true, ..)` fails while `ping(false, ..)` succeeds | audio of all instances in that container **continues**; the host decides (`kill_container`) |
| Plug-in slow to load | nothing fails: `create_timeout` is generous (120 s) | other instances of the container keep processing meanwhile; on timeout the half-made instance is destroyed behind the caller |
| Host dies | the container's I/O thread sees the socket close | the container exits immediately; nothing is left (no names in the file system or shm namespace) |

Other containers are never affected by any of this.

**Restart with state.** The client keeps, per instance, its configuration, the *last known state* (the bytes last fetched with `state()`, given with `set_state()` or `initial_state`, or snapshotted after `load_preset`; optionally refreshed every `auto_state_interval`), the parameter values set or reported since that state, and the activation, bypass and offline flags. `Supervisor::restart_container(id)` restarts the process and rebuilds every lost instance; `Instance::restore()` does it for one instance (starting the process if needed): create, load the state, replay the newer parameter values, restore flags, activate if it was active. Restarting is always on request: a plug-in that crashes on load cannot cause a restart loop (`InstanceRestoreFailed`). An open editor is not reopened automatically; `Instance::editor_title()` tells the host what was open.

After a request has failed with `ContainerDied`, and after `kill_container`, the instance statuses are final (`Lost`), so the caller can restore immediately. `restore` also copes with being called between the audio thread noticing a crash and the supervisor having processed it.

**Shutdown.** Dropping the `Supervisor` (or `shutdown()`) asks every container to exit, kills those that do not within `shutdown_timeout`, reaps them and removes the socket directory. Remaining `Instance`s become `Closed`.

## Verification on Linux

All of this ran on Linux x86-64 (4 cores, shared virtual machine), in debug and `--release`, each with address waits and with `STPC_TEST_WAKE=pipe`, and with `STPC_SHM=posix`. Every integration test spawns the real container binary through a real `Supervisor` and ends by asserting that no container process or zombie, no socket, no socket directory and no named shared-memory object is left.

```sh
cd rust/plugin-host
cargo test                      # 57 tests
cargo test --release
STPC_TEST_WAKE=pipe cargo test  # the portable wake-up path
STPC_SHM=posix cargo test       # the shared-memory path macOS uses
cargo test --release -p stpc-container --test realtime -- --ignored --nocapture --test-threads=1   # measurement
cargo check --target aarch64-apple-darwin --all-targets
```

| Suite | Tests | Covers |
|---|---|---|
| `transport` unit tests | 13 | encoding round trips and hostile input, region layout and attach checks (including a version mismatch), notification queue (overflow, wrap, 4 producers), futex and pipe waits, descriptor passing, 5 MB frames, listener cleanup, shm without leftover names |
| `tests/integration.rs` | 33 | bit-exact audio for 7 bus layouts (1 to 64 channels) and block sizes 1 to 4096 changing per call, arbitrary bit patterns; sample-accurate MIDI and parameter events; MIDI out; event overflow; sidechain; transport and flags; parameter metadata, text, set/get; presets; editor open/title/focus/close and editor-originated edits; 12 MiB state round trip; latency reporting and change; bypass (plug-in and latency-matched client bypass); reconfigure; process count under each policy (1, 2, 4, 6 for six instances); crash on the real-time thread and on the main thread (abort, segfault, exit code), in create, activate and state load; crash isolation and flagging within the block; late block and automatic recovery, with the discontinuity flag reaching the plug-in; hang detection, kill and restart; hung main thread with audio continuing; offline mode; restart with state, replayed parameters and activation; periodic state snapshots; failed restores; slow instantiation and creation timeout; version mismatch (both directions), bad invocations, host death; clean shutdown; stale directory sweep |
| `tests/realtime.rs` | 3 (+1 measurement) | zero allocator calls on the audio thread in the normal path (4500 blocks with events, MIDI out, transport), the inactive and bypass paths, deadline miss / late / resync, and while and after a container crash |
| `tests/stress.rs` | 1 | 3 seeds x 400 random operations (create, destroy, parameters, state, kill, internal crash, restore, restart, policy change, activate/deactivate, reconfigure) against a concurrently processing audio thread; about 60 to 90 container processes per seed |
| `tests/clap.rs` | 7 | the CLAP backend against a real CLAP library: metadata, parameters, text, flush and queued changes, automation, state, main-thread callback, bypass parameter, port layout mismatch, note and MIDI input, parameter output events, latency restart protocol, floating editor, crash (segfault and abort) inside the plug-in with restart and state restore, loading errors |

### Measured round trip

`measure_round_trip_overhead` (release build): wall-clock time of one `Instance::process` call for a stereo block through the `copy` plug-in, whose own work is about 0.1 µs, so the figures are the cost of the process boundary. "Back to back": 20 000 calls in a loop. "Paced": 2 000 calls, one per block period at 48 kHz, which is what an audio callback does and includes waking a sleeping thread. The machine was a 4-core VM shared with other workloads (load average 4 to 7 during the runs).

| Configuration | Frames | Median | p99 | p99.9 | Worst |
|---|---|---|---|---|---|
| futex, back to back | 64 | 2.9 µs | 7.9 µs | 36 µs | 7.2 ms |
| | 128 | 3.0 µs | 6.6 µs | 50 µs | 4.4 ms |
| | 512 | 3.0 µs | 4.0 µs | 22 µs | 4.2 ms |
| futex, paced | 64 | 14.0 µs | 23 µs | 458 µs | 0.62 ms |
| | 128 | 14.4 µs | 29 µs | 3.8 ms | 4.7 ms |
| | 512 | 15.6 µs | 29 µs | 373 µs | 4.0 ms |
| pipe, back to back | 64 | 3.3 µs | 71 µs | 1.4 ms | 8.5 ms |
| | 128 | 3.3 µs | 5.4 µs | 21 µs | 5.8 ms |
| | 512 | 5.0 µs | 52 µs | 132 µs | 3.2 ms |
| pipe, paced | 64 | 6.0 µs | 21 µs | 6.1 ms | 6.1 ms |
| | 128 | 8.6 µs | 48 µs | 4.9 ms | 6.2 ms |
| | 512 | 17.1 µs | 29 µs | 5.6 ms | 5.6 ms |
| in-process copy, for scale | 64 / 128 / 512 | 0.04 / 0.04 / 0.07 µs | | | |

Reading these numbers:

* The cost of the mechanism itself is about **3 µs per block** when both threads are warm, independent of the block size (the copies are negligible), and about **15 µs** when the container thread has to be woken from sleep, which is the realistic case.
* The worst cases (milliseconds) are not the transport: they are this machine's scheduler preempting one of the two threads, on a virtual machine with more runnable threads than cores. Two other runs of the same measurement gave paced medians of 12 to 15 µs with worst cases between 0.6 and 6 ms, and one run under heavier load a paced median of 91 µs at 512 frames. With the default deadline such blocks would have been dropped for that instance (the measurement used a 10 s deadline, so every block was waited for; no block fell back).
* The container's real-time threads did run under `SCHED_FIFO` here (the tests ran as root). Putting the measuring host thread under `SCHED_FIFO` as well, as a real audio thread would be, made things *worse* on this machine (paced median 55 to 63 µs, worst 2 to 4 ms): two real-time threads of equal priority handing over to each other on a loaded VM. Spinning for up to 100 µs before sleeping (`SupervisorConfig::spin`) brought that back to 27 to 44 µs. None of this predicts a dedicated audio machine; it shows that thread scheduling, not the transport, decides the worst case, and that it has to be measured on the target (macOS, real audio device) before the defaults are final.
* State transfer for scale: a 12 MiB state takes about 40 ms to fetch and about 40 ms to load over the control socket (`state_of_several_megabytes_round_trips`, debug and release alike; most of it is copying).

## macOS: what a Rust host needs to know

Nothing below has run on macOS. `cargo check --target aarch64-apple-darwin --all-targets` passes for the whole workspace (and `cargo clippy` for that target is clean); that is the extent of the verification.

### Code that is type-checked only

| Where | What |
|---|---|
| `transport/src/wake.rs`, `address::imp` for macOS | `os_sync_wait_on_address*` / `os_sync_wake_by_address_all` with the SHARED flag, and the `__ulock_wait2` / `__ulock_wake` fallback, looked up with `dlsym`. Same semantics as the futex path, so the handshake logic itself is the code verified on Linux. |
| `transport/src/wake.rs`, `poll_one` (non-Linux) | `poll` instead of `ppoll`: millisecond timeouts, rounded up. |
| `transport/src/shm.rs`, `create_posix_shm` | Runs on Linux only under `STPC_SHM=posix`. On macOS it is the only path. Names are 23 bytes (limit 31). |
| `transport/src/channel.rs` | `SO_NOSIGPIPE` instead of `MSG_NOSIGNAL`; no `MSG_CMSG_CLOEXEC` (close-on-exec is set after receipt). |
| `runtime/src/rt.rs` | Time-constraint thread policy for real-time threads; FZ bit in FPCR on AArch64. |
| `container/src/clap_backend.rs` | Bundle layout (`X.clap/Contents/MacOS/X`), window API name `cocoa`. |

### Integration notes

* **Locating the container.** Put `supertracker-plugin-container` next to the application's main executable (`Contents/MacOS/`), which is where `Supervisor` looks by default. It must be code-signed with the application; under the hardened runtime it needs the same entitlements a plug-in host needs (`com.apple.security.cs.disable-library-validation` to load third-party plug-ins, audio input if plug-ins record).
* **Socket path length.** `sockaddr_un` holds 104 bytes on macOS and `$TMPDIR` is long there (`/var/folders/…/T/`). `Supervisor::new` fails early with a clear error if the path would not fit; set `SupervisorConfig::socket_dir` to something short (e.g. a directory under `~/Library/Caches/<app>`).
* **Wake-up.** `os_sync_wait_on_address` exists from macOS 14.4; on 14.0 to 14.3 the code falls back to `__ulock_*`, which is private API (the Rust standard library uses the same fallback). If that is not acceptable, or if the first runs show a problem with shared address waits, set `SupervisorConfig::wake = WakePreference::Pipe`: plain POSIX, verified on Linux, with millisecond deadline granularity on macOS.
* **Real-time priority.** The host's audio thread now sleeps while a container thread does the work, so that thread must be scheduled like an audio thread. The runtime requests a time-constraint policy per real-time thread. Joining the device's audio workgroup (`os_workgroup_join`) is what Apple recommends and is not done: the workgroup would have to be sent to the container as a Mach port. Measure dropouts on Apple silicon (efficiency cores) before deciding whether that is needed.
* **Editor windows.** They belong to the container process. A backend that opens real windows must (1) run `NSApplication` on the container's main thread and drive the runtime from it (`set_main_waker` + `run_pending`, see above; the built-in `Runtime::run()` loop does not pump AppKit), (2) choose an activation policy (accessory: no Dock icon), (3) implement `focus_editor` by activating the container application and ordering the window front, and (4) return the window number in `EditorInfo::window_id` if the host wants to order its own windows relative to it. Keyboard focus and window ordering across two processes (plug-in window above the main window, hiding with the application, Spaces, full screen) need to be designed on a Mac; Bitwig's behaviour is a reasonable reference. Reported through `HostEvent::EditorClosed` when the user closes a window.
* **Audio Units and VST3.** The `Plugin` / `Processor` traits were shaped for them: main-thread objects need not be `Send`; `activate` returns the audio half; parameter ids are 64-bit (AU addresses); parameter values travel in the parameter's own range (AU: min..max, VST3: 0..1); `identifier`, `group`, `unit_label`, `value_strings` and `step_count` carry the metadata the current rack shows; state is opaque bytes (wrap a property list or component + controller state as the backend sees fit). AUv2/AUv3 view controllers require the AppKit loop mentioned above.
* **Architectures.** A container is a separate executable, so an x86-64 build of it could host Intel-only plug-ins under Rosetta next to an arm64 host. Whether shared address waits work between a translated and a native process is unknown; pipe mode avoids the question.
* **Sandbox.** An App Sandbox host cannot spawn an arbitrary helper with inherited rights in the way assumed here; this design is for a non-sandboxed (Developer ID) application.

## Open points and risks

* Nothing has run on macOS (see above). The first thing to do there is to run the test suite itself: it has no Linux-only logic except the `/proc` checks in `tests/common/mod.rs` (process existence, children), which need a macOS equivalent.
* Real-time behaviour under load is only as good as the scheduling of the container threads. Without real-time priority the worst case is a scheduler quantum (see the measurements); the deadline plus fallback keeps the host's audio thread from stalling, at the price of a dropout on that instance.
* A deadline miss drops whole blocks for that instance and tells the plug-in (`discontinuity`); events of dropped blocks are lost, so a note-off can go missing. Hosts should send all-notes-off after `PROCESS_RESYNCED` for instruments.
* The default deadline (one block period, at least 2 ms) is a guess. The right value is the time left in the audio cycle, which only the host knows: pass it per block.
* Embedded editors (most CLAP, all VST3 and AU plug-ins) need window code in the container; only CLAP floating windows work today.
* No plug-in scanning. Scanning inside a container (so that a crashing plug-in cannot kill the scan) would be a natural next request type.
* Sysex, MIDI 2 and parameter ramps are not carried end to end (the event has a ramp field; no backend uses it yet).
* Windows: `channel.rs` and `shm.rs` are Unix only; the wake-up abstraction would get a third implementation (`WaitOnAddress` does not work across processes; an event object or a named pipe would).
* The container binary honours `STPC_TEST_PROTOCOL_VERSION` (announces another protocol version); it exists for the version-mismatch test.
