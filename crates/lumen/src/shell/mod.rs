//! The desktop shell that lives inside the compositor: top panel, window
//! overview, Alt+Tab switcher, notifications and the frame-time HUD.
//!
//! State and behavior are here; drawing is in [`render`].

pub mod render;

use std::collections::HashMap;
use std::time::Duration;

use smithay::input::keyboard::{keysyms, Keysym, ModifiersState};
use smithay::utils::{Logical, Point, Rectangle};

use crate::anim::{Spring, SpringParams};
use crate::input::BTN_LEFT;
use crate::state::Lumen;
use crate::theme::Theme;
use crate::wm::{Wm, WORKSPACES};

pub struct Switcher {
    /// Window ids in most-recently-used order.
    pub ids: Vec<u32>,
    pub index: usize,
    /// Fades the popup in (0 → 1).
    pub appear: Spring,
}

// Drawn by the shell UI together with text rendering (milestone 5); until
// then notifications are logged.
#[allow(dead_code)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub expires: Duration,
    /// 0 = hidden, 1 = fully shown; slides and fades.
    pub anim: Spring,
}

pub struct Shell {
    /// Super pressed with no other key since: releasing it toggles the overview.
    pub super_tap: bool,
    pub hud: bool,
    /// Animation clock (the predicted presentation time of the last frame).
    pub last_anim_time: Duration,
    pub overview: Spring,
    pub overview_open: bool,
    /// Window under the pointer (or keyboard selection) in the overview.
    pub hovered: Option<u32>,
    pub switcher: Option<Switcher>,
    pub notifications: Vec<Notification>,
    next_notification: u64,
    popover_params: SpringParams,
    pub shapes: render::ShellShapes,
}

/// Where windows go in the overview: the output minus the panel and margins.
pub fn overview_area(theme: &Theme, out: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
    let g = theme.geometry.grid;
    let top = theme.geometry.panel_height + g * 6.0;
    let side = g * 6.0;
    let bottom = g * 6.0;
    Rectangle::new(
        (out.loc.x + side, out.loc.y + top).into(),
        ((out.size.w - side * 2.0).max(1.0), (out.size.h - top - bottom).max(1.0)).into(),
    )
}

/// Arrange windows in rows inside `area` without upscaling them, choosing the
/// row count that shows the windows as large as possible.
pub fn overview_slots(wins: &[(u32, Rectangle<f64, Logical>)], area: Rectangle<f64, Logical>, gap: f64) -> HashMap<u32, Rectangle<f64, Logical>> {
    let mut out = HashMap::new();
    let n = wins.len();
    if n == 0 {
        return out;
    }
    // Reading order of window centers keeps the overview spatially familiar.
    let mut sorted: Vec<&(u32, Rectangle<f64, Logical>)> = wins.iter().collect();
    sorted.sort_by(|a, b| {
        let ca = a.1.loc.y + a.1.size.h / 2.0;
        let cb = b.1.loc.y + b.1.size.h / 2.0;
        ca.total_cmp(&cb).then(a.0.cmp(&b.0))
    });

    struct Row {
        items: Vec<(u32, Rectangle<f64, Logical>, f64)>,
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
            let mut items: Vec<(u32, Rectangle<f64, Logical>, f64)> = chunk
                .iter()
                .map(|(id, r)| (*id, *r, (row_h / r.size.h.max(1.0)).min(1.0)))
                .collect();
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
            out.insert(id, Rectangle::new((x, y + (row.height - h) / 2.0).into(), (w, h).into()));
            x += w + gap;
        }
        y += row.height + gap;
    }
    out
}

/// The workspace indicator in the panel (also the "Activities" button).
pub fn workspace_indicator_rect(theme: &Theme, out: Rectangle<f64, Logical>) -> Rectangle<f64, Logical> {
    let g = theme.geometry.grid;
    let h = theme.geometry.panel_height;
    let w = g * 2.0 + (WORKSPACES as f64 - 1.0) * (g + g * 0.5) + g * 3.0;
    Rectangle::new((out.loc.x + g, out.loc.y).into(), (w + g * 2.0, h).into())
}

