# DSP efficiency review — 11 October 2026

The portable C++ engine now avoids repeated work while preserving the rendered PCM and clock checksums in 13 comparison scenes. Public headers and ABI are unchanged by this performance patch. The baseline snapshot includes the earlier correctness fixes for sample-region isolation, frozen transport restoration and section-loop timing.

## Changes

- Cache Hermite interpolation bounds when a voice starts or its automated sample region changes. Stereo channels and overlapping grains reuse those bounds.
- Process only the set bits in the instrument automation mask, retaining ascending parameter order.
- Advance zero-depth LFO phase and random generators without evaluating an inaudible waveform.
- Use an exact integer power of two for bit-depth quantization.
- Evaluate native mono oscillators’ filter, drive and quantizer once, then mirror channel history before the existing pan law.
- Cache the immutable timing-map terminal frame instead of searching the map once per sample.

## Measurement

Apple M4, 10 CPU cores, 24 GiB memory; Apple clang 21.0.0, C++17, `-O2`, without fast-math. Each trial renders 96,000 frames at 48 kHz in 256-frame blocks. Five adjacent A/B pairs per scene alternate execution order. The benchmark uses user-initiated QoS on macOS and reports process CPU time separately from elapsed/block time. Hosted-bus scenes checksum all 16 buses and clock positions. Legacy-mix scenes include delay and reverb.

| Scene | CPU before → after (ms) | CPU reduction | Median block before → after (µs) | Block p95 before → after (µs) |
|---|---:|---:|---:|---:|
| saw-16 | 43.1 → 41.0 | 4.9% | 114 → 107 | 123 → 119 |
| saw-64 | 132.0 → 128.0 | 3.1% | 351 → 337 | 378 → 362 |
| saw-256 | 506.6 → 490.7 | 3.1% | 1334 → 1302 | 1465 → 1380 |
| loop-64 | 306.5 → 285.0 | 7.0% | 828 → 763 | 883 → 829 |
| loop-256 | 1224.1 → 1147.6 | 6.3% | 3270 → 3086 | 3564 → 3488 |
| saw-mod-64 | 490.1 → 399.7 | 18.4% | 1200 → 1065 | 1715 → 1137 |
| saw-mod-256 | 1723.7 → 1571.4 | 8.8% | 4600 → 4202 | 4839 → 4415 |
| granular-mod-64 | 779.4 → 799.0 | -2.5% | 2104 → 2084 | 2284 → 2916 |
| wavetable-mod-64 | 630.6 → 616.6 | 2.2% | 1670 → 1629 | 1785 → 1751 |
| slice-mod-64 | 777.7 → 734.3 | 5.6% | 2314 → 2173 | 2441 → 2298 |
| mapped-host-16 | 71.9 → 69.3 | 3.6% | 173 → 165 | 209 → 186 |
| mapped-host-loop-64 | 684.2 → 635.7 | 7.1% | 1800 → 1677 | 1929 → 1753 |
| automated-region-lfo-64 | 518.0 → 403.9 | 22.0% | 1361 → 1052 | 1576 → 1255 |

The block columns are medians of each trial’s median and p95, respectively. All 130 final renders produced matching baseline/current PCM and clock checksums with zero intercepted C++ render allocations. Granular timings are within noise; no improvement is claimed for that scene. Other development work was running during measurement. CPU scheduling and thermal changes remain relevant, and these results are neither cross-DAW comparisons nor a guarantee that hosted plugins meet device deadlines. The first, noisier exploratory measurements are retained in the evidence directory.

## Correctness checks

The final engine passed all 46 existing engine suites, 17 timing suites, step-FX regressions, the earlier 13 audit checks, and the new DSP behavioral tests. AddressSanitizer and UndefinedBehaviorSanitizer runs passed for both the earlier audit checks and the new DSP tests. The reusable comparison runner was also executed end to end against the preserved baseline. These checks preserve the existing sampler-region, transport-checkpoint and section-loop fixes.

## Reproduction

Run focused behavioral checks with `bash scripts/test-engine-dsp.sh`. They cover both LFOs, all five waveform shapes, inaudible random/phase continuity, changing sampler-region boundaries in six modes, and zero render allocations.

For a fresh immutable comparison, supply a checkout or source snapshot from before this patch:

```sh
python3 scripts/benchmark-engine-dsp.py \
  --baseline-root /path/to/baseline \
  --output /path/to/new-evidence \
  --repetitions 5
```

The runner saves source snapshots, compiler commands, hashes, hardware metadata, every raw trial, and a summary. It fails on PCM/clock mismatch or render allocation, but deliberately applies no unstable timing pass threshold. This is a pure offline engine test; it opens no audio device or vendor plugin.

The run records were kept locally with the source snapshot; they are not part of the repository.

## Parameter-boundary bug fixed during review

Six native integer conversions accepted finite floats larger than an integer can hold: bit-depth and slice-count automation, slice preview FX, and sequenced slice, ratchet and arpeggio FX. All six now saturate in floating point before converting. UBSan reproduced every original failure; 36 boundary/equivalence cases (six destinations × six extreme/fractional values) pass combined ASan/UBSan and compare PCM against explicitly bounded input. These are correctness fixes after the timed performance snapshot; `parameter-bounds/` preserves their separate before/after evidence.

