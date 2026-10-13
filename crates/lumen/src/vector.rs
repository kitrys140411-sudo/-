//! Small CPU canvas for vector art (cursors, icons): paths are rasterized by
//! FreeType (lumen-text) into coverage masks and composited here in
//! premultiplied RGBA.

use std::rc::Rc;

use lumen_text::{Library, Path};

pub struct Canvas {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, top-down.
    pub data: Vec<u8>,
    lib: Rc<Library>,
    /// Design units → pixels.
    pub scale: f32,
}

impl Canvas {
    pub fn new(lib: &Rc<Library>, width: u32, height: u32, scale: f32) -> Canvas {
        Canvas { width, height, data: vec![0; (width * height * 4) as usize], lib: lib.clone(), scale }
    }

    /// Scale a path from design units to pixels.
    pub fn path(&self, f: impl FnOnce(&mut Path, f32)) -> Path {
        let mut p = Path::new();
        f(&mut p, self.scale);
        p
    }

    /// Composite `color` (straight sRGB + alpha) with `coverage` over pixel `i`.
    #[inline]
    pub fn blend_at(&mut self, i: usize, coverage: u8, color: [f32; 4]) {
        if coverage == 0 {
            return;
        }
        let c = coverage as f32 / 255.0 * color[3];
        let o = i * 4;
        let k = 1.0 - c;
        for ch in 0..3 {
            self.data[o + ch] = (color[ch] * c * 255.0 + self.data[o + ch] as f32 * k).round().min(255.0) as u8;
        }
        self.data[o + 3] = (c * 255.0 + self.data[o + 3] as f32 * k).round().min(255.0) as u8;
    }

    /// Composite `mask` with `color` (straight sRGB + alpha) over the canvas.
    pub fn blend_mask(&mut self, mask: &[u8], color: [f32; 4]) {
        for (i, &m) in mask.iter().enumerate() {
            self.blend_at(i, m, color);
        }
    }

    pub fn fill(&mut self, path: &Path, color: [f32; 4]) {
        let m = path.rasterize(&self.lib, self.width, self.height, false);
        self.blend_mask(&m, color);
    }

    /// Fill with the even-odd rule: overlapping contours cut holes.
    pub fn fill_even_odd(&mut self, path: &Path, color: [f32; 4]) {
        let m = path.rasterize(&self.lib, self.width, self.height, true);
        self.blend_mask(&m, color);
    }

    pub fn stroke(&mut self, path: &Path, width: f32, closed: bool, color: [f32; 4]) {
        let m = path.stroke(&self.lib, self.width, self.height, width * self.scale, closed);
        self.blend_mask(&m, color);
    }

    /// A soft drop shadow of `path` (box blur of its coverage, offset).
    pub fn shadow(&mut self, path: &Path, dx: f32, dy: f32, radius: f32, alpha: f32) {
        let m = path.rasterize(&self.lib, self.width, self.height, false);
        let r = (radius * self.scale).round().max(1.0) as i32;
        let (w, h) = (self.width as i32, self.height as i32);
        let (ox, oy) = ((dx * self.scale).round() as i32, (dy * self.scale).round() as i32);
        let mut blurred = vec![0f32; m.len()];
        // Separable box blur, two passes (close to Gaussian).
        let mut tmp = vec![0f32; m.len()];
        let src: Vec<f32> = m.iter().map(|&v| v as f32 / 255.0).collect();
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for k in -r..=r {
                    let sx = x - ox + k;
                    let sy = y - oy;
                    if sx >= 0 && sx < w && sy >= 0 && sy < h {
                        acc += src[(sy * w + sx) as usize];
                    }
                }
                tmp[(y * w + x) as usize] = acc / (2 * r + 1) as f32;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0;
                for k in -r..=r {
                    let sy = y + k;
                    if sy >= 0 && sy < h {
                        acc += tmp[(sy * w + x) as usize];
                    }
                }
                blurred[(y * w + x) as usize] = acc / (2 * r + 1) as f32;
            }
        }
        let mask: Vec<u8> = blurred.iter().map(|v| (v * 255.0).round() as u8).collect();
        self.blend_mask(&mask, [0.0, 0.0, 0.0, alpha]);
    }
}
