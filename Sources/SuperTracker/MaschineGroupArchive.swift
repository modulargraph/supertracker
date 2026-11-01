import Foundation

/// A bounded reader for the portable archive stored inside a Maschine Group.
/// The accepted layouts are versioned. Object IDs, class records and the declared
/// sixteen-element Sound vector establish pad identity; sample filenames never do.
struct MaschineGroupArchive {
    struct Invalid: LocalizedError {
        let message: String
        var errorDescription: String? { "Maschine group: " + message }
    }
    struct Sound {
        let index: Int
        let objectID: Int
        let offset: Int
        let nameObjectID: Int
        let nameOffset: Int
        let name: String
        var end: Int
    }
    struct Module {
        let slotObjectID: Int
        let objectID: Int
        let offset: Int
        let bodyOffset: Int
        let type: String
        let version: Int
        let enabled: Bool
        var end: Int
    }
    struct ClassRecord {
        let classID: Int
        let name: String
        let version: Int
        let definitionOffset: Int
        let objectOffset: Int
    }
    let bytes: [UInt8]
    let archiveVersion: Int
    let groupVersion: Int
    let soundVersion: Int
    let soundClassID: Int
    let archiveRange: Range<Int>
    let sounds: [Sound]
    let classes: [Int: ClassRecord]
    /// nil for a Group without any insert Slot (MIDI-only Sounds).
    let slotClassID: Int?
    let modules: [Module]
    let groupNameOffset: Int
    let groupName: String
    let soundLayout: SoundLayout
    var modern: Bool { archiveVersion >= 13 }

    /// Boost serialization library versions written by Maschine 1.8 (10) up to Maschine 3 (19).
    /// Pattern event classes registered alongside the Module classes.
    static let eventClassNames: Set<String> = ["NoteEvent", "MidiEvent", "AudioEvent", "FloatModulationEvent",
        "IntModulationEvent", "BoolModulationEvent", "EnumModulationEvent"]
    static let supportedArchiveVersions: Set<Int> = [10, 12, 13, 14, 15, 16, 17, 18, 19]
    /// Object-ID distances that depend on the Sound class version. Each was established
    /// from the installed libraries and is re-validated per file: the name record must be a
    /// unique, well-formed WStringParameter and every mixer member must decode in range.
    struct SoundLayout: Equatable {
        /// Sound object ID to its name parameter (first Sound, which introduces tracked bases).
        let firstNameDelta: Int
        /// Sound object ID to its name parameter for Sounds 2...16.
        let nameDelta: Int
        /// Object-ID shift of the MixingLayer members relative to Sound version 6.
        let channelShift: Int
    }
    static func layout(soundVersion: Int, modern: Bool) -> SoundLayout? {
        switch (soundVersion, modern) {
        // Maschine 1.x: versions 1...3 lack one routing member of version 6;
        // versions 4 and 5 carry two further routing members that version 6 dropped.
        case (1...3, false): return SoundLayout(firstNameDelta: 92, nameDelta: 89, channelShift: -1)
        case (4...5, false): return SoundLayout(firstNameDelta: 95, nameDelta: 92, channelShift: 2)
        case (6, false): return SoundLayout(firstNameDelta: 93, nameDelta: 90, channelShift: 0)
        // Maschine 2: one tracked base per parameter (four IDs per parameter instead of three).
        case (6, true): return SoundLayout(firstNameDelta: 119, nameDelta: 115, channelShift: 0)
        // Versions 8 and 9 add the per-Sound note/MIDI parameter block before the name.
        case (8...9, true): return SoundLayout(firstNameDelta: 643, nameDelta: 639, channelShift: 0)
        default: return nil
        }
    }

