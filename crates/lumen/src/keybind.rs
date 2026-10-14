//! Keyboard shortcuts: one table of chord → action bindings that drives the
//! compositor's key handling, the shortcut cheat sheet and the IPC listing.
//!
//! The defaults are `rootfs/usr/share/lumen/keybindings.toml`, compiled in
//! (an installed copy in /usr/share/lumen takes precedence, so the system
//! file can be edited). A user file (`$LUMEN_KEYBINDINGS`, else
//! `~/.config/lumen/keybindings.toml`) in the same format is laid over them:
//! an entry binds its chords to its action, replacing whatever they were
//! bound to, and `action = "none"` unbinds them. A file with any error is
//! rejected as a whole, so a typo never leaves the desktop half configured.
//!
//! Chords are matched on keysyms, not keycodes: letters case-insensitively
//! and through every configured layout, first layout first (Super+T and
//! Super+/ work with a Russian layout active, where those keys type "е" and
//! "."), and a symbol typed with Shift matches a binding written with the
//! symbol itself (Super+? is Super+Shift+/ on a US keyboard).
//!
//! A chord of modifiers only ("Alt+Shift") fires when the modifiers are
//! released again without any other key or button in between, so it never
//! gets in the way of chords that start with the same modifiers
//! (Alt+Shift+Tab).

use serde::Deserialize;
use smithay::input::keyboard::{keysyms, xkb, Keysym, ModifiersState};
use toml::Spanned;

use crate::backlight::BrightnessCmd;
use crate::input::Action;
use crate::volume::VolumeCmd;
use crate::wm::{Tile, WORKSPACES};

/// The shipped defaults (also installed as /usr/share/lumen/keybindings.toml).
pub const DEFAULTS: &str = include_str!("../../../rootfs/usr/share/lumen/keybindings.toml");
const SYSTEM_FILE: &str = "/usr/share/lumen/keybindings.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub logo: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl From<&ModifiersState> for Mods {
    fn from(m: &ModifiersState) -> Mods {
        Mods { logo: m.logo, ctrl: m.ctrl, alt: m.alt, shift: m.shift }
    }
}

impl Mods {
    fn count(&self) -> usize {
        [self.logo, self.ctrl, self.alt, self.shift].iter().filter(|m| **m).count()
    }

    /// Names in canonical order (Super, Ctrl, Alt, Shift).
    fn names(&self) -> impl Iterator<Item = &'static str> {
        [(self.logo, "Super"), (self.ctrl, "Ctrl"), (self.alt, "Alt"), (self.shift, "Shift")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n)
    }
}

/// Parse a modifier name into `mods`; false if `name` is not a modifier.
fn set_modifier(mods: &mut Mods, name: &str) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "super" | "logo" | "win" | "mod4" | "meta" => mods.logo = true,
        "ctrl" | "control" => mods.ctrl = true,
        "alt" | "mod1" => mods.alt = true,
        "shift" => mods.shift = true,
        _ => return false,
    }
    true
}

/// Keysyms of the modifier keys themselves (a modifier-only chord is armed
/// by pressing these and fires when one is released).
pub fn is_modifier_key(sym: Keysym) -> bool {
    matches!(
        sym.raw(),
        keysyms::KEY_Shift_L
            | keysyms::KEY_Shift_R
            | keysyms::KEY_Control_L
            | keysyms::KEY_Control_R
            | keysyms::KEY_Alt_L
            | keysyms::KEY_Alt_R
            | keysyms::KEY_Meta_L
            | keysyms::KEY_Meta_R
            | keysyms::KEY_Super_L
            | keysyms::KEY_Super_R
            | keysyms::KEY_Hyper_L
            | keysyms::KEY_Hyper_R
            | keysyms::KEY_ISO_Level3_Shift
    )
}

/// A key with modifiers. `key` is normalized to lower case; `NoSymbol` makes
/// it a modifier-only chord ("Alt+Shift").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: Mods,
    pub key: Keysym,
}

/// Lower-case letter keysyms, so "Super+T" and "Super+t" are the same chord.
fn lower(sym: Keysym) -> Keysym {
    match sym.key_char() {
        Some(c) if c.is_uppercase() => match c.to_lowercase().next().map(Keysym::from_char) {
            Some(k) if k != Keysym::NoSymbol => k,
            _ => sym,
        },
        _ => sym,
    }
}

/// A printable character for the key, if it has one (letters, digits, punctuation).
fn printable(sym: Keysym) -> Option<char> {
    sym.key_char().filter(|c| !c.is_control() && !c.is_whitespace())
}

/// Key names people write that are not xkb keysym names.
fn alias(name: &str) -> Option<Keysym> {
    let raw = match name.to_ascii_lowercase().as_str() {
        "enter" => keysyms::KEY_Return,
        "esc" => keysyms::KEY_Escape,
        "del" => keysyms::KEY_Delete,
        "ins" => keysyms::KEY_Insert,
        "pageup" | "pgup" => keysyms::KEY_Page_Up,
        "pagedown" | "pgdn" | "pgdown" => keysyms::KEY_Page_Down,
        "printscreen" | "prtsc" => keysyms::KEY_Print,
        "plus" => keysyms::KEY_plus,
        "minus" => keysyms::KEY_minus,
        "volumeup" => keysyms::KEY_XF86AudioRaiseVolume,
        "volumedown" => keysyms::KEY_XF86AudioLowerVolume,
        "mute" => keysyms::KEY_XF86AudioMute,
        "calculator" => keysyms::KEY_XF86Calculator,
        "brightnessup" => keysyms::KEY_XF86MonBrightnessUp,
        "brightnessdown" => keysyms::KEY_XF86MonBrightnessDown,
        "sleep" => keysyms::KEY_XF86Sleep,
        _ => return None,
    };
    Some(Keysym::new(raw))
}

