//! Text: [`Label`] and the shorthands `ui.label`, `ui.heading`,
//! `ui.title_text`, `ui.caption`, `ui.mono`, `ui.paragraph`.

use crate::color::Color;
use crate::layout::{Layout, Sizing};
use crate::text::{Family, TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Visual};
use crate::ui::Ui;

/// A piece of text (not selectable). Single line and ellipsized by default.
#[must_use = "a Label does nothing until added with ui.add"]
pub struct Label {
    text: String,
    size: Option<f32>,
    weight: Weight,
    family: Family,
    color: Option<Color>,
    dim: bool,
    wrap: Wrap,
    align: TextAlign,
    line_height: f32,
    layout: Layout,
}

impl Label {
    pub fn new(text: impl Into<String>) -> Label {
        Label {
            text: text.into(),
            size: None,
            weight: Weight::Regular,
            family: Family::Ui,
            color: None,
            dim: false,
            wrap: Wrap::Ellipsize,
            align: TextAlign::Start,
            line_height: 1.4,
            layout: Layout::default(),
        }
    }

    /// Size in logical pixels (default: the theme's body size).
    pub fn size(mut self, px: f32) -> Label {
        self.size = Some(px);
        self
    }

    pub fn weight(mut self, w: Weight) -> Label {
        self.weight = w;
        self
    }

    /// Semi-bold.
    pub fn strong(self) -> Label {
        self.weight(Weight::SemiBold)
    }

    /// The monospace font.
    pub fn mono(mut self) -> Label {
        self.family = Family::Mono;
        self
    }

    pub fn color(mut self, c: Color) -> Label {
        self.color = Some(c);
        self
    }

    /// The theme's secondary text color.
    pub fn dim(mut self) -> Label {
        self.dim = true;
        self
    }

    /// Wrap at spaces into as many lines as needed.
    pub fn wrap(mut self) -> Label {
        self.wrap = Wrap::Wrap { max_lines: 0 };
        self
    }

    /// Wrap into at most `n` lines, the last one ellipsized.
    pub fn max_lines(mut self, n: u16) -> Label {
        self.wrap = Wrap::Wrap { max_lines: n };
        self
    }

    /// Never shorten or wrap (newlines still start lines).
    pub fn no_wrap(mut self) -> Label {
        self.wrap = Wrap::None;
        self
    }

    pub fn align(mut self, a: TextAlign) -> Label {
        self.align = a;
        self
    }

    pub fn line_height(mut self, k: f32) -> Label {
        self.line_height = k;
        self
    }

    pub fn show(self, ui: &mut Ui) {
        let t = ui.theme();
        let color = self.color.unwrap_or(if self.dim { t.text_dim } else { t.text });
        let size = self.size.unwrap_or(t.font_size);
        let style = TextStyle { size, weight: self.weight, family: self.family, line_height: self.line_height };
        let mut n = Node::new(self.layout);
        n.kind = "label";
        n.label = Some(self.text.clone());
        n.measure = Measure::Text { text: self.text, style, wrap: self.wrap };
        n.visual = Visual::Text { color, align: self.align };
        ui.add_node(n);
    }
}

impl Sizing for Label {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

impl Ui {
    /// Body text, single line, ellipsized to fit.
    pub fn label(&mut self, text: impl Into<String>) {
        Label::new(text).show(self);
    }

    /// Wrapped body text.
    pub fn paragraph(&mut self, text: impl Into<String>) {
        Label::new(text).wrap().fill_width().show(self);
    }

    /// A section heading.
    pub fn heading(&mut self, text: impl Into<String>) {
        let s = self.theme.size_heading();
        Label::new(text).size(s).weight(Weight::SemiBold).show(self);
    }

    /// A large page title.
    pub fn title_text(&mut self, text: impl Into<String>) {
        let s = self.theme.size_title();
        Label::new(text).size(s).weight(Weight::Bold).line_height(1.25).show(self);
    }

    /// Small secondary text.
    pub fn caption(&mut self, text: impl Into<String>) {
        let s = self.theme.size_small();
        Label::new(text).size(s).dim().show(self);
    }

    /// Monospace text.
    pub fn mono(&mut self, text: impl Into<String>) {
        let s = self.theme.font_size - 1.0;
        Label::new(text).mono().size(s).show(self);
    }
}
