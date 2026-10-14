//! The document of a [`super::TextEditor`]: text with line bookkeeping,
//! cursor and selection, and undo/redo as a list of edits (not snapshots, so
//! a large file costs only what changed). Independent of drawing; the app
//! owns it, so it can read the cursor position for a status bar, run find
//! and replace, or mark the document saved.

use super::edit::{next_grapheme, next_word, prev_grapheme, prev_word, word_at};

/// One replacement: `removed` at `at` became `inserted`.
#[derive(Clone, Debug, PartialEq)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

/// An undo step: changes applied in order, and the selection around them.
#[derive(Clone, Debug, PartialEq)]
struct Step {
    changes: Vec<Change>,
    before: (usize, usize),
    after: (usize, usize),
    /// Typing that later typing may extend (coalesced into one step).
    typing: bool,
}

/// Text being edited, with its cursor, selection and history.
///
/// Offsets are byte offsets into the text, always on character boundaries
/// (the editing operations keep them on grapheme boundaries). Lines are
/// separated by `\n`; a `\r\n` file is read as `\n` lines by
/// [`TextDoc::new`] and [`TextDoc::uses_crlf`] tells the app to write it back
/// that way.
#[derive(Clone, Debug)]
pub struct TextDoc {
    text: String,
    /// Byte offset of the start of every line (the first is 0).
    lines: Vec<usize>,
    /// The cursor (where typing goes) and the other end of the selection.
    cursor: usize,
    anchor: usize,
    /// The x position vertical moves keep (logical px; set by the editor).
    pub(crate) goal_x: Option<f32>,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Changes since load or the last `set_text`, counting undo as change.
    version: u64,
    saved: u64,
    crlf: bool,
    /// The editor should scroll the cursor into view (set by moves).
    pub(crate) reveal: bool,
}

impl Default for TextDoc {
    fn default() -> TextDoc {
        TextDoc::new("")
    }
}

const MAX_UNDO: usize = 1000;