fn key_from_name(name: &str) -> Option<Keysym> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let k = Keysym::from_char(c);
        return (k != Keysym::NoSymbol).then_some(k);
    }
    if let Some(k) = alias(name) {
        return Some(k);
    }
    // Exact names first: some differ only by case (e.g. "Ccedilla").
    [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE]
        .into_iter()
        .map(|flags| xkb::keysym_from_name(name, flags))
        .find(|k| *k != Keysym::NoSymbol)
}

impl Chord {
    /// Parse "Super+Shift+Page_Up", "Ctrl+Alt+T", "Super+/", "Print", "Alt+Shift".
    pub fn parse(spec: &str) -> Result<Chord, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty key chord".into());
        }
        // "Super++" binds the plus key: the last '+' is a separator only if
        // something follows it.
        let (mods_part, key_name) = match spec.strip_suffix("++") {
            Some(m) => (m, "+"),
            None => match spec.rfind('+') {
                Some(i) if i + 1 < spec.len() => (&spec[..i], &spec[i + 1..]),
                _ => ("", spec),
            },
        };
        let mut mods = Mods::default();
        for m in mods_part.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            if !set_modifier(&mut mods, m) {
                return Err(format!("unknown modifier '{m}' in '{spec}' (use Super, Ctrl, Alt, Shift)"));
            }
        }
        let key_name = key_name.trim();
        if key_name != "+" && set_modifier(&mut mods, key_name) {
            // Modifiers only: a lone modifier would fire on every tap of it
            // (and a lone Super is the overview gesture).
            if mods.count() < 2 {
                return Err(format!("'{spec}' is a single modifier; a chord of modifiers needs two or more (e.g. Alt+Shift)"));
            }
            return Ok(Chord { mods, key: Keysym::NoSymbol });
        }
        let key = key_from_name(key_name).ok_or_else(|| format!("unknown key '{key_name}' in '{spec}'"))?;
        Ok(Chord { mods, key: lower(key) })
    }

    /// A chord of modifiers only (fires on their release).
    pub fn is_modifier_only(&self) -> bool {
        self.key == Keysym::NoSymbol
    }

    /// Canonical text form, e.g. "Super+Shift+Page_Up", "Super+/" or "Alt+Shift".
    pub fn spec(&self) -> String {
        let mut s: Vec<String> = self.mods.names().map(String::from).collect();
        if !self.is_modifier_only() {
            s.push(match (printable(self.key), self.key.raw()) {
                (Some(c), _) => c.to_uppercase().collect(),
                // xkb names these by their older aliases (Prior, Next).
                (None, keysyms::KEY_Page_Up) => "Page_Up".into(),
                (None, keysyms::KEY_Page_Down) => "Page_Down".into(),
                (None, _) => xkb::keysym_get_name(self.key),
            });
        }
        s.join("+")
    }

    /// What to print on the key caps of this chord, modifiers first.
    pub fn caps(&self) -> Vec<String> {
        let mut caps: Vec<String> = self.mods.names().map(String::from).collect();
        if !self.is_modifier_only() {
            caps.push(key_label(self.key));
        }
        caps
    }
}

/// A short human label for a key cap.
pub fn key_label(sym: Keysym) -> String {
    let named = match sym.raw() {
        keysyms::KEY_Return | keysyms::KEY_KP_Enter => "Enter",
        keysyms::KEY_Escape => "Esc",
        keysyms::KEY_BackSpace => "Backspace",
        keysyms::KEY_Delete => "Delete",
        keysyms::KEY_Insert => "Insert",
        keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab => "Tab",
        keysyms::KEY_space => "Space",
        keysyms::KEY_Page_Up => "Page Up",
        keysyms::KEY_Page_Down => "Page Down",
        keysyms::KEY_Home => "Home",
        keysyms::KEY_End => "End",
        keysyms::KEY_Left => "←",
        keysyms::KEY_Right => "→",
        keysyms::KEY_Up => "↑",
        keysyms::KEY_Down => "↓",
        keysyms::KEY_Print => "Print",
        keysyms::KEY_XF86AudioRaiseVolume => "Volume Up",
        keysyms::KEY_XF86AudioLowerVolume => "Volume Down",
        keysyms::KEY_XF86AudioMute => "Mute",
        keysyms::KEY_XF86AudioMicMute => "Mic Mute",
        keysyms::KEY_XF86Calculator => "Calculator",
        keysyms::KEY_XF86MonBrightnessUp => "Brightness Up",
        keysyms::KEY_XF86MonBrightnessDown => "Brightness Down",
        keysyms::KEY_XF86Sleep => "Sleep",
        _ => "",
    };
    if !named.is_empty() {
        return named.into();
    }
    if let Some(c) = printable(sym) {
        return c.to_uppercase().collect();
    }
    let name = xkb::keysym_get_name(sym);
    name.strip_prefix("XF86").unwrap_or(&name).replace('_', " ")
}

/// One chord bound to an action, with how the cheat sheet shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub chord: Chord,
    pub action: Action,
    pub label: String,
    pub group: String,
}

