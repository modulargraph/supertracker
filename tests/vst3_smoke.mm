#include "VST3Host.h"
#include "vst3_signal_guard.h"
#import <AppKit/AppKit.h>
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <stdexcept>
#include <string>
#include <vector>

static std::vector<STVST3Class> classes;
static void found(const STVST3Class *item, void *) { classes.push_back(*item); }
static void require(bool condition, const std::string& message) { if (!condition) throw std::runtime_error(message); }
static void guardTeardown() { if (!VST3Diagnostic::installFatalSignalGuard()) { static constexpr char text[] = "DIAGNOSTIC_GUARD_SETUP_FAILED\n"; (void)write(STDERR_FILENO, text, sizeof(text) - 1); _exit(2); } }
static void destroyGuarded(STVST3Instance *&host) { guardTeardown(); if (host) st_vst3_destroy(host); host = nullptr; }
struct PluginScope {
    STVST3Instance *host = nullptr;
    ~PluginScope() { destroyGuarded(host); guardTeardown(); st_vst3_release_unused_modules(); }
};
static uint32_t little(const uint8_t *p) { return uint32_t(p[0]) | uint32_t(p[1]) << 8 | uint32_t(p[2]) << 16 | uint32_t(p[3]) << 24; }
static std::vector<uint8_t> nksState(const char *path) {
    std::ifstream file(path, std::ios::binary); require(bool(file), "NKS fixture missing");
    std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(file)), {});
    require(bytes.size() >= 12 && !memcmp(bytes.data(), "RIFF", 4) && !memcmp(bytes.data() + 8, "NIKS", 4), "NKS header");
    for (size_t p = 12; p + 8 <= bytes.size();) { size_t size = little(bytes.data() + p + 4); require(size <= bytes.size() - p - 8, "NKS chunk bounds"); if (!memcmp(bytes.data() + p, "PCHK", 4)) { require(size > 4 && little(bytes.data() + p + 8) == 1, "PCHK version"); return {bytes.begin() + p + 12, bytes.begin() + p + 8 + size}; } p += 8 + size + (size & 1); }
    throw std::runtime_error("NKS lacks PCHK");
}
static void writeWAV(const std::string& path, const std::vector<float>& data) {
    std::ofstream file(path, std::ios::binary); require(bool(file), "WAV destination");
    auto u16 = [&](uint16_t x) { file.put(x); file.put(x >> 8); }; auto u32 = [&](uint32_t x) { for (int n = 0; n < 4; ++n) file.put(x >> (n * 8)); };
    file.write("RIFF", 4); u32(36 + uint32_t(data.size() * 4)); file.write("WAVEfmt ", 8); u32(16); u16(3); u16(2); u32(48000); u32(48000 * 8); u16(8); u16(32); file.write("data", 4); u32(uint32_t(data.size() * 4)); file.write(reinterpret_cast<const char *>(data.data()), data.size() * 4);
}
static void inspectState(STVST3Instance *host, const char *prefix, const char *stage) {
    char error[2048]{}; void *bytes = nullptr; size_t size = 0;
    require(st_vst3_copy_state(host, &bytes, &size, error, sizeof(error)), error);
    { std::ofstream file(std::string(prefix) + "-" + stage + ".state", std::ios::binary); file.write(static_cast<const char *>(bytes), size); }
    uint64_t hash = 1469598103934665603ULL;
    for (size_t n = 0; n < size; ++n) { hash ^= static_cast<const unsigned char *>(bytes)[n]; hash *= 1099511628211ULL; }
    st_vst3_free(bytes);
    printf("State stage=%s bytes=%zu fingerprint=%016llx\n", stage, size, static_cast<unsigned long long>(hash));
    st_vst3_sync_controller(host);
    for (int n = 0; n < std::min(8, st_vst3_parameter_count(host)); ++n) { STVST3Parameter parameter{}; if (st_vst3_parameter(host, n, &parameter)) printf("Stage %s ID%u %s = %.8f cached=%.8f\n", stage, parameter.id, parameter.title, parameter.value, st_vst3_parameter_value(host, parameter.id)); }
}
static void primeBeforePreset(STVST3Instance *host) {
    float left[512]{}, right[512]{};
    for (int n = 0; n < 16; ++n) require(st_vst3_process(host, nullptr, nullptr, left, right, 512, nullptr, 0, nullptr, 0, nullptr), "Initial silent processing");
    // The initial processing lets a deferred plugin initialization begin; its
    // callbacks/workers then get a bounded opportunity to complete before a preset.
    [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:1]];
    require(st_vst3_process(host, nullptr, nullptr, left, right, 512, nullptr, 0, nullptr, 0, nullptr), "Initial deferred processing completion");
    st_vst3_sync_controller(host);
}
static std::vector<float> render(STVST3Instance *host) {
    constexpr uint32_t block = 512; std::vector<float> left(block), right(block), result;
    for (uint32_t position = 0; position < 96000; position += block) {
        auto count = std::min(block, 96000 - position); STVST3MIDI midi[2]{}; uint32_t midiCount = 0;
        if (position == 0) midi[midiCount++] = {37, 0x90, 60, 100, 60};
        if (position <= 48000 && position + count > 48000) midi[midiCount++] = {int32_t(48000 - position), 0x80, 60, 0, 60};
        STVST3Transport transport{position, double(position) / 24000, 120, 4, 4, 1};
        require(st_vst3_process(host, nullptr, nullptr, left.data(), right.data(), count, midi, midiCount, nullptr, 0, &transport), "VST3 process failed");
        for (uint32_t n = 0; n < count; ++n) { require(std::isfinite(left[n]) && std::isfinite(right[n]), "Nonfinite rendered audio"); result.push_back(left[n]); result.push_back(right[n]); }
    }
    return result;
}
int main(int argc, char **argv) { @autoreleasepool { try {
    if (!VST3Diagnostic::installFatalSignalGuard()) { fprintf(stderr, "FAIL diagnostic fatal-signal containment unavailable\n"); return 2; }
    setvbuf(stdout, nullptr, _IONBF, 0);
    [NSApplication sharedApplication];
    const char *bundle = argc > 1 ? argv[1] : "/Library/Audio/Plug-Ins/VST3/Massive X.vst3";
    const char *preset = argc > 2 ? argv[2] : "/Library/Application Support/Native Instruments/Massive X/Presets/Contender.nksf";
    const char *prefix = argc > 3 ? argv[3] : "test-output/vst3-massive-x";
    PluginScope pluginScope;
    STVST3Instance *&host = pluginScope.host;
    { std::ofstream json(std::string(prefix) + ".stage.json"); json << "{\"passed\":false,\"audioChecksPassed\":false,\"processExitPassed\":false,\"status\":\"running\"}\n"; }
    char error[2048]{}; require(st_vst3_scan_bundle(bundle, found, nullptr, error, sizeof(error)) > 0, error);
    for (const auto& item : classes) printf("Class %s %s %s %s\n", item.uid, item.name, item.vendor, item.category);
    host = st_vst3_create(bundle, classes[0].uid, 48000, 16384, error, sizeof(error)); require(host, error);
    guardTeardown();
    printf("Loaded %d parameters, latency %u, tail %u\n", st_vst3_parameter_count(host), st_vst3_latency(host), st_vst3_tail(host)); fflush(stdout);
    inspectState(host, prefix, "before-prime");
    primeBeforePreset(host);
    inspectState(host, prefix, "after-prime");
    std::vector<uint8_t> state;
    if (strcmp(preset, "-")) { state = nksState(preset); require(st_vst3_restore_component_state(host, state.data(), state.size(), error, sizeof(error)), error); }
    printf("Accepted NKS component state %zu bytes\n", state.size());
    [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:5]];
    { float l[512]{}, r[512]{}; for (int n = 0; n < 16; ++n) require(st_vst3_process(host, nullptr, nullptr, l, r, 512, nullptr, 0, nullptr, 0, nullptr), "Silent state flush"); }
    inspectState(host, prefix, "after-preset");
    std::vector<STVST3Parameter> params; for (int n = 0; n < st_vst3_parameter_count(host); ++n) { STVST3Parameter parameter{}; if (st_vst3_parameter(host, n, &parameter)) params.push_back(parameter); }
    for (size_t n = 0; n < std::min(size_t(12), params.size()); ++n) printf("Parameter ID %u %s = %.8f\n", params[n].id, params[n].title, params[n].value);
    // NKS Contender explicitly maps its first macro page to VST3 IDs 0..7.
    if (std::string(preset).find("Contender.nksf") != std::string::npos) for (uint32_t id = 0; id < 8; ++id) require(std::any_of(params.begin(), params.end(), [&](const auto& p) { return p.id == id; }), "NICA macro ID not exposed by matching plugin");
    void *saved = nullptr; size_t savedSize = 0; require(st_vst3_copy_state(host, &saved, &savedSize, error, sizeof(error)), error);
    { std::ofstream file(std::string(prefix) + ".state", std::ios::binary); file.write(static_cast<const char *>(saved), savedSize); }
    printf("Saved state %zu bytes\n", savedSize);
    auto audio = render(host); double sum = 0, peak = 0; for (float value : audio) { sum += value * value; peak = std::max(peak, std::abs(double(value))); } double rms = sqrt(sum / audio.size());
    printf("Initial note RMS %.9f peak %.9f\n", rms, peak);
    if (rms <= .00001) { destroyGuarded(host); throw std::runtime_error("NKS instrument rendered silence"); } writeWAV(std::string(prefix) + ".wav", audio);
    require(!params.empty(), "No exposed parameters"); auto parameter = params.front();
    double changed = parameter.value > .5 ? .1 : .9; require(st_vst3_set_parameter(host, parameter.id, changed), "Parameter edit rejected");
    require(std::abs(st_vst3_parameter_value(host, parameter.id) - changed) < .000001, "Atomic parameter value did not update");
    // Flush the queued edit to the processor before restoring the saved snapshot.
    float l[1]{}, r[1]{}; require(st_vst3_process(host, nullptr, nullptr, l, r, 1, nullptr, 0, nullptr, 0, nullptr), "Parameter flush");
    // Saving the edited state is a separate requirement from restoring an older
    // snapshot. The original smoke only exercised the latter; do not conflate them.
    void *editedSaved = nullptr; size_t editedSavedSize = 0;
    require(st_vst3_copy_state(host, &editedSaved, &editedSavedSize, error, sizeof(error)), error);
    { std::ofstream file(std::string(prefix) + ".edited.state", std::ios::binary); file.write(static_cast<const char *>(editedSaved), editedSavedSize); }
    require(st_vst3_restore_state(host, saved, savedSize, error, sizeof(error)), error);
    st_vst3_free(saved);
    require(std::abs(st_vst3_parameter_value(host, parameter.id) - parameter.value) < .00001, "State restore did not recover macro value");
    destroyGuarded(host);
    host = st_vst3_create(bundle, classes[0].uid, 48000, 16384, error, sizeof(error)); require(host, error);
    guardTeardown();
    primeBeforePreset(host);
    inspectState(host, prefix, "reopen-before-restore");
    require(st_vst3_restore_state(host, editedSaved, editedSavedSize, error, sizeof(error)), error); st_vst3_free(editedSaved);
    { float l[512]{}, r[512]{}; for (int n = 0; n < 16; ++n) require(st_vst3_process(host, nullptr, nullptr, l, r, 512, nullptr, 0, nullptr, 0, nullptr), "Silent reopen flush"); }
    inspectState(host, prefix, "reopen-after-restore");
    st_vst3_sync_controller(host);
    require(std::abs(st_vst3_parameter_value(host, parameter.id) - changed) < .00001, "Fresh edited-state restore did not recover the edited parameter");
    auto reopened = render(host); double reopenedSum = 0; for (float value : reopened) reopenedSum += value * value;
    require(sqrt(reopenedSum / reopened.size()) > .00001, "Reopened instrument rendered silence");
    require(st_vst3_dropped_events(host) == 0, "Dropped note or automation events"); destroyGuarded(host);
    std::ofstream json(std::string(prefix) + ".stage.json"); json << "{\"passed\":false,\"audioChecksPassed\":true,\"processExitPassed\":false,\"classID\":\"" << classes[0].uid << "\",\"parameters\":" << params.size() << ",\"nksStateBytes\":" << state.size() << ",\"savedStateBytes\":" << savedSize << ",\"rms\":" << rms << ",\"peak\":" << peak << ",\"reopenedRMS\":" << sqrt(reopenedSum / reopened.size()) << "}\n";
    printf("PASS VST3 %s note render, parameter edit, original snapshot restore, edited-state fresh-instance reopen; rms %.8f peak %.8f\n", state.empty() ? "default patch" : "direct NKS", rms, peak); return 0;
} catch (const std::exception& error) { fprintf(stderr, "FAIL %s\n", error.what()); return 1; } } }
