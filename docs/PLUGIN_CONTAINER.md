# Plug-in container: out-of-process plug-in hosting

Plug-ins run in separate processes ("containers"), as in Bitwig Studio, so that a crashing or hanging plug-in cannot take the application down. The code is the cargo workspace `crates/plugin-host/` (excluded from `Cargo.toml`). Everything is Rust; there is no C API.

Status in one paragraph: the cross-platform core (transport, host-side client, container runtime, container executable with test plug-ins and a CLAP backend) is built and **verified by running on Linux x86-64** (see [Verification](#verification-on-linux)). For macOS the whole workspace **type-checks** (`cargo check --target aarch64-apple-darwin --all-targets`), and nothing has run there; the macOS-only code paths are listed under [macOS](#macos-what-a-rust-host-needs-to-know). On top of that core: a [VST3 backend](#vst3-backend) (verified on Linux against a VST3 test plug-in; its macOS module loading and editor are type-checked only) and [plug-in scanning](#plug-in-scanning) for all formats (verified on Linux).

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
└───────────────────────────────┘               │  backends: "test", "clap", "vst3", "au"      │
                                                └──────────────────────────────────────────────┘
```

| Crate (`crates/plugin-host/crates/`) | Kind | Contents |
|---|---|---|
| `transport` (`stpc-transport`) | lib | Shared region layout, notification queue, wake-up primitive, control channel, protocol messages and encoding, anonymous shared memory. |
| `client` (`stpc-client`) | lib | What the host application links: `Supervisor`, `Instance`, events; `Scanner` (installed plug-ins). |
| `runtime` (`stpc-runtime`) | lib | What a container program links: `Runtime`, the `PluginBackend` / `Plugin` / `Processor` traits, `HostHandle`. |
| `container` (`stpc-container`) | bin + lib | The `supertracker-plugin-container` executable; backends `test` (`test_backend.rs`) and `clap` (`clap_backend.rs`); all integration tests. |
| `clap-test-plugin` | cdylib | A CLAP plug-in library (gain effect, sine instrument) used to test the CLAP backend. |
| `vst3-test-plugin` | cdylib | A VST3 module (gain effect with a separate controller, single-component sine instrument) used to test the VST3 backend and the scanner. |

Dependencies: `libc` and `clap-sys` (both MIT OR Apache-2.0, no transitive dependencies), and `vst3` 0.3 with its one dependency `com-scrape-types` (both MIT OR Apache-2.0; the VST3 bindings are generated from the MIT-licensed VST SDK 3.8.0 headers and ship with the crate). The workspace is GPL-3.0-or-later like the application.

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

`PROTOCOL_VERSION` (currently 2; version 2 added `Scan`) covers the messages **and** the region layout. It is checked in the handshake (host and container each refuse the other) and again in the region header when a container maps a region.

Requests (`messages.rs`): `Ping` (answered by the I/O thread), `PingMain` (answered by the main thread), `Shutdown`, `CreateInstance` (with the region), `DestroyInstance`, `Activate`, `Deactivate`, `Reset`, `SetOffline`, `SetBypass`, `Reconfigure` (new sample rate / block size, new region), `GetInfo`, `GetParameters`, `GetParameterValues`, `GetParameter`, `SetParameter`, `FormatParameter`, `ParseParameter`, `GetState`, `SetState` (opaque bytes, megabytes are fine), `GetPresets`, `LoadPreset`, `OpenEditor` (with a title), `CloseEditor`, `FocusEditor`, `SetEditorTitle`, and `Scan` (format and plug-in file: lists the plug-ins in it without instantiating them; sent to containers started for [scanning](#plug-in-scanning) only).

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

`Plugin::activate` hands out a `Box<dyn Processor>` (the audio half, `Send`), `deactivate` takes it back. A plug-in talks to the host through its `HostHandle`: `notify(..)` (lock-free, any thread), `request_main_callback()` (schedules `Plugin::idle` on the main thread) and `with_processing_excluded(..)` (main thread: runs a closure while the instance's real-time thread is kept out of `process`, for backends that must hand the plug-in something themselves, like the VST3 parameter flush). For tests of a backend inside one process there are `HostHandle::detached()` and `TestBlock` (the buffers of a block without a container).

### Main loop and editor windows

Editors are created by the container process itself, on its main thread (`Plugin::open_editor(title)`), which is why the control I/O lives on its own thread. `Runtime::run()` is a plain blocking loop, enough for backends without windows. A container with real editor windows must run the platform's event loop on the main thread instead (AppKit requires it): install `Runtime::set_main_waker(..)` (called from other threads when work is pending; it must make the event loop call `Runtime::run_pending()` on the main thread, e.g. by signalling a run-loop source) and leave the loop when `run_pending()` returns `LoopState::Exit`.

### Backends

* **`test`** (`test_backend.rs`): `copy`, `gain` (sample-accurate parameter, bypass, presets, simulated editor), `delay` (reports latency), `ducker` (sidechain), `sine` (MIDI instrument, MIDI thru), `probe` (reports transport and block flags), `bigstate` (megabyte state, 64 parameters), `crash` (abort, segfault or exit, on the main or real-time thread, in create/activate/state), `hang` (stall or hang in process or on the main thread), `slow` (slow instantiation).
* **`clap`** (`clap_backend.rs`): `plugin_id` is the CLAP plug-in id, `options` the path of the `.clap` file. Implements the CLAP life cycle and threading rules, 32-bit audio on all ports (ports the host did not allot get scratch buffers), notes and MIDI in (controllers, channel and poly pressure and pitch bend as MIDI for a note port that takes MIDI; for one that takes CLAP note events only, pressure and pitch bend as the note expressions *pressure* and *tuning*, two semitones either way, and control changes dropped), sample-accurate parameter events, transport, output events (MIDI; parameter changes and gestures become notifications), `clap.params`, `clap.state`, `clap.latency` (including the restart protocol: `request_restart` becomes `HostEvent::RestartRequested`, and the host answers with `deactivate()` + `activate()`), `clap.tail`, `clap.audio-ports`, `clap.note-ports`, bypass through the parameter flagged as bypass, `request_callback`, `params.request_flush`, `clap.thread-check`, `clap.log`, and **floating** editor windows via `clap.gui`. Untimed `set_parameter` on an active plug-in is queued and delivered with the next block (CLAP's rule), so a value read back before that block still shows the old value. Not implemented: embedded editors, 64-bit audio, sysex, MIDI 2, other note expressions, polyphonic modulation, timer and fd extensions, plug-in scanning. Exercised by the test plug-in (`tests/clap.rs`): everything listed except `clap.tail`, `params.request_flush`, `clap.thread-check`, `clap.log`, gesture events and MIDI / note *output* events, which are written from the CLAP headers and have not run.
* **`vst3`** (`vst3/`): see [VST3 backend](#vst3-backend).

Adding a format means one more `PluginBackend` in the container crate and one `register_backend` line in `main.rs` (plus a scanner function registered with `Runtime::register_scanner`, see [Plug-in scanning](#plug-in-scanning)); nothing in the transport or the client changes.

## VST3 backend

Format `"vst3"` (`crates/container/src/vst3/`). `plugin_id` is the class id of the plug-in's component as 32 hex digits (as `FUID::toString` prints it and as the scanner reports it), `options` the path of the `.vst3` bundle. The COM interfaces come from the `vst3` crate (0.3.0, MIT OR Apache-2.0): bindings generated from the headers of VST SDK 3.8.0, which Steinberg publishes under the MIT licence, and shipped with the crate, so no SDK and no C++ are involved. Its one dependency is `com-scrape-types` (MIT OR Apache-2.0).

**Verified by running on Linux** against `stpc-vst3-test-plugin` (`tests/vst3.rs`, `tests/vst3_realtime.rs`). Module loading on macOS and the editor are **type-checked only**; module loading on Windows is written from the SDK documentation and type-checked on its own (`vst3/module.rs` alone for `x86_64-pc-windows-gnu`; the workspace is Unix only). No third-party VST3 plug-in has been loaded yet.

| File | Contents |
|---|---|
| `module.rs` | Loading a module (Linux: `Contents/<arch>-linux/Name.so`, `ModuleEntry` / `ModuleExit`; macOS: `CFBundle`, `bundleEntry` / `bundleExit`; Windows: `Contents/<arch>-win/Name.vst3`, `InitDll` / `ExitDll`), `GetPluginFactory`, class enumeration (`PClassInfoW`, else `PClassInfo2`, else `PClassInfo`), class ids |
| `host.rs` | The host's COM objects: `IHostApplication` (with `IMessage` / `IAttributeList` for plug-ins that talk between their two halves), `IBStream` over memory, `IComponentHandler` + `IComponentHandler2` |
| `process.rs` | `IParameterChanges` / `IParamValueQueue` / `IEventList` with storage allocated at activation, the process context, the real-time `Processor`, the zero-sample flush |
| `plugin.rs` | The instance on the main thread: creation, buses, life cycle, parameters, state, presets, restart requests |
| `state.rs` | The state container |
| `editor.rs` | `IPlugView` in a container window, `IPlugFrame` |

### Life cycle

Creation: load the module (once per bundle path; `IPluginFactory3::setHostContext`), create the component, `initialize`, query `IAudioProcessor` (32-bit float is required). The edit controller is the component itself when it implements `IEditController` (single-component plug-ins; initialized once), otherwise the class named by `getControllerClassId`, initialized separately and connected to the component through `IConnectionPoint` in both directions. The controller gets the component handler and the component's state (`setComponentState`). Then bus arrangements are negotiated (below), the parameter list is read (bypass and program change parameters, MIDI controller mapping) and, on macOS, the editor is probed.

`Plugin::activate` is `setupProcessing` (process mode, 32-bit, block size, sample rate), `activateBus` for every audio and event bus, `setActive(true)`, `setProcessing(true)` (optional in VST3: "not implemented" is accepted), in that order; `deactivate` is `setProcessing(false)`, `setActive(false)`. A new sample rate or block size arrives as deactivate + activate. Teardown follows the SDK's own host: close the editor, stop, disconnect the connection points, clear the component handler, `terminate` the component and then a separate controller, release every interface, and only then release the factory, call the module's exit function and unload it. A module is shared by its instances in a container and unloaded with the last of them (the test plug-in counts its live objects at module exit: none).

### How the capabilities map to the traits

| Trait / client call | VST3 |
|---|---|
| `info` | Class name, vendor (the factory's when the class states none), version; instrument from the sub-categories; MIDI in / out from the event buses; latency (`getLatencySamples`), tail (`getTailSamples`, "infinite" becomes `u64::MAX`); per host bus the plug-in bus connected to it and the channels in use |
| `process` | One `IAudioProcessor::process` per block. Channel buffers point into the shared region; channels the host did not allot (and whole buses without a host bus) get scratch memory. Silence flags are passed in and out (a channel the plug-in flags silent is cleared) |
| events in | Notes, note-offs and poly pressure become events at their sample offset (`kIsLive` unless the event is flagged sequenced; note ids are passed). Control changes, channel pressure and pitch bend become points of the parameter the plug-in maps them to (`IMidiMapping`); unmapped ones are dropped. A program change becomes the program change parameter. Parameter events become queue points at their offset |
| events out | Note on / off, poly pressure and legacy MIDI events (controllers, pitch bend, pressure, program change) become MIDI out events |
| transport | `ProcessContext`: sample position, tempo, time signature, musical position, bar start, cycle range and the playing / recording / cycle flags, each flagged valid only when the host said so; a continuous sample counter and the system time are always valid |
| `parameters` | `ParamID` as id; title, short title, units, unit path as group ("Parent/Child" from `IUnitInfo`), step count, default; flags for automatable, read-only, hidden, bypass, stepped, boolean; list parameters carry their value strings. Values are **normalized** (0..1) everywhere |
| `parameter_value`, `format_parameter`, `parse_parameter` | The edit controller: `getParamNormalized`, `getParamStringByValue`, `getParamValueByString` |
| `set_parameter` (untimed) | `setParamNormalized` on the controller at once (so a read-back is current), and a point at offset 0 of the processor's next block |
| edits in the editor | `beginEdit` / `performEdit` / `endEdit` become `ParameterGesture` and `ParameterChanged` events, and `performEdit` is carried to the processor like `set_parameter`; `setDirty` becomes `StateDirty` |
| output parameters, automation | After each block the last value of every output queue (meters) is given to the controller and reported as `ParameterChanged`; values the host automated or MIDI mapped are given to the controller too (MIDI-mapped ones are also reported). This happens in `Plugin::idle`, once per value and block at most |
| `set_bypass` | The parameter flagged `kIsBypass`; without one the client bypasses by its dry path |
| `set_offline` | `kOffline` in the setup (redone, the component being restarted for it) and in every block |
| `reset` | Note-offs for held notes in a one-sample block nobody hears, then `setProcessing(false)`, `setActive(false)`, `setActive(true)`, `setProcessing(true)` |
| `state`, `set_state` | Component and controller state in one container (below) |
| `presets`, `load_preset` | The program list of the program change parameter's unit (`IUnitInfo`); loading sets that parameter |
| `open_editor` etc. | `IPlugView` (below) |

**Buses.** The host fixes the layout at creation. Host bus 0 is connected to the plug-in's first *main* bus (also when an auxiliary bus precedes it), the further host buses to the remaining plug-in buses in order: a second host input bus is the sidechain. The backend asks for mono, stereo or the plug-in's own arrangement of that width with `setBusArrangements`, then takes whatever the plug-in settles on: where it differs, the common channels are used, the rest is bridged with silent or discarded scratch channels, and `InstanceInfo` reports the channels really in use. Plug-in buses without a host bus are switched off. A mono plug-in output on a stereo host bus is not duplicated to the second channel.

**Parameter queues and events without allocation.** Queues (up to 512 parameters per block, 128 points each; a full queue overwrites its last point, so the final value always arrives) and event lists (the instance's event capacity plus room for a note-off per held note) are allocated at activation. Untimed changes travel from the main thread to the processor in a lock-free ring. `tests/vst3_realtime.rs` drives the processor in-process under a counting allocator: no allocator call in any block.

**Held notes.** The backend keeps track of the notes it sent on. "All notes off" and "all sound off" (CC 123 / 120) release them explicitly at their offset (not every plug-in maps those controllers) together with the sustain pedal parameter; so do a block flagged as following dropped blocks, and `reset`.

**Pending changes and the state.** A change made with `set_parameter` or in the editor is in the controller at once but reaches the processor only with a block. Before the state is read the backend makes sure the processor has it: while audio runs, the next block delivers it (waited for up to 50 ms); otherwise the backend hands it over itself with a zero-sample `process` call, with the real-time thread excluded (`HostHandle::with_processing_excluded`) or, for an instance that is not activated, inside a temporary activation. Loading a state discards changes queued before it.

### Restart requests (`IComponentHandler::restartComponent`)

Flags are collected (from any thread) and handled in `Plugin::idle`, never inside the plug-in's own call.

| Flag | Reaction |
|---|---|
| `kLatencyChanged` | The new latency is valid after `setActive(true)`. Active instance: `HostEvent::RestartRequested` once; the host calls `deactivate()` + `activate()`, and activation reports `LatencyChanged` if the value differs from the last one reported. Inactive instance: `LatencyChanged` at once |
| `kParamValuesChanged`, `kParamTitlesChanged` | `ParametersRescan`; titles also drop the cached metadata |
| `kIoChanged` | Buses are negotiated again at the next activation (the host layout stays); `RestartRequested` if active |
| `kReloadComponent` | All of the above |
| `kMidiCCAssignmentChanged` | The controller mapping is read again |
| others (`kIoTitlesChanged`, routing, note expression, key switches, prefetch) | ignored |

### State container

`Instance::state()` is one blob: component state and controller state, as VST3 hosts have to keep both. Restoring gives the component state to the component and (`setComponentState`) to the controller, then the controller its own.

```text
offset  size  field
     0     4  magic "STV3"
     4     2  format version, little-endian (currently 1)
     6     2  flags, little-endian: bit 0 = a controller state follows the component state
     8    16  class id of the component (the bytes that read as its 32 hex digits)
    24     8  length of the component state, little-endian
    32     8  length of the controller state, little-endian (0 without bit 0)
    40     …  component state, then controller state
```

A blob that belongs to another class, has a newer format version, or whose lengths do not add up is refused before the plug-in sees it. A controller that answers "not implemented" without writing has no state of its own (flag bit clear); bytes written by a controller that then reports failure are not stored: the snapshot fails instead. A controller that rejects `setComponentState` is logged and otherwise left alone (as other hosts do); a component that rejects its state fails the restore.

### Editors

`IPlugView` is created and attached on the container's main thread.

* **macOS** (type-checked only): the view is attached (`kPlatformTypeNSView`) to the content view of a `macos::editor_window::EditorWindow`, sized from `getSize`. `IPlugFrame::resizeView` resizes the window and calls `onSize`; a view that answers `canResize` gets a resizable window whose user resizes go through `checkSizeConstraint` and `onSize`. Closing the window reports `EditorClosed` and dismantles the view from `idle`. `INFO_HAS_EDITOR` is found out by creating a view once at creation and asking it for the platform type.
* **Linux: no editor yet.** `open_editor` answers "unsupported" and instances do not advertise an editor. An editor needs an X11 parent window (`kPlatformTypeX11EmbedWindowID`), the `IRunLoop` host interface (file descriptors and timers) and an event loop in the container to drive both; none of it exists.

### Not implemented

64-bit processing; note expression, key switches, chord and scale events, data (sysex) events; `IUnitData` / `IProgramListData` (program data), `IUnitHandler`; context menus, `IProgress`, `IPlugInterfaceSupport`; `IEditController2` (knob mode, help, about); `IParameterFinder`, content scale support; multiple MIDI ports beyond the event bus index; routing info; `IoMode`; the `.vstpreset` file format; sample-accurate ramps (points are passed, but the host's event has no ramp); splitting blocks at parameter changes for plug-ins that only apply the last value of a block (the old host did that for instruments); and every plug-in specific workaround of the old host (the Massive X 1.7.1 start-up priming and state settling among them: such a plug-in may need the host to process some blocks before a state is loaded).

### Validating on a Mac

Nothing of this has run on macOS. With the container built there, take a few well-known free VST3 plug-ins, for instance Surge XT and Surge XT Effects, Vital, Dexed, TAL-NoiseMaker and Valhalla Supermassive (between them: instruments and effects, different frameworks, resizable editors, program lists, large states), plus any compressor with a sidechain input, and check:

1. Scan the standard folders: every bundle is listed with name, vendor, sub-categories and class id; nothing lands on the blocklist; a second scan starts no process.
2. Create each plug-in from its scan result; `info` shows the right buses, and instruments are flagged as such.
3. Process: an effect passes audio, an instrument plays notes at the right time; pitch bend, mod wheel and sustain reach the plug-in (they travel as mapped parameters).
4. Parameters: names, units, groups and list values look right; `set_parameter` is audible; automation events are sample accurate; values moved in the editor arrive as `ParameterChanged` with gestures.
5. State: save, change, restore, also across a container restart; a preset selected in the plug-in's own browser survives; program lists show up as presets where the plug-in has them.
6. Editor: opens at the right size above the host window, resizes (by the plug-in and by the user), keyboard input works, closing it from the window and from the host both leave the plug-in usable; opening it again works.
7. Latency: a plug-in with look-ahead reports it; switching the look-ahead produces `RestartRequested`, and after `deactivate()` + `activate()` the new value.
8. Sidechain: a compressor with a second host input bus reacts to it.
9. Offline: an export with `set_offline(true)` sounds the same as real time.
10. Quit with instances alive, and destroy the last instance of a module in a shared container: no crash on the way out (plug-ins with global services are the usual suspects).

## Audio Units (macOS)

Format `"au"`: version 2 components and version 3 extensions, both through `AUAudioUnit` (AudioToolbox, AVFAudio, CoreAudioKit for views), written in Rust on `objc2` and its framework crates. There is no Swift, Objective-C or C++ source in the tree. The container's main thread runs AppKit on macOS; the editor window helper is shared with the VST3 backend.

**Status: nothing in this section has run.** It was written on Linux. Everything that needs no Mac is in platform-independent modules and tested on Linux (54 unit tests); the rest is type-checked for `aarch64-apple-darwin` (clippy clean) and written from Apple's documented contracts. [Validating on a Mac](#validating-audio-units-on-a-mac) is the ordered list of what to run first, with `tests/au.rs` as its automated part.

```sh
cd crates/plugin-host
cargo test -p stpc-container --lib au::                                      # Linux: the platform-independent logic
CC_aarch64_apple_darwin=clang cargo check  --target aarch64-apple-darwin -p stpc-container --all-targets
CC_aarch64_apple_darwin=clang cargo clippy --target aarch64-apple-darwin -p stpc-container --all-targets
```

(`CC_aarch64_apple_darwin=clang` is needed from Linux only: objc2's `exception` feature compiles a small Objective-C shim in its own helper crate. On a Mac the default compiler does it.)

### Files

| File (`crates/container/src/`) | Platform | Contents |
|---|---|---|
| `au/ident.rs` | any, tested | `plugin_id` codec (`FourCc`, `ComponentId`), `ComponentKind`, the plain-data `ComponentInfo`, preset id mapping |
| `au/params.rs` | any, tested | parameter metadata: unit labels, flags, step detection, value sanitising, duplicates |
| `au/events.rs` | any, tested | event translation (MIDI validation, parameter events and ramps, absolute sample times), render slices, MIDI output offsets, transport flags and change tracking |
| `au/state.rs` | any, tested | state frame, canonical form of a state dictionary, capture reuse |
| `au/timing.rs` | any, tested | latency / tail sanitising and change detection |
| `au/layout.rs` | any, tested | channel capabilities, layout candidates, per-bus channel adaptation, mixing kernels |
| `au/backend.rs` | macOS | `AuBackend` (`PluginBackend`) and the `Plugin` implementation |
| `au/unit.rs` | macOS | instantiation, bus configuration, the few version 2 calls on bridged units |
| `au/render.rs` | macOS | `Processor`: render block, pull-input block, schedule blocks, MIDI output and transport blocks |
| `au/observe.rs` | macOS | key-value observation, parameter-tree observers |
| `au/plist.rs` | macOS | state dictionary to bytes and back |
| `au/editor.rs` | macOS | view controller request, generic editor fallback, window following the view |
| `au/listing.rs` | macOS | `list_components()` |
| `macos/app.rs` | macOS | `NSApplication` loop driving the runtime |
| `macos/editor_window.rs` | macOS | `EditorWindow`, used by the AU and VST3 backends |
| `tests/au.rs` | macOS | end-to-end tests against AUDelay, DLSMusicDevice, AUSampler (never run) |

Dependencies, macOS only (`[target.'cfg(target_os = "macos")'.dependencies]`, all MIT / Apache-2.0 / Zlib, default features off and only the needed ones enabled): `objc2` 0.6.5 (feature `exception`), `block2` 0.6.2, `objc2-foundation`, `objc2-app-kit`, `objc2-audio-toolbox`, `objc2-avf-audio`, `objc2-core-audio-kit`, `objc2-core-audio-types`, `objc2-core-foundation` 0.3.2. Transitively: `objc2-encode`, `objc2-exception-helper` (with `cc` as a build dependency), `bitflags`.

### The `plugin_id`

```text
<type>:<subtype>:<manufacturer>        aufx:dely:appl   (AUDelay)      aumu:dls%20:appl   (DLSMusicDevice)
```

Three four-character codes, byte by byte: bytes `0x21..=0x7E` except `%` and `:` as that ASCII character, every other byte (space, control characters, the bytes above `0x7E` of MacRoman codes, `%`, `:`) as `%XX`. A canonical id is printable ASCII without spaces and maps one to one to the three 32-bit codes; the decoder also accepts a literal space for `%20` and lower-case hex digits, and nothing else (in particular not the old host's decimal form). Types that can be created: `aumu` (instrument), `aufx` (effect), `aumf` (music effect), `aumi` (MIDI effect), `augn` (generator).

`options` (comma- or space-separated words): `out-of-process` asks the system to run a version 3 unit in its extension process; the default asks for in-process loading (the container is the sandbox; the system falls back to out-of-process by itself for an extension that is not packaged for it). Version 2 units always run in the container.

### What is covered, and how it maps to the traits

| `AUAudioUnit` capability | Trait method | Notes |
|---|---|---|
| Component lookup, asynchronous instantiation | `PluginBackend::create` | `AudioComponentFindNext` first ("not installed" is a clear error); always the asynchronous entry point, the main run loop is served while waiting (limit 100 s) |
| Bus formats, `channelCapabilities`, `maximumFramesToRender` | `create`, `activate` (new sample rate) | see [Buses](#buses-and-channel-layouts); the configuration is proven by allocating render resources once |
| `allocateRenderResources` / `deallocateRenderResources` | `activate` / `deactivate` | order as documented: frame limit and offline flag, fetch `renderBlock`, `scheduleParameterBlock`, `scheduleMIDIEventBlock`, then allocate |
| `renderBlock`, pull-input block | `Processor::process` | see [Rendering](#rendering) |
| `scheduleMIDIEventBlock`, `scheduleParameterBlock` | events in `process` | absolute sample times; MIDI only for units with a MIDI input |
| `MIDIOutputEventBlock` | `ProcessData::push_event` | units with MIDI outputs; sorted by offset; sysex dropped |
| `musicalContextBlock`, `transportStateBlock` | `ProcessData::transport` | set before the first allocation |
| `parameterTree` | `parameters`, `parameter_value`, `set_parameter`, `format_parameter`, `parse_parameter` | see [Parameters](#parameters) |
| Parameter and automation observers, KVO of `parameterTree` / `allParameterValues` | `HostHandle::notify` (`ParamValue`, `ParamGestureBegin/End`, `ParamsRescan`) | |
| `fullStateForDocument` / `fullState` | `state`, `set_state` | see [State](#state) |
| `factoryPresets`, `userPresets`, `currentPreset` | `presets`, `load_preset` | preset id: the factory preset number, or the magnitude of a user preset's (negative) number with bit 31 set |
| `latency`, `tailTime` | `InstanceInfo`, `LatencyChanged`, `TailChanged` | sanitised; observed and polled |
| `shouldBypassEffect` | `set_bypass` | effects only; `Unsupported` (host-side bypass) for other types, for version 2 units without a writable bypass property, and when the value does not read back |
| `renderingOffline` | `set_offline` | version 2: set live; version 3: render resources are cycled around the change, as the property is documented to be read at allocation |
| `reset` | `Processor::reset` | |
| `requestViewController`, `AUGenericViewController`, `preferredContentSize` | `open_editor`, `close_editor`, `focus_editor`, `set_editor_title`, `EditorClosed`, `EditorResized` | see [Editors](#editors-and-windows-on-macos) |
| `AVAudioUnitComponentManager` | `au::list_components()` | used by the scanner for the format `"au"` |

Not covered: sysex and MIDI 2 (`scheduleMIDIEventListBlock`), MPE and MIDI-CI profile calls, `parametersForOverview`, saving and deleting user presets, `contextName`, input and output device units, `renderQuality`, audio workgroups for a unit's own threads (`renderContextObserver`), view configurations of version 3 units, more than discrete layouts for buses wider than stereo, splitting render blocks at parameter events (see below).

### Buses and channel layouts

The host fixes the layout; host bus *n* is the unit's bus *n* (input bus 0 main, further input buses sidechains).

* The main bus pair is negotiated: the host's channel counts first, then configurations the unit states in `channelCapabilities` (nearest first), then the usual ones. Each candidate is set with `setFormat:error:` (deinterleaved 32-bit float at the instance's sample rate) and **proven by allocating render resources once**, because a unit judges the combination of its formats only then. The old host learned of a refused layout as an opaque engine error at the next start; here `create` fails with the unit's own error.
* Where the unit's width differs from the host's, the bus is adapted in the backend: a mono unit on a wider bus is fed the average of the host channels and its output goes to every host channel; a wider unit on a mono bus is fed the host channel on all inputs and its outputs are averaged; other mismatches use the channels both have. `BusInfo::channels` reports the channels the unit really uses.
* A sidechain bus the host allotted is given the host's width (else stereo, else mono) and enabled; one the host did not allot is **disabled**, so that the unit sees it as unconnected (the old host did the same). If a unit pulls a bus that has no host channels anyway, it gets silence.
* Further output buses (multi-output instruments) are rendered only where the host has a bus for them.

### Rendering

On the instance's real-time thread, per block: publish the transport; split the block into slices of at most the unit's `maximumFramesToRender` (normally one); for each slice schedule its events, then call the render block once per rendered output bus with the same time stamp.

* **No Objective-C on the real-time thread.** The render and schedule blocks are fetched and retained on the main thread before allocation and only *invoked* (a C call) while rendering; the pull, MIDI-output, musical-context and transport blocks are created on the main thread and read plain shared memory. No message send, retain, release, autorelease pool, lock or allocation happens in `process`; every buffer is sized in `activate`, and the blocks are released on the main thread in `deactivate`.
* **Output.** The buffer list points straight into the shared region where the widths match (no copy). A unit may render elsewhere and replace the pointers (in-place processing in its input, or its own buffers): the audio is then copied from where the unit says it is. `OutputIsSilence` clears the channels and sets the block's silence mask.
* **Input.** If the unit supplies buffers, the host's input is copied into them; if it passes null pointers, it gets pointers into the region for channels that map one to one and into prepared staging buffers for adapted ones.
* **Time.** Each instance has its own running sample counter, passed as the render time stamp (with the host time). An event at offset *o* of a block starting at sample *t* is scheduled at the absolute time *t + o* immediately before the render call that covers it, which is exact for any block size and across slices.
* **Parameter events** go to `scheduleParameterBlock` in the parameter's own range; a ramp length is passed on for parameters flagged as rampable and dropped (a jump at the offset) for others. Many version 2 units apply a timed change at the start of the render call rather than at its offset; the backend does not split blocks at events to force it.
* **Transport change.** `AUHostTransportStateChanged` is set for the first block, after a reset, on start and stop, on a position jump and after dropped blocks.
* A failing render call fails the block (the runtime silences it); the count and last status are logged from `Plugin::idle`.

### Parameters

The address is the id; values are in the unit's range. Names, `displayNameWithLength:`, key path (as `identifier`), enclosing groups (as `group`, joined with `/`), unit label, flags, and value strings come from the tree, read parameter by parameter so that one misbehaving parameter costs only itself.

Carried over from the old host:

* **Stepped parameters** are reported only where the unit can be trusted: unit "boolean" starting at 0, unit "indexed" with integer bounds, or a label list covering the whole integer range. Labels are passed on only when they describe every step; for indexed parameters that say their values have names but list none, the labels are fetched with `stringFromValue:`.
* **`stringFromValue:` always gets a real value pointer.** The bridge forwards it to version 2 plug-ins, some of which (JUCE's) dereference it although nil is allowed.
* **Stale bridge values.** Apple's version 2 bridge caches each parameter value and refreshes it only when the plug-in posts a change; plug-ins that apply a write on their next render and post nothing leave the tree stale indefinitely. For bridged units the value of a global-scope parameter is read from the unit itself (`AudioUnitGetParameter`). A value read back before the next render can still be the old one, as with CLAP.
* **Duplicate addresses** in a third-party tree: the first entry wins.
* **Trees that change.** The tree is observed (KVO) and compared on every idle tick; a replaced tree moves the observers, drops the cached metadata and sends `ParamsRescan`. `allParameterValues` sends `ParamsRescan` as well.
* Host writes use `setValue:originator:` with the observer's token, so they are not echoed back as edits made in the plug-in.

`AUParameter` has no default value. For bridged units the default comes from `kAudioUnitProperty_ParameterInfo`; otherwise it is the value the parameter had when it was first seen (right after instantiation for the initial tree).

### State

`state()` returns a small frame (`STAU`, version, payload kind, flags, the component's three codes, length) around `fullStateForDocument` (or `fullState` if the unit has none), serialised as a binary property list. `set_state` refuses a frame saved by another component with a readable message, and also accepts a bare property list, which is what an `.aupreset` file contains.

* **Values a property list cannot hold** (the old host failed the whole save on them): the dictionary is then written as a keyed archive, which keeps them; only if that fails too are the offending values left out, and the frame says so. Objective-C exceptions raised while reading, writing or restoring state become errors.
* **Byte-identical state for an unchanged plug-in.** Property-list serialisation does not order dictionaries deterministically. Each capture also computes a canonical form (keys sorted, number types preserved); if it equals the previous capture's, the previous bytes are returned. After a restore the loaded bytes become that reference. A capture always reads the unit's real state first; reuse never substitutes for it.
* After a state or preset load the backend re-reads the tree and the latency, and tells a bridged unit's parameter listeners that everything may have changed (what refreshes the unit's own Cocoa view).

### Editors and windows on macOS

`open_editor` asks the unit for its view controller (`requestViewControllerWithCompletionHandler:`, waiting up to 15 s with the run loop served) and falls back to the system's generic parameter editor (`AUGenericViewController`), so every unit has an editor. The view is embedded in an `EditorWindow`; the window follows the view when the view resizes itself (frame-change notification) and when the view controller changes its `preferredContentSize`; a view that autoresizes in both directions makes the window user-resizable. The user closing the window sends `EditorClosed`; the editor is dismantled on the next idle tick or `close_editor`.

`macos::editor_window::EditorWindow` (main thread; also used for VST3): `open(title, on_close)`, `content_view()` (the `NSView*` to embed into), `set_content_size(width, height)`, `set_resizable(resizable, on_resize)`, `set_title`, `focus()`, `close()`, plus `content_size()`, `window_number()` and `is_open()`.

How the windows behave next to the host application (mode `floating`, the default; decided on paper, to be confirmed):

* The container is an *accessory* application: no Dock icon, no menu bar, but its windows can become key. It installs a main menu that is never shown, because AppKit routes the standard key equivalents (copy, paste, undo, close) through it; there is no Quit item.
* Editor windows float above normal windows, so they stay above the host's while the user works in the host. To keep them from covering unrelated applications, the container watches the frontmost application: the windows are shown while it is the host (the parent process), this container or another container of the same host, and ordered out otherwise.
* Opening an editor does not take the keyboard from the host; a click into the window, or `focus_editor`, activates the container and makes the window key. On macOS 14 and later the host should yield activation to the container before `focus_editor` (`NSApplication.yieldActivation(to:)`), or the window comes to the front without becoming key.
* The windows may join a full-screen space and are excluded from window tabbing.
* `STPC_EDITOR_WINDOWS=normal` in the environment selects plain windows at the normal level without the tracking, for comparison.

The main loop (`macos::app::run`, called by `main.rs` on macOS instead of `Runtime::run`): `[NSApp run]` on the main thread; `Runtime::set_main_waker` signals a run-loop source in the common modes (so requests are served during tracking and modal loops too) whose callback calls `run_pending()`; a 50 ms timer does the same as a safety net and drives the periodic `Plugin::idle` (200 ms on macOS). `run_pending` is never re-entered when a plug-in, or this crate while it waits for an asynchronous Audio Unit call, runs the loop from inside a request. Objective-C exceptions escaping a request are caught and logged. The process declares a latency-critical activity so that App Nap leaves it alone.

### Threading and ownership, as reviewed

| Concern | What the code does |
|---|---|
| Main-thread-only calls | Every `AUAudioUnit` property, bus, tree, preset, state and view call is made from `Plugin` methods, which the runtime runs on the main thread, here the AppKit thread. |
| Completion handlers on arbitrary threads | Instantiation and view-controller handlers only retain the result, store it as a raw pointer under a mutex and wake the main run loop. A result that arrives after the timeout stays retained forever (releasing it there could run plug-in teardown on a foreign thread). |
| Observer callbacks on arbitrary threads | KVO callbacks set atomic flags and request `idle`; parameter observers push lock-free notifications. Neither adds or removes observers (the tree forbids it). |
| Observer removal | Tree tokens are removed from the tree they were added to before it is released; KVO registrations on the unit and on the view controller are removed before those objects are released; removal that raises is caught. |
| Block lifetimes | Render and schedule blocks: copied before allocation, held by the processor, released on the main thread. Blocks given to the unit (context, transport, MIDI output) and to each render call (pull) capture their state through `Arc`s, so a unit keeping one longer than expected cannot reach freed memory. |
| `Retained` and autorelease | All owned objects are `Retained`; each pass of the request loop runs in an autorelease pool; the real-time thread creates none and autoreleases nothing itself. |
| Exceptions | Every group of Objective-C calls made on behalf of a request runs under a catch that turns an exception into an error or a log line. |
| Failed or invalid units | Instantiation failure, a refused layout and a failed allocation are errors of `create` / `activate` with the system's message. A unit that stops working later (its extension process died) fails its render calls: the block is silenced and flagged, and the failure is logged; nothing detects it earlier. |

### Validating Audio Units on a Mac

In order; stop at the first step that fails. Steps 2 to 4 are `tests/au.rs`.

1. `cargo build -p stpc-container` builds and links (frameworks: AppKit, AudioToolbox, AVFAudio, CoreAudioKit, Foundation, CoreFoundation). `cargo test -p stpc-container --lib` passes as on Linux.
2. `cargo test -p stpc-container --test au -- --nocapture --test-threads=1 the_listing` lists more than 20 components, with AUDelay as `aufx:dely:appl`, DLSMusicDevice as `aumu:dls%20:appl`, AUSampler as `aumu:samp:appl`. No Dock icon appears for the container at any point.
3. `... audelay_metadata` : AUDelay is created (name "AUDelay", vendor "Apple", stereo, no latency); its four parameters have the documented ranges (mix 0..100 %, time 0..2 s default 1); a fully wet impulse comes back about 480 frames later at 10 ms; nothing before; values read back; a parameter event takes effect; blocks of 1, 17 and 1024 frames render. `... audelay_state` : state starts with `STAU`, is byte-identical when unchanged, restores parameter values, is refused by another unit; bypass passes the input through; offline mode and a sample-rate change keep rendering. `... a_mono_slot` : mono and mixed-width slots render.
4. `... dls_music_device` and `... ausampler` : silence before a note, sound after a note-on (the printed first sounding frame shows whether the offset of 100 is honoured). `... the_editor_opens` : the generic editor appears in a floating window titled as given, the title changes, `focus_editor` brings it forward and makes it key, the container answers a main-thread ping while the window is open, closing and reopening works. `... errors` : the three error messages read well.
5. With a third-party unit that has its own view (any JUCE-based effect, and one version 3 extension): the custom view appears at its own size; moving a control sends `ParameterChanged` (and gesture begin/end) to the host; copy and paste work in its text fields; closing the window with the mouse sends `EditorClosed` once and the editor can be opened again; a view that resizes itself is followed by the window.
6. Window behaviour against the host application: the editor stays above the host's main window when the host is clicked; it disappears when another application is activated and returns with the host; it shows on the host's full-screen space; `focus_editor` makes it key (macOS 14 and later: with the host yielding activation). Compare with `STPC_EDITOR_WINDOWS=normal`.
7. A unit with a sidechain input (a compressor): created with `input_buses: [2, 2]` it reports two input buses and ducks to the second; created with `[2]` it behaves as unkeyed.
8. A unit that reports latency (a look-ahead limiter): `InstanceInfo::latency` after `activate`, and `LatencyChanged` when a look-ahead parameter changes. A unit with an infinite tail reports `u64::MAX`.
9. A unit whose parameter tree changes (a multi-timbral instrument loading a patch): `ParametersRescan` arrives and `parameters()` returns the new list.
10. A large sampler instrument: `state()` of tens of megabytes round-trips; the same state captured twice is byte-identical.
11. Real time: an audio device at 64 and 128 frames with ten instances, no `PROCESS_LATE`; Instruments' allocation trace shows no allocation on the `stpc-rt-*` threads; the console shows no "autoreleased with no pool in place" from them.
12. Kill a container while an editor is open and audio runs: the host keeps playing, `restore()` brings the instance and its state back.

### Doubts that only a Mac can settle

Ranked by how much breaks if the assumption is wrong.

1. **The AppKit loop integration** (`macos::app`): that a version 0 run-loop source in the common modes is serviced promptly inside `[NSApp run]`, and that `stop:` plus a posted event ends the loop from a source callback. If requests stall, everything stalls; the 50 ms timer bounds the damage to latency, not correctness.
2. **Event timing through the base class.** Scheduling with absolute sample times from the render thread right before the render call is how the API is documented; whether every bridged unit then sees the intended buffer offset is unverified. The alternative, `AUEventSampleTimeImmediate + offset` (offsets below 4096), is selected with `STPC_AU_EVENT_TIME=immediate` in the container's environment, to compare the two without rebuilding.
3. **Trial allocation in `create`** (allocate, deallocate, allocate again in `activate`): correct by contract, and what `auval` does repeatedly, but slow or fragile units exist.
4. **Disabling unallotted sidechain buses** and whether every bridged unit then treats the sidechain as unconnected rather than failing its render.
5. **Window policy**: ordering out on foreign activation is derived, not observed; so is activation on macOS 14 and later, and the behaviour of a helper executable inside the host's bundle (it shares the host's bundle identity unless it is packaged as its own helper app with `LSUIElement`).
6. **In-process loading of version 3 units** and the fallback; view controllers of out-of-process units (remote views) and their `preferredContentSize` updates.
7. **Bypass detection** for version 3 units: the property always reads back, whether or not the unit implements bypass.
8. **Offline switching for version 3 units** cycles render resources, which resets the unit; if the reallocation fails the backend sends `RestartRequested`.
9. **MIDI effects (`aumi`)** need an output bus to be driven through the render block; units without one are refused ("does not provide an audio output").
10. The generated bindings' method signatures are trusted; in debug builds objc2 verifies each message's type encoding against the runtime and panics on a mismatch, which would show in step 2 or 3.

## Plug-in scanning

The host lists the installed plug-ins without loading any of them into itself (`stpc_client::Scanner`). Finding plug-in *files* is plain file system work in the host; reading what is *in* a file means loading it, which is done by a **sacrificial container process per file**: started for the purpose, asked with `Request::Scan { format, path }`, answered with plain descriptions, and gone afterwards. A file that crashes or hangs its process costs that process only; it is recorded and the scan goes on.

```rust
let scanner = Scanner::new(ScannerConfig {
    cache_path: Some(app_support.join("plugins.stpcscan")),
    extra_paths: vec![("vst3".into(), user_folder)],       // besides the standard folders
    ..ScannerConfig::default()                              // formats vst3, clap (+ au on macOS); 30 s per file
})?;
let report = scanner.scan(ScanOptions::default(), &mut |event| match event {
    ScanEvent::Discovered { total } => { /* files to look at */ }
    ScanEvent::Started { format, path, pid } => { /* a process is scanning this file */ }
    ScanEvent::Completed { completed, total, path, outcome, .. } => { /* progress */ }
});
// report.plugins: Vec<PluginDescription>; an instance is created from one with
// InstanceConfig { format, plugin_id: id, options: path, .. }
```

| API | |
|---|---|
| `Scanner::new(config)` | Loads the cache. `Scanner` is `Clone + Send + Sync`; clones share the cache |
| `scan(options, progress)` | Blocks until done; progress events on the calling thread. `ScanOptions::retry_failed` scans failed files again, `rescan_all` ignores the cache |
| `cancel()` | From any thread (or the progress callback): scanning processes are killed, `scan` returns with `cancelled`; files not finished keep what the cache knew, nothing is blocklisted |
| `plugins()` | All known plug-ins, by name; a plug-in present in several files is listed from the folder that comes first in `search_paths` |
| `files()`, `blocklist()` | Every file with status (`Ok`, `Failed`, `Crashed`, `TimedOut`) and reason; the blocklist is the crashed and timed-out ones |
| `unblock(format, path)`, `clear_failures()` | Forget a file / all failed files, so the next scan tries again |
| `scan_file(format, path)` | One file now, whatever the cache says |
| `search_paths(format)`, `standard_search_paths(format)` | The folders searched |

**Search paths.** The user's own folders (`ScannerConfig::extra_paths`, which may also name single plug-in files) come first, then the standard ones unless `standard_paths` is off: VST3 `~/.vst3`, `/usr/lib/vst3`, `/usr/local/lib/vst3` (Linux) or `~/Library/Audio/Plug-Ins/VST3`, `/Library/Audio/Plug-Ins/VST3`, `/Network/Library/Audio/Plug-Ins/VST3` (macOS); CLAP `~/.clap`, `/usr/lib/clap` (Linux) or `~/Library/Audio/Plug-Ins/CLAP`, `/Library/Audio/Plug-Ins/CLAP` (macOS), then every entry of `$CLAP_PATH`. Folders are searched recursively (8 levels) for entries named `*.vst3` / `*.clap`, files or bundle directories; bundles are not looked into; hidden entries are skipped; a bundle reached twice (links, overlapping folders) counts once. Audio Units have no files: `"au"` is one request for the system's component registry (`au::list_components()`), answered inside a container without loading any unit, and asked again on every scan.

**What a description holds** (`PluginDescription`): format, id (VST3 class id, CLAP plug-in id, AU `type:subtype:manufacturer`), name, vendor, version, the category as the format states it and its parts (VST3 sub-categories, CLAP features, AU tags), the path, the SDK / format version, a URL, and flags: instrument / effect / note effect, audio in / out, MIDI in / out, sidechain, and "has an editor" with a separate "known" bit. Only what a factory or registry tells without instantiating is used, so for VST3 and CLAP the audio and MIDI flags are derived from the categories and the editor is unknown; Audio Units always have one.

**Scanning one file.** The container registers one scanner function per format (`Runtime::register_scanner`; `container/src/scan.rs`). VST3: load the module, read the class factory (only "Audio Module Class" entries are plug-ins), unload it again. CLAP: `clap_entry.init`, the plug-in factory's descriptors, `deinit`. No plug-in is instantiated. The host waits for the answer for `ScannerConfig::timeout`, then asks the process to exit (so that module exit functions run) and kills it if it lingers. Up to `ScannerConfig::jobs` files are scanned at a time.

| What happens | Recorded as | Next scan |
|---|---|---|
| The container answers with plug-ins | `Ok` | From the cache, no process |
| The container answers with an error (not a plug-in, cannot be loaded, no factory) | `Failed` + message | Skipped |
| The process dies (signal or exit) | `Crashed` + how it ended: **blocklisted** | Skipped |
| No answer within the time limit; the process is killed | `TimedOut`: **blocklisted** | Skipped |
| The scanning process cannot be started, or the scan is cancelled | nothing | Tried again |

Skipped files are scanned again when they change, when asked to (`unblock`, `clear_failures`, `retry_failed`, `scan_file`).

**Cache.** One file, written atomically (temporary file and rename), every 16 scanned files and at the end. Keyed by format and path; an entry is valid while the file's **fingerprint** is unchanged: size and modification time, which for a bundle directory are summed and maximised over the files in `Contents` and its direct sub-directories, so that replacing the binary inside a bundle is noticed. Entries of files that are gone are dropped by a completed scan.

```text
"STPCSCAN"            8 bytes
version               u32 little-endian, currently 1 (another version: the file is ignored)
entries               u32 count, then per entry, in the control protocol's encoding:
  format, path        strings (u32 length + UTF-8)
  size, mtime_ns      u64, u64: the fingerprint
  status              u32: 0 ok, 1 failed, 2 crashed, 3 timed out
  reason              string
  plugins             u32 count + PluginDescription (format, id, name, vendor, version, category,
                      subcategories, flags, path, sdk_version, url)
```

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
cd crates/plugin-host
cargo test
cargo test --release
STPC_TEST_WAKE=pipe cargo test  # the portable wake-up path
STPC_SHM=posix cargo test       # the shared-memory path macOS uses
cargo test --release -p stpc-container --test realtime -- --ignored --nocapture --test-threads=1   # measurement
CC_aarch64_apple_darwin=clang cargo check --target aarch64-apple-darwin --all-targets   # clang: an Objective-C shim of objc2
```

| Suite | Tests | Covers |
|---|---|---|
| `transport` unit tests | 13 | encoding round trips and hostile input, region layout and attach checks (including a version mismatch), notification queue (overflow, wrap, 4 producers), futex and pipe waits, descriptor passing, 5 MB frames, listener cleanup, shm without leftover names |
| `tests/integration.rs` | 33 | bit-exact audio for 7 bus layouts (1 to 64 channels) and block sizes 1 to 4096 changing per call, arbitrary bit patterns; sample-accurate MIDI and parameter events; MIDI out; event overflow; sidechain; transport and flags; parameter metadata, text, set/get; presets; editor open/title/focus/close and editor-originated edits; 12 MiB state round trip; latency reporting and change; bypass (plug-in and latency-matched client bypass); reconfigure; process count under each policy (1, 2, 4, 6 for six instances); crash on the real-time thread and on the main thread (abort, segfault, exit code), in create, activate and state load; crash isolation and flagging within the block; late block and automatic recovery, with the discontinuity flag reaching the plug-in; hang detection, kill and restart; hung main thread with audio continuing; offline mode; restart with state, replayed parameters and activation; periodic state snapshots; failed restores; slow instantiation and creation timeout; version mismatch (both directions), bad invocations, host death; clean shutdown; stale directory sweep |
| `tests/realtime.rs` | 3 (+1 measurement) | zero allocator calls on the audio thread in the normal path (4500 blocks with events, MIDI out, transport), the inactive and bypass paths, deadline miss / late / resync, and while and after a container crash |
| `tests/stress.rs` | 1 | 3 seeds x 400 random operations (create, destroy, parameters, state, kill, internal crash, restore, restart, policy change, activate/deactivate, reconfigure) against a concurrently processing audio thread; about 60 to 90 container processes per seed |
| `client` unit tests | 4 | scanner: discovery in folder order (bundles not looked into, links, hidden files), bundle fingerprints, cache file round trip and damaged caches, standard search paths |
| `container` unit tests (VST3, scanning) | 8 | class id text form, fixed-size strings, the lock-free ring, parameter queues (order, overflow, the COM view of them), plug-in events to MIDI, the state container (round trip, foreign, newer, damaged blobs), scanner requests that make no sense |
| `tests/vst3.rs` | 9 | the VST3 backend against a real VST3 module: metadata (Unicode class info, factory vendor), parameters with units, groups and list values, text both ways, bit-exact audio for block sizes 1 to 4096, sample-accurate automation and untimed changes, output parameter (meter), state round trip of component and controller state and refused blobs, state after changes made without audio (active and inactive flush), bypass parameter, reset, reconfigure; mono, sidechain (stereo and mono), surplus host buses, a refused layout bridged; sample-accurate notes, MIDI controller mapping, all-notes-off and reset releasing held notes, note and legacy MIDI output, silence flags; latency restart protocol (inactive, active, through a state); program list as presets, MIDI program change, editor edits and dirty flag; process context and offline mode; segfault, abort and hang in `process` with restart and state restore, a hung main thread with audio continuing; module entry / exit per last instance with no object left alive; loading errors |
| `tests/vst3_realtime.rs` | 1 | zero allocator calls in the VST3 processor over 300 blocks with dense automation, sidechain, notes, mapped controllers, pitch bend and events out (in-process, counting allocator) |
| `tests/scan.rs` | 4 | scanning real CLAP and VST3 plug-in files next to bundles that abort on load, segfault in the factory, hang, hold text, are empty, and a real shared library that is no plug-in: results, duplicates, blocklist with reasons, no process or socket left; a second scan and a new `Scanner` on the cache file start no process; a changed file is rescanned alone; `unblock`, `scan_file`, `retry_failed`, `rescan_all`, removed files, `clear_failures`; cancellation; instances created from scan results; an unknown format, a missing container, an executable that is no container |
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
| `container/src/vst3/module.rs`, macOS part | Loading through `CFBundle` (hand-declared CoreFoundation functions), `bundleEntry` / `bundleExit`, the bundle version. |
| `container/src/vst3/editor.rs` | The whole editor: `IPlugView` in an `EditorWindow`, `IPlugFrame`, resizing. See [Validating on a Mac](#validating-on-a-mac). |
| `client/src/scanner.rs`, `container/src/scan.rs` | The macOS search folders, CLAP bundles, and the Audio Unit registry listing behind `"au"`. |

### Integration notes

* **Locating the container.** Put `supertracker-plugin-container` next to the application's main executable (`Contents/MacOS/`), which is where `Supervisor` looks by default. It must be code-signed with the application; under the hardened runtime it needs the same entitlements a plug-in host needs (`com.apple.security.cs.disable-library-validation` to load third-party plug-ins, audio input if plug-ins record).
* **Socket path length.** `sockaddr_un` holds 104 bytes on macOS and `$TMPDIR` is long there (`/var/folders/…/T/`). `Supervisor::new` fails early with a clear error if the path would not fit; set `SupervisorConfig::socket_dir` to something short (e.g. a directory under `~/Library/Caches/<app>`).
* **Wake-up.** `os_sync_wait_on_address` exists from macOS 14.4; on 14.0 to 14.3 the code falls back to `__ulock_*`, which is private API (the Rust standard library uses the same fallback). If that is not acceptable, or if the first runs show a problem with shared address waits, set `SupervisorConfig::wake = WakePreference::Pipe`: plain POSIX, verified on Linux, with millisecond deadline granularity on macOS.
* **Real-time priority.** The host's audio thread now sleeps while a container thread does the work, so that thread must be scheduled like an audio thread. The runtime requests a time-constraint policy per real-time thread. Joining the device's audio workgroup (`os_workgroup_join`) is what Apple recommends and is not done: the workgroup would have to be sent to the container as a Mach port. Measure dropouts on Apple silicon (efficiency cores) before deciding whether that is needed.
* **Editor windows.** They belong to the container process. A backend that opens real windows must (1) run `NSApplication` on the container's main thread and drive the runtime from it (`set_main_waker` + `run_pending`, see above; the built-in `Runtime::run()` loop does not pump AppKit), (2) choose an activation policy (accessory: no Dock icon), (3) implement `focus_editor` by activating the container application and ordering the window front, and (4) return the window number in `EditorInfo::window_id` if the host wants to order its own windows relative to it. Keyboard focus and window ordering across two processes (plug-in window above the main window, hiding with the application, Spaces, full screen) need to be designed on a Mac; Bitwig's behaviour is a reasonable reference. Reported through `HostEvent::EditorClosed` when the user closes a window.
* **Audio Units and VST3.** The `Plugin` / `Processor` traits were shaped for them: main-thread objects need not be `Send`; `activate` returns the audio half; parameter ids are 64-bit (AU addresses); parameter values travel in the parameter's own range (AU: min..max, VST3: 0..1); `identifier`, `group`, `unit_label`, `value_strings` and `step_count` carry the metadata the current rack shows; state is opaque bytes (wrap a property list or component + controller state as the backend sees fit). AUv2/AUv3 view controllers require the AppKit loop mentioned above.
* **Architectures.** A container is a separate executable, so an x86-64 build of it could host Intel-only plug-ins under Rosetta next to an arm64 host. Whether shared address waits work between a translated and a native process is unknown; pipe mode avoids the question.
* **Sandbox.** An App Sandbox host cannot spawn an arbitrary helper with inherited rights in the way assumed here; this design is for a non-sandboxed (Developer ID) application.

## In the application

The application's host uses the client through `st_audio::plugins` (`crates/st-audio/src/plugins/`, `cargo doc -p st-audio`): a `PluginManager` owns the `Supervisor` and one instance per plug-in of the project, `PluginInsert` puts an instance into the mixer graph as an effect or an instrument (one `process_raw` call per block, parameter changes and notes as timed events, the time left in the device callback as the deadline, no deadline in the offline copy used for exports), `Catalog` wraps the `Scanner`, and a recovery store holds back plug-ins that crash twice in a row while loading. The bridge commands are in `st_host::areas::plugins` (`docs/HOST.md`).

## Open points and risks

* Nothing has run on macOS (see above). The first thing to do there is to run the test suite itself: it has no Linux-only logic except the `/proc` checks in `tests/common/mod.rs` (process existence, children), which need a macOS equivalent.
* Real-time behaviour under load is only as good as the scheduling of the container threads. Without real-time priority the worst case is a scheduler quantum (see the measurements); the deadline plus fallback keeps the host's audio thread from stalling, at the price of a dropout on that instance.
* A deadline miss drops whole blocks for that instance and tells the plug-in (`discontinuity`); events of dropped blocks are lost, so a note-off can go missing. Hosts should send all-notes-off after `PROCESS_RESYNCED` for instruments.
* The default deadline (one block period, at least 2 ms) is a guess. The right value is the time left in the audio cycle, which only the host knows: pass it per block.
* Editors: CLAP has floating windows only; VST3 editors exist for macOS (type-checked only) and not at all for Linux, where the container has no window system event loop.
* The VST3 backend has met one plug-in: its own test plug-in. Real plug-ins disagree with the specification in many small ways (results of `setProcessing`, `setBusArrangements`, `setComponentState`; threads `restartComponent` is called from; state loaded before the first block), and the old host needed a start-up profile for one of them. The backend is lenient where other hosts are, but only running real plug-ins will tell.
* VST3 parameters automated by the host or changed by the plug-in are reported to the edit controller and the host on the container's main thread, once per block at most: a plug-in with many meters produces a steady stream of `ParameterChanged` events for read-only parameters, which a host should not put into its undo history or its restore set.
* Scanning reads factories only. What needs an instance (the real bus layout, whether there is an editor, parameters) is known only after creating one; `PluginDescription` flags say what is derived from categories. A plug-in that shows a licence dialog or blocks on a dongle while its module loads times out and is blocklisted; `unblock` or `scan_file` retries it, and `ScannerConfig::timeout` is the knob.
* The scanner trusts the fingerprint: a plug-in replaced by a file of the same size and modification time is not rescanned (`rescan_all` does it).
* Sysex, MIDI 2 and parameter ramps are not carried end to end (the event has a ramp field; no backend uses it yet).
* Windows: `channel.rs` and `shm.rs` are Unix only; the wake-up abstraction would get a third implementation (`WaitOnAddress` does not work across processes; an event object or a named pipe would).
* CLAP: an untimed `set_parameter` on an active plug-in waits for the next block, is not flushed when no block arrives (`clap_plugin_params.flush` is not called for an active plug-in whose host has stopped sending blocks), and is not discarded by a state or preset load that follows it, so it lands on top of the loaded state. The VST3 backend handles both cases itself. The application's host works around it (`st_audio::plugins::Hosted`: a value reads as set at once, and state reads and loads wait for the block that delivers pending values); the backend should do it.
* Restoring an instance replays the parameter values set through `set_parameter` since the last known state. A value that made the plug-in crash is replayed too.
* The container binary honours `STPC_TEST_PROTOCOL_VERSION` (announces another protocol version); it exists for the version-mismatch test.
