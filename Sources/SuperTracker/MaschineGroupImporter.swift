import Foundation
import AVFoundation
import TrackerEngine

/// Imports proven native object layouts into independent editable kit objects.
/// Original archive bytes and numeric values remain attached for loss reporting
/// and future migrations; they are never passed to an unrelated plug-in.
enum MaschineGroupImporter {
    private static let internalEffectTypes: Set<String> = [
        "BeatDelay", "CabinetEmulation", "Chorus", "Compressor", "EQ", "FM", "Filter", "Flanger",
        "FreqShifter", "Gate", "GrainDelay", "GrainStretch", "Iceverb", "Limiter", "LoFi", "Maximizer",
        "Metaverb", "MullhollandDrive", "Phaser", "PlateReverb", "Reflex", "Resochord", "Reverb",
        "Saturator", "TransientDesigner"
    ]
    /// Insert slots per SuperTracker channel; the audio host accepts slots 0..<16.
    static let insertSlotLimit = MixerChannel.insertSlotCount
    static func metadata(at url: URL, root: URL? = nil) throws -> [String:Any] {
        try KitLibrary.metadata(at:url,root:root)
    }
    static func read(at url: URL, root: URL? = nil) throws -> [String:Any] {
        let size = try url.resourceValues(forKeys:[.fileSizeKey]).fileSize ?? 0
        guard size > 0,size <= 32*1024*1024 else {throw MaschineGroupArchive.Invalid(message:"file exceeds the supported 32 MB limit.")}
        let source = try Data(contentsOf:url)
        let archive = try MaschineGroupArchive(data:source)
        let kitID = UUID().uuidString.lowercased(), busID = UUID().uuidString.lowercased()
        var warnings:[String] = [], plugins:[[String:Any]] = [], sounds:[[String:Any]] = []
        var moduleReports:[[String:Any]] = [], frameCache:[String:Int64] = [:]
        let library = expansionRoot(url,root:root)
        func warn(_ text:String) {if !warnings.contains(text) {warnings.append(text)}}
        func report(_ module:MaschineGroupArchive.Module,owner:String,slot:Int) -> [String:Any] {
            var r:[String:Any] = ["owner":owner,"slot":slot,"type":module.type,"version":module.version,"objectID":module.objectID,"enabled":module.enabled]
            if let parameters = try? archive.parameters(for:module) {r["parameters"] = parameters}
            return r
        }
        // A Maschine Group can hold more inserts than a SuperTracker channel (sixteen
        // slots, 0...15). Loadable inserts are numbered contiguously; when a chain is
        // longer, bypassed inserts are left out first, then the trailing ones. Every
        // omitted module keeps its original state in the import report.
        func effects(_ modules:[MaschineGroupArchive.Module],owner:String,label:String) {
            var candidates:[(report:Int,descriptor:[String:Any],enabled:Bool,type:String,original:Int)] = []
            for (slot,module) in modules.enumerated() {
                var r = report(module,owner:owner,slot:slot)
                r["kind"]="effect"
                let raw = r["parameters"] as? [Double] ?? []
                if module.type == "PluginHost" {
                    do {
                        let state=try MaschineGroupPluginState.read(archive:archive,module:module)
                        r["pluginState"]=state.dictionary
                        guard state.isInstrument == false else {throw MaschineGroupArchive.Invalid(message:"an insert slot contains an instrument plug-in.")}
                        let descriptor=try state.descriptor(owner:owner,slot:0,enabled:module.enabled)
                        for warning in state.restorationWarnings {warn(warning)}
                        r["status"]="external-state-ready"
                        candidates.append((moduleReports.count,descriptor,module.enabled,module.type,slot))
                    } catch {r["status"]="unavailable";warn("\(label): \(error.localizedDescription)")}
                    moduleReports.append(r);continue
                }
                let mapping = MaschineGroupCoreMapping.map(type:module.type,version:module.version,parameters:raw)
                    ?? MaschineGroupSpatialMapping.map(type:module.type,version:module.version,parameters:raw)
                    ?? MaschineGroupAdditionalMapping.map(type:module.type,version:module.version,parameters:raw)
                    ?? MaschineGroupDistortionMapping.map(type:module.type,version:module.version,parameters:raw)
                guard let component = mapping?["id"] as? String,
                      let info = st_builtin_effect_find(String(component.dropFirst(5))) else {
                    r["status"] = "unavailable";moduleReports.append(r)
                    warn("\(module.type) on \(label) is unavailable; its original state is retained in the import report.");continue
                }
                let id = String(component.dropFirst(5))
                var values:[String:Double] = [:]
                for row in mapping?["parameters"] as? [[String:Any]] ?? [] {
                    if let address=row["address"] as? Int,let value=row["value"] as? Double,address>=0,address<Int(info.pointee.parameterCount),
                       let p=st_builtin_effect_parameter_info(id,UInt32(address)) {values[String(cString:p.pointee.id)] = value}
                }
                for warning in mapping?["warnings"] as? [String] ?? [] {warn(warning)}
                let state:[String:Any] = ["format":"SuperTracker.BuiltinFX","version":1,"id":id,"parameters":values]
                if let data=try? PropertyListSerialization.data(fromPropertyList:state,format:.binary,options:0) {
                    r["status"] = "approximation-mapped-controls"
                    candidates.append((moduleReports.count,["instanceID":UUID().uuidString.lowercased(),"owner":owner,"kind":"effect","slot":0,"component":component,"id":component,"name":String(cString:info.pointee.name),"format":"Builtin","state":data.base64EncodedString(),"bypass":!module.enabled],module.enabled,module.type,slot))
                }
                moduleReports.append(r)
            }
            var omitted = Set<Int>()
            if candidates.count > insertSlotLimit {
                let excess = candidates.count - insertSlotLimit
                let bypassed = candidates.indices.filter { !candidates[$0].enabled }.reversed()
                for index in bypassed.prefix(excess) { omitted.insert(index) }
                for index in candidates.indices.reversed() where omitted.count < excess { omitted.insert(index) }
            }
            var next = 0
            for (index,candidate) in candidates.enumerated() {
                if omitted.contains(index) {
                    moduleReports[candidate.report]["status"] = "omitted-insert-limit"
                    warn("\(label): \(candidate.type) (original insert \(candidate.original+1)) was not loaded because a SuperTracker channel holds \(insertSlotLimit) inserts; its original state is retained in the import report.")
                    continue
                }
                var descriptor = candidate.descriptor
                descriptor["slot"] = next
                moduleReports[candidate.report]["insertSlot"] = next
                plugins.append(descriptor); next += 1
            }
        }
        if archive.modules.isEmpty {
            warn("\(archive.groupName.isEmpty ? "This Group" : archive.groupName) has no audio modules: its Sounds only address external MIDI or audio inputs, so they import as empty pads.")
        }
        for sound in archive.sounds {
            let soundID = "pad-\(sound.index+1)", owner = "sound:\(kitID):\(soundID)"
            let modules = archive.modules(for:sound)
            // Maschine shows an unnamed Sound by its pad number.
            let soundName = displayName(sound)
            var instrument:[String:Any] = ["type":"sample","name":soundName,"volume":1.0,"mode":"oneshot","rootNote":60,"attack":0.0,"decay":0.0,"sustain":1.0,"release":0.01]
            // A Sound may contain only inserts for external/internal audio input.
            // Slot position alone does not establish that a module is a generator.
            let firstIsEffect = modules.first.map { module in
                internalEffectTypes.contains(module.type) || (module.type == "PluginHost" && (try? MaschineGroupPluginState.read(archive:archive,module:module).isInstrument) == false)
            } ?? false
            if firstIsEffect {
                effects(modules,owner:owner,label:soundName)
                warn("\(soundName): its effects-only Sound retains the insert chain, but original audio input routing is not converted.")
            } else if let sourceModule=modules.first {
                var r = report(sourceModule,owner:owner,slot:0)
                r["kind"]="instrument"
                do {
                    if sourceModule.type == "Sampler" {
                        let zones=try archive.zones(for:sourceModule)
                        if let controls=try? archive.samplerParameters(for:sourceModule,zones:zones) {
                            r["samplerParameters"]=controls
                            let mapping=MaschineGroupSamplerMapping.map(parameters:controls)
                            for (key,value) in mapping["instrument"] as? [String:Any] ?? [:] {instrument[key]=value}
                            for warning in mapping["warnings"] as? [String] ?? [] {warn(warning)}
                            r["samplerPhysical"]=mapping["physical"]
                        } else {warn("\(soundName): sampler controls were retained without conversion.")}
                        let mapped=mapZones(zones,library:library,frameCache:&frameCache,warn:warn)
                        if mapped.isEmpty {throw MaschineGroupArchive.Invalid(message:"no sample zone could be imported.")}
                        instrument["zones"] = mapped
                        instrument["samplePath"] = mapped.first?["path"]
                        r["status"] = "sample-zones-decoded"; r["zones"] = zones.map { ["objectID":$0.objectID,"path":$0.path,"origin":$0.origin,"library":$0.library,"values":$0.values] as [String:Any] }
                    } else if sourceModule.type == "AudioModule" {
                        let reference=try archive.audioModuleSample(for:sourceModule)
                        r["audioSample"]=["objectID":reference.objectID,"path":reference.path,"origin":reference.origin,"library":reference.library]
                        let zone=MaschineGroupArchive.Zone(objectID:reference.objectID,path:reference.path,origin:reference.origin,library:reference.library,
                            values:["keyLow":0,"keyHigh":127,"velocityLow":0,"velocityHigh":127,"root":60,"pan":0,"tune":0,"gain":MaschineGroupImporter.unityZoneGain,"loopEnabled":0,"startFrame":0,"endFrame":-1,"loopStartFrame":0,"loopEndFrame":-1])
                        let mapped=mapZones([zone],library:library,frameCache:&frameCache,warn:warn)
                        if mapped.isEmpty {throw MaschineGroupArchive.Invalid(message:"the Audio module's sample could not be imported.")}
                        instrument["zones"]=mapped
                        instrument["samplePath"]=mapped.first?["path"]
                        warn("\(soundName): the Audio module's loop is imported as a plain sample; tempo sync, time-stretching and its play modes are not reproduced.")
                        r["status"]="audio-sample-decoded"
                    } else if sourceModule.type == "PluginHost" {
                        let state=try MaschineGroupPluginState.read(archive:archive,module:sourceModule)
                        r["pluginState"]=state.dictionary
                        guard state.isInstrument == true else {throw MaschineGroupArchive.Invalid(message:"a Sound source contains an effect plug-in.")}
                        let descriptor=try state.descriptor(owner:owner,enabled:sourceModule.enabled)
                        plugins.append(descriptor)
                        instrument["type"]="au"
                        instrument["plugin"]=["name":state.pluginName,"id":descriptor["component"]!,"format":"VST2"]
                        for warning in state.restorationWarnings {warn(warning)}
                        r["status"]="external-state-ready"
                    } else if ["Kick","Snare","Hihat","Tom","Perc","Cymbal"].contains(sourceModule.type) {
                        if let raw=r["parameters"] as? [Double],let mapping=MaschineGroupDrumMapping.map(type:sourceModule.type,version:sourceModule.version,parameters:raw) {
                            // The drum model becomes a one-shot sample of its rendering (a built-in sample reference).
                            instrument["type"] = "synth";instrument["drumModel"] = mapping["drumModel"]
                            instrument["drumParameters"] = Dictionary(uniqueKeysWithValues:zip(ProjectMigration.drumParameterKeys,mapping["drumParameters"] as? [Double] ?? []))
                            instrument = ProjectMigration.migratedInstrument(instrument)
                            for warning in mapping["warnings"] as? [String] ?? [] {warn(warning)}
                            r["status"] = "approximation-mapped-controls"
                        } else {instrument["unavailable"]=true;r["status"]="unavailable";warn("\(soundName): unrecognized DrumSynth control layout.")}
                    } else {
                        instrument["unavailable"] = true;r["status"] = "unavailable"
                        warn("\(soundName): \(sourceModule.type) instrument state is retained but cannot be restored yet.")
                    }
                } catch {instrument["unavailable"] = true;r["status"] = "unavailable";warn("\(soundName): \(error.localizedDescription)")}
                if !sourceModule.enabled {instrument["volume"]=0.0}
                moduleReports.append(r)
                effects(Array(modules.dropFirst()),owner:owner,label:soundName)
            }
            var channel:[String:Any] = ["volume":1.0,"pan":0.0,"mute":false,"solo":false,"delaySend":0.0,"reverbSend":0.0]
            var originalChannel:[String:Double] = [:]
            var choke = 0
            do {
                originalChannel = try archive.channel(for:sound)
                let gain=MaschineGroupSamplerMapping.levelAmplitude(originalChannel["level"]!) ?? 1
                channel["volume"] = min(2,gain);channel["pan"] = originalChannel["pan"]!
                channel["mute"] = originalChannel["mute"]! != 0 || originalChannel["muteAudio"]! != 0
                choke=Int(originalChannel["chokeGroup"]!);instrument["chokeGroup"]=choke
                if gain>2 {warn("Sound level exceeded the mixer maximum and was limited: \(soundName)")}
                if (originalChannel["chokeMode"] ?? 0) != 0 {warn("Nonstandard choke behavior on \(soundName) uses SuperTracker's exclusive choke mode.")}
            } catch {warn("\(soundName): mixer controls were not decoded: \(error.localizedDescription)")}
            sounds.append(["id":soundID,"name":soundName,"instrument":instrument,"channel":channel,"chokeGroup":choke,"maschineChannel":originalChannel,"maschineObjectID":sound.objectID])
        }
        effects(archive.groupModules,owner:"bus:"+busID,label:archive.groupName)
        warn("Pattern notes, modulation, MIDI routing and auxiliary sends are retained in the original archive but are not converted yet.")
        warn("Group mixer level and unconverted sampler control modes remain in the original archive; sample zones retain their exact key, velocity and frame ranges.")
        let report:[String:Any] = ["format":"MaschineGroupImport","version":1,"archiveVersion":archive.archiveVersion,"groupVersion":archive.groupVersion,"soundVersion":archive.soundVersion,"sourcePath":url.path,"originalArchive":source.base64EncodedString(),"modules":moduleReports,"warnings":warnings]
        let kit:[String:Any] = ["id":kitID,"name":archive.groupName,"busID":busID,"sounds":sounds,"maschineImport":report]
        let bus:[String:Any] = ["id":busID,"name":archive.groupName,"type":"group","volume":1.0,"pan":0.0,"mute":false,"solo":false]
        let project:[String:Any] = ["kits":[kit],"audioPlugins":plugins,"routing":["version":1,"mixModel":"standard","buses":[bus],"outputs":["bus:"+busID:"master"],"sends":[],"sidechains":[]]]
        var meta=try metadata(at:url,root:root);meta["name"]=archive.groupName;meta["soundCount"]=16;meta["metadataSource"]="native Maschine object archive"
        return ["metadata":meta,"project":project,"path":url.path,"warnings":warnings,"report":report]
    }
    /// Display name of a Sound; Maschine shows an unnamed Sound by its pad number.
    static func displayName(_ sound:MaschineGroupArchive.Sound) -> String {
        sound.name.trimmingCharacters(in:.whitespaces).isEmpty ? "Sound \(sound.index+1)" : sound.name
    }
    /// Gain value of a zone at unity level (0 dB) in the native 0...1 level scale.
    static let unityZoneGain = 0.75
    /// The installed expansion folder: named "… Library", or (for expansions installed under
    /// another name, e.g. "Infinite Escape") the folder holding the product's .nicnt file.
    static func expansionRoot(_ url:URL,root:URL?) -> URL {
        var candidate=url.deletingLastPathComponent()
        for _ in 0..<8 {
            if candidate.lastPathComponent.hasSuffix(" Library") {return candidate}
            if let names=try? FileManager.default.contentsOfDirectory(atPath:candidate.path),
               names.contains(where:{$0.lowercased().hasSuffix(".nicnt")}) {return candidate}
            let parent=candidate.deletingLastPathComponent();if parent==candidate {break};candidate=parent
        }
        return root ?? url.deletingLastPathComponent().deletingLastPathComponent()
    }
    /// Sampler zones in SuperTracker form. A zone whose path leaves the expansion library is skipped (and warned about),
    /// so one bad zone never makes the whole Sound unavailable; a missing sample keeps its zone, marked unavailable.
    static func mapZones(_ zones:[MaschineGroupArchive.Zone],library:URL,frameCache:inout [String:Int64],warn:(String)->Void) -> [[String:Any]] {
        var mapped:[[String:Any]] = []
        for var zone in zones {
            var path:URL
            do {path=try resolve(zone.path,library:library)} catch {warn("Sample zone skipped: \(zone.path): \(error.localizedDescription)");continue}
            func frameCount(_ url:URL) -> Int64 {
                if let cached=frameCache[url.path] {return cached}
                let frames=(try? AVAudioFile(forReading:url).length) ?? 0;frameCache[url.path]=frames;return frames
            }
            var frames=frameCount(path)
            // REX2 loops (.rx2) cannot be decoded here. Expansions ship the same loop rendered
            // as a WAV beside it. That rendering is only used when this Sound's slices tile it
            // exactly (the last slice ends on its final frame); otherwise the timelines differ.
            if frames == 0, path.pathExtension.lowercased() == "rx2" {
                let wav=path.deletingPathExtension().appendingPathExtension("wav")
                let wavFrames=FileManager.default.fileExists(atPath:wav.path) ? frameCount(wav) : 0
                let ends=zones.filter{$0.path==zone.path}.map{max($0.values["endFrame"] ?? 0,$0.values["loopEndFrame"] ?? 0)}
                if wavFrames > 0, let last=ends.max(), abs(last + 1 - Double(wavFrames)) <= 1 {
                    path=wav;frames=wavFrames
                    warn("REX2 loop \(zone.path) plays from its bundled WAV rendering with the original slice ranges.")
                } else {warn("REX2 loop \(zone.path) cannot be decoded and its slices do not match a bundled WAV rendering.")}
            }
            // An Audio module reference covers the whole file (end markers of -1).
            if frames > 0 {
                for key in ["endFrame","loopEndFrame"] where (zone.values[key] ?? 0) < 0 {
                    var values=zone.values;values[key]=Double(frames-1)
                    zone=MaschineGroupArchive.Zone(objectID:zone.objectID,path:zone.path,origin:zone.origin,library:zone.library,values:values)
                }
            }
            var z:[String:Any] = ["path":path.path,"keyLow":Int(zone.values["keyLow"]!),"keyHigh":Int(zone.values["keyHigh"]!),"velocityLow":Int(zone.values["velocityLow"]!),"velocityHigh":Int(zone.values["velocityHigh"]!),"rootNote":Int(zone.values["root"]!),"pan":zone.values["pan"]!,"tune":zone.values["tune"]!,"volume":min(2,MaschineGroupSamplerMapping.levelAmplitude(zone.values["gain"]!) ?? 1),"loopEnabled":zone.values["loopEnabled"]! != 0,"maschineSource":zone.values]
            if (MaschineGroupSamplerMapping.levelAmplitude(zone.values["gain"]!) ?? 0) > 2 {warn("A sample-zone level exceeded the supported gain and was limited: \(zone.path)")}
            if frames > 0 {
                // Native ranges include their final frame; ST uses an exclusive endpoint.
                for (key,sourceKey,increment) in [("start","startFrame",0.0),("end","endFrame",1.0),("loopStart","loopStartFrame",0.0),("loopEnd","loopEndFrame",1.0)] {z[key]=min(1,max(0,(zone.values[sourceKey]!+increment)/Double(frames)))}
            } else {warn("Missing or unreadable sample: \(zone.path)");z["unavailable"] = true}
            mapped.append(z)
        }
        return mapped
    }

