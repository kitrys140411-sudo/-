//! Drawing of the shell UI. Every on-screen object owns a persistent shape or
//! image (keyed per output) so unchanged UI produces no damage, and every
//! interactive object records its hit area for input handling.

use std::collections::HashMap;

use lumen_text::{Align, Weight};
use smithay::backend::renderer::gles::{GlesPixelProgram, GlesRenderer, Uniform};
use smithay::output::Output;
use smithay::utils::{Logical, Physical, Point, Rectangle};

use super::sheet::{self, FlowItem};
use super::{dock_cell, dock_rect, layout, Hit, Rect, POWER_ITEMS};
use crate::render::elements::{enclose, Image, Shape};
use crate::render::{LumenElement, SceneCtx};
use crate::text::Style;
use crate::theme::Color;
use crate::wm::WORKSPACES;

#[derive(Default)]
pub struct ShellShapes {
    map: HashMap<(String, &'static str, u64), Shape>,
    hud: HashMap<String, Image>,
}

impl ShellShapes {
    pub fn get(&mut self, output: &str, key: &'static str, i: u64) -> &mut Shape {
        self.map.entry((output.to_string(), key, i)).or_default()
    }

    /// Forget shapes of objects that went away (notifications, dock items).
    pub fn retain(&mut self, f: impl Fn(&'static str, u64) -> bool) {
        self.map.retain(|(_, k, i), _| f(k, *i));
    }
}

pub fn to_phys(r: Rect, out: Rect, scale: f64) -> Rectangle<f64, Physical> {
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

/// Drawing helper bound to one output and one frame.
struct Ui<'a, 'b, 'c> {
    r: &'a mut GlesRenderer,
    ctx: &'a mut SceneCtx<'b>,
    out: Rect,
    scale: f64,
    output: String,
    elements: &'c mut Vec<LumenElement>,
    prog: GlesPixelProgram,
}

impl Ui<'_, '_, '_> {
    fn phys(&self, r: Rect) -> Rectangle<f64, Physical> {
        to_phys(r, self.out, self.scale)
    }

    fn hit(&mut self, r: Rect, h: Hit) {
        self.ctx.shell.hits.push((r, h));
    }

    #[allow(clippy::too_many_arguments)]
    fn rect(&mut self, key: &'static str, i: u64, r: Rect, radius: f64, fill: Color, border: Option<(Color, f64)>, alpha: f32) {
        self.rect_gradient(key, i, r, radius, (fill, fill), border, alpha);
    }

    /// A rounded rectangle with a vertical gradient (top, bottom).
    #[allow(clippy::too_many_arguments)]
    fn rect_gradient(&mut self, key: &'static str, i: u64, r: Rect, radius: f64, fill: (Color, Color), border: Option<(Color, f64)>, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let p = self.phys(r);
        let border = border.map(|(c, w)| (c, w * self.scale));
        let shape = self.ctx.shell.shapes.get(&self.output, key, i);
        let s = rect_shape(shape, &self.prog, p, radius * self.scale, fill.0, fill.1, border, alpha);
        self.elements.push(LumenElement::Shape(s.clone()));
    }

    /// Round a logical coordinate to whole physical pixels (crisp edges at
    /// any scale).
    fn snap(&self, v: f64) -> f64 {
        (v * self.scale).round() / self.scale
    }

    /// A frosted-glass pane: what is behind it blurred and tinted (or just
    /// the tint with blur disabled), with an optional hairline border.
    #[allow(clippy::too_many_arguments)]
    fn glass(&mut self, key: &'static str, i: u64, r: Rect, radius: f64, tint: Color, border: Option<(Color, f64)>, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let blur = self.ctx.theme.blur.enabled;
        let fill = if blur { tint.with_alpha(0.0) } else { tint };
        // The shape carries the border (and the tint without blur); it must
        // exist even without a border so the glass has a place in the list.
        let border = border.or(Some((tint.with_alpha(0.0), 0.0)));
        self.rect(key, i, r, radius, fill, border, alpha);
        if blur {
            let rect = self.phys(r);
            self.ctx.rs.glass_requests.push(crate::render::glass::GlassRequest {
                index: self.elements.len() - 1,
                rect,
                radius: (radius * self.scale) as f32,
                tint,
                alpha,
                key: (key, i),
            });
        }
    }

    /// Text with its layout box's top-left at `at`; returns the box (logical).
    fn text(&mut self, text: &str, style: &Style, at: Point<f64, Logical>, max_w: Option<f64>, lines: Option<usize>, alpha: f32) -> Rect {
        let p = Point::<f64, Physical>::from(((at.x - self.out.loc.x) * self.scale, (at.y - self.out.loc.y) * self.scale));
        match self.ctx.rs.text.element(self.r, text, style, self.scale, max_w, lines, p, alpha) {
            Some((e, rect)) => {
                if alpha > 0.0 {
                    self.elements.push(LumenElement::Image(e));
                }
                Rectangle::new(at, (rect.size.w / self.scale, rect.size.h / self.scale).into())
            }
            None => Rectangle::new(at, (0.0, 0.0).into()),
        }
    }

    /// Logical size of a text without drawing it.
    fn measure(&mut self, text: &str, style: &Style, max_w: Option<f64>, lines: Option<usize>) -> (f64, f64) {
        let (w, h) = self.ctx.rs.text.measure(text, style, self.scale, max_w, lines);
        (w / self.scale, h / self.scale)
    }

