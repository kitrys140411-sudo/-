//! Single-line text input: cursor and selection (mouse: click, drag,
//! double-click word, triple-click all; keyboard: arrows, Ctrl+arrows by
//! word, Home/End, Shift to select), clipboard (Ctrl+C/X/V, middle-click
//! primary paste, a context menu), undo/redo (Ctrl+Z, Ctrl+Shift+Z/Ctrl+Y),
//! placeholder, leading icon, clear button and password mode.

use std::time::{Duration, Instant};

use crate::anim::SpringParams;
use crate::geom::{Rect, Vec2};
use crate::icons::Icon;
use crate::id::Id;
use crate::input::{Key, KeyPress, Mods};
use crate::layout::{Layout, Size, Sizing};
use crate::text::{Family, TextAlign, TextStyle, Wrap};
use crate::tree::{Node, Sense, Visual};
use crate::ui::{CursorIcon, Response, Ui};

use super::edit::{self, EditState};
use super::menu::MenuItem;

#[derive(Default)]
struct InputState {
    edit: EditState,
    /// Horizontal scroll of the text (logical px).
    scroll: f32,
    /// Caret blink phase origin (reset by every edit and move).
    blink: Option<Instant>,
    was_focused: bool,
}

#[must_use = "a TextInput does nothing until shown"]
pub struct TextInput<'a> {
    name: String,
    text: &'a mut String,
    placeholder: String,
    icon: Option<Icon>,
    password: bool,
    clear: bool,
    mono: bool,
    layout: Layout,
}

const BULLET: &str = "•";

impl<'a> TextInput<'a> {
    /// An input editing `text`; `name` identifies it (test map, ids).
    pub fn new(name: impl Into<String>, text: &'a mut String) -> TextInput<'a> {
        TextInput {
            name: name.into(),
            text,
            placeholder: String::new(),
            icon: None,
            password: false,
            clear: false,
            mono: false,
            layout: Layout::row().width(Size::FILL).min_width(160.0),
        }
    }

    /// Dim text shown while the input is empty.
    pub fn placeholder(mut self, p: impl Into<String>) -> TextInput<'a> {
        self.placeholder = p.into();
        self
    }

