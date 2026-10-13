//! Editing model of single-line text inputs: cursor and selection on
//! grapheme boundaries, word movement, insertion and deletion, undo/redo with
//! typing coalesced into one step. Independent of drawing, so it is unit
//! tested on its own.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

/// Cursor, selection and history of one text input (kept by the toolkit per
/// input id; the text itself is the app's `String`).
#[derive(Clone, Debug, Default)]
pub struct EditState {
    /// Byte offset of the cursor (a grapheme boundary).
    pub cursor: usize,
    /// The other end of the selection (== cursor when nothing is selected).
    pub anchor: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The last change was typing (coalesced with the next typing).
    typing: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

pub fn prev_grapheme(s: &str, i: usize) -> usize {
    let mut c = GraphemeCursor::new(i.min(s.len()), s.len(), true);
    c.prev_boundary(s, 0).ok().flatten().unwrap_or(0)
}

pub fn next_grapheme(s: &str, i: usize) -> usize {
    let mut c = GraphemeCursor::new(i.min(s.len()), s.len(), true);
    c.next_boundary(s, 0).ok().flatten().unwrap_or(s.len())
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Start of the word before `i` (skipping separators first), like Ctrl+Left.
pub fn prev_word(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0 && !s[..j].chars().next_back().is_some_and(is_word) {
        j = prev_grapheme(s, j);
    }
    while j > 0 && s[..j].chars().next_back().is_some_and(is_word) {
        j = prev_grapheme(s, j);
    }
    j
}

/// End of the word after `i` (skipping separators first), like Ctrl+Right.
pub fn next_word(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j < s.len() && !s[j..].chars().next().is_some_and(is_word) {
        j = next_grapheme(s, j);
    }
    while j < s.len() && s[j..].chars().next().is_some_and(is_word) {
        j = next_grapheme(s, j);
    }
    j
}

/// The word (or run of separators) around byte offset `i`.
pub fn word_at(s: &str, i: usize) -> (usize, usize) {
    for (start, w) in s.split_word_bound_indices() {
        let end = start + w.len();
        if i >= start && i < end || (i == s.len() && end == s.len()) {
            return (start, end);
        }
    }
    (i, i)
}

impl EditState {
    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    /// The selected byte range (ordered).
    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    pub fn selected_text<'s>(&self, text: &'s str) -> &'s str {
        let (a, b) = self.selection();
        &text[a.min(text.len())..b.min(text.len())]
    }

    /// Keep offsets valid after the app changed the text behind our back.
    pub fn clamp(&mut self, text: &str) {
        let fix = |i: usize| {
            let mut i = i.min(text.len());
            while !text.is_char_boundary(i) {
                i -= 1;
            }
            i
        };
        self.cursor = fix(self.cursor);
        self.anchor = fix(self.anchor);
    }

