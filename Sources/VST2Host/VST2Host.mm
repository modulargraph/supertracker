#import <AppKit/AppKit.h>
#import <CoreFoundation/CoreFoundation.h>
#include "VST2Host.h"

extern "C" int st_objc_try(void (*body)(void *context), void *context, char *message, size_t capacity) {
    @try { body(context); return 0; }
    @catch (NSException *exception) {
        if (message && capacity) {
            NSString *text = exception.reason.length ? exception.reason : exception.name;
            strlcpy(message, text.UTF8String ?: "Objective-C exception", capacity);
        }
        return 1;
    }
}
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <vector>

// ---------------------------------------------------------------------------
// VST 2.4 binary interface, declared from the published ABI (field order, opcode
// numbers and structure layouts). Only what this host uses is declared.
// ---------------------------------------------------------------------------
namespace vst2 {
typedef intptr_t IntPtr;
struct Effect;
typedef IntPtr (*HostCallback)(Effect *, int32_t opcode, int32_t index, IntPtr value, void *ptr, float opt);
typedef IntPtr (*Dispatcher)(Effect *, int32_t opcode, int32_t index, IntPtr value, void *ptr, float opt);
typedef void (*ProcessFunction)(Effect *, float **inputs, float **outputs, int32_t frames);
typedef void (*ProcessDoubleFunction)(Effect *, double **inputs, double **outputs, int32_t frames);
typedef void (*SetParameterFunction)(Effect *, int32_t index, float value);
typedef float (*GetParameterFunction)(Effect *, int32_t index);
typedef Effect *(*EntryPoint)(HostCallback);

struct Effect {
    int32_t magic;
    Dispatcher dispatcher;
    ProcessFunction process;
    SetParameterFunction setParameter;
    GetParameterFunction getParameter;
    int32_t numPrograms, numParams, numInputs, numOutputs;
    int32_t flags;
    IntPtr hostReserved1, hostReserved2;
    int32_t initialDelay;
    int32_t realQualities, offQualities;
    float ioRatio;
    void *object, *user;
    int32_t uniqueID, version;
    ProcessFunction processReplacing;
    ProcessDoubleFunction processDoubleReplacing;
    char future[56];
};

enum : int32_t { kMagic = 0x56737450 /* 'VstP' */ };
enum : int32_t {
    kFlagHasEditor = 1 << 0, kFlagCanReplacing = 1 << 4, kFlagProgramChunks = 1 << 5,
    kFlagIsSynth = 1 << 8, kFlagNoSoundInStop = 1 << 9, kFlagCanDoubleReplacing = 1 << 12
};
enum : int32_t {
    effOpen = 0, effClose = 1, effSetProgram = 2, effGetProgram = 3, effGetProgramName = 5,
    effGetParamLabel = 6, effGetParamDisplay = 7, effGetParamName = 8, effSetSampleRate = 10,
    effSetBlockSize = 11, effMainsChanged = 12, effEditGetRect = 13, effEditOpen = 14,
    effEditClose = 15, effEditIdle = 19, effGetChunk = 23, effSetChunk = 24, effProcessEvents = 25,
    effCanBeAutomated = 26, effGetProgramNameIndexed = 29, effGetPlugCategory = 35,
    effSetBypass = 44, effGetEffectName = 45, effGetVendorString = 47, effGetProductString = 48,
    effGetVendorVersion = 49, effCanDo = 51, effGetTailSize = 52, effGetVstVersion = 58,
    effShellGetNextPlugin = 70, effStartProcess = 71, effStopProcess = 72,
    effBeginLoadBank = 75, effBeginLoadProgram = 76, effSetProcessPrecision = 77
};
enum : int32_t {
    audioMasterAutomate = 0, audioMasterVersion = 1, audioMasterCurrentId = 2, audioMasterIdle = 3,
    audioMasterWantMidi = 6, audioMasterGetTime = 7, audioMasterProcessEvents = 8,
    audioMasterIOChanged = 13, audioMasterNeedIdle = 14, audioMasterSizeWindow = 15,
    audioMasterGetSampleRate = 16, audioMasterGetBlockSize = 17, audioMasterGetInputLatency = 18,
    audioMasterGetOutputLatency = 19, audioMasterGetCurrentProcessLevel = 23,
    audioMasterGetAutomationState = 24, audioMasterGetVendorString = 32,
    audioMasterGetProductString = 33, audioMasterGetVendorVersion = 34, audioMasterCanDo = 37,
    audioMasterGetLanguage = 38, audioMasterGetDirectory = 41, audioMasterUpdateDisplay = 42,
    audioMasterBeginEdit = 43, audioMasterEndEdit = 44
};
enum : int32_t { kCategorySynth = 2, kCategoryShell = 10 };

struct TimeInfo {
    double samplePos, sampleRate, nanoSeconds, ppqPos, tempo, barStartPos, cycleStartPos, cycleEndPos;
    int32_t timeSigNumerator, timeSigDenominator, smpteOffset, smpteFrameRate, samplesToNextClock, flags;
};
enum : int32_t {
    kTransportChanged = 1, kTransportPlaying = 1 << 1, kTransportCycleActive = 1 << 2,
    kTransportRecording = 1 << 3, kNanosValid = 1 << 8, kPpqPosValid = 1 << 9, kTempoValid = 1 << 10,
    kBarsValid = 1 << 11, kCyclePosValid = 1 << 12, kTimeSigValid = 1 << 13
};

struct Event { int32_t type, byteSize, deltaFrames, flags; char data[16]; };
struct MidiEvent {
    int32_t type, byteSize, deltaFrames, flags, noteLength, noteOffset;
    char midiData[4];
    char detune, noteOffVelocity, reserved1, reserved2;
};
enum : int32_t { kMidiType = 1, kMidiEventIsRealtime = 1 };
struct Events { int32_t numEvents; IntPtr reserved; Event *events[2]; };
struct ERect { int16_t top, left, bottom, right; };
static_assert(sizeof(MidiEvent) == 32, "VST2 MIDI event layout");
static_assert(offsetof(Effect, processReplacing) == 120, "VST2 effect layout");
} // namespace vst2

