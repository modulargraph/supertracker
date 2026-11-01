#include "TrackerEngine.h"
#include <algorithm>
#include <array>
#include <atomic>
#include <cassert>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <new>
#include <vector>

// Count C++ allocations made by the audio callback, including event consumption.
// Test setup, result collection and normal control-thread project copies are excluded.
static thread_local bool rendering = false;
static std::atomic<unsigned> renderAllocations{0};
void *operator new(std::size_t size) {
    if (rendering) ++renderAllocations;
    if (void *p = std::malloc(size ? size : 1)) return p;
    throw std::bad_alloc();
}
void *operator new[](std::size_t size) { return ::operator new(size); }
void operator delete(void *p) noexcept { std::free(p); }
void operator delete[](void *p) noexcept { std::free(p); }

struct Engine {
    STEngine *p = st_engine_create(8000);
    Engine() {
        assert(p);
        st_engine_begin_update(p);
        st_engine_set_timing(p, 300, 16, 0, 1); // Exactly 100 samples per row.
        st_engine_set_track_count(p, 3);
        st_engine_set_pattern(p, 0, 8);
        st_engine_set_pattern(p, 1, 8);
        auto instrument = st_default_instrument();
        instrument.type = ST_INSTRUMENT_AU;
        st_engine_set_instrument(p, 0, instrument);
    }
    ~Engine() { st_engine_destroy(p); }
    operator STEngine *() const { return p; }
    void play() { st_engine_commit_update(p); st_engine_play(p, 0); }
};

struct TimedEvent { uint64_t frame; STEvent event; };
using Events = std::vector<TimedEvent>;
struct Destination {
    int track = 0;
    int slot = -1;
    int host = -1;
    uint64_t address = UINT64_MAX;
    bool matches(const STEvent &event) const {
        return event.address == address && (host >= 0
            ? event.type == ST_EVENT_HOST_PARAMETER && event.target == host && event.track == -1
            : event.type == ST_EVENT_PARAMETER && event.target == ST_TARGET_PLUGIN &&
              event.track == track && event.slot == slot);
    }
};

static Events drain(STEngine *engine, uint64_t base = 0) {
    Events events;
    for (;;) {
        STEvent event{};
        rendering = true;
        bool present = st_engine_pop_event(engine, &event);
        rendering = false;
        if (!present) break;
        assert(event.frameOffset >= 0);
        events.push_back({base + uint64_t(event.frameOffset), event});
    }
    return events;
}
static void renderWithoutDrain(STEngine *engine, uint32_t frames) {
    std::array<float, 16384> left{}, right{};
    assert(frames <= left.size());
    rendering = true;
    st_engine_render(engine, left.data(), right.data(), frames);
    rendering = false;
    for (uint32_t n = 0; n < frames; ++n) assert(std::isfinite(left[n]) && std::isfinite(right[n]));
}
static Events render(STEngine *engine, uint32_t frames, uint64_t base = 0, uint32_t block = 127) {
    Events result;
    for (uint32_t at = 0; at < frames; at += block) {
        renderWithoutDrain(engine, std::min(block, frames - at));
        auto events = drain(engine, base + at);
        result.insert(result.end(), events.begin(), events.end());
    }
    return result;
}
static Events renderBus(STEngine *engine, int bus, uint64_t time, uint32_t frames) {
    std::array<float, 1024> left{}, right{};
    assert(frames <= left.size());
    rendering = true;
    st_engine_render_bus(engine, bus, time, left.data(), right.data(), frames);
    rendering = false;
    return drain(engine, time);
}
static Events select(const Events &events, Destination destination) {
    Events result;
    for (const auto &event : events) if (destination.matches(event.event)) result.push_back(event);
    return result;
}
static void at(const TimedEvent &event, uint64_t frame, double value, double tolerance = 0) {
    if (event.frame != frame || std::abs(event.event.value - value) > tolerance)
        std::fprintf(stderr, "Expected frame %llu value %.17g, got frame %llu value %.17g (event %d target %d address %llu)\n",
            (unsigned long long)frame, value, (unsigned long long)event.frame, event.event.value,
            event.event.type, event.event.target, (unsigned long long)event.event.address);
    assert(event.frame == frame);
    assert(std::abs(event.event.value - value) <= tolerance);
}
static void step(STEngine *engine, int track, int row, int note = -3) {
    auto value = st_default_step();
    value.note = note;
    value.gate = 10;
    st_engine_set_step(engine, 0, track, row, value, nullptr, 0);
}
static void lock(STEngine *engine, Destination destination, int row, double value, int column = 0) {
    assert(st_engine_set_plugin_step_fx(engine, 0, destination.track, row, column,
        destination.slot, destination.host, destination.address, value) == 1);
}
static void lane(STEngine *engine, Destination destination, double row, double value, int interpolation = 1) {
    if (destination.host >= 0)
        st_engine_add_host_automation_point(engine, 0, destination.host, destination.address, interpolation, row, value);
    else st_engine_add_automation_point(engine, 0, destination.track, ST_TARGET_PLUGIN, destination.slot,
        destination.address, interpolation, row, value);
}

