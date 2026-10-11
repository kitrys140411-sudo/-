//! Lumen's own cursor set, drawn from vector paths at any size. Used when no
//! XCursor theme is installed (the default image ships none).
//!
//! Shapes are designed on a 24-unit grid: white fill, dark outline (a dilation
//! of the shape, so unions of parts get one clean contour) and a soft shadow.

use std::f32::consts::PI;
use std::rc::Rc;

use lumen_text::{Library, Path};

use crate::vector::Canvas;

pub struct Art {
    /// Premultiplied RGBA, `size` × `size`.
    pub rgba: Vec<u8>,
    pub size: u32,
    /// In pixels of this image.
    pub hotspot: (i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Arrow,
    Hand,
    OpenHand,
    Fist,
    Text,
    Crosshair,
    Move,
    Resize(f32),
    NotAllowed,
    Wait,
    Progress,
}

fn shape(name: &str) -> Shape {
    match name {
        "pointer" | "hand1" | "hand2" | "pointing_hand" => Shape::Hand,
        "grab" | "openhand" => Shape::OpenHand,
        "grabbing" | "closedhand" | "dnd-move" => Shape::Fist,
        "text" | "xterm" | "ibeam" | "vertical-text" => Shape::Text,
        "crosshair" | "cell" | "tcross" => Shape::Crosshair,
        "move" | "all-scroll" | "fleur" => Shape::Move,
        "ew-resize" | "e-resize" | "w-resize" | "col-resize" | "sb_h_double_arrow" => Shape::Resize(0.0),
        "ns-resize" | "n-resize" | "s-resize" | "row-resize" | "sb_v_double_arrow" => Shape::Resize(90.0),
        "nwse-resize" | "nw-resize" | "se-resize" => Shape::Resize(45.0),
        "nesw-resize" | "ne-resize" | "sw-resize" => Shape::Resize(135.0),
        "not-allowed" | "no-drop" | "crossed_circle" => Shape::NotAllowed,
        "wait" | "watch" => Shape::Wait,
        "progress" | "left_ptr_watch" => Shape::Progress,
        _ => Shape::Arrow,
    }
}

pub fn is_animated(name: &str) -> bool {
    matches!(shape(name), Shape::Wait | Shape::Progress)
}

/// Frames of animated cursors and their duration.
pub const SPIN_FRAMES: u32 = 16;
pub const SPIN_FRAME_MS: u64 = 50;

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const INK: [f32; 4] = [0.08, 0.08, 0.1, 1.0];

/// Path builder in design units with an optional rotation about (12, 12).
struct Pen<'a> {
    p: &'a mut Path,
    s: f32,
    rot: f32,
    dx: f32,
    dy: f32,
}

impl Pen<'_> {
    fn t(&self, x: f32, y: f32) -> (f32, f32) {
        let (c, sn) = (self.rot.cos(), self.rot.sin());
        let (x, y) = (x - 12.0, y - 12.0);
        ((x * c - y * sn + 12.0 + self.dx) * self.s, (x * sn + y * c + 12.0 + self.dy) * self.s)
    }

    fn poly(&mut self, pts: &[(f32, f32)]) {
        let (x, y) = self.t(pts[0].0, pts[0].1);
        self.p.move_to(x, y);
        for &(px, py) in &pts[1..] {
            let (x, y) = self.t(px, py);
            self.p.line_to(x, y);
        }
        self.p.close();
    }

    /// Axis-aligned rounded rectangle (only valid without rotation).
    fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let (px, py) = self.t(x, y);
        self.p.rounded_rect(px, py, w * self.s, h * self.s, r * self.s, false);
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, reverse: bool) {
        let (x, y) = self.t(cx, cy);
        self.p.circle(x, y, r * self.s, reverse);
    }
}

fn arrow(pen: &mut Pen) {
    pen.poly(&[(4.0, 3.0), (4.0, 19.5), (7.9, 15.9), (10.6, 21.9), (13.0, 20.8), (10.4, 15.0), (15.5, 15.0)]);
}

fn double_arrow(pen: &mut Pen) {
    pen.poly(&[
        (2.5, 12.0),
        (7.5, 7.0),
        (7.5, 10.6),
        (16.5, 10.6),
        (16.5, 7.0),
        (21.5, 12.0),
        (16.5, 17.0),
        (16.5, 13.4),
        (7.5, 13.4),
        (7.5, 17.0),
    ]);
}

