//! Two-state and choice controls: [`Switch`], checkbox, radio buttons.

use std::f32::consts::PI;

use crate::anim::SpringParams;
use crate::color::Color;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::input::Key;
use crate::layout::{Layout, Size};
use crate::text::{TextAlign, TextStyle, Wrap};
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

impl Ui {
    /// An on/off switch. `name` identifies it (test map, ids); it is not
    /// drawn — put a label next to it.
    pub fn switch(&mut self, name: &str, value: &mut bool) -> Response {
        let id = self.make_id(("switch", name));
        let mut r = self.interact(id);
        if r.focused && (self.key_pressed(Key::SPACE) || self.key_pressed(Key::Enter)) {
            r.clicked = true;
        }
        if r.clicked {
            *value = !*value;
            r.changed = true;
            self.log(&format!("toggle {name} {}", if *value { "on" } else { "off" }));
        }
        let t = self.theme.clone();
        let on = self.animate(id.with("on"), if *value { 1.0 } else { 0.0 }, SpringParams::SNAPPY);
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let press = self.animate_bool(id.with("p"), r.pressed);
        let mut n = self.widget_node(id, "switch", Some(name), Layout::default().size(Size::Px(44.0), Size::Px(24.0)), Sense::CLICK);
        n.focusable = true;
        n.deco.radius = 12.0;
        if self.test.enabled() {
            n.value = Some(if *value { "on" } else { "off" }.into());
        }
        let off_track = t.control_active;
        n.paint = Some(Box::new(move |p, r| {
            let track = off_track.mix(t.accent, on.clamp(0.0, 1.0)).mix(Color::WHITE.with_alpha(0.3), hover * 0.08);
            p.rounded(r, r.h / 2.0, track);
            // The knob stretches a little while pressed.
            let d = r.h - 6.0;
            let stretch = press * 4.0;
            let x = r.x + 3.0 + (r.w - 6.0 - d - stretch) * on;
            let knob = Rect::new(x, r.y + 3.0, d + stretch, d);
            p.shadow(knob, d / 2.0, 3.0, 1.0, Color::BLACK.with_alpha(0.35));
            p.rounded(knob, d / 2.0, Color::WHITE.mix(t.text, 0.15));
        }));
        self.add_node(n);
        r
    }

    /// A checkbox with a label; clicking either toggles it.
    pub fn checkbox(&mut self, label: &str, value: &mut bool) -> Response {
        let id = self.make_id(("checkbox", label));
        let mut r = self.interact(id);
        if r.focused && self.key_pressed(Key::SPACE) {
            r.clicked = true;
        }
        if r.clicked {
            *value = !*value;
            r.changed = true;
            self.log(&format!("toggle {label} {}", if *value { "on" } else { "off" }));
        }
        let t = self.theme.clone();
        let on = self.animate(id.with("on"), if *value { 1.0 } else { 0.0 }, SpringParams::FAST);
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let mut n = self.widget_node(id, "checkbox", Some(label), Layout::row().gap(10.0).height(Size::Px(28.0)), Sense::CLICK);
        n.focusable = true;
        n.deco.radius = 6.0;
        if self.test.enabled() {
            n.value = Some(if *value { "on" } else { "off" }.into());
        }
        self.add_container(n, |ui| {
            let mut b = Node::new(Layout::default().size(Size::Px(20.0), Size::Px(20.0)));
            b.paint = Some(Box::new(move |p, r| {
                let box_bg = t.input_bg.mix(t.control_hover, hover).mix(t.accent, on);
                p.rounded(r, 6.0, box_bg);
                if on < 0.5 {
                    p.border(r, 6.0, 1.5, t.text_dim.with_alpha(0.55 * (1.0 - on)));
                }
                if on > 0.01 {
                    p.push_opacity(on);
                    p.icon(Icon::Check, r, 16.0, t.on_accent);
                    p.pop_opacity();
                }
            }));
            ui.add_node(b);
            text_leaf(ui, label, t.text);
        });
        r
    }

    /// A radio button: selects `option` into `current` when clicked.
    pub fn radio<T: PartialEq + Clone>(&mut self, label: &str, current: &mut T, option: T) -> Response {
        let id = self.make_id(("radio", label));
        let mut r = self.interact(id);
        if r.focused && self.key_pressed(Key::SPACE) {
            r.clicked = true;
        }
        if r.clicked && *current != option {
            *current = option.clone();
            r.changed = true;
            self.log(&format!("select {label}"));
        }
        let selected = *current == option;
        let t = self.theme.clone();
        let on = self.animate(id.with("on"), if selected { 1.0 } else { 0.0 }, SpringParams::SNAPPY);
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let mut n = self.widget_node(id, "radio", Some(label), Layout::row().gap(10.0).height(Size::Px(28.0)), Sense::CLICK);
        n.focusable = true;
        n.deco.radius = 6.0;
        if self.test.enabled() {
            n.value = Some(if selected { "on" } else { "off" }.into());
        }
        self.add_container(n, |ui| {
            let mut b = Node::new(Layout::default().size(Size::Px(20.0), Size::Px(20.0)));
            b.paint = Some(Box::new(move |p, r| {
                let c = r.center();
                p.rounded(r, 10.0, t.input_bg.mix(t.control_hover, hover).mix(t.accent, on));
                if on < 0.5 {
                    p.border(r, 10.0, 1.5, t.text_dim.with_alpha(0.55 * (1.0 - on)));
                }
                let dot = 8.0 * on.clamp(0.0, 1.2);
                if dot > 0.2 {
                    p.rounded(Rect::new(c.x - dot / 2.0, c.y - dot / 2.0, dot, dot), dot / 2.0, t.on_accent);
                }
            }));
            ui.add_node(b);
            text_leaf(ui, label, t.text);
        });
        r
    }
}

pub(crate) fn text_leaf(ui: &mut Ui, text: &str, color: Color) {
    let size = ui.theme.font_size;
    let mut l = Node::new(Layout::default());
    l.measure = Measure::Text { text: text.to_string(), style: TextStyle::new(size), wrap: Wrap::Ellipsize };
    l.visual = Visual::Text { color, align: TextAlign::Start };
    ui.add_node(l);
}

/// Spinner arc angle for time `t` seconds (shared by spinners and busy
/// buttons): a rotating arc that grows and shrinks.
pub(crate) fn spinner_arc(t: f32) -> (f32, f32) {
    let rot = t * 2.0 * PI * 0.9;
    let phase = (t * 0.75).fract();
    let sweep = PI * (0.25 + 1.1 * (0.5 - 0.5 * (phase * 2.0 * PI).cos()));
    (rot + phase * PI, sweep)
}
