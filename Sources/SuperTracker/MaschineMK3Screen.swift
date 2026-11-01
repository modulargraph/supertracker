import Foundation
import CoreGraphics
import CoreText
import ImageIO

// Maschine MK3 screens: display list → CoreGraphics bitmap → 16-bit framebuffer → changed-region bulk packet.
// Pure (no IOKit) so it can be unit tested; MaschineMK3.swift sends the packets on interface 5, endpoint 0x04.

enum MK3PixelFormat: Sendable {
    /// Encdr: BGR565, big-endian on the wire (blue in the high bits).
    case bgr565BigEndian
    /// cabl: RGB565, big-endian on the wire.
    case rgb565BigEndian

    func pack(r: UInt8, g: UInt8, b: UInt8) -> UInt16 {
        switch self {
        case .bgr565BigEndian: return UInt16(b >> 3) << 11 | UInt16(g >> 2) << 5 | UInt16(r >> 3)
        case .rgb565BigEndian: return UInt16(r >> 3) << 11 | UInt16(g >> 2) << 5 | UInt16(b >> 3)
        }
    }

    /// Inverse of `pack` (low bits replicated), for dumps that show exactly what the device receives.
    func unpack(_ value: UInt16) -> (r: UInt8, g: UInt8, b: UInt8) {
        let hi = UInt8(value >> 11 & 0x1f), mid = UInt8(value >> 5 & 0x3f), lo = UInt8(value & 0x1f)
        let x5 = { (v: UInt8) in v << 3 | v >> 2 }, g = mid << 2 | mid >> 4
        switch self {
        case .bgr565BigEndian: return (x5(lo), g, x5(hi))
        case .rgb565BigEndian: return (x5(hi), g, x5(lo))
        }
    }
}

enum MK3Screen {
    static let width = 480
    static let height = 272
    static let pixelCount = width * height
    /// Flip here if colours come out with red and blue swapped on the hardware.
    static var pixelFormat: MK3PixelFormat = .bgr565BigEndian
    /// cabl documents a zero byte here; Encdr writes the screen index. Kept zero unless hardware disagrees.
    static var footerCarriesScreenIndex = false
}

struct MK3Color: Equatable, Sendable {
    var r: Double, g: Double, b: Double, a: Double
    static let black = MK3Color(r: 0, g: 0, b: 0, a: 1)

    init(r: Double, g: Double, b: Double, a: Double = 1) { self.r = r; self.g = g; self.b = b; self.a = a }
    init?(_ value: Any?) {
        guard var text = value as? String else { return nil }
        text = text.trimmingCharacters(in: .whitespaces)
        if text == "transparent" || text == "none" { return nil }
        if text.hasPrefix("#") { text.removeFirst() }
        if text.count == 3 || text.count == 4 { text = text.map { "\($0)\($0)" }.joined() }
        guard text.count == 6 || text.count == 8, let number = UInt64(text, radix: 16) else { return nil }
        let value = text.count == 6 ? number << 8 | 0xff : number
        r = Double(value >> 24 & 0xff) / 255; g = Double(value >> 16 & 0xff) / 255
        b = Double(value >> 8 & 0xff) / 255; a = Double(value & 0xff) / 255
    }
    var cgColor: CGColor { CGColor(srgbRed: r, green: g, blue: b, alpha: a) }
}

enum MK3TextAlign: String, Sendable { case left, center, right }

enum MK3DrawOp: Equatable, Sendable {
    case clear(MK3Color)
    case rect(CGRect, MK3Color, radius: Double)
    case frame(CGRect, MK3Color, width: Double)
    case line(CGPoint, CGPoint, MK3Color, width: Double)
    /// `y` is the top of the text line (like canvas textBaseline "top"); `width` clips with an ellipsis and
    /// aligns inside [x, x + width]; without it, center/right align around/at `x`.
    case text(CGPoint, String, size: Double, MK3Color, bold: Bool, align: MK3TextAlign, width: Double?, mono: Bool)
    case bar(CGRect, value: Double, MK3Color, background: MK3Color?)
    /// Knob ring centred at (x, y): 270° sweep from bottom-left, clockwise.
    case arc(CGPoint, radius: Double, value: Double, MK3Color, background: MK3Color?, width: Double)

