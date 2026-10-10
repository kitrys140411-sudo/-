//! Software rendering into an XRGB8888 back buffer.
//!
//! The UEFI target is soft-float, so every per-pixel path uses integer math:
//! blending happens in linear light through lookup tables (12-bit linear), rounded
//! corners use precomputed 4x4-supersampled coverage masks, and shadows are box
//! blurs of an alpha mask.

use alloc::vec;
use alloc::vec::Vec;

use crate::assets::{LINEAR_TO_SRGB, SRGB_TO_LINEAR};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn hex(v: u32) -> Self {
        Color {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        }
    }

    pub const fn pixel(self) -> u32 {
        (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    pub fn linear(self) -> Lin {
        Lin {
            r: SRGB_TO_LINEAR[self.r as usize] as i32,
            g: SRGB_TO_LINEAR[self.g as usize] as i32,
            b: SRGB_TO_LINEAR[self.b as usize] as i32,
        }
    }
}

/// A color in linear light, 12 bits per channel.
#[derive(Clone, Copy, Debug)]
pub struct Lin {
    pub r: i32,
    pub g: i32,
    pub b: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn inset(&self, d: i32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.w <= 0 || self.h <= 0 {
            return *o;
        }
        if o.w <= 0 || o.h <= 0 {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    pub fn clip(&self, w: i32, h: i32) -> Rect {
        let x0 = self.x.clamp(0, w);
        let y0 = self.y.clamp(0, h);
        let x1 = self.right().clamp(0, w);
        let y1 = self.bottom().clamp(0, h);
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

#[inline]
fn blend_channel(dst: u8, src_lin: i32, a: i32) -> u8 {
    let d = SRGB_TO_LINEAR[dst as usize] as i32;
    let o = d + (((src_lin - d) * a + 127) / 255);
    LINEAR_TO_SRGB[o.clamp(0, 4095) as usize]
}

/// Blend `src` over the pixel with coverage/alpha `a` (0..=255) in linear light.
#[inline]
pub fn blend(dst: u32, src: Lin, a: i32) -> u32 {
    if a <= 0 {
        return dst;
    }
    if a >= 255 {
        return (LINEAR_TO_SRGB[src.r as usize] as u32) << 16
            | (LINEAR_TO_SRGB[src.g as usize] as u32) << 8
            | LINEAR_TO_SRGB[src.b as usize] as u32;
    }
    let r = blend_channel((dst >> 16) as u8, src.r, a) as u32;
    let g = blend_channel((dst >> 8) as u8, src.g, a) as u32;
    let b = blend_channel(dst as u8, src.b, a) as u32;
    r << 16 | g << 8 | b
}

/// 4x4 Bayer matrix, values 0..15, used to dither gradients.
const BAYER4: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Coverage masks for rounded corners, cached per radius.
pub struct CornerCache {
    masks: Vec<(i32, Vec<u8>)>,
}

impl CornerCache {
    pub const fn new() -> Self {
        CornerCache { masks: Vec::new() }
    }

    /// Coverage (0..=255) of the pixel at (i, j) in an r x r corner whose circle
    /// center is at (r, r); (0, 0) is the outermost pixel.
    fn mask(&mut self, r: i32) -> &[u8] {
        if let Some(pos) = self.masks.iter().position(|(rr, _)| *rr == r) {
            return &self.masks[pos].1;
        }
        let n = r.max(0) as usize;
        let mut m = vec![0u8; n * n];
        let big_r = 8 * r as i64;
        for j in 0..n {
            for i in 0..n {
                let mut inside = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let x = 8 * i as i64 + 2 * sx + 1;
                        let y = 8 * j as i64 + 2 * sy + 1;
                        let dx = big_r - x;
                        let dy = big_r - y;
                        if dx * dx + dy * dy <= big_r * big_r {
                            inside += 1;
                        }
                    }
                }
                m[j * n + i] = ((inside * 255 + 8) / 16) as u8;
            }
        }
        self.masks.push((r, m));
        &self.masks.last().unwrap().1
    }
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Self {
        Canvas {
            w,
            h,
            px: vec![0; (w * h) as usize],
        }
    }

    pub fn copy_from(&mut self, src: &Canvas, r: Rect) {
        let r = r.clip(self.w, self.h);
        for y in r.y..r.bottom() {
            let a = (y * self.w + r.x) as usize;
            self.px[a..a + r.w as usize].copy_from_slice(&src.px[a..a + r.w as usize]);
        }
    }

    pub fn fill(&mut self, r: Rect, c: Color) {
        let r = r.clip(self.w, self.h);
        let p = c.pixel();
        for y in r.y..r.bottom() {
            let a = (y * self.w + r.x) as usize;
            self.px[a..a + r.w as usize].fill(p);
        }
    }

    pub fn fill_alpha(&mut self, r: Rect, c: Color, alpha: i32) {
        let r = r.clip(self.w, self.h);
        let lin = c.linear();
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            for x in r.x..r.right() {
                let i = row + x as usize;
                self.px[i] = blend(self.px[i], lin, alpha);
            }
        }
    }

    /// Vertical gradient `top` -> `bottom` plus an elliptical glow, dithered.
    pub fn background(&mut self, top: Color, bottom: Color, glow: Color, glow_strength: i32, glow_center: (i32, i32), glow_radius: (i32, i32)) {
        let (gx, gy) = glow_center;
        let (rx, ry) = (glow_radius.0.max(1) as i64, glow_radius.1.max(1) as i64);
        let h = self.h.max(2) as i64;
        for y in 0..self.h {
            let t = y as i64 * 256 / (h - 1);
            // 8.8 fixed point sRGB.
            let mix = |a: u8, b: u8| -> i64 { a as i64 * (256 - t) + b as i64 * t };
            let base = [mix(top.r, bottom.r), mix(top.g, bottom.g), mix(top.b, bottom.b)];
            let dy = (y - gy) as i64;
            let row = (y * self.w) as usize;
            for x in 0..self.w {
                let dx = (x - gx) as i64;
                // Normalized squared distance in 1/65536 units.
                let d2 = dx * dx * 65536 / (rx * rx) + dy * dy * 65536 / (ry * ry);
                let mut c = base;
                if d2 < 65536 {
                    let f = 65536 - d2; // 0..65536, smooth falloff (squared)
                    let k = (f >> 8) * (f >> 8) >> 8; // 0..65536 >> 8 => 0..256
                    let k = k * glow_strength as i64 / 256;
                    c[0] += (glow.r as i64 * 256 - c[0]) * k / 256 / 4;
                    c[1] += (glow.g as i64 * 256 - c[1]) * k / 256 / 4;
                    c[2] += (glow.b as i64 * 256 - c[2]) * k / 256 / 4;
                }
                let dither = BAYER4[(y & 3) as usize][(x & 3) as usize] as i64 * 16 + 8;
                let q = |v: i64| ((v + dither) >> 8).clamp(0, 255) as u32;
                self.px[row + x as usize] = q(c[0]) << 16 | q(c[1]) << 8 | q(c[2]);
            }
        }
    }

    /// Coverage of pixel (x, y) inside rounded rect `r` with radius `rad`.
    #[inline]
    fn rr_coverage(corners: &[u8], rad: i32, r: &Rect, x: i32, y: i32) -> i32 {
        if x < r.x || y < r.y || x >= r.right() || y >= r.bottom() {
            return 0;
        }
        let i = if x < r.x + rad {
            x - r.x
        } else if x >= r.right() - rad {
            r.right() - 1 - x
        } else {
            return 255;
        };
        let j = if y < r.y + rad {
            y - r.y
        } else if y >= r.bottom() - rad {
            r.bottom() - 1 - y
        } else {
            return 255;
        };
        corners[(j * rad + i) as usize] as i32
    }

    /// Fill a rounded rectangle with antialiased corners.
    pub fn rounded_rect(&mut self, cache: &mut CornerCache, r: Rect, rad: i32, c: Color, alpha: i32) {
        let rad = rad.min(r.w / 2).min(r.h / 2).max(0);
        let corners = cache.mask(rad);
        let clip = r.clip(self.w, self.h);
        let lin = c.linear();
        for y in clip.y..clip.bottom() {
            let row = (y * self.w) as usize;
            for x in clip.x..clip.right() {
                let cov = Self::rr_coverage(corners, rad, &r, x, y);
                if cov > 0 {
                    let i = row + x as usize;
                    self.px[i] = blend(self.px[i], lin, cov * alpha / 255);
                }
            }
        }
    }

    /// Stroke the inside edge of a rounded rectangle with width `bw`.
    pub fn rounded_border(&mut self, cache: &mut CornerCache, r: Rect, rad: i32, bw: i32, c: Color, alpha: i32) {
        let rad = rad.min(r.w / 2).min(r.h / 2).max(0);
        let inner = r.inset(bw);
        let irad = (rad - bw).max(0);
        let outer_mask = cache.mask(rad).to_vec();
        let inner_mask = cache.mask(irad);
        let clip = r.clip(self.w, self.h);
        let lin = c.linear();
        for y in clip.y..clip.bottom() {
            let row = (y * self.w) as usize;
            // Only rows/columns near the edge can be non-zero.
            let edge_row = y < r.y + rad.max(bw) || y >= r.bottom() - rad.max(bw);
            for x in clip.x..clip.right() {
                if !edge_row && x >= r.x + rad.max(bw) && x < r.right() - rad.max(bw) {
                    continue;
                }
                let cov = Self::rr_coverage(&outer_mask, rad, &r, x, y)
                    - Self::rr_coverage(inner_mask, irad, &inner, x, y);
                if cov > 0 {
                    let i = row + x as usize;
                    self.px[i] = blend(self.px[i], lin, cov * alpha / 255);
                }
            }
        }
    }

    /// Soft drop shadow of a rounded rect: the rect's coverage mask, offset by `dy`,
    /// blurred with three box passes of radius `blur / 3`, composited as `c` at `alpha`.
    pub fn shadow(&mut self, cache: &mut CornerCache, r: Rect, rad: i32, blur: i32, dy: i32, c: Color, alpha: i32) {
        let pad = blur + 2;
        let area = Rect::new(r.x - pad, r.y - pad + dy, r.w + 2 * pad, r.h + 2 * pad);
        let (aw, ah) = (area.w as usize, area.h as usize);
        let rad = rad.min(r.w / 2).min(r.h / 2).max(0);
        let corners = cache.mask(rad);
        let local = Rect::new(pad, pad, r.w, r.h);
        let mut m: Vec<i32> = vec![0; aw * ah];
        for y in 0..ah as i32 {
            for x in 0..aw as i32 {
                m[y as usize * aw + x as usize] = Self::rr_coverage(corners, rad, &local, x, y);
            }
        }
        let br = (blur / 3).max(1) as usize;
        let mut tmp = vec![0i32; aw.max(ah)];
        for _ in 0..3 {
            for y in 0..ah {
                box_pass(&mut m[y * aw..(y + 1) * aw], 1, aw, br, &mut tmp);
            }
            for x in 0..aw {
                box_pass(&mut m[x..], aw, ah, br, &mut tmp);
            }
        }
        let lin = c.linear();
        for y in 0..ah as i32 {
            let sy = area.y + y;
            if sy < 0 || sy >= self.h {
                continue;
            }
            for x in 0..aw as i32 {
                let sx = area.x + x;
                if sx < 0 || sx >= self.w {
                    continue;
                }
                let a = m[y as usize * aw + x as usize] * alpha / 255;
                if a > 0 {
                    let i = (sy * self.w + sx) as usize;
                    self.px[i] = blend(self.px[i], lin, a);
                }
            }
        }
    }

    /// Scale the rectangle toward black: `f` in 0..=256 (256 = unchanged).
    pub fn fade(&mut self, r: Rect, f: u32) {
        if f >= 256 {
            return;
        }
        let r = r.clip(self.w, self.h);
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            for p in &mut self.px[row + r.x as usize..row + r.right() as usize] {
                let v = *p;
                let rr = ((v >> 16) & 0xff) * f >> 8;
                let gg = ((v >> 8) & 0xff) * f >> 8;
                let bb = (v & 0xff) * f >> 8;
                *p = rr << 16 | gg << 8 | bb;
            }
        }
    }
}

/// In-place box blur of `n` samples spaced `stride` apart, radius `r`.
fn box_pass(data: &mut [i32], stride: usize, n: usize, r: usize, tmp: &mut [i32]) {
    let win = (2 * r + 1) as i32;
    let get = |i: isize, d: &[i32]| -> i32 {
        if i < 0 || i >= n as isize { 0 } else { d[i as usize * stride] }
    };
    let mut sum: i32 = 0;
    for i in -(r as isize)..=(r as isize) {
        sum += get(i, data);
    }
    for i in 0..n {
        tmp[i] = (sum + win / 2) / win;
        sum += get(i as isize + r as isize + 1, data) - get(i as isize - r as isize, data);
    }
    for i in 0..n {
        data[i * stride] = tmp[i];
    }
}
