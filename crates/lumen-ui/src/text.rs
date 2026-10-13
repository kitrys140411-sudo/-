//! UI text: font sets (the UI family in four weights, the monospace family),
//! cached layouts (shaping, wrapping, ellipsizing via `lumen-text`) and glyph
//! placement for the painter.
//!
//! Text is shaped at the physical pixel size (logical size × scale) so hinting
//! and advances match what is drawn; layouts report logical sizes. Layouts
//! are cached by their inputs, and glyph bitmaps live in the renderer's atlas,
//! so drawing unchanged text costs a hash lookup per glyph and no uploads.

use std::collections::HashMap;
use std::rc::Rc;

use lumen_text::{FontDb, FontSet, Layouter, Line};

pub use lumen_text::Weight;

use crate::geom::Vec2;

/// Which family: the UI font (Inter) or the monospace font.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Family {
    #[default]
    Ui,
    Mono,
}

/// How a piece of text is set (its color is a paint property, see
/// [`crate::widgets::Label`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Size in logical pixels.
    pub size: f32,
    pub weight: Weight,
    pub family: Family,
    /// Distance between baselines as a multiple of the size.
    pub line_height: f32,
}

impl TextStyle {
    pub fn new(size: f32) -> TextStyle {
        TextStyle { size, weight: Weight::Regular, family: Family::Ui, line_height: 1.4 }
    }

    pub fn weight(mut self, w: Weight) -> TextStyle {
        self.weight = w;
        self
    }

    pub fn mono(mut self) -> TextStyle {
        self.family = Family::Mono;
        self
    }

    /// Line box height in logical pixels.
    pub fn line_px(&self) -> f32 {
        (self.size * self.line_height).round()
    }
}

/// What happens to text wider than its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Wrap {
    /// One line per paragraph, never broken (overflow is clipped by the box).
    None,
    /// One line, shortened with "…" to fit.
    #[default]
    Ellipsize,
    /// Broken into lines at spaces (words longer than the box are split); at
    /// most `max_lines` lines (0 = unlimited), the last one ellipsized.
    Wrap { max_lines: u16 },
}

/// Horizontal alignment of lines inside the text box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
}

/// A laid-out paragraph. Coordinates inside lines are physical pixels (as
/// shaped); `size`, `line_px` and `baseline` are logical.
pub struct TextLayout {
    pub lines: Vec<Line>,
    pub scale: f32,
    /// Logical size of the text box (widest line × lines).
    pub size: Vec2,
    /// Logical line box height.
    pub line_px: f32,
    /// Baseline of the first line below the box top (logical).
    pub baseline: f32,
    /// Byte offset of each line's text in the source string (wrapping trims
    /// spaces, so lines are not contiguous).
    pub starts: Vec<usize>,
    /// The text was shortened with an ellipsis.
    pub truncated: bool,
}

impl TextLayout {
    /// Logical x of byte offset `b` of line `line` (carets).
    pub fn x_at(&self, line: usize, b: usize) -> f32 {
        self.lines.get(line).map(|l| l.x_at(b) / self.scale).unwrap_or(0.0)
    }

    /// The byte offset in line `line` closest to logical `x`.
    pub fn index_at(&self, line: usize, x: f32) -> usize {
        self.lines.get(line).map(|l| l.index_at(x * self.scale)).unwrap_or(0)
    }

    pub fn line_width(&self, line: usize) -> f32 {
        self.lines.get(line).map(|l| l.width / self.scale).unwrap_or(0.0)
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    text: String,
    family: Family,
    weight: Weight,
    size_bits: u32,
    lh_bits: u32,
    wrap: Wrap,
    /// Physical width limit, whole pixels (None for unconstrained).
    width: Option<u32>,
}

struct Cached {
    layout: Rc<TextLayout>,
    used: u64,
}

/// Fonts and the layout cache of one window.
pub struct Fonts {
    pub db: FontDb,
    ui_family: String,
    mono_family: String,
    sets: HashMap<(Family, Weight), FontSet>,
    layouter: Layouter,
    cache: HashMap<Key, Cached>,
    frame: u64,
}

impl Fonts {
    pub fn new(ui_family: &str, mono_family: &str) -> Result<Fonts, String> {
        let db = FontDb::scan().map_err(|e| e.0)?;
        if db.faces.is_empty() {
            return Err("no fonts installed".into());
        }
        Ok(Fonts {
            db,
            ui_family: ui_family.to_string(),
            mono_family: mono_family.to_string(),
            sets: HashMap::new(),
            layouter: Layouter::new(),
            cache: HashMap::new(),
            frame: 0,
        })
    }

