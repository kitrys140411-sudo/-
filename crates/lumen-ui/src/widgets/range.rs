//! Value controls: [`Slider`], progress bars and spinners.

use std::ops::RangeInclusive;

use crate::anim::SpringParams;
use crate::color::Color;
use crate::geom::Rect;
use crate::input::{Key, Mods};
use crate::layout::{Layout, Size, Sizing};
use crate::tree::{Node, Sense};
use crate::ui::{Response, Ui};

#[must_use = "a Slider does nothing until shown"]
pub struct Slider<'a> {
    name: String,
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    step: Option<f32>,
    layout: Layout,
}

impl<'a> Slider<'a> {
    /// A slider for `value` in `range`; `name` identifies it (not drawn).
    pub fn new(name: impl Into<String>, value: &'a mut f32, range: RangeInclusive<f32>) -> Slider<'a> {
        Slider { name: name.into(), value, range, step: None, layout: Layout::default().width(Size::FILL).min_width(120.0) }
    }

    /// Snap values to multiples of `step`.
    pub fn step(mut self, step: f32) -> Slider<'a> {
        self.step = Some(step);
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let id = ui.make_id(("slider", &self.name));
        let mut r = ui.interact(id);
        let (lo, hi) = (*self.range.start(), *self.range.end());
        let span = (hi - lo).max(f32::EPSILON);
        let knob = 18.0;
        let old = *self.value;
        let snap = |v: f32| {
            let v = v.clamp(lo, hi);
            match self.step {
                Some(s) if s > 0.0 => (lo + ((v - lo) / s).round() * s).clamp(lo, hi),
                _ => v,
            }
        };
        // Pointer: press jumps, dragging follows.
        let at = |x: f32, rect: Rect| lo + ((x - rect.x - knob / 2.0) / (rect.w - knob).max(1.0)).clamp(0.0, 1.0) * span;
        if let Some((p, _)) = ui.press_pos(id) {
            *self.value = snap(at(p.x, r.rect));
        }
        if r.dragging {
            if let Some(p) = ui.pointer() {
                *self.value = snap(at(p.x, r.rect));
            }
        }
        if r.focused {
            let small = self.step.unwrap_or(span / 20.0);
            for (key, d) in [(Key::Left, -small), (Key::Down, -small), (Key::Right, small), (Key::Up, small), (Key::PageDown, -span / 10.0), (Key::PageUp, span / 10.0)] {
                while ui.take_key(key, Mods::NONE) {
                    *self.value = snap(*self.value + d);
                }
            }
            if ui.key_pressed(Key::Home) {
                *self.value = lo;
            }
            if ui.key_pressed(Key::End) {
                *self.value = hi;
            }
        }
        if *self.value != old {
            r.changed = true;
            ui.log(&format!("slider {} {}", self.name, *self.value));
        }
        let t = ui.theme().clone();
        let frac = ((*self.value - lo) / span).clamp(0.0, 1.0);
        let shown = ui.animate(id.with("v"), frac, SpringParams { stiffness: 1400.0, damping: 1.0 });
        let hover = ui.animate_bool(id.with("h"), r.hovered || r.dragging);
        let mut layout = self.layout;
        layout.height = Size::Px(24.0);
        let mut n = ui.widget_node(id, "slider", Some(&self.name), layout, Sense::DRAG);
        n.focusable = true;
        n.deco.radius = 12.0;
        if ui.test.enabled() {
            n.value = Some(format!("{}", *self.value));
        }
        n.paint = Some(Box::new(move |p, r| {
            let cy = r.center().y;
            let track = Rect::new(r.x + knob / 2.0, cy - 2.0, r.w - knob, 4.0);
            p.rounded(track, 2.0, t.control_active);
            let x = track.x + track.w * shown;
            p.rounded(Rect::new(track.x, track.y, x - track.x, track.h), 2.0, t.accent);
            let k = knob + hover * 2.0;
            let kr = Rect::new(x - k / 2.0, cy - k / 2.0, k, k);
            if hover > 0.01 {
                p.rounded(kr.expand(4.0 * hover), k / 2.0 + 4.0, t.accent.with_alpha(0.18 * hover));
            }
            p.shadow(kr, k / 2.0, 4.0, 1.0, Color::BLACK.with_alpha(0.4));
            p.rounded(kr, k / 2.0, Color::WHITE.mix(t.text, 0.15));
        }));
        ui.add_node(n);
        r
    }
}

