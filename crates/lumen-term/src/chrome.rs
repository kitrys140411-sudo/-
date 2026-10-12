//! Window chrome: the header bar (tabs, title, buttons), pane overlays
//! (search bar, scrollbar, inactive-pane shade) and split dividers.

use std::time::Instant;

use lumen_term::layout::{PaneId, Rect};

use crate::app::{App, HeaderHits};
use crate::gfx::{color, RectInst};
use crate::view::{self, key_of, IRect};

type Rgb = (u8, u8, u8);

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

pub fn header_bg(app: &App) -> Rgb {
    mix(app.cfg.colors.background.tuple(), (255, 255, 255), 0.045)
}

pub fn header(app: &mut App, bw: i32) {
    let h = app.header_height();
    if h <= 0.0 {
        app.header_hits = HeaderHits::default();
        return;
    }
    let s = app.scale();
    let bg = header_bg(app);
    let term_bg = app.cfg.colors.background.tuple();
    let fg = app.cfg.colors.foreground.tuple();
    let dim = mix(fg, bg, 0.45);
    let accent = (0x7a, 0xa2, 0xf7);
    let titles: Vec<String> = app.tabs.iter().map(|t| app.pane_title(t.focus)).collect();
    let show_tabs = titles.len() > 1 || app.cfg.window.always_show_tabs;
    let hover = app.hover;
    let focused = app.kb_focus;
    let rect = IRect { x: 0, y: 0, w: bw, h: h as i32 };
    let hash = key_of((&titles, app.active, hover, focused, show_tabs, bw, h as i32, s.to_bits()));
    let (ascent, descent) = app.fonts.ui_metrics();
    let baseline = ((h - (ascent + descent)) / 2.0 + ascent).round();
    let mut hits = HeaderHits::default();
    let text_col = color(if focused { fg } else { dim }, 1.0);
    {
        let (scene, _, _, damage) = app.scene_mut();
        damage.region(key_of("header"), rect, hash);
        scene.layers[0].rects.push(RectInst::fill(0.0, 0.0, bw as f32, h, color(bg, 1.0)));
        scene.layers[0].rects.push(RectInst::fill(0.0, h - s.round().max(1.0), bw as f32, s.round().max(1.0), color(mix(bg, (0, 0, 0), 0.35), 1.0)));
    }
    // Close button (right).
    let br = (h * 0.30).round();
    let (cx, cy) = (bw as f32 - h / 2.0 - 2.0 * s, h / 2.0);
    let close = Rect::new(cx - br, cy - br, br * 2.0, br * 2.0);
    hits.close = Some(close);
    {
        let c = if hover.close { color(mix(bg, (0xf2, 0x6d, 0x78), 0.85), 1.0) } else { color(mix(bg, fg, 0.12), 1.0) };
        app.pill(0, close, br, c);
        let t = (1.4 * s).max(1.0);
        let arm = br * 0.42;
        // "×" from two thin rotated bars approximated by small steps.
        let n = (arm * 2.0).ceil() as i32;
        let xc = if hover.close { color((255, 255, 255), 1.0) } else { text_col };
        for i in 0..=n {
            let d = -arm + 2.0 * arm * i as f32 / n as f32;
            app.pill(1, Rect::new(cx + d - t / 2.0, cy + d - t / 2.0, t, t), t / 2.0, xc);
            app.pill(1, Rect::new(cx + d - t / 2.0, cy - d - t / 2.0, t, t), t / 2.0, xc);
        }
    }
    let right_limit = close.x - 8.0 * s;
    if show_tabs {
        let n = titles.len();
        let plus_w = h * 0.8;
        let avail = right_limit - 8.0 * s - plus_w - 8.0 * s;
        let tab_w = (avail / n as f32).min(240.0 * s).floor();
        let tab_h = (h - 10.0 * s).round();
        let ty = ((h - tab_h) / 2.0).round();
        let mut x = 8.0 * s;
        for (i, title) in titles.iter().enumerate() {
            let r = Rect::new(x, ty, tab_w - 4.0 * s, tab_h);
            let active = i == app.active;
            if active {
                app.pill(0, r, 8.0 * s, color(term_bg, 1.0));
            } else if hover.tab == Some(i) {
                app.pill(0, r, 8.0 * s, color(mix(bg, fg, 0.07), 1.0));
            }
            let cr = (tab_h * 0.36).round();
            let close_r = Rect::new(r.x + r.w - cr * 2.0 - 6.0 * s, r.y + (r.h - cr * 2.0) / 2.0, cr * 2.0, cr * 2.0);
            let show_close = active || hover.tab == Some(i);
            let max_w = r.w - 24.0 * s - if show_close { cr * 2.0 + 6.0 * s } else { 0.0 };
            let col = color(if active { fg } else { dim }, 1.0);
            let (scene, fonts, atlas, _) = app.scene_mut();
            let tw = view::ui_width(fonts, title).min(max_w);
            let tx = (r.x + (r.w - tw) / 2.0).max(r.x + 12.0 * s);
            view::ui_text(fonts, atlas, scene, 1, title, tx, baseline, col, max_w);
            if show_close {
                if hover.tab_close == Some(i) {
                    app.pill(0, close_r, cr, color(mix(bg, fg, 0.15), 1.0));
                }
                let (scene, fonts, atlas, _) = app.scene_mut();
                let w = view::ui_width(fonts, "×");
                view::ui_text(fonts, atlas, scene, 1, "×", close_r.x + (close_r.w - w) / 2.0, baseline, col, 100.0);
            }
            hits.tabs.push((r, close_r));
            x += tab_w;
        }
        let plus = Rect::new(x + 2.0 * s, ty, plus_w, tab_h);
        if hover.new_tab {
            app.pill(0, plus, 8.0 * s, color(mix(bg, fg, 0.07), 1.0));
        }
        let (scene, fonts, atlas, _) = app.scene_mut();
        let w = view::ui_width(fonts, "+");
        view::ui_text(fonts, atlas, scene, 1, "+", plus.x + (plus.w - w) / 2.0, baseline, color(fg, 1.0), 100.0);
        hits.new_tab = Some(plus);
    } else {
        let title = titles.first().cloned().unwrap_or_default();
        let max_w = (right_limit - 2.0 * 60.0 * s).max(40.0);
        let (scene, fonts, atlas, _) = app.scene_mut();
        let w = view::ui_width(fonts, &title).min(max_w);
        view::ui_text(fonts, atlas, scene, 1, &title, ((bw as f32 - w) / 2.0).round(), baseline, text_col, max_w);
        // A "+" for a new tab, at the left.
        let plus = Rect::new(8.0 * s, (h * 0.15).round(), h * 0.7, (h * 0.7).round());
        if hover.new_tab {
            app.pill(0, plus, 8.0 * s, color(mix(bg, fg, 0.07), 1.0));
        }
        let (scene, fonts, atlas, _) = app.scene_mut();
        let pw = view::ui_width(fonts, "+");
        view::ui_text(fonts, atlas, scene, 1, "+", plus.x + (plus.w - pw) / 2.0, baseline, color(dim, 1.0), 100.0);
        hits.new_tab = Some(plus);
    }
    let _ = accent;
    app.header_hits = hits;
}