impl TextDoc {
    /// A document with `text` (`\r\n` line ends are normalized to `\n`).
    pub fn new(text: &str) -> TextDoc {
        let crlf = text.contains("\r\n");
        let text = if crlf { text.replace("\r\n", "\n") } else { text.to_string() };
        let mut d = TextDoc {
            text,
            lines: Vec::new(),
            cursor: 0,
            anchor: 0,
            goal_x: None,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            saved: 0,
            crlf,
            reveal: false,
        };
        d.reindex();
        d
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text as it should be written to a file (with `\r\n` line ends
    /// if it was read with them).
    pub fn to_file_text(&self) -> String {
        if self.crlf { self.text.replace('\n', "\r\n") } else { self.text.clone() }
    }

    pub fn uses_crlf(&self) -> bool {
        self.crlf
    }

    /// Replace everything (opening a file): clears the history and marks
    /// the document unmodified.
    pub fn set_text(&mut self, text: &str) {
        *self = TextDoc::new(text);
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Increases with every change (and undo/redo).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Changed since it was loaded or last marked saved.
    pub fn is_modified(&self) -> bool {
        self.version != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.version;
    }

    // -- Lines ------------------------------------------------------------------------

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Byte range of line `i` without its `\n`.
    pub fn line_range(&self, i: usize) -> std::ops::Range<usize> {
        let start = self.lines[i.min(self.lines.len() - 1)];
        let end = self.lines.get(i + 1).map(|&n| n - 1).unwrap_or(self.text.len());
        start..end
    }

    pub fn line(&self, i: usize) -> &str {
        &self.text[self.line_range(i)]
    }

    /// The line containing byte offset `at`.
    pub fn line_of(&self, at: usize) -> usize {
        self.lines.partition_point(|&s| s <= at).saturating_sub(1)
    }

    /// Line and column (both from 0; the column counts characters) of `at`.
    pub fn position(&self, at: usize) -> (usize, usize) {
        let l = self.line_of(at);
        let start = self.lines[l];
        (l, self.text[start..at.min(self.text.len()).max(start)].chars().count())
    }

    /// The offset of `line`, `column` (characters), clamped to the text.
    pub fn offset(&self, line: usize, column: usize) -> usize {
        let r = self.line_range(line.min(self.lines.len() - 1));
        let s = &self.text[r.clone()];
        r.start + s.char_indices().nth(column).map(|(i, _)| i).unwrap_or(s.len())
    }

    fn reindex(&mut self) {
        self.lines.clear();
        self.lines.push(0);
        self.lines.extend(self.text.match_indices('\n').map(|(i, _)| i + 1));
    }

    // -- Cursor and selection ------------------------------------------------------------

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// Line and column of the cursor (for "Ln 12, Col 4": add one to each).
    pub fn cursor_position(&self) -> (usize, usize) {
        self.position(self.cursor)
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// The selected range, ordered.
    pub fn selection(&self) -> std::ops::Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection()]
    }

    fn fix(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Select `anchor..cursor` (equal for no selection) and scroll the
    /// cursor into view, e.g. for a search match or "go to line".
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.anchor = self.fix(anchor);
        self.cursor = self.fix(cursor);
        self.goal_x = None;
        self.reveal = true;
        self.end_typing();
    }

    /// Move the cursor to `to`; `extend` keeps the anchor (Shift).
    pub fn move_to(&mut self, to: usize, extend: bool) {
        self.cursor = self.fix(to);
        if !extend {
            self.anchor = self.cursor;
        }
        self.reveal = true;
        self.end_typing();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
        self.end_typing();
    }

    /// Put the cursor at the start of `line` (from 0) and show it.
    pub fn go_to_line(&mut self, line: usize) {
        let at = self.lines[line.min(self.lines.len() - 1)];
        self.set_selection(at, at);
    }

    pub(crate) fn left(&self, word: bool) -> usize {
        if word { prev_word(&self.text, self.cursor) } else { prev_grapheme(&self.text, self.cursor) }
    }

    pub(crate) fn right(&self, word: bool) -> usize {
        if word { next_word(&self.text, self.cursor) } else { next_grapheme(&self.text, self.cursor) }
    }

    /// Select the word around `at` (double click).
    pub fn select_word(&mut self, at: usize) {
        let l = self.line_of(at);
        let r = self.line_range(l);
        let (a, b) = word_at(&self.text[r.clone()], at - r.start);
        self.set_selection(r.start + a, r.start + b);
    }

    /// Select line `l` including its line end (triple click).
    pub fn select_line(&mut self, l: usize) {
        let r = self.line_range(l);
        let end = if r.end < self.text.len() { r.end + 1 } else { r.end };
        self.set_selection(r.start, end);
    }

    // -- Editing --------------------------------------------------------------------------

    fn end_typing(&mut self) {
        if let Some(s) = self.undo.last_mut() {
            s.typing = false;
        }
    }

    /// Apply one change to the text, keeping the line index current.
    fn apply(&mut self, c: &Change) {
        let end = c.at + c.removed.len();
        self.text.replace_range(c.at..end, &c.inserted);
        // Lines starting inside the replaced range go; later ones shift.
        let first = self.lines.partition_point(|&s| s <= c.at);
        let last = self.lines.partition_point(|&s| s <= end);
        let delta = c.inserted.len() as isize - c.removed.len() as isize;
        let new: Vec<usize> = c.inserted.match_indices('\n').map(|(i, _)| c.at + i + 1).collect();
        self.lines.splice(first..last, new.iter().copied());
        for s in &mut self.lines[first + new.len()..] {
            *s = (*s as isize + delta) as usize;
        }
        self.version += 1;
    }

    /// Replace `range` with `with` as one undo step (find and replace,
    /// programmatic edits). The cursor ends after the inserted text.
    pub fn replace(&mut self, range: std::ops::Range<usize>, with: &str) {
        let (a, b) = (self.fix(range.start), self.fix(range.end.max(range.start)));
        self.edit(vec![(a, b, with.to_string())], false);
    }

    /// Apply replacements given as (start, end, new text) in ascending,
    /// non-overlapping order, as one undo step; the cursor ends after the
    /// last insertion.
    fn edit(&mut self, mut repl: Vec<(usize, usize, String)>, typing: bool) {
        if repl.is_empty() {
            return;
        }
        let before = (self.anchor, self.cursor);
        // Apply from the end so earlier offsets stay valid.
        repl.sort_by_key(|r| r.0);
        let mut changes = Vec::with_capacity(repl.len());
        let mut cursor = 0;
        for (a, b, s) in repl.into_iter().rev() {
            let c = Change { at: a, removed: self.text[a..b].to_string(), inserted: s };
            self.apply(&c);
            if changes.is_empty() {
                cursor = a + c.inserted.len();
            }
            changes.push(c);
        }
        // Each change's offset is valid in the text as it was when that
        // change was applied: redo applies them in this order, undo reverts
        // them in the opposite one.
        let shift: isize = changes.iter().skip(1).map(|c| c.inserted.len() as isize - c.removed.len() as isize).sum();
        cursor = (cursor as isize + shift) as usize;
        self.cursor = cursor;
        self.anchor = cursor;
        self.goal_x = None;
        self.reveal = true;
        self.redo.clear();
        let after = (self.anchor, self.cursor);
        // Typing right after the previous typing extends its step.
        if typing {
            if let Some(last) = self.undo.last_mut() {
                if last.typing && last.after == before && last.changes.len() == 1 && changes.len() == 1 {
                    let (p, c) = (&mut last.changes[0], &changes[0]);
                    if c.removed.is_empty() && c.at == p.at + p.inserted.len() {
                        p.inserted.push_str(&c.inserted);
                        last.after = after;
                        return;
                    }
                }
            }
        }
        self.undo.push(Step { changes, before, after, typing });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
    }

    /// Type or paste `s` over the selection.
    pub fn insert(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let r = self.selection();
        if s.is_empty() && r.is_empty() {
            return;
        }
        // A word ends a typing run: undo restores word by word.
        let typing = s.chars().count() == 1 && r.is_empty() && !s.starts_with([' ', '\n', '\t']);
        self.edit(vec![(r.start, r.end, s)], typing);
    }

    /// A new line, indented like the current one.
    pub fn newline(&mut self) {
        let l = self.line_of(self.selection().start);
        let line = self.line(l);
        let indent: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        // Not past the cursor: Enter inside the indentation keeps it as is.
        let col = self.selection().start - self.line_range(l).start;
        let indent = &indent[..indent.len().min(col)];
        self.insert(&format!("\n{indent}"));
    }

    pub fn backspace(&mut self, word: bool) {
        if !self.has_selection() {
            if self.cursor == 0 {
                return;
            }
            self.anchor = self.left(word);
        }
        self.delete_selection();
    }

    pub fn delete(&mut self, word: bool) {
        if !self.has_selection() {
            if self.cursor >= self.text.len() {
                return;
            }
            self.anchor = self.right(word);
        }
        self.delete_selection();
    }

    pub fn delete_selection(&mut self) {
        let r = self.selection();
        if !r.is_empty() {
            self.edit(vec![(r.start, r.end, String::new())], false);
        }
    }

    /// Lines touched by the selection (a selection ending at the start of a
    /// line does not include that line).
    fn selected_lines(&self) -> std::ops::Range<usize> {
        let r = self.selection();
        let first = self.line_of(r.start);
        let mut last = self.line_of(r.end);
        if last > first && self.lines[last] == r.end {
            last -= 1;
        }
        first..last + 1
    }

    /// Indent the selected lines by `unit` (Tab with a multi-line selection).
    pub fn indent(&mut self, unit: &str) {
        let lines = self.selected_lines();
        let (a, c) = (self.anchor, self.cursor);
        let repl: Vec<_> = lines.clone().filter(|&l| !self.line(l).is_empty()).map(|l| (self.lines[l], self.lines[l], unit.to_string())).collect();
        self.edit(repl, false);
        self.reselect_lines(lines, a, c);
    }

    /// Remove one level of indentation (`tab_width` spaces or a tab) from the
    /// selected lines (Shift+Tab).
    pub fn unindent(&mut self, tab_width: usize) {
        let lines = self.selected_lines();
        let (a, c) = (self.anchor, self.cursor);
        let repl: Vec<_> = lines
            .clone()
            .filter_map(|l| {
                let s = self.line(l);
                let n = if s.starts_with('\t') { 1 } else { s.chars().take(tab_width).take_while(|c| *c == ' ').count() };
                (n > 0).then(|| (self.lines[l], self.lines[l] + n, String::new()))
            })
            .collect();
        self.edit(repl, false);
        self.reselect_lines(lines, a, c);
    }

    /// After indenting: select the whole lines again (or put the cursor
    /// back on its line when nothing was selected).
    fn reselect_lines(&mut self, lines: std::ops::Range<usize>, anchor: usize, cursor: usize) {
        if anchor == cursor {
            return;
        }
        let start = self.lines[lines.start];
        let end = self.line_range(lines.end - 1).end;
        if cursor >= anchor {
            self.anchor = start;
            self.cursor = end;
        } else {
            self.anchor = end;
            self.cursor = start;
        }
        if let Some(s) = self.undo.last_mut() {
            s.after = (self.anchor, self.cursor);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        // Changes were applied end-first; revert them start-first.
        for c in step.changes.iter().rev() {
            self.apply(&Change { at: c.at, removed: c.inserted.clone(), inserted: c.removed.clone() });
        }
        (self.anchor, self.cursor) = step.before;
        self.goal_x = None;
        self.reveal = true;
        self.redo.push(Step { typing: false, ..step });
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        for c in &step.changes {
            self.apply(c);
        }
        (self.anchor, self.cursor) = step.after;
        self.goal_x = None;
        self.reveal = true;
        self.undo.push(step);
        true
    }

    // -- Search ---------------------------------------------------------------------------

    /// Every match of `query` (case-insensitive unless `case`), as byte
    /// ranges, at most `limit`.
    pub fn find_all(&self, query: &str, case: bool, limit: usize) -> Vec<std::ops::Range<usize>> {
        if query.is_empty() {
            return Vec::new();
        }
        if case {
            return self.text.match_indices(query).take(limit).map(|(i, m)| i..i + m.len()).collect();
        }
        // Case-insensitive: compare lowercased characters, mapping back to
        // offsets in the original text.
        let q: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        let hay: Vec<(usize, char)> = self.text.char_indices().flat_map(|(i, c)| c.to_lowercase().map(move |l| (i, l))).collect();
        let mut out = Vec::new();
        let mut k = 0;
        while k + q.len() <= hay.len() && out.len() < limit {
            if hay[k..k + q.len()].iter().zip(&q).all(|((_, a), b)| a == b) {
                let start = hay[k].0;
                let last = hay[k + q.len() - 1].0;
                let end = last + self.text[last..].chars().next().map_or(0, char::len_utf8);
                out.push(start..end);
                // Skip past this match (whole characters).
                while k < hay.len() && hay[k].0 < end {
                    k += 1;
                }
            } else {
                k += 1;
            }
        }
        out
    }

    /// Select the next match of `query` after the selection (wrapping
    /// around); false when there is none.
    pub fn find_next(&mut self, query: &str, case: bool) -> bool {
        let all = self.find_all(query, case, usize::MAX);
        let from = self.selection().end;
        match all.iter().find(|r| r.start >= from).or(all.first()) {
            Some(r) => {
                self.set_selection(r.start, r.end);
                true
            }
            None => false,
        }
    }

    /// Select the previous match before the selection (wrapping around).
    pub fn find_previous(&mut self, query: &str, case: bool) -> bool {
        let all = self.find_all(query, case, usize::MAX);
        let from = self.selection().start;
        match all.iter().rev().find(|r| r.end <= from).or(all.last()) {
            Some(r) => {
                self.set_selection(r.start, r.end);
                true
            }
            None => false,
        }
    }

    /// Replace every match as one undo step; returns how many.
    pub fn replace_all(&mut self, query: &str, with: &str, case: bool) -> usize {
        let all = self.find_all(query, case, usize::MAX);
        let n = all.len();
        self.edit(all.into_iter().map(|r| (r.start, r.end, with.to_string())).collect(), false);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> TextDoc {
        TextDoc::new(s)
    }

    #[test]
    fn lines_follow_edits() {
        let mut d = doc("one\ntwo\nthree");
        assert_eq!(d.line_count(), 3);
        assert_eq!((d.line(0), d.line(1), d.line(2)), ("one", "two", "three"));
        d.move_to(5, false);
        d.insert("X\nY");
        assert_eq!(d.text(), "one\ntXwo\nthree".replace("tXwo", "tX\nYwo"));
        assert_eq!(d.line_count(), 4);
        assert_eq!(d.line(2), "Ywo");
        assert_eq!(d.line_of(d.cursor()), 2);
        d.select_all();
        d.insert("");
        d.delete_selection();
        assert_eq!((d.text(), d.line_count()), ("", 1));
        let fresh = doc("a\r\nb\r\n");
        assert!(fresh.uses_crlf());
        assert_eq!(fresh.text(), "a\nb\n");
        assert_eq!(fresh.line_count(), 3);
        assert_eq!(fresh.to_file_text(), "a\r\nb\r\n");
    }

    #[test]
    fn positions_and_offsets() {
        let d = doc("héllo\nwörld");
        assert_eq!(d.position(d.offset(1, 2)), (1, 2));
        assert_eq!(d.offset(0, 99), "héllo".len(), "clamped to the line");
        assert_eq!(d.offset(9, 0), 7, "clamped to the last line");
        assert_eq!(d.position(d.len()), (1, 5));
    }

    #[test]
    fn typing_coalesces_and_undo_redo_restore() {
        let mut d = doc("");
        for c in ["H", "i", " ", "y", "o", "u"] {
            d.insert(c);
        }
        assert_eq!(d.text(), "Hi you");
        assert!(d.is_modified());
        d.undo();
        assert_eq!(d.text(), "Hi ", "one word per step");
        d.undo();
        assert_eq!(d.text(), "Hi");
        d.undo();
        assert_eq!(d.text(), "");
        assert!(!d.undo());
        d.redo();
        d.redo();
        assert_eq!((d.text(), d.cursor()), ("Hi ", 3));
        d.insert("!");
        assert!(!d.can_redo(), "a new edit drops the redo history");
        d.mark_saved();
        assert!(!d.is_modified());
        d.backspace(false);
        assert!(d.is_modified());
    }

    #[test]
    fn newline_keeps_indentation() {
        let mut d = doc("    fn main() {");
        d.move_to(d.len(), false);
        d.newline();
        d.insert("x");
        assert_eq!(d.text(), "    fn main() {\n    x");
        d.move_to(2, false);
        d.newline();
        assert_eq!((d.line(0), d.line(1)), ("  ", "    fn main() {"), "Enter inside the indentation repeats only what is before the cursor");
    }

    #[test]
    fn indent_and_unindent_selected_lines() {
        let mut d = doc("a\nb\n\nc");
        d.set_selection(0, 5);
        d.indent("\t");
        assert_eq!(d.text(), "\ta\nb\n\nc".replace("\nb", "\n\tb"), "a selection ending at a line start leaves that line");
        d.undo();
        d.set_selection(0, 6);
        d.indent("\t");
        assert_eq!(d.text(), "\ta\n\tb\n\n\tc", "empty lines stay empty");
        assert_eq!(d.selection(), 0..d.len());
        d.unindent(4);
        assert_eq!(d.text(), "a\nb\n\nc");
        d.undo();
        assert_eq!(d.text(), "\ta\n\tb\n\n\tc");
        d.undo();
        assert_eq!(d.text(), "a\nb\n\nc");
        assert!(!d.can_undo(), "the first indent was undone before");
        let mut s = doc("      x");
        s.set_selection(6, 7);
        s.unindent(4);
        assert_eq!(s.text(), "  x");
    }

    #[test]
    fn word_and_line_selection() {
        let mut d = doc("alpha beta\ngamma");
        d.select_word(7);
        assert_eq!(d.selected_text(), "beta");
        d.select_line(0);
        assert_eq!(d.selected_text(), "alpha beta\n");
        d.select_line(1);
        assert_eq!(d.selected_text(), "gamma");
    }

    #[test]
    fn find_and_replace() {
        let mut d = doc("Cat cat CAT dog");
        assert_eq!(d.find_all("cat", false, 10).len(), 3);
        assert_eq!(d.find_all("cat", true, 10), vec![4..7]);
        assert!(d.find_next("cat", false));
        assert_eq!(d.selection(), 0..3);
        assert!(d.find_next("cat", false));
        assert_eq!(d.selection(), 4..7);
        assert!(d.find_previous("cat", false));
        assert_eq!(d.selection(), 0..3);
        assert_eq!(d.replace_all("cat", "bird", false), 3);
        assert_eq!(d.text(), "bird bird bird dog");
        d.undo();
        assert_eq!(d.text(), "Cat cat CAT dog", "replace all is one step");
        assert!(!d.find_next("zebra", false));
        // Matches map back to the original text across case changes.
        let u = doc("STRASSE straße");
        assert_eq!(u.find_all("straße", false, 10), vec![8..15]);
    }
}
