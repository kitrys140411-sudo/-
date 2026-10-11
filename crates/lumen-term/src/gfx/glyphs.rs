//! Fonts for the terminal: four styles of the configured monospace family
//! (with the usual fallback chain behind each), cell metrics, cluster →
//! glyph resolution and rasterization at subpixel phases.

use std::collections::HashMap;
use std::rc::Rc;

use lumen_text::{FontDb, FontSet, Shaper, Weight};

use super::atlas::Raster;
use super::boxdraw;

pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;

/// Stands for "no font has this character" (drawn as a hollow box).
pub const TOFU: char = '\u{10FFFF}';

/// Subpixel positions per pixel for glyph origins.
pub const PHASES: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// Advance per cell (fractional: glyphs are positioned with subpixel
    /// precision, backgrounds are snapped to whole pixels).
    pub w: f32,
    pub h: u32,
    /// Baseline below the cell top.
    pub baseline: f32,
    pub underline_pos: f32,
    pub underline_thick: f32,
    pub strike_pos: f32,
    /// Light line thickness for box drawing and the bar cursor.
    pub line: u32,
}

/// A glyph to draw for a cluster, offset from the cell's pen position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub style: u8,
    pub font: u16,
    pub glyph: u32,
    pub dx: f32,
    pub dy: f32,
    pub color: bool,
}

/// What the atlas caches: a glyph at a phase, or a procedural symbol at a
/// cell size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlyphKey {
    Font { style: u8, font: u16, glyph: u32, phase: u8 },
    Symbol { c: char, w: u16, h: u16 },
    Ui { font: u16, glyph: u32, phase: u8 },
}

pub struct Fonts {
    pub db: FontDb,
    sets: Vec<FontSet>,
    pub ui: Option<FontSet>,
    pub px: f32,
    pub ui_px: f32,
    pub cell: CellMetrics,
    shaper: Shaper,
    singles: HashMap<(char, u8), Option<Placed>>,
    clusters: HashMap<(String, u8), Rc<[Placed]>>,
}

impl Fonts {
    pub fn new(family: &str, px: f32, line_height: f32, letter_spacing: f32) -> Result<Fonts, String> {
        let db = FontDb::scan().map_err(|e| e.0)?;
        let families: Vec<&str> = vec![family, "JetBrains Mono", "DejaVu Sans Mono", "Noto Sans Mono"];
        let mut sets = Vec::new();
        for style in 0..4u8 {
            let weight = if style & BOLD != 0 { Weight::Bold } else { Weight::Regular };
            let set = FontSet::new_styled(&db, &families, weight, style & ITALIC != 0).map_err(|e| e.0)?;
            sets.push(set);
        }
        let ui = FontSet::new(&db, &["Inter", family], Weight::Medium).ok();
        let mut f = Fonts {
            db,
            sets,
            ui,
            px,
            ui_px: px,
            cell: CellMetrics { w: 8.0, h: 16, baseline: 12.0, underline_pos: 14.0, underline_thick: 1.0, strike_pos: 8.0, line: 1 },
            shaper: Shaper::new(),
            singles: HashMap::new(),
            clusters: HashMap::new(),
        };
        f.set_size(px, line_height, letter_spacing);
        Ok(f)
    }

    pub fn family(&self) -> String {
        self.sets[0].primary().family.clone()
    }

    pub fn set_size(&mut self, px: f32, line_height: f32, letter_spacing: f32) {
        self.px = px.max(4.0);
        self.ui_px = (self.px * 0.93).max(4.0);
        self.singles.clear();
        self.clusters.clear();
        let Some(font) = self.sets[0].font(0, self.px) else { return };
        let adv = self.shaper.shape(&font, "0", &[]).first().map(|g| g.x_advance).unwrap_or(self.px * 0.6);
        let m = font.metrics;
        let natural = m.ascent + m.descent;
        let h = (natural.max(m.line_height) * line_height.max(0.5)).round().max(1.0);
        let baseline = ((h - natural) / 2.0 + m.ascent).round();
        let thick = m.underline_thickness.max(1.0).round();
        self.cell = CellMetrics {
            w: (adv + letter_spacing).max(1.0),
            h: h as u32,
            baseline,
            underline_pos: (baseline + m.underline_position.max(1.0)).round().min(h - thick),
            underline_thick: thick,
            strike_pos: (baseline - m.ascent * 0.3).round(),
            line: thick as u32,
        };
    }

    fn set(&self, style: u8) -> &FontSet {
        &self.sets[(style & 3) as usize]
    }

