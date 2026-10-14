//! Window management: windows, workspaces, focus order, placement and the
//! springs that animate every geometry change.
//!
//! Coordinates are logical and global (the primary output starts at 0,0).
//! `Win::target` is where the window *should* be; `Win::pos` is the animated
//! position that is actually rendered.

use std::time::Duration;

use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::{Window, WindowSurfaceType};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::seat::WaylandFocus;
use smithay::xwayland::X11Surface;

use crate::anim::{Spring, Spring2, SpringParams};
use crate::render::elements::Shape;
use crate::theme::Theme;

pub const WORKSPACES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tile {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    Floating,
    Maximized { restore: Rectangle<f64, Logical> },
    Tiled { side: Tile, restore: Rectangle<f64, Logical> },
    Fullscreen { restore: Rectangle<f64, Logical> },
}

/// A window stretching from where it was drawn to its new geometry after a
/// layout change (maximize, tile, restore), GNOME style.
pub struct Morph {
    /// Logical, global, without workspace offset.
    pub from: Rectangle<f64, Logical>,
    /// The old layout had square corners.
    pub from_square: bool,
    /// 0 → 1.
    pub t: Spring,
}

/// A layout change waiting for the client's buffer of the new size.
pub struct PendingMorph {
    pub from: Rectangle<f64, Logical>,
    pub from_square: bool,
    pub waited: Duration,
}

/// How long to wait for a resized buffer before moving without a morph.
const MORPH_WAIT: Duration = Duration::from_millis(400);

pub struct Win {
    pub id: u32,
    pub window: Window,
    pub ws: usize,
    pub target: Point<f64, Logical>,
    pub pos: Spring2,
    /// 0 = closed/invisible, 1 = fully open (drives scale and opacity).
    pub open: Spring,
    pub mapped: bool,
    pub layout: Layout,
    /// Shadow shape per output (each output's damage tracker needs its own).
    pub shadows: std::collections::HashMap<String, Shape>,
    /// Where the window was last drawn (logical, global, without the
    /// workspace offset; includes overview and open-animation transforms).
    pub last_drawn: Option<Rectangle<f64, Logical>>,
    /// When resizing from the top/left edges, keep this corner fixed.
    pub resize_anchor: Option<(Point<f64, Logical>, bool, bool)>,
    pub last_size: Size<i32, Logical>,
    /// Size the compositor asked for in its last configure. A commit with this
    /// size is the answer to that request (the window is already placed for
    /// it); any other size change was the client's own decision.
    pub requested_size: Option<Size<i32, Logical>>,
    pub pending_morph: Option<PendingMorph>,
    pub morph: Option<Morph>,
    /// The position was chosen before the first buffer (X11 windows are
    /// placed when they ask to be mapped, so the X server knows where they
    /// are before they draw).
    pub placed: bool,
}

impl Win {
    /// Square corners (maximized, fullscreen).
    pub fn is_square(&self) -> bool {
        matches!(self.layout, Layout::Maximized { .. } | Layout::Fullscreen { .. })
    }

    /// The client answered the layout change with a buffer of the new size:
    /// stretch from the old rectangle to the new one.
    pub fn start_morph(&mut self, params: SpringParams) {
        let Some(p) = self.pending_morph.take() else { return };
        let mut t = Spring::new(0.0, params);
        t.set_target(1.0);
        self.morph = Some(Morph { from: p.from, from_square: p.from_square, t });
        self.pos.snap(self.target.x, self.target.y);
    }

    /// The window's main surface. X11 windows only have one once Xwayland
    /// has associated it with the X window.
    pub fn surface(&self) -> Option<WlSurface> {
        self.window.wl_surface().map(|s| s.into_owned())
    }

    pub fn x11(&self) -> Option<&X11Surface> {
        self.window.x11_surface()
    }

    pub fn size(&self) -> Size<i32, Logical> {
        self.window.geometry().size
    }

    /// Animated geometry (logical, global, without workspace offset).
    pub fn geo(&self) -> Rectangle<f64, Logical> {
        let (x, y) = self.pos.value();
        Rectangle::new((x, y).into(), self.size().to_f64())
    }

