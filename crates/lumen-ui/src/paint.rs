//! The painter: drawing in logical pixels into the frame's draw list, with a
//! clip stack, an offset stack (animated movement) and an opacity stack
//! (fades). Fills are snapped to device pixels so edges stay crisp at any
//! scale; text keeps subpixel positions horizontally and whole-pixel
//! baselines.
//!
//! Blending happens in linear light (gamma-correct anti-aliasing and text),
//! but translucent theme colors are written the way designers and the
//! desktop's compositor mix them: in sRGB (a hover tint of 12 % white is
//! meant to look like 12 % of the way to white). Blended in linear light
//! such a tint looks several times brighter, so the alpha of a translucent
//! color is adjusted to give, over the window background, the sRGB result
//! it was designed for.

use std::rc::Rc;

use lumen_text::Library;

use crate::color::Color;
use crate::geom::{Rect, Vec2};
use crate::gfx::atlas::{Entry, Raster};
use crate::gfx::glyphs::GlyphKey;
use crate::gfx::{DrawList, GlyphInst, IRect, ImageInst, Prim, RectInst, Renderer};
use crate::icons::{self, Icon};
use crate::image::{Image, ImageCache};
use crate::text::{TextAlign, TextLayout};

/// Subpixel positions per pixel for text.
const PHASES: f32 = 4.0;

pub struct Painter<'a> {
    list: &'a mut DrawList,
    renderer: &'a mut Renderer,
    images: &'a mut ImageCache,
    lib: &'a Rc<Library>,
    scale: f32,
    clip: IRect,
    clips: Vec<IRect>,
    offset: Vec2,
    offsets: Vec<Vec2>,
    opacity: f32,
    opacities: Vec<f32>,
    /// The window background (the typical backdrop of translucent colors).
    backdrop: Color,
    /// The last translucent color adjusted, and the alpha it got.
    last_alpha: std::cell::Cell<(Color, f32)>,
}