static void exactRowsAndOpaqueAddresses() {
    for (int host : {-1, 511}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        constexpr double first = 0.12345678901234567, second = 0.9876543210987654;
        step(engine, 0, 1); lock(engine, destination, 1, first);
        step(engine, 0, 3); lock(engine, destination, 3, second);
        engine.play();
        auto events = select(render(engine, 401, 0, 73), destination);
        assert(events.size() == 2);
        at(events[0], 100, first); at(events[1], 300, second);
        assert(st_engine_status(engine).droppedEvents == 0);
    }
}

static void legacySuppressionUsesTrackSlotAndAddress() {
    Engine engine;
    Destination locked{0, -1, -1, UINT64_MAX};
    const Destination otherAddress{0, -1, -1, UINT64_MAX - 1};
    const Destination otherSlot{0, 0, -1, UINT64_MAX};
    const Destination otherTrack{1, -1, -1, UINT64_MAX};
    for (const auto destination : {locked, otherSlot, otherTrack}) lane(engine, destination, 0, .25);
    // This unrelated continuous parameter must continue at the usual 32-frame interval.
    lane(engine, otherAddress, 0, .25, 0); lane(engine, otherAddress, 4, .75, 0);
    step(engine, 0, 0); lock(engine, locked, 0, .875);
    engine.play();
    auto events = render(engine, 101);
    auto target = select(events, locked);
    assert(target.size() == 2); at(target[0], 0, .875); at(target[1], 100, .25);
    for (auto destination : {otherSlot, otherTrack}) {
        auto untouched = select(events, destination);
        assert(untouched.size() == 1); at(untouched[0], 0, .25);
    }
    auto other = select(events, otherAddress);
    assert(other.size() == 4);
    for (size_t n = 0; n < other.size(); ++n) at(other[n], n * 32, .25 + (n * 32) * .5 / 400, 1e-7);
}

static void hostSuppressionUsesDestinationAndAddress() {
    Engine engine;
    Destination locked{0, 2, 127, UINT64_MAX};
    const Destination otherAddress{0, 2, 127, UINT64_MAX - 1};
    const Destination otherOwner{0, 2, 128, UINT64_MAX};
    const Destination legacy{0, 2, -1, UINT64_MAX};
    const Destination unrelatedLegacy{0, 3, -1, UINT64_MAX};
    lane(engine, locked, 0, .25);
    lane(engine, otherAddress, 0, -12.25, 0); lane(engine, otherAddress, 4, 7.75, 0);
    lane(engine, otherOwner, 0, 13.75); lane(engine, legacy, 0, .5);
    lane(engine, unrelatedLegacy, 0, .625);
    step(engine, 0, 0); lock(engine, locked, 0, .875);
    engine.play();
    auto events = render(engine, 101);
    auto target = select(events, locked);
    assert(target.size() == 2); at(target[0], 0, .875); at(target[1], 100, .25);
    auto owner = select(events, otherOwner);
    assert(owner.size() == 1); at(owner[0], 0, 13.75);
    auto direct = select(events, legacy);
    // A resolved host lock also identifies its legacy track/slot alias. Saved
    // indexed lanes must not fight owner-based lanes for that same plug-in.
    assert(direct.size() == 1); at(direct[0], 100, .5);
    auto untouchedLegacy = select(events, unrelatedLegacy);
    assert(untouchedLegacy.size() == 1); at(untouchedLegacy[0], 0, .625);
    auto other = select(events, otherAddress);
    assert(other.size() == 4);
    for (size_t n = 0; n < other.size(); ++n) at(other[n], n * 32, -12.25 + (n * 32) * 20.0 / 400, 1e-12);
}

