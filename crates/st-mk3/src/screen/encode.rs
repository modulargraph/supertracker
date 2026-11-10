//! From a framebuffer to the device: 16-bit pixel formats, the changed region, and the bulk packet
//! that carries it (interface 5, endpoint 0x04; see `docs/hardware/MaschineMK3-Display.md`).

use super::{HEIGHT, PIXEL_COUNT, WIDTH};

/// How a pixel's 16 bits are laid out. Both are big-endian on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Blue in the high five bits (the Encdr reference).
    Bgr565,
    /// Red in the high five bits (the cabl documentation).
    Rgb565,
}

/// The format sent to the device. Change it here if red and blue come out swapped on hardware.
pub const DEVICE_FORMAT: PixelFormat = PixelFormat::Bgr565;
/// The cabl documentation has a zero byte in the footer where Encdr writes the screen index. Zero
/// unless hardware disagrees.
pub const FOOTER_CARRIES_SCREEN_INDEX: bool = false;

impl PixelFormat {
    /// Packs an 8-bit colour.
    pub fn pack(self, r: u8, g: u8, b: u8) -> u16 {
        let (high, low) = match self {
            PixelFormat::Bgr565 => (b, r),
            PixelFormat::Rgb565 => (r, b),
        };
        u16::from(high >> 3) << 11 | u16::from(g >> 2) << 5 | u16::from(low >> 3)
    }

    /// The inverse of [`pack`](Self::pack), with the low bits replicated: what the device shows.
    pub fn unpack(self, value: u16) -> (u8, u8, u8) {
        let (high, middle, low) = ((value >> 11 & 0x1F) as u8, (value >> 5 & 0x3F) as u8, (value & 0x1F) as u8);
        let five = |bits: u8| bits << 3 | bits >> 2;
        let green = middle << 2 | middle >> 4;
        match self {
            PixelFormat::Bgr565 => (five(low), green, five(high)),
            PixelFormat::Rgb565 => (five(high), green, five(low)),
        }
    }
}

/// A rectangle of pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Left column.
    pub x: usize,
    /// Top row.
    pub y: usize,
    /// Columns.
    pub width: usize,
    /// Rows.
    pub height: usize,
}

