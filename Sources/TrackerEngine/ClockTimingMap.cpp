#include "ClockTimingMap.hpp"

#include <algorithm>
#include <cassert>
#include <cmath>
#include <stdexcept>

namespace supertracker::clock_detail {
namespace {

bool validBeat(double beat) noexcept {
    return std::isfinite(beat) && beat >= 0 && beat <= TimingMap::maxBeat;
}

template<class Event>
void validatePositions(const std::vector<Event>& events) {
    if (events.empty() || events.size() > TimingMap::maxEvents)
        throw std::invalid_argument("A timing map requires 1..1024 events.");
    if (events.front().beat != 0)
        throw std::invalid_argument("A timing map must start at quarter beat zero.");
    double previous = -1;
    for (const auto& event : events) {
        if (!validBeat(event.beat) || event.beat <= previous)
            throw std::invalid_argument("Timing positions must be finite, unique, ascending and within 0..1000000 quarter beats.");
        previous = event.beat;
    }
}

std::vector<TempoSegment> compileTempos(const std::vector<TempoEvent>& events) {
    validatePositions(events);
    std::vector<TempoSegment> segments;
    segments.reserve(events.size());
    long double seconds = 0, correction = 0;
    for (std::size_t i = 0; i < events.size(); ++i) {
        const auto& event = events[i];
        if (!std::isfinite(event.bpm) || event.bpm < TimingMap::minBPM || event.bpm > TimingMap::maxBPM)
            throw std::invalid_argument("Tempo must be finite and between 20 and 400 quarter notes per minute.");
        if (i) {
            const auto& previous = events[i - 1];
            // Compensation also helps targets where long double is only 64-bit.
            const long double duration = (static_cast<long double>(event.beat) - previous.beat) * 60 / previous.bpm;
            const long double adjusted = duration - correction;
            const long double next = seconds + adjusted;
            correction = (next - seconds) - adjusted;
            seconds = next;
        }
        const double rounded = static_cast<double>(seconds);
        if (!std::isfinite(rounded) || (i && rounded <= segments.back().seconds))
            throw std::invalid_argument("Tempo boundaries are too close to distinguish in double-precision seconds.");
        segments.push_back({event.beat == 0 ? 0 : event.beat, rounded, event.bpm, 60 / event.bpm});
    }
    return segments;
}

// A rounded start computed this way is the authoritative floating-point bar
// boundary. Comparing against it avoids epsilon-snapping a deliberate short bar.
double barStart(const MeterSegment& segment, std::uint64_t offset) noexcept {
    return std::fma(static_cast<double>(offset), segment.quartersPerBar, segment.beat);
}

std::uint64_t barOffset(const MeterSegment& segment, double beat) noexcept {
    auto offset = static_cast<std::uint64_t>(std::floor((beat - segment.beat) / segment.quartersPerBar));
    if (offset && beat < barStart(segment, offset)) --offset;
    if (beat >= barStart(segment, offset + 1)) ++offset;
    return offset;
}

std::vector<MeterSegment> compileMeters(const std::vector<MeterEvent>& events) {
    validatePositions(events);
    std::vector<MeterSegment> segments;
    segments.reserve(events.size());
    std::uint64_t firstBar = 1;
    for (const auto& event : events) {
        if (event.numerator < 1 || event.numerator > 16 ||
            (event.denominator != 2 && event.denominator != 4 && event.denominator != 8 && event.denominator != 16))
            throw std::invalid_argument("Meter requires numerator 1..16 and denominator 2, 4, 8 or 16.");
        if (!segments.empty()) {
            const auto& previous = segments.back();
            const auto offset = barOffset(previous, event.beat);
            firstBar = previous.firstBar + offset + (event.beat == barStart(previous, offset) ? 0 : 1);
        }
        segments.push_back({event.beat == 0 ? 0 : event.beat, firstBar, event.numerator, event.denominator,
                            static_cast<double>(event.numerator) * 4 / event.denominator});
    }
    return segments;
}

template<class Segment>
std::size_t segmentAtBeat(const std::vector<Segment>& segments, double beat) noexcept {
    const auto next = std::upper_bound(segments.begin(), segments.end(), beat,
        [](double value, const Segment& segment) { return value < segment.beat; });
    return static_cast<std::size_t>(next - segments.begin() - 1);
}

double secondsInSegment(const TempoSegment& segment, double beat) noexcept {
    return std::fma(beat - segment.beat, segment.secondsPerQuarter, segment.seconds);
}

} // namespace

TimingMap::TimingMap(const std::vector<TempoEvent>& tempos, const std::vector<MeterEvent>& meters)
    : tempos_(compileTempos(tempos)), meters_(compileMeters(meters)),
      maxSeconds_(secondsInSegment(tempos_.back(), maxBeat)),
      maxBar_(meters_.back().firstBar + barOffset(meters_.back(), maxBeat)) {
    if (!std::isfinite(maxSeconds_) || maxSeconds_ < tempos_.back().seconds ||
        (tempos_.back().beat < maxBeat && maxSeconds_ == tempos_.back().seconds))
        throw std::invalid_argument("Timing domain cannot be represented in finite seconds.");
}

std::optional<double> TimingMap::secondsAtBeat(double beat) const noexcept {
    if (!validBeat(beat)) return std::nullopt;
    if (beat == maxBeat) return maxSeconds_;
    const auto index = segmentAtBeat(tempos_, beat);
    double result = secondsInSegment(tempos_[index], beat);
    // Adjacent segments share their stored boundary despite prefix rounding.
    if (index + 1 < tempos_.size()) result = std::min(result, tempos_[index + 1].seconds);
    return std::min(result, maxSeconds_);
}

std::optional<double> TimingMap::beatAtSeconds(double seconds) const noexcept {
    if (!std::isfinite(seconds) || seconds < 0 || seconds > maxSeconds_) return std::nullopt;
    if (seconds == maxSeconds_) return maxBeat;
    const auto next = std::upper_bound(tempos_.begin(), tempos_.end(), seconds,
        [](double value, const TempoSegment& segment) { return value < segment.seconds; });
    const auto& segment = *(next - 1);
    if (seconds == segment.seconds) return segment.beat;
    double result = std::fma(seconds - segment.seconds, segment.bpm / 60, segment.beat);
    if (next != tempos_.end()) result = std::min(result, next->beat);
    return std::min(result, maxBeat);
}

std::optional<double> TimingMap::tempoAtBeat(double beat) const noexcept {
    if (!validBeat(beat)) return std::nullopt;
    return tempos_[segmentAtBeat(tempos_, beat)].bpm;
}

std::optional<BarPosition> TimingMap::barAtBeat(double beat) const noexcept {
    if (!validBeat(beat)) return std::nullopt;
    const auto index = segmentAtBeat(meters_, beat);
    const auto& segment = meters_[index];
    const auto offset = barOffset(segment, beat);
    const double start = barStart(segment, offset), nominalEnd = barStart(segment, offset + 1);
    const double end = index + 1 < meters_.size() ? std::min(nominalEnd, meters_[index + 1].beat) : nominalEnd;
    const double quarterOffset = beat - start;
    double notatedBeat = std::fma(quarterOffset, static_cast<double>(segment.denominator) / 4, 1);
    if (notatedBeat >= segment.numerator + 1.0)
        notatedBeat = std::nextafter(segment.numerator + 1.0, 1.0);
    // Adding the one-based label can round an interior point up to a short
    // bar's exclusive end. Keep the label readable back inside its own bar.
    // One label ULP suffices with the validated dyadic denominator units.
    if (notatedBeat > 1 && std::fma(notatedBeat - 1, 4.0 / segment.denominator, start) >= end)
        notatedBeat = std::nextafter(notatedBeat, 1.0);
    assert(std::fma(notatedBeat - 1, 4.0 / segment.denominator, start) < end);
    return BarPosition{segment.firstBar + offset, notatedBeat, segment.numerator, segment.denominator,
                       quarterOffset, start, end, end < nominalEnd};
}

std::optional<double> TimingMap::beatAtBarPosition(std::uint64_t bar, double notatedBeat) const noexcept {
    if (bar < 1 || bar > maxBar_ || !std::isfinite(notatedBeat) || notatedBeat < 1) return std::nullopt;
    const auto next = std::upper_bound(meters_.begin(), meters_.end(), bar,
        [](std::uint64_t value, const MeterSegment& segment) { return value < segment.firstBar; });
    const auto& segment = *(next - 1);
    if (notatedBeat >= segment.numerator + 1.0) return std::nullopt;
    const auto offset = bar - segment.firstBar;
    const double start = barStart(segment, offset), nominalEnd = barStart(segment, offset + 1);
    const double end = next != meters_.end() ? std::min(nominalEnd, next->beat) : nominalEnd;
    const double beat = std::fma(notatedBeat - 1, 4.0 / segment.denominator, start);
    if (!validBeat(beat) || beat >= end) return std::nullopt;
    return beat;
}

} // namespace supertracker::clock_detail
