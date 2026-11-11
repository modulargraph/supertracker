# Built-in effects and drum synthesis

These are first-party SuperTracker processors, written from scratch. The control
vocabulary and effect families take inspiration from the public
[Maschine effect reference](https://docs.native-instruments.com/ni-tech-manuals/maschine-mk3-manual/en/effect-reference).
They are approximate counterparts: no NI DSP, impulse responses, samples, or
proprietary implementation code is included. A matching name does not establish
matching sound, preset normalization, or parameter response. Ice has a limited
offline NI reference-render comparison described below; other families do not
yet have comparative NI renders or controlled listening validation.

## Catalogue

| Family | Stable processor IDs |
| --- | --- |
| Dynamics | `compressor`, `gate`, `transient`, `limiter`, `maximizer` |
| Filtering | `eq`, `filter`, `cabinet` |
| Modulation | `chorus`, `flanger`, `fm`, `freqshifter`, `phaser` |
| Space | `ice`, `metaverb`, `reflex`, `reverblegacy`, `reverb`, `plate` |
| Delay | `beatdelay`, `graindelay`, `grainstretch`, `resochord` |
| Distortion | `distortion`, `lofi`, `saturator` |
| Perform | `performfilter`, `performflanger`, `burstecho`, `resoecho`, `ring`, `stutter`, `tremolo`, `scratcher` |

The `distortion` processor offers Feedback Fuzz and Analog modes; `saturator`
offers Classic, Tape, and Tube. Reverb includes Room, Hall, and Plate modes in
addition to the separate plate processor.

Compressor uses stereo-linked peak detection, a soft-knee gain curve, and
feedforward/feedback detection. Gate adds hysteresis, hold, and attack/release.
Transient Shaper compares fast and slow envelopes. Peak Limiter has a fixed
3 ms lookahead and a sample ceiling; this is **not** a true-peak limiter.
Maximizer uses a separate loudness curve and optional second compression pass.

Four-band EQ implements two shelves and two parametric bells using standard
[biquad equations](https://www.w3.org/TR/audio-eq-cookbook/). Multimode Filter uses
a topology-preserving state-variable filter, LFO/tempo/envelope modulation, and
external key input. Cabinet is an algorithmic filter/reflection approximation,
not a measured cabinet impulse response.

Chorus and Flanger use interpolated modulated delays. Phaser uses four/eight
all-pass stages. Audio FM modulates a split-band delay, with a fixed 64-frame
latency. Frequency Shifter uses a 63-tap Hilbert FIR and quadrature oscillator,
with a fixed 31-frame latency. The short Hilbert FIR has imperfect rejection of
the unwanted sideband near DC and Nyquist; it is a creative effect.

## Control pages and the insert chain

Every built-in effect has one shared description in `ui/effect-pages.js`: named pages of up to eight
parameters in a musical order, grouped in sections, with short knob names (Maschine-style, for example Multimode
Filter: Main = Cutoff, Reso, Mode · Mod amount, Mod source · Mix; then LFO, Envelope and Sidechain pages). Ranges,
units, curves (linear, logarithmic, stepped), defaults and choice labels are not copied: they come from the native
catalogue through the bridge command `builtin.effects`, and values are shown in
their own units (16 kHz, −18 dB, 4:1, 1/8., Band-pass). The plug-in pages console, the insert chain and the MK3
PLUG-IN pages all use these pages; other plug-ins keep NKS or generated pages.

The insert chain (`insert-chain.js`) edits the inserts of any owner (track, bus, kit Sound or group, master): modules
as tiles in signal order (+ adds from a categorized list, drag or ⌥ ← → reorders, the power button or B bypasses,
D duplicates, ⌫ removes), and the selected module's pages as eight knobs (drag, ⇧-drag fine, double-click or ⌫
resets, ⇧ ← → pages, A automates). It is in the mixer (above the classic per-insert details) and the kit editor.
All chain edits go through `window.STInserts` (add / remove / setBypass / move / duplicate by owner and slot, each
one Undo step), which the MK3 Bypass and Remove buttons use too. `tests/effect_pages_tests.js` checks the pages
against the real catalogue (`tests/fixtures/builtin-effects.json`, generated and kept up to date by
`crates/tracker-engine/tests/effects_metadata.rs`).

## Hosting contract

`tracker_engine::effects` exposes the catalogue, physical parameter metadata, normalized
conversion, instance creation, atomic parameter/tempo/bypass mailboxes, reset,
latency/tail metadata, and in-place stereo processing with optional sidechain.
The host should persist processor IDs and parameter IDs/values, not catalogue
indices. Parameter metadata has log/integer/toggle flags and enum labels.

Create and destroy are non-render operations. Process/reset/destroy have one
serialized owner. Parameter, tempo, bypass and metadata getters may overlap
render. Reset preserves selected parameter targets but clears oscillator, delay,
filter and envelope state. Rendering, including parameter changes and sidechain
copying, performs no heap allocation. Scratch capacity does not limit callback
size: larger calls are processed in chunks. Tempo supports 20–999 BPM.

Tail metadata uses fixed family bounds or a pure calculation from current
atomic parameter/tempo targets, available before the first render and after
parameter restoration. Filter ringing is included. Long, high-feedback Beat
Delay settings can report tails of hours; the host should let the user choose
an intentional export duration. Freeze,
captured Grain Stretch, and looping performance effects
can sustain indefinitely; their 120-second value is an explicit offline-export
cap, not a guarantee of natural decay. The host must expose intentional export
duration/tail choices for sustained material.

Bypass returns input exactly and freezes DSP state. The host is responsible for
bypass fading and latency alignment. Processing sanitizes nonfinite input and
output; normal signal overload is still possible in creative/EQ chains, so gain
staging remains important. Feedback loops have bounded coefficients/state.

## Insert chain nodes

The mixer graph of `st-audio` runs a strip's inserts back to back on the strip's buffer: a built-in effect is
processed in place in the host (`tracker_engine::effects::Effect`), a plug-in in its container. Scheduled
automation (lanes, step-FX locks) reaches each effect's lock-free parameter queue with its frame offset.

## Drum families

`tracker_engine::effects::drums` supplies six allocation-free per-voice synthesizers: kick
(swept sine/harmonic body), snare (dual body modes plus filtered noise), hi-hat
(six band-limited square sources plus noise), tom (four modal partials),
percussion (decaying FM), and cymbal (eight inharmonic modes plus noise).
The public [drum documentation](https://docs.native-instruments.com/ni-tech-manuals/maschine-mk3-manual/en/working-with-plug-ins)
informs the workflow. These are six original families, not all of NI's individual
named drum engines. Imported NI model/parameter payloads require explicit
mapping; copying their raw serialized floats into these controls is invalid.

Each has eight normalized controls: Tune, Decay, Tone, Impact, Noise, Attack,
Drive, Character. Decay has a model-specific logarithmic range; metadata and
defaults come from `tracker_engine::effects::drums`. The engine supplies note/root pitch
ratio and applies velocity, channel gain, envelopes and routing. A voice's own
decay terminates it even if no note-off arrives. Noise is seeded deterministically.

## Offline validation

Run `cargo test -p tracker-engine --release` (the effect suites are
`tests/effects_*.rs`; `effects_builtin` and `effects_drum_synth` are the two described here). The effects suite checks every registered
processor at 48/96 kHz: parameter ranges and conversions, exact bypass,
deterministic reset, nonfinite/aliasing sidechain handling, boundary controls,
zero render allocations, and callback partition invariance. Measured assertions
cover compression ratio, external keying, limiter ceiling/latency, EQ gain,
low/high-pass rejection, neutral FM reconstruction, frequency-shift sideband
direction and transient emphasis. Timings are a local throughput smoke check,
not a claim about worst-case live performance.

The drum suite checks metadata, defaults, all control endpoints, deterministic
noise/reset, voice termination, finite bounded output, no trigger/render
allocation, distinct family output, keyboard transposition and Tune scaling.
These pure DSP tests establish behavior and stability, not NI sound similarity.

## Space and delay implementation notes

Room/Hall/Plate and Metaverb use an eight-line orthogonal feedback delay network
with different delay families and diffusion. Legacy adds room characters, dry
pan, EQ, and freeze. Ice uses twelve complex resonators with a Color-controlled
frequency band, nonlinear damping, and a diffuse space. Its maximum Ice value
retains finite decay, consistent with the measured reference settings. Reflex uses
six short resonating combs and variable all-pass diffusion.

Beat Delay provides tempo synchronization, stereo feedback crossover, filtering,
width, and musical half-unit Time choices. Grain Delay uses a deterministic
preallocated 32-grain cloud with jitter, reverse, pitch, density and space.
Grain Stretch captures 32 sixteenth notes when On rises, then loops the capture
with independent speed/pitch using eight grains. Resochord uses six tuned combs
with chord and string modes. Prepared buffers cover the supported 20 BPM floor:
Beat Delay reserves 51 seconds of stereo storage; Stretch 24 seconds; Grain
Delay six seconds. No render-time allocation is required.

`crates/tracker-engine/tests/effects_spatial_delay.rs` has checks at 44.1/48/96 kHz covering
tempo echoes, ping-pong, predelay, decay, stereo response, Ice resonant-band movement/decay,
Resochord tuning, granular octaves, capture/retrigger, block partitioning,
boundary parameters and zero render allocations. The suite also passes
AddressSanitizer and UndefinedBehaviorSanitizer. Tail tests include slow-tempo,
high-feedback echoes, long resonating combs, dense grain clouds and restored
parameters before the first render. Space/delay processors calculate current
target decay where useful, rather than advertising only fixed family maxima.

### Ice reference calibration (2026-10-11)

Maschine 3's GUI exported seven Ice settings plus a dry sampler baseline from a
private sample-only group using an original generated break. Exports are 48 kHz,
32-bit float, normalization and loop optimization off. Hardware template groups
were excluded and transport was not played. The exporter repeats the source at
8 seconds, so analysis uses only the first eight seconds, with the same exported
dry baseline passed into our standalone processor. No NI audio is embedded in
our DSP or shipped tests.

The reference revealed that Color moves a resonant band from bass to treble,
and all tested maximum-Ice settings still decayed. This corrected the earlier
fixed-pitch bank and forced high-Ice sustain. At Color75/Size50, Ice75's wet RMS
in the second second is about 0.00197 versus NI 0.00193; tail spectral centroid
is about 850 Hz versus 772 Hz. At Ice100 it is 2152 Hz versus 2121 Hz; Color0
measures 66 Hz versus 66 Hz, and Color100 8544 Hz versus 8424 Hz. Size100 also
extends decay in both processors. These are measurements of one generated
source and several endpoints, not proof of perceptual equivalence, identical
modal frequencies, nonlinear response, or every parameter combination.
The seven settings cover Ice0/75/100, Color0/75/100, and Size50/100. Regression
checks protect broad measured band movement, wet level and finite decay, alongside
all existing sample-rate, allocation, determinism and boundary checks.
