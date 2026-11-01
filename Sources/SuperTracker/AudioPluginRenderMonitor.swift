import Darwin

/// A render observer records only an atomic error counter. The control thread reads
/// the AU's diagnostic property and builds messages after rendering has returned.
/// Each run reports its first failure once, even if every subsequent render fails.
final class AudioPluginRenderMonitor {
    private let failures = UnsafeMutablePointer<Int64>.allocate(capacity: 1)
    private let firstFailure = UnsafeMutablePointer<Int64>.allocate(capacity: 4)
    private var baseline: Int64 = 0
    private var reported = false

    init() { failures.initialize(to: 0); firstFailure.initialize(repeating:0,count:4) }
    deinit {
        failures.deinitialize(count: 1); failures.deallocate()
        firstFailure.deinitialize(count:4); firstFailure.deallocate()
    }

    @inline(__always) func recordFailure(flags:UInt32,sampleTime:Double,frames:UInt32) {
        if OSAtomicCompareAndSwap64Barrier(0,1,firstFailure) {
            firstFailure[1] = Int64(flags)
            firstFailure[2] = Int64(bitPattern:sampleTime.bitPattern)
            firstFailure[3] = Int64(frames)
            _ = OSAtomicCompareAndSwap64Barrier(1,2,firstFailure)
        }
        OSAtomicIncrement64Barrier(failures)
    }

    /// Control thread only. Reset before starting a new stopped graph/offline pass.
    func reset(resetDiagnostics:Bool = true) {
        baseline = OSAtomicAdd64Barrier(0, failures)
        reported = false
        if resetDiagnostics {
            let old = OSAtomicAdd64Barrier(0,firstFailure)
            _ = OSAtomicCompareAndSwap64Barrier(old,0,firstFailure)
        }
    }

    /// Control-thread diagnostics, captured before later successful subcalls can
    /// overwrite the AU's last-render property. No render policy depends on this.
    func failureContext()->[String:Any]? {
        guard OSAtomicAdd64Barrier(0,firstFailure) == 2 else { return nil }
        return ["flags":firstFailure[1],"sampleTime":String(Double(bitPattern:UInt64(bitPattern:firstFailure[2]))),"frames":firstFailure[3]]
    }

    /// Control thread only. An error remains latched until reset, preventing a
    /// persistently broken live plug-in from creating an alert every timer tick.
    func takeFailure() -> Int64? {
        guard !reported else { return nil }
        let count = OSAtomicAdd64Barrier(0, failures) - baseline
        guard count > 0 else { return nil }
        reported = true
        return count
    }
}