fn hand(pen: &mut Pen) {
    pen.rrect(9.0, 2.5, 3.2, 11.0, 1.6); // index finger
    pen.rrect(12.0, 8.5, 2.9, 5.5, 1.4);
    pen.rrect(14.7, 9.3, 2.8, 5.0, 1.4);
    pen.rrect(17.3, 10.3, 2.6, 4.6, 1.3);
    pen.rrect(4.6, 12.6, 5.0, 3.0, 1.5); // thumb
    pen.rrect(7.4, 10.5, 12.5, 10.5, 3.6); // palm
}

fn open_hand(pen: &mut Pen) {
    pen.rrect(7.4, 4.0, 2.7, 9.0, 1.35);
    pen.rrect(10.2, 2.8, 2.7, 10.0, 1.35);
    pen.rrect(13.0, 3.6, 2.7, 9.2, 1.35);
    pen.rrect(15.8, 5.4, 2.5, 8.0, 1.25);
    pen.rrect(3.8, 11.0, 5.4, 3.0, 1.5);
    pen.rrect(6.6, 9.8, 12.2, 11.0, 3.6);
}

fn fist(pen: &mut Pen) {
    for (i, x) in [8.6f32, 11.3, 14.0, 16.6].iter().enumerate() {
        pen.circle(*x, 9.6 + if i == 0 { 0.6 } else { 0.0 }, 1.6, false);
    }
    pen.rrect(4.8, 11.6, 4.6, 3.0, 1.5);
    pen.rrect(7.0, 9.0, 11.6, 11.0, 3.6);
}

fn text_beam(pen: &mut Pen) {
    pen.rrect(11.2, 5.0, 1.6, 14.0, 0.0);
    pen.rrect(8.4, 3.2, 7.2, 1.8, 0.9);
    pen.rrect(8.4, 19.0, 7.2, 1.8, 0.9);
}

fn crosshair(pen: &mut Pen) {
    pen.rrect(3.0, 11.2, 7.0, 1.6, 0.0);
    pen.rrect(14.0, 11.2, 7.0, 1.6, 0.0);
    pen.rrect(11.2, 3.0, 1.6, 7.0, 0.0);
    pen.rrect(11.2, 14.0, 1.6, 7.0, 0.0);
    pen.circle(12.0, 12.0, 0.9, false);
}

fn move_arrows(pen: &mut Pen) {
    // Plus with four arrow heads.
    pen.poly(&[(12.0, 2.0), (16.0, 6.0), (13.2, 6.0), (13.2, 10.8), (18.0, 10.8), (18.0, 8.0), (22.0, 12.0), (18.0, 16.0),
        (18.0, 13.2), (13.2, 13.2), (13.2, 18.0), (16.0, 18.0), (12.0, 22.0), (8.0, 18.0), (10.8, 18.0), (10.8, 13.2),
        (6.0, 13.2), (6.0, 16.0), (2.0, 12.0), (6.0, 8.0), (6.0, 10.8), (10.8, 10.8), (10.8, 6.0), (8.0, 6.0)]);
}

fn not_allowed(pen: &mut Pen) {
    pen.circle(12.0, 12.0, 8.5, false);
    pen.circle(12.0, 12.0, 6.2, true);
    let mut slash = Pen { p: pen.p, s: pen.s, rot: PI / 4.0, dx: 0.0, dy: 0.0 };
    slash.poly(&[(5.0, 10.9), (19.0, 10.9), (19.0, 13.1), (5.0, 13.1)]);
}

/// Spinner arc (270°) starting at `phase` (0..1 of a turn), around (cx, cy).
fn spinner(s: f32, cx: f32, cy: f32, r: f32, phase: f32) -> Path {
    let mut p = Path::new();
    let start = phase * 2.0 * PI;
    let steps = 24;
    for i in 0..=steps {
        let a = start + i as f32 / steps as f32 * 1.5 * PI;
        let (x, y) = ((cx + r * a.cos()) * s, (cy + r * a.sin()) * s);
        if i == 0 {
            p.move_to(x, y);
        } else {
            p.line_to(x, y);
        }
    }
    p
}

