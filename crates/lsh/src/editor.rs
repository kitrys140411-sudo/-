//! Interactive line editor: emacs-style keys, history with incremental search,
//! completion, syntax highlighting and history-based suggestions.
//!
//! Rendering redraws the whole (possibly wrapped) line on every change: it moves
//! to the prompt's first row, clears to the end of the screen and repaints, then
//! places the cursor. Widths come from `unicode-width`, so wide CJK/emoji and
//! combining characters keep the cursor in the right column.

use unicode_width::UnicodeWidthChar;

use crate::sys::{self, Fd};

pub enum ReadResult {
    Line(String),
    Eof,
}

/// Hooks the shell provides to the editor.
pub trait Helper {
    /// ANSI-colored version of `line` (same visible text).
    fn highlight(&mut self, line: &str) -> String;
    /// Completion candidates for the word ending at `pos`: (word start, candidates).
    fn complete(&mut self, line: &str, pos: usize) -> (usize, Vec<String>);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    Ctrl(char),
    AltBackspace,
    AltD,
    AltDot,
    PasteStart,
    Escape,
    Unknown,
}

pub struct Editor {
    fd: Fd,
    pub history: Vec<String>,
    history_file: Option<String>,
    buf: Vec<char>,
    pos: usize,
    /// Rows between the prompt's first row and the cursor in the last render.
    cursor_row: usize,
    kill: String,
    last_kill: bool,
    last_tab: bool,
}

