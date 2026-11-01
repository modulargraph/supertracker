// Original VST3 processor probe. No vendor code, devices, window or file IO.
#define INIT_CLASS_IID
#include "vst3_timing_fixture.h"
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstring>
#include <limits>

using namespace Steinberg;
using namespace Steinberg::Vst;

static_assert(sizeof(STTimingFixtureSnapshot) == 88);
static_assert(offsetof(STTimingFixtureSnapshot, contextPresent) == 80);
static_assert(ST_TIMING_PLAYING == uint32_t(ProcessContext::kPlaying));
static_assert(ST_TIMING_CYCLE_ACTIVE == uint32_t(ProcessContext::kCycleActive));
static_assert(ST_TIMING_RECORDING == uint32_t(ProcessContext::kRecording));
static_assert(ST_TIMING_PROJECT_MUSIC_VALID == uint32_t(ProcessContext::kProjectTimeMusicValid));
static_assert(ST_TIMING_TEMPO_VALID == uint32_t(ProcessContext::kTempoValid));
static_assert(ST_TIMING_BAR_VALID == uint32_t(ProcessContext::kBarPositionValid));
static_assert(ST_TIMING_CYCLE_VALID == uint32_t(ProcessContext::kCycleValid));
static_assert(ST_TIMING_TIME_SIG_VALID == uint32_t(ProcessContext::kTimeSigValid));
static_assert(ST_TIMING_CONTINUOUS_VALID == uint32_t(ProcessContext::kContTimeValid));
static_assert(std::atomic<uint64_t>::is_always_lock_free);

