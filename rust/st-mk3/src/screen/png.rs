//! A minimal PNG writer (8-bit RGB, uncompressed deflate blocks) for screen dumps: enough to look
//! at a rendered frame in any image viewer without a compression library.

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in bytes.chunks(5000) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    b << 16 | a
}

fn chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encodes `rgb` (three bytes per pixel, rows top to bottom) as a PNG file. Returns `None` when the
/// data does not match the size.
pub fn encode_rgb(width: usize, height: usize, rgb: &[u8]) -> Option<Vec<u8>> {
    if width == 0 || height == 0 || rgb.len() != width * height * 3 {
        return None;
    }
    // Each row is preceded by its filter byte (0: none).
    let mut raw = Vec::with_capacity((width * 3 + 1) * height);
    for row in rgb.chunks(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut stream = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        stream.push(u8::from(blocks.peek().is_none()));
        stream.extend_from_slice(&(block.len() as u16).to_le_bytes());
        stream.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        stream.extend_from_slice(block);
    }
    stream.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bits, RGB, deflate, no filter, no interlace
    chunk(b"IHDR", &header, &mut out);
    chunk(b"IDAT", &stream, &mut out);
    chunk(b"IEND", &[], &mut out);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_their_reference_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
    }

    #[test]
    fn a_png_has_its_chunks_and_all_pixels() {
        let (width, height) = (480, 272);
        let rgb: Vec<u8> = (0..width * height * 3).map(|index| (index % 251) as u8).collect();
        let png = encode_rgb(width, height, &rgb).unwrap();
        assert_eq!(png[..8], [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(png[16..24], [0, 0, 1, 0xE0, 0, 0, 1, 0x10]);
        assert_eq!(png[png.len() - 12..], [0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
        // Walk the stored blocks of the one IDAT chunk and recover the rows.
        let length = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        assert_eq!(&png[37..41], b"IDAT");
        let stream = &png[41..41 + length];
        assert_eq!(stream[..2], [0x78, 0x01]);
        let (mut position, mut raw) = (2, Vec::new());
        loop {
            let last = stream[position] == 1;
            let size = usize::from(u16::from_le_bytes([stream[position + 1], stream[position + 2]]));
            assert_eq!(u16::from_le_bytes([stream[position + 3], stream[position + 4]]), !(size as u16));
            raw.extend_from_slice(&stream[position + 5..position + 5 + size]);
            position += 5 + size;
            if last {
                break;
            }
        }
        assert_eq!(stream[position..], adler32(&raw).to_be_bytes());
        let recovered: Vec<u8> = raw.chunks(width * 3 + 1).flat_map(|row| row[1..].to_vec()).collect();
        assert_eq!(recovered, rgb);
        assert_eq!(encode_rgb(2, 2, &[0; 11]), None);
        assert_eq!(encode_rgb(0, 2, &[]), None);
    }
}
