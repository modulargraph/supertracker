import Foundation

// Maschine MK3 USB protocol: pure data transformations, no IOKit.
//
// Sources (protocol facts only): docs/hardware/MaschineMK3-*.md (cabl), the Encdr MK3 descriptor and
// reference, and the device's own HID report descriptor as read from this Mac's IORegistry:
//   0x01 in  42 bytes: 80 one-bit buttons (bytes 1–10), 2 × 4-bit (byte 11, low nibble = main encoder),
//                      8 × 16-bit knobs with logical range 0…999 (bytes 12–27), 3 × 16-bit (28–33, touch strip),
//                      1 × 12-bit (34–35), 3 × 12-bit analog knobs (36–41: mic, headphone, master).
//   0x02 in  64 bytes: pad tuples. 0x20 in (16 × 16-bit) and 0x40 in (15 × 16-bit) are not decoded.
//   0x80 out 63 bytes: 62 button LEDs, logical range 0…127.  0x81 out 42 bytes: 25 strip + 16 pad LEDs, 0…127.
// Everything that is not confirmed on real hardware is a named constant here so it can be flipped quickly.

enum MK3Protocol {
    static let vendorID = 0x17cc
    static let productID = 0x1600
    static let buttonReportID: UInt8 = 0x01
    static let padReportID: UInt8 = 0x02
    static let buttonReportLength = 42
    static let padSetLength = 64
    static let buttonLEDReportID: UInt8 = 0x80
    static let buttonLEDReportLength = 63
    static let padLEDReportID: UInt8 = 0x81
    static let padLEDReportLength = 42
    /// Report descriptor: knobs report 0…999 (one revolution ≈ 1000 counts).
    static let knobModulus = 1000
    /// Counts that make contract delta 1.0 ("full sweep of a parameter").
    static var knobCountsPerUnit = 1000.0
    /// Analog knobs are 12-bit per the report descriptor.
    static let analogFullScale = 4095.0
    /// Touch strip position (bytes 30–31) per Encdr; normalized by this maximum.
    static var stripFullScale = 1024.0
    static let stripPositionOffset = 30
}

// MARK: - Input events (contract `mk3.input`)

enum MK3PadPhase: String { case down, pressure, up }

enum MK3InputEvent: Equatable {
    case button(name: String, pressed: Bool)
    case pad(pad: Int, phase: MK3PadPhase, pressure: Double, velocity: Int?)
    case encoder(index: Int, delta: Double)
    case main(delta: Int)
    case touch(target: String, touched: Bool)
    case strip(value: Double)
    case knob(name: String, value: Double)
    case pedal(down: Bool)

    var payload: [String: Any] {
        switch self {
        case let .button(name, pressed): return ["type": "button", "name": name, "pressed": pressed]
        case let .pad(pad, phase, pressure, velocity):
            var value: [String: Any] = ["type": "pad", "pad": pad, "phase": phase.rawValue, "pressure": MK3InputEvent.round(pressure)]
            if let velocity { value["velocity"] = velocity }
            return value
        case let .encoder(index, delta): return ["type": "encoder", "index": index, "delta": MK3InputEvent.round(delta, places: 4)]
        case let .main(delta): return ["type": "main", "delta": delta]
        case let .touch(target, touched): return ["type": "touch", "target": target, "touched": touched]
        case let .strip(value): return ["type": "strip", "value": MK3InputEvent.round(value)]
        case let .knob(name, value): return ["type": "knob", "name": name, "value": MK3InputEvent.round(value)]
        case let .pedal(down): return ["type": "pedal", "down": down]
        }
    }

    private static func round(_ value: Double, places: Double = 3) -> Double {
        let scale = Foundation.pow(10, places)
        return (value * scale).rounded() / scale
    }
}

// MARK: - Input parser

