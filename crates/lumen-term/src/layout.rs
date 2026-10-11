//! Split layout of a tab: a binary tree of panes.

pub type PaneId = u32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    /// `horizontal`: children side by side (a left of b); else stacked.
    Split { horizontal: bool, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A divider between two subtrees, for dragging: its rectangle and the path
/// (left = false, right = true) to its split node.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub rect: Rect,
    pub horizontal: bool,
    pub path: Vec<bool>,
    /// The split's whole area (to turn a pointer position into a ratio).
    pub area: Rect,
}

impl Node {
    pub fn panes(&self) -> Vec<PaneId> {
        match self {
            Node::Leaf(p) => vec![*p],
            Node::Split { a, b, .. } => {
                let mut v = a.panes();
                v.extend(b.panes());
                v
            }
        }
    }

    /// Pane rectangles in `r`, children separated by `gap`.
    pub fn layout(&self, r: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Node::Leaf(p) => out.push((*p, r)),
            Node::Split { horizontal, ratio, a, b } => {
                let (ra, rb) = split_rect(r, *horizontal, *ratio, gap);
                a.layout(ra, gap, out);
                b.layout(rb, gap, out);
            }
        }
    }

    pub fn dividers(&self, r: Rect, gap: f32, path: &mut Vec<bool>, out: &mut Vec<Divider>) {
        if let Node::Split { horizontal, ratio, a, b } = self {
            let (ra, rb) = split_rect(r, *horizontal, *ratio, gap);
            let rect = if *horizontal {
                Rect::new(ra.x + ra.w, r.y, rb.x - (ra.x + ra.w), r.h)
            } else {
                Rect::new(r.x, ra.y + ra.h, r.w, rb.y - (ra.y + ra.h))
            };
            out.push(Divider { rect, horizontal: *horizontal, path: path.clone(), area: r });
            path.push(false);
            a.dividers(ra, gap, path, out);
            path.pop();
            path.push(true);
            b.dividers(rb, gap, path, out);
            path.pop();
        }
    }

    pub fn set_ratio(&mut self, path: &[bool], ratio: f32) {
        match (self, path.split_first()) {
            (Node::Split { ratio: r, .. }, None) => *r = ratio.clamp(0.1, 0.9),
            (Node::Split { a, b, .. }, Some((side, rest))) => {
                if *side { b.set_ratio(rest, ratio) } else { a.set_ratio(rest, ratio) }
            }
            _ => {}
        }
    }

    /// Split `target` in two, the new pane after it. Returns false if
    /// `target` is not in the tree.
    pub fn split(&mut self, target: PaneId, new: PaneId, horizontal: bool) -> bool {
        match self {
            Node::Leaf(p) if *p == target => {
                *self = Node::Split { horizontal, ratio: 0.5, a: Box::new(Node::Leaf(target)), b: Box::new(Node::Leaf(new)) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split(target, new, horizontal) || b.split(target, new, horizontal),
        }
    }

    /// Remove `target`; its sibling takes the parent's place. Returns false
    /// when `target` is the only pane (the caller closes the tab).
    pub fn remove(&mut self, target: PaneId) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                if **a == Node::Leaf(target) {
                    *self = (**b).clone();
                    return true;
                }
                if **b == Node::Leaf(target) {
                    *self = (**a).clone();
                    return true;
                }
                a.remove(target) || b.remove(target)
            }
        }
    }
}

fn split_rect(r: Rect, horizontal: bool, ratio: f32, gap: f32) -> (Rect, Rect) {
    if horizontal {
        let wa = ((r.w - gap) * ratio).round();
        (Rect::new(r.x, r.y, wa, r.h), Rect::new(r.x + wa + gap, r.y, r.w - wa - gap, r.h))
    } else {
        let ha = ((r.h - gap) * ratio).round();
        (Rect::new(r.x, r.y, r.w, ha), Rect::new(r.x, r.y + ha + gap, r.w, r.h - ha - gap))
    }
}

/// The pane next to `from` in direction `dir` (the nearest one whose span
/// overlaps along the other axis).
pub fn neighbor(rects: &[(PaneId, Rect)], from: PaneId, dir: Dir) -> Option<PaneId> {
    let (_, r) = rects.iter().find(|(p, _)| *p == from)?;
    let (cx, cy) = r.center();
    rects
        .iter()
        .filter(|(p, o)| {
            *p != from
                && match dir {
                    Dir::Left => o.x + o.w <= r.x + 1.0 && o.y < r.y + r.h && o.y + o.h > r.y,
                    Dir::Right => o.x >= r.x + r.w - 1.0 && o.y < r.y + r.h && o.y + o.h > r.y,
                    Dir::Up => o.y + o.h <= r.y + 1.0 && o.x < r.x + r.w && o.x + o.w > r.x,
                    Dir::Down => o.y >= r.y + r.h - 1.0 && o.x < r.x + r.w && o.x + o.w > r.x,
                }
        })
        .min_by(|(_, a), (_, b)| {
            let d = |o: &Rect| {
                let (ox, oy) = o.center();
                (ox - cx).abs() + (oy - cy).abs()
            };
            d(a).total_cmp(&d(b))
        })
        .map(|(p, _)| *p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_layout_remove() {
        let mut n = Node::Leaf(1);
        assert!(n.split(1, 2, true));
        assert!(n.split(2, 3, false));
        assert_eq!(n.panes(), vec![1, 2, 3]);
        let mut rects = Vec::new();
        n.layout(Rect::new(0.0, 0.0, 101.0, 51.0), 1.0, &mut rects);
        assert_eq!(rects[0], (1, Rect::new(0.0, 0.0, 50.0, 51.0)));
        assert_eq!(rects[1], (2, Rect::new(51.0, 0.0, 50.0, 25.0)));
        assert_eq!(rects[2], (3, Rect::new(51.0, 26.0, 50.0, 25.0)));
        assert_eq!(neighbor(&rects, 1, Dir::Right), Some(2));
        assert_eq!(neighbor(&rects, 2, Dir::Down), Some(3));
        assert_eq!(neighbor(&rects, 3, Dir::Left), Some(1));
        assert_eq!(neighbor(&rects, 1, Dir::Left), None);
        let mut d = Vec::new();
        n.dividers(Rect::new(0.0, 0.0, 101.0, 51.0), 1.0, &mut Vec::new(), &mut d);
        assert_eq!(d.len(), 2);
        n.set_ratio(&d[1].path, 0.25);
        assert!(n.remove(2));
        assert_eq!(n.panes(), vec![1, 3]);
        assert!(n.remove(1));
        assert_eq!(n, Node::Leaf(3));
        assert!(!n.remove(3));
    }
}
