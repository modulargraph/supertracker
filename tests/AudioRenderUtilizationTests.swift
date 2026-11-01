import Foundation

@main struct AudioRenderUtilizationTests {
    static func main() {
        var meter = AudioRenderUtilization()
        func finish(_ end: UInt64, frames: UInt32 = 480, rate: Double = 48_000, tick: Double = 0.000_001) -> Double? {
            meter.finish(at:end,frames:frames,sampleRate:rate,secondsPerTick:tick)
        }
        precondition(finish(1000) == nil, "An unmatched post-render cannot publish CPU")
        meter.begin(at:1000,frames:480)
        precondition(abs(finish(1050)! - 0.5) < 1e-12, "A 50 microsecond graph quantum is 0.5%, not 50%")
        precondition(finish(1050) == nil, "A post-render sample is consumed exactly once")
        meter.begin(at:1000,frames:480)
        precondition(finish(11_000) == 100, "The complete frame budget is 100%")
        meter.begin(at:1000,frames:480)
        precondition(abs(finish(12_500)! - 115) < 1e-12, "Overruns remain visible")
        meter.begin(at:0,frames:480)
        precondition(finish(0) == 0, "Zero-valued valid clock origins are supported")
        meter.begin(at:1000,frames:480)
        precondition(finish(999) == nil, "Regressing clocks do not underflow")
        meter.begin(at:1000,frames:480)
        precondition(finish(2000,frames:256) == nil, "Mismatched callback quantum is rejected")
        for invalid in [0.0, -1.0, Double.nan, Double.infinity] {
            meter.begin(at:1000,frames:480)
            precondition(finish(2000,rate:invalid) == nil)
            meter.begin(at:1000,frames:480)
            precondition(finish(2000,tick:invalid) == nil)
        }
        meter.begin(at:0,frames:0)
        precondition(finish(1000,frames:0) == nil)
        meter.begin(at:0,frames:1)
        precondition(finish(UInt64.max,frames:1,rate:Double.greatestFiniteMagnitude,tick:Double.greatestFiniteMagnitude) == nil)
        print("PASS master render utilization: percent units, full budget, overruns, paired callbacks and numeric bounds")
    }
}
