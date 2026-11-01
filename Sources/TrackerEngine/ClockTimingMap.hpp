#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <vector>

namespace supertracker::clock_detail {

struct TempoEvent { double beat; double bpm; };
struct MeterEvent { double beat; int numerator; int denominator; };

struct TempoSegment {
    double beat;
    double seconds;
    double bpm;
    double secondsPerQuarter;
};
struct MeterSegment {
    double beat;
    std::uint64_t firstBar; // Human-facing bars are one-based.
    int numerator;
    int denominator;
    double quartersPerBar;
};
struct BarPosition {
    std::uint64_t bar;
    double notatedBeat; // One-based denominator units, e.g. 1..5 in 5/8.
    int numerator;
    int denominator;
    double quarterOffset;
    double barStart;
    double barEnd; // Exclusive. A meter event can shorten this bar.
    bool shortBar;
};

// Construct on a control thread; construction validates and may throw
// std::invalid_argument. Queries allocate nothing and never throw. Input event
// order is significant: require ascending unique positions, with an event at 0.
// No mutators or mutable views exist; caller-owned inputs are copied.
class TimingMap final {
public:
    static constexpr std::size_t maxEvents = 1024;
    static constexpr double maxBeat = 1'000'000;
    static constexpr double minBPM = 20;
    static constexpr double maxBPM = 400;

    TimingMap(const std::vector<TempoEvent>& tempos, const std::vector<MeterEvent>& meters);

    const std::vector<TempoSegment>& tempos() const noexcept { return tempos_; }
    const std::vector<MeterSegment>& meters() const noexcept { return meters_; }
    double maxSeconds() const noexcept { return maxSeconds_; }

    // Closed query domain: 0 <= quarter beat <= maxBeat, and corresponding
    // seconds. Invalid, nonfinite and out-of-domain queries return nullopt.
    std::optional<double> secondsAtBeat(double beat) const noexcept;
    std::optional<double> beatAtSeconds(double seconds) const noexcept;
    std::optional<double> tempoAtBeat(double beat) const noexcept;
    std::optional<BarPosition> barAtBeat(double beat) const noexcept;

    // Rejects a beat at/after this bar's exclusive end, including the removed
    // part of a short bar. Use the following bar's beat 1 for that boundary.
    std::optional<double> beatAtBarPosition(std::uint64_t bar, double notatedBeat) const noexcept;

private:
    const std::vector<TempoSegment> tempos_;
    const std::vector<MeterSegment> meters_;
    const double maxSeconds_;
    const std::uint64_t maxBar_;
};

} // namespace supertracker::clock_detail
