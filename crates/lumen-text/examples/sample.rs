//! Render sample text on dark and light backgrounds into a PPM image
//! (visual check of hinting, subpixel positioning and gamma).
//!
//!   cargo run -p lumen-text --example sample -- out.ppm

use lumen_text::{TextStyle, TextSystem, Weight};

fn blend(dst: &mut [u8], w: u32, img: &lumen_text::Image, x: i32, y: i32) {
    for iy in 0..img.height as i32 {
        for ix in 0..img.width as i32 {
            let (tx, ty) = (x + ix, y + iy);
            if tx < 0 || ty < 0 || tx >= w as i32 || (ty * w as i32 + tx) as usize * 3 >= dst.len() {
                continue;
            }
            let s = ((iy as u32 * img.width + ix as u32) * 4) as usize;
            let d = ((ty as u32 * w + tx as u32) * 3) as usize;
            let a = img.data[s + 3] as f32 / 255.0;
            for c in 0..3 {
                dst[d + c] = (img.data[s + c] as f32 + dst[d + c] as f32 * (1.0 - a)).round().min(255.0) as u8;
            }
        }
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "sample.ppm".into());
    let mut t = TextSystem::new("Inter").expect("fonts");
    let (w, h) = (900u32, 420u32);
    let mut px = vec![0u8; (w * h * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let v: [u8; 3] = if x < w / 2 { [0x16, 0x16, 0x1c] } else { [0xf4, 0xf4, 0xf6] };
            px[((y * w + x) * 3) as usize..][..3].copy_from_slice(&v);
        }
    }
    let samples = [
        (11.0, Weight::Regular, "Activities  12:45  Thu 11 Oct"),
        (13.0, Weight::Regular, "The quick brown fox jumps over the lazy dog"),
        (14.0, Weight::Medium, "Settings · Terminal · Files — 0123456789"),
        (16.0, Weight::SemiBold, "Notification title: Screenshot saved"),
        (20.0, Weight::Regular, "Lumen desktop — Привет, мир! ✓ 😀"),
        (28.0, Weight::Bold, "Overview  Search…"),
    ];
    let mut y = 16;
    for (size, weight, text) in samples {
        let light = TextStyle::new(size, [0.93, 0.93, 0.95, 1.0]).weight(weight);
        let dark = TextStyle::new(size, [0.10, 0.10, 0.12, 1.0]).weight(weight);
        let a = t.render(&light, text, Some(w as f32 / 2.0 - 24.0), Some(1));
        let b = t.render(&dark, text, Some(w as f32 / 2.0 - 24.0), Some(1));
        blend(&mut px, w, &a, 12 - a.origin.0, y - a.origin.1);
        blend(&mut px, w, &b, (w / 2) as i32 + 12 - b.origin.0, y - b.origin.1);
        y += (size * 1.9) as i32;
    }
    let mut f = format!("P6 {w} {h} 255\n").into_bytes();
    f.extend_from_slice(&px);
    std::fs::write(&out, f).expect("write");
    println!("wrote {out}");
}
