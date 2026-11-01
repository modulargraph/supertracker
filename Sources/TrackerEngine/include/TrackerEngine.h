#ifndef SUPERTRACKER_ENGINE_H
#define SUPERTRACKER_ENGINE_H
#include <stdint.h>
#include "TrackerClock.h"
#include "TrackerUnitRange.h"
#include "STBuiltinEffects.h"
#ifdef __cplusplus
extern "C" {
#endif

typedef struct STEngine STEngine;
/* Tracks are not a compile-time quantity: every per-track table is sized from the
   published project. ST_TRACK_SANITY_LIMIT only rejects absurd input (a validation
   error, far above realistic use), it is not an architectural size.
   Channel/bus numbering (helpers below; use them, never literal numbers):
     channels 0..trackCount-1         tracks (channel == track index)
     buses    0..returnBus-1          tracks (bus == channel; channels from trackCount on are silent)
     bus      returnBus               stereo returns + metronome
   returnBus = max(ST_MIN_RETURN_BUS, trackCount), so projects with <=16 tracks keep
   the historical return bus 16. Track groups (kits) are ordinary tracks routed by the
   host to a group bus; the engine has no separate kit channels. */
enum { ST_MIN_RETURN_BUS=16, ST_TRACK_SANITY_LIMIT=1024 };
/* Instruments, like tracks, are sized per project (st_engine_set_instrument_count):
   instruments 0..count-1. */
enum { ST_INSTRUMENT_SANITY_LIMIT=1024 };
enum { ST_MAX_NOTE_COLUMNS=8 }; /* note columns per track: column 0 (st_engine_set_step) plus up to seven more */
enum { ST_MAX_PATTERNS=256, ST_MAX_ROWS=256, ST_MAX_CHORD=8, ST_MAX_HOST_DESTINATIONS=2048, ST_MAX_INSERTS=32 /* channel slots: 16 inserts + 16 instrument-chain slots (MixerChannel.channelSlotCount) */ };
/* Instrument sources. Value 0 was the retired built-in synth; it now reads as a sampler. */
enum { ST_INSTRUMENT_SAMPLE=1, ST_INSTRUMENT_AU=2, ST_INSTRUMENT_MIDI=3 };
/* Wavetable mode reads the sample region as consecutive single-cycle frames of
   wavetableFrameSize samples (0 = the size registered with st_engine_set_sample_cycle,
   else 2048, or the whole region when shorter). wavetablePosition 0..1 picks the frame,
   crossfading between neighbours; LFO "position", automation and step FX move it. */
enum { ST_WAVETABLE_DEFAULT_FRAME=2048, ST_WAVETABLE_MIN_FRAME=16, ST_WAVETABLE_MAX_FRAME=8192 };
enum { ST_MODE_ONESHOT=0, ST_MODE_FORWARD=1, ST_MODE_REVERSE=2, ST_MODE_PINGPONG=3, ST_MODE_SLICE=4, ST_MODE_WAVETABLE=5, ST_MODE_GRANULAR=6 };
enum { ST_FILTER_LOWPASS=0, ST_FILTER_HIGHPASS=1, ST_FILTER_BANDPASS=2 };
/* LFO destinations. ST_MOD_GRAIN_SIZE scales Granular grain size by up to two octaves either way. */
enum { ST_MOD_CUTOFF=0, ST_MOD_PITCH=1, ST_MOD_VOLUME=2, ST_MOD_PAN=3, ST_MOD_POSITION=4, ST_MOD_GRAIN_SIZE=5 };
enum { ST_LFO_SINE=0, ST_LFO_TRIANGLE=1, ST_LFO_SAW=2, ST_LFO_SQUARE=3, ST_LFO_RANDOM=4 };
enum { ST_TARGET_VOLUME=0, ST_TARGET_PAN=1, ST_TARGET_CUTOFF=2, ST_TARGET_RESONANCE=3, ST_TARGET_DELAY=4, ST_TARGET_REVERB=5, ST_TARGET_PITCH=6, ST_TARGET_PLUGIN=7, ST_TARGET_ATTACK=8, ST_TARGET_DECAY=9, ST_TARGET_SUSTAIN=10, ST_TARGET_RELEASE=11, ST_TARGET_DRIVE=12, ST_TARGET_BIT_DEPTH=13, ST_TARGET_LFO_RATE=14, ST_TARGET_LFO_DEPTH=15, ST_TARGET_START=16, ST_TARGET_END=17, ST_TARGET_LOOP_START=18, ST_TARGET_LOOP_END=19, ST_TARGET_GRAIN_SIZE=20, ST_TARGET_GRAIN_POSITION=21, ST_TARGET_SLICE_COUNT=22, ST_TARGET_INSTRUMENT_VOLUME=23, ST_TARGET_INSTRUMENT_PAN=24, ST_TARGET_TUNE=25, ST_TARGET_FILTER_ATTACK=26, ST_TARGET_FILTER_DECAY=27, ST_TARGET_FILTER_SUSTAIN=28, ST_TARGET_FILTER_RELEASE=29, ST_TARGET_FILTER_ENV_AMOUNT=30, ST_TARGET_LFO2_RATE=31, ST_TARGET_LFO2_DEPTH=32, ST_TARGET_LFO_BEATS=33, ST_TARGET_LFO2_BEATS=34, ST_TARGET_WAVETABLE_POSITION=35,
    ST_TARGET_GRAIN_DENSITY=36, ST_TARGET_GRAIN_SPRAY=37, ST_TARGET_GRAIN_PITCH_SPREAD=38, ST_TARGET_GRAIN_SCAN=39, ST_TARGET_GRAIN_REVERSE=40, ST_TARGET_GRAIN_STEREO=41,
    ST_TARGET_LAST_INSTRUMENT=ST_TARGET_GRAIN_STEREO };
enum { ST_FX_NONE=0, ST_FX_VOLUME=1, ST_FX_PAN=2, ST_FX_PITCH=3, ST_FX_CUTOFF=4, ST_FX_RESONANCE=5, ST_FX_DELAY=6, ST_FX_REVERB=7, ST_FX_PROBABILITY=8, ST_FX_RATCHET=9, ST_FX_REVERSE=10, ST_FX_OFFSET=11, ST_FX_GLIDE=12, ST_FX_MICROTIMING=13, ST_FX_ARPEGGIO=14, ST_FX_SLICE=15, ST_FX_POSITION=16 };
enum { ST_EVENT_NOTE_ON=1, ST_EVENT_NOTE_OFF=2, ST_EVENT_PARAMETER=3, ST_EVENT_ALL_NOTES_OFF=4, ST_EVENT_HOST_PARAMETER=5 };

static inline int32_t st_instrument_capacity(int32_t instrumentCount) { return instrumentCount; }
static inline int32_t st_return_bus(int32_t trackCount) { return trackCount>ST_MIN_RETURN_BUS?trackCount:ST_MIN_RETURN_BUS; }
static inline int32_t st_channel_count(int32_t trackCount) { return st_return_bus(trackCount); }
static inline int32_t st_output_bus_count(int32_t trackCount) { return st_return_bus(trackCount)+1; }
static inline int32_t st_channel_output_bus(int32_t trackCount,int32_t channel) { (void)trackCount; return channel; }

typedef struct { float volume,pan,delaySend,reverbSend; int32_t mute,solo,instrument; } STTrack;
typedef struct {
    int32_t type,mode,rootNote,filterType,sliceCount,bitDepth;
    float volume,pan,tune,start,end,loopStart,loopEnd;
    float attack,decay,sustain,release,cutoff,resonance,drive;
    float lfoRate,lfoDepth,grainSize,grainPosition;
    int32_t voiceMode,chokeGroup; /* 0 poly, 1 mono, 2 legato; chokeGroup 0 disables. */
    float filterAttack,filterDecay,filterSustain,filterRelease,filterEnvAmount;
    int32_t lfoTarget,lfoShape,lfo2Target,lfo2Shape;
    float lfo2Rate,lfo2Depth; /* Hz and normalized0..1; filterEnvAmount is semitones. */
    int32_t lfoSync,lfo2Sync; /* Opt-in tempo sync; existing rate fields keep Hz units. */
    float lfoBeats,lfo2Beats; /* Quarter-note beats per cycle,1/64..64; defaults1. */
    float wavetablePosition; /* 0..1, see ST_WAVETABLE_DEFAULT_FRAME. */
    int32_t wavetableFrameSize; /* ST_WAVETABLE_MIN_FRAME..ST_WAVETABLE_MAX_FRAME; anything else is 0, automatic. */
    /* Granular mode. Grains of grainSize seconds (Hann window) read the region around grainPosition (0..1).
       grainDensity 1..ST_GRANULAR_MAX_GRAINS grains overlap (0 reads as the historical 2); grainSpray 0..1 s random
       position offset per grain; grainPitchSpread 0..24 random +/- semitones per grain; grainScan -4..4 moves the
       position through the region (1 = original speed, 0 = frozen, negative = backwards); grainReverse 0..1 chance a
       grain plays backwards; grainStereo 0..1 random grain pan. All zero (and density 2) is the original sound. */
    int32_t grainDensity;
    float grainSpray,grainPitchSpread,grainScan,grainReverse,grainStereo;
} STInstrument;
enum { ST_GRANULAR_MAX_GRAINS=8, ST_GRANULAR_DEFAULT_DENSITY=2 };
typedef struct { int32_t type; float value; } STStepFX;
typedef struct {
    int32_t note,instrument,velocity; float gate;
    STStepFX fx1,fx2;
} STStep;
/* hostMap: on ST_EVENT_HOST_PARAMETER, the host destination map of the project
   that emitted it (st_engine_set_host_automation_map); 0 on every other event. */
typedef struct { int32_t type,track,instrument,note,velocity,frameOffset,target,slot; uint64_t address; double value; uint64_t hostMap; } STEvent;
/* NOTE_ON/NOTE_OFF: slot is 1 when the voice was started by the sequencer (pattern/song
   playback, including its ratchets and delays) and 0 for live input and auditions. */
/* Optional recording-edge protocol. Clock provenance is the frame at which an
   input command is applied, NOT physical MIDI packet or UI receipt time. */
enum { ST_INPUT_NOTE_ON=1, ST_INPUT_NOTE_OFF=2, ST_INPUT_ACK_CAPACITY=4096 };
enum { ST_INPUT_ACK_APPLIED=1, ST_INPUT_ACK_DISCONTINUITY=2, ST_INPUT_ACK_CONTROL_FLUSH=3 };
enum { ST_INPUT_CLOCK_PLAYING=1, ST_INPUT_CLOCK_MAPPED=2, ST_INPUT_CLOCK_RECORDING=4 };
typedef struct {
    uint64_t requestID,runID,timingRevision;
    int64_t renderFrame,graphFrame; /* graphFrame=INT64_MIN for direct/non-bus render. */
    double distanceBeats;
    int32_t edge,track,instrument,note,velocity,applyNote;
    int32_t status,flags,pattern,row,sequence;
    /* A note-on in the second half of a row is placed on the next row (row reports it; early=1). That row's
       trigger of the same note on the same track is skipped once, since the live note already sounds. */
    int32_t early;
} STInputAck;
typedef enum { ST_FREEZE_DISABLED=0,ST_FREEZE_SONG=1,ST_FREEZE_NATURAL_TAIL=2,
    ST_FREEZE_STOPPED=3,ST_FREEZE_UNSUPPORTED=4,ST_FREEZE_INVALID_POSITION=5 } STFreezePhase;
typedef enum { ST_FREEZE_CAUSE_NONE=0,ST_FREEZE_CAUSE_PLAY=1,ST_FREEZE_CAUSE_SEEK=2,
    ST_FREEZE_CAUSE_STOP=3,ST_FREEZE_CAUSE_LOOP=4,ST_FREEZE_CAUSE_NATURAL_END=5,
    ST_FREEZE_CAUSE_REPLACEMENT=6,ST_FREEZE_CAUSE_RESTORE=7,ST_FREEZE_CAUSE_PANIC=8,
    ST_FREEZE_CAUSE_GRAPH_RESET=9 } STFreezeCause;
enum { ST_FREEZE_PREROLL=1,ST_FREEZE_NOT_CONFIGURED=-4 };
typedef struct {
    uint64_t publicationID,playbackID,transitionID;
    int64_t graphFrame,contentGraphFrame,projectFrame;
    int64_t transitionContentGraphFrame,transitionProjectFrame;
    double sampleRate;
    STFreezePhase phase;
    STFreezeCause cause;
    uint32_t flags;
} STFreezeContext;
typedef struct { STFreezeContext context;uint32_t frames; } STFreezeSpan;
typedef struct { int32_t playing,pattern,row,sequence,trackCount; float peakL,peakR; double position,seconds; uint64_t droppedCommands,droppedEvents; int32_t songMode;
    double beat,bpm,barStartBeat; int64_t bar; int32_t numerator,denominator; uint64_t timingRevision;
    int32_t recording;
    int32_t queuedPattern,queuedSequence; /* Pending st_engine_queue_jump target, -1 none. */
    int32_t countIn; /* Count-in clicks still to come (including the current one); 0 when not counting in. */
    /* 1 while a queued Play/Stop/count-in awaits the render thread (which may be never, if the device stopped);
       the rendered playing/countIn/queued fields are then stale, and requestedPlaying is what was asked for. */
    int32_t transportPending,requestedPlaying;
    uint64_t transportSeekRevision; /* Changes only when a seek is admitted; published coherently with position. */
} STStatus;
/* Transport state for the UI/control thread: a queued Stop reads as stopped and a queued Play as playing at once,
   instead of the stale rendered state (which turned the next Play press into another Stop). The render thread
   keeps using STStatus.playing, the transport it has actually rendered. */
static inline int32_t st_status_transport_playing(const STStatus *status) { return status->transportPending ? status->requestedPlaying : status->playing; }
static inline int32_t st_status_count_in(const STStatus *status) { return status->transportPending ? 0 : status->countIn; }
/* Ephemeral checkpoint for the same engine/project, not a saved project format.
   Frame fields preserve fractional row timing without seconds/row conversion. */
typedef struct {
    int32_t playing,songMode,pattern,row,sequence,rowHasStarted,sequenceFinished;
    double rowRemaining,rowDuration,transportFrames;
    /* Version0 is legacy; version1 carries mapped musical time. Ephemeral ABI:
       rebuild native clients, and never serialize this structure as project data. */
    int32_t timingVersion;
    uint64_t timingRevision;
    double beat,patternOriginBeat,loopBeatOffset;
    int64_t loopIteration;
    int32_t recording; /* Transient host arm/capture intent, including stopped. */
    /* Frozen Pattern print replay is transport state, not oscillator/tail DSP.
       Preserve the exact frame and head/steady-loop choice across temporary
       export. Zero/invalid fields reconstruct the current row's head print.
       Prior-pattern DSP/printed tails are cleared on restore, like other tails. */
    int64_t patternClipFrame;
    int32_t patternClipPattern,patternClipLooped,patternClipValid;
} STTransportState;

/* Ordinary control functions are single-producer; use one serial control queue.
   The explicitly documented atomic automation-refresh mailbox is the exception. Never call
   setters from the audio callback. Render/pop_event are serialized audio-thread operations; status and peak getters
   are atomic snapshots and may be read on any thread. */
STEngine *st_engine_create(double sampleRate);
void st_engine_destroy(STEngine *engine);
void st_engine_begin_update(STEngine *engine);
void st_engine_clear_project(STEngine *engine);
void st_engine_set_timing(STEngine *engine,double bpm,int32_t linesPerBeat,float swing,float masterVolume);
/* Default-off, full-song frozen replay policy in the immutable project draft.
   Enabling requires publicationID!=0 and preallocates bounded history on the
   control thread; errors leave the policy unchanged. Host owns session fencing
   separately. Publish graph/policy changes while readers are quiescent.
   Active Song seeks/restores ceil the native origin once, preserving the row's
   fractional remainder. Pattern mode is not substituted by this policy.
   Rollback must republish the original policy before restoring its checkpoint. */
int32_t st_engine_set_frozen_song_policy(STEngine *engine,int32_t enabled,uint64_t publicationID,
    char *error,uint32_t errorCapacity);
/* Optional exact full-song extent for a projected frozen publication. Requires
   the SAME enabled nonzero publicationID and musicalFrames in1..100000000.
   Repeating the same value is idempotent; a different value for an already set
   publication fails unchanged. New publication, disable and clear_project clear
   the extent. The old policy setter alone keeps its original behavior.
   Checked commit rejects an extent shorter than the projected live sequence or
   source-limited non-muted clips. Host must separately verify the canonical full
   song/printed stages agree, including duration-changing edits in either direction.
   Runtime end/loop/tail use these integer frames directly; duration() returns
   musicalFrames/sampleRate seconds (do not ceil a floating roundtrip to recover it).
   A replacement/restored cursor at or beyond this extent is clamped/stopped,
   retaining REPLACEMENT/RESTORE cause, never inventing natural completion.
   Rollback republishes original policy/extent BEFORE restoring its checkpoint. */
int32_t st_engine_set_frozen_song_extent(STEngine *engine,uint64_t publicationID,int64_t musicalFrames,
    char *error,uint32_t errorCapacity);
/* Optional stepped map. Copies/validates complete arrays on the control thread;
   failure leaves the draft map unchanged. Publish with checked commit after all
   other draft edits. No map retains legacy scheduling exactly. */
int32_t st_engine_set_timing_map(STEngine *engine,const STClockTempoEvent *tempos,uint32_t tempoCount,
    const STClockMeterEvent *meters,uint32_t meterCount,uint64_t revision,char *error,uint32_t errorCapacity);
void st_engine_clear_timing_map(STEngine *engine);
/* Pattern audition/export uses this song beat as its timing origin; default0.
   Song mode ignores it. Checked commit validates every pattern's end. */
int32_t st_engine_set_pattern_timing_origin(STEngine *engine,double beat);
void st_engine_set_metronome(STEngine *engine,int32_t enabled,int32_t numerator,int32_t denominator,float volume);
/* 1..ST_TRACK_SANITY_LIMIT; returns 0 and leaves the draft unchanged otherwise. Per-track
   tables (mixer state, ratchets, locks, meters, bus caches) are allocated on the control
   thread when the project is committed and swapped in with it; rendering never allocates. */
int32_t st_engine_set_track_count(STEngine *engine,int32_t count);
void st_engine_set_track(STEngine *engine,int32_t index,STTrack track);
/* Live note/preview/ACK, pitch-bend/mod-wheel, native automation and meters accept
   logical channels 0..st_channel_count(trackCount)-1 (channels from trackCount on are
   silent). Event.track carries the logical channel. All setters edit the immutable
   draft; publish using normal project updates. */
/* Per-track voice settings. chokeGroup 1..127 stops the sounding voices of other tracks with the same
   choke group and chokeScope (SuperTracker uses the track's group: 1 + group index, 0 when ungrouped) when
   this track starts a note; 0 uses the instrument's choke group (which chokes any track). voiceMode -1 uses the instrument's
   mode, else 0 poly, 1 mono, 2 legato. Returns 0 for invalid input. */
int32_t st_engine_set_track_voice(STEngine *engine,int32_t track,int32_t chokeGroup,int32_t chokeScope,int32_t voiceMode);



/* Opt-in raw buses for an external mixer; disabled by default. In this mode,
   track buses preserve instrument DSP but omit track/mute/solo/master/send mixing.
   Computational bits enable native/clip output and AU note events per track;
   removing a bit ends its AU voices, while native envelopes continue silently.
   Hardware MIDI retains legacy mute/solo gating. The return bus is unscaled metronome only.
   Changing modes clears legacy send tails. Publish with normal project updates.
   Legacy form: bit t of computationalMask is track t for t<32; tracks >=32 are enabled. */
void st_engine_set_host_mixing(STEngine *engine,int32_t enabled,uint32_t computationalMask);
/* Same, with a bitset of any length: bit t of words[t/64] is track t; tracks beyond
   wordCount*64 take enabledBeyond. words may be NULL when wordCount is 0. */
void st_engine_set_host_mixing_tracks(STEngine *engine,int32_t enabled,const uint64_t *words,uint32_t wordCount,int32_t enabledBeyond);
/* 1..ST_INSTRUMENT_SANITY_LIMIT ordinary instruments (default 64); returns 0 and leaves
   the draft unchanged otherwise. The table holds st_instrument_capacity(count) entries;
   shrinking drops the instruments beyond it, growing adds default instruments. Samples
   and settings of the instruments that remain are kept. */
int32_t st_engine_set_instrument_count(STEngine *engine,int32_t count);
void st_engine_set_instrument(STEngine *engine,int32_t index,STInstrument instrument);
void st_engine_set_pattern(STEngine *engine,int32_t index,int32_t length);
/* Copy-on-write reuse: after st_engine_clear_project, share pattern `index` (steps,
   plug-in locks, native and host automation) with the last published
   project instead of re-sending it. Steps are stored per (pattern,track) column and
   shared until edited, so commit cost scales with what changed. The caller must
   only reuse a pattern whose source and resolution context are unchanged.
   Returns 0 when there is no published pattern at that index. */
int32_t st_engine_reuse_pattern(STEngine *engine,int32_t index);
/* Process-unique identity of the last published pattern content (steps, locks,
   automation). Commits that edit no pattern keep it; different engines never
   share a value. A caller caching what it sent may reuse patterns only while it holds. */
uint64_t st_engine_pattern_epoch(STEngine *engine);
void st_engine_set_step(STEngine *engine,int32_t pattern,int32_t track,int32_t row,STStep step,const int32_t *chord,int32_t chordCount);
/* Attach a resolved plug-in parameter lock to an existing step, column0/1.
   slot -1 means instrument,0..ST_MAX_INSERTS-1 an insert; hostDestination -1 uses legacy
   track/slot events,0..2047 uses host event5. Both addresses participate in lane
   precedence. Values are normalized finite doubles; UInt64 addresses are exact.
   Resolve persistent instance identity and same-track ownership on the control
   thread. Invalid data returns0, never clamps/retargets. Replacing/clearing a
   step clears its locks. FX2 wins duplicate addresses; OFF/CUT rows still apply.
   Locks suppress only matching parameter lanes for this row. Matching lanes
   resume next row, otherwise the plug-in retains its last value. No render
   allocation. Seek/Stop/project refresh clear active locks; the atomic automation
   refresh mailbox preserves and re-emits current valid row locks. */
int32_t st_engine_set_plugin_step_fx(STEngine *engine,int32_t pattern,int32_t track,int32_t row,int32_t column,int32_t slot,int32_t hostDestination,uint64_t address,double value);
/* Clears column 0 and every extra note column of the row. */
void st_engine_clear_step(STEngine *engine,int32_t pattern,int32_t track,int32_t row);
/* Extra note column 1..ST_MAX_NOTE_COLUMNS-1 of a track's row (column 0 forwards to st_engine_set_step without a
   chord). Only note (0..127, -1 OFF, -2 CUT; anything else clears the cell), instrument (-1: the track's),
   velocity and gate are used; FX come from column 0 of the row. Each note column is its own sequencer lane on the
   track's channel: a note releases only its own column's gateless notes, and OFF/CUT end only its own column's
   notes. The row's note FX (column 0's FX 1/FX 2: ratchet, probability with one roll per row, microtiming,
   offset, reverse, glide, arpeggio, slice) apply to every column's note; channel FX apply to the channel as
   usual. A sticky ratchet continuing through empty rows repeats column 0's note only. */
void st_engine_set_step_column(STEngine *engine,int32_t pattern,int32_t track,int32_t row,int32_t column,STStep step);
void st_engine_clear_automation(STEngine *engine,int32_t pattern);
void st_engine_add_automation_point(STEngine *engine,int32_t pattern,int32_t track,int32_t target,int32_t slot,uint64_t address,int32_t interpolation,double row,double value);
/* Host-owned lanes:256 lanes/pattern,4096 points/lane; key(destination,address).
   destination is0..2047, address opaque, value finite raw double. Emit type5 with
   target=destination,track=-1,address/value unchanged; no native DSP mutation.
   Step points are sample-exact; linear events use the existing32-frame interval.
   clear_automation clears native and host lanes. The host owns removed-lane defaults. */
void st_engine_add_host_automation_point(STEngine *engine,int32_t pattern,int32_t destination,uint64_t address,int32_t interpolation,double row,double value);
/* Identifies the destination numbering of this project's host lanes and host
   plug-in locks (0 = unspecified). Every host event carries it (STEvent.hostMap),
   so a host whose render-side destination table was replaced before or after this
   project is published can drop events numbered for another table instead of
   delivering them to whatever now has that index. Control thread, inside an
   update; clear_project resets it to 0. */
void st_engine_set_host_automation_map(STEngine *engine,uint64_t map);
void st_engine_set_sequence(STEngine *engine,const int32_t *patterns,int32_t count,int32_t loopSong,int32_t activePattern);
void st_engine_set_effects(STEngine *engine,float delaySeconds,float delayFeedback,float delayMix,float reverbSize,float reverbDamping,float reverbMix);
/* Copies planar float PCM. right may be NULL for mono. Safe to free input on return.
   NULL left with zero frames clears the sample. The copy is made on the control thread. Old samples remain alive while voices use them. */
int32_t st_engine_set_sample(STEngine *engine,int32_t instrument,const float *left,const float *right,uint64_t frames,double sampleRate);
void st_engine_set_slice_points(STEngine *engine,int32_t instrument,const float *positions,int32_t count);
/* Single-cycle frame size detected in the instrument's sample (wavetable file metadata),
   used by Wavetable mode when the instrument's wavetableFrameSize is 0. 0 clears it;
   loading a new sample also clears it. Like st_engine_set_sample, it publishes at once
   outside an update. Returns 0 for an invalid instrument or size. */
int32_t st_engine_set_sample_cycle(STEngine *engine,int32_t instrument,int32_t frames);
void st_engine_clear_zones(STEngine *engine,int32_t instrument);
int32_t st_engine_add_sample_zone(STEngine *engine,int32_t instrument,const float *left,const float *right,uint64_t frames,double sampleRate,int32_t keyLow,int32_t keyHigh,int32_t velocityLow,int32_t velocityHigh,int32_t rootNote,float tune,float volume);
/* Timeline audio assets are independent of instrument sample slots; max256 each.
   Beat positions are quarter-note beats; offset/fades are seconds. durationBeats<=0
   uses available source duration. Assets persist across clear_project. */
int32_t st_engine_set_audio_asset(STEngine *engine,int32_t asset,const float *left,const float *right,uint64_t frames,double sampleRate);
/* Optional per-zone source geometry, normalized against that zone's full PCM.
   Default zones inherit the instrument region/pan exactly as before. Explicit
   regions override the parent start/end/loop; region automation stays within the
   selected zone. Zone pan adds to instrument/automated pan, clamped to[-1,1].
   Loop bounds are clipped to the selected region at voice creation. Invalid
   finite/range/index input returns0 unchanged. Call after add_sample_zone, before
   publishing the draft. Existing voices retain their selected source geometry. */
int32_t st_engine_set_sample_zone_region(STEngine *engine,int32_t instrument,int32_t zoneIndex,
    float start,float end,float loopStart,float loopEnd,float pan);
/* Per-zone onset mode: -1 inherits the instrument (default), 0 one-shot,
   1 forward loop using the zone/instrument loop region. Existing voices retain
   their onset mode; an explicit step Slice FX still takes precedence. Control
   thread/update transaction only. Returns 0 for invalid zone/value. */
int32_t st_engine_set_sample_zone_loop_enabled(STEngine *engine,int32_t instrument,int32_t zoneIndex,int32_t enabled);
void st_engine_clear_audio_clips(STEngine *engine);
void st_engine_add_audio_clip(STEngine *engine,int32_t asset,int32_t track,double startBeat,double durationBeats,double sourceOffset,float gain,float fadeIn,float fadeOut,int32_t muted);
/* Pattern-mode clip (used by Track Freeze): plays only while the transport is in
   Pattern mode, on `track`, whenever `pattern` plays. Frame 0 of `headAsset` is the
   pattern's row 0; it plays the first pass after Play/Seek/a pattern change, and
   `loopAsset` (>=0, steady-state looping pass followed by its tail) plays every pass
   that follows a natural wrap of the same pattern. When the pattern hands over to a
   different pattern at its natural end, the clip's remaining audio (its printed tail)
   continues under the next pattern. Cleared by st_engine_clear_audio_clips. */
void st_engine_add_pattern_clip(STEngine *engine,int32_t headAsset,int32_t loopAsset,int32_t track,int32_t pattern,float gain);
/* Song-loop tail (used by Track Freeze): whenever the whole song loops (Loop song, no section
   loop, no tempo map), `asset` from source frame `startFrame` on (the print's tail past its
   musical end) plays on `track` from the new pass's frame 0 until the asset ends, so a printed
   reverb/delay/release rings over the loop point as the live chain does. Stop, Seek, Play and
   Panic silence it. Cleared by st_engine_clear_audio_clips. */
void st_engine_add_song_tail_clip(STEngine *engine,int32_t asset,int32_t track,int64_t startFrame,float gain);
void st_engine_commit_update(STEngine *engine);
/* Compile and validate mapped occurrence/clip bounds before atomic publication.
   Failure keeps the previous published project and retains the editable draft. */
int32_t st_engine_commit_update_checked(STEngine *engine,char *error,uint32_t errorCapacity);
void st_engine_cancel_update(STEngine *engine);
void st_engine_collect(STEngine *engine);
void st_engine_play(STEngine *engine,int32_t songMode);
/* Optional pattern-mode loop range (inclusive rows, queued like Play/Seek). While
   set, pattern playback continues at startRow after endRow, sample-accurately at
   the row boundary; a cursor past endRow wraps at the next row boundary and Play
   outside the range starts at startRow. endRow is clamped to the pattern length;
   startRow beyond it disables the range. startRow<0 or endRow<startRow clears it.
   Song mode, export and duration ignore it. It survives Stop/Seek/panic and
   project publication; with no range, playback is unchanged. */
void st_engine_set_pattern_loop(STEngine *engine,int32_t startRow,int32_t endRow);
/* Song section loop over inclusive sequence indices while loopSong is on; -1 disables. Project data (commit). */
void st_engine_set_song_loop(STEngine *engine,int32_t startIndex,int32_t endIndex);
/* Send returns: delayBeats > 0 syncs delay time to tempo (quarter beats), delayTone 0..1 (1 = open)
   low-passes the delay feedback path, reverbPredelay in seconds (0..0.5). Project data (commit). */
void st_engine_set_send_effects(STEngine *engine,double delayBeats,float delayTone,float reverbPredelay);
void st_engine_stop(STEngine *engine);
/* Queue host recording intent at the next newly rendered/control-flush frame.
   May be true while stopped. Play/Stop do not alter it; host session/recording
   lifecycle owns this flag. Captured/restored by the ephemeral checkpoint. */
void st_engine_set_recording(STEngine *engine,int32_t enabled);
void st_engine_seek(STEngine *engine,int32_t pattern,int32_t row,int32_t sequence);
/* Optional queued jump (Maschine-style scene/pattern change), queued like Play/Seek. While playing, the
   next natural pattern end (or pattern-loop-range end) continues sample-accurately at the target instead
   of the normal next position, with voices sounding on as at an ordinary pattern change. Song mode jumps
   to sequence index `sequence` (or, when it is out of range, the first occurrence of `pattern`); Pattern
   mode plays `pattern` (or sequence[sequence] when pattern<0), keeping the current pattern timing origin
   and loop range. A later call replaces the target; both <0 clears it. Ignored while stopped;
   Play/Stop/Seek/panic/restore clear it. Status reports the pending target. Unused: no change. */
void st_engine_queue_jump(STEngine *engine,int32_t pattern,int32_t sequence);
/* Switch between Song (songMode 1) and Pattern (0) mode while playing, at the next pattern (or loop-range)
   end, sample-accurately like a queued jump: Pattern mode loops `pattern` from there; Song mode continues
   at sequence index `sequence`. Replaces a queued jump; reported as that queued target. Ignored while
   stopped or when the mode already matches; Play/Stop/Seek/panic/restore clear it. */
void st_engine_queue_mode(STEngine *engine,int32_t songMode,int32_t pattern,int32_t sequence);
/* Play after a count-in of `bars` (1..4) bars, queued like Play. Clicks once per meter beat at the
   tempo/meter of the start position (accent on each bar's first beat, metronome volume, audible even
   when the metronome is off) while the transport stays stopped, then starts Play (songMode) exactly
   countIn frames later; live notes played during the count-in keep sounding. Ignored while playing.
   Stop/panic/restore cancel it; Seek keeps it and moves its start position. Status reports countIn. */
void st_engine_play_count_in(STEngine *engine,int32_t songMode,int32_t bars);
/* Exclusive stopped-graph operations: no render or event consumer may overlap.
   Capture flushes pending control commands at zero audio time. Both operations
   discard queued events; the host must separately stop AU/external MIDI notes.
   Restore flushes pending commands, clears temporary voices/effects/schedules,
   restores the cursor/mode, and resumes only if state.playing && resume. The host
   owns user cancellation/Stop intent and must pass resume=0 when cancelled;
   temporary export Stop commands cannot distinguish that intent. Active voice,
   step-lock and plug-in DSP state are not checkpointed. A mid-row resume waits
   until the next row for new notes; automation refreshes on the first render.
   These functions invalidate the bus cache without advancing audio time. */
STTransportState st_engine_capture_transport(STEngine *engine);
void st_engine_restore_transport(STEngine *engine,STTransportState state,int32_t resume);
void st_engine_note_on(STEngine *engine,int32_t track,int32_t instrument,int32_t note,int32_t velocity);
void st_engine_note_off(STEngine *engine,int32_t track,int32_t note);
/* Same single control producer as ordinary notes. IDs must increase strictly
   over this engine's lifetime. applyNote=0 captures an input edge without changing
   voices (e.g. key release while sustain defers the sounding note-off).
   Returns 1 only when BOTH command and ACK capacity are reserved. A 0 result
   queues nothing; caller can fall back to ordinary note APIs for performance.
   Every accepted request yields one ACK, including explicit invalid-clock status
   for a discontinuity or quiescent zero-frame flush. Existing STEvent is unchanged.
   distanceBeats is nonnegative musical travel, continuous through EVERY normal
   song/pattern loop, independent of status polling. It need not start at zero.
   runID breaks on Play/Seek/Stop/Panic/restore, graph reset and semantic timing,
   arrangement or loop-domain edits; loops, record-arm changes, normal tempo/meter
   boundaries and otherwise identical project publication preserve it. Only
   APPLIED + CLOCK_PLAYING supplies a usable live recording endpoint. Pattern/row
   come from the applied audio frame AFTER row-boundary resolution. The host must
   correlate request-local project/session/track identities before inserting it. */
int32_t st_engine_input_note(STEngine *engine,uint64_t requestID,int32_t edge,
    int32_t track,int32_t instrument,int32_t note,int32_t velocity,int32_t applyNote);
/* One serialized consumer. A dedicated control-thread timer may drain while the
   audio producer runs: only the SPSC ring and an atomic reservation count are
   accessed. Keep the engine alive and never mix consumers with an audio-thread
   drain. Do not query periodic status to reconstruct this edge. */
int32_t st_engine_pop_input_ack(STEngine *engine,STInputAck *ack);
/* Immediate audition snapshot; replaces only prior preview voices. Negative note
   stops preview. Duration .03..2 seconds, independent of transport and step gate.
   Slice FX is 1-based; note/chord pitches transpose the explicitly selected slice. */
void st_engine_preview_step(STEngine *engine,int32_t track,STStep step,const int32_t *chord,int32_t chordCount,float durationSeconds);
void st_engine_panic(STEngine *engine);
/* Exclusive stopped-graph operation (no render or event consumer may overlap): end
   every AU/external-MIDI voice on `channel` WITHOUT emitting its note-off. For a host
   that moved the channel's plug-in elsewhere (track reorder) and released that
   plug-in's notes directly, so the old channel's new occupant never receives a stray
   release. Returns the number of voices ended; 0 for an invalid channel. */
int32_t st_engine_forget_external_voices(STEngine *engine,int32_t channel);
void st_engine_pitch_bend(STEngine *engine,int32_t track,float semitones);
/* Live modulation wheel (MIDI CC1) for built-in instruments on `track`, 0..1 (0 = off). The engine has no
   dedicated mod-wheel destination, so it closes the voice filter by up to 5.25 octaves (cutoff x 2^(-5.25 v)),
   smoothed like other cutoff changes. Reset by panic/restore. AU/MIDI instruments receive CC1 from the host. */
void st_engine_mod_wheel(STEngine *engine,int32_t track,float value);
/* Live instrument parameter (a knob drag) without republishing the project: queued like notes, applied on the
   render thread to the instrument's sounding voices (held keys, sustained pattern notes, previews) and to notes
   it starts until the next project publication, which carries the committed value and clears these overrides.
   target is an instrument ST_TARGET_* value (ST_TARGET_ATTACK..ST_TARGET_LAST_INSTRUMENT) or ST_TARGET_CUTOFF /
   ST_TARGET_RESONANCE, which here mean the instrument's own filter. Volume, pan and cutoff glide through the
   voices' existing smoothers; the region (start/end/loop) applies to the next note only. Track automation of
   the same target keeps precedence. Returns 0 for an invalid instrument/target or a full queue. */
int32_t st_engine_set_instrument_parameter(STEngine *engine,int32_t instrument,int32_t target,float value);
/* Coalesce an automation refresh request from any thread, including an audio callback.
   At the next newly rendered sample, emit effective mixer controls and current lanes
   after ordinary commands. Preserves transport, voices and locks; never fills the
   single-producer command queue. Useful after AU latency/routing changes. */
void st_engine_refresh_automation(STEngine *engine);
void st_engine_render(STEngine *engine,float *left,float *right,uint32_t frames);
/* Track buses and the return bus as numbered by st_channel_output_bus/st_return_bus
   for the published track count; buses beyond return silence.
   Buses that stay silent cost nothing: their retained history is implicit zeros.
   Track gain/pan are included in legacy mixing, omitted in raw host mixing.
   Serialized graph pulls may overlap and use different frame counts. A 65536-frame
   timestamp ring retains all buses; only an uncached suffix advances DSP. Requests
   may contain up to65536 frames and internally render in chunks of4096. Earlier
   frames outside retained history return silence. Event offsets refer to the caller's
   requested sampleTime, including any reused prefix. masterVolume is already applied.
   Ordinary control commands apply at the next uncached frame without invalidating
   another branch's audio. Do not mix direct render and bus render in one graph epoch. */
void st_engine_render_bus(STEngine *engine,int32_t bus,uint64_t sampleTime,float *left,float *right,uint32_t frames);
/* Same as render_bus, addressed by logical channel (its bus is resolved with the
   project the render thread is using); channel -1 is the return bus. Sources keyed
   by channel keep pulling the right audio when the track count moves the return bus. */
void st_engine_render_channel(STEngine *engine,int32_t channel,uint64_t sampleTime,float *left,float *right,uint32_t frames);
/* Call only while the graph is stopped, before a restart/manual-render timestamp reset.
   Starts a fresh timestamp epoch; does not reset music, effects or queued commands. */
void st_engine_reset_render_cache(STEngine *engine);
enum { ST_ENGINE_CLOCK_NOT_PREPARED=-2, ST_ENGINE_CLOCK_NOT_RETAINED=-3,
       ST_ENGINE_CLOCK_HISTORY_FRAMES=262144 };
/* Read-only, serialized with render_bus. Queries already prepared timestamped
   history, subtracting upstream latency; never renders or consumes commands.
   Clock history retains262144 frames independently of the65536-frame PCM ring.
   Future/unprepared content returns NOT_PREPARED; expired post-origin content
   returns NOT_RETAINED. Up to2^30 frames before the first prepared timestamp use
   the immutable initial map/consumed transport origin (PREROLL if playing, no
   backward loop); a stopped origin holds its position. Later edits cannot change
   this startup context. Resetting the render cache replaces that origin on the
   next prepared frame. Output is untouched on error. A successful span may end
   at the available prefix, a musical boundary, or a retained transport/revision
   edit. Graph coordinates are within +/-2^52, latency0..2^30, requested1..65536. */
int32_t st_engine_clock_span(STEngine *engine,int64_t graphFrame,uint32_t requestedFrames,
    int64_t upstreamLatencyFrames,STClockSpan *span);
/* Pure retained query with clock_span's synchronization, range and retention
   rules. No rendering, command consumption, allocation or latest-status lookup.
   Returns a bounded contiguous span, splitting every mapping/phase/publication/
   playback/transition boundary, including multiple loops within one pull.
   Playback IDs change at loops independently of input ACK run IDs. A natural
   tail requires phase NATURAL_TAIL AND explicit cause NATURAL_END; its project
   index advances beyond transitionProjectFrame while native transport is held.
   Pre-origin queries are explicitly silent STOPPED+PREROLL, even after a seek
   to positive song time. History exhaustion is unavailable, never extrapolated.
   Host must prepare each needed range with render_bus (null outputs are valid),
   including when every track is frozen/muted and no original source is pulled.
   A silent transport anchor or equivalent serialized preflight owns that step;
   cached overlap never advances twice. Query-before-prepare returns unavailable.
   No configured history returns ST_FREEZE_NOT_CONFIGURED. Other statuses match
   clock_span. Session ownership is the caller's responsibility. */
int32_t st_engine_freeze_span(STEngine *engine,int64_t graphFrame,uint32_t requestedFrames,
    int64_t upstreamLatencyFrames,STFreezeSpan *span);
int32_t st_engine_pop_event(STEngine *engine,STEvent *event);
STStatus st_engine_status(STEngine *engine);
/* Live playheads for the sample editor. While watched, the render thread publishes, once per render block, a
   fixed-size snapshot of the sounding sampler voices (seqlock, no allocation or locks; nothing when unwatched).
   position: frames into the voice's sample (the playhead; the scan centre in Granular mode; the frame start in
   Wavetable mode). windowStart/windowEnd: the current wavetable frame, or the span of the grains around the scan
   position in Granular mode; equal to position otherwise. grains: each sounding grain's read position (frames)
   and window level. direction is +1/-1 (Granular: the scan direction, 0 when frozen). */
enum { ST_VOICE_POSITION_CAPACITY=64 };
enum { ST_VOICE_POSITION_RELEASING=1, ST_VOICE_POSITION_PREVIEW=2, ST_VOICE_POSITION_ZONE=4, ST_VOICE_POSITION_SEQUENCED=8 };
typedef struct {
    int32_t instrument,track,note,mode,direction,flags,grainCount,reserved;
    double position,windowStart,windowEnd,sampleFrames;
    float level; float grainLevels[ST_GRANULAR_MAX_GRAINS];
    double grainPositions[ST_GRANULAR_MAX_GRAINS];
} STVoicePosition;
/* instrument >=0 watches that instrument, -1 every instrument, -2 (or less) stops publishing. Any thread. */
void st_engine_watch_voice_positions(STEngine *engine,int32_t instrument);
/* Copies the latest snapshot (up to capacity voices); returns the voice count, 0 when nothing sounds or nothing
   is watched. *serial (optional) increases with every published block. Any thread; never blocks the renderer. */
int32_t st_engine_voice_positions(STEngine *engine,STVoicePosition *voices,int32_t capacity,uint64_t *serial);
float st_engine_track_peak(STEngine *engine,int32_t track);
/* Duration excludes release/send tails; songMode 0 uses active pattern. */
double st_engine_duration(STEngine *engine,int32_t songMode);
STTrack st_default_track(void);
STInstrument st_default_instrument(void);
STStep st_default_step(void);

#ifdef __cplusplus
}
#endif
#endif
