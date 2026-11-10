//! Text for the screens.
//!
//! The old host drew text with the system's interface font. This one loads the same kind of font
//! from the system's font files at run time ([`Fonts::system`]) and rasterizes it with `ab_glyph`,
//! so no font is shipped with the application. Where no font file is found, and in tests, a
//! built-in 5 × 7 bitmap font is used ([`Fonts::builtin`]): plain, but complete for ASCII and
//! deterministic everywhere.

use std::path::{Path, PathBuf};

use ab_glyph::{point, Font, FontArc, FontVec, PxScale, ScaleFont, VariableFont};

/// One typeface.
#[derive(Clone)]
enum Face {
    Outline(FontArc),
    Builtin,
}

/// The four typefaces a display list can ask for: regular and bold, proportional and fixed-pitch.
#[derive(Clone)]
pub struct Fonts {
    regular: Face,
    bold: Face,
    mono: Face,
    mono_bold: Face,
    description: String,
}

/// A font file and how to read it.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFile {
    /// The file.
    pub path: PathBuf,
    /// The face's index inside a font collection (`.ttc`); 0 for single fonts.
    pub index: u32,
    /// For a variable font, the weight to select (400 regular, 700 bold).
    pub weight: Option<f32>,
}

impl FontFile {
    fn new(path: &str, index: u32, weight: Option<f32>) -> Self {
        FontFile { path: PathBuf::from(path), index, weight }
    }

    fn load(&self) -> Option<FontArc> {
        let data = std::fs::read(&self.path).ok()?;
        let mut font = FontVec::try_from_vec_and_index(data, self.index).ok()?;
        if let Some(weight) = self.weight {
            // Ignored by fonts without a weight axis.
            font.set_variation(b"wght", weight);
        }
        Some(FontArc::new(font))
    }
}

/// Where each platform keeps the fonts closest to the old host's (the system interface font and
/// its fixed-pitch companion), in order of preference.
fn candidates(bold: bool, mono: bool) -> Vec<FontFile> {
    let weight = if bold { 700.0 } else { 400.0 };
    let pick = |regular: &str, bolder: &str| if bold { bolder.to_string() } else { regular.to_string() };
    let mut list = Vec::new();
    if cfg!(target_os = "macos") {
        if mono {
            list.push(FontFile::new("/System/Library/Fonts/SFNSMono.ttf", 0, Some(weight)));
            list.push(FontFile::new("/System/Library/Fonts/Menlo.ttc", u32::from(bold), None));
            list.push(FontFile::new("/System/Library/Fonts/Monaco.ttf", 0, None));
        } else {
            list.push(FontFile::new("/System/Library/Fonts/SFNS.ttf", 0, Some(weight)));
            list.push(FontFile::new("/System/Library/Fonts/Helvetica.ttc", u32::from(bold), None));
            list.push(FontFile::new("/System/Library/Fonts/Supplemental/Arial.ttf", 0, None));
        }
    } else if cfg!(target_os = "windows") {
        let name = if mono { pick("consola.ttf", "consolab.ttf") } else { pick("segoeui.ttf", "segoeuib.ttf") };
        list.push(FontFile::new(&format!(r"C:\Windows\Fonts\{name}"), 0, None));
    } else {
        let families: [(&str, &str, &str); 4] = if mono {
            [
                ("/usr/share/fonts/truetype/dejavu", "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"),
                ("/usr/share/fonts/truetype/liberation", "LiberationMono-Regular.ttf", "LiberationMono-Bold.ttf"),
                ("/usr/share/fonts/truetype/noto", "NotoSansMono-Regular.ttf", "NotoSansMono-Bold.ttf"),
                ("/usr/share/fonts/TTF", "DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"),
            ]
        } else {
            [
                ("/usr/share/fonts/truetype/dejavu", "DejaVuSans.ttf", "DejaVuSans-Bold.ttf"),
                ("/usr/share/fonts/truetype/liberation", "LiberationSans-Regular.ttf", "LiberationSans-Bold.ttf"),
                ("/usr/share/fonts/truetype/noto", "NotoSans-Regular.ttf", "NotoSans-Bold.ttf"),
                ("/usr/share/fonts/TTF", "DejaVuSans.ttf", "DejaVuSans-Bold.ttf"),
            ]
        };
        for (folder, regular, bolder) in families {
            list.push(FontFile::new(&format!("{folder}/{}", pick(regular, bolder)), 0, None));
        }
    }
    list
}

