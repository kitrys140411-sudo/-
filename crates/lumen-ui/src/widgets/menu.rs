//! Menus drawn inside the window as overlays: context menus
//! ([`Response::context_menu`]), menu buttons and dropdowns. A menu closes
//! when an item is chosen, on Escape, or on a press outside it (which does
//! not reach what is below). Up/Down/Home/End move the highlight, Enter
//! chooses.

use std::hash::Hash;

use crate::anim::SpringParams;
use crate::geom::Vec2;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::{Button as MouseButton, Key, Shortcut};
use crate::layout::{Layout, Size};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Anchor, Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

#[derive(Default)]
struct MenuState {
    open: bool,
    anchor: Option<Anchor>,
    min_width: f32,
    /// Highlighted item (keyboard or pointer).
    highlight: Option<usize>,
    /// Number of enabled items last frame (keyboard wrap-around).
    count: usize,
    /// Pointer position last frame: hovering highlights only when the
    /// pointer moves, so a resting pointer does not undo keyboard moves.
    pointer: Option<Vec2>,
}

/// One menu entry.
#[must_use = "a MenuItem does nothing until added with menu.add"]
pub struct MenuItem {
    label: String,
    icon: Option<Icon>,
    shortcut: Option<String>,
    checked: Option<bool>,
    enabled: bool,
    destructive: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> MenuItem {
        MenuItem { label: label.into(), icon: None, shortcut: None, checked: None, enabled: true, destructive: false }
    }

    pub fn icon(mut self, icon: Icon) -> MenuItem {
        self.icon = Some(icon);
        self
    }

    /// Show the keyboard shortcut (the app handles it with `ui.shortcut`).
    pub fn shortcut(mut self, s: Shortcut) -> MenuItem {
        self.shortcut = Some(s.label());
        self
    }

    /// A check mark in front (toggle items, the selected choice).
    pub fn checked(mut self, on: bool) -> MenuItem {
        self.checked = Some(on);
        self
    }

    pub fn enabled(mut self, on: bool) -> MenuItem {
        self.enabled = on;
        self
    }

    /// Colored as a dangerous action.
    pub fn destructive(mut self) -> MenuItem {
        self.destructive = true;
        self
    }
}

/// Builds the items of an open menu.
pub struct MenuUi<'a> {
    pub ui: &'a mut Ui,
    menu: Id,
    index: usize,
    highlight: Option<usize>,
    activate: Option<usize>,
    chosen: bool,
    track_hover: bool,
}

impl MenuUi<'_> {
    /// Add an item; true when it was chosen (the menu then closes).
    pub fn add(&mut self, item: MenuItem) -> bool {
        let ui = &mut *self.ui;
        let id = self.menu.with(("item", &item.label));
        let r = if item.enabled { ui.interact(id) } else { Response::default() };
        let index = self.index;
        if item.enabled {
            self.index += 1;
            if r.hovered && self.track_hover {
                self.highlight = Some(index);
            }
        }
        let lit = item.enabled && self.highlight == Some(index);
        let chosen = item.enabled && (r.clicked || self.activate == Some(index));
        let t = ui.theme().clone();
        let hl = ui.animate_bool(id.with("hl"), lit);
        let fg = if item.destructive { t.danger } else { t.text };
        let mut n = ui.widget_node(id, "menuitem", Some(&item.label), Layout::row().size(Size::FILL, Size::Px(32.0)).padding((0.0, 10.0)).gap(10.0), if item.enabled { Sense::CLICK } else { Sense::NONE });
        n.deco.radius = t.radius;
        n.deco.bg = t.control_hover.fade(hl);
        if !item.enabled {
            n.opacity = 0.45;
        }
        ui.add_container(n, |ui| {
            let lead = match (item.checked, item.icon) {
                (Some(true), _) => Some(Icon::Check),
                (Some(false), _) => None,
                (None, i) => i,
            };
            let mut i = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
            if let Some(icon) = lead {
                i.visual = Visual::Icon { icon, size: 16.0, color: if item.checked == Some(true) { t.accent } else { fg } };
            }
            if lead.is_some() || item.checked.is_some() {
                ui.add_node(i);
            }
            let mut l = Node::new(Layout::default().width(Size::FILL));
            l.measure = Measure::Text { text: item.label.clone(), style: TextStyle::new(t.font_size), wrap: Wrap::Ellipsize };
            l.visual = Visual::Text { color: fg, align: TextAlign::Start };
            ui.add_node(l);
            if let Some(s) = &item.shortcut {
                let mut k = Node::new(Layout::default());
                k.measure = Measure::Text { text: s.clone(), style: TextStyle::new(t.size_small()), wrap: Wrap::None };
                k.visual = Visual::Text { color: t.text_dim, align: TextAlign::End };
                ui.add_node(k);
            }
        });
        if chosen {
            self.chosen = true;
            ui.log(&format!("menu {}", item.label));
        }
        chosen
    }

    /// A plain item; true when chosen.
    pub fn item(&mut self, label: &str) -> bool {
        self.add(MenuItem::new(label))
    }

    /// An item with an icon; true when chosen.
    pub fn item_icon(&mut self, icon: Icon, label: &str) -> bool {
        self.add(MenuItem::new(label).icon(icon))
    }

    /// A hairline between groups of items.
    pub fn separator(&mut self) {
        let t = self.ui.theme().clone();
        let mut n = Node::new(Layout::default().size(Size::FILL, Size::Px(1.0)));
        n.deco.bg = t.separator;
        let wrap = Node::new(Layout::column().width(Size::FILL).padding((4.0, 6.0)));
        self.ui.add_container(wrap, |ui| {
            ui.add_node(n);
        });
    }

    /// A small dim heading above a group.
    pub fn heading(&mut self, text: &str) {
        let t = self.ui.theme().clone();
        let mut l = Node::new(Layout::default().padding((6.0, 10.0, 4.0, 10.0)));
        l.measure = Measure::Text { text: text.to_string(), style: TextStyle::new(t.size_small()).weight(Weight::SemiBold), wrap: Wrap::Ellipsize };
        l.visual = Visual::Text { color: t.text_dim, align: TextAlign::Start };
        self.ui.add_node(l);
    }
}

