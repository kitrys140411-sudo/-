//! Boxed lists of rows, the pattern of settings pages and device lists:
//! [`Ui::group`] (a titled card) holding [`ListRow`]s (icon, title,
//! subtitle and trailing controls), with hairlines between the rows.

use std::hash::Hash;

use crate::color::Color;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::Key;
use crate::layout::{Align, Layout, Size, Sizing};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

use super::Frame;

/// A row: leading icon, title with an optional subtitle below, and trailing
/// widgets (a switch, a value, a button) built by the closure given to
/// [`ListRow::show`]. Activatable rows react to clicks, Enter and Space.
#[must_use = "a ListRow does nothing until shown"]
pub struct ListRow {
    title: String,
    subtitle: Option<String>,
    icon: Option<(Icon, Option<Color>)>,
    activatable: bool,
    /// Clickable without being a focus stop or showing a chevron (rows
    /// whose trailing control is the focus stop, like switch rows).
    clickable: bool,
    key: Option<Id>,
    layout: Layout,
}

impl ListRow {
    pub fn new(title: impl Into<String>) -> ListRow {
        ListRow { title: title.into(), subtitle: None, icon: None, activatable: false, clickable: false, key: None, layout: Layout::row().width(Size::FILL) }
    }

    /// Secondary text under the title ("Connected", "Secured · 5 GHz").
    pub fn subtitle(mut self, s: impl Into<String>) -> ListRow {
        self.subtitle = Some(s.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> ListRow {
        self.icon = Some((icon, None));
        self
    }

    pub fn icon_colored(mut self, icon: Icon, color: Color) -> ListRow {
        self.icon = Some((icon, Some(color)));
        self
    }

    /// The row is a button (opens a page, connects to a network).
    pub fn activatable(mut self) -> ListRow {
        self.activatable = true;
        self
    }

    /// Identify the row by `key` instead of its title.
    pub fn id(mut self, key: impl Hash) -> ListRow {
        self.key = Some(Id(0).with(key));
        self
    }

    /// Build the row; `trailing` adds widgets at its end. `.clicked` when
    /// an activatable row was activated.
    pub fn show(self, ui: &mut Ui, trailing: impl FnOnce(&mut Ui)) -> Response {
        let id = match self.key {
            Some(k) => ui.make_id(k),
            None => ui.make_id(("row", &self.title)),
        };
        let t = ui.theme().clone();
        // Hairline between this row and the previous one of the group.
        if let Some(n) = ui.group_rows {
            if n > 0 {
                Frame::column().padding((0.0, 12.0)).show(ui, |ui| ui.separator());
            }
            ui.group_rows = Some(n + 1);
        }
        let sensitive = self.activatable || self.clickable;
        let mut r = if sensitive { ui.interact(id) } else { Response { id, ..Default::default() } };
        if self.activatable && r.focused && ui.keys_allowed() && (ui.key_pressed(Key::Enter) || ui.key_pressed(Key::SPACE)) {
            r.clicked = true;
        }
        let hover = ui.animate_bool(id.with("h"), sensitive && r.hovered);
        let press = ui.animate_bool(id.with("p"), r.pressed);
        let mut layout = self.layout;
        layout.min_height = layout.min_height.max(if self.subtitle.is_some() { 56.0 } else { 48.0 });
        layout.padding = (6.0, 12.0).into();
        layout.gap = 12.0;
        let sense = if sensitive { Sense::CLICK } else { Sense::NONE };
        let mut n = ui.widget_node(id, "row", Some(&self.title), layout, sense);
        n.focusable = self.activatable;
        n.deco.radius = t.radius;
        n.deco.bg = t.control_hover.fade(hover * 0.7).mix(t.control_active, press * 0.6);
        if let Some(s) = &self.subtitle {
            if ui.test.enabled() {
                n.value = Some(s.clone());
            }
        }
        ui.add_container(n, |ui| {
            if let Some((icon, color)) = self.icon {
                let mut i = Node::new(Layout::default().size(Size::Px(20.0), Size::Px(20.0)));
                i.visual = Visual::Icon { icon, size: 20.0, color: color.unwrap_or(t.text.mix(t.text_dim, 0.3)) };
                ui.add_node(i);
            }
            let text = Node::new(Layout::column().width(Size::FILL).gap(1.0).align(Align::Start));
            ui.add_container(text, |ui| {
                let mut l = Node::new(Layout::default());
                l.measure = Measure::Text { text: self.title.clone(), style: TextStyle::new(t.font_size).weight(Weight::Medium), wrap: Wrap::Ellipsize };
                l.visual = Visual::Text { color: t.text, align: TextAlign::Start };
                ui.add_node(l);
                if let Some(s) = &self.subtitle {
                    let mut l = Node::new(Layout::default());
                    l.measure = Measure::Text { text: s.clone(), style: TextStyle::new(t.size_small()), wrap: Wrap::Ellipsize };
                    l.visual = Visual::Text { color: t.text_dim, align: TextAlign::Start };
                    ui.add_node(l);
                }
            });
            // Trailing widgets are built outside the group's row counting.
            let saved = ui.group_rows.take();
            trailing(ui);
            ui.group_rows = saved;
            if self.activatable {
                let mut c = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
                c.visual = Visual::Icon { icon: Icon::ChevronRight, size: 16.0, color: t.text_dim };
                ui.add_node(c);
            }
        });
        if r.clicked && self.activatable {
            ui.log(&format!("row {}", self.title));
        }
        r
    }
}

impl Sizing for ListRow {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

impl Ui {
    /// A group of rows: an optional small heading above a card; hairlines
    /// separate the [`ListRow`]s inside.
    pub fn group<R>(&mut self, title: &str, f: impl FnOnce(&mut Ui) -> R) -> R {
        let t = self.theme().clone();
        Frame::column().gap(t.grid).show(self, |ui| {
            if !title.is_empty() {
                let mut l = Node::new(Layout::default().padding((0.0, 4.0, 0.0, 4.0)));
                l.measure = Measure::Text { text: title.to_string(), style: TextStyle::new(t.size_small()).weight(Weight::SemiBold), wrap: Wrap::Ellipsize };
                l.visual = Visual::Text { color: t.text_dim, align: TextAlign::Start };
                l.kind = "heading";
                l.label = Some(title.to_string());
                ui.add_node(l);
            }
            Frame::column().background(t.card_bg).radius(t.radius_large - 2.0).border(1.0, t.separator).padding(4.0).show(ui, |ui| {
                let saved = ui.group_rows.replace(0);
                let r = f(ui);
                ui.group_rows = saved;
                r
            })
        })
    }

    /// A row with a title (and subtitle) and a switch at its end; returns
    /// the switch's response.
    pub fn switch_row(&mut self, title: &str, subtitle: Option<&str>, value: &mut bool) -> Response {
        let mut row = ListRow::new(title);
        if let Some(s) = subtitle {
            row = row.subtitle(s);
        }
        // A click anywhere on the row flips the switch too.
        row.clickable = true;
        let mut out = Response::default();
        let r = row.show(self, |ui| out = ui.switch(title, value));
        if r.clicked {
            *value = !*value;
            out.changed = true;
            self.log(&format!("toggle {title} {}", if *value { "on" } else { "off" }));
        }
        out
    }
}
