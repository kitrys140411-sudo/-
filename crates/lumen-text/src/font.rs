//! Font files and sized font instances (FreeType face + HarfBuzz font).
//!
//! Everything here is single-threaded (FreeType libraries and faces are not
//! thread-safe), hence `Rc` and no `Send`.

use std::ffi::{c_long, CStr};
use std::fmt;
use std::rc::Rc;

use freetype_sys as ft;
use harfbuzz_sys as hb;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn check(err: ft::FT_Error, what: &str) -> Result<(), Error> {
    if err == 0 { Ok(()) } else { Err(Error(format!("{what}: FreeType error {err}"))) }
}

unsafe extern "C" {
    // Part of libharfbuzz's FreeType integration (not in harfbuzz-sys).
    fn hb_ft_font_set_load_flags(font: *mut hb::hb_font_t, load_flags: i32);
}

/// The FreeType library handle.
pub struct Library {
    pub(crate) raw: ft::FT_Library,
}

impl Library {
    pub fn new() -> Result<Rc<Library>, Error> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: plain FFI call with a valid out pointer.
        check(unsafe { ft::FT_Init_FreeType(&mut raw) }, "FT_Init_FreeType")?;
        Ok(Rc::new(Library { raw }))
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: created by FT_Init_FreeType; every face holds an Rc to us.
        unsafe { ft::FT_Done_FreeType(self.raw) };
    }
}

/// One face of a font file (a .ttc holds several).
#[derive(Clone)]
pub struct FontFile {
    pub path: String,
    pub index: isize,
    pub family: String,
    pub style: String,
    /// Has color bitmaps (CBDT/sbix), e.g. Noto Color Emoji.
    pub color: bool,
    pub scalable: bool,
    data: Rc<Vec<u8>>,
}

impl fmt::Debug for FontFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} ({}#{})", self.family, self.style, self.path, self.index)
    }
}

/// Owned FT_Face (closed on drop).
struct Face {
    raw: ft::FT_Face,
    _lib: Rc<Library>,
    _data: Rc<Vec<u8>>,
}

impl Face {
    fn open(lib: &Rc<Library>, data: &Rc<Vec<u8>>, index: isize) -> Result<Face, Error> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: the data outlives the face (kept in `_data`).
        check(
            unsafe { ft::FT_New_Memory_Face(lib.raw, data.as_ptr(), data.len() as c_long, index as c_long, &mut raw) },
            "FT_New_Memory_Face",
        )?;
        Ok(Face { raw, _lib: lib.clone(), _data: data.clone() })
    }

    fn rec(&self) -> &ft::FT_FaceRec {
        // SAFETY: valid while the face is open.
        unsafe { &*self.raw }
    }
}

impl Drop for Face {
    fn drop(&mut self) {
        // SAFETY: opened by FT_New_Memory_Face.
        unsafe { ft::FT_Done_Face(self.raw) };
    }
}

fn cstr(p: *const std::ffi::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: FreeType returns NUL-terminated strings.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

impl FontFile {
    /// All faces in a font file.
    pub fn load_all(lib: &Rc<Library>, path: &str) -> Result<Vec<FontFile>, Error> {
        let data = Rc::new(std::fs::read(path).map_err(|e| Error(format!("{path}: {e}")))?);
        let first = Face::open(lib, &data, 0)?;
        let count = first.rec().num_faces.max(1) as isize;
        drop(first);
        let mut out = Vec::new();
        for index in 0..count {
            let face = Face::open(lib, &data, index)?;
            let r = face.rec();
            out.push(FontFile {
                path: path.to_string(),
                index,
                family: cstr(r.family_name),
                style: cstr(r.style_name),
                color: r.face_flags & ft::FT_FACE_FLAG_COLOR != 0,
                scalable: r.face_flags & ft::FT_FACE_FLAG_SCALABLE != 0,
                data: data.clone(),
            });
        }
        Ok(out)
    }

    /// An instance of this face at `px` pixels per em.
    pub fn instance(&self, lib: &Rc<Library>, px: f32) -> Result<Font, Error> {
        Font::new(lib, self, px)
    }
}

/// Vertical metrics in pixels (descent positive, below the baseline).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
}

/// A face at one size, ready for shaping and rasterization.
pub struct Font {
    face: Face,
    pub(crate) hb: *mut hb::hb_font_t,
    pub px: f32,
    /// Bitmap-only faces (color emoji) render a fixed strike that is scaled by
    /// this factor to the requested size; 1 for outline fonts.
    pub bitmap_scale: f32,
    pub metrics: Metrics,
    pub color: bool,
    /// Identifies (file, face, size) for glyph caches.
    pub key: u64,
    load_flags: i32,
}

