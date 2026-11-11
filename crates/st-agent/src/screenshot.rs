//! `app.screenshot`: the picture of the application's window as AI clients receive it.
//!
//! Only the window layer can take the picture, so it installs a [`ScreenshotProvider`]
//! ([`crate::BridgePage::set_screenshot_provider`]). The provider returns a PNG file; [`reply`]
//! turns it into the control method's result, which the MCP server passes on as image content.
//! A host without a window installs nothing, and the method answers with [`NO_WINDOW`].

use serde_json::{json, Value};

/// Takes a picture of the window and returns it as a PNG file.
///
/// The argument is the widest picture the client asked for, in pixels ([`MIN_WIDTH`] to
/// [`MAX_WIDTH`]), if it asked: a wider window is scaled down to it, keeping its proportions.
/// Without it the picture has the window's own width. The call comes from one of the control
/// server's threads and may block it; an `Err` is shown to the client as it is.
pub type ScreenshotProvider = Box<dyn Fn(Option<u32>) -> Result<Vec<u8>, String> + Send + Sync>;

/// The narrowest picture a client can ask for, in pixels.
pub const MIN_WIDTH: u32 = 320;
/// The widest picture a client gets, in pixels.
pub const MAX_WIDTH: u32 = 2560;

/// What `app.screenshot` answers in a host that has no window.
pub const NO_WINDOW: &str = "SuperTracker is running without a window (for example with --serve), so there is no window to take a screenshot of.";

/// The first eight bytes of every PNG file.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// The result of `app.screenshot` for the PNG file `png`:
/// `{"mimeType": "image/png", "data": <base64>, "width": .., "height": ..}`, with the size read
/// from the file itself.
pub fn reply(png: &[u8]) -> Result<Value, String> {
    let (width, height) = png_size(png).ok_or("The window returned a picture that is not a PNG file.")?;
    Ok(json!({"mimeType": "image/png", "data": base64(png), "width": width, "height": height}))
}

/// The size in pixels a PNG file states in its header, or `None` when `file` does not begin like a
/// PNG file with a picture in it.
pub fn png_size(file: &[u8]) -> Option<(u32, u32)> {
    // Signature, then the first chunk: length (13), "IHDR", width and height (big-endian).
    let header = file.get(..24)?;
    if header[..8] != PNG_SIGNATURE || header[8..16] != [0, 0, 0, 13, b'I', b'H', b'D', b'R'] {
        return None;
    }
    let number = |at: usize| u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
    let (width, height) = (number(16), number(20));
    (width > 0 && height > 0).then_some((width, height))
}

/// `bytes` in base64 (RFC 4648, the standard alphabet, padded), as MCP image content carries it.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let bits = u32::from(group[0]) << 16 | u32::from(*group.get(1).unwrap_or(&0)) << 8 | u32::from(*group.get(2).unwrap_or(&0));
        let symbol = |shift: u32| ALPHABET[(bits >> shift & 63) as usize] as char;
        text.push(symbol(18));
        text.push(symbol(12));
        text.push(if group.len() > 1 { symbol(6) } else { '=' });
        text.push(if group.len() > 2 { symbol(0) } else { '=' });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The start of a PNG file of the given size (reading the size needs no more than this).
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut file = PNG_SIGNATURE.to_vec();
        file.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        file.extend_from_slice(&width.to_be_bytes());
        file.extend_from_slice(&height.to_be_bytes());
        file.extend_from_slice(&[8, 2, 0, 0, 0]);
        file
    }

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (plain, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xFB, 0xFF, 0xFE]), "+//+", "the standard alphabet, not the URL one");
        assert_eq!(base64(&PNG_SIGNATURE), "iVBORw0KGgo=");
        let every_byte: Vec<u8> = (0..=255).collect();
        assert_eq!(base64(&every_byte).len(), 344);
    }

    #[test]
    fn the_size_is_read_from_the_png_header() {
        assert_eq!(png_size(&png_header(1500, 915)), Some((1500, 915)));
        assert_eq!(png_size(&png_header(70000, 1)), Some((70000, 1)));
        assert_eq!(png_size(&png_header(0, 10)), None, "a picture has at least one pixel");
        assert_eq!(png_size(&png_header(640, 400)[..23]), None, "a cut-off header");
        assert_eq!(png_size(b"GIF89a, and then enough bytes for a header"), None);
        let mut other_chunk = png_header(640, 400);
        other_chunk[12..16].copy_from_slice(b"IDAT");
        assert_eq!(png_size(&other_chunk), None, "the header chunk comes first");
    }

    #[test]
    fn the_reply_carries_the_file_and_its_size() {
        let file = png_header(800, 500);
        assert_eq!(reply(&file).unwrap(), json!({"mimeType": "image/png", "data": base64(&file), "width": 800, "height": 500}));
        assert_eq!(reply(b"not a picture"), Err("The window returned a picture that is not a PNG file.".to_string()));
    }
}
