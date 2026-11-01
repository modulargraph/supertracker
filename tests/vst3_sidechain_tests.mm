#include "VST3Host.h"
#include <algorithm>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>
#include <stdexcept>
#include <string>
#include <vector>

static thread_local bool measuringRender = false;
static thread_local size_t renderAllocations = 0;
void *operator new(size_t size) { if (measuringRender) ++renderAllocations; if (void *pointer = std::malloc(size ? size : 1)) return pointer; throw std::bad_alloc(); }
void *operator new[](size_t size) { return ::operator new(size); }
void operator delete(void *pointer) noexcept { std::free(pointer); }
void operator delete[](void *pointer) noexcept { std::free(pointer); }

namespace {
char error[1024]{};
int assertions = 0;
void check(bool result, const char *expression, int line) {
    ++assertions;
    if (!result) throw std::runtime_error(std::string("line ") + std::to_string(line) + ": " + expression + " — " + error);
}
#define CHECK(expression) check((expression), #expression, __LINE__)
struct Scope {
    std::vector<STVST3Instance *> instances;
    ~Scope() { for (auto instance : instances) st_vst3_destroy(instance); st_vst3_release_unused_modules(); }
};
struct SavedState {
    void *bytes = nullptr; size_t size = 0;
    ~SavedState() { st_vst3_free(bytes); }
};
std::vector<STVST3Class> classes;
void found(const STVST3Class *item, void *) { classes.push_back(*item); }
STVST3InputBus bus(STVST3Instance *instance, int index) {
    STVST3InputBus info{};
    CHECK(st_vst3_input_bus(instance, index, &info));
    return info;
}
void stop(STVST3Instance *instance) { CHECK(st_vst3_set_processing(instance, 0, error, sizeof(error))); CHECK(!st_vst3_is_processing(instance)); }
void start(STVST3Instance *instance) { CHECK(st_vst3_set_processing(instance, 1, error, sizeof(error))); CHECK(st_vst3_is_processing(instance)); }
void configure(STVST3Instance *instance, int index, int channels, bool active) {
    stop(instance);
    CHECK(st_vst3_configure_input_bus(instance, index, channels, active, error, sizeof(error)));
    CHECK(!st_vst3_is_processing(instance));
    CHECK(bool(bus(instance, index).is_active) == active);
    start(instance);
}
constexpr uint32_t capacity = 128;
struct Audio {
    std::array<float, capacity> mainL{}, mainR{}, keyL{}, keyR{}, spareL{}, spareR{}, outL{}, outR{};
    const float *program[2] = {mainL.data(), mainR.data()};
    const float *key[2] = {keyL.data(), keyR.data()};
    const float *spare[2] = {spareL.data(), spareR.data()};
    Audio() {
        // Different L/R and main/key signals catch wrong index, fanout, offset,
        // and stale-block errors without tolerance hiding a one-frame defect.
        for (uint32_t n = 0; n < capacity; ++n) {
            mainL[n] = n % 7 == 0 ? .75f : 0.f;
            mainR[n] = n % 11 == 0 ? .5f : 0.f;
            keyL[n] = n % 13 == 0 ? .25f : 0.f;
            keyR[n] = n % 5 == 0 ? .125f : 0.f;
            spareL[n] = 8; spareR[n] = 16;
        }
    }
    void sentinel() { outL.fill(99); outR.fill(99); }
    void expected(uint32_t frames, bool main, bool keyOn, bool mono) {
        for (uint32_t n = 0; n < frames; ++n) {
            const float left = (main ? mainL[n] : 0.f) - (keyOn ? keyL[n] : 0.f);
            const float right = (main ? mainR[n] : 0.f) - (keyOn ? (mono ? keyL[n] : keyR[n]) : 0.f);
            if (outL[n] != left || outR[n] != right) {
                std::snprintf(error, sizeof(error), "frame %u: expected %.3f/%.3f, received %.3f/%.3f", n, left, right, outL[n], outR[n]);
                CHECK(false);
            }
        }
        ++assertions;
    }
    void silent(uint32_t frames) {
        for (uint32_t n = 0; n < frames; ++n) CHECK(outL[n] == 0 && outR[n] == 0);
    }
};
int render(STVST3Instance *instance, Audio& audio, const STVST3InputBuffer *inputs, uint32_t count, uint32_t frames = 64) {
    audio.sentinel();
    measuringRender = true;
    const int result = st_vst3_process_indexed(instance, inputs, count, audio.outL.data(), audio.outR.data(), frames, nullptr, 0, nullptr, 0, nullptr);
    measuringRender = false;
    CHECK(renderAllocations == 0);
    return result;
}
void rejected(STVST3Instance *instance, Audio& audio, const STVST3InputBuffer *inputs, uint32_t count) {
    CHECK(!render(instance, audio, inputs, count));
    audio.silent(64);
}
void restoreUnchanged(STVST3Instance *instance, Audio& audio, const STVST3InputBuffer *inputs, uint32_t inputCount, int keyChannels) {
    std::vector<STVST3InputBus> before;
    for (int n = 0; n < st_vst3_input_bus_count(instance); ++n) before.push_back(bus(instance, n));
    auto unchanged = [&] {
        CHECK(st_vst3_input_bus_count(instance) == int(before.size()));
        for (const auto& previous : before) {
            const auto current = bus(instance, previous.index);
            CHECK(current.is_active == previous.is_active && current.channels == previous.channels && current.bus_type == previous.bus_type);
            CHECK(!std::strcmp(current.name, previous.name));
        }
    };
    SavedState state;
    CHECK(st_vst3_copy_state(instance, &state.bytes, &state.size, error, sizeof(error)));
    CHECK(state.bytes && state.size > 56);
    // Both opaque component presets and full host state leave the user's active
    // input selections alone when the component's layout has not changed.
    struct RawState { uint32_t magic; int32_t keyChannels; } raw{0x53434B59, keyChannels};
    CHECK(st_vst3_restore_component_state(instance, &raw, sizeof(raw), error, sizeof(error)));
    CHECK(st_vst3_is_processing(instance));
    unchanged();
    CHECK(render(instance, audio, inputs, inputCount)); audio.expected(64, true, true, keyChannels == 1);
    CHECK(st_vst3_restore_state(instance, state.bytes, state.size, error, sizeof(error)));
    CHECK(st_vst3_is_processing(instance));
    unchanged();
    CHECK(render(instance, audio, inputs, inputCount)); audio.expected(64, true, true, keyChannels == 1);
    stop(instance);
    CHECK(st_vst3_restore_state(instance, state.bytes, state.size, error, sizeof(error)));
    CHECK(!st_vst3_is_processing(instance));
    unchanged();
    CHECK(!render(instance, audio, inputs, inputCount)); audio.silent(64);
    start(instance);
    CHECK(render(instance, audio, inputs, inputCount)); audio.expected(64, true, true, keyChannels == 1);
}
void metadata(STVST3Instance *instance, int mainIndex, int keyIndex, int keyChannels, int count) {
    CHECK(!st_vst3_is_instrument(instance));
    CHECK(st_vst3_input_bus_count(instance) == count);
    CHECK(st_vst3_main_input_bus(instance) == mainIndex);
    for (int n = 0; n < count; ++n) {
        auto info = bus(instance, n);
        const bool main = n == mainIndex;
        CHECK(info.index == n);
        CHECK(info.channels == (n == keyIndex ? keyChannels : 2));
        CHECK(info.bus_type == (main ? 0 : 1)); // VST3 kMain / kAux.
        CHECK(bool(info.is_auxiliary) == !main);
        CHECK(bool(info.is_active) == main);
        CHECK(!std::strcmp(info.name, main ? "Program Input" : n == keyIndex ? "Detector Key" : "Unused Auxiliary"));
    }
    STVST3InputBus invalid{};
    CHECK(!st_vst3_input_bus(instance, -1, &invalid));
    CHECK(!st_vst3_input_bus(instance, count, &invalid));
}
void routing(STVST3Instance *instance, int mainIndex, int keyIndex, int keyChannels, int count) {
    Audio audio;
    const bool mono = keyChannels == 1;
    STVST3InputBuffer program{mainIndex, 2, audio.program};
    STVST3InputBuffer key{keyIndex, keyChannels, audio.key};
    // Descriptors deliberately arrive in reverse bus order.
    STVST3InputBuffer inputs[] = {key, program};
    CHECK(render(instance, audio, inputs, 2));
    audio.expected(64, true, false, mono); // Auxiliary default-active flag is ignored.
    CHECK(!st_vst3_configure_input_bus(instance, keyIndex, keyChannels, 1, error, sizeof(error)));
    CHECK(st_vst3_is_processing(instance));
    CHECK(!bus(instance, keyIndex).is_active); // Configuration requires stopped DSP.
    configure(instance, keyIndex, keyChannels, true);
    for (uint32_t frames : {1u, 7u, 64u}) {
        CHECK(render(instance, audio, inputs, 2, frames));
        audio.expected(frames, true, true, mono);
    }
    CHECK(render(instance, audio, &program, 1));
    audio.expected(64, true, false, mono); // Omission clears the preceding key buffer.
    CHECK(render(instance, audio, &key, 1));
    audio.expected(64, false, true, mono); // Omission clears the preceding main buffer.
    CHECK(render(instance, audio, nullptr, 0));
    audio.silent(64);

    // A present bus with a null channel array is silence, and an individually
    // null stereo channel is silence only for that channel. Neither can reuse
    // samples supplied by the previous block or silently fan out its neighbor.
    STVST3InputBuffer nullKey[] = {{keyIndex, keyChannels, nullptr}, program};
    CHECK(render(instance, audio, inputs, 2));
    CHECK(render(instance, audio, nullKey, 2)); audio.expected(64, true, false, mono);
    STVST3InputBuffer nullProgram[] = {key, {mainIndex, 2, nullptr}};
    CHECK(render(instance, audio, nullProgram, 2)); audio.expected(64, false, true, mono);
    const float *emptyChannels[2] = {nullptr, nullptr};
    STVST3InputBuffer empty[] = {{keyIndex, keyChannels, emptyChannels}, {mainIndex, 2, emptyChannels}};
    CHECK(render(instance, audio, empty, 2)); audio.silent(64);
    const float *leftOnly[2] = {audio.mainL.data(), nullptr};
    STVST3InputBuffer oneProgram{mainIndex, 2, leftOnly};
    CHECK(render(instance, audio, &oneProgram, 1));
    for (uint32_t n = 0; n < 64; ++n) CHECK(audio.outL[n] == audio.mainL[n] && audio.outR[n] == 0);
    const float *keyRightOnly[2] = {nullptr, audio.keyR.data()};
    STVST3InputBuffer oneKey[] = {{keyIndex, keyChannels, keyRightOnly}, program};
    CHECK(render(instance, audio, oneKey, 2));
    for (uint32_t n = 0; n < 64; ++n) {
        CHECK(audio.outL[n] == audio.mainL[n]);
        CHECK(audio.outR[n] == audio.mainR[n] - (mono ? 0.f : audio.keyR[n]));
    }

    // Exact cancellation at both block edges proves no implicit delay or shift.
    audio.mainL.fill(0); audio.mainR.fill(0); audio.keyL.fill(0); audio.keyR.fill(0);
    audio.mainL[0] = audio.mainR[0] = audio.keyL[0] = audio.keyR[0] = 1;
    audio.mainL[63] = audio.mainR[63] = audio.keyL[63] = audio.keyR[63] = .5f;
    CHECK(render(instance, audio, inputs, 2));
    audio.silent(64);
    audio.keyL[63] = audio.keyR[63] = 0;
    audio.keyL[62] = audio.keyR[62] = .5f;
    CHECK(render(instance, audio, inputs, 2));
    audio.expected(64, true, true, mono);
    CHECK(audio.outL[62] == -.5f && audio.outL[63] == .5f);

    // Prepare must retain the selected auxiliary through both fast and rebuild paths.
    CHECK(st_vst3_prepare(instance, 48000, 64, error, sizeof(error)));
    CHECK(bus(instance, keyIndex).is_active);
    CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, true, mono);
    CHECK(st_vst3_prepare(instance, 44100, capacity, error, sizeof(error)));
    CHECK(bus(instance, keyIndex).is_active);
    CHECK(render(instance, audio, inputs, 2, capacity)); audio.expected(capacity, true, true, mono);

