//! Vector paths rasterized with FreeType's anti-aliasing outline renderer —
//! used for the shell's icons and the cursor set (no image files needed).

use std::rc::Rc;

use freetype_sys as ft;

use crate::font::Library;

const TAG_ON: u8 = 1;
const TAG_CONIC: u8 = 0;
const TAG_CUBIC: u8 = 2;

/// A path in pixel coordinates (y down), made of closed contours.
#[derive(Debug, Clone, Default)]
pub struct Path {
    points: Vec<(f32, f32)>,
    tags: Vec<u8>,
    /// Index of the last point of each contour.
    ends: Vec<usize>,
    open: bool,
}

impl Path {
    pub fn new() -> Path {
        Path::default()
    }

    fn push(&mut self, x: f32, y: f32, tag: u8) {
        self.points.push((x, y));
        self.tags.push(tag);
    }

    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.close();
        self.push(x, y, TAG_ON);
        self.open = true;
        self
    }

    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        self.push(x, y, TAG_ON);
        self
    }

    pub fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) -> &mut Self {
        self.push(cx, cy, TAG_CONIC);
        self.push(x, y, TAG_ON);
        self
    }

    pub fn cubic_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) -> &mut Self {
        self.push(c1x, c1y, TAG_CUBIC);
        self.push(c2x, c2y, TAG_CUBIC);
        self.push(x, y, TAG_ON);
        self
    }

    /// End the current contour (FreeType contours are implicitly closed).
    pub fn close(&mut self) -> &mut Self {
        if self.open {
            // A trailing point equal to the start would create a zero-length edge.
            let start = self.ends.last().map(|e| e + 1).unwrap_or(0);
            if self.points.len() > start + 1 && self.points.last() == self.points.get(start) && self.tags.last() == Some(&TAG_ON) {
                self.points.pop();
                self.tags.pop();
            }
            self.ends.push(self.points.len() - 1);
            self.open = false;
        }
        self
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> &mut Self {
        self.move_to(x, y).line_to(x + w, y).line_to(x + w, y + h).line_to(x, y + h).close()
    }

    /// Rounded rectangle; `reverse` draws it counter-clockwise (for holes).
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, reverse: bool) -> &mut Self {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        // Cubic approximation of a quarter circle.
        let k = r * 0.552_284_8;
        let (x1, y1) = (x + w, y + h);
        if !reverse {
            self.move_to(x + r, y)
                .line_to(x1 - r, y)
                .cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r)
                .line_to(x1, y1 - r)
                .cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1)
                .line_to(x + r, y1)
                .cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r)
                .line_to(x, y + r)
                .cubic_to(x, y + r - k, x + r - k, y, x + r, y)
                .close()
        } else {
            self.move_to(x + r, y)
                .cubic_to(x + r - k, y, x, y + r - k, x, y + r)
                .line_to(x, y1 - r)
                .cubic_to(x, y1 - r + k, x + r - k, y1, x + r, y1)
                .line_to(x1 - r, y1)
                .cubic_to(x1 - r + k, y1, x1, y1 - r + k, x1, y1 - r)
                .line_to(x1, y + r)
                .cubic_to(x1, y + r - k, x1 - r + k, y, x1 - r, y)
                .close()
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, reverse: bool) -> &mut Self {
        self.rounded_rect(cx - r, cy - r, r * 2.0, r * 2.0, r, reverse)
    }

    /// A stroked polyline (miter-less: each segment is a quad, joints get a
    /// round cap), useful for simple line icons.
    pub fn stroke_polyline(&mut self, pts: &[(f32, f32)], width: f32) -> &mut Self {
        let hw = width / 2.0;
        for w in pts.windows(2) {
            let ((ax, ay), (bx, by)) = (w[0], w[1]);
            let (dx, dy) = (bx - ax, by - ay);
            let len = (dx * dx + dy * dy).sqrt().max(1e-6);
            let (nx, ny) = (-dy / len * hw, dx / len * hw);
            self.move_to(ax + nx, ay + ny).line_to(bx + nx, by + ny).line_to(bx - nx, by - ny).line_to(ax - nx, ay - ny).close();
        }
        for &(x, y) in pts {
            self.circle(x, y, hw, false);
        }
        self
    }

    /// The path as FreeType outline data (26.6, y flipped for a bitmap of `height` rows).
    fn ft_data(&self, height: u32) -> (Vec<ft::FT_Vector>, Vec<std::ffi::c_char>, Vec<std::ffi::c_short>) {
        let mut path = self.clone();
        path.close();
        let points = path
            .points
            .iter()
            .map(|&(x, y)| ft::FT_Vector { x: (x * 64.0).round() as ft::FT_Pos, y: ((height as f32 - y) * 64.0).round() as ft::FT_Pos })
            .collect();
        let tags = path.tags.iter().map(|&t| t as std::ffi::c_char).collect();
        let contours = path.ends.iter().map(|&e| e as std::ffi::c_short).collect();
        (points, tags, contours)
    }

    /// Coverage mask (`width` × `height`, top-down), nonzero winding unless
    /// `even_odd`.
    pub fn rasterize(&self, lib: &Rc<Library>, width: u32, height: u32, even_odd: bool) -> Vec<u8> {
        if self.points.is_empty() || width == 0 || height == 0 {
            return vec![0u8; (width * height) as usize];
        }
        let (mut points, mut tags, mut contours) = self.ft_data(height);
        let outline = ft::FT_Outline {
            n_contours: contours.len() as std::ffi::c_short,
            n_points: points.len() as std::ffi::c_short,
            points: points.as_mut_ptr(),
            tags: tags.as_mut_ptr(),
            contours: contours.as_mut_ptr(),
            flags: if even_odd { ft::FT_OUTLINE_EVEN_ODD_FILL } else { 0 },
        };
        render_outline(lib, &outline, width, height)
    }

    /// Coverage of the path's outline stroked with `width` (round joins and
    /// caps); `closed` contours are stroked all around, open ones end with caps.
    pub fn stroke(&self, lib: &Rc<Library>, width: u32, height: u32, line_width: f32, closed: bool) -> Vec<u8> {
        if self.points.is_empty() || width == 0 || height == 0 {
            return vec![0u8; (width * height) as usize];
        }
        let (mut points, mut tags, mut contours) = self.ft_data(height);
        let mut src = ft::FT_Outline {
            n_contours: contours.len() as std::ffi::c_short,
            n_points: points.len() as std::ffi::c_short,
            points: points.as_mut_ptr(),
            tags: tags.as_mut_ptr(),
            contours: contours.as_mut_ptr(),
            flags: 0,
        };
        // SAFETY: the stroker is created, used and freed here; the exported
        // outline arrays are sized from FT_Stroker_GetCounts as documented.
        unsafe {
            let mut stroker: ft::FT_Stroker = std::ptr::null_mut();
            if ft::FT_Stroker_New(lib.raw, &mut stroker) != 0 {
                return vec![0u8; (width * height) as usize];
            }
            let radius = (line_width / 2.0 * 64.0).round() as ft::FT_Fixed;
            ft::FT_Stroker_Set(stroker, radius, ft::FT_STROKER_LINECAP_ROUND, ft::FT_STROKER_LINEJOIN_ROUND, 0x40000);
            let mut out = vec![0u8; (width * height) as usize];
            if ft::FT_Stroker_ParseOutline(stroker, &mut src, if closed { 0 } else { 1 }) == 0 {
                let (mut np, mut nc) = (0u32, 0u32);
                if ft::FT_Stroker_GetCounts(stroker, &mut np, &mut nc) == 0 {
                    let mut pts = vec![ft::FT_Vector { x: 0, y: 0 }; np as usize];
                    let mut tgs = vec![0 as std::ffi::c_char; np as usize];
                    let mut cts = vec![0 as std::ffi::c_short; nc as usize];
                    let mut dst = ft::FT_Outline {
                        n_contours: 0,
                        n_points: 0,
                        points: pts.as_mut_ptr(),
                        tags: tgs.as_mut_ptr(),
                        contours: cts.as_mut_ptr(),
                        flags: 0,
                    };
                    ft::FT_Stroker_Export(stroker, &mut dst);
                    out = render_outline(lib, &dst, width, height);
                }
            }
            ft::FT_Stroker_Done(stroker);
            out
        }
    }
}

