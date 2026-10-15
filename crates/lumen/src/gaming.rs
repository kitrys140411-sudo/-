//! Protocols games (and video players, VMs, remote desktops) rely on:
//!
//! * pointer constraints + relative pointer: FPS mouse look. A lock keeps the
//!   pointer where it is and only relative motion reaches the game; the
//!   cursor is hidden meanwhile. A confinement keeps it inside a region.
//!   Constraints only hold for the focused window: switching windows (Alt+Tab,
//!   Super for the overview) releases them, so the user can always escape.
//!   X11 override-redirect windows (unmanaged fullscreen games; Xwayland turns
//!   their pointer grabs into constraints) count as focused while X11 has the
//!   keyboard, e.g. through their keyboard grab. A confined pointer slides
//!   along the region's edge, and the hot corner is off while a constraint
//!   holds or a fullscreen game is on top.
//! * keyboard shortcuts inhibit: while granted, keys go to the window instead
//!   of triggering shortcuts, except those with Super and console switching.
//! * idle inhibit: a visible surface with an inhibitor (or a fullscreen window
//!   on top) keeps the screen on; otherwise outputs are blanked (DPMS off on
//!   KMS) after `LUMEN_IDLE_BLANK` minutes without input (default 10, 0 =
//!   never; `idle-timeout <s>` on the control socket changes it at runtime).

use std::time::Duration;

use smithay::input::pointer::{PointerHandle, RelativeMotionEvent};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{IsAlive, Logical, Point};
use smithay::wayland::compositor::{get_parent, RectangleKind, RegionAttributes};
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor, KeyboardShortcutsInhibitorSeat,
};
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint, PointerConstraintsHandler};

use crate::focus::FocusTarget;
use crate::state::Lumen;

/// What an active pointer constraint does to motion.
#[derive(Debug, Clone)]
pub enum Constraint {
    /// The pointer stays put; only relative motion is delivered.
    Locked,
    /// The pointer stays inside the region (surface-local; None: the surface).
    Confined(Option<smithay::wayland::compositor::RegionAttributes>),
}

/// Idle tracking and screen blanking.
pub struct Idle {
    /// Last user input (presentation clock).
    pub last_input: Duration,
    /// Blank after this long without input (None: never).
    pub timeout: Option<Duration>,
    pub blanked: bool,
    /// Surfaces with an idle inhibitor (only visible ones count).
    pub inhibitors: Vec<WlSurface>,
    timer: Option<RegistrationToken>,
}

impl Idle {
    pub fn new() -> Idle {
        let timeout = blank_timeout(std::env::var("LUMEN_IDLE_BLANK").ok().as_deref());
        Idle { last_input: Duration::ZERO, timeout, blanked: false, inhibitors: Vec::new(), timer: None }
    }
}

/// `LUMEN_IDLE_BLANK` (minutes, fractions allowed; 0 = never) as a timeout;
/// unset or unparsable: 10 minutes.
fn blank_timeout(minutes: Option<&str>) -> Option<Duration> {
    let minutes = minutes.and_then(|v| v.trim().parse::<f64>().ok()).filter(|m| m.is_finite()).unwrap_or(10.0);
    (minutes > 0.0).then(|| Duration::from_secs_f64(minutes * 60.0))
}

/// The toplevel surface of a (sub)surface.
fn root_surface(surface: &WlSurface) -> WlSurface {
    let mut root = surface.clone();
    while let Some(parent) = get_parent(&root) {
        root = parent;
    }
    root
}

impl Lumen {
    /// The focused window's main surface.
    fn focused_surface(&self) -> Option<WlSurface> {
        self.wm.focused.and_then(|id| self.wm.get(id)).and_then(|w| w.surface())
    }

    /// The active constraint of the surface under the pointer, if any.
    pub fn active_constraint(&self) -> Option<(WlSurface, Point<f64, Logical>, Constraint)> {
        let (surface, origin) = self.pointer_target.clone()?;
        let pointer = self.seat.get_pointer()?;
        let c = with_pointer_constraint(&surface, &pointer, |c| {
            let c = c.filter(|c| c.is_active())?;
            Some(match &*c {
                PointerConstraint::Locked(_) => Constraint::Locked,
                PointerConstraint::Confined(_) => Constraint::Confined(c.region().cloned()),
            })
        })?;
        Some((surface, origin, c))
    }