    /// Parses a contract display list; malformed ops are skipped.
    static func parse(_ list: [Any]) -> [MK3DrawOp] {
        list.compactMap { item -> MK3DrawOp? in
            guard let op = item as? [String: Any], let name = op["op"] as? String else { return nil }
            func n(_ key: String, _ fallback: Double? = nil) -> Double? { PayloadValue.double(op[key]) ?? fallback }
            let color = MK3Color(op["color"])
            switch name {
            case "clear": return .clear(color ?? .black)
            case "rect":
                guard let x = n("x"), let y = n("y"), let w = n("w"), let h = n("h"), let color else { return nil }
                return .rect(CGRect(x: x, y: y, width: w, height: h), color, radius: n("radius", 0)!)
            case "frame":
                guard let x = n("x"), let y = n("y"), let w = n("w"), let h = n("h"), let color else { return nil }
                return .frame(CGRect(x: x, y: y, width: w, height: h), color, width: n("width", 1)!)
            case "line":
                guard let x1 = n("x1"), let y1 = n("y1"), let x2 = n("x2"), let y2 = n("y2"), let color else { return nil }
                return .line(CGPoint(x: x1, y: y1), CGPoint(x: x2, y: y2), color, width: n("width", 1)!)
            case "text":
                guard let x = n("x"), let y = n("y"), let color else { return nil }
                let text = op["text"] as? String ?? (op["text"].map { "\($0)" } ?? "")
                return .text(CGPoint(x: x, y: y), text, size: n("size", 14)!, color, bold: op["weight"] as? String == "bold",
                             align: MK3TextAlign(rawValue: op["align"] as? String ?? "left") ?? .left,
                             width: n("width"), mono: op["mono"] as? Bool ?? false)
            case "bar":
                guard let x = n("x"), let y = n("y"), let w = n("w"), let h = n("h"), let color else { return nil }
                return .bar(CGRect(x: x, y: y, width: w, height: h), value: max(0, min(1, n("value", 0)!)), color, background: MK3Color(op["background"]))
            case "arc":
                guard let x = n("x"), let y = n("y"), let r = n("r"), let color else { return nil }
                return .arc(CGPoint(x: x, y: y), radius: r, value: max(0, min(1, n("value", 0)!)), color,
                            background: MK3Color(op["background"]), width: n("width", 4)!)
            default: return nil
            }
        }
    }
}

/// Renders display lists into a 480 × 272 RGBA bitmap and converts it to the device's 16-bit format.
final class MK3ScreenRenderer {
    let context: CGContext
    private var fonts: [String: CTFont] = [:]