    fn set(&mut self, family: Family, weight: Weight) -> &FontSet {
        if !self.sets.contains_key(&(family, weight)) {
            let fams: Vec<&str> = match family {
                Family::Ui => vec![self.ui_family.as_str(), "Inter", "DejaVu Sans", "Noto Sans"],
                Family::Mono => vec![self.mono_family.as_str(), "JetBrains Mono", "DejaVu Sans Mono", "Noto Sans Mono"],
            };
            let set = FontSet::new(&self.db, &fams, weight).expect("fonts were found at startup");
            self.sets.insert((family, weight), set);
        }
        &self.sets[&(family, weight)]
    }

    /// Ascent and descent (logical) of the primary font of a style.
    pub fn metrics(&mut self, style: &TextStyle, scale: f32) -> (f32, f32) {
        let px = style.size * scale;
        match self.set(style.family, style.weight).font(0, px) {
            Some(f) => (f.metrics.ascent / scale, f.metrics.descent / scale),
            None => (style.size * 0.8, style.size * 0.2),
        }
    }

    /// Lay out `text`; `max_width` (logical) applies to `Wrap::Ellipsize` and
    /// `Wrap::Wrap`.
    pub fn layout(&mut self, text: &str, style: &TextStyle, wrap: Wrap, max_width: Option<f32>, scale: f32) -> Rc<TextLayout> {
        let px = (style.size * scale).max(1.0);
        let width = match wrap {
            Wrap::None => None,
            _ => max_width.map(|w| (w * scale).floor().max(0.0) as u32),
        };
        // Constrained text that fits needs no constrained layout: reuse the
        // natural one (so widths changing during a resize cost nothing).
        if let Some(w) = width {
            let natural = self.layout(text, style, Wrap::None, None, scale);
            if natural.lines.iter().all(|l| l.width <= w as f32 + 0.01) {
                return natural;
            }
        }
        let key = Key { text: text.to_string(), family: style.family, weight: style.weight, size_bits: px.to_bits(), lh_bits: style.line_height.to_bits(), wrap, width };
        let frame = self.frame;
        if let Some(c) = self.cache.get_mut(&key) {
            c.used = frame;
            return c.layout.clone();
        }
        let layout = Rc::new(self.build(text, style, wrap, width, scale, px));
        if self.cache.len() > 4096 {
            // Keep what recent frames used.
            self.cache.retain(|_, c| c.used + 120 > frame);
        }
        self.cache.insert(key, Cached { layout: layout.clone(), used: frame });
        layout
    }

    fn build(&mut self, text: &str, style: &TextStyle, wrap: Wrap, width: Option<u32>, scale: f32, px: f32) -> TextLayout {
        self.set(style.family, style.weight);
        let set = &self.sets[&(style.family, style.weight)];
        let layouter = &mut self.layouter;
        let mut truncated = false;
        let (lines, starts): (Vec<Line>, Vec<usize>) = match (wrap, width) {
            (Wrap::Ellipsize, Some(w)) => {
                // One line: newlines become spaces.
                let flat = text.replace('\n', " ");
                let l = layouter.ellipsize(set, &flat, px, w as f32);
                truncated = l.text != flat;
                (vec![l], vec![0])
            }
            (Wrap::Wrap { max_lines }, Some(w)) => {
                let max = (max_lines > 0).then_some(max_lines as usize);
                let lines = layouter.wrap(set, text, px, w as f32, max);
                truncated = lines.last().is_some_and(|l| l.text.ends_with('…')) && !text.ends_with('…');
                let starts = line_starts(text, &lines);
                (lines, starts)
            }
            _ => {
                let mut starts = Vec::new();
                let mut off = 0;
                let lines = text
                    .split('\n')
                    .map(|para| {
                        starts.push(off);
                        off += para.len() + 1;
                        layouter.line(set, para, px)
                    })
                    .collect();
                (lines, starts)
            }
        };
        let font = set.font(0, px);
        let (ascent, descent) = font.as_ref().map(|f| (f.metrics.ascent, f.metrics.descent)).unwrap_or((px * 0.8, px * 0.2));
        let line_px = style.line_px();
        // Center the font's extent in the line box; the baseline sits on a
        // whole physical pixel (light hinting snaps vertical stems to it).
        let baseline = (((line_px * scale - (ascent + descent)) / 2.0 + ascent).round()) / scale;
        let widest = lines.iter().map(|l| l.width).fold(0.0f32, f32::max) / scale;
        let n = lines.len().max(1) as f32;
        TextLayout { size: Vec2::new(widest.ceil(), line_px * n), lines, scale, line_px, baseline, starts, truncated }
    }

