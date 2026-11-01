#include "VST3Host.h"
#include <cassert>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>
struct Scope {
    STVST3Instance *first = nullptr, *second = nullptr;
    ~Scope() { if (first) st_vst3_destroy(first); if (second) st_vst3_destroy(second); st_vst3_release_unused_modules(); }
};
static std::string uid;
static void found(const STVST3Class *item, void *) { uid = item->uid; }
int main(int argc, char **argv) { try {
    assert(argc == 3); char error[1024]{}; Scope scope;
    const uint32_t parameterID = !strcmp(argv[2], "unmapped-midi-zero") ? 0 : 42;
    STVST3ParameterDiagnostics trace{};
    assert(st_vst3_scan_bundle(argv[1], found, nullptr, error, sizeof(error)) == 1);
    if (!strcmp(argv[2], "failure")) { scope.first = st_vst3_create(argv[1], uid.c_str(), 48000, 512, error, sizeof(error)); assert(!scope.first); assert(strstr(error, "float32")); throw std::runtime_error("Expected lifecycle unwind"); }
    scope.first = st_vst3_create(argv[1], uid.c_str(), 48000, 512, error, sizeof(error)); assert(scope.first);
    assert(!strcmp(st_vst3_bundle_version(scope.first), "1.7.1"));
    assert(st_vst3_required_state_flush_frames(scope.first) == 0); // A matching version never substitutes for class identity.
    assert(st_vst3_prime(scope.first, 512, error, sizeof(error)));
    assert(st_vst3_is_instrument(scope.first) && st_vst3_parameter_count(scope.first) == 1);
    assert(st_vst3_watch_parameter(scope.first, parameterID));
    assert(st_vst3_set_parameter(scope.first, parameterID, .875));
    // No render between the edit and save: processor state must still include it.
    void *bytes = nullptr; size_t size = 0; assert(st_vst3_copy_state(scope.first, &bytes, &size, error, sizeof(error)));
    assert(st_vst3_parameter_diagnostics(scope.first, &trace));
    assert(trace.ui_queued == 1 && trace.ui_dequeued == 1 && trace.offered_points == 1 && trace.read_points >= 1);
    assert(trace.last_offered_value == .875 && trace.last_read_value == .875 && trace.last_read_frames == 0 && trace.state_copies == 1);
    scope.second = st_vst3_create(argv[1], uid.c_str(), 48000, 512, error, sizeof(error)); assert(scope.second);
    assert(st_vst3_prime(scope.second, 512, error, sizeof(error)));
    assert(st_vst3_restore_state(scope.second, bytes, size, error, sizeof(error)));
    assert(st_vst3_prepare(scope.second, 48000, 512, error, sizeof(error)));
    std::vector<uint8_t> bad(static_cast<uint8_t *>(bytes), static_cast<uint8_t *>(bytes) + size); bad[8] ^= 1;
    assert(!st_vst3_restore_state(scope.second, bad.data(), bad.size(), error, sizeof(error)));
    st_vst3_free(bytes);
    st_vst3_destroy(scope.first); scope.first = nullptr;
    st_vst3_release_unused_modules(); // Must retain factory while second instance lives.
    float left[16]{}, right[16]{}; STVST3MIDI notes[] = {{4, 0x90, 60, 100, 77}, {12, 0x80, 60, 0, 77}};
    assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, notes, 2, nullptr, 0, nullptr));
    for (int n = 0; n < 16; ++n) { float expected = n >= 4 && n < 12 ? .875f : 0; assert(left[n] == expected && right[n] == expected); }
    st_vst3_sync_controller(scope.second);
    assert(std::abs(st_vst3_parameter_value(scope.second, parameterID) - .875) < .0000001);
    STVST3MIDI held{0, 0x90, 60, 100, 78}; assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, &held, 1, nullptr, 0, nullptr));
    assert(left[15] == .875f && st_vst3_reset(scope.second, error, sizeof(error)));
    assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, nullptr, 0, nullptr, 0, nullptr));
    for (float sample : left) assert(sample == 0);
    assert(st_vst3_watch_parameter(scope.second, parameterID));
    // Export/seek resets can arrive after the graph has stopped. A new static
    // control edit must survive that reset, even while an old note needs panic.
    assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, &held, 1, nullptr, 0, nullptr));
    assert(st_vst3_set_processing(scope.second, 0, error, sizeof(error)));
    assert(st_vst3_set_parameter(scope.second, parameterID, .625));
    assert(st_vst3_reset(scope.second, error, sizeof(error)));
    assert(st_vst3_parameter_diagnostics(scope.second, &trace));
    assert(trace.ui_queued == 1 && trace.ui_dequeued == 0 && trace.pending_ui_edits == 1 && trace.offered_points == 0 && trace.read_points == 0);
    assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, nullptr, 0, nullptr, 0, nullptr));
    for (float sample : left) assert(sample == 0);
    assert(st_vst3_process(scope.second, nullptr, nullptr, left, right, 16, notes, 2, nullptr, 0, nullptr));
    for (int n = 0; n < 16; ++n) { float expected = n >= 4 && n < 12 ? .625f : 0; if (left[n] != expected || right[n] != expected) { fprintf(stderr, "FAIL stopped edit/reset PCM: frame %d expected %.3f, got %.3f/%.3f\n", n, expected, left[n], right[n]); return 1; } }
    assert(st_vst3_copy_state(scope.second, &bytes, &size, error, sizeof(error)));
    assert(st_vst3_parameter_diagnostics(scope.second, &trace));
    assert(trace.ui_queued == 1 && trace.ui_dequeued == 1 && trace.pending_ui_edits == 0);
    assert(trace.last_offered_value == .625 && trace.last_read_value == .625 && trace.last_read_frames == 16);
    assert(trace.offered_process == trace.read_process && trace.resets == 1 && trace.state_copies == 1 && trace.midi_received == 0);
    scope.first = st_vst3_create(argv[1], uid.c_str(), 48000, 512, error, sizeof(error)); assert(scope.first);
    assert(st_vst3_prime(scope.first, 512, error, sizeof(error)));
    assert(st_vst3_restore_state(scope.first, bytes, size, error, sizeof(error)));
    st_vst3_free(bytes);
    assert(st_vst3_process(scope.first, nullptr, nullptr, left, right, 16, notes, 2, nullptr, 0, nullptr));
    for (int n = 0; n < 16; ++n) { float expected = n >= 4 && n < 12 ? .625f : 0; assert(left[n] == expected && right[n] == expected); }
    st_vst3_sync_controller(scope.first);
    assert(std::abs(st_vst3_parameter_value(scope.first, parameterID) - .625) < .0000001);
    // The same guarantee applies to a running processor; coalescing two queued
    // edits happens only in the first ordinary render after panic/reset.
    assert(st_vst3_process(scope.first, nullptr, nullptr, left, right, 16, &held, 1, nullptr, 0, nullptr));
    assert(st_vst3_watch_parameter(scope.first, parameterID));
    assert(st_vst3_set_parameter(scope.first, parameterID, .5));
    assert(st_vst3_set_parameter(scope.first, parameterID, .4375));
    assert(st_vst3_reset(scope.first, error, sizeof(error)));
    assert(st_vst3_parameter_diagnostics(scope.first, &trace));
    assert(trace.pending_ui_edits == 2 && trace.ui_queued == 2 && trace.ui_dequeued == 0 && trace.offered_points == 0 && trace.read_points == 0);
    assert(st_vst3_process(scope.first, nullptr, nullptr, left, right, 16, notes, 2, nullptr, 0, nullptr));
    for (int n = 0; n < 16; ++n) { float expected = n >= 4 && n < 12 ? .4375f : 0; assert(left[n] == expected && right[n] == expected); }
    assert(st_vst3_parameter_diagnostics(scope.first, &trace));
    assert(trace.pending_ui_edits == 0 && trace.ui_dequeued == 2 && trace.offered_points == 1 && trace.last_read_value == .4375 && trace.last_read_frames == 16);
    if (!strcmp(argv[2], "mapped-midi")) {
        STVST3MIDI mapped[] = {{0, 0xb0, 7, 64, -1}, {0, 0x90, 60, 100, 80}};
        assert(st_vst3_watch_parameter(scope.first, parameterID));
        assert(st_vst3_process(scope.first, nullptr, nullptr, left, right, 16, mapped, 2, nullptr, 0, nullptr));
        assert(std::abs(left[15] - 64. / 127.) < .000001);
        assert(st_vst3_parameter_diagnostics(scope.first, &trace));
        assert(trace.midi_received == 1 && trace.read_points >= 1 && trace.last_read_value == 64. / 127.);
    }
    printf("PASS VST3 fixture lifecycle, multi-instance module retention, immediate-edit state save/reopen, stopped-edit reset/reopen PCM, exact MIDI, reset panic, identity rejection.\n");
    return 0;
} catch (const std::exception& error) { printf("Expected error path cleaned: %s\n", error.what()); return 0; } }
