//! The desktop shell that lives inside the compositor: top panel (workspace
//! pills, clock, power menu), overview (window grid, search, app grid,
//! workspace thumbnails), dock, Alt+Tab switcher, notifications and the
//! frame-time HUD.
//!
//! State and behavior are here; drawing is in [`render`], geometry in
//! [`layout`]. Interactive areas are recorded while drawing (`hits`) and used
//! by input handling, so the two can never disagree.

pub mod layout;
pub mod render;

use std::collections::HashMap;
use std::time::Duration;

use smithay::input::keyboard::{keysyms, Keysym, ModifiersState};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Logical, Point};

pub use self::layout::*;
use crate::anim::{Spring, SpringParams};
use crate::apps::{self, App};
use crate::input::{BTN_LEFT, BTN_MIDDLE};
use crate::state::Lumen;
use crate::theme::Theme;
use crate::wm::{Wm, WORKSPACES};

/// Something the pointer can hit in the shell UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    Activities,
    Power,
    PowerItem(usize),
    Popover,
    DockItem(usize),
    DockApps,
    Dock,
    Search,
    App(usize),
    Workspace(usize),
    Notification(u64),
    NotificationClose(u64),
    Switcher,
}

pub const POWER_ITEMS: [&str; 3] = ["Log Out", "Restart", "Power Off"];

pub struct Switcher {
    /// Window ids in most-recently-used order.
    pub ids: Vec<u32>,
    pub index: usize,
    /// Fades the popup in (0 → 1).
    pub appear: Spring,
}

pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub expires: Duration,
    /// 0 = hidden, 1 = fully shown; slides and fades.
    pub anim: Spring,
}

pub struct Popover {
    pub anim: Spring,
    pub open: bool,
}

/// A dock entry: a favorite app and/or a running app with its windows.
#[derive(Debug, Clone)]
pub struct DockItem {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub exec: Option<Vec<String>>,
    pub windows: Vec<u32>,
}

pub struct Shell {
    /// Super pressed with no other key since: releasing it toggles the overview.
    pub super_tap: bool,
    pub hud: bool,
    /// Animation clock (the predicted presentation time of the last frame).
    pub last_anim_time: Duration,
    pub overview: Spring,
    pub overview_open: bool,
    /// Overview shows the app grid instead of windows (Super+A, dock button).
    pub apps_mode: bool,
    /// 0 = windows, 1 = app grid (crossfade).
    pub apps_anim: Spring,
    /// Window under the pointer (or keyboard selection) in the overview.
    pub hovered: Option<u32>,
    /// Shell element under the pointer.
    pub hover: Option<Hit>,
    pub query: String,
    /// Indices into `apps` matching `query`, best first.
    pub results: Vec<usize>,
    /// Keyboard selection among results / app grid.
    pub selected: usize,
    pub apps: Vec<App>,
    pub favorites: Vec<String>,
    /// 0 = hidden below the screen edge, 1 = shown.
    pub dock: Spring,
    pub popover: Option<Popover>,
    pub switcher: Option<Switcher>,
    pub notifications: Vec<Notification>,
    next_notification: u64,
    popover_params: SpringParams,
    /// Interactive regions of the last frame, topmost first.
    pub hits: Vec<(Rect, Hit)>,
    pub pointer: Point<f64, Logical>,
    pub shapes: render::ShellShapes,
}

impl Shell {
    pub fn new(theme: &Theme) -> Shell {
        let a = &theme.animation;
        let favorites = std::env::var("LUMEN_FAVORITES")
            .map(|v| v.split(',').map(String::from).filter(|s| !s.is_empty()).collect())
            .unwrap_or_else(|_| vec!["lumen-term".to_string()]);
        let mut dock = Spring::new(1.0, theme.spring(a.popover, 0.001));
        dock.snap(1.0);
        Shell {
            super_tap: false,
            hud: std::env::var("LUMEN_HUD").is_ok(),
            last_anim_time: Duration::ZERO,
            overview: Spring::new(0.0, theme.spring(a.overview, 0.0005)),
            overview_open: false,
            apps_mode: false,
            apps_anim: Spring::new(0.0, theme.spring(a.overview, 0.001)),
            hovered: None,
            hover: None,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            apps: apps::load(),
            favorites,
            dock,
            popover: None,
            switcher: None,
            notifications: Vec::new(),
            next_notification: 1,
            popover_params: theme.spring(a.popover, 0.002),
            hits: Vec::new(),
            pointer: (0.0, 0.0).into(),
            shapes: render::ShellShapes::default(),
        }
    }

