# Maschine Compressor parameter mapping

Verified on 2026-10-11 by static inspection of the installed **Maschine 3.7.1** universal executable:

`/Applications/Native Instruments/Maschine 3/Maschine 3.app/Contents/MacOS/Maschine 3`

SHA-256: `9013ba0958decfc02dd37e0f8777ef243c792c8d507798b582af2622706730d1`.

All addresses below are unslid virtual addresses in its **arm64** slice. No NI executable, plug-in, constructor, setter, or audio process was executed. `nm`, read-only file mapping, and bounded `llvm-objdump` output were used.

## Serialized order and versions

`NI::MASCHINE::DATA::Compressor::serialize<portable_binary_iarchive>` begins at `0x101a05800`; the output archive counterpart begins at `0x101a0594c`. They agree on this order:

1. `NI::MASCOMMON::DATA::Module` base object.
2. **Mode enum, only for Compressor version 1.**
3. Threshold numeric parameter.
4. Amount numeric parameter.
5. Knee numeric parameter.
6. Attack numeric parameter.
7. Release numeric parameter.
8. Gain numeric parameter.
9. SideChainFilter object.

Version **0 omits Mode** (`cbz w21` at `0x101a05834` / `0x101a05978`); the constructor default is Classic. The version checker at `0x101a058a4` accepts 0 and 1 and rejects values at least 2 (`cmp w0,#2` at `0x101a058c0`). Mode is a raw enum: **0 = Classic, 1 = Feedback**, with default 0. Its label strings are at `0x104d27ad8` and `0x104d27af8`; the two-entry vector is assembled at `0x101a09ce8..0x101a09d60`.

These are structured Boost parameter records, not six adjacent floats. `MaschineNumericParameter<float>::save` at `0x100357d50` saves its bases plus multiple raw fields; its load at `0x100357a98` restores them. An archive reader must retain those record boundaries and class metadata.

## Main parameter domains

The full data constructor is `0x101a05a38` (alias `0x101a067e8`). All six numeric parameter constructors receive minimum 0 and maximum 1. Let `u` denote the extracted normalized parameter value. The displayed conversions below are verified from the attached converter and corresponding setter, where applicable.

| Numeric serial index | Name | Member offset | UI position | Default `u` | Source display conversion |
| --- | --- | --- | --- | --- | --- |
| 0 | Threshold | `0xb0` | 1 | float32 `0x3e2aaaab`, approximately 1/6 | `-60 * u` dB |
| 1 | Amount | `0xc0` | 3 | 0.75 | `100 * u` percent |
| 2 | Knee | `0xb8` | 2 | 0.5 | `100 * u` percent |
| 3 | Attack | `0xc8` | 4 | `(0.25 / 8)^(1/3)`, approximately 0.3149803 | `8 * u^3` ms |
| 4 | Release | `0xd0` | 5 | 0.5 | `clamp(B^(32 + 16*u), 40, 251)` ms |
| 5 | Gain | `0xd8` | 6 | float32 `0x3f6aaaab`, approximately 11/12 | `-36 + 36*u` dB |

Here `B` is the exact float32 constant `0x3f8f9e4d`, or approximately `1.1220184564590454`. Thus Release endpoints display 40 and 251 ms, and its midpoint is approximately 100 ms. The default source values are approximately -10 dB Threshold, 75% Amount, 50% Knee, 0.25 ms Attack, 100 ms Release, and -3 dB Gain.

**Amount and Knee swap positions between the serialized order and the UI order.** `getParameterByPosition` at `0x101a06910` proves the UI sequence; its 13-byte table at `0x104d27a38` is `00 05 0a 0f 14 19 1e 34 23 27 2a 2e 31`. Position 0 is Mode, positions 1–6 are the values shown above, position 7 is empty, and positions 8–12 address the sidechain controls.

## Conversion evidence

