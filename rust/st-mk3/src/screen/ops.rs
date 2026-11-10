//! The display list: what the interface asks to be drawn on a screen.

use serde_json::Value;

use crate::protocol::number;

/// A colour with alpha, each 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    /// Red.
    pub r: f64,
    /// Green.
    pub g: f64,
    /// Blue.
    pub b: f64,
    /// Opacity.
    pub a: f64,
}

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };

    /// Parses `"#rgb"`, `"#rgba"`, `"#rrggbb"` or `"#rrggbbaa"`. `"transparent"` and `"none"` are
    /// no colour.
    pub fn from_hex(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == "transparent" || text == "none" {
            return None;
        }
        let text = text.strip_prefix('#').unwrap_or(text);
        if !text.is_ascii() {
            return None;
        }
        let expanded: String = if text.len() == 3 || text.len() == 4 { text.chars().flat_map(|digit| [digit, digit]).collect() } else { text.to_string() };
        if expanded.len() != 6 && expanded.len() != 8 {
            return None;
        }
        let number = u64::from_str_radix(&expanded, 16).ok()?;
        let rgba = if expanded.len() == 6 { number << 8 | 0xFF } else { number };
        let channel = |shift: u32| ((rgba >> shift) & 0xFF) as f64 / 255.0;
        Some(Color { r: channel(24), g: channel(16), b: channel(8), a: channel(0) })
    }

    fn from_value(value: Option<&Value>) -> Option<Self> {
        value.and_then(Value::as_str).and_then(Self::from_hex)
    }
}

/// Horizontal alignment of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// Starts at `x`.
    Left,
    /// Centred on `x`, or inside `[x, x + width]`.
    Center,
    /// Ends at `x`, or at `x + width`.
    Right,
}

/// A rectangle in screen coordinates (origin top left, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// One drawing operation.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawOp {
    /// Fills the screen, replacing what is there.
    Clear(Color),
    /// A filled rectangle, with rounded corners when `radius` is above zero.
    Rect {
        /// Where.
        rect: Rect,
        /// Fill.
        color: Color,
        /// Corner radius.
        radius: f64,
    },
    /// A rectangle's outline, drawn inside the rectangle.
    Frame {
        /// Where.
        rect: Rect,
        /// Colour.
        color: Color,
        /// Line width.
        width: f64,
    },
    /// A straight line with square ends.
    Line {
        /// Start.
        from: (f64, f64),
        /// End.
        to: (f64, f64),
        /// Colour.
        color: Color,
        /// Line width.
        width: f64,
    },
    /// One line of text. `y` is the top of the line. With `width` the text is cut with an ellipsis
    /// and aligned inside `[x, x + width]`; without it, centred or right-aligned text is placed
    /// around or before `x`.
    Text {
        /// Anchor.
        x: f64,
        /// Top of the line.
        y: f64,
        /// The text.
        text: String,
        /// Font size in pixels.
        size: f64,
        /// Colour.
        color: Color,
        /// Bold weight.
        bold: bool,
        /// Alignment.
        align: TextAlign,
        /// Available width.
        width: Option<f64>,
        /// Fixed-pitch font.
        mono: bool,
    },
    /// A level bar: horizontal when wider than high (fills from the left), else vertical (fills
    /// from the bottom).
    Bar {
        /// Where.
        rect: Rect,
        /// Level, 0 to 1.
        value: f64,
        /// Colour of the filled part.
        color: Color,
        /// Colour of the whole bar, drawn first.
        background: Option<Color>,
    },
    /// A knob ring centred at `center`: a 270° sweep from bottom left, clockwise, with round ends.
    Arc {
        /// Centre.
        center: (f64, f64),
        /// Radius of the ring's middle line.
        radius: f64,
        /// How much of the sweep is drawn, 0 to 1.
        value: f64,
        /// Colour of the drawn part.
        color: Color,
        /// Colour of the whole sweep, drawn first.
        background: Option<Color>,
        /// Line width.
        width: f64,
    },
}

impl DrawOp {
    /// Parses a display list as the interface sends it: an array of objects with an `op` name
    /// (`clear`, `rect`, `frame`, `line`, `text`, `bar`, `arc`). Malformed operations are skipped.
    pub fn parse(list: &[Value]) -> Vec<DrawOp> {
        list.iter().filter_map(Self::parse_one).collect()
    }

