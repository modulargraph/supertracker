// Standalone render/control handoff tests. No MIDI client, device, or audio graph is opened.
// swiftc -O -swift-version 5 -module-cache-path /tmp/supertracker-swift-cache \
//   Sources/SuperTracker/MIDIHost.swift Sources/SuperTracker/AudioRenderControl.swift \
//   tests/audio_render_control_tests.swift -o /tmp/supertracker-audio-render-control-tests
// /tmp/supertracker-audio-render-control-tests
// Repeat with -sanitize=thread and a distinct output path to check concurrent access.
import Foundation

private let routeCount = AudioRenderControl.routeCount
private let publicationCount = 100_000

private func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
    guard condition() else {
        throw NSError(domain: "AudioRenderControlTests", code: 1,
                      userInfo: [NSLocalizedDescriptionKey: message])
    }
}

private func sameRoute(_ lhs: AudioMIDIRoute, _ rhs: AudioMIDIRoute) -> Bool {
    guard lhs.external == rhs.external, lhs.destination == rhs.destination,
          lhs.channel == rhs.channel, lhs.leadTicks == rhs.leadTicks else { return false }
    switch (lhs.output, rhs.output) {
    case (nil, nil): return true
    case (let left?, let right?):
        return left.port == right.port && left.endpoint == right.endpoint && left.uniqueID == right.uniqueID
    default: return false
    }
}

/// Every field depends on the publication and index. Optional presence is independent
/// of the payload so nil, zero-valued identifiers, and zero-valued routes remain distinct.
private func patternedRoute(generation: Int, index: Int) -> AudioMIDIRoute {
    let seed = UInt32(truncatingIfNeeded: generation &* 0x9E37_79B1 &+ index &* 0x85EB_CA77)
    var route = AudioMIDIRoute()
    route.external = (seed & 1) != 0
    route.destination = (seed & 2) == 0 ? nil : Int32(bitPattern: seed ^ 0xA5A5_5A5A)
    route.channel = UInt8(truncatingIfNeeded: seed >> 7)
    route.leadTicks = (seed & 8) == 0 ? 0 : seed &* 0x2545_F491
    if (seed & 4) != 0 {
        route.output = MIDIHost.OutputRoute(port: (seed << 13) | (seed >> 19),
                                           endpoint: ~seed,
                                           uniqueID: Int32(bitPattern: seed ^ 0x8000_0000))
    }
    return route
}

private func patternedRoutes(generation: Int, count: Int = routeCount) -> [AudioMIDIRoute] {
    (0..<count).map { patternedRoute(generation: generation, index: $0) }
}

/// Checks the borrowed pointer immediately; it is deliberately never retained across read().
private func validatePattern(_ snapshot: AudioRenderControlSnapshot, maximum: Int) throws -> Int {
    let encodedGeneration = snapshot.tempo - 1_000.125
    guard encodedGeneration.isFinite, encodedGeneration >= 0,
          encodedGeneration <= Double(maximum), encodedGeneration.rounded(.towardZero) == encodedGeneration else {
        throw NSError(domain: "AudioRenderControlTests", code: 2,
                      userInfo: [NSLocalizedDescriptionKey: "Torn or unexpected tempo: \(snapshot.tempo)"])
    }
    let generation = Int(encodedGeneration)
    var index = 0
    while index < routeCount {
        if !sameRoute(snapshot.routes[index], patternedRoute(generation: generation, index: index)) {
            throw NSError(domain: "AudioRenderControlTests", code: 3,
                          userInfo: [NSLocalizedDescriptionKey: "Torn snapshot at publication \(generation), route \(index)"])
        }
        index += 1
    }
    return generation
}

@main
private struct AudioRenderControlTests {
    static func initialAndLengths() throws {
        let control = AudioRenderControl()
        let initial = control.read()
        try require(initial.tempo == 120, "Initial tempo is 120 BPM")
        try require(control.loadCPU().bitPattern == Double(0).bitPattern, "Initial CPU value is positive zero")
        for index in 0..<routeCount {
            try require(sameRoute(initial.routes[index], AudioMIDIRoute()), "Initial route \(index) is zero/nil")
        }

        // Long publication followed by a short/empty one catches uncleared trailing data.
        for count in [0, 1, 63, 64, 65, 127, 3, 0] {
            let routes = patternedRoutes(generation: count + 1, count: count)
            let tempo = Double(count) + 87.25
            control.publish(tempo: tempo, routes: routes)
            let snapshot = control.read()
            try require(snapshot.tempo == tempo, "Tempo survives a \(count)-route publication")
            for index in 0..<routeCount {
                let expected = index < count ? routes[index] : AudioMIDIRoute()
                try require(sameRoute(snapshot.routes[index], expected),
                            "\(count)-route publication truncates or clears route \(index) correctly")
            }
        }
        print("PASS deterministic defaults, empty/short route zero-fill, and long route truncation")
    }

