//! Virtualized lists and grids with selection: only the rows on screen are
//! built, so a list of 100 000 items costs what a screenful does.
//!
//! Selection follows the desktop conventions: click selects one item,
//! Ctrl+click toggles, Shift+click extends from the anchor, dragging on empty
//! space draws a selection rectangle (Ctrl adds to the selection). Keyboard:
//! arrows, Home/End, Page Up/Down move the cursor (Shift extends), Space
//! toggles with Ctrl, Ctrl+A selects all, Enter activates. Double click
//! activates too; right click opens the app's context menu for the item.

use std::collections::BTreeSet;
use std::hash::Hash;

use crate::geom::{Rect, Vec2};
use crate::id::Id;
use crate::input::{Key, Mods};
use crate::layout::{Layout, Size, Sizing};
use crate::tree::{Node, Sense};
use crate::ui::{Response, Ui};

use super::scroll::ScrollArea;

/// Selected items and the keyboard cursor of a list or grid (owned by the
/// app, so it can act on the selection and keep it across view changes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    selected: BTreeSet<usize>,
    /// The focused item (keyboard navigation, Enter activates it).
    pub cursor: Option<usize>,
    anchor: Option<usize>,
}

impl Selection {
    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.contains(&i)
    }

    /// Selected indices in ascending order.
    pub fn selected(&self) -> impl Iterator<Item = usize> + '_ {
        self.selected.iter().copied()
    }

    pub fn count(&self) -> usize {
        self.selected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Select just `i` (and put the cursor on it).
    pub fn select_only(&mut self, i: usize) {
        self.selected.clear();
        self.selected.insert(i);
        self.cursor = Some(i);
        self.anchor = Some(i);
    }

    pub fn toggle(&mut self, i: usize) {
        if !self.selected.remove(&i) {
            self.selected.insert(i);
        }
        self.cursor = Some(i);
        self.anchor = Some(i);
    }

    /// Select the range from the anchor to `i` (replacing the selection).
    pub fn extend_to(&mut self, i: usize) {
        let a = self.anchor.unwrap_or(i);
        self.selected = (a.min(i)..=a.max(i)).collect();
        self.cursor = Some(i);
        self.anchor = Some(a);
    }

    pub fn select_all(&mut self, count: usize) {
        self.selected = (0..count).collect();
    }

    /// Forget indices at or past `count` (after items were removed).
    pub fn truncate(&mut self, count: usize) {
        self.selected.retain(|&i| i < count);
        if self.cursor.is_some_and(|c| c >= count) {
            self.cursor = count.checked_sub(1);
        }
        if self.anchor.is_some_and(|a| a >= count) {
            self.anchor = None;
        }
    }

    /// Apply a click on `i` with modifiers (`multi`: Ctrl/Shift allowed).
    pub fn click(&mut self, i: usize, mods: Mods, multi: bool) {
        if multi && mods.ctrl {
            self.toggle(i);
        } else if multi && mods.shift {
            self.extend_to(i);
        } else {
            self.select_only(i);
        }
    }

    fn set(&mut self, items: BTreeSet<usize>) {
        self.selected = items;
    }
}

/// Information for building one row (or grid cell).
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub index: usize,
    pub selected: bool,
    pub hovered: bool,
    /// The keyboard cursor is on it.
    pub cursor: bool,
}

/// What happened in a list or grid this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListResponse {
    /// The list box itself (focused, hovered…).
    pub response: Response,
    /// Double-clicked, or Enter on the cursor item.
    pub activated: Option<usize>,
    /// Right-clicked (or Menu key on the cursor item), with where to open
    /// the menu. `None` index: on empty space.
    pub context: Option<(Option<usize>, Vec2)>,
    /// The selection changed.
    pub changed: bool,
}

#[derive(Default)]
struct BandState {
    /// Drag start in content coordinates and the selection when it began.
    start: Option<(Vec2, BTreeSet<usize>)>,
    end: Vec2,
}

/// Geometry of a virtualized list (one column) or grid.
struct Grid {
    count: usize,
    cols: usize,
    cell: Vec2,
    pad: f32,
}

impl Grid {
    fn rows(&self) -> usize {
        self.count.div_ceil(self.cols.max(1))
    }

    /// Content-space rectangle of item `i`.
    fn rect(&self, i: usize) -> Rect {
        let (r, c) = (i / self.cols, i % self.cols);
        Rect::new(self.pad + c as f32 * self.cell.x, self.pad + r as f32 * self.cell.y, self.cell.x, self.cell.y)
    }

