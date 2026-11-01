// Original asynchronous-state fixture. No installed vendor code is linked or loaded.
// The audio class ID and bundle version deliberately exercise the host's exact
// state-capture compatibility profile; this is never a vendor implementation.
#define INIT_CLASS_IID
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include <CoreFoundation/CoreFoundation.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>

using namespace Steinberg;
using namespace Steinberg::Vst;
namespace {
constexpr const char *audioClass = "5653544E6924486D6173736976652078";
constexpr const char *controllerClass = "5343544553544341505455524543544C";
constexpr const char *instrumentName = "SuperTracker Async Capture Instrument";
constexpr ParamID gainID = 42;
struct State { uint32 magic = 0x53434346; uint32 version = 1; double gain = .25; };
static_assert(sizeof(State) == 16);
static_assert(std::atomic<double>::is_always_lock_free && std::atomic<uint64>::is_always_lock_free);
bool same(const TUID a, const FUID& b) { return FUnknownPrivate::iidEqual(a, b); }
FUID uid(const char *text) { FUID value; value.fromString(text); return value; }
bool readState(IBStream *stream, State& state) {
    int32 bytes = 0;
    return stream && stream->read(&state, sizeof(state), &bytes) == kResultOk && bytes == sizeof(state)
        && state.magic == 0x53434346 && state.version == 1 && std::isfinite(state.gain) && state.gain >= 0 && state.gain <= 1;
}
void title(String128 target, const char16_t *source) {
    std::fill(target, target + 128, 0);
    for (int n = 0; source[n] && n < 127; ++n) target[n] = source[n];
}

class Component final : public IComponent, public IAudioProcessor {
    std::atomic<uint32> refs{1};
    // Audio-thread writes only the live model and atomics. Durable state and its
    // revision are owned by the main run loop, as are getState/setState/lifecycle.
    double liveGain = .25, durableGain = .25;
    std::atomic<double> pendingGain{.25};
    std::atomic<uint64> pendingRevision{0};
    uint64 durableRevision = 0;
    CFRunLoopTimerRef timer = nullptr;
    std::array<std::array<bool, 128>, 16> held{};
    uint32 heldCount = 0;
    bool setupDone = false, active = false, processing = false;
    void stopTimer() {
        if (timer) { CFRunLoopTimerInvalidate(timer); CFRelease(timer); timer = nullptr; }
    }
    static void commit(CFRunLoopTimerRef, void *context) {
        auto& self = *static_cast<Component *>(context);
        const auto revision = self.pendingRevision.load(std::memory_order_acquire);
        if (revision != self.durableRevision) {
            self.durableGain = self.pendingGain.load(std::memory_order_relaxed);
            self.durableRevision = revision;
        }
    }
    void note(const Event& event) {
        if (event.type != Event::kNoteOnEvent && event.type != Event::kNoteOffEvent) return;
        const int channel = event.type == Event::kNoteOnEvent ? event.noteOn.channel : event.noteOff.channel;
        const int pitch = event.type == Event::kNoteOnEvent ? event.noteOn.pitch : event.noteOff.pitch;
        if (channel < 0 || channel >= 16 || pitch < 0 || pitch >= 128) return;
        const bool on = event.type == Event::kNoteOnEvent && event.noteOn.velocity > 0;
        if (on != held[channel][pitch]) { held[channel][pitch] = on; if (on) ++heldCount; else --heldCount; }
    }
public:
    ~Component() { stopTimer(); }
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IPluginBase::iid) || same(iid, IComponent::iid)) *object = static_cast<IComponent *>(this);
        else if (same(iid, IAudioProcessor::iid)) *object = static_cast<IAudioProcessor *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API initialize(FUnknown *) override {
        if (timer) return kResultFalse;
        CFRunLoopTimerContext context{}; context.info = this;
        timer = CFRunLoopTimerCreate(kCFAllocatorDefault, CFAbsoluteTimeGetCurrent() + .01, .01, 0, 0, commit, &context);
        if (!timer) return kResultFalse;
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, kCFRunLoopCommonModes);
        return kResultOk;
    }
    tresult PLUGIN_API terminate() override { stopTimer(); return kResultOk; }
    tresult PLUGIN_API getControllerClassId(TUID result) override { uid(controllerClass).toTUID(result); return kResultOk; }
    tresult PLUGIN_API setIoMode(IoMode) override { return kResultOk; }
    int32 PLUGIN_API getBusCount(MediaType media, BusDirection direction) override {
        return (media == kAudio && direction == kOutput) || (media == kEvent && direction == kInput) ? 1 : 0;
    }
    tresult PLUGIN_API getBusInfo(MediaType media, BusDirection direction, int32 index, BusInfo& info) override {
        if (index != 0 || !getBusCount(media, direction)) return kInvalidArgument;
        info = {}; info.mediaType = media; info.direction = direction;
        info.channelCount = media == kAudio ? 2 : 16; info.busType = kMain; info.flags = BusInfo::kDefaultActive;
        title(info.name, media == kAudio ? u"Stereo Output" : u"Note Input"); return kResultOk;
    }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&, RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType media, BusDirection direction, int32 index, TBool) override {
        return index == 0 && getBusCount(media, direction) && setupDone && !active && !processing ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API setActive(TBool value) override { if (value && !setupDone) return kResultFalse; active = value; return kResultOk; }
    tresult PLUGIN_API setState(IBStream *stream) override {
        State state; if (!readState(stream, state)) return kResultFalse;
        liveGain = durableGain = state.gain;
        pendingGain.store(state.gain, std::memory_order_relaxed);
        durableRevision = pendingRevision.fetch_add(1, std::memory_order_release) + 1;
        for (auto& channel : held) channel.fill(false); heldCount = 0;
        return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream *stream) override {
        State state; state.gain = durableGain; return stream->write(&state, sizeof(state), nullptr);
    }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32 inputs, SpeakerArrangement *outputs, int32 count) override {
        return inputs == 0 && count == 1 && outputs && outputs[0] == SpeakerArr::kStereo ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection direction, int32 index, SpeakerArrangement& result) override {
        if (direction != kOutput || index != 0) return kInvalidArgument;
        result = SpeakerArr::kStereo; return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { setupDone = true; return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool value) override { if (value && !active) return kResultFalse; processing = value; return kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (!processing || data.symbolicSampleSize != kSample32 || data.numSamples < 0 || data.numOutputs != 1 || data.outputs[0].numChannels != 2) return kResultFalse;
        struct Point { int32 offset; double value; };
        std::array<Point, 256> points{}; size_t count = 0, next = 0;
        if (data.inputParameterChanges) for (int32 n = 0; n < data.inputParameterChanges->getParameterCount(); ++n) {
            auto queue = data.inputParameterChanges->getParameterData(n);
            if (!queue || queue->getParameterId() != gainID) continue;
            for (int32 p = 0; p < queue->getPointCount(); ++p) {
                int32 offset = 0; double value = 0;
                if (queue->getPoint(p, offset, value) != kResultOk || !std::isfinite(value) || offset < 0 || offset >= std::max(1, data.numSamples)) continue;
                if (count == points.size()) return kResultFalse;
                points[count++] = {offset, std::clamp(value, 0., 1.)};
            }
        }
        std::sort(points.begin(), points.begin() + count, [](const Point& left, const Point& right) { return left.offset < right.offset; });
        bool changed = false;
        auto parameters = [&](int32 frame) {
            while (next < count && points[next].offset == frame) { liveGain = points[next++].value; changed = true; }
        };
        if (!data.numSamples) parameters(0);
        for (int32 frame = 0; frame < data.numSamples; ++frame) {
            parameters(frame);
            if (data.inputEvents) for (int32 n = 0; n < data.inputEvents->getEventCount(); ++n) {
                Event event{}; if (data.inputEvents->getEvent(n, event) == kResultOk && event.sampleOffset == frame) note(event);
            }
            const float value = heldCount ? float(liveGain) : 0.f;
            data.outputs[0].channelBuffers32[0][frame] = data.outputs[0].channelBuffers32[1][frame] = value;
        }
        // No dispatch, timer creation, allocation, lock or wait on the audio path.
        // Both normal 512-frame callbacks and discarded 4096-frame preparation
        // work, but getState remains old until the main run loop commits this.
        if (changed && data.numSamples) {
            pendingGain.store(liveGain, std::memory_order_relaxed);
            pendingRevision.fetch_add(1, std::memory_order_release);
        }
        return kResultOk;
    }
};