namespace {
std::array<STTimingFixtureSnapshot, ST_TIMING_FIXTURE_CAPACITY> snapshots{};
std::atomic<uint64_t> capturedCalls{0};
std::atomic<uint32_t> configuredDelay{0}, liveComponents{0};
constexpr uint32_t stateMagic = 0x5354544d;

bool same(const TUID value, const FUID& expected) { return FUnknownPrivate::iidEqual(value, expected); }
FUID classID() { FUID id; id.fromString(ST_TIMING_FIXTURE_CLASS_ID); return id; }
void busName(String128 out, const char16_t *text) {
    std::fill(out, out + 128, 0);
    for (size_t index = 0; index < 127 && text[index]; ++index) out[index] = text[index];
}

void capture(const ProcessData& data) {
    // The reader is quiescent. Atomic reservation also keeps separate original
    // instances from writing the same slot if the host renders in parallel.
    const uint64_t index = capturedCalls.fetch_add(1, std::memory_order_relaxed);
    if (index >= snapshots.size()) return;
    STTimingFixtureSnapshot value{};
    value.processFrames = uint32_t(std::max(0, data.numSamples));
    if (const auto *context = data.processContext) {
        value.contextPresent = 1;
        value.state = context->state;
        value.sampleRate = context->sampleRate;
        value.projectSamples = context->projectTimeSamples;
        value.continuousSamples = context->continousTimeSamples;
        value.musicalBeat = context->projectTimeMusic;
        value.tempo = context->tempo;
        value.numerator = context->timeSigNumerator;
        value.denominator = context->timeSigDenominator;
        value.barBeat = context->barPositionMusic;
        value.cycleStartBeat = context->cycleStartMusic;
        value.cycleEndBeat = context->cycleEndMusic;
    }
    snapshots[size_t(index)] = value;
}

class Component final : public IComponent, public IAudioProcessor, public IEditController {
    std::atomic<uint32> refs{1};
    const uint32_t delayFrames;
    std::array<float, 127> delay{};
    std::array<std::array<bool, 128>, 16> held{};
    uint32_t cursor = 0, heldCount = 0;
    void clearAudio() {
        delay.fill(0.f);
        for (auto& channel : held) channel.fill(false);
        cursor = heldCount = 0;
    }
    void noteEvent(const Event& event) {
        const bool on = event.type == Event::kNoteOnEvent && event.noteOn.velocity > 0;
        if (event.type != Event::kNoteOnEvent && event.type != Event::kNoteOffEvent) return;
        const int channel = event.type == Event::kNoteOnEvent ? event.noteOn.channel : event.noteOff.channel;
        const int pitch = event.type == Event::kNoteOnEvent ? event.noteOn.pitch : event.noteOff.pitch;
        if (channel < 0 || channel >= 16 || pitch < 0 || pitch >= 128) return;
        auto& active = held[size_t(channel)][size_t(pitch)];
        if (active != on) { if (on) ++heldCount; else --heldCount; active = on; }
    }
public:
    Component() : delayFrames(configuredDelay.load(std::memory_order_relaxed)) { liveComponents.fetch_add(1); }
    ~Component() { liveComponents.fetch_sub(1); }
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        if (!object) return kInvalidArgument;
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IComponent::iid) || same(iid, IPluginBase::iid)) *object = static_cast<IComponent *>(this);
        else if (same(iid, IAudioProcessor::iid)) *object = static_cast<IAudioProcessor *>(this);
        else if (same(iid, IEditController::iid)) *object = static_cast<IEditController *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API initialize(FUnknown *) override { return kResultOk; }
    tresult PLUGIN_API terminate() override { return kResultOk; }
    tresult PLUGIN_API getControllerClassId(TUID) override { return kNotImplemented; }
    tresult PLUGIN_API setIoMode(IoMode) override { return kResultOk; }
    int32 PLUGIN_API getBusCount(MediaType media, BusDirection direction) override {
        return (media == kAudio && direction == kOutput) || (media == kEvent && direction == kInput) ? 1 : 0;
    }
    tresult PLUGIN_API getBusInfo(MediaType media, BusDirection direction, int32 index, BusInfo& info) override {
        if (index != 0 || !getBusCount(media, direction)) return kInvalidArgument;
        info = {}; info.mediaType = media; info.direction = direction;
        info.channelCount = media == kAudio ? 2 : 16;
        info.busType = kMain; info.flags = BusInfo::kDefaultActive;
        busName(info.name, media == kAudio ? u"Timing Probe Stereo" : u"Timing Probe Notes");
        return kResultOk;
    }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&, RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType media, BusDirection direction, int32 index, TBool) override {
        return index == 0 && getBusCount(media, direction) ? kResultOk : kInvalidArgument;
    }
    tresult PLUGIN_API setActive(TBool active) override { if (!active) clearAudio(); return kResultOk; }
    tresult PLUGIN_API setState(IBStream *stream) override {
        if (!stream) return kInvalidArgument;
        uint32_t magic = 0; int32 read = 0;
        if (stream->read(&magic, sizeof(magic), &read) != kResultOk || read != sizeof(magic) || magic != stateMagic) return kResultFalse;
        clearAudio(); return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream *stream) override { return stream ? stream->write(const_cast<uint32_t *>(&stateMagic), sizeof(stateMagic), nullptr) : kInvalidArgument; }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32 inputCount, SpeakerArrangement *outputs, int32 outputCount) override {
        return inputCount == 0 && outputCount == 1 && outputs && outputs[0] == SpeakerArr::kStereo ? kResultOk : kResultFalse;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection direction, int32 index, SpeakerArrangement& arrangement) override {
        if (direction != kOutput || index != 0) return kInvalidArgument;
        arrangement = SpeakerArr::kStereo; return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return delayFrames; }
    tresult PLUGIN_API setupProcessing(ProcessSetup& setup) override { return setup.symbolicSampleSize == kSample32 ? kResultOk : kResultFalse; }
    tresult PLUGIN_API setProcessing(TBool processing) override { if (!processing) clearAudio(); return kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return delayFrames; }
    tresult PLUGIN_API process(ProcessData& data) override {
        capture(data);
        if (data.numSamples < 0 || data.symbolicSampleSize != kSample32 || data.numOutputs != 1 || !data.outputs || data.outputs[0].numChannels != 2 || !data.outputs[0].channelBuffers32 || !data.outputs[0].channelBuffers32[0] || !data.outputs[0].channelBuffers32[1]) return kResultFalse;
        auto& output = data.outputs[0];
        bool silent = true;
        for (int32 sample = 0; sample < data.numSamples; ++sample) {
            if (data.inputEvents) for (int32 index = 0; index < data.inputEvents->getEventCount(); ++index) {
                Event event{};
                if (data.inputEvents->getEvent(index, event) == kResultOk && event.sampleOffset == sample) noteEvent(event);
            }
            float value = heldCount ? .25f : 0.f;
            if (delayFrames) {
                const float previous = delay[cursor];
                delay[cursor] = value;
                cursor = (cursor + 1) % delayFrames;
                value = previous;
            }
            output.channelBuffers32[0][sample] = output.channelBuffers32[1][sample] = value;
            silent = silent && value == 0.f;
        }
        output.silenceFlags = silent ? 3 : 0;
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream *) override { return kResultOk; }
    int32 PLUGIN_API getParameterCount() override { return 0; }
    tresult PLUGIN_API getParameterInfo(int32, ParameterInfo&) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamStringByValue(ParamID, ParamValue, String128) override { return kInvalidArgument; }
    tresult PLUGIN_API getParamValueByString(ParamID, TChar *, ParamValue&) override { return kInvalidArgument; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override { return 0; }
    tresult PLUGIN_API setParamNormalized(ParamID, ParamValue) override { return kInvalidArgument; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler *) override { return kResultOk; }
    IPlugView *PLUGIN_API createView(FIDString) override { return nullptr; }
};

