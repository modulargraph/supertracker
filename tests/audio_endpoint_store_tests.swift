// Pure concurrency tests: no Audio Unit registry, audio hardware, or vendor plug-ins.
// swiftc -O -swift-version 5 -module-cache-path /tmp/supertracker-swift-cache \
//   Sources/SuperTracker/AudioEndpointStore.swift tests/audio_endpoint_store_tests.swift \
//   -o /tmp/supertracker-endpoint-tests
// /tmp/supertracker-endpoint-tests
// Add -sanitize=address to the compile command for lifetime/UAF instrumentation.
// Add -sanitize=thread instead to instrument concurrent publication and reclamation.
import Foundation
import Darwin

private final class Counters {
    let writer = pthread_self()
    let created = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    let destroyed = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    let wrongThread = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    let reads = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    let stop = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    init() {
        for pointer in [created, destroyed, wrongThread, reads, stop] { pointer.initialize(to: 0) }
    }
    deinit {
        for pointer in [created, destroyed, wrongThread, reads, stop] { pointer.deinitialize(count: 1); pointer.deallocate() }
    }
    func load(_ pointer: UnsafeMutablePointer<Int64>) -> Int64 { OSAtomicAdd64Barrier(0, pointer) }
}

private final class Endpoint {
    let id: Int64
    let check: Int64
    let counters: Counters
    let renderObserver: (() -> Void)?
    init(_ id: Int64, _ counters: Counters, scope: AudioEndpointReadScope? = nil) {
        self.id = id; check = id ^ 0x1234_5678; self.counters = counters
        // Models an AU-owned render observer: capture only the independent scope.
        renderObserver = scope.map { readScope in
            { readScope.beginRead(); readScope.endRead() }
        }
        OSAtomicIncrement64Barrier(counters.created)
    }
    deinit {
        if pthread_equal(pthread_self(), counters.writer) == 0 { OSAtomicIncrement64Barrier(counters.wrongThread) }
        OSAtomicIncrement64Barrier(counters.destroyed)
    }
    func verify() {
        precondition(check == id ^ 0x1234_5678, "Endpoint memory changed while borrowed.")
        OSAtomicIncrement64Barrier(counters.reads)
    }
}

private enum Expected: Error { case renderFailure }

@main private struct EndpointStoreTests {
    @inline(never)
    private static func retainedScopeFixture(_ counters: Counters)
        -> (scope: AudioEndpointReadScope, storeWasReleased: () -> Bool) {
        let store = AudioEndpointStore<Endpoint>(capacity: 1)
        let scope = store.readScope
        store.publish(Endpoint(1, counters, scope: scope), at: 0)
        store.publish(Endpoint(2, counters, scope: scope), at: 0)
        precondition(store.retiredCount == 1 && counters.load(counters.destroyed) == 0)
        store.withValue(at: 0) { endpoint in
            precondition(scope.activeReaders == 1)
            endpoint.renderObserver?()
            precondition(scope.activeReaders == 1)
        }
        // Returning just the scope must leave neither an ownership cycle through an
        // observer nor an owner reference hidden in the independently retained scope.
        return (scope, { [weak store] in store == nil })
    }

    static func main() throws {
        let counters = Counters()
        let store = AudioEndpointStore<Endpoint>(capacity: 4)

        // A callback deliberately remains inside its borrowed scope while the writer
        // replaces and removes its slot. Neither endpoint may be destroyed in it.
        store.publish(Endpoint(1, counters), at: 0)
        let entered = DispatchSemaphore(value: 0), leave = DispatchSemaphore(value: 0)
        let held = DispatchGroup(); held.enter()
        DispatchQueue.global(qos: .userInteractive).async {
            store.beginRead() // models main-mixer pre-render
            store.withValue(at: 0) { endpoint in
                precondition(store.activeReaders == 2)
                entered.signal(); leave.wait() // test-only, deliberately stalls the callback
                endpoint.verify()
            }
            store.endRead() // models post-render, after the nested callback returns
            held.leave()
        }
        entered.wait()
        store.publish(Endpoint(2, counters), at: 0)
        store.publish(nil, at: 0)
        precondition(store.retiredCount == 2 && counters.load(counters.destroyed) == 0)
        precondition(store.collectRetired() == 0)
        leave.signal(); held.wait()
        precondition(store.activeReaders == 0 && store.collectRetired() == 2)
        precondition(counters.load(counters.destroyed) == 2 && counters.load(counters.wrongThread) == 0)
        print("PASS held render scope preserves retired objects until control-thread collection")

        // Error unwinding must release the borrowed temporary before ending its read
        // registration, matching a failed render callback's deferred scope cleanup.
        store.publish(Endpoint(3, counters), at: 1)
        do {
            try store.withValue(at: 1) { value in value.verify(); throw Expected.renderFailure }
            preconditionFailure("The fixture must throw.")
        } catch Expected.renderFailure { }
        precondition(store.activeReaders == 0)
        store.publish(nil, at: 1)
        store.collectRetired()
        precondition(counters.load(counters.destroyed) == 3 && counters.load(counters.wrongThread) == 0)
        precondition(!store.withValue(at: 1) { _ in preconditionFailure("Empty slot callback") })
        precondition(!store.hasValue(at: -1) && !store.hasValue(at: 4))
        print("PASS throwing, absent-slot, and nested reader paths balance registrations")

        // Continuous concurrent readers race with publication and quiescence checks.
        // A current slot always owns a reference; retired references can only be freed
        // when every reader that might have loaded their pointer has returned.
        let readers = DispatchGroup()
        for worker in 0..<8 {
            readers.enter()
            DispatchQueue.global(qos: .userInteractive).async {
                while counters.load(counters.stop) == 0 {
                    store.withValue(at: worker & 3) { $0.verify() }
                }
                readers.leave()
            }
        }
        for id in 4..<40_004 {
            store.publish(Endpoint(Int64(id), counters), at: id & 3)
            if id & 31 == 0 { store.collectRetired() }
        }
        _ = OSAtomicCompareAndSwap64Barrier(0, 1, counters.stop)
        readers.wait()
        for index in 0..<4 { store.publish(nil, at: index) }
        store.collectRetired()
        precondition(store.activeReaders == 0 && store.retiredCount == 0)
        precondition(counters.load(counters.created) == counters.load(counters.destroyed))
        precondition(counters.load(counters.wrongThread) == 0)
        precondition(counters.load(counters.reads) > 0)
        print("PASS 40000 replacements with 8 concurrent readers: no leaked or audio-thread-destroyed endpoints")

        let scopeCounters = Counters()
        let retained = retainedScopeFixture(scopeCounters)
        precondition(retained.storeWasReleased(), "A retained reader scope must not keep its endpoint store alive.")
        precondition(scopeCounters.load(scopeCounters.created) == 2 && scopeCounters.load(scopeCounters.destroyed) == 2,
                     "Store destruction must reclaim both current and retired observer-owning endpoints.")
        precondition(scopeCounters.load(scopeCounters.wrongThread) == 0,
                     "Endpoints must still be destroyed on the control thread while their read scope survives.")
        precondition(retained.scope.activeReaders == 0)
        retained.scope.beginRead()
        precondition(retained.scope.activeReaders == 1)
        retained.scope.endRead()
        precondition(retained.scope.activeReaders == 0)
        print("PASS retained observer scope outlives store without retaining current or retired endpoints")
        print("Endpoint store concurrency tests passed; reads \(counters.load(counters.reads)).")
    }
}