using namespace vst2;

namespace {
constexpr uint32_t kMaximumFrames = 16384;
constexpr int kEventCapacity = 2048;
constexpr int32_t kMaximumChannels = 64;

void write_error(char *error, size_t size, const std::string &message) {
    if (!error || size == 0) return;
    std::snprintf(error, size, "%s", message.c_str());
}

void copy_text(char *destination, size_t capacity, const char *source) {
    if (!destination || capacity == 0) return;
    std::snprintf(destination, capacity, "%s", source ? source : "");
}

// Modules stay loaded for the life of the process. Many VST2 plug-ins register
// Objective-C classes or static workers that crash if their image is unloaded.
std::mutex module_lock;
std::map<std::string, CFBundleRef> modules;

EntryPoint load_entry(const char *path, std::string &message) {
    std::lock_guard<std::mutex> guard(module_lock);
    CFBundleRef bundle = nullptr;
    auto existing = modules.find(path);
    if (existing != modules.end()) bundle = existing->second;
    else {
        CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, (const UInt8 *)path, std::strlen(path), true);
        if (!url) { message = "The VST2 bundle path is invalid."; return nullptr; }
        bundle = CFBundleCreate(nullptr, url);
        CFRelease(url);
        if (!bundle) { message = "The VST2 bundle could not be opened."; return nullptr; }
        CFErrorRef error = nullptr;
        if (!CFBundleLoadExecutableAndReturnError(bundle, &error)) {
            message = "Unable to load VST2 executable (architecture, dependency, or code signature).";
            if (error) {
                CFStringRef text = CFErrorCopyDescription(error);
                char buffer[512] = {0};
                if (text && CFStringGetCString(text, buffer, sizeof buffer, kCFStringEncodingUTF8)) message += std::string(" ") + buffer;
                if (text) CFRelease(text);
                CFRelease(error);
            }
            CFRelease(bundle);
            return nullptr;
        }
        modules[path] = bundle;
    }
    for (CFStringRef name : {CFSTR("VSTPluginMain"), CFSTR("main_macho"), CFSTR("main")}) {
        if (void *symbol = CFBundleGetFunctionPointerForName(bundle, name)) return (EntryPoint)symbol;
    }
    message = "The bundle does not export a VST2 entry point.";
    return nullptr;
}
} // namespace

struct STVST2Instance {
    Effect *effect = nullptr;
    std::string path;
    int32_t shellID = 0;
    double sampleRate = 48000;
    uint32_t maximumFrames = 512;
    bool active = false, editorOpen = false;
    int32_t numInputs = 0, numOutputs = 0, numParams = 0;
    std::unique_ptr<std::atomic<float>[]> values;
    std::unique_ptr<std::atomic<uint8_t>[]> changed;
    std::atomic<uint32_t> hostFlags{0};
    std::atomic<int32_t> requestedWidth{0}, requestedHeight{0};
    std::atomic<uint8_t> sizeRequested{0};
    TimeInfo time{};
    std::vector<float> inputStorage, outputStorage;
    std::vector<float *> inputs, outputs;
    std::vector<MidiEvent> midiEvents;
    std::array<uint32_t,kEventCapacity> parameterOrder{};
    std::vector<uint8_t> eventsBlock;
    // Sounding notes by channel/pitch. Many VST2 instruments ignore CC 120/123,
    // which timing-event epochs and reset() rely on; the host sends explicit note-offs.
    std::array<std::array<uint8_t,128>,16> heldNotes{};
};

