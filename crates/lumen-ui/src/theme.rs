//! The look of every Lumen app, derived from the desktop's theme file so apps
//! match the shell: `[colors]` (accent, background, surface, surface_hover,
//! border, text, text_dim, danger), `[fonts]` (ui, ui_size, monospace),
//! `[geometry]` (grid, popover_radius) and `[animation]` (speed).
//!
//! Lookup order is the compositor's: `$LUMEN_THEME`,
//! `~/.config/lumen/theme.toml`, `/usr/share/lumen/theme.toml`, built-in
//! defaults. Unknown keys are ignored and malformed values keep their default,
//! so a theme written for a newer desktop never breaks an app.

use crate::color::Color;

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    // -- From the theme file --------------------------------------------------------
    pub accent: Color,
    /// The desktop background; windows are a little lighter.
    pub background: Color,
    /// Menus, popovers and dialogs (the shell's surface color).
    pub surface: Color,
    pub surface_hover: Color,
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub danger: Color,

    // -- Derived (overridable after loading) ----------------------------------------
    pub window_bg: Color,
    pub header_bg: Color,
    pub sidebar_bg: Color,
    /// Cards and boxed lists on the window background.
    pub card_bg: Color,
    /// Button and other control backgrounds: rest, hover, pressed.
    pub control: Color,
    pub control_hover: Color,
    pub control_active: Color,
    pub input_bg: Color,
    pub accent_hover: Color,
    pub accent_active: Color,
    /// Text and icons on accent-colored backgrounds.
    pub on_accent: Color,
    /// Selected rows and text selections.
    pub selection: Color,
    pub focus_ring: Color,
    pub separator: Color,
    pub text_disabled: Color,
    pub success: Color,
    pub warning: Color,
    pub shadow: Color,
    /// Dimming behind modal dialogs.
    pub scrim: Color,

    // -- Fonts and metrics ----------------------------------------------------------
    pub font_ui: String,
    pub font_mono: String,
    /// Body text size (logical px); other sizes are derived from it.
    pub font_size: f32,
    /// The spacing unit: paddings and gaps are multiples of it.
    pub grid: f32,
    /// Corner radius of buttons, entries and list rows.
    pub radius: f32,
    /// Corner radius of cards, popovers and dialogs.
    pub radius_large: f32,
    /// Animation speed factor (0 disables animations).
    pub anim_speed: f32,
}

impl Default for Theme {
    fn default() -> Theme {
        let mut t = Theme {
            accent: Color::hex("#7aa2f7"),
            background: Color::hex("#101014"),
            surface: Color::hex("#1d1d25f2"),
            surface_hover: Color::hex("#2a2a36"),
            border: Color::hex("#ffffff1a"),
            text: Color::hex("#ececf2"),
            text_dim: Color::hex("#9a9aaa"),
            danger: Color::hex("#ff7b72"),
            window_bg: Color::TRANSPARENT,
            header_bg: Color::TRANSPARENT,
            sidebar_bg: Color::TRANSPARENT,
            card_bg: Color::TRANSPARENT,
            control: Color::TRANSPARENT,
            control_hover: Color::TRANSPARENT,
            control_active: Color::TRANSPARENT,
            input_bg: Color::TRANSPARENT,
            accent_hover: Color::TRANSPARENT,
            accent_active: Color::TRANSPARENT,
            on_accent: Color::TRANSPARENT,
            selection: Color::TRANSPARENT,
            focus_ring: Color::TRANSPARENT,
            separator: Color::TRANSPARENT,
            text_disabled: Color::TRANSPARENT,
            success: Color::hex("#73d99f"),
            warning: Color::hex("#f2c46d"),
            shadow: Color::BLACK,
            scrim: Color::TRANSPARENT,
            font_ui: "Inter".into(),
            font_mono: "JetBrains Mono".into(),
            font_size: 14.0,
            grid: 8.0,
            radius: 8.0,
            radius_large: 14.0,
            anim_speed: 1.0,
        };
        t.derive();
        t
    }
}

/// Contrast ratio of two colors (WCAG definition).
fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (a.luminance() + 0.05, b.luminance() + 0.05);
    la.max(lb) / la.min(lb)
}