/// Search bar, scrollbar, shade of unfocused panes, bell flash.
pub fn pane_overlays(app: &mut App, id: PaneId, area: Rect, content: Rect, focused: bool, multi: bool, now: Instant) {
    let s = app.scale();
    let fg = app.cfg.colors.foreground.tuple();
    let bg = app.cfg.colors.background.tuple();
    let Some(p) = app.panes.get(&id) else { return };
    let history = p.history() as f32;
    let offset = p.scroll.offset as f32;
    let rows = p.rows as f32;
    let moved = p.scroll.moved;
    let bell = p.bell;
    let search = p.search.as_ref().map(|s| (s.query.clone(), s.matches.len(), s.current));
    let area_i = IRect::of(area);

    // Unfocused panes are shaded a little (only with splits).
    let shade = multi && !focused;
    {
        let (scene, _, _, damage) = app.scene_mut();
        damage.region(key_of(("shade", id)), area_i, key_of((shade, area_i)));
        if shade {
            scene.layers[3].rects.push(RectInst::fill(area.x, area.y, area.w, area.h, color((0, 0, 0), 0.18)));
        }
    }
    // Bell: a brief flash.
    if let Some(b) = bell {
        let t = now.duration_since(b).as_secs_f32() / 0.25;
        if t < 1.0 {
            let a = 0.10 * (1.0 - t);
            let (scene, _, _, damage) = app.scene_mut();
            damage.add(area_i);
            scene.layers[3].rects.push(RectInst::fill(area.x, area.y, area.w, area.h, color(fg, a)));
        }
    }
    // Scrollbar: visible while scrolled back, fading out after scrolling.
    if history > 0.0 {
        let fade = moved.map(|m| (1.0 - (now.duration_since(m).as_secs_f32() - 0.8) / 0.6).clamp(0.0, 1.0)).unwrap_or(0.0);
        let alpha = if offset > 0.01 { 1.0f32.max(fade) } else { fade };
        let track_h = content.h;
        let total = history + rows;
        let thumb_h = (track_h * rows / total).max(24.0 * s);
        let pos = (history - offset) / history.max(1.0);
        let ty = content.y + (track_h - thumb_h) * pos;
        let w = (4.0 * s).round();
        let x = area.x + area.w - w - (3.0 * s).round();
        let r = Rect::new(x, ty, w, thumb_h);
        let ri = IRect::of(Rect::new(x - 1.0, content.y - 1.0, w + 2.0, track_h + 2.0));
        let (scene, _, _, damage) = app.scene_mut();
        damage.region(key_of(("scrollbar", id)), ri, key_of(((ty * 4.0) as i32, (thumb_h * 4.0) as i32, (alpha * 32.0) as i32)));
        if alpha > 0.0 {
            scene.layers[3].rects.push(RectInst::rounded(r.x, r.y, r.w, r.h, w / 2.0, color(fg, 0.35 * alpha)));
        }
    }
    // Search bar (top right of the pane).
    if let Some((query, n, current)) = search {
        let h = (30.0 * s).round();
        let w = (320.0 * s).round().min(area.w - 16.0 * s);
        let r = Rect::new(area.x + area.w - w - 10.0 * s, area.y + 8.0 * s, w, h);
        let ri = IRect::of(Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0));
        let count = if query.is_empty() {
            String::new()
        } else if n == 0 {
            "no matches".to_string()
        } else {
            format!("{}/{}", current.map(|c| c + 1).unwrap_or(0), n)
        };
        let (ascent, descent) = app.fonts.ui_metrics();
        let baseline = (r.y + (h - (ascent + descent)) / 2.0 + ascent).round();
        let panel = mix(bg, fg, 0.10);
        let (scene, fonts, atlas, damage) = app.scene_mut();
        damage.region(key_of(("search", id)), ri, key_of((&query, n, current, focused)));
        scene.layers[3].rects.push(RectInst::rounded(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0, h / 2.0 + 1.0, color(mix(panel, fg, 0.18), 1.0)));
        scene.layers[3].rects.push(RectInst::rounded(r.x, r.y, r.w, r.h, h / 2.0, color(panel, 1.0)));
        let cw = view::ui_width(fonts, &count);
        let tx = r.x + 14.0 * s;
        let max = r.w - 28.0 * s - cw - 8.0 * s;
        let shown = if query.is_empty() { "Search".to_string() } else { query.clone() };
        let qcol = if query.is_empty() { color(mix(fg, panel, 0.5), 1.0) } else { color(fg, 1.0) };
        let qw = view::ui_text(fonts, atlas, scene, 3, &shown, tx, baseline, qcol, max);
        if focused {
            // Caret after the query.
            let cx = if query.is_empty() { tx } else { tx + qw + 1.0 };
            scene.layers[3].rects.push(RectInst::fill(cx.round(), r.y + h * 0.22, (1.5 * s).max(1.0), h * 0.56, color((0x7a, 0xa2, 0xf7), 1.0)));
        }
        view::ui_text(fonts, atlas, scene, 3, &count, r.x + r.w - 14.0 * s - cw, baseline, color(mix(fg, panel, 0.35), 1.0), 400.0);
    }
}

/// Lines between split panes.
pub fn dividers(app: &mut App) {
    let (bw, bh) = app.buffer_size();
    let top = app.header_height();
    let area = Rect::new(0.0, top, bw as f32, (bh as f32 - top).max(1.0));
    let gap = app.gap();
    let mut divs = Vec::new();
    if let Some(t) = app.tab() {
        t.root.dividers(area, gap, &mut Vec::new(), &mut divs);
    }
    let c = color(mix(app.cfg.colors.background.tuple(), (0, 0, 0), 0.45), 1.0);
    let (scene, _, _, damage) = app.scene_mut();
    for (i, d) in divs.iter().enumerate() {
        let ri = IRect::of(d.rect);
        damage.region(key_of(("divider", i)), ri, key_of(ri));
        scene.layers[0].rects.push(RectInst::fill(d.rect.x, d.rect.y, d.rect.w, d.rect.h, c));
    }
}
