import Foundation
import CoreAudio

// Line-input routes saved against a physical interface, resolved against the running
// input device. Synthetic sub-device lists only: no hardware is read or changed.
@main struct AudioLineInputTopologyTests {
    typealias M=LineInputTopology.Member
    static let umc="AppleUSBAudioEngine:Acme:AudioBox10:00000001:1,2"
    static let tracker="AppleUSBAudioEngine:Polyend:Tracker Mini:00000002:1,2"
    static let aggregate="~:AMS2_Aggregate:0"
    static func expect(_ value:LineInputTopology.Resolution,_ channels:[Int],_ line:UInt=#line) {
        guard case .live(let got)=value,got == channels else { fatalError("line \(line): expected live \(channels), got \(value)") }
    }
    static func expectPaused(_ value:LineInputTopology.Resolution,_ text:String,_ line:UInt=#line) {
        guard case .unavailable(let reason)=value,reason.contains(text) else { fatalError("line \(line): expected paused containing \(text), got \(value)") }
    }
    static func main() {
        let layout=[M(uid:umc,name:"AudioBox10",inputChannels:10),M(uid:tracker,name:"Tracker Mini",inputChannels:28)]
        let routes:[(String,[Int])]=[("Synth A",[4,5]),("Synth B",[3]),("Synth C",[6,7]),("Synth D",[0,1])]

        // The user's aggregate, AudioBox first: AudioBox channels keep their indices.
        let both=LineInputTopology(uid:aggregate,name:"Tracker + AudioBox",inputChannels:38,members:layout,aggregates:[aggregate:layout])
        for (_,channels) in routes { expect(both.resolve(deviceUID:umc,channels:channels),channels) }
        expect(both.resolve(deviceUID:tracker,channels:[0,1]),[10,11])
        expect(both.resolve(deviceUID:tracker,channels:[27]),[37])
        expectPaused(both.resolve(deviceUID:umc,channels:[10]),"has 10 inputs")
        // Aggregate order decides offsets: AudioBox second → offset by the Tracker Mini's 28.
        let reversed=LineInputTopology(uid:aggregate,name:"Tracker + AudioBox",inputChannels:38,members:Array(layout.reversed()),aggregates:[aggregate:Array(layout.reversed())])
        expect(reversed.resolve(deviceUID:umc,channels:[4,5]),[32,33])
        expect(reversed.resolve(deviceUID:umc,channels:[0,1]),[28,29])
        // A route saved directly against the active device is unchanged.
        expect(both.resolve(deviceUID:aggregate,channels:[30]),[30])

        // AudioBox unplugged: the aggregate keeps only the Tracker Mini (no padding).
        let trackerOnly=LineInputTopology(uid:aggregate,name:"Tracker + AudioBox",inputChannels:28,members:[layout[1]],aggregates:[aggregate:layout])
        expectPaused(trackerOnly.resolve(deviceUID:umc,channels:[4,5]),"AudioBox10 not connected — returns paused")
        expect(trackerOnly.resolve(deviceUID:tracker,channels:[2]),[2])
        // Padded aggregate (absent members keep their channel slots): configured layout applies.
        let padded=LineInputTopology(uid:aggregate,name:"Tracker + AudioBox",inputChannels:38,members:[layout[1]],aggregates:[aggregate:layout])
        expect(padded.resolve(deviceUID:tracker,channels:[2]),[12])
        // Both members gone: zero-channel aggregate pauses everything.
        let empty=LineInputTopology(uid:aggregate,name:"Tracker + AudioBox",inputChannels:0,members:[],aggregates:[aggregate:layout])
        for (_,channels) in routes { expectPaused(empty.resolve(deviceUID:umc,channels:channels),"AudioBox10 not connected") }
        // An inconsistent layout is never guessed.
        let odd=LineInputTopology(uid:aggregate,name:"X",inputChannels:16,members:layout,aggregates:[aggregate:layout])
        expectPaused(odd.resolve(deviceUID:umc,channels:[0]),"Cannot locate AudioBox10")
        // Interface absent from the system entirely: the name comes from its UID.
        let mic=LineInputTopology(uid:"BuiltInMicrophoneDevice",name:"Mic",inputChannels:1)
        expectPaused(mic.resolve(deviceUID:umc,channels:[4,5]),"AudioBox10 not connected — returns paused")
        expectPaused(mic.resolve(deviceUID:"vendor-specific-uid",channels:[0]),"The saved audio interface not connected")
        expectPaused(mic.resolve(deviceUID:"",channels:[0]),"Choose the interface")

        // Reverse: routes saved against the aggregate, now running on the AudioBox directly.
        let direct=LineInputTopology(uid:umc,name:"AudioBox10",inputChannels:10,aggregates:[aggregate:layout],names:[umc:"AudioBox10",aggregate:"Tracker + AudioBox"])
        for (_,channels) in routes { expect(direct.resolve(deviceUID:umc,channels:channels),channels) }
        expect(direct.resolve(deviceUID:aggregate,channels:[4,5]),[4,5])
        expectPaused(direct.resolve(deviceUID:aggregate,channels:[12,13]),"Tracker Mini")
        expectPaused(direct.resolve(deviceUID:aggregate,channels:[9,10]),"Tracker + AudioBox not connected")
        // Reverse with the AudioBox second in the saved aggregate.
        let directReversed=LineInputTopology(uid:umc,name:"AudioBox10",inputChannels:10,aggregates:[aggregate:Array(layout.reversed())])
        expect(directReversed.resolve(deviceUID:aggregate,channels:[32,33]),[4,5])
        // Saved aggregate deleted: no stable mapping.
        expectPaused(LineInputTopology(uid:umc,name:"AudioBox10",inputChannels:10).resolve(deviceUID:aggregate,channels:[4]),"not connected")
        // Saved against one aggregate, running another containing the same member.
        let other="~:AMS2_Aggregate:1"
        let otherLayout=[M(uid:"BuiltInMicrophoneDevice",name:"Mic",inputChannels:1),M(uid:umc,name:"AudioBox10",inputChannels:10)]
        let swapped=LineInputTopology(uid:other,name:"Mic + AudioBox",inputChannels:11,members:otherLayout,aggregates:[aggregate:layout,other:otherLayout])
        expect(swapped.resolve(deviceUID:aggregate,channels:[4,5]),[5,6])

        // Problems are reported once per distinct condition, not on every edit.
        var reporter=LineInputProblemReporter()
        let summary=LineInputProblemReporter.summary([0:"AudioBox10 not connected — returns paused",1:"AudioBox10 not connected — returns paused",3:"AudioBox10 not connected — returns paused"])!
        precondition(summary.hasPrefix("AudioBox10 not connected — returns paused (tracks 1, 2, 4)"),summary)
        precondition(reporter.update(summary) == summary)
        for _ in 0..<20 { precondition(reporter.update(summary) == nil,"Repeated edits re-reported the same pause") }
        precondition(reporter.update(nil) == nil && reporter.reported == nil)
        precondition(reporter.update(summary) == summary,"A recurrence after recovery must be reported again")
        precondition(LineInputProblemReporter.summary([:]) == nil)

        // SuperTracker's own input/output pair: input first (offset 0), clocked by the output.
        let description=AudioDeviceSelection.pairDescription(inputUID:umc,inputChannels:10,outputUID:"BuiltInSpeakerDevice",outputChannels:2)
        let members=description[kAudioAggregateDeviceSubDeviceListKey] as! [[String:Any]]
        precondition(members.map { $0[kAudioSubDeviceUIDKey] as! String } == [umc,"BuiltInSpeakerDevice"])
        precondition(members[0][kAudioSubDeviceInputChannelsKey] as? Int == 10 && members[0][kAudioSubDeviceOutputChannelsKey] as? Int == 0)
        precondition(members[1][kAudioSubDeviceInputChannelsKey] as? Int == 0 && members[1][kAudioSubDeviceOutputChannelsKey] as? Int == 2)
        precondition(members[0][kAudioSubDeviceDriftCompensationKey] as? Int == 1 && members[1][kAudioSubDeviceDriftCompensationKey] as? Int == 0)
        precondition(description[kAudioAggregateDeviceMainSubDeviceKey] as? String == "BuiltInSpeakerDevice")
        precondition(description[kAudioAggregateDeviceIsPrivateKey] as? Bool == true)
        let pair=LineInputTopology(uid:umc,name:"AudioBox10",inputChannels:10)
        for (_,channels) in routes { expect(pair.resolve(deviceUID:umc,channels:channels),channels) }
        print("PASS line-input aggregate mapping, reverse mapping, report-once and pair layout")
    }
}
