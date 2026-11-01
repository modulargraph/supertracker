// Pure Maschine MK3 protocol tests: synthetic reports, no device.
// swiftc -O -swift-version 5 Sources/SuperTracker/MaschineMK3Protocol.swift Sources/SuperTracker/MaschineMK3Screen.swift \
//   tests/MaschineMK3ProtocolTests.swift -o /tmp/mk3-tests && /tmp/mk3-tests
import Foundation
import CoreGraphics

@main enum MaschineMK3ProtocolTests {
    static func check(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        if !condition() { throw NSError(domain: "MaschineMK3ProtocolTests: " + message, code: 1) }
    }

    static func buttonReport(_ edit: (inout [UInt8]) -> Void = { _ in }) -> [UInt8] {
        var report = [UInt8](repeating: 0, count: 42); report[0] = 0x01; edit(&report); return report
    }
    static func setKnob(_ report: inout [UInt8], _ index: Int, _ value: Int) {
        report[12 + index * 2] = UInt8(value & 0xff); report[13 + index * 2] = UInt8(value >> 8)
    }
    static func padSet(_ tuples: [(UInt8, UInt8, UInt8)]) -> [UInt8] {
        var set = [UInt8](repeating: 0, count: 64); set[0] = 0x02
        for (i, t) in tuples.enumerated() { set[1 + i * 3] = t.0; set[2 + i * 3] = t.1; set[3 + i * 3] = t.2 }
        return set
    }
    static func pads(_ events: [MK3InputEvent]) -> [(Int, MK3PadPhase)] {
        events.compactMap { if case let .pad(pad, phase, _, _) = $0 { return (pad, phase) }; return nil }
    }