impl Fonts {
    /// The built-in bitmap font for every face.
    pub fn builtin() -> Self {
        Fonts { regular: Face::Builtin, bold: Face::Builtin, mono: Face::Builtin, mono_bold: Face::Builtin, description: "built-in bitmap font".into() }
    }

    /// The system's fonts, each face falling back to its regular weight and then to the built-in
    /// font when no file can be read.
    pub fn system() -> Self {
        let find = |bold: bool, mono: bool| candidates(bold, mono).into_iter().find_map(|file| file.load().map(|font| (font, file.path)));
        let mut used: Vec<String> = Vec::new();
        let mut face = |bold: bool, mono: bool| match find(bold, mono).or_else(|| find(false, mono)) {
            Some((font, path)) => {
                let name = path.display().to_string();
                if !used.contains(&name) {
                    used.push(name);
                }
                Face::Outline(font)
            }
            None => Face::Builtin,
        };
        let (regular, bold, mono, mono_bold) = (face(false, false), face(true, false), face(false, true), face(true, true));
        let description = if used.is_empty() { "built-in bitmap font (no system font found)".to_string() } else { used.join(", ") };
        Fonts { regular, bold, mono, mono_bold, description }
    }

    /// Fonts from these files (regular, bold, fixed-pitch, fixed-pitch bold); a file that cannot be
    /// read leaves its face on the built-in font.
    pub fn from_files(regular: &Path, bold: &Path, mono: &Path, mono_bold: &Path) -> Self {
        let face = |path: &Path| FontFile { path: path.to_path_buf(), index: 0, weight: None }.load().map_or(Face::Builtin, Face::Outline);
        Fonts {
            regular: face(regular),
            bold: face(bold),
            mono: face(mono),
            mono_bold: face(mono_bold),
            description: [regular, bold, mono, mono_bold].map(|path| path.display().to_string()).join(", "),
        }
    }

    /// Which fonts are in use, for diagnostics.
    pub fn description(&self) -> &str {
        &self.description
    }

    fn face(&self, bold: bool, mono: bool) -> &Face {
        match (bold, mono) {
            (false, false) => &self.regular,
            (true, false) => &self.bold,
            (false, true) => &self.mono,
            (true, true) => &self.mono_bold,
        }
    }

    /// A run of text in one face and size, ready to measure and draw.
    pub(crate) fn line(&self, text: &str, size: f64, bold: bool, mono: bool) -> Line<'_> {
        let face = self.face(bold, mono);
        let mut glyphs = Vec::with_capacity(text.len());
        let mut pen = 0.0f64;
        let mut previous = None;
        for character in text.chars() {
            match face {
                Face::Outline(font) if font.glyph_id(character).0 != 0 => {
                    let scaled = font.as_scaled(outline_scale(font, size));
                    let id = font.glyph_id(character);
                    if let Some(previous) = previous {
                        pen += f64::from(scaled.kern(previous, id));
                    }
                    glyphs.push((character, pen, true));
                    pen += f64::from(scaled.h_advance(id));
                    previous = Some(id);
                }
                // The built-in font also stands in for characters an outline font lacks.
                _ => {
                    glyphs.push((character, pen, false));
                    pen += builtin::ADVANCE * builtin::unit(size);
                    previous = None;
                }
            }
        }
        Line { face, glyphs, width: pen, size, bold }
    }

    /// The distance from the top of a line to its baseline.
    pub(crate) fn ascent(&self, size: f64, bold: bool, mono: bool) -> f64 {
        match self.face(bold, mono) {
            Face::Outline(font) => f64::from(font.as_scaled(outline_scale(font, size)).ascent()),
            Face::Builtin => builtin::ASCENT * builtin::unit(size),
        }
    }
}