impl<'a> Painter<'a> {
    pub(crate) fn new(
        list: &'a mut DrawList,
        renderer: &'a mut Renderer,
        images: &'a mut ImageCache,
        lib: &'a Rc<Library>,
        scale: f32,
        backdrop: Color,
    ) -> Painter<'a> {
        let (w, h) = renderer.canvas_size;
        Painter {
            list,
            renderer,
            images,
            lib,
            scale,
            clip: IRect::new(0, 0, w, h),
            clips: Vec::new(),
            offset: Vec2::ZERO,
            offsets: Vec::new(),
            opacity: 1.0,
            opacities: Vec::new(),
            backdrop,
            last_alpha: std::cell::Cell::new((Color::TRANSPARENT, 0.0)),
        }
    }

    /// Device pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn color(&self, c: Color) -> [f32; 4] {
        let c = c.fade(self.opacity);
        let a = self.blend_alpha(c);
        let [r, g, b, _] = c.with_alpha(1.0).linear_premul();
        [r * a, g * a, b * a, a]
    }

    /// The alpha that makes `c` blended in linear light over the backdrop
    /// look like `c` blended in sRGB (see the module docs).
    fn blend_alpha(&self, c: Color) -> f32 {
        let a = c.a.clamp(0.0, 1.0);
        if a <= 0.0 || a >= 1.0 {
            return a;
        }
        let (last, alpha) = self.last_alpha.get();
        if last == c {
            return alpha;
        }
        let out = c.srgb_blend_alpha(self.backdrop);
        self.last_alpha.set((c, out));
        out
    }

    /// Physical rectangle of a logical one, edges snapped to pixels.
    fn snap(&self, r: Rect) -> [f32; 4] {
        let s = self.scale;
        let x0 = ((r.x + self.offset.x) * s).round();
        let y0 = ((r.y + self.offset.y) * s).round();
        let x1 = ((r.right() + self.offset.x) * s).round();
        let y1 = ((r.bottom() + self.offset.y) * s).round();
        [x0, y0, x1 - x0, y1 - y0]
    }

    fn phys(&self, p: Vec2) -> (f32, f32) {
        ((p.x + self.offset.x) * self.scale, (p.y + self.offset.y) * self.scale)
    }

    fn push(&mut self, prim: Prim) {
        self.list.push(prim, self.clip);
    }

    // -- State stacks ------------------------------------------------------------

    /// Restrict drawing to `r` (intersected with the current clip) until
    /// `pop_clip`.
    pub fn push_clip(&mut self, r: Rect) {
        let [x, y, w, h] = self.snap(r);
        self.clips.push(self.clip);
        self.clip = self.clip.intersect(&IRect::new(x as i32, y as i32, w as i32, h as i32));
    }

    pub fn pop_clip(&mut self) {
        if let Some(c) = self.clips.pop() {
            self.clip = c;
        }
    }

    /// The current clip in logical coordinates.
    pub fn clip_rect(&self) -> Rect {
        let s = self.scale;
        Rect::new(self.clip.x as f32 / s - self.offset.x, self.clip.y as f32 / s - self.offset.y, self.clip.w as f32 / s, self.clip.h as f32 / s)
    }

    /// Shift everything drawn until `pop_offset` by `d`.
    pub fn push_offset(&mut self, d: Vec2) {
        self.offsets.push(self.offset);
        self.offset = self.offset + d;
    }

    pub fn pop_offset(&mut self) {
        if let Some(o) = self.offsets.pop() {
            self.offset = o;
        }
    }

    /// Multiply the alpha of everything drawn until `pop_opacity` by `k`.
    pub fn push_opacity(&mut self, k: f32) {
        self.opacities.push(self.opacity);
        self.opacity *= k.clamp(0.0, 1.0);
    }

    pub fn pop_opacity(&mut self) {
        if let Some(o) = self.opacities.pop() {
            self.opacity = o;
        }
    }

    // -- Shapes ---------------------------------------------------------------------

    pub fn fill(&mut self, r: Rect, color: Color) {
        self.rounded(r, 0.0, color);
    }

    /// A filled rectangle with corner radius `radius` (half the height makes
    /// a pill).
    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        if color.is_transparent() || r.is_empty() {
            return;
        }
        let [x, y, w, h] = self.snap(r);
        let c = self.color(color);
        self.push(Prim::Rect(RectInst::rounded(x, y, w, h, radius * self.scale, c)));
    }

    /// A border of `width` inside the rectangle's edge.
    pub fn border(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        if color.is_transparent() || r.is_empty() {
            return;
        }
        let [x, y, w, h] = self.snap(r);
        let c = self.color(color);
        let bw = (width * self.scale).max(1.0);
        self.push(Prim::Rect(RectInst::border(x, y, w, h, radius * self.scale, bw, c)));
    }

    /// The soft shadow of a rounded box, blurred by `blur` (≈ the CSS
    /// blur radius) and moved down by `dy`.
    pub fn shadow(&mut self, r: Rect, radius: f32, blur: f32, dy: f32, color: Color) {
        if color.is_transparent() || r.is_empty() {
            return;
        }
        let s = self.scale;
        let (x, y) = self.phys(Vec2::new(r.x, r.y + dy));
        let c = self.color(color);
        self.push(Prim::Rect(RectInst::shadow(x, y, r.w * s, r.h * s, radius * s, (blur * 0.5 * s).max(0.5), c)));
    }

    /// A rounded fill fading from `color` to `color` × `end_alpha`, top to
    /// bottom (or left to right when `horizontal`).
    pub fn gradient(&mut self, r: Rect, radius: f32, color: Color, end_alpha: f32, horizontal: bool) {
        if color.is_transparent() || r.is_empty() {
            return;
        }
        let [x, y, w, h] = self.snap(r);
        let c = self.color(color);
        self.push(Prim::Rect(RectInst::gradient(x, y, w, h, radius * self.scale, end_alpha, horizontal, c)));
    }

    /// An anti-aliased line with round caps.
    pub fn line(&mut self, a: Vec2, b: Vec2, width: f32, color: Color) {
        if color.is_transparent() {
            return;
        }
        let c = self.color(color);
        let (pa, pb) = (self.phys(a), self.phys(b));
        self.push(Prim::Rect(RectInst::line(pa, pb, width * self.scale, c)));
    }

    /// Connected line segments (charts).
    pub fn polyline(&mut self, pts: &[Vec2], width: f32, color: Color) {
        for w in pts.windows(2) {
            self.line(w[0], w[1], width, color);
        }
    }

    /// Fill the area between the line through `pts` (ordered by x) and the
    /// horizontal line `base_y`, anti-aliased along the top edge; the alpha
    /// goes from `color`'s at the top of the area to `color`'s × `end_alpha`
    /// at `base_y` (1 for a flat fill). Graphs: `area` under a `polyline`.
    pub fn area(&mut self, pts: &[Vec2], base_y: f32, color: Color, end_alpha: f32) {
        if color.is_transparent() || pts.len() < 2 {
            return;
        }
        let c = self.color(color);
        let phys: Vec<(f32, f32)> = pts.iter().map(|&p| self.phys(p)).collect();
        let (_, base) = self.phys(Vec2::new(0.0, base_y));
        let top = phys.iter().map(|p| p.1).fold(base, f32::min).floor();
        let x0 = phys[0].0.round();
        let x1 = phys[phys.len() - 1].0.round();
        // Only the columns inside the clip need instances.
        let cx0 = self.clip.x as f32;
        let cx1 = (self.clip.x + self.clip.w) as f32;
        let (from, to) = (x0.max(cx0), x1.min(cx1));
        if to <= from || base <= top {
            return;
        }
        let mut seg = 0;
        let mut y_at = |x: f32| -> f32 {
            while seg + 2 < phys.len() && phys[seg + 1].0 < x {
                seg += 1;
            }
            let (a, b) = (phys[seg], phys[seg + 1]);
            if b.0 <= a.0 {
                return b.1;
            }
            a.1 + (b.1 - a.1) * ((x - a.0) / (b.0 - a.0)).clamp(0.0, 1.0)
        };
        let mut x = from;
        let mut left = y_at(x);
        while x < to {
            let right = y_at(x + 1.0);
            self.push(Prim::Rect(RectInst::area_column(x, top, base, left.min(base), right.min(base), end_alpha, c)));
            left = right;
            x += 1.0;
        }
    }

    /// An arc of the ring at `center` (outer radius `radius`, `thickness`),
    /// from `start` radians clockwise from 12 o'clock over `sweep` radians.
    pub fn arc(&mut self, center: Vec2, radius: f32, thickness: f32, start: f32, sweep: f32, color: Color) {
        if color.is_transparent() {
            return;
        }
        let c = self.color(color);
        let (cx, cy) = self.phys(center);
        let s = self.scale;
        self.push(Prim::Rect(RectInst::arc(cx, cy, radius * s, thickness * s, start, sweep, c)));
    }

    // -- Text, icons, images ------------------------------------------------------------

    /// Draw a laid-out text with its box at `pos` (top-left), lines aligned
    /// within `width` (or the layout's own width).
    pub fn text(&mut self, layout: &TextLayout, pos: Vec2, color: Color, align: TextAlign, width: Option<f32>) {
        if color.is_transparent() {
            return;
        }
        let s = self.scale;
        let col = self.color(color);
        let box_w = width.unwrap_or(layout.size.x) * s;
        let (ox, oy) = self.phys(pos);
        // Skip lines outside the clip (long texts in scroll areas).
        let (clip_y0, clip_y1) = (self.clip.y as f32, (self.clip.y + self.clip.h) as f32);
        for (i, line) in layout.lines.iter().enumerate() {
            let top = oy + i as f32 * layout.line_px * s;
            if top > clip_y1 || top + layout.line_px * s < clip_y0 {
                continue;
            }
            let baseline = (top + layout.baseline * s).round();
            let dx = match align {
                TextAlign::Start => 0.0,
                TextAlign::Center => ((box_w - line.width) / 2.0).max(0.0),
                TextAlign::End => (box_w - line.width).max(0.0),
            };
            for g in &line.glyphs {
                if g.id == 0 {
                    continue;
                }
                let pen = ox + dx + g.x;
                let mut ix = pen.floor();
                let mut phase = ((pen - ix) * PHASES).round() as u8;
                if phase as f32 >= PHASES {
                    ix += 1.0;
                    phase = 0;
                }
                let key = GlyphKey::Face { font: g.font.key, glyph: g.id, phase };
                let font = &g.font;
                let id = g.id;
                let entry = self.renderer.atlas.get(&key, || {
                    let r = font.rasterize(id, phase as f32 / PHASES)?;
                    Some(Raster { width: r.width, height: r.height, data: r.data, left: r.left as f32, top: r.top as f32, color: r.color })
                });
                if let Some(e) = entry {
                    let c = if e.color { [1.0, 1.0, 1.0, col[3]] } else { col };
                    self.push(Prim::Glyph(glyph_inst(&e, ix, baseline + g.y.round(), c)));
                }
            }
        }
    }

    /// Draw `icon` centered in `r` at `size` logical pixels, tinted `color`.
    pub fn icon(&mut self, icon: Icon, r: Rect, size: f32, color: Color) {
        if color.is_transparent() {
            return;
        }
        let px = (size * self.scale).round().clamp(1.0, 1024.0) as u32;
        let (cx, cy) = self.phys(r.center());
        // Whole-pixel position: icons are rasterized for the pixel grid.
        let x = (cx - px as f32 / 2.0).round();
        let y = (cy - px as f32 / 2.0).round();
        let key = GlyphKey::Icon { icon: icon as u16, px: px as u16 };
        let lib = self.lib;
        let entry = self.renderer.atlas.get(&key, || {
            Some(Raster { width: px, height: px, data: icons::rasterize(lib, icon, px), left: 0.0, top: 0.0, color: false })
        });
        if let Some(e) = entry {
            let c = self.color(color);
            self.push(Prim::Glyph(GlyphInst { rect: [x, y, e.width as f32, e.height as f32], uv: e.uv, color: c, kind: 0.0 }));
        }
    }

    /// Draw `img` stretched over `r` with rounded corners.
    pub fn image(&mut self, img: &Image, r: Rect, radius: f32) {
        self.image_uv(img, r, [0.0, 0.0, 1.0, 1.0], radius);
    }

    /// Draw the part `uv` (u0, v0, u1, v1 in 0..1) of `img` over `r`.
    pub fn image_uv(&mut self, img: &Image, r: Rect, uv: [f32; 4], radius: f32) {
        if r.is_empty() || self.opacity <= 0.0 {
            return;
        }
        let Some(tex) = self.images.texture(self.renderer, img) else { return };
        let s = self.scale;
        let (x, y) = self.phys(r.min());
        self.push(Prim::Image(tex, ImageInst { rect: [x, y, r.w * s, r.h * s], uv, params: [radius * s, self.opacity, 0.0, 0.0] }));
    }
}

fn glyph_inst(e: &Entry, pen_x: f32, baseline: f32, color: [f32; 4]) -> GlyphInst {
    GlyphInst {
        rect: [pen_x + e.left, baseline - e.top, e.width as f32, e.height as f32],
        uv: e.uv,
        color,
        kind: if e.color { 1.0 } else { 0.0 },
    }
}
