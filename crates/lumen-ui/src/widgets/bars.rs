//! Bars apps put around their content: a path bar (breadcrumbs), sortable
//! column headers over list rows (tables), document tabs, and a field that
//! records a keyboard shortcut (settings).

use std::hash::Hash;
use std::path::{Path, PathBuf};

use crate::anim::SpringParams;
use crate::files;
use crate::geom::Vec2;
use crate::icons::Icon;
use crate::id::Id;
use crate::input::{Key, Mods, Shortcut};
use crate::layout::{Layout, Size, Sizing};
use crate::text::{TextAlign, TextStyle, Weight, Wrap};
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

use super::{Button, Frame, Label};

// -- Path bar ------------------------------------------------------------------------------

impl Ui {
    /// The folders of `path` as buttons, the last four levels (the folders
    /// before them behind a "…" button), with the home folder named "Home"
    /// and the root "Computer". Returns the folder clicked.
    pub fn path_bar(&mut self, name: &str, path: &Path) -> Option<PathBuf> {
        let home = files::home();
        let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
        let mut p = path;
        loop {
            if p == home && home != Path::new("/") {
                crumbs.push(("Home".into(), p.to_path_buf()));
                break;
            }
            match p.file_name() {
                Some(n) => crumbs.push((n.to_string_lossy().into_owned(), p.to_path_buf())),
                None => {
                    crumbs.push(("Computer".into(), p.to_path_buf()));
                    break;
                }
            }
            match p.parent() {
                Some(parent) => p = parent,
                None => break,
            }
        }
        crumbs.reverse();
        const MAX: usize = 4;
        let skipped = crumbs.len().saturating_sub(MAX);
        let mut go = None;
        self.push_id(("path-bar", name), |ui| {
            Frame::row().fill_width().gap(0.0).clip().show(ui, |ui| {
                let t = ui.theme().clone();
                if skipped > 0 {
                    let target = crumbs[skipped - 1].1.clone();
                    if Button::icon_only(Icon::MoreHorizontal).tooltip(files::display_path(&target)).id("more").show(ui).clicked {
                        go = Some(target);
                    }
                }
                let last = crumbs.len() - 1;
                for (i, (label, path)) in crumbs.iter().enumerate().skip(skipped) {
                    if i > skipped || skipped > 0 {
                        ui.icon_colored(Icon::ChevronRight, 14.0, t.text_dim);
                    }
                    let b = Button::new(label.clone()).flat().id(("crumb", i)).toggled(i == last).max_width(220.0);
                    if b.show(ui).clicked && i != last {
                        go = Some(path.clone());
                    }
                }
            });
        });
        if let Some(p) = &go {
            self.log(&format!("path {name} {}", p.display()));
        }
        go
    }
}

// -- Column headers -------------------------------------------------------------------------

/// A column of a table: its title, width and alignment.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub title: String,
    pub width: Size,
    pub align: TextAlign,
}

impl Column {
    /// A left-aligned column; give one column `Size::FILL`, the others
    /// fixed widths.
    pub fn new(title: impl Into<String>, width: Size) -> Column {
        Column { title: title.into(), width, align: TextAlign::Start }
    }

    /// Right-aligned (numbers, sizes).
    pub fn end(mut self) -> Column {
        self.align = TextAlign::End;
        self
    }
}

/// Which column a table is sorted by, and in which direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SortOrder {
    pub column: usize,
    pub descending: bool,
}

impl SortOrder {
    /// Apply the direction to an ascending comparison.
    pub fn apply(&self, o: std::cmp::Ordering) -> std::cmp::Ordering {
        if self.descending { o.reverse() } else { o }
    }
}

