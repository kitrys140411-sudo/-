//! Text for Lumen: font discovery and fallback, HarfBuzz shaping, layout
//! (wrapping, ellipsis, caret positions), FreeType rasterization with subpixel
//! positioning, gamma-adjusted painting into RGBA images, and vector paths for
//! icons. Used by the compositor's shell UI and the terminal.

pub mod font;
pub mod fonts;
pub mod layout;
pub mod path;
pub mod render;
pub mod shape;

pub use font::{Error, Font, FontFile, Library, Metrics, RasterGlyph};
pub use fonts::{FontDb, FontSet, Weight};
pub use layout::{Glyph, Layouter, Line};
pub use path::Path;
pub use render::{Align, GlyphCache, Image, Painter};
pub use shape::{Feature, ShapedGlyph, Shaper};

use std::collections::HashMap;

/// How a piece of UI text looks; sizes in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: Weight,
    /// Straight-alpha sRGB.
    pub color: [f32; 4],
    pub align: Align,
    /// Baseline distance as a multiple of the font size.
    pub line_spacing: f32,
}

impl TextStyle {
    pub fn new(size: f32, color: [f32; 4]) -> TextStyle {
        TextStyle { size, weight: Weight::Regular, color, align: Align::Left, line_spacing: 1.3 }
    }

    pub fn weight(mut self, w: Weight) -> Self {
        self.weight = w;
        self
    }

    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
}

/// Fonts (one fallback chain per weight), layout and painting in one place.
pub struct TextSystem {
    pub db: FontDb,
    family: String,
    sets: HashMap<Weight, FontSet>,
    pub layouter: Layouter,
    pub painter: Painter,
}

impl TextSystem {
    pub fn new(family: &str) -> Result<TextSystem, Error> {
        let db = FontDb::scan()?;
        let mut sets = HashMap::new();
        sets.insert(Weight::Regular, FontSet::new(&db, &[family], Weight::Regular)?);
        Ok(TextSystem { db, family: family.to_string(), sets, layouter: Layouter::new(), painter: Painter::default() })
    }

    pub fn set(&mut self, weight: Weight) -> &FontSet {
        if !self.sets.contains_key(&weight) {
            let s = FontSet::new(&self.db, &[self.family.as_str()], weight).expect("fonts were found at startup");
            self.sets.insert(weight, s);
        }
        &self.sets[&weight]
    }

    /// Lay out `text` (wrapped to `max_width` and at most `max_lines` lines,
    /// or a single ellipsized line when `max_lines` is 1).
    pub fn layout(&mut self, style: &TextStyle, text: &str, max_width: Option<f32>, max_lines: Option<usize>) -> Vec<Line> {
        self.set(style.weight);
        let set = &self.sets[&style.weight];
        match (max_width, max_lines) {
            (Some(w), Some(1)) => vec![self.layouter.ellipsize(set, text, style.size, w)],
            (Some(w), n) => self.layouter.wrap(set, text, style.size, w, n),
            (None, _) => text.split('\n').map(|t| self.layouter.line(set, t, style.size)).collect(),
        }
    }

