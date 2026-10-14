//! Running an app's view without a window: build, lay out and interact in
//! process (no compositor, no GPU), with a simulated clock. For unit tests of
//! apps and widgets:
//!
//! ```no_run
//! use lumen_ui::{App, Harness, Ui};
//!
//! struct Counter(u32);
//! impl App for Counter {
//!     fn view(&mut self, ui: &mut Ui) {
//!         ui.label(format!("Count: {}", self.0));
//!         if ui.button("Add").clicked {
//!             self.0 += 1;
//!         }
//!     }
//! }
//!
//! let mut app = Counter(0);
//! let mut h = Harness::new(400.0, 300.0).unwrap();
//! h.frame(&mut app);
//! h.click("Add");
//! h.frame(&mut app);
//! assert_eq!(app.0, 1);
//! assert!(h.widget("Count: 1").is_some());
//! ```
//!
//! Widgets are found by label (the text of labels and buttons, the name of
//! inputs, switches, sliders…), like in the headless window tests.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::geom::{Rect, Vec2};
use crate::input::{Button, Event, Key, KeyPress, Mods};
use crate::text::Fonts;
use crate::theme::Theme;
use crate::ui::Ui;
use crate::{App, Waker};

/// A widget as laid out in the last frame.
#[derive(Clone, Debug, PartialEq)]
pub struct WidgetInfo {
    pub kind: &'static str,
    pub label: String,
    /// Visible part, logical window coordinates.
    pub rect: Rect,
    pub value: Option<String>,
    pub focused: bool,
    pub hovered: bool,
    /// Overlay layer (0 is the window content; menus, dialogs, tooltips and
    /// toasts are drawn above it in higher layers).
    pub layer: u32,
}

/// Kinds of overlay surfaces that hide what is below them.
const SURFACES: [&str; 4] = ["menu", "dialog", "toast", "tooltip"];

pub struct Harness {
    ui: Ui,
    events: VecDeque<Event>,
    clock: Instant,
    /// The app asked to close (and agreed).
    pub closed: bool,
}

impl Harness {
    /// A `width` × `height` (logical) window with the default theme.
    pub fn new(width: f32, height: f32) -> Result<Harness, String> {
        let theme = Theme::default();
        let fonts = Fonts::new(&theme.font_ui, &theme.font_mono)?;
        let mut ui = Ui::new(theme, fonts, Waker::none());
        ui.window.size = Vec2::new(width, height);
        ui.test.capture();
        Ok(Harness { ui, events: VecDeque::new(), clock: Instant::now(), closed: false })
    }

    pub fn ui(&mut self) -> &mut Ui {
        &mut self.ui
    }

    /// Whether a font family is installed (tests that compare text
    /// geometry skip without Inter).
    pub fn has_font(&self, family: &str) -> bool {
        self.ui.fonts.db.faces.iter().any(|f| f.family == family)
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.ui.window.size = Vec2::new(width, height);
    }

    /// One frame: handle queued input, build, lay out. The clock advances
    /// 16 ms per frame.
    pub fn frame(&mut self, app: &mut dyn App) {
        self.clock += Duration::from_millis(16);
        if self.ui.run_frame(app, &mut self.events, self.clock) {
            self.closed = true;
        }
    }

    /// Frames until animations have settled (at most 3 simulated seconds).
    pub fn settle(&mut self, app: &mut dyn App) {
        for _ in 0..190 {
            self.frame(app);
            if !self.ui.animating && self.events.is_empty() {
                break;
            }
        }
    }

    /// Let simulated time pass (tooltips, toasts, caret blink).
    pub fn advance(&mut self, d: Duration) {
        self.clock += d;
    }

    /// Event lines logged since the last call (`click Add`, `toggle wifi on`…).
    pub fn log(&mut self) -> Vec<String> {
        self.ui.test.take_captured()
    }