    pub fn wants_keyboard(&self) -> bool {
        self.overview_open || self.switcher.is_some() || self.popover.as_ref().is_some_and(|p| p.open)
    }

    pub fn switcher_active(&self) -> bool {
        self.switcher.is_some()
    }

    pub fn overview_progress(&self) -> f64 {
        self.overview.value.clamp(0.0, 1.0)
    }

    pub fn hit_at(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        self.hits.iter().find(|(r, _)| r.contains(pos)).map(|(_, h)| *h)
    }

    /// Overview slot for every window on the current workspace (that matches
    /// the search), while the overview is visible at all. With search results
    /// the app tiles take a row at the top of the area.
    pub fn overview_layout(&self, wm: &Wm, theme: &Theme, out: Rect) -> Option<HashMap<u32, Rect>> {
        self.overview_layout_ws(wm, theme, out, wm.current)
    }

    /// Overview slots of the windows on workspace `ws` (each workspace is laid
    /// out on its own; others show while the overview slides between them).
    pub fn overview_layout_ws(&self, wm: &Wm, theme: &Theme, out: Rect, ws: usize) -> Option<HashMap<u32, Rect>> {
        if !self.overview_open && self.overview.value <= 0.0 && self.overview.is_settled() {
            return None;
        }
        let wins: Vec<(u32, Rect)> = wm
            .windows
            .iter()
            .filter(|w| w.mapped && w.ws == ws && self.window_matches(wm, w.id))
            .map(|w| (w.id, w.target_geo()))
            .collect();
        Some(overview_slots(&wins, self.windows_area(theme, out), theme.geometry.grid * 4.0))
    }

    /// Area for window slots: the overview area below the search results row.
    pub fn windows_area(&self, theme: &Theme, out: Rect) -> Rect {
        let area = overview_area(theme, out);
        if self.query.trim().is_empty() || self.results.is_empty() {
            return area;
        }
        let row = self.results_area(theme, out);
        let top = row.loc.y + row.size.h + theme.geometry.grid * 3.0;
        rect(area.loc.x, top, area.size.w, (area.loc.y + area.size.h - top).max(1.0))
    }

    /// Where app tiles go: one row at the top while searching, the whole
    /// overview area for the app grid.
    pub fn results_area(&self, theme: &Theme, out: Rect) -> Rect {
        let area = overview_area(theme, out);
        if self.query.trim().is_empty() {
            return area;
        }
        rect(area.loc.x, area.loc.y, area.size.w, theme.geometry.grid * 14.0)
    }

    /// How visible a window is in the overview (search and app grid hide
    /// windows); 1 outside the overview.
    pub fn window_visibility(&self, wm: &Wm, id: u32) -> f64 {
        let ov = self.overview_progress();
        if ov <= 0.0 {
            return 1.0;
        }
        let mut v = 1.0 - self.apps_anim.value.clamp(0.0, 1.0) * ov;
        if !self.window_matches(wm, id) {
            v *= 1.0 - ov;
        }
        v.clamp(0.0, 1.0)
    }