static void duplicateColumnsCoalesceToFX2() {
    for (int host : {-1, 5}) {
        Engine engine;
        Destination destination{0, 7, host, UINT64_MAX};
        step(engine, 0, 0);
        lock(engine, destination, 0, .125, 0); lock(engine, destination, 0, .875, 1);
        lane(engine, destination, 0, .25);
        engine.play();
        auto events = select(render(engine, 50), destination);
        assert(events.size() == 1); at(events[0], 0, .875);
        st_engine_refresh_automation(engine);
        events = select(render(engine, 1, 50), destination);
        assert(events.size() == 1); at(events[0], 50, .875);
    }
    Engine engine;
    Destination first{0, 0, 10, 17}, second{0, 1, 11, 17};
    step(engine, 0, 0); lock(engine, first, 0, .125, 0); lock(engine, second, 0, .875, 1);
    engine.play();
    auto events = render(engine, 1);
    events.erase(std::remove_if(events.begin(), events.end(), [](const TimedEvent &event) {
        return event.event.type != ST_EVENT_HOST_PARAMETER;
    }), events.end());
    assert(events.size() == 2 && first.matches(events[0].event) && second.matches(events[1].event));
    at(events[0], 0, .125); at(events[1], 0, .875);
}

static void unchangedStepAndLinearLanesResumeAtNextRow() {
    for (int host : {-1, 511}) for (int interpolation : {0, 1}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        lane(engine, destination, 0, .125, interpolation);
        lane(engine, destination, 4, interpolation ? .125 : .625, interpolation);
        step(engine, 0, 1); lock(engine, destination, 1, .875);
        engine.play();
        auto events = select(render(engine, 257, 0, 43), destination);
        for (const auto &event : events) assert(event.frame <= 100 || event.frame >= 200);
        auto locked = std::find_if(events.begin(), events.end(), [](const TimedEvent &event) { return event.frame == 100; });
        auto resumed = std::find_if(events.begin(), events.end(), [](const TimedEvent &event) { return event.frame == 200; });
        assert(locked != events.end() && resumed != events.end());
        at(*locked, 100, .875); at(*resumed, 200, interpolation ? .125 : .375, 1e-7);
        if (interpolation) {
            assert(events.size() == 3); at(events[0], 0, .125);
        } else {
            assert(events.size() == 8);
            for (const auto &event : events) if (event.frame != 100)
                at(event, event.frame, .125 + event.frame * .5 / 400, 1e-7);
        }
    }
}

static void standaloneLocksDoNotInventResets() {
    for (int host : {-1, 511}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        step(engine, 0, 1); lock(engine, destination, 1, .75);
        step(engine, 0, 3); lock(engine, destination, 3, .25);
        engine.play();
        auto events = select(render(engine, 701), destination);
        assert(events.size() == 2); at(events[0], 100, .75); at(events[1], 300, .25);
        st_engine_refresh_automation(engine);
        assert(select(render(engine, 1), destination).empty());
    }
}

static void offAndCutRowsStillApplyLocks() {
    for (int host : {-1, 13}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        step(engine, 0, 0, 60);
        step(engine, 0, 1, -1); lock(engine, destination, 1, .25);
        step(engine, 0, 2, -2); lock(engine, destination, 2, .75);
        engine.play();
        auto events = render(engine, 301);
        auto parameters = select(events, destination);
        assert(parameters.size() == 2); at(parameters[0], 100, .25); at(parameters[1], 200, .75);
        auto off = std::find_if(events.begin(), events.end(), [](const TimedEvent &event) {
            return event.event.type == ST_EVENT_NOTE_OFF && event.event.note == 60;
        });
        assert(off != events.end() && off->frame == 100);
    }
}

