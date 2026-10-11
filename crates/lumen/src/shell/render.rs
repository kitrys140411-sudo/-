//! Drawing of the shell UI. Every on-screen object owns a persistent
//! [`Shape`] so unchanged UI produces no damage.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesPixelProgram, GlesRenderer, Uniform};
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Rectangle, Transform};

use crate::render::elements::{enclose, Shape};
use crate::render::{LumenElement, SceneCtx};
use crate::theme::Color;
use crate::wm::WORKSPACES;

#[derive(Default)]
pub struct ShellShapes {
    pub panel: Shape,
    pub ws_dots: Vec<Shape>,
    pub ws_hover: Shape,
    pub overview_dim: Shape,
    pub hover_outline: Shape,
    pub switcher_bg: Shape,
    pub switcher_sel: Shape,
    pub hud: Option<MemoryRenderBuffer>,
}

pub fn to_phys(r: Rectangle<f64, Logical>, out: Rectangle<f64, Logical>, scale: f64) -> Rectangle<f64, Physical> {
    Rectangle::new(
        ((r.loc.x - out.loc.x) * scale, (r.loc.y - out.loc.y) * scale).into(),
        (r.size.w * scale, r.size.h * scale).into(),
    )
}

/// A rounded rectangle (fill, optional vertical gradient and border).
#[allow(clippy::too_many_arguments)]
pub fn rect_shape<'a>(
    shape: &'a mut Shape,
    program: &GlesPixelProgram,
    r: Rectangle<f64, Physical>,
    radius: f64,
    fill: Color,
    fill2: Color,
    border: Option<(Color, f64)>,
    alpha: f32,
) -> &'a Shape {
    let (dst, bx) = enclose(r, 1.0);
    let (bc, bw) = border.unwrap_or((Color::rgba(0.0, 0.0, 0.0, 0.0), 0.0));
    let (f1, f2, b) = (fill.premultiplied(), fill2.premultiplied(), bc.premultiplied());
    let key = [bx[0], bx[1], bx[2], bx[3], radius as f32, f1[0], f1[1], f1[2], f1[3], f2[0], f2[1], f2[2], f2[3], b[0], b[1], b[2], b[3], bw as f32];
    shape.set(program, dst, alpha, &key, || {
        vec![
            Uniform::new("fill", f1),
            Uniform::new("fill2", f2),
            Uniform::new("border", b),
            Uniform::new("border_width", bw as f32),
            Uniform::new("radius", radius as f32),
            Uniform::new("box", bx),
        ]
    })
}

/// Shell elements drawn above windows, front first.
pub fn overlay_elements(
    r: &mut GlesRenderer,
    ctx: &mut SceneCtx,
    output: &Output,
    out: Rectangle<f64, Logical>,
    scale: f64,
    elements: &mut Vec<LumenElement>,
) {
    let _ = output;
    if ctx.shell.hud {
        hud(r, ctx, out, scale, elements);
    }
    switcher(r, ctx, out, scale, elements);
    panel(ctx, out, scale, elements);
}

