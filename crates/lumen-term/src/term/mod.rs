//! Terminal emulation: an xterm-compatible state machine fed by the `vte`
//! parser. Pure logic, no I/O: bytes go in, the grid changes and replies for
//! the application (device reports) collect in `replies`.

pub mod cell;
pub mod grid;
mod osc;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod tests;

use unicode_width::UnicodeWidthChar;

pub use self::cell::{attr, Cell, Color, Underline};
pub use self::grid::{Grid, Row};

/// Lines of scrollback by default.
pub const DEFAULT_HISTORY: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseMode {
    #[default]
    None,
    /// X10: button presses only.
    Press,
    /// 1000: presses and releases.
    Normal,
    /// 1002: plus motion while a button is held.
    Button,
    /// 1003: all motion.
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseEncoding {
    #[default]
    X10,
    Utf8,
    Sgr,
    Urxvt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Charset {
    #[default]
    Ascii,
    /// DEC Special Graphics (line drawing).
    DecSpecial,
    /// UK national: '#' is '£'.
    Uk,
}

impl Charset {
    fn map(self, c: char) -> char {
        match self {
            Charset::Ascii => c,
            Charset::Uk => {
                if c == '#' {
                    '£'
                } else {
                    c
                }
            }
            Charset::DecSpecial => match c {
                '`' => '◆',
                'a' => '▒',
                'b' => '␉',
                'c' => '␌',
                'd' => '␍',
                'e' => '␊',
                'f' => '°',
                'g' => '±',
                'h' => '␤',
                'i' => '␋',
                'j' => '┘',
                'k' => '┐',
                'l' => '┌',
                'm' => '└',
                'n' => '┼',
                'o' => '⎺',
                'p' => '⎻',
                'q' => '─',
                'r' => '⎼',
                's' => '⎽',
                't' => '├',
                'u' => '┤',
                'v' => '┴',
                'w' => '┬',
                'x' => '│',
                'y' => '≤',
                'z' => '≥',
                '{' => 'π',
                '|' => '≠',
                '}' => '£',
                '~' => '·',
                '_' => ' ',
                _ => c,
            },
        }
    }
}

/// Mode flags (DEC private and ANSI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub origin: bool,
    pub autowrap: bool,
    pub insert: bool,
    pub newline: bool,
    pub show_cursor: bool,
    pub cursor_blink: bool,
    pub reverse_video: bool,
    pub mouse: MouseMode,
    pub mouse_encoding: MouseEncoding,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    /// Mouse wheel sends arrow keys on the alternate screen (1007).
    pub alternate_scroll: bool,
    /// Synchronized output (2026): hold rendering until reset.
    pub sync: bool,
    pub reverse_wrap: bool,
}

impl Default for Modes {
    fn default() -> Self {
        Modes {
            app_cursor: false,
            app_keypad: false,
            origin: false,
            autowrap: true,
            insert: false,
            newline: false,
            show_cursor: true,
            cursor_blink: true,
            reverse_video: false,
            mouse: MouseMode::None,
            mouse_encoding: MouseEncoding::X10,
            bracketed_paste: false,
            focus_events: false,
            alternate_scroll: true,
            sync: false,
            reverse_wrap: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Cursor {
    pub x: usize,
    pub y: usize,
    /// Current SGR attributes (as a template cell).
    pub pen: Cell,
    /// The last column was written: the next printable wraps first.
    pub pending_wrap: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Saved {
    cursor: Cursor,
    charsets: [Charset; 4],
    gl: usize,
    origin: bool,
    autowrap: bool,
}

/// Requests for the embedding UI (clipboard, bell...), drained by it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Bell,
    Title(String),
    /// OSC 52 write: target ('c' clipboard, 'p' primary) and text.
    ClipboardSet(char, String),
    /// OSC 52 read request; the UI may answer with `clipboard_reply`.
    ClipboardQuery(char),
}

pub struct Term {
    primary: Grid,
    alt: Grid,
    pub alt_active: bool,
    pub cursor: Cursor,
    saved_primary: Saved,
    saved_alt: Saved,
    pub modes: Modes,
    pub cursor_shape: CursorShape,
    /// Scroll region, inclusive.
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    tabs: Vec<bool>,
    charsets: [Charset; 4],
    gl: usize,
    single_shift: Option<usize>,
    pub title: String,
    title_stack: Vec<String>,
    /// Palette overrides (OSC 4) and dynamic colors (OSC 10/11/12).
    pub palette: [Option<(u8, u8, u8)>; 256],
    pub fg_override: Option<(u8, u8, u8)>,
    pub bg_override: Option<(u8, u8, u8)>,
    pub cursor_override: Option<(u8, u8, u8)>,
    /// The configured colors, for answering queries.
    pub base_palette: [(u8, u8, u8); 256],
    pub base_fg: (u8, u8, u8),
    pub base_bg: (u8, u8, u8),
    pub base_cursor: (u8, u8, u8),
    /// Bytes to send back to the application (device reports).
    pub replies: Vec<u8>,
    pub events: Vec<Event>,
    /// Hyperlinks by id (index + 1); `link_ids` maps OSC 8 `id=` values.
    pub links: Vec<String>,
    current_link: u16,
    last_char: Option<char>,
    /// Something visible changed since the UI last looked.
    pub changed: bool,
    /// Lines that scrolled into history so far (the UI keeps a scrolled-back
    /// view anchored with it).
    pub scrolled: u64,
    /// Cell size in pixels, for size reports (set by the UI).
    pub cell_px: (u32, u32),
    dcs: Option<Dcs>,
    /// The last printed character was a zero width joiner.
    zwj: bool,
}

struct Dcs {
    intermediates: Vec<u8>,
    action: char,
    data: Vec<u8>,
}

/// The terminal plus its parser.
pub struct Terminal {
    parser: vte::Parser,
    pub term: Term,
}

impl Terminal {
    pub fn new(cols: usize, rows: usize, history: usize) -> Terminal {
        Terminal { parser: vte::Parser::new(), term: Term::new(cols, rows, history) }
    }

    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }
}

/// The xterm 256-color palette with the given 16 base colors.
pub fn xterm_palette(base16: &[(u8, u8, u8); 16]) -> [(u8, u8, u8); 256] {
    let mut p = [(0u8, 0u8, 0u8); 256];
    p[..16].copy_from_slice(base16);
    let steps = [0u8, 95, 135, 175, 215, 255];
    for i in 0..216 {
        p[16 + i] = (steps[i / 36], steps[(i / 6) % 6], steps[i % 6]);
    }
    for i in 0..24 {
        let v = (8 + i * 10) as u8;
        p[232 + i] = (v, v, v);
    }
    p
}

fn default_tabs(cols: usize) -> Vec<bool> {
    (0..cols).map(|x| x % 8 == 0 && x > 0).collect()
}

impl Term {
    pub fn new(cols: usize, rows: usize, history: usize) -> Term {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let base16 = [
            (0x1d, 0x1f, 0x28),
            (0xf2, 0x6d, 0x78),
            (0x9e, 0xce, 0x6a),
            (0xe0, 0xaf, 0x68),
            (0x7a, 0xa2, 0xf7),
            (0xbb, 0x9a, 0xf7),
            (0x7d, 0xcf, 0xff),
            (0xc0, 0xca, 0xf5),
            (0x41, 0x48, 0x68),
            (0xff, 0x7a, 0x93),
            (0xb9, 0xf2, 0x7c),
            (0xff, 0x9e, 0x64),
            (0x7d, 0xa6, 0xff),
            (0xbb, 0x9a, 0xf7),
            (0x0d, 0xb9, 0xd7),
            (0xe6, 0xe9, 0xf5),
        ];
        Term {
            primary: Grid::new(cols, rows, history),
            alt: Grid::new(cols, rows, 0),
            alt_active: false,
            cursor: Cursor::default(),
            saved_primary: Saved { autowrap: true, ..Default::default() },
            saved_alt: Saved { autowrap: true, ..Default::default() },
            modes: Modes::default(),
            cursor_shape: CursorShape::Block,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            tabs: default_tabs(cols),
            charsets: [Charset::Ascii; 4],
            gl: 0,
            single_shift: None,
            title: String::new(),
            title_stack: Vec::new(),
            palette: [None; 256],
            fg_override: None,
            bg_override: None,
            cursor_override: None,
            base_palette: xterm_palette(&base16),
            base_fg: (0xd8, 0xdc, 0xec),
            base_bg: (0x12, 0x13, 0x1a),
            base_cursor: (0xd8, 0xdc, 0xec),
            replies: Vec::new(),
            events: Vec::new(),
            links: Vec::new(),
            current_link: 0,
            last_char: None,
            changed: true,
            scrolled: 0,
            cell_px: (8, 16),
            dcs: None,
            zwj: false,
        }
    }

    pub fn grid(&self) -> &Grid {
        if self.alt_active { &self.alt } else { &self.primary }
    }

    pub fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_active { &mut self.alt } else { &mut self.primary }
    }

    pub fn cols(&self) -> usize {
        self.grid().cols
    }

    pub fn rows(&self) -> usize {
        self.grid().rows
    }

    /// Effective palette color `i` (overrides included).
    pub fn color(&self, i: u8) -> (u8, u8, u8) {
        self.palette[i as usize].unwrap_or(self.base_palette[i as usize])
    }

    pub fn fg(&self) -> (u8, u8, u8) {
        self.fg_override.unwrap_or(self.base_fg)
    }

    pub fn bg(&self) -> (u8, u8, u8) {
        self.bg_override.unwrap_or(self.base_bg)
    }

    pub fn cursor_color(&self) -> (u8, u8, u8) {
        self.cursor_override.unwrap_or(self.base_cursor)
    }

    /// Text of screen row `y` (for tests and dumps), trailing blanks trimmed.
    pub fn row_text(&self, y: usize) -> String {
        let r = self.grid().row(y);
        r.text(0, r.content_len()).trim_end().to_string()
    }

    pub fn screen_text(&self) -> String {
        (0..self.rows()).map(|y| self.row_text(y)).collect::<Vec<_>>().join("\n")
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols() && rows == self.rows() {
            return;
        }
        let blank = Cell::default();
        let cur = (self.cursor.x, self.cursor.y);
        if self.alt_active {
            let moved = self.alt.resize_simple(cols, rows, cur.1, blank);
            self.cursor.y = (cur.1 as isize - moved).clamp(0, rows as isize - 1) as usize;
            // The primary screen reflows around its saved cursor.
            let sc = (self.saved_primary.cursor.x, self.saved_primary.cursor.y);
            let (x, y) = self.primary.resize_reflow(cols, rows, sc, blank);
            self.saved_primary.cursor.x = x;
            self.saved_primary.cursor.y = y;
        } else {
            let (x, y) = self.primary.resize_reflow(cols, rows, cur, blank);
            self.cursor.x = x;
            self.cursor.y = y;
            self.alt.resize_simple(cols, rows, 0, blank);
        }
        self.cursor.x = self.cursor.x.min(cols - 1);
        self.cursor.y = self.cursor.y.min(rows - 1);
        self.cursor.pending_wrap = false;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        let old = self.tabs.len();
        self.tabs.resize(cols, false);
        for x in old..cols {
            self.tabs[x] = x % 8 == 0;
        }
        self.changed = true;
    }

    fn blank(&self) -> Cell {
        Cell::blank(&self.cursor.pen)
    }

    // -- cursor movement --------------------------------------------------------------

    fn clamp_x(&self, x: usize) -> usize {
        x.min(self.cols() - 1)
    }

    pub fn goto(&mut self, x: usize, y: usize) {
        let (top, bottom) = if self.modes.origin { (self.scroll_top, self.scroll_bottom) } else { (0, self.rows() - 1) };
        let y = if self.modes.origin { y + top } else { y };
        self.cursor.x = self.clamp_x(x);
        self.cursor.y = y.clamp(top, bottom);
        self.cursor.pending_wrap = false;
    }

    fn move_up(&mut self, n: usize) {
        let top = if self.cursor.y >= self.scroll_top { self.scroll_top } else { 0 };
        self.cursor.y = self.cursor.y.saturating_sub(n).max(top);
        self.cursor.pending_wrap = false;
    }

    fn move_down(&mut self, n: usize) {
        let bottom = if self.cursor.y <= self.scroll_bottom { self.scroll_bottom } else { self.rows() - 1 };
        self.cursor.y = (self.cursor.y + n).min(bottom);
        self.cursor.pending_wrap = false;
    }

    fn move_left(&mut self, n: usize) {
        self.cursor.x = self.cursor.x.saturating_sub(n);
        self.cursor.pending_wrap = false;
    }

    fn move_right(&mut self, n: usize) {
        self.cursor.x = self.clamp_x(self.cursor.x + n);
        self.cursor.pending_wrap = false;
    }

    pub fn linefeed(&mut self) {
        if self.cursor.y == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.y < self.rows() - 1 {
            self.cursor.y += 1;
        }
        self.cursor.pending_wrap = false;
    }

    fn reverse_index(&mut self) {
        if self.cursor.y == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.y > 0 {
            self.cursor.y -= 1;
        }
        self.cursor.pending_wrap = false;
    }

    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let blank = self.blank();
        let history = !self.alt_active;
        let full = top == 0 && bottom == self.rows() - 1;
        self.grid_mut().scroll_up(top, bottom, n, blank, history);
        if full && history {
            self.scrolled += n.min(bottom + 1 - top) as u64;
        }
        self.changed = true;
    }

    fn scroll_down(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let blank = self.blank();
        self.grid_mut().scroll_down(top, bottom, n, blank);
        self.changed = true;
    }

    fn tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cursor.x + 1;
            while x < self.cols() - 1 && !self.tabs[x] {
                x += 1;
            }
            self.cursor.x = x.min(self.cols() - 1);
        }
        self.cursor.pending_wrap = false;
    }

    fn back_tab(&mut self, n: usize) {
        for _ in 0..n {
            let mut x = self.cursor.x;
            while x > 0 {
                x -= 1;
                if self.tabs[x] {
                    break;
                }
            }
            self.cursor.x = x;
        }
        self.cursor.pending_wrap = false;
    }

    // -- printing ---------------------------------------------------------------------

    /// Fix up wide characters around `x..x+w` before overwriting those cells.
    fn clear_wide_around(&mut self, y: usize, x: usize, w: usize) {
        let cols = self.cols();
        let row = self.grid_mut().row_mut(y);
        // Overwriting the spacer half: blank the wide char's first half.
        if x > 0 && row.cells[x].has(attr::SPACER) {
            row.cells[x - 1].ch = ' ';
            row.cells[x - 1].attrs &= !(attr::WIDE | attr::EXTRA);
        }
        // Overwriting the first half: blank its spacer.
        let end = (x + w).min(cols);
        if end > x && end < cols && row.cells[end - 1].has(attr::WIDE) {
            row.cells[end].ch = ' ';
            row.cells[end].attrs &= !attr::SPACER;
        }
        if end > x && end < cols && row.cells[end].has(attr::SPACER) && !row.cells[end - 1].has(attr::WIDE) {
            row.cells[end].attrs &= !attr::SPACER;
        }
        row.clear_extras(x, end);
    }

    fn print_char(&mut self, c: char) {
        let ascii = (c as u32) >= 0x20 && (c as u32) < 0x7f;
        let c = if let Some(g) = self.single_shift.take() {
            self.charsets[g].map(c)
        } else if ascii && self.charsets[self.gl] != Charset::Ascii {
            self.charsets[self.gl].map(c)
        } else {
            c
        };
        let mut width = if ascii { 1 } else { c.width().unwrap_or(0) };
        if width == 0 {
            self.combine(c);
            return;
        }
        // Flags are pairs of regional indicators shown as one wide glyph.
        if is_regional_indicator(c) {
            if let Some((x, y)) = self.previous_cell() {
                let row = self.grid().row(y);
                if is_regional_indicator(row.cells[x].ch) && !row.cells[x].has(attr::EXTRA) {
                    self.combine(c);
                    return;
                }
            }
            width = 2;
        }
        // Emoji sequences: after a zero width joiner the next pictograph
        // belongs to the same grapheme.
        if self.zwj {
            self.zwj = false;
            if !ascii {
                self.combine(c);
                return;
            }
        }
        let cols = self.cols();
        if self.cursor.pending_wrap && self.modes.autowrap {
            let y = self.cursor.y;
            self.grid_mut().row_mut(y).wrapped = true;
            self.cursor.x = 0;
            self.linefeed();
        }
        if width == 2 && self.cursor.x == cols - 1 {
            if self.modes.autowrap && cols > 1 {
                // A wide char does not fit in the last column: wrap first.
                let (x, y) = (self.cursor.x, self.cursor.y);
                let blank = self.blank();
                self.clear_wide_around(y, x, 1);
                let row = self.grid_mut().row_mut(y);
                row.cells[x] = blank;
                row.wrapped = true;
                self.cursor.x = 0;
                self.linefeed();
            } else {
                return;
            }
        }
        let (x, y) = (self.cursor.x, self.cursor.y);
        if self.modes.insert {
            self.insert_blanks(width);
        }
        let mut cell = self.cursor.pen;
        cell.ch = c;
        cell.link = self.current_link;
        cell.attrs &= attr::STYLE;
        const LAYOUT: u16 = attr::WIDE | attr::SPACER | attr::EXTRA;
        let simple = {
            let row = self.grid().row(y);
            row.cells[x].attrs & LAYOUT == 0
                && (width == 1 || x + 1 >= cols || row.cells[x + 1].attrs & LAYOUT == 0)
                && (x + width >= cols || row.cells[x + width].attrs & attr::SPACER == 0)
        };
        if !simple {
            self.clear_wide_around(y, x, width);
        }
        let row = self.grid_mut().row_mut(y);
        if width == 2 {
            cell.attrs |= attr::WIDE;
            row.cells[x] = cell;
            if x + 1 < cols {
                row.cells[x + 1] = Cell { ch: ' ', attrs: (cell.attrs & attr::STYLE) | attr::SPACER, ..cell };
            }
        } else {
            row.cells[x] = cell;
        }
        self.last_char = Some(c);
        if x + width >= cols {
            self.cursor.x = cols - 1;
            self.cursor.pending_wrap = true;
        } else {
            self.cursor.x = x + width;
        }
        self.changed = true;
    }

    /// The cell a zero-width character attaches to: the one before the cursor
    /// (or under it while a wrap is pending).
    fn previous_cell(&self) -> Option<(usize, usize)> {
        let y = self.cursor.y;
        let mut x = if self.cursor.pending_wrap { self.cursor.x } else { self.cursor.x.checked_sub(1)? };
        let row = self.grid().row(y);
        if row.cells[x].has(attr::SPACER) && x > 0 {
            x -= 1;
        }
        Some((x, y))
    }

    fn combine(&mut self, c: char) {
        self.zwj = c == '\u{200D}';
        let Some((x, y)) = self.previous_cell() else { return };
        let row = self.grid_mut().row_mut(y);
        if row.cells[x].ch == ' ' && !row.cells[x].has(attr::EXTRA) && c != '\u{200D}' {
            // Nothing to combine with: show the mark on its own.
            row.cells[x].ch = c;
        } else {
            row.push_extra(x, c);
        }
        self.changed = true;
    }

    // -- editing ----------------------------------------------------------------------

    fn insert_blanks(&mut self, n: usize) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols();
        let n = n.min(cols - x);
        let blank = self.blank();
        self.clear_wide_around(y, x, 0);
        let row = self.grid_mut().row_mut(y);
        row.cells[x..].rotate_right(n);
        row.cells[x..x + n].fill(blank);
        row.shift_extras(x, n as isize, cols);
        // A wide char pushed to the edge loses its spacer.
        if row.cells[cols - 1].has(attr::WIDE) {
            row.cells[cols - 1] = blank;
        }
        self.changed = true;
    }

    fn delete_chars(&mut self, n: usize) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols();
        let n = n.min(cols - x);
        let blank = self.blank();
        self.clear_wide_around(y, x, n);
        let row = self.grid_mut().row_mut(y);
        row.clear_extras(x, x + n);
        row.cells[x..].rotate_left(n);
        row.cells[cols - n..].fill(blank);
        row.shift_extras(x + n, -(n as isize), cols);
        if row.cells[x].has(attr::SPACER) {
            row.cells[x] = blank;
        }
        self.cursor.pending_wrap = false;
        self.changed = true;
    }

    fn erase_chars(&mut self, n: usize) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let end = (x + n).min(self.cols());
        self.erase_range(y, x, end);
        self.cursor.pending_wrap = false;
    }

    fn erase_range(&mut self, y: usize, from: usize, to: usize) {
        if from >= to {
            return;
        }
        let blank = self.blank();
        self.clear_wide_around(y, from, to - from);
        let row = self.grid_mut().row_mut(y);
        row.cells[from..to].fill(blank);
        row.clear_extras(from, to);
        if to >= row.cells.len() {
            row.wrapped = false;
        }
        self.changed = true;
    }

    fn erase_line(&mut self, mode: u16) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let cols = self.cols();
        match mode {
            0 => self.erase_range(y, x, cols),
            1 => self.erase_range(y, 0, x + 1),
            2 => self.erase_range(y, 0, cols),
            _ => {}
        }
        self.cursor.pending_wrap = false;
    }

    fn erase_display(&mut self, mode: u16) {
        let (x, y) = (self.cursor.x, self.cursor.y);
        let (cols, rows) = (self.cols(), self.rows());
        match mode {
            0 => {
                self.erase_range(y, x, cols);
                for r in y + 1..rows {
                    self.erase_range(r, 0, cols);
                }
            }
            1 => {
                for r in 0..y {
                    self.erase_range(r, 0, cols);
                }
                self.erase_range(y, 0, x + 1);
            }
            2 => {
                for r in 0..rows {
                    self.erase_range(r, 0, cols);
                }
            }
            3 => {
                if !self.alt_active {
                    self.primary.clear_history();
                }
            }
            _ => {}
        }
        self.cursor.pending_wrap = false;
        self.changed = true;
    }

    fn insert_lines(&mut self, n: usize) {
        let y = self.cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let blank = self.blank();
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_down(y, bottom, n, blank);
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
        self.changed = true;
    }

    fn delete_lines(&mut self, n: usize) {
        let y = self.cursor.y;
        if y < self.scroll_top || y > self.scroll_bottom {
            return;
        }
        let blank = self.blank();
        let bottom = self.scroll_bottom;
        self.grid_mut().scroll_up(y, bottom, n, blank, false);
        self.cursor.x = 0;
        self.cursor.pending_wrap = false;
        self.changed = true;
    }

    fn repeat_last(&mut self, n: usize) {
        if let Some(c) = self.last_char {
            for _ in 0..n.min(65535) {
                self.print_char(c);
            }
        }
    }

    // -- state ------------------------------------------------------------------------

    fn save_cursor(&mut self) {
        let s = Saved {
            cursor: self.cursor,
            charsets: self.charsets,
            gl: self.gl,
            origin: self.modes.origin,
            autowrap: self.modes.autowrap,
        };
        if self.alt_active {
            self.saved_alt = s;
        } else {
            self.saved_primary = s;
        }
    }

    fn restore_cursor(&mut self) {
        let s = if self.alt_active { self.saved_alt } else { self.saved_primary };
        self.cursor = s.cursor;
        self.cursor.x = self.cursor.x.min(self.cols() - 1);
        self.cursor.y = self.cursor.y.min(self.rows() - 1);
        self.charsets = s.charsets;
        self.gl = s.gl;
        self.modes.origin = s.origin;
        self.modes.autowrap = s.autowrap;
    }

    fn enter_alt(&mut self, save: bool, clear: bool) {
        if self.alt_active {
            return;
        }
        if save {
            self.save_cursor();
        }
        self.alt_active = true;
        if clear {
            let blank = self.blank();
            for y in 0..self.rows() {
                self.alt.row_mut(y).reset(blank);
            }
        }
        self.changed = true;
    }

    fn leave_alt(&mut self, restore: bool) {
        if !self.alt_active {
            return;
        }
        self.alt_active = false;
        if restore {
            self.restore_cursor();
        }
        self.changed = true;
    }

    pub fn reset(&mut self) {
        let (cols, rows) = (self.cols(), self.rows());
        self.alt_active = false;
        let blank = Cell::default();
        for y in 0..rows {
            self.primary.row_mut(y).reset(blank);
            self.alt.row_mut(y).reset(blank);
        }
        self.cursor = Cursor::default();
        self.saved_primary = Saved { autowrap: true, ..Default::default() };
        self.saved_alt = Saved { autowrap: true, ..Default::default() };
        self.modes = Modes::default();
        self.cursor_shape = CursorShape::Block;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
        self.tabs = default_tabs(cols);
        self.charsets = [Charset::Ascii; 4];
        self.gl = 0;
        self.single_shift = None;
        self.palette = [None; 256];
        self.fg_override = None;
        self.bg_override = None;
        self.cursor_override = None;
        self.current_link = 0;
        self.changed = true;
    }

    fn soft_reset(&mut self) {
        self.modes.show_cursor = true;
        self.modes.insert = false;
        self.modes.origin = false;
        self.modes.autowrap = true;
        self.modes.app_cursor = false;
        self.modes.app_keypad = false;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows() - 1;
        self.charsets = [Charset::Ascii; 4];
        self.gl = 0;
        self.cursor.pen = Cell::default();
        self.cursor.pending_wrap = false;
        self.saved_primary = Saved { autowrap: true, ..Default::default() };
        self.saved_alt = Saved { autowrap: true, ..Default::default() };
    }

    fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let rows = self.rows();
        let top = top.saturating_sub(1);
        let bottom = if bottom == 0 { rows - 1 } else { (bottom - 1).min(rows - 1) };
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.goto(0, 0);
        }
    }

    fn decaln(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows() - 1;
        let cell = Cell { ch: 'E', ..Cell::default() };
        for y in 0..self.rows() {
            let row = self.grid_mut().row_mut(y);
            row.cells.fill(cell);
            row.extras.clear();
            row.wrapped = false;
        }
        self.cursor.x = 0;
        self.cursor.y = 0;
        self.cursor.pending_wrap = false;
        self.changed = true;
    }

    fn reply(&mut self, s: &str) {
        self.replies.extend_from_slice(s.as_bytes());
    }

    fn report_cursor(&mut self, private: bool) {
        let y = if self.modes.origin { self.cursor.y.saturating_sub(self.scroll_top) } else { self.cursor.y };
        let q = if private { "?" } else { "" };
        let s = format!("\x1b[{q}{};{}R", y + 1, self.cursor.x + 1);
        self.reply(&s);
    }

    fn set_private_mode(&mut self, mode: u16, on: bool) {
        match mode {
            1 => self.modes.app_cursor = on,
            3 => {
                // DECCOLM: the window keeps its width, but the side effects
                // (clear, margins reset, home) are what applications rely on.
                self.scroll_top = 0;
                self.scroll_bottom = self.rows() - 1;
                self.erase_display(2);
                self.goto(0, 0);
            }
            5 => self.modes.reverse_video = on,
            6 => {
                self.modes.origin = on;
                self.goto(0, 0);
            }
            7 => self.modes.autowrap = on,
            9 => self.modes.mouse = if on { MouseMode::Press } else { MouseMode::None },
            12 => self.modes.cursor_blink = on,
            25 => self.modes.show_cursor = on,
            45 => self.modes.reverse_wrap = on,
            47 => {
                if on {
                    self.enter_alt(false, false)
                } else {
                    self.leave_alt(false)
                }
            }
            66 => self.modes.app_keypad = on,
            1000 => self.modes.mouse = if on { MouseMode::Normal } else { MouseMode::None },
            1002 => self.modes.mouse = if on { MouseMode::Button } else { MouseMode::None },
            1003 => self.modes.mouse = if on { MouseMode::Any } else { MouseMode::None },
            1004 => self.modes.focus_events = on,
            1005 => self.modes.mouse_encoding = if on { MouseEncoding::Utf8 } else { MouseEncoding::X10 },
            1006 => self.modes.mouse_encoding = if on { MouseEncoding::Sgr } else { MouseEncoding::X10 },
            1007 => self.modes.alternate_scroll = on,
            1015 => self.modes.mouse_encoding = if on { MouseEncoding::Urxvt } else { MouseEncoding::X10 },
            1047 => {
                if on {
                    self.enter_alt(false, true)
                } else {
                    // Clear the alternate screen on the way out.
                    let blank = self.blank();
                    for y in 0..self.rows() {
                        self.alt.row_mut(y).reset(blank);
                    }
                    self.leave_alt(false)
                }
            }
            1048 => {
                if on {
                    self.save_cursor()
                } else {
                    self.restore_cursor()
                }
            }
            1049 => {
                if on {
                    self.enter_alt(true, true)
                } else {
                    self.leave_alt(true)
                }
            }
            2004 => self.modes.bracketed_paste = on,
            2026 => self.modes.sync = on,
            _ => {}
        }
        self.changed = true;
    }

    fn private_mode_state(&self, mode: u16) -> u8 {
        let b = |v: bool| if v { 1 } else { 2 };
        match mode {
            1 => b(self.modes.app_cursor),
            3 => 2,
            5 => b(self.modes.reverse_video),
            6 => b(self.modes.origin),
            7 => b(self.modes.autowrap),
            9 => b(self.modes.mouse == MouseMode::Press),
            12 => b(self.modes.cursor_blink),
            25 => b(self.modes.show_cursor),
            45 => b(self.modes.reverse_wrap),
            47 | 1047 | 1049 => b(self.alt_active),
            66 => b(self.modes.app_keypad),
            1000 => b(self.modes.mouse == MouseMode::Normal),
            1002 => b(self.modes.mouse == MouseMode::Button),
            1003 => b(self.modes.mouse == MouseMode::Any),
            1004 => b(self.modes.focus_events),
            1005 => b(self.modes.mouse_encoding == MouseEncoding::Utf8),
            1006 => b(self.modes.mouse_encoding == MouseEncoding::Sgr),
            1007 => b(self.modes.alternate_scroll),
            1015 => b(self.modes.mouse_encoding == MouseEncoding::Urxvt),
            2004 => b(self.modes.bracketed_paste),
            2026 => b(self.modes.sync),
            _ => 0,
        }
    }

    fn sgr(&mut self, params: &vte::Params) {
        let pen = &mut self.cursor.pen;
        let mut it = params.iter().peekable();
        if params.is_empty() {
            *pen = Cell::default();
            return;
        }
        while let Some(p) = it.next() {
            let code = p[0];
            match code {
                0 => *pen = Cell::default(),
                1 => pen.attrs |= attr::BOLD,
                2 => pen.attrs |= attr::DIM,
                3 => pen.attrs |= attr::ITALIC,
                4 => {
                    pen.underline = match p.get(1).copied() {
                        None | Some(1) => Underline::Single,
                        Some(0) => Underline::None,
                        Some(2) => Underline::Double,
                        Some(3) => Underline::Curly,
                        Some(4) => Underline::Dotted,
                        Some(5) => Underline::Dashed,
                        Some(_) => Underline::Single,
                    }
                }
                5 | 6 => pen.attrs |= attr::BLINK,
                7 => pen.attrs |= attr::INVERSE,
                8 => pen.attrs |= attr::HIDDEN,
                9 => pen.attrs |= attr::STRIKE,
                21 => pen.underline = Underline::Double,
                22 => pen.attrs &= !(attr::BOLD | attr::DIM),
                23 => pen.attrs &= !attr::ITALIC,
                24 => pen.underline = Underline::None,
                25 => pen.attrs &= !attr::BLINK,
                27 => pen.attrs &= !attr::INVERSE,
                28 => pen.attrs &= !attr::HIDDEN,
                29 => pen.attrs &= !attr::STRIKE,
                30..=37 => pen.fg = Color::Indexed((code - 30) as u8),
                38 | 48 | 58 => {
                    let color = if p.len() > 1 {
                        // Colon form: 38:5:n or 38:2:[cs]:r:g:b.
                        match p[1] {
                            5 if p.len() > 2 => Some(Color::Indexed(p[2] as u8)),
                            2 if p.len() >= 6 => Some(Color::Rgb(p[3] as u8, p[4] as u8, p[5] as u8)),
                            2 if p.len() == 5 => Some(Color::Rgb(p[2] as u8, p[3] as u8, p[4] as u8)),
                            _ => None,
                        }
                    } else {
                        // Semicolon form: 38;5;n or 38;2;r;g;b.
                        match it.next().map(|q| q[0]) {
                            Some(5) => it.next().map(|q| Color::Indexed(q[0] as u8)),
                            Some(2) => {
                                let r = it.next().map(|q| q[0] as u8);
                                let g = it.next().map(|q| q[0] as u8);
                                let b = it.next().map(|q| q[0] as u8);
                                match (r, g, b) {
                                    (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                                    _ => None,
                                }
                            }
                            _ => None,
                        }
                    };
                    if let Some(c) = color {
                        match code {
                            38 => pen.fg = c,
                            48 => pen.bg = c,
                            _ => pen.ul_color = c,
                        }
                    }
                }
                39 => pen.fg = Color::Default,
                40..=47 => pen.bg = Color::Indexed((code - 40) as u8),
                49 => pen.bg = Color::Default,
                53 => pen.attrs |= attr::OVERLINE,
                55 => pen.attrs &= !attr::OVERLINE,
                59 => pen.ul_color = Color::Default,
                90..=97 => pen.fg = Color::Indexed((code - 90 + 8) as u8),
                100..=107 => pen.bg = Color::Indexed((code - 100 + 8) as u8),
                _ => {}
            }
        }
    }

    fn dcs_finish(&mut self, d: Dcs) {
        // DECRQSS: report the current setting of a control function.
        if d.intermediates == b"$" && d.action == 'q' {
            let req = String::from_utf8_lossy(&d.data).into_owned();
            let answer = match req.as_str() {
                "m" => Some(format!("{}m", self.sgr_string())),
                "r" => Some(format!("{};{}r", self.scroll_top + 1, self.scroll_bottom + 1)),
                " q" => {
                    let n = match (self.cursor_shape, self.modes.cursor_blink) {
                        (CursorShape::Block, true) => 1,
                        (CursorShape::Block, false) => 2,
                        (CursorShape::Underline, true) => 3,
                        (CursorShape::Underline, false) => 4,
                        (CursorShape::Bar, true) => 5,
                        (CursorShape::Bar, false) => 6,
                    };
                    Some(format!("{n} q"))
                }
                "\"q" => Some("0\"q".into()),
                "\"p" => Some("62;1\"p".into()),
                _ => None,
            };
            let s = match answer {
                Some(a) => format!("\x1bP1$r{a}\x1b\\"),
                None => "\x1bP0$r\x1b\\".to_string(),
            };
            self.reply(&s);
        } else if d.intermediates == b"+" && d.action == 'q' {
            // XTGETTCAP: no terminfo capabilities are reported.
            self.reply("\x1bP0+r\x1b\\");
        }
    }

    /// The current SGR attributes as a parameter string (for DECRQSS).
    fn sgr_string(&self) -> String {
        let p = &self.cursor.pen;
        let mut v = vec!["0".to_string()];
        for (a, n) in [
            (attr::BOLD, "1"),
            (attr::DIM, "2"),
            (attr::ITALIC, "3"),
            (attr::BLINK, "5"),
            (attr::INVERSE, "7"),
            (attr::HIDDEN, "8"),
            (attr::STRIKE, "9"),
        ] {
            if p.has(a) {
                v.push(n.into());
            }
        }
        if p.underline != Underline::None {
            v.push("4".into());
        }
        let color = |c: Color, base: u16| match c {
            Color::Default => None,
            Color::Indexed(i) if i < 8 => Some(format!("{}", base + i as u16)),
            Color::Indexed(i) if i < 16 => Some(format!("{}", base + 60 + (i - 8) as u16)),
            Color::Indexed(i) => Some(format!("{};5;{}", base + 8, i)),
            Color::Rgb(r, g, b) => Some(format!("{};2;{};{};{}", base + 8, r, g, b)),
        };
        v.extend(color(p.fg, 30));
        v.extend(color(p.bg, 40));
        v.join(";")
    }
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// First value of each parameter, with `default` for missing/zero ones.
fn arg(params: &[u16], i: usize, default: u16) -> u16 {
    match params.get(i).copied() {
        Some(0) | None => default,
        Some(v) => v,
    }
}

impl vte::Perform for Term {
    #[inline]
    fn print(&mut self, c: char) {
        // Fast path: printable ASCII in the middle of a row with nothing
        // special around (the bulk of all terminal output).
        let x = self.cursor.x;
        if (c as u32).wrapping_sub(0x20) < 0x5f
            && !self.cursor.pending_wrap
            && self.single_shift.is_none()
            && !self.zwj
            && !self.modes.insert
            && self.charsets[self.gl] == Charset::Ascii
        {
            let y = self.cursor.y;
            let pen = self.cursor.pen;
            let link = self.current_link;
            let grid = if self.alt_active { &mut self.alt } else { &mut self.primary };
            let cols = grid.cols;
            if x + 1 < cols {
                let row = grid.row_mut(y);
                const LAYOUT: u16 = attr::WIDE | attr::SPACER | attr::EXTRA;
                if row.cells[x].attrs & LAYOUT == 0 && row.cells[x + 1].attrs & attr::SPACER == 0 {
                    row.cells[x] = Cell { ch: c, link, attrs: pen.attrs & attr::STYLE, ..pen };
                    self.cursor.x = x + 1;
                    self.last_char = Some(c);
                    self.changed = true;
                    return;
                }
            }
        }
        self.print_char(c);
    }

    fn execute(&mut self, byte: u8) {
        self.zwj = false;
        match byte {
            0x07 => self.events.push(Event::Bell),
            0x08 => {
                if self.cursor.pending_wrap {
                    self.cursor.pending_wrap = false;
                } else if self.cursor.x == 0 && self.modes.reverse_wrap && self.cursor.y > 0 {
                    self.cursor.y -= 1;
                    self.cursor.x = self.cols() - 1;
                } else {
                    self.move_left(1);
                }
            }
            0x09 => self.tab(1),
            0x0a..=0x0c => {
                self.linefeed();
                if self.modes.newline {
                    self.cursor.x = 0;
                }
            }
            0x0d => {
                self.cursor.x = 0;
                self.cursor.pending_wrap = false;
            }
            0x0e => self.gl = 1,
            0x0f => self.gl = 0,
            // C1 controls (8-bit).
            0x84 => self.linefeed(),
            0x85 => {
                self.cursor.x = 0;
                self.linefeed();
            }
            0x88 => {
                let x = self.cursor.x;
                self.tabs[x] = true;
            }
            0x8d => self.reverse_index(),
            _ => {}
        }
        self.changed = true;
    }

    fn hook(&mut self, _params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        self.dcs = (!ignore).then(|| Dcs { intermediates: intermediates.to_vec(), action, data: Vec::new() });
    }

    fn put(&mut self, byte: u8) {
        if let Some(d) = &mut self.dcs {
            if d.data.len() < 4096 {
                d.data.push(byte);
            }
        }
    }

    fn unhook(&mut self) {
        if let Some(d) = self.dcs.take() {
            self.dcs_finish(d);
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        osc::dispatch(self, params, bell_terminated);
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        self.zwj = false;
        if ignore {
            return;
        }
        // First value of each parameter, without allocating.
        let mut buf = [0u16; 32];
        let mut len = 0;
        for s in params.iter().take(32) {
            buf[len] = s[0];
            len += 1;
        }
        let p = &buf[..len];
        let private = intermediates.first() == Some(&b'?');
        let n = |i: usize| arg(p, i, 1) as usize;
        match (action, intermediates) {
            ('@', []) => self.insert_blanks(n(0)),
            ('A', []) => self.move_up(n(0)),
            ('B', []) | ('e', []) => self.move_down(n(0)),
            ('C', []) | ('a', []) => self.move_right(n(0)),
            ('D', []) => self.move_left(n(0)),
            ('E', []) => {
                self.move_down(n(0));
                self.cursor.x = 0;
            }
            ('F', []) => {
                self.move_up(n(0));
                self.cursor.x = 0;
            }
            ('G', []) | ('`', []) => {
                self.cursor.x = self.clamp_x(n(0) - 1);
                self.cursor.pending_wrap = false;
            }
            ('H', []) | ('f', []) => self.goto(n(1) - 1, n(0) - 1),
            ('I', []) => self.tab(n(0)),
            ('J', []) | ('J', [b'?']) => self.erase_display(arg(p, 0, 0)),
            ('K', []) | ('K', [b'?']) => self.erase_line(arg(p, 0, 0)),
            ('L', []) => self.insert_lines(n(0)),
            ('M', []) => self.delete_lines(n(0)),
            ('P', []) => self.delete_chars(n(0)),
            ('S', []) => self.scroll_up(n(0)),
            ('T', []) if p.len() <= 1 => self.scroll_down(n(0)),
            ('X', []) => self.erase_chars(n(0)),
            ('Z', []) => self.back_tab(n(0)),
            ('b', []) => self.repeat_last(n(0)),
            ('c', []) => {
                if arg(p, 0, 0) == 0 {
                    // VT220 with ANSI color.
                    self.reply("\x1b[?62;22c");
                }
            }
            ('c', [b'>']) => self.reply("\x1b[>1;10;0c"),
            ('d', []) => {
                let x = self.cursor.x;
                self.goto(x, n(0) - 1);
            }
            ('g', []) => match arg(p, 0, 0) {
                0 => {
                    let x = self.cursor.x;
                    self.tabs[x] = false;
                }
                3 => self.tabs.fill(false),
                _ => {}
            },
            ('h', _) | ('l', _) => {
                let on = action == 'h';
                for &m in p {
                    if private {
                        self.set_private_mode(m, on);
                    } else {
                        match m {
                            4 => self.modes.insert = on,
                            20 => self.modes.newline = on,
                            _ => {}
                        }
                    }
                }
            }
            ('m', []) => self.sgr(params),
            ('n', []) => match arg(p, 0, 0) {
                5 => self.reply("\x1b[0n"),
                6 => self.report_cursor(false),
                _ => {}
            },
            ('n', [b'?']) => {
                if arg(p, 0, 0) == 6 {
                    self.report_cursor(true);
                }
            }
            ('p', [b'!']) => self.soft_reset(),
            ('p', [b'?', b'$']) => {
                let m = arg(p, 0, 0);
                let v = self.private_mode_state(m);
                self.reply(&format!("\x1b[?{m};{v}$y"));
            }
            ('p', [b'$']) => {
                let m = arg(p, 0, 0);
                let v = match m {
                    4 => if self.modes.insert { 1 } else { 2 },
                    20 => if self.modes.newline { 1 } else { 2 },
                    _ => 0,
                };
                self.reply(&format!("\x1b[{m};{v}$y"));
            }
            ('q', [b' ']) => {
                let (shape, blink) = match arg(p, 0, 0) {
                    0 | 1 => (CursorShape::Block, true),
                    2 => (CursorShape::Block, false),
                    3 => (CursorShape::Underline, true),
                    4 => (CursorShape::Underline, false),
                    5 => (CursorShape::Bar, true),
                    _ => (CursorShape::Bar, false),
                };
                self.cursor_shape = shape;
                self.modes.cursor_blink = blink;
            }
            ('q', [b'>']) => self.reply(&format!("\x1bP>|lumen-term {}\x1b\\", env!("CARGO_PKG_VERSION"))),
            ('r', []) => self.set_scroll_region(arg(p, 0, 1) as usize, arg(p, 1, 0) as usize),
            ('s', []) => self.save_cursor(),
            ('t', []) => match arg(p, 0, 0) {
                14 => {
                    let (w, h) = (self.cell_px.0 as usize * self.cols(), self.cell_px.1 as usize * self.rows());
                    self.reply(&format!("\x1b[4;{h};{w}t"));
                }
                16 => {
                    let (w, h) = self.cell_px;
                    self.reply(&format!("\x1b[6;{h};{w}t"));
                }
                18 => {
                    let s = format!("\x1b[8;{};{}t", self.rows(), self.cols());
                    self.reply(&s);
                }
                22 => {
                    if self.title_stack.len() < 32 {
                        self.title_stack.push(self.title.clone());
                    }
                }
                23 => {
                    if let Some(t) = self.title_stack.pop() {
                        self.title = t.clone();
                        self.events.push(Event::Title(t));
                    }
                }
                _ => {}
            },
            ('u', []) => self.restore_cursor(),
            ('x', []) => {
                // DECREQTPARM: no parity, 8 bits, 38400 baud, clock multiplier 1.
                let kind = if arg(p, 0, 0) == 1 { 3 } else { 2 };
                self.reply(&format!("\x1b[{kind};1;1;112;112;1;0x"));
            }
            _ => {}
        }
        self.changed = true;
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore {
            return;
        }
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            ([], b'D') => self.linefeed(),
            ([], b'E') => {
                self.cursor.x = 0;
                self.linefeed();
            }
            ([], b'H') => {
                let x = self.cursor.x;
                self.tabs[x] = true;
            }
            ([], b'M') => self.reverse_index(),
            ([], b'Z') => self.reply("\x1b[?62;22c"),
            ([], b'c') => self.reset(),
            ([], b'=') => self.modes.app_keypad = true,
            ([], b'>') => self.modes.app_keypad = false,
            ([], b'N') => self.single_shift = Some(2),
            ([], b'O') => self.single_shift = Some(3),
            ([], b'n') => self.gl = 2,
            ([], b'o') => self.gl = 3,
            ([b'#'], b'8') => self.decaln(),
            ([i @ (b'(' | b')' | b'*' | b'+')], c) => {
                let g = (*i - b'(') as usize;
                self.charsets[g] = match c {
                    b'0' => Charset::DecSpecial,
                    b'A' => Charset::Uk,
                    _ => Charset::Ascii,
                };
            }
            _ => {}
        }
        self.changed = true;
    }
}
