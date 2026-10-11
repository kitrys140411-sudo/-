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
        let t: GlesTexture = r.create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))?;
        self.textures.insert(key, t.clone());
        Ok(t)
    }

    /// Drop textures of sizes no longer in use (after output mode changes).
    pub fn clear(&mut self) {
        self.textures.clear();
    }
}

fn pass(
    r: &mut GlesRenderer,
    src: &GlesTexture,
    src_size: Size<i32, Physical>,
    dst: &mut GlesTexture,
    dst_size: Size<i32, Physical>,
    program: &GlesTexProgram,
    offset: f32,
) -> Result<(), GlesError> {
    let mut fb = r.bind(dst)?;
    let mut frame = r.render(&mut fb, dst_size, Transform::Normal)?;
    let full = Rectangle::from_size(dst_size);
    let src_rect = Rectangle::<f64, Buffer>::from_size((src_size.w as f64, src_size.h as f64).into());
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
    let passes = passes.clamp(1, 6);
    let mut sizes = vec![size];
    let mut cur = src.clone();
    let mut cur_size = size;
    for level in 1..=passes {
        let ns = Size::from(((cur_size.w / 2).max(1), (cur_size.h / 2).max(1)));
        let mut t = cache.get(r, 0, level, ns)?;
        pass(r, &cur, cur_size, &mut t, ns, &shaders.kawase_down, offset)?;
        sizes.push(ns);
        cur = t;
        cur_size = ns;
    }
    for level in (0..passes).rev() {
        let ts = sizes[level as usize];
        if level == 0 {
            pass(r, &cur, cur_size, dst, ts, &shaders.kawase_up, offset)?;
            break;
        }
        let mut t = cache.get(r, 1, level, ts)?;
        pass(r, &cur, cur_size, &mut t, ts, &shaders.kawase_up, offset)?;
        cur = t;
        cur_size = ts;
    }
    Ok(())
}

/// Allocate a texture suitable as a blur destination.
pub fn new_target(r: &mut GlesRenderer, size: Size<i32, Physical>) -> Result<GlesTexture, GlesError> {
    r.create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))
}
