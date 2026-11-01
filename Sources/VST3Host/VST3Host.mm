#include "include/VST3Host.h"
#define INIT_CLASS_IID
#include "pluginterfaces/base/ipluginbase.h"
#include "pluginterfaces/base/ibstream.h"
#include "pluginterfaces/gui/iplugview.h"
#include "pluginterfaces/vst/ivstaudioprocessor.h"
#include "pluginterfaces/vst/ivstcomponent.h"
#include "pluginterfaces/vst/ivsteditcontroller.h"
#include "pluginterfaces/vst/ivstevents.h"
#include "pluginterfaces/vst/ivsthostapplication.h"
#include "pluginterfaces/vst/ivstparameterchanges.h"
#include "pluginterfaces/vst/ivstprocesscontext.h"
#include "pluginterfaces/vst/vstspeaker.h"
#include "public.sdk/source/vst/utility/vst2persistence.h"
#include "STHardwarePatchBaseline.h"
#import <AppKit/AppKit.h>
#include <mach/mach_time.h>
#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstring>
#include <cstdlib>
#include <memory>
#include <string>
#include <unordered_map>
#include <vector>

using namespace Steinberg;
using namespace Steinberg::Vst;
namespace {
constexpr size_t stateLimit = 256u * 1024u * 1024u;
constexpr uint32_t maxEvents = 4096, maxChangedParameters = 512, maxPoints = 256;
bool lifecycleTrace() { const char *value = getenv("ST_VST3_LIFECYCLE"); return value && !strcmp(value, "1"); }
bool iidIs(const TUID a, const FUID& b) { return FUnknownPrivate::iidEqual(a, b); }
void errorText(char *buffer, size_t size, const std::string& value) {
    if (buffer && size) { const auto n = std::min(size - 1, value.size()); memcpy(buffer, value.data(), n); buffer[n] = 0; }
}
template<size_t N> void copyText(char (&target)[N], const char *value) { errorText(target, N, value ? value : ""); }
void utf8(const TChar *value, char *target, size_t size) {
    if (!target || !size) return;
    target[0] = 0;
    size_t count = 0; while (count < 128 && value[count]) ++count;
    CFStringRef string = CFStringCreateWithCharacters(nullptr, reinterpret_cast<const UniChar *>(value), count);
    if (string) { CFStringGetCString(string, target, size, kCFStringEncodingUTF8); CFRelease(string); }
}
#define EMBEDDED_REFS uint32 PLUGIN_API addRef() override { return 1; } uint32 PLUGIN_API release() override { return 1; }
#define QUERY_ONE(Interface) tresult PLUGIN_API queryInterface(const TUID iid, void **object) override { if (!object) return kInvalidArgument; *object = nullptr; if (iidIs(iid, FUnknown::iid) || iidIs(iid, Interface::iid)) { *object = static_cast<Interface *>(this); addRef(); return kResultOk; } return kNoInterface; }

class MemoryStream final : public IBStream {
public:
    std::vector<uint8_t> bytes;
    size_t position = 0;
    bool writable = true;
    MemoryStream() = default;
    MemoryStream(const void *source, size_t size) : bytes(static_cast<const uint8_t *>(source), static_cast<const uint8_t *>(source) + size), writable(false) {}
    QUERY_ONE(IBStream) EMBEDDED_REFS
    tresult PLUGIN_API read(void *buffer, int32 requested, int32 *readCount) override {
        if (readCount) *readCount = 0;
        if (requested < 0 || (!buffer && requested)) return kInvalidArgument;
        auto count = std::min(size_t(requested), bytes.size() - std::min(position, bytes.size()));
        if (count) memcpy(buffer, bytes.data() + position, count);
        position += count; if (readCount) *readCount = int32(count); return kResultOk;
    }
    tresult PLUGIN_API write(void *buffer, int32 requested, int32 *written) override {
        if (written) *written = 0;
        if (!writable || requested < 0 || (!buffer && requested) || position > stateLimit || size_t(requested) > stateLimit - position) return kInvalidArgument;
        if (position + requested > bytes.size()) bytes.resize(position + requested);
        if (requested) memcpy(bytes.data() + position, buffer, requested);
        position += requested; if (written) *written = requested; return kResultOk;
    }
    tresult PLUGIN_API seek(int64 offset, int32 mode, int64 *result) override {
        int64 base = mode == kIBSeekSet ? 0 : mode == kIBSeekCur ? int64(position) : mode == kIBSeekEnd ? int64(bytes.size()) : -1;
        if (base < 0 || offset < -base || offset > int64(stateLimit) - base) return kInvalidArgument;
        position = size_t(base + offset); if (result) *result = position; return kResultOk;
    }
    tresult PLUGIN_API tell(int64 *result) override { if (!result) return kInvalidArgument; *result = position; return kResultOk; }
};

class Attributes final : public IAttributeList {
    struct Value { int64 integer = 0; double real = 0; std::u16string string; std::vector<uint8_t> binary; };
    std::unordered_map<std::string, Value> values;
    std::atomic<uint32> refs{1};
public:
    QUERY_ONE(IAttributeList)
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto n = --refs; if (!n) delete this; return n; }
    tresult PLUGIN_API setInt(AttrID id, int64 value) override { if (!id) return kInvalidArgument; values[id].integer = value; return kResultOk; }
    tresult PLUGIN_API getInt(AttrID id, int64& value) override { auto it = values.find(id ? id : ""); if (it == values.end()) return kResultFalse; value = it->second.integer; return kResultOk; }
    tresult PLUGIN_API setFloat(AttrID id, double value) override { if (!id) return kInvalidArgument; values[id].real = value; return kResultOk; }
    tresult PLUGIN_API getFloat(AttrID id, double& value) override { auto it = values.find(id ? id : ""); if (it == values.end()) return kResultFalse; value = it->second.real; return kResultOk; }
    tresult PLUGIN_API setString(AttrID id, const TChar *value) override { if (!id || !value) return kInvalidArgument; values[id].string = reinterpret_cast<const char16_t *>(value); return kResultOk; }
    tresult PLUGIN_API getString(AttrID id, TChar *value, uint32 bytes) override { auto it = values.find(id ? id : ""); if (it == values.end() || !value || bytes < 2) return kResultFalse; auto count = std::min(it->second.string.size(), size_t(bytes / 2 - 1)); memcpy(value, it->second.string.data(), count * 2); value[count] = 0; return kResultOk; }
    tresult PLUGIN_API setBinary(AttrID id, const void *value, uint32 bytes) override { if (!id || (!value && bytes) || bytes > stateLimit) return kInvalidArgument; auto& data = values[id].binary; data.resize(bytes); if (bytes) memcpy(data.data(), value, bytes); return kResultOk; }
    tresult PLUGIN_API getBinary(AttrID id, const void *&value, uint32& bytes) override { auto it = values.find(id ? id : ""); if (it == values.end()) return kResultFalse; value = it->second.binary.data(); bytes = uint32(it->second.binary.size()); return kResultOk; }
};
class Message final : public IMessage {
    std::atomic<uint32> refs{1}; std::string id; Attributes *attributes = new Attributes;
public:
    ~Message() { attributes->release(); }
    QUERY_ONE(IMessage)
    uint32 PLUGIN_API addRef() override { return ++refs; }
    uint32 PLUGIN_API release() override { auto n = --refs; if (!n) delete this; return n; }
    const char *PLUGIN_API getMessageID() override { return id.c_str(); }
    void PLUGIN_API setMessageID(const char *value) override { id = value ? value : ""; }
    IAttributeList *PLUGIN_API getAttributes() override { return attributes; }
};
class EventList final : public IEventList {
public:
    std::array<Event, maxEvents> events{}; int32 count = 0;
    QUERY_ONE(IEventList) EMBEDDED_REFS
    int32 PLUGIN_API getEventCount() override { return count; }
    tresult PLUGIN_API getEvent(int32 index, Event& event) override { if (index < 0 || index >= count) return kInvalidArgument; event = events[index]; return kResultOk; }
    tresult PLUGIN_API addEvent(Event& event) override { if (count == maxEvents) return kResultFalse; events[count++] = event; return kResultOk; }
};
struct ParameterTrace {
    static_assert(std::atomic<double>::is_always_lock_free && std::atomic<uint64_t>::is_always_lock_free, "VST3 trace must remain lock-free on the audio thread");
    // The watched identity changes only while rendering is quiesced. Counter
    // access stays atomic so UI snapshots need not interrupt a running graph.
    bool enabled = false; uint32 id = 0;
    std::atomic<uint64_t> uiQueued{0}, uiDequeued{0}, automationReceived{0}, midiReceived{0};
    std::atomic<uint64_t> offeredPoints{0}, readPoints{0}, processes{0}, starts{0}, stops{0};
    std::atomic<uint64_t> resets{0}, copies{0}, restores{0}, offeredProcess{0}, readProcess{0};
    std::atomic<double> uiValue{0}, dequeuedValue{0}, automationValue{0}, midiValue{0}, offeredValue{0}, readValue{0};
    std::atomic<int32> offeredOffset{0}, readOffset{0}, offeredFrames{0}, readFrames{0}, processResult{0};
    uint64_t currentProcess = 0; int32 currentFrames = 0;
    bool matches(uint32 parameter) const { return enabled && id == parameter; }
    void reset(uint32 parameter, bool enable) {
        enabled = enable; id = parameter;
        uiQueued = uiDequeued = automationReceived = midiReceived = 0;
        offeredPoints = readPoints = processes = starts = stops = 0;
        resets = copies = restores = offeredProcess = readProcess = 0;
        uiValue = dequeuedValue = automationValue = midiValue = offeredValue = readValue = 0;
        offeredOffset = readOffset = offeredFrames = readFrames = processResult = 0;
        currentProcess = 0; currentFrames = 0;
    }
};
class ParameterQueue final : public IParamValueQueue {
public:
    uint32 id = 0; int32 count = 0;
    ParameterTrace *trace = nullptr;
    struct Point { int32 offset; double value; }; std::array<Point, maxPoints> points{};
    QUERY_ONE(IParamValueQueue) EMBEDDED_REFS
    ParamID PLUGIN_API getParameterId() override { return id; }
    int32 PLUGIN_API getPointCount() override { return count; }
    tresult PLUGIN_API getPoint(int32 index, int32& offset, double& value) override {
        if (index < 0 || index >= count) return kInvalidArgument;
        offset = points[index].offset; value = points[index].value;
        if (trace && trace->matches(id)) {
            trace->readPoints.fetch_add(1, std::memory_order_relaxed);
            trace->readValue.store(value, std::memory_order_relaxed); trace->readOffset.store(offset, std::memory_order_relaxed);
            trace->readFrames.store(trace->currentFrames, std::memory_order_relaxed); trace->readProcess.store(trace->currentProcess, std::memory_order_relaxed);
        }
        return kResultOk;
    }
    tresult PLUGIN_API addPoint(int32 offset, double value, int32& index) override {
        // Coalesce repeated timestamps and preserve ascending sample positions.
        int32 place = 0; while (place < count && points[place].offset < offset) ++place;
        if (place < count && points[place].offset == offset) { points[place].value = value; index = place; return kResultOk; }
        if (count == maxPoints) return kResultFalse;
        for (int32 n = count; n > place; --n) points[n] = points[n - 1];
        points[place] = {offset, value}; ++count; index = place; return kResultOk;
    }
};
class ParameterChanges final : public IParameterChanges {
public:
    std::array<ParameterQueue, maxChangedParameters> queues; int32 count = 0;
    ParameterTrace *trace = nullptr;
    QUERY_ONE(IParameterChanges) EMBEDDED_REFS
    int32 PLUGIN_API getParameterCount() override { return count; }
    IParamValueQueue *PLUGIN_API getParameterData(int32 index) override { return index >= 0 && index < count ? &queues[index] : nullptr; }
    IParamValueQueue *PLUGIN_API addParameterData(const ParamID& id, int32& index) override {
        for (index = 0; index < count; ++index) if (queues[index].id == id) return &queues[index];
        if (count == maxChangedParameters) return nullptr;
        auto& queue = queues[count++]; queue.id = id; queue.count = 0; queue.trace = trace && trace->matches(id) ? trace : nullptr; return &queue;
    }
    bool add(uint32 id, int32 offset, double value) { int32 index; auto queue = addParameterData(id, index); return queue && queue->addPoint(offset, value, index) == kResultOk; }
};