    fn icon(&mut self, name: &str, r: Rect, alpha: f32, letter_of: &str) {
        if alpha <= 0.0 {
            return;
        }
        let p = self.phys(r);
        let px = p.size.w.round().max(1.0) as u32;
        let letter = letter_of.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
        let rs = &mut *self.ctx.rs;
        let text = &mut rs.text;
        if let Some(img) = rs.icons.get(self.r, name, px, || text.render_image(&letter, px as f32 * 0.5, Weight::SemiBold)) {
            let e = img.element((p.loc.x.round() as i32, p.loc.y.round() as i32).into(), alpha);
            self.elements.push(LumenElement::Image(e));
        }
    }
}

/// Shell elements drawn above windows, front first.
pub fn overlay_elements(
    r: &mut GlesRenderer,
    ctx: &mut SceneCtx,
    output: &Output,
    out: Rect,
    scale: f64,
    elements: &mut Vec<LumenElement>,
) {
    let Some(prog) = ctx.rs.shaders.as_ref().map(|s| s.rect.clone()) else { return };
    ctx.shell.hits.clear();
    let mut ui = Ui { r, ctx, out, scale, output: output.name(), elements, prog };
    let overview = ui.ctx.shell.overview_progress();
    // A fullscreen window on top hides the panel and the dock (unless the
    // overview is shown).
    let fullscreen = overview <= 0.0 && ui.ctx.wm.fullscreen_on_top();
    if ui.ctx.shell.hud {
        hud(&mut ui);
    }
    osd(&mut ui);
    sheet(&mut ui);
    if ui.ctx.shell.sheet.is_none() {
        ui.ctx.shell.shapes.retain(|k, _| !SHEET_SHAPES.contains(&k));
    }
    popover(&mut ui);
    notifications(&mut ui);
    switcher(&mut ui);
    if !fullscreen {
        panel(&mut ui);
    }
    if !fullscreen || overview > 0.0 {
        dock(&mut ui);
    }
    if overview > 0.0 {
        overview_ui(&mut ui, overview);
    }
}

fn panel(ui: &mut Ui) {
    let theme = ui.ctx.theme;
    let out = ui.out;
    let g = theme.geometry.grid;
    let h = theme.geometry.panel_height;
    let overview = ui.ctx.shell.overview_progress() as f32;

    // Workspace pills (the "Activities" button): one per workspace, the
    // active one wide; they follow the workspace spring.
    let ind = layout::workspace_indicator_rect(theme, out);
    let ws_pos = ui.ctx.wm.ws_anim.value;
    let dot = g * 0.875;
    let mut x = ind.loc.x + g;
    let y = out.loc.y + (h - dot) / 2.0;
    for i in 0..WORKSPACES {
        let active = (1.0 - (ws_pos - i as f64).abs()).clamp(0.0, 1.0);
        let w = dot + g * 3.0 * active;
        let occupied = ui.ctx.wm.windows.iter().any(|win| win.mapped && win.ws == i);
        let base = if occupied { theme.colors.text_dim } else { theme.colors.text_dim.with_alpha(0.45) };
        let color = base.lerp(theme.colors.text, active as f32);
        ui.rect("ws-dot", i as u64, layout::rect(x, y, w, dot), dot / 2.0, color, None, 1.0);
        x += w + g * 0.5;
    }
    ui.hit(ind, Hit::Activities);
    let hovered = ui.ctx.shell.hover == Some(Hit::Activities) || ui.ctx.shell.overview_open;
    if hovered {
        let pad = g * 0.5;
        let r = layout::rect(ind.loc.x, ind.loc.y + pad, ind.size.w, h - pad * 2.0);
        ui.rect("ws-hover", 0, r, (h - pad * 2.0) / 2.0, theme.colors.surface_hover, None, 1.0);
    }

    // Clock in the middle.
    let style = Style::new(13.0, theme.colors.text).weight(Weight::SemiBold);
    let clock = super::clock_text();
    let (cw, chh) = ui.measure(&clock, &style, None, None);
    let at = Point::from((out.loc.x + (out.size.w - cw) / 2.0, out.loc.y + (h - chh) / 2.0));
    ui.text(&clock, &style, at, None, None, 1.0);

    // Power button on the right.
    let p = layout::power_rect(theme, out);
    ui.hit(p, Hit::Power);
    let isz = g * 2.0;
    ui.icon("power", layout::rect(p.loc.x + (p.size.w - isz) / 2.0, p.loc.y + (p.size.h - isz) / 2.0, isz, isz), 0.9, "");
    if ui.ctx.shell.hover == Some(Hit::Power) || ui.ctx.shell.popover.as_ref().is_some_and(|p| p.open) {
        let pad = g * 0.5;
        ui.rect("power-hover", 0, layout::rect(p.loc.x, p.loc.y + pad, p.size.w, h - pad * 2.0), (h - pad * 2.0) / 2.0, theme.colors.surface_hover, None, 1.0);
    }

    // The bar itself fades out in the overview, leaving its content floating.
    let bar = layout::rect(out.loc.x, out.loc.y, out.size.w, h);
    ui.glass("panel", 0, bar, 0.0, theme.colors.panel, None, 1.0 - overview * 0.85);
}

fn dock(ui: &mut Ui) {
    let theme = ui.ctx.theme;
    let out = ui.out;
    let g = theme.geometry.grid;
    let shown = ui.ctx.shell.dock.value.clamp(0.0, 1.2);
    if shown <= 0.001 {
        return;
    }
    let items = ui.ctx.shell.dock_items(ui.ctx.wm);
    let n = items.len();
    let full = dock_rect(theme, out, n);
    // Slide below the screen edge when hidden.
    let dy = (1.0 - shown) * (full.size.h + g * 2.0);
    let dock = layout::rect(full.loc.x, full.loc.y + dy, full.size.w, full.size.h);
    let alpha = shown.min(1.0) as f32;
    let focused = ui.ctx.wm.focused;
    let size = theme.geometry.dock_icon_size;
    // Tooltip above the hovered item.
    if let Some(Hit::DockItem(i)) = ui.ctx.shell.hover {
        if let Some(item) = items.get(i) {
            let cell = dock_cell(theme, dock, n, i);
            tooltip(ui, &item.name.clone(), cell, alpha);
        }
    }
    if ui.ctx.shell.hover == Some(Hit::DockApps) {
        let cell = dock_cell(theme, dock, n, n);
        tooltip(ui, "Show Apps", cell, alpha);
    }
    for (i, item) in items.iter().enumerate() {
        let cell = dock_cell(theme, dock, n, i);
        ui.hit(cell, Hit::DockItem(i));
        // Running indicator: a dot (a short bar when focused).
        if !item.windows.is_empty() {
            let active = focused.is_some_and(|f| item.windows.contains(&f));
            let w = if active { g * 1.5 } else { g * 0.6 };
            let r = layout::rect(cell.loc.x + (cell.size.w - w) / 2.0, cell.loc.y + cell.size.h + g * 0.35, w, g * 0.45);
            ui.rect("dock-dot", i as u64, r, g * 0.25, theme.colors.text, None, alpha);
        }
        ui.icon(&item.icon, cell, alpha, &item.name);
        if ui.ctx.shell.hover == Some(Hit::DockItem(i)) {
            let pad = g * 0.5;
            let r = layout::rect(cell.loc.x - pad, cell.loc.y - pad, cell.size.w + pad * 2.0, cell.size.h + pad * 2.0);
            ui.rect("dock-hover", i as u64, r, theme.geometry.dock_radius * 0.6, theme.colors.surface_hover.with_alpha(0.7), None, alpha);
        }
    }
    // Separator and the app grid button.
    if n > 0 {
        let c = dock_cell(theme, dock, n, n);
        let sep = layout::rect(c.loc.x - g * 1.5 - 0.5, dock.loc.y + g * 1.5, 1.0, size - g);
        ui.rect("dock-sep", 0, sep, 0.0, theme.colors.border, None, alpha);
    }
    let apps_cell = dock_cell(theme, dock, n, n);
    ui.hit(apps_cell, Hit::DockApps);
    ui.icon("app-grid", apps_cell, alpha, "");
    ui.hit(dock, Hit::Dock);
    ui.glass("dock-bg", 0, dock, theme.geometry.dock_radius, theme.colors.glass_tint, Some((theme.colors.border, 1.0)), alpha);
    ui.ctx.shell.shapes.retain(|k, i| !(k == "dock-dot" || k == "dock-hover") || (i as usize) < n.max(1));
}

fn tooltip(ui: &mut Ui, text: &str, anchor: Rect, alpha: f32) {
    let theme = ui.ctx.theme;
    let g = theme.geometry.grid;
    let style = Style::new(12.0, theme.colors.text);
    // Window titles can be arbitrarily long: one ellipsized line.
    let max_w = (ui.out.size.w * 0.5).max(g * 8.0);
    let (w, h) = ui.measure(text, &style, Some(max_w), Some(1));
    let bw = (w + g * 2.0).min(ui.out.size.w - g * 2.0);
    let bh = h + g;
    let lo = ui.out.loc.x + g;
    let hi = (ui.out.loc.x + ui.out.size.w - bw - g).max(lo);
    let x = (anchor.loc.x + (anchor.size.w - bw) / 2.0).clamp(lo, hi);
    let y = anchor.loc.y - bh - g * 1.5;
    ui.text(text, &style, Point::from((x + g, y + g * 0.5)), Some(max_w), Some(1), alpha);
    ui.rect("tooltip", 0, layout::rect(x, y, bw, bh), bh / 2.0, theme.colors.surface, Some((theme.colors.border, 1.0)), alpha);
}

fn popover(ui: &mut Ui) {
    let Some(p) = ui.ctx.shell.popover.as_ref() else { return };
    let t = p.anim.value.clamp(0.0, 1.1);
    if t <= 0.001 {
        return;
    }
    let theme = ui.ctx.theme;
    let g = theme.geometry.grid;
    let (menu, rows) = layout::power_menu(theme, ui.out, POWER_ITEMS.len());
    // Grow from the button: slide down a little and fade in.
    let dy = (1.0 - t.min(1.0)) * -g * 1.5;
    let alpha = t.min(1.0) as f32;
    let shift = |r: Rect| layout::rect(r.loc.x, r.loc.y + dy, r.size.w, r.size.h);
    let style = Style::new(14.0, theme.colors.text);
    for (i, label) in POWER_ITEMS.iter().enumerate() {
        let row = shift(rows[i]);
        ui.hit(row, Hit::PowerItem(i));
        let (_, th) = ui.measure(label, &style, None, None);
        ui.text(label, &style, Point::from((row.loc.x + g * 1.5, row.loc.y + (row.size.h - th) / 2.0)), None, None, alpha);
        if ui.ctx.shell.hover == Some(Hit::PowerItem(i)) {
            ui.rect("pop-hover", 0, row, theme.geometry.popover_radius * 0.6, theme.colors.surface_hover, None, alpha);
        }
    }
    let m = shift(menu);
    ui.hit(m, Hit::Popover);
    ui.glass("pop-bg", 0, m, theme.geometry.popover_radius, theme.colors.glass_surface, Some((theme.colors.border, 1.0)), alpha);
}

fn notifications(ui: &mut Ui) {
    let theme = ui.ctx.theme;
    let g = theme.geometry.grid;
    let w = theme.geometry.notification_width.min(ui.out.size.w - g * 4.0);
    // Top center; in the overview they move to the right edge so the search
    // entry stays visible.
    let center = ui.out.loc.x + (ui.out.size.w - w) / 2.0;
    let right = ui.out.loc.x + ui.out.size.w - w - g * 2.0;
    let x = center + (right - center) * ui.ctx.shell.overview_progress();
    let mut y = ui.out.loc.y + theme.geometry.panel_height + g * 1.5;
    let title_style = Style::new(14.0, theme.colors.text).weight(Weight::SemiBold);
    let body_style = Style::new(13.0, theme.colors.text_dim);
    let items: Vec<(u64, String, String, f64)> =
        ui.ctx.shell.notifications.iter().map(|n| (n.id, n.title.clone(), n.body.clone(), n.anim.value)).collect();
    let text_w = w - g * 4.0 - g * 3.0;
    for (id, title, body, t) in items {
        let t = t.clamp(0.0, 1.2);
        let (_, title_h) = ui.measure(&title, &title_style, Some(text_w), Some(1));
        let (_, body_h) = if body.is_empty() { (0.0, 0.0) } else { ui.measure(&body, &body_style, Some(text_w), Some(3)) };
        let h = g * 2.0 + title_h + if body.is_empty() { 0.0 } else { g * 0.5 + body_h } + g * 2.0;
        // Slide down from under the panel while fading in; the stack below
        // follows smoothly as cards come and go.
        let dy = (1.0 - t.min(1.0)) * -(h * 0.6);
        let alpha = t.min(1.0) as f32;
        let card = layout::rect(x, y + dy, w, h);
        ui.hit(layout::rect(card.loc.x + card.size.w - g * 4.0, card.loc.y + g, g * 3.0, g * 3.0), Hit::NotificationClose(id));
        ui.hit(card, Hit::Notification(id));
        let hovered = matches!(ui.ctx.shell.hover, Some(Hit::Notification(n)) | Some(Hit::NotificationClose(n)) if n == id);
        if hovered {
            let c = layout::rect(card.loc.x + card.size.w - g * 3.5, card.loc.y + g * 1.5, g * 2.0, g * 2.0);
            ui.icon("close", c, alpha * 0.8, "");
        }
        let tx = card.loc.x + g * 2.0;
        ui.text(&title, &title_style, Point::from((tx, card.loc.y + g * 2.0)), Some(text_w), Some(1), alpha);
        if !body.is_empty() {
            ui.text(&body, &body_style, Point::from((tx, card.loc.y + g * 2.5 + title_h)), Some(text_w), Some(3), alpha);
        }
        ui.glass("notif", id, card, theme.geometry.popover_radius, theme.colors.glass_surface, Some((theme.colors.border, 1.0)), alpha);
        y += (h + g) * t.min(1.0);
    }
    let alive: Vec<u64> = ui.ctx.shell.notifications.iter().map(|n| n.id).collect();
    ui.ctx.shell.shapes.retain(|k, i| k != "notif" || alive.contains(&i));
}

/// Alt+Tab popup: a card with a live thumbnail of every window and the
/// selected window's title.
fn switcher(ui: &mut Ui) {
    let Some(sw) = ui.ctx.shell.switcher.as_ref() else { return };
    let theme = ui.ctx.theme;
    let out = ui.out;
    let g = theme.geometry.grid;
    let appear = sw.appear.value.clamp(0.0, 1.0) as f32;
    let ids: Vec<u32> = sw.ids.iter().copied().filter(|id| ui.ctx.wm.get(*id).is_some()).collect();
    if ids.is_empty() {
        return;
    }
    let selected = sw.ids.get(sw.index).copied();
    let thumb_h = (out.size.h * 0.16).clamp(96.0, 180.0);
    let max_w = thumb_h * 1.8;
    let sizes: Vec<(f64, f64)> = ids
        .iter()
        .map(|id| {
            let s = ui.ctx.wm.get(*id).unwrap().size();
            let k = (thumb_h / s.h.max(1) as f64).min(max_w / s.w.max(1) as f64);
            (s.w as f64 * k, s.h as f64 * k)
        })
        .collect();
    let pad = g * 2.0;
    let gap = g * 2.0;
    let title_h = g * 4.0;
    let card_w = |w: f64| w.max(thumb_h * 0.6) + pad;
    let mut total_w = sizes.iter().map(|(w, _)| card_w(*w)).sum::<f64>() + gap * (ids.len() - 1) as f64 + pad * 2.0;
    let k = if total_w > out.size.w - g * 8.0 { (out.size.w - g * 8.0) / total_w } else { 1.0 };
    total_w *= k;
    let total_h = (thumb_h + pad * 3.0) * k + title_h;
    let bg = layout::rect(out.loc.x + (out.size.w - total_w) / 2.0, out.loc.y + (out.size.h - total_h) / 2.0, total_w, total_h);
    ui.hit(bg, Hit::Switcher);
    // Title of the selected window under the thumbnails.
    if let Some(title) = selected.and_then(|id| ui.ctx.wm.get(id)).map(|w| if w.title().is_empty() { w.app_id() } else { w.title() }) {
        let style = Style::new(14.0, theme.colors.text).weight(Weight::Medium).align(Align::Center);
        let tw = bg.size.w - pad * 2.0;
        ui.text(&title, &style, Point::from((bg.loc.x + pad, bg.loc.y + bg.size.h - title_h - g * 0.5)), Some(tw), Some(1), appear);
    }
    let mut x = bg.loc.x + pad * k;
    let mut thumbs = Vec::new();
    let mut sel_pos = None;
    for (id, (w, h)) in ids.iter().zip(&sizes) {
        let cw = card_w(*w) * k;
        let card = layout::rect(x, bg.loc.y + pad * k, cw, (thumb_h + pad) * k);
        let (tw, th) = (w * k, h * k);
        let thumb = layout::rect(card.loc.x + (cw - tw) / 2.0, card.loc.y + pad / 2.0 * k + (thumb_h * k - th) / 2.0, tw, th);
        thumbs.push((*id, ui.phys(thumb)));
        if Some(*id) == selected {
            sel_pos = Some(ui.elements.len());
            ui.rect("sw-sel", 0, card, theme.geometry.popover_radius * 0.75, theme.colors.surface_hover, Some((theme.colors.accent.with_alpha(0.9), 2.0)), appear);
        }
        x += cw + gap * k;
    }
    // Thumbnails go in front of the selection card (pushed before the
    // background, so in front of it as well).
    // (Only if the selection card was really drawn.)
    let pos = sel_pos.filter(|&p| p < ui.elements.len()).unwrap_or(ui.elements.len());
    let mut thumb_elems = Vec::new();
    for (id, rect) in thumbs {
        crate::render::thumbnail_elements(ui.r, ui.ctx, id, rect, theme.geometry.window_radius * 0.5 * ui.scale, appear, 1, &mut thumb_elems);
    }
    let tail: Vec<LumenElement> = ui.elements.drain(pos..).collect();
    ui.elements.extend(thumb_elems);
    ui.elements.extend(tail);
    ui.glass("sw-bg", 0, bg, theme.geometry.popover_radius, theme.colors.glass_surface, Some((theme.colors.border, 1.0)), appear);
}

/// Overview chrome: search entry, app tiles (search results or the app grid),
/// workspace thumbnails and window titles.
fn overview_ui(ui: &mut Ui, t: f64) {
    let theme = ui.ctx.theme;
    let out = ui.out;
    let g = theme.geometry.grid;
    let alpha = t as f32;

    // Search entry.
    let s = layout::search_rect(theme, out);
    ui.hit(s, Hit::Search);
    let query = ui.ctx.shell.query.clone();
    let style = Style::new(15.0, if query.is_empty() { theme.colors.text_dim } else { theme.colors.text });
    let icon = layout::rect(s.loc.x + g * 1.5, s.loc.y + (s.size.h - g * 2.25) / 2.0, g * 2.25, g * 2.25);
    let text_x = icon.loc.x + icon.size.w + g;
    let shown = if query.is_empty() { "Type to search" } else { &query };
    let (tw, th) = ui.measure(shown, &style, None, None);
    let max_text = s.size.w - (text_x - s.loc.x) - g * 2.0;
    ui.text(shown, &style, Point::from((text_x, s.loc.y + (s.size.h - th) / 2.0)), Some(max_text), Some(1), alpha);
    // Caret after the query (the entry always has the keyboard).
    if !query.is_empty() || ui.ctx.shell.overview_open {
        let cx = if query.is_empty() { text_x } else { text_x + tw.min(max_text) + 1.0 };
        let caret = layout::rect(cx, s.loc.y + (s.size.h - th * 0.9) / 2.0, 1.5, th * 0.9);
        ui.rect("caret", 0, caret, 0.0, theme.colors.accent, None, alpha);
    }
    ui.icon("search", icon, alpha * 0.7, "");
    ui.rect("search", 0, s, s.size.h / 2.0, theme.colors.surface, Some((theme.colors.border, 1.0)), alpha);

    // Workspace thumbnails with live miniature windows.
    let thumbs = layout::workspace_thumbs(theme, out);
    let current = ui.ctx.wm.current;
    for (ws, r) in thumbs.iter().enumerate() {
        ui.hit(*r, Hit::Workspace(ws));
        let k = r.size.w / out.size.w;
        let ids: Vec<(u32, Rect)> = ui
            .ctx
            .wm
            .windows
            .iter()
            .rev()
            .filter(|w| w.mapped && w.ws == ws)
            .map(|w| {
                let gw = w.target_geo();
                (w.id, layout::rect(r.loc.x + (gw.loc.x - out.loc.x) * k, r.loc.y + (gw.loc.y - out.loc.y) * k, gw.size.w * k, gw.size.h * k))
            })
            .collect();
        let mut mini = Vec::new();
        for (id, wr) in ids {
            let p = ui.phys(wr);
            crate::render::thumbnail_elements(ui.r, ui.ctx, id, p, 2.0 * ui.scale, alpha, 2, &mut mini);
        }
        ui.elements.extend(mini);
        let border = if ws == current { Some((theme.colors.accent, 2.0)) } else { Some((theme.colors.border, 1.0)) };
        let hover = ui.ctx.shell.hover == Some(Hit::Workspace(ws));
        let fill = if hover { theme.colors.surface_hover.with_alpha(0.55) } else { theme.colors.surface.with_alpha(0.35) };
        ui.rect("ws-thumb", ws as u64, *r, g * 1.0, fill, border, alpha);
    }

    // App tiles: search results (a row), or the whole app grid.
    let area = ui.ctx.shell.results_area(theme, out);
    let apps = ui.ctx.shell.visible_apps();
    let apps_t = if ui.ctx.shell.query.trim().is_empty() { ui.ctx.shell.apps_anim.value.clamp(0.0, 1.0) } else { 1.0 };
    if !apps.is_empty() && apps_t > 0.0 {
        let tiles = layout::app_tiles(theme, area, apps.len());
        let a = alpha * apps_t as f32;
        let selected = ui.ctx.shell.selected;
        let label_style = Style::new(12.5, theme.colors.text).align(Align::Center);
        for (n, (app, tile)) in apps.iter().zip(&tiles).enumerate() {
            let Some(info) = ui.ctx.shell.apps.get(*app).cloned() else { continue };
            ui.hit(*tile, Hit::App(*app));
            let isz = g * 8.0;
            let icon = layout::rect(tile.loc.x + (tile.size.w - isz) / 2.0, tile.loc.y + g * 1.5, isz, isz);
            let name = super::icon_name(&info);
            ui.icon(&name, icon, a, &info.name);
            ui.text(&info.name, &label_style, Point::from((tile.loc.x + g * 0.5, icon.loc.y + isz + g)), Some(tile.size.w - g), Some(1), a);
            if n == selected || ui.ctx.shell.hover == Some(Hit::App(*app)) {
                ui.rect("tile-sel", n as u64, *tile, g * 2.0, theme.colors.surface_hover.with_alpha(0.8), None, a);
            }
        }
        ui.ctx.shell.shapes.retain(|k, i| k != "tile-sel" || (i as usize) < apps.len());
    } else if !ui.ctx.shell.query.trim().is_empty() && ui.ctx.shell.results.is_empty() {
        let style = Style::new(15.0, theme.colors.text_dim).align(Align::Center);
        ui.text("No apps found", &style, Point::from((area.loc.x, area.loc.y + g * 4.0)), Some(area.size.w), Some(1), alpha);
    }

    // Window titles under their overview slots (not in app grid mode).
    let windows_t = 1.0 - ui.ctx.shell.apps_anim.value.clamp(0.0, 1.0);
    if windows_t > 0.0 && apps.is_empty() {
        if let Some(slots) = ui.ctx.shell.overview_layout(ui.ctx.wm, theme, out) {
            let hovered = ui.ctx.shell.hovered;
            let mut slots: Vec<(u32, Rect)> = slots.into_iter().collect();
            slots.sort_by_key(|(id, _)| *id);
            for (id, slot) in slots {
                let Some(w) = ui.ctx.wm.get(id) else { continue };
                let title = if w.title().is_empty() { w.app_id() } else { w.title() };
                let color = if Some(id) == hovered { theme.colors.text } else { theme.colors.text_dim };
                let style = Style::new(12.5, color).align(Align::Center);
                let a = alpha * (((t - 0.5) * 2.0).clamp(0.0, 1.0) * windows_t) as f32;
                let tw = (slot.size.w + g * 4.0).min(g * 40.0);
                ui.text(&title, &style, Point::from((slot.loc.x + (slot.size.w - tw) / 2.0, slot.loc.y + slot.size.h + g)), Some(tw), Some(1), a);
            }
        }
    }
}

/// Shape keys of the cheat sheet (dropped when it is gone).
const SHEET_SHAPES: [&str; 9] =
    ["sheet-cap", "sheet-row", "sheet-sep", "sheet-search", "sheet-caret", "sheet-bg", "sheet-dim", "sheet-thumb", "sheet-track"];

/// Measured layout of one cheat-sheet row.
struct RowLayout {
    row: usize,
    /// Width the label may use and its height (it wraps to two lines).
    label_max: f64,
    label_h: f64,
    /// Key cap widths per chord, and the chords packed into lines.
    caps: Vec<Vec<f64>>,
    lines: Vec<Vec<usize>>,
    line_w: Vec<f64>,
    h: f64,
}

/// A group of rows under one heading.
struct GroupLayout {
    name: String,
    rows: Vec<RowLayout>,
}

struct SheetMetrics {
    cap_h: f64,
    cap_pad: f64,
    cap_gap: f64,
    or_w: f64,
    line_gap: f64,
    group_head: f64,
    row_min: f64,
}

fn sheet_styles(theme: &crate::theme::Theme) -> (Style, Style, Style, Style, Style) {
    let c = &theme.colors;
    (
        Style::new(19.0, c.text).weight(Weight::SemiBold),
        Style::new(11.5, c.text_dim).weight(Weight::SemiBold),
        Style::new(13.5, c.text),
        Style::new(12.0, c.text).weight(Weight::Medium),
        Style::new(11.5, c.text_dim),
    )
}

/// Measure the rows `which` of the sheet, grouped, for columns `col_w` wide.
fn sheet_groups(ui: &mut Ui, rows: &[super::sheet::SheetRow], which: &[usize], col_w: f64, m: &SheetMetrics) -> Vec<GroupLayout> {
    let (_, _, label_style, cap_style, _) = sheet_styles(ui.ctx.theme);
    let g = ui.ctx.theme.geometry.grid;
    let mut groups: Vec<GroupLayout> = Vec::new();
    for &i in which {
        let row = &rows[i];
        let caps: Vec<Vec<f64>> = row
            .chords
            .iter()
            .map(|c| c.iter().map(|k| (ui.measure(k, &cap_style, None, Some(1)).0 + m.cap_pad * 2.0).max(m.cap_h).ceil()).collect())
            .collect();
        let chord_w: Vec<f64> = caps.iter().map(|c| c.iter().sum::<f64>() + m.cap_gap * (c.len().max(1) - 1) as f64).collect();
        let label_w = ui.measure(&row.label, &label_style, None, Some(1)).0;
        let keys_max = col_w - label_w.min(col_w * 0.5) - g * 2.0;
        let lines = super::sheet::pack(&chord_w, keys_max, m.or_w);
        let line_w: Vec<f64> = lines.iter().map(|l| l.iter().map(|&c| chord_w[c]).sum::<f64>() + m.or_w * (l.len() - 1) as f64).collect();
        let keys_w = line_w.iter().copied().fold(0.0, f64::max);
        let label_max = (col_w - keys_w - g * 2.0).max(g * 8.0);
        let (_, label_h) = ui.measure(&row.label, &label_style, Some(label_max), Some(2));
        let n = lines.len() as f64;
        let h = (n * m.cap_h + (n - 1.0) * m.line_gap + g).max(label_h + g * 1.5).max(m.row_min);
        let layout = RowLayout { row: i, label_max, label_h, caps, lines, line_w, h };
        match groups.last_mut().filter(|gr| gr.name == row.group) {
            Some(gr) => gr.rows.push(layout),
            None => groups.push(GroupLayout { name: row.group.clone(), rows: vec![layout] }),
        }
    }
    groups
}

/// The keyboard-shortcut cheat sheet: a centered glass panel with the
/// binding table in grouped columns, key caps, and a filter field.
fn sheet(ui: &mut Ui) {
    let Some(sh) = ui.ctx.shell.sheet.as_ref() else { return };
    let t = sh.anim.value.clamp(0.0, 1.1);
    if t <= 0.001 {
        return;
    }
    let (rows, visible, query, selected, scroll, open) =
        (sh.rows.clone(), sh.visible.clone(), sh.query.clone(), sh.selected_row(), sh.scroll.value, sh.open);
    let theme = ui.ctx.theme;
    let c = &theme.colors;
    let g = theme.geometry.grid;
    let out = ui.out;
    let alpha = t.min(1.0) as f32;
    let (title_style, group_style, label_style, cap_style, or_style) = sheet_styles(theme);
    let or_text_w = ui.measure("or", &or_style, None, Some(1)).0;
    let m = SheetMetrics {
        cap_h: g * 3.0,
        cap_pad: g,
        cap_gap: g * 0.5,
        or_w: or_text_w + g * 2.0,
        line_gap: g * 0.75,
        group_head: g * 3.5,
        row_min: g * 4.0,
    };

    // Panel size from the unfiltered table, so it does not jump while typing.
    let margin = g * 4.0;
    let pad = g * 4.0;
    let col_gap = g * 5.0;
    let all: Vec<usize> = (0..rows.len()).collect();
    let n_groups = {
        let mut names: Vec<&str> = rows.iter().map(|r| r.group.as_str()).collect();
        names.dedup();
        names.len().max(1)
    };
    let max_w = (out.size.w - margin * 2.0).min(g * 150.0);
    let min_col = g * 38.0;
    let cols = (((max_w - pad * 2.0 + col_gap) / (min_col + col_gap)).floor() as usize).clamp(1, n_groups);
    let col_w = ((max_w - pad * 2.0 - col_gap * (cols - 1) as f64) / cols as f64).min(g * 48.0).floor();
    let panel_w = col_w * cols as f64 + col_gap * (cols - 1) as f64 + pad * 2.0;
    let group_gap = g * 3.0;
    let flow_items = |groups: &[GroupLayout]| -> Vec<FlowItem> {
        groups.iter().flat_map(|gr| sheet::group_items(&gr.rows.iter().map(|r| r.h).collect::<Vec<_>>(), m.group_head)).collect()
    };
    let all_items = flow_items(&sheet_groups(ui, &rows, &all, col_w, &m));
    let column_h = sheet::balanced_height(&all_items, cols, m.group_head, group_gap);
    let all_h = sheet::flow(&all_items, cols, column_h, m.group_head, group_gap).tallest;

    // Header: icon and title, filter field to the right (or below when narrow).
    let title = "Keyboard Shortcuts";
    let (title_w, title_h) = ui.measure(title, &title_style, None, Some(1));
    let icon_sz = g * 4.0;
    let title_block = icon_sz + g * 1.5 + title_w;
    let field_h = g * 5.0;
    let stacked = panel_w - pad * 2.0 < title_block + g * 4.0 + g * 26.0;
    let header_h = if stacked { field_h + g * 1.5 + field_h } else { field_h };
    let chrome_h = pad + header_h + g * 3.0 + pad;
    let max_h = out.size.h - theme.geometry.panel_height - margin * 2.0;
    let view_h = all_h.min((max_h - chrome_h).max(g * 12.0)).max(g * 12.0);
    let panel_h = chrome_h + view_h;
    let px = ui.snap(out.loc.x + (out.size.w - panel_w) / 2.0);
    let py = ui.snap((out.loc.y + (out.size.h - panel_h) / 2.0).max(out.loc.y + theme.geometry.panel_height + g));
    let panel = layout::rect(px, py, panel_w, panel_h);

    // Grow in from 96 % about the panel's center.
    let k = 0.96 + 0.04 * t.min(1.0);
    let (cx, cy) = (px + panel_w / 2.0, py + panel_h / 2.0);
    let tf = |r: Rect| layout::rect(cx + (r.loc.x - cx) * k, cy + (r.loc.y - cy) * k, r.size.w * k, r.size.h * k);
    let tp = |x: f64, y: f64| Point::<f64, Logical>::from((cx + (x - cx) * k, cy + (y - cy) * k));

    // Content: the matching rows flowed into the columns, read top to
    // bottom, left to right (a filtered list stays in the first column).
    let view_top = py + pad + header_h + g * 3.0;
    let view_bottom = view_top + view_h;
    let groups = sheet_groups(ui, &rows, &visible, col_w, &m);
    let flowed = sheet::flow(&flow_items(&groups), cols, column_h, m.group_head, group_gap);
    let (placed, content_h) = (&flowed.pos, flowed.tallest);
    let fade = g * 2.5;
    let edge_alpha = |top: f64, bottom: f64| -> f32 {
        let a_top = 1.0 - ((view_top - top) / fade).clamp(0.0, 1.0);
        let a_bottom = 1.0 - ((bottom - view_bottom) / fade).clamp(0.0, 1.0);
        (a_top * a_bottom) as f32
    };
    let hover = match ui.ctx.shell.hover {
        Some(Hit::SheetRow(r)) => Some(r),
        _ => None,
    };
    let one_line = ui.measure("Ag", &label_style, None, Some(1)).1;
    let mut spans = Vec::new();
    let mut highlights: Vec<(Rect, bool, f32)> = Vec::new();
    let mut seps: Vec<(Rect, f32)> = Vec::new();
    let mut cap_i = 0u64;
    let mut item = 0;
    for gr in &groups {
        let heading = gr.name.to_uppercase();
        let (_, gh) = ui.measure(&heading, &group_style, None, Some(1));
        let draw_heading = |ui: &mut Ui, col: usize, top: f64| {
            let (x, y) = (px + pad + col as f64 * (col_w + col_gap), view_top - scroll + top);
            let a = edge_alpha(y, y + gh) * alpha;
            if a > 0.0 {
                ui.text(&heading, &group_style, tp(x, y + g * 0.5), Some(col_w), Some(1), a);
            }
        };
        let Some(&(col, gy)) = placed.get(item) else { break };
        draw_heading(ui, col, gy);
        item += 1;
        for (ri, rl) in gr.rows.iter().enumerate() {
            let Some(&(col, ry)) = placed.get(item) else { break };
            if flowed.continued.contains(&item) {
                draw_heading(ui, col, ry - m.group_head);
            }
            let next_col = placed.get(item + 1).map(|p| p.0);
            item += 1;
            let x = px + pad + col as f64 * (col_w + col_gap);
            let y = view_top - scroll + ry;
            let row = &rows[rl.row];
            spans.push((rl.row, y - (view_top - scroll), y + rl.h - (view_top - scroll)));
            let a = edge_alpha(y, y + rl.h) * alpha;
            let row_rect = layout::rect(x - g, y, col_w + g * 2.0, rl.h);
            if a > 0.0 {
                // A one-line label sits level with the first line of keys; a
                // wrapped one and the keys are centered in the row.
                let keys_h = rl.lines.len() as f64 * (m.cap_h + m.line_gap) - m.line_gap;
                let keys_top = if rl.label_h > one_line * 1.5 { y + (rl.h - keys_h) / 2.0 } else { y + g * 0.5 };
                let label_top = if rl.label_h > one_line * 1.5 { y + (rl.h - rl.label_h) / 2.0 } else { keys_top + (m.cap_h - rl.label_h) / 2.0 };
                ui.text(&row.label, &label_style, tp(x, label_top), Some(rl.label_max), Some(2), a);
                for (li, line) in rl.lines.iter().enumerate() {
                    let ly = ui.snap(keys_top + li as f64 * (m.cap_h + m.line_gap));
                    let mut kx = x + col_w - rl.line_w[li];
                    for (n, &chord) in line.iter().enumerate() {
                        if n > 0 {
                            let (ow, oh) = ui.measure("or", &or_style, None, Some(1));
                            ui.text("or", &or_style, tp(kx + (m.or_w - ow) / 2.0, ly + (m.cap_h - oh) / 2.0), None, Some(1), a);
                            kx += m.or_w;
                        }
                        for (cap, &w) in row.chords[chord].iter().zip(&rl.caps[chord]) {
                            let r = layout::rect(ui.snap(kx), ly, w, m.cap_h);
                            let (tw, th) = ui.measure(cap, &cap_style, None, Some(1));
                            ui.text(cap, &cap_style, tp(r.loc.x + (w - tw) / 2.0, r.loc.y + (m.cap_h - th) / 2.0), None, Some(1), a);
                            let top = Color::rgba(1.0, 1.0, 1.0, 0.13);
                            let bottom = Color::rgba(1.0, 1.0, 1.0, 0.07);
                            ui.rect_gradient("sheet-cap", cap_i, tf(r), g * 0.75 * k, (top, bottom), Some((Color::rgba(1.0, 1.0, 1.0, 0.16), 1.0)), a);
                            cap_i += 1;
                            kx += w + m.cap_gap;
                        }
                        kx -= m.cap_gap;
                    }
                }
                let on = Some(rl.row) == selected;
                if on || hover == Some(rl.row) {
                    highlights.push((row_rect, on, a));
                }
                // Separators between rows of a group, within a column.
                if ri + 1 < gr.rows.len() && next_col == Some(col) {
                    seps.push((layout::rect(x, y + rl.h - 0.5, col_w, 1.0), a));
                }
                if a > 0.5 {
                    let vis = layout::rect(row_rect.loc.x, row_rect.loc.y.max(view_top), row_rect.size.w, (y + rl.h).min(view_bottom) - y.max(view_top));
                    if vis.size.h > 0.0 && open {
                        ui.hit(tf(vis), Hit::SheetRow(rl.row));
                    }
                }
            }
        }
    }
    for (i, (r, on, a)) in highlights.into_iter().enumerate() {
        let fill = if on { c.accent.with_alpha(0.22) } else { Color::rgba(1.0, 1.0, 1.0, 0.06) };
        let border = on.then_some((c.accent.with_alpha(0.45), 1.0));
        ui.rect("sheet-row", i as u64, tf(r), g * k, fill, border, a);
    }
    for (i, (r, a)) in seps.into_iter().enumerate() {
        ui.rect("sheet-sep", i as u64, tf(r), 0.0, Color::rgba(1.0, 1.0, 1.0, 0.06), None, a);
    }
    if visible.is_empty() {
        let style = Style::new(14.0, c.text_dim).align(Align::Center);
        let msg = format!("No shortcuts match \u{201c}{}\u{201d}", query.trim());
        ui.text(&msg, &style, tp(px + pad, view_top + g * 4.0), Some(panel_w - pad * 2.0), Some(1), alpha);
    }
    if let Some(s) = ui.ctx.shell.sheet.as_mut() {
        s.content_h = content_h;
        s.view_h = view_h;
        s.row_spans = spans;
    }
    // A thin scroll indicator in the right margin when the content overflows.
    if content_h > view_h + 0.5 {
        let track = layout::rect(px + panel_w - pad / 2.0 - g * 0.25, view_top, g * 0.5, view_h);
        let thumb_h = (view_h * view_h / content_h).max(g * 4.0);
        let max_scroll = (content_h - view_h).max(1.0);
        let thumb_y = view_top + (scroll / max_scroll).clamp(0.0, 1.0) * (view_h - thumb_h);
        let thumb = layout::rect(track.loc.x, thumb_y, track.size.w, thumb_h);
        ui.rect("sheet-thumb", 0, tf(thumb), g * 0.25 * k, Color::rgba(1.0, 1.0, 1.0, 0.32), None, alpha);
        ui.rect("sheet-track", 0, tf(track), g * 0.25 * k, Color::rgba(1.0, 1.0, 1.0, 0.07), None, alpha);
    }

    // Header.
    let hy = py + pad;
    ui.icon("input-keyboard", tf(layout::rect(px + pad, hy + (field_h - icon_sz) / 2.0, icon_sz, icon_sz)), alpha, "");
    ui.text(title, &title_style, tp(px + pad + icon_sz + g * 1.5, hy + (field_h - title_h) / 2.0), None, Some(1), alpha);
    let field = if stacked {
        layout::rect(px + pad, hy + field_h + g * 1.5, panel_w - pad * 2.0, field_h)
    } else {
        let w = (g * 36.0).min(panel_w - pad * 2.0 - title_block - g * 4.0);
        layout::rect(px + panel_w - pad - w, hy, w, field_h)
    };
    let search_style = Style::new(14.0, if query.is_empty() { c.text_dim } else { c.text });
    let icon = layout::rect(field.loc.x + g * 1.5, field.loc.y + (field_h - g * 2.25) / 2.0, g * 2.25, g * 2.25);
    let text_x = icon.loc.x + icon.size.w + g;
    let shown = if query.is_empty() { "Type to filter" } else { &query };
    let max_text = field.size.w - (text_x - field.loc.x) - g * 2.0;
    let (qw, qh) = ui.measure(shown, &search_style, Some(max_text), Some(1));
    ui.text(shown, &search_style, tp(text_x, field.loc.y + (field_h - qh) / 2.0), Some(max_text), Some(1), alpha);
    let caret_x = if query.is_empty() { text_x } else { text_x + qw.min(max_text) + 1.0 };
    ui.rect("sheet-caret", 0, tf(layout::rect(caret_x, field.loc.y + (field_h - qh * 0.9) / 2.0, 1.5, qh * 0.9)), 0.0, c.accent, None, alpha);
    ui.icon("search", tf(icon), alpha * 0.7, "");
    ui.rect("sheet-search", 0, tf(field), field_h / 2.0 * k, c.surface.with_alpha(0.7), Some((c.border, 1.0)), alpha);

    // The panel and a dim layer over the desktop (it is modal).
    let p = tf(panel);
    if open {
        ui.hit(p, Hit::Sheet);
    }
    // The frosted backdrop leads the content in (and trails it out), so what
    // is behind is blurred before the text is legible.
    let backdrop = (t * 1.7).min(1.0) as f32;
    ui.glass("sheet-bg", 0, p, theme.geometry.popover_radius * 1.5 * k, c.glass_surface, Some((c.border, 1.0)), backdrop);
    ui.rect("sheet-dim", 0, out, 0.0, Color::rgba(0.0, 0.0, 0.0, 0.32), None, backdrop);
}

/// Volume OSD: a glass pill near the bottom with icon, level bar and value.
fn osd(ui: &mut Ui) {
    let Some(o) = ui.ctx.shell.osd.as_ref() else { return };
    let t = o.anim.value.clamp(0.0, 1.1);
    if t <= 0.001 {
        return;
    }
    let (level, bar, muted, icon_name) = (o.level, o.bar.value.max(0.0), o.muted, o.icon());
    let theme = ui.ctx.theme;
    let c = &theme.colors;
    let g = theme.geometry.grid;
    let out = ui.out;
    let alpha = t.min(1.0) as f32;
    let (w, h) = (g * 34.0, g * 6.0);
    let dy = (1.0 - t.min(1.0)) * g * 2.0;
    let x = ui.snap(out.loc.x + (out.size.w - w) / 2.0);
    let y = ui.snap(out.loc.y + out.size.h - layout::dock_height(theme) - g * 6.0 - h + dy);
    let pill = layout::rect(x, y, w, h);
    let isz = g * 3.0;
    ui.icon(icon_name, layout::rect(x + g * 2.0, y + (h - isz) / 2.0, isz, isz), alpha, "");
    let value_style = Style::new(13.0, c.text).weight(Weight::SemiBold).align(Align::Right);
    let value_w = g * 6.0;
    let text = if muted { "Muted".to_string() } else { format!("{}%", (level * 100.0).round() as i64) };
    let (_, th) = ui.measure(&text, &value_style, Some(value_w), Some(1));
    ui.text(&text, &value_style, Point::from((x + w - g * 2.0 - value_w, y + (h - th) / 2.0)), Some(value_w), Some(1), alpha);
    // Track 0..100 %, or 0..150 % while over-amplified (with a mark at 100 %).
    let bx = x + g * 2.0 + isz + g * 1.5;
    let bw = x + w - g * 2.0 - value_w - g * 1.5 - bx;
    let bh = g * 0.75;
    let by = ui.snap(y + (h - bh) / 2.0);
    let max = if level > 1.0 + 1e-6 { 1.5 } else { 1.0 };
    let frac = (bar / max).clamp(0.0, 1.0);
    let fill_a = if muted { alpha * 0.35 } else { alpha };
    if max > 1.0 {
        let mark = bx + bw / max;
        ui.rect("osd-mark", 0, layout::rect(mark - 0.75, by - g * 0.5, 1.5, bh + g), 0.75, c.text.with_alpha(0.6), None, alpha);
        let over = (bx + bw * frac - mark).max(0.0);
        if over > 0.0 {
            ui.rect("osd-over", 0, layout::rect(mark, by, over.max(bh), bh), bh / 2.0, c.danger, None, fill_a);
        }
    }
    let normal_w = (bw * frac).min(bw / max);
    if normal_w > 0.0 {
        ui.rect("osd-fill", 0, layout::rect(bx, by, normal_w.max(bh), bh), bh / 2.0, c.accent, None, fill_a);
    }
    ui.rect("osd-track", 0, layout::rect(bx, by, bw, bh), bh / 2.0, Color::rgba(1.0, 1.0, 1.0, 0.16), None, alpha);
    ui.glass("osd-bg", 0, pill, h / 2.0, c.glass_surface, Some((c.border, 1.0)), alpha);
}

/// Frame-time HUD: render time (bars) and presentation intervals (line) of the
/// last frames, with 60 Hz budget marks. Drawn on the CPU into a small image.
fn hud(ui: &mut Ui) {
    const W: f64 = 240.0;
    const H: f64 = 72.0;
    let pw = (W * ui.scale).round() as usize;
    let ph = (H * ui.scale).round() as usize;
    let stats = ui.ctx.stats.clone();
    let mut mem = vec![0u8; pw * ph * 4];
    let put = |mem: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
        if x < pw && y < ph {
            let i = (y * pw + x) * 4;
            mem[i..i + 4].copy_from_slice(&c);
        }
    };
    for px in mem.chunks_exact_mut(4) {
        px.copy_from_slice(&[8, 8, 11, 200]);
    }
    // 0..33 ms vertical range; marks at 8.3 and 16.7 ms.
    let y_of = |ms: f32| (ph as f32 - 1.0 - ms / 33.3 * (ph as f32 - 1.0)).clamp(0.0, ph as f32 - 1.0) as usize;
    for x in 0..pw {
        put(&mut mem, x, y_of(16.7), [90, 90, 110, 255]);
        if x % 4 == 0 {
            put(&mut mem, x, y_of(8.3), [60, 60, 75, 255]);
        }
    }
    let n = pw.min(stats.render_ms.len());
    for (i, ms) in stats.render_ms[stats.render_ms.len() - n..].iter().enumerate() {
        let x = pw - n + i;
        let color = if *ms > 16.7 { [230, 90, 90, 255] } else if *ms > 8.3 { [230, 190, 90, 255] } else { [110, 200, 130, 255] };
        for y in y_of(*ms)..ph {
            put(&mut mem, x, y, color);
        }
    }
    let n = pw.min(stats.interval_ms.len());
    for (i, ms) in stats.interval_ms[stats.interval_ms.len() - n..].iter().enumerate() {
        let x = pw - n + i;
        let c = if *ms > 25.0 { [255, 80, 80, 255] } else { [122, 162, 247, 255] };
        put(&mut mem, x, y_of(*ms), c);
        put(&mut mem, x, y_of(*ms).saturating_sub(1), c);
    }
    let g = ui.ctx.theme.geometry.grid;
    let x = ((ui.out.size.w - W - g * 2.0) * ui.scale).round() as i32;
    let y = ((ui.ctx.theme.geometry.panel_height + g * 2.0) * ui.scale).round() as i32;
    let key = ui.output.clone();
    let img = match ui.ctx.shell.shapes.hud.remove(&key) {
        Some(mut img) if img.size.w == pw as i32 && img.size.h == ph as i32 => {
            let _ = img.update(ui.r, &mem, pw as u32, ph as u32);
            Some(img)
        }
        _ => Image::upload(ui.r, &mem, pw as u32, ph as u32).ok(),
    };
    if let Some(mut img) = img {
        ui.elements.push(LumenElement::Image(img.element((x, y).into(), 1.0)));
        ui.ctx.shell.shapes.hud.insert(key, img);
    }
}
