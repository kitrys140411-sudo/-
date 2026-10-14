//! Protocols games use: pointer lock/confinement, relative pointer motion,
//! idle inhibition and keyboard shortcut inhibition (`--lock-pointer`,
//! `--confine-pointer`, `--relative`, `--idle-inhibit`, `--inhibit-shortcuts`).
//!
//! Output lines:
//!   lock event=locked|unlocked            confine event=confined|unconfined
//!   relative dx=.. dy=.. dx_unaccel=.. dy_unaccel=..
//!   idle-inhibit event=created            shortcuts-inhibit event=active|inactive

use crate::sctk;
use sctk::reexports::client::globals::GlobalList;
use sctk::reexports::client::protocol::{wl_pointer, wl_seat, wl_surface};
use sctk::reexports::client::{Connection, Dispatch, QueueHandle};
use sctk::reexports::protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1;
use sctk::reexports::protocols::wp::idle_inhibit::zv1::client::zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1;
use sctk::reexports::protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1;
use sctk::reexports::protocols::wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibitor_v1::{
    self, ZwpKeyboardShortcutsInhibitorV1,
};
use sctk::reexports::protocols::wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::Lifetime;
use sctk::reexports::protocols::wp::pointer_constraints::zv1::client::{zwp_confined_pointer_v1, zwp_locked_pointer_v1};
use sctk::reexports::protocols::wp::relative_pointer::zv1::client::zwp_relative_pointer_v1;
use sctk::seat::pointer_constraints::{PointerConstraintsHandler, PointerConstraintsState};
use sctk::seat::relative_pointer::{RelativeMotionEvent, RelativePointerHandler, RelativePointerState};
use sctk::shell::WaylandSurface;
use sctk::{delegate_pointer_constraints, delegate_relative_pointer};

use crate::App;
use crate::args::Options;
use crate::out::emit;

/// The globals and objects of the gaming protocols in use.
#[derive(Default)]
pub struct Gaming {
    relative_state: Option<RelativePointerState>,
    constraints: Option<PointerConstraintsState>,
    idle_manager: Option<ZwpIdleInhibitManagerV1>,
    shortcuts_manager: Option<ZwpKeyboardShortcutsInhibitManagerV1>,
    relative_pointer: Option<zwp_relative_pointer_v1::ZwpRelativePointerV1>,
    locked: Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    confined: Option<zwp_confined_pointer_v1::ZwpConfinedPointerV1>,
    idle_inhibitor: Option<ZwpIdleInhibitorV1>,
    shortcuts_inhibitor: Option<ZwpKeyboardShortcutsInhibitorV1>,
}

impl Gaming {
    /// Binds the globals the options need; a missing one is reported on stderr.
    pub fn bind(globals: &GlobalList, qh: &QueueHandle<App>, opts: &Options) -> Gaming {
        let mut g = Gaming::default();
        let missing = |name: &str| eprintln!("lumen-testclient: {name} not available, ignoring");
        if opts.relative {
            g.relative_state = Some(RelativePointerState::bind(globals, qh));
        }
        if opts.lock_pointer || opts.confine_pointer {
            g.constraints = Some(PointerConstraintsState::bind(globals, qh));
        }
        if opts.idle_inhibit {
            g.idle_manager = globals.bind(qh, 1..=1, ()).ok();
            if g.idle_manager.is_none() {
                missing("zwp_idle_inhibit_manager_v1");
            }
        }
        if opts.inhibit_shortcuts {
            g.shortcuts_manager = globals.bind(qh, 1..=1, ()).ok();
            if g.shortcuts_manager.is_none() {
                missing("zwp_keyboard_shortcuts_inhibit_manager_v1");
            }
        }
        g
    }
}

impl App {
    /// The pointer exists: ask for relative motion and the constraint. The
    /// compositor activates the constraint once the pointer is on the window.
    pub(crate) fn gaming_pointer_ready(&mut self, pointer: &wl_pointer::WlPointer) {
        let surface = self.window.wl_surface().clone();
        let qh = self.qh.clone();
        let g = &mut self.gaming;
        if let Some(rel) = &g.relative_state {
            match rel.get_relative_pointer(pointer, &qh) {
                Ok(p) => g.relative_pointer = Some(p),
                Err(e) => eprintln!("lumen-testclient: zwp_relative_pointer_manager_v1: {e}"),
            }
        }
        if let Some(c) = &g.constraints {
            if self.opts.lock_pointer {
                match c.lock_pointer(&surface, pointer, None, Lifetime::Persistent, &qh) {
                    Ok(l) => g.locked = Some(l),
                    Err(e) => eprintln!("lumen-testclient: zwp_pointer_constraints_v1: {e}"),
                }
            } else if self.opts.confine_pointer {
                match c.confine_pointer(&surface, pointer, None, Lifetime::Persistent, &qh) {
                    Ok(cp) => g.confined = Some(cp),
                    Err(e) => eprintln!("lumen-testclient: zwp_pointer_constraints_v1: {e}"),
                }
            }
        }
    }

    /// The seat is known: ask for every key.
    pub(crate) fn gaming_seat_ready(&mut self, seat: &wl_seat::WlSeat) {
        let surface = self.window.wl_surface().clone();
        if let (Some(m), None) = (&self.gaming.shortcuts_manager, &self.gaming.shortcuts_inhibitor) {
            self.gaming.shortcuts_inhibitor = Some(m.inhibit_shortcuts(&surface, seat, &self.qh, ()));
        }
    }

    /// The window is about to be shown: keep the screen on while it is.
    pub(crate) fn gaming_surface_ready(&mut self) {
        let surface = self.window.wl_surface().clone();
        if let (Some(m), None) = (&self.gaming.idle_manager, &self.gaming.idle_inhibitor) {
            self.gaming.idle_inhibitor = Some(m.create_inhibitor(&surface, &self.qh, ()));
            emit("idle-inhibit event=created");
        }
    }
}

impl RelativePointerHandler for App {
    fn relative_pointer_motion(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        _: &wl_pointer::WlPointer,
        event: RelativeMotionEvent,
    ) {
        emit(&format!(
            "relative dx={} dy={} dx_unaccel={} dy_unaccel={}",
            event.delta.0, event.delta.1, event.delta_unaccel.0, event.delta_unaccel.1
        ));
    }
}

impl PointerConstraintsHandler for App {
    fn confined(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        emit("confine event=confined");
    }

    fn unconfined(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        emit("confine event=unconfined");
    }

    fn locked(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        emit("lock event=locked");
    }

    fn unlocked(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        _: &wl_surface::WlSurface,
        _: &wl_pointer::WlPointer,
    ) {
        emit("lock event=unlocked");
    }
}

impl Dispatch<ZwpIdleInhibitManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpIdleInhibitManagerV1,
        _: <ZwpIdleInhibitManagerV1 as sctk::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpIdleInhibitorV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpIdleInhibitorV1,
        _: <ZwpIdleInhibitorV1 as sctk::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitManagerV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitManagerV1,
        _: <ZwpKeyboardShortcutsInhibitManagerV1 as sctk::reexports::client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpKeyboardShortcutsInhibitorV1, ()> for App {
    fn event(
        _: &mut Self,
        _: &ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => emit("shortcuts-inhibit event=active"),
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => emit("shortcuts-inhibit event=inactive"),
            _ => {}
        }
    }
}

delegate_relative_pointer!(App);
delegate_pointer_constraints!(App);
