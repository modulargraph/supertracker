import AudioToolbox

/// Serial render-thread clock. Only a master render timestamp with both valid
/// representations can anchor look-ahead MIDI; an AU's private offset timestamp
/// is never allowed to replace this authority. No allocation or wall-clock read.
struct AudioMIDITimebase {
    private var frame: Int64 = 0
    private var ticks: UInt64 = 0
    private(set) var valid = false
    let ticksPerFrame: Double
    let maximumDistance: Int64

    init(ticksPerFrame: Double, maximumDistance: Int64 = 262_144) {
        self.ticksPerFrame = ticksPerFrame
        self.maximumDistance = maximumDistance
    }

    mutating func reset() { valid = false }

    /// Unflagged host-time storage is undefined, especially in manual rendering.
    /// Zero asks CoreMIDI for immediate delivery rather than scheduling garbage.
    static func hostTime(_ timestamp:AudioTimeStamp)->UInt64 {
        timestamp.mFlags.contains(.hostTimeValid) ? timestamp.mHostTime : 0
    }


    /// Every main quantum invalidates an older anchor before validating the new
    /// one. A graph restart or malformed timestamp cannot retain an old clock.
    mutating func observe(_ timestamp: AudioTimeStamp) {
        valid = false
        let required: AudioTimeStampFlags = [.sampleTimeValid, .hostTimeValid]
        guard timestamp.mFlags.contains(required), timestamp.mHostTime > 0,
              timestamp.mSampleTime.isFinite,
              timestamp.mSampleTime.rounded() == timestamp.mSampleTime,
              abs(timestamp.mSampleTime) < Double(1 << 52),
              ticksPerFrame.isFinite, ticksPerFrame > 0, maximumDistance > 0 else { return }
        frame = Int64(timestamp.mSampleTime); ticks = timestamp.mHostTime; valid = true
    }

    /// Source nodes can retain private sample-time origins after reconnection.
    /// Their valid host timestamp still identifies the same physical render time.
    func graphFrame(atHostTime target:UInt64)->Int64? {
        guard valid,target > 0 else { return nil }
        let delta = target >= ticks ? Double(target-ticks) : -Double(ticks-target)
        let frames=(delta/ticksPerFrame).rounded()
        guard frames.isFinite,abs(frames) <= Double(maximumDistance) else { return nil }
        let (result,overflow)=frame.addingReportingOverflow(Int64(frames))
        return overflow ? nil : result
    }

    func timestamp(atGraphFrame target: Int64) -> UInt64? {
        guard valid else { return nil }
        let (delta, overflow) = target.subtractingReportingOverflow(frame)
        guard !overflow, delta >= -maximumDistance, delta <= maximumDistance else { return nil }
        let offset = (Double(delta) * ticksPerFrame).rounded()
        guard offset.isFinite, abs(offset) < Double(UInt64.max) else { return nil }
        if offset < 0 {
            let amount = UInt64(-offset)
            guard amount < ticks else { return nil }
            return ticks - amount
        }
        let (result, carry) = ticks.addingReportingOverflow(UInt64(offset))
        return carry || result == 0 ? nil : result
    }
}