    /// A leading icon (e.g. `Icon::Search`).
    pub fn icon(mut self, icon: Icon) -> TextInput<'a> {
        self.icon = Some(icon);
        self
    }

    /// Show bullets instead of the text; copying is disabled.
    pub fn password(mut self) -> TextInput<'a> {
        self.password = true;
        self
    }

    /// A button that clears the text (search fields).
    pub fn clear_button(mut self) -> TextInput<'a> {
        self.clear = true;
        self
    }

    pub fn mono(mut self) -> TextInput<'a> {
        self.mono = true;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let id = ui.make_id(("input", &self.name));
        let area_id = id.with("area");
        let mut r = ui.interact(id);
        let mut st = std::mem::take(ui.state::<InputState>(id));
        let text = self.text;
        let before = text.clone();
        st.edit.clamp(text);
        let now = ui.now();
        let focused = r.focused;
        if focused && !st.was_focused {
            st.blink = Some(now);
            // Focus by keyboard selects everything, like other toolkits.
            if ui.focus_visible() && ui.press_pos(id).is_none() {
                st.edit.select_all(text);
            }
        }
        st.was_focused = focused;
        let t = ui.theme().clone();
        let mut style = TextStyle::new(t.font_size);
        if self.mono {
            style.family = Family::Mono;
        }
        let scale = ui.scale();
        let area = ui.prev_rect(area_id).unwrap_or_default();
        let password = self.password;

        // Mouse.
        let index_at = |ui: &mut Ui, text: &str, x: f32, scroll: f32| -> usize {
            let shown = display(text, password);
            let l = ui.fonts().layout(&shown, &style, Wrap::None, None, scale);
            let i = l.index_at(0, x - area.x + scroll);
            from_display(text, i, password)
        };
        if let Some((p, count)) = ui.press_pos(id) {
            let i = index_at(ui, text, p.x, st.scroll);
            match count {
                2 if !password => {
                    let (a, b) = edit::word_at(text, i);
                    st.edit.move_to(a, false);
                    st.edit.move_to(b, true);
                }
                2 | 3 => st.edit.select_all(text),
                _ => st.edit.move_to(i, ui.mods().shift),
            }
            st.blink = Some(now);
        }
        if r.dragging {
            if let Some(p) = ui.pointer() {
                let i = index_at(ui, text, p.x, st.scroll);
                st.edit.move_to(i, true);
            }
        }
        if (r.drag_released || r.double_clicked) && st.edit.has_selection() && !password {
            ui.set_primary_selection(st.edit.selected_text(text).to_string());
        }
        if r.middle_clicked {
            ui.request_paste(id, true);
        }
        let menu_id = id.with("menu");
        if r.secondary_clicked {
            let at = r.pointer.unwrap_or(r.rect.center());
            super::menu::open_menu(ui, menu_id, crate::tree::Anchor::At(at), 0.0, false);
            ui.request_focus(id);
        }

        // Keyboard.
        let mut submitted = false;
        if focused && ui.keys_allowed() {
            ui.mark_text_focus();
        }
        if focused {
            ui.consume_keys(|ui, k| {
                let used = key(ui, id, k, text, &mut st.edit, password);
                if k.key == Key::Enter && k.mods == Mods::NONE {
                    submitted = true;
                    return true;
                }
                if used {
                    st.blink = Some(now);
                }
                used
            });
        }
        if let Some(s) = ui.take_paste(id) {
            st.edit.insert(text, &s);
            st.blink = Some(now);
        }

        // Context menu.
        let has_sel = st.edit.has_selection();
        let mut action = None;
        super::menu::show_menu(ui, menu_id, |m| {
            if m.add(MenuItem::new("Cut").icon(Icon::Cut).enabled(has_sel && !password)) {
                action = Some("cut");
            }
            if m.add(MenuItem::new("Copy").icon(Icon::Copy).enabled(has_sel && !password)) {
                action = Some("copy");
            }
            if m.add(MenuItem::new("Paste").icon(Icon::Paste)) {
                action = Some("paste");
            }
            m.separator();
            if m.add(MenuItem::new("Select All")) {
                action = Some("all");
            }
        });
        match action {
            Some("cut") => {
                ui.copy_text(st.edit.selected_text(text).to_string());
                st.edit.delete_selection(text);
            }
            Some("copy") => ui.copy_text(st.edit.selected_text(text).to_string()),
            Some("paste") => ui.request_paste(id, false),
            Some("all") => st.edit.select_all(text),
            _ => {}
        }

        // Clear button.
        let clear_id = id.with("clear");
        let clear_r = ui.interact(clear_id);
        if self.clear && clear_r.clicked && !text.is_empty() {
            st.edit.select_all(text);
            st.edit.delete_selection(text);
            ui.request_focus(id);
        }

        let show_clear = self.clear && !text.is_empty();
        if *text != before {
            r.changed = true;
            ui.log(&format!("input {} {}", self.name, if password { "***".to_string() } else { text.clone() }));
        }
        if submitted {
            r.submitted = true;
            ui.log(&format!("submit {}", self.name));
        }

        // Keep the cursor visible.
        let shown = display(text, password);
        let layout = ui.fonts().layout(&shown, &style, Wrap::None, None, scale);
        let cur_d = to_display(text, st.edit.cursor, password);
        let anchor_d = to_display(text, st.edit.anchor, password);
        let cx = layout.x_at(0, cur_d);
        let aw = (area.w - 2.0).max(1.0);
        if area.w > 0.0 {
            if cx - st.scroll > aw {
                st.scroll = cx - aw;
            }
            if cx - st.scroll < 0.0 {
                st.scroll = cx;
            }
            st.scroll = st.scroll.clamp(0.0, (layout.size.x - aw).max(0.0));
        }

        // Caret blink: solid for a moment after input, then blinking, and
        // solid (no more frames) after a while without input.
        let mut caret = 0.0;
        if focused && ui.window_focused() {
            let since = now.duration_since(st.blink.unwrap_or(now)).as_secs_f32();
            if since > 12.0 {
                caret = 1.0;
            } else {
                let phase = since % 1.0;
                caret = if phase < 0.6 { 1.0 } else { 0.0 };
                let next = if phase < 0.6 { 0.6 - phase } else { 1.0 - phase };
                ui.request_repaint_after(Duration::from_secs_f32(next.max(0.01)));
            }
        }

        if r.hovered {
            ui.set_cursor(CursorIcon::Text);
        }
        let focus_t = ui.animate(id.with("focus"), if focused { 1.0 } else { 0.0 }, SpringParams::FAST);
        let hover = ui.animate_bool(id.with("h"), r.hovered);
        let mut layout_box = self.layout;
        layout_box.height = Size::Px(super::button::CONTROL_HEIGHT);
        layout_box.padding = (0.0, 10.0).into();
        layout_box.gap = 8.0;
        let mut n = ui.widget_node(id, "input", Some(&self.name), layout_box, Sense::DRAG);
        n.focusable = true;
        n.deco.bg = t.input_bg.mix(t.control_hover, hover * 0.5);
        n.deco.radius = t.radius;
        n.deco.border = t.separator.mix(t.focus_ring, focus_t);
        n.deco.border_width = 1.0 + focus_t;
        if ui.test.enabled() {
            n.value = Some(if password { "*".repeat(text.chars().count()) } else { text.clone() });
        }
        let placeholder = self.placeholder;
        let icon = self.icon;
        let (sa, sb) = (cur_d.min(anchor_d), cur_d.max(anchor_d));
        let scroll = st.scroll;
        let has_selection = sa != sb;
        let empty = text.is_empty();
        let clear_label = format!("{} clear", self.name);
        let ph_layout = (!placeholder.is_empty() && empty).then(|| ui.fonts().layout(&placeholder, &style, Wrap::Ellipsize, Some(area.w.max(1.0)), scale));
        ui.add_container(n, |ui| {
            if let Some(icon) = icon {
                let mut i = Node::new(Layout::default().size(Size::Px(16.0), Size::Px(16.0)));
                i.visual = Visual::Icon { icon, size: 16.0, color: t.text_dim };
                ui.add_node(i);
            }
            let mut a = Node::new(Layout::default().size(Size::FILL, Size::FILL));
            a.id = Some(area_id);
            a.clip = true;
            let lh = layout.line_px;
            a.paint = Some(Box::new(move |p, r| {
                let y = r.y + ((r.h - lh) / 2.0).round();
                let x0 = r.x - scroll;
                if has_selection {
                    let (xa, xb) = (layout.x_at(0, sa), layout.x_at(0, sb));
                    let sel = Rect::new(x0 + xa, y, xb - xa, lh);
                    p.push_clip(r);
                    p.rounded(sel, 3.0, if focused { t.selection } else { t.selection.fade(0.5) });
                    p.pop_clip();
                }
                p.push_clip(r);
                if let Some(ph) = &ph_layout {
                    p.text(ph, Vec2::new(r.x, y), t.text_disabled, TextAlign::Start, None);
                }
                p.text(&layout, Vec2::new(x0, y), t.text, TextAlign::Start, None);
                if caret > 0.0 {
                    let cx = (x0 + layout.x_at(0, cur_d)).max(r.x + 0.5).min(r.right() - 1.0);
                    p.fill(Rect::new(cx - 0.5, y + 2.0, 1.5, lh - 4.0), t.accent);
                }
                p.pop_clip();
            }));
            ui.add_node(a);
            if show_clear {
                let ch = ui.animate_bool(clear_id.with("h"), clear_r.hovered);
                let mut c = Node::new(Layout::default().size(Size::Px(20.0), Size::Px(20.0)));
                c.id = Some(clear_id);
                c.sense = Sense::CLICK;
                c.kind = "button";
                c.label = Some(clear_label);
                c.deco.radius = 10.0;
                c.deco.bg = t.control_hover.fade(ch);
                c.visual = Visual::Icon { icon: Icon::Close, size: 14.0, color: t.text_dim };
                ui.add_node(c);
            }
        });
        *ui.state::<InputState>(id) = st;
        r
    }
}