    init(data: Data) throws {
        guard data.count >= 256, data.count <= 32 * 1024 * 1024 else {
            throw Invalid(message: "the file is empty, truncated, or larger than 32 MB.")
        }
        let b = Array(data)
        let view = View(bytes: b)
        guard view.little(at: 0, count: 8) == UInt64(data.count),
              Array(b[12..<16]) == Array("hsin".utf8),
              !view.matches(Array("2SAM PRG".utf8), within: 0..<min(4096, b.count)).isEmpty else {
            throw Invalid(message: "this is not a supported native .mxgrp container.")
        }
        let signature = Array("serialization::archive".utf8)
        let signatures = view.matches(signature, within: 0..<b.count)
        guard signatures.count == 1, let magic = signatures.first, magic >= 6,
              b[magic-2] == 1, b[magic-1] == signature.count,
              let count = view.little(at: magic-6, count: 4), count >= 32,
              count <= UInt64(b.count - magic + 2) else {
            throw Invalid(message: "the portable archive envelope is invalid or ambiguous.")
        }
        let stop = magic - 2 + Int(count)
        let range = (magic + signature.count)..<stop
        var position = range.lowerBound
        let av = try view.integer(&position, limit: stop)
        guard try view.integer(&position, limit: stop) == 0,
              try view.integer(&position, limit: stop) == 1 else {
            throw Invalid(message: "the archive byte order or tracking mode is unsupported.")
        }
        let gv = try view.integer(&position, limit: stop)
        // Boost library versions 10 and 12...19 and Group versions 1...23 occur in the
        // installed Maschine 1.8 to 3 libraries. Every accepted file must then also pass
        // the per-Sound-version member layout checks below; unknown versions are refused.
        guard Self.supportedArchiveVersions.contains(av), (1...23).contains(gv) else {
            throw Invalid(message: "archive \(av), Group \(gv) has not been decoded safely yet.")
        }
        // std::vector<shared_ptr<Sound>>: count=16, item version=1,
        // shared_ptr class introduction, followed by the Sound class/version.
        let vectorPrefix: [UInt8] = [1,16,1,1,0,1,1]
        var headers: [(Int,Int,Int)] = []
        for offset in view.matches(vectorPrefix, within: position..<stop) {
            var cursor = offset + vectorPrefix.count
            guard let cid = try? view.integer(&cursor, limit: stop), (1...255).contains(cid),
                  (try? view.integer(&cursor, limit: stop)) == 1,
                  let version = try? view.integer(&cursor, limit: stop), Self.layout(soundVersion: version, modern: av >= 13) != nil,
                  let object = try? view.integer(&cursor, limit: stop), object > 0,
                  (try? view.integer(&cursor, limit: stop)) == 2,
                  (try? view.integer(&cursor, limit: stop)) == object + 1 else { continue }
            headers.append((offset + vectorPrefix.count, cid, version))
        }
        guard headers.count == 1, let header = headers.first else {
            throw Invalid(message: "the sixteen-Sound vector cannot be identified unambiguously.")
        }
        var records: [(Int,Int)] = []
        for offset in view.matches(Self.encoded(header.1), within: position..<stop) {
            var cursor = offset + Self.encoded(header.1).count
            if offset == header.0 {
                guard (try? view.integer(&cursor, limit: stop)) == 1,
                      (try? view.integer(&cursor, limit: stop)) == header.2 else { continue }
            }
            guard let object = try? view.integer(&cursor, limit: stop), object > 0,
                  (try? view.integer(&cursor, limit: stop)) == 2,
                  (try? view.integer(&cursor, limit: stop)) == object+1 else { continue }
            records.append((offset, object))
        }
        guard records.count == 16, records.first?.0 == header.0,
              zip(records, records.dropFirst()).allSatisfy({ $0.0.1 < $0.1.1 }) else {
            throw Invalid(message: "Sound object identities are incomplete or ambiguous.")
        }
        guard let layout = Self.layout(soundVersion: header.2, modern: av >= 13) else {
            throw Invalid(message: "Sound version \(header.2) has not been decoded safely yet.")
        }
        var decodedSounds: [Sound] = []
        for (index, record) in records.enumerated() {
            let end = index + 1 < records.count ? records[index+1].0 : stop
            // Each Sound version has a fixed sequence of routing, MIDI and MixingLayer
            // members before its name. The first Sound also introduces tracked objects
            // that later Sounds normally reference by ID, so its name sits a few object
            // IDs further on. Some Groups (e.g. external-MIDI kits) give every Sound its
            // own copy instead; exactly one of the two readings must validate.
            let deltas = index == 0 ? [layout.firstNameDelta] : [layout.nameDelta, layout.firstNameDelta]
            let candidates = deltas.flatMap { delta in
                view.names(objectID: record.1 + delta, within: record.0..<min(end,record.0+8192), modern: av >= 13)
            }
            guard candidates.count == 1, let name = candidates.first else {
                throw Invalid(message: "Sound \(index+1)'s name/member layout is unsupported.")
            }
            let nameID = name.objectID
            decodedSounds.append(Sound(index: index, objectID: record.1, offset: record.0,
                nameObjectID: nameID, nameOffset: name.offset, name: name.value, end: end))
        }
        // There is one further MixingLayer, owned by the Group, after Sound 16.
        // Its member shape, rather than its display name, determines the boundary.
        let last = decodedSounds[15]
        let groupRange = (last.nameOffset + last.name.utf8.count + 32)..<stop
        var groupNames = view.names(objectID: nil, within: groupRange, modern: av >= 13)
            .filter { $0.objectID > last.nameObjectID }
        if groupNames.isEmpty {
            // A few Groups store non-zero data in the name parameter's trailing block.
            // Only then is the relaxed reading (all three floats finite, flag 0/1) used,
            // and it must still identify exactly one record.
            groupNames = view.names(objectID: nil, within: groupRange, modern: av >= 13, strictTrailer: false)
                .filter { $0.objectID > last.nameObjectID }
        }
        guard groupNames.count == 1, let gn = groupNames.first else {
            throw Invalid(message: "the Group/Sound effect boundary is not unambiguous.")
        }
        decodedSounds[15].end = gn.offset
        var classRecords: [Int: ClassRecord] = [:]
        let typePrefix = Array("NI::MASCHINE::DATA::".utf8)
        for offset in view.matches(typePrefix, within: position..<stop) {
            guard offset >= 4, b[offset-2] == 1 else { continue }
            let length = Int(b[offset-1])
            guard length >= typePrefix.count, length <= 128, offset+length < stop,
                  let full = String(bytes: b[offset..<offset+length], encoding: .utf8),
                  full.utf8.allSatisfy({ $0 == 58 || $0 == 95 || (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) }) else { continue }
            var c = offset-4
            guard let id = try? view.integer(&c, limit: offset-2), c == offset-2 else { continue }
            c = offset+length
            guard (try? view.integer(&c, limit: stop)) == 1,
                  let version = try? view.integer(&c, limit: stop), version <= 32 else { continue }
            let item = ClassRecord(classID: id, name: String(full.dropFirst(typePrefix.count)), version: version,
                                   definitionOffset: offset-4, objectOffset: c)
            guard classRecords[id] == nil else { throw Invalid(message: "duplicate module class registration.") }
            classRecords[id] = item
        }
        // The Slot class wraps an enable parameter and one polymorphic Module. It is
        // declared with the first Slot in the archive: the first Sound (or, failing that,
        // the Group) that has an insert chain. Its first Slot directly follows the name.
        let owners = decodedSounds.map { ($0.nameObjectID, $0.nameOffset, $0.end) } + [(gn.objectID, gn.offset, stop)]
        var declaration: (offset: Int, classID: Int, slotID: Int)?
        for (nameID, nameOffset, ownerEnd) in owners {
            let firstSlot = nameID + (av >= 13 ? 18 : 14)
            let idSequence = (firstSlot...(firstSlot + (av >= 13 ? 4 : 3))).flatMap(Self.encoded)
            let hits = view.matches(idSequence, within: nameOffset..<ownerEnd)
            guard hits.count == 1, let hit = hits.first, hit >= 5 else {
                guard hits.isEmpty else { throw Invalid(message: "the plug-in Slot layout is unsupported.") }
                continue
            }
            var p = hit - 5
            guard let sc = try? view.integer(&p, limit: hit),
                  (try? view.integer(&p, limit: hit)) == 1,
                  (try? view.integer(&p, limit: hit)) == 0,
                  p == hit else { throw Invalid(message: "the Slot class declaration is invalid.") }
            declaration = (hit, sc, firstSlot); break
        }
        var allModules: [Module] = []
        let moduleClasses = classRecords.values.filter { !Self.eventClassNames.contains($0.name) }
        guard let (firstSlotOffset, sc, firstSlot) = declaration else {
            // MIDI-only Groups (external hardware templates, input Sounds) have no Slot at
            // all. That is only accepted when no Module class is registered anywhere.
            guard moduleClasses.isEmpty else { throw Invalid(message: "the plug-in Slot layout is unsupported.") }
            bytes = b; archiveVersion = av; groupVersion = gv; soundVersion = header.2; soundLayout = layout
            soundClassID = header.1; archiveRange = range; sounds = decodedSounds
            classes = classRecords; slotClassID = nil; modules = []
            groupNameOffset = gn.offset; groupName = gn.value
            return
        }
        for offset in view.matches(Self.encoded(sc), within: firstSlotOffset-5..<stop) {
            var c = offset + Self.encoded(sc).count
            if c == firstSlotOffset-3 { c = firstSlotOffset }
            guard let slotID = try? view.integer(&c, limit: stop), slotID >= firstSlot else { continue }
            var valid = true
            let inherited = av >= 13 ? 4 : 3
            for n in 1...inherited where (try? view.integer(&c, limit: stop)) != slotID+n { valid = false; break }
            guard valid else { continue }
            if av >= 13 { guard c < stop, b[c] == 0 else { continue }; c += 1 }
            guard let enabled = try? view.integer(&c, limit: stop), enabled <= 1 else { continue }
            for _ in 0..<4 { guard let ignored = try? view.integer(&c, limit: stop), ignored <= 1 else { valid = false; break } }
            guard valid else { continue }
            // First shared_ptr<Module> introduces the shared-pointer class.
            if c+3 <= stop, Array(b[c..<c+3]) == [0,1,1] { c += 3 }
            let moduleClassOffset = c
            guard let classID = try? view.integer(&c, limit: stop), let info = classRecords[classID] else { continue }
            if moduleClassOffset == info.definitionOffset { c = info.objectOffset }
            guard let objectID = try? view.integer(&c, limit: stop), objectID == slotID+inherited+1 else { continue }
            allModules.append(Module(slotObjectID: slotID, objectID: objectID, offset: offset,
                bodyOffset: c, type: info.name, version: info.version, enabled: enabled == 1, end: stop))
        }
        guard !allModules.isEmpty else { throw Invalid(message: "no supported Module records were found.") }
        for index in allModules.indices {
            let ownBoundary = decodedSounds.first(where: { $0.offset <= allModules[index].offset && allModules[index].offset < $0.end })?.end ?? stop
            allModules[index].end = min(ownBoundary, index+1 < allModules.count ? allModules[index+1].offset : stop)
        }
        bytes = b; archiveVersion = av; groupVersion = gv; soundVersion = header.2; soundLayout = layout
        soundClassID = header.1; archiveRange = range; sounds = decodedSounds
        classes = classRecords; slotClassID = sc; modules = allModules
        groupNameOffset = gn.offset; groupName = gn.value
    }

