//! Frosted glass behind shell surfaces (panel, dock, menus, notifications,
//! the window switcher): what is behind a pane is rendered offscreen, blurred
//! (dual Kawase over just that region) and drawn under the pane, saturated
//! and tinted.
//!
//! A pane only re-renders when something behind it changed: the ids,
//! commits, positions and opacities of the elements it covers are hashed
//! every frame and compared with the last blur.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use smithay::backend::renderer::element::{Element, RenderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, Color32F, Frame, Renderer};
use smithay::utils::{Buffer, Physical, Rectangle, Scale, Size, Transform};

use super::blur::{self, BlurCache};
use super::elements::Image;
use super::shaders::Shaders;
use super::LumenElement;
use crate::theme::{Color, Theme};

/// A glass pane requested by the shell while building a frame.
pub struct GlassRequest {
    /// Index of the pane's own (border) shape in the element list: the glass
    /// goes right behind it, everything after it is what it frosts.
    pub index: usize,
    pub rect: Rectangle<f64, Physical>,
    pub radius: f32,
    pub tint: Color,
    pub alpha: f32,
    pub key: (&'static str, u64),
}

struct Pane {
    image: Image,
    /// The blurred texture covers `region` (the pane plus a margin, so the
    /// blur near its edges sees real content).
    region: Rectangle<i32, Physical>,
    signature: u64,
    used: bool,
}

#[derive(Default)]
pub struct GlassCache {
    /// Output-sized scratch texture that the content behind panes is drawn into.
    capture: HashMap<(i32, i32), GlesTexture>,
    panes: HashMap<(String, &'static str, u64), Pane>,
    blur: BlurCache,
}

fn commit_value(c: CommitCounter) -> usize {
    c.distance(Some(CommitCounter::default())).unwrap_or(0)
}

/// What a region shows, as a hash of the elements covering it.
fn hash_rect(r: Rectangle<i32, Physical>, h: &mut impl Hasher) {
    (r.loc.x, r.loc.y, r.size.w, r.size.h).hash(h);
}

fn signature(elements: &[LumenElement], region: Rectangle<i32, Physical>, scale: f64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_rect(region, &mut h);
    for e in elements {
        let geo = e.geometry(Scale::from(scale));
        if !geo.overlaps(region) {
            continue;
        }
        e.id().hash(&mut h);
        commit_value(e.current_commit()).hash(&mut h);
        hash_rect(geo, &mut h);
        e.alpha().to_bits().hash(&mut h);
    }
    h.finish()
}

/// Draw `elements` (front to back) into `target`, only inside `region`.
fn capture(
    r: &mut GlesRenderer,
    target: &mut GlesTexture,
    size: Size<i32, Physical>,
    region: Rectangle<i32, Physical>,
    elements: &[LumenElement],
    clear: Color32F,
    scale: f64,
) -> Result<(), smithay::backend::renderer::gles::GlesError> {
    let mut fb = r.bind(target)?;
    let mut frame = r.render(&mut fb, size, Transform::Normal)?;
    frame.clear(clear, &[region])?;
    for e in elements.iter().rev() {
        let geo = e.geometry(Scale::from(scale));
        let Some(visible) = geo.intersection(region) else { continue };
        let mut local = visible;
        local.loc -= geo.loc;
        e.draw(&mut frame, e.src(), geo, &[local], &[])?;
    }
    let _ = frame.finish()?;
    Ok(())
}

impl GlassCache {
    /// Insert a glass element behind every requested pane.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        r: &mut GlesRenderer,
        shaders: &Shaders,
        theme: &Theme,
        output: &str,
        size: Size<i32, Physical>,
        scale: f64,
        mut requests: Vec<GlassRequest>,
        elements: &mut Vec<LumenElement>,
        clear: Color32F,
    ) {
        for ((o, _, _), p) in self.panes.iter_mut() {
            if o == output {
                p.used = false;
            }
        }
        // Back to front: a pane in front of another frosts the other's glass too.
        requests.sort_by(|a, b| b.index.cmp(&a.index));
        let b = &theme.blur;
        let margin = (b.offset * (1u32 << b.passes.clamp(1, 6)) as f32).ceil() as i32;
        for req in requests {
            if req.alpha <= 0.0 || req.index >= elements.len() {
                continue;
            }
            let full = Rectangle::<i32, Physical>::from_size(size);
            let x0 = req.rect.loc.x.floor() as i32;
            let y0 = req.rect.loc.y.floor() as i32;
            let x1 = (req.rect.loc.x + req.rect.size.w).ceil() as i32;
            let y1 = (req.rect.loc.y + req.rect.size.h).ceil() as i32;
            let dst = Rectangle::<i32, Physical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into());
            // Not clipped to the output (sampling clamps at its edges), so the
            // region keeps its size while a pane slides in or out and its
            // textures are reused.
            let region = Rectangle::<i32, Physical>::new((x0 - margin, y0 - margin).into(), (x1 - x0 + 2 * margin, y1 - y0 + 2 * margin).into());
            if dst.size.w <= 0 || dst.size.h <= 0 || !region.overlaps(full) {
                continue;
            }
            let behind = &elements[req.index + 1..];
            let sig = signature(behind, region, scale);
            let key = (output.to_string(), req.key.0, req.key.1);
            let fresh = self.panes.get(&key).is_none_or(|p| p.signature != sig || p.region != region);
            if fresh {
                let blurred = (|| -> Result<GlesTexture, smithay::backend::renderer::gles::GlesError> {
                    let mut cap = match self.capture.get(&(size.w, size.h)) {
                        Some(t) => t.clone(),
                        None => {
                            let t = blur::new_target(r, size)?;
                            self.capture.insert((size.w, size.h), t.clone());
                            t
                        }
                    };
                    // A few pixels more than the blur reads, so it never sees stale texels.
                    let pad = Rectangle::<i32, Physical>::new(
                        (region.loc.x - 4, region.loc.y - 4).into(),
                        (region.size.w + 8, region.size.h + 8).into(),
                    )
                        .intersection(full)
                        .unwrap_or(region);
                    capture(r, &mut cap, size, pad, behind, clear, scale)?;
                    // Reuse the pane's texture when its size is unchanged.
                    let mut out = match self.panes.get(&key).filter(|p| p.region.size == region.size) {
                        Some(p) => p.image.texture().clone(),
                        None => blur::new_target(r, region.size)?,
                    };
                    blur::blur_region(r, shaders, &mut self.blur, &cap, size, region, b.passes, b.offset, &mut out)?;
                    Ok(out)
                })();
                match blurred {
                    Ok(tex) => match self.panes.get_mut(&key) {
                        Some(p) => {
                            p.image.set_texture(tex, region.size);
                            p.region = region;
                            p.signature = sig;
                        }
                        None => {
                            self.panes.insert(
                                key.clone(),
                                Pane { image: Image::from_texture(tex, region.size), region, signature: sig, used: true },
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!("glass blur failed: {e:?}");
                        continue;
                    }
                }
            }
            let Some(p) = self.panes.get_mut(&key) else { continue };
            p.used = true;
            let src = Rectangle::<f64, Buffer>::new(
                ((dst.loc.x - region.loc.x) as f64, (dst.loc.y - region.loc.y) as f64).into(),
                (dst.size.w as f64, dst.size.h as f64).into(),
            );
            let px = (dst.size.w as f32, dst.size.h as f32);
            let tint = req.tint.premultiplied();
            let uniforms = vec![
                Uniform::new("px_size", px),
                Uniform::new("radius", req.radius),
                Uniform::new("tint_color", tint),
                Uniform::new("saturation", b.saturation),
                Uniform::new("highlight", 0.06f32),
                Uniform::new(
                    "uv_rect",
                    [
                        src.loc.x as f32 / region.size.w as f32,
                        src.loc.y as f32 / region.size.h as f32,
                        src.size.w as f32 / region.size.w as f32,
                        src.size.h as f32 / region.size.h as f32,
                    ],
                ),
            ];
            let key_vals = [px.0, px.1, req.radius, tint[0], tint[1], tint[2], tint[3], region.size.w as f32, region.size.h as f32];
            let el = p.image.element_src(dst, src, req.alpha, Some((shaders.glass.clone(), uniforms)), &key_vals);
            elements.insert(req.index + 1, LumenElement::Image(el));
        }
        self.panes.retain(|(o, _, _), p| o != output || p.used);
        self.blur.trim(64);
    }
}