/// Parse an action name (and the command of "spawn"). `Ok(None)` is "none":
/// the chords are unbound.
pub fn parse_action(name: &str, command: Option<Vec<String>>) -> Result<Option<Action>, String> {
    let ws = |rest: &str| -> Result<usize, String> {
        match rest.parse::<usize>() {
            Ok(n) if (1..=WORKSPACES).contains(&n) => Ok(n - 1),
            _ => Err(format!("no workspace '{rest}' in '{name}' (there are {WORKSPACES})")),
        }
    };
    if name != "spawn" && command.is_some() {
        return Err(format!("'command' only goes with action = \"spawn\", not '{name}'"));
    }
    let action = match name {
        "none" => return Ok(None),
        "spawn" => match command {
            Some(c) if !c.is_empty() => Action::Spawn(c),
            _ => return Err("action \"spawn\" needs a non-empty 'command'".into()),
        },
        "overview" => Action::ToggleOverview,
        "app-grid" => Action::ShowApps,
        "shortcuts" => Action::ToggleShortcuts,
        "close-window" => Action::CloseWindow,
        "maximize" => Action::Maximize,
        "restore" => Action::Restore,
        "tile-left" => Action::Tile(Tile::Left),
        "tile-right" => Action::Tile(Tile::Right),
        "fullscreen" => Action::ToggleFullscreen,
        "switcher" => Action::SwitcherNext { reverse: false },
        "switcher-reverse" => Action::SwitcherNext { reverse: true },
        "workspace-left" => Action::WorkspaceRelative(-1),
        "workspace-right" => Action::WorkspaceRelative(1),
        "move-to-workspace-left" => Action::MoveToWorkspaceRelative(-1),
        "move-to-workspace-right" => Action::MoveToWorkspaceRelative(1),
        "screenshot" => Action::Screenshot,
        "volume-up" => Action::Volume(VolumeCmd::Up),
        "volume-down" => Action::Volume(VolumeCmd::Down),
        "volume-mute" => Action::Volume(VolumeCmd::ToggleMute),
        "brightness-up" => Action::Brightness(BrightnessCmd::Up),
        "brightness-down" => Action::Brightness(BrightnessCmd::Down),
        "switch-layout" => Action::SwitchLayout { backward: false },
        "switch-layout-backward" => Action::SwitchLayout { backward: true },
        "suspend" => Action::Suspend,
        "hud" => Action::ToggleHud,
        "quit-session" => Action::Quit,
        _ => {
            if let Some(n) = name.strip_prefix("move-to-workspace-") {
                Action::MoveToWorkspace(ws(n)?)
            } else if let Some(n) = name.strip_prefix("workspace-") {
                Action::Workspace(ws(n)?)
            } else {
                return Err(format!("unknown action '{name}'"));
            }
        }
    };
    Ok(Some(action))
}

/// The configuration name of an action (inverse of [`parse_action`]).
pub fn action_name(a: &Action) -> String {
    match a {
        Action::Spawn(_) => "spawn".into(),
        Action::ToggleOverview => "overview".into(),
        Action::ShowApps => "app-grid".into(),
        Action::ToggleShortcuts => "shortcuts".into(),
        Action::CloseWindow => "close-window".into(),
        Action::Maximize => "maximize".into(),
        Action::Restore => "restore".into(),
        Action::Tile(Tile::Left) => "tile-left".into(),
        Action::Tile(Tile::Right) => "tile-right".into(),
        Action::ToggleFullscreen => "fullscreen".into(),
        Action::SwitcherNext { reverse: false } => "switcher".into(),
        Action::SwitcherNext { reverse: true } => "switcher-reverse".into(),
        Action::Workspace(n) => format!("workspace-{}", n + 1),
        Action::MoveToWorkspace(n) => format!("move-to-workspace-{}", n + 1),
        Action::WorkspaceRelative(d) => if *d < 0 { "workspace-left" } else { "workspace-right" }.into(),
        Action::MoveToWorkspaceRelative(d) => if *d < 0 { "move-to-workspace-left" } else { "move-to-workspace-right" }.into(),
        Action::Screenshot => "screenshot".into(),
        Action::Volume(VolumeCmd::Up) => "volume-up".into(),
        Action::Volume(VolumeCmd::Down) => "volume-down".into(),
        Action::Volume(VolumeCmd::ToggleMute) => "volume-mute".into(),
        Action::Brightness(BrightnessCmd::Up) => "brightness-up".into(),
        Action::Brightness(BrightnessCmd::Down) => "brightness-down".into(),
        Action::SwitchLayout { backward: false } => "switch-layout".into(),
        Action::SwitchLayout { backward: true } => "switch-layout-backward".into(),
        Action::Suspend => "suspend".into(),
        Action::ToggleHud => "hud".into(),
        Action::Quit => "quit-session".into(),
        Action::None | Action::VtSwitch(_) | Action::ShellKey { .. } => "none".into(),
    }
}

