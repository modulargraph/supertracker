// Original deterministic VST3 effect. This fixture never loads vendor code.
#define INIT_CLASS_IID
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cstdlib>
#include <cstring>

using namespace Steinberg;
using namespace Steinberg::Vst;
namespace {
constexpr const char *classIDs[] = {
    "A0B10000000000000000000000000001", // Stereo key at input index 2.
    "A0B10000000000000000000000000002", // Mono key at input index 2.
    "A0B10000000000000000000000000003", // Main input at index 1, key at index 0.
    "A0B10000000000000000000000000004", // Unsupported three-channel key.
};
constexpr const char *classNames[] = {
    "Stereo Key Fixture", "Mono Key Fixture", "Swapped Main Fixture", "Three Channel Key Fixture"
};
bool same(const TUID a, const FUID& b) { return FUnknownPrivate::iidEqual(a, b); }
FUID classID(int index) { FUID id; id.fromString(classIDs[index]); return id; }
void name(String128 target, const char16_t *source) {
    std::fill(target, target + 128, 0);
    for (int n = 0; source[n] && n < 127; ++n) target[n] = source[n];
}
struct State { uint32 magic; int32 keyChannels; };
constexpr uint32 stateMagic = 0x53434B59;

class Component final : public IComponent, public IAudioProcessor, public IEditController {
    std::atomic<uint32> refs{1};
    const int variant;
    int32 keyChannels;
    std::array<bool, 3> active{};
    bool processing = false;
    bool setupDone = false, componentActive = false;
    char activeFailureMode[32]{};
    bool activeFailureConsumed = false;
    int32 inputCount() const { return variant == 2 ? 2 : 3; }
    int32 mainIndex() const { return variant == 2 ? 1 : 0; }
    int32 keyIndex() const { return variant == 2 ? 0 : 2; }
    int32 channels(int32 index) const { return index == keyIndex() ? keyChannels : 2; }
    bool validInput(int32 index) const { return index >= 0 && index < inputCount(); }
public:
    explicit Component(int value) : variant(value), keyChannels(value == 1 ? 1 : value == 3 ? 3 : 2) {}
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
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
        return media == kAudio ? (direction == kInput ? inputCount() : 1) : 0;
    }
    tresult PLUGIN_API getBusInfo(MediaType media, BusDirection direction, int32 index, BusInfo& info) override {
        if (media != kAudio || index < 0 || index >= getBusCount(media, direction)) return kInvalidArgument;
        info = {}; info.mediaType = media; info.direction = direction;
        info.channelCount = direction == kOutput ? 2 : channels(index);
        const bool main = direction == kOutput || index == mainIndex();
        info.busType = main ? kMain : kAux;
        // Deliberately advertise auxiliary default-active. The host's explicit
        // routing policy must still keep them silent until configured.
        info.flags = BusInfo::kDefaultActive;
        name(info.name, direction == kOutput ? u"Stereo Output" : main ? u"Program Input" : index == keyIndex() ? u"Detector Key" : u"Unused Auxiliary");
        return kResultOk;
    }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&, RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType media, BusDirection direction, int32 index, TBool value) override {
        if (media != kAudio || index < 0 || index >= getBusCount(media, direction)) return kInvalidArgument;
        if (!setupDone || componentActive || processing) return kResultFalse;
        if (direction == kOutput) return kResultOk;
        if (index == keyIndex()) {
            const char *failure = std::getenv("ST_VST3_SIDECHAIN_FAIL_ACTIVATION");
            if (failure && (!std::strcmp(failure, "both") || (value && !std::strcmp(failure, "on")) || (!value && !std::strcmp(failure, "off")))) {
                // A component can mutate before reporting failure. The host
                // must restore the prior activation, not only its own cache.
                active[index] = value; return kResultFalse;
            }
        }
        active[index] = value; return kResultOk;
    }
    tresult PLUGIN_API setActive(TBool value) override {
        if (value && !setupDone) return kResultFalse;
        const char *mode = std::getenv("ST_VST3_SIDECHAIN_FAIL_SET_ACTIVE");
        if (!mode) mode = "";
        if (std::strcmp(mode, activeFailureMode)) {
            std::strncpy(activeFailureMode, mode, sizeof(activeFailureMode) - 1);
            activeFailureMode[sizeof(activeFailureMode) - 1] = 0;
            activeFailureConsumed = false;
        }
        const bool once = !activeFailureConsumed && ((!value && !std::strcmp(mode, "off-once")) || (value && !std::strcmp(mode, "on-once")));
        if (once || (value && !std::strcmp(mode, "on-always"))) { activeFailureConsumed = true; return kResultFalse; }
        componentActive = value; return kResultOk;
    }
    tresult PLUGIN_API setState(IBStream *stream) override {
        State state{}; int32 count = 0;
        if (stream->read(&state, sizeof(state), &count) != kResultOk || count != sizeof(state) || state.magic != stateMagic || state.keyChannels < 1 || state.keyChannels > 3) return kResultFalse;
        keyChannels = state.keyChannels; return kResultOk;
    }
    tresult PLUGIN_API getState(IBStream *stream) override { State state{stateMagic, keyChannels}; return stream->write(&state, sizeof(state), nullptr); }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *inputs, int32 numInputs, SpeakerArrangement *outputs, int32 numOutputs) override {
        if (numInputs != inputCount() || numOutputs != 1 || outputs[0] != SpeakerArr::kStereo) return kResultFalse;
        for (int32 n = 0; n < numInputs; ++n) if (SpeakerArr::getChannelCount(inputs[n]) != channels(n)) return kResultFalse;
        return kResultOk;
    }
    tresult PLUGIN_API getBusArrangement(BusDirection direction, int32 index, SpeakerArrangement& arrangement) override {
        if (index < 0 || index >= getBusCount(kAudio, direction)) return kInvalidArgument;
        int32 count = direction == kOutput ? 2 : channels(index);
        arrangement = count == 1 ? SpeakerArr::kMono : count == 2 ? SpeakerArr::kStereo : SpeakerArr::k30Cine;
        return kResultOk;
    }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { setupDone = true; return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool value) override {
        if (value && !componentActive) return kResultFalse;
        processing = value; return kResultOk;
    }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (data.numInputs != inputCount() || data.numOutputs != 1 || data.outputs[0].numChannels != 2) return kResultFalse;
        for (int32 bus = 0; bus < inputCount(); ++bus) {
            if (data.inputs[bus].numChannels != channels(bus)) return kResultFalse;
            if (!active[bus]) for (int32 channel = 0; channel < channels(bus); ++channel)
                for (int32 frame = 0; frame < data.numSamples; ++frame)
                    if (data.inputs[bus].channelBuffers32[channel][frame] != 0) return kResultFalse;
        }
        for (int32 channel = 0; channel < 2; ++channel) for (int32 frame = 0; frame < data.numSamples; ++frame) {
            const float program = data.inputs[mainIndex()].channelBuffers32[channel][frame];
            const float key = data.inputs[keyIndex()].channelBuffers32[std::min(channel, keyChannels - 1)][frame];
            data.outputs[0].channelBuffers32[channel][frame] = program - key;
        }
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
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IPluginFactory::iid) || same(iid, IPluginFactory2::iid)) *object = static_cast<IPluginFactory2 *>(this);
        if (!*object) return kNoInterface;
        addRef(); return kResultOk;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo *info) override { *info = {}; std::strcpy(info->vendor, "SuperTracker Tests"); return kResultOk; }
    int32 PLUGIN_API countClasses() override { return 4; }
    tresult PLUGIN_API getClassInfo(int32 index, PClassInfo *info) override {
        if (index < 0 || index >= countClasses()) return kInvalidArgument;
        *info = {}; classID(index).toTUID(info->cid); std::strcpy(info->name, classNames[index]);
        std::strcpy(info->category, kVstAudioEffectClass); info->cardinality = PClassInfo::kManyInstances; return kResultOk;
    }
    tresult PLUGIN_API createInstance(FIDString cid, FIDString iid, void **object) override {
        *object = nullptr;
        for (int n = 0; n < countClasses(); ++n) if (same(cid, classID(n))) {
            auto component = new Component(n); auto result = component->queryInterface(iid, object); component->release(); return result;
        }
        return kNoInterface;
    }
    tresult PLUGIN_API getClassInfo2(int32 index, PClassInfo2 *info) override {
        if (index < 0 || index >= countClasses()) return kInvalidArgument;
        *info = {}; classID(index).toTUID(info->cid); std::strcpy(info->name, classNames[index]);
        std::strcpy(info->category, kVstAudioEffectClass); std::strcpy(info->subCategories, "Fx|Dynamics");
        std::strcpy(info->vendor, "SuperTracker Tests"); return kResultOk;
    }
};
}
extern "C" __attribute__((visibility("default"))) bool bundleEntry(void *) { return true; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() { return true; }
extern "C" __attribute__((visibility("default"))) IPluginFactory *GetPluginFactory() { return new Factory; }
