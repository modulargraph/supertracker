import Foundation
import Darwin

/// One serialized native render observer produces these words. Readers never block
/// that observer: a racing snapshot returns nil, so the control side keeps its last
/// coherent value. Allocate outside callbacks and stop callbacks before destruction.
final class RenderHeartbeat: @unchecked Sendable {
    struct Snapshot: Equatable, Sendable {
        let began: UInt64
        let completed: UInt64
        let lastCompletionTicks: UInt64
        var failed: UInt64 = 0
        var lastCompletionSucceeded = true
    }

    private let words: UnsafeMutablePointer<Int64>

    init() {
        words = .allocate(capacity: 6)
        words.initialize(repeating: 0, count: 6)
        words[5] = 1
    }

    deinit { words.deinitialize(count: 6); words.deallocate() }

    /// Pair once at main-mixer PreRender, before entering potentially blocking DSP.
    @inline(__always) func beganRender() {
        OSAtomicIncrement64Barrier(words)
        OSAtomicIncrement64Barrier(words.advanced(by: 1))
        OSAtomicIncrement64Barrier(words)
    }

    /// Pair at PostRender, after the entire quantum returns, even for render errors.
    /// Host ticks must come from the same monotonic clock used by policy callers.
    @inline(__always) func completedRender(at ticks: UInt64, succeeded: Bool = true) {
        OSAtomicIncrement64Barrier(words)
        OSAtomicIncrement64Barrier(words.advanced(by: 2))
        if !succeeded { OSAtomicIncrement64Barrier(words.advanced(by: 4)) }
        let previousResult = OSAtomicAdd64Barrier(0, words.advanced(by: 5))
        _ = OSAtomicCompareAndSwap64Barrier(previousResult, succeeded ? 1 : 0, words.advanced(by: 5))
        let old = OSAtomicAdd64Barrier(0, words.advanced(by: 3))
        _ = OSAtomicCompareAndSwap64Barrier(old, Int64(bitPattern: ticks), words.advanced(by: 3))
        OSAtomicIncrement64Barrier(words)
    }

    func snapshot() -> Snapshot? {
        let before = OSAtomicAdd64Barrier(0, words)
        guard before & 1 == 0 else { return nil }
        let began = UInt64(bitPattern: OSAtomicAdd64Barrier(0, words.advanced(by: 1)))
        let completed = UInt64(bitPattern: OSAtomicAdd64Barrier(0, words.advanced(by: 2)))
        let ticks = UInt64(bitPattern: OSAtomicAdd64Barrier(0, words.advanced(by: 3)))
        let failed = UInt64(bitPattern: OSAtomicAdd64Barrier(0, words.advanced(by: 4)))
        let succeeded = OSAtomicAdd64Barrier(0, words.advanced(by: 5)) != 0
        guard before == OSAtomicAdd64Barrier(0, words) else { return nil }
        return Snapshot(began: began, completed: completed, lastCompletionTicks: ticks, failed: failed, lastCompletionSucceeded: succeeded)
    }
}
