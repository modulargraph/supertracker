import Foundation

extension MaschineGroupArchive {
    struct Zone {
        let objectID: Int
        let path: String
        let origin: String
        let library: String
        let values: [String:Double]
    }
    func zones(for module: Module) throws -> [Zone] {
        guard module.type == "Sampler", [0,1,2].contains(module.version), let first = modules.first(where:{$0.type == "Sampler"}) else {
            throw Invalid(message:"unsupported sampler layout.")
        }
        let view = View(bytes:bytes)
        let firstID = first.objectID + 1
        let firstHits = view.matches(Self.encoded(firstID),within:first.bodyOffset..<min(first.end,first.bodyOffset+48))
        var classIDs: [Int] = []
        for hit in firstHits where hit >= 6 {
            var p = hit-6
            if let cid = try? view.integer(&p,limit:hit), (try? view.integer(&p,limit:hit)) == 1,
               (try? view.integer(&p,limit:hit)) == 2,p == hit { classIDs.append(cid) }
        }
        guard classIDs.count == 1 else { throw Invalid(message:"the sample Zone class is ambiguous.") }
        let cid = classIDs[0]
        var candidates: [(Int,Int,String,String,String)] = []
        for hit in view.matches(Self.encoded(cid),within:module.bodyOffset..<module.end) {
            var p = hit + Self.encoded(cid).count
            if module.objectID == first.objectID && p+4 <= module.end && bytes[p..<p+4].elementsEqual([1,1,1,2]) { p += 4 }
            guard let id = try? view.integer(&p,limit:module.end), id > module.objectID else { continue }
            if p+2 <= module.end && bytes[p] == 0 && bytes[p+1] == 0 {p += 2}
            guard (try? view.integer(&p,limit:module.end)) == 2,
                  let path = try? view.string(&p,limit:module.end),!path.isEmpty else { continue }
            if p+2 <= module.end && bytes[p] == 0 && bytes[p+1] == 0 {p += 2}
            guard let origin = try? view.string(&p,limit:module.end),let library = try? view.string(&p,limit:module.end) else {continue}
            candidates.append((hit,id,path,origin,library))
        }
        guard !candidates.isEmpty,candidates.count <= 128,candidates[0].1 == module.objectID+1 else {throw Invalid(message:"the sampler has no safely decoded sample zones.")}
        // The vector's stored count must match the parsed Zone objects exactly.
        let vector = candidates[0].0
        let countPattern = Self.encoded(candidates.count) + [1,1]
        let expectedGap = module.objectID == first.objectID ? 3 : 0
        guard vector >= countPattern.count+expectedGap,
              bytes[(vector-countPattern.count-expectedGap)..<(vector-expectedGap)].elementsEqual(countPattern) else {
            throw Invalid(message:"the sampler's declared zone count does not match its objects.")
        }
        let keys = ["startFrame","endFrame","loopStartFrame","loopEndFrame","loopEnabled","crossfadeFrames","unused1","unused2","root","keyLow","keyHigh","velocityLow","velocityHigh","gain","pan","tune"]
        let deltas = modern ? [4,7,11,14,18,22,26,30,34,38,41,45,48,52,56,60] : [3,5,8,10,13,16,19,22,25,28,30,33,35,38,41,44]
        return try candidates.enumerated().map { index, item in
            let end = index+1 < candidates.count ? candidates[index+1].0 : module.end
            var values:[String:Double] = [:]
            for i in keys.indices {values[keys[i]] = try scalar(finalID:item.1+deltas[i],float:i>=13,within:item.0..<end)}
            for key in ["root","keyLow","keyHigh","velocityLow","velocityHigh"] {
                guard let v=values[key],(0...127).contains(v) else {throw Invalid(message:"invalid zone MIDI range.")}
            }
            // Sampler version 0 stores an unset loop end as 0; it then spans the playback range.
            if module.version == 0, values["loopEndFrame"] == 0, values["loopStartFrame"] == 0 { values["loopEndFrame"] = values["endFrame"] }
            guard values["keyLow"]! <= values["keyHigh"]!,values["velocityLow"]! <= values["velocityHigh"]!,
                  values["startFrame"]! >= 0,values["endFrame"]! >= values["startFrame"]!,
                  values["loopStartFrame"]! >= 0,values["loopEndFrame"]! >= values["loopStartFrame"]!,
                  (0...1).contains(values["gain"]!),(-1...1).contains(values["pan"]!),(-36...36).contains(values["tune"]!),
                  [0,1].contains(values["loopEnabled"]!) else {throw Invalid(message:"invalid zone sample range.")}
            return Zone(objectID:item.1,path:item.2,origin:item.3,library:item.4,values:values)
        }
    }
}