class FactoryContext final : public IHostApplication {
public:
    QUERY_ONE(IHostApplication) EMBEDDED_REFS
    tresult PLUGIN_API getName(String128 name) override { const char16_t text[] = u"SuperTracker"; memcpy(name, text, sizeof(text)); return kResultOk; }
    tresult PLUGIN_API createInstance(TUID cid, TUID iid, void **object) override {
        if (!object) return kInvalidArgument; *object = nullptr;
        if (iidIs(cid, IMessage::iid) && iidIs(iid, IMessage::iid)) *object = static_cast<IMessage *>(new Message);
        if (iidIs(cid, IAttributeList::iid) && iidIs(iid, IAttributeList::iid)) *object = static_cast<IAttributeList *>(new Attributes);
        return *object ? kResultOk : kNoInterface;
    }
};
struct Module {
    FactoryContext hostContext;
    std::string bundleVersion;
    CFBundleRef bundle = nullptr; IPluginFactory *factory = nullptr; bool entered = false;
    using Entry = bool (*)(CFBundleRef); using Exit = bool (*)();
    ~Module() {
        if (lifecycleTrace()) fprintf(stderr, "VST3 module teardown begin factory=%p\n", factory);
        if (factory) factory->release();
        if (lifecycleTrace()) fprintf(stderr, "VST3 factory released\n");
        if (bundle) { if (entered) if (auto exit = reinterpret_cast<Exit>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleExit")))) { if (lifecycleTrace()) fprintf(stderr, "VST3 bundleExit begin\n"); exit(); if (lifecycleTrace()) fprintf(stderr, "VST3 bundleExit returned\n"); } CFRelease(bundle); }
        if (lifecycleTrace()) fprintf(stderr, "VST3 module teardown complete\n");
    }
    bool open(const char *path, std::string& error) {
        if (!path) { error = "No VST3 bundle path."; return false; }
        CFURLRef url = CFURLCreateFromFileSystemRepresentation(nullptr, reinterpret_cast<const UInt8 *>(path), strlen(path), true);
        if (!url) { error = "Invalid VST3 bundle path."; return false; }
        bundle = CFBundleCreate(nullptr, url); CFRelease(url);
        if (!bundle || !CFBundleLoadExecutable(bundle)) { error = "Unable to load VST3 executable (architecture, dependency, or code signature)."; return false; }
        // Cache the version from this exact loaded CFBundle. VST3 factory version
        // strings need not match the public bundle release version. Preserve the
        // complete UTF-8 value for strict matching; never normalize a near match.
        auto version = CFBundleGetValueForInfoDictionaryKey(bundle, CFSTR("CFBundleShortVersionString"));
        if (version && CFGetTypeID(version) == CFStringGetTypeID()) {
            auto string = static_cast<CFStringRef>(version); auto length = CFStringGetLength(string);
            auto maximum = CFStringGetMaximumSizeForEncoding(length, kCFStringEncodingUTF8);
            if (maximum >= 0 && maximum <= 65536) {
                std::string value(size_t(maximum), '\0'); CFIndex bytes = 0;
                auto converted = CFStringGetBytes(string, CFRangeMake(0, length), kCFStringEncodingUTF8, 0, false, reinterpret_cast<UInt8 *>(value.data()), maximum, &bytes);
                if (converted == length) { value.resize(size_t(bytes)); if (value.find('\0') == std::string::npos) bundleVersion = std::move(value); }
            }
        }
        if (auto entry = reinterpret_cast<Entry>(CFBundleGetFunctionPointerForName(bundle, CFSTR("bundleEntry")))) {
            if (!entry(bundle)) { error = "VST3 bundleEntry failed."; return false; } entered = true;
        }
        using Factory = IPluginFactory *(*)();
        auto getFactory = reinterpret_cast<Factory>(CFBundleGetFunctionPointerForName(bundle, CFSTR("GetPluginFactory")));
        factory = getFactory ? getFactory() : nullptr;
        if (!factory) { error = "VST3 bundle has no plugin factory."; return false; }
        IPluginFactory3 *factory3 = nullptr;
        if (factory->queryInterface(IPluginFactory3::iid, reinterpret_cast<void **>(&factory3)) == kResultOk && factory3) { factory3->setHostContext(&hostContext); factory3->release(); }
        return true;
    }
};
// A module can own process-global services (licensing, worker pools, Qt). Keep one
// entry/factory per path until the host's explicit shutdown cleanup
// (st_vst3_release_unused_modules) instead of calling bundleExit while another
// instance or a vendor background worker still uses the same module.
// The cache is intentionally leaked: if the process exits without that cleanup,
// destroying it during static teardown would call bundleExit after the bundle's
// own C++ statics are gone (JUCE frees its already-destroyed bundle array and
// aborts). The OS reclaims leaked modules at exit; explicit cleanup still unloads.
std::unordered_map<std::string, std::shared_ptr<Module>>& moduleCache() { static auto *modules = new std::unordered_map<std::string, std::shared_ptr<Module>>(); return *modules; }
std::shared_ptr<Module> loadModule(const char *path, std::string& error) {
    auto& modules = moduleCache();
    if (!path) { error = "No VST3 bundle path."; return {}; }
    auto existing = modules.find(path); if (existing != modules.end()) return existing->second;
    auto module = std::make_shared<Module>(); if (!module->open(path, error)) return {};
    modules.emplace(path, module); return module;
}
template<class T> T *query(FUnknown *object) { T *result = nullptr; if (object) object->queryInterface(T::iid, reinterpret_cast<void **>(&result)); return result; }
struct Buses {
    std::vector<AudioBusBuffers> buses;
    std::vector<std::vector<float>> storage;
    std::vector<std::vector<float *>> channels;
    std::vector<BusInfo> metadata;
    std::vector<uint8_t> activation;
    bool configured = false;
    int32 mainIndex = -1;
    static bool sameLayout(const BusInfo& a, const BusInfo& b) {
        return a.mediaType == b.mediaType && a.direction == b.direction && a.channelCount == b.channelCount && a.busType == b.busType && !memcmp(a.name, b.name, sizeof(a.name));
    }
    bool layoutMatches(IComponent *component, BusDirection direction) const {
        if (!configured) return true;
        if (component->getBusCount(kAudio, direction) != int32(metadata.size())) return false;
        for (int32 n = 0; n < int32(metadata.size()); ++n) { BusInfo info{}; if (component->getBusInfo(kAudio, direction, n, info) != kResultOk || !sameLayout(metadata[n], info)) return false; }
        return true;
    }
    bool reapplyActivation(IComponent *component, BusDirection direction, std::string& error) const {
        for (int32 n = 0; n < int32(activation.size()); ++n) if (component->activateBus(kAudio, direction, n, activation[n] != 0) != kResultOk) { error = "VST3 rejected its saved audio bus activation."; return false; }
        return true;
    }
    bool configure(IComponent *component, BusDirection direction, uint32 frames, std::string& error) {
        int32 count = component->getBusCount(kAudio, direction);
        if (count < 0 || count > 32) { error = "VST3 audio bus count exceeds host limit."; return false; }
        if (!layoutMatches(component, direction)) { error = "VST3 audio bus layout changed. Reload this component before processing."; return false; }
        Buses next; next.configured = true;
        next.buses.resize(count); next.storage.resize(count); next.channels.resize(count); next.metadata.resize(count); next.activation.resize(count);
        for (int32 n = 0; n < count; ++n) {
            auto& info = next.metadata[n];
            if (component->getBusInfo(kAudio, direction, n, info) != kResultOk || info.channelCount < 0 || info.channelCount > 64) { error = "Unsupported VST3 audio channel configuration."; return false; }
            if (next.mainIndex < 0 && info.busType == kMain) next.mainIndex = n;
            next.channels[n].resize(info.channelCount); next.storage[n].resize(size_t(info.channelCount) * frames);
            for (int32 c = 0; c < info.channelCount; ++c) next.channels[n][c] = next.storage[n].data() + size_t(c) * frames;
            next.buses[n].numChannels = info.channelCount; next.buses[n].channelBuffers32 = next.channels[n].data();
        }
        for (int32 n = 0; n < count; ++n) {
            next.activation[n] = configured ? activation[n] : (direction == kInput ? n == next.mainIndex : n == 0);
        }
        *this = std::move(next); return true;
    }
    void clear(uint32 frames, bool silent = false) {
        for (auto& bus : buses) {
            bus.silenceFlags = silent ? (bus.numChannels == 64 ? UINT64_MAX : (uint64(1) << bus.numChannels) - 1) : 0;
            for (int32 n = 0; n < bus.numChannels; ++n) std::fill_n(bus.channelBuffers32[n], frames, 0.f);
        }
    }
};
}

// IAudioProcessor::setProcessing is optional: kNotImplemented means the component
// processes without the call (iZotope Ozone 12 answers it), not a refusal.
static inline bool processingAccepted(tresult result) { return result == kResultOk || result == kNotImplemented; }

struct STVST3Instance final : IHostApplication, IComponentHandler, IPlugFrame {
    std::shared_ptr<Module> module;
    IComponent *component = nullptr; IAudioProcessor *processor = nullptr; IEditController *controller = nullptr;
    IConnectionPoint *componentConnection = nullptr, *controllerConnection = nullptr;
    IMidiMapping *midiMapping = nullptr; IPlugView *view = nullptr; NSView *parent = nil;
    bool componentInitialized = false, controllerInitialized = false, active = false, processing = false;
    bool instrument = false; double sampleRate = 48000; uint32 maxFrames = 16384;
    // Sub-block scratch for instruments split at parameter changes (sized once, never on the render thread).
    std::vector<STVST3MIDI> splitMIDI = std::vector<STVST3MIDI>(4096);
    std::vector<STVST3ParameterChange> splitParameters = std::vector<STVST3ParameterChange>(4096);
    char uid[33]{}, pluginVersion[64]{};
    Buses inputs, outputs;
    EventList events, outputEvents;
    ParameterChanges parameterChanges, outputChanges;
    ParameterTrace parameterTrace;
    std::atomic<uint32_t> restartFlags{0}; std::atomic<bool> controllerRefreshNeeded{false}, midiRefreshNeeded{false}; std::atomic<uint64_t> dropped{0};
    std::array<STVST3ParameterChange, 8192> pending{};
    std::atomic<uint32_t> pendingWrite{0}, pendingRead{0};
    std::atomic<uint64_t> editGeneration{0}, capturedGeneration{0};
    enum class CapturePhase { idle, processing, waiting, authorized, copying };
    CapturePhase capturePhase = CapturePhase::idle;
    uint64_t captureEpoch = 0, captureGeneration = 0;
    bool captureEpochExhausted = false, captureVendorCallActive = false;
    bool failedStateRestore = false;
    std::chrono::steady_clock::time_point captureDeadline{};
    void invalidateCapture() {
        capturePhase = CapturePhase::idle;
        if (captureEpoch == UINT64_MAX) captureEpochExhausted = true;
        else ++captureEpoch;
    }
    std::array<std::array<std::atomic<ParamID>, 130>, 16> midiParameter{};
    struct HeldNote { bool active = false; int32 id = -1; };
    std::array<std::array<HeldNote, 128>, 16> heldNotes{};
    struct ParameterState { std::atomic<double> value{0}; std::atomic<bool> dirty{false}; };
    std::vector<ParamID> parameterIDs;
    std::unique_ptr<ParameterState[]> parameterStates;
    /// Bumped after every cached-value write (render thread included; lock-free), so
    /// the main-thread poll can skip republishing hundreds of unchanged parameters.
    std::atomic<uint64_t> parameterGeneration{0};
    STVST3Instance() { for (auto& channel : midiParameter) for (auto& parameter : channel) parameter.store(kNoParamId, std::memory_order_relaxed); }
    ~STVST3Instance() {
        if (lifecycleTrace()) fprintf(stderr, "VST3 instance teardown begin %p separateController=%d\n", this, controllerInitialized);
        st_vst3_detach_editor(this);
        if (processing) processor->setProcessing(false);
        if (active) component->setActive(false);
        if (componentConnection && controllerConnection) { componentConnection->disconnect(controllerConnection); controllerConnection->disconnect(componentConnection); }
        if (componentConnection) componentConnection->release(); if (controllerConnection) controllerConnection->release();
        if (controller) controller->setComponentHandler(nullptr);
        // Match SDK PlugProvider: terminate the component before a distinct
        // controller, while all queried interfaces remain alive. Combined plugins
        // receive initialize/terminate exactly once.
        if (component && componentInitialized) component->terminate();
        if (controller && controllerInitialized) controller->terminate();
        if (midiMapping) midiMapping->release();
        if (controller) controller->release();
        if (processor) processor->release();
        if (component) component->release();
        if (lifecycleTrace()) fprintf(stderr, "VST3 instance teardown complete %p\n", this);
    }
    tresult PLUGIN_API queryInterface(const TUID iid, void **object) override {
        if (!object) return kInvalidArgument; *object = nullptr;
        if (iidIs(iid, FUnknown::iid) || iidIs(iid, IHostApplication::iid)) *object = static_cast<IHostApplication *>(this);
        else if (iidIs(iid, IComponentHandler::iid)) *object = static_cast<IComponentHandler *>(this);
        else if (iidIs(iid, IPlugFrame::iid)) *object = static_cast<IPlugFrame *>(this);
        return *object ? kResultOk : kNoInterface;
    }
    EMBEDDED_REFS
    tresult PLUGIN_API getName(String128 name) override { const char16_t text[] = u"SuperTracker"; memcpy(name, text, sizeof(text)); return kResultOk; }
    tresult PLUGIN_API createInstance(TUID cid, TUID iid, void **object) override {
        if (!object) return kInvalidArgument; *object = nullptr;
        if (iidIs(cid, IMessage::iid) && iidIs(iid, IMessage::iid)) *object = static_cast<IMessage *>(new Message);
        if (iidIs(cid, IAttributeList::iid) && iidIs(iid, IAttributeList::iid)) *object = static_cast<IAttributeList *>(new Attributes);
        return *object ? kResultOk : kNoInterface;
    }
    tresult PLUGIN_API beginEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API endEdit(ParamID) override { return kResultOk; }
    tresult PLUGIN_API performEdit(ParamID id, ParamValue value) override { return enqueue(id, value) ? kResultOk : kResultFalse; }
    tresult PLUGIN_API restartComponent(int32 flags) override { restartFlags.fetch_or(uint32(flags), std::memory_order_relaxed); if (flags & kParamValuesChanged) controllerRefreshNeeded.store(true, std::memory_order_release); if (flags & kMidiCCAssignmentChanged) midiRefreshNeeded.store(true, std::memory_order_release); return kResultOk; }
    tresult PLUGIN_API resizeView(IPlugView *target, ViewRect *size) override {
        if (!parent || target != view || !size || size->getWidth() <= 0 || size->getHeight() <= 0) return kInvalidArgument;
        [parent setFrameSize:NSMakeSize(size->getWidth(), size->getHeight())];
        if ([parent window].contentView == parent) [[parent window] setContentSize:[parent frame].size];
        return target->onSize(size);
    }
    bool enqueue(uint32 id, double value) {
        if (!std::isfinite(value)) return false;
        uint32 write = pendingWrite.load(std::memory_order_relaxed), next = (write + 1) % pending.size();
        if (next == pendingRead.load(std::memory_order_acquire)) { dropped.fetch_add(1, std::memory_order_relaxed); return false; }
        const auto generation = editGeneration.load(std::memory_order_relaxed);
        if (generation == UINT64_MAX) { dropped.fetch_add(1, std::memory_order_relaxed); return false; }
        pending[write] = {0, id, std::clamp(value, 0., 1.)};
        editGeneration.store(generation + 1, std::memory_order_release);
        pendingWrite.store(next, std::memory_order_release); cacheValue(id, value, false);
        if (parameterTrace.matches(id)) { parameterTrace.uiQueued.fetch_add(1, std::memory_order_relaxed); parameterTrace.uiValue.store(std::clamp(value, 0., 1.), std::memory_order_relaxed); }
        return true;
    }
    ParameterState *parameterState(uint32 id) { auto it = std::lower_bound(parameterIDs.begin(), parameterIDs.end(), id); return it != parameterIDs.end() && *it == id ? &parameterStates[it - parameterIDs.begin()] : nullptr; }
    void cacheValue(uint32 id, double value, bool dirty) { if (auto state = parameterState(id)) { state->value.store(value, std::memory_order_relaxed); if (dirty) state->dirty.store(true, std::memory_order_release); parameterGeneration.fetch_add(1, std::memory_order_release); } }
    void readControllerValues(bool force = true) {
        if (!controller) return;
        for (size_t n = 0; n < parameterIDs.size(); ++n) {
            auto& state = parameterStates[n];
            if (force) state.dirty.store(false, std::memory_order_release);
            // During normal UI polling, preserve newer pending audio automation.
            if (force || !state.dirty.load(std::memory_order_acquire)) state.value.store(controller->getParamNormalized(parameterIDs[n]), std::memory_order_relaxed);
        }
        parameterGeneration.fetch_add(1, std::memory_order_release);
    }
    void refreshMIDIMapping() {
        if (!midiMapping) midiMapping = query<IMidiMapping>(controller);
        if (midiMapping) for (int16 channel = 0; channel < 16; ++channel) for (int16 cc = 0; cc < 130; ++cc) {
            ParamID id = kNoParamId;
            // A failed assignment has no valid out-parameter. Some components
            // write zero on failure; treating that as ParamID 0 turns a panic or
            // an unmapped CC into an unrelated macro edit.
            if (midiMapping->getMidiControllerAssignment(0, channel, cc, id) != kResultOk) id = kNoParamId;
            midiParameter[channel][cc].store(id, std::memory_order_relaxed);
        }
    }
    bool initialize(const char *path, const char *classUID, double rate, uint32 frames, std::string& error) {
        sampleRate = rate; maxFrames = frames; module = loadModule(path, error);
        if (!module) return false;
        FUID classID; if (!classUID || strlen(classUID) != 32 || !classID.fromString(classUID)) { error = "Invalid VST3 class identity."; return false; }
        classID.toString(uid);
        if (auto factory2 = query<IPluginFactory2>(module->factory)) {
            for (int32 n = 0; n < std::min(16384, module->factory->countClasses()); ++n) { PClassInfo2 info{}; if (factory2->getClassInfo2(n, &info) == kResultOk && FUnknownPrivate::iidEqual(info.cid, classID)) { instrument = std::string(info.subCategories).find("Instrument") != std::string::npos; copyText(pluginVersion, info.version); break; } }
            factory2->release();
        }
        if (module->factory->createInstance(classID, IComponent::iid, reinterpret_cast<void **>(&component)) != kResultOk || !component) { error = "VST3 factory could not create this class."; return false; }
        component->setIoMode(kAdvanced);
        if (component->initialize(&module->hostContext) != kResultOk) { error = "VST3 component initialization failed."; return false; } componentInitialized = true;
        processor = query<IAudioProcessor>(component); if (!processor || processor->canProcessSampleSize(kSample32) != kResultOk) { error = "VST3 does not support float32 audio processing."; return false; }
        controller = query<IEditController>(component);
        if (!controller) { TUID controllerID{}; if (component->getControllerClassId(controllerID) == kResultOk && module->factory->createInstance(controllerID, IEditController::iid, reinterpret_cast<void **>(&controller)) == kResultOk && controller) {
            if (controller->initialize(&module->hostContext) != kResultOk) { error = "VST3 controller initialization failed."; return false; } controllerInitialized = true;
        } }
        if (controller) {
            controller->setComponentHandler(this);
            if (controllerInitialized) {
                componentConnection = query<IConnectionPoint>(component); controllerConnection = query<IConnectionPoint>(controller);
                if (componentConnection && controllerConnection) { componentConnection->connect(controllerConnection); controllerConnection->connect(componentConnection); }
            }
            MemoryStream state; if (component->getState(&state) == kResultOk) {
                state.position = 0; const auto result = controller->setComponentState(&state);
                // Match the SDK base controller and PresetFile verifier: optional
                // synchronization may be unsupported, but explicit failure is real.
                if (result != kResultOk && result != kNotImplemented) { error = "VST3 controller rejected initial component-state synchronization."; return false; }
            }
            refreshMIDIMapping();
            for (int32 n = 0; n < std::min(65536, controller->getParameterCount()); ++n) { ParameterInfo info{}; if (controller->getParameterInfo(n, info) == kResultOk) parameterIDs.push_back(info.id); }
            std::sort(parameterIDs.begin(), parameterIDs.end()); parameterIDs.erase(std::unique(parameterIDs.begin(), parameterIDs.end()), parameterIDs.end());
            parameterStates = std::make_unique<ParameterState[]>(parameterIDs.size()); readControllerValues();
        }
        int32 inputCount = component->getBusCount(kAudio, kInput), outputCount = component->getBusCount(kAudio, kOutput);
        if (outputCount < 1 || inputCount < 0 || inputCount > 32 || outputCount > 32) { error = "Unsupported VST3 bus configuration."; return false; }
        std::vector<SpeakerArrangement> in(inputCount), out(outputCount);
        bool foundMainInput = false;
        for (int32 n = 0; n < inputCount; ++n) { processor->getBusArrangement(kInput, n, in[n]); BusInfo info{}; if (!foundMainInput && component->getBusInfo(kAudio, kInput, n, info) == kResultOk && info.busType == kMain) { in[n] = SpeakerArr::kStereo; foundMainInput = true; } }
        for (int32 n = 0; n < outputCount; ++n) { processor->getBusArrangement(kOutput, n, out[n]); if (n == 0) out[n] = SpeakerArr::kStereo; }
        // Failure leaves the vendor's default layout; mono defaults are supported.
        processor->setBusArrangements(in.data(), inputCount, out.data(), outputCount);
        if (!inputs.configure(component, kInput, maxFrames, error) || !outputs.configure(component, kOutput, maxFrames, error)) return false;
        instrument = instrument || (component->getBusCount(kEvent, kInput) > 0 && inputCount == 0);
        ProcessSetup setup{kRealtime, kSample32, int32(maxFrames), sampleRate};
        if (processor->setupProcessing(setup) != kResultOk) { error = "VST3 setupProcessing failed."; return false; }
        // activateBus is a Setup Done operation: setupProcessing must precede it,
        // and the component must still be inactive.
        if (!inputs.reapplyActivation(component, kInput, error) || !outputs.reapplyActivation(component, kOutput, error)) return false;
        for (int32 n = 0; n < component->getBusCount(kEvent, kInput); ++n) if (component->activateBus(kEvent, kInput, n, n == 0) != kResultOk) { error = "VST3 rejected its event input activation."; return false; }
        for (int32 n = 0; n < component->getBusCount(kEvent, kOutput); ++n) if (component->activateBus(kEvent, kOutput, n, false) != kResultOk) { error = "VST3 rejected its event output activation."; return false; }
        if (component->setActive(true) != kResultOk) { error = "VST3 could not activate audio processing."; return false; } active = true;
        if (!processingAccepted(processor->setProcessing(true))) { error = "VST3 could not start processing."; return false; } processing = true;
        return true;
    }
};

extern "C" {
static int processBlock(STVST3Instance *, const STVST3InputBuffer *, uint32_t, float *, float *, uint32_t,
                        const STVST3MIDI *, uint32_t, const STVST3ParameterChange *, uint32_t,
                        const STVST3Transport *, bool consumePendingUI, int64_t systemTimeNs);
int st_vst3_scan_bundle(const char *path, STVST3ClassCallback callback, void *context, char *error, size_t capacity) {
    @autoreleasepool { try {
        std::string message; auto module = loadModule(path, message); if (!module) { errorText(error, capacity, message); return 0; }
        PFactoryInfo factoryInfo{}; module->factory->getFactoryInfo(&factoryInfo);
        auto factory2 = query<IPluginFactory2>(module->factory); int count = 0;
        for (int32 n = 0; n < std::min(16384, module->factory->countClasses()); ++n) {
            PClassInfo info{}; if (module->factory->getClassInfo(n, &info) != kResultOk || strcmp(info.category, kVstAudioEffectClass)) continue;
            STVST3Class result{}; FUID(info.cid).toString(result.uid); copyText(result.name, info.name); copyText(result.vendor, factoryInfo.vendor);
            if (factory2) { PClassInfo2 extended{}; if (factory2->getClassInfo2(n, &extended) == kResultOk) { copyText(result.category, extended.subCategories); if (extended.vendor[0]) copyText(result.vendor, extended.vendor); } }
            if (callback) callback(&result, context); ++count;
        }
        if (factory2) factory2->release(); errorText(error, capacity, ""); return count;
    } catch (const std::exception& e) { errorText(error, capacity, e.what()); return 0; } }
}
STVST3Instance *st_vst3_create(const char *path, const char *uid, double rate, uint32_t maximum, char *error, size_t capacity) {
    @autoreleasepool { try {
        if (!std::isfinite(rate) || rate < 8000 || rate > 384000 || maximum < 1 || maximum > 65536) { errorText(error, capacity, "Invalid VST3 processing configuration."); return nullptr; }
        auto result = std::make_unique<STVST3Instance>(); std::string message;
        if (!result->initialize(path, uid, rate, maximum, message)) { errorText(error, capacity, message); return nullptr; }
        errorText(error, capacity, ""); return result.release();
    } catch (const std::exception& e) { errorText(error, capacity, e.what()); return nullptr; } }
}
void st_vst3_destroy(STVST3Instance *instance) { @autoreleasepool { delete instance; } }
void st_vst3_release_unused_modules(void) { auto& modules = moduleCache(); if (lifecycleTrace()) fprintf(stderr, "VST3 releaseUnused begin modules=%zu\n", modules.size()); for (auto it = modules.begin(); it != modules.end();) { if (lifecycleTrace()) fprintf(stderr, "VST3 module %s use_count=%ld\n", it->first.c_str(), it->second.use_count()); if (it->second.use_count() == 1) it = modules.erase(it); else ++it; } if (lifecycleTrace()) fprintf(stderr, "VST3 releaseUnused complete modules=%zu\n", modules.size()); }
int st_vst3_prepare(STVST3Instance *instance, double rate, uint32_t frames, char *error, size_t capacity) {
    if (!instance || !std::isfinite(rate) || rate < 8000 || rate > 384000 || frames < 1 || frames > 65536) { errorText(error, capacity, "Invalid VST3 prepare configuration."); return 0; }
    if (!instance->inputs.layoutMatches(instance->component, kInput) || !instance->outputs.layoutMatches(instance->component, kOutput)) { st_vst3_set_processing(instance, 0, nullptr, 0); errorText(error, capacity, "VST3 audio bus layout changed. Reload this component before processing."); return 0; }
    // AVAudioEngine can allocate the already configured adapter after state load.
    // Avoid resetting a successfully primed plugin when no format changed.
    if (instance->active && instance->sampleRate == rate && instance->maxFrames == frames) return st_vst3_set_processing(instance, 1, error, capacity);
    if (!st_vst3_set_processing(instance, 0, error, capacity)) return 0;
    if (instance->active) { if (instance->component->setActive(false) != kResultOk) { errorText(error, capacity, "VST3 could not deactivate before preparation."); return 0; } instance->active = false; }
    std::string message;
    if (!instance->inputs.configure(instance->component, kInput, frames, message) || !instance->outputs.configure(instance->component, kOutput, frames, message)) { errorText(error, capacity, message); return 0; }
    ProcessSetup setup{kRealtime, kSample32, int32(frames), rate};
    if (instance->processor->setupProcessing(setup) != kResultOk) { errorText(error, capacity, "VST3 prepare failed."); return 0; }
    if (!instance->inputs.reapplyActivation(instance->component, kInput, message) || !instance->outputs.reapplyActivation(instance->component, kOutput, message)) { errorText(error, capacity, message); return 0; }
    if (instance->component->setActive(true) != kResultOk) { errorText(error, capacity, "VST3 could not reactivate after preparation."); return 0; }
    instance->active = true; instance->sampleRate = rate; instance->maxFrames = frames;
    return st_vst3_set_processing(instance, 1, error, capacity);
}
int st_vst3_reset(STVST3Instance *instance, char *error, size_t capacity) {
    if (!instance) return 0;
    if (instance->parameterTrace.enabled) instance->parameterTrace.resets.fetch_add(1, std::memory_order_relaxed);
    // setProcessing(false) alone is not a MIDI panic in every instrument. Flush
    // tracked notes now, but leave UI edits queued until normal processing resumes:
    // a following stop/start may discard a component's internally deferred edits.
    if (!st_vst3_set_processing(instance, 1, error, capacity)) return 0;
    {
        std::array<STVST3MIDI, 16> panic{};
        for (int n = 0; n < 16; ++n) panic[n] = {0, uint8_t(0xb0 | n), 120, 0, -1};
        float left = 0, right = 0;
        if (!processBlock(instance, nullptr, 0, &left, &right, 1, panic.data(), uint32(panic.size()), nullptr, 0, nullptr, false, 0)) { errorText(error, capacity, "VST3 reset note-off flush failed."); return 0; }
    }
    if (!st_vst3_set_processing(instance, 0, error, capacity)) return 0;
    // Do not discard the UI queue: these are current control values, not stale
    // timeline events, and another edit may have arrived during the panic flush.
    for (auto& channel : instance->heldNotes) for (auto& note : channel) note.active = false;
    return st_vst3_set_processing(instance, 1, error, capacity);
}
int st_vst3_prime(STVST3Instance *instance, uint32_t frames, char *error, size_t capacity) {
    if (!instance || frames > 65536) { errorText(error, capacity, "Invalid VST3 priming request."); return 0; }
    const bool wasProcessing = instance->processing;
    if (!wasProcessing && !st_vst3_set_processing(instance, 1, error, capacity)) return 0;
    float left[512]{}, right[512]{}; uint32_t remaining = frames; bool succeeded = true;
    do {
        const auto count = std::min(remaining, std::min(uint32_t(512), instance->maxFrames));
        if (!st_vst3_process(instance, nullptr, nullptr, left, right, count, nullptr, 0, nullptr, 0, nullptr)) { succeeded = false; break; }
        remaining -= count;
    } while (remaining);
    st_vst3_sync_controller(instance);
    if (!wasProcessing && !st_vst3_set_processing(instance, 0, error, capacity)) return 0;
    if (!succeeded) errorText(error, capacity, "VST3 initialization processing failed.");
    return succeeded;
}
int st_vst3_set_processing(STVST3Instance *instance, int processing, char *error, size_t capacity) {
    if (!instance) return 0; bool enabled = processing != 0;
    if (enabled && !instance->active) { errorText(error, capacity, "VST3 component is inactive. Prepare or reload it before starting processing."); return 0; }
    if (instance->processing == enabled) return 1;
    if (!processingAccepted(instance->processor->setProcessing(enabled))) { errorText(error, capacity, "VST3 setProcessing failed."); return 0; }
    if (instance->parameterTrace.enabled) (enabled ? instance->parameterTrace.starts : instance->parameterTrace.stops).fetch_add(1, std::memory_order_relaxed);
    instance->processing = enabled; return 1;
}
int st_vst3_is_processing(STVST3Instance *instance) { return instance && instance->processing; }
uint32_t st_vst3_latency(STVST3Instance *instance) { return instance ? instance->processor->getLatencySamples() : 0; }
uint32_t st_vst3_tail(STVST3Instance *instance) { return instance ? instance->processor->getTailSamples() : 0; }
int st_vst3_is_instrument(STVST3Instance *instance) { return instance && instance->instrument; }
int st_vst3_input_bus_count(STVST3Instance *instance) { return instance ? int(instance->inputs.metadata.size()) : 0; }
int st_vst3_input_bus(STVST3Instance *instance, int32_t index, STVST3InputBus *result) {
    if (!instance || !result || index < 0 || size_t(index) >= instance->inputs.metadata.size()) return 0;
    const auto& info = instance->inputs.metadata[index]; *result = {};
    result->index = index; utf8(info.name, result->name, sizeof(result->name)); result->channels = info.channelCount; result->bus_type = info.busType;
    result->is_auxiliary = info.busType == kAux; result->is_active = instance->inputs.activation[index] != 0; return 1;
}
int32_t st_vst3_main_input_bus(STVST3Instance *instance) { return instance ? instance->inputs.mainIndex : -1; }
int st_vst3_configure_input_bus(STVST3Instance *instance, int32_t index, int32_t channels, int active, char *error, size_t capacity) {
    if (!instance || instance->processing) { errorText(error, capacity, "Stop VST3 processing before changing input bus activation."); return 0; }
    if (index < 0 || size_t(index) >= instance->inputs.metadata.size() || channels < 1 || channels > 2 || instance->inputs.metadata[index].channelCount != channels) { errorText(error, capacity, "The requested VST3 input bus or mono/stereo layout is unavailable."); return 0; }
    if (!instance->inputs.layoutMatches(instance->component, kInput)) { errorText(error, capacity, "VST3 input bus layout changed. Reload the component before reconnecting this input."); return 0; }
    const bool enabled = active != 0, previous = instance->inputs.activation[index] != 0;
    if (enabled == previous) return 1;
    const bool wasActive = instance->active;
    if (wasActive) {
        if (instance->component->setActive(false) != kResultOk) { errorText(error, capacity, "VST3 could not deactivate before changing the input bus."); return 0; }
        instance->active = false;
    }
    if (instance->component->activateBus(kAudio, kInput, index, enabled) != kResultOk) {
        const bool rolledBack = instance->component->activateBus(kAudio, kInput, index, previous) == kResultOk;
        if (rolledBack && wasActive) instance->active = instance->component->setActive(true) == kResultOk;
        errorText(error, capacity, rolledBack && (!wasActive || instance->active) ? "VST3 rejected the input bus activation; its previous configuration was retained." : "VST3 rejected the input bus activation and could not restore its previous configuration. Reload the component."); return 0;
    }
    if (wasActive && instance->component->setActive(true) != kResultOk) {
        const bool rolledBack = instance->component->activateBus(kAudio, kInput, index, previous) == kResultOk;
        if (rolledBack) instance->active = instance->component->setActive(true) == kResultOk;
        errorText(error, capacity, rolledBack && instance->active ? "VST3 could not reactivate the changed input bus; its previous configuration was retained." : "VST3 could not reactivate or restore the input bus. Reload the component."); return 0;
    }
    instance->active = wasActive;
    instance->inputs.activation[index] = enabled; return 1;
}
int st_vst3_parameter_count(STVST3Instance *instance) { return instance && instance->controller ? std::max(0, instance->controller->getParameterCount()) : 0; }
int st_vst3_parameter(STVST3Instance *instance, int index, STVST3Parameter *result) {
    if (!instance || !instance->controller || !result) return 0; ParameterInfo info{};
    if (instance->controller->getParameterInfo(index, info) != kResultOk) return 0;
    *result = {}; result->id = info.id; utf8(info.title, result->title, sizeof(result->title)); utf8(info.shortTitle, result->short_title, sizeof(result->short_title)); utf8(info.units, result->units, sizeof(result->units));
    result->steps = info.stepCount; result->flags = info.flags; result->default_value = info.defaultNormalizedValue; result->value = instance->controller->getParamNormalized(info.id); return 1;
}
static StudioTools::IHardwarePatchBaseline *baselineInterface(STVST3Instance *instance) {
    if (!instance || !instance->processor) return nullptr;
    StudioTools::IHardwarePatchBaseline *extension=nullptr;
    if (instance->processor->queryInterface(StudioTools::hardwarePatchBaselineIID,reinterpret_cast<void **>(&extension)) != kResultOk || !extension) return nullptr;
    if (extension->protocolVersion()!=1) { extension->release();return nullptr; }
    return extension;
}
uint32_t st_vst3_baseline_capabilities(STVST3Instance *instance) {
    auto *extension=baselineInterface(instance);if(!extension)return 0;
    const auto flags=extension->baselineCapabilities();extension->release();
    return (flags & StudioTools::stateIsBaseline) ? flags : 0;
}
enum class PendingEditFlush { flushed, processingFailed, changed, stalled };
// A UI edit may be pending before any audio callback occurs. VST3's zero-sample
// parameter flush updates the processor without advancing audio. Drain the
// original finite batch only; a moving edit target is never chased. The caller
// owns the drained state-access lease, so no render callback runs concurrently.
static PendingEditFlush flushPendingParameterEdits(STVST3Instance *instance, uint64_t generation, char *error, size_t capacity) {
    if (instance->pendingRead.load(std::memory_order_acquire) == instance->pendingWrite.load(std::memory_order_acquire)) return PendingEditFlush::flushed;
    bool wasProcessing = instance->processing;
    if (!wasProcessing && !st_vst3_set_processing(instance, 1, error, capacity)) return PendingEditFlush::processingFailed;
    float left = 0, right = 0;
    auto outcome = PendingEditFlush::flushed;
    auto remaining = st_vst3_pending_parameter_edits(instance);
    // The queue can contain more distinct controls (or points per control)
    // than one bounded process block accepts.
    while (remaining) {
        if (!st_vst3_process(instance, nullptr, nullptr, &left, &right, 0, nullptr, 0, nullptr, 0, nullptr)) { outcome = PendingEditFlush::stalled; break; }
        const auto next = st_vst3_pending_parameter_edits(instance);
        if (instance->editGeneration.load(std::memory_order_acquire) != generation) { outcome = PendingEditFlush::changed; break; }
        if (next >= remaining) { outcome = PendingEditFlush::stalled; break; }
        remaining = next;
    }
    if (!wasProcessing && !st_vst3_set_processing(instance, 0, error, capacity)) return PendingEditFlush::processingFailed;
    return outcome;
}
int st_vst3_capture_baseline(STVST3Instance *instance,char *error,size_t capacity) {
    // A control edit updates its baseline field immediately but reaches the live
    // processor on its next block. Capturing before that would promote stale
    // live values over the deliberate edit, so deliver pending edits with the
    // same zero-sample flush as state saving: no audio is rendered or advanced.
    if(instance && st_vst3_pending_parameter_edits(instance)>0) {
        switch(flushPendingParameterEdits(instance,instance->editGeneration.load(std::memory_order_acquire),error,capacity)) {
            case PendingEditFlush::flushed: break;
            case PendingEditFlush::processingFailed: return 0;
            case PendingEditFlush::changed: errorText(error,capacity,"VST3 controls changed while delivering edits. Retry Update base patch.");return 0;
            case PendingEditFlush::stalled: errorText(error,capacity,"Wait for pending parameter edits to reach the plug-in, then retry Update base patch.");return 0;
        }
    }
    auto *extension=baselineInterface(instance);
    if(!extension){errorText(error,capacity,"This plug-in does not support baseline-safe patch capture.");return 0;}
    const auto flags=extension->baselineCapabilities();
    if((flags & (StudioTools::stateIsBaseline|StudioTools::captureReady)) != (StudioTools::stateIsBaseline|StudioTools::captureReady)) {
        extension->release();errorText(error,capacity,"Receive or load a valid patch before updating its baseline.");return 0;
    }
    const bool accepted=extension->captureBaseline()==kResultOk;extension->release();
    errorText(error,capacity,accepted ? "" : "The plug-in declined baseline capture; its saved baseline was not updated.");return accepted;
}
int st_vst3_set_parameter(STVST3Instance *instance, uint32_t id, double value) {
    if (!instance || !instance->controller || !instance->parameterState(id) || !std::isfinite(value) || !instance->enqueue(id, value)) return 0;
    const auto normalized=std::clamp(value,0.,1.);
    const bool accepted=instance->controller->setParamNormalized(id,normalized)==kResultOk;
    // This control-side API is reserved for intentional UI edits. Realtime
    // automation enters process() directly and never reaches this extension.
    if(accepted)if(auto *extension=baselineInterface(instance)) {
        if(extension->baselineCapabilities() & StudioTools::manualParameterEdits)extension->setBaselineParameter(id,normalized);
        extension->release();
    }
    return accepted;
}
double st_vst3_parameter_value(STVST3Instance *instance, uint32_t id) { if (instance) if (auto state = instance->parameterState(id)) return state->value.load(std::memory_order_relaxed); return 0; }
uint64_t st_vst3_parameter_generation(STVST3Instance *instance) { return instance ? instance->parameterGeneration.load(std::memory_order_acquire) : 0; }
void st_vst3_sync_controller(STVST3Instance *instance) {
    if (!instance || !instance->controller) return;
    for (size_t n = 0; n < instance->parameterIDs.size(); ++n) if (instance->parameterStates[n].dirty.exchange(false, std::memory_order_acq_rel)) instance->controller->setParamNormalized(instance->parameterIDs[n], instance->parameterStates[n].value.load(std::memory_order_relaxed));
    // A program/state load may update the controller asynchronously after setState
    // returns. The SDK requires hosts to invalidate value caches on this callback.
    if (instance->controllerRefreshNeeded.exchange(false, std::memory_order_acq_rel)) instance->readControllerValues(false);
    if (instance->midiRefreshNeeded.exchange(false, std::memory_order_acq_rel)) instance->refreshMIDIMapping();
}
int st_vst3_parameter_text(STVST3Instance *instance, uint32_t id, double value, char *text, size_t capacity) { String128 string{}; if (!instance || !instance->controller || !std::isfinite(value) || instance->controller->getParamStringByValue(id, value, string) != kResultOk) return 0; utf8(string, text, capacity); return 1; }
uint32_t st_vst3_take_restart_flags(STVST3Instance *instance) { return instance ? instance->restartFlags.exchange(0, std::memory_order_acq_rel) : 0; }
int st_vst3_restart_component(STVST3Instance *instance, char *error, size_t capacity) {
    if (!instance) return 0;
    if (!instance->active) { errorText(error, capacity, "VST3 component is inactive. Prepare or reload it before restarting."); return 0; }
    const bool wasProcessing = instance->processing;
    if (!st_vst3_set_processing(instance, 0, error, capacity)) return 0;
    if (instance->component->setActive(false) != kResultOk) { errorText(error, capacity, "VST3 could not deactivate for its latency change."); return 0; }
    instance->active = false;
    for (auto& channel : instance->heldNotes) for (auto& note : channel) note.active = false;
    if (instance->component->setActive(true) != kResultOk) { errorText(error, capacity, "VST3 could not reactivate after its latency change. Reload this plug-in."); return 0; }
    instance->active = true;
    return wasProcessing ? st_vst3_set_processing(instance, 1, error, capacity) : 1;
}
uint64_t st_vst3_dropped_events(STVST3Instance *instance) { return instance ? instance->dropped.load(std::memory_order_relaxed) : 0; }
uint32_t st_vst3_pending_parameter_edits(STVST3Instance *instance) {
    if (!instance) return 0;
    return uint32_t((instance->pendingWrite.load(std::memory_order_acquire) + instance->pending.size() - instance->pendingRead.load(std::memory_order_acquire)) % instance->pending.size());
}
uint32_t st_vst3_required_state_flush_frames(STVST3Instance *instance) {
    return instance && instance->module && !strcmp(instance->uid, "5653544E6924486D6173736976652078") && instance->module->bundleVersion == "1.7.1" ? 4096 : 0;
}
const char *st_vst3_factory_version(STVST3Instance *instance) { return instance ? instance->pluginVersion : ""; }
const char *st_vst3_bundle_version(STVST3Instance *instance) { return instance && instance->module ? instance->module->bundleVersion.c_str() : ""; }
int st_vst3_state_capture_status(STVST3Instance *instance, STVST3StateCaptureStatus *status) {
    if (!instance || !status) return 0;
    *status = {}; status->edit_generation = instance->editGeneration.load(std::memory_order_acquire);
    status->pending_edits = st_vst3_pending_parameter_edits(instance);
    status->required_frames = st_vst3_required_state_flush_frames(instance);
    status->settle_milliseconds = status->required_frames ? 1000 : 0;
    status->dirty = status->required_frames && status->edit_generation != instance->capturedGeneration.load(std::memory_order_acquire);
    return 1;
}
int st_vst3_begin_state_capture(STVST3Instance *instance, int allowStoppedAudioAdvance, uint64_t *token, char *error, size_t capacity) {
    if (token) *token = 0;
    if (!instance || !token) { errorText(error, capacity, "No VST3 component or capture token output is available."); return 0; }
    if (instance->captureVendorCallActive) { errorText(error, capacity, "A VST3 state operation is already in progress."); return 0; }
    STVST3StateCaptureStatus status{}; st_vst3_state_capture_status(instance, &status);
    if (instance->failedStateRestore) { errorText(error, capacity, "VST3 state restoration did not complete. Restore a valid state or reload the component before saving."); return 0; }
    instance->invalidateCapture();
    if (!status.dirty) return 1;
    if (instance->captureEpochExhausted) { errorText(error, capacity, "VST3 capture identity is exhausted. Reload the component."); return 0; }
    const uint64_t attempt = instance->captureEpoch;
    instance->captureGeneration = status.edit_generation; instance->capturePhase = STVST3Instance::CapturePhase::processing;
    struct BeginGuard {
        STVST3Instance *host; uint64_t attempt;
        ~BeginGuard() { host->captureVendorCallActive = false; if (host->captureEpoch == attempt && host->capturePhase == STVST3Instance::CapturePhase::processing) host->capturePhase = STVST3Instance::CapturePhase::idle; }
    } guard{instance, attempt};
    if (status.pending_edits) {
        if (!allowStoppedAudioAdvance) { errorText(error, capacity, "VST3 control edits are waiting for normal audio processing. Retry capture after audio delivers them."); return 0; }
        for (const auto& channel : instance->heldNotes) for (const auto& note : channel) if (note.active) { errorText(error, capacity, "Release held VST3 notes or resume audio before saving pending edits."); return 0; }
        if (status.required_frames > instance->maxFrames) { errorText(error, capacity, "The VST3 block capacity is too small for state preparation."); return 0; }
        instance->captureVendorCallActive = true;
        const bool wasProcessing = instance->processing;
        if (!wasProcessing && !st_vst3_set_processing(instance, 1, error, capacity)) return 0;
        std::array<float, 4096> left{}, right{};
        const bool rendered = processBlock(instance, nullptr, 0, left.data(), right.data(), status.required_frames, nullptr, 0, nullptr, 0, nullptr, true, 0);
        if (!wasProcessing && !st_vst3_set_processing(instance, 0, error, capacity)) return 0;
        if (!rendered) { errorText(error, capacity, "VST3 could not deliver pending edits for state preparation."); return 0; }
    }
    if (instance->captureEpoch != attempt || instance->capturePhase != STVST3Instance::CapturePhase::processing || instance->editGeneration.load(std::memory_order_acquire) != status.edit_generation || st_vst3_pending_parameter_edits(instance)) {
        errorText(error, capacity, "VST3 controls or capture identity changed during preparation. Retry saving."); return 0;
    }
    instance->captureDeadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(status.settle_milliseconds);
    instance->capturePhase = STVST3Instance::CapturePhase::waiting; *token = attempt; return 1;
}
int st_vst3_authorize_state_capture(STVST3Instance *instance, uint64_t token, char *error, size_t capacity) {
    if (!instance || !token || instance->captureVendorCallActive || instance->captureEpochExhausted || instance->captureEpoch != token || instance->capturePhase != STVST3Instance::CapturePhase::waiting) { errorText(error, capacity, "The VST3 capture attempt is no longer current."); return 0; }
    if (instance->editGeneration.load(std::memory_order_acquire) != instance->captureGeneration || st_vst3_pending_parameter_edits(instance)) { errorText(error, capacity, "VST3 controls changed while waiting to save. Retry capture."); return 0; }
    if (std::chrono::steady_clock::now() < instance->captureDeadline) { errorText(error, capacity, "VST3 state has not completed its settling interval."); return 0; }
    instance->capturePhase = STVST3Instance::CapturePhase::authorized; return 1;
}
void st_vst3_cancel_state_capture(STVST3Instance *instance, uint64_t token) {
    if (instance && token && instance->captureEpoch == token && instance->capturePhase != STVST3Instance::CapturePhase::idle) instance->invalidateCapture();
}
int st_vst3_prepare_state_capture(STVST3Instance *instance, int allowStoppedAudioAdvance, char *error, size_t capacity) {
    if (!instance) { errorText(error, capacity, "No VST3 component is loaded."); return 0; }
    (void)allowStoppedAudioAdvance;
    if (instance->captureVendorCallActive) { errorText(error, capacity, "A VST3 state operation is already in progress."); return 0; }
    STVST3StateCaptureStatus status{}; st_vst3_state_capture_status(instance, &status);
    if (instance->failedStateRestore) { errorText(error, capacity, "VST3 state restoration did not complete. Restore a valid state or reload the component before saving."); return 0; }
    if (!status.required_frames) return 1;
    if (instance->captureEpochExhausted) { errorText(error, capacity, "VST3 capture identity is exhausted. Reload the component."); return 0; }
    if (status.pending_edits || (status.dirty && (instance->capturePhase != STVST3Instance::CapturePhase::authorized || instance->captureGeneration != status.edit_generation))) { errorText(error, capacity, "VST3 control edits require an authorized settled capture. Retry saving after preparation."); return 0; }
    return 1;
}
int st_vst3_watch_parameter(STVST3Instance *instance, uint32_t id) {
    if (!instance) return 0;
    bool found = instance->parameterState(id) != nullptr;
    instance->parameterTrace.reset(id, found); return found;
}
int st_vst3_parameter_diagnostics(STVST3Instance *instance, STVST3ParameterDiagnostics *result) {
    if (!instance || !result || !instance->parameterTrace.enabled) return 0;
    const auto& t = instance->parameterTrace; *result = {}; result->id = t.id;
    result->pending_ui_edits = st_vst3_pending_parameter_edits(instance);
    result->ui_queued = t.uiQueued.load(std::memory_order_relaxed); result->ui_dequeued = t.uiDequeued.load(std::memory_order_relaxed);
    result->automation_received = t.automationReceived.load(std::memory_order_relaxed); result->midi_received = t.midiReceived.load(std::memory_order_relaxed);
    result->offered_points = t.offeredPoints.load(std::memory_order_relaxed); result->read_points = t.readPoints.load(std::memory_order_relaxed); result->processes = t.processes.load(std::memory_order_relaxed);
    result->processing_starts = t.starts.load(std::memory_order_relaxed); result->processing_stops = t.stops.load(std::memory_order_relaxed);
    result->resets = t.resets.load(std::memory_order_relaxed); result->state_copies = t.copies.load(std::memory_order_relaxed); result->state_restores = t.restores.load(std::memory_order_relaxed);
    result->offered_process = t.offeredProcess.load(std::memory_order_relaxed); result->read_process = t.readProcess.load(std::memory_order_relaxed);
    result->last_ui_value = t.uiValue.load(std::memory_order_relaxed); result->last_dequeued_value = t.dequeuedValue.load(std::memory_order_relaxed);
    result->last_automation_value = t.automationValue.load(std::memory_order_relaxed); result->last_midi_value = t.midiValue.load(std::memory_order_relaxed);
    result->last_offered_value = t.offeredValue.load(std::memory_order_relaxed); result->last_read_value = t.readValue.load(std::memory_order_relaxed);
    result->last_offered_offset = t.offeredOffset.load(std::memory_order_relaxed); result->last_read_offset = t.readOffset.load(std::memory_order_relaxed);
    result->last_offered_frames = t.offeredFrames.load(std::memory_order_relaxed); result->last_read_frames = t.readFrames.load(std::memory_order_relaxed); result->last_process_result = t.processResult.load(std::memory_order_relaxed);
    return 1;
}
static int processBlock(STVST3Instance *instance, const STVST3InputBuffer *inputs, uint32_t inputCount, float *outLeft, float *outRight, uint32_t frames, const STVST3MIDI *midi, uint32_t midiCount, const STVST3ParameterChange *parameters, uint32_t parameterCount, const STVST3Transport *transport, bool consumePendingUI, int64_t systemTimeNs) {
    auto invalid = [&] { if (instance) { auto count = std::min(frames, instance->maxFrames); if (outLeft) std::fill_n(outLeft, count, 0.f); if (outRight) std::fill_n(outRight, count, 0.f); } return 0; };
    if (!instance || !outLeft || !outRight || frames > instance->maxFrames || !instance->processing || instance->outputs.buses.empty() || (!midi && midiCount) || (!parameters && parameterCount) || (!inputs && inputCount) || inputCount > 32) return invalid();
    uint32 supplied = 0;
    for (uint32 n = 0; n < inputCount; ++n) {
        const auto& input = inputs[n];
        if (input.bus_index < 0 || size_t(input.bus_index) >= instance->inputs.buses.size() || input.channel_count < 1 || input.channel_count > 2 || input.channel_count != instance->inputs.buses[input.bus_index].numChannels) return invalid();
        uint32 bit = uint32(1) << input.bus_index; if (supplied & bit) return invalid(); supplied |= bit;
    }
    instance->inputs.clear(frames, true); instance->outputs.clear(frames);
    for (uint32 n = 0; n < inputCount; ++n) {
        const auto& input = inputs[n];
        if (!instance->inputs.activation[input.bus_index] || !input.channels) continue;
        auto& bus = instance->inputs.buses[input.bus_index];
        for (int32 c = 0; c < input.channel_count; ++c) if (input.channels[c]) { std::copy_n(input.channels[c], frames, bus.channelBuffers32[c]); bus.silenceFlags &= ~(uint64(1) << c); }
    }
    instance->events.count = instance->outputEvents.count = instance->parameterChanges.count = instance->outputChanges.count = 0;
    auto& trace = instance->parameterTrace; instance->parameterChanges.trace = trace.enabled ? &trace : nullptr;
    if (trace.enabled) { trace.currentProcess = trace.processes.fetch_add(1, std::memory_order_relaxed) + 1; trace.currentFrames = int32(frames); }
    uint32 pendingCommitRead = 0; uint64 tracedDequeued = 0; double lastDequeued = 0;
    if (consumePendingUI) {
        uint32 read = instance->pendingRead.load(std::memory_order_relaxed), write = instance->pendingWrite.load(std::memory_order_acquire);
        while (read != write) { auto item = instance->pending[read]; if (!instance->parameterChanges.add(item.id, 0, item.value)) break; if (trace.matches(item.id)) { ++tracedDequeued; lastDequeued = item.value; } read = (read + 1) % instance->pending.size(); }
        pendingCommitRead = read;
    }
    for (uint32 n = 0; n < parameterCount; ++n) { const auto& item = parameters[n]; if (item.sample_offset < 0 || uint32(item.sample_offset) >= frames || !std::isfinite(item.value) || !instance->parameterChanges.add(item.id, item.sample_offset, std::clamp(item.value, 0., 1.))) instance->dropped.fetch_add(1, std::memory_order_relaxed); else { instance->cacheValue(item.id, std::clamp(item.value, 0., 1.), true); if (trace.matches(item.id)) { trace.automationReceived.fetch_add(1, std::memory_order_relaxed); trace.automationValue.store(std::clamp(item.value, 0., 1.), std::memory_order_relaxed); } } }
    for (uint32 n = 0; n < midiCount; ++n) {
        const auto& item = midi[n]; if (item.sample_offset < 0 || uint32(item.sample_offset) >= frames) { instance->dropped.fetch_add(1, std::memory_order_relaxed); continue; }
        auto type = item.status & 0xf0, channel = item.status & 0x0f; Event event{}; event.busIndex = 0; event.sampleOffset = item.sample_offset; event.flags = item.flags & ST_VST3_MIDI_SEQUENCED ? 0 : Event::kIsLive;
        if (type == 0x90 && item.data2 > 0) { event.type = Event::kNoteOnEvent; event.noteOn = {int16(channel), int16(item.data1 & 127), 0.f, float(item.data2 & 127) / 127.f, 0, item.note_id}; instance->heldNotes[channel][item.data1 & 127] = {true, item.note_id}; }
        else if (type == 0x80 || type == 0x90) { event.type = Event::kNoteOffEvent; event.noteOff = {int16(channel), int16(item.data1 & 127), float(item.data2 & 127) / 127.f, item.note_id, 0.f}; instance->heldNotes[channel][item.data1 & 127].active = false; }
        else if (type == 0xa0) { event.type = Event::kPolyPressureEvent; event.polyPressure = {int16(channel), int16(item.data1 & 127), float(item.data2 & 127) / 127.f, item.note_id}; }
        else {
            if (type == 0xb0 && (item.data1 == 120 || item.data1 == 123)) {
                auto sustain = instance->midiParameter[channel][64].load(std::memory_order_relaxed); if (sustain != kNoParamId) { instance->parameterChanges.add(sustain, item.sample_offset, 0); if (trace.matches(sustain)) { trace.midiReceived.fetch_add(1, std::memory_order_relaxed); trace.midiValue.store(0, std::memory_order_relaxed); } }
                for (int16 pitch = 0; pitch < 128; ++pitch) { auto& held = instance->heldNotes[channel][pitch]; if (held.active) { Event off{}; off.busIndex = 0; off.sampleOffset = item.sample_offset; off.type = Event::kNoteOffEvent; off.noteOff = {int16(channel), pitch, 0.f, held.id, 0.f}; if (instance->events.addEvent(off) != kResultOk) instance->dropped.fetch_add(1, std::memory_order_relaxed); held.active = false; } }
            }
            int cc = type == 0xb0 ? item.data1 & 127 : type == 0xd0 ? 128 : type == 0xe0 ? 129 : -1;
            double value = type == 0xe0 ? double((item.data1 & 127) | ((item.data2 & 127) << 7)) / 16383. : double(type == 0xd0 ? item.data1 & 127 : item.data2 & 127) / 127.;
            auto parameter = cc >= 0 ? instance->midiParameter[channel][cc].load(std::memory_order_relaxed) : kNoParamId;
            if (parameter != kNoParamId) { if (!instance->parameterChanges.add(parameter, item.sample_offset, value)) instance->dropped.fetch_add(1, std::memory_order_relaxed); if (trace.matches(parameter)) { trace.midiReceived.fetch_add(1, std::memory_order_relaxed); trace.midiValue.store(value, std::memory_order_relaxed); } }
            continue;
        }
        if (instance->events.addEvent(event) != kResultOk) instance->dropped.fetch_add(1, std::memory_order_relaxed);
    }
    ProcessContext context{}; context.sampleRate = instance->sampleRate;
    context.tempo = 120; context.timeSigNumerator = context.timeSigDenominator = 4;
    // Host clock (mach time, ns) when this block's first sample is handed out: plug-ins that send
    // MIDI to hardware time it from here, which stays exact when a block is split into sub-blocks.
    if (systemTimeNs > 0) { context.systemTime = systemTimeNs; context.state |= ProcessContext::kSystemTimeValid; }
    if (transport) {
        context.projectTimeSamples = transport->sample_position;
        const bool explicitValidity = (transport->valid_fields & ST_VST3_TRANSPORT_EXPLICIT) != 0;
        const auto valid = [&](uint32_t bit) { return explicitValidity ? (transport->valid_fields & bit) != 0 :
            (bit & (ST_VST3_TRANSPORT_MUSIC_VALID | ST_VST3_TRANSPORT_TEMPO_VALID |
                    ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID | ST_VST3_TRANSPORT_TRANSPORT_VALID)) != 0; };
        const auto musicalPosition = [](double value) { return std::isfinite(value) && std::abs(value) < 1e12; };
        if (valid(ST_VST3_TRANSPORT_MUSIC_VALID) && musicalPosition(transport->beat_position)) {
            context.projectTimeMusic = transport->beat_position; context.state |= ProcessContext::kProjectTimeMusicValid;
        }
        if (valid(ST_VST3_TRANSPORT_TEMPO_VALID) && std::isfinite(transport->tempo) && transport->tempo > 0 && transport->tempo <= 10000) {
            context.tempo = transport->tempo; context.state |= ProcessContext::kTempoValid;
        }
        if (valid(ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID) && transport->numerator > 0 && transport->denominator > 0) {
            context.timeSigNumerator = transport->numerator; context.timeSigDenominator = transport->denominator;
            context.state |= ProcessContext::kTimeSigValid;
        }
        if (valid(ST_VST3_TRANSPORT_CONTINUOUS_VALID)) {
            context.continousTimeSamples = transport->continuous_sample_position; context.state |= ProcessContext::kContTimeValid;
        }
        if (valid(ST_VST3_TRANSPORT_BAR_VALID) && musicalPosition(transport->bar_start_beat) &&
            (!(context.state & ProcessContext::kProjectTimeMusicValid) || transport->bar_start_beat <= context.projectTimeMusic)) {
            context.barPositionMusic = transport->bar_start_beat; context.state |= ProcessContext::kBarPositionValid;
        }
        if (valid(ST_VST3_TRANSPORT_CYCLE_VALID) && musicalPosition(transport->cycle_start_beat) &&
            musicalPosition(transport->cycle_end_beat) && transport->cycle_start_beat < transport->cycle_end_beat) {
            context.cycleStartMusic = transport->cycle_start_beat; context.cycleEndMusic = transport->cycle_end_beat;
            context.state |= ProcessContext::kCycleValid;
        }
        if (valid(ST_VST3_TRANSPORT_TRANSPORT_VALID)) {
            if (transport->playing) context.state |= ProcessContext::kPlaying;
            if (explicitValidity && transport->recording) context.state |= ProcessContext::kRecording;
            if (explicitValidity && transport->cycling && (context.state & ProcessContext::kCycleValid)) context.state |= ProcessContext::kCycleActive;
        }
    }
    ProcessData data{}; data.processMode = kRealtime; data.symbolicSampleSize = kSample32; data.numSamples = int32(frames); data.numInputs = int32(instance->inputs.buses.size()); data.numOutputs = int32(instance->outputs.buses.size()); data.inputs = instance->inputs.buses.data(); data.outputs = instance->outputs.buses.data(); data.inputEvents = &instance->events; data.outputEvents = &instance->outputEvents; data.inputParameterChanges = &instance->parameterChanges; data.outputParameterChanges = &instance->outputChanges; data.processContext = &context;
    if (trace.enabled) for (int32 n = 0; n < instance->parameterChanges.count; ++n) {
        const auto& queue = instance->parameterChanges.queues[n];
        if (trace.matches(queue.id) && queue.count) {
            const auto& point = queue.points[queue.count - 1]; trace.offeredPoints.fetch_add(queue.count, std::memory_order_relaxed);
            trace.offeredValue.store(point.value, std::memory_order_relaxed); trace.offeredOffset.store(point.offset, std::memory_order_relaxed);
            trace.offeredFrames.store(int32(frames), std::memory_order_relaxed); trace.offeredProcess.store(trace.currentProcess, std::memory_order_relaxed);
        }
    }
    tresult result = instance->processor->process(data);
    // Release entries only when the processor accepts the block. A failed call
    // leaves edits retryable without rewinding, copying or re-enqueuing the SPSC
    // ring. The producer can append while these original entries remain owned.
    if (result == kResultOk && consumePendingUI) {
        instance->pendingRead.store(pendingCommitRead, std::memory_order_release);
        if (tracedDequeued) { trace.uiDequeued.fetch_add(tracedDequeued, std::memory_order_relaxed); trace.dequeuedValue.store(lastDequeued, std::memory_order_relaxed); }
    }
    if (trace.enabled) trace.processResult.store(result, std::memory_order_relaxed);
    for (int32 n = 0; n < instance->outputChanges.count; ++n) { const auto& queue = instance->outputChanges.queues[n]; if (queue.count) instance->cacheValue(queue.id, queue.points[queue.count - 1].value, true); }
    auto& bus = instance->outputs.buses[0];
    if (result == kResultOk && bus.numChannels > 0) { if (bus.silenceFlags & 1) std::fill_n(outLeft, frames, 0.f); else std::copy_n(bus.channelBuffers32[0], frames, outLeft); if (bus.numChannels > 1) { if (bus.silenceFlags & 2) std::fill_n(outRight, frames, 0.f); else std::copy_n(bus.channelBuffers32[1], frames, outRight); } else std::copy_n(outLeft, frames, outRight); }
    else { std::fill_n(outLeft, frames, 0.f); std::fill_n(outRight, frames, 0.f); }
    return result == kResultOk;
}
static int64_t hostNanoseconds() {
    static const mach_timebase_info_data_t timebase = [] { mach_timebase_info_data_t value{}; mach_timebase_info(&value); return value; }();
    return int64_t(double(mach_absolute_time()) * timebase.numer / timebase.denom);
}
/* Most plug-in frameworks (JUCE among them) apply only the last value of each parameter per
   process call, at the block start. Instruments are therefore processed in sub-blocks that
   begin at each automated parameter change, so a stepped parameter takes effect on its sample;
   each sub-block carries its own host time, so MIDI a plug-in forwards to hardware is timed
   from it rather than from the (shared) moment the whole block was processed. */
static int processSplit(STVST3Instance *instance, const STVST3InputBuffer *inputs, uint32_t inputCount, float *outLeft, float *outRight, uint32_t frames, const STVST3MIDI *midi, uint32_t midiCount, const STVST3ParameterChange *parameters, uint32_t parameterCount, const STVST3Transport *transport) {
    const int64_t base = hostNanoseconds();
    const auto whole = [&] { return processBlock(instance, inputs, inputCount, outLeft, outRight, frames, midi, midiCount, parameters, parameterCount, transport, true, base); };
    if (!instance || !instance->instrument || !parameters || parameterCount == 0 || inputCount > 32 || !outLeft || !outRight
        || midiCount > instance->splitMIDI.size() || parameterCount > instance->splitParameters.size()) return whole();
    constexpr int maximumCuts = 32;
    uint32_t cuts[maximumCuts + 2]; int count = 0; cuts[count++] = 0;
    for (uint32_t n = 0; n < parameterCount && count <= maximumCuts; ++n) {
        const auto offset = parameters[n].sample_offset;
        if (offset <= 0 || uint32_t(offset) >= frames) continue;
        int at = count; while (at > 1 && cuts[at - 1] > uint32_t(offset)) --at;
        if (cuts[at - 1] == uint32_t(offset)) continue;
        std::memmove(cuts + at + 1, cuts + at, size_t(count - at) * sizeof(uint32_t)); cuts[at] = uint32_t(offset); ++count;
    }
    if (count == 1) return whole();
    cuts[count] = frames;
    int result = 1;
    for (int segment = 0; segment < count; ++segment) {
        const uint32_t start = cuts[segment], end = cuts[segment + 1], length = end - start;
        const float *channels[32][2]{}; STVST3InputBuffer shifted[32];
        for (uint32_t n = 0; n < inputCount; ++n) {
            shifted[n] = inputs[n];
            for (int32_t c = 0; c < std::min<int32_t>(2, inputs[n].channel_count); ++c) channels[n][c] = inputs[n].channels && inputs[n].channels[c] ? inputs[n].channels[c] + start : nullptr;
            shifted[n].channels = inputs[n].channels ? channels[n] : nullptr;
        }
        uint32_t midiInSegment = 0, parametersInSegment = 0;
        for (uint32_t n = 0; n < midiCount; ++n) {
            const auto offset = midi[n].sample_offset;
            if (offset >= int32_t(start) && (offset < int32_t(end) || (segment == count - 1 && offset >= int32_t(end)))) {
                auto item = midi[n]; item.sample_offset = offset - int32_t(start); instance->splitMIDI[midiInSegment++] = item;
            } else if (offset < 0 && segment == 0) instance->splitMIDI[midiInSegment++] = midi[n];
        }
        for (uint32_t n = 0; n < parameterCount; ++n) {
            const auto offset = std::max<int32_t>(0, parameters[n].sample_offset);
            if (offset >= int32_t(start) && (offset < int32_t(end) || (segment == count - 1 && offset >= int32_t(end)))) {
                auto item = parameters[n]; item.sample_offset = offset - int32_t(start); instance->splitParameters[parametersInSegment++] = item;
            }
        }
        STVST3Transport moved{}; const STVST3Transport *segmentTransport = transport;
        if (transport && start > 0) {
            moved = *transport; moved.sample_position += int64_t(start); moved.continuous_sample_position += int64_t(start);
            if (std::isfinite(moved.tempo) && moved.tempo > 0) moved.beat_position += double(start) * moved.tempo / 60.0 / instance->sampleRate;
            segmentTransport = &moved;
        }
        const int64_t time = base + int64_t(double(start) * 1e9 / instance->sampleRate);
        result &= processBlock(instance, inputCount ? shifted : nullptr, inputCount, outLeft + start, outRight + start, length,
                               midiInSegment ? instance->splitMIDI.data() : nullptr, midiInSegment,
                               parametersInSegment ? instance->splitParameters.data() : nullptr, parametersInSegment, segmentTransport, segment == 0, time);
    }
    return result;
}
int st_vst3_process(STVST3Instance *instance, const float *left, const float *right, float *outLeft, float *outRight, uint32_t frames, const STVST3MIDI *midi, uint32_t midiCount, const STVST3ParameterChange *parameters, uint32_t parameterCount, const STVST3Transport *transport) {
    const auto main = instance ? instance->inputs.mainIndex : -1;
    const float *channels[2]{left, right};
    STVST3InputBuffer input{main, main >= 0 ? instance->inputs.buses[main].numChannels : 0, channels};
    return processSplit(instance, main >= 0 ? &input : nullptr, main >= 0 ? 1 : 0, outLeft, outRight, frames, midi, midiCount, parameters, parameterCount, transport);
}
int st_vst3_process_indexed(STVST3Instance *instance, const STVST3InputBuffer *inputs, uint32_t inputCount, float *outLeft, float *outRight, uint32_t frames, const STVST3MIDI *midi, uint32_t midiCount, const STVST3ParameterChange *parameters, uint32_t parameterCount, const STVST3Transport *transport) {
    return processSplit(instance, inputs, inputCount, outLeft, outRight, frames, midi, midiCount, parameters, parameterCount, transport);
}
static int copyState(STVST3Instance *instance, void **bytes, size_t *size, char *error, size_t capacity, bool concurrent);
int st_vst3_copy_state(STVST3Instance *instance, void **bytes, size_t *size, char *error, size_t capacity) {
    return copyState(instance, bytes, size, error, capacity, false);
}
int st_vst3_copy_state_concurrent(STVST3Instance *instance, void **bytes, size_t *size, char *error, size_t capacity) {
    return copyState(instance, bytes, size, error, capacity, true);
}
static int copyState(STVST3Instance *instance, void **bytes, size_t *size, char *error, size_t capacity, bool concurrent) {
    if (bytes) *bytes = nullptr; if (size) *size = 0;
    if (!instance || !bytes || !size) return 0;
    if (!st_vst3_prepare_state_capture(instance, 0, error, capacity)) return 0;
    // While the render thread owns process(), the host must never run its own
    // flush block: the UI-edit ring has exactly one consumer. Edits still in the
    // ring reach the processor on the next ordinary block; the caller retries.
    if (concurrent && st_vst3_pending_parameter_edits(instance)) { errorText(error, capacity, "VST3 control edits are waiting for the next audio block."); return -1; }
    const bool profiled = st_vst3_required_state_flush_frames(instance) != 0;
    const auto captureGeneration = instance->editGeneration.load(std::memory_order_acquire), captureEpoch = instance->captureEpoch;
    if (profiled) instance->capturePhase = STVST3Instance::CapturePhase::copying;
    instance->captureVendorCallActive = true;
    struct CopyGuard {
        STVST3Instance *host; uint64_t epoch;
        ~CopyGuard() { host->captureVendorCallActive = false; if (host->captureEpoch == epoch && host->capturePhase == STVST3Instance::CapturePhase::copying) host->capturePhase = STVST3Instance::CapturePhase::idle; }
    } captureGuard{instance, captureEpoch};
    if (instance->parameterTrace.enabled) instance->parameterTrace.copies.fetch_add(1, std::memory_order_relaxed);
    // A UI edit may be saved before any audio callback occurs. The zero-sample
    // flush updates the processor before serializing its component state.
    switch (concurrent ? PendingEditFlush::flushed : flushPendingParameterEdits(instance, captureGeneration, error, capacity)) {
        case PendingEditFlush::flushed: break;
        case PendingEditFlush::processingFailed: return 0;
        case PendingEditFlush::changed: errorText(error, capacity, "VST3 controls changed while delivering edits for saving. Retry capture."); return 0;
        case PendingEditFlush::stalled: errorText(error, capacity, "VST3 could not finish delivering parameter edits before saving. Pending edits were retained."); return 0;
    }
    st_vst3_sync_controller(instance);
    try { MemoryStream component, controller; if (instance->component->getState(&component) != kResultOk) { errorText(error, capacity, "VST3 component did not provide its state."); return 0; }
        if (instance->controller) {
            auto result = instance->controller->getState(&controller);
            // The SDK's base controller legitimately has no separate state.
            // Partial bytes from a failed/unsupported save are not restorable.
            if (result != kResultOk && !(result == kNotImplemented && controller.bytes.empty())) {
                errorText(error, capacity, "VST3 controller did not provide a complete state; the project snapshot was not saved."); return 0;
            }
        }
        if (profiled && (instance->captureEpochExhausted || instance->captureEpoch != captureEpoch || instance->capturePhase != STVST3Instance::CapturePhase::copying || instance->editGeneration.load(std::memory_order_acquire) != captureGeneration || st_vst3_pending_parameter_edits(instance))) {
            errorText(error, capacity, "VST3 controls or capture identity changed while collecting state. The snapshot was not saved."); return 0;
        }
        if (component.bytes.size() + controller.bytes.size() > stateLimit - 56) { errorText(error, capacity, "VST3 state exceeds 256 MiB."); return 0; }
        size_t length = 56 + component.bytes.size() + controller.bytes.size(); auto result = static_cast<uint8_t *>(calloc(1, length)); if (!result) return 0;
        memcpy(result, "STV3\1\0\0\0", 8); memcpy(result + 8, instance->uid, 32); uint64_t a = component.bytes.size(), b = controller.bytes.size(); memcpy(result + 40, &a, 8); memcpy(result + 48, &b, 8);
        if (a) memcpy(result + 56, component.bytes.data(), a); if (b) memcpy(result + 56 + a, controller.bytes.data(), b);
        if (profiled) instance->capturedGeneration.store(captureGeneration, std::memory_order_release);
        *bytes = result; *size = length; return 1;
    } catch (const std::exception& e) { errorText(error, capacity, e.what()); return 0; }
}
static int restoreComponentState(STVST3Instance *instance, const void *bytes, size_t size, char *error, size_t capacity, bool *componentRejected) {
    if (componentRejected) *componentRejected = false;
    if (!instance || !bytes || size > stateLimit || size == 0) { errorText(error, capacity, "Invalid VST3 component state size."); return 0; }
    if (instance->captureVendorCallActive) { errorText(error, capacity, "A VST3 state operation is already in progress."); return 0; }
    instance->invalidateCapture();
    instance->failedStateRestore = true;
    instance->captureVendorCallActive = true;
    struct RestoreGuard { STVST3Instance *host; ~RestoreGuard() { host->captureVendorCallActive = false; } } restoreGuard{instance};
    if (instance->parameterTrace.enabled) instance->parameterTrace.restores.fetch_add(1, std::memory_order_relaxed);
    try { MemoryStream state(bytes, size); bool wasProcessing = instance->processing; if (wasProcessing && !st_vst3_set_processing(instance, 0, error, capacity)) return 0;
        bool wasActive = instance->active;
        if (wasActive) { if (instance->component->setActive(false) != kResultOk) { errorText(error, capacity, "VST3 could not suspend for state restore."); return 0; } instance->active = false; }
        auto result = instance->component->setState(&state);
        if (!instance->inputs.layoutMatches(instance->component, kInput) || !instance->outputs.layoutMatches(instance->component, kOutput)) {
            errorText(error, capacity, "VST3 state changed the audio bus layout. Reload this component before processing."); return 0;
        }
        std::string busError;
        if (!instance->inputs.reapplyActivation(instance->component, kInput, busError) || !instance->outputs.reapplyActivation(instance->component, kOutput, busError)) { errorText(error, capacity, busError); return 0; }
        if (result == kResultOk && instance->controller) {
            state.position = 0; const auto synchronized = instance->controller->setComponentState(&state);
            if (synchronized != kResultOk && synchronized != kNotImplemented) { errorText(error, capacity, "VST3 controller rejected component-state synchronization. Restore a valid state or reload the component."); return 0; }
            instance->refreshMIDIMapping(); instance->readControllerValues();
        }
        instance->pendingRead.store(instance->pendingWrite.load(std::memory_order_acquire), std::memory_order_release);
        for (auto& channel : instance->heldNotes) for (auto& note : channel) note.active = false;
        if (wasActive) { if (instance->component->setActive(true) != kResultOk) { errorText(error, capacity, "VST3 could not reactivate after state restore."); return 0; } instance->active = true; }
        if (wasProcessing && !st_vst3_set_processing(instance, 1, error, capacity)) return 0;
        if (result != kResultOk) { if (componentRejected) *componentRejected = true; errorText(error, capacity, "The VST3 plugin rejected this component state. This preset may require a different plugin format or version."); return 0; }
        instance->failedStateRestore = false; return 1;
    } catch (const std::exception& e) { errorText(error, capacity, e.what()); return 0; }
}
int st_vst3_restore_component_state(STVST3Instance *instance, const void *bytes, size_t size, char *error, size_t capacity) {
    return restoreComponentState(instance, bytes, size, error, capacity, nullptr);
}
int st_vst3_restore_vst2_bank(STVST3Instance *instance, uint32_t uniqueID, int32_t program,
                            const void *bytes, size_t size, int *usedRawFallback, char *error, size_t capacity) {
    if (usedRawFallback) *usedRawFallback = 0;
    if (!instance || !bytes || !size || size > 32u * 1024u * 1024u || !uniqueID || program < 0) {
        errorText(error, capacity, "Invalid legacy VST2 bank identity, program or size."); return 0;
    }
    char predecessor[15]{}; snprintf(predecessor, sizeof(predecessor), "565354%08X", uniqueID);
    if (strncmp(instance->uid, predecessor, 14)) {
        errorText(error, capacity, "This VST3 class does not encode the exact legacy VST2 identity."); return 0;
    }
    if (instance->captureVendorCallActive) { errorText(error, capacity, "A VST3 state operation is already in progress."); return 0; }
    try {
        VST3::Vst2xState legacy;
        // Preserve the complete four-byte ID, including its signed bit.
        memcpy(&legacy.fxUniqueID, &uniqueID, sizeof(uniqueID));
        legacy.currentProgram = program;
        const auto *payload = static_cast<const int8_t *>(bytes);
        legacy.chunk.assign(payload, payload + size);
        MemoryStream wrapped;
        // This is Steinberg's legacy FXB representation with bypass metadata,
        // also used by Maschine's VST2-to-VST3 serializer. Vendor bytes stay opaque.
        if (!VST3::writeVst2State(legacy, wrapped, true) || wrapped.bytes.empty()) {
            errorText(error, capacity, "The legacy VST2 bank could not be wrapped for VST3 restoration."); return 0;
        }
        bool componentRejected = false;
        if (restoreComponentState(instance, wrapped.bytes.data(), wrapped.bytes.size(), error, capacity, &componentRejected)) {
            errorText(error, capacity, ""); return 1;
        }
        // A controller/layout/lifecycle failure is not evidence that a second
        // encoding is safe. Retry only a rejected component stream, once.
        if (!componentRejected) return 0;
        if (!restoreComponentState(instance, bytes, size, error, capacity, nullptr)) return 0;
        if (usedRawFallback) *usedRawFallback = 1;
        errorText(error, capacity, ""); return 1;
    } catch (const std::exception& e) { errorText(error, capacity, e.what()); return 0; }
}
int st_vst3_restore_state(STVST3Instance *instance, const void *bytes, size_t size, char *error, size_t capacity) {
    if (!instance || !bytes || size < 56 || size > stateLimit) { errorText(error, capacity, "Invalid VST3 host state."); return 0; }
    const auto data = static_cast<const uint8_t *>(bytes); uint64_t a = 0, b = 0; memcpy(&a, data + 40, 8); memcpy(&b, data + 48, 8);
    if (memcmp(data, "STV3\1\0\0\0", 8) || memcmp(data + 8, instance->uid, 32) || a > size - 56 || b != size - 56 - a) { errorText(error, capacity, "VST3 state identity or bounds do not match this plugin."); return 0; }
    if (!st_vst3_restore_component_state(instance, data + 56, size_t(a), error, capacity)) return 0;
    try { if (instance->controller && b) {
        instance->failedStateRestore = true;
        instance->captureVendorCallActive = true;
        struct ControllerRestoreGuard { STVST3Instance *host; ~ControllerRestoreGuard() { host->captureVendorCallActive = false; } } restoreGuard{instance};
        MemoryStream state(data + 56 + a, size_t(b)); if (instance->controller->setState(&state) != kResultOk) { errorText(error, capacity, "VST3 controller state restore failed."); return 0; } instance->readControllerValues(); instance->failedStateRestore = false;
    } return 1; } catch (const std::exception& e) { errorText(error, capacity, e.what()); return 0; }
}
void st_vst3_free(void *pointer) { free(pointer); }
static int createEditorView(STVST3Instance *instance, int *width, int *height, char *error, size_t capacity) {
    st_vst3_detach_editor(instance);
    instance->view = instance->controller->createView(ViewType::kEditor);
    if (!instance->view || instance->view->isPlatformTypeSupported(kPlatformTypeNSView) != kResultOk) { st_vst3_detach_editor(instance); errorText(error, capacity, "This VST3 has no Cocoa editor. Use the parameter controls."); return 0; }
    ViewRect rect{}; if (instance->view->getSize(&rect) == kResultOk) { if (width) *width = rect.getWidth(); if (height) *height = rect.getHeight(); }
    return 1;
}
int st_vst3_create_editor(STVST3Instance *instance, int *width, int *height, char *error, size_t capacity) {
    if (!instance || !instance->controller) return 0;
    return createEditorView(instance, width, height, error, capacity);
}
int st_vst3_attach_editor(STVST3Instance *instance, void *parent, int *width, int *height, char *error, size_t capacity) {
    if (!instance || !instance->controller || !parent) return 0;
    // Reuse a view from st_vst3_create_editor; an attached one is recreated for the new parent.
    if ((!instance->view || instance->parent) && !createEditorView(instance, nullptr, nullptr, error, capacity)) return 0;
    instance->parent = (__bridge NSView *)parent; instance->view->setFrame(instance);
    if (instance->view->attached(parent, kPlatformTypeNSView) != kResultOk) { st_vst3_detach_editor(instance); errorText(error, capacity, "VST3 editor attachment failed."); return 0; }
    ViewRect rect{}; if (instance->view->getSize(&rect) == kResultOk) { if (width) *width = rect.getWidth(); if (height) *height = rect.getHeight(); }
    return 1;
}
void st_vst3_detach_editor(STVST3Instance *instance) { if (instance && instance->view) { if (instance->parent) instance->view->removed(); instance->view->setFrame(nullptr); instance->view->release(); instance->view = nullptr; instance->parent = nil; } }
}
