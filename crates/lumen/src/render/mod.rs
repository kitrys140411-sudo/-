//! Scene composition: turns compositor state into a front-to-back list of
//! render elements for one output.

pub mod blur;
pub mod elements;
pub mod shaders;

use std::collections::HashMap;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement, WaylandSurfaceTexture};
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::element::{PixelShaderElement, TextureShaderElement};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::{Bind, Color32F, Frame, Offscreen, Renderer};
use smithay::desktop::PopupManager;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::with_states;

use self::elements::{enclose, Clip, Shape, WindowSurfaceElement};
use self::shaders::Shaders;
use crate::cursor::CursorTheme;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::wm::{Layout, Win, Wm, WORKSPACES};

smithay::backend::renderer::element::render_elements! {
    pub LumenElement<=GlesRenderer>;
    Window=WindowSurfaceElement,
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Pixel=PixelShaderElement,
    TexShader=TextureShaderElement,
    Shape=Shape,
    Texture=TextureRenderElement<GlesTexture>,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// Renderer-side state kept across frames.
#[derive(Default)]
pub struct RenderState {
    pub shaders: Option<Shaders>,
    /// Wallpaper (and its blurred copy) rendered once per output size.
    pub wallpaper: Option<(Size<i32, Physical>, TextureBuffer<GlesTexture>)>,
    pub wallpaper_blurred: Option<(Size<i32, Physical>, TextureBuffer<GlesTexture>)>,
    wallpaper_source: Option<GlesTexture>,
    /// What each window was last drawn with (texture references stay alive),
    /// used to snapshot a window for its closing animation.
    pub last_window: HashMap<u32, WindowSnapshot>,
    pub blur: blur::BlurCache,
    /// Element id aliases for surfaces drawn more than once per frame.
    pub aliases: HashMap<(Id, u8), Id>,
}

/// One surface of a window as last drawn: enough to redraw it without the
/// client (which may already be gone when the closing animation starts).
pub struct SnapPart {
    pub texture: GlesTexture,
    pub src: Rectangle<f64, smithay::utils::Buffer>,
    /// Relative to the window geometry's top-left corner.
    pub dst: Rectangle<i32, Physical>,
    pub transform: Transform,
}

pub struct WindowSnapshot {
    /// Front to back.
    pub parts: Vec<SnapPart>,
    pub size: Size<i32, Physical>,
}

/// Everything the scene builder reads, borrowed from the compositor state.
pub struct SceneCtx<'a> {
    pub wm: &'a mut Wm,
    pub stats: &'a crate::state::FrameStats,
    /// Scale of the output being built (set by `build_scene`).
    pub scale: f64,
    pub shell: &'a mut Shell,
    pub theme: &'a Theme,
    pub cursor_theme: &'a mut CursorTheme,
    pub cursor_status: &'a mut CursorImageStatus,
    pub pointer: Point<f64, Logical>,
    pub rs: &'a mut RenderState,
    pub now: std::time::Duration,
}

pub fn output_scale(output: &Output) -> f64 {
    output.current_scale().fractional_scale()
}

/// Output size in physical pixels (after transform).
pub fn output_physical_size(output: &Output) -> Size<i32, Physical> {
    let mode = output.current_mode().map(|m| m.size).unwrap_or_else(|| (1280, 800).into());
    output.current_transform().transform_size(mode)
}

pub fn output_logical_rect(output: &Output) -> Rectangle<f64, Logical> {
    let scale = output_scale(output);
    let size = output_physical_size(output).to_f64().to_logical(scale);
    Rectangle::new(output.current_location().to_f64(), size)
}