namespace {
thread_local STVST2Instance *constructing = nullptr;
thread_local int32_t constructingShellID = 0;
thread_local bool audioThread = false;

STVST2Instance *owner(Effect *effect) {
    if (effect && effect->hostReserved1) return reinterpret_cast<STVST2Instance *>(effect->hostReserved1);
    return constructing;
}

bool host_can_do(const char *text) {
    if (!text) return false;
    static const char *supported[] = {"sendVstEvents", "sendVstMidiEvent", "sendVstTimeInfo", "receiveVstEvents",
        "receiveVstMidiEvent", "sizeWindow", "shellCategory", "supportShell", "startStopProcess",
        "supplyIdle", "sendVstMidiEventFlagIsRealtime"};
    for (const char *item : supported) if (std::strcmp(text, item) == 0) return true;
    return false;
}

IntPtr host_callback(Effect *effect, int32_t opcode, int32_t index, IntPtr value, void *ptr, float opt) {
    STVST2Instance *instance = owner(effect);
    switch (opcode) {
    case audioMasterVersion: return 2400;
    case audioMasterCurrentId: return constructingShellID ? constructingShellID : (instance ? instance->shellID : 0);
    case audioMasterIdle: return 0;
    case audioMasterWantMidi: return 1;
    case audioMasterGetTime: return instance ? (IntPtr)&instance->time : 0;
    case audioMasterProcessEvents: return 0;
    case audioMasterAutomate:
        if (instance && index >= 0 && index < instance->numParams && std::isfinite(opt)) {
            instance->values[index].store(opt, std::memory_order_relaxed);
            instance->changed[index].store(1, std::memory_order_release);
        }
        return 0;
    case audioMasterBeginEdit: case audioMasterEndEdit: return 1;
    case audioMasterIOChanged: if (instance) instance->hostFlags.fetch_or(1); return 1;
    case audioMasterUpdateDisplay: if (instance) instance->hostFlags.fetch_or(2); return 1;
    case audioMasterNeedIdle: return 1;
    case audioMasterSizeWindow:
        if (instance && index > 0 && value > 0 && index < 16384 && value < 16384) {
            instance->requestedWidth = index; instance->requestedHeight = (int32_t)value; instance->sizeRequested = 1;
            return 1;
        }
        return 0;
    case audioMasterGetSampleRate: return instance ? (IntPtr)instance->sampleRate : 48000;
    case audioMasterGetBlockSize: return instance ? (IntPtr)instance->maximumFrames : 512;
    case audioMasterGetInputLatency: case audioMasterGetOutputLatency: return 0;
    case audioMasterGetCurrentProcessLevel: return audioThread ? 2 : 1;
    case audioMasterGetAutomationState: return 1;
    case audioMasterGetVendorString: if (ptr) std::strcpy((char *)ptr, "SuperTracker"); return 1;
    case audioMasterGetProductString: if (ptr) std::strcpy((char *)ptr, "SuperTracker"); return 1;
    case audioMasterGetVendorVersion: return 1000;
    case audioMasterCanDo: return host_can_do((const char *)ptr) ? 1 : 0;
    case audioMasterGetLanguage: return 1;
    case audioMasterGetDirectory: return 0;
    default: return 0;
    }
}

IntPtr dispatch(Effect *effect, int32_t opcode, int32_t index = 0, IntPtr value = 0, void *ptr = nullptr, float opt = 0) {
    return effect->dispatcher(effect, opcode, index, value, ptr, opt);
}

std::string effect_string(Effect *effect, int32_t opcode, int32_t index = 0) {
    char buffer[512] = {0};
    dispatch(effect, opcode, index, 0, buffer);
    buffer[sizeof buffer - 1] = 0;
    return buffer;
}

Effect *instantiate(const char *path, int32_t shellID, STVST2Instance *instance, std::string &message) {
    EntryPoint entry = load_entry(path, message);
    if (!entry) return nullptr;
    STVST2Instance *previous = constructing;
    int32_t previousShell = constructingShellID;
    constructing = instance; constructingShellID = shellID;
    Effect *effect = nullptr;
    @try { effect = entry(host_callback); }
    @catch (NSException *exception) { message = std::string("The VST2 plug-in raised an exception: ") + (exception.reason.UTF8String ?: ""); effect = nullptr; }
    constructing = previous; constructingShellID = previousShell;
    if (!effect) { if (message.empty()) message = "The VST2 plug-in could not be instantiated."; return nullptr; }
    if (effect->magic != kMagic) { message = "The plug-in returned an invalid VST2 effect."; return nullptr; }
    return effect;
}

void fill_info(Effect *effect, int32_t shellID, STVST2Info *info) {
    std::memset(info, 0, sizeof *info);
    info->unique_id = effect->uniqueID;
    info->shell_id = shellID;
    copy_text(info->name, sizeof info->name, effect_string(effect, effGetEffectName).c_str());
    copy_text(info->vendor, sizeof info->vendor, effect_string(effect, effGetVendorString).c_str());
    copy_text(info->product, sizeof info->product, effect_string(effect, effGetProductString).c_str());
    info->vendor_version = (int32_t)dispatch(effect, effGetVendorVersion);
    info->category = (int32_t)dispatch(effect, effGetPlugCategory);
    info->is_instrument = ((effect->flags & kFlagIsSynth) != 0 || info->category == kCategorySynth) ? 1 : 0;
    info->has_editor = (effect->flags & kFlagHasEditor) ? 1 : 0;
    info->program_chunks = (effect->flags & kFlagProgramChunks) ? 1 : 0;
    info->is_shell = info->category == kCategoryShell ? 1 : 0;
    info->inputs = effect->numInputs; info->outputs = effect->numOutputs;
    info->parameters = effect->numParams; info->programs = effect->numPrograms;
}

void close_effect(Effect *effect) {
    if (!effect) return;
    dispatch(effect, effMainsChanged, 0, 0);
    dispatch(effect, effClose);
}

uint32_t be32(const uint8_t *p) { return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3]; }
float befloat(const uint8_t *p) { uint32_t bits = be32(p); float value; std::memcpy(&value, &bits, 4); return value; }

struct StateHeader {
    char magic[4];
    uint32_t version;
    int32_t uniqueID;
    int32_t kind; // 1 = bank chunk, 2 = parameter snapshot
    int32_t program;
    uint32_t payloadBytes;
};
} // namespace

