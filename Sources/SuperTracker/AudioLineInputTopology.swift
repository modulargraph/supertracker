import Foundation
import CoreAudio

/// Where a saved line-input route's channels sit in the engine's input stream.
///
/// A route stores the physical interface's CoreAudio UID and that interface's own
/// channel numbers. The engine records one flat stream from the active input device,
/// which may be an aggregate (Audio MIDI Setup's, or SuperTracker's private
/// input/output pair). Channels of an aggregate member are offset by the input
/// channels of the members before it, in the aggregate's order.
struct LineInputTopology: Equatable {
    struct Member: Equatable {
        var uid:String
        var name:String
        var inputChannels:Int
        init(uid:String,name:String="",inputChannels:Int) { self.uid=uid;self.name=name;self.inputChannels=inputChannels }
    }
    enum Resolution: Equatable {
        /// Channel indices in the engine's input stream.
        case live([Int])
        /// Kept as saved; the text is the user-facing status.
        case unavailable(String)
    }
    /// The engine's input device ("" when it is gone).
    var uid:String
    var name:String
    /// Channels the engine's input stream actually carries.
    var inputChannels:Int
    /// Active members of the input device in channel order (empty for a plain device).
    var members:[Member]=[]
    /// Configured member layouts of every aggregate on the system (including the active one).
    var aggregates:[String:[Member]]=[:]
    /// Names of present devices, for status text.
    var names:[String:String]=[:]

    var signature:String {
        "\(uid)#\(inputChannels)#"+members.map { "\($0.uid)=\($0.inputChannels)" }.joined(separator:",")
    }

    func resolve(deviceUID:String,channels:[Int])->Resolution { resolve(deviceUID,channels,reverse:true) }

    private func resolve(_ deviceUID:String,_ channels:[Int],reverse:Bool)->Resolution {
        let label=displayName(deviceUID)
        guard !deviceUID.isEmpty,!channels.isEmpty,channels.allSatisfy({ $0 >= 0 }) else {
            return .unavailable("Choose the interface and channels for this line input again — returns paused")
        }
        if !uid.isEmpty && inputChannels > 0 {
            if deviceUID == uid { return .live(channels) }
            if let layout=activeLayout(),let index=layout.firstIndex(where: { $0.uid == deviceUID }) {
                let member=layout[index]
                guard member.inputChannels > 0 else { return .unavailable("\(label) not connected — returns paused") }
                guard channels.allSatisfy({ $0 < member.inputChannels }) else {
                    return .unavailable("\(label) has \(member.inputChannels) inputs in \(name.isEmpty ? "the aggregate" : name); choose its channels again — returns paused")
                }
                let offset=layout[..<index].reduce(0) { $0+$1.inputChannels }
                return .live(channels.map { $0+offset })
            }
            if activeLayout() == nil,members.contains(where: { $0.uid == deviceUID }) {
                return .unavailable("Cannot locate \(label)'s channels inside \(name.isEmpty ? "the aggregate" : name) — returns paused")
            }
        }
        // Saved against an aggregate that is not running: its configured layout says
        // which member the channels belong to; follow that member to the live device.
        if reverse,let layout=aggregates[deviceUID],deviceUID != uid {
            var offset=0
            for member in layout {
                let range=offset..<(offset+member.inputChannels)
                if channels.allSatisfy(range.contains) {
                    let local=channels.map { $0-offset }
                    if case .live(let mapped)=resolve(member.uid,local,reverse:false) { return .live(mapped) }
                    return .unavailable("\(label) not active (its \(member.name.isEmpty ? displayName(member.uid) : member.name) inputs are not connected) — returns paused")
                }
                offset += member.inputChannels
            }
        }
        return .unavailable("\(label) not connected — returns paused")
    }

    /// Members in stream order whose counts add up to the engine's input channels.
    /// Present members first; a configured (padded) layout if that is what matches.
    private func activeLayout()->[Member]? {
        guard !members.isEmpty || aggregates[uid] != nil else { return nil }
        if !members.isEmpty && members.reduce(0,{ $0+$1.inputChannels }) == inputChannels { return members }
        if let configured=aggregates[uid],configured.reduce(0,{ $0+$1.inputChannels }) == inputChannels {
            // Members not present contribute no channels unless the configured layout pads them.
            return configured
        }
        return nil
    }

    func displayName(_ deviceUID:String)->String {
        if let name=names[deviceUID],!name.isEmpty { return name }
        for layout in aggregates.values { if let name=layout.first(where: { $0.uid == deviceUID })?.name,!name.isEmpty { return name } }
        if let name=members.first(where: { $0.uid == deviceUID })?.name,!name.isEmpty { return name }
        // "AppleUSBAudioEngine:Acme:AudioBox10:00000001:1,2" → "AudioBox10".
        let parts=deviceUID.split(separator:":").map(String.init)
        if parts.count >= 3,parts[0].hasPrefix("Apple"),!parts[2].isEmpty { return parts[2] }
        return "The saved audio interface"
    }
}