    fn items_in(&self, band: Rect) -> BTreeSet<usize> {
        let mut out = BTreeSet::new();
        if band.is_empty() && band.w <= 0.0 && band.h <= 0.0 {
            return out;
        }
        let r0 = (((band.y - self.pad) / self.cell.y).floor().max(0.0)) as usize;
        let r1 = (((band.bottom() - self.pad) / self.cell.y).ceil().max(0.0)) as usize;
        for r in r0..r1.min(self.rows()) {
            for c in 0..self.cols {
                let i = r * self.cols + c;
                if i < self.count && self.rect(i).intersects(&band) {
                    out.insert(i);
                }
            }
        }
        out
    }
}

/// A virtualized list of rows of equal height.
#[must_use = "a ListView does nothing until shown"]
pub struct ListView {
    key: Id,
    count: usize,
    row_height: f32,
    cell_width: Option<f32>,
    multi: bool,
    reveal: Option<usize>,
    layout: Layout,
}

impl ListView {
    /// `count` rows of `row_height`; `key` identifies the list.
    pub fn new(key: impl Hash, count: usize, row_height: f32) -> ListView {
        ListView { key: Id(0).with(key), count, row_height, cell_width: None, multi: true, reveal: None, layout: Layout::default().size(Size::FILL, Size::FILL) }
    }

    /// A grid of `cell_width` × `row_height` cells instead (as many columns
    /// as fit; the cells stretch to use the width).
    pub fn grid(key: impl Hash, count: usize, cell_width: f32, cell_height: f32) -> ListView {
        ListView { cell_width: Some(cell_width), ..ListView::new(key, count, cell_height) }
    }

    /// Only one item can be selected.
    pub fn single_selection(mut self) -> ListView {
        self.multi = false;
        self
    }

    /// Scroll (animated) so that item `index` is visible, e.g. after the app
    /// selected it (type-ahead find, "reveal in folder"). Pass it for one
    /// frame; the list keeps scrolling there by itself.
    pub fn reveal(mut self, index: Option<usize>) -> ListView {
        self.reveal = index;
        self
    }

