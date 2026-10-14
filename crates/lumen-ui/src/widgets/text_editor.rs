//! Multi-line text editing: [`TextEditor`] shows a [`TextDoc`] and edits it.
//!
//! Only the lines on screen are laid out and built, so a 100 000-line file
//! scrolls like a short one. With wrapping, lines that were never on screen
//! have an estimated height (from their length) until they are shown; the
//! first visible line stays in place when estimates are corrected, so the
//! view never jumps. Tabs are drawn up to the next tab stop.
//!
//! Keys: arrows (Ctrl: by word), Home (first non-blank, then column 0) and
//! End, Ctrl+Home/End, Page Up/Down, Shift to select; Enter keeps the
//! indentation, Tab indents (the selected lines, if several), Shift+Tab
//! unindents; Backspace/Delete (Ctrl: words); Ctrl+A/C/X/V, Ctrl+Z,
//! Ctrl+Shift+Z or Ctrl+Y. Mouse: click, drag (scrolling at the edges),
//! double-click a word, triple-click a line, Shift+click extends,
//! middle-click pastes the primary selection, right-click opens a menu.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::color::Color;
use crate::geom::{Insets, Rect, Vec2};
use crate::icons::Icon;
use crate::input::{Key, KeyPress, Mods};
use crate::layout::{Layout, Size, Sizing};
use crate::text::{Family, TextAlign, TextLayout, TextStyle, Wrap};
use crate::tree::{Anchor, Node, Sense};
use crate::ui::{CursorIcon, Response, Ui};

use super::menu::{self, MenuItem};
use super::scroll::ScrollArea;
use super::text_doc::TextDoc;

/// A line as laid out last frame (for hit testing).
#[derive(Clone)]
struct Shown {
    line: usize,
    /// Top relative to the text box (logical px).
    y: f32,
    layout: Rc<TextLayout>,
}

#[derive(Default)]
struct EditorState {
    shown: Vec<Shown>,
    /// The first visible line and its top (content px) last frame: it stays
    /// put when lines above it change height.
    anchor: Option<(usize, f32)>,
    version: u64,
    content_w: f32,
    blink: Option<Instant>,
    was_focused: bool,
    /// Average advance per byte of text (estimates), for a style key.
    avg: (u64, f32),
    /// A primary-button drag selecting text is in progress.
    selecting: bool,
}

/// Editor of a [`TextDoc`] (see the module docs for its keys).
///
/// ```no_run
/// # use lumen_ui::{TextDoc, TextEditor, Ui};
/// # fn f(ui: &mut Ui, doc: &mut TextDoc) {
/// let r = TextEditor::new("editor", doc).line_numbers(true).show(ui);
/// if r.changed {
///     let (line, col) = doc.cursor_position();
///     ui.title(format!("notes.txt • Ln {}, Col {}", line + 1, col + 1));
/// }
/// # }
/// ```
#[must_use = "a TextEditor does nothing until shown"]
pub struct TextEditor<'a> {
    name: String,
    doc: &'a mut TextDoc,
    wrap: bool,
    line_numbers: bool,
    mono: bool,
    size: Option<f32>,
    tab_width: usize,
    spaces: bool,
    read_only: bool,
    framed: bool,
    placeholder: String,
    highlights: Vec<std::ops::Range<usize>>,
    layout: Layout,
}

impl<'a> TextEditor<'a> {
    /// An editor of `doc`; `name` identifies it (ids, the test map). It
    /// fills the space it is given.
    pub fn new(name: impl Into<String>, doc: &'a mut TextDoc) -> TextEditor<'a> {
        TextEditor {
            name: name.into(),
            doc,
            wrap: true,
            line_numbers: false,
            mono: true,
            size: None,
            tab_width: 4,
            spaces: false,
            read_only: false,
            framed: false,
            placeholder: String::new(),
            highlights: Vec::new(),
            layout: Layout::row().size(Size::FILL, Size::FILL),
        }
    }

    /// Wrap long lines at the width (default), or scroll sideways.
    pub fn wrap(mut self, on: bool) -> TextEditor<'a> {
        self.wrap = on;
        self
    }

