//! Pictures of the window for AI clients (`app.screenshot`, the MCP tool `screenshot`).
//!
//! The picture is taken by the web view itself, so it shows what the user sees of the interface:
//!
//! - Linux: WebKitGTK's snapshot of the visible part of the page, scaled and written as PNG by cairo.
//!   Exercised under Xvfb by `tests/host_e2e/window_smoke.sh`.
//! - macOS: `-[WKWebView takeSnapshotWithConfiguration:completionHandler:]`, drawn at the wanted
//!   size and written as PNG by AppKit. Type-checked from Linux; it has not run on a Mac yet.
//!
//! The request comes from a thread of the control server and the web view belongs to the window's
//! thread, so [`provider`] posts a [`Request`] to the event loop and waits for the answer, for at
//! most [`PATIENCE`]: a window that is busy costs the client an error, not a hang. The web view
//! answers later still, from its own callback; an answer nobody waits for any more is dropped.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use st_host::areas::agent::{ScreenshotProvider, SCREENSHOT_MAX_WIDTH};

/// How long a client waits for the window to produce the picture.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// One picture to take, on its way to the window's thread.
pub struct Request {
    /// The widest picture the client asked for, in pixels, if it asked.
    pub width: Option<u32>,
    answer: mpsc::Sender<Result<Vec<u8>, String>>,
}

impl Request {
    /// Hands the PNG file, or why there is none, to the waiting client.
    pub fn answer(self, picture: Result<Vec<u8>, String>) {
        // A client that stopped waiting has been told so already.
        let _ = self.answer.send(picture);
    }
}

/// What AI control calls for a picture: `post` sends the request to the window's event loop
/// (false when the loop has ended), and the caller waits up to `patience` for the PNG file.
pub fn provider(post: impl Fn(Request) -> bool + Send + Sync + 'static, patience: Duration) -> ScreenshotProvider {
    Box::new(move |width| {
        let (answer, picture) = mpsc::channel();
        if !post(Request { width, answer }) {
            return Err("The SuperTracker window is closing.".to_string());
        }
        match picture.recv_timeout(patience) {
            Ok(picture) => picture,
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "The SuperTracker window did not produce a screenshot within {} s. It is busy, or a dialog or a menu is open in it.",
                patience.as_secs()
            )),
            Err(RecvTimeoutError::Disconnected) => Err("The SuperTracker window closed before the screenshot was taken.".to_string()),
        }
    })
}

/// The size of the picture in pixels.
///
/// `logical_width` is the width of the window's content in the units the page is laid out in,
/// `pixels` the size it is drawn at on the display (larger on a high-density display), `requested`
/// the widest picture the client asked for. Without a request the picture is as wide as the layout,
/// so text has the size the page gave it. It is never wider than the client asked, than the
/// widest picture a client gets, or than the drawn pixels (nothing is enlarged), and it keeps the
/// window's proportions.
pub fn target_size(logical_width: f64, pixels: (u32, u32), requested: Option<u32>) -> (u32, u32) {
    let (pixel_width, pixel_height) = (pixels.0.max(1), pixels.1.max(1));
    let wanted = requested.unwrap_or(logical_width.round().clamp(1.0, f64::from(u32::MAX)) as u32);
    let width = wanted.min(SCREENSHOT_MAX_WIDTH).min(pixel_width).max(1);
    let height = (f64::from(pixel_height) * f64::from(width) / f64::from(pixel_width)).round().max(1.0) as u32;
    (width, height)
}

/// Starts taking the picture of `web_view` for `request`, which is answered from the web view's
/// callback. Call on the window's thread.
#[cfg(target_os = "linux")]
pub fn take(web_view: &wry::WebView, request: Request) {
    use gtk::prelude::WidgetExt;
    use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
    use wry::WebViewExtUnix;

    let view = web_view.webview();
    let logical = (view.allocated_width(), view.allocated_height());
    view.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE, None::<&gtk::gio::Cancellable>, move |snapshot| {
        let picture = match snapshot {
            Ok(surface) => png_of_surface(&surface, logical, request.width),
            Err(error) => Err(format!("The window's content could not be captured: {}.", error.message().trim_end_matches('.'))),
        };
        request.answer(picture);
    });
}

