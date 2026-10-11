//! UI text: labels are laid out and painted on the CPU (lumen-text) and
//! uploaded once as textures drawn 1:1 in physical pixels; a label is
//! re-rendered only when its text or style changes. Unused labels are evicted.

use std::collections::HashMap;

use lumen_text::{Align, TextStyle, TextSystem, Weight};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::utils::{Physical, Point, Rectangle};

use crate::render::elements::{Image, ImageElement};
use crate::theme::Color;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    text: String,
    size: u32,
    weight: Weight,
    color: [u32; 4],
    align: Align,
    max_width: Option<u32>,
    max_lines: Option<usize>,
}

pub struct Label {
    image: Image,
    /// Layout origin inside the image (ink may overhang the layout box).
    origin: (i32, i32),
    /// Layout box size in physical pixels.
    pub width: f32,
    pub height: f32,
    last_used: u64,
}

/// A text request, sizes in logical pixels (scaled by the output scale).
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub size: f64,
    pub weight: Weight,
    pub color: Color,
    pub align: Align,
}

impl Style {
    pub fn new(size: f64, color: Color) -> Style {
        Style { size, weight: Weight::Regular, color, align: Align::Left }
    }

    pub fn weight(mut self, w: Weight) -> Self {
        self.weight = w;
        self
    }

    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
}

pub struct UiText {
    sys: Option<TextSystem>,
    labels: HashMap<Key, Label>,
    frame: u64,
}

impl UiText {
    pub fn new(family: &str) -> UiText {
        let sys = match TextSystem::new(family) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("text rendering unavailable: {e}");
                None
            }
        };
        UiText { sys, labels: HashMap::new(), frame: 0 }
    }

    fn key(text: &str, style: &Style, scale: f64, max_width: Option<f64>, max_lines: Option<usize>) -> Key {
        let c = style.color;
        Key {
            text: text.to_string(),
            size: ((style.size * scale) as f32).to_bits(),
            weight: style.weight,
            color: [c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits()],
            align: style.align,
            max_width: max_width.map(|w| ((w * scale) as f32).to_bits()),
            max_lines,
        }
    }

    /// Lay out (and paint, if new) a label; sizes of the result are physical.
    #[allow(clippy::too_many_arguments)]
    pub fn label(
        &mut self,
        r: &mut GlesRenderer,
        text: &str,
        style: &Style,
        scale: f64,
        max_width: Option<f64>,
        max_lines: Option<usize>,
    ) -> Option<&mut Label> {
        let key = Self::key(text, style, scale, max_width, max_lines);
        let frame = self.frame;
        if !self.labels.contains_key(&key) {
            let sys = self.sys.as_mut()?;
            let c = style.color;
            let ts = TextStyle::new((style.size * scale) as f32, [c.r, c.g, c.b, c.a]).weight(style.weight).align(style.align);
            let img = sys.render(&ts, text, max_width.map(|w| (w * scale) as f32), max_lines);
            let image = Image::upload(r, &img.data, img.width, img.height).map_err(|e| tracing::warn!("label upload: {e:?}")).ok()?;
            self.labels.insert(
                key.clone(),
                Label { image, origin: img.origin, width: img.layout_size.0, height: img.layout_size.1, last_used: frame },
            );
        }
        let label = self.labels.get_mut(&key)?;
        label.last_used = frame;
        Some(label)
    }

    /// Layout size (physical) of a label without drawing it.
    pub fn measure(&mut self, text: &str, style: &Style, scale: f64, max_width: Option<f64>, max_lines: Option<usize>) -> (f64, f64) {
        let Some(sys) = self.sys.as_mut() else { return (0.0, 0.0) };
        let c = style.color;
        let ts = TextStyle::new((style.size * scale) as f32, [c.r, c.g, c.b, c.a]).weight(style.weight);
        let lines = sys.layout(&ts, text, max_width.map(|w| (w * scale) as f32), max_lines);
        let w = lines.iter().map(|l| l.width).fold(0.0f32, f32::max) as f64;
        let line_h = (ts.size * ts.line_spacing).round() as f64;
        let h = match lines.first() {
            Some(first) => first.ascent as f64 + (lines.len() - 1) as f64 * line_h + lines.last().unwrap().descent as f64,
            None => 0.0,
        };
        (w, h)
    }

    /// A render element drawing the label with its layout box's top-left at
    /// `at` (physical; snapped to whole pixels so glyphs stay crisp), and the
    /// layout box.
    #[allow(clippy::too_many_arguments)]
    pub fn element(
        &mut self,
        r: &mut GlesRenderer,
        text: &str,
        style: &Style,
        scale: f64,
        max_width: Option<f64>,
        max_lines: Option<usize>,
        at: Point<f64, Physical>,
        alpha: f32,
    ) -> Option<(ImageElement, Rectangle<f64, Physical>)> {
        let label = self.label(r, text, style, scale, max_width, max_lines)?;
        let loc = Point::<i32, Physical>::from((at.x.round() as i32 - label.origin.0, at.y.round() as i32 - label.origin.1));
        let rect = Rectangle::new(at, (label.width as f64, label.height as f64).into());
        Some((label.image.element(loc, alpha), rect))
    }

    /// Call once per rendered frame: labels unused for ~10 s are dropped.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        if frame % 120 == 0 {
            self.labels.retain(|_, l| frame - l.last_used < 600);
        }
    }

    /// Paint text into an image without caching (icon letters).
    pub fn render_image(&mut self, text: &str, size_px: f32, weight: Weight) -> Option<lumen_text::Image> {
        let sys = self.sys.as_mut()?;
        Some(sys.render(&TextStyle::new(size_px, [1.0, 1.0, 1.0, 1.0]).weight(weight), text, None, None))
    }
}
