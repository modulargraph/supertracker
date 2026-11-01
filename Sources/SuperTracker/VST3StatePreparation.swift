import Foundation

/// Coordinates exact-profile state preparation without holding a render lease
/// across an actor suspension. The caller owns graph quiescence and opaque capture.
enum VST3StatePreparation {
    enum Failure: LocalizedError {
        case stopped, timedOut, changed, replaced, graphChanged
        var errorDescription: String? {
            switch self {
            case .stopped: return "Audio processing stopped while a plug-in edit was being applied. Retry saving."
            case .timedOut: return "A plug-in edit is still waiting for normal audio processing. Retry saving when audio is responding."
            case .changed: return "The plug-in controls changed while their state was being prepared. Retry saving."
            case .replaced: return "The plug-in was replaced while its state was being prepared. Retry saving."
            case .graphChanged: return "Audio processing changed while the plug-in state was being prepared. Retry saving."
            }
        }
    }

    /// The native host owns the durable dirty marker. Dequeuing an edit does not
    /// clear it: only a successful guarded opaque capture can acknowledge it.
    struct CaptureSnapshot: Equatable {
        let editGeneration: UInt64
        let dirty: Bool
        let pendingEdits: UInt32
    }
    struct CapturePreparation: Equatable {
        let editGeneration: UInt64
        let token: UInt64
    }

    struct CaptureTarget {
        let snapshot: () -> CaptureSnapshot
        /// Must include both instance identity and a graph/configuration epoch;
        /// isGraphRunning alone cannot detect a completed rebuild or restart.
        let isCurrent: () -> Bool
        let beginCapture: (_ allowStoppedAudioAdvance: Bool) throws -> UInt64
        let cancelCapture: (_ token: UInt64) -> Void
    }
    struct IndexedCapturePreparation: Equatable {
        let index: Int
        let preparation: CapturePreparation
    }

    /// Exact-profile batch preflight. All dirty instances share one settlement
    /// interval. The caller pauses/drains once around performBegins and resumes
    /// before this helper awaits; no render lease is held across the suspension.
    @MainActor static func prepareCaptures(
        targets: [CaptureTarget],
        isGraphRunning: () -> Bool,
        performBegins: (_ operations: () throws -> Void) throws -> Void = { try $0() },
        settle: () async throws -> Void = { try await Task.sleep(for: .seconds(1)) }
    ) async throws -> [IndexedCapturePreparation] {
        try Task.checkCancellation()
        let originalRunning = isGraphRunning()
        var selected: [(index: Int, initial: CaptureSnapshot)] = []
        var initialStates: [CaptureSnapshot] = []
        for (index, target) in targets.enumerated() {
            guard target.isCurrent() else { throw Failure.replaced }
            let initial = target.snapshot()
            initialStates.append(initial)
            if initial.dirty { selected.append((index, initial)) }
        }
        guard !selected.isEmpty else { return [] }

        func validate() throws -> Bool {
            try Task.checkCancellation()
            guard isGraphRunning() == originalRunning else { throw Failure.graphChanged }
            var pending = false
            for (index, target) in targets.enumerated() {
                guard target.isCurrent() else { throw Failure.replaced }
                let current = target.snapshot()
                guard current.editGeneration == initialStates[index].editGeneration else { throw Failure.changed }
                pending = pending || current.pendingEdits > 0
            }
            return pending
        }
        if originalRunning {
            let clock = ContinuousClock(), deadline = clock.now.advanced(by: .milliseconds(250))
            while try validate() {
                guard clock.now < deadline else { throw Failure.timedOut }
                try await Task.sleep(for: .milliseconds(1))
            }
        }
        _ = try validate()
        var prepared: [IndexedCapturePreparation] = [], completed = false
        defer {
            if !completed {
                for item in prepared { targets[item.index].cancelCapture(item.preparation.token) }
            }
        }
        try performBegins {
            for item in selected {
                try Task.checkCancellation()
                guard targets[item.index].isCurrent() else { throw Failure.replaced }
                guard targets[item.index].snapshot().editGeneration == item.initial.editGeneration else {
                    throw Failure.changed
                }
                // The graph can be paused inside this synchronous batch wrapper;
                // preserve the original running mode when granting advance permission.
                let token = try targets[item.index].beginCapture(!originalRunning)
                prepared.append(.init(index: item.index, preparation: .init(
                    editGeneration: item.initial.editGeneration, token: token)))
            }
        }
        guard try !validate() else { throw Failure.changed }
        // The measured recipe gives the model event loop one shared interval
        // after the last begin, even when normal audio already drained every edit.
        try await settle()
        guard try !validate() else { throw Failure.changed }
        completed = true
        return prepared
    }

    @MainActor static func prepareCapture(
        snapshot: @escaping () -> CaptureSnapshot,
        isCurrent: @escaping () -> Bool,
        isGraphRunning: () -> Bool,
        beginCapture: @escaping (_ allowStoppedAudioAdvance: Bool) throws -> UInt64,
        cancelCapture: @escaping (_ token: UInt64) -> Void,
        settle: () async throws -> Void = { try await Task.sleep(for: .seconds(1)) }
    ) async throws -> CapturePreparation? {
        try await prepareCaptures(targets: [.init(snapshot: snapshot, isCurrent: isCurrent,
            beginCapture: beginCapture, cancelCapture: cancelCapture)], isGraphRunning: isGraphRunning,
            settle: settle).first?.preparation
    }

    @MainActor static func waitForRunningEdits(
        isGraphRunning: () -> Bool,
        hasPendingEdits: () -> Bool,
        timeout: Duration = .milliseconds(250)
    ) async throws {
        try Task.checkCancellation()
        guard isGraphRunning(), hasPendingEdits() else { return }
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: min(.milliseconds(250), max(.zero, timeout)))
        while hasPendingEdits() {
            guard isGraphRunning() else { throw Failure.stopped }
            guard clock.now < deadline else { throw Failure.timedOut }
            // Yield the main actor so UI cancellation and vendor control workers
            // remain responsive while the independent audio thread renders.
            try await Task.sleep(for: .milliseconds(1))
        }
        try Task.checkCancellation()
    }
}
