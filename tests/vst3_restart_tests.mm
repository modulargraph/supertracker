// kLatencyChanged handling and kIsLive marking against the original fixture only.
#include "VST3Host.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <string>
#include <vector>
static std::string uid;
static void found(const STVST3Class *item, void *) { uid = item->uid; }
static int fail(const char *message) { fprintf(stderr, "FAIL %s\n", message); return 1; }
static std::vector<std::string> lines(const char *path) { std::vector<std::string> out; std::ifstream file(path); for (std::string line; std::getline(file, line);) out.push_back(line); return out; }
int main(int argc, char **argv) {
    if (argc != 3) return fail("usage: vst3-restart-tests <Fixture.vst3> <trace>");
    char error[1024]{};
    if (st_vst3_scan_bundle(argv[1], found, nullptr, error, sizeof(error)) != 1) return fail(error);
    STVST3Instance *instance = st_vst3_create(argv[1], uid.c_str(), 48000, 512, error, sizeof(error));
    if (!instance) return fail(error);
    int status = 0;
    do {
        // The fixture requests kLatencyChanged from process(); its new latency is
        // reported only after deactivate/reactivate, as the VST3 specification allows.
        if (!st_vst3_prime(instance, 512, error, sizeof(error))) { status = fail(error); break; }
        if (!(st_vst3_take_restart_flags(instance) & ST_VST3_RESTART_LATENCY_CHANGED)) { status = fail("kLatencyChanged was not recorded"); break; }
        if (st_vst3_latency(instance) != 0) { status = fail("fixture latency changed before reactivation"); break; }
        if (!st_vst3_set_processing(instance, 1, error, sizeof(error))) { status = fail(error); break; }
        if (!st_vst3_restart_component(instance, error, sizeof(error))) { status = fail(error); break; }
        if (st_vst3_latency(instance) != 256) { status = fail("reactivation did not refresh latency to 256"); break; }
        if (!st_vst3_is_processing(instance)) { status = fail("restart did not resume processing"); break; }
        auto order = lines(argv[2]); size_t inactive = 0, active = 0;
        for (size_t n = 0; n < order.size(); ++n) { if (order[n] == "component.inactive") inactive = n; if (order[n] == "component.active") active = n; }
        if (!inactive || active < inactive) { status = fail("restart must deactivate then reactivate the component"); break; }
        float left[16]{}, right[16]{};
        STVST3MIDI notes[] = {{0, 0x90, 60, 100, 1, ST_VST3_MIDI_SEQUENCED}, {8, 0x80, 60, 0, 1, ST_VST3_MIDI_SEQUENCED}, {2, 0x90, 64, 100, 2, 0}, {9, 0x80, 64, 0, 2, 0}};
        if (!st_vst3_process(instance, nullptr, nullptr, left, right, 16, notes, 4, nullptr, 0, nullptr)) { status = fail("process failed"); break; }
        order = lines(argv[2]); int live = 0, sequenced = 0;
        for (auto& line : order) { live += line == "event.on.live"; sequenced += line == "event.on.sequenced"; }
        if (sequenced != 1 || live != 1) { status = fail("sequenced note-ons must not carry kIsLive while live notes keep it"); break; }
    } while (false);
    st_vst3_destroy(instance); st_vst3_release_unused_modules();
    if (!status) printf("PASS VST3 kLatencyChanged restart refreshes latency off the render thread; sequenced notes clear kIsLive.\n");
    return status;
}
