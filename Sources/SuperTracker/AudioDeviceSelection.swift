import Foundation
import AudioToolbox
import CoreAudio
import AVFoundation
import ObjectiveC

/// AVAudioEngine may expose the SAME duplex HAL unit for both I/O nodes. Its
/// CurrentDevice is often a private aggregate, not either physical device.
/// Choose the complete pair and never install an input-only device as that unit's output.
final class AudioDeviceSelection {
    struct Pair:Equatable { let input:AudioDeviceID;let output:AudioDeviceID }
    enum Failure:LocalizedError {
        case invalid(String)
        var errorDescription:String? { if case let .invalid(message)=self { return message };return nil }
    }
    private weak var engine:AVAudioEngine?
    private static var ownershipKey:UInt8=0
    private final class OwnedAggregate {
        let id:AudioDeviceID
        init(_ id:AudioDeviceID) { self.id=id }
        deinit { AudioHardwareDestroyAggregateDevice(id) }
    }
    init(engine:AVAudioEngine) { self.engine=engine }
    private func own(_ device:AudioDeviceID?) {
        guard let engine else { return }
        // Associated storage is released after AVAudioEngine's dealloc has
        // disposed its I/O nodes, including when an external caller owns it.
        objc_setAssociatedObject(engine,&Self.ownershipKey,device.map(OwnedAggregate.init),.OBJC_ASSOCIATION_RETAIN_NONATOMIC)
    }
    static func currentDevice(_ unit:AudioUnit)->AudioDeviceID {
        var value:AudioDeviceID=0,size=UInt32(MemoryLayout<AudioDeviceID>.size)
        _ = AudioUnitGetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&value,&size)
        return value
    }
    private static func contains(_ pair:Pair,device:AudioDeviceID)->Bool {
        if pair.input == pair.output || pair.input == 0 { return device == pair.output }
        var property=AudioObjectPropertyAddress(mSelector:kAudioAggregateDevicePropertyFullSubDeviceList,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:Unmanaged<CFArray>?,size=UInt32(MemoryLayout<Unmanaged<CFArray>?>.size)
        guard AudioObjectGetPropertyData(device,&property,0,nil,&size,&value) == noErr,let value,
              let members=value.takeRetainedValue() as? [String] else { return false } // returned retained
        return members.contains(uid(pair.input)) && members.contains(uid(pair.output))
    }

    static func defaultDevice(input:Bool)->AudioDeviceID {
        var property=AudioObjectPropertyAddress(mSelector:input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice,
            mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:AudioDeviceID=0,size=UInt32(MemoryLayout<AudioDeviceID>.size)
        _ = AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject),&property,0,nil,&size,&value)
        return value
    }
    /// Seconds from sound at the device's input to the samples reaching the engine: device and safety-offset
    /// latency of the input side, plus the IO buffer twice (captured in one cycle, rendered in the next).
    static func inputPathSeconds(_ device:AudioDeviceID)->Double {
        func value(_ selector:AudioObjectPropertySelector,_ scope:AudioObjectPropertyScope)->UInt32 {
            var property=AudioObjectPropertyAddress(mSelector:selector,mScope:scope,mElement:kAudioObjectPropertyElementMain)
            var result:UInt32=0,size=UInt32(MemoryLayout<UInt32>.size)
            return AudioObjectGetPropertyData(device,&property,0,nil,&size,&result) == noErr ? result : 0
        }
        var rateProperty=AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyNominalSampleRate,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var rate:Float64=0,size=UInt32(MemoryLayout<Float64>.size)
        guard device != 0,AudioObjectGetPropertyData(device,&rateProperty,0,nil,&size,&rate) == noErr,rate > 0 else { return 0 }
        let frames=value(kAudioDevicePropertyLatency,kAudioObjectPropertyScopeInput)+value(kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeInput)
            + 2*value(kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal)
        return min(0.5,Double(frames)/rate)
    }
    /// Seconds a hardware synth played from a render arrives late in the mix through a line
    /// input, compared to that render's own output: one IO buffer (the render runs a buffer
    /// ahead of output, the return is mixed a buffer later) plus the input latency and both
    /// safety offsets. The round trip is the input path plus the output path; the output
    /// path is shared with the rest of the mix, so only this difference needs compensating.
    static func hardwareReturnSeconds(_ device:AudioDeviceID)->Double {
        func value(_ selector:AudioObjectPropertySelector,_ scope:AudioObjectPropertyScope)->UInt32 {
            var property=AudioObjectPropertyAddress(mSelector:selector,mScope:scope,mElement:kAudioObjectPropertyElementMain)
            var result:UInt32=0,size=UInt32(MemoryLayout<UInt32>.size)
            return AudioObjectGetPropertyData(device,&property,0,nil,&size,&result) == noErr ? result : 0
        }
        var rateProperty=AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyNominalSampleRate,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var rate:Float64=0,size=UInt32(MemoryLayout<Float64>.size)
        guard device != 0,AudioObjectGetPropertyData(device,&rateProperty,0,nil,&size,&rate) == noErr,rate > 0 else { return 0 }
        let frames=value(kAudioDevicePropertyLatency,kAudioObjectPropertyScopeInput)+value(kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeInput)
            + value(kAudioDevicePropertySafetyOffset,kAudioObjectPropertyScopeOutput)+value(kAudioDevicePropertyBufferFrameSize,kAudioObjectPropertyScopeGlobal)
        return min(0.25,Double(frames)/rate)
    }
    /// Whether `aggregate` is an aggregate device with `device` among its sub-devices.
    static func isAggregate(_ aggregate:AudioDeviceID,containing device:AudioDeviceID)->Bool {
        var property=AudioObjectPropertyAddress(mSelector:kAudioAggregateDevicePropertyFullSubDeviceList,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:Unmanaged<CFArray>?,size=UInt32(MemoryLayout<Unmanaged<CFArray>?>.size)
        guard AudioObjectGetPropertyData(aggregate,&property,0,nil,&size,&value) == noErr,let value,
              let members=value.takeRetainedValue() as? [String] else { return false }
        let wanted=uid(device)
        return !wanted.isEmpty && members.contains(wanted)
    }
    static func uid(_ device:AudioDeviceID)->String {
        var property=AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyDeviceUID,mScope:kAudioObjectPropertyScopeGlobal,mElement:kAudioObjectPropertyElementMain)
        var value:Unmanaged<CFString>?,size=UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(device,&property,0,nil,&size,&value) == noErr,let value else { return "" }
        return value.takeRetainedValue() as String // Core Audio hands the string over retained
    }
    static func channels(_ device:AudioDeviceID,input:Bool)->Int {
        var property=AudioObjectPropertyAddress(mSelector:kAudioDevicePropertyStreamConfiguration,mScope:input ? kAudioObjectPropertyScopeInput:kAudioObjectPropertyScopeOutput,mElement:kAudioObjectPropertyElementMain)
        var bytes:UInt32=0
        guard AudioObjectGetPropertyDataSize(device,&property,0,nil,&bytes) == noErr,bytes >= MemoryLayout<AudioBufferList>.size else { return 0 }
        let data=UnsafeMutableRawPointer.allocate(byteCount:Int(bytes),alignment:MemoryLayout<AudioBufferList>.alignment)
        defer { data.deallocate() }
        guard AudioObjectGetPropertyData(device,&property,0,nil,&bytes,data) == noErr else { return 0 }
        return UnsafeMutableAudioBufferListPointer(data.assumingMemoryBound(to:AudioBufferList.self)).reduce(0) { $0+Int($1.mNumberChannels) }
    }

    /// Caller stops/drains its engine first. State changes commit only after HAL
    /// accepts the pair. Re-selecting the current microphone is intentionally a no-op.
    func select(_ pair:Pair,current:Pair,inputUnit:AudioUnit,outputUnit:AudioUnit) throws {
        guard (pair.input == 0 || Self.channels(pair.input,input:true)>0),Self.channels(pair.output,input:false)>0 else {
            throw Failure.invalid("Choose an available input device and an available output device.")
        }
        if inputUnit == outputUnit {
            if pair == current && Self.contains(pair,device:Self.currentDevice(outputUnit)) { return }
        } else if Self.currentDevice(inputUnit) == pair.input && Self.currentDevice(outputUnit) == pair.output { return }
        if inputUnit != outputUnit {
            if pair.output != current.output { try Self.setDevice(pair.output,on:outputUnit) }
            do { if pair.input != 0 && pair.input != current.input { try Self.setDevice(pair.input,on:inputUnit) } }
            catch { if pair.output != current.output { try? Self.setDevice(current.output,on:outputUnit) };throw error }
            return
        }
        if pair.input == pair.output || pair.input == 0 {
            try Self.setDevice(pair.output,on:outputUnit)
            own(nil)
            return
        }
        let inputUID=Self.uid(pair.input),outputUID=Self.uid(pair.output)
        guard !inputUID.isEmpty,!outputUID.isEmpty else { throw Failure.invalid("The selected audio device disconnected.") }
        let description=Self.pairDescription(inputUID:inputUID,inputChannels:Self.channels(pair.input,input:true),
            outputUID:outputUID,outputChannels:Self.channels(pair.output,input:false))
        var candidate:AudioDeviceID=0
        let result=AudioHardwareCreateAggregateDevice(description as CFDictionary,&candidate)
        guard result == noErr,candidate != 0 else { throw Failure.invalid("Unable to combine the selected input and output devices (Core Audio \(result)). Use an Aggregate Device in Audio MIDI Setup.") }
        // Some drivers ignore directional subdevice limits. Never let an input
        // interface's output streams displace the user's chosen speakers.
        guard Self.channels(candidate,input:true) == Self.channels(pair.input,input:true),
              Self.channels(candidate,input:false) == Self.channels(pair.output,input:false) else {
            AudioHardwareDestroyAggregateDevice(candidate)
            throw Failure.invalid("These devices cannot form an independent input/output pair. Create an Aggregate Device in Audio MIDI Setup and select it for both input and output.")
        }
        do { try Self.setDevice(candidate,on:outputUnit) }
        catch { AudioHardwareDestroyAggregateDevice(candidate);throw error }
        own(candidate)
    }
    /// Private input/output pair. The input device is the FIRST member, so its channels
    /// keep their own indices in the engine's input stream (line-input routes rely on
    /// this). The output device is the clock source; only the input is drift-corrected.
    /// Same-device pairs never reach here (select() uses the device directly), and the
    /// aggregate is private (invisible to other apps, gone with this process) and
    /// destroyed when replaced or when the engine is released.
    static func pairDescription(inputUID:String,inputChannels:Int,outputUID:String,outputChannels:Int)->[String:Any] {
        [kAudioAggregateDeviceUIDKey:"studio.supertracker.io."+UUID().uuidString,
         kAudioAggregateDeviceNameKey:"SuperTracker Input and Output",
         kAudioAggregateDeviceIsPrivateKey:true,kAudioAggregateDeviceIsStackedKey:false,
         kAudioAggregateDeviceMainSubDeviceKey:outputUID,
         kAudioAggregateDeviceSubDeviceListKey:[
            [kAudioSubDeviceUIDKey:inputUID,kAudioSubDeviceInputChannelsKey:inputChannels,kAudioSubDeviceOutputChannelsKey:0,kAudioSubDeviceDriftCompensationKey:1],
            [kAudioSubDeviceUIDKey:outputUID,kAudioSubDeviceInputChannelsKey:0,kAudioSubDeviceOutputChannelsKey:outputChannels,kAudioSubDeviceDriftCompensationKey:0]
         ]]
    }
    private static func setDevice(_ device:AudioDeviceID,on unit:AudioUnit) throws {
        var device=device
        let result=AudioUnitSetProperty(unit,kAudioOutputUnitProperty_CurrentDevice,kAudioUnitScope_Global,0,&device,UInt32(MemoryLayout<AudioDeviceID>.size))
        guard result == noErr else { throw Failure.invalid("Unable to select the audio input/output pair (Core Audio \(result)).") }
    }
}