/// Label and group for a binding whose entry does not name them.
fn default_label(a: &Action) -> (String, &'static str) {
    let (label, group) = match a {
        Action::Spawn(cmd) => {
            let prog = cmd.first().map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or_default();
            return (format!("Run {prog}"), "Apps");
        }
        Action::ToggleOverview => ("Activities overview", "System"),
        Action::ShowApps => ("Show all apps", "System"),
        Action::ToggleShortcuts => ("Keyboard shortcuts", "System"),
        Action::CloseWindow => ("Close window", "Windows"),
        Action::Maximize => ("Maximize", "Windows"),
        Action::Restore => ("Restore size", "Windows"),
        Action::Tile(Tile::Left) => ("Tile left", "Windows"),
        Action::Tile(Tile::Right) => ("Tile right", "Windows"),
        Action::ToggleFullscreen => ("Fullscreen", "Windows"),
        Action::SwitcherNext { reverse: false } => ("Switch windows", "Windows"),
        Action::SwitcherNext { reverse: true } => ("Switch windows backwards", "Windows"),
        Action::Workspace(_) => ("Go to workspace", "Workspaces"),
        Action::MoveToWorkspace(_) => ("Move window to workspace", "Workspaces"),
        Action::WorkspaceRelative(d) => (if *d < 0 { "Previous workspace" } else { "Next workspace" }, "Workspaces"),
        Action::MoveToWorkspaceRelative(d) => {
            (if *d < 0 { "Move window to previous workspace" } else { "Move window to next workspace" }, "Workspaces")
        }
        Action::Screenshot => ("Take a screenshot", "System"),
        Action::Volume(VolumeCmd::Up) => ("Volume up", "System"),
        Action::Volume(VolumeCmd::Down) => ("Volume down", "System"),
        Action::Volume(VolumeCmd::ToggleMute) => ("Mute", "System"),
        Action::Brightness(BrightnessCmd::Up) => ("Brightness up", "System"),
        Action::Brightness(BrightnessCmd::Down) => ("Brightness down", "System"),
        Action::SwitchLayout { backward: false } => ("Switch keyboard layout", "System"),
        Action::SwitchLayout { backward: true } => ("Previous keyboard layout", "System"),
        Action::Suspend => ("Suspend", "System"),
        Action::ToggleHud => ("Frame-time HUD", "System"),
        Action::Quit => ("Log out", "System"),
        Action::None | Action::VtSwitch(_) | Action::ShellKey { .. } => ("", "System"),
    };
    (label.into(), group)
}

/// Split a command line into words: whitespace separates, single and double
/// quotes group, a backslash escapes the next character (outside single quotes).
pub fn split_command(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err(format!("unterminated ' in command: {s}")),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => cur.push(chars.next().ok_or_else(|| format!("trailing \\ in command: {s}"))?),
                        Some(c) => cur.push(c),
                        None => return Err(format!("unterminated \" in command: {s}")),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.push(chars.next().ok_or_else(|| format!("trailing \\ in command: {s}"))?);
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    keys: Spanned<OneOrMany>,
    action: Spanned<String>,
    command: Option<OneOrMany>,
    label: Option<String>,
    group: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    bind: Vec<FileEntry>,
}

/// One `[[bind]]` entry after validation. `action: None` unbinds the chords.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub chords: Vec<Chord>,
    pub action: Option<Action>,
    pub label: String,
    pub group: String,
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// Parse a keybindings file. Errors name the line and say what is wrong.
pub fn parse_file(text: &str) -> Result<Vec<Entry>, String> {
    let file: File = toml::from_str(text).map_err(|e| e.to_string().trim().to_string())?;
    let mut entries = Vec::new();
    let mut seen: Vec<(Chord, usize)> = Vec::new();
    for e in file.bind {
        let line = line_of(text, e.keys.span().start);
        let err = |m: String| format!("line {line}: {m}");
        let command = match e.command {
            None => None,
            Some(OneOrMany::One(s)) => Some(split_command(&s).map_err(err)?),
            Some(OneOrMany::Many(v)) => Some(v),
        };
        let action = parse_action(e.action.get_ref(), command).map_err(|m| format!("line {}: {m}", line_of(text, e.action.span().start)))?;
        let keys = e.keys.into_inner().into_vec();
        if keys.is_empty() {
            return Err(err("'keys' is empty".into()));
        }
        let mut chords = Vec::new();
        for k in &keys {
            let chord = Chord::parse(k).map_err(err)?;
            if let Some((_, prev)) = seen.iter().find(|(c, _)| *c == chord) {
                return Err(err(format!("{} is already bound on line {prev}", chord.spec())));
            }
            seen.push((chord, line));
            chords.push(chord);
        }
        let (default_label, default_group) = action.as_ref().map(default_label).unwrap_or_default();
        entries.push(Entry {
            chords,
            action,
            label: e.label.filter(|l| !l.trim().is_empty()).unwrap_or(default_label),
            group: e.group.filter(|g| !g.trim().is_empty()).unwrap_or_else(|| default_group.to_string()),
        });
    }
    Ok(entries)
}

/// The live binding table.
#[derive(Debug, Clone, Default)]
pub struct KeyBindings {
    pub bindings: Vec<Binding>,
}

impl KeyBindings {
    /// Lay `entries` over the table: their chords are rebound (or unbound by
    /// "none"). A replacement takes the place of the first binding it
    /// displaces, so an edited copy of the defaults keeps their order.
    pub fn apply(&mut self, entries: Vec<Entry>) {
        for e in entries {
            let first = self.bindings.iter().position(|b| e.chords.contains(&b.chord));
            self.bindings.retain(|b| !e.chords.contains(&b.chord));
            let Some(action) = e.action else { continue };
            let at = first.unwrap_or(self.bindings.len()).min(self.bindings.len());
            let new = e.chords.iter().map(|&chord| Binding { chord, action: action.clone(), label: e.label.clone(), group: e.group.clone() });
            self.bindings.splice(at..at, new);
        }
    }

    pub fn from_entries(entries: Vec<Entry>) -> KeyBindings {
        let mut k = KeyBindings::default();
        k.apply(entries);
        k
    }

    /// The compiled-in defaults.
    pub fn builtin() -> KeyBindings {
        KeyBindings::from_entries(parse_file(DEFAULTS).expect("the built-in keybindings are valid"))
    }

    /// Path of the user's file.
    pub fn user_path() -> Option<std::path::PathBuf> {
        if let Some(p) = std::env::var_os("LUMEN_KEYBINDINGS") {
            return Some(p.into());
        }
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config/lumen/keybindings.toml"))
    }

