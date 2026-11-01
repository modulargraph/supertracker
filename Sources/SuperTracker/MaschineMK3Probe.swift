import Foundation

/// `SuperTracker --mk3-probe <seconds> [--exclusive] [--raw]`: hardware diagnostic without a window.
/// Prints every decoded event, lights the pads in a rainbow, steps button backlight levels and — only with
/// --exclusive — draws a test pattern on both screens. Always releases the device and restores NI's agents.
@MainActor
enum MaschineMK3Probe {
    static func line(_ text: String) { print(text); fflush(stdout) }

    static func json(_ value: Any) -> String {
        guard JSONSerialization.isValidJSONObject(value), let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) else { return "\(value)" }
        return String(decoding: data, as: UTF8.self)
    }

    static func hex(hue: Double, value: Double = 1) -> String {
        let h = hue.truncatingRemainder(dividingBy: 360) / 60, c = value, x = c * (1 - abs(h.truncatingRemainder(dividingBy: 2) - 1))
        let (r, g, b): (Double, Double, Double) = h < 1 ? (c, x, 0) : h < 2 ? (x, c, 0) : h < 3 ? (0, c, x) : h < 4 ? (0, x, c) : h < 5 ? (x, 0, c) : (c, 0, x)
        return String(format: "#%02x%02x%02x", Int(r * 255), Int(g * 255), Int(b * 255))
    }

    static func screenOps(side: String, elapsed: Int, seconds: Int) -> [[String: Any]] {
        var ops: [[String: Any]] = [["op": "clear", "color": "#0b0e14"]]
        // Pure colour bars, labelled, to verify RGB565 vs BGR565 channel order.
        let bars: [(String, String)] = [("#ff0000", "RED"), ("#00ff00", "GREEN"), ("#0000ff", "BLUE"), ("#ffffff", "WHITE")]
        for (index, bar) in bars.enumerated() {
            let x = 16 + index * 114
            ops.append(["op": "rect", "x": x, "y": 52, "w": 106, "h": 60, "color": bar.0, "radius": 6])
            ops.append(["op": "text", "x": x, "y": 118, "w": 106, "width": 106, "text": bar.1, "size": 14, "color": "#e6e9ef", "align": "center", "weight": "bold"])
        }
        ops.append(["op": "text", "x": 16, "y": 12, "text": "SuperTracker · Maschine MK3 probe · \(side) screen", "size": 20, "color": "#ffffff", "weight": "bold", "width": 448])
        ops.append(["op": "frame", "x": 0, "y": 0, "w": 480, "h": 272, "color": "#3a7bd5", "width": 2])
        ops.append(["op": "line", "x1": 16, "y1": 44, "x2": 464, "y2": 44, "color": "#3a7bd5", "width": 1])
        for knob in 0..<4 {
            let value = Double((elapsed + knob * 3) % 12) / 11
            ops.append(["op": "arc", "x": 60 + knob * 120, "y": 190, "r": 26, "value": value, "color": hex(hue: Double(knob) * 90), "background": "#2a2f3a", "width": 5])
            ops.append(["op": "text", "x": 60 + knob * 120, "y": 182, "text": "\(Int(value * 100))", "size": 13, "color": "#e6e9ef", "align": "center", "mono": true])
        }
        ops.append(["op": "bar", "x": 16, "y": 236, "w": 448, "h": 10, "value": Double(elapsed) / Double(max(1, seconds)), "color": "#5ad19a", "background": "#2a2f3a"])
        ops.append(["op": "text", "x": 464, "y": 250, "text": "\(elapsed) / \(seconds) s", "size": 12, "color": "#9aa3b2", "align": "right", "mono": true])
        return ops
    }

    static func run(seconds: Double, exclusive: Bool, raw: Bool) async -> Int32 {
        let driver = MaschineMK3Driver.shared
        var counts: [String: Int] = [:]
        driver.onLog = { line("log " + $0) }
        driver.onStatus = { line("status " + json($0)) }
        driver.onInput = { payload in
            counts[payload["type"] as? String ?? "?", default: 0] += 1
            line("input " + json(payload))
        }
        if raw { driver.onRawReport = { id, bytes in line(String(format: "raw 0x%02x ", id) + bytes.map { String(format: "%02x", $0) }.joined(separator: " ")) } }
        line("mk3-probe: \(exclusive ? "EXCLUSIVE (NI agents will be stopped and restored)" : "shared") mode, \(Int(seconds)) s")
        await driver.connect(exclusive: exclusive)
        let start = Date()
        while !driver.connected && Date().timeIntervalSince(start) < 2 { try? await Task.sleep(nanoseconds: 100_000_000) }
        guard driver.connected else {
            line("mk3-probe: no Maschine MK3 (17cc:1600) found")
            await driver.disconnect()
            return 2
        }
        let pads = (0..<16).map { hex(hue: Double($0) * 22.5) }
        let groups = (0..<8).map { hex(hue: Double($0) * 45) }
        driver.setLEDs(["pads": pads, "groups": groups, "strip": (0..<25).map { Double($0) / 24 },
                        "nav": ["up": "#ff0000", "right": "#00ff00", "down": "#0000ff", "left": "#ffffff"]])
        line("leds pads rainbow (pad 1 = red, bottom-left), groups A–H rainbow, nav up/right/down/left = red/green/blue/white")
        let names = MK3LEDState.buttonOffsets.keys.sorted()
        let levels: [Double] = [0.25, 0.5, 0.75, 1.0, 0]
        var level = 0
        var drawn = -1
        let total = Int(seconds.rounded(.up))
        var nextLevel = Date()
        while Date().timeIntervalSince(start) < seconds {
            let second = Int(Date().timeIntervalSince(start))
            if Date() >= nextLevel {
                let value = levels[level % levels.count]
                driver.setLEDs(["buttons": Dictionary(uniqueKeysWithValues: names.map { ($0, value) })])
                line(String(format: "leds all button backlights level %.2f → byte 0x%02x", value, MK3LEDCode.single(value)))
                level += 1
                nextLevel = Date().addingTimeInterval(1.5)
            }
            if driver.screens, second != drawn {
                drawn = second
                driver.setScreens(["left": screenOps(side: "left", elapsed: second, seconds: total),
                                   "right": screenOps(side: "right", elapsed: second, seconds: total)])
            }
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        line("diagnostics " + json(driver.diagnostics))
        line("event counts " + json(counts))
        await driver.disconnect()
        line("final status " + json(driver.status))
        if exclusive {
            let agents = MK3AgentController.agents.map { "\($0.name)=\(MK3AgentController.isRunning($0.name) ? "running" : "NOT RUNNING")" }
            line("ni agents " + agents.joined(separator: " "))
        }
        return 0
    }
}
