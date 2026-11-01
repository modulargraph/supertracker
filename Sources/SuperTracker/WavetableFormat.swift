import Foundation

/// Wavetable files are ordinary WAV files holding consecutive single-cycle frames.
/// The frame size travels in a `clm ` chunk ("<!>2048 …", the common wavetable
/// convention); files without one fall back to size heuristics (see `detectFrameSize`).
enum WavetableFormat {
    /// Polyend-style default: one cycle is 2048 samples.
    static let defaultFrameSize = 2048
    /// Frame sizes the sampler accepts as an explicit setting.
    static let frameSizeRange = 16...8192

    /// Frame size declared in a WAV file's `clm ` chunk, nil when absent or not a WAV file.
    static func declaredFrameSize(atPath path: String) -> Int? {
        guard let handle = FileHandle(forReadingAtPath: path) else { return nil }
        defer { try? handle.close() }
        // Metadata chunks precede or follow the audio; only small headers are read.
        guard let header = try? handle.read(upToCount: 12), header.count == 12,
              header.prefix(4) == Data("RIFF".utf8), header.subdata(in: 8..<12) == Data("WAVE".utf8) else { return nil }
        var offset: UInt64 = 12
        for _ in 0..<64 {
            guard (try? handle.seek(toOffset: offset)) != nil, let chunk = try? handle.read(upToCount: 8), chunk.count == 8 else { return nil }
            let id = String(decoding: chunk.prefix(4), as: UTF8.self)
            let size = UInt64(chunk[4]) | UInt64(chunk[5]) << 8 | UInt64(chunk[6]) << 16 | UInt64(chunk[7]) << 24
            if id == "clm " {
                guard size <= 4096, let body = try? handle.read(upToCount: Int(size)) else { return nil }
                return frameSize(clmText: String(decoding: body, as: UTF8.self))
            }
            offset += 8 + size + (size & 1)
        }
        return nil
    }

    /// "<!>2048 01000000 wavetable (vendor)" -> 2048.
    static func frameSize(clmText text: String) -> Int? {
        guard text.hasPrefix("<!>") else { return nil }
        let digits = text.dropFirst(3).prefix(while: \.isNumber)
        guard let value = Int(digits), frameSizeRange.contains(value) else { return nil }
        return value
    }

    /// Declared size first; otherwise a whole number of default-size frames, or a
    /// short power-of-two file that is itself one cycle. Nil: the sampler default.
    static func detectFrameSize(declared: Int?, frames: Int) -> Int? {
        if let declared, frameSizeRange.contains(declared), frames >= declared { return declared }
        guard frames > 0 else { return nil }
        if frames % defaultFrameSize == 0 { return defaultFrameSize }
        if frames <= 4096, frames & (frames - 1) == 0, frames >= frameSizeRange.lowerBound { return frames }
        return nil
    }

    /// Mono 32-bit float WAV with a `clm ` frame-size chunk when `frameSize` is given.
    static func wavData(samples: [Float], sampleRate: Int = 48_000, frameSize: Int? = nil) -> Data {
        var data = Data()
        func append(_ text: String) { data.append(contentsOf: Array(text.utf8)) }
        func u32(_ value: UInt32) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
        func u16(_ value: UInt16) { withUnsafeBytes(of: value.littleEndian) { data.append(contentsOf: $0) } }
        var clm = Data()
        if let frameSize {
            clm.append(contentsOf: Array("<!>\(frameSize) 10000000 wavetable (SuperTracker)".utf8))
            if clm.count % 2 == 1 { clm.append(0) }
        }
        let audioBytes = UInt32(samples.count * 4)
        let clmBytes = clm.isEmpty ? 0 : UInt32(8 + clm.count)
        append("RIFF"); u32(4 + (8 + 16) + clmBytes + (8 + audioBytes)); append("WAVE")
        append("fmt "); u32(16); u16(3); u16(1); u32(UInt32(sampleRate)); u32(UInt32(sampleRate * 4)); u16(4); u16(32)
        if !clm.isEmpty { append("clm "); u32(UInt32(clm.count)); data.append(clm) }
        append("data"); u32(audioBytes)
        for sample in samples { u32(sample.bitPattern) }
        return data
    }
}
