//! Cursor images: XCursor themes (Adwaita by default) with a built-in fallback
//! arrow, so the pointer is always visible even without a theme installed.

use std::collections::HashMap;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Transform};

#[derive(Clone)]
pub struct CursorFrame {
    pub buffer: MemoryRenderBuffer,
    pub hotspot: Point<i32, Logical>,
    pub delay: Duration,
}

pub struct CursorTheme {
    theme: Option<xcursor::CursorTheme>,
    size: u32,
    cache: HashMap<(String, i32), Vec<CursorFrame>>,
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

impl CursorTheme {
    pub fn load() -> CursorTheme {
        let name = std::env::var("XCURSOR_THEME").unwrap_or_else(|_| "Adwaita".into());
        let size = std::env::var("XCURSOR_SIZE").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
        let theme = xcursor::CursorTheme::load(&name);
        CursorTheme { theme: Some(theme), size, cache: HashMap::new() }
    }

    fn load_frames(&self, name: &str, scale: i32) -> Option<Vec<CursorFrame>> {
        let theme = self.theme.as_ref()?;
        let path = std::iter::once(name)
            .chain(alternatives(name).iter().copied())
            .find_map(|n| theme.load_icon(n))?;
        let data = std::fs::read(path).ok()?;
        let images = xcursor::parser::parse_xcursor(&data)?;
        let want = self.size * scale as u32;
        // Nearest available nominal size, then all frames of that size.
        let best = images.iter().map(|i| i.size).min_by_key(|s| (*s as i64 - want as i64).abs())?;
        let frames: Vec<CursorFrame> = images
            .iter()
            .filter(|i| i.size == best)
            .map(|img| {
                // XCursor pixels are ARGB8888 little-endian, i.e. byte order B,G,R,A.
                let buffer = MemoryRenderBuffer::from_slice(
                    &img.pixels_argb,
                    Fourcc::Argb8888,
                    (img.width as i32, img.height as i32),
                    scale,
                    Transform::Normal,
                    None,
                );
                CursorFrame {
                    buffer,
                    hotspot: ((img.xhot as i32) / scale, (img.yhot as i32) / scale).into(),
                    delay: Duration::from_millis(img.delay.max(1) as u64),
                }
            })
            .collect();
        (!frames.is_empty()).then_some(frames)
    }

    /// The frame to show for `name` at `scale` and time `now`.
    pub fn frame(&mut self, name: &str, scale: i32, now: Duration) -> CursorFrame {
        let key = (name.to_string(), scale);
        if !self.cache.contains_key(&key) {
            let frames = self
                .load_frames(name, scale)
                .or_else(|| self.load_frames("default", scale))
                .unwrap_or_else(|| vec![fallback_arrow(scale)]);
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

    pub fn is_animated(&self, name: &str, scale: i32) -> bool {
        self.cache.get(&(name.to_string(), scale)).is_some_and(|f| f.len() > 1)
    }
}

/// A clean anti-aliased arrow drawn from a polygon (used when no theme exists).
fn fallback_arrow(scale: i32) -> CursorFrame {
    let s = scale.max(1) as f32;
    let size = (24.0 * s) as usize;
    // Arrow outline in 24x24 design units.
    let poly: [(f32, f32); 7] = [(2.0, 1.5), (2.0, 19.0), (6.5, 14.8), (9.6, 21.5), (12.6, 20.2), (9.6, 13.6), (15.8, 13.6)];
    let inside = |px: f32, py: f32, shrink: f32| -> bool {
        // Even-odd test against the polygon, optionally shrunk toward its centroid.
        let (cx, cy) = (6.0, 12.0);
        let pts: Vec<(f32, f32)> = poly
            .iter()
            .map(|&(x, y)| (cx + (x - cx) * (1.0 - shrink), cy + (y - cy) * (1.0 - shrink)))
            .collect();
        let mut c = false;
        let n = pts.len();
        for i in 0..n {
            let (x1, y1) = pts[i];
            let (x2, y2) = pts[(i + 1) % n];
            if (y1 > py) != (y2 > py) && px < (x2 - x1) * (py - y1) / (y2 - y1) + x1 {
                c = !c;
            }
        }
        c
    };
    let mut argb = vec![0u8; size * size * 4];
    const SS: usize = 4;
    for y in 0..size {
        for x in 0..size {
            let (mut outer, mut inner) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (x as f32 + (sx as f32 + 0.5) / SS as f32) / s;
                    let py = (y as f32 + (sy as f32 + 0.5) / SS as f32) / s;
                    if inside(px, py, 0.0) {
                        outer += 1;
                    }
                    if inside(px, py, 0.16) {
                        inner += 1;
                    }
                }
            }
            let a = outer as f32 / (SS * SS) as f32;
            let w = inner as f32 / (SS * SS) as f32;
            // White fill with a dark outline; premultiplied ARGB, little-endian.
            let v = (w * 255.0) as u8;
            let i = (y * size + x) * 4;
            argb[i] = v;
            argb[i + 1] = v;
            argb[i + 2] = v;
            argb[i + 3] = (a * 255.0) as u8;
        }
    }
    CursorFrame {
        buffer: MemoryRenderBuffer::from_slice(&argb, Fourcc::Argb8888, (size as i32, size as i32), scale, Transform::Normal, None),
        hotspot: (2, 2).into(),
        delay: Duration::from_secs(1),
    }
}
