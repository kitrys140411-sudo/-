//! An ordered list of drawing primitives (the toolkit's frame output) and
//! damage computation by comparing two frames' lists.
//!
//! Primitives keep painter's order across kinds: a rectangle drawn after a
//! glyph covers it. The renderer batches consecutive primitives of the same
//! kind, texture and clip into one instanced draw call.

use super::{GlyphInst, ImageInst, RectInst};

/// Integer rectangle in buffer pixels, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> IRect {
        IRect { x, y, w, h }
    }

    /// The smallest integer rectangle covering a float rectangle.
    pub fn cover(x: f32, y: f32, w: f32, h: f32) -> IRect {
        let (x0, y0) = (x.floor() as i32, y.floor() as i32);
        let (x1, y1) = ((x + w).ceil() as i32, (y + h).ceil() as i32);
        IRect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn intersect(&self, o: &IRect) -> IRect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        IRect { x: x0, y: y0, w: (x1 - x0).max(0), h: (y1 - y0).max(0) }
    }

    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.w).max(o.x + o.w);
        let y1 = (self.y + self.h).max(o.y + o.h);
        IRect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

/// A texture to sample, with a version that changes whenever its contents
/// do (so a re-uploaded image damages the places it is drawn).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TexRef {
    pub id: u32,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prim {
    Rect(RectInst),
    Glyph(GlyphInst),
    Image(TexRef, ImageInst),
}

impl Prim {
    fn rect(&self) -> [f32; 4] {
        match self {
            Prim::Rect(r) => r.rect,
            Prim::Glyph(g) => g.rect,
            Prim::Image(_, i) => i.rect,
        }
    }
}

/// One primitive and the clip rectangle it is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub prim: Prim,
    pub clip: IRect,
}

impl Item {
    /// The pixels this item can touch.
    pub fn bounds(&self) -> IRect {
        let [x, y, w, h] = self.prim.rect();
        IRect::cover(x, y, w, h).intersect(&self.clip)
    }
}

#[derive(Default, Clone)]
pub struct DrawList {
    pub items: Vec<Item>,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn push(&mut self, prim: Prim, clip: IRect) {
        if !clip.is_empty() {
            self.items.push(Item { prim, clip });
        }
    }

    /// Pixels that may differ between `prev` and this list: the bounds of
    /// every item that is not part of their common prefix or suffix. An
    /// unchanged item at an unchanged place in the sequence covers the same
    /// pixels in the same order, so only the differing middle can change
    /// what is on screen.
    pub fn damage_since(&self, prev: &DrawList) -> Option<IRect> {
        let (a, b) = (&prev.items, &self.items);
        let prefix = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
        let max_suffix = a.len().min(b.len()) - prefix;
        let suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
        let mut dmg = IRect::default();
        for it in a[prefix..a.len() - suffix].iter().chain(b[prefix..b.len() - suffix].iter()) {
            dmg = dmg.union(&it.bounds());
        }
        (!dmg.is_empty()).then_some(dmg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: IRect = IRect::new(0, 0, 1000, 1000);

    fn rect(x: f32, y: f32, c: f32) -> Prim {
        Prim::Rect(RectInst::fill(x, y, 10.0, 10.0, [c, c, c, 1.0]))
    }

    fn list(prims: &[Prim]) -> DrawList {
        let mut l = DrawList::default();
        for p in prims {
            l.push(*p, FULL);
        }
        l
    }

    #[test]
    fn identical_lists_have_no_damage() {
        let a = list(&[rect(0.0, 0.0, 1.0), rect(20.0, 0.0, 1.0)]);
        assert_eq!(a.damage_since(&a.clone()), None);
    }

    #[test]
    fn changed_item_damages_only_itself() {
        let a = list(&[rect(0.0, 0.0, 1.0), rect(20.0, 0.0, 1.0), rect(40.0, 0.0, 1.0)]);
        let b = list(&[rect(0.0, 0.0, 1.0), rect(20.0, 0.0, 0.5), rect(40.0, 0.0, 1.0)]);
        assert_eq!(b.damage_since(&a), Some(IRect::new(20, 0, 10, 10)));
    }

    #[test]
    fn moved_item_damages_old_and_new_place() {
        let a = list(&[rect(0.0, 0.0, 1.0), rect(20.0, 0.0, 1.0)]);
        let b = list(&[rect(0.0, 0.0, 1.0), rect(25.5, 30.0, 1.0)]);
        assert_eq!(b.damage_since(&a), Some(IRect::new(20, 0, 16, 40)));
    }

    #[test]
    fn inserted_and_removed_items() {
        let a = list(&[rect(0.0, 0.0, 1.0), rect(100.0, 100.0, 1.0)]);
        let b = list(&[rect(0.0, 0.0, 1.0), rect(50.0, 50.0, 1.0), rect(100.0, 100.0, 1.0)]);
        assert_eq!(b.damage_since(&a), Some(IRect::new(50, 50, 10, 10)));
        assert_eq!(a.damage_since(&b), Some(IRect::new(50, 50, 10, 10)));
        // Everything removed: the old items' area.
        assert_eq!(DrawList::default().damage_since(&a), Some(IRect::new(0, 0, 110, 110)));
    }

    #[test]
    fn clip_limits_bounds() {
        let mut a = DrawList::default();
        a.push(rect(0.0, 0.0, 1.0), IRect::new(5, 5, 100, 100));
        assert_eq!(a.damage_since(&DrawList::default()), Some(IRect::new(5, 5, 5, 5)));
        // Fully clipped items are not even recorded.
        a.push(rect(0.0, 0.0, 1.0), IRect::new(500, 500, 0, 10));
        assert_eq!(a.items.len(), 1);
    }
}