    static func exactEncoding() throws {
        let control = AudioRenderControl()
        let identifiers: [Int32?] = [nil, 0, .min, .max, -1, 1]
        let bitPatterns: [UInt32] = [0, 1, .max, 0x8000_0000, 0x7FFF_FFFF, 0xAAAA_AAAA, 0x5555_5555]
        var routes = [AudioMIDIRoute](repeating: AudioMIDIRoute(), count: routeCount)
        for pass in 0..<8 {
            for index in 0..<routeCount {
                let value = index + pass * routeCount
                var route = AudioMIDIRoute()
                route.external = (value & 1) != 0
                route.destination = identifiers[value % identifiers.count]
                route.channel = UInt8(truncatingIfNeeded: value)
                // Lead ticks share the output identity word: both halves stay independent.
                route.leadTicks = bitPatterns[(value / 7) % bitPatterns.count]
                if value % 3 != 0 {
                    route.output = MIDIHost.OutputRoute(port: bitPatterns[value % bitPatterns.count],
                                                       endpoint: bitPatterns[(value / bitPatterns.count) % bitPatterns.count],
                                                       uniqueID: identifiers[(value + 2) % identifiers.count] ?? 0)
                }
                routes[index] = route
            }
            control.publish(tempo: Double(pass) + 120.75, routes: routes)
            let snapshot = control.read()
            for index in 0..<routeCount {
                try require(sameRoute(snapshot.routes[index], routes[index]),
                            "All route bits survive pass \(pass), index \(index)")
            }
        }

        // Explicitly distinguish an absent output from a present all-zero output.
        var present = AudioMIDIRoute()
        present.destination = 0
        present.output = MIDIHost.OutputRoute(port: 0, endpoint: 0, uniqueID: 0)
        control.publish(tempo: 91, routes: [AudioMIDIRoute(), present])
        let snapshot = control.read()
        try require(sameRoute(snapshot.routes[0], AudioMIDIRoute()), "Nil route fields remain absent")
        try require(sameRoute(snapshot.routes[1], present), "Present zero payloads retain their presence bits")

        let doubleBits: [UInt64] = [0, 0x8000_0000_0000_0000, 1, 0x8000_0000_0000_0001,
                                    0x7FEF_FFFF_FFFF_FFFF, 0xFFEF_FFFF_FFFF_FFFF,
                                    0x7FF0_0000_0000_0000, 0xFFF0_0000_0000_0000,
                                    0x7FF8_1234_ABCD_5678, 0xFFF8_ABCD_1234_5678]
        for bits in doubleBits {
            let value = Double(bitPattern: bits)
            control.publish(tempo: value, routes: [])
            try require(control.read().tempo.bitPattern == bits, "Tempo preserves exact Double bits \(bits)")
            control.storeCPU(value)
            try require(control.loadCPU().bitPattern == bits, "CPU preserves exact Double bits \(bits)")
        }
        print("PASS signed IDs, optional presence, all UInt8 channels, UInt32 route bits, and exact Double encoding")
    }