    func modules(for sound: Sound) -> [Module] { modules.filter { sound.offset <= $0.offset && $0.offset < sound.end } }
    var groupModules: [Module] { modules.filter { $0.offset > groupNameOffset } }

    static func encoded(_ value: Int) -> [UInt8] {
        guard value > 0 else { return [0] }
        var v = UInt64(value), result: [UInt8] = []
        while v != 0 { result.append(UInt8(v & 255)); v >>= 8 }
        return [UInt8(result.count)] + result
    }

    struct View {
        let bytes: [UInt8]
        func little(at: Int, count: Int) -> UInt64? {
            guard count >= 0, count <= 8, at >= 0, at <= bytes.count-count else { return nil }
            return (0..<count).reduce(0) { $0 | UInt64(bytes[at+$1]) << (8*$1) }
        }
        func integer(_ offset: inout Int, limit: Int) throws -> Int {
            guard offset >= 0, offset < min(limit,bytes.count) else { throw Invalid(message: "truncated integer.") }
            let width = Int(bytes[offset]); offset += 1
            guard width <= 4, offset <= min(limit,bytes.count)-width,
                  let value = little(at: offset,count: width), width == 0 || bytes[offset+width-1] != 0 else {
                throw Invalid(message: "non-canonical or oversized archive integer.")
            }
            offset += width
            return Int(value)
        }
        func string(_ offset: inout Int, limit: Int, maximum: Int = 4096) throws -> String {
            let length = try integer(&offset,limit:limit)
            guard length <= maximum, offset <= min(limit,bytes.count)-length,
                  let value = String(bytes: bytes[offset..<offset+length], encoding:.utf8), !value.contains("\0") else {
                throw Invalid(message: "invalid archive string.")
            }
            offset += length
            return value
        }
        func matches(_ pattern: [UInt8], within range: Range<Int>) -> [Int] {
            guard !pattern.isEmpty, range.lowerBound >= 0, range.upperBound <= bytes.count,
                  range.count >= pattern.count else { return [] }
            var matches: [Int] = []
            for i in range.lowerBound...(range.upperBound-pattern.count) where bytes[i] == pattern[0] {
                if pattern.indices.allSatisfy({ bytes[i+$0] == pattern[$0] }) { matches.append(i) }
            }
            return matches
        }
        struct Name { let offset: Int; let objectID: Int; let value: String }
        func names(objectID: Int?, within range: Range<Int>, modern: Bool, strictTrailer: Bool = true) -> [Name] {
            var found: [Name] = []
            let candidates: AnySequence<Int> = objectID.map { AnySequence(matches(MaschineGroupArchive.encoded($0),within:range)) } ?? AnySequence(range)
            for start in candidates {
                var at = start
                guard let id = try? integer(&at,limit:range.upperBound), objectID == nil || id == objectID, id > 0 else { continue }
                // The first WStringParameter instance registers two untracked bases (two zero
                // bytes). An empty name also begins with zero bytes, so both readings are tried;
                // the trailing tracked IDs and value block allow at most one of them to validate.
                let skips = at+2 <= range.upperBound && bytes[at] == 0 && bytes[at+1] == 0 ? [2,0] : [0]
                for skip in skips {
                    if let name = parameterName(id:id, at:at+skip, range:range, modern:modern, strictTrailer:strictTrailer) {
                        found.append(Name(offset:start,objectID:id,value:name)); break
                    }
                }
            }
            return found
        }
        private func parameterName(id:Int, at start:Int, range:Range<Int>, modern:Bool, strictTrailer:Bool) -> String? {
            var at = start
            if modern { guard at < range.upperBound, bytes[at] == 0 else { return nil }; at += 1 }
            guard let name = try? string(&at,limit:range.upperBound,maximum:128),
                  name.unicodeScalars.allSatisfy({ !CharacterSet.controlCharacters.contains($0) }) else { return nil }
            let width = modern ? 4 : 3
            for n in 1...width where (try? integer(&at,limit:range.upperBound)) != id+n { return nil }
            guard at + (modern ? 18 : 17) <= range.upperBound else { return nil }
            let f = at + (modern ? 1 : 0)
            guard !modern || bytes[at] == 0, let bits = little(at:f,count:4), Float(bitPattern:UInt32(bits)).isFinite else { return nil }
            if strictTrailer {
                guard bytes[(f+12)..<(f+17)].allSatisfy({ $0 == 0 }) else { return nil }
            } else {
                guard (1...2).allSatisfy({ little(at:f+4*$0,count:4).map { Float(bitPattern:UInt32($0)).isFinite } ?? false }),
                      bytes[f+12] <= 1 else { return nil }
            }
            return name
        }
    }
}