    static func resolve(_ relative:String,library:URL) throws -> URL {
        var normalized=relative.replacingOccurrences(of:"\\",with:"/")
        // Some expansions write library-relative references with one or two leading
        // slashes ("//Samples/Drums/Kick.wav", "/Samples/…"; the original location is a
        // separate field). Only the expansion's own Samples folder is read this way; any
        // other absolute path is still rejected.
        for prefix in ["//Samples/","/Samples/"] where normalized.hasPrefix(prefix) {
            normalized.removeFirst(prefix.count - "Samples/".count); break
        }
        guard !normalized.hasPrefix("/"),!normalized.contains(":"),!normalized.split(separator:"/",omittingEmptySubsequences:false).contains("..") else {throw MaschineGroupArchive.Invalid(message:"sample path leaves its expansion library.")}
        let root=library.resolvingSymlinksInPath().standardizedFileURL
        var url=root, pending=normalized.split(separator:"/").map(String.init), hops=0
        while !pending.isEmpty {
            let part=pending.removeFirst()
            url=url.appendingPathComponent(part).standardizedFileURL
            guard url.path.hasPrefix(root.path+"/") || url.path==root.path else {throw MaschineGroupArchive.Invalid(message:"sample path leaves its expansion library.")}
            if let target=try? FileManager.default.destinationOfSymbolicLink(atPath:url.path) {
                hops += 1
                guard hops <= 32 else {throw MaschineGroupArchive.Invalid(message:"sample path contains a symbolic-link cycle.")}
                let destination=(target.hasPrefix("/") ? URL(fileURLWithPath:target) : url.deletingLastPathComponent().appendingPathComponent(target)).standardizedFileURL
                guard destination.path.hasPrefix(root.path+"/") || destination.path==root.path else {throw MaschineGroupArchive.Invalid(message:"sample path leaves its expansion library.")}
                pending=String(destination.path.dropFirst(root.path.count)).split(separator:"/").map(String.init)+pending
                url=root
            }
        }
        guard url.path.hasPrefix(root.path+"/") else {throw MaschineGroupArchive.Invalid(message:"sample path leaves its expansion library.")}
        return url
    }
}