impl Sizing for Slider<'_> {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// A progress bar: a fraction, or indeterminate (busy).
#[must_use = "a ProgressBar does nothing until shown"]
pub struct ProgressBar {
    fraction: Option<f32>,
    layout: Layout,
}

impl ProgressBar {
    /// `fraction` in 0..=1.
    pub fn new(fraction: f32) -> ProgressBar {
        ProgressBar { fraction: Some(fraction), layout: Layout::default().width(Size::FILL).min_width(80.0) }
    }

    /// Busy without a known amount: a moving segment.
    pub fn indeterminate() -> ProgressBar {
        ProgressBar { fraction: None, layout: Layout::default().width(Size::FILL).min_width(80.0) }
    }

    pub fn show(self, ui: &mut Ui) {
        let id = ui.make_id("progress");
        let t = ui.theme().clone();
        let mut layout = self.layout;
        layout.height = Size::Px(6.0);
        let mut n = Node::new(layout);
        n.kind = "progress";
        n.label = Some("progress".into());
        match self.fraction {
            Some(f) => {
                let shown = ui.animate(id, f.clamp(0.0, 1.0), SpringParams::GENTLE);
                if ui.test.enabled() {
                    n.value = Some(format!("{:.3}", f));
                }
                n.paint = Some(Box::new(move |p, r| {
                    p.rounded(r, 3.0, t.control_active);
                    if shown > 0.001 {
                        p.rounded(Rect::new(r.x, r.y, (r.w * shown).max(r.h), r.h), 3.0, t.accent);
                    }
                }));
            }
            None => {
                let secs = ui.now().duration_since(*START).as_secs_f32();
                ui.request_repaint();
                n.paint = Some(Box::new(move |p, r| {
                    p.rounded(r, 3.0, t.control_active);
                    // A segment sweeping across with an eased speed.
                    let phase = (secs * 0.7).fract();
                    let e = crate::anim::ease_out(phase);
                    let seg = r.w * 0.35;
                    let x0 = r.x - seg + (r.w + seg) * e;
                    p.push_clip(r);
                    p.rounded(Rect::new(x0, r.y, seg, r.h), 3.0, t.accent);
                    p.pop_clip();
                }));
            }
        }
        ui.add_node(n);
    }
}

impl Sizing for ProgressBar {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// A spinning busy indicator.
pub struct Spinner {
    size: f32,
    color: Option<Color>,
}

impl Spinner {
    pub fn new() -> Spinner {
        Spinner { size: 16.0, color: None }
    }

    pub fn size(mut self, px: f32) -> Spinner {
        self.size = px;
        self
    }

    pub fn color(mut self, c: Color) -> Spinner {
        self.color = Some(c);
        self
    }

    pub fn show(self, ui: &mut Ui) {
        let secs = ui.now().duration_since(*START).as_secs_f32();
        ui.request_repaint();
        let color = self.color.unwrap_or(ui.theme().text);
        let mut n = Node::new(Layout::default().size(Size::Px(self.size), Size::Px(self.size)));
        n.kind = "spinner";
        n.label = Some("spinner".into());
        let s = self.size;
        n.paint = Some(Box::new(move |p, r| {
            let (start, sweep) = super::toggle::spinner_arc(secs);
            let th = (s / 8.0).max(2.0);
            p.arc(r.center(), s / 2.0, th, start, sweep, color);
        }));
        ui.add_node(n);
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Spinner::new()
    }
}

/// Process-wide animation epoch (spinners keep their phase across frames).
static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);

impl Ui {
    /// A slider for `value` in `range`.
    pub fn slider(&mut self, name: &str, value: &mut f32, range: RangeInclusive<f32>) -> Response {
        Slider::new(name, value, range).show(self)
    }

    /// A progress bar for `fraction` (0..=1).
    pub fn progress(&mut self, fraction: f32) {
        ProgressBar::new(fraction).show(self);
    }

    /// A 16 px spinner.
    pub fn spinner(&mut self) {
        Spinner::new().show(self);
    }
}
