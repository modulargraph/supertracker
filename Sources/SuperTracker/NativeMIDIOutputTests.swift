#if SELFTESTS
import Foundation
import AVFoundation
import CoreMIDI

/// Private virtual destinations only, with an offline graph and no input subscriptions.
enum NativeMIDIOutputTests {
    @MainActor static func run(output:URL) async throws -> [String] {
        let folder=output.appendingPathComponent("midi-output",isDirectory:true)
        try FileManager.default.createDirectory(at:folder,withIntermediateDirectories:true)
        var client:MIDIClientRef=0,endpoints:[MIDIEndpointRef]=[],ids:[Int32]=[]
        let capture=MIDIOutputCapture()
        guard MIDIClientCreateWithBlock("SuperTracker private output test" as CFString,&client,nil)==noErr else { throw AudioSampleError.invalid("CoreMIDI service unavailable for private output tests.") }
        defer { for endpoint in endpoints { MIDIEndpointDispose(endpoint) };MIDIClientDispose(client) }
        for index in 0..<2 {
            var endpoint:MIDIEndpointRef=0
            guard MIDIDestinationCreateWithBlock(client,"Private synth \(index)" as CFString,&endpoint,{list,_ in capture.receive(list,destination:index)})==noErr else { throw AudioSampleError.invalid("Cannot create private MIDI destination.") }
            endpoints.append(endpoint)
            guard MIDIObjectSetIntegerProperty(endpoint,kMIDIPropertyPrivate,1)==noErr else { throw AudioSampleError.invalid("Cannot make test destination private.") }
            var id:Int32=0
            guard MIDIObjectGetIntegerProperty(endpoint,kMIDIPropertyUniqueID,&id)==noErr else { throw AudioSampleError.invalid("Cannot read test destination identity.") }
            ids.append(id)
        }
        let privateEndpoints=endpoints,graph=AVAudioEngine(),format=AVAudioFormat(standardFormatWithSampleRate:48_000,channels:2)!
        try graph.enableManualRenderingMode(.offline,format:format,maximumFrameCount:4096)
        let host=try AudioHost(audioGraph:graph,midiInputIDs:[],midiInputSourceProvider:{[]},midiOutputDestinationProvider:{privateEndpoints})
        let buffer=AVAudioPCMBuffer(pcmFormat:format,frameCapacity:4096)!
        var checks:[String]=[],completed=false,stage="setup"
        defer {
            host.shutdown()
            let report:[String:Any]=["passed":completed,"stage":stage,"checks":checks,"messages":capture.snapshot(),"physicalPlayback":false,"physicalMIDI":false,"privateEndpoints":true]
            if let data=try? JSONSerialization.data(withJSONObject:report,options:[.prettyPrinted,.sortedKeys]) {try? data.write(to:folder.appendingPathComponent("metrics.json"))}
        }
        func require(_ value:Bool,_ reason:String) throws {if !value {throw AudioSampleError.invalid("MIDI output regression: "+reason)}}
        func render(_ count:Int=2) throws {
            for _ in 0..<count { try require(try host.renderOfflineQuantum(frames:4096,to:buffer) == .success,"Offline render failed") }
        }
        func waitFor(_ predicate:()->Bool) async throws {
            for _ in 0..<200 {if predicate(){return};try await Task.sleep(nanoseconds:10_000_000)}
            throw AudioSampleError.invalid("Private MIDI output timed out during \(stage): \(capture.snapshot()); host: \(host.status())")
        }
        func has(_ destination:Int,_ status:Int,_ a:Int,_ b:Int)->Bool {capture.snapshot().contains([destination,status,a,b])}
        var project:[String:Any]=["version":1,"id":"private-midi-output","bpm":120,"linesPerBeat":4,"activePattern":0,"sequence":[0],"loopSong":false,"masterVolume":0,
            "tracks":[["routingID":"a","instrument":0,"volume":1],["routingID":"b","instrument":1,"volume":1]],
            "instruments":[["type":"midi","midiOutput":ids[0],"midiChannel":3],["type":"midi","midiOutput":ids[1],"midiChannel":12]],
            "patterns":[["id":0,"length":2,"steps":[[["note":60,"instrument":0,"velocity":100,"gate":0.25],NSNull()],[["note":67,"instrument":1,"velocity":90,"gate":0.25],NSNull()]],"automation":[]]],"audioPlugins":[],"audioClips":[],"delay":["mix":0],"reverb":["mix":0]]
        do {
            stage="sequenced notes"
            _ = try await host.handle("engine.project",payload:["project":project,"replace":true,"awaitPlugins":true])
            _ = try await host.handle("midi.configure",payload:["enabled":false])
            _ = try await host.handle("transport.play",payload:["mode":"pattern"])
            try render(4)
            try await waitFor {has(0,0x92,60,100) && has(1,0x9b,67,90) && has(0,0x82,60,0) && has(1,0x8b,67,0)}
            checks.append("Sequenced notes and releases reach two independent hardware destinations/channels with MIDI input disabled")
            _ = try await host.handle("transport.stop",payload:[:]);try render()
            stage="returned note lead"
            // Track 0 hears its synth back through a compensated line input: its notes lead like the clock.
            let clockURL=folder.appendingPathComponent("midi-clock.json")
            var clockPreferences=MIDIClockPreferences();clockPreferences.leadMilliseconds=25;try clockPreferences.save(clockURL)
            host.midiClockPreferencesURL=clockURL
            host.lineInputs=[0:try AudioLineInput(format:format,channels:[0,1])]
            host.refreshMIDIClock()
            try require(host.midiRoutes[0].leadTicks==0,"A line input without hardware-return compensation led notes")
            host.rack.hardwareReturnFrames=480;host.refreshMIDIClock()
            let leadTicks=UInt32(AVAudioTime.hostTime(forSeconds:0.025))
            try require(host.midiRoutes[0].leadTicks==leadTicks && host.midiRoutes[1].leadTicks==0,
                        "Returned note lead \(host.midiRoutes[0].leadTicks)/\(host.midiRoutes[1].leadTicks), expected \(leadTicks)/0")
            let ticks=host.hostTicksPerFrame,base:UInt64=1_000_000_000
            try require(AudioHost.externalNoteTimestamp(hostTime:base,frameOffset:48,ticksPerFrame:ticks,leadTicks:leadTicks)==base+UInt64(48*ticks)-UInt64(leadTicks)
                        && AudioHost.externalNoteTimestamp(hostTime:base,frameOffset:48,ticksPerFrame:ticks,leadTicks:0)==base+UInt64(48*ticks),"External note timestamp ignores the route lead")
            host.lineInputs=[:];host.rack.hardwareReturnFrames=0;host.refreshMIDIClock()
            try require(host.midiRoutes[0].leadTicks==0,"Removing the line input kept the note lead")
            checks.append("Notes to an External MIDI synth heard through a compensated line input lead by the clock lead; others do not")
            capture.clear();stage="live keyboard onset"
            _ = try await host.handle("note.on",payload:["track":1,"instrument":1,"note":72,"velocity":111]);try render()
            try await waitFor {has(1,0x9b,72,111)}
            stage="live keyboard release"
            _ = try await host.handle("note.off",payload:["track":1,"note":72]);try render()
            try await waitFor {has(1,0x9b,72,111) && has(1,0x8b,72,0)}
            checks.append("Live keyboard note and release use the assigned external instrument destination")
            stage="rapid key press and release"
            _ = try await host.handle("note.on",payload:["track":1,"instrument":1,"note":73,"velocity":80]);try render(1)
            _ = try await host.handle("note.off",payload:["track":1,"note":73]);try render(1)
            try await waitFor {has(1,0x9b,73,80) && has(1,0x8b,73,0)}
            checks.append("Rapid key press and release across adjacent render quanta preserve ordered output")
            stage="incoming CC and program"
            _ = try await host.handle("midi.configure",payload:["track":1,"instrument":1,"enabled":true])
            host.receiveMIDI(0xb0,7,99,source:12345);host.receiveMIDI(0xc0,42,0,source:12345)
            try await waitFor {has(1,0xbb,7,99) && has(1,0xcb,42,0)}
            checks.append("Incoming CC and program changes follow selected instrument output channel")
            stage="pedal panic"
            _ = try await host.handle("midi.expression",payload:["track":1,"kind":"sustain","value":1])
            _ = try await host.handle("engine.panic",payload:[:])
            try await waitFor {has(1,0xbb,64,0) && has(1,0xbb,123,0) && has(1,0xbb,120,0)}
            checks.append("Panic releases hardware sustain before all-notes-off and all-sound-off")
            capture.clear();stage="route change"
            var instruments=project["instruments"] as! [[String:Any]];instruments[1]["midiOutput"]=ids[0];instruments[1]["midiChannel"]=5;project["instruments"]=instruments
            _ = try await host.handle("engine.project",payload:["project":project,"awaitPlugins":true])
            try await waitFor {has(1,0xbb,64,0) && has(1,0xbb,123,0)}
            _ = try await host.handle("note.on",payload:["track":1,"instrument":1,"note":74,"velocity":88]);try render()
            _ = try await host.handle("note.off",payload:["track":1,"note":74]);try render()
            try await waitFor {has(0,0x94,74,88) && has(0,0x84,74,0)}
            let saved=try await host.handle("engine.snapshot",payload:[:]) as? [String:Any] ?? [:]
            let savedInstruments=saved["instruments"] as? [[String:Any]] ?? []
            try require(savedInstruments.count==2 && (savedInstruments[1]["midiOutput"] as? NSNumber)?.int32Value==ids[0] && (savedInstruments[1]["midiChannel"] as? Int)==5,"Persistent MIDI assignment changed")
            checks.append("Changing destination/channel releases the old route and persists the new hardware assignment")
            completed=true
            await host.shutdownAndWait()
            return checks
        } catch {await host.shutdownAndWait();throw error}
    }
}
private final class MIDIOutputCapture:@unchecked Sendable {
    private let lock=NSLock()
    private var messages:[[Int]]=[]
    func receive(_ list:UnsafePointer<MIDIPacketList>,destination:Int) {
        var packet=UnsafeRawPointer(list).advanced(by:MemoryLayout<MIDIPacketList>.offset(of:\.packet)!).assumingMemoryBound(to:MIDIPacket.self)
        lock.lock();defer{lock.unlock()}
        for _ in 0..<list.pointee.numPackets {
            // CoreMIDI may coalesce adjacent sends (for example CC followed by
            // a two-byte program change) into a single packet. Decode every
            // complete short message rather than treating packet == message.
            let bytes=UnsafeRawPointer(packet).advanced(by:MemoryLayout<MIDIPacket>.offset(of:\.data)!).assumingMemoryBound(to:UInt8.self)
            var offset=0
            while offset<Int(packet.pointee.length) {
                let status=bytes[offset]
                guard status>=0x80 else {offset += 1;continue}
                let size=status>=0xf8 ? 1 : (status & 0xe0 == 0xc0 ? 2 : 3)
                guard offset+size<=Int(packet.pointee.length) else {break}
                messages.append([destination,Int(status),size>1 ? Int(bytes[offset+1]):0,size>2 ? Int(bytes[offset+2]):0])
                offset += size
            }
            packet=UnsafePointer(MIDIPacketNext(packet))
        }
    }
    func snapshot()->[[Int]] {lock.lock();defer{lock.unlock()};return messages}
    func clear(){lock.lock();messages.removeAll();lock.unlock()}
}
#endif