extension LineInputTopology {
    /// Reads the topology of `device` from the HAL. Read-only: never changes devices.
    static func current(device:AudioDeviceID,inputChannels:Int)->LineInputTopology {
        var topology=LineInputTopology(uid:device == 0 ? "" : stringProperty(device,kAudioDevicePropertyDeviceUID),
            name:device == 0 ? "" : stringProperty(device,kAudioObjectPropertyName),inputChannels:inputChannels)
        var present:[String:AudioObjectID]=[:]
        for id in objectList(AudioObjectID(kAudioObjectSystemObject),kAudioHardwarePropertyDevices) {
            let uid=stringProperty(id,kAudioDevicePropertyDeviceUID)
            guard !uid.isEmpty else { continue }
            present[uid]=id
            topology.names[uid]=stringProperty(id,kAudioObjectPropertyName)
        }
        for (uid,id) in present where transportType(id) == kAudioDeviceTransportTypeAggregate {
            // A composition's "channels-in" is not always the member's real input count
            // (built-in speakers report 6); trust present members' own streams.
            topology.aggregates[uid]=configuredMembers(id).map { member in
                guard let device=present[member.uid] else { return member }
                return Member(uid:member.uid,name:member.name,inputChannels:AudioDeviceSelection.channels(device,input:true))
            }
        }
        if !topology.uid.isEmpty,transportType(device) == kAudioDeviceTransportTypeAggregate {
            topology.members=objectList(device,kAudioAggregateDevicePropertyActiveSubDeviceList).map { sub in
                let uid=stringProperty(sub,kAudioDevicePropertyDeviceUID)
                return Member(uid:uid,name:topology.names[uid] ?? stringProperty(sub,kAudioObjectPropertyName),
                              inputChannels:AudioDeviceSelection.channels(sub,input:true))
            }
        }
        return topology
    }

    static func configuredMembers(_ aggregate:AudioDeviceID)->[Member] {
        var address=AudioObjectPropertyAddress(mSelector:kAudioAggregateDevicePropertyComposition,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:Unmanaged<CFDictionary>?,size=UInt32(MemoryLayout<Unmanaged<CFDictionary>?>.size)
        guard AudioObjectGetPropertyData(aggregate,&address,0,nil,&size,&value) == noErr,let value,
              let composition=value.takeRetainedValue() as? [String:Any],
              let list=composition[kAudioAggregateDeviceSubDeviceListKey] as? [[String:Any]] else { return [] }
        return list.compactMap { item in
            guard let uid=item[kAudioSubDeviceUIDKey] as? String else { return nil }
            return Member(uid:uid,name:item[kAudioSubDeviceNameKey] as? String ?? "",
                          inputChannels:(item[kAudioSubDeviceInputChannelsKey] as? NSNumber)?.intValue ?? 0)
        }
    }

    private static func transportType(_ device:AudioObjectID)->UInt32 {
        var address=AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyTransportType,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:UInt32=0,size=UInt32(MemoryLayout<UInt32>.size)
        return AudioObjectGetPropertyData(device,&address,0,nil,&size,&value) == noErr ? value : 0
    }
    private static func stringProperty(_ object:AudioObjectID,_ selector:AudioObjectPropertySelector)->String {
        var address=AudioObjectPropertyAddress(mSelector:selector,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:Unmanaged<CFString>?,size=UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(object,&address,0,nil,&size,&value) == noErr,let value else { return "" }
        return value.takeRetainedValue() as String
    }
    static func objectList(_ object:AudioObjectID,_ selector:AudioObjectPropertySelector)->[AudioObjectID] {
        var address=AudioObjectPropertyAddress(mSelector:selector,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var size:UInt32=0
        guard AudioObjectGetPropertyDataSize(object,&address,0,nil,&size) == noErr,size > 0 else { return [] }
        var ids=[AudioObjectID](repeating:0,count:Int(size)/MemoryLayout<AudioObjectID>.size)
        guard AudioObjectGetPropertyData(object,&address,0,nil,&size,&ids) == noErr else { return [] }
        return Array(ids.prefix(Int(size)/MemoryLayout<AudioObjectID>.size))
    }
}

/// Reports a line-input problem once per distinct message, not on every project edit.
struct LineInputProblemReporter {
    private(set) var reported:String?
    /// Returns the message to emit, or nil when it was already reported. nil clears.
    mutating func update(_ message:String?)->String? {
        guard message != reported else { return nil }
        reported=message
        return message
    }
    static func summary(_ paused:[Int:String])->String? {
        guard !paused.isEmpty else { return nil }
        var seen=Set<String>(),reasons:[String]=[]
        for track in paused.keys.sorted() { let reason=paused[track]!; if seen.insert(reason).inserted { reasons.append(reason) } }
        let tracks=paused.keys.sorted().map { String($0+1) }.joined(separator:", ")
        return reasons.joined(separator:"; ")+" (track\(paused.count == 1 ? "" : "s") \(tracks)). Saved routing is kept and resumes when the interface returns."
    }
}

/// Calls back on main whenever audio devices appear or disappear (hotplug).
final class AudioDeviceListObserver {
    private var address=AudioObjectPropertyAddress(mSelector:kAudioHardwarePropertyDevices,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
    private let block:AudioObjectPropertyListenerBlock
    init(_ changed:@escaping ()->Void) {
        block={ _,_ in changed() }
        AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject),&address,DispatchQueue.main,block)
    }
    deinit { AudioObjectRemovePropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject),&address,DispatchQueue.main,block) }
}
