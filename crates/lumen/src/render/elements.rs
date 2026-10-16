//! Custom render elements.
//!
//! * [`WindowSurfaceElement`] — a client surface moved/scaled for animations and
//!   clipped to the window geometry with rounded corners.
//! * [`Shape`] — persistent pixel-shader elements (rounded rects, shadows) that
//!   keep their id across frames so damage tracking stays precise.

use smithay::backend::renderer::element::surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{
    GlesError, GlesFrame, GlesPixelProgram, GlesRenderer, GlesTexProgram, GlesTexture, Uniform, UniformValue,
};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Texture;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform};

/// 3x3 matrix, column-major (same conventions as the cgmath matrices smithay
/// uses internally, so the renderer's texture matrix can be reproduced).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3 {
    /// `c[col][row]`
    pub c: [[f32; 3]; 3],
}

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3 { c: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };

    /// Arguments in cgmath order: column 0 (rows 0..3), column 1, column 2.
    #[allow(clippy::too_many_arguments)]
    pub const fn new(c0r0: f32, c0r1: f32, c0r2: f32, c1r0: f32, c1r1: f32, c1r2: f32, c2r0: f32, c2r1: f32, c2r2: f32) -> Mat3 {
        Mat3 { c: [[c0r0, c0r1, c0r2], [c1r0, c1r1, c1r2], [c2r0, c2r1, c2r2]] }
    }

    pub fn translation(x: f32, y: f32) -> Mat3 {
        Mat3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, y, 1.0)
    }

    pub fn scale(x: f32, y: f32) -> Mat3 {
        Mat3::new(x, 0.0, 0.0, 0.0, y, 0.0, 0.0, 0.0, 1.0)
    }

    fn at(&self, row: usize, col: usize) -> f32 {
        self.c[col][row]
    }

    pub fn mul(&self, o: &Mat3) -> Mat3 {
        let mut r = [[0.0f32; 3]; 3];
        for (col, rc) in r.iter_mut().enumerate() {
            for (row, v) in rc.iter_mut().enumerate() {
                *v = (0..3).map(|k| self.at(row, k) * o.at(k, col)).sum();
            }
        }
        Mat3 { c: r }
    }

    #[cfg(test)]
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.at(0, 0) * x + self.at(0, 1) * y + self.at(0, 2),
            self.at(1, 0) * x + self.at(1, 1) * y + self.at(1, 2),
        )
    }

    pub fn inverse(&self) -> Option<Mat3> {
        let a = |r, c| self.at(r, c) as f64;
        let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1)) - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
            + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = |r: usize, c: usize| -> f32 {
            // Cofactor of (c, r) (transposed) / det.
            let rows: Vec<usize> = (0..3).filter(|&x| x != c).collect();
            let cols: Vec<usize> = (0..3).filter(|&x| x != r).collect();
            let minor = a(rows[0], cols[0]) * a(rows[1], cols[1]) - a(rows[0], cols[1]) * a(rows[1], cols[0]);
            let sign = if (r + c) % 2 == 0 { 1.0 } else { -1.0 };
            (sign * minor / det) as f32
        };
        let mut m = Mat3::IDENTITY;
        for row in 0..3 {
            for col in 0..3 {
                m.c[col][row] = inv(row, col);
            }
        }
        Some(m)
    }

    pub fn uniform(&self, name: &'static str) -> Uniform<'static> {
        let flat = [
            self.c[0][0], self.c[0][1], self.c[0][2], self.c[1][0], self.c[1][1], self.c[1][2], self.c[2][0], self.c[2][1],
            self.c[2][2],
        ];
        Uniform::new(name, UniformValue::Matrix3x3 { matrices: vec![flat], transpose: false })
    }
}