/// Open the menu `id` at `anchor` (from a click handler).
pub(crate) fn open_menu(ui: &mut Ui, id: Id, anchor: Anchor, min_width: f32, keyboard: bool) {
    let st = ui.state::<MenuState>(id);
    st.open = true;
    st.anchor = Some(anchor);
    st.min_width = min_width;
    st.highlight = if keyboard { Some(0) } else { None };
    ui.request_repaint();
}

pub(crate) fn is_open(ui: &mut Ui, id: Id) -> bool {
    ui.state::<MenuState>(id).open
}

pub(crate) fn close_menu(ui: &mut Ui, id: Id) {
    ui.state::<MenuState>(id).open = false;
}

/// Build the menu `id` if it is open; returns true when an item was chosen.
pub(crate) fn show_menu(ui: &mut Ui, id: Id, f: impl FnOnce(&mut MenuUi)) -> bool {
    let pointer = ui.pointer();
    let (open, anchor, min_width, mut highlight, count, pointer_moved) = {
        let st = ui.state::<MenuState>(id);
        let moved = st.pointer != pointer;
        st.pointer = pointer;
        (st.open, st.anchor, st.min_width, st.highlight, st.count, moved)
    };
    let Some(anchor) = anchor.filter(|_| open) else {
        // Reset the appear animation for the next opening.
        ui.forget(id.with("anim"));
        return false;
    };
    let t = ui.theme().clone();
    // Presses outside close the menu and go no further.
    let dismiss = id.with("dismiss");
    let mut d = Node::new(Layout::default().size(Size::FILL, Size::FILL));
    d.id = Some(dismiss);
    d.sense = Sense { click: true, blocking: true, ..Sense::NONE };
    ui.overlay(dismiss, Anchor::Fill, false, d, |_| {});
    let pressed_outside = [MouseButton::Left, MouseButton::Right, MouseButton::Middle].iter().any(|&b| ui.input.press(dismiss, b).is_some());
    let appear = ui.animate_from(id.with("anim"), 0.0, 1.0, SpringParams::POPOVER);
    let mut body = Node::new(Layout::column().padding(6.0).gap(2.0).min_width(min_width.max(180.0)).max_width(360.0));
    body.id = Some(id);
    body.sense = Sense::BLOCK;
    body.kind = "menu";
    body.label = Some("menu".into());
    body.deco.bg = t.surface;
    body.deco.radius = t.radius_large - 2.0;
    body.deco.border = t.border;
    body.deco.border_width = 1.0;
    body.deco.shadow = Some((24.0, 8.0, t.shadow.with_alpha(0.45)));
    body.opacity = appear.clamp(0.0, 1.0);
    body.paint_offset = Vec2::new(0.0, (1.0 - appear) * -6.0);
    let mut escape = false;
    let mut activate = None;
    let mut keyboard_moved = false;
    let (_, chosen) = ui.overlay(id, anchor, true, body, |ui| {
        // Keys (this menu owns the keyboard while open), in order.
        let n = count.max(1);
        for k in ui.keys() {
            if k.mods.any() {
                continue;
            }
            let to = match k.key {
                Key::Down => Some(highlight.map_or(0, |h| (h + 1) % n)),
                Key::Up => Some(highlight.map_or(n - 1, |h| (h + n - 1) % n)),
                Key::Home => Some(0),
                Key::End => Some(n - 1),
                Key::Enter | Key::Char(' ') => {
                    ui.take_key(k.key, k.mods);
                    activate = highlight;
                    break;
                }
                _ => None,
            };
            if let Some(to) = to {
                ui.take_key(k.key, k.mods);
                highlight = Some(to);
                keyboard_moved = true;
            }
        }
        escape = ui.key_pressed(Key::Escape);
        let track_hover = pointer_moved && !keyboard_moved;
        let mut m = MenuUi { ui, menu: id, index: 0, highlight, activate, chosen: false, track_hover };
        f(&mut m);
        let count = m.index;
        let (hl, chosen) = (m.highlight, m.chosen);
        let st = ui.state::<MenuState>(id);
        st.count = count;
        st.highlight = hl;
        chosen
    });
    if chosen || escape || pressed_outside {
        close_menu(ui, id);
        ui.request_repaint();
    }
    chosen
}

