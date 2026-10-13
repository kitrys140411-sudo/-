//! Box drawing, block elements, braille and powerline symbols drawn
//! procedurally at the exact cell size, so lines meet seamlessly between
//! cells whatever the font and line height.

use std::rc::Rc;

use lumen_text::{Library, Path};

/// Whether `c` is drawn by this module.
pub fn handles(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F | 0x2800..=0x28FF | 0xE0B0..=0xE0B7)
}

/// Line weights per side (left, right, up, down): 0 none, 1 light, 2 heavy,
/// 3 double.
fn lines(c: char) -> Option<[u8; 4]> {
    let v = match c as u32 {
        0x2500 => [1, 1, 0, 0],
        0x2501 => [2, 2, 0, 0],
        0x2502 => [0, 0, 1, 1],
        0x2503 => [0, 0, 2, 2],
        0x250C => [0, 1, 0, 1],
        0x250D => [0, 2, 0, 1],
        0x250E => [0, 1, 0, 2],
        0x250F => [0, 2, 0, 2],
        0x2510 => [1, 0, 0, 1],
        0x2511 => [2, 0, 0, 1],
        0x2512 => [1, 0, 0, 2],
        0x2513 => [2, 0, 0, 2],
        0x2514 => [0, 1, 1, 0],
        0x2515 => [0, 2, 1, 0],
        0x2516 => [0, 1, 2, 0],
        0x2517 => [0, 2, 2, 0],
        0x2518 => [1, 0, 1, 0],
        0x2519 => [2, 0, 1, 0],
        0x251A => [1, 0, 2, 0],
        0x251B => [2, 0, 2, 0],
        0x251C => [0, 1, 1, 1],
        0x251D => [0, 2, 1, 1],
        0x251E => [0, 1, 2, 1],
        0x251F => [0, 1, 1, 2],
        0x2520 => [0, 1, 2, 2],
        0x2521 => [0, 2, 2, 1],
        0x2522 => [0, 2, 1, 2],
        0x2523 => [0, 2, 2, 2],
        0x2524 => [1, 0, 1, 1],
        0x2525 => [2, 0, 1, 1],
        0x2526 => [1, 0, 2, 1],
        0x2527 => [1, 0, 1, 2],
        0x2528 => [1, 0, 2, 2],
        0x2529 => [2, 0, 2, 1],
        0x252A => [2, 0, 1, 2],
        0x252B => [2, 0, 2, 2],
        0x252C => [1, 1, 0, 1],
        0x252D => [2, 1, 0, 1],
        0x252E => [1, 2, 0, 1],
        0x252F => [2, 2, 0, 1],
        0x2530 => [1, 1, 0, 2],
        0x2531 => [2, 1, 0, 2],
        0x2532 => [1, 2, 0, 2],
        0x2533 => [2, 2, 0, 2],
        0x2534 => [1, 1, 1, 0],
        0x2535 => [2, 1, 1, 0],
        0x2536 => [1, 2, 1, 0],
        0x2537 => [2, 2, 1, 0],
        0x2538 => [1, 1, 2, 0],
        0x2539 => [2, 1, 2, 0],
        0x253A => [1, 2, 2, 0],
        0x253B => [2, 2, 2, 0],
        0x253C => [1, 1, 1, 1],
        0x253D => [2, 1, 1, 1],
        0x253E => [1, 2, 1, 1],
        0x253F => [2, 2, 1, 1],
        0x2540 => [1, 1, 2, 1],
        0x2541 => [1, 1, 1, 2],
        0x2542 => [1, 1, 2, 2],
        0x2543 => [2, 1, 2, 1],
        0x2544 => [1, 2, 2, 1],
        0x2545 => [2, 1, 1, 2],
        0x2546 => [1, 2, 1, 2],
        0x2547 => [2, 2, 2, 1],
        0x2548 => [2, 2, 1, 2],
        0x2549 => [2, 1, 2, 2],
        0x254A => [1, 2, 2, 2],
        0x254B => [2, 2, 2, 2],
        0x2550 => [3, 3, 0, 0],
        0x2551 => [0, 0, 3, 3],
        0x2552 => [0, 3, 0, 1],
        0x2553 => [0, 1, 0, 3],
        0x2554 => [0, 3, 0, 3],
        0x2555 => [3, 0, 0, 1],
        0x2556 => [1, 0, 0, 3],
        0x2557 => [3, 0, 0, 3],
        0x2558 => [0, 3, 1, 0],
        0x2559 => [0, 1, 3, 0],
        0x255A => [0, 3, 3, 0],
        0x255B => [3, 0, 1, 0],
        0x255C => [1, 0, 3, 0],
        0x255D => [3, 0, 3, 0],
        0x255E => [0, 3, 1, 1],
        0x255F => [0, 1, 3, 3],
        0x2560 => [0, 3, 3, 3],
        0x2561 => [3, 0, 1, 1],
        0x2562 => [1, 0, 3, 3],
        0x2563 => [3, 0, 3, 3],
        0x2564 => [3, 3, 0, 1],
        0x2565 => [1, 1, 0, 3],
        0x2566 => [3, 3, 0, 3],
        0x2567 => [3, 3, 1, 0],
        0x2568 => [1, 1, 3, 0],
        0x2569 => [3, 3, 3, 0],
        0x256A => [3, 3, 1, 1],
        0x256B => [1, 1, 3, 3],
        0x256C => [3, 3, 3, 3],
        0x2574 => [1, 0, 0, 0],
        0x2575 => [0, 0, 1, 0],
        0x2576 => [0, 1, 0, 0],
        0x2577 => [0, 0, 0, 1],
        0x2578 => [2, 0, 0, 0],
        0x2579 => [0, 0, 2, 0],
        0x257A => [0, 2, 0, 0],
        0x257B => [0, 0, 0, 2],
        0x257C => [1, 2, 0, 0],
        0x257D => [0, 0, 1, 2],
        0x257E => [2, 1, 0, 0],
        0x257F => [0, 0, 2, 1],
        _ => return None,
    };
    Some(v)
}