    init() {
        context = CGContext(data: nil, width: MK3Screen.width, height: MK3Screen.height, bitsPerComponent: 8,
                            bytesPerRow: MK3Screen.width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        // Top-left origin, y down, like the interface's canvas coordinates.
        context.translateBy(x: 0, y: CGFloat(MK3Screen.height))
        context.scaleBy(x: 1, y: -1)
        context.setShouldAntialias(true)
        context.interpolationQuality = .high
    }

    private func font(size: Double, bold: Bool, mono: Bool) -> CTFont {
        let key = "\(size)-\(bold)-\(mono)"
        if let font = fonts[key] { return font }
        let base: CTFont
        if mono {
            base = CTFontCreateUIFontForLanguage(.userFixedPitch, size, nil) ?? CTFontCreateWithName("Menlo" as CFString, size, nil)
        } else {
            base = CTFontCreateUIFontForLanguage(bold ? .emphasizedSystem : .system, size, nil) ?? CTFontCreateWithName("Helvetica" as CFString, size, nil)
        }
        var font = base
        if mono && bold, let boldFont = CTFontCreateCopyWithSymbolicTraits(base, size, nil, .traitBold, .traitBold) { font = boldFont }
        fonts[key] = font
        return font
    }

    func render(_ ops: [MK3DrawOp]) {
        let c = context
        c.saveGState()
        c.setFillColor(MK3Color.black.cgColor)
        c.fill(CGRect(x: 0, y: 0, width: MK3Screen.width, height: MK3Screen.height))
        for op in ops {
            switch op {
            case let .clear(color):
                c.setBlendMode(.copy); c.setFillColor(color.cgColor)
                c.fill(CGRect(x: 0, y: 0, width: MK3Screen.width, height: MK3Screen.height))
                c.setBlendMode(.normal)
            case let .rect(rect, color, radius):
                c.setFillColor(color.cgColor)
                if radius > 0 {
                    let r = min(radius, Double(min(rect.width, rect.height)) / 2)
                    c.addPath(CGPath(roundedRect: rect, cornerWidth: r, cornerHeight: r, transform: nil)); c.fillPath()
                } else { c.fill(rect) }
            case let .frame(rect, color, width):
                c.setStrokeColor(color.cgColor); c.setLineWidth(width)
                c.stroke(rect.insetBy(dx: width / 2, dy: width / 2))
            case let .line(from, to, color, width):
                c.setStrokeColor(color.cgColor); c.setLineWidth(width); c.setLineCap(.butt)
                c.move(to: from); c.addLine(to: to); c.strokePath()
            case let .text(origin, text, size, color, bold, align, width, mono):
                drawText(text, at: origin, size: size, color: color, bold: bold, align: align, width: width, mono: mono)
            case let .bar(rect, value, color, background):
                if let background { c.setFillColor(background.cgColor); c.fill(rect) }
                c.setFillColor(color.cgColor)
                if rect.width >= rect.height { c.fill(CGRect(x: rect.minX, y: rect.minY, width: rect.width * value, height: rect.height)) }
                else { c.fill(CGRect(x: rect.minX, y: rect.maxY - rect.height * value, width: rect.width, height: rect.height * value)) }
            case let .arc(center, radius, value, color, background, width):
                c.setLineWidth(width); c.setLineCap(.round)
                if let background { strokeArc(center: center, radius: radius, from: 0, to: 1, color: background) }
                if value > 0 { strokeArc(center: center, radius: radius, from: 0, to: value, color: color) }
            }
        }
        c.restoreGState()
    }

    private func strokeArc(center: CGPoint, radius: Double, from: Double, to: Double, color: MK3Color) {
        let start = 135.0, sweep = 270.0
        let a0 = start + sweep * from, a1 = start + sweep * to
        let steps = max(2, Int((a1 - a0) / 3))
        for step in 0...steps {
            let angle = (a0 + (a1 - a0) * Double(step) / Double(steps)) * .pi / 180
            let point = CGPoint(x: center.x + radius * cos(angle), y: center.y + radius * sin(angle))
            if step == 0 { context.move(to: point) } else { context.addLine(to: point) }
        }
        context.setStrokeColor(color.cgColor)
        context.strokePath()
    }

    private func drawText(_ text: String, at origin: CGPoint, size: Double, color: MK3Color, bold: Bool, align: MK3TextAlign, width: Double?, mono: Bool) {
        guard !text.isEmpty else { return }
        let font = font(size: size, bold: bold, mono: mono)
        let attributes: [CFString: Any] = [kCTFontAttributeName: font, kCTForegroundColorAttributeName: color.cgColor]
        let attributed = CFAttributedStringCreate(nil, text as CFString, attributes as CFDictionary)!
        var line = CTLineCreateWithAttributedString(attributed)
        var lineWidth = CTLineGetTypographicBounds(line, nil, nil, nil)
        if let width, lineWidth > width {
            let ellipsis = CTLineCreateWithAttributedString(CFAttributedStringCreate(nil, "…" as CFString, attributes as CFDictionary)!)
            if let truncated = CTLineCreateTruncatedLine(line, width, .end, ellipsis) {
                line = truncated; lineWidth = CTLineGetTypographicBounds(line, nil, nil, nil)
            }
        }
        var x = Double(origin.x)
        switch (align, width) {
        case (.left, _): break
        case (.center, let width?): x += (width - lineWidth) / 2
        case (.right, let width?): x += width - lineWidth
        case (.center, nil): x -= lineWidth / 2
        case (.right, nil): x -= lineWidth
        }
        let baseline = Double(origin.y) + Double(CTFontGetAscent(font))
        context.saveGState()
        if let width { context.clip(to: CGRect(x: Double(origin.x) - 1, y: Double(origin.y) - size, width: width + 2, height: size * 3)) }
        // The context is flipped; flip glyphs back.
        context.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        context.textPosition = CGPoint(x: x, y: baseline)
        CTLineDraw(line, context)
        context.restoreGState()
    }

    /// Current bitmap (before 16-bit conversion) as an image, for screen dumps.
    func image() -> CGImage? { context.makeImage() }

    /// Current bitmap as 16-bit device pixels (row-major, top row first).
    func framebuffer(format: MK3PixelFormat = MK3Screen.pixelFormat) -> [UInt16] {
        var output = [UInt16](repeating: 0, count: MK3Screen.pixelCount)
        guard let data = context.data else { return output }
        let bytes = data.assumingMemoryBound(to: UInt8.self)
        let stride = context.bytesPerRow
        output.withUnsafeMutableBufferPointer { out in
            for y in 0..<MK3Screen.height {
                // Bitmap memory row 0 is the top row of the image.
                let row = bytes + y * stride
                for x in 0..<MK3Screen.width {
                    let p = row + x * 4
                    out[y * MK3Screen.width + x] = format.pack(r: p[0], g: p[1], b: p[2])
                }
            }
        }
        return output
    }
}

/// Builds screen bulk packets for changed regions.
enum MK3ScreenEncoder {
    struct Region: Equatable { var x: Int, y: Int, width: Int, height: Int }

