//! The layout model: boxes in rows, columns and stacks (a small flexbox).
//!
//! Every box has a width and a height [`Size`]:
//! - `Fit`: as large as its content (text, icon, children);
//! - `Px(n)`: exactly `n` logical pixels;
//! - `Fill(weight)`: on the parent's main axis, a share of the space left
//!   after the other children; on the cross axis, the parent's full inner size;
//! - `Frac(f)`: a fraction of the parent's inner size.
//!
//! A `Row` places children left to right, a `Column` top to bottom, a `Stack`
//! on top of each other. `padding` insets the children, `gap` separates them,
//! `justify` distributes free space on the main axis and `align` places
//! children on the cross axis (`Stretch` makes `Fit` children as wide/tall as
//! the parent). In a `Stack`, `align` places children horizontally and
//! `justify` vertically. When `Fit` children of a row do not fit, those that can
//! shrink (ellipsized or wrapped text, and boxes containing it) shrink first.
//!
//! Layout runs after the UI is built, in four passes: preferred and minimum
//! widths bottom-up, widths top-down, heights bottom-up (text wraps at its
//! final width), heights and positions top-down.

use crate::geom::Insets;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Fit,
    Px(f32),
    Fill(f32),
    Frac(f32),
}

impl Size {
    /// `Fill(1.0)`.
    pub const FILL: Size = Size::Fill(1.0);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Row,
    Column,
    Stack,
}

/// Cross-axis placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

/// Main-axis distribution of free space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Layout properties of one box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub dir: Dir,
    pub width: Size,
    pub height: Size,
    pub min_width: f32,
    pub max_width: f32,
    pub min_height: f32,
    pub max_height: f32,
    pub padding: Insets,
    pub gap: f32,
    pub align: Align,
    pub justify: Justify,
    /// Overrides the parent's `align` for this box.
    pub align_self: Option<Align>,
    /// Content may be wider (taller) than the box: children are laid out
    /// unconstrained on that axis (scroll areas).
    pub scroll_x: bool,
    pub scroll_y: bool,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            dir: Dir::Column,
            width: Size::Fit,
            height: Size::Fit,
            min_width: 0.0,
            max_width: f32::INFINITY,
            min_height: 0.0,
            max_height: f32::INFINITY,
            padding: Insets::ZERO,
            gap: 0.0,
            align: Align::Start,
            justify: Justify::Start,
            align_self: None,
            scroll_x: false,
            scroll_y: false,
        }
    }
}

impl Layout {
    /// A row: children left to right, vertically centered.
    pub fn row() -> Layout {
        Layout { dir: Dir::Row, align: Align::Center, ..Layout::default() }
    }

    /// A column: children top to bottom, left-aligned.
    pub fn column() -> Layout {
        Layout { dir: Dir::Column, ..Layout::default() }
    }

    /// A stack: children on top of each other, top-left.
    pub fn stack() -> Layout {
        Layout { dir: Dir::Stack, ..Layout::default() }
    }

    pub fn width(mut self, s: Size) -> Layout {
        self.width = s;
        self
    }

    pub fn height(mut self, s: Size) -> Layout {
        self.height = s;
        self
    }

    pub fn size(mut self, w: Size, h: Size) -> Layout {
        self.width = w;
        self.height = h;
        self
    }

    pub fn padding(mut self, p: impl Into<Insets>) -> Layout {
        self.padding = p.into();
        self
    }

    pub fn gap(mut self, g: f32) -> Layout {
        self.gap = g;
        self
    }

    pub fn align(mut self, a: Align) -> Layout {
        self.align = a;
        self
    }

    pub fn justify(mut self, j: Justify) -> Layout {
        self.justify = j;
        self
    }

    pub fn align_self(mut self, a: Align) -> Layout {
        self.align_self = Some(a);
        self
    }

    pub fn min_width(mut self, w: f32) -> Layout {
        self.min_width = w;
        self
    }

    pub fn max_width(mut self, w: f32) -> Layout {
        self.max_width = w;
        self
    }

    pub fn min_height(mut self, h: f32) -> Layout {
        self.min_height = h;
        self
    }

    pub fn max_height(mut self, h: f32) -> Layout {
        self.max_height = h;
        self
    }

    pub(crate) fn clamp_w(&self, w: f32) -> f32 {
        w.min(self.max_width).max(self.min_width)
    }

    pub(crate) fn clamp_h(&self, h: f32) -> f32 {
        h.min(self.max_height).max(self.min_height)
    }
}

/// Builder methods shared by everything that carries a [`Layout`] (frames
/// and widgets): `.width(Size::Fill(1.0))`, `.padding(8.0)` and so on.
pub trait Sizing: Sized {
    fn layout_mut(&mut self) -> &mut Layout;

    fn width(mut self, s: Size) -> Self {
        self.layout_mut().width = s;
        self
    }

    fn height(mut self, s: Size) -> Self {
        self.layout_mut().height = s;
        self
    }

    /// `Fill(1)` width.
    fn fill_width(self) -> Self {
        self.width(Size::FILL)
    }

    /// `Fill(1)` height.
    fn fill_height(self) -> Self {
        self.height(Size::FILL)
    }

    fn min_width(mut self, w: f32) -> Self {
        self.layout_mut().min_width = w;
        self
    }

    fn max_width(mut self, w: f32) -> Self {
        self.layout_mut().max_width = w;
        self
    }

    fn min_height(mut self, h: f32) -> Self {
        self.layout_mut().min_height = h;
        self
    }

    fn max_height(mut self, h: f32) -> Self {
        self.layout_mut().max_height = h;
        self
    }

    fn align_self(mut self, a: Align) -> Self {
        self.layout_mut().align_self = Some(a);
        self
    }
}
