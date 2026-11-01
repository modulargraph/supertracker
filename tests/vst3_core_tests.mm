// Compile without loading third-party modules. Exercises original host boundaries,
// event conversion, panic handling and render allocation behavior deterministically.
#include "../Sources/VST3Host/VST3Host.mm"
#include "vst3_state_fixture.h"
#include <cassert>
#include <cstdio>
#include <new>

static thread_local bool measuringRender = false;
static thread_local size_t renderAllocations = 0;
void *operator new(size_t size) { if (measuringRender) ++renderAllocations; if (void *p = malloc(size ? size : 1)) return p; throw std::bad_alloc(); }
void *operator new[](size_t size) { return ::operator new(size); }
void operator delete(void *pointer) noexcept { free(pointer); }
void operator delete[](void *pointer) noexcept { free(pointer); }

class ProbeProcessor final : public IAudioProcessor {
public:
    int noteOn = 0, noteOff = 0; int firstOn = -1, firstOff = -1;
    std::array<Event, 4096> lastEvents{}; int eventCount = 0;
    std::array<STVST3ParameterChange, 4096> parameters{}; int parameterCount = 0;
    int processingCalls = 0, failProcessingCall = -1; bool failProcess = false;
    QUERY_ONE(IAudioProcessor) EMBEDDED_REFS
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32, SpeakerArrangement *, int32) override { return kResultOk; }
    tresult PLUGIN_API getBusArrangement(BusDirection, int32, SpeakerArrangement& arrangement) override { arrangement = SpeakerArr::kStereo; return kResultOk; }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool) override { return ++processingCalls == failProcessingCall ? kResultFalse : kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        eventCount = data.inputEvents->getEventCount(); parameterCount = 0;
        for (int32 n = 0; n < eventCount; ++n) { data.inputEvents->getEvent(n, lastEvents[n]); const auto& e = lastEvents[n]; if (e.type == Event::kNoteOnEvent) { ++noteOn; firstOn = e.sampleOffset; } if (e.type == Event::kNoteOffEvent) { ++noteOff; firstOff = e.sampleOffset; } }
        for (int32 n = 0; n < data.inputParameterChanges->getParameterCount(); ++n) { auto queue = data.inputParameterChanges->getParameterData(n); for (int32 p = 0; p < queue->getPointCount(); ++p) { int32 offset; double value; queue->getPoint(p, offset, value); parameters[parameterCount++] = {offset, queue->getParameterId(), value}; } }
        if (failProcess) return kResultFalse;
        for (int32 n = 0; n < data.numSamples; ++n) { data.outputs[0].channelBuffers32[0][n] = .25f; data.outputs[0].channelBuffers32[1][n] = -.25f; }
        int32 index; auto output = data.outputParameterChanges->addParameterData(42, index); output->addPoint(data.numSamples ? data.numSamples - 1 : 0, .75, index);
        return kResultOk;
    }
};
int main() { @autoreleasepool {
    MemoryStream stream; char data[] = "abcdef"; int32 bytes = 0;
    assert(stream.write(data, 6, &bytes) == kResultOk && bytes == 6);
    assert(stream.seek(-2, IBStream::kIBSeekEnd, nullptr) == kResultOk);
    char result[16]{}; assert(stream.read(result, 10, &bytes) == kResultOk && bytes == 2 && !strcmp(result, "ef"));
    assert(stream.seek(-100, IBStream::kIBSeekCur, nullptr) == kInvalidArgument);
    assert(stream.seek(stateLimit, IBStream::kIBSeekSet, nullptr) == kResultOk);
    assert(stream.write(data, 1, nullptr) == kInvalidArgument);
    MemoryStream readOnly(data, 6); assert(readOnly.write(data, 1, nullptr) == kInvalidArgument);
    ParameterQueue queue; int32 index; queue.id = 7;
    assert(queue.addPoint(15, .1, index) == kResultOk); assert(queue.addPoint(3, .2, index) == kResultOk); assert(queue.addPoint(15, .9, index) == kResultOk);
    assert(queue.count == 2 && queue.points[0].offset == 3 && queue.points[1].value == .9);
    for (int n = 0; n < 254; ++n) assert(queue.addPoint(100 + n, .5, index) == kResultOk);
    assert(queue.addPoint(999, .5, index) == kResultFalse);
    auto changes = std::make_unique<ParameterChanges>(); for (uint32 n = 0; n < maxChangedParameters; ++n) assert(changes->add(n, 0, .5)); assert(!changes->add(maxChangedParameters, 0, .5));
    auto list = std::make_unique<EventList>(); Event event{}; for (uint32 n = 0; n < maxEvents; ++n) assert(list->addEvent(event) == kResultOk); assert(list->addEvent(event) == kResultFalse);
    StateComponent component; ProbeProcessor probe; auto host = std::make_unique<STVST3Instance>(); host->component = &component; host->processor = &probe; host->active = true; host->processing = true; host->maxFrames = 512;
    std::array<float, 512> left{}, right{}, outLeft{}, outRight{}; float *channels[] = {left.data(), right.data()};
    host->outputs.buses.resize(1); host->outputs.buses[0].numChannels = 2; host->outputs.buses[0].channelBuffers32 = channels;
    host->parameterIDs = {42}; host->parameterStates = std::make_unique<STVST3Instance::ParameterState[]>(1);
    host->midiParameter[0][64] = 88; host->midiParameter[0][129] = 89;
    STVST3MIDI notes[] = {{37, 0x90, 60, 100, 900}, {251, 0x80, 60, 0, 900}};
    STVST3ParameterChange automation[] = {{78, 42, .25}, {152, 42, .5}};
    STVST3Transport transport{1234, 2.5, 133, 7, 8, 1};
    measuringRender = true;
    assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, notes, 2, automation, 2, &transport));
    measuringRender = false;
    assert(renderAllocations == 0 && probe.noteOn == 1 && probe.noteOff == 1 && probe.firstOn == 37 && probe.firstOff == 251);
    assert(probe.parameterCount == 2 && probe.parameters[0].sample_offset == 78 && probe.parameters[1].value == .5);
    assert(outLeft[511] == .25f && outRight[511] == -.25f && st_vst3_parameter_value(host.get(), 42) == .75);
    STVST3MIDI panic[] = {{0, 0x90, 61, 100, 901}, {64, 0xb0, 123, 0, -1}};
    measuringRender = true; assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, panic, 2, nullptr, 0, nullptr)); measuringRender = false;
    assert(probe.eventCount == 2 && probe.lastEvents[1].type == Event::kNoteOffEvent && probe.lastEvents[1].noteOff.noteId == 901 && probe.lastEvents[1].sampleOffset == 64);
    assert(probe.parameterCount == 1 && probe.parameters[0].id == 88 && probe.parameters[0].value == 0);
    STVST3MIDI bend[] = {{19, 0xe0, 0, 64, -1}};
    assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, bend, 1, nullptr, 0, nullptr)); assert(probe.parameterCount == 1 && probe.parameters[0].id == 89 && std::abs(probe.parameters[0].value - 8192. / 16383.) < .000001);
    assert(!st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 513, nullptr, 0, nullptr, 0, nullptr));
    STVST3MIDI held{0, 0x90, 72, 100, 1234};
    assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, &held, 1, nullptr, 0, nullptr));
    assert(st_vst3_reset(host.get(), nullptr, 0) && probe.eventCount == 1 && probe.lastEvents[0].type == Event::kNoteOffEvent && probe.lastEvents[0].noteOff.noteId == 1234 && !host->heldNotes[0][72].active);
    // Passive trace distinguishes UI input from same-ID automation/MIDI that
    // legitimately coalesces it at one offset, and counts actual plugin reads.
    STVST3ParameterDiagnostics trace{};
    assert(!st_vst3_parameter_diagnostics(host.get(), &trace));
    assert(st_vst3_watch_parameter(host.get(), 42));
    assert(host->enqueue(42, .6));
    STVST3ParameterChange sameOffset{0, 42, .9};
    measuringRender = true;
    assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, nullptr, 0, &sameOffset, 1, nullptr));
    measuringRender = false;
    assert(st_vst3_parameter_diagnostics(host.get(), &trace));
    assert(trace.ui_queued == 1 && trace.ui_dequeued == 1 && trace.automation_received == 1 && trace.midi_received == 0);
    assert(trace.last_ui_value == .6 && trace.last_dequeued_value == .6 && trace.last_automation_value == .9);
    assert(trace.offered_points == 1 && trace.read_points == 1 && trace.last_offered_value == .9 && trace.last_read_value == .9);
    assert(trace.offered_process == 1 && trace.read_process == 1 && trace.last_read_frames == 512 && trace.pending_ui_edits == 0);
    host->midiParameter[0][1] = 42;
    STVST3MIDI mappedCC{0, 0xb0, 1, 0, -1};
    assert(host->enqueue(42, .6));
    measuringRender = true;
    assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, &mappedCC, 1, &sameOffset, 1, nullptr));
    measuringRender = false;
    assert(st_vst3_parameter_diagnostics(host.get(), &trace));
    assert(trace.midi_received == 1 && trace.last_midi_value == 0 && trace.last_offered_value == 0 && trace.last_read_value == 0);
    assert(trace.ui_queued == 2 && trace.automation_received == 2 && trace.offered_points == 2 && trace.read_points == 2 && trace.processes == 2);
    // Failure at every reset stage must leave pending UI controls in their
    // original SPSC positions for the next successful ordinary render.
    for (int failure = 0; failure < 4; ++failure) {
        assert(st_vst3_set_processing(host.get(), failure == 0 ? 0 : 1, nullptr, 0));
        assert(st_vst3_watch_parameter(host.get(), 42));
        assert(host->enqueue(42, .625));
        const uint32_t pendingRead = host->pendingRead.load(), pendingWrite = host->pendingWrite.load();
        probe.failProcess = failure == 1;
        probe.failProcessingCall = failure == 1 ? -1 : probe.processingCalls + (failure == 3 ? 2 : 1);
        assert(!st_vst3_reset(host.get(), nullptr, 0));
        assert(st_vst3_parameter_diagnostics(host.get(), &trace));
        assert(trace.ui_queued == 1 && trace.ui_dequeued == 0 && trace.pending_ui_edits == 1 && trace.offered_points == 0 && trace.read_points == 0);
        assert(host->pendingRead.load() == pendingRead && host->pendingWrite.load() == pendingWrite);
        probe.failProcess = false; probe.failProcessingCall = -1;
        assert(st_vst3_set_processing(host.get(), 1, nullptr, 0));
        measuringRender = true;
        assert(st_vst3_process(host.get(), nullptr, nullptr, outLeft.data(), outRight.data(), 512, nullptr, 0, nullptr, 0, nullptr));
        measuringRender = false;
        assert(st_vst3_parameter_diagnostics(host.get(), &trace));
        assert(trace.ui_dequeued == 1 && trace.pending_ui_edits == 0 && trace.last_read_value == .625 && trace.last_read_frames == 512);
    }
    assert(!st_vst3_watch_parameter(host.get(), 0xfffffff0) && !st_vst3_parameter_diagnostics(host.get(), &trace));
    assert(st_vst3_dropped_events(host.get()) == 0 && renderAllocations == 0);
    printf("PASS VST3 host stream bounds, bounded queues, exact MIDI/automation offsets, output cache, pitch bend, panic note-offs, stereo processing, zero render allocations.\n");
} }
