#ifndef ST_BUILTIN_EFFECTS_H
#define ST_BUILTIN_EFFECTS_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* First-party processors inspired by conventional studio effects. IDs and
   parameter IDs are persistent; catalogue indices are not project identities.
   All ranges/defaults are in displayed physical units. */
enum { ST_BUILTIN_PARAMETER_LOG=1, ST_BUILTIN_PARAMETER_INTEGER=2,
       ST_BUILTIN_PARAMETER_TOGGLE=4, ST_BUILTIN_EFFECT_SIDECHAIN=1 };
typedef struct {
    const char *id, *name, *unit;
    double minimum, maximum, defaultValue;
    uint32_t flags;
    const char * const *labels;
    uint32_t labelCount;
} STBuiltinParameterInfo;
typedef struct {
    const char *id, *name, *category;
    uint32_t parameterCount, flags;
} STBuiltinEffectInfo;
typedef struct STBuiltinEffect STBuiltinEffect;

uint32_t st_builtin_effect_count(void);
const STBuiltinEffectInfo *st_builtin_effect_info(uint32_t index);
const STBuiltinEffectInfo *st_builtin_effect_find(const char *id);
const STBuiltinParameterInfo *st_builtin_effect_parameter_info(const char *id,uint32_t parameter);
const char *st_builtin_effect_parameter_label(const char *id,uint32_t parameter,uint32_t label);
double st_builtin_effect_to_normalized(const char *id,uint32_t parameter,double physicalValue);
double st_builtin_effect_from_normalized(const char *id,uint32_t parameter,double normalizedValue);

/* Create/destroy allocate and belong on a non-render thread. Supported rate
   8k..384k, maxFrames 1..1048576. Unknown IDs/invalid arguments return NULL.
   A handle has one serialized render owner. Reset, destroy, and process must
   never overlap. Atomic set/get/bypass/tempo may overlap process. */
STBuiltinEffect *st_builtin_effect_create(const char *id,double sampleRate,uint32_t maxFrames);
void st_builtin_effect_destroy(STBuiltinEffect *effect);
int32_t st_builtin_effect_set_parameter(STBuiltinEffect *effect,uint32_t parameter,double normalizedValue);
double st_builtin_effect_get_parameter(const STBuiltinEffect *effect,uint32_t parameter);
void st_builtin_effect_set_bypass(STBuiltinEffect *effect,int32_t bypass);
/* Finite tempos are clamped to the tracker tempo range, 20..999 BPM. */
void st_builtin_effect_set_tempo(STBuiltinEffect *effect,double bpm);
void st_builtin_effect_reset(STBuiltinEffect *effect);
uint32_t st_builtin_effect_latency(const STBuiltinEffect *effect);
/* Conservative estimate from atomic parameter targets or fixed family maximum,
   available immediately after create/restore without requiring a render.
   Sustain/freeze algorithms return a finite offline-export cap (120 seconds),
   not a promise that a frozen/looped sound naturally decays by then. */
double st_builtin_effect_tail_seconds(const STBuiltinEffect *effect);

/* In-place stereo. left/right must be distinct arrays of at least frames.
   Both sidechain pointers NULL means internal detection; one NULL duplicates
   the other. Sidechain may alias main inputs. Oversized calls are chunked.
   Render performs no allocation, locking or system calls. Bypass is exact and
   freezes processor state; host handles click-free bypass and delay alignment.
   With the effect active, nonfinite live input is replaced by silence.
   Render runs with denormals flushed (the caller's FP control state is
   restored). Silence gating: once main and sidechain input have stayed below
   -120 dBFS for longer than the tail estimate and the output has decayed
   below it too, the DSP is skipped and the output is exact silence until
   input returns (sustaining/freeze modes are never gated).
   Returns 0 only for invalid
   handle/buffers. No guarantee of NI bit/sound/preset compatibility is made. */
int32_t st_builtin_effect_process(STBuiltinEffect *effect,float *left,float *right,
    const float *sidechainLeft,const float *sidechainRight,uint32_t frames);

/* Original drum synthesis families. Model numbers 1..6 are Kick, Snare, Hihat,
   Tom, Perc, Cymbal; zero means no drum model. Catalogue index is 0..5.
   Eight normalized controls, not NI serialized parameter arrays. */
uint32_t st_drum_model_count(void);
const STBuiltinEffectInfo *st_drum_model_info(uint32_t index);
int32_t st_drum_model_for_id(const char *id);
const STBuiltinParameterInfo *st_drum_parameter_info(int32_t model,uint32_t parameter);
double st_drum_parameter_default(int32_t model,uint32_t parameter);
double st_drum_parameter_to_normalized(int32_t model,uint32_t parameter,double physicalValue);
double st_drum_parameter_from_normalized(int32_t model,uint32_t parameter,double normalizedValue);
/* Offline render of one drum hit at its root pitch into `out` (mono), until the
   voice ends or `capacity` frames. Nonfinite controls use the model defaults.
   Returns the number of frames written; 0 for an invalid model or buffer. */
uint32_t st_drum_render(int32_t model,const float *normalized,uint32_t count,double sampleRate,float *out,uint32_t capacity);
#ifdef __cplusplus
}
#endif
#endif