impl Sizing for TextInput<'_> {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// Handle one key press for a focused input; true if used.
fn key(ui: &mut Ui, id: Id, k: &KeyPress, text: &mut String, e: &mut EditState, password: bool) -> bool {
    let m = k.mods;
    let word = m.ctrl;
    match k.key {
        Key::Left => {
            let to = if e.has_selection() && !m.shift && !word {
                e.selection().0
            } else if word {
                edit::prev_word(text, e.cursor)
            } else {
                edit::prev_grapheme(text, e.cursor)
            };
            e.move_to(to, m.shift);
        }
        Key::Right => {
            let to = if e.has_selection() && !m.shift && !word {
                e.selection().1
            } else if word {
                edit::next_word(text, e.cursor)
            } else {
                edit::next_grapheme(text, e.cursor)
            };
            e.move_to(to, m.shift);
        }
        Key::Home => e.move_to(0, m.shift),
        Key::End => e.move_to(text.len(), m.shift),
        Key::Backspace => e.backspace(text, word),
        Key::Delete => e.delete(text, word),
        Key::Char('a') if m == Mods::CTRL => e.select_all(text),
        Key::Char('c') if m == Mods::CTRL => {
            if e.has_selection() && !password {
                ui.copy_text(e.selected_text(text).to_string());
            }
        }
        Key::Char('x') if m == Mods::CTRL => {
            if e.has_selection() && !password {
                ui.copy_text(e.selected_text(text).to_string());
                e.delete_selection(text);
            }
        }
        Key::Char('v') if m == Mods::CTRL => ui.request_paste(id, false),
        Key::Char('z') if m == Mods::CTRL => {
            e.undo(text);
        }
        Key::Char('z') if m == Mods::CTRL_SHIFT => {
            e.redo(text);
        }
        Key::Char('y') if m == Mods::CTRL => {
            e.redo(text);
        }
        Key::Insert if m == Mods::SHIFT => ui.request_paste(id, false),
        _ => match &k.text {
            Some(s) if !m.command() && !s.chars().any(|c| c.is_control()) => e.insert(text, s),
            _ => return false,
        },
    }
    true
}

