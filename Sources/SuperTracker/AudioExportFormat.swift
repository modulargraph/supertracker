import Foundation
import CoreFoundation
import AVFoundation

/// File encoding policy only. Rendering and the engine's internal buffers remain Float32 at 48 kHz.
/// Validate this before preparing the graph or creating export files.
enum AudioExportFormat: String, CaseIterable {
    case pcm16, pcm24, float32

    static let defaultSampleRate = 48_000
    static let supportedSampleRates = [44_100, 48_000, 96_000]
    static let sampleRate = Double(defaultSampleRate)
    static let defaultEncoding = AudioExportFormat.pcm24

    var bitDepth: Int { self == .pcm16 ? 16 : self == .pcm24 ? 24 : 32 }
    var isFloat: Bool { self == .float32 }

    var settings: [String: Any] {
        [AVFormatIDKey: kAudioFormatLinearPCM,
         AVSampleRateKey: Self.sampleRate,
         AVNumberOfChannelsKey: 2,
         AVLinearPCMBitDepthKey: bitDepth,
         AVLinearPCMIsFloatKey: isFloat,
         AVLinearPCMIsBigEndianKey: false]
    }

    var metadata: [String: Any] {
        ["container": "wav", "encoding": rawValue, "sampleRate": Self.sampleRate,
         "bitDepth": bitDepth, "isFloat": isFloat]
    }

    /// Advertise only after both legacy and routed export writers use this policy.
    static var capabilities: [String: Any] {
        ["container": "wav", "sampleRate": sampleRate,
         "sampleRates": supportedSampleRates, "defaultSampleRate": defaultSampleRate,
         "encodings": allCases.map(\.rawValue), "defaultEncoding": defaultEncoding.rawValue]
    }

    static func resolve(payload: [String: Any], mode: String) throws -> AudioExportFormat {
        guard ["song", "pattern", "stems", "track"].contains(mode) else {
            throw ValidationError("Choose song, pattern, stems, or track export.")
        }
        if let container = payload["container"] {
            guard let container = container as? String, container == "wav" else {
                throw ValidationError("Audio export supports WAV files only.")
            }
        }
        _ = try resolveSampleRate(payload: payload, mode: mode)
        let requested: AudioExportFormat
        if let encoding = payload["encoding"] {
            guard let name = encoding as? String, let encoding = Self(rawValue: name) else {
                throw ValidationError("Choose PCM 16-bit, PCM 24-bit, or 32-bit float WAV encoding.")
            }
            requested = encoding
        } else {
            requested = defaultEncoding
        }
        // Bounce is an internal editable print, independent of the delivery export selector.
        // A supplied known encoding is validated above but cannot reduce bounce precision.
        return mode == "track" ? .float32 : requested
    }

    /// Delivery rate is independent of encoding. Legacy settings/metadata above
    /// still describe the engine's 48 kHz intermediate; conversion uses this result.
    static func resolveSampleRate(payload: [String: Any], mode: String) throws -> Int {
        guard ["song", "pattern", "stems", "track"].contains(mode) else {
            throw ValidationError("Choose song, pattern, stems, or track export.")
        }
        let requested: Int
        if let rate = payload["sampleRate"] {
            guard let number = rate as? NSNumber,
                  CFGetTypeID(number) != CFBooleanGetTypeID(),
                  number.doubleValue.isFinite,
                  let supported = supportedSampleRates.first(where: {
                      number.decimalValue == Decimal($0)
                  }) else {
                throw ValidationError("Choose a 44.1, 48, or 96 kHz WAV delivery rate.")
            }
            requested = supported
        } else {
            requested = defaultSampleRate
        }
        // Known delivery values are accepted but never resample an internal bounce.
        return mode == "track" ? defaultSampleRate : requested
    }

    struct ValidationError: LocalizedError {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }
}
