//! The keyboard-shortcut cheat sheet (Super+/): a modal glass panel listing
//! the live binding table by group, with type-to-filter search. Typing a
//! filter selects the first match, Enter runs it, and clicking a row runs
//! its action too, so the sheet doubles as a launcher.
//!
//! Rows are built from the binding table (bindings with the same group and
//! label share a row; runs of digit keys collapse to "1–4") plus a few
//! built-in gestures that are not configurable. Drawing and the measured
//! layout are in [`super::render`]; it stores the content geometry back here
//! so keyboard scrolling knows its limits.

use std::time::Duration;

use smithay::input::keyboard::{keysyms, Keysym, ModifiersState};

use super::Hit;
use crate::anim::{Spring, SpringParams};
use crate::input::{Action, BTN_LEFT};
use crate::keybind::KeyBindings;
use crate::state::Lumen;

/// Gestures handled outside the binding table, shown for completeness:
/// (group, label, key caps). A row with the same group and label as a
/// binding gets these caps first.
const FIXED: [(&str, &str, &[&str]); 4] = [
    ("Windows", "Move window", &["Super", "Drag"]),
    ("Windows", "Resize window", &["Super", "Right-drag"]),
    ("System", "Activities overview", &["Super"]),
    ("System", "Text console", &["Ctrl", "Alt", "F1–F12"]),
];

#[derive(Debug, Clone, PartialEq)]
pub struct SheetRow {
    pub group: String,
    pub label: String,
    /// Alternative chords, each a list of key caps.
    pub chords: Vec<Vec<String>>,
    /// What clicking the row (or Enter) does; None for gestures and rows
    /// that merge different actions (workspace 1–4).
    pub action: Option<Action>,
    /// Lower-case text that searches match against.
    haystack: String,
}

/// Collapse chords that differ only in a run of consecutive digits
/// (Super+1, Super+2, Super+3, Super+4 → Super+1–4).
pub fn compact(chords: Vec<Vec<String>>) -> Vec<Vec<String>> {
    let digit = |c: &Vec<String>| c.last().and_then(|k| k.parse::<u32>().ok().filter(|_| k.len() == 1));
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut i = 0;
    while i < chords.len() {
        let mut j = i + 1;
        if let Some(first) = digit(&chords[i]) {
            let prefix = &chords[i][..chords[i].len() - 1];
            while j < chords.len() && digit(&chords[j]) == Some(first + (j - i) as u32) && chords[j][..chords[j].len() - 1] == *prefix {
                j += 1;
            }
            if j - i >= 3 {
                let mut c = prefix.to_vec();
                c.push(format!("{first}–{}", first + (j - i - 1) as u32));
                out.push(c);
                i = j;
                continue;
            }
        }
        out.push(chords[i].clone());
        i += 1;
    }
    out
}

/// Sheet rows for a binding table, grouped (groups in order of first appearance).
pub fn rows(keys: &KeyBindings) -> Vec<SheetRow> {
    struct Acc {
        group: String,
        label: String,
        chords: Vec<Vec<String>>,
        actions: Vec<Action>,
    }
    let mut acc: Vec<Acc> = Vec::new();
    for b in &keys.bindings {
        let caps = b.chord.caps();
        match acc.iter_mut().find(|r| r.group == b.group && r.label == b.label) {
            Some(r) => {
                r.chords.push(caps);
                if !r.actions.contains(&b.action) {
                    r.actions.push(b.action.clone());
                }
            }
            None => acc.push(Acc { group: b.group.clone(), label: b.label.clone(), chords: vec![caps], actions: vec![b.action.clone()] }),
        }
    }
    for (group, label, caps) in FIXED {
        let caps: Vec<String> = caps.iter().map(|s| s.to_string()).collect();
        match acc.iter_mut().find(|r| r.group == group && r.label == label) {
            Some(r) => r.chords.insert(0, caps),
            None => acc.push(Acc { group: group.into(), label: label.into(), chords: vec![caps], actions: Vec::new() }),
        }
    }
    let mut groups: Vec<String> = Vec::new();
    for r in &acc {
        if !groups.contains(&r.group) {
            groups.push(r.group.clone());
        }
    }
    // Stable: rows keep their order within a group.
    acc.sort_by_key(|r| groups.iter().position(|g| *g == r.group));
    acc.into_iter()
        .map(|r| {
            let chords = compact(r.chords);
            let keys: Vec<String> = chords.iter().flat_map(|c| [c.join(" "), c.join("+")]).collect();
            let haystack = format!("{} {} {}", r.label, r.group, keys.join(" ")).to_lowercase();
            let action = if r.actions.len() == 1 { r.actions.into_iter().next() } else { None };
            SheetRow { group: r.group, label: r.label, chords, action, haystack }
        })
        .collect()
}