    static func pausedPublication() throws {
        let control = AudioRenderControl()
        let previous = patternedRoutes(generation: 7)
        let next = patternedRoutes(generation: 8)
        control.publish(tempo: 1_007.125, routes: previous)
        _ = try validatePattern(control.read(), maximum: 8)
        let markedBusy = DispatchSemaphore(value: 0)
        let resume = DispatchSemaphore(value: 0)
        let finished = DispatchSemaphore(value: 0)
        DispatchQueue(label: "AudioRenderControlTests.pausedPublisher").async {
            control.publish(tempo: 1_008.125, routes: next, afterMarkingBusy: {
                markedBusy.signal()
                // A broken spinning reader cannot deadlock the test process indefinitely.
                _ = resume.wait(timeout: .now() + 5)
            })
            finished.signal()
        }
        defer { resume.signal() }
        try require(markedBusy.wait(timeout: .now() + 5) == .success, "Publisher reaches the busy test hook")
        let started = DispatchTime.now().uptimeNanoseconds
        for _ in 0..<10_000 {
            let generation = try validatePattern(control.read(), maximum: 8)
            try require(generation == 7, "Busy publication returns the previous coherent snapshot")
        }
        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started) / 1_000_000_000
        try require(elapsed < 2, "10,000 reads finish promptly while publication is paused (\(elapsed)s)")
        resume.signal()
        try require(finished.wait(timeout: .now() + 5) == .success, "Paused publisher resumes")
        let resumedGeneration = try validatePattern(control.read(), maximum: 8)
        try require(resumedGeneration == 8, "Next read observes completed publication")
        print("PASS 10,000 bounded reads retain the prior snapshot during a paused writer (\(String(format: "%.3f", elapsed))s)")
    }

    static func concurrentSnapshots() throws {
        let control = AudioRenderControl()
        control.publish(tempo: 1_000.125, routes: patternedRoutes(generation: 0))
        let start = DispatchSemaphore(value: 0)
        let finished = DispatchSemaphore(value: 0)
        DispatchQueue(label: "AudioRenderControlTests.snapshotPublisher").async {
            start.wait()
            var routes = [AudioMIDIRoute](repeating: AudioMIDIRoute(), count: routeCount)
            for generation in 1...publicationCount {
                var index = 0
                while index < routeCount {
                    routes[index] = patternedRoute(generation: generation, index: index)
                    index += 1
                }
                control.publish(tempo: Double(generation) + 1_000.125, routes: routes)
            }
            finished.signal()
        }
        // The first snapshot must be cached before allowing the writer to become busy.
        _ = try validatePattern(control.read(), maximum: publicationCount)
        start.signal()
        var reads = 0, lastGeneration = 0, observedChanges = 0
        while reads < publicationCount {
            let generation = try validatePattern(control.read(), maximum: publicationCount)
            try require(generation >= lastGeneration, "Sequential reader never regresses to an older publication")
            if generation != lastGeneration { observedChanges += 1 }
            lastGeneration = generation
            reads += 1
        }
        try require(finished.wait(timeout: .now() + 30) == .success, "100,000 route publications complete")
        let last = try validatePattern(control.read(), maximum: publicationCount)
        try require(last == publicationCount, "Final coherent snapshot contains the final publication")
        print("PASS \(publicationCount) concurrent publications and \(reads) complete-snapshot reads (\(observedChanges) observed changes)")
    }

    /// Each half identifies the same generation; any torn pair is invalid.
    static func cpuBits(_ generation: Int) -> UInt64 {
        let upper = UInt32(0x3FE0_0000) | UInt32(generation)
        return (UInt64(upper) << 32) | UInt64(upper ^ 0xA5C3_9E17)
    }

    static func concurrentCPU() throws {
        let control = AudioRenderControl()
        control.storeCPU(Double(bitPattern: cpuBits(0)))
        let start = DispatchSemaphore(value: 0)
        let finished = DispatchSemaphore(value: 0)
        DispatchQueue(label: "AudioRenderControlTests.cpuWriter").async {
            start.wait()
            for generation in 1...publicationCount {
                control.storeCPU(Double(bitPattern: cpuBits(generation)))
            }
            finished.signal()
        }
        start.signal()
        var lastGeneration: UInt32 = 0, observedChanges = 0
        for _ in 0..<publicationCount {
            let bits = control.loadCPU().bitPattern
            let upper = UInt32(truncatingIfNeeded: bits >> 32)
            let lower = UInt32(truncatingIfNeeded: bits)
            let generation = upper & 0x000F_FFFF
            guard (upper & 0xFFF0_0000) == 0x3FE0_0000,
                  lower == (upper ^ 0xA5C3_9E17), generation <= UInt32(publicationCount) else {
                throw NSError(domain: "AudioRenderControlTests", code: 4,
                              userInfo: [NSLocalizedDescriptionKey: "Torn CPU Double bit pattern: \(bits)"])
            }
            try require(generation >= lastGeneration, "Sequential CPU reader never regresses")
            if generation != lastGeneration { observedChanges += 1 }
            lastGeneration = generation
        }
        try require(finished.wait(timeout: .now() + 10) == .success, "100,000 CPU stores complete")
        try require(control.loadCPU().bitPattern == cpuBits(publicationCount), "Final CPU publication is visible")
        print("PASS \(publicationCount) concurrent 64-bit CPU stores/loads (\(observedChanges) observed changes)")
    }

    static func main() throws {
        try initialAndLengths()
        try exactEncoding()
        try pausedPublication()
        try concurrentSnapshots()
        try concurrentCPU()
        print("PASS all render-control handoff tests; no CoreMIDI runtime access")
    }
}