impl Shell {
    pub fn new(theme: &Theme) -> Shell {
        let a = &theme.animation;
        Shell {
            super_tap: false,
            hud: std::env::var("LUMEN_HUD").is_ok(),
            last_anim_time: Duration::ZERO,
            overview: Spring::new(0.0, theme.spring(a.overview, 0.0005)),
            overview_open: false,
            hovered: None,
            switcher: None,
            notifications: Vec::new(),
            next_notification: 1,
            popover_params: theme.spring(a.popover, 0.002),
            shapes: render::ShellShapes::default(),
        }
    }

    pub fn wants_keyboard(&self) -> bool {
        self.overview_open || self.switcher.is_some()
    }

    pub fn switcher_active(&self) -> bool {
        self.switcher.is_some()
    }

    pub fn overview_progress(&self) -> f64 {
        self.overview.value.clamp(0.0, 1.0)
    }

    /// Overview slot for every window on the current workspace, while the
    /// overview is visible at all.
    pub fn overview_layout(&self, wm: &Wm, theme: &Theme, out: Rectangle<f64, Logical>) -> Option<HashMap<u32, Rectangle<f64, Logical>>> {
        if !self.overview_open && self.overview.value <= 0.0 && self.overview.is_settled() {
            return None;
        }
        let wins: Vec<(u32, Rectangle<f64, Logical>)> = wm
            .windows
            .iter()
            .filter(|w| w.mapped && w.ws == wm.current)
            .map(|w| (w.id, w.target_geo()))
            .collect();
        Some(overview_slots(&wins, overview_area(theme, out), theme.geometry.grid * 4.0))
    }

    pub fn notify(&mut self, theme: &Theme, title: &str, body: &str, now: Duration) -> u64 {
        tracing::info!(title, body, "notification");
        let id = self.next_notification;
        self.next_notification += 1;
        let mut anim = Spring::new(0.0, self.popover_params);
        anim.set_target(1.0);
        let timeout = Duration::from_millis(theme.animation.notification_timeout_ms);
        self.notifications.push(Notification { id, title: title.into(), body: body.into(), expires: now + timeout, anim });
        // At most three on screen; the oldest leaves first.
        while self.notifications.iter().filter(|n| n.anim.target > 0.0).count() > 3 {
            if let Some(n) = self.notifications.iter_mut().find(|n| n.anim.target > 0.0) {
                n.anim.set_target(0.0);
            }
        }
        id
    }

    pub fn on_windows_changed(&mut self) {}

    /// Pointer moved; returns true if the shell owns the pointer here (clients
    /// must not get pointer focus).
    pub fn pointer_motion(&mut self, wm: &Wm, theme: &Theme, pos: Point<f64, Logical>, out: Rectangle<f64, Logical>) -> bool {
        if self.overview_open {
            let slots = self.overview_layout(wm, theme, out).unwrap_or_default();
            self.hovered = slots.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id);
            return true;
        }
        self.switcher.is_some() || pos.y < out.loc.y + theme.geometry.panel_height
    }

    pub fn advance(&mut self, dt: Duration, now: Duration, _theme: &Theme) {
        self.overview.step(dt);
        if let Some(s) = &mut self.switcher {
            s.appear.step(dt);
        }
        for n in &mut self.notifications {
            if now >= n.expires && n.anim.target > 0.0 {
                n.anim.set_target(0.0);
            }
            n.anim.step(dt);
        }
        self.notifications.retain(|n| n.anim.target > 0.0 || n.anim.value > 0.0 && !n.anim.is_settled());
    }

    pub fn animating(&self, now: Duration) -> bool {
        let _ = now;
        !self.overview.is_settled()
            || self.switcher.as_ref().is_some_and(|s| !s.appear.is_settled())
            || self.notifications.iter().any(|n| !n.anim.is_settled())
            || self.hud
    }
}

impl Lumen {
    pub fn shell_toggle_overview(&mut self, apps: bool) {
        let _ = apps;
        let open = !self.shell.overview_open;
        self.shell_set_overview(open);
    }

