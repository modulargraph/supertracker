# Maschine dynamics and modulation import controls

Verified on 2026-10-11 against the installed Maschine 3.7.1 arm64 executable,
SHA-256 `9013ba0958decfc02dd37e0f8777ef243c792c8d507798b582af2622706730d1`.
The analysis read symbols, bounded constructor/serializer/converter disassembly,
and parameter-name literals. No NI executable, plug-in, or DSP was run.
This is saved-state interoperability information, not a claim of sonic equivalence.

`MaschineGroupAdditionalMapping.swift` consumes raw values in the following
verified serializer order. Constructor/UI order can differ. Unknown versions,
wrong field counts and nonfinite payloads fail closed; invalid individual fields
keep target defaults with warnings. The importer retains original numeric fields.

| Type / version | Serialized fields |
| --- | --- |
| Gate 0 | Threshold, Attack, Hold, Release, Mix |
| Limiter 1 | Threshold, Mode, Release, Ceiling; embedded SideChainFilter excluded |
| Maximizer 0 | Amount, Curve, Turbo |
| TransientDesigner 0 | Input Gain, Attack, Sustain, Limit |
| CabinetEmulation 0 | Cabinet, Microphone, Distance, Mix |
| Chorus 0 | Rate, Amount, Mix |
| FM 0 | Rate, Split, Contour, Amount |
| Filter 0 | Mode, Cutoff, Resonance, Amount, Source, Speed, LFO Shape, Phase, Decay, Smooth, Shape |
| Flanger 0 | Frequency, Amount, Speed, Shape, Stereo, Feedback, Invert, Mix |
| FreqShifter 0 | Coarse, Fine, Feedback, Stereo, Invert, Mix |
| Phaser 0 | Frequency, Amount, Source, Speed, Shape, Stereo, Feedback, EightPole, Mix |

## Verified unit conversions

- **Gate Threshold:** raw range float32 0.1…1, displayed `40*log10(raw)` dB.
  Constructor `0x101a316a8`, parameter insertion `0x101a3170c`, converter
  `GateThresholdDisplayConverter::convertTo` at `0x101a32dc0`, static coefficient
  construction `0x101a31b38..0x101a31b50`. The coefficient is double bits
  `0x40315f2ced384f29`, equal to `40/ln(10)`.
- **Limiter Threshold:** raw physical dB, -40…0, not normalized. The serializer
  places this before Mode, despite the constructor's Mode-first order.
  Constructor `0x101a44190` (parameter insert `0x101a44228`).
- **Limiter Release:** `clamp(pow(B, K*raw),1,500)` milliseconds, where B is
  float32 `0x402df854` and K is float32 `0x40c6de12` (approximately e and ln500).
  `FloatExponentialConverter(1,500,B,0,K)` setup `0x101a44718..0x101a4474c`.
- **Limiter Ceiling:** `-40+39.7*raw` dB, linear converter setup
  `0x101a44798..0x101a447c8`.
- **Transient Input Gain:** raw -1…1; `raw*24` dB below zero and `raw*6` dB
  at or above zero. `TransientDesignerGainConverter::convertTo` at
  `0x101a7d964..0x101a7d978`; constructor assigns it to member `0xb8` at
  `0x101a7d35c`. Attack/Sustain are signed percentages -100…100.
- **Filter Cutoff:** exponential `46*pow(12540/46,raw)` Hz. Constructor
  `0x101a22ba4` uses `FloatExponentialConverter` at global `0x10e305fa0`;
  `getFrequencyValueString` at `0x101a238c0` calls that converter. Initializer
  `0x101a28b10..0x101a28b2c` supplies float32 endpoint bits `0x42380000` and
  `0x4643f000`. The existing generic exponential converter was independently
  checked by the core mapping research.
- **Filter modes:** LP, BP, HP, Notch. Enum vector initialization
  `0x101a2899c..0x101a28a14`. Source enums for Filter/Phaser are LFO, LFO Sync,
  Envelope (`0x101a28a54..0x101a28ac0`, `0x101a5bbe4..0x101a5bc70`). Flanger v0
  has no serialized Source; the current constructor's free LFO default is used.
- **Cabinet/microphone enums:** four cabinets in American 2x12, British 2x12,
  Vintage 4x12, Modern 4x12 order; six microphones Dynamic57/421/441,
  Ribbon121, Condenser47/67. Initializer `0x101a7c600` assembles the labels.
  Target responses are original filters and reflections, not the NI responses.
- Percent controls retain their raw signedness. Flanger and Phaser Amount are
  bipolar; their target metadata now permits negative sweeps. Constructor
  Amount member `0xb0` accepts -1…1, despite Frequency using 0…1.

## Deliberate approximations and unmapped fields

The original UI exposes many timing/frequency controls as percentages through
`DefaultConverters::ScaleNormalizedToPercentOneDecimal` (`0x10dce2238`). That
proves neither a physical Hz/ms curve nor identical DSP response. The mapper
labels these translations explicitly:

- Gate timing: 0.05…100 ms logarithmic Attack, 0…1000 ms Hold, 1…2000 ms
  logarithmic Release.
- Chorus Rate: 0.03…8 Hz logarithmic; FM Rate: 0.1…10000 Hz and Split:
  20…16000 Hz logarithmic. Maximizer Amount: 0…30 dB.
- Flanger/Phaser Frequency: 20…5000 / 20…10000 Hz logarithmic; Speed:
  0.03…8 Hz; stereo phase: 0…180 degrees; envelope shape: 0.25…4 logarithmic.
- Filter signed Amount: ±5 octaves; Speed: 0.03…16 Hz; Phase: 0…360 degrees;
  envelope Decay: 5…2000 ms, Smooth: 0.1…100 ms, Shape: 0.25…4 logarithmic.
- Original continuous LFO waveform morphing is not equated with target enum
  indices. Filter retains its sine default; modulation processors use their
  own sinusoidal LFO. Synced speed division mappings are unverified, so the
  target four-beat period remains, with a warning.
- Frequency Shifter Coarse/Fine percentage-to-Hz laws were not established.
  Target shifts stay at zero with a specific warning. Other controls map.
- Separate sidechain source/filter objects are not part of these flat arrays.

All returned effects include an independent-implementation warning. Exceeding
our physical range reports clamping. The mapping is useful editable import
state; it does not establish perceptual or sample-identical NI compatibility.

Run `bash tests/run_maschine_additional_mapping_tests.sh` for 155 pure checks
covering all eleven types, field order, signed values, physical endpoints and
midpoints, strict payload boundaries, enum validation, clamping, and warnings.
