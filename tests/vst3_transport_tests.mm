// Original in-memory processor only: observe the ProcessContext actually passed
// through both public rendering entry points; no installed plug-ins or devices.
// Contract reference: Vendor/vst3sdk/pluginterfaces/vst/ivstprocesscontext.h.
#include "../Sources/VST3Host/VST3Host.mm"
#include "vst3_state_fixture.h"
#include <cassert>
#include <cstdio>
#include <limits>
#include <new>

static thread_local bool measuringRender = false;
static thread_local size_t renderAllocations = 0;
void *operator new(size_t size) {
    if (measuringRender) ++renderAllocations;
    if (void *pointer = malloc(size ? size : 1)) return pointer;
    throw std::bad_alloc();
}
void *operator new[](size_t size) { return ::operator new(size); }
void operator delete(void *pointer) noexcept { free(pointer); }
void operator delete[](void *pointer) noexcept { free(pointer); }

class TransportProbeProcessor final : public IAudioProcessor {
public:
    ProcessContext context{};
    uint32_t calls = 0;
    int32 frames = 0;
    QUERY_ONE(IAudioProcessor) EMBEDDED_REFS
    tresult PLUGIN_API setBusArrangements(SpeakerArrangement *, int32, SpeakerArrangement *, int32) override { return kResultOk; }
    tresult PLUGIN_API getBusArrangement(BusDirection, int32, SpeakerArrangement& arrangement) override { arrangement = SpeakerArr::kStereo; return kResultOk; }
    tresult PLUGIN_API canProcessSampleSize(int32 size) override { return size == kSample32 ? kResultOk : kResultFalse; }
    uint32 PLUGIN_API getLatencySamples() override { return 0; }
    tresult PLUGIN_API setupProcessing(ProcessSetup&) override { return kResultOk; }
    tresult PLUGIN_API setProcessing(TBool) override { return kResultOk; }
    uint32 PLUGIN_API getTailSamples() override { return 0; }
    tresult PLUGIN_API process(ProcessData& data) override {
        assert(data.processContext);
        context = *data.processContext;
        ++calls;
        frames = data.numSamples;
        for (int32 channel = 0; channel < data.outputs[0].numChannels; ++channel)
            std::fill_n(data.outputs[0].channelBuffers32[channel], data.numSamples, 0.f);
        return kResultOk;
    }
};

static constexpr uint32_t allFields = ST_VST3_TRANSPORT_EXPLICIT
    | ST_VST3_TRANSPORT_MUSIC_VALID | ST_VST3_TRANSPORT_TEMPO_VALID
    | ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID | ST_VST3_TRANSPORT_TRANSPORT_VALID
    | ST_VST3_TRANSPORT_CONTINUOUS_VALID | ST_VST3_TRANSPORT_BAR_VALID
    | ST_VST3_TRANSPORT_CYCLE_VALID;
static constexpr uint32_t musicFlags = ProcessContext::kProjectTimeMusicValid
    | ProcessContext::kTempoValid | ProcessContext::kTimeSigValid;
static constexpr uint32_t rangeFlags = ProcessContext::kContTimeValid
    | ProcessContext::kBarPositionValid | ProcessContext::kCycleValid;
static constexpr uint32_t transportFlags = ProcessContext::kPlaying
    | ProcessContext::kCycleActive | ProcessContext::kRecording;