/// Render `elements` (front to back) into a new texture of `size`.
pub fn render_to_texture(
    r: &mut GlesRenderer,
    size: Size<i32, Physical>,
    elements: &[LumenElement],
    clear: Color32F,
) -> Result<GlesTexture, smithay::backend::renderer::gles::GlesError> {
    let mut tex: GlesTexture = r.create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal))?;
    {
        let mut fb = r.bind(&mut tex)?;
        let mut frame = r.render(&mut fb, size, Transform::Normal)?;
        let full = Rectangle::from_size(size);
        frame.clear(clear, &[full])?;
        for e in elements.iter().rev() {
            let geo = e.geometry(Scale::from(1.0));
            let src = e.src();
            if let Some(damage) = geo.intersection(full) {
                let mut local = damage;
                local.loc -= geo.loc;
                e.draw(&mut frame, src, geo, &[local], &[])?;
            }
        }
        let _ = frame.finish()?;
    }
    Ok(tex)
}

fn ensure_wallpaper(r: &mut GlesRenderer, rs: &mut RenderState, theme: &Theme, size: Size<i32, Physical>) {
    if rs.wallpaper.as_ref().is_some_and(|(s, _)| *s == size) {
        return;
    }
    let Some(shaders) = rs.shaders.as_ref() else { return };
    let c = &theme.colors;
    let area = Rectangle::new((0, 0).into(), (size.w, size.h).into());
    let elem = PixelShaderElement::new(
        shaders.wallpaper.clone(),
        area,
        None,
        1.0,
        vec![
            Uniform::new("top", c.wallpaper_top.to_array()),
            Uniform::new("bottom", c.wallpaper_bottom.to_array()),
            Uniform::new("glow_a", c.wallpaper_glow_a.to_array()),
            Uniform::new("glow_b", c.wallpaper_glow_b.to_array()),
        ],
        Kind::Unspecified,
    );
    let elems = [LumenElement::Pixel(elem)];
    match render_to_texture(r, size, &elems, Color32F::new(0.0, 0.0, 0.0, 1.0)) {
        Ok(tex) => {
            rs.wallpaper_source = Some(tex.clone());
            let buf = TextureBuffer::from_texture(r, tex, 1, Transform::Normal, Some(vec![Rectangle::from_size((size.w, size.h).into())]));
            rs.wallpaper = Some((size, buf));
            rs.wallpaper_blurred = None;
        }
        Err(e) => tracing::warn!("wallpaper rendering failed: {e:?}"),
    }
}

/// The blurred wallpaper (overview backdrop), made on first use: it is not
/// needed for the first frame, which should appear as early as possible.
fn ensure_wallpaper_blurred(r: &mut GlesRenderer, rs: &mut RenderState, size: Size<i32, Physical>) {
    if rs.wallpaper_blurred.as_ref().is_some_and(|(s, _)| *s == size) {
        return;
    }
    let (Some(shaders), Some(src)) = (rs.shaders.as_ref(), rs.wallpaper_source.as_ref()) else { return };
    let blurred = blur::new_target(r, size)
        .and_then(|mut dst| blur::blur_into(r, shaders, &mut rs.blur, src, size, 5, 4.0, &mut dst).map(|_| dst));
    match blurred {
        Ok(t) => {
            let opaque = Some(vec![Rectangle::from_size((size.w, size.h).into())]);
            rs.wallpaper_blurred = Some((size, TextureBuffer::from_texture(r, t, 1, Transform::Normal, opaque)));
        }
        Err(e) => tracing::warn!("wallpaper blur failed: {e:?}"),
    }
}

