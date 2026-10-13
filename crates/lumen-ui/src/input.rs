//! Input as the toolkit sees it: keys, modifiers, pointer and scroll events,
//! independent of Wayland (the window runtime translates; tests can feed
//! events directly).

use crate::geom::Vec2;

/// Modifier keys held during an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The Super ("Windows") key.
    pub logo: bool,
}

impl Mods {
    pub const NONE: Mods = Mods { ctrl: false, shift: false, alt: false, logo: false };
    pub const CTRL: Mods = Mods { ctrl: true, shift: false, alt: false, logo: false };
    pub const SHIFT: Mods = Mods { ctrl: false, shift: true, alt: false, logo: false };
    pub const ALT: Mods = Mods { ctrl: false, shift: false, alt: true, logo: false };
    pub const CTRL_SHIFT: Mods = Mods { ctrl: true, shift: true, alt: false, logo: false };

    pub fn any(&self) -> bool {
        self.ctrl || self.shift || self.alt || self.logo
    }

    /// Ctrl, Alt or Super: the key is a command, not text.
    pub fn command(&self) -> bool {
        self.ctrl || self.alt || self.logo
    }
}

/// A key, named or by the character it types without Shift (letters are
/// lower case: Ctrl+Shift+S is `Char('s')` with ctrl and shift).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F(u8),
    /// The context-menu key.
    Menu,
    /// Anything else, by X keysym.
    Other(u32),
}

impl Key {
    pub const SPACE: Key = Key::Char(' ');

    /// Display name for shortcut labels ("Ctrl+S", "Delete").
    pub fn name(&self) -> String {
        match self {
            Key::Char(' ') => "Space".into(),
            Key::Char(c) => c.to_uppercase().collect(),
            Key::Enter => "Enter".into(),
            Key::Tab => "Tab".into(),
            Key::Backspace => "Backspace".into(),
            Key::Delete => "Delete".into(),
            Key::Escape => "Esc".into(),
            Key::Left => "Left".into(),
            Key::Right => "Right".into(),
            Key::Up => "Up".into(),
            Key::Down => "Down".into(),
            Key::Home => "Home".into(),
            Key::End => "End".into(),
            Key::PageUp => "Page Up".into(),
            Key::PageDown => "Page Down".into(),
            Key::Insert => "Insert".into(),
            Key::F(n) => format!("F{n}"),
            Key::Menu => "Menu".into(),
            Key::Other(k) => format!("{k:#x}"),
        }
    }
}

/// A key press (repeats included) with what it types.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyPress {
    pub key: Key,
    pub mods: Mods,
    /// Text typed by the key (None for command keys and control characters).
    pub text: Option<String>,
    pub repeat: bool,
}

/// A key combination for app shortcuts, e.g. `Shortcut::ctrl('s')`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub mods: Mods,
    pub key: Key,
}

impl Shortcut {
    pub const fn new(mods: Mods, key: Key) -> Shortcut {
        Shortcut { mods, key }
    }

    /// Ctrl + a character key.
    pub const fn ctrl(c: char) -> Shortcut {
        Shortcut { mods: Mods::CTRL, key: Key::Char(c) }
    }

    /// Ctrl + Shift + a character key.
    pub const fn ctrl_shift(c: char) -> Shortcut {
        Shortcut { mods: Mods::CTRL_SHIFT, key: Key::Char(c) }
    }

    /// Alt + a key.
    pub const fn alt(key: Key) -> Shortcut {
        Shortcut { mods: Mods::ALT, key }
    }

    /// A key without modifiers.
    pub const fn key(key: Key) -> Shortcut {
        Shortcut { mods: Mods::NONE, key }
    }

    pub fn matches(&self, k: &KeyPress) -> bool {
        self.key == k.key && self.mods == k.mods
    }

    /// "Ctrl+Shift+S".
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (on, name) in [(self.mods.ctrl, "Ctrl"), (self.mods.alt, "Alt"), (self.mods.logo, "Super"), (self.mods.shift, "Shift")] {
            if on {
                s.push_str(name);
                s.push('+');
            }
        }
        s.push_str(&self.key.name());
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// One input event, in window-logical coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    PointerMove(Vec2),
    /// The pointer left the window.
    PointerLeave,
    PointerButton { pos: Vec2, button: Button, pressed: bool },
    /// Scrolling by `delta` logical pixels (positive y scrolls content up,
    /// i.e. moves the view down). `kinetic_end` marks the end of a touchpad
    /// gesture (fingers lifted), where kinetic scrolling takes over.
    Scroll { pos: Vec2, delta: Vec2, wheel: bool, kinetic_end: bool },
    Key(KeyPress),
    /// Text from the clipboard (or primary selection), for the widget that
    /// asked for it.
    Paste { target: crate::Id, text: String },
    /// The window gained or lost keyboard focus.
    Focus(bool),
}

impl Event {
    /// Events after which the UI may look different, so later events of the
    /// same batch are handled by a fresh build (a click must hit what the
    /// previous click revealed).
    pub fn is_barrier(&self) -> bool {
        match self {
            Event::PointerButton { pressed: false, .. } => true,
            Event::Key(k) => matches!(k.key, Key::Enter | Key::Escape | Key::Tab) || k.mods.command(),
            Event::Paste { .. } | Event::Focus(_) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_labels_and_matching() {
        let s = Shortcut::ctrl_shift('s');
        assert_eq!(s.label(), "Ctrl+Shift+S");
        let k = KeyPress { key: Key::Char('s'), mods: Mods::CTRL_SHIFT, text: None, repeat: false };
        assert!(s.matches(&k));
        assert!(!Shortcut::ctrl('s').matches(&k));
        assert_eq!(Shortcut::key(Key::Delete).label(), "Delete");
    }
}
