//! Dual-Kawase blur: N downsample passes (each halves the resolution) followed by
//! N upsample passes. Cost is roughly 1.33x a single full-resolution copy no
//! matter how strong the blur, which keeps glass effects cheap.

use std::collections::HashMap;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::{GlesError, GlesRenderer, GlesTexProgram, GlesTexture, Uniform};
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};

use super::shaders::Shaders;

/// Intermediate textures, reused across frames (keyed by role and size).
#[derive(Default)]
pub struct BlurCache {
    textures: HashMap<(u8, u32, i32, i32), GlesTexture>,
}

impl BlurCache {
    fn get(&mut self, r: &mut GlesRenderer, role: u8, level: u32, size: Size<i32, Physical>) -> Result<GlesTexture, GlesError> {
        let key = (role, level, size.w, size.h);
        if let Some(t) = self.textures.get(&key) {
            return Ok(t.clone());
        }
        let t = new_target(r, size)?;
        self.textures.insert(key, t.clone());
        Ok(t)
    }

    /// Drop textures of sizes no longer in use (after output mode changes).
    pub fn clear(&mut self) {
        self.textures.clear();
    }

    /// Keep the cache bounded (glass panes of changing sizes add entries).
    pub fn trim(&mut self, max: usize) {
        if self.textures.len() > max {
            self.textures.clear();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn pass(
    r: &mut GlesRenderer,
    src: &GlesTexture,
    src_size: Size<i32, Physical>,
    src_rect: Option<Rectangle<i32, Physical>>,
    dst: &mut GlesTexture,
    dst_size: Size<i32, Physical>,
    program: &GlesTexProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let mut fb = r.bind(dst)?;
    let mut frame = r.render(&mut fb, dst_size, Transform::Normal)?;
    let full = Rectangle::from_size(dst_size);
    let src_rect = match src_rect {
        Some(s) => Rectangle::<f64, Buffer>::new((s.loc.x as f64, s.loc.y as f64).into(), (s.size.w as f64, s.size.h as f64).into()),
        None => Rectangle::<f64, Buffer>::from_size((src_size.w as f64, src_size.h as f64).into()),
    };
    frame.clear(Color32F::TRANSPARENT, &[full])?;
    frame.render_texture_from_to(
        src,
        src_rect,
        full,
        &[full],
        &[],
        Transform::Normal,
        1.0,
        Some(program),
        &[
            Uniform::new("half_pixel", (0.5 / src_size.w as f32, 0.5 / src_size.h as f32)),
            Uniform::new("offset", offset),
        ],
    )?;
    let _ = frame.finish()?;
    Ok(())
}

/// Blur `src` (of `size`) into `dst` (same size). Intermediate textures come
/// from `cache`; `dst` is owned by the caller so results survive later blurs.
#[allow(clippy::too_many_arguments)]
pub fn blur_into(
    r: &mut GlesRenderer,
    shaders: &Shaders,
    cache: &mut BlurCache,
    src: &GlesTexture,
    size: Size<i32, Physical>,
    passes: u32,
    offset: f32,
    dst: &mut GlesTexture,
) -> Result<(), GlesError> {
    blur_region(r, shaders, cache, src, size, Rectangle::from_size(size), passes, offset, dst)
}

/// Blur the `region` of `src` (a texture of `src_size`) into `dst`, which
/// has the region's size.
#[allow(clippy::too_many_arguments)]
pub fn blur_region(
    r: &mut GlesRenderer,
    shaders: &Shaders,
    cache: &mut BlurCache,
    src: &GlesTexture,
    src_size: Size<i32, Physical>,
    region: Rectangle<i32, Physical>,
    passes: u32,
    offset: f32,
    dst: &mut GlesTexture,
) -> Result<(), GlesError> {
    let passes = passes.clamp(1, 6);
    let (down, up) = (shaders.kawase_down.get(r)?, shaders.kawase_up.get(r)?);
    let size = region.size;
    let mut sizes = vec![size];
    let mut cur = src.clone();
    let mut cur_size = src_size;
    let mut cur_rect = Some(region);
    for level in 1..=passes {
        let base = sizes[level as usize - 1];
        let ns = Size::from(((base.w / 2).max(1), (base.h / 2).max(1)));
        let mut t = cache.get(r, 0, level, ns)?;
        pass(r, &cur, cur_size, cur_rect.take(), &mut t, ns, down, offset)?;
        sizes.push(ns);
        cur = t;
        cur_size = ns;
    }
    for level in (0..passes).rev() {
        let ts = sizes[level as usize];
        if level == 0 {
            pass(r, &cur, cur_size, None, dst, ts, up, offset)?;
            break;
        }
        let mut t = cache.get(r, 1, level, ts)?;
        pass(r, &cur, cur_size, None, &mut t, ts, up, offset)?;
        cur = t;
        cur_size = ts;
    }
    Ok(())
}

/// Allocate an offscreen texture (blur targets, wallpaper, snapshots) that
/// samples with clamped edges.
pub fn new_target(r: &mut GlesRenderer, size: Size<i32, Physical>) -> Result<GlesTexture, GlesError> {
    let t: GlesTexture = r.create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))?;
    clamp_to_edge(r, &t);
    Ok(t)
}

/// Offscreen textures keep GL's default REPEAT wrapping (smithay sets
/// CLAMP_TO_EDGE only on imported textures); blur taps and bilinear edge
/// samples would then wrap around to the opposite edge.
pub fn clamp_to_edge(r: &mut GlesRenderer, t: &GlesTexture) {
    use smithay::backend::renderer::gles::ffi;
    let id = t.tex_id();
    let _ = r.with_context(|gl| unsafe {
        gl.BindTexture(ffi::TEXTURE_2D, id);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE as i32);
        gl.TexParameteri(ffi::TEXTURE_2D, ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE as i32);
        gl.BindTexture(ffi::TEXTURE_2D, 0);
    });
}