static void stopSeekAndProjectReplacementClearActiveLocks() {
    for (int host : {-1, 17}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        lane(engine, destination, 0, .25);
        step(engine, 0, 0); lock(engine, destination, 0, .875);
        engine.play();
        auto events = select(render(engine, 50), destination);
        assert(events.size() == 1); at(events[0], 0, .875);
        st_engine_stop(engine); render(engine, 1);
        st_engine_refresh_automation(engine);
        assert(select(render(engine, 1), destination).empty());
        st_engine_seek(engine, 0, 2, 0); st_engine_play(engine, 0);
        events = select(render(engine, 1), destination);
        assert(events.size() == 1); at(events[0], 0, .25);
        st_engine_seek(engine, 0, 0, 0);
        events = select(render(engine, 1), destination);
        assert(events.size() == 1); at(events[0], 0, .875);
        st_engine_begin_update(engine); step(engine, 0, 0); st_engine_commit_update(engine);
        events = select(render(engine, 1), destination);
        assert(events.size() == 1); at(events[0], 0, .25);
        st_engine_begin_update(engine); st_engine_clear_project(engine); st_engine_commit_update(engine);
        st_engine_refresh_automation(engine);
        assert(select(render(engine, 1), destination).empty());
    }
}

static void mailboxRefreshReemitsEffectiveLockWithoutQueueing() {
    for (int host : {-1, 511}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        lane(engine, destination, 0, .25);
        step(engine, 0, 0); lock(engine, destination, 0, .875);
        engine.play(); render(engine, 47);
        rendering = true;
        for (int n = 0; n < 100000; ++n) st_engine_refresh_automation(engine);
        rendering = false;
        auto events = select(render(engine, 1, 47), destination);
        assert(events.size() == 1); at(events[0], 47, .875);
        assert(select(render(engine, 52, 48), destination).empty());
        events = select(render(engine, 1, 100), destination);
        assert(events.size() == 1); at(events[0], 100, .25);
        assert(st_engine_status(engine).droppedCommands == 0 && st_engine_status(engine).droppedEvents == 0);
    }
}

static void cachedBusOverlapsDoNotReplayEvents() {
    for (int host : {-1, 31}) {
        Engine engine;
        Destination destination{0, -1, host, UINT64_MAX};
        step(engine, 0, 1); lock(engine, destination, 1, .75);
        step(engine, 0, 3); lock(engine, destination, 3, .25);
        engine.play();
        auto events = select(renderBus(engine, 0, 0, 150), destination);
        assert(events.size() == 1); at(events[0], 100, .75);
        auto before = st_engine_status(engine).seconds;
        assert(renderBus(engine, st_return_bus(ST_MIN_RETURN_BUS), 0, 150).empty());
        assert(st_engine_status(engine).seconds == before);
        assert(select(renderBus(engine, 1, 50, 125), destination).empty());
        st_engine_refresh_automation(engine);
        assert(renderBus(engine, 2, 50, 125).empty()); // Entirely cached: mailbox stays pending.
        events = select(renderBus(engine, 0, 125, 101), destination);
        assert(events.size() == 1); at(events[0], 175, .75);
        events = select(renderBus(engine, 1, 200, 151), destination);
        assert(events.size() == 1); at(events[0], 300, .25);
        assert(renderBus(engine, st_return_bus(ST_MIN_RETURN_BUS), 200, 151).empty());
    }
}

static void saturatedQueueRetriesActiveLock() {
    Engine engine;
    st_engine_set_pattern(engine, 0, 128);
    for (int destination = 0; destination < 256; ++destination) {
        st_engine_add_host_automation_point(engine, 0, destination, 1, 0, 0, 0);
        st_engine_add_host_automation_point(engine, 0, destination, 1, 0, 128, 1);
    }
    Destination destination{0, -1, 511, UINT64_MAX};
    step(engine, 0, 120); lock(engine, destination, 120, .75);
    engine.play();
    renderWithoutDrain(engine, 12001);
    assert(st_engine_status(engine).droppedEvents > 0);
    assert(select(drain(engine), destination).empty());
    auto events = select(render(engine, 64, 12001), destination);
    assert(events.size() == 1 && events[0].frame >= 12001 && events[0].frame < 12065);
    assert(events[0].event.value == .75);
}