/// `ab_glyph` scales by the font's height (ascent to descent); a font size is its em.
fn outline_scale(font: &FontArc, size: f64) -> PxScale {
    let em = font.units_per_em().filter(|em| *em > 0.0).unwrap_or(1000.0);
    PxScale::from((size * f64::from(font.height_unscaled()) / f64::from(em)) as f32)
}

/// Laid-out text.
pub(crate) struct Line<'a> {
    face: &'a Face,
    /// (character, pen position, drawn with the outline font).
    glyphs: Vec<(char, f64, bool)>,
    /// Advance width of the whole run.
    pub width: f64,
    size: f64,
    bold: bool,
}

impl Line<'_> {
    /// Draws the run with its pen at `x` on `baseline`: `plot(column, row, coverage)` for every
    /// pixel a glyph touches.
    pub fn draw(&self, x: f64, baseline: f64, plot: &mut dyn FnMut(i32, i32, f32)) {
        for &(character, pen, outline) in &self.glyphs {
            match (self.face, outline) {
                (Face::Outline(font), true) => {
                    let glyph = font.glyph_id(character).with_scale_and_position(outline_scale(font, self.size), point((x + pen) as f32, baseline as f32));
                    if let Some(outlined) = font.outline_glyph(glyph) {
                        let bounds = outlined.px_bounds();
                        let (left, top) = (bounds.min.x as i32, bounds.min.y as i32);
                        outlined.draw(|column, row, coverage| plot(left + column as i32, top + row as i32, coverage));
                    }
                }
                _ => builtin::draw(character, x + pen, baseline, self.size, self.bold, plot),
            }
        }
    }
}

/// The built-in font: the classic 5 × 7 dot-matrix glyphs, one byte per column, lowest bit on top,
/// bit 7 one row below the baseline for descenders.
mod builtin {
    /// Advance of a glyph, in units (five columns and one of spacing).
    pub const ADVANCE: f64 = 6.0;
    /// Rows above the baseline, plus one of headroom.
    pub const ASCENT: f64 = 8.0;

    /// The size of one dot: capital letters are seven dots, 0.7 em, high.
    pub fn unit(size: f64) -> f64 {
        size / 10.0
    }

