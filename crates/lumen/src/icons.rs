//! The shell's icons, drawn from vector paths. Application icons are
//! rounded squares ("squircles") with a glyph; other apps get the PNG icon
//! they installed into the hicolor theme ([`crate::icontheme`], looked up off
//! the main loop), else a tinted tile with their initial. All art is
//! original (no third-party logos).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use lumen_text::{Library, Path};
use smithay::backend::renderer::gles::GlesRenderer;

use crate::render::elements::Image;
use crate::vector::Canvas;

/// Pleasant tile colors for generic app icons, picked by a hash of the app id.
const TILES: [([f32; 3], [f32; 3]); 6] = [
    ([0.36, 0.48, 0.92], [0.24, 0.33, 0.78]),
    ([0.60, 0.40, 0.90], [0.45, 0.27, 0.76]),
    ([0.22, 0.68, 0.62], [0.13, 0.52, 0.48]),
    ([0.93, 0.55, 0.30], [0.82, 0.38, 0.20]),
    ([0.90, 0.38, 0.52], [0.74, 0.24, 0.40]),
    ([0.38, 0.62, 0.30], [0.26, 0.48, 0.20]),
];

fn hash(s: &str) -> usize {
    s.bytes().fold(5381usize, |h, b| h.wrapping_mul(33) ^ b as usize)
}

/// Vertical gradient fill of `path` from `top` to `bottom` color.
fn gradient(c: &mut Canvas, lib: &Rc<Library>, path: &Path, top: [f32; 3], bottom: [f32; 3]) {
    let m = path.rasterize(lib, c.width, c.height, false);
    let (w, h) = (c.width as usize, c.height.max(1) as usize);
    for y in 0..h {
        let t = y as f32 / h as f32;
        let col = [top[0] + (bottom[0] - top[0]) * t, top[1] + (bottom[1] - top[1]) * t, top[2] + (bottom[2] - top[2]) * t, 1.0];
        for x in 0..w {
            c.blend_at(y * w + x, m[y * w + x], col);
        }
    }
}

fn tile(c: &mut Canvas, lib: &Rc<Library>, top: [f32; 3], bottom: [f32; 3]) {
    let s = c.scale;
    let body = c.path(|p, s| {
        p.rounded_rect(2.0 * s, 2.0 * s, 44.0 * s, 44.0 * s, 12.0 * s, false);
    });
    c.shadow(&body, 0.0, 1.2, 1.6, 0.35);
    gradient(c, lib, &body, top, bottom);
    // Hairline highlight along the top edge.
    let edge = c.path(|p, s| {
        p.rounded_rect(2.0 * s, 2.0 * s, 44.0 * s, 44.0 * s, 12.0 * s, false);
    });
    let m = edge.stroke(lib, c.width, c.height, 1.0 * s, true);
    let h = c.height as usize;
    let w = c.width as usize;
    let top_only: Vec<u8> = m.iter().enumerate().map(|(i, &v)| if (i / w) < h * 2 / 5 { v / 3 } else { 0 }).collect();
    c.blend_mask(&top_only, [1.0, 1.0, 1.0, 1.0]);
    let _ = s;
}

/// The icon we draw for an Icon= value or app id (aliases included), if any.
/// Names follow the freedesktop icon naming spec where one exists.
pub fn canonical(name: &str) -> Option<&'static str> {
    Some(match name {
        "utilities-terminal" | "terminal" | "lumen-term" | "org.gnome.Terminal" => "utilities-terminal",
        "web-browser" | "firefox" | "firefox-esr" | "org.mozilla.firefox" => "web-browser",
        "youtube" => "youtube",
        "claude-code" => "claude-code",
        "system-file-manager" | "lumen-files" | "folder" => "system-file-manager",
        "text-editor" | "accessories-text-editor" | "lumen-edit" => "text-editor",
        "preferences-system" | "lumen-settings" => "preferences-system",
        "accessories-calculator" | "lumen-calc" => "accessories-calculator",
        "utilities-system-monitor" | "lumen-monitor" => "utilities-system-monitor",
        "image-viewer" | "lumen-view" => "image-viewer",
        "multimedia-video-player" | "mpv" | "io.mpv.Mpv" => "multimedia-video-player",
        "input-keyboard" => "input-keyboard",
        "applications-games" | "steam" | "com.valvesoftware.Steam" | "lumen-steam" => "applications-games",
        _ => return None,
    })
}