    fn parse_one(item: &Value) -> Option<DrawOp> {
        let op = item.as_object()?;
        let n = |key: &str| op.get(key).and_then(number);
        let color = Color::from_value(op.get("color"));
        let rect = || Some(Rect { x: n("x")?, y: n("y")?, w: n("w")?, h: n("h")? });
        match op.get("op")?.as_str()? {
            "clear" => Some(DrawOp::Clear(color.unwrap_or(Color::BLACK))),
            "rect" => Some(DrawOp::Rect { rect: rect()?, color: color?, radius: n("radius").unwrap_or(0.0) }),
            "frame" => Some(DrawOp::Frame { rect: rect()?, color: color?, width: n("width").unwrap_or(1.0) }),
            "line" => Some(DrawOp::Line { from: (n("x1")?, n("y1")?), to: (n("x2")?, n("y2")?), color: color?, width: n("width").unwrap_or(1.0) }),
            "text" => {
                let text = match op.get("text") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                };
                let align = match op.get("align").and_then(Value::as_str) {
                    Some("center") => TextAlign::Center,
                    Some("right") => TextAlign::Right,
                    _ => TextAlign::Left,
                };
                Some(DrawOp::Text {
                    x: n("x")?,
                    y: n("y")?,
                    text,
                    size: n("size").unwrap_or(14.0),
                    color: color?,
                    bold: op.get("weight").and_then(Value::as_str) == Some("bold"),
                    align,
                    width: n("width"),
                    mono: op.get("mono").and_then(Value::as_bool).unwrap_or(false),
                })
            }
            "bar" => Some(DrawOp::Bar {
                rect: rect()?,
                value: n("value").unwrap_or(0.0).clamp(0.0, 1.0),
                color: color?,
                background: Color::from_value(op.get("background")),
            }),
            "arc" => Some(DrawOp::Arc {
                center: (n("x")?, n("y")?),
                radius: n("r")?,
                value: n("value").unwrap_or(0.0).clamp(0.0, 1.0),
                color: color?,
                background: Color::from_value(op.get("background")),
                width: n("width").unwrap_or(4.0),
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn colours() {
        assert_eq!(Color::from_hex("#ff0000"), Some(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }));
        assert_eq!(Color::from_hex(" #0f0 "), Some(Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }));
        assert_eq!(Color::from_hex("#0000ff80").map(|color| (color.b, (color.a * 255.0).round())), Some((1.0, 128.0)));
        assert_eq!(Color::from_hex("#f008").map(|color| (color.r, (color.a * 255.0).round())), Some((1.0, 136.0)));
        for bad in ["transparent", "none", "", "#12", "#12345", "red", "#gggggg", "#ééé"] {
            assert_eq!(Color::from_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn display_lists_are_parsed_leniently() {
        let ops = DrawOp::parse(&[
            json!({"op": "clear"}),
            json!({"op": "rect", "x": 1, "y": 2, "w": 3, "h": 4, "color": "#fff", "radius": 2}),
            json!({"op": "rect", "x": 1, "y": 2, "w": 3, "color": "#fff"}),
            json!({"op": "rect", "x": 1, "y": 2, "w": 3, "h": 4}),
            json!({"op": "text", "x": 0, "y": 0, "text": 42, "color": "#fff", "weight": "bold", "align": "right", "width": 50, "mono": true}),
            json!({"op": "text", "x": 0, "y": 0, "color": "#fff"}),
            json!({"op": "bar", "x": 0, "y": 0, "w": 10, "h": 2, "value": 7, "color": "#fff", "background": "none"}),
            json!({"op": "arc", "x": 5, "y": 5, "r": 3, "color": "#fff"}),
            json!({"op": "nonsense"}),
            json!("text"),
            json!({"x": 1}),
        ]);
        assert_eq!(ops.len(), 6, "{ops:?}");
        assert_eq!(ops[0], DrawOp::Clear(Color::BLACK));
        assert!(matches!(&ops[1], DrawOp::Rect { radius, .. } if *radius == 2.0));
        assert!(matches!(&ops[2], DrawOp::Text { text, bold: true, align: TextAlign::Right, width: Some(w), mono: true, size, .. } if text == "42" && *w == 50.0 && *size == 14.0));
        assert!(matches!(&ops[3], DrawOp::Text { text, bold: false, align: TextAlign::Left, width: None, .. } if text.is_empty()));
        assert!(matches!(&ops[4], DrawOp::Bar { value, background: None, .. } if *value == 1.0));
        assert!(matches!(&ops[5], DrawOp::Arc { value, width, .. } if *value == 0.0 && *width == 4.0));
    }
}
