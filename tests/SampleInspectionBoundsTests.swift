import Foundation

// In-memory waveform math only. The build script extracts the exact production
// DecodedSample section, without importing AVFoundation or linking its I/O code.
@main struct SampleInspectionBoundsTests {
    struct Failure: Error, CustomStringConvertible { let description: String }
    static func check(_ value: @autoclosure () -> Bool,_ message:String)throws {if !value(){throw Failure(description:message)}}
    static func tryRange(_ source:DecodedSample,_ payload:[String:Any])->Range<Int>? {try? SampleEditSelection.range(source:source,payload:payload)}
    static func rejects(_ body:()throws->Void)throws {do{try body()}catch is SampleEditSelection.Failure{return};throw Failure(description:"Expected edit selection rejection")}
    static func main() throws {
        let values:[Float]=[-1,-0.5,0.1,0.8,0.7,0.2,-0.1,-0.8]
        let sample=DecodedSample(path:"synthetic",sampleRate:1000,left:values,right:values,originalChannels:2)
        for (input,expected) in [(1e100,4096),(-1e100,1),(1.9,1),(Double.nan,1000),(Double.infinity,1000)] {
            let result=sample.inspect(payload:["bins":input])
            try check((result["min"] as? [Float])?.count == expected,"Bins must be bounded before conversion")
            try check((result["max"] as? [Float])?.count == expected,"Min/max columns must remain aligned")
        }
        print("PASS bounded bins, including finite overflow, negative and nonfinite inputs")
        for direction in [-1e100,0,1e100] {
            let result=sample.inspect(payload:["bins":8,"direction":direction,"window":1e100,"snap":[0.5]])
            let snapped=(result["snapped"] as! [Double])[0]
            try check(snapped == (direction < 0 ? 0.125 : 0.75),"Huge directional searches must retain the intended side")
        }
        for window in [Int.max,Int.min,0,1,3] {
            let snapped=sample.zeroCrossing(near:0.5,direction:0,window:window)
            try check(snapped.isFinite && (0...1).contains(snapped),"Frame windows must not overflow origin arithmetic")
        }
        print("PASS bounded directional and frame windows without integer overflow")
        for position in [Double.greatestFiniteMagnitude,-Double.greatestFiniteMagnitude,Double.nan,Double.infinity,-Double.infinity,0,1] {
            let snapped=sample.zeroCrossing(near:position,direction:0,window:Int.max)
            try check(snapped.isFinite && (0...1).contains(snapped),"Marker positions must be normalized before conversion")
        }
        print("PASS normalized finite/nonfinite positions before multiplication and conversion")
        let regular=sample.inspect(payload:["bins":4,"start":0.25,"end":0.75])
        try check(regular["start"] as? Double == 0.25 && regular["end"] as? Double == 0.75,"Valid zoom boundaries must stay exact")
        try check(regular["min"] as? [Float] == [0.1,0.8,0.7,0.2],"Ordinary waveform columns must be unchanged")
        try check(sample.zeroCrossing(near:0.5,direction:1,window:3) == 0.75,"Forward snap must find the expected crossing")
        try check(sample.zeroCrossing(near:0.5,direction:-1,window:3) == 0.125,"Backward snap must find the expected crossing")
        try check(sample.zeroCrossing(near:0.5,direction:0,window:1) == 0.5,"No crossing in range must retain the marker")
        let empty=DecodedSample(path:"empty",sampleRate:48000,left:[],right:[],originalChannels:2)
        try check((empty.inspect(payload:["bins":1e100])["min"] as? [Float]) == [],"Empty waveform stays empty")
        print("PASS ordinary zoom/snap values, no-crossing fallback and empty sample")
        let support=URL(fileURLWithPath:"/synthetic/application-support",isDirectory:true)
        try check(SampleEditPaths.directory(payload:[:],environment:["SUPERTRACKER_SUPPORT_DIR":"/synthetic/private"],applicationSupport:support).path == "/synthetic/private/Edits","Private sample edits must honor the same support override as recordings/projects")
        try check(SampleEditPaths.directory(payload:["outputDirectory":"/synthetic/explicit"],environment:["SUPERTRACKER_SUPPORT_DIR":"/synthetic/private"],applicationSupport:support).path == "/synthetic/explicit","Explicit destination must take precedence")
        for environment in [[:],["SUPERTRACKER_SUPPORT_DIR":"relative"]] {
            try check(SampleEditPaths.directory(payload:[:],environment:environment,applicationSupport:support).path == "/synthetic/application-support/SuperTracker/Edits","Absent or nonabsolute override must retain the normal support policy")
        }
        print("PASS private edit support root, explicit output and default-path policy without I/O")
        try check(tryRange(sample, [:]) == 0..<8,"Missing selection retains the entire sample")
        try check(tryRange(sample, ["start":0.25,"end":0.75]) == 2..<6,"Ordinary edit boundaries remain exact")
        try check(tryRange(sample, ["start":-1e100,"end":1e100]) == 0..<8,"Huge finite selection positions clamp before integer conversion")
        try check(tryRange(sample, ["start":1e100,"end":-1e100]) == 7..<8,"Reversed/outside selection keeps the prior nonempty single-frame policy")
        for key in ["start","end"] {for invalid:Any in [Double.nan,Double.infinity,-Double.infinity,"not numeric",true,NSNull()] {
            try rejects { _=try SampleEditSelection.range(source:sample,payload:[key:invalid]) }
        }}
        for source in [empty,DecodedSample(path:"bad",sampleRate:1000,left:values,right:[],originalChannels:2),DecodedSample(path:"bad",sampleRate:0,left:values,right:values,originalChannels:2),DecodedSample(path:"bad",sampleRate:Double.nan,left:values,right:values,originalChannels:2),DecodedSample(path:"bad",sampleRate:1000,left:[.nan],right:[0],originalChannels:2)] {
            try rejects {_=try SampleEditSelection.range(source:source,payload:[:])}
        }
        print("PASS finite edit bounds, nonfinite/type rejection, empty/unbalanced PCM and invalid source rate before any I/O")
    }
}