    pub fn target_geo(&self) -> Rectangle<f64, Logical> {
        Rectangle::new(self.target, self.size().to_f64())
    }

    pub fn title(&self) -> String {
        if let Some(x) = self.x11() {
            return x.title();
        }
        self.window
            .toplevel()
            .and_then(|t| {
                smithay::wayland::compositor::with_states(t.wl_surface(), |s| {
                    s.data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().ok().and_then(|d| d.title.clone()))
                })
            })
            .unwrap_or_default()
    }

    /// The xdg app id, or for X11 windows the WM_CLASS class (what desktop
    /// files name in StartupWMClass), falling back to the instance name.
    pub fn app_id(&self) -> String {
        if let Some(x) = self.x11() {
            let class = x.class();
            return if class.is_empty() { x.instance() } else { class };
        }
        self.window
            .toplevel()
            .and_then(|t| {
                smithay::wayland::compositor::with_states(t.wl_surface(), |s| {
                    s.data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().ok().and_then(|d| d.app_id.clone()))
                })
            })
            .unwrap_or_default()
    }
}

/// A window that is gone but still animating out, drawn from a snapshot.
pub struct Closing {
    pub image: crate::render::elements::Image,
    /// Where it was last drawn (logical, global, without workspace offset).
    pub rect: Rectangle<f64, Logical>,
    pub ws: usize,
    pub progress: Spring,
    /// Windows that were above it, so the ghost keeps its stacking position.
    pub above: Vec<u32>,
}

pub enum GrabKind {
    Move { pointer_start: Point<f64, Logical>, win_start: Point<f64, Logical> },
    Resize { pointer_start: Point<f64, Logical>, start: Rectangle<f64, Logical>, left: bool, right: bool, top: bool, bottom: bool },
}

pub struct Grab {
    pub win: u32,
    pub button: u32,
    pub kind: GrabKind,
}

pub struct Wm {
    pub windows: Vec<Win>,
    /// X11 override-redirect windows (menus, tooltips, drag icons), bottom to
    /// top. Not managed: they sit where the X client put them, above every
    /// managed window.
    pub overrides: Vec<X11Surface>,
    pub closing: Vec<Closing>,
    pub current: usize,
    /// Animated workspace position (fractional while switching).
    pub ws_anim: Spring,
    /// Window ids in most-recently-focused order (front = most recent).
    pub mru: Vec<u32>,
    pub focused: Option<u32>,
    pub grab: Option<Grab>,
    next_id: u32,
    move_params: SpringParams,
    pub morph_params: SpringParams,
    open_params: SpringParams,
    close_params: SpringParams,
    ws_params: SpringParams,
}

impl Wm {
    pub fn new(theme: &Theme) -> Wm {
        let a = &theme.animation;
        Wm {
            windows: Vec::new(),
            overrides: Vec::new(),
            closing: Vec::new(),
            current: 0,
            ws_anim: Spring::new(0.0, theme.spring(a.workspace, 0.0005)),
            mru: Vec::new(),
            focused: None,
            grab: None,
            next_id: 1,
            move_params: theme.spring(a.window_move, 0.05),
            morph_params: theme.spring(a.window_move, 0.002),
            open_params: theme.spring(a.window_open, 0.001),
            close_params: theme.spring(a.window_close, 0.002),
            ws_params: theme.spring(a.workspace, 0.0005),
        }
    }

