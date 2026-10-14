//! Cursor images: an installed XCursor theme ($XCURSOR_THEME, Adwaita by
//! default) if it has the shape, otherwise Lumen's own vector cursor set
//! (cursor_art), so every shape is available without theme files.
//!
//! Frames are made at the exact physical size for the output scale
//! (24 logical px × scale), so cursors stay sharp at fractional scales.

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Physical, Point, Size, Transform};

use crate::cursor_art;

#[derive(Clone)]
pub struct CursorFrame {
    /// Buffer scale 1, drawn 1:1.
    pub buffer: MemoryRenderBuffer,
    /// Logical size that maps to exactly the buffer's pixel size at the
    /// requested scale.
    pub logical: Size<i32, Logical>,
    pub hotspot: Point<i32, Physical>,
    pub delay: Duration,
}

/// A cursor image for X clients: premultiplied ARGB32 in X's byte order
/// (B, G, R, A).
pub struct X11Cursor {
    pub bgra: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
}

pub struct CursorTheme {
    theme: Option<xcursor::CursorTheme>,
    /// Nominal size in logical pixels.
    size: u32,
    /// Keyed by name and scale (in 1/120 steps, as wp_fractional_scale).
    cache: HashMap<(String, u32), Vec<CursorFrame>>,
    lib: Option<std::rc::Rc<lumen_text::Library>>,
}

/// Legacy X cursor names for CSS cursor names (themes ship one or the other).
fn alternatives(name: &str) -> &'static [&'static str] {
    match name {
        "default" => &["left_ptr", "arrow"],
        "text" => &["xterm", "ibeam"],
        "pointer" => &["hand2", "hand1", "pointing_hand"],
        "grab" | "grabbing" => &["closedhand", "fleur", "hand1"],
        "move" | "all-scroll" => &["fleur"],
        "wait" => &["watch"],
        "progress" => &["left_ptr_watch", "watch"],
        "crosshair" => &["cross", "tcross"],
        "not-allowed" | "no-drop" => &["crossed_circle", "forbidden"],
        "col-resize" | "ew-resize" | "e-resize" | "w-resize" => &["sb_h_double_arrow", "h_double_arrow", "size_hor"],
        "row-resize" | "ns-resize" | "n-resize" | "s-resize" => &["sb_v_double_arrow", "v_double_arrow", "size_ver"],
        "nwse-resize" | "nw-resize" | "se-resize" => &["bd_double_arrow", "size_fdiag"],
        "nesw-resize" | "ne-resize" | "sw-resize" => &["fd_double_arrow", "size_bdiag"],
        "help" => &["question_arrow"],
        _ => &[],
    }
}

/// Bilinear resample of premultiplied RGBA.
fn resample(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    let (kx, ky) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * ky - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as u32, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * kx - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as u32, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            for c in 0..4 {
                let p = |xx: u32, yy: u32| src[((yy * sw + xx) * 4 + c) as usize] as f32;
                let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                let bot = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                out[((y * dw + x) * 4 + c) as usize] = (top * (1.0 - ty) + bot * ty).round() as u8;
            }
        }
    }
    out
}

fn scale_key(scale: f64) -> u32 {
    (scale * 120.0).round() as u32
}

/// Pixel size for `logical` at `scale`, and the logical size to give the
/// element so that it maps back to exactly that many pixels.
fn exact(logical: f64, scale: f64) -> (i32, i32) {
    let l = logical.round().max(1.0) as i32;
    ((l as f64 * scale).round() as i32, l)
}

