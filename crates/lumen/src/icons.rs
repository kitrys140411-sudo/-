//! The shell's icons, drawn from vector paths (no icon theme in the image).
//! Application icons are rounded squares ("squircles") with a glyph; apps
//! without a known icon get a tinted tile with their initial.

use std::collections::HashMap;
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

/// Draw icon `name` at `px` × `px` (designed on a 48-unit grid). `letter` is
/// used by generic app tiles.
pub fn draw(lib: &Rc<Library>, name: &str, px: u32, letter: Option<&lumen_text::Image>) -> Vec<u8> {
    let mut c = Canvas::new(lib, px, px, px as f32 / 48.0);
    let light = [0.93, 0.94, 0.97, 1.0];
    match name {
        "terminal" | "utilities-terminal" | "lumen-term" => {
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
#[derive(Default)]
pub struct IconCache {
    lib: Option<Rc<Library>>,
    map: HashMap<(String, u32), Image>,
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
        let lib = self.lib.as_ref()?;
        let key = (name.to_string(), px);
        if !self.map.contains_key(&key) {
            let known = matches!(name, "terminal" | "utilities-terminal" | "lumen-term" | "app-grid" | "power" | "search" | "close");
            let letter = if known { None } else { letter() };
            let data = draw(lib, name, px, letter.as_ref());
            let img = Image::upload(r, &data, px, px).ok()?;
            self.map.insert(key.clone(), img);
        }
        self.map.get_mut(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_have_ink() {
        let lib = Library::new().unwrap();
        for name in ["terminal", "app-grid", "power", "search", "close", "some-app"] {
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
        let icons = ["terminal", "app-grid", "power", "search", "close", "some-app"];
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