    /// Whether the constraints of the surface tree `root` may hold: it is
    /// the focused window, or an X11 override-redirect window while an X11
    /// surface has the keyboard (its own keyboard grab, its app's window) or,
    /// with nothing focused, while it covers the output.
    fn constraint_allowed(&self, root: &WlSurface) -> bool {
        if self.shell.wants_keyboard() {
            return false;
        }
        if self.focused_surface().as_ref() == Some(root) {
            return true;
        }
        let Some(or) = self.wm.override_with(root).map(|o| &o.x) else { return false };
        let nothing_focused = self.seat.get_keyboard().is_none_or(|k| k.current_focus().is_none());
        self.x11_focused() || (nothing_focused && or.geometry().to_f64().contains_rect(self.output_rect()))
    }

    /// Bring constraints in line with focus: the one of the surface under the
    /// pointer becomes active when its window has keyboard focus and the
    /// pointer is inside its region; any other active one is released.
    pub fn update_pointer_constraints(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        let target = self.pointer_target.clone();
        // Release a lock held by a window that lost focus or the pointer.
        if let Some(prev) = self.constrained.take() {
            let keep = prev.alive() && target.as_ref().is_some_and(|(s, _)| *s == prev) && self.constraint_allowed(&root_surface(&prev));
            if keep {
                self.constrained = Some(prev);
            } else if prev.alive() {
                with_pointer_constraint(&prev, &pointer, |c| {
                    if let Some(c) = c.filter(|c| c.is_active()) {
                        c.deactivate();
                    }
                });
            }
        }
        let Some((surface, origin)) = target else { return };
        if !self.constraint_allowed(&root_surface(&surface)) {
            return;
        }
        let local = (self.pointer_pos - origin).to_i32_round();
        let activated = with_pointer_constraint(&surface, &pointer, |c| {
            let Some(c) = c else { return false };
            if !c.is_active() && c.region().is_none_or(|r| r.contains(local)) {
                c.activate();
            }
            c.is_active()
        });
        if activated {
            self.constrained = Some(surface);
            self.queue_redraw_all();
        }
    }