static void setterValidationAndStepReplacement() {
    Engine engine;
    Destination destination{0, -1, 0, UINT64_MAX};
    step(engine, 0, 0);
    auto set = [&](int pattern, int track, int row, int column, int slot, int host, double value) {
        return st_engine_set_plugin_step_fx(engine, pattern, track, row, column, slot, host, UINT64_MAX, value);
    };
    assert(!st_engine_set_plugin_step_fx(nullptr, 0, 0, 0, 0, -1, 0, 1, .5));
    assert(!set(-1, 0, 0, 0, -1, 0, .5)); assert(!set(256, 0, 0, 0, -1, 0, .5));
    assert(!set(0, -1, 0, 0, -1, 0, .5)); assert(!set(0, ST_TRACK_SANITY_LIMIT, 0, 0, -1, 0, .5));
    assert(!set(0, 0, -1, 0, -1, 0, .5)); assert(!set(0, 0, 8, 0, -1, 0, .5));
    assert(!set(0, 0, 0, -1, -1, 0, .5)); assert(!set(0, 0, 0, 2, -1, 0, .5));
    assert(!set(0, 0, 0, 0, -2, 0, .5)); assert(!set(0, 0, 0, 0, ST_MAX_INSERTS, 0, .5));
    assert(!set(0, 0, 0, 0, -1, -2, .5)); assert(!set(0, 0, 0, 0, -1, ST_MAX_HOST_DESTINATIONS, .5));
    assert(!set(0, 0, 0, 0, -1, 0, -.001)); assert(!set(0, 0, 0, 0, -1, 0, 1.001));
    assert(!set(0, 0, 0, 0, -1, 0, std::numeric_limits<double>::quiet_NaN()));
    assert(!set(0, 0, 0, 0, -1, 0, std::numeric_limits<double>::infinity()));
    // Sixteen inserts per channel: slots 8...15 are valid insert locks.
    assert(set(0, 0, 0, 0, 8, 0, .5)); assert(set(0, 0, 0, 0, ST_MAX_INSERTS - 1, 0, .5));
    lock(engine, destination, 0, .75);
    step(engine, 0, 0); // Replacing the STStep removes its separately stored locks.
    step(engine, 0, 1); lock(engine, destination, 1, .25);
    st_engine_clear_step(engine, 0, 0, 1);
    step(engine, 0, 2); lock(engine, destination, 2, 0);
    step(engine, 0, 3); lock(engine, destination, 3, 1);
    engine.play();
    auto events = select(render(engine, 401), destination);
    assert(events.size() == 2); at(events[0], 200, 0); at(events[1], 300, 1);
}

int main(int argc, char **argv) {
    const struct { const char *name; void (*run)(); } groups[] = {
        {"sample-exact rows, full-width addresses and double values", exactRowsAndOpaqueAddresses},
        {"legacy suppression is local to track, slot and address", legacySuppressionUsesTrackSlotAndAddress},
        {"host suppression is local to destination and address", hostSuppressionUsesDestinationAndAddress},
        {"FX2 duplicate coalescing and distinct-column order", duplicateColumnsCoalesceToFX2},
        {"unchanged stepped and linear lanes resume at next row", unchangedStepAndLinearLanesResumeAtNextRow},
        {"standalone parameter values hold without synthetic resets", standaloneLocksDoNotInventResets},
        {"OFF and CUT rows apply parameter locks", offAndCutRowsStillApplyLocks},
        {"stop, seek and project replacement clear stale locks", stopSeekAndProjectReplacementClearActiveLocks},
        {"coalesced refresh preserves the effective active lock", mailboxRefreshReemitsEffectiveLockWithoutQueueing},
        {"cached bus overlap and refresh have no duplicate events", cachedBusOverlapsDoNotReplayEvents},
        {"event pressure retries a still-active lock", saturatedQueueRetriesActiveLock},
        {"setter validation and step replacement", setterValidationAndStepReplacement},
    };
    const size_t count = sizeof(groups) / sizeof(groups[0]);
    const int selected = argc > 1 ? std::atoi(argv[1]) : 0;
    assert(selected >= 0 && selected <= int(count));
    size_t ran = 0;
    for (size_t index = 0; index < count; ++index) {
        if (selected && selected != int(index + 1)) continue;
        const auto &group = groups[index];
        group.run();
        ++ran;
        std::printf("PASS %s\n", group.name);
    }
    assert(renderAllocations.load() == 0);
    std::printf("PASS plugin step FX: %zu groups; zero C++ render/event-consumer allocations\n",
        ran);
}