    /// Advance the cache clock (once per frame).
    pub fn next_frame(&mut self) {
        self.frame += 1;
    }
}

/// Byte offsets of wrapped lines in the source: each line's text is a
/// substring that starts at or after the previous line's end.
fn line_starts(text: &str, lines: &[Line]) -> Vec<usize> {
    let mut out = Vec::with_capacity(lines.len());
    let mut from = 0;
    for l in lines {
        let t = l.text.trim_end_matches('…');
        match text[from..].find(t) {
            Some(i) if !t.is_empty() => {
                out.push(from + i);
                from += i + t.len();
            }
            _ => out.push(from),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Option<Fonts> {
        let f = Fonts::new("Inter", "JetBrains Mono").ok()?;
        if !f.db.faces.iter().any(|x| x.family == "Inter") {
            eprintln!("Inter not installed; skipping");
            return None;
        }
        Some(f)
    }

    #[test]
    fn layouts_are_cached_and_scale_independent_in_logical_units() {
        let Some(mut f) = fonts() else { return };
        let s = TextStyle::new(14.0);
        let a = f.layout("Hello world", &s, Wrap::None, None, 1.0);
        let b = f.layout("Hello world", &s, Wrap::None, None, 1.0);
        assert!(Rc::ptr_eq(&a, &b), "second layout comes from the cache");
        let c = f.layout("Hello world", &s, Wrap::None, None, 2.0);
        assert!((a.size.x - c.size.x).abs() <= 1.0, "logical widths match: {} {}", a.size.x, c.size.x);
        assert_eq!(a.size.y, s.line_px());
        assert!(a.baseline > 0.0 && a.baseline < a.line_px);
    }

    #[test]
    fn fitting_text_reuses_the_natural_layout() {
        let Some(mut f) = fonts() else { return };
        let s = TextStyle::new(14.0);
        let natural = f.layout("Short", &s, Wrap::None, None, 1.0);
        let fit = f.layout("Short", &s, Wrap::Ellipsize, Some(500.0), 1.0);
        assert!(Rc::ptr_eq(&natural, &fit));
        let cut = f.layout("A much longer label that will not fit", &s, Wrap::Ellipsize, Some(80.0), 1.0);
        assert!(cut.truncated && cut.size.x <= 80.0 && cut.lines[0].text.ends_with('…'));
    }

    #[test]
    fn wrapping_reports_line_starts() {
        let Some(mut f) = fonts() else { return };
        let s = TextStyle::new(14.0);
        let text = "The quick brown fox jumps over the lazy dog";
        let l = f.layout(text, &s, Wrap::Wrap { max_lines: 0 }, Some(100.0), 1.0);
        assert!(l.lines.len() >= 3);
        for (line, &start) in l.lines.iter().zip(&l.starts) {
            assert!(text[start..].starts_with(&line.text), "{start} {:?}", line.text);
        }
        assert_eq!(l.size.y, s.line_px() * l.lines.len() as f32);
        let two = f.layout(text, &s, Wrap::Wrap { max_lines: 2 }, Some(100.0), 1.0);
        assert!(two.lines.len() == 2 && two.truncated);
    }

    #[test]
    fn multiline_without_wrapping() {
        let Some(mut f) = fonts() else { return };
        let l = f.layout("one\ntwo\nthree", &TextStyle::new(12.0), Wrap::None, None, 1.0);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.starts, vec![0, 4, 8]);
    }
}
