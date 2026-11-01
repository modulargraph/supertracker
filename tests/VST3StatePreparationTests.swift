import Foundation

@main enum VST3StatePreparationTests {
    @MainActor static func main() async throws {
        var pending = true
        try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { false }, hasPendingEdits: { pending })
        precondition(pending, "Stopped preparation must not consume or synthesize edits")
        try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { true }, hasPendingEdits: { false })

        let consumer = Task { @MainActor in
            try await Task.sleep(for: .milliseconds(3))
            pending = false
        }
        try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { true }, hasPendingEdits: { pending })
        try await consumer.value
        precondition(!pending, "Waiting must let another main-actor task run")

        do {
            try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { true }, hasPendingEdits: { true }, timeout: .milliseconds(3))
            preconditionFailure("A stuck edit must fail within its deadline")
        } catch VST3StatePreparation.Failure.timedOut {}

        var running = true
        let stopper = Task { @MainActor in
            try await Task.sleep(for: .milliseconds(3)); running = false
        }
        do {
            try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { running }, hasPendingEdits: { true })
            preconditionFailure("A stopped graph must invalidate a running-state wait")
        } catch VST3StatePreparation.Failure.stopped {}
        try await stopper.value

        let cancelled = Task { @MainActor in
            try await VST3StatePreparation.waitForRunningEdits(isGraphRunning: { true }, hasPendingEdits: { true })
        }
        cancelled.cancel()
        do { try await cancelled.value; preconditionFailure("Cancelled save preparation must fail") }
        catch is CancellationError {}
        try await captureFixtureChecks()
        print("PASS VST3 state preparation: stopped/no-op, main-actor progress, bounded timeout, graph stop, cancellation")
    }
}

/// An original asynchronous state model: delivery stages an edit, and a later
/// main-actor turn updates the component state. Dirty acknowledgement is separate.
@MainActor private final class CaptureFixture {
    enum Error: Swift.Error { case invalid, serialization }
    var running = false, current = true, failCapture = false, failBegin = false
    var generation: UInt64 = 0, capturedGeneration: UInt64 = 0, nextToken: UInt64 = 0
    var activeToken: UInt64?
    var pending: UInt32 = 0
    var edited = 0.0740625, component = 0.0740625, staged: Double?
    var syntheticCalls = 0, normalCalls = 0, begins = 0, cancellations = 0, captures = 0, settles = 0
    var snapshot: VST3StatePreparation.CaptureSnapshot {
        .init(editGeneration: generation, dirty: generation != capturedGeneration, pendingEdits: pending)
    }
    func edit(_ value: Double) { generation += 1; edited = value; pending = 1 }
    func normalProcess() { normalCalls += 1; staged = edited; pending = 0 }
    func begin(_ allowStopped: Bool) throws -> UInt64 {
        begins += 1
        if failBegin { throw Error.invalid }
        if pending > 0 {
            guard allowStopped && !running else { throw Error.invalid }
            syntheticCalls += 1; staged = edited; pending = 0
        }
        nextToken += 1; activeToken = nextToken
        return nextToken
    }
    func cancel(_ token: UInt64) {
        if activeToken == token { activeToken = nil; cancellations += 1 }
    }
    func settle() async throws {
        settles += 1; await Task.yield()
        if let staged { component = staged; self.staged = nil }
    }
    func capture(_ prepared: VST3StatePreparation.CapturePreparation) throws -> Double {
        guard current, activeToken == prepared.token, generation == prepared.editGeneration,
              pending == 0 else { throw Error.invalid }
        activeToken = nil
        if failCapture { throw Error.serialization }
        capturedGeneration = generation; captures += 1
        return component
    }
    func prepare(settle operation: (() async throws -> Void)? = nil) async throws -> VST3StatePreparation.CapturePreparation? {
        try await VST3StatePreparation.prepareCapture(snapshot: { self.snapshot },
            isCurrent: { self.current }, isGraphRunning: { self.running },
            beginCapture: { try self.begin($0) }, cancelCapture: { self.cancel($0) },
            settle: { if let operation { try await operation() } else { try await self.settle() } })
    }
    var target: VST3StatePreparation.CaptureTarget {
        .init(snapshot: { self.snapshot }, isCurrent: { self.current },
            beginCapture: { try self.begin($0) }, cancelCapture: { self.cancel($0) })
    }
}

