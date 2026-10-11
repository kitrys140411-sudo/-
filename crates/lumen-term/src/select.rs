//! Selection and scrollback search over the grid, in stable absolute line
//! numbers (unaffected by new output scrolling the screen).

use crate::term::{attr, Grid};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    /// Absolute line number (see `Grid::dropped`).
    pub line: u64,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    Char,
    Word,
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Pos,
    pub head: Pos,
    pub mode: SelMode,
}

fn row_at(grid: &Grid, line: u64) -> Option<&crate::term::Row> {
    let i = line.checked_sub(grid.dropped)? as usize;
    grid.line(i)
}

/// Characters that belong to words (paths and URLs select as one word).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || "_-./~:@?=&%+#$,".contains(c)
}

fn cell_char(grid: &Grid, p: Pos) -> Option<char> {
    let row = row_at(grid, p.line)?;
    let mut col = p.col;
    if col < row.cells.len() && row.cells[col].has(attr::SPACER) && col > 0 {
        col -= 1;
    }
    row.cells.get(col).map(|c| c.ch)
}

fn prev(grid: &Grid, p: Pos) -> Option<Pos> {
    if p.col > 0 {
        return Some(Pos { line: p.line, col: p.col - 1 });
    }
    let above = row_at(grid, p.line.checked_sub(1)?)?;
    above.wrapped.then(|| Pos { line: p.line - 1, col: grid.cols - 1 })
}

fn next(grid: &Grid, p: Pos) -> Option<Pos> {
    if p.col + 1 < grid.cols {
        return Some(Pos { line: p.line, col: p.col + 1 });
    }
    let row = row_at(grid, p.line)?;
    (row.wrapped && row_at(grid, p.line + 1).is_some()).then(|| Pos { line: p.line + 1, col: 0 })
}

/// The word around `p` (across soft wraps).
pub fn word_at(grid: &Grid, p: Pos) -> (Pos, Pos) {
    let Some(c) = cell_char(grid, p) else { return (p, p) };
    let class = |c: char| if is_word_char(c) { 1 } else if c == ' ' { 0 } else { 2 };
    let k = class(c);
    if k == 2 {
        return (p, p);
    }
    let mut a = p;
    while let Some(q) = prev(grid, a) {
        if cell_char(grid, q).map(class) != Some(k) {
            break;
        }
        a = q;
    }
    let mut b = p;
    while let Some(q) = next(grid, b) {
        if cell_char(grid, q).map(class) != Some(k) {
            break;
        }
        b = q;
    }
    (a, b)
}

/// The logical line (soft-wrapped rows joined) around `line`.
pub fn line_at(grid: &Grid, line: u64) -> (Pos, Pos) {
    let mut a = line;
    while a > grid.dropped && row_at(grid, a - 1).is_some_and(|r| r.wrapped) {
        a -= 1;
    }
    let mut b = line;
    while row_at(grid, b).is_some_and(|r| r.wrapped) && row_at(grid, b + 1).is_some() {
        b += 1;
    }
    (Pos { line: a, col: 0 }, Pos { line: b, col: grid.cols - 1 })
}

impl Selection {
    pub fn new(at: Pos, mode: SelMode) -> Selection {
        Selection { anchor: at, head: at, mode }
    }

    /// Inclusive start and end, expanded for word/line modes.
    pub fn range(&self, grid: &Grid) -> (Pos, Pos) {
        let (a, b) = if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) };
        match self.mode {
            SelMode::Char => (a, b),
            SelMode::Word => (word_at(grid, a).0, word_at(grid, b).1),
            SelMode::Line => (line_at(grid, a.line).0, line_at(grid, b.line).1),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.mode == SelMode::Char && self.anchor == self.head
    }

    /// Columns of `line` inside the selection, as an inclusive range.
    pub fn cols_on(&self, grid: &Grid, line: u64) -> Option<(usize, usize)> {
        let (a, b) = self.range(grid);
        if line < a.line || line > b.line {
            return None;
        }
        let from = if line == a.line { a.col } else { 0 };
        let to = if line == b.line { b.col } else { grid.cols - 1 };
        (from <= to).then_some((from, to))
    }

    pub fn text(&self, grid: &Grid) -> String {
        let (a, b) = self.range(grid);
        let mut out = String::new();
        let mut line = a.line;
        while line <= b.line {
            let Some(row) = row_at(grid, line) else { break };
            let from = if line == a.line { a.col } else { 0 };
            let to = if line == b.line { b.col + 1 } else { row.cells.len() };
            let mut t = row.text(from, to.min(row.cells.len()));
            if !row.wrapped || line == b.line {
                let trimmed = t.trim_end_matches(' ').len();
                t.truncate(trimmed);
            }
            out.push_str(&t);
            if line != b.line && !row.wrapped {
                out.push('\n');
            }
            line += 1;
        }
        out
    }
}

