//! The software rasterizer: display lists to a 480 × 272 bitmap, anti-aliased, with the origin at
//! the top left and y pointing down like the interface's canvas.

use std::sync::Arc;

use super::encode::PixelFormat;
use super::font::Fonts;
use super::ops::{Color, DrawOp, Rect, TextAlign};
use super::{HEIGHT, PIXEL_COUNT, WIDTH};

/// Renders display lists. One renderer is reused for every frame.
pub struct Renderer {
    /// Red, green and blue per pixel, 0 to 1, rows from the top.
    canvas: Vec<[f32; 3]>,
    fonts: Arc<Fonts>,
}

/// How much of the unit pixel at `(column, row)` an axis-aligned rectangle covers.
fn box_coverage(column: i32, row: i32, x0: f64, y0: f64, x1: f64, y1: f64) -> f64 {
    let overlap = |low: f64, high: f64, cell: i32| (high.min(f64::from(cell) + 1.0) - low.max(f64::from(cell))).clamp(0.0, 1.0);
    overlap(x0, x1, column) * overlap(y0, y1, row)
}

/// Coverage from a signed distance (negative inside) at the pixel's centre.
fn edge(distance: f64) -> f64 {
    (0.5 - distance).clamp(0.0, 1.0)
}

impl Renderer {
    /// A renderer that draws text with `fonts`.
    pub fn new(fonts: Arc<Fonts>) -> Self {
        Renderer { canvas: vec![[0.0; 3]; PIXEL_COUNT], fonts }
    }

    /// The fonts in use.
    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    fn blend(&mut self, column: i32, row: i32, color: Color, coverage: f64) {
        if column < 0 || row < 0 || column >= WIDTH as i32 || row >= HEIGHT as i32 {
            return;
        }
        let alpha = (color.a * coverage).clamp(0.0, 1.0) as f32;
        if alpha <= 0.0 {
            return;
        }
        let pixel = &mut self.canvas[row as usize * WIDTH + column as usize];
        for (channel, source) in pixel.iter_mut().zip([color.r, color.g, color.b]) {
            *channel += (source as f32 - *channel) * alpha;
        }
    }