    pub fn add(&mut self, window: Window) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push(Win {
            id,
            window,
            ws: self.current,
            target: (0.0, 0.0).into(),
            pos: Spring2::new(0.0, 0.0, self.move_params),
            open: Spring::new(0.0, self.open_params),
            mapped: false,
            layout: Layout::Floating,
            shadows: std::collections::HashMap::new(),
            last_drawn: None,
            resize_anchor: None,
            last_size: Size::default(),
            requested_size: None,
            pending_morph: None,
            morph: None,
            placed: false,
        });
        id
    }

    pub fn index_of(&self, id: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    pub fn get(&self, id: u32) -> Option<&Win> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Win> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// The window whose main surface is `surface` (Wayland or X11).
    pub fn find_toplevel(&self, surface: &WlSurface) -> Option<u32> {
        self.windows
            .iter()
            .find(|w| match w.window.toplevel() {
                Some(t) => t.wl_surface() == surface,
                None => w.window.wl_surface().is_some_and(|s| *s == *surface),
            })
            .map(|w| w.id)
    }

    /// The managed window of an X11 window. Compared by X window id:
    /// `X11Surface` equality fails once the window is destroyed.
    pub fn find_x11(&self, x: &X11Surface) -> Option<u32> {
        self.windows.iter().find(|w| w.x11().is_some_and(|o| same_x11(o, x))).map(|w| w.id)
    }

    /// The window `id` is a dialog of: the xdg parent or the X11
    /// WM_TRANSIENT_FOR window.
    pub fn parent_of(&self, id: u32) -> Option<u32> {
        let w = self.get(id)?;
        if let Some(x) = w.x11() {
            let parent = x.is_transient_for()?;
            return self.windows.iter().find(|o| o.x11().is_some_and(|p| p.window_id() == parent)).map(|o| o.id);
        }
        let parent = w.window.toplevel()?.parent()?;
        self.find_toplevel(&parent)
    }

    /// Choose an initial position: centered in the work area, cascaded away
    /// from windows already at that spot.
    pub fn place(&self, size: Size<i32, Logical>, work: Rectangle<f64, Logical>, ws: usize) -> Point<f64, Logical> {
        let w = (size.w as f64).min(work.size.w);
        let h = (size.h as f64).min(work.size.h);
        let mut x = work.loc.x + (work.size.w - w) / 2.0;
        let mut y = work.loc.y + (work.size.h - h) / 2.0;
        for _ in 0..12 {
            let taken = self
                .windows
                .iter()
                .any(|o| o.mapped && o.ws == ws && (o.target.x - x).abs() < 8.0 && (o.target.y - y).abs() < 8.0);
            if !taken {
                break;
            }
            x += 32.0;
            y += 32.0;
            if x + w > work.loc.x + work.size.w || y + h > work.loc.y + work.size.h {
                x = work.loc.x + 24.0;
                y = work.loc.y + 24.0;
            }
        }
        (x.round(), y.round()).into()
    }

    /// Where a new window of `size` goes: dialogs centered over their parent,
    /// other windows by [`Wm::place`].
    pub fn place_window(&self, id: u32, size: Size<i32, Logical>, work: Rectangle<f64, Logical>) -> Point<f64, Logical> {
        if let Some(parent) = self.parent_of(id).and_then(|p| self.get(p)).filter(|p| p.mapped) {
            let pg = parent.target_geo();
            let x = pg.loc.x + (pg.size.w - size.w as f64) / 2.0;
            let y = pg.loc.y + (pg.size.h - size.h as f64) / 2.0;
            let x = x.min(work.loc.x + work.size.w - size.w as f64).max(work.loc.x);
            let y = y.min(work.loc.y + work.size.h - size.h as f64).max(work.loc.y);
            return (x.round(), y.round()).into();
        }
        self.place(size, work, self.current)
    }

    pub fn map(&mut self, id: u32, work: Rectangle<f64, Logical>) {
        let current = self.current;
        let size = match self.get(id) {
            Some(w) => w.size(),
            None => return,
        };
        // A window that asked for a layout (fullscreen, maximized) before its
        // first buffer is already placed by that layout.
        let floating = self.get(id).is_some_and(|w| matches!(w.layout, Layout::Floating) && !w.placed);
        let loc = if floating { self.place_window(id, size, work) } else { self.get(id).map(|w| w.target).unwrap_or_default() };
        if let Some(w) = self.get_mut(id) {
            w.mapped = true;
            w.ws = current;
            w.target = loc;
            w.pos.snap(loc.x, loc.y);
            w.open.snap(0.0);
            w.open.set_target(1.0);
            w.last_size = size;
        }
    }

    /// Move a window to the top of the stacking order, with its dialogs
    /// (and theirs) above it in their current order.
    pub fn raise(&mut self, id: u32) {
        let mut group = vec![id];
        let mut i = 0;
        while i < group.len() {
            let parent = group[i];
            for w in &self.windows {
                if !group.contains(&w.id) && self.parent_of(w.id) == Some(parent) {
                    group.push(w.id);
                }
            }
            i += 1;
        }
        // The raised window first, then its dialogs bottom to top.
        let order: Vec<u32> = std::iter::once(id)
            .chain(self.windows.iter().map(|w| w.id).filter(|w| *w != id && group.contains(w)))
            .collect();
        for wid in order {
            if let Some(i) = self.index_of(wid) {
                let w = self.windows.remove(i);
                self.windows.push(w);
            }
        }
    }

    /// Record focus; returns true if it changed.
    pub fn set_focused(&mut self, id: Option<u32>) -> bool {
        if self.focused == id {
            return false;
        }
        self.focused = id;
        if let Some(id) = id {
            self.mru.retain(|&x| x != id);
            self.mru.insert(0, id);
        }
        true
    }

    pub fn remove(&mut self, id: u32) -> Option<Win> {
        let i = self.index_of(id)?;
        self.mru.retain(|&x| x != id);
        if self.focused == Some(id) {
            self.focused = None;
        }
        if self.grab.as_ref().is_some_and(|g| g.win == id) {
            self.grab = None;
        }
        Some(self.windows.remove(i))
    }

    pub fn start_closing(&mut self, image: crate::render::elements::Image, rect: Rectangle<f64, Logical>, ws: usize, above: Vec<u32>) {
        let mut progress = Spring::new(1.0, self.close_params);
        progress.set_target(0.0);
        self.closing.push(Closing { image, rect, ws, progress, above });
    }

    /// Whether the topmost window on the current workspace is fullscreen
    /// (the panel and dock hide).
    pub fn fullscreen_on_top(&self) -> bool {
        self.windows
            .iter()
            .rev()
            .find(|w| w.mapped && w.ws == self.current)
            .is_some_and(|w| matches!(w.layout, Layout::Fullscreen { .. }))
    }

    /// An X11 override-redirect window covers `out`: an unmanaged
    /// fullscreen game (SDL 1.2, some Wine games), drawn above everything.
    pub fn covered_by_override(&self, out: Rectangle<f64, Logical>) -> bool {
        self.overrides.iter().any(|o| o.wl_surface().is_some() && o.geometry().to_f64().contains_rect(out))
    }

    /// Topmost mapped window on the current workspace (for focus fallback).
    pub fn top_on_current(&self) -> Option<u32> {
        self.windows.iter().rev().find(|w| w.mapped && w.ws == self.current).map(|w| w.id)
    }

    pub fn switch_workspace(&mut self, ws: usize) -> bool {
        let ws = ws.min(WORKSPACES - 1);
        if ws == self.current {
            return false;
        }
        self.current = ws;
        self.ws_anim.stiffness = self.ws_params.stiffness;
        self.ws_anim.set_target(ws as f64);
        true
    }

    /// Workspace x offset (in output widths) at which `ws` is drawn now.
    pub fn ws_offset(&self, ws: usize) -> f64 {
        ws as f64 - self.ws_anim.value
    }

    pub fn ws_visible(&self, ws: usize) -> bool {
        self.ws_offset(ws).abs() < 1.0
    }

    pub fn set_target(&mut self, id: u32, loc: Point<f64, Logical>, animate: bool) {
        if let Some(w) = self.get_mut(id) {
            // An explicit position (drags) replaces a pending morph.
            w.pending_morph = None;
            let moved = w.target != loc;
            w.target = loc;
            if animate {
                w.pos.set_target(loc.x, loc.y);
            } else {
                w.pos.snap(loc.x, loc.y);
            }
            // X clients position their menus from where the X server thinks
            // their window is: keep it in sync with ours.
            if moved && w.x11().is_some() {
                self.configure_x11(id);
            }
        }
    }

    /// Send an X11 window its geometry and states: the target position (or,
    /// during a top/left resize, where the anchored corner puts it), the
    /// requested size, and the maximized/fullscreen flags.
    pub fn configure_x11(&mut self, id: u32) {
        let Some(w) = self.get(id) else { return };
        let Some(x) = w.x11() else { return };
        if !x.alive() || x.is_override_redirect() {
            return;
        }
        let size = w.requested_size.unwrap_or_else(|| x.geometry().size);
        let loc = match w.resize_anchor {
            Some((anchor, left, top)) => Point::from((
                if left { anchor.x - size.w as f64 } else { w.target.x },
                if top { anchor.y - size.h as f64 } else { w.target.y },
            )),
            None => w.target,
        };
        let rect = Rectangle::new(loc.to_i32_round(), size);
        let _ = x.set_maximized(matches!(w.layout, Layout::Maximized { .. } | Layout::Tiled { .. }));
        let _ = x.set_fullscreen(matches!(w.layout, Layout::Fullscreen { .. }));
        if x.geometry() != rect {
            if let Err(e) = x.configure(rect) {
                tracing::debug!("X11 configure failed: {e}");
            }
        }
    }

    /// Ask the client for a new size (and states); the position follows on commit.
    pub fn configure(&mut self, id: u32, size: Option<Size<i32, Logical>>, maximized: bool, fullscreen: bool, tiled: Option<Tile>) {
        let Some(w) = self.get_mut(id) else { return };
        w.requested_size = size;
        if w.x11().is_some() {
            self.configure_x11(id);
            return;
        }
        let Some(t) = w.window.toplevel() else { return };
        t.with_pending_state(|s| {
            s.size = size;
            let set = |s: &mut smithay::wayland::shell::xdg::ToplevelState, st: xdg_toplevel::State, on: bool| {
                if on {
                    s.states.set(st);
                } else {
                    s.states.unset(st);
                }
            };
            set(s, xdg_toplevel::State::Maximized, maximized);
            set(s, xdg_toplevel::State::Fullscreen, fullscreen);
            let (l, r) = match tiled {
                Some(Tile::Left) => (true, false),
                Some(Tile::Right) => (false, true),
                None => (false, false),
            };
            let tiled_any = tiled.is_some() || maximized;
            set(s, xdg_toplevel::State::TiledLeft, l || maximized);
            set(s, xdg_toplevel::State::TiledRight, r || maximized);
            set(s, xdg_toplevel::State::TiledTop, tiled_any);
            set(s, xdg_toplevel::State::TiledBottom, tiled_any);
        });
        if t.is_initial_configure_sent() {
            t.send_pending_configure();
        }
    }

    pub fn set_layout(&mut self, id: u32, layout: Layout, work: Rectangle<f64, Logical>, output: Rectangle<f64, Logical>) {
        let Some(w) = self.get(id) else { return };
        let current_rect = w.target_geo();
        let restore = match w.layout {
            // Not shown yet (a layout requested before the first buffer):
            // restoring later gives a centered window of a pleasant size.
            Layout::Floating if !w.mapped || current_rect.size.w <= 0.0 => {
                let (rw, rh) = ((work.size.w * 0.6).round(), (work.size.h * 0.7).round());
                Rectangle::new(
                    ((work.loc.x + (work.size.w - rw) / 2.0).round(), (work.loc.y + (work.size.h - rh) / 2.0).round()).into(),
                    (rw, rh).into(),
                )
            }
            Layout::Floating => current_rect,
            Layout::Maximized { restore } | Layout::Tiled { restore, .. } | Layout::Fullscreen { restore } => restore,
        };
        let layout = match layout {
            Layout::Maximized { .. } => Layout::Maximized { restore },
            Layout::Tiled { side, .. } => Layout::Tiled { side, restore },
            Layout::Fullscreen { .. } => Layout::Fullscreen { restore },
            Layout::Floating => Layout::Floating,
        };
        let (rect, max, full, tile) = match layout {
            Layout::Floating => (restore, false, false, None),
            Layout::Maximized { .. } => (work, true, false, None),
            Layout::Tiled { side, .. } => {
                let half = (work.size.w / 2.0).floor();
                let x = if side == Tile::Left { work.loc.x } else { work.loc.x + work.size.w - half };
                (Rectangle::new((x, work.loc.y).into(), (half, work.size.h).into()), false, false, Some(side))
            }
            Layout::Fullscreen { .. } => (output, false, true, None),
        };
        let size = Size::from((rect.size.w.round() as i32, rect.size.h.round() as i32));
        let mut morph = false;
        if let Some(w) = self.get_mut(id) {
            let from_square = w.is_square();
            w.layout = layout;
            w.resize_anchor = None;
            // X11 windows report the new size at once (it is the X geometry,
            // not the client's buffer), so they cannot wait for it: they slide.
            if w.mapped && size != w.size() && w.x11().is_none() {
                // Stay put until the client has a buffer of the new size, then
                // stretch from where the window is drawn now.
                let from = if w.morph.is_some() { w.last_drawn.unwrap_or_else(|| w.geo()) } else { w.geo() };
                w.pending_morph = Some(PendingMorph { from, from_square, waited: Duration::ZERO });
                w.target = rect.loc;
                morph = true;
            }
        }
        if !morph {
            self.set_target(id, rect.loc, true);
        }
        self.configure(id, Some(size), max, full, tile);
    }

    pub fn any_animating(&self) -> bool {
        !self.ws_anim.is_settled()
            || !self.closing.is_empty()
            || self.windows.iter().any(|w| {
                w.mapped && (!w.pos.is_settled() || !w.open.is_settled() || w.pending_morph.is_some() || w.morph.is_some())
            })
    }

    pub fn advance(&mut self, dt: Duration) {
        self.ws_anim.step(dt);
        for w in &mut self.windows {
            w.pos.step(dt);
            w.open.step(dt);
            if let Some(p) = &mut w.pending_morph {
                p.waited += dt;
                if p.waited > MORPH_WAIT {
                    // The client did not resize (in time): just move.
                    w.pending_morph = None;
                    w.pos.set_target(w.target.x, w.target.y);
                }
            }
            if let Some(m) = &mut w.morph {
                m.t.step(dt);
                if m.t.is_settled() {
                    w.morph = None;
                }
            }
        }
        for c in &mut self.closing {
            c.progress.step(dt);
        }
        self.closing.retain(|c| !c.progress.is_settled());
    }

    /// The window and surface under a global logical point (current workspace).
    pub fn surface_under(&self, p: Point<f64, Logical>) -> Option<(Option<u32>, WlSurface, Point<f64, Logical>)> {
        if let Some((surface, loc)) = self.override_surface_under(p) {
            return Some((None, surface, loc));
        }
        for w in self.windows.iter().rev() {
            if !w.mapped || w.ws != self.current || w.open.target == 0.0 {
                continue;
            }
            let geo = w.geo();
            let render_loc = geo.loc - w.window.geometry().loc.to_f64();
            if let Some((surface, loc)) = w.window.surface_under(p - render_loc, WindowSurfaceType::ALL) {
                return Some((Some(w.id), surface, loc.to_f64() + render_loc));
            }
        }
        None
    }

    /// The override-redirect window (X11 menu, tooltip) under a point.
    pub fn override_surface_under(&self, p: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        for x in self.overrides.iter().rev() {
            let geo = x.geometry().to_f64();
            if !geo.contains(p) {
                continue;
            }
            let Some(surface) = x.wl_surface() else { continue };
            if let Some((s, loc)) = under_from_surface_tree(&surface, p - geo.loc, (0, 0), WindowSurfaceType::ALL) {
                return Some((s, loc.to_f64() + geo.loc));
            }
        }
        None
    }

    /// Whether an override-redirect window (an X11 menu) is under a point.
    pub fn override_under(&self, p: Point<f64, Logical>) -> bool {
        self.override_surface_under(p).is_some()
    }

    /// Window (not surface) under a point, by geometry only.
    pub fn window_under(&self, p: Point<f64, Logical>) -> Option<u32> {
        self.windows
            .iter()
            .rev()
            .find(|w| w.mapped && w.ws == self.current && w.geo().contains(p))
            .map(|w| w.id)
    }
}

/// The same X11 window (by id, even after it was destroyed).
pub fn same_x11(a: &X11Surface, b: &X11Surface) -> bool {
    a.window_id() == b.window_id() && a.xwm_id() == b.xwm_id()
}