    /// Every labelled widget visible in the last frame, in paint order.
    pub fn widgets(&self) -> Vec<WidgetInfo> {
        let tree = &self.ui.tree;
        let size = self.ui.window.size;
        let mut out = Vec::new();
        let mut stack: Vec<(u32, Rect, u32, f32)> = tree.roots.iter().rev().map(|r| (r.node, Rect::new(0.0, 0.0, size.x, size.y), r.layer, 1.0)).collect();
        while let Some((i, clip, layer, alpha)) = stack.pop() {
            let n = &tree.nodes[i as usize];
            let r = n.rect.intersect(&clip);
            let alpha = alpha * n.opacity;
            if let Some(label) = &n.label {
                if !n.kind.is_empty() && !r.is_empty() && alpha > 0.0 {
                    out.push(WidgetInfo {
                        kind: n.kind,
                        label: label.clone(),
                        rect: r,
                        value: n.value.clone(),
                        focused: n.id.is_some() && n.id == self.ui.input.focus,
                        hovered: n.id.is_some() && n.id == self.ui.input.hovered,
                        layer,
                    });
                }
            }
            let clip = if n.clip { clip.intersect(&n.rect) } else { clip };
            let kids: Vec<u32> = tree.children(i).collect();
            for c in kids.into_iter().rev() {
                stack.push((c, clip, layer, alpha));
            }
        }
        out
    }

    /// The topmost visible widget labelled `label`.
    pub fn widget(&self, label: &str) -> Option<WidgetInfo> {
        self.widgets().into_iter().rev().find(|w| w.label == label)
    }

    /// Where a user would click `label`: the center of its visible part.
    /// Panics when there is no such widget, or when a menu, dialog, toast or
    /// tooltip of a higher layer covers that point (the click would land on
    /// the overlay, which is never what a test means).
    fn center(&self, label: &str) -> Vec2 {
        let all = self.widgets();
        let Some(w) = all.iter().rev().find(|w| w.label == label) else {
            panic!("no widget {label:?}; visible: {:?}", all.iter().map(|w| &w.label).collect::<Vec<_>>());
        };
        let p = w.rect.center();
        if let Some(cover) = all.iter().find(|o| o.layer > w.layer && SURFACES.contains(&o.kind) && o.rect.contains(p)) {
            panic!("widget {label:?} at {p:?} is covered by the {} {:?}", cover.kind, cover.label);
        }
        p
    }

    pub fn move_to(&mut self, p: Vec2) {
        self.events.push_back(Event::PointerMove(p));
    }

    pub fn press(&mut self, p: Vec2, button: Button, pressed: bool) {
        self.events.push_back(Event::PointerButton { pos: p, button, pressed });
    }

    /// Click (press and release) the widget labelled `label`.
    pub fn click(&mut self, label: &str) {
        let p = self.center(label);
        self.click_at(p, Button::Left);
    }

    pub fn right_click(&mut self, label: &str) {
        let p = self.center(label);
        self.click_at(p, Button::Right);
    }

    pub fn double_click(&mut self, label: &str) {
        let p = self.center(label);
        self.click_at(p, Button::Left);
        self.click_at(p, Button::Left);
    }

    pub fn click_at(&mut self, p: Vec2, button: Button) {
        self.move_to(p);
        self.press(p, button, true);
        self.press(p, button, false);
    }

    /// Press and drag from one point to another (in a few steps).
    pub fn drag(&mut self, from: Vec2, to: Vec2) {
        self.move_to(from);
        self.press(from, Button::Left, true);
        for k in 1..=4 {
            self.move_to(from + (to - from) * (k as f32 / 4.0));
        }
        self.press(to, Button::Left, false);
    }

    pub fn hover(&mut self, label: &str) {
        let p = self.center(label);
        self.move_to(p);
    }

    /// Scroll over the widget labelled `label` by `dy` pixels (wheel).
    pub fn scroll(&mut self, label: &str, dy: f32) {
        let p = self.center(label);
        self.events.push_back(Event::Scroll { pos: p, delta: Vec2::new(0.0, dy), wheel: true, kinetic_end: false });
    }

    /// Scroll at a point by `dy` pixels (wheel).
    pub fn scroll_at(&mut self, p: Vec2, dy: f32) {
        self.events.push_back(Event::Scroll { pos: p, delta: Vec2::new(0.0, dy), wheel: true, kinetic_end: false });
    }

    pub fn key(&mut self, key: Key, mods: Mods) {
        self.ui.input.mods = mods;
        self.events.push_back(Event::Key(KeyPress { key, mods, text: None, repeat: false }));
    }