    const ASCII: [[u8; 5]; 95] = [
        [0x00, 0x00, 0x00, 0x00, 0x00],
        [0x00, 0x00, 0x5F, 0x00, 0x00],
        [0x00, 0x07, 0x00, 0x07, 0x00],
        [0x14, 0x7F, 0x14, 0x7F, 0x14],
        [0x24, 0x2A, 0x7F, 0x2A, 0x12],
        [0x23, 0x13, 0x08, 0x64, 0x62],
        [0x36, 0x49, 0x56, 0x20, 0x50],
        [0x00, 0x08, 0x07, 0x03, 0x00],
        [0x00, 0x1C, 0x22, 0x41, 0x00],
        [0x00, 0x41, 0x22, 0x1C, 0x00],
        [0x2A, 0x1C, 0x7F, 0x1C, 0x2A],
        [0x08, 0x08, 0x3E, 0x08, 0x08],
        [0x00, 0x80, 0x70, 0x30, 0x00],
        [0x08, 0x08, 0x08, 0x08, 0x08],
        [0x00, 0x00, 0x60, 0x60, 0x00],
        [0x20, 0x10, 0x08, 0x04, 0x02],
        [0x3E, 0x51, 0x49, 0x45, 0x3E],
        [0x00, 0x42, 0x7F, 0x40, 0x00],
        [0x72, 0x49, 0x49, 0x49, 0x46],
        [0x21, 0x41, 0x49, 0x4D, 0x33],
        [0x18, 0x14, 0x12, 0x7F, 0x10],
        [0x27, 0x45, 0x45, 0x45, 0x39],
        [0x3C, 0x4A, 0x49, 0x49, 0x31],
        [0x41, 0x21, 0x11, 0x09, 0x07],
        [0x36, 0x49, 0x49, 0x49, 0x36],
        [0x46, 0x49, 0x49, 0x29, 0x1E],
        [0x00, 0x00, 0x14, 0x00, 0x00],
        [0x00, 0x40, 0x34, 0x00, 0x00],
        [0x00, 0x08, 0x14, 0x22, 0x41],
        [0x14, 0x14, 0x14, 0x14, 0x14],
        [0x00, 0x41, 0x22, 0x14, 0x08],
        [0x02, 0x01, 0x59, 0x09, 0x06],
        [0x3E, 0x41, 0x5D, 0x59, 0x4E],
        [0x7C, 0x12, 0x11, 0x12, 0x7C],
        [0x7F, 0x49, 0x49, 0x49, 0x36],
        [0x3E, 0x41, 0x41, 0x41, 0x22],
        [0x7F, 0x41, 0x41, 0x41, 0x3E],
        [0x7F, 0x49, 0x49, 0x49, 0x41],
        [0x7F, 0x09, 0x09, 0x09, 0x01],
        [0x3E, 0x41, 0x41, 0x51, 0x73],
        [0x7F, 0x08, 0x08, 0x08, 0x7F],
        [0x00, 0x41, 0x7F, 0x41, 0x00],
        [0x20, 0x40, 0x41, 0x3F, 0x01],
        [0x7F, 0x08, 0x14, 0x22, 0x41],
        [0x7F, 0x40, 0x40, 0x40, 0x40],
        [0x7F, 0x02, 0x1C, 0x02, 0x7F],
        [0x7F, 0x04, 0x08, 0x10, 0x7F],
        [0x3E, 0x41, 0x41, 0x41, 0x3E],
        [0x7F, 0x09, 0x09, 0x09, 0x06],
        [0x3E, 0x41, 0x51, 0x21, 0x5E],
        [0x7F, 0x09, 0x19, 0x29, 0x46],
        [0x26, 0x49, 0x49, 0x49, 0x32],
        [0x03, 0x01, 0x7F, 0x01, 0x03],
        [0x3F, 0x40, 0x40, 0x40, 0x3F],
        [0x1F, 0x20, 0x40, 0x20, 0x1F],
        [0x3F, 0x40, 0x38, 0x40, 0x3F],
        [0x63, 0x14, 0x08, 0x14, 0x63],
        [0x03, 0x04, 0x78, 0x04, 0x03],
        [0x61, 0x59, 0x49, 0x4D, 0x43],
        [0x00, 0x7F, 0x41, 0x41, 0x41],
        [0x02, 0x04, 0x08, 0x10, 0x20],
        [0x00, 0x41, 0x41, 0x41, 0x7F],
        [0x04, 0x02, 0x01, 0x02, 0x04],
        [0x40, 0x40, 0x40, 0x40, 0x40],
        [0x00, 0x03, 0x07, 0x08, 0x00],
        [0x20, 0x54, 0x54, 0x78, 0x40],
        [0x7F, 0x28, 0x44, 0x44, 0x38],
        [0x38, 0x44, 0x44, 0x44, 0x28],
        [0x38, 0x44, 0x44, 0x28, 0x7F],
        [0x38, 0x54, 0x54, 0x54, 0x18],
        [0x00, 0x08, 0x7E, 0x09, 0x02],
        [0x18, 0xA4, 0xA4, 0x9C, 0x78],
        [0x7F, 0x08, 0x04, 0x04, 0x78],
        [0x00, 0x44, 0x7D, 0x40, 0x00],
        [0x20, 0x40, 0x40, 0x3D, 0x00],
        [0x7F, 0x10, 0x28, 0x44, 0x00],
        [0x00, 0x41, 0x7F, 0x40, 0x00],
        [0x7C, 0x04, 0x78, 0x04, 0x78],
        [0x7C, 0x08, 0x04, 0x04, 0x78],
        [0x38, 0x44, 0x44, 0x44, 0x38],
        [0xFC, 0x18, 0x24, 0x24, 0x18],
        [0x18, 0x24, 0x24, 0x18, 0xFC],
        [0x7C, 0x08, 0x04, 0x04, 0x08],
        [0x48, 0x54, 0x54, 0x54, 0x24],
        [0x04, 0x04, 0x3F, 0x44, 0x24],
        [0x3C, 0x40, 0x40, 0x20, 0x7C],
        [0x1C, 0x20, 0x40, 0x20, 0x1C],
        [0x3C, 0x40, 0x30, 0x40, 0x3C],
        [0x44, 0x28, 0x10, 0x28, 0x44],
        [0x4C, 0x90, 0x90, 0x90, 0x7C],
        [0x44, 0x64, 0x54, 0x4C, 0x44],
        [0x00, 0x08, 0x36, 0x41, 0x00],
        [0x00, 0x00, 0x77, 0x00, 0x00],
        [0x00, 0x41, 0x36, 0x08, 0x00],
        [0x02, 0x01, 0x02, 0x04, 0x02],
    ];