/// The snapshot `surface` of a web view whose layout size is `logical`, as a PNG file no wider
/// than `requested`.
#[cfg(target_os = "linux")]
fn png_of_surface(surface: &cairo::Surface, logical: (i32, i32), requested: Option<u32>) -> Result<Vec<u8>, String> {
    let failed = |error: &dyn std::fmt::Display| format!("The screenshot could not be drawn: {error}.");
    // The surface's own units are pixels divided by its device scale (2 on a high-density display).
    let (scale_x, scale_y) = surface.device_scale();
    let pixels = match cairo::ImageSurface::try_from(surface.clone()) {
        Ok(image) => (image.width(), image.height()),
        // A surface without pixels of its own covers the web view.
        Err(_) => ((f64::from(logical.0) * scale_x).round() as i32, (f64::from(logical.1) * scale_y).round() as i32),
    };
    if logical.0 < 1 || pixels.0 < 1 || pixels.1 < 1 {
        return Err("The window has no visible content to take a picture of.".to_string());
    }
    let (width, height) = target_size(f64::from(logical.0), (pixels.0 as u32, pixels.1 as u32), requested);
    let picture = cairo::ImageSurface::create(cairo::Format::Rgb24, width as i32, height as i32).map_err(|error| failed(&error))?;
    {
        let context = cairo::Context::new(&picture).map_err(|error| failed(&error))?;
        // The page is opaque; where a snapshot is not, the window's background shows through.
        let (red, green, blue, _) = crate::window::BACKGROUND;
        context.set_source_rgb(f64::from(red) / 255.0, f64::from(green) / 255.0, f64::from(blue) / 255.0);
        context.paint().map_err(|error| failed(&error))?;
        context.scale(f64::from(width) * scale_x / f64::from(pixels.0), f64::from(height) * scale_y / f64::from(pixels.1));
        context.set_source_surface(surface, 0.0, 0.0).map_err(|error| failed(&error))?;
        // Shrinking averages the pixels (cairo's default filter); the edge repeats instead of fading out.
        context.source().set_extend(cairo::Extend::Pad);
        context.paint().map_err(|error| failed(&error))?;
    }
    let mut png = Vec::new();
    picture.write_to_png(&mut png).map_err(|error| failed(&error))?;
    Ok(png)
}

/// Starts taking the picture of `web_view` for `request`, which is answered from the web view's
/// completion handler. `scale_factor` is the window's (pixels per point). Call on the main thread.
#[cfg(target_os = "macos")]
pub fn take(web_view: &wry::WebView, scale_factor: f64, request: Request) {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSImage;
    use objc2_foundation::NSError;
    use objc2_web_kit::WKSnapshotConfiguration;
    use wry::WebViewExtMacOS;

    let Some(main_thread) = MainThreadMarker::new() else {
        request.answer(Err("The screenshot was not requested on the window's thread.".to_string()));
        return;
    };
    let view = web_view.webview();
    // SAFETY: a plain `new` on the main thread; the defaults are the whole view, after pending updates.
    let configuration = unsafe { WKSnapshotConfiguration::new(main_thread) };
    // WebKit calls the handler once; a block can be called more often, so the request is taken out.
    let request = std::cell::Cell::new(Some(request));
    let done = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let Some(request) = request.take() else { return };
        // SAFETY: WebKit passes an image or nil and an error or nil, both alive during the call.
        let (image, error) = unsafe { (image.as_ref(), error.as_ref()) };
        let picture = match (image, error) {
            (Some(image), _) => png_of_image(image, scale_factor, request.width),
            (None, Some(error)) => Err(format!("The window's content could not be captured: {}", error.localizedDescription())),
            (None, None) => Err("The window's content could not be captured.".to_string()),
        };
        request.answer(picture);
    });
    // SAFETY: the web view is alive and this is the main thread; the handler is copied by WebKit.
    unsafe { view.takeSnapshotWithConfiguration_completionHandler(Some(&configuration), &done) };
}