/// Clickable column titles over a [`super::ListView`]: a click sorts by a
/// column, a second click reverses the order. The titles line up with list
/// rows built from labels of the same widths (rows put 12 px between
/// their children; `leading` is the space of a leading icon and its gap).
///
/// ```no_run
/// # use lumen_ui::prelude::*;
/// # fn f(ui: &mut Ui, sort: &mut SortOrder, sel: &mut Selection, procs: &mut Vec<(String, f32)>) {
/// let cols = [Column::new("Name", Size::FILL), Column::new("CPU", Size::Px(70.0)).end()];
/// if ColumnHeaders::new("procs", &cols).show(ui, sort) {
///     procs.sort_by(|a, b| sort.apply(match sort.column {
///         0 => a.0.cmp(&b.0),
///         _ => a.1.total_cmp(&b.1),
///     }));
/// }
/// ListView::new("procs", procs.len(), 32.0).show(ui, sel, |ui, item| {
///     let (name, cpu) = &procs[item.index];
///     Label::new(name.clone()).width(cols[0].width).show(ui);
///     Label::new(format!("{cpu:.1}%")).width(cols[1].width).align(TextAlign::End).show(ui);
/// });
/// # }
/// ```
#[must_use = "column headers do nothing until shown"]
pub struct ColumnHeaders<'a> {
    key: Id,
    columns: &'a [Column],
    leading: f32,
}

impl<'a> ColumnHeaders<'a> {
    pub fn new(key: impl Hash, columns: &'a [Column]) -> ColumnHeaders<'a> {
        ColumnHeaders { key: Id(0).with(key), columns, leading: 0.0 }
    }

    /// Space before the first column (rows that start with an icon: its
    /// size plus the 12 px gap).
    pub fn leading(mut self, px: f32) -> ColumnHeaders<'a> {
        self.leading = px;
        self
    }

    /// Show the titles; returns true when the order changed.
    pub fn show(self, ui: &mut Ui, sort: &mut SortOrder) -> bool {
        let t = ui.theme().clone();
        let mut changed = false;
        let key = self.key;
        let (cols, leading) = (self.columns, self.leading);
        Frame::row().fill_width().height(Size::Px(28.0)).gap(12.0).padding((0.0, 18.0)).show(ui, |ui| {
            if leading > 0.0 {
                ui.space(leading - 12.0);
            }
            for (i, col) in cols.iter().enumerate() {
                let active = sort.column == i;
                let (_, r) = Frame::row().id((key, "sort", i)).sense(Sense::CLICK).width(col.width).height(Size::FILL).gap(4.0).label(&col.title).show_with_response(ui, |ui| {
                    if col.align == TextAlign::End {
                        ui.spacer();
                    }
                    let c = if active { t.text } else { t.text_dim };
                    Label::new(col.title.clone()).size(t.size_small()).weight(Weight::Medium).color(c).show(ui);
                    if active {
                        ui.icon_colored(if sort.descending { Icon::ChevronUp } else { Icon::ChevronDown }, 12.0, t.text_dim);
                    }
                });
                if r.clicked {
                    if active {
                        sort.descending = !sort.descending;
                    } else {
                        *sort = SortOrder { column: i, descending: false };
                    }
                    changed = true;
                }
            }
        });
        ui.separator();
        if changed {
            let dir = if sort.descending { "descending" } else { "ascending" };
            ui.log(&format!("sort {} {dir}", cols[sort.column].title));
        }
        changed
    }
}

// -- Document tabs --------------------------------------------------------------------------

/// A document tab of a [`Ui::tab_bar`].
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub title: String,
    pub icon: Option<Icon>,
    /// Unsaved changes: a dot where the close button is (until hovered).
    pub modified: bool,
}

impl Tab {
    pub fn new(title: impl Into<String>) -> Tab {
        Tab { title: title.into(), icon: None, modified: false }
    }

    pub fn icon(mut self, icon: Icon) -> Tab {
        self.icon = Some(icon);
        self
    }

    pub fn modified(mut self, on: bool) -> Tab {
        self.modified = on;
        self
    }
}

/// What happened in a tab bar this frame. Closing is the app's decision
/// (it may ask about unsaved changes first), so the bar only reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TabBarResponse {
    /// Another tab was activated (`active` is updated).
    pub changed: bool,
    /// Close this tab: its close button, a middle click or Ctrl+W.
    pub close: Option<usize>,
    /// The new-tab button or Ctrl+T.
    pub new_tab: bool,
    /// Right-clicked tab and where to open its menu.
    pub context: Option<(usize, Vec2)>,
}