    /// Characters outside ASCII that the interface's screens use.
    fn extra(character: char) -> Option<[u8; 5]> {
        Some(match character {
            '…' => [0x40, 0x00, 0x40, 0x00, 0x40],
            '·' | '•' => [0x00, 0x00, 0x18, 0x18, 0x00],
            '°' => [0x00, 0x06, 0x09, 0x09, 0x06],
            '►' | '▶' => [0x7F, 0x3E, 0x1C, 0x08, 0x00],
            '◄' | '◀' => [0x00, 0x08, 0x1C, 0x3E, 0x7F],
            '▲' => [0x60, 0x78, 0x7E, 0x78, 0x60],
            '▼' => [0x03, 0x0F, 0x3F, 0x0F, 0x03],
            '–' | '—' | '−' => return glyph('-'),
            '×' => return glyph('x'),
            '♯' => return glyph('#'),
            '♭' => return glyph('b'),
            '’' | '‘' => return glyph('\''),
            '“' | '”' => return glyph('"'),
            '\u{a0}' => return glyph(' '),
            _ => return None,
        })
    }

    fn glyph(character: char) -> Option<[u8; 5]> {
        let code = character as u32;
        if (0x20..0x7F).contains(&code) {
            Some(ASCII[(code - 0x20) as usize])
        } else {
            extra(character)
        }
    }