extension VST3StatePreparationTests {
    @MainActor static func captureFixtureChecks() async throws {
        let clean = CaptureFixture()
        let cleanPreparation = try await clean.prepare()
        precondition(cleanPreparation == nil && clean.begins == 0 && clean.settles == 0)

        let stopped = CaptureFixture(); stopped.edit(0.47)
        let prepared = try await stopped.prepare()!
        precondition(stopped.syntheticCalls == 1 && stopped.normalCalls == 0 && stopped.snapshot.dirty)
        let stoppedValue = try stopped.capture(prepared)
        precondition(stoppedValue == 0.47 && !stopped.snapshot.dirty && stopped.captures == 1)

        let running = CaptureFixture(); running.running = true; running.edit(0.31)
        let audio = Task { @MainActor in
            try await Task.sleep(for: .milliseconds(3)); running.normalProcess()
        }
        let runningPreparation = try await running.prepare()!
        try await audio.value
        let runningValue = try running.capture(runningPreparation)
        precondition(runningValue == 0.31 && running.normalCalls == 1 && running.syntheticCalls == 0)

        let drained = CaptureFixture(); drained.running = true; drained.edit(0.62); drained.normalProcess()
        precondition(drained.pending == 0 && drained.snapshot.dirty && drained.component != 0.62)
        let drainedPreparation = try await drained.prepare()!
        precondition(drained.settles == 1 && drained.syntheticCalls == 0 && drained.snapshot.dirty)
        let drainedValue = try drained.capture(drainedPreparation)
        precondition(drainedValue == 0.62 && !drained.snapshot.dirty)

        let cancelled = CaptureFixture(); cancelled.edit(0.48)
        let cancellation = Task { @MainActor in
            try await cancelled.prepare(settle: { try await Task.sleep(for: .seconds(1)) })
        }
        while cancelled.syntheticCalls == 0 { await Task.yield() }
        cancellation.cancel()
        do { _ = try await cancellation.value; preconditionFailure("Cancellation accepted a capture") }
        catch is CancellationError {}
        precondition(cancelled.pending == 0 && cancelled.snapshot.dirty && cancelled.captures == 0 && cancelled.activeToken == nil)
        let retry = try await cancelled.prepare()!
        precondition(cancelled.syntheticCalls == 1, "A drained retry must not advance audio again")
        let retryValue = try cancelled.capture(retry)
        precondition(retryValue == 0.48 && !cancelled.snapshot.dirty)

        let changing = CaptureFixture(); changing.edit(0.2)
        do {
            _ = try await changing.prepare(settle: { changing.edit(0.8); await Task.yield() })
            preconditionFailure("A new edit accepted stale preparation")
        } catch VST3StatePreparation.Failure.changed {}
        precondition(changing.snapshot.dirty && changing.pending == 1 && changing.captures == 0 && changing.activeToken == nil)
        let latest = try await changing.prepare()!
        let latestValue = try changing.capture(latest)
        precondition(latestValue == 0.8)

        let replaced = CaptureFixture(); replaced.edit(0.4)
        do {
            _ = try await replaced.prepare(settle: { replaced.current = false; await Task.yield() })
            preconditionFailure("A replacement accepted stale preparation")
        } catch VST3StatePreparation.Failure.replaced {}
        precondition(replaced.snapshot.dirty && replaced.captures == 0 && replaced.activeToken == nil)

        let graphChanged = CaptureFixture(); graphChanged.edit(0.4)
        do {
            _ = try await graphChanged.prepare(settle: { graphChanged.running = true; await Task.yield() })
            preconditionFailure("A changed graph accepted stale preparation")
        } catch VST3StatePreparation.Failure.graphChanged {}
        precondition(graphChanged.snapshot.dirty && graphChanged.captures == 0)

        // A rebuild can return to the same running Boolean. The rack supplies a
        // captured topology/processing epoch as part of target identity.
        let rebuilt = CaptureFixture(); rebuilt.edit(0.43)
        var graphEpoch: UInt64 = 8
        let initialEpoch = graphEpoch
        let rebuildingTarget = VST3StatePreparation.CaptureTarget(snapshot: { rebuilt.snapshot },
            isCurrent: { rebuilt.current && graphEpoch == initialEpoch },
            beginCapture: { try rebuilt.begin($0) }, cancelCapture: { rebuilt.cancel($0) })
        do {
            _ = try await VST3StatePreparation.prepareCaptures(targets: [rebuildingTarget],
                isGraphRunning: { rebuilt.running }, settle: {
                    rebuilt.running = true; graphEpoch += 1; rebuilt.running = false
                    await Task.yield()
                })
            preconditionFailure("A completed graph rebuild accepted obsolete preparation")
        } catch VST3StatePreparation.Failure.replaced {}
        precondition(!rebuilt.running && rebuilt.snapshot.dirty && rebuilt.activeToken == nil && rebuilt.captures == 0)

        let failed = CaptureFixture(); failed.edit(0.7); failed.failCapture = true
        let failedPreparation = try await failed.prepare()!
        do { _ = try failed.capture(failedPreparation); preconditionFailure("Serialization failure accepted") }
        catch CaptureFixture.Error.serialization {}
        precondition(failed.snapshot.dirty && failed.captures == 0)
        failed.failCapture = false
        let retriedCapture = try await failed.prepare()!
        let capturedValue = try failed.capture(retriedCapture)
        precondition(capturedValue == 0.7 && failed.syntheticCalls == 1)

        // A cancelled older same-generation preparation cannot revoke a newer
        // transaction's token. Native token ownership is tested separately in C++.
        let overlapping = CaptureFixture(); overlapping.edit(0.9)
        let old = try overlapping.begin(true), newer = try overlapping.begin(true)
        overlapping.cancel(old)
        precondition(overlapping.activeToken == newer && overlapping.snapshot.dirty)

        let first = CaptureFixture(), second = CaptureFixture(), untouched = CaptureFixture()
        first.edit(0.11); second.edit(0.22)
        var windows = 0, leases = 0, held = false
        let batch = try await VST3StatePreparation.prepareCaptures(targets: [first.target, second.target, untouched.target],
            isGraphRunning: { false }, performBegins: { operation in
                leases += 1; held = true; defer { held = false }; try operation()
            }, settle: {
                precondition(!held, "No graph quiescence lease may survive the suspension")
                windows += 1; try await first.settle(); try await second.settle()
            })
        precondition(batch.map(\.index) == [0, 1] && windows == 1 && leases == 1 && untouched.begins == 0)
        let firstValue = try first.capture(batch[0].preparation), secondValue = try second.capture(batch[1].preparation)
        precondition(firstValue == 0.11 && secondValue == 0.22)

        let partial = CaptureFixture(), refusal = CaptureFixture()
        partial.edit(0.3); refusal.edit(0.4); refusal.failBegin = true
        do {
            _ = try await VST3StatePreparation.prepareCaptures(targets: [partial.target, refusal.target],
                isGraphRunning: { false }, settle: { preconditionFailure("A failed begin reached settlement") })
            preconditionFailure("A partially prepared batch was accepted")
        } catch CaptureFixture.Error.invalid {}
        precondition(partial.snapshot.dirty && partial.pending == 0 && partial.activeToken == nil &&
                     refusal.snapshot.dirty && refusal.pending == 1 && refusal.activeToken == nil)

        let existingDirty = CaptureFixture(), initiallyClean = CaptureFixture()
        existingDirty.edit(0.55)
        do {
            _ = try await VST3StatePreparation.prepareCaptures(targets: [existingDirty.target, initiallyClean.target],
                isGraphRunning: { false }, settle: { initiallyClean.edit(0.66); await Task.yield() })
            preconditionFailure("A new edit to an initially clean batch member was missed")
        } catch VST3StatePreparation.Failure.changed {}
        precondition(existingDirty.activeToken == nil && existingDirty.snapshot.dirty && initiallyClean.snapshot.dirty)

        let cancelFirst = CaptureFixture(), cancelSecond = CaptureFixture()
        cancelFirst.edit(0.1); cancelSecond.edit(0.2)
        let cancelledBatch = Task { @MainActor in
            try await VST3StatePreparation.prepareCaptures(targets: [cancelFirst.target, cancelSecond.target],
                isGraphRunning: { false }, settle: { try await Task.sleep(for: .seconds(1)) })
        }
        while cancelSecond.begins == 0 { await Task.yield() }
        cancelledBatch.cancel()
        do { _ = try await cancelledBatch.value; preconditionFailure("Cancelled batch was accepted") }
        catch is CancellationError {}
        precondition([cancelFirst, cancelSecond].allSatisfy { $0.activeToken == nil && $0.snapshot.dirty && $0.captures == 0 })
        print("PASS aggregate capture: one shared settlement and quiescence window, clean-member edits detected, partial begin/cancellation preserve every dirty instance")
        print("PASS asynchronous capture model: stopped/running/already-drained edits, cancellation and retry, concurrent edit, instance and graph changes, failed capture and token isolation")
    }
}