fn render_outline(lib: &Rc<Library>, outline: &ft::FT_Outline, width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width * height) as usize];
    let bitmap = ft::FT_Bitmap {
        rows: height as std::ffi::c_int,
        width: width as std::ffi::c_int,
        pitch: width as std::ffi::c_int,
        buffer: out.as_mut_ptr(),
        num_grays: 256,
        pixel_mode: ft::FT_PIXEL_MODE_GRAY as std::ffi::c_char,
        palette_mode: 0,
        palette: std::ptr::null_mut(),
    };
    // SAFETY: the outline arrays and the bitmap buffer outlive the call;
    // FreeType only reads the outline and writes inside rows × pitch.
    unsafe { ft::FT_Outline_Get_Bitmap(lib.raw, outline as *const _ as *mut _, &bitmap) };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_coverage() {
        let lib = Library::new().unwrap();
        let mut p = Path::new();
        // Covers pixels 2..6 fully; the right edge at 6.5 half-covers column 6.
        p.rect(2.0, 2.0, 4.5, 4.0);
        let m = p.rasterize(&lib, 10, 10, false);
        let at = |x: usize, y: usize| m[y * 10 + x];
        assert_eq!(at(3, 3), 255);
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(3, 1), 0, "top edge must be at y = 2 (y axis flipped correctly)");
        assert!((120..=135).contains(&at(6, 3)), "half pixel: {}", at(6, 3));
    }

    #[test]
    fn stroke_draws_only_the_outline() {
        let lib = Library::new().unwrap();
        let mut p = Path::new();
        p.rect(4.0, 4.0, 12.0, 12.0);
        let m = p.stroke(&lib, 20, 20, 2.0, true);
        assert_eq!(m[10 * 20 + 10], 0, "inside stays empty");
        assert_eq!(m[10 * 20 + 4], 255, "left edge stroked");
        let mut l = Path::new();
        l.move_to(2.0, 10.0).line_to(18.0, 10.0);
        let m = l.stroke(&lib, 20, 20, 2.0, false);
        assert_eq!(m[10 * 20 + 10], 255);
        assert_eq!(m[5 * 20 + 10], 0);
    }

    #[test]
    fn ring_with_hole() {
        let lib = Library::new().unwrap();
        let mut p = Path::new();
        p.circle(10.0, 10.0, 8.0, false).circle(10.0, 10.0, 4.0, true);
        let m = p.rasterize(&lib, 20, 20, false);
        assert_eq!(m[10 * 20 + 10], 0, "hole");
        assert_eq!(m[10 * 20 + 3], 255, "ring");
    }
}