    /// Pointer motion under a constraint. Returns true if the motion was
    /// consumed (locked: only relative motion was sent).
    pub fn constrained_motion(
        &mut self,
        pos: &mut Point<f64, Logical>,
        relative: Option<(Point<f64, Logical>, Point<f64, Logical>, u64)>,
    ) -> bool {
        let Some((surface, origin, c)) = self.active_constraint() else { return false };
        let Some(pointer) = self.seat.get_pointer() else { return false };
        match c {
            Constraint::Locked => {
                if let Some((delta, delta_unaccel, utime)) = relative {
                    let focus = Some((FocusTarget::Surface(surface), origin));
                    pointer.relative_motion(self, focus, &RelativeMotionEvent { delta, delta_unaccel, utime });
                    pointer.frame(self);
                }
                true
            }
            Constraint::Confined(region) => {
                // Keep the pointer inside: clamp to the surface, and slide
                // along the region's edge (the region may also have shrunk
                // away from the pointer: then it moves to the nearest point).
                let size = smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |s| s.surface_size()).flatten();
                let mut local = *pos - origin;
                if let Some(size) = size {
                    local.x = local.x.clamp(0.0, (size.w - 1).max(0) as f64);
                    local.y = local.y.clamp(0.0, (size.h - 1).max(0) as f64);
                }
                if let Some(r) = region.as_ref() {
                    // Regions are in the client's coordinates (Xwayland may
                    // have a client scale).
                    let cs = surface.client().and_then(|c| c.get_data::<smithay::xwayland::XWaylandClientData>().map(|d| d.compositor_state.client_scale())).unwrap_or(1.0);
                    let from = (self.pointer_pos - origin).upscale(cs);
                    local = confine_to_region(r, from, local.upscale(cs)).downscale(cs);
                }
                *pos = origin + local;
                false
            }
        }
    }

    /// Whether the cursor should be hidden (the pointer is locked).
    pub fn pointer_locked(&self) -> bool {
        matches!(self.active_constraint(), Some((_, _, Constraint::Locked)))
    }

    /// Whether the focused window inhibits compositor shortcuts.
    pub fn shortcuts_inhibited(&self) -> bool {
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else { return false };
        let Some(surface) = smithay::wayland::seat::WaylandFocus::wl_surface(&focus).map(|s| s.into_owned()) else { return false };
        self.seat.keyboard_shortcuts_inhibitor_for_surface(&surface).is_some_and(|i| i.is_active())
    }

    // -- idle ----------------------------------------------------------------------

    /// User input happened: wake the screen and restart the idle timer.
    pub fn note_input(&mut self) {
        self.idle.last_input = self.now();
        if self.idle.blanked {
            self.set_blanked(false);
        }
        if self.idle.timer.is_none() {
            self.arm_idle_timer();
        }
    }

    /// Something keeps the screen on: a visible surface with an idle
    /// inhibitor, or a fullscreen window on top (games and players that do
    /// not use the protocol, X11 ones included).
    pub fn idle_inhibited(&self) -> bool {
        if self.fullscreen_game() {
            return true;
        }
        self.idle.inhibitors.iter().any(|s| {
            s.alive() && {
                let root = root_surface(s);
                self.wm.find_toplevel(&root).and_then(|id| self.wm.get(id)).is_some_and(|w| w.mapped && w.ws == self.wm.current)
                    || self.wm.override_with(&root).is_some_and(|o| o.ws == self.wm.current)
            }
        })
    }

    /// A fullscreen window is on top, or an X11 override-redirect window
    /// covers the primary output (older SDL and Wine games go fullscreen
    /// that way, unmanaged).
    pub fn fullscreen_game(&self) -> bool {
        self.wm.fullscreen_on_top() || self.wm.covered_by_override(self.output_rect())
    }

    pub fn set_idle_timeout(&mut self, timeout: Option<Duration>) {
        self.idle.timeout = timeout;
        self.idle.last_input = self.now();
        if let Some(t) = self.idle.timer.take() {
            self.lh.remove(t);
        }
        self.arm_idle_timer();
    }

    /// Check for idleness when the timeout would expire (re-armed as input
    /// keeps arriving, so there is no periodic wakeup while busy).
    fn arm_idle_timer(&mut self) {
        let Some(timeout) = self.idle.timeout else { return };
        let due = (self.idle.last_input + timeout).saturating_sub(self.now()).max(Duration::from_millis(50));
        let token = self.lh.insert_source(Timer::from_duration(due), |_, _, state| {
            state.idle.timer = None;
            let Some(timeout) = state.idle.timeout else { return TimeoutAction::Drop };
            let idle_for = state.now().saturating_sub(state.idle.last_input);
            if idle_for >= timeout {
                if state.idle_inhibited() {
                    // Look again later: the inhibitor may go away.
                    state.idle.last_input = state.now();
                } else if !state.idle.blanked {
                    state.set_blanked(true);
                    return TimeoutAction::Drop;
                }
            }
            state.arm_idle_timer();
            TimeoutAction::Drop
        });
        self.idle.timer = token.ok();
    }

    fn set_blanked(&mut self, blanked: bool) {
        self.idle.blanked = blanked;
        tracing::info!(blanked, "screen blanking");
        self.backend.set_blanked(blanked);
        if !blanked {
            self.queue_redraw_all();
        }
    }
}

/// Where a pointer confined to `region` (surface-local) goes when it moves
/// from `from` toward `to`: there if inside, else along the axis that stays
/// inside (sliding along an edge), else the nearest point of the region.
fn confine_to_region(region: &RegionAttributes, from: Point<f64, Logical>, to: Point<f64, Logical>) -> Point<f64, Logical> {
    let inside = |p: Point<f64, Logical>| region.contains(p.to_i32_round());
    if inside(to) {
        return to;
    }
    for p in [Point::from((to.x, from.y)), Point::from((from.x, to.y))] {
        if inside(p) && inside(from) {
            return p;
        }
    }
    nearest_in_region(region, to).unwrap_or(from)
}

/// The point of `region` closest to `p` (None: the region is empty).
fn nearest_in_region(region: &RegionAttributes, p: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
    region
        .rects
        .iter()
        .filter(|(kind, r)| matches!(kind, RectangleKind::Add) && r.size.w > 0 && r.size.h > 0)
        .map(|(_, r)| {
            let x = p.x.clamp(r.loc.x as f64, (r.loc.x + r.size.w - 1) as f64);
            let y = p.y.clamp(r.loc.y as f64, (r.loc.y + r.size.h - 1) as f64);
            Point::<f64, Logical>::from((x, y))
        })
        .filter(|q| region.contains(q.to_i32_round()))
        .min_by(|a, b| {
            let d = |q: &Point<f64, Logical>| (q.x - p.x).powi(2) + (q.y - p.y).powi(2);
            d(a).total_cmp(&d(b))
        })
}

