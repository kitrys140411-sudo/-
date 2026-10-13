//! Feedback: tooltips, toasts (transient messages at the bottom of the
//! window) and infobars (inline messages with an optional action).

use std::time::{Duration, Instant};

use crate::anim::SpringParams;
use crate::color::Color;
use crate::geom::Vec2;
use crate::icons::Icon;
use crate::id::Id;
use crate::layout::{Layout, Size};
use crate::text::{TextAlign, TextStyle, Wrap};
use crate::tree::{Anchor, Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

use super::Button;

const TOOLTIP_DELAY: Duration = Duration::from_millis(600);
const TOAST_TIME: Duration = Duration::from_millis(3500);

#[derive(Default)]
struct TooltipState {
    since: Option<Instant>,
}

impl Response {
    /// Show `text` in a tooltip after hovering this widget a moment.
    pub fn tooltip(&self, ui: &mut Ui, text: &str) {
        let id = self.id.with("tooltip");
        let now = ui.now();
        let hovering = self.hovered && !self.pressed;
        let since = {
            let st = ui.state::<TooltipState>(id);
            if !hovering {
                st.since = None;
            } else if st.since.is_none() {
                st.since = Some(now);
            }
            st.since
        };
        let Some(since) = since else {
            ui.forget(id.with("in"));
            return;
        };
        let waited = now.duration_since(since);
        if waited < TOOLTIP_DELAY {
            ui.request_repaint_after(TOOLTIP_DELAY - waited);
            return;
        }
        let t = ui.theme().clone();
        let appear = ui.animate_from(id.with("in"), 0.0, 1.0, SpringParams::FAST);
        let mut n = Node::new(Layout::default().padding((5.0, 10.0)).max_width(320.0));
        n.measure = Measure::Text { text: text.to_string(), style: TextStyle::new(t.size_small()), wrap: Wrap::Wrap { max_lines: 4 } };
        n.visual = Visual::Text { color: t.text, align: TextAlign::Start };
        n.kind = "tooltip";
        n.label = Some(text.to_string());
        n.deco.bg = t.surface.with_alpha(0.98);
        n.deco.radius = 6.0;
        n.deco.border = t.border;
        n.deco.border_width = 1.0;
        n.deco.shadow = Some((12.0, 4.0, t.shadow.with_alpha(0.35)));
        n.opacity = appear.clamp(0.0, 1.0);
        ui.overlay(id, Anchor::Above(self.rect), false, n, |_| {});
    }
}

#[derive(Clone)]
struct Toast {
    text: String,
    action: Option<String>,
    born: Instant,
    serial: u64,
}

#[derive(Default)]
struct Toasts {
    current: Option<Toast>,
    serial: u64,
    clicked: Option<String>,
}

const TOASTS: Id = Id(0x7_0a57);

/// How serious an infobar's message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl Ui {
    /// Show a short message at the bottom of the window for a few seconds
    /// (a newer toast replaces the current one).
    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast_with_action(text, None::<String>);
    }

    /// A toast with an action button (e.g. "Undo"); check
    /// [`Ui::toast_action`] for clicks.
    pub fn toast_with_action(&mut self, text: impl Into<String>, action: Option<impl Into<String>>) {
        let now = self.now();
        let text = text.into();
        self.log(&format!("toast {text}"));
        let st = self.state::<Toasts>(TOASTS);
        st.serial += 1;
        st.current = Some(Toast { text, action: action.map(Into::into), born: now, serial: st.serial });
        self.request_repaint();
    }

    /// True once when the toast action `label` was clicked.
    pub fn toast_action(&mut self, label: &str) -> bool {
        let st = self.state::<Toasts>(TOASTS);
        if st.clicked.as_deref() == Some(label) {
            st.clicked = None;
            return true;
        }
        false
    }

    /// Draw the current toast (called by the window frame after the view).
    pub(crate) fn show_toasts(&mut self) {
        let now = self.now();
        let Some(toast) = self.state::<Toasts>(TOASTS).current.clone() else { return };
        let age = now.duration_since(toast.born);
        if age > TOAST_TIME + Duration::from_millis(300) {
            self.state::<Toasts>(TOASTS).current = None;
            return;
        }
        let id = TOASTS.with(toast.serial);
        let leaving = age > TOAST_TIME;
        let appear = self.animate_from(id.with("in"), 0.0, if leaving { 0.0 } else { 1.0 }, SpringParams::POPOVER);
        self.request_repaint_after(if leaving { Duration::from_millis(16) } else { TOAST_TIME - age });
        if leaving {
            self.request_repaint();
        }
        let t = self.theme().clone();
        let mut n = Node::new(Layout::row().padding((8.0, 10.0, 8.0, 18.0)).gap(12.0).min_height(44.0).max_width(520.0));
        n.kind = "toast";
        n.label = Some(toast.text.clone());
        n.deco.bg = t.surface.with_alpha(1.0).mix(Color::WHITE, 0.04);
        n.deco.radius = 22.0;
        n.deco.border = t.border;
        n.deco.border_width = 1.0;
        n.deco.shadow = Some((24.0, 8.0, t.shadow.with_alpha(0.45)));
        n.opacity = appear.clamp(0.0, 1.0);
        n.paint_offset = Vec2::new(0.0, (1.0 - appear) * 20.0);
        n.id = Some(id);
        n.sense = Sense::BLOCK;
        let mut clicked = None;
        self.overlay(id, Anchor::Bottom(24.0), false, n, |ui| {
            let mut l = Node::new(Layout::default());
            l.measure = Measure::Text { text: toast.text.clone(), style: TextStyle::new(t.font_size), wrap: Wrap::Wrap { max_lines: 2 } };
            l.visual = Visual::Text { color: t.text, align: TextAlign::Start };
            ui.add_node(l);
            if let Some(a) = &toast.action {
                if Button::new(a.clone()).flat().show(ui).clicked {
                    clicked = Some(a.clone());
                }
            }
        });
        if let Some(a) = clicked {
            let st = self.state::<Toasts>(TOASTS);
            st.clicked = Some(a);
            st.current = None;
        }
    }

    /// An inline message across the content (e.g. "Folder is read-only"),
    /// with an optional action button; `.clicked` when the action was.
    pub fn infobar(&mut self, severity: Severity, text: &str, action: Option<&str>) -> Response {
        let t = self.theme().clone();
        let (color, icon) = match severity {
            Severity::Info => (t.accent, Icon::Info),
            Severity::Success => (t.success, Icon::Check),
            Severity::Warning => (t.warning, Icon::Warning),
            Severity::Error => (t.danger, Icon::Error),
        };
        let mut out = Response::default();
        let mut n = Node::new(Layout::row().width(Size::FILL).padding((8.0, 12.0)).gap(12.0).min_height(44.0));
        n.kind = "infobar";
        n.label = Some(text.to_string());
        n.deco.bg = t.window_bg.over(color.with_alpha(0.14));
        n.deco.border = color.with_alpha(0.35);
        n.deco.border_width = 1.0;
        n.deco.radius = t.radius + 2.0;
        self.add_container(n, |ui| {
            let mut i = Node::new(Layout::default().size(Size::Px(18.0), Size::Px(18.0)));
            i.visual = Visual::Icon { icon, size: 18.0, color };
            ui.add_node(i);
            let mut l = Node::new(Layout::default().width(Size::FILL));
            l.measure = Measure::Text { text: text.to_string(), style: TextStyle::new(t.font_size), wrap: Wrap::Wrap { max_lines: 0 } };
            l.visual = Visual::Text { color: t.text, align: TextAlign::Start };
            ui.add_node(l);
            if let Some(a) = action {
                out = Button::new(a).show(ui);
            }
        });
        out
    }
}