    /// Defaults (system file, else built in) with the user's file laid over
    /// them. A file with errors is left out (the defaults stay in effect) and
    /// its errors are returned for reporting.
    pub fn load() -> (KeyBindings, Vec<String>) {
        let mut errors = Vec::new();
        let mut keys = match std::fs::read_to_string(SYSTEM_FILE) {
            Ok(text) => match parse_file(&text) {
                Ok(e) => KeyBindings::from_entries(e),
                Err(e) => {
                    errors.push(format!("{SYSTEM_FILE}: {e}"));
                    KeyBindings::builtin()
                }
            },
            Err(_) => KeyBindings::builtin(),
        };
        if let Err(e) = keys.apply_user_file() {
            errors.push(e);
        }
        (keys, errors)
    }

    /// Lay the user's file over the table (all or nothing).
    fn apply_user_file(&mut self) -> Result<(), String> {
        let Some(path) = KeyBindings::user_path() else { return Ok(()) };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let entries = parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        self.apply(entries);
        Ok(())
    }

    /// Re-read everything; on any error the result is `Err` and the caller
    /// keeps its current table.
    pub fn reload() -> Result<KeyBindings, String> {
        let (keys, errors) = KeyBindings::load();
        if errors.is_empty() { Ok(keys) } else { Err(errors.join("\n")) }
    }

    /// The binding for a key press (see [`KeySyms`] for what is tried).
    pub fn lookup(&self, mods: Mods, syms: &KeySyms) -> Option<&Binding> {
        let find = |m: Mods, k: Keysym| {
            let chord = Chord { mods: m, key: lower(k) };
            self.bindings.iter().find(|b| b.chord == chord)
        };
        let base = syms.layouts.iter().map(|(b, _)| *b).chain([syms.raw]).filter(|k| *k != Keysym::NoSymbol);
        let mut tried: Vec<Keysym> = Vec::with_capacity(4);
        for k in base {
            let k = lower(k);
            if tried.contains(&k) {
                continue;
            }
            tried.push(k);
            if let Some(b) = find(mods, k) {
                return Some(b);
            }
        }
        // A symbol that needs Shift: Super+Shift+/ is Super+? (but Super+Shift+T is not Super+T).
        if mods.shift {
            let unshifted = Mods { shift: false, ..mods };
            let shifted = syms.layouts.iter().map(|(_, s)| *s).chain([syms.modified]);
            for k in shifted.filter(|k| *k != Keysym::NoSymbol) {
                let k = lower(k);
                if !tried.contains(&k) {
                    if let Some(b) = find(unshifted, k) {
                        return Some(b);
                    }
                }
            }
        }
        None
    }

    /// The modifier-only binding for exactly `mods` (e.g. Alt+Shift).
    pub fn lookup_modifiers(&self, mods: Mods) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.chord.is_modifier_only() && b.chord.mods == mods)
    }
}

/// The symbols of one physical key that shortcuts are matched against.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeySyms {
    /// Per configured layout, first layout first: the unshifted symbol and
    /// the one Shift gives (level 2). Matching the first layout first keeps
    /// shortcuts on the same keys whatever layout is active.
    pub layouts: Vec<(Keysym, Keysym)>,
    /// Unmodified symbol in the active layout.
    pub raw: Keysym,
    /// Symbol with the current modifiers applied.
    pub modified: Keysym,
}

