import AudioToolbox
import Foundation

@main enum AudioMIDITimebaseTests {
    static func main() throws {
        var clock = AudioMIDITimebase(ticksPerFrame: 500)
        func stamp(_ sample:Double,_ host:UInt64,_ flags:AudioTimeStampFlags = [.sampleTimeValid,.hostTimeValid])->AudioTimeStamp {
            var result = AudioTimeStamp(); result.mSampleTime=sample;result.mHostTime=host;result.mFlags=flags;return result
        }
        func check(_ condition:Bool,_ message:String) throws { if !condition { throw NSError(domain:message,code:1) } }
        try check(AudioMIDITimebase.hostTime(stamp(0,UInt64.max,[.sampleTimeValid]))==0,"Unflagged host time scheduled MIDI in the distant future")
        try check(AudioMIDITimebase.hostTime(stamp(0,UInt64.max,[]))==0,"Invalid timestamp storage escaped validation")
        try check(AudioMIDITimebase.hostTime(stamp(0,123456))==123456,"Valid live host clock was discarded")
        try check(clock.timestamp(atGraphFrame:0)==nil,"Unanchored clock")
        clock.observe(stamp(4096,1_000_000_000))
        try check(clock.timestamp(atGraphFrame:4096)==1_000_000_000,"Current frame")
        try check(clock.timestamp(atGraphFrame:8193)==1_002_048_500,"Lookahead remains scheduled in future")
        try check(clock.timestamp(atGraphFrame:4032)==999_968_000,"Latency offset")
        try check(clock.timestamp(atGraphFrame:4096+262145)==nil,"Stale bound")
        clock.observe(stamp(8192,2_000_000_000))
        try check(clock.timestamp(atGraphFrame:8193)==2_000_000_500,"New master clock")
        for invalid in [stamp(0,10,[]),stamp(0,10,[.sampleTimeValid]),stamp(0,10,[.hostTimeValid]),stamp(0,0),stamp(0.5,10),stamp(.nan,10),stamp(.infinity,10)] {
            clock.observe(invalid); try check(clock.timestamp(atGraphFrame:0)==nil,"Invalid stamp retained old anchor")
        }
        clock.observe(stamp(0,100)); try check(clock.timestamp(atGraphFrame:-1)==nil,"Host time underflow")
        clock.observe(stamp(0,UInt64.max-10));try check(clock.timestamp(atGraphFrame:1)==nil,"Host time overflow")
        clock.observe(stamp(0,100));clock.reset();try check(clock.timestamp(atGraphFrame:0)==nil,"Graph reset")
        clock.observe(stamp(1_480_704,1_000_000_000))
        try check(clock.graphFrame(atHostTime:1_000_000_000)==1_480_704,"Private source origin did not align to the master clock")
        try check(clock.graphFrame(atHostTime:1_000_256_000)==1_481_216,"Lookahead source quantum lost its graph offset")
        try check(clock.graphFrame(atHostTime:999_744_000)==1_480_192,"Earlier source quantum lost its graph offset")
        try check(clock.graphFrame(atHostTime:0)==nil,"Invalid source host clock accepted")
        try check(clock.graphFrame(atHostTime:UInt64.max)==nil,"Unbounded source time accepted")
        var fractional=AudioMIDITimebase(ticksPerFrame:500.25);fractional.observe(stamp(0,1_000_000))
        try check(fractional.timestamp(atGraphFrame:2)==1_001_001,"Nearest host tick")
        print("PASS MIDI master-clock authority, lookahead, latency, invalidation and overflow (20 cases)")
    }
}
