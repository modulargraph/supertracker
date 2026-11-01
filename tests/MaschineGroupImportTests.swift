import Foundation
import TrackerEngine

@main struct MaschineGroupImportTests {
    static var checks=0
    static func check(_ condition:@autoclosure () throws -> Bool,_ message:String) throws {
        checks += 1
        if try !condition() {throw NSError(domain:"MaschineGroupImportTests",code:1,userInfo:[NSLocalizedDescriptionKey:message])}
    }
    static func reject(_ action:() throws -> Void,_ message:String) throws {
        do {try action()} catch {checks += 1;return};throw NSError(domain:"MaschineGroupImportTests",code:2,userInfo:[NSLocalizedDescriptionKey:message])
    }
    static func e(_ n:Int)->[UInt8] {MaschineGroupArchive.encoded(n)}
    static func string(_ text:String)->[UInt8] {e(text.utf8.count)+Array(text.utf8)}
    static func float(_ x:Float)->[UInt8] {(0..<4).map {UInt8((x.bitPattern >> ($0*8))&255)}}
    static func numeric(_ id:Int,_ value:Float)->[UInt8] {
        var bytes:[UInt8]=[]
        for part in [e(id-2),e(id-1),e(id),float(value),float(value),float(0),[0],float(0)] {bytes += part}
        return bytes
    }
    /// Original, synthetic archive containing a fixed Sound vector and tracked
    /// Iceverb modules. It contains no Native Instruments assets or executable code.
    /// `groupInserts` appends that many Group-level Iceverb Slots (bypassed where listed):
    /// Maschine Groups may hold more inserts than a SuperTracker channel's eight slots.
    static func fixture(moduleType:String="Iceverb",groupInserts:Int=0,bypassedGroupInserts:Set<Int> = [])->Data {
        var archive:[UInt8]=[1,22]
        for part in [Array("serialization::archive".utf8),e(12),e(0),e(1),e(9)] {archive += part}
        archive += [1,16,1,1,0,1,1]
        for index in 0..<16 {
            let id=100+index*1000,name=id+(index==0 ? 93:90),slot=name+14,module=slot+4
            archive += e(21)
            if index==0 {archive += e(1)+e(6)}
            archive += e(id)+e(2)+e(id+1)
            archive += e(name)+string("Sound \(index+1)")+numeric(name+3,0)
            archive += e(32)
            if index==0 {archive += e(1)+e(0)}
            for part in [e(slot),e(slot+1),e(slot+2),e(slot+3),e(1),e(1),e(0),e(0),e(0)] {archive += part}
            archive += e(40)
            if index==0 {archive += string("NI::MASCHINE::DATA::"+moduleType)+e(1)+e(0)}
            archive += e(module)
            for n in 1...4 {archive += numeric(module+n*3,Float(n)/4)}
        }
        archive += e(17000)+string("Synthetic Kit")+numeric(17003,0)
        for insert in 0..<groupInserts {
            let slot=20000+insert*100,module=slot+4
            for part in [e(32),e(slot),e(slot+1),e(slot+2),e(slot+3),e(bypassedGroupInserts.contains(insert) ? 0:1),e(1),e(0),e(0),e(0),e(40),e(module)] {archive += part}
            for n in 1...4 {archive += numeric(module+n*3,Float(n)/4)}
        }
        var container=[UInt8](repeating:0,count:32)
        container.replaceSubrange(12..<16,with:Array("hsin".utf8));container.replaceSubrange(20..<28,with:Array("2SAM PRG".utf8))
        container += (0..<4).map {UInt8((archive.count >> ($0*8))&255)}
        container += archive
        for i in 0..<8 {container[i]=UInt8((container.count >> (i*8))&255)}
        return Data(container)
    }
    static func main() throws {
        let original=fixture(),archive=try MaschineGroupArchive(data:original)
        try check(archive.sounds.count==16,"Synthetic Sound count")
        try check(archive.sounds.map(\.name)==(1...16).map{"Sound \($0)"},"Sound names must retain declared pad identity")
        try check(archive.modules.count==16,"Synthetic modules")
        for module in archive.modules {try check(try archive.parameters(for:module)==[0.25,0.5,0.75,1],"Typed parameter identity")}
        for length in [0,12,255,original.count-1] {try reject({_=try MaschineGroupArchive(data:original.prefix(length))},"truncated archive accepted")}
        var bad=original;bad[12]=0;try reject({_=try MaschineGroupArchive(data:bad)},"wrong format accepted")
        bad=original;bad[0] ^= 1;try reject({_=try MaschineGroupArchive(data:bad)},"wrong length accepted")
        bad=original;let signature=Data("serialization::archive".utf8);let signatureAt=bad.range(of:signature)!.lowerBound
        bad[signatureAt+signature.count+1]=99;try reject({_=try MaschineGroupArchive(data:bad)},"unknown archive version accepted")
        bad=original;let second=archive.sounds[1].offset;bad[second+1]=22;try reject({_=try MaschineGroupArchive(data:bad)},"missing Sound class accepted")
        bad=original
        let module=archive.modules[0]
        let parameterPrefix=e(module.objectID+2)+e(module.objectID+3)
        let parameterOffset=MaschineGroupArchive.View(bytes:Array(bad)).matches(parameterPrefix,within:module.bodyOffset..<module.end)[0]+parameterPrefix.count
        bad.replaceSubrange(parameterOffset..<parameterOffset+4,with:float(.nan))
        let nanArchive=try MaschineGroupArchive(data:bad)
        try reject({_=try nanArchive.parameters(for:nanArchive.modules[0])},"nonfinite parameter accepted")
        let temporary=FileManager.default.temporaryDirectory.appendingPathComponent("SuperTracker-MaschineTests-"+UUID().uuidString,isDirectory:true)
        try FileManager.default.createDirectory(at:temporary,withIntermediateDirectories:true)
        defer {try? FileManager.default.removeItem(at:temporary)}
        let syntheticURL=temporary.appendingPathComponent("Synthetic.mxgrp")
        try original.write(to:syntheticURL)
        let syntheticImport=try MaschineGroupImporter.read(at:syntheticURL,root:temporary)
        let syntheticProject=syntheticImport["project"] as! [String:Any]
        let syntheticSounds=try KitProject(project:syntheticProject).sounds
        try check(syntheticSounds.count==16 && syntheticSounds.allSatisfy{$0.instrument["unavailable"] as? Bool != true},"Effects-only Sounds must retain empty sources")
        let syntheticEffects=syntheticProject["audioPlugins"] as! [[String:Any]]
        try check(syntheticEffects.count==16 && syntheticEffects.allSatisfy{$0["kind"] as? String == "effect" && $0["slot"] as? Int == 0},"First-slot effects must remain actual inserts")
        // Regression (Faded Reels "Beautiful Kit"): a Group with eleven inserts. Channels hold sixteen
        // inserts, so every one is kept (renumbered contiguously, bypass state preserved).
        try check(MaschineGroupImporter.insertSlotLimit==16,"A channel holds sixteen inserts")
        let longGroup=fixture(groupInserts:11,bypassedGroupInserts:[2])
        try check(try MaschineGroupArchive(data:longGroup).groupModules.count==11,"Synthetic Group insert chain")
        try longGroup.write(to:syntheticURL)
        let longImport=try MaschineGroupImporter.read(at:syntheticURL,root:temporary)
        let longProject=longImport["project"] as! [String:Any]
        let longPlugins=longProject["audioPlugins"] as! [[String:Any]]
        let busOwner="bus:"+((longProject["kits"] as! [[String:Any]])[0]["busID"] as! String)
        let busPlugins=longPlugins.filter{$0["owner"] as? String == busOwner}
        try check(longPlugins.allSatisfy{($0["slot"] as? Int).map{(0..<MaschineGroupImporter.insertSlotLimit).contains($0)} ?? false},"Every imported insert must use a host slot 0...15")
        for owner in Set(longPlugins.compactMap{$0["owner"] as? String}) {
            let slots=longPlugins.filter{$0["owner"] as? String == owner}.compactMap{$0["slot"] as? Int}
            try check(slots==Array(0..<slots.count),"Inserts must be numbered contiguously per owner: \(owner) \(slots)")
        }
        try check(busPlugins.count==11,"A Group keeps all eleven inserts, got \(busPlugins.count)")
        try check(busPlugins.map{$0["bypass"] as? Bool ?? false}==(0..<11).map{$0==2},"Bypassed inserts are kept, bypassed, in their original order")
        let longReports=(longImport["report"] as! [String:Any])["modules"] as! [[String:Any]]
        try check(!longReports.contains{$0["status"] as? String == "omitted-insert-limit"},"Nothing is omitted within sixteen inserts")
        try check(!(longImport["warnings"] as! [String]).contains{$0.contains("inserts; its original state")},"No insert-limit warnings within sixteen inserts")
        // Beyond sixteen: bypassed inserts are left out first, then the trailing ones.
        try fixture(groupInserts:19,bypassedGroupInserts:[2]).write(to:syntheticURL)
        let hugeImport=try MaschineGroupImporter.read(at:syntheticURL,root:temporary)
        let hugeProject=hugeImport["project"] as! [String:Any]
        let hugeBus="bus:"+((hugeProject["kits"] as! [[String:Any]])[0]["busID"] as! String)
        let hugePlugins=(hugeProject["audioPlugins"] as! [[String:Any]]).filter{$0["owner"] as? String == hugeBus}
        try check(hugePlugins.compactMap{$0["slot"] as? Int}==Array(0..<16),"A Group keeps sixteen loadable inserts in slots 0...15")
        try check(hugePlugins.allSatisfy{$0["bypass"] as? Bool == false},"Bypassed inserts are omitted before active ones")
        let hugeReports=(hugeImport["report"] as! [String:Any])["modules"] as! [[String:Any]]
        let omitted=hugeReports.filter{$0["status"] as? String == "omitted-insert-limit"}
        try check(omitted.compactMap{$0["slot"] as? Int}.sorted()==[2,17,18],"Omitted inserts: bypassed first, then trailing; original state retained")
        try check(omitted.allSatisfy{$0["parameters"] != nil},"Omitted inserts keep their original parameters in the report")
        try check((hugeImport["warnings"] as! [String]).filter{$0.contains("holds 16 inserts")}.count==3,"Every omitted insert is reported")
        try check((try KitProject(project:longProject)).sounds.count==16,"Long Group remains a canonical kit")
        try fixture(moduleType:"UnknownInstrument").write(to:syntheticURL)
        let unknownProject=try MaschineGroupImporter.read(at:syntheticURL,root:temporary)["project"] as! [String:Any]
        try check(try KitProject(project:unknownProject).sounds.allSatisfy{$0.instrument["unavailable"] as? Bool == true},"Unsupported source modules must remain explicit unavailable Sounds")
        try FileManager.default.createSymbolicLink(at:temporary.appendingPathComponent("escaped"),withDestinationURL:temporary.deletingLastPathComponent())
        try reject({_=try MaschineGroupImporter.resolve("escaped/file.wav",library:temporary)},"symlink escaped expansion root")
        try FileManager.default.createSymbolicLink(atPath:temporary.appendingPathComponent("nested").path,withDestinationPath:"escaped/missing.wav")
        try reject({_=try MaschineGroupImporter.resolve("nested",library:temporary)},"nested dangling symlink escaped expansion root")
        try FileManager.default.createSymbolicLink(atPath:temporary.appendingPathComponent("cycle").path,withDestinationPath:"cycle")
        try reject({_=try MaschineGroupImporter.resolve("cycle/sample.wav",library:temporary)},"symbolic-link cycle accepted")
        try FileManager.default.createSymbolicLink(atPath:temporary.appendingPathComponent("inside").path,withDestinationPath:"Samples")
        try check(try MaschineGroupImporter.resolve("inside/missing.wav",library:temporary)==temporary.resolvingSymlinksInPath().appendingPathComponent("Samples/missing.wav"),"valid internal link should retain missing sample target")
        // One zone escaping its expansion library is skipped with a warning; the Sound keeps its other zones.
        let zoneValues:[String:Double]=["keyLow":0,"keyHigh":127,"velocityLow":0,"velocityHigh":127,"root":60,"pan":0,"tune":0,"gain":0.5,"loopEnabled":0,"startFrame":0,"endFrame":9,"loopStartFrame":0,"loopEndFrame":9]
        var zoneWarnings:[String]=[],zoneFrames:[String:Int64]=[:]
        let mappedZones=MaschineGroupImporter.mapZones([MaschineGroupArchive.Zone(objectID:1,path:"../escape.wav",origin:"",library:"",values:zoneValues),MaschineGroupArchive.Zone(objectID:2,path:"Samples/kept.wav",origin:"",library:"",values:zoneValues)],library:temporary,frameCache:&zoneFrames){zoneWarnings.append($0)}
        try check(mappedZones.count==1 && (mappedZones[0]["path"] as? String)?.hasSuffix("Samples/kept.wav")==true,"A zone outside its library must be skipped, not fail the Sound")
        try check(zoneWarnings.contains{$0.contains("../escape.wav") && $0.contains("leaves its expansion library")},"The skipped zone is reported")
        var cursor=0;let signed=MaschineGroupArchive.View(bytes:[255,12]);try check(try signed.signedInteger(&cursor,limit:2) == -12,"signed portable scalar")
        cursor=0;try reject({_=try MaschineGroupArchive.View(bytes:[5,1,2,3,4,5]).integer(&cursor,limit:6)},"oversized integer accepted")
        let library=URL(fileURLWithPath:"/tmp/expansion")
        try check(try MaschineGroupImporter.resolve("Samples/Drum.wav",library:library).lastPathComponent=="Drum.wav","relative sample")
        for path in ["../other.wav","/tmp/sample.wav","C:/sample.wav","Samples/../../escape.wav"] {try reject({_=try MaschineGroupImporter.resolve(path,library:library)},"unsafe source path accepted")}
        // Real expansions are checked when SUPERTRACKER_NI_CONTENT_DIR names the folder that holds them.
        let root=URL(fileURLWithPath:ProcessInfo.processInfo.environment["SUPERTRACKER_NI_CONTENT_DIR"] ?? "/Users/Shared/Native Access Content")
        var kits=0,zones=0,internalModules=0,samples=0
        if FileManager.default.fileExists(atPath:root.path) {
            for library in ["Deep Matter Library","Molten Veil Library"] {
                let directory=root.appendingPathComponent(library+"/Kits")
                guard FileManager.default.fileExists(atPath:directory.path) else {continue}
                for file in try FileManager.default.contentsOfDirectory(at:directory,includingPropertiesForKeys:nil).filter({$0.pathExtension=="mxgrp"}) {
                    let a=try MaschineGroupArchive(data:Data(contentsOf:file));kits += 1
                    try check(a.sounds.count==16,"real pad count")
                    for sound in a.sounds {_ = try a.channel(for:sound)}
                    for module in a.modules {
                        if module.type == "Sampler" {
                            let z=try a.zones(for:module);samples += 1;zones += z.count
                            _ = try a.samplerParameters(for:module,zones:z)
                        } else if module.type != "PluginHost" {_=try a.parameters(for:module);internalModules += 1}
                    }
                    let imported=try MaschineGroupImporter.read(at:file,root:directory.deletingLastPathComponent())
                    let project=imported["project"] as! [String:Any]
                    let report=imported["report"] as! [String:Any]
                    let moduleReports=report["modules"] as! [[String:Any]]
                    try check(moduleReports.count==a.modules.count,"every source module must have a report")
                    try check(moduleReports.allSatisfy{$0["status"] as? String != "unavailable"},"installed corpus contains an unmapped module: \(file.lastPathComponent) \(moduleReports.filter{$0["status"] as? String == "unavailable"}.map{String(describing: $0["type"] ?? "?")+":"+String(describing: $0["version"] ?? "?")+":"+String(($0["parameters"] as? [Double] ?? []).count)})")
                    try check((try KitProject(project:project)).sounds.count==16,"canonical validation")
                    try check(JSONSerialization.isValidJSONObject(imported),"import must round-trip JSON")
                    let kit=(project["kits"] as! [[String:Any]])[0]
                    try check((kit["sounds"] as! [[String:Any]]).map{$0["name"] as! String}==a.sounds.map(MaschineGroupImporter.displayName),"canonical slot preservation")
                    for sound in kit["sounds"] as! [[String:Any]] {
                        let instrument=sound["instrument"] as! [String:Any]
                        try check(instrument["drumModel"] == nil && instrument["type"] as? String != "synth","DrumSynth modules must import as samplers")
                        if let path=instrument["samplePath"] as? String,path.hasPrefix("builtin:drums/") {
                            try check(path.split(separator:"/").last?.count==64,"DrumSynth renders carry all eight controls")
                        }
                    }
                    for plugin in project["audioPlugins"] as! [[String:Any]] {
                        try check((plugin["slot"] as? Int).map{(0..<MaschineGroupImporter.insertSlotLimit).contains($0)} ?? false,"installed import produced an insert slot the host rejects: \(file.lastPathComponent)")
                        let data=Data(base64Encoded:plugin["state"] as! String)!
                        let state=try PropertyListSerialization.propertyList(from:data,format:nil) as! [String:Any]
                        if plugin["format"] as? String == "VST2" {
                            try check(state["format"] as? String == "SuperTracker.VST2","external VST2 envelope")
                            try check((state["data"] as? Data)?.prefix(4)==Data("STV2".utf8),"external VST2 binary state header")
                        } else {try check(state["format"] as? String == "SuperTracker.BuiltinFX","real restorable builtin effect state")}
                    }
                }
            }
        }
        // Faded Reels "Beautiful Kit" (when the expansion is mounted): its Group's eleven inserts all load.
        let beautiful=root.appendingPathComponent("Faded Reels Library/Groups/Kits/Beautiful Kit.mxgrp")
        if FileManager.default.fileExists(atPath:beautiful.path) {
            let imported=try MaschineGroupImporter.read(at:beautiful,root:root.appendingPathComponent("Faded Reels Library"))
            let project=imported["project"] as! [String:Any]
            let bus="bus:"+((project["kits"] as! [[String:Any]])[0]["busID"] as! String)
            let groupInserts=(project["audioPlugins"] as! [[String:Any]]).filter{$0["owner"] as? String == bus}
            let reports=(imported["report"] as! [String:Any])["modules"] as! [[String:Any]]
            let groupReports=reports.filter{$0["owner"] as? String == bus}
            print("Beautiful Kit Group modules: "+groupReports.map{"\($0["type"] ?? "?")=\($0["status"] ?? "?")"}.joined(separator:", "))
            let loadable=groupReports.filter{$0["status"] as? String != "unavailable"}.count
            try check(loadable>=11,"Beautiful Kit's Group holds at least eleven loadable inserts, got \(loadable)")
            try check(groupInserts.count==loadable && groupInserts.compactMap{$0["slot"] as? Int}==Array(0..<loadable),"Beautiful Kit imports every loadable Group insert in slots 0...\(loadable-1), got \(groupInserts.compactMap{$0["slot"] as? Int})")
            try check(!reports.contains{$0["status"] as? String == "omitted-insert-limit"},"Beautiful Kit omits no inserts")
            try check((try KitProject(project:project)).sounds.count==16,"Beautiful Kit remains a canonical kit")
            print("Beautiful Kit: \(groupInserts.count) Group inserts imported (\(groupInserts.compactMap{$0["name"] as? String}.joined(separator:", ")))")
        }
        // Every installed expansion (all archive/Group/Sound versions from Maschine 1.8 to 3):
        // a few Groups each by default, all of them with MASCHINE_FULL_CORPUS=1.
        var corpusGroups=0,corpusZones=0,corpusResolved=0,corpusREX=0
        if let libraries=try? FileManager.default.contentsOfDirectory(at:root,includingPropertiesForKeys:nil) {
            let full=ProcessInfo.processInfo.environment["MASCHINE_FULL_CORPUS"]=="1"
            for library in libraries.sorted(by:{$0.path<$1.path}) {
                guard let walker=FileManager.default.enumerator(at:library,includingPropertiesForKeys:nil) else {continue}
                var files:[URL]=[]
                for case let file as URL in walker where file.pathExtension=="mxgrp" && !file.lastPathComponent.hasPrefix("._") {files.append(file)}
                for file in files.sorted(by:{$0.path<$1.path}).prefix(full ? Int.max : 3) {
                    try autoreleasepool {
                        let label=file.path.dropFirst(root.path.count)
                        let a:MaschineGroupArchive
                        do {a=try MaschineGroupArchive(data:Data(contentsOf:file))} catch {throw NSError(domain:"MaschineGroupImportTests",code:3,userInfo:[NSLocalizedDescriptionKey:"\(label): \(error.localizedDescription)"])}
                        let imported=try MaschineGroupImporter.read(at:file)
                        let project=imported["project"] as! [String:Any]
                        try check((try KitProject(project:project)).sounds.count==16,"\(label): canonical kit")
                        let sounds=(project["kits"] as! [[String:Any]])[0]["sounds"] as! [[String:Any]]
                        try check(sounds.map{$0["name"] as! String}==a.sounds.map(MaschineGroupImporter.displayName),"\(label): pad order")
                        for sound in sounds {
                            for zone in (sound["instrument"] as? [String:Any])?["zones"] as? [[String:Any]] ?? [] {
                                // REX2 slices only play where a bundled WAV rendering matches them.
                                if (zone["path"] as? String)?.lowercased().hasSuffix(".rx2") == true {corpusREX += 1;continue}
                                corpusZones += 1;if zone["unavailable"] == nil {corpusResolved += 1}
                            }
                        }
                        corpusGroups += 1
                    }
                }
            }
            try check(corpusZones==0 || Double(corpusResolved) >= 0.99*Double(corpusZones),"installed sample zones must resolve: \(corpusResolved)/\(corpusZones)")
        }
        print("Installed corpus: \(corpusGroups) Groups imported, \(corpusResolved)/\(corpusZones) sample zones resolved, \(corpusREX) undecodable REX2 slices")
        let versionChecks=try MaschineGroupVersionTests.run()
        print("PASS \(versionChecks) archive-version checks (Maschine 1.8 to 3 layouts)")
        print("PASS \(checks) checks; \(kits) installed kits, \(samples) samplers, \(zones) sample zones, \(internalModules) internal FX/drum modules (read-only)")
    }
}