impl PointerConstraintsHandler for Lumen {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        self.update_pointer_constraints();
    }

    /// A locked client says where it draws its cursor: the pointer follows,
    /// so it is there when the lock ends.
    fn cursor_position_hint(&mut self, surface: &WlSurface, _pointer: &PointerHandle<Self>, location: Point<f64, Logical>) {
        if let Some((s, origin, Constraint::Locked)) = self.active_constraint() {
            if &s == surface {
                self.pointer_pos = origin + location;
            }
        }
    }
}

impl KeyboardShortcutsInhibitHandler for Lumen {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.shortcuts_inhibit_state
    }

    /// Granted without asking (games, VMs, remote desktops want it); Super
    /// shortcuts keep working, so the user is never locked in.
    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        tracing::info!(surface = ?inhibitor.wl_surface().id(), "keyboard shortcuts inhibited");
        inhibitor.activate();
    }

    fn inhibitor_destroyed(&mut self, _inhibitor: KeyboardShortcutsInhibitor) {}
}

impl IdleInhibitHandler for Lumen {
    fn inhibit(&mut self, surface: WlSurface) {
        if !self.idle.inhibitors.contains(&surface) {
            self.idle.inhibitors.push(surface);
        }
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle.inhibitors.retain(|s| s != &surface && s.alive());
    }
}

smithay::delegate_pointer_constraints!(Lumen);
smithay::delegate_relative_pointer!(Lumen);
smithay::delegate_keyboard_shortcuts_inhibit!(Lumen);
smithay::delegate_idle_inhibit!(Lumen);

#[cfg(test)]
mod tests {
    use super::*;

    fn region(rects: &[(RectangleKind, (i32, i32, i32, i32))]) -> RegionAttributes {
        RegionAttributes { rects: rects.iter().map(|(k, (x, y, w, h))| (*k, smithay::utils::Rectangle::new((*x, *y).into(), (*w, *h).into()))).collect() }
    }

    fn pt(x: f64, y: f64) -> Point<f64, Logical> {
        (x, y).into()
    }

    #[test]
    fn confined_pointer_slides_and_recovers() {
        let r = region(&[(RectangleKind::Add, (100, 100, 200, 100))]);
        // Inside: moves freely.
        assert_eq!(confine_to_region(&r, pt(150.0, 150.0), pt(160.0, 170.0)), pt(160.0, 170.0));
        // Diagonally out through the right edge: slides along it.
        assert_eq!(confine_to_region(&r, pt(290.0, 150.0), pt(320.0, 160.0)), pt(290.0, 160.0));
        // Out through the bottom: keeps x.
        assert_eq!(confine_to_region(&r, pt(150.0, 190.0), pt(170.0, 260.0)), pt(170.0, 190.0));
        // The region shrank away from the pointer: the next move lands on its nearest point.
        let small = region(&[(RectangleKind::Add, (100, 100, 50, 50))]);
        assert_eq!(confine_to_region(&small, pt(250.0, 180.0), pt(252.0, 181.0)), pt(149.0, 149.0));
        // A hole (subtracted rectangle) is never entered.
        let holed = region(&[(RectangleKind::Add, (0, 0, 100, 100)), (RectangleKind::Subtract, (40, 40, 20, 20))]);
        let p = confine_to_region(&holed, pt(30.0, 50.0), pt(50.0, 50.0));
        assert!(holed.contains(p.to_i32_round()), "{p:?}");
        assert_eq!(nearest_in_region(&region(&[]), pt(1.0, 1.0)), None);
    }

    #[test]
    fn blank_timeout_from_env() {
        assert_eq!(blank_timeout(None), Some(Duration::from_secs(600)));
        assert_eq!(blank_timeout(Some("5")), Some(Duration::from_secs(300)));
        assert_eq!(blank_timeout(Some(" 0.5 ")), Some(Duration::from_secs(30)));
        assert_eq!(blank_timeout(Some("0")), None);
        assert_eq!(blank_timeout(Some("-1")), None);
        assert_eq!(blank_timeout(Some("soon")), Some(Duration::from_secs(600)));
        assert_eq!(blank_timeout(Some("inf")), Some(Duration::from_secs(600)));
    }
}