    /// Glyphs for one cell's grapheme in `style`.
    pub fn lookup(&mut self, text: &str, style: u8) -> Rc<[Placed]> {
        let mut chars = text.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let p = self.single(c, style);
            return match p {
                Some(p) => Rc::from([p].as_slice()),
                None => Rc::from([].as_slice()),
            };
        }
        let key = (text.to_string(), style);
        if let Some(v) = self.clusters.get(&key) {
            return v.clone();
        }
        let px = self.px;
        let set = &self.sets[(style & 3) as usize];
        let idx = set.select(text, px);
        let placed: Vec<Placed> = match set.font(idx, px) {
            Some(font) => {
                let shaped = self.shaper.shape(&font, text, &[]);
                let mut x = 0.0;
                shaped
                    .iter()
                    .filter(|g| g.id != 0)
                    .map(|g| {
                        let p = Placed { style, font: idx as u16, glyph: g.id, dx: x + g.x_offset, dy: g.y_offset, color: font.color };
                        x += g.x_advance;
                        p
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        if self.clusters.len() > 4096 {
            self.clusters.clear();
        }
        let v: Rc<[Placed]> = Rc::from(placed);
        self.clusters.insert(key, v.clone());
        v
    }

    /// A single character (the common case, without shaping).
    pub fn single(&mut self, c: char, style: u8) -> Option<Placed> {
        if let Some(p) = self.singles.get(&(c, style)) {
            return *p;
        }
        let px = self.px;
        let set = &self.sets[(style & 3) as usize];
        let mut buf = [0u8; 4];
        let idx = set.select(c.encode_utf8(&mut buf), px);
        let p = set.font(idx, px).and_then(|font| {
            let g = font.glyph_index(c);
            (g != 0).then_some(Placed { style, font: idx as u16, glyph: g, dx: 0.0, dy: 0.0, color: font.color })
        });
        if self.singles.len() > 16384 {
            self.singles.clear();
        }
        self.singles.insert((c, style), p);
        p
    }

    /// Rasterize a placed glyph at `phase` (0..PHASES). Color glyphs are
    /// scaled to fit `cells` cells.
    pub fn raster(&self, p: &Placed, phase: u8, cells: u32) -> Option<Raster> {
        let font = self.set(p.style).font(p.font as usize, self.px)?;
        let g = font.rasterize(p.glyph, phase as f32 / PHASES)?;
        if !g.color {
            return Some(Raster { width: g.width, height: g.height, data: g.data, left: g.left as f32, top: g.top as f32, color: false });
        }
        // Emoji: fit the box of `cells` cells (keeping the aspect ratio).
        let box_w = self.cell.w * cells as f32;
        let box_h = self.cell.h as f32;
        let k = (box_w / g.width as f32).min(box_h / g.height as f32).min(1.0);
        let (w, h) = (((g.width as f32 * k).round() as u32).max(1), ((g.height as f32 * k).round() as u32).max(1));
        let data = if k < 0.999 { lumen_text::font::resample_rgba(&g.data, g.width, g.height, w, h) } else { g.data };
        // Centered in the box, vertically centered on the cell.
        let left = ((box_w - w as f32) / 2.0).round();
        let top = (self.cell.baseline - (box_h - h as f32) / 2.0).round();
        Some(Raster { width: w, height: h, data, left, top, color: true })
    }

    /// Procedural symbol (box drawing...) covering a `w`×`h` cell.
    pub fn symbol(&self, c: char, w: u32, h: u32) -> Option<Raster> {
        if c == TOFU {
            // A hollow box for characters no installed font has.
            let t = self.cell.line.max(1) as i32;
            let (wi, hi) = (w as i32, h as i32);
            let (x0, x1) = ((wi / 8).max(1), wi - (wi / 8).max(1));
            let (y0, y1) = (hi / 5, hi - hi / 6);
            let mut data = vec![0u8; (w * h) as usize];
            for y in y0..y1 {
                for x in x0..x1 {
                    if x < x0 + t || x >= x1 - t || y < y0 + t || y >= y1 - t {
                        data[(y * wi + x) as usize] = 160;
                    }
                }
            }
            return Some(Raster { width: w, height: h, data, left: 0.0, top: self.cell.baseline, color: false });
        }
        let data = boxdraw::render(&self.db.lib, c, w, h, self.cell.line)?;
        Some(Raster { width: w, height: h, data, left: 0.0, top: self.cell.baseline, color: false })
    }

    // -- UI text (tab titles, search bar) in the proportional UI font --

    /// Shape a UI string: glyphs with x positions, and the total width.
    pub fn ui_line(&mut self, text: &str) -> (Vec<(u16, u32, f32, f32)>, f32) {
        let px = self.ui_px;
        let Some(set) = self.ui.as_ref() else { return (Vec::new(), 0.0) };
        let mut out = Vec::new();
        let mut x = 0.0;
        use unicode_segmentation::UnicodeSegmentation;
        // Itemize by font: consecutive graphemes from the same fallback font.
        let mut runs: Vec<(usize, String)> = Vec::new();
        for g in text.graphemes(true) {
            let i = set.select(g, px);
            match runs.last_mut() {
                Some((fi, s)) if *fi == i => s.push_str(g),
                _ => runs.push((i, g.to_string())),
            }
        }
        for (i, run) in runs {
            let Some(font) = set.font(i, px) else { continue };
            for g in self.shaper.shape(&font, &run, &[]) {
                if g.id != 0 {
                    out.push((i as u16, g.id, x + g.x_offset, g.y_offset));
                }
                x += g.x_advance;
            }
        }
        (out, x)
    }

    pub fn ui_metrics(&self) -> (f32, f32) {
        match self.ui.as_ref().and_then(|s| s.font(0, self.ui_px)) {
            Some(f) => (f.metrics.ascent, f.metrics.descent),
            None => (self.ui_px * 0.8, self.ui_px * 0.2),
        }
    }

    pub fn ui_raster(&self, font: u16, glyph: u32, phase: u8) -> Option<Raster> {
        let f = self.ui.as_ref()?.font(font as usize, self.ui_px)?;
        let g = f.rasterize(glyph, phase as f32 / PHASES)?;
        if g.color {
            let k = (self.ui_px * 1.15 / g.height as f32).min(1.0);
            let (w, h) = (((g.width as f32 * k).round() as u32).max(1), ((g.height as f32 * k).round() as u32).max(1));
            let data = lumen_text::font::resample_rgba(&g.data, g.width, g.height, w, h);
            return Some(Raster { width: w, height: h, data, left: g.left as f32 * k, top: g.top as f32 * k, color: true });
        }
        Some(Raster { width: g.width, height: g.height, data: g.data, left: g.left as f32, top: g.top as f32, color: false })
    }
}
