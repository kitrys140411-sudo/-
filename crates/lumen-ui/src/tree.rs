//! The per-frame box tree built by the app's `view` and the layout algorithm
//! run on it (see [`crate::layout`] for the model).

use std::rc::Rc;

use crate::color::Color;
use crate::geom::{Rect, Vec2};
use crate::icons::Icon;
use crate::id::Id;
use crate::layout::{Align, Dir, Justify, Layout, Size};
use crate::paint::Painter;
use crate::text::{Fonts, TextAlign, TextLayout, TextStyle, Wrap};

pub(crate) const NONE: u32 = u32::MAX;

/// What a box reacts to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sense {
    pub click: bool,
    pub drag: bool,
    pub hover: bool,
    pub scroll: bool,
    /// Hides everything below it in lower layers from the pointer (popover
    /// and dialog backgrounds, modal scrims).
    pub blocking: bool,
}

impl Sense {
    pub const NONE: Sense = Sense { click: false, drag: false, hover: false, scroll: false, blocking: false };
    pub const HOVER: Sense = Sense { click: false, drag: false, hover: true, scroll: false, blocking: false };
    pub const CLICK: Sense = Sense { click: true, drag: false, hover: true, scroll: false, blocking: false };
    pub const DRAG: Sense = Sense { click: true, drag: true, hover: true, scroll: false, blocking: false };
    pub const SCROLL: Sense = Sense { click: false, drag: false, hover: false, scroll: true, blocking: false };
    pub const BLOCK: Sense = Sense { click: false, drag: false, hover: false, scroll: false, blocking: true };

    pub fn any(&self) -> bool {
        self.click || self.drag || self.hover || self.scroll || self.blocking
    }
}

/// Background decoration of a box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Deco {
    pub bg: Color,
    pub radius: f32,
    pub border: Color,
    pub border_width: f32,
    /// Blur radius, vertical offset and color of a drop shadow.
    pub shadow: Option<(f32, f32, Color)>,
}

/// How a leaf measures itself.
pub(crate) enum Measure {
    None,
    Text { text: String, style: TextStyle, wrap: Wrap },
    Fixed(Vec2),
}

/// What a leaf draws (besides its decoration).
pub(crate) enum Visual {
    None,
    Text { color: Color, align: TextAlign },
    Icon { icon: Icon, size: f32, color: Color },
}

pub(crate) type PaintFn = Box<dyn FnOnce(&mut Painter, Rect)>;

pub(crate) struct Node {
    pub id: Option<Id>,
    pub layout: Layout,
    pub measure: Measure,
    pub deco: Deco,
    pub visual: Visual,
    /// Custom drawing under the children, and over them.
    pub paint: Option<PaintFn>,
    pub paint_over: Option<PaintFn>,
    pub sense: Sense,
    /// The box takes keyboard focus (Tab order follows creation order).
    pub focusable: bool,
    pub clip: bool,
    /// Children are drawn shifted by minus this (scroll position).
    pub scroll_offset: Vec2,
    /// Opacity of the box and everything in it.
    pub opacity: f32,
    /// Drawn shifted by this (appear animations; layout and hit boxes are
    /// unaffected).
    pub paint_offset: Vec2,
    /// Kind and label reported in the test widget map.
    pub kind: &'static str,
    pub label: Option<String>,
    /// State reported in the test widget map ("on", the text of an input...).
    pub value: Option<String>,
    pub parent: u32,
    pub first: u32,
    pub last: u32,
    pub next: u32,
    // -- Layout results -----------------------------------------------------------
    pub rect: Rect,
    /// Size of the children's extent plus padding (scrollable size).
    pub content: Vec2,
    pub text: Option<Rc<TextLayout>>,
    fit_w: f32,
    min_w: f32,
    fit_h: f32,
}

