# Maschine expansion Group import

The importer reads installed `.mxgrp` files without running Maschine or a vendor plug-in. Loading an imported group adds it to the session as a track group: one track per loaded pad plus the group bus ([kits](KITS.md)). Imported kits have sixteen stable Sound IDs, individual instrument and insert chains, and a dedicated Group bus. A Sound's position comes from the declared sixteen-object vector, tracked object identities and the native member layout. Filenames and the Group preview never determine pad assignment.

## Format coverage

Groups written by Maschine 1.8 up to Maschine 3 are accepted: Boost archive versions 10 and 12–19, Group versions 1–23 and Sound versions 1–6 (legacy three-ID parameters) and 6, 8 and 9 (Maschine 2+ four-ID parameters). The Sound version selects a verified object-ID layout (`MaschineGroupArchive.layout`): the distance from each Sound to its name parameter (versions 1–3: 92/89, 4–5: 95/92, 6: 93/90, Maschine 2 version 6: 119/115, versions 8–9: 643/639, first Sound/later Sounds) and the shift of the mixer members. Every file is still validated member by member, so an unknown version, or a known version with a different layout, is refused. Other forms found in the installed libraries:

- Later Sounds normally reference tracked members of the first Sound; external-MIDI templates give every Sound its own copy. Exactly one of the two name positions must validate.
- Empty pads have an empty name and import as “Sound N”. Sounds without any insert Slot (audio-input pads, MIDI-only templates) are allowed; a Group without any Slot is accepted only when no Module class is registered at all.
- Sampler version 0 (Maschine 1.x) lacks the member at 0xc0; its unset loop end (0) spans the playback range. PluginHost version 0 uses legacy Bool parameters.
- Sample references may be library-relative with a leading `/` or `//` (`//Samples/…`); only the expansion's own `Samples` folder is read that way. Expansions installed without a “… Library” folder name are found by their `.nicnt` file.
- An Audio module (Maschine 2 loop player) imports its single sample as a whole-file zone; tempo sync, stretching and play modes are reported as not reproduced.
- REX2 slices (`.rx2`) play from the WAV rendering shipped beside them only when the slices tile that file exactly; otherwise the zones stay unavailable with a warning.

All 5,513 installed Groups (97 expansions) now decode; previously 1,762 did. BassSynth, PolySynth, PerformanceFX, AudioInputModule and some Maschine 1.x effect/DrumSynth versions (Compressor 0, PlateReverb 0, Limiter 0, MullhollandDrive 0, Hihat 1, Kick 1, Perc 0) and Flanger 1 have no verified control schema yet; those Sounds or inserts are reported per Sound and their original state stays in the import report. In the installed Deep Matter and Molten Veil libraries, the regression corpus contains 90 Groups, 1,440 Sounds, 1,055 Samplers, 2,185 sample zones and 2,124 internal effect/DrumSynth modules. An additional 48 external PluginHost records are inspected separately.

Sample zones retain their expansion-relative source, MIDI key and velocity ranges, root note, physical tuning, gain, pan, sample trim and loop ranges. Native sample ends are inclusive; SuperTracker uses an exclusive endpoint. Repeated references to a break preserve separate slice ranges and key assignments. Source files remain in their installed library. Missing media is reported, and neither an unrelated sample nor a kit preview is substituted.

Internal effects are independent SuperTracker DSP implementations. The importer decodes typed native fields and converts only supported control meanings through versioned mapping helpers. This does **not** promise matching NI sound, algorithms or every parameter curve. The report distinguishes converted approximations, defaults and unavailable modules. External plug-in state is opaque and is not converted between VST2, VST3 and AU formats.

A SuperTracker channel holds sixteen inserts (slots 0–15), so Maschine Groups with longer chains than the former eight-insert limit now import completely; for example Faded Reels “Beautiful Kit” keeps all eleven loadable Group inserts (its twelfth module, Performance FX, has no SuperTracker equivalent and is reported as unavailable). Loadable inserts are numbered contiguously per Sound or Group. Only a chain longer than sixteen is shortened: bypassed inserts are left out first, then the trailing ones; each omitted insert is listed in the import notes and keeps its original state in the report (`omitted-insert-limit`).

Effects-only Sounds retain their insert chains, including an effect in the first slot. Their original audio input routing is reported as unconverted. DrumSynth controls use the same named parameter dictionary as the normal instrument editor and native engine, so the converted values remain editable and are applied during playback.

The original archive, typed module values, sampler controls and original mixer values are retained in `kit.maschineImport` and source fields for future migrations. Unknown state is not discarded. The import report lists remaining differences, including unconverted patterns/modulation, auxiliary routing and unsupported control modes. Kit import does not currently promise a complete Maschine project conversion.

## Implementation and provenance

`MaschineGroupArchive` validates the native envelope and portable archive header, dynamic class registration, sixteen Sound records, tracked parameter identities and Group/Sound boundaries. `MaschineGroupSamples` and `MaschineGroupParameters` decode versioned member layouts. Pure mapping helpers turn original raw values into the engine's own physical controls and normalized AU parameter state. Built-in FX descriptors contain the same binary-plist state used by normal project saving and restoration.

The format was investigated by reading installed Group files and statically examining the locally installed unstripped Maschine serializer/control metadata. No vendor executable was invoked for this research. No NI DSP code, implementation or binary assets are included in the program or tests. Original test fixtures are generated in Swift. Installed commercial content is only read when locally present and is never copied into this repository.
