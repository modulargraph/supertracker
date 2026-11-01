import Foundation
import CoreFoundation

/// Bounded AU property-list encoding reuse. Main/control-thread use only; never render-thread work.
/// The fresh read/serialization closure runs for EVERY request, including hits.
final class AudioAUStateReuse {
    struct Limits {
        var maximumEntries = 320
        var maximumRetainedBytes = 32 * 1024 * 1024
        var maximumEntryBytes = 8 * 1024 * 1024
        var maximumDepth = 64
        var maximumNodes = 100_000
    }
    struct Result { let data: Data; let reused: Bool; let retained: Bool }
    struct Statistics { let entries: Int; let retainedBytes: Int }
    private struct Entry { let encoded: Data; let semantic: Data; let cost: Int; var use: UInt64 }
    private let limits: Limits
    private var entries: [ObjectIdentifier: Entry] = [:]
    private var retainedBytes = 0
    private var clock: UInt64 = 0
    // Explicit allowance for key/LRU/dictionary metadata, in addition to both
    // exact byte buffers. Entry count independently bounds allocator overhead.
    private static let entryAllowance = 256

    init(limits: Limits = Limits()) { self.limits = limits }
    var statistics: Statistics { Statistics(entries: entries.count, retainedBytes: retainedBytes) }

    func capture(for identity: ObjectIdentifier, readFreshEncoded: () throws -> Data) rethrows -> Result {
        let fresh = try readFreshEncoded() // Deliberately before every cache lookup.
        return accept(fresh, for: identity)
    }

    /// Optional: seed the saved encoding only AFTER restoration succeeds. The
    /// next capture still reads fresh state and must prove typed equivalence.
    @discardableResult func seed(_ savedEncoding: Data, for identity: ObjectIdentifier) -> Bool {
        remove(identity)
        return accept(savedEncoding, for: identity).retained
    }

    func remove(_ identity: ObjectIdentifier) {
        if let old = entries.removeValue(forKey: identity) { retainedBytes -= old.cost }
    }
    func removeAll() { entries.removeAll(keepingCapacity: false); retainedBytes = 0; clock = 0 }

    private func accept(_ fresh: Data, for identity: ObjectIdentifier) -> Result {
        let entryBudget = min(limits.maximumEntryBytes, limits.maximumRetainedBytes)
        guard limits.maximumEntries > 0, entryBudget > Self.entryAllowance, fresh.count <= entryBudget - Self.entryAllowance,
              let semantic = Self.semanticBytes(fresh, maximumBytes: entryBudget - Self.entryAllowance - fresh.count,
                                                maximumDepth: limits.maximumDepth, maximumNodes: limits.maximumNodes) else {
            // Oversized, malformed, nonfinite or overly complex states remain
            // fresh. A cache must never convert a failed comparison to old state.
            remove(identity)
            return Result(data: fresh, reused: false, retained: false)
        }
        tick()
        if var old = entries[identity], old.semantic == semantic {
            old.use = clock; entries[identity] = old
            return Result(data: old.encoded, reused: true, retained: true)
        }
        remove(identity)
        let cost = fresh.count + semantic.count + Self.entryAllowance
        while entries.count >= limits.maximumEntries || retainedBytes + cost > limits.maximumRetainedBytes {
            guard let least = entries.min(by: { $0.value.use < $1.value.use })?.key else { break }
            remove(least)
        }
        // Copy exactly the retained payload, so slices/mutable Foundation backing
        // stores cannot keep an unrelated large buffer alive or mutate the cache.
        let encodedCopy = fresh.withUnsafeBytes { Data(bytes: $0.baseAddress!, count: $0.count) }
        let semanticCopy = semantic.withUnsafeBytes { Data(bytes: $0.baseAddress!, count: $0.count) }
        entries[identity] = Entry(encoded: encodedCopy, semantic: semanticCopy, cost: cost, use: clock)
        retainedBytes += cost
        return Result(data: encodedCopy, reused: false, retained: true)
    }

    private func tick() {
        if clock == UInt64.max {
            for (index, key) in entries.sorted(by: { $0.value.use < $1.value.use }).map(\.key).enumerated() {
                entries[key]!.use = UInt64(index + 1)
            }
            clock = UInt64(entries.count)
        }
        clock += 1
    }

    /// Equality concerns the serialized property-list meaning, not NSNumber's
    /// coercive equality or Swift String's Unicode-normalized equality.
    static func equivalent(_ lhs: Data, _ rhs: Data, limits: Limits = Limits()) -> Bool {
        guard lhs.count <= limits.maximumEntryBytes, rhs.count <= limits.maximumEntryBytes,
              let first = semanticBytes(lhs, maximumBytes: limits.maximumEntryBytes,
                                        maximumDepth: limits.maximumDepth, maximumNodes: limits.maximumNodes),
              let second = semanticBytes(rhs, maximumBytes: limits.maximumEntryBytes,
                                         maximumDepth: limits.maximumDepth, maximumNodes: limits.maximumNodes) else { return false }
        return first == second
    }

