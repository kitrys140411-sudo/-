//! Client-side window decorations: the header bar (app widgets at the start
//! and end, the centered title, maximize and close buttons), moving the
//! window by dragging the header, double-click to maximize. Rounded corners
//! and the shadow are the compositor's.

use std::time::{Duration, Instant};

use crate::geom::Rect;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::Button;
use crate::layout::{Layout, Size};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Sense, Visual, NONE};
use crate::ui::{Chrome, Ui};
use crate::App;

/// Header bar height (logical px).
pub const HEADER_HEIGHT: f32 = 48.0;
/// Width of the resize border inside the window edge.
pub const RESIZE_BORDER: f32 = 5.0;

pub(crate) const HEADER_ID: Id = Id(0x4845_4144_4552);

#[derive(Default)]
struct HeaderState {
    last_press: Option<Instant>,
}

/// Build the window frame around the app's view.
pub(crate) fn build(ui: &mut Ui, app: &mut dyn App) {
    let size = ui.window.size;
    let theme = ui.theme.clone();
    let mut root = Node::new(Layout::column().size(Size::Px(size.x), Size::Px(size.y)));
    root.deco.bg = theme.window_bg;
    let root = ui.begin_root(root);
    let mut chrome = Chrome { header: NONE, start: NONE, end: NONE, title: NONE, content: NONE };
    if ui.window.decorated && !ui.window.fullscreen {
        let mut header = Node::new(Layout::stack().size(Size::FILL, Size::Px(HEADER_HEIGHT)));
        header.id = Some(HEADER_ID);
        header.sense = Sense::CLICK;
        header.deco.bg = theme.header_bg;
        let (h, _) = ui.add_container(header, |ui| {
            let row = Node::new(Layout::row().size(Size::FILL, Size::FILL).padding((8.0, 8.0)).gap(8.0));
            ui.add_container(row, |ui| {
                chrome.start = ui.add_node(Node::new(Layout::row().gap(4.0)));
                ui.add_node(Node::new(Layout::default().width(Size::FILL)));
                chrome.end = ui.add_node(Node::new(Layout::row().gap(4.0)));
                ui.add_container(Node::new(Layout::row().gap(8.0).padding((0.0, 0.0, 0.0, 0.0))), |ui| {
                    let max_icon = if ui.window.maximized { Icon::Restore } else { Icon::Maximize };
                    if window_button(ui, "window-maximize", max_icon) {
                        ui.toggle_maximize();
                    }
                    if window_button(ui, "window-close", Icon::Close) {
                        ui.out.close = true;
                    }
                });
            });
            let mut title = Node::new(Layout::default());
            title.kind = "title";
            chrome.title = ui.add_node(title);
        });
        chrome.header = h;
        let mut line = Node::new(Layout::default().size(Size::FILL, Size::Px(1.0)));
        line.deco.bg = theme.separator.with_alpha(theme.separator.a * 1.4);
        ui.add_node(line);
    }
    let content = Node::new(Layout::column().size(Size::FILL, Size::FILL));
    chrome.content = ui.add_node(content);
    ui.chrome = chrome;
    let prev = ui.set_parent(chrome.content);
    app.view(ui);
    ui.set_parent(prev);
    ui.show_toasts();
    let _ = root;
    // The title is known only now.
    if chrome.title != NONE {
        let focused = ui.input.window_focused;
        let color = if focused { theme.text } else { theme.text_dim };
        let title = ui.title.clone();
        let n = ui.node_mut(chrome.title);
        n.label = Some(title.clone());
        n.measure = Measure::Text { text: title, style: TextStyle::new(theme.font_size).weight(Weight::SemiBold), wrap: Wrap::None };
        n.visual = Visual::Text { color, align: TextAlign::Center };
        header_interaction(ui);
    }
}