class Controller final : public IEditController {
    std::atomic<uint32> refs{1}; double gain = .25;
#if ST_VST3_CAPTURE_INITIALIZATION_EDITS
    IComponentHandler *handler = nullptr;
    bool initialEditPending = false;
    void publishEdit() {
        if (!handler) return;
        handler->beginEdit(gainID);
        handler->performEdit(gainID, gain);
        handler->endEdit(gainID);
    }
#endif
public:
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IPluginBase::iid) || same(iid, IEditController::iid)) *object = static_cast<IEditController *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API initialize(FUnknown *) override {
#if ST_VST3_CAPTURE_INITIALIZATION_EDITS
        // The VST3 host supplies the handler after initialize. Retain the intent
        // and publish the initialization edit as soon as that handler is ready.
        initialEditPending = true;
#endif
        return kResultOk;
    }
    tresult PLUGIN_API terminate() override {
#if ST_VST3_CAPTURE_INITIALIZATION_EDITS
        handler = nullptr;
#endif
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream *stream) override {
        State state; if (!readState(stream, state)) return kResultFalse; gain = state.gain;
#if ST_VST3_CAPTURE_INITIALIZATION_EDITS
        publishEdit();
#endif
        return kResultOk;
    }
    tresult PLUGIN_API setState(IBStream *) override { return kNotImplemented; }
    // No independent controller snapshot can hide stale durable component state.
    tresult PLUGIN_API getState(IBStream *) override { return kNotImplemented; }
    int32 PLUGIN_API getParameterCount() override { return 1; }
    tresult PLUGIN_API getParameterInfo(int32 index, ParameterInfo& info) override {
        if (index) return kInvalidArgument;
        info = {}; info.id = gainID; info.flags = ParameterInfo::kCanAutomate; info.defaultNormalizedValue = .25;
        title(info.title, u"Gain"); title(info.shortTitle, u"Gain"); return kResultOk;
    }
    tresult PLUGIN_API getParamStringByValue(ParamID id, ParamValue value, String128 text) override {
        if (id != gainID) return kInvalidArgument;
        char formatted[32]{}; std::snprintf(formatted, sizeof(formatted), "%.3f", value);
        std::fill(text, text + 128, 0); for (int n = 0; formatted[n]; ++n) text[n] = formatted[n]; return kResultOk;
    }
    tresult PLUGIN_API getParamValueByString(ParamID, TChar *, ParamValue&) override { return kNotImplemented; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID id) override { return id == gainID ? gain : 0; }
    tresult PLUGIN_API setParamNormalized(ParamID id, ParamValue value) override { if (id != gainID || !std::isfinite(value)) return kInvalidArgument; gain = std::clamp(value, 0., 1.); return kResultOk; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler *value) override {
#if ST_VST3_CAPTURE_INITIALIZATION_EDITS
        handler = value;
        if (handler && initialEditPending) { initialEditPending = false; publishEdit(); }
#else
        (void)value;
#endif
        return kResultOk;
    }
    IPlugView *PLUGIN_API createView(FIDString) override { return nullptr; }
};

