#include "../Sources/TrackerEngine/EffectsDistortion.hpp"
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <memory>
#include <new>
#include <set>
#include <string>
#include <vector>

// Interpose C++ allocations while rendering/resetting. All fixtures and
// expected-output buffers are prepared outside the guarded region.
static bool countAllocations = false;
static size_t renderAllocations = 0;
void *operator new(std::size_t n) {
    if (countAllocations) ++renderAllocations;
    if (void *memory = std::malloc(n ? n : 1)) return memory;
    throw std::bad_alloc();
}
void *operator new[](std::size_t n) { return ::operator new(n); }
void operator delete(void *memory) noexcept { std::free(memory); }
void operator delete[](void *memory) noexcept { std::free(memory); }
void operator delete(void *memory, std::size_t) noexcept { std::free(memory); }
void operator delete[](void *memory, std::size_t) noexcept { std::free(memory); }

namespace {
using stfx::Processor;
using stfx::Descriptor;
constexpr double sampleRate = 48000;

void require(bool condition, const std::string &message) {
    if (!condition) { std::cerr << "FAIL: " << message << '\n'; std::exit(1); }
}

const Descriptor &descriptor(const char *id) {
    size_t count = 0;
    const Descriptor *all = stfx::distortionDescriptors(count);
    for (size_t i = 0; i < count; ++i) if (!std::strcmp(id, all[i].info.id)) return all[i];
    std::abort();
}

struct Effect {
    const Descriptor &info;
    std::unique_ptr<Processor> dsp;
    explicit Effect(const char *id, double sr = sampleRate) : info(descriptor(id)), dsp(info.create(sr, 256)) {}
    void set(const char *id, double value) {
        for (size_t i = 0; i < info.info.parameterCount; ++i) {
            if (!std::strcmp(id, info.params[i].id)) { dsp->p[i] = value; return; }
        }
        require(false, std::string("missing parameter ") + id);
    }
};

struct Audio {
    std::vector<float> left, right;
    explicit Audio(size_t n) : left(n), right(n) {}
    size_t size() const { return left.size(); }
};

Audio tone(size_t frames, double hz = 375, double amplitude = 0.25, double sr = sampleRate) {
    Audio audio(frames);
    for (size_t i = 0; i < frames; ++i)
        audio.left[i] = audio.right[i] = float(amplitude * std::sin(2 * stfx::pi * hz * double(i) / sr));
    return audio;
}

Audio varied(size_t frames) {
    Audio audio(frames);
    uint32_t seed = 1234567;
    for (size_t i = 0; i < frames; ++i) {
        seed = seed * 1664525u + 1013904223u;
        audio.left[i] = float((double(seed) / 4294967296.0 - 0.5) * 0.6);
        audio.right[i] = float(0.3 * std::sin(double(i) * 0.023) + 0.1 * std::sin(double(i) * 0.139));
    }
    return audio;
}

void render(Effect &effect, Audio &audio, size_t chunk = 0) {
    if (!chunk) chunk = audio.size();
    for (size_t offset = 0; offset < audio.size(); offset += chunk) {
        const uint32_t frames = uint32_t(std::min(chunk, audio.size() - offset));
        countAllocations = true;
        effect.dsp->process(audio.left.data() + offset, audio.right.data() + offset, nullptr, nullptr, frames);
        countAllocations = false;
    }
}

double energy(const std::vector<float> &audio, size_t begin = 0) {
    double result = 0;
    for (size_t i = begin; i < audio.size(); ++i) result += double(audio[i]) * audio[i];
    return result;
}

double difference(const std::vector<float> &a, const std::vector<float> &b, size_t begin = 0) {
    double result = 0;
    for (size_t i = begin; i < a.size(); ++i) { const double d = a[i] - b[i]; result += d * d; }
    return result;
}

double binAmplitude(const std::vector<float> &audio, size_t begin, double hz) {
    double real = 0, imaginary = 0;
    for (size_t i = begin; i < audio.size(); ++i) {
        const double phase = 2 * stfx::pi * hz * double(i) / sampleRate;
        real += audio[i] * std::cos(phase); imaginary += audio[i] * std::sin(phase);
    }
    return 2 * std::hypot(real, imaginary) / double(audio.size() - begin);
}

void checkCatalogue() {
    size_t count = 0;
    const Descriptor *all = stfx::distortionDescriptors(count);
    require(count == 3, "three distortion-family descriptors");
    const char *expected[] = {"distortion", "lofi", "saturator"};
    const uint32_t counts[] = {16, 8, 11};
    for (size_t i = 0; i < count; ++i) {
        require(!std::strcmp(all[i].info.id, expected[i]), "stable processor ID/order");
        require(all[i].info.parameterCount == counts[i], "parameter page counts");
        std::set<std::string> ids;
        for (size_t j = 0; j < all[i].info.parameterCount; ++j) {
            const auto &p = all[i].params[j];
            require(p.id && *p.id && p.name && *p.name && p.unit, "complete metadata");
            require(ids.insert(p.id).second, "unique persistent parameter IDs");
            require(p.minimum <= p.defaultValue && p.defaultValue <= p.maximum, "defaults within ranges");
            if (p.labelCount) require(p.labels && p.maximum - p.minimum + 1 == p.labelCount, "enum labels match range");
            if (p.flags & ST_BUILTIN_PARAMETER_LOG) require(p.minimum > 0, "positive logarithmic minimum");
        }
    }
}

void checkCommonContracts() {
    for (const char *id : {"distortion", "lofi", "saturator"}) {
        Effect silence(id);
        Audio zero(2048);
        render(silence, zero);
        require(energy(zero.left) == 0 && energy(zero.right) == 0, std::string(id) + " fresh silence");

        Effect dry(id);
        dry.set("mix", 0);
        Audio input = varied(4097), output = input;
        render(dry, output, 73);
        require(input.left == output.left && input.right == output.right, std::string(id) + " exact zero-mix dry path");

        Effect oneBlock(id), partitioned(id);
        Audio full = input, chunks = input;
        render(oneBlock, full); render(partitioned, chunks, 113);
        require(full.left == chunks.left && full.right == chunks.right, std::string(id) + " partition invariant render");
        countAllocations = true;
        partitioned.dsp->reset();
        countAllocations = false;
        chunks = input; render(partitioned, chunks, 31);
        require(full.left == chunks.left && full.right == chunks.right, std::string(id) + " deterministic reset");

        Effect stereo(id);
        Audio mono = tone(4096); render(stereo, mono, 79);
        require(mono.left == mono.right, std::string(id) + " identical stereo channels remain identical");
        stereo.dsp->reset();
        Audio leftOnly = tone(4096);
        std::fill(leftOnly.right.begin(), leftOnly.right.end(), 0);
        render(stereo, leftOnly, 101);
        require(energy(leftOnly.right) == 0, std::string(id) + " no unintended channel leakage");
        require(energy(leftOnly.left) > 0.01, std::string(id) + " active channel produces sound");
        require(stereo.dsp->latency() == 0 && std::isfinite(stereo.dsp->tail()) && stereo.dsp->tail() > 0,
            std::string(id) + " latency/tail contract");
    }
}

void checkExtremeStability() {
    for (double sr : {8000.0, 44100.0, 192000.0, 384000.0}) {
        for (const char *id : {"distortion", "lofi", "saturator"}) {
            Effect effect(id, sr);
            Audio audio(257);
            for (int block = 0; block < 48; ++block) {
                for (size_t i = 0; i < effect.info.info.parameterCount; ++i)
                    effect.dsp->p[i] = (block % 3 == 0) ? effect.info.params[i].minimum : effect.info.params[i].maximum;
                if (block % 11 == 0) effect.dsp->p[0] = std::numeric_limits<double>::quiet_NaN();
                for (size_t i = 0; i < audio.size(); ++i) {
                    audio.left[i] = float(8 * std::sin((block * audio.size() + i) * 0.13));
                    audio.right[i] = -audio.left[i];
                }
                audio.left[5] = std::numeric_limits<float>::infinity();
                audio.right[6] = std::numeric_limits<float>::quiet_NaN();
                render(effect, audio);
                for (size_t i = 0; i < audio.size(); ++i)
                    require(std::isfinite(audio.left[i]) && std::isfinite(audio.right[i]) &&
                        std::abs(audio.left[i]) < 32 && std::abs(audio.right[i]) < 32,
                        std::string(id) + " bounded extreme render at " + std::to_string(sr));
            }
            // Every state must remain usable after malformed input and extreme automation.
            effect.dsp->reset();
            Audio zero(2048); render(effect, zero);
            require(energy(zero.left) == 0 && energy(zero.right) == 0, std::string(id) + " reset removes all state");
        }
    }
}

void checkFuzzAndAnalog() {
    Effect noFeedback("distortion"), feedback("distortion");
    for (auto *effect : {&noFeedback, &feedback}) {
        effect->set("gate", 0); effect->set("drive", 0); effect->set("output", 0);
        effect->set("color", 100); effect->set("tone", 16000);
    }
    noFeedback.set("feedback", 0); feedback.set("feedback", 95);
    Audio plain(144000), echoed(144000);
    plain.left[0] = plain.right[0] = echoed.left[0] = echoed.right[0] = 0.5f;
    render(noFeedback, plain, 137); render(feedback, echoed, 137);
    require(energy(echoed.left, 500) > energy(plain.left, 500) + 1e-8, "fuzz feedback creates an audible tail");
    require(energy(echoed.left, 96000) < 1e-12, "maximum feedback decays within reported tail");

    Effect openGate("distortion"), gated("distortion");
    for (auto *effect : {&openGate, &gated}) {
        effect->set("drive", 0); effect->set("feedback", 95); effect->set("output", 0);
        effect->set("gate_threshold", -18); effect->set("gate_release", 5);
    }
    openGate.set("gate", 0); gated.set("gate", 1);
    Audio ungated = tone(14400), closed = ungated;
    std::fill(ungated.left.begin() + 4800, ungated.left.end(), 0); ungated.right = ungated.left; closed = ungated;
    render(openGate, ungated); render(gated, closed);
    require(energy(closed.left, 5600) < energy(ungated.left, 5600) * 0.05, "gate suppresses fuzz feedback after input stops");

    Effect bass("distortion"), cutBass("distortion");
    for (auto *effect : {&bass, &cutBass}) {
        effect->set("mode", 1); effect->set("drive", 0); effect->set("saturation", 0); effect->set("output", 0);
    }
    cutBass.set("bass", 0);
    Audio full = tone(24000, 50, 0.03), cut = full;
    render(bass, full); render(cutBass, cut);
    require(energy(cut.left, 12000) < energy(full.left, 12000) * 0.15, "analog bass attenuates low frequencies");

    Effect dark("distortion"), bright("distortion");
    dark.set("mode", 1); bright.set("mode", 1); dark.set("tone", 150); bright.set("tone", 16000);
    Audio dull = tone(16000, 6000, 0.2), clear = dull;
    render(dark, dull); render(bright, clear);
    require(energy(clear.left, 8000) > energy(dull.left, 8000) * 50, "analog tone controls the high cut");
}

void checkLofi() {
    Effect hold("lofi");
    hold.set("sample_rate", 12000); hold.set("bits", 24);
    Audio ramp(64);
    for (size_t i = 0; i < ramp.size(); ++i) ramp.left[i] = ramp.right[i] = float(double(i) / 100.0);
    render(hold, ramp, 7);
    for (size_t i = 0; i < ramp.size(); ++i)
        require(std::abs(ramp.left[i] - double(i / 4 * 4) / 100.0) < 1e-6, "12 kHz hold interval is four frames at 48 kHz");

    Effect crush("lofi");
    crush.set("sample_rate", 48000); crush.set("bits", 4);
    Audio quantized(151);
    for (size_t i = 0; i < quantized.size(); ++i) quantized.left[i] = quantized.right[i] = float((double(i) - 75) / 100.0);
    const Audio original = quantized;
    render(crush, quantized, 17);
    for (size_t i = 0; i < quantized.size(); ++i) {
        require(std::abs(quantized.left[i] * 8 - std::round(quantized.left[i] * 8)) < 1e-7, "four-bit quantization grid");
        require(std::abs(quantized.left[i] - original.left[i]) <= 1.0 / 16 + 1e-7, "four-bit quantization error bound");
        if (i) require(quantized.left[i] >= quantized.left[i - 1], "monotonic quantizer");
    }

    Effect raw("lofi"), smooth("lofi");
    raw.set("sample_rate", 3000); smooth.set("sample_rate", 3000); smooth.set("smooth", 100);
    Audio aliased = tone(24000, 7250, 0.5), filtered = aliased;
    render(raw, aliased); render(smooth, filtered);
    require(energy(filtered.left, 12000) < energy(aliased.left, 12000) * 0.02, "smooth attenuates out-of-band aliasing");

    Effect mono("lofi"); mono.set("stereo", 0);
    Audio stereo = varied(1024); render(mono, stereo);
    require(stereo.left == stereo.right, "zero stereo width folds wet signal to mono");

    Effect jitter("lofi"); jitter.set("jitter", 100);
    Audio first = varied(4096), second = first; render(jitter, first, 51);
    jitter.dsp->reset(); render(jitter, second, 103);
    require(first.left == second.left && first.right == second.right, "clock jitter is deterministic across reset and block size");
    Effect regularClock("lofi");
    Audio regular = varied(4096); render(regularClock, regular);
    require(difference(first.left, regular.left) > 0.1, "clock jitter changes the sample clock");
}

void checkSaturation() {
    Audio input = tone(24000, 400, 0.25);
    std::array<Audio, 3> modes = {input, input, input};
    for (int i = 0; i < 3; ++i) {
        Effect mode("saturator"); mode.set("mode", i); mode.set("drive", 65); mode.set("input", 3);
        render(mode, modes[size_t(i)], 89);
    }
    for (size_t a = 0; a < 3; ++a) for (size_t b = a + 1; b < 3; ++b)
        require(difference(modes[a].left, modes[b].left, 12000) > 1, "saturation modes have distinct responses");

    Effect subtle("saturator"), driven("saturator");
    subtle.set("drive", 0); driven.set("drive", 100);
    Audio low = input, high = input; render(subtle, low); render(driven, high);
    const double lowRatio = binAmplitude(low.left, 12000, 1200) / binAmplitude(low.left, 12000, 400);
    const double highRatio = binAmplitude(high.left, 12000, 1200) / binAmplitude(high.left, 12000, 400);
    require(highRatio > lowRatio * 10, "classic drive increases harmonic content");

    require(binAmplitude(modes[2].left, 12000, 800) > 0.002, "tube mode produces asymmetric even harmonics");
    double mean = 0;
    for (size_t i = 12000; i < modes[2].size(); ++i) mean += modes[2].left[i];
    require(std::abs(mean / 12000) < 0.003, "tube DC blocker removes steady bias");

    Effect neutral("saturator"), eqBypassed("saturator");
    neutral.set("mode", 2); eqBypassed.set("mode", 2); eqBypassed.set("bass", 18);
    eqBypassed.set("treble", -18); eqBypassed.set("eq_bypass", 1);
    Audio normal = input, bypassed = input; render(neutral, normal); render(eqBypassed, bypassed);
    require(normal.left == bypassed.left, "tube EQ bypass removes bass and treble coloration");

    Effect modeChange("saturator"), unchanged("saturator");
    Audio warm = tone(8192), warmCopy = warm; render(modeChange, warm); render(unchanged, warmCopy);
    modeChange.set("mode", 2);
    Audio next = tone(2048), reference = next; render(modeChange, next); render(unchanged, reference);
    require(std::abs(next.left[0] - reference.left[0]) < 0.005, "mode changes crossfade instead of jumping");
    require(difference(next.left, reference.left, 1024) > 0.001, "mode transition reaches the new response");
}

void checkConservativeTails() {
    for (double sr : {8000.0, 48000.0, 192000.0}) {
        for (const char *id : {"distortion", "lofi", "saturator"}) {
            const int modes = !std::strcmp(id, "saturator") ? 3 : (!std::strcmp(id, "distortion") ? 2 : 1);
            for (int mode = 0; mode < modes; ++mode) {
                Effect effect(id, sr);
                if (!std::strcmp(id, "distortion")) {
                    effect.set("mode", mode); effect.set("gate", 0); effect.set("feedback", 95);
                    effect.set("tone_mod", 100); effect.set("tone", 150); effect.set("color", 0);
                    effect.set("drive", 42); effect.set("saturation", 100); effect.set("output", 12);
                    effect.set("mod_rate", 0.05);
                } else if (!std::strcmp(id, "lofi")) {
                    effect.set("sample_rate", 99.5); effect.set("smooth", 100); effect.set("jitter", 100);
                    effect.set("bits", 24); effect.set("input", 24); effect.set("output", 12);
                } else {
                    effect.set("mode", mode); effect.set("input", 24); effect.set("output", 12);
                    effect.set("drive", 100); effect.set("charge", 100); effect.set("overload", 18);
                    effect.set("bass", 18); effect.set("treble", 18); effect.set("contour", 0);
                }
                // Sustained full-scale DC excites slow filter and charge states,
                // rather than relying on an impulse that may miss the sample clock.
                Audio warm(size_t(sr * 2));
                std::fill(warm.left.begin(), warm.left.end(), 1);
                std::fill(warm.right.begin(), warm.right.end(), -1);
                render(effect, warm, 127);
                const double reported = effect.dsp->tailForParameters(effect.dsp->p.data(), 120);
                Audio tail(size_t(std::ceil((reported + 0.1) * sr)));
                render(effect, tail, 127);
                double peak = 0;
                for (size_t i = size_t(std::ceil(reported * sr)); i < tail.size(); ++i)
                    peak = std::max(peak, std::max(std::abs(double(tail.left[i])), std::abs(double(tail.right[i]))));
                require(peak < 1e-6, std::string(id) + " worst-case state decays before reported tail at " + std::to_string(sr));
            }
        }
    }
}
} // namespace

int main() {
    checkCatalogue();
    checkCommonContracts();
    checkExtremeStability();
    checkFuzzAndAnalog();
    checkLofi();
    checkSaturation();
    checkConservativeTails();
    require(renderAllocations == 0, "no C++ allocation in process/reset");
    std::cout << "PASS: distortion family metadata, signal behavior, reset/partition invariance, transitions, stability, and render allocation checks\n";
}