/// Window surface elements drawn so that the window geometry covers `f`
/// (physical, output-local). Unscaled windows snap to whole pixels so client
/// content stays crisp; scaled ones keep sub-pixel precision for smooth motion.
#[allow(clippy::too_many_arguments)]
fn push_window(
    r: &mut GlesRenderer,
    w: &Win,
    f: Rectangle<f64, Physical>,
    scale: f64,
    alpha: f32,
    radius: f32,
    program: &smithay::backend::renderer::gles::GlesTexProgram,
    mut aliases: Option<(&mut HashMap<(Id, u8), Id>, u8)>,
    out: &mut Vec<LumenElement>,
) -> Option<WindowSnapshot> {
    let geo = w.window.geometry();
    let geo_phys_w = (geo.size.w as f64 * scale).max(1.0);
    let total = f.size.w / geo_phys_w;
    let unscaled = (total - 1.0).abs() < 1e-6;
    let base = Point::<i32, Physical>::from((f.loc.x.round() as i32, f.loc.y.round() as i32));
    let render_loc = base - geo.loc.to_physical_precise_round(scale);
    let origin = base.to_f64();
    let (offset, clip_geo) = if unscaled {
        (Point::<f64, Physical>::from((0.0, 0.0)), Rectangle::new(origin, (geo_phys_w, geo.size.h as f64 * scale).into()))
    } else {
        (f.loc - origin, f)
    };
    let surface = w.surface().clone();
    let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(r, &surface, render_loc, scale, alpha, Kind::Unspecified);
    let clip = Some(Clip { geo: clip_geo, radius });
    let mut wrap = |e: WaylandSurfaceRenderElement<GlesRenderer>, clip: Option<Clip>| {
        let mut el = WindowSurfaceElement::new(e, origin, total, offset, clip, program.clone());
        if let Some((map, tag)) = aliases.as_mut() {
            let key = (el.inner_id().clone(), *tag);
            let id = map.entry(key).or_insert_with(Id::new).clone();
            el = el.with_alias(id);
        }
        LumenElement::Window(el)
    };
    // Popups above the window, unclipped.
    for (popup, popup_offset) in PopupManager::popups_for_surface(&surface) {
        let at = render_loc + (geo.loc + popup_offset - popup.geometry().loc).to_physical_precise_round(scale);
        let pe: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(r, popup.wl_surface(), at, scale, alpha, Kind::Unspecified);
        out.extend(pe.into_iter().map(|e| wrap(e, None)));
    }
    let snapshot = (unscaled && alpha >= 1.0).then(|| WindowSnapshot {
        parts: elems
            .iter()
            .filter_map(|e| {
                let WaylandSurfaceTexture::Texture(texture) = e.texture() else { return None };
                let mut dst = e.geometry(Scale::from(scale));
                dst.loc -= base;
                Some(SnapPart { texture: texture.clone(), src: e.src(), dst, transform: e.transform() })
            })
            .collect(),
        size: Size::from((geo_phys_w.round() as i32, (geo.size.h as f64 * scale).round() as i32)),
    });
    out.extend(elems.into_iter().map(|e| wrap(e, clip)));
    snapshot
}

/// A live thumbnail of window `id` covering `rect` (switcher).
pub fn thumbnail_elements(r: &mut GlesRenderer, ctx: &mut SceneCtx, id: u32, rect: Rectangle<f64, Physical>, radius: f64, alpha: f32, out: &mut Vec<LumenElement>) {
    let Some(w) = ctx.wm.get(id) else { return };
    let Some(shaders) = ctx.rs.shaders.as_ref() else { return };
    let program = shaders.rounded_surface.clone();
    let scale = ctx.scale;
    push_window(r, w, rect, scale, alpha, radius as f32, &program, Some((&mut ctx.rs.aliases, 1)), out);
}

