//! Geometry in logical pixels (the toolkit's unit; the renderer multiplies by
//! the output scale).

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

/// An axis-aligned rectangle: top-left corner and size.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 };

    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn from_min_max(min: Vec2, max: Vec2) -> Rect {
        Rect::new(min.x, min.y, (max.x - min.x).max(0.0), (max.y - min.y).max(0.0))
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn min(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn max(&self) -> Vec2 {
        Vec2::new(self.right(), self.bottom())
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Half-open containment: a point on the right/bottom edge is outside, so
    /// adjacent rectangles never both contain it.
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect::from_min_max(Vec2::new(self.x.min(o.x), self.y.min(o.y)), Vec2::new(self.right().max(o.right()), self.bottom().max(o.bottom())))
    }

    /// Grown by `d` on every side (negative shrinks).
    pub fn expand(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, (self.w + 2.0 * d).max(0.0), (self.h + 2.0 * d).max(0.0))
    }

    pub fn inset(&self, i: Insets) -> Rect {
        Rect::new(self.x + i.left, self.y + i.top, (self.w - i.left - i.right).max(0.0), (self.h - i.top - i.bottom).max(0.0))
    }

    pub fn translate(&self, d: Vec2) -> Rect {
        Rect::new(self.x + d.x, self.y + d.y, self.w, self.h)
    }

    /// A `w`×`h` rectangle centered in this one.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect::new(self.x + (self.w - w) / 2.0, self.y + (self.h - h) / 2.0, w, h)
    }
}

/// Space inside a box's edges (padding).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub const ZERO: Insets = Insets { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };

    pub const fn all(v: f32) -> Insets {
        Insets { left: v, top: v, right: v, bottom: v }
    }

    /// Vertical and horizontal padding (CSS order: `vertical, horizontal`).
    pub const fn symmetric(vertical: f32, horizontal: f32) -> Insets {
        Insets { left: horizontal, top: vertical, right: horizontal, bottom: vertical }
    }

    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Insets {
        Insets { left, top, right, bottom }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

impl From<f32> for Insets {
    fn from(v: f32) -> Insets {
        Insets::all(v)
    }
}

impl From<(f32, f32)> for Insets {
    /// `(vertical, horizontal)`.
    fn from((v, h): (f32, f32)) -> Insets {
        Insets::symmetric(v, h)
    }
}

impl From<(f32, f32, f32, f32)> for Insets {
    /// `(left, top, right, bottom)`.
    fn from((l, t, r, b): (f32, f32, f32, f32)) -> Insets {
        Insets::new(l, t, r, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_ops() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert_eq!(a.intersect(&b), Rect::new(5.0, 5.0, 5.0, 5.0));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        assert!(a.contains(Vec2::new(0.0, 9.9)));
        assert!(!a.contains(Vec2::new(10.0, 5.0)), "right edge is outside");
        assert!(!a.intersect(&Rect::new(20.0, 0.0, 5.0, 5.0)).w.is_nan());
        assert_eq!(a.inset(Insets::symmetric(1.0, 2.0)), Rect::new(2.0, 1.0, 6.0, 8.0));
        assert_eq!(a.centered(4.0, 2.0), Rect::new(3.0, 4.0, 4.0, 2.0));
    }
}