/// Indices of the rows matching `query`: every word must occur somewhere in
/// the row (label, group or keys).
pub fn filter(rows: &[SheetRow], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    rows.iter().enumerate().filter(|(_, r)| words.iter().all(|w| r.haystack.contains(w.as_str()))).map(|(i, _)| i).collect()
}

/// Greedy line breaking of chords (by width) into lines no wider than
/// `max_w`; a chord wider than that gets a line of its own.
pub fn pack(widths: &[f64], max_w: f64, gap: f64) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (i, &w) in widths.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if used + gap + w <= max_w => {
                line.push(i);
                used += gap + w;
            }
            _ => {
                lines.push(vec![i]);
                used = w;
            }
        }
    }
    lines
}

/// One item of the column flow: a group heading or a row. `glue` keeps it in
/// the same column as the next item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowItem {
    pub heading: bool,
    pub h: f64,
    pub glue: bool,
}

/// Flow items of a group: its heading stays with the first two rows and the
/// last row never sits alone at the top of a column.
pub fn group_items(row_heights: &[f64], head_h: f64) -> Vec<FlowItem> {
    let n = row_heights.len();
    let mut items = vec![FlowItem { heading: true, h: head_h, glue: n > 0 }];
    for (i, &h) in row_heights.iter().enumerate() {
        let glue = i + 1 < n && (i == 0 || i + 2 == n || n <= 3);
        items.push(FlowItem { heading: false, h, glue });
    }
    items
}

/// Where the flow put each item.
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    /// (column, y) per item.
    pub pos: Vec<(usize, f64)>,
    pub tallest: f64,
    /// Items (rows) that start a column in the middle of their group: the
    /// group's heading is repeated above them, in the `head_h` reserved there.
    pub continued: Vec<usize>,
}

/// Newspaper-style flow of headings and rows into `cols` columns of height
/// `cap`, in reading order, breaking only between glued runs. Groups are
/// `gap` apart.
pub fn flow(items: &[FlowItem], cols: usize, cap: f64, head_h: f64, gap: f64) -> Flow {
    let cols = cols.max(1);
    let mut out = Flow { pos: Vec::with_capacity(items.len()), tallest: 0.0, continued: Vec::new() };
    let (mut col, mut y) = (0usize, 0.0f64);
    let mut i = 0;
    while i < items.len() {
        let mut j = i;
        while items[j].glue && j + 1 < items.len() {
            j += 1;
        }
        let run_h: f64 = items[i..=j].iter().map(|it| it.h).sum();
        let gap_before = if items[i].heading && y > 0.0 { gap } else { 0.0 };
        if y > 0.0 && y + gap_before + run_h > cap + 1e-6 && col + 1 < cols {
            col += 1;
            y = 0.0;
            if !items[i].heading {
                out.continued.push(i);
                y = head_h;
            }
        } else {
            y += gap_before;
        }
        for it in &items[i..=j] {
            out.pos.push((col, y));
            y += it.h;
        }
        out.tallest = out.tallest.max(y);
        i = j + 1;
    }
    out
}

/// The smallest column height at which `items` flow into `cols` columns.
pub fn balanced_height(items: &[FlowItem], cols: usize, head_h: f64, gap: f64) -> f64 {
    let total: f64 = items.iter().map(|i| i.h + if i.heading { gap + head_h } else { 0.0 }).sum();
    let (mut lo, mut hi) = (0.0f64, total.max(1.0));
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if flow(items, cols, mid, head_h, gap).tallest <= mid + 1e-6 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    flow(items, cols, hi, head_h, gap).tallest
}

pub struct Sheet {
    pub open: bool,
    /// 0 = hidden, 1 = shown (fade and scale).
    pub anim: Spring,
    pub rows: Vec<SheetRow>,
    pub query: String,
    /// Indices into `rows` matching the query.
    pub visible: Vec<usize>,
    /// Keyboard selection (index into `visible`) while filtering.
    pub selected: Option<usize>,
    /// Scroll offset of the content (logical px), animated.
    pub scroll: Spring,
    /// Geometry of the last frame (logical): content height, viewport height
    /// and each visible row's extent in content coordinates.
    pub content_h: f64,
    pub view_h: f64,
    pub row_spans: Vec<(usize, f64, f64)>,
}

