//! Shell geometry shared by drawing and input (logical, global coordinates).

use std::collections::HashMap;

use smithay::utils::{Logical, Rectangle};

use crate::theme::Theme;
use crate::wm::WORKSPACES;

pub type Rect = Rectangle<f64, Logical>;

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// The overview's search entry.
pub fn search_rect(theme: &Theme, out: Rect) -> Rect {
    let g = theme.geometry.grid;
    let w = (g * 45.0).min(out.size.w - g * 8.0);
    rect(out.loc.x + (out.size.w - w) / 2.0, out.loc.y + theme.geometry.panel_height + g * 2.0, w, g * 5.0)
}

/// Workspace thumbnails strip, one rect per workspace.
pub fn workspace_thumbs(theme: &Theme, out: Rect) -> Vec<Rect> {
    let g = theme.geometry.grid;
    let s = search_rect(theme, out);
    let h = (out.size.h * 0.085).round().clamp(40.0, 110.0);
    let w = (h * out.size.w / out.size.h.max(1.0)).round();
    let total = w * WORKSPACES as f64 + g * 2.0 * (WORKSPACES as f64 - 1.0);
    let x0 = out.loc.x + (out.size.w - total) / 2.0;
    let y = s.loc.y + s.size.h + g * 2.0;
    (0..WORKSPACES).map(|i| rect(x0 + i as f64 * (w + g * 2.0), y, w, h)).collect()
}

/// Height of the dock (icon plus padding).
pub fn dock_height(theme: &Theme) -> f64 {
    theme.geometry.dock_icon_size + theme.geometry.grid * 2.0
}

/// The dock with `items` icons (plus the app-grid button), fully shown.
pub fn dock_rect(theme: &Theme, out: Rect, items: usize) -> Rect {
    let g = theme.geometry.grid;
    let icon = theme.geometry.dock_icon_size;
    let n = items + 1;
    let w = n as f64 * icon + (n as f64 - 1.0) * g + g * 2.0 + if items > 0 { g * 2.0 } else { 0.0 };
    let h = dock_height(theme);
    rect(out.loc.x + (out.size.w - w) / 2.0, out.loc.y + out.size.h - h - g * 1.5, w, h)
}

/// Icon cell `i` of the dock (the app-grid button is the last cell, after a gap).
pub fn dock_cell(theme: &Theme, dock: Rect, items: usize, i: usize) -> Rect {
    let g = theme.geometry.grid;
    let icon = theme.geometry.dock_icon_size;
    let extra = if i >= items && items > 0 { g * 2.0 } else { 0.0 };
    rect(dock.loc.x + g + i as f64 * (icon + g) + extra, dock.loc.y + g, icon, icon)
}

/// Where windows go in the overview: below the search entry and workspace
/// strip, above the dock.
pub fn overview_area(theme: &Theme, out: Rect) -> Rect {
    let g = theme.geometry.grid;
    let thumbs = workspace_thumbs(theme, out);
    let top = thumbs[0].loc.y + thumbs[0].size.h + g * 4.0;
    let bottom = out.loc.y + out.size.h - dock_height(theme) - g * 6.0;
    let side = g * 6.0;
    rect(out.loc.x + side, top, (out.size.w - side * 2.0).max(1.0), (bottom - top).max(1.0))
}

/// Grid of app tiles (search results and the app grid) inside `area`.
pub fn app_tiles(theme: &Theme, area: Rect, count: usize) -> Vec<Rect> {
    let g = theme.geometry.grid;
    let (tw, th) = (g * 14.0, g * 14.0);
    let cols = (((area.size.w + g * 2.0) / (tw + g * 2.0)).floor() as usize).clamp(1, 8);
    let rows = count.div_ceil(cols).max(1);
    let used_w = cols.min(count.max(1)) as f64 * (tw + g * 2.0) - g * 2.0;
    let used_h = rows as f64 * (th + g * 2.0) - g * 2.0;
    let x0 = area.loc.x + (area.size.w - used_w) / 2.0;
    let y0 = area.loc.y + ((area.size.h - used_h) / 2.0).min(g * 4.0).max(0.0);
    (0..count).map(|i| rect(x0 + (i % cols) as f64 * (tw + g * 2.0), y0 + (i / cols) as f64 * (th + g * 2.0), tw, th)).collect()
}

/// The workspace indicator in the panel (also the "Activities" button).
pub fn workspace_indicator_rect(theme: &Theme, out: Rect) -> Rect {
    let g = theme.geometry.grid;
    let h = theme.geometry.panel_height;
    let w = g * 2.0 + (WORKSPACES as f64 - 1.0) * (g + g * 0.5) + g * 3.0;
    rect(out.loc.x + g, out.loc.y, w + g * 2.0, h)
}

/// The power button at the right end of the panel.
pub fn power_rect(theme: &Theme, out: Rect) -> Rect {
    let h = theme.geometry.panel_height;
    let g = theme.geometry.grid;
    rect(out.loc.x + out.size.w - h - g, out.loc.y, h, h)
}

/// The power menu popover and its item rows.
pub fn power_menu(theme: &Theme, out: Rect, items: usize) -> (Rect, Vec<Rect>) {
    let g = theme.geometry.grid;
    let row = g * 5.0;
    let w = g * 26.0;
    let p = power_rect(theme, out);
    let menu = rect(p.loc.x + p.size.w - w, p.loc.y + p.size.h + g, w, row * items as f64 + g * 2.0);
    let rows = (0..items).map(|i| rect(menu.loc.x + g, menu.loc.y + g + i as f64 * row, w - g * 2.0, row)).collect();
    (menu, rows)
}