/// Build the element list for `output`, front (topmost) first.
pub fn build_scene(r: &mut GlesRenderer, ctx: &mut SceneCtx, output: &Output) -> (Vec<LumenElement>, Color32F) {
    let scale = output_scale(output);
    ctx.scale = scale;
    let out_rect = output_logical_rect(output);
    let phys = output_physical_size(output);
    let mut elements: Vec<LumenElement> = Vec::new();
    let clear = ctx.theme.colors.background;
    let clear = Color32F::new(clear.r, clear.g, clear.b, 1.0);

    if ctx.rs.shaders.is_none() {
        match Shaders::compile(r) {
            Ok(s) => ctx.rs.shaders = Some(s),
            Err(e) => {
                tracing::error!("shader compilation failed: {e:?}");
                return (elements, clear);
            }
        }
    }
    ensure_wallpaper(r, ctx.rs, ctx.theme, phys);
    if ctx.rs.aliases.len() > 1024 {
        ctx.rs.aliases.clear();
    }

    // Cursor on top, then the shell UI.
    cursor_elements(r, ctx, out_rect, scale, &mut elements);
    crate::shell::render::overlay_elements(r, ctx, output, out_rect, scale, &mut elements);

    let shaders = ctx.rs.shaders.as_ref().unwrap();
    let rounded = shaders.rounded_surface.clone();
    let shadow_prog = shaders.shadow.clone();
    let rounded_tex = shaders.rounded_texture.clone();
    let overview = ctx.shell.overview_progress();
    let overview_layout = ctx.shell.overview_layout(ctx.wm, ctx.theme, out_rect);
    let hovered = if ctx.shell.overview_open { ctx.shell.hovered } else { None };
    let focused = ctx.wm.focused;
    let theme = ctx.theme;
    let g = &theme.geometry;

    // Windows, top to bottom.
    for i in (0..ctx.wm.windows.len()).rev() {
        let w = &ctx.wm.windows[i];
        if !w.mapped || w.ws >= WORKSPACES {
            continue;
        }
        let ws_dx = ctx.wm.ws_offset(w.ws) * out_rect.size.w;
        if ws_dx.abs() >= out_rect.size.w {
            continue;
        }
        let open = w.open.value.clamp(0.0, 1.2);
        let alpha = open.min(1.0) as f32;
        if alpha <= 0.0 {
            continue;
        }
        let geo = w.geo();
        // Unscaled rect U (logical, output-local); in the overview it moves
        // toward the window's slot.
        let mut u = Rectangle::<f64, Logical>::new((geo.loc.x + ws_dx - out_rect.loc.x, geo.loc.y - out_rect.loc.y).into(), geo.size);
        if let Some(slot) = overview_layout.as_ref().and_then(|l| l.get(&w.id)) {
            let t = overview;
            let k = (slot.size.w / geo.size.w.max(1.0)).min(slot.size.h / geo.size.h.max(1.0));
            let target = Rectangle::<f64, Logical>::new(
                (
                    slot.loc.x - out_rect.loc.x + (slot.size.w - geo.size.w * k) / 2.0,
                    slot.loc.y - out_rect.loc.y + (slot.size.h - geo.size.h * k) / 2.0,
                )
                    .into(),
                (geo.size.w * k, geo.size.h * k).into(),
            );
            u = Rectangle::new(
                (u.loc.x + (target.loc.x - u.loc.x) * t, u.loc.y + (target.loc.y - u.loc.y) * t).into(),
                (u.size.w + (target.size.w - u.size.w) * t, u.size.h + (target.size.h - u.size.h) * t).into(),
            );
        }
        // Open animation: grow from 94% about the center.
        let os = 0.94 + 0.06 * open;
        let f_log = Rectangle::<f64, Logical>::new(
            (u.loc.x + u.size.w * (1.0 - os) / 2.0, u.loc.y + u.size.h * (1.0 - os) / 2.0).into(),
            (u.size.w * os, u.size.h * os).into(),
        );
        let f = Rectangle::<f64, Physical>::new(
            (f_log.loc.x * scale, f_log.loc.y * scale).into(),
            (f_log.size.w * scale, f_log.size.h * scale).into(),
        );
        let total = f_log.size.w / geo.size.w.max(1.0);
        let square = matches!(w.layout, Layout::Maximized { .. } | Layout::Fullscreen { .. }) && overview <= 0.0;
        let radius = if square { 0.0 } else { (g.window_radius * scale * total) as f32 };

        if hovered == Some(w.id) {
            let pad = 4.0 * scale;
            let ring = Rectangle::<f64, Physical>::new(
                (f.loc.x - pad, f.loc.y - pad).into(),
                (f.size.w + pad * 2.0, f.size.h + pad * 2.0).into(),
            );
            let c = theme.colors.accent;
            let clear_c = c.with_alpha(0.0);
            let shape = crate::shell::render::rect_shape(
                &mut ctx.shell.shapes.hover_outline,
                &ctx.rs.shaders.as_ref().unwrap().rect,
                ring,
                radius as f64 + pad,
                clear_c,
                clear_c,
                Some((c, 3.0 * scale)),
                overview as f32,
            );
            elements.push(LumenElement::Shape(shape.clone()));
        }

        let w = &ctx.wm.windows[i];
        let snapshot = push_window(r, w, f, scale, alpha, radius, &rounded, None, &mut elements);
        if let Some(snap) = snapshot {
            if overview <= 0.0 && open >= 1.0 {
                ctx.rs.last_window.insert(w.id, snap);
            }
        }

        // Shadow under the window.
        if !matches!(w.layout, Layout::Fullscreen { .. }) {
            let sigma = g.shadow_blur * scale * total.max(0.3) / 2.0;
            let dy = g.shadow_offset_y * scale * total;
            let strength = if Some(w.id) == focused { g.shadow_alpha_focused } else { g.shadow_alpha } as f32;
            let sbox = Rectangle::<f64, Physical>::new((f.loc.x, f.loc.y + dy).into(), f.size);
            let (dst, bx) = enclose(sbox, sigma * 3.0);
            let cut = [(f.loc.x - dst.loc.x as f64) as f32, (f.loc.y - dst.loc.y as f64) as f32, f.size.w as f32, f.size.h as f32];
            let sc = theme.colors.shadow;
            let color = [sc.r * strength, sc.g * strength, sc.b * strength, strength];
            let key = [bx[0], bx[1], bx[2], bx[3], cut[0], cut[1], radius, sigma as f32, color[3]];
            let w = &mut ctx.wm.windows[i];
            let shape = w.shadow.set(&shadow_prog, dst, alpha, &key, || {
                vec![
                    Uniform::new("color", color),
                    Uniform::new("box", bx),
                    Uniform::new("radius", radius),
                    Uniform::new("sigma", sigma as f32),
                    Uniform::new("cutout", cut),
                ]
            });
            elements.push(LumenElement::Shape(shape.clone()));
        }
    }

    // Windows that are closing, drawn from their snapshots.
    for c in &ctx.wm.closing {
        let ws_dx = ctx.wm.ws_offset(c.ws) * out_rect.size.w;
        let p = c.progress.value.clamp(0.0, 1.0);
        let s = 0.92 + 0.08 * p;
        let gc = c.geo;
        let cx = gc.loc.x + ws_dx - out_rect.loc.x + gc.size.w / 2.0;
        let cy = gc.loc.y - out_rect.loc.y + gc.size.h / 2.0;
        let (w, h) = (gc.size.w * s, gc.size.h * s);
        let loc = Point::<f64, Logical>::from((cx - w / 2.0, cy - h / 2.0)).to_physical(scale);
        let buffer = TextureBuffer::from_texture(r, c.texture.clone(), 1, Transform::Normal, None);
        let size = Size::<i32, Logical>::from(((w * scale).round() as i32, (h * scale).round() as i32));
        let tex = TextureRenderElement::from_texture_buffer(loc, &buffer, Some(p as f32), None, Some(size), Kind::Unspecified);
        let radius = (g.window_radius * scale * s) as f32;
        let elem = TextureShaderElement::new(
            tex,
            rounded_tex.clone(),
            vec![Uniform::new("px_size", ((w * scale) as f32, (h * scale) as f32)), Uniform::new("radius", radius)],
        );
        elements.push(LumenElement::TexShader(elem));
    }

    // Overview backdrop: dim layer and the blurred wallpaper fading in.
    if overview > 0.0 {
        ensure_wallpaper_blurred(r, ctx.rs, phys);
        let full = Rectangle::<f64, Physical>::from_size((phys.w as f64, phys.h as f64).into());
        let c = theme.colors.overview_dim;
        let shape = crate::shell::render::rect_shape(
            &mut ctx.shell.shapes.overview_dim,
            &ctx.rs.shaders.as_ref().unwrap().rect,
            full,
            0.0,
            c,
            c,
            None,
            overview as f32,
        );
        elements.push(LumenElement::Shape(shape.clone()));
        if let Some((_, buf)) = &ctx.rs.wallpaper_blurred {
            elements.push(LumenElement::Texture(TextureRenderElement::from_texture_buffer(
                (0.0, 0.0),
                buf,
                Some(overview as f32),
                None,
                None,
                Kind::Unspecified,
            )));
        }
    }
    if let Some((_, buf)) = &ctx.rs.wallpaper {
        elements.push(LumenElement::Texture(TextureRenderElement::from_texture_buffer((0.0, 0.0), buf, None, None, None, Kind::Unspecified)));
    }
    (elements, clear)
}