struct MK3InputParser {
    /// (byte, bit, contract name). Byte 1 bit 1 is the pedal; byte 5 bit 0 is "microphone connected".
    static let buttonBits: [(Int, Int, String)] = {
        var bits: [(Int, Int, String)] = [
            (1, 0, "encoderPush"), (1, 2, "encoderUp"), (1, 3, "encoderRight"), (1, 4, "encoderDown"),
            (1, 5, "encoderLeft"), (1, 6, "shift"), (1, 7, "display8"),
            (3, 0, "notes"), (3, 1, "volume"), (3, 2, "swing"), (3, 3, "tempo"), (3, 4, "noteRepeat"), (3, 5, "lock"),
            (4, 0, "padMode"), (4, 1, "keyboard"), (4, 2, "chords"), (4, 3, "step"), (4, 4, "fixedVel"),
            (4, 5, "scene"), (4, 6, "pattern"), (4, 7, "events"),
            (5, 1, "variation"), (5, 2, "duplicate"), (5, 3, "select"), (5, 4, "solo"), (5, 5, "mute"),
            (5, 6, "pitch"), (5, 7, "mod"),
            (6, 0, "perform"), (6, 1, "restart"), (6, 2, "erase"), (6, 3, "tap"), (6, 4, "follow"),
            (6, 5, "play"), (6, 6, "rec"), (6, 7, "stop"),
            (7, 0, "macro"), (7, 1, "settings"), (7, 2, "arrowRight"), (7, 3, "sampling"), (7, 4, "mixer"), (7, 5, "plugin"),
            (8, 0, "channel"), (8, 1, "arranger"), (8, 2, "browser"), (8, 3, "arrowLeft"), (8, 4, "file"), (8, 5, "auto"),
        ]
        for group in 0..<8 { bits.append((2, group, "group" + String(UnicodeScalar(UInt8(65 + group))))) }
        for display in 0..<7 { bits.append((9, display, "display\(display + 1)")) }
        return bits
    }()
    static let pedalBit = (1, 1)
    static let mainTouchBit = (9, 7)
    /// Byte 10: bit 7 = encoder 1 … bit 0 = encoder 8.
    static func encoderTouchBit(_ index: Int) -> (Int, Int) { (10, 7 - index) }
    static let analogKnobs: [(Int, String)] = [(36, "mic"), (38, "headphone"), (40, "master")]

    struct PadState { var pressed = false; var pressure = 0; var strikeTime = -1.0; var reportedPressure = 0 }

    private(set) var buttons: [UInt8]?
    private var knobs: [Int?] = Array(repeating: nil, count: 8)
    private var mainPosition: Int?
    private var stripTouched = false
    private var stripValue: Int?
    private var analog: [Int?] = Array(repeating: nil, count: 3)
    private(set) var pads = Array(repeating: PadState(), count: 16)

    /// Pressure changes smaller than this (raw 0…4095) are not reported as "pressure" events.
    var pressureStep = 24
    /// Minimum analog-knob change (raw) before a knob event is emitted.
    var analogStep = 12
    /// Encoders that are not being touched need this many counts before deltas are reported (jitter filter).
    var untouchedKnobThreshold = 3

    /// Maschine pad number (1 = bottom-left) for a hardware index (0 = top-left, row-major).
    static func padNumber(hardwareIndex index: Int) -> Int { (3 - index / 4) * 4 + index % 4 + 1 }
    static func hardwareIndex(padNumber pad: Int) -> Int { (3 - (pad - 1) / 4) * 4 + (pad - 1) % 4 }

    /// Velocity 1…127 from a 12-bit hit pressure (gentle curve: light hits still register).
    static func velocity(raw: Int) -> Int {
        let normalized = min(1, max(0, Double(raw) / 4095))
        return max(1, min(127, Int((Foundation.pow(normalized, 0.6) * 127).rounded())))
    }

    /// Normalizes a delivered report: IOHID may or may not include the report ID byte.
    static func normalize(_ bytes: [UInt8], reportID: UInt32) -> [UInt8] {
        guard reportID != 0, reportID <= 0xff else { return bytes }
        let expected: Int
        switch reportID {
        case 0x01: expected = MK3Protocol.buttonReportLength
        case 0x02: expected = MK3Protocol.padSetLength
        default: expected = -1
        }
        if bytes.count == expected { return bytes }
        // A double-pumped pad delivery has multiple 64-byte sets. IOHID can
        // omit its leading ID just as for a single set; the first pad index
        // can itself be 0x02, so inspecting only that byte is ambiguous.
        if reportID == UInt32(MK3Protocol.padReportID),
           bytes.count >= MK3Protocol.padSetLength - 1,
           bytes.count % MK3Protocol.padSetLength == MK3Protocol.padSetLength - 1 {
            return [UInt8(reportID)] + bytes
        }
        if bytes.count == expected - 1 || bytes.first.map({ UInt32($0) != reportID }) ?? true { return [UInt8(reportID)] + bytes }
        return bytes
    }

