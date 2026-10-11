//! Configuration (`~/.config/lumen/term.toml`, falling back to
//! `/usr/share/lumen/term.toml`, then built-in defaults). Every key is
//! optional.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.trim().strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    pub fn tuple(self) -> (u8, u8, u8) {
        (self.0, self.1, self.2)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Rgb, D::Error> {
        let s = String::deserialize(d)?;
        Rgb::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color {s:?} (want #rrggbb)")))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Font {
    pub family: String,
    /// Size in logical pixels.
    pub size: f32,
    /// Line height as a multiple of the font's natural height.
    pub line_height: f32,
    /// Extra letter spacing in logical pixels.
    pub letter_spacing: f32,
}

impl Default for Font {
    fn default() -> Self {
        Font { family: "JetBrains Mono".into(), size: 14.0, line_height: 1.0, letter_spacing: 0.0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub cursor_text: Rgb,
    pub selection: Rgb,
    pub search_match: Rgb,
    pub search_current: Rgb,
    /// The 16 ANSI colors.
    pub palette: Vec<Rgb>,
    /// Draw bold text in the bright variant of colors 0–7.
    pub bold_is_bright: bool,
}

impl Default for Colors {
    fn default() -> Self {
        let p = [
            "#1d1f28", "#f26d78", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5", "#414868", "#ff7a93", "#b9f27c",
            "#ff9e64", "#7da6ff", "#bb9af7", "#0db9d7", "#e6e9f5",
        ];
        Colors {
            foreground: Rgb(0xd8, 0xdc, 0xec),
            background: Rgb(0x12, 0x13, 0x1a),
            cursor: Rgb(0xc8, 0xd3, 0xf5),
            cursor_text: Rgb(0x12, 0x13, 0x1a),
            selection: Rgb(0x2d, 0x3b, 0x66),
            search_match: Rgb(0x5a, 0x4a, 0x1e),
            search_current: Rgb(0xb0, 0x7a, 0x1e),
            palette: p.iter().map(|s| Rgb::parse(s).unwrap()).collect(),
            bold_is_bright: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cursor {
    pub style: CursorStyle,
    pub blink: bool,
    /// Cursor glides between cells instead of jumping.
    pub smooth: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Cursor { style: CursorStyle::Block, blink: true, smooth: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    pub history: usize,
    /// Lines per mouse wheel notch.
    pub wheel_lines: f32,
    pub smooth: bool,
}

impl Default for Scroll {
    fn default() -> Self {
        Scroll { history: crate::term::DEFAULT_HISTORY, wheel_lines: 3.0, smooth: true }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    /// Padding around the cell grid, logical pixels.
    pub padding: f32,
    pub width_cells: u16,
    pub height_cells: u16,
    /// Show the tab bar even with a single tab.
    pub always_show_tabs: bool,
}

impl Default for Window {
    fn default() -> Self {
        Window { padding: 10.0, width_cells: 100, height_cells: 30, always_show_tabs: false }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Shell {
    /// Program and arguments; empty means $SHELL (or lsh).
    pub program: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Behavior {
    /// Let applications set the clipboard (OSC 52 writes).
    pub osc52_write: bool,
    /// Let applications read the clipboard (OSC 52 queries).
    pub osc52_read: bool,
    /// Copy selections to the clipboard as well as the primary selection.
    pub copy_on_select: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Behavior { osc52_write: true, osc52_read: false, copy_on_select: false }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: Font,
    pub colors: Colors,
    pub cursor: Cursor,
    pub scroll: Scroll,
    pub window: Window,
    pub shell: Shell,
    pub behavior: Behavior,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let c: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        if c.colors.palette.len() != 16 {
            return Err(format!("colors.palette needs 16 colors, has {}", c.colors.palette.len()));
        }
        if !(4.0..=200.0).contains(&c.font.size) {
            return Err(format!("font.size {} out of range 4–200", c.font.size));
        }
        Ok(c)
    }

    /// Load the first config file that exists. A broken file is reported and
    /// the defaults are used (with the error, for showing it to the user).
    pub fn load() -> (Config, Option<String>) {
        let mut candidates = Vec::new();
        if let Ok(p) = std::env::var("LUMEN_TERM_CONFIG") {
            candidates.push(p);
        }
        if let Ok(h) = std::env::var("XDG_CONFIG_HOME") {
            candidates.push(format!("{h}/lumen/term.toml"));
        }
        if let Ok(h) = std::env::var("HOME") {
            candidates.push(format!("{h}/.config/lumen/term.toml"));
        }
        candidates.push("/usr/share/lumen/term.toml".into());
        for path in candidates {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            return match Config::parse(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("{path}: {e}"))),
            };
        }
        (Config::default(), None)
    }

    pub fn palette16(&self) -> [(u8, u8, u8); 16] {
        let mut p = [(0, 0, 0); 16];
        for (i, c) in self.colors.palette.iter().take(16).enumerate() {
            p[i] = c.tuple();
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_partial_files() {
        let c = Config::parse("[font]\nsize = 16\n[colors]\nbackground = \"#000000\"\n").unwrap();
        assert_eq!(c.font.size, 16.0);
        assert_eq!(c.font.family, "JetBrains Mono");
        assert_eq!(c.colors.background, Rgb(0, 0, 0));
        assert_eq!(c.colors.palette.len(), 16);
    }

    #[test]
    fn errors_are_reported() {
        assert!(Config::parse("[font]\nsize = \"big\"").is_err());
        assert!(Config::parse("[colors]\nforeground = \"red\"").unwrap_err().contains("invalid color"));
        assert!(Config::parse("[nope]\n").is_err());
        assert!(Config::parse("[colors]\npalette = [\"#000000\"]").unwrap_err().contains("16 colors"));
    }
}