    /// Line numbers in a gutter at the start.
    pub fn line_numbers(mut self, on: bool) -> TextEditor<'a> {
        self.line_numbers = on;
        self
    }

    /// The monospace font (default) or the UI font (prose).
    pub fn mono(mut self, on: bool) -> TextEditor<'a> {
        self.mono = on;
        self
    }

    /// Font size in logical px (default: the theme's body size).
    pub fn font_size(mut self, px: f32) -> TextEditor<'a> {
        self.size = Some(px);
        self
    }

    /// Columns per tab stop (default 4).
    pub fn tab_width(mut self, n: usize) -> TextEditor<'a> {
        self.tab_width = n.clamp(1, 16);
        self
    }

    /// Tab inserts spaces up to the next tab stop instead of a tab.
    pub fn insert_spaces(mut self, on: bool) -> TextEditor<'a> {
        self.spaces = on;
        self
    }

    /// Selectable and copyable, not editable.
    pub fn read_only(mut self, on: bool) -> TextEditor<'a> {
        self.read_only = on;
        self
    }

    /// A rounded, bordered box like a text input (for forms); by default
    /// the editor is plain and fills its area (an editor window).
    pub fn framed(mut self) -> TextEditor<'a> {
        self.framed = true;
        self
    }

    /// Dim text shown while the document is empty.
    pub fn placeholder(mut self, p: impl Into<String>) -> TextEditor<'a> {
        self.placeholder = p.into();
        self
    }

    /// Ranges to mark (search matches), byte offsets into the document.
    pub fn highlights(mut self, ranges: &[std::ops::Range<usize>]) -> TextEditor<'a> {
        self.highlights = ranges.to_vec();
        self
    }
}

impl Sizing for TextEditor<'_> {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

// -- Tabs ------------------------------------------------------------------------------

/// The line as drawn: tabs become spaces up to the next tab stop.
fn expand_tabs(line: &str, tab: usize) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0;
    for c in line.chars() {
        if c == '\t' {
            let n = tab - col % tab;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
        } else {
            out.push(c);
            col += 1;
        }
    }
    Cow::Owned(out)
}

/// Byte offset `off` of `line` → byte offset in its drawn form.
fn to_shown(line: &str, off: usize, tab: usize) -> usize {
    if !line.contains('\t') {
        return off;
    }
    let (mut col, mut d) = (0, 0);
    for (i, c) in line.char_indices() {
        if i >= off {
            break;
        }
        if c == '\t' {
            let n = tab - col % tab;
            col += n;
            d += n;
        } else {
            col += 1;
            d += c.len_utf8();
        }
    }
    d
}

/// Byte offset `d` of the drawn form → byte offset in `line` (inside a
/// tab's spaces: the nearer side of the tab).
fn from_shown(line: &str, d: usize, tab: usize) -> usize {
    if !line.contains('\t') {
        return d.min(line.len());
    }
    let (mut col, mut at) = (0, 0);
    for (i, c) in line.char_indices() {
        let w = if c == '\t' { tab - col % tab } else { c.len_utf8() };
        if d < at + w {
            return if c == '\t' && d - at > w / 2 { i + 1 } else { i };
        }
        at += w;
        col += if c == '\t' { w } else { 1 };
    }
    line.len()
}

// -- Geometry ----------------------------------------------------------------------------

/// The settings that decide how lines are laid out this frame.
struct Geo {
    style: TextStyle,
    /// Wrap width (logical px) or None.
    wrap: Option<f32>,
    tab: usize,
    scale: f32,
    lh: f32,
}

impl Geo {
    fn layout(&self, ui: &mut Ui, doc: &TextDoc, line: usize) -> Rc<TextLayout> {
        let shown = expand_tabs(doc.line(line), self.tab);
        match self.wrap {
            Some(w) => ui.fonts().layout(&shown, &self.style, Wrap::Wrap { max_lines: 0 }, Some(w), self.scale),
            None => ui.fonts().layout(&shown, &self.style, Wrap::None, None, self.scale),
        }
    }

    /// Screen row and x of byte offset `at` (on line `line`).
    fn caret(&self, doc: &TextDoc, line: usize, layout: &TextLayout, at: usize) -> (usize, f32) {
        let r = doc.line_range(line);
        let d = to_shown(doc.line(line), at.clamp(r.start, r.end) - r.start, self.tab);
        let row = layout.starts.partition_point(|&s| s <= d).saturating_sub(1).min(layout.lines.len().saturating_sub(1));
        let start = layout.starts.get(row).copied().unwrap_or(0);
        (row, layout.x_at(row, d.saturating_sub(start)))
    }

    /// The document offset at `x` on screen row `row` of line `line`.
    fn offset_at(&self, doc: &TextDoc, line: usize, layout: &TextLayout, row: usize, x: f32) -> usize {
        let row = row.min(layout.lines.len().saturating_sub(1));
        let start = layout.starts.get(row).copied().unwrap_or(0);
        let d = start + layout.index_at(row, x.max(0.0));
        doc.line_range(line).start + from_shown(doc.line(line), d, self.tab)
    }

