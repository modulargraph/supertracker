#pragma once
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Runs body(context) and reports an Objective-C exception it raises instead of letting it
/// unwind into the caller (AVAudioEngine raises for graph edits it refuses). Returns 0 when
/// the body completed, 1 when it raised; the exception reason is copied into message.
int st_objc_try(void (*body)(void *context), void *context, char *message, size_t capacity);

/// Clean-room VST 2.4 host. The plug-in ABI is declared privately in VST2Host.mm;
/// no Steinberg VST2 SDK headers are used or distributed.
typedef struct STVST2Instance STVST2Instance;

typedef struct {
    int32_t unique_id;
    /// Nonzero only for a sub-plug-in of a shell (for example WaveShell).
    int32_t shell_id;
    char name[128];
    char vendor[128];
    char product[128];
    int32_t vendor_version;
    int32_t category;
    uint8_t is_instrument, has_editor, program_chunks, is_shell;
    int32_t inputs, outputs, parameters, programs;
} STVST2Info;
typedef void (*STVST2InfoCallback)(const STVST2Info *, void *context);

typedef struct {
    int32_t sample_offset;
    uint8_t status, data1, data2;
} STVST2MIDI;

typedef struct {
    int32_t sample_offset;
    int32_t index;
    float value;
} STVST2ParameterChange;

typedef struct {
    double sample_position;
    double sample_rate;
    double beat_position;
    double bar_start_beat;
    double tempo;
    double cycle_start_beat, cycle_end_beat;
    int32_t numerator, denominator;
    uint8_t playing, recording, cycling, musical_valid;
} STVST2Transport;

/// Main thread. Opens the bundle, instantiates every class it exposes (each shell
/// child is reported separately) and closes them again. Intended for an isolated
/// scanner process: a broken plug-in may crash the caller.
int st_vst2_scan_bundle(const char *bundle_path, STVST2InfoCallback, void *context, char *error, size_t error_size);
/// Nonzero when the bundle executable contains code for the running architecture.
int st_vst2_bundle_is_loadable(const char *bundle_path, char *error, size_t error_size);

/// Main thread. shell_id is zero for an ordinary plug-in.
STVST2Instance *st_vst2_create(const char *bundle_path, int32_t shell_id, double sample_rate, uint32_t maximum_frames, char *error, size_t error_size);
void st_vst2_destroy(STVST2Instance *);
int st_vst2_info(STVST2Instance *, STVST2Info *);

/// Main thread, rendering stopped. Suspends, applies format and resumes processing.
int st_vst2_prepare(STVST2Instance *, double sample_rate, uint32_t maximum_frames, char *error, size_t error_size);
void st_vst2_suspend(STVST2Instance *);
/// Control thread, render quiesced: releases held notes and restarts processing (AU reset).
int st_vst2_reset(STVST2Instance *);
int st_vst2_is_active(STVST2Instance *);
uint32_t st_vst2_latency(STVST2Instance *);
uint32_t st_vst2_tail(STVST2Instance *);

int st_vst2_parameter_count(STVST2Instance *);
int st_vst2_parameter_name(STVST2Instance *, int32_t index, char *text, size_t capacity);
int st_vst2_parameter_label(STVST2Instance *, int32_t index, char *text, size_t capacity);
/// Display text for the current value. Plug-ins only format their live value.
int st_vst2_parameter_display(STVST2Instance *, int32_t index, char *text, size_t capacity);
/// Cached atomic value; safe on any thread.
float st_vst2_parameter_value(STVST2Instance *, int32_t index);
/// Control thread edit. Forwarded to the plug-in and recorded in the value cache.
int st_vst2_set_parameter(STVST2Instance *, int32_t index, float value);
/// Main-thread tick: refreshes cached values for edits announced by the plug-in UI.
/// Returns the number of indices whose value changed; writes up to capacity indices.
int st_vst2_take_changed_parameters(STVST2Instance *, int32_t *indices, int32_t capacity);
/// Re-reads every parameter from the plug-in (after state restore or program change).
void st_vst2_refresh_parameters(STVST2Instance *);
/// Plug-in requested an I/O or parameter-layout reload (audioMasterIOChanged/UpdateDisplay).
uint32_t st_vst2_take_host_flags(STVST2Instance *);

int st_vst2_program_count(STVST2Instance *);
int st_vst2_program(STVST2Instance *);
int st_vst2_set_program(STVST2Instance *, int32_t index);

/// Realtime: no allocation, no locks. Inputs may be null for instruments.
/// MIDI and parameter offsets must lie in [0, frames).
int st_vst2_process(STVST2Instance *, const float *input_left, const float *input_right,
                    float *output_left, float *output_right, uint32_t frames,
                    const STVST2MIDI *midi, uint32_t midi_count,
                    const STVST2ParameterChange *parameters, uint32_t parameter_count,
                    const STVST2Transport *transport);

/// Self-describing host state: chunk or parameter snapshot plus program. Free with st_vst2_free.
int st_vst2_copy_state(STVST2Instance *, void **bytes, size_t *size, char *error, size_t error_size);
int st_vst2_restore_state(STVST2Instance *, const void *bytes, size_t size, char *error, size_t error_size);
/// Raw plug-in chunk as stored by NKS (PCHK) and other VST2 hosts. preset selects
/// the program chunk instead of the bank chunk.
int st_vst2_restore_chunk(STVST2Instance *, const void *bytes, size_t size, int preset, char *error, size_t error_size);
void st_vst2_free(void *);

/// parent_view is an NSView. Returns logical editor size.
int st_vst2_attach_editor(STVST2Instance *, void *parent_view, int *width, int *height, char *error, size_t error_size);
void st_vst2_detach_editor(STVST2Instance *);
/// Main thread: editor idle and resize polling. Returns 1 when the editor asked for a new size.
int st_vst2_editor_idle(STVST2Instance *, int *width, int *height);

#ifdef __cplusplus
}
#endif