/// The rasterized glyph (left/top relative to the pen position on the baseline,
/// top is positive upwards as in FreeType).
#[derive(Debug, Clone)]
pub struct RasterGlyph {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// Coverage (8-bit) for outline glyphs; premultiplied RGBA for color glyphs.
    pub data: Vec<u8>,
    pub color: bool,
}

const FIXED: f32 = 64.0;

impl Font {
    fn new(lib: &Rc<Library>, file: &FontFile, px: f32) -> Result<Font, Error> {
        let face = Face::open(lib, &file.data, file.index)?;
        let px = px.max(1.0);
        let mut bitmap_scale = 1.0;
        let rec = face.rec();
        if rec.face_flags & ft::FT_FACE_FLAG_SCALABLE != 0 {
            // 72 dpi: one point is one pixel.
            check(
                unsafe { ft::FT_Set_Char_Size(face.raw, 0, (px * FIXED).round() as ft::FT_F26Dot6, 72, 72) },
                "FT_Set_Char_Size",
            )?;
        } else if rec.num_fixed_sizes > 0 {
            // Smallest strike at least as large as wanted, else the largest.
            // SAFETY: available_sizes has num_fixed_sizes entries.
            let sizes = unsafe { std::slice::from_raw_parts(rec.available_sizes, rec.num_fixed_sizes as usize) };
            let ppem = |i: usize| sizes[i].y_ppem as f32 / FIXED;
            let best = (0..sizes.len())
                .filter(|&i| ppem(i) >= px)
                .min_by(|&a, &b| ppem(a).total_cmp(&ppem(b)))
                .or_else(|| (0..sizes.len()).max_by(|&a, &b| ppem(a).total_cmp(&ppem(b))))
                .unwrap_or(0);
            check(unsafe { ft::FT_Select_Size(face.raw, best as ft::FT_Int) }, "FT_Select_Size")?;
            bitmap_scale = px / ppem(best).max(1.0);
        } else {
            return Err(Error(format!("{file:?}: neither scalable nor bitmap strikes")));
        }
        let color = file.color;
        let load_flags = if color {
            ft::FT_LOAD_COLOR
        } else {
            // Light (vertical-only) hinting keeps advances unhinted, which is
            // what subpixel positioning needs, and baselines crisp.
            ft::FT_LOAD_TARGET_LIGHT
        };
        // SAFETY: the face is valid; HarfBuzz references it (not owning it).
        let hbf = unsafe { hb::freetype::hb_ft_font_create_referenced(face.raw) };
        unsafe { hb_ft_font_set_load_flags(hbf, load_flags) };
        let rec = face.rec();
        // SAFETY: size is set above.
        let m = unsafe { (*rec.size).metrics };
        let s = bitmap_scale / FIXED;
        let ascent = m.ascender as f32 * s;
        let descent = -(m.descender as f32) * s;
        let units = rec.units_per_EM.max(1) as f32;
        let em = px / units;
        let metrics = Metrics {
            ascent,
            descent,
            line_height: (m.height as f32 * s).max(ascent + descent),
            underline_position: if rec.underline_position != 0 { -(rec.underline_position as f32) * em } else { px * 0.1 },
            underline_thickness: if rec.underline_thickness != 0 { rec.underline_thickness as f32 * em } else { px / 14.0 },
        };
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            file.path.hash(&mut h);
            file.index.hash(&mut h);
            px.to_bits().hash(&mut h);
            h.finish()
        };
        Ok(Font { face, hb: hbf, px, bitmap_scale, metrics, color, key, load_flags })
    }

    pub fn glyph_index(&self, c: char) -> u32 {
        // SAFETY: valid face.
        unsafe { ft::FT_Get_Char_Index(self.face.raw, c as ft::FT_ULong) }
    }

    pub fn has_char(&self, c: char) -> bool {
        self.glyph_index(c) != 0
    }

    /// Rasterize `glyph` with its origin shifted right by `phase` pixels (0..1)
    /// for subpixel positioning. None for empty glyphs (spaces).
    pub fn rasterize(&self, glyph: u32, phase: f32) -> Option<RasterGlyph> {
        let face = self.face.raw;
        // SAFETY: valid face; the glyph slot is owned by it and read right after loading.
        unsafe {
            if ft::FT_Load_Glyph(face, glyph, self.load_flags) != 0 {
                return None;
            }
            let slot = (*face).glyph;
            if (*slot).format == ft::FT_GLYPH_FORMAT_OUTLINE {
                let dx = (phase.clamp(0.0, 1.0) * FIXED).round() as ft::FT_Pos;
                if dx != 0 {
                    ft::FT_Outline_Translate(&(*slot).outline, dx, 0);
                }
            }
            if (*slot).format != ft::FT_GLYPH_FORMAT_BITMAP && ft::FT_Render_Glyph(slot, ft::FT_RENDER_MODE_NORMAL) != 0 {
                return None;
            }
            let bm = &(*slot).bitmap;
            let (w, h) = (bm.width as u32, bm.rows as u32);
            if w == 0 || h == 0 || bm.buffer.is_null() {
                return None;
            }
            let pitch = bm.pitch as isize;
            let row = |y: u32| -> *const u8 {
                // Negative pitch means the rows are stored bottom-up.
                if pitch >= 0 { bm.buffer.offset(y as isize * pitch) } else { bm.buffer.offset((h as isize - 1 - y as isize) * -pitch) }
            };
            match bm.pixel_mode as u32 {
                m if m == ft::FT_PIXEL_MODE_GRAY as u32 => {
                    let mut data = Vec::with_capacity((w * h) as usize);
                    for y in 0..h {
                        data.extend_from_slice(std::slice::from_raw_parts(row(y), w as usize));
                    }
                    Some(RasterGlyph { left: (*slot).bitmap_left, top: (*slot).bitmap_top, width: w, height: h, data, color: false })
                }
                m if m == ft::FT_PIXEL_MODE_MONO as u32 => {
                    let mut data = Vec::with_capacity((w * h) as usize);
                    for y in 0..h {
                        let r = row(y);
                        for x in 0..w {
                            let byte = *r.offset((x / 8) as isize);
                            data.push(if byte & (0x80 >> (x % 8)) != 0 { 255 } else { 0 });
                        }
                    }
                    Some(RasterGlyph { left: (*slot).bitmap_left, top: (*slot).bitmap_top, width: w, height: h, data, color: false })
                }
                m if m == ft::FT_PIXEL_MODE_BGRA as u32 => {
                    // Premultiplied BGRA → premultiplied RGBA.
                    let mut src = Vec::with_capacity((w * h * 4) as usize);
                    for y in 0..h {
                        let r = std::slice::from_raw_parts(row(y), (w * 4) as usize);
                        for px in r.chunks_exact(4) {
                            src.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                        }
                    }
                    let left = (*slot).bitmap_left;
                    let top = (*slot).bitmap_top;
                    if (self.bitmap_scale - 1.0).abs() < 1e-3 {
                        return Some(RasterGlyph { left, top, width: w, height: h, data: src, color: true });
                    }
                    let s = self.bitmap_scale;
                    let (nw, nh) = (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1));
                    let data = resample_rgba(&src, w, h, nw, nh);
                    Some(RasterGlyph {
                        left: (left as f32 * s).round() as i32,
                        top: (top as f32 * s).round() as i32,
                        width: nw,
                        height: nh,
                        data,
                        color: true,
                    })
                }
                _ => None,
            }
        }
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: created by hb_ft_font_create_referenced; the face is dropped after.
        unsafe { hb::hb_font_destroy(self.hb) };
    }
}