/// Visible width of text that may contain ANSI CSI/OSC sequences and the
/// \x01..\x02 "non-printing" markers used by prompts.
pub fn display_width(s: &str) -> usize {
    let mut w = 0;
    let mut chars = s.chars().peekable();
    let mut hidden = false;
    while let Some(c) = chars.next() {
        match c {
            '\u{1}' => hidden = true,
            '\u{2}' => hidden = false,
            '\u{1b}' => {
                match chars.peek() {
                    Some('[') => {
                        chars.next();
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    Some(']') => {
                        // OSC: until BEL or ST.
                        while let Some(c) = chars.next() {
                            if c == '\u{7}' {
                                break;
                            }
                            if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                                chars.next();
                                break;
                            }
                        }
                    }
                    _ => {
                        chars.next();
                    }
                }
            }
            _ if hidden => {}
            c => w += c.width().unwrap_or(0),
        }
    }
    w
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new(fd: Fd, history_file: Option<String>) -> Self {
        let mut ed = Editor {
            fd,
            history: Vec::new(),
            history_file,
            buf: Vec::new(),
            pos: 0,
            cursor_row: 0,
            kill: String::new(),
            last_kill: false,
            last_tab: false,
        };
        if let Some(path) = &ed.history_file {
            if let Ok(text) = std::fs::read_to_string(path) {
                ed.history = text.lines().filter(|l| !l.is_empty()).map(String::from).collect();
                let n = ed.history.len();
                if n > 5000 {
                    ed.history.drain(..n - 5000);
                }
            }
        }
        ed
    }

    pub fn add_history(&mut self, line: &str) {
        let line = line.trim_end_matches('\n');
        if line.trim().is_empty() || line.starts_with(' ') || self.history.last().map(String::as_str) == Some(line) {
            return;
        }
        self.history.push(line.to_string());
        if let Some(path) = &self.history_file {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                // The history file is line based; multi-line commands are joined.
                let _ = writeln!(f, "{}", line.replace('\n', "; ").replace("; ; ", "; "));
            }
        }
    }

    fn write(&self, s: &str) {
        let _ = sys::write_all(self.fd, s.as_bytes());
    }

    fn read_byte(&self) -> Option<u8> {
        let mut b = [0u8; 1];
        match sys::read(self.fd, &mut b) {
            Ok(1) => Some(b[0]),
            _ => None,
        }
    }

    /// Read one byte if more input arrives within `ms` (escape sequence parsing).
    fn read_byte_timeout(&self, ms: i32) -> Option<u8> {
        if sys::poll_read(self.fd, ms) { self.read_byte() } else { None }
    }

    fn read_key(&self) -> Option<Key> {
        let b = self.read_byte()?;
        Some(match b {
            b'\r' | b'\n' => Key::Enter,
            b'\t' => Key::Tab,
            0x7f | 0x08 => Key::Backspace,
            0x1b => self.read_escape(),
            1..=26 => Key::Ctrl((b'a' + b - 1) as char),
            0..=31 => Key::Unknown,
            _ => {
                // UTF-8 sequence.
                let len = match b {
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf7 => 4,
                    _ => 1,
                };
                let mut bytes = vec![b];
                for _ in 1..len {
                    bytes.push(self.read_byte()?);
                }
                match std::str::from_utf8(&bytes) {
                    Ok(s) => Key::Char(s.chars().next().unwrap_or('?')),
                    Err(_) => Key::Unknown,
                }
            }
        })
    }

    fn read_escape(&self) -> Key {
        let Some(b) = self.read_byte_timeout(30) else { return Key::Escape };
        match b {
            b'[' | b'O' => {
                let mut params = String::new();
                loop {
                    let Some(c) = self.read_byte_timeout(50) else { return Key::Unknown };
                    if (0x40..=0x7e).contains(&c) {
                        return match (params.as_str(), c) {
                            ("", b'A') => Key::Up,
                            ("", b'B') => Key::Down,
                            ("", b'C') => Key::Right,
                            ("", b'D') => Key::Left,
                            ("", b'H') | ("1", b'~') | ("7", b'~') => Key::Home,
                            ("", b'F') | ("4", b'~') | ("8", b'~') => Key::End,
                            ("3", b'~') => Key::Delete,
                            ("1;5", b'C') | ("1;3", b'C') => Key::WordRight,
                            ("1;5", b'D') | ("1;3", b'D') => Key::WordLeft,
                            ("200", b'~') => Key::PasteStart,
                            _ => Key::Unknown,
                        };
                    }
                    params.push(c as char);
                }
            }
            b'b' => Key::WordLeft,
            b'f' => Key::WordRight,
            b'd' => Key::AltD,
            b'.' => Key::AltDot,
            0x7f | 0x08 => Key::AltBackspace,
            _ => Key::Unknown,
        }
    }

    /// Read pasted text until ESC [201~.
    fn read_paste(&self) -> String {
        let mut bytes = Vec::new();
        let end = b"\x1b[201~";
        while let Some(b) = self.read_byte() {
            bytes.push(b);
            if bytes.ends_with(end) {
                bytes.truncate(bytes.len() - end.len());
                break;
            }
        }
        String::from_utf8_lossy(&bytes).replace("\r\n", "\n").replace('\r', "\n")
    }

    fn line(&self) -> String {
        self.buf.iter().collect()
    }

    fn suggestion(&self) -> Option<String> {
        if self.buf.is_empty() || self.pos != self.buf.len() {
            return None;
        }
        let line = self.line();
        self.history
            .iter()
            .rev()
            .find(|h| h.len() > line.len() && h.starts_with(&line))
            .map(|h| h[line.len()..].to_string())
    }

    fn refresh(&mut self, prompt: &str, helper: &mut dyn Helper, extra_hint: Option<&str>) {
        self.render(prompt, helper, extra_hint, true);
    }

    fn render(&mut self, prompt: &str, helper: &mut dyn Helper, extra_hint: Option<&str>, suggest: bool) {
        let cols = sys::terminal_width(self.fd).max(1);
        let line = self.line();
        let colored = helper.highlight(&line);
        let suggestion = if extra_hint.is_none() && suggest { self.suggestion() } else { None };
        let mut out = String::new();
        // Back to the first row of the prompt.
        if self.cursor_row > 0 {
            out.push_str(&format!("\x1b[{}A", self.cursor_row));
        }
        out.push_str("\r\x1b[J");
        out.push_str(&prompt.replace(['\u{1}', '\u{2}'], ""));
        out.push_str(&colored);
        out.push_str("\x1b[0m");
        let prompt_w = display_width(prompt);
        let text_w: usize = self.buf.iter().map(|&c| char_width(c)).sum();
        let mut end_w = prompt_w + text_w;
        if let Some(s) = &suggestion {
            out.push_str(&format!("\x1b[90m{s}\x1b[0m"));
            end_w += display_width(s);
        }
        if let Some(h) = extra_hint {
            out.push_str(h);
            end_w += display_width(h);
        }
        // Terminals defer wrapping at the right margin; force it so cursor math holds.
        if end_w > 0 && end_w % cols == 0 {
            out.push_str(" \r");
        }
        let end_row = end_w / cols;
        let cur_w = prompt_w + self.buf[..self.pos].iter().map(|&c| char_width(c)).sum::<usize>();
        let (cur_row, cur_col) = (cur_w / cols, cur_w % cols);
        if end_row > cur_row {
            out.push_str(&format!("\x1b[{}A", end_row - cur_row));
        }
        out.push('\r');
        if cur_col > 0 {
            out.push_str(&format!("\x1b[{cur_col}C"));
        }
        self.cursor_row = cur_row;
        self.write(&out);
    }

    /// Move below the edited text (before printing output or a newline).
    fn finish_line(&mut self, prompt: &str) {
        let cols = sys::terminal_width(self.fd).max(1);
        let prompt_w = display_width(prompt);
        let text_w: usize = self.buf.iter().map(|&c| char_width(c)).sum();
        let end_row = (prompt_w + text_w) / cols;
        if end_row > self.cursor_row {
            self.write(&format!("\x1b[{}B", end_row - self.cursor_row));
        }
        self.write("\r\n");
        self.cursor_row = 0;
    }

    pub fn read_line(&mut self, prompt: &str, helper: &mut dyn Helper) -> ReadResult {
        let Some(saved) = sys::get_termios(self.fd) else {
            return self.read_line_dumb(prompt);
        };
        sys::set_termios(self.fd, &sys::raw_termios(&saved));
        self.write("\x1b[?2004h"); // bracketed paste
        let r = self.edit(prompt, helper);
        self.write("\x1b[?2004l");
        sys::set_termios(self.fd, &saved);
        r
    }

    fn read_line_dumb(&mut self, prompt: &str) -> ReadResult {
        self.write(prompt);
        let mut line = Vec::new();
        loop {
            match self.read_byte() {
                None => {
                    return if line.is_empty() { ReadResult::Eof } else { ReadResult::Line(String::from_utf8_lossy(&line).into_owned()) };
                }
                Some(b'\n') => return ReadResult::Line(String::from_utf8_lossy(&line).into_owned()),
                Some(b) => line.push(b),
            }
        }
    }

    fn word_start(&self, from: usize) -> usize {
        let mut i = from;
        while i > 0 && !is_word_char(self.buf[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word_char(self.buf[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_end(&self, from: usize) -> usize {
        let mut i = from;
        while i < self.buf.len() && !is_word_char(self.buf[i]) {
            i += 1;
        }
        while i < self.buf.len() && is_word_char(self.buf[i]) {
            i += 1;
        }
        i
    }

    fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.buf.insert(self.pos, c);
            self.pos += 1;
        }
    }

    fn edit(&mut self, prompt: &str, helper: &mut dyn Helper) -> ReadResult {
        self.buf.clear();
        self.pos = 0;
        self.cursor_row = 0;
        self.last_tab = false;
        let mut hist_idx = self.history.len();
        let mut saved_line: Option<Vec<char>> = None;
        self.refresh(prompt, helper, None);
        loop {
            let Some(key) = self.read_key() else {
                self.finish_line(prompt);
                return ReadResult::Eof;
            };
            let was_tab = self.last_tab;
            self.last_tab = false;
            // Consecutive kills accumulate into one kill-buffer entry (like readline).
            let continuing_kill = std::mem::take(&mut self.last_kill);
            let is_kill = matches!(key, Key::Ctrl('k') | Key::Ctrl('u') | Key::Ctrl('w') | Key::AltBackspace | Key::AltD);
            if is_kill {
                self.last_kill = true;
                if !continuing_kill {
                    self.kill.clear();
                }
            }
            match key {
                Key::Enter => {
                    self.refresh_plain(prompt, helper);
                    self.finish_line(prompt);
                    return ReadResult::Line(self.line());
                }
                Key::Ctrl('d') => {
                    if self.buf.is_empty() {
                        self.finish_line(prompt);
                        return ReadResult::Eof;
                    }
                    if self.pos < self.buf.len() {
                        self.buf.remove(self.pos);
                    }
                }
                Key::Ctrl('c') => {
                    self.refresh_plain(prompt, helper);
                    self.write("^C");
                    self.finish_line(prompt);
                    self.buf.clear();
                    self.pos = 0;
                    hist_idx = self.history.len();
                    saved_line = None;
                }
                Key::Char(c) => {
                    self.buf.insert(self.pos, c);
                    self.pos += 1;
                }
                Key::PasteStart => {
                    let text = self.read_paste();
                    self.insert_str(&text);
                }
                Key::Backspace | Key::Ctrl('h') => {
                    if self.pos > 0 {
                        self.pos -= 1;
                        self.buf.remove(self.pos);
                    }
                }
                Key::Delete => {
                    if self.pos < self.buf.len() {
                        self.buf.remove(self.pos);
                    }
                }
                Key::Left | Key::Ctrl('b') => self.pos = self.pos.saturating_sub(1),
                Key::Right | Key::Ctrl('f') => {
                    if self.pos < self.buf.len() {
                        self.pos += 1;
                    } else if let Some(s) = self.suggestion() {
                        self.insert_str(&s);
                    }
                }
                Key::Home | Key::Ctrl('a') => self.pos = 0,
                Key::End | Key::Ctrl('e') => {
                    if self.pos == self.buf.len() {
                        if let Some(s) = self.suggestion() {
                            self.insert_str(&s);
                        }
                    }
                    self.pos = self.buf.len();
                }
                Key::WordLeft => self.pos = self.word_start(self.pos),
                Key::WordRight => {
                    if self.pos == self.buf.len() {
                        // Accept one word of the suggestion.
                        if let Some(s) = self.suggestion() {
                            let chars: Vec<char> = s.chars().collect();
                            let mut n = 0;
                            while n < chars.len() && !is_word_char(chars[n]) {
                                n += 1;
                            }
                            while n < chars.len() && is_word_char(chars[n]) {
                                n += 1;
                            }
                            let part: String = chars[..n].iter().collect();
                            self.insert_str(&part);
                        }
                    } else {
                        self.pos = self.word_end(self.pos);
                    }
                }
                Key::Ctrl('k') => {
                    let killed: String = self.buf.drain(self.pos..).collect();
                    self.kill.push_str(&killed);
                }
                Key::Ctrl('u') => {
                    let killed: String = self.buf.drain(..self.pos).collect();
                    self.kill.insert_str(0, &killed);
                    self.pos = 0;
                }
                Key::Ctrl('w') | Key::AltBackspace => {
                    let mut start = self.pos;
                    if key == Key::Ctrl('w') {
                        while start > 0 && self.buf[start - 1].is_whitespace() {
                            start -= 1;
                        }
                        while start > 0 && !self.buf[start - 1].is_whitespace() {
                            start -= 1;
                        }
                    } else {
                        start = self.word_start(self.pos);
                    }
                    let killed: String = self.buf.drain(start..self.pos).collect();
                    self.kill.insert_str(0, &killed);
                    self.pos = start;
                }
                Key::AltD => {
                    let end = self.word_end(self.pos);
                    let killed: String = self.buf.drain(self.pos..end).collect();
                    self.kill.push_str(&killed);
                }
                Key::Ctrl('y') => {
                    let k = self.kill.clone();
                    self.insert_str(&k);
                }
                Key::Ctrl('t') => {
                    if self.pos > 0 && self.buf.len() >= 2 {
                        let p = if self.pos == self.buf.len() { self.pos - 1 } else { self.pos };
                        self.buf.swap(p - 1, p);
                        self.pos = (p + 1).min(self.buf.len());
                    }
                }
                Key::AltDot => {
                    if let Some(last) = self.history.last() {
                        if let Some(word) = last.split_whitespace().last() {
                            let w = word.to_string();
                            self.insert_str(&w);
                        }
                    }
                }
                Key::Ctrl('l') => {
                    self.write("\x1b[H\x1b[2J");
                    self.cursor_row = 0;
                }
                Key::Up | Key::Ctrl('p') => {
                    if hist_idx > 0 {
                        if saved_line.is_none() {
                            saved_line = Some(self.buf.clone());
                        }
                        // Prefix search when the line was typed before pressing Up.
                        let prefix: String = saved_line.as_ref().unwrap().iter().collect();
                        let mut i = hist_idx;
                        while i > 0 {
                            i -= 1;
                            if self.history[i].starts_with(&prefix) && self.history[i] != self.line() {
                                hist_idx = i;
                                self.buf = self.history[i].chars().collect();
                                self.pos = self.buf.len();
                                break;
                            }
                        }
                    }
                }
                Key::Down | Key::Ctrl('n') => {
                    if let Some(saved) = &saved_line {
                        let prefix: String = saved.iter().collect();
                        let mut i = hist_idx;
                        let mut found = false;
                        while i + 1 < self.history.len() {
                            i += 1;
                            if self.history[i].starts_with(&prefix) {
                                hist_idx = i;
                                self.buf = self.history[i].chars().collect();
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            hist_idx = self.history.len();
                            self.buf = saved_line.take().unwrap();
                        }
                        self.pos = self.buf.len();
                    }
                }
                Key::Ctrl('r') => {
                    if let Some((line, accept)) = self.reverse_search(prompt, helper) {
                        self.buf = line.chars().collect();
                        self.pos = self.buf.len();
                        if accept {
                            self.refresh_plain(prompt, helper);
                            self.finish_line(prompt);
                            return ReadResult::Line(line);
                        }
                    }
                }
                Key::Tab => self.complete(prompt, helper, was_tab),
                _ => {}
            }
            // Coalesce redraws while more input is already queued (typing ahead, pastes
            // without bracketed paste, slow serial consoles).
            if !sys::poll_read(self.fd, 0) {
                self.refresh(prompt, helper, None);
            }
        }
    }

    /// Render without the suggestion (before accepting the line).
    fn refresh_plain(&mut self, prompt: &str, helper: &mut dyn Helper) {
        self.render(prompt, helper, None, false);
    }

    fn complete(&mut self, prompt: &str, helper: &mut dyn Helper, second: bool) {
        let line = self.line();
        let byte_pos: usize = self.buf[..self.pos].iter().map(|c| c.len_utf8()).sum();
        let (start_byte, mut cands) = helper.complete(&line, byte_pos);
        cands.sort();
        cands.dedup();
        if cands.is_empty() {
            self.write("\u{7}");
            return;
        }
        let start = line[..start_byte].chars().count();
        let typed: String = self.buf[start..self.pos].iter().collect();
        // Longest common prefix.
        let mut common: String = cands[0].clone();
        for c in &cands[1..] {
            let n = common.chars().zip(c.chars()).take_while(|(a, b)| a == b).count();
            common = common.chars().take(n).collect();
        }
        if cands.len() == 1 {
            let mut c = cands[0].clone();
            if !c.ends_with('/') {
                c.push(' ');
            }
            self.buf.splice(start..self.pos, c.chars());
            self.pos = start + c.chars().count();
            return;
        }
        if common.chars().count() > typed.chars().count() {
            self.buf.splice(start..self.pos, common.chars());
            self.pos = start + common.chars().count();
            self.last_tab = true;
            return;
        }
        if !second {
            self.last_tab = true;
            self.write("\u{7}");
            return;
        }
        // Second Tab: list candidates in columns below the line.
        self.finish_line(prompt);
        let cols = sys::terminal_width(self.fd).max(1);
        let display: Vec<String> = cands
            .iter()
            .map(|c| {
                let t = c.trim_end_matches('/');
                let name = t.rsplit('/').next().unwrap_or(t);
                if c.ends_with('/') { format!("{name}/") } else { name.to_string() }
            })
            .collect();
        let width = display.iter().map(|d| display_width(d)).max().unwrap_or(1) + 2;
        let per_row = (cols / width).max(1);
        let rows = display.len().div_ceil(per_row);
        let mut out = String::new();
        for r in 0..rows.min(40) {
            for c in 0..per_row {
                if let Some(d) = display.get(c * rows + r) {
                    let pad = width - display_width(d);
                    let colored = if d.ends_with('/') { format!("\x1b[1;34m{d}\x1b[0m") } else { d.clone() };
                    out.push_str(&colored);
                    if c + 1 < per_row {
                        out.push_str(&" ".repeat(pad));
                    }
                }
            }
            out.push_str("\r\n");
        }
        if rows > 40 {
            out.push_str(&format!("... and {} more\r\n", display.len() - 40 * per_row));
        }
        self.write(&out);
        self.cursor_row = 0;
    }

    /// Incremental history search. Returns the chosen line and whether Enter
    /// accepted it (run it) rather than another key (keep editing).
    fn reverse_search(&mut self, prompt: &str, helper: &mut dyn Helper) -> Option<(String, bool)> {
        let mut query = String::new();
        let mut idx = self.history.len();
        let mut found: Option<usize> = None;
        let original = self.buf.clone();
        loop {
            let shown = found.map(|i| self.history[i].clone()).unwrap_or_default();
            self.buf = shown.chars().collect();
            self.pos = shown.find(&query).map(|b| shown[..b].chars().count()).unwrap_or(0);
            let hint = format!("   \x1b[90m(search: {query}{})\x1b[0m", if found.is_none() && !query.is_empty() { " — no match" } else { "" });
            let pos = self.pos;
            self.pos = self.buf.len();
            self.refresh(prompt, helper, Some(&hint));
            self.pos = pos;
            let key = self.read_key()?;
            match key {
                Key::Char(c) => {
                    query.push(c);
                    idx = self.history.len();
                }
                Key::Backspace => {
                    query.pop();
                    idx = self.history.len();
                }
                Key::Ctrl('r') => {
                    if let Some(f) = found {
                        idx = f;
                    }
                }
                Key::Ctrl('g') | Key::Ctrl('c') | Key::Escape => {
                    self.buf = original;
                    self.pos = self.buf.len();
                    return None;
                }
                Key::Enter | Key::Right | Key::Left | Key::Ctrl('e') | Key::Ctrl('a') => {
                    let line = found.map(|i| self.history[i].clone()).unwrap_or_else(|| original.iter().collect());
                    return Some((line, key == Key::Enter));
                }
                _ => {}
            }
            found = (0..idx).rev().find(|&i| !query.is_empty() && self.history[i].contains(&query));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("\x1b[1;32mabc\x1b[0m"), 3);
        assert_eq!(display_width("\u{1}\x1b[31m\u{2}x"), 1);
        assert_eq!(display_width("日本"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width("\x1b]0;title\x07$ "), 2);
    }
}