    /// Move the cursor to `i`; with `extend` the anchor stays (shift).
    pub fn move_to(&mut self, i: usize, extend: bool) {
        self.cursor = i;
        if !extend {
            self.anchor = i;
        }
        self.typing = false;
    }

    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.cursor = text.len();
        self.typing = false;
    }

    fn snapshot(&mut self, text: &str, typing: bool) {
        // Consecutive typing is one undo step; anything else starts a new one.
        if !(typing && self.typing) {
            self.undo.push(Snapshot { text: text.to_string(), cursor: self.cursor, anchor: self.anchor });
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.typing = typing;
    }

    /// Replace the selection with `s` (typing or pasting). Newlines become
    /// spaces (single line).
    pub fn insert(&mut self, text: &mut String, s: &str) {
        let s: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        if s.is_empty() && !self.has_selection() {
            return;
        }
        // A word boundary ends a typing run (undo restores word by word).
        let typing = s.chars().count() == 1 && !self.has_selection() && !s.starts_with(' ');
        self.snapshot(text, typing);
        let (a, b) = self.selection();
        text.replace_range(a..b, &s);
        self.cursor = a + s.len();
        self.anchor = self.cursor;
    }

    /// Backspace (`word`: Ctrl+Backspace).
    pub fn backspace(&mut self, text: &mut String, word: bool) {
        if !self.has_selection() {
            if self.cursor == 0 {
                return;
            }
            self.anchor = if word { prev_word(text, self.cursor) } else { prev_grapheme(text, self.cursor) };
        }
        self.delete_selection(text);
    }

    /// Delete (`word`: Ctrl+Delete).
    pub fn delete(&mut self, text: &mut String, word: bool) {
        if !self.has_selection() {
            if self.cursor >= text.len() {
                return;
            }
            self.anchor = if word { next_word(text, self.cursor) } else { next_grapheme(text, self.cursor) };
        }
        self.delete_selection(text);
    }

    pub fn delete_selection(&mut self, text: &mut String) {
        if !self.has_selection() {
            return;
        }
        self.snapshot(text, false);
        let (a, b) = self.selection();
        text.replace_range(a..b, "");
        self.cursor = a;
        self.anchor = a;
    }

    pub fn undo(&mut self, text: &mut String) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(Snapshot { text: text.clone(), cursor: self.cursor, anchor: self.anchor });
        *text = s.text;
        self.cursor = s.cursor;
        self.anchor = s.anchor;
        self.typing = false;
        true
    }

    pub fn redo(&mut self, text: &mut String) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(Snapshot { text: text.clone(), cursor: self.cursor, anchor: self.anchor });
        *text = s.text;
        self.cursor = s.cursor;
        self.anchor = s.anchor;
        self.typing = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_and_words() {
        let s = "héllo wörld, 👍🏽 ok";
        assert_eq!(next_grapheme(s, 0), 1);
        assert_eq!(next_grapheme(s, 1), 3, "é is two bytes");
        let thumbs = s.find('👍').unwrap();
        assert_eq!(next_grapheme(s, thumbs), thumbs + "👍🏽".len(), "emoji with modifier is one grapheme");
        assert_eq!(prev_word(s, s.len()), s.len() - 2);
        assert_eq!(prev_word(s, s.find("wörld").unwrap() + 3), s.find("wörld").unwrap());
        assert_eq!(next_word(s, 0), 6);
        assert_eq!(next_word(s, 6), s.find(',').unwrap(), "skips the space, stops after the word");
        assert_eq!(word_at("foo bar", 5), (4, 7));
        assert_eq!(word_at("foo bar", 7), (4, 7));
    }

    #[test]
    fn typing_selection_and_deletion() {
        let mut t = String::new();
        let mut e = EditState::default();
        for c in ["h", "e", "l", "l", "o"] {
            e.insert(&mut t, c);
        }
        e.insert(&mut t, " world");
        assert_eq!(t, "hello world");
        e.move_to(5, false);
        e.move_to(0, true);
        assert_eq!(e.selected_text(&t), "hello");
        e.insert(&mut t, "HI");
        assert_eq!((t.as_str(), e.cursor), ("HI world", 2));
        e.move_to(t.len(), false);
        e.backspace(&mut t, true);
        assert_eq!(t, "HI ");
        e.move_to(0, false);
        e.delete(&mut t, false);
        assert_eq!(t, "I ");
        e.insert(&mut t, "a\nb");
        assert_eq!(t, "a bI ", "newlines become spaces");
    }

    #[test]
    fn undo_coalesces_typing() {
        let mut t = String::new();
        let mut e = EditState::default();
        for c in "abc".chars() {
            e.insert(&mut t, &c.to_string());
        }
        e.insert(&mut t, " ");
        for c in "de".chars() {
            e.insert(&mut t, &c.to_string());
        }
        assert_eq!(t, "abc de");
        assert!(e.undo(&mut t));
        assert_eq!(t, "abc ");
        assert!(e.undo(&mut t));
        assert_eq!(t, "abc");
        assert!(e.undo(&mut t));
        assert_eq!(t, "");
        assert!(!e.undo(&mut t));
        assert!(e.redo(&mut t));
        assert_eq!(t, "abc");
        e.insert(&mut t, "!");
        assert!(!e.redo(&mut t), "an edit clears the redo stack");
    }

    #[test]
    fn clamp_after_external_change() {
        let t = "hé".to_string();
        let mut e = EditState { cursor: 5, anchor: 5, ..Default::default() };
        e.clamp(&t);
        assert_eq!(e.cursor, 3);
        e.cursor = 2;
        e.anchor = 2;
        e.clamp(&t);
        assert_eq!(e.cursor, 1, "inside é moves back to a boundary");
    }
}