    private static func semanticBytes(_ data: Data, maximumBytes: Int, maximumDepth: Int, maximumNodes: Int) -> Data? {
        guard maximumBytes > 0, maximumDepth >= 0, maximumNodes > 0,
              let plist = try? PropertyListSerialization.propertyList(from: data, options: [], format: nil) else { return nil }
        var writer = SemanticWriter(maximumBytes: maximumBytes, maximumDepth: maximumDepth, maximumNodes: maximumNodes)
        do { try writer.value(plist, depth: 0); return writer.bytes } catch { return nil }
    }
}

private struct SemanticWriter {
    enum Failure: Error { case unsupported, limit }
    var bytes = Data()
    let maximumBytes: Int
    let maximumDepth: Int
    let maximumNodes: Int
    var nodes = 0

    mutating func append(_ value: UInt8) throws {
        guard bytes.count < maximumBytes else { throw Failure.limit }; bytes.append(value)
    }
    mutating func append(_ data: Data) throws {
        guard data.count <= maximumBytes - bytes.count else { throw Failure.limit }; bytes.append(data)
    }
    mutating func integer(_ value: UInt64) throws {
        var big = value.bigEndian
        try withUnsafeBytes(of: &big) { try append(Data($0)) }
    }
    func codeUnits(_ text: NSString) throws -> [UInt16] {
        guard text.length <= maximumBytes / 2 else { throw Failure.limit }
        var units = [UInt16](repeating: 0, count: text.length)
        if !units.isEmpty { text.getCharacters(&units, range: NSRange(location: 0, length: units.count)) }
        return units
    }
    mutating func string(_ units: [UInt16]) throws {
        try integer(UInt64(units.count))
        guard units.count <= (maximumBytes - bytes.count) / 2 else { throw Failure.limit }
        for unit in units { try append(UInt8(unit >> 8)); try append(UInt8(unit & 255)) }
    }
    mutating func value(_ value: Any, depth: Int) throws {
        nodes += 1
        guard depth <= maximumDepth, nodes <= maximumNodes else { throw Failure.limit }
        let object = value as AnyObject
        let type = CFGetTypeID(object)
        if type == CFBooleanGetTypeID(), let number = object as? NSNumber {
            try append(1); try append(number.boolValue ? 1 : 0)
        } else if type == CFNumberGetTypeID(), let number = object as? NSNumber {
            if CFNumberIsFloatType(unsafeBitCast(number, to: CFNumber.self)) {
                // Conservative for NaN/infinity: preserve the fresh encoding,
                // without making a claim about equality or payload semantics.
                guard number.doubleValue.isFinite else { throw Failure.unsupported }
                try append(3)
                // Preserve even the decoded real storage type: vendors can
                // observe CFNumberGetType after the property list is restored.
                try integer(UInt64(CFNumberGetType(unsafeBitCast(number, to: CFNumber.self)).rawValue))
                try integer(number.doubleValue.bitPattern)
            } else {
                // Preserve the entire signed/unsigned range without NSNumber's
                // coercive equality, text formatting, or conversion via Double.
                try append(2)
                if String(cString: number.objCType) == "Q" {
                    try append(0); try integer(number.uint64Value)
                } else {
                    var signed: Int64 = 0
                    guard CFNumberGetValue(unsafeBitCast(number, to: CFNumber.self), .sInt64Type, &signed) else { throw Failure.unsupported }
                    try append(signed < 0 ? 1 : 0)
                    try integer(signed < 0 ? UInt64(-(signed + 1)) + 1 : UInt64(signed))
                }
            }
        } else if type == CFStringGetTypeID(), let text = object as? NSString {
            try append(4); try string(codeUnits(text))
        } else if type == CFDataGetTypeID(), let data = object as? NSData {
            try append(5); try integer(UInt64(data.length))
            guard data.length <= maximumBytes - bytes.count else { throw Failure.limit }
            if data.length > 0 { bytes.append(data.bytes.assumingMemoryBound(to: UInt8.self), count: data.length) }
        } else if type == CFDateGetTypeID(), let date = object as? NSDate {
            let seconds = date.timeIntervalSinceReferenceDate
            guard seconds.isFinite else { throw Failure.unsupported }
            try append(6); try integer(seconds.bitPattern)
        } else if type == CFArrayGetTypeID(), let array = object as? NSArray {
            guard array.count <= maximumNodes - nodes else { throw Failure.limit }
            try append(7); try integer(UInt64(array.count))
            for item in array { try self.value(item, depth: depth + 1) }
        } else if type == CFDictionaryGetTypeID(), let dictionary = object as? NSDictionary {
            guard dictionary.count <= (maximumNodes - nodes) / 2 else { throw Failure.limit }
            var ordered: [(key: [UInt16], value: Any)] = []
            var keyBytes = 0
            for rawKey in dictionary.allKeys {
                guard CFGetTypeID(rawKey as AnyObject) == CFStringGetTypeID(), let key = rawKey as? NSString,
                      let item = dictionary.object(forKey: key) else { throw Failure.unsupported }
                let units = try codeUnits(key)
                keyBytes += units.count * 2
                guard keyBytes <= maximumBytes else { throw Failure.limit }
                ordered.append((units, item))
            }
            ordered.sort { $0.key.lexicographicallyPrecedes($1.key) }
            try append(8); try integer(UInt64(ordered.count))
            for entry in ordered {
                nodes += 1; try string(entry.key); try self.value(entry.value, depth: depth + 1)
            }
        } else { throw Failure.unsupported }
    }
}
