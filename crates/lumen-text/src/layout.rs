//! Line layout: font fallback per grapheme, shaping per run, wrapping and
//! ellipsizing. Coordinates are pixels with the pen starting at x = 0 on the
//! baseline (y grows downward).

use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;

use crate::font::Font;
use crate::fonts::FontSet;
use crate::shape::{Feature, Shaper};

#[derive(Clone)]
pub struct Glyph {
    pub font: Rc<Font>,
    pub id: u32,
    /// Pen position of the glyph origin (offsets applied).
    pub x: f32,
    pub y: f32,
    /// Byte offset of the glyph's cluster in the line text.
    pub cluster: usize,
    pub advance: f32,
}

#[derive(Clone)]
pub struct Line {
    pub text: String,
    pub glyphs: Vec<Glyph>,
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
}

impl Line {
    /// x of the pen at byte offset `b` (a grapheme boundary), for carets and
    /// hit testing (left-to-right text).
    pub fn x_at(&self, b: usize) -> f32 {
        let mut x = 0.0;
        for g in &self.glyphs {
            if g.cluster >= b {
                return x;
            }
            x += g.advance;
        }
        self.width
    }

    /// The grapheme boundary closest to `x`.
    pub fn index_at(&self, x: f32) -> usize {
        let mut best = (0usize, f32::MAX);
        for (i, _) in self.text.grapheme_indices(true).chain(std::iter::once((self.text.len(), ""))) {
            let d = (self.x_at(i) - x).abs();
            if d < best.1 {
                best = (i, d);
            }
        }
        best.0
    }
}

pub struct Layouter {
    shaper: Shaper,
    pub features: Vec<Feature>,
}

impl Default for Layouter {
    fn default() -> Self {
        Self::new()
    }
}

impl Layouter {
    pub fn new() -> Layouter {
        Layouter { shaper: Shaper::new(), features: Vec::new() }
    }

    /// One line of text (newlines are not interpreted).
    pub fn line(&mut self, set: &FontSet, text: &str, px: f32) -> Line {
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        for (i, g) in text.grapheme_indices(true) {
            let f = set.select(g, px);
            match runs.last_mut() {
                Some(r) if r.0 == f => r.2 = i + g.len(),
                _ => runs.push((f, i, i + g.len())),
            }
        }
        let primary = set.font(0, px);
        let (ascent, descent) = primary.as_ref().map(|f| (f.metrics.ascent, f.metrics.descent)).unwrap_or((px * 0.8, px * 0.2));
        let mut glyphs = Vec::new();
        let mut x = 0.0f32;
        for (fi, start, end) in runs {
            let Some(font) = set.font(fi, px) else { continue };
            for g in self.shaper.shape(&font, &text[start..end], &self.features) {
                glyphs.push(Glyph {
                    font: font.clone(),
                    id: g.id,
                    x: x + g.x_offset,
                    y: -g.y_offset,
                    cluster: start + g.cluster,
                    advance: g.x_advance,
                });
                x += g.x_advance;
            }
        }
        Line { text: text.to_string(), glyphs, width: x, ascent, descent }
    }

    /// The line, shortened with "…" to fit `max_width` if needed.
    pub fn ellipsize(&mut self, set: &FontSet, text: &str, px: f32, max_width: f32) -> Line {
        let full = self.line(set, text, px);
        if full.width <= max_width {
            return full;
        }
        // Prefix of k graphemes ends at cut(k).
        let bounds: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        let cut = |k: usize| if k >= bounds.len() { text.len() } else { bounds[k] };
        // Largest k whose prefix fits together with the ellipsis (k = 0 always "fits").
        let (mut lo, mut hi) = (0usize, bounds.len());
        let mut best = self.line(set, "…", px);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let l = self.line(set, &format!("{}…", text[..cut(mid)].trim_end()), px);
            if l.width <= max_width {
                best = l;
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        best
    }

    /// Wrap `text` into lines of at most `max_width`: hard breaks at '\n',
    /// soft breaks after spaces, overlong words split between graphemes. With
    /// `max_lines`, the last line is ellipsized.
    pub fn wrap(&mut self, set: &FontSet, text: &str, px: f32, max_width: f32, max_lines: Option<usize>) -> Vec<Line> {
        let mut texts: Vec<String> = Vec::new();
        for para in text.split('\n') {
            let full = self.line(set, para, px);
            if full.width <= max_width || para.is_empty() {
                texts.push(para.to_string());
                continue;
            }
            let bounds: Vec<usize> = para.grapheme_indices(true).map(|(i, _)| i).chain(std::iter::once(para.len())).collect();
            // Break opportunities: after a run of whitespace.
            let mut opportunities = Vec::new();
            let chars: Vec<(usize, char)> = para.char_indices().collect();
            for w in chars.windows(2) {
                if w[0].1.is_whitespace() && !w[1].1.is_whitespace() {
                    opportunities.push(w[1].0);
                }
            }
            opportunities.push(para.len());
            let mut start = 0usize;
            while start < para.len() {
                let x0 = full.x_at(start);
                let fits = |end: usize| full.x_at(end) - x0 <= max_width + 0.01;
                // Trailing spaces do not count toward the width.
                let soft = opportunities.iter().copied().filter(|&o| o > start).take_while(|&o| {
                    let trimmed = start + para[start..o].trim_end().len();
                    fits(trimmed)
                });
                let end = match soft.last() {
                    Some(o) => o,
                    None => {
                        // No space fits: split the word at the last fitting grapheme.
                        let g = bounds.iter().copied().filter(|&b| b > start).take_while(|&b| fits(b)).last();
                        g.unwrap_or_else(|| bounds.iter().copied().find(|&b| b > start).unwrap_or(para.len()))
                    }
                };
                texts.push(para[start..end].trim_end().to_string());
                start = end;
                while start < para.len() && para[start..].starts_with(char::is_whitespace) {
                    start += para[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                }
            }
        }
        if let Some(max) = max_lines {
            if texts.len() > max {
                let rest = texts[max - 1..].join(" ");
                texts.truncate(max - 1);
                let last = self.ellipsize(set, &rest, px, max_width);
                let mut lines: Vec<Line> = texts.iter().map(|t| self.line(set, t, px)).collect();
                lines.push(last);
                return lines;
            }
        }
        texts.iter().map(|t| self.line(set, t, px)).collect()
    }
}