    /// Windows that match the search query (all of them when it is empty).
    pub fn window_matches(&self, wm: &Wm, id: u32) -> bool {
        let q = self.query.trim().to_lowercase();
        q.is_empty()
            || wm.get(id).is_some_and(|w| w.title().to_lowercase().contains(&q) || w.app_id().to_lowercase().contains(&q))
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

    pub fn dismiss_notification(&mut self, id: u64) {
        if let Some(n) = self.notifications.iter_mut().find(|n| n.id == id) {
            n.anim.set_target(0.0);
        }
    }

    pub fn on_windows_changed(&mut self) {}

    /// Dock entries: favorites first, then running apps (grouped by app id).
    pub fn dock_items(&self, wm: &Wm) -> Vec<DockItem> {
        let mut items: Vec<DockItem> = self
            .favorites
            .iter()
            .map(|fav| match self.apps.iter().find(|a| &a.id == fav) {
                Some(a) => DockItem { id: a.id.clone(), name: a.name.clone(), icon: icon_name(a), exec: Some(a.exec.clone()), windows: vec![] },
                None => DockItem { id: fav.clone(), name: fav.clone(), icon: fav.clone(), exec: Some(vec![fav.clone()]), windows: vec![] },
            })
            .collect();
        // Windows in stacking order, so each item's last window is its topmost.
        for w in wm.windows.iter().filter(|w| w.mapped) {
            let app_id = w.app_id();
            let key = if app_id.is_empty() { format!("window-{}", w.id) } else { app_id.clone() };
            if let Some(item) = items.iter_mut().find(|i| app_matches(&i.id, &i.exec, &key)) {
                item.windows.push(w.id);
                continue;
            }
            let app = self.apps.iter().find(|a| app_matches(&a.id, &Some(a.exec.clone()), &key));
            items.push(DockItem {
                id: key.clone(),
                name: app.map(|a| a.name.clone()).unwrap_or_else(|| if w.title().is_empty() { key.clone() } else { w.title() }),
                icon: app.map(icon_name).unwrap_or_else(|| key.clone()),
                exec: app.map(|a| a.exec.clone()),
                windows: vec![w.id],
            });
        }
        items
    }

    /// Keep the dock out of the way: it hides when a window on the current
    /// workspace overlaps it, and shows in the overview, on an empty
    /// workspace, or when the pointer touches the bottom edge.
    fn update_dock(&mut self, wm: &Wm, theme: &Theme, out: Rect) {
        let n = self.dock_items(wm).len();
        let r = dock_rect(theme, out, n);
        let at_edge = self.pointer.y >= out.loc.y + out.size.h - 2.0;
        let over = self.dock.value > 0.5 && r.contains(self.pointer);
        let covered = wm.windows.iter().any(|w| w.mapped && w.ws == wm.current && w.target_geo().overlaps(r));
        let show = self.overview_open || at_edge || over || !covered || self.popover.is_some();
        self.dock.set_target(if show { 1.0 } else { 0.0 });
    }

    pub fn advance(&mut self, dt: Duration, now: Duration, wm: &Wm, theme: &Theme, out: Rect) {
        self.overview.step(dt);
        self.apps_anim.step(dt);
        self.update_dock(wm, theme, out);
        self.dock.step(dt);
        if let Some(p) = &mut self.popover {
            p.anim.step(dt);
        }
        if self.popover.as_ref().is_some_and(|p| !p.open && p.anim.value <= 0.01 && p.anim.is_settled()) {
            self.popover = None;
        }
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
            || !self.apps_anim.is_settled()
            || !self.dock.is_settled()
            || self.popover.as_ref().is_some_and(|p| !p.anim.is_settled())
            || self.switcher.as_ref().is_some_and(|s| !s.appear.is_settled())
            || self.notifications.iter().any(|n| !n.anim.is_settled())
            || self.hud
    }

    fn update_results(&mut self) {
        let q = self.query.trim();
        self.results = if q.is_empty() {
            Vec::new()
        } else {
            apps::search(&self.apps, q).into_iter().filter_map(|a| self.apps.iter().position(|x| x.id == a.id)).take(12).collect()
        };
        self.selected = 0;
    }

    /// Apps currently shown as tiles (search results, or the whole grid).
    pub fn visible_apps(&self) -> Vec<usize> {
        if !self.query.trim().is_empty() {
            self.results.clone()
        } else if self.apps_mode {
            (0..self.apps.len()).collect()
        } else {
            Vec::new()
        }
    }
}

/// Icon name for an app: its Icon= key if we draw it, else its id.
fn icon_name(a: &App) -> String {
    match a.icon.as_str() {
        "utilities-terminal" | "terminal" => "terminal".into(),
        _ if a.id == "lumen-term" => "terminal".into(),
        _ => a.id.clone(),
    }
}

/// Does a window with app id `key` belong to the desktop entry `id`?
fn app_matches(id: &str, exec: &Option<Vec<String>>, key: &str) -> bool {
    let k = key.to_lowercase();
    let id = id.to_lowercase();
    if k == id || k.ends_with(&format!(".{id}")) || id.ends_with(&format!(".{k}")) {
        return true;
    }
    exec.as_ref()
        .and_then(|e| e.first())
        .and_then(|p| p.rsplit('/').next())
        .is_some_and(|bin| bin.to_lowercase() == k)
}

/// Local time for the panel clock, e.g. "Thu 11 Oct  12:45".
pub fn clock_text() -> String {
    // SAFETY: localtime_r writes into the provided struct.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!(
            "{} {} {}  {:02}:{:02}",
            DAYS[tm.tm_wday.rem_euclid(7) as usize],
            tm.tm_mday,
            MONTHS[tm.tm_mon.rem_euclid(12) as usize],
            tm.tm_hour,
            tm.tm_min
        )
    }
}

