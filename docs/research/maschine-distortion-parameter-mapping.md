# Maschine distortion parameter mapping

Static evidence from Maschine 3.7.1, arm64 slice of
`/Applications/Native Instruments/Maschine 3/Maschine 3.app/Contents/MacOS/Maschine 3`.
SHA256: `9013ba0958decfc02dd37e0f8777ef243c792c8d507798b582af2622706730d1`.
No vendor code, application or device was executed. Symbols came from `nm`, followed by a bounded disassembly (both local research
outputs, not kept in the repository). Referenced UTF-32 strings were
read with a read-only Mach-O parser (not part of this repository).

`MaschineGroupDistortionMapping.map(type:version:parameters:)` accepts only the
three exact schemas below and finite payloads. It returns first-party effect IDs,
sorted normalized parameter values, and warnings. The importer retains the complete
original fields separately. These conversions do not reproduce NI DSP.

## LoFi v0 — five floats

Constructor `0x101a48a6c` names members at offsets `a8,b0,b8,c0,c8` as SR, Bits,
Smooth, Stereo, Mix; all are stored in 0…1. Serializer `0x101a49ecc` writes the
different order **SR, Smooth, Bits, Stereo, Mix** (`a8,b8,b0,c0,c8`).

| Index | Source | Conversion |
|---:|---|---|
| 0 | SR | `44100 * Float(0x3f879c63)^(-105.5*u)` Hz |
| 1 | Smooth | `100*u` percent |
| 2 | Bits | `12-11*u` bits |
| 3 | Stereo | `100*u` percent |
| 4 | Mix | `100*u` percent |

SR display converter `0x101a492e8` explicitly multiplies the power by 44100;
`0x3f879c63` is approximately 1.05946004. This is an absolute display frequency,
not the host sample rate. Bits is independently confirmed by
`LoFiCore::processEventInBits` at `0x101a1dac4`: `12-11*u`, followed by `2^(bits-1)`.
Smooth and Stereo select similar first-party controls, with a disclosed difference
in filtering/stereo processing.

## Saturator v0 — enum, eleven floats, two booleans

Constructor `0x101a739a4`; output serializer `0x101a738d0` writes offsets
`a8,b0,b8,c0,c8,d0,d8,e0,e8,f0,f8,100,108,110` sequentially.
Global initializer `0x101a77fb4` establishes mode labels Classic, Tape, Tube.

| Index | Field | Stored domain | Conversion/status |
|---:|---|---|---|
| 0 | Mode | integer 0…2 | Same target mode |
| 1 | Classic Input | 0…1 | Approximate `24*u` dB, explicitly warned |
| 2 | Classic Drive | 0…1 | `100*u` percent |
| 3 | Classic Contour | 0…1 | `100*u` percent |
| 4 | Tape Input | 0…1 | Verified `30*u` dB, target limited to +24 dB with warning |
| 5 | Tape Drive | **−1…1** | Verified `3*u` dB; target knob `50+25*u` yields this LF tilt |
| 6 | Tape Contour | 0…1 | Verified `(12+8*u)` kHz; inverse target cutoff law gives knob |
| 7 | Tube Drive | 0…1 | `100*u` percent |
| 8 | Tube Charge | **0…4** | Verified `25*u` percent |
| 9 | Tube Bass | 0…1 | Unknown dB law; retain neutral target, warn |
| 10 | Tube Treble | 0…1 | Unknown dB law; retain neutral target, warn |
| 11 | Tube Gain | **−12…12 dB** | Direct output dB |
| 12 | Tube Overload | boolean | Approximate off/+12 dB boost, explicitly warned |
| 13 | Tube EQ Bypass | boolean | Direct toggle |

Display conversion functions `0x101a74ac4…0x101a74bec` prove the Tape and Tube
unit conversions above. Only the active source mode writes the shared target
controls; inactive mode settings remain in the original import data with a warning.
No claim is made that original tape warmth equals our low-frequency tilt, or that
the original tape contour equals our low-pass filter.

## MullhollandDrive v1 — enum, three floats, boolean, seven floats, boolean, two floats

The source identifier is spelled `MullhollandDrive`. Constructor `0x101a514bc`
establishes the names and bounds. Serializer `0x101a5134c` prepends the enum and
six Analog members to the older eight Fuzz members. Mode labels are Mulholland
(0) and Analog (1), established by initializer `0x101a55570` and UTF-32 strings
`0x104d30ec8`, `0x104d30ef4`.

| Index | Member offset | Name | Domain |
|---:|---|---|---|
| 0 | a8 | Mode | integer 0…1 |
| 1 | f0 | Analog Saturation | 0…1 |
| 2 | f8 | Analog Bass | 0…1 |
| 3 | 100 | Analog Tone | 0…1 |
| 4 | 108 | Analog Focus | boolean |
| 5 | 110 | Analog Definition | 0…1 |
| 6 | 118 | Analog Mix | 0…1 |
| 7 | b0 | Fuzz Drive | 0…1 |
| 8 | b8 | Fuzz Color | 0…1 |
| 9 | c0 | Fuzz Feedback | **−1…1** |
| 10 | c8 | Fuzz Tone | 0…1 |
| 11 | d0 | Fuzz Tone Mod | **−1…1** |
| 12 | d8 | Fuzz Gate | boolean |
| 13 | e0 | Fuzz Release | 0…1 |
| 14 | e8 | Fuzz Mix | 0…1 |

`ReleaseDisplayScaler::getValueAsString` at `0x101a51468` calls
`scaleReleaseValue` at `0x101a178e0` and multiplies by 1000. The exact float
constant form is `1000 * (Float(0x3c23d70a) +
(u² + (u⁴-u²)*Float(0x3f4f41f2))*Float(0x409fae14))` ms: approximately 10…5000 ms.
The first-party 2000 ms ceiling is disclosed whenever applied.

Drive uses an explicitly approximate 0…42 dB curve; Tone uses an explicitly
approximate 150…16000 Hz exponential curve. Analog uses its own Tone/Mix and
Saturation control, with the extra Fuzz drive gain set to unity. Percent controls
and booleans are mapped directly. Negative Fuzz Feedback/Tone Mod are unsupported
by the positive-only first-party processor, so they are disabled with explicit
warnings and their signed originals remain in import data. They are never clamped
into a positive amount or misread as normalized controls. Positive feedback is
limited to the target's 95% stability bound with a warning.

## Validation

Run `bash tests/run_maschine_distortion_mapping_tests.sh`. This compiles only the
pure mapping helper, test code and first-party builtin effects catalogue; it does
not launch an application, open audio/MIDI, or load a vendor binary. Tests check
serializer indexing, active-mode separation, signed and non-normalized domains,
known unit curves, warnings, invalid enums, count/version validation and finite
normalized results.