## Further work

The read-only clock audit found that precomputing tempo-boundary frame coordinates can reduce large-map searches; its isolated candidate passed 13 clock groups and 900,000 context comparisons, but that candidate is not included in production. Broader span-based clock evaluation needs careful boundary/invalidation tests before adoption. Granular wrapping/window math and the native host/plugin graph should be profiled separately before larger architectural changes.

## Kit-channel optimization

The kit implementation expands native routing to eighty channels. A bounded follow-up now clears inactive Sound buffers once per block and visits only active Sound channels, while retaining ordinary track summation order and cache history. In the isolated 48 kHz comparison, four seconds of silent configured-kit processing drop from about 183 ms to 42 ms; one active Sound among sixty-four drops from about 176 ms to 48 ms. These figures cover native cached preparation, not the entire plugin graph.

Nineteen kit groups pass with no render allocations, including sparse final-Sound, mute/solo/send automation and overlapping cache reads. Nine prior fixtures preserve byte-identical PCM, events and status over 2,689,489 stereo frames and 164,952 event/status records. Optimized and sanitizer runs pass.

The host DSP display also now spans master PreRender to PostRender, instead of only native preparation, and uses consistent percentage units below and above one percent. Dormant strip allocation and constant-control processing remain separate profiling opportunities.

## Whole-host kit throughput

The final optimized release C passes 81 manual-offline trials: nine scenes, 128/256/512-frame blocks, three trials each, at 48 kHz. Each trial warms 8,192 frames then measures 192,000 frames through `renderOfflineQuantum`; setup, PCM validation and reporting are outside the timer. Every scene checks finite PCM and the expected audible or silent output. No audio device, external MIDI or vendor plugin participates. Other native test runs and compilation were stopped during measurement.

At 256 frames, median total render time as a percentage of audio duration is 2.53% for ordinary silence, 3.73% for one silent kit, 7.30% for four silent kits, 3.82% for one sample Sound, 17.37% for sixty-four simultaneous sample Sounds, and 6.15% for six DrumSynth Sounds with group EQ. Corresponding process CPU readings are 3.08%, 4.80%, 9.94%, 4.88%, 20.01% and 7.22%; process CPU includes all app threads. For the sixty-four-Sound case, the median of trial p99 render durations is 1.032 ms against 5.333 ms of audio per block. These are throughput observations on this Mac, not device-deadline or cross-DAW guarantees.

Reproduce using an optimized private build that includes the self-tests (`scripts/build.sh selftest <private>.app`) with `SUPERTRACKER_NO_MK3=1 SUPERTRACKER_SUPPORT_DIR=<private-support> <app-executable> --kits-performance <output>`. The earlier B run rejected a benchmark bus missing its required name; C fixes the fixture without relaxing production validation.

## Graph node overhead — 26 October 2026

After the per-effect DSP work, most of a kit's cost was AVAudioEngine per-node overhead. The routed graph now renders
far fewer nodes:

- Consecutive built-in inserts of a strip render in one insert-chain node (see [BUILTIN_EFFECTS.md](BUILTIN_EFFECTS.md));
  built-in inserts no longer get a converter mixer.
- A track without an AU instrument or line input feeds its native source straight into its strip, and its input stage
  is that source (no compensator, no one-input mixers). A track whose channel moves reconnects live.
- An owner's output route uses the owner's mix processor as its solo gate when nothing else but delay/reverb sends
  leaves it (meters still read before the gate). The pre-fader stage collapses onto the last insert.
- Delay/reverb send routes exist only for tracks whose send can be audible (a level, a delay/reverb step FX or lane);
  a return's first send rebuilds, later ones join live, and unneeded ones stay until the next rebuild.
- Compensation delays have block headroom, so steady delays use the block-copy path; the effect wrapper's input/output
  clean-up is vectorized (bit-identical).

Optimised self-test builds, offline full graph, 48 kHz, median % of real time (`--kit-dsp-measurement`: eight empty
template tracks plus a real imported Maschine group with its built-in effects; `--render-performance`), same machine,
back to back:

| Scenario | Rendered nodes | 128 frames | 256 frames | 512 frames |
|---|---|---|---|---|
| 1988 Kit (3 effects) | 228 → 73 | 2.72 → 0.98 | 1.58 → 0.62 | 1.15 → 0.51 |
| Ronka Kit (55 effects) | 407 → 102 | 7.92 → 4.22 | 5.83 → 3.49 | 4.27 → 2.58 |
| Evolet Kit (35 effects) | 368 → 103 | 6.96 → 3.66 | 4.68 → 2.68 | 3.42 → 2.00 |
| Template + kit, playing | 234 → 67 | 3.22 → 1.48 | 2.24 → 1.26 | 1.78 → 1.15 |
| Template + kit, stopped | 234 → 67 | 2.21 → 0.53 | 1.28 → 0.35 | 0.88 → 0.26 |
| 64 sampler tracks | 527 → 139 | 10.99 → 7.02 | 8.82 → 6.51 | 7.93 → 6.29 |

What remains is mostly DSP: the effects themselves in the heavy kits (reverb networks, saturators, EQs) and the
native instruments in the template scenarios.
