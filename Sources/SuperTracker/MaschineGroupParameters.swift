import Foundation

extension MaschineGroupArchive {
    /// The persisted value is a float, integer or enum depending on the original
    /// parameter class. Signed archive values are preserved before control mapping.
    func scalar(finalID: Int, float: Bool, within range: Range<Int>) throws -> Double {
        let view = View(bytes: bytes)
        let prefix = Self.encoded(finalID - 1) + Self.encoded(finalID)
        var values: [Double] = []
        for offset in view.matches(prefix, within: range) {
            var cursor = offset + prefix.count
            if modern { guard cursor < range.upperBound, bytes[cursor] == 0 else { continue }; cursor += 1 }
            if float {
                guard cursor + 17 <= range.upperBound,
                      let bits = view.little(at: cursor, count: 4),
                      let second = view.little(at: cursor+4,count:4), let third = view.little(at:cursor+8,count:4),
                      Float(bitPattern:UInt32(bits)).isFinite,Float(bitPattern:UInt32(second)).isFinite,
                      Float(bitPattern:UInt32(third)).isFinite else { continue }
                var tail = cursor + 12
                guard let flag = try? view.integer(&tail,limit:range.upperBound), flag <= 1,
                      tail + 4 <= range.upperBound,let modulation = view.little(at:tail,count:4),Float(bitPattern:UInt32(modulation)).isFinite else {continue}
                values.append(Double(Float(bitPattern:UInt32(bits))))
            } else {
                guard let value = try? view.signedInteger(&cursor, limit: range.upperBound) else { continue }
                var valid = true
                for _ in 0..<4 { if (try? view.signedInteger(&cursor,limit:range.upperBound)) == nil { valid = false;break } }
                if valid { values.append(Double(value)) }
            }
        }
        guard values.count == 1 else { throw Invalid(message:"parameter object \(finalID) is missing or ambiguous.") }
        return values[0]
    }
    // Layouts checked against the native class serializers. SideChainFilter is
    // deliberately outside these flat arrays; it carries a separate routing graph.
    func parameters(for module: Module) throws -> [Double] {
        if module.type == "Limiter",module.version == 1 {
            let width = modern ? 4 : 3
            let range = module.bodyOffset..<module.end
            var result = [try scalar(finalID:module.objectID+width,float:true,within:range)]
            // SideChainFilter contains one source object, three numeric controls
            // and one boolean. Validate every member before crossing this block.
            var id = module.objectID+width+1
            for kind in [true,true,true,false] {
                id += width
                _ = try scalar(finalID:id,float:kind,within:range)
            }
            id += modern ? 5 : 4
            result.append(try scalar(finalID:id,float:false,within:range))
            for _ in 0..<2 {id += width;result.append(try scalar(finalID:id,float:true,within:range))}
            return result
        }
        let schemas: [String:(Set<Int>,String)] = [
            "BeatDelay":([0],"eeffffffe"), "CabinetEmulation":([0],"eeff"), "Chorus":([0],"fff"),
            "Compressor":([1],"effffff"), "Cymbal":([0],"efffffffeffff"), "EQ":([0],"fffffffffff"),
            "FM":([0],"ffff"), "Filter":([0],"efffeffffff"), "Flanger":([0],"ffffffbf"),
            "FreqShifter":([0],"ffffbf"), "Gate":([0],"fffff"), "GrainDelay":([0],"fffbfffff"),
            "GrainStretch":([0],"bfefbff"), "Hihat":([2],"efffbffffffeffffff"), "Iceverb":([0],"ffff"),
            "Kick":([2],"effffffefffffffffffeffeffeffbff"), "LoFi":([0],"fffff"),
            "Maximizer":([0],"ffb"), "Metaverb":([0],"fffff"), "MullhollandDrive":([1],"efffbfffffffbff"),
            "Perc":([1],"effffeffffffffbeffeeffefeffff"), "Phaser":([0],"ffeffffbf"),
            "PlateReverb":([1],"fffffefffffffffffffffff"), "Reflex":([0],"ffff"),
            "Resochord":([0],"eeefiffff"), "Reverb":([0],"effffffb"), "Saturator":([0],"efffffffffffbb"),
            "Snare":([1],"efffffeffffefffffbffff"), "Tom":([1],"efffffffffeffffffffffff"),
            "TransientDesigner":([0],"fffb")]
        guard let schema = schemas[module.type], schema.0.contains(module.version) else {
            throw Invalid(message:"\(module.type) version \(module.version) has no verified parameter schema.")
        }
        var id = module.objectID
        return try schema.1.map { kind in
            id += kind == "e" ? (modern ? 5 : 4) : (modern ? 4 : 3)
            return try scalar(finalID:id,float:kind == "f",within:module.bodyOffset..<module.end)
        }
    }
}

extension MaschineGroupArchive.View {
    func signedInteger(_ offset: inout Int, limit: Int) throws -> Int {
        guard offset >= 0, offset < min(limit,bytes.count) else { throw MaschineGroupArchive.Invalid(message:"truncated signed integer.") }
        let marker = Int(Int8(bitPattern:bytes[offset])); offset += 1
        let width = abs(marker)
        guard width <= 4, offset <= min(limit,bytes.count)-width,
              let raw = little(at:offset,count:width),width == 0 || bytes[offset+width-1] != 0 else {
            throw MaschineGroupArchive.Invalid(message:"non-canonical signed integer.")
        }
        offset += width
        return marker < 0 ? -Int(raw) : Int(raw)
    }
}