    fn rows(layout: &TextLayout) -> usize {
        layout.lines.len().max(1)
    }
}

/// Tops of every line (content px from the text start), from measured
/// rows or estimates; `tops[n]` is the total height.
fn line_tops(doc: &TextDoc, geo: &Geo, avg: f32) -> Vec<f32> {
    let n = doc.line_count();
    let mut tops = Vec::with_capacity(n + 1);
    let mut y = 0.0;
    for i in 0..n {
        tops.push(y);
        let rows = match (geo.wrap, doc.rows.get(i)) {
            (None, _) => 1,
            (Some(_), Some(&r)) if r > 0 => r,
            (Some(w), _) => {
                let r = doc.line_range(i);
                ((r.len() as f32 * avg / w.max(1.0)).ceil() as u32).max(1)
            }
        };
        y += rows as f32 * geo.lh;
    }
    tops.push(y);
    tops
}

/// The line at content y (its top is at or above y).
fn line_at(tops: &[f32], y: f32) -> usize {
    tops.partition_point(|&t| t <= y).saturating_sub(1).min(tops.len().saturating_sub(2))
}

impl TextEditor<'_> {
    pub fn show(self, ui: &mut Ui) -> Response {
        let id = ui.make_id(("editor", &self.name));
        let scroll_id = id.with("scroll");
        let text_id = id.with("text");
        let gutter_id = id.with("gutter");
        let mut r = ui.interact(id);
        let mut st = std::mem::take(ui.state::<EditorState>(id));
        let doc = self.doc;
        let version0 = doc.version();
        let t = ui.theme().clone();
        let scale = ui.scale();
        let now = ui.now();
        let focused = r.focused;
        let mut style = TextStyle::new(self.size.unwrap_or(t.font_size));
        if self.mono {
            style.family = Family::Mono;
        }
        style.line_height = if self.mono { 1.5 } else { 1.45 };
        let lh = style.line_px();
        let pad = Insets::new(if self.line_numbers { 8.0 } else { 14.0 }, 10.0, 14.0, 10.0);
        let vp = ui.prev_rect(scroll_id.with("vp"));
        let text_box = ui.prev_rect(text_id);
        let view = vp.map(|v| v.size()).unwrap_or(ui.window_size());
        let wrap_w = (view.x - pad.horizontal() - 2.0).max(40.0);
        let geo = Geo { style, wrap: self.wrap.then_some(wrap_w), tab: self.tab_width, scale, lh };

        // Average advance per byte, for estimating the height of lines not
        // yet shown.
        let style_key = {
            let mut h = std::hash::DefaultHasher::new();
            (style.size.to_bits(), style.family, scale.to_bits(), self.tab_width).hash(&mut h);
            h.finish()
        };
        if st.avg.0 != style_key || st.avg.1 <= 0.0 {
            let sample = "The quick brown fox jumps over the lazy dog; 0123456789 (it said).";
            let l = ui.fonts().layout(sample, &style, Wrap::None, None, scale);
            st.avg = (style_key, l.size.x / sample.len() as f32);
        }
        let rows_key = {
            let mut h = std::hash::DefaultHasher::new();
            (style_key, geo.wrap.map(f32::to_bits)).hash(&mut h);
            h.finish()
        };
        if doc.rows_key != rows_key || doc.rows.len() != doc.line_count() {
            doc.rows = vec![0; doc.line_count()];
            doc.rows_key = rows_key;
        }
        if focused && !st.was_focused {
            st.blink = Some(now);
        }
        st.was_focused = focused;

        // -- Mouse ------------------------------------------------------------------------
        let hit = |ui: &mut Ui, doc: &TextDoc, st: &EditorState, p: Vec2| -> Option<usize> {
            let tb = text_box?;
            let first = st.shown.first()?;
            let last = st.shown.last()?;
            let rel = p - tb.min();
            let s = if rel.y < first.y {
                return Some(geo.offset_at(doc, first.line, &first.layout, 0, rel.x));
            } else {
                st.shown.iter().rev().find(|s| s.y <= rel.y).unwrap_or(last)
            };
            let layout = geo.layout(ui, doc, s.line);
            let row = ((rel.y - s.y) / lh).floor().max(0.0) as usize;
            Some(geo.offset_at(doc, s.line, &layout, row, rel.x))
        };
        let menu_id = id.with("menu");
        if let Some((p, count)) = ui.press_pos(id) {
            if let Some(at) = hit(ui, doc, &st, p) {
                match count {
                    2 => doc.select_word(at),
                    3 => doc.select_line(doc.line_of(at)),
                    _ => doc.move_to(at, ui.mods().shift),
                }
                doc.goal_x = None;
                st.selecting = count == 1;
                st.blink = Some(now);
            }
        }
        // (Moves and the release can arrive in one pass: the release still
        // ends the drag where the pointer is.)
        if (r.dragging || r.drag_released) && st.selecting {
            if let (Some(p), Some(v)) = (ui.pointer(), vp) {
                if let Some(at) = hit(ui, doc, &st, p) {
                    doc.move_to(at, true);
                    doc.reveal = false;
                }
                // Past the top or bottom edge: scroll toward the pointer,
                // faster the farther it is.
                let over = if p.y < v.y { p.y - v.y } else if p.y > v.bottom() { p.y - v.bottom() } else { 0.0 };
                if over != 0.0 {
                    ui.scroll_state(scroll_id).shift(Vec2::new(0.0, over.clamp(-60.0, 60.0) * 0.5));
                    ui.request_repaint();
                }
            }
        }
        if !r.dragging && st.selecting && ui.press_pos(id).is_none() {
            st.selecting = false;
        }
        if (r.drag_released || r.double_clicked) && doc.has_selection() {
            ui.set_primary_selection(doc.selected_text().to_string());
        }
        if r.middle_clicked && !self.read_only {
            if let Some(at) = ui.pointer().and_then(|p| hit(ui, doc, &st, p)) {
                doc.move_to(at, false);
            }
            ui.request_paste(id, true);
        }
        if r.secondary_clicked {
            let at = r.pointer.unwrap_or(r.rect.center());
            menu::open_menu(ui, menu_id, Anchor::At(at), 0.0, false);
            ui.request_focus(id);
        }

        // -- Keyboard ---------------------------------------------------------------------
        if focused && ui.keys_allowed() {
            ui.mark_text_focus(true);
        }
        let page_rows = ((view.y / lh).floor() as usize).saturating_sub(1).max(1);
        if focused {
            let ro = self.read_only;
            let (tab, spaces) = (self.tab_width, self.spaces);
            ui.consume_keys(|ui, k| {
                let used = key(ui, id, k, doc, &geo, page_rows, ro, tab, spaces);
                if used {
                    st.blink = Some(now);
                }
                used
            });
        }
        if let Some(s) = ui.take_paste(id) {
            if !self.read_only {
                doc.insert(&s);
            }
        }

        // -- Context menu -----------------------------------------------------------------
        let has_sel = doc.has_selection();
        let (can_undo, can_redo, ro) = (doc.can_undo(), doc.can_redo(), self.read_only);
        let mut action = None;
        menu::show_menu(ui, menu_id, |m| {
            if !ro {
                if m.add(MenuItem::new("Undo").shortcut(crate::Shortcut::ctrl('z')).enabled(can_undo)) {
                    action = Some("undo");
                }
                if m.add(MenuItem::new("Redo").shortcut(crate::Shortcut::ctrl_shift('z')).enabled(can_redo)) {
                    action = Some("redo");
                }
                m.separator();
                if m.add(MenuItem::new("Cut").icon(Icon::Cut).shortcut(crate::Shortcut::ctrl('x')).enabled(has_sel)) {
                    action = Some("cut");
                }
            }
            if m.add(MenuItem::new("Copy").icon(Icon::Copy).shortcut(crate::Shortcut::ctrl('c')).enabled(has_sel)) {
                action = Some("copy");
            }
            if !ro && m.add(MenuItem::new("Paste").icon(Icon::Paste).shortcut(crate::Shortcut::ctrl('v'))) {
                action = Some("paste");
            }
            m.separator();
            if m.add(MenuItem::new("Select All").shortcut(crate::Shortcut::ctrl('a'))) {
                action = Some("all");
            }
        });
        match action {
            Some("undo") => {
                doc.undo();
            }
            Some("redo") => {
                doc.redo();
            }
            Some("cut") => {
                ui.copy_text(doc.selected_text().to_string());
                doc.delete_selection();
            }
            Some("copy") => ui.copy_text(doc.selected_text().to_string()),
            Some("paste") => ui.request_paste(id, false),
            Some("all") => doc.select_all(),
            _ => {}
        }

        // -- Geometry of this frame -------------------------------------------------------
        // The cursor's line is measured now, so revealing it uses its real
        // height.
        let cursor_line = doc.line_of(doc.cursor());
        let cursor_layout = geo.layout(ui, doc, cursor_line);
        if geo.wrap.is_some() {
            doc.rows[cursor_line] = Geo::rows(&cursor_layout) as u32;
        }
        let avg = st.avg.1;
        let mut tops = line_tops(doc, &geo, avg);
        // Lines above the view were (re)measured: keep the first visible
        // line where it was.
        if let Some((line, top)) = st.anchor {
            if st.version == doc.version() && line < doc.line_count() && (tops[line] - top).abs() > 0.25 {
                ui.scroll_state(scroll_id).shift(Vec2::new(0.0, tops[line] - top));
            }
        }
        if std::mem::take(&mut doc.reveal) {
            let (row, x) = geo.caret(doc, cursor_line, &cursor_layout, doc.cursor());
            let y = pad.top + tops[cursor_line] + row as f32 * lh;
            let margin = lh.min(view.y / 4.0);
            let s = ui.scroll_state(scroll_id);
            s.reveal_y(y - margin, y + lh + margin, view.y);
            if geo.wrap.is_none() {
                s.reveal_x(pad.left + x - 40.0, pad.left + x + 40.0, view.x);
            }
        }

        // -- Build ----------------------------------------------------------------------------
        if r.hovered {
            ui.set_cursor(CursorIcon::Text);
        }
        let mut caret_on = false;
        if focused && ui.window_focused() && !self.read_only {
            let since = now.duration_since(st.blink.unwrap_or(now)).as_secs_f32();
            if since > 12.0 {
                caret_on = true;
            } else {
                let phase = since % 1.0;
                caret_on = phase < 0.6;
                let next = if phase < 0.6 { 0.6 - phase } else { 1.0 - phase };
                ui.request_repaint_after(Duration::from_secs_f32(next.max(0.01)));
            }
        }
        let mut n = ui.widget_node(id, "editor", Some(&self.name), self.layout, Sense::DRAG);
        n.focusable = true;
        if self.framed {
            let focus_t = ui.animate_bool(id.with("focus"), focused);
            n.deco.bg = t.input_bg;
            n.deco.radius = t.radius;
            n.deco.border = t.separator.mix(t.focus_ring, focus_t);
            n.deco.border_width = 1.0 + focus_t;
            n.clip = true;
        }
        if ui.test.enabled() {
            let (l, c) = doc.cursor_position();
            n.value = Some(format!("{}:{} {}", l + 1, c + 1, if doc.len() <= 4096 { doc.text() } else { "…" }));
        }
        let digits = doc.line_count().to_string().len().max(2);
        let gutter_w = if self.line_numbers {
            let w = ui.fonts().layout(&"0".repeat(digits), &style, Wrap::None, None, scale).size.x;
            (w + 28.0).round()
        } else {
            0.0
        };
        let sel = doc.selection();
        let cursor = doc.cursor();
        let highlights = self.highlights;
        let placeholder = (doc.is_empty() && !self.placeholder.is_empty()).then(|| ui.fonts().layout(&self.placeholder, &style, Wrap::Wrap { max_lines: 0 }, Some(wrap_w), scale));
        let line_numbers = self.line_numbers;
        let wrap = self.wrap;
        let mut shown: Vec<Shown> = Vec::new();
        let mut gutter_index = None;
        ui.add_container(n, |ui| {
            if line_numbers {
                let mut g = Node::new(Layout::default().size(Size::Px(gutter_w), Size::FILL));
                g.id = Some(gutter_id);
                gutter_index = Some(ui.add_node(g));
            }
            let area = if wrap { ScrollArea::vertical() } else { ScrollArea::both() };
            area.with_id(scroll_id).content(Layout::column().padding(pad)).show_viewport(ui, |ui, v| {
                // The lines on screen, laid out exactly.
                let n_lines = doc.line_count();
                let first = line_at(&tops, (v.rect.y - pad.top).max(0.0));
                let bottom = v.rect.bottom() - pad.top;
                let mut y = tops[first];
                let mut i = first;
                let mut remeasured = false;
                while i < n_lines && (y < bottom + lh || i == first) {
                    let layout = if i == cursor_line { cursor_layout.clone() } else { geo.layout(ui, doc, i) };
                    let rows = Geo::rows(&layout);
                    if geo.wrap.is_some() && doc.rows[i] != rows as u32 {
                        doc.rows[i] = rows as u32;
                        remeasured = true;
                    }
                    shown.push(Shown { line: i, y: y - tops[first], layout });
                    y += rows as f32 * lh;
                    i += 1;
                }
                if remeasured {
                    tops = line_tops(doc, &geo, avg);
                }
                let total = tops[n_lines];
                let top = tops[first];
                let end = top + shown.last().map(|s| s.y + Geo::rows(&s.layout) as f32 * lh).unwrap_or(0.0);
                if top > 0.0 {
                    ui.add_node(Node::new(Layout::default().size(Size::FILL, Size::Px(top))));
                }
                // Width: the wrap width, or the widest line seen so far and an
                // estimate from the longest one.
                let width = if wrap {
                    Size::FILL
                } else {
                    let longest = (0..n_lines).map(|l| doc.line_range(l).len()).max().unwrap_or(0) as f32;
                    let seen = shown.iter().map(|s| s.layout.size.x).fold(0.0, f32::max);
                    st.content_w = seen.max(longest * avg) + 2.0;
                    Size::Px(st.content_w.max(v.rect.w - pad.horizontal()))
                };
                let mut tn = Node::new(Layout::default().size(width, Size::Px((end - top).max(lh))));
                tn.id = Some(text_id);
                tn.paint = Some(paint_lines(&geo, doc, &shown, &t, sel.clone(), cursor, cursor_line, caret_on, focused, &highlights, placeholder.clone()));
                ui.add_node(tn);
                if total > end {
                    ui.add_node(Node::new(Layout::default().size(Size::FILL, Size::Px(total - end))));
                }
                st.anchor = Some((first, top));
            });
        });
        // The gutter: numbers at the lines' positions (it does not scroll
        // sideways, so it is outside the scroll area).
        if let Some(gi) = gutter_index {
            let offset = ui.scroll_state(scroll_id).offset();
            let base = shown.first().map(|s| tops[s.line]).unwrap_or(0.0);
            let nums: Vec<(f32, Rc<TextLayout>, bool)> = shown
                .iter()
                .map(|s| {
                    let l = ui.fonts().layout(&(s.line + 1).to_string(), &style, Wrap::None, None, scale);
                    (pad.top + base + s.y - offset.y, l, s.line == cursor_line)
                })
                .collect();
            let (dim, bright) = (t.text_dim.fade(0.75), t.text_dim);
            ui.node_mut(gi).paint = Some(Box::new(move |p, r| {
                p.push_clip(r);
                for (y, l, current) in &nums {
                    p.text(l, Vec2::new(r.x, r.y + y), if *current { bright } else { dim }, TextAlign::End, Some(r.w - 14.0));
                }
                p.pop_clip();
            }));
        }
        st.shown = shown;
        st.version = doc.version();
        if doc.version() != version0 {
            r.changed = true;
            // Test log: where the cursor is and the text of its line.
            let (l, c) = doc.cursor_position();
            ui.log(&format!("edit {} {}:{} {}", self.name, l + 1, c + 1, doc.line(l)));
        }
        *ui.state::<EditorState>(id) = st;
        r
    }
}