| Claim | Bounded evidence |
| --- | --- |
| Constructor names and member offsets | UTF-32 strings `0x104d27b50` Threshold, `0x104d27b78` Knee, `0x104d27b8c` Amount, `0x104d27ba8` Attack, `0x104d27bc4` Release, `0x104d27be4` Gain; member stores at `0x101a05ad0`, `0x101a05afc`, `0x101a05b2c`, `0x101a05b6c`, `0x101a05b98`, `0x101a05bd4` |
| Threshold display | `FloatLinearConverter(0,-60,"%.1f dB")` setup at `0x101a06234..0x101a0625c`; Classic `setThreshold` at `0x101a18e98` multiplies `u` by -60 |
| Amount and Knee display | Both parameter converter pointers are assigned at `0x101a05d90..0x101a05da4` to `0x10dce2238`, symbol `NI::MASCOMMON::DATA::DefaultConverters::ScaleNormalizedToPercentOneDecimal` |
| Attack display and default | `FloatPowerConverter(0,8,3,"%.2f ms")` setup at `0x101a09dc0..0x101a09dd8`; constructor invokes `convertFrom(0.25)` at `0x101a05b30..0x101a05b44`; Classic `setAttack` starts at `0x101a18f00` |
| Release display | `FloatExponentialConverter(40,251,B,32,16,"%.1f ms")` setup at `0x101a062b8..0x101a062e8`; `convertTo` at `0x1024adcdc` proves exponent and endpoint clamp; Classic `setRelease` at `0x101a18f5c` uses the same law |
| Gain display | `FloatLinearConverter(-36,0,"%.1f dB")` setup at `0x101a06334..0x101a0635c`; Classic `setGain` at `0x101a18fd0` computes `36*(u-1)` |
| Dispatch IDs | `CompressorProcessor::processFloatParameter` at `0x101a066fc`, table `0x104d27a30 = 00 05 09 0d 11 15 18 18`, maps IDs 0–5 to Threshold, Amount, Knee, Attack, Release, Gain respectively |

`FloatLinearConverter` at `0x1024adfd0` stores an offset and `(upper-lower)`; its `convertTo` is at `0x1024ae074`. `FloatPowerConverter::convertTo` is at `0x1024ae17c`, and `convertFrom` at `0x1024ae1a4`. Classic Attack uses a `1e-10` numerical floor internally; the displayed endpoint remains 0 ms.

## Amount, Knee, and target compatibility

The bounded setters establish these facts, **not a conventional ratio or knee-width conversion**:

- Classic Amount (`0x101a18ebc`) stores `u` and the internal coefficient `u*(2-u)`.
- Classic Knee (`0x101a18ee0`) stores `u` and the internal coefficient `0.5*u`.
- Feedback mode receives the same raw IDs/values through `FBCompressorCore::setParameter` at `0x101a113dc`. Its Amount branch at `0x101a1146c` constructs a nonlinear internal coefficient from `19*u`; its Knee branch at `0x101a11504` begins with `0.5+4.5*u` and performs additional gain-table conversions.
- The source UI converters remain percent-based in both modes. Feedback mode's internal timing coefficients also differ from its displayed Attack/Release values. Matching display values alone does not establish matching DSP behavior.

No gain-computer proof was established for the proposed formula `ratio=1/(1-u)^2`, and no exact mapping to SuperTracker's knee width in dB was established. An importer should preserve the raw Amount/Knee values in source metadata and report that these controls were not reproduced exactly, rather than inventing a verified conversion.

SuperTracker's existing first-party Compressor has narrower lower bounds for Attack (0.1 ms) and Gain (-24 dB) than the source ranges here. Clamping those source values should be reported. Mode labels and four displayed physical quantities can be mapped; the resulting processor is still an independent implementation, not a NI sound-compatible emulation.

## SideChainFilter boundary

`SideChainFilter::serialize<portable_binary_iarchive>` at `0x101847404` and its save counterpart at `0x101847504` serialize five records in member order:

| Order | Member offset | Constructor name | Domain established here |
| --- | --- | --- | --- |
| 0 | `0x00` | Source | Structured routing/source parameter; not a numeric float |
| 1 | `0x08` | Gain | Numeric `[0,1]`, default 1 |
| 2 | `0x10` | Center Freq | Numeric `[0,1]`, default 0.5 |
| 3 | `0x18` | Width | Numeric `[0,1]`, default 0.5; percent display converter |
| 4 | `0x20` | Filter | Boolean, default false |

The constructor is `0x10184755c` (called through alias `0x101847aac`). Its name literals are `0x104d090fc`, `0x104d09118`, `0x104d0912c`, `0x104d0915c`, and `0x104d09174`, respectively. The SideChainFilter version checker at `0x101847464` accepts only version 0. Gain/frequency/width physical DSP conversion and source-routing resolution were not established by this bounded analysis; keep this object separate from the six main numeric parameters.

## Reproducing the static reads

Use `nm -C -arch arm64` on the exact executable above, confirm its SHA-256 and `CFBundleShortVersionString`, then disassemble the bounded ranges in this note with `llvm-objdump` and read the listed strings/jump tables. Do not load the inspected executable. Address claims must be re-established for any different binary hash.
