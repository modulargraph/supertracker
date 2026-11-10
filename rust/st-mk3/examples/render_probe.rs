//! Renders the probe's test pattern to PNG files: `render_probe <folder>` writes `system.png` (the
//! system's fonts) and `builtin.png` (the built-in font). A way to look at the screens' rendering
//! without a controller.

use std::sync::Arc;

use st_mk3::probe::screen_ops;
use st_mk3::screen::font::Fonts;
use st_mk3::screen::ops::DrawOp;
use st_mk3::screen::render::Renderer;
use st_mk3::screen::{png, HEIGHT, WIDTH};

fn main() {
    let folder = std::path::PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    for (name, fonts) in [("system", Fonts::system()), ("builtin", Fonts::builtin())] {
        println!("{name}: {}", fonts.description());
        let mut renderer = Renderer::new(Arc::new(fonts));
        renderer.render(&DrawOp::parse(&screen_ops("left", 7, 20)));
        let picture = png::encode_rgb(WIDTH, HEIGHT, &renderer.rgb()).expect("a picture");
        std::fs::write(folder.join(format!("{name}.png")), picture).expect("write the picture");
    }
}
