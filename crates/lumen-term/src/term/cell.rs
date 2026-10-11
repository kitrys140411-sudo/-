//! Screen cells and their attributes.

/// A color as the application specified it; resolved against the palette
/// only when drawing (so palette changes apply to text already on screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Underline styles (SGR 4:n, 21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// Attribute bits of a cell.
pub mod attr {
    pub const BOLD: u16 = 1 << 0;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const BLINK: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const HIDDEN: u16 = 1 << 5;
    pub const STRIKE: u16 = 1 << 6;
    pub const OVERLINE: u16 = 1 << 7;
    /// The cell holds a double-width character (the next cell is its spacer).
    pub const WIDE: u16 = 1 << 8;
    /// Right half of a double-width character.
    pub const SPACER: u16 = 1 << 9;
    /// The cell's grapheme continues in the row's extras (combining marks,
    /// joiners, variation selectors).
    pub const EXTRA: u16 = 1 << 10;
    /// Attributes that SGR sets (everything but the layout bits).
    pub const STYLE: u16 = BOLD | DIM | ITALIC | BLINK | INVERSE | HIDDEN | STRIKE | OVERLINE;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    /// Underline color (SGR 58); Default follows the foreground.
    pub ul_color: Color,
    pub attrs: u16,
    pub underline: Underline,
    /// Hyperlink (OSC 8) id, 0 = none.
    pub link: u16,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, ul_color: Color::Default, attrs: 0, underline: Underline::None, link: 0 }
    }
}

impl Cell {
    #[inline]
    pub fn has(&self, a: u16) -> bool {
        self.attrs & a != 0
    }

    /// An empty cell carrying only the background of `pen` (erase with BCE).
    #[inline]
    pub fn blank(pen: &Cell) -> Cell {
        Cell { bg: pen.bg, ..Cell::default() }
    }

    /// Nothing visible: a space without background, decorations or inverse.
    pub fn is_empty(&self) -> bool {
        self.ch == ' '
            && self.bg == Color::Default
            && self.attrs & (attr::INVERSE | attr::EXTRA | attr::STRIKE | attr::OVERLINE) == 0
            && self.underline == Underline::None
    }
}
