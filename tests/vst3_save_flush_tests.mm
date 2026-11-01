// Exact-profile capture tests use original in-memory components only.
#include "../Sources/VST3Host/VST3Host.mm"
#include "vst3_state_fixture.h"
#include <cassert>
#include <cstdio>
#include <functional>
#include <new>
static thread_local bool measuringRender = false;
static thread_local size_t renderAllocations = 0;
void *operator new(size_t size) { if (measuringRender) ++renderAllocations; if (void *p = malloc(size ? size : 1)) return p; throw std::bad_alloc(); }
void *operator new[](size_t size) { return ::operator new(size); }
void operator delete(void *p) noexcept { free(p); }
void operator delete[](void *p) noexcept { free(p); }
class FixtureCaptureComponent final : public StateComponent {
public:
    std::function<void()> duringSave;
    bool failSave = false, failRestore = false;
    tresult PLUGIN_API getState(IBStream *stream) override {
        if (duringSave) duringSave();
        if (failSave) { stream->write(&value, 3, nullptr); return kResultFalse; }
        return StateComponent::getState(stream);
    }
    tresult PLUGIN_API setState(IBStream *stream) override {
        auto result = StateComponent::setState(stream);
        return failRestore ? kResultFalse : result;
    }
};
class CaptureController final : public StateController {
public:
    std::function<void()> duringSave;
    tresult PLUGIN_API getState(IBStream *stream) override { if (duringSave) duringSave(); return StateController::getState(stream); }
};
class SaveProcessor final : public IAudioProcessor {
public:
    FixtureCaptureComponent& component;
    uint32_t calls = 0, lastFrames = 0, starts = 0, stops = 0;
    uint64_t points = 0;
    bool failStart = false, failProcess = false, deferred = true, staged = false;
    double stagedValue = 0;
    std::function<void()> duringProcess;
    int noteOns = 0, firstNoteOffset = -1;
    explicit SaveProcessor(FixtureCaptureComponent& value) : component(value) {}
    void eventLoopProgress() { if (staged) { component.value = stagedValue; staged = false; } }
    QUERY_ONE(IAudioProcessor) EMBEDDED_REFS
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32, SpeakerArrangement *, int32) override { return kResultOk; }
    tresult PLUGIN_API getBusArrangement(BusDirection, int32, SpeakerArrangement& arrangement) override { arrangement = SpeakerArr::kStereo; return kResultOk; }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool enabled) override { if (enabled) { ++starts; if (failStart) return kResultFalse; } else ++stops; return kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        ++calls; lastFrames = data.numSamples;
        for (int32 i = 0; i < data.inputParameterChanges->getParameterCount(); ++i) {
            auto queue = data.inputParameterChanges->getParameterData(i);
            for (int32 p = 0; p < queue->getPointCount(); ++p) {
                int32 offset = 0; double value = 0;
                if (queue->getPoint(p, offset, value) == kResultOk) {
                    ++points;
                    if (queue->getParameterId() == 0 && !failProcess && (data.numSamples >= 4096 || !deferred)) {
                        if (deferred) { stagedValue = value; staged = true; } else component.value = value;
                    }
                }
            }
        }
        if (duringProcess) duringProcess();
        if (failProcess) return kResultFalse;
        for (int32 i = 0; i < data.inputEvents->getEventCount(); ++i) { Event event{}; if (data.inputEvents->getEvent(i,event) == kResultOk && event.type == Event::kNoteOnEvent) { ++noteOns; firstNoteOffset = event.sampleOffset; } }
        for (int32 n = 0; n < data.numSamples; ++n) for (int32 c = 0; c < data.outputs[0].numChannels; ++c) data.outputs[0].channelBuffers32[c][n] = float(component.value);
        return kResultOk;
    }
};
struct Fixture {
    FixtureCaptureComponent component; CaptureController controller; SaveProcessor processor{component};
    std::array<float,4096> left{}, right{}; float *channels[2]{left.data(),right.data()};
    std::unique_ptr<STVST3Instance> host = std::make_unique<STVST3Instance>();
    Fixture() {
        component.value = controller.value = .0740625;
        host->component = &component; host->controller = &controller; host->processor = &processor;
        host->maxFrames = 4096; host->active = true; host->processing = false;
        host->outputs.buses.resize(1); host->outputs.buses[0].numChannels = 2; host->outputs.buses[0].channelBuffers32 = channels;
        host->parameterIDs = {0}; host->parameterStates = std::make_unique<STVST3Instance::ParameterState[]>(1);
        copyText(host->uid,"5653544E6924486D6173736976652078"); copyText(host->pluginVersion,"1.7.1 (R0)");
        host->module = std::make_shared<Module>(); host->module->bundleVersion = "1.7.1";
    }
    STVST3StateCaptureStatus status() { STVST3StateCaptureStatus value{}; assert(st_vst3_state_capture_status(host.get(),&value)); return value; }
    bool render(const STVST3MIDI *midi = nullptr, uint32_t count = 0) {
        assert(st_vst3_set_processing(host.get(),1,nullptr,0));
        measuringRender = true;
        const bool result = st_vst3_process(host.get(),nullptr,nullptr,left.data(),right.data(),4096,midi,count,nullptr,0,nullptr);
        measuringRender = false; return result;
    }
    void progress() { processor.eventLoopProgress(); host->captureDeadline = std::chrono::steady_clock::now() - std::chrono::milliseconds(1); }
    uint64_t begin(bool stopped = true) { uint64_t token = 0; char error[512]{}; assert(st_vst3_begin_state_capture(host.get(),stopped,&token,error,sizeof(error))); return token; }
    void authorize(uint64_t token) { assert(st_vst3_authorize_state_capture(host.get(),token,nullptr,0)); }
    std::vector<uint8_t> save() { void *bytes = nullptr; size_t size = 0; assert(st_vst3_copy_state(host.get(),&bytes,&size,nullptr,0)); std::vector<uint8_t> result(static_cast<uint8_t *>(bytes),static_cast<uint8_t *>(bytes)+size); st_vst3_free(bytes); return result; }
    void refused() { void *bytes = reinterpret_cast<void *>(1); size_t size = 99; assert(!st_vst3_copy_state(host.get(),&bytes,&size,nullptr,0) && !bytes && !size); }
};
int main() { @autoreleasepool {
    char error[512]{};
    // Generic plug-ins may restore more than one process block's parameter capacity.
    // Flush the whole admitted batch, with no audio frames and no lost points.
    { Fixture bulk;bulk.host->uid[0]='0';bulk.processor.deferred=false;
      for(uint32_t id=0;id<maxChangedParameters+74;++id)assert(bulk.host->enqueue(id,.4));
      auto saved=bulk.save();assert(!saved.empty()&&bulk.processor.calls==2&&bulk.processor.points==maxChangedParameters+74);
      assert(bulk.processor.lastFrames==0&&!bulk.host->processing&&st_vst3_pending_parameter_edits(bulk.host.get())==0);
    }
    { Fixture bulk;bulk.host->uid[0]='0';bulk.processor.deferred=false;
      for(uint32_t id=0;id<maxChangedParameters+74;++id)assert(bulk.host->enqueue(id,.4));
      bulk.processor.duringProcess=[&]{if(bulk.processor.calls==2)bulk.processor.failProcess=true;};
      bulk.refused();assert(bulk.processor.calls==2&&st_vst3_pending_parameter_edits(bulk.host.get())==74&&!bulk.host->processing);
    }
    { Fixture changing;changing.host->uid[0]='0';assert(changing.host->enqueue(0,.4));
      changing.processor.duringProcess=[&]{assert(changing.host->enqueue(0,.7));};
      changing.refused();assert(changing.processor.calls==1&&st_vst3_pending_parameter_edits(changing.host.get())==1);
    }
    Fixture f; auto h = f.host.get();
    assert(f.status().required_frames == 4096 && f.status().settle_milliseconds == 1000 && !f.status().dirty);
    assert(!strcmp(st_vst3_factory_version(h),"1.7.1 (R0)") && !strcmp(st_vst3_bundle_version(h),"1.7.1"));
    for (const auto& version : {"1.7.2", "", "1.7.1 ", "01.7.1", "1.7.1.0", "1.7.1-beta"}) { h->module->bundleVersion = version; assert(!f.status().required_frames); }
    h->module->bundleVersion = "1.7.1"; h->uid[0] = '0'; assert(!f.status().required_frames); h->uid[0] = '5';
    assert(st_vst3_set_parameter(h,0,.47));
    assert(f.status().edit_generation == 1 && f.status().dirty && f.status().pending_edits == 1);
    f.refused(); assert(!st_vst3_prepare_state_capture(h,1,error,sizeof(error)) && !f.processor.calls);
    uint64_t token = 123;
    assert(!st_vst3_begin_state_capture(h,0,&token,error,sizeof(error)) && !token && !f.processor.calls);
    const auto read = h->pendingRead.load(), write = h->pendingWrite.load();
    h->heldNotes[0][60] = {true,60};
    assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error)) && !token && strstr(error,"held"));
    h->heldNotes[0][60].active = false; h->maxFrames = 512;
    assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error)) && !token); h->maxFrames = 4096;
    f.processor.failStart = true; assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error))); f.processor.failStart = false;
    f.processor.failProcess = true; assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error)) && !h->processing); f.processor.failProcess = false;
    assert(h->pendingRead.load() == read && h->pendingWrite.load() == write && f.status().dirty);
    token = f.begin(); assert(token && f.status().pending_edits == 0 && f.status().dirty && !h->processing);
    assert(f.component.value == .0740625 && f.processor.lastFrames == 4096 && f.processor.calls == 2);
    f.refused(); assert(!st_vst3_authorize_state_capture(h,token,error,sizeof(error)) && strstr(error,"settling"));
    f.progress(); f.authorize(token); assert(f.status().dirty); auto saved = f.save(); assert(!f.status().dirty);
    const auto calls = f.processor.calls; assert(f.begin() == 0 && f.processor.calls == calls && f.save() == saved);
    Fixture restored; assert(st_vst3_restore_state(restored.host.get(),saved.data(),saved.size(),error,sizeof(error)));
    assert(restored.component.value == .47 && restored.controller.value == .47 && restored.render());
    for (float sample : restored.left) assert(sample == float(.47));

    // Ordinary callbacks also leave dirty until a settled, authorized save.
    assert(st_vst3_set_parameter(h,0,.31) && f.render());
    assert(f.status().pending_edits == 0 && f.status().dirty); f.refused();
    const auto normalCalls = f.processor.calls; token = f.begin(false); assert(f.processor.calls == normalCalls);
    f.progress(); f.authorize(token); saved = f.save(); assert(!f.status().dirty && f.component.value == .31);

    // Cancel/retry and overlapping same-generation attempts never alias tokens.
    assert(st_vst3_set_parameter(h,0,.62)); const auto a = f.begin();
    st_vst3_cancel_state_capture(h,a); assert(f.status().dirty); f.refused();
    const auto retryCalls = f.processor.calls, starts = f.processor.starts, stops = f.processor.stops;
    const auto b = f.begin(), c = f.begin(); assert(a != b && b != c && f.processor.calls == retryCalls);
    st_vst3_cancel_state_capture(h,b); f.progress(); assert(!st_vst3_authorize_state_capture(h,a,error,sizeof(error)) && !st_vst3_authorize_state_capture(h,b,error,sizeof(error)));
    f.authorize(c); f.save(); assert(!f.status().dirty && f.processor.starts == starts && f.processor.stops == stops);

    // New edits during preparation or after authorization invalidate capture.
    assert(st_vst3_set_parameter(h,0,.4)); f.processor.duringProcess = [&] { assert(h->enqueue(0,.5)); };
    assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error)) && !token && f.status().pending_edits == 1);
    f.processor.duringProcess = {}; token = f.begin(); f.progress(); f.authorize(token);
    assert(st_vst3_set_parameter(h,0,.6)); f.refused(); assert(f.status().dirty);
    token = f.begin(); f.progress(); f.authorize(token);
    f.component.failSave = true; f.refused(); f.component.failSave = false;
    assert(f.status().dirty); f.refused(); assert(!st_vst3_authorize_state_capture(h,token,error,sizeof(error)));
    token = f.begin(); f.progress(); f.authorize(token); f.save(); assert(!f.status().dirty);

    // Component/controller callbacks can change controls while being serialized.
    for (int where = 0; where < 2; ++where) {
        assert(st_vst3_set_parameter(h,0,.7)); token = f.begin(); f.progress(); f.authorize(token);
        auto callback = [&] { assert(st_vst3_set_parameter(h,0,.8)); };
        if (where == 0) f.component.duringSave = callback; else f.controller.duringSave = callback;
        f.refused(); assert(f.status().dirty && f.status().pending_edits == 1);
        f.component.duringSave = {}; f.controller.duringSave = {};
        token = f.begin(); f.progress(); f.authorize(token); f.save();
    }
    // A clean-entry copy must also reject an edit raised by a vendor callback.
    f.component.duringSave = [&] { assert(st_vst3_set_parameter(h,0,.9)); }; f.refused(); f.component.duringSave = {};
    assert(f.status().dirty); token = f.begin(); f.progress(); f.authorize(token);
    f.component.duringSave = [&] { st_vst3_cancel_state_capture(h,token); };
    f.refused(); f.component.duringSave = {}; assert(f.status().dirty);
    token = f.begin(); f.progress(); f.authorize(token);
    f.component.duringSave = [&] {
        uint64_t nested = 999; void *bytes = reinterpret_cast<void *>(1); size_t size = 4; double raw = .2;
        assert(!st_vst3_begin_state_capture(h,1,&nested,nullptr,0) && !nested);
        assert(!st_vst3_copy_state(h,&bytes,&size,nullptr,0) && !bytes && !size);
        assert(!st_vst3_restore_component_state(h,&raw,sizeof(raw),nullptr,0));
    };
    f.save(); f.component.duringSave = {}; assert(!f.status().dirty);

    // Even a failed partially mutating restore invalidates an earlier token.
    assert(st_vst3_set_parameter(h,0,.2)); token = f.begin(); f.progress();
    f.component.failRestore = true; double raw = .3;
    assert(!st_vst3_restore_component_state(h,&raw,sizeof(raw),error,sizeof(error))); f.component.failRestore = false;
    assert(f.status().dirty && !st_vst3_authorize_state_capture(h,token,error,sizeof(error))); f.refused();
    assert(!st_vst3_begin_state_capture(h,1,&token,error,sizeof(error)) && !token);
    assert(st_vst3_restore_component_state(h,&raw,sizeof(raw),error,sizeof(error)));
    token = f.begin(); f.progress(); f.authorize(token); f.save();
    Fixture cleanFailure; cleanFailure.component.failRestore = true;
    assert(!st_vst3_restore_component_state(cleanFailure.host.get(),&raw,sizeof(raw),error,sizeof(error)));
    cleanFailure.refused(); assert(!st_vst3_begin_state_capture(cleanFailure.host.get(),1,&token,error,sizeof(error)));
    cleanFailure.component.failRestore = false;
    assert(st_vst3_restore_component_state(cleanFailure.host.get(),&raw,sizeof(raw),error,sizeof(error)));
    cleanFailure.save();

    // A future AU-owned note is not delivered or advanced by state preparation.
    assert(st_vst3_set_parameter(h,0,.55)); token = f.begin(); f.progress(); f.authorize(token); f.save();
    STVST3MIDI note{0,0x90,60,100,60}; assert(f.processor.noteOns == 0 && f.render(&note,1));
    assert(f.processor.noteOns == 1 && f.processor.firstNoteOffset == 0 && f.left[0] == float(.55));

    // Fixed process capacity may retire only the represented prefix, never an
    // unrepresented 513th parameter. Successful ordinary retry delivers it.
    Fixture many;
    for (uint32_t id = 0; id < 513; ++id) assert(many.host->enqueue(id,.25));
    assert(many.render() && many.processor.points == 512 && many.status().pending_edits == 1 && many.status().dirty);
    many.refused(); assert(many.render() && many.processor.points == 513 && !many.status().pending_edits);
    token = many.begin(false); many.progress(); many.authorize(token); many.save(); assert(!many.status().dirty);

    // Failed/full/invalid enqueues do not create a generation; counters never wrap.
    Fixture full; assert(!full.host->enqueue(0,NAN) && full.status().edit_generation == 0);
    for (size_t n = 1; n < full.host->pending.size(); ++n) assert(full.host->enqueue(0,.25));
    const auto generation = full.status().edit_generation; assert(!full.host->enqueue(0,.5) && full.status().edit_generation == generation);
    Fixture overflow; overflow.host->editGeneration = UINT64_MAX-1; overflow.host->capturedGeneration = UINT64_MAX-1;
    assert(overflow.host->enqueue(0,.25) && overflow.status().edit_generation == UINT64_MAX);
    assert(!overflow.host->enqueue(0,.5) && overflow.status().pending_edits == 1 && overflow.status().dirty);
    token = overflow.begin(); overflow.progress(); overflow.authorize(token); overflow.save(); assert(!overflow.status().dirty);
    assert(!overflow.host->enqueue(0,.5) && !overflow.status().dirty);
    Fixture exhausted; assert(exhausted.host->enqueue(0,.2)); exhausted.host->captureEpoch = UINT64_MAX;
    assert(!st_vst3_begin_state_capture(exhausted.host.get(),1,&token,error,sizeof(error)) && !token && exhausted.status().dirty);
    exhausted.refused();

    // Generic components retain immediate standard zero-frame capture.
    Fixture generic; generic.host->module->bundleVersion = "1.7.2"; generic.processor.deferred = false;
    assert(st_vst3_set_parameter(generic.host.get(),0,.5)); assert(!generic.status().dirty && generic.begin() == 0);
    generic.save(); assert(generic.processor.calls == 1 && generic.processor.lastFrames == 0 && generic.component.value == .5);
    assert(renderAllocations == 0);
    printf("PASS exact-profile generation/token/settling capture; stopped and ordinary delivery, cancellation, stale/new edits, failed/partial state, nested callbacks, 513-ID retention, overflow, fresh PCM, generic zero-frame save; zero render allocations.\n");
} }