    mutating func parse(_ report: [UInt8], now: TimeInterval) -> [MK3InputEvent] {
        guard let id = report.first else { return [] }
        switch id {
        case MK3Protocol.buttonReportID: return parseButtons(report)
        case MK3Protocol.padReportID: return parsePads(report, now: now)
        default: return []
        }
    }

    private static func bit(_ bytes: [UInt8], _ location: (Int, Int)) -> Bool {
        location.0 < bytes.count && bytes[location.0] & UInt8(1 << location.1) != 0
    }
    private static func u16(_ bytes: [UInt8], _ offset: Int) -> Int? {
        offset + 1 < bytes.count ? Int(bytes[offset]) | Int(bytes[offset + 1]) << 8 : nil
    }

    mutating func parseButtons(_ report: [UInt8]) -> [MK3InputEvent] {
        guard report.count >= 12 else { return [] }
        var events: [MK3InputEvent] = []
        let previous = buttons
        let bit = MK3InputParser.bit
        // Touch first, so a touch-and-turn in one report arrives in a useful order.
        func edge(_ location: (Int, Int)) -> Bool? {
            let now = bit(report, location)
            if let previous { return bit(previous, location) == now ? nil : now }
            return now ? true : nil
        }
        if let touched = edge(MK3InputParser.mainTouchBit) { events.append(.touch(target: "main", touched: touched)) }
        for index in 0..<8 {
            if let touched = edge(MK3InputParser.encoderTouchBit(index)) { events.append(.touch(target: "encoder\(index + 1)", touched: touched)) }
        }
        for (byte, bitIndex, name) in MK3InputParser.buttonBits {
            if let pressed = edge((byte, bitIndex)) { events.append(.button(name: name, pressed: pressed)) }
        }
        if let down = edge(MK3InputParser.pedalBit) { events.append(.pedal(down: down)) }
        buttons = Array(report.prefix(MK3Protocol.buttonReportLength))

        // Main encoder: 4-bit position, wraps 15 → 0.
        let position = Int(report[11] & 0x0f)
        if let mainPosition {
            let delta = ((position - mainPosition + 8) & 15) - 8
            if delta != 0 { events.append(.main(delta: delta)) }
        }
        mainPosition = position

        // Knobs: absolute 0…999, endless. Delta with wrap-around in either direction.
        for index in 0..<8 {
            guard let raw = MK3InputParser.u16(report, 12 + index * 2) else { continue }
            let value = raw % MK3Protocol.knobModulus
            guard let last = knobs[index] else { knobs[index] = value; continue }
            var delta = (value - last) % MK3Protocol.knobModulus
            if delta >= MK3Protocol.knobModulus / 2 { delta -= MK3Protocol.knobModulus }
            if delta < -MK3Protocol.knobModulus / 2 { delta += MK3Protocol.knobModulus }
            guard delta != 0 else { continue }
            let touched = bit(report, MK3InputParser.encoderTouchBit(index))
            if !touched && abs(delta) < untouchedKnobThreshold {
                // Keep the baseline so a slow untouched turn still accumulates.
                continue
            }
            knobs[index] = value
            events.append(.encoder(index: index, delta: Double(delta) / MK3Protocol.knobCountsPerUnit))
        }

        // Touch strip: position at bytes 30–31, zero when nobody touches it.
        if let raw = MK3InputParser.u16(report, MK3Protocol.stripPositionOffset) {
            let touched = raw != 0
            if touched != stripTouched {
                stripTouched = touched
                events.append(.touch(target: "strip", touched: touched))
            }
            if touched, raw != stripValue {
                events.append(.strip(value: min(1, max(0, Double(raw) / MK3Protocol.stripFullScale))))
            }
            stripValue = touched ? raw : nil
        }

        // Analog knobs (audio interface): 12-bit absolute.
        for (slot, (offset, name)) in MK3InputParser.analogKnobs.enumerated() {
            guard let raw = MK3InputParser.u16(report, offset) else { continue }
            let value = raw & 0x0fff
            if let last = analog[slot], abs(last - value) < analogStep, value != 0, value != 0x0fff { continue }
            if analog[slot] == value { continue }
            analog[slot] = value
            events.append(.knob(name: name, value: Double(value) / MK3Protocol.analogFullScale))
        }
        return events
    }