    /// Bounding box of changed pixels, expanded to the 4 × 2 alignment the device needs. nil = no change.
    static func dirtyRegion(previous: [UInt16]?, next: [UInt16]) -> Region? {
        let w = MK3Screen.width, h = MK3Screen.height
        guard let previous, previous.count == next.count else { return Region(x: 0, y: 0, width: w, height: h) }
        var minX = w, maxX = -1, minY = h, maxY = -1
        previous.withUnsafeBufferPointer { old in
            next.withUnsafeBufferPointer { new in
                for y in 0..<h {
                    let base = y * w
                    var rowMin = -1
                    for x in 0..<w where old[base + x] != new[base + x] { rowMin = x; break }
                    guard rowMin >= 0 else { continue }
                    var rowMax = rowMin
                    for x in stride(from: w - 1, through: rowMin, by: -1) where old[base + x] != new[base + x] { rowMax = x; break }
                    minX = min(minX, rowMin); maxX = max(maxX, rowMax); minY = min(minY, y); maxY = max(maxY, y)
                }
            }
        }
        guard maxX >= 0 else { return nil }
        let x0 = minX / 4 * 4, y0 = minY / 2 * 2
        let x1 = min(w, (maxX / 4 + 1) * 4), y1 = min(h, (maxY / 2 + 1) * 2)
        return Region(x: x0, y: y0, width: x1 - x0, height: y1 - y0)
    }

    static func header(screen: Int, region: Region) -> [UInt8] {
        func be(_ value: Int) -> [UInt8] { [UInt8(value >> 8 & 0xff), UInt8(value & 0xff)] }
        return [0x84, 0x00, UInt8(screen), 0x60, 0x00, 0x00, 0x00, 0x00] + be(region.x) + be(region.y) + be(region.width) + be(region.height)
    }

    static func footer(screen: Int) -> [UInt8] {
        [0x03, 0x00, 0x00, 0x00, 0x40, 0x00, MK3Screen.footerCarriesScreenIndex ? UInt8(screen) : 0x00, 0x00]
    }

    private static func command(_ code: UInt8, _ count: Int, into out: inout [UInt8]) {
        out.append(code); out.append(UInt8(count >> 16 & 0xff)); out.append(UInt8(count >> 8 & 0xff)); out.append(UInt8(count & 0xff))
    }
    private static func pixel(_ value: UInt16, into out: inout [UInt8]) {
        out.append(UInt8(value >> 8)); out.append(UInt8(value & 0xff))
    }

