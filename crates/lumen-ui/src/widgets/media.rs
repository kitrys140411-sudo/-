//! Images, icons and custom drawing.

use std::hash::Hash;

use crate::color::Color;
use crate::geom::Rect;
use crate::icons::Icon;
use crate::id::Id;
use crate::image::Image;
use crate::layout::{Layout, Size, Sizing};
use crate::paint::Painter;
use crate::tree::{Measure, Node, Sense, Visual};
use crate::ui::{Response, Ui};

/// How an image fills its box.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    /// All of it, as large as fits, centered (letterboxed).
    #[default]
    Contain,
    /// The whole box, cropping what does not fit (thumbnails).
    Cover,
    /// Stretched to the box.
    Stretch,
}

/// The rectangle and texture coordinates to draw an `iw`×`ih` image in `r`.
pub fn fit_image(iw: f32, ih: f32, r: Rect, fit: Fit) -> (Rect, [f32; 4]) {
    if iw <= 0.0 || ih <= 0.0 || r.is_empty() {
        return (r, [0.0, 0.0, 1.0, 1.0]);
    }
    match fit {
        Fit::Stretch => (r, [0.0, 0.0, 1.0, 1.0]),
        Fit::Contain => {
            let k = (r.w / iw).min(r.h / ih);
            (r.centered(iw * k, ih * k), [0.0, 0.0, 1.0, 1.0])
        }
        Fit::Cover => {
            let k = (r.w / iw).max(r.h / ih);
            let (vw, vh) = (r.w / k / iw, r.h / k / ih);
            let (u0, v0) = ((1.0 - vw) / 2.0, (1.0 - vh) / 2.0);
            (r, [u0, v0, u0 + vw, v0 + vh])
        }
    }
}

/// An image widget.
#[must_use = "an ImageView does nothing until shown"]
pub struct ImageView {
    image: Image,
    fit: Fit,
    radius: f32,
    layout: Layout,
}

impl ImageView {
    /// The image at its natural size (in logical pixels); size it with
    /// `.width(..)` / `.height(..)` and choose a [`Fit`].
    pub fn new(image: &Image) -> ImageView {
        ImageView { image: image.clone(), fit: Fit::Contain, radius: 0.0, layout: Layout::default() }
    }

    pub fn fit(mut self, f: Fit) -> ImageView {
        self.fit = f;
        self
    }

    /// Rounded corners (thumbnails).
    pub fn radius(mut self, r: f32) -> ImageView {
        self.radius = r;
        self
    }

    pub fn show(self, ui: &mut Ui) {
        let (iw, ih) = self.image.size();
        let mut n = Node::new(self.layout);
        n.measure = Measure::Fixed(crate::geom::Vec2::new(iw as f32, ih as f32));
        n.kind = "image";
        n.label = Some(format!("image {iw}x{ih}"));
        let (img, fit, radius) = (self.image, self.fit, self.radius);
        n.paint = Some(Box::new(move |p, r| {
            let (dst, uv) = fit_image(iw as f32, ih as f32, r, fit);
            p.image_uv(&img, dst, uv, radius);
        }));
        ui.add_node(n);
    }
}

impl Sizing for ImageView {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

/// A box drawn by the app: `paint` gets the painter and the box's
/// rectangle (charts, previews, anything the widgets do not cover).
#[must_use = "a Canvas does nothing until shown"]
pub struct Canvas {
    key: Option<Id>,
    sense: Sense,
    focusable: bool,
    layout: Layout,
}

impl Canvas {
    pub fn new(width: Size, height: Size) -> Canvas {
        Canvas { key: None, sense: Sense::NONE, focusable: false, layout: Layout::default().size(width, height) }
    }

    /// Make the canvas interactive (clicks, drags, scroll) under `key`.
    pub fn sense(mut self, key: impl Hash, sense: Sense) -> Canvas {
        self.key = Some(Id(0).with(key));
        self.sense = sense;
        self
    }

    /// Take keyboard focus on click and Tab.
    pub fn focusable(mut self) -> Canvas {
        self.focusable = true;
        self
    }

    pub fn show(self, ui: &mut Ui, paint: impl FnOnce(&mut Painter, Rect) + 'static) -> Response {
        let id = self.key.map(|k| ui.make_id(k));
        let r = id.map(|id| ui.interact(id)).unwrap_or_default();
        let mut n = Node::new(self.layout);
        n.id = id;
        n.sense = self.sense;
        n.focusable = self.focusable && id.is_some();
        n.kind = "canvas";
        n.paint = Some(Box::new(paint));
        ui.add_node(n);
        r
    }
}

impl Sizing for Canvas {
    fn layout_mut(&mut self) -> &mut Layout {
        &mut self.layout
    }
}

impl Ui {
    /// `img` scaled to fit `width` × `height` (logical px).
    pub fn image(&mut self, img: &Image, width: f32, height: f32) {
        ImageView::new(img).width(Size::Px(width)).height(Size::Px(height)).show(self);
    }

    /// A symbolic icon of `size` in the text color.
    pub fn icon(&mut self, icon: Icon, size: f32) {
        let c = self.theme().text;
        self.icon_colored(icon, size, c);
    }

    pub fn icon_colored(&mut self, icon: Icon, size: f32, color: Color) {
        let mut n = Node::new(Layout::default().size(Size::Px(size), Size::Px(size)));
        n.visual = Visual::Icon { icon, size, color };
        n.kind = "icon";
        n.label = Some(icon.name().to_string());
        self.add_node(n);
    }

    /// Custom drawing in a `width` × `height` box.
    pub fn canvas(&mut self, width: Size, height: Size, paint: impl FnOnce(&mut Painter, Rect) + 'static) -> Response {
        Canvas::new(width, height).show(self, paint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting() {
        let r = Rect::new(0.0, 0.0, 200.0, 100.0);
        let (c, uv) = fit_image(400.0, 400.0, r, Fit::Contain);
        assert_eq!((c, uv), (Rect::new(50.0, 0.0, 100.0, 100.0), [0.0, 0.0, 1.0, 1.0]));
        let (c, uv) = fit_image(400.0, 400.0, r, Fit::Cover);
        assert_eq!(c, r);
        assert_eq!(uv, [0.0, 0.25, 1.0, 0.75]);
    }
}
