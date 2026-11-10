//! SuperTracker's audio engine: the Rust port of `Sources/TrackerEngine/TrackerEngine.cpp`,
//! exporting every function declared in `Sources/TrackerEngine/include/TrackerEngine.h`.
//!
//! # Threading model (unchanged from the C++ engine)
//!
//! * One serial **control thread** edits a draft [`project::Project`] and publishes a complete
//!   immutable copy through an atomic pointer (`begin_update` / setters / `commit_update`).
//! * The **render thread** adopts the latest project at the next block boundary, consumes
//!   transport and note commands from a bounded single-producer/single-consumer ring, and never
//!   blocks, locks, allocates or frees. Replaced projects go onto a lock-free retired list.
//! * `collect()` on the control thread destroys retired projects and the samples and timing maps
//!   nothing refers to any more.
//! * Status, peaks and the voice playhead snapshot are atomics / seqlocks readable anywhere.
//!
//! # Modules
//!
//! | module | C++ code it replaces |
//! |---|---|
//! | [`abi`] | the header's structures and constants |
//! | [`util`] | `clamp`, `finite`, `Ring`, `FloatBlock`, `randomBipolar` |
//! | [`clock_ffi`] | `EngineTiming`, `fixedClockContext`, the `st_clock_*` calls |
//! | [`project`] | `Sample`, `Zone`, `Instrument`, `Step`, `Lane`, `Pattern`, `Project`, timing compilation |
//! | [`tables`] | `ChannelTables`, `BusRing`, `PeakStore`, `FreezeHistory`, `TrackState`, `RatchetState` |
//! | [`control`] | draft/commit/collect, `buildTables`, engine construction and destruction |
//! | [`state`] | `STEngine`'s data members, `Voice`, `Scheduled`, `Command` |
//! | [`transport`] | `refresh`, Play/Stop/Seek, clock, loops, queued jumps, input acks, checkpoints |
//! | [`sequencer`] | `triggerRow`/`triggerStep`, step FX, ratchets, plug-in locks, automation lanes |
//! | [`voice`] | `noteOn`, `kill`/`release`, `voiceSample` and all voice DSP |
//! | [`mixer`] | `process`, sends (delay/reverb), metronome, timeline clips |
//! | [`render_cache`] | `renderCached`, bus rings, retained clock/freeze history and span queries |
//! | [`capi`] | the `extern "C"` functions |

pub mod abi;
pub mod capi;
pub mod clock_ffi;
pub mod control;
pub mod mixer;
pub mod project;
pub mod render_cache;
pub mod sequencer;
pub mod state;
pub mod tables;
pub mod transport;
pub mod util;
pub mod voice;

pub use abi::*;
pub use state::STEngine;
