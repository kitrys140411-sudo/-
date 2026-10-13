//! Containers: [`Frame`] (a row, column or stack with padding, gap,
//! alignment, background, border, shadow, clipping) and the shorthands
//! `ui.row`, `ui.column`, `ui.stack`, `ui.spacer`, `ui.space`,
//! `ui.separator`.

use std::hash::Hash;

use crate::color::Color;
use crate::geom::Insets;
use crate::id::Id;
use crate::layout::{Align, Dir, Justify, Layout, Size, Sizing};
use crate::tree::{Deco, Node, Sense};
use crate::ui::{Response, Ui};

/// A container box. Without an explicit width, a frame in a column (or
/// stack) spans the column's width and a frame in a row is as wide as its
/// content; its height fits the content.
#[must_use = "a Frame does nothing until shown"]
pub struct Frame {
    layout: Layout,
    width_set: bool,
    deco: Deco,
    clip: bool,
    key: Option<Id>,
    sense: Sense,
    opacity: f32,
    label: Option<String>,
}

impl Frame {
    fn new(layout: Layout) -> Frame {
        Frame { layout, width_set: false, deco: Deco::default(), clip: false, key: None, sense: Sense::NONE, opacity: 1.0, label: None }
    }

    /// Children left to right, vertically centered.
    pub fn row() -> Frame {
        Frame::new(Layout::row())
    }

    /// Children top to bottom, left-aligned.
    pub fn column() -> Frame {
        Frame::new(Layout::column())
    }

    /// Children on top of each other.
    pub fn stack() -> Frame {
        Frame::new(Layout::stack())
    }

    pub fn padding(mut self, p: impl Into<Insets>) -> Frame {
        self.layout.padding = p.into();
        self
    }

    pub fn gap(mut self, g: f32) -> Frame {
        self.layout.gap = g;
        self
    }

    /// Cross-axis alignment of the children (horizontal in a stack).
    pub fn align(mut self, a: Align) -> Frame {
        self.layout.align = a;
        self
    }

    /// Main-axis distribution (vertical placement in a stack).
    pub fn justify(mut self, j: Justify) -> Frame {
        self.layout.justify = j;
        self
    }

    /// Center the children on both axes.
    pub fn center(mut self) -> Frame {
        self.layout.align = Align::Center;
        self.layout.justify = Justify::Center;
        self
    }

    pub fn background(mut self, c: Color) -> Frame {
        self.deco.bg = c;
        self
    }

    pub fn radius(mut self, r: f32) -> Frame {
        self.deco.radius = r;
        self
    }

    pub fn border(mut self, width: f32, c: Color) -> Frame {
        self.deco.border_width = width;
        self.deco.border = c;
        self
    }

    /// A drop shadow (blur radius, vertical offset, color).
    pub fn shadow(mut self, blur: f32, dy: f32, c: Color) -> Frame {
        self.deco.shadow = Some((blur, dy, c));
        self
    }

    /// Clip the children to the frame.
    pub fn clip(mut self) -> Frame {
        self.clip = true;
        self
    }

    pub fn opacity(mut self, k: f32) -> Frame {
        self.opacity = k;
        self
    }

    /// Give the frame an id (to make it clickable with [`Frame::sense`],
    /// and to scope the ids of its children under `key`).
    pub fn id(mut self, key: impl Hash) -> Frame {
        self.key = Some(Id(0).with(key));
        self
    }

    pub fn sense(mut self, s: Sense) -> Frame {
        self.sense = s;
        self
    }

    /// Name the frame in the test widget map.
    pub fn label(mut self, l: impl Into<String>) -> Frame {
        self.label = Some(l.into());
        self
    }

    /// Build the children with `f`.
    pub fn show<R>(self, ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> R {
        self.show_with_response(ui, f).0
    }

    /// Build the children and return the frame's response too (needs an
    /// [`id`](Frame::id) and a [`sense`](Frame::sense)).
    pub fn show_with_response<R>(mut self, ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> (R, Response) {
        if !self.width_set {
            self.layout.width = ui.default_container_width();
        }
        let id = self.key.map(|k| ui.make_id(k));
        let mut n = Node::new(self.layout);
        n.deco = self.deco;
        n.clip = self.clip;
        n.opacity = self.opacity;
        n.id = id.or(self.label.as_ref().map(|l| ui.make_id(("frame", l))));
        n.sense = self.sense;
        if self.label.is_some() {
            n.kind = "frame";
            n.label = self.label;
        }
        let resp = n.id.map(|id| ui.interact(id)).unwrap_or_default();
        let (_, r) = ui.add_container(n, |ui| match id {
            Some(id) => ui.push_id(id, f),
            None => f(ui),
        });
        (r, resp)
    }
}

impl Sizing for Frame {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }

    fn width(mut self, s: Size) -> Self {
        self.layout.width = s;
        self.width_set = true;
        self
    }

    fn fill_width(self) -> Self {
        self.width(Size::FILL)
    }
}

impl Ui {
    /// A row (see [`Frame::row`]).
    pub fn row<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        Frame::row().gap(self.theme.grid).show(self, f)
    }

    /// A column (see [`Frame::column`]).
    pub fn column<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        Frame::column().gap(self.theme.grid).show(self, f)
    }

    /// A stack (see [`Frame::stack`]).
    pub fn stack<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        Frame::stack().show(self, f)
    }

    /// Flexible empty space: pushes what follows to the end of the row or
    /// column.
    pub fn spacer(&mut self) {
        let l = match self.parent_dir() {
            Dir::Row => Layout::default().width(Size::FILL),
            _ => Layout::default().height(Size::FILL),
        };
        self.add_node(Node::new(l));
    }

    /// Fixed empty space of `px` along the parent's main axis.
    pub fn space(&mut self, px: f32) {
        let l = match self.parent_dir() {
            Dir::Row => Layout::default().width(Size::Px(px)),
            _ => Layout::default().height(Size::Px(px)),
        };
        self.add_node(Node::new(l));
    }

    /// A hairline across the parent (horizontal in a column, vertical in a
    /// row).
    pub fn separator(&mut self) {
        let l = match self.parent_dir() {
            Dir::Row => Layout::default().size(Size::Px(1.0), Size::FILL),
            _ => Layout::default().size(Size::FILL, Size::Px(1.0)),
        };
        let mut n = Node::new(l);
        n.deco.bg = self.theme.separator;
        self.add_node(n);
    }

    /// A rounded card on the window background (groups of settings, boxed
    /// lists).
    pub fn card<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> R {
        let t = &self.theme;
        Frame::column().background(t.card_bg).radius(t.radius_large - 2.0).border(1.0, t.separator).padding(t.grid * 2.0).gap(t.grid).show(self, f)
    }
}
