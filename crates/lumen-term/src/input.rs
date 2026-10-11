//! Keyboard and mouse input encoding (xterm conventions).

use crate::term::{Modes, MouseEncoding, MouseMode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub logo: bool,
}

impl Mods {
    /// xterm modifier parameter (1 = none).
    pub fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8 + 8 * self.logo as u8
    }

    pub fn any(self) -> bool {
        self.shift || self.alt || self.ctrl || self.logo
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    /// Text produced by the keyboard layout (already includes Shift and
    /// Ctrl effects, e.g. "\x01" for Ctrl+A).
    Text(String),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    KpEnter,
}

/// Bytes to send to the application for a key press, or None if the key
/// produces nothing.
pub fn encode_key(key: &Key, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    let m = mods.param();
    let csi = |s: &str| Some(format!("\x1b[{s}").into_bytes());
    // Cursor keys: SS3 in application mode, CSI otherwise; CSI 1;m with mods.
    let cursor = |c: char| {
        if m > 1 {
            csi(&format!("1;{m}{c}"))
        } else if modes.app_cursor {
            Some(format!("\x1bO{c}").into_bytes())
        } else {
            csi(&c.to_string())
        }
    };
    let tilde = |n: u8| if m > 1 { csi(&format!("{n};{m}~")) } else { csi(&format!("{n}~")) };
    let alt = |mut v: Vec<u8>| {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        Some(v)
    };
    match key {
        Key::Text(t) => {
            if t.is_empty() {
                return None;
            }
            let mut bytes = t.as_bytes().to_vec();
            // Ctrl+Space and Ctrl+@ send NUL; layouts give " " for them.
            if mods.ctrl && t == " " {
                bytes = vec![0];
            }
            alt(bytes)
        }
        Key::Enter => {
            let v = if modes.newline { b"\r\n".to_vec() } else { b"\r".to_vec() };
            alt(v)
        }
        Key::KpEnter => {
            if modes.app_keypad {
                Some(b"\x1bOM".to_vec())
            } else {
                alt(b"\r".to_vec())
            }
        }
        Key::Tab => {
            if mods.shift {
                csi("Z")
            } else {
                alt(b"\t".to_vec())
            }
        }
        Key::Backspace => {
            let v = if mods.ctrl { vec![0x08] } else { vec![0x7f] };
            alt(v)
        }
        Key::Escape => alt(vec![0x1b]),
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n) => match n {
            1..=4 => {
                let c = [b'P', b'Q', b'R', b'S'][(*n - 1) as usize] as char;
                if m > 1 { csi(&format!("1;{m}{c}")) } else { Some(format!("\x1bO{c}").into_bytes()) }
            }
            5 => tilde(15),
            6 => tilde(17),
            7 => tilde(18),
            8 => tilde(19),
            9 => tilde(20),
            10 => tilde(21),
            11 => tilde(23),
            12 => tilde(24),
            _ => None,
        },
    }
}