    /// One packet for the changed region of `screen` (0 = left, 1 = right), or nil when nothing changed.
    /// Inside the region, pixel pairs are encoded as skip (unchanged), repeat (≥ 3 identical pairs) or literal.
    static func packet(screen: Int, previous: [UInt16]?, next: [UInt16]) -> [UInt8]? {
        guard next.count == MK3Screen.pixelCount, let region = dirtyRegion(previous: previous, next: next) else { return nil }
        let w = MK3Screen.width
        let pairsPerRow = region.width / 2
        let totalPairs = pairsPerRow * region.height
        var out = header(screen: screen, region: region)
        out.reserveCapacity(out.count + totalPairs * 4 + 64)
        // Pair k of the region, as (first, second, unchanged).
        @inline(__always) func pair(_ k: Int) -> (UInt16, UInt16, Bool) {
            let row = region.y + k / pairsPerRow, column = region.x + (k % pairsPerRow) * 2
            let index = row * w + column
            let a = next[index], b = next[index + 1]
            let same = previous.map { $0[index] == a && $0[index + 1] == b } ?? false
            return (a, b, same)
        }
        var k = 0
        var literalStart = -1
        func flushLiteral(upTo end: Int) {
            guard literalStart >= 0 else { return }
            command(0x00, end - literalStart, into: &out)
            for j in literalStart..<end { let (a, b, _) = pair(j); pixel(a, into: &out); pixel(b, into: &out) }
            literalStart = -1
        }
        while k < totalPairs {
            let (a, b, same) = pair(k)
            if same {
                var end = k + 1
                while end < totalPairs, pair(end).2 { end += 1 }
                if end - k >= 2 || literalStart < 0 {
                    flushLiteral(upTo: k)
                    command(0x02, end - k, into: &out)
                    k = end; continue
                }
            }
            var end = k + 1
            while end < totalPairs { let (c, d, _) = pair(end); if c == a && d == b { end += 1 } else { break } }
            if end - k >= 3 {
                flushLiteral(upTo: k)
                command(0x01, end - k, into: &out); pixel(a, into: &out); pixel(b, into: &out)
                k = end; continue
            }
            if literalStart < 0 { literalStart = k }
            k += 1
        }
        flushLiteral(upTo: totalPairs)
        out += footer(screen: screen)
        return out
    }

    /// Decodes a packet back into a framebuffer (test support and self-check).
    static func apply(packet: [UInt8], to framebuffer: inout [UInt16]) -> Bool {
        guard packet.count >= 24, packet[0] == 0x84, packet[3] == 0x60 else { return false }
        func be(_ i: Int) -> Int { Int(packet[i]) << 8 | Int(packet[i + 1]) }
        let x = be(8), y = be(10), width = be(12), height = be(14)
        guard width > 0, height > 0, x + width <= MK3Screen.width, y + height <= MK3Screen.height else { return false }
        var cursor = 0, i = 16
        let total = width * height
        func put(_ value: UInt16) {
            let row = y + cursor / width, column = x + cursor % width
            framebuffer[row * MK3Screen.width + column] = value; cursor += 1
        }
        while i + 4 <= packet.count {
            let code = packet[i], count = Int(packet[i + 1]) << 16 | Int(packet[i + 2]) << 8 | Int(packet[i + 3])
            i += 4
            switch code {
            case 0x00:
                guard i + count * 4 <= packet.count, cursor + count * 2 <= total else { return false }
                for _ in 0..<(count * 2) { put(UInt16(packet[i]) << 8 | UInt16(packet[i + 1])); i += 2 }
            case 0x01:
                guard i + 4 <= packet.count, cursor + count * 2 <= total else { return false }
                let a = UInt16(packet[i]) << 8 | UInt16(packet[i + 1]), b = UInt16(packet[i + 2]) << 8 | UInt16(packet[i + 3])
                i += 4
                for _ in 0..<count { put(a); put(b) }
            case 0x02:
                guard cursor + count * 2 <= total else { return false }
                cursor += count * 2
            case 0x03:
                return cursor == total && i + 4 <= packet.count && packet[i] == 0x40
            default: return false
            }
        }
        return false
    }
}

/// Debug dumps (SUPERTRACKER_MK3_SCREEN_DUMP=<dir>): PNGs of the rendered bitmap and of the 16-bit framebuffer
/// decoded back from the bulk packet, so packing and encoding bugs show up as differences between the two.
enum MK3ScreenDump {
    static func image(framebuffer: [UInt16], format: MK3PixelFormat = MK3Screen.pixelFormat) -> CGImage? {
        guard framebuffer.count == MK3Screen.pixelCount else { return nil }
        var bytes = [UInt8](repeating: 255, count: MK3Screen.pixelCount * 4)
        for (i, value) in framebuffer.enumerated() {
            let (r, g, b) = format.unpack(value)
            bytes[i * 4] = r; bytes[i * 4 + 1] = g; bytes[i * 4 + 2] = b
        }
        guard let provider = CGDataProvider(data: Data(bytes) as CFData) else { return nil }
        return CGImage(width: MK3Screen.width, height: MK3Screen.height, bitsPerComponent: 8, bitsPerPixel: 32,
                       bytesPerRow: MK3Screen.width * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue), provider: provider,
                       decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }

    @discardableResult
    static func writePNG(_ image: CGImage?, to url: URL) -> Bool {
        guard let image, let destination = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else { return false }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination)
    }
}
