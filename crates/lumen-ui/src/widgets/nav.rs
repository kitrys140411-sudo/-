//! Navigation: segmented tabs with a sliding indicator, sidebar items and
//! the sidebar + content layout most apps use.

use std::hash::Hash;

use crate::anim::SpringParams;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::Key;
use crate::layout::{Layout, Size};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

use super::Frame;
use crate::layout::Sizing;

impl Ui {
    /// Segmented tabs choosing `selected` among `labels`. Left/Right switch
    /// when focused.
    pub fn tabs(&mut self, name: &str, selected: &mut usize, labels: &[&str]) -> Response {
        let id = self.make_id(("tabs", name));
        let mut r = self.interact(id);
        let n = labels.len();
        if n == 0 {
            return r;
        }
        let old = *selected;
        if r.focused {
            if self.key_pressed(Key::Left) {
                *selected = selected.saturating_sub(1);
            }
            if self.key_pressed(Key::Right) {
                *selected = (*selected + 1).min(n - 1);
            }
        }
        let seg_ids: Vec<Id> = (0..n).map(|i| id.with(("tab", i))).collect();
        for (i, sid) in seg_ids.iter().enumerate() {
            if self.interact(*sid).clicked {
                *selected = i;
            }
        }
        *selected = (*selected).min(n - 1);
        if *selected != old {
            r.changed = true;
            self.log(&format!("tab {name} {}", labels[*selected]));
        }
        let t = self.theme().clone();
        // The indicator slides between the segments' previous rectangles.
        let container = self.prev_rect(id);
        let target = self.prev_rect(seg_ids[*selected]);
        let (ix, iw) = match (container, target) {
            (Some(c), Some(s)) => (self.animate(id.with("x"), s.x - c.x, SpringParams::SNAPPY), self.animate(id.with("w"), s.w, SpringParams::SNAPPY)),
            _ => (0.0, 0.0),
        };
        let mut node = self.widget_node(id, "tabs", Some(name), Layout::row().height(Size::Px(34.0)).padding(3.0).gap(2.0), Sense::HOVER);
        node.focusable = true;
        node.deco.bg = t.control;
        node.deco.radius = t.radius + 2.0;
        if self.test.enabled() {
            node.value = Some(labels[*selected].to_string());
        }
        node.paint = Some(Box::new(move |p, r| {
            if iw > 0.0 {
                let ind = Rect::new(r.x + ix, r.y + 3.0, iw, r.h - 6.0);
                p.shadow(ind, t.radius, 4.0, 1.0, t.shadow.with_alpha(0.25));
                p.rounded(ind, t.radius, t.surface_hover.mix(t.text, 0.06));
            }
        }));
        let sel = *selected;
        let first_frame = iw == 0.0;
        self.add_container(node, |ui| {
            for (i, label) in labels.iter().enumerate() {
                let sid = seg_ids[i];
                let sr = ui.interact(sid);
                let hover = ui.animate_bool(sid.with("h"), sr.hovered && i != sel);
                let mut s = ui.widget_node(sid, "tab", Some(label), Layout::row().height(Size::FILL).padding((0.0, 16.0)).justify(crate::layout::Justify::Center), Sense::CLICK);
                s.deco.radius = t.radius;
                s.deco.bg = if first_frame && i == sel { t.surface_hover } else { t.control_hover.fade(hover * 0.6) };
                if ui.test.enabled() {
                    s.value = Some(if i == sel { "on" } else { "off" }.into());
                }
                ui.add_container(s, |ui| {
                    let mut l = Node::new(Layout::default());
                    l.measure = Measure::Text { text: label.to_string(), style: TextStyle::new(t.font_size).weight(Weight::Medium), wrap: Wrap::Ellipsize };
                    l.visual = Visual::Text { color: if i == sel { t.text } else { t.text_dim }, align: TextAlign::Center };
                    ui.add_node(l);
                });
            }
        });
        r
    }

    /// A sidebar entry (icon and label); draw `selected` ones highlighted.
    pub fn nav_item(&mut self, icon: Option<Icon>, label: &str, selected: bool) -> Response {
        let id = self.make_id(("nav", label));
        let mut r = self.interact(id);
        if r.focused && (self.key_pressed(Key::Enter) || self.key_pressed(Key::SPACE)) {
            r.clicked = true;
        }
        let t = self.theme().clone();
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let sel = self.animate(id.with("s"), if selected { 1.0 } else { 0.0 }, SpringParams::FAST);
        let mut n = self.widget_node(id, "nav", Some(label), Layout::row().size(Size::FILL, Size::Px(36.0)).padding((0.0, 12.0)).gap(12.0), Sense::CLICK);
        n.focusable = true;
        n.deco.radius = t.radius;
        n.deco.bg = t.control_hover.fade(hover * 0.7).mix(t.selection, sel);
        if self.test.enabled() {
            n.value = Some(if selected { "on" } else { "off" }.into());
        }
        let fg = if selected { t.text } else { t.text.mix(t.text_dim, 0.25) };
        self.add_container(n, |ui| {
            if let Some(icon) = icon {
                let mut i = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
                i.visual = Visual::Icon { icon, size: 16.0, color: if selected { t.accent } else { fg } };
                ui.add_node(i);
            }
            let mut l = Node::new(Layout::default().width(Size::FILL));
            l.measure = Measure::Text { text: label.to_string(), style: TextStyle::new(t.font_size).weight(if selected { Weight::Medium } else { Weight::Regular }), wrap: Wrap::Ellipsize };
            l.visual = Visual::Text { color: fg, align: TextAlign::Start };
            ui.add_node(l);
        });
        if r.clicked {
            self.log(&format!("nav {label}"));
        }
        r
    }

    /// A list of sidebar entries choosing `selected`; `.changed` when it did.
    pub fn nav_list(&mut self, name: impl Hash, selected: &mut usize, items: &[(Icon, &str)]) -> Response {
        let mut out = Response::default();
        self.push_id(("nav-list", name), |ui| {
            Frame::column().fill_width().gap(2.0).show(ui, |ui| {
                for (i, (icon, label)) in items.iter().enumerate() {
                    let r = ui.nav_item(Some(*icon), label, i == *selected);
                    if r.clicked && i != *selected {
                        *selected = i;
                        out.changed = true;
                    }
                    out = out.union(r);
                }
            });
        });
        out
    }

    /// The common app layout: a sidebar of `width` on the left (its own
    /// background, a hairline to the content) and the content filling the
    /// rest.
    pub fn sidebar_layout<A, B>(&mut self, width: f32, sidebar: impl FnOnce(&mut Ui) -> A, content: impl FnOnce(&mut Ui) -> B) -> (A, B) {
        let t = self.theme().clone();
        Frame::row().fill_width().height(Size::FILL).show(self, |ui| {
            let a = Frame::column().width(Size::Px(width)).height(Size::FILL).background(t.sidebar_bg).padding(t.grid).gap(2.0).show(ui, sidebar);
            ui.separator();
            let b = Frame::column().fill_width().height(Size::FILL).show(ui, content);
            (a, b)
        })
    }
}