struct Canvas {
    w: i32,
    h: i32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Canvas {
        Canvas { w: w as i32, h: h as i32, data: vec![0; (w * h) as usize] }
    }

    /// Fill `[x0, x1) × [y0, y1)` with coverage `v` (clipped).
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, v: u8) {
        for y in y0.max(0)..y1.min(self.h) {
            for x in x0.max(0)..x1.min(self.w) {
                let i = (y * self.w + x) as usize;
                self.data[i] = self.data[i].max(v);
            }
        }
    }

    fn mask(&mut self, m: &[u8]) {
        for (d, s) in self.data.iter_mut().zip(m) {
            *d = (*d).max(*s);
        }
    }
}

/// Coverage bitmap (`w`×`h`) of `c`, with light lines `t` pixels thick.
pub fn render(lib: &Rc<Library>, c: char, w: u32, h: u32, t: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 {
        return None;
    }
    let mut cv = Canvas::new(w, h);
    let (wi, hi) = (w as i32, h as i32);
    let t = t.max(1) as i32;
    let heavy = (t * 2).max(t + 1);
    let cx = wi / 2;
    let cy = hi / 2;
    let u = c as u32;
    if let Some([l, r, up, dn]) = lines(c) {
        let thick = |wt: u8| if wt == 2 { heavy } else { t };
        // Single and heavy lines: from the edge to the center, overlapping
        // the widest perpendicular line so joints are square.
        let hw = thick(up.max(dn).min(2)).max(if up == 3 || dn == 3 { t } else { 0 });
        let vw = thick(l.max(r).min(2)).max(if l == 3 || r == 3 { t } else { 0 });
        let hx = |wt: u8| {
            let s = thick(wt);
            (cy - s / 2, cy - s / 2 + s)
        };
        let vx = |wt: u8| {
            let s = thick(wt);
            (cx - s / 2, cx - s / 2 + s)
        };
        if l == 1 || l == 2 {
            let (y0, y1) = hx(l);
            cv.rect(0, y0, cx + hw / 2 + hw % 2, y1, 255);
        }
        if r == 1 || r == 2 {
            let (y0, y1) = hx(r);
            cv.rect(cx - hw / 2, y0, wi, y1, 255);
        }
        if up == 1 || up == 2 {
            let (x0, x1) = vx(up);
            cv.rect(x0, 0, x1, cy + vw / 2 + vw % 2, 255);
        }
        if dn == 1 || dn == 2 {
            let (x0, x1) = vx(dn);
            cv.rect(x0, cy - vw / 2, x1, hi, 255);
        }
        // Double lines: two light lines `g` from the center, with the
        // junction rules that keep their insides open.
        let g = (t + 1).max(wi / 6).min(wi / 3).max(t + 1);
        let (has_l, has_r, has_u, has_d) = (l == 3, r == 3, up == 3, dn == 3);
        let (top, bot) = (cy - g, cy + g);
        let (lft, rgt) = (cx - g, cx + g);
        let hline = |cv: &mut Canvas, y: i32, x0: i32, x1: i32| cv.rect(x0, y - t / 2, x1, y - t / 2 + t, 255);
        let vline = |cv: &mut Canvas, x: i32, y0: i32, y1: i32| cv.rect(x - t / 2, y0, x - t / 2 + t, y1, 255);
        let any_v = up != 0 || dn != 0;
        let any_h = l != 0 || r != 0;
        if has_r {
            let ts = if has_u { rgt } else if l != 0 { cx } else if any_v { lft } else { cx };
            let bs = if has_d { rgt } else if l != 0 { cx } else if any_v { lft } else { cx };
            hline(&mut cv, top, ts, wi);
            hline(&mut cv, bot, bs, wi);
        }
        if has_l {
            let te = if has_u { lft + 1 } else if r != 0 { cx } else if any_v { rgt + 1 } else { cx };
            let be = if has_d { lft + 1 } else if r != 0 { cx } else if any_v { rgt + 1 } else { cx };
            hline(&mut cv, top, 0, te);
            hline(&mut cv, bot, 0, be);
        }
        if has_u {
            let le = if has_l { top + 1 } else if dn != 0 { cy } else if any_h { bot + 1 } else { cy };
            let re = if has_r { top + 1 } else if dn != 0 { cy } else if any_h { bot + 1 } else { cy };
            vline(&mut cv, lft, 0, le);
            vline(&mut cv, rgt, 0, re);
        }
        if has_d {
            let ls = if has_l { bot } else if up != 0 { cy } else if any_h { top } else { cy };
            let rs = if has_r { bot } else if up != 0 { cy } else if any_h { top } else { cy };
            vline(&mut cv, lft, ls, hi);
            vline(&mut cv, rgt, rs, hi);
        }
        return Some(cv.data);
    }
    match u {
        // Dashed lines.
        0x2504..=0x250B | 0x254C..=0x254F => {
            let (n, heavy_line, vertical) = match u {
                0x2504 => (3, false, false),
                0x2505 => (3, true, false),
                0x2506 => (3, false, true),
                0x2507 => (3, true, true),
                0x2508 => (4, false, false),
                0x2509 => (4, true, false),
                0x250A => (4, false, true),
                0x250B => (4, true, true),
                0x254C => (2, false, false),
                0x254D => (2, true, false),
                0x254E => (2, false, true),
                _ => (2, true, true),
            };
            let s = if heavy_line { heavy } else { t };
            let len = if vertical { hi } else { wi };
            for i in 0..n {
                let a = len * i / n;
                let b = a + (len / n) * 2 / 3;
                if vertical {
                    cv.rect(cx - s / 2, a, cx - s / 2 + s, b.max(a + 1), 255);
                } else {
                    cv.rect(a, cy - s / 2, b.max(a + 1), cy - s / 2 + s, 255);
                }
            }
        }
        // Rounded corners.
        0x256D..=0x2570 => {
            let r = (wi.min(hi) as f32 / 2.0).max(1.0);
            let (fx, fy) = (cx as f32 + 0.5 * (t % 2) as f32, cy as f32 + 0.5 * (t % 2) as f32);
            let mut p = Path::new();
            match u {
                0x256D => {
                    p.move_to(fx, hi as f32).line_to(fx, fy + r).quad_to(fx, fy, fx + r, fy).line_to(wi as f32, fy);
                }
                0x256E => {
                    p.move_to(fx, hi as f32).line_to(fx, fy + r).quad_to(fx, fy, fx - r, fy).line_to(0.0, fy);
                }
                0x256F => {
                    p.move_to(fx, 0.0).line_to(fx, fy - r).quad_to(fx, fy, fx - r, fy).line_to(0.0, fy);
                }
                _ => {
                    p.move_to(fx, 0.0).line_to(fx, fy - r).quad_to(fx, fy, fx + r, fy).line_to(wi as f32, fy);
                }
            }
            let m = p.stroke(lib, w, h, t as f32, false);
            cv.mask(&m);
        }
        // Diagonals.
        0x2571..=0x2573 => {
            let mut p = Path::new();
            if u == 0x2571 || u == 0x2573 {
                p.move_to(wi as f32, 0.0).line_to(0.0, hi as f32);
            }
            if u == 0x2572 || u == 0x2573 {
                p.move_to(0.0, 0.0).line_to(wi as f32, hi as f32);
            }
            let m = p.stroke(lib, w, h, t as f32, false);
            cv.mask(&m);
        }
        // Block elements.
        0x2580 => cv.rect(0, 0, wi, hi / 2, 255),
        0x2581..=0x2588 => {
            let eighths = (u - 0x2580) as i32;
            cv.rect(0, hi - hi * eighths / 8, wi, hi, 255);
        }
        0x2589..=0x258F => {
            let eighths = (0x2590 - u) as i32;
            cv.rect(0, 0, wi * eighths / 8, hi, 255);
        }
        0x2590 => cv.rect(wi / 2, 0, wi, hi, 255),
        0x2591..=0x2593 => {
            let v = [64u8, 128, 192][(u - 0x2591) as usize];
            cv.rect(0, 0, wi, hi, v);
        }
        0x2594 => cv.rect(0, 0, wi, (hi / 8).max(1), 255),
        0x2595 => cv.rect(wi - (wi / 8).max(1), 0, wi, hi, 255),
        0x2596..=0x259F => {
            // Quadrants: upper-left, upper-right, lower-left, lower-right.
            let q: [bool; 4] = match u {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let (mx, my) = (wi / 2, hi / 2);
            if q[0] {
                cv.rect(0, 0, mx, my, 255);
            }
            if q[1] {
                cv.rect(mx, 0, wi, my, 255);
            }
            if q[2] {
                cv.rect(0, my, mx, hi, 255);
            }
            if q[3] {
                cv.rect(mx, my, wi, hi, 255);
            }
        }
        // Braille: 2×4 dots.
        0x2800..=0x28FF => {
            let bits = u - 0x2800;
            let order = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
            let dot = ((wi / 4).min(hi / 8)).max(1) as f32 * 0.9;
            let mut p = Path::new();
            for (i, (col, row)) in order.iter().enumerate() {
                if bits & (1 << i) != 0 {
                    let x = wi as f32 * (0.25 + 0.5 * *col as f32);
                    let y = hi as f32 * (0.125 + 0.25 * *row as f32);
                    p.circle(x, y, dot, false);
                }
            }
            let m = p.rasterize(lib, w, h, false);
            cv.mask(&m);
        }
        // Powerline separators.
        0xE0B0..=0xE0B7 => {
            let (wf, hf) = (wi as f32, hi as f32);
            let mut p = Path::new();
            match u {
                0xE0B0 => {
                    p.move_to(0.0, 0.0).line_to(wf, hf / 2.0).line_to(0.0, hf).close();
                }
                0xE0B2 => {
                    p.move_to(wf, 0.0).line_to(0.0, hf / 2.0).line_to(wf, hf).close();
                }
                0xE0B4 => {
                    p.move_to(0.0, 0.0).cubic_to(wf * 1.33, 0.0, wf * 1.33, hf, 0.0, hf).close();
                }
                0xE0B6 => {
                    p.move_to(wf, 0.0).cubic_to(-wf * 0.33, 0.0, -wf * 0.33, hf, wf, hf).close();
                }
                _ => {}
            }
            let m = if matches!(u, 0xE0B1 | 0xE0B3 | 0xE0B5 | 0xE0B7) {
                let mut q = Path::new();
                match u {
                    0xE0B1 => {
                        q.move_to(0.0, 0.0).line_to(wf, hf / 2.0).line_to(0.0, hf);
                    }
                    0xE0B3 => {
                        q.move_to(wf, 0.0).line_to(0.0, hf / 2.0).line_to(wf, hf);
                    }
                    0xE0B5 => {
                        q.move_to(0.0, 0.0).cubic_to(wf * 1.33, 0.0, wf * 1.33, hf, 0.0, hf);
                    }
                    _ => {
                        q.move_to(wf, 0.0).cubic_to(-wf * 0.33, 0.0, -wf * 0.33, hf, wf, hf);
                    }
                }
                q.stroke(lib, w, h, t as f32, false)
            } else {
                p.rasterize(lib, w, h, false)
            };
            cv.mask(&m);
        }
        _ => return None,
    }
    Some(cv.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(d: &[u8], w: u32, x: u32, y: u32) -> u8 {
        d[(y * w + x) as usize]
    }

    #[test]
    fn lines_reach_the_edges() {
        let lib = Library::new().unwrap();
        let (w, h) = (10, 20);
        let d = render(&lib, '┼', w, h, 1).unwrap();
        assert_eq!(at(&d, w, 0, 10), 255);
        assert_eq!(at(&d, w, 9, 10), 255);
        assert_eq!(at(&d, w, 5, 0), 255);
        assert_eq!(at(&d, w, 5, 19), 255);
        assert_eq!(at(&d, w, 1, 1), 0);
        let d = render(&lib, '─', w, h, 1).unwrap();
        assert_eq!(at(&d, w, 5, 0), 0);
        // Double corner: open inside.
        let d = render(&lib, '╔', w, h, 1).unwrap();
        // Two horizontal lines reach the right edge, one above the other.
        let right: Vec<u32> = (0..h).filter(|&y| at(&d, w, 9, y) == 255).collect();
        assert!(right.len() >= 2 && right[0] < 10 && *right.last().unwrap() > right[0] + 1, "{right:?}");
        // Nothing at the top-left corner (inside of the corner is open).
        assert_eq!(at(&d, w, 0, 0), 0);
    }

    #[test]
    fn blocks_and_braille() {
        let lib = Library::new().unwrap();
        let d = render(&lib, '▀', 8, 16, 1).unwrap();
        assert_eq!((at(&d, 8, 4, 2), at(&d, 8, 4, 12)), (255, 0));
        let d = render(&lib, '█', 8, 16, 1).unwrap();
        assert!(d.iter().all(|&v| v == 255));
        let d = render(&lib, '⣿', 8, 16, 1).unwrap();
        assert!(d.iter().filter(|&&v| v > 128).count() > 8);
        assert!(render(&lib, 'a', 8, 16, 1).is_none());
    }
}