    static func main() throws {
        do {
            var parser = MK3InputParser()
            _ = parser.parse(buttonReport { $0[1] = 0x02; $0[9] = 0x80; $0[10] = 0x81; $0[31] = 0x02 }, now: 0)
            let released = parser.releaseAll()
            for event: MK3InputEvent in [.pedal(down: false), .touch(target: "main", touched: false), .touch(target: "encoder1", touched: false), .touch(target: "encoder8", touched: false), .touch(target: "strip", touched: false)] {
                try check(released.contains(event), "disconnect releases \(event)")
            }
            try check(parser.releaseAll().isEmpty, "disconnect release is idempotent")
            let fresh = parser.parse(buttonReport { $0[11] = 11; setKnob(&$0, 0, 800) }, now: 1)
            try check(!fresh.contains { if case .encoder = $0 { return true }; if case .main = $0 { return true }; return false }, "reconnect establishes fresh encoder baselines")
            print("PASS disconnect releases pedal and all touch sensors, reconnect baselines")
        }
        do {
            var parser = MK3InputParser()
            let first = parser.parse(buttonReport(), now: 0)
            try check(first.allSatisfy { if case .knob = $0 { return true }; return false } && first.count == 3, "idle first report only reports analog knob positions: \(first)")
            var events = parser.parse(buttonReport { $0[6] = 0x20; $0[1] = 0x40 }, now: 0.01)
            try check(events.contains(.button(name: "play", pressed: true)), "play down")
            try check(events.contains(.button(name: "shift", pressed: true)), "shift down")
            events = parser.parse(buttonReport { $0[1] = 0x40 }, now: 0.02)
            try check(events == [.button(name: "play", pressed: false)], "play up only: \(events)")
            events = parser.parse(buttonReport { $0[1] = 0x80 | 0x01; $0[2] = 0x81; $0[8] = 0x01; $0[9] = 0x01; $0[7] = 0x04 }, now: 0.03)
            for name in ["shift"] { try check(events.contains(.button(name: name, pressed: false)), "\(name) up") }
            for name in ["display8", "encoderPush", "groupA", "groupH", "channel", "display1", "arrowRight"] {
                try check(events.contains(.button(name: name, pressed: true)), "\(name) down")
            }
            events = parser.parse(buttonReport { $0[9] = 0x80; $0[10] = 0x81 }, now: 0.04)
            try check(events.contains(.touch(target: "main", touched: true)), "main touch")
            try check(events.contains(.touch(target: "encoder1", touched: true)), "encoder 1 touch (bit 7)")
            try check(events.contains(.touch(target: "encoder8", touched: true)), "encoder 8 touch (bit 0)")
            let contract = "channel plugin arranger mixer browser sampling arrowLeft arrowRight file settings auto macro display1 display2 display3 display4 display5 display6 display7 display8 notes volume swing tempo noteRepeat lock pitch mod perform padMode keyboard chords step fixedVel scene pattern events variation duplicate select solo mute groupA groupB groupC groupD groupE groupF groupG groupH restart erase tap follow play rec stop shift encoderPush encoderUp encoderDown encoderLeft encoderRight"
            let decoded = Set(MK3InputParser.buttonBits.map { $0.2 })
            try check(decoded == Set(contract.split(separator: " ").map(String.init)) && MK3InputParser.buttonBits.count == 63, "all 63 contract buttons decoded once")
            let locations = MK3InputParser.buttonBits.map { "\($0.0).\($0.1)" }
            try check(Set(locations).count == locations.count, "no two buttons share a bit")
            print("PASS buttons, edges and touch sensors")
        }
        do {
            var parser = MK3InputParser()
            _ = parser.parse(buttonReport { $0[11] = 14; $0[10] = 0xff; setKnob(&$0, 0, 990); setKnob(&$0, 1, 500) }, now: 0)
            var events = parser.parse(buttonReport { $0[11] = 1; $0[10] = 0xff; setKnob(&$0, 0, 10); setKnob(&$0, 1, 480) }, now: 0.01)
            try check(events.contains(.main(delta: 3)), "main encoder wraps 14 → 1 as +3: \(events)")
            try check(events.contains(.encoder(index: 0, delta: 20.0 / 1000)), "knob wraps 990 → 10 as +20 counts")
            try check(events.contains(.encoder(index: 1, delta: -20.0 / 1000)), "knob 2 turns down")
            events = parser.parse(buttonReport { $0[11] = 15; $0[10] = 0xff; setKnob(&$0, 0, 995); setKnob(&$0, 1, 480) }, now: 0.02)
            try check(events.contains(.main(delta: -2)), "main encoder wraps 1 → 15 as -2")
            try check(events.contains(.encoder(index: 0, delta: -15.0 / 1000)), "knob wraps 10 → 995 as -15 counts")
            // Untouched jitter is ignored but accumulates.
            events = parser.parse(buttonReport { $0[11] = 15; setKnob(&$0, 0, 996); setKnob(&$0, 1, 480) }, now: 0.03)
            try check(!events.contains { if case .encoder = $0 { return true }; return false }, "1-count untouched jitter ignored")
            events = parser.parse(buttonReport { $0[11] = 15; setKnob(&$0, 0, 998); setKnob(&$0, 1, 480) }, now: 0.04)
            try check(events.contains(.encoder(index: 0, delta: 3.0 / 1000)), "accumulated untouched turn reported")
            print("PASS main encoder and knob wrap-around deltas")
        }
        do {
            var parser = MK3InputParser()
            _ = parser.parse(buttonReport(), now: 0)
            var events = parser.parse(buttonReport { $0[30] = 0x00; $0[31] = 0x02; $0[40] = 0xff; $0[41] = 0x0f }, now: 0.01)
            try check(events.contains(.touch(target: "strip", touched: true)), "strip touch")
            try check(events.contains(.strip(value: 0.5)), "strip value 512/1024")
            try check(events.contains(.knob(name: "master", value: 1)), "master knob full")
            events = parser.parse(buttonReport { $0[40] = 0xff; $0[41] = 0x0f; $0[1] = 0x02 }, now: 0.02)
            try check(events.contains(.touch(target: "strip", touched: false)), "strip release")
            try check(events.contains(.pedal(down: true)), "pedal")
            print("PASS touch strip, analog knobs, pedal")
        }
        do {
            var parser = MK3InputParser()
            // Hardware index 12 (bottom-left) = pad 1; index 3 (top-right) = pad 16.
            try check(MK3InputParser.padNumber(hardwareIndex: 12) == 1 && MK3InputParser.padNumber(hardwareIndex: 3) == 16
                      && MK3InputParser.padNumber(hardwareIndex: 0) == 13, "pad numbering")
            for pad in 1...16 { try check(MK3InputParser.padNumber(hardwareIndex: MK3InputParser.hardwareIndex(padNumber: pad)) == pad, "pad map round trip") }
            var events = parser.parse(padSet([(12, 0x1a, 0x00)]), now: 0)
            guard case let .pad(1, .down, pressure, velocity?) = events.first else { throw NSError(domain: "hit must press pad 1: \(events)", code: 1) }
            try check(abs(pressure - 2560.0 / 4095) < 0.001 && velocity > 90 && velocity <= 127, "velocity from hit pressure")
            events = parser.parse(padSet([(12, 0x4b, 0x00), (12, 0x4b, 0x80)]), now: 0.01)
            try check(pads(events).count == 1 && pads(events)[0] == (1, .pressure), "pressure coalesced per report: \(events)")
            events = parser.parse(padSet([(12, 0x40, 0x10)]), now: 0.02)
            try check(events.isEmpty, "release suppressed during 30 ms rebound window")
            events = parser.parse(padSet([(12, 0x40, 0x10)]), now: 0.05)
            try check(pads(events).count == 1 && pads(events)[0] == (1, .up), "aftertouch below 16 releases after window")
            // Switch-on hysteresis: below 32 does not press.
            events = parser.parse(padSet([(5, 0x00, 20)]), now: 0.1)
            try check(events.isEmpty, "deadband does not press")
            events = parser.parse(padSet([(5, 0x00, 40)]), now: 0.11)
            try check(pads(events).count == 1 && pads(events)[0] == (MK3InputParser.padNumber(hardwareIndex: 5), .down), "switch on ≥ 32 presses")
            events = parser.parse(padSet([(5, 0x30, 0x00)]), now: 0.12)
            try check(pads(events).count == 1 && pads(events)[0].1 == .up, "explicit off releases immediately")
            print("PASS pad hit, pressure, hysteresis and release")
        }
        do {
            // Low-pressure event on hardware index 0 (d1 == 0) must not terminate the tuple list.
            var parser = MK3InputParser()
            let events = parser.parse(padSet([(0, 0x00, 0x30), (7, 0x10, 0x90)]), now: 0)
            let decoded = pads(events)
            try check(decoded.count == 2 && decoded[0] == (13, .down) && decoded[1] == (12, .down), "index-0 low-pressure tuple keeps the set going: \(events)")
            var tail = MK3InputParser()
            let stop = tail.parse(padSet([(0, 0x00, 0x00), (7, 0x10, 0x90)]), now: 0)
            try check(stop.isEmpty, "all-zero tuple ends the set")
            print("PASS index-0 low-pressure edge case and terminator")
        }
        do {
            // Double-pumped: set A still holds the pad, set B releases it; both decoded in order.
            var parser = MK3InputParser()
            _ = parser.parse(padSet([(9, 0x10, 0x80)]), now: 0)
            let packet = padSet([(9, 0x41, 0x75)]) + padSet([(9, 0x30, 0x00)])
            let events = parser.parse(packet, now: 0.2)
            let decoded = pads(events)
            try check(decoded.last.map { $0 == (MK3InputParser.padNumber(hardwareIndex: 9), .up) } ?? false, "set B release honoured: \(events)")
            try check(!parser.pads[9].pressed, "pad state released")
            // Second set re-reporting a hit does not double-trigger.
            var again = MK3InputParser()
            let twice = again.parse(padSet([(2, 0x10, 0x80)]) + padSet([(2, 0x10, 0x82)]), now: 0)
            try check(pads(twice).filter { $0.1 == .down }.count == 1, "one down for a hit repeated in set B")
            // Release on disconnect.
            let released = again.releaseAll()
            try check(pads(released).count == 1 && pads(released)[0].1 == .up, "releaseAll lifts held pads")
            print("PASS double-pumped pad sets")
        }
        do {
            try check(MK3InputParser.normalize([0x00, 0x01], reportID: 2) == [0x02, 0x00, 0x01], "prepend missing report ID")
            let full = buttonReport()
            try check(MK3InputParser.normalize(full, reportID: 1) == full, "keep report with ID")
            try check(MK3InputParser.normalize(Array(full.dropFirst()), reportID: 1) == full, "41-byte report gets its ID back")
            // The first pad index can equal the report ID. Size, rather than that byte,
            // must identify the omitted ID even when two pad sets arrive together.
            let pumped = padSet([(2, 0x10, 0x80)]) + padSet([(2, 0x30, 0x00)])
            let normalized = MK3InputParser.normalize(Array(pumped.dropFirst()), reportID: 2)
            try check(normalized == pumped, "double-pumped report restores missing ID before hardware pad index 2")
            try check(MK3InputParser.normalize(pumped, reportID: 2) == pumped, "double-pumped report retains existing ID")
            var parser = MK3InputParser()
            let transitions = pads(parser.parse(normalized, now: 0))
            try check(transitions.count == 2 && transitions[0] == (15, .down) && transitions[1] == (15, .up),
                      "normalization preserves both pad-15 press and release")
            print("PASS report ID normalisation")
        }
        do {
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#ff0000")) == (1 << 2 | 3), "red full")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#00ff00")) == (7 << 2 | 3), "green full")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#0000ff")) == (11 << 2 | 3), "blue full")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#ffff00")) == (5 << 2 | 3), "yellow")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#00ffff")) == (9 << 2 | 3), "cyan")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#ff00ff")) == (13 << 2 | 3), "magenta")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#ffffff")) == (17 << 2 | 3), "white")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#400000")) == (1 << 2 | 0), "dim red")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#000000")) == 0 && MK3LEDCode.indexed(nil) == 0, "off")
            // SuperTracker's muted default track palette, at full value (mk3.js normalizes LED colours): every
            // colour keeps a hue (not white), and the eight groups are not all the same palette entry.
            let palette = ["#bba374", "#a0adb1", "#b0af89", "#c58b69", "#b79583", "#9da688", "#94a39a", "#aaa0ad"].map { hex -> UInt8 in
                let c = MK3RGB(hex: hex)!, k = 255.0 / Double(max(c.r, c.g, c.b))
                return MK3LEDCode.indexed(MK3RGB(r: UInt8((Double(c.r) * k).rounded()), g: UInt8((Double(c.g) * k).rounded()), b: UInt8((Double(c.b) * k).rounded())))
            }
            try check(palette.allSatisfy { $0 >> 2 != 17 && $0 & 3 == 3 }, "track colours light in their hue at full intensity \(palette)")
            try check(Set(palette.map { $0 >> 2 }).count >= 5, "track colours stay distinguishable \(palette)")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#0f0f0f")) == (17 << 2 | 0), "6 % white marker (empty beat step) lights dimly")
            try check(MK3LEDCode.indexed(MK3RGB(hex: "#f4f4f2")) >> 2 == 17, "near-white stays white")
            try check(MK3LEDCode.single(0) == 0 && MK3LEDCode.single(1) == 0x7f && MK3LEDCode.single(0.2) & 3 == 0 && MK3LEDCode.single(0.6) & 3 == 2, "single levels")
            var leds = MK3LEDState()
            leds.apply(["buttons": ["play": 1, "channel": 0.5, "bogus": 1], "groups": ["#ff0000", NSNull()],
                        "pads": ["#00ff00"] + Array(repeating: NSNull(), count: 14) + ["#0000ff"],
                        "strip": [1, 0, "#ff0000"], "nav": ["up": "#ffffff"]])
            var buttons = leds.buttonReport(), padBytes = leds.padReport()
            try check(buttons.count == 63 && buttons[0] == 0x80 && padBytes.count == 42 && padBytes[0] == 0x81, "report sizes")
            try check(buttons[42] == 0x7f && buttons[1] == MK3LEDCode.single(0.5), "play/channel LED offsets")
            try check(buttons[30] == (1 << 2 | 3) && buttons[31] == 0 && buttons[59] == (17 << 2 | 3), "group A and nav up")
            try check(padBytes[26 + 12] == (7 << 2 | 3), "pad 1 → hardware index 12")
            try check(padBytes[26 + 3] == (11 << 2 | 3), "pad 16 → hardware index 3")
            try check(padBytes[1] == (17 << 2 | 3) && padBytes[2] == 0 && padBytes[3] == (1 << 2 | 3), "strip")
            leds.apply(["buttons": ["play": 0]])
            buttons = leds.buttonReport(); padBytes = leds.padReport()
            try check(buttons[42] == 0 && buttons[1] == MK3LEDCode.single(0.5) && padBytes[26 + 12] == (7 << 2 | 3), "partial update keeps others")
            print("PASS LED reports and palette mapping")
        }
        do {
            let renderer = MK3ScreenRenderer()
            renderer.render(MK3DrawOp.parse([["op": "clear", "color": "#000000"],
                                             ["op": "rect", "x": 10, "y": 20, "w": 30, "h": 10, "color": "#ff0000"]]))
            let frame = renderer.framebuffer(format: .rgb565BigEndian)
            try check(frame[25 * 480 + 20] == 0xf800 && frame[0] == 0, "rect red at top-left origin (RGB565)")
            let bgr = renderer.framebuffer(format: .bgr565BigEndian)
            try check(bgr[25 * 480 + 20] == 0x001f, "BGR565 swaps channels")
            // Full first frame round-trips through the packet decoder.
            guard let full = MK3ScreenEncoder.packet(screen: 1, previous: nil, next: frame) else { throw NSError(domain: "no full packet", code: 1) }
            try check(Array(full.prefix(16)) == [0x84, 0, 1, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0xe0, 0x01, 0x10], "full header")
            try check(Array(full.suffix(8)) == [0x03, 0, 0, 0, 0x40, 0, 0, 0], "footer")
            try check(full.count < 2000, "mostly-black frame compresses with repeat commands: \(full.count)")
            var decoded = [UInt16](repeating: 0x1234, count: MK3Screen.pixelCount)
            try check(MK3ScreenEncoder.apply(packet: full, to: &decoded) && decoded == frame, "full packet decodes to frame")
            try check(MK3ScreenEncoder.packet(screen: 0, previous: frame, next: frame) == nil, "unchanged frame sends nothing")
            // Partial update: text in a corner.
            renderer.render(MK3DrawOp.parse([["op": "clear", "color": "#000000"],
                                             ["op": "rect", "x": 10, "y": 20, "w": 30, "h": 10, "color": "#ff0000"],
                                             ["op": "text", "x": 301, "y": 201, "text": "SuperTracker", "size": 18, "color": "#ffffff", "weight": "bold"]]))
            let next = renderer.framebuffer(format: .rgb565BigEndian)
            guard let region = MK3ScreenEncoder.dirtyRegion(previous: frame, next: next) else { throw NSError(domain: "text must change pixels", code: 1) }
            try check(region.x % 4 == 0 && region.y % 2 == 0 && region.width % 4 == 0 && region.height % 2 == 0, "4×2 alignment \(region)")
            try check(region.x >= 296 && region.y >= 200 && region.x + region.width <= 480, "region bounds text \(region)")
            guard let partial = MK3ScreenEncoder.packet(screen: 0, previous: frame, next: next) else { throw NSError(domain: "no partial", code: 1) }
            var screen = frame
            try check(MK3ScreenEncoder.apply(packet: partial, to: &screen) && screen == next, "partial packet reproduces frame")
            try check(partial.count < full.count + 40_000 && partial[2] == 0, "partial packet is small, left screen")
            // Every op renders; arc and bar draw colour.
            renderer.render(MK3DrawOp.parse([["op": "clear", "color": "#101010"],
                                             ["op": "frame", "x": 0, "y": 0, "w": 480, "h": 272, "color": "#ffffff", "width": 2],
                                             ["op": "line", "x1": 0, "y1": 136, "x2": 480, "y2": 136, "color": "#00ff00"],
                                             ["op": "bar", "x": 20, "y": 100, "w": 100, "h": 10, "value": 0.5, "color": "#00ff00", "background": "#333333"],
                                             ["op": "arc", "x": 240, "y": 200, "r": 30, "value": 0.75, "color": "#ffaa00", "background": "#333333", "width": 4],
                                             ["op": "text", "x": 240, "y": 10, "text": "Centered", "size": 14, "color": "#ffffff", "align": "center", "mono": true],
                                             ["op": "text", "x": 20, "y": 40, "text": "A very long label that must be truncated", "size": 14, "color": "#ffffff", "width": 80],
                                             ["op": "nonsense"]]))
            let ops = renderer.framebuffer(format: .rgb565BigEndian)
            try check(ops[105 * 480 + 30] == 0x07e0 && ops[105 * 480 + 110] != 0x07e0, "bar fills half")
            try check(ops[0] == 0xffff, "frame border")
            let ring = (0..<MK3Screen.pixelCount).filter { ops[$0] >> 11 > 25 && (ops[$0] >> 5 & 0x3f) > 30 && (ops[$0] & 0x1f) < 6 }.count
            try check(ring > 50, "arc draws its value portion")
            let truncated = (0..<272).flatMap { y in (104..<460).map { y * 480 + $0 } }.filter { y in let p = y / 480; return p >= 40 && p < 60 && ops[y] > 0x8000 }
            try check(truncated.isEmpty, "truncated text stays inside its width")
            print("PASS screen rendering, diff regions and packet encoding")
        }
        print("PASS MaschineMK3ProtocolTests")
    }
}
