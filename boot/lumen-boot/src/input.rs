//! Keyboard input normalized into the few keys the menu understands.

use uefi::proto::console::text::{Key, ScanCode};
use uefi::system;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
    Escape,
    Backspace,
    Delete,
    Char(char),
}

/// Read one pending key, if any (non-blocking).
pub fn poll() -> Option<UiKey> {
    let key = system::with_stdin(|stdin| stdin.read_key()).ok()??;
    Some(match key {
        Key::Printable(c) => match char::from(c) {
            '\r' | '\n' => UiKey::Enter,
            '\u{8}' => UiKey::Backspace,
            '\u{1b}' => UiKey::Escape,
            c => UiKey::Char(c),
        },
        Key::Special(s) => match s {
            ScanCode::UP => UiKey::Up,
            ScanCode::DOWN => UiKey::Down,
            ScanCode::LEFT => UiKey::Left,
            ScanCode::RIGHT => UiKey::Right,
            ScanCode::HOME | ScanCode::PAGE_UP => UiKey::Home,
            ScanCode::END | ScanCode::PAGE_DOWN => UiKey::End,
            ScanCode::ESCAPE => UiKey::Escape,
            ScanCode::DELETE => UiKey::Delete,
            // OVMF's virtio keyboard driver reports printable keys as a scan code
            // equal to their ASCII value with no Unicode character.
            ScanCode(code @ 0x20..=0x7e) => UiKey::Char(code as u8 as char),
            ScanCode(code) => {
                crate::log!("unhandled scan code {code:#x}");
                return None;
            }
        },
    })
}

/// Event that is signaled when a key is available.
pub fn key_event() -> Option<uefi::Event> {
    system::with_stdin(|stdin| stdin.wait_for_key_event().ok())
}
