//! Scene composition: turns compositor state into a front-to-back list of
//! render elements for one output.

pub mod blur;
pub mod elements;
pub mod shaders;

use std::collections::HashMap;

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement, WaylandSurfaceTexture};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::gles::element::PixelShaderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture, Uniform};
use smithay::backend::renderer::{Bind, Color32F, Frame, Renderer};
use smithay::desktop::PopupManager;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::with_states;

use self::elements::{enclose, Clip, Image, ImageElement, Shape, WindowSurfaceElement};
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
    Shape=Shape,
    Image=ImageElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
}

/// The wallpaper for one output size, and its blurred copy (overview
/// backdrop) made on first use.
pub struct Wallpaper {
    source: GlesTexture,
    image: Image,
    blurred: Option<Image>,
}

/// Renderer-side state kept across frames.
pub struct RenderState {
    pub shaders: Option<Shaders>,
    /// Wallpapers by output size (physical).
    pub wallpapers: HashMap<(i32, i32), Wallpaper>,
    /// What each window was last drawn with (texture references stay alive),
    /// used to snapshot a window for its closing animation.
    pub last_window: HashMap<u32, WindowSnapshot>,
    pub blur: blur::BlurCache,
    /// Element id aliases for surfaces drawn more than once per frame.
    pub aliases: HashMap<(Id, u8), Id>,
    pub text: crate::text::UiText,
    pub icons: crate::icons::IconCache,
}

impl RenderState {
    pub fn new(theme: &Theme) -> RenderState {
        RenderState {
            shaders: None,
            wallpapers: HashMap::new(),
            last_window: HashMap::new(),
            blur: blur::BlurCache::default(),
            aliases: HashMap::new(),
            text: crate::text::UiText::new(&theme.fonts.ui),
            icons: crate::icons::IconCache::default(),
        }
    }
}

/// One surface of a window as last drawn: enough to redraw it without the
/// client (which may already be gone when the closing animation starts).
/// Destinations are relative to the window geometry's top-left corner.
pub enum SnapPart {
    Texture { texture: GlesTexture, src: Rectangle<f64, smithay::utils::Buffer>, dst: Rectangle<i32, Physical>, transform: Transform },
    Solid { color: Color32F, dst: Rectangle<i32, Physical> },
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
    pub dnd_icon: Option<&'a WlSurface>,
    /// Name of the primary output: the shell UI (panel, dock, overview
    /// controls) lives there, other outputs show windows only.
    pub primary: Option<String>,
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
    let mut tex = blur::new_target(r, size)?;
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
    if rs.wallpapers.contains_key(&(size.w, size.h)) {
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
            // Outputs come and go rarely; keep the cache from growing anyway.
            if rs.wallpapers.len() >= 4 {
                rs.wallpapers.clear();
            }
            let image = Image::from_texture(tex.clone(), size).opaque();
            rs.wallpapers.insert((size.w, size.h), Wallpaper { source: tex, image, blurred: None });
        }
        Err(e) => tracing::warn!("wallpaper rendering failed: {e:?}"),
    }
}

/// The blurred wallpaper (overview backdrop), made on first use: it is not
/// needed for the first frame, which should appear as early as possible.
fn ensure_wallpaper_blurred(r: &mut GlesRenderer, rs: &mut RenderState, size: Size<i32, Physical>) {
    let RenderState { shaders, wallpapers, blur: cache, .. } = rs;
    let (Some(shaders), Some(wp)) = (shaders.as_ref(), wallpapers.get_mut(&(size.w, size.h))) else { return };
    if wp.blurred.is_some() {
        return;
    }
    let blurred =
        blur::new_target(r, size).and_then(|mut dst| blur::blur_into(r, shaders, cache, &wp.source, size, 5, 4.0, &mut dst).map(|_| dst));
    match blurred {
        Ok(t) => wp.blurred = Some(Image::from_texture(t, size).opaque()),
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
    shaders: &Shaders,
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
        let mut el = WindowSurfaceElement::new(e, origin, total, offset, clip, shaders.rounded_surface.clone())
            .with_solid_program(shaders.rect.clone());
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
                let mut dst = e.geometry(Scale::from(scale));
                dst.loc -= base;
                match e.texture() {
                    WaylandSurfaceTexture::Texture(texture) => {
                        Some(SnapPart::Texture { texture: texture.clone(), src: e.src(), dst, transform: e.transform() })
                    }
                    WaylandSurfaceTexture::SolidColor(color) => Some(SnapPart::Solid { color: *color, dst }),
                }
            })
            .collect(),
        size: Size::from((geo_phys_w.round() as i32, (geo.size.h as f64 * scale).round() as i32)),
    });
    out.extend(elems.into_iter().map(|e| wrap(e, clip)));
    snapshot
}

