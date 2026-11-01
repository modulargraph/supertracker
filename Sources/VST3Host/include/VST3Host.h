#pragma once
#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct STVST3Instance STVST3Instance;
typedef struct {
    char uid[33];
    char name[128];
    char vendor[128];
    char category[128];
} STVST3Class;
typedef void (*STVST3ClassCallback)(const STVST3Class *, void *context);
typedef struct {
    uint32_t id;
    char title[256];
    char short_title[128];
    char units[128];
    int32_t steps;
    int32_t flags;
    double default_value;
    double value;
} STVST3Parameter;
/// flags: ST_VST3_MIDI_SEQUENCED marks timeline (sequencer) notes; zero is live input,
/// which VST3 note events report with Event::kIsLive.
enum { ST_VST3_MIDI_SEQUENCED = 1u << 0 };
typedef struct {
    int32_t sample_offset;
    uint8_t status, data1, data2;
    int32_t note_id;
    uint8_t flags;
} STVST3MIDI;
typedef struct { int32_t sample_offset; uint32_t id; double value; } STVST3ParameterChange;
enum {
    ST_VST3_TRANSPORT_MUSIC_VALID = 1u << 0,
    ST_VST3_TRANSPORT_TEMPO_VALID = 1u << 1,
    ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID = 1u << 2,
    ST_VST3_TRANSPORT_TRANSPORT_VALID = 1u << 3,
    ST_VST3_TRANSPORT_CONTINUOUS_VALID = 1u << 4,
    ST_VST3_TRANSPORT_BAR_VALID = 1u << 5,
    ST_VST3_TRANSPORT_CYCLE_VALID = 1u << 6,
    ST_VST3_TRANSPORT_EXPLICIT = 1u << 31
};
typedef struct {
    /* The original six fields retain their source-compatible aggregate order.
       sample_position is the musical project timeline at this processor, not
       the graph's monotonically advancing render clock. Negative preroll is valid. */
    int64_t sample_position;
    double beat_position;
    double tempo;
    int32_t numerator, denominator;
    uint8_t playing;
    /* Modern callers set EXPLICIT, then only flags for information actually
       supplied. Zero-initialized legacy aggregates imply the original finite
       tempo/music/meter/playing fields, never continuous/bar/cycle information.
       Invalid optional values are ignored even if their validity bit is set. */
    uint32_t valid_fields;
    int64_t continuous_sample_position;
    double bar_start_beat, cycle_start_beat, cycle_end_beat;
    uint8_t cycling, recording;
} STVST3Transport;
/// Optional passive diagnostics for one parameter. Counters distinguish host UI,
/// scheduled automation, MIDI mapping, final process input and actual getPoint
/// reads by the component. A read proves delivery, not semantic application.
typedef struct {
    uint32_t id, pending_ui_edits;
    uint64_t ui_queued, ui_dequeued, automation_received, midi_received;
    uint64_t offered_points, read_points, processes, processing_starts, processing_stops;
    uint64_t resets, state_copies, state_restores, offered_process, read_process;
    double last_ui_value, last_dequeued_value, last_automation_value, last_midi_value;
    double last_offered_value, last_read_value;
    int32_t last_offered_offset, last_read_offset, last_offered_frames, last_read_frames, last_process_result;
} STVST3ParameterDiagnostics;
typedef struct {
    int32_t index;
    char name[256];
    int32_t channels, bus_type;
    uint8_t is_auxiliary, is_active;
} STVST3InputBus;
typedef struct {
    int32_t bus_index, channel_count;
    const float *const *channels;
} STVST3InputBuffer;
typedef struct {
    uint64_t edit_generation;
    uint32_t pending_edits, required_frames, settle_milliseconds;
    uint8_t dirty;
} STVST3StateCaptureStatus;