    pub fn draw(character: char, x: f64, baseline: f64, size: f64, bold: bool, plot: &mut dyn FnMut(i32, i32, f32)) {
        let columns = glyph(character).or_else(|| glyph('?')).unwrap_or([0; 5]);
        let unit = unit(size);
        if unit <= 0.0 {
            return;
        }
        // Row 0 is seven dots above the baseline.
        let top = baseline - 7.0 * unit;
        let lit = |sample_x: f64, sample_y: f64| -> bool {
            let (column, row) = (((sample_x - x) / unit).floor(), ((sample_y - top) / unit).floor());
            (0.0..5.0).contains(&column) && (0.0..8.0).contains(&row) && columns[column as usize] & (1 << row as u32) != 0
        };
        let extra = if bold { 0.6 * unit } else { 0.0 };
        let (first_column, last_column) = (x.floor() as i32, (x + 5.0 * unit + extra).ceil() as i32);
        let (first_row, last_row) = (top.floor() as i32, (top + 8.0 * unit).ceil() as i32);
        for row in first_row..last_row {
            for column in first_column..last_column {
                // 3 × 3 samples per pixel.
                let mut hits = 0;
                for sub_y in 0..3 {
                    for sub_x in 0..3 {
                        let (sample_x, sample_y) = (f64::from(column) + (f64::from(sub_x) + 0.5) / 3.0, f64::from(row) + (f64::from(sub_y) + 0.5) / 3.0);
                        if lit(sample_x, sample_y) || (bold && lit(sample_x - extra, sample_y)) {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    plot(column, row, hits as f32 / 9.0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ink(fonts: &Fonts, text: &str, size: f64, bold: bool) -> (usize, i32, i32, i32, i32) {
        let line = fonts.line(text, size, bold, false);
        let (mut count, mut left, mut right, mut top, mut bottom) = (0, i32::MAX, i32::MIN, i32::MAX, i32::MIN);
        line.draw(100.0, 100.0, &mut |column, row, coverage| {
            if coverage > 0.5 {
                count += 1;
                left = left.min(column);
                right = right.max(column);
                top = top.min(row);
                bottom = bottom.max(row);
            }
        });
        (count, left, right, top, bottom)
    }

    #[test]
    fn the_builtin_font_measures_and_draws_predictably() {
        let fonts = Fonts::builtin();
        assert_eq!(fonts.description(), "built-in bitmap font");
        assert_eq!(fonts.line("Hello", 10.0, false, false).width, 30.0, "six dots per character at one pixel per dot");
        assert_eq!(fonts.line("Hello", 20.0, true, true).width, 60.0);
        assert_eq!(fonts.line("", 10.0, false, false).width, 0.0);
        assert_eq!(fonts.ascent(10.0, false, false), 8.0);
        // A capital H at size 10: two full-height stems, 7 pixels high, 5 wide, sitting on the baseline.
        let (count, left, right, top, bottom) = ink(&fonts, "H", 10.0, false);
        assert_eq!((left, right, top, bottom), (100, 104, 93, 99));
        assert_eq!(count, 17);
        // Descenders go one dot below the baseline; spaces draw nothing; bold is heavier.
        assert_eq!(ink(&fonts, "g", 10.0, false).4, 100);
        assert_eq!(ink(&fonts, "   ", 10.0, false).0, 0);
        assert!(ink(&fonts, "H", 20.0, true).0 > ink(&fonts, "H", 20.0, false).0);
        // Characters the screens use beyond ASCII have glyphs; anything else shows as a question mark.
        for character in ['…', '·', '►', '◄', '–', '×', '°', '▲', '▼'] {
            assert!(ink(&fonts, &character.to_string(), 20.0, false).0 > 0, "{character}");
        }
        assert_eq!(ink(&fonts, "é", 20.0, false), ink(&fonts, "?", 20.0, false));
    }

    #[test]
    fn system_fonts_load_where_present_and_fall_back_where_not() {
        let fonts = Fonts::system();
        assert!(!fonts.description().is_empty());
        let line = fonts.line("SuperTracker 128", 18.0, true, false);
        assert!(line.width > 60.0 && line.width < 300.0, "a plausible width: {}", line.width);
        let ascent = fonts.ascent(18.0, false, false);
        assert!(ascent > 9.0 && ascent < 24.0, "a plausible ascent: {ascent}");
        let mut pixels = 0;
        line.draw(10.0, 30.0, &mut |_, _, coverage| pixels += usize::from(coverage > 0.5));
        assert!(pixels > 100);
        // Files that do not exist leave the built-in font in place.
        let missing = Path::new("/nonexistent/font.ttf");
        let fallback = Fonts::from_files(missing, missing, missing, missing);
        assert_eq!(fallback.line("ab", 10.0, false, false).width, 12.0);
        if let Some(file) = candidates(false, false).into_iter().find(|file| file.path.exists()) {
            let loaded = Fonts::from_files(&file.path, missing, missing, missing);
            let (outline, builtin) = (loaded.line("Wi", 20.0, false, false).width, loaded.line("Wi", 20.0, true, false).width);
            assert_eq!(builtin, 24.0);
            assert!(outline != builtin, "the file's own metrics are used");
            // A character the font lacks is drawn with the built-in glyph instead of nothing.
            let mut drawn = 0;
            loaded.line("\u{10FFFF}", 20.0, false, false).draw(0.0, 20.0, &mut |_, _, _| drawn += 1);
            assert!(drawn > 0);
        }
    }

    #[test]
    fn every_platform_has_candidates_for_every_face() {
        for bold in [false, true] {
            for mono in [false, true] {
                assert!(!candidates(bold, mono).is_empty());
            }
        }
    }
}