    pub fn shell_set_overview(&mut self, open: bool) {
        if self.shell.overview_open == open {
            return;
        }
        self.shell.switcher = None;
        self.shell.overview_open = open;
        self.shell.overview.set_target(if open { 1.0 } else { 0.0 });
        self.shell.hovered = if open { self.wm.focused } else { None };
        // While the overview is open the shell owns the keyboard.
        let focused = self.wm.focused;
        self.focus_window(focused);
        if !open {
            // Re-evaluate pointer focus now that windows are back in place.
            let pos = self.pointer_pos;
            self.on_pointer_moved(pos, self.now().as_millis() as u32, None);
        }
        self.queue_redraw_all();
    }

    pub fn shell_switcher_next(&mut self, reverse: bool) {
        if self.shell.overview_open {
            self.shell_set_overview(false);
        }
        match &mut self.shell.switcher {
            Some(s) => {
                let n = s.ids.len();
                s.index = if reverse { (s.index + n - 1) % n } else { (s.index + 1) % n };
            }
            None => {
                let current = self.wm.current;
                let ids: Vec<u32> = self
                    .wm
                    .mru
                    .iter()
                    .copied()
                    .filter(|id| self.wm.get(*id).is_some_and(|w| w.mapped && w.ws == current))
                    .chain(self.wm.windows.iter().rev().filter(|w| w.mapped && w.ws == current).map(|w| w.id))
                    .fold(Vec::new(), |mut v, id| {
                        if !v.contains(&id) {
                            v.push(id);
                        }
                        v
                    });
                if ids.is_empty() {
                    return;
                }
                let n = ids.len();
                let index = if n == 1 { 0 } else if reverse { n - 1 } else { 1 };
                let mut appear = Spring::new(0.0, self.shell.popover_params);
                appear.set_target(1.0);
                self.shell.switcher = Some(Switcher { ids, index, appear });
                let focused = self.wm.focused;
                self.focus_window(focused);
            }
        }
        self.queue_redraw_all();
    }

    pub fn shell_switcher_commit(&mut self) {
        let Some(s) = self.shell.switcher.take() else { return };
        let target = s.ids.get(s.index).copied().filter(|id| self.wm.get(*id).is_some());
        let target = target.or(self.wm.focused);
        self.focus_window(target);
        self.queue_redraw_all();
    }

    pub fn shell_switcher_cancel(&mut self) {
        if self.shell.switcher.take().is_some() {
            let focused = self.wm.focused;
            self.focus_window(focused);
            self.queue_redraw_all();
        }
    }

    /// A key while the shell owns the keyboard.
    pub fn shell_key(&mut self, sym: Keysym, utf8: Option<String>, mods: ModifiersState) {
        let _ = (utf8, mods);
        let raw = sym.raw();
        if let Some(s) = &mut self.shell.switcher {
            let n = s.ids.len();
            match raw {
                keysyms::KEY_Escape => self.shell_switcher_cancel(),
                keysyms::KEY_Return | keysyms::KEY_KP_Enter => self.shell_switcher_commit(),
                keysyms::KEY_Left => s.index = (s.index + n - 1) % n,
                keysyms::KEY_Right => s.index = (s.index + 1) % n,
                _ => {}
            }
            return;
        }
        if self.shell.overview_open {
            let out = self.output_rect();
            let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
            match raw {
                keysyms::KEY_Escape => self.shell_set_overview(false),
                keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                    let pick = self.shell.hovered.filter(|id| slots.contains_key(id));
                    if let Some(id) = pick {
                        self.focus_window(Some(id));
                    }
                    self.shell_set_overview(false);
                }
                keysyms::KEY_Left | keysyms::KEY_Right | keysyms::KEY_Up | keysyms::KEY_Down | keysyms::KEY_Tab => {
                    self.shell.hovered = navigate(&slots, self.shell.hovered, raw);
                }
                _ => {}
            }
        }
    }

    /// Pointer button for the shell; returns true if it was consumed.
    pub fn shell_click(&mut self, button: u32, pos: Point<f64, Logical>) -> bool {
        let out = self.output_rect();
        if self.shell.switcher.is_some() {
            return true;
        }
        if workspace_indicator_rect(&self.theme, out).contains(pos) {
            if button == BTN_LEFT {
                let open = !self.shell.overview_open;
                self.shell_set_overview(open);
            }
            return true;
        }
        if self.shell.overview_open {
            let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
            let hit = slots.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id);
            match (hit, button) {
                (Some(id), BTN_LEFT) => {
                    self.focus_window(Some(id));
                    self.shell_set_overview(false);
                }
                (Some(id), crate::input::BTN_MIDDLE) => self.close_window(id),
                (None, BTN_LEFT) => self.shell_set_overview(false),
                _ => {}
            }
            return true;
        }
        pos.y < out.loc.y + self.theme.geometry.panel_height
    }

    /// Scroll for the shell; returns true if consumed. In the overview the
    /// wheel switches workspaces.
    pub fn shell_scroll(&mut self, v: f64) -> bool {
        if !self.shell.overview_open {
            return false;
        }
        if v.abs() >= 1.0 {
            let d = if v > 0.0 { 1 } else { -1 };
            let n = (self.wm.current as i32 + d).clamp(0, WORKSPACES as i32 - 1) as usize;
            self.switch_workspace(n);
            self.shell.hovered = None;
        }
        true
    }
}