/// Area-averaging resample of premultiplied RGBA (used to scale emoji strikes;
/// mostly downscaling, where a box filter is exact enough and alias-free).
pub fn resample_rgba(src: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    let sx = w as f32 / nw as f32;
    let sy = h as f32 / nh as f32;
    for oy in 0..nh {
        let y0 = oy as f32 * sy;
        let y1 = y0 + sy;
        for ox in 0..nw {
            let x0 = ox as f32 * sx;
            let x1 = x0 + sx;
            let mut acc = [0f32; 4];
            let mut total = 0f32;
            let mut y = y0.floor() as u32;
            while (y as f32) < y1 && y < h {
                let wy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
                let mut x = x0.floor() as u32;
                while (x as f32) < x1 && x < w {
                    let wx = ((x + 1) as f32).min(x1) - (x as f32).max(x0);
                    let wgt = wx * wy;
                    let i = ((y * w + x) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += src[i + c] as f32 * wgt;
                    }
                    total += wgt;
                    x += 1;
                }
                y += 1;
            }
            let o = ((oy * nw + ox) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / total.max(1e-6)).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_flat_color() {
        let src: Vec<u8> = std::iter::repeat([10u8, 20, 30, 255]).take(16).flatten().collect();
        let out = resample_rgba(&src, 4, 4, 3, 3);
        assert!(out.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    }
}