/// Dilate a coverage mask by `r` pixels (max filter, round kernel).
fn dilate(mask: &[u8], w: u32, h: u32, r: f32) -> Vec<u8> {
    let ri = r.ceil() as i32;
    let (w, h) = (w as i32, h as i32);
    let mut out = vec![0u8; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let mut m = 0u8;
            for ky in -ri..=ri {
                for kx in -ri..=ri {
                    let d = ((kx * kx + ky * ky) as f32).sqrt();
                    if d > r + 0.5 {
                        continue;
                    }
                    let (sx, sy) = (x + kx, y + ky);
                    if sx < 0 || sy < 0 || sx >= w || sy >= h {
                        continue;
                    }
                    // Partial weight at the kernel rim keeps the outline smooth.
                    let wgt = (r + 0.5 - d).clamp(0.0, 1.0);
                    let v = (mask[(sy * w + sx) as usize] as f32 * wgt) as u8;
                    m = m.max(v);
                }
            }
            out[(y * w + x) as usize] = m;
        }
    }
    out
}

/// Draw cursor `name` as a `size` × `size` image. `phase` animates spinners.
pub fn draw(lib: &Rc<Library>, name: &str, size: u32, phase: f32) -> Art {
    let size = size.max(8);
    let s = size as f32 / 24.0;
    let sh = shape(name);
    let mut c = Canvas::new(lib, size, size, s);
    let body = c.path(|p, s| {
        let mut pen = Pen { p, s, rot: 0.0, dx: 0.0, dy: 0.0 };
        match sh {
            Shape::Arrow | Shape::Progress => arrow(&mut pen),
            Shape::Hand => hand(&mut pen),
            Shape::OpenHand => open_hand(&mut pen),
            Shape::Fist => fist(&mut pen),
            Shape::Text => text_beam(&mut pen),
            Shape::Crosshair => crosshair(&mut pen),
            Shape::Move => move_arrows(&mut pen),
            Shape::Resize(deg) => {
                pen.rot = deg.to_radians();
                double_arrow(&mut pen);
            }
            Shape::NotAllowed => not_allowed(&mut pen),
            Shape::Wait => {}
        }
    });
    let outline_px = (1.1 * s).max(1.0);
    let mask = body.rasterize(lib, size, size, false);
    if sh != Shape::Wait {
        c.shadow(&body, 0.5, 0.9, 1.2, 0.35);
        c.blend_mask(&dilate(&mask, size, size, outline_px), INK);
        c.blend_mask(&mask, WHITE);
    }
    if matches!(sh, Shape::Wait | Shape::Progress) {
        let (cx, cy, r) = if sh == Shape::Wait { (12.0, 12.0, 7.0) } else { (17.5, 17.5, 4.0) };
        let arc = spinner(s, cx, cy, r, phase);
        let w = if sh == Shape::Wait { 2.6 } else { 1.8 };
        let outer = arc.stroke(lib, size, size, (w + 1.8) * s, false);
        c.blend_mask(&outer, INK);
        let inner = arc.stroke(lib, size, size, w * s, false);
        c.blend_mask(&inner, WHITE);
    }
    let hot = match sh {
        Shape::Arrow | Shape::Progress => (4.0, 3.0),
        Shape::Hand => (10.6, 2.8),
        _ => (12.0, 12.0),
    };
    Art { rgba: c.data, size, hotspot: ((hot.0 * s).round() as i32, (hot.1 * s).round() as i32) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_has_ink_and_hotspot_inside() {
        let lib = Library::new().unwrap();
        for name in ["default", "pointer", "grab", "grabbing", "text", "crosshair", "move", "ew-resize", "ns-resize",
            "nwse-resize", "nesw-resize", "not-allowed", "wait", "progress"] {
            let art = draw(&lib, name, 24, 0.0);
            let ink = art.rgba.chunks_exact(4).filter(|p| p[3] > 128).count();
            assert!(ink > 20, "{name}: {ink}");
            assert!(art.hotspot.0 >= 0 && art.hotspot.0 < 24 && art.hotspot.1 >= 0 && art.hotspot.1 < 24, "{name}");
            // Premultiplied.
            assert!(art.rgba.chunks_exact(4).all(|p| p[0] <= p[3]), "{name}");
        }
        // The arrow's tip is at the hotspot: the pixel there is drawn.
        let a = draw(&lib, "default", 48, 0.0);
        let (x, y) = (a.hotspot.0 as usize + 1, a.hotspot.1 as usize + 2);
        assert!(a.rgba[(y * 48 + x) * 4 + 3] > 128);
        // Spinner frames differ.
        assert_ne!(draw(&lib, "wait", 24, 0.0).rgba, draw(&lib, "wait", 24, 0.25).rgba);
    }
}