fn until_next_minute() -> Duration {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    Duration::from_millis(60_000 - (now.as_millis() % 60_000) as u64 + 5)
}

impl Lumen {
    /// Redraw on every minute boundary so the panel clock stays exact.
    pub fn start_clock(&mut self) {
        let _ = self.lh.insert_source(Timer::from_duration(until_next_minute()), |_, _, state| {
            state.queue_redraw_all();
            TimeoutAction::ToDuration(until_next_minute())
        });
    }

    pub fn shell_toggle_overview(&mut self, apps: bool) {
        if self.shell.overview_open && self.shell.apps_mode != apps {
            self.shell_set_apps_mode(apps);
            return;
        }
        let open = !self.shell.overview_open;
        self.shell_set_apps_mode(apps && open);
        self.shell_set_overview(open);
    }

    fn shell_set_apps_mode(&mut self, apps: bool) {
        self.shell.apps_mode = apps;
        self.shell.apps_anim.set_target(if apps { 1.0 } else { 0.0 });
        self.shell.selected = 0;
        self.queue_redraw_all();
    }

    pub fn shell_set_overview(&mut self, open: bool) {
        if self.shell.overview_open == open {
            return;
        }
        self.shell.switcher = None;
        self.shell.popover = None;
        self.shell.overview_open = open;
        self.shell.overview.set_target(if open { 1.0 } else { 0.0 });
        self.shell.hovered = if open { self.wm.focused } else { None };
        self.shell.query.clear();
        self.shell.results.clear();
        if open {
            // Pick up apps installed since the last time.
            self.shell.apps = apps::load();
        } else if self.shell.apps_mode {
            self.shell_set_apps_mode(false);
        }
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

    pub fn shell_launch(&mut self, app: usize) {
        let Some(a) = self.shell.apps.get(app).cloned() else { return };
        tracing::info!(app = %a.id, "launching");
        self.spawn(&a.exec);
        self.shell_set_overview(false);
    }

    /// Dock click: launch an app without windows, focus its window, or cycle
    /// through its windows when one of them is already focused.
    pub fn shell_dock_activate(&mut self, index: usize) {
        let items = self.shell.dock_items(&self.wm);
        let Some(item) = items.get(index) else { return };
        if item.windows.is_empty() {
            if let Some(exec) = &item.exec {
                self.spawn(exec);
            }
            self.shell_set_overview(false);
            return;
        }
        let target = match self.wm.focused.and_then(|f| item.windows.iter().position(|w| *w == f)) {
            // Cycle: the bottom-most of the app's windows comes to the front.
            Some(_) if item.windows.len() > 1 => item.windows[0],
            Some(i) => item.windows[i],
            None => *item.windows.iter().max_by_key(|w| self.wm.mru.iter().rev().position(|m| m == *w)).unwrap_or(&item.windows[0]),
        };
        if let Some(ws) = self.wm.get(target).map(|w| w.ws) {
            self.switch_workspace(ws);
        }
        self.focus_window(Some(target));
        self.shell_set_overview(false);
    }

    fn shell_power(&mut self, item: usize) {
        self.shell.popover = None;
        match item {
            0 => {
                tracing::info!("log out");
                self.signal.stop();
            }
            1 | 2 => {
                let request = if item == 1 { "reboot" } else { "poweroff" };
                let result = std::os::unix::net::UnixStream::connect("/run/lumen/control").and_then(|mut s| {
                    use std::io::Write;
                    s.write_all(format!("{request}\n").as_bytes())
                });
                if let Err(e) = result {
                    let now = self.now();
                    self.shell.notify(&self.theme, "Cannot reach the system", &format!("{request}: {e}"), now);
                }
            }
            _ => {}
        }
        self.queue_redraw_all();
    }

    fn shell_toggle_popover(&mut self) {
        match &mut self.shell.popover {
            Some(p) if p.open => {
                p.open = false;
                p.anim.set_target(0.0);
            }
            _ => {
                let mut anim = Spring::new(0.0, self.shell.popover_params);
                anim.set_target(1.0);
                self.shell.popover = Some(Popover { anim, open: true });
            }
        }
        let focused = self.wm.focused;
        self.focus_window(focused);
        self.queue_redraw_all();
    }

    fn shell_close_popover(&mut self) {
        if let Some(p) = &mut self.shell.popover {
            p.open = false;
            p.anim.set_target(0.0);
            let focused = self.wm.focused;
            self.focus_window(focused);
        }
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
        if self.shell.popover.as_ref().is_some_and(|p| p.open) {
            if raw == keysyms::KEY_Escape {
                self.shell_close_popover();
            }
            return;
        }
        if !self.shell.overview_open {
            return;
        }
        let out = self.output_rect();
        let tiles = self.shell.visible_apps();
        match raw {
            keysyms::KEY_Escape => {
                if !self.shell.query.is_empty() {
                    self.shell.query.clear();
                    self.shell.update_results();
                } else {
                    self.shell_set_overview(false);
                }
            }
            keysyms::KEY_BackSpace => {
                self.shell.query.pop();
                self.shell.update_results();
            }
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => {
                if let Some(&app) = tiles.get(self.shell.selected) {
                    self.shell_launch(app);
                } else {
                    let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
                    let pick = self.shell.hovered.filter(|id| slots.contains_key(id));
                    let pick = pick.or_else(|| {
                        // With a query, Enter picks the first matching window.
                        let mut ids: Vec<(f64, f64, u32)> = slots
                            .iter()
                            .filter(|(id, _)| self.shell.window_matches(&self.wm, **id))
                            .map(|(id, r)| (r.loc.y, r.loc.x, *id))
                            .collect();
                        ids.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        ids.first().map(|t| t.2)
                    });
                    if let Some(id) = pick {
                        self.focus_window(Some(id));
                    }
                    self.shell_set_overview(false);
                }
            }
            keysyms::KEY_Left | keysyms::KEY_Right | keysyms::KEY_Up | keysyms::KEY_Down | keysyms::KEY_Tab if !tiles.is_empty() => {
                let n = tiles.len();
                let tiles_r = app_tiles(&self.theme, self.shell.results_area(&self.theme, out), n);
                let cols = tiles_r.iter().filter(|t| (t.loc.y - tiles_r[0].loc.y).abs() < 1.0).count().max(1);
                let s = self.shell.selected as isize;
                let next = match raw {
                    keysyms::KEY_Left => s - 1,
                    keysyms::KEY_Right | keysyms::KEY_Tab => s + 1,
                    keysyms::KEY_Up => s - cols as isize,
                    _ => s + cols as isize,
                };
                self.shell.selected = next.clamp(0, n as isize - 1) as usize;
            }
            keysyms::KEY_Left | keysyms::KEY_Right | keysyms::KEY_Up | keysyms::KEY_Down | keysyms::KEY_Tab => {
                let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
                self.shell.hovered = navigate(&slots, self.shell.hovered, raw);
            }
            _ => {
                // Typing starts or extends the search.
                if !mods.ctrl && !mods.alt && !mods.logo {
                    if let Some(text) = utf8.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
                        self.shell.query.push_str(&text);
                        self.shell.update_results();
                    }
                }
            }
        }
        self.queue_redraw_all();
    }