    if (count == 3) {
        configure(instance, 1, 2, true);
        STVST3InputBuffer all[] = {key, {1, 2, audio.spare}, program};
        CHECK(render(instance, audio, all, 3));
        audio.expected(64, true, true, mono); // The spare auxiliary is not the detector.
        restoreUnchanged(instance, audio, all, 3, keyChannels);
        configure(instance, 1, 2, false);
    } else restoreUnchanged(instance, audio, inputs, 2, keyChannels);
    configure(instance, keyIndex, keyChannels, false);
    CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, false, mono);
    configure(instance, keyIndex, keyChannels, true);

    stop(instance);
    CHECK(!st_vst3_configure_input_bus(instance, keyIndex, keyChannels == 1 ? 2 : 1, 1, error, sizeof(error)));
    CHECK(!st_vst3_configure_input_bus(instance, -1, 2, 1, error, sizeof(error)));
    CHECK(!st_vst3_configure_input_bus(instance, count, 2, 1, error, sizeof(error)));
    CHECK(bus(instance, keyIndex).is_active && bus(instance, keyIndex).channels == keyChannels);
    CHECK(!setenv("ST_VST3_SIDECHAIN_FAIL_ACTIVATION", "off", 1));
    CHECK(!st_vst3_configure_input_bus(instance, keyIndex, keyChannels, 0, error, sizeof(error)));
    CHECK(!st_vst3_is_processing(instance));
    CHECK(bus(instance, keyIndex).is_active);
    CHECK(!unsetenv("ST_VST3_SIDECHAIN_FAIL_ACTIVATION"));
    start(instance);
    CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, true, mono);
    configure(instance, keyIndex, keyChannels, false);
    stop(instance);
    CHECK(!setenv("ST_VST3_SIDECHAIN_FAIL_ACTIVATION", "on", 1));
    CHECK(!st_vst3_configure_input_bus(instance, keyIndex, keyChannels, 1, error, sizeof(error)));
    CHECK(!st_vst3_is_processing(instance));
    CHECK(!bus(instance, keyIndex).is_active);
    CHECK(!unsetenv("ST_VST3_SIDECHAIN_FAIL_ACTIVATION"));
    start(instance);
    CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, false, mono);
    configure(instance, keyIndex, keyChannels, true);

    // Component state transitions can fail independently from bus activation.
    // A recoverable failure must restore both the real component and host cache.
    for (const char *failure : {"off-once", "on-once"}) {
        stop(instance);
        CHECK(!setenv("ST_VST3_SIDECHAIN_FAIL_SET_ACTIVE", failure, 1));
        CHECK(!st_vst3_configure_input_bus(instance, keyIndex, keyChannels, 0, error, sizeof(error)));
        CHECK(!st_vst3_is_processing(instance));
        CHECK(bus(instance, keyIndex).is_active && bus(instance, keyIndex).channels == keyChannels);
        CHECK(!unsetenv("ST_VST3_SIDECHAIN_FAIL_SET_ACTIVE"));
        start(instance);
        CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, true, mono);
    }

    STVST3InputBuffer duplicate[] = {program, key, key};
    rejected(instance, audio, duplicate, 3);
    STVST3InputBuffer invalid = key;
    invalid.bus_index = -1; rejected(instance, audio, &invalid, 1);
    invalid.bus_index = count; rejected(instance, audio, &invalid, 1);
    invalid = key; invalid.channel_count = keyChannels == 1 ? 2 : 1; rejected(instance, audio, &invalid, 1);
    invalid.channel_count = -1; rejected(instance, audio, &invalid, 1);
    rejected(instance, audio, nullptr, 1);
    // Failed validation cannot poison subsequent valid rendering.
    CHECK(render(instance, audio, inputs, 2)); audio.expected(64, true, true, mono);

    // Legacy callers still supply the real main bus even when it is input 1.
    audio.sentinel();
    CHECK(st_vst3_process(instance, audio.mainL.data(), audio.mainR.data(), audio.outL.data(), audio.outR.data(), 64, nullptr, 0, nullptr, 0, nullptr));
    audio.expected(64, true, false, mono);
}
}