    /// Report 0x02: one or more 64-byte sets ("double-pumped"), each `02` + 3-byte tuples ending in 00 00 00.
    mutating func parsePads(_ report: [UInt8], now: TimeInterval) -> [MK3InputEvent] {
        var events: [MK3InputEvent] = []
        var start = 0
        while start < report.count {
            let end = min(report.count, start + MK3Protocol.padSetLength)
            if report[start] == MK3Protocol.padReportID { decodeSet(report[start..<end], now: now, into: &events) }
            start = end
        }
        // Coalesce: keep only the latest pressure event per pad within one report.
        var seenPressure = Set<Int>()
        var result: [MK3InputEvent] = []
        for event in events.reversed() {
            if case let .pad(pad, .pressure, _, _) = event {
                if seenPressure.contains(pad) { continue }
                seenPressure.insert(pad)
            }
            result.append(event)
        }
        return result.reversed()
    }

    private mutating func decodeSet(_ set: ArraySlice<UInt8>, now: TimeInterval, into events: inout [MK3InputEvent]) {
        var offset = set.startIndex + 1
        while offset + 2 < set.endIndex {
            let index = Int(set[offset]), d1 = set[offset + 1], d2 = set[offset + 2]
            offset += 3
            // End of list only when all three bytes are zero: pad index 0 can send d1 == 0 at low pressure.
            if index == 0 && d1 == 0 && d2 == 0 { break }
            guard index < 16 else { continue }
            let raw = Int(d1 & 0x0f) << 8 | Int(d2)
            apply(hardwareIndex: index, tag: d1 & 0xf0, raw: raw, now: now, into: &events)
        }
    }

    private mutating func apply(hardwareIndex: Int, tag: UInt8, raw: Int, now: TimeInterval, into events: inout [MK3InputEvent]) {
        let pad = MK3InputParser.padNumber(hardwareIndex: hardwareIndex)
        var state = pads[hardwareIndex]
        func press() {
            state.pressed = true; state.pressure = raw; state.reportedPressure = raw; state.strikeTime = now
            events.append(.pad(pad: pad, phase: .down, pressure: Double(raw) / 4095, velocity: MK3InputParser.velocity(raw: raw)))
        }
        func release() {
            state.pressed = false; state.pressure = 0; state.reportedPressure = 0
            events.append(.pad(pad: pad, phase: .up, pressure: 0, velocity: nil))
        }
        func update() {
            state.pressure = raw
            if abs(raw - state.reportedPressure) >= pressureStep {
                state.reportedPressure = raw
                events.append(.pad(pad: pad, phase: .pressure, pressure: Double(raw) / 4095, velocity: nil))
            }
        }
        switch tag {
        case 0x10: // hit on: always pressed
            if state.pressed { update() } else { press() }
        case 0x20, 0x30: // explicit off
            if state.pressed { release() }
        case 0x00, 0x40: // switch on / aftertouch: hysteresis, rebound guard after a strike
            if !state.pressed {
                if raw >= 32 { press() }
            } else if raw <= 16 {
                if now - state.strikeTime >= 0.030 { release() } else { state.pressure = raw }
            } else { update() }
        default:
            if !state.pressed { if raw >= 64 { press() } }
            else if raw <= 48 { release() } else { update() }
        }
        pads[hardwareIndex] = state
    }

    /// Releases every held pad and button (device disconnect).
    mutating func releaseAll() -> [MK3InputEvent] {
        var events: [MK3InputEvent] = []
        for index in 0..<16 where pads[index].pressed {
            pads[index] = PadState()
            events.append(.pad(pad: MK3InputParser.padNumber(hardwareIndex: index), phase: .up, pressure: 0, velocity: nil))
        }
        if let buttons {
            if MK3InputParser.bit(buttons, MK3InputParser.pedalBit) { events.append(.pedal(down: false)) }
            if MK3InputParser.bit(buttons, MK3InputParser.mainTouchBit) { events.append(.touch(target: "main", touched: false)) }
            for index in 0..<8 where MK3InputParser.bit(buttons, MK3InputParser.encoderTouchBit(index)) {
                events.append(.touch(target: "encoder\(index + 1)", touched: false))
            }
            for (byte, bitIndex, name) in MK3InputParser.buttonBits where MK3InputParser.bit(buttons, (byte, bitIndex)) {
                events.append(.button(name: name, pressed: false))
            }
        }
        if stripTouched { events.append(.touch(target: "strip", touched: false)) }
        var fresh = MK3InputParser()
        fresh.pressureStep = pressureStep; fresh.analogStep = analogStep; fresh.untouchedKnobThreshold = untouchedKnobThreshold
        self = fresh
        return events
    }
}

