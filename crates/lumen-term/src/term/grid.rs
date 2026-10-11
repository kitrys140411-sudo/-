//! The cell grid: the visible screen plus scrollback history, with region
//! scrolling and reflow on resize.

use std::collections::VecDeque;

use super::cell::{attr, Cell};

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// The line continues on the next row (soft wrap): selection, search and
    /// reflow treat them as one line.
    pub wrapped: bool,
    /// Grapheme continuations (combining marks, ZWJ sequences) by column, for
    /// cells with the EXTRA attribute. Rare, so kept out of the cells.
    pub extras: Vec<(u16, String)>,
}

impl Row {
    pub fn new(cols: usize, blank: Cell) -> Row {
        Row { cells: vec![blank; cols], wrapped: false, extras: Vec::new() }
    }

    pub fn reset(&mut self, blank: Cell) {
        self.cells.fill(blank);
        self.wrapped = false;
        self.extras.clear();
    }

    /// The rest of the grapheme starting at `col` (after its base char).
    pub fn extra(&self, col: usize) -> Option<&str> {
        self.extras.iter().find(|(c, _)| *c as usize == col).map(|(_, s)| s.as_str())
    }

    pub fn push_extra(&mut self, col: usize, c: char) {
        if let Some((_, s)) = self.extras.iter_mut().find(|(x, _)| *x as usize == col) {
            // Bound pathological inputs (endless combining marks).
            if s.len() < 64 {
                s.push(c);
            }
        } else {
            self.extras.push((col as u16, c.to_string()));
        }
        self.cells[col].attrs |= attr::EXTRA;
    }

    /// Forget extras of columns `from..to` (their cells are being replaced).
    pub fn clear_extras(&mut self, from: usize, to: usize) {
        if self.extras.is_empty() {
            return;
        }
        self.extras.retain(|(c, _)| (*c as usize) < from || (*c as usize) >= to);
    }

    /// Move extras along with cells shifted by `delta` columns in `from..`.
    pub fn shift_extras(&mut self, from: usize, delta: isize, cols: usize) {
        if self.extras.is_empty() {
            return;
        }
        self.extras.retain_mut(|(c, _)| {
            if (*c as usize) < from {
                return true;
            }
            let n = *c as isize + delta;
            if n < from as isize || n >= cols as isize {
                return false;
            }
            *c = n as u16;
            true
        });
    }

    /// Number of columns up to the last non-empty cell.
    pub fn content_len(&self) -> usize {
        self.cells.iter().rposition(|c| !c.is_empty()).map(|i| i + 1).unwrap_or(0)
    }

    /// The text of `from..to` (wide spacers skipped, graphemes complete).
    pub fn text(&self, from: usize, to: usize) -> String {
        let mut s = String::new();
        for x in from..to.min(self.cells.len()) {
            let c = &self.cells[x];
            if c.has(attr::SPACER) {
                continue;
            }
            s.push(c.ch);
            if c.has(attr::EXTRA) {
                if let Some(e) = self.extra(x) {
                    s.push_str(e);
                }
            }
        }
        s
    }