fn panel(ctx: &mut SceneCtx, out: Rectangle<f64, Logical>, scale: f64, elements: &mut Vec<LumenElement>) {
    let theme = ctx.theme;
    let g = theme.geometry.grid;
    let h = theme.geometry.panel_height;
    let overview = ctx.shell.overview_progress() as f32;
    let prog = ctx.rs.shaders.as_ref().expect("shaders compiled").rect.clone();
    let mut shapes = std::mem::take(&mut ctx.shell.shapes);

    // Workspace indicator: one pill per workspace, the active one wide. The
    // pills follow the workspace spring, so switching animates them too.
    let ind = crate::shell::workspace_indicator_rect(theme, out);
    let ws_pos = ctx.wm.ws_anim.value;
    let dot = g * 0.875;
    let mut x = ind.loc.x + g;
    let y = out.loc.y + (h - dot) / 2.0;
    shapes.ws_dots.resize_with(WORKSPACES, Shape::default);
    for (i, shape) in shapes.ws_dots.iter_mut().enumerate() {
        let active = (1.0 - (ws_pos - i as f64).abs()).clamp(0.0, 1.0);
        let w = dot + g * 3.0 * active;
        let occupied = ctx.wm.windows.iter().any(|win| win.mapped && win.ws == i);
        let base = if occupied { theme.colors.text_dim } else { theme.colors.text_dim.with_alpha(0.45) };
        let color = base.lerp(theme.colors.text, active as f32);
        let rect = to_phys(Rectangle::new((x, y).into(), (w, dot).into()), out, scale);
        let s = rect_shape(shape, &prog, rect, dot * scale / 2.0, color, color, None, 1.0);
        elements.push(LumenElement::Shape(s.clone()));
        x += w + g * 0.5;
    }
    if ctx.shell.overview_open || ind.contains(ctx.pointer) {
        let pad = g * 0.5;
        let r = Rectangle::new((ind.loc.x, ind.loc.y + pad).into(), (ind.size.w, h - pad * 2.0).into());
        let c = theme.colors.surface_hover;
        let s = rect_shape(&mut shapes.ws_hover, &prog, to_phys(r, out, scale), (h - pad * 2.0) * scale / 2.0, c, c, None, 1.0);
        elements.push(LumenElement::Shape(s.clone()));
    }

    // The bar itself fades out in the overview, leaving the indicator floating.
    let bar = Rectangle::new(out.loc, (out.size.w, h).into());
    let c = theme.colors.panel;
    let alpha = 1.0 - overview * 0.85;
    let s = rect_shape(&mut shapes.panel, &prog, to_phys(bar, out, scale), 0.0, c, c, None, alpha);
    elements.push(LumenElement::Shape(s.clone()));
    ctx.shell.shapes = shapes;
}

/// Alt+Tab popup: a glass card with a live thumbnail of every window.
fn switcher(r: &mut GlesRenderer, ctx: &mut SceneCtx, out: Rectangle<f64, Logical>, scale: f64, elements: &mut Vec<LumenElement>) {
    let Some(sw) = ctx.shell.switcher.as_ref() else {
        return;
    };
    let theme = ctx.theme;
    let g = theme.geometry.grid;
    let appear = sw.appear.value.clamp(0.0, 1.0) as f32;
    let prog = ctx.rs.shaders.as_ref().expect("shaders compiled").rect.clone();
    let ids: Vec<u32> = sw.ids.iter().copied().filter(|id| ctx.wm.get(*id).is_some()).collect();
    if ids.is_empty() {
        return;
    }
    let selected = sw.ids.get(sw.index).copied();
    let thumb_h = (out.size.h * 0.16).clamp(96.0, 180.0);
    let max_w = thumb_h * 1.8;
    let sizes: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| {
            let s = ctx.wm.get(*id).unwrap().size();
            let k = (thumb_h / s.h.max(1) as f64).min(max_w / s.w.max(1) as f64);
            (s.w as f64 * k, s.h as f64 * k)
        })
        .collect();
    let pad = g * 2.0;
    let gap = g * 2.0;
    let card_w = |w: f64| w.max(thumb_h * 0.6) + pad;
    let mut total_w = sizes.iter().map(|(w, _)| card_w(*w)).sum::<f64>() + gap * (ids.len() - 1) as f64 + pad * 2.0;
    let k = if total_w > out.size.w - g * 8.0 { (out.size.w - g * 8.0) / total_w } else { 1.0 };
    total_w *= k;
    let total_h = (thumb_h + pad * 3.0) * k;
    let bg = Rectangle::new(
        (out.loc.x + (out.size.w - total_w) / 2.0, out.loc.y + (out.size.h - total_h) / 2.0).into(),
        (total_w, total_h).into(),
    );
    let mut shapes = std::mem::take(&mut ctx.shell.shapes);
    let mut x = bg.loc.x + pad * k;
    let mut thumbs = Vec::new();
    let mut cards = Vec::new();
    for (id, (w, h)) in ids.iter().zip(&sizes) {
        let cw = card_w(*w) * k;
        let card = Rectangle::new((x, bg.loc.y + pad * k).into(), (cw, (thumb_h + pad) * k).into());
        let (tw, th) = (w * k, h * k);
        let thumb = Rectangle::new(
            (card.loc.x + (cw - tw) / 2.0, card.loc.y + pad / 2.0 * k + (thumb_h * k - th) / 2.0).into(),
            (tw, th).into(),
        );
        thumbs.push((*id, to_phys(thumb, out, scale)));
        if Some(*id) == selected {
            let c = theme.colors.surface_hover;
            let s = rect_shape(
                &mut shapes.switcher_sel,
                &prog,
                to_phys(card, out, scale),
                theme.geometry.popover_radius * 0.75 * scale,
                c,
                c,
                Some((theme.colors.accent.with_alpha(0.9), 2.0 * scale)),
                appear,
            );
            cards.push(LumenElement::Shape(s.clone()));
        }
        x += cw + gap * k;
    }
    // Front to back: thumbnails, selection card, background.
    for (id, rect) in thumbs {
        crate::render::thumbnail_elements(r, ctx, id, rect, theme.geometry.window_radius * 0.5 * scale, appear, elements);
    }
    elements.extend(cards);
    let c = theme.colors.surface;
    let s = rect_shape(
        &mut shapes.switcher_bg,
        &prog,
        to_phys(bg, out, scale),
        theme.geometry.popover_radius * scale,
        c,
        c,
        Some((theme.colors.border, 1.0 * scale)),
        appear,
    );
    elements.push(LumenElement::Shape(s.clone()));
    ctx.shell.shapes = shapes;
}