impl Node {
    pub fn new(layout: Layout) -> Node {
        Node {
            id: None,
            layout,
            measure: Measure::None,
            deco: Deco::default(),
            visual: Visual::None,
            paint: None,
            paint_over: None,
            sense: Sense::NONE,
            focusable: false,
            clip: false,
            scroll_offset: Vec2::ZERO,
            opacity: 1.0,
            paint_offset: Vec2::ZERO,
            kind: "",
            label: None,
            value: None,
            parent: NONE,
            first: NONE,
            last: NONE,
            next: NONE,
            rect: Rect::ZERO,
            content: Vec2::ZERO,
            text: None,
            fit_w: 0.0,
            min_w: 0.0,
            fit_h: 0.0,
        }
    }
}

/// How an overlay root is placed in the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// Top-left corner at a point (context menus at the pointer).
    At(Vec2),
    /// Below a rectangle, left-aligned with it; above it when there is no
    /// room below (dropdowns, menu buttons).
    Below(Rect),
    /// Above a rectangle, centered on it; below when there is no room
    /// (tooltips).
    Above(Rect),
    /// Centered in the window (dialogs).
    Center,
    /// Centered horizontally, `margin` above the bottom edge (toasts).
    Bottom(f32),
    /// The whole window (modal scrims).
    Fill,
}

pub(crate) struct Root {
    pub node: u32,
    pub anchor: Option<Anchor>,
    /// Overlay layer index (0 is the window content).
    pub layer: u32,
}

#[derive(Default)]
pub(crate) struct Tree {
    pub nodes: Vec<Node>,
    pub roots: Vec<Root>,
}