/// The paint closure of the visible lines: current-line band, marks,
/// selection, text, caret.
#[allow(clippy::too_many_arguments)]
fn paint_lines(
    geo: &Geo,
    doc: &TextDoc,
    shown: &[Shown],
    t: &crate::theme::Theme,
    sel: std::ops::Range<usize>,
    cursor: usize,
    cursor_line: usize,
    caret_on: bool,
    focused: bool,
    highlights: &[std::ops::Range<usize>],
    placeholder: Option<Rc<TextLayout>>,
) -> crate::tree::PaintFn {
    let lh = geo.lh;
    let mut rects: Vec<(Rect, Color)> = Vec::new();
    let sel_color = if focused { t.selection } else { t.selection.fade(0.55) };
    let mark = t.warning.with_alpha(0.28);
    let newline_w = (geo.style.size * 0.45).round();
    let mut caret = None;
    // Spans of `range` on the rows of a shown line.
    let spans = |s: &Shown, range: &std::ops::Range<usize>, extend: bool, out: &mut Vec<Rect>| {
        let lr = doc.line_range(s.line);
        if range.end < lr.start || range.start > lr.end || range.is_empty() {
            return;
        }
        let src = doc.line(s.line);
        let a = to_shown(src, range.start.max(lr.start) - lr.start, geo.tab);
        let b = to_shown(src, range.end.min(lr.end) - lr.start, geo.tab);
        let past_end = extend && range.end > lr.end;
        let rows = Geo::rows(&s.layout);
        for row in 0..rows {
            let rs = s.layout.starts.get(row).copied().unwrap_or(0);
            let re = s.layout.starts.get(row + 1).copied().unwrap_or(usize::MAX);
            let last = row + 1 == rows;
            if b < rs || (a >= re && !last) || (a > b) {
                continue;
            }
            let x0 = s.layout.x_at(row, a.max(rs) - rs);
            let mut x1 = if b >= re { s.layout.line_width(row) } else { s.layout.x_at(row, b - rs) };
            if last && past_end {
                x1 += newline_w;
            }
            if x1 > x0 {
                out.push(Rect::new(x0, s.y + row as f32 * lh, x1 - x0, lh));
            }
        }
    };
    for s in shown {
        if s.line == cursor_line && sel.is_empty() && focused {
            rects.push((Rect::new(-1000.0, s.y, 100_000.0, Geo::rows(&s.layout) as f32 * lh), t.text.with_alpha(0.035)));
        }
        let mut v = Vec::new();
        for h in highlights {
            spans(s, h, false, &mut v);
        }
        rects.extend(v.drain(..).map(|r| (r, mark)));
        spans(s, &sel, true, &mut v);
        rects.extend(v.drain(..).map(|r| (r, sel_color)));
        if s.line == cursor_line && caret_on {
            let (row, x) = geo.caret(doc, s.line, &s.layout, cursor);
            caret = Some(Rect::new(x - 0.75, s.y + row as f32 * lh + 2.0, 1.75, lh - 4.0));
        }
    }
    let texts: Vec<(f32, Rc<TextLayout>)> = shown.iter().map(|s| (s.y, s.layout.clone())).collect();
    let (text, accent, dim) = (t.text, t.accent, t.text_disabled);
    Box::new(move |p, r| {
        for (rect, c) in &rects {
            p.rounded(rect.translate(r.min()), 3.0, *c);
        }
        if let Some(l) = &placeholder {
            p.text(l, r.min(), dim, TextAlign::Start, None);
        }
        for (y, l) in &texts {
            p.text(l, Vec2::new(r.x, r.y + y), text, TextAlign::Start, None);
        }
        if let Some(c) = caret {
            p.fill(c.translate(r.min()), accent);
        }
    })
}