extern "C" {

int st_vst2_bundle_is_loadable(const char *bundle_path, char *error, size_t error_size) {
    @autoreleasepool {
        NSBundle *bundle = [NSBundle bundleWithPath:[NSString stringWithUTF8String:bundle_path ?: ""]];
        if (!bundle) { write_error(error, error_size, "The VST2 bundle could not be read."); return 0; }
        NSArray<NSNumber *> *architectures = bundle.executableArchitectures;
#if defined(__arm64__)
        NSNumber *current = @(NSBundleExecutableArchitectureARM64);
#else
        NSNumber *current = @(NSBundleExecutableArchitectureX86_64);
#endif
        if (architectures && ![architectures containsObject:current]) {
            write_error(error, error_size, "This VST2 plug-in is built for Intel Macs only and cannot run natively on this Mac.");
            return 0;
        }
        return 1;
    }
}

int st_vst2_scan_bundle(const char *bundle_path, STVST2InfoCallback callback, void *context, char *error, size_t error_size) {
    if (!bundle_path || !callback) { write_error(error, error_size, "Invalid VST2 scan request."); return 0; }
    if (!st_vst2_bundle_is_loadable(bundle_path, error, error_size)) return 0;
    std::string message;
    STVST2Instance probe;
    Effect *effect = instantiate(bundle_path, 0, &probe, message);
    if (!effect) { write_error(error, error_size, message); return 0; }
    effect->hostReserved1 = (IntPtr)&probe;
    dispatch(effect, effOpen);
    STVST2Info info;
    fill_info(effect, 0, &info);
    if (info.is_shell) {
        std::vector<std::pair<int32_t, std::string>> children;
        for (int count = 0; count < 4096; ++count) {
            char name[256] = {0};
            int32_t id = (int32_t)dispatch(effect, effShellGetNextPlugin, 0, 0, name);
            if (id == 0) break;
            name[sizeof name - 1] = 0;
            children.emplace_back(id, name);
        }
        std::string vendor = info.vendor;
        close_effect(effect);
        for (auto &child : children) {
            STVST2Instance childProbe;
            childProbe.shellID = child.first;
            std::string childMessage;
            Effect *instance = instantiate(bundle_path, child.first, &childProbe, childMessage);
            STVST2Info childInfo;
            if (instance) {
                instance->hostReserved1 = (IntPtr)&childProbe;
                dispatch(instance, effOpen);
                fill_info(instance, child.first, &childInfo);
                close_effect(instance);
            } else {
                std::memset(&childInfo, 0, sizeof childInfo);
                childInfo.unique_id = child.first; childInfo.shell_id = child.first;
                copy_text(childInfo.vendor, sizeof childInfo.vendor, vendor.c_str());
                childInfo.inputs = 2; childInfo.outputs = 2;
            }
            if (!child.second.empty()) copy_text(childInfo.name, sizeof childInfo.name, child.second.c_str());
            childInfo.shell_id = child.first;
            childInfo.is_shell = 0;
            callback(&childInfo, context);
        }
        return 1;
    }
    close_effect(effect);
    if (info.name[0] == 0) {
        NSString *fallback = [[NSString stringWithUTF8String:bundle_path].lastPathComponent stringByDeletingPathExtension];
        copy_text(info.name, sizeof info.name, fallback.UTF8String);
    }
    callback(&info, context);
    return 1;
}

STVST2Instance *st_vst2_create(const char *bundle_path, int32_t shell_id, double sample_rate, uint32_t maximum_frames,
                               char *error, size_t error_size) {
    if (!bundle_path || !(sample_rate >= 8000 && sample_rate <= 768000) || maximum_frames == 0 || maximum_frames > kMaximumFrames) {
        write_error(error, error_size, "Invalid VST2 configuration."); return nullptr;
    }
    if (!st_vst2_bundle_is_loadable(bundle_path, error, error_size)) return nullptr;
    auto instance = std::make_unique<STVST2Instance>();
    instance->path = bundle_path; instance->shellID = shell_id;
    instance->sampleRate = sample_rate; instance->maximumFrames = maximum_frames;
    std::string message;
    Effect *effect = instantiate(bundle_path, shell_id, instance.get(), message);
    if (!effect) { write_error(error, error_size, message); return nullptr; }
    effect->hostReserved1 = (IntPtr)instance.get();
    instance->effect = effect;
    constructing = instance.get();
    dispatch(effect, effOpen);
    constructing = nullptr;
    if (dispatch(effect, effGetPlugCategory) == kCategoryShell) {
        close_effect(effect); instance->effect = nullptr;
        write_error(error, error_size, "Choose a plug-in inside this VST2 shell."); return nullptr;
    }
    if (effect->numInputs < 0 || effect->numOutputs <= 0 || effect->numInputs > kMaximumChannels ||
        effect->numOutputs > kMaximumChannels || effect->numParams < 0 || effect->numParams > 100000) {
        close_effect(effect); instance->effect = nullptr;
        write_error(error, error_size, "The VST2 plug-in reports an unsupported audio layout."); return nullptr;
    }
    instance->numInputs = effect->numInputs; instance->numOutputs = effect->numOutputs; instance->numParams = effect->numParams;
    instance->values.reset(new std::atomic<float>[std::max(1, instance->numParams)]);
    instance->changed.reset(new std::atomic<uint8_t>[std::max(1, instance->numParams)]);
    for (int32_t i = 0; i < instance->numParams; ++i) {
        float value = effect->getParameter(effect, i);
        instance->values[i].store(std::isfinite(value) ? value : 0); instance->changed[i].store(0);
    }
    instance->inputStorage.assign((size_t)std::max(2, instance->numInputs) * kMaximumFrames, 0.f);
    instance->outputStorage.assign((size_t)std::max(2, instance->numOutputs) * kMaximumFrames, 0.f);
    instance->inputs.resize(std::max(1, instance->numInputs));
    instance->outputs.resize(std::max(1, instance->numOutputs));
    for (int32_t i = 0; i < instance->numInputs; ++i) instance->inputs[i] = instance->inputStorage.data() + (size_t)i * kMaximumFrames;
    for (int32_t i = 0; i < instance->numOutputs; ++i) instance->outputs[i] = instance->outputStorage.data() + (size_t)i * kMaximumFrames;
    instance->midiEvents.resize(kEventCapacity * 2); // Input events plus panic-generated note-offs.
    instance->eventsBlock.assign(sizeof(Events) + sizeof(Event *) * kEventCapacity * 2, 0);
    instance->time.sampleRate = sample_rate; instance->time.tempo = 120; instance->time.timeSigNumerator = 4; instance->time.timeSigDenominator = 4;
    dispatch(effect, effSetProcessPrecision, 0, 0);
    dispatch(effect, effSetSampleRate, 0, 0, nullptr, (float)sample_rate);
    dispatch(effect, effSetBlockSize, 0, (IntPtr)maximum_frames);
    return instance.release();
}

void st_vst2_destroy(STVST2Instance *instance) {
    if (!instance) return;
    if (instance->effect) {
        if (instance->editorOpen) { dispatch(instance->effect, effEditClose); instance->editorOpen = false; }
        if (instance->active) { dispatch(instance->effect, effStopProcess); dispatch(instance->effect, effMainsChanged, 0, 0); }
        dispatch(instance->effect, effClose);
        instance->effect = nullptr;
    }
    delete instance;
}

int st_vst2_info(STVST2Instance *instance, STVST2Info *info) {
    if (!instance || !instance->effect || !info) return 0;
    fill_info(instance->effect, instance->shellID, info);
    return 1;
}

int st_vst2_prepare(STVST2Instance *instance, double sample_rate, uint32_t maximum_frames, char *error, size_t error_size) {
    if (!instance || !instance->effect) { write_error(error, error_size, "No VST2 plug-in is loaded."); return 0; }
    if (!(sample_rate >= 8000 && sample_rate <= 768000) || maximum_frames == 0 || maximum_frames > kMaximumFrames) {
        write_error(error, error_size, "Invalid VST2 audio format."); return 0;
    }
    Effect *effect = instance->effect;
    if (instance->active && instance->sampleRate == sample_rate && instance->maximumFrames == maximum_frames) return 1;
    st_vst2_suspend(instance);
    instance->sampleRate = sample_rate; instance->maximumFrames = maximum_frames;
    instance->time.sampleRate = sample_rate;
    dispatch(effect, effSetSampleRate, 0, 0, nullptr, (float)sample_rate);
    dispatch(effect, effSetBlockSize, 0, (IntPtr)maximum_frames);
    dispatch(effect, effMainsChanged, 0, 1);
    dispatch(effect, effStartProcess);
    instance->active = true;
    return 1;
}

void st_vst2_suspend(STVST2Instance *instance) {
    if (!instance || !instance->effect || !instance->active) return;
    dispatch(instance->effect, effStopProcess);
    dispatch(instance->effect, effMainsChanged, 0, 0);
    instance->active = false;
}

int st_vst2_is_active(STVST2Instance *instance) { return instance && instance->active ? 1 : 0; }
uint32_t st_vst2_latency(STVST2Instance *instance) {
    return instance && instance->effect && instance->effect->initialDelay > 0 ? (uint32_t)instance->effect->initialDelay : 0;
}
uint32_t st_vst2_tail(STVST2Instance *instance) {
    if (!instance || !instance->effect) return 0;
    IntPtr tail = dispatch(instance->effect, effGetTailSize);
    return tail > 0 ? (uint32_t)std::min<IntPtr>(tail, 48000 * 60) : 0;
}

int st_vst2_parameter_count(STVST2Instance *instance) { return instance ? instance->numParams : 0; }

static int parameter_text(STVST2Instance *instance, int32_t opcode, int32_t index, char *text, size_t capacity) {
    if (!instance || !instance->effect || index < 0 || index >= instance->numParams) return 0;
    copy_text(text, capacity, effect_string(instance->effect, opcode, index).c_str());
    return 1;
}
int st_vst2_parameter_name(STVST2Instance *instance, int32_t index, char *text, size_t capacity) { return parameter_text(instance, effGetParamName, index, text, capacity); }
int st_vst2_parameter_label(STVST2Instance *instance, int32_t index, char *text, size_t capacity) { return parameter_text(instance, effGetParamLabel, index, text, capacity); }
int st_vst2_parameter_display(STVST2Instance *instance, int32_t index, char *text, size_t capacity) { return parameter_text(instance, effGetParamDisplay, index, text, capacity); }

float st_vst2_parameter_value(STVST2Instance *instance, int32_t index) {
    if (!instance || index < 0 || index >= instance->numParams) return 0;
    return instance->values[index].load(std::memory_order_relaxed);
}

int st_vst2_set_parameter(STVST2Instance *instance, int32_t index, float value) {
    if (!instance || !instance->effect || index < 0 || index >= instance->numParams || !std::isfinite(value)) return 0;
    value = std::max(0.f, std::min(1.f, value));
    instance->effect->setParameter(instance->effect, index, value);
    instance->values[index].store(value, std::memory_order_relaxed);
    return 1;
}

int st_vst2_take_changed_parameters(STVST2Instance *instance, int32_t *indices, int32_t capacity) {
    if (!instance) return 0;
    int32_t count = 0;
    for (int32_t i = 0; i < instance->numParams; ++i) {
        if (instance->changed[i].exchange(0, std::memory_order_acq_rel)) {
            if (indices && count < capacity) indices[count] = i;
            ++count;
        }
    }
    return count;
}

void st_vst2_refresh_parameters(STVST2Instance *instance) {
    if (!instance || !instance->effect) return;
    for (int32_t i = 0; i < instance->numParams; ++i) {
        float value = instance->effect->getParameter(instance->effect, i);
        if (std::isfinite(value)) instance->values[i].store(value, std::memory_order_relaxed);
    }
}

uint32_t st_vst2_take_host_flags(STVST2Instance *instance) { return instance ? instance->hostFlags.exchange(0) : 0; }

int st_vst2_program_count(STVST2Instance *instance) { return instance && instance->effect ? instance->effect->numPrograms : 0; }
int st_vst2_program(STVST2Instance *instance) { return instance && instance->effect ? (int)dispatch(instance->effect, effGetProgram) : 0; }
int st_vst2_set_program(STVST2Instance *instance, int32_t index) {
    if (!instance || !instance->effect || index < 0 || index >= instance->effect->numPrograms) return 0;
    dispatch(instance->effect, effSetProgram, 0, index);
    st_vst2_refresh_parameters(instance);
    return 1;
}

int st_vst2_process(STVST2Instance *instance, const float *input_left, const float *input_right,
                    float *output_left, float *output_right, uint32_t frames,
                    const STVST2MIDI *midi, uint32_t midi_count,
                    const STVST2ParameterChange *parameters, uint32_t parameter_count,
                    const STVST2Transport *transport) {
    if (!instance || !instance->effect || !output_left || !output_right || frames > instance->maximumFrames ||
        midi_count > kEventCapacity || parameter_count > kEventCapacity ||
        (midi_count && !midi) || (parameter_count && !parameters)) return 0;
    if (frames == 0) return midi_count == 0 && parameter_count == 0;
    // Refuse malformed deadlines before advancing vendor DSP or applying edits.
    for (uint32_t i=0;i<parameter_count;++i) {
        if (parameters[i].sample_offset < 0 || (uint32_t)parameters[i].sample_offset >= frames) return 0;
        instance->parameterOrder[i]=i;
    }
    for (uint32_t i=0;i<midi_count;++i)
        if (midi[i].sample_offset < 0 || (uint32_t)midi[i].sample_offset >= frames) return 0;
    std::sort(instance->parameterOrder.begin(),instance->parameterOrder.begin()+parameter_count,
        [parameters](uint32_t a,uint32_t b) {
            return parameters[a].sample_offset == parameters[b].sample_offset ? a < b
                : parameters[a].sample_offset < parameters[b].sample_offset;
        });
    Effect *effect = instance->effect;
    if (!instance->active) { std::memset(output_left, 0, frames * 4); std::memset(output_right, 0, frames * 4); return 1; }
    audioThread = true;
    TimeInfo &time = instance->time;
    time.sampleRate = instance->sampleRate;
    int32_t flags = kTempoValid | kTimeSigValid;
    if (transport) {
        time.samplePos = transport->sample_position;
        if (transport->tempo > 0 && std::isfinite(transport->tempo)) time.tempo = transport->tempo;
        if (transport->numerator > 0) time.timeSigNumerator = transport->numerator;
        if (transport->denominator > 0) time.timeSigDenominator = transport->denominator;
        if (transport->musical_valid) {
            time.ppqPos = transport->beat_position; time.barStartPos = transport->bar_start_beat;
            flags |= kPpqPosValid | kBarsValid;
        }
        if (transport->cycling) {
            time.cycleStartPos = transport->cycle_start_beat; time.cycleEndPos = transport->cycle_end_beat;
            flags |= kCyclePosValid | kTransportCycleActive;
        }
        bool wasPlaying = (time.flags & kTransportPlaying) != 0;
        if (transport->playing) flags |= kTransportPlaying;
        if (transport->recording) flags |= kTransportRecording;
        if (wasPlaying != (transport->playing != 0)) flags |= kTransportChanged;
    }
    time.flags = flags;
    for (int32_t i = 0; i < instance->numInputs; ++i) {
        const float *source = i == 0 ? input_left : i == 1 ? (input_right ? input_right : input_left) : nullptr;
        if (instance->numInputs == 1 && input_left && input_right) {
            for (uint32_t f = 0; f < frames; ++f) instance->inputs[0][f] = 0.5f * (input_left[f] + input_right[f]);
        } else if (source) std::memcpy(instance->inputs[i], source, frames * 4);
        else std::memset(instance->inputs[i], 0, frames * 4);
    }
    for (int32_t i = 0; i < instance->numOutputs; ++i) std::memset(instance->outputs[i], 0, frames * 4);
    const double initialSample=time.samplePos, initialBeat=time.ppqPos, initialBar=time.barStartPos;
    const double beatsPerSample=time.tempo/(60.0*instance->sampleRate);
    uint32_t cursor=0, nextParameter=0;
    while (cursor<frames) {
        while (nextParameter<parameter_count && parameters[instance->parameterOrder[nextParameter]].sample_offset==(int32_t)cursor) {
            const auto &change=parameters[instance->parameterOrder[nextParameter++]];
            if (change.index>=0 && change.index<instance->numParams && std::isfinite(change.value)) {
                const float value=std::max(0.f,std::min(1.f,change.value));
                effect->setParameter(effect,change.index,value);
                instance->values[change.index].store(value,std::memory_order_relaxed);
            }
        }
        const uint32_t end=nextParameter<parameter_count
            ? (uint32_t)parameters[instance->parameterOrder[nextParameter]].sample_offset : frames;
        time.samplePos=initialSample+cursor;
        if (time.flags & kPpqPosValid) {
            time.ppqPos=initialBeat+cursor*beatsPerSample;
            const double barBeats=time.timeSigNumerator*4.0/time.timeSigDenominator;
            if ((time.flags & kBarsValid) && barBeats>0)
                time.barStartPos=initialBar+std::floor((time.ppqPos-initialBar)/barBeats)*barBeats;
        }
        Events *events=reinterpret_cast<Events *>(instance->eventsBlock.data());
        uint32_t count=0;
        for (uint32_t i=0;i<midi_count;++i) {
            if ((uint32_t)midi[i].sample_offset<cursor || (uint32_t)midi[i].sample_offset>=end) continue;
            if (count>=instance->midiEvents.size()) break; // Panic note-offs of re-held notes can fill the buffer before the input ends.
            MidiEvent &event=instance->midiEvents[count];std::memset(&event,0,sizeof event);
            event.type=kMidiType;event.byteSize=sizeof(MidiEvent);
            event.deltaFrames=midi[i].sample_offset-(int32_t)cursor;event.flags=kMidiEventIsRealtime;
            event.midiData[0]=(char)midi[i].status;event.midiData[1]=(char)midi[i].data1;event.midiData[2]=(char)midi[i].data2;
            events->events[count++]=reinterpret_cast<Event *>(&event);
            const int type=midi[i].status&0xf0, channel=midi[i].status&0x0f, pitch=midi[i].data1&127;
            if (type==0x90 && midi[i].data2) instance->heldNotes[channel][pitch]=1;
            else if (type==0x80 || type==0x90) instance->heldNotes[channel][pitch]=0;
            else if (type==0xb0 && (midi[i].data1==120 || midi[i].data1==123)) {
                for (int note=0;note<128;++note) if (instance->heldNotes[channel][note] && count<instance->midiEvents.size()) {
                    MidiEvent &off=instance->midiEvents[count];std::memset(&off,0,sizeof off);
                    off.type=kMidiType;off.byteSize=sizeof(MidiEvent);off.deltaFrames=event.deltaFrames;off.flags=kMidiEventIsRealtime;
                    off.midiData[0]=(char)(0x80|channel);off.midiData[1]=(char)note;
                    events->events[count++]=reinterpret_cast<Event *>(&off);instance->heldNotes[channel][note]=0;
                }
            }
        }
        if (count) { events->numEvents=(int32_t)count;events->reserved=0;dispatch(effect,effProcessEvents,0,0,events); }
        for (int32_t i=0;i<instance->numInputs;++i) instance->inputs[i]=instance->inputStorage.data()+(size_t)i*kMaximumFrames+cursor;
        for (int32_t i=0;i<instance->numOutputs;++i) instance->outputs[i]=instance->outputStorage.data()+(size_t)i*kMaximumFrames+cursor;
        if ((effect->flags & kFlagCanReplacing) && effect->processReplacing)
            effect->processReplacing(effect,instance->inputs.data(),instance->outputs.data(),(int32_t)(end-cursor));
        else if (effect->process) effect->process(effect,instance->inputs.data(),instance->outputs.data(),(int32_t)(end-cursor));
        cursor=end;
        time.flags &= ~kTransportChanged;
    }
    for (int32_t i=0;i<instance->numInputs;++i) instance->inputs[i]=instance->inputStorage.data()+(size_t)i*kMaximumFrames;
    for (int32_t i=0;i<instance->numOutputs;++i) instance->outputs[i]=instance->outputStorage.data()+(size_t)i*kMaximumFrames;
    std::memcpy(output_left,instance->outputs[0],frames*sizeof(float));
    std::memcpy(output_right,instance->numOutputs>1?instance->outputs[1]:instance->outputs[0],frames*sizeof(float));
    time.samplePos=initialSample+frames;
    audioThread = false;
    return 1;
}

int st_vst2_reset(STVST2Instance *instance) {
    if (!instance || !instance->effect) return 0;
    if (instance->active) {
        // Release held notes explicitly, then restart processing so voices and tails clear.
        STVST2MIDI panic[16];float left=0,right=0;
        for (int channel=0;channel<16;++channel) panic[channel]={0,(uint8_t)(0xb0|channel),123,0};
        st_vst2_process(instance,nullptr,nullptr,&left,&right,1,panic,16,nullptr,0,nullptr);
        dispatch(instance->effect, effStopProcess); dispatch(instance->effect, effStartProcess);
    }
    for (auto &channel : instance->heldNotes) channel.fill(0);
    return 1;
}

int st_vst2_copy_state(STVST2Instance *instance, void **bytes, size_t *size, char *error, size_t error_size) {
    if (!instance || !instance->effect || !bytes || !size) { write_error(error, error_size, "No VST2 plug-in is loaded."); return 0; }
    *bytes = nullptr; *size = 0;
    Effect *effect = instance->effect;
    StateHeader header{{'S', 'T', 'V', '2'}, 1, effect->uniqueID, 2, (int32_t)dispatch(effect, effGetProgram), 0};
    const void *payload = nullptr;
    std::vector<float> parameters;
    if (effect->flags & kFlagProgramChunks) {
        void *chunk = nullptr;
        IntPtr length = dispatch(effect, effGetChunk, 0, 0, &chunk);
        if (length <= 0 || !chunk || length > 256 * 1024 * 1024) {
            write_error(error, error_size, "The VST2 plug-in did not provide its required chunk state.");
            return 0;
        }
        header.kind = 1; payload = chunk; header.payloadBytes = (uint32_t)length;
    }
    if (header.kind == 2) {
        parameters.resize(instance->numParams);
        for (int32_t i = 0; i < instance->numParams; ++i) parameters[i] = effect->getParameter(effect, i);
        payload = parameters.data(); header.payloadBytes = (uint32_t)(parameters.size() * sizeof(float));
    }
    size_t total = sizeof header + header.payloadBytes;
    uint8_t *result = (uint8_t *)std::malloc(total);
    if (!result) { write_error(error, error_size, "Out of memory while saving the VST2 state."); return 0; }
    std::memcpy(result, &header, sizeof header);
    if (header.payloadBytes) std::memcpy(result + sizeof header, payload, header.payloadBytes);
    *bytes = result; *size = total;
    return 1;
}

static void apply_parameters(STVST2Instance *instance, const float *values, size_t count) {
    Effect *effect = instance->effect;
    for (size_t i = 0; i < count && (int32_t)i < instance->numParams; ++i)
        if (std::isfinite(values[i])) effect->setParameter(effect, (int32_t)i, std::max(0.f, std::min(1.f, values[i])));
}

int st_vst2_restore_state(STVST2Instance *instance, const void *bytes, size_t size, char *error, size_t error_size) {
    if (!instance || !instance->effect || !bytes || size < sizeof(StateHeader)) { write_error(error, error_size, "The saved VST2 state is invalid."); return 0; }
    StateHeader header;
    std::memcpy(&header, bytes, sizeof header);
    if (std::memcmp(header.magic, "STV2", 4) != 0 || header.version != 1 || header.payloadBytes != size - sizeof header) {
        write_error(error, error_size, "The saved VST2 state is invalid."); return 0;
    }
    Effect *effect = instance->effect;
    if (header.uniqueID != effect->uniqueID) { write_error(error, error_size, "The saved VST2 state belongs to a different plug-in."); return 0; }
    const uint8_t *payload = (const uint8_t *)bytes + sizeof header;
    if (header.kind == 1) {
        if (!(effect->flags & kFlagProgramChunks)) { write_error(error, error_size, "This VST2 plug-in no longer accepts chunk state."); return 0; }
        dispatch(effect, effBeginLoadBank);
        dispatch(effect, effSetChunk, 0, (IntPtr)header.payloadBytes, (void *)payload);
        // copy_state saves the selected program independently of its bank. A
        // bank load may reset selection, so restore it only after the chunk.
        if (header.program >= 0 && header.program < effect->numPrograms)
            dispatch(effect, effSetProgram, 0, header.program);
    } else if (header.kind == 2) {
        if (header.program >= 0 && header.program < effect->numPrograms) dispatch(effect, effSetProgram, 0, header.program);
        apply_parameters(instance, (const float *)payload, header.payloadBytes / sizeof(float));
    } else { write_error(error, error_size, "The saved VST2 state uses an unknown format."); return 0; }
    st_vst2_refresh_parameters(instance);
    return 1;
}

int st_vst2_restore_chunk(STVST2Instance *instance, const void *bytes, size_t size, int preset, char *error, size_t error_size) {
    if (!instance || !instance->effect || !bytes || size == 0 || size > 256 * 1024 * 1024) { write_error(error, error_size, "The VST2 preset state is invalid."); return 0; }
    Effect *effect = instance->effect;
    const uint8_t *data = (const uint8_t *)bytes;
    // Standard FXP/FXB containers (big-endian): CcnK, size, kind, version, id, version, count/...
    if (size >= 28 && std::memcmp(data, "CcnK", 4) == 0) {
        uint32_t kind = be32(data + 8);
        int32_t identifier = (int32_t)be32(data + 16);
        if (identifier != effect->uniqueID) { write_error(error, error_size, "The preset belongs to a different VST2 plug-in."); return 0; }
        if (kind == 0x4678436Bu && size >= 56) {
            uint32_t count = be32(data + 24);
            for (uint32_t i = 0; i < count && 56 + (size_t)i * 4 + 4 <= size && (int32_t)i < instance->numParams; ++i)
                effect->setParameter(effect, (int32_t)i, befloat(data + 56 + i * 4));
        } else if (kind == 0x46504368u && size >= 60) {
            uint32_t length = be32(data + 56);
            if (60 + (size_t)length > size) { write_error(error, error_size, "The VST2 program chunk is truncated."); return 0; }
            dispatch(effect, effBeginLoadProgram);
            dispatch(effect, effSetChunk, 1, (IntPtr)length, (void *)(data + 60));
        } else if (kind == 0x46424368u && size >= 160) {
            uint32_t length = be32(data + 156);
            if (160 + (size_t)length > size) { write_error(error, error_size, "The VST2 bank chunk is truncated."); return 0; }
            dispatch(effect, effBeginLoadBank);
            dispatch(effect, effSetChunk, 0, (IntPtr)length, (void *)(data + 160));
        } else { write_error(error, error_size, "This VST2 preset container is not supported."); return 0; }
        st_vst2_refresh_parameters(instance);
        return 1;
    }
    if (effect->flags & kFlagProgramChunks) {
        dispatch(effect, preset ? effBeginLoadProgram : effBeginLoadBank);
        dispatch(effect, effSetChunk, preset ? 1 : 0, (IntPtr)size, (void *)bytes);
    } else if (size == (size_t)instance->numParams * sizeof(float)) {
        apply_parameters(instance, (const float *)bytes, size / sizeof(float));
    } else { write_error(error, error_size, "This VST2 plug-in does not accept opaque preset data."); return 0; }
    st_vst2_refresh_parameters(instance);
    return 1;
}

void st_vst2_free(void *bytes) { std::free(bytes); }

int st_vst2_attach_editor(STVST2Instance *instance, void *parent_view, int *width, int *height, char *error, size_t error_size) {
    if (!instance || !instance->effect || !parent_view) { write_error(error, error_size, "No VST2 plug-in is loaded."); return 0; }
    Effect *effect = instance->effect;
    if (!(effect->flags & kFlagHasEditor)) { write_error(error, error_size, "This VST2 plug-in has no editor."); return 0; }
    if (instance->editorOpen) return 1;
    vst2::ERect *rect = nullptr;
    dispatch(effect, effEditGetRect, 0, 0, &rect);
    if (!dispatch(effect, effEditOpen, 0, 0, parent_view)) {
        // Several plug-ins return zero despite opening successfully; verify a subview appeared.
        NSView *view = (__bridge NSView *)parent_view;
        if (view.subviews.count == 0) { write_error(error, error_size, "The VST2 editor could not be opened."); return 0; }
    }
    instance->editorOpen = true;
    rect = nullptr;
    dispatch(effect, effEditGetRect, 0, 0, &rect);
    int w = 640, h = 420;
    if (rect && rect->right > rect->left && rect->bottom > rect->top) { w = rect->right - rect->left; h = rect->bottom - rect->top; }
    if (width) *width = w;
    if (height) *height = h;
    return 1;
}

void st_vst2_detach_editor(STVST2Instance *instance) {
    if (!instance || !instance->effect || !instance->editorOpen) return;
    dispatch(instance->effect, effEditClose);
    instance->editorOpen = false;
}

int st_vst2_editor_idle(STVST2Instance *instance, int *width, int *height) {
    if (!instance || !instance->effect || !instance->editorOpen) return 0;
    dispatch(instance->effect, effEditIdle);
    if (instance->sizeRequested.exchange(0)) {
        if (width) *width = instance->requestedWidth;
        if (height) *height = instance->requestedHeight;
        return 1;
    }
    return 0;
}

} // extern "C"