/// All matches of `query` (case-insensitive unless it has capitals), as
/// inclusive cell ranges, oldest first. Matches may span soft wraps.
pub fn search(grid: &Grid, query: &str) -> Vec<(Pos, Pos)> {
    if query.is_empty() {
        return Vec::new();
    }
    let case = query.chars().any(|c| c.is_uppercase());
    let fold = |c: char| if case { c } else { c.to_lowercase().next().unwrap_or(c) };
    let q: Vec<char> = query.chars().map(fold).collect();
    let mut out = Vec::new();
    let total = grid.total_lines();
    let mut i = 0;
    // Logical line text with the position of each char.
    let mut chars: Vec<char> = Vec::new();
    let mut pos: Vec<Pos> = Vec::new();
    while i < total {
        chars.clear();
        pos.clear();
        loop {
            let row = grid.line(i).expect("line");
            let line = grid.dropped + i as u64;
            for (x, c) in row.cells.iter().enumerate() {
                if c.has(attr::SPACER) {
                    continue;
                }
                chars.push(fold(c.ch));
                pos.push(Pos { line, col: x });
            }
            i += 1;
            if !row.wrapped || i >= total {
                break;
            }
        }
        if chars.len() < q.len() {
            continue;
        }
        let mut s = 0;
        while s + q.len() <= chars.len() {
            if chars[s..s + q.len()] == q[..] {
                let end = pos[s + q.len() - 1];
                // Wide chars cover two cells.
                out.push((pos[s], end));
                s += q.len();
            } else {
                s += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Terminal;

    fn term(text: &str, cols: usize) -> Terminal {
        let mut t = Terminal::new(cols, 4, 100);
        t.advance(text.as_bytes());
        t
    }

    #[test]
    fn char_word_line_selection() {
        let t = term("hello world-wide /usr/bin\r\nnext", 40);
        let g = t.term.grid();
        let at = |col| Pos { line: g.dropped + g.history() as u64, col };
        let mut s = Selection::new(at(1), SelMode::Char);
        s.head = at(7);
        assert_eq!(s.text(g), "ello wo");
        let s = Selection::new(at(8), SelMode::Word);
        assert_eq!(s.text(g), "world-wide");
        let s = Selection::new(at(20), SelMode::Word);
        assert_eq!(s.text(g), "/usr/bin");
        let s = Selection::new(at(3), SelMode::Line);
        assert_eq!(s.text(g), "hello world-wide /usr/bin");
    }

    #[test]
    fn selection_across_wrapped_and_hard_lines() {
        let t = term("abcdefgh\r\nxy", 5);
        let g = t.term.grid();
        let top = g.dropped + g.history() as u64;
        let s = Selection { anchor: Pos { line: top, col: 2 }, head: Pos { line: top + 2, col: 1 }, mode: SelMode::Char };
        assert_eq!(s.text(g), "cdefgh\nxy");
        let s = Selection::new(Pos { line: top + 1, col: 0 }, SelMode::Line);
        assert_eq!(s.text(g), "abcdefgh");
    }

    #[test]
    fn search_finds_matches_across_wraps() {
        let t = term("foo bar\r\nbarfoo\r\nxxfo", 4);
        let g = t.term.grid();
        let m = search(g, "foo");
        assert_eq!(m.len(), 2);
        assert_eq!(search(g, "FOO").len(), 0, "capitals make it case-sensitive");
        assert_eq!(search(g, "bar").len(), 2);
        // "bar foo" style: across the wrap between "foo " and "bar".
        assert_eq!(search(g, "o b").len(), 1);
    }
}