impl Theme {
    /// The desktop's theme (see the module docs for the lookup order).
    pub fn load() -> Theme {
        let mut candidates = Vec::new();
        if let Ok(p) = std::env::var("LUMEN_THEME") {
            candidates.push(p);
        }
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(format!("{}/.config/lumen/theme.toml", home.to_string_lossy()));
        }
        candidates.push("/usr/share/lumen/theme.toml".into());
        for path in candidates {
            if let Ok(text) = std::fs::read_to_string(&path) {
                return Theme::from_toml(&text);
            }
        }
        Theme::default()
    }

    /// Theme from the text of a theme file (lenient, see the module docs).
    pub fn from_toml(text: &str) -> Theme {
        let mut t = Theme::default();
        let Ok(doc) = text.parse::<toml::Table>() else {
            eprintln!("lumen-ui: theme file does not parse; using defaults");
            return t;
        };
        let section = |name: &str| doc.get(name).and_then(|v| v.as_table());
        if let Some(c) = section("colors") {
            let get = |k: &str, d: Color| c.get(k).and_then(|v| v.as_str()).and_then(Color::parse).unwrap_or(d);
            t.accent = get("accent", t.accent);
            t.background = get("background", t.background);
            t.surface = get("surface", t.surface);
            t.surface_hover = get("surface_hover", t.surface_hover);
            t.border = get("border", t.border);
            t.text = get("text", t.text);
            t.text_dim = get("text_dim", t.text_dim);
            t.danger = get("danger", t.danger);
        }
        let num = |v: &toml::Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).map(|f| f as f32);
        if let Some(f) = section("fonts") {
            if let Some(s) = f.get("ui").and_then(|v| v.as_str()) {
                t.font_ui = s.to_string();
            }
            if let Some(s) = f.get("monospace").and_then(|v| v.as_str()) {
                t.font_mono = s.to_string();
            }
            if let Some(v) = f.get("ui_size").and_then(num) {
                t.font_size = v.clamp(8.0, 32.0);
            }
        }
        if let Some(g) = section("geometry") {
            if let Some(v) = g.get("grid").and_then(num) {
                t.grid = v.clamp(2.0, 32.0);
            }
            if let Some(v) = g.get("popover_radius").and_then(num) {
                t.radius_large = v.clamp(0.0, 40.0);
            }
        }
        if let Some(a) = section("animation") {
            if let Some(v) = a.get("speed").and_then(num) {
                t.anim_speed = v.max(0.0);
            }
        }
        t.derive();
        t
    }

    /// Recompute the derived colors from the base ones (call after changing
    /// a base color by hand).
    pub fn derive(&mut self) {
        let white = Color::WHITE;
        let black = Color::BLACK;
        let opaque = |c: Color| self.background.over(c);
        self.window_bg = opaque(white.with_alpha(0.035));
        self.header_bg = opaque(white.with_alpha(0.06));
        self.sidebar_bg = opaque(white.with_alpha(0.018));
        self.card_bg = self.window_bg.over(white.with_alpha(0.04));
        self.control = white.with_alpha(0.08);
        self.control_hover = white.with_alpha(0.12);
        self.control_active = white.with_alpha(0.17);
        self.input_bg = white.with_alpha(0.06);
        self.accent_hover = self.accent.mix(white, 0.12);
        self.accent_active = self.accent.mix(black, 0.12);
        let dark = Color::hex("#0c0e16");
        self.on_accent = if contrast(self.accent, dark) >= contrast(self.accent, white) { dark } else { white };
        self.selection = self.accent.with_alpha(0.26);
        self.focus_ring = self.accent.with_alpha(0.75);
        self.separator = white.with_alpha(0.07);
        self.text_disabled = self.text_dim.with_alpha(0.5);
        self.scrim = black.with_alpha(0.45);
    }

    /// Text sizes (logical px) of the type scale.
    pub fn size_small(&self) -> f32 {
        (self.font_size * 0.86).round()
    }

    pub fn size_heading(&self) -> f32 {
        (self.font_size * 1.3).round()
    }

    pub fn size_title(&self) -> f32 {
        (self.font_size * 2.0).round()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_keeps_defaults() {
        let t = Theme::from_toml("[colors]\naccent = \"#ff8800\"\nnew_key = \"#000000\"\n[fonts]\nui_size = 15\n[future]\nx = 1\n");
        assert_eq!(t.accent, Color::hex("#ff8800"));
        assert_eq!(t.text, Theme::default().text);
        assert_eq!(t.font_size, 15.0);
        assert_eq!(t.accent_hover, Color::hex("#ff8800").mix(Color::WHITE, 0.12), "derived from the new accent");
    }

    #[test]
    fn malformed_values_and_files() {
        let t = Theme::from_toml("[colors]\naccent = \"blue\"\ntext = 3\n");
        assert_eq!(t.accent, Theme::default().accent);
        assert_eq!(Theme::from_toml("this is [ not toml"), Theme::default());
    }

    #[test]
    fn shipped_theme_matches_defaults() {
        let text = include_str!("../../../rootfs/usr/share/lumen/theme.toml");
        let t = Theme::from_toml(text);
        assert_eq!(t.accent, Theme::default().accent);
        assert_eq!(t.font_ui, "Inter");
        assert_eq!(t.radius_large, 14.0);
    }

    #[test]
    fn readable_text_on_accent() {
        let t = Theme::default();
        assert!(contrast(t.accent, t.on_accent) > 4.5);
        let mut dark = Theme::default();
        dark.accent = Color::hex("#2050c0");
        dark.derive();
        assert_eq!(dark.on_accent, Color::WHITE);
    }
}