impl Response {
    /// A context menu for this widget: opens on right click (or the Menu
    /// key while focused) at the pointer; `f` builds the items.
    pub fn context_menu(&self, ui: &mut Ui, f: impl FnOnce(&mut MenuUi)) -> bool {
        let id = self.id.with("context-menu");
        if self.secondary_clicked {
            let at = self.pointer.unwrap_or(self.rect.center());
            open_menu(ui, id, Anchor::At(at), 0.0, false);
        }
        show_menu(ui, id, f)
    }
}

impl Ui {
    /// An icon button that opens a menu below it.
    pub fn menu_button(&mut self, icon: Icon, key: impl Hash, f: impl FnOnce(&mut MenuUi)) -> Response {
        let r = super::Button::icon_only(icon).id(("menu-button", key)).toggled(false).show(self);
        let id = r.id.with("menu");
        let open = is_open(self, id);
        if r.clicked {
            if open {
                close_menu(self, id);
            } else {
                let keyboard = r.focused && self.focus_visible();
                open_menu(self, id, Anchor::Below(r.rect), 0.0, keyboard);
            }
        }
        show_menu(self, id, f);
        r
    }

    /// A dropdown choosing one of `options` into `selected`.
    pub fn dropdown(&mut self, name: &str, selected: &mut usize, options: &[&str]) -> Response {
        let id = self.make_id(("dropdown", name));
        let mut r = self.interact(id);
        if r.focused && (self.key_pressed(Key::Enter) || self.key_pressed(Key::SPACE) || self.key_pressed(Key::Down)) {
            r.clicked = true;
        }
        let menu = id.with("menu");
        if r.clicked {
            if is_open(self, menu) {
                close_menu(self, menu);
            } else {
                let keyboard = r.focused && self.focus_visible();
                open_menu(self, menu, Anchor::Below(r.rect), r.rect.w, keyboard);
                self.state::<MenuState>(menu).highlight = keyboard.then_some(*selected);
            }
        }
        let t = self.theme().clone();
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let open = is_open(self, menu);
        let press = self.animate_bool(id.with("p"), r.pressed || open);
        let current = options.get(*selected).copied().unwrap_or("").to_string();
        let mut n = self.widget_node(id, "dropdown", Some(name), Layout::row().height(Size::Px(32.0)).padding((0.0, 10.0, 0.0, 12.0)).gap(8.0).min_width(140.0), Sense::CLICK);
        n.focusable = true;
        n.deco.bg = t.control.mix(t.control_hover, hover).mix(t.control_active, press);
        n.deco.radius = t.radius;
        if self.test.enabled() {
            n.value = Some(current.clone());
        }
        self.add_container(n, |ui| {
            let mut l = Node::new(Layout::default().width(Size::FILL));
            l.measure = Measure::Text { text: current, style: TextStyle::new(t.font_size), wrap: Wrap::Ellipsize };
            l.visual = Visual::Text { color: t.text, align: TextAlign::Start };
            ui.add_node(l);
            let mut i = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
            i.visual = Visual::Icon { icon: Icon::ChevronDown, size: 16.0, color: t.text_dim };
            ui.add_node(i);
        });
        let mut picked = None;
        let sel = *selected;
        show_menu(self, menu, |m| {
            for (i, o) in options.iter().enumerate() {
                if m.add(MenuItem::new(*o).checked(i == sel)) {
                    picked = Some(i);
                }
            }
        });
        if let Some(i) = picked {
            if i != *selected {
                *selected = i;
                r.changed = true;
                self.log(&format!("select {name} {}", options[i]));
            }
        }
        r
    }
}