impl CursorTheme {
    pub fn load() -> CursorTheme {
        let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "Adwaita".into());
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
        let theme = xcursor::CursorTheme::load(&name);
        let lib = lumen_text::Library::new().map_err(|e| tracing::error!("cursor rasterizer: {e}")).ok();
        CursorTheme { theme: Some(theme), size, cache: HashMap::new(), lib }
    }

    fn load_frames(&self, name: &str, scale: f64) -> Option<Vec<CursorFrame>> {
        let theme = self.theme.as_ref()?;
        let path = std::iter::once(name)
            .chain(alternatives(name).iter().copied())
            .find_map(|n| theme.load_icon(n))?;
        let data = std::fs::read(path).ok()?;
        let images = xcursor::parser::parse_xcursor(&data)?;
        let want = self.size as f64 * scale;
        // Nearest available nominal size, then all frames of that size.
        let best = images.iter().map(|i| i.size).min_by_key(|s| (*s as f64 - want).abs() as i64)?;
        let k = want / best as f64;
        let frames: Vec<CursorFrame> = images
            .iter()
            .filter(|i| i.size == best && i.width > 0 && i.height > 0)
            .map(|img| {
                // XCursor pixels are premultiplied ARGB32 in little-endian
                // byte order (B, G, R, A): DRM's Argb8888. The parser's
                // "rgba" field holds those raw bytes.
                let (w, lw) = exact(img.width as f64 * k / scale, scale);
                let (h, lh) = exact(img.height as f64 * k / scale, scale);
                let pixels = if w as u32 == img.width && h as u32 == img.height {
                    img.pixels_rgba.clone()
                } else {
                    resample(&img.pixels_rgba, img.width, img.height, w as u32, h as u32)
                };
                let (fx, fy) = (w as f64 / img.width as f64, h as f64 / img.height as f64);
                CursorFrame {
                    buffer: MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None),
                    logical: (lw, lh).into(),
                    hotspot: ((img.xhot as f64 * fx).round() as i32, (img.yhot as f64 * fy).round() as i32).into(),
                    delay: Duration::from_millis(img.delay.max(1) as u64),
                }
            })
            .collect();
        (!frames.is_empty()).then_some(frames)
    }

    /// The frame to show for `name` at `scale` and time `now`.
    pub fn frame(&mut self, name: &str, scale: f64, now: Duration) -> CursorFrame {
        let key = (name.to_string(), scale_key(scale));
        if !self.cache.contains_key(&key) {
            let frames = self.load_frames(name, scale).unwrap_or_else(|| self.drawn_frames(name, scale));
            self.cache.insert(key.clone(), frames);
        }
        let frames = &self.cache[&key];
        if frames.len() == 1 {
            return frames[0].clone();
        }
        let total: u128 = frames.iter().map(|f| f.delay.as_millis()).sum::<u128>().max(1);
        let mut t = now.as_millis() % total;
        for f in frames {
            if t < f.delay.as_millis() {
                return f.clone();
            }
            t -= f.delay.as_millis();
        }
        frames[0].clone()
    }

    /// Whether `name` animates (at any scale it was loaded for).
    pub fn is_animated(&self, name: &str) -> bool {
        self.cache.iter().any(|((n, _), f)| n == name && f.len() > 1)
    }

    /// The arrow X windows show when they set no cursor of their own.
    /// Xwayland runs at client scale 1, so it is made at the nominal size.
    pub fn x11_default(&self) -> Option<X11Cursor> {
        if let Some(theme) = self.theme.as_ref() {
            let path = std::iter::once("default").chain(alternatives("default").iter().copied()).find_map(|n| theme.load_icon(n));
            let images = path.and_then(|p| std::fs::read(p).ok()).and_then(|d| xcursor::parser::parse_xcursor(&d));
            if let Some(img) = images.as_ref().and_then(|v| v.iter().min_by_key(|i| (i.size as i64 - self.size as i64).abs())) {
                // The parser's "rgba" bytes already are X's B, G, R, A.
                return Some(X11Cursor {
                    bgra: img.pixels_rgba.clone(),
                    width: img.width as u16,
                    height: img.height as u16,
                    hot_x: img.xhot as u16,
                    hot_y: img.yhot as u16,
                });
            }
        }
        let art = cursor_art::draw(self.lib.as_ref()?, "default", self.size, 0.0);
        let mut bgra = art.rgba;
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        Some(X11Cursor {
            bgra,
            width: art.size as u16,
            height: art.size as u16,
            hot_x: art.hotspot.0.max(0) as u16,
            hot_y: art.hotspot.1.max(0) as u16,
        })
    }

    /// Frames of Lumen's vector cursor for `name`.
    fn drawn_frames(&self, name: &str, scale: f64) -> Vec<CursorFrame> {
        let (px, l) = exact(self.size as f64, scale);
        let px = px.max(4);
        let Some(lib) = self.lib.as_ref() else {
            // No FreeType: a plain square keeps the pointer visible.
            let side = (px / 4).max(2);
            let data = vec![255u8; (side * side * 4) as usize];
            return vec![CursorFrame {
                buffer: MemoryRenderBuffer::from_slice(&data, Fourcc::Abgr8888, (side, side), 1, Transform::Normal, None),
                logical: ((l / 4).max(1), (l / 4).max(1)).into(),
                hotspot: (0, 0).into(),
                delay: Duration::from_secs(1),
            }];
        };
        let n = if cursor_art::is_animated(name) { cursor_art::SPIN_FRAMES } else { 1 };
        (0..n)
            .map(|i| {
                let art = cursor_art::draw(lib, name, px as u32, i as f32 / n as f32);
                CursorFrame {
                    buffer: MemoryRenderBuffer::from_slice(
                        &art.rgba,
                        Fourcc::Abgr8888,
                        (art.size as i32, art.size as i32),
                        1,
                        Transform::Normal,
                        None,
                    ),
                    logical: (l, l).into(),
                    hotspot: art.hotspot.into(),
                    delay: Duration::from_millis(cursor_art::SPIN_FRAME_MS),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sizes_round_trip() {
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0, 2.25] {
            let (px, l) = exact(24.0, scale);
            assert_eq!(((l as f64) * scale).round() as i32, px);
        }
    }

    #[test]
    fn resample_keeps_flat_color() {
        let src: Vec<u8> = [10u8, 20, 30, 255].repeat(16);
        let out = resample(&src, 4, 4, 7, 5);
        assert!(out.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    }
}