// MARK: - Colours and LEDs

struct MK3RGB: Equatable, Sendable {
    var r: UInt8, g: UInt8, b: UInt8

    /// "#rrggbb", "#rgb" or "#rrggbbaa" (alpha ignored for LEDs).
    init?(hex value: Any?) {
        guard var text = value as? String else { return nil }
        text = text.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("#") { text.removeFirst() }
        if text.count == 3 { text = text.map { "\($0)\($0)" }.joined() }
        guard text.count == 6 || text.count == 8, let number = UInt32(text.prefix(6), radix: 16) else { return nil }
        r = UInt8(number >> 16 & 0xff); g = UInt8(number >> 8 & 0xff); b = UInt8(number & 0xff)
    }
    init(r: UInt8, g: UInt8, b: UInt8) { self.r = r; self.g = g; self.b = b }
}

enum MK3LEDCode {
    /// NI indexed colour: 1…16 hues (red → fuchsia), 17 white. Hue in degrees for each index.
    static let paletteHues: [Double] = [0, 15, 30, 45, 60, 90, 120, 150, 180, 210, 240, 270, 300, 315, 330, 345]
    static let white = 17
    /// Below this value (max channel) the LED is off. Dim markers (≈ 6 %) must still light.
    static var offBelow = 0.03
    /// Colours less saturated than this light white. SuperTracker's track palette is muted (saturation
    /// 0.07–0.4); a higher threshold turned most groups and pads white instead of their hue.
    static var whiteBelowSaturation = 0.05

    /// Pad / group / nav LED byte: (palette << 2) | intensity, 0 = off.
    static func indexed(_ color: MK3RGB?) -> UInt8 {
        guard let color else { return 0 }
        let r = Double(color.r) / 255, g = Double(color.g) / 255, b = Double(color.b) / 255
        let maximum = max(r, g, b), minimum = min(r, g, b)
        let value = maximum
        guard value >= offBelow else { return 0 }
        let saturation = maximum == 0 ? 0 : (maximum - minimum) / maximum
        let intensity: UInt8 = value < 0.3 ? 0 : value < 0.55 ? 1 : value < 0.8 ? 2 : 3
        guard saturation >= whiteBelowSaturation else { return UInt8(white << 2) | intensity }
        var hue: Double
        let delta = maximum - minimum
        if maximum == r { hue = 60 * ((g - b) / delta).truncatingRemainder(dividingBy: 6) }
        else if maximum == g { hue = 60 * ((b - r) / delta + 2) }
        else { hue = 60 * ((r - g) / delta + 4) }
        if hue < 0 { hue += 360 }
        var best = 0, bestDistance = Double.infinity
        for (index, candidate) in paletteHues.enumerated() {
            let distance = min(abs(hue - candidate), 360 - abs(hue - candidate))
            if distance < bestDistance { bestDistance = distance; best = index }
        }
        return UInt8((best + 1) << 2) | intensity
    }

    /// Single-colour backlight byte for brightness 0…1. These values are chosen so that both competing
    /// readings of the protocol give a sensible ramp: as a 7-bit level (0…127, the descriptor's logical
    /// range) they are ≈ 9 %, 26 %, 52 %, 100 %; read as NI's 2-bit index (byte & 3) they are 0, 1, 2, 3.
    static let singleLevels: [UInt8] = [0x0C, 0x21, 0x42, 0x7F]
    static func single(_ level: Double) -> UInt8 {
        guard level > 0.001 else { return 0 }
        let step = level < 0.3 ? 0 : level < 0.55 ? 1 : level < 0.8 ? 2 : 3
        return singleLevels[step]
    }
}