/// Glyph-only icons of the shell itself (no app tile).
const SHELL_ICONS: [&str; 10] = [
    "app-grid", "power", "search", "close", "audio-volume-high", "audio-volume-medium", "audio-volume-low", "audio-volume-muted",
    "display-brightness", "battery-charging",
];

/// Every icon drawn from vector art (not a letter tile).
pub fn is_known(name: &str) -> bool {
    canonical(name).is_some() || SHELL_ICONS.contains(&name)
}

/// An ellipse as four cubic Béziers.
fn ellipse(p: &mut Path, cx: f32, cy: f32, rx: f32, ry: f32) {
    const K: f32 = 0.552_284_8;
    let (kx, ky) = (rx * K, ry * K);
    p.move_to(cx, cy - ry)
        .cubic_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy)
        .cubic_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry)
        .cubic_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy)
        .cubic_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry)
        .close();
}

/// Points of an arc (degrees, 0 = right, clockwise on screen) for strokes.
fn arc(cx: f32, cy: f32, r: f32, from: f32, to: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn polyline(p: &mut Path, pts: &[(f32, f32)], s: f32) {
    for (i, &(x, y)) in pts.iter().enumerate() {
        if i == 0 {
            p.move_to(x * s, y * s);
        } else {
            p.line_to(x * s, y * s);
        }
    }
}

/// A four-pointed sparkle centered at (cx, cy) with radius r.
fn sparkle(p: &mut Path, cx: f32, cy: f32, r: f32, s: f32) {
    let w = r * 0.16;
    p.move_to(cx * s, (cy - r) * s)
        .quad_to((cx + w) * s, (cy - w) * s, (cx + r) * s, cy * s)
        .quad_to((cx + w) * s, (cy + w) * s, cx * s, (cy + r) * s)
        .quad_to((cx - w) * s, (cy + w) * s, (cx - r) * s, cy * s)
        .quad_to((cx - w) * s, (cy - w) * s, cx * s, (cy - r) * s)
        .close();
}

/// The speaker of the volume icons.
fn speaker(c: &mut Canvas, color: [f32; 4]) {
    let body = c.path(|p, s| {
        polyline(p, &[(7.0, 19.5), (13.0, 19.5), (21.0, 12.0), (21.0, 36.0), (13.0, 28.5), (7.0, 28.5)], s);
        p.close();
    });
    c.fill(&body, color);
    c.stroke(&body, 1.6, true, color);
}

/// Draw icon `name` at `px` × `px` (designed on a 48-unit grid). `letter` is
/// used by generic app tiles.
pub fn draw(lib: &Rc<Library>, name: &str, px: u32, letter: Option<&lumen_text::Image>) -> Vec<u8> {
    let mut c = Canvas::new(lib, px, px, px as f32 / 48.0);
    let light = [0.93, 0.94, 0.97, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
    match canonical(name).unwrap_or(name) {
        "utilities-terminal" => {
            tile(&mut c, lib, [0.20, 0.22, 0.29], [0.11, 0.12, 0.16]);
            let prompt = c.path(|p, s| {
                p.move_to(13.0 * s, 17.0 * s).line_to(20.0 * s, 23.5 * s).line_to(13.0 * s, 30.0 * s);
            });
            c.stroke(&prompt, 3.2, false, [0.55, 0.85, 0.60, 1.0]);
            let cursor = c.path(|p, s| {
                p.rounded_rect(23.0 * s, 28.4 * s, 12.0 * s, 3.2 * s, 1.6 * s, false);
            });
            c.fill(&cursor, light);
        }
        "web-browser" => {
            // A globe: rim, a meridian, the equator and two parallels.
            tile(&mut c, lib, [0.24, 0.70, 0.97], [0.12, 0.42, 0.88]);
            let (cx, cy, r) = (24.0, 24.0, 13.0);
            let rim = c.path(|p, s| {
                p.circle(cx * s, cy * s, r * s, false);
            });
            c.stroke(&rim, 2.6, true, white);
            let meridian = c.path(|p, s| ellipse(p, cx * s, cy * s, 5.6 * s, r * s));
            c.stroke(&meridian, 2.2, true, white);
            let lines = c.path(|p, s| {
                for dy in [-6.5f32, 0.0, 6.5] {
                    let half = (r * r - dy * dy).sqrt() - 0.6;
                    p.move_to((cx - half) * s, (cy + dy) * s).line_to((cx + half) * s, (cy + dy) * s);
                }
            });
            c.stroke(&lines, 2.2, false, white);
        }
        "youtube" => {
            // A red tile with a white play triangle (rounded by stroking it too).
            tile(&mut c, lib, [1.0, 0.33, 0.31], [0.80, 0.10, 0.13]);
            let play = c.path(|p, s| {
                polyline(p, &[(19.5, 15.5), (33.5, 24.0), (19.5, 32.5)], s);
                p.close();
            });
            c.fill(&play, white);
            c.stroke(&play, 3.0, true, white);
        }
        "claude-code" => {
            // A warm terminal tile: prompt, cursor and a sparkle.
            tile(&mut c, lib, [0.25, 0.21, 0.19], [0.13, 0.11, 0.10]);
            let prompt = c.path(|p, s| {
                polyline(p, &[(11.5, 19.0), (18.0, 25.0), (11.5, 31.0)], s);
            });
            c.stroke(&prompt, 3.2, false, [0.98, 0.92, 0.84, 1.0]);
            let cursor = c.path(|p, s| {
                p.rounded_rect(21.5 * s, 29.4 * s, 10.0 * s, 3.2 * s, 1.6 * s, false);
            });
            c.fill(&cursor, [0.98, 0.92, 0.84, 1.0]);
            let spark = c.path(|p, s| sparkle(p, 33.5, 15.0, 8.0, s));
            c.fill(&spark, [1.0, 0.70, 0.36, 1.0]);
            let small = c.path(|p, s| sparkle(p, 24.5, 11.5, 3.2, s));
            c.fill(&small, [1.0, 0.82, 0.55, 1.0]);
        }
        "system-file-manager" => {
            tile(&mut c, lib, [0.50, 0.50, 0.99], [0.32, 0.30, 0.84]);
            let back = c.path(|p, s| {
                p.rounded_rect(10.5 * s, 13.0 * s, 12.0 * s, 8.0 * s, 2.5 * s, false);
                p.rounded_rect(10.5 * s, 15.5 * s, 27.0 * s, 18.0 * s, 3.0 * s, false);
            });
            c.fill(&back, [1.0, 1.0, 1.0, 0.62]);
            let front = c.path(|p, s| {
                p.rounded_rect(10.5 * s, 19.5 * s, 27.0 * s, 16.0 * s, 3.0 * s, false);
            });
            c.shadow(&front, 0.0, -0.6, 1.2, 0.18);
            c.fill(&front, white);
        }
        "text-editor" => {
            tile(&mut c, lib, [1.0, 0.68, 0.30], [0.93, 0.44, 0.12]);
            let page = c.path(|p, s| {
                p.rounded_rect(13.5 * s, 9.5 * s, 21.0 * s, 29.0 * s, 3.0 * s, false);
            });
            c.shadow(&page, 0.0, 0.8, 1.2, 0.2);
            c.fill(&page, white);
            let lines = c.path(|p, s| {
                for (y, w) in [(16.0, 13.0), (21.0, 13.0), (26.0, 9.0), (31.0, 11.0)] {
                    p.rounded_rect(17.5 * s, (y - 1.1) * s, w * s, 2.2 * s, 1.1 * s, false);
                }
            });
            c.fill(&lines, [0.95, 0.55, 0.20, 1.0]);
        }
        "preferences-system" => {
            // A gear: eight teeth around a ring.
            tile(&mut c, lib, [0.56, 0.60, 0.68], [0.35, 0.39, 0.47]);
            let gear = c.path(|p, s| {
                let (cx, cy, outer, inner) = (24.0f32, 24.0f32, 14.0f32, 10.6f32);
                let mut pts = Vec::new();
                for i in 0..8 {
                    let a = i as f32 * 45.0;
                    for (da, r) in [(-16.0, inner), (-9.0, outer), (9.0, outer), (16.0, inner), (22.5, inner)] {
                        let t = (a + da).to_radians();
                        pts.push((cx + r * t.cos(), cy + r * t.sin()));
                    }
                }
                polyline(p, &pts, s);
                p.close();
                p.circle(cx * s, cy * s, 4.6 * s, false);
            });
            c.stroke(&gear, 1.2, true, white);
            c.fill_even_odd(&gear, white);
        }
        "accessories-calculator" => {
            tile(&mut c, lib, [0.36, 0.82, 0.50], [0.15, 0.60, 0.31]);
            let body = c.path(|p, s| {
                p.rounded_rect(13.0 * s, 8.5 * s, 22.0 * s, 31.0 * s, 4.0 * s, false);
            });
            c.shadow(&body, 0.0, 0.8, 1.2, 0.2);
            c.fill(&body, white);
            let display = c.path(|p, s| {
                p.rounded_rect(16.0 * s, 11.5 * s, 16.0 * s, 7.0 * s, 1.6 * s, false);
            });
            c.fill(&display, [0.13, 0.42, 0.24, 1.0]);
            let keys = c.path(|p, s| {
                for row in 0..3 {
                    for col in 0..3 {
                        p.rounded_rect((16.0 + col as f32 * 6.0) * s, (21.5 + row as f32 * 5.6) * s, 4.0 * s, 3.8 * s, 1.1 * s, false);
                    }
                }
            });
            c.fill(&keys, [0.22, 0.66, 0.38, 1.0]);
        }
        "utilities-system-monitor" => {
            tile(&mut c, lib, [0.69, 0.49, 1.0], [0.46, 0.28, 0.86]);
            let grid = c.path(|p, s| {
                for y in [16.0f32, 24.0, 32.0] {
                    p.move_to(10.5 * s, y * s).line_to(37.5 * s, y * s);
                }
            });
            c.stroke(&grid, 1.0, false, [1.0, 1.0, 1.0, 0.25]);
            let line = c.path(|p, s| {
                polyline(p, &[(10.0, 29.0), (16.0, 29.0), (20.0, 17.0), (25.5, 34.0), (29.5, 22.5), (32.5, 27.0), (38.0, 27.0)], s);
            });
            c.stroke(&line, 3.0, false, white);
        }
        "image-viewer" => {
            tile(&mut c, lib, [0.24, 0.82, 0.78], [0.07, 0.56, 0.60]);
            let frame = c.path(|p, s| {
                p.rounded_rect(9.5 * s, 12.5 * s, 29.0 * s, 23.0 * s, 3.5 * s, false);
            });
            c.shadow(&frame, 0.0, 0.8, 1.2, 0.2);
            c.fill(&frame, white);
            let hills = c.path(|p, s| {
                polyline(p, &[(12.5, 32.5), (20.0, 21.5), (26.0, 29.5), (29.5, 25.0), (35.5, 32.5)], s);
                p.close();
            });
            c.fill(&hills, [0.08, 0.58, 0.60, 1.0]);
            let sun = c.path(|p, s| {
                p.circle(30.5 * s, 18.5 * s, 2.8 * s, false);
            });
            c.fill(&sun, [1.0, 0.74, 0.30, 1.0]);
        }
        "multimedia-video-player" => {
            // A disc with a play triangle cut out (distinct from YouTube's).
            tile(&mut c, lib, [1.0, 0.44, 0.66], [0.84, 0.18, 0.47]);
            let disc = c.path(|p, s| {
                p.circle(24.0 * s, 24.0 * s, 13.5 * s, false);
                polyline(p, &[(20.5, 17.5), (31.0, 24.0), (20.5, 30.5)], s);
                p.close();
            });
            c.fill_even_odd(&disc, white);
        }
        "input-keyboard" => {
            tile(&mut c, lib, [0.49, 0.55, 0.68], [0.30, 0.35, 0.47]);
            let body = c.path(|p, s| {
                p.rounded_rect(7.5 * s, 14.5 * s, 33.0 * s, 20.0 * s, 3.5 * s, false);
                for row in 0..2 {
                    for col in 0..5 {
                        p.rounded_rect((11.4 + col as f32 * 5.4) * s, (17.8 + row as f32 * 5.0) * s, 3.6 * s, 3.4 * s, 0.9 * s, false);
                    }
                }
                p.rounded_rect(16.5 * s, 27.8 * s, 15.0 * s, 3.4 * s, 0.9 * s, false);
            });
            c.fill_even_odd(&body, white);
        }
        "applications-games" => {
            // A game controller: D-pad, two buttons, two grips.
            tile(&mut c, lib, [0.22, 0.29, 0.50], [0.09, 0.12, 0.25]);
            let body = c.path(|p, s| {
                p.rounded_rect(8.0 * s, 15.5 * s, 32.0 * s, 15.0 * s, 7.5 * s, false);
                p.circle(14.5 * s, 29.0 * s, 6.0 * s, false);
                p.circle(33.5 * s, 29.0 * s, 6.0 * s, false);
            });
            c.shadow(&body, 0.0, 0.8, 1.2, 0.25);
            c.fill(&body, white);
            let dpad = c.path(|p, s| {
                p.rounded_rect(11.0 * s, 21.7 * s, 9.0 * s, 2.6 * s, 0.8 * s, false);
                p.rounded_rect(14.2 * s, 18.5 * s, 2.6 * s, 9.0 * s, 0.8 * s, false);
            });
            c.fill(&dpad, [0.13, 0.17, 0.32, 1.0]);
            let a = c.path(|p, s| {
                p.circle(34.0 * s, 25.5 * s, 2.0 * s, false);
            });
            c.fill(&a, [0.36, 0.78, 0.98, 1.0]);
            let b = c.path(|p, s| {
                p.circle(30.0 * s, 21.0 * s, 2.0 * s, false);
            });
            c.fill(&b, [1.0, 0.45, 0.55, 1.0]);
        }
        "display-brightness" => {
            // A sun: a disc and eight rays.
            let disc = c.path(|p, s| {
                p.circle(24.0 * s, 24.0 * s, 6.5 * s, false);
            });
            c.fill(&disc, light);
            let rays = c.path(|p, s| {
                for i in 0..8 {
                    let a = (i as f32 * 45.0).to_radians();
                    let (dx, dy) = (a.cos(), a.sin());
                    p.move_to((24.0 + dx * 11.0) * s, (24.0 + dy * 11.0) * s).line_to((24.0 + dx * 15.5) * s, (24.0 + dy * 15.5) * s);
                }
            });
            c.stroke(&rays, 3.0, false, light);
        }
        "battery-charging" => {
            let bolt = c.path(|p, s| {
                polyline(p, &[(27.0, 6.0), (13.0, 27.0), (23.0, 27.0), (20.0, 42.0), (35.0, 20.0), (25.0, 20.0)], s);
                p.close();
            });
            c.fill(&bolt, light);
            c.stroke(&bolt, 1.2, true, light);
        }
        "app-grid" => {
            tile(&mut c, lib, [0.27, 0.28, 0.34], [0.19, 0.20, 0.25]);
            let dots = c.path(|p, s| {
                for i in 0..3 {
                    for j in 0..3 {
                        p.circle((15.0 + 9.0 * i as f32) * s, (15.0 + 9.0 * j as f32) * s, 2.6 * s, false);
                    }
                }
            });
            c.fill(&dots, light);
        }
        "power" => {
            let arc = c.path(|p, s| {
                let (cx, cy, r) = (24.0, 26.0, 13.0);
                let steps = 32;
                for i in 0..=steps {
                    // From -60° to 240° (open at the top).
                    let a = (-60.0f32 + 300.0 * i as f32 / steps as f32).to_radians() - std::f32::consts::FRAC_PI_2;
                    let (x, y) = ((cx + r * a.cos()) * s, (cy + r * a.sin()) * s);
                    if i == 0 {
                        p.move_to(x, y);
                    } else {
                        p.line_to(x, y);
                    }
                }
                p.move_to(24.0 * s, 8.0 * s).line_to(24.0 * s, 24.0 * s);
            });
            c.stroke(&arc, 4.0, false, light);
        }
        "search" => {
            let glass = c.path(|p, s| {
                p.circle(21.0 * s, 21.0 * s, 11.0 * s, false).circle(21.0 * s, 21.0 * s, 7.6 * s, true);
            });
            c.fill(&glass, light);
            let handle = c.path(|p, s| {
                p.move_to(29.5 * s, 29.5 * s).line_to(39.0 * s, 39.0 * s);
            });
            c.stroke(&handle, 4.4, false, light);
        }
        "close" => {
            let x = c.path(|p, s| {
                p.move_to(15.0 * s, 15.0 * s).line_to(33.0 * s, 33.0 * s).move_to(33.0 * s, 15.0 * s).line_to(15.0 * s, 33.0 * s);
            });
            c.stroke(&x, 4.0, false, light);
        }
        n @ ("audio-volume-high" | "audio-volume-medium" | "audio-volume-low" | "audio-volume-muted") => {
            speaker(&mut c, light);
            if n == "audio-volume-muted" {
                let x = c.path(|p, s| {
                    p.move_to(28.0 * s, 19.0 * s).line_to(38.0 * s, 29.0 * s).move_to(38.0 * s, 19.0 * s).line_to(28.0 * s, 29.0 * s);
                });
                c.stroke(&x, 3.4, false, light);
            } else {
                let waves = match n {
                    "audio-volume-high" => 3,
                    "audio-volume-medium" => 2,
                    _ => 1,
                };
                for (i, r) in [7.0f32, 12.5, 18.0].into_iter().enumerate() {
                    let on = i < waves;
                    let w = c.path(|p, s| polyline(p, &arc(21.0, 24.0, r, -42.0, 42.0, 16), s));
                    c.stroke(&w, 3.2, false, if on { light } else { [light[0], light[1], light[2], 0.28] });
                }
            }
        }
        _ => {
            let (top, bottom) = TILES[hash(name) % TILES.len()];
            tile(&mut c, lib, top, bottom);
            if let Some(img) = letter {
                // Center the letter's layout box in the tile.
                let ox = (px as i32 - img.layout_size.0.round() as i32) / 2 - img.origin.0;
                let oy = (px as i32 - img.layout_size.1.round() as i32) / 2 - img.origin.1;
                let mut mask = vec![0u8; (px * px) as usize];
                for y in 0..img.height as i32 {
                    for x in 0..img.width as i32 {
                        let (tx, ty) = (x + ox, y + oy);
                        if tx >= 0 && ty >= 0 && tx < px as i32 && ty < px as i32 {
                            mask[(ty as u32 * px + tx as u32) as usize] = img.data[((y as u32 * img.width + x as u32) * 4 + 3) as usize];
                        }
                    }
                }
                c.blend_mask(&mask, light);
            }
        }
    }
    c.data
}

/// Icons are drawn once per (name, pixel size) and kept as textures.
///
/// Names without vector art are first drawn as letter tiles and looked up in
/// the installed icon themes in the background: [`IconCache::take_requests`]
/// hands the lookups to the caller, [`IconCache::arrived`] takes the result,
/// and the next [`IconCache::get`] replaces the tile with the image.
#[derive(Default)]
pub struct IconCache {
    lib: Option<Rc<Library>>,
    map: HashMap<(String, u32), Image>,
    /// Letter tiles standing in while a themed image is looked up.
    provisional: HashSet<(String, u32)>,
    /// Lookups to start (name, pixel size).
    requests: Vec<(String, u32)>,
    /// Lookup results (premultiplied RGBA, None: not found) not yet uploaded.
    results: HashMap<(String, u32), Option<Vec<u8>>>,
}

/// Names that are worth looking for in icon themes (not our own letter keys).
fn themable(name: &str) -> bool {
    !name.is_empty() && !name.starts_with("letter-") && !is_known(name)
}

impl IconCache {
    pub fn get(
        &mut self,
        r: &mut GlesRenderer,
        name: &str,
        px: u32,
        letter: impl FnOnce() -> Option<lumen_text::Image>,
    ) -> Option<&mut Image> {
        if self.lib.is_none() {
            self.lib = Library::new().ok();
        }
        let key = (name.to_string(), px);
        if let Some(found) = self.results.remove(&key) {
            self.provisional.remove(&key);
            if let Some(data) = found {
                match Image::upload(r, &data, px, px) {
                    Ok(img) => {
                        self.map.insert(key.clone(), img);
                    }
                    Err(e) => tracing::warn!("icon {name}: {e:?}"),
                }
            }
        }
        if !self.map.contains_key(&key) {
            let lib = self.lib.as_ref()?;
            let letter = if is_known(name) { None } else { letter() };
            let data = draw(lib, name, px, letter.as_ref());
            let img = Image::upload(r, &data, px, px).ok()?;
            self.map.insert(key.clone(), img);
            if themable(name) {
                self.provisional.insert(key.clone());
                self.requests.push(key.clone());
            }
        }
        self.map.get_mut(&key)
    }

    /// Themed lookups to run (off the main loop), each once.
    pub fn take_requests(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.requests)
    }

    /// The result of a themed lookup.
    pub fn arrived(&mut self, name: String, px: u32, data: Option<Vec<u8>>) {
        if self.provisional.contains(&(name.clone(), px)) {
            self.results.insert((name, px), data);
        }
    }

    /// Forget every themed or letter icon (apps were installed or removed:
    /// their icons are looked up again on next use).
    pub fn forget_themed(&mut self) {
        self.map.retain(|(n, _), _| !themable(n));
        self.provisional.clear();
        self.results.clear();
        self.requests.clear();
    }

    /// How many icons still wait for their themed image (tests, IPC).
    pub fn pending(&self) -> usize {
        self.provisional.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the shared desktop-entry contract names.
    const APP_ICONS: [&str; 13] = [
        "utilities-terminal", "web-browser", "youtube", "claude-code", "system-file-manager", "text-editor", "preferences-system",
        "accessories-calculator", "utilities-system-monitor", "image-viewer", "multimedia-video-player", "input-keyboard",
        "applications-games",
    ];

    #[test]
    fn aliases_map_to_drawn_icons() {
        assert_eq!(canonical("firefox"), Some("web-browser"));
        assert_eq!(canonical("mpv"), Some("multimedia-video-player"));
        assert_eq!(canonical("lumen-term"), Some("utilities-terminal"));
        assert_eq!(canonical("lumen-files"), Some("system-file-manager"));
        assert_eq!(canonical("steam"), Some("applications-games"));
        assert_eq!(canonical("com.valvesoftware.Steam"), Some("applications-games"));
        assert_eq!(canonical("nope"), None);
        assert!(themable("org.gimp.GIMP") && !themable("steam") && !themable("letter-a") && !themable("power"));
        for n in APP_ICONS {
            assert_eq!(canonical(n), Some(n));
        }
    }

    #[test]
    fn app_icons_are_distinct_tiles() {
        let lib = Library::new().unwrap();
        let drawn: Vec<Vec<u8>> = APP_ICONS.iter().map(|n| draw(&lib, n, 48, None)).collect();
        let generic = draw(&lib, "some-app", 48, None);
        for (i, d) in drawn.iter().enumerate() {
            assert_ne!(*d, generic, "{} falls back to a letter tile", APP_ICONS[i]);
            for (j, e) in drawn.iter().enumerate().skip(i + 1) {
                assert_ne!(d, e, "{} looks like {}", APP_ICONS[i], APP_ICONS[j]);
            }
        }
    }

    #[test]
    fn icons_have_ink() {
        let lib = Library::new().unwrap();
        let names = [
            "terminal", "app-grid", "power", "search", "close", "some-app", "audio-volume-high", "audio-volume-muted", "display-brightness",
            "battery-charging",
        ];
        for name in names.into_iter().chain(APP_ICONS) {
            let d = draw(&lib, name, 48, None);
            let ink = d.chunks_exact(4).filter(|p| p[3] > 128).count();
            assert!(ink > 100, "{name}: {ink}");
            assert!(d.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "{name} premultiplied");
        }
    }
}

/// `LUMEN_ART_DUMP=dir cargo test art_sheet` writes a contact sheet of every
/// cursor and icon on light and dark backgrounds (visual review).
#[cfg(test)]
mod sheet {
    #[test]
    fn art_sheet() {
        let Ok(dir) = std::env::var("LUMEN_ART_DUMP") else { return };
        let lib = lumen_text::Library::new().unwrap();
        let cursors = ["default", "pointer", "grab", "grabbing", "text", "crosshair", "move", "ew-resize", "ns-resize", "nwse-resize",
            "nesw-resize", "not-allowed", "wait", "progress"];
        let icons = [
            "utilities-terminal", "web-browser", "youtube", "claude-code", "system-file-manager", "text-editor", "preferences-system",
            "accessories-calculator", "utilities-system-monitor", "image-viewer", "multimedia-video-player", "input-keyboard",
            "applications-games", "app-grid", "power", "search", "close", "audio-volume-high", "audio-volume-medium", "audio-volume-low",
            "audio-volume-muted", "display-brightness", "battery-charging", "some-app",
        ];
        let cell = 72u32;
        let (w, h) = (cell * cursors.len().max(icons.len()) as u32, cell * 4);
        let mut img = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let bg: [u8; 4] = if (y / cell) % 2 == 0 { [0x1c, 0x1d, 0x24, 255] } else { [0xe8, 0xe9, 0xee, 255] };
                img[((y * w + x) * 4) as usize..][..4].copy_from_slice(&bg);
            }
        }
        let mut put = |data: &[u8], size: u32, cx: u32, cy: u32| {
            for y in 0..size {
                for x in 0..size {
                    let s = ((y * size + x) * 4) as usize;
                    let d = (((cy + y) * w + cx + x) * 4) as usize;
                    let a = data[s + 3] as f32 / 255.0;
                    for c in 0..3 {
                        img[d + c] = (data[s + c] as f32 + img[d + c] as f32 * (1.0 - a)).round() as u8;
                    }
                }
            }
        };
        for (i, n) in cursors.iter().enumerate() {
            let a = crate::cursor_art::draw(&lib, n, 48, 0.1);
            put(&a.rgba, 48, i as u32 * cell + 12, 12);
            put(&a.rgba, 48, i as u32 * cell + 12, cell + 12);
        }
        for (i, n) in icons.iter().enumerate() {
            let d = super::draw(&lib, n, 56, None);
            put(&d, 56, i as u32 * cell + 8, 2 * cell + 8);
            put(&d, 56, i as u32 * cell + 8, 3 * cell + 8);
        }
        crate::png::write_file(&std::path::Path::new(&dir).join("art.png"), w, h, &img).unwrap();
    }
}