pub fn transform_matrix(t: Transform) -> Mat3 {
    match t {
        Transform::Normal => Mat3::new(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
        Transform::_90 => Mat3::new(0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Transform::_180 => Mat3::new(-1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
        Transform::_270 => Mat3::new(0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Transform::Flipped => Mat3::new(-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0),
        Transform::Flipped90 => Mat3::new(0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        Transform::Flipped180 => Mat3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0),
        Transform::Flipped270 => Mat3::new(0.0, -1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
    }
}

/// The matrix GlesRenderer uses to map dest-local pixel positions to texture
/// coordinates (mirrors `build_texture_mat` plus the y-inversion step).
pub fn texture_matrix(
    src: Rectangle<f64, Buffer>,
    dst_size: Size<i32, Physical>,
    tex_size: Size<i32, Buffer>,
    transform: Transform,
    y_inverted: bool,
) -> Mat3 {
    let dst_src = transform.transform_size(src.size);
    let mut m = Mat3::scale((dst_src.w / dst_size.w as f64) as f32, (dst_src.h / dst_size.h as f64) as f32);
    m = transform_matrix(transform).mul(&m);
    let (w, h) = (dst_src.w as f32, dst_src.h as f32);
    let translation = match transform {
        Transform::Normal | Transform::Flipped90 => Mat3::IDENTITY,
        Transform::_90 => Mat3::translation(0.0, w),
        Transform::_180 => Mat3::translation(w, h),
        Transform::_270 => Mat3::translation(h, 0.0),
        Transform::Flipped => Mat3::translation(w, 0.0),
        Transform::Flipped180 => Mat3::translation(0.0, h),
        Transform::Flipped270 => Mat3::translation(h, w),
    };
    m = translation.mul(&m);
    m = Mat3::translation(src.loc.x as f32, src.loc.y as f32).mul(&m);
    m = Mat3::scale(1.0 / tex_size.w as f32, 1.0 / tex_size.h as f32).mul(&m);
    if y_inverted {
        m = Mat3::new(1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 1.0).mul(&m);
    }
    m
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clip {
    /// Window geometry in final physical coordinates.
    pub geo: Rectangle<f64, Physical>,
    /// Corner radius in physical pixels.
    pub radius: f32,
}

/// A client surface with an animation transform and rounded clipping:
/// final = (inner - origin) * scale + origin + offset, with separate x and y
/// scales (maximize morphs stretch windows non-uniformly).
pub struct WindowSurfaceElement {
    inner: WaylandSurfaceRenderElement<GlesRenderer>,
    /// Replaces the surface's id when the same surface is drawn twice in one
    /// frame (switcher thumbnails), so damage tracking keeps them apart.
    alias: Option<Id>,
    origin: Point<f64, Physical>,
    scale: (f64, f64),
    offset: Point<f64, Physical>,
    clip: Option<Clip>,
    program: GlesTexProgram,
    /// Rounded-rect pixel shader for single-pixel-buffer (solid color) surfaces.
    solid: Option<GlesPixelProgram>,
}

impl WindowSurfaceElement {
    pub fn new(
        inner: WaylandSurfaceRenderElement<GlesRenderer>,
        origin: Point<f64, Physical>,
        scale: (f64, f64),
        offset: Point<f64, Physical>,
        clip: Option<Clip>,
        program: GlesTexProgram,
    ) -> Self {
        WindowSurfaceElement { inner, alias: None, origin, scale, offset, clip, program, solid: None }
    }

    /// Clip solid-color surfaces with this rounded-rect pixel shader.
    pub fn with_solid_program(mut self, p: GlesPixelProgram) -> Self {
        self.solid = Some(p);
        self
    }

    pub fn with_alias(mut self, id: Id) -> Self {
        self.alias = Some(id);
        self
    }

    pub fn inner_id(&self) -> &Id {
        self.inner.id()
    }

    fn map_point(&self, p: Point<f64, Physical>) -> Point<f64, Physical> {
        Point::from((
            (p.x - self.origin.x) * self.scale.0 + self.origin.x + self.offset.x,
            (p.y - self.origin.y) * self.scale.1 + self.origin.y + self.offset.y,
        ))
    }

    fn is_identity(&self) -> bool {
        self.scale == (1.0, 1.0) && self.offset.x.fract() == 0.0 && self.offset.y.fract() == 0.0
    }

    fn clip_uniforms(&self, src: Rectangle<f64, Buffer>, dst: Rectangle<i32, Physical>) -> Option<Vec<Uniform<'static>>> {
        let clip = self.clip?;
        let WaylandSurfaceTexture::Texture(tex) = self.inner.texture() else { return None };
        if dst.size.w <= 0 || dst.size.h <= 0 {
            return None;
        }
        let tex_m = texture_matrix(src, dst.size, tex.size(), self.inner.transform(), tex.is_y_inverted());
        let inv = tex_m.inverse()?;
        let to_geo = Mat3::translation((dst.loc.x as f64 - clip.geo.loc.x) as f32, (dst.loc.y as f64 - clip.geo.loc.y) as f32)
            .mul(&inv);
        Some(vec![
            Uniform::new("geo_size", (clip.geo.size.w as f32, clip.geo.size.h as f32)),
            Uniform::new("radius", clip.radius),
            to_geo.uniform("input_to_geo"),
        ])
    }
}

impl Element for WindowSurfaceElement {
    fn id(&self) -> &Id {
        self.alias.as_ref().unwrap_or_else(|| self.inner.id())
    }

    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> Transform {
        self.inner.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let g = self.inner.geometry(scale).to_f64();
        let a = self.map_point(g.loc);
        let b = self.map_point(Point::from((g.loc.x + g.size.w, g.loc.y + g.size.h)));
        let (x0, y0) = (a.x.round() as i32, a.y.round() as i32);
        let (x1, y1) = (b.x.round() as i32, b.y.round() as i32);
        Rectangle::new((x0, y0).into(), ((x1 - x0).max(0), (y1 - y0).max(0)).into())
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        let damage = self.inner.damage_since(scale, commit);
        if self.scale == (1.0, 1.0) {
            return damage;
        }
        // Inflated by a pixel: the scaled rectangle's sub-pixel origin is
        // rounded differently from the element's geometry.
        damage
            .into_iter()
            .map(|r| {
                let mut d = r.to_f64().upscale(Scale::from(self.scale)).to_i32_up();
                d.loc -= (1, 1).into();
                d.size += (2, 2).into();
                d
            })
            .collect()
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        // Rounded corners make the corners non-opaque; report opaque regions only
        // for unclipped, unscaled surfaces (e.g. maximized windows).
        match self.clip {
            Some(c) if c.radius > 0.0 => OpaqueRegions::default(),
            _ if self.scale != (1.0, 1.0) => OpaqueRegions::default(),
            _ => self.inner.opaque_regions(scale),
        }
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for WindowSurfaceElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        if let (WaylandSurfaceTexture::SolidColor(color), Some(clip), Some(prog)) = (self.inner.texture(), self.clip, &self.solid) {
            if clip.radius > 0.0 && dst.size.w > 0 && dst.size.h > 0 {
                // The window's rounded rectangle, relative to this element.
                let bx = [
                    (clip.geo.loc.x - dst.loc.x as f64) as f32,
                    (clip.geo.loc.y - dst.loc.y as f64) as f32,
                    clip.geo.size.w as f32,
                    clip.geo.size.h as f32,
                ];
                let c = [color.r(), color.g(), color.b(), color.a()];
                let uniforms = [
                    Uniform::new("fill", c),
                    Uniform::new("fill2", c),
                    Uniform::new("border", [0.0f32; 4]),
                    Uniform::new("border_width", 0.0f32),
                    Uniform::new("radius", clip.radius),
                    Uniform::new("box", bx),
                ];
                let size = Size::<i32, Buffer>::from((dst.size.w, dst.size.h));
                let src = Rectangle::<f64, Buffer>::from_size((dst.size.w as f64, dst.size.h as f64).into());
                return frame.render_pixel_shader_to(prog, src, dst, size, Some(damage), self.inner.alpha(), &uniforms);
            }
        }
        match self.clip_uniforms(src, dst) {
            Some(uniforms) => {
                frame.override_default_tex_program(self.program.clone(), uniforms);
                let r = RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions);
                frame.clear_tex_program_override();
                r
            }
            None => RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions),
        }
    }

    fn underlying_storage(&self, renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        if self.alias.is_none() && self.clip.is_none_or(|c| c.radius == 0.0) && self.is_identity() {
            self.inner.underlying_storage(renderer)
        } else {
            None
        }
    }
}

/// A pixel-shader shape (rounded rect, shadow, ...) in physical coordinates.
///
/// Keep one `Shape` per on-screen object and call [`Shape::set`] every frame:
/// the element id stays the same and the commit counter only advances when a
/// parameter really changed, so damage tracking redraws exactly what moved.
#[derive(Debug, Clone)]
pub struct Shape {
    id: Id,
    commit: CommitCounter,
    program: Option<GlesPixelProgram>,
    dst: Rectangle<i32, Physical>,
    alpha: f32,
    uniforms: Vec<Uniform<'static>>,
    key: Vec<u32>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            id: Id::new(),
            commit: CommitCounter::default(),
            program: None,
            dst: Rectangle::default(),
            alpha: 1.0,
            uniforms: Vec::new(),
            key: Vec::new(),
        }
    }
}

impl Shape {
    /// Update the shape; `key` must capture every value the uniforms depend on.
    pub fn set(
        &mut self,
        program: &GlesPixelProgram,
        dst: Rectangle<i32, Physical>,
        alpha: f32,
        key: &[f32],
        uniforms: impl FnOnce() -> Vec<Uniform<'static>>,
    ) -> &Shape {
        let key_bits: Vec<u32> = key.iter().map(|v| v.to_bits()).collect();
        if self.program.is_none() || self.dst != dst || self.alpha != alpha || self.key != key_bits {
            self.program = Some(program.clone());
            self.dst = dst;
            self.alpha = alpha;
            self.key = key_bits;
            self.uniforms = uniforms();
            self.commit.increment();
        }
        self
    }
}

/// Integer element rectangle enclosing a fractional box plus `pad` pixels, and
/// the box position relative to it (for the shader's `box` uniform).
pub fn enclose(r: Rectangle<f64, Physical>, pad: f64) -> (Rectangle<i32, Physical>, [f32; 4]) {
    let x0 = (r.loc.x - pad).floor();
    let y0 = (r.loc.y - pad).floor();
    let x1 = (r.loc.x + r.size.w + pad).ceil();
    let y1 = (r.loc.y + r.size.h + pad).ceil();
    let dst = Rectangle::new((x0 as i32, y0 as i32).into(), ((x1 - x0) as i32, (y1 - y0) as i32).into());
    (dst, [(r.loc.x - x0) as f32, (r.loc.y - y0) as f32, r.size.w as f32, r.size.h as f32])
}

impl Element for Shape {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((self.dst.size.w as f64, self.dst.size.h as f64).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for Shape {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let Some(program) = &self.program else { return Ok(()) };
        let size = Size::<i32, Buffer>::from((self.dst.size.w, self.dst.size.h));
        frame.render_pixel_shader_to(program, src, dst, size, Some(damage), self.alpha, &self.uniforms)
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

/// A texture drawn at exact physical coordinates: CPU-drawn images (labels,
/// icons, HUD) 1:1, and GPU textures (wallpaper, window snapshots) scaled to
/// any rectangle, optionally through a custom shader. Pixel-exact at
/// fractional scales, unlike smithay's logical-size texture elements.
pub struct Image {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    pub size: Size<i32, Physical>,
    last_key: Vec<u32>,
    /// Every pixel is opaque (wallpapers): lets damage tracking skip what
    /// is underneath.
    opaque: bool,
}

impl Image {
    /// Upload premultiplied RGBA (`w` × `h`).
    pub fn upload(r: &mut GlesRenderer, rgba: &[u8], w: u32, h: u32) -> Result<Image, GlesError> {
        use smithay::backend::renderer::ImportMem;
        let texture = r.import_memory(rgba, smithay::backend::allocator::Fourcc::Abgr8888, (w as i32, h as i32).into(), false)?;
        Ok(Image::from_texture(texture, (w as i32, h as i32).into()))
    }

    pub fn from_texture(texture: GlesTexture, size: Size<i32, Physical>) -> Image {
        Image { id: Id::new(), commit: CommitCounter::default(), texture, size, last_key: Vec::new(), opaque: false }
    }

    pub fn opaque(mut self) -> Image {
        self.opaque = true;
        self
    }

    /// Replace the pixels (same id: damage tracking sees one changing element).
    pub fn update(&mut self, r: &mut GlesRenderer, rgba: &[u8], w: u32, h: u32) -> Result<(), GlesError> {
        let fresh = Image::upload(r, rgba, w, h)?;
        self.texture = fresh.texture;
        self.size = fresh.size;
        self.commit.increment();
        Ok(())
    }

    /// Drawn 1:1 with its top-left at `loc`.
    pub fn element(&mut self, loc: Point<i32, Physical>, alpha: f32) -> ImageElement {
        let dst = Rectangle::new(loc, self.size);
        self.element_in(dst, alpha, None, &[])
    }

    /// Drawn scaled into `dst`, optionally with a texture `program` whose
    /// uniforms depend only on `key`.
    pub fn element_in(
        &mut self,
        dst: Rectangle<i32, Physical>,
        alpha: f32,
        program: Option<(GlesTexProgram, Vec<Uniform<'static>>)>,
        key: &[f32],
    ) -> ImageElement {
        let src = Rectangle::from_size((self.size.w as f64, self.size.h as f64).into());
        self.element_src(dst, src, alpha, program, key)
    }

    pub fn texture(&self) -> &GlesTexture {
        &self.texture
    }

    /// The id its elements carry.
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Replace the texture (new content, same element).
    pub fn set_texture(&mut self, texture: GlesTexture, size: Size<i32, Physical>) {
        self.texture = texture;
        self.size = size;
        self.commit.increment();
    }

    /// The part `src` (texture pixels) of the image drawn into `dst`.
    pub fn element_src(
        &mut self,
        dst: Rectangle<i32, Physical>,
        src: Rectangle<f64, Buffer>,
        alpha: f32,
        program: Option<(GlesTexProgram, Vec<Uniform<'static>>)>,
        key: &[f32],
    ) -> ImageElement {
        let k: Vec<u32> = key
            .iter()
            .chain(&[src.loc.x as f32, src.loc.y as f32, src.size.w as f32, src.size.h as f32])
            .map(|v| v.to_bits())
            .chain([alpha.to_bits()])
            .collect();
        if k != self.last_key {
            self.last_key = k;
            self.commit.increment();
        }
        let (program, uniforms) = match program {
            Some((p, u)) => (Some(p), u),
            None => (None, Vec::new()),
        };
        ImageElement {
            id: self.id.clone(),
            commit: self.commit,
            texture: self.texture.clone(),
            src,
            dst,
            alpha,
            program,
            uniforms,
            opaque: self.opaque && alpha >= 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageElement {
    id: Id,
    commit: CommitCounter,
    texture: GlesTexture,
    src: Rectangle<f64, Buffer>,
    dst: Rectangle<i32, Physical>,
    alpha: f32,
    program: Option<GlesTexProgram>,
    uniforms: Vec<Uniform<'static>>,
    opaque: bool,
}

impl Element for ImageElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        self.src
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.dst
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        if self.opaque {
            OpaqueRegions::from_slice(&[Rectangle::from_size(self.dst.size)])
        } else {
            OpaqueRegions::default()
        }
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for ImageElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(
            &self.texture,
            src,
            dst,
            damage,
            opaque_regions,
            Transform::Normal,
            self.alpha,
            self.program.as_ref(),
            &self.uniforms,
        )
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_roundtrip() {
        let m = Mat3::translation(3.0, -2.0).mul(&Mat3::scale(2.0, 0.5)).mul(&transform_matrix(Transform::_90));
        let inv = m.inverse().unwrap();
        let (x, y) = m.apply(1.5, 7.0);
        let (bx, by) = inv.apply(x, y);
        assert!((bx - 1.5).abs() < 1e-5 && (by - 7.0).abs() < 1e-5);
    }

    #[test]
    fn texture_matrix_normal_maps_corners() {
        // A 100x50 buffer drawn 1:1: dest pixel (100, 50) is texture (1, 1).
        let src = Rectangle::<f64, Buffer>::new((0.0, 0.0).into(), (100.0, 50.0).into());
        let m = texture_matrix(src, (100, 50).into(), (100, 50).into(), Transform::Normal, false);
        let (u, v) = m.apply(100.0, 50.0);
        assert!((u - 1.0).abs() < 1e-6 && (v - 1.0).abs() < 1e-6);
        // With a viewport crop of the right half, dest (0, 0) is texture u=0.5.
        let src = Rectangle::<f64, Buffer>::new((50.0, 0.0).into(), (50.0, 50.0).into());
        let m = texture_matrix(src, (100, 100).into(), (100, 50).into(), Transform::Normal, false);
        let (u, _) = m.apply(0.0, 0.0);
        assert!((u - 0.5).abs() < 1e-6);
        // y-inverted buffers flip v.
        let m = texture_matrix(
            Rectangle::new((0.0, 0.0).into(), (10.0, 10.0).into()),
            (10, 10).into(),
            (10, 10).into(),
            Transform::Normal,
            true,
        );
        let (_, v) = m.apply(0.0, 0.0);
        assert!((v - 0.0).abs() < 1e-6 || (v + 0.0).abs() < 1e-6);
    }
}