struct MK3LEDState: Equatable {
    /// Byte index inside report 0x80 (byte 0 = report ID). JSON descriptor offset + 1.
    static let buttonOffsets: [String: Int] = {
        var map: [String: Int] = [
            "channel": 1, "plugin": 2, "arranger": 3, "mixer": 4, "browser": 5, "sampling": 6,
            "arrowLeft": 7, "arrowRight": 8, "file": 9, "settings": 10, "auto": 11, "macro": 12,
            "volume": 21, "swing": 22, "noteRepeat": 23, "tempo": 24, "lock": 25, "pitch": 26, "mod": 27,
            "perform": 28, "notes": 29,
            "restart": 38, "erase": 39, "tap": 40, "follow": 41, "play": 42, "rec": 43, "stop": 44, "shift": 45,
            "fixedVel": 46, "padMode": 47, "keyboard": 48, "chords": 49, "step": 50, "scene": 51, "pattern": 52,
            "events": 53, "variation": 54, "duplicate": 55, "select": 56, "solo": 57, "mute": 58,
        ]
        for display in 1...8 { map["display\(display)"] = 12 + display }
        return map
    }()
    static let groupOffset = 30
    static let navOffsets: [String: Int] = ["up": 59, "left": 60, "right": 61, "down": 62]
    static let stripOffset = 1
    static let padOffset = 26

    var buttons: [String: Double] = [:]
    var groups: [MK3RGB?] = Array(repeating: nil, count: 8)
    var pads: [MK3RGB?] = Array(repeating: nil, count: 16)
    /// Strip LEDs: brightness (white) or an explicit colour.
    var strip: [MK3RGB?] = Array(repeating: nil, count: 25)
    var nav: [String: MK3RGB] = [:]

    static func gray(_ level: Double) -> MK3RGB? {
        guard level > 0.001 else { return nil }
        let value = UInt8(max(0, min(255, (level * 255).rounded())))
        return MK3RGB(r: value, g: value, b: value)
    }

    /// Applies a partial `mk3.leds` payload.
    mutating func apply(_ payload: [String: Any]) {
        if let values = payload["buttons"] as? [String: Any] {
            for (name, value) in values where MK3LEDState.buttonOffsets[name] != nil {
                buttons[name] = max(0, min(1, PayloadValue.double(value) ?? 0))
            }
        }
        if let values = payload["groups"] as? [Any] {
            for (index, value) in values.prefix(8).enumerated() { groups[index] = MK3RGB(hex: value) }
        }
        if let values = payload["pads"] as? [Any] {
            for (index, value) in values.prefix(16).enumerated() { pads[index] = MK3RGB(hex: value) }
        }
        if let values = payload["strip"] as? [Any] {
            for (index, value) in values.prefix(25).enumerated() {
                strip[index] = MK3RGB(hex: value) ?? PayloadValue.double(value).flatMap(MK3LEDState.gray)
            }
        }
        if let values = payload["nav"] as? [String: Any] {
            for (name, value) in values where MK3LEDState.navOffsets[name] != nil { nav[name] = MK3RGB(hex: value) }
        }
    }

    func buttonReport() -> [UInt8] {
        var report = [UInt8](repeating: 0, count: MK3Protocol.buttonLEDReportLength)
        report[0] = MK3Protocol.buttonLEDReportID
        for (name, level) in buttons { if let offset = MK3LEDState.buttonOffsets[name] { report[offset] = MK3LEDCode.single(level) } }
        for index in 0..<8 { report[MK3LEDState.groupOffset + index] = MK3LEDCode.indexed(groups[index]) }
        for (name, offset) in MK3LEDState.navOffsets { report[offset] = MK3LEDCode.indexed(nav[name]) }
        return report
    }

    func padReport() -> [UInt8] {
        var report = [UInt8](repeating: 0, count: MK3Protocol.padLEDReportLength)
        report[0] = MK3Protocol.padLEDReportID
        for index in 0..<25 { report[MK3LEDState.stripOffset + index] = MK3LEDCode.indexed(strip[index]) }
        for pad in 1...16 {
            report[MK3LEDState.padOffset + MK3InputParser.hardwareIndex(padNumber: pad)] = MK3LEDCode.indexed(pads[pad - 1])
        }
        return report
    }
}