extension MaschineGroupArchive {
    /// The single audio file referenced by an Audio module (Maschine 2's loop player).
    /// Its sample reference (record version 2, relative path, original location, library
    /// name) sits near the start of the module body: in versions 4+ inside its own tracked
    /// object, in version 3 directly after the first parameter. Only the reference is
    /// decoded; the module's own playback controls are not.
    func audioModuleSample(for module: Module) throws -> (objectID: Int, path: String, origin: String, library: String) {
        guard module.type == "AudioModule" else { throw Invalid(message:"not an Audio module.") }
        let view = View(bytes:bytes)
        let limit = min(module.end, module.bodyOffset + 4096)
        var found: [(Int,String,String,String)] = []
        for start in module.bodyOffset..<min(limit, module.bodyOffset + 96) {
            var p = start
            guard (try? view.integer(&p,limit:limit)) == 2,
                  let path = try? view.string(&p,limit:limit), !path.isEmpty,
                  Self.audioExtensions.contains((path as NSString).pathExtension.lowercased()) else { continue }
            if p+2 <= limit && bytes[p] == 0 && bytes[p+1] == 0 { p += 2 }
            guard let origin = try? view.string(&p,limit:limit), let library = try? view.string(&p,limit:limit) else { continue }
            found.append((module.objectID,path,origin,library))
        }
        guard found.count == 1, let item = found.first else { throw Invalid(message:"the Audio module's sample reference is missing or ambiguous.") }
        return (item.0, item.1, item.2, item.3)
    }
    static let audioExtensions: Set<String> = ["wav","wave","aif","aiff","flac","mp3","ogg","rx2","rex","rcy"]

    /// Flat Sampler fields followed by two nine-control envelope objects.
    /// Keys are native member offsets until a verified control mapping is available.
    func samplerParameters(for module:Module,zones:[Zone]) throws -> [String:Double] {
        guard let last=zones.last else {throw Invalid(message:"empty sample zone vector.")}
        var id=last.objectID+(modern ? 79 : 58)
        let fields:[(String,Character)] = [("0xc0","e"),("0xc8","e"),("0xd0","f"),("0xd8","f"),("0xe0","e"),("0xe8","e"),("0xf0","b"),("0xf8","f"),("0x100","f"),("0x128","f"),("0x130","f"),("0x138","f"),("0x140","f"),("0x148","e"),("0x150","f"),("0x158","f"),("0x160","f"),("0x168","f"),("0x170","f"),("0x178","f"),("0x180","f"),("0x188","f"),("0x190","f"),("0x198","f"),("0x1a0","f"),("0x1a8","f"),("0x1b0","f")]
        var result:[String:Double]=[:]
        // Sampler version 0 (Maschine 1.x) has no member at 0xc0; every later field
        // keeps its kind and order, so its object IDs are those of version 1 shifted by one field.
        for (key,kind) in fields where module.version > 0 || key != "0xc0" {
            id += kind == "e" ? (modern ? 5 : 4) : (modern ? 4 : 3)
            result[key]=try scalar(finalID:id,float:kind == "f",within:module.bodyOffset..<module.end)
        }
        for envelope in ["ampEnvelope","filterEnvelope"] {
            for i in 0..<9 {id += modern ? 4 : 3;result["\(envelope).\(i)"]=try scalar(finalID:id,float:true,within:module.bodyOffset..<module.end)}
        }
        return result
    }
}