impl Tree {
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.roots.clear();
    }

    pub fn add(&mut self, parent: u32, node: Node) -> u32 {
        let i = self.nodes.len() as u32;
        self.nodes.push(node);
        self.nodes[i as usize].parent = parent;
        if parent != NONE {
            let p = &mut self.nodes[parent as usize];
            if p.last == NONE {
                p.first = i;
            } else {
                let last = p.last;
                p.last = i;
                self.nodes[last as usize].next = i;
            }
            self.nodes[parent as usize].last = i;
        }
        i
    }

    pub fn children(&self, i: u32) -> Children<'_> {
        Children { tree: self, next: self.nodes[i as usize].first }
    }

    fn child_list(&self, i: u32) -> Vec<u32> {
        self.children(i).collect()
    }

    // -- Layout ---------------------------------------------------------------------

    /// Lay out every root: the content root fills `window`, overlays are
    /// sized to fit (at most the window minus a margin) and placed by their
    /// anchor.
    pub fn layout(&mut self, window: Vec2, fonts: &mut Fonts, scale: f32) {
        for r in 0..self.roots.len() {
            let Root { node, anchor, .. } = self.roots[r];
            self.measure_w(node, fonts, scale);
            let margin = 8.0;
            let n = &self.nodes[node as usize];
            let w = match (anchor, n.layout.width) {
                (None, _) | (Some(Anchor::Fill), _) => window.x,
                (_, Size::Px(v)) => v,
                (_, Size::Frac(f)) => window.x * f,
                (_, Size::Fill(_)) => window.x - 2.0 * margin,
                (_, Size::Fit) => n.fit_w.min(window.x - 2.0 * margin),
            };
            let w = n.layout.clamp_w(w).max(0.0);
            self.arrange_w(node, w);
            self.measure_h(node, fonts, scale);
            let n = &self.nodes[node as usize];
            let h = match (anchor, n.layout.height) {
                (None, _) | (Some(Anchor::Fill), _) => window.y,
                (_, Size::Px(v)) => v,
                (_, Size::Frac(f)) => window.y * f,
                (_, Size::Fill(_)) => window.y - 2.0 * margin,
                (_, Size::Fit) => n.fit_h.min(window.y - 2.0 * margin),
            };
            let h = n.layout.clamp_h(h).max(0.0);
            let (x, y) = place(anchor, w, h, window, margin);
            self.arrange_h(node, x, y, h);
        }
    }

    /// Preferred and minimum widths, bottom-up.
    fn measure_w(&mut self, i: u32, fonts: &mut Fonts, scale: f32) {
        let kids = self.child_list(i);
        for &c in &kids {
            self.measure_w(c, fonts, scale);
        }
        let n = &self.nodes[i as usize];
        let l = n.layout;
        let pad = l.padding.horizontal();
        let gaps = l.gap * kids.len().saturating_sub(1) as f32;
        let contrib = |c: &Node| match c.layout.width {
            Size::Px(v) => (v, v),
            _ => (c.fit_w, c.min_w),
        };
        let (mut fit, mut min) = if kids.is_empty() {
            let (f, m) = match &n.measure {
                Measure::None => (0.0, 0.0),
                Measure::Fixed(v) => (v.x, v.x),
                Measure::Text { text, style, wrap } => {
                    let natural = fonts.layout(text, style, Wrap::None, None, scale).size.x;
                    (natural, if *wrap == Wrap::None { natural } else { 0.0 })
                }
            };
            (f + pad, m + pad)
        } else {
            match l.dir {
                Dir::Row => {
                    let (f, m) = kids.iter().map(|&c| contrib(&self.nodes[c as usize])).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
                    (f + gaps + pad, m + gaps + pad)
                }
                Dir::Column | Dir::Stack => {
                    let (f, m) = kids.iter().map(|&c| contrib(&self.nodes[c as usize])).fold((0.0f32, 0.0f32), |a, b| (a.0.max(b.0), a.1.max(b.1)));
                    (f + pad, m + pad)
                }
            }
        };
        if l.scroll_x {
            min = pad;
        }
        if let Size::Px(v) = l.width {
            (fit, min) = (v, v);
        }
        let n = &mut self.nodes[i as usize];
        n.fit_w = l.clamp_w(fit);
        n.min_w = l.clamp_w(min).min(n.fit_w);
    }

    /// Widths, top-down.
    fn arrange_w(&mut self, i: u32, w: f32) {
        self.nodes[i as usize].rect.w = w;
        let kids = self.child_list(i);
        if kids.is_empty() {
            return;
        }
        let l = self.nodes[i as usize].layout;
        let inner = (w - l.padding.horizontal()).max(0.0);
        let mut sizes = vec![0.0f32; kids.len()];
        match l.dir {
            Dir::Row => {
                let avail = inner - l.gap * (kids.len() - 1) as f32;
                let mut fill_weight = 0.0;
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    sizes[k] = match cn.layout.width {
                        Size::Px(v) => cn.layout.clamp_w(v),
                        Size::Frac(f) => cn.layout.clamp_w(f * inner),
                        Size::Fit => cn.fit_w,
                        Size::Fill(wt) => {
                            fill_weight += wt.max(0.0);
                            0.0
                        }
                    };
                }
                let used: f32 = sizes.iter().sum();
                if !l.scroll_x && used > avail {
                    // Shrink what can shrink, in proportion to how much it can.
                    let caps: Vec<f32> = kids
                        .iter()
                        .map(|&c| {
                            let cn = &self.nodes[c as usize];
                            if cn.layout.width == Size::Fit { (cn.fit_w - cn.min_w).max(0.0) } else { 0.0 }
                        })
                        .collect();
                    let total: f32 = caps.iter().sum();
                    if total > 0.0 {
                        let k = ((used - avail) / total).min(1.0);
                        for (s, cap) in sizes.iter_mut().zip(&caps) {
                            *s -= cap * k;
                        }
                    }
                }
                let remaining = (avail - sizes.iter().sum::<f32>()).max(0.0);
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    if let Size::Fill(wt) = cn.layout.width {
                        let share = if fill_weight > 0.0 { remaining * wt.max(0.0) / fill_weight } else { 0.0 };
                        let share = if l.scroll_x { share.max(cn.fit_w) } else { share };
                        sizes[k] = cn.layout.clamp_w(share).max(cn.min_w);
                    }
                }
            }
            Dir::Column | Dir::Stack => {
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    let align = cn.layout.align_self.unwrap_or(l.align);
                    let s = match cn.layout.width {
                        Size::Px(v) => v,
                        Size::Frac(f) => f * inner,
                        Size::Fill(_) if l.scroll_x => cn.fit_w.max(inner),
                        Size::Fill(_) => inner,
                        Size::Fit if l.scroll_x => cn.fit_w,
                        Size::Fit if align == Align::Stretch => inner,
                        Size::Fit => cn.fit_w.min(inner).max(cn.min_w),
                    };
                    sizes[k] = cn.layout.clamp_w(s);
                }
            }
        }
        for (k, &c) in kids.iter().enumerate() {
            self.arrange_w(c, sizes[k].max(0.0));
        }
    }

    /// Preferred heights (at the final widths), bottom-up.
    fn measure_h(&mut self, i: u32, fonts: &mut Fonts, scale: f32) {
        let kids = self.child_list(i);
        for &c in &kids {
            self.measure_h(c, fonts, scale);
        }
        let n = &mut self.nodes[i as usize];
        let l = n.layout;
        let pad = l.padding.vertical();
        let fit = if kids.is_empty() {
            let h = match &n.measure {
                Measure::None => 0.0,
                Measure::Fixed(v) => v.y,
                Measure::Text { text, style, wrap } => {
                    let width = (n.rect.w - l.padding.horizontal()).max(0.0);
                    // Rounding in the width passes must not wrap text that fit.
                    let t = fonts.layout(text, style, *wrap, Some(width + 0.01), scale);
                    let h = t.size.y;
                    n.text = Some(t);
                    h
                }
            };
            h + pad
        } else {
            let contrib = |c: &Node| match c.layout.height {
                Size::Px(v) => c.layout.clamp_h(v),
                _ => c.fit_h,
            };
            let nodes = &self.nodes;
            let it = kids.iter().map(|&c| contrib(&nodes[c as usize]));
            match l.dir {
                Dir::Column => it.sum::<f32>() + l.gap * (kids.len() - 1) as f32 + pad,
                Dir::Row | Dir::Stack => it.fold(0.0f32, f32::max) + pad,
            }
        };
        let n = &mut self.nodes[i as usize];
        n.fit_h = match l.height {
            Size::Px(v) => l.clamp_h(v),
            _ => l.clamp_h(fit),
        };
    }

    /// Heights and positions, top-down.
    fn arrange_h(&mut self, i: u32, x: f32, y: f32, h: f32) {
        {
            let n = &mut self.nodes[i as usize];
            n.rect.x = x;
            n.rect.y = y;
            n.rect.h = h;
        }
        let kids = self.child_list(i);
        if kids.is_empty() {
            let n = &mut self.nodes[i as usize];
            n.content = Vec2::new(n.rect.w, n.rect.h);
            return;
        }
        let n = &self.nodes[i as usize];
        let l = n.layout;
        let w = n.rect.w;
        let off = n.scroll_offset;
        let (ix, iy) = (x + l.padding.left - off.x, y + l.padding.top - off.y);
        let (iw, ih) = ((w - l.padding.horizontal()).max(0.0), (h - l.padding.vertical()).max(0.0));
        let cross = |a: Align, avail: f32, size: f32| match a {
            Align::Start | Align::Stretch => 0.0,
            Align::Center => ((avail - size) / 2.0).max(0.0),
            Align::End => (avail - size).max(0.0),
        };
        let mut extent = Vec2::ZERO;
        match l.dir {
            Dir::Column => {
                let mut sizes = vec![0.0f32; kids.len()];
                let avail = ih - l.gap * (kids.len() - 1) as f32;
                let mut fill_weight = 0.0;
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    sizes[k] = match cn.layout.height {
                        Size::Px(v) => cn.layout.clamp_h(v),
                        Size::Frac(f) => cn.layout.clamp_h(f * ih),
                        Size::Fit => cn.fit_h,
                        Size::Fill(wt) => {
                            fill_weight += wt.max(0.0);
                            0.0
                        }
                    };
                }
                let remaining = (avail - sizes.iter().sum::<f32>()).max(0.0);
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    if let Size::Fill(wt) = cn.layout.height {
                        let share = if fill_weight > 0.0 { remaining * wt.max(0.0) / fill_weight } else { 0.0 };
                        let share = if l.scroll_y { share.max(cn.fit_h) } else { share };
                        sizes[k] = cn.layout.clamp_h(share);
                    }
                }
                let total = sizes.iter().sum::<f32>() + l.gap * (kids.len() - 1) as f32;
                let (mut pos, extra) = justify(l.justify, ih - total, kids.len(), l.scroll_y);
                for (k, &c) in kids.iter().enumerate() {
                    let cn = &self.nodes[c as usize];
                    let cw = cn.rect.w;
                    let cx = ix + cross(cn.layout.align_self.unwrap_or(l.align), iw, cw);
                    self.arrange_h(c, cx, iy + pos, sizes[k]);
                    extent.x = extent.x.max(cx - ix + cw);
                    pos += sizes[k] + l.gap + extra;
                }
                extent.y = total;
            }
            Dir::Row => {
                let total = kids.iter().map(|&c| self.nodes[c as usize].rect.w).sum::<f32>() + l.gap * (kids.len() - 1) as f32;
                let (mut pos, extra) = justify(l.justify, iw - total, kids.len(), l.scroll_x);
                for &c in &kids {
                    let cn = &self.nodes[c as usize];
                    let align = cn.layout.align_self.unwrap_or(l.align);
                    let ch = match cn.layout.height {
                        Size::Px(v) => cn.layout.clamp_h(v),
                        Size::Frac(f) => cn.layout.clamp_h(f * ih),
                        Size::Fill(_) => cn.layout.clamp_h(ih),
                        Size::Fit if align == Align::Stretch => cn.layout.clamp_h(ih),
                        Size::Fit => cn.fit_h,
                    };
                    let cw = cn.rect.w;
                    let cy = iy + cross(align, ih, ch);
                    self.arrange_h(c, ix + pos, cy, ch);
                    extent.y = extent.y.max(cy - iy + ch);
                    pos += cw + l.gap + extra;
                }
                extent.x = total;
            }
            Dir::Stack => {
                for &c in &kids {
                    let cn = &self.nodes[c as usize];
                    let ch = match cn.layout.height {
                        Size::Px(v) => cn.layout.clamp_h(v),
                        Size::Frac(f) => cn.layout.clamp_h(f * ih),
                        Size::Fill(_) => cn.layout.clamp_h(ih),
                        Size::Fit if l.justify == Justify::SpaceBetween => cn.layout.clamp_h(ih),
                        Size::Fit => cn.fit_h,
                    };
                    let cw = cn.rect.w;
                    let cx = ix + cross(cn.layout.align_self.unwrap_or(l.align), iw, cw);
                    let vy = match l.justify {
                        Justify::Start | Justify::SpaceBetween => 0.0,
                        Justify::Center => ((ih - ch) / 2.0).max(0.0),
                        Justify::End => (ih - ch).max(0.0),
                    };
                    self.arrange_h(c, cx, iy + vy, ch);
                    extent.x = extent.x.max(cx - ix + cw);
                    extent.y = extent.y.max(vy + ch);
                }
            }
        }
        let n = &mut self.nodes[i as usize];
        n.content = Vec2::new(extent.x + l.padding.horizontal(), extent.y + l.padding.vertical());
    }
}