fn cursor_elements(r: &mut GlesRenderer, ctx: &mut SceneCtx, out_rect: Rectangle<f64, Logical>, scale: f64, elements: &mut Vec<LumenElement>) {
    if !out_rect.contains(ctx.pointer) {
        return;
    }
    let local = ctx.pointer - out_rect.loc;
    if let CursorImageStatus::Surface(surface) = &*ctx.cursor_status {
        if !surface.alive() {
            *ctx.cursor_status = CursorImageStatus::default_named();
        }
    }
    match &*ctx.cursor_status {
        CursorImageStatus::Hidden => {}
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<smithay::input::pointer::CursorImageSurfaceData>()
                    .map(|d| d.lock().unwrap().hotspot)
                    .unwrap_or_default()
            });
            let at: Point<i32, Physical> = (local - hotspot.to_f64()).to_physical_precise_round(scale);
            let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(r, surface, at, scale, 1.0, Kind::Cursor);
            elements.extend(elems.into_iter().map(LumenElement::Surface));
        }
        CursorImageStatus::Named(icon) => {
            let int_scale = scale.ceil() as i32;
            let frame = ctx.cursor_theme.frame(icon.name(), int_scale, ctx.now);
            let at: Point<i32, Physical> = (local - frame.hotspot.to_f64()).to_physical_precise_round(scale);
            if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(r, at.to_f64(), &frame.buffer, None, None, None, Kind::Cursor) {
                elements.push(LumenElement::Memory(e));
            }
        }
    }
}