    /// Build the visible items with `item`; each is a row (horizontal)
    /// container for a list, a column for a grid.
    pub fn show(self, ui: &mut Ui, sel: &mut Selection, mut item: impl FnMut(&mut Ui, Item)) -> ListResponse {
        let id = ui.make_id(("list", self.key));
        let mut out = ListResponse { response: ui.interact(id), ..Default::default() };
        let before = sel.clone();
        let pad = 6.0;
        let scroll_id = id.with("scroll");
        let vp_rect = ui.prev_rect(scroll_id.with("vp"));
        let view_w = vp_rect.map(|r| r.w).unwrap_or(ui.window_size().x);
        let cols = match self.cell_width {
            Some(w) => (((view_w - 2.0 * pad) / w).floor() as usize).max(1),
            None => 1,
        };
        let cell_w = match self.cell_width {
            Some(_) => ((view_w - 2.0 * pad) / cols as f32).floor().max(1.0),
            None => (view_w - 2.0 * pad).max(1.0),
        };
        let g = Grid { count: self.count, cols, cell: Vec2::new(cell_w, self.row_height), pad };
        sel.truncate(self.count);
        let multi = self.multi;

        // Keyboard.
        if out.response.focused && ui.keys_allowed() {
            let page = vp_rect.map(|r| ((r.h / self.row_height).floor() as usize).max(1)).unwrap_or(10) * cols;
            let last = self.count.saturating_sub(1);
            let mut moved = None;
            for k in ui.keys() {
                let m = k.mods;
                let cur = sel.cursor;
                let base = cur.unwrap_or(0);
                let to = match k.key {
                    Key::Down => Some(if cur.is_none() { 0 } else { (base + cols).min(last) }),
                    Key::Up => Some(if cur.is_none() { 0 } else { base.saturating_sub(cols) }),
                    Key::Right if cols > 1 => Some((base + 1).min(last)),
                    Key::Left if cols > 1 => Some(base.saturating_sub(1)),
                    Key::Home => Some(0),
                    Key::End => Some(last),
                    Key::PageDown => Some((base + page).min(last)),
                    Key::PageUp => Some(base.saturating_sub(page)),
                    _ => None,
                };
                if let Some(to) = to.filter(|_| self.count > 0 && !m.alt) {
                    ui.take_key(k.key, m);
                    if m.shift && multi {
                        sel.extend_to(to);
                    } else if m.ctrl && multi {
                        sel.cursor = Some(to);
                    } else {
                        sel.select_only(to);
                    }
                    moved = Some(to);
                    continue;
                }
                match k.key {
                    Key::Enter if m == Mods::NONE => {
                        if let Some(c) = cur {
                            ui.take_key(Key::Enter, m);
                            out.activated = Some(c);
                        }
                    }
                    Key::Char(' ') if m == Mods::CTRL && multi => {
                        if let Some(c) = cur {
                            ui.take_key(k.key, m);
                            sel.toggle(c);
                        }
                    }
                    Key::Char(' ') if m == Mods::NONE => {
                        if let Some(c) = cur {
                            ui.take_key(k.key, m);
                            sel.select_only(c);
                        }
                    }
                    Key::Char('a') if m == Mods::CTRL && multi => {
                        ui.take_key(k.key, m);
                        sel.select_all(self.count);
                    }
                    Key::Menu => {
                        if let Some(c) = cur {
                            ui.take_key(k.key, m);
                            if !sel.is_selected(c) {
                                sel.select_only(c);
                            }
                            let r = g.rect(c);
                            let origin = vp_rect.map(|v| v.min()).unwrap_or_default() - ui.scroll_state(scroll_id).offset();
                            out.context = Some((Some(c), origin + Vec2::new(r.x + 16.0, r.bottom())));
                        }
                    }
                    _ => {}
                }
            }
            if let (Some(to), Some(v)) = (moved, vp_rect) {
                let r = g.rect(to);
                ui.scroll_state(scroll_id).reveal_y(r.y - pad, r.bottom() + pad, v.h);
            }
        }

        if let (Some(i), Some(v)) = (self.reveal.filter(|&i| i < self.count), vp_rect) {
            let r = g.rect(i);
            ui.scroll_state(scroll_id).reveal_y(r.y - pad, r.bottom() + pad, v.h);
        }

        // Rubber band on empty space.
        let to_content = |ui: &mut Ui, p: Vec2| -> Vec2 {
            let origin = vp_rect.map(|v| v.min()).unwrap_or_default();
            p - origin + ui.scroll_state(scroll_id).offset()
        };
        if let Some((p, _)) = ui.press_pos(id) {
            // A press on empty space: clear (unless Ctrl/Shift), maybe a band.
            if !ui.mods().ctrl && !ui.mods().shift {
                sel.clear();
            }
            let start = to_content(ui, p);
            let base = if ui.mods().ctrl { sel.selected.clone() } else { BTreeSet::new() };
            let st = ui.state::<BandState>(id);
            st.start = Some((start, base));
            st.end = start;
        }
        let band = {
            let dragging = out.response.dragging;
            let pointer = ui.pointer();
            let end = pointer.map(|p| to_content(ui, p));
            let st = ui.state::<BandState>(id);
            match (&st.start, dragging && multi, end) {
                (Some((start, base)), true, Some(end)) => {
                    st.end = end;
                    let r = Rect::from_min_max(Vec2::new(start.x.min(end.x), start.y.min(end.y)), Vec2::new(start.x.max(end.x), start.y.max(end.y)));
                    let mut items = base.clone();
                    items.extend(g.items_in(r));
                    sel.set(items);
                    Some(r)
                }
                _ => {
                    if !dragging {
                        st.start = None;
                    }
                    None
                }
            }
        };
        if out.response.secondary_clicked {
            out.context = Some((None, out.response.pointer.unwrap_or_default()));
        }

        let t = ui.theme().clone();
        let focused = out.response.focused;
        let mut n = ui.widget_node(id, "list", Some(&format!("list-{:x}", self.key.0 & 0xffffff)), self.layout, Sense::DRAG);
        n.focusable = true;
        n.deco.radius = t.radius;
        if ui.test.enabled() {
            n.value = Some(sel.selected().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
        }
        let count = self.count;
        let row_h = self.row_height;
        let grid = self.cell_width.is_some();
        let sel_snapshot = sel.clone();
        let mut events: Vec<(usize, Response)> = Vec::new();
        ui.add_container(n, |ui| {
            ScrollArea::vertical().with_id(scroll_id).content(Layout::column().padding(pad)).show_viewport(ui, |ui, vp| {
                let rows = g.rows();
                let first = (((vp.rect.y - pad) / row_h).floor().max(0.0) as usize).min(rows);
                let last = (((vp.rect.bottom() - pad) / row_h).ceil().max(0.0) as usize).min(rows);
                if first > 0 {
                    ui.add_node(Node::new(Layout::default().size(Size::FILL, Size::Px(first as f32 * row_h))));
                }
                for r in first..last {
                    let row = Node::new(Layout::row().size(Size::FILL, Size::Px(row_h)));
                    ui.add_container(row, |ui| {
                        for c in 0..cols {
                            let i = r * cols + c;
                            if i >= count {
                                break;
                            }
                            let rid = id.with(("item", i));
                            let resp = ui.interact(rid);
                            let selected = sel_snapshot.is_selected(i);
                            let cursor = sel_snapshot.cursor == Some(i) && focused && ui.focus_visible();
                            let hover = ui.animate_bool(rid.with("h"), resp.hovered);
                            let mut cell = if grid {
                                Node::new(Layout::column().size(Size::Px(cell_w), Size::Px(row_h)).padding(8.0).gap(6.0).align(crate::layout::Align::Center).justify(crate::layout::Justify::Center))
                            } else {
                                Node::new(Layout::row().size(Size::FILL, Size::Px(row_h)).padding((0.0, 12.0)).gap(12.0))
                            };
                            cell.id = Some(rid);
                            cell.sense = Sense::CLICK;
                            cell.kind = "item";
                            cell.label = Some(format!("item-{i}"));
                            cell.deco.radius = t.radius;
                            cell.deco.bg = if selected {
                                if focused { t.selection } else { t.control_active }
                            } else {
                                t.control_hover.fade(hover * 0.8)
                            };
                            if cursor {
                                cell.deco.border = t.focus_ring;
                                cell.deco.border_width = 1.5;
                            }
                            ui.add_container(cell, |ui| {
                                ui.push_id(i, |ui| item(ui, Item { index: i, selected, hovered: resp.hovered, cursor }));
                            });
                            if resp.clicked || resp.double_clicked || resp.secondary_clicked {
                                events.push((i, resp));
                            }
                        }
                    });
                }
                if last < rows {
                    ui.add_node(Node::new(Layout::default().size(Size::FILL, Size::Px((rows - last) as f32 * row_h))));
                }
            });
            // The selection rectangle, over the items.
            if let (Some(b), Some(v)) = (band, vp_rect) {
                let off = ui.scroll_state(scroll_id).offset();
                let r = b.translate(v.min() - off);
                let mut o = Node::new(Layout::default().size(Size::Px(0.0), Size::Px(0.0)));
                o.paint_over = Some(Box::new(move |p, _| {
                    p.push_clip(v);
                    p.rounded(r, 3.0, t.accent.with_alpha(0.16));
                    p.border(r, 3.0, 1.0, t.accent.with_alpha(0.7));
                    p.pop_clip();
                }));
                ui.add_node(o);
            }
        });
        for (i, resp) in events {
            if resp.clicked && !resp.double_clicked {
                sel.click(i, resp.mods, multi);
            }
            if resp.double_clicked {
                out.activated = Some(i);
            }
            if resp.secondary_clicked {
                if !sel.is_selected(i) {
                    sel.select_only(i);
                }
                out.context = Some((Some(i), resp.pointer.unwrap_or(resp.rect.center())));
            }
        }
        if let Some(i) = out.activated {
            ui.log(&format!("activate {i}"));
        }
        if *sel != before {
            out.changed = true;
            ui.log(&format!("selection {}", sel.selected().map(|i| i.to_string()).collect::<Vec<_>>().join(",")));
        }
        out
    }
}

impl Sizing for ListView {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_clicks() {
        let mut s = Selection::default();
        s.click(3, Mods::NONE, true);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![3]);
        s.click(6, Mods::SHIFT, true);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![3, 4, 5, 6]);
        s.click(1, Mods::SHIFT, true);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![1, 2, 3], "range from the same anchor");
        s.click(2, Mods::CTRL, true);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![1, 3]);
        s.click(5, Mods::CTRL, false);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![5], "single selection ignores modifiers");
        s.select_all(4);
        s.truncate(2);
        assert_eq!(s.selected().collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(s.cursor, Some(1));
    }

    #[test]
    fn grid_band_hits() {
        let g = Grid { count: 10, cols: 4, cell: Vec2::new(100.0, 50.0), pad: 6.0 };
        assert_eq!(g.rows(), 3);
        assert_eq!(g.rect(5), Rect::new(106.0, 56.0, 100.0, 50.0));
        let hit: Vec<usize> = g.items_in(Rect::new(150.0, 30.0, 100.0, 40.0)).into_iter().collect();
        assert_eq!(hit, vec![1, 2, 5, 6]);
        let tail: Vec<usize> = g.items_in(Rect::new(0.0, 120.0, 400.0, 100.0)).into_iter().collect();
        assert_eq!(tail, vec![8, 9], "past the last item there is nothing");
    }
}