/// Keyboard navigation between overview slots: nearest slot in the direction.
fn navigate(slots: &HashMap<u32, Rectangle<f64, Logical>>, from: Option<u32>, key: u32) -> Option<u32> {
    let center = |r: &Rectangle<f64, Logical>| (r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0);
    let Some(cur) = from.and_then(|id| slots.get(&id)) else {
        // Nothing selected yet: start at the top-left slot.
        return slots.iter().min_by(|a, b| (a.1.loc.y, a.1.loc.x).partial_cmp(&(b.1.loc.y, b.1.loc.x)).unwrap()).map(|(id, _)| *id);
    };
    let (cx, cy) = center(cur);
    if key == keysyms::KEY_Tab {
        // Reading order, wrapping around.
        let mut order: Vec<(u32, (f64, f64))> = slots.iter().map(|(id, r)| (*id, (r.loc.y, r.loc.x))).collect();
        order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let i = order.iter().position(|(id, _)| Some(*id) == from).unwrap_or(0);
        return Some(order[(i + 1) % order.len()].0);
    }
    let (dx, dy): (f64, f64) = match key {
        keysyms::KEY_Left => (-1.0, 0.0),
        keysyms::KEY_Right => (1.0, 0.0),
        keysyms::KEY_Up => (0.0, -1.0),
        _ => (0.0, 1.0),
    };
    slots
        .iter()
        .filter(|(id, _)| Some(**id) != from)
        .filter_map(|(id, r)| {
            let (x, y) = center(r);
            let along = (x - cx) * dx + (y - cy) * dy;
            if along <= 1.0 {
                return None;
            }
            let across = ((x - cx) * dy).abs() + ((y - cy) * dx).abs();
            Some((*id, along + across * 2.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
        .or(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn single_window_is_not_upscaled() {
        let slots = overview_slots(&[(1, rect(100.0, 100.0, 400.0, 300.0))], rect(0.0, 0.0, 1600.0, 1000.0), 32.0);
        let r = slots[&1];
        assert_eq!((r.size.w, r.size.h), (400.0, 300.0));
        // Centered.
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
            // Aspect ratio is preserved.
            assert!((ra.size.w / ra.size.h - 1.5).abs() < 1e-6);
            for (b, rb) in &slots {
                if a != b {
                    assert!(ra.intersection(*rb).is_none_or(|i| i.size.w < 1e-6 || i.size.h < 1e-6), "{a} overlaps {b}");
                }
            }
        }
    }

    #[test]
    fn keyboard_navigation_moves_in_direction() {
        let mut slots = HashMap::new();
        slots.insert(1, rect(0.0, 0.0, 100.0, 100.0));
        slots.insert(2, rect(200.0, 0.0, 100.0, 100.0));
        slots.insert(3, rect(0.0, 200.0, 100.0, 100.0));
        assert_eq!(navigate(&slots, Some(1), keysyms::KEY_Right), Some(2));
        assert_eq!(navigate(&slots, Some(1), keysyms::KEY_Down), Some(3));
        assert_eq!(navigate(&slots, Some(2), keysyms::KEY_Left), Some(1));
        assert_eq!(navigate(&slots, Some(1), keysyms::KEY_Left), Some(1));
        assert_eq!(navigate(&slots, None, keysyms::KEY_Right), Some(1));
    }
}