/// The bounding box of the pixels that differ, grown to the 4 × 2 alignment the device needs.
/// `None` when nothing changed; the whole screen when there is no previous frame.
pub fn dirty_region(previous: Option<&[u16]>, next: &[u16]) -> Option<Region> {
    let full = Region { x: 0, y: 0, width: WIDTH, height: HEIGHT };
    let Some(previous) = previous.filter(|previous| previous.len() == next.len() && next.len() == PIXEL_COUNT) else {
        return Some(full);
    };
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (WIDTH, 0usize, HEIGHT, 0usize);
    let mut any = false;
    for y in 0..HEIGHT {
        let (old, new) = (&previous[y * WIDTH..(y + 1) * WIDTH], &next[y * WIDTH..(y + 1) * WIDTH]);
        let Some(first) = (0..WIDTH).find(|&x| old[x] != new[x]) else { continue };
        let last = (first..WIDTH).rev().find(|&x| old[x] != new[x]).unwrap_or(first);
        any = true;
        min_x = min_x.min(first);
        max_x = max_x.max(last);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    if !any {
        return None;
    }
    let (x0, y0) = (min_x / 4 * 4, min_y / 2 * 2);
    let (x1, y1) = (WIDTH.min((max_x / 4 + 1) * 4), HEIGHT.min((max_y / 2 + 1) * 2));
    Some(Region { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// The 16-byte packet header for a region of `screen` (0 left, 1 right).
pub fn header(screen: u8, region: Region) -> [u8; 16] {
    let be = |value: usize| [(value >> 8) as u8, value as u8];
    let (x, y, w, h) = (be(region.x), be(region.y), be(region.width), be(region.height));
    [0x84, 0x00, screen, 0x60, 0x00, 0x00, 0x00, 0x00, x[0], x[1], y[0], y[1], w[0], w[1], h[0], h[1]]
}

/// The 8-byte packet footer.
pub fn footer(screen: u8) -> [u8; 8] {
    [0x03, 0x00, 0x00, 0x00, 0x40, 0x00, if FOOTER_CARRIES_SCREEN_INDEX { screen } else { 0x00 }, 0x00]
}

fn command(code: u8, count: usize, out: &mut Vec<u8>) {
    out.extend_from_slice(&[code, (count >> 16) as u8, (count >> 8) as u8, count as u8]);
}

fn pixel(value: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_be_bytes());
}

/// One packet for the changed region of `screen`, or `None` when nothing changed.
///
/// Inside the region, pairs of pixels are encoded as skip (unchanged), repeat (three or more equal
/// pairs) or literal runs.
pub fn packet(screen: u8, previous: Option<&[u16]>, next: &[u16]) -> Option<Vec<u8>> {
    if next.len() != PIXEL_COUNT {
        return None;
    }
    let previous = previous.filter(|previous| previous.len() == PIXEL_COUNT);
    let region = dirty_region(previous, next)?;
    let pairs_per_row = region.width / 2;
    let total = pairs_per_row * region.height;
    let mut out = Vec::with_capacity(16 + total * 4 / 8 + 64);
    out.extend_from_slice(&header(screen, region));
    // Pair `k` of the region: (first pixel, second pixel, unchanged).
    let pair = |k: usize| -> (u16, u16, bool) {
        let index = (region.y + k / pairs_per_row) * WIDTH + region.x + (k % pairs_per_row) * 2;
        let (a, b) = (next[index], next[index + 1]);
        (a, b, previous.is_some_and(|previous| previous[index] == a && previous[index + 1] == b))
    };
    let flush = |start: &mut Option<usize>, end: usize, out: &mut Vec<u8>| {
        if let Some(start) = start.take() {
            command(0x00, end - start, out);
            for k in start..end {
                let (a, b, _) = pair(k);
                pixel(a, out);
                pixel(b, out);
            }
        }
    };
    let mut literal: Option<usize> = None;
    let mut k = 0;
    while k < total {
        let (a, b, same) = pair(k);
        if same {
            let mut end = k + 1;
            while end < total && pair(end).2 {
                end += 1;
            }
            // A single unchanged pair inside a literal run is cheaper as part of the run.
            if end - k >= 2 || literal.is_none() {
                flush(&mut literal, k, &mut out);
                command(0x02, end - k, &mut out);
                k = end;
                continue;
            }
        }
        let mut end = k + 1;
        while end < total {
            let (c, d, _) = pair(end);
            if c != a || d != b {
                break;
            }
            end += 1;
        }
        if end - k >= 3 {
            flush(&mut literal, k, &mut out);
            command(0x01, end - k, &mut out);
            pixel(a, &mut out);
            pixel(b, &mut out);
            k = end;
            continue;
        }
        literal.get_or_insert(k);
        k += 1;
    }
    flush(&mut literal, total, &mut out);
    out.extend_from_slice(&footer(screen));
    Some(out)
}

/// Decodes a packet into a framebuffer, as the device would. Returns false for a packet that is
/// malformed or does not cover its region exactly. Used by tests and by screen dumps.
pub fn apply(packet: &[u8], framebuffer: &mut [u16]) -> bool {
    if packet.len() < 24 || packet[0] != 0x84 || packet[3] != 0x60 || framebuffer.len() != PIXEL_COUNT {
        return false;
    }
    let be = |index: usize| usize::from(packet[index]) << 8 | usize::from(packet[index + 1]);
    let (x, y, width, height) = (be(8), be(10), be(12), be(14));
    if width == 0 || height == 0 || x + width > WIDTH || y + height > HEIGHT {
        return false;
    }
    let total = width * height;
    let mut cursor = 0usize;
    let mut index = 16usize;
    let mut put = |cursor: &mut usize, value: u16| {
        framebuffer[(y + *cursor / width) * WIDTH + x + *cursor % width] = value;
        *cursor += 1;
    };
    let word = |index: usize| u16::from(packet[index]) << 8 | u16::from(packet[index + 1]);
    while index + 4 <= packet.len() {
        let code = packet[index];
        let count = usize::from(packet[index + 1]) << 16 | usize::from(packet[index + 2]) << 8 | usize::from(packet[index + 3]);
        index += 4;
        match code {
            0x00 => {
                if index + count * 4 > packet.len() || cursor + count * 2 > total {
                    return false;
                }
                for _ in 0..count * 2 {
                    put(&mut cursor, word(index));
                    index += 2;
                }
            }
            0x01 => {
                if index + 4 > packet.len() || cursor + count * 2 > total {
                    return false;
                }
                let (a, b) = (word(index), word(index + 2));
                index += 4;
                for _ in 0..count {
                    put(&mut cursor, a);
                    put(&mut cursor, b);
                }
            }
            0x02 => {
                if cursor + count * 2 > total {
                    return false;
                }
                cursor += count * 2;
            }
            0x03 => return cursor == total && index + 4 <= packet.len() && packet[index] == 0x40,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_formats_pack_and_unpack() {
        assert_eq!(PixelFormat::Rgb565.pack(255, 0, 0), 0xF800);
        assert_eq!(PixelFormat::Bgr565.pack(255, 0, 0), 0x001F);
        assert_eq!(PixelFormat::Rgb565.pack(0, 255, 0), 0x07E0);
        assert_eq!(PixelFormat::Bgr565.pack(0, 0, 255), 0xF800);
        for format in [PixelFormat::Rgb565, PixelFormat::Bgr565] {
            assert_eq!(format.unpack(format.pack(255, 255, 255)), (255, 255, 255));
            assert_eq!(format.unpack(format.pack(0, 0, 0)), (0, 0, 0));
            assert_eq!(format.unpack(format.pack(255, 0, 0)), (255, 0, 0));
            assert_eq!(format.unpack(format.pack(8, 4, 8)), (8, 4, 8));
        }
    }

    #[test]
    fn dirty_regions_are_aligned_bounding_boxes() {
        let base = vec![0u16; PIXEL_COUNT];
        assert_eq!(dirty_region(None, &base), Some(Region { x: 0, y: 0, width: WIDTH, height: HEIGHT }));
        assert_eq!(dirty_region(Some(&base), &base), None);
        assert_eq!(dirty_region(Some(&base[..10]), &base), Some(Region { x: 0, y: 0, width: WIDTH, height: HEIGHT }), "a baseline of another size is no baseline");
        let mut next = base.clone();
        next[101 * WIDTH + 203] = 1;
        assert_eq!(dirty_region(Some(&base), &next), Some(Region { x: 200, y: 100, width: 4, height: 2 }));
        next[5 * WIDTH + 479] = 1;
        next[271 * WIDTH] = 1;
        assert_eq!(dirty_region(Some(&base), &next), Some(Region { x: 0, y: 4, width: 480, height: 268 }));
    }

    fn roundtrip(previous: Option<&[u16]>, next: &[u16]) -> Vec<u8> {
        let packet = packet(1, previous, next).expect("a packet");
        let mut decoded = previous.map(<[u16]>::to_vec).unwrap_or_else(|| vec![0x1234; PIXEL_COUNT]);
        assert!(apply(&packet, &mut decoded), "the packet decodes");
        assert!(decoded == next, "and reproduces the frame");
        packet
    }

    #[test]
    fn packets_round_trip_through_the_decoder() {
        let black = vec![0u16; PIXEL_COUNT];
        let full = roundtrip(None, &black);
        assert_eq!(full[..16], [0x84, 0, 1, 0x60, 0, 0, 0, 0, 0, 0, 0, 0, 0x01, 0xE0, 0x01, 0x10]);
        assert_eq!(full[16..24], [0x01, 0x00, 0xFF, 0x00, 0, 0, 0, 0], "one repeat command for 65 280 equal pairs");
        assert_eq!(full[full.len() - 8..], [0x03, 0, 0, 0, 0x40, 0, 0, 0]);
        assert_eq!(full.len(), 32);
        assert_eq!(packet(0, Some(&black), &black), None, "an unchanged frame sends nothing");

        // Noise, runs, single unchanged pairs between literals, and changes at the region's edges.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut random = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut frame = black.clone();
        for _ in 0..40 {
            let previous = frame.clone();
            for _ in 0..(random() % 6) {
                let (x, y, w, h) = ((random() % 470) as usize, (random() % 262) as usize, 1 + (random() % 60) as usize, 1 + (random() % 20) as usize);
                let solid = random() % 3 == 0;
                let colour = random() as u16;
                for row in y..(y + h).min(HEIGHT) {
                    for column in x..(x + w).min(WIDTH) {
                        frame[row * WIDTH + column] = if solid { colour } else { random() as u16 };
                    }
                }
            }
            if frame == previous {
                assert_eq!(packet(0, Some(&previous), &frame), None);
            } else {
                roundtrip(Some(&previous), &frame);
                roundtrip(None, &frame);
            }
        }
    }

    #[test]
    fn malformed_packets_are_refused() {
        let black = vec![0u16; PIXEL_COUNT];
        let mut frame = black.clone();
        frame[0] = 0xFFFF;
        let good = packet(0, Some(&black), &frame).unwrap();
        let mut target = black.clone();
        assert!(apply(&good, &mut target));
        assert!(!apply(&good[..good.len() - 8], &mut target), "no footer");
        assert!(!apply(&good[..20], &mut target), "too short");
        let mut wrong_magic = good.clone();
        wrong_magic[0] = 0x85;
        assert!(!apply(&wrong_magic, &mut target));
        let mut outside = good.clone();
        outside[9] = 0xFF;
        outside[8] = 0x01;
        assert!(!apply(&outside, &mut target), "a region outside the screen");
        let mut unknown = good.clone();
        unknown[16] = 0x07;
        assert!(!apply(&unknown, &mut target));
        assert!(!apply(&good, &mut target[..100]), "a framebuffer of the wrong size");
        assert_eq!(packet(0, None, &black[..100]), None);
    }
}