/// Text pasted into the terminal: bracketed if the application asked, and
/// with escape characters removed so pasted text cannot end the bracket or
/// inject control sequences.
pub fn encode_paste(text: &str, modes: &Modes) -> Vec<u8> {
    let clean: String = text.chars().filter(|&c| c != '\x1b' && c != '\u{9b}').collect();
    // Line endings as the Enter key sends them.
    let clean = clean.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(clean.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        clean.into_bytes()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    /// Motion with no button held.
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Encode a mouse event at cell (`col`, `row`) (0-based), or None if the
/// current mouse mode does not report it.
pub fn encode_mouse(button: MouseButton, action: MouseAction, held: bool, col: usize, row: usize, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    let report = match modes.mouse {
        MouseMode::None => false,
        MouseMode::Press => action == MouseAction::Press,
        MouseMode::Normal => action != MouseAction::Motion,
        MouseMode::Button => action != MouseAction::Motion || held,
        MouseMode::Any => true,
    };
    if !report {
        return None;
    }
    let wheel = matches!(button, MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::WheelLeft | MouseButton::WheelRight);
    if wheel && action == MouseAction::Release {
        return None;
    }
    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
        MouseButton::WheelLeft => 66,
        MouseButton::WheelRight => 67,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    if modes.mouse != MouseMode::Press {
        code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;
    }
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    match modes.mouse_encoding {
        MouseEncoding::Sgr => {
            let c = if action == MouseAction::Release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{c}").into_bytes())
        }
        MouseEncoding::Urxvt => {
            let code = if action == MouseAction::Release && !wheel { 3 + (code & !3) } else { code };
            Some(format!("\x1b[{};{x};{y}M", code + 32).into_bytes())
        }
        MouseEncoding::X10 | MouseEncoding::Utf8 => {
            // Releases are button 3 in the old encodings.
            let code = if action == MouseAction::Release { 3 + (code & !3) } else { code };
            let mut v = b"\x1b[M".to_vec();
            v.push((32 + code) as u8);
            let mut coord = |n: u32| {
                if modes.mouse_encoding == MouseEncoding::Utf8 {
                    let c = char::from_u32(32 + n).unwrap_or(' ');
                    let mut b = [0u8; 4];
                    v.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                    true
                } else if n <= 223 {
                    v.push((32 + n) as u8);
                    true
                } else {
                    false
                }
            };
            if !coord(x) || !coord(y) {
                return None;
            }
            Some(v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> Modes {
        Modes::default()
    }

    #[test]
    fn keys() {
        let m = modes();
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..Default::default() };
        let alt = Mods { alt: true, ..Default::default() };
        let shift = Mods { shift: true, ..Default::default() };
        assert_eq!(encode_key(&Key::Up, none, &m).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(&Key::Up, ctrl, &m).unwrap(), b"\x1b[1;5A");
        let app = Modes { app_cursor: true, ..modes() };
        assert_eq!(encode_key(&Key::Up, none, &app).unwrap(), b"\x1bOA");
        assert_eq!(encode_key(&Key::F(1), none, &m).unwrap(), b"\x1bOP");
        assert_eq!(encode_key(&Key::F(5), shift, &m).unwrap(), b"\x1b[15;2~");
        assert_eq!(encode_key(&Key::Delete, none, &m).unwrap(), b"\x1b[3~");
        assert_eq!(encode_key(&Key::Text("x".into()), alt, &m).unwrap(), b"\x1bx");
        assert_eq!(encode_key(&Key::Text(" ".into()), ctrl, &m).unwrap(), b"\0");
        assert_eq!(encode_key(&Key::Backspace, none, &m).unwrap(), b"\x7f");
        assert_eq!(encode_key(&Key::Tab, shift, &m).unwrap(), b"\x1b[Z");
    }

    #[test]
    fn paste_is_sanitized() {
        let m = Modes { bracketed_paste: true, ..modes() };
        assert_eq!(encode_paste("a\x1b[201~b\nc", &m), b"\x1b[200~a[201~b\rc\x1b[201~");
        assert_eq!(encode_paste("x\r\ny", &modes()), b"x\ry");
    }

    #[test]
    fn mouse() {
        let none = Mods::default();
        let sgr = Modes { mouse: MouseMode::Normal, mouse_encoding: MouseEncoding::Sgr, ..modes() };
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, true, 4, 9, none, &sgr).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Release, false, 4, 9, none, &sgr).unwrap(), b"\x1b[<0;5;10m");
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Motion, true, 4, 9, none, &sgr), None);
        let btn = Modes { mouse: MouseMode::Button, ..sgr };
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Motion, true, 4, 9, none, &btn).unwrap(), b"\x1b[<32;5;10M");
        assert_eq!(encode_mouse(MouseButton::WheelUp, MouseAction::Press, false, 0, 0, none, &sgr).unwrap(), b"\x1b[<64;1;1M");
        let x10 = Modes { mouse: MouseMode::Normal, ..modes() };
        assert_eq!(encode_mouse(MouseButton::Right, MouseAction::Press, true, 0, 0, none, &x10).unwrap(), b"\x1b[M\x22\x21\x21");
        assert_eq!(encode_mouse(MouseButton::Right, MouseAction::Release, false, 0, 0, none, &x10).unwrap(), b"\x1b[M\x23\x21\x21");
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, true, 300, 0, none, &x10), None);
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, true, 0, 0, none, &modes()), None);
    }
}
