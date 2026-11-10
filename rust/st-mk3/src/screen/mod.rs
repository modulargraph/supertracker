//! The two displays: display lists from the interface are drawn into a 480 × 272 bitmap, converted
//! to the device's 16-bit pixels and sent as the changed region in one bulk packet.
//!
//! | Module | Contents |
//! |---|---|
//! | [`ops`] | the display list (`mk3.screen` payloads) |
//! | [`render`] | the software rasterizer |
//! | [`font`] | text: system fonts through `ab_glyph`, and a built-in bitmap font |
//! | [`encode`] | pixel formats, the dirty region and the bulk packet |
//! | [`png`] | a minimal PNG writer for screen dumps |
//!
//! Everything here is pure (no device access), and deterministic with the built-in font.

pub mod encode;
pub mod font;
pub mod ops;
pub mod png;
pub mod render;

/// Width of a display in pixels.
pub const WIDTH: usize = 480;
/// Height of a display in pixels.
pub const HEIGHT: usize = 272;
/// Pixels of a display.
pub const PIXEL_COUNT: usize = WIDTH * HEIGHT;
