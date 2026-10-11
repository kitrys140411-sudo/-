//! The desktop theme: one TOML file with colors, radii, shadows, blur and
//! animation timings. Missing keys fall back to the built-in defaults, so a
//! user file only needs the values it changes.
//!
//! Lookup order: `$LUMEN_THEME`, `~/.config/lumen/theme.toml`,
//! `/usr/share/lumen/theme.toml`, built-in defaults.

use serde::Deserialize;

use crate::anim::SpringParams;

/// Linear-light premultiplied RGBA is what the GL shaders want; colors are
/// stored as sRGB (0..1) with straight alpha and converted where needed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Color { r, g, b, a }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().strip_prefix('#')?;
        let v = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|x| x as f32 / 255.0);
        match h.len() {
            6 => Some(Color::rgba(v(0)?, v(2)?, v(4)?, 1.0)),
            8 => Some(Color::rgba(v(0)?, v(2)?, v(4)?, v(6)?)),
            _ => None,
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Straight-alpha sRGB → premultiplied sRGB array for shaders.
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color::rgba(
            self.r + (o.r - self.r) * t,
            self.g + (o.g - self.g) * t,
            self.b + (o.b - self.b) * t,
            self.a + (o.a - self.a) * t,
        )
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color '{s}' (use #rrggbb or #rrggbbaa)")))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub accent: Color,
    pub background: Color,
    pub wallpaper_top: Color,
    pub wallpaper_bottom: Color,
    pub wallpaper_glow_a: Color,
    pub wallpaper_glow_b: Color,
    pub panel: Color,
    pub glass_tint: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub border: Color,
    pub text: Color,
    pub text_dim: Color,
    pub danger: Color,
    pub shadow: Color,
    pub overview_dim: Color,
}

impl Default for Colors {
    fn default() -> Self {
        let c = |s| Color::parse(s).unwrap();
        Colors {
            accent: c("#7aa2f7"),
            background: c("#101014"),
            wallpaper_top: c("#1b1d2b"),
            wallpaper_bottom: c("#0b0b10"),
            wallpaper_glow_a: c("#5b6fd6"),
            wallpaper_glow_b: c("#b05bd6"),
            panel: c("#0b0b0fd9"),
            glass_tint: c("#16161cb3"),
            surface: c("#1d1d25f2"),
            surface_hover: c("#2a2a36ff"),
            border: c("#ffffff1a"),
            text: c("#ececf2"),
            text_dim: c("#9a9aaa"),
            danger: c("#ff7b72"),
            shadow: c("#000000"),
            overview_dim: c("#00000040"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Geometry {
    /// Base spacing unit; paddings and gaps are multiples of it.
    pub grid: f64,
    pub window_radius: f64,
    pub panel_height: f64,
    pub popover_radius: f64,
    pub dock_icon_size: f64,
    pub dock_radius: f64,
    pub notification_width: f64,
    pub switcher_icon_size: f64,
    pub shadow_blur: f64,
    pub shadow_offset_y: f64,
    pub shadow_alpha: f64,
    pub shadow_alpha_focused: f64,
    pub hot_corner: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Geometry {
            grid: 8.0,
            window_radius: 12.0,
            panel_height: 32.0,
            popover_radius: 14.0,
            dock_icon_size: 52.0,
            dock_radius: 22.0,
            notification_width: 380.0,
            switcher_icon_size: 72.0,
            shadow_blur: 26.0,
            shadow_offset_y: 8.0,
            shadow_alpha: 0.38,
            shadow_alpha_focused: 0.55,
            hot_corner: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Blur {
    pub enabled: bool,
    /// Dual-Kawase down/up-sample passes (each halves the resolution).
    pub passes: u32,
    /// Sample offset in texels of the half-resolution image.
    pub offset: f32,
    pub saturation: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Blur { enabled: true, passes: 3, offset: 3.0, saturation: 1.25 }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringConfig {
    pub stiffness: f64,
    pub damping: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Animation {
    /// Global multiplier for all durations (0 disables animations).
    pub speed: f64,
    pub window_move: SpringConfig,
    pub window_open: SpringConfig,
    pub window_close: SpringConfig,
    pub workspace: SpringConfig,
    pub overview: SpringConfig,
    pub popover: SpringConfig,
    pub notification_timeout_ms: u64,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            speed: 1.0,
            window_move: SpringConfig { stiffness: 800.0, damping: 1.0 },
            window_open: SpringConfig { stiffness: 520.0, damping: 0.82 },
            window_close: SpringConfig { stiffness: 900.0, damping: 1.0 },
            workspace: SpringConfig { stiffness: 380.0, damping: 1.0 },
            overview: SpringConfig { stiffness: 420.0, damping: 1.0 },
            popover: SpringConfig { stiffness: 700.0, damping: 0.85 },
            notification_timeout_ms: 5000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fonts {
    pub ui: String,
    pub ui_size: f32,
    pub monospace: String,
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts { ui: "Inter".into(), ui_size: 14.0, monospace: "JetBrains Mono".into() }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub colors: Colors,
    pub geometry: Geometry,
    pub blur: Blur,
    pub animation: Animation,
    pub fonts: Fonts,
}

impl Theme {
    pub fn parse(text: &str) -> Result<Theme, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Load the first theme file that exists; report (and ignore) broken ones.
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
            match std::fs::read_to_string(&path) {
                Ok(text) => match Theme::parse(&text) {
                    Ok(t) => {
                        tracing::info!("theme: loaded {path}");
                        return t;
                    }
                    Err(e) => tracing::error!("theme: {path}: {e}"),
                },
                Err(_) => continue,
            }
        }
        Theme::default()
    }

    /// Spring parameters scaled by the global animation speed (speed 0 → instant).
    pub fn spring(&self, cfg: SpringConfig, epsilon: f64) -> SpringParams {
        let speed = self.animation.speed;
        if speed <= 0.0 {
            return SpringParams::new(1e12, 1.0, epsilon);
        }
        SpringParams::new(cfg.stiffness * speed * speed, cfg.damping, epsilon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#ff0000"), Some(Color::rgba(1.0, 0.0, 0.0, 1.0)));
        let c = Color::parse("#00000080").unwrap();
        assert!((c.a - 0.502).abs() < 0.01);
        assert!(Color::parse("red").is_none());
        assert!(Color::parse("#12345").is_none());
    }

    #[test]
    fn partial_theme_file() {
        let t = Theme::parse("[colors]\naccent = \"#ff8800\"\n[geometry]\nwindow_radius = 4\n").unwrap();
        assert_eq!(t.colors.accent, Color::parse("#ff8800").unwrap());
        assert_eq!(t.geometry.window_radius, 4.0);
        assert_eq!(t.geometry.panel_height, Geometry::default().panel_height);
        assert!(Theme::parse("[colors]\nnope = \"#000000\"\n").is_err());
        assert!(Theme::parse("[colors]\naccent = \"blue\"\n").unwrap_err().contains("invalid color"));
    }

    #[test]
    fn shipped_theme_parses() {
        let text = include_str!("../../../rootfs/usr/share/lumen/theme.toml");
        Theme::parse(text).unwrap();
    }
}