/// All discovery, lifecycle, state, controller and editor calls must run on the main
/// thread with rendering stopped for creation/destruction/state restoration.
/// Parameter edits use a bounded SPSC queue; process is its only consumer.
int st_vst3_scan_bundle(const char *bundle_path, STVST3ClassCallback, void *context, char *error, size_t error_size);
STVST3Instance *st_vst3_create(const char *bundle_path, const char *class_uid, double sample_rate, uint32_t maximum_frames, char *error, size_t error_size);
void st_vst3_destroy(STVST3Instance *);
/// Main-thread shutdown hook, after all instances are destroyed and before AppKit
/// teardown. Releases unused module factories and their vendor worker services.
void st_vst3_release_unused_modules(void);
int st_vst3_prepare(STVST3Instance *, double sample_rate, uint32_t maximum_frames, char *error, size_t error_size);
int st_vst3_reset(STVST3Instance *, char *error, size_t error_size);
/// Control-thread initialization only, with the node unattached or rendering
/// stopped. Processes bounded silent input into discarded output, with no MIDI.
/// It never waits or runs a nested event loop. Host UI may await between calls for
/// plugins whose startup/preset workers complete asynchronously.
int st_vst3_prime(STVST3Instance *, uint32_t frames, char *error, size_t error_size);
int st_vst3_set_processing(STVST3Instance *, int processing, char *error, size_t error_size);
/// Control-thread only; rendering must be quiesced for configuration transitions.
int st_vst3_is_processing(STVST3Instance *);
uint32_t st_vst3_latency(STVST3Instance *);
uint32_t st_vst3_tail(STVST3Instance *);
int st_vst3_is_instrument(STVST3Instance *);
int st_vst3_input_bus_count(STVST3Instance *);
int st_vst3_input_bus(STVST3Instance *, int32_t index, STVST3InputBus *);
/// Actual first kMain input index, or -1. An auxiliary input is never a fallback.
int32_t st_vst3_main_input_bus(STVST3Instance *);
/// Control-thread only, rendering quiesced and processing=false. Channels checks
/// the current mono/stereo layout; activation never silently changes bus layout.
int st_vst3_configure_input_bus(STVST3Instance *, int32_t index, int32_t channels, int active, char *error, size_t error_size);
int st_vst3_parameter_count(STVST3Instance *);
int st_vst3_parameter(STVST3Instance *, int index, STVST3Parameter *result);
/* Optional baseline-safe hardware editor protocol. Zero means unsupported. */
uint32_t st_vst3_baseline_capabilities(STVST3Instance *);
int st_vst3_capture_baseline(STVST3Instance *, char *error, size_t capacity);
int st_vst3_set_parameter(STVST3Instance *, uint32_t id, double normalized);
/// Atomic value lookup, safe on render thread. Unknown IDs return zero.
double st_vst3_parameter_value(STVST3Instance *, uint32_t id);
/// Changes whenever any cached parameter value may have changed (processor output,
/// controller edits, controller refresh). Equal values mean nothing to republish.
uint64_t st_vst3_parameter_generation(STVST3Instance *);
/// Main-thread tick: copies parameter changes from processing into edit controller.
void st_vst3_sync_controller(STVST3Instance *);
int st_vst3_parameter_text(STVST3Instance *, uint32_t id, double normalized, char *text, size_t capacity);
/// Pull controller changes requested by native plugin UI; restart flags are consumed.
/// Bits are Steinberg::Vst::RestartFlags values.
enum { ST_VST3_RESTART_LATENCY_CHANGED = 1u << 3 };
uint32_t st_vst3_take_restart_flags(STVST3Instance *);
/// kLatencyChanged response. Control thread only, rendering quiesced: stops
/// processing, deactivates and reactivates the component (the VST3 contract for
/// reporting a new latency), then resumes processing if it was running. Held notes
/// end with the deactivation. Query st_vst3_latency afterwards.
int st_vst3_restart_component(STVST3Instance *, char *error, size_t error_size);
uint64_t st_vst3_dropped_events(STVST3Instance *);
/// Disabled by default. Enable/reset with rendering quiesced; a nonexistent ID
/// disables collection. Snapshot uses atomic reads and never processes audio.
int st_vst3_watch_parameter(STVST3Instance *, uint32_t id);
int st_vst3_parameter_diagnostics(STVST3Instance *, STVST3ParameterDiagnostics *);
/// Atomic queue count for pre-capture draining by normal graph callbacks.
uint32_t st_vst3_pending_parameter_edits(STVST3Instance *);
/// Nonzero only for an exact factory class and loaded bundle-version profile.
/// This metadata alone grants no processing or capture permission. Use the
/// explicit begin/settle/authorize protocol below for dirty profiled components.
uint32_t st_vst3_required_state_flush_frames(STVST3Instance *);
/// Control-thread metadata from the actual loaded module/factory, never project
/// descriptors. Returned UTF-8 strings remain valid until the instance is freed.
const char *st_vst3_factory_version(STVST3Instance *);
const char *st_vst3_bundle_version(STVST3Instance *);
/// Control-thread status. Dirty survives audio delivery until a successful capture.
int st_vst3_state_capture_status(STVST3Instance *, STVST3StateCaptureStatus *);
/// Begin a uniquely identified capture attempt, with rendering quiesced. Pending
/// profiled edits may process one bounded block only when the caller explicitly
/// confirms the graph was already stopped; held notes refuse that advancement.
/// No audio is processed when pending_edits is zero. A dirty profile returns a
/// nonzero token. The caller must then suspend on the main event loop for at least
/// settle_milliseconds, and recheck graph/instance identity before authorization.
int st_vst3_begin_state_capture(STVST3Instance *, int allow_stopped_audio_advance, uint64_t *preparation_token, char *error, size_t error_size);
/// Quiesced control-thread authorization. Verifies token, unchanged edit
/// generation, empty queue, and monotonic deadline. The deadline alone is not
/// proof of main-loop progress: the caller must also perform the suspension above.
int st_vst3_authorize_state_capture(STVST3Instance *, uint64_t preparation_token, char *error, size_t error_size);
/// Cancels only the named attempt. Never clears dirty edits or another attempt.
void st_vst3_cancel_state_capture(STVST3Instance *, uint64_t preparation_token);
/// Final quiesced validation only; never advances audio. Profiled dirty edits
/// require an authorized one-shot token. The legacy flag grants no implicit flush.
int st_vst3_prepare_state_capture(STVST3Instance *, int allow_stopped_audio_advance, char *error, size_t error_size);
/// Realtime safe host code: no allocations or locks. Float32, stereo main bus,
/// additional plugin inputs are silence and additional outputs are discarded.
/// MIDI and parameter offsets must lie in [0, frames); input pointers may be null.
int st_vst3_process(STVST3Instance *, const float *input_left, const float *input_right, float *output_left, float *output_right, uint32_t frames, const STVST3MIDI *midi, uint32_t midi_count, const STVST3ParameterChange *parameters, uint32_t parameter_count, const STVST3Transport *transport);
/// Indexed mono/stereo inputs, including the actual main index. Omitted, null or
/// inactive buses are silent. Duplicate/out-of-range/layout-mismatched descriptors
/// fail with silent output. Inputs and channel-pointer arrays live through the call.
int st_vst3_process_indexed(STVST3Instance *, const STVST3InputBuffer *inputs, uint32_t input_count, float *output_left, float *output_right, uint32_t frames, const STVST3MIDI *midi, uint32_t midi_count, const STVST3ParameterChange *parameters, uint32_t parameter_count, const STVST3Transport *transport);
/// Host state wraps component + controller state with class identity. Caller frees
/// returned bytes using st_vst3_free. Maximum accepted state is 256 MiB.
/// Empty controller kNotImplemented state is optional. Any other failed controller
/// save, including nonempty kNotImplemented output, fails with null bytes/zero size.
int st_vst3_copy_state(STVST3Instance *, void **bytes, size_t *size, char *error, size_t error_size);
/// The same capture while the render thread may be inside process(). It never runs a
/// host flush block: with UI edits still queued for the processor it returns -1 and
/// copies nothing (retry after the next audio block). Component/controller getState
/// run on the calling (main) thread as the VST3 threading model specifies.
int st_vst3_copy_state_concurrent(STVST3Instance *, void **bytes, size_t *size, char *error, size_t error_size);
int st_vst3_restore_state(STVST3Instance *, const void *bytes, size_t size, char *error, size_t error_size);
/// Opaque NKS/VST3 component payload, passed only to matching plugin's setState.
/// Success means the plugin accepted it; it does not assert universal NKS compatibility.
int st_vst3_restore_component_state(STVST3Instance *, const void *bytes, size_t size, char *error, size_t error_size);
/// Explicit VST2 bank migration, limited to class UIDs encoding the exact legacy
/// ID as 565354 + eight hexadecimal digits. Caller must quiesce rendering. Uses
/// Steinberg's FXB wrapper, then retries unchanged raw bytes only when setState
/// rejects that wrapper. Controller/layout/lifecycle failures do not retry.
/// Payload must be nonempty and <=32 MiB, program >=0, unique_id !=0. Optional
/// used_raw_fallback is zeroed on failure and set to 1 only after raw success.
int st_vst3_restore_vst2_bank(STVST3Instance *, uint32_t unique_id, int32_t program,
    const void *bytes, size_t size, int *used_raw_fallback, char *error, size_t error_size);
void st_vst3_free(void *);
/// Creates the Cocoa editor view and returns its logical size without attaching it.
/// Attach once the parent view is in a window: vendors (iZotope Ozone 12) read the
/// parent's window during IPlugView::attached and crash on a windowless parent.
int st_vst3_create_editor(STVST3Instance *, int *width, int *height, char *error, size_t error_size);
/// parent_view is a Cocoa NSView; logical dimensions are returned on attachment.
int st_vst3_attach_editor(STVST3Instance *, void *parent_view, int *width, int *height, char *error, size_t error_size);
void st_vst3_detach_editor(STVST3Instance *);

#ifdef __cplusplus
}
#endif