    /// Calls `coverage(column, row)` for every pixel of the screen inside the bounds and blends
    /// the colour by the result.
    fn fill(&mut self, bounds: (f64, f64, f64, f64), color: Color, coverage: impl Fn(i32, i32) -> f64) {
        let (x0, y0, x1, y1) = bounds;
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return;
        }
        let (first_column, last_column) = (x0.floor().max(0.0) as i32, x1.ceil().min(WIDTH as f64) as i32);
        let (first_row, last_row) = (y0.floor().max(0.0) as i32, y1.ceil().min(HEIGHT as f64) as i32);
        for row in first_row..last_row {
            for column in first_column..last_column {
                let amount = coverage(column, row);
                if amount > 0.0 {
                    self.blend(column, row, color, amount);
                }
            }
        }
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let (x0, y0, x1, y1) = (rect.x.min(rect.x + rect.w), rect.y.min(rect.y + rect.h), rect.x.max(rect.x + rect.w), rect.y.max(rect.y + rect.h));
        self.fill((x0, y0, x1, y1), color, |column, row| box_coverage(column, row, x0, y0, x1, y1));
    }

    /// Draws a display list on a black screen.
    pub fn render(&mut self, ops: &[DrawOp]) {
        self.canvas.fill([0.0; 3]);
        for op in ops {
            match op {
                DrawOp::Clear(color) => {
                    // Replaces what is there; a translucent colour shows over black.
                    let alpha = color.a as f32;
                    self.canvas.fill([color.r as f32 * alpha, color.g as f32 * alpha, color.b as f32 * alpha]);
                }
                DrawOp::Rect { rect, color, radius } => {
                    if *radius > 0.0 && rect.w > 0.0 && rect.h > 0.0 {
                        let radius = radius.min(rect.w.min(rect.h) / 2.0);
                        let (center_x, center_y, half_w, half_h) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, rect.w / 2.0, rect.h / 2.0);
                        self.fill((rect.x, rect.y, rect.x + rect.w, rect.y + rect.h), *color, |column, row| {
                            let (qx, qy) = ((f64::from(column) + 0.5 - center_x).abs() - (half_w - radius), (f64::from(row) + 0.5 - center_y).abs() - (half_h - radius));
                            edge(qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - radius)
                        });
                    } else {
                        self.fill_rect(*rect, *color);
                    }
                }
                DrawOp::Frame { rect, color, width } => {
                    // The outline lies inside the rectangle: the rectangle less itself inset by the width.
                    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
                    let width = width.max(0.0);
                    self.fill((x0, y0, x1, y1), *color, |column, row| {
                        let outer = box_coverage(column, row, x0, y0, x1, y1);
                        let inner = if x1 - x0 > 2.0 * width && y1 - y0 > 2.0 * width { box_coverage(column, row, x0 + width, y0 + width, x1 - width, y1 - width) } else { 0.0 };
                        (outer - inner).max(0.0)
                    });
                }
                DrawOp::Line { from, to, color, width } => self.line(*from, *to, *color, *width),
                DrawOp::Text { x, y, text, size, color, bold, align, width, mono } => self.text(*x, *y, text, *size, *color, *bold, *align, *width, *mono),
                DrawOp::Bar { rect, value, color, background } => {
                    if let Some(background) = background {
                        self.fill_rect(*rect, *background);
                    }
                    let filled = if rect.w >= rect.h {
                        Rect { w: rect.w * value, ..*rect }
                    } else {
                        Rect { y: rect.y + rect.h - rect.h * value, h: rect.h * value, ..*rect }
                    };
                    self.fill_rect(filled, *color);
                }
                DrawOp::Arc { center, radius, value, color, background, width } => {
                    if let Some(background) = background {
                        self.arc(*center, *radius, 1.0, *background, *width);
                    }
                    if *value > 0.0 {
                        self.arc(*center, *radius, *value, *color, *width);
                    }
                }
            }
        }
    }

    fn line(&mut self, from: (f64, f64), to: (f64, f64), color: Color, width: f64) {
        let half = width.max(0.0) / 2.0;
        if from.1 == to.1 {
            // Horizontal and vertical lines get exact edges.
            let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
            return self.fill((x0, from.1 - half, x1, from.1 + half), color, |column, row| box_coverage(column, row, x0, from.1 - half, x1, from.1 + half));
        }
        if from.0 == to.0 {
            let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
            return self.fill((from.0 - half, y0, from.0 + half, y1), color, |column, row| box_coverage(column, row, from.0 - half, y0, from.0 + half, y1));
        }
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = dx.hypot(dy);
        let (ux, uy) = (dx / length, dy / length);
        let (mid_x, mid_y) = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
        let bounds = (from.0.min(to.0) - half - 1.0, from.1.min(to.1) - half - 1.0, from.0.max(to.0) + half + 1.0, from.1.max(to.1) + half + 1.0);
        self.fill(bounds, color, |column, row| {
            let (px, py) = (f64::from(column) + 0.5 - mid_x, f64::from(row) + 0.5 - mid_y);
            // Distance to a box of the line's length and width, turned along the line.
            let (qx, qy) = ((px * ux + py * uy).abs() - length / 2.0, (-px * uy + py * ux).abs() - half);
            edge(qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0))
        });
    }

    /// A ring segment with round ends: from 135° (bottom left) clockwise over `value` of 270°.
    fn arc(&mut self, center: (f64, f64), radius: f64, value: f64, color: Color, width: f64) {
        let half = width.max(0.0) / 2.0;
        let sweep = 270.0 * value.clamp(0.0, 1.0);
        let end_point = |degrees: f64| (center.0 + radius * degrees.to_radians().cos(), center.1 + radius * degrees.to_radians().sin());
        let (start, end) = (end_point(135.0), end_point(135.0 + sweep));
        let reach = radius.abs() + half + 1.0;
        self.fill((center.0 - reach, center.1 - reach, center.0 + reach, center.1 + reach), color, |column, row| {
            let (px, py) = (f64::from(column) + 0.5, f64::from(row) + 0.5);
            let (dx, dy) = (px - center.0, py - center.1);
            // With y pointing down, increasing angles run clockwise on the screen.
            let along = (dy.atan2(dx).to_degrees() - 135.0).rem_euclid(360.0);
            let distance = if along <= sweep { (dx.hypot(dy) - radius).abs() } else { (px - start.0).hypot(py - start.1).min((px - end.0).hypot(py - end.1)) };
            edge(distance - half)
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, x: f64, y: f64, text: &str, size: f64, color: Color, bold: bool, align: TextAlign, width: Option<f64>, mono: bool) {
        if text.is_empty() || size <= 0.0 || !size.is_finite() {
            return;
        }
        let fonts = self.fonts.clone();
        let mut line = fonts.line(text, size, bold, mono);
        if let Some(available) = width {
            if line.width > available {
                // Cut at the end with an ellipsis: the longest beginning that still fits with it.
                let characters: Vec<char> = text.chars().collect();
                let mut kept = characters.len();
                loop {
                    kept = kept.saturating_sub(1);
                    let candidate: String = characters[..kept].iter().collect::<String>().trim_end().to_string() + "…";
                    line = fonts.line(&candidate, size, bold, mono);
                    if line.width <= available || kept == 0 {
                        break;
                    }
                }
            }
        }
        let start = match (align, width) {
            (TextAlign::Left, _) => x,
            (TextAlign::Center, Some(width)) => x + (width - line.width) / 2.0,
            (TextAlign::Right, Some(width)) => x + width - line.width,
            (TextAlign::Center, None) => x - line.width / 2.0,
            (TextAlign::Right, None) => x - line.width,
        };
        let baseline = y + fonts.ascent(size, bold, mono);
        // Text with a width never draws outside it (one pixel of slack for anti-aliasing).
        let clip = width.map(|width| (x - 1.0, y - size, x + width + 1.0, y + 2.0 * size));
        line.draw(start, baseline, &mut |column, row, coverage| {
            if let Some((x0, y0, x1, y1)) = clip {
                let (px, py) = (f64::from(column), f64::from(row));
                if px < x0.floor() || px >= x1.ceil() || py < y0.floor() || py >= y1.ceil() {
                    return;
                }
            }
            self.blend(column, row, color, f64::from(coverage));
        });
    }

    /// The bitmap as 8-bit red, green and blue, rows from the top (for dumps).
    pub fn rgb(&self) -> Vec<u8> {
        self.canvas.iter().flat_map(|pixel| pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8)).collect()
    }

    /// The bitmap as 16-bit device pixels, rows from the top.
    pub fn framebuffer(&self, format: PixelFormat) -> Vec<u16> {
        self.canvas
            .iter()
            .map(|pixel| {
                let [r, g, b] = pixel.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
                format.pack(r, g, b)
            })
            .collect()
    }
}