impl Ui {
    /// Document tabs choosing `active` among `tabs`, with a new-tab button
    /// when `add` is set. Keys (unless a menu or dialog has the keyboard):
    /// Ctrl+Page Up/Down switch, Ctrl+W closes, Ctrl+T opens a new tab.
    pub fn tab_bar(&mut self, name: &str, active: &mut usize, tabs: &[Tab], add: bool) -> TabBarResponse {
        let mut out = TabBarResponse::default();
        let n = tabs.len();
        let before = *active;
        if n > 0 {
            if self.shortcut(Shortcut::new(Mods::CTRL, Key::PageDown)) {
                *active = (*active + 1) % n;
            }
            if self.shortcut(Shortcut::new(Mods::CTRL, Key::PageUp)) {
                *active = (*active + n - 1) % n;
            }
            if self.shortcut(Shortcut::ctrl('w')) {
                out.close = Some((*active).min(n - 1));
            }
        }
        if add && self.shortcut(Shortcut::ctrl('t')) {
            out.new_tab = true;
        }
        let t = self.theme().clone();
        let id = self.make_id(("tab-bar", name));
        let mut bar = self.widget_node(id, "tabbar", Some(name), Layout::row().width(Size::FILL).height(Size::Px(44.0)).padding((6.0, 8.0)).gap(4.0), Sense::NONE);
        bar.deco.bg = t.header_bg;
        bar.clip = true;
        self.add_container(bar, |ui| {
            for (i, tab) in tabs.iter().enumerate() {
                let tid = id.with(("tab", i));
                let close_id = tid.with("close");
                let r = ui.interact(tid);
                let cr = ui.interact(close_id);
                let selected = i == *active;
                if r.clicked && !cr.clicked {
                    *active = i;
                }
                if r.middle_clicked || cr.clicked {
                    out.close = Some(i);
                }
                if r.secondary_clicked {
                    out.context = Some((i, r.pointer.unwrap_or(r.rect.center())));
                }
                let hover = ui.animate_bool(tid.with("h"), r.hovered || cr.hovered);
                let sel = ui.animate(tid.with("s"), if selected { 1.0 } else { 0.0 }, SpringParams::FAST);
                let mut node = ui.widget_node(tid, "tab", Some(&tab.title), Layout::row().width(Size::FILL).height(Size::FILL).max_width(240.0).min_width(72.0).padding((0.0, 6.0, 0.0, 12.0)).gap(8.0), Sense::CLICK);
                node.deco.radius = t.radius;
                node.deco.bg = t.control_hover.fade(hover * 0.6).mix(t.surface_hover.mix(t.text, 0.06), sel);
                if ui.test.enabled() {
                    node.value = Some(format!("{}{}", if selected { "on" } else { "off" }, if tab.modified { " modified" } else { "" }));
                }
                let fg = if selected { t.text } else { t.text_dim };
                ui.add_container(node, |ui| {
                    if let Some(icon) = tab.icon {
                        ui.icon_colored(icon, 16.0, fg);
                    }
                    let mut l = Node::new(Layout::default().width(Size::FILL));
                    l.measure = Measure::Text { text: tab.title.clone(), style: TextStyle::new(t.font_size).weight(Weight::Medium), wrap: Wrap::Ellipsize };
                    l.visual = Visual::Text { color: fg, align: TextAlign::Start };
                    ui.add_node(l);
                    // The close button; a modified tab shows a dot there until
                    // the tab is hovered.
                    let ch = ui.animate_bool(close_id.with("h"), cr.hovered);
                    let dot = tab.modified && !r.hovered && !cr.hovered;
                    let mut c = Node::new(Layout::default().size(Size::Px(22.0), Size::Px(22.0)));
                    c.id = Some(close_id);
                    c.sense = Sense::CLICK;
                    c.kind = "button";
                    c.label = Some(format!("close {}", tab.title));
                    c.deco.radius = 11.0;
                    c.deco.bg = t.control_active.fade(ch);
                    let show_x = !dot && (selected || r.hovered || cr.hovered);
                    if show_x {
                        c.visual = Visual::Icon { icon: Icon::Close, size: 14.0, color: fg };
                    }
                    let accent = t.text_dim;
                    if dot {
                        c.paint = Some(Box::new(move |p, r| p.rounded(r.centered(8.0, 8.0), 4.0, accent)));
                    }
                    ui.add_node(c);
                });
            }
            if add && Button::icon_only(Icon::Plus).tooltip("New Tab (Ctrl+T)").id((name, "new-tab")).show(ui).clicked {
                out.new_tab = true;
            }
        });
        if n > 0 {
            *active = (*active).min(n - 1);
        }
        if *active != before {
            out.changed = true;
            self.log(&format!("tab-select {name} {}", tabs[*active].title));
        }
        if let Some(i) = out.close {
            self.log(&format!("tab-close {name} {}", tabs[i].title));
        }
        if out.new_tab {
            self.log(&format!("tab-new {name}"));
        }
        out
    }
}