/// Arrange windows in rows inside `area` without upscaling them, choosing the
/// row count that shows the windows as large as possible.
pub fn overview_slots(wins: &[(u32, Rect)], area: Rect, gap: f64) -> HashMap<u32, Rect> {
    let mut out = HashMap::new();
    let n = wins.len();
    if n == 0 {
        return out;
    }
    // Reading order of window centers keeps the overview spatially familiar.
    let mut sorted: Vec<&(u32, Rect)> = wins.iter().collect();
    sorted.sort_by(|a, b| {
        let ca = a.1.loc.y + a.1.size.h / 2.0;
        let cb = b.1.loc.y + b.1.size.h / 2.0;
        ca.total_cmp(&cb).then(a.0.cmp(&b.0))
    });

    struct Row {
        items: Vec<(u32, Rect, f64)>,
        width: f64,
        height: f64,
    }
    let mut best: Option<(f64, Vec<Row>)> = None;
    for rows in 1..=n {
        let per = n.div_ceil(rows);
        let row_h = (area.size.h - gap * (rows - 1) as f64) / rows as f64;
        if row_h <= 0.0 {
            break;
        }
        let mut layout = Vec::new();
        let mut covered = 0.0;
        for chunk in sorted.chunks(per) {
            let mut items: Vec<(u32, Rect, f64)> =
                chunk.iter().map(|(id, r)| (*id, *r, (row_h / r.size.h.max(1.0)).min(1.0))).collect();
            items.sort_by(|a, b| (a.1.loc.x + a.1.size.w / 2.0).total_cmp(&(b.1.loc.x + b.1.size.w / 2.0)));
            let gaps = gap * (items.len() - 1) as f64;
            let w: f64 = items.iter().map(|(_, r, s)| r.size.w * s).sum();
            if w + gaps > area.size.w {
                let f = (area.size.w - gaps).max(1.0) / w.max(1.0);
                for it in &mut items {
                    it.2 *= f;
                }
            }
            let width = items.iter().map(|(_, r, s)| r.size.w * s).sum::<f64>() + gaps;
            let height = items.iter().map(|(_, r, s)| r.size.h * s).fold(0.0, f64::max);
            covered += items.iter().map(|(_, r, s)| r.size.w * r.size.h * s * s).sum::<f64>();
            layout.push(Row { items, width, height });
        }
        if best.as_ref().is_none_or(|(c, _)| covered > *c * 1.0001) {
            best = Some((covered, layout));
        }
    }
    let Some((_, rows)) = best else { return out };
    let total_h = rows.iter().map(|r| r.height).sum::<f64>() + gap * (rows.len() - 1) as f64;
    let mut y = area.loc.y + (area.size.h - total_h) / 2.0;
    for row in rows {
        let mut x = area.loc.x + (area.size.w - row.width) / 2.0;
        for (id, r, s) in row.items {
            let (w, h) = (r.size.w * s, r.size.h * s);
            out.insert(id, rect(x, y + (row.height - h) / 2.0, w, h));
            x += w + gap;
        }
        y += row.height + gap;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_window_is_not_upscaled() {
        let slots = overview_slots(&[(1, rect(100.0, 100.0, 400.0, 300.0))], rect(0.0, 0.0, 1600.0, 1000.0), 32.0);
        let r = slots[&1];
        assert_eq!((r.size.w, r.size.h), (400.0, 300.0));
        assert!((r.loc.x - 600.0).abs() < 1e-9 && (r.loc.y - 350.0).abs() < 1e-9);
    }

    #[test]
    fn slots_fit_and_do_not_overlap() {
        let wins: Vec<_> = (0..7).map(|i| (i, rect(i as f64 * 50.0, i as f64 * 30.0, 1200.0, 800.0))).collect();
        let area = rect(48.0, 80.0, 1184.0, 672.0);
        let slots = overview_slots(&wins, area, 32.0);
        assert_eq!(slots.len(), 7);
        for (a, ra) in &slots {
            assert!(ra.loc.x >= area.loc.x - 1e-6 && ra.loc.x + ra.size.w <= area.loc.x + area.size.w + 1e-6);
            assert!(ra.loc.y >= area.loc.y - 1e-6 && ra.loc.y + ra.size.h <= area.loc.y + area.size.h + 1e-6);
            assert!((ra.size.w / ra.size.h - 1.5).abs() < 1e-6);
            for (b, rb) in &slots {
                if a != b {
                    assert!(ra.intersection(*rb).is_none_or(|i| i.size.w < 1e-6 || i.size.h < 1e-6), "{a} overlaps {b}");
                }
            }
        }
    }

    #[test]
    fn overview_regions_do_not_overlap() {
        let theme = Theme::default();
        let out = rect(0.0, 0.0, 1280.0, 800.0);
        let s = search_rect(&theme, out);
        let thumbs = workspace_thumbs(&theme, out);
        let area = overview_area(&theme, out);
        let dock = dock_rect(&theme, out, 3);
        assert!(s.loc.y >= theme.geometry.panel_height);
        assert!(thumbs[0].loc.y >= s.loc.y + s.size.h);
        assert!(area.loc.y >= thumbs[0].loc.y + thumbs[0].size.h);
        assert!(area.loc.y + area.size.h <= dock.loc.y);
        assert!(dock.loc.y + dock.size.h <= out.size.h);
        // Dock cells are inside the dock and ordered.
        let cells: Vec<Rect> = (0..4).map(|i| dock_cell(&theme, dock, 3, i)).collect();
        for c in &cells {
            assert!(dock.contains_rect(*c), "{c:?} outside {dock:?}");
        }
        assert!(cells.windows(2).all(|w| w[1].loc.x > w[0].loc.x + w[0].size.w));
        let tiles = app_tiles(&theme, area, 10);
        assert!(tiles.iter().all(|t| area.contains_rect(*t) || t.loc.y + t.size.h > area.loc.y + area.size.h));
    }
}