impl Sheet {
    pub fn new(rows: Vec<SheetRow>, anim: SpringParams, scroll: SpringParams) -> Sheet {
        let visible = (0..rows.len()).collect();
        let mut a = Spring::new(0.0, anim);
        a.set_target(1.0);
        Sheet {
            open: true,
            anim: a,
            rows,
            query: String::new(),
            visible,
            selected: None,
            scroll: Spring::new(0.0, scroll),
            content_h: 0.0,
            view_h: 0.0,
            row_spans: Vec::new(),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<SheetRow>) {
        self.rows = rows;
        let q = self.query.clone();
        self.set_query(q);
    }

    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.visible = filter(&self.rows, &self.query);
        self.selected = (!self.query.trim().is_empty() && !self.visible.is_empty()).then_some(0);
        self.scroll.set_target(0.0);
    }

    pub fn max_scroll(&self) -> f64 {
        (self.content_h - self.view_h).max(0.0)
    }

    pub fn scroll_to(&mut self, y: f64) {
        self.scroll.set_target(y.clamp(0.0, self.max_scroll()));
    }

    pub fn scroll_by(&mut self, dy: f64) {
        let y = self.scroll.target + dy;
        self.scroll_to(y);
    }

    /// The selected row's index into `rows`.
    pub fn selected_row(&self) -> Option<usize> {
        self.selected.and_then(|s| self.visible.get(s).copied())
    }

    /// Scroll so the selected row is in view.
    fn reveal_selected(&mut self) {
        let Some(row) = self.selected_row() else { return };
        let Some(&(_, top, bottom)) = self.row_spans.iter().find(|(r, _, _)| *r == row) else { return };
        let margin = 8.0;
        if top - margin < self.scroll.target {
            self.scroll_to(top - margin);
        } else if bottom + margin > self.scroll.target + self.view_h {
            self.scroll_to(bottom + margin - self.view_h);
        }
    }

    pub fn advance(&mut self, dt: Duration) {
        self.anim.step(dt);
        self.scroll.step(dt);
    }

    pub fn animating(&self) -> bool {
        !self.anim.is_settled() || !self.scroll.is_settled()
    }

    /// Fully faded out after closing.
    pub fn gone(&self) -> bool {
        !self.open && self.anim.value <= 0.01 && self.anim.is_settled()
    }
}

impl Lumen {
    pub fn sheet_open(&self) -> bool {
        self.shell.sheet.as_ref().is_some_and(|s| s.open)
    }

    pub fn shell_toggle_sheet(&mut self) {
        if self.sheet_open() {
            self.shell_close_sheet();
        } else {
            self.shell_open_sheet();
        }
    }

    pub fn shell_open_sheet(&mut self) {
        // Modal: other shell UI makes way.
        if self.shell.overview_open {
            self.shell_set_overview(false);
        }
        self.shell_switcher_cancel();
        self.shell.popover = None;
        let rows = rows(&self.keys);
        let a = &self.theme.animation;
        let (anim, scroll) = (self.theme.spring(a.popover, 0.001), self.theme.spring(a.overview, 0.05));
        match &mut self.shell.sheet {
            // Reopened while fading out: continue from where it is.
            Some(s) => {
                s.open = true;
                s.anim.set_target(1.0);
                s.set_rows(rows);
                s.set_query(String::new());
                s.scroll.snap(0.0);
            }
            None => self.shell.sheet = Some(Sheet::new(rows, anim, scroll)),
        }
        let focused = self.wm.focused;
        self.focus_window(focused);
        self.queue_redraw_all();
    }

    pub fn shell_close_sheet(&mut self) {
        let Some(s) = &mut self.shell.sheet else { return };
        if !s.open {
            return;
        }
        s.open = false;
        s.anim.set_target(0.0);
        let focused = self.wm.focused;
        self.focus_window(focused);
        let pos = self.pointer_pos;
        self.on_pointer_moved(pos, self.now().as_millis() as u32, None);
        self.queue_redraw_all();
    }

    /// The binding table changed: refresh an open sheet.
    pub fn shell_sheet_bindings_changed(&mut self) {
        let rows = rows(&self.keys);
        if let Some(s) = &mut self.shell.sheet {
            s.set_rows(rows);
        }
    }