impl KeySyms {
    /// A key that has the same symbols in every layout.
    #[cfg(test)]
    pub fn single(raw: Keysym, shifted: Keysym, modified: Keysym) -> KeySyms {
        KeySyms { layouts: vec![(raw, shifted)], raw, modified }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(raw: u32) -> Keysym {
        Keysym::new(raw)
    }

    fn mods(spec: &str) -> Mods {
        let mut m = Mods::default();
        for p in spec.split('+').filter(|p| !p.is_empty()) {
            match p {
                "super" => m.logo = true,
                "ctrl" => m.ctrl = true,
                "alt" => m.alt = true,
                "shift" => m.shift = true,
                _ => panic!("{p}"),
            }
        }
        m
    }

    #[test]
    fn chord_parsing() {
        let c = Chord::parse("Super+Return").unwrap();
        assert_eq!((c.mods, c.key), (mods("super"), k(keysyms::KEY_Return)));
        assert_eq!(Chord::parse("Ctrl+Alt+T").unwrap(), Chord::parse("ctrl+alt+t").unwrap());
        assert_eq!(Chord::parse("Ctrl+Alt+T").unwrap().key, k(keysyms::KEY_t));
        let c = Chord::parse("Super+Shift+Page_Up").unwrap();
        assert_eq!((c.mods, c.key), (mods("super+shift"), k(keysyms::KEY_Page_Up)));
        assert_eq!(Chord::parse("Super+PageUp").unwrap().key, k(keysyms::KEY_Page_Up));
        assert_eq!(Chord::parse("XF86AudioRaiseVolume").unwrap().key, k(keysyms::KEY_XF86AudioRaiseVolume));
        assert_eq!(Chord::parse("xf86audioraisevolume").unwrap().key, k(keysyms::KEY_XF86AudioRaiseVolume));
        assert_eq!(Chord::parse("Print").unwrap().mods, Mods::default());
        assert_eq!(Chord::parse("Super+/").unwrap().key, k(keysyms::KEY_slash));
        assert_eq!(Chord::parse("Super+?").unwrap().key, k(keysyms::KEY_question));
        assert_eq!(Chord::parse("Super++").unwrap().key, k(keysyms::KEY_plus));
        assert_eq!(Chord::parse("Ctrl+Shift+Escape").unwrap(), Chord::parse("Control+Shift+Esc").unwrap());
        assert_eq!(Chord::parse("Super+F1").unwrap().key, k(keysyms::KEY_F1));
        assert!(Chord::parse("Hyper+T").unwrap_err().contains("unknown modifier"));
        assert!(Chord::parse("Super+NoSuchKey").unwrap_err().contains("unknown key"));
        assert!(Chord::parse("").is_err());
        // Modifiers only.
        let c = Chord::parse("Alt+Shift").unwrap();
        assert!(c.is_modifier_only() && c.mods == mods("alt+shift"));
        assert_eq!(c, Chord::parse("shift+alt").unwrap());
        assert!(Chord::parse("Shift").unwrap_err().contains("single modifier"));
        assert!(Chord::parse("Super+Super").unwrap_err().contains("single modifier"));
        assert!(!Chord::parse("Super+Space").unwrap().is_modifier_only());
        assert!(!Chord::parse("Shift++").unwrap().is_modifier_only());
    }

    #[test]
    fn chord_text_and_caps() {
        assert_eq!(Chord::parse("shift+super+page_up").unwrap().spec(), "Super+Shift+Page_Up");
        assert_eq!(Chord::parse("ctrl+alt+t").unwrap().spec(), "Ctrl+Alt+T");
        assert_eq!(Chord::parse("Super+/").unwrap().spec(), "Super+/");
        assert_eq!(Chord::parse("Super+Left").unwrap().caps(), vec!["Super", "←"]);
        assert_eq!(Chord::parse("Super+Return").unwrap().caps(), vec!["Super", "Enter"]);
        assert_eq!(Chord::parse("XF86AudioMute").unwrap().caps(), vec!["Mute"]);
        assert_eq!(Chord::parse("Alt+Shift+Tab").unwrap().caps(), vec!["Alt", "Shift", "Tab"]);
        assert_eq!(Chord::parse("shift+alt").unwrap().caps(), vec!["Alt", "Shift"]);
        assert_eq!(Chord::parse("shift+alt").unwrap().spec(), "Alt+Shift");
        assert_eq!(Chord::parse("XF86MonBrightnessUp").unwrap().caps(), vec!["Brightness Up"]);
        // Every canonical spec parses back to the same chord.
        for b in KeyBindings::builtin().bindings {
            assert_eq!(Chord::parse(&b.chord.spec()).unwrap(), b.chord, "{}", b.chord.spec());
        }
    }

    #[test]
    fn actions_round_trip() {
        for name in [
            "overview", "app-grid", "shortcuts", "close-window", "maximize", "restore", "tile-left", "tile-right", "fullscreen",
            "switcher", "switcher-reverse", "workspace-1", "workspace-4", "workspace-left", "workspace-right", "move-to-workspace-2",
            "move-to-workspace-left", "move-to-workspace-right", "screenshot", "volume-up", "volume-down", "volume-mute", "hud",
            "quit-session", "brightness-up", "brightness-down", "switch-layout", "switch-layout-backward", "suspend",
        ] {
            let a = parse_action(name, None).unwrap().unwrap();
            assert_eq!(action_name(&a), name);
        }
        assert_eq!(parse_action("none", None).unwrap(), None);
        assert!(parse_action("workspace-9", None).is_err());
        assert!(parse_action("spawn", None).is_err());
        assert!(parse_action("maximize", Some(vec!["x".into()])).is_err());
        assert!(parse_action("maximise", None).unwrap_err().contains("unknown action"));
    }

    #[test]
    fn commands_split_like_a_shell() {
        assert_eq!(split_command("lumen-open https://www.youtube.com").unwrap(), vec!["lumen-open", "https://www.youtube.com"]);
        assert_eq!(split_command(r#"sh -c 'echo "a b" > /tmp/x'"#).unwrap(), vec!["sh", "-c", r#"echo "a b" > /tmp/x"#]);
        assert_eq!(split_command(r#"a "b \"c\"" d\ e"#).unwrap(), vec!["a", r#"b "c""#, "d e"]);
        assert_eq!(split_command("wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+").unwrap()[3], "5%+");
        assert!(split_command("a 'b").is_err());
    }

    #[test]
    fn shipped_defaults() {
        let keys = KeyBindings::builtin();
        let find = |spec: &str| keys.bindings.iter().find(|b| b.chord == Chord::parse(spec).unwrap()).map(|b| b.action.clone());
        let spawn = |cmd: &[&str]| Some(Action::Spawn(cmd.iter().map(|s| s.to_string()).collect()));
        for t in ["Super+Return", "Super+T", "Ctrl+Alt+T"] {
            assert_eq!(find(t), spawn(&["lumen-term"]), "{t}");
        }
        assert_eq!(find("Super+B"), spawn(&["lumen-open", "--browser"]));
        assert_eq!(find("Super+Y"), spawn(&["lumen-open", "https://www.youtube.com"]));
        assert_eq!(find("Super+C"), spawn(&["lumen-claude"]));
        assert_eq!(find("Super+E"), spawn(&["lumen-files"]));
        assert_eq!(find("Super+N"), spawn(&["lumen-edit"]));
        assert_eq!(find("Super+I"), spawn(&["lumen-settings"]));
        assert_eq!(find("Ctrl+Shift+Escape"), spawn(&["lumen-monitor"]));
        assert_eq!(find("Super+K"), spawn(&["lumen-calc"]));
        assert_eq!(find("Super+G"), spawn(&["lumen-steam"]));
        for s in ["Super+Space", "Alt+Shift"] {
            assert_eq!(find(s), Some(Action::SwitchLayout { backward: false }), "{s}");
        }
        assert_eq!(find("Super+Shift+Space"), Some(Action::SwitchLayout { backward: true }));
        assert_eq!(find("XF86MonBrightnessUp"), Some(Action::Brightness(BrightnessCmd::Up)));
        assert_eq!(find("XF86MonBrightnessDown"), Some(Action::Brightness(BrightnessCmd::Down)));
        assert_eq!(find("XF86Sleep"), Some(Action::Suspend));
        assert_eq!(find("XF86Calculator"), spawn(&["lumen-calc"]));
        for s in ["Super+/", "Super+?", "Super+F1"] {
            assert_eq!(find(s), Some(Action::ToggleShortcuts), "{s}");
        }
        assert_eq!(find("Print"), Some(Action::Screenshot));
        assert_eq!(find("XF86AudioRaiseVolume"), Some(Action::Volume(VolumeCmd::Up)));
        assert_eq!(find("XF86AudioLowerVolume"), Some(Action::Volume(VolumeCmd::Down)));
        assert_eq!(find("XF86AudioMute"), Some(Action::Volume(VolumeCmd::ToggleMute)));
        assert_eq!(find("Alt+Tab"), Some(Action::SwitcherNext { reverse: false }));
        assert_eq!(find("Ctrl+Alt+BackSpace"), Some(Action::Quit));
        assert_eq!(find("Super+Shift+Page_Down"), Some(Action::MoveToWorkspaceRelative(1)));
        assert_eq!(find("Super+3"), Some(Action::Workspace(2)));
        // Every binding has a label and one of the four groups.
        for b in &keys.bindings {
            assert!(!b.label.is_empty(), "{b:?}");
            assert!(["Apps", "Windows", "Workspaces", "System"].contains(&b.group.as_str()), "{b:?}");
        }
    }

    #[test]
    fn user_overrides_add_change_and_remove() {
        let mut keys = KeyBindings::builtin();
        let n = keys.bindings.len();
        let pos = |keys: &KeyBindings, s: &str| keys.bindings.iter().position(|b| b.chord == Chord::parse(s).unwrap());
        let term_pos = pos(&keys, "Super+Return").unwrap();
        let user = r#"
            [[bind]]
            keys = "Super+Return"
            action = "spawn"
            command = "foot --server"

            [[bind]]
            keys = ["Super+H", "Super+Q"]
            action = "none"

            [[bind]]
            keys = "Super+J"
            action = "spawn"
            command = ["gimp"]
            label = "GIMP"
        "#;
        keys.apply(parse_file(user).unwrap());
        assert_eq!(keys.bindings.len(), n - 2 + 1);
        let b = &keys.bindings[pos(&keys, "Super+Return").unwrap()];
        assert_eq!(b.action, Action::Spawn(vec!["foot".into(), "--server".into()]));
        assert_eq!((b.label.as_str(), b.group.as_str()), ("Run foot", "Apps"));
        // The replacement keeps the replaced binding's place.
        assert_eq!(pos(&keys, "Super+Return"), Some(term_pos));
        assert!(pos(&keys, "Super+H").is_none() && pos(&keys, "Super+Q").is_none());
        // Other chords of the old binding still work.
        assert_eq!(keys.bindings[pos(&keys, "Super+T").unwrap()].action, Action::Spawn(vec!["lumen-term".into()]));
        let g = &keys.bindings[pos(&keys, "Super+J").unwrap()];
        assert_eq!((g.label.as_str(), g.group.as_str()), ("GIMP", "Apps"));
        assert_eq!(pos(&keys, "Super+J"), Some(keys.bindings.len() - 1));
        // Rebinding a default keeps its place: Steam's chord now opens GIMP.
        let steam_pos = pos(&keys, "Super+G").unwrap();
        keys.apply(parse_file("[[bind]]\nkeys = \"Super+G\"\naction = \"spawn\"\ncommand = \"gimp\"\n").unwrap());
        assert_eq!(pos(&keys, "Super+G"), Some(steam_pos));
        assert_eq!(keys.bindings[steam_pos].action, Action::Spawn(vec!["gimp".into()]));
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse_file("[[bind]]\nkeys = \"Super+T\"\naction = \"maximise\"\n").unwrap_err();
        assert!(e.contains("line 3") && e.contains("unknown action 'maximise'"), "{e}");
        let e = parse_file("[[bind]]\nkeys = \"Super+Nope\"\naction = \"maximize\"\n").unwrap_err();
        assert!(e.contains("line 2") && e.contains("unknown key 'Nope'"), "{e}");
        let e = parse_file("[[bind]]\nkeys = \"Super+T\"\naction = \"spawn\"\n").unwrap_err();
        assert!(e.contains("needs a non-empty 'command'"), "{e}");
        let e = parse_file("[[bind]]\nkeys = \"Super+T\"\naction = \"maximize\"\n\n[[bind]]\nkeys = \"super+t\"\naction = \"restore\"\n")
            .unwrap_err();
        assert!(e.contains("line 6") && e.contains("already bound on line 2"), "{e}");
        let e = parse_file("[[bind]]\nkeys = \"Super+T\"\nactoin = \"maximize\"\n").unwrap_err();
        assert!(e.contains("actoin"), "{e}");
        assert!(parse_file("[[bind]\n").is_err());
        assert!(parse_file("").unwrap().is_empty());
    }

    #[test]
    fn lookup_matches_case_and_shifted_symbols() {
        let keys = KeyBindings::builtin();
        let action = |m: &str, raw: u32, shifted: u32, modified: u32| {
            keys.lookup(mods(m), &KeySyms::single(k(raw), k(shifted), k(modified))).map(|b| b.action.clone())
        };
        let term = Some(Action::Spawn(vec!["lumen-term".into()]));
        // Ctrl+Alt+T with Caps Lock or a typed capital: the same binding.
        assert_eq!(action("ctrl+alt", keysyms::KEY_t, keysyms::KEY_T, keysyms::KEY_T), term);
        // Super+? is Super+Shift+/ on a US keyboard.
        let sheet = Some(Action::ToggleShortcuts);
        assert_eq!(action("super", keysyms::KEY_slash, keysyms::KEY_question, keysyms::KEY_slash), sheet);
        assert_eq!(action("super+shift", keysyms::KEY_slash, keysyms::KEY_question, keysyms::KEY_question), sheet);
        // Shift is significant where it is part of the chord...
        let ws_left = Some(Action::MoveToWorkspaceRelative(-1));
        assert_eq!(action("super+shift", keysyms::KEY_Page_Up, keysyms::KEY_Page_Up, keysyms::KEY_Page_Up), ws_left);
        let rev = Some(Action::SwitcherNext { reverse: true });
        assert_eq!(action("alt+shift", keysyms::KEY_Tab, keysyms::KEY_ISO_Left_Tab, keysyms::KEY_ISO_Left_Tab), rev);
        assert_eq!(action("super+shift", keysyms::KEY_3, keysyms::KEY_numbersign, keysyms::KEY_numbersign), Some(Action::MoveToWorkspace(2)));
        // ...and an uppercase letter is not a shifted symbol.
        assert_eq!(action("super+shift", keysyms::KEY_t, keysyms::KEY_T, keysyms::KEY_T), None);
        assert_eq!(action("", keysyms::KEY_t, keysyms::KEY_T, keysyms::KEY_t), None);
        assert_eq!(action("", keysyms::KEY_Print, keysyms::KEY_Print, keysyms::KEY_Print), Some(Action::Screenshot));
        assert_eq!(action("super", keysyms::KEY_space, keysyms::KEY_space, keysyms::KEY_space), Some(Action::SwitchLayout { backward: false }));
    }

    #[test]
    fn lookup_works_in_every_layout() {
        let keys = KeyBindings::builtin();
        // Layouts "us,ru" with the Russian one active: the T key types "е",
        // the / key "." (Shift: ","), the 3 key "3" (Shift: "№").
        let ru_key = |us: (u32, u32), ru: (u32, u32), modified: u32| KeySyms {
            layouts: vec![(k(us.0), k(us.1)), (k(ru.0), k(ru.1))],
            raw: k(ru.0),
            modified: k(modified),
        };
        let t = ru_key((keysyms::KEY_t, keysyms::KEY_T), (keysyms::KEY_Cyrillic_ie, keysyms::KEY_Cyrillic_IE), keysyms::KEY_Cyrillic_ie);
        assert_eq!(keys.lookup(mods("super"), &t).map(|b| b.action.clone()), Some(Action::Spawn(vec!["lumen-term".into()])));
        let slash = ru_key((keysyms::KEY_slash, keysyms::KEY_question), (keysyms::KEY_period, keysyms::KEY_comma), keysyms::KEY_period);
        assert_eq!(keys.lookup(mods("super"), &slash).map(|b| b.action.clone()), Some(Action::ToggleShortcuts));
        let shifted = KeySyms { modified: k(keysyms::KEY_comma), ..slash.clone() };
        assert_eq!(keys.lookup(mods("super+shift"), &shifted).map(|b| b.action.clone()), Some(Action::ToggleShortcuts));
        // The first layout wins: with "ru,us", Super+. (unbound) is tried before Super+/.
        let mut user = KeyBindings::builtin();
        user.apply(parse_file("[[bind]]\nkeys = \"Super+.\"\naction = \"maximize\"\n").unwrap());
        let ru_first = KeySyms { layouts: vec![slash.layouts[1], slash.layouts[0]], ..slash.clone() };
        assert_eq!(user.lookup(mods("super"), &ru_first).map(|b| b.action.clone()), Some(Action::Maximize));
        assert_eq!(user.lookup(mods("super"), &slash).map(|b| b.action.clone()), Some(Action::ToggleShortcuts));
    }

    #[test]
    fn modifier_only_chords() {
        let keys = KeyBindings::builtin();
        assert_eq!(keys.lookup_modifiers(mods("alt+shift")).map(|b| b.action.clone()), Some(Action::SwitchLayout { backward: false }));
        assert!(keys.lookup_modifiers(mods("alt")).is_none());
        assert!(keys.lookup_modifiers(mods("ctrl+shift")).is_none());
        // A modifier-only chord never matches as a key chord, and vice versa.
        let shift_key = KeySyms::single(k(keysyms::KEY_Shift_L), k(keysyms::KEY_Shift_L), k(keysyms::KEY_Shift_L));
        assert!(keys.lookup(mods("alt+shift"), &shift_key).is_none());
        assert!(is_modifier_key(k(keysyms::KEY_Shift_L)) && is_modifier_key(k(keysyms::KEY_Super_R)) && !is_modifier_key(k(keysyms::KEY_Tab)));
        let mut user = KeyBindings::builtin();
        user.apply(parse_file("[[bind]]\nkeys = [\"Ctrl+Shift\"]\naction = \"switch-layout\"\n\n[[bind]]\nkeys = \"Alt+Shift\"\naction = \"none\"\n").unwrap());
        assert!(user.lookup_modifiers(mods("alt+shift")).is_none());
        assert!(user.lookup_modifiers(mods("ctrl+shift")).is_some());
    }
}
