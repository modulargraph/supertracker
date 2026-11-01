import Foundation
import AVFoundation

/// Pure policy assertions: no audio graph, devices, plug-ins, or file writes.
@main enum AudioExportFormatTests {
    static func reject(_ payload: [String: Any], mode: String = "song") {
        do {
            _ = try AudioExportFormat.resolve(payload: payload, mode: mode)
            preconditionFailure("Invalid export format was accepted: \(payload), \(mode)")
        } catch {
            precondition(!(error.localizedDescription.isEmpty))
        }
    }

    static func main() throws {
        for mode in ["song", "pattern", "stems"] {
            let implicit = try AudioExportFormat.resolve(payload: [:], mode: mode)
            precondition(implicit == .pcm24)
            for encoding in AudioExportFormat.allCases {
                let format = try AudioExportFormat.resolve(payload: ["encoding": encoding.rawValue, "container": "wav", "sampleRate": 48_000], mode: mode)
                precondition(format == encoding)
                let settings = format.settings
                precondition(settings[AVFormatIDKey] as? UInt32 == kAudioFormatLinearPCM)
                precondition(settings[AVSampleRateKey] as? Double == 48_000)
                precondition(settings[AVNumberOfChannelsKey] as? Int == 2)
                precondition(settings[AVLinearPCMBitDepthKey] as? Int == encoding.bitDepth)
                precondition(settings[AVLinearPCMIsFloatKey] as? Bool == encoding.isFloat)
                precondition(settings[AVLinearPCMIsBigEndianKey] as? Bool == false)
                precondition(format.metadata["encoding"] as? String == encoding.rawValue)
                precondition(format.metadata["container"] as? String == "wav")
                precondition(format.metadata["sampleRate"] as? Double == 48_000)
            }
        }
        for mode in ["song", "pattern", "stems", "track"] {
            let implicitRate = try AudioExportFormat.resolveSampleRate(payload: [:], mode: mode)
            precondition(implicitRate == 48_000)
            for rate in [44_100, 48_000, 96_000] {
                for encoding in AudioExportFormat.allCases {
                    for representation: Any in [rate, Double(rate), NSNumber(value: rate), NSDecimalNumber(value: rate)] {
                        let payload: [String: Any] = ["sampleRate": representation, "encoding": encoding.rawValue]
                        let actualRate = try AudioExportFormat.resolveSampleRate(payload: payload, mode: mode)
                        let actualEncoding = try AudioExportFormat.resolve(payload: payload, mode: mode)
                        precondition(actualRate == (mode == "track" ? 48_000 : rate))
                        precondition(actualEncoding == (mode == "track" ? .float32 : encoding))
                        // Legacy intermediate settings and metadata must remain at 48 kHz.
                        precondition(actualEncoding.settings[AVSampleRateKey] as? Double == 48_000)
                        precondition(actualEncoding.metadata["sampleRate"] as? Double == 48_000)
                    }
                }
            }
        }
        let bounce = try AudioExportFormat.resolve(payload: [:], mode: "track")
        precondition(bounce == .float32)
        for encoding in AudioExportFormat.allCases {
            let bounce = try AudioExportFormat.resolve(payload: ["encoding": encoding.rawValue], mode: "track")
            precondition(bounce == .float32 && bounce.bitDepth == 32 && bounce.isFloat)
        }
        for value: Any in [true, false, NSNull(), 16, 24.0, "", "pcm32", "PCM24", " pcm24", "pcm24 ", ["pcm24"], ["value": "pcm24"]] {
            reject(["encoding": value]); reject(["encoding": value], mode: "track")
        }
        for value: Any in [true, false, NSNull(), "48000", "44100", "96000", 22_050, 88_200, 192_000, 0, -48_000, 44_100.25, 48_000.5, 96_000.5, Double.nan, Double.infinity, -Double.infinity, NSDecimalNumber(string: "48000.0000000000000000001"), [48000], ["rate": 48000]] {
            for mode in ["song", "pattern", "stems", "track"] {
                reject(["sampleRate": value], mode: mode)
                do {
                    _ = try AudioExportFormat.resolveSampleRate(payload: ["sampleRate": value], mode: mode)
                    preconditionFailure("Invalid standalone sample rate was accepted: \(value), \(mode)")
                } catch {}
            }
        }
        for value: Any in [true, 1, NSNull(), "WAV", "aiff", "", "wav "] { reject(["container": value]) }
        for mode in ["", "bounce", "Song", "stems "] {
            reject([:], mode: mode)
            do { _ = try AudioExportFormat.resolveSampleRate(payload: [:], mode: mode); preconditionFailure("Invalid mode accepted by rate policy") } catch {}
        }
        // JSON number and boolean bridge representations must remain distinct.
        let validJSON = try JSONSerialization.jsonObject(with: Data(#"{"encoding":"pcm16","sampleRate":48000}"#.utf8)) as! [String: Any]
        let parsed = try AudioExportFormat.resolve(payload: validJSON, mode: "song")
        precondition(parsed == .pcm16)
        let boolJSON = try JSONSerialization.jsonObject(with: Data(#"{"sampleRate":true}"#.utf8)) as! [String: Any]
        reject(boolJSON)
        for mode in ["song", "pattern", "stems", "track"] {
            for rate in [44_100, 48_000, 96_000] {
                let payload = try JSONSerialization.jsonObject(with: Data("{\"sampleRate\":\(rate)}".utf8)) as! [String: Any]
                let resolved = try AudioExportFormat.resolveSampleRate(payload: payload, mode: mode)
                precondition(resolved == (mode == "track" ? 48_000 : rate))
            }
        }
        let cap = AudioExportFormat.capabilities
        precondition(cap["encodings"] as? [String] == ["pcm16", "pcm24", "float32"])
        precondition(cap["defaultEncoding"] as? String == "pcm24")
        precondition(cap["sampleRate"] as? Double == 48_000)
        precondition(cap["sampleRates"] as? [Int] == [44_100, 48_000, 96_000])
        precondition(cap["defaultSampleRate"] as? Int == 48_000)
        print("PASS AudioExportFormat: 144 rate/encoding/mode/number cases, JSON bridge rates, legacy48k settings/metadata, fixed48k float bounce, additive capabilities and strict malformed payload rejection (no audio/file operations)")
    }
}