class Factory final : public IPluginFactory2 {
    std::atomic<uint32> refs{1};
public:
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IPluginFactory::iid) || same(iid, IPluginFactory2::iid)) *object = static_cast<IPluginFactory2 *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo *info) override { *info = {}; std::strcpy(info->vendor, "SuperTracker Tests"); return kResultOk; }
    int32 PLUGIN_API countClasses() override { return 2; }
    tresult PLUGIN_API getClassInfo(int32 index, PClassInfo *info) override {
        if (index < 0 || index > 1) return kInvalidArgument;
        *info = {}; uid(index ? controllerClass : audioClass).toTUID(info->cid);
        std::strcpy(info->name, index ? "SuperTracker Async Capture Controller" : instrumentName);
        std::strcpy(info->category, index ? kVstComponentControllerClass : kVstAudioEffectClass);
        info->cardinality = PClassInfo::kManyInstances; return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid, FIDString iid, void **object) override {
        *object = nullptr;
        if (same(cid, uid(audioClass))) { auto component = new Component; auto result = component->queryInterface(iid, object); component->release(); return result; }
        if (same(cid, uid(controllerClass))) { auto controller = new Controller; auto result = controller->queryInterface(iid, object); controller->release(); return result; }
        return kNoInterface;
    }
    tresult PLUGIN_API getClassInfo2(int32 index, PClassInfo2 *info) override {
        if (index < 0 || index > 1) return kInvalidArgument;
        *info = {}; uid(index ? controllerClass : audioClass).toTUID(info->cid);
        std::strcpy(info->name, index ? "SuperTracker Async Capture Controller" : instrumentName);
        std::strcpy(info->category, index ? kVstComponentControllerClass : kVstAudioEffectClass);
        std::strcpy(info->subCategories, index ? "" : "Instrument|Synth");
        std::strcpy(info->vendor, "SuperTracker Tests"); std::strcpy(info->version, "1.7.1 (Original fixture)");
        info->cardinality = PClassInfo::kManyInstances; return kResultOk;
    }
};
}
extern "C" __attribute__((visibility("default"))) bool bundleEntry(void *) { return true; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() { return true; }
extern "C" __attribute__((visibility("default"))) IPluginFactory *GetPluginFactory() { return new Factory; }