/// Start offset and extra gap for `free` main-axis space.
fn justify(j: Justify, free: f32, n: usize, unbounded: bool) -> (f32, f32) {
    if free <= 0.0 || unbounded {
        return (0.0, 0.0);
    }
    match j {
        Justify::Start => (0.0, 0.0),
        Justify::Center => (free / 2.0, 0.0),
        Justify::End => (free, 0.0),
        Justify::SpaceBetween if n > 1 => (0.0, free / (n - 1) as f32),
        Justify::SpaceBetween => (0.0, 0.0),
    }
}

/// Top-left corner of an overlay of `w`×`h`.
fn place(anchor: Option<Anchor>, w: f32, h: f32, win: Vec2, m: f32) -> (f32, f32) {
    let clamp_x = |x: f32| x.min(win.x - w - m).max(m);
    let clamp_y = |y: f32| y.min(win.y - h - m).max(m);
    match anchor {
        None | Some(Anchor::Fill) => (0.0, 0.0),
        Some(Anchor::At(p)) => {
            // Open toward where there is room.
            let x = if p.x + w > win.x - m { p.x - w } else { p.x };
            let y = if p.y + h > win.y - m { p.y - h } else { p.y };
            (clamp_x(x), clamp_y(y))
        }
        Some(Anchor::Below(r)) => {
            let gap = 4.0;
            let y = if r.bottom() + gap + h <= win.y - m || r.y - gap - h < m { r.bottom() + gap } else { r.y - gap - h };
            (clamp_x(r.x), clamp_y(y))
        }
        Some(Anchor::Above(r)) => {
            let gap = 6.0;
            let y = if r.y - gap - h >= m { r.y - gap - h } else { r.bottom() + gap };
            (clamp_x(r.center().x - w / 2.0), clamp_y(y))
        }
        Some(Anchor::Center) => (((win.x - w) / 2.0).round(), ((win.y - h) / 2.0).round()),
        Some(Anchor::Bottom(margin)) => (((win.x - w) / 2.0).round(), (win.y - h - margin).round()),
    }
}