    /// Type text (one key event per character).
    pub fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let mods = if c.is_uppercase() { Mods::SHIFT } else { Mods::NONE };
            let key = Key::Char(c.to_lowercase().next().unwrap_or(c));
            self.events.push_back(Event::Key(KeyPress { key, mods, text: Some(c.to_string()), repeat: false }));
        }
    }

    /// Deliver clipboard text to the widget that asked for it last.
    pub fn paste(&mut self, text: &str) {
        if let Some((target, _)) = self.ui.out.paste.take() {
            self.events.push_back(Event::Paste { target, text: text.to_string() });
        }
    }

    /// Text the app copied last (and forget it).
    pub fn copied(&mut self) -> Option<String> {
        self.ui.out.copy.take().map(|c| c.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::{Button as ButtonW, ListView, Selection};
    use crate::{Icon, Shortcut};

    fn harness() -> Option<Harness> {
        let h = Harness::new(640.0, 480.0).ok()?;
        if !h.has_font("Inter") {
            eprintln!("Inter not installed; skipping");
            return None;
        }
        Some(h)
    }

    /// An app from a closure.
    struct F<S, V: FnMut(&mut S, &mut Ui)>(S, V);

    impl<S, V: FnMut(&mut S, &mut Ui)> App for F<S, V> {
        fn view(&mut self, ui: &mut Ui) {
            (self.1)(&mut self.0, ui)
        }
    }

    #[test]
    fn button_click_counts() {
        let Some(mut h) = harness() else { return };
        let mut app = F(0u32, |n: &mut u32, ui: &mut Ui| {
            ui.label(format!("Count: {n}"));
            if ui.button("Add").clicked {
                *n += 1;
            }
        });
        h.frame(&mut app);
        h.frame(&mut app);
        h.click("Add");
        h.frame(&mut app);
        h.click("Add");
        h.frame(&mut app);
        assert_eq!(app.0, 2);
        assert!(h.widget("Count: 2").is_some());
        assert_eq!(h.log(), vec!["click Add", "click Add"]);
    }

    #[test]
    fn two_clicks_in_one_frame_hit_the_updated_ui() {
        let Some(mut h) = harness() else { return };
        // The second button only exists after the first was clicked.
        let mut app = F(0u32, |n: &mut u32, ui: &mut Ui| {
            if *n == 0 && ui.button("First").clicked {
                *n = 1;
            }
            if *n >= 1 && ui.button("Second").clicked {
                *n = 2;
            }
        });
        h.frame(&mut app);
        let first = h.widget("First").unwrap().rect.center();
        h.click_at(first, Button::Left);
        // "Second" takes the place of "First": the next click lands on it.
        h.click_at(first, Button::Left);
        h.frame(&mut app);
        assert_eq!(app.0, 2);
    }

    #[test]
    fn typing_into_an_input() {
        let Some(mut h) = harness() else { return };
        let mut app = F(String::new(), |s: &mut String, ui: &mut Ui| {
            ui.text_input("name", s);
        });
        h.frame(&mut app);
        h.click("name");
        h.frame(&mut app);
        h.type_text("Hello wörld");
        h.frame(&mut app);
        assert_eq!(app.0, "Hello wörld");
        h.key(Key::Backspace, Mods::CTRL);
        h.frame(&mut app);
        assert_eq!(app.0, "Hello ");
        h.key(Key::Char('a'), Mods::CTRL);
        h.key(Key::Char('c'), Mods::CTRL);
        h.frame(&mut app);
        assert_eq!(h.copied().as_deref(), Some("Hello "));
        h.key(Key::Char('z'), Mods::CTRL);
        h.frame(&mut app);
        assert_eq!(app.0, "Hello wörld");
        h.key(Key::End, Mods::NONE);
        h.key(Key::Char('v'), Mods::CTRL);
        h.frame(&mut app);
        h.paste("!");
        h.frame(&mut app);
        assert_eq!(app.0, "Hello wörld!");
        assert_eq!(h.widget("name").unwrap().value.as_deref(), Some("Hello wörld!"));
    }

    #[test]
    fn switch_checkbox_radio_slider() {
        let Some(mut h) = harness() else { return };
        #[derive(Default)]
        struct S {
            on: bool,
            check: bool,
            choice: u8,
            v: f32,
        }
        let mut app = F(S::default(), |s: &mut S, ui: &mut Ui| {
            ui.switch("wifi", &mut s.on);
            ui.checkbox("Remember", &mut s.check);
            ui.radio("One", &mut s.choice, 1);
            ui.radio("Two", &mut s.choice, 2);
            ui.slider("volume", &mut s.v, 0.0..=100.0);
        });
        h.frame(&mut app);
        h.click("wifi");
        h.click("Remember");
        h.click("Two");
        h.frame(&mut app);
        assert!(app.0.on && app.0.check && app.0.choice == 2);
        assert_eq!(h.widget("wifi").unwrap().value.as_deref(), Some("on"));
        // Slider: click at 3/4 of its track, then keyboard.
        let r = h.widget("volume").unwrap().rect;
        h.click_at(Vec2::new(r.x + 9.0 + (r.w - 18.0) * 0.75, r.center().y), Button::Left);
        h.frame(&mut app);
        assert!((app.0.v - 75.0).abs() < 1.0, "{}", app.0.v);
        h.key(Key::Home, Mods::NONE);
        h.frame(&mut app);
        assert_eq!(app.0.v, 0.0);
    }

    #[test]
    fn tab_moves_focus_and_enter_activates() {
        let Some(mut h) = harness() else { return };
        let mut app = F(Vec::<&'static str>::new(), |log: &mut Vec<&'static str>, ui: &mut Ui| {
            for name in ["A", "B", "C"] {
                if ui.button(name).clicked {
                    log.push(name);
                }
            }
        });
        h.frame(&mut app);
        h.key(Key::Tab, Mods::NONE);
        h.frame(&mut app);
        assert!(h.widget("window-maximize").is_some());
        // Header buttons come first in the window; skip to the content.
        let order: Vec<String> = (0..5)
            .map(|_| {
                h.key(Key::Tab, Mods::NONE);
                h.frame(&mut app);
                h.widgets().into_iter().find(|w| w.focused).map(|w| w.label).unwrap_or_default()
            })
            .collect();
        assert!(order.windows(3).any(|w| w == ["A", "B", "C"]), "{order:?}");
        while h.widgets().into_iter().find(|w| w.focused).map(|w| w.label) != Some("B".into()) {
            h.key(Key::Tab, Mods::NONE);
            h.frame(&mut app);
        }
        h.key(Key::Enter, Mods::NONE);
        h.frame(&mut app);
        assert_eq!(app.0, vec!["B"]);
        h.key(Key::Tab, Mods::SHIFT);
        h.frame(&mut app);
        assert!(h.widget("A").unwrap().focused);
    }

    #[test]
    fn menus_open_choose_and_close() {
        let Some(mut h) = harness() else { return };
        let mut app = F(None::<&'static str>, |picked: &mut Option<&'static str>, ui: &mut Ui| {
            // "Behind" sits next to the menu button, outside the open menu.
            ui.row(|ui| {
                ui.menu_button(Icon::Menu, "main", |m| {
                    if m.item("Copy") {
                        *picked = Some("Copy");
                    }
                    if m.item("Paste") {
                        *picked = Some("Paste");
                    }
                });
                if ui.button("Behind").clicked {
                    *picked = Some("Behind");
                }
            });
        });
        h.frame(&mut app);
        h.click("menu");
        h.settle(&mut app);
        assert!(h.widget("Paste").is_some(), "menu open");
        h.click("Paste");
        h.settle(&mut app);
        assert_eq!(app.0, Some("Paste"));
        assert!(h.widget("Paste").is_none(), "closed after choosing");
        // Keyboard: open, Down twice, Enter.
        app.0 = None;
        h.click("menu");
        h.frame(&mut app);
        h.key(Key::Down, Mods::NONE);
        h.key(Key::Down, Mods::NONE);
        h.key(Key::Enter, Mods::NONE);
        h.settle(&mut app);
        assert_eq!(app.0, Some("Paste"));
        // A click outside closes without reaching what is below.
        app.0 = None;
        h.click("menu");
        h.settle(&mut app);
        h.click("Behind");
        h.settle(&mut app);
        assert_eq!(app.0, None);
        assert!(h.widget("Copy").is_none());
    }

    #[test]
    fn dropdown_and_context_menu() {
        let Some(mut h) = harness() else { return };
        let mut app = F((0usize, 0u32), |s: &mut (usize, u32), ui: &mut Ui| {
            ui.dropdown("size", &mut s.0, &["Small", "Medium", "Large"]);
            let r = ui.button("Target");
            r.context_menu(ui, |m| {
                if m.item("Rename") {
                    s.1 += 1;
                }
            });
        });
        h.frame(&mut app);
        h.click("size");
        h.settle(&mut app);
        h.click("Large");
        h.settle(&mut app);
        assert_eq!(app.0 .0, 2);
        assert_eq!(h.widget("size").unwrap().value.as_deref(), Some("Large"));
        h.right_click("Target");
        h.settle(&mut app);
        h.click("Rename");
        h.frame(&mut app);
        assert_eq!(app.0 .1, 1);
    }

    #[test]
    fn dialogs_trap_focus_and_cancel_with_escape() {
        let Some(mut h) = harness() else { return };
        #[derive(Default)]
        struct S {
            open: bool,
            name: String,
            result: Option<bool>,
            behind: u32,
        }
        let mut app = F(S::default(), |s: &mut S, ui: &mut Ui| {
            if ui.button("Rename…").clicked {
                s.open = true;
                s.name = "old".into();
            }
            if ui.button("Behind").clicked {
                s.behind += 1;
            }
            if let Some(r) = ui.prompt("rename", &mut s.open, "Rename", &mut s.name, "Rename") {
                s.result = Some(r);
            }
        });
        h.frame(&mut app);
        h.click("Rename…");
        h.settle(&mut app);
        assert!(h.widget("rename-text").unwrap().focused, "focus moves into the dialog");
        h.click("Behind");
        h.frame(&mut app);
        assert_eq!(app.0.behind, 0, "the backdrop blocks the window");
        h.key(Key::Char('a'), Mods::CTRL);
        h.type_text("new");
        h.key(Key::Enter, Mods::NONE);
        h.settle(&mut app);
        assert_eq!((app.0.result, app.0.name.as_str(), app.0.open), (Some(true), "new", false));
        h.click("Rename…");
        h.settle(&mut app);
        h.key(Key::Escape, Mods::NONE);
        h.settle(&mut app);
        assert_eq!((app.0.result, app.0.open), (Some(false), false));
    }

    #[test]
    fn virtual_list_builds_only_visible_rows_and_selects() {
        let Some(mut h) = harness() else { return };
        let mut app = F((Selection::default(), None::<usize>), |s: &mut (Selection, Option<usize>), ui: &mut Ui| {
            let r = ListView::new("files", 10_000, 32.0).show(ui, &mut s.0, |ui, item| ui.label(format!("Row {}", item.index)));
            if r.activated.is_some() {
                s.1 = r.activated;
            }
        });
        h.settle(&mut app);
        let rows = h.widgets().iter().filter(|w| w.kind == "item").count();
        assert!(rows > 5 && rows < 30, "only visible rows are built: {rows}");
        h.click("Row 3");
        h.frame(&mut app);
        assert_eq!(app.0 .0.selected().collect::<Vec<_>>(), vec![3]);
        h.key(Key::Down, Mods::SHIFT);
        h.key(Key::Down, Mods::SHIFT);
        h.frame(&mut app);
        assert_eq!(app.0 .0.selected().collect::<Vec<_>>(), vec![3, 4, 5]);
        h.key(Key::End, Mods::NONE);
        h.settle(&mut app);
        assert!(h.widget("Row 9999").is_some(), "End scrolls to the last row");
        h.key(Key::Enter, Mods::NONE);
        h.frame(&mut app);
        assert_eq!(app.0 .1, Some(9999));
        h.double_click("Row 9998");
        h.frame(&mut app);
        assert_eq!(app.0 .1, Some(9998));
    }

    #[test]
    fn scrolling_moves_content() {
        let Some(mut h) = harness() else { return };
        let mut app = F((), |_: &mut (), ui: &mut Ui| {
            ui.scroll(|ui| {
                for i in 0..100 {
                    ui.label(format!("Line {i}"));
                }
            });
        });
        h.settle(&mut app);
        assert!(h.widget("Line 0").is_some() && h.widget("Line 60").is_none());
        h.scroll("Line 5", 1500.0);
        h.settle(&mut app);
        assert!(h.widget("Line 0").is_none() && h.widget("Line 60").is_some());
    }

    #[test]
    fn shortcuts_and_disabled_buttons() {
        let Some(mut h) = harness() else { return };
        let mut app = F(0u32, |n: &mut u32, ui: &mut Ui| {
            if ui.shortcut(Shortcut::ctrl('s')) {
                *n += 10;
            }
            if ButtonW::new("Off").enabled(false).show(ui).clicked {
                *n += 1;
            }
        });
        h.frame(&mut app);
        h.key(Key::Char('s'), Mods::CTRL);
        h.click("Off");
        h.frame(&mut app);
        assert_eq!(app.0, 10);
    }

    #[test]
    fn header_buttons_close_the_window() {
        let Some(mut h) = harness() else { return };
        let mut app = F((), |_: &mut (), ui: &mut Ui| ui.title("Test"));
        h.frame(&mut app);
        assert!(h.widget("Test").is_some(), "title shown in the header");
        h.click("window-close");
        h.frame(&mut app);
        assert!(h.closed);
    }
}