// -- Shortcut field ----------------------------------------------------------------------------

#[derive(Default)]
struct CaptureState {
    capturing: bool,
}

impl Ui {
    /// A field showing a keyboard shortcut ("Ctrl+Alt+T", or "Disabled");
    /// activating it records the next key combination: Escape cancels,
    /// Backspace disables. While it records, app shortcuts see no keys.
    pub fn shortcut_input(&mut self, name: &str, value: &mut Option<Shortcut>) -> Response {
        let id = self.make_id(("shortcut", name));
        let mut r = self.interact(id);
        let mut capturing = self.state::<CaptureState>(id).capturing && r.focused;
        if !capturing && (r.clicked || (r.focused && self.keys_allowed() && (self.key_pressed(Key::Enter) || self.key_pressed(Key::SPACE)))) {
            capturing = true;
            self.request_focus(id);
        } else if capturing && self.keys_allowed() {
            self.mark_key_capture();
            let mut done = None;
            self.consume_keys(|_, k| {
                if done.is_some() {
                    return false;
                }
                done = match (k.key, k.mods) {
                    (Key::Escape, Mods::NONE) => Some(None),
                    (Key::Backspace, Mods::NONE) => Some(Some(None)),
                    // A plain character is typing, not a shortcut.
                    (Key::Char(_), m) if !m.command() => return true,
                    (Key::Tab | Key::Enter, m) if !m.command() => return true,
                    (key, mods) => Some(Some(Some(Shortcut { mods, key }))),
                };
                true
            });
            match done {
                Some(None) => capturing = false,
                Some(Some(v)) => {
                    capturing = false;
                    if *value != v {
                        *value = v;
                        r.changed = true;
                        self.log(&format!("shortcut {name} {}", v.map(|s| s.label()).unwrap_or_else(|| "disabled".into())));
                    }
                }
                None => {}
            }
        }
        self.state::<CaptureState>(id).capturing = capturing;
        let t = self.theme().clone();
        let hover = self.animate_bool(id.with("h"), r.hovered);
        let rec = self.animate_bool(id.with("rec"), capturing);
        let (text, color) = match (capturing, *value) {
            (true, _) => ("Press a shortcut…".to_string(), t.accent),
            (false, Some(s)) => (s.label(), t.text),
            (false, None) => ("Disabled".to_string(), t.text_dim),
        };
        let mut n = self.widget_node(id, "shortcut", Some(name), Layout::row().height(Size::Px(32.0)).min_width(150.0).padding((0.0, 12.0)).justify(crate::layout::Justify::Center), Sense::CLICK);
        n.focusable = true;
        n.deco.bg = t.control.mix(t.control_hover, hover);
        n.deco.radius = t.radius;
        n.deco.border = t.focus_ring;
        n.deco.border_width = 2.0 * rec;
        if self.test.enabled() {
            n.value = Some(if capturing { "recording".into() } else { value.map(|s| s.label()).unwrap_or_else(|| "disabled".into()) });
        }
        self.add_container(n, |ui| {
            let mut l = Node::new(Layout::default());
            l.measure = Measure::Text { text, style: TextStyle::new(t.font_size).weight(Weight::Medium), wrap: Wrap::Ellipsize };
            l.visual = Visual::Text { color, align: TextAlign::Center };
            ui.add_node(l);
        });
        r
    }
}