/// The snapshot `image` (its size is in points) as a PNG file no wider than `requested`.
#[cfg(target_os = "macos")]
fn png_of_image(image: &objc2_app_kit::NSImage, scale_factor: f64, requested: Option<u32>) -> Result<Vec<u8>, String> {
    use objc2::AnyThread;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSCompositingOperation, NSDeviceRGBColorSpace, NSGraphicsContext, NSImageInterpolation, NSRectFill,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize};

    let points = image.size();
    if !(points.width >= 1.0 && points.height >= 1.0) {
        return Err("The window has no visible content to take a picture of.".to_string());
    }
    let pixels = ((points.width * scale_factor).round() as u32, (points.height * scale_factor).round() as u32);
    let (width, height) = target_size(points.width, pixels, requested);

    // A bitmap of exactly the wanted pixels, which the snapshot is drawn into.
    // SAFETY: no planes are passed (AppKit allocates them), and the format is 8-bit RGBA, which
    // AppKit can draw into; the colour space name is AppKit's own constant.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width as isize,
            height as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }
    .ok_or("The screenshot could not be allocated.")?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap).ok_or("The screenshot could not be drawn.")?;
    let whole = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(f64::from(width), f64::from(height)));
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    context.setImageInterpolation(NSImageInterpolation::High);
    // The page is opaque; where a snapshot is not, the window's background shows through.
    let (red, green, blue, _) = crate::window::BACKGROUND;
    NSColor::colorWithSRGBRed_green_blue_alpha(f64::from(red) / 255.0, f64::from(green) / 255.0, f64::from(blue) / 255.0, 1.0).setFill();
    NSRectFill(whole);
    // An empty source rectangle means the whole image.
    image.drawInRect_fromRect_operation_fraction(whole, NSRect::ZERO, NSCompositingOperation::SourceOver, 1.0);
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();

    // SAFETY: an empty properties dictionary is valid for every file type.
    let png = unsafe { bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }
        .ok_or("The screenshot could not be encoded as PNG.")?;
    Ok(png.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_is_as_wide_as_the_layout_unless_the_client_wants_it_narrower() {
        // An ordinary display: one pixel per layout unit.
        assert_eq!(target_size(1500.0, (1500, 915), None), (1500, 915));
        assert_eq!(target_size(1500.0, (1500, 915), Some(750)), (750, 458), "915 / 2 rounds up");
        assert_eq!(target_size(1500.0, (1500, 915), Some(640)), (640, 390));
        assert_eq!(target_size(1500.0, (1500, 915), Some(1500)), (1500, 915));
        assert_eq!(target_size(1500.0, (1500, 915), Some(2560)), (1500, 915), "nothing is enlarged");
        // A high-density display draws two pixels per unit: the default is the layout's width, and
        // a client may ask for up to the drawn pixels.
        assert_eq!(target_size(1500.0, (3000, 1830), None), (1500, 915));
        assert_eq!(target_size(1500.0, (3000, 1830), Some(2560)), (2560, 1562));
        assert_eq!(target_size(1500.0, (3000, 1830), Some(320)), (320, 195));
        // A very wide window is limited to the widest picture a client gets.
        assert_eq!(target_size(3440.0, (3440, 1400), None), (2560, 1042));
        assert_eq!(target_size(3440.0, (3440, 1400), Some(4000)), (2560, 1042));
        // Degenerate sizes still give a picture of at least one pixel.
        assert_eq!(target_size(0.0, (0, 0), None), (1, 1));
        assert_eq!(target_size(1500.0, (1500, 1), Some(320)), (320, 1));
        assert_eq!(target_size(f64::NAN, (800, 600), None), (1, 1));
    }

    #[test]
    fn the_provider_passes_the_request_to_the_window_and_returns_its_answer() {
        let take = provider(
            |request| {
                // The window answers later, from another call of its loop.
                std::thread::spawn(move || {
                    let picture = match request.width {
                        Some(640) => Err("The window's content could not be captured: no memory.".to_string()),
                        width => Ok(format!("picture {width:?}").into_bytes()),
                    };
                    request.answer(picture);
                });
                true
            },
            Duration::from_secs(30),
        );
        assert_eq!(take(None), Ok(b"picture None".to_vec()));
        assert_eq!(take(Some(800)), Ok(b"picture Some(800)".to_vec()));
        assert_eq!(take(Some(640)), Err("The window's content could not be captured: no memory.".to_string()));
    }

    #[test]
    fn a_window_that_is_gone_or_busy_is_an_error_not_a_hang() {
        // The event loop has ended: the request cannot be posted.
        let closing = provider(|_| false, Duration::from_secs(30));
        assert_eq!(closing(None), Err("The SuperTracker window is closing.".to_string()));

        // The loop took the request and went away without answering.
        let closed = provider(|request| { drop(request); true }, Duration::from_secs(30));
        assert_eq!(closed(None), Err("The SuperTracker window closed before the screenshot was taken.".to_string()));

        // The loop is busy: the request waits in its queue (kept here) past the client's patience.
        let (queue, queued) = mpsc::channel();
        let busy = provider(move |request| queue.send(request).is_ok(), Duration::from_secs(1));
        assert_eq!(
            busy(Some(800)),
            Err("The SuperTracker window did not produce a screenshot within 1 s. It is busy, or a dialog or a menu is open in it.".to_string())
        );
        // The answer that comes after all goes nowhere.
        queued.recv().unwrap().answer(Ok(vec![1, 2, 3]));
    }

    /// A surface of `width`×`height` pixels whose left half is red and right half blue.
    #[cfg(target_os = "linux")]
    fn two_halves(width: i32, height: i32, device_scale: f64) -> cairo::ImageSurface {
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap();
        {
            let context = cairo::Context::new(&surface).unwrap();
            context.set_source_rgb(1.0, 0.0, 0.0);
            context.rectangle(0.0, 0.0, f64::from(width) / 2.0, f64::from(height));
            context.fill().unwrap();
            context.set_source_rgb(0.0, 0.0, 1.0);
            context.rectangle(f64::from(width) / 2.0, 0.0, f64::from(width) / 2.0, f64::from(height));
            context.fill().unwrap();
        }
        surface.set_device_scale(device_scale, device_scale);
        surface
    }

    /// Decodes a PNG file: its size and the red, green and blue of the pixel at `(x, y)`.
    #[cfg(target_os = "linux")]
    fn decoded(png: &[u8], x: i32, y: i32) -> ((i32, i32), (u8, u8, u8)) {
        assert_eq!(png[..8], [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A], "a PNG file");
        let mut surface = cairo::ImageSurface::create_from_png(&mut std::io::Cursor::new(png)).expect("cairo reads the file back");
        let (size, stride) = ((surface.width(), surface.height()), surface.stride());
        let data = surface.data().unwrap();
        // Native-endian 0x00RRGGBB.
        let at = (y * stride + x * 4) as usize;
        let pixel = u32::from_ne_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        (size, ((pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8))
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_snapshot_becomes_a_png_of_the_window_or_scaled_down_to_the_requested_width() {
        const RED: (u8, u8, u8) = (255, 0, 0);
        const BLUE: (u8, u8, u8) = (0, 0, 255);
        let snapshot = two_halves(1200, 800, 1.0);

        let whole = png_of_surface(&snapshot, (1200, 800), None).unwrap();
        assert_eq!(decoded(&whole, 10, 10), ((1200, 800), RED));
        assert_eq!(decoded(&whole, 1190, 790), ((1200, 800), BLUE));
        assert_eq!(decoded(&whole, 599, 400).1, RED, "pixels are copied, not blurred, at the window's own size");
        assert_eq!(decoded(&whole, 600, 400).1, BLUE);
        assert_eq!(png_of_surface(&snapshot, (1200, 800), Some(2560)).unwrap(), whole, "a wider request does not enlarge the picture");

        let small = png_of_surface(&snapshot, (1200, 800), Some(600)).unwrap();
        assert_eq!(decoded(&small, 5, 5), ((600, 400), RED));
        assert_eq!(decoded(&small, 594, 394), ((600, 400), BLUE));
        assert_eq!(decoded(&small, 0, 0).1, RED, "the edge keeps its colour");
        assert_eq!(decoded(&small, 599, 399).1, BLUE);
        let odd = png_of_surface(&snapshot, (1200, 800), Some(500)).unwrap();
        assert_eq!(decoded(&odd, 499, 332), ((500, 333), BLUE), "the height follows the proportions");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_high_density_snapshot_is_scaled_to_the_layout_width_by_default() {
        const RED: (u8, u8, u8) = (255, 0, 0);
        const BLUE: (u8, u8, u8) = (0, 0, 255);
        // Two pixels per layout unit, as the surface says itself...
        let dense = two_halves(2400, 1600, 2.0);
        let picture = png_of_surface(&dense, (1200, 800), None).unwrap();
        assert_eq!(decoded(&picture, 300, 400), ((1200, 800), RED));
        assert_eq!(decoded(&picture, 900, 400), ((1200, 800), BLUE));
        assert_eq!(decoded(&png_of_surface(&dense, (1200, 800), Some(2400)).unwrap(), 2399, 1599), ((2400, 1600), BLUE), "every drawn pixel on request");
        // ... or not: the pixels are what counts.
        let unmarked = two_halves(2400, 1600, 1.0);
        let picture = png_of_surface(&unmarked, (1200, 800), None).unwrap();
        assert_eq!(decoded(&picture, 300, 400), ((1200, 800), RED));
        assert_eq!(decoded(&picture, 900, 400), ((1200, 800), BLUE));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn transparent_parts_show_the_window_background_and_an_empty_view_is_refused() {
        let empty = cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300).unwrap();
        let picture = png_of_surface(&empty, (400, 300), None).unwrap();
        let (red, green, blue, _) = crate::window::BACKGROUND;
        assert_eq!(decoded(&picture, 200, 150), ((400, 300), (red, green, blue)));

        // A surface that is not an image covers the web view's layout size.
        let recording = cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None).unwrap();
        {
            let context = cairo::Context::new(&recording).unwrap();
            context.set_source_rgb(0.0, 1.0, 0.0);
            context.paint().unwrap();
        }
        let picture = png_of_surface(&recording, (400, 300), Some(320)).unwrap();
        assert_eq!(decoded(&picture, 160, 120), ((320, 240), (0, 255, 0)));

        assert_eq!(png_of_surface(&empty, (0, 0), None), Err("The window has no visible content to take a picture of.".to_string()));
    }
}