/// Render a window's last frame into a texture of its geometry size.
pub fn snapshot_window(r: &mut GlesRenderer, snap: &WindowSnapshot) -> Option<GlesTexture> {
    let size = snap.size;
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let mut tex: GlesTexture = r.create_buffer(Fourcc::Abgr8888, size.to_logical(1).to_buffer(1, Transform::Normal)).ok()?;
    {
        let mut fb = r.bind(&mut tex).ok()?;
        let mut frame = r.render(&mut fb, size, Transform::Normal).ok()?;
        let full = Rectangle::from_size(size);
        frame.clear(Color32F::TRANSPARENT, &[full]).ok()?;
        for p in snap.parts.iter().rev() {
            let Some(visible) = p.dst.intersection(full) else { continue };
            let mut local = visible;
            local.loc -= p.dst.loc;
            frame.render_texture_from_to(&p.texture, p.src, p.dst, &[local], &[], p.transform, 1.0, None, &[]).ok()?;
        }
        let _ = frame.finish().ok()?;
    }
    Some(tex)
}

impl crate::state::Lumen {
    /// Start the closing animation for a window (snapshot of its last frame).
    pub fn close_animation(&mut self, id: u32) {
        let Some(w) = self.wm.get(id) else { return };
        let geo = w.geo();
        let ws = w.ws;
        let Some(snap) = self.render.last_window.remove(&id) else { return };
        if let Some(tex) = self.backend.with_renderer(|r| snapshot_window(r, &snap)).flatten() {
            self.wm.start_closing(tex, geo, ws);
        }
    }
}
