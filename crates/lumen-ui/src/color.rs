//! Colors as the theme file writes them: sRGB components with straight
//! alpha. The painter converts to linear premultiplied for the GPU.

/// sRGB color with straight alpha, components 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// From 8-bit sRGB components.
    pub fn rgb8(r: u8, g: u8, b: u8) -> Color {
        Color::rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
    }

    /// `#rrggbb` or `#rrggbbaa`.
    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().strip_prefix('#')?;
        let v = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|x| x as f32 / 255.0);
        match h.len() {
            6 => Some(Color::rgba(v(0)?, v(2)?, v(4)?, 1.0)),
            8 => Some(Color::rgba(v(0)?, v(2)?, v(4)?, v(6)?)),
            _ => None,
        }
    }

    /// Like [`Color::parse`], panicking on malformed input (for literals).
    pub fn hex(s: &str) -> Color {
        Color::parse(s).unwrap_or_else(|| panic!("invalid color literal {s:?}"))
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// Alpha multiplied by `k` (fades).
    pub fn fade(self, k: f32) -> Color {
        Color { a: self.a * k.clamp(0.0, 1.0), ..self }
    }

    /// Linear interpolation of the sRGB components and alpha (`t` = 0 gives
    /// `self`).
    pub fn mix(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        Color::rgba(self.r + (o.r - self.r) * t, self.g + (o.g - self.g) * t, self.b + (o.b - self.b) * t, self.a + (o.a - self.a) * t)
    }

    /// `top` composited over this color (source-over, in sRGB like a
    /// designer's color picker would): the opaque result of a translucent
    /// overlay.
    pub fn over(self, top: Color) -> Color {
        let a = top.a + self.a * (1.0 - top.a);
        if a <= 0.0 {
            return Color::TRANSPARENT;
        }
        let c = |t: f32, b: f32| (t * top.a + b * self.a * (1.0 - top.a)) / a;
        Color::rgba(c(top.r, self.r), c(top.g, self.g), c(top.b, self.b), a)
    }

    /// Relative luminance (0 black .. 1 white), for picking readable text.
    pub fn luminance(self) -> f32 {
        0.2126 * to_linear(self.r) + 0.7152 * to_linear(self.g) + 0.0722 * to_linear(self.b)
    }

    /// Linear-light premultiplied RGBA, what the shaders blend with.
    pub fn linear_premul(self) -> [f32; 4] {
        let a = self.a.clamp(0.0, 1.0);
        [to_linear(self.r) * a, to_linear(self.g) * a, to_linear(self.b) * a, a]
    }

    pub fn is_transparent(self) -> bool {
        self.a <= 0.0
    }

    /// The alpha that makes this color, blended in linear light over
    /// `backdrop`, give the same luminance as blending it in sRGB (how
    /// theme colors are designed and how the desktop draws them; see
    /// [`crate::Painter`]).
    pub fn srgb_blend_alpha(self, backdrop: Color) -> f32 {
        let a = self.a.clamp(0.0, 1.0);
        if a <= 0.0 || a >= 1.0 {
            return a;
        }
        let (lc, lb) = (self.luminance(), backdrop.luminance());
        if (lc - lb).abs() < 0.01 {
            return a;
        }
        let mixed = backdrop.with_alpha(1.0).mix(self.with_alpha(1.0), a).luminance();
        ((mixed - lb) / (lc - lb)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_mix() {
        assert_eq!(Color::parse("#ff0000"), Some(Color::rgba(1.0, 0.0, 0.0, 1.0)));
        assert!((Color::parse("#00000080").unwrap().a - 0.502).abs() < 0.01);
        assert!(Color::parse("red").is_none());
        let m = Color::BLACK.mix(Color::WHITE, 0.5);
        assert!((m.r - 0.5).abs() < 1e-6 && m.a == 1.0);
        let o = Color::BLACK.over(Color::WHITE.with_alpha(0.25));
        assert!((o.r - 0.25).abs() < 1e-6 && (o.a - 1.0).abs() < 1e-6);
        assert_eq!(Color::WHITE.linear_premul(), [1.0, 1.0, 1.0, 1.0]);
        assert!(Color::hex("#7aa2f7").luminance() > Color::hex("#101014").luminance());
    }

    #[test]
    fn translucent_colors_blend_as_designed() {
        let bg = Color::hex("#18181c");
        let tint = Color::WHITE.with_alpha(0.12);
        let a = tint.srgb_blend_alpha(bg);
        assert!(a > 0.01 && a < 0.06, "much less than 0.12 in linear light: {a}");
        // Linear blend with the new alpha == sRGB blend with the old one.
        let lin = |c: Color| c.luminance();
        let linear = lin(Color::WHITE) * a + lin(bg) * (1.0 - a);
        let srgb = lin(bg.mix(Color::WHITE, 0.12));
        assert!((linear - srgb).abs() < 1e-4, "{linear} vs {srgb}");
        assert_eq!(Color::WHITE.srgb_blend_alpha(bg), 1.0, "opaque stays opaque");
        assert_eq!(Color::hex("#1a1a1e80").srgb_blend_alpha(bg), Color::hex("#1a1a1e80").a, "near the backdrop: unchanged");
    }
}
