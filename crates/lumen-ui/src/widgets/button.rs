//! Buttons: text, icon + text and icon-only, in four styles, with spring
//! hover/press feedback, keyboard activation (Enter/Space when focused) and
//! a toggled state for toolbar toggles.

use std::hash::Hash;

use crate::anim::SpringParams;
use crate::color::Color;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::Key;
use crate::layout::{Layout, Size, Sizing};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{CursorIcon, Response, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    /// A subtle filled button.
    #[default]
    Normal,
    /// The suggested action: accent colored.
    Primary,
    /// Transparent until hovered (toolbars, header bars, lists).
    Flat,
    /// A dangerous action (delete): danger colored.
    Destructive,
}

#[must_use = "a Button does nothing until added with ui.add"]
pub struct Button {
    label: Option<String>,
    icon: Option<Icon>,
    style: ButtonStyle,
    key: Option<Id>,
    enabled: bool,
    toggled: Option<bool>,
    round: bool,
    layout: Layout,
}

/// Height of buttons, entries and other controls.
pub const CONTROL_HEIGHT: f32 = 32.0;

impl Button {
    pub fn new(label: impl Into<String>) -> Button {
        Button { label: Some(label.into()), icon: None, style: ButtonStyle::Normal, key: None, enabled: true, toggled: None, round: false, layout: Layout::row() }
    }

    /// An icon-only button (flat by default).
    pub fn icon_only(icon: Icon) -> Button {
        Button { label: None, icon: Some(icon), style: ButtonStyle::Flat, key: None, enabled: true, toggled: None, round: false, layout: Layout::row() }
    }

    /// A leading icon.
    pub fn icon(mut self, icon: Icon) -> Button {
        self.icon = Some(icon);
        self
    }

    pub fn style(mut self, s: ButtonStyle) -> Button {
        self.style = s;
        self
    }

    pub fn primary(self) -> Button {
        self.style(ButtonStyle::Primary)
    }

    pub fn flat(self) -> Button {
        self.style(ButtonStyle::Flat)
    }

    pub fn destructive(self) -> Button {
        self.style(ButtonStyle::Destructive)
    }

    /// Disabled buttons are dimmed and ignore input.
    pub fn enabled(mut self, on: bool) -> Button {
        self.enabled = on;
        self
    }

    /// A toggle button: drawn pressed while `on`.
    pub fn toggled(mut self, on: bool) -> Button {
        self.toggled = Some(on);
        self
    }

    /// Fully rounded (pill or circle).
    pub fn round(mut self) -> Button {
        self.round = true;
        self
    }

    /// Identify the button by `key` instead of its label (two buttons with
    /// the same label in one scope, or labels that change).
    pub fn id(mut self, key: impl Hash) -> Button {
        self.key = Some(Id(0).with(key));
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let name = self.label.clone().or_else(|| self.icon.map(|i| i.name().to_string())).unwrap_or_default();
        let id = match self.key {
            Some(k) => ui.make_id(k),
            None => ui.make_id(("button", &name)),
        };
        let mut r = if self.enabled { ui.interact(id) } else { Response { id, ..Default::default() } };
        if self.enabled && r.focused && ui.keys_allowed() && (ui.key_pressed(Key::Enter) || ui.key_pressed(Key::SPACE)) {
            r.clicked = true;
        }
        if r.hovered {
            ui.set_cursor(CursorIcon::Default);
        }
        let t = ui.theme().clone();
        let toggled = self.toggled.unwrap_or(false);
        let hover = ui.animate_bool(id.with("h"), r.hovered);
        let press = ui.animate(id.with("p"), if r.pressed || toggled { 1.0 } else { 0.0 }, SpringParams::FAST);
        let (rest, hov, act, fg) = match self.style {
            ButtonStyle::Normal => (t.control, t.control_hover, t.control_active, t.text),
            ButtonStyle::Flat => (Color::TRANSPARENT, t.control_hover, t.control_active, t.text),
            ButtonStyle::Primary => (t.accent, t.accent_hover, t.accent_active, t.on_accent),
            ButtonStyle::Destructive => {
                let fg = if t.danger.luminance() > 0.35 { Color::hex("#1a0c0c") } else { Color::WHITE };
                (t.danger, t.danger.mix(Color::WHITE, 0.12), t.danger.mix(Color::BLACK, 0.12), fg)
            }
        };
        let bg = rest.mix(hov, hover).mix(act, press);
        let icon_only = self.label.is_none();
        let mut layout = self.layout;
        layout.height = match layout.height {
            Size::Fit => Size::Px(CONTROL_HEIGHT),
            h => h,
        };
        if icon_only && layout.width == Size::Fit {
            layout.width = Size::Px(CONTROL_HEIGHT);
            layout.justify = crate::layout::Justify::Center;
        } else {
            let left = if self.icon.is_some() { 12.0 } else { 16.0 };
            layout.padding = crate::geom::Insets::new(left, 0.0, 16.0, 0.0);
            layout.justify = crate::layout::Justify::Center;
        }
        layout.gap = 8.0;
        let sense = if self.enabled { Sense::CLICK } else { Sense::NONE };
        let mut n = ui.widget_node(id, "button", Some(&name), layout, sense);
        n.focusable = self.enabled;
        n.deco.bg = bg;
        n.deco.radius = if self.round { CONTROL_HEIGHT / 2.0 } else { t.radius };
        if !self.enabled {
            n.opacity = 0.45;
        }
        if self.toggled.is_some() && ui.test.enabled() {
            n.value = Some(if toggled { "on" } else { "off" }.into());
        }
        ui.add_container(n, |ui| {
            if let Some(icon) = self.icon {
                let mut i = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
                i.visual = Visual::Icon { icon, size: 16.0, color: fg };
                ui.add_node(i);
            }
            if let Some(label) = self.label {
                let mut l = Node::new(Layout::default());
                l.measure = Measure::Text { text: label, style: TextStyle::new(t.font_size).weight(Weight::Medium), wrap: Wrap::Ellipsize };
                l.visual = Visual::Text { color: fg, align: TextAlign::Center };
                ui.add_node(l);
            }
        });
        if r.clicked {
            ui.log(&format!("click {name}"));
        }
        r
    }
}

impl Sizing for Button {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

impl Ui {
    /// A normal button; `.clicked` when activated.
    pub fn button(&mut self, label: impl Into<String>) -> Response {
        Button::new(label).show(self)
    }

    /// The suggested action of a view or dialog.
    pub fn primary_button(&mut self, label: impl Into<String>) -> Response {
        Button::new(label).primary().show(self)
    }

    /// A flat icon-only button (toolbars, header bars).
    pub fn icon_button(&mut self, icon: Icon) -> Response {
        Button::icon_only(icon).show(self)
    }

    /// Add any widget builder.
    pub fn add<W: Widget>(&mut self, w: W) -> W::Output {
        w.show(self)
    }
}

/// Anything `ui.add` can show.
pub trait Widget {
    type Output;
    fn show(self, ui: &mut Ui) -> Self::Output;
}

impl Widget for Button {
    type Output = Response;
    fn show(self, ui: &mut Ui) -> Response {
        Button::show(self, ui)
    }
}

impl Widget for super::Label {
    type Output = ();
    fn show(self, ui: &mut Ui) {
        super::Label::show(self, ui)
    }
}
