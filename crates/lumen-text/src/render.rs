//! Painting laid-out text into premultiplied RGBA images.
//!
//! Glyphs are rasterized at 4 horizontal subpixel phases and cached. Coverage
//! is gamma-adjusted for the text color before it is blended (in sRGB, as the
//! compositor's GL pipeline blends): light text gets the coverage curve of
//! linear-light blending over a dark background, dark text the curve for a light
//! background, so stems keep their weight in both themes.

use std::collections::HashMap;
use std::rc::Rc;

use crate::font::RasterGlyph;
use crate::layout::Line;

pub const PHASES: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Premultiplied RGBA, rows top to bottom.
#[derive(Debug, Clone, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    /// Where the layout origin (left edge, top of the first line box) lies in
    /// the image: glyph ink may extend beyond the layout box.
    pub origin: (i32, i32),
    /// Size of the layout box (lines × line height, widest line).
    pub layout_size: (f32, f32),
}

#[derive(Default)]
pub struct GlyphCache {
    map: HashMap<(u64, u32, u8), Option<Rc<RasterGlyph>>>,
}

impl GlyphCache {
    pub fn get(&mut self, font: &crate::font::Font, id: u32, phase: u8) -> Option<Rc<RasterGlyph>> {
        if self.map.len() > 8192 {
            self.map.clear();
        }
        self.map
            .entry((font.key, id, phase))
            .or_insert_with(|| font.rasterize(id, phase as f32 / PHASES as f32).map(Rc::new))
            .clone()
    }
}

/// Coverage adjustment table for a text color.
pub fn coverage_lut(color: [f32; 4], gamma: f32) -> [u8; 256] {
    let lum = (0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]).clamp(0.0, 1.0);
    let inv = 1.0 / gamma.max(1.0);
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        let light = c.powf(inv);
        let dark = 1.0 - (1.0 - c).powf(inv);
        *v = ((dark + (light - dark) * lum) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    lut
}

pub struct Painter {
    pub cache: GlyphCache,
    /// 1.0 = plain sRGB-space blending, 2.2 = exact linear-light curves; the
    /// default sits in between (full linear blending makes light-on-dark text
    /// look heavy).
    pub gamma: f32,
}

impl Default for Painter {
    fn default() -> Self {
        Painter { cache: GlyphCache::default(), gamma: 1.6 }
    }
}

struct Placed {
    glyph: Rc<RasterGlyph>,
    x: i32,
    y: i32,
}

impl Painter {
    /// Paint `lines` with baselines `line_height` apart. `color` is straight
    /// (non-premultiplied) sRGB. `box_width` aligns lines within that width.
    pub fn paint(&mut self, lines: &[Line], line_height: f32, color: [f32; 4], align: Align, box_width: Option<f32>) -> Image {
        let widest = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
        let box_w = box_width.unwrap_or(widest);
        let ascent = lines.first().map(|l| l.ascent).unwrap_or(0.0);
        let mut placed = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let baseline = (ascent + i as f32 * line_height).round() as i32;
            let dx = match align {
                Align::Left => 0.0,
                Align::Center => ((box_w - line.width) / 2.0).max(0.0),
                Align::Right => (box_w - line.width).max(0.0),
            };
            for g in &line.glyphs {
                let pen = g.x + dx;
                let mut ix = pen.floor();
                let mut phase = ((pen - ix) * PHASES as f32).round() as u32;
                if phase == PHASES {
                    ix += 1.0;
                    phase = 0;
                }
                let Some(r) = self.cache.get(&g.font, g.id, phase as u8) else { continue };
                placed.push(Placed { x: ix as i32 + r.left, y: baseline + g.y.round() as i32 - r.top, glyph: r });
            }
        }
        let layout_h = if lines.is_empty() { 0.0 } else { ascent + (lines.len() - 1) as f32 * line_height + lines.last().unwrap().descent };
        let (mut x0, mut y0, mut x1, mut y1) = (0i32, 0i32, box_w.ceil() as i32, layout_h.ceil() as i32);
        for p in &placed {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x + p.glyph.width as i32);
            y1 = y1.max(p.y + p.glyph.height as i32);
        }
        let (w, h) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let mut data = vec![0u8; (w * h * 4) as usize];
        let lut = coverage_lut(color, self.gamma);
        let a = color[3].clamp(0.0, 1.0);
        let premul = [color[0] * a, color[1] * a, color[2] * a, a];
        for p in &placed {
            let r = &p.glyph;
            for gy in 0..r.height {
                let ty = p.y - y0 + gy as i32;
                for gx in 0..r.width {
                    let tx = p.x - x0 + gx as i32;
                    let o = ((ty as u32 * w + tx as u32) * 4) as usize;
                    let src: [f32; 4] = if r.color {
                        let i = ((gy * r.width + gx) * 4) as usize;
                        [r.data[i] as f32 * a, r.data[i + 1] as f32 * a, r.data[i + 2] as f32 * a, r.data[i + 3] as f32 * a]
                    } else {
                        let c = lut[r.data[(gy * r.width + gx) as usize] as usize] as f32;
                        [premul[0] * c, premul[1] * c, premul[2] * c, premul[3] * c]
                    };
                    if src[3] <= 0.0 {
                        continue;
                    }
                    let k = 1.0 - src[3] / 255.0;
                    for c in 0..4 {
                        data[o + c] = (src[c] + data[o + c] as f32 * k).round().min(255.0) as u8;
                    }
                }
            }
        }
        Image { width: w, height: h, data, origin: (-x0, -y0), layout_size: (box_w, layout_h) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_endpoints_and_direction() {
        let white = coverage_lut([1.0, 1.0, 1.0, 1.0], 2.2);
        let black = coverage_lut([0.0, 0.0, 0.0, 1.0], 2.2);
        assert_eq!((white[0], white[255]), (0, 255));
        assert_eq!((black[0], black[255]), (0, 255));
        // Light text is boosted at mid coverage, dark text attenuated.
        assert!(white[128] > 128 && black[128] < 128);
        let plain = coverage_lut([1.0, 1.0, 1.0, 1.0], 1.0);
        assert!((plain[128] as i32 - 128).abs() <= 1);
    }
}