/// Handle one key for a focused editor; true if used.
#[allow(clippy::too_many_arguments)]
fn key(ui: &mut Ui, id: crate::Id, k: &KeyPress, doc: &mut TextDoc, geo: &Geo, page: usize, ro: bool, tab: usize, spaces: bool) -> bool {
    let m = k.mods;
    let word = m.ctrl;
    let shift = m.shift;
    if m.alt || m.logo {
        return false;
    }
    let horizontal = |doc: &mut TextDoc, to: usize| {
        doc.move_to(to, shift);
        doc.goal_x = None;
    };
    match k.key {
        Key::Left => {
            let to = if doc.has_selection() && !shift && !word { doc.selection().start } else { doc.left(word) };
            horizontal(doc, to);
        }
        Key::Right => {
            let to = if doc.has_selection() && !shift && !word { doc.selection().end } else { doc.right(word) };
            horizontal(doc, to);
        }
        Key::Up if !word => vertical(ui, doc, geo, -1, shift),
        Key::Down if !word => vertical(ui, doc, geo, 1, shift),
        Key::PageUp if !word => vertical(ui, doc, geo, -(page as i32), shift),
        Key::PageDown if !word => vertical(ui, doc, geo, page as i32, shift),
        Key::Home if word => horizontal(doc, 0),
        Key::End if word => horizontal(doc, doc.len()),
        Key::Home => {
            let line = doc.line_of(doc.cursor());
            let layout = geo.layout(ui, doc, line);
            let (row, _) = geo.caret(doc, line, &layout, doc.cursor());
            let lr = doc.line_range(line);
            let to = if row > 0 {
                geo.offset_at(doc, line, &layout, row, 0.0)
            } else {
                // First non-blank, then the line start.
                let blank = doc.line(line).len() - doc.line(line).trim_start_matches([' ', '\t']).len();
                if doc.cursor() == lr.start + blank { lr.start } else { lr.start + blank }
            };
            horizontal(doc, to);
        }
        Key::End => {
            let line = doc.line_of(doc.cursor());
            let layout = geo.layout(ui, doc, line);
            let (row, _) = geo.caret(doc, line, &layout, doc.cursor());
            let to = if row + 1 < Geo::rows(&layout) { geo.offset_at(doc, line, &layout, row, 1.0e6) } else { doc.line_range(line).end };
            horizontal(doc, to);
        }
        Key::Char('a') if m == Mods::CTRL => doc.select_all(),
        Key::Char('c') | Key::Insert if m == Mods::CTRL => {
            if doc.has_selection() {
                ui.copy_text(doc.selected_text().to_string());
            }
        }
        Key::Char('x') if m == Mods::CTRL && !ro => {
            if doc.has_selection() {
                ui.copy_text(doc.selected_text().to_string());
                doc.delete_selection();
            }
        }
        Key::Delete if m == Mods::SHIFT && !ro => {
            if doc.has_selection() {
                ui.copy_text(doc.selected_text().to_string());
                doc.delete_selection();
            }
        }
        Key::Char('v') if m == Mods::CTRL && !ro => ui.request_paste(id, false),
        Key::Insert if m == Mods::SHIFT && !ro => ui.request_paste(id, false),
        Key::Char('z') if m == Mods::CTRL && !ro => {
            doc.undo();
        }
        Key::Char('z') if m == Mods::CTRL_SHIFT && !ro => {
            doc.redo();
        }
        Key::Char('y') if m == Mods::CTRL && !ro => {
            doc.redo();
        }
        _ if ro => return false,
        Key::Enter if !word => doc.newline(),
        Key::Tab if m == Mods::SHIFT => doc.unindent(tab),
        Key::Tab if m == Mods::NONE => {
            if doc.selected_text().contains('\n') {
                let unit = if spaces { " ".repeat(tab) } else { "\t".to_string() };
                doc.indent(&unit);
            } else if spaces {
                let (_, col) = doc.position(doc.selection().start);
                doc.insert(&" ".repeat(tab - col % tab));
            } else {
                doc.insert("\t");
            }
        }
        Key::Backspace => doc.backspace(word),
        Key::Delete => doc.delete(word),
        _ => match &k.text {
            Some(s) if !m.command() && !s.chars().any(|c| c.is_control()) => doc.insert(s),
            _ => return false,
        },
    }
    true
}