int main(int argc, char **argv) {
    try {
        CHECK(argc == 2);
        Scope scope;
        CHECK(st_vst3_scan_bundle(argv[1], found, nullptr, error, sizeof(error)));
        CHECK(classes.size() == 4);
        for (int variant = 0; variant < 4; ++variant) {
            const int mainIndex = variant == 2 ? 1 : 0;
            const int keyIndex = variant == 2 ? 0 : 2;
            const int keyChannels = variant == 1 ? 1 : variant == 3 ? 3 : 2;
            const int count = variant == 2 ? 2 : 3;
            auto instance = st_vst3_create(argv[1], classes[variant].uid, 48000, 64, error, sizeof(error));
            CHECK(instance != nullptr); scope.instances.push_back(instance);
            metadata(instance, mainIndex, keyIndex, keyChannels, count);
            if (variant < 3) routing(instance, mainIndex, keyIndex, keyChannels, count);
            else {
                stop(instance);
                CHECK(!st_vst3_configure_input_bus(instance, keyIndex, 3, 1, error, sizeof(error)));
                CHECK(!bus(instance, keyIndex).is_active && bus(instance, keyIndex).channels == 3);
                start(instance);
                Audio audio; STVST3InputBuffer program{mainIndex, 2, audio.program};
                CHECK(render(instance, audio, &program, 1)); audio.expected(64, true, false, false);
            }
        }
        // When reactivation or activation rollback cannot be confirmed, the
        // component stays inactive. Starting DSP must be rejected by the host
        // before calling the processor, even after failure injection is removed.
        for (const bool failRollback : {false, true}) {
            auto instance = st_vst3_create(argv[1], classes[0].uid, 48000, 64, error, sizeof(error));
            CHECK(instance != nullptr); scope.instances.push_back(instance);
            configure(instance, 2, 2, true);
            stop(instance);
            const char *variable = failRollback ? "ST_VST3_SIDECHAIN_FAIL_ACTIVATION" : "ST_VST3_SIDECHAIN_FAIL_SET_ACTIVE";
            CHECK(!setenv(variable, failRollback ? "both" : "on-always", 1));
            CHECK(!st_vst3_configure_input_bus(instance, 2, 2, 0, error, sizeof(error)));
            CHECK(std::strstr(error, "Reload"));
            CHECK(!st_vst3_is_processing(instance));
            CHECK(bus(instance, 2).is_active);
            CHECK(!st_vst3_set_processing(instance, 1, error, sizeof(error)));
            CHECK(std::strstr(error, "inactive")); // Host guard, not plugin's rejection.
            CHECK(!st_vst3_is_processing(instance));
            CHECK(!unsetenv(variable));
            CHECK(!st_vst3_set_processing(instance, 1, error, sizeof(error)));
            CHECK(std::strstr(error, "inactive"));
            CHECK(!st_vst3_is_processing(instance));
            Audio audio; STVST3InputBuffer program{0, 2, audio.program};
            CHECK(!render(instance, audio, &program, 1)); audio.silent(64);
        }
        // State can change a vendor's bus shape without a host API request. The
        // host must reject it before reactivation, then refuse stale-buffer DSP.
        auto mono = scope.instances[1];
        struct State { uint32_t magic; int32_t keyChannels; } changed{0x53434B59, 2};
        CHECK(!st_vst3_restore_component_state(mono, &changed, sizeof(changed), error, sizeof(error)));
        CHECK(!st_vst3_is_processing(mono));
        CHECK(std::strstr(error, "layout") || std::strstr(error, "bus") || std::strstr(error, "reload"));
        Audio audio; STVST3InputBuffer program{0, 2, audio.program};
        CHECK(!render(mono, audio, &program, 1)); audio.silent(64);
        std::printf("PASS VST3 indexed sidechain fixture: stereo/mono key, actual bus indices, activation lifecycle/rollback, prepare/state retention, exact timing, omitted/null-input silence, descriptor rejection, legacy main routing, state layout rejection, no C++ render allocations (%d checks).\n", assertions);
        return 0;
    } catch (const std::exception& failure) {
        std::fprintf(stderr, "FAIL VST3 indexed sidechain fixture: %s\n", failure.what()); return 1;
    }
}