/// Frame-time HUD: render time (bars) and presentation intervals (line) of the
/// last frames, with 60 Hz budget marks. Drawn on the CPU into a small buffer.
fn hud(r: &mut GlesRenderer, ctx: &mut SceneCtx, out: Rectangle<f64, Logical>, scale: f64, elements: &mut Vec<LumenElement>) {
    const W: i32 = 240;
    const H: i32 = 72;
    let pw = (W as f64 * scale).round() as i32;
    let ph = (H as f64 * scale).round() as i32;
    let buf = ctx.shell.shapes.hud.get_or_insert_with(|| MemoryRenderBuffer::new(Fourcc::Abgr8888, (pw, ph), 1, Transform::Normal, None));
    let stats = ctx.stats.clone();
    let mut rc = buf.render();
    rc.resize((pw, ph));
    let _ = rc.draw(|mem: &mut [u8]| -> Result<_, ()> {
        let (w, h) = (pw as usize, ph as usize);
        let put = |mem: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
            if x < w && y < h {
                let i = (y * w + x) * 4;
                mem[i..i + 4].copy_from_slice(&c);
            }
        };
        for px in mem.chunks_exact_mut(4) {
            px.copy_from_slice(&[10, 10, 14, 200]);
        }
        // 0..33 ms vertical range; marks at 8.3 and 16.7 ms.
        let y_of = |ms: f32| (h as f32 - 1.0 - ms / 33.3 * (h as f32 - 1.0)).clamp(0.0, h as f32 - 1.0) as usize;
        for x in 0..w {
            put(mem, x, y_of(16.7), [90, 90, 110, 255]);
            if x % 4 == 0 {
                put(mem, x, y_of(8.3), [60, 60, 75, 255]);
            }
        }
        let n = w.min(stats.render_ms.len());
        for (i, ms) in stats.render_ms[stats.render_ms.len() - n..].iter().enumerate() {
            let x = w - n + i;
            let top = y_of(*ms);
            let color = if *ms > 16.7 { [230, 90, 90, 255] } else if *ms > 8.3 { [230, 190, 90, 255] } else { [110, 200, 130, 255] };
            for y in top..h {
                put(mem, x, y, color);
            }
        }
        let n = w.min(stats.interval_ms.len());
        for (i, ms) in stats.interval_ms[stats.interval_ms.len() - n..].iter().enumerate() {
            let x = w - n + i;
            let c = if *ms > 25.0 { [255, 80, 80, 255] } else { [122, 162, 247, 255] };
            put(mem, x, y_of(*ms), c);
            put(mem, x, y_of(*ms).saturating_sub(1), c);
        }
        Ok(vec![Rectangle::from_size((pw, ph).into())])
    });
    drop(rc);
    let g = ctx.theme.geometry.grid;
    let x = (out.size.w - W as f64 - g * 2.0) * scale;
    let y = (ctx.theme.geometry.panel_height + g * 2.0) * scale;
    if let Ok(e) = MemoryRenderBufferRenderElement::from_buffer(r, (x.round(), y.round()), buf, None, None, None, Kind::Unspecified) {
        elements.push(LumenElement::Memory(e));
    }
}