pub(crate) struct Children<'a> {
    tree: &'a Tree,
    next: u32,
}

impl Iterator for Children<'_> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        if self.next == NONE {
            return None;
        }
        let i = self.next;
        self.next = self.tree.nodes[i as usize].next;
        Some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Insets;

    fn fonts() -> Fonts {
        Fonts::new("Inter", "JetBrains Mono").expect("fonts")
    }

    fn fixed(t: &mut Tree, parent: u32, w: f32, h: f32, layout: Layout) -> u32 {
        let mut n = Node::new(layout);
        n.measure = Measure::Fixed(Vec2::new(w, h));
        t.add(parent, n)
    }

    fn root(t: &mut Tree, layout: Layout) -> u32 {
        let r = t.add(NONE, Node::new(layout));
        t.roots.push(Root { node: r, anchor: None, layer: 0 });
        r
    }

    fn rect(t: &Tree, i: u32) -> Rect {
        t.nodes[i as usize].rect
    }

    #[test]
    fn row_with_fill_and_padding() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::row().padding(Insets::all(10.0)).gap(5.0));
        let a = fixed(&mut t, r, 50.0, 20.0, Layout::default());
        let b = t.add(r, Node::new(Layout::default().width(Size::FILL)));
        let c = fixed(&mut t, r, 30.0, 40.0, Layout::default());
        t.layout(Vec2::new(300.0, 100.0), &mut f, 1.0);
        assert_eq!(rect(&t, r), Rect::new(0.0, 0.0, 300.0, 100.0));
        assert_eq!(rect(&t, a), Rect::new(10.0, 40.0, 50.0, 20.0), "vertically centered");
        assert_eq!(rect(&t, b).w, 300.0 - 20.0 - 10.0 - 80.0);
        assert_eq!(rect(&t, c).x, 300.0 - 10.0 - 30.0);
        assert_eq!(rect(&t, c).y, 30.0);
    }

    #[test]
    fn fill_weights_and_fractions() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::row());
        let a = t.add(r, Node::new(Layout::default().width(Size::Fill(1.0))));
        let b = t.add(r, Node::new(Layout::default().width(Size::Fill(3.0))));
        let c = t.add(r, Node::new(Layout::default().width(Size::Frac(0.2))));
        t.layout(Vec2::new(500.0, 100.0), &mut f, 1.0);
        assert_eq!(rect(&t, c).w, 100.0);
        assert_eq!(rect(&t, a).w, 100.0);
        assert_eq!(rect(&t, b).w, 300.0);
        assert_eq!(rect(&t, b).x, 100.0);
    }

    #[test]
    fn column_alignment_and_justify() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::column().align(Align::Center).justify(Justify::Center).gap(10.0));
        let a = fixed(&mut t, r, 40.0, 10.0, Layout::default());
        let b = fixed(&mut t, r, 80.0, 20.0, Layout::default().align_self(Align::Stretch));
        t.layout(Vec2::new(200.0, 100.0), &mut f, 1.0);
        assert_eq!(rect(&t, a), Rect::new(80.0, 30.0, 40.0, 10.0));
        assert_eq!(rect(&t, b), Rect::new(0.0, 50.0, 200.0, 20.0));
    }

    #[test]
    fn space_between_and_end() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::row().justify(Justify::SpaceBetween));
        let a = fixed(&mut t, r, 10.0, 10.0, Layout::default());
        let b = fixed(&mut t, r, 10.0, 10.0, Layout::default());
        let c = fixed(&mut t, r, 10.0, 10.0, Layout::default());
        t.layout(Vec2::new(110.0, 10.0), &mut f, 1.0);
        assert_eq!((rect(&t, a).x, rect(&t, b).x, rect(&t, c).x), (0.0, 50.0, 100.0));
    }

    #[test]
    fn ellipsized_text_shrinks_in_a_tight_row() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::row());
        let mut label = Node::new(Layout::default());
        label.measure = Measure::Text { text: "A rather long label that cannot fit in the row".into(), style: TextStyle::new(14.0), wrap: Wrap::Ellipsize };
        let l = t.add(r, label);
        let b = fixed(&mut t, r, 60.0, 30.0, Layout::default());
        t.layout(Vec2::new(200.0, 40.0), &mut f, 1.0);
        assert_eq!(rect(&t, b).w, 60.0, "fixed sibling keeps its size");
        assert!((rect(&t, l).w - 140.0).abs() < 0.01, "{:?}", rect(&t, l));
        let tl = t.nodes[l as usize].text.clone().unwrap();
        assert!(tl.truncated && tl.size.x <= 140.0);
    }

    #[test]
    fn wrapped_text_height_follows_width() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::column().align(Align::Stretch));
        let mut p = Node::new(Layout::default());
        p.measure = Measure::Text { text: "word ".repeat(40), style: TextStyle::new(14.0), wrap: Wrap::Wrap { max_lines: 0 } };
        let l = t.add(r, p);
        t.layout(Vec2::new(200.0, 400.0), &mut f, 1.0);
        let lines = t.nodes[l as usize].text.as_ref().unwrap().lines.len();
        assert!(lines > 3);
        assert_eq!(rect(&t, l).h, TextStyle::new(14.0).line_px() * lines as f32);
        assert_eq!(rect(&t, l).w, 200.0);
    }

    #[test]
    fn scroll_content_is_unconstrained_and_offset() {
        let mut f = fonts();
        let mut t = Tree::default();
        let r = root(&mut t, Layout::column().size(Size::Px(100.0), Size::Px(100.0)));
        t.nodes[r as usize].layout.scroll_y = true;
        t.nodes[r as usize].scroll_offset = Vec2::new(0.0, 30.0);
        let items: Vec<u32> = (0..10).map(|_| fixed(&mut t, r, 50.0, 40.0, Layout::default())).collect();
        t.layout(Vec2::new(100.0, 100.0), &mut f, 1.0);
        assert_eq!(rect(&t, items[0]).y, -30.0);
        assert_eq!(rect(&t, items[9]).y, 9.0 * 40.0 - 30.0);
        assert_eq!(t.nodes[r as usize].content, Vec2::new(50.0, 400.0));
    }

    #[test]
    fn overlays_are_placed_by_anchor() {
        let mut f = fonts();
        let mut t = Tree::default();
        root(&mut t, Layout::column());
        let o = t.add(NONE, Node::new(Layout::column()));
        fixed(&mut t, o, 100.0, 50.0, Layout::default());
        t.roots.push(Root { node: o, anchor: Some(Anchor::Below(Rect::new(390.0, 10.0, 40.0, 20.0))), layer: 1 });
        let d = t.add(NONE, Node::new(Layout::column()));
        fixed(&mut t, d, 200.0, 100.0, Layout::default());
        t.roots.push(Root { node: d, anchor: Some(Anchor::Center), layer: 2 });
        t.layout(Vec2::new(400.0, 300.0), &mut f, 1.0);
        // Clamped inside the window with the 8 px margin.
        assert_eq!(rect(&t, o), Rect::new(292.0, 34.0, 100.0, 50.0));
        assert_eq!(rect(&t, d), Rect::new(100.0, 100.0, 200.0, 100.0));
    }
}