    /// Run a row's action (click or Enter).
    fn sheet_activate(&mut self, row: usize) {
        let Some(action) = self.shell.sheet.as_ref().and_then(|s| s.rows.get(row)).and_then(|r| r.action.clone()) else { return };
        tracing::info!(action = %crate::keybind::action_name(&action), "shortcut run from the cheat sheet");
        self.shell_close_sheet();
        // The sheet is closed already; a toggle must not reopen it.
        if action != Action::ToggleShortcuts {
            self.run_action(action);
        }
    }

    /// A key while the sheet is open.
    pub fn sheet_key(&mut self, sym: Keysym, utf8: Option<String>, mods: ModifiersState) {
        let Some(s) = &mut self.shell.sheet else { return };
        let page = (s.view_h * 0.85).max(32.0);
        match sym.raw() {
            keysyms::KEY_Escape => {
                if s.query.is_empty() {
                    self.shell_close_sheet();
                } else {
                    s.set_query(String::new());
                }
            }
            keysyms::KEY_BackSpace => {
                let mut q = s.query.clone();
                q.pop();
                s.set_query(q);
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                if let Some(row) = s.selected_row() {
                    self.sheet_activate(row);
                }
            }
            keysyms::KEY_Up | keysyms::KEY_Down if s.selected.is_some() => {
                let n = s.visible.len();
                let cur = s.selected.unwrap_or(0);
                s.selected = Some(if sym.raw() == keysyms::KEY_Up { cur.saturating_sub(1) } else { (cur + 1).min(n.saturating_sub(1)) });
                s.reveal_selected();
            }
            keysyms::KEY_Up => s.scroll_by(-40.0),
            keysyms::KEY_Down => s.scroll_by(40.0),
            keysyms::KEY_Page_Up => s.scroll_by(-page),
            keysyms::KEY_Page_Down => s.scroll_by(page),
            keysyms::KEY_Home => s.scroll_to(0.0),
            keysyms::KEY_End => {
                let m = s.max_scroll();
                s.scroll_to(m);
            }
            _ => {
                if !mods.ctrl && !mods.alt && !mods.logo {
                    if let Some(text) = utf8.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
                        let q = format!("{}{text}", s.query);
                        s.set_query(q);
                    }
                }
            }
        }
        self.queue_redraw_all();
    }

    /// A click while the sheet is open (it is modal: always consumed).
    pub fn sheet_click(&mut self, button: u32, hit: Option<Hit>) {
        match hit {
            Some(Hit::SheetRow(row)) if button == BTN_LEFT => self.sheet_activate(row),
            Some(Hit::Sheet) | Some(Hit::SheetRow(_)) => {}
            _ => self.shell_close_sheet(),
        }
    }

    pub fn sheet_scroll(&mut self, v: f64) {
        if let Some(s) = &mut self.shell.sheet {
            // Wheel notches arrive as 15 px; a row per notch reads better.
            s.scroll_by(v * 2.5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybind::{parse_file, KeyBindings};

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn digit_runs_collapse() {
        let chords = vec![s(&["Super", "1"]), s(&["Super", "2"]), s(&["Super", "3"]), s(&["Super", "4"]), s(&["Super", "Page Up"])];
        assert_eq!(compact(chords), vec![s(&["Super", "1–4"]), s(&["Super", "Page Up"])]);
        // Two in a row stay apart; different modifiers do not merge.
        let chords = vec![s(&["Super", "1"]), s(&["Super", "2"]), s(&["Alt", "3"])];
        assert_eq!(compact(chords.clone()), chords);
    }

    #[test]
    fn rows_group_merge_and_include_gestures() {
        let rows = rows(&KeyBindings::builtin());
        let row = |label: &str| rows.iter().find(|r| r.label == label).unwrap_or_else(|| panic!("no row {label}"));
        assert_eq!(row("Terminal").chords, vec![s(&["Super", "Enter"]), s(&["Super", "T"]), s(&["Ctrl", "Alt", "T"])]);
        assert_eq!(row("Terminal").action, Some(Action::Spawn(vec!["lumen-term".into()])));
        assert_eq!(row("Go to workspace").chords, vec![s(&["Super", "1–4"])]);
        assert_eq!(row("Go to workspace").action, None);
        assert_eq!(row("Activities overview").chords[0], s(&["Super"]));
        assert_eq!(row("Move window").action, None);
        assert_eq!(row("Switch keyboard layout").chords, vec![s(&["Super", "Space"]), s(&["Alt", "Shift"])]);
        assert_eq!(row("Steam").chords, vec![s(&["Super", "G"])]);
        assert_eq!(row("Brightness up").chords, vec![s(&["Brightness Up"])]);
        // Groups are contiguous, in order of first appearance.
        let groups: Vec<&str> = rows.iter().map(|r| r.group.as_str()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for g in &groups {
            if seen.last() != Some(g) {
                assert!(!seen.contains(g), "group {g} split: {groups:?}");
                seen.push(g);
            }
        }
        assert_eq!(seen, vec!["Apps", "Windows", "Workspaces", "System"]);
    }

    #[test]
    fn filtering_matches_labels_groups_and_keys() {
        let mut keys = KeyBindings::builtin();
        keys.apply(parse_file("[[bind]]\nkeys = \"Super+J\"\naction = \"spawn\"\ncommand = \"gimp\"\nlabel = \"GIMP\"\ngroup = \"Apps\"\n").unwrap());
        let rows = rows(&keys);
        let labels = |q: &str| filter(&rows, q).into_iter().map(|i| rows[i].label.clone()).collect::<Vec<_>>();
        assert_eq!(labels("youtube"), vec!["YouTube"]);
        assert_eq!(labels("TERM"), vec!["Terminal"]);
        assert!(labels("super+y").contains(&"YouTube".to_string()));
        assert!(labels("volume").len() == 2 && labels("volume").contains(&"Volume up".to_string()));
        // The user's binding joins its group, not the end of the list.
        let gimp = rows.iter().position(|r| r.label == "GIMP").unwrap();
        assert!(rows[..gimp].iter().all(|r| r.group == "Apps") && rows[gimp + 1].group == "Windows");
        assert!(labels("zzz").is_empty());
        assert_eq!(filter(&rows, "").len(), rows.len());
    }

    #[test]
    fn chord_lines() {
        assert_eq!(pack(&[50.0, 40.0, 60.0], 100.0, 8.0), vec![vec![0, 1], vec![2]]);
        assert_eq!(pack(&[150.0, 20.0], 100.0, 8.0), vec![vec![0], vec![1]]);
    }

    #[test]
    fn columns_balance_and_keep_headings_with_rows() {
        let group = |n: usize| group_items(&vec![20.0; n], 10.0);
        // Headings stay with two rows; a last row is never alone.
        let glue: Vec<bool> = group(5).iter().map(|i| i.glue).collect();
        assert_eq!(glue, vec![true, true, false, false, true, false]);
        assert!(group(3).iter().take(3).all(|i| i.glue));
        // Two groups of three rows in two columns: one group each.
        let items: Vec<FlowItem> = group(3).into_iter().chain(group(3)).collect();
        let h = balanced_height(&items, 2, 10.0, 5.0);
        let f = flow(&items, 2, h, 10.0, 5.0);
        assert_eq!(f.tallest, 70.0);
        assert_eq!(f.pos[4], (1, 0.0));
        assert!(f.continued.is_empty());
        // A long group splits; the continuation gets its heading repeated.
        let items = group(6);
        let h = balanced_height(&items, 2, 10.0, 5.0);
        let f = flow(&items, 2, h, 10.0, 5.0);
        assert_eq!(f.pos, vec![(0, 0.0), (0, 10.0), (0, 30.0), (0, 50.0), (1, 10.0), (1, 30.0), (1, 50.0)]);
        assert_eq!(f.continued, vec![4]);
        assert_eq!(f.tallest, 70.0);
        // Everything stays in one column when it fits.
        let f = flow(&items, 3, 1000.0, 10.0, 5.0);
        assert!(f.pos.iter().all(|(c, _)| *c == 0));
    }

    #[test]
    fn selection_follows_the_filter() {
        let params = SpringParams::new(400.0, 1.0, 0.001);
        let mut sheet = Sheet::new(rows(&KeyBindings::builtin()), params, params);
        assert_eq!(sheet.selected, None);
        sheet.set_query("calc".into());
        assert_eq!(sheet.selected_row().map(|r| sheet.rows[r].label.as_str()), Some("Calculator"));
        sheet.set_query("zzz".into());
        assert_eq!(sheet.selected, None);
    }
}