class Factory final : public IPluginFactory2 {
    std::atomic<uint32> refs{1};
public:
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        if (!object) return kInvalidArgument;
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IPluginFactory::iid) || same(iid, IPluginFactory2::iid)) *object = static_cast<IPluginFactory2 *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo *info) override { if (!info) return kInvalidArgument; *info = {}; std::strcpy(info->vendor, "SuperTracker Tests"); return kResultOk; }
    int32 PLUGIN_API countClasses() override { return 1; }
    tresult PLUGIN_API getClassInfo(int32 index, PClassInfo *info) override {
        if (index != 0 || !info) return kInvalidArgument;
        *info = {}; classID().toTUID(info->cid);
        std::strcpy(info->name, "SuperTracker Timing Probe"); std::strcpy(info->category, kVstAudioEffectClass);
        info->cardinality = PClassInfo::kManyInstances; return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid, FIDString iid, void **object) override {
        if (!object) return kInvalidArgument;
        *object = nullptr;
        if (!same(cid, classID())) return kNoInterface;
        auto component = new Component;
        const auto result = component->queryInterface(iid, object);
        component->release(); return result;
    }
    tresult PLUGIN_API getClassInfo2(int32 index, PClassInfo2 *info) override {
        if (index != 0 || !info) return kInvalidArgument;
        *info = {}; classID().toTUID(info->cid);
        std::strcpy(info->name, "SuperTracker Timing Probe"); std::strcpy(info->category, kVstAudioEffectClass);
        std::strcpy(info->subCategories, "Instrument|Synth"); std::strcpy(info->vendor, "SuperTracker Tests");
        std::strcpy(info->version, "1.0.0"); info->cardinality = PClassInfo::kManyInstances; return kResultOk;
    }
};
}

#define ST_TIMING_EXPORT extern "C" __attribute__((visibility("default")))
ST_TIMING_EXPORT void st_timing_fixture_reset(void) { capturedCalls.store(0, std::memory_order_relaxed); }
ST_TIMING_EXPORT uint32_t st_timing_fixture_count(void) { return uint32_t(std::min<uint64_t>(capturedCalls.load(std::memory_order_relaxed), snapshots.size())); }
ST_TIMING_EXPORT uint32_t st_timing_fixture_dropped(void) {
    const uint64_t count = capturedCalls.load(std::memory_order_relaxed);
    return uint32_t(std::min<uint64_t>(count > snapshots.size() ? count - snapshots.size() : 0, std::numeric_limits<uint32_t>::max()));
}
ST_TIMING_EXPORT int st_timing_fixture_copy(uint32_t index, STTimingFixtureSnapshot *out) {
    if (!out || index >= st_timing_fixture_count()) return 0;
    *out = snapshots[index]; return 1;
}
ST_TIMING_EXPORT int st_timing_fixture_set_delay(uint32_t frames) {
    if ((frames != 0 && frames != 127) || liveComponents.load(std::memory_order_relaxed) != 0) return 0;
    configuredDelay.store(frames, std::memory_order_relaxed); return 1;
}
ST_TIMING_EXPORT bool bundleEntry(void *) { return true; }
ST_TIMING_EXPORT bool bundleExit() { return true; }
ST_TIMING_EXPORT IPluginFactory *GetPluginFactory() { return new Factory; }