/// Move the cursor `rows` screen rows up (negative) or down, keeping its x.
fn vertical(ui: &mut Ui, doc: &mut TextDoc, geo: &Geo, rows: i32, extend: bool) {
    let mut line = doc.line_of(doc.cursor());
    let mut layout = geo.layout(ui, doc, line);
    let (mut row, x) = geo.caret(doc, line, &layout, doc.cursor());
    let goal = doc.goal_x.unwrap_or(x);
    let mut left = rows.unsigned_abs();
    let mut edge = None;
    while left > 0 {
        if rows < 0 {
            if row > 0 {
                row -= 1;
            } else if line > 0 {
                line -= 1;
                layout = geo.layout(ui, doc, line);
                row = Geo::rows(&layout) - 1;
            } else {
                edge = Some(0);
                break;
            }
        } else if row + 1 < Geo::rows(&layout) {
            row += 1;
        } else if line + 1 < doc.line_count() {
            line += 1;
            layout = geo.layout(ui, doc, line);
            row = 0;
        } else {
            edge = Some(doc.len());
            break;
        }
        left -= 1;
    }
    // Past the first or last row: to the very start or end (only when no
    // row was moved at all, a page move still lands on a row).
    let to = match edge {
        Some(e) if left == rows.unsigned_abs() => e,
        _ => geo.offset_at(doc, line, &layout, row, goal),
    };
    doc.move_to(to, extend);
    doc.goal_x = Some(goal);
}

impl Ui {
    /// A plain editor of `doc` filling the available space (see
    /// [`TextEditor`] for options).
    pub fn text_editor(&mut self, name: &str, doc: &mut TextDoc) -> Response {
        TextEditor::new(name, doc).show(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_stops_and_map_both_ways() {
        let line = "a\tbc\td";
        assert_eq!(expand_tabs(line, 4), "a   bc  d");
        assert_eq!(to_shown(line, 2, 4), 4, "after the tab");
        assert_eq!(to_shown(line, 5, 4), 8);
        assert_eq!(from_shown(line, 4, 4), 2);
        assert_eq!(from_shown(line, 2, 4), 1, "near the start of the tab: before it");
        assert_eq!(from_shown(line, 3, 4), 2, "near its end: after it");
        assert_eq!(from_shown(line, 99, 4), line.len());
        assert_eq!(to_shown("plain", 3, 4), 3);
    }
}
