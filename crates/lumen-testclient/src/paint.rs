//! Software rendering into ARGB8888 shm buffers.
//!
//! The window is a solid fill with a 1-px (scaled) lighter border and a darker 24-px
//! (scaled) "title bar" strip at the top, so screenshots can check placement and corner
//! clipping. With `--animate` a light vertical bar moves across the body every frame.

pub type Rgb = (u8, u8, u8);

/// Width of the animated bar in logical pixels.
pub const BAR_WIDTH: u32 = 16;
/// Logical pixels the animated bar advances per frame.
pub const BAR_STEP: u32 = 4;
const TITLE_HEIGHT: f64 = 24.0;

pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let m = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

pub fn lighter(c: Rgb) -> Rgb {
    mix(c, (255, 255, 255), 0.35)
}

pub fn darker(c: Rgb) -> Rgb {
    mix(c, (0, 0, 0), 0.35)
}

/// A rectangle in buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Vertical layout of a window buffer, in buffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    pub border: u32,
    /// First row below the title bar.
    pub body_top: u32,
}

impl Layout {
    /// `scale` is the number of buffer pixels per logical pixel.
    pub fn new(width: u32, height: u32, scale: f64, title_bar: bool) -> Layout {
        let border = (scale.round() as u32).max(1).min(width / 2).min(height / 2);
        let title = if title_bar {
            (TITLE_HEIGHT * scale).round() as u32
        } else {
            0
        };
        let body_top = (border + title).min(height.saturating_sub(border));
        Layout {
            width,
            height,
            border,
            body_top,
        }
    }

    /// The animated bar for frame `step`, or None if the body is too small to hold it.
    pub fn bar(&self, step: u64, scale: f64) -> Option<Rect> {
        let inner_w = self.width.saturating_sub(2 * self.border);
        let body_h = self
            .height
            .saturating_sub(self.border)
            .saturating_sub(self.body_top);
        let bar_w = ((f64::from(BAR_WIDTH) * scale).round() as u32).max(1);
        if inner_w <= bar_w || body_h == 0 {
            return None;
        }
        let travel = u64::from(inner_w - bar_w);
        let offset = (step * u64::from(((f64::from(BAR_STEP) * scale).round() as u32).max(1)))
            % (travel + 1);
        Some(Rect {
            x: self.border + offset as u32,
            y: self.body_top,
            w: bar_w,
            h: body_h,
        })
    }
}

/// Fills `canvas` (rows of `stride` bytes) according to `layout`.
pub fn paint(
    canvas: &mut [u8],
    stride: usize,
    layout: &Layout,
    fill: Rgb,
    title: Option<Rgb>,
    bar: Option<(Rect, Rgb)>,
) {
    let width = layout.width as usize;
    let border_color = lighter(fill);
    let pixel = |c: Rgb| [c.2, c.1, c.0, 0xff];
    let fill_px = |row: &mut [u8], from: usize, to: usize, c: Rgb| {
        let p = pixel(c);
        for px in row[from * 4..to * 4].chunks_exact_mut(4) {
            px.copy_from_slice(&p);
        }
    };

    let b = layout.border as usize;
    let inner = (b.min(width), width.saturating_sub(b).max(b.min(width)));
    let mut border_row = vec![0u8; width * 4];
    fill_px(&mut border_row, 0, width, border_color);
    let mut title_row = border_row.clone();
    fill_px(&mut title_row, inner.0, inner.1, title.unwrap_or(fill));
    let mut body_row = border_row.clone();
    fill_px(&mut body_row, inner.0, inner.1, fill);
    let mut bar_row = body_row.clone();
    if let Some((rect, color)) = bar {
        let x0 = (rect.x as usize).clamp(inner.0, inner.1);
        let x1 = ((rect.x + rect.w) as usize).clamp(inner.0, inner.1);
        fill_px(&mut bar_row, x0, x1, color);
    }

    let bottom = layout.height.saturating_sub(layout.border);
    for y in 0..layout.height {
        let src = if y < layout.border || y >= bottom {
            &border_row
        } else if y < layout.body_top {
            &title_row
        } else if bar.is_some_and(|(r, _)| y >= r.y && y < r.y + r.h) {
            &bar_row
        } else {
            &body_row
        };
        let start = y as usize * stride;
        canvas[start..start + width * 4].copy_from_slice(src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(canvas: &[u8], stride: usize, x: usize, y: usize) -> Rgb {
        let i = y * stride + x * 4;
        assert_eq!(canvas[i + 3], 0xff);
        (canvas[i + 2], canvas[i + 1], canvas[i])
    }

    #[test]
    fn colors() {
        assert_eq!(lighter((0, 0, 0)), (89, 89, 89));
        assert_eq!(darker((200, 100, 0)), (130, 65, 0));
        assert_eq!(mix((10, 20, 30), (10, 20, 30), 0.5), (10, 20, 30));
    }

    #[test]
    fn window_regions() {
        let (w, h, stride) = (100usize, 80usize, 400usize);
        let mut canvas = vec![0u8; stride * h];
        let fill = (0x3b, 0x42, 0x61);
        let layout = Layout::new(w as u32, h as u32, 1.0, true);
        assert_eq!(
            layout,
            Layout {
                width: 100,
                height: 80,
                border: 1,
                body_top: 25
            }
        );
        paint(&mut canvas, stride, &layout, fill, Some(darker(fill)), None);
        assert_eq!(px(&canvas, stride, 0, 0), lighter(fill));
        assert_eq!(px(&canvas, stride, 99, 40), lighter(fill));
        assert_eq!(px(&canvas, stride, 50, 79), lighter(fill));
        assert_eq!(px(&canvas, stride, 50, 1), darker(fill));
        assert_eq!(px(&canvas, stride, 50, 24), darker(fill));
        assert_eq!(px(&canvas, stride, 50, 25), fill);
        assert_eq!(px(&canvas, stride, 1, 78), fill);
    }

    #[test]
    fn scaled_layout() {
        let layout = Layout::new(300, 200, 1.5, true);
        assert_eq!(layout.border, 2);
        assert_eq!(layout.body_top, 2 + 36);
        let tiny = Layout::new(3, 3, 2.0, true);
        assert_eq!(tiny.border, 1);
        assert_eq!(tiny.body_top, 2);
        assert_eq!(tiny.bar(0, 2.0), None);
    }

    #[test]
    fn bar_moves_and_wraps() {
        let layout = Layout::new(100, 80, 1.0, true);
        let b0 = layout.bar(0, 1.0).unwrap();
        let b1 = layout.bar(1, 1.0).unwrap();
        assert_eq!(
            b0,
            Rect {
                x: 1,
                y: 25,
                w: 16,
                h: 54
            }
        );
        assert_eq!(b1.x, 5);
        // inner width 98, bar 16: offsets cycle through 0..=82.
        for step in 0..200 {
            let r = layout.bar(step, 1.0).unwrap();
            assert!(r.x >= 1 && r.x + r.w <= 99);
        }
        let mut canvas = vec![0u8; 400 * 80];
        let fill = (10, 10, 10);
        paint(
            &mut canvas,
            400,
            &layout,
            fill,
            None,
            Some((b1, (250, 250, 250))),
        );
        assert_eq!(px(&canvas, 400, 5, 30), (250, 250, 250));
        assert_eq!(px(&canvas, 400, 20, 78), (250, 250, 250));
        assert_eq!(px(&canvas, 400, 21, 30), fill);
        assert_eq!(px(&canvas, 400, 4, 30), fill);
        assert_eq!(px(&canvas, 400, 5, 10), fill);
    }
}
