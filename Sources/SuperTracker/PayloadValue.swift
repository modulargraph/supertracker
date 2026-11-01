import Foundation

/// Values read from bridge payloads and saved project JSON (`[String: Any]` from
/// JSONSerialization or the web view). Strict readers return nil for anything but the
/// exact JSON type so callers can phrase their own errors; lenient readers fall back.
enum PayloadValue {
    static func isBoolean(_ number: NSNumber) -> Bool { CFGetTypeID(number) == CFBooleanGetTypeID() }

    /// A JSON number, never `true`/`false`. The value may be non-finite.
    static func numeric(_ value: Any?) -> Double? {
        guard let number = value as? NSNumber, !isBoolean(number) else { return nil }
        return number.doubleValue
    }

    /// A finite JSON number inside `range`.
    static func number(_ value: Any?, in range: ClosedRange<Double>) -> Double? {
        guard let result = numeric(value), result.isFinite, range.contains(result) else { return nil }
        return result
    }

    /// Whether `value` is a whole number that converts to Int64 exactly.
    static func isWhole(_ value: Double) -> Bool {
        value.isFinite && value.rounded(.towardZero) == value && value >= -9_223_372_036_854_775_808 && value < 9_223_372_036_854_775_808
    }

    /// JSON `true` or `false` only.
    static func boolean(_ value: Any?) -> Bool? {
        guard let number = value as? NSNumber, isBoolean(number) else { return nil }
        return number.boolValue
    }

    /// Any number, Booleans read as 0 or 1 (controller and screen descriptions).
    static func double(_ value: Any?) -> Double? { (value as? NSNumber)?.doubleValue }

    /// Lenient project field: any number (Booleans read as 0 or 1); missing or non-finite values fall back.
    static func number(_ object: [String: Any], _ key: String, _ fallback: Double) -> Double {
        guard let result = (object[key] as? NSNumber)?.doubleValue, result.isFinite else { return fallback }
        return result
    }

    /// Lenient project field: any number, truncated toward zero.
    static func integer(_ object: [String: Any], _ key: String, _ fallback: Int) -> Int {
        (object[key] as? NSNumber)?.intValue ?? fallback
    }

    /// An integer from a number or decimal text (settings and older records store either).
    static func integer(_ value: Any?) -> Int? {
        if let number = value as? NSNumber { return number.intValue }
        if let text = value as? String { return Int(text) }
        return nil
    }

    static func integer(_ value: Any?, _ fallback: Int) -> Int { integer(value) ?? fallback }

    /// Text with surrounding whitespace removed; nil when missing or empty.
    static func text(_ value: Any?) -> String? {
        guard let text = value as? String else { return nil }
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