    /// Pointer moved; returns true if the shell owns the pointer here (clients
    /// must not get pointer focus).
    pub fn shell_pointer_motion(&mut self, pos: Point<f64, Logical>) -> bool {
        self.shell.pointer = pos;
        let out = self.output_rect();
        let hit = self.shell.hit_at(pos);
        if hit != self.shell.hover {
            self.shell.hover = hit;
            self.queue_redraw_all();
        }
        if self.shell.overview_open {
            let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
            let hovered = if hit.is_some() { None } else { slots.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id) };
            if hovered.is_some() || hit.is_some() {
                self.shell.hovered = hovered;
            }
            if let Some(Hit::App(i)) = hit {
                if let Some(p) = self.shell.visible_apps().iter().position(|a| *a == i) {
                    self.shell.selected = p;
                }
            }
            return true;
        }
        hit.is_some()
            || self.shell.switcher.is_some()
            || self.shell.popover.as_ref().is_some_and(|p| p.open)
            || (pos.y < out.loc.y + self.theme.geometry.panel_height && !self.wm.fullscreen_on_top())
    }

    /// Pointer button for the shell; returns true if it was consumed.
    pub fn shell_click(&mut self, button: u32, pos: Point<f64, Logical>) -> bool {
        let out = self.output_rect();
        if self.shell.switcher.is_some() {
            return true;
        }
        let hit = self.shell.hit_at(pos);
        if self.shell.popover.as_ref().is_some_and(|p| p.open) {
            match hit {
                Some(Hit::PowerItem(i)) => self.shell_power(i),
                Some(Hit::Popover) => {}
                _ => self.shell_close_popover(),
            }
            return true;
        }
        if button == BTN_LEFT {
            match hit {
                Some(Hit::Activities) => {
                    let open = !self.shell.overview_open;
                    self.shell_set_overview(open);
                    return true;
                }
                Some(Hit::Power) => {
                    self.shell_toggle_popover();
                    return true;
                }
                Some(Hit::DockItem(i)) => {
                    self.shell_dock_activate(i);
                    return true;
                }
                Some(Hit::DockApps) => {
                    self.shell_toggle_overview(true);
                    return true;
                }
                Some(Hit::App(i)) => {
                    self.shell_launch(i);
                    return true;
                }
                Some(Hit::Workspace(ws)) => {
                    self.switch_workspace(ws);
                    self.shell.hovered = None;
                    return true;
                }
                Some(Hit::Notification(id)) | Some(Hit::NotificationClose(id)) => {
                    self.shell.dismiss_notification(id);
                    return true;
                }
                Some(_) => return true,
                None => {}
            }
        } else if hit.is_some() {
            return true;
        }
        if self.shell.overview_open {
            let slots = self.shell.overview_layout(&self.wm, &self.theme, out).unwrap_or_default();
            let on_window = slots.iter().find(|(_, r)| r.contains(pos)).map(|(id, _)| *id).filter(|_| !self.shell.apps_mode);
            match (on_window, button) {
                (Some(id), BTN_LEFT) => {
                    self.focus_window(Some(id));
                    self.shell_set_overview(false);
                }
                (Some(id), BTN_MIDDLE) => self.close_window(id),
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
fn navigate(slots: &HashMap<u32, Rect>, from: Option<u32>, key: u32) -> Option<u32> {
    let center = |r: &Rect| (r.loc.x + r.size.w / 2.0, r.loc.y + r.size.h / 2.0);
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

    #[test]
    fn window_app_ids_match_desktop_entries() {
        let exec = Some(vec!["/usr/bin/lumen-term".to_string()]);
        assert!(app_matches("lumen-term", &exec, "lumen-term"));
        assert!(app_matches("org.gnome.Terminal", &None, "org.gnome.terminal"));
        assert!(app_matches("terminal", &None, "org.example.terminal"));
        assert!(app_matches("x", &exec, "lumen-term"));
        assert!(!app_matches("files", &None, "lumen-term"));
    }

    #[test]
    fn clock_text_shape() {
        let t = clock_text();
        assert!(t.len() >= 14 && t.contains(':'), "{t}");
    }
}