/// A live thumbnail of window `id` covering `rect` (switcher, workspace
/// strip). `tag` keeps the element ids of each kind of thumbnail apart.
#[allow(clippy::too_many_arguments)]
pub fn thumbnail_elements(
    r: &mut GlesRenderer,
    ctx: &mut SceneCtx,
    id: u32,
    rect: Rectangle<f64, Physical>,
    radius: f64,
    alpha: f32,
    tag: u8,
    out: &mut Vec<LumenElement>,
) {
    let Some(w) = ctx.wm.get(id) else { return };
    let Some(shaders) = ctx.rs.shaders.as_ref() else { return };
    let scale = ctx.scale;
    push_window(r, w, rect, scale, alpha, radius as f32, shaders, Some((&mut ctx.rs.aliases, tag)), out);
}

fn lerp_rect(a: Rectangle<f64, Logical>, b: Rectangle<f64, Logical>, t: f64) -> Rectangle<f64, Logical> {
    Rectangle::new(
        (a.loc.x + (b.loc.x - a.loc.x) * t, a.loc.y + (b.loc.y - a.loc.y) * t).into(),
        (a.size.w + (b.size.w - a.size.w) * t, a.size.h + (b.size.h - a.size.h) * t).into(),
    )
}

/// A closing window's ghost: its snapshot shrinking and fading out where it
/// was last drawn.
fn ghost_element(
    c: &mut crate::wm::Closing,
    ws_dx: f64,
    out_rect: Rectangle<f64, Logical>,
    scale: f64,
    window_radius: f64,
    program: &smithay::backend::renderer::gles::GlesTexProgram,
) -> Option<LumenElement> {
    let p = c.progress.value.clamp(0.0, 1.0);
    let s = 0.92 + 0.08 * p;
    let gc = c.rect;
    let cx = gc.loc.x + ws_dx - out_rect.loc.x + gc.size.w / 2.0;
    let cy = gc.loc.y - out_rect.loc.y + gc.size.h / 2.0;
    let (w, h) = (gc.size.w * s, gc.size.h * s);
    let x0 = ((cx - w / 2.0) * scale).round() as i32;
    let y0 = ((cy - h / 2.0) * scale).round() as i32;
    let x1 = ((cx + w / 2.0) * scale).round() as i32;
    let y1 = ((cy + h / 2.0) * scale).round() as i32;
    if x1 <= x0 || y1 <= y0 || p <= 0.0 {
        return None;
    }
    let dst = Rectangle::<i32, Physical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into());
    // The snapshot is at 100% size; the ghost may have been in the overview.
    let total = (x1 - x0) as f64 / c.image.size.w.max(1) as f64;
    let radius = (window_radius * scale * total) as f32;
    let px = ((x1 - x0) as f32, (y1 - y0) as f32);
    let uniforms = vec![Uniform::new("px_size", px), Uniform::new("radius", radius)];
    Some(LumenElement::Image(c.image.element_in(dst, p as f32, Some((program.clone(), uniforms)), &[px.0, px.1, radius])))
}