    fn resize(&mut self, cols: usize, blank: Cell) {
        if cols < self.cells.len() {
            // Do not leave half of a wide character behind.
            if cols > 0 && self.cells[cols - 1].has(attr::WIDE) {
                self.cells[cols - 1] = blank;
            }
            self.clear_extras(cols, usize::MAX);
        }
        self.cells.resize(cols, blank);
    }
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// History followed by the screen (the last `rows` lines).
    lines: VecDeque<Row>,
    pub max_history: usize,
    /// Lines dropped from the top of the history so far: line `i` of `lines`
    /// has the stable absolute number `dropped + i`.
    pub dropped: u64,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, max_history: usize) -> Grid {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let lines = (0..rows).map(|_| Row::new(cols, Cell::default())).collect();
        Grid { cols, rows, lines, max_history, dropped: 0 }
    }

    pub fn history(&self) -> usize {
        self.lines.len() - self.rows
    }

    /// Screen row `y` (0 = top of the screen).
    #[inline]
    pub fn row(&self, y: usize) -> &Row {
        let h = self.history();
        &self.lines[h + y]
    }

    #[inline]
    pub fn row_mut(&mut self, y: usize) -> &mut Row {
        let h = self.history();
        &mut self.lines[h + y]
    }

    /// Any line: 0 = oldest history line, history() = top of the screen.
    pub fn line(&self, i: usize) -> Option<&Row> {
        self.lines.get(i)
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    /// Absolute number of the first line still stored.
    pub fn first_abs(&self) -> u64 {
        self.dropped
    }

    /// Scroll `top..=bottom` up by `n`: lines leave at the top (into history
    /// when the region is the whole screen and `history` is set) and blank
    /// lines enter at the bottom.
    pub fn scroll_up(&mut self, top: usize, bottom: usize, n: usize, blank: Cell, history: bool) {
        let n = n.min(bottom + 1 - top);
        if n == 0 {
            return;
        }
        if top == 0 && bottom == self.rows - 1 && history && self.max_history > 0 {
            for _ in 0..n {
                // Reuse the oldest line's allocation when history is full.
                let row = if self.history() >= self.max_history {
                    self.dropped += 1;
                    let mut r = self.lines.pop_front().expect("lines");
                    r.reset(blank);
                    if r.cells.len() != self.cols {
                        r = Row::new(self.cols, blank);
                    }
                    r
                } else {
                    Row::new(self.cols, blank)
                };
                self.lines.push_back(row);
            }
            return;
        }
        let h = self.history();
        let (a, b) = (h + top, h + bottom);
        for _ in 0..n {
            let mut r = self.lines.remove(a).expect("row");
            r.reset(blank);
            self.lines.insert(b, r);
        }
    }

    /// Scroll `top..=bottom` down by `n` (blank lines enter at the top).
    pub fn scroll_down(&mut self, top: usize, bottom: usize, n: usize, blank: Cell) {
        let n = n.min(bottom + 1 - top);
        let h = self.history();
        let (a, b) = (h + top, h + bottom);
        for _ in 0..n {
            let mut r = self.lines.remove(b).expect("row");
            r.reset(blank);
            self.lines.insert(a, r);
        }
    }

    pub fn clear_history(&mut self) {
        let h = self.history();
        self.lines.drain(..h);
        self.dropped += h as u64;
    }

    /// Resize without reflow (alternate screen, or only the height changed).
    /// Returns how many lines the content moved up (cursor adjustment).
    pub fn resize_simple(&mut self, cols: usize, rows: usize, cursor_y: usize, blank: Cell) -> isize {
        let cols = cols.max(1);
        let rows = rows.max(1);
        for r in &mut self.lines {
            r.resize(cols, blank);
        }
        self.cols = cols;
        let mut moved = 0isize;
        if rows < self.rows {
            // Drop empty lines below the cursor first, then push lines from the
            // top into history so the cursor stays on screen.
            let mut excess = self.rows - rows;
            while excess > 0 && self.rows - 1 > cursor_y && self.row(self.rows - 1).content_len() == 0 {
                self.lines.pop_back();
                self.rows -= 1;
                excess -= 1;
            }
            self.rows -= excess;
            moved = excess as isize;
            if self.max_history == 0 {
                self.lines.drain(..self.history());
            }
            self.trim_history();
        } else if rows > self.rows {
            // Pull lines back from history, then add blank lines below.
            let add = rows - self.rows;
            let pull = add.min(self.history());
            self.rows += pull;
            moved = -(pull as isize);
            for _ in pull..add {
                self.lines.push_back(Row::new(cols, blank));
                self.rows += 1;
            }
        }
        moved
    }

    fn trim_history(&mut self) {
        while self.history() > self.max_history {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    /// Resize with reflow: soft-wrapped lines are re-wrapped at the new
    /// width. `cursor` (screen coordinates) is tracked through the reflow and
    /// returned in new screen coordinates.
    pub fn resize_reflow(&mut self, cols: usize, rows: usize, cursor: (usize, usize), blank: Cell) -> (usize, usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols {
            let moved = self.resize_simple(cols, rows, cursor.1, blank);
            let y = (cursor.1 as isize - moved).clamp(0, rows as isize - 1) as usize;
            return (cursor.0.min(cols - 1), y);
        }
        // Lines below the cursor that are empty are not content.
        let h = self.history();
        let cursor_line = h + cursor.1;
        let mut last = self.lines.len();
        while last > cursor_line + 1 && self.lines[last - 1].content_len() == 0 {
            last -= 1;
        }
        // Join soft-wrapped rows into logical lines, remembering the cursor.
        let old: Vec<Row> = self.lines.drain(..).take(last).collect();
        let mut logical: Vec<(Vec<Cell>, Vec<(usize, String)>)> = Vec::new();
        let mut cursor_at: Option<(usize, usize)> = None; // (logical line, cell offset)
        let mut cur: (Vec<Cell>, Vec<(usize, String)>) = (Vec::new(), Vec::new());
        for (i, row) in old.into_iter().enumerate() {
            let base = cur.0.len();
            if i == cursor_line {
                cursor_at = Some((logical.len(), base + cursor.0));
            }
            let len = if row.wrapped { row.cells.len() } else { row.content_len() };
            // Keep the cursor's own cells even if they are blank.
            let len = if i == cursor_line { len.max(cursor.0) } else { len };
            for (c, s) in row.extras {
                if (c as usize) < len {
                    cur.1.push((base + c as usize, s));
                }
            }
            cur.0.extend_from_slice(&row.cells[..len.min(row.cells.len())]);
            if !row.wrapped {
                logical.push(std::mem::take(&mut cur));
            }
        }
        if !cur.0.is_empty() || cursor_at.is_some_and(|(l, _)| l == logical.len()) {
            logical.push(cur);
        }
        // Re-wrap.
        let mut out: VecDeque<Row> = VecDeque::new();
        let mut new_cursor = (0usize, 0usize);
        for (li, (cells, extras)) in logical.into_iter().enumerate() {
            let start = out.len();
            let mut row = Row::new(cols, blank);
            let mut x = 0;
            let mut placed: Vec<(usize, usize, usize)> = Vec::new(); // (src index, row offset, col)
            let mut i = 0;
            while i < cells.len() {
                let c = cells[i];
                let wide = c.has(attr::WIDE);
                if c.has(attr::SPACER) {
                    i += 1;
                    continue;
                }
                let w = if wide { 2 } else { 1 };
                if x + w > cols {
                    row.wrapped = true;
                    out.push_back(std::mem::replace(&mut row, Row::new(cols, blank)));
                    x = 0;
                }
                if w <= cols {
                    row.cells[x] = c;
                    if wide && x + 1 < cols {
                        row.cells[x + 1] = Cell { attrs: attr::SPACER, ch: ' ', ..c };
                    }
                    placed.push((i, out.len() - start, x));
                    if let Some((_, s)) = extras.iter().find(|(e, _)| *e == i) {
                        row.extras.push((x as u16, s.clone()));
                    }
                }
                x += w;
                i += 1;
            }
            out.push_back(row);
            if let Some((cl, off)) = cursor_at.filter(|(cl, _)| *cl == li) {
                // Map the cursor's cell offset to its new row and column.
                let (r, col) = match placed.iter().find(|(src, _, _)| *src >= off) {
                    Some(&(src, r, col)) if src == off => (r, col),
                    _ => {
                        // Past the content: continue after the last cell.
                        let extra = off.saturating_sub(cells.len());
                        let (lr, lc) = placed.last().map(|&(s, r, c)| (r, c + if cells[s].has(attr::WIDE) { 2 } else { 1 })).unwrap_or((0, 0));
                        let col = lc + extra;
                        (lr + col / cols, col % cols)
                    }
                };
                let _ = cl;
                while out.len() < start + r + 1 {
                    let n = out.len();
                    out[n - 1].wrapped = true;
                    out.push_back(Row::new(cols, blank));
                }
                new_cursor = (col.min(cols - 1), start + r);
            }
        }
        if out.is_empty() {
            out.push_back(Row::new(cols, blank));
        }
        // The screen is the last `rows` lines, but never so that the cursor
        // would be above it; pad below when there is not enough content.
        let mut total = out.len();
        let min_total = new_cursor.1 + 1;
        while total < rows.max(min_total) {
            out.push_back(Row::new(cols, blank));
            total += 1;
        }
        let mut hist = total - rows;
        if new_cursor.1 < hist {
            hist = new_cursor.1;
            while out.len() > hist + rows {
                out.pop_back();
            }
        }
        while out.len() < hist + rows {
            out.push_back(Row::new(cols, blank));
        }
        self.lines = out;
        self.cols = cols;
        self.rows = rows;
        let before = self.lines.len();
        self.trim_history();
        let trimmed = before - self.lines.len();
        let _ = trimmed;
        let y = new_cursor.1.saturating_sub(hist);
        (new_cursor.0, y.min(rows - 1))
    }
}