int main() { @autoreleasepool {
    StateComponent component;
    TransportProbeProcessor probe;
    auto host = std::make_unique<STVST3Instance>();
    host->component = &component;
    host->processor = &probe;
    host->active = host->processing = true;
    host->sampleRate = 48000;
    host->maxFrames = 128;
    std::array<float, 128> left{}, right{}, outputLeft{}, outputRight{};
    float *channels[]{left.data(), right.data()};
    host->outputs.buses.resize(1);
    host->outputs.buses[0].numChannels = 2;
    host->outputs.buses[0].channelBuffers32 = channels;
    auto render = [&](const STVST3Transport *transport, bool indexed = false) {
        const auto priorCalls = probe.calls;
        measuringRender = true;
        const auto result = indexed
            ? st_vst3_process_indexed(host.get(), nullptr, 0, outputLeft.data(), outputRight.data(), 128, nullptr, 0, nullptr, 0, transport)
            : st_vst3_process(host.get(), nullptr, nullptr, outputLeft.data(), outputRight.data(), 128, nullptr, 0, nullptr, 0, transport);
        measuringRender = false;
        assert(result && probe.calls == priorCalls + 1 && probe.frames == 128);
        assert(probe.context.sampleRate == 48000);
        assert(renderAllocations == 0);
        return probe.context;
    };
    int groups = 0;

    // Source-compatible six-field aggregate: the appended capability mask is
    // zero, and no continuous/bar/cycle position may be invented from project time.
    STVST3Transport legacy{1234, 2.5, 133, 7, 8, 1};
    assert(legacy.valid_fields == 0);
    auto context = render(&legacy);
    assert(context.state == (musicFlags | ProcessContext::kPlaying));
    assert(context.projectTimeSamples == 1234 && context.projectTimeMusic == 2.5);
    assert(context.tempo == 133 && context.timeSigNumerator == 7 && context.timeSigDenominator == 8);
    legacy.playing = 0;
    assert(render(&legacy, true).state == musicFlags);
    ++groups;

    STVST3Transport complete{240000, 10, 145.25, 5, 8, 1};
    complete.valid_fields = allFields;
    complete.continuous_sample_position = 960000;
    complete.bar_start_beat = 7.5;
    complete.cycle_start_beat = 5;
    complete.cycle_end_beat = 12.5;
    complete.cycling = complete.recording = 1;
    context = render(&complete);
    assert(context.state == (musicFlags | rangeFlags | transportFlags));
    assert(context.projectTimeSamples == 240000 && context.continousTimeSamples == 960000);
    assert(context.projectTimeMusic == 10 && context.barPositionMusic == 7.5);
    assert(context.cycleStartMusic == 5 && context.cycleEndMusic == 12.5);
    assert(context.tempo == 145.25 && context.timeSigNumerator == 5 && context.timeSigDenominator == 8);
    ++groups;

    // A missing context after a fully populated one must clear every optional
    // flag and transport state. Defaults are benign values, not asserted facts.
    context = render(nullptr, true);
    assert(context.state == 0 && context.projectTimeSamples == 0);
    assert(context.tempo == 120 && context.timeSigNumerator == 4 && context.timeSigDenominator == 4);
    assert(context.continousTimeSamples == 0 && context.projectTimeMusic == 0);
    assert(context.barPositionMusic == 0 && context.cycleStartMusic == 0 && context.cycleEndMusic == 0);
    ++groups;

    // Optional fields are independent. Populated but unadvertised values must
    // not acquire validity merely because they are finite or nonzero.
    struct FieldCase { uint32_t input, expected; };
    constexpr FieldCase fields[]{
        {ST_VST3_TRANSPORT_MUSIC_VALID, ProcessContext::kProjectTimeMusicValid},
        {ST_VST3_TRANSPORT_TEMPO_VALID, ProcessContext::kTempoValid},
        {ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID, ProcessContext::kTimeSigValid},
        {ST_VST3_TRANSPORT_CONTINUOUS_VALID, ProcessContext::kContTimeValid},
        {ST_VST3_TRANSPORT_BAR_VALID, ProcessContext::kBarPositionValid},
        {ST_VST3_TRANSPORT_CYCLE_VALID, ProcessContext::kCycleValid}
    };
    for (const auto& field : fields) {
        auto partial = complete;
        partial.valid_fields = ST_VST3_TRANSPORT_EXPLICIT | field.input;
        context = render(&partial);
        assert(context.state == field.expected && context.projectTimeSamples == complete.sample_position);
    }
    auto none = complete;
    none.valid_fields = ST_VST3_TRANSPORT_EXPLICIT | (1u << 27); // Unknown capabilities do not leak into SDK flags.
    assert(render(&none).state == 0);
    ++groups;

    // Playing/recording belong to the transport capability, not the availability
    // of tempo or musical time. Stopped does not imply zero sample position.
    auto stateOnly = complete;
    stateOnly.valid_fields = ST_VST3_TRANSPORT_EXPLICIT | ST_VST3_TRANSPORT_TRANSPORT_VALID;
    stateOnly.cycling = 0;
    assert(render(&stateOnly).state == (ProcessContext::kPlaying | ProcessContext::kRecording));
    stateOnly.cycling = 1;
    assert(render(&stateOnly).state == (ProcessContext::kPlaying | ProcessContext::kRecording));
    stateOnly.playing = stateOnly.recording = 0;
    context = render(&stateOnly);
    assert(context.state == 0 && context.projectTimeSamples == complete.sample_position);
    auto noState = complete;
    noState.valid_fields &= ~ST_VST3_TRANSPORT_TRANSPORT_VALID;
    assert(render(&noState).state == (musicFlags | rangeFlags));
    ++groups;

    // Callers supply positions at the start of each block. A seek, loop wrap or
    // PDC preroll changes project time without implicitly changing continuous
    // time. This verifies the C boundary, not the higher-level host scheduler.
    constexpr int64_t exactBeyondDouble = INT64_C(9007199254740993);
    struct PositionCase { int64_t project, continuous; double beat; };
    const PositionCase positions[]{
        {0, 0, 0}, {48000, 48000, 2}, {96000, 48128, 4},
        {24000, 48256, 1}, {-512, 48384, -0.02},
        {exactBeyondDouble, exactBeyondDouble + 17, 5.25},
        {std::numeric_limits<int64_t>::max(), std::numeric_limits<int64_t>::min(), 6.5},
        {std::numeric_limits<int64_t>::min(), std::numeric_limits<int64_t>::max(), -6.5}
    };
    for (size_t index = 0; index < std::size(positions); ++index) {
        const auto& position = positions[index];
        auto supplied = complete;
        supplied.valid_fields = ST_VST3_TRANSPORT_EXPLICIT | ST_VST3_TRANSPORT_MUSIC_VALID | ST_VST3_TRANSPORT_CONTINUOUS_VALID;
        supplied.sample_position = position.project;
        supplied.continuous_sample_position = position.continuous;
        supplied.beat_position = position.beat;
        context = render(&supplied, index % 2);
        assert(context.state == (ProcessContext::kProjectTimeMusicValid | ProcessContext::kContTimeValid));
        assert(context.projectTimeSamples == position.project);
        assert(context.continousTimeSamples == position.continuous);
        assert(context.projectTimeMusic == position.beat);
    }
    ++groups;

    const double nan = std::numeric_limits<double>::quiet_NaN();
    const double infinity = std::numeric_limits<double>::infinity();
    const double invalidBeats[]{nan, infinity, -infinity, 1e12, -1e12};
    for (const double invalid : invalidBeats) {
        auto supplied = complete;
        supplied.beat_position = invalid;
        context = render(&supplied);
        assert(!(context.state & ProcessContext::kProjectTimeMusicValid));
        assert((context.state & (ProcessContext::kTempoValid | ProcessContext::kTimeSigValid)) == (ProcessContext::kTempoValid | ProcessContext::kTimeSigValid));
        supplied = legacy;
        supplied.beat_position = invalid;
        assert(render(&supplied).state == (ProcessContext::kTempoValid | ProcessContext::kTimeSigValid));
    }
    for (const double valid : {-999999999999.0, -0.25, 0.0, 999999999999.0}) {
        auto supplied = none;
        supplied.valid_fields |= ST_VST3_TRANSPORT_MUSIC_VALID;
        supplied.beat_position = valid;
        context = render(&supplied);
        assert(context.state == ProcessContext::kProjectTimeMusicValid && context.projectTimeMusic == valid);
    }
    ++groups;

    for (const double invalid : {nan, infinity, -infinity, -1.0, 0.0, 10000.5}) {
        auto supplied = complete;
        supplied.tempo = invalid;
        context = render(&supplied);
        assert(!(context.state & ProcessContext::kTempoValid));
        assert(context.state & ProcessContext::kProjectTimeMusicValid);
        supplied = legacy;
        supplied.tempo = invalid;
        assert(render(&supplied).state == (ProcessContext::kProjectTimeMusicValid | ProcessContext::kTimeSigValid));
    }
    for (const double valid : {0.001, 120.0, 10000.0}) {
        auto supplied = none;
        supplied.valid_fields |= ST_VST3_TRANSPORT_TEMPO_VALID;
        supplied.tempo = valid;
        context = render(&supplied);
        assert(context.state == ProcessContext::kTempoValid && context.tempo == valid);
    }
    ++groups;

    for (const auto invalid : {std::pair<int32_t, int32_t>{0, 4}, {-1, 4}, {4, 0}, {4, -1}}) {
        auto supplied = complete;
        supplied.numerator = invalid.first;
        supplied.denominator = invalid.second;
        assert(!(render(&supplied).state & ProcessContext::kTimeSigValid));
        supplied.valid_fields = 0;
        assert(render(&supplied).state == (ProcessContext::kProjectTimeMusicValid | ProcessContext::kTempoValid | ProcessContext::kPlaying));
    }
    auto meter = none;
    meter.valid_fields |= ST_VST3_TRANSPORT_TIME_SIGNATURE_VALID;
    meter.numerator = 3;
    meter.denominator = 7; // The C contract requires positive values, not powers of two.
    context = render(&meter);
    assert(context.state == ProcessContext::kTimeSigValid && context.timeSigNumerator == 3 && context.timeSigDenominator == 7);
    ++groups;

    for (const double invalid : {nan, infinity, -infinity, 1e12, -1e12, 10.001}) {
        auto supplied = complete;
        supplied.bar_start_beat = invalid;
        context = render(&supplied);
        assert(!(context.state & ProcessContext::kBarPositionValid));
        assert(context.state & ProcessContext::kProjectTimeMusicValid);
    }
    for (const double valid : {-2.5, 0.0, 10.0}) {
        auto supplied = complete;
        supplied.bar_start_beat = valid;
        context = render(&supplied);
        assert(context.state & ProcessContext::kBarPositionValid);
        assert(context.barPositionMusic == valid);
    }
    auto barOnly = none;
    barOnly.valid_fields |= ST_VST3_TRANSPORT_BAR_VALID;
    barOnly.beat_position = -200; // Unadvertised musical time cannot invalidate a known bar start.
    barOnly.bar_start_beat = 7.5;
    assert(render(&barOnly).state == ProcessContext::kBarPositionValid);
    ++groups;

    struct CycleCase { double start, end; };
    const CycleCase invalidCycles[]{
        {nan, 4}, {0, nan}, {infinity, 4}, {0, infinity},
        {-infinity, 4}, {0, -infinity}, {4, 4}, {5, 4},
        {-1e12, 0}, {0, 1e12}
    };
    for (const auto& invalid : invalidCycles) {
        auto supplied = complete;
        supplied.cycle_start_beat = invalid.start;
        supplied.cycle_end_beat = invalid.end;
        context = render(&supplied);
        assert(!(context.state & (ProcessContext::kCycleValid | ProcessContext::kCycleActive)));
        assert(context.state & ProcessContext::kProjectTimeMusicValid);
    }
    auto negativeCycle = complete;
    negativeCycle.cycle_start_beat = -8;
    negativeCycle.cycle_end_beat = -4;
    context = render(&negativeCycle, true);
    assert((context.state & (ProcessContext::kCycleActive | ProcessContext::kCycleValid)) == (ProcessContext::kCycleActive | ProcessContext::kCycleValid));
    assert(context.cycleStartMusic == -8 && context.cycleEndMusic == -4);
    negativeCycle.cycling = 0;
    context = render(&negativeCycle);
    assert((context.state & (ProcessContext::kCycleActive | ProcessContext::kCycleValid)) == ProcessContext::kCycleValid);
    ++groups;

    auto malformed = complete;
    malformed.valid_fields &= ~(ST_VST3_TRANSPORT_CONTINUOUS_VALID | ST_VST3_TRANSPORT_TRANSPORT_VALID);
    malformed.beat_position = malformed.tempo = malformed.bar_start_beat = malformed.cycle_start_beat = malformed.cycle_end_beat = nan;
    malformed.numerator = malformed.denominator = -1;
    context = render(&malformed);
    assert(context.state == 0 && context.projectTimeSamples == malformed.sample_position);
    // Repeated validity changes cannot preserve stale bits or allocate on render.
    for (int index = 0; index < 128; ++index) {
        assert(render(&complete, index % 2).state == (musicFlags | rangeFlags | transportFlags));
        assert(render(&malformed, index % 2).state == 0);
        assert(render(nullptr, index % 2).state == 0);
    }
    assert(renderAllocations == 0);
    ++groups;
    printf("PASS VST3 transport: %d groups, %u actual processor calls, exact int64 sample coordinates, independent optional validity, legacy compatibility, zero C++ render allocations.\n", groups, probe.calls);
} }
