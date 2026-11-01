import Darwin

/// Shared counters have no reference back to the endpoint owner. Audio Unit render
/// observers may retain this scope without forming store -> unit -> observer cycles.
final class AudioEndpointReadScope {
    private let readers = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    init() { readers.initialize(to: 0) }
    deinit {
        precondition(activeReaders == 0, "Release render scopes only after callbacks drain.")
        readers.deinitialize(count: 1); readers.deallocate()
    }
    @inline(__always) func beginRead() { OSAtomicIncrement64Barrier(readers) }
    @inline(__always) func endRead() {
        let remaining = OSAtomicDecrement64Barrier(readers)
        precondition(remaining >= 0, "Unbalanced audio endpoint reader scope.")
    }
    var activeReaders: Int { Int(OSAtomicAdd64Barrier(0, readers)) }
}

/// Single-control-writer ownership for atomically published audio callback endpoints.
/// Readers register before loading a pointer and never own the store's references.
/// Replaced values are released only by the creating control thread, after observing
/// that every registered reader has left. A later reader can see only current slots.
///
/// beginRead/endRead may also bracket an entire graph render. This protects removed
/// audio units while downstream processing is still using them, not just while the
/// host is scheduling MIDI. The host must prevent future work on detached graph nodes
/// before collecting them; pause alone is not used as evidence that readers finished.
final class AudioEndpointStore<Value: AnyObject> {
    let capacity: Int
    private let writer = pthread_self()
    private let slots: UnsafeMutablePointer<Int64>
    let readScope = AudioEndpointReadScope()
    private var current: [Value?]
    private var retired: [Value] = []

    init(capacity: Int) {
        precondition(capacity > 0)
        self.capacity = capacity
        slots = .allocate(capacity: capacity)
        slots.initialize(repeating: 0, count: capacity)
        current = [Value?](repeating: nil, count: capacity)
    }

    deinit {
        precondition(pthread_equal(pthread_self(), writer) != 0,
                     "Audio endpoints must be destroyed on their control thread.")
        precondition(activeReaders == 0, "Stop and drain render readers before destroying their endpoints.")
        slots.deinitialize(count: capacity); slots.deallocate()
        // current and retired release their final references on this control thread.
    }

    @inline(__always) func beginRead() { readScope.beginRead() }
    @inline(__always) func endRead() { readScope.endRead() }
    var activeReaders: Int { readScope.activeReaders }

    /// Only the single control writer may inspect ownership arrays or collect them.
    var retiredCount: Int { assertWriter(); return retired.count }

    /// Existence checks do not dereference the published object and need no lease.
    @inline(__always) func hasValue(at index: Int) -> Bool {
        guard index >= 0, index < capacity else { return false }
        return OSAtomicAdd64Barrier(0, slots.advanced(by: index)) != 0
    }

    /// The borrowed value must not escape the callback. The store cannot reclaim it
    /// until the callback and all of its temporary strong references have returned.
    @discardableResult
    func withValue(at index: Int, _ body: (Value) throws -> Void) rethrows -> Bool {
        guard index >= 0, index < capacity else { return false }
        beginRead()
        defer { endRead() }
        let bits = OSAtomicAdd64Barrier(0, slots.advanced(by: index))
        guard bits != 0 else { return false }
        try invokeBorrowed(bits, body)
        return true
    }

    // This boundary is deliberately not inlined. All temporary owned Swift references
    // created from the borrowed pointer are cleaned up before this call returns (also
    // on throw), while the outer reader lease is still active. The outer function uses
    // only pointer bits, so its final endRead cannot precede an endpoint ARC release.
    @inline(never)
    private func invokeBorrowed(_ bits: Int64, _ body: (Value) throws -> Void) rethrows {
        let pointer = UnsafeRawPointer(bitPattern: UInt(UInt64(bitPattern: bits)))!
        let value = Unmanaged<Value>.fromOpaque(pointer).takeUnretainedValue()
        try withExtendedLifetime(value) { try body(value) }
    }

    /// Retain the replacement before publishing it. Keep the old ownership until a
    /// later quiescent observation; no reader ever executes a final store release.
    func publish(_ value: Value?, at index: Int) {
        assertWriter()
        precondition(index >= 0 && index < capacity)
        let previous = current[index]
        current[index] = value
        let bits = value.map { Int64(bitPattern: UInt64(UInt(bitPattern: Unmanaged.passUnretained($0).toOpaque()))) } ?? 0
        let pointer = slots.advanced(by: index)
        let old = OSAtomicAdd64Barrier(0, pointer)
        precondition(OSAtomicCompareAndSwap64Barrier(old, bits, pointer),
                     "Audio endpoint publication has more than one writer.")
        if let previous { retired.append(previous) }
        // Publication does not prove the old graph node was detached. Its render
        // callback may not read the endpoint slot at all, so collection is an explicit
        // later control operation, after no new graph work can enter that old node.
    }

    /// Never waits. A busy graph keeps its retired endpoints until the next control
    /// collection; callers should collect after detaching nodes and from their timer.
    @discardableResult
    func collectRetired() -> Int {
        assertWriter()
        guard readScope.activeReaders == 0 else { return 0 }
        let count = retired.count
        retired.removeAll(keepingCapacity: true)
        return count
    }

    private func assertWriter() {
        precondition(pthread_equal(pthread_self(), writer) != 0,
                     "Audio endpoint ownership belongs to one control thread.")
    }
}