/// Dragging the header moves the window, double-clicking maximizes, right
/// click opens the compositor's window menu.
fn header_interaction(ui: &mut Ui) {
    if ui.input.press(HEADER_ID, Button::Left).is_some() {
        let now = ui.now;
        let st = ui.state::<HeaderState>(HEADER_ID);
        let double = st.last_press.is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
        st.last_press = if double { None } else { Some(now) };
        if double {
            ui.out.toggle_maximize = true;
        } else {
            ui.out.start_move = true;
        }
    }
    if let Some(c) = ui.input.press(HEADER_ID, Button::Right) {
        ui.out.window_menu = Some(c.pos);
    }
}

/// A round header button; returns true when clicked.
fn window_button(ui: &mut Ui, key: &'static str, icon: Icon) -> bool {
    let id = Id::ROOT.with(key);
    let r = ui.interact(id);
    let t = ui.theme.clone();
    let hover = ui.animate_bool(id.with("hover"), r.hovered);
    let press = ui.animate_bool(id.with("press"), r.pressed);
    let mut n = ui.widget_node(id, "button", Some(key), Layout::default().size(Size::Px(24.0), Size::Px(24.0)), Sense::CLICK);
    n.deco.bg = t.control.mix(t.control_hover, hover).mix(t.control_active, press);
    n.deco.radius = 12.0;
    n.visual = Visual::Icon { icon, size: 14.0, color: t.text };
    ui.add_node(n);
    if r.clicked {
        ui.log(&format!("click {key}"));
    }
    r.clicked
}

/// Center the title in the header, or in the space between the start and
/// end widgets when centering would overlap them (ellipsized if needed).
pub(crate) fn place_title(ui: &mut Ui) {
    let c = ui.chrome;
    if c.title == NONE || c.header == NONE {
        return;
    }
    let header = ui.tree.nodes[c.header as usize].rect;
    let start = ui.tree.nodes[c.start as usize].rect;
    let end_x = {
        let end = ui.tree.nodes[c.end as usize].rect;
        if end.w > 0.0 { end.x } else { header.right() - 8.0 - 24.0 * 2.0 - 8.0 - 8.0 }
    };
    let left = if start.w > 0.0 { start.right() + 12.0 } else { header.x + 12.0 };
    let right = end_x - 12.0;
    let avail = (right - left).max(0.0);
    let scale = ui.window.scale;
    let (text, style) = match &ui.tree.nodes[c.title as usize].measure {
        Measure::Text { text, style, .. } => (text.clone(), *style),
        _ => return,
    };
    let layout = ui.fonts.layout(&text, &style, Wrap::Ellipsize, Some(avail), scale);
    let w = layout.size.x.min(avail);
    let centered = header.x + (header.w - w) / 2.0;
    let x = if centered >= left && centered + w <= right { centered } else { left + (avail - w) / 2.0 };
    let h = layout.size.y;
    let n = &mut ui.tree.nodes[c.title as usize];
    n.rect = Rect::new(x.round(), (header.y + (header.h - h) / 2.0).round(), w, h);
    n.text = Some(layout);
}

/// Whether `p` (logical) is on the resize border, and which edge.
pub(crate) fn resize_edge(size: (f32, f32), x: f32, y: f32) -> Option<(bool, bool, bool, bool)> {
    let b = RESIZE_BORDER;
    let (l, r, t, bo) = (x < b, x >= size.0 - b, y < b, y >= size.1 - b);
    (l || r || t || bo).then_some((l, r, t, bo))
}

impl Ui {
    /// Add widgets at the start (left) of the header bar.
    pub fn header_start<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        self.in_slot(self.chrome.start, f)
    }

    /// Add widgets at the end (right, before the window buttons) of the
    /// header bar.
    pub fn header_end<R>(&mut self, f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        self.in_slot(self.chrome.end, f)
    }

    fn in_slot<R>(&mut self, slot: u32, f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
        if slot == NONE {
            return None;
        }
        let prev = self.set_parent(slot);
        let r = f(self);
        self.set_parent(prev);
        Some(r)
    }
}