    /// Lay out and paint.
    pub fn render(&mut self, style: &TextStyle, text: &str, max_width: Option<f32>, max_lines: Option<usize>) -> Image {
        let lines = self.layout(style, text, max_width, max_lines);
        let line_height = (style.size * style.line_spacing).round();
        self.painter.paint(&lines, line_height, style.color, style.align, max_width.filter(|_| style.align != Align::Left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> Option<TextSystem> {
        let t = TextSystem::new("Inter").ok()?;
        if !t.db.faces.iter().any(|f| f.family == "Inter") {
            eprintln!("Inter not installed; skipping");
            return None;
        }
        Some(t)
    }

    #[test]
    fn finds_inter_weights() {
        let Some(t) = system() else { return };
        assert_eq!(t.db.find("Inter", Weight::Regular).unwrap().style, "Regular");
        assert_eq!(t.db.find("Inter", Weight::SemiBold).unwrap().style, "SemiBold");
    }

    #[test]
    fn line_width_grows_with_text_and_size() {
        let Some(mut t) = system() else { return };
        let s14 = TextStyle::new(14.0, [1.0; 4]);
        let s28 = TextStyle::new(28.0, [1.0; 4]);
        let a = t.layout(&s14, "Hello", None, None)[0].width;
        let b = t.layout(&s14, "Hello, world", None, None)[0].width;
        let c = t.layout(&s28, "Hello", None, None)[0].width;
        assert!(a > 20.0 && b > a, "{a} {b}");
        assert!((c / a - 2.0).abs() < 0.05, "scales linearly: {a} {c}");
    }

    #[test]
    fn kerning_is_applied() {
        let Some(mut t) = system() else { return };
        let s = TextStyle::new(40.0, [1.0; 4]);
        let av = t.layout(&s, "AV", None, None)[0].width;
        let a = t.layout(&s, "A", None, None)[0].width;
        let v = t.layout(&s, "V", None, None)[0].width;
        assert!(av < a + v - 0.5, "AV {av} vs A+V {}", a + v);
    }

    #[test]
    fn wrapping_and_ellipsis() {
        let Some(mut t) = system() else { return };
        let s = TextStyle::new(14.0, [1.0; 4]);
        let text = "The quick brown fox jumps over the lazy dog again and again";
        let lines = t.layout(&s, text, Some(120.0), None);
        assert!(lines.len() >= 3, "{}", lines.len());
        for l in &lines {
            assert!(l.width <= 120.5, "{} too wide: {}", l.text, l.width);
            assert!(!l.text.starts_with(' ') && !l.text.ends_with(' '));
        }
        let joined: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(joined.join(" "), text);
        let two = t.layout(&s, text, Some(120.0), Some(2));
        assert_eq!(two.len(), 2);
        assert!(two[1].text.ends_with('…') && two[1].width <= 120.5);
        // A single word longer than the width is split.
        let long = t.layout(&s, "Supercalifragilisticexpialidocious", Some(60.0), None);
        assert!(long.len() >= 2 && long.iter().all(|l| l.width <= 60.5));
    }

    #[test]
    fn caret_positions_are_monotonic() {
        let Some(mut t) = system() else { return };
        let s = TextStyle::new(14.0, [1.0; 4]);
        let line = &t.layout(&s, "abc def", None, None)[0];
        let xs: Vec<f32> = (0..=7).map(|i| line.x_at(i)).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "{xs:?}");
        assert_eq!(line.index_at(xs[4] + 0.1), 4);
    }

    #[test]
    fn subpixel_phases_differ_and_paint_has_ink() {
        let Some(mut t) = system() else { return };
        let set = t.set(Weight::Regular);
        let font = set.font(0, 16.0).unwrap();
        let id = font.glyph_index('l');
        let a = font.rasterize(id, 0.0).unwrap();
        let b = font.rasterize(id, 0.5).unwrap();
        assert_ne!(a.data, b.data, "phase must shift coverage");
        let img = t.render(&TextStyle::new(16.0, [1.0, 1.0, 1.0, 1.0]), "Lumen", None, None);
        assert!(img.width > 30 && img.height >= 16);
        let ink = img.data.chunks_exact(4).filter(|p| p[3] > 200).count();
        assert!(ink > 40, "ink pixels {ink}");
        // Premultiplied: color never exceeds alpha.
        assert!(img.data.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
    }

    #[test]
    fn emoji_fall_back_to_color_font() {
        let Some(mut t) = system() else { return };
        if t.db.emoji().is_none() {
            eprintln!("no color emoji font; skipping");
            return;
        }
        let s = TextStyle::new(20.0, [1.0; 4]);
        let line = &t.layout(&s, "hi 😀", None, None)[0];
        let last = line.glyphs.last().unwrap();
        assert!(last.font.color, "emoji should use the color font");
        assert!((last.advance - 20.0).abs() < 8.0, "emoji scaled to the text size: {}", last.advance);
        let img = t.render(&s, "😀", None, None);
        assert!(img.data.chunks_exact(4).any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2])), "emoji has color");
    }
}