/// Put the cursor (and selection) of the input `id` at byte offsets
/// `anchor..cursor`, e.g. to preselect the name part of "Untitled.txt"
/// before focusing it with [`Ui::request_focus`]. The selection is kept when
/// the focus arrives.
pub fn select_range(ui: &mut Ui, id: Id, anchor: usize, cursor: usize) {
    let st = ui.state::<InputState>(id);
    st.edit.move_to(anchor, false);
    st.edit.move_to(cursor, true);
    st.was_focused = true;
}

/// Keys a focused input uses for editing (see [`Ui::shortcut`]).
pub(crate) fn is_editing_key(key: Key, m: Mods) -> bool {
    let plain = !m.command();
    match key {
        Key::Left | Key::Right | Key::Home | Key::End | Key::Backspace | Key::Delete => !m.alt && !m.logo,
        Key::Char(c) if plain => !c.is_control(),
        Key::Char('a' | 'c' | 'x' | 'v' | 'z' | 'y') => m == Mods::CTRL || (m == Mods::CTRL_SHIFT && key == Key::Char('z')),
        Key::Insert => m == Mods::SHIFT,
        _ => false,
    }
}

/// What is drawn: the text, or a bullet per grapheme.
fn display(text: &str, password: bool) -> String {
    if password {
        use unicode_segmentation::UnicodeSegmentation;
        BULLET.repeat(text.graphemes(true).count())
    } else {
        text.to_string()
    }
}

fn to_display(text: &str, i: usize, password: bool) -> usize {
    if !password {
        return i;
    }
    use unicode_segmentation::UnicodeSegmentation;
    text[..i.min(text.len())].graphemes(true).count() * BULLET.len()
}

fn from_display(text: &str, i: usize, password: bool) -> usize {
    if !password {
        return i;
    }
    use unicode_segmentation::UnicodeSegmentation;
    let n = i / BULLET.len();
    text.grapheme_indices(true).nth(n).map(|(b, _)| b).unwrap_or(text.len())
}

impl Ui {
    /// A text input for `text` (see [`TextInput`] for options).
    pub fn text_input(&mut self, name: &str, text: &mut String) -> Response {
        TextInput::new(name, text).show(self)
    }

    /// A search field: magnifier icon, placeholder and clear button.
    pub fn search_input(&mut self, name: &str, text: &mut String) -> Response {
        TextInput::new(name, text).icon(Icon::Search).placeholder("Search").clear_button().show(self)
    }
}