/// Build the element list for `output`, front (topmost) first.
pub fn build_scene(r: &mut GlesRenderer, ctx: &mut SceneCtx, output: &Output) -> (Vec<LumenElement>, Color32F) {
    let scale = output_scale(output);
    ctx.scale = scale;
    let out_rect = output_logical_rect(output);
    let phys = output_physical_size(output);
    let out_name = output.name();
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

    // Cursor (and a drag icon) on top, then the shell UI.
    cursor_elements(r, ctx, out_rect, scale, &mut elements);
    if ctx.primary.as_deref().is_none_or(|p| p == out_name) {
        crate::shell::render::overlay_elements(r, ctx, output, out_rect, scale, &mut elements);
    }

    let overview = ctx.shell.overview_progress();
    // Overview slots of every workspace on screen (several while sliding).
    let mut slots: HashMap<u32, Rectangle<f64, Logical>> = HashMap::new();
    for ws in 0..WORKSPACES {
        if ctx.wm.ws_visible(ws) {
            if let Some(l) = ctx.shell.overview_layout_ws(ctx.wm, ctx.theme, out_rect, ws) {
                slots.extend(l);
            }
        }
    }
    let hovered = if ctx.shell.overview_open { ctx.shell.hovered } else { None };
    let focused = ctx.wm.focused;
    let theme = ctx.theme;
    let g = &theme.geometry;
    let mut ghost_done = vec![false; ctx.wm.closing.len()];
    let ghost_dx: Vec<f64> = ctx.wm.closing.iter().map(|c| ctx.wm.ws_offset(c.ws) * out_rect.size.w).collect();

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
        let alpha = (open.min(1.0) * ctx.shell.window_visibility(ctx.wm, w.id)) as f32;
        if alpha <= 0.0 {
            continue;
        }
        // Ghosts of closed windows that were stacked above this one.
        let (wid, wws) = (w.id, w.ws);
        let shaders = ctx.rs.shaders.as_ref().unwrap();
        for (k, c) in ctx.wm.closing.iter_mut().enumerate() {
            if !ghost_done[k] && c.ws == wws && !c.above.contains(&wid) {
                ghost_done[k] = true;
                elements.extend(ghost_element(c, ghost_dx[k], out_rect, scale, g.window_radius, &shaders.rounded_texture));
            }
        }
        let w = &ctx.wm.windows[i];
        let geo = w.geo();
        // Unscaled rect U (logical, output-local); in the overview it moves
        // toward the window's slot.
        let mut u = Rectangle::<f64, Logical>::new((geo.loc.x + ws_dx - out_rect.loc.x, geo.loc.y - out_rect.loc.y).into(), geo.size);
        if let Some(slot) = slots.get(&w.id) {
            let k = (slot.size.w / geo.size.w.max(1.0)).min(slot.size.h / geo.size.h.max(1.0));
            let target = Rectangle::<f64, Logical>::new(
                (
                    slot.loc.x + ws_dx - out_rect.loc.x + (slot.size.w - geo.size.w * k) / 2.0,
                    slot.loc.y - out_rect.loc.y + (slot.size.h - geo.size.h * k) / 2.0,
                )
                    .into(),
                (geo.size.w * k, geo.size.h * k).into(),
            );
            u = lerp_rect(u, target, overview);
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
        let shaders = ctx.rs.shaders.as_ref().unwrap();

        if hovered == Some(w.id) {
            let pad = 4.0 * scale;
            let ring = Rectangle::<f64, Physical>::new(
                (f.loc.x - pad, f.loc.y - pad).into(),
                (f.size.w + pad * 2.0, f.size.h + pad * 2.0).into(),
            );
            let c = theme.colors.accent;
            let clear_c = c.with_alpha(0.0);
            let shape = crate::shell::render::rect_shape(
                ctx.shell.shapes.get(&out_name, "hover-outline", 0),
                &shaders.rect,
                ring,
                radius as f64 + pad,
                clear_c,
                clear_c,
                Some((c, 3.0 * scale)),
                overview as f32,
            );
            elements.push(LumenElement::Shape(shape.clone()));
        }

        let snapshot = push_window(r, w, f, scale, alpha, radius, shaders, None, &mut elements);
        if let Some(snap) = snapshot {
            if overview <= 0.0 && open >= 1.0 {
                ctx.rs.last_window.insert(w.id, snap);
            }
        }

        // Shadow under the window.
        let shadow_prog = shaders.shadow.clone();
        let w = &mut ctx.wm.windows[i];
        w.last_drawn = Some(Rectangle::new(
            (f_log.loc.x + out_rect.loc.x - ws_dx, f_log.loc.y + out_rect.loc.y).into(),
            f_log.size,
        ));
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
            let shape = w.shadows.entry(out_name.clone()).or_default().set(&shadow_prog, dst, alpha, &key, || {
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

    // Ghosts below every window (or with nothing left below them).
    let shaders = ctx.rs.shaders.as_ref().unwrap();
    for (k, c) in ctx.wm.closing.iter_mut().enumerate() {
        if !ghost_done[k] {
            elements.extend(ghost_element(c, ghost_dx[k], out_rect, scale, g.window_radius, &shaders.rounded_texture));
        }
    }

    // Overview backdrop: dim layer and the blurred wallpaper fading in.
    let full = Rectangle::<i32, Physical>::from_size(phys);
    if overview > 0.0 {
        ensure_wallpaper_blurred(r, ctx.rs, phys);
        let c = theme.colors.overview_dim;
        let shape = crate::shell::render::rect_shape(
            ctx.shell.shapes.get(&out_name, "overview-dim", 0),
            &ctx.rs.shaders.as_ref().unwrap().rect,
            full.to_f64(),
            0.0,
            c,
            c,
            None,
            overview as f32,
        );
        elements.push(LumenElement::Shape(shape.clone()));
        if let Some(b) = ctx.rs.wallpapers.get_mut(&(phys.w, phys.h)).and_then(|wp| wp.blurred.as_mut()) {
            elements.push(LumenElement::Image(b.element_in(full, overview as f32, None, &[])));
        }
    }
    if let Some(wp) = ctx.rs.wallpapers.get_mut(&(phys.w, phys.h)) {
        elements.push(LumenElement::Image(wp.image.element_in(full, 1.0, None, &[])));
    }
    ctx.rs.text.end_frame();
    (elements, clear)
}

fn cursor_elements(r: &mut GlesRenderer, ctx: &mut SceneCtx, out_rect: Rectangle<f64, Logical>, scale: f64, elements: &mut Vec<LumenElement>) {
    if !out_rect.contains(ctx.pointer) {
        return;
    }
    let local = ctx.pointer - out_rect.loc;
    // Pixel-aligned pointer position on this output.
    let at_px = Point::<i32, Physical>::from(((local.x * scale).round() as i32, (local.y * scale).round() as i32));
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
            let at = at_px - hotspot.to_physical_precise_round(scale);
            let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                render_elements_from_surface_tree(r, surface, at, scale, 1.0, Kind::Cursor);
            elements.extend(elems.into_iter().map(LumenElement::Surface));
        }
        CursorImageStatus::Named(icon) => {
            let frame = ctx.cursor_theme.frame(icon.name(), scale, ctx.now);
            let at = at_px - frame.hotspot;
            if let Ok(e) =
                MemoryRenderBufferRenderElement::from_buffer(r, at.to_f64(), &frame.buffer, None, None, Some(frame.logical), Kind::Cursor)
            {
                elements.push(LumenElement::Memory(e));
            }
        }
    }
    // The icon of a drag-and-drop in progress, under the pointer.
    if let Some(icon) = ctx.dnd_icon.filter(|s| s.alive()) {
        let elems: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
            render_elements_from_surface_tree(r, icon, at_px, scale, 0.9, Kind::Unspecified);
        elements.extend(elems.into_iter().map(LumenElement::Surface));
    }
}

/// Render a window's last frame into a texture of its geometry size.
pub fn snapshot_window(r: &mut GlesRenderer, snap: &WindowSnapshot) -> Option<GlesTexture> {
    let size = snap.size;
    if size.w <= 0 || size.h <= 0 {
        return None;
    }
    let mut tex = blur::new_target(r, size).ok()?;
    {
        let mut fb = r.bind(&mut tex).ok()?;
        let mut frame = r.render(&mut fb, size, Transform::Normal).ok()?;
        let full = Rectangle::from_size(size);
        frame.clear(Color32F::TRANSPARENT, &[full]).ok()?;
        for p in snap.parts.iter().rev() {
            match p {
                SnapPart::Texture { texture, src, dst, transform } => {
                    let Some(visible) = dst.intersection(full) else { continue };
                    let mut local = visible;
                    local.loc -= dst.loc;
                    frame.render_texture_from_to(texture, *src, *dst, &[local], &[], *transform, 1.0, None, &[]).ok()?;
                }
                SnapPart::Solid { color, dst } => {
                    let Some(visible) = dst.intersection(full) else { continue };
                    let mut local = visible;
                    local.loc -= dst.loc;
                    frame.draw_solid(*dst, &[local], *color).ok()?;
                }
            }
        }
        let _ = frame.finish().ok()?;
    }
    Some(tex)
}

impl crate::state::Lumen {
    /// Start the closing animation for a window (snapshot of its last frame),
    /// from where and at what stacking position it was last drawn.
    pub fn close_animation(&mut self, id: u32) {
        let Some(w) = self.wm.get(id) else { return };
        let rect = w.last_drawn.unwrap_or_else(|| w.geo());
        let ws = w.ws;
        let Some(i) = self.wm.index_of(id) else { return };
        let above: Vec<u32> = self.wm.windows[i + 1..].iter().filter(|o| o.ws == ws).map(|o| o.id).collect();
        let Some(snap) = self.render.last_window.remove(&id) else { return };
        let size = snap.size;
        if let Some(tex) = self.backend.with_renderer(|r| snapshot_window(r, &snap)).flatten() {
            self.wm.start_closing(Image::from_texture(tex, size), rect, ws, above);
        }
    }
}
