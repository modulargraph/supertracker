// Original deterministic VST3 test component. No installed vendor plugin is loaded.
#define INIT_CLASS_IID
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <algorithm>
using namespace Steinberg; using namespace Steinberg::Vst;
namespace {
void trace(const char *text) { if (const char *path = getenv("ST_VST3_FIXTURE_TRACE")) if (FILE *file = fopen(path, "a")) { fprintf(file, "%s\n", text); fclose(file); } }
struct Global { Global() { trace("static.init"); } ~Global() { trace("static.destroy"); } } global;
bool same(const TUID a, const FUID& b) { return FUnknownPrivate::iidEqual(a, b); }
FUID classID() { FUID id; id.fromString("0123456789ABCDEF0123456789ABCDEF"); return id; }
FUnknown *sharedHostContext = nullptr;
ParamID gainParameterID() { return getenv("ST_VST3_FIXTURE_FAILED_MIDI_ZERO_OUTPUT") ? 0 : 42; }
class Component final : public IComponent, public IAudioProcessor, public IEditController, public IConnectionPoint, public IMidiMapping {
    std::atomic<uint32> refs{1}; double gain = .25, controlGain = .25; bool note = false;
    bool deferred = getenv("ST_VST3_FIXTURE_DEFER_CONTROLLER") != nullptr, pendingRestore = false;
    bool startupPending = getenv("ST_VST3_FIXTURE_DEFER_STARTUP") != nullptr;
    bool deferOneFrame = getenv("ST_VST3_FIXTURE_DEFER_ONE_FRAME_EDIT") != nullptr;
    bool stagedEdit = false; double stagedGain = 0;
    // Optional kLatencyChanged fixture: the new latency is reported only after the
    // host deactivates and reactivates the component, as the VST3 specification allows.
    bool latencyRestart = getenv("ST_VST3_FIXTURE_LATENCY_RESTART") != nullptr; uint32 latency = 0, pendingLatency = 0;
    IComponentHandler *handler = nullptr;
public:
    Component() { trace("component.create"); }
    ~Component() { trace("component.destroy"); }
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        *object = nullptr;
        if (same(iid, FUnknown::iid) || same(iid, IComponent::iid) || same(iid, IPluginBase::iid)) *object = static_cast<IComponent *>(this);
        else if (same(iid, IAudioProcessor::iid)) { if (!getenv("ST_VST3_FIXTURE_FAIL_PROCESSOR")) *object = static_cast<IAudioProcessor *>(this); }
        else if (same(iid, IEditController::iid)) *object = static_cast<IEditController *>(this);
        else if (same(iid, IConnectionPoint::iid)) *object = static_cast<IConnectionPoint *>(this);
        else if (same(iid, IMidiMapping::iid)) *object = static_cast<IMidiMapping *>(this);
        if (*object) { addRef(); return kResultOk; } return kNoInterface;
    }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API initialize(FUnknown *host) override { trace("component.initialize"); trace(host == sharedHostContext ? "component.context.shared" : "component.context.different"); return kResultOk; }
    tresult PLUGIN_API terminate() override { trace("component.terminate"); return kResultOk; }
    tresult PLUGIN_API getControllerClassId(TUID) override { return kNotImplemented; }
    tresult PLUGIN_API setIoMode(IoMode) override { return kResultOk; }
    int32 PLUGIN_API getBusCount(MediaType media, BusDirection direction) override { return (media == kAudio && direction == kOutput) || (media == kEvent && direction == kInput) ? 1 : 0; }
    tresult PLUGIN_API getBusInfo(MediaType media, BusDirection direction, int32 index, BusInfo& info) override { if (index != 0 || getBusCount(media, direction) == 0) return kInvalidArgument; info = {}; info.mediaType = media; info.direction = direction; info.channelCount = media == kAudio ? 2 : 16; info.busType = kMain; info.flags = BusInfo::kDefaultActive; return kResultOk; }
    tresult PLUGIN_API getRoutingInfo(RoutingInfo&, RoutingInfo&) override { return kNotImplemented; }
    tresult PLUGIN_API activateBus(MediaType, BusDirection, int32, TBool) override { return kResultOk; }
    tresult PLUGIN_API setActive(TBool active) override { trace(active ? "component.active" : "component.inactive"); if (active && pendingLatency) { latency = pendingLatency; pendingLatency = 0; } return kResultOk; }
    tresult PLUGIN_API setState(IBStream *stream) override { struct State { uint32 magic; double dsp, control; } state{}; int32 read = 0; if (stream->read(&state, sizeof(state), &read) != kResultOk || read != sizeof(state) || state.magic != 0x53545633) return kResultFalse; gain = state.dsp; controlGain = deferred ? 0 : state.control; pendingRestore = deferred; note = false; return kResultOk; }
    tresult PLUGIN_API getState(IBStream *stream) override { struct State { uint32 magic; double dsp, control; } state{0x53545633, gain, controlGain}; return stream->write(&state, sizeof(state), nullptr); }
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32, SpeakerArrangement *, int32) override { return kResultOk; }
    tresult PLUGIN_API getBusArrangement(BusDirection, int32, SpeakerArrangement& arrangement) override { arrangement = SpeakerArr::kStereo; return kResultOk; }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return latency; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool processing) override { trace(processing ? "processor.start" : "processor.stop"); if (!processing) stagedEdit = false; return kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        if (startupPending && data.numSamples) { startupPending = false; gain = controlGain = .25; }
        if (latencyRestart && handler) { latencyRestart = false; pendingLatency = 256; handler->restartComponent(kLatencyChanged); }
        if (data.numSamples > 1 && stagedEdit) { gain = stagedGain; stagedEdit = false; }
        auto updateParameters = [&](int32 sample) { if (data.inputParameterChanges) for (int32 n = 0; n < data.inputParameterChanges->getParameterCount(); ++n) { auto queue = data.inputParameterChanges->getParameterData(n); if (queue->getParameterId() == gainParameterID()) for (int32 p = 0; p < queue->getPointCount(); ++p) { int32 offset = 0; double value = 0; if (queue->getPoint(p, offset, value) == kResultOk && offset == sample) { if (deferOneFrame && data.numSamples == 1) { stagedEdit = true; stagedGain = value; } else gain = value; } } } };
        if (!data.numSamples) updateParameters(0);
        for (int32 sample = 0; sample < data.numSamples; ++sample) {
            updateParameters(sample);
            if (data.inputEvents) for (int32 n = 0; n < data.inputEvents->getEventCount(); ++n) { Event event{}; if (data.inputEvents->getEvent(n, event) == kResultOk && event.sampleOffset == sample) { if (event.type == Event::kNoteOnEvent) { note = true; if (latency) trace(event.flags & Event::kIsLive ? "event.on.live" : "event.on.sequenced"); } if (event.type == Event::kNoteOffEvent) note = false; } }
            for (int32 channel = 0; channel < data.outputs[0].numChannels; ++channel) data.outputs[0].channelBuffers32[channel][sample] = note ? float(gain) : 0.f;
        }
        if (pendingRestore) { pendingRestore = false; controlGain = gain; if (handler) handler->restartComponent(kParamValuesChanged); }
        return kResultOk;
    }
    tresult PLUGIN_API setComponentState(IBStream *stream) override { struct State { uint32 magic; double dsp, control; } state{}; int32 read = 0; if (stream->read(&state, sizeof(state), &read) != kResultOk || read != sizeof(state) || state.magic != 0x53545633) return kResultFalse; controlGain = deferred ? 0 : state.dsp; pendingRestore = deferred; return kResultOk; }
    int32 PLUGIN_API getParameterCount() override { return 1; }
    tresult PLUGIN_API getParameterInfo(int32 index, ParameterInfo& info) override { if (index) return kInvalidArgument; info = {}; info.id = gainParameterID(); info.flags = ParameterInfo::kCanAutomate; info.defaultNormalizedValue = .25; const char16_t title[] = u"Gain"; memcpy(info.title, title, sizeof(title)); return kResultOk; }
    tresult PLUGIN_API getParamStringByValue(ParamID, ParamValue, String128 string) override { string[0] = '0'; string[1] = 0; return kResultOk; }
    tresult PLUGIN_API getParamValueByString(ParamID, TChar *, ParamValue& value) override { value = 0; return kResultOk; }
    ParamValue PLUGIN_API normalizedParamToPlain(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API plainParamToNormalized(ParamID, ParamValue value) override { return value; }
    ParamValue PLUGIN_API getParamNormalized(ParamID) override { return controlGain; }
    tresult PLUGIN_API setParamNormalized(ParamID id, ParamValue value) override { if (id != gainParameterID()) return kInvalidArgument; controlGain = value; return kResultOk; }
    tresult PLUGIN_API setComponentHandler(IComponentHandler *value) override { handler = value; return kResultOk; }
    tresult PLUGIN_API getMidiControllerAssignment(int32, int16, CtrlNumber cc, ParamID& id) override {
        // An out-parameter is unspecified on failure. Writing a real parameter
        // here catches hosts that incorrectly turn unmapped MIDI into macro edits.
        if (getenv("ST_VST3_FIXTURE_FAILED_MIDI_OUTPUT")) id = 42;
        if (getenv("ST_VST3_FIXTURE_FAILED_MIDI_ZERO_OUTPUT")) id = 0;
        if (getenv("ST_VST3_FIXTURE_VALID_MIDI") && cc == 7) { id = gainParameterID(); return kResultOk; }
        return kResultFalse;
    }
    IPlugView *PLUGIN_API createView(FIDString) override { return nullptr; }
    tresult PLUGIN_API connect(IConnectionPoint *) override { trace("connection.connect"); return kResultOk; }
    tresult PLUGIN_API disconnect(IConnectionPoint *) override { trace("connection.disconnect"); return kResultOk; }
    tresult PLUGIN_API notify(IMessage *) override { return kResultOk; }
};
class Factory final : public IPluginFactory3 {
    std::atomic<uint32> refs{1};
public:
    Factory() { trace("factory.create"); } ~Factory() { if (sharedHostContext) { IHostApplication *host = nullptr; if (sharedHostContext->queryInterface(IHostApplication::iid, reinterpret_cast<void **>(&host)) == kResultOk && host) { String128 name{}; host->getName(name); if (name[0] == 'S') trace("factory.context.valid"); host->release(); } sharedHostContext->release(); sharedHostContext = nullptr; } trace("factory.destroy"); }
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override { *object = nullptr; if (same(iid, FUnknown::iid) || same(iid, IPluginFactory::iid) || same(iid, IPluginFactory2::iid) || same(iid, IPluginFactory3::iid)) { *object = static_cast<IPluginFactory3 *>(this); addRef(); return kResultOk; } return kNoInterface; }
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto count = --refs; if (!count) delete this; return count; }
    tresult PLUGIN_API getFactoryInfo(PFactoryInfo *info) override { *info = {}; strcpy(info->vendor, "SuperTracker Tests"); return kResultOk; }
    int32 PLUGIN_API countClasses() override { return 1; }
    tresult PLUGIN_API getClassInfo(int32 index, PClassInfo *info) override { if (index) return kInvalidArgument; *info = {}; classID().toTUID(info->cid); strcpy(info->name, "Deterministic Lifecycle Instrument"); strcpy(info->category, kVstAudioEffectClass); info->cardinality = PClassInfo::kManyInstances; return kResultOk; }
    tresult PLUGIN_API createInstance(FIDString cid, FIDString iid, void **object) override { *object = nullptr; if (!same(cid, classID())) return kNoInterface; auto component = new Component; auto result = component->queryInterface(iid, object); component->release(); return result; }
    tresult PLUGIN_API getClassInfo2(int32 index, PClassInfo2 *info) override { if (index) return kInvalidArgument; *info = {}; classID().toTUID(info->cid); strcpy(info->name, "Deterministic Lifecycle Instrument"); strcpy(info->category, kVstAudioEffectClass); strcpy(info->subCategories, "Instrument|Synth"); strcpy(info->vendor, "SuperTracker Tests"); return kResultOk; }
    tresult PLUGIN_API getClassInfoUnicode(int32, PClassInfoW *) override { return kNotImplemented; }
    tresult PLUGIN_API setHostContext(FUnknown *context) override { if (sharedHostContext) sharedHostContext->release(); sharedHostContext = context; if (context) context->addRef(); trace("factory.context.set"); return kResultOk; }
};
}
extern "C" __attribute__((visibility("default"))) bool bundleEntry(void *) { trace("bundle.entry"); return true; }
extern "C" __attribute__((visibility("default"))) bool bundleExit() { trace("bundle.exit"); return true; }
extern "C" __attribute__((visibility("default"))) IPluginFactory *GetPluginFactory() { return new Factory; }
